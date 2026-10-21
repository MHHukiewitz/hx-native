//! Issue #23: SVG icons. Inline `<svg>` and `data:image/svg+xml` CSS images
//! paint; the offline provider serves nothing else; SVGs with `<text>`,
//! `<image>`, too many bytes or bad markup reject the whole load or swap.
//!
//! Pixel counts are pinned to headless Chrome's for the same fixture
//! (`tests/fixtures/svg_icons.html`, which carries Mainnet's token icons
//! verbatim): crispEdges paths on integer coordinates give exact colours,
//! except the antialiased diagonal noted in `icon_counts`.

use std::collections::HashMap;

use hx_native::*;

const ICONS: &str = include_str!("fixtures/svg_icons.html");
const BOLD: &[u8] = include_bytes!("fonts/DejaVuSans-Bold.ttf");

const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const YELLOW: [u8; 3] = [255, 255, 0];
const NAVY: [u8; 3] = [0, 0, 128];
const RED: [u8; 3] = [255, 0, 0];
const PAGE: [u8; 3] = [255, 0, 255];

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 200_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

fn engine() -> Engine {
    let mut e = Engine::new(cfg());
    e.declare_actions(&["act.do"]);
    e
}

fn vp(css_w: u32, css_h: u32, scale: f64) -> Viewport {
    Viewport::with_scale(
        (css_w as f64 * scale).round() as u32,
        (css_h as f64 * scale).round() as u32,
        scale,
    )
}

fn paint(e: &mut Engine, css_w: u32, css_h: u32, scale: f64) -> RgbaImage {
    e.paint(vp(css_w, css_h, scale))
        .expect("paint")
        .rgba
        .expect("rgba")
}

fn render(html: &str, css_w: u32, css_h: u32, scale: f64) -> RgbaImage {
    let mut e = engine();
    e.load_html(html).expect("load_html");
    paint(&mut e, css_w, css_h, scale)
}

/// Colour histogram of a CSS-pixel region.
fn hist(
    img: &RgbaImage,
    scale: f64,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
) -> HashMap<[u8; 3], u32> {
    let s = scale as u32;
    let mut h = HashMap::new();
    for y in y0 * s..y1 * s {
        for x in x0 * s..x1 * s {
            let i = ((y * img.width + x) * 4) as usize;
            let px = [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]];
            *h.entry(px).or_insert(0) += 1;
        }
    }
    h
}

fn expect(h: &HashMap<[u8; 3], u32>, want: &[([u8; 3], u32)], what: &str) {
    let mut want_map = HashMap::new();
    for (c, n) in want {
        want_map.insert(*c, *n);
    }
    assert_eq!(
        h, &want_map,
        "{what}: colour counts differ from the Chrome-checked pin"
    );
}

fn icon_counts(scale: f64) {
    // First paint after load_html: the data: images must already be in.
    let img = render(ICONS, 220, 40, scale);
    let k = (scale * scale) as u32;
    // --icon-warn closes both paths with a diagonal edge ((6,2)->(7,0) and
    // (6,4)->(7,2)). Chrome snaps it under crispEdges; anyrender_svg/vello_cpu
    // ignore shape-rendering and antialias it, so column 6 rows 0-3 blend.
    // Chrome: page 103, yellow 81, black 72. Every other pixel of every icon
    // is identical to Chrome (checked against chrome-icons2-s{1,2}.png).
    let s = scale as u32;
    let warn = [
        (PAGE, 103 * k - s),
        (YELLOW, 81 * k - s),
        (BLACK, 72 * k - 2 * s),
        ([191, 0, 191], s),
        ([64, 0, 64], s),
        ([64, 64, 0], s),
        ([191, 191, 0], s),
    ];
    expect(&hist(&img, scale, (0, 0, 16, 16)), &warn, "--icon-warn");
    expect(
        &hist(&img, scale, (20, 0, 36, 16)),
        &[
            (WHITE, 103 * k),
            (PAGE, 93 * k),
            (BLACK, 43 * k),
            (NAVY, 17 * k),
        ],
        "--icon-info",
    );
    expect(
        &hist(&img, scale, (40, 0, 56, 16)),
        &[
            (RED, 124 * k),
            (PAGE, 48 * k),
            (BLACK, 44 * k),
            (WHITE, 40 * k),
        ],
        "--icon-stop",
    );
    expect(
        &hist(&img, scale, (60, 0, 76, 16)),
        &warn,
        "inline <svg> warn",
    );
    expect(
        &hist(&img, scale, (80, 0, 89, 9)),
        &[(WHITE, 64 * k), (BLACK, 17 * k)],
        "--icon-check-x (9x9, #46)",
    );
    expect(
        &hist(&img, scale, (92, 0, 99, 7)),
        &[(WHITE, 36 * k), (BLACK, 13 * k)],
        "--icon-check-x-sm (7x7, #46)",
    );
    expect(
        &hist(&img, scale, (0, 20, 200, 38)),
        &[(BLACK, 1800 * k), (WHITE, 1800 * k)],
        "path-only 1px checker over 200x18 (#45)",
    );
}

#[test]
fn token_icons_inline_svg_and_tiles_match_chrome_scale_1() {
    icon_counts(1.0);
}

#[test]
fn token_icons_inline_svg_and_tiles_match_chrome_scale_2() {
    icon_counts(2.0);
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

const WARN_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16' shape-rendering='crispEdges'><path fill='#000' d='M7 0h2v2h1v2h1v2h1v2h1v2h1v2h1v3h-1v1H1v-1H0v-3h1v-2h1V8h1V6h1V4h1V2h1z'/><path fill='#ff0' d='M7 2h2v2h1v2h1v2h1v2h1v2h1v2H1v-2h1v-2h1V8h1V6h1V4h1z'/></svg>";

fn one_bg_page(bg: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#f0f}}#a{{width:16px;height:16px;background:{bg}}}</style></head><body><div id="a"></div></body></html>"#
    )
}

#[test]
fn base64_and_percent_encoded_paint_the_same() {
    let pct = WARN_SVG
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('#', "%23");
    let a = render(
        &one_bg_page(&format!("url(\"data:image/svg+xml,{pct}\")")),
        32,
        32,
        1.0,
    );
    let b = render(
        &one_bg_page(&format!(
            "url(data:image/svg+xml;base64,{})",
            base64(WARN_SVG.as_bytes())
        )),
        32,
        32,
        1.0,
    );
    assert_eq!(a.pixels, b.pixels);
    assert!(
        hist(&a, 1.0, (0, 0, 16, 16))
            .get(&YELLOW)
            .copied()
            .unwrap_or(0)
            > 0
    );
}

#[test]
fn file_http_and_relative_urls_are_never_fetched() {
    let none = render(&one_bg_page("none"), 32, 32, 1.0);
    for bg in [
        "url(file:///etc/hostname)",
        "url(\"http://127.0.0.1:9/a.svg\")",
        "url(icons/a.svg)",
        "url(data:image/png;base64,iVBORw0KGgo=)",
    ] {
        let mut e = engine();
        e.load_html(&one_bg_page(bg)).expect("load");
        let img = paint(&mut e, 32, 32, 1.0);
        assert_eq!(
            img.pixels, none.pixels,
            "{bg} must paint like no background"
        );
        let (served, refused) = e.test_data_svg_counts();
        assert_eq!(served, 0, "{bg}");
        // A relative URL has no base (load_html sets none), so Blitz never
        // builds a request; absolute ones reach the provider and are dropped.
        if !bg.contains("icons/") {
            assert!(refused >= 1, "{bg} must reach the provider and be dropped");
        }
    }
}

fn text_page(font_face: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>{font_face}html,body{{margin:0;background:#fff}}p{{margin:0;font:20px "Hx Data",sans-serif;color:#000}}</style></head><body><p>Mainnet</p></body></html>"#
    )
}

#[test]
fn data_font_face_is_never_served() {
    let plain = render(&text_page(""), 160, 40, 1.0);
    let b64 = base64(BOLD);
    // A data: font with its own MIME type: dropped by the provider.
    let mut e = engine();
    e.load_html(&text_page(&format!(
        "@font-face{{font-family:\"Hx Data\";src:url(data:font/ttf;base64,{b64})}}"
    )))
    .expect("load");
    assert_eq!(
        paint(&mut e, 160, 40, 1.0).pixels,
        plain.pixels,
        "data:font/ttf must not be used"
    );
    assert_eq!(e.test_data_svg_counts().0, 0);
    // A font hidden behind the SVG MIME type: the whole load is refused
    // (the head of the font, so the size cap does not fire first).
    let head = base64(&BOLD[..4096]);
    let err = engine()
        .load_html(&text_page(&format!(
            "@font-face{{font-family:\"Hx Data\";src:url(data:image/svg+xml;base64,{head})}}"
        )))
        .unwrap_err();
    assert!(
        matches!(
            &err,
            EngineError::InvalidSvg {
                rejection: SvgRejection::Malformed { .. },
                ..
            }
        ),
        "{err:?}"
    );
    // A valid SVG offered as a truetype font: whatever Blitz does with it,
    // the text keeps the bundled face.
    let pct = WARN_SVG
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('#', "%23");
    let mut e = engine();
    e.load_html(&text_page(&format!(
        "@font-face{{font-family:\"Hx Data\";src:url(\"data:image/svg+xml,{pct}\") format(\"truetype\")}}"
    )))
    .expect("load");
    assert_eq!(paint(&mut e, 160, 40, 1.0).pixels, plain.pixels);
}

#[test]
fn data_css_link_is_ignored_and_never_blocks_paint() {
    let page = r#"<!DOCTYPE html><html><head><link rel="stylesheet" href="data:text/css,body%7Bbackground:%23f00%7D"><style>html,body{margin:0;background:#fff}#b{width:10px;height:10px;background:#000}</style></head><body><div id="b"></div></body></html>"#;
    let img = render(page, 32, 32, 1.0);
    let h = hist(&img, 1.0, (0, 0, 10, 10));
    assert_eq!(
        h.get(&BLACK).copied(),
        Some(100),
        "page must resolve and paint"
    );
    let h = hist(&img, 1.0, (12, 12, 32, 32));
    assert_eq!(
        h.get(&WHITE).copied(),
        Some(400),
        "data:text/css must not apply"
    );
}

fn assert_rejected(html: &str, element: &str) {
    let err = engine().load_html(html).unwrap_err();
    assert_eq!(
        err,
        match err.clone() {
            EngineError::InvalidSvg { id, .. } => EngineError::InvalidSvg {
                id,
                rejection: SvgRejection::ForbiddenElement {
                    element: element.into()
                },
            },
            other => panic!("expected InvalidSvg for <{element}>, got {other:?}"),
        }
    );
}

fn inline_page(svg_body: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><body><div id="host"><svg id="ic" xmlns="http://www.w3.org/2000/svg" width="16" height="16">{svg_body}</svg></div></body></html>"#
    )
}

fn data_page(svg_body: &str) -> String {
    let svg =
        format!("<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'>{svg_body}</svg>")
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace('#', "%23");
    format!(
        r#"<!DOCTYPE html><html><head><style>:root{{--i:url("data:image/svg+xml,{svg}")}}</style></head><body><div style="width:16px;height:16px;background:var(--i)"></div></body></html>"#
    )
}

#[test]
fn svg_text_rejects_the_load_inline_and_data() {
    for (body, el) in [
        ("<text x='0' y='10'>A</text>", "text"),
        ("<g><tspan>A</tspan></g>", "tspan"),
        ("<textPath href='#p'>A</textPath>", "textPath"),
    ] {
        assert_rejected(&inline_page(body), el);
        assert_rejected(&data_page(body), el);
    }
    // A prefixed element in a data: SVG.
    let svg = "%3Csvg xmlns='http://www.w3.org/2000/svg' xmlns:s='http://www.w3.org/2000/svg'%3E%3Cs:text%3EA%3C/s:text%3E%3C/svg%3E";
    let page = format!(
        r#"<!DOCTYPE html><html><body><div style="background:url(&quot;data:image/svg+xml,{svg}&quot;)"></div></body></html>"#
    );
    assert_rejected(&page, "text");
}

#[test]
fn svg_image_rejects_the_load_inline_and_data() {
    assert_rejected(
        &inline_page("<image href='/etc/hostname' width='16' height='16'/>"),
        "image",
    );
    assert_rejected(
        &inline_page("<filter id='f'><feImage href='/etc/hostname'/></filter>"),
        "feImage",
    );
    assert_rejected(
        &data_page("<image href='/etc/hostname' width='16' height='16'/>"),
        "image",
    );
    // An <img src> data: SVG goes through the same gate.
    let img = format!(
        r#"<!DOCTYPE html><html><body><img id="im" src="data:image/svg+xml,{}"></body></html>"#,
        "%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cimage href='x.png'/%3E%3C/svg%3E"
    );
    assert_rejected(&img, "image");
}

#[test]
fn oversize_and_malformed_data_svg_reject_the_load() {
    let big = format!("<path d='M0 0h1v1H0z'/>{}", " ".repeat(DATA_SVG_MAX_BYTES));
    let err = engine().load_html(&data_page(&big)).unwrap_err();
    assert!(
        matches!(err, EngineError::InvalidSvg { rejection: SvgRejection::TooLarge { limit, .. }, .. } if limit == DATA_SVG_MAX_BYTES),
        "{err:?}"
    );
    for bg in [
        "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cg%3E\")",
        "url(data:image/svg+xml;base64,@@@@)",
        // Escaped `data:` is decoded the way Stylo reads it.
        r"url(da\74 a:image/svg+xml,%3Csvg)",
    ] {
        let err = engine().load_html(&one_bg_page(bg)).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidSvg {
                    rejection: SvgRejection::Malformed { .. },
                    ..
                }
            ),
            "{bg}: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Swaps: partial == full, damage stays local, rejected fragments change nothing.

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
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        self.0.clone()
    }
}

fn shell(translate: bool) -> String {
    let tokens: String = ICONS
        .lines()
        .filter(|l| l.trim_start().starts_with("--icon-"))
        .collect::<Vec<_>>()
        .join("\n");
    let tf = if translate {
        "transform:translate(3px,2px);"
    } else {
        ""
    };
    format!(
        r##"<!DOCTYPE html><html><head><style>:root{{{tokens}}}
html,body{{margin:0;background:#c0c0c0}}
#other{{position:absolute;left:0;top:0;width:160px;height:20px;background:#000080}}
#sec{{position:absolute;left:20px;top:40px;width:120px;height:40px;{tf}}}
.ic{{display:inline-block;width:16px;height:16px;margin-right:4px}}
</style></head><body><div id="other"></div>
<section id="sec-s" hx-trigger="slice:hud" hx-target="#sec" hx-swap="innerHTML"><div id="sec"></div></section>
</body></html>"##
    )
}

fn icons_fragment() -> String {
    format!(
        r#"<span class="ic" id="f-warn" style="background:var(--icon-warn)"></span><span class="ic" id="f-stop" style="background:var(--icon-stop)"></span><span class="ic" id="f-inline">{WARN_SVG}</span>"#
    )
}

fn swapped(translate: bool, scale: f64) -> (Engine, PaintOutput) {
    let mut e = engine();
    e.load_html(&shell(translate)).unwrap();
    let mut slices = Slices(HashMap::from([("hud".to_string(), 1)]));
    let mut frags = Frags("<span id=\"f-empty\"></span>".into());
    e.sync(&slices, &mut frags).unwrap();
    e.paint(vp(160, 100, scale)).unwrap();
    slices.0.insert("hud".into(), 2);
    frags.0 = icons_fragment();
    e.sync(&slices, &mut frags).unwrap();
    let out = e.paint(vp(160, 100, scale)).unwrap();
    (e, out)
}

fn partial_equals_full(translate: bool, scale: f64) {
    let (e, partial) = swapped(translate, scale);
    assert!(
        !e.last_paint_stats().full_repaint,
        "icon swap must not full-repaint"
    );
    let mut f = engine();
    f.load_html(&shell(translate)).unwrap();
    let slices = Slices(HashMap::from([("hud".to_string(), 1)]));
    f.sync(&slices, &mut Frags(icons_fragment())).unwrap();
    f.mark_theme_or_stylesheet_changed();
    let full = f.paint(vp(160, 100, scale)).unwrap();
    let p = partial.rgba.as_ref().unwrap();
    assert_eq!(
        p.pixels,
        full.rgba.as_ref().unwrap().pixels,
        "translate={translate} scale={scale}"
    );
    let yellow = |img: &RgbaImage, w, h| {
        hist(img, scale, (0, 0, w, h))
            .get(&YELLOW)
            .copied()
            .unwrap_or(0)
    };
    let alone = |html: &str| yellow(&render(html, 16, 16, scale), 16, 16);
    let page = |css: &str, body: &str| {
        format!("<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#c0c0c0}}svg{{display:block}}{css}</style></head><body>{body}</body></html>")
    };
    let warn_line = ICONS.lines().find(|l| l.contains("--icon-warn:")).unwrap();
    let data_warn = alone(&page(
        &format!(":root{{{warn_line}}}#d{{width:16px;height:16px;background:var(--icon-warn)}}"),
        "<div id=\"d\"></div>",
    ));
    let inline_warn = alone(&page("", WARN_SVG));
    assert!(data_warn > 0 && inline_warn > 0);
    assert_eq!(
        yellow(p, 160, 100),
        data_warn + inline_warn,
        "data: warn + inline warn painted in the same swap"
    );
    // Damage stays inside the section (plus translate and the snap pad),
    // never the top strip; in particular the inline <svg>'s layout-less
    // children must not add a rect at the origin.
    for r in &partial.damage {
        assert!(
            r.y >= 36.0 * scale as f32,
            "damage {r:?} reaches the top strip"
        );
    }
}

#[test]
fn swap_partial_equals_full_scale_1() {
    partial_equals_full(false, 1.0);
}

#[test]
fn swap_partial_equals_full_scale_2() {
    partial_equals_full(false, 2.0);
}

#[test]
fn swap_partial_equals_full_translated_scale_1() {
    partial_equals_full(true, 1.0);
}

#[test]
fn swap_partial_equals_full_translated_scale_2() {
    partial_equals_full(true, 2.0);
}

#[test]
fn rejected_svg_fragment_changes_nothing() {
    for (frag, el) in [
        ("<svg xmlns='http://www.w3.org/2000/svg'><text>A</text></svg>".to_string(), "text"),
        (
            "<span style=\"background:url(&quot;data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cimage href='a.png'/%3E%3C/svg%3E&quot;)\"></span>".to_string(),
            "image",
        ),
    ] {
        let (mut e, _) = swapped(false, 1.0);
        let before = e.serialize_html().unwrap();
        let frame_before = e.last_frame().unwrap().pixels.clone();
        let slices = Slices(HashMap::from([("hud".to_string(), 3)]));
        let err = e.sync(&slices, &mut Frags(frag.clone())).unwrap_err();
        assert!(
            matches!(&err, EngineError::InvalidSvg { rejection: SvgRejection::ForbiddenElement { element }, .. } if element == el),
            "{frag}: {err:?}"
        );
        assert_eq!(e.serialize_html().unwrap(), before, "DOM must be unchanged");
        let out = e.paint(vp(160, 100, 1.0)).unwrap();
        let now = out.rgba.as_ref().map(|r| r.pixels.clone()).unwrap_or(frame_before.clone());
        assert_eq!(now, frame_before, "pixels must be unchanged");
    }
}

#[test]
fn warm_up_runs_once_and_reports_its_cost() {
    let _e = engine(); // starts the background warm-up
    let first = svg_warm_up();
    assert!(svg_warm_up_done());
    assert_eq!(svg_warm_up(), first, "the warm-up runs once per process");
}
