//! Scroll containers (issue #26): `overflow-y: auto | scroll` elements.
//!
//! Blitz `=0.3.0-beta.4` owns the offset itself (`Node::scroll_offset`), and
//! its paint, hit testing and our damage geometry already translate by it.
//! hx-native owns everything around it: which container a wheel / key /
//! scrollbar press moves, clamping after layout, keeping an offset by element
//! id across fragment swaps, and the engine-drawn Win95 scrollbar (Blitz's
//! own overlay thumb is behind the `scrollbars` feature, which stays off: it
//! fades on a wall-clock timer and is not the Win95 look).
//!
//! The engine also reserves the 16 px scrollbar gutter (Blitz beta.4 layout
//! hard-codes Taffy's `scrollbar_width` to 0): see [`gutter_css`] and
//! `LiveSurface::reserve_new_gutters`.
//!
//! Vertical only: `overflow-x` offsets are never written.

use blitz_dom::{NodeId, ScrollBehavior};
use blitz_html::HtmlDocument;
use style::values::computed::Overflow;

use crate::damage::{element_border_box_css, IntRect};
use crate::select_ui::ControlRect;
use crate::types::{RgbaImage, SCROLLBAR_WIDTH_PX, SCROLL_ROW_PX};

/// Minimum thumb length in CSS px (HTML_CONTRACT §4).
pub(crate) const THUMB_MIN_PX: f32 = 8.0;

const BG: [u8; 4] = [0xC0, 0xC0, 0xC0, 0xFF];
const HI: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const SH: [u8; 4] = [0x80, 0x80, 0x80, 0xFF];
const DK: [u8; 4] = [0x0A, 0x0A, 0x0A, 0xFF];
const PANEL: [u8; 4] = [0xDF, 0xDF, 0xDF, 0xFF];
const INK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];

/// True for an element whose computed `overflow-y` is `auto` or `scroll`,
/// other than `<html>` / `<body>` (their overflow belongs to the viewport,
/// which hx-native never scrolls).
pub(crate) fn is_scroll_container(doc: &HtmlDocument, nid: NodeId) -> bool {
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    let Some(el) = node.element_data() else {
        return false;
    };
    if matches!(&*el.name.local, "html" | "body") {
        return false;
    }
    node.primary_styles()
        .is_some_and(|s| matches!(s.clone_overflow_y(), Overflow::Auto | Overflow::Scroll))
}

fn overflow_y_is_scroll(doc: &HtmlDocument, nid: NodeId) -> bool {
    doc.get_node(nid)
        .and_then(|n| n.primary_styles())
        .is_some_and(|s| matches!(s.clone_overflow_y(), Overflow::Scroll))
}

/// Whether a container needs the engine-reserved 16 px gutter (issue #26
/// round 3): `overflow-y: scroll` always, `auto` once its content overflows.
pub(crate) fn wants_gutter(doc: &HtmlDocument, nid: NodeId) -> bool {
    is_scroll_container(doc, nid)
        && (overflow_y_is_scroll(doc, nid) || max_offset_y(doc, nid) > 0.5)
}

/// The author's computed `padding-right` of `nid` as CSS text, read before
/// the engine's gutter rule applies to it: a length (`8px`), a percentage
/// (`5%`, still relative to the containing block), or for a mixed `calc()`
/// the used value in px from the last layout.
pub(crate) fn author_padding_right_css(doc: &HtmlDocument, nid: NodeId) -> String {
    let Some(node) = doc.get_node(nid) else {
        return "0px".into();
    };
    let used = node.final_layout().padding.right;
    let Some(styles) = node.primary_styles() else {
        return format!("{used}px");
    };
    let lp = &styles.get_padding().padding_right.0;
    if let Some(l) = lp.to_length() {
        format!("{}px", l.px())
    } else if let Some(p) = lp.to_percentage() {
        format!("{}%", p.0 * 100.0)
    } else {
        format!("{used}px")
    }
}

/// CSS for the engine-owned gutter sheet: one `!important` rule per
/// reserved id, author padding + [`SCROLLBAR_WIDTH_PX`].
pub(crate) fn gutter_css<'a>(gutters: impl Iterator<Item = (&'a String, &'a String)>) -> String {
    let mut css = String::new();
    for (id, base) in gutters {
        let esc: String = id
            .chars()
            .flat_map(|c| match c {
                '"' | '\\' => vec!['\\', c],
                '\n' | '\r' | '\x0c' => vec![' '],
                c => vec![c],
            })
            .collect();
        css.push_str(&format!(
            "[id=\"{esc}\"]{{padding-right:calc({base} + {SCROLLBAR_WIDTH_PX}px) !important}}\n"
        ));
    }
    css
}

/// Every scroll container, in document order (ancestors before descendants).
pub(crate) fn scroll_containers(doc: &HtmlDocument) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in node.children.iter().rev() {
            stack.push(c);
        }
        if node.is_element() && is_scroll_container(doc, nid) {
            out.push(nid);
        }
    }
    out
}

/// Current vertical offset in CSS px.
pub(crate) fn offset_y(doc: &HtmlDocument, nid: NodeId) -> f32 {
    doc.get_node(nid)
        .map(|n| n.scroll_offset().y as f32)
        .unwrap_or(0.0)
}

/// Largest vertical offset (scroll height minus visible height), CSS px.
pub(crate) fn max_offset_y(doc: &HtmlDocument, nid: NodeId) -> f32 {
    doc.get_node(nid)
        .map(|n| n.final_layout().scroll_height().max(0.0))
        .unwrap_or(0.0)
}

/// Write a vertical offset (Blitz clamps it to `[0, max]`). Returns whether
/// the offset changed.
pub(crate) fn set_offset_y(doc: &mut HtmlDocument, nid: NodeId, y: f32) -> bool {
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    let before = *node.scroll_offset();
    let y = if y.is_finite() { y } else { 0.0 };
    doc.scroll_to(nid, before.x, y as f64, ScrollBehavior::Instant);
    doc.get_node(nid)
        .is_some_and(|n| (n.scroll_offset().y - before.y).abs() > f64::EPSILON)
}

/// The container's padding box in CSS document px (border box minus borders;
/// Blitz clips scrolled content to this box).
pub(crate) fn padding_box_css(doc: &HtmlDocument, nid: NodeId) -> Option<ControlRect> {
    let (x, y, w, h) = element_border_box_css(doc, nid)?;
    let b = doc.get_node(nid)?.final_layout().border;
    Some(ControlRect {
        x: x + b.left,
        y: y + b.top,
        w: (w - b.left - b.right).max(0.0),
        h: (h - b.top - b.bottom).max(0.0),
    })
}

/// Visible (client) height in CSS px.
pub(crate) fn client_height(doc: &HtmlDocument, nid: NodeId) -> f32 {
    doc.get_node(nid)
        .map(|n| {
            let l = n.final_layout();
            (l.size.height - l.border.top - l.border.bottom).max(0.0)
        })
        .unwrap_or(0.0)
}

/// One page in CSS px: the visible height minus one row (never below one row).
pub(crate) fn page_px(doc: &HtmlDocument, nid: NodeId) -> f32 {
    (client_height(doc, nid) - SCROLL_ROW_PX).max(SCROLL_ROW_PX)
}

/// Scroll-container ancestors of `nid` (innermost first), walking the
/// containing-block chain the way Blitz positions boxes. `nid` itself is not
/// included.
pub(crate) fn scroll_ancestors(doc: &HtmlDocument, nid: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = doc.get_node(nid).and_then(|n| n.containing_block());
    while let Some(id) = cur {
        if is_scroll_container(doc, id) {
            out.push(id);
        }
        cur = doc.get_node(id).and_then(|n| n.containing_block());
    }
    out
}

/// Intersection of the padding boxes of `nid`'s scroll-container ancestors
/// (CSS px): the region where `nid` can be visible. `None` = not clipped by
/// any scroll container.
pub(crate) fn ancestor_clip_css(doc: &HtmlDocument, nid: NodeId) -> Option<ControlRect> {
    let mut clip: Option<ControlRect> = None;
    for a in scroll_ancestors(doc, nid) {
        if let Some(pb) = padding_box_css(doc, a) {
            clip = Some(match clip {
                None => pb,
                Some(c) => intersect(c, pb),
            });
        }
    }
    clip
}

pub(crate) fn intersect(a: ControlRect, b: ControlRect) -> ControlRect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    ControlRect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
}

/// `element_from_point`, but a hit on content that a scroll container clips
/// away (scrolled out of its padding box) resolves to that container's parent
/// instead, so an invisible row never takes a click.
pub(crate) fn visible_element_from_point(doc: &HtmlDocument, x: f32, y: f32) -> Option<NodeId> {
    let hit = doc.element_from_point(x, y)?;
    let mut result = hit;
    // Innermost first; the outermost container the point is outside of wins.
    for a in scroll_ancestors(doc, hit) {
        if padding_box_css(doc, a).is_some_and(|pb| !pb.contains(x, y)) {
            if let Some(p) = doc.get_node(a).and_then(|n| n.parent) {
                result = p;
            }
        }
    }
    Some(result)
}

/// Which part of a scrollbar a point is on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BarPart {
    Up,
    Down,
    TrackAbove,
    TrackBelow,
    Thumb,
}

/// Win95 scrollbar geometry for one container, CSS document px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BarGeom {
    pub nid: NodeId,
    pub bar: ControlRect,
    pub up: ControlRect,
    pub down: ControlRect,
    pub track: ControlRect,
    /// `None` when there is nothing to scroll (`overflow-y: scroll`, content
    /// fits) or the bar is too short for a thumb.
    pub thumb: Option<ControlRect>,
    pub offset: f32,
    pub max: f32,
    /// Clip from scroll-container ancestors (`None` = unclipped).
    pub clip: Option<ControlRect>,
}

impl BarGeom {
    pub fn part_at(&self, x: f32, y: f32) -> Option<BarPart> {
        if !self.bar.contains(x, y) {
            return None;
        }
        if self.clip.is_some_and(|c| !c.contains(x, y)) {
            return None;
        }
        if self.up.contains(x, y) {
            return Some(BarPart::Up);
        }
        if self.down.contains(x, y) {
            return Some(BarPart::Down);
        }
        match self.thumb {
            Some(t) if t.contains(x, y) => Some(BarPart::Thumb),
            Some(t) if y < t.y => Some(BarPart::TrackAbove),
            Some(_) => Some(BarPart::TrackBelow),
            None => None,
        }
    }

    /// Offset for a thumb whose top edge sits at `thumb_top` (CSS px).
    pub fn offset_for_thumb_top(&self, thumb_top: f32) -> f32 {
        let Some(t) = self.thumb else {
            return self.offset;
        };
        let travel = self.track.h - t.h;
        if travel <= 0.0 {
            return 0.0;
        }
        ((thumb_top - self.track.y) / travel).clamp(0.0, 1.0) * self.max
    }
}

/// Geometry of the bar the engine draws for `nid`, or `None` when no bar is
/// shown (`overflow-y: auto` with everything fitting, or no layout).
pub(crate) fn bar_geometry(doc: &HtmlDocument, nid: NodeId) -> Option<BarGeom> {
    if !is_scroll_container(doc, nid) {
        return None;
    }
    let max = max_offset_y(doc, nid);
    let always = overflow_y_is_scroll(doc, nid);
    if max <= 0.5 && !always {
        return None;
    }
    let pb = padding_box_css(doc, nid)?;
    if pb.w <= 0.0 || pb.h <= 0.0 {
        return None;
    }
    let w = SCROLLBAR_WIDTH_PX.min(pb.w);
    let bar = ControlRect {
        x: pb.x + pb.w - w,
        y: pb.y,
        w,
        h: pb.h,
    };
    let arrow_h = SCROLLBAR_WIDTH_PX.min(pb.h / 2.0);
    let up = ControlRect {
        x: bar.x,
        y: bar.y,
        w,
        h: arrow_h,
    };
    let down = ControlRect {
        x: bar.x,
        y: bar.y + bar.h - arrow_h,
        w,
        h: arrow_h,
    };
    let track = ControlRect {
        x: bar.x,
        y: up.y + up.h,
        w,
        h: (bar.h - 2.0 * arrow_h).max(0.0),
    };
    let offset = offset_y(doc, nid).clamp(0.0, max.max(0.0));
    let client = client_height(doc, nid);
    let thumb = if max > 0.5 && track.h >= THUMB_MIN_PX {
        let len = (track.h * client / (client + max))
            .max(THUMB_MIN_PX)
            .min(track.h)
            .round();
        let top = track.y + ((track.h - len) * offset / max).round();
        Some(ControlRect {
            x: bar.x,
            y: top,
            w,
            h: len,
        })
    } else {
        None
    };
    Some(BarGeom {
        nid,
        bar,
        up,
        down,
        track,
        thumb,
        offset,
        max,
        clip: ancestor_clip_css(doc, nid),
    })
}

/// Bar geometry for every container that shows one, innermost last.
pub(crate) fn all_bars(doc: &HtmlDocument) -> Vec<BarGeom> {
    scroll_containers(doc)
        .into_iter()
        .filter_map(|nid| bar_geometry(doc, nid))
        .collect()
}

/// The innermost bar part under `(x, y)`.
pub(crate) fn bar_hit(doc: &HtmlDocument, x: f32, y: f32) -> Option<(BarGeom, BarPart)> {
    all_bars(doc)
        .into_iter()
        .rev()
        .find_map(|g| g.part_at(x, y).map(|p| (g, p)))
}

/// Vertical scroll containers under `(x, y)`, innermost first, that are
/// visible there (inside every scroll ancestor's padding box).
pub(crate) fn containers_at(doc: &HtmlDocument, x: f32, y: f32) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = scroll_containers(doc)
        .into_iter()
        .filter(|&nid| {
            padding_box_css(doc, nid).is_some_and(|pb| pb.contains(x, y))
                && ancestor_clip_css(doc, nid).is_none_or(|c| c.contains(x, y))
        })
        .collect();
    v.reverse();
    v
}

/// Whether `nid` can move by `dy` (negative = up).
pub(crate) fn can_scroll(doc: &HtmlDocument, nid: NodeId, dy: f32) -> bool {
    let off = offset_y(doc, nid);
    let max = max_offset_y(doc, nid);
    if dy < 0.0 {
        off > 0.0
    } else if dy > 0.0 {
        off < max - 0.01
    } else {
        false
    }
}

/// Container border box in device px (snapped outward), for damage.
pub(crate) fn container_damage_rect(doc: &HtmlDocument, nid: NodeId, scale: f64) -> IntRect {
    match crate::damage::element_border_box_device(doc, nid, scale) {
        Some((x, y, w, h)) => {
            let x0 = x.floor() as i32;
            let y0 = y.floor() as i32;
            let x1 = (x + w).ceil() as i32;
            let y1 = (y + h).ceil() as i32;
            IntRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            }
        }
        None => IntRect::default(),
    }
}

/// Pressed visual state (an arrow held down).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct PressedArrow {
    pub nid: NodeId,
    pub part: BarPart,
}

/// Paint every visible scrollbar onto `frame` (after Blitz raster, before
/// the `<select>` overlays). Opaque and drawn the same way every frame, so a
/// partial repaint equals a full one.
pub(crate) fn paint_scrollbars(
    doc: &HtmlDocument,
    frame: &mut RgbaImage,
    scale: f64,
    pressed: Option<PressedArrow>,
) {
    for g in all_bars(doc) {
        let mut cv = Canvas::new(frame, scale, g.clip);
        let enabled = g.thumb.is_some();
        // Track: 1 px checker of --bg and --hi, phase from the track origin
        // (`repeating-conic-gradient(#fff 0 25%, #c0c0c0 0 50%) 0 0/2px 2px`).
        cv.checker(g.track);
        for (part, rect) in [(BarPart::Up, g.up), (BarPart::Down, g.down)] {
            let is_pressed = pressed.is_some_and(|p| p.nid == g.nid && p.part == part);
            cv.fill(rect, BG);
            if is_pressed {
                cv.press_bevel(rect);
            } else {
                cv.raise_bevel(rect);
            }
            let shift = if is_pressed { 1.0 } else { 0.0 };
            let up = part == BarPart::Up;
            if enabled {
                cv.arrow(rect, up, shift, INK);
            } else {
                // Win95 disabled glyph: grey with a white emboss.
                cv.arrow(rect, up, 1.0, HI);
                cv.arrow(rect, up, 0.0, SH);
            }
        }
        if let Some(t) = g.thumb {
            cv.fill(t, BG);
            cv.raise_bevel(t);
        }
    }
}

/// Device-pixel drawing in CSS-px rects, clipped to the surface and an
/// optional CSS clip rect.
struct Canvas<'a> {
    frame: &'a mut RgbaImage,
    scale: f64,
    clip: IntRect,
}

impl<'a> Canvas<'a> {
    fn new(frame: &'a mut RgbaImage, scale: f64, clip: Option<ControlRect>) -> Self {
        let surface = IntRect {
            x: 0,
            y: 0,
            w: frame.width as i32,
            h: frame.height as i32,
        };
        let clip = match clip {
            Some(c) => intersect_int(surface, c.as_device(scale)),
            None => surface,
        };
        Self { frame, scale, clip }
    }

    fn device(&self, r: ControlRect) -> IntRect {
        // Round edges (not outward) so adjacent rects tile without overlap.
        let x0 = (r.x as f64 * self.scale).round() as i32;
        let y0 = (r.y as f64 * self.scale).round() as i32;
        let x1 = ((r.x + r.w) as f64 * self.scale).round() as i32;
        let y1 = ((r.y + r.h) as f64 * self.scale).round() as i32;
        intersect_int(
            self.clip,
            IntRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
        )
    }

    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        let di = ((y as u32 * self.frame.width + x as u32) * 4) as usize;
        self.frame.pixels[di..di + 4].copy_from_slice(&c);
    }

    fn fill(&mut self, r: ControlRect, c: [u8; 4]) {
        let d = self.device(r);
        for y in d.y..d.y + d.h {
            for x in d.x..d.x + d.w {
                self.put(x, y, c);
            }
        }
    }

    fn checker(&mut self, r: ControlRect) {
        let d = self.device(r);
        for y in d.y..d.y + d.h {
            for x in d.x..d.x + d.w {
                let lx = ((x as f64 + 0.5) / self.scale - r.x as f64).floor() as i64;
                let ly = ((y as f64 + 0.5) / self.scale - r.y as f64).floor() as i64;
                let c = if (lx + ly).rem_euclid(2) == 1 { HI } else { BG };
                self.put(x, y, c);
            }
        }
    }

    fn top(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(ControlRect { h: n, ..r }, c);
    }
    fn left(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(ControlRect { w: n, ..r }, c);
    }
    fn bottom(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(
            ControlRect {
                y: r.y + r.h - n,
                h: n,
                ..r
            },
            c,
        );
    }
    fn right(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(
            ControlRect {
                x: r.x + r.w - n,
                w: n,
                ..r
            },
            c,
        );
    }

    /// `--raise`: `inset -1px -1px dk, inset 1px 1px hi, inset -2px -2px sh,
    /// inset 2px 2px panel`, painted last-listed first.
    fn raise_bevel(&mut self, r: ControlRect) {
        self.top(r, 2.0, PANEL);
        self.left(r, 2.0, PANEL);
        self.bottom(r, 2.0, SH);
        self.right(r, 2.0, SH);
        self.top(r, 1.0, HI);
        self.left(r, 1.0, HI);
        self.bottom(r, 1.0, DK);
        self.right(r, 1.0, DK);
    }

    /// `--press`: `inset -1px -1px hi, inset 1px 1px dk, inset -2px -2px panel,
    /// inset 2px 2px sh`.
    fn press_bevel(&mut self, r: ControlRect) {
        self.top(r, 2.0, SH);
        self.left(r, 2.0, SH);
        self.bottom(r, 2.0, PANEL);
        self.right(r, 2.0, PANEL);
        self.top(r, 1.0, DK);
        self.left(r, 1.0, DK);
        self.bottom(r, 1.0, HI);
        self.right(r, 1.0, HI);
    }

    /// Win95 arrow glyph: 4 rows of 1/3/5/7 px, centred in a 16x16 button.
    fn arrow(&mut self, r: ControlRect, up: bool, shift: f32, c: [u8; 4]) {
        let cx = (r.x + (r.w / 2.0).floor() - 1.0 + shift).floor();
        let top = (r.y + (r.h / 2.0).floor() - 2.0 + shift).floor();
        for i in 0..4 {
            let half = if up { i } else { 3 - i } as f32;
            self.fill(
                ControlRect {
                    x: cx - half,
                    y: top + i as f32,
                    w: 2.0 * half + 1.0,
                    h: 1.0,
                },
                c,
            );
        }
    }
}

fn intersect_int(a: IntRect, b: IntRect) -> IntRect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    IntRect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0),
        h: (y1 - y0).max(0),
    }
}
