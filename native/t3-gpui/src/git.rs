//! Branch toolbar and git actions (web: `BranchToolbar.tsx`, `GitActionsControl.tsx`).

use crate::header::{details_label, details_row, details_split};
use crate::i18n::{t, tv};
use crate::ui;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
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

    /// (insertions, deletions, base ref) for the Changes row (web: `resolveChangesTotals` from
    /// status alone): the branch's changes against its base when it has any, else the working tree.
    pub fn changes_totals(&self) -> ChangesTotals {
        let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
        let (branch, tree) = (&self.local["branchChanges"], &self.local["workingTree"]);
        let pick_branch = branch.is_object()
            && (n(branch, "insertions") > 0
                || n(branch, "deletions") > 0
                || (n(tree, "insertions") == 0 && n(tree, "deletions") == 0));
        if pick_branch {
            let base = branch["baseRef"].as_str().map(str::to_owned);
            (n(branch, "insertions"), n(branch, "deletions"), base)
        } else {
            (n(tree, "insertions"), n(tree, "deletions"), None)
        }
    }

    pub fn ahead_behind(&self) -> (u64, u64) {
        (
            self.remote["aheadCount"].as_u64().unwrap_or(0),
            self.remote["behindCount"].as_u64().unwrap_or(0),
        )
    }

    /// `sourceControlProvider.kind` ("github", "gitlab", …), or "" when unknown.
    pub fn host_kind(&self) -> &str {
        self.local["sourceControlProvider"]["kind"]
            .as_str()
            .unwrap_or("")
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
    ("commit_push_pr", "Commit, push & create"),
    ("push", "Push"),
    ("create_pr", "Create"),
];

impl T3App {
    /// The open thread's working folder: its worktree, else its project's root.
    pub(crate) fn thread_cwd(&self) -> Option<String> {
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))?;
        thread.worktree_path.clone().or_else(|| {
            self.shell()
                .projects
                .iter()
                .find(|p| p.id == thread.project_id)
                .map(|p| p.workspace_root.clone())
        })
    }

    /// Lives as long as the thread selection; replaced on switch or reconnect.
    /// Switches the open thread's branch (web: `BranchToolbarBranchSelector`): a branch already
    /// checked out in another worktree moves the thread there; otherwise the thread's folder
    /// checks it out. Either way the thread records it with `thread.metadata.update`.
    pub(crate) fn switch_thread_branch(
        &mut self,
        branch: String,
        worktree: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .cloned()
        else {
            return;
        };
        if thread.is_running() {
            self.error = Some(crate::i18n::t("Stop the thread before switching branches.").into());
            return cx.notify();
        }
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let mut update =
            json!({ "type": "thread.metadata.update", "threadId": thread.id, "branch": branch });
        if let Some(path) =
            worktree.filter(|p| Some(p) != thread.worktree_path.as_ref() && *p != cwd)
        {
            update["worktreePath"] = json!(path);
            return self.dispatch(update, cx);
        }
        if self.git_busy {
            return;
        }
        self.git_busy = true;
        self.git_progress = Some(format!("Switching to {branch}…").into());
        cx.notify();
        let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let payload = json!({ "cwd": cwd, "refName": branch });
            let result = on_tokio(async move { client.call("vcs.switchRef", payload).await }).await;
            this.update(cx, |app, cx| {
                app.git_busy = false;
                match result {
                    Ok(Ok(_)) => {
                        app.git_progress = None;
                        app.dispatch_to(&env, update, cx);
                    }
                    Ok(Err(e)) => app.git_progress = Some(format!("Switch failed: {e}").into()),
                    Err(e) => app.git_progress = Some(format!("Switch failed: {e}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The open thread's local branches, for the branch menu.
    fn load_thread_refs(&mut self, cx: &mut Context<Self>) {
        self.thread_refs.clear();
        let (Some(client), Some(cwd), Some(thread)) =
            (self.client(), self.thread_cwd(), self.selected.clone())
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let payload = json!({ "cwd": cwd, "refKind": "local", "limit": 100 });
            if let Ok(Ok(result)) =
                on_tokio(async move { client.call("vcs.listRefs", payload).await }).await
            {
                this.update(cx, |app, cx| {
                    if app.selected.as_deref() == Some(thread.as_str()) {
                        app.thread_refs = result["refs"].as_array().cloned().unwrap_or_default();
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn subscribe_git(&mut self, cx: &mut Context<Self>) {
        self.load_thread_refs(cx);
        if let (Some(client), Some(old)) = (self.client(), self.git_sub.take()) {
            client.cancel(&old);
        }
        self.git = GitStatus::default();
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
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
                    app.refresh_changes_preview(cx);
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
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id));
        let mut payload =
            json!({ "actionId": uuid::Uuid::new_v4().to_string(), "cwd": cwd, "action": action });
        if let Some(thread) = thread {
            payload["threadId"] = json!(thread.id);
            payload["projectId"] = json!(thread.project_id);
        }
        if self.git_busy {
            return;
        }
        self.git_busy = true;
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
                app.git_busy = false;
                app.git_progress = outcome.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn git_pull(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        if self.git_busy {
            return;
        }
        self.git_busy = true;
        self.git_progress = Some("Pulling…".into());
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("vcs.pull", json!({ "cwd": cwd })).await }).await;
            this.update(cx, |app, cx| {
                app.git_busy = false;
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

    /// The Changes row's totals: the branch-range preview for the current status when it has
    /// arrived, else the status's own counts.
    fn changes_totals(&self) -> ChangesTotals {
        match &self.changes_preview {
            Some((key, Some(totals))) if Some(key) == self.changes_key().as_ref() => totals.clone(),
            _ => self.git.changes_totals(),
        }
    }

    /// The preview read's input, changing whenever the status totals do (web: `changesStatusRevision`).
    fn changes_key(&self) -> Option<String> {
        let (local, cwd) = (&self.git.local, self.thread_cwd()?);
        let (tree, branch) = (&local["workingTree"], &local["branchChanges"]);
        Some(
            json!([
                cwd,
                self.diff_ignore_whitespace(),
                local["refName"],
                tree["insertions"],
                tree["deletions"],
                branch["baseRef"],
                branch["insertions"],
                branch["deletions"]
            ])
            .to_string(),
        )
    }

    /// Reads the branch-range totals (web: `GitActionsControl`'s `diffPreview` query), only while
    /// the details card shows them and only once per status revision.
    pub(crate) fn refresh_changes_preview(&mut self, cx: &mut Context<Self>) {
        if !self.details_open || !self.git.is_repo() {
            return;
        }
        let (Some(client), Some(cwd), Some(key)) =
            (self.client(), self.thread_cwd(), self.changes_key())
        else {
            return;
        };
        if self
            .changes_preview
            .as_ref()
            .is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        self.changes_preview = Some((key.clone(), None));
        let payload = json!({ "cwd": cwd, "metadataOnly": true, "sourceKind": "branch-range",
            "ignoreWhitespace": self.diff_ignore_whitespace() });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("review.getDiffPreview", payload).await }).await;
            let Ok(Ok(result)) = result else { return };
            this.update(cx, |app, cx| {
                if let Some((k, slot)) = &mut app.changes_preview
                    && *k == key
                {
                    *slot = preview_totals(&result);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether the open thread links any pull request or issue.
    pub(crate) fn thread_has_links(&self) -> bool {
        !self.thread_pull_requests().is_empty()
            || self
                .selected
                .as_ref()
                .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
                .is_some_and(|t| !t.issues.is_empty())
    }

    /// The branch row (web: `BranchToolbar panelSection="branch"`), a menu of local branches.
    pub(crate) fn render_git_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.git.is_repo() {
            return div().into_any_element();
        }
        let c = cx.theme().colors;
        let refs: Vec<(String, Option<String>)> = self
            .thread_refs
            .iter()
            .filter_map(|r| {
                Some((
                    r["name"].as_str()?.to_owned(),
                    r["worktreePath"].as_str().map(str::to_owned),
                ))
            })
            .collect();
        let current = self.git.branch().map(str::to_owned);
        let app = cx.entity().downgrade();
        details_row(
            "thread-branch",
            "git-branch",
            h_flex()
                .flex_1()
                .min_w_0()
                .child(details_label(
                    current.clone().unwrap_or_else(|| t("Detached HEAD")),
                ))
                .child(
                    ui::icon("chevron-down")
                        .size(px(16.))
                        .flex_none()
                        .text_color(c.muted_foreground),
                ),
            cx,
        )
        .dropdown_menu(move |mut menu, _, _| {
            for (name, worktree) in refs.iter().take(60) {
                let (app, name, worktree) = (app.clone(), name.clone(), worktree.clone());
                let checked = current.as_deref() == Some(name.as_str());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        app.update(cx, |this, cx| {
                            this.switch_thread_branch(name.clone(), worktree.clone(), cx)
                        })
                        .ok();
                    },
                ));
            }
            menu
        })
        .into_any_element()
    }

    /// The git action split (web: `GitActionsControl` in the panel): the suggested next step, and
    /// every action plus PR and issue linking under the chevron.
    pub(crate) fn render_git_action(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.git.is_repo() {
            return div().into_any_element();
        }
        let c = cx.theme().colors;
        // While a run is going the row shows its progress and offers nothing else, as on the web.
        if self.git_busy {
            let progress = self
                .git_progress
                .clone()
                .unwrap_or_else(|| t("Working…").into());
            return div()
                .rounded(px(8.))
                .bg(c.foreground.opacity(0.055))
                .child(
                    details_row(
                        "git-quick-action",
                        "loader-circle",
                        details_label(progress),
                        cx,
                    )
                    .disabled(true),
                )
                .into_any_element();
        }
        let quick = quick_action(&self.git, false);
        let (short, _) = change_request_terms(&self.git);
        let (_, behind) = self.git.ahead_behind();
        let app = cx.entity().downgrade();
        let step = quick.step;
        let primary = details_row(
            "git-quick-action",
            quick.icon(&self.git),
            details_label(quick.label.clone()),
            cx,
        )
        .disabled(step.is_none())
        .when_some(quick.hint, |b, hint| b.tooltip(hint))
        .on_click(cx.listener(move |this, _, _, cx| match step {
            Some(Step::Pull) => this.git_pull(cx),
            Some(Step::Run(action)) => this.run_git_action(action, cx),
            None => {}
        }));
        v_flex()
            .child(details_split(
                "git-actions",
                primary,
                move |mut menu, _, _| {
                    for (action, label) in ACTIONS {
                        let app = app.clone();
                        let label = match action {
                            "commit_push_pr" => format!("{} {short}", t("Commit, push & create")),
                            "create_pr" => format!("{} {short}", t("Create")),
                            _ => t(label),
                        };
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            app.update(cx, |this, cx| this.run_git_action(action, cx))
                                .ok();
                        }));
                    }
                    if behind > 0 {
                        let app = app.clone();
                        menu =
                            menu.item(PopupMenuItem::new(t("Pull")).on_click(move |_, _, cx| {
                                app.update(cx, |this, cx| this.git_pull(cx)).ok();
                            }));
                    }
                    let (pr, issue) = (app.clone(), app.clone());
                    menu.separator()
                        .item(
                            PopupMenuItem::new(t("Link PR…")).on_click(move |_, window, cx| {
                                pr.update(cx, |this, cx| this.open_link_pr(window, cx)).ok();
                            }),
                        )
                        .item(PopupMenuItem::new(t("Link issue…")).on_click(
                            move |_, window, cx| {
                                issue
                                    .update(cx, |this, cx| this.open_link_issue(window, cx))
                                    .ok();
                            },
                        ))
                },
                cx,
            ))
            .children(self.git_progress.clone().map(|outcome| {
                div()
                    .h(px(20.))
                    .pl(px(36.))
                    .pr(px(10.))
                    .truncate()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child(outcome)
            }))
            .into_any_element()
    }

    /// "Changes vs origin/main +12 −3" (web: `GitActionsControl`'s changes row); opens the diff.
    pub(crate) fn render_changes_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.git.is_repo() {
            return div().into_any_element();
        }
        let c = cx.theme().colors;
        let (insertions, deletions, base) = self.changes_totals();
        let count = |n: u64, sign: &str, color: Hsla| {
            div()
                .text_color(if n > 0 { color } else { c.muted_foreground })
                .child(format!("{sign}{}", group_digits(n)))
        };
        let base = base.map_or_else(|| t("Uncommitted"), |r| tv("vs {ref}", &[("ref", &r)]));
        details_row(
            "details-changes",
            "file-diff",
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(div().flex_none().child(t("Changes")))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(base),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .pl_2()
                        .gap_2()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(count(insertions, "+", c.success_foreground))
                        .child(count(deletions, "-", c.danger_foreground)),
                ),
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.details_open = false;
            if !this.diff_open {
                this.toggle_diff(cx);
            }
        }))
        .into_any_element()
    }
}

/// (insertions, deletions, base ref) shown on the Changes row.
pub type ChangesTotals = (u64, u64, Option<String>);

/// What the git row's primary half does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Run(&'static str),
    Pull,
}

/// The suggested git step (web: `resolveQuickAction`); `step` is None when there is nothing to do.
#[derive(Clone, Debug, PartialEq)]
struct QuickAction {
    label: String,
    step: Option<Step>,
    hint: Option<String>,
}

impl QuickAction {
    /// web: `GitQuickActionIcon`; opening a change request shows the host's mark.
    fn icon(&self, git: &GitStatus) -> &'static str {
        match self.step {
            Some(Step::Pull) => "cloud-download",
            Some(Step::Run("commit")) => "git-commit-horizontal",
            Some(Step::Run("push" | "commit_push")) => "cloud-upload",
            Some(Step::Run(_)) => match git.host_kind() {
                "github" => "github",
                "gitlab" => "gitlab",
                _ => "git-pull-request",
            },
            None if self.label == t("Push") => "cloud-upload",
            None => "git-commit-horizontal",
        }
    }
}

/// The host's word for a pull request (web: `changeRequestTerminologyFor`): (short, singular).
/// Hosts the web does not know, or a repository without one, say "change request".
fn change_request_terms(git: &GitStatus) -> (&'static str, &'static str) {
    match git.host_kind() {
        "gitlab" => ("MR", "merge request"),
        "github" | "bitbucket" | "azure-devops" | "forgejo" | "cursor-origin" | "gitcafe"
        | "tanstack-source" => ("PR", "pull request"),
        _ => ("change request", "change request"),
    }
}

fn quick_action(git: &GitStatus, busy: bool) -> QuickAction {
    let run = |label: String, action| QuickAction {
        label,
        step: Some(Step::Run(action)),
        hint: None,
    };
    let hint = |label: &str, hint: String| QuickAction {
        label: t(label),
        step: None,
        hint: Some(hint),
    };
    if busy {
        return hint("Commit", t("Git action in progress."));
    }
    let (short, singular) = change_request_terms(git);
    let (local, remote) = (&git.local, &git.remote);
    if local["refName"].is_null() {
        return hint(
            "Commit",
            format!(
                "{} {singular}.",
                t("Create and checkout a ref before pushing or opening a")
            ),
        );
    }
    let has_upstream = remote["hasUpstream"] == true;
    let has_primary_remote = local["hasPrimaryRemote"] != false;
    let is_default = local["isDefaultRef"] == true;
    let open_pr = remote["pr"]["state"] == "open";
    let (ahead, behind) = git.ahead_behind();
    let ahead_of_default = remote["aheadOfDefaultCount"].as_u64().unwrap_or(ahead);
    let push_and_create = || run(format!("{} {short}", t("Push & create")), "create_pr");
    if local["hasWorkingTreeChanges"] == true {
        if behind > 0 || (!has_upstream && !has_primary_remote) {
            return run(t("Commit"), "commit");
        }
        if open_pr || is_default {
            return run(t("Commit & push"), "commit_push");
        }
        return run(format!("{} {short}", t("Commit, push &")), "commit_push_pr");
    }
    if !has_upstream {
        if !has_primary_remote {
            return hint(
                "Publish repository",
                t("Add a remote to publish this repository."),
            );
        }
        if ahead == 0 {
            return if open_pr {
                hint("Commit", t("Branch is up to date. No action needed."))
            } else {
                hint("Push", t("No local commits to push."))
            };
        }
        return if open_pr || is_default {
            run(t("Push"), "push")
        } else {
            push_and_create()
        };
    }
    if ahead > 0 && behind > 0 {
        return hint(
            "Sync ref",
            t("Branch has diverged from upstream. Rebase/merge first."),
        );
    }
    if behind > 0 {
        return QuickAction {
            label: t("Pull"),
            step: Some(Step::Pull),
            hint: None,
        };
    }
    if ahead > 0 {
        return if open_pr || is_default {
            run(t("Push"), "push")
        } else {
            push_and_create()
        };
    }
    if !open_pr && ahead_of_default > 0 && !is_default {
        return run(format!("{} {short}", t("Create")), "create_pr");
    }
    hint("Commit", t("Branch is up to date. No action needed."))
}

/// (insertions, deletions, base ref) of a diff preview's branch-range source with file stats
/// (web: `resolveChangesTotals`); None leaves the status counts in place.
fn preview_totals(result: &Value) -> Option<ChangesTotals> {
    let source = result["sources"]
        .as_array()?
        .iter()
        .find(|s| s["kind"] == "branch-range")?;
    let files = source["files"].as_array()?;
    let sum = |k: &str| files.iter().filter_map(|f| f[k].as_u64()).sum();
    Some((
        sum("additions"),
        sum("deletions"),
        source["baseRef"].as_str().map(str::to_owned),
    ))
}

/// 34604 → "34,604" (web: `formatDiffCount`).
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
    use super::{GitStatus, Step, git_result_summary, group_digits, quick_action};
    use serde_json::json;

    #[test]
    fn tracks_local_and_remote_status() {
        let mut git = GitStatus::default();
        git.apply(&json!({"_tag": "snapshot", "remote": null, "local": {"isRepo": true, "refName": "main",
            "workingTree": {"files": [{"path": "a", "insertions": 3, "deletions": 1}, {"path": "b", "insertions": 2, "deletions": 0}]}}}));
        assert_eq!(
            (git.branch(), git.changes_totals(), git.ahead_behind()),
            (Some("main"), (0, 0, None), (0, 0))
        );
        git.apply(
            &json!({"_tag": "remoteUpdated", "remote": {"aheadCount": 2, "behindCount": 1,
            "pr": {"number": 7, "url": "https://x/pr/7"}}}),
        );
        assert_eq!(git.ahead_behind(), (2, 1));
        assert_eq!(git.pr(), Some((7, "https://x/pr/7".into())));
        git.apply(&json!({"_tag": "localUpdated", "local": {"isRepo": true, "refName": "feat", "workingTree": {"files": []}}}));
        assert_eq!(git.branch(), Some("feat"));
    }

    fn status(local: serde_json::Value, remote: serde_json::Value) -> GitStatus {
        let mut git = GitStatus::default();
        git.apply(&json!({"_tag": "snapshot", "local": local, "remote": remote}));
        git
    }

    #[test]
    fn totals_prefer_branch_changes_like_the_web() {
        let tree = json!({"insertions": 4, "deletions": 1, "files": []});
        let branch = json!({"baseRef": "origin/main", "insertions": 30, "deletions": 2});
        let git = status(
            json!({"workingTree": tree, "branchChanges": branch}),
            json!(null),
        );
        assert_eq!(git.changes_totals(), (30, 2, Some("origin/main".into())));
        let empty = json!({"baseRef": "origin/main", "insertions": 0, "deletions": 0});
        let git = status(
            json!({"workingTree": tree, "branchChanges": empty}),
            json!(null),
        );
        assert_eq!(git.changes_totals(), (4, 1, None));
        let git = status(
            json!({"workingTree": {"insertions": 0, "deletions": 0}, "branchChanges": empty}),
            json!(null),
        );
        assert_eq!(git.changes_totals(), (0, 0, Some("origin/main".into())));
    }

    #[test]
    fn suggests_the_next_git_step_like_the_web() {
        let step = |local: serde_json::Value, remote: serde_json::Value| {
            let q = quick_action(&status(local, remote), false);
            (q.label, q.step)
        };
        let base = |dirty: bool| {
            json!({"refName": "feat", "hasWorkingTreeChanges": dirty, "hasPrimaryRemote": true,
            "isDefaultRef": false, "sourceControlProvider": {"kind": "github"}})
        };
        let up = |ahead: u64, behind: u64| json!({"hasUpstream": true, "aheadCount": ahead, "behindCount": behind, "pr": null});
        let s = |l: &str| l.to_owned();
        assert_eq!(
            step(base(true), up(0, 0)),
            (s("Commit, push & PR"), Some(Step::Run("commit_push_pr")))
        );
        assert_eq!(
            step(base(true), up(0, 1)),
            (s("Commit"), Some(Step::Run("commit")))
        );
        assert_eq!(step(base(false), up(0, 2)), (s("Pull"), Some(Step::Pull)));
        assert_eq!(step(base(false), up(1, 1)).1, None);
        assert_eq!(
            step(base(false), up(3, 0)),
            (s("Push & create PR"), Some(Step::Run("create_pr")))
        );
        let no_upstream = json!({"hasUpstream": false, "aheadCount": 0, "behindCount": 0, "aheadOfDefaultCount": 2, "pr": null});
        assert_eq!(step(base(false), no_upstream), (s("Push"), None));
        let ahead_of_default = json!({"hasUpstream": true, "aheadCount": 0, "behindCount": 0, "aheadOfDefaultCount": 2, "pr": null});
        assert_eq!(
            step(base(false), ahead_of_default.clone()),
            (s("Create PR"), Some(Step::Run("create_pr")))
        );
        let open_pr = json!({"hasUpstream": true, "aheadCount": 1, "behindCount": 0, "pr": {"state": "open"}});
        assert_eq!(
            step(base(false), open_pr),
            (s("Push"), Some(Step::Run("push")))
        );
        assert_eq!(step(json!({"refName": null}), json!(null)).1, None);

        let gitlab = json!({"refName": "feat", "hasWorkingTreeChanges": false, "hasPrimaryRemote": true,
            "sourceControlProvider": {"kind": "gitlab"}});
        assert_eq!(step(gitlab, ahead_of_default.clone()).0, "Create MR");
        let unknown =
            json!({"refName": "feat", "hasWorkingTreeChanges": true, "hasPrimaryRemote": true});
        assert_eq!(step(unknown, up(0, 0)).0, "Commit, push & change request");
        let busy = quick_action(&status(base(true), up(0, 0)), true);
        assert_eq!(
            (busy.step, busy.hint.as_deref()),
            (None, Some("Git action in progress."))
        );
    }

    #[test]
    fn reads_branch_range_preview_totals() {
        let result = json!({"sources": [
            {"kind": "working-tree", "baseRef": null, "files": [{"additions": 9, "deletions": 9}]},
            {"kind": "branch-range", "baseRef": "origin/main", "files": [{"additions": 30, "deletions": 1}, {"additions": 4, "deletions": 0}]}]});
        assert_eq!(
            super::preview_totals(&result),
            Some((34, 1, Some("origin/main".into())))
        );
        let no_files = json!({"sources": [{"kind": "branch-range", "baseRef": "origin/main"}]});
        assert_eq!(super::preview_totals(&no_files), None);
    }

    #[test]
    fn groups_diff_counts() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(34604), "34,604");
        assert_eq!(group_digits(1234567), "1,234,567");
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
