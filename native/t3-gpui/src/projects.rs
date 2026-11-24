//! Add a project on the active environment (web: `CommandPaletteAddProject.tsx`). The folder is on
//! the server's machine, so suggestions come from `filesystem.browse`, never the local disk.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;

use crate::{T3App, on_tokio};

const BROWSE_DEBOUNCE: Duration = Duration::from_millis(150);

/// `inferProjectTitleFromPath`: the last path segment.
pub fn title_from_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

impl T3App {
    pub(crate) fn open_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_project = true;
        self.settings_open = false;
        self.project_suggestions.clear();
        self.project_path.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Suggests folders under what was typed, from the server's filesystem.
    pub(crate) fn browse_project_path(&mut self, cx: &mut Context<Self>) {
        self.browse_generation += 1;
        let generation = self.browse_generation;
        let partial = self.project_path.read(cx).value().trim().to_string();
        let Some(client) = self.client().filter(|_| !partial.is_empty()) else {
            self.project_suggestions.clear();
            return cx.notify();
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BROWSE_DEBOUNCE).await;
            if this
                .update(cx, |app, _| app.browse_generation != generation)
                .unwrap_or(true)
            {
                return;
            }
            let result = on_tokio(async move {
                client
                    .call("filesystem.browse", json!({ "partialPath": partial }))
                    .await
            })
            .await;
            this.update(cx, |app, cx| {
                if app.browse_generation == generation {
                    app.project_suggestions = match result {
                        Ok(Ok(r)) => r["entries"].as_array().cloned().unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self
            .project_path
            .read(cx)
            .value()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let (Some(client), Some(env)) = (self.client(), self.env().map(|e| e.key.clone())) else {
            return;
        };
        if path.is_empty() {
            return;
        }
        let project_id = uuid::Uuid::new_v4().to_string();
        let mutation = json!({
            "type": "project.create",
            "commandId": uuid::Uuid::new_v4().to_string(),
            "projectId": project_id,
            "title": title_from_path(&path),
            "workspaceRoot": path,
            "createWorkspaceRootIfMissing": true,
            "defaultModelSelection": null,
        });
        self.project_status = Some("Adding…".into());
        cx.notify();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.mutate", mutation).await }).await;
            let error = match result {
                Ok(Ok(project)) => {
                    let id = project["id"].as_str().unwrap_or(&project_id).to_owned();
                    cx.update_window(window_handle, |_, window, cx| {
                        this.update(cx, |app, cx| {
                            app.adding_project = false;
                            app.project_status = None;
                            app.new_thread(env, id, window, cx);
                        })
                        .ok();
                    })
                    .ok();
                    return;
                }
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            this.update(cx, |app, cx| {
                app.project_status = Some(format!("Could not add the project: {error}").into());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_add_project(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let server = self.env().map(|e| e.label()).unwrap_or_default();
        v_flex().flex_1().h_full().justify_center().child(
            v_flex()
                .mx_auto()
                .max_w(px(640.))
                .w_full()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Add project"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(c.muted_foreground)
                        .child(format!(
                            "A folder on {server}. It is created if it does not exist."
                        )),
                )
                .child(Input::new(&self.project_path))
                .child(
                    v_flex().children(self.project_suggestions.iter().take(12).enumerate().map(
                        |(i, entry)| {
                            let full: String = entry["fullPath"].as_str().unwrap_or("").to_owned();
                            let name: String = entry["name"].as_str().unwrap_or("").to_owned();
                            div()
                                .id(("project-suggestion", i))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|d| d.bg(c.list_hover))
                                .child(format!("{name}/"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let next = format!("{full}/");
                                    this.project_path.update(cx, |input, cx| {
                                        input.set_value(next.clone(), window, cx)
                                    });
                                    this.browse_project_path(cx);
                                }))
                        },
                    )),
                )
                .children(
                    self.project_status
                        .clone()
                        .map(|s| div().text_sm().text_color(c.muted_foreground).child(s)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("add-project")
                                .primary()
                                .label("Add project")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("add-project-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.adding_project = false;
                                    cx.notify();
                                })),
                        )
                        .when(self.envs.len() > 1, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .child("Switch servers in Settings."),
                            )
                        }),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::title_from_path;

    #[test]
    fn titles_come_from_the_last_segment() {
        assert_eq!(title_from_path("/Users/me/code/web/"), "web");
        assert_eq!(title_from_path("C:\\work\\api"), "api");
        assert_eq!(title_from_path("plain"), "plain");
    }
}
