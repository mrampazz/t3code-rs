//! Command palette (web: `apps/web/src/components/CommandPalette*.tsx`): threads by title,
//! server message search, and commands.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::{t, tv};
use crate::{T3App, on_tokio};

actions!(t3, [ClosePalette]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ClosePalette, Some("Palette"))]);
}

/// Server search waits for the typing to pause.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_ROWS: usize = 12;

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum PaletteAction {
    /// (environment, thread)
    Open(String, String),
    /// (environment, project)
    NewThread(String, String),
    Disconnect,
    Settings,
    AddProject,
    LinkPr,
    CheckoutPr,
    OpenInEditor,
    RunScript(String),
    /// A keybinding command (`run_command`).
    Command(&'static str),
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteRow {
    pub label: String,
    pub detail: String,
    pub action: PaletteAction,
    pub shortcut: Option<String>,
}

impl T3App {
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        self.palette_matches.clear();
        self.palette_index = 0;
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
                if let Some(row) = self.palette_rows(cx).into_iter().nth(self.palette_index) {
                    self.run_palette(row.action, window, cx);
                }
            }
            InputEvent::Change => {
                self.palette_generation += 1;
                self.palette_index = 0;
                let generation = self.palette_generation;
                let query = self.palette_input.read(cx).value().trim().to_string();
                self.palette_matches.clear();
                cx.notify();
                let Some(client) = self.client().filter(|_| query.chars().count() >= 2) else {
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
            self.shell()
                .threads
                .iter()
                .chain(&self.shell().archived)
                .find(|t| t.id == id)
                .map(|t| t.title.clone())
        };
        let shells: Vec<&crate::model::Shell> = self.envs.iter().map(|e| &e.shell).collect();
        let active = self.env().map(|e| e.key.clone()).unwrap_or_default();
        let mut rows = palette_rows(&shells, &self.palette_matches, &active, &query, title_of);
        let thread = self.selected.is_some();
        let mut commands = vec![];
        if thread {
            commands.push((
                "Toggle diff".to_owned(),
                PaletteAction::Command("diff.toggle"),
            ));
            commands.push((
                t("Find in current thread"),
                PaletteAction::Command("chat.find"),
            ));
            if let Some((_, pr)) = self.thread_reference() {
                let label = t(if pr { "Copy PR link" } else { "Copy thread" });
                commands.push((label, PaletteAction::Command("thread.copyReference")));
            }
            commands.push((t("Go to file"), PaletteAction::Command("filePicker.toggle")));
            commands.push((
                t("Search project contents"),
                PaletteAction::Command("projectSearch.toggle"),
            ));
            commands.push((t("Link pull request to thread"), PaletteAction::LinkPr));
            if let Some(editor) = self.favorite_editor() {
                let label = crate::editors::label(&editor);
                commands.push((
                    tv("Open in {app}", &[("app", &label)]),
                    PaletteAction::OpenInEditor,
                ));
            }
            for (id, name) in self.script_names() {
                commands.push((
                    tv("Run {name}", &[("name", &name)]),
                    PaletteAction::RunScript(id),
                ));
            }
        }
        if !self.shell().projects.is_empty() {
            commands.push((t("Check out pull request"), PaletteAction::CheckoutPr));
        }
        commands.push((t("Add project"), PaletteAction::AddProject));
        commands.push((
            t("Change appearance"),
            PaletteAction::Command("appearance.cycle"),
        ));
        commands.push((t("Open settings"), PaletteAction::Settings));
        commands.push(("Disconnect".to_owned(), PaletteAction::Disconnect));
        let rules = self.keybinding_rules();
        rows.extend(
            commands
                .into_iter()
                .filter(|(label, _)| label.to_lowercase().contains(&query))
                .map(|(label, action)| PaletteRow {
                    label,
                    detail: "Command".into(),
                    shortcut: match &action {
                        PaletteAction::Command(c) => {
                            crate::keybindings::shortcut_label(&rules, c, cfg!(target_os = "macos"))
                        }
                        _ => None,
                    },
                    action,
                }),
        );
        rows.truncate(MAX_ROWS);
        rows
    }

    fn run_palette(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        match action {
            PaletteAction::Open(env, id) => self.select_thread(env, id, cx),
            PaletteAction::NewThread(env, project) => self.new_thread(env, project, window, cx),
            PaletteAction::Command(command) => {
                self.run_command(command, window, cx);
            }
            PaletteAction::Disconnect => self.forget_session(cx),
            PaletteAction::Settings => self.toggle_settings(cx),
            PaletteAction::AddProject => self.open_add_project(window, cx),
            PaletteAction::LinkPr => self.open_link_pr(window, cx),
            PaletteAction::CheckoutPr => self.open_checkout_pr(window, cx),
            PaletteAction::OpenInEditor => self.open_in_editor(None, cx),
            PaletteAction::RunScript(id) => {
                self.run_script(&id, cx);
            }
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
                    // Captured so the input does not take ↑/↓ as caret moves.
                    .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                        let count = this.palette_rows(cx).len();
                        let Some(step) = palette_step(&e.keystroke.key, count) else {
                            return;
                        };
                        this.palette_index = (this.palette_index + step) % count;
                        cx.stop_propagation();
                        cx.notify();
                    }))
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
                                    .when(i == self.palette_index, |r| r.bg(c.list_active))
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
                                    .children(row.shortcut.map(|keys| {
                                        div().text_xs().text_color(c.muted_foreground).child(keys)
                                    }))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_palette(action.clone(), window, cx)
                                    }))
                            })),
                    ),
            )
    }
}

/// How far ↑/↓ move the selection, as a forward step that wraps around `count` rows.
fn palette_step(key: &str, count: usize) -> Option<usize> {
    match key {
        _ if count == 0 => None,
        "down" => Some(1),
        "up" => Some(count - 1),
        _ => None,
    }
}

/// Thread rows: title matches first (local, instant), then server message matches not already
/// listed, then "New thread in …" per matching project.
pub(crate) fn palette_rows(
    shells: &[&crate::model::Shell],
    matches: &[Value],
    matches_env: &str,
    query: &str,
    title_of: impl Fn(&str) -> Option<String>,
) -> Vec<PaletteRow> {
    let project_title = |shell: &crate::model::Shell, id: &str| {
        shell
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.title.clone())
            .unwrap_or_default()
    };
    let mut rows: Vec<PaletteRow> = shells
        .iter()
        .flat_map(|shell| {
            shell
                .threads
                .iter()
                .chain(&shell.archived)
                .filter(|t| query.is_empty() || t.title.to_lowercase().contains(query))
                .map(|t| PaletteRow {
                    label: t.title.clone(),
                    detail: project_title(shell, &t.project_id),
                    action: PaletteAction::Open(t.env.clone(), t.id.clone()),
                    shortcut: None,
                })
        })
        .collect();
    for m in matches {
        let Some(id) = m["threadId"].as_str() else {
            continue;
        };
        let action = PaletteAction::Open(matches_env.to_owned(), id.to_owned());
        if rows.iter().any(|r| r.action == action) {
            continue;
        }
        rows.push(PaletteRow {
            label: title_of(id).unwrap_or_else(|| "Thread".into()),
            detail: m["snippet"].as_str().unwrap_or("").replace('\n', " "),
            action,
            shortcut: None,
        });
    }
    for shell in shells {
        for p in shell.projects.iter().filter(|p| !p.is_archived()) {
            let label = format!("New thread in {}", p.title);
            if query.is_empty() || label.to_lowercase().contains(query) {
                rows.push(PaletteRow {
                    label,
                    detail: "Command".into(),
                    action: PaletteAction::NewThread(shell.env.clone(), p.id.clone()),
                    shortcut: None,
                });
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{PaletteAction, palette_rows, palette_step};

    #[test]
    fn arrows_wrap() {
        assert_eq!((2 + palette_step("down", 3).unwrap()) % 3, 0);
        assert_eq!(palette_step("up", 3).unwrap() % 3, 2);
        assert_eq!(palette_step("up", 0), None);
        assert_eq!(palette_step("a", 3), None);
    }
    use serde_json::json;

    #[test]
    fn titles_then_message_matches_then_commands() {
        let mut shell = crate::model::Shell::new("http://a");
        shell.apply(
            &json!({"kind": "snapshot", "snapshot": {"snapshotSequence": 1,
            "projects": [{"id": "p", "title": "Web"}],
            "threads": [
                {"id": "a", "projectId": "p", "title": "Fix login bug", "updatedAt": "1"},
                {"id": "b", "projectId": "p", "title": "Refactor", "updatedAt": "2"}]}}),
        );
        let mut other = crate::model::Shell::new("http://b");
        other.apply(
            &json!({"kind": "snapshot", "snapshot": {"snapshotSequence": 1, "projects": [],
            "threads": [{"id": "a", "projectId": "q", "title": "Login on b", "updatedAt": "1"}]}}),
        );
        let matches = vec![
            json!({"threadId": "b", "snippet": "the login\nform"}),
            json!({"threadId": "a", "snippet": "dup"}),
        ];
        let open = |env: &str, id: &str| PaletteAction::Open(env.into(), id.into());
        let rows = palette_rows(&[&shell, &other], &matches, "http://a", "login", |id| {
            (id == "b").then(|| "Refactor".into())
        });
        let actions: Vec<_> = rows.iter().map(|r| r.action.clone()).collect();
        assert_eq!(
            actions,
            [
                open("http://a", "a"),
                open("http://b", "a"),
                open("http://a", "b")
            ],
            "same id, two servers"
        );
        assert_eq!(rows[2].detail, "the login form");
        let all = palette_rows(&[&shell], &[], "http://a", "", |_| None);
        assert_eq!(
            all.last().unwrap().action,
            PaletteAction::NewThread("http://a".into(), "p".into())
        );
    }
}
