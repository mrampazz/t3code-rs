//! Client projections of `orchestration.subscribeShell` / `subscribeThread`.
//! Only fields the UI reads are typed; serde ignores the rest so newer servers still decode.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub workspace_root: String,
    #[serde(default)]
    pub default_thread_env_mode: Option<String>,
    #[serde(default)]
    pub default_model_selection: Value,
    #[serde(default)]
    pub repository_identity: Value,
    /// The project as the shell sent it, for fields read in one place (pin, archive, auto-pull).
    #[serde(skip)]
    pub raw: Value,
    /// `ProjectScript`s; settings may override them (`scripts::resolve`).
    #[serde(default)]
    pub scripts: Value,
}

impl Project {
    fn from_value(v: &Value) -> Option<Self> {
        let mut project: Project = decode(v)?;
        project.raw = v.clone();
        Some(project)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadShell {
    /// The environment (server URL) this thread lives in; set by `Shell`, not the wire.
    #[serde(skip)]
    pub env: String,
    pub id: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub status: Value,
    pub updated_at: String,
    #[serde(default)]
    pub model_selection: Value,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub runtime_mode: String,
    #[serde(default)]
    pub interaction_mode: String,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub pinned_at: Option<String>,
    #[serde(default)]
    pub settled_at: Option<String>,
    #[serde(default)]
    pub settled_override: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub unsettled_at: Option<String>,
    #[serde(default)]
    pub active_order_key: Option<String>,
    #[serde(default)]
    pub pin_order_key: Option<String>,
    #[serde(default)]
    pub snoozed_until: Option<String>,
    #[serde(default)]
    pub snoozed_at: Option<String>,
    #[serde(default)]
    pub latest_user_message_at: Option<String>,
    #[serde(default)]
    pub pending_runtime_request: Value,
    #[serde(default)]
    pub last_visited_at: Option<String>,
    #[serde(default)]
    pub latest_run_completed_at: Option<String>,
    #[serde(default)]
    pub activity_run_started_at: Option<String>,
    #[serde(default)]
    pub latest_run_started_at: Option<String>,
    #[serde(default)]
    pub pull_requests: Vec<Value>,
    /// `ThreadIssueLink`s.
    #[serde(default)]
    pub issues: Vec<Value>,
    #[serde(default)]
    pub latest_run_id: Option<String>,
    #[serde(default)]
    pub activity_run_status: Option<String>,
    #[serde(default)]
    pub blocked_on_thread_wait: bool,
    #[serde(default)]
    pub pending_guest_message_count: u64,
    #[serde(default)]
    pub has_actionable_proposed_plan: bool,
    #[serde(default)]
    pub last_error_class: Option<String>,
    #[serde(default)]
    pub usage_limit_reset_at: Option<String>,
    /// `OrchestrationV2AppThreadLineage`.
    #[serde(default)]
    pub lineage: Value,
    /// `OrchestrationV2LimitRecovery`, or null.
    #[serde(default)]
    pub limit_recovery: Value,
}

impl ThreadShell {
    /// `hasUnseenCompletion` (apps/web/src/components/Sidebar.logic.ts): the latest run finished
    /// after the last visit. Interrupted and cancelled runs, and never-visited threads, are not unread.
    pub fn has_unseen_completion(&self) -> bool {
        if matches!(self.status.as_str(), Some("interrupted" | "cancelled")) {
            return false;
        }
        let ms = |s: &Option<String>| s.as_deref().and_then(|s| s.parse::<jiff::Timestamp>().ok());
        match (ms(&self.latest_run_completed_at), ms(&self.last_visited_at)) {
            (Some(done), Some(seen)) => done > seen,
            (Some(_), None) if self.last_visited_at.is_some() => true,
            _ => false,
        }
    }

    /// The thread changed after the last recorded visit.
    pub fn needs_visit(&self) -> bool {
        let ms = |s: Option<&str>| s.and_then(|s| s.parse::<jiff::Timestamp>().ok());
        match (
            ms(Some(&self.updated_at)),
            ms(self.last_visited_at.as_deref()),
        ) {
            (Some(updated), Some(seen)) => updated > seen,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// `isListedThreadLineage`: subagents, side chats and workflow steps live inside their
    /// parent's view, never in thread lists.
    pub fn is_listed(&self) -> bool {
        self.lineage["relationshipToParent"] != "subagent"
            && self.lineage["sideChat"] != true
            && self.lineage["workflowStep"] != true
    }

    /// The thread this one was spawned from, for a "back" link.
    pub fn parent_id(&self) -> Option<&str> {
        self.lineage["parentThreadId"]
            .as_str()
            .filter(|_| !self.is_listed())
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.status.as_str(),
            Some("preparing" | "queued" | "starting" | "running" | "waiting")
        )
    }
}

#[derive(Default)]
pub struct Shell {
    /// Environment key stamped on every thread this shell decodes.
    pub env: String,
    pub projects: Vec<Project>,
    pub threads: Vec<ThreadShell>,
    /// Loaded with `getArchivedShellSnapshot`; the stream reports archiving as a removal.
    pub archived: Vec<ThreadShell>,
    /// Bumped per `thread.removed`, which is when the archive list goes stale.
    pub removals: u64,
    pub synced: bool,
    /// Resume cursor for `afterSequence`; 0 until the first snapshot.
    pub sequence: u64,
}

impl Shell {
    pub fn apply(&mut self, item: &Value) {
        if let Some(seq) = item["sequence"].as_u64() {
            self.sequence = self.sequence.max(seq);
        }
        match item["kind"].as_str() {
            Some("snapshot") => {
                let snap = &item["snapshot"];
                self.sequence = snap["snapshotSequence"].as_u64().unwrap_or(0);
                self.projects = snap["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Project::from_value)
                    .collect();
                self.threads = decode_vec(&snap["threads"]);
                self.stamp();
            }
            Some("synchronized") => self.synced = true,
            Some("project.updated") => {
                if let Some(p) = Project::from_value(&item["project"]) {
                    upsert(&mut self.projects, p, |p| &p.id);
                }
            }
            Some("project.removed") => {
                let id = item["projectId"].as_str().unwrap_or_default();
                self.projects.retain(|p| p.id != id);
            }
            Some("thread.updated") => {
                let Some(mut t) = decode::<ThreadShell>(&item["thread"]) else {
                    return;
                };
                t.env = self.env.clone();
                let (to, from) = if item["location"] == "active" {
                    (&mut self.threads, &mut self.archived)
                } else {
                    (&mut self.archived, &mut self.threads)
                };
                from.retain(|x| x.id != t.id);
                upsert(to, t, |t| &t.id);
            }
            Some("thread.removed") => {
                let id = item["threadId"].as_str().unwrap_or_default();
                self.threads.retain(|t| t.id != id);
                self.archived.retain(|t| t.id != id);
                self.removals += 1;
            }
            _ => {}
        }
        // Pinned first, then most recently updated.
        for list in [&mut self.threads, &mut self.archived] {
            list.sort_by(|a, b| {
                (b.pinned_at.is_some(), &b.updated_at).cmp(&(a.pinned_at.is_some(), &a.updated_at))
            });
        }
    }

    pub fn new(env: &str) -> Self {
        Self {
            env: env.to_owned(),
            ..Self::default()
        }
    }

    fn stamp(&mut self) {
        for t in self.threads.iter_mut().chain(self.archived.iter_mut()) {
            t.env.clone_from(&self.env);
        }
    }

    pub fn set_archived(&mut self, snapshot: &Value) {
        self.archived = decode_vec(&snapshot["threads"]);
        self.stamp();
        self.archived
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub instance_id: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub availability: Value,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub auth: Value,
    #[serde(default)]
    pub models_retained: bool,
    #[serde(default)]
    pub supports_conversation_rollback: Option<bool>,
    #[serde(default)]
    pub supported_runtime_modes: Option<Vec<String>>,
    #[serde(default)]
    pub show_interaction_mode_toggle: Option<bool>,
    #[serde(default)]
    pub slash_commands: Vec<Value>,
    #[serde(default)]
    pub compatibility_advisory: Value,
    /// `ServerProviderUsageLimits`: rate-limit windows, when the provider reports them.
    #[serde(default)]
    pub usage_limits: Value,
    #[serde(default)]
    pub skills: Vec<Value>,
    /// Per-folder slash commands and skills (`ServerProviderWorkspaceSnapshot`).
    #[serde(default)]
    pub workspace_snapshots: Vec<Value>,
    #[serde(default)]
    pub models: Vec<ProviderModel>,
}

impl Provider {
    /// `isProviderInstancePickerReady` (apps/web/src/providerInstances.ts), minus sub-providers.
    pub fn picker_ready(&self) -> bool {
        if !self.enabled || self.availability == "unavailable" {
            return false;
        }
        self.status == "ready"
            || (self.status == "warning"
                && (self.models_retained
                    || (self.installed
                        && self.auth["status"] == "authenticated"
                        && !self.models.is_empty())))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub is_default: bool,
}

/// `subscribeServerConfig` projection (`packages/contracts/src/server.ts`).
#[derive(Default)]
pub struct ServerConfig {
    pub providers: Vec<Provider>,
    /// Server supports `thread.stop`; older servers need `run.interrupt`.
    pub thread_stop: bool,
    /// Bumped on every provider change so views can rebuild derived lists.
    pub revision: u64,
    /// `settings.snoozePresets`; `None` uses the defaults.
    pub snooze_presets: Option<Value>,
    /// The environment's `ServerSettings`.
    pub settings: Value,
    /// `ResolvedKeybindingsConfig`; empty until the snapshot arrives.
    pub keybindings: Vec<Value>,
    /// `EditorId`s the server can launch.
    pub available_editors: Vec<String>,
    /// `RemoteOpenTarget`s: SSH hosts this server is reachable at.
    pub remote_open_targets: Vec<Value>,
    /// `orchestration.forceStopThread` is available.
    pub thread_force_stop: bool,
    /// Scripts save to `projectSettingsOverrides`; older servers use `projectScriptOverrides`.
    pub project_settings_overrides: bool,
}

/// Picker key for a model selection: provider instance and model slug.
pub fn model_key(selection: &Value) -> String {
    format!(
        "{}/{}",
        selection["instanceId"].as_str().unwrap_or(""),
        selection["model"].as_str().unwrap_or("")
    )
}

impl ServerConfig {
    pub fn apply(&mut self, event: &Value) {
        self.revision += 1;
        match event["type"].as_str() {
            Some("snapshot") => {
                let config = &event["config"];
                self.providers = decode_vec(&config["providers"]);
                self.thread_stop = config["environment"]["capabilities"]["threadStop"] == true;
                self.thread_force_stop =
                    config["environment"]["capabilities"]["threadForceStop"] == true;
                self.project_settings_overrides =
                    config["environment"]["capabilities"]["projectSettingsOverrides"] == true;
                self.snooze_presets = config["settings"].get("snoozePresets").cloned();
                self.settings = config["settings"].clone();
                self.keybindings = config["keybindings"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                self.available_editors = decode_vec(&config["availableEditors"]);
                self.remote_open_targets = decode_vec(&config["remoteOpenTargets"]);
            }
            Some("providerStatuses") => self.providers = decode_vec(&event["payload"]["providers"]),
            Some("keybindingsUpdated") => {
                self.keybindings = event["payload"]["keybindings"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
            Some("settingsUpdated") => {
                if event["payload"]["settings"].is_object() {
                    self.settings = event["payload"]["settings"].clone();
                }
                if let Some(presets) = event["payload"]["settings"].get("snoozePresets") {
                    self.snooze_presets = Some(presets.clone());
                }
            }
            _ => {}
        }
    }

    /// Merges a `server.refreshProviders` reply's folder snapshots into the known providers.
    pub fn merge_workspace_snapshots(&mut self, reply: &Value) {
        for incoming in reply["providers"].as_array().into_iter().flatten() {
            let Some(ours) = self
                .providers
                .iter_mut()
                .find(|p| incoming["instanceId"] == p.instance_id.as_str())
            else {
                continue;
            };
            for snapshot in incoming["workspaceSnapshots"]
                .as_array()
                .into_iter()
                .flatten()
            {
                ours.workspace_snapshots
                    .retain(|s| s["cwd"] != snapshot["cwd"]);
                ours.workspace_snapshots.push(snapshot.clone());
            }
        }
    }

    /// Every model of every available provider, as (label, selection).
    pub fn model_options(&self) -> Vec<(String, Value)> {
        self.providers
            .iter()
            .filter(|p| p.picker_ready())
            .flat_map(|p| {
                p.models.iter().map(move |m| {
                    let selection =
                        serde_json::json!({ "instanceId": p.instance_id, "model": m.slug });
                    (self.label(&selection), selection)
                })
            })
            .collect()
    }

    /// "Provider · Model" with display names when this server knows them.
    pub fn label(&self, selection: &Value) -> String {
        let (instance, slug) = (
            selection["instanceId"].as_str().unwrap_or(""),
            selection["model"].as_str().unwrap_or(""),
        );
        let provider = self.providers.iter().find(|p| p.instance_id == instance);
        let model = provider
            .and_then(|p| p.models.iter().find(|m| m.slug == slug))
            .map_or(slug, |m| m.name.as_str());
        let provider = provider
            .and_then(|p| p.display_name.as_deref())
            .unwrap_or(instance);
        format!("{provider} · {model}")
    }

    /// The project's default if set, else the first available provider's default model.
    pub fn model_for(&self, project: Option<&Project>) -> Option<Value> {
        if let Some(selection) = project
            .map(|p| &p.default_model_selection)
            .filter(|v| v.is_object())
        {
            return Some(selection.clone());
        }
        let provider = self
            .providers
            .iter()
            .find(|p| p.picker_ready() && !p.models.is_empty())?;
        let model = provider
            .models
            .iter()
            .find(|m| m.is_default)
            .unwrap_or(&provider.models[0]);
        Some(serde_json::json!({ "instanceId": provider.instance_id, "model": model.slug }))
    }
}

/// One `OrchestrationV2TurnItem`; renderers read type-specific fields from `raw`.
#[derive(Clone, Debug)]
pub struct TurnItem {
    pub id: String,
    pub ordinal: u64,
    pub kind: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub raw: Value,
}

impl TurnItem {
    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            id: v["id"].as_str()?.to_owned(),
            ordinal: v["ordinal"].as_u64()?,
            kind: v["type"].as_str()?.to_owned(),
            run_id: v["runId"].as_str().map(str::to_owned),
            node_id: v["nodeId"].as_str().map(str::to_owned),
            raw: v.clone(),
        })
    }

    pub fn str(&self, key: &str) -> &str {
        self.raw[key].as_str().unwrap_or("")
    }
}

/// A `visibleTurnItems` row: local items plus rows inherited from a fork source.
#[derive(Clone, Debug)]
pub struct Row {
    pub local: bool,
    pub item: TurnItem,
}

/// A turn's captured checkpoint; `turn` is the `appRunOrdinal` diff ranges use.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub id: String,
    pub turn: u64,
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
}

impl Checkpoint {
    /// `deriveThreadCheckpointSummaries` skips checkpoints without a run or ordinal.
    fn from_value(v: &Value) -> Option<Self> {
        v["runId"].as_str()?;
        let files = v["files"].as_array().cloned().unwrap_or_default();
        let totals = &v["filesTotal"];
        let sum = |key: &str| {
            totals[key]
                .as_u64()
                .unwrap_or_else(|| files.iter().filter_map(|f| f[key].as_u64()).sum())
        };
        Some(Self {
            id: v["id"].as_str()?.to_owned(),
            turn: v["appRunOrdinal"].as_u64()?,
            files: totals["count"].as_u64().map_or(files.len(), |n| n as usize),
            additions: sum("additions"),
            deletions: sum("deletions"),
        })
    }
}

#[derive(Clone, Debug)]
struct RewindPoint {
    turn: u64,
    run_id: Option<String>,
    id: String,
    scope_id: String,
    status: String,
}

#[derive(Clone, Debug)]
struct Run {
    status: String,
    started: bool,
    ordinal: u64,
    queue_position: Option<u64>,
    user_message_id: String,
}

/// A message waiting for its own turn (`deriveThreadQueueWorkflowState`).
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedRun {
    pub run_id: String,
    pub text: String,
}

/// Thread timeline, mirroring `client-runtime/src/state/orchestrationV2Projection.ts`.
#[derive(Default)]
pub struct ThreadView {
    rows: Vec<Row>,
    runs: HashMap<String, Run>,
    /// Text of messages waiting in the queue, by message id; dropped once their run leaves it.
    queued_messages: HashMap<String, String>,
    /// runId -> root node ids of superseded attempts.
    superseded: HashMap<String, HashSet<String>>,
    /// Newest turn first.
    pub checkpoints: Vec<Checkpoint>,
    /// Every checkpoint a rewind can target, keyed by turn (0 = thread start).
    rewind: Vec<RewindPoint>,
    /// Resume cursor for `afterSequence`; 0 until the first snapshot.
    pub sequence: u64,
    /// Older history the bounded snapshot left out (`threadHistoryPage` cursor).
    pub history_cursor: Option<String>,
    pub has_more_history: bool,
    /// The projection's `thread` (`OrchestrationV2AppThread`), replaced by every `thread.*` event.
    pub app_thread: Value,
}

fn projected_rows(rows: &Value) -> Vec<Row> {
    rows.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(Row {
                local: row["visibility"] == "local",
                item: TurnItem::from_value(&row["item"])?,
            })
        })
        .collect()
}

impl ThreadView {
    /// Prepends a history page (older rows), skipping rows already present.
    pub fn prepend_history(&mut self, page: &Value) {
        let mut older: Vec<Row> = projected_rows(&page["items"])
            .into_iter()
            .filter(|r| !self.rows.iter().any(|x| x.item.id == r.item.id))
            .collect();
        older.append(&mut self.rows);
        self.rows = older;
        self.history_cursor = page["nextCursor"].as_str().map(str::to_owned);
        self.has_more_history = page["hasMoreHistory"] == true;
    }

    fn apply_checkpoint(&mut self, v: &Value) {
        let turn = match (
            v["appRunOrdinal"].as_u64(),
            v["ordinalWithinScope"].as_u64(),
        ) {
            (Some(turn), _) => Some(turn),
            (None, Some(0)) => Some(0),
            _ => None,
        };
        if let (Some(turn), Some(id), Some(scope_id)) =
            (turn, v["id"].as_str(), v["scopeId"].as_str())
        {
            // shortcut: tracked-repository scopes also have a turn-0 baseline; the first wins here,
            // while the web skips repository scopes (findRevertCheckpoint). Upgrade with multi-repo support.
            self.rewind.retain(|p| p.id != id);
            self.rewind.push(RewindPoint {
                turn,
                run_id: v["runId"].as_str().map(str::to_owned),
                id: id.to_owned(),
                scope_id: scope_id.to_owned(),
                status: v["status"].as_str().unwrap_or("").to_owned(),
            });
        }
        let Some(checkpoint) = Checkpoint::from_value(v) else {
            return;
        };
        upsert(&mut self.checkpoints, checkpoint, |c| &c.id);
        self.checkpoints.sort_by_key(|c| std::cmp::Reverse(c.turn));
    }

    /// The turn each user message rewinds to: the previous run's checkpoint, or the thread start.
    /// Port of `deriveRevertTurnCountByUserMessageId` (apps/web/src/session-logic.ts).
    pub fn revert_turns(&self, running_run: Option<&str>) -> HashMap<String, u64> {
        let rewindable = |p: &&RewindPoint| p.status != "stale";
        let turn_by_run: HashMap<&str, u64> = self
            .rewind
            .iter()
            .filter(rewindable)
            .filter_map(|p| Some((p.run_id.as_deref()?, p.turn)))
            .collect();
        // The first loaded row is the thread's first only when no older history is unloaded.
        let start_available = !self.has_more_history
            && self
                .rewind
                .iter()
                .filter(rewindable)
                .any(|p| p.turn == 0 && p.run_id.is_none());
        let mut target_by_run: HashMap<&str, Option<u64>> = HashMap::new();
        let mut previous: Option<&str> = None;
        let mut out = HashMap::new();
        for row in self
            .rows
            .iter()
            .filter(|r| r.local && self.visible(&r.item))
        {
            let item = &row.item;
            let Some(run) = item.run_id.as_deref() else {
                continue;
            };
            if !target_by_run.contains_key(run) {
                let target = match previous {
                    None => start_available.then_some(0),
                    Some(prev) => turn_by_run.get(prev).copied(),
                };
                target_by_run.insert(run, target);
                previous = Some(run);
            }
            let intent = item.str("inputIntent");
            if item.kind != "user_message"
                || Some(run) == running_run
                || !matches!(intent, "turn_start" | "queued_turn")
            {
                continue;
            }
            if let Some(Some(target)) = target_by_run.get(run) {
                out.insert(item.id.clone(), *target);
            }
        }
        out
    }

    /// `checkpoint.rollback` to `turn`; restoring files needs a ready snapshot.
    pub fn rollback_command(
        &self,
        thread_id: &str,
        turn: u64,
        restore_files: bool,
    ) -> Option<Value> {
        let point = self
            .rewind
            .iter()
            .rev()
            .find(|p| p.turn == turn && (turn > 0 || p.run_id.is_none()))?;
        let usable = point.status == "ready" || (!restore_files && point.status != "stale");
        usable.then(|| {
            serde_json::json!({
                "type": "checkpoint.rollback",
                "threadId": thread_id,
                "scopeId": point.scope_id,
                "checkpointId": point.id,
                "restoreFiles": restore_files,
            })
        })
    }

    /// Queued user runs in queue order, with their message text (`deriveThreadQueueWorkflowState`).
    pub fn queued_runs(&self) -> Vec<QueuedRun> {
        let mut queued: Vec<(&String, &Run)> = self
            .runs
            .iter()
            .filter(|(_, r)| r.status == "queued")
            .collect();
        queued.sort_by_key(|(_, r)| (r.queue_position.unwrap_or(r.ordinal), r.ordinal));
        queued
            .into_iter()
            .filter_map(|(id, run)| {
                Some(QueuedRun {
                    run_id: id.clone(),
                    text: self.queued_messages.get(&run.user_message_id)?.clone(),
                })
            })
            .collect()
    }

    /// `presentRunStop`: a Stop the provider has not finished, as (run id, unresponsive).
    pub fn run_stop(&self) -> Option<(String, bool)> {
        let newest = |keep: &dyn Fn(&Run) -> bool| {
            self.runs
                .iter()
                .filter(|(_, r)| keep(r))
                .max_by_key(|(_, r)| r.ordinal)
                .map(|(id, _)| id.clone())
        };
        let active = newest(&|r| matches!(r.status.as_str(), "preparing" | "starting" | "running"));
        let run = active
            .clone()
            .or_else(|| newest(&|r| r.status != "queued"))?;
        let mut request = None;
        for row in &self.rows {
            if row.item.run_id.as_deref() != Some(run.as_str()) {
                continue;
            }
            match row.item.kind.as_str() {
                "run_interrupt_result" => return None,
                "run_interrupt_request" => request = Some(&row.item),
                _ => {}
            }
        }
        let unresponsive = !request?.raw["unresponsiveAt"].is_null();
        (active.is_some() || unresponsive).then_some((run, unresponsive))
    }

    /// The newest preparing, starting, running or waiting run (`resolveActiveThreadRun`).
    pub fn active_run(&self) -> Option<&str> {
        self.runs
            .iter()
            .filter(|(_, r)| {
                matches!(
                    r.status.as_str(),
                    "preparing" | "starting" | "running" | "waiting"
                )
            })
            .max_by_key(|(_, r)| r.ordinal)
            .map(|(id, _)| id.as_str())
    }

    pub fn latest_turn(&self) -> Option<u64> {
        self.checkpoints.first().map(|c| c.turn)
    }

    pub fn apply(&mut self, item: &Value) {
        match item["kind"].as_str() {
            Some("snapshot") => {
                let projection = &item["projection"];
                self.sequence = item["snapshotSequence"].as_u64().unwrap_or(0);
                self.runs.clear();
                self.superseded.clear();
                for run in projection["runs"].as_array().into_iter().flatten() {
                    self.apply_run(run);
                }
                for attempt in projection["attempts"].as_array().into_iter().flatten() {
                    self.apply_attempt(attempt);
                }
                self.queued_messages.clear();
                for message in projection["messages"].as_array().into_iter().flatten() {
                    let id = message["id"].as_str().unwrap_or("");
                    if self
                        .runs
                        .values()
                        .any(|r| r.status == "queued" && r.user_message_id == id)
                    {
                        self.apply_message(message);
                    }
                }
                self.checkpoints.clear();
                self.rewind.clear();
                for checkpoint in projection["checkpoints"].as_array().into_iter().flatten() {
                    self.apply_checkpoint(checkpoint);
                }
                self.rows = projected_rows(&projection["visibleTurnItems"]);
                self.app_thread = projection["thread"].clone();
                self.history_cursor = item["historyCursor"].as_str().map(str::to_owned);
                self.has_more_history = item["hasMoreHistory"] == true;
            }
            Some("event") => {
                self.sequence = self.sequence.max(item["sequence"].as_u64().unwrap_or(0));
                let payload = &item["event"]["payload"];
                match item["event"]["type"].as_str() {
                    Some("turn-item.updated") => {
                        if let Some(t) = TurnItem::from_value(payload) {
                            self.upsert(t);
                        }
                    }
                    Some("run.created" | "run.updated") => self.apply_run(payload),
                    Some("message.updated") => self.apply_message(payload),
                    Some(kind) if kind.starts_with("thread.") && payload["id"].is_string() => {
                        self.app_thread = payload.clone();
                    }
                    Some("run-attempt.created" | "run-attempt.updated") => {
                        self.apply_attempt(payload)
                    }
                    Some("checkpoint.captured") => self.apply_checkpoint(payload),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn apply_run(&mut self, run: &Value) {
        if let (Some(id), Some(status)) = (run["id"].as_str(), run["status"].as_str()) {
            let started = !run["startedAt"].is_null();
            if status != "queued"
                && let Some(message) = run["userMessageId"].as_str()
            {
                self.queued_messages.remove(message);
            }
            self.runs.insert(
                id.to_owned(),
                Run {
                    status: status.to_owned(),
                    started,
                    ordinal: run["ordinal"].as_u64().unwrap_or(0),
                    queue_position: run["queuePosition"].as_u64(),
                    user_message_id: run["userMessageId"].as_str().unwrap_or("").to_owned(),
                },
            );
        }
    }

    /// Keeps a user message that may sit in the queue; automatic completions never show there
    /// (`getUserQueuedThreadRuns`).
    fn apply_message(&mut self, message: &Value) {
        let Some(id) = message["id"].as_str() else {
            return;
        };
        let automatic =
            !message["delegatedCompletion"].is_null() || !message["notification"].is_null();
        let started = self
            .runs
            .values()
            .any(|r| r.user_message_id == id && r.status != "queued");
        if message["role"] != "user" || automatic || started {
            return;
        }
        let text = message["context"]["guest"]["text"]
            .as_str()
            .or(message["text"].as_str())
            .unwrap_or("");
        self.queued_messages.insert(id.to_owned(), text.to_owned());
    }

    fn apply_attempt(&mut self, attempt: &Value) {
        if attempt["status"] != "superseded" {
            return;
        }
        if let (Some(run), Some(root)) = (attempt["runId"].as_str(), attempt["rootNodeId"].as_str())
        {
            self.superseded
                .entry(run.to_owned())
                .or_default()
                .insert(root.to_owned());
        }
    }

    /// Replaces in place at the same ordinal; otherwise inserts before the first later local row.
    fn upsert(&mut self, item: TurnItem) {
        let existing = self
            .rows
            .iter()
            .rposition(|r| r.local && r.item.id == item.id);
        if let Some(i) = existing.filter(|&i| self.rows[i].item.ordinal == item.ordinal) {
            self.rows[i].item = item;
            return;
        }
        if let Some(i) = existing {
            self.rows.remove(i);
        } else if self.below_window(&item) {
            // `shouldDropMissingPartialTurnItem`: an unseen item older than the loaded window
            // belongs to history that is not loaded; it arrives with its page.
            return;
        }
        let at = self
            .rows
            .iter()
            .position(|r| r.local && (r.item.ordinal, &r.item.id) > (item.ordinal, &item.id))
            .unwrap_or(self.rows.len());
        self.rows.insert(at, Row { local: true, item });
    }

    /// An item older than the loaded window of a thread with more history, unless its run is
    /// still unfinished (a queued run keeps its early ordinal).
    fn below_window(&self, item: &TurnItem) -> bool {
        if !self.has_more_history {
            return false;
        }
        let unfinished = item
            .run_id
            .as_ref()
            .and_then(|id| self.runs.get(id))
            .is_some_and(|r| {
                matches!(
                    r.status.as_str(),
                    "preparing" | "queued" | "starting" | "running" | "waiting"
                )
            });
        let oldest = self
            .rows
            .iter()
            .filter(|r| r.local)
            .map(|r| r.item.ordinal)
            .min();
        !unfinished && oldest.is_some_and(|oldest| item.ordinal < oldest)
    }

    /// `isOrchestrationV2TurnItemVisible` from packages/shared/src/orchestrationV2Timeline.ts.
    fn visible(&self, item: &TurnItem) -> bool {
        let run = item.run_id.as_ref().and_then(|id| self.runs.get(id));
        if run.is_some_and(|r| r.status == "rolled_back") {
            return false;
        }
        let held =
            run.is_some_and(|r| r.status == "queued" || (r.status == "cancelled" && !r.started));
        if held && item.kind == "user_message" && item.str("inputIntent") == "queued_turn" {
            return false;
        }
        if item.kind != "run_interrupt_result" {
            return true;
        }
        let (Some(run_id), Some(node_id)) = (&item.run_id, &item.node_id) else {
            return true;
        };
        let superseded = self
            .superseded
            .get(run_id)
            .is_some_and(|roots| roots.contains(node_id));
        let has_request = self.rows.iter().any(|r| {
            r.item.kind == "run_interrupt_request" && r.item.run_id.as_ref() == Some(run_id)
        });
        !superseded || has_request
    }

    pub fn items(&self) -> impl Iterator<Item = &TurnItem> {
        self.rows
            .iter()
            .filter(|r| !r.local || self.visible(&r.item))
            .map(|r| &r.item)
    }
}

/// The value an option answers with: its `value`, else its label.
pub fn option_value(option: &Value) -> String {
    option["value"]
        .as_str()
        .or(option["label"].as_str())
        .unwrap_or("")
        .to_owned()
}

/// Provider answers for a `user_input_request`, or `None` while a required question is open.
/// Port of `buildPendingUserInputAnswers` (client-runtime/src/state/pendingUserInput.ts);
/// `custom` is the composer text, used where a question allows custom answers.
pub fn build_answers(
    questions: &[Value],
    selected: impl Fn(&str) -> Vec<String>,
    custom: &str,
) -> Option<serde_json::Map<String, Value>> {
    let custom = custom.trim();
    let mut answers = serde_json::Map::new();
    for q in questions {
        let id = q["id"].as_str()?;
        let options = q["options"].as_array().cloned().unwrap_or_default();
        let allows_custom = q["allowCustomAnswer"] != false;
        let required = q["required"] != false;
        let answer = if allows_custom && !custom.is_empty() {
            Value::from(custom)
        } else if !required && options.is_empty() {
            Value::from("")
        } else {
            let picked: Vec<String> = selected(id)
                .into_iter()
                .filter(|v| options.iter().any(|o| &option_value(o) == v))
                .collect();
            match (q["multiSelect"] == true, picked.first()) {
                (true, _) if !picked.is_empty() || !required => Value::from(picked),
                (false, Some(first)) => Value::from(first.clone()),
                _ => return None,
            }
        };
        answers.insert(id.to_owned(), answer);
    }
    Some(answers)
}

fn decode<T: for<'de> Deserialize<'de>>(v: &Value) -> Option<T> {
    serde_json::from_value(v.clone())
        .map_err(|e| eprintln!("decode: {e}"))
        .ok()
}

/// Skips rows that fail to decode instead of dropping the whole list.
fn decode_vec<T: for<'de> Deserialize<'de>>(v: &Value) -> Vec<T> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(decode)
        .collect()
}

fn upsert<T>(list: &mut Vec<T>, item: T, id: impl Fn(&T) -> &String) {
    match list.iter().position(|x| id(x) == id(&item)) {
        Some(i) => list[i] = item,
        None => list.push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread(id: &str, at: &str) -> Value {
        json!({"id": id, "projectId": "p", "title": id, "status": "idle", "updatedAt": at, "extra": 1})
    }

    #[test]
    fn shell_applies_snapshot_then_deltas() {
        let mut shell = Shell::default();
        shell.apply(&json!({"kind": "snapshot", "snapshot": {
            "projects": [{"id": "p", "title": "P"}],
            "threads": [thread("a", "1"), {"broken": true}],
        }}));
        assert_eq!(shell.threads.len(), 1);
        shell.apply(&json!({"kind": "thread.updated", "sequence": 2, "location": "active", "thread": thread("b", "2")}));
        assert_eq!(shell.threads[0].id, "b");
        let mut pinned = thread("a", "1");
        pinned["pinnedAt"] = json!("t");
        shell.apply(&json!({"kind": "thread.updated", "sequence": 2, "location": "active", "thread": pinned}));
        assert_eq!(shell.threads[0].id, "a");
        shell.apply(&json!({"kind": "thread.updated", "sequence": 3, "location": "archive", "thread": thread("a", "3")}));
        assert_eq!(shell.archived.len(), 1);
        shell.apply(&json!({"kind": "thread.removed", "sequence": 3, "location": "active", "threadId": "gone"}));
        assert_eq!(shell.removals, 1);
        shell.set_archived(&json!({"threads": [thread("x", "1"), thread("y", "2")]}));
        assert_eq!(shell.archived[0].id, "y");
        shell.apply(&json!({"kind": "project.removed", "sequence": 4, "projectId": "p"}));
        assert_eq!(
            shell
                .threads
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert_eq!(shell.sequence, 4);
        assert!(shell.projects.is_empty());
    }

    #[test]
    fn server_config_picks_default_model() {
        let mut config = ServerConfig::default();
        config.apply(&json!({"type": "snapshot", "version": 1, "config": {"providers": [
            {"instanceId": "off", "enabled": true, "status": "ready", "availability": "unavailable", "models": [{"slug": "x", "name": "X", "isCustom": false}]},
            {"instanceId": "warn", "enabled": true, "status": "error", "models": [{"slug": "w", "name": "W", "isCustom": false}]},
            {"instanceId": "codex", "enabled": true, "status": "ready", "models": [{"slug": "a", "name": "A", "isCustom": false}, {"slug": "b", "name": "B", "isCustom": false, "isDefault": true}]},
        ]}}));
        assert_eq!(
            config.model_for(None),
            Some(json!({"instanceId": "codex", "model": "b"}))
        );
        let project: Project = serde_json::from_value(json!({"id": "p", "title": "P", "defaultModelSelection": {"instanceId": "c", "model": "m"}})).unwrap();
        assert_eq!(config.model_for(Some(&project)).unwrap()["model"], "m");
        assert_eq!(
            config.label(&json!({"instanceId": "codex", "model": "b"})),
            "codex · B"
        );
        assert_eq!(
            config.label(&json!({"instanceId": "c", "model": "m"})),
            "c · m"
        );
        let options: Vec<String> = config
            .model_options()
            .into_iter()
            .map(|o| model_key(&o.1))
            .collect();
        assert_eq!(options, ["codex/a", "codex/b"]);
    }

    #[test]
    fn thread_view_upserts_turn_items_in_order() {
        let mut view = ThreadView::default();
        let item = |id: &str, ord: u64, text: &str| json!({"id": id, "ordinal": ord, "type": "assistant_message", "text": text, "runId": "r1", "nodeId": null});
        let texts = |view: &ThreadView| {
            view.items()
                .map(|t| t.str("text").to_owned())
                .collect::<Vec<_>>()
        };
        view.apply(
            &json!({"kind": "snapshot", "snapshotSequence": 5, "projection": {
            "runs": [], "attempts": [],
            "visibleTurnItems": [
                {"visibility": "inherited", "item": item("src", 0, "old")},
                {"visibility": "local", "item": item("b", 2, "x")},
            ]}}),
        );
        view.apply(&json!({"kind": "event", "sequence": 6, "event": {"type": "turn-item.updated", "payload": item("a", 1, "hi")}}));
        view.apply(&json!({"kind": "event", "sequence": 7, "event": {"type": "turn-item.updated", "payload": item("b", 2, "xy")}}));
        assert_eq!(texts(&view), ["old", "hi", "xy"]);
        assert_eq!(view.sequence, 7);

        view.apply(&json!({"kind": "event", "sequence": 8, "event": {"type": "run.updated", "payload": {"id": "r1", "status": "rolled_back"}}}));
        assert_eq!(texts(&view), ["old"]);
    }

    #[test]
    fn queued_steer_stays_hidden_until_its_run_starts() {
        let mut view = ThreadView::default();
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 1, "projection": {
            "runs": [{"id": "q", "status": "queued", "startedAt": null}], "attempts": [], "visibleTurnItems": []}}));
        let steer = json!({"id": "u", "ordinal": 1, "type": "user_message", "text": "steer", "runId": "q", "nodeId": null, "inputIntent": "queued_turn"});
        view.apply(&json!({"kind": "event", "sequence": 2, "event": {"type": "turn-item.updated", "payload": steer}}));
        assert_eq!(view.items().count(), 0);
        view.apply(&json!({"kind": "event", "sequence": 3, "event": {"type": "run.updated", "payload": {"id": "q", "status": "running", "startedAt": "t"}}}));
        assert_eq!(view.items().count(), 1);
    }

    #[test]
    fn lists_queued_runs_in_queue_order() {
        let mut view = ThreadView::default();
        let msg = |id: &str, text: &str| json!({"id": id, "role": "user", "text": text});
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 1, "projection": {
            "runs": [
                {"id": "a", "status": "running", "ordinal": 1, "userMessageId": "m0", "startedAt": "t"},
                {"id": "q1", "status": "queued", "ordinal": 2, "queuePosition": 2, "userMessageId": "m1", "startedAt": null},
                {"id": "q2", "status": "queued", "ordinal": 3, "queuePosition": 1, "userMessageId": "m2", "startedAt": null}],
            "messages": [msg("m0", "running"), msg("m1", "first"), msg("m2", "second")],
            "attempts": [], "visibleTurnItems": []}}));
        let texts: Vec<_> = view.queued_runs().into_iter().map(|q| q.text).collect();
        assert_eq!(texts, ["second", "first"]);
        assert_eq!(view.active_run(), Some("a"));
        assert_eq!(
            view.queued_messages.len(),
            2,
            "only queued messages are kept"
        );

        let event = |seq: u64, kind: &str, payload: serde_json::Value| json!({"kind": "event", "sequence": seq, "event": {"type": kind, "payload": payload}});
        view.apply(&event(2, "run.updated", json!({"id": "q2", "status": "cancelled", "ordinal": 3, "userMessageId": "m2", "startedAt": null})));
        assert_eq!(view.queued_runs().len(), 1);
        view.apply(&event(3, "message.updated", msg("m3", "later")));
        view.apply(&event(4, "run.created", json!({"id": "q3", "status": "queued", "ordinal": 4, "userMessageId": "m3", "startedAt": null})));
        assert_eq!(view.queued_runs().last().unwrap().text, "later");
        let auto = json!({"id": "m4", "role": "user", "text": "done", "delegatedCompletion": {}});
        view.apply(&event(5, "message.updated", auto));
        view.apply(&event(6, "run.created", json!({"id": "q4", "status": "queued", "ordinal": 5, "userMessageId": "m4", "startedAt": null})));
        assert_eq!(
            view.queued_runs().len(),
            2,
            "automatic completions stay out of the queue"
        );
    }

    #[test]
    fn presents_a_pending_stop() {
        let mut view = ThreadView::default();
        let item = |id: &str, kind: &str, extra: serde_json::Value| {
            let mut v = json!({"visibility": "local", "item": {"id": id, "ordinal": 1, "type": kind, "runId": "r", "nodeId": null}});
            v["item"]
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let snapshot = |items: serde_json::Value| {
            json!({"kind": "snapshot", "snapshotSequence": 1, "projection": {
            "runs": [{"id": "r", "status": "running", "ordinal": 1, "startedAt": "t"}], "attempts": [], "visibleTurnItems": items}})
        };
        view.apply(&snapshot(json!([])));
        assert_eq!(view.run_stop(), None);
        view.apply(&snapshot(json!([item(
            "q",
            "run_interrupt_request",
            json!({})
        )])));
        assert_eq!(view.run_stop(), Some(("r".into(), false)));
        view.apply(&snapshot(json!([item(
            "q",
            "run_interrupt_request",
            json!({"unresponsiveAt": "t"})
        )])));
        assert_eq!(view.run_stop(), Some(("r".into(), true)));
        view.apply(&snapshot(json!([
            item("q", "run_interrupt_request", json!({})),
            item("s", "run_interrupt_result", json!({}))
        ])));
        assert_eq!(view.run_stop(), None);
    }

    #[test]
    fn builds_user_input_answers() {
        let questions = vec![
            json!({"id": "color", "options": [{"label": "Red", "value": "r"}, {"label": "Blue"}]}),
            json!({"id": "extras", "multiSelect": true, "required": false, "options": [{"label": "A"}]}),
        ];
        let picks = |id: &str| {
            if id == "color" {
                vec!["Blue".to_owned()]
            } else {
                vec![]
            }
        };
        let answers = build_answers(&questions, picks, "").unwrap();
        assert_eq!(answers["color"], "Blue");
        assert_eq!(answers["extras"], json!([]));
        assert!(
            build_answers(&questions, |_| vec![], "").is_none(),
            "color is required"
        );
        assert!(build_answers(&questions, |_| vec!["nope".into()], "").is_none());
        assert_eq!(
            build_answers(&questions, |_| vec![], " typed ").unwrap()["color"],
            "typed"
        );
    }

    #[test]
    fn tracks_checkpoints_newest_first() {
        let mut view = ThreadView::default();
        let cp = |id: &str, turn: Value, run: Value| {
            json!({"id": id, "runId": run, "appRunOrdinal": turn,
            "files": [{"path": "a", "kind": "modified", "additions": 2, "deletions": 1}, {"path": "b", "kind": "added", "additions": 3, "deletions": 0}]})
        };
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 1, "projection": {"runs": [], "attempts": [], "visibleTurnItems": [],
            "checkpoints": [cp("c1", json!(1), json!("r1")), cp("base", json!(null), json!(null))]}}));
        view.apply(&json!({"kind": "event", "sequence": 2, "event": {"type": "checkpoint.captured", "payload": cp("c2", json!(2), json!("r2"))}}));
        assert_eq!(
            view.checkpoints.iter().map(|c| c.turn).collect::<Vec<_>>(),
            [2, 1]
        );
        assert_eq!(
            (
                view.checkpoints[0].files,
                view.checkpoints[0].additions,
                view.checkpoints[0].deletions
            ),
            (2, 5, 1)
        );
        assert_eq!(view.latest_turn(), Some(2));
    }

    #[test]
    fn user_messages_rewind_to_the_previous_turn() {
        let mut view = ThreadView::default();
        let user = |id: &str, ord: u64, run: &str| json!({"visibility": "local", "item": {"id": id, "ordinal": ord, "type": "user_message", "runId": run, "nodeId": null, "inputIntent": "turn_start", "text": id}});
        let cp = |id: &str, run: Value, turn: Value, within: u64, status: &str| json!({"id": id, "scopeId": "s", "runId": run, "appRunOrdinal": turn, "ordinalWithinScope": within, "status": status, "files": []});
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 1, "projection": {
            "runs": [], "attempts": [],
            "checkpoints": [cp("base", json!(null), json!(null), 0, "ready"), cp("c1", json!("r1"), json!(1), 1, "ready"), cp("c2", json!("r2"), json!(2), 2, "missing")],
            "visibleTurnItems": [user("u1", 1, "r1"), user("u2", 2, "r2"), user("u3", 3, "r3")]}}));
        let turns = view.revert_turns(Some("r3"));
        assert_eq!(
            (turns.get("u1"), turns.get("u2"), turns.get("u3")),
            (Some(&0), Some(&1), None)
        );
        assert_eq!(
            view.rollback_command("t", 0, true).unwrap()["checkpointId"],
            "base"
        );
        assert!(
            view.rollback_command("t", 2, true).is_none(),
            "missing snapshot cannot restore files"
        );
        assert_eq!(
            view.rollback_command("t", 2, false).unwrap()["checkpointId"],
            "c2"
        );
    }

    #[test]
    fn unread_follows_the_web_rule() {
        let t = |extra: Value| -> ThreadShell {
            let mut v = json!({"id": "a", "projectId": "p", "title": "a", "status": "idle", "updatedAt": "2026-01-02T00:00:00Z"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value(v).unwrap()
        };
        let done = "2026-01-02T00:00:00Z";
        assert!(
            t(json!({"latestRunCompletedAt": done, "lastVisitedAt": "2026-01-01T00:00:00Z"}))
                .has_unseen_completion()
        );
        assert!(
            !t(json!({"latestRunCompletedAt": done, "lastVisitedAt": "2026-01-03T00:00:00Z"}))
                .has_unseen_completion()
        );
        assert!(
            !t(json!({"latestRunCompletedAt": done})).has_unseen_completion(),
            "never visited"
        );
        assert!(!t(json!({"status": "interrupted", "latestRunCompletedAt": done, "lastVisitedAt": "2026-01-01T00:00:00Z"})).has_unseen_completion());
        assert!(t(json!({"lastVisitedAt": "2026-01-01T00:00:00Z"})).needs_visit());
        assert!(!t(json!({"lastVisitedAt": "2026-01-02T00:00:00Z"})).needs_visit());
    }

    #[test]
    fn pages_older_history_and_drops_strays_below_the_window() {
        let mut view = ThreadView::default();
        let row = |id: &str, ord: u64| json!({"visibility": "local", "item": {"id": id, "ordinal": ord, "type": "assistant_message", "text": id, "runId": "r", "nodeId": null}});
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 1, "historyCursor": "c1", "hasMoreHistory": true,
            "projection": {"runs": [{"id": "r", "status": "completed", "startedAt": "t"}], "attempts": [], "visibleTurnItems": [row("m5", 5), row("m6", 6)]}}));
        let stray = json!({"id": "m2", "ordinal": 2, "type": "assistant_message", "text": "m2", "runId": "r", "nodeId": null});
        view.apply(&json!({"kind": "event", "sequence": 2, "event": {"type": "turn-item.updated", "payload": stray}}));
        assert_eq!(
            view.items().count(),
            2,
            "unseen older item waits for its page"
        );
        view.prepend_history(&json!({"items": [row("m1", 1), row("m2", 2), row("m5", 5)], "nextCursor": null, "hasMoreHistory": false}));
        let ids: Vec<String> = view.items().map(|i| i.id.clone()).collect();
        assert_eq!(ids, ["m1", "m2", "m5", "m6"]);
        assert!(!view.has_more_history && view.history_cursor.is_none());
    }
}
