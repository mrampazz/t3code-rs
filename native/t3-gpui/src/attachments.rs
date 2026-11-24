//! Composer attachments (web: `ComposerAttachmentStrip.tsx`, `lib/attachmentUploadQueue.ts`):
//! `attachments.createUploadUrl`, then POST the bytes to the signed URL.

use crate::i18n::t;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use crate::{T3App, on_tokio};

/// `PROVIDER_SEND_TURN_MAX_IMAGE_BYTES` / `PROVIDER_SEND_TURN_MAX_FILE_BYTES`.
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Where an attachment's bytes come from.
enum Source {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Upload {
    Uploading,
    Ready(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub key: u64,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub image: bool,
    pub upload: Upload,
}

impl Attachment {
    /// The `ChatAttachment` a message carries once uploaded.
    pub fn wire(&self) -> Option<Value> {
        let Upload::Ready(id) = &self.upload else {
            return None;
        };
        Some(json!({
            "type": if self.image { "image" } else { "file" },
            "id": id,
            "name": self.name,
            "mimeType": self.mime,
            "sizeBytes": self.size,
        }))
    }
}

/// (mime type, is a provider-supported image) from the file extension.
pub fn classify(path: &Path) -> (String, bool) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let image = match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    };
    if let Some(mime) = image {
        return (mime.into(), true);
    }
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "json" => "application/json",
        "md" | "txt" | "rs" | "ts" | "tsx" | "js" | "py" | "go" | "toml" | "yaml" | "yml"
        | "csv" | "log" => "text/plain",
        _ => "application/octet-stream",
    };
    (mime.into(), false)
}

impl T3App {
    pub(crate) fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |app, cx| app.add_attachments(paths, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub(crate) fn add_attachments(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let (mime, image) = classify(&path);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            self.add_attachment(name, mime, image, size, Source::Path(path), cx);
        }
    }

    /// A pasted image (web: composer paste): supported formats upload, others are refused.
    pub(crate) fn add_pasted_image(&mut self, image: &gpui_kit::Image, cx: &mut Context<Self>) {
        let (mime, ext) = match image.format {
            ImageFormat::Png => ("image/png", "png"),
            ImageFormat::Jpeg => ("image/jpeg", "jpg"),
            ImageFormat::Gif => ("image/gif", "gif"),
            ImageFormat::Webp => ("image/webp", "webp"),
            _ => {
                self.error = Some("Paste a PNG, JPEG, GIF, or WebP image.".into());
                return cx.notify();
            }
        };
        let size = image.bytes.len() as u64;
        self.add_attachment(
            format!("pasted-image.{ext}"),
            mime.into(),
            true,
            size,
            Source::Bytes(image.bytes.clone()),
            cx,
        );
    }

    /// Validates the size, then uploads: `attachments.createUploadUrl` and POST to the signed URL.
    fn add_attachment(
        &mut self,
        name: String,
        mime: String,
        image: bool,
        size: u64,
        source: Source,
        cx: &mut Context<Self>,
    ) {
        self.attachment_seq += 1;
        let key = self.attachment_seq;
        let limit = if image {
            MAX_IMAGE_BYTES
        } else {
            MAX_FILE_BYTES
        };
        let valid = size > 0 && size <= limit;
        let upload = if valid {
            Upload::Uploading
        } else {
            Upload::Failed(format!("Must be 1 byte to {} MB", limit / 1024 / 1024))
        };
        self.attachments.push(Attachment {
            key,
            name: name.clone(),
            mime: mime.clone(),
            size,
            image,
            upload,
        });
        cx.notify();
        let (true, Some(client), Some(session)) =
            (valid, self.client(), self.env().map(|e| e.session.clone()))
        else {
            return;
        };
        let request = json!({ "type": if image { "image" } else { "file" }, "name": name, "mimeType": mime, "sizeBytes": size });
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let url = client.call("attachments.createUploadUrl", request).await?;
                let bytes = match source {
                    Source::Bytes(bytes) => bytes,
                    Source::Path(path) => {
                        tokio::task::spawn_blocking(move || std::fs::read(path)).await??
                    }
                };
                let relative = url["relativeUrl"].as_str().unwrap_or_default();
                let res = reqwest::Client::new()
                    .post(format!("{}{relative}", session.base_url))
                    .header("content-type", mime)
                    .body(bytes)
                    .send()
                    .await?;
                anyhow::ensure!(
                    res.status().is_success(),
                    "upload rejected ({})",
                    res.status()
                );
                Ok(url["attachmentId"].as_str().unwrap_or_default().to_string())
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            this.update(cx, |app, cx| {
                if let Some(a) = app.attachments.iter_mut().find(|a| a.key == key) {
                    a.upload = match result {
                        Ok(id) => Upload::Ready(id),
                        Err(e) => Upload::Failed(e.to_string()),
                    };
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The attachments to send, or an error while any is uploading or failed.
    pub(crate) fn take_attachments(&mut self) -> Result<Vec<Value>, &'static str> {
        if self
            .attachments
            .iter()
            .any(|a| a.upload == Upload::Uploading)
        {
            return Err("Wait for attachments to finish uploading.");
        }
        if self
            .attachments
            .iter()
            .any(|a| matches!(a.upload, Upload::Failed(_)))
        {
            return Err("Remove failed attachments before sending.");
        }
        Ok(std::mem::take(&mut self.attachments)
            .iter()
            .filter_map(Attachment::wire)
            .collect())
    }

    pub(crate) fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(self.attachments.iter().map(|a| {
                let key = a.key;
                let (state, color) = match &a.upload {
                    Upload::Uploading => (" · uploading".to_string(), c.muted_foreground),
                    Upload::Ready(_) => (String::new(), c.foreground),
                    Upload::Failed(e) => (format!(" · {e}"), c.danger),
                };
                h_flex()
                    .gap_1()
                    .px_2()
                    .rounded_md()
                    .bg(c.secondary)
                    .text_xs()
                    .text_color(color)
                    .child(format!(
                        "{} {}{state}",
                        if a.image { "🖼" } else { "📄" },
                        a.name
                    ))
                    .child(
                        Button::new(("remove-attachment", key as usize))
                            .xsmall()
                            .ghost()
                            .label("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.attachments.retain(|a| a.key != key);
                                cx.notify();
                            })),
                    )
            }))
            .child(
                Button::new("attach")
                    .xsmall()
                    .ghost()
                    .label(t("Attach…"))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_attachments(cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Attachment, Upload, classify};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn classifies_by_extension_and_builds_wire_shape() {
        assert_eq!(
            classify(Path::new("a/shot.PNG")),
            ("image/png".into(), true)
        );
        assert_eq!(
            classify(Path::new("notes.md")),
            ("text/plain".into(), false)
        );
        assert_eq!(
            classify(Path::new("bin")),
            ("application/octet-stream".into(), false)
        );
        let mut a = Attachment {
            key: 1,
            name: "shot.png".into(),
            mime: "image/png".into(),
            size: 5,
            image: true,
            upload: Upload::Uploading,
        };
        assert_eq!(a.wire(), None);
        a.upload = Upload::Ready("att_1".into());
        assert_eq!(
            a.wire().unwrap(),
            json!({"type": "image", "id": "att_1", "name": "shot.png", "mimeType": "image/png", "sizeBytes": 5})
        );
    }
}
