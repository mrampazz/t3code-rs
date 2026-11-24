//! Settings → Sharing & access (web: `AuthorizedClientsSettings.tsx`): pairing links for other
//! devices, connected clients, and revoking either.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, auth, on_tokio};

/// `subscribeAuthAccess` projection.
#[derive(Default)]
pub struct Access {
    pub links: Vec<Value>,
    pub clients: Vec<Value>,
}

impl Access {
    pub fn apply(&mut self, event: &Value) {
        let p = &event["payload"];
        let upsert = |list: &mut Vec<Value>, key: &str| {
            list.retain(|x| x[key] != p[key]);
            list.push(p.clone());
        };
        match event["type"].as_str() {
            Some("snapshot") => {
                self.links = p["pairingLinks"].as_array().cloned().unwrap_or_default();
                self.clients = p["clientSessions"].as_array().cloned().unwrap_or_default();
            }
            Some("pairingLinkUpserted") => upsert(&mut self.links, "id"),
            Some("pairingLinkRemoved") => self.links.retain(|x| x["id"] != p["id"]),
            Some("clientUpserted") => upsert(&mut self.clients, "sessionId"),
            Some("clientRemoved") => self.clients.retain(|x| x["sessionId"] != p["sessionId"]),
            _ => {}
        }
    }
}

/// `buildPairingUrl`: the credential in the fragment so it never reaches server logs.
pub fn pairing_url(base_url: &str, credential: &str) -> String {
    let token: String = url::form_urlencoded::byte_serialize(credential.as_bytes()).collect();
    format!("{}/pair#token={token}", base_url.trim_end_matches('/'))
}

impl T3App {
    /// Follows the active environment's links and clients while Settings is open.
    pub(crate) fn subscribe_access(&mut self, cx: &mut Context<Self>) {
        if let (Some(client), Some(old)) = (self.client(), self.access_sub.take()) {
            client.cancel(&old);
        }
        self.access = Access::default();
        let Some(client) = self.client() else { return };
        let (sub, mut rx) = client.request("subscribeAuthAccess", json!({}));
        self.access_sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(Ok(event)) = rx.recv().await {
                let alive = this.update(cx, |app, cx| {
                    if app.access_sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    app.access.apply(&event);
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

    pub(crate) fn release_access(&mut self) {
        if let (Some(client), Some(old)) = (self.client(), self.access_sub.take()) {
            client.cancel(&old);
        }
    }

    fn access_call(&mut self, path: &'static str, body: Value, cx: &mut Context<Self>) {
        let Some(session) = self.env().map(|e| e.session.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { auth::post_json(&session, path, &body).await }).await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(Ok(value)) if path == "/api/auth/pairing-token" => {
                        if let (Some(credential), Some(env)) =
                            (value["credential"].as_str(), app.env())
                        {
                            app.new_pairing_url = Some(pairing_url(&env.key, credential).into());
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => app.settings_status = Some(e.to_string().into()),
                    Err(e) => app.settings_status = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_access(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let row = || {
            h_flex()
                .py_2()
                .gap_3()
                .border_b_1()
                .border_color(c.border)
                .text_sm()
        };
        let mut col = v_flex().child(
            row()
                .child(
                    v_flex().flex_1().child(t("Pair another device")).child(
                        div()
                            .text_xs()
                            .text_color(c.muted_foreground)
                            .child(t("Open the link on the other device; it works once.")),
                    ),
                )
                .child(
                    Button::new("create-pairing")
                        .small()
                        .outline()
                        .label(t("Create link"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let label = format!("Link from {}", hostname());
                            this.access_call(
                                "/api/auth/pairing-token",
                                json!({ "label": label }),
                                cx,
                            );
                        })),
                ),
        );
        if let Some(url) = self.new_pairing_url.clone() {
            let copy = url.to_string();
            col = col.child(
                row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(url),
                    )
                    .child(
                        Button::new("copy-pairing")
                            .small()
                            .ghost()
                            .label(t("Copy"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            }),
                    ),
            );
        }
        for (i, link) in self.access.links.iter().enumerate() {
            let id = link["id"].as_str().unwrap_or("").to_owned();
            col =
                col.child(
                    row()
                        .child(
                            v_flex()
                                .flex_1()
                                .child(link["label"].as_str().unwrap_or("Pairing link").to_owned())
                                .child(div().text_xs().text_color(c.muted_foreground).child(
                                    format!(
                                        "{} {}",
                                        t("Expires"),
                                        link["expiresAt"].as_str().unwrap_or("")
                                    ),
                                )),
                        )
                        .child(
                            Button::new(("revoke-link", i))
                                .small()
                                .ghost()
                                .label(t("Revoke"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.access_call(
                                        "/api/auth/pairing-links/revoke",
                                        json!({ "id": id }),
                                        cx,
                                    )
                                })),
                        ),
                );
        }
        for (i, client) in self.access.clients.iter().enumerate() {
            let id = client["sessionId"].as_str().unwrap_or("").to_owned();
            let meta = &client["client"];
            let name = meta["label"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    [
                        meta["deviceType"].as_str(),
                        meta["os"].as_str(),
                        meta["browser"].as_str(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
                });
            let current = client["current"] == true;
            col = col.child(
                row()
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .when(client["connected"] == true, |d| d.bg(c.success)),
                    )
                    .child(
                        v_flex().flex_1().child(name).child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .child(client["lastConnectedAt"].as_str().unwrap_or("").to_owned()),
                        ),
                    )
                    .when(current, |r| {
                        r.child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .child(t("This device")),
                        )
                    })
                    .when(!current, |r| {
                        r.child(
                            Button::new(("revoke-client", i))
                                .small()
                                .ghost()
                                .label(t("Revoke"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.access_call(
                                        "/api/auth/clients/revoke",
                                        json!({ "sessionId": id }),
                                        cx,
                                    )
                                })),
                        )
                    }),
            );
        }
        col
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "desktop".into())
}

#[cfg(test)]
mod tests {
    use super::{Access, pairing_url};
    use serde_json::json;

    #[test]
    fn tracks_links_and_clients() {
        let mut access = Access::default();
        access.apply(&json!({"type": "snapshot", "payload": {"pairingLinks": [{"id": "l1"}], "clientSessions": [{"sessionId": "s1"}, {"sessionId": "s2"}]}}));
        access.apply(&json!({"type": "pairingLinkUpserted", "payload": {"id": "l2"}}));
        access.apply(&json!({"type": "pairingLinkRemoved", "payload": {"id": "l1"}}));
        access.apply(&json!({"type": "clientRemoved", "payload": {"sessionId": "s1"}}));
        access.apply(
            &json!({"type": "clientUpserted", "payload": {"sessionId": "s2", "connected": true}}),
        );
        assert_eq!(access.links, [json!({"id": "l2"})]);
        assert_eq!(
            access.clients,
            [json!({"sessionId": "s2", "connected": true})]
        );
        assert_eq!(
            pairing_url("http://h:1/", "a+b"),
            "http://h:1/pair#token=a%2Bb"
        );
    }
}
