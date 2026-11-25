//! Scheduled tasks (web: `ScheduledTasksSettings.tsx`, contract `scheduledTask.ts`): prompts the
//! server runs on a schedule. Listed live in Settings (`scheduledTasks.subscribe` while open)
//! with on/off, Run now and Delete, and created from a short schedule phrase.
//! shortcut: webhook and one-off schedules are listed but not created here, tasks are not
//! edited in place, and new tasks start a new thread in the project folder with its default model.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt, dialog::DialogButtonProps, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

fn time_of_day(s: &str) -> Option<String> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u8, u8) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60 && s.len() <= 5).then(|| format!("{h:02}:{m:02}"))
}

/// A `ScheduledTaskUpsertSchedule` from `every 30m` / `every 2h` / `daily 09:00` /
/// `weekdays 09:00`.
pub fn parse_schedule(input: &str) -> Option<Value> {
    let input = input.trim().to_lowercase();
    let (head, rest) = input.split_once(' ')?;
    let rest = rest.trim();
    match head {
        "every" => {
            let split = rest.find(|c: char| !c.is_ascii_digit())?;
            let n: u64 = rest[..split].parse().ok().filter(|n| *n > 0)?;
            let minutes = match rest[split..].trim() {
                "m" | "min" | "minutes" => n,
                "h" | "hours" => n * 60,
                "d" | "days" => n * 24 * 60,
                _ => return None,
            };
            Some(json!({ "type": "interval", "everyMs": minutes * 60_000 }))
        }
        "daily" => Some(json!({ "type": "fixed_time", "timeOfDay": time_of_day(rest)? })),
        "weekdays" => Some(
            json!({ "type": "fixed_time", "timeOfDay": time_of_day(rest)?, "weekdays": [1, 2, 3, 4, 5] }),
        ),
        _ => None,
    }
}

/// One line describing a stored schedule.
pub fn describe(schedule: &Value) -> String {
    match schedule["type"].as_str() {
        Some("interval") => {
            let minutes = schedule["everyMs"].as_u64().unwrap_or(0) / 60_000;
            if minutes.is_multiple_of(24 * 60) {
                format!("every {}d", minutes / (24 * 60))
            } else if minutes.is_multiple_of(60) {
                format!("every {}h", minutes / 60)
            } else {
                format!("every {minutes}m")
            }
        }
        Some("fixed_time") => {
            let time = schedule["timeOfDay"].as_str().unwrap_or("");
            match schedule["weekdays"].as_array().map(Vec::len) {
                None | Some(7) => format!("daily {time}"),
                Some(_) if schedule["weekdays"] == json!([1, 2, 3, 4, 5]) => {
                    format!("weekdays {time}")
                }
                Some(n) => format!("{n} days a week, {time}"),
            }
        }
        Some("once") => format!("once at {}", schedule["runAt"].as_str().unwrap_or("")),
        Some("webhook") => "webhook".into(),
        _ => String::new(),
    }
}

impl T3App {
    pub(crate) fn subscribe_scheduled(&mut self, cx: &mut Context<Self>) {
        self.release_scheduled();
        let Some(client) = self.client() else { return };
        let (sub, mut rx) = client.request("scheduledTasks.subscribe", json!({}));
        self.scheduled_sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(Ok(list)) = rx.recv().await {
                let alive = this.update(cx, |app, cx| {
                    if app.scheduled_sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    app.scheduled = list["tasks"].as_array().cloned().unwrap_or_default();
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn release_scheduled(&mut self) {
        if let (Some(client), Some(old)) = (self.client(), self.scheduled_sub.take()) {
            client.cancel(&old);
        }
    }

    fn scheduled_call(&mut self, method: &'static str, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            if let Some(e) = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            } {
                this.update(cx, |app, cx| {
                    app.settings_status = Some(format!("{method} failed: {e}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn render_scheduled(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        v_flex()
            .children(self.scheduled.iter().enumerate().map(|(i, task)| {
                let id = task["id"].clone();
                let enabled = task["enabled"] == true;
                let next = task["nextRunAt"]
                    .as_str()
                    .and_then(|n| n.parse::<jiff::Timestamp>().ok())
                    .map(|n| {
                        n.to_zoned(jiff::tz::TimeZone::system())
                            .strftime("next %b %-d, %H:%M")
                            .to_string()
                    });
                let detail = [Some(describe(&task["schedule"])), next]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let (toggle_id, run_id, delete_id) = (id.clone(), id.clone(), id);
                h_flex()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(c.border)
                    .text_sm()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(task["title"].as_str().unwrap_or("").to_owned())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .child(
                        Button::new(("task-toggle", i))
                            .small()
                            .label(if enabled { t("On") } else { t("Off") })
                            .when(enabled, |b| b.primary())
                            .when(!enabled, |b| b.outline())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scheduled_call(
                                    "scheduledTasks.setEnabled",
                                    json!({ "id": toggle_id, "enabled": !enabled }),
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new(("task-run", i))
                            .small()
                            .ghost()
                            .label(t("Run now"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scheduled_call(
                                    "scheduledTasks.runNow",
                                    json!({ "id": run_id }),
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new(("task-delete", i))
                            .small()
                            .ghost()
                            .label("✕")
                            .tooltip(t("Delete"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scheduled_call(
                                    "scheduledTasks.delete",
                                    json!({ "id": delete_id }),
                                    cx,
                                )
                            })),
                    )
            }))
            .child(
                div().py_2().child(
                    Button::new("task-new")
                        .small()
                        .outline()
                        .label(t("New scheduled task"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_new_task(window, cx)),
                        ),
                ),
            )
    }

    fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .map(|t| t.project_id.clone())
            .or_else(|| self.shell().projects.first().map(|p| p.id.clone()));
        let Some(project_id) = project else {
            self.settings_status = Some(t("Add a project first.").into());
            return cx.notify();
        };
        let model = self
            .config()
            .model_for(self.shell().projects.iter().find(|p| p.id == project_id));
        let Some(model) = model else { return };
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let title = field("Title", window, cx);
        let prompt = field("Prompt", window, cx);
        let schedule = field("every 30m  ·  daily 09:00  ·  weekdays 09:00", window, cx);
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, title, prompt, schedule, project_id, model) =
                (app.clone(), title.clone(), prompt.clone(), schedule.clone(), project_id.clone(), model.clone());
            dialog
                .title(t("New scheduled task"))
                .child(v_flex().gap_2().child(Input::new(&title)).child(Input::new(&prompt)).child(Input::new(&schedule)))
                .button_props(DialogButtonProps::default().ok_text(t("Save")).show_cancel(true))
                .on_ok(move |_, _, cx: &mut App| {
                    let (title, prompt) = (title.read(cx).value().trim().to_string(), prompt.read(cx).value().trim().to_string());
                    let Some(schedule) = parse_schedule(&schedule.read(cx).value()) else { return false };
                    if title.is_empty() || prompt.is_empty() {
                        return false;
                    }
                    let payload = json!({ "title": title, "prompt": prompt, "enabled": true, "schedule": schedule,
                        "projectId": project_id, "workspaceStrategy": { "type": "root" }, "modelSelection": model,
                        "runtimeMode": "full-access", "interactionMode": "default", "creationSource": "web" });
                    app.update(cx, |this, cx| this.scheduled_call("scheduledTasks.upsert", payload, cx)).ok();
                    true
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{describe, parse_schedule};
    use serde_json::json;

    #[test]
    fn parses_and_describes_schedules() {
        assert_eq!(
            parse_schedule("every 30m"),
            Some(json!({"type": "interval", "everyMs": 1_800_000}))
        );
        assert_eq!(parse_schedule("Every 2h").unwrap()["everyMs"], 7_200_000);
        assert_eq!(
            parse_schedule("daily 9:05"),
            Some(json!({"type": "fixed_time", "timeOfDay": "09:05"}))
        );
        assert_eq!(
            parse_schedule("weekdays 18:00").unwrap()["weekdays"],
            json!([1, 2, 3, 4, 5])
        );
        assert_eq!(parse_schedule("daily 25:00"), None);
        assert_eq!(parse_schedule("sometimes"), None);
        assert_eq!(describe(&parse_schedule("every 90m").unwrap()), "every 90m");
        assert_eq!(describe(&parse_schedule("every 1d").unwrap()), "every 1d");
        assert_eq!(
            describe(&parse_schedule("weekdays 09:00").unwrap()),
            "weekdays 09:00"
        );
        assert_eq!(describe(&json!({"type": "webhook"})), "webhook");
    }
}
