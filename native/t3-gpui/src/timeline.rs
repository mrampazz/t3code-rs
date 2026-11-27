//! Timeline rows per turn item type (web: `apps/web/src/components/chat/Timeline*.tsx`).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, ThemeColor, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::T3App;
use crate::model::TurnItem;

fn open_link(url: &SharedString, _: &ClickEvent, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(crate::editors::OpenLink { url: url.clone() }), cx);
}

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
            _ if crate::work_rows::is_work_row(item) => self.render_work_row(item, true, false, cx),
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
            .style(crate::ui::markdown_style(cx))
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
                        .style(crate::ui::markdown_style(cx))
                        .on_link_click(open_link),
                    )
                    .into_any_element()
            }
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

    /// A folded turn's header (web: `MessagesTimeline` work summary): label and chevron, the
    /// answer's time, and a rule under it.
    fn render_fold(
        &self,
        id: String,
        label: &str,
        time: &str,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let group: SharedString = format!("fold-row-{id}").into();
        h_flex()
            .group(group.clone())
            .pt_1()
            .pb_2()
            .pr(px(2.))
            .gap_1()
            .border_b_1()
            .border_color(c.border.opacity(0.6))
            .child(
                h_flex()
                    .id(SharedString::from(id.clone()))
                    .px_1()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .line_height(relative(1.625))
                    .text_color(c.muted_foreground)
                    .hover(|s| s.text_color(c.foreground))
                    .child(label.to_owned())
                    .child(
                        crate::ui::icon(if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        })
                        .size(px(14.)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id.clone());
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .ml_auto()
                    .mr_1()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .child(time.to_owned()),
            )
            .into_any_element()
    }

    /// The thread's transcript rows as the list shows them.
    pub(crate) fn rows(&self) -> Vec<Row<'_>> {
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .filter(|t| t.is_running());
        let live = thread.map(|t| LiveTurn {
            run: t.active_run_id.as_deref(),
            started: t
                .activity_run_started_at
                .clone()
                .or_else(|| t.latest_run_started_at.clone()),
            turn_ended: self.run_activity().turn_ended,
        });
        timeline_rows(self.thread.items(), &self.expanded, live.as_ref())
    }

    /// The open thread's unsettled run while it works.
    fn live_run(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .filter(|t| t.is_running())
            .and_then(|t| t.active_run_id.as_deref())
    }

    /// The open thread's active run (`summarizeThreadRun`): provider starting, turn ended.
    pub(crate) fn run_activity(&self) -> crate::model::RunActivity {
        let drivers: Vec<(String, String)> = self
            .config()
            .providers
            .iter()
            .map(|p| (p.instance_id.clone(), p.driver.clone()))
            .collect();
        self.thread
            .run_activity(self.selected.as_deref().unwrap_or(""), &drivers)
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
                    .child(icon(crate::work_rows::icon_name(item), glyph_color))
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
    /// A finished turn's work (web: "Worked for 5.3s"); when `open`, its rows follow it.
    Fold {
        id: String,
        label: String,
        time: String,
        open: bool,
        items: Vec<&'a TurnItem>,
    },
    /// "Working for 12s", right after the prompt of the turn that is running.
    Working {
        started: Option<String>,
    },
    /// The running turn's latest tool call standing in for its work so far (web: `work-live`);
    /// open, the work shows under it.
    Live {
        id: String,
        group: String,
        entry: &'a TurnItem,
        /// The newest thought with text, shown above while the group is closed.
        thought: Option<&'a TurnItem>,
        items: Vec<&'a TurnItem>,
        /// The latest call still runs, or succeeded with nothing waiting on the user.
        active: bool,
    },
    /// "Thinking" at the end of a running turn with nothing else live; after a failed call it
    /// opens to that work.
    Thinking {
        group: Option<(String, Vec<&'a TurnItem>)>,
        label: Option<String>,
    },
    /// Thumbnails of the images a work group opened, under it.
    Images {
        id: String,
        paths: Vec<String>,
    },
}

/// The running turn as the timeline needs it (`isWorking`, the unsettled run).
pub(crate) struct LiveTurn<'a> {
    pub run: Option<&'a str>,
    pub started: Option<String>,
    pub turn_ended: bool,
}

/// `workEntryIsVisibleInGroup`: thoughts with text, and calls that failed, finished or stopped
/// (`running` also shows the ones still going).
fn visible_in_group(item: &TurnItem, running: bool) -> bool {
    if item.kind == "reasoning" {
        return !item.str("text").trim().is_empty();
    }
    let status = item.str("status");
    let in_progress = matches!(status, "pending" | "running" | "waiting");
    let stopped = matches!(status, "cancelled" | "interrupted");
    let neutral = !crate::work_rows::failed(item) && (in_progress || stopped || status == "idle");
    (running && in_progress) || stopped || !neutral
}

impl Row<'_> {
    fn key(&self) -> &str {
        match self {
            Row::Item(item) => &item.id,
            Row::Group { id, .. }
            | Row::Fold { id, .. }
            | Row::Live { id, .. }
            | Row::Images { id, .. } => id,
            Row::Working { .. } => "working-indicator-row",
            Row::Thinking { .. } => LIVE_ACTIVITY_ROW_ID,
        }
    }

    /// Work log rows sit flush against each other (web: `continuesWorkLog`).
    fn is_work_log(&self) -> bool {
        match self {
            Row::Item(item) => crate::work_rows::is_work_row(item),
            Row::Group { .. } | Row::Live { .. } | Row::Thinking { .. } => true,
            Row::Fold { .. } | Row::Working { .. } | Row::Images { .. } => false,
        }
    }
}

const LIVE_ACTIVITY_ROW_ID: &str = "live-activity-row";

/// Kinds a finished turn folds away before its answer (web: `deriveTurnFolds`); approvals,
/// questions, plans, subagents and errors stay in view.
fn foldable(item: &TurnItem) -> bool {
    matches!(
        item.kind.as_str(),
        "reasoning"
            | "command_execution"
            | "file_change"
            | "file_search"
            | "web_search"
            | "dynamic_tool"
            | "assistant_message"
            | "todo_list"
            | "system_notice"
    ) && item.str("status") != "failed"
}

/// `formatDuration` (`@t3tools/shared/orchestrationTiming`).
pub(crate) fn format_duration(ms: i64) -> String {
    if ms < 1_000 {
        return format!("{}ms", ms.max(1));
    }
    if ms < 10_000 {
        return format!("{}s", (ms as f64 / 100.).round() / 10.);
    }
    let total = (ms as f64 / 1_000.).round() as i64;
    if total < 60 {
        return format!("{total}s");
    }
    let (h, m, sec) = (total / 3_600, total % 3_600 / 60, total % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if sec > 0 || m > 0 {
        parts.push(format!("{sec}s"));
    }
    parts.join(" ")
}

/// The `timestampFormat` client setting: `locale`, `12-hour` or `24-hour`.
static TIMESTAMP_FORMAT: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Follows a saved change to the `timestampFormat` preference.
pub(crate) fn set_timestamp_format(prefs: &serde_json::Value) {
    *TIMESTAMP_FORMAT.write().unwrap() = Some(
        prefs["timestampFormat"]
            .as_str()
            .unwrap_or("locale")
            .to_owned(),
    );
}

fn timestamp_format() -> String {
    if let Some(format) = TIMESTAMP_FORMAT.read().unwrap().clone() {
        return format;
    }
    set_timestamp_format(&crate::settings::load_prefs());
    timestamp_format()
}

/// Formats `at` in the local zone from an ICU skeleton (`jmm`, `Md`, `yMd`) the way the
/// system's locale lays it out, as the web's `Intl.DateTimeFormat` does.
#[cfg(target_os = "macos")]
fn system_format(at: jiff::Timestamp, skeleton: &str) -> String {
    use objc2_foundation::{NSDate, NSDateFormatter, NSString};
    let formatter = NSDateFormatter::new();
    formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str(skeleton));
    let date = NSDate::dateWithTimeIntervalSince1970(at.as_millisecond() as f64 / 1_000.);
    formatter.stringFromDate(&date).to_string()
}

#[cfg(not(target_os = "macos"))]
fn system_format(at: jiff::Timestamp, skeleton: &str) -> String {
    let local = at.to_zoned(jiff::tz::TimeZone::system());
    let pattern = match skeleton {
        "Hmm" => "%H:%M",
        "Md" => "%-m/%-d",
        "yMd" => "%-m/%-d/%Y",
        _ => "%-I:%M %p",
    };
    local.strftime(pattern).to_string()
}

/// `formatDayAwareTimestamp`: the time today, "yesterday at …", else the numeric date and time,
/// in the system locale and the `timestampFormat` hour cycle.
pub(crate) fn day_aware_time(at: jiff::Timestamp) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (local, today) = (at.to_zoned(tz.clone()), jiff::Timestamp::now().to_zoned(tz));
    let skeleton = match timestamp_format().as_str() {
        "12-hour" => "hmm",
        "24-hour" => "Hmm",
        _ => "jmm",
    };
    let time = system_format(at, skeleton);
    let days = (today.date() - local.date()).get_days();
    match days {
        ..=0 => time,
        1 => crate::i18n::tv("yesterday at {time}", &[("time", &time)]),
        _ if local.year() == today.year() => format!("{} {time}", system_format(at, "Md")),
        _ => format!("{} {time}", system_format(at, "yMd")),
    }
}

/// `formatUpcomingTimestamp`: a time today, "tomorrow at …", else the date and time.
pub(crate) fn upcoming_time(at: jiff::Timestamp) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (local, today) = (at.to_zoned(tz.clone()), jiff::Timestamp::now().to_zoned(tz));
    let days = (local.date() - today.date()).get_days();
    if days < 0 {
        return day_aware_time(at);
    }
    let skeleton = match timestamp_format().as_str() {
        "12-hour" => "hmm",
        "24-hour" => "Hmm",
        _ => "jmm",
    };
    let time = system_format(at, skeleton);
    match days {
        0 => time,
        1 => crate::i18n::tv("tomorrow at {time}", &[("time", &time)]),
        _ if local.year() == today.year() => format!("{} {time}", system_format(at, "Md")),
        _ => format!("{} {time}", system_format(at, "yMd")),
    }
}

/// A work row's time (`projectedItemCreatedAt`): when it started, else when it last changed.
pub(crate) fn row_time(item: &TurnItem) -> String {
    timestamp(item, "startedAt")
        .or_else(|| timestamp(item, "updatedAt"))
        .map(day_aware_time)
        .unwrap_or_default()
}

fn timestamp(item: &TurnItem, key: &str) -> Option<jiff::Timestamp> {
    item.str(key).parse().ok()
}

/// The transcript's rows: consecutive work of one run grouped, and each finished turn's work before
/// its final answer folded into one "Worked for …" row that expands in place.
pub(crate) fn timeline_rows<'a>(
    items: impl Iterator<Item = &'a TurnItem>,
    expanded: &std::collections::HashSet<String>,
    live: Option<&LiveTurn>,
) -> Vec<Row<'a>> {
    use std::collections::{HashMap, HashSet};
    let items: Vec<&TurnItem> = items
        .filter(|i| !crate::work_rows::hidden_from_timeline(i))
        .collect();
    let active_run = live.and_then(|l| l.run);
    let mut terminal: HashMap<&str, usize> = HashMap::new();
    let mut prompt: HashMap<&str, &TurnItem> = HashMap::new();
    let mut interrupted: HashSet<&str> = HashSet::new();
    for (i, item) in items.iter().enumerate() {
        let Some(run) = item.run_id.as_deref() else {
            continue;
        };
        match item.kind.as_str() {
            "assistant_message" => {
                terminal.insert(run, i);
            }
            "user_message" => {
                prompt.entry(run).or_insert(item);
            }
            "run_interrupt_request" | "run_interrupt_result" => {
                interrupted.insert(run);
            }
            _ => {}
        }
    }
    let folds = |i: usize, item: &TurnItem| {
        item.run_id.as_deref().is_some_and(|run| {
            Some(run) != active_run
                && !interrupted.contains(run)
                && terminal.get(run).is_some_and(|&end| i < end)
                && foldable(item)
        })
    };

    // The running turn: its prompt (`lastResponseBoundaryIndex`), and the work at its tail.
    let header = match live {
        Some(_) => items
            .iter()
            .rposition(|i| {
                (i.kind == "user_message"
                    && !matches!(i.str("inputIntent"), "steer" | "promoted_queued_to_steer"))
                    || i.kind == "notification"
            })
            .map_or(0, |p| p + 1),
        None => items.len(),
    };
    let pending_approval = items
        .iter()
        .any(|i| i.kind == "approval_request" && is_open(i));
    let mut tail: Vec<&TurnItem> = Vec::new();
    if live.is_some() && active_run.is_some() {
        for (i, item) in items.iter().enumerate().skip(header).rev() {
            if !is_work(item) || item.run_id.as_deref() != active_run || folds(i, item) {
                break;
            }
            tail.push(item);
        }
        tail.reverse();
    }
    let visible: Vec<&TurnItem> = tail
        .iter()
        .copied()
        .filter(|i| visible_in_group(i, true))
        .collect();
    let running = |i: &&&TurnItem| matches!(i.str("status"), "pending" | "running" | "waiting");
    let latest_running = visible.iter().rfind(running).copied();
    let latest = visible.last().copied();
    let succeeded = latest.is_some_and(|i| {
        !crate::work_rows::failed(i) && matches!(i.str("status"), "completed" | "")
    });
    let keeps_live = latest_running.is_some() || (succeeded && !pending_approval);
    let latest_failed = latest_running.is_none()
        && latest.is_some_and(|i| i.str("status") != "declined" && crate::work_rows::failed(i));
    let anchor = tail.first().copied();
    let group_id = anchor.map(|a| format!("work-toggle:{}", a.id));
    let mut live_row = match (anchor, latest, &group_id) {
        (Some(anchor), Some(latest), Some(group)) if !latest_failed => Some(Row::Live {
            id: if keeps_live {
                LIVE_ACTIVITY_ROW_ID.to_owned()
            } else {
                format!("work-live:{}", anchor.id)
            },
            group: group.clone(),
            entry: latest_running.unwrap_or(latest),
            thought: visible
                .iter()
                .rfind(|i| i.kind == "reasoning" && !i.str("text").trim().is_empty())
                .copied(),
            items: visible.clone(),
            active: keeps_live,
        }),
        _ => None,
    };
    let has_live = live_row.is_some();
    let placement = latest.map(|i| i.id.as_str());
    let taken: HashSet<&str> = if has_live || latest_failed {
        tail.iter().map(|i| i.id.as_str()).collect()
    } else {
        HashSet::new()
    };

    let mut rows = Vec::new();
    let mut working_pushed = false;
    let mut run: Vec<&TurnItem> = Vec::new();
    let flush = |run: &mut Vec<&'a TurnItem>, rows: &mut Vec<Row<'a>>| {
        let visible: Vec<&'a TurnItem> = run
            .drain(..)
            .filter(|i| visible_in_group(i, false))
            .collect();
        match visible.len() {
            0 => {}
            1 => rows.push(Row::Item(visible[0])),
            _ => {
                let id = format!("work-toggle:{}", visible[0].id);
                let paths = crate::work_rows::viewed_image_paths(&visible);
                rows.push(Row::Group {
                    id: id.clone(),
                    items: visible,
                });
                if !paths.is_empty() {
                    rows.push(Row::Images {
                        id: format!("{id}:viewed-images"),
                        paths,
                    });
                }
            }
        }
    };
    let mut i = 0;
    while i < items.len() {
        if live.is_some() && i == header && !working_pushed {
            flush(&mut run, &mut rows);
            rows.push(Row::Working {
                started: live.and_then(|l| l.started.clone()),
            });
            working_pushed = true;
        }
        let fold = folds(i, items[i]);
        if !fold {
            let item = items[i];
            if Some(item.id.as_str()) == placement
                && let Some(row) = live_row.take()
            {
                flush(&mut run, &mut rows);
                let images = match &row {
                    Row::Live { group, items, .. } => Some((
                        format!("{group}:viewed-images"),
                        crate::work_rows::viewed_image_paths(items),
                    )),
                    _ => None,
                };
                rows.push(row);
                if let Some((id, paths)) = images.filter(|(_, p)| !p.is_empty()) {
                    rows.push(Row::Images { id, paths });
                }
            }
            if !taken.contains(item.id.as_str()) {
                let continues = run.last().is_none_or(|last| last.run_id == item.run_id);
                if is_work(item) && continues {
                    run.push(item);
                } else {
                    flush(&mut run, &mut rows);
                    if is_work(item) {
                        run.push(item);
                    } else {
                        rows.push(Row::Item(item));
                    }
                }
            }
            i += 1;
            continue;
        }
        flush(&mut run, &mut rows);
        let start = i;
        while i < items.len() && folds(i, items[i]) && items[i].run_id == items[start].run_id {
            i += 1;
        }
        let chunk = &items[start..i];
        let run_id = chunk[0].run_id.as_deref().unwrap_or_default();
        let answer = items[terminal[run_id]];
        let started = prompt
            .get(run_id)
            .and_then(|p| timestamp(p, "startedAt").or_else(|| timestamp(p, "updatedAt")))
            .or_else(|| timestamp(chunk[0], "startedAt"));
        let ended = timestamp(answer, "completedAt").or_else(|| timestamp(answer, "updatedAt"));
        let label = match started.zip(ended) {
            Some((a, b)) => crate::i18n::tv(
                "Worked for {duration}",
                &[(
                    "duration",
                    &format_duration(b.duration_since(a).as_millis() as i64),
                )],
            ),
            None => crate::i18n::t("Worked"),
        };
        let time = ended.map(day_aware_time).unwrap_or_default();
        let id = format!("fold:{}", chunk[0].id);
        let open = expanded.contains(&id);
        rows.push(Row::Fold {
            id,
            label,
            time,
            open,
            items: chunk.to_vec(),
        });
        if open {
            let mut inner = Vec::new();
            for item in chunk {
                if is_work(item) && inner_run_continues(&inner, item) {
                    inner.push(*item);
                } else {
                    flush(&mut inner, &mut rows);
                    if is_work(item) {
                        inner.push(*item);
                    } else {
                        rows.push(Row::Item(item));
                    }
                }
            }
            flush(&mut inner, &mut rows);
        }
    }
    flush(&mut run, &mut rows);
    let Some(live) = live else {
        return rows;
    };
    if !working_pushed {
        rows.push(Row::Working {
            started: live.started.clone(),
        });
    }
    let reply_streaming = items.last().is_some_and(|i| {
        i.kind == "assistant_message"
            && (i.raw["streaming"] == true || i.str("status") == "running")
    });
    if !live.turn_ended && !reply_streaming && (latest_failed || (!has_live && !pending_approval)) {
        let group = latest_failed
            .then(|| group_id.clone().map(|id| (id, visible.clone())))
            .flatten();
        // `runningSubagentsAtRunTail`: the run's finished reply parks on children still running.
        let waiting = active_run.map_or(0, |run| {
            items
                .iter()
                .rev()
                .filter(|i| {
                    !(i.kind == "assistant_message"
                        && i.raw["streaming"] != true
                        && i.run_id.as_deref() == Some(run))
                })
                .take_while(|i| i.kind == "subagent" && i.run_id.as_deref() == Some(run))
                .filter(|i| matches!(i.str("status"), "pending" | "running" | "waiting"))
                .count()
        });
        let images = group.as_ref().map(|(id, items)| {
            (
                format!("{id}:viewed-images"),
                crate::work_rows::viewed_image_paths(items),
            )
        });
        rows.push(Row::Thinking {
            label: (waiting > 0 && group.is_none()).then(|| {
                crate::i18n::tp(
                    "{count, plural, =1 {Waiting on a subagent} other {Waiting on {count} subagents}}",
                    waiting,
                    &[],
                )
            }),
            group,
        });
        if let Some((id, paths)) = images.filter(|(_, p)| !p.is_empty()) {
            rows.push(Row::Images { id, paths });
        }
    }
    rows
}

fn inner_run_continues(run: &[&TurnItem], item: &TurnItem) -> bool {
    run.last().is_none_or(|last| last.run_id == item.run_id)
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
        let keys: Vec<String> = self.rows().iter().map(|r| r.key().to_owned()).collect();
        let (owner, old) = &mut self.transcript_rows;
        if *owner != self.selected {
            *owner = self.selected.clone();
            self.transcript.reset(keys.len());
            self.transcript.set_follow_mode(FollowMode::Tail);
        } else if let Some((range, count)) = row_splice(old, &keys) {
            self.transcript.splice(range, count);
        }
        *old = keys;
    }

    /// One transcript list item: row `i`, spaced like the web's rows (`TimelineRowContent`).
    pub(crate) fn render_transcript_row(
        &self,
        i: usize,
        revert_turns: &std::collections::HashMap<String, u64>,
        matches: &[usize],
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().colors;
        let rows = self.rows();
        let Some(row) = rows.get(i) else {
            return div().into_any_element();
        };
        let next = rows.get(i + 1);
        let expanded_group = match row {
            Row::Group { id, .. } => self.expanded.contains(id),
            Row::Live { group, .. } => self.expanded.contains(group),
            Row::Thinking {
                group: Some((id, _)),
                ..
            } => self.expanded.contains(id),
            _ => false,
        };
        let bottom = match row {
            _ if row.is_work_log() && expanded_group => 4.,
            _ if row.is_work_log() && next.is_some_and(Row::is_work_log) => 0.,
            _ if row.is_work_log() => 8.,
            Row::Fold { .. } | Row::Working { .. } => 6.,
            // `showAssistantMeta`: a turn's last answer, once its run settled, gets more room.
            Row::Item(item) if item.kind == "assistant_message" => {
                let settled = item.run_id.as_deref() != self.live_run();
                let last = !rows[i + 1..].iter().any(|r| {
                    matches!(r, Row::Item(next) if next.kind == "assistant_message"
                        && next.run_id == item.run_id)
                });
                if settled && last { 16. } else { 8. }
            }
            Row::Item(item) if item.kind == "user_message" => {
                let grouped =
                    next.is_some_and(|n| matches!(n, Row::Item(u) if u.kind == "user_message"));
                if grouped { 4. } else { 16. }
            }
            _ => 8.,
        };
        let frame = div().w_full().max_w(px(768.)).px_4().pb(px(bottom));
        let element = match row {
            Row::Item(item) => self.render_item(item, revert_turns.get(&item.id).copied(), cx),
            Row::Group { id, items } => self.render_work_group(id.clone(), items, cx),
            Row::Fold {
                id,
                label,
                time,
                open,
                ..
            } => self.render_fold(id.clone(), label, time, *open, cx),
            Row::Working { started } => self.render_working(started.as_deref(), cx),
            Row::Live {
                group,
                entry,
                thought,
                items,
                active,
                ..
            } => self.render_live(group, entry, *thought, items, *active, cx),
            Row::Thinking { group, label } => {
                self.render_thinking(group.as_ref(), label.as_deref(), cx)
            }
            Row::Images { paths, .. } => self.render_viewed_images(paths, cx),
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
    pub(crate) fn render_working(
        &self,
        started: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let elapsed = started
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| (jiff::Timestamp::now().as_second() - t.as_second()).max(0) as u64);
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
        div()
            .pt_1()
            .pb_2()
            .border_b_1()
            .border_color(c.border.opacity(0.6))
            .child(
                h_flex()
                    .h(px(24.))
                    .px_1()
                    .text_sm()
                    .line_height(relative(1.625))
                    .text_color(c.muted_foreground)
                    .child(match elapsed {
                        Some(elapsed) => crate::i18n::tv(
                            "Working for {time}",
                            &[("time", &working_timer(elapsed))],
                        ),
                        None => crate::i18n::t("Working..."),
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Row, TurnItem, approval_options, row_splice, timeline_rows, tool_parts, working_timer,
    };
    use crate::work_rows::summarize as summarize_group;
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
    fn summarizes_other_items() {
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
        let rows = timeline_rows(items.iter(), &Default::default(), None);
        assert_eq!(rows.len(), 4);
        let Row::Group { items: group, .. } = &rows[1] else {
            panic!("expected a group")
        };
        assert_eq!(group.len(), 6, "a thought without text is left out");
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
    fn folds_a_finished_turn_before_its_answer() {
        let mk = |id: &str, kind: &str, run: &str, at: &str| {
            item(json!({"id": id, "ordinal": 1, "type": kind, "runId": run,
                "startedAt": at, "updatedAt": at}))
        };
        let items = [
            mk("u", "user_message", "r1", "2026-01-01T10:00:00Z"),
            mk("t", "reasoning", "r1", "2026-01-01T10:00:01Z"),
            mk("c", "command_execution", "r1", "2026-01-01T10:00:02Z"),
            mk("m", "assistant_message", "r1", "2026-01-01T10:00:03Z"),
            mk("a", "assistant_message", "r1", "2026-01-01T10:00:05.300Z"),
            mk("u2", "user_message", "r2", "2026-01-01T10:01:00Z"),
            mk("c2", "command_execution", "r2", "2026-01-01T10:01:01Z"),
            mk("a2", "assistant_message", "r2", "2026-01-01T10:01:02Z"),
        ];
        let none = std::collections::HashSet::new();
        let keys = |rows: &[Row]| rows.iter().map(|r| r.key().to_owned()).collect::<Vec<_>>();
        let live = super::LiveTurn {
            run: Some("r2"),
            started: None,
            turn_ended: false,
        };
        let rows = super::timeline_rows(items.iter(), &none, Some(&live));
        assert_eq!(
            keys(&rows),
            [
                "u",
                "fold:t",
                "a",
                "u2",
                "working-indicator-row",
                "c2",
                "a2",
                "live-activity-row"
            ],
            "a running turn stays open, timed after its prompt and thinking at its end"
        );
        let Row::Fold {
            label,
            items: folded,
            ..
        } = &rows[1]
        else {
            panic!("expected a fold")
        };
        assert_eq!(label, "Worked for 5.3s");
        assert_eq!(folded.len(), 3);
        let open = std::collections::HashSet::from(["fold:t".to_owned()]);
        let rows = super::timeline_rows(items.iter(), &open, None);
        assert_eq!(
            keys(&rows),
            ["u", "fold:t", "c", "m", "a", "u2", "fold:c2", "a2"],
            "an open fold shows its rows after it; a thought without text is left out"
        );
        assert_eq!(super::format_duration(75_000), "1m 15s");
    }

    #[test]
    fn a_running_turn_shows_its_live_rows_like_the_web() {
        let mk = |id: &str, kind: &str, status: &str, extra: Value| {
            let mut v =
                json!({"id": id, "ordinal": 1, "type": kind, "runId": "r", "status": status});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            item(v)
        };
        let live = super::LiveTurn {
            run: Some("r"),
            started: None,
            turn_ended: false,
        };
        let none = std::collections::HashSet::new();
        let keys = |rows: &[Row]| rows.iter().map(|r| r.key().to_owned()).collect::<Vec<_>>();

        let running = [
            mk("u", "user_message", "completed", json!({})),
            mk(
                "t",
                "reasoning",
                "completed",
                json!({"text": "**Plan**\nlook"}),
            ),
            mk(
                "c",
                "command_execution",
                "running",
                json!({"input": "vp test"}),
            ),
        ];
        let rows = timeline_rows(running.iter(), &none, Some(&live));
        assert_eq!(
            keys(&rows),
            ["u", "working-indicator-row", "live-activity-row"]
        );
        let Row::Live {
            entry,
            thought,
            active,
            ..
        } = &rows[2]
        else {
            panic!("expected a live row")
        };
        assert_eq!(entry.id, "c");
        assert_eq!(thought.map(|t| t.id.as_str()), Some("t"));
        assert!(*active);
        assert_eq!(
            crate::work_rows::live_label(entry, None, true),
            "Running vp"
        );

        let failed = [
            mk("u", "user_message", "completed", json!({})),
            mk(
                "c",
                "command_execution",
                "completed",
                json!({"input": "ls", "exitCode": 2}),
            ),
        ];
        let rows = timeline_rows(failed.iter(), &none, Some(&live));
        assert_eq!(
            keys(&rows),
            ["u", "working-indicator-row", "live-activity-row"]
        );
        assert!(
            matches!(&rows[2], Row::Thinking { group: Some((_, items)), .. } if items.len() == 1),
            "a failed call leaves Thinking, which opens to it"
        );

        let answering = [
            mk("u", "user_message", "completed", json!({})),
            mk(
                "a",
                "assistant_message",
                "running",
                json!({"streaming": true}),
            ),
        ];
        let rows = timeline_rows(answering.iter(), &none, Some(&live));
        assert_eq!(
            keys(&rows),
            ["u", "working-indicator-row", "a"],
            "no Thinking while the answer streams"
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
