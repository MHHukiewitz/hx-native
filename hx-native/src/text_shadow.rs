//! Interim zero-blur `text-shadow` paint (issue #57).
//!
//! Blitz 0.3.0-beta.4 computes `text-shadow` (stylo) but never paints it. Until
//! upstream paints it, hx-native draws zero-blur shadows itself, in paint order,
//! by wrapping the scene Blitz paints into ([`ShadowScene`]):
//!
//! 1. Before `paint_scene`, [`ShadowTable::build`] walks every inline layout.
//!    For each glyph run whose node has a non-empty computed `text-shadow`,
//!    it records a key over the run (font blob id and index, font size, glyph
//!    ids and positions) and the device origin Blitz will paint that inline
//!    root at.
//! 2. When Blitz calls `draw_glyphs` for a recorded run at that origin, the
//!    wrapper first draws the same glyphs once per shadow, offset by the
//!    shadow's `(x, y)` in the element's local space and filled with the
//!    shadow's computed colour, last shadow first, so the first shadow ends
//!    up on top (CSS order). Then it forwards the original run.
//!
//! Colours come from the author's computed `text-shadow` (`currentcolor`
//! resolves to the run's `color`), not from `--hx-*` variables: this is
//! author CSS, not engine chrome.
//!
//! Not drawn (documented limits): shadows with a non-zero blur radius (the
//! whole shadow is skipped, the others on the run still paint), and text
//! inside `<input>` / `<textarea>` editors (not an inline layout). A run whose
//! origin does not match the recorded one within half a device pixel fails
//! safe: it paints without a shadow.
//!
//! Pin-sensitive (BLITZ_PIN.md): relies on Blitz passing parley's positioned
//! glyph `x`/`y` and the run's `FontData` unchanged to `draw_glyphs`, and on
//! the inline-root transform `parent * translate(location) * transform *
//! translate(-scroll) * translate(padding + border - scroll + block_offset)`.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use anyrender::filters::Filter;
use anyrender::{PaintScene, RenderContext};
use blitz_dom::NodeId;
use blitz_html::HtmlDocument;
use kurbo::{Affine, Rect as KurboRect, Shape, Stroke, Vec2};
use parley::PositionedLayoutItem;
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};
use style::color::ColorSpace;

/// Max distance (device px) between the recorded and the painted run origin.
const ORIGIN_TOLERANCE: f64 = 0.5;

/// One zero-blur shadow, ready to paint: local device offset and colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowPaint {
    pub offset: Vec2,
    pub color: Color,
}

#[derive(Debug)]
struct Candidate {
    origin: (f64, f64),
    /// In CSS order (first = topmost).
    shadows: Arc<[ShadowPaint]>,
}

/// Shadowed glyph runs of one document at one paint scale.
#[derive(Debug, Default)]
pub(crate) struct ShadowTable {
    runs: HashMap<u64, Vec<Candidate>>,
}

fn run_key(font: &FontData, font_size: f32, glyphs: impl Iterator<Item = (u32, f32, f32)>) -> u64 {
    let mut h = DefaultHasher::new();
    font.data.id().hash(&mut h);
    font.index.hash(&mut h);
    font_size.to_bits().hash(&mut h);
    for (id, x, y) in glyphs {
        id.hash(&mut h);
        x.to_bits().hash(&mut h);
        y.to_bits().hash(&mut h);
    }
    h.finish()
}

/// Zero-blur shadows of a node's computed `text-shadow`, CSS order.
fn node_shadows(doc: &HtmlDocument, node_id: NodeId, scale: f64) -> Option<Arc<[ShadowPaint]>> {
    let node = doc.get_node(node_id)?;
    let styles = node.primary_styles()?;
    let itext = styles.get_inherited_text();
    let list = &itext.text_shadow.0;
    if list.is_empty() {
        return None;
    }
    let current = itext.color;
    let out: Vec<ShadowPaint> = list
        .iter()
        .filter(|s| s.blur.px() == 0.0)
        .map(|s| {
            let abs = s.color.resolve_to_absolute(&current);
            ShadowPaint {
                offset: Vec2::new(
                    s.horizontal.px() as f64 * scale,
                    s.vertical.px() as f64 * scale,
                ),
                color: Color::new(*abs.to_color_space(ColorSpace::Srgb).raw_components()),
            }
        })
        .collect();
    (!out.is_empty()).then(|| out.into())
}

impl ShadowTable {
    /// Record every shadowed run. Cheap when no node has a `text-shadow`:
    /// one style read per glyph run, no hashing.
    pub(crate) fn build(doc: &HtmlDocument, scale: f64) -> Self {
        let mut table = Self::default();
        let mut stack = vec![doc.root_node().id];
        while let Some(nid) = stack.pop() {
            let Some(node) = doc.get_node(nid) else {
                continue;
            };
            stack.extend(node.children.iter().copied());
            let Some(layout) = node
                .element_data()
                .and_then(|e| e.inline_layout_data.as_ref())
            else {
                continue;
            };
            let mut origin: Option<(f64, f64)> = None;
            for line in layout.layout.lines() {
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                        continue;
                    };
                    let Some(shadows) = node_shadows(doc, glyph_run.style().brush.id, scale) else {
                        continue;
                    };
                    let origin = match origin {
                        Some(o) => o,
                        None => match inline_origin(doc, nid, layout.block_offset, scale) {
                            Some(o) => *origin.insert(o),
                            None => break,
                        },
                    };
                    let run = glyph_run.run();
                    let key = run_key(
                        run.font(),
                        run.font_size(),
                        glyph_run.positioned_glyphs().map(|g| (g.id, g.x, g.y)),
                    );
                    table
                        .runs
                        .entry(key)
                        .or_default()
                        .push(Candidate { origin, shadows });
                }
            }
        }
        table
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    fn lookup(&self, key: u64, origin: (f64, f64)) -> Option<&[ShadowPaint]> {
        self.runs.get(&key)?.iter().find_map(|c| {
            ((c.origin.0 - origin.0).abs() <= ORIGIN_TOLERANCE
                && (c.origin.1 - origin.1).abs() <= ORIGIN_TOLERANCE)
                .then_some(&*c.shadows)
        })
    }
}

/// Device-space origin of the transform Blitz 0.3.0-beta.4 uses for an inline
/// root's glyph runs (`render.rs` `render_element` + `draw_inline_layout`).
fn inline_origin(
    doc: &HtmlDocument,
    nid: NodeId,
    block_offset: f32,
    scale: f64,
) -> Option<(f64, f64)> {
    let node = doc.get_node(nid)?;
    let xf = crate::damage::document_affine_device(doc, nid, scale)?;
    let l = node.final_layout();
    let scroll = node.scroll_offset();
    // Blitz moves the contents by the node's own scroll offset twice: once on
    // the element transform and once on the content position.
    let x = (l.padding.left + l.border.left) as f64 - 2.0 * scroll.x;
    let y = (l.padding.top + l.border.top) as f64 + block_offset as f64 - 2.0 * scroll.y;
    let p = (xf * Affine::translate((x * scale, y * scale))).translation();
    Some((p.x, p.y))
}

/// Scene adapter that paints recorded shadows right before their run.
pub(crate) struct ShadowScene<'a, S: PaintScene> {
    pub inner: &'a mut S,
    pub table: &'a ShadowTable,
}

impl<S: PaintScene> RenderContext for ShadowScene<'_, S> {
    fn try_register_custom_resource(
        &mut self,
        resource: Box<dyn std::any::Any>,
    ) -> Result<anyrender::ResourceId, anyrender::RegisterResourceError> {
        self.inner.try_register_custom_resource(resource)
    }

    fn unregister_resource(&mut self, resource_id: anyrender::ResourceId) {
        self.inner.unregister_resource(resource_id);
    }

    fn renderer_specific_context(&self) -> Option<Box<dyn std::any::Any>> {
        self.inner.renderer_specific_context()
    }
}

impl<S: PaintScene> PaintScene for ShadowScene<'_, S> {
    fn reset(&mut self) {
        self.inner.reset();
    }

    fn push_layer(
        &mut self,
        fill: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
    ) {
        self.inner
            .push_layer(fill, blend, alpha, transform, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.inner.push_clip_layer(fill, transform, clip);
    }

    fn pop_layer(&mut self) {
        self.inner.pop_layer();
    }

    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<anyrender::PaintRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.inner
            .stroke(style, transform, brush, brush_transform, shape);
    }

    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<anyrender::PaintRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.inner
            .fill(style, transform, brush, brush_transform, shape);
    }

    fn draw_glyphs<'b, 's: 'b>(
        &'s mut self,
        font: &'b FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'b [anyrender::NormalizedCoord],
        embolden: Vec2,
        style: impl Into<StyleRef<'b>>,
        brush: impl Into<anyrender::PaintRef<'b>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph> + Clone,
    ) {
        let style: StyleRef<'b> = style.into();
        if !self.table.is_empty() {
            let key = run_key(font, font_size, glyphs.clone().map(|g| (g.id, g.x, g.y)));
            let t = transform.translation();
            if let Some(shadows) = self.table.lookup(key, (t.x, t.y)) {
                // Last shadow first: the first one in CSS order ends on top.
                for s in shadows.iter().rev() {
                    self.inner.draw_glyphs(
                        font,
                        font_size,
                        hint,
                        normalized_coords,
                        embolden,
                        style,
                        &anyrender::Paint::from(s.color),
                        brush_alpha,
                        transform * Affine::translate(s.offset),
                        glyph_transform,
                        glyphs.clone(),
                    );
                }
            }
        }
        self.inner.draw_glyphs(
            font,
            font_size,
            hint,
            normalized_coords,
            embolden,
            style,
            brush,
            brush_alpha,
            transform,
            glyph_transform,
            glyphs,
        );
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: KurboRect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.inner
            .draw_box_shadow(transform, rect, brush, radius, std_dev);
    }
}

/// Extra ink (left, top, right, bottom, device px, all >= 0) that shadows of
/// the text an element paints add around its box, for damage (issue #57).
///
/// Covers the element's own inline layout and those of its anonymous-block
/// children (the text of a block with mixed content), reading each run's own
/// node: an inline `<b>` with a shadow inside a block without one still grows
/// the block's damage. Blurred shadows count too (offset + 1.5×blur), even
/// though they are not painted yet.
pub(crate) fn text_shadow_ink_pad(
    doc: &HtmlDocument,
    nid: NodeId,
    scale: f64,
) -> Option<(f64, f64, f64, f64)> {
    let node = doc.get_node(nid)?;
    let mut pad = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut grow = |styles: &style::properties::ComputedValues| {
        for s in styles.get_inherited_text().text_shadow.0.iter() {
            let sx = s.horizontal.px() as f64 * scale;
            let sy = s.vertical.px() as f64 * scale;
            let b = s.blur.px() as f64 * scale * 1.5;
            pad.0 = pad.0.max(b - sx);
            pad.1 = pad.1.max(b - sy);
            pad.2 = pad.2.max(b + sx);
            pad.3 = pad.3.max(b + sy);
        }
    };
    if let Some(styles) = node.primary_styles() {
        grow(&styles);
    }
    let layouts = std::iter::once(node).chain(
        node.children
            .iter()
            .filter_map(|&c| doc.get_node(c))
            .filter(|c| c.is_anonymous()),
    );
    for n in layouts {
        let Some(layout) = n.element_data().and_then(|e| e.inline_layout_data.as_ref()) else {
            continue;
        };
        let mut last = None;
        for line in layout.layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let id = run.style().brush.id;
                if last == Some(id) {
                    continue;
                }
                last = Some(id);
                if let Some(styles) = doc.get_node(id).and_then(|r| r.primary_styles()) {
                    grow(&styles);
                }
            }
        }
    }
    (pad != (0.0, 0.0, 0.0, 0.0)).then_some(pad)
}
