//! Proposed plan card actions (web: `ProposedPlanCard.tsx`, `proposedPlan.ts`,
//! `shared/proposedPlanText.ts`): the plan's heading as its title, copy, and save to the
//! workspace through `projects.writeFile`.
//! shortcut: no "Download as markdown"; Save to workspace covers getting the file on disk.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{WindowExt, dialog::DialogButtonProps, v_flex};
use gpui_kit::*;
use serde_json::json;

use crate::i18n::t;
use crate::{T3App, on_tokio};

fn heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    let rest = &trimmed[hashes..];
    ((1..=6).contains(&hashes) && rest.starts_with([' ', '\t']))
        .then(|| rest.trim())
        .filter(|h| !h.is_empty())
}

/// `proposedPlanTitle`: the first markdown heading.
pub fn title(markdown: &str) -> Option<String> {
    markdown.lines().find_map(heading).map(str::to_owned)
}

/// `stripDisplayedPlanMarkdown`: drops a leading heading (shown as the title) and a "Summary" one.
pub fn body(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.trim_end().lines().collect();
    let skip_blank = |mut i: usize| {
        while lines.get(i).is_some_and(|l| l.trim().is_empty()) {
            i += 1;
        }
        i
    };
    let mut start = skip_blank(usize::from(
        lines.first().is_some_and(|l| heading(l).is_some()),
    ));
    if lines
        .get(start)
        .and_then(|l| heading(l))
        .is_some_and(|h| h.eq_ignore_ascii_case("summary"))
    {
        start = skip_blank(start + 1);
    }
    lines.get(start..).unwrap_or_default().join("\n")
}

/// `buildProposedPlanMarkdownFilename`.
pub fn filename(markdown: &str) -> String {
    let title = title(markdown)
        .unwrap_or_else(|| "plan".into())
        .to_lowercase();
    let cleaned: String = title
        .chars()
        .filter(|c| !"`'\".,!?()[]{}".contains(*c))
        .collect();
    let slug = cleaned
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{}.md", if slug.is_empty() { "plan" } else { &slug })
}

impl T3App {
    pub(crate) fn open_save_plan(
        &mut self,
        markdown: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cwd) = self.thread_cwd() else {
            self.error = Some(t("This thread does not have a workspace path to save into.").into());
            return cx.notify();
        };
        let default = filename(&markdown);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(default));
        let contents = format!("{}\n", markdown.trim_end());
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, cwd, contents) =
                (app.clone(), input.clone(), cwd.clone(), contents.clone());
            dialog
                .title(t("Save plan to workspace"))
                .child(
                    v_flex()
                        .gap_2()
                        .child(t("Workspace path"))
                        .child(Input::new(&input)),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Save"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let path = input.read(cx).value().trim().to_string();
                    if path.is_empty() {
                        return false;
                    }
                    let payload = json!({ "cwd": cwd, "relativePath": path, "contents": contents });
                    app.update(cx, |this, cx| this.write_plan(payload, cx)).ok();
                    true
                })
        });
    }

    fn write_plan(&mut self, payload: serde_json::Value, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { client.call("projects.writeFile", payload).await }).await;
            this.update(cx, |app, cx| {
                app.error = match result {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(format!("{}: {e}", t("Could not save plan")).into()),
                    Err(e) => Some(format!("{}: {e}", t("Could not save plan")).into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{body, filename, title};

    #[test]
    fn reads_plan_titles_and_bodies() {
        let plan = "# Add dark mode!\n\n## Summary\n\nDo it.\n\n- step";
        assert_eq!(title(plan).as_deref(), Some("Add dark mode!"));
        assert_eq!(body(plan), "Do it.\n\n- step");
        assert_eq!(filename(plan), "add-dark-mode.md");
        assert_eq!(title("no heading"), None);
        assert_eq!(filename("no heading"), "plan.md");
        assert_eq!(body("Just text"), "Just text");
        assert_eq!(title("    # code"), None, "four spaces is code");
    }
}
