//! Native application menus (desktop: `apps/desktop` menu): each item runs the same keybinding
//! command as its shortcut, so menus and keys never drift.

use gpui_kit::*;

use crate::T3App;

/// A menu item that runs a keybinding command.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = t3, no_json)]
pub struct RunCommand {
    pub command: SharedString,
}

actions!(t3, [Quit]);

fn item(name: &str, command: &str) -> MenuItem {
    MenuItem::action(
        crate::i18n::t(name),
        RunCommand {
            command: command.to_owned().into(),
        },
    )
}

pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus([
        Menu {
            name: "T3 Code".into(),
            items: vec![
                item("Settings", "settings.open"),
                MenuItem::separator(),
                MenuItem::action(crate::i18n::t("Quit"), Quit),
            ],
            disabled: false,
        },
        Menu {
            name: crate::i18n::t("File").into(),
            items: vec![
                item("New thread", "chat.new"),
                item("New local thread", "chat.newLocal"),
                item("Add project", "project.add"),
            ],
            disabled: false,
        },
        Menu {
            name: crate::i18n::t("View").into(),
            items: vec![
                item("Command palette", "commandPalette.toggle"),
                item("Toggle sidebar", "sidebar.toggle"),
                item("Toggle terminal", "terminal.toggle"),
                item("Toggle diff", "diff.toggle"),
                MenuItem::separator(),
                item("Find in current thread", "chat.find"),
                item("Change appearance", "appearance.cycle"),
            ],
            disabled: false,
        },
        Menu {
            name: crate::i18n::t("Go").into(),
            items: vec![
                item("Back", "navigation.back"),
                item("Forward", "navigation.forward"),
                MenuItem::separator(),
                item("Next thread", "thread.next"),
                item("Previous thread", "thread.previous"),
            ],
            disabled: false,
        },
    ]);
}

/// Window bounds as stored in prefs (`{"x","y","w","h","maximized"}`).
pub fn bounds_to_json(bounds: WindowBounds) -> serde_json::Value {
    let (b, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    serde_json::json!({ "x": f32::from(b.origin.x), "y": f32::from(b.origin.y),
        "w": f32::from(b.size.width), "h": f32::from(b.size.height), "maximized": maximized })
}

pub fn bounds_from_json(v: &serde_json::Value) -> Option<WindowBounds> {
    let n = |k: &str| v[k].as_f64().map(|f| px(f as f32));
    let (w, h) = (n("w")?, n("h")?);
    if w < px(400.) || h < px(300.) {
        return None;
    }
    let b = Bounds {
        origin: point(n("x")?, n("y")?),
        size: size(w, h),
    };
    Some(if v["maximized"] == true {
        WindowBounds::Maximized(b)
    } else {
        WindowBounds::Windowed(b)
    })
}

impl T3App {
    /// Remembers the window's bounds in memory; they reach disk on quit.
    pub(crate) fn remember_bounds(&mut self, window: &mut Window) {
        self.prefs["window"] = bounds_to_json(window.window_bounds());
    }
}

#[cfg(test)]
mod tests {
    use super::{bounds_from_json, bounds_to_json};
    use gpui_kit::{Bounds, WindowBounds, point, px, size};

    #[test]
    fn window_bounds_round_trip() {
        let b = WindowBounds::Windowed(Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(900.), px(700.)),
        });
        assert_eq!(bounds_from_json(&bounds_to_json(b)), Some(b));
        assert_eq!(
            bounds_from_json(&serde_json::json!({"x": 0, "y": 0, "w": 10, "h": 10})),
            None,
            "too small to be real"
        );
    }
}
