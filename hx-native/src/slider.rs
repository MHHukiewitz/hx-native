//! Engine-drawn range slider (issue #35); colours and sizes from `--hx-*`.
//!
//! Blitz `=0.3.0-beta.4` paints no track or thumb for `<input type=range>`
//! (blitz-paint only draws checkbox / radio controls), and Stylo's servo
//! selector parser rejects `::-webkit-slider-*`, so an author's slider CSS
//! never applies. The engine draws the shape instead: a sunken groove across
//! the content box and a raised thumb (content-box height) placed from
//! `value` / `min` / `max`, over the Blitz raster after paint (like the #26
//! scrollbars and the #14 select face). Colours come from the `--hx-*`
//! variables, thumb width and groove height from `--hx-thumb-w` /
//! `--hx-groove-h` (defaults 11 px / 4 px). Pseudo-element CSS is ignored.
//!
//! The value ↔ x mapping lives here too, so the pointer path and the painter
//! can never disagree: the thumb's centre sits on the value, and a pointer x
//! maps back through that same centre.

use blitz_dom::{NodeId, UserAgentStylesheet};
use blitz_html::HtmlDocument;
use style::computed_values::visibility::T as Visibility;

use crate::damage::{element_border_box_css, IntRect};
use crate::form_args::{is_range_input, numeric_limits, parse_f64};
use crate::form_controls::control_is_disabled;
use crate::scroll::{ancestor_clip_css, hx_px, intersect, palette_for, Canvas, Palette};
use crate::select_ui::ControlRect;
use crate::types::RgbaImage;

/// Default thumb width in CSS px (`--hx-thumb-w`).
pub(crate) const THUMB_W: f32 = 11.0;
/// Default groove height in CSS px (`--hx-groove-h`).
pub(crate) const TRACK_H: f32 = 4.0;

/// UA rules: drop Blitz's empty input box under the engine-drawn slider and
/// give an unstyled range a usable default size. Author CSS still wins.
pub fn slider_ua_stylesheet() -> UserAgentStylesheet {
    UserAgentStylesheet::parse(
        r#"
input[type="range" i] {
    appearance: none;
    box-sizing: border-box;
    width: 129px;
    height: 20px;
    border: 0;
    padding: 0;
    background: transparent;
    vertical-align: middle;
}
input[type="range" i]:focus {
    outline: none;
}
"#,
    )
}

/// One slider's painted geometry (CSS document px). Compared frame to frame
/// to damage only sliders that changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SliderGeom {
    pub nid: NodeId,
    /// Content box: the track spans it, the thumb travels inside it.
    pub content: ControlRect,
    pub track: ControlRect,
    pub thumb: ControlRect,
    pub disabled: bool,
    /// Scroll-container clip (`None` = unclipped).
    pub clip: Option<ControlRect>,
    /// Focused: dotted focus rectangle around the content box.
    pub focused: bool,
    /// Colours from the `--hx-*` variables computed on the input.
    pub pal: Palette,
}

impl SliderGeom {
    /// Everything this slider paints (content box ∪ thumb), CSS px.
    pub fn paint_rect(&self) -> ControlRect {
        let x0 = self.content.x.min(self.thumb.x);
        let y0 = self.content.y.min(self.thumb.y);
        let x1 = (self.content.x + self.content.w).max(self.thumb.x + self.thumb.w);
        let y1 = (self.content.y + self.content.h).max(self.thumb.y + self.thumb.h);
        let r = ControlRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        };
        match self.clip {
            Some(c) => intersect(r, c),
            None => r,
        }
    }

    /// Device-px damage rect for this slider.
    pub fn damage_rect(&self, scale: f64) -> IntRect {
        self.paint_rect().as_device(scale)
    }

    pub fn thumb_contains(&self, x: f32, y: f32) -> bool {
        self.thumb.contains(x, y)
    }

    /// Left edge of the thumb's travel and its length (CSS px).
    fn travel(&self) -> (f32, f32) {
        (self.content.x, (self.content.w - self.thumb.w).max(0.0))
    }
}

/// `(value - min) / (max - min)` clamped to `0..=1` (0 when the range is
/// empty). The value falls back to `min` when missing or unparseable.
pub(crate) fn value_ratio(doc: &HtmlDocument, nid: NodeId) -> Option<f64> {
    let node = doc.get_node(nid)?;
    if !is_range_input(node) {
        return None;
    }
    let limits = numeric_limits(node);
    let min = limits.min.unwrap_or(0.0);
    let max = limits.max.unwrap_or(100.0).max(min);
    let value = range_value(node, &limits);
    if max - min <= 0.0 {
        return Some(0.0);
    }
    Some(((value - min) / (max - min)).clamp(0.0, 1.0))
}

/// A range's `value` attribute, or `min` when missing or unparseable.
pub(crate) fn range_value(node: &blitz_dom::Node, limits: &crate::form_args::NumericLimits) -> f64 {
    node.attr(blitz_dom::local_name!("value"))
        .and_then(parse_f64)
        .filter(|v| v.is_finite())
        .unwrap_or(limits.min.unwrap_or(0.0))
}

/// Slider geometry for a rendered, visible range input.
pub(crate) fn slider_geometry(doc: &HtmlDocument, nid: NodeId) -> Option<SliderGeom> {
    let node = doc.get_node(nid)?;
    if !is_range_input(node) {
        return None;
    }
    let style = node.primary_styles()?;
    if style.get_inherited_box().visibility != Visibility::Visible {
        return None;
    }
    let (bx, by, bw, bh) = element_border_box_css(doc, nid)?;
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let l = node.final_layout();
    let (b, p) = (l.border, l.padding);
    let content = ControlRect {
        x: bx + b.left + p.left,
        y: by + b.top + p.top,
        w: (bw - b.left - b.right - p.left - p.right).max(0.0),
        h: (bh - b.top - b.bottom - p.top - p.bottom).max(0.0),
    };
    let ratio = value_ratio(doc, nid)? as f32;
    // Sizes come from CSS: the thumb is as tall as the content box (author
    // height / padding), its width and the groove height are `--hx-thumb-w`
    // and `--hx-groove-h` (defaults 11 px and 4 px).
    let thumb_w = hx_px(doc, nid, "hx-thumb-w")
        .unwrap_or(THUMB_W)
        .min(content.w);
    let groove_h = hx_px(doc, nid, "hx-groove-h")
        .unwrap_or(TRACK_H)
        .min(content.h);
    let travel = (content.w - thumb_w).max(0.0);
    let thumb = ControlRect {
        x: content.x + (ratio * travel).round(),
        y: content.y,
        w: thumb_w,
        h: content.h,
    };
    let track = ControlRect {
        x: content.x,
        y: content.y + ((content.h - groove_h) / 2.0).floor(),
        w: content.w,
        h: groove_h,
    };
    Some(SliderGeom {
        nid,
        content,
        track,
        thumb,
        disabled: control_is_disabled(doc, nid),
        clip: ancestor_clip_css(doc, nid),
        focused: node.is_focussed(),
        pal: palette_for(doc, nid),
    })
}

/// Every visible slider, in document order.
pub(crate) fn all_sliders(doc: &HtmlDocument) -> Vec<SliderGeom> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in node.children.iter().rev() {
            stack.push(c);
        }
        if node.is_element() {
            if let Some(g) = slider_geometry(doc, nid) {
                out.push(g);
            }
        }
    }
    out
}

/// Ratio (0..=1) for pointer `x` (document CSS px) when the thumb is held
/// `grab` px right of its centre: the thumb centre follows the pointer.
pub(crate) fn ratio_for_x(g: &SliderGeom, x: f32, grab: f32) -> f64 {
    let (left, travel) = g.travel();
    if travel <= 0.0 {
        return 0.0;
    }
    (((x - grab) - left - g.thumb.w / 2.0) / travel).clamp(0.0, 1.0) as f64
}

/// Pointer offset from the thumb centre when the press lands on the thumb
/// (the thumb is grabbed in place), else 0 (the thumb jumps under the pointer).
pub(crate) fn grab_offset(g: &SliderGeom, x: f32, y: f32) -> f32 {
    if g.thumb_contains(x, y) {
        x - (g.thumb.x + g.thumb.w / 2.0)
    } else {
        0.0
    }
}

/// Paint every visible slider over the Blitz raster. Opaque and identical
/// every frame, so a partial repaint equals a full one.
pub(crate) fn paint_sliders(doc: &HtmlDocument, frame: &mut RgbaImage, scale: f64) {
    for g in all_sliders(doc) {
        let mut cv = Canvas::new(frame, scale, g.clip, g.pal);
        // Groove: sunken bevel, `--hx-groove-h` tall.
        cv.sunken_bevel(g.track);
        // Focus: dotted `--hx-ink` rectangle on the content box, under the thumb.
        if g.focused {
            cv.dotted_rect(g.content);
        }
        // Thumb: `--hx-face` + raised bevel; disabled = `--hx-face` /
        // `--hx-bevel-hi` checker.
        if g.disabled {
            cv.checker(g.thumb, g.pal.face, g.pal.hi);
        } else {
            cv.fill(g.thumb, g.pal.face);
        }
        cv.raise_bevel(g.thumb);
    }
}
