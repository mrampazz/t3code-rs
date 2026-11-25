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

/// A link clicked in chat markdown, dispatched from the `TextView` link handler (which must be
/// `Send`, so it cannot reach the app directly).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = t3, no_json)]
pub struct OpenLink {
    pub url: SharedString,
}

/// Where a chat link points: a web page, or a file on the server with an optional position.
#[derive(Debug, PartialEq)]
pub enum LinkTarget {
    Url(String),
    File {
        path: String,
        line: Option<u64>,
        column: Option<u64>,
    },
}

/// `splitFilePathPosition`: `path:12`, `path:12:3`, `path:12-20`, or a `#L12`, `#L12C3`, `#L12-L20` hash.
fn split_position(path: &str, hash: &str) -> (String, Option<u64>, Option<u64>) {
    let num = |s: &str| s.parse::<u64>().ok().filter(|n| *n > 0);
    let parts: Vec<&str> = path.rsplitn(3, ':').collect();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match parts.as_slice() {
        [col, line, rest] if digits(col) && digits(line) => {
            return (rest.to_string(), num(line), num(col));
        }
        [last, rest @ ..] if !rest.is_empty() => {
            let line = last.split_once('-').map_or(*last, |(l, _)| l);
            if digits(line) && last.split_once('-').is_none_or(|(_, e)| digits(e)) {
                let base = path[..path.len() - last.len() - 1].to_owned();
                return (base, num(line), None);
            }
        }
        _ => {}
    }
    let Some(spec) = hash.strip_prefix("#L").or_else(|| hash.strip_prefix("#l")) else {
        return (path.to_owned(), None, None);
    };
    let line_end = spec
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(spec.len());
    let column = spec[line_end..].strip_prefix(['C', 'c']).and_then(num);
    (path.to_owned(), num(&spec[..line_end]), column)
}

/// The home folder a server path lives under (`inferHomeFromCwd`), for `~/` links.
fn home_of(cwd: &str) -> Option<String> {
    let parts: Vec<&str> = cwd.split('/').collect();
    match parts.as_slice() {
        ["", root @ ("Users" | "home"), user, ..] if !user.is_empty() => {
            Some(format!("/{root}/{user}"))
        }
        _ => None,
    }
}

/// The thread a `t3-thread://v1/<id>`, `t3-thread://v2/<env>/<id>` or hosted
/// `https://app.t3.codes/threads/<env>/<id>` link names (`parseThreadLinkHref`), as candidates:
/// the id as written, then percent-decoded. The environment id is not used; the thread is
/// looked up across connected environments.
pub fn thread_link_ids(href: &str) -> Vec<String> {
    let id = if let Some(id) = href.strip_prefix("t3-thread://v1/") {
        Some(id)
    } else if let Some(rest) = href.strip_prefix("t3-thread://v2/") {
        rest.split_once('/').map(|(_, id)| id)
    } else if let Some(rest) = href.strip_prefix("https://app.t3.codes/threads/") {
        rest.split_once('/')
            .filter(|(_, id)| !id.contains('/'))
            .map(|(_, id)| id)
    } else {
        None
    };
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    let decoded = url::form_urlencoded::parse(format!("x={}", id.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .filter(|d| d != id);
    std::iter::once(id.to_owned()).chain(decoded).collect()
}

/// Classifies a markdown link like the web's `resolveMarkdownFileLink`: `file:` URLs and bare
/// paths are files (relative ones resolve against the thread's folder); other schemes are URLs.
pub fn link_target(href: &str, cwd: Option<&str>) -> LinkTarget {
    let href = href.trim();
    if href.len() >= 5 && href[..5].eq_ignore_ascii_case("file:") {
        if let Ok(url) = url::Url::parse(href)
            && let Ok(path) = url.to_file_path()
        {
            let hash = url.fragment().map(|f| format!("#{f}")).unwrap_or_default();
            let (path, line, column) = split_position(&path.to_string_lossy(), &hash);
            return LinkTarget::File { path, line, column };
        }
        return LinkTarget::Url(href.to_owned());
    }
    // A scheme of two or more letters; one letter is a Windows drive (`C:\…`).
    let scheme = href.find(':').filter(|&i| {
        i > 1
            && href[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    });
    if scheme.is_some() || href.starts_with('#') {
        return LinkTarget::Url(href.to_owned());
    }
    let (raw, hash) = href.split_at(href.find('#').unwrap_or(href.len()));
    let (path, line, column) = split_position(raw, hash);
    let path = match (cwd, path.strip_prefix("~/")) {
        (Some(cwd), Some(rest)) => {
            home_of(cwd).map_or(path.clone(), |home| format!("{home}/{rest}"))
        }
        (Some(cwd), None) if !path.starts_with('/') && !path.contains(":\\") => {
            format!(
                "{}/{}",
                cwd.trim_end_matches('/'),
                path.trim_start_matches("./")
            )
        }
        _ => path,
    };
    LinkTarget::File { path, line, column }
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

/// The same link for a file: a trailing position makes VS Code open it instead of a folder.
pub fn remote_file_url(scheme: &str, host: &str, username: Option<&str>, path: &str) -> String {
    remote_folder_url(scheme, host, username, path).replace("?windowId=_blank", ":1:1")
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

    /// The editor `editor.openFavorite` would use, when one can open here.
    pub(crate) fn favorite_editor(&self) -> Option<String> {
        self.preferred_editor(&self.offered_editors(&self.open_mode()))
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

    /// Opens a chat link: web pages in the browser, files in the preferred editor at their line.
    pub(crate) fn open_link(&mut self, href: &str, cx: &mut Context<Self>) {
        let ids = thread_link_ids(href);
        if !ids.is_empty() {
            let active = self.env().map(|e| e.key.clone());
            let mut envs: Vec<&crate::env::Env> = self.envs.iter().collect();
            envs.sort_by_key(|e| Some(&e.key) != active.as_ref());
            let found = ids.iter().find_map(|id| {
                envs.iter()
                    .find(|e| e.shell.threads.iter().any(|t| &t.id == id))
                    .map(|e| (e.key.clone(), id.clone()))
            });
            return match found {
                Some((env, id)) => self.select_thread(env, id, cx),
                None => {
                    self.error = Some(t("The thread's environment is not available.").into());
                    cx.notify()
                }
            };
        }
        let cwd = self.thread_cwd();
        let (path, line, column) = match link_target(href, cwd.as_deref()) {
            LinkTarget::Url(url) => return cx.open_url(&url),
            LinkTarget::File { path, line, column } => (path, line, column),
        };
        let mode = self.open_mode();
        let Some(editor) = self.preferred_editor(&self.offered_editors(&mode)) else {
            self.error = Some(t("No installed editors found").into());
            return cx.notify();
        };
        match mode {
            OpenMode::Unavailable => {
                self.error = Some(t("No SSH route to this machine").into());
                cx.notify();
            }
            OpenMode::Remote { host, username } => {
                cx.open_url(&remote_file_url(&editor, &host, username.as_deref(), &path))
            }
            OpenMode::Local => {
                let Some(client) = self.client() else { return };
                let mut position = json!({});
                if let Some(line) = line {
                    position["line"] = json!(line);
                }
                if let Some(column) = column {
                    position["column"] = json!(column);
                }
                let payload = json!({ "cwd": path, "editor": editor, "position": position, "projectRoot": cwd });
                cx.spawn(async move |this, cx| {
                    let result =
                        on_tokio(async move { client.call("shell.openInEditor", payload).await })
                            .await;
                    if let Some(e) = match result {
                        Ok(Ok(_)) => None,
                        Ok(Err(e)) => Some(e.to_string()),
                        Err(e) => Some(e.to_string()),
                    } {
                        this.update(cx, |app, cx| {
                            app.error = Some(format!("Open in editor failed: {e}").into());
                            cx.notify();
                        })
                        .ok();
                    }
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
    use super::{LinkTarget, OpenMode, link_target, open_mode, preferred, remote_folder_url};
    use serde_json::json;

    #[test]
    fn parses_thread_links() {
        use super::thread_link_ids;
        assert_eq!(thread_link_ids("t3-thread://v1/t1"), vec!["t1"]);
        assert_eq!(thread_link_ids("t3-thread://v2/env/t:1"), vec!["t:1"]);
        assert_eq!(
            thread_link_ids("https://app.t3.codes/threads/e/thread%3Aa"),
            vec!["thread%3Aa", "thread:a"]
        );
        assert!(thread_link_ids("https://app.t3.codes/threads/e").is_empty());
        assert!(thread_link_ids("https://x.dev/threads/e/t").is_empty());
    }

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
    fn classifies_chat_links() {
        let file = |path: &str, line: Option<u64>, column: Option<u64>| LinkTarget::File {
            path: path.into(),
            line,
            column,
        };
        let cwd = Some("/Users/me/repo");
        assert_eq!(
            link_target("https://x.dev/a", cwd),
            LinkTarget::Url("https://x.dev/a".into())
        );
        assert_eq!(
            link_target("src/main.rs:12:4", cwd),
            file("/Users/me/repo/src/main.rs", Some(12), Some(4))
        );
        assert_eq!(
            link_target("./src/a.ts#L42", cwd),
            file("/Users/me/repo/src/a.ts", Some(42), None)
        );
        assert_eq!(
            link_target("src/a.ts#L42C7", cwd),
            file("/Users/me/repo/src/a.ts", Some(42), Some(7))
        );
        assert_eq!(
            link_target("/etc/hosts:3-9", cwd),
            file("/etc/hosts", Some(3), None)
        );
        assert_eq!(
            link_target("~/notes.md", cwd),
            file("/Users/me/notes.md", None, None)
        );
        assert_eq!(
            link_target("file:///tmp/my%20file.rs#L5", cwd),
            file("/tmp/my file.rs", Some(5), None)
        );
        assert_eq!(
            link_target("README.md", cwd),
            file("/Users/me/repo/README.md", None, None)
        );
        assert_eq!(
            link_target("mailto:a@b.c", cwd),
            LinkTarget::Url("mailto:a@b.c".into())
        );
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
