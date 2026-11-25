//! Server-defined keybindings (`ServerConfig.keybindings`), resolved like the web's
//! `resolveShortcutCommand` (apps/web/src/keybindings.ts, packages/shared/src/keybindingMatching.ts).

use std::collections::HashMap;

use serde_json::{Value, json};

/// The DOM key name the web matches for a gpui key (`normalizeEventKey`).
pub fn dom_key(gpui_key: &str) -> String {
    match gpui_key {
        "up" => "arrowup".into(),
        "down" => "arrowdown".into(),
        "left" => "arrowleft".into(),
        "right" => "arrowright".into(),
        "space" => " ".into(),
        "esc" => "escape".into(),
        other => other.to_lowercase(),
    }
}

#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub meta: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

fn eval(node: &Value, ctx: &HashMap<&str, bool>) -> bool {
    match node["type"].as_str() {
        Some("identifier") => match node["name"].as_str() {
            Some("true") => true,
            Some("false") | None => false,
            Some(name) => ctx.get(name).copied().unwrap_or(false),
        },
        Some("not") => !eval(&node["node"], ctx),
        Some("and") => eval(&node["left"], ctx) && eval(&node["right"], ctx),
        Some("or") => eval(&node["left"], ctx) || eval(&node["right"], ctx),
        _ => false,
    }
}

/// The command bound to a key press, searching the newest rules first.
pub fn resolve(
    rules: &[Value],
    key: &str,
    mods: Mods,
    mac: bool,
    ctx: &HashMap<&str, bool>,
) -> Option<String> {
    let key = dom_key(key);
    rules.iter().rev().find_map(|rule| {
        if !rule["whenAst"].is_null() && !eval(&rule["whenAst"], ctx) {
            return None;
        }
        let s = &rule["shortcut"];
        let flag = |k: &str| s[k] == true;
        let (meta, ctrl) = (
            flag("metaKey") || (flag("modKey") && mac),
            flag("ctrlKey") || (flag("modKey") && !mac),
        );
        let matches = s["key"].as_str() == Some(key.as_str())
            && mods.meta == meta
            && mods.ctrl == ctrl
            && mods.shift == flag("shiftKey")
            && mods.alt == flag("altKey");
        matches.then(|| rule["command"].as_str().unwrap_or("").to_owned())
    })
}

/// A resolved rule's shortcut as a keybinding value (`mod+shift+k`), like the web writes it.
pub fn value_of(rule: &Value) -> String {
    let s = &rule["shortcut"];
    let mut parts: Vec<&str> = [
        ("modKey", "mod"),
        ("ctrlKey", "ctrl"),
        ("metaKey", "meta"),
        ("altKey", "alt"),
        ("shiftKey", "shift"),
    ]
    .into_iter()
    .filter(|(flag, _)| s[*flag] == true)
    .map(|(_, token)| token)
    .collect();
    let key = match s["key"].as_str().unwrap_or("") {
        " " => "space",
        "escape" => "esc",
        k => k,
    };
    parts.push(key);
    parts.join("+")
}

/// A `whenAst` written back as the `when` string it came from, for replacing or removing a rule.
pub fn when_of(node: &Value) -> Option<String> {
    let wrap = |n: &Value| {
        let s = when_of(n)?;
        Some(if matches!(n["type"].as_str(), Some("and" | "or")) {
            format!("({s})")
        } else {
            s
        })
    };
    Some(match node["type"].as_str()? {
        "identifier" => node["name"].as_str()?.to_owned(),
        "not" => format!("!{}", wrap(&node["node"])?),
        "and" => format!("{} && {}", wrap(&node["left"])?, wrap(&node["right"])?),
        "or" => format!("{} || {}", wrap(&node["left"])?, wrap(&node["right"])?),
        _ => return None,
    })
}

/// The `when` field for a rule, omitted when the rule has none.
pub fn rule_target(rule: &Value) -> Value {
    let mut target = json!({ "key": value_of(rule), "command": rule["command"] });
    if let Some(when) = when_of(&rule["whenAst"]) {
        target["when"] = json!(when);
    }
    target
}

/// The `DEFAULT_KEYBINDINGS` this client acts on, used until the server's list arrives.
pub fn defaults() -> Vec<Value> {
    let not_terminal =
        json!({ "type": "not", "node": { "type": "identifier", "name": "terminalFocus" } });
    let rule = |key: &str, mods: &[&str], command: &str, when: Option<&Value>| {
        let has = |m: &str| mods.contains(&m);
        json!({
            "command": command,
            "shortcut": { "key": key, "metaKey": false, "ctrlKey": false, "modKey": has("mod"),
                          "shiftKey": has("shift"), "altKey": has("alt") },
            "whenAst": when,
        })
    };
    vec![
        rule("b", &["mod"], "sidebar.toggle", None),
        rule("j", &["mod"], "terminal.toggle", None),
        rule("d", &["mod"], "diff.toggle", Some(&not_terminal)),
        rule("k", &["mod"], "commandPalette.toggle", Some(&not_terminal)),
        rule("n", &["mod"], "chat.new", Some(&not_terminal)),
        rule("f", &["mod"], "chat.find", Some(&not_terminal)),
        rule("o", &["mod"], "editor.openFavorite", None),
        rule("[", &["mod"], "navigation.back", Some(&not_terminal)),
        rule("]", &["mod"], "navigation.forward", Some(&not_terminal)),
        rule("b", &["mod", "alt"], "rightPanel.toggle", None),
        rule("w", &["mod"], "rightPanel.close", Some(&not_terminal)),
        rule(
            "a",
            &["mod", "alt", "shift"],
            "appearance.cycle",
            Some(&not_terminal),
        ),
        rule("n", &["mod", "shift"], "chat.newLocal", Some(&not_terminal)),
        rule("a", &["mod", "shift"], "composer.mode", Some(&not_terminal)),
        rule(
            "k",
            &["mod", "shift"],
            "pullRequest.copyNumber",
            Some(&not_terminal),
        ),
        rule("[", &["mod", "shift"], "thread.previous", None),
        rule("]", &["mod", "shift"], "thread.next", None),
        rule("s", &["mod", "shift"], "thread.settle", Some(&not_terminal)),
        rule("p", &["mod", "shift"], "thread.pin", Some(&not_terminal)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_rules_back_as_values() {
        let rule = json!({"command": "chat.find", "shortcut": {"key": "f", "modKey": true, "shiftKey": true},
            "whenAst": {"type": "and", "left": {"type": "not", "node": {"type": "identifier", "name": "terminalFocus"}},
                "right": {"type": "or", "left": {"type": "identifier", "name": "a"}, "right": {"type": "identifier", "name": "b"}}}});
        assert_eq!(value_of(&rule), "mod+shift+f");
        assert_eq!(
            rule_target(&rule),
            json!({"key": "mod+shift+f", "command": "chat.find", "when": "!terminalFocus && (a || b)"})
        );
        assert!(
            rule_target(&json!({"command": "x", "shortcut": {"key": " "}}))
                .get("when")
                .is_none()
        );
    }

    #[test]
    fn resolves_like_the_web() {
        let rules = defaults();
        let mut ctx = HashMap::new();
        let cmd = Mods {
            meta: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(&rules, "k", cmd, true, &ctx).as_deref(),
            Some("commandPalette.toggle")
        );
        assert_eq!(
            resolve(
                &rules,
                "k",
                Mods {
                    ctrl: true,
                    ..Default::default()
                },
                false,
                &ctx
            )
            .as_deref(),
            Some("commandPalette.toggle"),
            "mod is ctrl off macOS"
        );
        assert_eq!(
            resolve(&rules, "k", cmd, false, &ctx),
            None,
            "meta is not mod off macOS"
        );
        ctx.insert("terminalFocus", true);
        assert_eq!(
            resolve(&rules, "k", cmd, true, &ctx),
            None,
            "!terminalFocus"
        );
        assert_eq!(
            resolve(&rules, "j", cmd, true, &ctx).as_deref(),
            Some("terminal.toggle")
        );
        let shift_cmd = Mods {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(&rules, "]", shift_cmd, true, &ctx).as_deref(),
            Some("thread.next")
        );
        // Later rules win.
        let mut custom = rules.clone();
        custom.push(json!({"command": "diff.toggle", "shortcut": {"key": "j", "modKey": true}}));
        assert_eq!(
            resolve(&custom, "j", cmd, true, &ctx).as_deref(),
            Some("diff.toggle")
        );
        assert_eq!(dom_key("up"), "arrowup");
    }
}
