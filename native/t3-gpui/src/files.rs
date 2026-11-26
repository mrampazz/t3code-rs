//! Files panel (web: `files/FileBrowserPanel.tsx`, `FileExplorerPane`, `FilePreviewPanel`): the
//! thread's folder as a lazily loaded tree (`projects.listEntries`), and the picked file in a
//! highlighted editor (`projects.readFile`) that saves back with `projects.writeFile`.
//! Search finds files by name (`projects.searchEntries`) or by content (`projects.searchContents`);
//! images preview through a signed asset URL (`workspace-file`).
//! shortcut: no PDF, office, audio or font previews, comments or external-change
//! reconciliation; a truncated file opens read-only so a save cannot cut it.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{Editor, EditorState, Input, Position};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
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

/// A Lucide icon for a file by its extension.
/// shortcut: generic Lucide glyphs, not the web's per-language colored file icons.
pub(crate) fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase();
    match ext.as_str() {
        _ if is_image(name) => "file-image",
        "json" | "jsonc" | "toml" | "yml" | "yaml" => "file-braces",
        "md" | "mdx" | "txt" => "file-text",
        "" => "file",
        _ => "file-code",
    }
}

/// Search waits for the typing to pause.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// One listed entry: its path, whether it is a folder, and whether git ignores it.
pub type Entry = (String, bool, bool);

/// Directory entries sorted like a file explorer: folders first, then by name.
pub fn sorted_entries(entries: &[Value]) -> Vec<Entry> {
    let mut out: Vec<Entry> = entries
        .iter()
        .filter_map(|e| {
            Some((
                e["path"].as_str()?.to_owned(),
                e["kind"] == "directory",
                e["ignored"] == true,
            ))
        })
        .collect();
    out.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    out
}

#[derive(Default)]
pub struct FilesState {
    /// Entries per loaded directory ("" is the root).
    children: HashMap<String, Vec<Entry>>,
    expanded: HashSet<String>,
    /// (editor, editable) per open file tab, so switching tabs keeps unsaved edits.
    editors: HashMap<String, (Entity<EditorState>, bool)>,
    /// Signed URL, once minted, per open image tab.
    images: HashMap<String, Option<String>>,
    status: Option<SharedString>,
    /// Search file contents instead of names.
    contents_mode: bool,
    results: Vec<SearchHit>,
    search_generation: u64,
}

impl FilesState {
    /// Forgets the tree and open files, for another thread's folder.
    pub fn reset(&mut self) {
        *self = FilesState {
            contents_mode: self.contents_mode,
            ..Default::default()
        };
    }

    /// Whether a file tab's content is loaded (or loading).
    pub fn has(&self, path: &str) -> bool {
        self.editors.contains_key(path) || self.images.contains_key(path)
    }

    /// Drops a closed file tab's editor.
    pub fn forget(&mut self, path: &str) {
        self.editors.remove(path);
        self.images.remove(path);
    }
}

impl T3App {
    /// Loads the explorer's root once for the thread's folder.
    pub(crate) fn ensure_file_tree(&mut self, cx: &mut Context<Self>) {
        if !self.files.children.contains_key("") {
            self.load_dir(String::new(), cx);
        }
    }

    /// The file tab on screen, if any.
    fn active_file(&self) -> Option<&str> {
        match self.panel.shown() {
            Some(crate::panel::Surface::File(path)) => Some(path),
            _ => None,
        }
    }

    /// Opens a file as a tab (web: `openFile`), jumping to `line` when given.
    fn show_file(
        &mut self,
        path: String,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (self.files.editors.get(&path), line) {
            (Some((editor, _)), Some(line)) => editor.update(cx, |e, cx| {
                e.set_cursor_position(Position::new(line.saturating_sub(1), 0), window, cx)
            }),
            (None, _) if !self.files.has(&path) => self.open_file(path.clone(), line, window, cx),
            _ => {}
        }
        self.open_surface(crate::panel::Surface::File(path), cx);
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

    /// Reads a file into an editor for its tab.
    pub(crate) fn open_file(
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
                        app.files.editors.insert(path, (editor, editable));
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
        self.files.status = None;
        self.files.images.insert(path.clone(), None);
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
                if !app.files.images.contains_key(&path) {
                    return;
                }
                match minted {
                    Ok(Ok(r)) => {
                        let url = r["relativeUrl"].as_str().map(|u| format!("{base}{u}"));
                        app.files.images.insert(path, url);
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
        if !matches!(
            self.panel.shown(),
            Some(crate::panel::Surface::Files | crate::panel::Surface::File(_))
        ) {
            self.open_surface(crate::panel::Surface::Files, cx);
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
        let Some(path) = self.active_file().map(str::to_owned) else {
            return;
        };
        let Some((editor, true)) = self.files.editors.get(&path).cloned() else {
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

    fn tree_rows(&self, dir: &str, depth: usize, out: &mut Vec<(Entry, usize)>) {
        for entry in self.files.children.get(dir).into_iter().flatten() {
            out.push((entry.clone(), depth));
            let (path, is_dir, _) = entry;
            if *is_dir && self.files.expanded.contains(path) {
                self.tree_rows(path, depth + 1, out);
            }
        }
    }

    /// The tree, virtualized: only the rows in view are built, so a huge folder stays smooth.
    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = vec![];
        self.tree_rows("", 0, &mut rows);
        uniform_list(
            "file-tree",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let c = cx.theme().colors;
                let open = this.active_file().map(str::to_owned);
                let mut rows = vec![];
                this.tree_rows("", 0, &mut rows);
                rows[range.clone()]
                    .iter()
                    .cloned()
                    .zip(range)
                    .map(|(((path, is_dir, ignored), depth), i)| {
                        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                        let expanded = this.files.expanded.contains(&path);
                        let selected = open.as_deref() == Some(path.as_str());
                        h_flex()
                            .id(("file-row", i))
                            .w_full()
                            .h(px(24.))
                            .px_2()
                            .gap(px(6.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .when(ignored, |r| r.text_color(c.muted_foreground))
                            .when(selected, |r| r.bg(c.accent))
                            .hover(|r| r.bg(c.list_hover))
                            .children((0..depth).map(|_| {
                                div()
                                    .w(px(16.))
                                    .h_full()
                                    .flex_none()
                                    .flex()
                                    .justify_center()
                                    .child(div().w_px().h_full().bg(c.border))
                            }))
                            .child(
                                crate::ui::icon(match (is_dir, expanded) {
                                    (true, true) => "chevron-down",
                                    (true, false) => "chevron-right",
                                    (false, _) => file_icon(&name),
                                })
                                .size(px(16.))
                                .flex_none()
                                .text_color(c.muted_foreground),
                            )
                            .child(div().truncate().child(name))
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
                                    this.show_file(path.clone(), None, window, cx);
                                }
                            }))
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .flex_1()
        .px_3()
        .into_any_element()
    }

    /// Refresh, search and collapse over the tree, or the matches while searching.
    fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let searching = !self.files_search.read(cx).value().trim().is_empty();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(36.))
                    .px_3()
                    .gap_1()
                    .child(
                        crate::ui::icon_button("files-refresh", "refresh-cw", cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.files.children.clear();
                                let open: Vec<String> =
                                    this.files.expanded.iter().cloned().collect();
                                this.load_dir(String::new(), cx);
                                for dir in open {
                                    this.load_dir(dir, cx);
                                }
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.files_search).appearance(false)),
                    )
                    .child(
                        crate::ui::icon_button(
                            "files-search-mode",
                            if self.files.contents_mode {
                                "text-search"
                            } else {
                                "file-search"
                            },
                            cx,
                        )
                        .tooltip(|window, cx| {
                            Tooltip::new(t("Search file names or contents")).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.files.contents_mode = !this.files.contents_mode;
                            this.search_files(cx);
                        })),
                    )
                    .child(
                        crate::ui::icon_button("files-collapse", "chevrons-down-up", cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.files.expanded.clear();
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .when(searching, |col| {
                col.child(
                    v_flex()
                        .id("file-results")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_3()
                        .children(
                            self.files
                                .status
                                .clone()
                                .filter(|_| self.files.results.is_empty())
                                .map(|s| {
                                    div()
                                        .px_2()
                                        .text_sm()
                                        .text_color(c.muted_foreground)
                                        .child(s)
                                }),
                        )
                        .children(self.files.results.iter().enumerate().map(|(i, hit)| {
                            let (path, line) = (hit.path.clone(), hit.line);
                            let name = hit.path.rsplit('/').next().unwrap_or(&hit.path).to_owned();
                            h_flex()
                                .id(("file-hit", i))
                                .h(px(24.))
                                .px_2()
                                .gap(px(6.))
                                .rounded_md()
                                .text_sm()
                                .cursor_pointer()
                                .hover(|r| r.bg(c.list_hover))
                                .child(
                                    crate::ui::icon(file_icon(&name))
                                        .size(px(16.))
                                        .flex_none()
                                        .text_color(c.muted_foreground),
                                )
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
                                    this.show_file(path.clone(), line, window, cx)
                                }))
                        })),
                )
            })
            .when(!searching, |col| col.child(self.render_tree(cx)))
    }

    /// The open file or image: a breadcrumb with its actions, then the editor or picture.
    fn render_open_file(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors;
        let path = self.active_file()?.to_owned();
        let editor = self.files.editors.get(&path).cloned();
        let editable = editor.as_ref().is_some_and(|(_, e)| *e);
        let project = self
            .thread_cwd()
            .and_then(|cwd| cwd.rsplit('/').next().map(str::to_owned))
            .unwrap_or_default();
        let close = path.clone();
        let crumb = h_flex()
            .h(px(40.))
            .px_3()
            .gap_1()
            .text_sm()
            .child(div().text_color(c.muted_foreground).child(project))
            .children(path.split('/').map(|part| {
                h_flex()
                    .gap_1()
                    .child(
                        crate::ui::icon("chevron-right")
                            .size(px(14.))
                            .text_color(c.muted_foreground),
                    )
                    .child(part.to_owned())
            }))
            .child(div().flex_1())
            .children(
                self.files
                    .status
                    .clone()
                    .map(|s| div().text_xs().text_color(c.muted_foreground).child(s)),
            )
            .when(editable, |row| {
                row.child(
                    crate::ui::icon_button("file-save", "save", cx)
                        .tooltip(|window, cx| Tooltip::new(t("Save")).build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.save_file(cx))),
                )
            })
            .child({
                let copy = path.clone();
                crate::ui::icon_button("file-copy-path", "copy", cx)
                    .tooltip(|window, cx| Tooltip::new(t("Copy path")).build(window, cx))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    })
            })
            .child(
                crate::ui::icon_button("file-close", "x", cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.files.status = None;
                        this.close_file_tab(&close, cx);
                    },
                )),
            );
        let body = if let Some((editor, editable)) = &editor {
            div()
                .size_full()
                .child(Editor::new(editor).size_full().readonly(!*editable))
                .into_any_element()
        } else {
            let url = self.files.images.get(&path).cloned().flatten();
            div()
                .size_full()
                .p_2()
                .flex()
                .justify_center()
                .children(url.map(|url| {
                    img(SharedString::from(url.clone()))
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
                }))
                .into_any_element()
        };
        Some(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(crumb)
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }

    /// The Files and file tabs' content (web: `FileBrowserPanel`): the explorer alone, or beside
    /// the tab's file as a narrow column.
    pub(crate) fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        match self.render_open_file(cx) {
            Some(file) => h_flex()
                .flex_1()
                .min_h_0()
                .child(file)
                .child(
                    v_flex()
                        .w(px(256.))
                        .h_full()
                        .flex_none()
                        .border_l_1()
                        .border_color(c.border)
                        .child(self.render_explorer(cx)),
                )
                .into_any_element(),
            None => self.render_explorer(cx).into_any_element(),
        }
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
            .map(|(p, ..)| p)
            .collect();
        assert_eq!(sorted, ["src/A", "src/a.rs", "src/b.rs"]);
        assert_eq!(language_for("main.RS"), "rust");
        assert_eq!(language_for("App.tsx"), "tsx");
        assert_eq!(language_for("Makefile"), "text");
    }
}
