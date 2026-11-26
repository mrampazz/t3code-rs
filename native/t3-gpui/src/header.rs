//! The thread header (web: `ChatHeader.tsx`, `PanelLayoutControls.tsx`), the thread details card
//! it toggles (`ThreadDetailsPanel.tsx`).

use crate::i18n::t;
use crate::model::ThreadShell;
use crate::{T3App, ui};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// `--workspace-topbar-height`.
pub const TOPBAR_HEIGHT: f32 = 52.;

/// A ghost toggle (web: `Toggle variant="ghost" size="sm"`), filled while pressed.
pub(crate) fn toggle(id: &'static str, icon: &str, pressed: bool, cx: &App) -> Stateful<Div> {
    let c = cx.theme().colors;
    ui::icon_button(id, icon, cx)
        .size(px(32.))
        .when(pressed, |b| b.bg(c.accent).text_color(c.foreground))
}

/// A details card row's icon and label (web: `THREAD_DETAILS_PANEL_ROW_CONTENT_CLASS`).
pub(crate) fn details_content(icon: impl IntoElement, label: impl IntoElement, cx: &App) -> Div {
    let c = cx.theme().colors;
    h_flex()
        .w_full()
        .min_w_0()
        .gap(px(10.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.foreground.opacity(0.8))
        .child(icon)
        .child(label)
}

/// A row icon in the muted tone (web: `THREAD_DETAILS_PANEL_ICON_CLASS`).
pub(crate) fn details_icon(name: &str, cx: &App) -> Icon {
    ui::icon(name)
        .size(px(16.))
        .flex_none()
        .text_color(cx.theme().colors.muted_foreground)
}

/// A truncating row label.
pub(crate) fn details_label(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
}

/// One details card control (web: `ThreadDetailsControl part="row"`): 32px, soft hover fill.
pub(crate) fn details_row(
    id: impl Into<ElementId>,
    icon: &str,
    label: impl IntoElement,
    cx: &App,
) -> Button {
    details_row_with(id, details_icon(icon, cx), label, cx)
}

/// [`details_row`] with its own icon, e.g. a pull request's state color.
pub(crate) fn details_row_with(
    id: impl Into<ElementId>,
    icon: impl IntoElement,
    label: impl IntoElement,
    cx: &App,
) -> Button {
    Button::new(id)
        .ghost()
        .w_full()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(8.))
        .justify_start()
        .child(details_content(icon, label, cx))
}

/// A row without an action (web: `THREAD_DETAILS_PANEL_LOCKED_ROW_CLASS`).
pub(crate) fn details_static_row(icon: &str, label: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .w_full()
        .h(px(32.))
        .px(px(10.))
        .gap(px(10.))
        .child(details_content(details_icon(icon, cx), label, cx))
}

/// A row with a chevron menu beside its action (web: `THREAD_DETAILS_PANEL_SPLIT_*`); the
/// hairline between them shows only while the pair is hovered.
pub(crate) fn details_split(
    id: &'static str,
    primary: Button,
    menu: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    cx: &App,
) -> Div {
    let c = cx.theme().colors;
    h_flex()
        .group(id)
        .w_full()
        .child(primary.flex_1().min_w_0().rounded_r(px(0.)))
        .child(
            div()
                .w_px()
                .h(px(16.))
                .flex_none()
                .group_hover(id, |s| s.bg(c.border)),
        )
        .child(
            Button::new((id, 1usize))
                .ghost()
                .w(px(32.))
                .h(px(32.))
                .flex_none()
                .rounded_l(px(0.))
                .rounded_r(px(8.))
                .child(
                    ui::icon("chevron-down")
                        .size(px(16.))
                        .text_color(c.muted_foreground),
                )
                .dropdown_menu_with_anchor(Anchor::TopRight, menu),
        )
}

impl T3App {
    pub(crate) fn render_thread_header(
        &self,
        thread: &ThreadShell,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = cx.theme().colors;
        let project = self
            .shell()
            .projects
            .iter()
            .find(|p| p.id == thread.project_id)
            .cloned();
        let separator = || div().text_color(c.muted_foreground).child("/");
        h_flex()
            .h(px(TOPBAR_HEIGHT))
            .flex_none()
            .pl(px(20.))
            .pr(px(12.))
            .gap_3()
            .when(self.sidebar_hidden, |row| {
                row.child(
                    ui::icon_button("sidebar-open", "panel-left-open", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.sidebar_hidden = false;
                            cx.notify();
                        },
                    )),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .text_sm()
                    .children(project.map(|p| {
                        let (env, id) = (thread.env.clone(), p.id.clone());
                        h_flex()
                            .id("header-project")
                            .flex_shrink(1.)
                            .min_w_0()
                            .gap(px(6.))
                            .cursor_pointer()
                            .text_color(c.muted_foreground)
                            .hover(|s| s.text_color(c.foreground))
                            .child(ui::project_mark(&p.title, px(14.), cx))
                            .child(div().truncate().child(p.title.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_draft(env.clone(), id.clone(), window, cx)
                            }))
                    }))
                    .child(separator())
                    .children(thread.parent_id().map(|parent| {
                        let (env, parent) = (thread.env.clone(), parent.to_owned());
                        let title = self
                            .shell()
                            .threads
                            .iter()
                            .find(|t| t.id == parent)
                            .map_or_else(|| t("Parent thread"), |t| t.title.clone());
                        h_flex()
                            .gap_3()
                            .min_w_0()
                            .child(
                                div()
                                    .id("header-parent")
                                    .truncate()
                                    .cursor_pointer()
                                    .text_color(c.muted_foreground)
                                    .hover(|s| s.text_color(c.foreground))
                                    .child(title)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_thread(env.clone(), parent.clone(), cx)
                                    })),
                            )
                            .child(separator())
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_10()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(thread.title.clone()),
                    ),
            )
            .child(self.render_guest_share(cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        toggle("thread-details", "square-menu", self.details_open, cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.details_open = !this.details_open;
                                this.refresh_changes_preview(cx);
                                cx.notify();
                            }),
                        ),
                    )
                    .when(!self.panel_visible(), |row| {
                        row.child(
                            toggle("terminal-toggle", "panel-bottom", self.term.is_some(), cx)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_terminal_place(window, cx)
                                })),
                        )
                        .child(
                            toggle("right-panel-toggle", "panel-right", false, cx).on_click(
                                cx.listener(|this, _, _, cx| this.toggle_right_panel(cx)),
                            ),
                        )
                    }),
            )
    }

    /// The details card under the header (web: `ThreadDetailsPanel`): a Workspace section (where
    /// the thread runs, open in editor, project scripts) and a Version Control one (branch, pull
    /// requests, issues, git action, changes).
    pub(crate) fn render_thread_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let env = self.env();
        let worktree = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .is_some_and(|t| t.worktree_path.is_some());
        let section = || v_flex().px_2().pt_2().pb(px(10.));
        let workspace = section()
            .child(
                details_static_row(
                    ui::machine_icon(env.and_then(|e| e.machine.as_deref())),
                    details_label(env.and_then(|e| e.name.clone()).unwrap_or_default()),
                    cx,
                )
                .child(
                    div()
                        .id("details-env-mode")
                        .flex_none()
                        .text_color(c.muted_foreground)
                        .child(
                            ui::icon(if worktree { "folder-git-2" } else { "folder" })
                                .size(px(16.)),
                        )
                        .tooltip(move |window, cx| {
                            Tooltip::new(t(if worktree { "Worktree" } else { "Local" }))
                                .build(window, cx)
                        }),
                ),
            )
            .child(self.render_open_in(cx))
            .child(self.render_scripts(cx));
        let version_control = section()
            .border_t_1()
            .border_color(c.border.opacity(0.65))
            .child(self.render_git_toolbar(cx))
            .child(self.render_pull_requests(cx))
            .child(self.render_issues(cx))
            .child(self.render_git_action(cx))
            .child(self.render_changes_row(cx));
        v_flex()
            .id("thread-details-card")
            .absolute()
            .top(px(TOPBAR_HEIGHT))
            .right(px(12.))
            .w(px(280.))
            .rounded(px(16.))
            .border_1()
            .border_color(c.border)
            .bg(c.popover)
            .shadow_lg()
            .occlude()
            .child(workspace)
            .when(self.git.is_repo() || self.thread_has_links(), |card| {
                card.child(version_control)
            })
    }
}
