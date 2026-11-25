//! Workflows (web: `workflows/WorkflowRunFeed.tsx`, contract `workflow.ts`), shown in Settings when
//! the Beta is on: questions runs ask a person (answered with one of their choices) and runs that
//! have not ended, with Resume for a paused run and Cancel. Streams while Settings is open.
//! shortcut: no run detail (steps, results), no definitions editor; finished runs are left out.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// Runs that have not ended, newest first.
pub fn active_runs(snapshot: &Value) -> Vec<Value> {
    let mut runs: Vec<Value> = snapshot["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| matches!(r["status"].as_str(), Some("running" | "waiting" | "paused")))
        .cloned()
        .collect();
    runs.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
    runs
}

#[derive(Default)]
pub struct WorkflowsState {
    snapshot: Value,
    sub: Option<String>,
}

impl T3App {
    pub(crate) fn subscribe_workflows(&mut self, cx: &mut Context<Self>) {
        self.release_workflows();
        if self.config().settings["workflowsEnabled"] != true {
            return;
        }
        let Some(client) = self.client() else { return };
        let (sub, mut rx) = client.request("workflows.subscribe", json!({}));
        self.workflows.sub = Some(sub.clone());
        cx.spawn(async move |this, cx| {
            while let Some(Ok(snapshot)) = rx.recv().await {
                let alive = this.update(cx, |app, cx| {
                    if app.workflows.sub.as_deref() != Some(sub.as_str()) {
                        return false;
                    }
                    app.workflows.snapshot = snapshot;
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn release_workflows(&mut self) {
        if let (Some(client), Some(sub)) = (self.client(), self.workflows.sub.take()) {
            client.cancel(&sub);
        }
    }

    fn workflow_call(&mut self, method: &'static str, payload: Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, payload).await }).await;
            if let Some(e) = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            } {
                this.update(cx, |app, cx| {
                    app.settings_status = Some(e.into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn render_workflows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let snapshot = &self.workflows.snapshot;
        let questions = snapshot["questions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let runs = active_runs(snapshot);
        v_flex()
            .text_sm()
            .children(questions.into_iter().enumerate().map(|(i, q)| {
                let choices: Vec<String> = q["request"]["choices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.as_str().map(str::to_owned))
                    .collect();
                let (run_id, name) = (q["runId"].clone(), q["name"].clone());
                v_flex()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(c.border)
                    .child(q["request"]["title"].as_str().unwrap_or("").to_owned())
                    .child(h_flex().gap_1().flex_wrap().children(
                        choices.into_iter().enumerate().map(|(j, choice)| {
                            let payload =
                                json!({ "runId": run_id, "name": name, "choice": choice });
                            Button::new(SharedString::from(format!("wf-answer-{i}-{j}")))
                                .xsmall()
                                .outline()
                                .label(choice)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workflow_call("workflows.answer", payload.clone(), cx)
                                }))
                        }),
                    ))
            }))
            .children(runs.into_iter().enumerate().map(|(i, run)| {
                let run_id = run["runId"].clone();
                let paused = run["status"] == "paused";
                let detail = [run["status"].as_str(), run["detail"].as_str()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let (resume, cancel) = (run_id.clone(), run_id);
                h_flex()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(run["title"].as_str().unwrap_or("").to_owned())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .children(paused.then(|| {
                        Button::new(("wf-resume", i))
                            .xsmall()
                            .ghost()
                            .label(t("Resume"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workflow_call(
                                    "workflows.resume",
                                    json!({ "runId": resume }),
                                    cx,
                                )
                            }))
                    }))
                    .child(
                        Button::new(("wf-cancel", i))
                            .xsmall()
                            .ghost()
                            .label(t("Cancel"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workflow_call(
                                    "workflows.cancel",
                                    json!({ "runId": cancel }),
                                    cx,
                                )
                            })),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::active_runs;
    use serde_json::json;

    #[test]
    fn keeps_runs_that_have_not_ended() {
        let snapshot = json!({"runs": [
            {"runId": "a", "status": "completed", "updatedAt": "3"},
            {"runId": "b", "status": "paused", "updatedAt": "1"},
            {"runId": "c", "status": "running", "updatedAt": "2"}]});
        let ids: Vec<_> = active_runs(&snapshot)
            .iter()
            .map(|r| r["runId"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["c", "b"]);
    }
}
