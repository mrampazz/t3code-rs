//! One connection per environment (web: `client-runtime/src/connection/registry.ts`).
//! Each environment owns its session, socket, shell and config; thread, git and terminal
//! streams follow the active environment (the one owning the open thread or draft).

use std::sync::LazyLock;

use gpui_kit::*;
use serde_json::json;

use crate::model::{ServerConfig, Shell};
use crate::rpc::RpcClient;
use crate::{Screen, T3App, auth, backoff, on_tokio};

pub struct Env {
    /// The server URL; environments are keyed by it.
    pub key: String,
    pub session: auth::Session,
    pub client: Option<RpcClient>,
    pub shell: Shell,
    pub config: ServerConfig,
    /// Every terminal on the environment (`subscribeTerminalMetadata`).
    pub terminals: crate::terminal::TerminalList,
    /// Bumped per connect or removal so a stale supervisor loop stops.
    pub generation: u64,
    pub error: Option<SharedString>,
}

impl Env {
    /// Host and port, for labels.
    pub fn label(&self) -> String {
        self.key.split("://").nth(1).unwrap_or(&self.key).to_owned()
    }
}

static EMPTY_SHELL: LazyLock<Shell> = LazyLock::new(Shell::default);
static EMPTY_CONFIG: LazyLock<ServerConfig> = LazyLock::new(ServerConfig::default);

impl T3App {
    /// The active environment, else the first one.
    pub(crate) fn env(&self) -> Option<&Env> {
        self.active
            .as_ref()
            .and_then(|k| self.env_by(k))
            .or(self.envs.first())
    }

    pub(crate) fn env_mut(&mut self) -> Option<&mut Env> {
        let key = self.env()?.key.clone();
        self.env_by_mut(&key)
    }

    pub(crate) fn env_by(&self, key: &str) -> Option<&Env> {
        self.envs.iter().find(|e| e.key == key)
    }

    pub(crate) fn env_by_mut(&mut self, key: &str) -> Option<&mut Env> {
        self.envs.iter_mut().find(|e| e.key == key)
    }

    pub(crate) fn client(&self) -> Option<RpcClient> {
        self.env()?.client.clone()
    }

    pub(crate) fn shell(&self) -> &Shell {
        self.env().map_or(&EMPTY_SHELL, |e| &e.shell)
    }

    pub(crate) fn config(&self) -> &ServerConfig {
        self.env().map_or(&EMPTY_CONFIG, |e| &e.config)
    }

    /// Cancels the active environment's thread, git and terminal streams on its own socket.
    /// Request ids are per socket, so this must run before the active environment changes.
    pub(crate) fn release_streams(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.client() {
            for sub in [
                self.thread_sub.take(),
                self.git_sub.take(),
                self.term_sub.take(),
                self.setup_sub.take(),
            ]
            .into_iter()
            .flatten()
            {
                client.cancel(&sub);
            }
        }
        self.term = None;
        self.term_status = None;
        cx.notify();
    }

    /// Adds (or reconnects) an environment and owns its transport: connect, stream the shell,
    /// retry with backoff.
    pub(crate) fn connect(&mut self, session: auth::Session, cx: &mut Context<Self>) {
        let key = session.base_url.clone();
        let generation = match self.env_by_mut(&key) {
            Some(env) => {
                env.generation += 1;
                env.session = session.clone();
                env.generation
            }
            None => {
                self.envs.push(Env {
                    key: key.clone(),
                    session: session.clone(),
                    client: None,
                    shell: Shell::new(&key),
                    config: ServerConfig::default(),
                    terminals: Default::default(),
                    generation: 1,
                    error: None,
                });
                1
            }
        };
        if self.active.is_none() {
            self.active = Some(key.clone());
        }
        if self.envs.iter().all(|e| e.shell.sequence == 0) {
            self.screen = Screen::Connecting;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut session = session;
            let alive = |app: &T3App| app.env_by(&key).is_some_and(|e| e.generation == generation);
            let mut attempt = 0u32;
            loop {
                let s = session.clone();
                let result = on_tokio(async move {
                    // A renewable DPoP session renews itself just before its token expires.
                    let now = jiff::Timestamp::now().as_millisecond();
                    let s = if s.needs_renewal(now) {
                        auth::renew(&s).await?
                    } else {
                        s
                    };
                    let url = auth::socket_url(&s).await?;
                    anyhow::Ok((s, RpcClient::connect(&url).await?))
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r);
                let error = match result {
                    Err(e) if e.is::<auth::Rejected>() => {
                        this.update(cx, |app, cx| {
                            if alive(app) {
                                app.remove_env(&key, cx);
                                app.error = Some(e.to_string().into());
                            }
                        })
                        .ok();
                        return;
                    }
                    Err(e) => e.to_string(),
                    Ok((renewed, client)) => {
                        session = renewed.clone();
                        let Ok(Some(after)) = this.update(cx, |app, cx| {
                            if !alive(app) {
                                client.close();
                                return None;
                            }
                            let active = app.env().is_some_and(|e| e.key == key);
                            let env = app.env_by_mut(&key)?;
                            env.session = renewed;
                            env.client = Some(client.clone());
                            env.error = None;
                            let after = env.shell.sequence;
                            app.subscribe_config(&key, &client, cx);
                            app.subscribe_terminal_list(&key, &client, cx);
                            app.screen = Screen::Connected;
                            if active && app.selected.is_some() {
                                app.subscribe_thread(cx);
                                app.subscribe_git(cx);
                                if app.term.is_some() {
                                    app.open_terminal(true, cx);
                                }
                            }
                            cx.notify();
                            Some(after)
                        }) else {
                            return;
                        };
                        // The server sends `synchronized` only when asked; backoff resets on it.
                        let mut payload = json!({ "requestCompletionMarker": true });
                        if after > 0 {
                            payload["afterSequence"] = json!(after);
                        }
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
                                if !alive(app) {
                                    return false;
                                }
                                let env = app.env_by_mut(&key).expect("alive");
                                let removals = env.shell.removals;
                                env.shell.apply(&item);
                                let reload = env.shell.removals != removals
                                    || item["kind"] == "synchronized";
                                if app.env().is_some_and(|e| e.key == key) {
                                    app.maybe_visit(cx);
                                }
                                if reload {
                                    app.load_archived(&key, cx);
                                }
                                cx.notify();
                                true
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
                    if !alive(app) {
                        return false;
                    }
                    if app.env().is_some_and(|e| e.key == key) {
                        app.thread_sub = None;
                        app.git_sub = None;
                        app.term_sub = None;
                    }
                    let env = app.env_by_mut(&key).expect("alive");
                    env.client = None;
                    env.error = Some(
                        format!(
                            "Disconnected: {error}. Retrying in {}s…",
                            delay.as_secs().max(1)
                        )
                        .into(),
                    );
                    cx.notify();
                    true
                }) else {
                    return;
                };
                cx.background_executor().timer(delay).await;
            }
        })
        .detach();
    }

    /// Forgets an environment: closes its socket and saved session. With none left, pairing shows.
    pub(crate) fn remove_env(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.env().is_some_and(|e| e.key == key) {
            self.release_streams(cx);
            self.selected = None;
            self.draft_project = None;
            self.settings_open = false;
            self.thread = Default::default();
            self.active = None;
        }
        if let Some(env) = self.env_by(key)
            && let Some(client) = &env.client
        {
            client.close();
        }
        self.envs.retain(|e| e.key != key);
        auth::forget(key);
        if self.envs.is_empty() {
            self.screen = Screen::Pair;
        }
        cx.notify();
    }

    /// Refreshes an environment's Archived list; on sync and whenever its shell reports a removal.
    pub(crate) fn load_archived(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(client) = self.env_by(key).and_then(|e| e.client.clone()) else {
            return;
        };
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                client
                    .call("orchestration.getArchivedShellSnapshot", json!({}))
                    .await
            })
            .await;
            if let Ok(Ok(snapshot)) = result {
                this.update(cx, |app, cx| {
                    if let Some(env) = app.env_by_mut(&key) {
                        env.shell.set_archived(&snapshot);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Lives as long as the socket; a new connection starts a new one.
    fn subscribe_config(&mut self, key: &str, client: &RpcClient, cx: &mut Context<Self>) {
        let (_, mut rx) = client.request("subscribeServerConfig", json!({}));
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            while let Some(Ok(event)) = rx.recv().await {
                let applied = this.update(cx, |app, cx| {
                    if let Some(env) = app.env_by_mut(&key) {
                        env.config.apply(&event);
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Lives as long as the socket, like the config stream.
    fn subscribe_terminal_list(&mut self, key: &str, client: &RpcClient, cx: &mut Context<Self>) {
        let (_, mut rx) = client.request("subscribeTerminalMetadata", json!({}));
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            while let Some(Ok(event)) = rx.recv().await {
                let applied = this.update(cx, |app, cx| {
                    if let Some(env) = app.env_by_mut(&key) {
                        env.terminals.apply(&event);
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Makes `key` the active environment, releasing the previous one's streams first.
    pub(crate) fn activate(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.env().is_some_and(|e| e.key == key) {
            return;
        }
        self.release_streams(cx);
        self.selected = None;
        self.thread = Default::default();
        self.active = Some(key.to_owned());
    }
}
