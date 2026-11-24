//! End-to-end checks against a live dev server (`vp run dev --home-dir <tmp> --no-seed`).
//! Run: `T3_PAIRING_URL=… cargo test e2e -- --ignored --nocapture --test-threads 1`
//! (or `T3_SERVER` + `T3_TOKEN` to reuse a session). Uses the `Replay (dev)` provider.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::model::{ServerConfig, Shell, ThreadView, build_answers, option_value};
use crate::rpc::RpcClient;
use crate::{auth, diff_panel};

const REPLAY: &str = "devReplay";
const WAIT: Duration = Duration::from_secs(60);

/// A stream's projection, updated by a background task, with a wakeup per item.
struct Live<T> {
    state: Arc<Mutex<T>>,
    changed: Arc<Notify>,
}

impl<T: Default + Send + 'static> Live<T> {
    fn start(client: &RpcClient, tag: &str, payload: Value, apply: fn(&mut T, &Value)) -> Self {
        let (_, mut rx) = client.request(tag, payload);
        let state = Arc::new(Mutex::new(T::default()));
        let changed = Arc::new(Notify::new());
        let (s, n) = (state.clone(), changed.clone());
        tokio::spawn(async move {
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(item) => apply(&mut s.lock().unwrap(), &item),
                    Err(e) => panic!("stream failed: {e}"),
                }
                n.notify_waiters();
            }
        });
        Self { state, changed }
    }

    async fn until<R>(&self, what: &str, check: impl Fn(&T) -> Option<R>) -> R {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let notified = self.changed.notified();
            if let Some(r) = check(&self.state.lock().unwrap()) {
                return r;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!(
                    "timed out waiting for {what}: {:?}",
                    Dbg(&*self.state.lock().unwrap())
                );
            }
        }
    }
}

/// Timeout context: the turn items of a thread view, or the thread statuses of a shell.
struct Dbg<'a, T>(&'a T);

impl<T: 'static> std::fmt::Debug for Dbg<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let any = self.0 as &dyn std::any::Any;
        if let Some(view) = any.downcast_ref::<ThreadView>() {
            return f
                .debug_list()
                .entries(view.items().map(|i| (&i.kind, i.str("status"))))
                .finish();
        }
        if let Some(shell) = any.downcast_ref::<Shell>() {
            return f
                .debug_list()
                .entries(shell.threads.iter().map(|t| (&t.title, &t.status)))
                .finish();
        }
        f.write_str("…")
    }
}

async fn session() -> auth::Session {
    if let (Ok(base_url), Ok(access_token)) =
        (std::env::var("T3_SERVER"), std::env::var("T3_TOKEN"))
    {
        return auth::Session {
            base_url,
            access_token,
        };
    }
    let url = std::env::var("T3_PAIRING_URL").expect("set T3_PAIRING_URL or T3_SERVER + T3_TOKEN");
    let session = auth::pair(&url).await.unwrap();
    println!(
        "paired; reuse with T3_SERVER={} T3_TOKEN={}",
        session.base_url, session.access_token
    );
    session
}

async fn dispatch(client: &RpcClient, mut command: Value) -> Value {
    command["commandId"] = json!(uuid::Uuid::new_v4().to_string());
    let kind = command["type"].clone();
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .unwrap_or_else(|e| panic!("{kind} failed: {e}"))
}

struct Env {
    client: RpcClient,
    shell: Live<Shell>,
    config: Live<ServerConfig>,
    project_id: String,
}

async fn connect() -> Env {
    let session = session().await;
    let client = RpcClient::connect(&auth::socket_url(&session).await.unwrap())
        .await
        .unwrap();
    let config = Live::start(
        &client,
        "subscribeServerConfig",
        json!({}),
        ServerConfig::apply,
    );
    let shell = Live::start(
        &client,
        "orchestration.subscribeShell",
        json!({ "requestCompletionMarker": true }),
        Shell::apply,
    );
    shell.until("shell sync", |s| s.synced.then_some(())).await;
    config
        .until("replay provider", |c| {
            c.providers
                .iter()
                .any(|p| p.instance_id == REPLAY)
                .then_some(())
        })
        .await;
    let name = format!("e2e-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let created = client
        .call("projects.createNew", json!({ "name": name }))
        .await
        .unwrap();
    let project_id = created["projectId"].as_str().unwrap().to_owned();
    shell
        .until("project in shell", |s| {
            s.projects.iter().any(|p| p.id == project_id).then_some(())
        })
        .await;
    Env {
        client,
        shell,
        config,
        project_id,
    }
}

/// Starts a thread on a replay scenario exactly as the app's `launch` does.
async fn launch(env: &Env, scenario: &str, text: &str) -> (String, Live<ThreadView>) {
    launch_in(env, scenario, text, "full-access").await
}

async fn launch_in(
    env: &Env,
    scenario: &str,
    text: &str,
    runtime_mode: &str,
) -> (String, Live<ThreadView>) {
    let id = uuid::Uuid::new_v4().to_string();
    let launched = env
        .client
        .call(
            "orchestration.launchThread",
            json!({
                "commandId": id,
                "creationSource": "web",
                "projectId": env.project_id,
                "title": text,
                "generateTitle": true,
                "modelSelection": { "instanceId": REPLAY, "model": scenario },
                "runtimeMode": runtime_mode,
                "interactionMode": "default",
                "workspaceStrategy": { "type": "root" },
                "initialMessage": { "messageId": id, "text": text, "attachments": [] },
            }),
        )
        .await
        .unwrap();
    let thread_id = launched["threadId"].as_str().unwrap().to_owned();
    let view = Live::start(
        &env.client,
        "orchestration.subscribeThread",
        json!({ "threadId": thread_id, "requestCompletionMarker": true }),
        ThreadView::apply,
    );
    (thread_id, view)
}

fn kinds(view: &ThreadView) -> Vec<String> {
    view.items().map(|i| i.kind.clone()).collect()
}

async fn wait_idle(env: &Env, thread_id: &str) {
    env.shell
        .until("thread idle", |s| {
            s.threads
                .iter()
                .find(|t| t.id == thread_id)
                .filter(|t| !t.is_running())
                .map(|_| ())
        })
        .await;
}

#[tokio::test]
#[ignore]
async fn e2e_chat_diff_and_model() {
    let env = connect().await;
    let models: Vec<String> = env
        .config
        .state
        .lock()
        .unwrap()
        .model_options()
        .iter()
        .map(|o| crate::model::model_key(&o.1))
        .collect();
    println!("models: {models:?}");

    let (thread_id, view) = launch(&env, "showcase", "Show me what you can do").await;
    view.until("assistant reply", |v| {
        kinds(v)
            .iter()
            .any(|k| k == "assistant_message")
            .then_some(())
    })
    .await;
    wait_idle(&env, &thread_id).await;
    println!("showcase items: {:?}", kinds(&view.state.lock().unwrap()));

    let found = env
        .client
        .call(
            "orchestration.searchThreads",
            json!({ "query": "what you can do", "limit": 20 }),
        )
        .await
        .unwrap();
    let rows = crate::palette::palette_rows(
        &env.shell.state.lock().unwrap(),
        found["matches"].as_array().unwrap(),
        "what you can do",
        |_| None,
    );
    assert!(
        rows.iter()
            .any(|r| r.action == crate::palette::PaletteAction::Open(thread_id.clone())),
        "palette finds the thread by message: {found}"
    );

    dispatch(
        &env.client,
        json!({
            "type": "message.dispatch", "threadId": thread_id, "messageId": uuid::Uuid::new_v4().to_string(),
            "text": "and again", "attachments": [], "dispatchMode": { "type": "queue_after_active" },
            "createdBy": "user", "creationSource": "web",
        }),
    )
    .await;
    view.until("second user message", |v| {
        (kinds(v).iter().filter(|k| *k == "user_message").count() >= 2).then_some(())
    })
    .await;
    wait_idle(&env, &thread_id).await;

    dispatch(
        &env.client,
        json!({ "type": "thread.model-selection.set", "threadId": thread_id,
                "modelSelection": { "instanceId": REPLAY, "model": "multi-turn" } }),
    )
    .await;
    env.shell
        .until("model switched", |s| {
            s.threads
                .iter()
                .find(|t| t.id == thread_id)
                .filter(|t| t.model_selection["model"] == "multi-turn")
                .map(|_| ())
        })
        .await;

    let latest = view.state.lock().unwrap().latest_turn();
    println!("checkpoints latest turn: {latest:?}");
    if let Some(turn) = latest {
        for (method, payload) in [
            (
                "orchestration.getFullThreadDiff",
                json!({ "threadId": thread_id, "toTurnCount": turn }),
            ),
            (
                "orchestration.getTurnDiff",
                json!({ "threadId": thread_id, "fromTurnCount": turn - 1, "toTurnCount": turn }),
            ),
        ] {
            let diff = env
                .client
                .call(method, payload)
                .await
                .unwrap_or_else(|e| panic!("{method}: {e}"));
            let (files, _) = diff_panel::parse_turn_diff(&diff).unwrap();
            println!("{method}: {} files", files.len());
        }
    }
}

#[tokio::test]
#[ignore]
async fn e2e_approval_and_question() {
    let env = connect().await;

    let (thread_id, view) = launch_in(&env, "approval", "write a file", "approval-required").await;
    let request = view
        .until("approval request", |v| {
            v.items()
                .find(|i| i.kind == "approval_request" && i.str("status") != "completed")
                .map(|i| i.str("requestId").to_owned())
        })
        .await;
    dispatch(
        &env.client,
        json!({ "type": "runtime-request.respond", "threadId": thread_id, "requestId": request, "decision": "accept" }),
    )
    .await;
    wait_idle(&env, &thread_id).await;
    println!("approval items: {:?}", kinds(&view.state.lock().unwrap()));

    let (thread_id, view) = launch(&env, "question", "ask me something").await;
    let (request, questions) = view
        .until("question", |v| {
            v.items().find(|i| i.kind == "user_input_request").map(|i| {
                (
                    i.str("requestId").to_owned(),
                    i.raw["questions"].as_array().cloned().unwrap_or_default(),
                )
            })
        })
        .await;
    println!("questions: {}", serde_json::to_string(&questions).unwrap());
    let first = |qid: &str| {
        questions
            .iter()
            .find(|q| q["id"] == qid)
            .and_then(|q| q["options"].as_array()?.first().map(option_value))
            .into_iter()
            .collect()
    };
    let answers = build_answers(&questions, first, "").expect("answers for every question");
    dispatch(
        &env.client,
        json!({ "type": "runtime-request.respond", "threadId": thread_id, "requestId": request, "answers": answers }),
    )
    .await;
    wait_idle(&env, &thread_id).await;
    println!("question items: {:?}", kinds(&view.state.lock().unwrap()));
}

#[tokio::test]
#[ignore]
async fn e2e_thread_actions() {
    let env = connect().await;
    let (thread_id, _view) = launch(&env, "multi-turn", "actions").await;
    wait_idle(&env, &thread_id).await;
    let thread = |s: &Shell| s.threads.iter().find(|t| t.id == thread_id).cloned();

    let cmd = |kind: &str, extra: Value| {
        let mut c = json!({ "type": kind, "threadId": thread_id });
        c.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        c
    };
    dispatch(&env.client, cmd("thread.pin", json!({}))).await;
    env.shell
        .until("pinned", |s| thread(s)?.pinned_at.map(|_| ()))
        .await;
    dispatch(&env.client, cmd("thread.unpin", json!({}))).await;
    env.shell
        .until("unpinned", |s| {
            thread(s).filter(|t| t.pinned_at.is_none()).map(|_| ())
        })
        .await;
    dispatch(&env.client, cmd("thread.settle", json!({}))).await;
    env.shell
        .until("settled", |s| thread(s)?.settled_at.map(|_| ()))
        .await;
    {
        let shell = env.shell.state.lock().unwrap();
        let sections =
            crate::sidebar::sections(&shell.threads, jiff::Timestamp::now().as_millisecond());
        assert!(
            sections.settled.iter().any(|t| t.id == thread_id),
            "settled section"
        );
    }
    dispatch(
        &env.client,
        cmd("thread.unsettle", json!({ "reason": "user" })),
    )
    .await;
    env.shell
        .until("unsettled", |s| {
            thread(s).filter(|t| t.settled_at.is_none()).map(|_| ())
        })
        .await;
    dispatch(
        &env.client,
        cmd(
            "thread.metadata.update",
            json!({ "title": "Renamed by e2e" }),
        ),
    )
    .await;
    env.shell
        .until("renamed", |s| {
            thread(s)
                .filter(|t| t.title == "Renamed by e2e")
                .map(|_| ())
        })
        .await;
    dispatch(&env.client, cmd("thread.mark-unread", json!({}))).await;
    let presets = crate::sidebar::snooze_presets(&jiff::Zoned::now(), None);
    dispatch(
        &env.client,
        cmd(
            "thread.snooze",
            json!({ "snoozedUntil": presets[0].until.to_string() }),
        ),
    )
    .await;
    env.shell
        .until("snoozed section", |s| {
            let now = jiff::Timestamp::now().as_millisecond();
            crate::sidebar::sections(&s.threads, now)
                .snoozed
                .iter()
                .any(|t| t.id == thread_id)
                .then_some(())
        })
        .await;
    dispatch(
        &env.client,
        cmd("thread.unsnooze", json!({ "reason": "user" })),
    )
    .await;
    env.shell
        .until("unsnoozed", |s| {
            thread(s).filter(|t| t.snoozed_until.is_none()).map(|_| ())
        })
        .await;
    dispatch(
        &env.client,
        cmd(
            "thread.runtime-mode.set",
            json!({ "runtimeMode": "approval-required" }),
        ),
    )
    .await;
    env.shell
        .until("runtime mode", |s| {
            thread(s)
                .filter(|t| t.runtime_mode == "approval-required")
                .map(|_| ())
        })
        .await;
    dispatch(
        &env.client,
        cmd(
            "thread.interaction-mode.set",
            json!({ "interactionMode": "plan" }),
        ),
    )
    .await;
    env.shell
        .until("plan mode", |s| {
            thread(s)
                .filter(|t| t.interaction_mode == "plan")
                .map(|_| ())
        })
        .await;
    let removals = env.shell.state.lock().unwrap().removals;
    dispatch(&env.client, cmd("thread.archive", json!({}))).await;
    env.shell
        .until("archive reported", |s| {
            (s.removals > removals && thread(s).is_none()).then_some(())
        })
        .await;
    let archived = env
        .client
        .call("orchestration.getArchivedShellSnapshot", json!({}))
        .await
        .unwrap();
    env.shell.state.lock().unwrap().set_archived(&archived);
    assert!(
        env.shell
            .state
            .lock()
            .unwrap()
            .archived
            .iter()
            .any(|t| t.id == thread_id),
        "archived list"
    );
    dispatch(&env.client, cmd("thread.unarchive", json!({}))).await;
    env.shell
        .until("unarchived", |s| thread(s).map(|_| ()))
        .await;
    dispatch(&env.client, cmd("thread.delete", json!({}))).await;
    env.shell
        .until("deleted", |s| {
            (thread(s).is_none() && !s.archived.iter().any(|t| t.id == thread_id)).then_some(())
        })
        .await;
}

#[tokio::test]
#[ignore]
async fn e2e_rewind() {
    let env = connect().await;
    let (thread_id, view) = launch(&env, "multi-turn", "first").await;
    wait_idle(&env, &thread_id).await;
    dispatch(
        &env.client,
        json!({
            "type": "message.dispatch", "threadId": thread_id, "messageId": uuid::Uuid::new_v4().to_string(),
            "text": "second", "attachments": [], "dispatchMode": { "type": "queue_after_active" },
            "createdBy": "user", "creationSource": "web",
        }),
    )
    .await;
    let (second_id, turn) = view
        .until("revert target for the second message", |v| {
            let turns = v.revert_turns(None);
            v.items()
                .filter(|i| i.kind == "user_message" && i.str("text") == "second")
                .find_map(|i| turns.get(&i.id).map(|t| (i.id.clone(), *t)))
        })
        .await;
    wait_idle(&env, &thread_id).await;
    let command = view
        .state
        .lock()
        .unwrap()
        .rollback_command(&thread_id, turn, false)
        .expect("rollback checkpoint");
    println!("rewinding to turn {turn}: {command}");
    dispatch(&env.client, command).await;
    view.until("second turn removed", |v| {
        (!v.items().any(|i| i.id == second_id)).then_some(())
    })
    .await;
    println!("after rewind: {:?}", kinds(&view.state.lock().unwrap()));
}

#[tokio::test]
#[ignore]
async fn e2e_terminal() {
    let env = connect().await;
    let (thread_id, _view) = launch(&env, "multi-turn", "terminal").await;
    wait_idle(&env, &thread_id).await;
    let cwd = {
        let shell = env.shell.state.lock().unwrap();
        shell
            .projects
            .iter()
            .find(|p| p.id == env.project_id)
            .unwrap()
            .workspace_root
            .clone()
    };
    let term = Live::start(
        &env.client,
        "terminal.attach",
        json!({ "threadId": thread_id, "terminalId": crate::terminal::TERMINAL_ID, "cwd": cwd,
                "cols": 80, "rows": 10, "maxHistoryBytes": crate::terminal::MAX_HISTORY_BYTES }),
        |t: &mut TermBox, e: &Value| match e["type"].as_str() {
            Some("snapshot") => t.0.feed(e["snapshot"]["history"].as_str().unwrap_or("")),
            Some("output") => t.0.feed(e["data"].as_str().unwrap_or("")),
            _ => {}
        },
    );
    term.until("shell prompt", |t| {
        t.0.lines()
            .iter()
            .any(|l| !l.text.trim().is_empty())
            .then_some(())
    })
    .await;
    dispatch_terminal(&env.client, &thread_id, "echo t3-e2e-$((40+2))\r").await;
    term.until("echo output", |t| {
        t.0.lines()
            .iter()
            .any(|l| l.text.trim() == "t3-e2e-42")
            .then_some(())
    })
    .await;
    let screen: Vec<String> = term
        .state
        .lock()
        .unwrap()
        .0
        .lines()
        .into_iter()
        .map(|l| l.text)
        .collect();
    println!("terminal: {screen:#?}");
    env.client
        .call("terminal.close", json!({ "threadId": thread_id, "terminalId": crate::terminal::TERMINAL_ID, "deleteHistory": true }))
        .await
        .unwrap();
}

struct TermBox(crate::terminal::TermView);

impl Default for TermBox {
    fn default() -> Self {
        Self(crate::terminal::TermView::new(80, 10))
    }
}

async fn dispatch_terminal(client: &RpcClient, thread_id: &str, data: &str) {
    client
        .call("terminal.write", json!({ "threadId": thread_id, "terminalId": crate::terminal::TERMINAL_ID, "data": data }))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn e2e_git_status_and_commit() {
    let env = connect().await;
    let (thread_id, _view) = launch(&env, "multi-turn", "git").await;
    wait_idle(&env, &thread_id).await;
    let cwd = {
        let shell = env.shell.state.lock().unwrap();
        shell
            .projects
            .iter()
            .find(|p| p.id == env.project_id)
            .unwrap()
            .workspace_root
            .clone()
    };
    let git = Live::start(
        &env.client,
        "subscribeVcsStatus",
        json!({ "cwd": cwd, "includeRemote": true }),
        crate::git::GitStatus::apply,
    );
    let branch = git
        .until("repo status", |g| {
            g.is_repo().then(|| g.branch().map(str::to_owned))
        })
        .await;
    println!("branch: {branch:?}");

    let term = Live::start(
        &env.client,
        "terminal.attach",
        json!({ "threadId": thread_id, "terminalId": crate::terminal::TERMINAL_ID, "cwd": cwd, "cols": 80, "rows": 10 }),
        |t: &mut TermBox, e: &Value| match e["type"].as_str() {
            Some("snapshot") => t.0.feed(e["snapshot"]["history"].as_str().unwrap_or("")),
            Some("output") => t.0.feed(e["data"].as_str().unwrap_or("")),
            _ => {}
        },
    );
    term.until("shell prompt", |t| {
        t.0.lines()
            .iter()
            .any(|l| !l.text.trim().is_empty())
            .then_some(())
    })
    .await;
    dispatch_terminal(&env.client, &thread_id, "echo hello > e2e.txt\r").await;
    let _ = env
        .client
        .call("vcs.refreshStatus", json!({ "cwd": cwd }))
        .await;
    git.until("file change", |g| (g.changes().0 >= 1).then_some(()))
        .await;

    let (_, mut rx) = env.client.request(
        "git.runStackedAction",
        json!({ "actionId": uuid::Uuid::new_v4().to_string(), "cwd": cwd, "action": "commit",
                "commitMessage": "e2e commit", "threadId": thread_id, "projectId": env.project_id }),
    );
    let mut finished = None;
    while let Some(event) = rx.recv().await {
        let event = event.expect("git action");
        println!("git: {} {}", event["kind"], event["label"]);
        if event["kind"] == "action_failed" {
            panic!("commit failed: {event}");
        }
        if event["kind"] == "action_finished" {
            finished = Some(event["result"].clone());
        }
    }
    let result = finished.expect("action_finished");
    println!("result: {}", crate::git::git_result_summary(&result));
    assert_eq!(result["commit"]["status"], "created");
    let _ = env
        .client
        .call("vcs.refreshStatus", json!({ "cwd": cwd }))
        .await;
    git.until("clean tree", |g| (g.changes().0 == 0).then_some(()))
        .await;
    env.client
        .call("terminal.close", json!({ "threadId": thread_id, "terminalId": crate::terminal::TERMINAL_ID, "deleteHistory": true }))
        .await
        .unwrap();
}
