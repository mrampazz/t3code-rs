//! Provider status banner over the composer (web: `ProviderStatusBanner.tsx`): the selected
//! model's provider is unauthenticated, unavailable, limited or a known-bad version. Refresh
//! re-probes it; a dismissal lasts the session for that exact status.
//! shortcut: no "Open provider setup" (provider setup is not ported yet).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::json;

use crate::i18n::{t, tv};
use crate::model::Provider;
use crate::{T3App, on_tokio};

fn name(p: &Provider) -> String {
    p.display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| p.driver.clone(), str::to_owned)
}

/// The compatibility advisory worth a banner: broken, or unsupported on a ready provider.
fn incompatible(p: &Provider) -> Option<&str> {
    if p.status == "error" && p.auth["status"] == "unauthenticated" {
        return None;
    }
    match p.compatibility_advisory["status"].as_str() {
        Some("broken") => Some("broken"),
        Some("unsupported") if p.status == "ready" => Some("unsupported"),
        _ => None,
    }
}

/// `getProviderStatusBannerKey`: `None` when there is nothing to report.
pub fn banner_key(p: &Provider) -> Option<String> {
    if p.status == "disabled" {
        return None;
    }
    if let Some(kind) = incompatible(p) {
        let message = p.compatibility_advisory["message"].as_str().unwrap_or("");
        return Some(
            [
                p.instance_id.as_str(),
                kind,
                p.version.as_deref().unwrap_or(""),
                message,
            ]
            .join("\0"),
        );
    }
    if p.status == "ready"
        || (p.driver == "antigravity"
            && p.installed
            && p.status == "warning"
            && p.auth["status"] == "unknown")
    {
        return None;
    }
    let auth = p.auth["status"].as_str().unwrap_or("");
    Some(
        [
            p.instance_id.as_str(),
            &p.status,
            auth,
            p.message.as_deref().unwrap_or(""),
        ]
        .join("\0"),
    )
}

/// (title, message, is_warning) as the web words them.
pub fn banner_text(p: &Provider) -> (String, String, bool) {
    let provider = name(p);
    let version = p.version.clone().unwrap_or_default();
    let unauthenticated = p.status == "error" && p.auth["status"] == "unauthenticated";
    let bad = incompatible(p);
    let title = if unauthenticated {
        tv("{provider} is unauthenticated", &[("provider", &provider)])
    } else if bad == Some("broken") {
        tv(
            "{provider} {version} is known to be broken",
            &[("provider", &provider), ("version", &version)],
        )
    } else if bad.is_some() {
        tv(
            "{provider} {version} is unsupported",
            &[("provider", &provider), ("version", &version)],
        )
    } else {
        tv("{provider} provider status", &[("provider", &provider)])
    };
    let advisory = p.compatibility_advisory["message"]
        .as_str()
        .filter(|_| bad.is_some());
    let message = advisory
        .map(str::to_owned)
        .or_else(|| p.message.clone())
        .unwrap_or_else(|| {
            if p.auth["status"] == "unauthenticated" {
                t("Sign in via the CLI to authenticate again.")
            } else if p.status == "ready" {
                t("No models are available for this provider.")
            } else if p.status == "error" {
                tv(
                    "{provider} provider is unavailable.",
                    &[("provider", &provider)],
                )
            } else {
                tv(
                    "{provider} provider has limited availability.",
                    &[("provider", &provider)],
                )
            }
        });
    let warning = bad != Some("broken") && (p.status == "warning" || bad.is_some());
    (title, message, warning)
}

impl T3App {
    pub(crate) fn render_provider_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selection = self.current_model()?;
        let provider = self
            .config()
            .providers
            .iter()
            .find(|p| selection["instanceId"] == p.instance_id.as_str())?;
        let env = self.env()?.key.clone();
        let key = format!("{env}\0{}", banner_key(provider)?);
        if self.dismissed_banners.contains(&key) {
            return None;
        }
        let (title, message, warning) = banner_text(provider);
        let c = cx.theme().colors;
        let instance = provider.instance_id.clone();
        Some(
            h_flex()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(if warning { c.warning } else { c.danger })
                .text_sm()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(div().text_color(c.muted_foreground).child(message)),
                )
                .child(
                    Button::new("provider-refresh")
                        .xsmall()
                        .outline()
                        .label(t("Refresh"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.refresh_provider(instance.clone(), cx)
                        })),
                )
                .child(
                    Button::new("provider-dismiss")
                        .xsmall()
                        .ghost()
                        .label("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dismissed_banners.insert(key.clone());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn refresh_provider(&mut self, instance_id: String, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let payload = json!({ "instanceId": instance_id, "refreshModels": true });
        // The new status arrives on the config stream.
        on_tokio(async move { client.call("server.refreshProviders", payload).await });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{banner_key, banner_text};
    use crate::model::Provider;
    use serde_json::json;

    fn provider(v: serde_json::Value) -> Provider {
        let mut p = json!({"instanceId": "codex", "driver": "codex", "status": "ready", "installed": true, "auth": {"status": "authenticated"}});
        p.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
        serde_json::from_value(p).unwrap()
    }

    #[test]
    fn reports_only_unhealthy_providers() {
        assert_eq!(banner_key(&provider(json!({}))), None);
        assert_eq!(banner_key(&provider(json!({"status": "disabled"}))), None);
        let unauth = provider(
            json!({"status": "error", "auth": {"status": "unauthenticated"}, "displayName": "Codex"}),
        );
        assert!(banner_key(&unauth).is_some());
        let (title, message, warning) = banner_text(&unauth);
        assert_eq!(title, "Codex is unauthenticated");
        assert_eq!(message, "Sign in via the CLI to authenticate again.");
        assert!(!warning);
        let broken = provider(
            json!({"version": "1.2", "compatibilityAdvisory": {"status": "broken", "message": "Upgrade"}}),
        );
        assert_eq!(
            banner_text(&broken),
            (
                "codex 1.2 is known to be broken".into(),
                "Upgrade".into(),
                false
            )
        );
        let limited = provider(json!({"status": "warning"}));
        assert_eq!(
            banner_text(&limited).1,
            "codex provider has limited availability."
        );
        assert!(banner_text(&limited).2);
    }
}
