//! Checkpoint diff panel (web: `apps/web/src/components/DiffPanel.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

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
            self.client.clone(),
            self.selected.clone(),
            self.thread.latest_turn(),
        ) else {
            self.diff_state = None;
            self.diff_note = Some("No changes yet.".into());
            return cx.notify();
        };
        let (method, payload) = match target {
            Some(turn) => (
                "orchestration.getTurnDiff",
                json!({ "threadId": thread_id, "fromTurnCount": turn.saturating_sub(1), "toTurnCount": turn }),
            ),
            None => (
                "orchestration.getFullThreadDiff",
                json!({ "threadId": thread_id, "toTurnCount": latest }),
            ),
        };
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
                        app.diff_state = Some(cx.new(|cx| DiffState::new(files, cx)));
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

    pub(crate) fn render_diff_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mut turns = h_flex()
            .flex_wrap()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(c.border)
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
                    .map(|state| div().flex_1().min_h_0().child(Diff::new(state))),
            )
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
