//! Issue #116: a radio sits on the line like in Chrome (border-box bottom on
//! the text baseline), and its painted ring and dot do not depend on the
//! display scale (same CSS-px geometry at scale 1, 1.5, 2).
//!
//! Everything compares layout boxes and painted pixels, never engine internals.
use hx_native::*;

const FONT: &str = "font:16px \"DejaVu Sans\"";
const SCALES: [f64; 3] = [1.0, 1.5, 2.0];

/// Open `body` in a `w`×`h` CSS-px viewport at `scale` and paint once.
fn open(body: &str, scale: f64, w: u32, h: u32) -> Engine {
    let html = format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff;{FONT}}}\
         </style></head><body>{body}</body></html>"
    );
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    let vp = viewport(scale, w, h);
    e.open_surface(
        SurfaceId::DEFAULT,
        &html,
        vp.clone(),
        SurfaceOpts {
            visible: true,
            pinned: true,
        },
    )
    .unwrap();
    e.paint(vp).unwrap();
    e
}

fn viewport(scale: f64, w: u32, h: u32) -> Viewport {
    Viewport::with_scale(
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
        scale,
    )
}

/// Zero-margin empty inline-block: its bottom edge is the line baseline in
/// every engine (Chrome and Blitz alike), so it is the baseline probe.
const PROBE: &str =
    "<span id=\"p\" style=\"display:inline-block;width:1px;height:0;margin:0\"></span>";

fn bottom(e: &Engine, id: &str) -> f32 {
    let (_, y, _, h) = e
        .test_element_box(id)
        .unwrap_or_else(|| panic!("no box {id}"));
    y + h
}

fn top(e: &Engine, id: &str) -> f32 {
    e.test_element_box(id)
        .unwrap_or_else(|| panic!("no box {id}"))
        .1
}

fn radio_line(style: &str) -> String {
    format!(
        "<p style=\"margin:10px\">Term {PROBE}<input type=\"radio\" id=\"r\" name=\"g\" style=\"{style}\">\
         <label>12</label></p>"
    )
}

fn assert_on_baseline(e: &Engine, what: &str) {
    let (r, p) = (bottom(e, "r"), bottom(e, "p"));
    assert!(
        (r - p).abs() < 0.05,
        "{what}: radio bottom {r} vs baseline {p}"
    );
}

/// Chrome 156 (issue #116): the border-box bottom of a radio is the baseline,
/// whatever its margin-bottom.
#[test]
fn radio_border_box_bottom_sits_on_the_baseline() {
    for scale in SCALES {
        for style in [
            "",
            "margin-bottom:0",
            "margin-bottom:10px",
            "margin:0 0 7px 0",
            "box-sizing:border-box;padding:0;width:14px;height:14px",
        ] {
            let e = open(&radio_line(style), scale, 300, 80);
            assert_on_baseline(&e, &format!("scale {scale} style {style:?}"));
        }
    }
}

/// The radio inside an inline `<label>` (the usual markup) and followed by
/// wrapped text lines is on the baseline of its own line.
#[test]
fn radio_inside_a_label_sits_on_the_baseline() {
    let body = format!(
        "<p style=\"margin:10px\">First line of text that wraps around<br>\
         <label>Term {PROBE}<input type=\"radio\" id=\"r\" name=\"g\" style=\"margin:0 0 5px 0\"> twelve</label></p>"
    );
    for scale in SCALES {
        let e = open(&body, scale, 300, 100);
        assert_on_baseline(&e, &format!("scale {scale}"));
    }
}

/// The shift is applied after every layout and never accumulates: resizing
/// (re-wrapping the text) and toggling the radio keep it on the baseline.
#[test]
fn radio_stays_on_the_baseline_across_relayouts() {
    for scale in SCALES {
        let body = format!(
            "<p style=\"margin:10px\">Some words before the radio <span style=\"white-space:nowrap\">{PROBE}\
             <input type=\"radio\" id=\"r\" name=\"g\" style=\"margin:0 0 5px 0\"></span> and after</p>"
        );
        let mut e = open(&body, scale, 300, 120);
        assert_on_baseline(&e, &format!("scale {scale} first layout"));
        for (w, h) in [(60, 160), (300, 120), (90, 160), (300, 120)] {
            let vp = viewport(scale, w, h);
            e.resize_surface(SurfaceId::DEFAULT, vp.clone()).unwrap();
            e.paint(vp).unwrap();
            let r = e.test_element_box("r").expect("radio box");
            assert!(r.3 > 0.0, "scale {scale} {w}px: radio has a size");
            assert_on_baseline(&e, &format!("scale {scale} width {w}"));
        }
        let y = top(&e, "r");
        let vp = viewport(scale, 300, 120);
        e.paint(vp).unwrap();
        assert_eq!(top(&e, "r"), y, "scale {scale}: repaint must not move it");
    }
}

/// Alignments that use the margin box (not the baseline) are the same in
/// Chrome and Blitz and must not be shifted. Ground truth: an empty
/// inline-block of the same size, margin and `vertical-align` (Blitz lays it
/// out like it laid out the radio before the fix).
#[test]
fn margin_box_alignments_are_not_shifted() {
    let size = "box-sizing:border-box;padding:0;width:14px;height:14px";
    for scale in SCALES {
        for va in ["middle", "text-top", "text-bottom", "top", "bottom"] {
            let style = format!("{size};vertical-align:{va};margin:2px 0 8px 0");
            let radio = open(&radio_line(&style), scale, 300, 80);
            let twin = format!(
                "<p style=\"margin:10px\">Term {PROBE}<span id=\"r\" style=\"display:inline-block;{style}\"></span>\
                 <label>12</label></p>"
            );
            let twin = open(&twin, scale, 300, 80);
            assert_eq!(
                radio.test_element_box("r"),
                twin.test_element_box("r"),
                "scale {scale} vertical-align:{va}"
            );
        }
    }
}

/// A radio that is not part of a line (flex item) has no baseline to move to:
/// it keeps its box.
#[test]
fn radio_outside_a_line_is_not_shifted() {
    let body = "<div style=\"display:flex;align-items:flex-start;margin:10px\">\
        <input type=\"radio\" id=\"r\" name=\"g\" style=\"margin:0 0 6px 0\"></div>";
    for scale in SCALES {
        let e = open(body, scale, 300, 80);
        assert_eq!(top(&e, "r"), 10.0, "scale {scale}");
    }
}

// ---- ring and dot geometry --------------------------------------------

struct Ink {
    /// Sum of coverage over the zone, in device px².
    area: f64,
    cx: f64,
    cy: f64,
}

/// Coverage-weighted area and centroid of non-white pixels of `img` whose
/// centre is within `[r0, r1)` device px of `(cx, cy)`. `ink_g` is the
/// green channel of the ink colour (white is 255).
fn ink(img: &RgbaImage, c: (f64, f64), r0: f64, r1: f64, ink_g: f64) -> Ink {
    let (mut area, mut sx, mut sy) = (0.0, 0.0, 0.0);
    let reach = r1.ceil() as i64 + 1;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let x = c.0.floor() as i64 + dx;
            let y = c.1.floor() as i64 + dy;
            if x < 0 || y < 0 || x >= img.width as i64 || y >= img.height as i64 {
                continue;
            }
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let r = ((px - c.0).powi(2) + (py - c.1).powi(2)).sqrt();
            if r < r0 || r >= r1 {
                continue;
            }
            let g = img.pixels[((y as u32 * img.width + x as u32) * 4 + 1) as usize] as f64;
            let cov = ((255.0 - g) / (255.0 - ink_g)).clamp(0.0, 1.0);
            area += cov;
            sx += cov * px;
            sy += cov * py;
        }
    }
    Ink {
        area,
        cx: if area > 0.0 { sx / area } else { f64::NAN },
        cy: if area > 0.0 { sy / area } else { f64::NAN },
    }
}

fn rel(got: f64, want: f64) -> f64 {
    (got - want).abs() / want
}

/// Ring and dot areas of a `size`×`size` CSS-px radio painted into `img` at
/// `scale`, divided by `π k²` (k = `(size - 4) / 16 * scale`, the ring unit):
/// nominally 28 for the ring and 16 for the dot (ring radii 8 and 6, dot 4).
/// Also asserts the ring is centred on the radio's border box.
fn measure(e: &Engine, img: &RgbaImage, size: f64, checked: bool, scale: f64) -> (f64, f64) {
    let (x, y, w, h) = e.test_element_box("r").expect("radio box");
    assert!(
        (w as f64 - size).abs() < 0.01 && (h as f64 - size).abs() < 0.01,
        "radio box {w}x{h}, want {size}"
    );
    let c = (
        (x as f64 + size / 2.0) * scale,
        (y as f64 + size / 2.0) * scale,
    );
    let k = (size - 4.0) / 16.0 * scale;
    let unit = std::f64::consts::PI * k * k;
    let reach = 8.0 * k + 1.5;
    let what = format!("size {size} checked {checked} scale {scale}");
    let near = |i: &Ink, what2: &str| {
        assert!(
            (i.cx - c.0).abs() < 0.6 && (i.cy - c.1).abs() < 0.6,
            "{what}: {what2} centre {:?} want {c:?}",
            (i.cx, i.cy)
        );
    };
    if checked {
        // Accent colour is the text colour: rgb(200, 0, 0) -> green 0.
        let dot = ink(img, c, 0.0, 5.0 * k, 0.0);
        let ring = ink(img, c, 5.0 * k, reach, 0.0);
        near(&dot, "dot");
        near(&ring, "ring");
        (ring.area / unit, dot.area / unit)
    } else {
        // Ring is gray (128).
        let ring = ink(img, c, 0.0, reach, 128.0);
        near(&ring, "ring");
        (ring.area / unit, 0.0)
    }
}

/// Paint `body` at each scale: the ring and dot cover the same CSS-px area as
/// at the base scale (1.5), and the geometry at scale 1 is the nominal one (AA slack).
fn check_geometry(body: &str, size: f64, checked: bool) {
    // Scale 1 is the thin case where anti-aliasing is least exact, so the
    // CSS-px areas are compared against the first scale >= 1.5.
    let mut base: Option<(f64, f64)> = None;
    for scale in [1.5, 1.0, 2.0] {
        let e = open(body, scale, 120, 80);
        let img = e.last_frame().expect("frame");
        let (ring, dot) = measure(&e, img, size, checked, scale);
        let what = format!("size {size} checked {checked} scale {scale}");
        let (b_ring, b_dot) = *base.get_or_insert((ring, dot));
        if scale == 1.0 {
            assert!(rel(ring, 28.0) < 0.15, "{what}: ring {ring} units");
            assert!(!checked || rel(dot, 16.0) < 0.15, "{what}: dot {dot} units");
            continue;
        }
        assert!(
            rel(ring, 28.0) < 0.15,
            "{what}: ring {ring} units, want ~28"
        );
        assert!(
            rel(ring, b_ring) < 0.05,
            "{what}: ring {ring} units vs {b_ring} at the base scale"
        );
        if checked {
            assert!(rel(dot, 16.0) < 0.15, "{what}: dot {dot} units, want ~16");
            assert!(
                rel(dot, b_dot) < 0.05,
                "{what}: dot {dot} units vs {b_dot} at the base scale"
            );
        }
    }
}

fn abs_radio(size: u32, checked: bool) -> String {
    format!(
        "<input type=\"radio\" id=\"r\" name=\"g\"{} style=\"position:absolute;left:20px;top:20px;\
         margin:0;padding:0;box-sizing:border-box;width:{size}px;height:{size}px;color:rgb(200,0,0)\">",
        if checked { " checked" } else { "" }
    )
}

/// The ring and dot are the same size in CSS px at every scale (issue #116:
/// the ring was 10 CSS px at scale 1 and 12 at scale 2).
#[test]
fn radio_ring_and_dot_are_scale_invariant() {
    for size in [14.0, 20.0] {
        for checked in [false, true] {
            check_geometry(&abs_radio(size as u32, checked), size, checked);
        }
    }
}

/// The same holds for a radio laid out on a line (its baseline-shifted box
/// is where the ring is painted).
#[test]
fn inline_radio_ring_follows_the_shifted_box() {
    for checked in [false, true] {
        {
            let body = format!(
                "<p style=\"margin:10px\">Term<br><input type=\"radio\" id=\"r\" name=\"g\"{} \
                 style=\"margin:0 0 5px 0;padding:0;box-sizing:border-box;width:14px;height:14px;color:rgb(200,0,0)\"></p>",
                if checked { " checked" } else { "" }
            );
            check_geometry(&body, 14.0, checked);
        }
    }
}

/// A partial (exact) repaint of the radio's rect paints the same ring as the
/// full paint: same CSS-px areas, centred on the box.
///
/// Shifted mode is not covered: its cull compares shifted shapes with an
/// unshifted clip and drops small shapes (the dot) at some scales, with or
/// without this fix. Reported on the PR as a separate finding.
#[test]
fn partial_repaint_paints_the_same_ring() {
    for mode in [PaintMode::Exact] {
        for scale in [1.5, 2.0] {
            let html = format!(
                "<!DOCTYPE html><html><body style=\"margin:0;background:#fff\">{}</body></html>",
                abs_radio(14, true)
            );
            let mut e = Engine::new(EngineConfig {
                paint_mode: mode,
                ..Default::default()
            });
            let vp = viewport(scale, 120, 80);
            e.open_surface(
                SurfaceId::DEFAULT,
                &html,
                vp.clone(),
                SurfaceOpts {
                    visible: true,
                    pinned: true,
                },
            )
            .unwrap();
            e.paint(vp.clone()).unwrap();
            let full = e.last_frame().unwrap().clone();
            // Damage only the radio's rect (device px).
            e.test_push_pending_damage(Rect {
                x: (18.0 * scale) as f32,
                y: (18.0 * scale) as f32,
                width: (18.0 * scale) as f32,
                height: (18.0 * scale) as f32,
            });
            let out = e.paint(vp).unwrap();
            assert!(!out.skipped, "{mode:?} {scale}: radio repainted");
            assert!(!out.damage.is_empty(), "{mode:?} {scale}: damage reported");
            let (fr, fd) = measure(&e, &full, 14.0, true, scale);
            let (pr, pd) = measure(&e, e.last_frame().unwrap(), 14.0, true, scale);
            assert!(
                rel(pr, fr) < 0.03,
                "{mode:?} scale {scale}: ring {pr} vs {fr}"
            );
            assert!(
                rel(pd, fd) < 0.03,
                "{mode:?} scale {scale}: dot {pd} vs {fd}"
            );
            assert!(
                rel(pr, 28.0) < 0.1 && rel(pd, 16.0) < 0.1,
                "{mode:?} {scale}: nominal"
            );
        }
    }
}
