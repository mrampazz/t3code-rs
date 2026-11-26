//! Project actions (web: `ProjectScriptsControl.tsx`, `projectScriptEditor.tsx`,
//! `useProjectScriptEditing.ts`, `shared/projectScripts.ts`): run one in a new thread terminal,
//! add, edit and delete them. Scripts live in server settings, not on the project.
//! shortcut: every run opens a new terminal (the web reuses an idle one), and the editor leaves
//! out keybindings, icons, preview URLs and wait-for-setup; existing values are kept on save.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{WindowExt, dialog::DialogButtonProps, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::T3App;
use crate::header::{details_label, details_row, details_split};
use crate::i18n::{t, tv};

const MAX_SCRIPT_ID_LENGTH: usize = 24;

type AppAction = Box<dyn Fn(&mut T3App, &mut Window, &mut Context<T3App>)>;

/// `resolveProjectScripts`: per-project override, then folded defaults, then the legacy
/// override, then the project's own scripts, then the defaults.
pub fn resolve(settings: &Value, project_id: &str, project_scripts: &Value) -> Vec<Value> {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let defaults = &settings["defaultProjectScripts"];
    if let Some(o) = settings["projectSettingsOverrides"][project_id].get("defaultProjectScripts") {
        return list(o);
    }
    if settings["projectSettingsFolded"] == true {
        return list(defaults);
    }
    match settings["projectScriptOverrides"].get(project_id) {
        Some(Value::Null) => list(defaults),
        Some(legacy) => list(legacy),
        None if project_scripts.as_array().is_some_and(|a| !a.is_empty()) => list(project_scripts),
        None => list(defaults),
    }
}

/// `primaryProjectScript`: the first plain action, else the first that is not a settle hook.
pub fn primary(scripts: &[Value]) -> Option<&Value> {
    scripts
        .iter()
        .find(|s| s["runOnWorktreeCreate"] != true && s["runOnSettle"] != true)
        .or_else(|| scripts.iter().find(|s| s["runOnSettle"] != true))
}

/// `nextProjectScriptId`: a slug of the name, suffixed until unused.
pub fn next_id(name: &str, existing: &[Value]) -> String {
    let taken = |id: &str| existing.iter().any(|s| s["id"] == id);
    let slug = name
        .trim()
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let mut base: String = slug.chars().take(MAX_SCRIPT_ID_LENGTH).collect();
    base = base.trim_end_matches('-').to_owned();
    if base.is_empty() {
        base = "script".into();
    }
    if !taken(&base) {
        return base;
    }
    (2..10_000)
        .map(|n| {
            let suffix = format!("-{n}");
            let keep = MAX_SCRIPT_ID_LENGTH.saturating_sub(suffix.len()).max(1);
            format!("{}{suffix}", &base[..base.len().min(keep)])
        })
        .find(|id| !taken(id))
        .unwrap_or(base)
}

/// The fields the editor owns, merged onto the existing script so others survive.
/// Only one action may run on worktree creation and one on settle (`releaseClaimedRoles`).
pub fn upsert(scripts: &[Value], id: Option<&str>, edit: &Value) -> Vec<Value> {
    let claims = |key: &str| edit[key] == true;
    let mut next: Vec<Value> = scripts
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if Some(s["id"].as_str().unwrap_or("")) == id {
                for (k, v) in edit.as_object().unwrap() {
                    s[k] = v.clone();
                }
            } else {
                for key in ["runOnWorktreeCreate", "runOnSettle"] {
                    if claims(key) && s[key] == true {
                        s[key] = json!(false);
                    }
                }
            }
            s
        })
        .collect();
    if id.is_none() {
        let mut script =
            json!({ "id": next_id(edit["name"].as_str().unwrap_or(""), scripts), "icon": "play" });
        for (k, v) in edit.as_object().unwrap() {
            script[k] = v.clone();
        }
        next.push(script);
    }
    next
}

/// The `server.updateSettings` patch that stores a project's scripts.
pub fn settings_patch(
    settings: &Value,
    project_id: &str,
    scripts: Vec<Value>,
    per_project: bool,
) -> Value {
    if per_project {
        let mut overrides = settings["projectSettingsOverrides"][project_id].clone();
        if !overrides.is_object() {
            overrides = json!({});
        }
        overrides["defaultProjectScripts"] = json!(scripts);
        json!({ "patch": { "projectSettingsOverrides": { project_id: overrides } } })
    } else {
        json!({ "patch": { "projectScriptOverrides": { project_id: scripts } } })
    }
}

/// Editor checkboxes; lives in an entity so the dialog can re-render them.
struct Flags {
    on_create: bool,
    on_settle: bool,
    stop_on_settle: bool,
}

impl T3App {
    /// The open thread's project and its effective scripts.
    fn project_scripts(&self) -> Option<(String, Vec<Value>)> {
        let thread = self
            .selected
            .as_ref()
            .and_then(|id| self.shell().threads.iter().find(|t| &t.id == id))?;
        let project = self
            .shell()
            .projects
            .iter()
            .find(|p| p.id == thread.project_id)?;
        Some((
            project.id.clone(),
            resolve(&self.config().settings, &project.id, &project.scripts),
        ))
    }

    /// (id, name) of the open thread's project actions.
    pub(crate) fn script_names(&self) -> Vec<(String, String)> {
        self.project_scripts()
            .map_or_else(Vec::new, |(_, scripts)| {
                scripts
                    .iter()
                    .map(|s| {
                        (
                            s["id"].as_str().unwrap_or("").to_owned(),
                            s["name"].as_str().unwrap_or("").to_owned(),
                        )
                    })
                    .collect()
            })
    }

    /// Runs a script in a new terminal; its command is typed once the shell is up.
    pub(crate) fn run_script(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some((project, scripts)) = self.project_scripts() else {
            return false;
        };
        let Some(script) = scripts.into_iter().find(|s| s["id"] == id) else {
            return false;
        };
        self.last_script.insert(project, id.to_owned());
        self.script_launch = Some(script);
        self.new_terminal(cx);
        true
    }

    fn save_scripts(&mut self, scripts: Vec<Value>, cx: &mut Context<Self>) {
        let Some((project, _)) = self.project_scripts() else {
            return;
        };
        let per_project = self.config().project_settings_overrides;
        let patch = settings_patch(&self.config().settings, &project, scripts, per_project);
        self.update_settings(patch, cx);
    }

    fn open_script_editor(
        &mut self,
        existing: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field =
            |text: &str, placeholder: String, window: &mut Window, cx: &mut Context<Self>| {
                let text = text.to_owned();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(text)
                })
            };
        let s = existing.clone().unwrap_or_default();
        let name = field(s["name"].as_str().unwrap_or(""), t("Name"), window, cx);
        let command = field(
            s["command"].as_str().unwrap_or(""),
            t("Command"),
            window,
            cx,
        );
        let flags = cx.new(|_| Flags {
            on_create: s["runOnWorktreeCreate"] == true,
            on_settle: s["runOnSettle"] == true,
            stop_on_settle: s["stopOnSettle"] == true,
        });
        let id = existing.and_then(|s| s["id"].as_str().map(str::to_owned));
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let f = flags.read(cx);
            let checkbox = |key: &'static str, label: &str, checked: bool| {
                let flags = flags.clone();
                Checkbox::new(key).label(t(label)).checked(checked).on_click(move |v, window, cx| {
                    let v = *v;
                    flags.update(cx, |f, _| match key {
                        "on-create" => f.on_create = v,
                        "on-settle" => f.on_settle = v,
                        _ => f.stop_on_settle = v,
                    });
                    window.refresh();
                })
            };
            let (app, name, command, flags, id) = (app.clone(), name.clone(), command.clone(), flags.clone(), id.clone());
            dialog
                .title(if id.is_some() { t("Edit Action") } else { t("Add Action") })
                .child(
                    v_flex()
                        .gap_2()
                        .child(t("Actions are project-scoped commands you can run from the top bar or keybindings."))
                        .child(Input::new(&name))
                        .child(Input::new(&command))
                        .child(checkbox("on-create", "Run automatically on worktree creation", f.on_create))
                        .child(checkbox("on-settle", "Run in the thread's worktree when the thread settles", f.on_settle))
                        .child(checkbox("stop-on-settle", "Stop this action when the thread settles", f.stop_on_settle)),
                )
                .button_props(DialogButtonProps::default().ok_text(if id.is_some() { t("Save changes") } else { t("Save action") }).show_cancel(true))
                .on_ok(move |_, _, cx: &mut App| {
                    let name = name.read(cx).value().trim().to_string();
                    let command = command.read(cx).value().trim().to_string();
                    if name.is_empty() || command.is_empty() {
                        return false;
                    }
                    let f = flags.read(cx);
                    let edit = json!({ "name": name, "command": command, "runOnWorktreeCreate": f.on_create,
                        "runOnSettle": f.on_settle, "stopOnSettle": f.stop_on_settle });
                    let id = id.clone();
                    app.update(cx, |this, cx| {
                        let Some((_, scripts)) = this.project_scripts() else { return };
                        this.save_scripts(upsert(&scripts, id.as_deref(), &edit), cx);
                    })
                    .ok();
                    true
                })
        });
    }

    /// The project scripts row (web: `ProjectScriptsControl displayMode="panel"`).
    pub(crate) fn render_scripts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((project, scripts)) = self.project_scripts() else {
            return div().into_any_element();
        };
        let current = self
            .last_script
            .get(&project)
            .and_then(|id| scripts.iter().find(|s| s["id"] == id.as_str()))
            .or_else(|| primary(&scripts))
            .cloned();
        let app = cx.entity().downgrade();
        let Some(script) = current else {
            return details_row(
                "add-script",
                "plus",
                details_label(t("Add project script")),
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_script_editor(None, window, cx)))
            .into_any_element();
        };
        let id = script["id"].as_str().unwrap_or("").to_owned();
        let name = script["name"].as_str().unwrap_or("").to_owned();
        let run = details_row(
            "run-script",
            script_icon(&script),
            details_label(name.clone()),
            cx,
        )
        .tooltip(tv("Run {name}", &[("name", &name)]))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.run_script(&id, cx);
        }));
        details_split(
            "script-actions",
            run,
            move |mut menu, window, cx| {
                let click = |app: &WeakEntity<T3App>, f: AppAction| {
                    let app = app.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        app.update(cx, |this, cx| f(this, window, cx)).ok();
                    }
                };
                for script in &scripts {
                    let name = script["name"].as_str().unwrap_or("").to_owned();
                    let id = script["id"].as_str().unwrap_or("").to_owned();
                    let run = click(
                        &app,
                        Box::new(move |this, _, cx| {
                            this.run_script(&id, cx);
                        }),
                    );
                    menu = menu.item(
                        PopupMenuItem::new(tv("Run {name}", &[("name", &name)])).on_click(run),
                    );
                }
                if !scripts.is_empty() {
                    menu = menu.separator();
                    for script in &scripts {
                        let name = script["name"].as_str().unwrap_or("").to_owned();
                        let (app, scripts, script) = (app.clone(), scripts.clone(), script.clone());
                        menu = menu.submenu(
                            tv("Edit {name}", &[("name", &name)]),
                            window,
                            cx,
                            move |sub, _, _| {
                                let edit = {
                                    let s = script.clone();
                                    click(
                                        &app,
                                        Box::new(move |this, window, cx| {
                                            this.open_script_editor(Some(s.clone()), window, cx)
                                        }),
                                    )
                                };
                                let remaining: Vec<Value> = scripts
                                    .iter()
                                    .filter(|s| s["id"] != script["id"])
                                    .cloned()
                                    .collect();
                                let delete = click(
                                    &app,
                                    Box::new(move |this, _, cx| {
                                        this.save_scripts(remaining.clone(), cx)
                                    }),
                                );
                                sub.item(PopupMenuItem::new(t("Edit Action")).on_click(edit))
                                    .item(PopupMenuItem::new(t("Delete action")).on_click(delete))
                            },
                        );
                    }
                    menu = menu.separator();
                }
                menu.item(PopupMenuItem::new(t("Add project action…")).on_click(click(
                    &app,
                    Box::new(|this, window, cx| this.open_script_editor(None, window, cx)),
                )))
            },
            cx,
        )
        .into_any_element()
    }
}

/// A script's Lucide icon (web: `ScriptIcon`).
fn script_icon(script: &Value) -> &'static str {
    match script["icon"].as_str() {
        Some("test") => "flask-conical",
        Some("lint") => "list-checks",
        Some("configure") => "wrench",
        Some("build") => "hammer",
        Some("debug") => "bug",
        _ => "play",
    }
}

#[cfg(test)]
mod tests {
    use super::{next_id, primary, resolve, settings_patch, upsert};
    use serde_json::json;

    #[test]
    fn resolves_scripts_like_the_web() {
        let own = json!([{"id": "own"}]);
        let defaults = json!({"defaultProjectScripts": [{"id": "def"}], "projectScriptOverrides": {}, "projectSettingsOverrides": {}});
        assert_eq!(resolve(&defaults, "p", &own)[0]["id"], "own");
        assert_eq!(resolve(&defaults, "p", &json!([]))[0]["id"], "def");
        let legacy = json!({"projectScriptOverrides": {"p": [{"id": "leg"}]}, "projectSettingsOverrides": {}});
        assert_eq!(resolve(&legacy, "p", &own)[0]["id"], "leg");
        let cleared = json!({"defaultProjectScripts": [{"id": "def"}], "projectScriptOverrides": {"p": null}});
        assert_eq!(resolve(&cleared, "p", &own)[0]["id"], "def");
        let per = json!({"projectSettingsOverrides": {"p": {"defaultProjectScripts": []}}, "projectScriptOverrides": {"p": [{"id": "leg"}]}});
        assert!(resolve(&per, "p", &own).is_empty());
    }

    #[test]
    fn picks_the_primary_action() {
        let scripts = [
            json!({"id": "setup", "runOnWorktreeCreate": true}),
            json!({"id": "dev", "runOnWorktreeCreate": false}),
        ];
        assert_eq!(primary(&scripts).unwrap()["id"], "dev");
        assert_eq!(primary(&scripts[..1]).unwrap()["id"], "setup");
        assert!(primary(&[json!({"id": "s", "runOnSettle": true})]).is_none());
    }

    #[test]
    fn slugs_unique_ids() {
        assert_eq!(next_id("Run Tests!", &[]), "run-tests");
        assert_eq!(next_id("  ", &[]), "script");
        assert_eq!(
            next_id("dev", &[json!({"id": "dev"}), json!({"id": "dev-2"})]),
            "dev-3"
        );
        assert_eq!(
            next_id("a very long action name that overflows", &[]),
            "a-very-long-action-name"
        );
    }

    #[test]
    fn upserts_and_releases_claimed_roles() {
        let scripts = vec![
            json!({"id": "setup", "name": "Setup", "command": "x", "icon": "build", "runOnWorktreeCreate": true, "previewUrl": "http://l"}),
        ];
        let edit = json!({"name": "Install", "command": "pnpm i", "runOnWorktreeCreate": true, "runOnSettle": false, "stopOnSettle": false});
        let added = upsert(&scripts, None, &edit);
        assert_eq!(added[0]["runOnWorktreeCreate"], false);
        assert_eq!(added[1]["id"], "install");
        assert_eq!(added[1]["icon"], "play");
        let edited = upsert(&scripts, Some("setup"), &edit);
        assert_eq!(edited.len(), 1);
        assert_eq!(edited[0]["name"], "Install");
        assert_eq!(
            edited[0]["previewUrl"], "http://l",
            "fields the editor does not own survive"
        );
    }

    #[test]
    fn patches_the_right_settings_key() {
        let settings = json!({"projectSettingsOverrides": {"p": {"autoPull": true}}});
        let patch = settings_patch(&settings, "p", vec![json!({"id": "a"})], true);
        assert_eq!(
            patch["patch"]["projectSettingsOverrides"]["p"]["autoPull"],
            true
        );
        assert_eq!(
            patch["patch"]["projectSettingsOverrides"]["p"]["defaultProjectScripts"][0]["id"],
            "a"
        );
        let legacy = settings_patch(&settings, "p", vec![], false);
        assert_eq!(legacy["patch"]["projectScriptOverrides"]["p"], json!([]));
    }
}
