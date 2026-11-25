//! Checkpoint diff panel (web: `apps/web/src/components/DiffPanel.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{Diff, DiffFile, DiffMode, DiffState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// Ends a diff the server cut at its size limit (`CHECKPOINT_DIFF_TRUNCATED_MARKER`).
const TRUNCATED_MARKER: &str = "\n\n[truncated]";

impl T3App {
    pub(crate) fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_open = !self.diff_open;
        if self.diff_open {
            self.load_diff(None, cx);
        }
        cx.notify();
    }

    /// `None` diffs the whole thread up to its latest turn; `Some(n)` diffs turn `n` alone.
    pub(crate) fn load_diff(&mut self, target: Option<u64>, cx: &mut Context<Self>) {
        self.diff_target = target;
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let (Some(client), Some(thread_id), Some(latest)) = (
            self.client(),
            self.selected.clone(),
            self.thread.latest_turn(),
        ) else {
            self.diff_state = None;
            self.diff_note = Some("No changes yet.".into());
            return cx.notify();
        };
        let ignore_whitespace = self.diff_ignore_whitespace();
        let (method, mut payload) = match target {
            Some(turn) => (
                "orchestration.getTurnDiff",
                json!({ "threadId": thread_id, "fromTurnCount": turn.saturating_sub(1), "toTurnCount": turn }),
            ),
            None => (
                "orchestration.getFullThreadDiff",
                json!({ "threadId": thread_id, "toTurnCount": latest }),
            ),
        };
        payload["ignoreWhitespace"] = json!(ignore_whitespace);
        self.diff_note = Some("Loading…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            this.update(cx, |app, cx| {
                if app.diff_generation != generation {
                    return;
                }
                let result = result.map_err(anyhow::Error::from).and_then(|r| r);
                match result.map(|value| parse_turn_diff(&value)) {
                    Ok(Ok((files, truncated))) if !files.is_empty() => {
                        let mode = if app.diff_split {
                            DiffMode::Split
                        } else {
                            DiffMode::Unified
                        };
                        app.diff_state =
                            Some(cx.new(|cx| DiffState::new(files, cx).with_mode(mode)));
                        app.diff_note = truncated.then(|| "The server truncated this diff.".into());
                    }
                    Ok(Ok(_)) => {
                        app.diff_state = None;
                        app.diff_note = Some("No changes in this range.".into());
                    }
                    Ok(Err(e)) | Err(e) => {
                        app.diff_state = None;
                        app.diff_note = Some(format!("Could not load the diff: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// This session's choice, else the `diffIgnoreWhitespace` setting.
    fn diff_ignore_whitespace(&self) -> bool {
        self.diff_whitespace
            .unwrap_or(self.config().settings["diffIgnoreWhitespace"] == true)
    }

    pub(crate) fn render_diff_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mut turns = h_flex()
            .flex_wrap()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(c.border)
            .child(
                Button::new("diff-whitespace")
                    .xsmall()
                    .ghost()
                    .label(if self.diff_ignore_whitespace() {
                        t("Show whitespace changes")
                    } else {
                        t("Hide whitespace changes")
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.diff_whitespace = Some(!this.diff_ignore_whitespace());
                        this.load_diff(this.diff_target, cx);
                    })),
            )
            .child(self.turn_button("diff-all", "All turns".into(), None, cx));
        for checkpoint in &self.thread.checkpoints {
            let label = format!(
                "Turn {} · {} {} · +{} −{}",
                checkpoint.turn,
                checkpoint.files,
                if checkpoint.files == 1 {
                    "file"
                } else {
                    "files"
                },
                checkpoint.additions,
                checkpoint.deletions
            );
            turns = turns.child(self.turn_button(
                SharedString::from(format!("diff-{}", checkpoint.id)),
                label,
                Some(checkpoint.turn),
                cx,
            ));
        }
        v_flex()
            .w(px(560.))
            .h_full()
            .border_l_1()
            .border_color(c.border)
            .child(turns)
            .children(self.diff_note.clone().map(|note| {
                div()
                    .p_3()
                    .text_sm()
                    .text_color(c.muted_foreground)
                    .child(note)
            }))
            .children(
                self.diff_state
                    .as_ref()
                    .map(|state| self.render_diff_files(state, cx)),
            )
            .children(self.diff_state.as_ref().map(|state| {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Diff::new(state).render_header_suffix(|file, _, _| {
                        let path = file.path().clone();
                        Button::new(SharedString::from(format!("diff-open-{path}")))
                            .xsmall()
                            .ghost()
                            .label(t("Open"))
                            .tooltip(t("Open in editor"))
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(crate::editors::OpenLink { url: path.clone() }),
                                    cx,
                                )
                            })
                    }))
            }))
    }

    /// The changed files (web: `DiffFileTree`); a click scrolls the diff to that file.
    /// shortcut: a flat list rather than a folder tree.
    fn render_diff_files(
        &self,
        state: &Entity<DiffState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = cx.theme().colors;
        let files: Vec<(SharedString, usize, usize)> = state
            .read(cx)
            .files()
            .iter()
            .map(|f| (f.path().clone(), f.additions(), f.deletions()))
            .collect();
        let split = self.diff_split;
        v_flex()
            .border_b_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(
                        Button::new("diff-files-toggle")
                            .xsmall()
                            .ghost()
                            .label(if self.diff_files_open {
                                t("Hide file tree")
                            } else {
                                format!("{} ({})", t("Changes"), files.len())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.diff_files_open = !this.diff_files_open;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("diff-layout")
                            .xsmall()
                            .ghost()
                            .label(if split { "Unified" } else { "Split" })
                            .tooltip(t("Diff layout"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.diff_split = !this.diff_split;
                                let mode = if this.diff_split {
                                    DiffMode::Split
                                } else {
                                    DiffMode::Unified
                                };
                                if let Some(state) = &this.diff_state {
                                    state.update(cx, |s, cx| s.set_mode(mode, cx));
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(self.diff_files_open, |col| {
                col.child(
                    v_flex()
                        .id("diff-files")
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .pb_1()
                        .children(
                            files
                                .into_iter()
                                .enumerate()
                                .map(|(i, (path, adds, dels))| {
                                    let target = path.clone();
                                    h_flex()
                                        .id(("diff-file", i))
                                        .px_3()
                                        .gap_2()
                                        .text_xs()
                                        .cursor_pointer()
                                        .hover(|r| r.bg(c.list_hover))
                                        .child(div().flex_1().truncate().child(path))
                                        .child(
                                            div().text_color(c.success).child(format!("+{adds}")),
                                        )
                                        .child(div().text_color(c.danger).child(format!("−{dels}")))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(state) = &this.diff_state {
                                                state.update(cx, |s, cx| {
                                                    s.scroll_to_file(&target, cx)
                                                });
                                            }
                                        }))
                                }),
                        ),
                )
            })
    }

    fn turn_button(
        &self,
        id: impl Into<ElementId>,
        label: String,
        target: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Button {
        let button = Button::new(id)
            .xsmall()
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| this.load_diff(target, cx)));
        if self.diff_target == target {
            button.primary()
        } else {
            button.ghost()
        }
    }
}

/// Parses a `ThreadTurnDiff`: the thread folder's patch, then each tracked repository's.
pub(crate) fn parse_turn_diff(value: &Value) -> anyhow::Result<(Vec<DiffFile>, bool)> {
    let repositories = value["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| &r["diff"]);
    let mut files = Vec::new();
    let mut truncated = false;
    for patch in std::iter::once(&value["diff"])
        .chain(repositories)
        .filter_map(Value::as_str)
    {
        let patch = match patch.strip_suffix(TRUNCATED_MARKER) {
            Some(rest) => {
                truncated = true;
                rest
            }
            None => patch,
        };
        if !patch.trim().is_empty() {
            files.extend(DiffFile::parse(patch).map_err(|e| anyhow::anyhow!("{e:?}"))?);
        }
    }
    Ok((files, truncated))
}

#[cfg(test)]
mod tests {
    use super::{TRUNCATED_MARKER, parse_turn_diff};
    use serde_json::json;

    const PATCH: &str =
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";

    #[test]
    fn parses_thread_and_repository_patches() {
        let value = json!({"diff": PATCH, "repositories": [{"name": "lib", "diff": format!("{PATCH}{TRUNCATED_MARKER}")}]});
        let (files, truncated) = parse_turn_diff(&value).unwrap();
        assert_eq!(files.len(), 2);
        assert!(truncated);
        let (files, truncated) = parse_turn_diff(&json!({"diff": ""})).unwrap();
        assert!(files.is_empty() && !truncated);
    }
}
