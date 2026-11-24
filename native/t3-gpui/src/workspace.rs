//! Where a new thread runs: the project folder, or a new worktree from a base branch
//! (web: `BranchToolbarEnvModeSelector.tsx`, `BranchToolbarBranchSelector.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::{T3App, on_tokio};

/// The branch a new worktree starts from by default: the repository's default, else the
/// current one, else the first listed.
pub fn default_base_ref(refs: &[Value]) -> Option<String> {
    let name = |r: &Value| r["name"].as_str().map(str::to_owned);
    refs.iter()
        .find(|r| r["isDefault"] == true)
        .or_else(|| refs.iter().find(|r| r["current"] == true))
        .or(refs.first())
        .and_then(name)
}

/// `OrchestrationV2ThreadLaunchWorkspaceStrategy` for the draft's choices.
pub fn strategy(mode: &str, base_ref: Option<&str>, start_from_origin: bool) -> Value {
    match (mode, base_ref) {
        ("worktree", Some(base)) => {
            let mut s = json!({ "type": "worktree", "baseRef": base });
            if start_from_origin {
                s["startFromOrigin"] = json!(true);
            }
            s
        }
        _ => json!({ "type": "root" }),
    }
}

impl T3App {
    /// The draft's default mode: the project's, else the environment's setting, else local.
    pub(crate) fn default_env_mode(&self, project_id: &str) -> String {
        let project = self.shell().projects.iter().find(|p| p.id == project_id);
        project
            .and_then(|p| p.default_thread_env_mode.clone())
            .or_else(|| {
                self.config().settings["defaultThreadEnvMode"]
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "local".into())
    }

    /// Loads the draft project's local branches for the base branch menu.
    pub(crate) fn load_draft_refs(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.draft_refs.clear();
        self.draft_base_ref = None;
        let cwd = self
            .shell()
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.workspace_root.clone());
        let (Some(client), Some(cwd)) = (self.client(), cwd) else {
            return;
        };
        let project_id = project_id.to_owned();
        cx.spawn(async move |this, cx| {
            let payload = json!({ "cwd": cwd, "refKind": "local", "limit": 100 });
            let result = on_tokio(async move { client.call("vcs.listRefs", payload).await }).await;
            if let Ok(Ok(result)) = result {
                this.update(cx, |app, cx| {
                    if app.draft_project.as_deref() != Some(project_id.as_str()) {
                        return;
                    }
                    app.draft_refs = result["refs"].as_array().cloned().unwrap_or_default();
                    app.draft_base_ref = default_base_ref(&app.draft_refs);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn draft_strategy(&self) -> Value {
        let from_origin = self.config().settings["newWorktreesStartFromOrigin"] == true;
        strategy(
            &self.draft_env_mode,
            self.draft_base_ref.as_deref(),
            from_origin,
        )
    }

    /// Local / New worktree, and the base branch when it is a worktree.
    pub(crate) fn render_workspace_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let worktree = self.draft_env_mode == "worktree";
        let can_worktree = !self.draft_refs.is_empty();
        let app = cx.entity().downgrade();
        let branches: Vec<String> = self
            .draft_refs
            .iter()
            .filter_map(|r| r["name"].as_str().map(str::to_owned))
            .collect();
        let base = self.draft_base_ref.clone();
        h_flex()
            .gap_2()
            .child(
                Button::new("env-mode")
                    .small()
                    .ghost()
                    .label(if worktree { "New worktree" } else { "Local" })
                    .dropdown_menu({
                        let app = app.clone();
                        move |menu, _, _| {
                            let pick = |label: &str, mode: &'static str| {
                                let app = app.clone();
                                PopupMenuItem::new(label.to_string())
                                    .checked((mode == "worktree") == worktree)
                                    .on_click(move |_, _, cx| {
                                        app.update(cx, |this, cx| {
                                            this.draft_env_mode = mode.into();
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                            };
                            let menu = menu.item(pick("Local", "local"));
                            if can_worktree {
                                menu.item(pick("New worktree", "worktree"))
                            } else {
                                menu
                            }
                        }
                    }),
            )
            .when(worktree, |row| {
                row.child(
                    Button::new("base-ref")
                        .small()
                        .ghost()
                        .label(format!(
                            "from {}",
                            base.clone().unwrap_or_else(|| "…".into())
                        ))
                        .dropdown_menu(move |mut menu, _, _| {
                            for name in branches.iter().take(60) {
                                let (app, name) = (app.clone(), name.clone());
                                let checked = base.as_deref() == Some(name.as_str());
                                menu = menu.item(
                                    PopupMenuItem::new(name.clone()).checked(checked).on_click(
                                        move |_, _, cx| {
                                            app.update(cx, |this, cx| {
                                                this.draft_base_ref = Some(name.clone());
                                                cx.notify();
                                            })
                                            .ok();
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{default_base_ref, strategy};
    use serde_json::json;

    #[test]
    fn picks_the_default_branch_then_the_current_one() {
        let refs = vec![
            json!({"name": "feat", "current": true}),
            json!({"name": "main", "isDefault": true}),
        ];
        assert_eq!(default_base_ref(&refs).as_deref(), Some("main"));
        assert_eq!(default_base_ref(&refs[..1]).as_deref(), Some("feat"));
        assert_eq!(default_base_ref(&[]), None);
    }

    #[test]
    fn builds_launch_strategies() {
        assert_eq!(
            strategy("local", Some("main"), false),
            json!({"type": "root"})
        );
        assert_eq!(
            strategy("worktree", Some("main"), true),
            json!({"type": "worktree", "baseRef": "main", "startFromOrigin": true})
        );
        assert_eq!(
            strategy("worktree", None, false),
            json!({"type": "root"}),
            "no branch list, no worktree"
        );
    }
}
