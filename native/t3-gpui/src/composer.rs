//! Composer controls: model, permission mode, plan mode (web: `ComposerControlsBar.tsx`).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;
use std::collections::HashMap;

use crate::T3App;

/// `runtimeModeConfig.ts` labels, in the web's order.
pub(crate) const RUNTIME_MODES: [(&str, &str); 4] = [
    ("approval-required", "Supervised"),
    ("auto-accept-edits", "Auto-accept edits"),
    ("auto", "Auto"),
    ("full-access", "Full access"),
];

fn runtime_label(mode: &str) -> &str {
    RUNTIME_MODES
        .iter()
        .find(|(m, _)| *m == mode)
        .map_or(mode, |(_, l)| l)
}

impl T3App {
    /// (runtime mode, interaction mode) of the draft or the open thread.
    pub(crate) fn current_modes(&self) -> (String, String) {
        if self.draft_project.is_some() {
            return (
                self.draft_runtime_mode.clone(),
                self.draft_interaction_mode.clone(),
            );
        }
        self.selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .map(|t| (t.runtime_mode.clone(), t.interaction_mode.clone()))
            .unwrap_or_default()
    }

    fn set_runtime_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.draft_project.is_some() {
            self.draft_runtime_mode = mode.to_owned();
            return cx.notify();
        }
        if let Some(thread_id) = self.selected.clone() {
            self.dispatch(json!({ "type": "thread.runtime-mode.set", "threadId": thread_id, "runtimeMode": mode }), cx);
        }
    }

    pub(crate) fn set_interaction_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.draft_project.is_some() {
            self.draft_interaction_mode = mode.to_owned();
            return cx.notify();
        }
        if let Some(thread_id) = self.selected.clone() {
            self.dispatch(
                json!({ "type": "thread.interaction-mode.set", "threadId": thread_id, "interactionMode": mode }),
                cx,
            );
        }
    }

    /// The permission modes the current model's provider offers, in the web's order.
    fn supported_runtime_modes(&self) -> Vec<(&'static str, &'static str)> {
        let provider = self.current_model().and_then(|m| {
            self.config()
                .providers
                .iter()
                .find(|p| m["instanceId"] == p.instance_id.as_str())
                .cloned()
        });
        RUNTIME_MODES
            .into_iter()
            .filter(|(mode, _)| {
                provider
                    .as_ref()
                    .and_then(|p| p.supported_runtime_modes.as_ref())
                    .is_none_or(|m| m.iter().any(|s| s == mode))
            })
            .collect()
    }
}

/// `runtimeModeConfig.ts` icons.
fn runtime_icon(mode: &str) -> &'static str {
    match mode {
        "auto-accept-edits" => "pen-line",
        "auto" => "shield-check",
        "full-access" => "shield-off",
        _ => "lock",
    }
}

impl T3App {
    /// The composer card (web: `ComposerSurface` + `ChatComposer`): the prompt, then a footer with
    /// attach, permissions, plan, model and send.
    pub(crate) fn render_composer_card(
        &self,
        running: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = cx.theme().colors;
        let web = crate::web_theme::current(cx);
        let (runtime, interaction) = self.current_modes();
        let default_runtime = self.config().settings["defaultRuntimeMode"]
            .as_str()
            .unwrap_or("full-access")
            .to_owned();
        let empty = self.composer.read(cx).value().trim().is_empty() && self.attachments.is_empty();
        let app = cx.entity().downgrade();
        let chip = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px_2()
                .gap(px(6.))
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(c.foreground.opacity(0.8))
                .hover(|s| s.bg(c.accent).text_color(c.foreground))
        };
        let supported = self.supported_runtime_modes();
        let show_plan = self.current_model().is_none_or(|m| {
            self.config()
                .providers
                .iter()
                .find(|p| m["instanceId"] == p.instance_id.as_str())
                .is_none_or(|p| p.show_interaction_mode_toggle != Some(false))
        });
        v_flex()
            .w_full()
            .rounded(px(16.))
            .border_1()
            .border_color(web.composer_outline)
            .bg(web.composer)
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .min_h(px(58.))
                    .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        this.composer_history_key(e, window, cx)
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.add_attachments(paths.paths().to_vec(), cx)
                    }))
                    .child(Textarea::new(&self.composer).appearance(false)),
            )
            .child(
                h_flex()
                    .px(px(10.))
                    .pb(px(10.))
                    .gap_1()
                    .child(
                        crate::ui::icon_button("composer-attach", "paperclip", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_attachments(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("runtime-mode")
                            .ghost()
                            .small()
                            .h(px(28.))
                            .icon(crate::ui::icon(runtime_icon(&runtime)))
                            .when(runtime != default_runtime, |b| {
                                b.label(t(runtime_label(&runtime)))
                            })
                            .dropdown_menu(move |mut menu, _, _| {
                                for (mode, label) in supported.clone() {
                                    let app = app.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(t(label))
                                            .checked(mode == runtime)
                                            .on_click(move |_, _, cx| {
                                                app.update(cx, |this, cx| {
                                                    this.set_runtime_mode(mode, cx)
                                                })
                                                .ok();
                                            }),
                                    );
                                }
                                menu
                            }),
                    )
                    .when(show_plan && interaction == "plan", |row| {
                        row.child(
                            chip("plan-mode")
                                .bg(c.accent)
                                .child(crate::ui::icon("list-todo").size(px(16.)))
                                .child(t("Plan"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_interaction_mode("default", cx)
                                })),
                        )
                    })
                    .child(
                        div()
                            .w(px(200.))
                            .child(Select::new(&self.model_picker).small().appearance(false)),
                    )
                    .child(if running && empty {
                        div()
                            .id("composer-stop")
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(c.danger.opacity(0.9))
                            .hover(|s| s.bg(c.danger))
                            .child(div().size(px(8.)).rounded(px(1.5)).bg(white()))
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                    } else {
                        div()
                            .id("composer-send")
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(empty, |b| {
                                b.bg(c.foreground.opacity(0.12))
                                    .text_color(c.muted_foreground)
                            })
                            .when(!empty, |b| {
                                b.cursor_pointer()
                                    .bg(c.primary)
                                    .text_color(c.primary_foreground)
                                    .hover(|s| s.bg(c.primary_hover))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send(false, window, cx)
                                    }))
                            })
                            .child(crate::ui::icon("arrow-up").size(px(16.)))
                    }),
            )
    }
}

/// `resolveComposerDispatchMode`: "auto" when idle; while a turn runs, the follow-up default
/// ("steer" or "queue"), swapped by the alternate send (mod+Enter).
pub fn dispatch_mode(running: bool, alternate: bool, default: &str) -> &'static str {
    let steer = default != "queue";
    match (running, steer != alternate) {
        (false, _) => "auto",
        (true, true) => "steer",
        (true, false) => "queue",
    }
}

/// `message.dispatch` delivery fields for a mode, as the web sends them.
pub fn delivery(mode: &str) -> serde_json::Value {
    if mode == "queue" {
        json!({ "dispatchMode": { "type": "queue_after_active" } })
    } else {
        json!({ "dispatchMode": { "type": "start_immediately" }, "deliveryIntent": mode })
    }
}

/// The composer draft's owner: the open thread, or a new thread in a project.
pub fn draft_key(env: &str, thread: Option<&str>, draft_project: Option<&str>) -> Option<String> {
    match (thread, draft_project) {
        (_, Some(project)) => Some(format!("{env}\u{0}new:{project}")),
        (Some(thread), None) => Some(format!("{env}\u{0}{thread}")),
        (None, None) => None,
    }
}

fn drafts_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config/t3-gpui/drafts.json"))
}

pub fn load_drafts() -> HashMap<String, String> {
    drafts_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

impl T3App {
    /// Writes every unsent draft, including the one the composer holds now.
    pub(crate) fn save_drafts(&self, cx: &App) {
        let Some(path) = drafts_path() else { return };
        let mut drafts = self.drafts.clone();
        let text = match &self.queued_edit {
            Some(edit) => edit.draft.clone(),
            None => self.composer.read(cx).value().to_string(),
        };
        if let Some(owner) = &self.composer_owner
            && !text.trim().is_empty()
        {
            drafts.insert(owner.clone(), text);
        }
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, serde_json::to_vec(&drafts).unwrap());
    }

    /// Keeps unsent text per thread: on a switch, the old owner's text is stashed and the new
    /// owner's is restored. Runs in render, which has the window `set_value` needs. Drafts are
    /// written on each switch and on quit, so a crash loses at most the open draft's edits.
    /// shortcut: attachments are not kept per thread.
    pub(crate) fn sync_composer_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
        let key = draft_key(
            &env,
            self.selected.as_deref(),
            self.draft_project.as_deref(),
        );
        if key == self.composer_owner {
            return;
        }
        // Leaving a thread mid-edit keeps its draft, not the queued text.
        let text = match self.queued_edit.take() {
            Some(edit) => edit.draft,
            None => self.composer.read(cx).value().to_string(),
        };
        if let Some(old) = self.composer_owner.take() {
            if text.trim().is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, text);
            }
        }
        let restored = key
            .as_ref()
            .and_then(|k| self.drafts.remove(k))
            .unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_value(restored, window, cx));
        self.composer_owner = key;
        self.save_drafts(cx);
    }
}

#[cfg(test)]
mod draft_tests {
    use super::{delivery, dispatch_mode, draft_key};

    #[test]
    fn steers_or_queues_follow_ups_like_the_web() {
        assert_eq!(dispatch_mode(false, false, "steer"), "auto");
        assert_eq!(dispatch_mode(true, false, "steer"), "steer");
        assert_eq!(dispatch_mode(true, true, "steer"), "queue");
        assert_eq!(dispatch_mode(true, false, "queue"), "queue");
        assert_eq!(dispatch_mode(true, true, "queue"), "steer");
        assert_eq!(
            delivery("queue")["dispatchMode"]["type"],
            "queue_after_active"
        );
        assert_eq!(delivery("steer")["deliveryIntent"], "steer");
        assert_eq!(
            delivery("auto")["dispatchMode"]["type"],
            "start_immediately"
        );
    }

    #[test]
    fn keys_drafts_by_owner() {
        assert_eq!(draft_key("e", Some("t"), None).as_deref(), Some("e\u{0}t"));
        assert_eq!(
            draft_key("e", Some("t"), Some("p")).as_deref(),
            Some("e\u{0}new:p"),
            "a new-thread draft wins"
        );
        assert_ne!(
            draft_key("a", Some("t"), None),
            draft_key("b", Some("t"), None)
        );
        assert_eq!(draft_key("e", None, None), None);
    }
}
