//! Thread notifications (web: `ThreadNotificationCoordinator.tsx`): when a thread you are not
//! looking at newly needs approval or input, fails, gets a guest request or completes, show an
//! in-app toast that opens it. Only transitions count, so connecting never floods toasts.
//! shortcut: in-app toasts only; no OS notifications, sound or dock badge yet.

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
}

impl T3App {
    /// Compares an environment's threads with their last statuses and queues toasts.
    pub(crate) fn observe_statuses(&mut self, env: &str) {
        if self.prefs["inAppNotifications"] == false {
            return;
        }
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
            let open = self.selected.as_deref() == Some(thread.id.as_str());
            if let Some(title) = toast_title(prev, next).filter(|_| !open) {
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

    /// Shows queued toasts; runs in render, which has the window.
    pub(crate) fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for toast in self.notifier.queue.drain(..) {
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
}

#[cfg(test)]
mod tests {
    use super::toast_title;
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
