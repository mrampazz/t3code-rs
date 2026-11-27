//! The right panel (web: `RightPanelTabs.tsx`, `ChatRightPanel.tsx`, `PanelLayoutControls.tsx`):
//! a tab per open surface with close, reorder and a `+` menu, the maximize toggle, and the
//! "Open a surface" picker with its single-key picks. Tabs are remembered per thread.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::T3App;
use crate::header::{TOPBAR_HEIGHT, toggle};
use crate::i18n::t;
use crate::panel::{Panel, Surface};
use crate::ui;

/// Width of the right panel (web: its default inline size).
pub const RIGHT_PANEL_WIDTH: f32 = 540.;

/// The surfaces this client offers, in the web's picker order, with their single-key picks.
const PICKS: [(Surface, &str, &str, &str); 3] = [
    (Surface::Terminal, "square-terminal", "Terminal", "T"),
    (Surface::Files, "files", "Files", "F"),
    (Surface::Diff, "file-diff", "Diff", "D"),
];

/// The surface a pressed key picks (web: `surfaceShortcutActionForKey`).
pub fn pick_for_key(key: &str) -> Option<Surface> {
    PICKS
        .iter()
        .find(|(.., k)| k.eq_ignore_ascii_case(key))
        .map(|(s, ..)| s.clone())
}

/// What a tab carries while dragged; also renders the drag ghost.
#[derive(Clone)]
struct DraggedTab {
    surface: Surface,
    title: SharedString,
    icon: &'static str,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        h_flex()
            .h(px(24.))
            .px_2()
            .gap_1()
            .rounded_md()
            .bg(c.accent)
            .text_xs()
            .child(ui::icon(self.icon).size(px(12.)))
            .child(self.title.clone())
    }
}

/// A hint key (web: `Kbd`).
fn kbd(key: &str, cx: &App) -> Div {
    let c = cx.theme().colors;
    div()
        .h(px(20.))
        .min_w(px(20.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(c.foreground.opacity(0.07))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.muted_foreground)
        .child(key.to_owned())
}

impl T3App {
    /// The panel shows beside (or, maximized, instead of) the selected thread.
    pub(crate) fn panel_visible(&self) -> bool {
        self.panel.open && self.selected.is_some()
    }

    /// The thread's terminal lives in a panel tab instead of the bottom drawer.
    pub(crate) fn terminal_in_panel(&self) -> bool {
        self.panel.surfaces.contains(&Surface::Terminal)
    }

    fn panel_key(&self) -> Option<String> {
        Some(format!(
            "{}\u{0}{}",
            self.env()?.key,
            self.selected.as_ref()?
        ))
    }

    /// Saves the thread's tabs, as the web's persisted right panel store does.
    /// shortcut: entries of deleted threads stay in prefs; prune if prefs grow noticeably.
    pub(crate) fn remember_panel(&mut self) {
        let Some(key) = self.panel_key() else { return };
        let saved = (!self.panel.is_empty()).then(|| self.panel.to_json());
        if self.prefs["rightPanel"].get(&key) == saved.as_ref() {
            return;
        }
        if !self.prefs["rightPanel"].is_object() {
            self.prefs["rightPanel"] = serde_json::json!({});
        }
        let map = self.prefs["rightPanel"].as_object_mut().unwrap();
        match saved {
            Some(v) => map.insert(key, v),
            None => map.remove(&key),
        };
        crate::settings::save_prefs(&self.prefs);
    }

    /// Reopens the tabs the newly selected thread had; their content loads as each shows.
    pub(crate) fn restore_panel(&mut self, _cx: &mut Context<Self>) {
        let saved = self
            .panel_key()
            .map(|k| self.prefs["rightPanel"][&k].clone())
            .unwrap_or_default();
        self.panel = Panel::from_json(&saved);
        self.panel_synced = None;
        self.files.reset();
    }

    /// Applies a tab change, saves it, and repaints; `sync_panel` loads what now shows.
    fn update_panel(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Panel)) {
        f(&mut self.panel);
        self.remember_panel();
        cx.notify();
    }

    pub(crate) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.update_panel(cx, Panel::toggle);
    }

    pub(crate) fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        self.update_panel(cx, |p| p.toggle_surface(Surface::Diff));
    }

    pub(crate) fn open_surface(&mut self, surface: Surface, cx: &mut Context<Self>) {
        self.update_panel(cx, |p| p.open_surface(surface));
    }

    /// Closes a tab and lets go of what it held: a terminal detaches, a file's editor drops.
    fn close_surfaces(&mut self, closing: Vec<Surface>, cx: &mut Context<Self>) {
        for surface in &closing {
            match surface {
                Surface::Terminal => self.close_terminal(cx),
                Surface::File(path) => self.files.forget(path),
                _ => {}
            }
        }
        self.update_panel(cx, |p| {
            for surface in &closing {
                p.close(surface);
            }
        });
    }

    pub(crate) fn close_file_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.close_surfaces(vec![Surface::File(path.to_owned())], cx);
    }

    /// The panel-bottom toggle: a terminal in the panel moves back to the drawer.
    pub(crate) fn toggle_terminal_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_in_panel() {
            self.update_panel(cx, |p| p.close(&Surface::Terminal));
        } else {
            self.toggle_terminal(window, cx);
        }
    }

    /// Loads the surface that just came on screen, once per change (render calls this).
    pub(crate) fn sync_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.panel_visible();
        let shown = self.panel.shown().cloned().filter(|_| visible);
        if shown == self.panel_synced {
            return;
        }
        self.panel_synced = shown.clone();
        match shown {
            None if visible => {
                self.picker_highlight = None;
                window.focus(&self.picker_focus, cx);
            }
            None => {}
            // A pull request's diff stays chosen; otherwise the thread or turn diff refreshes.
            Some(Surface::Diff) if self.diff_pr.is_some() || self.diff_tool.is_some() => {}
            Some(Surface::Diff) => self.load_diff(self.diff_target, cx),
            Some(Surface::Files) => self.ensure_file_tree(cx),
            Some(Surface::File(path)) => {
                self.ensure_file_tree(cx);
                if !self.files.has(&path) {
                    self.open_file(path, None, window, cx);
                }
            }
            Some(Surface::Terminal) => {
                if self.term.is_none() {
                    self.open_terminal(false, cx);
                }
                window.focus(&self.term_focus, cx);
            }
        }
    }

    fn surface_title(&self, surface: &Surface) -> String {
        match surface {
            Surface::Files => t("Files"),
            Surface::Diff => t("Diff"),
            Surface::File(path) => path.rsplit('/').next().unwrap_or(path).to_owned(),
            Surface::Terminal => self
                .selected
                .iter()
                .flat_map(|thread| self.terminal_tabs(thread))
                .find(|(id, ..)| *id == self.term_id)
                .map_or_else(|| t("Terminal"), |(_, label, _)| label),
        }
    }

    fn surface_icon(surface: &Surface) -> &'static str {
        match surface {
            Surface::Files => "files",
            Surface::Diff => "file-diff",
            Surface::File(path) => crate::files::file_icon(path),
            Surface::Terminal => "square-terminal",
        }
    }

    /// One tab (web: `RightPanelTab`): its icon turns into a close button while hovered; middle
    /// click closes; it drags to reorder and right click offers the close actions.
    fn render_panel_tab(&self, i: usize, surface: &Surface, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let active = self.panel.active.as_ref() == Some(surface);
        let title: SharedString = self.surface_title(surface).into();
        let icon = Self::surface_icon(surface);
        let group: SharedString = format!("panel-tab-{i}").into();
        let dragged = DraggedTab {
            surface: surface.clone(),
            title: title.clone(),
            icon,
        };
        let (activate, close, aux, menu_surface) = (
            surface.clone(),
            surface.clone(),
            surface.clone(),
            surface.clone(),
        );
        let app = cx.entity().downgrade();
        h_flex()
            .id(("panel-tab", i))
            .group(group.clone())
            .h(px(24.))
            .max_w(px(144.))
            .flex_none()
            .pl(px(6.))
            .pr_2()
            .gap(px(2.))
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .map(|tab| {
                if active {
                    tab.bg(c.accent).text_color(c.foreground)
                } else {
                    tab.text_color(c.muted_foreground)
                        .hover(|s| s.bg(c.accent.opacity(0.6)).text_color(c.foreground))
                }
            })
            .child(
                div()
                    .id(("panel-tab-close", i))
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .hover(|s| s.bg(c.muted))
                    .child(
                        div()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(ui::icon(icon).size(px(12.))),
                    )
                    .child(
                        div()
                            .absolute()
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .child(ui::icon("x").size(px(12.))),
                    )
                    .tooltip({
                        let label = crate::i18n::tv("Close {title}", &[("title", &title)]);
                        move |window, cx| Tooltip::new(label.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_surfaces(vec![close.clone()], cx);
                    })),
            )
            .child(div().min_w_0().truncate().child(title.clone()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_surface(activate.clone(), cx);
            }))
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close_surfaces(vec![aux.clone()], cx)),
            )
            .on_drag(dragged, |tab, _, _, cx| cx.new(|_| tab.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, cx| s.bg(cx.theme().colors.accent))
            .on_drop(cx.listener(move |this, tab: &DraggedTab, _, cx| {
                let surface = tab.surface.clone();
                this.update_panel(cx, |p| p.move_surface(&surface, i));
            }))
            .context_menu(move |menu, _, cx| {
                let surfaces = app
                    .upgrade()
                    .map(|a| a.read(cx).panel.surfaces.clone())
                    .unwrap_or_default();
                let at = surfaces
                    .iter()
                    .position(|s| *s == menu_surface)
                    .unwrap_or(0);
                let item = |label: &str, closing: Vec<Surface>| {
                    let app = app.clone();
                    PopupMenuItem::new(t(label))
                        .disabled(closing.is_empty())
                        .on_click(move |_, _, cx| {
                            app.update(cx, |this, cx| this.close_surfaces(closing.clone(), cx))
                                .ok();
                        })
                };
                let others: Vec<Surface> = surfaces
                    .iter()
                    .filter(|s| **s != menu_surface)
                    .cloned()
                    .collect();
                menu.item(item("Close", vec![menu_surface.clone()]))
                    .item(item("Close others", others))
                    .item(item("Close to the right", surfaces[at + 1..].to_vec()))
                    .separator()
                    .item(item("Close all", surfaces.clone()))
            })
            .into_any_element()
    }

    /// The panel frame (web: `RightPanelTabs`): tabs and `+`, then maximize and the terminal
    /// and panel toggles that leave the thread header while the panel is open.
    pub(crate) fn render_right_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let maximized = self.panel.maximized;
        let body = match self.panel.shown() {
            None => self.render_surface_picker(cx).into_any_element(),
            Some(Surface::Diff) => self.render_diff_panel(cx).into_any_element(),
            Some(Surface::Files | Surface::File(_)) => self.render_files(cx).into_any_element(),
            Some(Surface::Terminal) => self.render_terminal(true, cx).into_any_element(),
        };
        v_flex()
            .map(|p| {
                if maximized {
                    p.flex_1().min_w_0()
                } else {
                    p.w(px(RIGHT_PANEL_WIDTH)).flex_none().border_l_1()
                }
            })
            .h_full()
            .border_color(c.border)
            .child(
                h_flex()
                    .h(px(TOPBAR_HEIGHT))
                    .flex_none()
                    .px_3()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("panel-tabs")
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .overflow_x_scroll()
                            .children(
                                self.panel
                                    .surfaces
                                    .iter()
                                    .enumerate()
                                    .map(|(i, s)| self.render_panel_tab(i, s, cx)),
                            )
                            .child(self.render_add_surface(cx)),
                    )
                    .child(
                        toggle(
                            "panel-maximize",
                            if maximized {
                                "minimize-2"
                            } else {
                                "maximize-2"
                            },
                            maximized,
                            cx,
                        )
                        .tooltip(move |window, cx| {
                            let label = if maximized {
                                "Restore panel size"
                            } else {
                                "Maximize panel"
                            };
                            Tooltip::new(t(label)).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_panel_maximized(cx))),
                    )
                    .child(
                        toggle(
                            "panel-terminal-toggle",
                            "panel-bottom",
                            self.term.is_some() && !self.terminal_in_panel(),
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.toggle_terminal_place(window, cx)
                            }),
                        ),
                    )
                    .child(
                        toggle("panel-right-toggle", "panel-right", true, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_right_panel(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
    }

    /// Handles a key for the picker or the open `+` menu: T/F/D pick, arrows move, Enter opens
    /// the highlighted one, Escape closes the menu. Returns whether it used the key.
    pub(crate) fn surface_pick_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = &e.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return false;
        }
        let n = PICKS.len();
        let pick = match e.keystroke.key.as_str() {
            "down" | "right" => {
                self.picker_highlight = Some(self.picker_highlight.map_or(0, |h| (h + 1) % n));
                cx.notify();
                return true;
            }
            "up" | "left" => {
                self.picker_highlight =
                    Some(self.picker_highlight.map_or(n - 1, |h| (h + n - 1) % n));
                cx.notify();
                return true;
            }
            "escape" if self.add_menu_open => {
                self.add_menu_open = false;
                cx.notify();
                return true;
            }
            "enter" => self.picker_highlight.map(|h| PICKS[h].0.clone()),
            key => pick_for_key(key),
        };
        let Some(surface) = pick else { return false };
        self.add_menu_open = false;
        self.open_surface(surface, cx);
        true
    }

    /// `rightPanel.new` and the `+` button (web: the add surface menu).
    pub(crate) fn open_add_surface_menu(&mut self, cx: &mut Context<Self>) {
        if !self.panel.open {
            self.update_panel(cx, |p| p.open = true);
        }
        self.add_menu_open = true;
        self.picker_highlight = None;
        cx.notify();
    }

    /// `rightPanel.close` (web: closes the active tab), or the panel when it shows the picker.
    pub(crate) fn close_active_surface(&mut self, cx: &mut Context<Self>) {
        match self.panel.shown().cloned() {
            Some(surface) => self.close_surfaces(vec![surface], cx),
            None => self.update_panel(cx, |p| p.open = false),
        }
    }

    pub(crate) fn toggle_panel_maximized(&mut self, cx: &mut Context<Self>) {
        self.update_panel(cx, |p| p.maximized = !p.maximized);
    }

    /// A surface row (web: the picker's buttons and the `+` menu's items).
    fn pick_row(&self, i: usize, id: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = cx.theme().colors;
        let (surface, icon, label, key) = PICKS[i].clone();
        h_flex()
            .id((id, i))
            .h(px(32.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(self.picker_highlight == Some(i), |row| {
                row.bg(c.accent.opacity(0.6))
            })
            .hover(|s| s.bg(c.accent.opacity(0.6)))
            .child(ui::icon(icon).size(px(16.)))
            .child(div().flex_1().min_w_0().truncate().child(t(label)))
            .child(kbd(key, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.add_menu_open = false;
                this.open_surface(surface.clone(), cx)
            }))
    }

    /// The `+` beside the tabs: a menu of surfaces with their keys.
    fn render_add_surface(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let app = cx.entity().downgrade();
        let rows_app = app.clone();
        Popover::new("panel-add-surface-menu")
            .anchor(Anchor::TopLeft)
            .open(self.add_menu_open)
            .on_open_change(move |open, _, cx| {
                let open = *open;
                app.update(cx, |this, cx| {
                    if open {
                        this.open_add_surface_menu(cx);
                    } else {
                        this.add_menu_open = false;
                        cx.notify();
                    }
                })
                .ok();
            })
            .trigger(
                Button::new("panel-add-surface")
                    .ghost()
                    .size(px(24.))
                    .flex_none()
                    .child(
                        ui::icon("plus")
                            .size(px(14.))
                            .text_color(c.muted_foreground),
                    )
                    .tooltip(t("Add panel surface")),
            )
            .content(move |_, _, cx| {
                let rows = rows_app.upgrade().map(|app| {
                    app.update(cx, |this, cx| {
                        (0..PICKS.len())
                            .map(|i| this.pick_row(i, "add-surface", cx))
                            .collect::<Vec<_>>()
                    })
                });
                v_flex()
                    .w(px(220.))
                    .p_1()
                    .children(rows.into_iter().flatten())
            })
    }

    /// The empty panel (web: `RightPanelEmptyState`): T/F/D pick a surface, arrows and Enter
    /// move through the list.
    fn render_surface_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("surface-picker")
            .track_focus(&self.picker_focus)
            .flex_1()
            .items_center()
            .justify_center()
            .px_6()
            .pb(px(TOPBAR_HEIGHT))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if this.surface_pick_key(e, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(320.))
                    .py_6()
                    .child(
                        div()
                            .mb_3()
                            .text_center()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t("Open a surface")),
                    )
                    .child(
                        v_flex().gap(px(2.)).children(
                            (0..PICKS.len()).map(|i| self.pick_row(i, "surface-pick", cx)),
                        ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::pick_for_key;
    use crate::panel::Surface;

    #[test]
    fn picks_surfaces_by_key() {
        assert_eq!(pick_for_key("t"), Some(Surface::Terminal));
        assert_eq!(pick_for_key("F"), Some(Surface::Files));
        assert_eq!(pick_for_key("d"), Some(Surface::Diff));
        assert_eq!(pick_for_key("x"), None);
    }
}
