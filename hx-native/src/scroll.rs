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

/// Win95 literals from Mainnet `_tokens.css` 9fc61473 (look-token fallbacks).
pub(crate) const BG: [u8; 4] = [0xC0, 0xC0, 0xC0, 0xFF];
pub(crate) const HI: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
pub(crate) const SH: [u8; 4] = [0x80, 0x80, 0x80, 0xFF];
pub(crate) const EDGE_DK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
pub(crate) const PANEL: [u8; 4] = [0xDF, 0xDF, 0xDF, 0xFF];
pub(crate) const INK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];

/// Win95 chrome colours for engine-drawn controls (scrollbar, slider),
/// read from the Mainnet `_tokens.css` look tokens (9fc61473) as computed on
/// the control (issue #35): `--bg` face, `--edge-hi` highlight, `--edge-lt`
/// 3D light, `--edge-sh` shadow, `--edge-dk` dark shadow, `--ink` glyphs and
/// focus dots, `--scroll-track` the scrollbar track. A missing or
/// unparseable token falls back to the Win95 literal, so a host swaps Win
/// 3.1 / 95 / 98 looks at runtime by changing the tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    pub bg: [u8; 4],
    pub hi: [u8; 4],
    pub lt: [u8; 4],
    pub sh: [u8; 4],
    pub dk: [u8; 4],
    pub ink: [u8; 4],
    /// `--scroll-track`: `None` = the Win95 1 px face / highlight checker
    /// (also when the token is the `--dither` background, not a colour).
    pub track: Option<[u8; 4]>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: BG,
            hi: HI,
            lt: PANEL,
            sh: SH,
            dk: EDGE_DK,
            ink: INK,
            track: None,
        }
    }
}

/// The palette from `nid`'s computed custom properties (stylo
/// `ComputedValues::custom_properties`, `var()` already substituted).
pub(crate) fn palette_for(doc: &HtmlDocument, nid: NodeId) -> Palette {
    let mut pal = Palette::default();
    let Some(style) = doc.get_node(nid).and_then(|n| n.primary_styles()) else {
        return pal;
    };
    let props = style.custom_properties();
    let token = |name: &str| -> Option<[u8; 4]> {
        props
            .get_for_cssom(&style::Atom::from(name))
            .and_then(|v| parse_css_color(&v.to_variable_value().css))
    };
    pal.bg = token("bg").unwrap_or(pal.bg);
    pal.hi = token("edge-hi").unwrap_or(pal.hi);
    pal.lt = token("edge-lt").unwrap_or(pal.lt);
    pal.sh = token("edge-sh").unwrap_or(pal.sh);
    pal.dk = token("edge-dk").unwrap_or(pal.dk);
    pal.ink = token("ink").unwrap_or(pal.ink);
    pal.track = token("scroll-track");
    pal
}

/// Opaque sRGB from a token value: `#rgb`, `#rrggbb`, `#rgba`, `#rrggbbaa`,
/// `rgb(r, g, b)` / `rgba(r, g, b, a)` with 0-255 channels (space or comma
/// separated), or a few basic keywords. Alpha is dropped (chrome is opaque).
pub(crate) fn parse_css_color(text: &str) -> Option<[u8; 4]> {
    let t = text.trim().to_ascii_lowercase();
    if let Some(hex) = t.strip_prefix('#') {
        let v: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        return match v.len() {
            3 | 4 => Some([v[0] * 17, v[1] * 17, v[2] * 17, 0xFF]),
            6 | 8 => Some([v[0] * 16 + v[1], v[2] * 16 + v[3], v[4] * 16 + v[5], 0xFF]),
            _ => None,
        };
    }
    if let Some(args) = t
        .strip_prefix("rgba(")
        .or_else(|| t.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let ch: Vec<u8> = args
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .take(3)
            .map(|p| {
                p.parse::<f32>()
                    .ok()
                    .map(|f| f.clamp(0.0, 255.0).round() as u8)
            })
            .collect::<Option<_>>()?;
        return (ch.len() == 3).then(|| [ch[0], ch[1], ch[2], 0xFF]);
    }
    let rgb = match t.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "silver" => [192, 192, 192],
        "gray" | "grey" => [128, 128, 128],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], 0xFF])
}

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
    if !clipped_at(doc, hit, x, y) {
        return Some(hit);
    }
    // Blitz returned scrolled-away content. Find the last element in document
    // order (paint-order approximation; z-index ignored) whose own box holds
    // the point and that is not clipped there; fall back to the root.
    let mut best = None;
    let mut stack = vec![doc.root_node().id];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in node.children.iter().rev() {
            stack.push(c);
        }
        if !node.is_element() || !hittable(node) {
            continue;
        }
        let inside = element_border_box_css(doc, nid)
            .is_some_and(|(bx, by, bw, bh)| x >= bx && y >= by && x < bx + bw && y < by + bh);
        if inside && !clipped_at(doc, nid, x, y) {
            best = Some(nid);
        }
    }
    best.or(Some(doc.root_node().id))
}

fn clipped_at(doc: &HtmlDocument, nid: NodeId, x: f32, y: f32) -> bool {
    scroll_ancestors(doc, nid)
        .into_iter()
        .any(|a| padding_box_css(doc, a).is_some_and(|pb| !pb.contains(x, y)))
}

fn hittable(node: &blitz_dom::Node) -> bool {
    use style::computed_values::pointer_events::T as Pe;
    use style::computed_values::visibility::T as Vis;
    node.primary_styles().is_none_or(|s| {
        s.clone_pointer_events() != Pe::None
            && !matches!(s.clone_visibility(), Vis::Hidden | Vis::Collapse)
    })
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
    /// Colours from the container's `--bg` / `--hi` / … tokens (issue #35);
    /// a token change damages the bar.
    pub pal: Palette,
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
        pal: palette_for(doc, nid),
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

/// Whether the topmost element at `(x, y)` is `nid` or inside it, i.e. no
/// later-painted element (a dialog, a tooltip) covers the bar there.
pub(crate) fn bar_unoccluded(doc: &HtmlDocument, nid: NodeId, x: f32, y: f32) -> bool {
    let Some(mut cur) = doc.element_from_point(x, y) else {
        return true;
    };
    loop {
        if cur == nid {
            return true;
        }
        match doc.get_node(cur).and_then(|n| n.parent) {
            Some(p) => cur = p,
            None => return false,
        }
    }
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
        let mut cv = Canvas::new(frame, scale, g.clip, g.pal);
        let pal = g.pal;
        let enabled = g.thumb.is_some();
        // Track: 1 px checker of --bg and --hi, phase from the track origin
        // (`repeating-conic-gradient(#fff 0 25%, #c0c0c0 0 50%) 0 0/2px 2px`).
        match pal.track {
            Some(c) => cv.fill(g.track, c),
            None => cv.checker(g.track),
        }
        for (part, rect) in [(BarPart::Up, g.up), (BarPart::Down, g.down)] {
            let is_pressed = pressed.is_some_and(|p| p.nid == g.nid && p.part == part);
            cv.fill(rect, pal.bg);
            if is_pressed {
                cv.press_bevel(rect);
            } else {
                cv.raise_bevel(rect);
            }
            let shift = if is_pressed { 1.0 } else { 0.0 };
            let up = part == BarPart::Up;
            if enabled {
                cv.arrow(rect, up, shift, pal.ink);
            } else {
                // Win95 disabled glyph: grey with a white emboss.
                cv.arrow(rect, up, 1.0, pal.hi);
                cv.arrow(rect, up, 0.0, pal.sh);
            }
        }
        if let Some(t) = g.thumb {
            cv.fill(t, pal.bg);
            cv.raise_bevel(t);
        }
    }
}

/// Device-pixel drawing in CSS-px rects, clipped to the surface and an
/// optional CSS clip rect.
pub(crate) struct Canvas<'a> {
    frame: &'a mut RgbaImage,
    scale: f64,
    clip: IntRect,
    pal: Palette,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(
        frame: &'a mut RgbaImage,
        scale: f64,
        clip: Option<ControlRect>,
        pal: Palette,
    ) -> Self {
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
        Self {
            frame,
            scale,
            clip,
            pal,
        }
    }

    pub(crate) fn device(&self, r: ControlRect) -> IntRect {
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

    pub(crate) fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        let di = ((y as u32 * self.frame.width + x as u32) * 4) as usize;
        self.frame.pixels[di..di + 4].copy_from_slice(&c);
    }

    pub(crate) fn fill(&mut self, r: ControlRect, c: [u8; 4]) {
        let d = self.device(r);
        for y in d.y..d.y + d.h {
            for x in d.x..d.x + d.w {
                self.put(x, y, c);
            }
        }
    }

    pub(crate) fn checker(&mut self, r: ControlRect) {
        let d = self.device(r);
        for y in d.y..d.y + d.h {
            for x in d.x..d.x + d.w {
                let lx = ((x as f64 + 0.5) / self.scale - r.x as f64).floor() as i64;
                let ly = ((y as f64 + 0.5) / self.scale - r.y as f64).floor() as i64;
                let c = if (lx + ly).rem_euclid(2) == 1 {
                    self.pal.hi
                } else {
                    self.pal.bg
                };
                self.put(x, y, c);
            }
        }
    }

    pub(crate) fn top(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(ControlRect { h: n, ..r }, c);
    }
    pub(crate) fn left(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(ControlRect { w: n, ..r }, c);
    }
    pub(crate) fn bottom(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
        self.fill(
            ControlRect {
                y: r.y + r.h - n,
                h: n,
                ..r
            },
            c,
        );
    }
    pub(crate) fn right(&mut self, r: ControlRect, n: f32, c: [u8; 4]) {
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
    pub(crate) fn raise_bevel(&mut self, r: ControlRect) {
        self.top(r, 2.0, self.pal.lt);
        self.left(r, 2.0, self.pal.lt);
        self.bottom(r, 2.0, self.pal.sh);
        self.right(r, 2.0, self.pal.sh);
        self.top(r, 1.0, self.pal.hi);
        self.left(r, 1.0, self.pal.hi);
        self.bottom(r, 1.0, self.pal.dk);
        self.right(r, 1.0, self.pal.dk);
    }

    /// `--press`: `inset -1px -1px hi, inset 1px 1px dk, inset -2px -2px panel,
    /// inset 2px 2px sh`.
    pub(crate) fn press_bevel(&mut self, r: ControlRect) {
        self.top(r, 2.0, self.pal.sh);
        self.left(r, 2.0, self.pal.sh);
        self.bottom(r, 2.0, self.pal.lt);
        self.right(r, 2.0, self.pal.lt);
        self.top(r, 1.0, self.pal.dk);
        self.left(r, 1.0, self.pal.dk);
        self.bottom(r, 1.0, self.pal.hi);
        self.right(r, 1.0, self.pal.hi);
    }

    /// `--sunken` / `--field`: `inset -1px -1px hi, inset 1px 1px sh,
    /// inset -2px -2px panel, inset 2px 2px dk`, painted last-listed first.
    pub(crate) fn sunken_bevel(&mut self, r: ControlRect) {
        self.top(r, 2.0, self.pal.dk);
        self.left(r, 2.0, self.pal.dk);
        self.bottom(r, 2.0, self.pal.lt);
        self.right(r, 2.0, self.pal.lt);
        self.top(r, 1.0, self.pal.sh);
        self.left(r, 1.0, self.pal.sh);
        self.bottom(r, 1.0, self.pal.hi);
        self.right(r, 1.0, self.pal.hi);
    }

    /// Win95 focus rectangle: 1 device px dotted line (every other pixel,
    /// phase from the rect origin) along the edges of `r`, in `--ink`.
    pub(crate) fn dotted_rect(&mut self, r: ControlRect) {
        let d = self.device(r);
        if d.w <= 0 || d.h <= 0 {
            return;
        }
        let ink = self.pal.ink;
        let (x0, y0, x1, y1) = (d.x, d.y, d.x + d.w - 1, d.y + d.h - 1);
        for x in x0..=x1 {
            if (x - x0) % 2 == 0 {
                self.put(x, y0, ink);
                self.put(x, y1, ink);
            }
        }
        for y in y0..=y1 {
            if (y - y0) % 2 == 0 {
                self.put(x0, y, ink);
                self.put(x1, y, ink);
            }
        }
    }

    /// Win95 arrow glyph: 4 rows of 1/3/5/7 px, centred in a 16x16 button.
    pub(crate) fn arrow(&mut self, r: ControlRect, up: bool, shift: f32, c: [u8; 4]) {
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

pub(crate) fn intersect_int(a: IntRect, b: IntRect) -> IntRect {
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
