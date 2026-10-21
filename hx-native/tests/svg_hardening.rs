//! Issue #23, round 3 (Graph HOLD on #52, 2026-10-21): the holes Graph's
//! probes found at 1d74e68 and the image-round checks.
//!
//! - Element names are checked by local name in every namespace and under
//!   any prefix (`<é:text>`, `<svg:text>`, `<svg:image href=/abs>`).
//! - Every non-`data:` href on any element is refused.
//! - `stroke-dasharray` / `stroke-dashoffset` are refused (147 bytes aborted
//!   the process at 1d74e68).
//! - Nesting is capped at 64, checked without recursion before any parse.
//! - `<use>` is forbidden.

use hx_native::*;

const RED_PNG: &[u8] = include_bytes!("fixtures/img/red16.png");
const RED: [u8; 3] = [255, 0, 0];
const NS: &str = "http://www.w3.org/2000/svg";

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

fn uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

fn engine() -> Engine {
    Engine::new(EngineConfig {
        max_paint_ns: 200_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    })
}

fn vp(css_w: u32, css_h: u32, scale: f64) -> Viewport {
    Viewport::with_scale(
        (css_w as f64 * scale).round() as u32,
        (css_h as f64 * scale).round() as u32,
        scale,
    )
}

fn render(html: &str, scale: f64) -> RgbaImage {
    let mut e = engine();
    e.load_html(html).expect("load_html");
    e.paint(vp(32, 32, scale)).unwrap().rgba.unwrap()
}

/// Every device pixel of the 16x16 CSS box, 1 CSS px in from each edge
/// (image smoothing may blend the outermost ring), within `tol` of `want`.
fn assert_fill(img: &RgbaImage, scale: f64, want: [u8; 3], tol: u8, what: &str) {
    let s = scale as u32;
    let mut bad = 0;
    for y in s..15 * s {
        for x in s..15 * s {
            let i = ((y * img.width + x) * 4) as usize;
            let px = &img.pixels[i..i + 3];
            if px.iter().zip(want).any(|(a, b)| a.abs_diff(b) > tol) {
                bad += 1;
            }
        }
    }
    assert_eq!(
        bad, 0,
        "{what} at scale {scale}: {bad} pixels differ from {want:?}"
    );
}

fn reject(html: &str) -> SvgRejection {
    match engine().load_html(html) {
        Err(EngineError::InvalidSvg { rejection, .. }) => rejection,
        other => panic!("expected InvalidSvg, got {other:?}"),
    }
}

/// Loads, and paints nothing but the white page.
fn assert_blank(html: &str, what: &str) {
    let img = render(html, 1.0);
    let ink = img
        .pixels
        .chunks(4)
        .filter(|p| p[..3] != [255, 255, 255])
        .count();
    assert_eq!(ink, 0, "{what}: {ink} pixels drawn");
}

fn data_svg_raw(svg: &str) -> String {
    let svg = svg
        .replace('%', "%25")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('#', "%23")
        .replace('"', "%22");
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}#d{{width:16px;height:16px;background:url("data:image/svg+xml,{svg}")}}</style></head><body><div id="d"></div></body></html>"#
    )
}

fn data_svg_b64(svg: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}</style></head><body><div id="d" style="width:16px;height:16px;background-image:url(&quot;data:image/svg+xml;base64,{}&quot;)"></div></body></html>"#,
        base64(svg.as_bytes())
    )
}

fn inline_raw(svg: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}svg{{display:block}}</style></head><body>{svg}</body></html>"#
    )
}

const TEXT: &str = "x='0' y='14' font-size='14' font-family='DejaVu Sans,sans-serif'";

#[test]
fn prefixed_and_foreign_forbidden_elements_are_refused() {
    let text = || SvgRejection::ForbiddenElement {
        element: "text".into(),
    };
    for prefix in ["é", "a·b", "svg", "x"] {
        let svg = format!("<svg xmlns='{NS}' xmlns:{prefix}='{NS}' width='16' height='16'><{prefix}:text {TEXT}>HH</{prefix}:text></svg>");
        assert_eq!(reject(&data_svg_raw(&svg)), text(), "data: {prefix}:text");
        assert_eq!(reject(&data_svg_b64(&svg)), text(), "b64 {prefix}:text");
        if prefix == "é" {
            // HTML only starts a tag at an ASCII letter: inline, `<é:text>`
            // is character data, so nothing is drawn.
            assert_blank(&inline_raw(&svg), "inline é:text");
        } else {
            assert_eq!(reject(&inline_raw(&svg)), text(), "inline {prefix}:text");
        }
    }
    // Inline only: HTML parsing keeps these names, Blitz serialises them
    // by local name for usvg.
    for svg in [
        format!("<svg xmlns:svg='{NS}' width='16' height='16'><svg:text {TEXT}>HH</svg:text></svg>"),
        format!("<svg width='16' height='16'><TEXT {TEXT}>HH</TEXT></svg>"),
        format!("<svg width='16' height='16'><foreignObject width='16' height='16'><text {TEXT}>HH</text></foreignObject></svg>"),
    ] {
        assert_eq!(reject(&inline_raw(&svg)), text(), "{svg}");
    }
    let tspan = format!(
        "<svg xmlns='{NS}' xmlns:é='{NS}' width='16' height='16'><é:tspan>H</é:tspan></svg>"
    );
    assert_eq!(
        reject(&data_svg_b64(&tspan)),
        SvgRejection::ForbiddenElement {
            element: "tspan".into()
        }
    );
}

#[test]
fn prefixed_image_with_a_file_href_is_refused() {
    let png = "/tmp/hx-native-svg-hardening.png";
    std::fs::write(png, RED_PNG).unwrap();
    for prefix in ["é", "svg", "x"] {
        let svg = format!("<svg xmlns='{NS}' xmlns:{prefix}='{NS}' width='16' height='16'><{prefix}:image href='{png}' width='16' height='16'/></svg>");
        let want = SvgRejection::ImageHref { href: png.into() };
        assert_eq!(reject(&data_svg_b64(&svg)), want, "data: {prefix}:image");
        if prefix == "é" {
            assert_blank(&inline_raw(&svg), "inline é:image");
        } else {
            assert_eq!(reject(&inline_raw(&svg)), want, "inline {prefix}:image");
        }
    }
    let svg = format!("<svg xmlns:svg='{NS}' width='16' height='16'><svg:image href='{png}' width='16' height='16'/></svg>");
    assert_eq!(
        reject(&inline_raw(&svg)),
        SvgRejection::ImageHref { href: png.into() }
    );
    // A data: raster under a prefix is still allowed and paints.
    let ok = uri("image/png", RED_PNG);
    let svg = format!("<svg xmlns='{NS}' xmlns:é='{NS}' width='16' height='16'><é:image href='{ok}' width='16' height='16'/></svg>");
    assert_fill(
        &render(&data_svg_b64(&svg), 1.0),
        1.0,
        RED,
        0,
        "data: é:image",
    );
}

#[test]
fn non_data_hrefs_are_refused_on_every_element() {
    for (el, attr, href) in [
        ("linearGradient", "xlink:href", "#g0"),
        ("linearGradient", "href", "/tmp/x.svg"),
        ("pattern", "href", "#p0"),
        ("a", "href", "https://example.com/"),
        ("a", "xlink:href", "file:///etc/hostname"),
        ("g", "href", "x.png"),
    ] {
        let svg = format!("<svg xmlns='{NS}' xmlns:xlink='http://www.w3.org/1999/xlink' width='16' height='16'><{el} id='e' {attr}='{href}'/></svg>");
        let want = SvgRejection::ExternalHref {
            element: el.into(),
            href: href.into(),
        };
        assert_eq!(reject(&data_svg_b64(&svg)), want, "data: <{el} {attr}>");
        let got = reject(&inline_raw(&svg));
        assert!(
            matches!(&got, SvgRejection::ExternalHref { href: h, .. } if h == href),
            "inline <{el} {attr}>: {got:?}"
        );
    }
    // A data: href on another element is not fetched and stays allowed.
    let svg = format!("<svg xmlns='{NS}' width='16' height='16'><a href='data:,x'><rect width='16' height='16' fill='red'/></a></svg>");
    assert_fill(
        &render(&data_svg_b64(&svg), 1.0),
        1.0,
        RED,
        0,
        "data: <a href=data:>",
    );
    assert_fill(
        &render(&inline_raw(&svg), 1.0),
        1.0,
        RED,
        0,
        "inline <a href=data:>",
    );
}

#[test]
fn stroke_dashes_are_refused_as_attributes_styles_and_sheets() {
    for (svg, property) in [
        // Graph's 147-byte bomb: 7.5 GB at paint at 1d74e68.
        (format!("<svg xmlns='{NS}' width='64' height='24'><path d='M0 0L1e7 1e7' stroke='red' stroke-width='1' stroke-dasharray='0.01'/></svg>"), "stroke-dasharray"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><path d='M0 0L16 16' stroke='red' stroke-dashoffset='1'/></svg>"), "stroke-dashoffset"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><path d='M0 0L16 16' stroke='red' STROKE-DASHARRAY='1'/></svg>"), "stroke-dasharray"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><path d='M0 0L16 16' style='stroke:red;stroke-dasharray:0.01'/></svg>"), "stroke-dasharray"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><path d='M0 0L16 16' style='stroke:red;STROKE-dash\\61rray:0.01'/></svg>"), "stroke-dasharray"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><style>path{{stroke-/**/dasharray:0.01}}</style><path d='M0 0L16 16' stroke='red'/></svg>"), "stroke-dasharray"),
        (format!("<svg xmlns='{NS}' width='16' height='16'><style><![CDATA[path{{stroke-dashoffset:3}}]]></style><path d='M0 0L16 16' stroke='red'/></svg>"), "stroke-dashoffset"),
    ] {
        let want = SvgRejection::ForbiddenProperty {
            property: property.into(),
        };
        assert_eq!(reject(&data_svg_b64(&svg)), want, "data: {svg}");
        assert_eq!(reject(&inline_raw(&svg)), want, "inline {svg}");
    }
    // A plain stroke still paints.
    let svg = format!("<svg xmlns='{NS}' width='16' height='16'><path d='M0 8H16' stroke='red' stroke-width='16'/></svg>");
    assert_fill(
        &render(&data_svg_b64(&svg), 1.0),
        1.0,
        RED,
        0,
        "data: stroke",
    );
    assert_fill(
        &render(&inline_raw(&svg), 1.0),
        1.0,
        RED,
        0,
        "inline stroke",
    );
}

fn nested(depth: usize) -> String {
    // The root <svg> is depth 1; the rect is depth `depth`.
    let gs = depth.saturating_sub(2);
    format!(
        "<svg xmlns='{NS}' width='16' height='16'>{}<rect width='16' height='16' fill='red'/>{}</svg>",
        "<g>".repeat(gs),
        "</g>".repeat(gs)
    )
}

#[test]
fn nesting_is_capped_at_64_without_overflowing_the_stack() {
    // Runs on a default 2 MiB test thread: at 1d74e68 about 200 <g> in a
    // data: SVG overflowed it in debug.
    let ok = nested(SVG_MAX_DEPTH);
    assert_fill(
        &render(&data_svg_b64(&ok), 1.0),
        1.0,
        RED,
        0,
        "data: depth 64",
    );
    assert_fill(
        &render(&inline_raw(&ok), 1.0),
        1.0,
        RED,
        0,
        "inline depth 64",
    );
    for depth in [SVG_MAX_DEPTH + 1, 200, 1100, 5000] {
        let svg = nested(depth);
        let want = SvgRejection::TooDeep {
            depth: SVG_MAX_DEPTH + 1,
            limit: SVG_MAX_DEPTH,
        };
        assert_eq!(reject(&data_svg_b64(&svg)), want, "data: depth {depth}");
        assert_eq!(reject(&inline_raw(&svg)), want, "inline depth {depth}");
    }
    // Comments, CDATA and self-closing tags do not count; `>` in a quoted
    // attribute does not end the tag.
    let tricky = format!(
        "<svg xmlns='{NS}' width='16' height='16'><!-- {} --><![CDATA[{}]]>{}<g data-x='a>b'>{}<rect width='16' height='16' fill='red'/>{}</g></svg>",
        "<g>".repeat(100),
        "<g>".repeat(100),
        "<g/>".repeat(100),
        "<g>".repeat(SVG_MAX_DEPTH - 3),
        "</g>".repeat(SVG_MAX_DEPTH - 3)
    );
    assert_fill(
        &render(&data_svg_b64(&tricky), 1.0),
        1.0,
        RED,
        0,
        "data: tricky depth",
    );
    // Elements an entity expands to are counted on the parsed tree.
    let entity = format!(
        "<!DOCTYPE svg [<!ENTITY deep \"{}<rect width='1' height='1'/>{}\">]><svg xmlns='{NS}' width='16' height='16'>&deep;</svg>",
        "<g>".repeat(70),
        "</g>".repeat(70)
    );
    assert_eq!(
        reject(&data_svg_b64(&entity)),
        SvgRejection::TooDeep {
            depth: SVG_MAX_DEPTH + 1,
            limit: SVG_MAX_DEPTH
        }
    );
}

#[test]
fn nested_svg_in_an_image_is_refused_whatever_type_it_claims() {
    for inner in [
        format!("<svg xmlns='{NS}' width='16' height='16'><text {TEXT}>HH</text></svg>"),
        format!("<svg xmlns='{NS}' width='16' height='16'><image href='/tmp/hx-native-svg-hardening.png' width='16' height='16'/></svg>"),
    ] {
        for mime in ["image/svg+xml", "image/png", "image/jpeg", "image/gif", "image/webp", "text/plain", ""] {
            let href = format!("data:{mime};base64,{}", base64(inner.as_bytes()));
            let svg = format!("<svg xmlns='{NS}' width='16' height='16'><image href='{href}' width='16' height='16'/></svg>");
            for (what, html) in [("data", data_svg_b64(&svg)), ("inline", inline_raw(&svg))] {
                let got = reject(&html);
                let ok = if SVG_IMAGE_MIME_TYPES.contains(&mime) {
                    got == SvgRejection::ImageTypeMismatch {
                        declared: mime.into(),
                        actual: "image/svg+xml".into(),
                    }
                } else {
                    matches!(got, SvgRejection::ImageHref { .. })
                };
                assert!(ok, "{what} {mime:?}: {got:?}");
            }
        }
    }
}

#[test]
fn use_is_forbidden_inline_and_in_data() {
    let svg = format!("<svg xmlns='{NS}' width='16' height='16'><defs><rect id='r' width='16' height='16' fill='red'/></defs><use href='#r'/></svg>");
    let want = SvgRejection::ForbiddenElement {
        element: "use".into(),
    };
    assert_eq!(reject(&data_svg_b64(&svg)), want);
    assert_eq!(reject(&inline_raw(&svg)), want);
    let svg = format!("<svg xmlns='{NS}' xmlns:é='{NS}' width='16' height='16'><é:USE/></svg>");
    assert_eq!(reject(&data_svg_b64(&svg)), want);
}
