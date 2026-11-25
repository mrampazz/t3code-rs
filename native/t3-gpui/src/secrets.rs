//! Secret requests (web: `SecretRequestCard.tsx`, `client-runtime/src/secretRequest.ts`): an agent
//! asks for a secret; the user types it into masked fields or declines. The value goes straight to
//! `secrets.answerRequest` and never into the chat.
//! shortcut: no secret-provider references (1Password), no link found inside `reason`, and
//! requests inherited from a fork show as pending here; the server refuses answering those.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    Sizable, ThemeColor, WindowExt, dialog::DialogButtonProps, h_flex, v_flex,
};
use gpui_kit::*;
use serde_json::{Map, Value, json};

use crate::i18n::t;
use crate::model::TurnItem;
use crate::{T3App, on_tokio};

/// (key, label, placeholder, masked) per input; one unnamed input without `fields`.
pub fn inputs(item: &Value) -> Vec<(String, String, String, bool)> {
    match item["fields"].as_array() {
        None => vec![(
            String::new(),
            item["label"].as_str().unwrap_or("").to_owned(),
            item["placeholder"]
                .as_str()
                .map_or_else(|| t("Paste the secret"), str::to_owned),
            true,
        )],
        Some(fields) => fields
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap_or("").to_owned(),
                    f["label"].as_str().unwrap_or("").to_owned(),
                    f["placeholder"].as_str().unwrap_or("").to_owned(),
                    f["secret"] != false,
                )
            })
            .collect(),
    }
}

/// `secretRequestAnswerInput` for a typed answer; `None` while any value is empty.
pub fn save_answer(item: &Value, values: &[(String, String)]) -> Option<Value> {
    let values: Vec<(String, String)> = values
        .iter()
        .map(|(k, v)| (k.clone(), v.trim().to_owned()))
        .collect();
    if values.is_empty() || values.iter().any(|(_, v)| v.is_empty()) {
        return None;
    }
    let answer = if item["fields"].is_array() {
        let map: Map<String, Value> = values.into_iter().map(|(k, v)| (k, v.into())).collect();
        json!({ "type": "save", "values": map })
    } else {
        json!({ "type": "save", "secret": values[0].1 })
    };
    Some(json!({ "threadId": item["threadId"], "turnItemId": item["id"], "answer": answer }))
}

/// The answered line, or `None` while the request waits.
fn answered_label(status: &str) -> Option<String> {
    match status {
        "saved" => Some(t("Saved securely and kept private")),
        "declined" => Some(t("Declined")),
        "cancelled" => Some(t("Request ended")),
        _ => None,
    }
}

fn privacy_note(item: &Value) -> String {
    match (item["delivery"] == "file", item["fields"].is_array()) {
        (false, _) => t("Stored securely, never shown to the agent"),
        (true, false) => {
            t("Kept out of chat, in a private file this agent's commands can read for 24 hours")
        }
        (true, true) => {
            t("Kept out of chat, in private files this agent's commands can read for 24 hours")
        }
    }
}

impl T3App {
    pub(crate) fn render_secret_request(
        &self,
        item: &TurnItem,
        c: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = item.str("label").to_owned();
        if let Some(done) = answered_label(item.str("secretStatus")) {
            return div()
                .text_sm()
                .text_color(c.muted_foreground)
                .child(format!("{label} · {done}"))
                .into_any_element();
        }
        let raw = item.raw.clone();
        let decline = json!({ "threadId": raw["threadId"], "turnItemId": raw["id"], "answer": { "type": "decline" } });
        let link = raw["url"]
            .as_str()
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .map(str::to_owned);
        v_flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(c.info)
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
            .child(item.str("reason").to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child(privacy_note(&raw)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("secret-save-{}", item.id)))
                            .small()
                            .primary()
                            .label(t("Save securely"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_secret_dialog(raw.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("secret-decline-{}", item.id)))
                            .small()
                            .ghost()
                            .label(t("Decline"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer_secret(decline.clone(), cx)
                            })),
                    )
                    .children(link.map(|url| {
                        Button::new(SharedString::from(format!("secret-link-{}", item.id)))
                            .small()
                            .ghost()
                            .label(
                                url::Url::parse(&url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(str::to_owned))
                                    .unwrap_or_default()
                                    + " ↗",
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    })),
            )
            .into_any_element()
    }

    fn open_secret_dialog(&mut self, item: Value, window: &mut Window, cx: &mut Context<Self>) {
        let fields: Vec<(String, String, Entity<InputState>)> = inputs(&item)
            .into_iter()
            .map(|(key, label, placeholder, masked)| {
                let state = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(masked)
                });
                (key, label, state)
            })
            .collect();
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, item, fields) = (app.clone(), item.clone(), fields.clone());
            dialog
                .title(item["label"].as_str().unwrap_or("").to_owned())
                .child(
                    v_flex()
                        .gap_2()
                        .children(fields.iter().map(|(_, label, state)| {
                            v_flex()
                                .gap_1()
                                .child(div().text_xs().child(label.clone()))
                                .child(Input::new(state).mask_toggle())
                        })),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Save securely"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let values: Vec<(String, String)> = fields
                        .iter()
                        .map(|(key, _, state)| (key.clone(), state.read(cx).value().to_string()))
                        .collect();
                    let Some(answer) = save_answer(&item, &values) else {
                        return false;
                    };
                    app.update(cx, |this, cx| this.answer_secret(answer, cx))
                        .ok();
                    true
                })
        });
    }

    fn answer_secret(&mut self, input: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("secrets.answerRequest", input).await }).await;
            if !matches!(result, Ok(Ok(_))) {
                // Generic copy: an error could echo the request, and the typed value must not surface.
                this.update(cx, |app, cx| {
                    app.error = Some(t("Could not save the secret.").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{inputs, save_answer};
    use serde_json::json;

    #[test]
    fn answers_single_and_field_requests() {
        let single = json!({"id": "i", "threadId": "t", "label": "API key"});
        assert_eq!(inputs(&single).len(), 1);
        assert!(save_answer(&single, &[("".into(), "  ".into())]).is_none());
        let answer = save_answer(&single, &[("".into(), " sk-1 ".into())]).unwrap();
        assert_eq!(answer["answer"], json!({"type": "save", "secret": "sk-1"}));
        assert_eq!(answer["turnItemId"], "i");

        let multi = json!({"id": "i", "threadId": "t", "label": "DB", "fields": [
            {"name": "user", "label": "User", "secret": false}, {"name": "pass", "label": "Password"}]});
        let fields = inputs(&multi);
        assert_eq!((fields[0].3, fields[1].3), (false, true));
        let values = [("user".into(), "me".into()), ("pass".into(), "pw".into())];
        assert_eq!(
            save_answer(&multi, &values).unwrap()["answer"],
            json!({"type": "save", "values": {"user": "me", "pass": "pw"}})
        );
        assert!(
            save_answer(
                &multi,
                &[("user".into(), "me".into()), ("pass".into(), "".into())]
            )
            .is_none()
        );
    }
}
