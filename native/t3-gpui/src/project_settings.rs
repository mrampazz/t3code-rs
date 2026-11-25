//! Project settings (web: `settings/ProjectSettingsPanel`, `ProjectDefaultsSettings`): per project,
//! rename, new-thread mode, auto-pull, pin, archive and remove, all through `projects.mutate`.
//! shortcut: no icon, tracked folders or per-project provider overrides yet.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt, dialog::DialogButtonProps, h_flex, v_flex,
};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// A `project.update` mutation with the given fields.
pub fn update(project_id: &str, fields: Value) -> Value {
    let mut command = json!({ "type": "project.update", "commandId": uuid::Uuid::new_v4().to_string(), "projectId": project_id });
    command
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().cloned().unwrap_or_default());
    command
}

impl T3App {
    fn mutate_project(&mut self, mutation: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.mutate", mutation).await }).await;
            if let Some(e) = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            } {
                this.update(cx, |app, cx| {
                    app.settings_status =
                        Some(format!("{}: {e}", t("Could not update the project")).into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn rename_project(
        &mut self,
        id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, id) = (app.clone(), input.clone(), id.clone());
            dialog
                .title(t("Rename project"))
                .child(Input::new(&input))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Rename"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let title = input.read(cx).value().trim().to_string();
                    if title.is_empty() {
                        return false;
                    }
                    app.update(cx, |this, cx| {
                        this.mutate_project(update(&id, json!({ "title": title })), cx)
                    })
                    .ok();
                    true
                })
        });
    }

    pub(crate) fn render_project_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let app = cx.entity().downgrade();
        v_flex().children(self.shell().projects.iter().enumerate().map(|(i, p)| {
            let (id, title, raw) = (p.id.clone(), p.title.clone(), p.raw.clone());
            let pinned = !raw["pinnedAt"].is_null();
            let archived = !raw["archivedAt"].is_null();
            let auto_pull = raw["autoPull"] == true;
            let mode = raw["defaultThreadEnvMode"].as_str().map(str::to_owned);
            let app = app.clone();
            h_flex()
                .py_2()
                .gap_2()
                .border_b_1()
                .border_color(c.border)
                .text_sm()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(format!("{title}{}", if archived { format!(" · {}", t("Archived")) } else { String::new() }))
                        .child(div().text_xs().text_color(c.muted_foreground).truncate().child(p.workspace_root.clone())),
                )
                .child(Button::new(("project-actions", i)).small().outline().label(t("Edit")).dropdown_menu(move |menu, _, _| {
                    let act = |label: String, fields: Value| {
                        let (app, id) = (app.clone(), id.clone());
                        PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            app.update(cx, |this, cx| this.mutate_project(update(&id, fields.clone()), cx)).ok();
                        })
                    };
                    let rename = {
                        let (app, id, title) = (app.clone(), id.clone(), title.clone());
                        PopupMenuItem::new(t("Rename…")).on_click(move |_, window, cx| {
                            app.update(cx, |this, cx| this.rename_project(id.clone(), title.clone(), window, cx)).ok();
                        })
                    };
                    let remove = {
                        let (app, id) = (app.clone(), id.clone());
                        PopupMenuItem::new(t("Remove project")).on_click(move |_, _, cx| {
                            let delete = json!({ "type": "project.delete", "commandId": uuid::Uuid::new_v4().to_string(), "projectId": id });
                            app.update(cx, |this, cx| this.mutate_project(delete, cx)).ok();
                        })
                    };
                    menu.item(rename)
                        .separator()
                        .item(act(t("Local"), json!({ "defaultThreadEnvMode": "local" })).checked(mode.as_deref() == Some("local")))
                        .item(act(t("New worktree"), json!({ "defaultThreadEnvMode": "worktree" })).checked(mode.as_deref() == Some("worktree")))
                        .item(act(t("Use the default"), json!({ "defaultThreadEnvMode": null })).checked(mode.is_none()))
                        .separator()
                        .item(act(t("Automatically pull"), json!({ "autoPull": !auto_pull })).checked(auto_pull))
                        .item(act(if pinned { t("Unpin project") } else { t("Pin project") }, json!({ "pinned": !pinned })))
                        .item(act(if archived { t("Unarchive") } else { t("Archive") }, json!({ "archived": !archived })))
                        .separator()
                        .item(remove)
                }))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::update;
    use serde_json::json;

    #[test]
    fn builds_project_updates() {
        let m = update("p", json!({ "pinned": true }));
        assert_eq!(
            (
                m["type"].as_str(),
                m["projectId"].as_str(),
                m["pinned"].as_bool()
            ),
            (Some("project.update"), Some("p"), Some(true))
        );
        assert!(m["commandId"].is_string());
    }
}
