//! Timeline rows per turn item type (web: `apps/web/src/components/chat/Timeline*.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Sizable, ThemeColor, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::Value;

use crate::T3App;
use crate::model::TurnItem;

/// Long tool output is cut to its tail; the full text stays on the server.
const OUTPUT_TAIL_CHARS: usize = 8_000;

impl T3App {
    pub(crate) fn render_item(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let expanded = self.expanded.contains(&item.id);
        let text = item.str("text").to_string();
        match item.kind.as_str() {
            "user_message" => h_flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.8))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(c.secondary)
                        .child(text),
                )
                .into_any_element(),
            "assistant_message" => {
                TextView::markdown(SharedString::from(item.id.clone()), text).into_any_element()
            }
            "proposed_plan" => v_flex()
                .gap_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(c.border)
                .child(
                    div()
                        .text_xs()
                        .text_color(c.muted_foreground)
                        .child("Proposed plan"),
                )
                .child(TextView::markdown(
                    SharedString::from(item.id.clone()),
                    item.str("markdown").to_string(),
                ))
                .into_any_element(),
            "todo_list" => v_flex()
                .gap_1()
                .text_sm()
                .children(
                    item.raw["steps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|step| {
                            let (mark, color) = match step["status"].as_str() {
                                Some("completed") => ("☑", c.muted_foreground),
                                Some("running") => ("◐", c.foreground),
                                _ => ("☐", c.muted_foreground),
                            };
                            h_flex()
                                .gap_2()
                                .text_color(color)
                                .child(mark)
                                .child(step["text"].as_str().unwrap_or("").to_string())
                        }),
                )
                .into_any_element(),
            "error" => div()
                .text_sm()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(c.danger)
                .text_color(c.danger)
                .child(
                    item.raw["failure"]["message"]
                        .as_str()
                        .unwrap_or("The provider failed.")
                        .to_string(),
                )
                .into_any_element(),
            "approval_request" => {
                let pending = is_open(item);
                let request_id = item.str("requestId").to_string();
                let thread_id = item.str("threadId").to_string();
                let buttons: Vec<Button> = approval_options(item)
                    .into_iter()
                    .map(|(decision, label)| {
                        let command = serde_json::json!({
                            "type": "runtime-request.respond",
                            "threadId": thread_id,
                            "requestId": request_id,
                            "decision": decision,
                        });
                        let button =
                            Button::new(SharedString::from(format!("{}-{decision}", item.id)))
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.dispatch(command.clone(), cx)
                                }));
                        if decision == "accept" {
                            button.primary()
                        } else {
                            button.outline()
                        }
                    })
                    .collect();
                v_flex()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(c.warning)
                    .text_sm()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title_or(item, "Approval requested")),
                    )
                    .children(
                        [item.str("prompt"), item.str("reason")]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .map(|s| div().text_color(c.muted_foreground).child(s.to_string())),
                    )
                    .when(pending, |col| {
                        col.child(h_flex().pt_1().gap_2().children(buttons))
                    })
                    .into_any_element()
            }
            "user_input_request" if is_open(item) => self.render_question(item, &c, cx),
            "system_notice" | "run_interrupt_request" | "run_interrupt_result" => div()
                .text_xs()
                .text_color(c.muted_foreground)
                .child(item.str("message").to_string())
                .into_any_element(),
            _ => self.render_tool(item, expanded, &c, mono, cx),
        }
    }

    /// Agent questions: toggle options, then submit; composer text answers as a custom reply.
    fn render_question(
        &self,
        item: &TurnItem,
        c: &ThemeColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request_id = item.str("requestId").to_string();
        let thread_id = item.str("threadId").to_string();
        let questions = item.raw["questions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut card = v_flex()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(c.info)
            .text_sm();
        for q in &questions {
            let qid = q["id"].as_str().unwrap_or("").to_string();
            let key = format!("{request_id}:{qid}");
            let picked = self.input_picks.get(&key).cloned().unwrap_or_default();
            let multi = q["multiSelect"] == true;
            let options = q["options"].as_array().cloned().unwrap_or_default();
            card =
                card.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .child(q["header"].as_str().unwrap_or("").to_string()),
                        )
                        .child(div().child(q["question"].as_str().unwrap_or("").to_string()))
                        .child(h_flex().flex_wrap().gap_2().children(
                            options.iter().enumerate().map(|(i, option)| {
                                let value = crate::model::option_value(option);
                                let on = picked.contains(&value);
                                let key = key.clone();
                                let button = Button::new(SharedString::from(format!(
                                    "{}-{qid}-{i}",
                                    item.id
                                )))
                                .small()
                                .label(option["label"].as_str().unwrap_or("").to_string())
                                .tooltip(option["description"].as_str().unwrap_or("").to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let picks = this.input_picks.entry(key.clone()).or_default();
                                    match (picks.iter().position(|v| v == &value), multi) {
                                        (Some(i), _) => {
                                            picks.remove(i);
                                        }
                                        (None, true) => picks.push(value.clone()),
                                        (None, false) => *picks = vec![value.clone()],
                                    }
                                    cx.notify();
                                }));
                                if on {
                                    button.primary()
                                } else {
                                    button.outline()
                                }
                            }),
                        )),
                );
        }
        let submit = {
            let (request_id, thread_id) = (request_id.clone(), thread_id.clone());
            Button::new(SharedString::from(format!("{}-submit", item.id)))
                .small()
                .primary()
                .label("Submit")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let custom = this.composer.read(cx).value().to_string();
                    let picks = |qid: &str| {
                        this.input_picks
                            .get(&format!("{request_id}:{qid}"))
                            .cloned()
                            .unwrap_or_default()
                    };
                    let Some(answers) = crate::model::build_answers(&questions, picks, &custom)
                    else {
                        this.error = Some("Answer every required question first.".into());
                        return cx.notify();
                    };
                    if !custom.trim().is_empty() {
                        this.composer
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    this.error = None;
                    this.dispatch(
                        serde_json::json!({
                            "type": "runtime-request.respond",
                            "threadId": thread_id,
                            "requestId": request_id,
                            "answers": answers,
                        }),
                        cx,
                    );
                }))
        };
        let dismiss = Button::new(SharedString::from(format!("{}-dismiss", item.id)))
            .small()
            .ghost()
            .label("Dismiss")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dispatch(
                    serde_json::json!({ "type": "thread.user-input.dismiss", "threadId": thread_id, "requestId": request_id }),
                    cx,
                )
            }));
        card.child(
            h_flex().gap_2().child(submit).child(dismiss).child(
                div()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child("Or type an answer in the composer."),
            ),
        )
        .into_any_element()
    }

    /// One-line work entry that expands to its detail.
    fn render_tool(
        &self,
        item: &TurnItem,
        expanded: bool,
        c: &ThemeColor,
        mono: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (summary, detail) = tool_parts(item);
        let (glyph, glyph_color) = match item.str("status") {
            "running" | "pending" | "waiting" => ("●", c.info),
            "failed" => ("✕", c.danger),
            "cancelled" | "interrupted" => ("–", c.muted_foreground),
            _ => ("✓", c.muted_foreground),
        };
        let id = item.id.clone();
        let has_detail = detail.is_some();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id(SharedString::from(format!("tool-{id}")))
                    .gap_2()
                    .text_sm()
                    .text_color(c.muted_foreground)
                    .when(has_detail, |row| {
                        row.cursor_pointer()
                            .hover(|row| row.text_color(c.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&id) {
                                    this.expanded.insert(id.clone());
                                }
                                cx.notify();
                            }))
                    })
                    .child(div().text_color(glyph_color).child(glyph))
                    .child(div().flex_1().truncate().child(summary)),
            )
            .when_some(detail.filter(|_| expanded), |col, detail| {
                col.child(
                    div()
                        .ml_5()
                        .p_2()
                        .rounded_md()
                        .bg(c.secondary)
                        .font_family(mono)
                        .text_xs()
                        .children(detail.lines().map(|line| {
                            let color = if line.starts_with('+') && !line.starts_with("+++") {
                                c.success
                            } else if line.starts_with('-') && !line.starts_with("---") {
                                c.danger
                            } else {
                                c.foreground
                            };
                            div().text_color(color).child(if line.is_empty() {
                                " ".to_string()
                            } else {
                                line.to_string()
                            })
                        })),
                )
            })
            .into_any_element()
    }
}

/// Server-provided options, else the web client's defaults; decline first, approve last.
pub(crate) fn approval_options(item: &TurnItem) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = item.raw["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some((
                o["decision"].as_str()?.to_owned(),
                o["label"].as_str()?.to_owned(),
            ))
        })
        .collect();
    if options.is_empty() {
        options = [
            ("decline", "Decline"),
            ("acceptForSession", "Always allow this session"),
            ("accept", "Approve"),
        ]
        .map(|(d, l)| (d.to_owned(), l.to_owned()))
        .into();
    }
    options.sort_by_key(|(d, _)| match d.as_str() {
        "decline" => 0,
        "accept" => 2,
        _ => 1,
    });
    options
}

/// A request still waiting for the user.
fn is_open(item: &TurnItem) -> bool {
    !matches!(
        item.str("status"),
        "completed" | "failed" | "cancelled" | "interrupted"
    )
}

fn title_or(item: &TurnItem, fallback: &str) -> String {
    let title = item.str("title");
    if title.is_empty() {
        fallback.to_string()
    } else {
        title.to_string()
    }
}

/// Summary line and optional expandable detail for work items.
pub(crate) fn tool_parts(item: &TurnItem) -> (String, Option<String>) {
    let raw = &item.raw;
    match item.kind.as_str() {
        "reasoning" => {
            let text = item.str("text");
            let first = text
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("Thinking");
            (
                "Thinking · ".to_string() + first.trim_matches('*'),
                non_empty(text),
            )
        }
        "command_execution" => {
            let mut summary = format!("$ {}", item.str("input").lines().next().unwrap_or(""));
            if let Some(code) = raw["exitCode"].as_i64().filter(|&c| c != 0) {
                summary += &format!("  (exit {code})");
            }
            let output = raw["output"].as_str().map(tail);
            (
                summary,
                Some(format!(
                    "$ {}\n{}",
                    item.str("input"),
                    output.unwrap_or_default()
                )),
            )
        }
        "file_change" => {
            let mut summary = format!("Edited {}", item.str("fileName"));
            if let (Some(a), Some(d)) = (raw["additions"].as_u64(), raw["deletions"].as_u64()) {
                summary += &format!("  +{a} −{d}");
            }
            (summary, raw["diffStr"].as_str().map(tail))
        }
        "file_search" => {
            let n = raw["results"].as_array().map_or(0, Vec::len);
            (
                format!("Searched files for {} · {n} results", item.str("pattern")),
                None,
            )
        }
        "web_search" => {
            let patterns: Vec<&str> = raw["patterns"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            (format!("Searched the web: {}", patterns.join(", ")), None)
        }
        "dynamic_tool" => {
            let name = raw["toolName"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| title_or(item, "Tool"));
            let detail = [&raw["input"], &raw["output"]]
                .into_iter()
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| serde_json::to_string_pretty(v).unwrap_or_default())
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            (name, non_empty(&tail(&detail)))
        }
        "subagent" => {
            let prompt = item.str("prompt").lines().next().unwrap_or("").to_string();
            (
                format!("Subagent · {prompt}"),
                raw["result"].as_str().and_then(non_empty),
            )
        }
        "compaction" => (
            "Context compacted".to_string(),
            raw["summary"].as_str().and_then(non_empty),
        ),
        "checkpoint" => {
            let n = raw["files"].as_array().map_or(0, Vec::len);
            (format!("Checkpoint · {n} files changed"), None)
        }
        "handoff" => (
            format!(
                "Handed off to {}",
                raw["toModel"].as_str().unwrap_or("another model")
            ),
            None,
        ),
        "user_input_request" => (title_or(item, "Question for you"), None),
        kind => (title_or(item, &kind.replace('_', " ")), None),
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_string())
}

fn tail(s: &str) -> String {
    match s.char_indices().rev().nth(OUTPUT_TAIL_CHARS - 1) {
        Some((i, _)) => format!("…{}", &s[i..]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTPUT_TAIL_CHARS, TurnItem, approval_options, tail, tool_parts};
    use serde_json::{Value, json};

    fn item(v: Value) -> TurnItem {
        TurnItem::from_value(&v).unwrap()
    }

    #[test]
    fn summarizes_work_items() {
        let cmd = item(
            json!({"id": "1", "ordinal": 1, "type": "command_execution", "input": "ls\n-la", "output": "a\nb", "exitCode": 2}),
        );
        let (summary, detail) = tool_parts(&cmd);
        assert_eq!(summary, "$ ls  (exit 2)");
        assert_eq!(detail.unwrap(), "$ ls\n-la\na\nb");

        let edit = item(
            json!({"id": "2", "ordinal": 2, "type": "file_change", "fileName": "a.rs", "additions": 3, "deletions": 1}),
        );
        assert_eq!(tool_parts(&edit), ("Edited a.rs  +3 −1".to_string(), None));

        let unknown = item(json!({"id": "3", "ordinal": 3, "type": "future_thing", "title": null}));
        assert_eq!(tool_parts(&unknown).0, "future thing");
    }

    #[test]
    fn approval_options_put_decline_first_and_accept_last() {
        let defaults = item(json!({"id": "a", "ordinal": 1, "type": "approval_request"}));
        let decisions: Vec<String> = approval_options(&defaults)
            .into_iter()
            .map(|o| o.0)
            .collect();
        assert_eq!(decisions, ["decline", "acceptForSession", "accept"]);
        let custom = item(
            json!({"id": "b", "ordinal": 1, "type": "approval_request", "options": [
            {"decision": "accept", "label": "Yes"}, {"decision": "cancel", "label": "Stop"}, {"decision": "decline", "label": "No"}]}),
        );
        let labels: Vec<String> = approval_options(&custom).into_iter().map(|o| o.1).collect();
        assert_eq!(labels, ["No", "Stop", "Yes"]);
    }

    #[test]
    fn tail_keeps_the_end_on_char_boundaries() {
        let long = "é".repeat(OUTPUT_TAIL_CHARS + 10);
        let cut = tail(&long);
        assert!(cut.starts_with('…'));
        assert_eq!(cut.chars().count(), OUTPUT_TAIL_CHARS + 1);
        assert_eq!(tail("short"), "short");
    }
}
