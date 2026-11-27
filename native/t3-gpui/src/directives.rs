//! Codex's markdown directives (`shared/codexMarkdownDirectives.ts`): `:codex-file-citation{…}`
//! links a file, `:codex-followup[label]{prompt=…}` suggests a reply, and a line of
//! `::artifact-template{…}` offers a template. Code spans and fences keep them as written.

use std::collections::HashMap;

const FILE_CITATION: &str = "codex-file-citation";
const FOLLOW_UP: &str = "codex-followup";
const ARTIFACT_TEMPLATE: &str = "artifact-template";

struct Directive {
    name: &'static str,
    label: Option<String>,
    attributes: HashMap<String, String>,
    /// Byte range in the line.
    range: std::ops::Range<usize>,
}

/// `directiveNameEnds`: the name stops at whitespace or punctuation other than `-` and `_`.
fn name_ends(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || (c.is_ascii_punctuation() && c != '-' && c != '_'))
}

/// `[label]` with balanced brackets on one line; returns the label and the bytes it took.
fn label(rest: &str) -> Option<(String, usize)> {
    let mut depth = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((rest[1..i].to_owned(), i + 1));
                }
            }
            '\n' => return None,
            _ => {}
        }
        if i == 0 && c != '[' {
            return None;
        }
    }
    None
}

/// `{key="v" key='v' key=v key #id .class}`; returns the attributes and the bytes they took.
fn attributes(rest: &str) -> Option<(HashMap<String, String>, usize)> {
    let body = rest.strip_prefix('{')?;
    let end = body.find('}')?;
    let inner = &body[..end];
    let mut out = HashMap::new();
    let mut chars = inner.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut key_end = inner.len();
        while let Some(&(i, c)) = chars.peek() {
            if c.is_whitespace() || c == '=' {
                key_end = i;
                break;
            }
            chars.next();
        }
        let key = &inner[start..key_end];
        if chars.peek().is_some_and(|&(_, c)| c == '=') {
            chars.next();
            let value = match chars.peek().map(|&(_, c)| c) {
                Some(q @ ('"' | '\'')) => {
                    let (open, _) = chars.next()?;
                    let close = inner[open + 1..].find(q)? + open + 1;
                    while chars.peek().is_some_and(|&(i, _)| i <= close) {
                        chars.next();
                    }
                    inner[open + 1..close].to_owned()
                }
                _ => {
                    let from = chars.peek().map_or(inner.len(), |&(i, _)| i);
                    let mut to = inner.len();
                    while let Some(&(i, c)) = chars.peek() {
                        if c.is_whitespace() {
                            to = i;
                            break;
                        }
                        chars.next();
                    }
                    inner[from..to].to_owned()
                }
            };
            out.insert(key.to_owned(), value);
        } else if let Some(id) = key.strip_prefix('#') {
            out.insert("id".into(), id.to_owned());
        } else if let Some(class) = key.strip_prefix('.') {
            out.insert("class".into(), class.to_owned());
        } else if !key.is_empty() {
            out.insert(key.to_owned(), String::new());
        }
    }
    Some((out, end + 2))
}

/// The directive named `name` at `at` (just after its colons), if well formed.
fn parse_at(line: &str, colons: usize, at: usize, names: &[&'static str]) -> Option<Directive> {
    let rest = &line[at..];
    let name = names
        .iter()
        .copied()
        .find(|n| rest.starts_with(n) && name_ends(&rest[n.len()..]))?;
    let mut end = at + name.len();
    let label = label(&line[end..]).map(|(text, used)| {
        end += used;
        text
    });
    let attributes = attributes(&line[end..]).map_or_else(HashMap::new, |(attrs, used)| {
        end += used;
        attrs
    });
    Some(Directive {
        name,
        label,
        attributes,
        range: at - colons..end,
    })
}

/// What each directive in `markdown` becomes; `None` keeps its source.
fn rewrite(markdown: &str, replace: impl Fn(&Directive, bool) -> Option<String>) -> String {
    if !markdown.contains(&format!(":{FILE_CITATION}"))
        && !markdown.contains(&format!(":{FOLLOW_UP}"))
        && !markdown.contains(&format!("::{ARTIFACT_TEMPLATE}"))
    {
        return markdown.to_owned();
    }
    let mut out = String::with_capacity(markdown.len());
    let mut fence: Option<String> = None;
    for line in markdown.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let marker: String = match trimmed.chars().next() {
            Some(c @ ('`' | '~')) => trimmed.chars().take_while(|&m| m == c).collect(),
            _ => String::new(),
        };
        match &fence {
            Some(open) => {
                if marker.len() >= open.len() && marker.starts_with(&open[..1]) {
                    fence = None;
                }
                out.push_str(line);
                continue;
            }
            None if marker.len() >= 3 => {
                fence = Some(marker);
                out.push_str(line);
                continue;
            }
            None => {}
        }
        let indent = body.len() - trimmed.len();
        if indent <= 3
            && trimmed.starts_with("::")
            && let Some(d) = parse_at(body, 2, indent + 2, &[ARTIFACT_TEMPLATE])
            && body[d.range.end..].trim().is_empty()
        {
            match replace(&d, false) {
                Some(text) => {
                    out.push_str(&body[..indent]);
                    out.push_str(&text);
                    out.push_str(&line[body.len()..]);
                }
                None => out.push_str(line),
            }
            continue;
        }
        out.push_str(&rewrite_inline(line, &replace));
    }
    out
}

fn rewrite_inline(line: &str, replace: &impl Fn(&Directive, bool) -> Option<String>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let (mut i, mut copied, mut open_brackets) = (0, 0, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => {
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let fence = "`".repeat(run);
                i = line[i + run..]
                    .find(&fence)
                    .map_or(i + run, |end| i + run + end + run);
            }
            b'[' => {
                open_brackets += 1;
                i += 1;
            }
            b']' => {
                open_brackets = open_brackets.saturating_sub(1);
                i += 1;
            }
            b':' if i == 0 || bytes[i - 1] != b':' => {
                match parse_at(line, 1, i + 1, &[FILE_CITATION, FOLLOW_UP]) {
                    Some(d) => {
                        let inside_link = open_brackets > 0;
                        if let Some(text) = replace(&d, inside_link) {
                            out.push_str(&line[copied..d.range.start]);
                            out.push_str(&text);
                            copied = d.range.end;
                        }
                        i = d.range.end;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out.push_str(&line[copied.min(line.len())..]);
    out
}

/// `resolveCodexFileCitationLink` + `codexFileCitationMarkdown`.
fn file_citation_markdown(attributes: &HashMap<String, String>) -> Option<String> {
    let path = attributes.get("path")?.trim();
    if path.is_empty() {
        return None;
    }
    let line = attributes
        .get("line_range_start")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0 && n <= (1 << 53) - 1);
    let destination = path
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('?', "%3F");
    let destination = if reads_as_path(path) {
        destination
    } else {
        format!("./{destination}")
    };
    let href = match line {
        Some(n) => format!("{destination}#L{n}"),
        None => destination,
    };
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let name = normalized.rsplit('/').next().unwrap_or("");
    let label = if !name.is_empty() {
        name
    } else if !normalized.is_empty() {
        normalized
    } else {
        "File"
    };
    let label: String = label
        .chars()
        .flat_map(|c| {
            let escape = matches!(c, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '&');
            escape.then_some('\\').into_iter().chain(Some(c))
        })
        .collect();
    let href = href
        .replace('\\', "%5C")
        .replace('&', "%26")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    Some(format!("[{label}](<{href}>)"))
}

/// `markdownDestinationPath`'s test: a scheme, a root, or what `parseMarkdownFileLink` reads as a
/// file path keeps its spelling; anything else gets `./` so it stays a relative link.
pub(crate) fn reads_as_path(path: &str) -> bool {
    const EXTENSIONLESS: [&str; 27] = [
        "Makefile",
        "makefile",
        "GNUmakefile",
        "Dockerfile",
        "Containerfile",
        "Justfile",
        "justfile",
        "Rakefile",
        "Gemfile",
        "Procfile",
        "Brewfile",
        "Caddyfile",
        "Vagrantfile",
        "Jenkinsfile",
        "Podfile",
        "Fastfile",
        "BUILD",
        "WORKSPACE",
        "LICENSE",
        "LICENCE",
        "COPYING",
        "NOTICE",
        "AUTHORS",
        "CONTRIBUTORS",
        "CHANGELOG",
        "README",
        "CODEOWNERS",
    ];
    static ROOTED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(?:[a-z][a-z\d+.-]*:|[\\/])").unwrap()
    });
    static ELIDED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[\\/])(?:\.{3,}|…)(?:[\\/]|$)").unwrap()
    });
    static RELATIVE_PATH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:[\p{L}\p{M}\p{N}._$+@()\[\]-]+(?: +[\p{L}\p{M}\p{N}._$+@()\[\]-]+)*/)+(?:[\p{L}\p{M}\p{N}._$+@()\[\]-]+(?: +[\p{L}\p{M}\p{N}._$+@()\[\]-]+)*(?::\d+(?::\d+|-\d+)?)?)?$").unwrap()
    });
    static RELATIVE_NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^[\p{L}\p{M}\p{N}._$+()\[\]-]+(?: +[\p{L}\p{M}\p{N}._$+()\[\]-]+)*\.[\p{L}\p{M}\p{N}_-]+(?::\d+(?::\d+|-\d+)?)?$").unwrap()
    });
    if ROOTED.is_match(path) {
        return true;
    }
    !ELIDED.is_match(path)
        && (path.starts_with("./")
            || path.starts_with("../")
            || path.starts_with("~/")
            || EXTENSIONLESS.contains(&path)
            || RELATIVE_PATH.is_match(path)
            || RELATIVE_NAME.is_match(path))
}

/// `CodexArtifactTemplate`: what an `::artifact-template` line offers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Template {
    pub kind: String,
    pub display_name: String,
    pub skill_name: String,
}

impl Template {
    /// `codexArtifactTemplatePresentationLabel`.
    pub fn label(&self) -> String {
        crate::i18n::t(presentation_label(&self.kind).unwrap_or("Document template"))
    }

    /// The lucide icon for the kind (`ARTIFACT_TEMPLATE_ICON_BY_KIND`).
    pub fn icon(&self) -> &'static str {
        match self.kind.as_str() {
            "presentation" | "google-slides" => "presentation",
            "spreadsheet" | "google-sheets" => "file-spreadsheet",
            "site" => "globe",
            "image" => "image",
            "email" => "mail",
            "slack" => "message-square",
            _ => "file-text",
        }
    }

    /// `codexArtifactTemplateUsePrompt`.
    pub fn use_prompt(&self) -> String {
        let skill = format!("${}", self.skill_name);
        match self.kind.as_str() {
            "presentation" => format!("Create a presentation using the {skill} template about…"),
            "spreadsheet" => format!("Create a spreadsheet using this {skill} about…"),
            "site" => format!("Create a Site using this {skill} about…"),
            "google-docs" => format!("Create a Google Doc using this {skill} about…"),
            "google-slides" => {
                format!("Create a Google Slides presentation using this {skill} about…")
            }
            "google-sheets" => format!("Create a Google Sheet using this {skill} about…"),
            "image" => format!("Create an image using this {skill} of…"),
            "email" => format!("Draft an email using this {skill} about…"),
            "slack" => format!("Draft a Slack message using this {skill} about…"),
            _ => format!("Create a document using this {skill} about…"),
        }
    }
}

/// `codexArtifactTemplatePromptToAppend`: the use prompt, unless the draft already ends with it.
pub(crate) fn template_prompt_to_append(draft: &str, template: &Template) -> Option<String> {
    let prompt = template.use_prompt();
    let trimmed = draft.trim_end();
    let ends = trimmed
        .strip_suffix(prompt.as_str())
        .is_some_and(|before| before.is_empty() || before.ends_with(char::is_whitespace));
    (!ends).then_some(prompt)
}

/// The template a paragraph that is only an `::artifact-template` line offers.
pub(crate) fn template_line(source: &str) -> Option<Template> {
    let body = source.trim();
    if body.contains('\n') {
        return None;
    }
    let d = parse_at(body, 2, 2, &[ARTIFACT_TEMPLATE])
        .filter(|d| body.starts_with("::") && body[d.range.end..].trim().is_empty())?;
    resolve_template(&d.attributes)
}

fn presentation_label(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "document" => "Document template",
        "presentation" => "Presentation template",
        "spreadsheet" => "Spreadsheet template",
        "site" => "Site template",
        "google-docs" => "Google Doc template",
        "google-slides" => "Google Slides template",
        "google-sheets" => "Google Sheet template",
        "image" => "Image template",
        "email" => "Email template",
        "slack" => "Slack template",
        _ => return None,
    })
}

/// `resolveCodexArtifactTemplate`.
fn resolve_template(attributes: &HashMap<String, String>) -> Option<Template> {
    let kind = attributes.get("artifact_kind")?;
    presentation_label(kind)?;
    let name = attributes.get("display_name")?.trim();
    let directory = attributes.get("skill_directory")?;
    static ABSOLUTE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:/[^/]|/$|[A-Za-z]:[\\/]|\\\\[^\\]+\\[^\\]+|//[^/]+/[^/]+)").unwrap()
    });
    let valid = !name.is_empty()
        && ABSOLUTE.is_match(directory)
        && attributes
            .get("skill_name")
            .is_some_and(|s| s.starts_with("artifact-template-"))
        && attributes
            .get("gallery_kind")
            .is_none_or(|g| matches!(g.as_str(), "imagegen" | "product-design"));
    valid.then(|| Template {
        kind: kind.clone(),
        display_name: name.to_owned(),
        skill_name: attributes["skill_name"].clone(),
    })
}

fn follow_up_label(d: &Directive) -> Option<String> {
    let prompt = d.attributes.get("prompt")?;
    let label = d.label.as_ref()?;
    (!prompt.trim().is_empty() && !label.trim().is_empty()).then(|| label.clone())
}

/// `renderCodexDirectivesForCopy`: citations as markdown links, follow-ups as their label, and
/// templates as "Name (Kind template)".
pub fn for_copy(markdown: &str) -> String {
    rewrite(markdown, |d, inside_link| match d.name {
        FILE_CITATION if !inside_link => file_citation_markdown(&d.attributes),
        FOLLOW_UP => follow_up_label(d),
        ARTIFACT_TEMPLATE => resolve_template(&d.attributes).map(|t| {
            format!(
                "{} ({})",
                t.display_name,
                presentation_label(&t.kind).unwrap_or("")
            )
        }),
        _ => None,
    })
}

/// What the transcript shows: citations as links and follow-ups as their label; a template
/// line stays for the card that replaces it (`with_templates`).
pub fn for_display(markdown: &str) -> String {
    rewrite(markdown, |d, inside_link| match d.name {
        FILE_CITATION if !inside_link => file_citation_markdown(&d.attributes),
        FOLLOW_UP => follow_up_label(d),
        _ => None,
    })
}

const TEMPLATE_NODE: &str = "artifact-template";

/// Draws `view`'s `::artifact-template` lines as the web's card (`CodexArtifactTemplateCard`);
/// "Use template" adds its prompt to the composer.
pub(crate) fn with_templates(
    view: gpui_kit::component::text::TextView,
    app: gpui_kit::WeakEntity<crate::T3App>,
) -> gpui_kit::component::text::TextView {
    use gpui_kit::component::text::MarkdownNode;
    view.markdown_block_parser(|node, ctx| match node {
        markdown::mdast::Node::Paragraph(_) => {
            let source = ctx.node_source(node)?;
            let template = template_line(source)?;
            let copy = format!("{} ({})\n\n", template.display_name, template.label());
            Some(
                MarkdownNode::new(TEMPLATE_NODE, template)
                    .text(copy)
                    .markdown(source.to_owned()),
            )
        }
        _ => None,
    })
    .markdown_block_renderer(TEMPLATE_NODE, move |node, _, cx| {
        match node.data::<Template>() {
            Some(template) => template_card(template.clone(), app.clone(), cx),
            None => gpui_kit::IntoElement::into_any_element(gpui_kit::div()),
        }
    })
}

fn template_card(
    template: Template,
    app: gpui_kit::WeakEntity<crate::T3App>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
    use gpui_kit::prelude::FluentBuilder;
    use gpui_kit::*;
    let theme = cx.theme();
    let c = theme.colors;
    let dark = theme.mode.is_dark();
    let web = crate::web_theme::current(cx);
    let shadow = vec![BoxShadow {
        color: hsla(0., 0., 0., 0.05),
        offset: point(px(0.), px(1.)),
        blur_radius: px(2.),
        spread_radius: px(0.),
        inset: false,
    }];
    let id = format!(
        "template-use-{}-{}",
        template.skill_name, template.display_name
    );
    h_flex()
        .w_full()
        .min_w_0()
        .gap_3()
        .rounded(px(12.))
        .border_1()
        .border_color(c.border.opacity(0.7))
        .bg(web.card.opacity(0.6))
        .px_3()
        .py(px(10.))
        .text_color(c.foreground)
        .shadow(shadow.clone())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_3()
                .child(
                    div()
                        .size(px(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .border_1()
                        .border_color(c.border.opacity(0.7))
                        .bg(c.background)
                        .text_color(c.muted_foreground)
                        .shadow(shadow.clone())
                        .child(crate::ui::icon(template.icon()).size(px(20.))),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(template.display_name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .child(template.label()),
                        ),
                ),
        )
        .child(
            div()
                .id(SharedString::from(id))
                .flex_none()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(c.input)
                .cursor_pointer()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .shadow(shadow)
                .map(|b| {
                    if dark {
                        b.bg(c.input.opacity(0.32))
                            .hover(move |s| s.bg(c.input.opacity(0.64)))
                    } else {
                        b.bg(c.popover).hover(move |s| s.bg(c.accent.opacity(0.5)))
                    }
                })
                .child(crate::i18n::t("Use template"))
                .on_click(move |_, window, cx| {
                    let template = template.clone();
                    app.update(cx, |this, cx| this.use_template(&template, window, cx))
                        .ok();
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::for_copy;

    #[test]
    fn copies_directives_as_the_web_does() {
        assert_eq!(
            for_copy("See :codex-file-citation{path=\"src/app.ts\" line_range_start=12} now"),
            "See [app.ts](<src/app.ts#L12>) now"
        );
        for (path, link) in [
            ("README", "[README](<README>)"),
            ("/abs/x.rs", "[x.rs](</abs/x.rs>)"),
            ("a b", "[a b](<./a b>)"),
            ("notes", "[notes](<./notes>)"),
            ("x/...", "[...](<./x/...>)"),
            (".env", "[.env](<./.env>)"),
            ("a#b", "[a#b](<./a%23b>)"),
        ] {
            assert_eq!(
                for_copy(&format!(":codex-file-citation{{path=\"{path}\"}}")),
                link
            );
        }
        assert_eq!(
            for_copy("Try :codex-followup[Run tests]{prompt=\"run the tests\"}."),
            "Try Run tests."
        );
        let line = "::artifact-template{artifact_kind=document display_name=\"Memo\" skill_directory=\"/s/m\" skill_name=artifact-template-memo}";
        assert_eq!(for_copy(&format!("{line}\n")), "Memo (Document template)\n");
        assert_eq!(super::for_display(line), line, "the card replaces the line");
        let template = super::template_line(line).unwrap();
        assert_eq!(
            template.use_prompt(),
            "Create a document using this $artifact-template-memo about…"
        );
        let prompt = template.use_prompt();
        assert_eq!(
            super::template_prompt_to_append(&format!("hi {prompt}  "), &template),
            None
        );
        assert_eq!(
            super::template_prompt_to_append("hi", &template),
            Some(prompt)
        );
        assert!(super::template_line("::artifact-template{artifact_kind=document}").is_none());
        let untouched =
            "`:codex-file-citation{path=a.ts}`\n```\n:codex-followup[x]{prompt=y}\n```\n";
        assert_eq!(for_copy(untouched), untouched);
        assert_eq!(
            for_copy(":codex-file-citation{line_range_start=3}"),
            ":codex-file-citation{line_range_start=3}",
            "a citation without a path keeps its source"
        );
        assert_eq!(
            for_copy("[see :codex-file-citation{path=a.ts}](x)"),
            "[see :codex-file-citation{path=a.ts}](x)",
            "a citation inside a link stays text"
        );
    }
}
