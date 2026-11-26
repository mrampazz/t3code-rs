//! In-thread find (web: `ThreadFindBar.tsx`, the `chat.find` command): matches the loaded
//! timeline rows and steps through them.
//! shortcut: searches only the loaded rows; the web also asks `orchestration.searchThread`
//! for history outside the window. Upgrade once history paging lands.

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex};
use gpui_kit::*;

use crate::T3App;
use crate::model::TurnItem;
use crate::timeline::Row;

/// The text a find matches in one item: its message, command, output, file and title.
fn item_text(item: &TurnItem) -> String {
    [
        "text", "markdown", "input", "output", "fileName", "title", "prompt", "result",
    ]
    .iter()
    .filter_map(|k| item.raw[*k].as_str())
    .collect::<Vec<_>>()
    .join("\n")
    .to_lowercase()
}

/// Indexes of the rows whose items contain `query` (case-insensitive).
pub fn matching_rows(rows: &[Row<'_>], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    rows.iter()
        .enumerate()
        .filter(|(_, row)| match row {
            Row::Item(item) => item_text(item).contains(&query),
            Row::Group { items, .. } | Row::Fold { items, .. } => {
                items.iter().any(|i| item_text(i).contains(&query))
            }
        })
        .map(|(i, _)| i)
        .collect()
}

impl T3App {
    pub(crate) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.find_index = 0;
        self.find_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// After the query changes: back to the first match.
    pub(crate) fn first_find(&mut self, cx: &mut Context<Self>) {
        self.find_index = 0;
        let query = self.find_input.read(cx).value().to_string();
        let rows = self.rows();
        if let Some(&first) = matching_rows(&rows, &query).first() {
            self.transcript.scroll_to_reveal_item(first);
        }
        cx.notify();
    }

    /// Moves to the next (or previous) match and scrolls it into view.
    pub(crate) fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let query = self.find_input.read(cx).value().to_string();
        let rows = self.rows();
        let matches = matching_rows(&rows, &query);
        if matches.is_empty() {
            return cx.notify();
        }
        let n = matches.len();
        self.find_index = if forward {
            (self.find_index + 1) % n
        } else {
            (self.find_index + n - 1) % n
        };
        self.transcript
            .scroll_to_reveal_item(matches[self.find_index]);
        cx.notify();
    }

    pub(crate) fn render_find_bar(&self, total: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let position = if total == 0 {
            "No results".to_string()
        } else {
            format!("{} of {total}", self.find_index.min(total - 1) + 1)
        };
        h_flex()
            .px_4()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .w(px(320.))
                    .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        if e.keystroke.key == "escape" {
                            this.find_open = false;
                            this.composer
                                .update(cx, |input, cx| input.focus(window, cx));
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(Input::new(&self.find_input).small()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child(position),
            )
            .child(
                Button::new("find-prev")
                    .xsmall()
                    .ghost()
                    .label("↑")
                    .on_click(cx.listener(|this, _, _, cx| this.step_find(false, cx))),
            )
            .child(
                Button::new("find-next")
                    .xsmall()
                    .ghost()
                    .label("↓")
                    .on_click(cx.listener(|this, _, _, cx| this.step_find(true, cx))),
            )
            .child(
                Button::new("find-close")
                    .xsmall()
                    .ghost()
                    .label(t("Done"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.find_open = false;
                        cx.notify();
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::matching_rows;
    use crate::model::TurnItem;
    use crate::timeline::group_rows;
    use serde_json::json;

    #[test]
    fn matches_rows_and_groups_case_insensitively() {
        let item = |id: &str, kind: &str, extra: serde_json::Value| {
            let mut v = json!({"id": id, "ordinal": 1, "type": kind});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            TurnItem::from_value(&v).unwrap()
        };
        let items = [
            item("u", "user_message", json!({"text": "Fix the Login page"})),
            item("c1", "command_execution", json!({"input": "npm test"})),
            item(
                "c2",
                "command_execution",
                json!({"input": "ls", "output": "login.ts"}),
            ),
            item("a", "assistant_message", json!({"text": "Done."})),
        ];
        let rows = group_rows(items.iter());
        assert_eq!(
            matching_rows(&rows, "LOGIN"),
            [0, 1],
            "the user row and the work group"
        );
        assert!(matching_rows(&rows, "  ").is_empty());
    }
}
