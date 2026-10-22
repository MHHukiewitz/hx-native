//! Issue #67: Blitz's per-tile gradient fills are painted as one repeated
//! fill. The merged path must be pixel-identical to Blitz's per-tile path
//! (conic, linear and radial tiles, scale 1, 1.5 and 2), partial repaint
//! must equal full repaint (damage starting partway through a tile), and the
//! 2×2 dither box must cost no more than twice a solid fill.

use std::time::Instant;

use hx_native::*;

/// Kit-shaped dither (Mainnet `--dither`, rising stops), plus linear and
/// radial tiles, odd box sizes and offsets so tiles are cut at every edge.
fn page(extra: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>
html,body{{margin:0;background:#808080}}
.b{{position:absolute;box-sizing:border-box}}
.dither{{background:#c0c0c0 repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/2px 2px}}
.lin{{background:linear-gradient(90deg,#c00 50%,#00c 50%) 1px 3px/5px 3px}}
.rad{{background:radial-gradient(circle,#000 0 2px,#ff0 2px) 0 0/6px 6px}}
.soft{{background:linear-gradient(90deg,#c00,#00c) 0 0/7px 4px}}
.clear{{background:repeating-conic-gradient(#000 0deg 90deg,transparent 90deg 180deg) 0 0/2px 2px}}
.half{{background:repeating-conic-gradient(rgba(0,0,0,.5) 0deg 90deg,transparent 90deg 180deg) 0 0/2px 2px}}
.space{{background:linear-gradient(#0a0 50%,#a0a 50%) 0 0/4px 4px space}}
.big{{background:repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/4px 4px}}
{extra}
</style></head><body>
<div id="d1" class="b dither" style="left:3px;top:5px;width:43px;height:21px;box-shadow:inset 1px 1px #000"></div>
<div id="d2" class="b dither" style="left:60px;top:2px;width:10px;height:10px;border:1px solid #000"></div>
<div id="l1" class="b lin" style="left:80px;top:3px;width:33px;height:17px"></div>
<div id="r1" class="b rad" style="left:120px;top:1px;width:37px;height:25px"></div>
<div id="s1" class="b soft" style="left:160px;top:4px;width:30px;height:20px"></div>
<div id="c1" class="b clear" style="left:195px;top:5px;width:21px;height:13px"></div>
<div id="h1" class="b half" style="left:220px;top:5px;width:21px;height:13px"></div>
<div id="p1" class="b space" style="left:245px;top:3px;width:22px;height:22px"></div>
<div id="g1" class="b big" style="left:270px;top:7px;width:17px;height:19px"></div>
</body></html>"#
    )
}

fn engine(mode: PaintMode) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 10_000_000_000,
        paint_mode: mode,
        ..EngineConfig::default()
    });
    e.declare_actions(&[]);
    e
}

fn full_paint(html: &str, scale: f64, merge: bool) -> Vec<u8> {
    set_tile_coalesce_for_test(merge);
    let mut e = engine(PaintMode::Exact);
    e.load_html(html).unwrap();
    let (w, h) = ((300.0 * scale) as u32, (30.0 * scale) as u32);
    let out = e.paint(Viewport::with_scale(w, h, scale)).unwrap();
    set_tile_coalesce_for_test(true);
    out.rgba.unwrap().pixels
}

fn first_diff(a: &[u8], b: &[u8], w: usize) -> Option<(usize, usize, [u8; 4], [u8; 4])> {
    a.chunks(4)
        .zip(b.chunks(4))
        .enumerate()
        .find_map(|(i, (p, q))| {
            (p != q).then(|| {
                (
                    i % w,
                    i / w,
                    [p[0], p[1], p[2], p[3]],
                    [q[0], q[1], q[2], q[3]],
                )
            })
        })
}

#[test]
fn merged_tiles_are_pixel_identical_to_blitz_per_tile_path() {
    clear_tile_cache_for_test();
    let html = page("");
    for scale in [1.0, 1.5, 2.0] {
        let _ = take_tile_coalesce_counters_for_test();
        let merged = full_paint(&html, scale, true);
        let c = take_tile_coalesce_counters_for_test();
        let per_tile = full_paint(&html, scale, false);
        let w = (300.0 * scale) as usize;
        assert_eq!(
            first_diff(&merged, &per_tile, w),
            None,
            "scale {scale}: (x, y, merged, per-tile)"
        );
        // Every exact run merges: dither x2, linear, radial, clear, big (6).
        // soft is one tile per 7x4 run too, but merges; half (partial alpha)
        // and space (spaced tiles) must not.
        assert!(
            c.gradient_fills_forwarded > 0,
            "half/space fall back: {c:?}"
        );
        if scale == 1.5 {
            // Odd CSS offsets land on half pixels: the per-tile path, unchanged.
            continue;
        }
        assert!(c.runs_merged >= 6, "scale {scale}: {c:?}");
    }
}

#[test]
fn merged_runs_count_and_fallbacks_at_scale_1() {
    clear_tile_cache_for_test();
    let one = |cls: &str, w: u32, h: u32| {
        format!(
            r#"<!DOCTYPE html><html><head><style>{}</style></head><body><div class="b {cls}" style="left:3px;top:5px;width:{w}px;height:{h}px"></div></body></html>"#,
            page("")
                .split("<style>")
                .nth(1)
                .unwrap()
                .split("</style>")
                .next()
                .unwrap()
        )
    };
    let count = |html: &str| {
        let _ = take_tile_coalesce_counters_for_test();
        let _ = full_paint(html, 1.0, true);
        take_tile_coalesce_counters_for_test()
    };
    let c = count(&one("dither", 43, 20));
    assert_eq!(
        (c.runs_merged, c.tiles_merged, c.gradient_fills_forwarded),
        (1, 22 * 10, 0),
        "{c:?}"
    );
    let c = count(&one("half", 21, 13));
    assert_eq!(c.runs_merged, 0, "partial alpha falls back: {c:?}");
    assert_eq!(c.gradient_fills_forwarded, 11 * 7, "{c:?}");
    let c = count(&one("space", 22, 22));
    assert_eq!(c.runs_merged, 0, "spaced tiles are not one grid: {c:?}");
    // Cache: a second paint of the same box rasterizes no new tile.
    let html = one("dither", 43, 20);
    let _ = count(&html);
    let c = count(&html);
    assert_eq!((c.runs_merged, c.tile_rasters), (1, 0), "cache hit: {c:?}");
}

#[test]
fn tiles_that_differ_in_size_centre_or_colour_do_not_share_a_cache_entry() {
    clear_tile_cache_for_test();
    // Blitz puts a conic's centre in the brush transform, so k4/k4c differ
    // only there, k4/k6c only in tile size, k4/k4b only in colour.
    let html = page(
        ".k2{background:repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/2px 2px}
.k4{background:repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/4px 4px}
.k6{background:repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/6px 2px}
.k4b{background:repeating-conic-gradient(#000 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/4px 4px}
.k4c{background:repeating-conic-gradient(at 1px 1px,#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/4px 4px}
.k6c{background:repeating-conic-gradient(at 2px 2px,#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/6px 6px}",
    )
    .replace(
        "</body>",
        r#"<div class="b k2" style="left:0;top:0;width:9px;height:9px"></div><div class="b k4" style="left:10px;top:0;width:9px;height:9px"></div><div class="b k6" style="left:20px;top:0;width:13px;height:9px"></div><div class="b k4b" style="left:40px;top:0;width:9px;height:9px"></div><div class="b k4c" style="left:50px;top:0;width:9px;height:9px"></div><div class="b k6c" style="left:60px;top:0;width:13px;height:13px"></div></body>"#,
    );
    for scale in [1.0, 2.0] {
        let merged = full_paint(&html, scale, true);
        let again = full_paint(&html, scale, true);
        let per_tile = full_paint(&html, scale, false);
        let w = (300.0 * scale) as usize;
        assert_eq!(first_diff(&merged, &per_tile, w), None, "scale {scale}");
        assert_eq!(
            first_diff(&again, &per_tile, w),
            None,
            "scale {scale}, cached"
        );
    }
}

#[test]
fn partial_repaint_from_mid_tile_damage_equals_full_repaint() {
    clear_tile_cache_for_test();
    let html = page("");
    // Exact only: a Shifted partial paint already differs from a full paint
    // for plain solid fills on main (CullScene compares shifted bounds with
    // the unshifted clip), with or without this merge.
    for mode in [PaintMode::Exact] {
        for scale in [1.0, 2.0] {
            let (w, h) = ((300.0 * scale) as u32, (30.0 * scale) as u32);
            let vp = Viewport::with_scale(w, h, scale);
            let mut e = engine(mode);
            e.load_html(&html).unwrap();
            let full = e.paint(vp.clone()).unwrap().rgba.unwrap().pixels;
            // Rects that start and end partway through 2, 3, 5 and 6 px tiles.
            for (x, y, rw, rh) in [
                (6.0, 8.0, 7.0, 5.0),
                (4.0, 6.0, 41.0, 19.0),
                (82.0, 4.0, 9.0, 11.0),
                (123.0, 3.0, 13.0, 7.0),
                (197.0, 6.0, 3.0, 3.0),
                (272.0, 9.0, 5.0, 13.0),
            ] {
                let s = scale as f32;
                e.test_push_pending_damage(Rect {
                    x: x * s + 1.0,
                    y: y * s + 1.0,
                    width: rw * s,
                    height: rh * s,
                });
                let out = e.paint(vp.clone()).unwrap();
                assert!(!out.skipped);
                assert!(
                    !e.last_paint_stats().full_repaint,
                    "{mode:?} must be a partial"
                );
                let part = out.rgba.unwrap().pixels;
                assert_eq!(
                    first_diff(&part, &full, w as usize),
                    None,
                    "{mode:?} scale {scale} rect {x},{y} {rw}x{rh}"
                );
            }
        }
    }
}

fn min_paint_ns(html: &str, iters: usize) -> u64 {
    let mut best = u64::MAX;
    let vp = Viewport::with_scale(300, 30, 1.0);
    let mut e = engine(PaintMode::Exact);
    e.load_html(html).unwrap();
    let _ = e.paint(vp.clone()).unwrap();
    for _ in 0..iters {
        e.test_push_pending_damage(Rect {
            x: 0.0,
            y: 0.0,
            width: 60.0,
            height: 30.0,
        });
        let t = Instant::now();
        let out = e.paint(vp.clone()).unwrap();
        let ns = t.elapsed().as_nanos() as u64;
        assert!(!out.skipped);
        best = best.min(ns);
    }
    best
}

#[test]
fn dither_box_costs_at_most_twice_a_solid_fill() {
    clear_tile_cache_for_test();
    let box_page = |bg: &str| {
        format!(
            r#"<!DOCTYPE html><html><head><style>html,body{{margin:0}}#b{{position:absolute;left:3px;top:5px;width:43px;height:20px;background:{bg};box-shadow:inset 1px 1px #000}}</style></head><body><div id="b"></div></body></html>"#
        )
    };
    let dither = box_page(
        "#c0c0c0 repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/2px 2px",
    );
    let solid = box_page("#c0c0c0");
    // Counts first (load-independent): the 220 tiles reach the renderer as
    // one image fill, no gradient fill at all.
    let _ = take_tile_coalesce_counters_for_test();
    let _ = min_paint_ns(&dither, 1);
    let c = take_tile_coalesce_counters_for_test();
    assert_eq!(c.gradient_fills_forwarded, 0, "{c:?}");
    assert_eq!(
        c.tiles_merged,
        2 * 22 * 10,
        "first + one partial paint: {c:?}"
    );
    // Timing: best of N partial repaints, so load only makes both slower.
    let iters = 9;
    let d = min_paint_ns(&dither, iters);
    let s = min_paint_ns(&solid, iters);
    assert!(
        d <= 2 * s,
        "dither {:.3} ms vs solid {:.3} ms (limit 2x)",
        d as f64 / 1e6,
        s as f64 / 1e6
    );
    // And the per-tile path really is the slow one this guards against.
    set_tile_coalesce_for_test(false);
    let slow = min_paint_ns(&dither, 3);
    set_tile_coalesce_for_test(true);
    eprintln!(
        "dither box: merged {:.3} ms, solid {:.3} ms, per-tile {:.3} ms",
        d as f64 / 1e6,
        s as f64 / 1e6,
        slow as f64 / 1e6
    );
}

#[test]
fn sub_pixel_background_size_paints_without_panicking() {
    // background-size below half a device pixel on the non-repeating axis:
    // Blitz still emits one column (or row) of tiles.
    for (bg, scale) in [
        ("linear-gradient(90deg,#f00 50%,#00f 50%) 0 0/0.0000005px 2px repeat-y", 1.0),
        ("linear-gradient(#f00 50%,#00f 50%) 0 0/2px 0.0000004px repeat-x", 1.0),
        ("repeating-conic-gradient(#fff 0deg 90deg,#c0c0c0 90deg 180deg) 0 0/0.0000002px 2px repeat-y", 2.0),
    ] {
        let html = format!(
            r#"<!DOCTYPE html><html><head><style>html,body{{margin:0}}#b{{position:absolute;left:4px;top:6px;width:40px;height:20px;background:#c0c0c0 {bg}}}</style></head><body><div id="b"></div></body></html>"#
        );
        let merged = full_paint(&html, scale, true);
        let per_tile = full_paint(&html, scale, false);
        let w = (300.0 * scale) as usize;
        assert_eq!(first_diff(&merged, &per_tile, w), None, "{bg} at {scale}");
    }
}
