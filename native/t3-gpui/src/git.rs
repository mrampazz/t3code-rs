//! Branch toolbar and git actions (web: `BranchToolbar.tsx`, `GitActionsControl.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::{T3App, on_tokio};

/// `subscribeVcsStatus` projection: local status plus remote (upstream, PR) when known.
#[derive(Default, Debug)]
pub struct GitStatus {
    pub local: Value,
    pub remote: Value,
}

impl GitStatus {
    pub fn apply(&mut self, event: &Value) {
        match event["_tag"].as_str() {
            Some("snapshot") => {
                (self.local, self.remote) = (event["local"].clone(), event["remote"].clone())
            }
            Some("localUpdated") => self.local = event["local"].clone(),
            Some("remoteUpdated") => self.remote = event["remote"].clone(),
            _ => {}
        }
    }

    pub fn is_repo(&self) -> bool {
        self.local["isRepo"] == true
    }

    pub fn branch(&self) -> Option<&str> {
        self.local["refName"].as_str()
    }

    /// (files, insertions, deletions) in the working tree.
    pub fn changes(&self) -> (usize, u64, u64) {
        let files = self.local["workingTree"]["files"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let sum = |k: &str| files.iter().filter_map(|f| f[k].as_u64()).sum();
        (files.len(), sum("insertions"), sum("deletions"))
    }

    pub fn ahead_behind(&self) -> (u64, u64) {
        (
            self.remote["aheadCount"].as_u64().unwrap_or(0),
            self.remote["behindCount"].as_u64().unwrap_or(0),
        )
    }

    pub fn pr(&self) -> Option<(u64, String)> {
        let pr = &self.remote["pr"];
        Some((pr["number"].as_u64()?, pr["url"].as_str()?.to_owned()))
    }
}

/// `GitStackedAction` choices in the web's order.
const ACTIONS: [(&str, &str); 5] = [
    ("commit", "Commit"),
    ("commit_push", "Commit & push"),
    ("commit_push_pr", "Commit, push & create PR"),
    ("push", "Push"),
    ("create_pr", "Create PR"),
];

impl T3App {
    /// The open thread's working folder: its worktree, else its project's root.
    pub(crate) fn thread_cwd(&self) -> Option<String> {
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell.threads.iter().find(|t| &t.id == id))?;
        thread.worktree_path.clone().or_else(|| {
            self.shell
                .projects
                .iter()
                .find(|p| p.id == thread.project_id)
                .map(|p| p.workspace_root.clone())
        })
    }

    /// Lives as long as the thread selection; replaced on switch or reconnect.
    pub(crate) fn subscribe_git(&mut self, cx: &mut Context<Self>) {
        if let (Some(client), Some(old)) = (&self.client, self.git_sub.take()) {
            client.cancel(&old);
        }
        self.git = GitStatus::default();
        let (Some(client), Some(cwd)) = (self.client.clone(), self.thread_cwd()) else {
            return;
        };
        let (sub, mut rx) = client.request(
            "subscribeVcsStatus",
            json!({ "cwd": cwd, "includeRemote": true }),
        );
        self.git_sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(Ok(event)) = rx.recv().await {
                let alive = this.update(cx, |app, cx| {
                    if app.git_sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    app.git.apply(&event);
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

    pub(crate) fn run_git_action(&mut self, action: &'static str, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client.clone(), self.thread_cwd()) else {
            return;
        };
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell.threads.iter().find(|t| &t.id == id));
        let mut payload =
            json!({ "actionId": uuid::Uuid::new_v4().to_string(), "cwd": cwd, "action": action });
        if let Some(thread) = thread {
            payload["threadId"] = json!(thread.id);
            payload["projectId"] = json!(thread.project_id);
        }
        self.git_progress = Some("Starting…".into());
        cx.notify();
        let (_, mut rx) = client.request("git.runStackedAction", payload);
        cx.spawn(async move |this, cx| {
            let mut outcome = None;
            while let Some(event) = rx.recv().await {
                let label: Option<String> = match &event {
                    Err(e) => {
                        outcome = Some(format!("Failed: {e}"));
                        None
                    }
                    Ok(e) => match e["kind"].as_str() {
                        Some("phase_started") => e["label"].as_str().map(str::to_owned),
                        Some("hook_started") => {
                            e["hookName"].as_str().map(|h| format!("Running {h}…"))
                        }
                        Some("action_failed") => {
                            outcome =
                                Some(format!("Failed: {}", e["message"].as_str().unwrap_or("")));
                            None
                        }
                        Some("action_finished") => {
                            outcome = Some(git_result_summary(&e["result"]));
                            None
                        }
                        _ => None,
                    },
                };
                if let Some(label) = label {
                    this.update(cx, |app, cx| {
                        app.git_progress = Some(label.into());
                        cx.notify();
                    })
                    .ok();
                }
            }
            this.update(cx, |app, cx| {
                app.git_progress = outcome.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn git_pull(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client.clone(), self.thread_cwd()) else {
            return;
        };
        self.git_progress = Some("Pulling…".into());
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("vcs.pull", json!({ "cwd": cwd })).await }).await;
            this.update(cx, |app, cx| {
                app.git_progress = Some(match result {
                    Ok(Ok(_)) => "Pulled".into(),
                    Ok(Err(e)) => format!("Pull failed: {e}").into(),
                    Err(e) => format!("Pull failed: {e}").into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_git_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        if !self.git.is_repo() {
            return div().into_any_element();
        }
        let (files, insertions, deletions) = self.git.changes();
        let (ahead, behind) = self.git.ahead_behind();
        let app = cx.entity().downgrade();
        h_flex()
            .gap_2()
            .text_xs()
            .text_color(c.muted_foreground)
            .child(format!("⎇ {}", self.git.branch().unwrap_or("detached")))
            .when(files > 0, |row| {
                row.child(format!("{files} changed"))
                    .child(div().text_color(c.success).child(format!("+{insertions}")))
                    .child(div().text_color(c.danger).child(format!("−{deletions}")))
            })
            .when(ahead > 0, |row| row.child(format!("↑{ahead}")))
            .when(behind > 0, |row| {
                row.child(
                    Button::new("git-pull")
                        .xsmall()
                        .ghost()
                        .label(format!("↓{behind} Pull"))
                        .on_click(cx.listener(|this, _, _, cx| this.git_pull(cx))),
                )
            })
            .children(self.git.pr().map(|(number, url)| {
                Button::new("git-pr")
                    .xsmall()
                    .ghost()
                    .label(format!("PR #{number}"))
                    .on_click(move |_, _, cx| cx.open_url(&url))
            }))
            .children(self.git_progress.clone())
            .child(
                Button::new("git-actions")
                    .xsmall()
                    .outline()
                    .label("Git")
                    .dropdown_menu(move |mut menu, _, _| {
                        for (action, label) in ACTIONS {
                            let app = app.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    app.update(cx, |this, cx| this.run_git_action(action, cx))
                                        .ok();
                                }));
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
}

/// "Committed abc1234 · pushed main · PR #12" from a `GitRunStackedActionResult`.
pub(crate) fn git_result_summary(result: &Value) -> String {
    if let Some(title) = result["toast"]["title"].as_str() {
        return title.to_owned();
    }
    let mut parts = Vec::new();
    if let Some(sha) = result["commit"]["commitSha"].as_str() {
        parts.push(format!("Committed {}", &sha[..sha.len().min(7)]));
    }
    if result["push"]["status"] == "pushed" {
        parts.push(format!(
            "pushed {}",
            result["push"]["branch"].as_str().unwrap_or("")
        ));
    }
    if let Some(n) = result["pr"]["number"].as_u64() {
        parts.push(format!("PR #{n}"));
    }
    if parts.is_empty() {
        "Nothing to do".into()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::{GitStatus, git_result_summary};
    use serde_json::json;

    #[test]
    fn tracks_local_and_remote_status() {
        let mut git = GitStatus::default();
        git.apply(&json!({"_tag": "snapshot", "remote": null, "local": {"isRepo": true, "refName": "main",
            "workingTree": {"files": [{"path": "a", "insertions": 3, "deletions": 1}, {"path": "b", "insertions": 2, "deletions": 0}]}}}));
        assert_eq!(
            (git.branch(), git.changes(), git.ahead_behind()),
            (Some("main"), (2, 5, 1), (0, 0))
        );
        git.apply(
            &json!({"_tag": "remoteUpdated", "remote": {"aheadCount": 2, "behindCount": 1,
            "pr": {"number": 7, "url": "https://x/pr/7"}}}),
        );
        assert_eq!(git.ahead_behind(), (2, 1));
        assert_eq!(git.pr(), Some((7, "https://x/pr/7".into())));
        git.apply(&json!({"_tag": "localUpdated", "local": {"isRepo": true, "refName": "feat", "workingTree": {"files": []}}}));
        assert_eq!((git.branch(), git.changes().0), (Some("feat"), 0));
    }

    #[test]
    fn summarizes_action_results() {
        let result = json!({"commit": {"status": "created", "commitSha": "abcdef123456"},
            "push": {"status": "pushed", "branch": "main"}, "pr": {"number": 12}});
        assert_eq!(
            git_result_summary(&result),
            "Committed abcdef1 · pushed main · PR #12"
        );
        assert_eq!(
            git_result_summary(&json!({"commit": {"status": "skipped"}})),
            "Nothing to do"
        );
        assert_eq!(
            git_result_summary(&json!({"toast": {"title": "Pushed to main"}})),
            "Pushed to main"
        );
    }
}
