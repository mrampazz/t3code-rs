//! Composer controls: model, permission mode, plan mode (web: `ComposerControlsBar.tsx`).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Sizable, h_flex};
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

    pub(crate) fn render_composer_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (runtime, interaction) = self.current_modes();
        let provider = self.current_model().and_then(|m| {
            self.config()
                .providers
                .iter()
                .find(|p| m["instanceId"] == p.instance_id.as_str())
                .cloned()
        });
        let supported: Vec<(&'static str, &'static str)> = RUNTIME_MODES
            .into_iter()
            .filter(|(mode, _)| {
                provider
                    .as_ref()
                    .and_then(|p| p.supported_runtime_modes.as_ref())
                    .is_none_or(|m| m.iter().any(|s| s == mode))
            })
            .collect();
        let show_plan = provider
            .as_ref()
            .is_none_or(|p| p.show_interaction_mode_toggle != Some(false));
        let app = cx.entity().downgrade();
        let plan = interaction == "plan";
        h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(280.))
                    .child(Select::new(&self.model_picker).small()),
            )
            .child(
                Button::new("runtime-mode")
                    .small()
                    .ghost()
                    .label(t(runtime_label(&runtime)))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (mode, label) in supported.clone() {
                            let app = app.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t(label))
                                    .checked(mode == runtime)
                                    .on_click(move |_, _, cx| {
                                        app.update(cx, |this, cx| this.set_runtime_mode(mode, cx))
                                            .ok();
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .when(show_plan, |row| {
                row.child(
                    Button::new("plan-mode")
                        .small()
                        .label(t("Plan"))
                        .when(plan, |b| b.primary())
                        .when(!plan, |b| b.ghost())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_interaction_mode(if plan { "default" } else { "plan" }, cx)
                        })),
                )
            })
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
