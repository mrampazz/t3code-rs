//! Work rows (web: `TimelineWorkEntryRow`, `WorkLog`, `TimelineToolActivityIcon`,
//! `V2ItemInspector`, `CommandOutputPanel`): one line per thought, task list or tool call that
//! opens to its detail. Labels follow `workEntryDisplayLabel` and `summarizeToolGroup` in
//! `apps/web/src/components/chat/MessagesTimeline.logic.ts` and `client-runtime/src/work-log`.
//! shortcut: T3's own MCP tools show their tool name, not the web's generated sentences
//! (`t3ToolSentences.ts`); failure comes from status and exit code, not output text heuristics.

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

/// Kinds the web shows as work rows.
pub(crate) fn is_work_row(item: &TurnItem) -> bool {
    matches!(
        item.kind.as_str(),
        "reasoning"
            | "command_execution"
            | "file_change"
            | "file_search"
            | "web_search"
            | "dynamic_tool"
            | "todo_list"
    )
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

/// `toolGroupAction`: read, edit, command, search, code-search or other.
pub(crate) fn action(item: &TurnItem) -> &'static str {
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
    let first = action(first);
    if !tools.iter().all(|i| action(i) == first) {
        return "hammer";
    }
    match first {
        "other" => "hammer",
        action => action_icon(action),
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

/// `workEntryDisplayIndicatesToolFailure` without the output text scan.
pub(crate) fn failed(item: &TurnItem) -> bool {
    matches!(item.str("status"), "failed" | "declined")
        || (item.kind == "command_execution"
            && item.raw["exitCode"].as_i64().is_some_and(|c| c != 0))
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

/// `commandDisplayText`: a `sh -c '…'` wrapper shows just its script, control bytes as pictures.
pub(crate) fn command_text(command: &str) -> String {
    visible_controls(&unwrap_shell(command))
}

pub(crate) fn visible_controls(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => c,
            '\u{7f}' => '␡',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// `withoutShellWrapper` for the common `bash -lc "script"` shape.
/// shortcut: one quoted script after `-c`/`-lc`; the web's full shell tokenizer also unwraps
/// PowerShell and cmd and keeps chained wrappers intact.
fn unwrap_shell(command: &str) -> String {
    let trimmed = command.trim();
    let mut words = trimmed.splitn(3, ' ');
    let (Some(program), Some(flag), Some(script)) = (words.next(), words.next(), words.next())
    else {
        return trimmed.to_owned();
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let shell = matches!(program, "sh" | "bash" | "zsh" | "dash" | "fish");
    let script_flag = flag.starts_with('-') && flag.ends_with('c') && flag.len() <= 4;
    let script = script.trim();
    let quoted = |q: char| {
        script.len() >= 2
            && script.starts_with(q)
            && script.ends_with(q)
            && !script[1..script.len() - 1].contains(q)
    };
    if !shell || !script_flag {
        return trimmed.to_owned();
    }
    if quoted('\'') || quoted('"') {
        script[1..script.len() - 1].trim().to_owned()
    } else if !script.contains([' ', '\'', '"']) {
        script.to_owned()
    } else {
        trimmed.to_owned()
    }
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
    // (action, entries); integrations with a source name are told apart by name.
    let mut groups: Vec<(&str, Vec<&TurnItem>)> = Vec::new();
    let mut sources: Vec<(String, bool)> = Vec::new();
    for item in &tools {
        if let Some(name) = item.raw["toolSource"]["name"].as_str() {
            let integration = item.raw["toolSource"]["kind"] == "integration";
            if !sources.iter().any(|(n, _)| n == name) {
                sources.push((name.to_owned(), integration));
            }
            continue;
        }
        let action = action(item);
        match groups.iter_mut().find(|g| g.0 == action) {
            Some(group) => group.1.push(item),
            None => groups.push((action, vec![item])),
        }
    }
    let mut summaries: Vec<(usize, u8, usize, String)> = groups
        .iter()
        .enumerate()
        .map(|(index, (action, entries))| {
            let count = if *action == "edit" {
                let mut files: Vec<String> = Vec::new();
                for entry in entries {
                    for f in changed_files(entry) {
                        if !files.contains(&f) {
                            files.push(f);
                        }
                    }
                }
                files.len()
            } else {
                entries.len()
            };
            let priority = if *action == "other" { 2 } else { 1 };
            (index, priority, entries.len(), action_label(action, count))
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
        .filter(|i| i.raw["toolSource"]["name"].is_string())
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
        "todo_list" => any(&raw["steps"]),
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
const PALETTE: [[u32; 16]; 2] = [
    [
        0x3b4048, 0xf47067, 0x57ab5a, 0xc69026, 0x539bf5, 0xb083f0, 0x39c5cf, 0xadbac7, 0x768390,
        0xff938a, 0x6bc46d, 0xdaaa3f, 0x6cb6ff, 0xdcbdfb, 0x56d4dd, 0xcdd9e5,
    ],
    [
        0x24292f, 0xcf222e, 0x116329, 0x7d4e00, 0x0550ae, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a,
        0xa40e26, 0x1a7f37, 0x633c01, 0x0969da, 0x6639ba, 0x3192aa, 0x8c959f,
    ],
];

fn xterm(index: u32, palette: &[u32; 16]) -> u32 {
    if index < 16 {
        return palette[index as usize];
    }
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return level << 16 | level << 8 | level;
    }
    let cube = index - 16;
    let channel = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
    channel(cube / 36) << 16 | channel(cube / 6 % 6) << 8 | channel(cube % 6)
}

/// `terminalOutputSpans`: SGR escapes become styles over the plain text. Each escape carries the
/// whole pen, as the server folds output.
pub(crate) fn terminal_spans(
    text: &str,
    dark: bool,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let palette = &PALETTE[usize::from(!dark)];
    let mut plain = String::with_capacity(text.len());
    let mut styles = Vec::new();
    let mut pen: Option<HighlightStyle> = None;
    let mut start = 0;
    let mut rest = text;
    while let Some(escape) = rest.find("\u{1b}[") {
        plain.push_str(&rest[..escape]);
        let Some(end) = rest[escape..].find(|c: char| c.is_ascii_alphabetic()) else {
            rest = "";
            break;
        };
        let params = &rest[escape + 2..escape + end];
        let is_sgr = rest[escape + end..].starts_with('m');
        rest = &rest[escape + end + 1..];
        if !is_sgr {
            continue;
        }
        if let Some(style) = pen.take()
            && plain.len() > start
        {
            styles.push((start..plain.len(), style));
        }
        start = plain.len();
        pen = sgr_style(params, palette);
    }
    plain.push_str(rest);
    if let Some(style) = pen
        && plain.len() > start
    {
        styles.push((start..plain.len(), style));
    }
    (plain, styles)
}

fn sgr_style(params: &str, palette: &[u32; 16]) -> Option<HighlightStyle> {
    let codes: Vec<u32> = params
        .split([';', ':'])
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let mut style = HighlightStyle::default();
    let mut any = false;
    let mut i = 0;
    let rgb = |c: u32| Some(Hsla::from(gpui_kit::rgb(c)));
    while i < codes.len() {
        let code = codes[i];
        let extended = |i: &mut usize| -> Option<u32> {
            match codes.get(*i + 1) {
                Some(5) => {
                    *i += 2;
                    codes.get(*i).map(|n| xterm(*n, palette))
                }
                Some(2) => {
                    *i += 4;
                    let c = |k: usize| codes.get(*i + k - 3).copied().unwrap_or(0).min(255);
                    Some(c(1) << 16 | c(2) << 8 | c(3))
                }
                _ => None,
            }
        };
        match code {
            1 => style.font_weight = Some(FontWeight::SEMIBOLD),
            2 => style.fade_out = Some(0.35),
            3 => style.font_style = Some(FontStyle::Italic),
            4 => {
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                })
            }
            9 => {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    ..Default::default()
                })
            }
            30..=37 => style.color = rgb(palette[(code - 30) as usize]),
            90..=97 => style.color = rgb(palette[(code - 82) as usize]),
            40..=47 => style.background_color = rgb(palette[(code - 40) as usize]),
            100..=107 => style.background_color = rgb(palette[(code - 92) as usize]),
            38 => style.color = extended(&mut i).and_then(rgb),
            48 => style.background_color = extended(&mut i).and_then(rgb),
            _ => {
                i += 1;
                continue;
            }
        }
        any = true;
        i += 1;
    }
    any.then_some(style)
}

/// One expanded command row's output (`CommandOutputView`), fed by `subscribeCommandOutput`.
#[derive(Default)]
pub(crate) struct CommandOutput {
    pub(crate) text: String,
    truncated: bool,
    pub(crate) running: bool,
    loaded: bool,
    error: Option<String>,
    /// The stream's request id while it runs.
    request: Option<String>,
}

impl CommandOutput {
    /// `applyCommandOutputFrame`.
    /// shortcut: appended live text keeps only SGR styling and carriage-return line rewrites;
    /// the final `replace` frame is the server's fully folded output.
    pub(crate) fn apply(&mut self, frame: &Value) {
        let text = frame["text"].as_str().unwrap_or("");
        if frame["kind"] == "replace" {
            self.text.clear();
            self.truncated = false;
        }
        self.truncated |= frame["truncated"] == true;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' if chars.peek() == Some(&'\n') => {}
                '\r' => {
                    let line_start = self.text.rfind('\n').map_or(0, |n| n + 1);
                    self.text.truncate(line_start);
                }
                c => self.text.push(c),
            }
        }
        self.running = frame["running"] != false;
        self.loaded = true;
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
                name => crate::ui::icon(name)
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
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let expanded = self.expanded.contains(&item.id);
        let can_expand = has_detail(item);
        let reasoning = item.kind == "reasoning";
        let root = self.thread_cwd();
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(tokens.secondary_label)
                    .child(text),
            )
            .child(
                div()
                    .flex_none()
                    .mr_1()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .absolute()
                    .invisible()
                    .group_hover(group.clone(), |s| s.relative().visible())
                    .child(time),
            )
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
            None => crate::ui::icon(group_icon(items))
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
            .child(
                div()
                    .flex_none()
                    .mr_1()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .absolute()
                    .invisible()
                    .group_hover(group, |s| s.relative().visible())
                    .child(time),
            )
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
                            .child(self.code_text(format!("{id}-cmd"), "bash", &command, cx)),
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
            .child(body)
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
        if output.text.is_empty() {
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
        let (plain, styles) = terminal_spans(&output.text, cx.theme().mode.is_dark());
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
                .when(output.truncated, |d| {
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
        let (plain, styles) = terminal_spans("ok \u{1b}[32mpass\u{1b}[0m done\u{1b}[K", true);
        assert_eq!(plain, "ok pass done");
        assert_eq!(styles.len(), 1);
        assert_eq!(styles[0].0, 3..7);

        let mut out = CommandOutput::default();
        out.apply(
            &json!({ "kind": "replace", "text": "a\n", "truncated": false, "running": true }),
        );
        out.apply(&json!({ "kind": "append", "text": "10%\r50%\n", "truncated": false, "running": false }));
        assert_eq!(out.text, "a\n50%\n");
        assert!(!out.running);
    }

    #[test]
    fn previews_thoughts_on_one_line() {
        assert_eq!(thought_preview("# Title\n- a *b*"), "Title - a b");
    }
}
