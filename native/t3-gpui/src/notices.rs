//! The notices stacked on the composer (web: `ComposerBannerStack`, `ComposerBanner`): each an
//! attached card inset from the composer's edges, joined edge to edge with the next and tucked
//! under the composer below. Machine notices come first, then what needs you.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme, ThemeMode, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::T3App;

/// `ComposerBannerVariant`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Tone {
    Default,
    Warning,
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// `ComposerBannerStackItem`.
pub(crate) struct Notice {
    pub id: String,
    pub tone: Tone,
    /// `group: "environment"`: about the machine, so it leads the stack.
    pub environment: bool,
    pub icon: AnyElement,
    pub title: String,
    pub description: Option<String>,
    /// Clicking the description (the reply's cited text) does this.
    pub on_description: Option<Handler>,
    pub actions: Vec<AnyElement>,
    pub dismiss: Option<(String, Handler)>,
}

/// `--chat-composer-drawer-inset`, and how far a card tucks under what follows it
/// (`--chat-composer-attachment-overlap`, 1rem + 1px).
const INSET: f32 = 22.;
const OVERLAP: f32 = 17.;
const ICON_COLUMN: f32 = 24.;

/// Text widths from the last layout, natural and available, so cut-off text gets a details button.
pub(crate) type Widths = Rc<RefCell<HashMap<String, (Pixels, Pixels)>>>;

/// Records `key`'s measured width in slot 0 (natural) or 1 (available); a change re-renders.
fn measure(widths: &Widths, key: String, slot: usize, view: EntityId) -> impl IntoElement {
    let widths = widths.clone();
    canvas(
        move |bounds, _, cx| {
            let mut map = widths.borrow_mut();
            let entry = map.entry(key.clone()).or_insert((px(0.), px(0.)));
            let value = if slot == 0 {
                &mut entry.0
            } else {
                &mut entry.1
            };
            if (*value - bounds.size.width).abs() > px(0.5) {
                *value = bounds.size.width;
                cx.notify(view);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

impl T3App {
    /// `ComposerBannerStack`, attached: the notices in group order above the composer card.
    pub(crate) fn render_notice_stack(
        &self,
        mut notices: Vec<Notice>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if notices.is_empty() {
            return None;
        }
        notices.sort_by_key(|n| !n.environment);
        let count = notices.len();
        let cards: Vec<AnyElement> = notices
            .into_iter()
            .enumerate()
            .map(|(i, notice)| self.render_notice(notice, i == 0, i + 1 == count, cx))
            .collect();
        // The stack's own gap is undone so the last card reaches under the composer.
        Some(
            div()
                .flex()
                .flex_col()
                .mb(px(-8.))
                .children(cards)
                .into_any_element(),
        )
    }

    /// `NoticeCard`: icon, title and description on one line, then actions and dismiss.
    fn render_notice(
        &self,
        notice: Notice,
        first: bool,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let c = theme.colors;
        let dark = theme.mode == ThemeMode::Dark;
        let web = crate::web_theme::current(cx);
        // `--chat-composer-glass-surface` at `--glass-opacity`, tinted by the variant.
        let surface = if dark { rgb(0x111111).into() } else { web.card }.opacity(0.8);
        let (outline, tint) = match notice.tone {
            Tone::Default => (web.composer_outline, None),
            Tone::Warning => (c.warning.opacity(0.28), Some(c.warning.opacity(0.08))),
        };
        let corner = if first { px(16.) } else { px(8.) };
        let view = cx.entity_id();
        let id = notice.id.clone();
        // `NoticeText`: text cut off by the card's width gets a button showing all of it.
        let text = |key: &str, value: &str, cx: &mut Context<Self>| -> (Div, Option<AnyElement>) {
            let key = format!("{id}:{key}");
            let cut = self
                .notice_widths
                .borrow()
                .get(&key)
                .is_some_and(|(natural, available)| *natural > *available + px(0.5));
            let element = div()
                .relative()
                .min_w_0()
                .truncate()
                .child(value.to_owned())
                .child(measure(&self.notice_widths, key.clone(), 1, view))
                // An unconstrained copy, laid out but not painted, gives the full width.
                .child(
                    div()
                        .absolute()
                        .invisible()
                        .whitespace_nowrap()
                        .child(value.to_owned())
                        .child(measure(&self.notice_widths, key.clone(), 0, view)),
                );
            let details = cut.then(|| {
                crate::ui::xs_button(
                    SharedString::from(format!("{key}:details")),
                    "info",
                    true,
                    crate::ui::Variant::GhostMuted,
                    cx,
                )
                .tooltip(crate::ui::tooltip(value.to_owned()))
                .into_any_element()
            });
            (element, details)
        };
        let (title, title_details) = text("title", &notice.title, cx);
        let description = notice
            .description
            .as_deref()
            .map(|d| text("description", d, cx));

        h_flex()
            .w_full()
            .relative()
            .flex_none()
            .child(
                div()
                    .mx(px(INSET))
                    .w_full()
                    .mb(px(-OVERLAP))
                    .relative()
                    .px_1()
                    .pt(px(5.))
                    .pb(px(5. + OVERLAP))
                    .text_xs()
                    .line_height(px(16.))
                    .bg(surface)
                    .when_some(tint, |d, tint| d.bg(blend(surface, tint)))
                    .border_color(outline)
                    .border_l_1()
                    .border_r_1()
                    .when(first, |d| d.border_t_1().rounded_t(px(16.)))
                    // The composer's top edge casts a shade into the last card (`composer-seam-above`).
                    .when(dark && last, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom(px(OVERLAP))
                                .h(px(10.))
                                .bg(linear_gradient(
                                    0.,
                                    linear_color_stop(hsla(0., 0., 0., 0.18), 0.),
                                    linear_color_stop(hsla(0., 0., 0., 0.), 1.),
                                )),
                        )
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .min_h(px(ICON_COLUMN))
                            .gap_1()
                            .child(
                                div()
                                    .w(px(ICON_COLUMN))
                                    .h(px(ICON_COLUMN))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(c.muted_foreground)
                                    .child(notice.icon),
                            )
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .whitespace_nowrap()
                                    .child(
                                        title.font_weight(FontWeight::MEDIUM).line_height(px(24.)),
                                    )
                                    .children(title_details)
                                    .children(description.map(|(d, details)| {
                                        let d =
                                            d.w_0().flex_grow(1.).text_color(c.muted_foreground);
                                        let d = match &notice.on_description {
                                            Some(handler) => {
                                                let handler = handler.clone();
                                                d.id(SharedString::from(format!(
                                                    "{}:description",
                                                    notice.id
                                                )))
                                                .cursor_pointer()
                                                .hover(|s| s.text_color(c.foreground))
                                                .on_click(move |_, window, cx| handler(window, cx))
                                                .into_any_element()
                                            }
                                            None => d.into_any_element(),
                                        };
                                        h_flex().flex_1().min_w_0().child(d).children(details)
                                    })),
                            )
                            .when(
                                !notice.actions.is_empty() || notice.dismiss.is_some(),
                                |row| {
                                    row.child(
                                        h_flex()
                                            .flex_none()
                                            .min_w(px(48.))
                                            .justify_end()
                                            .gap_1()
                                            .children(notice.actions)
                                            .children(notice.dismiss.map(|(label, handler)| {
                                                crate::ui::xs_button(
                                                    SharedString::from(format!(
                                                        "{}:dismiss",
                                                        notice.id
                                                    )),
                                                    "x",
                                                    true,
                                                    crate::ui::Variant::Ghost,
                                                    cx,
                                                )
                                                .rounded_tr(corner)
                                                .tooltip(crate::ui::tooltip(label))
                                                .on_click(move |_, window, cx| handler(window, cx))
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// `tint` laid over `surface`, as the web stacks its two background layers.
fn blend(surface: Hsla, tint: Hsla) -> Hsla {
    let (s, t) = (Rgba::from(surface), Rgba::from(tint));
    let a = t.a + s.a * (1. - t.a);
    let mix = |x: f32, y: f32| (y * t.a + x * s.a * (1. - t.a)) / a.max(f32::EPSILON);
    Rgba {
        r: mix(s.r, t.r),
        g: mix(s.g, t.g),
        b: mix(s.b, t.b),
        a,
    }
    .into()
}

/// An action inside a notice (web: `InlineButton` or `Button size="xs" variant="ghost"`).
pub(crate) fn notice_action(
    id: impl Into<ElementId>,
    label: String,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let c = cx.theme().colors;
    div()
        .id(id)
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded_md()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .when(disabled, |b| b.opacity(0.64))
        .when(!disabled, |b| {
            b.cursor_pointer().hover(move |s| s.bg(c.accent))
        })
        .child(label)
}
