//! Damage-rect detection, inflation (ink), and merge.

use std::collections::HashMap;

use blitz_dom::NodeId;
use blitz_html::HtmlDocument;

use crate::types::{Rect, FULL_REPAINT_AREA_FRACTION, MAX_DAMAGE_RECTS};

/// Integer device-pixel bounds used while merging (converted to [`Rect`] for output).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IntRect {
    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn area(self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.w as u64 * self.h as u64
        }
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    pub fn intersects_or_near(self, other: Self, gap: i32) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        let ax0 = self.x - gap;
        let ay0 = self.y - gap;
        let ax1 = self.x + self.w + gap;
        let ay1 = self.y + self.h + gap;
        let bx0 = other.x;
        let by0 = other.y;
        let bx1 = other.x + other.w;
        let by1 = other.y + other.h;
        ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
    }

    pub fn clamp_to_surface(self, width: u32, height: u32) -> Self {
        let sw = width as i32;
        let sh = height as i32;
        let x0 = self.x.max(0).min(sw);
        let y0 = self.y.max(0).min(sh);
        let x1 = (self.x + self.w).max(0).min(sw);
        let y1 = (self.y + self.h).max(0).min(sh);
        Self {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0),
            h: (y1 - y0).max(0),
        }
    }

    pub fn to_rect(self) -> Rect {
        Rect {
            x: self.x as f32,
            y: self.y as f32,
            width: self.w as f32,
            height: self.h as f32,
        }
    }

    pub fn full_surface(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            w: width as i32,
            h: height as i32,
        }
    }

    /// True when this rect covers the whole surface (pending fold / full damage).
    pub fn is_full_surface(self, width: u32, height: u32) -> bool {
        self == Self::full_surface(width, height)
    }
}

/// Snapshot of one element's own box and ink at last paint (device pixels, float).
#[derive(Clone, Debug)]
pub(crate) struct NodeBounds {
    /// Border-box origin and size in device pixels (no overflow/shadow).
    pub own: (f64, f64, f64, f64),
    /// Ink rect (own + overflow + outline + shadow) in device pixels.
    pub ink: (f64, f64, f64, f64),
}

pub(crate) type BoundsSnapshot = HashMap<NodeId, NodeBounds>;

/// Result of damage detection + merge for one paint.
#[derive(Clone, Debug)]
pub(crate) struct DamagePlan {
    pub rects: Vec<IntRect>,
    pub merged_to_bbox: bool,
    pub full_repaint_area_fallback: bool,
    pub full_repaint: bool,
}

impl DamagePlan {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            rects: vec![IntRect::full_surface(width, height)],
            merged_to_bbox: false,
            full_repaint_area_fallback: false,
            full_repaint: true,
        }
    }
}

/// Round a float ink rect outward to integer device pixels and add a 1px AA pad.
pub(crate) fn snap_outward(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    surface_w: u32,
    surface_h: u32,
) -> IntRect {
    let x0 = x.floor() as i32 - 1;
    let y0 = y.floor() as i32 - 1;
    let x1 = (x + w).ceil() as i32 + 1;
    let y1 = (y + h).ceil() as i32 + 1;
    IntRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
    .clamp_to_surface(surface_w, surface_h)
}

/// Merge overlapping / nearby rects; collapse when over the cap; full-repaint on area.
pub(crate) fn merge_damage_rects(
    mut rects: Vec<IntRect>,
    surface_w: u32,
    surface_h: u32,
) -> DamagePlan {
    rects.retain(|r| !r.is_empty());
    if rects.is_empty() {
        return DamagePlan {
            rects: Vec::new(),
            merged_to_bbox: false,
            full_repaint_area_fallback: false,
            full_repaint: false,
        };
    }

    // Iteratively merge overlapping or nearby (2px gap) rects.
    let gap = 2;
    let mut changed = true;
    while changed {
        changed = false;
        let mut out: Vec<IntRect> = Vec::new();
        for r in rects.drain(..) {
            let mut merged = false;
            for o in out.iter_mut() {
                if o.intersects_or_near(r, gap) {
                    *o = o.union(r);
                    merged = true;
                    changed = true;
                    break;
                }
            }
            if !merged {
                out.push(r);
            }
        }
        rects = out;
    }

    let mut merged_to_bbox = false;
    if rects.len() > MAX_DAMAGE_RECTS {
        let bbox = rects
            .iter()
            .copied()
            .fold(IntRect::default(), IntRect::union);
        rects = vec![bbox];
        merged_to_bbox = true;
    }

    let surface_area = surface_w as u64 * surface_h as u64;
    let damage_area: u64 = rects.iter().map(|r| r.area()).sum();
    let mut full_repaint_area_fallback = false;
    let mut full_repaint = false;
    if surface_area > 0 && (damage_area as f64) > (surface_area as f64) * FULL_REPAINT_AREA_FRACTION
    {
        rects = vec![IntRect::full_surface(surface_w, surface_h)];
        full_repaint_area_fallback = true;
        full_repaint = true;
    }

    DamagePlan {
        rects,
        merged_to_bbox,
        full_repaint_area_fallback,
        full_repaint,
    }
}

/// Compute the element's own border box in device pixels: (x, y, w, h).
fn own_box_device(doc: &HtmlDocument, nid: NodeId, scale: f64) -> Option<(f64, f64, f64, f64)> {
    let node = doc.get_node(nid)?;
    if !node.is_element() {
        return None;
    }
    let pos = node.absolute_position(0.0, 0.0);
    let layout = node.final_layout();
    let x = pos.x as f64 * scale;
    let y = pos.y as f64 * scale;
    let w = layout.size.width as f64 * scale;
    let h = layout.size.height as f64 * scale;
    Some((x, y, w, h))
}

/// Inflate own box by outline, box-shadow, and scrollable overflow → ink rect.
pub(crate) fn ink_box_device(
    doc: &HtmlDocument,
    nid: NodeId,
    scale: f64,
) -> Option<(f64, f64, f64, f64)> {
    let (ox, oy, ow, oh) = own_box_device(doc, nid, scale)?;
    let node = doc.get_node(nid)?;

    let mut x0 = ox;
    let mut y0 = oy;
    let mut x1 = ox + ow;
    let mut y1 = oy + oh;

    // `scrollable_overflow` is node-local device px at the document's current
    // viewport scale. Convert through CSS so origin and size always use the
    // same `scale` (avoids size-only double-scale when the two diverge).
    let overflow = node.scrollable_overflow();
    let doc_scale = doc.viewport().scale_f64().max(1e-6);
    let to_scale = scale / doc_scale;
    x0 = x0.min(ox + overflow.x0 * to_scale);
    y0 = y0.min(oy + overflow.y0 * to_scale);
    x1 = x1.max(ox + overflow.x1 * to_scale);
    y1 = y1.max(oy + overflow.y1 * to_scale);

    if let Some(style) = node.primary_styles() {
        let outline_w = style.get_outline().outline_width.0.to_f64_px() * scale;
        if outline_w > 0.0 {
            x0 -= outline_w;
            y0 -= outline_w;
            x1 += outline_w;
            y1 += outline_w;
        }

        // box-shadow ≈ offset + 1.5×blur + spread (probe heuristic).
        for shadow in style.get_effects().box_shadow.0.iter() {
            if shadow.inset {
                continue;
            }
            let sx = shadow.base.horizontal.px() as f64 * scale;
            let sy = shadow.base.vertical.px() as f64 * scale;
            let blur = shadow.base.blur.px() as f64 * scale;
            let spread = shadow.spread.px() as f64 * scale;
            let pad = spread + blur * 1.5;
            x0 = x0.min(ox + sx - pad);
            y0 = y0.min(oy + sy - pad);
            x1 = x1.max(ox + ow + sx + pad);
            y1 = y1.max(oy + oh + sy + pad);
        }
    }

    Some((x0, y0, x1 - x0, y1 - y0))
}

/// Snapshot every element's own + ink bounds after a successful paint resolve.
pub(crate) fn snapshot_bounds(doc: &HtmlDocument, scale: f64) -> BoundsSnapshot {
    let mut map = BoundsSnapshot::new();
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
        if let (Some(own), Some(ink)) = (
            own_box_device(doc, nid, scale),
            ink_box_device(doc, nid, scale),
        ) {
            map.insert(nid, NodeBounds { own, ink });
        }
    }
    map
}

/// Collect node ids that still carry non-empty restyle damage (call after `resolve_stylist`).
pub(crate) fn collect_style_damaged(doc: &HtmlDocument) -> Vec<NodeId> {
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
        if node.damage().is_some_and(|d| !d.is_empty()) {
            out.push(nid);
        }
    }
    out
}

fn own_changed(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    const EPS: f64 = 0.01;
    (a.0 - b.0).abs() > EPS
        || (a.1 - b.1).abs() > EPS
        || (a.2 - b.2).abs() > EPS
        || (a.3 - b.3).abs() > EPS
}

/// Build raw damage rects from style-damaged ids + own-box diffs vs last paint.
///
/// Ancestors whose only change is overflow (own box unchanged, not style-damaged)
/// are ignored so a ticker crawl does not dirty `<html>` into a full-screen rect.
pub(crate) fn compute_raw_damage(
    doc: &HtmlDocument,
    scale: f64,
    surface_w: u32,
    surface_h: u32,
    previous: &BoundsSnapshot,
    style_damaged: &[NodeId],
) -> Vec<IntRect> {
    let style_set: std::collections::HashSet<NodeId> = style_damaged.iter().copied().collect();
    let mut rects: Vec<IntRect> = Vec::new();

    let current = snapshot_bounds(doc, scale);

    // Disappeared nodes: old ink.
    for (nid, prev) in previous {
        if !current.contains_key(nid) {
            let (x, y, w, h) = prev.ink;
            let r = snap_outward(x, y, w, h, surface_w, surface_h);
            if !r.is_empty() {
                rects.push(r);
            }
        }
    }

    // Current nodes: style damage, appearance, or own-box change.
    for (nid, now) in &current {
        let prev = previous.get(nid);
        let style = style_set.contains(nid);
        let appeared = prev.is_none();
        let box_changed = prev.is_some_and(|p| own_changed(p.own, now.own));

        // Overflow-only on an ancestor: own unchanged and not style-damaged → skip.
        if !style && !appeared && !box_changed {
            continue;
        }

        if let Some(p) = prev {
            let (x, y, w, h) = p.ink;
            let r = snap_outward(x, y, w, h, surface_w, surface_h);
            if !r.is_empty() {
                rects.push(r);
            }
        }
        let (x, y, w, h) = now.ink;
        let r = snap_outward(x, y, w, h, surface_w, surface_h);
        if !r.is_empty() {
            rects.push(r);
        }
    }

    rects
}

#[cfg(test)]
mod scale_space_tests {
    use super::*;
    use blitz_dom::{DocumentConfig, StyleThreading};
    use blitz_html::{HtmlDocument, HtmlProvider};
    use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};
    use std::sync::Arc;

    fn doc_at(html: &str, w: u32, h: u32, scale: f64) -> HtmlDocument {
        HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(BlitzViewport::new(w, h, scale as f32, ColorScheme::Light)),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                style_threading: StyleThreading::Sequential,
                incremental: Some(true),
                net_provider: None,
                ..Default::default()
            },
        )
    }

    #[test]
    fn own_and_ink_scale_consistently() {
        let html = r#"<!DOCTYPE html><html><body style="margin:0">
<div id="box" style="position:absolute;left:100px;top:50px;width:80px;height:40px;background:#f00"></div>
</body></html>"#;
        for scale in [1.0_f64, 2.0, 1.25] {
            let phys_w = (400.0 * scale).round() as u32;
            let phys_h = (300.0 * scale).round() as u32;
            let mut doc = doc_at(html, phys_w, phys_h, scale);
            doc.resolve(0.0);
            let nid = doc.get_element_by_id("box").expect("box");
            let own = own_box_device(&doc, nid, scale).unwrap();
            let ink = ink_box_device(&doc, nid, scale).unwrap();
            let node = doc.get_node(nid).unwrap();
            let pos = node.absolute_position(0.0, 0.0);
            let layout = node.final_layout();
            let overflow = *node.scrollable_overflow();
            eprintln!(
                "scale={scale} pos={:?} layout_size={:?} overflow={:?} own={:?} ink={:?}",
                (pos.x, pos.y),
                (layout.size.width, layout.size.height),
                (overflow.x0, overflow.y0, overflow.x1, overflow.y1),
                own,
                ink
            );
            assert!(
                (own.0 - 100.0 * scale).abs() < 1.0,
                "own.x scale={scale} own={own:?}"
            );
            assert!(
                (own.1 - 50.0 * scale).abs() < 1.0,
                "own.y scale={scale} own={own:?}"
            );
            assert!(
                (own.2 - 80.0 * scale).abs() < 1.0,
                "own.w scale={scale} own={own:?}"
            );
            assert!(
                (own.3 - 40.0 * scale).abs() < 1.0,
                "own.h scale={scale} own={own:?}"
            );
        }
    }

    /// Regression for issue #20: if the ink `scale` argument and the document
    /// viewport scale diverge, overflow must be converted so origin and size
    /// stay in one space (not size-only scaled).
    #[test]
    fn ink_overflow_matches_own_scale_when_doc_scale_diverges() {
        let html = r#"<!DOCTYPE html><html><body style="margin:0">
<div id="box" style="position:absolute;left:100px;top:50px;width:80px;height:40px;background:#f00">
  <div style="position:absolute;left:0;top:0;width:120px;height:60px;background:#0f0"></div>
</div>
</body></html>"#;
        let mut doc = doc_at(html, 800, 600, 2.0);
        doc.resolve(0.0);
        let nid = doc.get_element_by_id("box").expect("box");
        // Ask for ink at scale 1 while the document viewport is still scale 2.
        let ink = ink_box_device(&doc, nid, 1.0).unwrap();
        // Child extends to 120×60 CSS → ink at scale 1 must be ~120×60 (plus a
        // few px of outline/AA pad), not ~240×120 from raw device overflow.
        assert!(
            ink.2 > 110.0 && ink.2 < 140.0,
            "ink width must follow requested scale (~120), got {ink:?}"
        );
        assert!(
            ink.3 > 50.0 && ink.3 < 80.0,
            "ink height must follow requested scale (~60), got {ink:?}"
        );
        assert!(
            ink.0 > 90.0 && ink.0 < 110.0,
            "ink origin x must follow requested scale (~100), got {ink:?}"
        );
        assert!(
            ink.1 > 40.0 && ink.1 < 60.0,
            "ink origin y must follow requested scale (~50), got {ink:?}"
        );
    }
}
