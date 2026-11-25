//! A thread's pull requests (web: `ThreadDetailsPrRows.tsx`, `pullRequestLinkInput.ts`):
//! chips with state and checks, open, watch, unlink, and linking one by URL or number.
//! shortcut: URL parsing covers GitHub, GitLab and Bitbucket shapes; the web asks each source
//! control client (`sourceControlClients.parseChangeRequestUrl`). Add hosts as they come up.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt, dialog::DialogButtonProps, h_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::T3App;
use crate::i18n::t;

/// (host, repository, number) of a pull request URL.
pub fn parse_pr_url(input: &str) -> Option<(String, String, u64)> {
    let url = url::Url::parse(input.trim()).ok()?;
    let host = url.host_str()?.to_owned();
    let parts: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let find = |marker: &[&str]| parts.windows(marker.len()).position(|w| w == marker);
    let (repo_end, number_at) = if let Some(i) = find(&["-", "merge_requests"]) {
        (i, i + 2)
    } else {
        let i = parts
            .iter()
            .position(|p| *p == "pull" || *p == "pull-requests")?;
        (i, i + 1)
    };
    let number = parts.get(number_at)?.parse().ok().filter(|n| *n > 0)?;
    (repo_end >= 2).then(|| (host, parts[..repo_end].join("/"), number))
}

/// A bare `#123` against the project's GitHub repository.
pub fn resolve_number(input: &str, identity: &Value) -> Option<(String, String, u64, String)> {
    let number: u64 = input
        .trim()
        .trim_start_matches('#')
        .parse()
        .ok()
        .filter(|n| *n > 0)?;
    if identity["provider"] != "github" {
        return None;
    }
    let repository = match (identity["owner"].as_str(), identity["name"].as_str()) {
        (Some(owner), Some(name)) => format!("{owner}/{name}"),
        _ => identity["displayName"].as_str()?.to_owned(),
    };
    let web = identity["webUrl"]
        .as_str()
        .map_or_else(|| format!("https://github.com/{repository}"), str::to_owned);
    Some((
        "github.com".into(),
        repository,
        number,
        format!("{}/pull/{number}", web.trim_end_matches('/')),
    ))
}

/// "#12 · open · checks failing" for a chip.
pub fn chip_label(link: &Value) -> String {
    let snap = &link["snapshot"];
    let mut parts = vec![format!("#{}", link["number"])];
    match (snap["state"].as_str(), snap["isDraft"] == true) {
        (Some("open"), true) => parts.push(t("Draft")),
        (Some(state), _) => parts.push(t(&capitalize(state))),
        _ => {}
    }
    if let Some(checks) = snap["checksState"].as_str() {
        parts.push(format!("{} {}", t("Checks"), t(checks)));
    }
    if snap["reviewDecision"] == "changes-requested" {
        parts.push(t("Changes requested"));
    } else if snap["reviewDecision"] == "approved" {
        parts.push(t("Approved"));
    }
    parts.join(" · ")
}

fn capitalize(s: &str) -> String {
    s.chars()
        .take(1)
        .flat_map(char::to_uppercase)
        .chain(s.chars().skip(1))
        .collect()
}

impl T3App {
    pub(crate) fn render_pull_requests(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id));
        let links: Vec<Value> = thread
            .map(|t| {
                t.pull_requests
                    .iter()
                    .filter(|l| l["source"] != "stack-dismissed")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let thread_id = thread.map(|t| t.id.clone()).unwrap_or_default();
        let app = cx.entity().downgrade();
        h_flex()
            .gap_1()
            .children(links.into_iter().enumerate().map(|(i, link)| {
                let color = match (link["snapshot"]["state"].as_str(), link["snapshot"]["checksState"].as_str()) {
                    (Some("merged"), _) => c.magenta,
                    (Some("closed"), _) => c.muted_foreground,
                    (_, Some("failing")) => c.danger,
                    (_, Some("pending")) => c.warning,
                    _ => c.success,
                };
                let (app, thread_id) = (app.clone(), thread_id.clone());
                let watching = link["watch"].is_object();
                Button::new(("pr-chip", i))
                    .xsmall()
                    .ghost()
                    .label(chip_label(&link))
                    .text_color(color)
                    .dropdown_menu(move |menu, _, _| {
                        let key = json!({ "threadId": thread_id, "host": link["host"], "repository": link["repository"], "number": link["number"] });
                        let command = |kind: &str, extra: Value| {
                            let mut c = key.clone();
                            c["type"] = json!(kind);
                            c.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
                            c
                        };
                        let item = |label: String, command: Value| {
                            let app = app.clone();
                            PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                app.update(cx, |this, cx| this.dispatch(command.clone(), cx)).ok();
                            })
                        };
                        let url = link["url"].as_str().unwrap_or("").to_owned();
                        let menu = menu.item(PopupMenuItem::new(t("Open in browser")).on_click(move |_, _, cx| cx.open_url(&url)));
                        let menu = if watching {
                            menu.item(item(t("Stop watching"), command("thread.pull-request.watch", json!({ "watching": false }))))
                        } else {
                            menu.item(item(t("Watch until merged"), command("thread.pull-request.watch", json!({ "watching": true, "until": "merged" }))))
                                .item(item(t("Watch checks"), command("thread.pull-request.watch", json!({ "watching": true, "until": "checks" }))))
                        };
                        menu.separator().item(item(t("Unlink"), command("thread.pull-request.unlink", json!({}))))
                    })
            }))
            .child(
                Button::new("link-pr")
                    .xsmall()
                    .ghost()
                    .label(t("Link PR…"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_link_pr(window, cx))),
            )
    }

    pub(crate) fn open_link_pr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .cloned()
        else {
            return;
        };
        let identity = self
            .shell()
            .projects
            .iter()
            .find(|p| p.id == thread.project_id)
            .map(|p| p.repository_identity.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://github.com/owner/repo/pull/123  or  #123")
        });
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, identity, thread_id) = (app.clone(), input.clone(), identity.clone(), thread.id.clone());
            dialog
                .title(t("Link a pull request"))
                .child(Input::new(&input))
                .button_props(DialogButtonProps::default().ok_text(t("Link")).show_cancel(true))
                .on_ok(move |_, _, cx: &mut App| {
                    let text = input.read(cx).value().trim().to_string();
                    let resolved = parse_pr_url(&text)
                        .map(|(h, r, n)| (h, r, n, text.clone()))
                        .or_else(|| resolve_number(&text, &identity));
                    let Some((host, repository, number, url)) = resolved else { return false };
                    let command = json!({ "type": "thread.pull-request.link", "threadId": thread_id, "host": host,
                        "repository": repository, "number": number, "url": url, "source": "manual" });
                    app.update(cx, |this, cx| this.dispatch(command, cx)).ok();
                    true
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{chip_label, parse_pr_url, resolve_number};
    use serde_json::json;

    #[test]
    fn parses_pull_request_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/o/r/pull/12"),
            Some(("github.com".into(), "o/r".into(), 12))
        );
        assert_eq!(
            parse_pr_url("https://gitlab.com/g/sub/r/-/merge_requests/7/diffs"),
            Some(("gitlab.com".into(), "g/sub/r".into(), 7))
        );
        assert_eq!(
            parse_pr_url("https://bitbucket.org/ws/r/pull-requests/3"),
            Some(("bitbucket.org".into(), "ws/r".into(), 3))
        );
        assert_eq!(parse_pr_url("https://github.com/o/r/issues/12"), None);
        assert_eq!(parse_pr_url("not a url"), None);
    }

    #[test]
    fn resolves_bare_numbers_against_github_projects() {
        let identity = json!({"provider": "github", "owner": "o", "name": "r", "webUrl": "https://github.com/o/r"});
        assert_eq!(
            resolve_number("#5", &identity),
            Some((
                "github.com".into(),
                "o/r".into(),
                5,
                "https://github.com/o/r/pull/5".into()
            ))
        );
        assert_eq!(resolve_number("5", &json!({"provider": "gitlab"})), None);
    }

    #[test]
    fn labels_chips() {
        let link = json!({"number": 9, "snapshot": {"state": "open", "isDraft": false, "checksState": "failing", "reviewDecision": "approved"}});
        assert_eq!(chip_label(&link), "#9 · Open · Checks failing · Approved");
        assert_eq!(chip_label(&json!({"number": 1, "snapshot": null})), "#1");
    }
}
