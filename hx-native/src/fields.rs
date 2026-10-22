//! Typed field look fixes (issue #35): `text-align` inside single-line
//! inputs and the full four-sided inset bevel (author `box-shadow`).
//!
//! * Blitz `=0.3.0-beta.4` builds a single-line input's `PlainEditor` with no
//!   width (`create_text_editor` → `set_width(None)`; only `<textarea>` gets
//!   its content width in layout), so parley has nothing to align against
//!   and `text-align: right | center | end` is ignored. After each resolve the
//!   engine hands such an editor its content-box width (and `nowrap`, which a
//!   single-line field means anyway). Start-aligned fields are left alone.
//! * Blitz's `draw_inset_box_shadow` paints inset shadows in list order, but
//!   CSS paints the first shadow on top. A multi-layer hard bevel such as
//!   `box-shadow: inset -1px -1px #fff, inset 1px 1px #888, inset -2px -2px
//!   #ddd, inset 2px 2px #444` therefore comes out as dark top/left only. For typed
//!   fields whose shadows are all hard insets (no blur / spread, opaque) the
//!   engine repaints those bands in CSS order over the Blitz raster.

use blitz_dom::NodeId;
use blitz_html::HtmlDocument;
use parley::style::TextWrapMode;
use parley::{Alignment, StyleProperty};
use style::color::ColorSpace;
use style::values::computed::text::TextAlign;
use style::values::specified::TextAlignKeyword;

use crate::damage::element_border_box_css;
use crate::scroll::{ancestor_clip_css, Canvas, Palette};
use crate::select_ui::ControlRect;
use crate::text_edit::is_editable_input;
use crate::types::RgbaImage;

fn typed_fields(doc: &HtmlDocument) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &c in node.children.iter().rev() {
            stack.push(c);
        }
        if node.is_element() && is_editable_input(node) {
            out.push(nid);
        }
    }
    out
}

/// Parley alignment for a computed `text-align` that needs a container
/// width; `None` for start-like values (left in LTR), which need none.
fn needs_width(align: TextAlign) -> Option<Alignment> {
    match align {
        TextAlignKeyword::Right | TextAlignKeyword::MozRight => Some(Alignment::Right),
        TextAlignKeyword::End => Some(Alignment::End),
        TextAlignKeyword::Center | TextAlignKeyword::MozCenter => Some(Alignment::Center),
        _ => None,
    }
}

/// Give every single-line typed field with a non-start `text-align` its
/// content-box width so parley can align the text (and the caret) in it.
/// Run after every `resolve`.
pub(crate) fn align_text_inputs(doc: &mut HtmlDocument) {
    let scale = doc.viewport().scale_f64() as f32;
    let mut todo: Vec<(NodeId, f32, Alignment)> = Vec::new();
    for nid in typed_fields(doc) {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        let single_line = node
            .element_data()
            .and_then(|el| el.text_input_data())
            .is_some_and(|t| !t.is_multiline);
        if !single_line {
            continue;
        }
        let Some(align) = node
            .primary_styles()
            .and_then(|s| needs_width(s.clone_text_align()))
        else {
            continue;
        };
        let l = node.final_layout();
        let content_w =
            l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right;
        todo.push((nid, content_w.max(0.0) * scale, align));
    }
    for (nid, width, align) in todo {
        doc.with_text_input(nid, |mut driver| {
            driver
                .editor
                .edit_styles()
                .insert(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
            driver.editor.set_alignment(align);
            driver.editor.set_width(Some(width));
            driver.refresh_layout();
        });
    }
}

/// One hard inset shadow: offset (CSS px) and opaque sRGB colour.
struct HardInset {
    dx: f32,
    dy: f32,
    color: [u8; 4],
}

/// The node's inset shadows when every one of them is hard (no blur, no
/// spread, opaque) and there are no outset ones; `None` otherwise (Blitz's
/// own paint is left as is).
fn hard_insets(doc: &HtmlDocument, nid: NodeId) -> Option<Vec<HardInset>> {
    let node = doc.get_node(nid)?;
    let style = node.primary_styles()?;
    let shadows = &style.get_effects().box_shadow.0;
    if shadows.is_empty() {
        return None;
    }
    // The replay paints square bands; a rounded field keeps Blitz's paint.
    let b = style.get_border();
    let basis = style::values::computed::CSSPixelLength::new(1.0);
    if [
        &b.border_top_left_radius,
        &b.border_top_right_radius,
        &b.border_bottom_right_radius,
        &b.border_bottom_left_radius,
    ]
    .iter()
    .any(|r| r.0.width.0.resolve(basis).px() != 0.0 || r.0.height.0.resolve(basis).px() != 0.0)
    {
        return None;
    }
    let current = style.clone_color();
    let mut out = Vec::with_capacity(shadows.len());
    for s in shadows.iter() {
        if !s.inset || s.base.blur.px() != 0.0 || s.spread.px() != 0.0 {
            return None;
        }
        let c = s
            .base
            .color
            .resolve_to_absolute(&current)
            .to_color_space(ColorSpace::Srgb);
        let [r, g, b, a] = *c.raw_components();
        if a < 1.0 {
            return None;
        }
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        out.push(HardInset {
            dx: s.base.horizontal.px(),
            dy: s.base.vertical.px(),
            color: [q(r), q(g), q(b), 255],
        });
    }
    Some(out)
}

/// Repaint hard inset bevels of typed fields in CSS order (first shadow on
/// top). Same pixels every frame, so a partial repaint equals a full one.
pub(crate) fn paint_field_bevels(doc: &HtmlDocument, frame: &mut RgbaImage, scale: f64) {
    for nid in typed_fields(doc) {
        let Some(shadows) = hard_insets(doc, nid) else {
            continue;
        };
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if node.primary_styles().is_some_and(|s| {
            s.get_inherited_box().visibility != style::computed_values::visibility::T::Visible
        }) {
            continue;
        }
        let Some((bx, by, bw, bh)) = element_border_box_css(doc, nid) else {
            continue;
        };
        let b = node.final_layout().border;
        let pad = ControlRect {
            x: bx + b.left,
            y: by + b.top,
            w: (bw - b.left - b.right).max(0.0),
            h: (bh - b.top - b.bottom).max(0.0),
        };
        if pad.w <= 0.0 || pad.h <= 0.0 {
            continue;
        }
        let mut cv = Canvas::new(
            frame,
            scale,
            ancestor_clip_css(doc, nid),
            Palette::default(),
        );
        // Painted last-listed first so the first shadow ends on top. A hard
        // inset with offset (dx, dy) shows as the bands the shifted hole
        // leaves uncovered: top/left for positive offsets, bottom/right for
        // negative ones.
        for s in shadows.iter().rev() {
            if s.dy > 0.0 {
                cv.top(pad, s.dy.min(pad.h), s.color);
            } else if s.dy < 0.0 {
                cv.bottom(pad, (-s.dy).min(pad.h), s.color);
            }
            if s.dx > 0.0 {
                cv.left(pad, s.dx.min(pad.w), s.color);
            } else if s.dx < 0.0 {
                cv.right(pad, (-s.dx).min(pad.w), s.color);
            }
        }
    }
}
