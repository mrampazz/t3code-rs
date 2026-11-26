//! Small visual primitives shared across surfaces, styled like the web's (`components/ui`).

use gpui_kit::component::{ActiveTheme, Icon, ThemeMode};
use gpui_kit::*;

/// A Lucide icon by its web name (`lucide-react`'s kebab-case file name).
pub fn icon(name: &str) -> Icon {
    Icon::empty().path(format!("icons/{name}.svg"))
}

/// A square ghost icon button (web: `Button size="icon-xs" variant="ghost"`), 28px with a 16px icon.
pub fn icon_button(id: impl Into<ElementId>, name: &str, cx: &App) -> Stateful<Div> {
    let c = cx.theme().colors;
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .text_color(c.muted_foreground)
        .hover(|s| s.bg(c.accent).text_color(c.foreground))
        .child(icon(name).size(px(16.)))
}

/// A project's monogram tile (web: `ProjectMonogram`): two letters on a 14% tint of its color.
pub fn project_mark(name: &str, size: Pixels, cx: &App) -> Div {
    let (dark, light) = crate::sidebar::PROJECT_COLORS[crate::sidebar::project_color_index(name)];
    let color: Hsla = rgb(if cx.theme().mode == ThemeMode::Light {
        light
    } else {
        dark
    })
    .into();
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(size * 0.25)
        .bg(color.opacity(0.14))
        .text_color(color)
        .font_family(cx.theme().mono_font_family.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(size * 0.5)
        .line_height(size)
        .child(crate::sidebar::monogram(name))
}

/// The lucide icon for a machine kind (web: `EnvironmentMachineIcon`).
pub fn machine_icon(machine: Option<&str>) -> &'static str {
    match machine {
        Some("desktop") => "monitor",
        Some("server") => "server",
        Some("cloud") => "cloud",
        _ => "laptop",
    }
}
