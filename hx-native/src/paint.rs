//! CPU paint via `anyrender_vello_cpu` (G1 path — no wgpu).
//!
//! Full-surface paint, plus the probe's exact / shifted partial-raster paths.

use std::sync::Arc;

use anyrender::filters::Filter;
use anyrender::{render_to_buffer, PaintScene, RenderContext};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use kurbo::{Affine, Rect as KurboRect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};

use crate::damage::IntRect;
use crate::error::EngineError;
use crate::inset_shadow::InsetScene;
use crate::text_shadow::{ShadowScene, ShadowTable};
use crate::tile_coalesce::TileScene;
use crate::types::{PaintMode, RgbaImage};

pub fn paint_rgba(
    doc: &mut HtmlDocument,
    width: u32,
    height: u32,
    scale: f64,
) -> Result<RgbaImage, EngineError> {
    if width == 0 || height == 0 {
        return Err(EngineError::Paint("viewport size must be non-zero".into()));
    }
    let table = ShadowTable::build(doc, scale);
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            let mut tiles = TileScene::new(scene);
            {
                let mut inset = InsetScene::new(&mut tiles);
                {
                    let mut shadow = ShadowScene {
                        inner: &mut inset,
                        table: &table,
                    };
                    paint_scene(&mut shadow, doc, scale, width, height, 0, 0);
                }
                inset.finish();
            }
            tiles.finish();
        },
        width,
        height,
    );
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Rasterize `damage` into `frame` (full surface buffer), using Exact or Shifted.
pub fn paint_damage_into(
    doc: &mut HtmlDocument,
    frame: &mut RgbaImage,
    scale: f64,
    damage: &[IntRect],
    mode: PaintMode,
) -> Result<(), EngineError> {
    let width = frame.width;
    let height = frame.height;
    if width == 0 || height == 0 {
        return Err(EngineError::Paint("viewport size must be non-zero".into()));
    }
    // Issue #57: one shadow table for every rect of this paint.
    let table = ShadowTable::build(doc, scale);
    for rect in damage {
        if rect.is_empty() {
            continue;
        }
        match mode {
            PaintMode::Exact => paint_exact_rect(doc, frame, scale, *rect, &table)?,
            PaintMode::Shifted => paint_shifted_rect(doc, frame, scale, *rect, &table)?,
        }
    }
    Ok(())
}

/// Exact method: cull + full-size clip layer, render W×H, copy the rect.
fn paint_exact_rect(
    doc: &mut HtmlDocument,
    frame: &mut RgbaImage,
    scale: f64,
    rect: IntRect,
    table: &ShadowTable,
) -> Result<(), EngineError> {
    let width = frame.width;
    let height = frame.height;
    let clip = KurboRect::new(
        rect.x as f64,
        rect.y as f64,
        (rect.x + rect.w) as f64,
        (rect.y + rect.h) as f64,
    );
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            let mut cull = CullScene {
                inner: scene,
                clip,
                shift: None,
                skip_depth: 0,
            };
            // Full-size clip so fragments outside the rect cannot leak.
            PaintScene::push_clip_layer(&mut cull, Fill::NonZero, Affine::IDENTITY, &clip);
            // Shadows are emitted above the tile merge, inset shadow layers are
            // folded and tile runs are merged before culling, so full and
            // partial paints see the same commands (issues #57, #67).
            let mut tiles = TileScene::new(&mut cull);
            {
                let mut inset = InsetScene::new(&mut tiles);
                {
                    let mut shadow = ShadowScene {
                        inner: &mut inset,
                        table,
                    };
                    paint_scene(&mut shadow, doc, scale, width, height, 0, 0);
                }
                inset.finish();
            }
            tiles.finish();
            PaintScene::pop_layer(&mut cull);
        },
        width,
        height,
    );
    copy_rect_from_full(&pixels, width, frame, rect);
    Ok(())
}

/// Shifted method: translate(-x,-y), cull, render into a rect-sized target, copy rows.
fn paint_shifted_rect(
    doc: &mut HtmlDocument,
    frame: &mut RgbaImage,
    scale: f64,
    rect: IntRect,
    table: &ShadowTable,
) -> Result<(), EngineError> {
    let width = frame.width;
    let height = frame.height;
    let rw = rect.w as u32;
    let rh = rect.h as u32;
    if rw == 0 || rh == 0 {
        return Ok(());
    }
    let clip = KurboRect::new(
        rect.x as f64,
        rect.y as f64,
        (rect.x + rect.w) as f64,
        (rect.y + rect.h) as f64,
    );
    let shift = Affine::translate(Vec2::new(-(rect.x as f64), -(rect.y as f64)));
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            let mut cull = CullScene {
                inner: scene,
                clip,
                shift: Some(shift),
                skip_depth: 0,
            };
            let mut tiles = TileScene::new(&mut cull);
            {
                let mut inset = InsetScene::new(&mut tiles);
                {
                    let mut shadow = ShadowScene {
                        inner: &mut inset,
                        table,
                    };
                    paint_scene(&mut shadow, doc, scale, width, height, 0, 0);
                }
                inset.finish();
            }
            tiles.finish();
        },
        rw,
        rh,
    );
    copy_rows_into(frame, rect, &pixels);
    Ok(())
}

fn copy_rect_from_full(src: &[u8], src_w: u32, dst: &mut RgbaImage, rect: IntRect) {
    let x0 = rect.x.max(0) as u32;
    let y0 = rect.y.max(0) as u32;
    let x1 = ((rect.x + rect.w) as u32).min(dst.width).min(src_w);
    let y1 = ((rect.y + rect.h) as u32).min(dst.height);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_bytes = ((x1 - x0) * 4) as usize;
    for y in y0..y1 {
        let src_off = ((y * src_w + x0) * 4) as usize;
        let dst_off = ((y * dst.width + x0) * 4) as usize;
        dst.pixels[dst_off..dst_off + row_bytes]
            .copy_from_slice(&src[src_off..src_off + row_bytes]);
    }
}

fn copy_rows_into(dst: &mut RgbaImage, rect: IntRect, src: &[u8]) {
    let x0 = rect.x.max(0) as u32;
    let y0 = rect.y.max(0) as u32;
    let rw = rect.w as u32;
    let rh = rect.h as u32;
    let x1 = (x0 + rw).min(dst.width);
    let y1 = (y0 + rh).min(dst.height);
    let copy_w = x1.saturating_sub(x0);
    if copy_w == 0 || y0 >= y1 {
        return;
    }
    let row_bytes = (copy_w * 4) as usize;
    for row in 0..(y1 - y0) {
        let src_off = (row * rw * 4) as usize;
        let dst_off = (((y0 + row) * dst.width + x0) * 4) as usize;
        dst.pixels[dst_off..dst_off + row_bytes]
            .copy_from_slice(&src[src_off..src_off + row_bytes]);
    }
}

/// PaintScene adapter: drops commands whose bounds miss `clip`, optionally
/// prepending a translation (shifted path). Layers with filters are never dropped.
struct CullScene<'a, S: PaintScene> {
    inner: &'a mut S,
    clip: KurboRect,
    shift: Option<Affine>,
    /// When > 0 we are inside a culled layer and must swallow matching pops.
    skip_depth: u32,
}

impl<'a, S: PaintScene> CullScene<'a, S> {
    fn xf(&self, transform: Affine) -> Affine {
        match self.shift {
            Some(s) => s * transform,
            None => transform,
        }
    }

    fn misses(&self, transform: Affine, bounds: KurboRect) -> bool {
        let bb = transform.transform_rect_bbox(bounds);
        !bb.overlaps(self.clip)
    }

    fn shape_misses(&self, transform: Affine, shape: &impl Shape) -> bool {
        self.misses(transform, shape.bounding_box())
    }
}

impl<'a, S: PaintScene> RenderContext for CullScene<'a, S> {
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

impl<'a, S: PaintScene> PaintScene for CullScene<'a, S> {
    fn reset(&mut self) {
        self.inner.reset();
        self.skip_depth = 0;
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
        if self.skip_depth > 0 {
            self.skip_depth += 1;
            return;
        }
        let xf = self.xf(transform);
        // Layers with filters are never dropped (probe rule).
        let has_filter = filter.is_some() || backdrop_filter.is_some();
        if !has_filter && self.shape_misses(xf, clip) {
            self.skip_depth = 1;
            return;
        }
        self.inner
            .push_layer(fill, blend, alpha, xf, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        if self.skip_depth > 0 {
            self.skip_depth += 1;
            return;
        }
        let xf = self.xf(transform);
        if self.shape_misses(xf, clip) {
            self.skip_depth = 1;
            return;
        }
        self.inner.push_clip_layer(fill, xf, clip);
    }

    fn pop_layer(&mut self) {
        if self.skip_depth > 0 {
            self.skip_depth -= 1;
            return;
        }
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
        if self.skip_depth > 0 {
            return;
        }
        let xf = self.xf(transform);
        // Inflate bounds by stroke width for cull test.
        let pad = style.width;
        let bb = shape.bounding_box().inflate(pad, pad);
        if self.misses(xf, bb) {
            return;
        }
        let brush_xf = brush_transform.map(|b| self.xf(b));
        self.inner.stroke(style, xf, brush, brush_xf, shape);
    }

    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<anyrender::PaintRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        if self.skip_depth > 0 {
            return;
        }
        let xf = self.xf(transform);
        if self.shape_misses(xf, shape) {
            return;
        }
        let brush_xf = brush_transform.map(|b| self.xf(b));
        self.inner.fill(style, xf, brush, brush_xf, shape);
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
        if self.skip_depth > 0 {
            return;
        }
        let xf = self.xf(transform);
        // Approximate glyph-run bounds from glyph positions + font size.
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;
        for g in glyphs.clone() {
            any = true;
            let gx = g.x as f64;
            let gy = g.y as f64;
            min_x = min_x.min(gx);
            min_y = min_y.min(gy - font_size as f64);
            max_x = max_x.max(gx + font_size as f64);
            max_y = max_y.max(gy + font_size as f64 * 0.3);
        }
        if any {
            let bb = KurboRect::new(min_x, min_y, max_x, max_y);
            if self.misses(xf, bb) {
                return;
            }
        }
        let glyph_xf = glyph_transform.map(|g| self.xf(g));
        self.inner.draw_glyphs(
            font,
            font_size,
            hint,
            normalized_coords,
            embolden,
            style,
            brush,
            brush_alpha,
            xf,
            glyph_xf,
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
        if self.skip_depth > 0 {
            return;
        }
        let xf = self.xf(transform);
        let pad = std_dev * 3.0 + radius;
        let bb = rect.inflate(pad, pad);
        if self.misses(xf, bb) {
            return;
        }
        self.inner.draw_box_shadow(xf, rect, brush, radius, std_dev);
    }
}

/// Re-export so engine can reuse a renderer across full paints if needed later.
#[allow(dead_code)]
pub(crate) type CpuRenderer = VelloCpuImageRenderer;
