//! Thread terminal drawer: attaches to the server PTY and renders it with `terminal::TermView`.

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::terminal::{MAX_HISTORY_BYTES, TermView, key_bytes, next_terminal_id};
use crate::{T3App, on_tokio};

/// The grid before the drawer has been measured.
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

    /// Opens another terminal for the thread, on the next free `term-N`.
    pub(crate) fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.selected.clone() else {
            return;
        };
        let ids: Vec<String> = self
            .terminal_tabs(&thread)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        self.term_id = next_terminal_id(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        self.open_terminal(false, cx);
    }

    pub(crate) fn switch_terminal(&mut self, id: String, cx: &mut Context<Self>) {
        if self.term_id != id {
            self.term_id = id;
            self.open_terminal(false, cx);
        }
    }

    /// Closes one terminal on the server; the drawer moves to another or hides.
    pub(crate) fn close_terminal_tab(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(thread) = self.selected.clone() else {
            return;
        };
        if let Some(client) = self.client() {
            let payload = json!({ "threadId": thread, "terminalId": id, "deleteHistory": true });
            on_tokio(async move { client.call("terminal.close", payload).await });
        }
        if id != self.term_id {
            return cx.notify();
        }
        let next = self
            .terminal_tabs(&thread)
            .into_iter()
            .map(|(t, _, _)| t)
            .find(|t| *t != id);
        match next {
            Some(next) => self.switch_terminal(next, cx),
            None => self.close_terminal(cx),
        }
    }

    /// (id, label, running a command) for the thread's terminals, including the attached one.
    pub(crate) fn terminal_tabs(&self, thread: &str) -> Vec<(String, String, bool)> {
        let mut tabs: Vec<(String, String, bool)> = self
            .env()
            .map(|e| e.terminals.of_thread(thread))
            .unwrap_or_default()
            .into_iter()
            .map(|t| {
                let id = t["terminalId"].as_str().unwrap_or("").to_owned();
                let label = t["label"]
                    .as_str()
                    .filter(|l| !l.is_empty())
                    .unwrap_or(&id)
                    .to_owned();
                (id, label, t["hasRunningSubprocess"] == true)
            })
            .collect();
        if self.term.is_some() && !tabs.iter().any(|t| t.0 == self.term_id) {
            tabs.push((self.term_id.clone(), self.term_id.clone(), false));
        }
        tabs
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
        let (cols, rows) = self.term_requested.unwrap_or((COLS, ROWS));
        self.term = Some(TermView::new(cols, rows));
        self.term_status = Some("Starting…".into());
        let mut payload = json!({
            "threadId": thread.id,
            "terminalId": self.term_id,
            "cwd": cwd,
            "worktreePath": thread.worktree_path,
            "cols": cols,
            "rows": rows,
            "maxHistoryBytes": MAX_HISTORY_BYTES,
        });
        if resume {
            payload["resume"] = json!(true);
        }
        if let Some(script) = &self.script_launch {
            payload["stopOnSettle"] = json!(script["stopOnSettle"] == true);
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
                if let Some(script) = self.script_launch.take() {
                    // A leading space keeps the command out of shell history (`projectScriptTerminalInput`).
                    let input = format!(" {}\r", script["command"].as_str().unwrap_or(""));
                    self.terminal_write(input, false, cx);
                }
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

    /// Asks the server for a grid that fills the drawer; it echoes the shared size back.
    pub(crate) fn fit_terminal(&mut self, cols: usize, rows: usize, cx: &mut Context<Self>) {
        let Some(term) = self.term.as_mut() else {
            return;
        };
        if (cols, rows) == (term.cols, term.rows) || self.term_requested == Some((cols, rows)) {
            return;
        }
        self.term_requested = Some((cols, rows));
        term.resize(cols, rows);
        let (Some(client), Some(thread_id)) = (self.client(), self.selected.clone()) else {
            return;
        };
        let payload = json!({ "threadId": thread_id, "terminalId": self.term_id, "cols": cols, "rows": rows });
        on_tokio(async move { client.call("terminal.resize", payload).await });
        cx.notify();
    }

    fn terminal_write(&mut self, data: String, reply: bool, cx: &mut Context<Self>) {
        let (Some(client), Some(thread_id)) = (self.client(), self.selected.clone()) else {
            return;
        };
        let mut payload =
            json!({ "threadId": thread_id, "terminalId": self.term_id, "data": data });
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
            if let Some(term) = self.term.as_mut() {
                term.scroll_to_bottom();
            }
            self.terminal_write(bytes, false, cx);
            cx.stop_propagation();
        }
    }

    pub(crate) fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mono = cx.theme().mono_font_family.clone();
        let mono_family = mono.clone();
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
                    .children(
                        self.selected
                            .iter()
                            .flat_map(|thread| self.terminal_tabs(thread))
                            .enumerate()
                            .map(|(i, (id, label, busy))| {
                                let current = id == self.term_id;
                                let (switch_id, close_id) = (id.clone(), id.clone());
                                h_flex()
                                    .id(("terminal-tab", i))
                                    .gap_1()
                                    .px_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(current, |t| t.bg(c.secondary).text_color(c.foreground))
                                    .when(busy, |t| t.child(div().text_color(c.info).child("●")))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.switch_terminal(switch_id.clone(), cx)
                                    }))
                                    .child(
                                        Button::new(("terminal-tab-close", i))
                                            .xsmall()
                                            .ghost()
                                            .label("×")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.close_terminal_tab(close_id.clone(), cx)
                                            })),
                                    )
                            }),
                    )
                    .child(
                        Button::new("terminal-new")
                            .xsmall()
                            .ghost()
                            .label("+")
                            .tooltip(t("New terminal"))
                            .on_click(cx.listener(|this, _, _, cx| this.new_terminal(cx))),
                    )
                    .children(self.term_status.clone())
                    .child(div().flex_1())
                    .child(
                        Button::new("term-clear")
                            .xsmall()
                            .ghost()
                            .label(t("Clear"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(id) = thread_id.clone() {
                                    let payload =
                                        json!({ "threadId": id, "terminalId": this.term_id });
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
                            .label(t("Hide"))
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
                    .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                        let lines = match e.delta {
                            ScrollDelta::Lines(d) => d.y.round() as i32,
                            ScrollDelta::Pixels(d) => (f32::from(d.y) / 16.0).round() as i32,
                        };
                        if let Some(term) = this.term.as_mut()
                            && lines != 0
                        {
                            term.scroll(lines);
                            cx.notify();
                        }
                    }))
                    .child({
                        // Measures the drawer after layout; the grid follows its size.
                        let app = cx.entity().downgrade();
                        let font = gpui_kit::font(mono_family.clone());
                        canvas(
                            move |bounds, window, cx| {
                                let id = window.text_system().resolve_font(&font);
                                let cell = window
                                    .text_system()
                                    .advance(id, px(12.), 'm')
                                    .map_or(7.2, |s| f32::from(s.width));
                                let cols = ((f32::from(bounds.size.width) / cell).floor() as usize)
                                    .clamp(20, 1000);
                                let rows = ((f32::from(bounds.size.height) / 16.0).floor()
                                    as usize)
                                    .clamp(4, 500);
                                let app = app.clone();
                                cx.defer(move |cx| {
                                    app.update(cx, |this, cx| this.fit_terminal(cols, rows, cx))
                                        .ok();
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
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
