//! Single-line `text-overflow: ellipsis` for hx-native (issue #12).
//!
//! Blitz =0.3.0-beta.4 / stylo parses `text-overflow` but does not honor it
//! (clip only). This module applies truncation for paint without forking Blitz.
//!
//! # When it applies
//!
//! A block (or inline-root) that has all of:
//! - `text-overflow: ellipsis`
//! - `white-space: nowrap` (equivalently `text-wrap-mode: nowrap`)
//! - `overflow-x` not `visible`
//! - inline content wider than the content box
//!
//! Truncation cuts at a **parley cluster** boundary (grapheme / ZWJ-safe) so
//! `prefix + "…"` fits the content box. Fit is decided with a binary search
//! over shaped cluster advances — no per-character re-layout. The ellipsis
//! glyph width is measured by shaping `"…"` with the same style.
//!
//! # Paint without dirtying layout for hit-testing
//!
//! The DOM text nodes always keep the full original string. For paint we swap
//! the element's cached [`TextLayout`](blitz_dom::node::TextLayout) with a
//! truncated shaped layout built **directly** via parley (same bundled
//! [`FontContext`](blitz_dom::FontContext) as document construction). We never
//! mutate DOM text and never run a document `resolve` for truncation, so
//! content-sized boxes (inline-block, flex items with `min-width: 0`) and
//! siblings keep their full-geometry layout for hit-testing and hover.
//!
//! [`EllipsisPaintSession`] restores the original `inline_layout_data` in
//! [`Drop`], so early `?` returns and panics still undo the swap.
//!
//! Truncated layouts are cached under a key of
//! `(original text, content-box width rounded to 1/64 px, font/style fingerprint)`.
//! A cache miss reshapes once via parley (not counted as a document style/layout
//! pass). Clear the cache when the DOM mutates (`EllipsisCache::clear`).
//!
//! # Mixed inline styling
//!
//! Truncated paint uses a single style taken from the ellipsis element (first
//! text node's brush id). Nested inline styling such as `<b>` / `<em>` is
//! flattened to that style for the truncated string. Full mixed-run restyling
//! is out of scope until needed.
//!
//! # Pin sensitivity (Blitz / stylo)
//!
//! This module relies on Blitz internals that are public but not a stable
//! contract. A Blitz or stylo bump can break detection or paint **silently**
//! (no compile error). Relied-on surfaces:
//!
//! - `ElementData::inline_layout_data` — swapped for paint, restored on Drop
//! - `Node::final_layout()` — content-box width
//! - `Node::primary_styles()` + `format!("{:?}", styles.clone_text_overflow())`
//!   (and wrap / overflow-x / font fingerprint fields) — style detection
//! - `TextLayout::{text, layout, block_offset, span_cb_flags}` — paint payload
//! - `TextBrush { id }` — glyph brush for paint / hit attribution
//! - [`crate::stylo_text_style`] — copy of Blitz's `pub(crate)`
//!   `stylo_to_parley::style` (font-size/family/weight/style, letter-/word-spacing,
//!   line-height, features). Must stay aligned when bumping Blitz/stylo.
//! - Direct `style = { package = "stylo", version = "=0.22.0" }` in
//!   `hx-native/Cargo.toml`. Change this pin together with any Blitz bump that
//!   moves stylo: a stylo patch release stays held at `=0.22.0`, and on 0.23
//!   the types won't match so compilation fails loudly (the safe failure).
//! - [`crate::fonts::bundled_font_context`] — copies Blitz's
//!   `build_single_font_ctx` (system fonts off; register one face as every
//!   generic family). Shares one parley `Blob` (and `SourceCache`) so the
//!   glyph cache sees a single font id.
//! - [`crate::stylo_text_style::{text_align,text_align_last,base_direction}`] —
//!   copies of Blitz helpers applied outside `TextStyle` the way Blitz does
//!   (`construct.rs` `set_base_direction`, `inline.rs` `layout.align`).
//!
//! Detection and pixel-match tests in `tests/text_overflow.rs` run in CI's
//! normal test job (not `#[ignore]`, not feature-gated) so a bump that breaks
//! ellipsis fails CI.
//!
//! # Out of scope
//!
//! - RTL / bidirectional ellipsis sides
//! - Multi-line clamp (`-webkit-line-clamp` / `line-clamp`)

use std::collections::HashMap;

use blitz_dom::node::{TextBrush, TextLayout};
use blitz_dom::{FontContext, NodeId};
use blitz_html::HtmlDocument;
use parley::layout::{Alignment, AlignmentOptions, Layout};
use parley::style::TextStyle;
use parley::{BaseDirection, LayoutContext};

use crate::fonts::bundled_font_context;
use crate::stylo_text_style::{
    base_direction, text_align, text_align_last, text_style_from_computed,
};

/// U+2026 HORIZONTAL ELLIPSIS — CSS `text-overflow: ellipsis` marker.
const ELLIPSIS: &str = "\u{2026}";

/// Cache key for a truncated shaped layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TruncationKey {
    original: String,
    /// Content-box width in CSS px, quantized to 1/64 px (`to_bits` of the rounded value).
    content_w_bits: u32,
    style_fp: u64,
}

struct CacheEntry {
    key: TruncationKey,
    /// Truncated paint string (full original stays in the DOM).
    truncated: String,
    layout: Box<TextLayout>,
}

/// Per-surface cache of truncated paint layouts.
#[derive(Default)]
pub(crate) struct EllipsisCache {
    by_node: HashMap<NodeId, CacheEntry>,
}

impl EllipsisCache {
    pub(crate) fn clear(&mut self) {
        self.by_node.clear();
    }

    /// Test/harness: last truncated paint string for an element node, if cached.
    pub(crate) fn truncated_for(&self, nid: NodeId) -> Option<&str> {
        self.by_node.get(&nid).map(|e| e.truncated.as_str())
    }
}

/// Font + layout contexts used to shape truncated text without touching the DOM.
pub(crate) struct EllipsisShaper {
    font_ctx: FontContext,
    layout_ctx: LayoutContext<TextBrush>,
}

impl EllipsisShaper {
    pub(crate) fn new() -> Self {
        Self {
            font_ctx: bundled_font_context(),
            layout_ctx: LayoutContext::new(),
        }
    }
}

impl Default for EllipsisShaper {
    fn default() -> Self {
        Self::new()
    }
}

/// Holds original inline layouts while truncated ones are installed for paint.
/// Restores on [`Drop`] (covers `?` early returns and panics).
pub(crate) struct EllipsisPaintSession<'a> {
    doc: &'a mut HtmlDocument,
    backups: Vec<(NodeId, Box<TextLayout>)>,
    /// True when at least one truncated layout was installed for this paint.
    #[allow(dead_code)]
    pub(crate) applied: bool,
    /// True when this paint had to reshape (cache miss).
    #[allow(dead_code)]
    pub(crate) reshaped: bool,
}

impl<'a> EllipsisPaintSession<'a> {
    pub(crate) fn doc_mut(&mut self) -> &mut HtmlDocument {
        self.doc
    }
}

impl Drop for EllipsisPaintSession<'_> {
    fn drop(&mut self) {
        for (nid, layout) in self.backups.drain(..) {
            if let Some(el) = self
                .doc
                .get_node_mut(nid)
                .and_then(|n| n.element_data_mut())
            {
                el.inline_layout_data = Some(layout);
            }
        }
    }
}

/// Install truncated inline layouts for paint. Call after the document’s normal
/// style+layout resolve (DOM still holds the full text). Does not mutate DOM
/// text and does not call `HtmlDocument::resolve`.
///
/// `shaper` is created lazily on the first ellipsis candidate (shared font bytes).
pub(crate) fn prepare_ellipsis_for_paint<'a>(
    doc: &'a mut HtmlDocument,
    cache: &mut EllipsisCache,
    shaper: &mut Option<EllipsisShaper>,
) -> EllipsisPaintSession<'a> {
    let scale = doc.viewport().scale().max(0.000_1);
    let candidates = find_ellipsis_candidates(doc, scale);
    if candidates.is_empty() {
        return EllipsisPaintSession {
            doc,
            backups: Vec::new(),
            applied: false,
            reshaped: false,
        };
    }

    let shaper = shaper.get_or_insert_with(EllipsisShaper::new);
    let mut backups = Vec::new();
    let mut applied = false;
    let mut reshaped = false;

    for cand in candidates {
        let Some(prep) = prepare_one(doc, cache, shaper, &cand, scale) else {
            continue;
        };
        if prep.reshaped {
            reshaped = true;
        }
        backups.push((cand.element_id, prep.original_layout));
        if let Some(el) = doc
            .get_node_mut(cand.element_id)
            .and_then(|n| n.element_data_mut())
        {
            el.inline_layout_data = Some(prep.truncated_layout);
        }
        applied = true;
    }

    EllipsisPaintSession {
        doc,
        backups,
        applied,
        reshaped,
    }
}

struct Candidate {
    element_id: NodeId,
    content_w_css: f32,
    text_nodes: Vec<NodeId>,
}

struct Prepared {
    original_layout: Box<TextLayout>,
    truncated_layout: Box<TextLayout>,
    reshaped: bool,
}

fn find_ellipsis_candidates(doc: &HtmlDocument, scale: f32) -> Vec<Candidate> {
    let mut out = Vec::new();
    let root = doc.root_node().id;
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in &node.children {
            stack.push(c);
        }
        if !node.is_element() || !is_ellipsis_candidate(node) {
            continue;
        }
        let content_w_css = content_box_width_css(node);
        if content_w_css <= 0.0 {
            continue;
        }
        let Some(text_w_css) = inline_text_width_css(node, scale) else {
            continue;
        };
        if text_w_css <= content_w_css + 0.5 {
            continue;
        }
        let text_nodes = collect_text_nodes(doc, nid);
        if text_nodes.is_empty() {
            continue;
        }
        out.push(Candidate {
            element_id: nid,
            content_w_css,
            text_nodes,
        });
    }
    out
}

fn is_ellipsis_candidate(node: &blitz_dom::Node) -> bool {
    let Some(styles) = node.primary_styles() else {
        return false;
    };
    // Avoid `resolved_style_value` (needs ThreadState::LAYOUT).
    // Pin sensitivity: Debug string match — see module rustdoc.
    let text_overflow = format!("{:?}", styles.clone_text_overflow());
    if !text_overflow.contains("Ellipsis") {
        return false;
    }
    let wrap = format!("{:?}", styles.clone_text_wrap_mode());
    if !wrap.contains("Nowrap") {
        return false;
    }
    let ox = format!("{:?}", styles.clone_overflow_x());
    !ox.contains("Visible")
}

fn content_box_width_css(node: &blitz_dom::Node) -> f32 {
    let layout = node.final_layout();
    (layout.size.width
        - layout.padding.left
        - layout.padding.right
        - layout.border.left
        - layout.border.right)
        .max(0.0)
}

fn inline_text_width_css(node: &blitz_dom::Node, scale: f32) -> Option<f32> {
    let el = node.element_data()?;
    let inline = el.inline_layout_data.as_ref()?;
    Some(inline.layout.full_width() / scale)
}

fn collect_text_nodes(doc: &HtmlDocument, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in node.children.iter().rev() {
            stack.push(c);
        }
        if node.is_text_node() {
            out.push(nid);
        }
    }
    out.reverse();
    out
}

fn read_combined_text(doc: &HtmlDocument, text_nodes: &[NodeId]) -> String {
    let mut s = String::new();
    for &tid in text_nodes {
        if let Some(td) = doc.get_node(tid).and_then(|n| n.text_data()) {
            s.push_str(&td.content);
        }
    }
    s
}

fn style_fingerprint(node: &blitz_dom::Node) -> u64 {
    let Some(styles) = node.primary_styles() else {
        return 0;
    };
    // Stable-enough fingerprint of metrics that affect shaping width.
    // Pin sensitivity: Debug of stylo computed values — see module rustdoc.
    let raw = format!(
        "{:?}|{:?}|{:?}|{:?}",
        styles.clone_font_size(),
        styles.clone_font_weight(),
        styles.clone_font_family(),
        styles.clone_letter_spacing(),
    );
    let mut h: u64 = 0xcbf29ce484222325;
    for b in raw.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Quantize CSS px width to 1/64 px for stable cache keys across float noise.
fn width_key_bits(w_css: f32) -> u32 {
    let rounded = (w_css * 64.0).round() / 64.0;
    rounded.to_bits()
}

/// Cluster end byte offsets and cumulative advances (layout / scaled px).
fn cluster_cum_advances(layout: &Layout<TextBrush>) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    let mut cum = 0.0f32;
    out.push((0, 0.0));
    for line in layout.lines() {
        for run in line.runs() {
            for cluster in run.clusters() {
                cum += cluster.advance();
                let end = cluster.text_range().end;
                if out.last().is_some_and(|(e, _)| *e == end) {
                    // Ligature continuation with zero extra bytes — keep max advance.
                    if let Some(last) = out.last_mut() {
                        last.1 = cum;
                    }
                } else {
                    out.push((end, cum));
                }
            }
        }
    }
    out
}

/// Build a parley TextStyle from the element's computed styles (Blitz parity).
fn text_style_for_element(
    doc: &HtmlDocument,
    element_id: NodeId,
) -> Option<TextStyle<'static, 'static, TextBrush>> {
    let node = doc.get_node(element_id)?;
    let styles = node.primary_styles()?;
    // Prefer the brush id from the original layout's first glyph run when present;
    // fall back to the element id (Blitz root span brush).
    let brush_id = node
        .element_data()
        .and_then(|el| el.inline_layout_data.as_ref())
        .and_then(|tl| {
            for line in tl.layout.lines() {
                for item in line.items() {
                    if let parley::PositionedLayoutItem::GlyphRun(run) = item {
                        return Some(run.style().brush.id);
                    }
                }
            }
            None
        })
        .unwrap_or(element_id);
    Some(text_style_from_computed(brush_id, &styles))
}

/// Blitz applies base direction / text-align outside `TextStyle`
/// (`construct.rs:1061`, `inline.rs:827`).
struct AlignParams {
    base_direction: BaseDirection,
    alignment: Alignment,
    last_line_alignment: Option<Alignment>,
}

fn align_params_for_element(doc: &HtmlDocument, element_id: NodeId) -> Option<AlignParams> {
    let styles = doc.get_node(element_id)?.primary_styles()?;
    Some(AlignParams {
        base_direction: base_direction(styles.clone_direction(), styles.clone_unicode_bidi()),
        alignment: text_align(styles.clone_text_align()),
        last_line_alignment: text_align_last(styles.clone_text_align_last()),
    })
}

/// Shape `text` with the ellipsis element's computed style; returns a paint `TextLayout`.
fn shape_text_layout(
    shaper: &mut EllipsisShaper,
    text: &str,
    style: &TextStyle<'static, 'static, TextBrush>,
    scale: f32,
    max_advance: Option<f32>,
    template: &TextLayout,
    align: &AlignParams,
) -> Box<TextLayout> {
    let mut builder = shaper
        .layout_ctx
        .tree_builder(&mut shaper.font_ctx, scale, true, style);
    builder.set_base_direction(align.base_direction);
    builder.push_text(text);
    let mut layout = Layout::new();
    let shaped_text = builder.build_into(&mut layout);
    layout.break_all_lines(max_advance);
    layout.align(
        align.alignment,
        AlignmentOptions {
            align_when_overflowing: false,
            last_line_alignment: align.last_line_alignment,
        },
    );
    Box::new(TextLayout {
        text: shaped_text,
        content_widths: None,
        layout,
        block_offset: template.block_offset,
        span_cb_flags: template.span_cb_flags,
    })
}

fn measure_ellipsis_advance(
    shaper: &mut EllipsisShaper,
    style: &TextStyle<'static, 'static, TextBrush>,
    scale: f32,
    align: &AlignParams,
) -> f32 {
    let mut builder = shaper
        .layout_ctx
        .tree_builder(&mut shaper.font_ctx, scale, true, style);
    builder.set_base_direction(align.base_direction);
    builder.push_text(ELLIPSIS);
    let mut layout = Layout::new();
    let _ = builder.build_into(&mut layout);
    layout.break_all_lines(None);
    layout.full_width()
}

/// Binary search cluster boundaries so prefix advance + ellipsis fits `budget`
/// (same units as cluster advances — scaled px).
fn truncate_by_clusters(
    original: &str,
    layout: &Layout<TextBrush>,
    budget: f32,
    ellipsis_adv: f32,
) -> String {
    let clusters = cluster_cum_advances(layout);
    if clusters.len() <= 1 {
        return ELLIPSIS.to_string();
    }
    let full = clusters.last().map(|(_, a)| *a).unwrap_or(0.0);
    if full + 0.5 <= budget {
        return original.to_string();
    }
    let target = (budget - ellipsis_adv).max(0.0);

    let mut lo = 0usize;
    let mut hi = clusters.len() - 1;
    let mut best = 0usize;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        if clusters[mid].1 <= target + 0.5 {
            best = mid;
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    let end = clusters[best].0;
    let prefix = original.get(..end).unwrap_or("");
    format!("{prefix}{ELLIPSIS}")
}

fn take_inline_layout(doc: &mut HtmlDocument, nid: NodeId) -> Option<Box<TextLayout>> {
    doc.get_node_mut(nid)
        .and_then(|n| n.element_data_mut())
        .and_then(|el| el.inline_layout_data.take())
}

fn prepare_one(
    doc: &mut HtmlDocument,
    cache: &mut EllipsisCache,
    shaper: &mut EllipsisShaper,
    cand: &Candidate,
    scale: f32,
) -> Option<Prepared> {
    let original_text = read_combined_text(doc, &cand.text_nodes);
    if original_text.is_empty() {
        return None;
    }

    let node = doc.get_node(cand.element_id)?;
    let style_fp = style_fingerprint(node);
    let key = TruncationKey {
        original: original_text.clone(),
        content_w_bits: width_key_bits(cand.content_w_css),
        style_fp,
    };

    if let Some(entry) = cache.by_node.get(&cand.element_id) {
        if entry.key == key {
            let original_layout = take_inline_layout(doc, cand.element_id)?;
            return Some(Prepared {
                original_layout,
                truncated_layout: Box::new((*entry.layout).clone()),
                reshaped: false,
            });
        }
    }

    // Cache miss: cut from shaped cluster advances, reshape truncated string
    // directly (no DOM mutation, no document resolve).
    let original_layout = {
        let node = doc.get_node(cand.element_id)?;
        let el = node.element_data()?;
        let inline = el.inline_layout_data.as_ref()?;
        Box::new((**inline).clone())
    };

    let budget = cand.content_w_css * scale;
    let style = text_style_for_element(doc, cand.element_id)?;
    let align = align_params_for_element(doc, cand.element_id)?;
    let ellipsis_adv = measure_ellipsis_advance(shaper, &style, scale, &align);
    let truncated = truncate_by_clusters(
        &original_text,
        &original_layout.layout,
        budget,
        ellipsis_adv,
    );
    if truncated == original_text {
        return None;
    }

    let truncated_layout = shape_text_layout(
        shaper,
        &truncated,
        &style,
        scale,
        Some(budget),
        &original_layout,
        &align,
    );

    cache.by_node.insert(
        cand.element_id,
        CacheEntry {
            key,
            truncated,
            layout: Box::new((*truncated_layout).clone()),
        },
    );

    Some(Prepared {
        original_layout,
        truncated_layout,
        reshaped: true,
    })
}
