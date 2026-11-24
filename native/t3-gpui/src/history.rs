//! Composer prompt history, ported from `apps/web/src/components/chat/composerPromptHistory.ts`.
//! shortcut: legacy trailing context blocks and mention chips are not stripped or restored;
//! port `recallableComposerPrompt` fully when composer context chips land.

use crate::model::TurnItem;

const ULTRATHINK_PREFIX: &str = "Ultrathink:\n";

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub prompt: String,
}

/// Where the composer is in the history: the recalled entry and the text it put there.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub entry_id: String,
    pub recalled: String,
}

/// What the user typed in this thread, oldest first, repeats collapsed to the latest.
pub fn entries<'a>(items: impl Iterator<Item = &'a TurnItem>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for item in items.filter(|i| i.kind == "user_message") {
        // Automations and other agents send user-role messages; recall only the user's own.
        let raw = &item.raw;
        let attributed = raw["createdBy"].as_str().is_some_and(|c| c != "user")
            || !raw["scheduledTaskId"].is_null()
            || !raw["senderThreadId"].is_null()
            || !raw["linkedSender"].is_null();
        if attributed {
            continue;
        }
        let text = item.str("text").trim();
        let prompt = text
            .strip_prefix(ULTRATHINK_PREFIX)
            .unwrap_or(text)
            .trim()
            .to_string();
        if prompt.is_empty() {
            continue;
        }
        let entry = Entry {
            id: item.id.clone(),
            prompt,
        };
        match out.last_mut() {
            Some(last) if last.prompt == entry.prompt => *last = entry,
            _ => out.push(entry),
        }
    }
    out
}

/// `stepComposerPromptHistory`: `Some((position, text))` to show, `None` to leave the key alone.
pub fn step(
    backward: bool,
    entries: &[Entry],
    position: Option<&Position>,
    current: &str,
) -> Option<(Option<Position>, String)> {
    let active = position.filter(|p| p.recalled == current).and_then(|p| {
        entries
            .iter()
            .position(|e| e.id == p.entry_id)
            .or_else(|| entries.iter().rposition(|e| e.prompt == p.recalled))
    });
    let recall = |e: &Entry| {
        Some((
            Some(Position {
                entry_id: e.id.clone(),
                recalled: e.prompt.clone(),
            }),
            e.prompt.clone(),
        ))
    };
    if backward {
        if active.is_none() && !current.is_empty() {
            return None;
        }
        let index = match active {
            None => entries.len().checked_sub(1)?,
            Some(i) => i.checked_sub(1)?,
        };
        return recall(&entries[index]);
    }
    let i = active?;
    match entries.get(i + 1) {
        Some(e) => recall(e),
        None => Some((None, String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(id: &str, text: &str, extra: serde_json::Value) -> TurnItem {
        let mut v = json!({"id": id, "ordinal": 1, "type": "user_message", "text": text, "createdBy": "user"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        TurnItem::from_value(&v).unwrap()
    }

    #[test]
    fn builds_entries_from_the_users_own_messages() {
        let items = [
            user("a", "first", json!({})),
            user("b", "Ultrathink:\nthink hard", json!({})),
            user("c", "bot", json!({"scheduledTaskId": "t"})),
            user("d", "think hard", json!({})),
            user("e", "agent", json!({"createdBy": "agent"})),
        ];
        let e = entries(items.iter());
        assert_eq!(
            e.iter()
                .map(|e| (e.id.as_str(), e.prompt.as_str()))
                .collect::<Vec<_>>(),
            [("a", "first"), ("d", "think hard")]
        );
    }

    #[test]
    fn steps_like_the_web_composer() {
        let e = vec![
            Entry {
                id: "a".into(),
                prompt: "one".into(),
            },
            Entry {
                id: "b".into(),
                prompt: "two".into(),
            },
        ];
        assert_eq!(
            step(true, &e, None, "typing"),
            None,
            "a draft is never replaced"
        );
        let (pos, text) = step(true, &e, None, "").unwrap();
        assert_eq!(text, "two");
        let (pos, text) = step(true, &e, pos.as_ref(), &text).unwrap();
        assert_eq!(text, "one");
        assert_eq!(
            step(true, &e, pos.as_ref(), "one"),
            None,
            "oldest stays put"
        );
        let (pos, text) = step(false, &e, pos.as_ref(), "one").unwrap();
        assert_eq!(text, "two");
        assert_eq!(
            step(false, &e, pos.as_ref(), &text),
            Some((None, String::new())),
            "past newest clears"
        );
        assert_eq!(
            step(false, &e, pos.as_ref(), "edited"),
            None,
            "edits leave history"
        );
    }
}
