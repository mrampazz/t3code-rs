//! Mermaid diagrams in chat markdown (web: `MarkdownMermaidCodeBlock`, `MermaidDiagram`): a
//! ```` ```mermaid ```` fence renders as an SVG drawn by `mermaid-rs-renderer`, or as its source
//! when the diagram does not parse.
//! shortcut: no zoom, pan or "open in viewer" controls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

const NODE: &str = "mermaid";
/// Streaming re-renders every partial diagram; keep the cache from growing without bound.
const CACHE_LIMIT: usize = 64;

type Cache = Mutex<HashMap<(String, bool), Option<Arc<Image>>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The diagram as an SVG image, or `None` when it does not parse.
pub fn render_svg(source: &str, dark: bool) -> Option<String> {
    let theme = if dark {
        mermaid_rs_renderer::Theme::dark()
    } else {
        mermaid_rs_renderer::Theme::modern()
    };
    let options = mermaid_rs_renderer::RenderOptions {
        theme,
        ..Default::default()
    };
    mermaid_rs_renderer::render_with_options(source, options).ok()
}

fn diagram(source: &str, dark: bool) -> Option<Arc<Image>> {
    let key = (source.to_owned(), dark);
    if let Some(hit) = cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let image = render_svg(source, dark)
        .map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())));
    let mut cache = cache().lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, image.clone());
    image
}

/// Draws `mermaid` fences in `view` as diagrams.
pub fn with_mermaid(view: TextView) -> TextView {
    view.markdown_block_parser(|node, _| match node {
        markdown::mdast::Node::Code(code) if code.lang.as_deref() == Some(NODE) => Some(
            gpui_kit::component::text::MarkdownNode::new(NODE, code.value.clone())
                .text(code.value.clone())
                .markdown(format!("```mermaid\n{}\n```", code.value)),
        ),
        _ => None,
    })
    .markdown_block_renderer(NODE, |node, _, cx| {
        let source = node.data::<String>().cloned().unwrap_or_default();
        let theme = cx.theme();
        let block = v_flex()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.border);
        match diagram(&source, theme.mode.is_dark()) {
            Some(image) => block
                .items_center()
                .child(img(ImageSource::Image(image)).max_w_full())
                .into_any_element(),
            None => block
                .font_family(theme.mono_font_family.clone())
                .text_sm()
                .child(source)
                .into_any_element(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::render_svg;

    #[test]
    fn renders_diagrams_and_rejects_garbage() {
        let svg = render_svg("flowchart LR; A-->B", true).unwrap();
        assert!(svg.contains("<svg"));
        assert!(render_svg("not a diagram at all", false).is_none());
    }
}
