//! Markdown images (web: `ChatMarkdownMedia`, `classifyMarkdownImageSource`). Web and `data:`
//! images load directly; a path on the environment's host becomes a signed asset URL first
//! (`assets.createUrl`), since the client never reads host files itself.
//! shortcut: minted URLs are not refreshed when they expire; gpui keeps an image once loaded.
//! shortcut: no expand/gallery, size hints, or file-link fallback for non-image host paths.

use std::borrow::Cow;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use gpui_kit::http_client::{self, AsyncBody, HttpClient, Response, Url, http::HeaderValue};
use gpui_kit::*;
use serde_json::json;

use crate::editors::{LinkTarget, link_target};
use crate::{RUNTIME, T3App, on_tokio};

/// gpui's image loader fetches remote images through the app's HTTP client.
pub struct ImageHttp(reqwest::Client);

impl ImageHttp {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(reqwest::Client::new()))
    }
}

impl HttpClient for ImageHttp {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: http_client::Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let request = self.0.request(req.method().clone(), req.uri().to_string());
        // Tests run inside their own runtime instead of the app's.
        let runtime = RUNTIME
            .get()
            .map_or_else(tokio::runtime::Handle::current, |r| r.handle().clone());
        let task = runtime.spawn(async move {
            let response = request.send().await?;
            let status = response.status();
            let bytes = response.bytes().await?;
            anyhow::Ok(
                Response::builder()
                    .status(status.as_u16())
                    .body(AsyncBody::from(bytes.to_vec()))?,
            )
        });
        async move { task.await? }.boxed()
    }
}

/// The host file an image destination names (absolute, or relative to `cwd`), or `None` when it
/// loads directly or is not loadable at all.
pub fn host_image_path(dest: &str, cwd: Option<&str>) -> Option<(String, bool)> {
    let lower = dest.trim().to_ascii_lowercase();
    if ["http:", "https:", "data:", "//"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return None;
    }
    let relative = !lower.starts_with("file:") && !lower.starts_with('/');
    match link_target(dest, cwd) {
        LinkTarget::File { path, .. } if path.starts_with('/') || path.contains(":\\") => {
            Some((path, relative))
        }
        _ => None,
    }
}

/// `text` with each image's destination replaced by `resolve(dest)` where that gives one.
pub fn rewrite_images<'a>(
    text: &'a str,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Cow<'a, str> {
    if !text.contains("![") {
        return Cow::Borrowed(text);
    }
    let Ok(tree) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) else {
        return Cow::Borrowed(text);
    };
    let mut images = Vec::new();
    let mut stack = vec![&tree];
    while let Some(node) = stack.pop() {
        if let markdown::mdast::Node::Image(image) = node
            && let Some(position) = &image.position
            && let Some(url) = resolve(&image.url)
        {
            let escaped: String = image
                .alt
                .chars()
                .flat_map(|c| match c {
                    '[' | ']' | '\\' => vec!['\\', c],
                    c => vec![c],
                })
                .collect();
            images.push((
                position.start.offset..position.end.offset,
                format!("![{escaped}](<{url}>)"),
            ));
        }
        stack.extend(node.children().into_iter().flatten());
    }
    if images.is_empty() {
        return Cow::Borrowed(text);
    }
    images.sort_by_key(|(range, _)| range.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, image) in images {
        out.push_str(&text[at..range.start]);
        out.push_str(&image);
        at = range.end;
    }
    out.push_str(&text[at..]);
    Cow::Owned(out)
}

impl T3App {
    /// Markdown with host-file images pointed at signed asset URLs, minting any not yet known.
    pub(crate) fn resolve_images<'a>(&self, text: &'a str, cx: &mut Context<Self>) -> Cow<'a, str> {
        let (Some(env), Some(thread_id)) = (self.env(), self.selected.clone()) else {
            return Cow::Borrowed(text);
        };
        let cwd = self.thread_cwd();
        let base = env.session.base_url.trim_end_matches('/').to_owned();
        let client = env.client.clone();
        rewrite_images(text, |dest| {
            let (path, relative) = host_image_path(dest, cwd.as_deref())?;
            let key = format!("{}\u{0}{thread_id}\u{0}{path}", env.key);
            if let Some(known) = self.asset_urls.borrow().get(&key) {
                return known.clone();
            }
            self.asset_urls.borrow_mut().insert(key.clone(), None);
            let client = client.clone()?;
            let mut resource = json!({ "_tag": "media-file", "threadId": thread_id, "path": path });
            if relative {
                resource["hostOnly"] = json!(true);
            }
            let base = base.clone();
            cx.spawn(async move |this, cx| {
                let minted = on_tokio(async move {
                    client
                        .call("assets.createUrl", json!({ "resource": resource }))
                        .await
                })
                .await;
                let url = match minted {
                    Ok(Ok(result)) => result["relativeUrl"]
                        .as_str()
                        .map(|relative| format!("{base}{relative}")),
                    _ => None,
                };
                this.update(cx, |app, cx| {
                    if url.is_some() {
                        app.asset_urls.borrow_mut().insert(key, url);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{host_image_path, rewrite_images};

    #[test]
    fn classifies_image_sources() {
        let cwd = Some("/repo");
        assert_eq!(host_image_path("https://x.dev/a.png", cwd), None);
        assert_eq!(host_image_path("data:image/png;base64,AA", cwd), None);
        assert_eq!(
            host_image_path("shots/a.png", cwd),
            Some(("/repo/shots/a.png".into(), true))
        );
        assert_eq!(
            host_image_path("/tmp/a.png", cwd),
            Some(("/tmp/a.png".into(), false))
        );
        assert_eq!(
            host_image_path("shots/a.png", None),
            None,
            "relative needs a root"
        );
        assert_eq!(host_image_path("mailto:x", cwd), None);
    }

    #[test]
    fn rewrites_only_resolved_images() {
        let text = "see ![a [b]](x.png) and ![c](https://y/c.png)\n\n`![d](x.png)`";
        let out = rewrite_images(text, |dest| (dest == "x.png").then(|| "http://h/1".into()));
        assert_eq!(
            out,
            "see ![a \\[b\\]](<http://h/1>) and ![c](https://y/c.png)\n\n`![d](x.png)`"
        );
        assert!(matches!(
            rewrite_images("plain", |_| None),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
