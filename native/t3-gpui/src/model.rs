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
    pub default_model_selection: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadShell {
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
}

impl ThreadShell {
    pub fn is_running(&self) -> bool {
        matches!(
            self.status.as_str(),
            Some("preparing" | "queued" | "starting" | "running" | "waiting")
        )
    }
}

#[derive(Default)]
pub struct Shell {
    pub projects: Vec<Project>,
    pub threads: Vec<ThreadShell>,
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
                self.projects = decode_vec(&snap["projects"]);
                self.threads = decode_vec(&snap["threads"]);
            }
            Some("synchronized") => self.synced = true,
            Some("project.updated") => {
                if let Some(p) = decode::<Project>(&item["project"]) {
                    upsert(&mut self.projects, p, |p| &p.id);
                }
            }
            Some("project.removed") => {
                let id = item["projectId"].as_str().unwrap_or_default();
                self.projects.retain(|p| p.id != id);
            }
            Some("thread.updated") => {
                let Some(t) = decode::<ThreadShell>(&item["thread"]) else {
                    return;
                };
                if item["location"] == "active" {
                    upsert(&mut self.threads, t, |t| &t.id);
                } else {
                    self.threads.retain(|x| x.id != t.id);
                }
            }
            Some("thread.removed") => {
                let id = item["threadId"].as_str().unwrap_or_default();
                self.threads.retain(|t| t.id != id);
            }
            _ => {}
        }
        self.threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }

    pub fn threads_of<'a>(&'a self, project_id: &'a str) -> impl Iterator<Item = &'a ThreadShell> {
        self.threads
            .iter()
            .filter(move |t| t.project_id == project_id)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub instance_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub availability: Value,
    #[serde(default)]
    pub models: Vec<ProviderModel>,
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
            }
            Some("providerStatuses") => self.providers = decode_vec(&event["payload"]["providers"]),
            _ => {}
        }
    }

    /// Every model of every available provider, as (label, selection).
    pub fn model_options(&self) -> Vec<(String, Value)> {
        self.providers
            .iter()
            .filter(|p| p.availability != "unavailable")
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
            .find(|p| p.availability != "unavailable" && !p.models.is_empty())?;
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

#[derive(Clone, Debug)]
struct Run {
    status: String,
    started: bool,
}

/// Thread timeline, mirroring `client-runtime/src/state/orchestrationV2Projection.ts`.
#[derive(Default)]
pub struct ThreadView {
    rows: Vec<Row>,
    runs: HashMap<String, Run>,
    /// runId -> root node ids of superseded attempts.
    superseded: HashMap<String, HashSet<String>>,
    /// Resume cursor for `afterSequence`; 0 until the first snapshot.
    pub sequence: u64,
}

impl ThreadView {
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
                self.rows = projection["visibleTurnItems"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|row| {
                        Some(Row {
                            local: row["visibility"] == "local",
                            item: TurnItem::from_value(&row["item"])?,
                        })
                    })
                    .collect();
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
                    Some("run-attempt.created" | "run-attempt.updated") => {
                        self.apply_attempt(payload)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn apply_run(&mut self, run: &Value) {
        if let (Some(id), Some(status)) = (run["id"].as_str(), run["status"].as_str()) {
            let started = !run["startedAt"].is_null();
            self.runs.insert(
                id.to_owned(),
                Run {
                    status: status.to_owned(),
                    started,
                },
            );
        }
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
        }
        let at = self
            .rows
            .iter()
            .position(|r| r.local && (r.item.ordinal, &r.item.id) > (item.ordinal, &item.id))
            .unwrap_or(self.rows.len());
        self.rows.insert(at, Row { local: true, item });
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
        shell.apply(&json!({"kind": "thread.updated", "sequence": 3, "location": "archive", "thread": thread("a", "3")}));
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
            {"instanceId": "off", "availability": "unavailable", "models": [{"slug": "x", "name": "X", "isCustom": false}]},
            {"instanceId": "codex", "models": [{"slug": "a", "name": "A", "isCustom": false}, {"slug": "b", "name": "B", "isCustom": false, "isDefault": true}]},
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
}
