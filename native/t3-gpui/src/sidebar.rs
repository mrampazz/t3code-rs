//! Sidebar sections and snooze presets, ported from `apps/web/src/components/Sidebar.tsx`
//! (`partitionSidebarThreads`) and `client-runtime/src/state/{threadSort,threadSettled}.ts`.

use jiff::civil::Weekday;
use jiff::{Timestamp, ToSpan, Zoned};
use serde_json::Value;

use crate::model::ThreadShell;

pub fn ts_ms(iso: Option<&str>) -> Option<i64> {
    iso?.parse::<Timestamp>().ok().map(|t| t.as_millisecond())
}

#[derive(Default)]
pub struct Sections<'a> {
    pub pinned: Vec<&'a ThreadShell>,
    pub active: Vec<&'a ThreadShell>,
    pub snoozed: Vec<&'a ThreadShell>,
    pub settled: Vec<&'a ThreadShell>,
}

/// `effectiveSnoozed`: a future wake time, unless a new request raised its hand meanwhile.
pub fn is_snoozed(t: &ThreadShell, now_ms: i64) -> bool {
    let Some(wake) = ts_ms(t.snoozed_until.as_deref()) else {
        return false;
    };
    if wake <= now_ms {
        return false;
    }
    let request = &t.pending_runtime_request;
    if request.is_object() {
        if request["kind"] != "user_input" {
            return false;
        }
        // Snoozing an existing question means "ask me later"; only a newer one wakes it.
        let asked = ts_ms(request["createdAt"].as_str());
        let snoozed = ts_ms(t.snoozed_at.as_deref());
        if !matches!((asked, snoozed), (Some(a), Some(s)) if a <= s) {
            return false;
        }
    }
    true
}

pub fn sections(threads: &[ThreadShell], now_ms: i64) -> Sections<'_> {
    let mut out = Sections::default();
    for t in threads.iter().filter(|t| t.is_listed()) {
        if is_snoozed(t, now_ms) {
            out.snoozed.push(t);
        } else if t.settled_override.as_deref() == Some("settled") {
            out.settled.push(t);
        } else if t.pinned_at.is_some() {
            out.pinned.push(t);
        } else {
            out.active.push(t);
        }
    }
    let created = |t: &ThreadShell| ts_ms(Some(&t.created_at)).unwrap_or(0);
    out.pinned
        .sort_by(|a, b| match (&a.pin_order_key, &b.pin_order_key) {
            (Some(x), Some(y)) => x.cmp(y).then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => created(b).cmp(&created(a)).then_with(|| a.id.cmp(&b.id)),
        });
    out.active
        .sort_by_cached_key(|t| (active_sort_key(t), t.id.clone()));
    out.snoozed
        .sort_by_key(|t| ts_ms(t.snoozed_until.as_deref()).unwrap_or(0));
    let settled_ms = |t: &ThreadShell| {
        ts_ms(t.settled_at.as_deref())
            .or_else(|| ts_ms(t.latest_user_message_at.as_deref()))
            .or_else(|| ts_ms(Some(&t.updated_at)))
            .unwrap_or(0)
    };
    out.settled.sort_by(|a, b| {
        settled_ms(b)
            .cmp(&settled_ms(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

const IMPLIED_KEY_WIDTH: u32 = 10;

/// `activeThreadSortKey`: arranged threads keep their key; the rest get an implied key from
/// their anchor time, newest first, in the leading tier "0".
pub fn active_sort_key(t: &ThreadShell) -> String {
    if let Some(key) = &t.active_order_key {
        return key.clone();
    }
    let max = 26i64.pow(IMPLIED_KEY_WIDTH) - 1;
    let anchor = ts_ms(Some(&t.created_at))
        .unwrap_or(0)
        .max(ts_ms(t.unsettled_at.as_deref()).unwrap_or(0));
    let mut value = max - anchor.clamp(0, max);
    let mut digits = vec![b'a'; IMPLIED_KEY_WIDTH as usize];
    for slot in digits.iter_mut().rev() {
        *slot = b'a' + (value % 26) as u8;
        value /= 26;
    }
    format!("0{}n", String::from_utf8(digits).unwrap())
}

/// One snooze choice: label and wake time.
#[derive(Debug, PartialEq)]
pub struct SnoozePreset {
    pub label: String,
    pub until: Timestamp,
}

/// `DEFAULT_SNOOZE_PRESETS` (packages/contracts/src/settings.ts).
pub fn default_presets() -> Vec<Value> {
    serde_json::from_str(
        r#"[{"kind":"after","minutes":60},{"kind":"after","minutes":180},
            {"kind":"at","day":"today","time":"18:00"},{"kind":"at","day":"tomorrow","time":"09:00"},
            {"kind":"at","day":"monday","time":"09:00"}]"#,
    )
    .unwrap()
}

fn weekday(day: &str) -> Option<Weekday> {
    Some(match day {
        "sunday" => Weekday::Sunday,
        "monday" => Weekday::Monday,
        "tuesday" => Weekday::Tuesday,
        "wednesday" => Weekday::Wednesday,
        "thursday" => Weekday::Thursday,
        "friday" => Weekday::Friday,
        "saturday" => Weekday::Saturday,
        _ => return None,
    })
}

fn plural(n: i64, one: &str) -> String {
    if n == 1 {
        format!("In 1 {one}")
    } else {
        format!("In {n} {one}s")
    }
}

/// A `SnoozePresetSetting` from `90m` / `2h` / `3d`, or `<day> HH:MM` (today, tomorrow, a weekday).
pub fn parse_preset(input: &str) -> Option<Value> {
    let input = input.trim().to_lowercase();
    if let Some((day, time)) = input.split_once(' ') {
        let valid_day = matches!(day, "today" | "tomorrow") || weekday(day).is_some();
        let (h, m) = time.trim().split_once(':')?;
        let ok = h.len() == 2
            && m.len() == 2
            && h.parse::<u8>().ok()? < 24
            && m.parse::<u8>().ok()? < 60;
        return (valid_day && ok)
            .then(|| serde_json::json!({ "kind": "at", "day": day, "time": time.trim() }));
    }
    let split = input.find(|c: char| !c.is_ascii_digit())?;
    let amount: u64 = input[..split].parse().ok().filter(|n| *n > 0)?;
    let minutes = match &input[split..] {
        "m" => amount,
        "h" => amount * 60,
        "d" => amount * 24 * 60,
        _ => return None,
    };
    (minutes <= 30 * 24 * 60).then(|| serde_json::json!({ "kind": "after", "minutes": minutes }))
}

/// How a preset setting reads in the editor.
pub fn preset_text(p: &Value) -> String {
    match (p["kind"].as_str(), p["minutes"].as_u64()) {
        (Some("after"), Some(m)) if m % (24 * 60) == 0 => format!("{}d", m / (24 * 60)),
        (Some("after"), Some(m)) if m % 60 == 0 => format!("{}h", m / 60),
        (Some("after"), Some(m)) => format!("{m}m"),
        _ => format!(
            "{} {}",
            p["day"].as_str().unwrap_or(""),
            p["time"].as_str().unwrap_or("")
        ),
    }
}

/// A sidebar row's status, most urgent first (`resolveThreadStatusPill`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    PendingApproval,
    AwaitingInput,
    GuestRequest,
    Working,
    Connecting,
    Waiting,
    Failed,
    PlanReady,
    Completed,
}

impl Status {
    /// The web's label, for `i18n::t`.
    pub fn label(self) -> &'static str {
        match self {
            Status::PendingApproval => "Pending Approval",
            Status::AwaitingInput => "Awaiting Input",
            Status::GuestRequest => "Guest Request",
            Status::Working => "Working",
            Status::Connecting => "Connecting",
            Status::Waiting => "Waiting",
            Status::Failed => "Failed",
            Status::PlanReady => "Plan Ready",
            Status::Completed => "Completed",
        }
    }
}

/// shortcut: subagent requests and background tasks that hold a finished run are not read yet;
/// those threads show their own state instead. Add when the shell decodes them.
pub fn status(t: &ThreadShell) -> Option<Status> {
    let request = t.pending_runtime_request["kind"].as_str();
    match request {
        Some("user_input") => return Some(Status::AwaitingInput),
        Some("auth_refresh") | None => {}
        Some(_) => return Some(Status::PendingApproval),
    }
    if t.pending_guest_message_count > 0 {
        return Some(Status::GuestRequest);
    }
    let thread_status = t.status.as_str().unwrap_or("");
    let runtime = t
        .latest_run_id
        .as_ref()
        .map(|_| t.activity_run_status.as_deref().unwrap_or(thread_status));
    match runtime {
        Some("running") if t.blocked_on_thread_wait => return Some(Status::Waiting),
        Some("running" | "waiting") => return Some(Status::Working),
        Some("preparing" | "starting" | "queued") => return Some(Status::Connecting),
        _ => {}
    }
    let after_visit = |at: Option<&str>| match (ts_ms(at), ts_ms(t.last_visited_at.as_deref())) {
        (_, None) => true,
        (Some(at), Some(seen)) => at > seen,
        (None, Some(_)) => true,
    };
    if thread_status == "failed"
        && after_visit(
            t.latest_run_completed_at
                .as_deref()
                .or(t.latest_run_started_at.as_deref()),
        )
    {
        return Some(Status::Failed);
    }
    if t.interaction_mode == "plan" && t.has_actionable_proposed_plan && !t.is_running() {
        return Some(Status::PlanReady);
    }
    t.has_unseen_completion().then_some(Status::Completed)
}

/// `resolveCustomSnooze` from one field: a local `YYYY-MM-DD HH:MM`, or a duration such as `90m`,
/// `2h` or `3d`. `None` unless it lands in the future; a time skipped by a DST change is rejected.
pub fn custom_snooze(input: &str, now: &Zoned) -> Option<Timestamp> {
    let input = input.trim();
    let wake = if let Ok(local) = input.replacen(' ', "T", 1).parse::<jiff::civil::DateTime>() {
        let zoned = local.to_zoned(now.time_zone().clone()).ok()?;
        (zoned.datetime() == local).then_some(zoned)?
    } else {
        let split = input.find(|c: char| !c.is_ascii_digit())?;
        let amount: i64 = input[..split].parse().ok().filter(|n| *n > 0)?;
        let minutes = match input[split..].trim().chars().next()? {
            'm' => amount,
            'h' => amount.checked_mul(60)?,
            'd' => amount.checked_mul(24 * 60)?,
            _ => return None,
        };
        now.checked_add(minutes.minutes()).ok()?
    };
    (wake.timestamp() > now.timestamp()).then(|| wake.timestamp())
}

/// `resolveSnoozePresets`: the environment's presets (or defaults) resolved against `now`,
/// passed "today" times dropped, sorted by wake minute with duplicates removed.
pub fn snooze_presets(now: &Zoned, settings: Option<&Value>) -> Vec<SnoozePreset> {
    let presets = settings
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(default_presets);
    let mut out: Vec<SnoozePreset> = presets
        .iter()
        .filter_map(|p| resolve_preset(now, p))
        .collect();
    out.sort_by_key(|p| p.until.as_second() / 60);
    out.dedup_by_key(|p| p.until.as_second() / 60);
    out
}

fn resolve_preset(now: &Zoned, preset: &Value) -> Option<SnoozePreset> {
    if preset["kind"] == "after" {
        let minutes = preset["minutes"].as_i64()?;
        let label = if minutes % (24 * 60) == 0 {
            plural(minutes / (24 * 60), "day")
        } else if minutes % 60 == 0 {
            plural(minutes / 60, "hour")
        } else {
            plural(minutes, "minute")
        };
        return Some(SnoozePreset {
            label,
            until: now.timestamp().checked_add(minutes.minutes()).ok()?,
        });
    }
    let day = preset["day"].as_str()?;
    let (hour, minute) = preset["time"].as_str()?.split_once(':')?;
    let days_ahead: i64 = match day {
        "today" => 0,
        "tomorrow" => 1,
        other => {
            let target = weekday(other)?.to_sunday_zero_offset() as i64;
            let today = now.weekday().to_sunday_zero_offset() as i64;
            match (target - today).rem_euclid(7) {
                0 => 7,
                n => n,
            }
        }
    };
    let date = now.date().checked_add(days_ahead.days()).ok()?;
    let wake = date
        .at(hour.parse().ok()?, minute.parse().ok()?, 0, 0)
        .to_zoned(now.time_zone().clone())
        .ok()?;
    if wake.timestamp() <= now.timestamp() {
        return None;
    }
    let label = match (days_ahead, hour.parse::<i8>().ok()?) {
        (0, h) if h < 12 => "This morning".to_string(),
        (0, h) if h < 17 => "This afternoon".to_string(),
        (0, _) => "This evening".to_string(),
        (1, _) => "Tomorrow".to_string(),
        (_, _) if day == "monday" => "Next week".to_string(),
        (7, _) => format!("Next {}{}", day[..1].to_uppercase(), &day[1..]),
        _ => format!("{}{}", day[..1].to_uppercase(), &day[1..]),
    };
    let time = wake.strftime("%-I:%M %p").to_string();
    Some(SnoozePreset {
        label: format!("{label} · {time}"),
        until: wake.timestamp(),
    })
}

impl crate::T3App {
    /// The web's `CustomSnoozeDialog`, as one field taking a date and time or a duration.
    pub(crate) fn open_custom_snooze(
        &mut self,
        env: String,
        thread_id: String,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        use crate::i18n::t;
        use gpui_kit::component::input::{Input, InputState};
        use gpui_kit::component::{WindowExt, dialog::DialogButtonProps, v_flex};
        use gpui_kit::{AppContext, ParentElement, Styled};
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("2026-12-01 09:00  ·  90m  ·  2h  ·  3d")
        });
        let app = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (app, input, env, thread_id) = (app.clone(), input.clone(), env.clone(), thread_id.clone());
            dialog
                .title(t("Custom snooze"))
                .child(
                    v_flex()
                        .gap_2()
                        .child(t("Choose when snoozed threads return to your inbox."))
                        .child(Input::new(&input)),
                )
                .button_props(DialogButtonProps::default().ok_text(t("Snooze")).show_cancel(true))
                .on_ok(move |_, _, cx: &mut gpui_kit::App| {
                    let text = input.read(cx).value().to_string();
                    let Some(until) = custom_snooze(&text, &Zoned::now()) else {
                        return false;
                    };
                    let command = serde_json::json!({ "type": "thread.snooze", "threadId": thread_id,
                        "snoozedUntil": until.to_string() });
                    app.update(cx, |this, cx| this.dispatch_to(&env, command, cx)).ok();
                    true
                })
        });
    }
}

/// The sidebar's compact age (web: `compactSidebarTimeLabel`): "now", "5m", "3h", "2d".
pub fn compact_age(iso: &str, now_ms: i64) -> String {
    let Some(at) = ts_ms(Some(iso)) else {
        return String::new();
    };
    let seconds = (now_ms - at).max(0) / 1000;
    match seconds {
        s if s < 60 => crate::i18n::t("now"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Two letters for a project without an icon (web: `projectMonogram`).
pub fn monogram(name: &str) -> String {
    let words: Vec<Vec<char>> = name
        .trim()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().collect())
        .collect();
    let Some(first_word) = words.first() else {
        return "PR".into();
    };
    let first = first_word[0];
    let second = first_word[1..]
        .iter()
        .copied()
        .find(|c| c.is_numeric())
        .or_else(|| match words.len() {
            1 => first_word.last().copied(),
            _ => words.last().and_then(|w| w.first().copied()),
        })
        .unwrap_or(first);
    format!("{first}{second}")
        .to_uppercase()
        .chars()
        .take(2)
        .collect()
}

/// The project color a name hashes to, as an index into `PROJECT_ICON_COLORS` (web: `projectColor`).
pub fn project_color_index(name: &str) -> usize {
    let seed = name.trim().to_lowercase();
    let seed = if seed.is_empty() {
        "project".to_owned()
    } else {
        seed
    };
    seed.chars()
        .fold(0usize, |i, c| (i * 31 + c as usize) % PROJECT_COLORS.len())
}

/// `PROJECT_ICON_COLORS` in order, as (dark `-400`, light `-600`) Tailwind shades.
pub const PROJECT_COLORS: [(u32, u32); 18] = [
    (0x9ca3af, 0x4b5563), // gray
    (0xf87171, 0xdc2626), // red
    (0xfb923c, 0xea580c), // orange
    (0xfbbf24, 0xd97706), // amber
    (0xfacc15, 0xca8a04), // yellow
    (0xa3e635, 0x65a30d), // lime
    (0x4ade80, 0x16a34a), // green
    (0x34d399, 0x059669), // emerald
    (0x2dd4bf, 0x0d9488), // teal
    (0x22d3ee, 0x0891b2), // cyan
    (0x38bdf8, 0x0284c7), // sky
    (0x60a5fa, 0x2563eb), // blue
    (0x818cf8, 0x4f46e5), // indigo
    (0xa78bfa, 0x7c3aed), // violet
    (0xc084fc, 0x9333ea), // purple
    (0xe879f9, 0xc026d3), // fuchsia
    (0xf472b6, 0xdb2777), // pink
    (0xfb7185, 0xe11d48), // rose
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread(id: &str, extra: Value) -> ThreadShell {
        let mut v = json!({"id": id, "projectId": "p", "title": id, "status": "idle",
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn derives_project_identity_like_the_web() {
        use super::{compact_age, monogram, project_color_index};
        assert_eq!(monogram("server"), "SR");
        assert_eq!(monogram("t3code rs"), "T3");
        assert_eq!(monogram("my app"), "MA");
        assert_eq!(monogram("  "), "PR");
        // (('s'=115)*31 + ...) mod 18, as the web's loop computes it.
        let expected = "server"
            .chars()
            .fold(0usize, |i, c| (i * 31 + c as usize) % 18);
        assert_eq!(project_color_index("Server"), expected);
        let now = "2026-01-01T00:10:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        assert_eq!(compact_age("2026-01-01T00:09:30Z", now), "now");
        assert_eq!(compact_age("2026-01-01T00:05:00Z", now), "5m");
        assert_eq!(compact_age("2025-12-30T00:10:00Z", now), "2d");
    }

    #[test]
    fn partitions_like_the_web_sidebar() {
        let now = ts_ms(Some("2026-06-01T12:00:00Z")).unwrap();
        let threads = vec![
            thread("old", json!({})),
            thread("new", json!({"createdAt": "2026-05-01T00:00:00Z"})),
            thread("pin", json!({"pinnedAt": "2026-01-02T00:00:00Z"})),
            thread(
                "nap",
                json!({"snoozedUntil": "2026-06-02T00:00:00Z", "pinnedAt": "x"}),
            ),
            thread("woke", json!({"snoozedUntil": "2026-05-01T00:00:00Z"})),
            thread(
                "done",
                json!({"settledOverride": "settled", "settledAt": "2026-05-05T00:00:00Z"}),
            ),
            thread(
                "asks",
                json!({"snoozedUntil": "2026-06-02T00:00:00Z",
                "pendingRuntimeRequest": {"kind": "approval", "createdAt": "2026-06-01T11:00:00Z"}}),
            ),
        ];
        let s = sections(&threads, now);
        let ids = |v: &Vec<&ThreadShell>| v.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&s.pinned), ["pin"]);
        assert_eq!(ids(&s.snoozed), ["nap"]);
        assert_eq!(ids(&s.settled), ["done"]);
        // Newest anchor first, ties by id; a new approval wakes the snoozed "asks".
        assert_eq!(ids(&s.active), ["new", "asks", "old", "woke"]);
    }

    #[test]
    fn implied_active_keys_order_newest_first() {
        let a = thread("a", json!({"createdAt": "2026-01-01T00:00:00Z"}));
        let b = thread("b", json!({"createdAt": "2026-02-01T00:00:00Z"}));
        assert!(active_sort_key(&b) < active_sort_key(&a));
        assert!(active_sort_key(&a).starts_with('0') && active_sort_key(&a).ends_with('n'));
        let arranged = thread("c", json!({"activeOrderKey": "m"}));
        assert_eq!(active_sort_key(&arranged), "m");
    }

    #[test]
    fn resolves_row_status() {
        let row = |extra: serde_json::Value| {
            let mut t = json!({"id": "t", "projectId": "p", "title": "x", "updatedAt": "2026-01-01T00:00:00Z",
                "latestRunId": "r", "status": "idle", "lastVisitedAt": "2026-01-01T00:00:00Z"});
            t.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<ThreadShell>(t).unwrap()
        };
        assert_eq!(
            status(&row(
                json!({"pendingRuntimeRequest": {"kind": "command"}, "status": "running"})
            )),
            Some(Status::PendingApproval)
        );
        assert_eq!(
            status(&row(
                json!({"pendingRuntimeRequest": {"kind": "user_input"}})
            )),
            Some(Status::AwaitingInput)
        );
        assert_eq!(
            status(&row(json!({"status": "running"}))),
            Some(Status::Working)
        );
        assert_eq!(
            status(&row(
                json!({"status": "running", "blockedOnThreadWait": true})
            )),
            Some(Status::Waiting)
        );
        assert_eq!(
            status(&row(json!({"status": "starting"}))),
            Some(Status::Connecting)
        );
        assert_eq!(
            status(&row(
                json!({"status": "failed", "latestRunCompletedAt": "2026-01-02T00:00:00Z"})
            )),
            Some(Status::Failed)
        );
        assert_eq!(
            status(&row(
                json!({"status": "failed", "latestRunCompletedAt": "2025-12-31T00:00:00Z"})
            )),
            None,
            "seen"
        );
        assert_eq!(
            status(&row(
                json!({"interactionMode": "plan", "hasActionableProposedPlan": true})
            )),
            Some(Status::PlanReady)
        );
        assert_eq!(
            status(&row(
                json!({"latestRunCompletedAt": "2026-01-02T00:00:00Z"})
            )),
            Some(Status::Completed)
        );
        assert_eq!(status(&row(json!({"latestRunId": null}))), None);
    }

    #[test]
    fn leaves_subagent_threads_out_of_sections() {
        let thread = |id: &str, lineage: serde_json::Value| {
            serde_json::from_value::<ThreadShell>(json!({"id": id, "projectId": "p", "title": id,
                "updatedAt": "2026-01-01T00:00:00Z", "lineage": lineage}))
            .unwrap()
        };
        let threads = [
            thread(
                "parent",
                json!({"parentThreadId": null, "relationshipToParent": null, "rootThreadId": "parent"}),
            ),
            thread(
                "child",
                json!({"parentThreadId": "parent", "relationshipToParent": "subagent", "rootThreadId": "parent"}),
            ),
            thread(
                "fork",
                json!({"parentThreadId": "parent", "relationshipToParent": "fork", "rootThreadId": "parent"}),
            ),
        ];
        let s = sections(&threads, 0);
        let mut ids: Vec<_> = s.active.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["fork", "parent"]);
        assert_eq!(threads[1].parent_id(), Some("parent"));
        assert_eq!(threads[2].parent_id(), None, "a fork is listed on its own");
    }

    #[test]
    fn parses_snooze_presets() {
        assert_eq!(
            parse_preset("90m"),
            Some(json!({"kind": "after", "minutes": 90}))
        );
        assert_eq!(
            parse_preset("2h"),
            Some(json!({"kind": "after", "minutes": 120}))
        );
        assert_eq!(
            parse_preset("Tomorrow 09:00"),
            Some(json!({"kind": "at", "day": "tomorrow", "time": "09:00"}))
        );
        assert_eq!(parse_preset("friday 17:30").unwrap()["day"], "friday");
        assert_eq!(parse_preset("someday 09:00"), None);
        assert_eq!(parse_preset("today 9:00"), None, "the contract wants HH:MM");
        assert_eq!(parse_preset("31d"), None, "past the 30-day cap");
        assert_eq!(preset_text(&json!({"kind": "after", "minutes": 180})), "3h");
        assert_eq!(
            preset_text(&json!({"kind": "at", "day": "monday", "time": "09:00"})),
            "monday 09:00"
        );
    }

    #[test]
    fn resolves_custom_snooze() {
        let now: Zoned = "2026-03-08T01:00:00[America/New_York]".parse().unwrap();
        let at = |s: &str| custom_snooze(s, &now).map(|t| t.to_string());
        assert_eq!(at("90m").as_deref(), Some("2026-03-08T07:30:00Z"));
        assert_eq!(at("2 hours").as_deref(), Some("2026-03-08T08:00:00Z"));
        assert_eq!(
            at("2026-03-09 09:00").as_deref(),
            Some("2026-03-09T13:00:00Z")
        );
        assert_eq!(at("2026-03-08 02:30"), None, "skipped by the DST change");
        assert_eq!(at("2026-03-07 09:00"), None, "in the past");
        assert_eq!(at("0h"), None);
        assert_eq!(at("soon"), None);
    }

    #[test]
    fn resolves_default_snooze_presets() {
        // A Wednesday afternoon in UTC.
        let now: Zoned = "2026-06-03T15:00:00[UTC]".parse().unwrap();
        let labels: Vec<String> = snooze_presets(&now, None)
            .into_iter()
            .map(|p| p.label)
            .collect();
        // 15:00 + 3h and 18:00 wake in the same minute; the earlier setting wins.
        assert_eq!(
            labels,
            [
                "In 1 hour",
                "In 3 hours",
                "Tomorrow · 9:00 AM",
                "Next week · 9:00 AM"
            ]
        );
        let noon: Zoned = "2026-06-03T12:00:00[UTC]".parse().unwrap();
        assert!(
            snooze_presets(&noon, None)
                .iter()
                .any(|p| p.label == "This evening · 6:00 PM")
        );
        let late: Zoned = "2026-06-03T19:00:00[UTC]".parse().unwrap();
        assert!(
            !snooze_presets(&late, None)
                .iter()
                .any(|p| p.label.starts_with("This evening"))
        );
    }
}
