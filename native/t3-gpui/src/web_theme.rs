//! The web app's design tokens (`apps/web/src/index.css`, `:root` and its dark variant) applied
//! over gpui-kit's theme, so both clients share one look. Colors are the CSS values resolved to
//! sRGB (oklch and `color-mix` computed once); translucent ones keep their alpha.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

fn black(alpha: f32) -> Hsla {
    hsla(0., 0., 0., alpha)
}

/// The web's tokens for one mode.
pub struct Tokens {
    pub background: Hsla,
    pub foreground: Hsla,
    pub card: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub primary: Hsla,
    pub sidebar: Hsla,
    pub sidebar_row_hover: Hsla,
    pub sidebar_row_selected: Hsla,
    pub error: Hsla,
    pub error_foreground: Hsla,
    pub success_foreground: Hsla,
    pub warning_foreground: Hsla,
    pub info_foreground: Hsla,
}

pub fn tokens(dark: bool) -> Tokens {
    if dark {
        let card: Hsla = rgb(0x111111).into();
        Tokens {
            background: rgb(0x000000).into(),
            foreground: rgb(0xf5f5f5).into(),
            card,
            muted: white(0.03),
            muted_foreground: rgb(0xa3a3a3).into(),
            accent: white(0.04),
            border: white(0.08),
            input: white(0.11),
            primary: rgb(0x346bf1).into(),
            sidebar: card,
            sidebar_row_hover: white(0.04),
            sidebar_row_selected: white(0.03),
            error: rgb(0xf05252).into(),
            error_foreground: rgb(0xf87171).into(),
            success_foreground: rgb(0x34d399).into(),
            warning_foreground: rgb(0xfbbf24).into(),
            info_foreground: rgb(0x60a5fa).into(),
        }
    } else {
        Tokens {
            background: rgb(0xfcfcfc).into(),
            foreground: rgb(0x18181b).into(),
            card: rgb(0xffffff).into(),
            muted: rgb(0xfafafa).into(),
            muted_foreground: rgb(0x71717a).into(),
            accent: rgb(0xf4f4f5).into(),
            border: rgb(0xe4e4e7).into(),
            input: rgb(0xd4d4d8).into(),
            primary: rgb(0x1b4ed8).into(),
            sidebar: rgb(0xfafafa).into(),
            sidebar_row_hover: rgb(0xfcfcfc).into(),
            sidebar_row_selected: rgb(0xffffff).into(),
            error: rgb(0xef4444).into(),
            error_foreground: rgb(0xb91c1c).into(),
            success_foreground: rgb(0x047857).into(),
            warning_foreground: rgb(0xb45309).into(),
            info_foreground: rgb(0x1d4ed8).into(),
        }
    }
}

/// Switches light/dark, then lays the web's tokens over gpui-kit's defaults.
pub fn change(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    Theme::change(mode, window, cx);
    let t = tokens(mode != ThemeMode::Light);
    let theme = Theme::global_mut(cx);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(14.);
    theme.mono_font_family = "Menlo".into();
    theme.radius = px(8.);
    theme.radius_lg = px(12.);
    let c = &mut theme.colors;
    c.background = t.background;
    c.foreground = t.foreground;
    c.border = t.border;
    c.input = t.input;
    c.ring = t.primary;
    c.caret = t.foreground;
    c.muted = t.muted;
    c.muted_foreground = t.muted_foreground;
    c.accent = t.accent;
    c.accent_foreground = t.foreground;
    c.popover = t.card;
    c.popover_foreground = t.foreground;
    c.primary = t.primary;
    c.primary_hover = t.primary.opacity(0.9);
    c.primary_active = t.primary.opacity(0.8);
    c.primary_foreground = rgb(0xffffff).into();
    c.secondary = t.muted;
    c.secondary_hover = t.accent;
    c.secondary_active = t.accent;
    c.secondary_foreground = t.foreground;
    c.list = t.background;
    c.list_hover = t.sidebar_row_hover;
    c.list_active = t.sidebar_row_selected;
    c.list_active_border = black(0.);
    c.sidebar = t.sidebar;
    c.sidebar_foreground = t.foreground;
    c.sidebar_border = t.border;
    c.sidebar_accent = t.sidebar_row_selected;
    c.sidebar_accent_foreground = t.foreground;
    c.title_bar = t.sidebar;
    c.title_bar_border = t.border;
    c.danger = t.error;
    c.danger_foreground = t.error_foreground;
    c.success_foreground = t.success_foreground;
    c.warning_foreground = t.warning_foreground;
    c.info_foreground = t.info_foreground;
    c.window_border = t.border;
    c.overlay = black(0.5);
}
