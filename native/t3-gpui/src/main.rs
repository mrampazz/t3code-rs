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
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::json;

use model::{ServerConfig, Shell, ThreadView};
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
    transcript_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl T3App {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pair_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://host:3773/pair#token=…"));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Ask anything…"));
        let _subscriptions = vec![
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
            client: None,
            shell: Shell::default(),
            config: ServerConfig::default(),
            draft_project: None,
            selected: None,
            thread: ThreadView::default(),
            thread_sub: None,
            expanded: HashSet::new(),
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
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn launch(&mut self, project_id: String, text: String, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let project = self.shell.projects.iter().find(|p| p.id == project_id);
        let Some(model) = self.config.model_for(project) else {
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
        let (Some(client), Some(thread_id)) = (self.client.clone(), self.selected.clone()) else {
            return;
        };
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        let command_id = uuid::Uuid::new_v4().to_string();
        let command = json!({
            "type": "message.dispatch",
            "commandId": command_id,
            "messageId": command_id,
            "threadId": thread_id,
            "text": text,
            "attachments": [],
            "dispatchMode": { "type": "queue_after_active" },
            "createdBy": "user",
            "creationSource": "web",
        });
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { client.call("orchestration.dispatchCommand", command).await },
                )
                .await;
            if let Ok(Err(e)) = result {
                this.update(cx, |app, cx| {
                    app.error = Some(format!("Send failed: {e}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
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
                let id = thread.id.clone();
                let selected = self.selected.as_deref() == Some(id.as_str());
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("thread-{id}")))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
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
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_thread(id.clone(), cx)),
                        ),
                );
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
            let model = self.config.model_for(Some(project));
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
                        .child(div().text_sm().text_color(theme.muted_foreground).child(
                            match &model {
                                Some(m) => self.config.label(m),
                                None => "No available provider".to_string(),
                            },
                        ))
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
                    ),
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
            .child(
                div()
                    .mx_auto()
                    .max_w(px(760.))
                    .w_full()
                    .p_4()
                    .child(Input::new(&self.composer)),
            )
            .into_any_element()
    }
}

impl Render for T3App {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
