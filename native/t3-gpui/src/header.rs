//! The thread header (web: `ChatHeader.tsx`, `PanelLayoutControls.tsx`), the thread details card
//! it toggles (`ThreadDetailsPanel.tsx`), and the right panel's surface picker (`RightPanelTabs`).

use crate::i18n::t;
use crate::model::ThreadShell;
use crate::{T3App, ui};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// `--workspace-topbar-height`.
pub const TOPBAR_HEIGHT: f32 = 52.;

/// A ghost toggle (web: `Toggle variant="ghost" size="sm"`), filled while pressed.
fn toggle(id: &'static str, icon: &str, pressed: bool, cx: &App) -> Stateful<Div> {
    let c = cx.theme().colors;
    ui::icon_button(id, icon, cx)
        .size(px(32.))
        .when(pressed, |b| b.bg(c.accent).text_color(c.foreground))
}

impl T3App {
    /// The right panel shows one surface; with none open it offers the picker.
    pub(crate) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        if self.diff_open || self.files.open || self.surfaces_open {
            self.diff_open = false;
            self.files.open = false;
            self.surfaces_open = false;
        } else {
            self.surfaces_open = true;
        }
        cx.notify();
    }

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
        let right_open = self.diff_open || self.files.open || self.surfaces_open;
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
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        toggle("terminal-toggle", "panel-bottom", self.term.is_some(), cx)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)),
                            ),
                    )
                    .child(
                        toggle("right-panel-toggle", "panel-right", right_open, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_right_panel(cx))),
                    ),
            )
    }

    /// The details card under the header: where the thread runs and what to do with its work.
    /// shortcut: hosts the existing controls as-is; their rows are not yet restyled like the web's.
    pub(crate) fn render_thread_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let env = self.env();
        let row = || h_flex().min_h(px(36.)).px_3().gap_2().items_center();
        v_flex()
            .id("thread-details-card")
            .absolute()
            .top(px(TOPBAR_HEIGHT))
            .right(px(12.))
            .w(px(320.))
            .py_1()
            .rounded(px(16.))
            .border_1()
            .border_color(c.border)
            .bg(c.popover)
            .shadow_lg()
            .occlude()
            .child(
                row()
                    .child(
                        ui::icon(ui::machine_icon(env.and_then(|e| e.machine.as_deref())))
                            .size(px(16.))
                            .text_color(c.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .children(env.and_then(|e| e.name.clone())),
                    ),
            )
            .child(row().child(self.render_open_in(cx)))
            .child(row().child(self.render_scripts(cx)))
            .child(div().my_1().h_px().bg(c.border))
            .child(row().child(self.render_git_toolbar(cx)))
            .child(
                row()
                    .child(self.render_pull_requests(cx))
                    .child(self.render_issues(cx)),
            )
            .child(
                row()
                    .id("details-changes")
                    .cursor_pointer()
                    .hover(|s| s.bg(c.accent))
                    .child(
                        ui::icon("file-diff")
                            .size(px(16.))
                            .text_color(c.muted_foreground),
                    )
                    .child(div().text_sm().child(t("Changes")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.details_open = false;
                        if !this.diff_open {
                            this.toggle_diff(cx);
                        }
                    })),
            )
    }

    /// The empty right panel (web: "Open a surface"), listing the surfaces this client has.
    /// shortcut: no single-key picks (the web's T/F/D badges) until the picker takes focus.
    pub(crate) fn render_surface_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let item = |id: &'static str, icon: &'static str, label: &str| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px_2()
                .gap_3()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(c.accent))
                .child(ui::icon(icon).size(px(16.)))
                .child(div().flex_1().text_sm().child(t(label)))
        };
        v_flex()
            .w(px(440.))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(c.border)
            .child(div().h(px(TOPBAR_HEIGHT)).flex_none())
            .child(
                v_flex().flex_1().items_center().justify_center().child(
                    v_flex()
                        .w(px(300.))
                        .gap_1()
                        .child(
                            div()
                                .mb_3()
                                .text_center()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(t("Open a surface")),
                        )
                        .child(
                            item("surface-terminal", "square-terminal", "Terminal").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.surfaces_open = false;
                                    if this.term.is_none() {
                                        this.toggle_terminal(window, cx);
                                    }
                                }),
                            ),
                        )
                        .child(
                            item("surface-files", "files", "Files").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.surfaces_open = false;
                                    this.toggle_files(cx);
                                },
                            )),
                        )
                        .child(
                            item("surface-diff", "file-diff", "Diff").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.surfaces_open = false;
                                    this.toggle_diff(cx);
                                },
                            )),
                        ),
                ),
            )
    }
}
