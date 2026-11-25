//! Settings → per-provider sign-in, the managed Codex runtime and its ChatGPT accounts, and the
//! model list (web: `ProviderAuthenticationSection`, `CodexSetupSection`, `ProviderModelsSection`).
//! shortcut: the sign-in terminal is plain text plus a line input, not an emulator; upgrade if a
//! provider's login needs a full-screen TUI.
//! shortcut: no ChatGPT handoff through a primary environment and no desktop callback catcher;
//! a remote sign-in finishes by pasting the localhost URL.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::{t, tv};
use crate::model::Provider;
use crate::{T3App, on_tokio};

#[derive(Default)]
pub struct ProviderSetup {
    /// `ProviderAuthState` per instance.
    auth: HashMap<String, Value>,
    /// `ProviderInstallState` per instance (managed Codex only).
    install: HashMap<String, Value>,
    error: HashMap<String, String>,
    method: HashMap<String, String>,
    models_open: HashSet<String>,
    pending: HashSet<String>,
    subs: Vec<String>,
}

pub fn codex_managed(settings: &Value, p: &Provider) -> bool {
    p.driver == "codex"
        && settings["providerInstances"][&p.instance_id]["config"]["setupMode"] == "managed"
}

/// The web only shows the account row where the server can drive a sign-in.
pub fn has_account(settings: &Value, p: &Provider) -> bool {
    p.setup["canAuthenticate"] == true
        || (p.driver == "acpRegistry" && p.installed)
        || codex_managed(settings, p)
}

fn active(auth: &Value) -> bool {
    matches!(
        auth["phase"].as_str(),
        Some("starting" | "waiting" | "verifying")
    )
}

fn signed_in(p: &Provider, auth: &Value) -> bool {
    p.auth["status"] == "authenticated"
        || (p.auth["status"] == "unknown" && auth["phase"] == "succeeded")
}

/// The page a waiting flow wants opened, from its interaction or the legacy field.
pub fn auth_url(auth: &Value) -> Option<&str> {
    let i = &auth["interaction"];
    match i["type"].as_str() {
        Some("browser" | "deviceCode") => i["url"].as_str(),
        _ => auth["authorizationUrl"].as_str(),
    }
}

/// A pasted localhost URL can finish the flow: browser flows that accept it, or bare URL flows.
fn accepts_callback(auth: &Value) -> bool {
    auth_url(auth).is_some()
        && match auth["interaction"]["type"].as_str() {
            Some("browser") => auth["interaction"]["acceptsCallback"] == true,
            Some(_) => false,
            None => true,
        }
}

/// The account row's description, as the web words it.
pub fn account_status(p: &Provider, auth: &Value) -> String {
    let credentials = p.driver == "antigravity"
        && matches!(
            p.auth["type"].as_str(),
            Some("gemini-api-key" | "agent-platform")
        );
    if active(auth) {
        return match (auth["phase"].as_str(), auth["interaction"]["type"].as_str()) {
            (Some("starting"), _) => {
                auth["message"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        t(if credentials {
                            "Connecting…"
                        } else {
                            "Starting sign-in…"
                        })
                        .to_string()
                    })
            }
            (Some("verifying"), _) if credentials => {
                t("Checking credentials and loading models…").to_string()
            }
            (Some("verifying"), _) => t("Checking your account…").to_string(),
            (_, Some("terminal")) => t("Complete sign-in in the terminal below.").to_string(),
            (_, Some("credentials")) => t("Enter your credentials below.").to_string(),
            _ => t("Finish signing in in your browser.").to_string(),
        };
    }
    if signed_in(p, auth) {
        return match p.auth["email"]
            .as_str()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            Some(email) => format!("{} {email}", t("Signed in as")),
            None => t("Signed in.").to_string(),
        };
    }
    if p.setup["canAuthenticate"] == false {
        return t("No in-app sign-in advertised. Follow the provider's docs to finish setup.")
            .to_string();
    }
    if credentials {
        t("Connect to load models.").to_string()
    } else {
        t("Sign in to use this provider.").to_string()
    }
}

/// The managed runtime's description (web: `runtimeDescription`).
fn runtime_status(install: &Value) -> String {
    let mb = |key: &str| format!("{:.1}", install[key].as_f64().unwrap_or(0.) / 1_000_000.);
    match install["phase"].as_str() {
        Some("downloading") if install["totalBytes"].is_number() => tv(
            "Downloading {downloaded} of {total} MB.",
            &[
                ("downloaded", &mb("downloadedBytes")),
                ("total", &mb("totalBytes")),
            ],
        ),
        Some("downloading") => tv(
            "Downloading {downloaded} MB.",
            &[("downloaded", &mb("downloadedBytes"))],
        ),
        Some("extracting") => t("Installing Codex.").to_string(),
        Some("verifying") => t("Checking Codex.").to_string(),
        _ => match install["installedVersion"].as_str() {
            Some(v) if install["source"] == "local" => tv(
                "Using your installed Codex · v{version}.",
                &[("version", v)],
            ),
            Some(v) => tv("Managed by T3 Code · v{version}.", &[("version", v)]),
            None => install["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| t("T3 Code downloads and manages Codex for you.").to_string()),
        },
    }
}

fn installing(install: &Value) -> bool {
    matches!(
        install["phase"].as_str(),
        Some("downloading" | "extracting" | "verifying")
    )
}

/// Terminal output without escape sequences or carriage-return overdraws, last lines only.
pub fn terminal_text(output: &str) -> String {
    let mut text = String::new();
    let mut chars = output.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                } else if chars.next_if_eq(&']').is_some() {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\r' if chars.peek() != Some(&'\n') => {
                text.truncate(text.rfind('\n').map_or(0, |i| i + 1));
            }
            '\r' => {}
            c if c == '\n' || c == '\t' || !c.is_control() => text.push(c),
            _ => {}
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(30)..].join("\n")
}

/// The custom model list after adding or removing `slug`, as stored in the instance config.
pub fn custom_models_mutation(settings: &Value, p: &Provider, slug: &str, add: bool) -> Value {
    let mut instance = settings["providerInstances"][&p.instance_id].clone();
    if !instance.is_object() {
        instance = json!({ "driver": p.driver });
    }
    let mut models: Vec<Value> = instance["config"]["customModels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let slug_of = |m: &Value| m.as_str().or(m["slug"].as_str()).unwrap_or("").to_owned();
    models.retain(|m| slug_of(m) != slug);
    if add {
        models.push(json!(slug));
    }
    if !instance["config"].is_object() {
        instance["config"] = json!({});
    }
    instance["config"]["customModels"] = json!(models);
    json!({ "operation": "upsert", "instanceId": p.instance_id, "instance": instance })
}

impl T3App {
    /// Follows sign-in (and managed install) state for each provider that has an account row.
    pub(crate) fn subscribe_provider_setup(&mut self, cx: &mut Context<Self>) {
        self.release_provider_setup();
        let Some(client) = self.client() else { return };
        let settings = self.config().settings.clone();
        let mut streams = vec![];
        for p in self
            .config()
            .providers
            .iter()
            .filter(|p| has_account(&settings, p))
        {
            streams.push(("provider.auth.subscribe", p.instance_id.clone()));
            if codex_managed(&settings, p) {
                streams.push(("provider.install.subscribe", p.instance_id.clone()));
            }
        }
        for (tag, id) in streams {
            let (sub, mut rx) = client.request(tag, json!({ "instanceId": id }));
            self.provider_setup.subs.push(sub.clone());
            let client = client.clone();
            cx.spawn(async move |this, cx| {
                while let Some(Ok(state)) = rx.recv().await {
                    let alive = this.update(cx, |app, cx| {
                        if !app.provider_setup.subs.contains(&sub) {
                            return false;
                        }
                        let setup = &mut app.provider_setup;
                        let map = if tag == "provider.auth.subscribe" {
                            &mut setup.auth
                        } else {
                            &mut setup.install
                        };
                        let was = map.insert(id.clone(), state.clone());
                        // The server does not refresh the provider snapshot when a flow succeeds.
                        if state["phase"] == "succeeded"
                            && was.is_none_or(|w| w["phase"] != "succeeded")
                        {
                            let (client, id) = (client.clone(), id.clone());
                            drop(on_tokio(async move {
                                client
                                    .call("server.refreshProviders", json!({ "instanceId": id }))
                                    .await
                            }));
                        }
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
    }

    pub(crate) fn release_provider_setup(&mut self) {
        let subs = std::mem::take(&mut self.provider_setup.subs);
        if let Some(client) = self.client() {
            subs.iter().for_each(|s| client.cancel(s));
        }
        self.provider_setup.auth.clear();
        self.provider_setup.install.clear();
    }

    /// Runs one setup RPC for `id`; the returned state replaces the streamed one.
    fn setup_call(
        &mut self,
        id: String,
        tag: &'static str,
        payload: Value,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client() else { return };
        if !self.provider_setup.pending.insert(id.clone()) {
            return;
        }
        self.provider_setup.error.remove(&id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(tag, payload).await }).await;
            this.update(cx, |app, cx| {
                let setup = &mut app.provider_setup;
                setup.pending.remove(&id);
                match result {
                    Ok(Ok(state)) if tag.starts_with("provider.install.") => {
                        setup.install.insert(id, state);
                    }
                    Ok(Ok(state)) if state.is_object() => {
                        setup.auth.insert(id, state);
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        setup.error.insert(id, e.to_string());
                    }
                    Err(e) => {
                        setup.error.insert(id, e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn respond(&mut self, id: String, auth: &Value, response: Value, cx: &mut Context<Self>) {
        let (Some(flow), Some(interaction)) =
            (auth["flowId"].as_str(), auth["interaction"]["id"].as_str())
        else {
            return;
        };
        let payload = json!({ "instanceId": id, "flowId": flow, "interactionId": interaction, "response": response });
        self.setup_call(id, "provider.auth.respond", payload, cx);
    }

    /// One dialog with a field per `(name, label, masked)`; OK hands the values to `submit`.
    fn setup_dialog(
        &mut self,
        title: String,
        fields: Vec<(String, String, bool)>,
        submit: impl Fn(&mut T3App, Vec<(String, String)>, &mut Context<T3App>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        use gpui_kit::component::dialog::DialogButtonProps;
        use gpui_kit::component::input::{Input, InputState};
        let inputs: Vec<_> = fields
            .into_iter()
            .map(|(name, label, masked)| {
                (
                    name,
                    label,
                    cx.new(|cx| InputState::new(window, cx).masked(masked)),
                )
            })
            .collect();
        let app = cx.entity().downgrade();
        let submit = std::rc::Rc::new(submit);
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, inputs, submit) = (app.clone(), inputs.clone(), submit.clone());
            dialog
                .title(title.clone())
                .child(
                    v_flex()
                        .gap_2()
                        .children(inputs.iter().map(|(_, label, state)| {
                            v_flex()
                                .gap_1()
                                .child(div().text_xs().child(label.clone()))
                                .child(Input::new(state))
                        })),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t("Continue"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx: &mut App| {
                    let values: Vec<_> = inputs
                        .iter()
                        .map(|(name, _, s)| (name.clone(), s.read(cx).value().to_string()))
                        .collect();
                    if values.iter().all(|(_, v)| v.trim().is_empty()) {
                        return false;
                    }
                    app.update(cx, |this, cx| submit(this, values, cx)).ok();
                    true
                })
        });
    }

    fn open_sign_in_page(&mut self, id: String, auth: &Value, cx: &mut Context<Self>) {
        let Some(url) =
            auth_url(auth).filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        else {
            return;
        };
        if auth["interaction"]["type"] == "browser"
            && auth["interaction"]["requiresConsent"] == true
        {
            self.respond(
                id,
                auth,
                json!({ "type": "browser", "action": "accept" }),
                cx,
            );
        }
        cx.open_url(url);
    }

    /// Account, runtime and model rows under one provider in Settings.
    pub(crate) fn render_provider_setup(&self, p: &Provider, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors;
        let settings = &self.config().settings;
        let id = p.instance_id.clone();
        let setup = &self.provider_setup;
        let auth = setup.auth.get(&id).cloned().unwrap_or(Value::Null);
        let install = setup.install.get(&id).cloned().unwrap_or(Value::Null);
        let managed = codex_managed(settings, p);
        let busy = setup.pending.contains(&id);
        let key = |name: &str| SharedString::from(format!("provider-{name}-{id}"));
        let row = || h_flex().gap_2().py_1().text_xs().flex_wrap();
        let mut col = v_flex().pl_4().pb_2().gap_1();

        if managed {
            let running = installing(&install);
            let installed = install["installedVersion"].is_string();
            let update = installed
                && install["source"] != "local"
                && install["version"].is_string()
                && install["version"] != install["installedVersion"];
            let (start_id, cancel_id) = (id.clone(), id.clone());
            col = col.child(
                row()
                    .child(div().w(px(110.)).child(t("Runtime")))
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.muted_foreground)
                            .child(runtime_status(&install)),
                    )
                    .when(running, |r| {
                        let op = install["operationId"].clone();
                        r.child(
                            Button::new(key("install-cancel"))
                                .xsmall()
                                .ghost()
                                .label(t("Cancel"))
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let payload =
                                        json!({ "instanceId": cancel_id, "operationId": op });
                                    this.setup_call(
                                        cancel_id.clone(),
                                        "provider.install.cancel",
                                        payload,
                                        cx,
                                    )
                                })),
                        )
                    })
                    .when(
                        !running && (!installed || update) && p.setup["canInstall"] == true,
                        |r| {
                            r.child(
                                Button::new(key("install"))
                                    .xsmall()
                                    .outline()
                                    .label(t(if update { "Update" } else { "Install" }))
                                    .disabled(busy || !p.enabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let payload = json!({ "instanceId": start_id });
                                        this.setup_call(
                                            start_id.clone(),
                                            "provider.install.start",
                                            payload,
                                            cx,
                                        )
                                    })),
                            )
                        },
                    ),
            );
        }

        if has_account(settings, p) {
            let is_active = active(&auth);
            let is_signed_in = signed_in(p, &auth);
            let methods: Vec<Value> = auth["methods"].as_array().cloned().unwrap_or_default();
            let chosen = setup
                .method
                .get(&id)
                .filter(|m| methods.iter().any(|x| x["id"] == **m))
                .cloned();
            let ready = if managed {
                install["installedVersion"].is_string()
            } else {
                p.installed
            };
            let can_start = !is_active && p.setup["canAuthenticate"] != false && auth.is_object();
            let url = auth_url(&auth).map(str::to_owned);
            let mut controls = h_flex().gap_1().flex_wrap();
            if !is_active && methods.len() > 1 {
                let app = cx.entity().downgrade();
                let label = chosen
                    .as_ref()
                    .and_then(|m| methods.iter().find(|x| x["id"] == **m))
                    .and_then(|m| m["accountEmail"].as_str().or(m["name"].as_str()))
                    .map_or_else(|| t("Provider default").to_string(), str::to_owned);
                let (menu_id, menu_methods) = (id.clone(), methods.clone());
                controls = controls.child(
                    Button::new(key("method"))
                        .xsmall()
                        .outline()
                        .label(label)
                        .dropdown_menu(move |mut menu, _, _| {
                            for m in &menu_methods {
                                let (app, id, mid) = (
                                    app.clone(),
                                    menu_id.clone(),
                                    m["id"].as_str().unwrap_or("").to_owned(),
                                );
                                let name = m["accountEmail"]
                                    .as_str()
                                    .or(m["name"].as_str())
                                    .unwrap_or("")
                                    .to_owned();
                                menu = menu.item(PopupMenuItem::new(name).on_click(
                                    move |_, _, cx| {
                                        app.update(cx, |this, cx| {
                                            this.provider_setup
                                                .method
                                                .insert(id.clone(), mid.clone());
                                            cx.notify();
                                        })
                                        .ok();
                                    },
                                ));
                            }
                            menu
                        }),
                );
            }
            if let Some(url) = url.clone() {
                let (open_id, open_auth) = (id.clone(), auth.clone());
                controls = controls
                    .child(
                        Button::new(key("open"))
                            .xsmall()
                            .outline()
                            .label(t("Open browser"))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_sign_in_page(open_id.clone(), &open_auth, cx)
                            })),
                    )
                    .child(
                        Button::new(key("copy"))
                            .xsmall()
                            .ghost()
                            .label(t("Copy sign-in link"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))
                            }),
                    );
            }
            if accepts_callback(&auth) {
                let (cb_id, flow) = (id.clone(), auth["flowId"].clone());
                controls = controls.child(Button::new(key("callback")).xsmall().ghost().label(t("Paste redirect URL…")).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let (id, flow) = (cb_id.clone(), flow.clone());
                        let label = t("If the final localhost page does not load, paste its full URL here.").to_string();
                        this.setup_dialog(t("Continue").to_string(), vec![("callback".into(), label, true)], move |this, values, cx| {
                            let payload = json!({ "instanceId": id, "flowId": flow, "callbackUrl": values[0].1.trim() });
                            this.setup_call(id.clone(), "provider.auth.complete", payload, cx);
                        }, window, cx);
                    }),
                ));
            }
            if is_active && auth["flowId"].is_string() {
                let (cancel_id, flow) = (id.clone(), auth["flowId"].clone());
                controls = controls.child(
                    Button::new(key("cancel"))
                        .xsmall()
                        .ghost()
                        .label(t("Cancel"))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let payload = json!({ "instanceId": cancel_id, "flowId": flow });
                            this.setup_call(cancel_id.clone(), "provider.auth.cancel", payload, cx)
                        })),
                );
            } else if p.setup["canAuthenticate"] == false {
                if let Some(docs) = p.setup["documentationUrl"].as_str().map(str::to_owned) {
                    controls = controls.child(
                        Button::new(key("docs"))
                            .xsmall()
                            .outline()
                            .label(t("Open docs"))
                            .on_click(move |_, _, cx| cx.open_url(&docs)),
                    );
                }
            } else if can_start {
                let retry = matches!(auth["phase"].as_str(), Some("failed" | "cancelled"));
                let label = if is_signed_in {
                    "Change account"
                } else if retry {
                    "Retry sign-in"
                } else {
                    "Sign in"
                };
                let (start_id, method) = (id.clone(), chosen.clone());
                controls = controls.child(
                    Button::new(key("sign-in"))
                        .xsmall()
                        .outline()
                        .label(t(label))
                        .disabled(busy || !p.enabled || !ready)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut payload =
                                json!({ "instanceId": start_id, "callbackMode": "server" });
                            if let Some(m) = &method {
                                payload["methodId"] = json!(m);
                            }
                            this.setup_call(start_id.clone(), "provider.auth.start", payload, cx)
                        })),
                );
            }
            if !is_active
                && is_signed_in
                && p.auth["canLogout"]
                    .as_bool()
                    .unwrap_or(p.setup["canAuthenticate"] == true)
            {
                let (out_id, name) = (
                    id.clone(),
                    p.display_name.clone().unwrap_or_else(|| p.driver.clone()),
                );
                controls = controls.child(Button::new(key("sign-out")).xsmall().ghost().label(t("Sign out")).disabled(busy).on_click(
                    cx.listener(move |_, _, window, cx| {
                        use gpui_kit::component::WindowExt;
                        let (app, id) = (cx.entity().downgrade(), out_id.clone());
                        let message = tv(
                            "Sign out of {provider} on {environment}? This stops running threads that share this sign-in. Thread history is kept.",
                            &[("provider", &name), ("environment", &t("this environment"))],
                        );
                        window.open_alert_dialog(cx, move |dialog, _, _| {
                            let (app, id) = (app.clone(), id.clone());
                            dialog.confirm().title(t("Sign out")).description(message.clone()).ok_text(t("Sign out")).on_ok(
                                move |_, _, cx| {
                                    app.update(cx, |this, cx| {
                                        this.setup_call(id.clone(), "provider.auth.logout", json!({ "instanceId": id }), cx)
                                    })
                                    .ok();
                                    true
                                },
                            )
                        });
                    }),
                ));
            }
            col = col.child(
                row()
                    .child(div().w(px(110.)).child(t("Account")))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(c.muted_foreground)
                            .child(account_status(p, &auth)),
                    )
                    .child(controls),
            );
            let interaction = &auth["interaction"];
            if let Some(code) = interaction["userCode"]
                .as_str()
                .filter(|_| interaction["type"] == "deviceCode")
            {
                col = col.child(
                    row()
                        .child(t("Enter code"))
                        .child(
                            div()
                                .font_family("monospace")
                                .text_color(c.foreground)
                                .child(code.to_owned()),
                        )
                        .child(t("in your browser.")),
                );
            }
            if interaction["type"] == "terminal" {
                let (term_id, term_auth) = (id.clone(), auth.clone());
                col = col
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(c.border)
                            .bg(c.secondary)
                            .font_family("monospace")
                            .text_xs()
                            .whitespace_normal()
                            .child(terminal_text(interaction["output"].as_str().unwrap_or(""))),
                    )
                    .child(
                        row().child(
                            Button::new(key("term-input"))
                                .xsmall()
                                .outline()
                                .label(t("Send input…"))
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let (id, auth) = (term_id.clone(), term_auth.clone());
                                    this.setup_dialog(
                                        t("Send input…").to_string(),
                                        vec![("data".into(), String::new(), false)],
                                        move |this, values, cx| {
                                            let data = format!("{}\r", values[0].1);
                                            this.respond(
                                                id.clone(),
                                                &auth,
                                                json!({ "type": "terminal", "data": data }),
                                                cx,
                                            );
                                        },
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                    );
            }
            if interaction["type"] == "credentials" {
                let fields: Vec<(String, String, bool)> = interaction["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        (
                            f["name"].as_str().unwrap_or("").to_owned(),
                            f["label"].as_str().unwrap_or("").to_owned(),
                            f["secret"] == true,
                        )
                    })
                    .collect();
                let (cred_id, cred_auth) = (id.clone(), auth.clone());
                col = col.child(
                    row().child(
                        Button::new(key("credentials"))
                            .xsmall()
                            .outline()
                            .label(t("Connect"))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let (id, auth) = (cred_id.clone(), cred_auth.clone());
                                this.setup_dialog(
                                    t("Enter your credentials below.").to_string(),
                                    fields.clone(),
                                    move |this, values, cx| {
                                        let values: serde_json::Map<String, Value> = values
                                            .into_iter()
                                            .map(|(k, v)| (k, json!(v)))
                                            .collect();
                                        this.respond(
                                            id.clone(),
                                            &auth,
                                            json!({ "type": "credentials", "values": values }),
                                            cx,
                                        );
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
                );
            }
            for m in methods.iter().filter(|m| m["canRemove"] == true) {
                let (rm_id, mid) = (id.clone(), m["id"].clone());
                let name = m["accountEmail"]
                    .as_str()
                    .or(m["name"].as_str())
                    .unwrap_or("")
                    .to_owned();
                col = col.child(
                    row()
                        .child(div().w(px(110.)))
                        .child(div().flex_1().text_color(c.muted_foreground).child(name))
                        .child(
                            Button::new(SharedString::from(format!("provider-rm-{id}-{mid}")))
                                .xsmall()
                                .ghost()
                                .label(t("Remove"))
                                .disabled(busy || is_active)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let payload = json!({ "instanceId": rm_id, "methodId": mid });
                                    this.setup_call(
                                        rm_id.clone(),
                                        "provider.auth.removeMethod",
                                        payload,
                                        cx,
                                    )
                                })),
                        ),
                );
            }
            let message = setup.error.get(&id).cloned().or_else(|| {
                (auth["phase"] == "failed")
                    .then(|| auth["message"].as_str().map(str::to_owned))
                    .flatten()
            });
            col = col.children(message.map(|m| div().text_xs().text_color(c.danger).child(m)));
        }

        if p.driver == "codex" {
            let (mode_id, settings) = (id.clone(), settings.clone());
            col = col.child(
                row().child(div().w(px(110.)).child(t("Setup"))).child(
                    Button::new(key("mode"))
                        .xsmall()
                        .ghost()
                        .label(t(if managed { "Use existing CLI" } else { "Let T3 Code manage Codex" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut instance = settings["providerInstances"][&mode_id].clone();
                            if !instance.is_object() {
                                instance = json!({ "driver": "codex" });
                            }
                            if !instance["config"].is_object() {
                                instance["config"] = json!({});
                            }
                            instance["config"]["setupMode"] = json!(if managed { "existing" } else { "managed" });
                            let mutation = json!({ "operation": "upsert", "instanceId": mode_id, "instance": instance });
                            this.update_settings(json!({ "patch": {}, "providerInstanceMutation": mutation }), cx);
                        })),
                ),
            );
        }

        let open = setup.models_open.contains(&id);
        let toggle_id = id.clone();
        col = col.child(
            row().child(
                Button::new(key("models"))
                    .xsmall()
                    .ghost()
                    .label(format!(
                        "{} {} ({})",
                        if open { "▾" } else { "▸" },
                        t("Models"),
                        p.models.len()
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let open = &mut this.provider_setup.models_open;
                        if !open.remove(&toggle_id) {
                            open.insert(toggle_id.clone());
                        }
                        cx.notify();
                    })),
            ),
        );
        if open {
            let hidden: Vec<String> = settings["providerModelPreferences"][&id]["hiddenModels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            for m in &p.models {
                let is_hidden = !m.is_custom && hidden.contains(&m.slug);
                let (slug, model_id, provider) = (m.slug.clone(), id.clone(), p.clone());
                let settings = settings.clone();
                let custom = m.is_custom;
                col = col.child(
                    row()
                        .pl_4()
                        .child(div().flex_1().min_w_0().truncate().when(is_hidden, |d| d.text_color(c.muted_foreground)).child(m.name.clone()))
                        .child(div().text_color(c.muted_foreground).child(m.slug.clone()))
                        .child(
                            Button::new(SharedString::from(format!("model-{id}-{}", m.slug)))
                                .xsmall()
                                .ghost()
                                .label(t(if custom { "Remove" } else if is_hidden { "Show" } else { "Hide" }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let payload = if custom {
                                        let mutation = custom_models_mutation(&settings, &provider, &slug, false);
                                        json!({ "patch": {}, "providerInstanceMutation": mutation })
                                    } else {
                                        let entry = json!({ "provider": model_id, "model": slug, "hidden": !is_hidden });
                                        json!({ "patch": { "setModelsHidden": [entry] } })
                                    };
                                    this.update_settings(payload, cx);
                                })),
                        ),
                );
            }
            let (add_settings, add_provider) = (settings.clone(), p.clone());
            col = col.child(row().pl_4().child(Button::new(key("model-add")).xsmall().outline().label(t("Add custom model")).on_click(
                cx.listener(move |this, _, window, cx| {
                    let (settings, provider) = (add_settings.clone(), add_provider.clone());
                    this.setup_dialog(t("Add custom model").to_string(), vec![("slug".into(), t("Model slug").to_string(), false)], move |this, values, cx| {
                        let mutation = custom_models_mutation(&settings, &provider, values[0].1.trim(), true);
                        this.update_settings(json!({ "patch": {}, "providerInstanceMutation": mutation }), cx);
                    }, window, cx);
                }),
            )));
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{account_status, auth_url, custom_models_mutation, terminal_text};
    use serde_json::json;

    fn provider(v: serde_json::Value) -> crate::model::Provider {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn terminal_text_drops_escapes_and_overdraws() {
        let out = "\x1b[1mLogin\x1b[0m\r\nProgress 10%\rProgress 99%\r\n\x1b]0;title\x07done";
        assert_eq!(terminal_text(out), "Login\nProgress 99%\ndone");
    }

    #[test]
    fn auth_url_prefers_the_interaction() {
        let auth = json!({ "authorizationUrl": "https://a", "interaction": { "type": "deviceCode", "url": "https://b" } });
        assert_eq!(auth_url(&auth), Some("https://b"));
        assert_eq!(
            auth_url(&json!({ "authorizationUrl": "https://a", "interaction": null })),
            Some("https://a")
        );
    }

    #[test]
    fn account_status_follows_the_flow() {
        let p = provider(
            json!({ "instanceId": "claude", "auth": { "status": "unauthenticated" }, "setup": { "canAuthenticate": true } }),
        );
        assert_eq!(
            account_status(&p, &json!({ "phase": "idle" })),
            "Sign in to use this provider."
        );
        let waiting = json!({ "phase": "waiting", "interaction": { "type": "terminal" } });
        assert_eq!(
            account_status(&p, &waiting),
            "Complete sign-in in the terminal below."
        );
        let p = provider(
            json!({ "instanceId": "claude", "auth": { "status": "authenticated", "email": "a@b.c" } }),
        );
        assert_eq!(
            account_status(&p, &json!({ "phase": "idle" })),
            "Signed in as a@b.c"
        );
    }

    #[test]
    fn custom_models_keep_other_config() {
        let p = provider(json!({ "instanceId": "codex", "driver": "codex" }));
        let settings = json!({ "providerInstances": { "codex": { "driver": "codex", "config": { "binaryPath": "/x", "customModels": ["a", { "slug": "b" }] } } } });
        let added = custom_models_mutation(&settings, &p, "c", true);
        assert_eq!(
            added["instance"]["config"]["customModels"],
            json!(["a", { "slug": "b" }, "c"])
        );
        assert_eq!(added["instance"]["config"]["binaryPath"], "/x");
        let removed = custom_models_mutation(&settings, &p, "b", false);
        assert_eq!(removed["instance"]["config"]["customModels"], json!(["a"]));
    }
}
