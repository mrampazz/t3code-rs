//! Thread notifications (web: `ThreadNotificationCoordinator.tsx`): when a thread you are not
//! looking at newly needs approval or input, fails, gets a guest request or completes, show an
//! in-app toast that opens it. Only transitions count, so connecting never floods toasts.
//! Like the web, `notificationMode` adds a sound per transition and, while the window is in the
//! background, an OS notification counted on the dock badge until the app is focused again.
//! shortcut: the dock badge and sounds are macOS only; Linux and Windows get toasts and OS
//! notifications.

use std::collections::HashMap;

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use crate::T3App;
use crate::i18n::t;
use crate::sidebar::{Status, status};

/// Opens a thread from a toast (toast callbacks cannot reach the app entity directly).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = t3, no_json)]
pub struct OpenThread {
    pub env: SharedString,
    pub id: SharedString,
}

/// The toast title for a status a thread just entered, if it is one worth a toast.
pub fn toast_title(prev: Option<Status>, next: Option<Status>) -> Option<&'static str> {
    if prev == next {
        return None;
    }
    Some(match next? {
        Status::PendingApproval => "Approval needed",
        Status::AwaitingInput => "Input needed",
        Status::Failed => "Thread failed",
        Status::GuestRequest => "Guest request",
        Status::Completed => "Thread completed",
        _ => return None,
    })
}

struct Thread {
    id: String,
    title: String,
}

pub struct Toast {
    title: &'static str,
    thread_title: String,
    env: String,
    id: String,
}

#[derive(Default)]
pub struct Notifier {
    /// Last seen status per (environment, thread); a thread's first sighting never toasts.
    seen: HashMap<(String, String), Option<Status>>,
    queue: Vec<Toast>,
    /// Tags of OS notifications shown since the window was last focused; the dock badge count.
    pending: std::collections::HashSet<String>,
}

/// `hasNotificationSound` / `hasDesktopNotifications` for the client's `notificationMode`.
pub fn mode_has(mode: &str, what: &str) -> bool {
    mode == what || mode == "notifications-and-sound"
}

/// An OS notification's tag, which carries its thread back when clicked.
fn tag(env: &str, id: &str) -> String {
    format!("{env}\n{id}")
}

fn untag(tag: &str) -> Option<(String, String)> {
    tag.split_once('\n')
        .map(|(e, i)| (e.to_owned(), i.to_owned()))
}

#[cfg(target_os = "macos")]
fn set_badge(count: usize) {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(mtm)
        .dockTile()
        .setBadgeLabel(label.as_deref());
}

#[cfg(target_os = "macos")]
fn play_sound(completion: bool) {
    use objc2::AllocAnyThread;
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSData;
    let bytes: &[u8] = if completion {
        include_bytes!("../../../apps/web/src/assets/notification-completion.mp3")
    } else {
        include_bytes!("../../../apps/web/src/assets/notification-input.mp3")
    };
    if let Some(sound) = NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(bytes)) {
        sound.play();
    }
}

#[cfg(not(target_os = "macos"))]
fn set_badge(_: usize) {}

#[cfg(not(target_os = "macos"))]
fn play_sound(_: bool) {}

impl T3App {
    /// Compares an environment's threads with their last statuses and queues toasts.
    pub(crate) fn observe_statuses(&mut self, env: &str) {
        let Some(shell) = self.env_by(env).map(|e| &e.shell) else {
            return;
        };
        let observed: Vec<(String, String, Option<Status>)> = shell
            .threads
            .iter()
            .filter(|t| t.is_listed())
            .map(|t| (t.id.clone(), t.title.clone(), status(t)))
            .collect();
        let mut toasts = vec![];
        for (id, title, next) in observed {
            let thread = Thread { id, title };
            let key = (env.to_owned(), thread.id.clone());
            let Some(prev) = self.notifier.seen.insert(key, next) else {
                continue;
            };
            if let Some(title) = toast_title(prev, next) {
                toasts.push(Toast {
                    title,
                    thread_title: thread.title.clone(),
                    env: env.to_owned(),
                    id: thread.id.clone(),
                });
            }
        }
        self.notifier.queue.extend(toasts);
    }

    /// Presents queued transitions; runs in render, which has the window. A focused window gets
    /// a toast unless the thread is open; a background one gets an OS notification.
    pub(crate) fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.prefs["notificationMode"]
            .as_str()
            .unwrap_or("off")
            .to_owned();
        let in_app = self.prefs["inAppNotifications"] != false;
        let queue = std::mem::take(&mut self.notifier.queue);
        for toast in queue {
            if mode_has(&mode, "sound") {
                play_sound(toast.title == "Thread completed");
            }
            if !window.is_window_active() {
                if mode_has(&mode, "notifications") {
                    let tag = tag(&toast.env, &toast.id);
                    cx.show_system_notification(SystemNotification {
                        tag: tag.clone().into(),
                        title: t(toast.title).into(),
                        body: toast.thread_title.into(),
                        actions: vec![],
                    });
                    self.notifier.pending.insert(tag);
                    set_badge(self.notifier.pending.len());
                }
                continue;
            }
            if !in_app || self.selected.as_deref() == Some(toast.id.as_str()) {
                continue;
            }
            let action = OpenThread {
                env: toast.env.into(),
                id: toast.id.into(),
            };
            let note = Notification::new()
                .title(t(toast.title))
                .message(toast.thread_title)
                .on_click(move |_, window, cx| {
                    window.dispatch_action(Box::new(action.clone()), cx)
                });
            window.push_notification(note, cx);
        }
    }

    /// Back in the app: the background notifications have done their job.
    pub(crate) fn clear_os_notifications(&mut self, cx: &mut Context<Self>) {
        for tag in self.notifier.pending.drain() {
            cx.dismiss_system_notification(&tag).detach();
        }
        set_badge(0);
    }

    /// Clicking an OS notification focuses the app on its thread.
    pub(crate) fn listen_for_notification_clicks(&self, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        cx.on_system_notification_response(move |response, cx| {
            let Some((env, id)) = untag(&response.tag) else {
                return;
            };
            cx.activate(true);
            app.update(cx, |this, cx| this.select_thread(env, id, cx))
                .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{mode_has, tag, toast_title, untag};

    #[test]
    fn modes_and_tags() {
        assert!(mode_has("notifications-and-sound", "sound") && mode_has("sound", "sound"));
        assert!(!mode_has("notifications", "sound") && !mode_has("off", "notifications"));
        assert_eq!(
            untag(&tag("http://h:1", "t1")),
            Some(("http://h:1".into(), "t1".into()))
        );
    }

    use crate::sidebar::Status;

    #[test]
    fn toasts_only_on_transitions_worth_attention() {
        assert_eq!(
            toast_title(Some(Status::Working), Some(Status::Completed)),
            Some("Thread completed")
        );
        assert_eq!(
            toast_title(Some(Status::Working), Some(Status::PendingApproval)),
            Some("Approval needed")
        );
        assert_eq!(
            toast_title(Some(Status::Completed), Some(Status::Completed)),
            None
        );
        assert_eq!(toast_title(None, Some(Status::Working)), None);
        assert_eq!(toast_title(Some(Status::Working), None), None);
    }
}
