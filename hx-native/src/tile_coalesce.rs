//! Merge Blitz's per-tile gradient fills into one repeated fill (issue #67).
//!
//! Blitz 0.3.0-beta.4 paints a tiled gradient background (`background-size`
//! smaller than the box, e.g. the 2×2 px `--dither` checker) as one
//! `fill` per tile (`blitz-paint` `render/background.rs`,
//! `draw_gradient_layer`). vello_cpu builds a fresh gradient lookup table for
//! every one of those fills, so a 43×20 px dither box (220 tiles) costs about
//! 300 ms per debug frame.
//!
//! [`TileScene`] sits between Blitz and the renderer. It buffers a run of
//! fills that share one gradient, fill rule, brush transform and tile rect,
//! whose transforms are pure translations stepping row-major over the tile
//! grid (Blitz's loop order). When the run ends it rasterizes one tile with
//! vello_cpu (cached), and paints the whole run as one fill of the run's
//! rect with that tile as a repeating, nearest-sampled image anchored at the
//! first tile. The pixels are vello's own; nothing is drawn that Blitz did
//! not ask for.
//!
//! Exactness rules (otherwise the run is replayed tile by tile, unchanged):
//! - the tiles' linear transform is the identity and every tile origin and
//!   the tile size are whole device pixels (no anti-aliased tile edges, and
//!   nearest sampling lands on the same pixel);
//! - every pixel of the rendered tile has alpha 0 or 255, so the image draw
//!   composites exactly like the gradient draw.
//!
//! One known difference: a conic tile whose centre lands exactly on a pixel
//! centre (odd tile size in device px, e.g. the 2 px dither at scale 1.5).
//! vello_cpu's angle at that pixel is float noise that depends on where the
//! tile sits, so Blitz's per-tile path paints an occasional centre pixel in
//! the other colour (1 of 450 tiles in Graph's probe). The merged fill gives
//! every tile the origin tile's centre pixel.
//!
//! Pin-sensitive: depends on how Blitz emits tile fills (see BLITZ_PIN.md).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use anyrender::filters::Filter;
use anyrender::{render_to_buffer, PaintRef, PaintScene, RenderContext};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use kurbo::{Affine, Rect, Shape, Stroke, Vec2};
use peniko::{
    BlendMode, Blob, Color, Extend, Fill, FontData, Gradient, ImageAlphaType, ImageBrush,
    ImageData, ImageFormat, ImageQuality, StyleRef,
};

/// Bounded number of cached tiles (cleared wholesale when full).
const CACHE_CAP: usize = 64;
/// Bounded cached pixel bytes per thread (cleared wholesale when a new tile
/// would go over). Without it 64 tiles of 256x256 px hold 16 MiB per painting
/// thread, outside the surface memory budget.
const CACHE_MAX_BYTES: usize = 1 << 20;
/// Largest tile (device px per side) worth rasterizing separately.
const MAX_TILE_SIDE: f64 = 256.0;
const EPS: f64 = 1e-6;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static CACHE: RefCell<HashMap<String, Option<Arc<ImageData>>>> = RefCell::new(HashMap::new());
    static CACHE_BYTES: Cell<usize> = const { Cell::new(0) };
    static COUNTERS: Cell<TileCoalesceCounters> = const { Cell::new(TileCoalesceCounters::ZERO) };
}

/// Counters for tests and benches (per thread, since the last reset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileCoalesceCounters {
    /// Runs painted as one repeated fill.
    pub runs_merged: u64,
    /// Tiles covered by those merged runs.
    pub tiles_merged: u64,
    /// Gradient fills forwarded to the renderer unchanged.
    pub gradient_fills_forwarded: u64,
    /// Tile rasters made (cache misses).
    pub tile_rasters: u64,
}

impl TileCoalesceCounters {
    const ZERO: Self = Self {
        runs_merged: 0,
        tiles_merged: 0,
        gradient_fills_forwarded: 0,
        tile_rasters: 0,
    };
}

fn bump(f: impl FnOnce(&mut TileCoalesceCounters)) {
    COUNTERS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// Test hook: turn the merge off (per thread) to get Blitz's per-tile path.
#[doc(hidden)]
pub fn set_tile_coalesce_for_test(enabled: bool) {
    ENABLED.with(|e| e.set(enabled));
}

/// Test hook: read and reset the per-thread counters.
#[doc(hidden)]
pub fn take_tile_coalesce_counters_for_test() -> TileCoalesceCounters {
    COUNTERS.with(|c| c.replace(TileCoalesceCounters::ZERO))
}

/// Test hook: drop cached tiles (per thread).
#[doc(hidden)]
pub fn clear_tile_cache_for_test() {
    CACHE.with(|c| c.borrow_mut().clear());
    CACHE_BYTES.with(|b| b.set(0));
}

struct Run {
    style: Fill,
    gradient: Gradient,
    brush_xf: Option<Affine>,
    tile: Rect,
    /// Linear part shared by every tile (translation zeroed).
    linear: Affine,
    /// Translation of the first tile.
    origin: Vec2,
    /// Every tile transform as received, for an unchanged replay.
    transforms: Vec<Affine>,
    /// Tiles per row once the second row starts.
    cols: Option<usize>,
}

impl Run {
    /// Where the `n`th tile of a row-major run sits.
    fn expected(&self, n: usize) -> Vec2 {
        let (w, h) = (self.tile.width(), self.tile.height());
        match self.cols {
            Some(c) => self.origin + Vec2::new((n % c) as f64 * w, (n / c) as f64 * h),
            None => self.origin + Vec2::new(n as f64 * w, 0.0),
        }
    }

    /// Accept `t` as the next tile, if it is the next grid position.
    fn accepts(&mut self, t: Affine) -> bool {
        if !same_linear(t, self.linear) {
            return false;
        }
        let pos = t.translation();
        let n = self.transforms.len();
        if close(pos, self.expected(n)) {
            return true;
        }
        // First tile of the second row fixes the column count.
        if self.cols.is_none() && n > 0 {
            let e = self.origin + Vec2::new(0.0, self.tile.height());
            if close(pos, e) {
                self.cols = Some(n);
                return true;
            }
        }
        false
    }

    fn rows_cols(&self) -> Option<(usize, usize)> {
        let n = self.transforms.len();
        let c = self.cols.unwrap_or(n);
        (c > 0 && n.is_multiple_of(c)).then_some((n / c, c))
    }
}

fn close(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS
}

fn same_linear(a: Affine, b: Affine) -> bool {
    let (a, b) = (a.as_coeffs(), b.as_coeffs());
    a[..4].iter().zip(&b[..4]).all(|(x, y)| (x - y).abs() < EPS)
}

fn linear_of(t: Affine) -> Affine {
    let c = t.as_coeffs();
    Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
}

fn whole(v: f64) -> bool {
    (v - v.round()).abs() < EPS
}

fn cache_key(style: Fill, gradient: &Gradient, brush_xf: Option<Affine>, tile: Rect) -> String {
    format!(
        "{style:?}|{gradient:?}|{:?}|{}x{}",
        brush_xf.map(|a| a.as_coeffs()),
        tile.width(),
        tile.height()
    )
}

/// Rasterize one tile at the origin with vello_cpu, or `None` when it
/// cannot be drawn back exactly (a pixel with partial alpha).
fn rasterize_tile(
    style: Fill,
    gradient: &Gradient,
    brush_xf: Option<Affine>,
    tile: Rect,
) -> Option<Arc<ImageData>> {
    let (w, h) = (tile.width().round() as u32, tile.height().round() as u32);
    bump(|c| c.tile_rasters += 1);
    let pixels = render_to_buffer::<VelloCpuImageRenderer, _>(
        |s| {
            s.fill(
                style,
                Affine::IDENTITY,
                PaintRef::Gradient(gradient),
                brush_xf,
                &Rect::new(0.0, 0.0, tile.width(), tile.height()),
            );
        },
        w,
        h,
    );
    if pixels.chunks_exact(4).any(|p| p[3] != 0 && p[3] != 255) {
        return None;
    }
    Some(Arc::new(ImageData {
        data: Blob::from(pixels),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::AlphaPremultiplied,
        width: w,
        height: h,
    }))
}

fn cached_tile(run: &Run) -> Option<Arc<ImageData>> {
    let key = cache_key(run.style, &run.gradient, run.brush_xf, run.tile);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let made = rasterize_tile(run.style, &run.gradient, run.brush_xf, run.tile);
    let size = made.as_ref().map_or(0, |i| i.data.len());
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let used = CACHE_BYTES.with(|b| b.get());
        if c.len() >= CACHE_CAP || used + size > CACHE_MAX_BYTES {
            c.clear();
            CACHE_BYTES.with(|b| b.set(0));
        }
        c.insert(key, made.clone());
        CACHE_BYTES.with(|b| b.set(b.get() + size));
    });
    made
}

/// PaintScene adapter that merges Blitz's gradient tile runs (issue #67).
pub(crate) struct TileScene<'a, S: PaintScene> {
    inner: &'a mut S,
    run: Option<Run>,
    enabled: bool,
}

impl<'a, S: PaintScene> TileScene<'a, S> {
    pub(crate) fn new(inner: &'a mut S) -> Self {
        Self {
            inner,
            run: None,
            enabled: ENABLED.with(|e| e.get()),
        }
    }

    /// Paint any buffered run. Call after `paint_scene`.
    pub(crate) fn finish(&mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        if run.transforms.len() >= 2 {
            if let Some((rows, cols)) = run.rows_cols() {
                let aligned = same_linear(run.linear, Affine::IDENTITY)
                    && whole(run.origin.x)
                    && whole(run.origin.y)
                    && whole(run.tile.width())
                    && whole(run.tile.height())
                    // A tile narrower than half a pixel rounds to a 0 px raster,
                    // which vello_cpu cannot allocate (panics).
                    && run.tile.width().round() >= 1.0
                    && run.tile.height().round() >= 1.0;
                if aligned {
                    if let Some(img) = cached_tile(&run) {
                        let brush = ImageBrush::new(ImageData::clone(&img))
                            .with_extend(Extend::Repeat)
                            .with_quality(ImageQuality::Low);
                        let area = Rect::new(
                            0.0,
                            0.0,
                            cols as f64 * run.tile.width(),
                            rows as f64 * run.tile.height(),
                        );
                        bump(|c| {
                            c.runs_merged += 1;
                            c.tiles_merged += run.transforms.len() as u64;
                        });
                        self.inner.fill(
                            run.style,
                            Affine::translate(run.origin),
                            PaintRef::Image(brush.as_ref()),
                            None,
                            &area,
                        );
                        return;
                    }
                }
            }
        }
        for t in &run.transforms {
            bump(|c| c.gradient_fills_forwarded += 1);
            self.inner.fill(
                run.style,
                *t,
                PaintRef::Gradient(&run.gradient),
                run.brush_xf,
                &run.tile,
            );
        }
    }
}

impl<'a, S: PaintScene> RenderContext for TileScene<'a, S> {
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

impl<'a, S: PaintScene> PaintScene for TileScene<'a, S> {
    fn reset(&mut self) {
        self.run = None;
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
        self.flush();
        self.inner
            .push_layer(fill, blend, alpha, transform, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.flush();
        self.inner.push_clip_layer(fill, transform, clip);
    }

    fn pop_layer(&mut self) {
        self.flush();
        self.inner.pop_layer();
    }

    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.flush();
        self.inner
            .stroke(style, transform, brush, brush_transform, shape);
    }

    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let brush = brush.into();
        let tile = match (&brush, shape.as_rect()) {
            (PaintRef::Gradient(g), Some(r))
                if self.enabled
                    && r.x0 == 0.0
                    && r.y0 == 0.0
                    && r.width() > 0.0
                    && r.height() > 0.0
                    && r.width() <= MAX_TILE_SIDE
                    && r.height() <= MAX_TILE_SIDE =>
            {
                Some((*g, r))
            }
            _ => None,
        };
        let Some((gradient, rect)) = tile else {
            self.flush();
            if matches!(brush, PaintRef::Gradient(_)) {
                bump(|c| c.gradient_fills_forwarded += 1);
            }
            self.inner
                .fill(style, transform, brush, brush_transform, shape);
            return;
        };
        if let Some(run) = self.run.as_mut() {
            if run.style == style
                && run.brush_xf == brush_transform
                && run.tile == rect
                && run.gradient == *gradient
                && run.accepts(transform)
            {
                run.transforms.push(transform);
                return;
            }
        }
        self.flush();
        self.run = Some(Run {
            style,
            gradient: gradient.clone(),
            brush_xf: brush_transform,
            tile: rect,
            linear: linear_of(transform),
            origin: transform.translation(),
            transforms: vec![transform],
            cols: None,
        });
    }

    fn draw_glyphs<'b, 's: 'b>(
        &'s mut self,
        font: &'b FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'b [anyrender::NormalizedCoord],
        embolden: Vec2,
        style: impl Into<StyleRef<'b>>,
        brush: impl Into<PaintRef<'b>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph> + Clone,
    ) {
        self.flush();
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
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.flush();
        self.inner
            .draw_box_shadow(transform, rect, brush, radius, std_dev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::recording::{RenderCommand, Scene};
    use anyrender::Paint;

    fn checker() -> Gradient {
        Gradient::new_linear((0.0, 0.0), (2.0, 0.0)).with_stops([
            (0.0, Color::BLACK),
            (0.5, Color::BLACK),
            (0.5, Color::WHITE),
            (1.0, Color::WHITE),
        ])
    }

    /// (gradient fills, image fills) recorded after feeding `tiles`.
    fn feed(tiles: &[(f64, f64)]) -> (usize, usize) {
        clear_tile_cache_for_test();
        let g = checker();
        let mut scene = Scene::new();
        let mut t = TileScene::new(&mut scene);
        t.enabled = true;
        for &(x, y) in tiles {
            t.fill(
                Fill::NonZero,
                Affine::translate((x, y)),
                &g,
                None,
                &Rect::new(0.0, 0.0, 2.0, 2.0),
            );
        }
        t.finish();
        scene.commands.iter().fold((0, 0), |(g, i), c| match c {
            RenderCommand::Fill(f) => match f.brush {
                Paint::Gradient(_) => (g + 1, i),
                Paint::Image(_) => (g, i + 1),
                _ => (g, i),
            },
            _ => (g, i),
        })
    }

    #[test]
    fn full_grid_merges_and_ragged_run_replays_every_tile() {
        let grid: Vec<_> = (0..2)
            .flat_map(|r| (0..3).map(move |c| (10.0 + 2.0 * c as f64, 4.0 + 2.0 * r as f64)))
            .collect();
        assert_eq!(feed(&grid), (0, 1));
        // A third row that stops after two tiles is not a rectangle.
        let mut ragged = grid.clone();
        ragged.extend([(10.0, 8.0), (12.0, 8.0)]);
        assert_eq!(feed(&ragged), (8, 0));
        // A gap in a row ends the run there.
        assert_eq!(feed(&[(0.0, 0.0), (2.0, 0.0), (6.0, 0.0)]), (1, 1));
    }

    #[test]
    fn tile_cache_stays_within_its_byte_bound() {
        clear_tile_cache_for_test();
        // 40 distinct 256x256 tiles (256 KiB each): 10 MiB unbounded.
        for i in 0..40u8 {
            let g = Gradient::new_linear((0.0, 0.0), (256.0, 0.0)).with_stops([
                (0.0, Color::from_rgb8(i, 0, 0)),
                (1.0, Color::from_rgb8(0, 0, 255 - i)),
            ]);
            let mut scene = Scene::new();
            let mut t = TileScene::new(&mut scene);
            t.enabled = true;
            for c in 0..2 {
                t.fill(
                    Fill::NonZero,
                    Affine::translate((256.0 * c as f64, 0.0)),
                    &g,
                    None,
                    &Rect::new(0.0, 0.0, 256.0, 256.0),
                );
            }
            t.finish();
            let held: usize =
                CACHE.with(|c| c.borrow().values().flatten().map(|i| i.data.len()).sum());
            assert!(held <= CACHE_MAX_BYTES, "tile {i}: {held} bytes cached");
            assert_eq!(held, CACHE_BYTES.with(|b| b.get()));
        }
    }

    #[test]
    fn sub_pixel_tiles_replay_instead_of_rasterizing_a_0px_tile() {
        // A column of 5e-7 x 2 px tiles counts as "whole" (within EPS) but
        // rounds to a 0 px wide raster.
        clear_tile_cache_for_test();
        let g = checker();
        let px = render_to_buffer::<VelloCpuImageRenderer, _>(
            |s| {
                let mut t = TileScene::new(s);
                t.enabled = true;
                for row in 0..6 {
                    t.fill(
                        Fill::NonZero,
                        Affine::translate((4.0, 4.0 + 2.0 * row as f64)),
                        &g,
                        None,
                        &Rect::new(0.0, 0.0, 5e-7, 2.0),
                    );
                }
                t.finish();
            },
            16,
            16,
        );
        assert_eq!(px.len(), 16 * 16 * 4);
    }
}
