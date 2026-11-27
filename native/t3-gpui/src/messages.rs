//! User and assistant messages as the web draws them (`TimelineUserMessageRow`,
//! `TimelineAssistantRows`): the user's bubble with its hover actions, and the assistant's
//! copy and time under the turn's last answer.

use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::T3App;
use crate::i18n::t;
use crate::model::TurnItem;

/// Where a user message sits among its neighbours, and whether it can be edited from.
#[derive(Default, Clone, Copy)]
pub(crate) struct UserRow {
    /// `revertTurnCount`: the turn "Edit from here" rewinds to.
    pub revert: Option<u64>,
    /// `isWorking`: the button shows but waits for the turn to end.
    pub revert_disabled: bool,
    /// Another of the user's messages directly above or below: the bubbles join.
    pub grouped_with_previous: bool,
    pub grouped_with_next: bool,
    /// The lane has room beside the bubble (`@min-[37rem]`) for its actions.
    pub wide: bool,
}

/// `ACTIONS_REVEAL_CLASS_NAME` and the assistant's `group-hover/assistant` reveal.
fn reveal(element: Div, group: &SharedString) -> Div {
    element
        .invisible()
        .group_hover(group.clone(), |s| s.visible())
}

/// `formatChatTimestampTooltip`: "3:04 PM, 5th March 2026", or the locale's long date.
pub(crate) fn timestamp_tooltip(at: jiff::Timestamp) -> String {
    let time = crate::timeline::time_of_day(at);
    if crate::i18n::locale() != "en" {
        let date = crate::timeline::system_format(at, "yMMMMd");
        return crate::i18n::tv("{time}, {date}", &[("time", &time), ("date", &date)]);
    }
    let local = at.to_zoned(jiff::tz::TimeZone::system());
    let day = local.day();
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    let month = crate::timeline::system_format(at, "MMMM");
    format!("{time}, {day}{suffix} {month} {}", local.year())
}

/// A message's time, with the full date on hover.
fn message_time(id: String, at: Option<jiff::Timestamp>, cx: &App) -> Option<Stateful<Div>> {
    let at = at?;
    Some(
        div()
            .id(SharedString::from(id))
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground)
            .child(crate::timeline::day_aware_time(at))
            .tooltip(crate::ui::tooltip(timestamp_tooltip(at))),
    )
}

fn item_time(item: &TurnItem, keys: &[&str]) -> Option<jiff::Timestamp> {
    keys.iter().find_map(|k| item.str(k).parse().ok())
}

/// `lineBreaks`: every newline outside a code fence breaks the line, as `remark-breaks` does.
/// Two trailing spaces, unlike a backslash, vanish where the paragraph ends instead.
fn hard_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut fence: Option<String> = None;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let marker: String = match trimmed.chars().next() {
            Some(c @ ('`' | '~')) => trimmed.chars().take_while(|&m| m == c).collect(),
            _ => String::new(),
        };
        let is_marker = match &fence {
            Some(open) if marker.len() >= open.len() && marker.starts_with(&open[..1]) => {
                fence = None;
                true
            }
            None if marker.len() >= 3 => {
                fence = Some(marker);
                true
            }
            _ => false,
        };
        out.push_str(line);
        if lines.peek().is_some() {
            let soft = fence.is_none() && !is_marker && !line.trim().is_empty();
            out.push_str(if soft { "  \n" } else { "\n" });
        }
    }
    out
}

/// `replaceComposerContextReferences(text, label)`: context links read as their labels.
fn plain_text(text: &str) -> String {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"!?\[([^\[\]\n]{0,512})\]\(t3-context://v1/[^\s)]{1,200}\)").unwrap()
    });
    LINK.replace_all(text, "$1").into_owned()
}

/// `promptLinkFor`: a link that opens the prompt in a new thread on the hosted app, or `None`
/// when it is empty or too long for one.
pub(crate) fn prompt_link(prompt: &str) -> Option<String> {
    const ATTACHMENT_ONLY: &str = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
    const MAX: usize = 8_000;
    let bootstrap = prompt
        .strip_prefix("Ultrathink:")
        .map_or(prompt, str::trim_start)
        == ATTACHMENT_ONLY
        || prompt == ATTACHMENT_ONLY;
    let trimmed = if bootstrap { "" } else { prompt.trim() };
    if trimmed.is_empty() || trimmed.encode_utf16().count() > MAX {
        return None;
    }
    let mut url = url::Url::parse("https://app.t3.codes/new").ok()?;
    url.query_pairs_mut().append_pair("prompt", trimmed);
    Some(url.into())
}

impl T3App {
    /// `UserTimelineRow`: the bubble, then its time and actions, shown on hover.
    pub(crate) fn render_user_message(
        &self,
        item: &TurnItem,
        row: UserRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let text = item.str("text").to_string();
        let plain = plain_text(&text);
        let copy_text = if plain == text {
            plain.clone()
        } else {
            text.clone()
        };
        let group: SharedString = format!("user-row-{}", item.id).into();
        let float = row.grouped_with_next && row.wide;

        let edit = row.revert.map(|turn| {
            let text = text.clone();
            crate::ui::xs_button(
                SharedString::from(format!("revert-{}", item.id)),
                "pencil",
                false,
                crate::ui::Variant::Ghost,
                cx,
            )
            .tooltip(crate::ui::tooltip(t("Edit from here")))
            .when(row.revert_disabled, |b| b.opacity(0.64).cursor_default())
            .when(!row.revert_disabled, |b| {
                b.on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_revert(turn, text.clone(), window, cx)
                }))
            })
        });
        let actions = h_flex()
            .justify_end()
            .gap_2()
            .flex_none()
            .children(message_time(
                format!("user-time-{}", item.id),
                item_time(item, &["createdAt", "startedAt", "updatedAt"]),
                cx,
            ))
            .child(
                h_flex()
                    .gap(px(2.))
                    .children(edit)
                    .when(!copy_text.is_empty(), |a| {
                        a.child(crate::ui::CopyButton::message(
                            SharedString::from(format!("copy-{}", item.id)),
                            copy_text.clone(),
                        ))
                    })
                    .children(prompt_link(&plain).map(|link| {
                        crate::ui::CopyButton::message(
                            SharedString::from(format!("prompt-link-{}", item.id)),
                            link,
                        )
                        .label(t("Copy prompt link"))
                        .icon("link")
                    })),
            );

        let bubble = div()
            .min_w_0()
            .p_3()
            .rounded(px(16.))
            .when(row.grouped_with_previous, |b| b.rounded_tr(px(6.)))
            .when(row.grouped_with_next, |b| b.rounded_br(px(6.)))
            .bg(crate::web_theme::current(cx).message)
            .text_color(c.foreground)
            .child(
                crate::code_block::with_code_blocks(
                    TextView::markdown(SharedString::from(item.id.clone()), hard_breaks(&text)),
                    item.id.clone().into(),
                    cx.weak_entity(),
                    false,
                )
                .plugin(crate::thread_chips::ThreadChips(cx.weak_entity()))
                .style(crate::ui::markdown_style(cx))
                .on_link_click(crate::timeline::open_link)
                .selectable(true),
            );

        v_flex()
            .group(group.clone())
            .w_full()
            .items_end()
            .gap_1()
            .map(|col| {
                if float {
                    // Beside the bubble, actions take at most the lane's spare fifth.
                    col.child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .items_center()
                            .gap_2()
                            .child(
                                reveal(div().max_w(relative(0.2)).flex().justify_end(), &group)
                                    .child(actions),
                            )
                            .child(bubble.max_w(relative(0.8))),
                    )
                } else {
                    col.child(bubble.max_w(relative(0.8))).child(
                        reveal(
                            h_flex().w_full().max_w(relative(0.8)).justify_end().pr_1(),
                            &group,
                        )
                        .child(actions),
                    )
                }
            })
            .into_any_element()
    }

    /// `AssistantTimelineRow`: the reply, and under the turn's last answer its meta row.
    /// `meta` is `Some(false)` for a hover meta row, `None` when the answer shows none.
    pub(crate) fn render_assistant_message(
        &self,
        item: &TurnItem,
        meta: Option<bool>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let streaming = item.raw["streaming"] == true || item.str("status") == "running";
        let markdown = crate::code_block::with_code_blocks(
            crate::mermaid::with_mermaid(TextView::markdown(
                SharedString::from(item.id.clone()),
                self.resolve_images(item.str("text"), cx).into_owned(),
            )),
            item.id.clone().into(),
            cx.weak_entity(),
            !streaming,
        )
        .plugin(crate::thread_chips::ThreadChips(cx.weak_entity()))
        .style(crate::ui::markdown_style(cx))
        .on_link_click(crate::timeline::open_link);
        let group: SharedString = format!("assistant-row-{}", item.id).into();
        v_flex()
            .group(group.clone())
            .w_full()
            .child(markdown)
            .map(|col| match meta {
                Some(always) => col.child(
                    self.render_assistant_meta(item, always, &group, cx)
                        .mt(px(6.)),
                ),
                None if item.str("status") == "interrupted" => {
                    col.child(h_flex().mt(px(6.)).child(status_badge("interrupted", cx)))
                }
                None => col,
            })
            .into_any_element()
    }

    /// `AssistantMessageMeta`: the answer's status when it did not complete, copy, and its time.
    /// `always` shows it without hover, as the row of its own after trailing work does.
    pub(crate) fn render_assistant_meta(
        &self,
        item: &TurnItem,
        always: bool,
        group: &SharedString,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = item.str("status");
        let streaming = item.raw["streaming"] == true || status == "running";
        let text = item.str("text");
        let row = h_flex()
            .gap_2()
            .text_xs()
            .when(!matches!(status, "completed" | ""), |r| {
                r.child(status_badge(status, cx))
            })
            .when(!streaming && !text.trim().is_empty(), |r| {
                r.child(crate::ui::CopyButton::message(
                    SharedString::from(format!("assistant-copy-{}", item.id)),
                    text.to_owned(),
                ))
            })
            .when(!streaming, |r| {
                r.children(message_time(
                    format!("assistant-time-{}", item.id),
                    item_time(item, &["updatedAt", "completedAt"]),
                    cx,
                ))
            });
        if always { row } else { reveal(row, group) }
    }
}

/// `AssistantStatusBadge`.
fn status_badge(status: &str, cx: &App) -> Div {
    let c = cx.theme().colors;
    div()
        .rounded_full()
        .border_1()
        .border_color(c.border.opacity(0.7))
        .px(px(6.))
        .py(px(2.))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.))
        .text_color(c.muted_foreground)
        .child(status.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn breaks_lines_outside_code_fences() {
        use super::hard_breaks;
        assert_eq!(hard_breaks("a\nb"), "a  \nb");
        assert_eq!(hard_breaks("a\n\nb"), "a  \n\nb");
        assert_eq!(
            hard_breaks("x\n```\nl1\nl2\n```\ny"),
            "x  \n```\nl1\nl2\n```\ny"
        );
    }

    #[test]
    fn builds_prompt_links_like_the_web() {
        use super::prompt_link;
        assert_eq!(
            prompt_link("  fix the bug & ship  ").as_deref(),
            Some("https://app.t3.codes/new?prompt=fix+the+bug+%26+ship")
        );
        assert_eq!(prompt_link("   "), None);
        assert_eq!(prompt_link(&"x".repeat(8_001)), None);
    }

    #[test]
    fn context_links_copy_as_their_labels() {
        assert_eq!(
            super::plain_text("see [Fix login](t3-context://v1/thread/abc) now"),
            "see Fix login now"
        );
    }

    #[test]
    fn orders_day_suffixes() {
        let at: jiff::Timestamp = "2026-03-02T12:00:00Z".parse().unwrap();
        assert!(super::timestamp_tooltip(at).contains("nd March 2026"));
    }
}
