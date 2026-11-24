//! Client projections of `orchestration.subscribeShell` / `subscribeThread`.
//! Only fields the UI reads are typed; serde ignores the rest so newer servers still decode.

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
}

impl ThreadShell {
    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), Some("preparing" | "queued" | "starting" | "running" | "waiting"))
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
                let Some(t) = decode::<ThreadShell>(&item["thread"]) else { return };
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
        self.threads.iter().filter(move |t| t.project_id == project_id)
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
}

impl ServerConfig {
    pub fn apply(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("snapshot") => self.providers = decode_vec(&event["config"]["providers"]),
            Some("providerStatuses") => self.providers = decode_vec(&event["payload"]["providers"]),
            _ => {}
        }
    }

    /// "Provider · Model" with display names when this server knows them.
    pub fn label(&self, selection: &Value) -> String {
        let (instance, slug) = (selection["instanceId"].as_str().unwrap_or(""), selection["model"].as_str().unwrap_or(""));
        let provider = self.providers.iter().find(|p| p.instance_id == instance);
        let model = provider.and_then(|p| p.models.iter().find(|m| m.slug == slug)).map_or(slug, |m| m.name.as_str());
        let provider = provider.and_then(|p| p.display_name.as_deref()).unwrap_or(instance);
        format!("{provider} · {model}")
    }

    /// The project's default if set, else the first available provider's default model.
    pub fn model_for(&self, project: Option<&Project>) -> Option<Value> {
        if let Some(selection) = project.map(|p| &p.default_model_selection).filter(|v| v.is_object()) {
            return Some(selection.clone());
        }
        let provider = self.providers.iter().find(|p| p.availability != "unavailable" && !p.models.is_empty())?;
        let model = provider.models.iter().find(|m| m.is_default).unwrap_or(&provider.models[0]);
        Some(serde_json::json!({ "instanceId": provider.instance_id, "model": model.slug }))
    }
}

/// One transcript row: a raw `OrchestrationV2TurnItem`.
#[derive(Clone, Debug, Deserialize)]
pub struct TurnItem {
    pub id: String,
    pub ordinal: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Value,
}

#[derive(Default)]
pub struct ThreadView {
    pub items: Vec<TurnItem>,
    /// Resume cursor for `afterSequence`; 0 until the first snapshot.
    pub sequence: u64,
}

impl ThreadView {
    pub fn apply(&mut self, item: &Value) {
        match item["kind"].as_str() {
            Some("snapshot") => {
                self.sequence = item["snapshotSequence"].as_u64().unwrap_or(0);
                self.items = decode_vec(&item["projection"]["turnItems"]);
            }
            Some("event") => {
                self.sequence = self.sequence.max(item["sequence"].as_u64().unwrap_or(0));
                if item["event"]["type"] != "turn-item.updated" {
                    return;
                }
                if let Some(t) = decode::<TurnItem>(&item["event"]["payload"]) {
                    upsert(&mut self.items, t, |t| &t.id);
                }
            }
            _ => return,
        }
        self.items.sort_by_key(|t| t.ordinal);
    }
}

fn decode<T: for<'de> Deserialize<'de>>(v: &Value) -> Option<T> {
    serde_json::from_value(v.clone()).map_err(|e| eprintln!("decode: {e}")).ok()
}

/// Skips rows that fail to decode instead of dropping the whole list.
fn decode_vec<T: for<'de> Deserialize<'de>>(v: &Value) -> Vec<T> {
    v.as_array().into_iter().flatten().filter_map(decode).collect()
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
        assert_eq!(shell.threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["b"]);
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
        assert_eq!(config.model_for(None), Some(json!({"instanceId": "codex", "model": "b"})));
        let project: Project = serde_json::from_value(json!({"id": "p", "title": "P", "defaultModelSelection": {"instanceId": "c", "model": "m"}})).unwrap();
        assert_eq!(config.model_for(Some(&project)).unwrap()["model"], "m");
        assert_eq!(config.label(&json!({"instanceId": "codex", "model": "b"})), "codex · B");
        assert_eq!(config.label(&json!({"instanceId": "c", "model": "m"})), "c · m");
    }

    #[test]
    fn thread_view_upserts_turn_items_in_order() {
        let mut view = ThreadView::default();
        let item = |id: &str, ord: u64, text: &str| json!({"id": id, "ordinal": ord, "type": "assistant_message", "text": text, "status": "running"});
        view.apply(&json!({"kind": "snapshot", "snapshotSequence": 5, "projection": {"turnItems": [item("b", 2, "x")]}}));
        view.apply(&json!({"kind": "event", "sequence": 6, "event": {"type": "turn-item.updated", "payload": item("a", 1, "hi")}}));
        view.apply(&json!({"kind": "event", "sequence": 7, "event": {"type": "turn-item.updated", "payload": item("b", 2, "xy")}}));
        view.apply(&json!({"kind": "event", "sequence": 8, "event": {"type": "run.updated", "payload": {}}}));
        assert_eq!(view.sequence, 8);
        assert_eq!(view.items.iter().map(|t| t.text.clone().unwrap()).collect::<Vec<_>>(), ["hi", "xy"]);
    }
}
