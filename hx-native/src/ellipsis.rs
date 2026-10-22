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
//! `prefix + "…"` fits the content box. Fit is decided from Blitz's own shaped
//! cluster advances — no per-character re-layout. Those advances already carry
//! each inline run's style, so a `<b>` run is measured in bold.
//!
//! # Per-run styles (issue #110)
//!
//! The truncated prefix is reshaped with one parley style span per inline run,
//! exactly as Blitz built the original: each cluster's `TextBrush` id names the
//! DOM node (the ellipsis element or a child like `<b>`, `<i>`, `<span>`, or a
//! `::before`/`::after` pseudo) whose computed style shaped it, and that node's
//! [`TextStyle`] is rebuilt with [`crate::stylo_text_style`]. Weight, italic,
//! size, family and spacing therefore survive truncation, and colour /
//! decorations follow because paint resolves them from the same brush node.
//!
//! The ellipsis glyph takes the style of the **last visible cluster's run**
//! (Chrome's behaviour), and its width is measured in that style, so the cut
//! depends on which run it lands in.
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
//! `(Blitz layout text, content-box width @ 1/64 px, scale bits,
//! fingerprint of the root TextStyle + AlignParams + every run's range and
//! TextStyle)`. The layout text is Blitz's
//! whitespace-collapsed, `text-transform`ed string in document order — not the
//! raw DOM concatenation. A cache miss reshapes once via parley (not counted as
//! a document style/layout pass). Clear the cache when the DOM mutates
//! (`EllipsisCache::clear`).
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
//! - `TextLayout::{text, layout, block_offset, span_cb_flags}` — paint payload;
//!   `text` is the source string for the cut (collapsed / transformed)
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
//! - Parley `TextStyle` / layout `Alignment` / `BaseDirection` `Debug` output
//!   format — used by `shaping_fingerprint`; a parley bump that changes Debug
//!   can invalidate cache keys silently (tests catch paint mismatches).
//!
//! Detection and pixel-match tests in `tests/text_overflow.rs` run in CI's
//! normal test job (not `#[ignore]`, not feature-gated) so a bump that breaks
//! ellipsis fails CI.
//!
//! # Out of scope
//!
//! - RTL / bidirectional ellipsis sides
//! - Multi-line clamp (`-webkit-line-clamp` / `line-clamp`)
//! - Atomic inline boxes (inline-block, `<img>`, …) inside the ellipsis element
//!   are not re-inserted into the truncated paint layout (unchanged from #12).
//! - A `list-style-position: inside` marker is reshaped in the element's own
//!   font rather than Blitz's bullet font (it shares the root brush id).
//! - `text-overflow` string values (`text-overflow: "…"`) and two-value syntax.

use std::collections::HashMap;
use std::ops::Range;

use blitz_dom::node::{TextBrush, TextLayout};
use blitz_dom::{FontContext, NodeId};
use blitz_html::HtmlDocument;
use parley::layout::{Alignment, AlignmentOptions, Layout};
use parley::style::TextStyle;
use parley::{BaseDirection, LayoutContext};

use crate::fonts::{bundled_font_context, HostFonts};
use crate::stylo_text_style::{
    base_direction, text_align, text_align_last, text_style_from_computed,
};

/// U+2026 HORIZONTAL ELLIPSIS — CSS `text-overflow: ellipsis` marker.
const ELLIPSIS: &str = "\u{2026}";

/// Cache key for a truncated shaped layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TruncationKey {
    /// Blitz `TextLayout.text` (collapsed / transformed, document order).
    original: String,
    /// Content-box width in CSS px, quantized to 1/64 px (`to_bits` of the rounded value).
    content_w_bits: u32,
    /// Exact bits of the viewport scale used for shaping (`f32::to_bits`).
    scale_bits: u32,
    /// Fingerprint of the full parley [`TextStyle`] plus [`AlignParams`].
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
    pub(crate) fn new(fonts: &HostFonts) -> Self {
        Self {
            font_ctx: bundled_font_context(fonts),
            layout_ctx: LayoutContext::new(),
        }
    }
}

/// Bundled DejaVu only (no host fonts).
impl Default for EllipsisShaper {
    fn default() -> Self {
        Self::new(&HostFonts::default())
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

    /// Install a truncated layout and record the original for Drop restore.
    /// Called as each candidate is prepared so a later panic still restores.
    fn apply_prepared(&mut self, nid: NodeId, prep: Prepared) {
        if prep.reshaped {
            self.reshaped = true;
        }
        self.backups.push((nid, prep.original_layout));
        if let Some(el) = self
            .doc
            .get_node_mut(nid)
            .and_then(|n| n.element_data_mut())
        {
            el.inline_layout_data = Some(prep.truncated_layout);
        }
        self.applied = true;
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
    fonts: &HostFonts,
) -> EllipsisPaintSession<'a> {
    let scale = doc.viewport().scale().max(0.000_1);
    let candidates = find_ellipsis_candidates(doc, scale);
    let mut session = EllipsisPaintSession {
        doc,
        backups: Vec::new(),
        applied: false,
        reshaped: false,
    };
    if candidates.is_empty() {
        return session;
    }

    let shaper = shaper.get_or_insert_with(|| EllipsisShaper::new(fonts));
    for cand in candidates {
        let Some(prep) = prepare_one(session.doc, cache, shaper, &cand, scale) else {
            continue;
        };
        session.apply_prepared(cand.element_id, prep);
    }
    session
}

struct Candidate {
    element_id: NodeId,
    content_w_css: f32,
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
        let has_text = node
            .element_data()
            .and_then(|el| el.inline_layout_data.as_ref())
            .is_some_and(|tl| !tl.text.is_empty());
        if !has_text {
            continue;
        }
        out.push(Candidate {
            element_id: nid,
            content_w_css,
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

/// Fingerprint everything used for the cut and the reshape: the element's root
/// parley [`TextStyle`], [`AlignParams`], and every inline run's byte range and
/// [`TextStyle`] (so a style change on a child `<b>` / `<span>` misses the cache).
/// Scale is keyed separately.
/// Pin sensitivity: depends on parley `Debug` formatting — see module rustdoc.
fn shaping_fingerprint(runs: &RunStyles, align: &AlignParams) -> u64 {
    let mut raw = format!("{:?}|{align:?}", runs.root);
    for (range, id) in &runs.segments {
        raw.push_str(&format!(
            "|{}..{}:{:?}",
            range.start,
            range.end,
            runs.style_of(*id)
        ));
    }
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

/// Cluster end byte offsets, cumulative advances (layout / scaled px) and the
/// brush node of the cluster that ends there. Advances come from Blitz's own
/// per-run shaping, so a bold run is measured in bold. Entry 0 is "nothing
/// visible" and carries the first cluster's brush (or `fallback`).
fn cluster_cum_advances(layout: &Layout<TextBrush>, fallback: NodeId) -> Vec<(usize, f32, NodeId)> {
    let mut out = Vec::new();
    let mut cum = 0.0f32;
    out.push((0, 0.0, fallback));
    for line in layout.lines() {
        for run in line.runs() {
            for cluster in run.clusters() {
                cum += cluster.advance();
                let end = cluster.text_range().end;
                let brush = cluster.style().brush.id;
                if out.len() > 1 && out.last().is_some_and(|(e, _, _)| *e == end) {
                    // Ligature continuation with zero extra bytes — keep max advance.
                    if let Some(last) = out.last_mut() {
                        last.1 = cum;
                    }
                } else {
                    out.push((end, cum, brush));
                }
            }
        }
    }
    if let Some(first) = out.get(1).map(|c| c.2) {
        out[0].2 = first;
    }
    out
}

/// Per-run text styles recovered from Blitz's shaped layout.
///
/// Blitz pushes one parley style span per inline element, with
/// `TextBrush { id: element node id }` and that element's full computed style
/// (`construct.rs` `build_inline_layout_recursive`). Each cluster's
/// `style().brush.id` therefore names the DOM node whose computed style shaped
/// it; we rebuild the same [`TextStyle`] from that node with
/// [`text_style_from_computed`].
struct RunStyles {
    /// The ellipsis element (Blitz's inline-context root span).
    root_id: NodeId,
    root: TextStyle<'static, 'static, TextBrush>,
    /// Style for every non-root brush node seen in the layout.
    by_brush: HashMap<NodeId, TextStyle<'static, 'static, TextBrush>>,
    /// Byte ranges of `TextLayout.text`, in logical order, covering the whole
    /// text, each with the brush node that styled it (adjacent equal brushes merged).
    segments: Vec<(Range<usize>, NodeId)>,
}

impl RunStyles {
    fn style_of(&self, id: NodeId) -> &TextStyle<'static, 'static, TextBrush> {
        if id == self.root_id {
            return &self.root;
        }
        self.by_brush.get(&id).unwrap_or(&self.root)
    }

    /// Push `text` in the style of brush `id` (a span unless it is the root).
    fn push(&self, builder: &mut parley::TreeBuilder<'_, TextBrush>, id: NodeId, text: &str) {
        if text.is_empty() {
            return;
        }
        if id == self.root_id {
            builder.push_text(text);
        } else {
            builder.push_style_span(self.style_of(id).clone());
            builder.push_text(text);
            builder.pop_style_span();
        }
    }
}

fn run_styles(doc: &HtmlDocument, element_id: NodeId, tl: &TextLayout) -> Option<RunStyles> {
    let root_styles = doc.get_node(element_id)?.primary_styles()?;
    let root = text_style_from_computed(element_id, &root_styles);

    let mut ranges: Vec<(Range<usize>, NodeId)> = Vec::new();
    for line in tl.layout.lines() {
        for run in line.runs() {
            for cluster in run.clusters() {
                let r = cluster.text_range();
                if !r.is_empty() {
                    ranges.push((r, cluster.style().brush.id));
                }
            }
        }
    }
    ranges.sort_by_key(|(r, _)| r.start);

    let text_len = tl.text.len();
    let mut segments: Vec<(Range<usize>, NodeId)> = Vec::new();
    let mut push_seg = |r: Range<usize>, id: NodeId| {
        if r.is_empty() {
            return;
        }
        if let Some(last) = segments.last_mut() {
            if last.1 == id && last.0.end == r.start {
                last.0.end = r.end;
                return;
            }
        }
        segments.push((r, id));
    };
    let mut pos = 0usize;
    for (r, id) in ranges {
        let start = r.start.max(pos);
        let end = r.end.min(text_len);
        if end <= start {
            continue;
        }
        // Bytes no cluster claims (none expected) keep the root style.
        push_seg(pos..start, element_id);
        push_seg(start..end, id);
        pos = end;
    }
    push_seg(pos..text_len, element_id);

    let mut by_brush = HashMap::new();
    for (_, id) in &segments {
        if *id == element_id || by_brush.contains_key(id) {
            continue;
        }
        let style = doc
            .get_node(*id)
            .and_then(|n| n.primary_styles())
            .map(|s| text_style_from_computed(*id, &s))
            .unwrap_or_else(|| root.clone());
        by_brush.insert(*id, style);
    }

    Some(RunStyles {
        root_id: element_id,
        root,
        by_brush,
        segments,
    })
}

/// Blitz applies base direction / text-align outside `TextStyle`
/// (`construct.rs:1061`, `inline.rs:827`).
#[derive(Debug)]
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

/// Shape `text[..cut.end]` + `"…"` with each run's own style (one parley span
/// per inline run, like Blitz) and the ellipsis in the last visible run's
/// style; returns a paint `TextLayout`.
#[allow(clippy::too_many_arguments)]
fn shape_text_layout(
    shaper: &mut EllipsisShaper,
    text: &str,
    cut: &Cut,
    runs: &RunStyles,
    scale: f32,
    max_advance: Option<f32>,
    template: &TextLayout,
    align: &AlignParams,
) -> Box<TextLayout> {
    let mut builder = shaper
        .layout_ctx
        .tree_builder(&mut shaper.font_ctx, scale, true, &runs.root);
    builder.set_base_direction(align.base_direction);
    for (range, id) in &runs.segments {
        if range.start >= cut.end {
            break;
        }
        let slice = text.get(range.start..range.end.min(cut.end)).unwrap_or("");
        runs.push(&mut builder, *id, slice);
    }
    runs.push(&mut builder, cut.ellipsis_brush, ELLIPSIS);
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

/// Where to cut: keep `text[..end]`, then draw `"…"` in `ellipsis_brush`'s style.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cut {
    end: usize,
    ellipsis_brush: NodeId,
}

/// Pick the longest cluster prefix whose advance plus the ellipsis advance fits
/// `budget` (scaled px). The ellipsis advance depends on the cut, because the
/// ellipsis takes the style of the last visible cluster; `ellipsis_adv` gives
/// it per brush. Returns `None` when the whole text fits.
fn truncate_by_clusters(
    layout: &Layout<TextBrush>,
    budget: f32,
    fallback_brush: NodeId,
    mut ellipsis_adv: impl FnMut(NodeId) -> f32,
) -> Option<Cut> {
    let clusters = cluster_cum_advances(layout, fallback_brush);
    if clusters.len() <= 1 {
        return Some(Cut {
            end: 0,
            ellipsis_brush: fallback_brush,
        });
    }
    let full = clusters.last().map(|c| c.1).unwrap_or(0.0);
    if full + 0.5 <= budget {
        return None;
    }
    // Cumulative advances are non-decreasing, so with a single style this is the
    // same "largest prefix that fits" answer the earlier binary search gave.
    let mut best = 0usize;
    for (i, &(_, cum, brush)) in clusters.iter().enumerate().rev() {
        let target = (budget - ellipsis_adv(brush)).max(0.0);
        if cum <= target + 0.5 {
            best = i;
            break;
        }
    }
    Some(Cut {
        end: clusters[best].0,
        ellipsis_brush: clusters[best].2,
    })
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
    // Cut against Blitz's shaped text (document order, whitespace-collapsed,
    // text-transform applied) — not a raw DOM concatenation.
    let original_layout = {
        let node = doc.get_node(cand.element_id)?;
        let el = node.element_data()?;
        let inline = el.inline_layout_data.as_ref()?;
        Box::new((**inline).clone())
    };
    let original_text = original_layout.text.clone();
    if original_text.is_empty() {
        return None;
    }

    let runs = run_styles(doc, cand.element_id, &original_layout)?;
    let align = align_params_for_element(doc, cand.element_id)?;
    let key = TruncationKey {
        original: original_text.clone(),
        content_w_bits: width_key_bits(cand.content_w_css),
        scale_bits: scale.to_bits(),
        style_fp: shaping_fingerprint(&runs, &align),
    };

    if let Some(entry) = cache.by_node.get(&cand.element_id) {
        if entry.key == key {
            let truncated_layout = Box::new((*entry.layout).clone());
            let original_layout = take_inline_layout(doc, cand.element_id)?;
            return Some(Prepared {
                original_layout,
                truncated_layout,
                reshaped: false,
            });
        }
    }

    // Cache miss: cut from Blitz's per-run shaped cluster advances, reshape the
    // kept prefix with the same per-run styles (no DOM mutation, no resolve).
    let budget = cand.content_w_css * scale;
    let mut ellipsis_advs: HashMap<NodeId, f32> = HashMap::new();
    let first_brush = runs.segments.first().map_or(runs.root_id, |s| s.1);
    let cut = truncate_by_clusters(&original_layout.layout, budget, first_brush, |brush| {
        *ellipsis_advs.entry(brush).or_insert_with(|| {
            measure_ellipsis_advance(shaper, runs.style_of(brush), scale, &align)
        })
    })?;
    let prefix = original_text.get(..cut.end).unwrap_or("");
    let truncated = format!("{prefix}{ELLIPSIS}");
    if truncated == original_text {
        return None;
    }

    let truncated_layout = shape_text_layout(
        shaper,
        &original_text,
        &cut,
        &runs,
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
