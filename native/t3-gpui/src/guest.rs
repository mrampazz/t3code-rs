//! Guest share links (web: `GuestShareControls.tsx`, `guestShareAvailability.ts`): one browser
//! watches the thread over T3 Connect and proposes messages the owner sends or dismisses.
//! Offered when the environment's `guestSharingEnabled` setting is on; the server explains any
//! other refusal (no Connect tunnel, unshareable thread).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt, dialog::DialogButtonProps, h_flex, v_flex,
};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::{t, tv};
use crate::{T3App, on_tokio};

/// The thread's share while its link is still valid.
pub fn active_share(app_thread: &Value, now: jiff::Timestamp) -> Option<&Value> {
    let share = &app_thread["guestShare"];
    let expires = share["expiresAt"]
        .as_str()?
        .parse::<jiff::Timestamp>()
        .ok()?;
    (expires > now).then_some(share)
}

impl T3App {
    fn guest_sharing_enabled(&self) -> bool {
        self.config().settings["guestSharingEnabled"] == true
    }

    pub(crate) fn render_guest_share(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(thread_id) = self
            .selected
            .clone()
            .filter(|_| self.guest_sharing_enabled())
        else {
            return div().into_any_element();
        };
        let c = cx.theme().colors;
        match active_share(&self.thread.app_thread, jiff::Timestamp::now()) {
            Some(share) => {
                let label = share["guestName"].as_str().map_or_else(
                    || t("Guest link active"),
                    |name| tv("{guestName} is a guest", &[("guestName", name)]),
                );
                let close = json!({ "type": "thread.guest-share.close", "threadId": thread_id });
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(c.cyan)
                    .child(label)
                    .child(
                        Button::new("guest-stop")
                            .xsmall()
                            .ghost()
                            .label(t("Stop sharing"))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.dispatch(close.clone(), cx)),
                            ),
                    )
                    .into_any_element()
            }
            None => Button::new("guest-share")
                .xsmall()
                .ghost()
                .label(t("Share with guest"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.create_guest_link(thread_id.clone(), window, cx)
                }))
                .into_any_element(),
        }
    }

    fn create_guest_link(
        &mut self,
        thread_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else { return };
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                client
                    .call("guestShare.create", json!({ "threadId": thread_id }))
                    .await
            })
            .await;
            let url = match result {
                Ok(Ok(reply)) => reply["url"].as_str().unwrap_or("").to_owned(),
                Ok(Err(e)) => {
                    let message = format!("{}: {e}", t("Could not create a guest link."));
                    this.update(cx, |app, cx| {
                        app.error = Some(message.into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            window_handle
                .update(cx, |_, window, cx| {
                    let input = cx.new(|cx| InputState::new(window, cx).default_value(url.clone()));
                    window.open_dialog(cx, move |dialog, _, _| {
                        let url = url.clone();
                        dialog
                            .title(t("Share with guest"))
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(Input::new(&input))
                                    .child(t("The link works once and expires in 24 hours.")),
                            )
                            .button_props(
                                DialogButtonProps::default()
                                    .ok_text(crate::i18n::tc("Copy", "verb, copy the link")),
                            )
                            .on_ok(move |_, _, cx: &mut App| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                true
                            })
                    });
                })
                .ok();
        })
        .detach();
    }

    /// Guest messages waiting for the owner, above the composer.
    pub(crate) fn render_guest_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        let share = &self.thread.app_thread["guestShare"];
        let pending: Vec<Value> = share["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["status"] == "pending")
            .cloned()
            .collect();
        let Some(thread_id) = self.selected.clone().filter(|_| !pending.is_empty()) else {
            return div().into_any_element();
        };
        let c = cx.theme().colors;
        let guest = share["guestName"]
            .as_str()
            .map_or_else(|| t("Guest"), str::to_owned);
        v_flex()
            .gap_1()
            .children(pending.into_iter().enumerate().map(|(i, m)| {
                let id = m["id"].clone();
                let command_id = uuid::Uuid::new_v4().to_string();
                let send = json!({ "type": "message.dispatch", "commandId": command_id, "messageId": command_id,
                    "threadId": thread_id, "guestMessageId": id, "text": "", "attachments": [],
                    "dispatchMode": { "type": "queue_after_active" }, "createdBy": "user", "creationSource": "web" });
                let dismiss = json!({ "type": "thread.guest-message.dismiss", "threadId": thread_id, "guestMessageId": id });
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(c.cyan)
                    .text_sm()
                    .child(div().text_xs().text_color(c.cyan).child(guest.clone()))
                    .child(div().flex_1().truncate().child(m["text"].as_str().unwrap_or("").replace('\n', " ")))
                    .child(
                        Button::new(("guest-dismiss", i))
                            .xsmall()
                            .ghost()
                            .label(t("Dismiss"))
                            .on_click(cx.listener(move |this, _, _, cx| this.dispatch(dismiss.clone(), cx))),
                    )
                    .child(
                        Button::new(("guest-send", i))
                            .xsmall()
                            .primary()
                            .label(t("Send"))
                            .on_click(cx.listener(move |this, _, _, cx| this.dispatch(send.clone(), cx))),
                    )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::active_share;
    use serde_json::json;

    #[test]
    fn shares_until_they_expire() {
        let now = "2026-01-01T12:00:00Z".parse().unwrap();
        let thread = json!({"guestShare": {"expiresAt": "2026-01-02T00:00:00Z", "messages": []}});
        assert!(active_share(&thread, now).is_some());
        let expired = json!({"guestShare": {"expiresAt": "2026-01-01T00:00:00Z"}});
        assert!(active_share(&expired, now).is_none());
        assert!(active_share(&json!({"guestShare": null}), now).is_none());
    }
}
