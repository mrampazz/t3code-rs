//! Files panel (web: `files/FileBrowserPanel.tsx`, `FileExplorerPane`, `FilePreviewPanel`): the
//! thread's folder as a lazily loaded tree (`projects.listEntries`), and the picked file in a
//! highlighted editor (`projects.readFile`) that saves back with `projects.writeFile`.
//! shortcut: text files only (no image, PDF or office previews), no search, comments or
//! external-change reconciliation; a truncated file opens read-only so a save cannot cut it.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// The highlighter language for a file name.
pub fn language_for(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" | "jsonc" => "json",
        "md" | "mdx" => "markdown",
        "py" => "python",
        "go" => "go",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "swift" => "swift",
        "java" => "java",
        "rb" => "ruby",
        _ => "text",
    }
}

/// Directory entries sorted like a file explorer: folders first, then by name.
pub fn sorted_entries(entries: &[Value]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = entries
        .iter()
        .filter_map(|e| Some((e["path"].as_str()?.to_owned(), e["kind"] == "directory")))
        .collect();
    out.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    out
}

#[derive(Default)]
pub struct FilesState {
    pub open: bool,
    /// Entries per loaded directory ("" is the root).
    children: HashMap<String, Vec<(String, bool)>>,
    expanded: HashSet<String>,
    /// (path, editor, editable) of the open file.
    preview: Option<(String, Entity<EditorState>, bool)>,
    status: Option<SharedString>,
}

impl T3App {
    pub(crate) fn toggle_files(&mut self, cx: &mut Context<Self>) {
        self.files.open = !self.files.open;
        if self.files.open {
            self.files.children.clear();
            self.files.expanded.clear();
            self.load_dir(String::new(), cx);
        }
        cx.notify();
    }

    fn load_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let mut payload = json!({ "cwd": cwd });
        if !dir.is_empty() {
            payload["directoryPath"] = json!(dir);
        }
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.listEntries", payload).await }).await;
            this.update(cx, |app, cx| {
                match result {
                    Ok(Ok(list)) => {
                        let entries = sorted_entries(
                            list["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                        );
                        app.files.children.insert(dir, entries);
                    }
                    Ok(Err(e)) => app.files.status = Some(e.to_string().into()),
                    Err(e) => app.files.status = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let window_handle = window.window_handle();
        self.files.status = Some(t("Loading…").into());
        cx.notify();
        let payload = json!({ "cwd": cwd, "relativePath": path });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.readFile", payload).await }).await;
            let file = match result {
                Ok(Ok(file)) => file,
                Ok(Err(e)) => {
                    this.update(cx, |app, cx| {
                        app.files.status = Some(e.to_string().into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            let editable = file["truncated"] != true;
            let contents = file["contents"].as_str().unwrap_or("").to_owned();
            window_handle
                .update(cx, |_, window, cx| {
                    let editor = cx.new(|cx| {
                        EditorState::new(window, cx)
                            .language(language_for(&path))
                            .line_number(true)
                            .default_value(contents)
                    });
                    this.update(cx, |app, cx| {
                        app.files.status =
                            (!editable).then(|| t("This file is too large to edit here.").into());
                        app.files.preview = Some((path, editor, editable));
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
    }

    fn save_file(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        let Some((path, editor, true)) = self.files.preview.clone() else {
            return;
        };
        let contents = editor.read(cx).value().to_string();
        let payload = json!({ "cwd": cwd, "relativePath": path, "contents": contents });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.writeFile", payload).await }).await;
            this.update(cx, |app, cx| {
                app.files.status = Some(match result {
                    Ok(Ok(_)) => t("Saved").into(),
                    Ok(Err(e)) => e.to_string().into(),
                    Err(e) => e.to_string().into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn tree_rows(&self, dir: &str, depth: usize, out: &mut Vec<(String, bool, usize)>) {
        for (path, is_dir) in self.files.children.get(dir).into_iter().flatten() {
            out.push((path.clone(), *is_dir, depth));
            if *is_dir && self.files.expanded.contains(path) {
                self.tree_rows(path, depth + 1, out);
            }
        }
    }

    pub(crate) fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let mut rows = vec![];
        self.tree_rows("", 0, &mut rows);
        let open_path = self.files.preview.as_ref().map(|(p, ..)| p.clone());
        v_flex()
            .w(px(560.))
            .h_full()
            .border_l_1()
            .border_color(c.border)
            .child(
                v_flex()
                    .id("file-tree")
                    .max_h(relative(0.4))
                    .overflow_y_scroll()
                    .border_b_1()
                    .border_color(c.border)
                    .py_1()
                    .children(
                        rows.into_iter()
                            .enumerate()
                            .map(|(i, (path, is_dir, depth))| {
                                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                                let expanded = self.files.expanded.contains(&path);
                                let selected = open_path.as_deref() == Some(path.as_str());
                                h_flex()
                                    .id(("file-row", i))
                                    .pl(px(8. + 14. * depth as f32))
                                    .pr_2()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(selected, |r| r.bg(c.list_active))
                                    .hover(|r| r.bg(c.list_hover))
                                    .child(if is_dir {
                                        format!("{} {name}", if expanded { "▾" } else { "▸" })
                                    } else {
                                        format!("  {name}")
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if is_dir {
                                            if !this.files.expanded.remove(&path) {
                                                this.files.expanded.insert(path.clone());
                                                if !this.files.children.contains_key(&path) {
                                                    this.load_dir(path.clone(), cx);
                                                }
                                            }
                                            cx.notify();
                                        } else {
                                            this.open_file(path.clone(), window, cx);
                                        }
                                    }))
                            }),
                    ),
            )
            .children(self.files.preview.as_ref().map(|(path, editor, editable)| {
                let editable = *editable;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .text_xs()
                            .child(div().flex_1().truncate().child(path.clone()))
                            .children(
                                self.files
                                    .status
                                    .clone()
                                    .map(|s| div().text_color(c.muted_foreground).child(s)),
                            )
                            .when(editable, |row| {
                                row.child(
                                    Button::new("file-save")
                                        .xsmall()
                                        .outline()
                                        .label(t("Save"))
                                        .on_click(cx.listener(|this, _, _, cx| this.save_file(cx))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(Editor::new(editor).readonly(!editable)),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{language_for, sorted_entries};
    use serde_json::json;

    #[test]
    fn sorts_folders_first_and_picks_languages() {
        let entries = [
            json!({"path": "src/b.rs", "kind": "file"}),
            json!({"path": "src/A", "kind": "directory"}),
            json!({"path": "src/a.rs", "kind": "file"}),
        ];
        let sorted: Vec<_> = sorted_entries(&entries)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(sorted, ["src/A", "src/a.rs", "src/b.rs"]);
        assert_eq!(language_for("main.RS"), "rust");
        assert_eq!(language_for("App.tsx"), "tsx");
        assert_eq!(language_for("Makefile"), "text");
    }
}
