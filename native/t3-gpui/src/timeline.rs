//! Timeline rows per turn item type (web: `apps/web/src/components/chat/Timeline*.tsx`).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, ThemeColor, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::Value;

use crate::T3App;
use crate::model::TurnItem;

fn open_link(url: &SharedString, _: &ClickEvent, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(crate::editors::OpenLink { url: url.clone() }), cx);
}

/// Long tool output is cut to its tail; the full text stays on the server.
const OUTPUT_TAIL_CHARS: usize = 8_000;

impl T3App {
    pub(crate) fn render_item(
        &self,
        item: &TurnItem,
        revert_turn: Option<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let expanded = self.expanded.contains(&item.id);
        let text = item.str("text").to_string();
        match item.kind.as_str() {
            "user_message" => h_flex()
                .group("user-row")
                .justify_end()
                .gap_2()
                .when_some(revert_turn, |row, turn| {
                    let text = text.clone();
                    row.child(
                        Button::new(SharedString::from(format!("revert-{}", item.id)))
                            .xsmall()
                            .ghost()
                            .label("↺")
                            .tooltip(t("Revert to here"))
                            .invisible()
                            .group_hover("user-row", |s| s.visible())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_revert(turn, text.clone(), window, cx)
                            })),
                    )
                })
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
            "assistant_message" => crate::mermaid::with_mermaid(TextView::markdown(
                SharedString::from(item.id.clone()),
                self.resolve_images(&text, cx).into_owned(),
            ))
            .plugin(crate::thread_chips::ThreadChips(cx.weak_entity()))
            .on_link_click(open_link)
            .into_any_element(),
            "proposed_plan" => {
                let markdown = item.str("markdown").to_string();
                let (copy, save) = (markdown.clone(), markdown.clone());
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(c.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .child(crate::i18n::tc("Plan", "badge, noun")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(
                                        crate::plans::title(&markdown)
                                            .unwrap_or_else(|| t("Proposed plan")),
                                    ),
                            )
                            .child(
                                Button::new(SharedString::from(format!("plan-copy-{}", item.id)))
                                    .xsmall()
                                    .ghost()
                                    .label(t("Copy to clipboard"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                            "{}\n",
                                            copy.trim_end()
                                        )))
                                    }),
                            )
                            .child({
                                let download = save.clone();
                                Button::new(SharedString::from(format!(
                                    "plan-download-{}",
                                    item.id
                                )))
                                .xsmall()
                                .ghost()
                                .label(t("Download"))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.download_plan(download.clone(), cx),
                                ))
                            })
                            .child(
                                Button::new(SharedString::from(format!("plan-save-{}", item.id)))
                                    .xsmall()
                                    .ghost()
                                    .label(t("Save to workspace"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_save_plan(save.clone(), window, cx)
                                    })),
                            ),
                    )
                    .child(
                        crate::mermaid::with_mermaid(TextView::markdown(
                            SharedString::from(item.id.clone()),
                            self.resolve_images(&crate::plans::body(&markdown), cx)
                                .into_owned(),
                        ))
                        .on_link_click(open_link),
                    )
                    .into_any_element()
            }
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
            "secret_request" => self.render_secret_request(item, &c, cx),
            "system_notice" | "run_interrupt_request" | "run_interrupt_result" => div()
                .text_xs()
                .text_color(c.muted_foreground)
                .child(item.str("message").to_string())
                .into_any_element(),
            "subagent" => {
                let child = item.raw["childThreadId"].as_str().map(str::to_owned);
                let progress = item.raw["progress"]
                    .as_str()
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned);
                let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
                let tooltip = SharedString::from(self.subagent_tooltip(item));
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!("subagent-tip-{}", item.id)))
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                    .build(window, cx)
                            })
                            .child(self.render_tool(item, expanded, &c, mono, cx)),
                    )
                    .children(progress.map(|p| {
                        div()
                            .pl_4()
                            .text_xs()
                            .text_color(c.muted_foreground)
                            .child(p)
                    }))
                    .children(child.map(|child| {
                        div().pl_4().child(
                            Button::new(SharedString::from(format!("subagent-open-{}", item.id)))
                                .xsmall()
                                .ghost()
                                .label(t("Open thread"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_thread(env.clone(), child.clone(), cx)
                                })),
                        )
                    }))
                    .into_any_element()
            }
            _ => self.render_tool(item, expanded, &c, mono, cx),
        }
    }

    /// The subagent hover card (web: `SubagentTooltipContent`): model, status, the child's
    /// worktree when it differs from this thread's, and the latest result or progress.
    fn subagent_tooltip(&self, item: &TurnItem) -> String {
        let raw = &item.raw;
        let model = raw["executedModel"]
            .as_str()
            .or(raw["model"].as_str())
            .or(raw["modelSelection"]["model"].as_str())
            .filter(|m| !m.trim().is_empty());
        let instance = raw["providerInstanceId"].as_str().unwrap_or("");
        let model = model.map_or_else(
            || t("Not reported").to_string(),
            |m| {
                self.config()
                    .label(&serde_json::json!({ "instanceId": instance, "model": m }))
            },
        );
        let status = item.str("status").replace('_', " ");
        let shell = self.env().map(|e| &e.shell);
        let worktree = |id: Option<&str>| {
            let id = id?;
            shell?
                .threads
                .iter()
                .find(|t| t.id == id)?
                .worktree_path
                .clone()
        };
        let child = worktree(raw["childThreadId"].as_str())
            .filter(|w| Some(w) != worktree(self.selected.as_deref()).as_ref());
        let preview = subagent_preview(
            item.str("status"),
            raw["result"].as_str(),
            raw["progress"].as_str(),
        );
        [
            Some(model),
            Some(t(&status).to_string()),
            child.map(|w| w.rsplit('/').next().unwrap_or(&w).to_owned()),
            preview,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// A folded run of work items; expands to the individual rows.
    pub(crate) fn render_group(
        &self,
        id: String,
        items: &[&TurnItem],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let expanded = self.expanded.contains(&id);
        let failed = items
            .iter()
            .rev()
            .find(|i| i.kind != "reasoning")
            .is_some_and(|i| i.str("status") == "failed");
        let running = items
            .iter()
            .any(|i| matches!(i.str("status"), "running" | "pending" | "waiting"));
        let mut col = v_flex().gap_1().child(
            h_flex()
                .id(SharedString::from(id.clone()))
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .text_color(if failed { c.danger } else { c.muted_foreground })
                .hover(|r| r.text_color(c.foreground))
                .child(if expanded { "▾" } else { "▸" })
                .child(icon(
                    group_icon(items),
                    if failed { c.danger } else { c.muted_foreground },
                ))
                .child(summarize_group(items))
                .when(running, |r| r.child(div().text_color(c.info).child("●")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded.remove(&id) {
                        this.expanded.insert(id.clone());
                    }
                    cx.notify();
                })),
        );
        if expanded {
            for item in items {
                col = col.child(div().pl_4().child(self.render_item(item, None, cx)));
            }
        }
        col.into_any_element()
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
                .label(t("Submit"))
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
            .label(t("Dismiss"))
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
                    .child(t("Or type an answer in the composer.")),
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
        let glyph_color = match item.str("status") {
            "running" | "pending" | "waiting" => c.info,
            "failed" => c.danger,
            _ => c.muted_foreground,
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
                    .child(icon(tool_icon(item), glyph_color))
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
        // The web's defaults; Decline and Approve carry its `$context`.
        let verb = "verb, response to an approval request";
        options = vec![
            ("decline".into(), crate::i18n::tc("Decline", verb)),
            ("acceptForSession".into(), t("Always allow this session")),
            ("accept".into(), crate::i18n::tc("Approve", verb)),
        ];
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

/// `subagentDetailPreview`: the result once finished, else the progress, whitespace collapsed
/// and cut at 280 characters.
pub(crate) fn subagent_preview(
    status: &str,
    result: Option<&str>,
    progress: Option<&str>,
) -> Option<String> {
    let done = matches!(status, "completed" | "failed" | "cancelled" | "interrupted");
    let (preferred, fallback) = if done {
        (result, progress)
    } else {
        (progress, result)
    };
    let detail = [preferred, fallback]
        .into_iter()
        .flatten()
        .find(|d| !d.trim().is_empty())?;
    let compact = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(match compact.char_indices().nth(280) {
        Some((cut, _)) => format!("{}…", compact[..cut].trim_end()),
        None => compact,
    })
}

/// `workEntryIconName`: the lucide icon for one work row.
/// shortcut: built-in icons only; the web also shows MCP favicons and provider logos.
pub(crate) fn tool_icon(item: &TurnItem) -> &'static str {
    match item.kind.as_str() {
        "reasoning" => "brain",
        "command_execution" => "terminal",
        "file_change" => "square-pen",
        "file_search" => "search",
        "web_search" => "globe",
        "dynamic_tool" => "hammer",
        "subagent" => "bot",
        "todo_list" => "list-todo",
        "user_input_request" | "approval_request" => "message-circle",
        "system_notice" if item.raw["level"] == "info" => "zap",
        "system_notice" => "circle-alert",
        "notification" if item.str("outcome") == "failed" => "circle-alert",
        "notification" => "zap",
        _ => "wrench",
    }
}

/// `toolGroupSummaryIconName`: one action's icon, a hammer for mixed groups, a brain for thoughts.
pub(crate) fn group_icon(items: &[&TurnItem]) -> &'static str {
    let mut icons = items
        .iter()
        .filter(|i| i.kind != "reasoning")
        .map(|i| tool_icon(i));
    match icons.next() {
        None => "brain",
        Some(first) if icons.all(|i| i == first) => first,
        Some(_) => "hammer",
    }
}

fn icon(name: &str, color: Hsla) -> Icon {
    Icon::empty()
        .path(format!("icons/{name}.svg"))
        .size_3p5()
        .text_color(color)
}

/// Items the web folds into a work group: tool activity and reasoning.
fn is_work(item: &TurnItem) -> bool {
    matches!(
        item.kind.as_str(),
        "reasoning"
            | "command_execution"
            | "file_change"
            | "file_search"
            | "web_search"
            | "dynamic_tool"
    )
}

pub(crate) enum Row<'a> {
    Item(&'a TurnItem),
    /// Two or more consecutive work items, shown as one toggle row.
    Group {
        id: String,
        items: Vec<&'a TurnItem>,
    },
}

pub(crate) fn group_rows<'a>(items: impl Iterator<Item = &'a TurnItem>) -> Vec<Row<'a>> {
    let mut rows: Vec<Row<'a>> = Vec::new();
    let mut run: Vec<&TurnItem> = Vec::new();
    let flush = |run: &mut Vec<&'a TurnItem>, rows: &mut Vec<Row<'a>>| match run.len() {
        0 => {}
        1 => rows.push(Row::Item(run.pop().unwrap())),
        _ => rows.push(Row::Group {
            id: format!("work-toggle:{}", run[0].id),
            items: std::mem::take(run),
        }),
    };
    for item in items {
        if is_work(item) {
            run.push(item);
        } else {
            flush(&mut run, &mut rows);
            rows.push(Row::Item(item));
        }
    }
    flush(&mut run, &mut rows);
    rows
}

impl Row<'_> {
    fn key(&self) -> &str {
        match self {
            Row::Item(item) => &item.id,
            Row::Group { id, .. } => id,
        }
    }
}

/// The list splice that turns rows `old` into `new`: the range between their common prefix and
/// suffix, and how many rows replace it. Older history lands before the rows in view, so the
/// list keeps its scroll anchor on them.
pub(crate) fn row_splice(
    old: &[String],
    new: &[String],
) -> Option<(std::ops::Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

impl T3App {
    /// Tells the transcript list which rows changed. A new thread starts over, following the tail.
    /// Heights of rows in view are measured every layout, so only row identity is tracked here.
    pub(crate) fn sync_transcript(&mut self) {
        let keys: Vec<String> = group_rows(self.thread.items())
            .iter()
            .map(|r| r.key().to_owned())
            .collect();
        let (owner, old) = &mut self.transcript_rows;
        if *owner != self.selected {
            *owner = self.selected.clone();
            self.transcript.reset(keys.len() + 1);
            self.transcript.set_follow_mode(FollowMode::Tail);
        } else if let Some((range, count)) = row_splice(old, &keys) {
            self.transcript.splice(range, count);
        }
        *old = keys;
    }

    /// One transcript list item: row `i`, or past the last row, the working indicator.
    pub(crate) fn render_transcript_row(
        &self,
        i: usize,
        revert_turns: &std::collections::HashMap<String, u64>,
        matches: &[usize],
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let frame = div().w_full().max_w(px(768.)).px_4().py_2();
        let rows = group_rows(self.thread.items());
        let Some(row) = rows.get(i) else {
            let thread = self
                .selected
                .as_ref()
                .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id));
            return match thread.filter(|t| t.is_running()).and_then(|t| {
                t.activity_run_started_at
                    .clone()
                    .or_else(|| t.latest_run_started_at.clone())
            }) {
                Some(started) => frame
                    .child(self.render_working(&started, cx))
                    .into_any_element(),
                None => div().into_any_element(),
            };
        };
        let element = match row {
            Row::Item(item) => self.render_item(item, revert_turns.get(&item.id).copied(), cx),
            Row::Group { id, items } => self.render_group(id.clone(), items, cx),
        };
        frame
            .when(matches.contains(&i), |d| {
                d.border_l_2().border_color(theme.warning)
            })
            .when(current == Some(i), |d| d.bg(theme.secondary))
            .child(element)
            .into_any_element()
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {n} {}",
        one.split(' ').next().unwrap(),
        if n == 1 {
            &one[one.find(' ').unwrap() + 1..]
        } else {
            many
        }
    )
}

/// `summarizeToolGroup` (client-runtime/src/work-log/presentation.ts) for the core actions.
pub(crate) fn summarize_group(items: &[&TurnItem]) -> String {
    let tools: Vec<&&TurnItem> = items.iter().filter(|i| i.kind != "reasoning").collect();
    if tools.is_empty() {
        return if items.len() == 1 {
            "Thought".into()
        } else {
            format!("Thought (×{})", items.len())
        };
    }
    // (action, priority, first index, count)
    let mut groups: Vec<(&str, u8, usize, usize)> = Vec::new();
    let mut edited = std::collections::HashSet::new();
    for (index, item) in tools.iter().enumerate() {
        let (action, priority) = match item.kind.as_str() {
            "command_execution" => ("command", 0),
            "file_change" => ("edit", 0),
            "file_search" => ("code-search", 1),
            "web_search" => ("search", 1),
            _ => ("other", 2),
        };
        if action == "edit" && !edited.insert(item.str("fileName").to_owned()) {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == action) {
            Some(g) => g.3 += 1,
            None => groups.push((action, priority, index, 1)),
        }
    }
    let mut selected = groups.clone();
    selected.sort_by_key(|g| (g.1, g.2));
    selected.truncate(2);
    selected.sort_by_key(|g| g.2);
    let mut labels: Vec<String> = selected
        .iter()
        .map(|&(action, _, _, n)| match action {
            "command" => plural(n, "Ran command", "commands"),
            "edit" => plural(n, "Changed file", "files"),
            "code-search" => format!(
                "Searched code {n} {}",
                if n == 1 { "time" } else { "times" }
            ),
            "search" => format!(
                "Searched the web {n} {}",
                if n == 1 { "time" } else { "times" }
            ),
            _ => plural(n, "Used tool", "tools"),
        })
        .collect();
    let shown: usize = selected.iter().map(|g| g.3).sum();
    let all: usize = groups.iter().map(|g| g.3).sum();
    if all > shown {
        labels.push(plural(
            all - shown,
            "Performed other action",
            "other actions",
        ));
    }
    let labels: Vec<String> = labels
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l
            } else {
                l[..1].to_lowercase() + &l[1..]
            }
        })
        .collect();
    match labels.len() {
        0 => String::new(),
        1 => labels[0].clone(),
        2 => format!("{} and {}", labels[0], labels[1]),
        n => format!("{}, and {}", labels[..n - 1].join(", "), labels[n - 1]),
    }
}

/// `formatWorkingTimerNow`: seconds under a minute, then `formatDuration` ("1m 3s", "1h 2m 3s").
/// shortcut: uses the client clock; the web corrects by the server clock offset.
pub(crate) fn working_timer(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    match (h, m, s) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, 0, s) => format!("{h}h {s}s"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

impl T3App {
    /// "Working for 1m 3s" under a running thread; re-renders once a second while shown.
    pub(crate) fn render_working(&self, started: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let elapsed = started
            .parse::<jiff::Timestamp>()
            .map(|t| (jiff::Timestamp::now().as_second() - t.as_second()).max(0) as u64)
            .unwrap_or(0);
        if !self.working_tick.replace(true) {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                this.update(cx, |app, cx| {
                    app.working_tick.set(false);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        h_flex()
            .gap_2()
            .text_sm()
            .text_color(c.muted_foreground)
            .child(div().text_color(c.info).child("●"))
            .child(crate::i18n::tv(
                "Working for {time}",
                &[("time", &working_timer(elapsed))],
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OUTPUT_TAIL_CHARS, Row, TurnItem, approval_options, group_rows, row_splice,
        summarize_group, tail, tool_parts, working_timer,
    };
    use serde_json::{Value, json};

    fn item(v: Value) -> TurnItem {
        TurnItem::from_value(&v).unwrap()
    }

    #[test]
    fn subagent_preview_prefers_result_when_done() {
        use super::subagent_preview;
        assert_eq!(
            subagent_preview("completed", Some("all  done\n"), Some("x")).as_deref(),
            Some("all done")
        );
        assert_eq!(
            subagent_preview("running", Some("r"), Some(" step 2 ")).as_deref(),
            Some("step 2")
        );
        assert_eq!(subagent_preview("running", None, Some("  ")), None);
        let long = subagent_preview("completed", Some(&"a ".repeat(300)), None).unwrap();
        assert!(long.ends_with('…') && long.chars().count() <= 281);
    }

    #[test]
    fn work_rows_and_groups_get_the_web_icons() {
        let cmd = item(json!({"id": "1", "ordinal": 1, "type": "command_execution"}));
        let edit = item(json!({"id": "2", "ordinal": 2, "type": "file_change"}));
        let think = item(json!({"id": "3", "ordinal": 3, "type": "reasoning"}));
        assert_eq!(super::tool_icon(&cmd), "terminal");
        assert_eq!(super::group_icon(&[&think, &cmd, &cmd]), "terminal");
        assert_eq!(super::group_icon(&[&cmd, &edit]), "hammer");
        assert_eq!(super::group_icon(&[&think]), "brain");
    }

    #[test]
    fn splices_only_changed_rows() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(row_splice(&k(&["a", "b"]), &k(&["a", "b"])), None);
        assert_eq!(
            row_splice(&k(&["c", "d"]), &k(&["a", "b", "c", "d"])),
            Some((0..0, 2)),
            "older history is inserted before the rows in view"
        );
        assert_eq!(row_splice(&k(&["a"]), &k(&["a", "b"])), Some((1..1, 1)));
        assert_eq!(
            row_splice(&k(&["a", "b", "c"]), &k(&["a", "x", "c"])),
            Some((1..2, 1))
        );
        assert_eq!(row_splice(&k(&["a", "a"]), &k(&["a"])), Some((1..2, 0)));
        assert_eq!(row_splice(&k(&["a"]), &k(&[])), Some((0..1, 0)));
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

    #[test]
    fn groups_consecutive_work_and_summarizes_like_the_web() {
        let mk = |id: &str, kind: &str, extra: Value| {
            let mut v = json!({"id": id, "ordinal": 1, "type": kind});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            item(v)
        };
        let items = [
            mk("u", "user_message", json!({})),
            mk("r", "reasoning", json!({})),
            mk("c1", "command_execution", json!({})),
            mk("e1", "file_change", json!({"fileName": "a.rs"})),
            mk("e2", "file_change", json!({"fileName": "a.rs"})),
            mk("e3", "file_change", json!({"fileName": "b.rs"})),
            mk("c2", "command_execution", json!({})),
            mk("t", "dynamic_tool", json!({})),
            mk("a", "assistant_message", json!({})),
            mk("solo", "command_execution", json!({})),
        ];
        let rows = group_rows(items.iter());
        assert_eq!(rows.len(), 4);
        let Row::Group { items: group, .. } = &rows[1] else {
            panic!("expected a group")
        };
        assert_eq!(group.len(), 7);
        assert_eq!(
            summarize_group(group),
            "Ran 2 commands, changed 2 files, and performed 1 other action"
        );
        assert!(
            matches!(rows[3], Row::Item(i) if i.id == "solo"),
            "a lone tool keeps its own row"
        );
        let thoughts = [
            mk("r1", "reasoning", json!({})),
            mk("r2", "reasoning", json!({})),
        ];
        assert_eq!(
            summarize_group(&thoughts.iter().collect::<Vec<_>>()),
            "Thought (×2)"
        );
    }

    #[test]
    fn formats_the_working_timer_like_the_web() {
        let out: Vec<String> = [0, 59, 63, 3600, 3605, 3723]
            .into_iter()
            .map(working_timer)
            .collect();
        assert_eq!(out, ["0s", "59s", "1m 3s", "1h", "1h 5s", "1h 2m 3s"]);
    }
}
