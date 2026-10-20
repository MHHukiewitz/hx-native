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
//! over shaped cluster advances — no per-character re-layout.
//!
//! # Paint without dirtying layout for hit-testing
//!
//! The DOM text nodes always keep the full original string. For paint we swap
//! the element's cached [`TextLayout`](blitz_dom::node::TextLayout) with a
//! truncated shaped layout, then swap the original layout back. Hit-testing,
//! tooltips, and slice diffs therefore see the full string and the original
//! geometry without a post-paint `needs_resolve`.
//!
//! Truncated layouts are cached under a key of
//! `(original text, content-box width, font/style fingerprint)`. A cache miss
//! does one reshape resolve; a hit does none. Clear the cache when the DOM
//! mutates (`EllipsisCache::clear`).
//!
//! # Out of scope
//!
//! - RTL / bidirectional ellipsis sides
//! - Multi-line clamp (`-webkit-line-clamp` / `line-clamp`)

use std::collections::HashMap;

use blitz_dom::node::{TextBrush, TextLayout};
use blitz_dom::NodeId;
use blitz_html::HtmlDocument;
use parley::layout::Layout;

/// U+2026 HORIZONTAL ELLIPSIS — CSS `text-overflow: ellipsis` marker.
const ELLIPSIS: &str = "\u{2026}";

/// Cache key for a truncated shaped layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TruncationKey {
    original: String,
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

/// Restores original inline layouts after paint. Does not touch DOM text and
/// does not mark the document dirty.
pub(crate) struct EllipsisPaintGuard {
    backups: Vec<(NodeId, Box<TextLayout>)>,
    /// True when at least one truncated layout was installed for this paint.
    #[allow(dead_code)]
    pub(crate) applied: bool,
    /// True when this paint had to reshape (cache miss).
    #[allow(dead_code)]
    pub(crate) reshaped: bool,
}

impl EllipsisPaintGuard {
    pub(crate) fn restore(self, doc: &mut HtmlDocument) {
        for (nid, layout) in self.backups {
            if let Some(el) = doc.get_node_mut(nid).and_then(|n| n.element_data_mut()) {
                el.inline_layout_data = Some(layout);
            }
        }
    }
}

/// Install truncated inline layouts for paint. Call after the document’s normal
/// style+layout resolve (DOM still holds the full text).
///
/// `resolve_extra` is called once per cache miss to reshape truncated text
/// (caller should count it separately from the main paint resolve).
pub(crate) fn prepare_ellipsis_for_paint(
    doc: &mut HtmlDocument,
    cache: &mut EllipsisCache,
    mut resolve_extra: impl FnMut(&mut HtmlDocument),
) -> EllipsisPaintGuard {
    let scale = doc.viewport().scale().max(0.000_1);
    let candidates = find_ellipsis_candidates(doc, scale);
    if candidates.is_empty() {
        return EllipsisPaintGuard {
            backups: Vec::new(),
            applied: false,
            reshaped: false,
        };
    }

    let mut backups = Vec::new();
    let mut applied = false;
    let mut reshaped = false;

    for cand in candidates {
        let Some(prep) = prepare_one(doc, cache, &cand, scale, &mut resolve_extra) else {
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

    EllipsisPaintGuard {
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

fn estimate_ellipsis_advance(layout: &Layout<TextBrush>) -> f32 {
    let mut font_size = None;
    for line in layout.lines() {
        if let Some(run) = line.runs().next() {
            font_size = Some(run.font_size());
            break;
        }
    }
    // U+2026 is typically ~0.6–0.75em in DejaVu-like faces.
    font_size.unwrap_or(13.0) * 0.7
}

/// Binary search cluster boundaries so prefix advance + ellipsis fits `budget`
/// (same units as cluster advances — scaled px).
fn truncate_by_clusters(original: &str, layout: &Layout<TextBrush>, budget: f32) -> String {
    let clusters = cluster_cum_advances(layout);
    if clusters.len() <= 1 {
        return ELLIPSIS.to_string();
    }
    let full = clusters.last().map(|(_, a)| *a).unwrap_or(0.0);
    let ellipsis_adv = estimate_ellipsis_advance(layout);
    if full + 0.5 <= budget {
        return original.to_string();
    }
    let target = (budget - ellipsis_adv).max(0.0);

    // clusters[0] = (0,0); search largest i >= 1 with cum <= target, or 0 for
    // empty prefix.
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
    // Never split a multi-byte scalar: cluster ends are already grapheme-safe.
    let prefix = original.get(..end).unwrap_or("");
    format!("{prefix}{ELLIPSIS}")
}

fn set_combined_text_damaging(doc: &mut HtmlDocument, text_nodes: &[NodeId], combined: &str) {
    let mut mutr = doc.mutate();
    if let Some((first, rest)) = text_nodes.split_first() {
        mutr.set_node_text(*first, combined);
        for &tid in rest {
            mutr.set_node_text(tid, "");
        }
    }
}

fn write_text_no_damage(doc: &mut HtmlDocument, text_nodes: &[(NodeId, String)]) {
    for (tid, text) in text_nodes {
        if let Some(td) = doc.get_node_mut(*tid).and_then(|n| n.text_data_mut()) {
            td.content.clear();
            td.content.push_str(text);
        }
    }
}

fn take_inline_layout(doc: &mut HtmlDocument, nid: NodeId) -> Option<Box<TextLayout>> {
    doc.get_node_mut(nid)
        .and_then(|n| n.element_data_mut())
        .and_then(|el| el.inline_layout_data.take())
}

fn prepare_one(
    doc: &mut HtmlDocument,
    cache: &mut EllipsisCache,
    cand: &Candidate,
    scale: f32,
    resolve_extra: &mut impl FnMut(&mut HtmlDocument),
) -> Option<Prepared> {
    let original_text = read_combined_text(doc, &cand.text_nodes);
    if original_text.is_empty() {
        return None;
    }

    let node = doc.get_node(cand.element_id)?;
    let style_fp = style_fingerprint(node);
    let key = TruncationKey {
        original: original_text.clone(),
        content_w_bits: cand.content_w_css.to_bits(),
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

    // Cache miss: truncate from shaped advances, reshape once, cache, restore DOM.
    let original_layout = {
        let node = doc.get_node(cand.element_id)?;
        let el = node.element_data()?;
        let inline = el.inline_layout_data.as_ref()?;
        Box::new((**inline).clone())
    };

    let budget = cand.content_w_css * scale;
    let truncated = truncate_by_clusters(&original_text, &original_layout.layout, budget);
    if truncated == original_text {
        return None;
    }

    let originals: Vec<(NodeId, String)> = cand
        .text_nodes
        .iter()
        .filter_map(|&tid| {
            let content = doc.get_node(tid)?.text_data()?.content.clone();
            Some((tid, content))
        })
        .collect();

    set_combined_text_damaging(doc, &cand.text_nodes, &truncated);
    resolve_extra(doc);

    let truncated_layout = take_inline_layout(doc, cand.element_id)?;
    write_text_no_damage(doc, &originals);

    cache.by_node.insert(
        cand.element_id,
        CacheEntry {
            key,
            truncated,
            layout: Box::new((*truncated_layout).clone()),
        },
    );

    // Document has no inline layout on this node until the caller installs the
    // truncated one for paint; originals stay in `original_layout` for restore.
    Some(Prepared {
        original_layout,
        truncated_layout,
        reshaped: true,
    })
}
