//! Composer controls: model, permission mode, plan mode (web: `ComposerControlsBar.tsx`).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;

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
