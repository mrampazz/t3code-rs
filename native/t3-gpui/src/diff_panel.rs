//! Checkpoint diff panel (web: `apps/web/src/components/DiffPanel.tsx`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::diff::{
    Diff, DiffEvent, DiffFile, DiffLineRange, DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// What a review comment needs from the loaded diff: its raw patches, the line selection, and
/// the folders folded in the file tree.
#[derive(Default)]
pub struct Review {
    patches: Vec<String>,
    selection: Option<DiffLineRange>,
    folded: std::collections::HashSet<String>,
    _events: Option<Subscription>,
}

/// Ends a diff the server cut at its size limit (`CHECKPOINT_DIFF_TRUNCATED_MARKER`).
const TRUNCATED_MARKER: &str = "\n\n[truncated]";

/// Bounds how much of a huge pull request one open fetches.
const MAX_PR_DIFF_PAGES: usize = 20;

impl T3App {
    /// `None` diffs the whole thread up to its latest turn; `Some(n)` diffs turn `n` alone.
    pub(crate) fn load_diff(&mut self, target: Option<u64>, cx: &mut Context<Self>) {
        self.diff_target = target;
        self.diff_pr = None;
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
                let result = result.map_err(anyhow::Error::from).and_then(|r| r);
                let loaded = result.and_then(|value| {
                    let (files, truncated) = parse_turn_diff(&value)?;
                    let note = truncated.then(|| "The server truncated this diff.".to_owned());
                    Ok((patch_texts(&value), files, note))
                });
                app.show_diff(generation, loaded, cx);
            })
            .ok();
        })
        .detach();
    }

    /// A linked pull request's diff (web: `PullRequestCodeTab`), every page of it.
    /// shortcut: files the server withholds as too large are listed, not readable one by one.
    pub(crate) fn load_pr_diff(&mut self, link: &Value, cx: &mut Context<Self>) {
        let (Some(session), Some(input)) =
            (self.env().map(|e| e.session.clone()), self.pr_ref(link))
        else {
            return;
        };
        self.diff_pr = Some(input.clone());
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let mut input = input;
        self.diff_note = Some("Loading…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let mut pages = Vec::new();
                loop {
                    let page =
                        crate::auth::post_json(&session, "/api/pull-requests/diff", &input).await?;
                    let next = page["nextCursor"].as_str().map(str::to_owned);
                    pages.push(page);
                    match next {
                        Some(cursor) if pages.len() < MAX_PR_DIFF_PAGES => {
                            input["cursor"] = json!(cursor)
                        }
                        _ => break,
                    }
                }
                anyhow::Ok(pages)
            })
            .await;
            this.update(cx, |app, cx| {
                let loaded = result
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r)
                    .and_then(|pages| parse_pr_diff(&pages));
                app.show_diff(generation, loaded, cx);
            })
            .ok();
        })
        .detach();
    }

    /// A linked pull request as a `PullRequestRef` in the selected thread's project.
    fn pr_ref(&self, link: &Value) -> Option<Value> {
        let project = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))?
            .project_id
            .clone();
        let mut pr = json!({ "projectId": project, "repository": link["repository"].as_str()?,
            "number": link["number"].as_u64()? });
        if let Some(host) = link["host"].as_str() {
            pr["host"] = json!(host);
        }
        Some(pr)
    }

    /// Asks for the review's summary, then submits it with the pending line comments
    /// (web: `PullRequestReviewForm`). A refused review keeps its comments to try again.
    /// shortcut: every verdict is offered; the host refuses ones the viewer may not give.
    fn submit_review(
        &mut self,
        verdict: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Textarea, TextareaState};
        let Some(pr) = self.diff_pr.clone() else {
            return;
        };
        let input = cx.new(|cx| {
            let mut state =
                TextareaState::new(window, cx).placeholder(t("Summarize your review (optional)"));
            state.set_auto_grow(3, 10, cx);
            state
        });
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, pr) = (app.clone(), input.clone(), pr.clone());
            dialog
                .title(verdict_label(verdict))
                .child(Textarea::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Submit review"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let body = input.read(cx).value().to_string();
                    app.update(cx, |this, cx| {
                        let comments = this
                            .pr_drafts
                            .get(&pr_key(&pr))
                            .cloned()
                            .unwrap_or_default();
                        if verdict != "approve" && body.trim().is_empty() && comments.is_empty() {
                            return false;
                        }
                        let Some(client) = this.client() else {
                            return true;
                        };
                        let mut payload = pr.clone();
                        payload["verdict"] = json!(verdict);
                        payload["body"] = json!(body);
                        payload["comments"] = json!(comments);
                        let sent = comments.len();
                        this.diff_note = Some(t("Submitting review…").into());
                        cx.notify();
                        let pr = pr.clone();
                        cx.spawn(async move |this, cx| {
                            let result = on_tokio(async move {
                                client.call("pullRequests.submitReview", payload).await
                            })
                            .await;
                            this.update(cx, |app, cx| {
                                app.diff_note = Some(match result {
                                    Ok(Ok(_)) => {
                                        // Comments added while it was sending stay pending.
                                        if let Some(d) = app.pr_drafts.get_mut(&pr_key(&pr)) {
                                            d.drain(..sent.min(d.len()));
                                        }
                                        t("Review submitted").into()
                                    }
                                    Ok(Err(e)) => {
                                        format!("{}: {e}", t("The review could not be submitted"))
                                            .into()
                                    }
                                    Err(e) => e.to_string().into(),
                                });
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                        true
                    })
                    .unwrap_or(true)
                })
        });
    }

    fn show_diff(
        &mut self,
        generation: u64,
        loaded: anyhow::Result<(Vec<String>, Vec<DiffFile>, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        if self.diff_generation != generation {
            return;
        }
        self.diff_review = Review::default();
        match loaded {
            Ok((patches, files, note)) if !files.is_empty() => {
                self.diff_review.patches = patches;
                let mode = if self.diff_split {
                    DiffMode::Split
                } else {
                    DiffMode::Unified
                };
                let state = cx.new(|cx| DiffState::new(files, cx).with_mode(mode));
                self.diff_review._events =
                    Some(cx.subscribe(&state, |app, _, event: &DiffEvent, cx| {
                        match event {
                            DiffEvent::SelectionEnded(range) => {
                                app.diff_review.selection = Some(range.clone())
                            }
                            DiffEvent::SelectionChanged(None) => app.diff_review.selection = None,
                            _ => return,
                        }
                        cx.notify();
                    }));
                self.diff_state = Some(state);
                self.diff_note = note.map(Into::into);
            }
            Ok((_, _, note)) => {
                self.diff_state = None;
                self.diff_note = Some(
                    note.unwrap_or_else(|| "No changes in this range.".into())
                        .into(),
                );
            }
            Err(e) => {
                self.diff_state = None;
                self.diff_note = Some(format!("Could not load the diff: {e}").into());
            }
        }
        cx.notify();
    }

    /// This session's choice, else the client's `diffIgnoreWhitespace` preference.
    pub(crate) fn diff_ignore_whitespace(&self) -> bool {
        self.diff_whitespace
            .unwrap_or(self.prefs["diffIgnoreWhitespace"] == true)
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
                        if this.diff_pr.is_none() {
                            this.load_diff(this.diff_target, cx);
                        }
                    })),
            )
            .child(self.turn_button("diff-all", "All turns".into(), None, cx));
        for (i, link) in self.thread_pull_requests().into_iter().enumerate() {
            let key = pr_key(&link);
            let button = Button::new(("diff-pr", i))
                .xsmall()
                .label(format!("PR #{}", link["number"]))
                .tooltip(link["repository"].as_str().unwrap_or("").to_owned())
                .on_click(cx.listener(move |this, _, _, cx| this.load_pr_diff(&link, cx)));
            turns = turns.child(if self.diff_pr.as_ref().map(pr_key) == Some(key) {
                button.primary()
            } else {
                button.ghost()
            });
        }
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
            .flex_1()
            .min_h_0()
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
                let stats: std::collections::HashMap<SharedString, (usize, usize)> = files
                    .iter()
                    .map(|(p, a, d)| (p.clone(), (*a, *d)))
                    .collect();
                let paths: Vec<String> = files.iter().map(|(p, _, _)| p.to_string()).collect();
                let folded = &self.diff_review.folded;
                let rows = file_tree(&paths).into_iter().filter(|(_, _, path, _)| {
                    !folded
                        .iter()
                        .any(|dir| path.starts_with(&format!("{dir}/")))
                });
                col.child(
                    v_flex()
                        .id("diff-files")
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .pb_1()
                        .children(rows.enumerate().map(|(i, (depth, name, path, dir))| {
                            let is_folded = dir && folded.contains(&path);
                            let (adds, dels) =
                                stats.get(path.as_str()).copied().unwrap_or_default();
                            h_flex()
                                .id(("diff-file", i))
                                .pl(px(12. + depth as f32 * 14.))
                                .pr_3()
                                .gap_2()
                                .text_xs()
                                .cursor_pointer()
                                .hover(|r| r.bg(c.list_hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .when(dir, |d| d.text_color(c.muted_foreground))
                                        .child(if dir {
                                            format!("{} {name}/", if is_folded { "▸" } else { "▾" })
                                        } else {
                                            name
                                        }),
                                )
                                .when(!dir, |r| {
                                    r.child(div().text_color(c.success).child(format!("+{adds}")))
                                        .child(div().text_color(c.danger).child(format!("−{dels}")))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if dir {
                                        let folded = &mut this.diff_review.folded;
                                        if !folded.remove(&path) {
                                            folded.insert(path.clone());
                                        }
                                        cx.notify();
                                    } else if let Some(state) = &this.diff_state {
                                        state.update(cx, |s, cx| s.scroll_to_file(&path, cx));
                                    }
                                }))
                        })),
                )
            })
            .children(self.diff_review.selection.clone().map(|range| {
                let lines = if range.start() == range.end() {
                    format!("L{}", range.start())
                } else {
                    format!("L{} to L{}", range.start(), range.end())
                };
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .child(format!("{} {lines}", range.path())),
                    )
                    .child(
                        Button::new("diff-comment")
                            .xsmall()
                            .outline()
                            .label(t("Comment…"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.comment_on_selection(range.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("diff-comment-clear")
                            .xsmall()
                            .ghost()
                            .label("✕")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.diff_review.selection = None;
                                if let Some(state) = &this.diff_state {
                                    state.update(cx, |s, cx| s.set_selected_lines(None, cx));
                                }
                                cx.notify();
                            })),
                    )
            }))
            .children(self.render_review_bar(cx))
    }

    /// Asks for the comment, then adds it to the composer as a `review-comment` context link
    /// sent with the next message (web: `AnnotatableCodeView` → composer draft review comments).
    /// shortcut: comments show as links in the composer, not inline in the diff.
    fn comment_on_selection(
        &mut self,
        range: DiffLineRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Textarea, TextareaState};
        let path = range.path().to_string();
        let rows = review_rows(&self.diff_review.patches, &path);
        let index = |side: DiffSide, line| review_index(&rows, side == DiffSide::Original, line);
        let Some(bounds) =
            index(range.side(), range.start()).zip(index(range.end_side(), range.end()))
        else {
            self.diff_note = Some(t("Could not find those lines in the diff.").into());
            return cx.notify();
        };
        if let Some(pr) = self.diff_pr.clone() {
            return self.comment_on_pr(pr, path, &rows, bounds, window, cx);
        }
        let section = match self.diff_target {
            Some(turn) => (format!("turn:{turn}"), format!("Turn {turn}")),
            None => ("thread".to_owned(), "All turns".to_owned()),
        };
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder(t("Leave a comment"));
            state.set_auto_grow(3, 10, cx);
            state
        });
        let app = cx.entity().downgrade();
        let rows = std::rc::Rc::new(rows);
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, rows, path, section) = (
                app.clone(),
                input.clone(),
                rows.clone(),
                path.clone(),
                section.clone(),
            );
            dialog
                .title(t("Comment"))
                .child(Textarea::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Add to chat"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx: &mut App| {
                    let text = input.read(cx).value().to_string();
                    if text.trim().is_empty() {
                        return false;
                    }
                    let id = uuid::Uuid::new_v4().to_string();
                    let Some(record) =
                        review_record(&rows, bounds, &path, (&section.0, &section.1), &text, &id)
                    else {
                        return true;
                    };
                    app.update(cx, |this, cx| this.add_composer_context(record, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// Asks for a line comment and keeps it pending for the next review (web: review drafts).
    /// shortcut: a multi-line selection anchors on its last line; `startPosition` needs the host's
    /// `review.rangeComment` capability, which this client does not read.
    fn comment_on_pr(
        &mut self,
        pr: Value,
        path: String,
        rows: &[ReviewRow],
        (start, end): (usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Textarea, TextareaState};
        let Some(position) = rows.get(start.max(end)).map(review_position) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder(t("Leave a comment"));
            state.set_auto_grow(3, 10, cx);
            state
        });
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, pr, path, position) = (
                app.clone(),
                input.clone(),
                pr.clone(),
                path.clone(),
                position.clone(),
            );
            dialog
                .title(t("Comment"))
                .child(Textarea::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Add to review"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let body = input.read(cx).value().to_string();
                    if body.trim().is_empty() {
                        return false;
                    }
                    let draft = json!({ "path": path, "position": position, "body": body });
                    app.update(cx, |this, cx| {
                        this.pr_drafts.entry(pr_key(&pr)).or_default().push(draft);
                        this.diff_review.selection = None;
                        if let Some(state) = &this.diff_state {
                            state.update(cx, |s, cx| s.set_selected_lines(None, cx));
                        }
                        cx.notify();
                    })
                    .ok();
                    true
                })
        });
    }

    /// Pending line comments and the verdict buttons, while a pull request's diff is open.
    fn render_review_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let pr = self.diff_pr.as_ref()?;
        let c = cx.theme().colors;
        let pending = self.pr_drafts.get(&pr_key(pr)).map_or(0, Vec::len);
        Some(
            h_flex()
                .px_3()
                .py_1p5()
                .gap_1()
                .text_xs()
                .border_t_1()
                .border_color(c.border)
                .child(
                    div()
                        .flex_1()
                        .text_color(c.muted_foreground)
                        .child(match pending {
                            0 => t("Select lines to comment"),
                            1 => t("1 pending comment"),
                            n => format!("{n} {}", t("pending comments")),
                        }),
                )
                .when(pending > 0, |bar| {
                    bar.child(
                        Button::new("review-discard")
                            .xsmall()
                            .ghost()
                            .label(t("Discard"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(pr) = &this.diff_pr {
                                    this.pr_drafts.remove(&pr_key(pr));
                                }
                                cx.notify();
                            })),
                    )
                })
                .children(["comment", "approve", "request-changes"].map(|verdict| {
                    Button::new(verdict)
                        .xsmall()
                        .outline()
                        .label(verdict_label(verdict))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.submit_review(verdict, window, cx)
                        }))
                })),
        )
    }

    /// Appends a context link for `record` to the composer and keeps the record for sending.
    pub(crate) fn add_composer_context(
        &mut self,
        record: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kind, id, label) = (
            record["kind"].as_str().unwrap_or(""),
            record["contextId"].as_str().unwrap_or(""),
            record["label"].as_str().unwrap_or(""),
        );
        let link = format!("[{label}](t3-context://v1/{kind}/{id}) ");
        self.composer.update(cx, |input, cx| {
            let text = input.value().to_string();
            let sep = if text.is_empty() || text.ends_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            input.set_value(format!("{text}{sep}{link}"), window, cx);
        });
        self.composer_contexts.push(record);
        self.diff_review.selection = None;
        if let Some(state) = &self.diff_state {
            state.update(cx, |s, cx| s.set_selected_lines(None, cx));
        }
        cx.notify();
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
        if self.diff_pr.is_none() && self.diff_target == target {
            button.primary()
        } else {
            button.ghost()
        }
    }
}

/// A `ThreadTurnDiff`'s patches: the thread folder's, then each tracked repository's.
pub(crate) fn patch_texts(value: &Value) -> Vec<String> {
    let repositories = value["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| &r["diff"]);
    std::iter::once(&value["diff"])
        .chain(repositories)
        .filter_map(Value::as_str)
        .map(|p| p.strip_suffix(TRUNCATED_MARKER).unwrap_or(p).to_owned())
        .collect()
}

/// (repository, number): which pull request a link, ref or draft list is about.
pub(crate) fn pr_key(pr: &Value) -> (String, u64) {
    (
        pr["repository"].as_str().unwrap_or("").to_owned(),
        pr["number"].as_u64().unwrap_or(0),
    )
}

fn verdict_label(verdict: &str) -> String {
    match verdict {
        "approve" => crate::i18n::tc("Approve", "review verdict"),
        "request-changes" => t("Request changes"),
        _ => crate::i18n::tc("Comment", "review verdict"),
    }
}

/// A diff row as a `PullRequestReviewPosition`.
pub(crate) fn review_position(row: &ReviewRow) -> Value {
    match (row.change, row.old, row.new) {
        ('+', _, Some(new)) => json!({ "kind": "added", "newLine": new }),
        ('-', Some(old), _) => json!({ "kind": "deleted", "oldLine": old }),
        (_, old, new) => {
            json!({ "kind": "context", "oldLine": old, "newLine": new, "side": "right" })
        }
    }
}

/// Parses `PullRequestDiffResult` pages into one diff, noting what the server left out.
pub(crate) fn parse_pr_diff(
    pages: &[Value],
) -> anyhow::Result<(Vec<String>, Vec<DiffFile>, Option<String>)> {
    let mut patches = Vec::new();
    let mut files = Vec::new();
    let mut omitted = Vec::new();
    for page in pages {
        let patch = page["patch"].as_str().unwrap_or("");
        if !patch.trim().is_empty() {
            files.extend(DiffFile::parse(patch).map_err(|e| anyhow::anyhow!("{e:?}"))?);
            patches.push(patch.to_owned());
        }
        omitted.extend(
            page["omittedFileStats"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f["path"].as_str()),
        );
    }
    let truncated = pages.iter().any(|p| p["truncated"] == true)
        || pages.last().is_some_and(|p| p["nextCursor"].is_string());
    let note = match (omitted.is_empty(), truncated) {
        (false, _) => Some(format!("Too large to show: {}", omitted.join(", "))),
        (true, true) => Some("The server truncated this diff.".to_owned()),
        (true, false) => None,
    };
    Ok((patches, files, note))
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

/// One row of a file's hunks, as the review comment's excerpt reads it.
#[derive(Debug, PartialEq)]
pub(crate) struct ReviewRow {
    change: char,
    old: Option<usize>,
    new: Option<usize>,
    content: String,
}

/// The hunk rows of `path` in these patches (web: `buildDiffReviewLines` without expanded context).
pub(crate) fn review_rows(patches: &[String], path: &str) -> Vec<ReviewRow> {
    let mut rows = Vec::new();
    for patch in patches {
        for section in patch.split("diff --git ").skip(1) {
            let header = |prefix: &str| {
                section
                    .lines()
                    .find_map(|l| l.strip_prefix(prefix))
                    .filter(|p| *p != "/dev/null")
                    .map(|p| p.get(2..).unwrap_or(p).to_owned())
            };
            if header("+++ ").or_else(|| header("--- ")).as_deref() != Some(path) {
                continue;
            }
            let (mut old, mut new) = (0, 0);
            for line in section.lines().skip_while(|l| !l.starts_with("@@")) {
                if let Some(h) = line.strip_prefix("@@ -") {
                    let num = |s: &str| {
                        s.split([',', ' '])
                            .next()
                            .and_then(|n| n.parse().ok())
                            .unwrap_or(0)
                    };
                    old = num(h);
                    new = h.split_once(" +").map_or(0, |(_, n)| num(n));
                    continue;
                }
                let Some(change) = line.chars().next().filter(|c| matches!(c, ' ' | '-' | '+'))
                else {
                    continue;
                };
                let content = line[1..].to_owned();
                let (o, n) = match change {
                    '-' => (Some(old), None),
                    '+' => (None, Some(new)),
                    _ => (Some(old), Some(new)),
                };
                old += usize::from(o.is_some());
                new += usize::from(n.is_some());
                rows.push(ReviewRow {
                    change,
                    old: o,
                    new: n,
                    content,
                });
            }
        }
    }
    rows
}

/// The `review-comment` context record for `rows[start..=end]` (web: `buildDiffReviewComment`).
pub(crate) fn review_record(
    rows: &[ReviewRow],
    (start, end): (usize, usize),
    path: &str,
    section: (&str, &str),
    text: &str,
    comment_id: &str,
) -> Option<Value> {
    let (start, end) = (start.min(end), start.max(end));
    let lines = rows.get(start..=end)?;
    let range = |key: fn(&ReviewRow) -> Option<usize>| {
        let numbered: Vec<usize> = lines.iter().filter_map(key).collect();
        (numbered.first().copied().unwrap_or(0), numbered.len())
    };
    let ((old_start, old_count), (new_start, new_count)) = (range(|r| r.old), range(|r| r.new));
    let (first, last) = (&lines[0], &lines[lines.len() - 1]);
    let marker = if first.change != ' ' && lines.iter().all(|l| l.change == first.change) {
        first.change.to_string()
    } else {
        String::new()
    };
    let (a, b) = (first.new.or(first.old)?, last.new.or(last.old)?);
    let range_label = if a == b {
        format!("{marker}{a}")
    } else {
        format!("{marker}{a} to {marker}{b}")
    };
    let before = if marker == "-" { " (before)" } else { "" };
    let lines_label = if marker.is_empty() {
        range_label.clone()
    } else if a == b {
        format!("L{a}{before}")
    } else {
        format!("L{a} to L{b}{before}")
    };
    let name = path.rsplit('/').next().unwrap_or(path);
    let label =
        crate::composer_menu::context_label(&format!("{name} {lines_label}"), "review-comment");
    let mut diff = vec![format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    )];
    diff.extend(lines.iter().map(|l| format!("{}{}", l.change, l.content)));
    Some(json!({
        "version": 1,
        "kind": "review-comment",
        "contextId": crate::composer_menu::context_id("review-comment", comment_id),
        "label": label,
        "sectionId": section.0,
        "sectionTitle": section.1,
        "filePath": path,
        "startIndex": start,
        "endIndex": end,
        "rangeLabel": range_label,
        "text": crate::composer_menu::clamp_context_text(text.trim(), 16_000),
        "diff": crate::composer_menu::clamp_context_text(&diff.join("\n"), 32_000),
        "fenceLanguage": "diff",
    }))
}

/// The row index a selected line lands on: the old side skips additions, the new side deletions.
pub(crate) fn review_index(rows: &[ReviewRow], original: bool, line: usize) -> Option<usize> {
    rows.iter().position(|r| {
        if original {
            r.change != '+' && r.old == Some(line)
        } else {
            r.change != '-' && r.new == Some(line)
        }
    })
}

/// Folder tree rows as (depth, name, path, is_dir), directories before the files under them.
pub(crate) fn file_tree(paths: &[String]) -> Vec<(usize, String, String, bool)> {
    let mut sorted: Vec<&String> = paths.iter().collect();
    sorted.sort_by(|a, b| a.split('/').cmp(b.split('/')));
    let mut rows = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    for path in sorted {
        let parts: Vec<&str> = path.split('/').collect();
        let dirs = &parts[..parts.len() - 1];
        let shared = open.iter().zip(dirs).take_while(|(a, b)| a == b).count();
        open.truncate(shared);
        for (depth, dir) in dirs.iter().enumerate().skip(shared) {
            open.push(dir);
            rows.push((depth, (*dir).to_owned(), dirs[..=depth].join("/"), true));
        }
        rows.push((
            dirs.len(),
            parts[parts.len() - 1].to_owned(),
            path.clone(),
            false,
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{
        TRUNCATED_MARKER, file_tree, parse_turn_diff, review_index, review_record, review_rows,
    };
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

    #[test]
    fn review_positions_follow_the_row() {
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3,2 +3,2 @@\n ctx\n-old\n+new\n";
        let rows = review_rows(&[patch.to_owned()], "f");
        let positions: Vec<_> = rows.iter().map(super::review_position).collect();
        assert_eq!(
            positions,
            [
                json!({"kind": "context", "oldLine": 3, "newLine": 3, "side": "right"}),
                json!({"kind": "deleted", "oldLine": 4}),
                json!({"kind": "added", "newLine": 4}),
            ]
        );
    }

    #[test]
    fn joins_pull_request_pages() {
        let patch = |name: &str| {
            format!(
                "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-a\n+b\n"
            )
        };
        let pages = [
            json!({"patch": patch("a.rs"), "truncated": false, "nextCursor": "c"}),
            json!({"patch": patch("b.rs"), "truncated": false, "nextCursor": null,
                   "omittedFileStats": [{"path": "big.lock", "additions": 1, "deletions": 0}]}),
        ];
        let (patches, files, note) = super::parse_pr_diff(&pages).unwrap();
        assert_eq!((patches.len(), files.len()), (2, 2));
        assert_eq!(note.as_deref(), Some("Too large to show: big.lock"));
        let (_, _, note) = super::parse_pr_diff(&pages[..1]).unwrap();
        assert_eq!(note.as_deref(), Some("The server truncated this diff."));
    }

    #[test]
    fn review_comment_excerpt_and_label() {
        let patch = "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -10,3 +10,4 @@ fn\n keep\n-gone\n+one\n+two\n";
        let rows = review_rows(&[patch.to_owned()], "src/x.rs");
        assert_eq!(rows.len(), 4);
        let (start, end) = (
            review_index(&rows, false, 11).unwrap(),
            review_index(&rows, false, 12).unwrap(),
        );
        let record = review_record(
            &rows,
            (start, end),
            "src/x.rs",
            ("turn:1", "Turn 1"),
            " fix ",
            "c1",
        )
        .unwrap();
        assert_eq!(record["rangeLabel"], "+11 to +12");
        assert_eq!(record["label"], "x.rs L11 to L12");
        assert_eq!(record["diff"], "@@ -0,0 +11,2 @@\n+one\n+two");
        assert_eq!(record["text"], "fix");
        assert_eq!(record["contextId"], "review-comment_c1");
        let old = review_index(&rows, true, 11).unwrap();
        let record = review_record(&rows, (old, old), "src/x.rs", ("t", "T"), "x", "c2").unwrap();
        assert_eq!(record["label"], "x.rs L11 (before)");
        assert!(review_rows(&[patch.to_owned()], "other.rs").is_empty());
    }

    #[test]
    fn file_tree_groups_by_folder() {
        let paths = ["src/b.rs", "README.md", "src/a/x.rs", "src/c.rs"].map(String::from);
        let names: Vec<_> = file_tree(&paths)
            .into_iter()
            .map(|(d, n, _, dir)| format!("{d}{n}{}", if dir { "/" } else { "" }))
            .collect();
        assert_eq!(
            names,
            ["0README.md", "0src/", "1a/", "2x.rs", "1b.rs", "1c.rs"]
        );
    }
}
