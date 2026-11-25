//! Source control in Settings (web: `settings/SourceControlSettings`): which version control tools
//! the host has and who it is signed in as on each code host (`server.discoverSourceControl`).
//! shortcut: read-only; sign-in and account switching happen on the host (`gh auth login`, …).

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// An Effect `Option` as JSON: `{"_tag":"Some","value":…}`, or a plain value / null.
fn opt(v: &Value) -> Option<&str> {
    match v["_tag"].as_str() {
        Some("Some") => v["value"].as_str(),
        Some(_) => None,
        None => v.as_str(),
    }
}

/// (label, status line) per tool and code host.
pub fn summary(found: &Value) -> Vec<(String, String)> {
    let items = |key: &str| found[key].as_array().cloned().unwrap_or_default();
    let line = |item: &Value| {
        let label = item["label"].as_str().unwrap_or("").to_owned();
        if item["status"] != "available" {
            return (
                label,
                format!(
                    "{} · {}",
                    t("Not installed"),
                    item["installHint"].as_str().unwrap_or("")
                ),
            );
        }
        let version = opt(&item["version"]).map(str::to_owned).unwrap_or_default();
        let auth = &item["auth"];
        let who = match auth["status"].as_str() {
            Some("authenticated") => opt(&auth["account"])
                .map_or_else(|| t("Signed in"), |a| format!("{} {a}", t("Signed in as"))),
            Some("unauthenticated") => t("Not signed in"),
            _ => String::new(),
        };
        (
            label,
            [version, who]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
        )
    };
    items("versionControlSystems")
        .iter()
        .chain(items("sourceControlProviders").iter())
        .map(line)
        .collect()
}

impl T3App {
    pub(crate) fn discover_source_control(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { client.call("server.discoverSourceControl", json!({})).await },
                )
                .await;
            if let Ok(Ok(found)) = result {
                this.update(cx, |app, cx| {
                    app.source_control = summary(&found);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn render_source_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        v_flex()
            .text_sm()
            .children(self.source_control.iter().map(|(label, status)| {
                h_flex()
                    .py_1()
                    .gap_2()
                    .child(div().w(px(160.)).child(label.clone()))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(c.muted_foreground)
                            .child(status.clone()),
                    )
            }))
            .child(
                div().py_1().child(
                    Button::new("sc-refresh")
                        .xsmall()
                        .label(t("Refresh"))
                        .on_click(cx.listener(|this, _, _, cx| this.discover_source_control(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::summary;
    use serde_json::json;

    #[test]
    fn summarizes_tools_and_hosts() {
        let found = json!({
            "versionControlSystems": [{"label": "Git", "status": "available", "version": {"_tag": "Some", "value": "2.45"}, "installHint": "x"}],
            "sourceControlProviders": [
                {"label": "GitHub", "status": "available", "version": "2.6", "installHint": "x",
                 "auth": {"status": "authenticated", "account": {"_tag": "Some", "value": "octo"}}},
                {"label": "GitLab", "status": "missing", "version": {"_tag": "None"}, "installHint": "brew install glab", "auth": {"status": "unknown"}}]});
        assert_eq!(
            summary(&found),
            [
                ("Git".to_owned(), "2.45".to_owned()),
                ("GitHub".to_owned(), "2.6 · Signed in as octo".to_owned()),
                (
                    "GitLab".to_owned(),
                    "Not installed · brew install glab".to_owned()
                ),
            ]
        );
    }
}
