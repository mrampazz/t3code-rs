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

use crate::i18n::t;
use crate::{T3App, on_tokio};

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

fn check_mark(status: &str) -> &'static str {
    match status {
        "success" => "✓",
        "failure" | "cancelled" => "✕",
        "pending" => "…",
        "action-required" => "!",
        _ => "·",
    }
}

fn capitalize(s: &str) -> String {
    s.chars()
        .take(1)
        .flat_map(char::to_uppercase)
        .chain(s.chars().skip(1))
        .collect()
}

impl T3App {
    /// The selected thread's pull requests, leaving out stack entries the user dismissed.
    pub(crate) fn thread_pull_requests(&self) -> Vec<Value> {
        self.selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .map(|t| {
                t.pull_requests
                    .iter()
                    .filter(|l| l["source"] != "stack-dismissed")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn render_pull_requests(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id));
        let links = self.thread_pull_requests();
        let thread_id = thread.map(|t| t.id.clone()).unwrap_or_default();
        let project_id = thread.map(|t| t.project_id.clone()).unwrap_or_default();
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
                let (app, thread_id, project_id) = (app.clone(), thread_id.clone(), project_id.clone());
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
                        let pr_ref = json!({ "projectId": project_id, "host": link["host"], "repository": link["repository"], "number": link["number"] });
                        let checks = {
                            let (app, pr_ref) = (app.clone(), pr_ref.clone());
                            PopupMenuItem::new(t("Checks…")).on_click(move |_, window, cx| {
                                app.update(cx, |this, cx| this.show_pr_checks(pr_ref.clone(), window, cx)).ok();
                            })
                        };
                        let action = |label: String, action: &'static str, confirm: bool| {
                            let (app, pr_ref) = (app.clone(), pr_ref.clone());
                            PopupMenuItem::new(label.clone()).on_click(move |_, window, cx| {
                                app.update(cx, |this, cx| this.pr_action(pr_ref.clone(), action, label.clone(), confirm, window, cx)).ok();
                            })
                        };
                        let state = link["snapshot"]["state"].as_str().unwrap_or("");
                        let draft = link["snapshot"]["isDraft"] == true;
                        let menu = menu.item(checks).separator();
                        let menu = match state {
                            "open" if draft => menu.item(action(t("Mark ready for review"), "ready", false)).item(action(t("Close"), "close", true)),
                            "open" => menu.item(action(t("Merge"), "merge", true)).item(action(t("Close"), "close", true)),
                            "closed" => menu.item(action(t("Reopen"), "reopen", false)),
                            _ => menu,
                        };
                        let menu = menu.separator();
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

    /// `pullRequests.checks` in a dialog; each check opens its page.
    fn show_pr_checks(&mut self, pr_ref: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("pullRequests.checks", pr_ref).await }).await;
            let checks = match result {
                Ok(Ok(v)) => v["checks"].as_array().cloned().unwrap_or_default(),
                Ok(Err(e)) => {
                    this.update(cx, |app, cx| {
                        app.error = Some(format!("Could not load checks: {e}").into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            window_handle
                .update(cx, |_, window, cx| {
                    window.open_dialog(cx, move |dialog, _, _| {
                        let rows = checks
                            .iter()
                            .filter(|c| c["placeholder"] != true)
                            .enumerate()
                            .map(|(i, check)| {
                                let url = check["url"].as_str().map(str::to_owned);
                                Button::new(("pr-check", i))
                                    .small()
                                    .ghost()
                                    .label(format!(
                                        "{} {}{}",
                                        check_mark(check["status"].as_str().unwrap_or("")),
                                        check["name"].as_str().unwrap_or(""),
                                        check["description"]
                                            .as_str()
                                            .map(|d| format!(" — {d}"))
                                            .unwrap_or_default()
                                    ))
                                    .on_click(move |_, _, cx| {
                                        if let Some(url) = &url {
                                            cx.open_url(url);
                                        }
                                    })
                            });
                        dialog.title(t("Checks")).child(
                            gpui_kit::component::v_flex()
                                .id("pr-checks")
                                .max_h(px(360.))
                                .overflow_y_scroll()
                                .children(rows),
                        )
                    });
                })
                .ok();
        })
        .detach();
    }

    fn pr_action(
        &mut self,
        pr_ref: Value,
        action: &'static str,
        label: String,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut input = pr_ref;
        input["action"] = json!(action);
        if !confirm {
            return self.run_pr_action(input, cx);
        }
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input) = (app.clone(), input.clone());
            dialog
                .title(format!("{label}?"))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(label.clone())
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    app.update(cx, |this, cx| this.run_pr_action(input.clone(), cx))
                        .ok();
                    true
                })
        });
    }

    fn run_pr_action(&mut self, input: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("pullRequests.runAction", input).await }).await;
            if let Some(e) = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            } {
                this.update(cx, |app, cx| {
                    app.error = Some(format!("Pull request action failed: {e}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
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
