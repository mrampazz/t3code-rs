//! Settings view (web: `apps/web/src/components/settings/`). Server settings come from the
//! config stream and change through `server.updateSettings`; appearance is a client preference.

use crate::i18n::{t, tv};
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::composer::RUNTIME_MODES;
use crate::{T3App, on_tokio};

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
        self.settings_open = !self.settings_open;
        self.new_pairing_url = None;
        if self.settings_open {
            self.subscribe_access(cx);
            self.subscribe_scheduled(cx);
        } else {
            self.release_access();
            self.release_scheduled();
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
        Theme::change(theme_mode(&self.prefs), Some(window), cx);
        cx.notify();
    }

    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let section = |title: &str| {
            let title = t(title);
            div()
                .pt_6()
                .pb_2()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string())
        };
        let row = || {
            h_flex()
                .py_2()
                .gap_3()
                .border_b_1()
                .border_color(c.border)
                .text_sm()
        };
        let settings = self.config().settings.clone();

        let mut providers = v_flex();
        for p in &self.config().providers {
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
            let usage_rows = v_flex()
                .gap_1()
                .pb_2()
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
                        .text_xs()
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
                .children(
                    unavailable.map(|m| div().text_xs().text_color(c.muted_foreground).child(m)),
                );
            providers = providers.child(
                row()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                p.display_name
                                    .clone()
                                    .unwrap_or_else(|| p.instance_id.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .when(p.instance_id != p.driver, |row| {
                        let id = p.instance_id.clone();
                        row.child(
                            Button::new(SharedString::from(format!("provider-remove-{id}")))
                                .small()
                                .ghost()
                                .label("✕")
                                .tooltip(t("Remove"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let mutation = json!({ "operation": "remove", "instanceId": id });
                                    this.update_settings(json!({ "patch": {}, "providerInstanceMutation": mutation }), cx);
                                })),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("provider-{}", p.instance_id)))
                            .small()
                            .label(if enabled { "Enabled" } else { "Disabled" })
                            .when(enabled, |b| b.primary())
                            .when(!enabled, |b| b.outline())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mutation = toggle_instance(&settings, &id, &driver, !enabled);
                                this.update_settings(
                                    json!({ "patch": {}, "providerInstanceMutation": mutation }),
                                    cx,
                                );
                            })),
                    ),
            );
            providers = providers.child(usage_rows);
        }
        let mut drivers: Vec<String> = self
            .config()
            .providers
            .iter()
            .map(|p| p.driver.clone())
            .collect();
        drivers.sort();
        drivers.dedup();
        let add_app = cx.entity().downgrade();
        providers = providers.child(
            div().py_2().child(
                Button::new("provider-add")
                    .small()
                    .outline()
                    .label(t("Add provider instance"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for driver in &drivers {
                            let (app, driver) = (add_app.clone(), driver.clone());
                            menu = menu.item(PopupMenuItem::new(driver.clone()).on_click(
                                move |_, window, cx| {
                                    app.update(cx, |this, cx| {
                                        this.add_provider_instance(driver.clone(), window, cx)
                                    })
                                    .ok();
                                },
                            ));
                        }
                        menu
                    }),
            ),
        );

        let runtime = settings["defaultRuntimeMode"]
            .as_str()
            .unwrap_or("full-access")
            .to_string();
        let runtime_label = RUNTIME_MODES
            .iter()
            .find(|(m, _)| *m == runtime)
            .map_or(runtime.clone(), |(_, l)| l.to_string());
        let app = cx.entity().downgrade();
        let presets =
            crate::sidebar::snooze_presets(&jiff::Zoned::now(), settings.get("snoozePresets"));
        let light = theme_mode(&self.prefs) == ThemeMode::Light;
        let server = self.env().map(|e| e.key.clone()).unwrap_or_default();

        div().id("settings").flex_1().h_full().overflow_y_scroll().child(
            v_flex()
                .mx_auto()
                .max_w(px(720.))
                .w_full()
                .p_6()
                .child(
                    h_flex()
                        .child(div().flex_1().text_xl().font_weight(FontWeight::SEMIBOLD).child(t("Settings")))
                        .children(self.settings_status.clone().map(|s| div().text_sm().text_color(c.muted_foreground).child(s)))
                        .child(
                            Button::new("settings-close")
                                .small()
                                .ghost()
                                .label(t("Close"))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                        ),
                )
                .child(section("Providers"))
                .child(providers)
                .child(section("New threads"))
                .child(
                    row().child(div().flex_1().child(t("Default permission mode"))).child(
                        Button::new("default-runtime").small().outline().label(t(&runtime_label)).dropdown_menu(
                            move |mut menu, _, _| {
                                for (mode, label) in RUNTIME_MODES {
                                    let app = app.clone();
                                    menu = menu.item(PopupMenuItem::new(t(label)).checked(mode == runtime).on_click(
                                        move |_, _, cx| {
                                            app.update(cx, |this, cx| {
                                                this.update_settings(json!({ "patch": { "defaultRuntimeMode": mode } }), cx)
                                            })
                                            .ok();
                                        },
                                    ));
                                }
                                menu
                            },
                        ),
                    ),
                )
                .child({
                    let stored: Vec<Value> = settings["snoozePresets"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(crate::sidebar::default_presets);
                    let resolved = presets.iter().map(|p| p.label.split(" · ").next().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ");
                    let full = stored.len() >= 8;
                    v_flex()
                        .child(
                            row()
                                .child(v_flex().flex_1().child(t("Snooze options")).child(div().text_xs().text_color(c.muted_foreground).child(resolved)))
                                .child({
                                    let stored = stored.clone();
                                    Button::new("snooze-add").small().outline().label(t("Add")).disabled(full).on_click(cx.listener(
                                        move |this, _, window, cx| this.add_snooze_preset(stored.clone(), window, cx),
                                    ))
                                }),
                        )
                        .child(h_flex().flex_wrap().gap_1().py_1().children(stored.iter().enumerate().map(|(i, p)| {
                            let remaining: Vec<Value> = stored.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, v)| v.clone()).collect();
                            Button::new(("snooze-preset", i))
                                .xsmall()
                                .ghost()
                                .label(format!("{} ✕", crate::sidebar::preset_text(p)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_settings(json!({ "patch": { "snoozePresets": remaining } }), cx)
                                }))
                        })))
                })
                .child(section("Appearance"))
                .child(row().child(div().flex_1().child(t("Language"))).child({
                    let app = cx.entity().downgrade();
                    let current = crate::i18n::locale();
                    let name = crate::i18n::LOCALES.iter().find(|(c, _)| *c == current).map_or("English", |(_, n)| n);
                    Button::new("language").small().outline().label(name.to_string()).dropdown_menu(move |mut menu, _, _| {
                        for (code, name) in crate::i18n::LOCALES {
                            let app = app.clone();
                            menu = menu.item(PopupMenuItem::new(name).checked(code == current).on_click(move |_, _, cx| {
                                app.update(cx, |this, cx| this.set_language(code, cx)).ok();
                            }));
                        }
                        menu
                    })
                }))
                .child(
                    row()
                        .child(div().flex_1().child(t("Theme")))
                        .child(
                            Button::new("theme-dark")
                                .small()
                                .label(t("Dark"))
                                .when(!light, |b| b.primary())
                                .when(light, |b| b.ghost())
                                .on_click(cx.listener(|this, _, window, cx| this.set_theme(false, window, cx))),
                        )
                        .child(
                            Button::new("theme-light")
                                .small()
                                .label(t("Light"))
                                .when(light, |b| b.primary())
                                .when(!light, |b| b.ghost())
                                .on_click(cx.listener(|this, _, window, cx| this.set_theme(true, window, cx))),
                        ),
                )
                .child({
                    let on = self.prefs["workingShelf"] != false;
                    row().child(div().flex_1().child(t("Working section"))).child(
                        Button::new("working-shelf")
                            .small()
                            .label(if on { t("On") } else { t("Off") })
                            .when(on, |b| b.primary())
                            .when(!on, |b| b.outline())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prefs["workingShelf"] = json!(!on);
                                save_prefs(&this.prefs);
                                cx.notify();
                            })),
                    )
                })
                .child({
                    let queue = self.prefs["followUpBehavior"] == "queue";
                    row().child(div().flex_1().child(t("Follow-up behavior"))).child(
                        Button::new("follow-up")
                            .small()
                            .outline()
                            .label(if queue { t("Queue") } else { t("Steer") })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prefs["followUpBehavior"] = json!(if queue { "steer" } else { "queue" });
                                save_prefs(&this.prefs);
                                cx.notify();
                            })),
                    )
                })
                .child({
                    let on = self.prefs["projectFolders"] == true;
                    row().child(div().flex_1().child(t("Boring sidebar (group by project)"))).child(
                        Button::new("project-folders")
                            .small()
                            .label(if on { t("On") } else { t("Off") })
                            .when(on, |b| b.primary())
                            .when(!on, |b| b.outline())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prefs["projectFolders"] = json!(!on);
                                save_prefs(&this.prefs);
                                cx.notify();
                            })),
                    )
                })
                .child({
                    let on = self.prefs["inAppNotifications"] != false;
                    row().child(div().flex_1().child(t("In-app notifications"))).child(
                        Button::new("in-app-notifications")
                            .small()
                            .label(if on { t("On") } else { t("Off") })
                            .when(on, |b| b.primary())
                            .when(!on, |b| b.outline())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prefs["inAppNotifications"] = json!(!on);
                                save_prefs(&this.prefs);
                                cx.notify();
                            })),
                    )
                })
                .child(section("Thread defaults"))
                .child({
                    let days = settings["sidebarAutoSettleAfterDays"].as_u64();
                    let label = days.map_or_else(|| t("Never"), |d| format!("{d} d"));
                    let app = cx.entity().downgrade();
                    row().child(div().flex_1().child(t("Auto-settle inactive threads"))).child(
                        Button::new("auto-settle-days").small().outline().label(label).dropdown_menu(move |mut menu, _, _| {
                            for option in [None, Some(1u64), Some(3), Some(7), Some(14), Some(30)] {
                                let app = app.clone();
                                let text = option.map_or_else(|| t("Never"), |d| format!("{d} d"));
                                menu = menu.item(PopupMenuItem::new(text).checked(option == days).on_click(move |_, _, cx| {
                                    app.update(cx, |this, cx| this.update_settings(json!({ "patch": { "sidebarAutoSettleAfterDays": option } }), cx)).ok();
                                }));
                            }
                            menu
                        }),
                    )
                })
                .child({
                    let mode = settings["defaultThreadEnvMode"].as_str().unwrap_or("local").to_owned();
                    let label = |m: &str| if m == "worktree" { t("New worktree") } else { t("Local") };
                    let app = cx.entity().downgrade();
                    row().child(div().flex_1().child(t("New thread mode"))).child(
                        Button::new("default-env-mode").small().outline().label(label(&mode)).dropdown_menu(move |mut menu, _, _| {
                            for option in ["local", "worktree"] {
                                let app = app.clone();
                                menu = menu.item(PopupMenuItem::new(label(option)).checked(option == mode).on_click(move |_, _, cx| {
                                    app.update(cx, |this, cx| this.update_settings(json!({ "patch": { "defaultThreadEnvMode": option } }), cx)).ok();
                                }));
                            }
                            menu
                        }),
                    )
                })
                .child({
                    let current = settings["threadTitleInstructions"].as_str().unwrap_or("").to_owned();
                    row()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(t("Thread title instructions"))
                                .child(div().text_xs().text_color(c.muted_foreground).truncate().child(current.clone())),
                        )
                        .child(
                            Button::new("title-instructions")
                                .small()
                                .outline()
                                .label(t("Edit"))
                                .on_click(cx.listener(move |this, _, window, cx| this.edit_title_instructions(current.clone(), window, cx))),
                        )
                })
                .children(SERVER_TOGGLES.iter().map(|(title, rows)| {
                    v_flex().child(section(title)).children(rows.iter().map(|(key, label)| {
                        // Absent keys come from servers that predate the setting.
                        let value = settings.get(*key).and_then(Value::as_bool);
                        row().child(div().flex_1().child(t(label))).children(value.map(|on| {
                            Button::new(SharedString::from(format!("setting-{key}")))
                                .small()
                                .label(if on { t("On") } else { t("Off") })
                                .when(on, |b| b.primary())
                                .when(!on, |b| b.outline())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_settings(json!({ "patch": { *key: !on } }), cx)
                                }))
                        }))
                    }))
                }))
                .child(section("Scheduled tasks"))
                .child(self.render_scheduled(cx))
                .child(section("Sharing & access"))
                .child(self.render_access(cx))
                .child(section("Servers"))
                .children(self.envs.iter().enumerate().map(|(i, e)| {
                    let active = e.key == server;
                    let status = match (&e.error, &e.client) {
                        (Some(error), _) => error.to_string(),
                        (None, Some(_)) => "Connected".to_string(),
                        (None, None) => "Connecting…".to_string(),
                    };
                    let (switch_key, remove_key) = (e.key.clone(), e.key.clone());
                    row()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(e.label())
                                .child(div().text_xs().text_color(c.muted_foreground).truncate().child(status)),
                        )
                        .child(
                            Button::new(("server-switch", i))
                                .small()
                                .label(if active { "Active" } else { "Switch" })
                                .when(active, |b| b.primary())
                                .when(!active, |b| b.ghost())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.release_access();
                                    this.activate(&switch_key, cx);
                                    this.new_pairing_url = None;
                                    this.subscribe_access(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("server-disconnect", i))
                                .small()
                                .outline()
                                .label(t("Disconnect"))
                                .on_click(cx.listener(move |this, _, _, cx| this.remove_env(&remove_key, cx))),
                        )
                }))
                .child(
                    row().child(
                        Button::new("settings-add-server")
                            .small()
                            .ghost()
                            .label(t("+ Connect another server"))
                            .on_click(cx.listener(|this, _, _, cx| this.add_environment(cx))),
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{new_instance_id, theme_mode, toggle_instance, usage_windows};

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
