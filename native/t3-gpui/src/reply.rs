//! Replying to and forking from an assistant response (web: `AssistantReplyButton`,
//! `AssistantForkButton`): a reply cites the response's opening in a `response` context record
//! shown above the composer; a fork copies the thread up to the response's run.

use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::T3App;
use crate::i18n::t;

/// `RESPONSE_OPENING_LENGTH`, `ASSISTANT_CITATION_CONTEXT_LENGTH`, `RESPONSE_LABEL_MAX_CHARS`.
const OPENING: usize = 300;
const CONTEXT: usize = 32;
const LABEL: usize = 64;

/// The response as a reader sees it: its markdown's text, one line per block.
pub(crate) fn plain_text(markdown: &str) -> String {
    fn walk(node: &markdown::mdast::Node, out: &mut String) {
        use markdown::mdast::Node;
        match node {
            Node::Text(t) => out.push_str(&t.value),
            Node::InlineCode(c) => out.push_str(&c.value),
            Node::Code(c) => {
                out.push_str(&c.value);
                out.push('\n');
            }
            Node::Break(_) => out.push('\n'),
            Node::Html(_) | Node::Image(_) | Node::ImageReference(_) => {}
            _ => {
                for child in node.children().into_iter().flatten() {
                    walk(child, out);
                }
                if matches!(
                    node,
                    Node::Paragraph(_)
                        | Node::Heading(_)
                        | Node::TableRow(_)
                        | Node::ThematicBreak(_)
                ) {
                    out.push('\n');
                }
            }
        }
    }
    let Ok(tree) = markdown::to_mdast(markdown, &markdown::ParseOptions::gfm()) else {
        return markdown.to_owned();
    };
    let mut out = String::new();
    walk(&tree, &mut out);
    out
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn text_of(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// `normalizeWhitespace`: every whitespace run becomes one space.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

fn splits_pair(units: &[u16], at: usize) -> bool {
    at > 0
        && at < units.len()
        && (0xd800..=0xdbff).contains(&units[at - 1])
        && (0xdc00..=0xdfff).contains(&units[at])
}

/// `captureAssistantMessageOpening` + `createAssistantTextSelector`: the response's first 300
/// characters, with offsets and context in whitespace-normalized UTF-16 units.
pub(crate) fn opening_selector(text: &str) -> Option<Value> {
    let units = utf16(text);
    let raw_start = units.len() - utf16(text.trim_start()).len();
    let mut raw_end = utf16(text.trim_end()).len().min(raw_start + OPENING);
    if splits_pair(&units, raw_end) {
        raw_end -= 1;
    }
    let quote = text_of(&units[raw_start..raw_end.max(raw_start)]);
    if quote.trim().is_empty() {
        return None;
    }
    let normalized = utf16(&normalize(text));
    let start = utf16(&normalize(&text_of(&units[..raw_start]))).len();
    let end = utf16(&normalize(&text_of(&units[..raw_end]))).len();
    let mut prefix_start = start.saturating_sub(CONTEXT);
    let mut suffix_end = (end + CONTEXT).min(normalized.len());
    if splits_pair(&normalized, prefix_start) {
        prefix_start += 1;
    }
    if splits_pair(&normalized, suffix_end) {
        suffix_end -= 1;
    }
    Some(json!({
        "text": quote,
        "start": start,
        "end": end,
        "prefix": text_of(&normalized[prefix_start..start]),
        "suffix": text_of(&normalized[end..suffix_end]),
    }))
}

/// `sanitizeComposerContextLabel`: one line, no brackets or backslashes, at most 200 units.
fn sanitize_label(label: &str, kind: &str) -> String {
    let cleaned = normalize(&label.replace(['[', ']', '\\', '\r', '\n'], " "));
    let units = utf16(cleaned.trim());
    let mut end = units.len().min(200);
    if splits_pair(&units, end) {
        end -= 1;
    }
    let cleaned = text_of(&units[..end]);
    if cleaned.is_empty() {
        kind.to_owned()
    } else {
        cleaned
    }
}

/// `assistantCitationLabel`: the cited text on one line, cut at 64 characters.
pub(crate) fn citation_label(citation: &Value) -> String {
    let preview = normalize(citation["text"].as_str().unwrap_or(""));
    let units = utf16(&preview);
    if units.len() <= LABEL {
        return preview;
    }
    let mut end = LABEL;
    if splits_pair(&units, end) {
        end -= 1;
    }
    format!("{}…", text_of(&units[..end]))
}

/// `responseContextRecord`.
pub(crate) fn response_record(
    citation: Value,
    item_id: &str,
    subagents: Vec<(String, String)>,
) -> Value {
    let units = utf16(citation["text"].as_str().unwrap_or(""));
    let mut end = units.len().min(LABEL);
    if end == LABEL && (0xd800..=0xdbff).contains(&units[end - 1]) {
        end -= 1;
    }
    let message = citation["messageId"].as_str().unwrap_or("").to_owned();
    let mut record = json!({
        "version": 1,
        "kind": "response",
        "contextId": crate::composer_menu::context_id("response", &message),
        "label": sanitize_label(&text_of(&units[..end]), "response"),
        "citation": citation,
        "itemId": item_id,
    });
    if !subagents.is_empty() {
        record["subagents"] = subagents
            .into_iter()
            .take(20)
            .map(|(thread, title)| json!({ "threadId": thread, "title": sanitize_label(&title, "response") }))
            .collect();
    }
    record
}

/// `forkThreadFromRun`: a `thread.fork` of `source` at the end of `run` into `target`.
pub(crate) fn fork_command(source: &str, target: &str, run: &str, title: &str) -> Value {
    json!({
        "type": "thread.fork",
        "commandId": uuid::Uuid::new_v4().to_string(),
        "createdBy": "user",
        "creationSource": "web",
        "sourceThreadId": source,
        "targetThreadId": target,
        "sourcePoint": { "type": "run", "runId": run },
        "title": title,
    })
}

impl T3App {
    /// "Reply to this response": cites the answer above the composer (web: `replyToResponse`).
    pub(crate) fn reply_to_response(
        &mut self,
        item_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.selected.clone() else {
            return;
        };
        let Some(item) = self.thread.items().find(|i| i.id == item_id) else {
            return;
        };
        // An open question owns the composer, and its answer would not carry the reply.
        if self
            .thread
            .items()
            .any(|i| i.kind == "user_input_request" && i.str("status") == "pending")
        {
            self.error = Some(t("The composer is not ready").into());
            return cx.notify();
        }
        let Some(environment) = self.env().and_then(|e| e.environment_id.clone()) else {
            return;
        };
        let raw = item.str("text");
        let rendered = plain_text(raw);
        let Some(mut citation) = opening_selector(if rendered.trim().is_empty() {
            raw
        } else {
            &rendered
        }) else {
            return;
        };
        citation["version"] = json!(1);
        citation["environmentId"] = json!(environment);
        citation["threadId"] = json!(thread_id);
        citation["messageId"] = json!(item.str("messageId"));
        let subagents = self
            .thread
            .response_subagents(item_id)
            .into_iter()
            .map(|(thread, title)| {
                let title = title
                    .filter(|t| !t.trim().is_empty())
                    .or_else(|| {
                        self.shell()
                            .threads
                            .iter()
                            .find(|t| t.id == thread)
                            .map(|t| t.title.clone())
                            .filter(|t| !t.trim().is_empty())
                    })
                    .unwrap_or_else(|| "Agent".to_owned());
                (thread, title)
            })
            .collect();
        self.reply_to = Some((thread_id, response_record(citation, item_id, subagents)));
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// `withReplyReference`: the reply's reference leads the message unless the text has it.
    pub(crate) fn take_reply(&mut self, text: String) -> String {
        let Some((thread, record)) = self.reply_to.take() else {
            return text;
        };
        if self.selected.as_deref() != Some(thread.as_str()) {
            self.reply_to = Some((thread, record));
            return text;
        }
        let id = record["contextId"].as_str().unwrap_or("").to_owned();
        let text = if text.contains(&format!("/response/{id})")) {
            text
        } else {
            let label = record["label"].as_str().unwrap_or("");
            format!("[{label}](t3-context://v1/response/{id}) {text}")
        };
        self.composer_contexts.push(record);
        text
    }

    /// The "Replying to" banner above the composer, with the cited opening and a way out.
    pub(crate) fn render_reply_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (thread, record) = self.reply_to.as_ref()?;
        if self.selected.as_ref() != Some(thread) {
            return None;
        }
        let c = cx.theme().colors;
        let tokens = crate::web_theme::current(cx);
        let titles: Vec<&str> = record["subagents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["title"].as_str())
            .collect();
        let title = if titles.is_empty() {
            crate::i18n::tc(
                "Replying to",
                "banner title; the message being replied to follows",
            )
        } else {
            crate::i18n::tv("Replying to {titles}", &[("titles", &titles.join(", "))])
        };
        let item_id = record["itemId"].as_str().unwrap_or("").to_owned();
        Some(
            h_flex()
                .gap_2()
                .px_3()
                .pt_2()
                // Tucked under the card below it, past the stack's gap (`Surface` attached).
                .pb(px(24.))
                .mb(px(-24.))
                .rounded_t(px(16.))
                .border_1()
                .border_color(tokens.composer_outline)
                .bg(tokens.composer)
                .text_sm()
                .child(
                    crate::ui::icon("reply")
                        .size(px(12.))
                        .text_color(c.muted_foreground),
                )
                .child(div().flex_none().child(title))
                .child(div().text_color(c.muted_foreground.opacity(0.4)).child("·"))
                .child(
                    div()
                        .id("reply-to-source")
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .cursor_pointer()
                        .text_color(c.muted_foreground)
                        .hover(|s| s.text_color(c.foreground))
                        .child(citation_label(&record["citation"]))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.reveal_item(&item_id, cx)),
                        ),
                )
                .child(
                    crate::ui::xs_button(
                        "reply-to-cancel",
                        "x",
                        true,
                        crate::ui::Variant::GhostMuted,
                        cx,
                    )
                    .tooltip(crate::ui::tooltip(t("Cancel reply")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reply_to = None;
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }

    /// Scrolls the transcript to the row showing `item_id`.
    pub(crate) fn reveal_item(&mut self, item_id: &str, cx: &mut Context<Self>) {
        let index = self.rows().iter().position(|row| match row {
            crate::timeline::Row::Item(item) => item.id == item_id,
            _ => false,
        });
        if let Some(index) = index {
            self.transcript.scroll_to_reveal_item(index);
            cx.notify();
        }
    }

    /// "Fork from this response" (web: `onForkFromRun`): a new thread with this one's history up
    /// to the response's run, opened once the server made it.
    pub(crate) fn fork_from_response(
        &mut self,
        item_id: String,
        source: String,
        run: String,
        cx: &mut Context<Self>,
    ) {
        let (Some(client), Some(env)) = (self.client(), self.env().map(|e| e.key.clone())) else {
            return;
        };
        let title = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
            .map(|t| format!("{} fork", t.title))
            .unwrap_or_else(|| "fork".into());
        let target = uuid::Uuid::new_v4().to_string();
        let command = fork_command(&source, &target, &run, &title);
        self.forking = Some(item_id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = crate::on_tokio(async move {
                client.call("orchestration.dispatchCommand", command).await
            })
            .await;
            this.update(cx, |app, cx| {
                app.forking = None;
                match result {
                    Ok(Ok(_)) => app.select_thread(env, target, cx),
                    Ok(Err(e)) => app.error = Some(e.to_string().into()),
                    Err(_) => app.error = Some(t("Failed to fork this response.").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{citation_label, opening_selector, plain_text, response_record};
    use serde_json::json;

    #[test]
    fn cites_the_opening_like_the_web() {
        let s = opening_selector("  Hello   world.\nNext line  ").unwrap();
        assert_eq!(s["text"], "Hello   world.\nNext line");
        assert_eq!(s["start"], 1);
        assert_eq!(s["end"], 23);
        assert_eq!(s["prefix"], " ");
        assert_eq!(s["suffix"], " ");
        let long = "word ".repeat(100);
        let s = opening_selector(&long).unwrap();
        assert_eq!(s["text"].as_str().unwrap().len(), 300);
        assert_eq!(s["suffix"].as_str().unwrap().len(), 32);
        assert!(opening_selector("   ").is_none());
    }

    #[test]
    fn reads_rendered_text_and_labels_records() {
        assert_eq!(
            plain_text("## Done\n\nI added `clamp`."),
            "Done\nI added clamp.\n"
        );
        let citation = json!({ "messageId": "m1", "text": "x".repeat(70) });
        let record = response_record(
            citation.clone(),
            "i1",
            vec![("t2".into(), "Scout [a]".into())],
        );
        assert_eq!(record["contextId"], "response_m1");
        assert_eq!(record["label"].as_str().unwrap().len(), 64);
        assert_eq!(record["subagents"][0]["title"], "Scout a");
        assert_eq!(citation_label(&citation), format!("{}…", "x".repeat(64)));
    }
}
