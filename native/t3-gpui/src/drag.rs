//! Sidebar drag and drop: reorder Pinned and Active, pin, unpin, settle, wake.
//! Ported from `packages/shared/src/pinOrderKey.ts`, `client-runtime` `planPinnedReorder`,
//! and `apps/web/src/components/Sidebar.logic.ts` `planSidebarThreadDrop`.
//! shortcut: every connected row counts as writable; the web also freezes rows of offline
//! environments (`resolveSidebarReorderableThreadKeys`). Upgrade with offline caching.

use std::collections::{HashMap, HashSet};

const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const TIER: &str = "0";

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|&d| d == c).unwrap_or(0)
}

fn valid(key: &str) -> bool {
    let key = key.strip_prefix(TIER).unwrap_or(key);
    !key.is_empty() && key.bytes().all(|b| DIGITS.contains(&b)) && !key.ends_with('a')
}

fn midpoint(a: &str, b: &str) -> String {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if !b.is_empty() {
        let mut n = 0;
        while ab.get(n).copied().unwrap_or(DIGITS[0]) == *bb.get(n).unwrap_or(&0) {
            n += 1;
        }
        if n > 0 {
            return b[..n].to_owned() + &midpoint(a.get(n..).unwrap_or(""), &b[n..]);
        }
    }
    let da = ab.first().map_or(0, |&c| digit(c));
    let db = bb.first().map_or(DIGITS.len(), |&c| digit(c));
    if db - da > 1 {
        return (DIGITS[((da + db) as f64 / 2.0).round() as usize] as char).to_string();
    }
    if b.len() > 1 {
        return b[..1].to_owned();
    }
    (DIGITS[da] as char).to_string() + &midpoint(a.get(1..).unwrap_or(""), "")
}

/// `pinOrderKeyBetween`: a key strictly between two neighbors (`None` = open end), or `None`
/// when the neighbors are corrupt or out of order.
pub fn key_between(before: Option<&str>, after: Option<&str>) -> Option<String> {
    let (a, b) = (before.unwrap_or(""), after.unwrap_or(""));
    if (!a.is_empty() && !valid(a)) || (!b.is_empty() && !valid(b)) || (!b.is_empty() && a >= b) {
        return None;
    }
    if let Some(b) = b.strip_prefix(TIER) {
        return Some(TIER.to_owned() + &midpoint(a.strip_prefix(TIER).unwrap_or(a), b));
    }
    if let Some(a) = a.strip_prefix(TIER) {
        return Some(TIER.to_owned() + &midpoint(a, ""));
    }
    Some(midpoint(a, b))
}

/// `generateSpreadPinOrderKeys`: `count` evenly spaced keys.
pub fn spread_keys(count: usize) -> Vec<String> {
    let base = DIGITS.len() as u64;
    let mut width = 2u32;
    let mut space = base.pow(width);
    while space <= (count as u64 + 1) * 2 {
        width += 1;
        space *= base;
    }
    let step = space as f64 / (count as f64 + 1.0);
    (0..count)
        .map(|i| {
            let mut value = (step * (i as f64 + 1.0)).round() as u64;
            if value.is_multiple_of(base) {
                value += 1;
            }
            let mut key = vec![b'a'; width as usize];
            for slot in key.iter_mut().rev() {
                *slot = DIGITS[(value % base) as usize];
                value /= base;
            }
            String::from_utf8(key).unwrap()
        })
        .collect()
}

/// `planPinnedReorder`: one new key for the moved row when its neighbors allow, else a rewrite
/// of the section with spread keys (skipping keys hidden rows still hold).
pub fn plan_reorder(
    order: &[String],
    keys: &HashMap<String, Option<String>>,
    moved: &str,
) -> Vec<(String, String)> {
    let visible: HashSet<&String> = order.iter().collect();
    let reserved: HashSet<&String> = keys
        .iter()
        .filter(|(id, k)| !visible.contains(id) && k.is_some())
        .filter_map(|(_, k)| k.as_ref())
        .collect();
    let Some(i) = order.iter().position(|id| id == moved) else {
        return Vec::new();
    };
    let key_of = |j: usize| keys.get(&order[j]).cloned().flatten();
    let before = (i > 0).then(|| key_of(i - 1));
    let after = (i + 1 < order.len()).then(|| key_of(i + 1));
    if before.as_ref().is_none_or(Option::is_some) && after.as_ref().is_none_or(Option::is_some) {
        let (b, a) = (before.flatten(), after.flatten());
        let mut key = key_between(b.as_deref(), a.as_deref());
        while let Some(k) = key.as_ref().filter(|k| reserved.contains(k)) {
            key = key_between(Some(k), a.as_deref());
        }
        if let Some(key) = key {
            return vec![(moved.to_owned(), key)];
        }
    }
    let fresh: Vec<String> = spread_keys(order.len() + reserved.len())
        .into_iter()
        .filter(|k| !reserved.contains(k))
        .take(order.len())
        .collect();
    order
        .iter()
        .zip(fresh)
        .filter(|(id, key)| keys.get(*id).cloned().flatten().as_ref() != Some(key))
        .map(|(id, key)| (id.clone(), key))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    Pinned,
    Active,
    Snoozed,
    Settled,
}

#[derive(Debug, PartialEq)]
pub enum DropPlan {
    None,
    /// Rewrite pinned order keys (`thread.pin.reorder`).
    ReorderPinned(Vec<(String, String)>),
    /// Pin the thread with a key, then rewrite other keys.
    Pin {
        key: Option<String>,
        extra: Vec<(String, String)>,
    },
    /// Into Active: leave pinned/settled/snoozed first, then write active keys.
    MoveActive {
        assignments: Vec<(String, String)>,
        unpin: bool,
        unsettle: bool,
        unsnooze: bool,
    },
    Settle,
}

/// `planSidebarThreadDrop` for a row dropped so it lands at `index` of the `target` section.
pub fn plan_drop(
    moved: &str,
    from: Section,
    target: Section,
    index: usize,
    pinned: (&[String], &HashMap<String, Option<String>>),
    active: (&[String], &HashMap<String, Option<String>>),
) -> DropPlan {
    let place = |order: &[String]| -> Vec<String> {
        let mut out: Vec<String> = order.iter().filter(|id| *id != moved).cloned().collect();
        out.insert(index.min(out.len()), moved.to_owned());
        out
    };
    match target {
        Section::Snoozed => DropPlan::None,
        Section::Settled => {
            if from == Section::Settled {
                DropPlan::None
            } else {
                DropPlan::Settle
            }
        }
        Section::Active => {
            let order = place(active.0);
            if from == Section::Active && order == active.0 {
                return DropPlan::None;
            }
            DropPlan::MoveActive {
                assignments: plan_reorder(&order, active.1, moved),
                unpin: from == Section::Pinned,
                unsettle: from == Section::Settled,
                unsnooze: from == Section::Snoozed,
            }
        }
        Section::Pinned => {
            let order = place(pinned.0);
            if from == Section::Pinned && order == pinned.0 {
                return DropPlan::None;
            }
            let assignments = plan_reorder(&order, pinned.1, moved);
            if from == Section::Pinned {
                return if assignments.is_empty() {
                    DropPlan::None
                } else {
                    DropPlan::ReorderPinned(assignments)
                };
            }
            let key = assignments
                .iter()
                .find(|(id, _)| id == moved)
                .map(|(_, k)| k.clone());
            DropPlan::Pin {
                key,
                extra: assignments
                    .into_iter()
                    .filter(|(id, _)| id != moved)
                    .collect(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(&str, Option<&str>)]) -> HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(id, k)| (id.to_string(), k.map(str::to_owned)))
            .collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keys_sort_between_their_neighbors() {
        for (a, b) in [
            (None, None),
            (None, Some("n")),
            (Some("n"), None),
            (Some("m"), Some("n")),
            (Some("mz"), Some("n")),
        ] {
            let k = key_between(a, b).unwrap();
            assert!(valid(&k), "{k}");
            assert!(
                a.is_none_or(|a| a < k.as_str()) && b.is_none_or(|b| k.as_str() < b),
                "{a:?} < {k} < {b:?}"
            );
        }
        assert_eq!(key_between(Some("n"), Some("m")), None, "out of order");
        assert_eq!(
            key_between(Some("ba"), None),
            None,
            "trailing minimum digit is corrupt"
        );
        let tiered = key_between(None, Some("0mn")).unwrap();
        assert!(tiered.starts_with('0') && tiered.as_str() < "0mn");
        let spread = spread_keys(5);
        assert!(spread.windows(2).all(|w| w[0] < w[1]) && spread.iter().all(|k| valid(k)));
    }

    #[test]
    fn reorders_with_one_write_when_neighbors_have_keys() {
        let order = ids(&["a", "c", "b"]);
        let k = keys(&[("a", Some("g")), ("b", Some("t")), ("c", Some("z"))]);
        let plan = plan_reorder(&order, &k, "c");
        assert_eq!(plan.len(), 1);
        assert!("g" < plan[0].1.as_str() && plan[0].1.as_str() < "t");
        let keyless = keys(&[("a", None), ("b", None), ("c", None)]);
        assert_eq!(
            plan_reorder(&order, &keyless, "c").len(),
            3,
            "keyless rows get spread keys"
        );
    }

    #[test]
    fn plans_become_ordered_commands() {
        let moved = ("http://a", "t1");
        let plan = DropPlan::MoveActive {
            assignments: vec![(row_key("http://b", "t2"), "m".into())],
            unpin: true,
            unsettle: false,
            unsnooze: true,
        };
        let cmds: Vec<(String, String)> = commands(plan, moved)
            .into_iter()
            .map(|(env, c)| (env, c["type"].as_str().unwrap().to_owned()))
            .collect();
        assert_eq!(
            cmds,
            [
                ("http://a".into(), "thread.unpin".into()),
                ("http://a".into(), "thread.unsnooze".into()),
                ("http://b".into(), "thread.active.reorder".into()),
            ]
        );
    }

    #[test]
    fn plans_section_moves() {
        let pinned = ids(&["p1", "p2"]);
        let active = ids(&["a1", "a2"]);
        let pk = keys(&[("p1", Some("g")), ("p2", Some("t"))]);
        let ak = keys(&[("a1", Some("0aaam")), ("a2", Some("0aaan")), ("p1", None)]);
        let plan = plan_drop(
            "a2",
            Section::Active,
            Section::Pinned,
            1,
            (&pinned, &pk),
            (&active, &ak),
        );
        let DropPlan::Pin {
            key: Some(key),
            extra,
        } = plan
        else {
            panic!("{plan:?}")
        };
        assert!("g" < key.as_str() && key.as_str() < "t" && extra.is_empty());
        let plan = plan_drop(
            "p1",
            Section::Pinned,
            Section::Active,
            0,
            (&pinned, &pk),
            (&active, &ak),
        );
        assert!(matches!(plan, DropPlan::MoveActive { unpin: true, .. }));
        assert_eq!(
            plan_drop(
                "a1",
                Section::Active,
                Section::Settled,
                0,
                (&pinned, &pk),
                (&active, &ak)
            ),
            DropPlan::Settle
        );
        assert_eq!(
            plan_drop(
                "a1",
                Section::Active,
                Section::Active,
                0,
                (&pinned, &pk),
                (&active, &ak)
            ),
            DropPlan::None
        );
    }
}

/// What a sidebar row carries while dragged; also renders the drag ghost.
#[derive(Clone)]
pub struct DraggedThread {
    pub env: String,
    pub id: String,
    pub title: gpui_kit::SharedString,
    pub section: Section,
}

impl gpui_kit::Render for DraggedThread {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::component::ActiveTheme;
        use gpui_kit::*;
        let c = cx.theme().colors;
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(c.popover)
            .border_1()
            .border_color(c.border)
            .text_sm()
            .child(self.title.clone())
    }
}

/// Row key across environments: thread ids are only unique per server.
pub fn row_key(env: &str, id: &str) -> String {
    format!("{env}\u{1}{id}")
}

/// The commands a plan sends, in order, each to its thread's environment.
pub fn commands(plan: DropPlan, moved: (&str, &str)) -> Vec<(String, serde_json::Value)> {
    use serde_json::json;
    let split = |key: &str| {
        let (env, id) = key.split_once('\u{1}').unwrap_or(("", key));
        (env.to_owned(), id.to_owned())
    };
    let one = |kind: &str, extra: serde_json::Value| {
        let mut c = json!({ "type": kind, "threadId": moved.1 });
        c.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        (moved.0.to_owned(), c)
    };
    let keyed = |kind: &'static str, assignments: Vec<(String, String)>| {
        assignments.into_iter().map(move |(key, order)| {
            let (env, id) = split(&key);
            (
                env,
                json!({ "type": kind, "threadId": id, "orderKey": order }),
            )
        })
    };
    match plan {
        DropPlan::None => Vec::new(),
        DropPlan::Settle => vec![one("thread.settle", json!({}))],
        DropPlan::ReorderPinned(a) => keyed("thread.pin.reorder", a).collect(),
        DropPlan::Pin { key, extra } => std::iter::once(one(
            "thread.pin",
            key.map_or(json!({}), |k| json!({ "orderKey": k })),
        ))
        .chain(keyed("thread.pin.reorder", extra))
        .collect(),
        DropPlan::MoveActive {
            assignments,
            unpin,
            unsettle,
            unsnooze,
        } => {
            let mut out = Vec::new();
            if unpin {
                out.push(one("thread.unpin", json!({})));
            }
            if unsettle {
                out.push(one("thread.unsettle", json!({ "reason": "user" })));
            }
            if unsnooze {
                out.push(one("thread.unsnooze", json!({ "reason": "user" })));
            }
            out.extend(keyed("thread.active.reorder", assignments));
            out
        }
    }
}
