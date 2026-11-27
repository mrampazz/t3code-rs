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
fn reads_as_path(path: &str) -> bool {
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

/// `resolveCodexArtifactTemplate`, as `codexArtifactTemplatePresentationLabel` names it.
fn artifact_template_label(attributes: &HashMap<String, String>) -> Option<String> {
    let kind = match attributes.get("artifact_kind")?.as_str() {
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
    };
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
    valid.then(|| format!("{name} ({kind})"))
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
        ARTIFACT_TEMPLATE => artifact_template_label(&d.attributes),
        _ => None,
    })
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
        assert_eq!(
            for_copy(
                "::artifact-template{artifact_kind=document display_name=\"Memo\" skill_directory=\"/s/m\" skill_name=artifact-template-memo}\n"
            ),
            "Memo (Document template)\n"
        );
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
