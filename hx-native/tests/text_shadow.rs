//! Issue #57: interim zero-blur `text-shadow` paint (`src/text_shadow.rs`).
//!
//! Text uses the bundled DejaVu Sans. Chrome numbers were taken with headless
//! Chrome at scale 1 drawing the same markup in DejaVu Sans (see the PR).

use std::collections::HashMap;

use hx_native::*;

fn cfg(mode: PaintMode) -> EngineConfig {
    EngineConfig {
        max_paint_ns: 200_000_000,
        pending_timeout_frames: 5,
        paint_mode: mode,
        ..EngineConfig::default()
    }
}

fn page(css: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff;color:#000}}\
         body{{font-family:\"DejaVu Sans\";font-size:20px}}{css}</style></head><body>{body}</body></html>"
    )
}

fn paint_at(html: &str, w: u32, h: u32, scale: f64) -> RgbaImage {
    let mut e = Engine::new(cfg(PaintMode::Exact));
    e.load_html(html).unwrap();
    let vw = (w as f64 * scale).round() as u32;
    let vh = (h as f64 * scale).round() as u32;
    let out = e.paint(Viewport::with_scale(vw, vh, scale)).unwrap();
    out.rgba
        .unwrap_or_else(|| e.last_frame().expect("frame").clone())
}

fn paint(html: &str, w: u32, h: u32) -> RgbaImage {
    paint_at(html, w, h, 1.0)
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
}

fn count(img: &RgbaImage, r: (u32, u32, u32, u32), f: impl Fn([u8; 3]) -> bool) -> usize {
    let mut n = 0;
    for y in r.1..r.3.min(img.height) {
        for x in r.0..r.2.min(img.width) {
            if f(px(img, x, y)) {
                n += 1;
            }
        }
    }
    n
}

/// Bounding box (x0, y0, x1, y1) of the pixels matching `f`.
fn bbox(img: &RgbaImage, f: impl Fn([u8; 3]) -> bool) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..img.height {
        for x in 0..img.width {
            if f(px(img, x, y)) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
                });
            }
        }
    }
    b
}

fn reddish(p: [u8; 3]) -> bool {
    p[0] > 160 && p[1] < 90 && p[2] < 90
}
fn bluish(p: [u8; 3]) -> bool {
    p[2] > 160 && p[0] < 90 && p[1] < 90
}
fn dark(p: [u8; 3]) -> bool {
    p[0] < 60 && p[1] < 60 && p[2] < 60
}

/// The #770 repro (`paint_gaps_repro.rs::d_emboss`): Win95 greyed text,
/// `#808080` on `#c0c0c0` with a 1px white emboss. Chrome (DejaVu Sans)
/// lights 148 pixels in each shadowed row and none in the control row.
#[test]
fn win95_emboss_rows_match_chrome() {
    let html = "<!DOCTYPE html><html><head><style>html,body{margin:0;background:#c0c0c0}\
        body{font-family:\"DejaVu Sans\";font-size:11px;color:#000}\
        button{display:block;margin:0;border:0;padding:2px 6px;font:inherit;color:inherit;background:#c0c0c0;position:absolute;left:4px}\
        button[aria-disabled=true]{color:#808080;text-shadow:1px 1px #fff}\
        button:disabled{color:#808080;text-shadow:1px 1px 0 #fff}</style></head><body>\
        <button aria-disabled=\"true\" style=\"top:4px\">Confirm exit</button>\
        <button disabled style=\"top:24px\">Confirm exit</button>\
        <button style=\"top:44px;color:#808080\">Confirm exit</button></body></html>";
    let img = paint(html, 100, 64);
    let lighter = |y0| count(&img, (4, y0, 100, y0 + 18), |p| p.iter().all(|&c| c > 200));
    let (aria, disabled, control) = (lighter(4), lighter(24), lighter(44));
    println!("lighter-than-bg px: aria {aria}, :disabled {disabled}, control {control} (Chrome 148/148/0)");
    assert_eq!(control, 0, "no shadow, no light pixels");
    assert_eq!(aria, disabled, "both rule forms paint the same emboss");
    let chrome = 148i64;
    assert!(
        (aria as i64 - chrome).abs() <= chrome / 10,
        "emboss must light about as many pixels as Chrome: {aria} vs {chrome}"
    );
}

/// Several shadows paint in CSS order: the first one listed is on top.
#[test]
fn multiple_shadows_first_on_top() {
    let css = "p{margin:10px;text-shadow:4px 4px #f00,4px 4px #00f,8px 8px #00f}";
    let img = paint(&page(css, "<p>Hi</p>"), 120, 60);
    let red = count(&img, (0, 0, 120, 60), reddish);
    let blue = count(&img, (0, 0, 120, 60), bluish);
    println!("red {red}, blue {blue}");
    assert!(red > 20, "the first shadow (red) must paint: {red}");
    // Blue only shows where the 8px shadow pokes out past the 4px ones.
    assert!(blue > 0, "the third shadow must paint: {blue}");
    let same_offset = page(
        "p{margin:10px;text-shadow:4px 4px #f00,4px 4px #00f}",
        "<p>Hi</p>",
    );
    let img2 = paint(&same_offset, 120, 60);
    assert_eq!(
        count(&img2, (0, 0, 120, 60), bluish),
        0,
        "a later shadow at the same offset is hidden under the first"
    );
}

/// Negative offsets move the shadow up and left of the text.
#[test]
fn negative_offset() {
    let plain = paint(&page("p{margin:20px}", "<p>Hi</p>"), 120, 70);
    let img = paint(
        &page("p{margin:20px;text-shadow:-5px -6px #f00}", "<p>Hi</p>"),
        120,
        70,
    );
    let t = bbox(&plain, dark).expect("text ink");
    let s = bbox(&img, reddish).expect("shadow ink");
    println!("text {t:?}, shadow {s:?}");
    assert!(
        t.0 - s.0 >= 4 && t.0 - s.0 <= 6,
        "x moves by -5: {t:?} {s:?}"
    );
    assert!(
        t.1 - s.1 >= 5 && t.1 - s.1 <= 7,
        "y moves by -6: {t:?} {s:?}"
    );
}

/// `text-shadow` is inherited: a shadow on an ancestor paints under a child
/// span's text; `text-shadow:none` on the child turns it off.
#[test]
fn inherited_and_overridden() {
    let on = paint(
        &page(
            "div{margin:10px;text-shadow:3px 3px #f00}",
            "<div><span>Hi</span></div>",
        ),
        120,
        50,
    );
    let off = paint(
        &page(
            "div{margin:10px;text-shadow:3px 3px #f00}span{text-shadow:none}",
            "<div><span>Hi</span></div>",
        ),
        120,
        50,
    );
    assert!(
        count(&on, (0, 0, 120, 50), reddish) > 20,
        "inherited shadow paints"
    );
    assert_eq!(
        count(&off, (0, 0, 120, 50), reddish),
        0,
        "none on the child wins"
    );
}

/// Blurred shadows are skipped (documented); zero-blur ones on the same
/// list still paint.
#[test]
fn blurred_shadow_is_skipped() {
    let blur = paint(
        &page("p{margin:10px;text-shadow:3px 3px 2px #f00}", "<p>Hi</p>"),
        120,
        50,
    );
    assert_eq!(
        count(&blur, (0, 0, 120, 50), |p| p[0] as i16 > p[1] as i16 + 30),
        0
    );
    let mixed = paint(
        &page(
            "p{margin:10px;text-shadow:3px 3px 2px #00f,3px 3px #f00}",
            "<p>Hi</p>",
        ),
        120,
        50,
    );
    assert!(count(&mixed, (0, 0, 120, 50), reddish) > 20);
    assert_eq!(
        count(&mixed, (0, 0, 120, 50), |p| p[2] as i16 > p[0] as i16 + 30),
        0
    );
}

/// Identical text in identical boxes, only one with a shadow: only that one
/// gets it (runs are matched by origin, not only by glyphs).
#[test]
fn identical_runs_only_shadow_their_own_node() {
    let css = "b{position:absolute;left:10px;font-weight:400}.s{text-shadow:3px 3px #f00}";
    let body =
        "<b style='top:5px'>Hi</b><b class='s' style='top:40px'>Hi</b><b style='top:75px'>Hi</b>";
    let img = paint(&page(css, body), 80, 110);
    let row = |y0| count(&img, (0, y0, 80, y0 + 35), reddish);
    assert_eq!(row(0), 0, "first copy has no shadow");
    assert!(row(35) > 20, "the shadowed copy paints red");
    assert_eq!(row(70), 0, "last copy has no shadow");
}

/// Offsets are CSS px: at scale 2 the shadow sits twice as far in device px.
/// Also under a CSS transform and inside a scrolled container.
#[test]
fn scale_transform_and_scroll() {
    for scale in [1.0, 2.0] {
        let plain = paint_at(&page("p{margin:10px}", "<p>Hi</p>"), 120, 50, scale);
        let img = paint_at(
            &page("p{margin:10px;text-shadow:4px 0 #f00}", "<p>Hi</p>"),
            120,
            50,
            scale,
        );
        let t = bbox(&plain, dark).unwrap();
        let s = bbox(&img, reddish).unwrap();
        let want = (4.0 * scale) as i64;
        let got = s.2 as i64 - t.2 as i64;
        assert!(
            (got - want).abs() <= 1,
            "scale {scale}: right edge moves {got}, want {want}"
        );
    }
    let transformed = paint(
        &page(
            "p{margin:10px;transform:translate(17px,9px) rotate(3deg);text-shadow:3px 3px #f00}",
            "<p>Hi</p>",
        ),
        140,
        70,
    );
    assert!(
        count(&transformed, (0, 0, 140, 70), reddish) > 20,
        "transformed text keeps its shadow"
    );
    let scrolled = paint(
        &page(
            "#s{height:40px;overflow:hidden;margin:0}#s p{margin:0;line-height:30px;text-shadow:3px 3px #f00}",
            "<div id='s'><p>A</p><p>Hi</p><p>B</p></div>",
        ),
        120,
        40,
    );
    assert!(
        count(&scrolled, (0, 0, 120, 40), reddish) > 20,
        "text in an overflow box keeps its shadow"
    );
}

struct Slices(HashMap<String, u64>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}
struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, _section: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

const SWAP_PAGE: &str = "<!DOCTYPE html><html><head><style>html,body{margin:0;background:#fff;color:#000}\
    body{font-family:\"DejaVu Sans\";font-size:20px}#sec{position:absolute;left:20px;top:20px}\
    .s{text-shadow:7px 6px #f00}.n{text-shadow:-6px -7px #00f}.far{text-shadow:24px 18px #f00}</style></head><body>\
    <section id=\"sec\" hx-trigger=\"slice:t\" hx-target=\"#sec\" hx-swap=\"innerHTML\"><b>Hi</b></section></body></html>";

fn swap(mode: PaintMode, scale: f64, before: &str, after: &str) -> (RgbaImage, RgbaImage) {
    // Large enough that a swap of #sec stays a partial repaint.
    let view = Viewport::with_scale((400.0 * scale) as u32, (300.0 * scale) as u32, scale);
    let run = |steps: &[&str], force_full: bool| {
        let mut e = Engine::new(cfg(mode));
        e.load_html(SWAP_PAGE).unwrap();
        let mut slices = Slices(HashMap::from([("t".to_string(), 1)]));
        let mut last = None;
        for (i, html) in steps.iter().enumerate() {
            slices.0.insert("t".into(), 2 + i as u64);
            e.sync(&slices, &mut Frags(html.to_string())).unwrap();
            if force_full && i + 1 == steps.len() {
                e.mark_theme_or_stylesheet_changed();
            }
            let out = e.paint(view.clone()).unwrap();
            if !force_full && i > 0 {
                assert!(!e.last_paint_stats().full_repaint, "swap must stay partial");
            }
            last = Some(out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone()));
        }
        last.unwrap()
    };
    (run(&[before, after], false), run(&[after], true))
}

/// Pixels that differ by more than `tol` in any channel. Exact must be
/// byte-identical (tol 0); Shifted is documented to differ by up to 1/255.
fn diffs(a: &RgbaImage, b: &RgbaImage, tol: u8) -> usize {
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > tol))
        .count()
}

/// A swap that removes, adds or moves a shadow repaints exactly what a full
/// repaint paints (Exact byte-identical, Shifted within its documented 1/255,
/// scale 1 and 2): damage covers the
/// shadow offset on both sides.
#[test]
fn swap_partial_equals_full() {
    let cases = [
        ("<b class=s>Hi</b>", "<b>Hi</b>"),
        ("<b>Hi</b>", "<b class=s>Hi</b>"),
        ("<b class=s>Hi</b>", "<b class=n>Hi</b>"),
        ("<b class=n>Hi</b>", "<span>Hi</span>"),
    ];
    for mode in [PaintMode::Exact, PaintMode::Shifted] {
        for scale in [1.0, 2.0] {
            for (before, after) in cases {
                let (partial, full) = swap(mode, scale, before, after);
                let tol = if mode == PaintMode::Exact { 0 } else { 1 };
                let d = diffs(&partial, &full, tol);
                assert_eq!(
                    d, 0,
                    "{mode:?} scale {scale}: {before} -> {after}: {d} px differ"
                );
            }
        }
    }
    // And the removal really erased the shadow.
    let (partial, _) = swap(PaintMode::Exact, 1.0, "<b class=s>Hi</b>", "<b>Hi</b>");
    assert_eq!(count(&partial, (0, 0, 140, 80), reddish), 0);
}

/// A shadow without a colour uses the text's `color` (currentcolor).
#[test]
fn colourless_shadow_uses_currentcolor() {
    let img = paint(
        &page("p{margin:10px;color:#f00;text-shadow:3px 3px}", "<p>Hi</p>"),
        120,
        50,
    );
    let plain = paint(&page("p{margin:10px;color:#f00}", "<p>Hi</p>"), 120, 50);
    let r = |i: &RgbaImage| count(i, (0, 0, 120, 50), reddish);
    assert!(
        r(&img) > r(&plain) + 20,
        "the shadow must be red too: {} vs {}",
        r(&img),
        r(&plain)
    );
}

/// Text in an anonymous block (a block with mixed inline and block children)
/// keeps its shadow through a swap: damage reads the runs' own nodes.
#[test]
fn anonymous_block_text_swap_partial_equals_full() {
    let (partial, full) = swap(
        PaintMode::Exact,
        1.0,
        "<b class=far>Hi</b><div>x</div>",
        "<b>Hi</b><div>x</div>",
    );
    assert_eq!(diffs(&partial, &full, 0), 0);
    assert_eq!(count(&partial, (0, 0, 140, 80), reddish), 0);
}

/// `align-content:center` moves the line boxes (Blitz `block_offset`); the
/// run origin follows it.
#[test]
fn align_content_offset_keeps_shadow() {
    let img = paint(
        &page(
            "div{height:60px;align-content:center;text-shadow:3px 3px #f00}",
            "<div>Hi</div>",
        ),
        120,
        70,
    );
    let t = bbox(&img, dark).expect("text");
    assert!(t.1 > 10, "text is centred, not at the top: {t:?}");
    assert!(count(&img, (0, 0, 120, 70), reddish) > 20);
}

/// Text directly inside a scrolled `overflow-y:auto` box (the inline root
/// scrolls itself) keeps its shadow after a wheel scroll.
#[test]
fn self_scrolled_inline_root_keeps_shadow() {
    let html = page(
        "#s{position:absolute;left:0;top:0;width:120px;height:40px;overflow-y:auto;line-height:20px;text-shadow:3px 3px #f00}",
        "<div id='s'>A<br>Hi<br>Hi<br>Hi<br>B</div>",
    );
    let mut e = Engine::new(cfg(PaintMode::Exact));
    e.load_html(&html).unwrap();
    let view = Viewport::with_scale(140, 50, 1.0);
    let before = e.paint(view.clone()).unwrap().rgba.unwrap();
    let _ = e.input(InputEvent::Wheel {
        pos: (20.0, 20.0),
        delta: (0.0, -30.0), // negative y scrolls down (#26)
        unit: WheelUnit::Pixel,
    });
    let out = e.paint(view).unwrap();
    let img = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    let red = count(&img, (0, 0, 110, 40), reddish);
    assert_ne!(before.pixels, img.pixels, "the wheel scrolled the box");
    assert!(red > 20, "scrolled text keeps its shadow: {red}");
}
