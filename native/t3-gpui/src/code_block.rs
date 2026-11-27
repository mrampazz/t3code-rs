//! Chat markdown code fences framed like the web's `MarkdownCodeBlock`: a header with the fence's
//! file title or its language (as a colored file icon when one exists), and wrap, run and copy.

use gpui_kit::component::text::{MarkdownNode, TextView};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::T3App;
use crate::i18n::t;

const NODE: &str = "code-fence";

struct Fence {
    language: String,
    title: Option<String>,
    code: String,
    closed: bool,
    /// Where the fence starts in the message, so its controls keep their state while it streams.
    start: usize,
}

/// `extractFenceLanguage`: the info string's first word; Shiki has no gitignore grammar.
fn fence_language(lang: Option<&str>) -> String {
    match lang.filter(|l| !l.is_empty()) {
        Some("gitignore") => "ini".into(),
        Some(lang) => lang.into(),
        None => "text".into(),
    }
}

/// `extractFenceTitle`: a `title=`/`file=`/`filename=` attribute, else a file-name-looking word.
fn fence_title(meta: Option<&str>) -> Option<String> {
    static ATTR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:^|\s)(?:title|file(?:name)?)=(?:"([^"]+)"|'([^']+)'|(\S+))"#)
            .unwrap()
    });
    static FILE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[\w@][\w@./-]*\.[A-Za-z0-9]+$").unwrap());
    let meta = meta?.trim();
    if meta.is_empty() {
        return None;
    }
    if let Some(m) = ATTR.captures(meta) {
        return (1..=3)
            .find_map(|i| m.get(i))
            .map(|m| m.as_str().to_owned());
    }
    meta.split_whitespace()
        .find(|w| FILE.is_match(w))
        .map(str::to_owned)
}

/// `isClosedCodeFence`: the source ends with a closer at least as long as its opener. `code`
/// is the parsed value, which drops the newline before the closer.
fn is_closed(source: &str, code: &str) -> bool {
    static OPEN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^(?:`{3,}|~{3,})").unwrap());
    static CLOSE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|\n)[ \t>]*(`{3,}|~{3,})[ \t\r]*$").unwrap()
    });
    let (Some(open), Some(close)) = (
        OPEN.find(source),
        CLOSE.captures(source).and_then(|c| c.get(1)),
    ) else {
        return false;
    };
    let (open, close) = (open.as_str(), close.as_str());
    let lines = |s: &str| s.split("\r\n").flat_map(|l| l.split(['\r', '\n'])).count();
    let code_lines = if code.is_empty() { 0 } else { lines(code) };
    open[..1] == close[..1]
        && close.len() >= open.len()
        && lines(source.strip_suffix('\r').unwrap_or(source)) >= code_lines + 2
}

/// `MarkdownCodeBlock`'s `canRun`: a closed, single-line shell fence where running is offered.
/// Control and invisible format characters could make the shown command differ from the one run.
fn can_run(fence: &Fence, runnable: bool) -> bool {
    static SHELL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:sh|bash|zsh|fish|shell|powershell|pwsh)$").unwrap()
    });
    static HIDDEN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[\p{Cc}\p{Cf}]").unwrap());
    let command = fence.code.trim();
    runnable
        && fence.closed
        && SHELL.is_match(&fence.language)
        && !command.is_empty()
        && !command.ends_with('\\')
        && !HIDDEN.is_match(&fence.code)
}

/// Draws `view`'s code fences (all but mermaid diagrams) in the web's frame. `id` scopes each
/// fence's wrap and copy state. `runnable` offers "Run in terminal" on shell fences, as the web
/// does only in a settled assistant reply.
pub fn with_code_blocks(
    view: TextView,
    id: SharedString,
    app: WeakEntity<T3App>,
    runnable: bool,
) -> TextView {
    view.markdown_block_parser(|node, ctx| match node {
        markdown::mdast::Node::Code(code) if code.lang.as_deref() != Some("mermaid") => {
            let source = ctx.node_source(node).unwrap_or_default();
            let fence = Fence {
                language: fence_language(code.lang.as_deref()),
                title: fence_title(code.meta.as_deref()),
                code: code.value.clone(),
                closed: is_closed(source, &code.value),
                start: ctx.offset() + node.position().map_or(0, |p| p.start.offset),
            };
            Some(
                MarkdownNode::new(NODE, fence)
                    .text(code.value.clone())
                    .markdown(source.to_owned()),
            )
        }
        _ => None,
    })
    .markdown_block_renderer(NODE, move |node, window, cx| match node.data::<Fence>() {
        Some(fence) => render(&id, fence, runnable, &app, window, cx),
        None => div().into_any_element(),
    })
}

fn render(
    id: &SharedString,
    fence: &Fence,
    runnable: bool,
    app: &WeakEntity<T3App>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let c = theme.colors;
    let dark = theme.mode.is_dark();
    let mono = theme.mono_font_family.clone();
    let key = format!("{id}:fence:{}", fence.start);
    let default_wrap = app
        .upgrade()
        .is_none_or(|app| app.read(cx).prefs["wordWrap"] != false);
    let wrap_override =
        window.use_keyed_state(ElementId::Name(format!("{key}:wrap").into()), cx, |_, _| {
            None::<bool>
        });
    let wrapped = wrap_override.read(cx).unwrap_or(default_wrap);

    let file_icon = |name: &str| {
        let (svg, generic) = crate::file_icons::for_file(name, dark);
        (crate::ui::svg_image(svg, cx).size(px(14.)), generic)
    };
    let title = match &fence.title {
        Some(title) => h_flex()
            .gap(px(6.))
            .min_w_0()
            .child(file_icon(title).0)
            .child(div().truncate().child(title.clone()))
            .into_any_element(),
        None => {
            let (icon, generic) =
                file_icon(&crate::file_icons::file_name_for_language(&fence.language));
            if generic {
                div()
                    .truncate()
                    .child(fence.language.clone())
                    .into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("{key}:language")))
                    .flex_none()
                    .rounded_sm()
                    .tooltip(crate::ui::tooltip(fence.language.clone()))
                    .child(icon)
                    .into_any_element()
            }
        }
    };

    let wrap_label = if wrapped {
        t("Disable line wrap")
    } else {
        t("Wrap lines")
    };
    let wrap = crate::ui::xs_button(
        SharedString::from(format!("{key}:wrap-toggle")),
        "text-wrap",
        true,
        if wrapped {
            crate::ui::Variant::Secondary
        } else {
            crate::ui::Variant::GhostMuted
        },
        cx,
    )
    .tooltip(crate::ui::tooltip(wrap_label))
    .on_click(move |_, _, cx| {
        wrap_override.update(cx, |w, cx| {
            *w = Some(!wrapped);
            cx.notify();
        })
    });
    let run = can_run(fence, runnable).then(|| {
        let (app, command) = (app.clone(), fence.code.trim().to_owned());
        crate::ui::xs_button(
            SharedString::from(format!("{key}:run")),
            "play",
            true,
            crate::ui::Variant::GhostMuted,
            cx,
        )
        .tooltip(crate::ui::tooltip(t("Run in terminal")))
        .on_click(move |_, _, cx| {
            app.update(cx, |this, cx| this.run_shell_command(command.clone(), cx))
                .ok();
        })
    });
    let copy = crate::ui::CopyButton::code(
        SharedString::from(format!("{key}:copy")),
        fence.code.clone(),
    );

    let code = TextView::markdown(
        SharedString::from(format!("{key}:code")),
        crate::work_rows::fenced(&fence.language, &fence.code),
    )
    .style(
        crate::ui::markdown_style(cx).code_block(
            StyleRefinement::default()
                .p_0()
                .m_0()
                .border_0()
                .rounded_none()
                .bg(hsla(0., 0., 0., 0.))
                .line_height(relative(1.375)),
        ),
    )
    .code_block_actions(|_, _, _| div())
    .selectable(true);
    let body = div().px(px(14.4)).pt(px(12.8)).pb(px(12.8));
    let body = if wrapped {
        body.child(code).into_any_element()
    } else {
        div()
            .id(SharedString::from(format!("{key}:scroll")))
            .w_full()
            .overflow_x_scroll()
            .child(body.whitespace_nowrap().child(code))
            .into_any_element()
    };

    v_flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded_lg()
        .border_1()
        .map(|frame| {
            if dark {
                frame
                    .border_color(hsla(0., 0., 0., 0.))
                    .bg(c.input.opacity(0.32))
            } else {
                frame.border_color(c.border.opacity(0.7)).bg(c.secondary)
            }
        })
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .pt(px(6.))
                .pr(px(6.))
                .pl_3()
                .text_color(c.foreground.opacity(0.72))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(px(6.))
                        .font_family(mono)
                        .text_size(px(11.))
                        .child(title),
                )
                .child(h_flex().gap(px(2.)).child(wrap).children(run).child(copy)),
        )
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_fence_titles_like_the_web() {
        use super::fence_title;
        assert_eq!(
            fence_title(Some(r#"title="src/a b.ts""#)).as_deref(),
            Some("src/a b.ts")
        );
        assert_eq!(
            fence_title(Some("filename=main.rs")).as_deref(),
            Some("main.rs")
        );
        assert_eq!(
            fence_title(Some("{1,3} lib/app.tsx")).as_deref(),
            Some("lib/app.tsx")
        );
        assert_eq!(fence_title(Some("showLineNumbers")), None);
        assert_eq!(fence_title(None), None);
    }

    #[test]
    fn a_fence_is_closed_only_by_a_matching_closer() {
        use super::is_closed;
        assert!(is_closed("```sh\nls\n```", "ls"));
        assert!(is_closed("````sh\nls\n`````", "ls"));
        assert!(is_closed("```\n```", ""));
        assert!(!is_closed("```sh\nls", "ls"));
        assert!(!is_closed("```sh\nls\n", "ls"));
        assert!(!is_closed("````sh\nls\n```", "ls\n```"));
        assert!(!is_closed("```sh\nls\n~~~", "ls\n~~~"));
    }

    #[test]
    fn runs_only_settled_single_line_shell_fences() {
        use super::{Fence, can_run};
        let fence = |language: &str, code: &str| Fence {
            language: language.into(),
            title: None,
            code: code.into(),
            closed: true,
            start: 0,
        };
        assert!(can_run(&fence("bash", "ls -la"), true));
        assert!(!can_run(&fence("bash", "ls -la"), false));
        assert!(!can_run(&fence("rust", "ls"), true));
        assert!(!can_run(&fence("sh", "ls\npwd"), true));
        assert!(!can_run(&fence("sh", "ls \\"), true));
        assert!(!can_run(&fence("sh", "rm\u{202e}x"), true));
        assert!(!can_run(
            &Fence {
                closed: false,
                ..fence("sh", "ls")
            },
            true
        ));
    }

    #[test]
    fn resolves_language_icons_like_the_web() {
        use crate::file_icons::{file_name_for_language, for_file};
        assert_eq!(file_name_for_language("TypeScript"), "file.ts");
        assert_eq!(file_name_for_language("dockerfile"), "Dockerfile");
        assert!(!for_file(&file_name_for_language("rust"), true).1);
        assert!(for_file(&file_name_for_language("text"), true).1);
        assert!(!for_file("package.json", false).1);
        assert!(!for_file("types.d.ts", false).1);
    }
}
