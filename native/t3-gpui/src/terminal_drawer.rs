//! Thread terminal drawer: attaches to the server PTY and renders it with `terminal::TermView`.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::terminal::{MAX_HISTORY_BYTES, TERMINAL_ID, TermView, key_bytes};
use crate::{T3App, on_tokio};

/// shortcut: a fixed grid; upgrade by measuring the drawer and sending `terminal.resize`.
const COLS: usize = 120;
const ROWS: usize = 18;

impl T3App {
    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.term.is_some() {
            self.close_terminal(cx);
        } else {
            self.open_terminal(false, cx);
            window.focus(&self.term_focus, cx);
        }
        cx.notify();
    }

    /// The server sizes the PTY to every attached viewer, so a hidden drawer detaches.
    pub(crate) fn close_terminal(&mut self, cx: &mut Context<Self>) {
        if let (Some(client), Some(sub)) = (self.client(), self.term_sub.take()) {
            client.cancel(&sub);
        }
        self.term = None;
        self.term_status = None;
        cx.notify();
    }

    /// `resume` reattaches after a reconnect without starting a new shell.
    pub(crate) fn open_terminal(&mut self, resume: bool, cx: &mut Context<Self>) {
        let (Some(client), Some(thread)) = (
            self.client(),
            self.selected
                .as_ref()
                .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
                .cloned(),
        ) else {
            return;
        };
        let Some(cwd) = self.thread_cwd() else { return };
        if let Some(old) = self.term_sub.take() {
            client.cancel(&old);
        }
        self.term = Some(TermView::new(COLS, ROWS));
        self.term_status = Some("Starting…".into());
        let mut payload = json!({
            "threadId": thread.id,
            "terminalId": TERMINAL_ID,
            "cwd": cwd,
            "worktreePath": thread.worktree_path,
            "cols": COLS,
            "rows": ROWS,
            "maxHistoryBytes": MAX_HISTORY_BYTES,
        });
        if resume {
            payload["resume"] = json!(true);
        }
        let (sub, mut rx) = client.request("terminal.attach", payload);
        self.term_sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                let alive = this.update(cx, |app, cx| {
                    if app.term_sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    match event {
                        Ok(event) => app.apply_terminal_event(&event, cx),
                        Err(e) => app.term_status = Some(format!("Terminal failed: {e}").into()),
                    }
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

    fn apply_terminal_event(&mut self, event: &Value, cx: &mut Context<Self>) {
        let Some(term) = self.term.as_mut() else {
            return;
        };
        // Every renderer draws the server's shared PTY grid.
        let grid = if event["type"] == "activity" {
            event
        } else {
            &event["snapshot"]
        };
        if let (Some(cols), Some(rows)) = (grid["cols"].as_u64(), grid["rows"].as_u64())
            && (cols as usize, rows as usize) != (term.cols, term.rows)
        {
            term.resize(cols as usize, rows as usize);
        }
        match event["type"].as_str() {
            Some("snapshot" | "restarted") => {
                let snapshot = &event["snapshot"];
                term.reset();
                term.feed(snapshot["history"].as_str().unwrap_or(""));
                self.term_status = match snapshot["status"].as_str() {
                    Some("running" | "starting") => None,
                    Some(status) => Some(
                        snapshot["error"]
                            .as_str()
                            .unwrap_or(status)
                            .to_string()
                            .into(),
                    ),
                    None => None,
                };
            }
            Some("output") => term.feed(event["data"].as_str().unwrap_or("")),
            Some("cleared") => term.reset(),
            Some("exited") => {
                let code = event["exitCode"]
                    .as_i64()
                    .map_or(String::new(), |c| format!(" ({c})"));
                self.term_status = Some(format!("Process exited{code}").into());
            }
            Some("closed") => self.term_status = Some("Terminal closed".into()),
            Some("error") => {
                self.term_status = Some(
                    event["message"]
                        .as_str()
                        .unwrap_or("Terminal error")
                        .to_string()
                        .into(),
                )
            }
            _ => {}
        }
        for reply in self
            .term
            .as_mut()
            .map(TermView::take_replies)
            .unwrap_or_default()
        {
            self.terminal_write(reply, true, cx);
        }
    }

    fn terminal_write(&mut self, data: String, reply: bool, cx: &mut Context<Self>) {
        let (Some(client), Some(thread_id)) = (self.client(), self.selected.clone()) else {
            return;
        };
        let mut payload = json!({ "threadId": thread_id, "terminalId": TERMINAL_ID, "data": data });
        if reply {
            payload["reply"] = json!(true);
        }
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) =
                on_tokio(async move { client.call("terminal.write", payload).await }).await
            {
                this.update(cx, |app, cx| {
                    app.term_status = Some(format!("Write failed: {e}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn terminal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if m.platform && k.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.terminal_write(text, false, cx);
                cx.stop_propagation();
            }
            return;
        }
        if let Some(bytes) = key_bytes(&k.key, k.key_char.as_deref(), m.control, m.alt, m.platform)
        {
            self.terminal_write(bytes, false, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let lines = self.term.as_ref().map(TermView::lines).unwrap_or_default();
        let thread_id = self.selected.clone();
        let rgb = |c: alacritty_terminal::vte::ansi::Rgb| -> Hsla {
            gpui_kit::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
        };
        v_flex()
            .h(px(320.))
            .border_t_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child("Terminal")
                    .children(self.term_status.clone())
                    .child(div().flex_1())
                    .child(
                        Button::new("term-clear")
                            .xsmall()
                            .ghost()
                            .label("Clear")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(id) = thread_id.clone() {
                                    let payload =
                                        json!({ "threadId": id, "terminalId": TERMINAL_ID });
                                    if let Some(client) = this.client() {
                                        on_tokio(async move {
                                            client.call("terminal.clear", payload).await
                                        });
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("term-hide")
                            .xsmall()
                            .ghost()
                            .label("Hide")
                            .on_click(cx.listener(|this, _, _, cx| this.close_terminal(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("terminal")
                    .flex_1()
                    .px_3()
                    .pb_2()
                    .bg(rgb(alacritty_terminal::vte::ansi::Rgb {
                        r: 0x1e,
                        g: 0x1e,
                        b: 0x1e,
                    }))
                    .font_family(mono)
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .overflow_hidden()
                    .track_focus(&self.term_focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.term_focus, cx)),
                    )
                    .on_key_down(
                        cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            this.terminal_key(event, cx)
                        }),
                    )
                    .children(lines.into_iter().map(|line| {
                        let highlights: Vec<_> = line
                            .runs
                            .into_iter()
                            .map(|run| {
                                (
                                    run.range,
                                    HighlightStyle {
                                        color: Some(rgb(run.fg)),
                                        background_color: run.bg.map(rgb),
                                        font_weight: run.bold.then_some(FontWeight::BOLD),
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect();
                        div()
                            .h(px(16.))
                            .child(StyledText::new(line.text).with_highlights(highlights))
                    })),
            )
    }
}
