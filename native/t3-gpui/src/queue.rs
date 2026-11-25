//! Messages queued behind the running turn (web: `QueuedRunsControl.tsx`): send one now
//! (start it, or steer it into the active turn) or remove it from the queue.
//! shortcut: "Send now" is offered whenever a turn is active and the server decides whether the
//! provider can steer (the web checks provider capabilities first); queued edits are not ported.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::T3App;
use crate::i18n::t;

/// `queued-run.start` when idle, else `queued-message.promote-to-steer` into the active run.
pub fn send_now_command(thread_id: &str, run_id: &str, active_run: Option<&str>) -> Value {
    match active_run {
        None => json!({ "type": "queued-run.start", "threadId": thread_id, "runId": run_id }),
        Some(target) => json!({ "type": "queued-message.promote-to-steer", "threadId": thread_id,
            "queuedRunId": run_id, "targetRunId": target }),
    }
}

impl T3App {
    pub(crate) fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let queued = self.thread.queued_runs();
        let Some(thread_id) = self.selected.clone().filter(|_| !queued.is_empty()) else {
            return div().into_any_element();
        };
        let active = self.thread.active_run().map(str::to_owned);
        v_flex()
            .gap_1()
            .text_sm()
            .child(
                div()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child(format!("{} {}", queued.len(), t("queued"))),
            )
            .children(queued.into_iter().enumerate().map(|(i, q)| {
                let send = send_now_command(&thread_id, &q.run_id, active.as_deref());
                let cancel = json!({ "type": "queued-run.cancel", "threadId": thread_id, "runId": q.run_id });
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(c.secondary)
                    .child(div().flex_1().truncate().child(q.text.replace('\n', " ")))
                    .child(
                        Button::new(("queue-send", i))
                            .xsmall()
                            .ghost()
                            .label(t("Send now"))
                            .on_click(cx.listener(move |this, _, _, cx| this.dispatch(send.clone(), cx))),
                    )
                    .child(
                        Button::new(("queue-remove", i))
                            .xsmall()
                            .ghost()
                            .label("✕")
                            .tooltip(t("Remove from queue"))
                            .on_click(cx.listener(move |this, _, _, cx| this.dispatch(cancel.clone(), cx))),
                    )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::send_now_command;

    #[test]
    fn sends_now_by_starting_or_steering() {
        assert_eq!(send_now_command("t", "q", None)["type"], "queued-run.start");
        let steer = send_now_command("t", "q", Some("a"));
        assert_eq!(steer["type"], "queued-message.promote-to-steer");
        assert_eq!(steer["targetRunId"], "a");
        assert_eq!(steer["queuedRunId"], "q");
    }
}
