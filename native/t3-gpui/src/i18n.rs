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
/// shortcut: English plural rules (`=n`, `one`, `other`) in every locale; Russian or Arabic
/// forms fall back to `other`.
pub fn tp(text: &str, count: usize, vars: &[(&str, &str)]) -> String {
    let message = lookup(text, None);
    let n = count.to_string();
    let chosen = plural_branch(&message, count).unwrap_or(message);
    tv(&chosen, vars).replace("{count}", &n)
}

/// The branch of a top-level `{x, plural, …}` for `count`, with its text around the block kept.
fn plural_branch(message: &str, count: usize) -> Option<String> {
    let start = message.find(", plural,")?;
    let open = message[..start].rfind('{')?;
    let mut depth = 0;
    let close = open
        + message[open..]
            .char_indices()
            .find(|&(_, c)| {
                depth += match c {
                    '{' => 1,
                    '}' => -1,
                    _ => 0,
                };
                depth == 0
            })?
            .0;
    let mut rest = &message[start + ", plural,".len()..close];
    let mut branches = Vec::new();
    while let Some(brace) = rest.find('{') {
        let key = rest[..brace].trim();
        let mut depth = 0;
        let end = brace
            + rest[brace..]
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    depth == 0
                })?
                .0;
        branches.push((key, &rest[brace + 1..end]));
        rest = &rest[end + 1..];
    }
    let exact = format!("={count}");
    let body = branches
        .iter()
        .find(|(k, _)| *k == exact)
        .or_else(|| branches.iter().find(|(k, _)| count == 1 && *k == "one"))
        .or_else(|| branches.iter().find(|(k, _)| *k == "other"))?
        .1;
    Some(format!(
        "{}{body}{}",
        &message[..open],
        &message[close + 1..]
    ))
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
