//! Small visual primitives shared across surfaces, styled like the web's (`components/ui`).

use gpui_kit::component::text::TextViewStyle;
use gpui_kit::component::{ActiveTheme, Icon, ThemeMode};
use gpui_kit::*;

/// A Lucide icon by its web name (`lucide-react`'s kebab-case file name).
pub fn icon(name: &str) -> Icon {
    Icon::empty().path(format!("icons/{name}.svg"))
}

/// A multicolor mark (a brand logo) from SVG source at 16px; `currentColor` takes the text color,
/// as on the web. The image id hashes the bytes, so each mark rasterizes once.
pub fn svg_image(svg: &str, cx: &App) -> Img {
    let ink = Rgba::from(cx.theme().colors.foreground.opacity(0.8));
    let ink = format!(
        "#{:02x}{:02x}{:02x}",
        (ink.r * 255.) as u8,
        (ink.g * 255.) as u8,
        (ink.b * 255.) as u8
    );
    let bytes = svg.replace("currentColor", &ink).into_bytes();
    img(ImageSource::Image(std::sync::Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        bytes,
    ))))
    .size(px(16.))
    .flex_none()
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

/// Chat markdown as the web's prose (`ChatMarkdown.tsx`): borderless code blocks on a raised
/// surface, tables with only row rules and a bold header, and filled inline code.
pub fn markdown_style(cx: &App) -> TextViewStyle {
    let c = cx.theme().colors;
    let clear = hsla(0., 0., 0., 0.);
    TextViewStyle::default()
        .code_block(StyleRefinement::default().border_0().rounded(px(12.)))
        .table(
            StyleRefinement::default()
                .border_0()
                .rounded_none()
                .bg(clear),
        )
        .table_head(StyleRefinement::default().font_weight(FontWeight::SEMIBOLD))
        .table_cell(
            StyleRefinement::default()
                .pl_0()
                .pr_3()
                .py(px(10.))
                .border_color(clear),
        )
        .inline_code(HighlightStyle {
            background_color: Some(c.foreground.opacity(0.08)),
            ..Default::default()
        })
}
