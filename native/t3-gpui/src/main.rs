mod attachments;
mod auth;
mod composer;
mod composer_menu;
mod diff_panel;
mod drag;
mod env;
mod git;
mod history;
mod keybindings;
mod model;
mod palette;
mod projects;
mod rpc;
mod settings;
mod sidebar;
mod terminal;
mod terminal_drawer;
mod timeline;

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::component::Sizable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;

use model::{ThreadView, model_key};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Network I/O runs on tokio; gpui tasks await the handle.
fn on_tokio<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME.get().unwrap().spawn(f)
}

#[derive(Clone)]
struct ModelOption {
    title: SharedString,
    key: SharedString,
}

impl SearchableListItem for ModelOption {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SharedString {
        &self.key
    }
}

type ModelPicker = SelectState<SearchableVec<ModelOption>>;

enum Screen {
    Pair,
    Connecting,
    Connected,
}

struct T3App {
    screen: Screen,
    error: Option<SharedString>,
    /// Connected environments (servers); see `env.rs`.
    envs: Vec<env::Env>,
    /// Key of the environment owning the open thread, draft, or settings.
    active: Option<String>,
    pair_input: Entity<InputState>,
    composer: Entity<TextareaState>,
    model_picker: Entity<ModelPicker>,
    /// What the picker was last synced to: config revision and selected key.
    picker_synced: (u64, String),
    /// Model chosen for the new-thread draft; `None` uses the default.
    draft_model: Option<serde_json::Value>,
    draft_runtime_mode: String,
    /// Prompt history position while Up/Down recall earlier messages.
    history_pos: Option<history::Position>,
    attachments: Vec<attachments::Attachment>,
    attachment_seq: u64,
    menu_trigger: Option<composer_menu::Trigger>,
    menu_items: Vec<composer_menu::MenuItem>,
    menu_index: usize,
    /// Drops path search results for an older trigger.
    menu_generation: u64,
    /// Provider/folder pairs whose command snapshot was already requested.
    requested_snapshots: HashSet<String>,
    draft_interaction_mode: String,
    /// Project of the new-thread draft shown instead of a thread.
    draft_project: Option<String>,
    selected: Option<String>,
    thread: ThreadView,
    thread_sub: Option<String>,
    /// Timeline rows the user opened.
    expanded: HashSet<String>,
    focus: FocusHandle,
    term: Option<terminal::TermView>,
    /// Request id of the `terminal.attach` stream.
    term_sub: Option<String>,
    term_status: Option<SharedString>,
    term_focus: FocusHandle,
    git: git::GitStatus,
    /// Request id of the `subscribeVcsStatus` stream.
    git_sub: Option<String>,
    /// Latest git action phase or outcome.
    git_progress: Option<SharedString>,
    palette_open: bool,
    palette_input: Entity<InputState>,
    /// Server message matches for the current palette query.
    palette_matches: Vec<serde_json::Value>,
    /// Drops search responses for an older query.
    palette_generation: u64,
    show_archived: bool,
    adding_project: bool,
    project_path: Entity<InputState>,
    /// `filesystem.browse` entries for the typed path.
    project_suggestions: Vec<serde_json::Value>,
    project_status: Option<SharedString>,
    browse_generation: u64,
    sidebar_hidden: bool,
    /// A one-second re-render is scheduled for the working timer.
    working_tick: std::cell::Cell<bool>,
    /// The selected thread's open still has to be reported with `thread.visit`.
    visit_open_pending: bool,
    visit_sent_at: Option<std::time::Instant>,
    visit_timer: bool,
    settings_open: bool,
    settings_status: Option<SharedString>,
    /// Client preferences (`settings::load_prefs`).
    prefs: serde_json::Value,
    /// Wake time a re-render is already scheduled for.
    snooze_wake_at: Option<i64>,
    diff_open: bool,
    /// `None` = whole thread, `Some(turn)` = one turn.
    diff_target: Option<u64>,
    diff_state: Option<Entity<gpui_kit::component::diff::DiffState>>,
    diff_note: Option<SharedString>,
    /// Drops responses for a diff the user has since replaced.
    diff_generation: u64,
    /// Selected option values per `requestId:questionId`.
    input_picks: std::collections::HashMap<String, Vec<String>>,
    transcript_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl T3App {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pair_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://host:3773/pair#token=…"));
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask anything…")
                .submit_on_enter(true);
            state.set_auto_grow(1, 12, cx);
            state
        });
        let model_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<ModelOption>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let project_path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/project"));
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search threads and commands…"));
        let _subscriptions = vec![
            cx.subscribe_in(
                &project_path,
                window,
                |this, _, e: &InputEvent, window, cx| match e {
                    InputEvent::Change => this.browse_project_path(cx),
                    InputEvent::PressEnter { .. } => this.add_project(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &palette_input,
                window,
                |this, _, e: &InputEvent, window, cx| this.on_palette_input(e, window, cx),
            ),
            cx.subscribe_in(
                &model_picker,
                window,
                |this, _, e: &SelectEvent<SearchableVec<ModelOption>>, _, cx| {
                    if let SelectEvent::Confirm(Some(key)) = e {
                        this.pick_model(key, cx);
                    }
                },
            ),
            cx.subscribe_in(&pair_input, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.pair(cx);
                }
            }),
            cx.subscribe_in(
                &composer,
                window,
                |this, _, e: &InputEvent, window, cx| match e {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => this.update_composer_menu(cx),
                    _ => {}
                },
            ),
        ];
        let mut app = Self {
            screen: Screen::Pair,
            error: None,
            envs: Vec::new(),
            active: None,
            pair_input,
            composer,
            model_picker,
            picker_synced: (u64::MAX, String::new()),
            draft_model: None,
            draft_runtime_mode: "full-access".into(),
            history_pos: None,
            attachments: Vec::new(),
            attachment_seq: 0,
            menu_trigger: None,
            menu_items: Vec::new(),
            menu_index: 0,
            menu_generation: 0,
            requested_snapshots: HashSet::new(),
            draft_interaction_mode: "default".into(),
            draft_project: None,
            selected: None,
            thread: ThreadView::default(),
            thread_sub: None,
            expanded: HashSet::new(),
            focus,
            term: None,
            term_sub: None,
            term_status: None,
            term_focus: cx.focus_handle(),
            git: Default::default(),
            git_sub: None,
            git_progress: None,
            palette_open: false,
            palette_input,
            palette_matches: Vec::new(),
            palette_generation: 0,
            show_archived: false,
            adding_project: false,
            project_path,
            project_suggestions: Vec::new(),
            project_status: None,
            browse_generation: 0,
            sidebar_hidden: false,
            working_tick: Default::default(),
            visit_open_pending: false,
            visit_sent_at: None,
            visit_timer: false,
            settings_open: false,
            settings_status: None,
            prefs: settings::load_prefs(),
            snooze_wake_at: None,
            diff_open: false,
            diff_target: None,
            diff_state: None,
            diff_note: None,
            diff_generation: 0,
            input_picks: Default::default(),
            transcript_scroll: ScrollHandle::new(),
            _subscriptions,
        };
        for session in auth::load_all() {
            app.connect(session, cx);
        }
        app
    }

    fn pair(&mut self, cx: &mut Context<Self>) {
        let url = self.pair_input.read(cx).value().to_string();
        self.screen = Screen::Connecting;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { auth::pair(&url).await }).await;
            this.update(cx, |app, cx| match result {
                Ok(Ok(session)) => {
                    let key = session.base_url.clone();
                    app.connect(session, cx);
                    app.activate(&key, cx);
                    app.screen = Screen::Connected;
                }
                Ok(Err(e)) => app.fail(e.to_string(), cx),
                Err(e) => app.fail(e.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.screen = Screen::Pair;
        self.error = Some(error.into());
        cx.notify();
    }

    fn new_thread(
        &mut self,
        env: String,
        project_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate(&env, cx);
        if let (Some(client), Some(sub)) = (self.client(), self.thread_sub.take()) {
            client.cancel(&sub);
        }
        self.selected = None;
        self.thread = ThreadView::default();
        self.adding_project = false;
        self.draft_project = Some(project_id);
        self.draft_model = None;
        self.settings_open = false;
        self.draft_runtime_mode = self.config().settings["defaultRuntimeMode"]
            .as_str()
            .unwrap_or("full-access")
            .to_string();
        self.draft_interaction_mode = "default".into();
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn launch(
        &mut self,
        project_id: String,
        text: String,
        attachments: Vec<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else {
            return;
        };
        let project = self.shell().projects.iter().find(|p| p.id == project_id);
        let Some(model) = self
            .draft_model
            .clone()
            .or_else(|| self.config().model_for(project))
        else {
            self.error = Some("No available provider on this server.".into());
            return cx.notify();
        };
        let title: String = text.lines().next().unwrap_or("").chars().take(60).collect();
        let command_id = uuid::Uuid::new_v4().to_string();
        let input = json!({
            "commandId": command_id,
            "creationSource": "web",
            "projectId": project_id,
            "title": if title.trim().is_empty() { "New thread".to_string() } else { title },
            "generateTitle": true,
            "modelSelection": model,
            "runtimeMode": self.draft_runtime_mode,
            "interactionMode": self.draft_interaction_mode,
            "workspaceStrategy": { "type": "root" },
            "initialMessage": { "messageId": command_id, "text": text, "attachments": attachments },
        });
        let env_key = self.env().map(|e| e.key.clone()).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("orchestration.launchThread", input).await })
                    .await;
            this.update(cx, |app, cx| match result {
                Ok(Ok(launched)) => {
                    app.draft_project = None;
                    if let Some(id) = launched["threadId"].as_str() {
                        app.select_thread(env_key.clone(), id.to_string(), cx);
                    }
                }
                Ok(Err(e)) => app.error = Some(format!("Could not start thread: {e}").into()),
                Err(e) => app.error = Some(e.to_string().into()),
            })
            .ok();
        })
        .detach();
    }

    fn select_thread(&mut self, env: String, id: String, cx: &mut Context<Self>) {
        self.adding_project = false;
        let same_env = self.env().is_some_and(|e| e.key == env);
        self.activate(&env, cx);
        if !same_env || self.selected.as_deref() != Some(id.as_str()) {
            self.visit_open_pending = true;
            self.visit_sent_at = None;
        }
        self.draft_project = None;
        self.settings_open = false;
        if !same_env || self.selected.as_deref() != Some(id.as_str()) {
            self.thread = ThreadView::default();
            self.history_pos = None;
            self.diff_generation += 1;
            self.diff_target = None;
            self.diff_state = None;
            self.diff_note = None;
        }
        self.selected = Some(id);
        self.subscribe_thread(cx);
        self.subscribe_git(cx);
        if self.term.is_some() {
            self.open_terminal(false, cx);
        }
        self.maybe_visit(cx);
    }

    /// (Re)subscribes the selected thread, resuming from its cursor when it already has data.
    fn subscribe_thread(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(id)) = (self.client(), self.selected.clone()) else {
            return;
        };
        if let Some(old) = self.thread_sub.take() {
            client.cancel(&old);
        }
        let mut payload = json!({ "threadId": id, "requestCompletionMarker": true });
        if self.thread.sequence > 0 {
            payload["afterSequence"] = json!(self.thread.sequence);
        }
        let (sub, mut rx) = client.request("orchestration.subscribeThread", payload);
        self.thread_sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.recv().await {
                let ok = this.update(cx, |app, cx| {
                    if app.thread_sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    match item {
                        Ok(item) => {
                            let before = app.thread.latest_turn();
                            app.thread.apply(&item);
                            if app.diff_open
                                && app.diff_target.is_none()
                                && app.thread.latest_turn() != before
                            {
                                app.load_diff(None, cx);
                            }
                        }
                        Err(e) => app.error = Some(e.to_string().into()),
                    }
                    app.transcript_scroll.scroll_to_bottom();
                    cx.notify();
                    true
                });
                if !matches!(ok, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let attachments = match self.take_attachments() {
            Ok(attachments) => attachments,
            Err(message) => {
                self.error = Some(message.into());
                return cx.notify();
            }
        };
        self.history_pos = None;
        if let Some(project_id) = self.draft_project.clone() {
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            return self.launch(project_id, text, attachments, cx);
        }
        let Some(thread_id) = self.selected.clone() else {
            return;
        };
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        let command_id = uuid::Uuid::new_v4().to_string();
        self.dispatch(
            json!({
                "type": "message.dispatch",
                "commandId": command_id,
                "messageId": command_id,
                "threadId": thread_id,
                "text": text,
                "attachments": attachments,
                "dispatchMode": { "type": "queue_after_active" },
                "createdBy": "user",
                "creationSource": "web",
            }),
            cx,
        );
    }

    /// Sends an `OrchestrationV2Command`, filling in `commandId`. Failures show in the error line.
    pub(crate) fn dispatch(&mut self, command: serde_json::Value, cx: &mut Context<Self>) {
        let env = self.env().map(|e| e.key.clone()).unwrap_or_default();
        self.dispatch_to(&env, command, cx);
    }

    /// Sends a command to a specific environment (a sidebar row's, not necessarily the active one).
    pub(crate) fn dispatch_to(
        &mut self,
        env: &str,
        mut command: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.env_by(env).and_then(|e| e.client.clone()) else {
            self.error = Some("Not connected.".into());
            return cx.notify();
        };
        if command["commandId"].is_null() {
            command["commandId"] = json!(uuid::Uuid::new_v4().to_string());
        }
        let kind = command["type"].as_str().unwrap_or("command").to_string();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { client.call("orchestration.dispatchCommand", command).await },
                )
                .await;
            let error = match result {
                Ok(Ok(_)) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            this.update(cx, |app, cx| {
                app.error = Some(format!("{kind} failed: {error}").into());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The selection the picker shows: the draft's choice or default, else the open thread's.
    fn current_model(&self) -> Option<serde_json::Value> {
        if let Some(project_id) = &self.draft_project {
            let project = self.shell().projects.iter().find(|p| &p.id == project_id);
            return self
                .draft_model
                .clone()
                .or_else(|| self.config().model_for(project));
        }
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))?;
        Some(thread.model_selection.clone())
    }

    fn pick_model(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((_, selection)) = self
            .config()
            .model_options()
            .into_iter()
            .find(|(_, s)| model_key(s) == key)
        else {
            return;
        };
        if self.draft_project.is_some() {
            self.draft_model = Some(selection);
            return cx.notify();
        }
        if let Some(thread_id) = self.selected.clone() {
            self.dispatch(
                json!({ "type": "thread.model-selection.set", "threadId": thread_id, "modelSelection": selection }),
                cx,
            );
        }
    }

    /// Select state needs a window, so the picker catches up with config and selection at render.
    fn sync_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self
            .current_model()
            .map(|m| model_key(&m))
            .unwrap_or_default();
        if self.picker_synced == (self.config().revision, key.clone()) {
            return;
        }
        let items: Vec<ModelOption> = self
            .config()
            .model_options()
            .into_iter()
            .map(|(title, selection)| ModelOption {
                title: title.into(),
                key: model_key(&selection).into(),
            })
            .collect();
        let rebuild = self.picker_synced.0 != self.config().revision;
        self.picker_synced = (self.config().revision, key.clone());
        self.model_picker.update(cx, |picker, cx| {
            if rebuild {
                picker.set_items(SearchableVec::new(items), window, cx);
            }
            picker.set_selected_value(&SharedString::from(key), window, cx);
        });
    }

    fn confirm_revert(
        &mut self,
        turn: u64,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let choice = |id: &'static str, label: &'static str, restore_files: Option<bool>| {
                let (app, text) = (app.clone(), text.clone());
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    if let Some(restore_files) = restore_files {
                        app.update(cx, |this, cx| {
                            this.revert(turn, restore_files, text.clone(), window, cx)
                        })
                        .ok();
                    }
                })
            };
            dialog
                .title("Revert to this message?")
                .child(div().text_sm().child(
                    "Later messages are removed and the message goes back into the composer.",
                ))
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(choice("revert-cancel", "Cancel", None).ghost())
                        .child(choice("revert-chat", "Conversation only", Some(false)).outline())
                        .child(
                            choice("revert-all", "Revert files and conversation", Some(true))
                                .primary(),
                        ),
                )
        });
    }

    fn revert(
        &mut self,
        turn: u64,
        restore_files: bool,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.selected.clone() else {
            return;
        };
        if self
            .shell()
            .threads
            .iter()
            .any(|t| t.id == thread_id && t.is_running())
        {
            self.error = Some("Interrupt the current turn before reverting.".into());
            return cx.notify();
        }
        let Some(command) = self
            .thread
            .rollback_command(&thread_id, turn, restore_files)
        else {
            self.error =
                Some("This checkpoint has no file snapshot. Revert the conversation only.".into());
            return cx.notify();
        };
        self.error = None;
        self.dispatch(command, cx);
        self.composer
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// Up/Down recall earlier prompts while the composer is empty or showing a recalled one.
    fn composer_history_key(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if !(m.control || m.alt || m.platform || m.shift)
            && self.composer_menu_key(&k.key, window, cx)
        {
            return cx.stop_propagation();
        }
        if !matches!(k.key.as_str(), "up" | "down") || m.control || m.alt || m.platform || m.shift {
            return;
        }
        let entries = history::entries(self.thread.items());
        let current = self.composer.read(cx).value().to_string();
        let Some((position, text)) =
            history::step(k.key == "up", &entries, self.history_pos.as_ref(), &current)
        else {
            return;
        };
        self.history_pos = position;
        self.composer
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.stop_propagation();
    }

    /// `thread.visit` like client-runtime's threadVisits: the open goes out at once, later
    /// activity on the open thread at most every 10s, so its unread state clears.
    fn maybe_visit(&mut self, cx: &mut Context<Self>) {
        const THROTTLE: Duration = Duration::from_secs(10);
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
        else {
            return;
        };
        let opened = self.visit_open_pending;
        if !opened && !thread.needs_visit() {
            return;
        }
        let elapsed = self.visit_sent_at.map_or(THROTTLE, |at| at.elapsed());
        if !opened && elapsed < THROTTLE {
            if !self.visit_timer {
                self.visit_timer = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(THROTTLE - elapsed).await;
                    this.update(cx, |app, cx| {
                        app.visit_timer = false;
                        app.maybe_visit(cx);
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        let mut command = json!({ "type": "thread.visit", "threadId": thread.id, "visitedAt": thread.updated_at });
        if opened {
            command["opened"] = json!(true);
        }
        self.visit_open_pending = false;
        self.visit_sent_at = Some(std::time::Instant::now());
        self.dispatch(command, cx);
    }

    /// Runs the command a key press is bound to (server keybindings, else the defaults).
    fn run_shortcut(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let mods = keybindings::Mods {
            meta: m.platform,
            ctrl: m.control,
            shift: m.shift,
            alt: m.alt,
        };
        if !(mods.meta || mods.ctrl || mods.alt || k.key.starts_with('f')) {
            return false;
        }
        let terminal_focus = self.term_focus.is_focused(window);
        let composer_focus = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let ctx = std::collections::HashMap::from([
            ("terminalFocus", terminal_focus),
            ("terminalOpen", self.term.is_some()),
            ("composerFocus", composer_focus),
            ("editableFocus", composer_focus || self.palette_open),
            ("rightPanelOpen", self.diff_open),
            ("draftThreadRoute", self.draft_project.is_some()),
            ("isDesktop", true),
        ]);
        let rules = if self.config().keybindings.is_empty() {
            keybindings::defaults()
        } else {
            self.config().keybindings.clone()
        };
        let Some(command) =
            keybindings::resolve(&rules, &k.key, mods, cfg!(target_os = "macos"), &ctx)
        else {
            return false;
        };
        let thread = self.selected.clone();
        match command.as_str() {
            "commandPalette.toggle" => self.toggle_palette(window, cx),
            "sidebar.toggle" => {
                self.sidebar_hidden = !self.sidebar_hidden;
                cx.notify();
            }
            "terminal.toggle" if thread.is_some() => self.toggle_terminal(window, cx),
            "diff.toggle" if thread.is_some() => self.toggle_diff(cx),
            "chat.new" => {
                let env = self.env().map(|e| e.key.clone());
                let project = self
                    .draft_project
                    .clone()
                    .or_else(|| {
                        thread
                            .as_ref()
                            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
                            .map(|t| t.project_id.clone())
                    })
                    .or_else(|| self.shell().projects.first().map(|p| p.id.clone()));
                let (Some(env), Some(project)) = (env, project) else {
                    return false;
                };
                self.new_thread(env, project, window, cx);
            }
            "thread.next" | "thread.previous" => {
                return self.step_thread(command == "thread.next", cx);
            }
            "thread.pin" | "thread.settle" => {
                let Some(t) = thread
                    .as_ref()
                    .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
                else {
                    return false;
                };
                let kind = match (command.as_str(), t.pinned_at.is_some()) {
                    ("thread.pin", true) => "thread.unpin",
                    ("thread.pin", false) => "thread.pin",
                    _ => "thread.settle",
                };
                let id = t.id.clone();
                self.dispatch(json!({ "type": kind, "threadId": id }), cx);
            }
            _ => return false,
        }
        true
    }

    /// Selects the next or previous thread in sidebar order.
    fn step_thread(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let threads: Vec<model::ThreadShell> = self
            .envs
            .iter()
            .flat_map(|e| e.shell.threads.iter().cloned())
            .collect();
        let s = sidebar::sections(&threads, jiff::Timestamp::now().as_millisecond());
        let order: Vec<(String, String)> = s
            .pinned
            .iter()
            .chain(&s.active)
            .chain(&s.snoozed)
            .chain(&s.settled)
            .map(|t| (t.env.clone(), t.id.clone()))
            .collect();
        if order.is_empty() {
            return false;
        }
        let current = self.env().map(|e| e.key.clone()).zip(self.selected.clone());
        let at = current.and_then(|c| order.iter().position(|o| *o == c));
        let next = match (at, forward) {
            (None, _) => 0,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        let (env, id) = order[next].clone();
        self.select_thread(env, id, cx);
        true
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
        else {
            return;
        };
        let command = match (&thread.active_run_id, self.config().thread_stop) {
            (_, true) => json!({ "type": "thread.stop", "threadId": thread.id }),
            (Some(run), false) => {
                json!({ "type": "run.interrupt", "threadId": thread.id, "runId": run })
            }
            (None, false) => return,
        };
        self.dispatch(command, cx);
    }

    /// Disconnects the active environment; others stay connected.
    fn forget_session(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.env().map(|e| e.key.clone()) {
            self.remove_env(&key, cx);
        }
    }

    /// Shows the pairing screen to add another environment; Cancel returns.
    fn add_environment(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.screen = Screen::Pair;
        cx.notify();
    }

    fn render_pair(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let connecting = matches!(self.screen, Screen::Connecting);
        let theme = cx.theme();
        v_flex().size_full().items_center().justify_center().child(
            v_flex()
                .w(px(440.))
                .gap_3()
                .p_6()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Connect to a T3 Code server"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Run `t3 pair` on the host and paste the pairing URL."),
                )
                .child(Input::new(&self.pair_input))
                .child(
                    Button::new("pair")
                        .primary()
                        .label(if connecting {
                            "Connecting…"
                        } else {
                            "Connect"
                        })
                        .disabled(connecting)
                        .on_click(cx.listener(|this, _, _, cx| this.pair(cx))),
                )
                .children(
                    self.error
                        .clone()
                        .map(|e| div().text_sm().text_color(theme.danger).child(e)),
                )
                .when(!self.envs.is_empty() && !connecting, |card| {
                    card.child(Button::new("pair-cancel").ghost().label("Cancel").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.error = None;
                            this.screen = Screen::Connected;
                            cx.notify();
                        }),
                    ))
                }),
        )
    }

    /// `drag`: the row's section and index, for drag and drop; `None` for archived rows.
    fn thread_row(
        &self,
        thread: &model::ThreadShell,
        archived: bool,
        drag: Option<(drag::Section, usize)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().colors;
        let id = thread.id.clone();
        let env_key = thread.env.clone();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let app = cx.entity().downgrade();
        let pinned = thread.pinned_at.is_some();
        let settled = thread.settled_override.as_deref() == Some("settled");
        let snoozed = thread.snoozed_until.is_some();
        let menu_id = id.clone();
        let menu_env = env_key.clone();
        let title = thread.title.clone();
        let env = self.env_by(&thread.env);
        let project = env
            .and_then(|e| e.shell.projects.iter().find(|p| p.id == thread.project_id))
            .map(|p| match (self.envs.len() > 1, env) {
                (true, Some(e)) => format!("{} · {}", p.title, e.label()),
                _ => p.title.clone(),
            });
        let presets = env.and_then(|e| e.config.snooze_presets.clone());
        h_flex()
            .id(SharedString::from(format!("thread-{id}")))
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(archived, |d| d.text_color(theme.muted_foreground))
            .when(selected, |d| {
                d.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .hover(|d| d.bg(theme.sidebar_accent))
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .when(thread.is_running(), |d| d.bg(theme.success))
                    .when(
                        !thread.is_running() && !selected && thread.has_unseen_completion(),
                        |d| d.bg(theme.info),
                    ),
            )
            .child(div().flex_1().truncate().child(thread.title.clone()))
            .children(project.map(|p| {
                div()
                    .max_w(px(90.))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(p)
            }))
            .on_click(cx.listener({
                let (env_key, id) = (env_key.clone(), id.clone());
                move |this, _, _, cx| this.select_thread(env_key.clone(), id.clone(), cx)
            }))
            .when_some(drag, |row, (section, index)| {
                let dragged = drag::DraggedThread {
                    env: env_key.clone(),
                    id: id.clone(),
                    title: thread.title.clone().into(),
                    section,
                };
                row.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    .when(section != drag::Section::Snoozed, |row| {
                        row.drag_over::<drag::DraggedThread>(move |s, _, _, _| {
                            s.border_t_2().border_color(theme.ring)
                        })
                        .on_drop(cx.listener(
                            move |this, d: &drag::DraggedThread, _, cx| {
                                this.drop_thread(d, section, index, cx)
                            },
                        ))
                    })
            })
            .context_menu(move |menu, window, cx| {
                let action = |label: &str, kind: &str, extra: serde_json::Value| {
                    let app = app.clone();
                    let env = menu_env.clone();
                    let mut command = json!({ "type": kind, "threadId": menu_id });
                    if let (Some(command), Some(extra)) =
                        (command.as_object_mut(), extra.as_object())
                    {
                        command.extend(extra.clone());
                    }
                    PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
                        app.update(cx, |this, cx| this.dispatch_to(&env, command.clone(), cx))
                            .ok();
                    })
                };
                if archived {
                    return menu.item(action("Unarchive", "thread.unarchive", json!({})));
                }
                let menu = menu
                    .item(if pinned {
                        action("Unpin", "thread.unpin", json!({}))
                    } else {
                        action("Pin", "thread.pin", json!({}))
                    })
                    .item(if settled {
                        action("Unsettle", "thread.unsettle", json!({ "reason": "user" }))
                    } else {
                        action("Settle", "thread.settle", json!({}))
                    });
                let menu = if snoozed {
                    menu.item(action(
                        "Unsnooze",
                        "thread.unsnooze",
                        json!({ "reason": "user" }),
                    ))
                } else {
                    let (app, thread_id, presets, env) = (
                        app.clone(),
                        menu_id.clone(),
                        presets.clone(),
                        menu_env.clone(),
                    );
                    menu.submenu("Snooze", window, cx, move |mut sub, _, _| {
                        // Resolved when the menu opens, like the web, so times are never stale.
                        let now = jiff::Zoned::now();
                        for preset in sidebar::snooze_presets(&now, presets.as_ref()) {
                            let (app, env) = (app.clone(), env.clone());
                            let command = json!({ "type": "thread.snooze", "threadId": thread_id,
                                "snoozedUntil": preset.until.to_string() });
                            sub = sub.item(PopupMenuItem::new(preset.label).on_click(
                                move |_, _, cx| {
                                    app.update(cx, |this, cx| {
                                        this.dispatch_to(&env, command.clone(), cx)
                                    })
                                    .ok();
                                },
                            ));
                        }
                        sub
                    })
                };
                menu.item(action("Mark unread", "thread.mark-unread", json!({})))
                    .item(rename_item(
                        app.clone(),
                        menu_env.clone(),
                        menu_id.clone(),
                        title.clone(),
                    ))
                    .separator()
                    .item(action("Archive", "thread.archive", json!({})))
                    .item(delete_item(
                        app.clone(),
                        menu_env.clone(),
                        menu_id.clone(),
                        title.clone(),
                    ))
            })
    }

    /// A header drops the row at the top of its section.
    fn section_header(
        &self,
        label: String,
        section: drag::Section,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = cx.theme().colors;
        div()
            .id(SharedString::from(format!("section-{label}")))
            .px_2()
            .pt_3()
            .pb_1()
            .text_xs()
            .text_color(c.muted_foreground)
            .child(label)
            .when(section != drag::Section::Snoozed, |h| {
                h.drag_over::<drag::DraggedThread>(move |s, _, _, _| {
                    s.text_color(c.foreground).bg(c.sidebar_accent)
                })
                .on_drop(cx.listener(
                    move |this, d: &drag::DraggedThread, _, cx| this.drop_thread(d, section, 0, cx),
                ))
            })
    }

    /// Plans a sidebar drop like the web and sends its commands in order.
    fn drop_thread(
        &mut self,
        d: &drag::DraggedThread,
        target: drag::Section,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let threads: Vec<model::ThreadShell> = self
            .envs
            .iter()
            .flat_map(|e| e.shell.threads.iter().cloned())
            .collect();
        let sections = sidebar::sections(&threads, jiff::Timestamp::now().as_millisecond());
        let key = |t: &model::ThreadShell| drag::row_key(&t.env, &t.id);
        let pinned: Vec<String> = sections.pinned.iter().map(|t| key(t)).collect();
        let active: Vec<String> = sections.active.iter().map(|t| key(t)).collect();
        let pinned_keys = threads
            .iter()
            .map(|t| (key(t), t.pin_order_key.clone()))
            .collect();
        // Stored keys, then implied sort keys for the Active rows (web: `activeKeysById`).
        let mut active_keys: std::collections::HashMap<String, Option<String>> = threads
            .iter()
            .map(|t| (key(t), t.active_order_key.clone()))
            .collect();
        for t in &sections.active {
            active_keys.insert(key(t), Some(sidebar::active_sort_key(t)));
        }
        let moved = drag::row_key(&d.env, &d.id);
        let plan = drag::plan_drop(
            &moved,
            d.section,
            target,
            index,
            (&pinned, &pinned_keys),
            (&active, &active_keys),
        );
        let commands = drag::commands(plan, (&d.env, &d.id));
        self.run_commands(commands, cx);
    }

    /// Sends commands one after another, stopping at the first failure.
    fn run_commands(&mut self, commands: Vec<(String, serde_json::Value)>, cx: &mut Context<Self>) {
        let steps: Vec<(Option<rpc::RpcClient>, serde_json::Value)> = commands
            .into_iter()
            .map(|(env, mut c)| {
                c["commandId"] = json!(uuid::Uuid::new_v4().to_string());
                (self.env_by(&env).and_then(|e| e.client.clone()), c)
            })
            .collect();
        cx.spawn(async move |this, cx| {
            for (client, command) in steps {
                let kind = command["type"].as_str().unwrap_or("command").to_owned();
                let Some(client) = client else {
                    this.update(cx, |app, cx| {
                        app.error = Some(format!("{kind}: server not connected").into());
                        cx.notify();
                    })
                    .ok();
                    return;
                };
                let result =
                    on_tokio(
                        async move { client.call("orchestration.dispatchCommand", command).await },
                    )
                    .await;
                let error = match result {
                    Ok(Ok(_)) => continue,
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                };
                this.update(cx, |app, cx| {
                    app.error = Some(format!("{kind} failed: {error}").into());
                    cx.notify();
                })
                .ok();
                return;
            }
        })
        .detach();
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().colors;
        let now_ms = jiff::Timestamp::now().as_millisecond();
        self.schedule_snooze_wake(now_ms, cx);
        let threads: Vec<model::ThreadShell> = self
            .envs
            .iter()
            .flat_map(|e| e.shell.threads.iter().cloned())
            .collect();
        let sections = sidebar::sections(&threads, now_ms);
        let mut list = v_flex()
            .id("threads")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .children(self.envs.iter().filter_map(|e| {
                let error = e.error.clone()?;
                Some(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(format!("{}: {error}", e.label())),
                )
            }));
        for (label, rows, section) in [
            ("Pinned", &sections.pinned, drag::Section::Pinned),
            ("Active", &sections.active, drag::Section::Active),
            ("Snoozed", &sections.snoozed, drag::Section::Snoozed),
            ("Settled", &sections.settled, drag::Section::Settled),
        ] {
            // An empty Pinned or Settled section still takes drops through its header.
            if rows.is_empty() && !matches!(section, drag::Section::Pinned | drag::Section::Settled)
            {
                continue;
            }
            list =
                list.child(self.section_header(format!("{label} ({})", rows.len()), section, cx));
            for (index, thread) in rows.iter().enumerate() {
                list = list.child(self.thread_row(thread, false, Some((section, index)), cx));
            }
        }
        let archived: Vec<model::ThreadShell> = self
            .envs
            .iter()
            .flat_map(|e| e.shell.archived.iter().cloned())
            .collect();
        if !archived.is_empty() {
            let open = self.show_archived;
            list = list.child(
                div()
                    .id("archived-toggle")
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .child(format!(
                        "{} Archived ({})",
                        if open { "▾" } else { "▸" },
                        archived.len()
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_archived = !this.show_archived;
                        cx.notify();
                    })),
            );
            if open {
                for thread in archived.iter() {
                    list = list.child(self.thread_row(thread, true, None, cx));
                }
            }
        }
        // Every environment's projects; named with their server when several are connected.
        let many = self.envs.len() > 1;
        let projects: Vec<(String, String, String)> = self
            .envs
            .iter()
            .flat_map(|e| {
                e.shell.projects.iter().map(move |p| {
                    let title = if many {
                        format!("{} · {}", p.title, e.label())
                    } else {
                        p.title.clone()
                    };
                    (e.key.clone(), p.id.clone(), title)
                })
            })
            .collect();
        let app = cx.entity().downgrade();
        v_flex()
            .w(px(300.))
            .h_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(44.))
                    .px_3()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("t3code-rs"),
                    )
                    .child(
                        Button::new("new-thread")
                            .small()
                            .ghost()
                            .label("New thread")
                            .dropdown_menu(move |mut menu, _, _| {
                                for (env, id, title) in projects.clone() {
                                    let app = app.clone();
                                    menu = menu.item(PopupMenuItem::new(title).on_click(
                                        move |_, window, cx| {
                                            app.update(cx, |this, cx| {
                                                this.new_thread(env.clone(), id.clone(), window, cx)
                                            })
                                            .ok();
                                        },
                                    ));
                                }
                                let app = app.clone();
                                menu.separator()
                                    .item(PopupMenuItem::new("Add project…").on_click(
                                        move |_, window, cx| {
                                            app.update(cx, |this, cx| {
                                                this.open_add_project(window, cx)
                                            })
                                            .ok();
                                        },
                                    ))
                            }),
                    )
                    .child(
                        Button::new("add-server")
                            .small()
                            .ghost()
                            .label("+ Server")
                            .tooltip("Connect another T3 Code server")
                            .on_click(cx.listener(|this, _, _, cx| this.add_environment(cx))),
                    )
                    .child(
                        Button::new("open-settings")
                            .small()
                            .ghost()
                            .label("Settings")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                    ),
            )
            .child(list)
    }

    /// Re-renders when the next snooze ends, so the thread leaves the Snoozed section on time.
    fn schedule_snooze_wake(&mut self, now_ms: i64, cx: &mut Context<Self>) {
        let next = self
            .envs
            .iter()
            .flat_map(|e| e.shell.threads.iter())
            .filter_map(|t| sidebar::ts_ms(t.snoozed_until.as_deref()))
            .filter(|&wake| wake > now_ms)
            .min();
        if next.is_none() || next == self.snooze_wake_at {
            return;
        }
        self.snooze_wake_at = next;
        let delay = Duration::from_millis((next.unwrap() - now_ms) as u64 + 50);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |app, cx| {
                app.snooze_wake_at = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_thread(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().colors;
        if let Some(project) = self
            .draft_project
            .as_ref()
            .and_then(|id| self.shell().projects.iter().find(|p| &p.id == id))
        {
            let model = self.current_model();
            return v_flex()
                .flex_1()
                .h_full()
                .justify_center()
                .child(
                    v_flex()
                        .mx_auto()
                        .max_w(px(760.))
                        .w_full()
                        .p_4()
                        .gap_2()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("New thread in {}", project.title)),
                        )
                        .child(match &model {
                            Some(_) => self.render_composer_controls(cx).into_any_element(),
                            None => div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No available provider")
                                .into_any_element(),
                        })
                        .when(
                            self.menu_trigger.is_some() && !self.menu_items.is_empty(),
                            |col| col.child(self.render_composer_menu(cx)),
                        )
                        .child(self.render_attachments(cx))
                        .child(Textarea::new(&self.composer))
                        .children(
                            self.error
                                .clone()
                                .map(|e| div().text_sm().text_color(theme.danger).child(e)),
                        ),
                )
                .into_any_element();
        }
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))
        else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(
                    if self.shell().synced || !self.shell().projects.is_empty() {
                        "Select a thread"
                    } else {
                        "Loading…"
                    },
                )
                .into_any_element();
        };
        let rollback_supported = self
            .config()
            .providers
            .iter()
            .find(|p| thread.model_selection["instanceId"] == p.instance_id.as_str())
            .is_none_or(|p| p.supports_conversation_rollback != Some(false));
        let revert_turns = if rollback_supported && !thread.is_running() {
            self.thread.revert_turns(thread.active_run_id.as_deref())
        } else {
            Default::default()
        };
        let items: Vec<AnyElement> = timeline::group_rows(self.thread.items())
            .into_iter()
            .map(|row| match row {
                timeline::Row::Item(item) => {
                    self.render_item(item, revert_turns.get(&item.id).copied(), cx)
                }
                timeline::Row::Group { id, items } => self.render_group(id, &items, cx),
            })
            .collect();
        v_flex()
            .flex_1()
            .h_full()
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(thread.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(self.config().label(&thread.model_selection)),
                    )
                    .child(div().flex_1())
                    .child(self.render_git_toolbar(cx))
                    .child(
                        Button::new("terminal-toggle")
                            .small()
                            .label("Terminal")
                            .when(self.term.is_some(), |b| b.primary())
                            .when(self.term.is_none(), |b| b.ghost())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("diff-toggle")
                            .small()
                            .label("Diff")
                            .when(self.diff_open, |b| b.primary())
                            .when(!self.diff_open, |b| b.ghost())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_diff(cx))),
                    )
                    .when(thread.is_running(), |header| {
                        header.child(
                            Button::new("stop")
                                .small()
                                .outline()
                                .label("Stop")
                                .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .child(
                        v_flex()
                            .mx_auto()
                            .max_w(px(760.))
                            .w_full()
                            .px_4()
                            .py_4()
                            .gap_4()
                            .children(items)
                            .children(
                                thread
                                    .activity_run_started_at
                                    .clone()
                                    .or_else(|| thread.latest_run_started_at.clone())
                                    .filter(|_| thread.is_running())
                                    .map(|started| self.render_working(&started, cx)),
                            ),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| div().px_4().text_sm().text_color(theme.danger).child(e)),
            )
            .when_some(
                thread.last_error.clone().filter(|_| !thread.is_running()),
                |col, error| {
                    col.child(
                        div().mx_auto().max_w(px(760.)).w_full().px_4().child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.danger)
                                .text_sm()
                                .text_color(theme.danger)
                                .child(error),
                        ),
                    )
                },
            )
            .child(
                v_flex()
                    .mx_auto()
                    .max_w(px(760.))
                    .w_full()
                    .p_4()
                    .gap_2()
                    .when(
                        self.menu_trigger.is_some() && !self.menu_items.is_empty(),
                        |col| col.child(self.render_composer_menu(cx)),
                    )
                    .child(self.render_attachments(cx))
                    .child(
                        div()
                            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                                this.composer_history_key(e, window, cx)
                            }))
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.add_attachments(paths.paths().to_vec(), cx)
                            }))
                            .child(Textarea::new(&self.composer)),
                    )
                    .child(self.render_composer_controls(cx)),
            )
            .when(self.term.is_some(), |col| {
                col.child(self.render_terminal(cx))
            })
            .into_any_element()
    }
}

impl Render for T3App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_picker(window, cx);
        let theme = cx.theme().colors;
        let root = h_flex()
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if matches!(this.screen, Screen::Connected) && this.run_shortcut(e, window, cx) {
                    cx.stop_propagation();
                }
            }));
        match self.screen {
            Screen::Connected => root
                .when(!self.sidebar_hidden, |root| {
                    root.child(self.render_sidebar(cx))
                })
                .when(self.settings_open, |root| {
                    root.child(self.render_settings(cx))
                })
                .when(!self.settings_open && self.adding_project, |root| {
                    root.child(self.render_add_project(cx))
                })
                .when(!self.settings_open && !self.adding_project, |root| {
                    root.child(self.render_thread(cx))
                })
                .when(self.diff_open && self.selected.is_some(), |root| {
                    root.child(self.render_diff_panel(cx))
                })
                .when(self.palette_open, |root| {
                    root.child(self.render_palette(cx))
                }),
            _ => root.child(self.render_pair(cx)),
        }
    }
}

fn main() {
    RUNTIME
        .set(tokio::runtime::Runtime::new().expect("tokio runtime"))
        .ok();
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        palette::bind_keys(cx);
        gpui_kit::component::Theme::change(settings::theme_mode(&settings::load_prefs()), None, cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1200.), px(800.)), cx)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| T3App::new(window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}

fn rename_item(
    app: WeakEntity<T3App>,
    env: String,
    thread_id: String,
    title: String,
) -> PopupMenuItem {
    PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(title.clone(), window, cx);
            state
        });
        let (app, thread_id, env) = (app.clone(), thread_id.clone(), env.clone());
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, thread_id, input, env) = (app.clone(), thread_id.clone(), input.clone(), env.clone());
            dialog
                .title("Rename thread")
                .child(Input::new(&input))
                .button_props(DialogButtonProps::default().ok_text("Rename").show_cancel(true))
                .on_ok(move |_, _, cx: &mut App| {
                    let title = input.read(cx).value().trim().to_string();
                    if title.is_empty() {
                        return false;
                    }
                    let command = json!({ "type": "thread.metadata.update", "threadId": thread_id, "title": title });
                    app.update(cx, |this, cx| this.dispatch_to(&env, command, cx)).ok();
                    true
                })
        });
    })
}

fn delete_item(
    app: WeakEntity<T3App>,
    env: String,
    thread_id: String,
    title: String,
) -> PopupMenuItem {
    PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
        let (app, thread_id, title, env) =
            (app.clone(), thread_id.clone(), title.clone(), env.clone());
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (app, thread_id, env) = (app.clone(), thread_id.clone(), env.clone());
            dialog
                .confirm()
                .title("Delete thread?")
                .description(format!(
                    "“{title}” and its history will be deleted. This cannot be undone."
                ))
                .ok_text("Delete")
                .on_ok(move |_, _, cx| {
                    let command = json!({ "type": "thread.delete", "threadId": thread_id });
                    app.update(cx, |this, cx| this.dispatch_to(&env, command, cx))
                        .ok();
                    true
                })
        });
    })
}

/// Jittered, capped exponential backoff: 0.25–0.5s at first, up to 15–30s.
fn backoff(attempt: u32) -> Duration {
    let cap = (500u64 << attempt.min(6)).min(30_000);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    Duration::from_millis(cap / 2 + u64::from(nanos) % (cap / 2 + 1))
}

#[cfg(test)]
mod e2e;

#[cfg(test)]
mod tests {
    #[test]
    fn backoff_is_bounded() {
        for attempt in 0..20 {
            let d = super::backoff(attempt).as_millis();
            assert!((250..=30_000).contains(&d), "{attempt}: {d}");
        }
        assert!(super::backoff(0).as_millis() <= 500);
    }
}
