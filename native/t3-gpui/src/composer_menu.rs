//! Composer command menu: `/` commands, `$` skills, `@` files (web: `ComposerCommandMenu.tsx`,
//! `composer-logic.ts` `detectComposerTrigger`, `ChatComposer.tsx` selection).

use std::time::Duration;

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::{T3App, on_tokio};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriggerKind {
    Slash,
    Skill,
    Path,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub query: String,
    /// Byte range of the token to replace.
    pub start: usize,
    pub end: usize,
}

/// `detectComposerTrigger`: the whitespace-delimited token ending at the cursor.
pub fn detect_trigger(text: &str, cursor: usize) -> Option<Trigger> {
    let cursor = cursor.min(text.len());
    let start = text[..cursor]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
    let token = &text[start..cursor];
    let (kind, query) = if let Some(q) = token.strip_prefix('/') {
        // `/usr/bin` is a path being typed, not a command.
        if q.contains('/') {
            return None;
        }
        (TriggerKind::Slash, q)
    } else if let Some(q) = token.strip_prefix('$') {
        // `$5` is money.
        if q.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        (TriggerKind::Skill, q)
    } else {
        (TriggerKind::Path, token.strip_prefix('@')?)
    };
    Some(Trigger {
        kind,
        query: query.to_owned(),
        start,
        end: cursor,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub detail: String,
    /// Text that replaces the trigger token; empty for actions.
    pub insert: String,
    pub action: Option<&'static str>,
    /// A context record (`ThreadContextRecord`) the message carries when the insert stays in it.
    pub context: Option<Value>,
}

/// `fnv1a64` from `shared/composerContextReferences.ts`, over UTF-16 code units like JS.
fn fnv1a64(value: &str) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    let (mut forward, mut reverse) = (0x811c9dc5u32, 0x9dc5811cu32);
    for i in 0..units.len() {
        forward = (forward ^ u32::from(units[i])).wrapping_mul(0x01000193);
        reverse = (reverse ^ u32::from(units[units.len() - 1 - i])).wrapping_mul(0x01000193);
    }
    format!("{forward:08x}{reverse:08x}")
}

/// `toKindScopedComposerContextId`.
pub fn context_id(kind: &str, producer: &str) -> String {
    let raw = format!("{kind}_{producer}");
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if (1..=128).contains(&raw.len()) && raw.chars().all(valid) {
        return raw;
    }
    let mut slug = String::new();
    for c in raw.chars() {
        if valid(c) {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(48).collect();
    format!(
        "{}-{}",
        if slug.is_empty() { "ctx" } else { &slug },
        fnv1a64(&raw)
    )
}

/// `clampContextText`: cuts at `max` UTF-16 units, as the contract counts, with the web's marker.
pub fn clamp_context_text(value: &str, max: usize) -> String {
    const MARKER: &str = "\n… truncated …";
    if value.encode_utf16().count() <= max {
        return value.to_owned();
    }
    let budget = max - MARKER.encode_utf16().count();
    let mut used = 0;
    let end = value
        .char_indices()
        .find(|(_, c)| {
            used += c.len_utf16();
            used > budget
        })
        .map_or(value.len(), |(i, _)| i);
    format!("{}{MARKER}", &value[..end])
}

/// `sanitizeComposerContextLabel`.
pub fn context_label(label: &str, kind: &str) -> String {
    let cleaned: String = label
        .replace(['[', ']', '\\', '\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    if cleaned.is_empty() {
        kind.to_owned()
    } else {
        cleaned
    }
}

/// Threads and models an `@` mention can name (web: `matchComposerThreadItems`,
/// `matchComposerModelItems`). A thread inserts a context link and carries its record; a model
/// inserts `model:<instance>/<slug>`.
/// shortcut: substring matching; the web ranks by recency and fuzzy score.
pub fn mention_items(
    query: &str,
    threads: &[(String, String, String)],
    models: &[(String, String, String)],
) -> Vec<MenuItem> {
    let q = query.to_lowercase();
    let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
    let threads = threads
        .iter()
        .filter(|(_, _, title)| hit(title))
        .take(5)
        .map(|(env, id, title)| {
            let context_id = context_id("thread", id);
            let label = context_label(title, "thread");
            MenuItem {
                label: title.clone(),
                detail: "Thread".into(),
                insert: format!("[{label}](t3-context://v1/thread/{context_id}) "),
                action: None,
                context: Some(
                    serde_json::json!({ "version": 1, "kind": "thread", "contextId": context_id,
                "label": label, "environmentId": env, "threadId": id, "title": label }),
                ),
            }
        });
    let models = models
        .iter()
        .filter(|(label, _, slug)| hit(label) || hit(slug))
        .take(5)
        .map(|(label, instance, slug)| MenuItem {
            label: label.clone(),
            detail: "Model".into(),
            insert: format!("model:{instance}/{slug} "),
            action: None,
            context: None,
        });
    threads.chain(models).collect()
}

/// The message context for the records whose links survived editing, or `None`.
pub fn message_context(text: &str, records: &[Value]) -> Option<Value> {
    let kept: Vec<&Value> = records
        .iter()
        .filter(|r| {
            r["contextId"]
                .as_str()
                .is_some_and(|id| text.contains(&format!("/{id})")))
        })
        .collect();
    (!kept.is_empty()).then(|| serde_json::json!({ "version": 1, "records": kept }))
}

/// Built-in commands the web composer handles itself.
const BUILTINS: [(&str, &str); 2] = [
    ("plan", "Switch to plan mode"),
    ("default", "Switch to default mode"),
];

/// Commands and skills for a `/` or `$` trigger, matching by prefix then substring.
pub fn command_items(trigger: &Trigger, commands: &[Value], skills: &[Value]) -> Vec<MenuItem> {
    let q = trigger.query.to_lowercase();
    let matches = |name: &str| q.is_empty() || name.to_lowercase().contains(&q);
    let mut items = Vec::new();
    if trigger.kind == TriggerKind::Slash {
        for (name, detail) in BUILTINS.into_iter().filter(|(n, _)| matches(n)) {
            items.push(MenuItem {
                label: format!("/{name}"),
                detail: detail.into(),
                insert: String::new(),
                action: Some(name),
                context: None,
            });
        }
        for c in commands {
            let Some(name) = c["name"].as_str().filter(|n| matches(n)) else {
                continue;
            };
            items.push(MenuItem {
                label: format!("/{name}"),
                detail: c["description"].as_str().unwrap_or("").into(),
                insert: format!("/{name} "),
                action: None,
                context: None,
            });
        }
    }
    for s in skills {
        // `userInvocable: false` skills are agent-only; composers hide them.
        if s["enabled"] == false || s["userInvocable"] == false {
            continue;
        }
        let Some(name) = s["name"].as_str().filter(|n| matches(n)) else {
            continue;
        };
        let detail = s["shortDescription"]
            .as_str()
            .or(s["description"].as_str())
            .unwrap_or("");
        items.push(MenuItem {
            label: format!("${name}"),
            detail: detail.into(),
            insert: format!("${name} "),
            action: None,
            context: None,
        });
    }
    items.sort_by_key(|i| !i.label[1..].to_lowercase().starts_with(&q));
    items.truncate(40);
    items
}

/// `serializeComposerFileLink`: `[basename](encoded path)`.
/// shortcut: always adds `./` to encoded relative paths instead of running the web's markdown
/// link parser (`needsDotSlashPrefix`); upgrade if a path ever renders wrong.
pub fn file_link(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let label: String = base
        .chars()
        .flat_map(|c| {
            if "\\[]*_&".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect();
    let mut dest = String::new();
    for byte in path.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-_.!~*';,/:@=+$".contains(&byte);
        if keep {
            dest.push(byte as char);
        } else {
            dest.push_str(&format!("%{byte:02X}"));
        }
    }
    let rooted = path.starts_with('/') || path.starts_with('\\') || path.contains("://");
    let prefix = if dest.contains('%') && !rooted {
        "./"
    } else {
        ""
    };
    format!("[{label}]({prefix}{dest})")
}

/// Search results for an `@` trigger.
pub fn path_items(entries: &[Value]) -> Vec<MenuItem> {
    entries
        .iter()
        .filter_map(|e| {
            let path = e["path"].as_str()?;
            let dir = e["kind"] == "directory";
            Some(MenuItem {
                label: if dir {
                    format!("{path}/")
                } else {
                    path.to_owned()
                },
                detail: if dir { "Folder".into() } else { String::new() },
                insert: format!("{} ", file_link(path)),
                action: None,
                context: None,
            })
        })
        .collect()
}

const PATH_SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

impl T3App {
    /// Recomputes the menu after every composer change.
    pub(crate) fn update_composer_menu(&mut self, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let input = self.composer.read(cx);
            (input.value().to_string(), input.cursor())
        };
        let trigger = detect_trigger(&text, cursor);
        self.menu_index = 0;
        self.menu_generation += 1;
        self.menu_items = match &trigger {
            None => Vec::new(),
            Some(t) if t.kind == TriggerKind::Path => {
                let mentions = self.mention_items(&t.query);
                self.search_paths(t.query.clone(), mentions.clone(), cx);
                mentions
            }
            Some(t) => {
                let (commands, skills) = self.provider_commands(cx);
                command_items(t, &commands, &skills)
            }
        };
        self.menu_trigger = trigger;
        cx.notify();
    }

    /// Slash commands and skills for the current model's provider in the thread's folder.
    fn provider_commands(&mut self, cx: &mut Context<Self>) -> (Vec<Value>, Vec<Value>) {
        let Some(instance) = self
            .current_model()
            .and_then(|m| m["instanceId"].as_str().map(str::to_owned))
        else {
            return Default::default();
        };
        let cwd = self.thread_cwd().or_else(|| {
            let project = self.draft_project.as_ref()?;
            self.shell()
                .projects
                .iter()
                .find(|p| &p.id == project)
                .map(|p| p.workspace_root.clone())
        });
        let Some(provider) = self
            .config()
            .providers
            .iter()
            .find(|p| p.instance_id == instance)
        else {
            return Default::default();
        };
        let snapshot = cwd.as_ref().and_then(|cwd| {
            provider
                .workspace_snapshots
                .iter()
                .find(|s| s["cwd"] == cwd.as_str())
        });
        let pick = |key: &str, fallback: &Vec<Value>| {
            snapshot
                .and_then(|s| s[key].as_array())
                .cloned()
                .unwrap_or_else(|| fallback.clone())
        };
        let lists = (
            pick("slashCommands", &provider.slash_commands),
            pick("skills", &provider.skills),
        );
        if let (None, Some(cwd)) = (snapshot, cwd)
            && self
                .requested_snapshots
                .insert(format!("{instance}\u{0}{cwd}"))
        {
            self.fetch_workspace_snapshot(instance, cwd, cx);
        }
        lists
    }

    fn fetch_workspace_snapshot(&mut self, instance: String, cwd: String, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let payload = json!({ "instanceId": instance, "cwd": cwd });
            if let Ok(Ok(reply)) =
                on_tokio(async move { client.call("server.refreshProviders", payload).await }).await
            {
                this.update(cx, |app, cx| {
                    if let Some(env) = app.env_mut() {
                        env.config.merge_workspace_snapshots(&reply);
                    }
                    if app.menu_trigger.is_some() {
                        app.update_composer_menu(cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Other threads in this environment and the picker's models, for an `@` mention.
    fn mention_items(&self, query: &str) -> Vec<MenuItem> {
        let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
        let threads: Vec<(String, String, String)> = self
            .shell()
            .threads
            .iter()
            .filter(|t| t.is_listed() && self.selected.as_deref() != Some(t.id.as_str()))
            .map(|t| (env.clone(), t.id.clone(), t.title.clone()))
            .collect();
        let models: Vec<(String, String, String)> = self
            .config()
            .model_options()
            .into_iter()
            .filter_map(|(label, sel)| {
                Some((
                    label,
                    sel["instanceId"].as_str()?.to_owned(),
                    sel["model"].as_str()?.to_owned(),
                ))
            })
            .collect();
        mention_items(query, &threads, &models)
    }

    fn search_paths(&mut self, query: String, mentions: Vec<MenuItem>, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let generation = self.menu_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PATH_SEARCH_DEBOUNCE).await;
            if this
                .update(cx, |app, _| app.menu_generation != generation)
                .unwrap_or(true)
            {
                return;
            }
            let payload = json!({ "cwd": cwd, "query": query, "limit": 20 });
            let result =
                on_tokio(async move { client.call("projects.searchEntries", payload).await }).await;
            if let Ok(Ok(result)) = result {
                this.update(cx, |app, cx| {
                    if app.menu_generation == generation {
                        app.menu_items = mentions
                            .into_iter()
                            .chain(path_items(
                                result["entries"]
                                    .as_array()
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[]),
                            ))
                            .collect();
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Menu keys: Up/Down move, Enter/Tab pick, Escape closes. Returns whether it handled the key.
    pub(crate) fn composer_menu_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.menu_trigger.is_none() || self.menu_items.is_empty() {
            return false;
        }
        let n = self.menu_items.len();
        match key {
            "up" => self.menu_index = (self.menu_index + n - 1) % n,
            "down" => self.menu_index = (self.menu_index + 1) % n,
            "enter" | "tab" => self.pick_menu_item(self.menu_index, window, cx),
            "escape" => {
                self.menu_trigger = None;
                self.menu_items.clear();
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn pick_menu_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(trigger), Some(item)) = (
            self.menu_trigger.take(),
            self.menu_items.get(index).cloned(),
        ) else {
            return;
        };
        self.menu_items.clear();
        let range = trigger.start..trigger.end;
        self.composer.update(cx, |input, cx| {
            input.set_selected_range(range, cx);
            input.replace(item.insert.clone(), window, cx);
        });
        self.composer_contexts.extend(item.context);
        match item.action {
            Some("plan") => self.set_interaction_mode("plan", cx),
            Some("default") => self.set_interaction_mode("default", cx),
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn render_composer_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        v_flex()
            .id("composer-menu")
            .max_h(px(260.))
            .overflow_y_scroll()
            .rounded_md()
            .border_1()
            .border_color(c.border)
            .bg(c.popover)
            .py_1()
            .children(self.menu_items.iter().enumerate().map(|(i, item)| {
                h_flex()
                    .id(("composer-menu-item", i))
                    .px_3()
                    .py_1()
                    .gap_3()
                    .text_sm()
                    .cursor_pointer()
                    .when(i == self.menu_index, |r| r.bg(c.list_active))
                    .hover(|r| r.bg(c.list_hover))
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .text_color(c.muted_foreground)
                            .child(item.detail.clone()),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick_menu_item(i, window, cx)),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TriggerKind, clamp_context_text, command_items, context_id, detect_trigger, file_link,
        mention_items, message_context, path_items,
    };

    #[test]
    fn clamps_context_text_like_the_web() {
        assert_eq!(clamp_context_text("short", 100), "short");
        let clamped = clamp_context_text(&"é".repeat(50), 20);
        assert_eq!(clamped.encode_utf16().count(), 20);
        assert!(clamped.ends_with("… truncated …"));
    }

    #[test]
    fn mentions_threads_and_models() {
        let threads = [(
            "e".to_owned(),
            "thread:abc".to_owned(),
            "Fix [login]".to_owned(),
        )];
        let models = [("Opus".to_owned(), "claude".to_owned(), "opus".to_owned())];
        let items = mention_items("", &threads, &models);
        let id = context_id("thread", "thread:abc");
        assert_eq!(
            id, "thread_thread-abc-56f5bc74ea7a077f",
            "matches the web's toComposerContextId"
        );
        assert_eq!(
            items[0].insert,
            format!("[Fix login](t3-context://v1/thread/{id}) ")
        );
        assert_eq!(items[1].insert, "model:claude/opus ");
        assert_eq!(mention_items("opu", &threads, &models).len(), 1);
        let record = items[0].context.clone().unwrap();
        assert!(message_context(&items[0].insert, std::slice::from_ref(&record)).is_some());
        assert!(message_context("edited away", &[record]).is_none());
        assert_eq!(context_id("thread", "abc"), "thread_abc");
    }
    use serde_json::json;

    #[test]
    fn detects_triggers_like_the_web() {
        let t = detect_trigger("run /rev", 8).unwrap();
        assert_eq!(
            (t.kind, t.query.as_str(), t.start, t.end),
            (TriggerKind::Slash, "rev", 4, 8)
        );
        assert_eq!(
            detect_trigger("ls /usr/bin", 11),
            None,
            "a path is not a command"
        );
        assert_eq!(detect_trigger("costs $5", 8), None, "money is not a skill");
        assert_eq!(
            detect_trigger("use $pdf", 8).unwrap().kind,
            TriggerKind::Skill
        );
        assert_eq!(detect_trigger("see @src/ma", 11).unwrap().query, "src/ma");
        assert_eq!(detect_trigger("plain words", 11), None);
        assert_eq!(
            detect_trigger("é @x", "é @x".len()).unwrap().start,
            3,
            "byte offsets"
        );
    }

    #[test]
    fn lists_commands_and_user_invocable_skills() {
        let trigger = detect_trigger("/re", 3).unwrap();
        let commands = vec![
            json!({"name": "review", "description": "Review"}),
            json!({"name": "compact"}),
        ];
        let skills = vec![
            json!({"name": "release", "enabled": true}),
            json!({"name": "remote", "enabled": true, "userInvocable": false}),
            json!({"name": "rest", "enabled": false}),
        ];
        let labels: Vec<String> = command_items(&trigger, &commands, &skills)
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert_eq!(labels, ["/review", "$release"]);
        let all = command_items(&detect_trigger("/", 1).unwrap(), &commands, &[]);
        assert_eq!(all[0].action, Some("plan"));
        assert_eq!(all[2].insert, "/review ");
    }

    #[test]
    fn serializes_file_links() {
        assert_eq!(file_link("src/main.rs"), "[main.rs](src/main.rs)");
        assert_eq!(
            file_link("docs/my file.md"),
            "[my file.md](./docs/my%20file.md)"
        );
        assert_eq!(
            file_link("pkg/__init__.py"),
            "[\\_\\_init\\_\\_.py](pkg/__init__.py)"
        );
        let items = path_items(&[
            json!({"path": "src", "kind": "directory"}),
            json!({"path": "a.rs", "kind": "file"}),
        ]);
        assert_eq!(
            (items[0].label.as_str(), items[1].insert.as_str()),
            ("src/", "[a.rs](a.rs) ")
        );
    }
}
