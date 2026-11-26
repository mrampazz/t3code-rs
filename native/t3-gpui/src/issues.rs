//! A thread's linked issues (web: `ThreadDetailsIssueRows`, `issue/`): chips that open or unlink
//! an issue, and "Link issue…" choosing from the project's open issues (`issues.list`).
//! shortcut: no issue detail panel, comments, creation or tracker setup; open the issue in the
//! browser for those.

use crate::header::{details_label, details_row};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Sizable, WindowExt, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// The `thread.issue.link` command for an `IssueListEntry`.
pub fn link_command(thread_id: &str, entry: &Value) -> Value {
    json!({ "type": "thread.issue.link", "threadId": thread_id, "tracker": entry["tracker"], "host": entry["host"],
        "scope": entry["scope"], "key": entry["key"], "projectId": entry["projectId"], "url": entry["url"],
        "displayKey": entry["displayKey"], "title": entry["title"], "source": "manual" })
}

impl T3App {
    /// The thread's linked issues as rows (web: `ThreadDetailsIssueRows`); each opens a menu.
    pub(crate) fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id));
        let links = thread.map(|t| t.issues.clone()).unwrap_or_default();
        let thread_id = thread.map(|t| t.id.clone()).unwrap_or_default();
        let app = cx.entity().downgrade();
        v_flex().children(links.into_iter().enumerate().map(|(i, link)| {
            let (app, thread_id) = (app.clone(), thread_id.clone());
            let key = link["displayKey"].as_str().unwrap_or("Issue").to_owned();
            let label = match link["title"].as_str() {
                Some(title) => format!("{key} {title}"),
                None => key,
            };
            details_row(("issue-row", i), "circle-dot", details_label(label), cx)
                .dropdown_menu(move |menu, _, _| {
                    let url = link["url"].as_str().unwrap_or("").to_owned();
                    let unlink = json!({ "type": "thread.issue.unlink", "threadId": thread_id, "tracker": link["tracker"],
                        "host": link["host"], "scope": link["scope"], "key": link["key"] });
                    let app = app.clone();
                    menu.item(PopupMenuItem::new(t("Open in browser")).on_click(move |_, _, cx| cx.open_url(&url)))
                        .item(PopupMenuItem::new(t("Unlink")).on_click(move |_, _, cx| {
                            app.update(cx, |this, cx| this.dispatch(unlink.clone(), cx)).ok();
                        }))
                })
        }))
    }

    pub(crate) fn open_link_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .cloned()
        else {
            return;
        };
        let Some(client) = self.client() else { return };
        let window_handle = window.window_handle();
        let payload = json!({ "state": "open", "projectId": thread.project_id, "limit": 50 });
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call("issues.list", payload).await }).await;
            let entries = match result {
                Ok(Ok(list)) => list["entries"].as_array().cloned().unwrap_or_default(),
                Ok(Err(e)) => {
                    this.update(cx, |app, cx| {
                        app.error = Some(format!("Could not load issues: {e}").into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            let app = this.clone();
            window_handle
                .update(cx, |_, window, cx| {
                    window.open_dialog(cx, move |dialog, _, _| {
                        let (app, thread_id) = (app.clone(), thread.id.clone());
                        let rows = v_flex()
                            .id("issue-list")
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .children(entries.iter().enumerate().map(|(i, entry)| {
                                let command = link_command(&thread_id, entry);
                                let app = app.clone();
                                Button::new(("issue-pick", i))
                                    .small()
                                    .ghost()
                                    .label(format!(
                                        "{}  {}",
                                        entry["displayKey"].as_str().unwrap_or(""),
                                        entry["title"].as_str().unwrap_or("")
                                    ))
                                    .on_click(move |_, window, cx| {
                                        app.update(cx, |this, cx| {
                                            this.dispatch(command.clone(), cx)
                                        })
                                        .ok();
                                        window.close_dialog(cx);
                                    })
                            }));
                        dialog.title(t("Link issue…")).child(if entries.is_empty() {
                            div().child(t("No open issues")).into_any_element()
                        } else {
                            rows.into_any_element()
                        })
                    });
                })
                .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::link_command;
    use serde_json::json;

    #[test]
    fn links_a_listed_issue() {
        let entry = json!({"tracker": "github", "host": "github.com", "scope": "o/r", "key": "12", "projectId": "p",
            "url": "https://github.com/o/r/issues/12", "displayKey": "#12", "title": "Bug", "state": "open"});
        let command = link_command("t", &entry);
        assert_eq!(command["type"], "thread.issue.link");
        assert_eq!(command["displayKey"], "#12");
        assert_eq!(command["source"], "manual");
        assert!(command.get("state").is_none());
    }
}
