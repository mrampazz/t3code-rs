//! Linked environments (web: `settings/LinkedEnvironmentsSettings`, contract `peerLink.ts`): other
//! T3 Code servers this one's agents may reach, each with an access level. Listed in Settings
//! (`peerLinks.list` when it opens), linked from a URL and pairing code, and unlinked.
//! shortcut: the access level is fixed at "approval-required" when linking; relink to change it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt, dialog::DialogButtonProps, h_flex, v_flex,
};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// "reachable · approval-required" for a link row.
pub fn link_detail(link: &Value) -> String {
    let mut parts = vec![
        link["status"].as_str().unwrap_or("").to_owned(),
        link["access"].as_str().unwrap_or("").to_owned(),
    ];
    if let Some(e) = link["lastError"].as_str() {
        parts.push(e.to_owned());
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

impl T3App {
    pub(crate) fn load_peer_links(&mut self, cx: &mut Context<Self>) {
        if self.config().settings["linkedEnvironmentsEnabled"] != true {
            self.peer_links.clear();
            return;
        }
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("peerLinks.list", json!({})).await }).await;
            if let Ok(Ok(list)) = result {
                this.update(cx, |app, cx| {
                    app.peer_links = list["links"].as_array().cloned().unwrap_or_default();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn peer_call(&mut self, method: &'static str, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.settings_status = Some(t("Working…").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            this.update(cx, |app, cx| {
                app.settings_status = match result {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e.to_string().into()),
                    Err(e) => Some(e.to_string().into()),
                };
                app.load_peer_links(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_link_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://other-machine.tail1234.ts.net")
        });
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(t("Pairing code")));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, url, code) = (app.clone(), url.clone(), code.clone());
            dialog
                .title(t("Link environment"))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Input::new(&url))
                        .child(Input::new(&code))
                        .child(t("Run `t3 pair` on the other machine, or copy a code from its Connections settings.")),
                )
                .button_props(DialogButtonProps::default().ok_text(t("Link")).show_cancel(true))
                .on_ok(move |_, _, cx: &mut App| {
                    let (url, code) = (url.read(cx).value().trim().to_string(), code.read(cx).value().trim().to_string());
                    if url.is_empty() || code.is_empty() {
                        return false;
                    }
                    let input = json!({ "url": url, "pairingCode": code, "access": "approval-required" });
                    app.update(cx, |this, cx| this.peer_call("peerLinks.link", input, cx)).ok();
                    true
                })
        });
    }

    pub(crate) fn render_peer_links(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.config().settings["linkedEnvironmentsEnabled"] != true {
            return div().into_any_element();
        }
        let c = cx.theme().colors;
        v_flex()
            .text_sm()
            .children(self.peer_links.iter().enumerate().map(|(i, link)| {
                let env = link["environmentId"].clone();
                h_flex()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(link["label"].as_str().unwrap_or("").to_owned())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .truncate()
                                    .child(link_detail(link)),
                            ),
                    )
                    .child(
                        Button::new(("peer-unlink", i))
                            .small()
                            .ghost()
                            .label(t("Unlink"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.peer_call(
                                    "peerLinks.unlink",
                                    json!({ "environmentId": env }),
                                    cx,
                                )
                            })),
                    )
            }))
            .child(
                div().py_2().child(
                    Button::new("peer-link")
                        .small()
                        .outline()
                        .label(t("Link environment"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_link_environment(window, cx)
                        })),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::link_detail;
    use serde_json::json;

    #[test]
    fn describes_links() {
        assert_eq!(
            link_detail(&json!({"status": "reachable", "access": "auto", "lastError": null})),
            "reachable · auto"
        );
        assert_eq!(
            link_detail(
                &json!({"status": "unreachable", "access": "read-only", "lastError": "timeout"})
            ),
            "unreachable · read-only · timeout"
        );
    }
}
