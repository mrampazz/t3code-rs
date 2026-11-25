//! Usage-limit recovery banner (web: `UsageLimitRecoveryBanner.tsx`, `limitRecoveryOwnsSnooze`):
//! a run stopped by a provider limit offers to resume at the reset time and to snooze until then,
//! both through `thread.metadata.update { limitRecovery }`.
//! shortcut: the ChatGPT partner notice and its admin/usage buttons are not ported.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::T3App;
use crate::i18n::{t, tv};
use crate::model::ThreadShell;

#[derive(Debug, PartialEq)]
pub struct Recovery {
    pub run_id: String,
    pub reset_at: Option<Timestamp>,
    /// Auto-resume and snooze can be scheduled (a future reset, nothing blocking).
    pub can_schedule: bool,
    pub resumes: bool,
    pub snoozed: bool,
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
    Some(Recovery {
        run_id,
        reset_at,
        can_schedule,
        resumes: can_schedule && same && rec["autoResume"] == true,
        snoozed,
    })
}

impl T3App {
    pub(crate) fn render_limit_banner(
        &self,
        thread: &ThreadShell,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = Timestamp::now();
        let r = recovery(thread, now)?;
        let c = cx.theme().colors;
        let passed = r.reset_at.is_some_and(|at| at <= now);
        let when = r.reset_at.map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%b %-d, %H:%M")
                .to_string()
        });
        let description = match (&when, r.resumes && !passed, passed) {
            (Some(time), true, _) => tv("Resumes {time}", &[("time", time)]),
            (Some(time), false, true) => tv("Reset {time}", &[("time", time)]),
            (Some(time), false, false) => tv("Resets {time}", &[("time", time)]),
            (None, ..) => t("Reset time unavailable; retry manually"),
        };
        let update = |field: &str, value: bool| -> Value {
            let mut limit = json!({ "runId": r.run_id, "resetAt": thread.usage_limit_reset_at });
            limit[field] = json!(value);
            json!({ "type": "thread.metadata.update", "threadId": thread.id, "limitRecovery": limit })
        };
        let resume = update("autoResume", !r.resumes);
        let snooze = update("snooze", !r.snoozed);
        Some(
            v_flex()
                .gap_1()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(c.warning)
                .text_sm()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("Usage limit reached")),
                )
                .child(div().text_color(c.muted_foreground).child(description))
                .when(r.can_schedule, |col| {
                    col.child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("limit-resume")
                                    .xsmall()
                                    .ghost()
                                    .label(if r.resumes {
                                        t("Cancel auto-resume")
                                    } else if passed {
                                        t("Resume now")
                                    } else {
                                        t("Resume at reset")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dispatch(resume.clone(), cx)
                                    })),
                            )
                            .child(
                                Button::new("limit-snooze")
                                    .xsmall()
                                    .ghost()
                                    .label(if r.snoozed {
                                        t("Wake now")
                                    } else {
                                        t("Snooze until reset")
                                    })
                                    .disabled(!r.snoozed && passed)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dispatch(snooze.clone(), cx)
                                    })),
                            ),
                    )
                })
                .into_any_element(),
        )
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
