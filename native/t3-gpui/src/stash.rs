//! Prompt stash (web: `promptStashStore.ts`, `useComposerStash.ts`, `ComposerStashMenu.tsx`):
//! `composer.stash` sets the draft aside, newest first, kept on disk across restarts; restoring
//! appends an entry to whatever the composer holds.
//! shortcut: text only; attachments and context chips stay in the composer when stashing.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::T3App;
use crate::i18n::t;

pub const MAX_ENTRIES: usize = 20;
const SNIPPET_MAX_CHARS: usize = 90;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Entry {
    pub id: String,
    pub prompt: String,
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/t3-gpui/stash.json"))
}

pub fn load() -> Vec<Entry> {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(entries: &[Entry]) {
    let Some(path) = path() else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, serde_json::to_vec(entries).unwrap());
}

/// Adds `prompt` first; returns whether the oldest entry made room for it.
pub fn push(entries: &mut Vec<Entry>, prompt: String) -> bool {
    entries.insert(
        0,
        Entry {
            id: uuid::Uuid::new_v4().to_string(),
            prompt,
        },
    );
    let evicted = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    evicted
}

/// The composer text after restoring `prompt` into `current`.
pub fn restored(current: &str, prompt: &str) -> String {
    if current.trim().is_empty() {
        prompt.to_owned()
    } else {
        format!("{}\n\n{prompt}", current.trim_end())
    }
}

/// One line of a stashed prompt for the menu (`stashEntrySnippet`).
pub fn snippet(prompt: &str) -> String {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return t("(empty)");
    }
    match flat.char_indices().nth(SNIPPET_MAX_CHARS) {
        Some((end, _)) => format!("{}…", &flat[..end]),
        None => flat,
    }
}

impl T3App {
    /// `composer.stash`: stashes the draft, or with an empty composer restores the only entry or
    /// toggles the list.
    pub(crate) fn stash_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.composer.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            match self.stash.as_slice() {
                [] => {}
                [only] => {
                    let id = only.id.clone();
                    self.restore_stash(&id, window, cx);
                }
                _ => self.stash_open = !self.stash_open,
            }
            return cx.notify();
        }
        if push(&mut self.stash, prompt) {
            self.error = Some(t("Oldest stashed prompt discarded").into());
        }
        save(&self.stash);
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn restore_stash(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.stash.iter().position(|e| e.id == id) else {
            return;
        };
        let entry = self.stash.remove(at);
        save(&self.stash);
        self.stash_open = false;
        let text = restored(&self.composer.read(cx).value(), &entry.prompt);
        self.composer.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn delete_stash(&mut self, id: &str, cx: &mut Context<Self>) {
        self.stash.retain(|e| e.id != id);
        save(&self.stash);
        self.stash_open &= !self.stash.is_empty();
        cx.notify();
    }

    /// The stash badge, and the list of entries while it is open.
    pub(crate) fn render_stash(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.stash.is_empty() {
            return div().into_any_element();
        }
        let c = cx.theme().colors;
        v_flex()
            .gap_1()
            .child(
                h_flex().child(
                    Button::new("stash-badge")
                        .xsmall()
                        .ghost()
                        .label(format!("{} · {}", t("Stash"), self.stash.len()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stash_open = !this.stash_open;
                            cx.notify();
                        })),
                ),
            )
            .when(self.stash_open, |col| {
                col.child(
                    v_flex()
                        .gap_1()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(c.border)
                        .children(self.stash.iter().enumerate().map(|(i, entry)| {
                            let (restore, delete) = (entry.id.clone(), entry.id.clone());
                            h_flex()
                                .id(("stash-entry", i))
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(c.secondary))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.restore_stash(&restore, window, cx)
                                }))
                                .child(div().flex_1().truncate().child(snippet(&entry.prompt)))
                                .child(
                                    Button::new(("stash-delete", i))
                                        .xsmall()
                                        .ghost()
                                        .label("✕")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.delete_stash(&delete, cx)
                                        })),
                                )
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ENTRIES, push, restored, snippet};

    #[test]
    fn keeps_the_newest_entries() {
        let mut entries = Vec::new();
        for n in 0..MAX_ENTRIES {
            assert!(!push(&mut entries, n.to_string()));
        }
        assert!(push(&mut entries, "new".into()), "the oldest makes room");
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].prompt, "new");
        assert_eq!(entries.last().unwrap().prompt, "1");
    }

    #[test]
    fn restores_after_the_draft() {
        assert_eq!(restored("  ", "b"), "b");
        assert_eq!(restored("a \n", "b"), "a\n\nb");
    }

    #[test]
    fn snippets_are_one_short_line() {
        assert_eq!(snippet("a\n\n  b"), "a b");
        let long = "x".repeat(100);
        assert_eq!(snippet(&long).chars().count(), 91);
    }
}
