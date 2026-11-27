//! Translations from the web's catalogs (`apps/web/src/i18n/translations/*.json`, see
//! `docs/internals/14_i18n.md`). Keys are GT's `hashSource` of the English ICU message, so a
//! string written exactly like the web's resolves to the web's translation; others stay English.

use std::collections::HashMap;
use std::sync::RwLock;

use sha2::{Digest, Sha256};

macro_rules! catalog {
    ($code:literal) => {
        (
            $code,
            include_str!(concat!(
                "../../../apps/web/src/i18n/translations/",
                $code,
                ".json"
            )),
        )
    };
}

/// Locales the web ships, with their names in their own language.
pub const LOCALES: [(&str, &str); 10] = [
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("pt-BR", "Português (Brasil)"),
    ("ru", "Русский"),
    ("sv", "Svenska"),
    ("zh", "中文"),
];

const CATALOGS: [(&str, &str); 9] = [
    catalog!("de"),
    catalog!("es"),
    catalog!("fr"),
    catalog!("ja"),
    catalog!("ko"),
    catalog!("pt-BR"),
    catalog!("ru"),
    catalog!("sv"),
    catalog!("zh"),
];

static ACTIVE: RwLock<Option<(String, HashMap<String, String>)>> = RwLock::new(None);

/// GT `hashSource` for an ICU string: SHA-256 of the sorted-key JSON, first 16 hex digits.
pub fn key(text: &str, context: Option<&str>) -> String {
    // Keys inserted in sorted order, like GT's stableStringify: serde_json may preserve
    // insertion order (another dependency enables `preserve_order`).
    let mut obj = serde_json::Map::new();
    if let Some(context) = context {
        obj.insert("context".into(), context.into());
    }
    obj.insert("dataFormat".into(), "ICU".into());
    obj.insert("source".into(), text.into());
    let digest = Sha256::digest(serde_json::Value::Object(obj).to_string().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The locale from `LC_ALL` / `LANG` (`de_DE.UTF-8` → `de`), if the web ships it.
pub fn system_locale() -> String {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let tag = raw.split('.').next().unwrap_or("").replace('_', "-");
    if tag == "pt-BR" {
        return tag;
    }
    let lang = tag.split('-').next().unwrap_or("").to_lowercase();
    LOCALES
        .iter()
        .find(|(code, _)| *code == lang)
        .map_or("en".into(), |(code, _)| code.to_string())
}

pub fn set_locale(code: &str) {
    let map = CATALOGS
        .iter()
        .find(|(c, _)| *c == code)
        .and_then(|(_, json)| serde_json::from_str::<HashMap<String, serde_json::Value>>(json).ok())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| Some((k, v.as_str()?.to_owned())))
                .collect()
        });
    *ACTIVE.write().unwrap() = map.map(|m| (code.to_owned(), m));
}

pub fn locale() -> String {
    ACTIVE
        .read()
        .unwrap()
        .as_ref()
        .map_or("en".into(), |(code, _)| code.clone())
}

fn lookup(text: &str, context: Option<&str>) -> String {
    let active = ACTIVE.read().unwrap();
    active
        .as_ref()
        .and_then(|(_, map)| map.get(&key(text, context)).cloned())
        .unwrap_or_else(|| text.to_owned())
}

/// Translates a UI string (English when the catalog has no entry).
pub fn t(text: &str) -> String {
    lookup(text, None)
}

/// Translates a short word whose web string carries a `$context`.
pub fn tc(text: &str, context: &str) -> String {
    lookup(text, Some(context))
}

/// Translates and fills `{name}` variables.
pub fn tv(text: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(lookup(text, None), |out, (name, value)| {
        out.replace(&format!("{{{name}}}"), value)
    })
}

/// Translates an ICU `{count, plural, …}` message and fills its variables (`count` included).
pub fn tp(text: &str, count: usize, vars: &[(&str, &str)]) -> String {
    let n = count.to_string();
    let mut args: Vec<(&str, &str)> = vec![("count", &n)];
    args.extend_from_slice(vars);
    tf(text, &args)
}

/// Translates an ICU message and formats it: `{name}`, and nested `{name, plural, …}` /
/// `{name, select, …}` with the active locale's plural rules. Numbers are passed as text.
pub fn tf(text: &str, vars: &[(&str, &str)]) -> String {
    format_icu(&lookup(text, None), vars, &locale())
}

/// CLDR cardinal categories for the catalogs' locales (integers only).
fn plural_category(locale: &str, n: u64) -> &'static str {
    let lang = locale.split(['-', '_']).next().unwrap_or(locale);
    let million = n != 0 && n.is_multiple_of(1_000_000);
    match lang {
        "ja" | "ko" | "zh" => "other",
        "ru" => match (n % 10, n % 100) {
            (1, r) if r != 11 => "one",
            (2..=4, r) if !(12..=14).contains(&r) => "few",
            _ => "many",
        },
        "fr" | "pt" if n <= 1 => "one",
        "fr" | "pt" | "es" if million => "many",
        "es" if n == 1 => "one",
        _ if n == 1 => "one",
        _ => "other",
    }
}

/// The text inside the brace that opens at `open`, and the index after its close.
fn braced(text: &str, open: usize) -> Option<(&str, usize)> {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[open + 1..open + i], open + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn format_icu(message: &str, vars: &[(&str, &str)], locale: &str) -> String {
    let value = |name: &str| {
        vars.iter()
            .find(|(k, _)| *k == name.trim())
            .map(|(_, v)| *v)
    };
    let mut out = String::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some((inner, after)) = braced(rest, open) else {
            out.push_str(&rest[open..]);
            return out;
        };
        let mut parts = inner.splitn(3, ',');
        let (name, kind, body) = (parts.next().unwrap_or(""), parts.next(), parts.next());
        match (kind.map(str::trim), body) {
            (Some(kind @ ("plural" | "select")), Some(body)) => {
                let arg = value(name).unwrap_or("");
                let mut branches = Vec::new();
                let mut cases = body;
                while let Some(start) = cases.find('{') {
                    let key = cases[..start].trim();
                    let Some((text, end)) = braced(cases, start) else {
                        break;
                    };
                    branches.push((key, text));
                    cases = &cases[end..];
                }
                let pick = |key: &str| branches.iter().find(|(k, _)| *k == key).map(|b| b.1);
                let chosen = if kind == "plural" {
                    let n: u64 = arg.parse().unwrap_or(0);
                    pick(&format!("={n}")).or_else(|| pick(plural_category(locale, n)))
                } else {
                    pick(arg)
                };
                if let Some(text) = chosen.or_else(|| pick("other")) {
                    out.push_str(&format_icu(text, vars, locale));
                }
            }
            _ => match value(name) {
                Some(v) => out.push_str(v),
                None => out.push_str(&rest[open..after]),
            },
        }
        rest = &rest[after..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_plural_branches() {
        let m = "{count, plural, one {Ran {count} command} other {Ran {count} commands}}";
        assert_eq!(tp(m, 1, &[]), "Ran 1 command");
        assert_eq!(tp(m, 3, &[]), "Ran 3 commands");
        let m = "{count, plural, =1 {Checked linked pull requests} other {Checked {count} times}}";
        assert_eq!(tp(m, 1, &[]), "Checked linked pull requests");
        assert_eq!(tp(m, 2, &[]), "Checked 2 times");
        let nested = "{messages, plural, one {Sent {messages} message to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Sent {messages} messages to {threads, plural, one {{threads} thread} other {{threads} threads}}}}";
        assert_eq!(
            tf(nested, &[("messages", "2"), ("threads", "1")]),
            "Sent 2 messages to 1 thread"
        );
        let select =
            "{status, select, completed {Linked {changeRequest} #{number}} other {Linking}}";
        assert_eq!(
            tf(
                select,
                &[
                    ("status", "completed"),
                    ("changeRequest", "PR"),
                    ("number", "4")
                ]
            ),
            "Linked PR #4"
        );
        assert_eq!(super::plural_category("ru", 3), "few");
        assert_eq!(super::plural_category("ru", 11), "many");
        assert_eq!(super::plural_category("fr", 0), "one");
        assert_eq!(super::plural_category("ja", 1), "other");
    }

    #[test]
    fn keys_match_the_web_catalogs() {
        assert_eq!(key("Settings", None), "0a75e464d833bca8");
        assert_eq!(
            key("Approve", Some("verb, response to an approval request")),
            "6a03aa0ca9085274"
        );
    }

    #[test]
    fn translates_with_fallback() {
        set_locale("de");
        assert_eq!(t("Settings"), "Einstellungen");
        assert_eq!(
            t("A string the web never had"),
            "A string the web never had"
        );
        assert_eq!(tv("{x} and {y}", &[("x", "1"), ("y", "2")]), "1 and 2");
        set_locale("en");
        assert_eq!(t("Settings"), "Settings");
    }
}
