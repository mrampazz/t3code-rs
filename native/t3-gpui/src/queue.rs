//! Messages queued behind the running turn (web: `QueuedRunsControl.tsx`): send one now
//! (start it, or steer it into the active turn) or remove it from the queue.
//! A queued message can also be edited in the composer (`useQueuedRunEdit`): its text replaces
//! the draft, which comes back on save or cancel.
//! shortcut: "Send now" is offered whenever a turn is active and the server decides whether the
//! provider can steer (the web checks provider capabilities first). Edits change the text only;
//! attachments and context stay as queued.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
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

/// A queued message open in the composer, and the draft it set aside.
pub struct QueuedEdit {
    pub thread_id: String,
    pub run_id: String,
    pub draft: String,
}

impl T3App {
    /// Opens a queued message (the latest when `run_id` is `None`) in the composer.
    pub(crate) fn edit_queued(
        &mut self,
        run_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(thread_id) = self.selected.clone() else {
            return false;
        };
        let queued = self.thread.queued_runs();
        let Some(run) = (match run_id {
            Some(id) => queued.iter().find(|q| q.run_id == id),
            None => queued.last(),
        }) else {
            return false;
        };
        let draft = match self.queued_edit.take() {
            Some(edit) => edit.draft,
            None => self.composer.read(cx).value().to_string(),
        };
        let text = run.text.clone();
        self.queued_edit = Some(QueuedEdit {
            thread_id,
            run_id: run.run_id.clone(),
            draft,
        });
        self.composer.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
        true
    }

    /// Puts the draft back; the queued message stays as it was.
    pub(crate) fn cancel_queued_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = self.queued_edit.take() {
            self.composer
                .update(cx, |input, cx| input.set_value(edit.draft, window, cx));
            cx.notify();
        }
    }

    /// Saves the composer text into the queued message (`queued-run.edit`).
    pub(crate) fn save_queued_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(edit) = self.queued_edit.take() else {
            return;
        };
        self.dispatch(
            json!({ "type": "queued-run.edit", "threadId": edit.thread_id, "runId": edit.run_id, "text": text }),
            cx,
        );
        self.composer
            .update(cx, |input, cx| input.set_value(edit.draft, window, cx));
        cx.notify();
    }

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
                let editing = self
                    .queued_edit
                    .as_ref()
                    .is_some_and(|e| e.run_id == q.run_id);
                let run_id = q.run_id.clone();
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(c.secondary)
                    .child(div().flex_1().truncate().child(q.text.replace('\n', " ")))
                    .when(editing, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .child(t("Editing in the composer")),
                        )
                        .child(
                            Button::new(("queue-cancel-edit", i))
                                .xsmall()
                                .ghost()
                                .label(t("Cancel"))
                                .tooltip(t("Cancel editing queued message"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_queued_edit(window, cx)
                                })),
                        )
                    })
                    .when(!editing, |row| {
                        row.child(
                            Button::new(("queue-edit", i))
                                .xsmall()
                                .ghost()
                                .label("✎")
                                .tooltip(t("Edit in the composer"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_queued(Some(&run_id), window, cx);
                                })),
                        )
                    })
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
