//! Thread links in chat markdown draw as chips with the thread's title (web:
//! `MarkdownThreadLink`, `ThreadContextChip`); clicking one opens the thread.
//! shortcut: an unresolved link shows its label (or "Thread") without the web's tooltip.

use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin};
use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::*;

use crate::T3App;
use crate::editors::{OpenLink, thread_link_ids};
use crate::i18n::t;

const NODE: &str = "thread-link";

/// (href, label as written)
struct ThreadLink(String, String);

pub struct ThreadChips(pub WeakEntity<T3App>);

impl MarkdownPlugin for ThreadChips {
    fn name(&self) -> &str {
        NODE
    }

    fn parse(
        &self,
        node: &markdown::mdast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown::mdast::Node::Link(link) = node else {
            return None;
        };
        if thread_link_ids(&link.url).is_empty() {
            return None;
        }
        let label = node.to_string();
        Some(MarkdownNode::new(NODE, ThreadLink(link.url.clone(), label.clone())).text(label))
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(ThreadLink(href, label)) = node.data::<ThreadLink>() else {
            return div().into_any_element();
        };
        let resolved = self
            .0
            .upgrade()
            .and_then(|app| app.read(cx).find_thread_link(href))
            .map(|(_, _, title)| title)
            .filter(|title| !title.trim().is_empty());
        let title = resolved.unwrap_or_else(|| {
            if label.trim().is_empty() || label == href {
                t("Thread")
            } else {
                label.clone()
            }
        });
        let c = cx.theme().colors;
        let url = SharedString::from(href.clone());
        h_flex()
            .id(SharedString::from(format!("thread-chip-{href}")))
            .px_1()
            .rounded_sm()
            .bg(c.secondary)
            .text_color(c.link)
            .cursor_pointer()
            .child(title)
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(OpenLink { url: url.clone() }), cx)
            })
            .into_any_element()
    }
}
