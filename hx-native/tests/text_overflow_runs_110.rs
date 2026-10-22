//! Issue #110: the `text-overflow: ellipsis` redraw keeps each inline run's own
//! style (weight, italic, colour, size, family from `<b>`, `<i>`, `<span>`),
//! measures the cut with those styles, and draws the "…" in the style of the
//! last visible run (Chrome's behaviour).
//!
//! Oracle: Blitz itself. An ellipsis paint must be byte-identical to Blitz
//! painting the same markup with the cut already applied and `text-overflow:
//! clip`, so every run is shaped and painted by Blitz's own per-span path.

use std::collections::HashMap;

use hx_native::*;

const LONG: &str = "MCI Communications Corporation";
const ELLIPSIS: char = '\u{2026}';
const SCALES: [f32; 2] = [1.0, 1.25];

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 10_000_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

/// Mainnet Deals table: DejaVu 11px, black on white, 150px cells.
fn table_page(extra_css: &str, rows: &[(&str, &str, &str)]) -> String {
    let mut trs = String::new();
    for (id, class, inner) in rows {
        trs.push_str(&format!(
            "<tr><td id=\"{id}\" class=\"{class}\">{inner}</td></tr>"
        ));
    }
    format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"/><style>
* {{ margin:0; }}
body {{ background:#fff; color:#000; font-family:"DejaVu Sans",sans-serif; font-size:11px; }}
.t98 {{ border-collapse:collapse; background:#fff; }}
.t98 td {{ padding:2px 5px; height:17px; white-space:nowrap; width:150px; max-width:150px;
  overflow:hidden; text-overflow:ellipsis; text-align:left; }}
.t98 td.clip {{ text-overflow:clip; }}
{extra_css}
</style></head><body><table class="t98">{trs}</table></body></html>"##
    )
}

fn view(scale: f32) -> Viewport {
    Viewport::with_scale((400.0 * scale) as u32, (160.0 * scale) as u32, scale.into())
}

fn boot(html: &str) -> Engine {
    let mut e = Engine::new(cfg());
    e.declare_actions(&[]);
    e.load_html(html).expect("load");
    e
}

fn paint(e: &mut Engine, scale: f32) -> RgbaImage {
    let out = e.paint(view(scale)).expect("paint");
    out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone())
}

fn dark(p: &[u8]) -> bool {
    (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 128
}

/// Dark pixels in the first `w` CSS px of `id`'s content box (5px padding).
fn ink(e: &Engine, img: &RgbaImage, id: &str, w: f32, scale: f32) -> u32 {
    let (x, y, _bw, h) = e.test_element_box(id).expect(id);
    let x0 = ((x + 5.0) * scale) as u32;
    let x1 = ((x + 5.0 + w) * scale) as u32;
    let mut n = 0;
    for py in (y * scale) as u32..((y + h) * scale) as u32 {
        for px in x0..x1 {
            let i = ((py * img.width + px) * 4) as usize;
            if dark(&img.pixels[i..i + 4]) {
                n += 1;
            }
        }
    }
    n
}

fn diff(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count()
}

/// Paint `inner` with ellipsis, read the cut, and return
/// `(truncated text, ellipsis image, image of Blitz painting `reference(prefix)` with clip)`.
fn ellipsis_vs_blitz(
    scale: f32,
    inner: &str,
    reference: impl Fn(&str) -> String,
) -> (String, RgbaImage, RgbaImage) {
    let mut e = boot(&table_page("", &[("c", "", inner)]));
    let img = paint(&mut e, scale);
    let truncated = e
        .test_ellipsis_truncated("c")
        .unwrap_or_else(|| panic!("s{scale} {inner}: expected truncation"));
    assert!(truncated.ends_with(ELLIPSIS), "{truncated:?}");
    let mut r = boot(&table_page("", &[("c", "clip", &reference(&truncated))]));
    let ref_img = paint(&mut r, scale);
    (truncated, img, ref_img)
}

#[test]
fn clipped_bold_ellipsis_has_same_first_40px_ink_as_clip() {
    for scale in SCALES {
        let bold = format!("<b>{LONG}</b>");
        let mut e = boot(&table_page(
            "",
            &[
                ("cut", "", &bold),
                ("clip", "clip", &bold),
                ("norm", "", LONG),
            ],
        ));
        let img = paint(&mut e, scale);
        assert!(e.test_ellipsis_truncated("cut").is_some(), "s{scale}: cut");
        let cut = ink(&e, &img, "cut", 40.0, scale);
        let clip = ink(&e, &img, "clip", 40.0, scale);
        let norm = ink(&e, &img, "norm", 40.0, scale);
        eprintln!("s{scale}: first-40px ink cut={cut} clip={clip} regular={norm}");
        assert_eq!(
            cut, clip,
            "s{scale}: ellipsis <b> must keep bold ink in its first 40px \
             (cut={cut} clip={clip} regular={norm})"
        );
        assert!(
            cut > norm,
            "s{scale}: bold ink must exceed regular (cut={cut} regular={norm})"
        );
    }
}

/// Widths in CSS px of `<b>prefix…</b>` (or regular) for every cluster prefix,
/// laid out by Blitz as shrink-to-fit inline-blocks.
fn measured_cut(scale: f32, bold: bool) -> String {
    let prefixes: Vec<&str> = LONG
        .char_indices()
        .map(|(i, _)| &LONG[..i])
        .chain([LONG])
        .collect();
    let mut spans = String::new();
    for (i, p) in prefixes.iter().enumerate() {
        let inner = if bold {
            format!("<b>{p}{ELLIPSIS}</b>")
        } else {
            format!("{p}{ELLIPSIS}")
        };
        spans.push_str(&format!(
            "<div><span id=\"m{i}\" style=\"display:inline-block;white-space:pre\">{inner}</span></div>"
        ));
    }
    let html = format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"/><style>
* {{ margin:0; }}
body {{ background:#fff; color:#000; font-family:"DejaVu Sans",sans-serif; font-size:11px; }}
</style></head><body>{spans}</body></html>"##
    );
    let mut e = boot(&html);
    let _ = e.paint(Viewport::with_scale(
        (400.0 * scale) as u32,
        (1200.0 * scale) as u32,
        scale.into(),
    ));
    let mut best = "";
    for (i, p) in prefixes.iter().enumerate() {
        let (_, _, w, _) = e.test_element_box(&format!("m{i}")).unwrap();
        if w <= 150.0 + 0.5 {
            best = p;
        }
    }
    format!("{best}{ELLIPSIS}")
}

#[test]
fn bold_cut_point_matches_measuring_in_bold() {
    for scale in SCALES {
        let (truncated, img, ref_img) =
            ellipsis_vs_blitz(scale, &format!("<b>{LONG}</b>"), |t| format!("<b>{t}</b>"));
        let bold_cut = measured_cut(scale, true);
        let regular_cut = measured_cut(scale, false);
        eprintln!("s{scale}: truncated={truncated:?} bold={bold_cut:?} regular={regular_cut:?}");
        assert_eq!(
            truncated, bold_cut,
            "s{scale}: cut must match bold measurement (regular would give {regular_cut:?})"
        );
        let d = diff(&img, &ref_img);
        assert_eq!(
            d, 0,
            "s{scale}: ellipsis <b> must be byte-identical to Blitz painting \
             <b>{truncated}</b> (diff={d})"
        );
    }
}

#[test]
fn mixed_regular_then_bold_runs_keep_each_weight() {
    for scale in SCALES {
        let inner = format!("Deal: <b>{LONG}</b>");
        let (truncated, img, ref_img) = ellipsis_vs_blitz(scale, &inner, |t| {
            let rest = t.strip_prefix("Deal: ").expect("cut lands in the bold run");
            format!("Deal: <b>{rest}</b>")
        });
        let d = diff(&img, &ref_img);
        assert_eq!(
            d, 0,
            "s{scale}: regular+bold runs must match Blitz (truncated={truncated:?} diff={d})"
        );
        // Discriminates: the flattened (all-regular) paint is visibly different.
        let mut flat = boot(&table_page("", &[("c", "clip", &truncated)]));
        let flat_img = paint(&mut flat, scale);
        assert!(
            diff(&img, &flat_img) > 0,
            "s{scale}: bold run was flattened"
        );
    }
}

/// Is `p` a saturated red (`#c00` text, antialiased)?
fn reddish(p: &[u8]) -> bool {
    p[0] > 150 && p[1] < 110 && p[2] < 110 && p[0] as i32 - p[1] as i32 > 80
}

/// Pixels of the rightmost 2.5 CSS px of ink inside `id` (the last "…" dot).
fn last_dot_pixels(e: &Engine, img: &RgbaImage, id: &str, scale: f32) -> Vec<[u8; 4]> {
    let (x, y, w, h) = e.test_element_box(id).unwrap();
    let (x0, x1) = ((x * scale) as u32, ((x + w) * scale) as u32);
    let (y0, y1) = ((y * scale) as u32, ((y + h) * scale) as u32);
    let inked = |p: &[u8]| p[0] < 200 || p[1] < 200 || p[2] < 200;
    let mut right = x0;
    for py in y0..y1 {
        for px in x0..x1 {
            let i = ((py * img.width + px) * 4) as usize;
            if inked(&img.pixels[i..i + 4]) {
                right = right.max(px);
            }
        }
    }
    let from = right.saturating_sub((2.5 * scale) as u32);
    let mut out = Vec::new();
    for py in y0..y1 {
        for px in from..=right {
            let i = ((py * img.width + px) * 4) as usize;
            let p = &img.pixels[i..i + 4];
            if inked(p) {
                out.push([p[0], p[1], p[2], p[3]]);
            }
        }
    }
    out
}

#[test]
fn ellipsis_takes_the_last_visible_runs_style() {
    for scale in SCALES {
        // Cut lands in a red bold run: the "…" is red and bold.
        let inner = format!("Deal <b style=\"color:#c00\">{LONG}</b>");
        let (truncated, img, ref_img) = ellipsis_vs_blitz(scale, &inner, |t| {
            let rest = t.strip_prefix("Deal ").unwrap();
            format!("Deal <b style=\"color:#c00\">{rest}</b>")
        });
        let d = diff(&img, &ref_img);
        assert_eq!(
            d, 0,
            "s{scale}: red bold ellipsis (truncated={truncated:?} diff={d})"
        );
        let e = boot(&table_page("", &[("c", "", &inner)]));
        let mut e = e;
        let img2 = paint(&mut e, scale);
        let dot = last_dot_pixels(&e, &img2, "c", scale);
        assert!(
            !dot.is_empty() && dot.iter().any(|p| reddish(p)),
            "s{scale}: last dot must be red: {dot:?}"
        );

        // Cut lands in a regular run after a red bold one: the "…" is regular black.
        let inner = "<b style=\"color:#c00\">MCI</b> communications corporation of america";
        let (truncated, img, ref_img) = ellipsis_vs_blitz(scale, inner, |t| {
            let rest = t.strip_prefix("MCI").unwrap();
            format!("<b style=\"color:#c00\">MCI</b>{rest}")
        });
        let d = diff(&img, &ref_img);
        assert_eq!(
            d, 0,
            "s{scale}: regular ellipsis (truncated={truncated:?} diff={d})"
        );
        let mut e = boot(&table_page("", &[("c", "", inner)]));
        let img2 = paint(&mut e, scale);
        let dot = last_dot_pixels(&e, &img2, "c", scale);
        assert!(
            !dot.is_empty() && !dot.iter().any(|p| reddish(p)),
            "s{scale}: last dot must not be red: {dot:?}"
        );

        // Italic / size / family from a <span> survive too.
        let inner = format!(
            "<span style=\"font-style:italic;font-size:13px;font-family:'DejaVu Serif',serif\">{LONG}</span>"
        );
        let (truncated, img, ref_img) = ellipsis_vs_blitz(scale, &inner, |t| {
            format!(
                "<span style=\"font-style:italic;font-size:13px;font-family:'DejaVu Serif',serif\">{t}</span>"
            )
        });
        let d = diff(&img, &ref_img);
        assert_eq!(
            d, 0,
            "s{scale}: italic span (truncated={truncated:?} diff={d})"
        );
    }
}

// --- partial repaint after a swap == fresh full paint -------------------------

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
struct Frag(String);
impl FragmentSource for Frag {
    fn render(&mut self, _section: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

fn swap_page() -> String {
    let mut rows = String::new();
    for i in 0..6 {
        rows.push_str(&format!("<p style='margin:0'>Row {i} <b>bold</b> text</p>"));
    }
    format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"/><style>
* {{ margin:0; }}
body {{ background:#fff; color:#000; font-family:"DejaVu Sans",sans-serif; font-size:11px; }}
#sec {{ position:absolute; left:120px; top:40px; width:160px; padding:2px 5px; height:17px;
  box-sizing:border-box; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }}
</style></head><body>{rows}<section id="sec" hx-trigger="slice:t" hx-target="#sec"
 hx-swap="innerHTML">a</section></body></html>"##
    )
}

fn swap_engine() -> Engine {
    let mut e = boot(&swap_page());
    e.declare_actions(&[]);
    e
}

fn full_of(html: &str, scale: f32) -> RgbaImage {
    let mut e = swap_engine();
    let _ = paint(&mut e, scale);
    let s = Slices(HashMap::from([("t".to_string(), 2)]));
    e.sync(&s, &mut Frag(html.into())).unwrap();
    e.mark_theme_or_stylesheet_changed();
    paint(&mut e, scale)
}

#[test]
fn partial_repaint_after_swap_equals_full_paint() {
    let steps = [
        format!("<b>{LONG}</b>"),
        format!("Deal: <b style=\"color:#c00\">{LONG}</b>"),
        format!("<i>{LONG}</i> and more"),
        "short".to_string(),
        format!("<b>{LONG}</b>"),
    ];
    for scale in SCALES {
        let mut e = swap_engine();
        let _ = paint(&mut e, scale);
        let mut slices = Slices(HashMap::from([("t".to_string(), 1)]));
        for (i, html) in steps.iter().enumerate() {
            slices.0.insert("t".into(), 2 + i as u64);
            e.sync(&slices, &mut Frag(html.clone())).unwrap();
            let partial = paint(&mut e, scale);
            assert!(
                !e.last_paint_stats().full_repaint,
                "s{scale} {html}: expected a partial repaint"
            );
            let full = full_of(html, scale);
            let d = diff(&partial, &full);
            assert_eq!(d, 0, "s{scale} {html}: partial != full (diff={d})");
        }
    }
}
