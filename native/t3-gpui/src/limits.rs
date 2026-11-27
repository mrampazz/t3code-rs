//! Usage-limit recovery banner (web: `UsageLimitRecoveryBanner.tsx`, `limitRecoveryOwnsSnooze`):
//! a run stopped by a provider limit offers to resume at the reset time and to snooze until then,
//! both through `thread.metadata.update { limitRecovery }`.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::T3App;
use crate::i18n::{t, tv};
use crate::notices::{Notice, Tone, notice_action};
use gpui_kit::component::notification::Notification;

/// `shared/usageLimits.ts`: OpenAI's codes and wording for a ChatGPT plan used in other apps.
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/#settings/Usage";
const CHATGPT_USAGE_LIMIT_CODE: &str = "subscription_sharing_usage_limit_exceeded";
const CHATGPT_WORKSPACE_USAGE_LIMIT_CODE: &str =
    "subscription_sharing_workplace_individual_usage_limit_exceeded";
const CHATGPT_WORKSPACE_ADMIN_REQUEST: &str = "I've reached the limit for using my ChatGPT plan in T3 Code. Could you increase our workspace's usage limits so I can continue?\n\nGo to admin.openai.com → Workspace → Usage limits. Thank you!";
use crate::model::ThreadShell;

#[derive(Debug, PartialEq)]
pub struct Recovery {
    pub run_id: String,
    pub reset_at: Option<Timestamp>,
    /// Auto-resume and snooze can be scheduled (a future reset, nothing blocking).
    pub can_schedule: bool,
    pub resumes: bool,
    pub snoozed: bool,
    /// When the thread comes back: a snooze past the reset holds the resume until it ends.
    pub display_at: Option<Timestamp>,
}

/// The recovery state of a thread whose latest run hit a usage limit, else `None`.
pub fn recovery(t: &ThreadShell, now: Timestamp) -> Option<Recovery> {
    if t.status.as_str() != Some("failed") || t.last_error_class.as_deref() != Some("usage_limit") {
        return None;
    }
    let run_id = t.latest_run_id.clone()?;
    let reset_raw = t.usage_limit_reset_at.as_deref();
    let reset_at = reset_raw.and_then(|r| r.parse::<Timestamp>().ok());
    let stopped = t
        .latest_run_completed_at
        .as_deref()
        .unwrap_or(&t.updated_at)
        .parse::<Timestamp>()
        .ok();
    let can_schedule = t.settled_override.as_deref() != Some("settled")
        && t.pending_runtime_request.is_null()
        && matches!((reset_at, stopped), (Some(r), Some(s)) if r > s);
    let rec = &t.limit_recovery;
    let same = rec["runId"] == run_id.as_str() && rec["resetAt"].as_str() == reset_raw;
    let snoozed = same
        && rec["snooze"] == true
        && reset_at.is_some_and(|r| r > now)
        && t.snoozed_until
            .as_deref()
            .and_then(|s| s.parse::<Timestamp>().ok())
            == reset_at;
    let resumes = can_schedule && same && rec["autoResume"] == true;
    let snoozed_until = t
        .snoozed_until
        .as_deref()
        .and_then(|s| s.parse::<Timestamp>().ok());
    let display_at = match (resumes, reset_at, snoozed_until) {
        (true, Some(reset), Some(until)) if until > reset => Some(until),
        _ => reset_at,
    };
    Some(Recovery {
        run_id,
        reset_at,
        can_schedule,
        resumes,
        snoozed,
        display_at,
    })
}

impl T3App {
    /// A Stop still in progress; once the agent stops answering, offer Force stop (web:
    /// `runStopNowItem`).
    pub(crate) fn render_stop_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (run_id, unresponsive) = self.thread.run_stop()?;
        let thread_id = self.selected.clone()?;
        let c = cx.theme().colors;
        let can_force = unresponsive && self.config().thread_force_stop;
        Some(
            h_flex()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(if unresponsive { c.warning } else { c.border })
                .text_sm()
                .child(
                    v_flex()
                        .flex_1()
                        .child(if unresponsive {
                            t("Agent not responding to Stop")
                        } else {
                            crate::i18n::tc("Stopping", "run status")
                        })
                        .when(unresponsive, |col| {
                            col.child(div().text_xs().text_color(c.muted_foreground).child(t(
                                "It may be short on memory. Force stop kills its processes.",
                            )))
                        }),
                )
                .when(can_force, |row| {
                    row.child(
                        Button::new("force-stop")
                            .xsmall()
                            .outline()
                            .label(t("Force stop"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.force_stop(thread_id.clone(), run_id.clone(), cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn force_stop(&mut self, thread_id: String, run_id: String, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let payload = json!({ "threadId": thread_id, "runId": run_id });
        cx.spawn(async move |this, cx| {
            let result = crate::on_tokio(async move {
                client.call("orchestration.forceStopThread", payload).await
            })
            .await;
            if !matches!(result, Ok(Ok(_))) {
                this.update(cx, |app, cx| {
                    app.error = Some(t("Could not force stop the agent").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// `usageLimitRecoveryBannerItem`: the stopped run's limit, when it resets, and the offers to
    /// resume then or snooze until then. A ChatGPT plan's limit uses OpenAI's partner wording.
    pub(crate) fn limit_notice(
        &self,
        thread: &ThreadShell,
        cx: &mut Context<Self>,
    ) -> Option<Notice> {
        let now = Timestamp::now();
        let r = recovery(thread, now)?;
        let reset_passed = r.reset_at.is_some_and(|at| at <= now);
        let display_passed = r.display_at.is_some_and(|at| at <= now);
        let upcoming = r.resumes && !display_passed;
        let shown = if upcoming { r.display_at } else { r.reset_at };
        let description = match shown.map(crate::timeline::upcoming_time) {
            Some(time) if upcoming => tv("Resumes {time}", &[("time", &time)]),
            Some(time) if reset_passed => tv("Reset {time}", &[("time", &time)]),
            Some(time) => tv("Resets {time}", &[("time", &time)]),
            None => t("Reset time unavailable; retry manually"),
        };
        let code = thread.last_error_code.as_deref();
        let workspace = code == Some(CHATGPT_WORKSPACE_USAGE_LIMIT_CODE);
        let chatgpt = workspace || code == Some(CHATGPT_USAGE_LIMIT_CODE);
        let pending = self.limit_pending;
        let mut actions = Vec::new();
        if r.can_schedule {
            let update = |field: &str, value: bool| -> Value {
                let mut limit =
                    json!({ "runId": r.run_id, "resetAt": thread.usage_limit_reset_at });
                limit[field] = json!(value);
                json!({ "type": "thread.metadata.update", "threadId": thread.id, "limitRecovery": limit })
            };
            let (resume, snooze) = (
                update("autoResume", !r.resumes),
                update("snooze", !r.snoozed),
            );
            let saving = || t("Saving...");
            actions.push(
                notice_action(
                    "limit-resume",
                    if pending {
                        saving()
                    } else if r.resumes {
                        t("Cancel auto-resume")
                    } else if reset_passed {
                        t("Resume now")
                    } else {
                        t("Resume at reset")
                    },
                    pending,
                    cx,
                )
                .when(!pending, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.change_limit_recovery(
                            resume.clone(),
                            t("Could not change auto-resume"),
                            window,
                            cx,
                        )
                    }))
                })
                .into_any_element(),
            );
            let snooze_disabled = pending || (!r.snoozed && reset_passed);
            actions.push(
                notice_action(
                    "limit-snooze",
                    if pending {
                        saving()
                    } else if r.snoozed {
                        t("Wake now")
                    } else {
                        t("Snooze until reset")
                    },
                    snooze_disabled,
                    cx,
                )
                .when(!snooze_disabled, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.change_limit_recovery(
                            snooze.clone(),
                            t("Could not snooze thread"),
                            window,
                            cx,
                        )
                    }))
                })
                .into_any_element(),
            );
        }
        if chatgpt {
            actions.push(if workspace {
                notice_action("limit-admin", t("Copy request for admin"), false, cx)
                    .on_click(|_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            CHATGPT_WORKSPACE_ADMIN_REQUEST.to_owned(),
                        ));
                        window.push_notification(
                            Notification::success(t("Send it to your workspace admin."))
                                .title(t("Request copied")),
                            cx,
                        );
                    })
                    .into_any_element()
            } else {
                notice_action("limit-usage", t("Manage usage"), false, cx)
                    .gap_1()
                    .child(crate::ui::icon("external-link").size(px(14.)))
                    .on_click(|_, _, cx| cx.open_url(CHATGPT_USAGE_URL))
                    .into_any_element()
            });
        }
        let dark = cx.theme().mode.is_dark();
        let icon = if chatgpt {
            crate::ui::provider_logo("openai", dark)
                .map(|logo| logo.size(px(12.)).into_any_element())
        } else {
            None
        }
        .unwrap_or_else(|| crate::ui::icon("gauge").size(px(12.)).into_any_element());
        Some(Notice {
            id: format!("usage-limit-recovery:{}", r.run_id),
            tone: Tone::Warning,
            environment: true,
            icon,
            title: if chatgpt {
                t("ChatGPT usage limit reached")
            } else {
                t("Usage limit reached")
            },
            description: Some(if workspace {
                t(
                    "You've reached your workspace's limit for using your ChatGPT plan in other apps.",
                )
            } else {
                description
            }),
            on_description: None,
            actions,
            dismiss: None,
        })
    }

    /// Saves a limit recovery change; the buttons read "Saving..." until the server answers.
    fn change_limit_recovery(
        &mut self,
        command: Value,
        failure: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else { return };
        self.limit_pending = true;
        cx.notify();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = crate::on_tokio(async move {
                client.call("orchestration.dispatchCommand", command).await
            })
            .await;
            let error = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            this.update(cx, |app, cx| {
                app.limit_pending = false;
                cx.notify();
            })
            .ok();
            if let Some(error) = error {
                window_handle
                    .update(cx, |_, window, cx| {
                        window.push_notification(Notification::error(error).title(failure), cx)
                    })
                    .ok();
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::recovery;
    use crate::model::ThreadShell;
    use serde_json::json;

    fn thread(extra: serde_json::Value) -> ThreadShell {
        let mut t = json!({"id": "t", "projectId": "p", "title": "x", "updatedAt": "2026-01-01T10:00:00Z",
            "latestRunId": "r", "status": "failed", "lastErrorClass": "usage_limit",
            "usageLimitResetAt": "2026-01-01T15:00:00Z", "pendingRuntimeRequest": null});
        t.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(t).unwrap()
    }

    #[test]
    fn derives_recovery_state() {
        let now = "2026-01-01T12:00:00Z".parse().unwrap();
        let r = recovery(&thread(json!({})), now).unwrap();
        assert!(r.can_schedule && !r.resumes && !r.snoozed);
        let scheduled = thread(
            json!({"limitRecovery": {"runId": "r", "resetAt": "2026-01-01T15:00:00Z", "autoResume": true, "snooze": true},
            "snoozedUntil": "2026-01-01T15:00:00Z"}),
        );
        let r = recovery(&scheduled, now).unwrap();
        assert!(r.resumes && r.snoozed);
        assert!(
            !recovery(&thread(json!({"settledOverride": "settled"})), now)
                .unwrap()
                .can_schedule
        );
        assert!(recovery(&thread(json!({"lastErrorClass": "other"})), now).is_none());
        assert!(
            !recovery(&scheduled, "2026-01-01T16:00:00Z".parse().unwrap())
                .unwrap()
                .snoozed,
            "a passed reset ends the snooze"
        );
    }
}
