//! Command palette (web: `apps/web/src/components/CommandPalette*.tsx`): threads by title,
//! server message search, and commands.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::{T3App, on_tokio};

actions!(t3, [TogglePalette, ClosePalette]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", TogglePalette, None),
        KeyBinding::new("ctrl-k", TogglePalette, None),
        KeyBinding::new("escape", ClosePalette, Some("Palette")),
    ]);
}

/// Server search waits for the typing to pause.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_ROWS: usize = 12;

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum PaletteAction {
    Open(String),
    NewThread(String),
    ToggleDiff,
    Disconnect,
    Settings,
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteRow {
    pub label: String,
    pub detail: String,
    pub action: PaletteAction,
}

impl T3App {
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        self.palette_matches.clear();
        if self.palette_open {
            self.palette_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn on_palette_input(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                if let Some(row) = self.palette_rows(cx).into_iter().next() {
                    self.run_palette(row.action, window, cx);
                }
            }
            InputEvent::Change => {
                self.palette_generation += 1;
                let generation = self.palette_generation;
                let query = self.palette_input.read(cx).value().trim().to_string();
                self.palette_matches.clear();
                cx.notify();
                let Some(client) = self.client.clone().filter(|_| query.chars().count() >= 2)
                else {
                    return;
                };
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                    if this
                        .update(cx, |app, _| app.palette_generation != generation)
                        .unwrap_or(true)
                    {
                        return;
                    }
                    let payload = json!({ "query": query, "limit": 20 });
                    let result = on_tokio(async move {
                        client.call("orchestration.searchThreads", payload).await
                    })
                    .await;
                    if let Ok(Ok(result)) = result {
                        this.update(cx, |app, cx| {
                            if app.palette_generation == generation {
                                app.palette_matches =
                                    result["matches"].as_array().cloned().unwrap_or_default();
                                cx.notify();
                            }
                        })
                        .ok();
                    }
                })
                .detach();
            }
            _ => {}
        }
    }

    pub(crate) fn palette_rows(&self, cx: &App) -> Vec<PaletteRow> {
        let query = self.palette_input.read(cx).value().trim().to_lowercase();
        let title_of = |id: &str| {
            self.shell
                .threads
                .iter()
                .chain(&self.shell.archived)
                .find(|t| t.id == id)
                .map(|t| t.title.clone())
        };
        let mut rows = palette_rows(&self.shell, &self.palette_matches, &query, title_of);
        if self.selected.is_some() && "toggle diff".contains(&query) {
            rows.push(PaletteRow {
                label: "Toggle diff".into(),
                detail: "Command".into(),
                action: PaletteAction::ToggleDiff,
            });
        }
        if "settings".contains(&query) {
            rows.push(PaletteRow {
                label: "Settings".into(),
                detail: "Command".into(),
                action: PaletteAction::Settings,
            });
        }
        if "disconnect".contains(&query) {
            rows.push(PaletteRow {
                label: "Disconnect".into(),
                detail: "Command".into(),
                action: PaletteAction::Disconnect,
            });
        }
        rows.truncate(MAX_ROWS);
        rows
    }

    fn run_palette(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        match action {
            PaletteAction::Open(id) => self.select_thread(id, cx),
            PaletteAction::NewThread(project) => self.new_thread(project, window, cx),
            PaletteAction::ToggleDiff => self.toggle_diff(cx),
            PaletteAction::Disconnect => self.forget_session(cx),
            PaletteAction::Settings => self.toggle_settings(cx),
        }
        cx.notify();
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let rows = self.palette_rows(cx);
        div()
            .absolute()
            .inset_0()
            .bg(c.overlay)
            .flex()
            .justify_center()
            .pt(px(96.))
            .key_context("Palette")
            .on_action(cx.listener(|this, _: &ClosePalette, _, cx| {
                this.palette_open = false;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("palette")
                    .w(px(620.))
                    .max_h(px(480.))
                    .rounded_lg()
                    .border_1()
                    .border_color(c.border)
                    .bg(c.popover)
                    .text_color(c.popover_foreground)
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }))
                    .child(div().p_2().child(Input::new(&self.palette_input)))
                    .child(
                        v_flex()
                            .id("palette-rows")
                            .overflow_y_scroll()
                            .pb_2()
                            .children(rows.into_iter().enumerate().map(|(i, row)| {
                                let action = row.action.clone();
                                h_flex()
                                    .id(("palette-row", i))
                                    .px_3()
                                    .py_1p5()
                                    .gap_3()
                                    .cursor_pointer()
                                    .when(i == 0, |r| r.bg(c.list_active))
                                    .hover(|r| r.bg(c.list_hover))
                                    .child(div().flex_1().truncate().text_sm().child(row.label))
                                    .child(
                                        div()
                                            .max_w(px(260.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(c.muted_foreground)
                                            .child(row.detail),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_palette(action.clone(), window, cx)
                                    }))
                            })),
                    ),
            )
    }
}

/// Thread rows: title matches first (local, instant), then server message matches not already
/// listed, then "New thread in …" per matching project.
pub(crate) fn palette_rows(
    shell: &crate::model::Shell,
    matches: &[Value],
    query: &str,
    title_of: impl Fn(&str) -> Option<String>,
) -> Vec<PaletteRow> {
    let project_title = |id: &str| {
        shell
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.title.clone())
            .unwrap_or_default()
    };
    let mut rows: Vec<PaletteRow> = shell
        .threads
        .iter()
        .chain(&shell.archived)
        .filter(|t| query.is_empty() || t.title.to_lowercase().contains(query))
        .map(|t| PaletteRow {
            label: t.title.clone(),
            detail: project_title(&t.project_id),
            action: PaletteAction::Open(t.id.clone()),
        })
        .collect();
    for m in matches {
        let Some(id) = m["threadId"].as_str() else {
            continue;
        };
        if rows
            .iter()
            .any(|r| r.action == PaletteAction::Open(id.to_owned()))
        {
            continue;
        }
        rows.push(PaletteRow {
            label: title_of(id).unwrap_or_else(|| "Thread".into()),
            detail: m["snippet"].as_str().unwrap_or("").replace('\n', " "),
            action: PaletteAction::Open(id.to_owned()),
        });
    }
    for p in &shell.projects {
        let label = format!("New thread in {}", p.title);
        if query.is_empty() || label.to_lowercase().contains(query) {
            rows.push(PaletteRow {
                label,
                detail: "Command".into(),
                action: PaletteAction::NewThread(p.id.clone()),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{PaletteAction, palette_rows};
    use serde_json::json;

    #[test]
    fn titles_then_message_matches_then_commands() {
        let mut shell = crate::model::Shell::default();
        shell.apply(
            &json!({"kind": "snapshot", "snapshot": {"snapshotSequence": 1,
            "projects": [{"id": "p", "title": "Web"}],
            "threads": [
                {"id": "a", "projectId": "p", "title": "Fix login bug", "updatedAt": "1"},
                {"id": "b", "projectId": "p", "title": "Refactor", "updatedAt": "2"}]}}),
        );
        let matches = vec![
            json!({"threadId": "b", "snippet": "the login\nform"}),
            json!({"threadId": "a", "snippet": "dup"}),
        ];
        let rows = palette_rows(&shell, &matches, "login", |id| {
            (id == "b").then(|| "Refactor".into())
        });
        let actions: Vec<_> = rows.iter().map(|r| r.action.clone()).collect();
        assert_eq!(
            actions,
            [
                PaletteAction::Open("a".into()),
                PaletteAction::Open("b".into())
            ]
        );
        assert_eq!(rows[1].detail, "the login form");
        let all = palette_rows(&shell, &[], "", |_| None);
        assert_eq!(
            all.last().unwrap().action,
            PaletteAction::NewThread("p".into())
        );
    }
}
