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
//! Truncation cuts at a Unicode scalar boundary so `prefix + "…"` fits the
//! content box, then Blitz re-lays-out and paints that text.
//!
//! # DOM restore
//!
//! Truncation mutates text-node `content` only for the paint pass. Original
//! strings are restored before `rasterize_plan` returns. `needs_resolve` is set
//! so the next hit-test / sync / `ensure_resolved` rebuilds layout from the
//! original text. Hit-testing, tooltips, and slice diffs therefore see the
//! full string.
//!
//! # Out of scope
//!
//! - RTL / bidirectional ellipsis sides
//! - Multi-line clamp (`-webkit-line-clamp` / `line-clamp`)

use blitz_dom::NodeId;
use blitz_html::HtmlDocument;

/// U+2026 HORIZONTAL ELLIPSIS — CSS `text-overflow: ellipsis` marker.
const ELLIPSIS: &str = "\u{2026}";

/// One text node whose content was changed for paint and must be restored.
struct TextRestore {
    node_id: NodeId,
    original: String,
}

/// Backups taken while paint uses truncated text. Callers must [`restore`].
pub(crate) struct EllipsisBackup {
    restores: Vec<TextRestore>,
}

impl EllipsisBackup {
    pub(crate) fn did_mutate(&self) -> bool {
        !self.restores.is_empty()
    }

    /// Write original text back. Marks ancestors damaged via `set_node_text`.
    pub(crate) fn restore(self, doc: &mut HtmlDocument) {
        if self.restores.is_empty() {
            return;
        }
        let mut mutr = doc.mutate();
        for r in self.restores {
            mutr.set_node_text(r.node_id, &r.original);
        }
    }
}

/// Mutate overflowing ellipsis candidates so paint draws truncated text + `…`.
///
/// Call after style+layout for damage planning. Caller must `doc.resolve` again
/// when [`EllipsisBackup::did_mutate`] is true, paint, then [`EllipsisBackup::restore`].
pub(crate) fn apply_text_overflow_ellipsis(doc: &mut HtmlDocument) -> EllipsisBackup {
    let scale = doc.viewport().scale().max(0.000_1);
    let candidates = find_ellipsis_candidates(doc, scale);
    if candidates.is_empty() {
        return EllipsisBackup {
            restores: Vec::new(),
        };
    }

    let mut restores = Vec::new();
    for cand in candidates {
        apply_one(doc, cand, scale, &mut restores);
    }
    EllipsisBackup { restores }
}

struct Candidate {
    element_id: NodeId,
    content_w_css: f32,
    text_nodes: Vec<NodeId>,
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
        if !node.is_element() {
            continue;
        }
        if !is_ellipsis_candidate(doc, nid) {
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

fn is_ellipsis_candidate(doc: &HtmlDocument, nid: NodeId) -> bool {
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    let Some(styles) = node.primary_styles() else {
        return false;
    };
    // Avoid `resolved_style_value` (needs ThreadState::LAYOUT). Use Debug of the
    // computed longhands — no direct stylo dep, Cargo.lock unchanged.
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

fn set_combined_text(doc: &mut HtmlDocument, text_nodes: &[NodeId], combined: &str) {
    let mut mutr = doc.mutate();
    if let Some((first, rest)) = text_nodes.split_first() {
        mutr.set_node_text(*first, combined);
        for &tid in rest {
            mutr.set_node_text(tid, "");
        }
    }
}

fn apply_one(doc: &mut HtmlDocument, cand: Candidate, scale: f32, restores: &mut Vec<TextRestore>) {
    let originals: Vec<(NodeId, String)> = cand
        .text_nodes
        .iter()
        .filter_map(|&tid| {
            let content = doc.get_node(tid)?.text_data()?.content.clone();
            Some((tid, content))
        })
        .collect();
    if originals.is_empty() {
        return;
    }

    let original = originals
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<String>();
    if original.is_empty() {
        return;
    }

    let char_ends: Vec<usize> = original
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .collect();
    if char_ends.is_empty() {
        return;
    }

    // Binary search largest prefix char-count whose width with `…` fits.
    let mut lo = 0usize;
    let mut hi = char_ends.len();
    let mut best = 0usize;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let prefix_end = if mid == 0 { 0 } else { char_ends[mid - 1] };
        let trial = format!("{}{}", &original[..prefix_end], ELLIPSIS);
        set_combined_text(doc, &cand.text_nodes, &trial);
        doc.resolve(0.0);
        let fits = element_text_fits(doc, cand.element_id, cand.content_w_css, scale);
        if fits {
            best = mid;
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }

    let prefix_end = if best == 0 { 0 } else { char_ends[best - 1] };
    let painted = format!("{}{}", &original[..prefix_end], ELLIPSIS);
    set_combined_text(doc, &cand.text_nodes, &painted);

    for (node_id, original) in originals {
        restores.push(TextRestore { node_id, original });
    }
}

fn element_text_fits(
    doc: &HtmlDocument,
    element_id: NodeId,
    content_w_css: f32,
    scale: f32,
) -> bool {
    let Some(node) = doc.get_node(element_id) else {
        return true;
    };
    match inline_text_width_css(node, scale) {
        Some(w) => w <= content_w_css + 0.5,
        None => true,
    }
}
