//! Files panel (web: `files/FileBrowserPanel.tsx`, `FileExplorerPane`, `FilePreviewPanel`): the
//! thread's folder as a lazily loaded tree (`projects.listEntries`), and the picked file in a
//! highlighted editor (`projects.readFile`) that saves back with `projects.writeFile`.
//! Search finds files by name (`projects.searchEntries`) or by content (`projects.searchContents`);
//! images preview through a signed asset URL (`workspace-file`).
//! shortcut: no PDF, office, audio or font previews, comments or external-change
//! reconciliation; a truncated file opens read-only so a save cannot cut it.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, Input, Position};
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

/// Images gpui can decode (web: `WORKSPACE_IMAGE_PREVIEW_EXTENSIONS`, minus AVIF and HEIC).
pub fn is_image(path: &str) -> bool {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tif" | "tiff"
    )
}

/// One search result: the file, the 1-based line of a content match, and what to show beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub path: String,
    pub line: Option<u32>,
    pub detail: String,
}

/// Hits from a `ProjectSearchEntriesResult` or `ProjectSearchContentsResult`.
pub fn search_hits(result: &Value) -> Vec<SearchHit> {
    let entries = result["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| SearchHit {
            path: e["path"].as_str().unwrap_or("").to_owned(),
            line: None,
            detail: String::new(),
        });
    let matches = result["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| SearchHit {
            path: m["path"].as_str().unwrap_or("").to_owned(),
            line: m["lineNumber"].as_u64().map(|n| n as u32),
            detail: m["lineContent"].as_str().unwrap_or("").trim().to_owned(),
        });
    entries
        .chain(matches)
        .filter(|h| !h.path.is_empty())
        .collect()
}

/// Search waits for the typing to pause.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

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
    /// (path, signed URL once minted) of an open image.
    image: Option<(String, Option<String>)>,
    status: Option<SharedString>,
    /// Search file contents instead of names.
    contents_mode: bool,
    results: Vec<SearchHit>,
    search_generation: u64,
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

    fn open_file(
        &mut self,
        path: String,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        if is_image(&path) {
            return self.open_image(path, cwd, cx);
        }
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
                    if let Some(line) = line {
                        editor.update(cx, |e, cx| {
                            e.set_cursor_position(
                                Position::new(line.saturating_sub(1), 0),
                                window,
                                cx,
                            )
                        });
                    }
                    this.update(cx, |app, cx| {
                        app.files.status =
                            (!editable).then(|| t("This file is too large to edit here.").into());
                        app.files.image = None;
                        app.files.preview = Some((path, editor, editable));
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
    }

    fn open_image(&mut self, path: String, cwd: String, cx: &mut Context<Self>) {
        let (Some(env), Some(thread_id)) = (self.env(), self.selected.clone()) else {
            return;
        };
        let (Some(client), base) = (
            env.client.clone(),
            env.session.base_url.trim_end_matches('/').to_owned(),
        ) else {
            return;
        };
        self.files.preview = None;
        self.files.status = None;
        self.files.image = Some((path.clone(), None));
        cx.notify();
        let absolute = format!("{}/{path}", cwd.trim_end_matches('/'));
        let resource = json!({ "_tag": "workspace-file", "threadId": thread_id, "path": absolute });
        cx.spawn(async move |this, cx| {
            let minted = on_tokio(async move {
                client
                    .call("assets.createUrl", json!({ "resource": resource }))
                    .await
            })
            .await;
            this.update(cx, |app, cx| {
                if app.files.image.as_ref().map(|(p, _)| p) != Some(&path) {
                    return;
                }
                match minted {
                    Ok(Ok(r)) => {
                        let url = r["relativeUrl"].as_str().map(|u| format!("{base}{u}"));
                        app.files.image = Some((path, url));
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

    /// Opens the panel with its search box focused, searching names or contents
    /// (`filePicker.toggle`, `projectSearch.toggle`).
    pub(crate) fn focus_file_search(
        &mut self,
        contents: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.files.open {
            self.toggle_files(cx);
        }
        if self.files.contents_mode != contents {
            self.files.contents_mode = contents;
            self.search_files(cx);
        }
        self.files_search
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Runs the search box's query once typing pauses; an empty query shows the tree again.
    pub(crate) fn search_files(&mut self, cx: &mut Context<Self>) {
        self.files.search_generation += 1;
        let generation = self.files.search_generation;
        let query = self.files_search.read(cx).value().to_string();
        self.files.results.clear();
        cx.notify();
        let (Some(client), Some(cwd)) = (self.client(), self.thread_cwd()) else {
            return;
        };
        if query.trim().is_empty() {
            return;
        }
        let (method, payload) = if self.files.contents_mode {
            (
                "projects.searchContents",
                json!({ "cwd": cwd, "query": query, "limit": 200,
                "caseSensitive": false, "wholeWord": false, "useRegex": false }),
            )
        } else {
            (
                "projects.searchEntries",
                json!({ "cwd": cwd, "query": query.trim(), "limit": 100, "kind": "file" }),
            )
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let current = |app: &mut T3App| app.files.search_generation == generation;
            if !this.update(cx, |app, _| current(app)).unwrap_or(false) {
                return;
            }
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            this.update(cx, |app, cx| {
                if !current(app) {
                    return;
                }
                match result {
                    Ok(Ok(r)) => {
                        app.files.results = search_hits(&r);
                        app.files.status =
                            app.files.results.is_empty().then(|| t("No results").into());
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
        let searching = !self.files_search.read(cx).value().trim().is_empty();
        let contents = self.files.contents_mode;
        v_flex()
            .w(px(560.))
            .h_full()
            .border_l_1()
            .border_color(c.border)
            .child(
                h_flex()
                    .p_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(c.border)
                    .child(div().flex_1().child(Input::new(&self.files_search).small()))
                    .child(
                        Button::new("files-search-mode")
                            .xsmall()
                            .ghost()
                            .label(if contents { t("Contents") } else { t("Names") })
                            .tooltip(t("Search file names or contents"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files.contents_mode = !this.files.contents_mode;
                                this.search_files(cx);
                            })),
                    ),
            )
            .when(searching, |col| {
                col.child(
                    v_flex()
                        .id("file-results")
                        .max_h(relative(0.4))
                        .overflow_y_scroll()
                        .border_b_1()
                        .border_color(c.border)
                        .py_1()
                        .children(
                            self.files
                                .status
                                .clone()
                                .filter(|_| self.files.results.is_empty())
                                .map(|s| {
                                    div()
                                        .px_2()
                                        .text_xs()
                                        .text_color(c.muted_foreground)
                                        .child(s)
                                }),
                        )
                        .children(self.files.results.iter().enumerate().map(|(i, hit)| {
                            let (path, line) = (hit.path.clone(), hit.line);
                            h_flex()
                                .id(("file-hit", i))
                                .px_2()
                                .gap_2()
                                .text_xs()
                                .cursor_pointer()
                                .hover(|r| r.bg(c.list_hover))
                                .child(div().flex_none().max_w(px(260.)).truncate().child(
                                    match line {
                                        Some(n) => format!("{}:{n}", hit.path),
                                        None => hit.path.clone(),
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(c.muted_foreground)
                                        .child(hit.detail.clone()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_file(path.clone(), line, window, cx)
                                }))
                        })),
                )
            })
            .when(!searching, |col| {
                col.child(
                    v_flex()
                        .id("file-tree")
                        .max_h(relative(0.4))
                        .overflow_y_scroll()
                        .border_b_1()
                        .border_color(c.border)
                        .py_1()
                        .children(rows.into_iter().enumerate().map(
                            |(i, (path, is_dir, depth))| {
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
                                            this.open_file(path.clone(), None, window, cx);
                                        }
                                    }))
                            },
                        )),
                )
            })
            .children(self.files.image.as_ref().map(|(path, url)| {
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
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_2()
                            .flex()
                            .justify_center()
                            .children(url.clone().map(|url| {
                                let source = SharedString::from(url.clone());
                                img(source)
                                    .id("file-image")
                                    .max_w_full()
                                    .max_h_full()
                                    .object_fit(ObjectFit::Contain)
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        crate::media::open_expanded(
                                            SharedString::from(url.clone()).into(),
                                            window,
                                            cx,
                                        )
                                    })
                            })),
                    )
            }))
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
    use super::{SearchHit, is_image, language_for, search_hits, sorted_entries};
    use serde_json::json;

    #[test]
    fn reads_name_and_content_hits() {
        let names = search_hits(
            &json!({"entries": [{"path": "src/a.rs", "kind": "file"}], "truncated": false}),
        );
        assert_eq!(
            names,
            [SearchHit {
                path: "src/a.rs".into(),
                line: None,
                detail: String::new()
            }]
        );
        let lines = search_hits(&json!({"matches": [{"path": "b.rs", "lineNumber": 7,
            "lineContent": "  let x = 1;", "matchRanges": []}], "truncated": false}));
        assert_eq!(
            lines,
            [SearchHit {
                path: "b.rs".into(),
                line: Some(7),
                detail: "let x = 1;".into()
            }]
        );
        assert!(is_image("docs/Logo.PNG") && !is_image("a.rs") && !is_image("x.heic"));
    }

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
