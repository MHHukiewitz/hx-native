//! Synthetic command streams fed through [`TileScene`] and straight to the
//! renderer must give the same pixels, and merge exactly where expected.
//! Cases C1–C14 are Graph's unit probes from the #71 review; C15–C19 cover
//! the alignment and tile-rect guards his engine-level probes caught.

use super::*;
use anyrender::recording::Scene;
use anyrender::Glyph;

const W: u32 = 64;
const H: u32 = 48;

fn hard(x1: f64) -> Gradient {
    Gradient::new_linear((0.0, 0.0), (x1, 0.0)).with_stops([
        (0.0, Color::from_rgb8(200, 0, 0)),
        (0.5, Color::from_rgb8(200, 0, 0)),
        (0.5, Color::from_rgb8(0, 0, 200)),
        (1.0, Color::from_rgb8(0, 0, 200)),
    ])
}

fn hard_v(y1: f64) -> Gradient {
    Gradient::new_linear((0.0, 0.0), (0.0, y1)).with_stops([
        (0.0, Color::from_rgb8(0, 160, 0)),
        (0.5, Color::from_rgb8(0, 160, 0)),
        (0.5, Color::from_rgb8(250, 250, 0)),
        (1.0, Color::from_rgb8(250, 250, 0)),
    ])
}

fn conic() -> Gradient {
    let (w, g) = (Color::WHITE, Color::from_rgb8(192, 192, 192));
    Gradient::new_sweep((0.0, 0.0), 0.0, std::f32::consts::TAU).with_stops([
        (0.0, w),
        (0.25, w),
        (0.25, g),
        (0.5, g),
        (0.5, w),
        (0.75, w),
        (0.75, g),
        (1.0, g),
    ])
}

fn half_alpha() -> Gradient {
    Gradient::new_linear((0.0, 0.0), (4.0, 0.0)).with_stops([
        (0.0, Color::from_rgba8(0, 0, 0, 128)),
        (0.5, Color::from_rgba8(0, 0, 0, 128)),
        (0.5, Color::from_rgba8(255, 0, 0, 77)),
        (1.0, Color::from_rgba8(255, 0, 0, 77)),
    ])
}

fn soft_radial() -> Gradient {
    Gradient::new_radial((3.0, 3.0), 3.0)
        .with_stops([(0.0, Color::WHITE), (1.0, Color::from_rgb8(0, 0, 255))])
}

/// One gradient tile at `xf`.
fn tile<S: PaintScene>(s: &mut S, g: &Gradient, bxf: Option<Affine>, xf: Affine, tw: f64, th: f64) {
    s.fill(Fill::NonZero, xf, g, bxf, &Rect::new(0.0, 0.0, tw, th));
}

/// Blitz-order grid: rows outer, columns inner.
#[allow(clippy::too_many_arguments)]
fn grid<S: PaintScene>(
    s: &mut S,
    g: &Gradient,
    bxf: Option<Affine>,
    (ox, oy): (f64, f64),
    (tw, th): (f64, f64),
    cols: usize,
    rows: usize,
) {
    for r in 0..rows {
        for c in 0..cols {
            let xf = Affine::translate((ox + c as f64 * tw, oy + r as f64 * th));
            tile(s, g, bxf, xf, tw, th);
        }
    }
}

fn solid<S: PaintScene>(s: &mut S, c: Color, r: Rect) {
    s.fill(Fill::NonZero, Affine::IDENTITY, c, None, &r);
}

fn font() -> FontData {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../assets/fonts/DejaVuSans.ttf"
    );
    FontData::new(Blob::from(std::fs::read(path).unwrap()), 0)
}

/// Case `k`, and how many runs it must merge.
fn case<S: PaintScene>(s: &mut S, k: usize, f: &FontData) -> u64 {
    solid(
        s,
        Color::from_rgb8(30, 140, 90),
        Rect::new(0.0, 0.0, W as f64, H as f64),
    );
    let h4 = hard(4.0);
    match k {
        // C1 cache key: the same vertical gradient in 4x2 then 4x4 tiles.
        1 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 3, 3);
            solid(s, Color::BLACK, Rect::new(40.0, 40.0, 41.0, 41.0));
            grid(s, &hard_v(4.0), None, (20.0, 2.0), (4.0, 2.0), 3, 3);
            solid(s, Color::BLACK, Rect::new(41.0, 40.0, 42.0, 41.0));
            grid(s, &hard_v(4.0), None, (20.0, 20.0), (4.0, 4.0), 3, 3);
            3
        }
        // C2 push_layer right after a run, a solid inside a half-alpha layer.
        2 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 6, 4);
            let clip = Rect::new(0.0, 0.0, 40.0, 40.0);
            s.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                0.5,
                Affine::IDENTITY,
                &clip,
                None,
                None,
            );
            solid(s, Color::WHITE, Rect::new(4.0, 4.0, 12.0, 12.0));
            s.pop_layer();
            1
        }
        // C3 glyphs right after a run, overlapping it.
        3 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 10, 8);
            let glyphs = [
                Glyph {
                    id: 36,
                    x: 4.0,
                    y: 30.0,
                },
                Glyph {
                    id: 37,
                    x: 22.0,
                    y: 30.0,
                },
            ];
            s.draw_glyphs(
                f,
                28.0,
                false,
                &[],
                Vec2::ZERO,
                Fill::NonZero,
                Color::BLACK,
                1.0,
                Affine::IDENTITY,
                None,
                glyphs.into_iter(),
            );
            1
        }
        // C4 box shadow right after a run.
        4 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 10, 8);
            let r = Rect::new(6.0, 6.0, 30.0, 26.0);
            s.draw_box_shadow(Affine::IDENTITY, r, Color::BLACK, 2.0, 0.5);
            1
        }
        // C5 stroke right after a run.
        5 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 10, 8);
            let r = Rect::new(6.0, 6.0, 30.0, 26.0);
            s.stroke(&Stroke::new(3.0), Affine::IDENTITY, Color::BLACK, None, &r);
            1
        }
        // C6 tile rect not at the origin: never merged.
        6 => {
            for r in 0..4 {
                for c in 0..5 {
                    let xf = Affine::translate((2.0 + 4.0 * c as f64, 2.0 + 4.0 * r as f64));
                    s.fill(Fill::NonZero, xf, &h4, None, &Rect::new(1.0, 1.0, 5.0, 5.0));
                }
            }
            0
        }
        // C7 the next row continues the grid with another gradient.
        7 => {
            grid(s, &h4, None, (2.0, 2.0), (4.0, 4.0), 5, 1);
            grid(s, &hard_v(4.0), None, (2.0, 6.0), (4.0, 4.0), 5, 1);
            2
        }
        // C8 the next row continues the grid with another brush transform.
        8 => {
            let g = conic();
            grid(
                s,
                &g,
                Some(Affine::translate((2.0, 2.0))),
                (2.0, 2.0),
                (4.0, 4.0),
                5,
                1,
            );
            grid(
                s,
                &g,
                Some(Affine::translate((1.0, 3.0))),
                (2.0, 6.0),
                (4.0, 4.0),
                5,
                1,
            );
            2
        }
        // C9 a mirrored tile at the next grid translation.
        9 => {
            tile(s, &h4, None, Affine::translate((2.0, 2.0)), 4.0, 4.0);
            tile(
                s,
                &h4,
                None,
                Affine::new([-1.0, 0.0, 0.0, 1.0, 6.0, 2.0]),
                4.0,
                4.0,
            );
            tile(s, &h4, None, Affine::translate((10.0, 2.0)), 4.0, 4.0);
            0
        }
        // C10 soft radial (nearest sampling must still match).
        10 => {
            grid(s, &soft_radial(), None, (1.0, 3.0), (6.0, 6.0), 9, 6);
            1
        }
        // C11 partial alpha over a coloured background: never merged.
        11 => {
            grid(s, &half_alpha(), None, (1.0, 3.0), (4.0, 4.0), 12, 8);
            0
        }
        // C12 near-whole origin (1e-7 off), negative and offscreen origins.
        12 => {
            let bxf = Some(Affine::translate((1.0, 1.0)));
            grid(s, &conic(), bxf, (10.0000001, 4.0 - 1e-7), (2.0, 2.0), 7, 5);
            grid(s, &h4, None, (-3.0, -5.0), (4.0, 4.0), 6, 5);
            grid(s, &h4, None, (5000.0, 5000.0), (4.0, 4.0), 3, 3);
            3
        }
        // C13 a tile 5e-7 px wide: "whole" within EPS but a 0 px raster.
        13 => {
            grid(s, &h4, None, (4.0, 4.0), (5e-7, 2.0), 1, 6);
            0
        }
        // C14 256 px tiles (MAX_TILE_SIDE), partly visible.
        14 => {
            grid(s, &hard(256.0), None, (-200.0, 0.0), (256.0, 16.0), 2, 3);
            1
        }
        // C15 half-pixel origin x.
        15 => {
            grid(s, &h4, None, (2.5, 2.0), (4.0, 4.0), 6, 4);
            0
        }
        // C16 2.5 px wide tiles.
        16 => {
            grid(s, &hard(2.5), None, (2.0, 2.0), (2.5, 4.0), 8, 4);
            0
        }
        // C17 every tile scaled 2x (stepping by the unscaled tile size).
        17 => {
            for r in 0..3 {
                for c in 0..4 {
                    let xf = Affine::scale(2.0)
                        .then_translate(Vec2::new(2.0 + 4.0 * c as f64, 2.0 + 4.0 * r as f64));
                    tile(s, &h4, None, xf, 4.0, 4.0);
                }
            }
            0
        }
        // C18 2.5 px tiles whose cut edge is transparent, so the alpha guard
        // alone would not catch the fractional size.
        18 => {
            let g = Gradient::new_linear((0.0, 0.0), (2.5, 0.0)).with_stops([
                (0.0, Color::from_rgb8(200, 0, 0)),
                (0.4, Color::from_rgb8(200, 0, 0)),
                (0.4, Color::TRANSPARENT),
                (1.0, Color::TRANSPARENT),
            ]);
            grid(s, &g, None, (2.0, 2.0), (2.5, 4.0), 8, 4);
            0
        }
        // C19 tile rect starting at x 1 only (y 0): never merged.
        19 => {
            for r in 0..4 {
                for c in 0..5 {
                    let xf = Affine::translate((2.0 + 4.0 * c as f64, 2.0 + 4.0 * r as f64));
                    s.fill(Fill::NonZero, xf, &h4, None, &Rect::new(1.0, 0.0, 5.0, 4.0));
                }
            }
            0
        }
        _ => unreachable!(),
    }
}

fn render(k: usize, merge: bool) -> (Vec<u8>, u64, TileCoalesceCounters) {
    let f = font();
    let _ = take_tile_coalesce_counters_for_test();
    let mut want = 0;
    let px = render_to_buffer::<VelloCpuImageRenderer, _>(
        |s| {
            if merge {
                let mut t = TileScene::new(s);
                t.enabled = true;
                want = case(&mut t, k, &f);
                t.finish();
            } else {
                case(s, k, &f);
            }
        },
        W,
        H,
    );
    (px, want, take_tile_coalesce_counters_for_test())
}

#[test]
fn synthetic_streams_match_the_renderer_and_merge_where_expected() {
    clear_tile_cache_for_test();
    let mut bad = vec![];
    for k in 1..=19 {
        let (merged, want, c) = render(k, true);
        let (direct, _, _) = render(k, false);
        let diff = merged
            .chunks(4)
            .zip(direct.chunks(4))
            .filter(|(a, b)| a != b)
            .count();
        if diff != 0 || c.runs_merged != want {
            bad.push(format!(
                "C{k}: {diff} px differ, merged {} of {want} runs, {c:?}",
                c.runs_merged
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_recorded_run_flushes_before_every_other_command() {
    // Order matters even when the pixels happen not to show it.
    let f = font();
    let mut scene = Scene::new();
    let mut t = TileScene::new(&mut scene);
    t.enabled = true;
    case(&mut t, 3, &f);
    t.finish();
    let kinds: Vec<&str> = scene
        .commands
        .iter()
        .map(|c| match c {
            anyrender::recording::RenderCommand::Fill(_) => "fill",
            anyrender::recording::RenderCommand::GlyphRun(_) => "glyphs",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["fill", "fill", "glyphs"]);
}
