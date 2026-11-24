mod auth;
mod model;
mod rpc;
mod timeline;

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;

use model::{ServerConfig, Shell, ThreadView, model_key};
use rpc::RpcClient;

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
    session: Option<auth::Session>,
    /// Bumped per connect or disconnect so a stale supervisor loop stops.
    generation: u64,
    pair_input: Entity<InputState>,
    composer: Entity<InputState>,
    model_picker: Entity<ModelPicker>,
    /// What the picker was last synced to: config revision and selected key.
    picker_synced: (u64, String),
    /// Model chosen for the new-thread draft; `None` uses the default.
    draft_model: Option<serde_json::Value>,
    client: Option<RpcClient>,
    shell: Shell,
    config: ServerConfig,
    /// Project of the new-thread draft shown instead of a thread.
    draft_project: Option<String>,
    selected: Option<String>,
    thread: ThreadView,
    thread_sub: Option<String>,
    /// Timeline rows the user opened.
    expanded: HashSet<String>,
    show_archived: bool,
    transcript_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl T3App {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pair_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://host:3773/pair#token=…"));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Ask anything…"));
        let model_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<ModelOption>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let _subscriptions = vec![
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
            cx.subscribe_in(&composer, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { shift: false, .. }) {
                    this.send(window, cx);
                }
            }),
        ];
        let mut app = Self {
            screen: Screen::Pair,
            error: None,
            session: None,
            generation: 0,
            pair_input,
            composer,
            model_picker,
            picker_synced: (u64::MAX, String::new()),
            draft_model: None,
            client: None,
            shell: Shell::default(),
            config: ServerConfig::default(),
            draft_project: None,
            selected: None,
            thread: ThreadView::default(),
            thread_sub: None,
            expanded: HashSet::new(),
            show_archived: false,
            transcript_scroll: ScrollHandle::new(),
            _subscriptions,
        };
        if let Some(session) = auth::load() {
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
                Ok(Ok(session)) => app.connect(session, cx),
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

    /// Owns the transport for one session: connects, streams the shell, and retries with backoff.
    fn connect(&mut self, session: auth::Session, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.session = Some(session.clone());
        if self.shell.sequence == 0 {
            self.screen = Screen::Connecting;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let alive = |app: &T3App| app.generation == generation;
            let mut attempt = 0u32;
            loop {
                let s = session.clone();
                let result = on_tokio(async move {
                    let url = auth::socket_url(&s).await?;
                    RpcClient::connect(&url).await
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r);
                let error = match result {
                    Err(e) if e.is::<auth::Rejected>() => {
                        this.update(cx, |app, cx| {
                            if alive(app) {
                                auth::forget();
                                app.session = None;
                                app.fail(e.to_string(), cx);
                            }
                        })
                        .ok();
                        return;
                    }
                    Err(e) => e.to_string(),
                    Ok(client) => {
                        let Ok(Some(after)) = this.update(cx, |app, cx| {
                            if !alive(app) {
                                client.close();
                                return None;
                            }
                            app.client = Some(client.clone());
                            app.subscribe_config(&client, cx);
                            app.screen = Screen::Connected;
                            app.error = None;
                            if app.selected.is_some() {
                                app.subscribe_thread(cx);
                            }
                            cx.notify();
                            Some(app.shell.sequence)
                        }) else {
                            return;
                        };
                        let payload = if after > 0 {
                            json!({ "afterSequence": after })
                        } else {
                            json!({})
                        };
                        let (_, mut shell) =
                            client.request("orchestration.subscribeShell", payload);
                        let mut ended = "connection closed".to_string();
                        while let Some(item) = shell.recv().await {
                            let item = match item {
                                Ok(item) => item,
                                Err(e) => {
                                    ended = e.to_string();
                                    break;
                                }
                            };
                            let Ok(true) = this.update(cx, |app, cx| {
                                app.shell.apply(&item);
                                cx.notify();
                                alive(app)
                            }) else {
                                client.close();
                                return;
                            };
                            if item["kind"] == "synchronized" {
                                attempt = 0;
                            }
                        }
                        client.close();
                        ended
                    }
                };
                let delay = backoff(attempt);
                attempt += 1;
                let Ok(true) = this.update(cx, |app, cx| {
                    if alive(app) {
                        app.client = None;
                        app.thread_sub = None;
                        app.error = Some(
                            format!(
                                "Disconnected: {error}. Retrying in {}s…",
                                delay.as_secs().max(1)
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                    alive(app)
                }) else {
                    return;
                };
                cx.background_executor().timer(delay).await;
            }
        })
        .detach();
    }

    /// Lives as long as the socket; a new connection starts a new one.
    fn subscribe_config(&mut self, client: &RpcClient, cx: &mut Context<Self>) {
        let (_, mut rx) = client.request("subscribeServerConfig", json!({}));
        cx.spawn(async move |this, cx| {
            while let Some(Ok(event)) = rx.recv().await {
                if this
                    .update(cx, |app, cx| {
                        app.config.apply(&event);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn new_thread(&mut self, project_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(client), Some(sub)) = (&self.client, self.thread_sub.take()) {
            client.cancel(&sub);
        }
        self.selected = None;
        self.thread = ThreadView::default();
        self.draft_project = Some(project_id);
        self.draft_model = None;
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn launch(&mut self, project_id: String, text: String, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let project = self.shell.projects.iter().find(|p| p.id == project_id);
        let Some(model) = self
            .draft_model
            .clone()
            .or_else(|| self.config.model_for(project))
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
            "runtimeMode": "full-access",
            "interactionMode": "default",
            "workspaceStrategy": { "type": "root" },
            "initialMessage": { "messageId": command_id, "text": text, "attachments": [] },
        });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("orchestration.launchThread", input).await })
                    .await;
            this.update(cx, |app, cx| match result {
                Ok(Ok(launched)) => {
                    app.draft_project = None;
                    if let Some(id) = launched["threadId"].as_str() {
                        app.select_thread(id.to_string(), cx);
                    }
                }
                Ok(Err(e)) => app.error = Some(format!("Could not start thread: {e}").into()),
                Err(e) => app.error = Some(e.to_string().into()),
            })
            .ok();
        })
        .detach();
    }

    fn select_thread(&mut self, id: String, cx: &mut Context<Self>) {
        self.draft_project = None;
        if self.selected.as_deref() != Some(id.as_str()) {
            self.thread = ThreadView::default();
        }
        self.selected = Some(id);
        self.subscribe_thread(cx);
    }

    /// (Re)subscribes the selected thread, resuming from its cursor when it already has data.
    fn subscribe_thread(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(id)) = (self.client.clone(), self.selected.clone()) else {
            return;
        };
        if let Some(old) = self.thread_sub.take() {
            client.cancel(&old);
        }
        let mut payload = json!({ "threadId": id });
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
                        Ok(item) => app.thread.apply(&item),
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
        if text.is_empty() {
            return;
        }
        if let Some(project_id) = self.draft_project.clone() {
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            return self.launch(project_id, text, cx);
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
                "attachments": [],
                "dispatchMode": { "type": "queue_after_active" },
                "createdBy": "user",
                "creationSource": "web",
            }),
            cx,
        );
    }

    /// Sends an `OrchestrationV2Command`, filling in `commandId`. Failures show in the error line.
    pub(crate) fn dispatch(&mut self, mut command: serde_json::Value, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
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
            let project = self.shell.projects.iter().find(|p| &p.id == project_id);
            return self
                .draft_model
                .clone()
                .or_else(|| self.config.model_for(project));
        }
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell.threads.iter().find(|t| &t.id == id))?;
        Some(thread.model_selection.clone())
    }

    fn pick_model(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((_, selection)) = self
            .config
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
        if self.picker_synced == (self.config.revision, key.clone()) {
            return;
        }
        let items: Vec<ModelOption> = self
            .config
            .model_options()
            .into_iter()
            .map(|(title, selection)| ModelOption {
                title: title.into(),
                key: model_key(&selection).into(),
            })
            .collect();
        let rebuild = self.picker_synced.0 != self.config.revision;
        self.picker_synced = (self.config.revision, key.clone());
        self.model_picker.update(cx, |picker, cx| {
            if rebuild {
                picker.set_items(SearchableVec::new(items), window, cx);
            }
            picker.set_selected_value(&SharedString::from(key), window, cx);
        });
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| self.shell.threads.iter().find(|t| &t.id == id))
        else {
            return;
        };
        let command = match (&thread.active_run_id, self.config.thread_stop) {
            (_, true) => json!({ "type": "thread.stop", "threadId": thread.id }),
            (Some(run), false) => {
                json!({ "type": "run.interrupt", "threadId": thread.id, "runId": run })
            }
            (None, false) => return,
        };
        self.dispatch(command, cx);
    }

    fn forget_session(&mut self, cx: &mut Context<Self>) {
        auth::forget();
        self.generation += 1;
        self.session = None;
        self.shell = Shell::default();
        self.thread = ThreadView::default();
        if let Some(client) = self.client.take() {
            client.close();
        }
        self.thread_sub = None;
        self.selected = None;
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
                ),
        )
    }

    fn thread_row(
        &self,
        thread: &model::ThreadShell,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().colors;
        let id = thread.id.clone();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let app = cx.entity().downgrade();
        let (pinned, settled) = (thread.pinned_at.is_some(), thread.settled_at.is_some());
        let menu_id = id.clone();
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
                    .when(thread.is_running(), |d| d.bg(theme.success)),
            )
            .child(div().flex_1().truncate().child(thread.title.clone()))
            .when(pinned, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("📌"),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_thread(id.clone(), cx)))
            .context_menu(move |menu, _, _| {
                let action = |label: &str, kind: &str, extra: serde_json::Value| {
                    let app = app.clone();
                    let mut command = json!({ "type": kind, "threadId": menu_id });
                    if let (Some(command), Some(extra)) =
                        (command.as_object_mut(), extra.as_object())
                    {
                        command.extend(extra.clone());
                    }
                    PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| {
                        app.update(cx, |this, cx| this.dispatch(command.clone(), cx))
                            .ok();
                    })
                };
                if archived {
                    return menu.item(action("Unarchive", "thread.unarchive", json!({})));
                }
                menu.item(if pinned {
                    action("Unpin", "thread.unpin", json!({}))
                } else {
                    action("Pin", "thread.pin", json!({}))
                })
                .item(if settled {
                    action("Unsettle", "thread.unsettle", json!({ "reason": "user" }))
                } else {
                    action("Settle", "thread.settle", json!({}))
                })
                .item(action("Mark unread", "thread.mark-unread", json!({})))
                .separator()
                .item(action("Archive", "thread.archive", json!({})))
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().colors;
        let mut list = v_flex()
            .id("threads")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .pb_2();
        for project in &self.shell.projects {
            let project_id = project.id.clone();
            list = list.child(
                h_flex()
                    .pl_2()
                    .pt_3()
                    .pb_1()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(project.title.clone())
                    .child(
                        Button::new(SharedString::from(format!("new-{project_id}")))
                            .ghost()
                            .xsmall()
                            .label("+")
                            .tooltip("New thread")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.new_thread(project_id.clone(), window, cx)
                            })),
                    ),
            );
            for thread in self.shell.threads_of(&project.id) {
                list = list.child(self.thread_row(thread, false, cx));
            }
        }
        if !self.shell.archived.is_empty() {
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
                        self.shell.archived.len()
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_archived = !this.show_archived;
                        cx.notify();
                    })),
            );
            if open {
                for thread in &self.shell.archived {
                    list = list.child(self.thread_row(thread, true, cx));
                }
            }
        }
        v_flex()
            .w(px(280.))
            .h_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("t3code-rs"))
                    .child(
                        Button::new("forget")
                            .ghost()
                            .label("Disconnect")
                            .on_click(cx.listener(|this, _, _, cx| this.forget_session(cx))),
                    ),
            )
            .child(list)
    }

    fn render_thread(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().colors;
        if let Some(project) = self
            .draft_project
            .as_ref()
            .and_then(|id| self.shell.projects.iter().find(|p| &p.id == id))
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
                            Some(_) => div()
                                .w(px(320.))
                                .child(Select::new(&self.model_picker))
                                .into_any_element(),
                            None => div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No available provider")
                                .into_any_element(),
                        })
                        .child(Input::new(&self.composer))
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
            .and_then(|id| self.shell.threads.iter().find(|t| &t.id == id))
        else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(if self.shell.synced || !self.shell.projects.is_empty() {
                    "Select a thread"
                } else {
                    "Loading…"
                })
                .into_any_element();
        };
        let items: Vec<AnyElement> = self
            .thread
            .items()
            .map(|item| self.render_item(item, cx))
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
                            .child(self.config.label(&thread.model_selection)),
                    )
                    .child(div().flex_1())
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
                            .children(items),
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
                    .child(Input::new(&self.composer))
                    .child(
                        div()
                            .w(px(280.))
                            .child(Select::new(&self.model_picker).small()),
                    ),
            )
            .into_any_element()
    }
}

impl Render for T3App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_picker(window, cx);
        let theme = cx.theme();
        let root = h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground);
        match self.screen {
            Screen::Connected => root
                .child(self.render_sidebar(cx))
                .child(self.render_thread(cx)),
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
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
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
mod e2e {
    #[test]
    fn backoff_is_bounded() {
        for attempt in 0..20 {
            let d = super::backoff(attempt).as_millis();
            assert!((250..=30_000).contains(&d), "{attempt}: {d}");
        }
        assert!(super::backoff(0).as_millis() <= 500);
    }

    /// `T3_PAIRING_URL=… cargo test e2e -- --ignored --nocapture` against a dev server.
    #[tokio::test]
    #[ignore]
    async fn pairs_and_streams_shell() {
        let url = std::env::var("T3_PAIRING_URL").expect("T3_PAIRING_URL");
        let session = crate::auth::pair(&url).await.unwrap();
        let client =
            crate::rpc::RpcClient::connect(&crate::auth::socket_url(&session).await.unwrap())
                .await
                .unwrap();
        let (_, mut rx) = client.request("orchestration.subscribeShell", serde_json::json!({}));
        let mut shell = crate::model::Shell::default();
        while !shell.synced {
            let item = rx.recv().await.unwrap().unwrap();
            println!("shell item: {}", item["kind"]);
            shell.apply(&item);
        }
        println!(
            "projects={} threads={}",
            shell.projects.len(),
            shell.threads.len()
        );
    }
}
