//! Work rows (web: `TimelineWorkEntryRow`, `WorkLog`, `TimelineToolActivityIcon`,
//! `V2ItemInspector`, `CommandOutputPanel`): one line per thought, task list or tool call that
//! opens to its detail. Labels follow `workEntryDisplayLabel` and `summarizeToolGroup` in
//! `apps/web/src/components/chat/MessagesTimeline.logic.ts` and `client-runtime/src/work-log`.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::component::button::Button;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::{t, tc, tp, tv};
use crate::model::TurnItem;
use crate::{T3App, on_tokio};

/// Kinds the web shows as work rows (`WorkEntryLogRow`): work, task lists, notices,
/// notifications, created threads, answered questions and failed provider errors.
pub(crate) fn is_work_row(item: &TurnItem) -> bool {
    match item.kind.as_str() {
        "reasoning" | "command_execution" | "file_change" | "file_search" | "web_search"
        | "dynamic_tool" | "todo_list" | "system_notice" | "notification" | "thread_created" => {
            true
        }
        "error" => item.str("status") == "failed",
        "user_input_request" => item
            .raw
            .get("questionAnswer")
            .is_some_and(|a| a.is_object()),
        _ => false,
    }
}

/// `workEntryViewedImagePath`: the image file a tool opened, if it can be previewed.
pub(crate) fn viewed_image_path(item: &TurnItem) -> Option<&str> {
    item.raw["viewedImagePath"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.contains(['\n', '\r']) && crate::files::is_image(p))
}

/// `workEntriesViewedImagePaths`: distinct images a group opened, in viewing order.
pub(crate) fn viewed_image_paths(items: &[&TurnItem]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for path in items.iter().filter_map(|i| viewed_image_path(i)) {
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
    }
    paths
}

/// `getQuestionAnswerText`: an answer as text, context chips in their readable form.
fn answer_text(value: &Value) -> String {
    match value {
        Value::String(s) => context_text_for_display(s),
        Value::Array(list) => list
            .iter()
            .map(answer_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(o) => o.get("answers").map(answer_text).unwrap_or_default(),
        _ => String::new(),
    }
}

/// `composerContextProviderTextForDisplay`: `[Kind: name; ref=…]` plus its envelope reads as
/// `[Kind: name]`.
fn context_text_for_display(text: &str) -> String {
    static MARKER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[([A-Z][^\[\]:;\n]*): ([^\[\];\n]*); ref=([^\[\]\s]+)\]").unwrap()
    });
    const PREFIX: &str = "\n\n<t3_context version=\"1\">\n<context kind=\"";
    const CLOSE: &str = "</t3_context>";
    let escape = |v: &str| {
        v.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut out = String::new();
    let mut cursor = 0;
    for captures in MARKER.captures_iter(text) {
        let whole = captures.get(0).unwrap();
        if whole.start() < cursor || !text[whole.end()..].starts_with(PREFIX) {
            continue;
        }
        let kind_start = whole.end() + PREFIX.len();
        let Some(kind_end) = text[kind_start..].find('"').map(|k| k + kind_start) else {
            continue;
        };
        if !text[kind_end + 1..].starts_with(&format!(" id=\"{}\"", escape(&captures[3]))) {
            continue;
        }
        let Some(close) = text[kind_end..].find(CLOSE).map(|c| c + kind_end) else {
            continue;
        };
        out.push_str(&text[cursor..whole.start()]);
        out.push_str(&format!("[{}: {}]", &captures[1], &captures[2]));
        cursor = close + CLOSE.len();
    }
    out.push_str(&text[cursor..]);
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `getQuestionTextPreview`.
fn question_text_preview(answer: &Value) -> String {
    answer["questionTextById"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, t)| t.as_str().map(collapse))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `hasQuestionAnswer`.
fn has_answer(answer: &Value) -> bool {
    answer["answers"]
        .as_object()
        .is_some_and(|a| a.values().any(|v| !answer_text(v).is_empty()))
        || answer["attachmentsByQuestionId"]
            .as_object()
            .is_some_and(|a| {
                a.values()
                    .any(|v| v.as_array().is_some_and(|l| !l.is_empty()))
            })
}

/// `getQuestionAnswerPreview`.
fn answer_preview(answer: &Value) -> String {
    let answers: Vec<String> = answer["answers"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, v)| answer_text(v))
        .filter(|t| !t.is_empty())
        .collect();
    let attachments: Vec<String> = answer["attachmentsByQuestionId"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(_, v)| v.as_array().cloned().unwrap_or_default())
        .filter_map(|a| a["name"].as_str().map(str::to_owned))
        .collect();
    let text = if !answers.is_empty() {
        answers.join(" · ")
    } else if !attachments.is_empty() {
        attachments.join(", ")
    } else {
        answer["questionTextById"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    };
    collapse(&text)
}

/// `turnItemIsWorkspacePreparation`, and checkpoints: items the web keeps out of the timeline.
pub(crate) fn hidden_from_timeline(item: &TurnItem) -> bool {
    match item.kind.as_str() {
        "checkpoint" => true,
        "command_execution" => {
            item.str("input") == "Preparing workspace" && item.raw.get("deferredWork").is_none()
        }
        "error" => {
            item.str("status") == "cancelled"
                && item.raw["failure"]["code"] == "workspace_preparation_failed"
                && item.raw.get("deferredWork").is_none()
        }
        _ => false,
    }
}

fn tool_token(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.contains("__") || name.contains(['.', '/']) {
        return None;
    }
    Some(
        name.chars()
            .filter(|c| !matches!(c, '_' | '-') && !c.is_whitespace())
            .collect::<String>()
            .to_lowercase(),
    )
}

/// The T3 Code MCP tool a row calls, as the web words it (`resolveWorkEntryToolPresentation`);
/// `fallback` is the status a row without one reads as.
fn t3_presentation(
    item: &TurnItem,
    fallback: Option<&str>,
) -> Option<crate::t3_tools::Presentation> {
    if item.kind != "dynamic_tool" {
        return None;
    }
    let status = match item.str("status") {
        "pending" | "running" | "waiting" => "inProgress",
        "cancelled" | "interrupted" => "stopped",
        "" => fallback.unwrap_or(""),
        s => s,
    };
    crate::t3_tools::presentation(
        item.str("toolName"),
        &item.raw["input"],
        &item.raw["output"],
        status,
    )
}

/// `toolGroupAction`: read, edit, command, search, code-search, a T3 pull request action,
/// browser, device, or other.
pub(crate) fn action(item: &TurnItem) -> &'static str {
    if let Some(t3) = t3_presentation(item, None) {
        match (t3.action, t3.icon) {
            (Some(action), _) => return action,
            (None, "browser") => return "browser",
            (None, "device") => return "device",
            _ => {}
        }
    }
    match item.kind.as_str() {
        "file_change" => "edit",
        "command_execution" => "command",
        "file_search" => "code-search",
        "web_search" if contains_word(item.str("title"), "grep") => "code-search",
        "web_search" => "search",
        "dynamic_tool" => {
            if item.raw["viewedImagePath"].is_string() {
                return "read";
            }
            match tool_token(item.str("toolName")).as_deref() {
                Some("find" | "grep" | "glob" | "rg" | "ls") => "code-search",
                Some("read" | "readfile") => "read",
                Some("terminal" | "bash" | "shell") => "command",
                _ => "other",
            }
        }
        _ => "other",
    }
}

fn contains_word(text: &str, word: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == word)
}

/// `toolGroupSummaryIconName` for one action.
fn action_icon(action: &str) -> &'static str {
    match action {
        "read" => "eye",
        "edit" => "square-pen",
        "command" => "terminal",
        "search" => "globe",
        "code-search" => "search",
        "link-pr" | "unlink-pr" | "list-prs" | "watch-pr" | "unwatch-pr" => "pull-request",
        "thread-create" => "t3-code",
        "browser" => "browser",
        "device" => "device",
        _ => "wrench",
    }
}

/// `workEntryIconName`: the Lucide icon (or `browser` / `t3-code` mark) for one row.
pub(crate) fn icon_name(item: &TurnItem) -> &'static str {
    let raw = &item.raw;
    match item.kind.as_str() {
        "notification" if item.str("outcome") == "failed" => return "circle-alert",
        "notification" => {
            return match raw["source"]["kind"].as_str() {
                Some("subagent" | "delegated_task") => "bot",
                Some("command") => "terminal",
                Some("monitor") => "eye",
                Some("system") => "t3-code",
                _ => "zap",
            };
        }
        "system_notice" if raw["level"] == "info" => return "zap",
        "system_notice" | "error" => return "circle-alert",
        "user_input_request" | "approval_request" => return "message-circle",
        "todo_list" => return "list-todo",
        "reasoning" => return "brain",
        _ => {}
    }
    if surface(item).is_some() {
        return "browser";
    }
    if item.kind == "dynamic_tool"
        && matches!(item.str("toolName"), "js" | "cua_repl.js" | "node_repl.js")
    {
        return "code";
    }
    if let Some(t3) = t3_presentation(item, None) {
        return t3.icon;
    }
    match action(item) {
        "other" if item.kind == "subagent" => "bot",
        "other" if item.kind == "dynamic_tool" => "wrench",
        "other" => "zap",
        action => action_icon(action),
    }
}

/// `toolGroupSummaryKind` → icon: one shared action's icon, a hammer for mixed groups.
pub(crate) fn group_icon(items: &[&TurnItem]) -> &'static str {
    let tools: Vec<&&TurnItem> = items.iter().filter(|i| i.kind != "reasoning").collect();
    let Some(first) = tools.first() else {
        return "brain";
    };
    if tools
        .iter()
        .all(|i| t3_presentation(i, None).is_some_and(|t| t.icon == "pull-request"))
    {
        return "pull-request";
    }
    let first = action(first);
    if !tools.iter().all(|i| action(i) == first) {
        return "hammer";
    }
    match first {
        "other" => "hammer",
        action => action_icon(action),
    }
}

/// The Lucide file behind a web icon name.
fn lucide(name: &str) -> &str {
    match name {
        "pull-request" => "git-pull-request-arrow",
        "device" => "smartphone",
        "code" => "code-xml",
        name => name,
    }
}

/// `toolSurface` (or a browser/computer `toolSource`).
fn surface(item: &TurnItem) -> Option<&str> {
    let raw = &item.raw;
    raw["toolSurface"]
        .as_str()
        .or_else(|| raw["toolSource"]["kind"].as_str())
        .filter(|s| matches!(*s, "browser" | "computer"))
}

/// `toolIcon ?? toolSource.icon`, validated like `extractToolActivityPresentation`.
fn activity_icon(item: &TurnItem) -> Option<&Value> {
    [&item.raw["toolIcon"], &item.raw["toolSource"]["icon"]]
        .into_iter()
        .find(|icon| {
            matches!(
                icon["_tag"].as_str(),
                Some("website" | "themed-logo" | "native-app")
            )
        })
}

fn web_url(raw: &str, data: bool) -> Option<String> {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    (raw.len() <= 4096
        && (lower.starts_with("https://")
            || lower.starts_with("http://")
            || (data && lower.starts_with("data:"))))
    .then(|| raw.to_owned())
}

/// `toolActivityFaviconUrl` / `themed-logo`: the image URL a website or logo icon shows.
pub(crate) fn icon_url(icon: &Value, dark: bool) -> Option<String> {
    let pick = |light: &str, dark_key: &str| {
        let explicit = |key: &str| icon[key].as_str().and_then(|u| web_url(u, true));
        if dark {
            explicit(dark_key).or_else(|| explicit(light))
        } else {
            explicit(light)
        }
    };
    match icon["_tag"].as_str()? {
        "themed-logo" => pick("logoUrl", "logoUrlDark"),
        "website" => {
            let page = web_url(icon["pageUrl"].as_str()?, false)?;
            let rest = page.split_once("://")?.1;
            let host = rest.split(['/', '?', '#']).next()?.to_owned();
            let origin = format!("{}://{host}", &page[..page.find("://")?]);
            let github = host.eq_ignore_ascii_case("github.com").then(|| {
                format!(
                    "https://github.githubassets.com/favicons/favicon{}.svg",
                    if dark { "-dark" } else { "" }
                )
            });
            if dark {
                icon["faviconUrlDark"]
                    .as_str()
                    .and_then(|u| web_url(u, true))
                    .or(github)
                    .or_else(|| icon["faviconUrl"].as_str().and_then(|u| web_url(u, true)))
            } else {
                icon["faviconUrl"]
                    .as_str()
                    .and_then(|u| web_url(u, true))
                    .or(github)
            }
            .or_else(|| Some(format!("{origin}/favicon.ico")))
        }
        _ => None,
    }
}

/// `workEntryDisplayIndicatesToolFailure`: failed status, a T3 tool's error result, a command's
/// exit code, or output that reads as a failure.
pub(crate) fn failed(item: &TurnItem) -> bool {
    let raw = &item.raw;
    if matches!(item.str("status"), "failed" | "declined") {
        return true;
    }
    if !matches!(
        item.kind.as_str(),
        "command_execution" | "file_change" | "file_search" | "web_search" | "dynamic_tool"
    ) {
        return false;
    }
    if item.kind == "dynamic_tool"
        && crate::t3_tools::definition(item.str("toolName"), &raw["input"]).is_some()
        && crate::t3_tools::result_failed(&raw["output"])
    {
        return true;
    }
    if item.kind == "command_execution" {
        if let Some(code) = raw["exitCode"].as_i64() {
            return code != 0;
        }
        if raw["outputIndicatesFailure"] == true {
            return true;
        }
        if let Some(output) = raw["output"].as_str() {
            let cut = output
                .char_indices()
                .nth(32_768)
                .map_or(output.len(), |(i, _)| i);
            if output_indicates_failure(&output[..cut]) {
                return true;
            }
        }
    }
    let detail = match item.kind.as_str() {
        "file_search" => item.str("pattern").to_owned(),
        "web_search" => raw["patterns"]
            .as_array()
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    !detail.is_empty() && output_indicates_failure(&detail)
}

/// `toolOutputIndicatesFailure`: providers sometimes report completion for output that says
/// the command or file was not found, or that it exited non-zero.
pub(crate) fn output_indicates_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    let has = |needle: &str| lower.contains(needle);
    let hint = [
        "enoent",
        "no such",
        "no files found",
        "not found",
        "commandnotfound",
        "not recognized",
        "cannot find path",
        "cannot be found",
        "exit",
    ]
    .iter()
    .any(|n| has(n));
    hint && ([
        "file not found",
        "no files found",
        "enoent",
        "no such file",
        "commandnotfoundexception",
        "command not found",
        "is not recognized as the name of a cmdlet",
        "a parameter cannot be found that matches parameter name",
    ]
    .iter()
    .any(|n| has(n))
        || (has("cannot find path") && has("because it does not exist"))
        || (has("is not recognized") && has("the term '"))
        || nonzero_exit_code(&lower))
}

/// `exit code: 2`, `exit code 2`, `exited with exit code 2`: a non-zero code after the words.
fn nonzero_exit_code(lower: &str) -> bool {
    lower.match_indices("exit code").any(|(at, needle)| {
        let rest = &lower[at + needle.len()..];
        let trimmed = rest.trim_start();
        let trimmed = trimmed.strip_prefix(':').unwrap_or(trimmed).trim_start();
        if trimmed.len() == rest.len() {
            return false;
        }
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        let boundary = trimmed[digits.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        !digits.is_empty() && !digits.starts_with('0') && boundary
    })
}

fn running(item: &TurnItem) -> bool {
    matches!(
        item.str("status"),
        "idle" | "pending" | "running" | "waiting"
    )
}

/// `formatWorkspaceRelativePath`: paths under the workspace read as `<folder>/rest`.
pub(crate) fn relative_path(path: &str, root: Option<&str>) -> String {
    let path = path.replace('\\', "/");
    let Some(root) = root.map(|r| r.replace('\\', "/").trim_end_matches('/').to_owned()) else {
        return path;
    };
    let label = root.rsplit('/').next().unwrap_or(&root).to_owned();
    if path == root {
        label
    } else if let Some(rest) = path.strip_prefix(&format!("{root}/")) {
        format!("{label}/{rest}")
    } else if path.starts_with('/')
        || path.get(1..3) == Some(":/")
        || path.starts_with("~/")
        || path.starts_with(&format!("{label}/"))
    {
        path
    } else {
        let mut rest = path.as_str();
        while let Some(stripped) = rest.strip_prefix("./") {
            rest = stripped;
        }
        format!("{label}/{rest}")
    }
}

const PATH_KEYS: [&str; 8] = [
    "path",
    "filePath",
    "file_path",
    "relativePath",
    "filename",
    "fileName",
    "newPath",
    "oldPath",
];

/// `collectToolFilePaths`: up to eight paths named anywhere in a tool's input.
fn tool_paths(value: &Value) -> Vec<String> {
    fn walk(value: &Value, out: &mut Vec<String>, depth: usize) {
        if depth > 4 || out.len() >= 8 {
            return;
        }
        if let Some(list) = value.as_array() {
            for v in list {
                walk(v, out, depth + 1);
            }
            return;
        }
        let Some(record) = value.as_object() else {
            return;
        };
        for key in PATH_KEYS {
            if let Some(p) = record.get(key).and_then(Value::as_str).map(str::trim)
                && !p.is_empty()
                && !out.iter().any(|o| o == p)
                && out.len() < 8
            {
                out.push(p.to_owned());
            }
        }
        for key in [
            "locations",
            "item",
            "input",
            "result",
            "rawInput",
            "data",
            "changes",
        ] {
            if let Some(v) = record.get(key) {
                walk(v, out, depth + 1);
            }
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out, 0);
    out
}

/// `formatSearchToolLabel` over a tool's input.
fn search_label(input: &Value) -> Option<String> {
    let first = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input[*k].as_str().map(str::trim).filter(|s| !s.is_empty()))
            .map(str::to_owned)
    };
    let query = first(&["pattern", "query", "searchTerm", "regex", "grep", "needle"]);
    let glob = first(&[
        "glob",
        "globPattern",
        "glob_pattern",
        "include",
        "filePattern",
        "file_pattern",
    ]);
    let target = first(&[
        "path",
        "target_directory",
        "targetDirectory",
        "directory",
        "cwd",
        "root",
    ])
    .and_then(|t| {
        t.split(['/', '\\'])
            .rfind(|p| !p.is_empty() && *p != ".")
            .map(str::to_owned)
    });
    Some(match (query, glob, target) {
        (Some(query), _, Some(target)) => tv(
            "Searched {query} in {target}",
            &[("query", &query), ("target", &target)],
        ),
        (None, Some(glob), Some(target)) => tv(
            "Searched files {glob} in {target}",
            &[("glob", &glob), ("target", &target)],
        ),
        (None, Some(glob), None) => tv("Searched files {glob}", &[("glob", &glob)]),
        (Some(query), _, None) => tv("Searched {query}", &[("query", &query)]),
        (None, None, Some(target)) => tv("Searched in {target}", &[("target", &target)]),
        (None, None, None) => return None,
    })
}

/// `formatReadToolLabel`.
fn read_label(path: &str, extra: usize) -> String {
    match (path.trim(), extra) {
        ("", 0) => t("Read file"),
        ("", n) => tv("Read file +{count} more", &[("count", &n.to_string())]),
        (path, 0) => tv("Read {path}", &[("path", path)]),
        (path, n) => tv(
            "Read {path} +{count} more",
            &[("path", path), ("count", &n.to_string())],
        ),
    }
}

/// `dynamicToolTitle`: the JavaScript REPL's own title, or a Claude skill's name.
fn dynamic_title(item: &TurnItem) -> Option<String> {
    let name = item.str("toolName");
    let input = &item.raw["input"];
    if matches!(name, "js" | "cua_repl.js" | "node_repl.js") {
        let title = item.str("title").trim();
        let meaningful = (!title.is_empty()
            && !title.eq_ignore_ascii_case("js")
            && !title.eq_ignore_ascii_case(name))
        .then_some(title);
        return Some(
            input["title"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or(meaningful)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if surface(item) == Some("computer") {
                        t("Computer use")
                    } else {
                        t("Run JavaScript")
                    }
                }),
        );
    }
    let skill = input["skill"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    (name == "Skill")
        .then_some(skill)
        .flatten()
        .map(|skill| tv("Skill: {name}", &[("name", skill)]))
}

/// `commandDisplayText`.
pub(crate) fn command_text(command: &str) -> String {
    crate::command_label::display_text(command)
}

pub(crate) fn visible_controls(text: &str) -> String {
    crate::command_label::visible_controls(text)
}

/// `commandlessCommandLabel` / `genericCommandStatusLabel`.
fn command_status_label(item: &TurnItem) -> String {
    t(match item.str("status") {
        "failed" => "Failed command",
        "declined" => "Declined command",
        "stopped" | "interrupted" | "cancelled" => "Stopped command",
        "idle" | "pending" | "running" | "waiting" => "Running command",
        _ => "Ran command",
    })
}

/// The changed paths of an edit (`changedFiles`).
fn changed_files(item: &TurnItem) -> Vec<String> {
    match item.raw["changes"].as_array().filter(|c| !c.is_empty()) {
        Some(changes) => changes
            .iter()
            .filter_map(|c| c["path"].as_str().map(str::to_owned))
            .collect(),
        None => vec![item.str("fileName").to_owned()],
    }
}

/// A thought as its row's one line (`thoughtPreviewMarkdown`): Markdown marks dropped,
/// whitespace collapsed.
pub(crate) fn thought_preview(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' | '*' | '~' => {}
            '#' if out.is_empty() || out.ends_with(' ') => {
                while chars.peek() == Some(&'#') {
                    chars.next();
                }
            }
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out.trim().to_owned()
}

/// `workEntryDisplayLabel` (`root` given), or `singleToolCallLabel` for a row on its own.
pub(crate) fn label(item: &TurnItem, root: Option<&str>, single: bool) -> String {
    let raw = &item.raw;
    let title = item.str("title").trim();
    match item.kind.as_str() {
        "reasoning" => {
            let thought = thought_preview(item.str("text"));
            return if !thought.is_empty() {
                thought
            } else if single {
                tc("Thought", "noun, a reasoning step")
            } else {
                tc("Thinking", "status of an agent reasoning")
            };
        }
        "todo_list" => {
            let steps = raw["steps"].as_array().cloned().unwrap_or_default();
            let done = steps.iter().filter(|s| s["status"] == "completed").count();
            let current = steps
                .iter()
                .find(|s| s["status"] == "running")
                .or_else(|| steps.iter().find(|s| s["status"] == "pending"));
            let (done, total) = (done.to_string(), steps.len().to_string());
            return match current {
                Some(step) => tv(
                    "Tasks {done}/{total} · {current}",
                    &[
                        ("done", &done),
                        ("total", &total),
                        ("current", step["text"].as_str().unwrap_or("")),
                    ],
                ),
                None => tv(
                    "Tasks {done}/{total}",
                    &[("done", &done), ("total", &total)],
                ),
            };
        }
        "system_notice" => return item.str("message").to_owned(),
        "notification" => return item.str("summary").to_owned(),
        "thread_created" => return t("Created thread"),
        "user_input_request" => {
            let preview = question_text_preview(&raw["questionAnswer"]);
            return if !preview.is_empty() {
                preview
            } else if !title.is_empty() {
                title.to_owned()
            } else if raw["questionAnswer"].is_object() {
                t("Answered questions")
            } else {
                t("Input requested")
            };
        }
        "file_change" if single => return tp(EDIT_COUNT, changed_files(item).len(), &[]),
        "web_search" if single => {
            return if title.is_empty() {
                t("Web search")
            } else {
                title.to_owned()
            };
        }
        _ => {}
    }
    if let Some(t3) = t3_presentation(item, single.then_some("completed")) {
        return t3.display_name;
    }
    if item.kind == "dynamic_tool"
        && let Some(title) = dynamic_title(item)
    {
        return title;
    }
    let root = if single { None } else { root };
    if item.kind == "command_execution" {
        let input = item.str("input");
        return if input.trim().is_empty() {
            let generic = matches!(title, "" | "Ran command" | "Run command" | "Command");
            if generic {
                command_status_label(item)
            } else {
                title.to_owned()
            }
        } else {
            command_text(input)
        };
    }
    let action = action(item);
    let input = &raw["input"];
    if matches!(action, "code-search" | "search") {
        let found = if item.kind == "file_search" {
            Some(if title.is_empty() {
                search_label(raw).unwrap_or_else(|| t("Searched files"))
            } else {
                title.to_owned()
            })
        } else {
            search_label(input)
        };
        if let Some(found) = found {
            return found;
        }
    }
    if action == "read" {
        let paths: Vec<String> = tool_paths(&json!({ "input": input }))
            .iter()
            .map(|p| relative_path(p, root))
            .collect();
        if let Some(first) = paths.first() {
            return read_label(first, paths.len() - 1);
        }
    }
    let detail = match item.kind.as_str() {
        "file_search" => item.str("pattern").to_owned(),
        "web_search" => raw["patterns"]
            .as_array()
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    let search_output = matches!(action, "code-search" | "search") && detail.contains('\n');
    if !detail.trim().is_empty() && action != "read" && !search_output {
        return detail.trim().to_owned();
    }
    if item.kind == "file_change" {
        let files = changed_files(item);
        let first = relative_path(&files[0], root);
        return if files.len() == 1 {
            first
        } else {
            tp(
                "{count, plural, one {{path} +{count} more} other {{path} +{count} more}}",
                files.len() - 1,
                &[("path", &first)],
            )
        };
    }
    if action == "read" {
        return t("Read file");
    }
    let heading = if !title.is_empty() {
        title.to_owned()
    } else {
        match item.kind.as_str() {
            "dynamic_tool" => {
                let name = item.str("toolName");
                if name.is_empty() {
                    tc("Tool call", "noun")
                } else {
                    name.to_owned()
                }
            }
            "file_search" => t("Searched files"),
            "web_search" => t("Searched the web"),
            kind => kind.replace('_', " "),
        }
    };
    let heading = heading
        .trim_end_matches(" completed")
        .trim_end_matches(" complete")
        .trim();
    let mut chars = heading.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

const EDIT_COUNT: &str =
    "{count, plural, one {Changed {count} file} other {Changed {count} files}}";

/// `toolGroupActionLabel`.
fn action_label(action: &str, count: usize) -> String {
    tp(
        match action {
            "read" => "{count, plural, one {Read {count} file} other {Read {count} files}}",
            "edit" => EDIT_COUNT,
            "command" => "{count, plural, one {Ran {count} command} other {Ran {count} commands}}",
            "search" => {
                "{count, plural, one {Searched the web {count} time} other {Searched the web {count} times}}"
            }
            "code-search" => {
                "{count, plural, one {Searched code {count} time} other {Searched code {count} times}}"
            }
            "thread-create" => {
                "{count, plural, one {Created {count} thread} other {Created {count} threads}}"
            }
            "device" => {
                "{count, plural, one {Used device controls {count} time} other {Used device controls {count} times}}"
            }
            "browser" => {
                "{count, plural, one {Used browser {count} time} other {Used browser {count} times}}"
            }
            "update" => {
                "{count, plural, one {Received {count} update} other {Received {count} updates}}"
            }
            "link-pr" => {
                "{count, plural, one {Linked {count} pull request} other {Linked {count} pull requests}}"
            }
            "unlink-pr" => {
                "{count, plural, one {Unlinked {count} pull request} other {Unlinked {count} pull requests}}"
            }
            "watch-pr" => {
                "{count, plural, one {Watching {count} pull request} other {Watching {count} pull requests}}"
            }
            "unwatch-pr" => {
                "{count, plural, one {Stopped watching {count} pull request} other {Stopped watching {count} pull requests}}"
            }
            "list-prs" => {
                "{count, plural, =1 {Checked linked pull requests} other {Checked linked pull requests {count} times}}"
            }
            _ => "{count, plural, one {Used {count} tool} other {Used {count} tools}}",
        },
        count,
        &[],
    )
}

/// `joinClauses`.
fn join_clauses(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => tv(
            "{first} and {second}",
            &[("first", first), ("second", second)],
        ),
        [rest @ .., last] => tv(
            "{items}, and {last}",
            &[("items", &rest.join(", ")), ("last", last)],
        ),
    }
}

/// `summarizeToolGroup`: the two most telling actions, then a count of the rest.
pub(crate) fn summarize(items: &[&TurnItem]) -> String {
    let tools: Vec<&&TurnItem> = items.iter().filter(|i| i.kind != "reasoning").collect();
    if tools.is_empty() {
        return if items.len() == 1 {
            tc("Thought", "noun, a reasoning step")
        } else {
            tv("Thought (×{count})", &[("count", &items.len().to_string())])
        };
    }
    // (key, T3 action?, entries); integrations with a source name are told apart by name.
    let mut groups: Vec<(String, bool, Vec<&TurnItem>)> = Vec::new();
    let mut sources: Vec<(String, bool)> = Vec::new();
    for item in &tools {
        let t3 = (item.kind == "dynamic_tool")
            .then(|| crate::t3_tools::summary_action(item.str("toolName"), &item.raw["input"]))
            .flatten();
        if t3.is_none()
            && let Some(name) = item.raw["toolSource"]["name"].as_str()
        {
            let integration = item.raw["toolSource"]["kind"] == "integration";
            if !sources.iter().any(|(n, _)| n == name) {
                sources.push((name.to_owned(), integration));
            }
            continue;
        }
        let (key, is_t3) = match t3 {
            Some(action) => (action, true),
            None => (action(item).to_owned(), false),
        };
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(group) => group.2.push(item),
            None => groups.push((key, is_t3, vec![item])),
        }
    }
    let mut summaries: Vec<(usize, u8, usize, String)> = groups
        .iter()
        .enumerate()
        .map(|(index, (action, is_t3, entries))| {
            let priority = match action.as_str() {
                "command" | "edit" => 0,
                "other" | "update" => 2,
                a if *is_t3 => crate::t3_tools::priority(a),
                _ => 1,
            };
            let label = if *is_t3 {
                let calls: Vec<crate::t3_tools::Call> = entries
                    .iter()
                    .map(|entry| crate::t3_tools::Call {
                        input: &entry.raw["input"],
                        output: &entry.raw["output"],
                        outcome: if matches!(entry.str("status"), "failed" | "declined") {
                            "failed"
                        } else if entry.str("status") == "completed" {
                            "completed"
                        } else {
                            "unfinished"
                        },
                    })
                    .collect();
                crate::t3_tools::summarize(action, &calls).0
            } else {
                let count = if action == "edit" {
                    let mut files: Vec<String> = Vec::new();
                    let mut without = 0;
                    for entry in entries {
                        let changed = changed_files(entry);
                        if changed.iter().all(|f| f.is_empty()) {
                            without += 1;
                        }
                        for f in changed.into_iter().filter(|f| !f.is_empty()) {
                            if !files.contains(&f) {
                                files.push(f);
                            }
                        }
                    }
                    files.len() + without
                } else {
                    entries.len()
                };
                action_label(action, count)
            };
            (index, priority, entries.len(), label)
        })
        .collect();
    summaries.sort_by_key(|s| (s.1, s.0));
    summaries.truncate(2);
    summaries.sort_by_key(|s| s.0);
    let mut labels: Vec<String> = summaries.iter().map(|s| s.3.clone()).collect();
    if !sources.is_empty() {
        let names: Vec<String> = sources.iter().map(|(n, _)| n.clone()).collect();
        let joined = join_clauses(&names);
        labels.insert(
            0,
            if sources.iter().all(|(_, integration)| *integration) {
                tp(
                    "{count, plural, =1 {Used {names} integration} other {Used {names} integrations}}",
                    names.len(),
                    &[("names", &joined)],
                )
            } else {
                tv("Used {names}", &[("names", &joined)])
            },
        );
    }
    let sourced = tools
        .iter()
        .filter(|i| {
            i.raw["toolSource"]["name"].is_string()
                && crate::t3_tools::definition(i.str("toolName"), &i.raw["input"]).is_none()
        })
        .count();
    let shown: usize = summaries.iter().map(|s| s.2).sum();
    let rest = tools.len() - sourced - shown;
    if rest > 0 {
        labels.push(tp(
            "{count, plural, one {Performed {count} other action} other {Performed {count} other actions}}",
            rest,
            &[],
        ));
    }
    let labels: Vec<String> = labels
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l
            } else {
                let mut chars = l.chars();
                chars
                    .next()
                    .map(|c| c.to_lowercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        })
        .collect();
    join_clauses(&labels)
}

/// `turnItemDetailRevision`: one key while an item runs, its update time once settled.
fn detail_revision(item: &TurnItem) -> String {
    if running(item) {
        "live".into()
    } else {
        item.str("updatedAt").to_owned()
    }
}

/// `turnItemNeedsDetailFetch`: the wire withheld output (or summarized a large input).
pub(crate) fn needs_fetch(item: &TurnItem) -> bool {
    let summarized = |v: &Value| v["truncated"] == true && v["summary"].is_string();
    match item.kind.as_str() {
        "command_execution" => item.raw["outputOmitted"] == true,
        "dynamic_tool" => item.raw["outputOmitted"] == true || summarized(&item.raw["input"]),
        _ => false,
    }
}

/// `turnItemHasDetail`: whether the row opens at all.
pub(crate) fn has_detail(item: &TurnItem) -> bool {
    let raw = &item.raw;
    let nonempty = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    let any = |v: &Value| v.as_array().is_some_and(|a| !a.is_empty());
    match item.kind.as_str() {
        "reasoning" => nonempty(&raw["text"]),
        "command_execution" => {
            nonempty(&raw["input"])
                || raw["outputOmitted"] == true
                || nonempty(&raw["output"])
                || raw["exitCode"].as_i64().is_some_and(|c| c != 0)
        }
        "file_change" => true,
        "file_search" => any(&raw["results"]) || nonempty(&raw["pattern"]),
        "web_search" => any(&raw["results"]) || any(&raw["patterns"]),
        "dynamic_tool" => {
            raw["outputOmitted"] == true || {
                let (args, text) = call_lines(&raw["input"]);
                args.is_some() || text.is_some()
            }
        }
        _ if viewed_image_path(item).is_some() => true,
        "todo_list" => any(&raw["steps"]),
        "notification" => nonempty(&raw["detail"]),
        "system_notice" => nonempty(&raw["message"]),
        "user_input_request" => raw["questionAnswer"].is_object() || any(&raw["questions"]),
        _ => false,
    }
}

/// `toolCallLines` for structured input: `key value` lines, or the whole value as text.
fn call_lines(input: &Value) -> (Option<Vec<(String, String)>>, Option<String>) {
    match input.as_object() {
        Some(record) if !(input["truncated"] == true && input["summary"].is_string()) => {
            let lines: Vec<(String, String)> = record
                .iter()
                .map(|(k, v)| {
                    let value = match v.as_str() {
                        Some(s) if !s.is_empty() => s.to_owned(),
                        _ => v.to_string(),
                    };
                    (k.clone(), value)
                })
                .collect();
            ((!lines.is_empty()).then_some(lines), None)
        }
        _ => (None, format_tool_value(input)),
    }
}

/// `textFromBlocks`: MCP/ACP content blocks as plain text, `None` when they are not only text.
fn text_from_blocks(value: &Value, depth: usize) -> Option<String> {
    if depth > 4 {
        return None;
    }
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(list) => {
            let parts: Option<Vec<String>> = list
                .iter()
                .map(|b| text_from_blocks(b, depth + 1))
                .collect();
            Some(
                parts?
                    .into_iter()
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        Value::Object(record) => {
            match record.get("type").and_then(Value::as_str) {
                Some("text") if record["text"].is_string() => {
                    return record["text"].as_str().map(str::to_owned);
                }
                Some("image") => return Some(String::new()),
                Some("resource_link") if record["uri"].is_string() => {
                    return record["uri"].as_str().map(str::to_owned);
                }
                Some("resource") if record["resource"].is_object() => {
                    let resource = &record["resource"];
                    return resource["text"]
                        .as_str()
                        .or(resource["uri"].as_str())
                        .map(str::to_owned);
                }
                _ => {}
            }
            let keys: Vec<&String> = record
                .keys()
                .filter(|k| *k != "isError" && *k != "is_error")
                .collect();
            if keys.len() == 1 && keys[0] == "content" {
                return text_from_blocks(&record["content"], depth + 1);
            }
            if keys.len() == 2
                && record.contains_key("content")
                && let Some(structured) = record.get("structuredContent")
            {
                let text = text_from_blocks(&record["content"], depth + 1);
                if text.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    return format_json_value(structured);
                }
                return text;
            }
            None
        }
        _ => None,
    }
}

fn format_json_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        v => {
            let json = serde_json::to_string_pretty(v).ok()?;
            (json != "{}" && json != "[]").then_some(json)
        }
    }
}

fn pretty_json_text(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with(['[', '{']) {
        return text.to_owned();
    }
    if let Ok(whole) = serde_json::from_str::<Value>(trimmed) {
        return serde_json::to_string_pretty(&whole).unwrap_or_else(|_| text.to_owned());
    }
    let docs: Option<Vec<Value>> = trimmed
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l.trim()).ok())
        .collect();
    match docs {
        Some(docs) => docs
            .iter()
            .filter_map(|d| serde_json::to_string_pretty(d).ok())
            .collect::<Vec<_>>()
            .join("\n\n"),
        None => text.to_owned(),
    }
}

/// `formatToolValue`.
pub(crate) fn format_tool_value(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    match text_from_blocks(value, 0) {
        Some(text) if text.trim().is_empty() => None,
        Some(text) => Some(pretty_json_text(&text)),
        None => format_json_value(value),
    }
}

/// `isJsonToolText`.
fn is_json(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok() || {
        let docs: Vec<&str> = text.trim().split("\n\n").collect();
        docs.len() > 1
            && docs
                .iter()
                .all(|d| serde_json::from_str::<Value>(d.trim()).is_ok())
    }
}

/// `turnItemOutputText`.
pub(crate) fn output_text(item: &Value) -> Option<String> {
    match item["type"].as_str()? {
        "command_execution" => {
            let output = item["output"].as_str()?;
            let output = claude_bash_envelope(item, output).unwrap_or_else(|| output.to_owned());
            (!output.trim().is_empty()).then_some(output)
        }
        "dynamic_tool" if item["outputOmitted"] == true => None,
        "dynamic_tool" => {
            let output = &item["output"];
            if output.get("__t3McpApp").is_some() && output.get("result").is_some() {
                let result = &output["result"];
                let structured = result
                    .get("structuredContent")
                    .filter(|s| s.as_object().is_some_and(|o| !o.is_empty()) || s.is_array());
                return format_tool_value(structured.unwrap_or(&result["content"]));
            }
            format_tool_value(output)
        }
        _ => None,
    }
}

/// `commandItemOutput`: Claude rows from early October nightlies stored the raw Bash envelope.
fn claude_bash_envelope(item: &Value, output: &str) -> Option<String> {
    let started = item["startedAt"].as_str().or(item["updatedAt"].as_str())?;
    if item["nativeItemRef"]["driver"] != "claudeAgent"
        || started >= "2026-10-08"
        || !output.trim_start().starts_with("{\"stdout\"")
    {
        return None;
    }
    let parsed: Value = serde_json::from_str(output).ok()?;
    let (out, err) = (parsed["stdout"].as_str()?, parsed["stderr"].as_str()?);
    let out = if out.trim().is_empty() { "" } else { out };
    let err = if err.trim().is_empty() { "" } else { err };
    Some(
        if !out.is_empty() && !err.is_empty() && !out.ends_with('\n') {
            format!("{out}\n{err}")
        } else {
            format!("{out}{err}")
        },
    )
}

/// `toolOutputImages`: how many image blocks a tool returned (each is a `tool-output-image`).
fn output_image_count(item: &Value) -> usize {
    if item["type"] != "dynamic_tool" || item["outputOmitted"] == true {
        return 0;
    }
    let output = &item["output"];
    let blocks: Vec<&Value> = match output {
        Value::Array(list) => list.iter().collect(),
        Value::Object(o) if o.get("content").is_some_and(Value::is_array) => {
            o["content"].as_array().unwrap().iter().collect()
        }
        v => vec![v],
    };
    blocks
        .into_iter()
        .filter(|b| {
            b["type"] == "image" && {
                let mime = b["source"]["media_type"]
                    .as_str()
                    .or(b["mimeType"].as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                (b["source"].is_null() || b["source"]["type"] == "base64")
                    && matches!(
                        mime.as_str(),
                        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                    )
            }
        })
        .take(8)
        .count()
}

/// The web's terminal palette (`TERMINAL_PALETTES`), dark then light.
pub(crate) const PALETTE: [[u32; 16]; 2] = [
    [
        0x3b4048, 0xf47067, 0x57ab5a, 0xc69026, 0x539bf5, 0xb083f0, 0x39c5cf, 0xadbac7, 0x768390,
        0xff938a, 0x6bc46d, 0xdaaa3f, 0x6cb6ff, 0xdcbdfb, 0x56d4dd, 0xcdd9e5,
    ],
    [
        0x24292f, 0xcf222e, 0x116329, 0x7d4e00, 0x0550ae, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a,
        0xa40e26, 0x1a7f37, 0x633c01, 0x0969da, 0x6639ba, 0x3192aa, 0x8c959f,
    ],
];

/// `terminalOutputSpans` resolved for gpui: plain text and a highlight per styled run.
pub(crate) fn terminal_spans(
    text: &str,
    dark: bool,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let palette = &PALETTE[usize::from(!dark)];
    let (plain, runs) = crate::terminal_output::spans(text, 2_000);
    let color = |c: u32| Hsla::from(gpui_kit::rgb(c));
    let styles = runs
        .into_iter()
        .filter_map(|(range, pen)| {
            let style = crate::terminal_output::span_style(&pen, palette)?;
            Some((
                range,
                HighlightStyle {
                    color: style.color.map(color),
                    background_color: style.background.map(color),
                    font_weight: style.bold.then_some(FontWeight::SEMIBOLD),
                    font_style: style.italic.then_some(FontStyle::Italic),
                    fade_out: style.dim.then_some(0.35),
                    underline: style.underline.then(|| UnderlineStyle {
                        thickness: px(1.),
                        ..Default::default()
                    }),
                    strikethrough: style.strike.then(|| StrikethroughStyle {
                        thickness: px(1.),
                        ..Default::default()
                    }),
                },
            ))
        })
        .collect();
    (plain, styles)
}

/// One expanded command row's output (`CommandOutputView`), fed by `subscribeCommandOutput`.
#[derive(Default)]
pub(crate) struct CommandOutput {
    pub(crate) output: crate::terminal_output::TerminalOutput,
    pub(crate) running: bool,
    loaded: bool,
    error: Option<String>,
    /// The stream's request id while it runs.
    request: Option<String>,
}

impl CommandOutput {
    /// `applyCommandOutputFrame`: a `replace` starts over from the whole bounded tail, an
    /// `append` folds raw terminal text onto what is there.
    pub(crate) fn apply(&mut self, frame: &Value) {
        let base = if frame["kind"] == "replace" {
            crate::terminal_output::TerminalOutput::default()
        } else {
            std::mem::take(&mut self.output)
        };
        let mut output = base.append(frame["text"].as_str().unwrap_or(""));
        output.truncated |= frame["truncated"] == true;
        self.output = output;
        self.running = frame["running"] != false;
        self.loaded = true;
    }

    pub(crate) fn text(&self) -> &str {
        &self.output.text
    }
}

/// A fetched `getTurnItem`: the full item once loaded, or why it is not.
#[derive(Clone)]
pub(crate) enum Fetched {
    Pending,
    Item(Value),
    Failed(String),
}

/// Expanded rows' streams and fetches; render reads them and starts what is missing.
#[derive(Default)]
pub(crate) struct WorkDetails {
    outputs: RefCell<HashMap<String, CommandOutput>>,
    fetched: RefCell<HashMap<(String, String), Fetched>>,
    /// A repaint for the sweep's next step is scheduled.
    sweep_tick: std::cell::Cell<bool>,
    /// The work row under the pointer.
    hovered: RefCell<Option<String>>,
    /// Whether each notice's label was cut off when last painted.
    cut_off: RefCell<HashMap<String, std::rc::Rc<std::cell::Cell<bool>>>>,
    /// Each live row's content width as last painted, so its icon can follow the sweep.
    sweep_widths: RefCell<HashMap<String, std::rc::Rc<std::cell::Cell<f32>>>>,
}

impl WorkDetails {
    /// Stops every stream (thread switch, reconnect).
    pub(crate) fn reset(&self, client: Option<&crate::rpc::RpcClient>) {
        for (_, output) in self.outputs.borrow_mut().drain() {
            if let (Some(client), Some(request)) = (client, output.request) {
                client.cancel(&request);
            }
        }
        self.fetched.borrow_mut().clear();
    }
}

fn mono(cx: &App) -> SharedString {
    cx.theme().mono_font_family.clone()
}

/// A code fence that never closes early, whatever backticks the code holds.
fn fenced(lang: &str, code: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in code.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{lang}\n{code}\n{fence}")
}

impl T3App {
    fn toggle_work(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        } else if let Some(output) = self.work_details.outputs.borrow_mut().get_mut(id)
            && let Some(request) = output.request.take()
        {
            // A running command's stream ends with its row; a finished one keeps its output.
            if let Some(client) = self.client() {
                client.cancel(&request);
            }
            if output.running {
                self.work_details.outputs.borrow_mut().remove(id);
            }
        }
        cx.notify();
    }

    /// The row's icon (`ToolActivityIconView`): a provider's favicon or logo when it has one.
    fn work_icon(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let dark = cx.theme().mode.is_dark();
        let failed = failed(item);
        let muted = c.muted_foreground.opacity(0.7);
        let named = |name: &str, cx: &mut Context<Self>| -> AnyElement {
            let color = if failed {
                crate::web_theme::current(cx).error_foreground.opacity(0.4)
            } else {
                muted
            };
            match name {
                "browser" => crate::ui::svg_image(BROWSER_ICON, cx)
                    .size(px(16.))
                    .opacity(0.7)
                    .into_any_element(),
                "t3-code" => crate::ui::svg_image(T3_WORDMARK, cx)
                    .w(px(16.))
                    .h(px(10.))
                    .opacity(0.7)
                    .into_any_element(),
                name => crate::ui::icon(lucide(name))
                    .size(px(16.))
                    .text_color(color)
                    .into_any_element(),
            }
        };
        let fallback = icon_name(item);
        let image = activity_icon(item).and_then(|icon| {
            if icon["_tag"] == "native-app" {
                self.native_app_icon(&icon["app"], cx)
            } else {
                icon_url(icon, dark)
            }
        });
        let Some(url) = image else {
            return named(fallback, cx);
        };
        let fallback: SharedString = fallback.into();
        div()
            .size(px(16.))
            .rounded(px(2.))
            .overflow_hidden()
            .bg(c.background)
            .when(dark, |d| d.opacity(0.7))
            .child(
                img(SharedUri::from(url))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || {
                        crate::ui::icon(&fallback)
                            .size(px(16.))
                            .text_color(muted)
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// `threadReadLabel`: a T3 thread read names the thread it read, while that thread is listed.
    /// Returns the environment, thread id, title and the whole sentence.
    fn thread_read(&self, item: &TurnItem) -> Option<(String, String, String, String)> {
        let input = &item.raw["input"];
        let definition = crate::t3_tools::definition(item.str("toolName"), input)?;
        if item.kind != "dynamic_tool" || definition.summary_action != "thread-read" {
            return None;
        }
        let thread = input["threadId"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())?;
        let env = self.env()?;
        let title = env
            .shell
            .threads
            .iter()
            .find(|t| t.id == thread)
            .map(|t| t.title.trim().to_owned())
            .filter(|t| !t.is_empty())?;
        let status = if crate::t3_tools::result_failed(&item.raw["output"]) {
            "failed"
        } else {
            match item.str("status") {
                "completed" => "completed",
                "failed" => "failed",
                "declined" => "declined",
                "cancelled" | "interrupted" => "stopped",
                _ => "inProgress",
            }
        };
        let text = crate::i18n::tf(
            "{status, select, inProgress {Reading thread “{title}”} completed {Read thread “{title}”} failed {Failed to read thread “{title}”} declined {Declined to read thread “{title}”} stopped {Stopped reading thread “{title}”} other {Reading thread “{title}”}}",
            &[("status", status), ("title", &title)],
        );
        Some((env.key.clone(), thread.to_owned(), title, text))
    }

    /// A native app's icon, served by the host as an asset.
    fn native_app_icon(&self, app: &Value, cx: &mut Context<Self>) -> Option<String> {
        let env = self.env()?;
        let key = format!("{}\u{0}app\u{0}{app}", env.key);
        self.mint_asset_url(key, json!({ "_tag": "native-app-icon", "app": app }), cx)
    }

    /// One work row (web: `WorkEntryLogRow`): icon, label, hover time, failure mark and chevron;
    /// open, its detail follows. `single` rows label themselves as a group of one would.
    pub(crate) fn render_work_row(
        &self,
        item: &TurnItem,
        single: bool,
        grouped: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item.kind == "error" {
            return self.render_error_row(item, cx);
        }
        let child = (item.kind == "notification")
            .then(|| {
                let source = &item.raw["source"];
                matches!(source["kind"].as_str(), Some("subagent" | "delegated_task"))
                    .then(|| source["childThreadId"].as_str())
                    .flatten()
            })
            .flatten()
            .map(str::to_owned);
        // `SubagentNotificationLink`: a child's result shows as that subagent's own row.
        if let Some(child) = &child
            && let Some(subagent) = self
                .thread
                .items()
                .find(|i| i.kind == "subagent" && i.str("childThreadId") == child)
        {
            let subagent = subagent.clone();
            return self.render_item(&subagent, None, cx);
        }
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let expanded = self.expanded.contains(&item.id);
        let notice = item.kind == "system_notice";
        let warning = notice && item.raw["level"] != "info";
        // A notice opens only when its row cuts the message off.
        let cut_off = self.label_cut_off(&item.id);
        let can_expand = has_detail(item) && (!notice || cut_off.get());
        let reasoning = item.kind == "reasoning";
        let root = self.thread_cwd();
        let tool_like = matches!(
            item.kind.as_str(),
            "reasoning"
                | "command_execution"
                | "file_change"
                | "file_search"
                | "web_search"
                | "dynamic_tool"
                | "subagent"
                | "thread_created"
                | "user_input_request"
                | "approval_request"
        );
        let heading = if warning {
            tokens.warning_foreground
        } else if tool_like {
            tokens.secondary_label
        } else {
            c.foreground.opacity(0.8)
        };
        let answer = item.raw.get("questionAnswer").filter(|a| a.is_object());
        let answer_shown = answer
            .filter(|a| has_answer(a))
            .map(answer_preview)
            .filter(|p| !p.is_empty());
        let text = if reasoning && expanded {
            if running(item) {
                tc("Thinking", "status of an agent reasoning")
            } else {
                tc("Thought", "noun, a reasoning step")
            }
        } else {
            label(item, root.as_deref(), single)
        };
        let failed = failed(item);
        let has_image_icon = activity_icon(item).is_some();
        let group: SharedString = format!("work-row-{}", item.id).into();
        let id = item.id.clone();
        let time = crate::timeline::row_time(item);
        let line = h_flex()
            .id(SharedString::from(format!("work-{}", item.id)))
            .group(group.clone())
            .w_full()
            .min_h(px(24.))
            .gap(px(6.))
            .text_sm()
            .line_height(relative(1.625))
            .child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.work_icon(item, cx)),
            )
            .child(match self.thread_read(item) {
                // `ThreadReadLabel`: the thread's title links to it.
                Some((env, thread, title, text)) => {
                    let (before, after) = text
                        .split_once(title.as_str())
                        .map(|(a, b)| (a.to_owned(), b.to_owned()))
                        .unwrap_or((text.clone(), String::new()));
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .text_color(tokens.secondary_label)
                        .child(div().flex_none().child(before))
                        .child(
                            div()
                                .id(SharedString::from(format!("thread-link-{}", item.id)))
                                .min_w_0()
                                .truncate()
                                .text_color(c.foreground)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.select_thread(env.clone(), thread.clone(), cx)
                                }))
                                .child(title),
                        )
                        .child(div().flex_none().child(after))
                        .into_any_element()
                }
                None => h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        div()
                            .relative()
                            .when(answer_shown.is_none(), |d| d.flex_1())
                            .min_w_0()
                            .truncate()
                            .text_color(heading)
                            .when(warning, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(text.clone())
                            .when(notice, |d| {
                                d.child(measure_cut_off(text.clone(), cut_off.clone()))
                            }),
                    )
                    .children(answer_shown.clone().map(|preview| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(if expanded {
                                c.muted_foreground
                            } else {
                                c.foreground
                            })
                            .child(preview)
                    }))
                    .into_any_element(),
            })
            // `TimelineRowTimestamp`: the time takes room only while the row is hovered.
            .when(!notice && self.work_hovered(&item.id), |row| {
                row.child(
                    div()
                        .flex_none()
                        .mr_1()
                        .text_xs()
                        .text_color(c.muted_foreground)
                        .child(time),
                )
            })
            .on_hover(self.hover_listener(&item.id, cx))
            .when(item.kind == "thread_created", |row| {
                let target = item.str("targetThreadId").to_owned();
                let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
                row.child(
                    div()
                        .id(SharedString::from(format!("open-chat-{}", item.id)))
                        .flex_none()
                        .text_sm()
                        .text_color(c.primary)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_thread(env.clone(), target.clone(), cx)
                        }))
                        .child(t("Open chat")),
                )
            })
            .when_some(child.clone(), |row, child| {
                let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
                row.child(
                    div()
                        .id(SharedString::from(format!("open-subagent-{}", item.id)))
                        .flex_none()
                        .text_sm()
                        .text_color(c.primary)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_thread(env.clone(), child.clone(), cx)
                        }))
                        .child(t("Open subagent")),
                )
            })
            .when(failed && !has_image_icon, |row| {
                row.child(
                    crate::ui::icon("x")
                        .size(px(12.))
                        .text_color(tokens.error_foreground.opacity(0.4)),
                )
            })
            .when(can_expand, |row| {
                row.cursor_pointer()
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::ui::icon(if expanded {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                })
                                .size(px(12.))
                                .text_color(c.muted_foreground.opacity(0.7)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_work(&id, cx)))
            });
        v_flex()
            .w_full()
            .min_w_0()
            .rounded_md()
            .px(px(2.))
            .when(!grouped, |d| d.py(px(2.)))
            .child(line)
            .when(expanded && can_expand, |col| {
                col.child(self.render_work_detail(item, cx))
            })
            .into_any_element()
    }

    fn work_hovered(&self, key: &str) -> bool {
        self.work_details.hovered.borrow().as_deref() == Some(key)
    }

    /// Tracks the hovered work row, whose time then shows.
    fn hover_listener(
        &self,
        key: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let key = key.to_owned();
        cx.listener(move |this, hovered: &bool, _, cx| {
            let mut current = this.work_details.hovered.borrow_mut();
            if *hovered {
                *current = Some(key.clone());
            } else if current.as_deref() == Some(key.as_str()) {
                *current = None;
            } else {
                return;
            }
            drop(current);
            cx.notify();
        })
    }

    /// Whether a notice's row label was cut off, as last painted.
    fn label_cut_off(&self, id: &str) -> std::rc::Rc<std::cell::Cell<bool>> {
        self.work_details
            .cut_off
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .clone()
    }

    /// `QuestionAnswerHistory`: each question, its answer, and attached files.
    fn question_answer_history(&self, answer: &Value, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let texts = answer["questionTextById"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let answers = answer["answers"].as_object().cloned().unwrap_or_default();
        let files = answer["attachmentsByQuestionId"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut ids: Vec<String> = Vec::new();
        for id in texts.keys().chain(answers.keys()).chain(files.keys()) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        v_flex()
            .ml(px(28.))
            .mt_2()
            .gap_2()
            .text_sm()
            .text_color(c.muted_foreground)
            .children(ids.into_iter().map(|id| {
                let reply = answers.get(&id).map(answer_text).unwrap_or_default();
                let attached: Vec<String> = files
                    .get(&id)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f["name"].as_str().map(str::to_owned))
                    .collect();
                v_flex()
                    .gap_1()
                    .children(
                        texts
                            .get(&id)
                            .and_then(Value::as_str)
                            .map(|q| div().child(q.to_owned())),
                    )
                    .when(!reply.is_empty(), |col| {
                        col.child(div().ml_3().child(reply))
                    })
                    .when(!attached.is_empty(), |col| {
                        col.child(
                            h_flex().flex_wrap().gap_2().children(
                                attached
                                    .into_iter()
                                    .map(|name| div().underline().child(name)),
                            ),
                        )
                    })
            }))
            .into_any_element()
    }

    /// A failed provider error (web: the error branch of `WorkEntryLogRow`): a warning for a
    /// usage limit, otherwise the failure and its message, and Retry for a workspace that
    /// failed to prepare.
    fn render_error_row(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let failure = &item.raw["failure"];
        let warning = failure["class"] == "usage_limit";
        let color = if warning {
            tokens.warning_foreground
        } else {
            tokens.error_foreground
        };
        let label = if warning {
            match failure["resetAt"]
                .as_str()
                .and_then(|r| r.parse::<jiff::Timestamp>().ok())
            {
                Some(at) => {
                    let zone = jiff::tz::TimeZone::system();
                    let name = zone.iana_name().unwrap_or("").to_owned();
                    let time = format!("{} ({name})", crate::timeline::upcoming_time(at));
                    tv(
                        "Usage limit reached. Retry after {resetTime}.",
                        &[("resetTime", &time)],
                    )
                }
                None => t("Usage limit reached."),
            }
        } else if item.raw.get("retry").is_some_and(|r| r.is_object()) {
            let retry = &item.raw["retry"];
            let attempt = retry["attempt"].as_u64().unwrap_or(1);
            let progress = match retry["maxAttempts"].as_u64() {
                Some(max) => format!("{attempt}/{max}"),
                None => attempt.to_string(),
            };
            tp(
                "{attempt, plural, one {Provider error after {progress} retries} other {Provider error after {progress} retries}}",
                attempt as usize,
                &[("progress", &progress), ("attempt", &attempt.to_string())],
            )
        } else {
            let title = item.str("title").trim();
            if title.is_empty() {
                t("Provider error")
            } else {
                title.to_owned()
            }
        };
        let retry_run = (!warning
            && failure["code"] == "workspace_preparation_failed"
            && self
                .thread
                .workspace_preparation_failed(item.run_id.as_deref()))
        .then(|| item.run_id.clone())
        .flatten();
        let thread_id = item.str("threadId").to_owned();
        v_flex()
            .w_full()
            .rounded_md()
            .px(px(2.))
            .py(px(2.))
            .child(
                h_flex()
                    .items_start()
                    .min_h(px(24.))
                    .gap(px(6.))
                    .text_sm()
                    .line_height(relative(1.625))
                    .child(
                        div()
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::ui::icon("circle-alert")
                                    .size(px(16.))
                                    .text_color(color),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mr_1()
                            .text_xs()
                            .text_color(c.muted_foreground)
                            .child(crate::timeline::row_time(item)),
                    ),
            )
            .when(!warning, |col| {
                col.child(
                    div()
                        .ml(px(28.))
                        .py_1()
                        .text_sm()
                        .line_height(relative(1.625))
                        .text_color(c.foreground.opacity(0.8))
                        .child(failure["message"].as_str().unwrap_or("").to_owned()),
                )
            })
            .when_some(retry_run, |col, run| {
                col.child(
                    div().ml(px(28.)).pb_1().child(
                        Button::new(SharedString::from(format!("{}-retry", item.id)))
                            .xsmall()
                            .outline()
                            .icon(crate::ui::icon("rotate-ccw"))
                            .label(t("Retry"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dispatch(
                                    json!({ "type": "prepared-run.retry",
                                        "threadId": thread_id, "runId": run }),
                                    cx,
                                )
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    /// A run of work folded into one line (web: `WorkGroupHeader`); open, its rows follow.
    pub(crate) fn render_work_group(
        &self,
        id: String,
        items: &[&TurnItem],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let expanded = self.expanded.contains(&id);
        let group: SharedString = format!("work-group-{id}").into();
        let time = items
            .first()
            .map(|i| crate::timeline::row_time(i))
            .unwrap_or_default();
        let source_icon = items.iter().find_map(|i| activity_icon(i).map(|_| *i));
        let icon = match source_icon {
            Some(item) => self.work_icon(item, cx),
            None if items.iter().any(|i| surface(i).is_some()) => {
                crate::ui::svg_image(BROWSER_ICON, cx)
                    .opacity(0.7)
                    .into_any_element()
            }
            None if group_icon(items) == "t3-code" => crate::ui::svg_image(T3_WORDMARK, cx)
                .w(px(16.))
                .h(px(10.))
                .opacity(0.7)
                .into_any_element(),
            None => crate::ui::icon(lucide(group_icon(items)))
                .size(px(16.))
                .text_color(c.muted_foreground.opacity(0.7))
                .into_any_element(),
        };
        let header = h_flex()
            .id(SharedString::from(id.clone()))
            .group(group.clone())
            .w_full()
            .min_h(px(24.))
            .gap(px(6.))
            .px(px(2.))
            .py(px(2.))
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .line_height(relative(1.625))
            .child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(tokens.secondary_label)
                    .child(summarize(items)),
            )
            .when(self.work_hovered(&id), |row| {
                row.child(
                    div()
                        .flex_none()
                        .mr_1()
                        .text_xs()
                        .text_color(c.muted_foreground)
                        .child(time),
                )
            })
            .on_hover(self.hover_listener(&id, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&id) {
                    this.expanded.insert(id.clone());
                }
                cx.notify();
            }));
        if !expanded {
            return header.into_any_element();
        }
        let visible: Vec<&&TurnItem> = items
            .iter()
            .filter(|i| i.kind != "reasoning" || !i.str("text").trim().is_empty())
            .collect();
        // An open group of thoughts reads as one trace (web: `ReasoningTraceContent`).
        if visible.iter().all(|i| i.kind == "reasoning") {
            let text = visible
                .iter()
                .map(|i| i.str("text"))
                .collect::<Vec<_>>()
                .join("\n\n");
            return v_flex()
                .child(header)
                .child(self.reasoning_trace(&format!("{}-trace", items[0].id), &text, cx))
                .into_any_element();
        }
        // The web caps compact rows at 18rem and grows the cap by what open rows add; an open
        // row here lifts the cap instead of measuring it.
        let capped = !visible.iter().any(|i| self.expanded.contains(&i.id));
        let rows_id = SharedString::from(format!("{}-rows", items[0].id));
        v_flex()
            .child(header)
            .child(
                div()
                    .id(rows_id)
                    .when(capped, |d| d.max_h(px(288.)).overflow_y_scroll())
                    .children(
                        visible
                            .into_iter()
                            .map(|item| self.render_work_row(item, false, true, cx)),
                    ),
            )
            .into_any_element()
    }

    /// A thought's full text (`ReasoningTraceContent`): Markdown, indented under the icon.
    fn reasoning_trace(&self, id: &str, text: &str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(SharedString::from(format!("trace-{id}")))
            .ml(px(28.))
            .px(px(2.))
            .py_1()
            .max_h(px(384.))
            .overflow_y_scroll()
            .text_color(cx.theme().colors.foreground)
            .child(
                TextView::markdown(
                    SharedString::from(format!("trace-md-{id}")),
                    self.resolve_images(text, cx).into_owned(),
                )
                .style(crate::ui::markdown_style(cx))
                .selectable(true),
            )
            .into_any_element()
    }

    /// Highlighted code with no code-block chrome (web: `SyntaxTokens`, `ShellCommandBlock`).
    fn code_text(&self, id: String, lang: &str, code: &str, cx: &App) -> AnyElement {
        TextView::markdown(SharedString::from(id), fenced(lang, code))
            .style(
                crate::ui::markdown_style(cx).code_block(
                    StyleRefinement::default()
                        .p_0()
                        .m_0()
                        .border_0()
                        .rounded_none()
                        .bg(hsla(0., 0., 0., 0.))
                        .text_xs(),
                ),
            )
            .code_block_actions(|_, _, _| div())
            .selectable(true)
            .into_any_element()
    }

    /// `StructuredValue`: mono text, JSON highlighted, scrolling past 320px.
    fn structured_value(&self, id: String, text: &str, json: bool, cx: &App) -> AnyElement {
        let c = cx.theme().colors;
        div()
            .id(SharedString::from(format!("{id}-scroll")))
            .max_h(px(320.))
            .overflow_y_scroll()
            .font_family(mono(cx))
            .text_xs()
            .line_height(relative(1.625))
            .text_color(c.muted_foreground)
            .child(if json && is_json(text) {
                self.code_text(id, "json", text, cx)
            } else {
                div().child(text.to_owned()).into_any_element()
            })
            .into_any_element()
    }

    /// The opened row's detail (`V2ItemInspector`, `QuestionAnswerHistory` aside).
    fn render_work_detail(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        if item.kind == "reasoning" {
            return self.reasoning_trace(&item.id, item.str("text"), cx);
        }
        if let Some(answer) = item.raw.get("questionAnswer").filter(|a| a.is_object()) {
            return self.question_answer_history(answer, cx);
        }
        // An image a tool viewed shows itself (web: `WorkLogDetails kind="media"`).
        let viewed = viewed_image_path(item).map(|path| self.viewed_image(path, 256., cx));
        if let Some(text) = match item.kind.as_str() {
            "notification" => Some(item.str("detail")),
            "system_notice" => Some(item.str("message")),
            "user_input_request" => None,
            _ => None,
        } {
            return div()
                .mt(px(2.))
                .mb(px(6.))
                .pb_2()
                .child(self.structured_value(format!("{}-text", item.id), text, false, cx))
                .into_any_element();
        }
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let id = item.id.clone();
        let fetched = self.fetched_item(item, cx);
        let (full, fetch_state) = match &fetched {
            Some(Fetched::Item(v)) if v["type"] == item.raw["type"] => (v, None),
            Some(other) => (&item.raw, Some(other.clone())),
            None => (&item.raw, None),
        };
        let root = self.thread_cwd();
        let mono = mono(cx);
        let muted = c.muted_foreground;
        let output_state = |output: Option<String>, cx: &App| -> Option<AnyElement> {
            if let Some(output) = output {
                return Some(self.structured_value(format!("{id}-out"), &output, true, cx));
            }
            let note = |text: String, color: Hsla| {
                div()
                    .text_color(color)
                    .when(color == muted, |d| d.italic())
                    .child(text)
                    .into_any_element()
            };
            match &fetch_state {
                Some(Fetched::Pending) => Some(note(t("Loading output…"), muted)),
                Some(Fetched::Failed(error)) => Some(note(
                    tv("Couldn't load output: {error}", &[("error", error)]),
                    tokens.error_foreground,
                )),
                None if needs_fetch(item) => Some(note(t("No output."), muted)),
                _ => None,
            }
        };
        let body = v_flex()
            .gap_2()
            .text_xs()
            .font_family(mono.clone())
            .line_height(relative(1.625));
        let body = match item.kind.as_str() {
            "todo_list" => body.child(
                self.structured_value(
                    format!("{id}-steps"),
                    &full["steps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|s| {
                            let mark = match s["status"].as_str() {
                                Some("completed") => "✓",
                                Some("running") => "◉",
                                _ => "○",
                            };
                            format!("{mark} {}", s["text"].as_str().unwrap_or(""))
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    false,
                    cx,
                ),
            ),
            "command_execution" => {
                let command = visible_controls(full["input"].as_str().unwrap_or("").trim());
                let exit = full["exitCode"].as_i64().filter(|&c| c != 0);
                let output = if self.config().command_output_streaming {
                    self.command_output(item, cx)
                } else {
                    output_state(output_text(full), cx)
                };
                body.when(!command.is_empty(), |b| {
                    b.child(
                        div()
                            .text_color(c.foreground.opacity(0.85))
                            .child(self.code_text(
                                format!("{id}-cmd"),
                                crate::command_label::highlight_language(&command),
                                &command,
                                cx,
                            )),
                    )
                })
                .children(output)
                .when_some(exit, |b, code| {
                    b.child(
                        div()
                            .text_color(tokens.error_foreground)
                            .child(tv("exit {code}", &[("code", &code.to_string())])),
                    )
                })
            }
            "file_change" => {
                let path = relative_path(full["fileName"].as_str().unwrap_or(""), root.as_deref());
                let counts = (full.get("additions").is_some() || full.get("deletions").is_some())
                    .then(|| {
                        (
                            full["additions"].as_u64().unwrap_or(0),
                            full["deletions"].as_u64().unwrap_or(0),
                        )
                    });
                let diff_item = item.clone();
                body.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(div().text_color(muted).child(path))
                        .when_some(counts, |row, (a, d)| {
                            row.child(
                                h_flex()
                                    .gap_1()
                                    .font_family(cx.theme().font_family.clone())
                                    .child(
                                        div()
                                            .text_color(tokens.success_foreground)
                                            .child(format!("+{a}")),
                                    )
                                    .child(
                                        div()
                                            .text_color(tokens.error_foreground)
                                            .child(format!("-{d}")),
                                    ),
                            )
                        })
                        .child(
                            div().font_family(cx.theme().font_family.clone()).child(
                                Button::new(SharedString::from(format!("{id}-open-diff")))
                                    .xsmall()
                                    .outline()
                                    .label(t("Open diff"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_tool_diff(&diff_item, cx)
                                    })),
                            ),
                        ),
                )
                .when(
                    item.str("status") == "failed"
                        && full["diffStr"]
                            .as_str()
                            .is_some_and(|d| !d.trim().is_empty()),
                    |b| {
                        b.child(self.structured_value(
                            format!("{id}-diff"),
                            full["diffStr"].as_str().unwrap_or(""),
                            false,
                            cx,
                        ))
                    },
                )
                .children(
                    full["changes"]
                        .as_array()
                        .filter(|c| !c.is_empty())
                        .map(|changes| {
                            v_flex()
                                .gap_1()
                                .text_color(muted)
                                .children(changes.iter().map(|change| {
                                    let old = change["oldPath"]
                                        .as_str()
                                        .map(|o| format!("{o} → "))
                                        .unwrap_or_default();
                                    let kinds: Vec<&str> =
                                        [&change["fileType"], &change["mimeType"]]
                                            .into_iter()
                                            .filter_map(Value::as_str)
                                            .filter(|s| !s.is_empty())
                                            .collect();
                                    let kinds = if kinds.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" ({})", kinds.join(", "))
                                    };
                                    div().child(format!(
                                        "{} {old}{}{kinds}",
                                        change["operation"].as_str().unwrap_or(""),
                                        relative_path(
                                            change["path"].as_str().unwrap_or(""),
                                            root.as_deref()
                                        )
                                    ))
                                }))
                        }),
                )
            }
            "file_search" => body
                .children(
                    full["pattern"]
                        .as_str()
                        .filter(|p| !p.trim().is_empty())
                        .map(|p| {
                            div()
                                .text_color(c.foreground.opacity(0.85))
                                .child(p.to_owned())
                        }),
                )
                .children(
                    full["results"]
                        .as_array()
                        .filter(|r| !r.is_empty())
                        .map(|results| {
                            v_flex().gap_1().children(results.iter().map(|r| {
                                let place = format!(
                                    "{}{}{}",
                                    relative_path(
                                        r["fileName"].as_str().unwrap_or(""),
                                        root.as_deref()
                                    ),
                                    r["line"]
                                        .as_u64()
                                        .map(|l| format!(":{l}"))
                                        .unwrap_or_default(),
                                    r["column"]
                                        .as_u64()
                                        .map(|l| format!(":{l}"))
                                        .unwrap_or_default()
                                );
                                v_flex()
                                    .child(div().text_color(c.foreground.opacity(0.8)).child(place))
                                    .children(r["preview"].as_str().filter(|p| !p.is_empty()).map(
                                        |p| {
                                            div()
                                                .mt(px(2.))
                                                .font_family(cx.theme().font_family.clone())
                                                .text_color(muted)
                                                .child(p.to_owned())
                                        },
                                    ))
                            }))
                        }),
                ),
            "web_search" => {
                let patterns: Vec<&str> = full["patterns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                body.when(!patterns.is_empty(), |b| {
                    b.child(
                        div()
                            .text_color(c.foreground.opacity(0.85))
                            .child(patterns.join("\n")),
                    )
                })
                .children(
                    full["results"]
                        .as_array()
                        .filter(|r| !r.is_empty())
                        .map(|results| {
                            v_flex()
                                .gap(px(6.))
                                .font_family(cx.theme().font_family.clone())
                                .children(results.iter().enumerate().map(|(i, r)| {
                                    let url = r["url"].as_str().and_then(|u| web_url(u, false));
                                    let title = r["title"]
                                        .as_str()
                                        .or(r["url"].as_str())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| t("Search result"));
                                    let heading = h_flex()
                                        .id(SharedString::from(format!("{id}-result-{i}")))
                                        .gap_1()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(c.foreground)
                                        .child(title);
                                    let heading = match url {
                                        Some(url) => heading
                                            .cursor_pointer()
                                            .hover(|s| s.underline())
                                            .child(crate::ui::icon("external-link").size(px(12.)))
                                            .on_click(move |_, _, cx| cx.open_url(&url))
                                            .into_any_element(),
                                        None => heading.into_any_element(),
                                    };
                                    v_flex().child(heading).children(
                                        r["snippet"]
                                            .as_str()
                                            .filter(|s| !s.is_empty())
                                            .map(|s| div().text_color(muted).child(s.to_owned())),
                                    )
                                }))
                        }),
                )
            }
            "dynamic_tool" => {
                let read = action(item) == "read";
                let skill_args = (item.str("toolName") == "Skill")
                    .then(|| full["input"]["args"].as_str().map(str::to_owned))
                    .flatten();
                let images = self.tool_output_images(full, cx);
                let body = body.children(images);
                if read || item.str("toolName") == "Skill" {
                    // Reads show the files they read, skills their arguments, then any output.
                    let plain = if read {
                        let paths = tool_paths(&json!({ "input": full["input"] }));
                        (!paths.is_empty()).then(|| paths.join("\n"))
                    } else {
                        skill_args
                    };
                    body.children(plain.map(|p| div().text_color(muted).child(p)))
                        .children(output_state(output_text(full), cx))
                } else {
                    let (args, args_text) = call_lines(&full["input"]);
                    let name = item.str("toolName").to_owned();
                    body.children(args.map(|args| {
                        v_flex().text_color(c.foreground.opacity(0.85)).children(
                            args.into_iter().map(|(key, value)| {
                                let lang = if key == "html" {
                                    Some("html")
                                } else if key == "code"
                                    && matches!(
                                        name.as_str(),
                                        "js" | "cua_repl.js" | "node_repl.js"
                                    )
                                {
                                    Some("typescript")
                                } else if is_json(&value) {
                                    Some("json")
                                } else {
                                    None
                                };
                                let head = div().text_color(muted).child(format!("{key} "));
                                match lang {
                                    Some(lang) => v_flex().child(head).child(self.code_text(
                                        format!("{id}-arg-{key}"),
                                        lang,
                                        &value,
                                        cx,
                                    )),
                                    None => v_flex().child(div().child(
                                        StyledText::new(format!("{key} {value}")).with_highlights(
                                            [(
                                                0..key.len(),
                                                HighlightStyle {
                                                    color: Some(muted),
                                                    ..Default::default()
                                                },
                                            )],
                                        ),
                                    )),
                                }
                            }),
                        )
                    }))
                    .children(
                        args_text.map(|text| {
                            self.structured_value(format!("{id}-args"), &text, true, cx)
                        }),
                    )
                    .children(output_state(output_text(full), cx))
                }
            }
            _ => body,
        };
        div()
            .id(SharedString::from(format!("work-detail-{}", item.id)))
            .mt(px(2.))
            .mb(px(6.))
            .pb_2()
            .cursor_default()
            .on_click(|_, _, cx| cx.stop_propagation())
            .children(viewed.map(|image| div().mt_1().mb_2().child(image)))
            .child(body)
            .into_any_element()
    }

    /// An image a tool opened (`ChatMarkdownAssetImage`), up to `max_height` tall; it opens full
    /// size on click.
    pub(crate) fn viewed_image(
        &self,
        path: &str,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(url) = self.host_image_url(path, cx) else {
            return div().into_any_element();
        };
        let source = ImageSource::from(SharedUri::from(url));
        let open = source.clone();
        div()
            .id(SharedString::from(format!("viewed-{path}")))
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::open_expanded(open.clone(), window, cx)
            })
            .child(
                img(source)
                    .max_h(px(max_height))
                    .max_w_full()
                    .rounded_md()
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element()
    }

    /// `ViewedImagesTimelineRow`: the images a group opened, as thumbnails under it.
    pub(crate) fn render_viewed_images(
        &self,
        paths: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .flex_wrap()
            .items_start()
            .gap_2()
            .px(px(2.))
            .children(paths.iter().map(|path| self.viewed_image(path, 160., cx)))
            .into_any_element()
    }

    /// Images a tool returned (`ToolOutputImages`), each minted as a `tool-output-image` asset.
    fn tool_output_images(&self, item: &Value, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let count = output_image_count(item);
        let (Some(env), Some(thread), Some(item_id)) =
            (self.env(), item["threadId"].as_str(), item["id"].as_str())
        else {
            return Vec::new();
        };
        let env_key = env.key.clone();
        (0..count)
            .filter_map(|index| {
                let key = format!("{env_key}\u{0}tool-image\u{0}{item_id}\u{0}{index}");
                let resource = json!({ "_tag": "tool-output-image", "threadId": thread,
                    "itemId": item_id, "index": index });
                let url = self.mint_asset_url(key, resource, cx)?;
                Some(
                    img(SharedUri::from(url))
                        .max_h(px(256.))
                        .max_w_full()
                        .rounded_md()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The full item behind a row whose wire copy withheld output, fetched once per revision.
    fn fetched_item(&self, item: &TurnItem, cx: &mut Context<Self>) -> Option<Fetched> {
        if !needs_fetch(item) {
            return None;
        }
        let key = (item.id.clone(), detail_revision(item));
        if let Some(known) = self.work_details.fetched.borrow().get(&key) {
            return Some(known.clone());
        }
        let client = self.client()?;
        self.work_details
            .fetched
            .borrow_mut()
            .insert(key.clone(), Fetched::Pending);
        let input =
            json!({ "threadId": item.raw["threadId"], "itemId": item.id, "revision": key.1 });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("orchestration.getTurnItem", input).await })
                    .await;
            this.update(cx, |app, cx| {
                let fetched = match result.map_err(anyhow::Error::from).and_then(|r| r) {
                    Ok(value) if value["item"].is_object() => Fetched::Item(value["item"].clone()),
                    Ok(_) => Fetched::Failed(t("Output is no longer available.")),
                    Err(e) => Fetched::Failed(e.to_string()),
                };
                app.work_details.fetched.borrow_mut().insert(key, fetched);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Some(Fetched::Pending)
    }

    /// An open command's output (`CommandOutputPanel`): streams while it runs, then stays.
    fn command_output(&self, item: &TurnItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let started = self.work_details.outputs.borrow().contains_key(&item.id);
        if !started && let Some(client) = self.client() {
            let (request, mut rx) = client.request(
                "orchestration.subscribeCommandOutput",
                json!({ "threadId": item.raw["threadId"], "itemId": item.id }),
            );
            self.work_details.outputs.borrow_mut().insert(
                item.id.clone(),
                CommandOutput {
                    request: Some(request),
                    running: true,
                    ..Default::default()
                },
            );
            let id = item.id.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    let frame = rx.recv().await;
                    let done = this
                        .update(cx, |app, cx| {
                            let mut outputs = app.work_details.outputs.borrow_mut();
                            let Some(output) = outputs.get_mut(&id) else {
                                return true;
                            };
                            match &frame {
                                Some(Ok(value)) => output.apply(value),
                                Some(Err(e)) => {
                                    output.error = Some(e.to_string());
                                    output.loaded = true;
                                }
                                None => {}
                            }
                            let finished = frame.is_none() || !output.running;
                            if finished {
                                output.request = None;
                                output.loaded = true;
                            }
                            cx.notify();
                            finished
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
            })
            .detach();
        }
        let outputs = self.work_details.outputs.borrow();
        let output = outputs.get(&item.id)?;
        if output.text().is_empty() {
            return match (&output.error, output.loaded, output.running) {
                (Some(error), _, _) => Some(
                    div()
                        .text_color(tokens.error_foreground)
                        .child(tv("Couldn't load output: {error}", &[("error", error)]))
                        .into_any_element(),
                ),
                (None, false, _) => Some(
                    div()
                        .italic()
                        .text_color(c.muted_foreground)
                        .child(t("Loading output…"))
                        .into_any_element(),
                ),
                (None, true, true) => None,
                (None, true, false) => Some(
                    div()
                        .italic()
                        .text_color(c.muted_foreground)
                        .child(t("No output."))
                        .into_any_element(),
                ),
            };
        }
        let (plain, styles) = terminal_spans(output.text(), cx.theme().mode.is_dark());
        let scroll = self
            .work_scrolls
            .borrow_mut()
            .entry(item.id.clone())
            .or_default()
            .clone();
        if output.running {
            // Follows the tail while it streams (web: `FOLLOW_SLACK_PX`).
            if scroll.max_offset().y + scroll.offset().y <= px(8.) {
                scroll.scroll_to_bottom();
            }
        }
        Some(
            div()
                .id(SharedString::from(format!("{}-output", item.id)))
                .max_h(px(320.))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .text_color(c.muted_foreground)
                .when(output.output.truncated, |d| {
                    d.child(div().italic().child(t("Earlier output not shown")))
                })
                .child(StyledText::new(plain).with_highlights(styles))
                .into_any_element(),
        )
    }

    /// Opens an edit's own diff in the diff panel (web: `onOpenToolDiff`).
    pub(crate) fn open_tool_diff(&mut self, item: &TurnItem, cx: &mut Context<Self>) {
        let diff = item.raw["diffStr"].as_str().map(str::to_owned);
        let label = item
            .str("fileName")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned();
        let fetch = (diff.is_none() || needs_fetch(item))
            .then(|| self.client())
            .flatten();
        let input = json!({ "threadId": item.raw["threadId"], "itemId": item.id,
            "revision": detail_revision(item) });
        self.show_tool_diff(label, diff, fetch.map(|client| (client, input)), cx);
    }
}

/// Paints nothing; records whether `text` overflows the label it sits in (a notice opens only
/// when its row cut it off) and repaints when that changes.
fn measure_cut_off(text: String, cut: std::rc::Rc<std::cell::Cell<bool>>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let font = window.text_style().font();
            let shaped = window.text_system().shape_line(
                SharedString::from(text.clone()),
                px(14.),
                &[TextRun {
                    len: text.len(),
                    font,
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let over = shaped.width() > bounds.size.width + px(0.5);
            if cut.replace(over) != over {
                window.refresh();
            }
        },
    )
    .absolute()
    .inset_0()
}

/// The web's live sweep: 2.2s across, in 30 steps (`live-tool-shine`, `live-activity-focus`).
const SWEEP_MS: u64 = 2_200;
const SWEEP_STEPS: u64 = 30;
/// `--live-activity-focus-width`, 4.5rem.
const SWEEP_BAND: f32 = 72.;
/// The label's offset in a row: the 24px icon slot and its 6px gap.
const LABEL_LEAD: f32 = 30.;

#[derive(Clone, Copy)]
enum Sweep {
    /// `live-tool-shine`: a band of foreground across the label text.
    Shine,
    /// `ActivityShimmerOverlay`: the highlighted row revealed through a moving soft mask.
    Focus,
}

impl Sweep {
    /// Where the band starts, in row coordinates, for content `width` wide.
    fn start(self, phase: f32, width: f32) -> f32 {
        match self {
            // The gradient image moves from just left of the label to its right edge.
            Sweep::Shine => LABEL_LEAD - SWEEP_BAND + phase * (width - LABEL_LEAD + SWEEP_BAND),
            // The mask spans the row plus a band on each side and slides its own width.
            Sweep::Focus => -SWEEP_BAND + phase * (width + 2. * SWEEP_BAND),
        }
    }

    /// How bright a point `offset` into the band is.
    fn alpha(self, offset: f32) -> f32 {
        if !(0.0..=SWEEP_BAND).contains(&offset) {
            return 0.;
        }
        match self {
            Sweep::Shine => 1. - (offset - SWEEP_BAND / 2.).abs() / (SWEEP_BAND / 2.),
            Sweep::Focus => {
                const STOPS: [(f32, f32); 7] = [
                    (0., 0.),
                    (10.8, 0.12),
                    (25.2, 0.55),
                    (36., 1.),
                    (46.8, 0.55),
                    (61.2, 0.12),
                    (72., 0.),
                ];
                STOPS.windows(2).find(|w| offset <= w[1].0).map_or(0., |w| {
                    let t = (offset - w[0].0) / (w[1].0 - w[0].0);
                    w[0].1 + (w[1].1 - w[0].1) * t
                })
            }
        }
    }
}

/// `liveThoughtLine`: a thought's bold heading, else its first sentence, as plain text.
pub(crate) fn live_thought_line(markdown: &str) -> String {
    let trimmed = markdown.trim_start();
    let heading = trimmed.strip_prefix("**").and_then(|rest| {
        let end = rest.find("**")?;
        let after = rest[end + 2..].trim_start_matches([' ', '\t']);
        (!rest[..end].contains(['*', '\n', '\r'])
            && (after.is_empty() || after.starts_with(['\n', '\r'])))
        .then(|| rest[..end].to_owned())
    });
    let text = thought_preview(heading.as_deref().unwrap_or(markdown));
    if heading.is_some() {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let closer = |k: usize| chars.get(k).is_some_and(|c| "\"'”’)".contains(*c));
        let cjk_closer = |k: usize| chars.get(k).is_some_and(|c| "\"'”’）」』".contains(*c));
        if matches!(c, '.' | '?' | '!') {
            let end = if closer(i + 1) { i + 2 } else { i + 1 };
            if chars.get(end).is_some_and(|c| c.is_whitespace()) {
                return chars[..end].iter().collect();
            }
        }
        if matches!(c, '。' | '！' | '？') {
            let end = if cjk_closer(i + 1) { i + 2 } else { i + 1 };
            return chars[..end].iter().collect();
        }
    }
    text
}

/// `liveActivityToolStatus`.
fn live_status(item: &TurnItem, present: bool) -> &'static str {
    match item.str("status") {
        "failed" => "failed",
        "declined" => "declined",
        "cancelled" | "interrupted" => "stopped",
        "idle" => "idle",
        "pending" | "running" | "waiting" => "inProgress",
        _ if present => "inProgress",
        _ => "completed",
    }
}

/// `liveWorkEntryLabel`: a running call in the present tense, by the program it runs.
pub(crate) fn live_label(item: &TurnItem, root: Option<&str>, active: bool) -> String {
    let status = live_status(item, active);
    if item.kind == "reasoning" {
        let thought = thought_preview(item.str("text"));
        return if !thought.is_empty() {
            thought
        } else if status == "inProgress" {
            t("Thinking")
        } else {
            t("Thought")
        };
    }
    if item.kind == "dynamic_tool"
        && let Some(t3) = crate::t3_tools::presentation(
            item.str("toolName"),
            &item.raw["input"],
            &item.raw["output"],
            status,
        )
    {
        return t3.display_name;
    }
    let command = item.str("input").trim();
    if item.kind == "command_execution" && !command.is_empty() {
        return match crate::command_label::program_name(command) {
            None => generic_command_label(status),
            Some(program) => tv(
                match status {
                    "inProgress" => "Running {program}",
                    "failed" => "Failed {program}",
                    "declined" => "Declined {program}",
                    "stopped" => "Stopped {program}",
                    _ => "Ran {program}",
                },
                &[("program", &program)],
            ),
        };
    }
    if item.kind == "command_execution" {
        let title = item.str("title").trim();
        let title = title
            .trim_end_matches(" completed")
            .trim_end_matches(" complete");
        if matches!(title, "" | "Ran command" | "Run command" | "Command") {
            return generic_command_label(status);
        }
    }
    label(item, root, false)
}

/// `genericCommandStatusLabel`.
fn generic_command_label(status: &str) -> String {
    t(match status {
        "inProgress" => "Running command",
        "failed" => "Failed command",
        "declined" => "Declined command",
        "stopped" => "Stopped command",
        _ => "Ran command",
    })
}

/// `PROVIDER_DISPLAY_NAMES`.
fn provider_display_name(driver: &str) -> Option<&'static str> {
    Some(match driver {
        "antigravity" => "Antigravity",
        "codex" => "Codex",
        "claudeAgent" => "Claude",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "fx" => "fx",
        "kiro" => "Kiro",
        "muse" => "Muse Code",
        "acpRegistry" => "ACP Registry",
        "pi" => "Pi",
        "opencode" => "OpenCode",
        _ => return None,
    })
}

/// `presentProviderStarting`: the step a launching provider reports.
fn provider_starting_label(driver: Option<&str>, phase: Option<&str>) -> String {
    let Some(provider) = driver.and_then(provider_display_name) else {
        return tc("Starting", "a provider is launching");
    };
    tv(
        match phase {
            Some("plugins") => "Loading {provider} plugins",
            Some("mcp") => "Starting {provider} MCP servers",
            Some("hooks") => "Loading {provider} hooks",
            Some("skills") => "Loading {provider} rules and skills",
            Some("resume") => "Resuming {provider} session",
            Some("connecting") => "Connecting to {provider}",
            _ => "Starting {provider}",
        },
        &[("provider", provider)],
    )
}

/// A label whose characters brighten as the sweep passes, cut with an ellipsis like `truncate`.
/// It reports the row width it painted so the row's icon can follow the same band.
fn sweep_text(
    text: String,
    base: Hsla,
    bright: Hsla,
    phase: f32,
    sweep: Sweep,
    painted: std::rc::Rc<std::cell::Cell<f32>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let font = window.text_style().font();
            let size = px(14.);
            let run = |len: usize, color: Hsla| TextRun {
                len,
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shape = |text: &str, runs: &[TextRun], window: &mut Window| {
                window.text_system().shape_line(
                    SharedString::from(text.to_owned()),
                    size,
                    runs,
                    None,
                )
            };
            let mut text = text.clone();
            let mut shaped = shape(&text, &[run(text.len(), base)], window);
            if shaped.width() > bounds.size.width {
                let ellipsis = shape("…", &[run("…".len(), base)], window).width();
                let cut = shaped
                    .index_for_x(bounds.size.width - ellipsis)
                    .unwrap_or(0)
                    .min(text.len());
                let cut = (0..=cut)
                    .rev()
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(0);
                text = format!("{}…", text[..cut].trim_end());
                shaped = shape(&text, &[run(text.len(), base)], window);
            }
            let width = f32::from(shaped.width());
            painted.set(LABEL_LEAD + width);
            let start = sweep.start(phase, LABEL_LEAD + width);
            let mut runs: Vec<TextRun> = Vec::new();
            for (i, c) in text.char_indices() {
                let (x0, x1) = (
                    f32::from(shaped.x_for_index(i)),
                    f32::from(shaped.x_for_index(i + c.len_utf8())),
                );
                let alpha = sweep.alpha(LABEL_LEAD + (x0 + x1) / 2. - start);
                let color = base.blend(bright.opacity(alpha));
                match runs.last_mut() {
                    Some(last) if last.color == color => last.len += c.len_utf8(),
                    _ => runs.push(run(c.len_utf8(), color)),
                }
            }
            let shaped = shape(&text, &runs, window);
            shaped
                .paint(
                    bounds.origin,
                    bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        },
    )
    .flex_1()
    .min_w_0()
    .h(px(22.75))
}

impl T3App {
    /// Where the live sweep is (0..1), stepped like the web's; while a live row is drawn, the
    /// next step's repaint is scheduled. Rows out of view aren't drawn, so nothing ticks.
    fn sweep_phase(&self, cx: &mut Context<Self>) -> f32 {
        static START: std::sync::LazyLock<std::time::Instant> =
            std::sync::LazyLock::new(std::time::Instant::now);
        let elapsed = START.elapsed().as_millis() as u64 % SWEEP_MS;
        let step = elapsed * SWEEP_STEPS / SWEEP_MS;
        if !self.work_details.sweep_tick.replace(true) {
            let wait = (step + 1) * SWEEP_MS / SWEEP_STEPS - elapsed;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(wait.max(1)))
                    .await;
                this.update(cx, |app, cx| {
                    app.work_details.sweep_tick.set(false);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        step as f32 / SWEEP_STEPS as f32
    }

    fn sweep_width(&self, key: &str) -> std::rc::Rc<std::cell::Cell<f32>> {
        self.work_details
            .sweep_widths
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// `LiveActivityRow`: icon and label, swept while `active` (`shine` brightens the label,
    /// otherwise the whole row is revealed through the moving mask).
    #[allow(clippy::too_many_arguments)]
    fn live_activity_row(
        &self,
        key: &str,
        icon: AnyElement,
        label: AnyElement,
        text: String,
        failed: bool,
        sweep: Option<Sweep>,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let label = match sweep.filter(|_| !failed) {
            Some(sweep) => {
                let phase = self.sweep_phase(cx);
                sweep_text(
                    text,
                    tokens.secondary_label,
                    c.foreground,
                    phase,
                    sweep,
                    self.sweep_width(key),
                )
                .into_any_element()
            }
            None => label,
        };
        h_flex()
            .min_h(px(24.))
            .max_w_full()
            .gap(px(6.))
            .text_sm()
            .line_height(relative(1.625))
            .child(
                div()
                    .size(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(label)
            .when(failed, |row| {
                row.child(
                    crate::ui::icon("x")
                        .size(px(12.))
                        .text_color(tokens.error_foreground.opacity(0.4)),
                )
            })
    }

    /// The live row's icon color: highlighted where the focus band passes over it.
    fn swept_icon_color(&self, key: &str, sweep: Option<Sweep>, cx: &mut Context<Self>) -> Hsla {
        let c = cx.theme().colors;
        let muted = c.muted_foreground.opacity(0.7);
        match sweep {
            Some(Sweep::Focus) => {
                let phase = self.sweep_phase(cx);
                let width = self.sweep_width(key).get().max(LABEL_LEAD);
                let alpha = Sweep::Focus.alpha(12. - Sweep::Focus.start(phase, width));
                muted.blend(c.foreground.opacity(alpha))
            }
            _ => muted,
        }
    }

    /// The running turn's latest call (web: `LiveWorkEntryTimelineRow`); the newest thought shows
    /// above it, and clicking opens the turn's work.
    pub(crate) fn render_live(
        &self,
        group: &str,
        entry: &TurnItem,
        thought: Option<&TurnItem>,
        items: &[&TurnItem],
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = crate::web_theme::current(cx);
        let expanded = self.expanded.contains(group);
        let root = self.thread_cwd();
        let thought_line = thought
            .filter(|_| !expanded)
            .map(|t| {
                let text = t.str("text");
                let cut = text
                    .char_indices()
                    .nth(8_192)
                    .map_or(text.len(), |(i, _)| i);
                live_thought_line(&text[..cut])
            })
            .unwrap_or_default();
        let thought_is_status =
            !thought_line.is_empty() && thought.is_some_and(|t| t.id == entry.id);
        let failed = failed(entry);
        let text = if thought_is_status {
            if active { t("Thinking") } else { t("Thought") }
        } else if let Some((_, _, _, sentence)) = self.thread_read(entry) {
            sentence
        } else {
            live_label(entry, root.as_deref(), active)
        };
        let key = format!("live-{group}");
        let sweep = active.then_some(Sweep::Shine);
        let icon = self.work_icon(entry, cx);
        let label = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(tokens.secondary_label)
            .child(text.clone())
            .into_any_element();
        let group_id = group.to_owned();
        let mut col = v_flex()
            .id(SharedString::from(format!("live-row-{group}")))
            .w_full()
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&group_id) {
                    this.expanded.insert(group_id.clone());
                }
                cx.notify();
            }));
        if !thought_line.is_empty() {
            col = col.child(
                h_flex()
                    .items_start()
                    .gap(px(6.))
                    .text_sm()
                    .line_height(relative(1.625))
                    .child(
                        div()
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::ui::icon("brain")
                                    .size(px(16.))
                                    .text_color(cx.theme().colors.muted_foreground.opacity(0.7)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .line_clamp(4)
                            .text_color(tokens.secondary_label)
                            .child(thought_line),
                    ),
            );
        }
        col = col.child(self.live_activity_row(&key, icon, label, text, failed, sweep, cx));
        if expanded {
            col = col.child(self.render_group_rows(items, cx));
        }
        col.into_any_element()
    }

    /// The rows of an opened work group, or one trace when they are all thoughts.
    fn render_group_rows(&self, items: &[&TurnItem], cx: &mut Context<Self>) -> AnyElement {
        let visible: Vec<&&TurnItem> = items
            .iter()
            .filter(|i| i.kind != "reasoning" || !i.str("text").trim().is_empty())
            .collect();
        if !visible.is_empty() && visible.iter().all(|i| i.kind == "reasoning") {
            let text = visible
                .iter()
                .map(|i| i.str("text"))
                .collect::<Vec<_>>()
                .join("\n\n");
            return self.reasoning_trace(&format!("{}-trace", visible[0].id), &text, cx);
        }
        let capped = !visible.iter().any(|i| self.expanded.contains(&i.id));
        div()
            .id(SharedString::from(format!(
                "{}-rows",
                visible.first().map_or("", |i| i.id.as_str())
            )))
            .when(capped, |d| d.max_h(px(288.)).overflow_y_scroll())
            .children(
                visible
                    .into_iter()
                    .map(|item| self.render_work_row(item, false, true, cx)),
            )
            .into_any_element()
    }

    /// "Thinking" at the end of a running turn (web: `ThinkingTimelineRow`): the launching
    /// provider's logo and step while it starts; after a failed call it opens to that work.
    pub(crate) fn render_thinking(
        &self,
        group: Option<&(String, Vec<&TurnItem>)>,
        waiting: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = crate::web_theme::current(cx);
        let dark = cx.theme().mode.is_dark();
        let starting = self.run_activity().provider_starting;
        let text = starting
            .as_ref()
            .map(|(_, driver, phase)| provider_starting_label(driver.as_deref(), phase.as_deref()))
            .or_else(|| waiting.map(str::to_owned))
            .unwrap_or_else(|| t("Thinking"));
        // Nothing thinks while an approval waits.
        let live = !self.thread.items().any(|i| {
            i.kind == "approval_request"
                && i.str("status") != "completed"
                && !matches!(i.str("status"), "failed" | "cancelled" | "interrupted")
        });
        let sweep = live.then_some(Sweep::Focus);
        let key = "thinking".to_owned();
        let color = self.swept_icon_color(&key, sweep, cx);
        let logo = starting.as_ref().and_then(|(instance, driver, _)| {
            let driver = driver.as_deref()?;
            Some(self.provider_starting_icon(instance, driver, dark))
        });
        let icon = match logo {
            Some(logo) => div()
                .opacity(
                    if color == cx.theme().colors.muted_foreground.opacity(0.7) {
                        0.7
                    } else {
                        1.
                    },
                )
                .child(logo)
                .into_any_element(),
            None => crate::ui::icon("brain")
                .size(px(16.))
                .text_color(color)
                .into_any_element(),
        };
        let label = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(tokens.secondary_label)
            .child(text.clone())
            .into_any_element();
        let row = self.live_activity_row(&key, icon, label, text, false, sweep, cx);
        let Some((id, items)) = group else {
            return row.into_any_element();
        };
        let expanded = self.expanded.contains(id);
        let group_id = id.clone();
        v_flex()
            .child(
                div()
                    .id(SharedString::from(format!("thinking-{id}")))
                    .w_full()
                    .rounded_md()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&group_id) {
                            this.expanded.insert(group_id.clone());
                        }
                        cx.notify();
                    }))
                    .child(row),
            )
            .when(expanded, |col| col.child(self.render_group_rows(items, cx)))
            .into_any_element()
    }

    /// `ProviderStartingIcon`: the instance's logo, or its ACP registry agent's icon.
    fn provider_starting_icon(&self, instance: &str, driver: &str, dark: bool) -> AnyElement {
        if driver == "acpRegistry" {
            let config = &self.config().settings["providerInstances"][instance]["config"];
            let registry = (config["source"] != "local")
                .then(|| {
                    config["registryIconUrl"]
                        .as_str()
                        .filter(|u| u.starts_with("https://cdn.agentclientprotocol.com/"))
                        .map(str::to_owned)
                        .or_else(|| {
                            config["agentId"]
                                .as_str()
                                .map(str::trim)
                                .filter(|id| {
                                    !id.is_empty()
                                        && id.chars().next().is_some_and(|c| {
                                            c.is_ascii_lowercase() || c.is_ascii_digit()
                                        })
                                        && id.chars().all(|c| {
                                            c.is_ascii_lowercase()
                                                || c.is_ascii_digit()
                                                || "._-".contains(c)
                                        })
                                })
                                .map(|id| {
                                    format!(
                                        "https://cdn.agentclientprotocol.com/registry/v1/latest/{id}.svg"
                                    )
                                })
                        })
                })
                .flatten();
            if let Some(url) = registry {
                let driver = driver.to_owned();
                return img(SharedUri::from(url))
                    .size(px(16.))
                    .with_fallback(move || match crate::ui::provider_logo(&driver, dark) {
                        Some(logo) => logo.into_any_element(),
                        None => div().into_any_element(),
                    })
                    .into_any_element();
            }
        }
        match crate::ui::provider_logo(driver, dark) {
            Some(logo) => logo.into_any_element(),
            None => {
                // `providerInstanceInitials` of the provider's name.
                let name = self
                    .config()
                    .providers
                    .iter()
                    .find(|p| p.instance_id == instance)
                    .and_then(|p| p.display_name.clone())
                    .or_else(|| provider_display_name(driver).map(str::to_owned))
                    .unwrap_or_default();
                let initials: String = name
                    .split_whitespace()
                    .filter_map(|w| w.chars().next())
                    .take(2)
                    .collect::<String>()
                    .to_uppercase();
                div()
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(9.))
                    .child(initials)
                    .into_any_element()
            }
        }
    }
}

/// `BrowserAppIcon`.
const BROWSER_ICON: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" xmlns="http://www.w3.org/2000/svg"><path d="M8.5 19H7.2C4.4 19 3 17.5 3 14.6V7.4C3 4.5 4.5 3 7.4 3h8.2C18.5 3 20 4.5 20 7.4v2.4"/><circle cx="7.4" cy="7.2" r="0.75" fill="currentColor" stroke="none"/><path d="M11.2 7.2h4.3"/><path d="m12.4 11.4 7.5 2.6-3.4 1.6-1.5 3.6z" fill="currentColor" stroke="none"/></svg>"#;

/// `T3Wordmark`.
const T3_WORDMARK: &str = r#"<svg viewBox="15.5309 37 94.3941 56.96" xmlns="http://www.w3.org/2000/svg"><path d="M33.4509 93V47.56H15.5309V37H64.3309V47.56H46.4109V93H33.4509ZM86.7253 93.96C82.832 93.96 78.9653 93.4533 75.1253 92.44C71.2853 91.3733 68.032 89.88 65.3653 87.96L70.4053 78.04C72.5386 79.5867 75.0186 80.8133 77.8453 81.72C80.672 82.6267 83.5253 83.08 86.4053 83.08C89.6586 83.08 92.2186 82.44 94.0853 81.16C95.952 79.88 96.8853 78.12 96.8853 75.88C96.8853 73.7467 96.0586 72.0667 94.4053 70.84C92.752 69.6133 90.0853 69 86.4053 69H80.4853V60.44L96.0853 42.76L97.5253 47.4H68.1653V37H107.365V45.4L91.8453 63.08L85.2853 59.32H89.0453C95.9253 59.32 101.125 60.8667 104.645 63.96C108.165 67.0533 109.925 71.0267 109.925 75.88C109.925 79.0267 109.099 81.9867 107.445 84.76C105.792 87.48 103.259 89.6933 99.8453 91.4C96.432 93.1067 92.0586 93.96 86.7253 93.96Z" fill="currentColor"/></svg>"#;

#[cfg(test)]
mod tests {
    use super::{
        CommandOutput, action, command_text, format_tool_value, group_icon, hidden_from_timeline,
        icon_name, icon_url, label, relative_path, summarize, terminal_spans, thought_preview,
    };
    use crate::model::TurnItem;
    use serde_json::{Value, json};

    fn item(v: Value) -> TurnItem {
        let mut v = v;
        v["id"] = json!(format!("i{}", v["type"]));
        v["ordinal"] = json!(1);
        TurnItem::from_value(&v).unwrap()
    }

    #[test]
    fn labels_rows_like_the_web() {
        let root = Some("/w/demo");
        let read = item(json!({ "type": "dynamic_tool", "toolName": "Read",
            "title": "Read /w/demo/package.json", "input": { "path": "/w/demo/package.json" } }));
        assert_eq!(action(&read), "read");
        assert_eq!(icon_name(&read), "eye");
        assert_eq!(label(&read, root, true), "Read /w/demo/package.json");
        assert_eq!(label(&read, root, false), "Read demo/package.json");

        let edit = item(json!({ "type": "file_change", "fileName": "/w/demo/src/math.ts" }));
        assert_eq!(label(&edit, root, true), "Changed 1 file");
        assert_eq!(label(&edit, root, false), "demo/src/math.ts");

        let cmd = item(json!({ "type": "command_execution", "input": "bash -lc 'vp test run'" }));
        assert_eq!(label(&cmd, root, false), "vp test run");
        let quiet = item(json!({ "type": "command_execution", "input": "", "status": "running" }));
        assert_eq!(label(&quiet, root, false), "Running command");

        let todo = item(json!({ "type": "todo_list", "steps": [
            { "text": "a", "status": "completed" }, { "text": "b", "status": "running" } ] }));
        assert_eq!(label(&todo, root, true), "Tasks 1/2 · b");

        let thought =
            item(json!({ "type": "reasoning", "text": "**Plan**\n\nRead  the `manifest`." }));
        assert_eq!(label(&thought, root, true), "Plan Read the manifest.");

        let grep = item(json!({ "type": "dynamic_tool", "toolName": "Grep",
            "input": { "pattern": "clamp", "path": "/w/demo/src" } }));
        assert_eq!(label(&grep, root, false), "Searched clamp in src");
        assert_eq!(icon_name(&grep), "search");
    }

    #[test]
    fn summarizes_groups_like_the_web() {
        let edit = item(json!({ "type": "file_change", "fileName": "a.ts" }));
        let cmd = item(json!({ "type": "command_execution", "input": "ls" }));
        let think = item(json!({ "type": "reasoning", "text": "x" }));
        assert_eq!(
            summarize(&[&think, &edit, &cmd]),
            "Changed 1 file and ran 1 command"
        );
        assert_eq!(group_icon(&[&edit, &cmd]), "hammer");
        assert_eq!(group_icon(&[&cmd, &think]), "terminal");
        assert_eq!(summarize(&[&think, &think]), "Thought (×2)");
        let browse = item(json!({ "type": "dynamic_tool", "toolName": "click",
            "toolSource": { "key": "b", "name": "Chrome", "kind": "browser" } }));
        assert_eq!(summarize(&[&browse, &cmd]), "Used Chrome and ran 1 command");
    }

    #[test]
    fn unwraps_shell_wrappers_only_when_safe() {
        assert_eq!(command_text("/bin/zsh -lc \"git status\""), "git status");
        assert_eq!(command_text("bash -c 'a' extra"), "bash -c 'a' extra");
        assert_eq!(command_text("bash -lc 'cd x && make'"), "cd x && make");
        assert_eq!(command_text("bash -lc 'a'; echo b"), "bash -lc 'a'; echo b");
        assert_eq!(
            command_text("pwsh -NoProfile -Command Get-ChildItem"),
            "Get-ChildItem"
        );
        assert_eq!(command_text(r#"cmd.exe /d /c "dir""#), "dir");
        assert_eq!(
            command_text(r#"bash -c 'echo "hi"' 2>&1"#),
            r#"bash -c 'echo "hi"' 2>&1"#
        );
        assert_eq!(command_text("echo \u{1b}"), "echo ␛");
    }

    #[test]
    fn hides_workspace_preparation_and_checkpoints() {
        let prep = item(json!({ "type": "command_execution", "input": "Preparing workspace" }));
        let check = item(json!({ "type": "checkpoint" }));
        let cmd = item(json!({ "type": "command_execution", "input": "ls" }));
        assert!(hidden_from_timeline(&prep) && hidden_from_timeline(&check));
        assert!(!hidden_from_timeline(&cmd));
    }

    #[test]
    fn paths_read_relative_to_the_workspace() {
        assert_eq!(relative_path("/w/demo", Some("/w/demo/")), "demo");
        assert_eq!(
            relative_path("./src/a.ts", Some("/w/demo")),
            "demo/src/a.ts"
        );
        assert_eq!(relative_path("/etc/hosts", Some("/w/demo")), "/etc/hosts");
    }

    #[test]
    fn formats_tool_output_like_the_web() {
        let mcp = json!({ "content": [{ "type": "text", "text": "{\"a\":1}" }] });
        assert_eq!(format_tool_value(&mcp).unwrap(), "{\n  \"a\": 1\n}");
        let acp = json!([{ "type": "content", "content": { "type": "text", "text": "hi" } }]);
        assert!(format_tool_value(&acp).unwrap().starts_with("[\n"));
        assert_eq!(format_tool_value(&json!("")), None);
    }

    #[test]
    fn picks_favicons_and_logos() {
        let site = json!({ "_tag": "website", "pageUrl": "https://example.com/a?b" });
        assert_eq!(
            icon_url(&site, false).unwrap(),
            "https://example.com/favicon.ico"
        );
        let gh = json!({ "_tag": "website", "pageUrl": "https://github.com/x" });
        assert!(icon_url(&gh, true).unwrap().ends_with("favicon-dark.svg"));
        let logo = json!({ "_tag": "themed-logo", "logoUrl": "https://l/a.svg",
            "logoUrlDark": "https://l/b.svg" });
        assert_eq!(icon_url(&logo, true).unwrap(), "https://l/b.svg");
        assert_eq!(icon_url(&logo, false).unwrap(), "https://l/a.svg");
    }

    #[test]
    fn styles_terminal_output() {
        let (plain, styles) = terminal_spans("ok \u{1b}[0;32mpass\u{1b}[0m done", true);
        assert_eq!(plain, "ok pass done");
        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].0, 3..7);

        let mut out = CommandOutput::default();
        out.apply(
            &json!({ "kind": "replace", "text": "a\n", "truncated": false, "running": true }),
        );
        out.apply(&json!({ "kind": "append", "text": "10%\r50%\n", "truncated": false, "running": false }));
        assert_eq!(out.text(), "a\n50%\n");
        assert!(!out.running);
    }

    #[test]
    fn live_thoughts_and_sweep_match_the_web() {
        use super::{Sweep, live_thought_line};
        assert_eq!(
            live_thought_line("**Reading files**\nthen more"),
            "Reading files"
        );
        assert_eq!(live_thought_line("First step. Second step."), "First step.");
        assert_eq!(Sweep::Focus.alpha(36.), 1.);
        assert_eq!(Sweep::Focus.alpha(-1.), 0.);
        assert!((Sweep::Focus.alpha(10.8) - 0.12).abs() < 1e-4);
        assert_eq!(Sweep::Shine.alpha(36.), 1.);
        assert_eq!(Sweep::Focus.start(0., 100.), -72.);
        assert_eq!(Sweep::Focus.start(1., 100.), 172.);
    }

    #[test]
    fn previews_thoughts_on_one_line() {
        assert_eq!(thought_preview("# Title\n- a *b*"), "Title - a b");
    }
}
