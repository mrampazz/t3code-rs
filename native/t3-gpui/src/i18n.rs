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

#[cfg(test)]
mod tests {
    use super::*;

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
