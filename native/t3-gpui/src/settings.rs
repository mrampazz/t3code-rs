//! Settings view (web: `apps/web/src/components/settings/`). Server settings come from the
//! config stream and change through `server.updateSettings`; appearance is a client preference.

use crate::i18n::{t, tv};
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::composer::RUNTIME_MODES;
use crate::{T3App, on_tokio};

/// `NotificationMode` values and the web's labels for them.
const NOTIFICATION_MODES: [(&str, &str); 4] = [
    ("off", "Off"),
    ("notifications", "Notifications only"),
    ("sound", "Sound only"),
    ("notifications-and-sound", "Notifications with sound"),
];

fn prefs_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/t3-gpui/prefs.json"))
}

/// Client preferences (`{"theme": "dark" | "light"}`), kept beside the session file.
pub fn load_prefs() -> Value {
    prefs_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
}

pub(crate) fn save_prefs(prefs: &Value) {
    crate::timeline::set_timestamp_format(prefs);
    let Some(path) = prefs_path() else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, serde_json::to_vec_pretty(prefs).unwrap());
}

pub fn theme_mode(prefs: &Value) -> ThemeMode {
    if prefs["theme"] == "light" {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

/// Server settings shown as on/off rows, by section, with the web's labels.
/// shortcut: number, text and per-project settings (auto-settle days, title instructions, project
/// overrides) are not editable here yet.
const SERVER_TOGGLES: &[(&str, &[(&str, &str)])] = &[
    (
        "Threads",
        &[
            ("autoResumeLimitedThreads", "Resume after usage limits"),
            ("snoozeLimitedThreads", "Snooze until limits reset"),
            ("sidebarAutoSettleOnMerge", "Auto-settle merged threads"),
            (
                "sidebarAutoSettleKeepPullRequests",
                "Keep threads with a PR",
            ),
            (
                "continueThreadsAfterServerUpdate",
                "Continue threads after restarts",
            ),
            ("keepAwakeWhileAgentsWork", "Keep awake while agents work"),
        ],
    ),
    (
        "Worktrees",
        &[
            (
                "newWorktreesStartFromOrigin",
                "New worktrees start from remote",
            ),
            ("ghqCloneLayout", "Clone into host folders"),
        ],
    ),
    (
        "Providers",
        &[
            ("enableProviderUpdateChecks", "Provider update checks"),
            ("enableAgentBrowserAccess", "Agent browser access"),
        ],
    ),
    (
        "Beta",
        &[
            ("guestSharingEnabled", "Guest links"),
            ("linkedEnvironmentsEnabled", "Linked environments"),
            ("workflowsEnabled", "Workflows"),
            ("promptSuggestionsEnabled", "Suggest next prompt"),
            ("fastWorktreeCheckout", "Fast worktree checkout"),
        ],
    ),
];

/// A free instance id for a new instance of `driver` named `name`: `driver-name` slugged to the
/// contract's `^[a-zA-Z][a-zA-Z0-9_-]*$`, suffixed until unused.
pub fn new_instance_id(driver: &str, name: &str, taken: &[String]) -> String {
    let slug: String = format!("{driver}-{name}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(56)
        .collect();
    let base = if slug.starts_with(|c: char| c.is_ascii_alphabetic()) {
        slug
    } else {
        format!("p-{slug}")
    };
    (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|id| !taken.contains(id))
        .unwrap()
}

/// Whether release `latest` is newer than `current` (dotted numbers; a pre-release sorts first).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| {
        let (core, pre) = v
            .trim_start_matches('v')
            .split_once('-')
            .map_or((v.trim_start_matches('v'), None), |(c, p)| (c, Some(p)));
        let nums: Vec<u64> = core.split('.').map(|n| n.parse().unwrap_or(0)).collect();
        (nums, pre.is_none())
    };
    parse(latest) > parse(current)
}

/// A provider's usage windows as (label, percent used, reset time), most used first.
pub fn usage_windows(usage: &Value) -> Vec<(String, f64, Option<jiff::Timestamp>)> {
    let mut windows: Vec<_> = usage["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some((
                w["label"].as_str()?.to_owned(),
                w["usedPercent"].as_f64()?.clamp(0., 100.),
                w["resetsAt"].as_str().and_then(|r| r.parse().ok()),
            ))
        })
        .collect();
    windows.sort_by(|a, b| b.1.total_cmp(&a.1));
    windows
}

/// The `providerInstanceMutation` that flips one instance's `enabled`, keeping its other config.
pub fn toggle_instance(settings: &Value, instance_id: &str, driver: &str, enabled: bool) -> Value {
    let mut instance = settings["providerInstances"][instance_id].clone();
    if !instance.is_object() {
        instance = json!({ "driver": driver });
    }
    instance["enabled"] = json!(enabled);
    json!({ "operation": "upsert", "instanceId": instance_id, "instance": instance })
}

impl T3App {
    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            self.record_route(&crate::Route::Settings);
        }
        self.set_settings_open(!self.settings_open, cx);
    }

    pub(crate) fn set_settings_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open == self.settings_open {
            return;
        }
        self.settings_open = open;
        self.new_pairing_url = None;
        if self.settings_open {
            self.subscribe_access(cx);
            self.subscribe_provider_setup(cx);
            self.subscribe_scheduled(cx);
            self.subscribe_storage(cx);
            self.discover_source_control(cx);
            self.load_peer_links(cx);
            self.subscribe_workflows(cx);
        } else {
            self.release_access();
            self.release_provider_setup();
            self.release_scheduled();
            self.release_storage();
            self.release_workflows();
        }
        cx.notify();
    }

    pub(crate) fn update_settings(&mut self, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        self.settings_status = Some("Saving…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("server.updateSettings", payload).await }).await;
            this.update(cx, |app, cx| {
                app.settings_status = match result {
                    Ok(Ok(settings)) => {
                        if let Some(env) = app.env_mut() {
                            env.config.settings = settings;
                        }
                        None
                    }
                    Ok(Err(e)) => Some(format!("Could not save: {e}").into()),
                    Err(e) => Some(format!("Could not save: {e}").into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add_snooze_preset(
        &mut self,
        stored: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("90m  ·  2h  ·  tomorrow 09:00  ·  friday 17:00")
        });
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, stored) = (app.clone(), input.clone(), stored.clone());
            dialog
                .title(t("Snooze options"))
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Add"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let Some(preset) = crate::sidebar::parse_preset(&input.read(cx).value()) else {
                        return false;
                    };
                    let mut next = stored.clone();
                    next.push(preset);
                    app.update(cx, |this, cx| {
                        this.update_settings(json!({ "patch": { "snoozePresets": next } }), cx)
                    })
                    .ok();
                    true
                })
        });
    }

    fn add_provider_instance(
        &mut self,
        driver: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let taken: Vec<String> = self
            .config()
            .providers
            .iter()
            .map(|p| p.instance_id.clone())
            .collect();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Name")));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, driver, taken) =
                (app.clone(), input.clone(), driver.clone(), taken.clone());
            dialog
                .title(format!("{} · {driver}", t("Add provider instance")))
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Add"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let name = input.read(cx).value().trim().to_string();
                    if name.is_empty() {
                        return false;
                    }
                    let id = new_instance_id(&driver, &name, &taken);
                    let mutation = json!({ "operation": "create", "instanceId": id,
                        "instance": { "driver": driver, "displayName": name, "enabled": true } });
                    app.update(cx, |this, cx| {
                        this.update_settings(
                            json!({ "patch": {}, "providerInstanceMutation": mutation }),
                            cx,
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    fn edit_branch_prefix(&mut self, current: String, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input) = (app.clone(), input.clone());
            dialog
                .title(t("Branch prefix"))
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Save"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let prefix = input.read(cx).value().trim().to_string();
                    app.update(cx, |this, cx| {
                        this.update_settings(json!({ "patch": { "branchNamePrefix": prefix } }), cx)
                    })
                    .ok();
                    true
                })
        });
    }

    fn check_server_release(&mut self, env: String, cx: &mut Context<Self>) {
        let Some(client) = self.env_by(&env).and_then(|e| e.client.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                client
                    .call("server.getNpmRelease", json!({ "spec": "latest" }))
                    .await
            })
            .await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(Ok(release)) => {
                        if let Some(v) = release["version"].as_str() {
                            app.latest_releases.insert(env, v.to_owned());
                        }
                    }
                    Ok(Err(e)) => app.settings_status = Some(e.to_string().into()),
                    Err(e) => app.settings_status = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the host to update itself; it restarts and the connection resumes on its own.
    fn update_server(&mut self, env: String, target: String, cx: &mut Context<Self>) {
        let Some(client) = self.env_by(&env).and_then(|e| e.client.clone()) else {
            return;
        };
        self.settings_status = Some(t("Updating…").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                client
                    .call("server.updateServer", json!({ "targetVersion": target }))
                    .await
            })
            .await;
            this.update(cx, |app, cx| {
                app.settings_status = match result {
                    Ok(Ok(_)) => Some(t("The server is restarting with the update.").into()),
                    Ok(Err(e)) => Some(e.to_string().into()),
                    Err(e) => Some(e.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn keybinding_call(&mut self, method: &'static str, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            this.update(cx, |app, cx| {
                match result {
                    // The reply carries the new list; the config stream confirms it too.
                    Ok(Ok(reply)) => {
                        if let (Some(env), Some(rules)) =
                            (app.env_mut(), reply["keybindings"].as_array())
                        {
                            env.config.keybindings = rules.clone();
                        }
                        app.settings_status = None;
                    }
                    Ok(Err(e)) => app.settings_status = Some(e.to_string().into()),
                    Err(e) => app.settings_status = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replaces a rule's shortcut with a typed value such as `mod+shift+k`.
    fn change_keybinding(&mut self, target: Value, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let current = target["key"].as_str().unwrap_or("").to_owned();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, target) = (app.clone(), input.clone(), target.clone());
            dialog
                .title(target["command"].as_str().unwrap_or("").to_owned())
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Save"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let key = input.read(cx).value().trim().to_lowercase();
                    if key.is_empty() {
                        return false;
                    }
                    let mut upsert =
                        json!({ "key": key, "command": target["command"], "replace": target });
                    if let Some(when) = target.get("when") {
                        upsert["when"] = when.clone();
                    }
                    app.update(cx, |this, cx| {
                        this.keybinding_call("server.upsertKeybinding", upsert, cx)
                    })
                    .ok();
                    true
                })
        });
    }

    fn edit_title_instructions(
        &mut self,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input) = (app.clone(), input.clone());
            dialog
                .title(t("Thread title instructions"))
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Save"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let text = input.read(cx).value().trim().to_string();
                    app.update(cx, |this, cx| {
                        this.update_settings(
                            json!({ "patch": { "threadTitleInstructions": text } }),
                            cx,
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    fn set_language(&mut self, code: &str, cx: &mut Context<Self>) {
        self.prefs["locale"] = json!(code);
        save_prefs(&self.prefs);
        crate::i18n::set_locale(code);
        cx.notify();
    }

    pub(crate) fn set_theme(&mut self, light: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs["theme"] = json!(if light { "light" } else { "dark" });
        save_prefs(&self.prefs);
        crate::web_theme::change(theme_mode(&self.prefs), Some(window), cx);
        cx.notify();
    }

    /// Settings as the web shows them (`SettingsDialog`): a modal over the app with the grouped
    /// page list on the left and one page of cards on the right. Only the open page is built.
    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let page = self.settings_page;
        let query = self.settings_search.read(cx).value().trim().to_lowercase();
        let nav = v_flex()
            .w(px(240.))
            .h_full()
            .flex_none()
            .bg(c.popover)
            .border_r_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .h(px(48.))
                    .px_3()
                    .gap_2()
                    .child(
                        crate::ui::icon("search")
                            .size(px(16.))
                            .text_color(c.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.settings_search).appearance(false)),
                    ),
            )
            .child(
                v_flex()
                    .id("settings-nav")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_3()
                    .children(SettingsPage::GROUPS.iter().filter_map(|(heading, pages)| {
                        let pages: Vec<SettingsPage> = pages
                            .iter()
                            .copied()
                            .filter(|p| {
                                query.is_empty() || t(p.label()).to_lowercase().contains(&query)
                            })
                            .collect();
                        (!pages.is_empty()).then(|| {
                            v_flex()
                                .pt_3()
                                .children(heading.map(|h| {
                                    div()
                                        .px_3()
                                        .pb_1()
                                        .text_sm()
                                        .text_color(c.muted_foreground)
                                        .child(t(h))
                                }))
                                .children(pages.into_iter().map(|p| {
                                    h_flex()
                                        .id(SharedString::from(format!(
                                            "settings-nav-{}",
                                            p.label()
                                        )))
                                        .h(px(36.))
                                        .px_3()
                                        .gap_3()
                                        .rounded(px(10.))
                                        .cursor_pointer()
                                        .when(p == page, |r| {
                                            r.bg(c.accent).text_color(c.foreground)
                                        })
                                        .when(p != page, |r| {
                                            r.text_color(c.foreground.opacity(0.85))
                                        })
                                        .hover(|r| r.bg(c.accent))
                                        .child(
                                            crate::ui::icon(p.icon())
                                                .size(px(16.))
                                                .text_color(c.muted_foreground),
                                        )
                                        .child(t(p.label()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings_page = p;
                                            cx.notify();
                                        }))
                                }))
                        })
                    })),
            );
        let header = h_flex()
            .h(px(48.))
            .flex_none()
            .px_5()
            .gap_3()
            .border_b_1()
            .border_color(c.border)
            .child(div().text_color(c.muted_foreground).child(t("Settings")))
            .child(div().text_color(c.muted_foreground).child("/"))
            .child(div().flex_1().child(t(page.label())))
            .children(
                self.settings_status
                    .clone()
                    .map(|s| div().text_sm().text_color(c.muted_foreground).child(s)),
            )
            .child(
                crate::ui::icon_button("settings-close", "x", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
            );
        let body = v_flex()
            .id("settings-page")
            .flex_1()
            .overflow_y_scroll()
            .px_8()
            .pb_8()
            .children(self.settings_sections(page, cx));
        div()
            .absolute()
            .inset_0()
            .bg(gpui_kit::black().opacity(0.85))
            .occlude()
            .p(px(40.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_settings(cx)),
            )
            .child(
                h_flex()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .size_full()
                    .rounded(px(16.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(c.border)
                    .bg(c.background)
                    .shadow_lg()
                    .child(nav)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(header)
                            .child(body),
                    ),
            )
    }

    /// The sections of one settings page.
    fn settings_sections(&self, page: SettingsPage, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors;
        let settings = self.config().settings.clone();
        let app = cx.entity().downgrade();
        // A dropdown of labelled values that applies the pick with `apply`.
        let choice = |id: &'static str,
                      current: String,
                      options: Vec<(String, String)>,
                      apply: ApplyChoice| {
            let label = options
                .iter()
                .find(|(v, _)| *v == current)
                .map_or(current.clone(), |(_, l)| l.clone());
            let app = app.clone();
            Button::new(id)
                .outline()
                .h(px(36.))
                .min_w(px(220.))
                .dropdown_caret(true)
                .label(label)
                .dropdown_menu(move |mut menu, _, _| {
                    for (value, label) in options.clone() {
                        let (app, apply) = (app.clone(), apply.clone());
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(value == current)
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |this, cx| {
                                        apply(this, value.clone(), window, cx)
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
                .into_any_element()
        };
        let pref_switch =
            |id: &'static str, key: &'static str, on: bool, cx: &mut Context<Self>| {
                Switch::new(id)
                    .checked(on)
                    .color(cx.theme().colors.primary)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.prefs[key] = json!(*checked);
                        save_prefs(&this.prefs);
                        cx.notify();
                    }))
                    .into_any_element()
            };
        let server_switches = |group: &str, cx: &mut Context<Self>| -> Vec<AnyElement> {
            SERVER_TOGGLES
                .iter()
                .filter(|(title, _)| *title == group)
                .flat_map(|(_, rows)| rows.iter())
                .filter_map(|(key, label)| {
                    // Absent keys come from servers that predate the setting.
                    let on = settings.get(*key).and_then(Value::as_bool)?;
                    let key = *key;
                    Some(setting_row(
                        t(label),
                        None,
                        Switch::new(SharedString::from(format!("setting-{key}")))
                            .checked(on)
                            .color(cx.theme().colors.primary)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.update_settings(json!({ "patch": { key: *checked } }), cx)
                            })),
                        cx,
                    ))
                })
                .collect()
        };
        match page {
            SettingsPage::General => {
                let mode = self.prefs["notificationMode"]
                    .as_str()
                    .unwrap_or("off")
                    .to_owned();
                vec![
                    settings_section(
                        "Notifications",
                        vec![
                            setting_row(
                                t("Thread notifications"),
                                Some(t(
                                    "System alerts when a thread finishes, fails, or needs input or approval. Applies to this device while T3 Code is open.",
                                )),
                                choice(
                                    "notification-mode",
                                    mode,
                                    NOTIFICATION_MODES
                                        .iter()
                                        .map(|(v, l)| (v.to_string(), t(l)))
                                        .collect(),
                                    std::rc::Rc::new(|this, v, _window, cx| {
                                        this.prefs["notificationMode"] = json!(v);
                                        save_prefs(&this.prefs);
                                        cx.notify();
                                    }),
                                ),
                                cx,
                            ),
                            setting_row(
                                t("In-app notifications"),
                                Some(t(
                                    "Show a toast when another thread finishes, fails, or needs input or approval while this app has focus.",
                                )),
                                pref_switch(
                                    "in-app-notifications",
                                    "inAppNotifications",
                                    self.prefs["inAppNotifications"] != false,
                                    cx,
                                ),
                                cx,
                            ),
                        ],
                        cx,
                    ),
                    settings_section(
                        "Sidebar",
                        vec![setting_row(
                            t("Working section"),
                            Some(t(
                                "Fold working and monitoring threads into a Working section. They return to the top of the inbox when they need you.",
                            )),
                            pref_switch(
                                "working-shelf",
                                "workingShelf",
                                self.prefs["workingShelf"] != false,
                                cx,
                            ),
                            cx,
                        )],
                        cx,
                    ),
                ]
            }
            SettingsPage::Appearance => {
                let light = theme_mode(&self.prefs) == ThemeMode::Light;
                let current = crate::i18n::locale();
                vec![settings_section(
                    "Appearance",
                    vec![
                        setting_row(
                            t("Theme"),
                            None,
                            choice(
                                "theme",
                                if light { "light" } else { "dark" }.into(),
                                vec![("dark".into(), t("Dark")), ("light".into(), t("Light"))],
                                std::rc::Rc::new(|this, v, window, cx| {
                                    this.set_theme(v == "light", window, cx)
                                }),
                            ),
                            cx,
                        ),
                        setting_row(
                            t("Language"),
                            None,
                            choice(
                                "language",
                                current,
                                crate::i18n::LOCALES
                                    .iter()
                                    .map(|(c, n)| (c.to_string(), n.to_string()))
                                    .collect(),
                                std::rc::Rc::new(|this, v, _window, cx| this.set_language(&v, cx)),
                            ),
                            cx,
                        ),
                    ],
                    cx,
                )]
            }
            SettingsPage::Chat => {
                let queue = self.prefs["followUpBehavior"] == "queue";
                vec![settings_section(
                    "Follow-ups",
                    vec![setting_row(
                        t("Follow-up behavior"),
                        None,
                        choice(
                            "follow-up",
                            if queue { "queue" } else { "steer" }.into(),
                            vec![("steer".into(), t("Steer")), ("queue".into(), t("Queue"))],
                            std::rc::Rc::new(|this, v, _window, cx| {
                                this.prefs["followUpBehavior"] = json!(v);
                                save_prefs(&this.prefs);
                                cx.notify();
                            }),
                        ),
                        cx,
                    )],
                    cx,
                )]
            }
            SettingsPage::Keybindings => vec![settings_section(
                "Keybindings",
                self.config()
                    .keybindings
                    .iter()
                    .enumerate()
                    .map(|(i, rule)| {
                        let target = crate::keybindings::rule_target(rule);
                        let (change, remove) = (target.clone(), target.clone());
                        setting_row(
                            rule["command"].as_str().unwrap_or("").to_owned(),
                            None,
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .px_2()
                                        .rounded_md()
                                        .bg(c.muted)
                                        .border_1()
                                        .border_color(c.border)
                                        .text_sm()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(target["key"].as_str().unwrap_or("").to_owned()),
                                )
                                .child(
                                    Button::new(("kb-change", i))
                                        .small()
                                        .ghost()
                                        .label(t("Change"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.change_keybinding(change.clone(), window, cx)
                                        })),
                                )
                                .child(crate::ui::icon_button(("kb-remove", i), "x", cx).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.keybinding_call(
                                            "server.removeKeybinding",
                                            remove.clone(),
                                            cx,
                                        )
                                    }),
                                )),
                            cx,
                        )
                    })
                    .collect(),
                cx,
            )],
            SettingsPage::Providers => {
                let mut out: Vec<AnyElement> = self
                    .config()
                    .providers
                    .iter()
                    .map(|p| self.render_provider_card(p, &settings, cx))
                    .collect();
                let mut drivers: Vec<String> = self
                    .config()
                    .providers
                    .iter()
                    .map(|p| p.driver.clone())
                    .collect();
                drivers.sort();
                drivers.dedup();
                let add_app = cx.entity().downgrade();
                out.push(
                    div()
                        .pt_4()
                        .child(
                            Button::new("provider-add")
                                .small()
                                .outline()
                                .label(t("Add provider instance"))
                                .dropdown_menu(move |mut menu, _, _| {
                                    for driver in &drivers {
                                        let (app, driver) = (add_app.clone(), driver.clone());
                                        menu =
                                            menu.item(PopupMenuItem::new(driver.clone()).on_click(
                                                move |_, window, cx| {
                                                    app.update(cx, |this, cx| {
                                                        this.add_provider_instance(
                                                            driver.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    })
                                                    .ok();
                                                },
                                            ));
                                    }
                                    menu
                                }),
                        )
                        .into_any_element(),
                );
                out.push(settings_section(
                    "Provider options",
                    server_switches("Providers", cx),
                    cx,
                ));
                out
            }
            SettingsPage::Threads => {
                let runtime = settings["defaultRuntimeMode"]
                    .as_str()
                    .unwrap_or("full-access")
                    .to_owned();
                let env_mode = settings["defaultThreadEnvMode"]
                    .as_str()
                    .unwrap_or("local")
                    .to_owned();
                let days = settings["sidebarAutoSettleAfterDays"].as_u64();
                let title_instructions = settings["threadTitleInstructions"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let stored: Vec<Value> = settings["snoozePresets"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(crate::sidebar::default_presets);
                let full = stored.len() >= 8;
                vec![
                    settings_section(
                        "New threads",
                        vec![
                            setting_row(
                                t("Default permission mode"),
                                None,
                                choice(
                                    "default-runtime",
                                    runtime,
                                    RUNTIME_MODES.iter().map(|(m, l)| (m.to_string(), t(l))).collect(),
                                    std::rc::Rc::new(|this, v, _window, cx| {
                                        this.update_settings(json!({ "patch": { "defaultRuntimeMode": v } }), cx)
                                    }),
                                ),
                                cx,
                            ),
                            setting_row(
                                t("New thread mode"),
                                None,
                                choice(
                                    "default-env-mode",
                                    env_mode,
                                    vec![("local".into(), t("Local")), ("worktree".into(), t("New worktree"))],
                                    std::rc::Rc::new(|this, v, _window, cx| {
                                        this.update_settings(json!({ "patch": { "defaultThreadEnvMode": v } }), cx)
                                    }),
                                ),
                                cx,
                            ),
                            setting_row(
                                t("Thread title instructions"),
                                (!title_instructions.is_empty()).then(|| title_instructions.clone()),
                                Button::new("title-instructions")
                                    .small()
                                    .outline()
                                    .label(t("Edit"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit_title_instructions(title_instructions.clone(), window, cx)
                                    })),
                                cx,
                            ),
                        ],
                        cx,
                    ),
                    settings_section(
                        "Inbox",
                        vec![
                            setting_row(
                                t("Auto-settle inactive threads"),
                                None,
                                choice(
                                    "auto-settle-days",
                                    days.map_or("never".into(), |d| d.to_string()),
                                    [None, Some(1u64), Some(3), Some(7), Some(14), Some(30)]
                                        .into_iter()
                                        .map(|o| {
                                            (
                                                o.map_or("never".into(), |d| d.to_string()),
                                                o.map_or_else(|| t("Never"), |d| format!("{d} d")),
                                            )
                                        })
                                        .collect(),
                                    std::rc::Rc::new(|this, v, _window, cx| {
                                        let days = v.parse::<u64>().ok();
                                        this.update_settings(json!({ "patch": { "sidebarAutoSettleAfterDays": days } }), cx)
                                    }),
                                ),
                                cx,
                            ),
                            setting_row(
                                t("Snooze options"),
                                None,
                                h_flex()
                                    .flex_wrap()
                                    .justify_end()
                                    .gap_1()
                                    .children(stored.iter().enumerate().map(|(i, p)| {
                                        let remaining: Vec<Value> = stored
                                            .iter()
                                            .enumerate()
                                            .filter(|(j, _)| *j != i)
                                            .map(|(_, v)| v.clone())
                                            .collect();
                                        Button::new(("snooze-preset", i))
                                            .xsmall()
                                            .outline()
                                            .label(format!("{} ✕", crate::sidebar::preset_text(p)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.update_settings(json!({ "patch": { "snoozePresets": remaining } }), cx)
                                            }))
                                    }))
                                    .child({
                                        let stored = stored.clone();
                                        Button::new("snooze-add")
                                            .xsmall()
                                            .ghost()
                                            .label(t("Add"))
                                            .disabled(full)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.add_snooze_preset(stored.clone(), window, cx)
                                            }))
                                    }),
                                cx,
                            ),
                        ],
                        cx,
                    ),
                    settings_section("Threads", server_switches("Threads", cx), cx),
                ]
            }
            SettingsPage::ScheduledTasks => vec![{
                let panel = self.render_scheduled(cx).into_any_element();
                settings_panel("Scheduled tasks", panel, cx)
            }],
            SettingsPage::Projects => vec![{
                let panel = self.render_project_settings(cx).into_any_element();
                settings_panel("Projects", panel, cx)
            }],
            SettingsPage::SourceControl => {
                vec![{
                    let panel = self.render_source_control(cx).into_any_element();
                    settings_panel("Source control & issues", panel, cx)
                }]
            }
            SettingsPage::Worktrees => {
                let mode = settings["branchNamingMode"]
                    .as_str()
                    .unwrap_or("static")
                    .to_owned();
                let prefix = settings["branchNamePrefix"]
                    .as_str()
                    .unwrap_or("t3")
                    .to_owned();
                let mut rows = vec![
                    setting_row(
                        t("Worktree branch naming"),
                        None,
                        choice(
                            "branch-naming",
                            mode,
                            vec![
                                ("static".into(), t("Static prefix")),
                                ("semantic".into(), t("Semantic prefix")),
                                ("custom".into(), t("Custom instructions")),
                            ],
                            std::rc::Rc::new(|this, v, _window, cx| {
                                this.update_settings(
                                    json!({ "patch": { "branchNamingMode": v } }),
                                    cx,
                                )
                            }),
                        ),
                        cx,
                    ),
                    setting_row(
                        t("Branch prefix"),
                        Some(if prefix.is_empty() {
                            t("No prefix")
                        } else {
                            prefix.clone()
                        }),
                        Button::new("branch-prefix")
                            .small()
                            .outline()
                            .label(t("Edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_branch_prefix(prefix.clone(), window, cx)
                            })),
                        cx,
                    ),
                ];
                rows.extend(server_switches("Worktrees", cx));
                vec![settings_section("Worktrees", rows, cx)]
            }
            SettingsPage::Environments => {
                let mut out = vec![settings_section("Environments", self.server_rows(cx), cx)];
                if settings["linkedEnvironmentsEnabled"] == true {
                    out.push({
                        let panel = self.render_peer_links(cx).into_any_element();
                        settings_panel("Linked environments", panel, cx)
                    });
                }
                out
            }
            SettingsPage::Access => vec![{
                let panel = self.render_access(cx).into_any_element();
                settings_panel("Sharing & access", panel, cx)
            }],
            SettingsPage::Maintenance => vec![{
                let panel = self.render_storage(cx).into_any_element();
                settings_panel("Storage", panel, cx)
            }],
            SettingsPage::Labs => {
                let mut rows = server_switches("Beta", cx);
                rows.push(setting_row(
                    t("Boring sidebar"),
                    Some(t("Group threads by project instead of the inbox.")),
                    pref_switch(
                        "project-folders",
                        "projectFolders",
                        self.prefs["projectFolders"] == true,
                        cx,
                    ),
                    cx,
                ));
                let mut out = vec![settings_section("Labs", rows, cx)];
                if settings["workflowsEnabled"] == true {
                    out.push({
                        let panel = self.render_workflows(cx).into_any_element();
                        settings_panel("Workflows", panel, cx)
                    });
                }
                out
            }
            SettingsPage::About => vec![settings_section(
                "About",
                vec![setting_row(
                    format!("t3-gpui v{}", env!("CARGO_PKG_VERSION")),
                    Some(t("Native client for T3 Code servers")),
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("about-source")
                                .small()
                                .ghost()
                                .label(t("Source code"))
                                .on_click(|_, _, cx| {
                                    cx.open_url("https://github.com/pingdotgg/t3code")
                                }),
                        )
                        .child(
                            Button::new("about-license")
                                .small()
                                .ghost()
                                .label(t("License"))
                                .on_click(|_, _, cx| {
                                    cx.open_url(
                                        "https://github.com/pingdotgg/t3code/blob/main/LICENSE",
                                    )
                                }),
                        ),
                    cx,
                )],
                cx,
            )],
        }
    }

    /// One provider instance as a card: name and status with its switch, usage, then setup.
    fn render_provider_card(
        &self,
        p: &crate::model::Provider,
        settings: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let enabled = p.enabled;
        let (id, driver) = (p.instance_id.clone(), p.driver.clone());
        let detail = [
            p.version.as_deref().map(|v| format!("v{v}")),
            Some(p.status.clone()),
            p.message.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let settings = settings.clone();
        let usage = usage_windows(&p.usage_limits);
        let unavailable = p.usage_limits["unavailable"]["message"]
            .as_str()
            .map(str::to_owned);
        let has_usage = !usage.is_empty() || unavailable.is_some();
        let usage_rows = v_flex()
            .gap_1()
            .children(usage.into_iter().map(|(label, used, resets)| {
                let color = if used >= 90. {
                    c.danger
                } else if used >= 70. {
                    c.warning
                } else {
                    c.primary
                };
                let resets = resets.map(|r| {
                    let local = r.to_zoned(jiff::tz::TimeZone::system());
                    tv(
                        "Resets {resetsAt}",
                        &[("resetsAt", &local.strftime("%b %-d, %H:%M").to_string())],
                    )
                });
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().w(px(140.)).truncate().child(label))
                    .child(
                        div()
                            .w(px(160.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(c.secondary)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(color)
                                    .w(relative(used as f32 / 100.)),
                            ),
                    )
                    .child(format!("{used:.0}%"))
                    .children(resets.map(|r| div().text_color(c.muted_foreground).child(r)))
            }))
            .children(unavailable.map(|m| div().text_sm().text_color(c.muted_foreground).child(m)));
        let remove = (p.instance_id != p.driver).then(|| {
            let id = p.instance_id.clone();
            crate::ui::icon_button(
                SharedString::from(format!("provider-remove-{id}")),
                "trash-2",
                cx,
            )
            .tooltip(|window, cx| Tooltip::new(t("Remove")).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                let mutation = json!({ "operation": "remove", "instanceId": id });
                this.update_settings(
                    json!({ "patch": {}, "providerInstanceMutation": mutation }),
                    cx,
                );
            }))
        });
        let toggle = Switch::new(SharedString::from(format!("provider-{}", p.instance_id)))
            .checked(enabled)
            .color(cx.theme().colors.primary)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let mutation = toggle_instance(&settings, &id, &driver, *checked);
                this.update_settings(
                    json!({ "patch": {}, "providerInstanceMutation": mutation }),
                    cx,
                );
            }));
        let mut rows = vec![setting_row(
            t("Enabled"),
            Some(detail),
            h_flex().gap_2().children(remove).child(toggle),
            cx,
        )];
        if has_usage {
            rows.push(div().px_4().py_3().child(usage_rows).into_any_element());
        }
        rows.push(
            div()
                .px_4()
                .py_3()
                .child(self.render_provider_setup(p, cx))
                .into_any_element(),
        );
        settings_section(
            p.display_name
                .clone()
                .unwrap_or_else(|| p.instance_id.clone())
                .as_str(),
            rows,
            cx,
        )
    }

    /// The connected servers, each with its status, update and switch actions.
    fn server_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors;
        let server = self.env().map(|e| e.key.clone()).unwrap_or_default();
        let mut rows: Vec<AnyElement> = self
            .envs
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let active = e.key == server;
                let status = match (&e.error, &e.client) {
                    (Some(error), _) => error.to_string(),
                    (None, Some(_)) => t("Connected"),
                    (None, None) => t("Connecting…"),
                };
                let (switch_key, remove_key) = (e.key.clone(), e.key.clone());
                let version = e.config.server_version.clone();
                let latest = self.latest_releases.get(&e.key).cloned();
                let newer = latest.clone().filter(|l| is_newer(l, &version));
                let can_update = matches!(
                    e.config.self_update.as_deref(),
                    Some("boot-service" | "respawn")
                );
                let (check_key, update_key) = (e.key.clone(), e.key.clone());
                let status = if version.is_empty() {
                    status
                } else {
                    format!("{status} · v{version}")
                };
                setting_row(
                    e.label(),
                    Some(status),
                    h_flex()
                        .gap_2()
                        .child(match (&newer, can_update) {
                            (Some(target), true) => {
                                let target = target.clone();
                                Button::new(("server-update", i))
                                    .small()
                                    .primary()
                                    .label(format!("{} v{target}", t("Update to")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_server(update_key.clone(), target.clone(), cx)
                                    }))
                                    .into_any_element()
                            }
                            (Some(target), false) => div()
                                .text_sm()
                                .text_color(c.warning)
                                .child(format!("v{target} {}", t("available")))
                                .into_any_element(),
                            (None, _) => Button::new(("server-check", i))
                                .small()
                                .ghost()
                                .label(if latest.is_some() {
                                    t("Up to date")
                                } else {
                                    t("Check for updates")
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.check_server_release(check_key.clone(), cx)
                                }))
                                .into_any_element(),
                        })
                        .child(
                            Button::new(("server-switch", i))
                                .small()
                                .label(if active { t("Active") } else { t("Switch") })
                                .when(active, |b| b.primary())
                                .when(!active, |b| b.outline())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.release_access();
                                    this.release_provider_setup();
                                    this.activate(&switch_key, cx);
                                    this.new_pairing_url = None;
                                    this.subscribe_access(cx);
                                    this.subscribe_provider_setup(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("server-disconnect", i))
                                .small()
                                .outline()
                                .label(t("Disconnect"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_env(&remove_key, cx)
                                })),
                        ),
                    cx,
                )
            })
            .collect();
        rows.push(
            div()
                .px_4()
                .py_3()
                .child(
                    Button::new("settings-add-server")
                        .small()
                        .ghost()
                        .label(t("+ Connect another server"))
                        .on_click(cx.listener(|this, _, _, cx| this.add_environment(cx))),
                )
                .into_any_element(),
        );
        rows
    }
}

/// The web's settings pages that this client has, in its sidebar order.
/// shortcut: pages for features gpui lacks (Diffs, Voice, SnapShots, Browser, Devices, Secrets,
/// Archive) are left out rather than shown empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SettingsPage {
    #[default]
    General,
    Appearance,
    Chat,
    Keybindings,
    Providers,
    Threads,
    ScheduledTasks,
    Projects,
    SourceControl,
    Worktrees,
    Environments,
    Access,
    Maintenance,
    Labs,
    About,
}

impl SettingsPage {
    /// Sidebar groups; a group without a heading is set apart by space alone.
    const GROUPS: &[(Option<&'static str>, &'static [SettingsPage])] = &[
        (
            None,
            &[
                Self::General,
                Self::Appearance,
                Self::Chat,
                Self::Keybindings,
            ],
        ),
        (
            Some("Agents"),
            &[Self::Providers, Self::Threads, Self::ScheduledTasks],
        ),
        (
            Some("Code"),
            &[Self::Projects, Self::SourceControl, Self::Worktrees],
        ),
        (Some("Connections"), &[Self::Environments, Self::Access]),
        (None, &[Self::Maintenance, Self::Labs, Self::About]),
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Chat => "Chat",
            Self::Keybindings => "Keybindings",
            Self::Providers => "Providers",
            Self::Threads => "Threads",
            Self::ScheduledTasks => "Scheduled tasks",
            Self::Projects => "Projects",
            Self::SourceControl => "Source control & issues",
            Self::Worktrees => "Worktrees",
            Self::Environments => "Environments",
            Self::Access => "Sharing & access",
            Self::Maintenance => "Maintenance",
            Self::Labs => "Labs",
            Self::About => "About",
        }
    }

    /// `SettingsSidebarNav`'s Lucide icons.
    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings-2",
            Self::Appearance => "palette",
            Self::Chat => "message-square-text",
            Self::Keybindings => "keyboard",
            Self::Providers => "bot",
            Self::Threads => "messages-square",
            Self::ScheduledTasks => "calendar-clock",
            Self::Projects => "panels-top-left",
            Self::SourceControl => "git-branch",
            Self::Worktrees => "folder-tree",
            Self::Environments => "server",
            Self::Access => "shield-check",
            Self::Maintenance => "wrench",
            Self::Labs => "flask-conical",
            Self::About => "info",
        }
    }
}

/// What a settings dropdown does with the picked value.
type ApplyChoice = std::rc::Rc<dyn Fn(&mut T3App, String, &mut Window, &mut Context<T3App>)>;

/// A settings row (web: `SettingsRow`): title and description, the control on the right.
fn setting_row(
    title: impl Into<SharedString>,
    description: Option<String>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let c = cx.theme().colors;
    h_flex()
        .px_4()
        .py_3()
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .children(description.filter(|d| !d.is_empty()).map(|d| {
                    div()
                        .text_size(px(13.))
                        .text_color(c.muted_foreground)
                        .child(d)
                })),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// A titled card of rows divided by rules (web: `SettingsSection`).
fn settings_section(title: &str, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    if rows.is_empty() {
        return div().into_any_element();
    }
    let last = rows.len() - 1;
    v_flex()
        .pt_8()
        .gap_3()
        .child(
            div()
                .px_1()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t(title)),
        )
        .child(
            v_flex()
                .rounded(px(14.))
                .border_1()
                .border_color(c.border)
                .bg(c.muted)
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    div()
                        .when(i < last, |d| d.border_b_1().border_color(c.border))
                        .child(row)
                })),
        )
        .into_any_element()
}

/// A titled card around a whole existing panel.
/// shortcut: these panels keep their own inner layout; their rows are not yet `setting_row`s.
fn settings_panel(title: &str, panel: AnyElement, cx: &App) -> AnyElement {
    settings_section(title, vec![div().p_4().child(panel).into_any_element()], cx)
}

#[cfg(test)]
mod tests {
    use super::{is_newer, new_instance_id, theme_mode, toggle_instance, usage_windows};

    #[test]
    fn compares_release_versions() {
        assert!(is_newer("0.9.2", "0.9.1"));
        assert!(is_newer("1.0.0", "0.10.9"));
        assert!(!is_newer("0.9.1", "0.9.1"));
        assert!(
            is_newer("0.9.1", "0.9.1-nightly.3"),
            "a release beats its pre-release"
        );
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn derives_free_instance_ids() {
        assert_eq!(
            new_instance_id("codex", "Work Account", &[]),
            "codex-work-account"
        );
        assert_eq!(
            new_instance_id("codex", "Work", &["codex-work".into()]),
            "codex-work-2"
        );
        assert_eq!(new_instance_id("9x", "a", &[]), "p-9x-a");
    }
    use gpui_kit::component::ThemeMode;
    use serde_json::json;

    #[test]
    fn orders_usage_windows_by_use() {
        let usage = json!({"windows": [
            {"id": "s", "label": "5-hour", "usedPercent": 12.5},
            {"id": "w", "label": "Weekly", "usedPercent": 140, "resetsAt": "2026-12-01T09:00:00Z"},
            {"id": "x", "usedPercent": 3}]});
        let windows = usage_windows(&usage);
        assert_eq!(windows.len(), 2, "a window without a label is skipped");
        assert_eq!((windows[0].0.as_str(), windows[0].1), ("Weekly", 100.));
        assert!(windows[0].2.is_some());
        assert!(usage_windows(&json!(null)).is_empty());
    }

    #[test]
    fn toggling_keeps_instance_config() {
        let settings = json!({"providerInstances": {"codex": {"driver": "codex", "config": {"binaryPath": "/x"}, "enabled": true}}});
        let m = toggle_instance(&settings, "codex", "codex", false);
        assert_eq!(
            m,
            json!({"operation": "upsert", "instanceId": "codex",
            "instance": {"driver": "codex", "config": {"binaryPath": "/x"}, "enabled": false}})
        );
        let fresh = toggle_instance(&json!({}), "pi", "pi", true);
        assert_eq!(fresh["instance"], json!({"driver": "pi", "enabled": true}));
    }

    #[test]
    fn theme_defaults_to_dark() {
        assert!(theme_mode(&json!({})) == ThemeMode::Dark);
        assert!(theme_mode(&json!({"theme": "light"})) == ThemeMode::Light);
    }
}
