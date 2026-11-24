//! Settings view (web: `apps/web/src/components/settings/`). Server settings come from the
//! config stream and change through `server.updateSettings`; appearance is a client preference.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Sizable, Theme, ThemeMode, h_flex, v_flex};
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

fn save_prefs(prefs: &Value) {
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
        cx.notify();
    }

    fn update_settings(&mut self, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
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
                        app.config.settings = settings;
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

    fn set_theme(&mut self, light: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs["theme"] = json!(if light { "light" } else { "dark" });
        save_prefs(&self.prefs);
        Theme::change(theme_mode(&self.prefs), Some(window), cx);
        cx.notify();
    }

    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let section = |title: &str| {
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
        let settings = self.config.settings.clone();

        let mut providers = v_flex();
        for p in &self.config.providers {
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
        }

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
        let server = self
            .session
            .as_ref()
            .map(|s| s.base_url.clone())
            .unwrap_or_default();

        div().id("settings").flex_1().h_full().overflow_y_scroll().child(
            v_flex()
                .mx_auto()
                .max_w(px(720.))
                .w_full()
                .p_6()
                .child(
                    h_flex()
                        .child(div().flex_1().text_xl().font_weight(FontWeight::SEMIBOLD).child("Settings"))
                        .children(self.settings_status.clone().map(|s| div().text_sm().text_color(c.muted_foreground).child(s)))
                        .child(
                            Button::new("settings-close")
                                .small()
                                .ghost()
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                        ),
                )
                .child(section("Providers"))
                .child(providers)
                .child(section("New threads"))
                .child(
                    row().child(div().flex_1().child("Default permission mode")).child(
                        Button::new("default-runtime").small().outline().label(runtime_label).dropdown_menu(
                            move |mut menu, _, _| {
                                for (mode, label) in RUNTIME_MODES {
                                    let app = app.clone();
                                    menu = menu.item(PopupMenuItem::new(label).checked(mode == runtime).on_click(
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
                .child(row().child(div().flex_1().child("Snooze presets")).child(
                    div().text_xs().text_color(c.muted_foreground).child(
                        presets.iter().map(|p| p.label.split(" · ").next().unwrap_or("").to_string()).collect::<Vec<_>>().join(", "),
                    ),
                ))
                .child(section("Appearance"))
                .child(
                    row()
                        .child(div().flex_1().child("Theme"))
                        .child(
                            Button::new("theme-dark")
                                .small()
                                .label("Dark")
                                .when(!light, |b| b.primary())
                                .when(light, |b| b.ghost())
                                .on_click(cx.listener(|this, _, window, cx| this.set_theme(false, window, cx))),
                        )
                        .child(
                            Button::new("theme-light")
                                .small()
                                .label("Light")
                                .when(light, |b| b.primary())
                                .when(!light, |b| b.ghost())
                                .on_click(cx.listener(|this, _, window, cx| this.set_theme(true, window, cx))),
                        ),
                )
                .child(section("Connection"))
                .child(
                    row()
                        .child(v_flex().flex_1().child("Server").child(div().text_xs().text_color(c.muted_foreground).child(server)))
                        .child(
                            Button::new("settings-disconnect")
                                .small()
                                .outline()
                                .label("Disconnect")
                                .on_click(cx.listener(|this, _, _, cx| this.forget_session(cx))),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{theme_mode, toggle_instance};
    use gpui_kit::component::ThemeMode;
    use serde_json::json;

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
