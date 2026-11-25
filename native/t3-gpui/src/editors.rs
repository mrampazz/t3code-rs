//! Open the thread's folder in an editor (web: `OpenInPicker.tsx`, `remoteOpen.ts`,
//! `editorPreferences.ts`). A loopback server launches the editor itself (`shell.openInEditor`);
//! a remote one is opened over SSH with an editor URL when the server advertises a route.
//! shortcut: remote opens offer only VS Code (the web's fallback for browsers) and ignore saved
//! SSH profiles, which this client has none of; probe local editors if users ask for more.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Sizable, h_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::{t, tc, tv};
use crate::{T3App, on_tokio};

/// `EDITORS` from `packages/contracts/src/editor.ts`, in its order (the default pick is the first
/// available one).
const EDITORS: [(&str, &str); 21] = [
    ("cursor", "Cursor"),
    ("trae", "Trae"),
    ("kiro", "Kiro"),
    ("vscode", "VS Code"),
    ("vscode-insiders", "VS Code Insiders"),
    ("vscodium", "VSCodium"),
    ("zed", "Zed"),
    ("antigravity", "Antigravity"),
    ("idea", "IntelliJ IDEA"),
    ("aqua", "Aqua"),
    ("clion", "CLion"),
    ("datagrip", "DataGrip"),
    ("dataspell", "DataSpell"),
    ("goland", "GoLand"),
    ("phpstorm", "PhpStorm"),
    ("pycharm", "PyCharm"),
    ("rider", "Rider"),
    ("rubymine", "RubyMine"),
    ("rustrover", "RustRover"),
    ("webstorm", "WebStorm"),
    ("file-manager", "File Manager"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum OpenMode {
    /// The server runs on this machine and launches the editor.
    Local,
    /// The editor opens the server's folder over SSH.
    Remote {
        host: String,
        username: Option<String>,
    },
    Unavailable,
}

/// Web `resolveRemoteOpenState`, for a client that only knows the server URL and its
/// advertised `remoteOpenTargets`.
pub fn open_mode(server_url: &str, remote_targets: &[Value]) -> OpenMode {
    let loopback = url::Url::parse(server_url)
        .ok()
        .is_some_and(|u| match u.host() {
            Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        });
    if loopback {
        return OpenMode::Local;
    }
    match remote_targets.first().and_then(|t| t["host"].as_str()) {
        Some(host) => OpenMode::Remote {
            host: host.to_owned(),
            username: remote_targets[0]["username"].as_str().map(str::to_owned),
        },
        None => OpenMode::Unavailable,
    }
}

pub fn label(id: &str) -> String {
    if id == "file-manager" {
        return if cfg!(target_os = "macos") {
            "Finder".into()
        } else if cfg!(windows) {
            tc("File Explorer", "the Windows file manager app")
        } else {
            tc("Files", "the Linux file manager app")
        };
    }
    EDITORS.iter().find(|(e, _)| *e == id).map_or_else(
        || tc("Editor", "noun, a code editor app"),
        |(_, l)| (*l).to_owned(),
    )
}

/// The saved editor when still offered, else the first offered in `EDITORS` order.
pub fn preferred(saved: Option<&str>, offered: &[String]) -> Option<String> {
    saved
        .filter(|s| offered.iter().any(|o| o == s))
        .map(str::to_owned)
        .or_else(|| {
            EDITORS
                .iter()
                .find(|(id, _)| offered.iter().any(|o| o == id))
                .map(|(id, _)| (*id).to_owned())
        })
}

fn encode_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Contract `buildRemoteOpenUrl` for VS Code-style (`goto`) editors opening a folder.
pub fn remote_folder_url(scheme: &str, host: &str, username: Option<&str>, path: &str) -> String {
    let posix = path.replace('\\', "/");
    let rooted = if posix.starts_with('/') {
        posix
    } else {
        format!("/{posix}")
    };
    let encoded_path = rooted
        .split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/");
    let plain = username.map_or_else(|| host.to_owned(), |u| format!("{u}@{host}"));
    // Remote-SSH splits the authority on "+", so such destinations use hex-encoded JSON.
    let destination = if plain.contains('+') {
        let mut target = serde_json::Map::new();
        target.insert("hostName".into(), host.into());
        if let Some(u) = username {
            target.insert("user".into(), u.into());
        }
        Value::Object(target)
            .to_string()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect()
    } else {
        plain
    };
    format!(
        "{scheme}://vscode-remote/ssh-remote+{}{encoded_path}?windowId=_blank",
        encode_component(&destination)
    )
}

impl T3App {
    fn open_mode(&self) -> OpenMode {
        self.env().map_or(OpenMode::Unavailable, |e| {
            open_mode(&e.key, &e.config.remote_open_targets)
        })
    }

    fn offered_editors(&self, mode: &OpenMode) -> Vec<String> {
        match mode {
            OpenMode::Local => self.config().available_editors.clone(),
            OpenMode::Remote { .. } => vec!["vscode".into()],
            OpenMode::Unavailable => vec![],
        }
    }

    fn preferred_editor(&self, offered: &[String]) -> Option<String> {
        preferred(self.prefs["editor"].as_str(), offered)
    }

    fn set_preferred_editor(&mut self, id: &str, cx: &mut Context<Self>) {
        self.prefs["editor"] = json!(id);
        crate::settings::save_prefs(&self.prefs);
        cx.notify();
    }

    /// Opens the thread's folder in `editor`, or the preferred one (`editor.openFavorite`).
    pub(crate) fn open_in_editor(&mut self, editor: Option<String>, cx: &mut Context<Self>) {
        let mode = self.open_mode();
        let offered = self.offered_editors(&mode);
        let (Some(cwd), Some(editor)) = (
            self.thread_cwd(),
            editor.or_else(|| self.preferred_editor(&offered)),
        ) else {
            return;
        };
        match mode {
            OpenMode::Unavailable => {}
            OpenMode::Remote { host, username } => {
                cx.open_url(&remote_folder_url(
                    &editor,
                    &host,
                    username.as_deref(),
                    &cwd,
                ));
                self.set_preferred_editor(&editor, cx);
            }
            OpenMode::Local => {
                let Some(client) = self.client() else { return };
                self.set_preferred_editor(&editor, cx);
                let payload = json!({ "cwd": cwd, "editor": editor });
                cx.spawn(async move |this, cx| {
                    let result =
                        on_tokio(async move { client.call("shell.openInEditor", payload).await })
                            .await;
                    let error = match result {
                        Ok(Ok(_)) => return,
                        Ok(Err(e)) => e.to_string(),
                        Err(e) => e.to_string(),
                    };
                    this.update(cx, |app, cx| {
                        app.error = Some(format!("Open in editor failed: {error}").into());
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    pub(crate) fn render_open_in(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.open_mode();
        let offered = self.offered_editors(&mode);
        let current = self.preferred_editor(&offered);
        let note = match &mode {
            OpenMode::Unavailable => Some(t("No SSH route to this machine")),
            OpenMode::Local if offered.is_empty() => Some(t("No installed editors found")),
            OpenMode::Remote { .. } => Some(t("Opens over SSH. Needs your key on this machine")),
            OpenMode::Local => None,
        };
        let app = cx.entity().downgrade();
        h_flex()
            .child(
                Button::new("open-in")
                    .xsmall()
                    .ghost()
                    .label(current.as_deref().map_or_else(
                        || t("Open in editor"),
                        |id| tv("Open in {editor}", &[("editor", &label(id))]),
                    ))
                    .disabled(current.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_editor(None, cx))),
            )
            .child(
                Button::new("open-in-choose")
                    .xsmall()
                    .ghost()
                    .label("▾")
                    .tooltip(t("Choose editor"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for id in &offered {
                            let (app, id) = (app.clone(), id.clone());
                            menu = menu.item(
                                PopupMenuItem::new(label(&id))
                                    .checked(current.as_ref() == Some(&id))
                                    .on_click(move |_, _, cx| {
                                        app.update(cx, |this, cx| {
                                            this.set_preferred_editor(&id, cx)
                                        })
                                        .ok();
                                    }),
                            );
                        }
                        if let Some(note) = &note {
                            menu = menu.item(PopupMenuItem::new(note.clone()).disabled(true));
                        }
                        menu
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenMode, open_mode, preferred, remote_folder_url};
    use serde_json::json;

    #[test]
    fn picks_the_open_mode_from_the_server_url() {
        assert_eq!(open_mode("http://127.0.0.1:3773", &[]), OpenMode::Local);
        assert_eq!(open_mode("http://localhost:3773", &[]), OpenMode::Local);
        assert_eq!(open_mode("http://[::1]:3773", &[]), OpenMode::Local);
        assert_eq!(
            open_mode("https://box.tail.ts.net", &[]),
            OpenMode::Unavailable
        );
        assert_eq!(
            open_mode(
                "https://box.tail.ts.net",
                &[json!({"kind": "tailscale", "host": "box", "username": "me"})]
            ),
            OpenMode::Remote {
                host: "box".into(),
                username: Some("me".into())
            }
        );
    }

    #[test]
    fn prefers_the_saved_editor_then_contract_order() {
        let offered = vec!["zed".to_owned(), "vscode".to_owned()];
        assert_eq!(preferred(Some("zed"), &offered).as_deref(), Some("zed"));
        assert_eq!(
            preferred(Some("cursor"), &offered).as_deref(),
            Some("vscode")
        );
        assert_eq!(preferred(None, &[]), None);
    }

    #[test]
    fn builds_remote_urls_like_the_contract() {
        assert_eq!(
            remote_folder_url("vscode", "box", Some("me"), "/home/me/my repo"),
            "vscode://vscode-remote/ssh-remote+me%40box/home/me/my%20repo?windowId=_blank"
        );
        assert_eq!(
            remote_folder_url("vscode", "a+b", None, "C:\\src"),
            "vscode://vscode-remote/ssh-remote+7b22686f73744e616d65223a22612b62227d/C%3A/src?windowId=_blank"
        );
    }
}
