//! Small visual primitives shared across surfaces, styled like the web's (`components/ui`).

use gpui_kit::component::text::TextViewStyle;
use gpui_kit::component::{ActiveTheme, Icon, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
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

/// A button's look (web: `Button variant`).
#[derive(Clone, Copy, PartialEq)]
pub enum Variant {
    /// `ghost`: the icon stays muted on hover.
    Ghost,
    /// `ghost-muted`: the icon brightens on hover.
    GhostMuted,
    /// `secondary`: a filled, pressed-looking toggle.
    Secondary,
}

/// An extra-small button around a 12px icon (web: `Button size="xs"` or `"icon-xs"`, 24px tall).
pub fn xs_button(
    id: impl Into<ElementId>,
    icon_name: &str,
    square: bool,
    variant: Variant,
    cx: &App,
) -> Stateful<Div> {
    let c = cx.theme().colors;
    div()
        .id(id)
        .h(px(24.))
        .when(square, |b| b.w(px(24.)))
        .when(!square, |b| b.px(px(8.)))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .map(|b| match variant {
            Variant::Secondary => b
                .bg(c.secondary)
                .text_color(c.secondary_foreground)
                .hover(|s| s.bg(c.secondary.opacity(0.9))),
            Variant::Ghost => b.text_color(c.muted_foreground).hover(|s| s.bg(c.accent)),
            Variant::GhostMuted => b
                .text_color(c.muted_foreground)
                .hover(|s| s.bg(c.accent).text_color(c.foreground)),
        })
        .child(icon(icon_name).size(px(12.)))
}

/// A tooltip with `label`, as the web's `TooltipPopup`.
pub fn tooltip(label: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
}

/// Copies `text` and shows a check for a second (web: `MessageCopyButton`, the code block's copy).
#[derive(IntoElement)]
pub struct CopyButton {
    id: ElementId,
    text: String,
    label: SharedString,
    icon: &'static str,
    square: bool,
    /// The check takes the primary color, as `MessageCopyButton` draws it.
    primary_check: bool,
    timeout: std::time::Duration,
}

impl CopyButton {
    /// `MessageCopyButton`: an xs ghost-muted button labelled "Copy message".
    pub fn message(id: impl Into<ElementId>, text: String) -> Self {
        Self {
            id: id.into(),
            text,
            label: crate::i18n::t("Copy message").into(),
            icon: "copy",
            square: false,
            primary_check: true,
            timeout: std::time::Duration::from_millis(1000),
        }
    }

    pub fn label(mut self, label: String) -> Self {
        self.label = label.into();
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = icon;
        self
    }

    /// The code block's `icon-xs` copy: a plain check, held a little longer.
    pub fn code(id: impl Into<ElementId>, text: String) -> Self {
        Self {
            id: id.into(),
            text,
            label: crate::i18n::t("Copy code").into(),
            icon: "copy",
            square: true,
            primary_check: false,
            timeout: std::time::Duration::from_millis(1200),
        }
    }
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let is_copied = *copied.read(cx);
        let label = if is_copied && !self.primary_check {
            crate::i18n::t("Copied").into()
        } else {
            self.label
        };
        let primary = cx.theme().colors.primary;
        let (text, timeout) = (self.text, self.timeout);
        xs_button(
            self.id,
            if is_copied { "check" } else { self.icon },
            self.square,
            Variant::GhostMuted,
            cx,
        )
        .when(is_copied && self.primary_check, |b| b.text_color(primary))
        .tooltip(tooltip(label))
        .on_click(move |_, _, cx| {
            if *copied.read(cx) {
                return;
            }
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            copied.update(cx, |c, cx| {
                *c = true;
                cx.notify();
            });
            let copied = copied.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(timeout).await;
                copied.update(cx, |c, cx| {
                    *c = false;
                    cx.notify();
                });
            })
            .detach();
        })
    }
}

/// A context chip's accent (`--context-chip-<kind>`), as the web's palette solves it for contrast
/// on each theme's surfaces.
pub fn chip_ink(kind: &str, cx: &App) -> Hsla {
    let dark = cx.theme().mode == ThemeMode::Dark;
    let (d, l) = match kind {
        "thread" => (0x009e97, 0x006f6b),
        "file" => (0x0f96d3, 0x006a99),
        "citation" => (0x4e8cee, 0x2361be),
        _ => (0x0d9ab4, 0x006d80),
    };
    rgb(if dark { d } else { l }).into()
}

/// An inline context chip (web: `ContextChip` + `ContextChipLabel`): icon and medium label in the
/// kind's ink, the label underlined on hover.
pub fn context_chip(
    id: impl Into<ElementId>,
    ink: Hsla,
    icon: impl IntoElement,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let id = id.into();
    let group = SharedString::from(format!("chip-{id:?}"));
    div()
        .id(id)
        .group(group.clone())
        .flex()
        .items_center()
        .max_w_full()
        .gap(px(3.))
        .rounded_sm()
        .cursor_pointer()
        .text_color(ink)
        .font_weight(FontWeight::MEDIUM)
        .child(icon)
        .child(
            div()
                .min_w_0()
                .truncate()
                .group_hover(group, |s| s.underline())
                .child(label.into()),
        )
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

/// A provider driver's logo as the web draws it (`ProviderInstanceIcon`), from
/// `assets/providers` (generated by `scripts/provider-logos.ts`).
pub fn provider_logo(driver: &str, dark: bool) -> Option<Img> {
    let svg = match (driver, dark) {
        ("acpRegistry", false) => include_str!("../assets/providers/acpRegistry-light.svg"),
        ("acpRegistry", true) => include_str!("../assets/providers/acpRegistry-dark.svg"),
        ("anthropic", false) => include_str!("../assets/providers/anthropic-light.svg"),
        ("anthropic", true) => include_str!("../assets/providers/anthropic-dark.svg"),
        ("codex", false) => include_str!("../assets/providers/codex-light.svg"),
        ("codex", true) => include_str!("../assets/providers/codex-dark.svg"),
        ("cursor", false) => include_str!("../assets/providers/cursor-light.svg"),
        ("cursor", true) => include_str!("../assets/providers/cursor-dark.svg"),
        ("fx", false) => include_str!("../assets/providers/fx-light.svg"),
        ("fx", true) => include_str!("../assets/providers/fx-dark.svg"),
        ("grok", false) => include_str!("../assets/providers/grok-light.svg"),
        ("grok", true) => include_str!("../assets/providers/grok-dark.svg"),
        ("ollama", false) => include_str!("../assets/providers/ollama-light.svg"),
        ("ollama", true) => include_str!("../assets/providers/ollama-dark.svg"),
        ("openai", false) => include_str!("../assets/providers/openai-light.svg"),
        ("openai", true) => include_str!("../assets/providers/openai-dark.svg"),
        ("opencode", false) => include_str!("../assets/providers/opencode-light.svg"),
        ("opencode", true) => include_str!("../assets/providers/opencode-dark.svg"),
        ("openrouter", false) => include_str!("../assets/providers/openrouter-light.svg"),
        ("openrouter", true) => include_str!("../assets/providers/openrouter-dark.svg"),
        ("pi", false) => include_str!("../assets/providers/pi-light.svg"),
        ("pi", true) => include_str!("../assets/providers/pi-dark.svg"),
        ("antigravity", _) => include_str!("../assets/providers/antigravity.svg"),
        ("claudeAgent", _) => include_str!("../assets/providers/claudeAgent.svg"),
        ("kiro", _) => include_str!("../assets/providers/kiro.svg"),
        ("muse", _) => include_str!("../assets/providers/muse.svg"),
        _ => return None,
    };
    Some(
        img(ImageSource::Image(std::sync::Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.as_bytes().to_vec(),
        ))))
        .size(px(16.))
        .flex_none(),
    )
}
