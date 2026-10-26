//! Paint zero-blur inset `box-shadow` layers as one fill (issue #123).
//!
//! Blitz 0.3.0-beta.4 (`blitz-paint` `render/box_shadow.rs`,
//! `draw_inset_box_shadow`) paints every inset shadow layer as six commands:
//!
//! 1. `push_layer` (Normal, alpha 1) clipped to the padding box path;
//! 2. `fill` of the padding box path with the shadow colour;
//! 3. `push_layer` (DestOut, alpha 1) clipped to the same path;
//! 4. `draw_box_shadow` of the hole (padding box shrunk by the spread, moved
//!    by the offset) in white, with the averaged corner radius and the blur;
//! 5. and 6. two `pop_layer`s.
//!
//! vello_cpu draws step 4 with its blurred rounded rect shader, which runs
//! `powf` per pixel over the whole hole, even at zero blur. A Win95 bevel
//! (`inset 1px 1px #fff, inset -1px -1px #808080`) on a big box therefore
//! costs more than the rest of the page (Mainnet Market: 87 ms of a 90 ms
//! debug repaint at 1280×657).
//!
//! [`InsetScene`] sits between Blitz and the renderer and recognises that
//! sequence. When the layer has no blur and no corner radius, and the padding
//! box and the hole both land on whole device pixels under a pure
//! translation, it paints one even-odd fill instead: the padding box minus
//! the hole (cut to the padding box), inside Blitz's outer layer only when
//! the colour is translucent. The pixels are the same: at zero blur
//! vello's shader is a hard step at each pixel centre, which equals area
//! coverage on whole-pixel edges. Anything else (blur, a radius, a scaled or
//! fractional box, a different command order) is replayed unchanged.
//!
//! Blitz's own offset is passed through as it is: it is in CSS px, not device
//! px (the layer shifts by 1 device px at scale 2), and the fast path keeps
//! that.
//!
//! Pin-sensitive: depends on the command order above (see BLITZ_PIN.md).

use std::cell::Cell;
use std::sync::Arc;

use anyrender::filters::Filter;
use anyrender::{PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Compose, Fill, FontData, Mix, StyleRef};

const EPS: f64 = 1e-6;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static COUNTERS: Cell<InsetShadowCounters> = const { Cell::new(InsetShadowCounters::ZERO) };
}

/// Counters for tests and benches (per thread, since the last reset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsetShadowCounters {
    /// Inset layers painted as one even-odd fill.
    pub fast: u64,
    /// Inset layers replayed to the renderer unchanged (blur, radius, or not
    /// on whole pixels).
    pub replayed: u64,
}

impl InsetShadowCounters {
    const ZERO: Self = Self {
        fast: 0,
        replayed: 0,
    };
}

fn bump(f: impl FnOnce(&mut InsetShadowCounters)) {
    COUNTERS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// Test hook: turn the fast path off (per thread) to get Blitz's layers.
#[doc(hidden)]
pub fn set_inset_shadow_fast_path_for_test(enabled: bool) {
    ENABLED.with(|e| e.set(enabled));
}

/// Test hook: read and reset the per-thread counters.
#[doc(hidden)]
pub fn take_inset_shadow_counters_for_test() -> InsetShadowCounters {
    COUNTERS.with(|c| c.replace(InsetShadowCounters::ZERO))
}

/// The hole draw (step 4).
#[derive(Clone, Copy)]
struct Hole {
    transform: Affine,
    rect: Rect,
    radius: f64,
    std_dev: f64,
}

/// How far into the six commands the buffered layer got.
#[derive(Clone, Copy)]
enum Stage {
    /// Step 1 seen.
    Clip,
    /// Step 2 seen (shadow colour).
    Filled(Color),
    /// Step 3 seen.
    Cut(Color),
    /// Step 4 seen.
    Hole(Color, Hole),
    /// Step 5 seen.
    Popped(Color, Hole),
}

struct Pending {
    transform: Affine,
    path: BezPath,
    stage: Stage,
}

/// PaintScene adapter that paints Blitz's zero-blur inset shadow layers as
/// one fill.
pub(crate) struct InsetScene<'a, S: PaintScene> {
    inner: &'a mut S,
    pending: Option<Pending>,
    enabled: bool,
}

impl<'a, S: PaintScene> InsetScene<'a, S> {
    pub(crate) fn new(inner: &'a mut S) -> Self {
        Self {
            inner,
            pending: None,
            enabled: ENABLED.with(|e| e.get()),
        }
    }

    /// Replay any buffered commands. Call after `paint_scene`.
    pub(crate) fn finish(&mut self) {
        self.flush();
    }

    /// Forward the buffered commands unchanged.
    fn flush(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let (t, path) = (p.transform, &p.path);
        self.inner
            .push_layer(Fill::NonZero, Mix::Normal, 1.0, t, path, None, None);
        let (color, hole, pops) = match p.stage {
            Stage::Clip => return,
            Stage::Filled(c) => (c, None, 0),
            Stage::Cut(c) => (c, None, 0),
            Stage::Hole(c, h) => (c, Some(h), 0),
            Stage::Popped(c, h) => (c, Some(h), 1),
        };
        self.inner.fill(Fill::NonZero, t, color, None, path);
        if matches!(p.stage, Stage::Filled(_)) {
            return;
        }
        self.inner
            .push_layer(Fill::NonZero, Compose::DestOut, 1.0, t, path, None, None);
        if let Some(h) = hole {
            self.inner
                .draw_box_shadow(h.transform, h.rect, Color::WHITE, h.radius, h.std_dev);
        }
        for _ in 0..pops {
            self.inner.pop_layer();
        }
    }

    /// The last `pop_layer` of a whole layer: one fill when exact, else the
    /// six commands unchanged.
    fn complete(&mut self, p: Pending, color: Color, hole: Hole) {
        if let Some(path) = fast_fill(p.transform, &p.path, hole) {
            bump(|c| c.fast += 1);
            // A translucent colour keeps Blitz's outer layer: composited
            // straight onto the backdrop it rounds one level differently
            // (207 vs 208 for 25% white over #c0c0c0).
            let layer = color.components[3] < 1.0;
            if layer {
                self.inner.push_layer(
                    Fill::NonZero,
                    Mix::Normal,
                    1.0,
                    p.transform,
                    &p.path,
                    None,
                    None,
                );
            }
            self.inner
                .fill(Fill::EvenOdd, Affine::IDENTITY, color, None, &path);
            if layer {
                self.inner.pop_layer();
            }
            return;
        }
        bump(|c| c.replayed += 1);
        self.pending = Some(Pending {
            stage: Stage::Popped(color, hole),
            ..p
        });
        self.flush();
        self.inner.pop_layer();
    }
}

/// Whole device px (within EPS), rounded.
fn whole(v: f64) -> Option<f64> {
    let r = v.round();
    ((v - r).abs() < EPS).then_some(r)
}

fn whole_rect(r: Rect) -> Option<Rect> {
    Some(Rect::new(
        whole(r.x0)?,
        whole(r.y0)?,
        whole(r.x1)?,
        whole(r.y1)?,
    ))
}

fn has_area(r: Rect) -> bool {
    r.x0 < r.x1 && r.y0 < r.y1
}

fn is_translation(t: Affine) -> bool {
    let c = t.as_coeffs();
    (c[0] - 1.0).abs() < EPS && c[1].abs() < EPS && c[2].abs() < EPS && (c[3] - 1.0).abs() < EPS
}

/// The path's rect, when it is one: a move and lines round its four corners,
/// each side along one axis (Blitz's padding box path with sharp corners).
fn path_rect(path: &BezPath) -> Option<Rect> {
    let mut pts = Vec::with_capacity(5);
    for (i, el) in path.elements().iter().enumerate() {
        match el {
            PathEl::MoveTo(p) if i == 0 => pts.push(*p),
            PathEl::LineTo(p) if i > 0 => pts.push(*p),
            PathEl::ClosePath if i > 0 => {}
            _ => return None,
        }
    }
    let near = |a: f64, b: f64| (a - b).abs() < EPS;
    if pts.len() == 5 && near(pts[0].x, pts[4].x) && near(pts[0].y, pts[4].y) {
        pts.pop();
    }
    if pts.len() != 4 {
        return None;
    }
    let side = |a: Point, b: Point| near(a.x, b.x) != near(a.y, b.y);
    (0..4)
        .all(|i| side(pts[i], pts[(i + 1) % 4]))
        .then(|| path.bounding_box())
}

/// The padding box minus the hole as one even-odd device-space path, when
/// that is pixel-identical to Blitz's layers.
fn fast_fill(transform: Affine, path: &BezPath, hole: Hole) -> Option<BezPath> {
    if hole.std_dev != 0.0
        || hole.radius != 0.0
        || !is_translation(transform)
        || !is_translation(hole.transform)
    {
        return None;
    }
    let outer = whole_rect(path_rect(path)? + transform.translation())?;
    let cut = whole_rect(hole.rect + hole.transform.translation())?;
    // A hole the spread turned inside out, or an empty one, is left to
    // vello (its shader divides by the hole's size).
    if !has_area(cut) || !has_area(outer) {
        return None;
    }
    let mut out = outer.to_path(0.1);
    let cut = cut.intersect(outer);
    if has_area(cut) {
        out.extend(cut.to_path(0.1));
    }
    Some(out)
}

fn path_of(shape: &impl Shape) -> Option<&[PathEl]> {
    shape.as_path_slice()
}

fn plain_layer(
    fill: Fill,
    alpha: f32,
    filter: &Option<Arc<Filter>>,
    backdrop: &Option<Arc<Filter>>,
) -> bool {
    fill == Fill::NonZero && alpha == 1.0 && filter.is_none() && backdrop.is_none()
}

impl<'a, S: PaintScene> RenderContext for InsetScene<'a, S> {
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

impl<'a, S: PaintScene> PaintScene for InsetScene<'a, S> {
    fn reset(&mut self) {
        self.pending = None;
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
        let blend = blend.into();
        let plain = plain_layer(fill, alpha, &filter, &backdrop_filter);
        // Step 3: the DestOut layer over the same path.
        if let Some(p) = self.pending.as_mut() {
            if let Stage::Filled(c) = p.stage {
                if plain
                    && blend == BlendMode::from(Compose::DestOut)
                    && transform == p.transform
                    && path_of(clip) == Some(p.path.elements())
                {
                    p.stage = Stage::Cut(c);
                    return;
                }
            }
        }
        self.flush();
        // Step 1.
        if self.enabled && plain && blend == BlendMode::from(Mix::Normal) {
            if let Some(els) = path_of(clip) {
                self.pending = Some(Pending {
                    transform,
                    path: BezPath::from_vec(els.to_vec()),
                    stage: Stage::Clip,
                });
                return;
            }
        }
        self.inner
            .push_layer(fill, blend, alpha, transform, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.flush();
        self.inner.push_clip_layer(fill, transform, clip);
    }

    fn pop_layer(&mut self) {
        let Some(mut p) = self.pending.take() else {
            self.inner.pop_layer();
            return;
        };
        match p.stage {
            // Step 5.
            Stage::Hole(c, h) => {
                p.stage = Stage::Popped(c, h);
                self.pending = Some(p);
            }
            // Step 6.
            Stage::Popped(c, h) => self.complete(p, c, h),
            _ => {
                self.pending = Some(p);
                self.flush();
                self.inner.pop_layer();
            }
        }
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
        // Step 2: the shadow colour over the clip path.
        if let Some(p) = self.pending.as_mut() {
            if let (Stage::Clip, PaintRef::Solid(c), None) = (&p.stage, &brush, brush_transform) {
                if style == Fill::NonZero
                    && transform == p.transform
                    && path_of(shape) == Some(p.path.elements())
                {
                    p.stage = Stage::Filled(*c);
                    return;
                }
            }
        }
        self.flush();
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
        // Step 4: the white hole.
        if let Some(p) = self.pending.as_mut() {
            if let Stage::Cut(c) = p.stage {
                if brush == Color::WHITE {
                    p.stage = Stage::Hole(
                        c,
                        Hole {
                            transform,
                            rect,
                            radius,
                            std_dev,
                        },
                    );
                    return;
                }
            }
        }
        self.flush();
        self.inner
            .draw_box_shadow(transform, rect, brush, radius, std_dev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::recording::{RenderCommand, Scene};

    /// Blitz's sharp-cornered padding box: a move and three lines.
    fn padding_path(r: Rect) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((r.x0, r.y0));
        p.line_to((r.x1, r.y0));
        p.line_to((r.x1, r.y1));
        p.line_to((r.x0, r.y1));
        p
    }

    /// Blitz's six commands for one inset layer.
    fn inset_layer(s: &mut impl PaintScene, t: Affine, path: &BezPath, hole: Hole, c: Color) {
        s.push_layer(Fill::NonZero, Mix::Normal, 1.0, t, path, None, None);
        s.fill(Fill::NonZero, t, c, None, path);
        s.push_layer(Fill::NonZero, Compose::DestOut, 1.0, t, path, None, None);
        s.draw_box_shadow(
            hole.transform,
            hole.rect,
            Color::WHITE,
            hole.radius,
            hole.std_dev,
        );
        s.pop_layer();
        s.pop_layer();
    }

    fn hole(t: Affine, rect: Rect, std_dev: f64) -> Hole {
        Hole {
            transform: t,
            rect,
            radius: 0.0,
            std_dev,
        }
    }

    fn adapted(feed: impl Fn(&mut InsetScene<'_, Scene>)) -> Vec<RenderCommand> {
        let mut scene = Scene::new();
        let mut inset = InsetScene::new(&mut scene);
        inset.enabled = true;
        feed(&mut inset);
        inset.finish();
        scene.commands
    }

    fn direct(feed: impl Fn(&mut Scene)) -> Vec<RenderCommand> {
        let mut scene = Scene::new();
        feed(&mut scene);
        scene.commands
    }

    const T: Affine = Affine::new([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);
    const BOX: Rect = Rect::new(0.0, 0.0, 50.0, 30.0);

    #[test]
    fn a_whole_pixel_bevel_layer_is_one_even_odd_fill() {
        let path = padding_path(BOX);
        let h = hole(T.then_translate(Vec2::new(1.0, 1.0)), BOX, 0.0);
        let out = adapted(|s| inset_layer(s, T, &path, h, Color::BLACK));
        assert_eq!(out.len(), 1, "{out:?}");
        let RenderCommand::Fill(f) = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(f.fill, Fill::EvenOdd);
    }

    #[test]
    fn blurred_and_unrelated_layers_reach_the_renderer_unchanged() {
        let path = padding_path(BOX);
        let blurred = hole(T, BOX, 2.0);
        let a = adapted(|s| inset_layer(s, T, &path, blurred, Color::BLACK));
        let b = direct(|s| inset_layer(s, T, &path, blurred, Color::BLACK));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));

        // A clip layer that is not an inset shadow: push, another fill, pop;
        // and one cut off after its colour fill by a nested layer.
        fn other(s: &mut impl PaintScene, path: &BezPath) {
            s.push_layer(Fill::NonZero, Mix::Normal, 1.0, T, path, None, None);
            s.fill(
                Fill::NonZero,
                T,
                Color::WHITE,
                None,
                &Rect::new(0.0, 0.0, 5.0, 5.0),
            );
            s.pop_layer();
            s.push_layer(Fill::NonZero, Mix::Normal, 1.0, T, path, None, None);
            s.fill(Fill::NonZero, T, Color::BLACK, None, path);
            s.push_layer(Fill::NonZero, Mix::Normal, 1.0, T, path, None, None);
            s.pop_layer();
            s.pop_layer();
        }
        let a = adapted(|s| other(s, &path));
        let b = direct(|s| other(s, &path));
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn only_a_four_sided_axis_aligned_path_counts_as_a_rect() {
        assert_eq!(path_rect(&padding_path(BOX)), Some(BOX));
        let mut closed = padding_path(BOX);
        closed.line_to((0.0, 0.0));
        closed.close_path();
        assert_eq!(path_rect(&closed), Some(BOX));
        // The same four corners in a bow-tie order.
        let mut tie = BezPath::new();
        tie.move_to((0.0, 0.0));
        tie.line_to((50.0, 30.0));
        tie.line_to((50.0, 0.0));
        tie.line_to((0.0, 30.0));
        assert_eq!(path_rect(&tie), None);
        let mut curved = padding_path(BOX);
        curved.quad_to((0.0, 0.0), (0.0, 0.0));
        assert_eq!(path_rect(&curved), None);
    }
}
