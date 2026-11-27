//! T3 Code's own MCP tools as the web presents them (`@t3tools/shared/t3McpToolPresentation`,
//! `client-runtime/src/work-log/t3ToolSentences.ts`, `client-runtime/src/t3ToolSummary.ts`).
//! The tables come from `t3_tools.json`, generated from that source by `scripts/t3-tools.ts`;
//! summaries the generator can't read as a table are ported in `special_summary`.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

use crate::i18n::tf;

#[derive(Deserialize)]
pub(crate) struct Definition {
    pub labels: [String; 4],
    pub icon: String,
    #[serde(rename = "summaryAction")]
    pub summary_action: String,
}

#[derive(Deserialize)]
struct Sentence {
    detail: String,
    #[serde(rename = "changeRequest")]
    change_request: Option<String>,
}

#[derive(Deserialize)]
struct Summary {
    count: String,
    done: String,
    tried: String,
}

#[derive(Deserialize)]
struct Tables {
    tools: HashMap<String, Definition>,
    sentences: HashMap<String, Sentence>,
    summaries: HashMap<String, Summary>,
    /// Read by the test that checks every special summary is ported.
    #[cfg_attr(not(test), allow(dead_code))]
    special: Vec<String>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    serde_json::from_str(include_str!("t3_tools.json")).expect("t3_tools.json is valid")
});

const SERVER_ALIASES: [&str; 5] = [
    "t3-code",
    "t3_code",
    "t3code",
    "t3-code-direct",
    "t3_code_direct",
];

fn known(name: &str) -> bool {
    TABLES.tools.contains_key(name)
}

/// `parseT3McpToolName`: the T3 tool a provider's tool name or title names.
fn tool_name(value: &str) -> Option<String> {
    let label = value.trim();
    let label = ["completed", "complete"]
        .iter()
        .find_map(|suffix| {
            let lower = label.to_lowercase();
            lower
                .strip_suffix(suffix)
                .filter(|rest| rest.ends_with(char::is_whitespace))
                .map(|rest| label[..rest.len()].trim_end())
        })
        .unwrap_or(label);
    let lower = label.to_lowercase();
    if let Some(rest) = lower.strip_prefix("mcp__") {
        let split = rest.find("__")?;
        let server = &rest[..split];
        let tool = &label["mcp__".len() + split + 2..];
        return (!server.is_empty() && !tool.is_empty() && SERVER_ALIASES.contains(&server))
            .then(|| tool.to_owned());
    }
    for server in [
        "t3-code-direct",
        "t3_code_direct",
        "t3-code",
        "t3_code",
        "t3code",
    ] {
        if let Some(rest) = lower.strip_prefix(server) {
            let rest_raw = &label[server.len()..];
            let tool = rest_raw.strip_prefix(['.', ':', '/']).or_else(|| {
                rest.trim_start()
                    .strip_prefix('·')
                    .map(|_| rest_raw.trim_start().trim_start_matches('·').trim_start())
            });
            if let Some(tool) = tool.filter(|t| !t.is_empty()) {
                return Some(tool.to_owned());
            }
        }
    }
    // `(mcp_|mcp__)?t3[-_ ]?code(__|[-_.:/ ])<tool>`
    let mut rest = lower.as_str();
    for prefix in ["mcp__", "mcp_", "mcp-", "mcp--"] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r;
            break;
        }
    }
    let candidate = rest.strip_prefix("t3").and_then(|r| {
        let r = r.strip_prefix(['-', '_', ' ']).unwrap_or(r);
        let r = r.strip_prefix("code")?;
        r.strip_prefix("__")
            .or_else(|| r.strip_prefix(['-', '_', '.', ':', '/', ' ']))
    });
    let candidate = match candidate {
        Some(tool) if !tool.is_empty() => &label[label.len() - tool.len()..],
        _ => label,
    };
    if known(candidate) {
        return Some(candidate.to_owned());
    }
    // OpenCode 2 thread prefixes can carry underscores; the longest known suffix wins.
    if !lower.starts_with("t3-code-") {
        return None;
    }
    TABLES
        .tools
        .keys()
        .filter(|tool| label.ends_with(&format!("_{tool}")))
        .max_by_key(|tool| tool.len())
        .cloned()
}

/// `resolveT3McpToolDefinition`: action tools resolve to the single tool their action replaced.
pub(crate) fn definition(tool: &str, input: &Value) -> Option<&'static Definition> {
    let name = tool_name(tool).filter(|n| known(n))?;
    let action = input["action"].as_str();
    let action_tool = action.and_then(|action| match name.as_str() {
        "preview_act" => Some(format!("preview_{action}")),
        "t3_queue" => Some(format!("t3_queue_{action}")),
        "t3_pending_request" => Some(format!("t3_pending_request_{action}")),
        "t3_thread_transfer" => match action {
            "fork" => Some("t3_thread_fork".into()),
            "merge_back" => Some("t3_thread_merge_back".into()),
            "status" => Some("t3_thread_transfers".into()),
            _ => None,
        },
        _ => None,
    });
    TABLES
        .tools
        .get(action_tool.filter(|t| known(t)).as_deref().unwrap_or(&name))
}

/// `resolveT3McpSummaryAction`.
pub(crate) fn summary_action(tool: &str, input: &Value) -> Option<String> {
    let definition = definition(tool, input)?;
    let by_fields = match tool_name(tool).as_deref() {
        Some("t3_thread_configure") if input.is_object() => {
            Some(if input.get("modelSelection").is_none() {
                "thread-configuration"
            } else {
                "thread-configure"
            })
        }
        Some("schedule_task") if input.is_object() => {
            Some(if input.get("scheduledTaskId").is_none() {
                "schedule-create"
            } else {
                "schedule-update"
            })
        }
        _ => None,
    };
    Some(by_fields.unwrap_or(&definition.summary_action).to_owned())
}

/// `readResult`: a tool result's data record, and whether it reports an error.
fn read_result(value: &Value, depth: usize) -> (Option<Value>, bool) {
    if depth > 4 {
        return (None, false);
    }
    match value {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(parsed) => read_result(&parsed, depth + 1),
            Err(_) => (None, false),
        },
        Value::Array(blocks) => {
            let mut data = None;
            let mut failed = false;
            for block in blocks {
                let text = block
                    .get("text")
                    .or_else(|| block.get("content").and_then(|c| c.get("text")))
                    .unwrap_or(&Value::Null);
                let text = text.get("text").unwrap_or(text);
                let (d, f) = read_result(text, depth + 1);
                if data.is_none() {
                    data = d;
                }
                failed |= f;
            }
            (data, failed)
        }
        Value::Object(record) => {
            let failed = record.get("isError") == Some(&Value::Bool(true))
                || record.get("is_error") == Some(&Value::Bool(true))
                || record
                    .get("_tag")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.ends_with("Error") || t.ends_with("Failure"))
                || record.get("error").is_some_and(|e| !e.is_null());
            match record.get("structuredContent").or(record.get("content")) {
                Some(content) => {
                    let (d, f) = read_result(content, depth + 1);
                    (d, failed || f)
                }
                None => (Some(value.clone()), failed),
            }
        }
        _ => (None, false),
    }
}

/// `t3ToolResultIndicatesFailure`: MCP errors can come back as data from a completed call.
pub(crate) fn result_failed(output: &Value) -> bool {
    read_result(output, 0).1
}

fn read_input(value: &Value) -> Option<Value> {
    let input = read_result(value, 0).0?;
    // Historical Cursor items retain the MCP args envelope.
    if input["toolName"].is_string() {
        return input.get("args").filter(|a| a.is_object()).cloned();
    }
    Some(input)
}

/// The short word a pull request URL's host uses (`parseChangeRequestUrl`), and its number.
fn change_request(input: &Value) -> (&'static str, Option<u64>) {
    let from_url = input["url"].as_str().and_then(|url| {
        let path = url.split("://").nth(1)?.split(['?', '#']).next()?;
        let segments: Vec<&str> = path.split('/').collect();
        segments.windows(2).find_map(|pair| {
            let number = pair[1].parse::<u64>().ok()?;
            match pair[0] {
                "merge_requests" => Some(("MR", number)),
                "pull" | "pulls" | "pull-requests" | "pullrequest" => Some(("PR", number)),
                _ => None,
            }
        })
    });
    match from_url {
        Some((short, number)) => (short, Some(number)),
        None => ("change request", input["number"].as_u64()),
    }
}

/// `resolveT3McpToolPresentation` for a row: its sentence for `status`, icon, and the PR
/// action it counts as.
pub(crate) struct Presentation {
    pub display_name: String,
    pub icon: &'static str,
    pub action: Option<&'static str>,
}

pub(crate) fn presentation(
    tool: &str,
    input: &Value,
    output: &Value,
    status: &str,
) -> Option<Presentation> {
    let definition = definition(tool, input)?;
    let status = if result_failed(output) {
        "failed"
    } else {
        status
    };
    let action = match definition.summary_action.as_str() {
        "link-pr" => Some("link-pr"),
        "unlink-pr" => Some("unlink-pr"),
        "list-prs" => Some("list-prs"),
        "watch-pr" => Some("watch-pr"),
        "unwatch-pr" => Some("unwatch-pr"),
        _ => None,
    };
    let (short, number) = change_request(input);
    let number = number.filter(|&n| n > 0 && action.is_some() && action != Some("list-prs"));
    let key = definition.labels.join("\n");
    let status_key = match status {
        "inProgress" | "completed" | "failed" | "declined" | "stopped" => status,
        _ => "other",
    };
    let [verb, running, completed, detail] = &definition.labels;
    let display_name = match TABLES.sentences.get(&key) {
        Some(sentence) => match (number, &sentence.change_request) {
            (Some(n), Some(message)) => tf(
                message,
                &[
                    ("status", status_key),
                    ("changeRequest", short),
                    ("number", &n.to_string()),
                ],
            ),
            _ => tf(&sentence.detail, &[("status", status_key)]),
        },
        // A tool added upstream after the sentences were generated.
        None => {
            let lead = match status {
                "inProgress" => running.clone(),
                "completed" => completed.clone(),
                "failed" => format!("Failed to {}", verb.to_lowercase()),
                "declined" => format!("Declined to {}", verb.to_lowercase()),
                "stopped" => format!("Stopped {}", running.to_lowercase()),
                _ => running.clone(),
            };
            match number {
                Some(n) => format!("{lead} {short} #{n}"),
                None => format!("{lead} {detail}"),
            }
        }
    };
    let icon = match definition.icon.as_str() {
        "browser" => "browser",
        "device" => "device",
        "pull-request" => "pull-request",
        _ => "t3-code",
    };
    Some(Presentation {
        display_name,
        icon,
        action,
    })
}

/// `summaryActionPriority` for T3 actions: what they change outranks what they read.
pub(crate) fn priority(action: &str) -> u8 {
    match action {
        "delegate" | "task-cancel" | "thread-create" | "thread-send" | "thread-interrupt"
        | "workflow-stop" | "background-wait" | "schedule-create" | "schedule-update"
        | "schedule-delete" | "schedule-run" | "thread-configure" | "thread-fork"
        | "thread-merge" | "thread-organize" | "thread-update" | "queue-edit" | "queue-cancel"
        | "queue-reorder" | "queue-steer" | "question-respond" | "worktree-handoff"
        | "project-create" | "project-update" | "project-delete" | "project-clone"
        | "environment-update" | "theme-set" | "attachment-prepare" | "attachment-discard"
        | "attachment-send" => 0,
        _ => 1,
    }
}

/// One call in a group: its input, output and how it ended.
pub(crate) struct Call<'a> {
    pub input: &'a Value,
    pub output: &'a Value,
    /// "completed", "failed" or "unfinished".
    pub outcome: &'static str,
}

struct Read {
    input: Option<Value>,
    output: Option<Value>,
}

fn id(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

fn count_entities(ids: &[Option<String>]) -> usize {
    let mut unique: Vec<&String> = ids.iter().flatten().collect();
    unique.sort();
    unique.dedup();
    unique.len() + ids.iter().filter(|i| i.is_none()).count()
}

fn n(value: usize) -> String {
    value.to_string()
}

/// `summarizeT3ToolCalls`: a group's label and how many of its calls failed.
pub(crate) fn summarize(action: &str, calls: &[Call]) -> (String, usize) {
    let results: Vec<(Read, &str)> = calls
        .iter()
        .map(|call| {
            let (output, failed) = read_result(call.output, 0);
            let read = Read {
                input: read_input(call.input),
                output,
            };
            (read, if failed { "failed" } else { call.outcome })
        })
        .collect();
    let mut failed = results.iter().filter(|r| r.1 == "failed").count();
    let completed: Vec<&Read> = results
        .iter()
        .filter(|r| r.1 == "completed")
        .map(|r| &r.0)
        .collect();
    let done = !completed.is_empty();
    let selected: Vec<&Read> = if done {
        completed
    } else {
        results.iter().map(|r| &r.0).collect()
    };
    let field = |r: &Read, key: &str| {
        id(r.output.as_ref().and_then(|o| o.get(key)))
            .or_else(|| id(r.input.as_ref().and_then(|i| i.get(key))))
    };
    let thread_ids: Vec<Option<String>> = selected
        .iter()
        .map(|r| {
            id(r.output.as_ref().and_then(|o| o.get("threadId")))
                .or_else(|| id(r.output.as_ref().and_then(|o| o["thread"].get("threadId"))))
                .or_else(|| id(r.input.as_ref().and_then(|i| i.get("threadId"))))
        })
        .collect();
    let project_ids: Vec<Option<String>> = selected
        .iter()
        .map(|r| {
            id(r.output.as_ref().and_then(|o| o.get("id")))
                .or_else(|| id(r.output.as_ref().and_then(|o| o.get("projectId"))))
                .or_else(|| id(r.input.as_ref().and_then(|i| i.get("projectId"))))
        })
        .collect();
    let label = if let Some(summary) = TABLES.summaries.get(action) {
        let count = match summary.count.as_str() {
            "calls" => selected.len(),
            "threads" => count_entities(&thread_ids),
            "projects" => count_entities(&project_ids),
            key => count_entities(&selected.iter().map(|r| field(r, key)).collect::<Vec<_>>()),
        };
        tf(
            if done { &summary.done } else { &summary.tried },
            &[("count", &n(count))],
        )
    } else {
        special_summary(action, &selected, done, &thread_ids, &mut failed)
    };
    (label, failed)
}

fn pick<'a>(done: bool, yes: &'a str, no: &'a str) -> &'a str {
    if done { yes } else { no }
}

/// The `summarizeT3ToolCalls` cases whose labels depend on more than one count.
fn special_summary(
    action: &str,
    selected: &[&Read],
    done: bool,
    thread_ids: &[Option<String>],
    failed: &mut usize,
) -> String {
    let calls = selected.len();
    let all_threads = thread_ids.iter().all(Option::is_some);
    let unique_threads = {
        let mut ids: Vec<&Option<String>> = thread_ids.iter().collect();
        ids.sort();
        ids.dedup();
        ids.len()
    };
    let output = |r: &Read, key: &str| r.output.as_ref().and_then(|o| o.get(key)).cloned();
    match action {
        "thread-read" | "thread-wait" => {
            let read = action == "thread-read";
            if all_threads {
                tf(
                    match (read, done) {
                        (true, true) => {
                            "{count, plural, one {Read {count} thread} other {Read {count} threads}}"
                        }
                        (true, false) => {
                            "{count, plural, one {Tried to read {count} thread} other {Tried to read {count} threads}}"
                        }
                        (false, true) => {
                            "{count, plural, one {Waited on {count} thread} other {Waited on {count} threads}}"
                        }
                        (false, false) => {
                            "{count, plural, one {Tried to wait on {count} thread} other {Tried to wait on {count} threads}}"
                        }
                    },
                    &[("count", &n(unique_threads))],
                )
            } else {
                tf(
                    match (read, done) {
                        (true, true) => {
                            "{count, plural, one {Read threads {count} time} other {Read threads {count} times}}"
                        }
                        (true, false) => {
                            "{count, plural, one {Tried to read threads {count} time} other {Tried to read threads {count} times}}"
                        }
                        (false, true) => {
                            "{count, plural, one {Waited on threads {count} time} other {Waited on threads {count} times}}"
                        }
                        (false, false) => {
                            "{count, plural, one {Tried to wait on threads {count} time} other {Tried to wait on threads {count} times}}"
                        }
                    },
                    &[("count", &n(calls))],
                )
            }
        }
        "thread-send" => {
            let messages = count_entities(
                &selected
                    .iter()
                    .map(|r| id(output(r, "messageId").as_ref()))
                    .collect::<Vec<_>>(),
            );
            let threads = unique_threads;
            if !all_threads {
                tf(
                    pick(
                        done,
                        "{count, plural, one {Sent {count} message} other {Sent {count} messages}}",
                        "{count, plural, one {Tried to send {count} message} other {Tried to send {count} messages}}",
                    ),
                    &[("count", &n(messages))],
                )
            } else if messages == threads && messages > 1 {
                tf(
                    pick(
                        done,
                        "{threads, plural, one {Sent messages to {threads} thread} other {Sent messages to {threads} threads}}",
                        "{threads, plural, one {Tried to send messages to {threads} thread} other {Tried to send messages to {threads} threads}}",
                    ),
                    &[("threads", &n(threads))],
                )
            } else {
                tf(
                    pick(
                        done,
                        "{messages, plural, one {Sent {messages} message to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Sent {messages} messages to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                        "{messages, plural, one {Tried to send {messages} message to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Tried to send {messages} messages to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    ),
                    &[("messages", &n(messages)), ("threads", &n(threads))],
                )
            }
        }
        "thread-create" => {
            let mut created: Vec<String> = Vec::new();
            let known = done
                && selected.iter().all(|r| {
                    let threads = match output(r, "threads") {
                        Some(Value::Array(list)) => list,
                        _ => vec![r.output.clone().unwrap_or(Value::Null)],
                    };
                    threads.iter().all(|thread| {
                        if thread["status"] == "rolled_back" {
                            return true;
                        }
                        match id(thread.get("threadId")) {
                            Some(thread_id) => {
                                created.push(thread_id);
                                true
                            }
                            None => false,
                        }
                    })
                });
            if known {
                created.sort();
                created.dedup();
                tf(
                    "{count, plural, one {Created {count} thread} other {Created {count} threads}}",
                    &[("count", &n(created.len()))],
                )
            } else {
                tf(
                    "{count, plural, one {Requested thread creation {count} time} other {Requested thread creation {count} times}}",
                    &[("count", &n(calls))],
                )
            }
        }
        "attachment-send" => {
            let mut messages: Vec<(String, &Read)> = Vec::new();
            for (index, r) in selected.iter().enumerate() {
                let key = id(output(r, "messageId").as_ref()).unwrap_or(format!("call-{index}"));
                match messages.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = r,
                    None => messages.push((key, r)),
                }
            }
            let sizes: Vec<Option<usize>> = messages
                .iter()
                .map(|(_, r)| {
                    r.input
                        .as_ref()
                        .and_then(|i| i["attachments"].as_array())
                        .map(Vec::len)
                })
                .collect();
            let attachments: usize = sizes.iter().flatten().sum();
            let counts_known = sizes.iter().all(|s| s.is_some_and(|s| s > 0));
            let threads = n(unique_threads);
            let (attachments, count) = (n(attachments), n(calls));
            match (all_threads, counts_known) {
                (true, true) => tf(
                    pick(
                        done,
                        "{attachments, plural, one {Sent {attachments} attachment to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Sent {attachments} attachments to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                        "{attachments, plural, one {Tried to send {attachments} attachment to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Tried to send {attachments} attachments to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    ),
                    &[("attachments", &attachments), ("threads", &threads)],
                ),
                (true, false) => tf(
                    pick(
                        done,
                        "{count, plural, one {Sent attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} time} other {Sent attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} times}}",
                        "{count, plural, one {Tried to send attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} time} other {Tried to send attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} times}}",
                    ),
                    &[("threads", &threads), ("count", &count)],
                ),
                (false, true) => tf(
                    pick(
                        done,
                        "{attachments, plural, one {Sent {attachments} attachment} other {Sent {attachments} attachments}}",
                        "{attachments, plural, one {Tried to send {attachments} attachment} other {Tried to send {attachments} attachments}}",
                    ),
                    &[("attachments", &attachments)],
                ),
                (false, false) => tf(
                    pick(
                        done,
                        "{count, plural, one {Sent attachments {count} time} other {Sent attachments {count} times}}",
                        "{count, plural, one {Tried to send attachments {count} time} other {Tried to send attachments {count} times}}",
                    ),
                    &[("count", &count)],
                ),
            }
        }
        "link-pr" | "unlink-pr" => {
            let link = action == "link-pr";
            let key = if link { "linkedCount" } else { "unlinkedCount" };
            let mut count = Some(0);
            for r in selected {
                let reported = r.output.as_ref().and_then(|o| o[key].as_u64());
                let single = r
                    .input
                    .as_ref()
                    .is_some_and(|i| i.get("url").is_some() || i.get("number").is_some());
                count = match (count, reported) {
                    (Some(c), Some(k)) => Some(c + k as usize),
                    (Some(c), None) if single => Some(c + 1),
                    _ => None,
                };
            }
            match (count, link) {
                (None, true) => tf(
                    pick(done, "Linked pull requests", "Tried to link pull requests"),
                    &[],
                ),
                (None, false) => tf(
                    pick(
                        done,
                        "Unlinked pull requests",
                        "Tried to unlink pull requests",
                    ),
                    &[],
                ),
                (Some(c), true) => tf(
                    pick(
                        done,
                        "{count, plural, one {Linked {count} pull request} other {Linked {count} pull requests}}",
                        "{count, plural, one {Tried to link {count} pull request} other {Tried to link {count} pull requests}}",
                    ),
                    &[("count", &n(c))],
                ),
                (Some(c), false) => tf(
                    pick(
                        done,
                        "{count, plural, one {Unlinked {count} pull request} other {Unlinked {count} pull requests}}",
                        "{count, plural, one {Tried to unlink {count} pull request} other {Tried to unlink {count} pull requests}}",
                    ),
                    &[("count", &n(c))],
                ),
            }
        }
        "list-prs" | "list-thread-issues" => {
            let prs = action == "list-prs";
            if calls == 1 {
                tf(
                    match (prs, done) {
                        (true, true) => "Checked linked pull requests",
                        (true, false) => "Tried to check linked pull requests",
                        (false, true) => "Checked linked issues",
                        (false, false) => "Tried to check linked issues",
                    },
                    &[],
                )
            } else {
                tf(
                    match (prs, done) {
                        (true, true) => {
                            "{count, plural, one {Checked linked pull requests {count} time} other {Checked linked pull requests {count} times}}"
                        }
                        (true, false) => {
                            "{count, plural, one {Tried to check linked pull requests {count} time} other {Tried to check linked pull requests {count} times}}"
                        }
                        (false, true) => {
                            "{count, plural, one {Checked linked issues {count} time} other {Checked linked issues {count} times}}"
                        }
                        (false, false) => {
                            "{count, plural, one {Tried to check linked issues {count} time} other {Tried to check linked issues {count} times}}"
                        }
                    },
                    &[("count", &n(calls))],
                )
            }
        }
        "write-pr-guides" => {
            // One call writes many guides, and reports a refusal per guide rather than failing.
            let outcomes: Vec<bool> = selected
                .iter()
                .flat_map(|r| {
                    output(r, "guides")
                        .and_then(|g| g.as_array().cloned())
                        .unwrap_or_default()
                })
                .map(|guide| guide["outcome"] != "failed")
                .collect();
            let started = outcomes.iter().filter(|s| **s).count();
            let refused = outcomes.len() - started;
            *failed += refused;
            if outcomes.is_empty() {
                if calls == 1 {
                    tf(
                        pick(
                            done,
                            "Started writing review guides",
                            "Tried to start writing review guides",
                        ),
                        &[],
                    )
                } else {
                    tf(
                        pick(
                            done,
                            "{count, plural, one {Started writing review guides {count} time} other {Started writing review guides {count} times}}",
                            "{count, plural, one {Tried to start writing review guides {count} time} other {Tried to start writing review guides {count} times}}",
                        ),
                        &[("count", &n(calls))],
                    )
                }
            } else if started == 0 {
                tf(
                    "{failed, plural, one {Could not start {failed} review guide} other {Could not start {failed} review guides}}",
                    &[("failed", &n(refused))],
                )
            } else if refused > 0 {
                tf(
                    pick(
                        done,
                        "{started, plural, one {Started {started} review guide, {failed} failed} other {Started {started} review guides, {failed} failed}}",
                        "{started, plural, one {Tried to start {started} review guide, {failed} failed} other {Tried to start {started} review guides, {failed} failed}}",
                    ),
                    &[("started", &n(started)), ("failed", &n(refused))],
                )
            } else {
                tf(
                    pick(
                        done,
                        "{started, plural, one {Started {started} review guide} other {Started {started} review guides}}",
                        "{started, plural, one {Tried to start {started} review guide} other {Tried to start {started} review guides}}",
                    ),
                    &[("started", &n(started))],
                )
            }
        }
        "create-pr" => {
            // An open PR for the branch is returned rather than duplicated.
            let existing = selected
                .iter()
                .filter(|r| output(r, "created") == Some(Value::Bool(false)))
                .count();
            let opened = calls - existing;
            if !done {
                tf(
                    "{count, plural, one {Tried to open {count} pull request} other {Tried to open {count} pull requests}}",
                    &[("count", &n(calls))],
                )
            } else if opened == 0 {
                tf(
                    "{count, plural, one {Found {count} open pull request} other {Found {count} open pull requests}}",
                    &[("count", &n(existing))],
                )
            } else if existing == 0 {
                tf(
                    "{count, plural, one {Opened {count} pull request} other {Opened {count} pull requests}}",
                    &[("count", &n(opened))],
                )
            } else {
                tf(
                    "{opened, plural, one {Opened {opened} pull request, found {existing} open} other {Opened {opened} pull requests, found {existing} open}}",
                    &[("opened", &n(opened)), ("existing", &n(existing))],
                )
            }
        }
        "merge-pr" => {
            // Auto-merge and merge queues leave the PR open until it lands.
            let queued = selected
                .iter()
                .filter(|r| match output(r, "state") {
                    None | Some(Value::Null) => r
                        .input
                        .as_ref()
                        .is_some_and(|i| i["auto"] == Value::Bool(true)),
                    Some(state) => state != "merged",
                })
                .count();
            let merged = calls - queued;
            if !done {
                tf(
                    "{count, plural, one {Tried to merge {count} pull request} other {Tried to merge {count} pull requests}}",
                    &[("count", &n(calls))],
                )
            } else if merged == 0 {
                tf(
                    "{count, plural, one {Queued {count} pull request to merge} other {Queued {count} pull requests to merge}}",
                    &[("count", &n(queued))],
                )
            } else if queued == 0 {
                tf(
                    "{count, plural, one {Merged {count} pull request} other {Merged {count} pull requests}}",
                    &[("count", &n(merged))],
                )
            } else {
                tf(
                    "{merged, plural, one {Merged {merged} pull request, queued {queued}} other {Merged {merged} pull requests, queued {queued}}}",
                    &[("merged", &n(merged)), ("queued", &n(queued))],
                )
            }
        }
        "ci-rerun" => {
            // A host can refuse a check it cannot re-run, such as one outside its CI.
            let outcomes: Vec<bool> = selected
                .iter()
                .flat_map(|r| {
                    output(r, "checks")
                        .and_then(|c| c.as_array().cloned())
                        .unwrap_or_default()
                })
                .map(|check| check["rerun"] == Value::Bool(true))
                .collect();
            let rerun = outcomes.iter().filter(|r| **r).count();
            let refused = outcomes.len() - rerun;
            *failed += refused;
            if !done {
                tf(
                    "{count, plural, one {Tried to re-run CI {count} time} other {Tried to re-run CI {count} times}}",
                    &[("count", &n(calls))],
                )
            } else if outcomes.is_empty() {
                // An empty list means no check had failed, so nothing was asked of the host.
                if selected
                    .iter()
                    .any(|r| output(r, "checks").is_some_and(|c| c.is_array()))
                {
                    tf("Found no failed checks to re-run", &[])
                } else {
                    tf(
                        "{count, plural, one {Re-ran CI {count} time} other {Re-ran CI {count} times}}",
                        &[("count", &n(calls))],
                    )
                }
            } else if rerun == 0 {
                tf(
                    "{count, plural, one {Could not re-run {count} check} other {Could not re-run {count} checks}}",
                    &[("count", &n(refused))],
                )
            } else if refused == 0 {
                tf(
                    "{count, plural, one {Re-ran {count} check} other {Re-ran {count} checks}}",
                    &[("count", &n(rerun))],
                )
            } else {
                tf(
                    "{rerun, plural, one {Re-ran {rerun} check, {refused} refused} other {Re-ran {rerun} checks, {refused} refused}}",
                    &[("rerun", &n(rerun)), ("refused", &n(refused))],
                )
            }
        }
        // An action added upstream since the table was generated reads as a generic tool use.
        _ => tf(
            "{count, plural, one {Used {count} tool} other {Used {count} tools}}",
            &[("count", &n(calls))],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{TABLES, presentation, result_failed, summarize, summary_action, tool_name};
    use serde_json::json;

    #[test]
    fn every_special_summary_is_ported() {
        let ported = [
            "attachment-send",
            "ci-rerun",
            "create-pr",
            "link-pr",
            "list-prs",
            "list-thread-issues",
            "merge-pr",
            "thread-create",
            "thread-read",
            "thread-send",
            "thread-wait",
            "unlink-pr",
            "write-pr-guides",
        ];
        for action in &TABLES.special {
            assert!(
                ported.contains(&action.as_str()),
                "regenerated t3_tools.json has a new special summary {action}; port it"
            );
        }
    }

    #[test]
    fn reads_provider_tool_names() {
        assert_eq!(
            tool_name("mcp__t3-code__link_pull_request").as_deref(),
            Some("link_pull_request")
        );
        assert_eq!(
            tool_name("t3-code.link_pull_request").as_deref(),
            Some("link_pull_request")
        );
        assert_eq!(
            tool_name("link_pull_request completed").as_deref(),
            Some("link_pull_request")
        );
        assert_eq!(tool_name("mcp__github__list"), None);
    }

    #[test]
    fn rows_read_as_the_web_sentences() {
        let row = presentation(
            "mcp__t3-code__link_pull_request",
            &json!({ "url": "https://github.com/a/b/pull/12" }),
            &json!(null),
            "completed",
        )
        .unwrap();
        assert_eq!(row.display_name, "Linked PR #12");
        assert_eq!(row.icon, "pull-request");
        assert_eq!(row.action, Some("link-pr"));
        let failed = presentation(
            "link_pull_request",
            &json!({}),
            &json!({ "isError": true }),
            "completed",
        )
        .unwrap();
        assert_eq!(failed.display_name, "Failed to link a pull request");
    }

    #[test]
    fn groups_summarize_like_the_web() {
        let out = json!({ "linkedCount": 2 });
        let calls = [super::Call {
            input: &json!({}),
            output: &out,
            outcome: "completed",
        }];
        assert_eq!(summarize("link-pr", &calls).0, "Linked 2 pull requests");
        assert_eq!(
            summary_action("schedule_task", &json!({ "scheduledTaskId": "x" })).as_deref(),
            Some("schedule-update")
        );
        assert!(result_failed(
            &json!([{ "type": "text", "text": "{\"_tag\":\"NotFoundError\"}" }])
        ));
    }
}
