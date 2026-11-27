//! File links in chat markdown draw as the web's file chips (`MarkdownFileLink`, `ContextChip`
//! kind `mention`, `FileTagChipContent`): the file's colored icon and "name · L3", the full path
//! on hover. A click opens the file in the side panel at the line (with ⌘ or Ctrl, the editor);
//! the context menu opens it in the editor, reveals it, or copies its relative or full path.

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin};
use gpui_kit::*;

use crate::T3App;
use crate::editors::{LinkTarget, OpenLink, link_target, thread_link_ids};
use crate::i18n::{t, tv};

const NODE: &str = "file-link";

/// A file link as written: its href, path, and the line range it points at.
struct FileLink {
    href: String,
    path: String,
    line: Option<u64>,
    end_line: Option<u64>,
    column: Option<u64>,
    /// `buildFileLinkParentSuffixByPath`: parent folders that tell same-named files apart.
    suffix: Option<String>,
}

/// `parseMarkdownFileLink`'s test: a `file:` URL, or a destination that reads as a path.
fn is_file_link(href: &str) -> bool {
    let href = href.trim();
    if href.len() >= 5 && href[..5].eq_ignore_ascii_case("file:") {
        return true;
    }
    if href.starts_with('#') || href.starts_with("//") || !thread_link_ids(href).is_empty() {
        return false;
    }
    let scheme = href.find(':').filter(|&i| {
        i > 1
            && href[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    });
    if scheme.is_some() {
        return false;
    }
    let path = href.split(['#', '?']).next().unwrap_or("");
    let path = path
        .rsplit_once(':')
        .filter(|(_, n)| n.split('-').all(|p| p.chars().all(|c| c.is_ascii_digit())))
        .map_or(path, |(p, _)| p);
    crate::directives::reads_as_path(&decode(path))
}

fn decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map_or_else(|| s.to_owned(), |(_, v)| v.into_owned())
}

/// The range's end line: `:3-5`, or `#L3-L5`.
fn end_line(href: &str) -> Option<u64> {
    let (before, hash) = href.split_once('#').unwrap_or((href, ""));
    let spec = hash
        .strip_prefix("L")
        .and_then(|s| s.split_once('-').map(|(_, e)| e.trim_start_matches('L')))
        .or_else(|| {
            before
                .rsplit_once(':')
                .and_then(|(_, last)| last.split_once('-').map(|(_, e)| e))
        })?;
    spec.parse().ok().filter(|n| *n > 0)
}

/// `buildFileLinkParentSuffixByPath`.
pub(crate) fn parent_suffixes(paths: &[String]) -> std::collections::HashMap<String, String> {
    use std::collections::{BTreeSet, HashMap};
    let mut groups: HashMap<String, BTreeSet<String>> = HashMap::new();
    for path in paths {
        let normalized = path.replace('\\', "/");
        if let Some(base) = normalized.split('/').filter(|s| !s.is_empty()).next_back() {
            groups
                .entry(base.to_owned())
                .or_default()
                .insert(normalized);
        }
    }
    let mut out = HashMap::new();
    for group in groups.values().filter(|g| g.len() > 1) {
        let parents = |p: &str| -> Vec<String> {
            let segments: Vec<String> = p
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            segments[..segments.len().saturating_sub(1)].to_vec()
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for path in group {
            let segments = parents(path);
            for depth in 1..=segments.len() {
                *counts
                    .entry(segments[segments.len() - depth..].join("/"))
                    .or_default() += 1;
            }
        }
        for path in group {
            let segments = parents(path);
            if segments.is_empty() {
                continue;
            }
            let resolved = (1..=segments.len())
                .find(|&depth| counts[&segments[segments.len() - depth..].join("/")] == 1)
                .unwrap_or(segments.len());
            let depth = segments.len().min(resolved.max(2));
            out.insert(path.clone(), segments[segments.len() - depth..].join("/"));
        }
    }
    out
}

/// `fileLinkLabel`: "name", its distinguishing folders, and "L3", "L3–5" or "L3:C7".
pub(crate) fn label(
    path: &str,
    suffix: Option<&str>,
    line: Option<u64>,
    end: Option<u64>,
    column: Option<u64>,
) -> String {
    let base = path
        .replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_owned();
    let position = line.map(|l| match (end, column) {
        (Some(e), _) => format!("L{l}–{e}"),
        (None, Some(c)) => format!("L{l}:C{c}"),
        _ => format!("L{l}"),
    });
    [Some(base), suffix.map(str::to_owned), position]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

pub struct FileChips(pub WeakEntity<T3App>);

impl MarkdownPlugin for FileChips {
    fn name(&self) -> &str {
        NODE
    }

    fn parse(
        &self,
        node: &markdown::mdast::Node,
        ctx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown::mdast::Node::Link(link) = node else {
            return None;
        };
        if !is_file_link(&link.url) {
            return None;
        }
        let LinkTarget::File { path, line, column } = link_target(&link.url, None) else {
            return None;
        };
        // Same-named files in one message are told apart by their folders.
        static LINKS: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"\]\(<?([^)>\s]+)>?\)").unwrap());
        let paths: Vec<String> = LINKS
            .captures_iter(ctx.source())
            .map(|c| c[1].to_owned())
            .filter(|h| is_file_link(h))
            .filter_map(|h| match link_target(&h, None) {
                LinkTarget::File { path, .. } => Some(path),
                LinkTarget::Url(_) => None,
            })
            .collect();
        let suffix = parent_suffixes(&paths).remove(&path.replace('\\', "/"));
        let text = label(&path, suffix.as_deref(), line, end_line(&link.url), column);
        Some(
            MarkdownNode::new(
                NODE,
                FileLink {
                    href: link.url.clone(),
                    end_line: end_line(&link.url),
                    path,
                    line,
                    column,
                    suffix,
                },
            )
            .text(text),
        )
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(link) = node.data::<FileLink>() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        let (icon, _) = crate::file_icons::for_file(&link.path, dark);
        let text = label(
            &link.path,
            link.suffix.as_deref(),
            link.line,
            link.end_line,
            link.column,
        );
        let app = self.0.clone();
        let cwd = app.upgrade().and_then(|a| a.read(cx).thread_cwd());
        let full = match link_target(&link.href, cwd.as_deref()) {
            LinkTarget::File { path, .. } => path,
            LinkTarget::Url(url) => url,
        };
        let position = link
            .line
            .map(|l| match (link.end_line, link.column) {
                (Some(e), _) => format!(":{l}-{e}"),
                (None, Some(c)) => format!(":{l}:{c}"),
                _ => format!(":{l}"),
            })
            .unwrap_or_default();
        let relative = cwd.as_deref().and_then(|cwd| {
            full.strip_prefix(cwd.trim_end_matches('/'))
                .and_then(|r| r.strip_prefix('/'))
                .map(str::to_owned)
        });
        let id = SharedString::from(format!("file-chip-{}", link.href));
        let (href, line) = (SharedString::from(link.href.clone()), link.line);
        let (click_app, click_relative) = (app.clone(), relative.clone());
        let tooltip = format!("{full}{position}");
        let actions = (
            app.clone(),
            href.clone(),
            relative.clone(),
            full.clone(),
            position.clone(),
        );
        crate::ui::context_chip(
            id,
            crate::ui::chip_ink("mention", cx),
            crate::ui::svg_image(icon, cx).size(rems(1.)),
            text,
        )
        .tooltip(crate::ui::tooltip(tooltip))
        .on_click(move |event, window, cx| {
            let editor = event.modifiers().platform || event.modifiers().control;
            match (&click_relative, editor) {
                (Some(path), false) => {
                    let path = path.clone();
                    click_app
                        .update(cx, |this, cx| {
                            this.show_file(path, line.map(|l| l as u32), window, cx)
                        })
                        .ok();
                }
                _ => window.dispatch_action(Box::new(OpenLink { url: href.clone() }), cx),
            }
        })
        .context_menu(move |menu, _, cx| {
            let (app, href, relative, full, position) = actions.clone();
            let Some(this) = app.upgrade() else {
                return menu;
            };
            let this = this.read(cx);
            let editor = this.favorite_editor();
            let open = match editor.as_deref() {
                None | Some("file-manager") => t("Open in editor"),
                Some(id) => tv(
                    "Open in {editor}",
                    &[("editor", &crate::editors::label(id))],
                ),
            };
            let reveal = this.reveal_label();
            let mut menu = menu.item(PopupMenuItem::new(open).on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(OpenLink { url: href.clone() }), cx)
            }));
            if let Some(reveal) = reveal {
                let (app, full) = (app.clone(), full.clone());
                menu = menu.item(PopupMenuItem::new(reveal).on_click(move |_, _, cx| {
                    app.update(cx, |this, cx| this.reveal_in_file_manager(full.clone(), cx))
                        .ok();
                }));
            }
            if let Some(relative) = relative {
                let value = format!("{relative}{position}");
                menu = menu.item(PopupMenuItem::new(t("Copy relative path")).on_click(
                    move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(value.clone())),
                ));
            }
            let value = format!("{full}{position}");
            menu.item(
                PopupMenuItem::new(t("Copy full path")).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()))
                }),
            )
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{end_line, is_file_link, label, parent_suffixes};

    #[test]
    fn labels_file_links_like_the_web() {
        assert_eq!(
            label("src/app.ts", None, Some(3), None, None),
            "app.ts · L3"
        );
        assert_eq!(label("a/b.rs", None, Some(3), Some(9), None), "b.rs · L3–9");
        assert_eq!(
            label("a/b.rs", None, Some(3), None, Some(7)),
            "b.rs · L3:C7"
        );
        assert_eq!(
            label("x/y/mod.rs", Some("x/y"), None, None, None),
            "mod.rs · x/y"
        );
        assert_eq!(end_line("src/a.ts#L3-L5"), Some(5));
        assert_eq!(end_line("src/a.ts:3-5"), Some(5));
        assert!(is_file_link("src/app.ts#L3"));
        assert!(is_file_link("README"));
        assert!(!is_file_link("https://example.com/a.ts"));
        assert!(!is_file_link("notes"));
        let suffixes = parent_suffixes(&[
            "apps/web/index.ts".into(),
            "apps/server/index.ts".into(),
            "lib/other.ts".into(),
        ]);
        assert_eq!(suffixes["apps/web/index.ts"], "apps/web");
        assert!(!suffixes.contains_key("lib/other.ts"));
    }
}
