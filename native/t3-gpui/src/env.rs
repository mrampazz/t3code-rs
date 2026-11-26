//! One connection per environment (web: `client-runtime/src/connection/registry.ts`).
//! Each environment owns its session, socket, shell and config; thread, git and terminal
//! streams follow the active environment (the one owning the open thread or draft).

use std::sync::LazyLock;
use std::time::Duration;

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
    /// Cuts a reconnect backoff short (window focus, "Retry now").
    pub wake: std::sync::Arc<tokio::sync::Notify>,
    pub error: Option<SharedString>,
    /// A token renewal is in flight (`renew_session`).
    pub renewing: bool,
    /// The server's `environmentId`: one server reached by two addresses is one environment.
    pub environment_id: Option<String>,
    /// The descriptor's `label` and `platform.machine` (web: `EnvironmentMachineIcon`).
    pub name: Option<String>,
    pub machine: Option<String>,
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
                    wake: Default::default(),
                    error: None,
                    renewing: false,
                    environment_id: None,
                    name: None,
                    machine: None,
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
                // A renewal while connected replaced the token; reconnect with that one.
                if let Ok(Some(current)) =
                    this.update(cx, |app, _| app.env_by(&key).map(|e| e.session.clone()))
                {
                    session = current;
                }
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
                    let client = RpcClient::connect(&url).await?;
                    // Which shell omissions the server supports (`shellOlderSettled`, ...).
                    let descriptor = auth::descriptor(&s.base_url).await;
                    anyhow::Ok((s, client, descriptor))
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
                    Ok((renewed, client, descriptor)) => {
                        session = renewed.clone();
                        let caps = descriptor["capabilities"].clone();
                        let environment_id =
                            descriptor["environmentId"].as_str().map(str::to_owned);
                        let Ok(Some(after)) = this.update(cx, |app, cx| {
                            if !alive(app) {
                                client.close();
                                return None;
                            }
                            let known: Vec<(&str, Option<&str>)> = app
                                .envs
                                .iter()
                                .map(|e| (e.key.as_str(), e.environment_id.as_deref()))
                                .collect();
                            if let Some(drop) = duplicate_to_drop(
                                &known,
                                &key,
                                environment_id.as_deref(),
                                app.active.as_deref(),
                            ) {
                                if drop == key {
                                    client.close();
                                }
                                app.remove_env(&drop, cx);
                                if drop == key {
                                    return None;
                                }
                            }
                            let active = app.env().is_some_and(|e| e.key == key);
                            let env = app.env_by_mut(&key)?;
                            env.session = renewed;
                            env.environment_id = environment_id;
                            env.name = descriptor["label"].as_str().map(str::to_owned);
                            env.machine = descriptor["platform"]["machine"]
                                .as_str()
                                .map(str::to_owned);
                            env.client = Some(client.clone());
                            env.error = None;
                            env.shell.omit_subagents = caps["shellThreadFamilies"] == true;
                            let after = env.shell.sequence;
                            app.subscribe_config(&key, &client, cx);
                            app.renew_session(&key, cx);
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
                        if caps["shellThreadFamilies"] == true {
                            payload["omitIdleSubagents"] = json!(true);
                        }
                        if caps["shellOlderSettled"] == true {
                            payload["omitOlderSettled"] = json!(true);
                        }
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
                                app.observe_statuses(&key);
                                if app.env().is_some_and(|e| e.key == key) {
                                    app.maybe_visit(cx);
                                }
                                if reload {
                                    app.load_archived(&key, cx);
                                }
                                if item["kind"] == "synchronized" {
                                    app.restore_last_thread(&key, cx);
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
                let wake = this
                    .update(cx, |app, _| app.env_by(&key).map(|e| e.wake.clone()))
                    .ok()
                    .flatten();
                let timer = cx.background_executor().timer(delay);
                match wake {
                    Some(wake) => {
                        let woken = wake.notified();
                        futures::pin_mut!(timer, woken);
                        futures::future::select(timer, woken).await;
                    }
                    None => timer.await,
                }
            }
        })
        .detach();
    }

    /// Coming back to the app (web: supervisor wakeups): disconnected environments retry now, and
    /// each live socket must answer `server.probe` in time or it is replaced. A socket that sat
    /// through a long absence (sleep, a network change) gets little time, since it is often dead.
    pub(crate) fn wake(&mut self, inactive: Option<Duration>, cx: &mut Context<Self>) {
        self.wake_disconnected();
        let keys: Vec<String> = self.envs.iter().map(|e| e.key.clone()).collect();
        for key in keys {
            self.renew_session(&key, cx);
        }
        let timeout = probe_timeout(inactive);
        for client in self.envs.iter().filter_map(|e| e.client.clone()) {
            cx.spawn(async move |_, _| {
                let probe = client.clone();
                let answered = on_tokio(async move {
                    tokio::time::timeout(timeout, probe.call("server.probe", json!({}))).await
                })
                .await;
                if !matches!(answered, Ok(Ok(Ok(_)))) {
                    // Closing ends the shell stream, and the connect loop reconnects at once.
                    client.close();
                }
            })
            .detach();
        }
    }

    /// Retries every disconnected environment now instead of after its backoff.
    pub(crate) fn wake_disconnected(&self) {
        for env in self.envs.iter().filter(|e| e.client.is_none()) {
            env.wake.notify_waiters();
        }
    }

    /// Forgets an environment: closes its socket and saved session. With none left, pairing shows.
    pub(crate) fn remove_env(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.env().is_some_and(|e| e.key == key) {
            self.release_streams(cx);
            self.selected = None;
            self.draft_project = None;
            self.settings_open = false;
            self.release_access();
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

/// How long before expiry a connected DPoP session renews its token.
const RENEW_BEFORE: Duration = Duration::from_secs(60);

impl T3App {
    /// Keeps a connected DPoP session's token fresh so HTTP calls keep working past its expiry:
    /// renews when due, else sleeps until it is. Also runs on wakeup, in case sleep outlasted
    /// the timer. A rejected renewal is left to the connect loop, which asks to pair again.
    pub(crate) fn renew_session(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(env) = self.env_by_mut(key) else {
            return;
        };
        let Some(expires_at) = env.session.expires_at.filter(|_| env.session.is_dpop()) else {
            return;
        };
        if env.renewing {
            return;
        }
        env.renewing = true;
        let (key, session, generation) = (key.to_owned(), env.session.clone(), env.generation);
        let now = jiff::Timestamp::now().as_millisecond();
        let due = Duration::from_millis(
            (expires_at - now - RENEW_BEFORE.as_millis() as i64).max(0) as u64,
        );
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(due).await;
            // A reconnect may have renewed meanwhile; only the token still in use is renewed.
            let current = this
                .update(cx, |app, _| {
                    app.env_by(&key)
                        .filter(|e| e.generation == generation)
                        .is_some_and(|e| e.session.access_token == session.access_token)
                })
                .unwrap_or(false);
            let renewed = if current {
                Some(on_tokio(async move { auth::renew(&session).await }).await)
            } else {
                None
            };
            this.update(cx, |app, cx| {
                let Some(env) = app.env_by_mut(&key) else {
                    return;
                };
                env.renewing = false;
                match renewed {
                    Some(Ok(Ok(session))) if env.generation == generation => {
                        env.session = session;
                        app.renew_session(&key, cx);
                    }
                    // A failed renewal is the connect loop's to report; a stale timer re-arms.
                    Some(_) => {}
                    None => app.renew_session(&key, cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Inactive this long, a socket is presumed dead unless it answers at once.
const LONG_INACTIVE: Duration = Duration::from_secs(600);

/// How long a woken live socket has to answer its probe (`CONNECTION_PROBE_TIMEOUT`,
/// `LONG_RESUME_PROBE_TIMEOUT`).
pub fn probe_timeout(inactive: Option<Duration>) -> Duration {
    if inactive.is_some_and(|d| d >= LONG_INACTIVE) {
        Duration::from_millis(1500)
    } else {
        Duration::from_secs(15)
    }
}

/// Rows per page of older settled threads (web: one "Show more" step).
/// When `key` turns out to be a server already connected under another address (the same
/// `environmentId`, e.g. `localhost` and `127.0.0.1`), the address to forget: the open one
/// stays, else the one connected first.
pub(crate) fn duplicate_to_drop(
    known: &[(&str, Option<&str>)],
    key: &str,
    environment_id: Option<&str>,
    active: Option<&str>,
) -> Option<String> {
    let id = environment_id?;
    let (other, _) = known.iter().find(|(k, e)| *k != key && *e == Some(id))?;
    Some(if active == Some(key) { other } else { key }.to_string())
}

const OLDER_SETTLED_PAGE: usize = 25;

impl T3App {
    /// Reads the next page of older settled threads in every environment that has more.
    pub(crate) fn load_older_settled(&mut self, cx: &mut Context<Self>) {
        for env in self.envs.iter_mut() {
            let (Some(client), Some(cursor)) = (env.client.clone(), env.shell.older_cursor.clone())
            else {
                continue;
            };
            if env.shell.older_loading || env.shell.older_remaining() == 0 {
                continue;
            }
            env.shell.older_loading = true;
            let key = env.key.clone();
            cx.spawn(async move |this, cx| {
                let page = on_tokio(async move {
                    client
                        .call(
                            "orchestration.getOlderSettledThreads",
                            json!({ "before": cursor, "limit": OLDER_SETTLED_PAGE }),
                        )
                        .await
                })
                .await;
                this.update(cx, |app, cx| {
                    let Some(env) = app.env_by_mut(&key) else {
                        return;
                    };
                    env.shell.older_loading = false;
                    match page {
                        Ok(Ok(page)) => {
                            let threads = page["threads"].as_array().cloned().unwrap_or_default();
                            env.shell.older_cursor = threads
                                .last()
                                .filter(|_| threads.len() == OLDER_SETTLED_PAGE)
                                .map(
                                    |t| json!({ "settledAt": t["settledAt"], "threadId": t["id"] }),
                                );
                            env.shell.add_threads(&page["threads"]);
                        }
                        Ok(Err(e)) => app.error = Some(e.to_string().into()),
                        Err(e) => app.error = Some(e.to_string().into()),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Reads a thread's family (`getThreadFamily`) when its shell left the thread out, as it
    /// does idle subagent children, so the thread can open.
    pub(crate) fn load_family(&mut self, env: &str, id: &str, cx: &mut Context<Self>) {
        let Some(e) = self.env_by(env) else { return };
        let Some(client) = e
            .client
            .clone()
            .filter(|_| !e.shell.threads.iter().any(|t| t.id == id))
        else {
            return;
        };
        let (key, id) = (env.to_owned(), id.to_owned());
        cx.spawn(async move |this, cx| {
            let family = on_tokio(async move {
                client
                    .call("orchestration.getThreadFamily", json!({ "threadId": id }))
                    .await
            })
            .await;
            if let Ok(Ok(family)) = family {
                this.update(cx, |app, cx| {
                    if let Some(env) = app.env_by_mut(&key) {
                        env.shell.add_threads(&family["threads"]);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod probe_tests {
    use super::probe_timeout;
    use std::time::Duration;

    #[test]
    fn long_absences_get_a_quick_probe() {
        assert_eq!(probe_timeout(None), Duration::from_secs(15));
        assert_eq!(
            probe_timeout(Some(Duration::from_secs(30))),
            Duration::from_secs(15)
        );
        assert_eq!(
            probe_timeout(Some(Duration::from_secs(3600))),
            Duration::from_millis(1500)
        );
    }
}
