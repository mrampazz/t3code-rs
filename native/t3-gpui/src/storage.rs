//! Storage settings (web: `settings/StorageSettings`): the database size with what a reclaim would
//! free, and the last cleanup with a button to run one now. Both stream while Settings is open.
//! shortcut: the per-entry cleanup report and the cleanup policy settings are not shown.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::i18n::t;
use crate::{T3App, on_tokio};

/// Bytes as the web shows them: B, KB, MB, GB with one decimal past bytes.
pub fn format_bytes(bytes: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.);
    let mut unit = 0;
    while value >= 1024. && unit < units.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

#[derive(Default)]
pub struct StorageState {
    pub database: Value,
    pub cleanup: Value,
    subs: Vec<String>,
}

impl T3App {
    pub(crate) fn subscribe_storage(&mut self, cx: &mut Context<Self>) {
        self.release_storage();
        let Some(client) = self.client() else { return };
        for (method, is_db) in [
            ("server.getDatabaseStorage", true),
            ("server.getStorageCleanupReport", false),
        ] {
            let (sub, mut rx) = client.request(method, json!({}));
            self.storage.subs.push(sub.clone());
            cx.spawn(async move |this, cx| {
                while let Some(Ok(value)) = rx.recv().await {
                    let alive = this.update(cx, |app, cx| {
                        if !app.storage.subs.contains(&sub) {
                            return false;
                        }
                        if is_db {
                            app.storage.database = value
                        } else {
                            app.storage.cleanup = value
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

    pub(crate) fn release_storage(&mut self) {
        if let Some(client) = self.client() {
            for sub in self.storage.subs.drain(..) {
                client.cancel(&sub);
            }
        }
    }

    fn storage_call(&mut self, method: &'static str, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.settings_status = Some(t("Working…").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { client.call(method, json!({})).await }).await;
            this.update(cx, |app, cx| {
                app.settings_status = match result {
                    Ok(Ok(report)) if method == "server.runStorageCleanup" => {
                        app.storage.cleanup = report;
                        None
                    }
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e.to_string().into()),
                    Err(e) => Some(e.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_storage(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let db = &self.storage.database;
        let report = &self.storage.cleanup;
        let db_line = db["databaseBytes"].as_f64().map(|size| {
            let reclaim = db["reclaimableBytes"].as_f64().unwrap_or(0.);
            format!(
                "{} · {} {}",
                format_bytes(size),
                format_bytes(reclaim),
                t("reclaimable")
            )
        });
        let can_reclaim =
            db["canReclaim"] == true && db["reclaimableBytes"].as_f64().unwrap_or(0.) > 0.;
        let cleanup_line = report["finishedAt"].as_str().map(|at| {
            let when = at.parse::<jiff::Timestamp>().map_or(at.to_owned(), |ts| {
                ts.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%b %-d, %H:%M")
                    .to_string()
            });
            format!(
                "{when} · {} {} · {} {}",
                report["counts"]["removed"].as_u64().unwrap_or(0),
                t("removed"),
                format_bytes(report["bytesFreed"].as_f64().unwrap_or(0.)),
                t("freed")
            )
        });
        let line = |label: String, value: Option<String>| {
            v_flex().flex_1().min_w_0().child(label).child(
                div()
                    .text_xs()
                    .text_color(c.muted_foreground)
                    .child(value.unwrap_or_else(|| "—".into())),
            )
        };
        v_flex()
            .text_sm()
            .child(
                h_flex()
                    .py_2()
                    .gap_2()
                    .child(line(t("Database"), db_line))
                    .children(can_reclaim.then(|| {
                        Button::new("db-reclaim")
                            .small()
                            .outline()
                            .label(t("Reclaim space"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_call("server.reclaimDatabaseSpace", cx)
                            }))
                    })),
            )
            .child(
                h_flex()
                    .py_2()
                    .gap_2()
                    .child(line(t("Cleanup"), cleanup_line))
                    .child(
                        Button::new("cleanup-run")
                            .small()
                            .outline()
                            .label(t("Run cleanup now"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_call("server.runStorageCleanup", cx)
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(512.), "512 B");
        assert_eq!(format_bytes(1536.), "1.5 KB");
        assert_eq!(format_bytes(3. * 1024. * 1024. * 1024.), "3.0 GB");
    }
}
