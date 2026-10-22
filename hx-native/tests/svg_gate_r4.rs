//! Graph #52 round 4: `<svg:style>`, CSS strings holding `/*`, nested
//! markers and inflating PNG chunks must not get past the SVG gate.
use hx_native::*;

const NS: &str = "http://www.w3.org/2000/svg";
const RED16: &[u8] = include_bytes!("fixtures/img/red16.png");
const PATH: &str = "<path d='M0 0L64 24' stroke='red' fill='none'/>";

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18 & 63) as usize] as char);
        s.push(T[(n >> 12 & 63) as usize] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}

fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><body>{body}</body></html>")
}

fn data_page(svg: &str) -> String {
    page(&format!(
        r#"<div id="d" style="width:64px;height:24px;background-image:url(&quot;data:image/svg+xml;base64,{}&quot;)"></div>"#,
        b64(svg.as_bytes())
    ))
}

fn load(html: &str) -> Result<(), EngineError> {
    let mut e = Engine::new(EngineConfig::default());
    e.declare_actions(&["act.do"]);
    e.load_html(html)
}

fn rejected(what: &str, html: &str) -> SvgRejection {
    match load(html) {
        Err(EngineError::InvalidSvg { rejection, .. }) => rejection,
        other => panic!("{what} must be rejected, got {other:?}"),
    }
}

fn accepted(what: &str, html: &str) {
    assert!(load(html).is_ok(), "{what} must load: {:?}", load(html));
}

#[test]
fn a_prefixed_style_element_is_a_sheet() {
    let sheet = "path{stroke-dasharray:0.01}";
    for (decl, el) in [
        (format!("xmlns:svg='{NS}'"), "svg:style"),
        (format!("xmlns:x='{NS}'"), "x:style"),
        ("xmlns:x='urn:other'".to_string(), "x:style"),
        (String::new(), "STYLE"),
    ] {
        let svg = format!("<svg {decl} width='64' height='24'><{el}>{sheet}</{el}>{PATH}</svg>");
        let r = rejected(&format!("inline <{el}>"), &page(&svg));
        assert!(matches!(r, SvgRejection::ForbiddenProperty { .. }), "{r:?}");
    }
}

#[test]
fn a_comment_opener_inside_a_css_string_does_not_hide_a_dash() {
    let sheet = r#"path{font-family:"/*";stroke-dasharray:0.01}a{font-family:"*/"}"#;
    let attr = r#"style='font-family:"/*";stroke-dasharray:0.01;font-family:"*/"'"#;
    let attr_path = format!("<path d='M0 0L64 24' stroke='red' fill='none' {attr}/>");
    let single = sheet.replace('"', "'");
    for (what, inner) in [
        ("sheet", format!("<style>{sheet}</style>{PATH}")),
        (
            "sheet, single quotes",
            format!("<style>{single}</style>{PATH}"),
        ),
        ("style attribute", attr_path.clone()),
    ] {
        let inline = format!("<svg width='64' height='24'>{inner}</svg>");
        rejected(&format!("inline {what}"), &page(&inline));
        let data = format!("<svg xmlns='{NS}' width='64' height='24'>{inner}</svg>");
        rejected(&format!("data: {what}"), &data_page(&data));
    }
    // A string holding `/*` without any dash still loads.
    let ok = r#"<style>path{font-family:"/*"}a{font-family:"*/"}</style>"#;
    accepted(
        "inline sheet without dashes",
        &page(&format!("<svg width='64' height='24'>{ok}{PATH}</svg>")),
    );
    accepted(
        "data: sheet without dashes",
        &data_page(&format!(
            "<svg xmlns='{NS}' width='64' height='24'>{ok}{PATH}</svg>"
        )),
    );
}

#[test]
fn markers_are_refused() {
    let m = "<marker id='m' markerWidth='4' markerHeight='4'><rect width='1' height='1' fill='red'/></marker>";
    let p = "<path d='M0 0L8 0L16 0' fill='none' stroke='none' marker-mid='url(#m)'/>";
    let inline = format!("<svg width='64' height='24'><defs>{m}</defs>{p}</svg>");
    let r = rejected("inline marker", &page(&inline));
    assert!(matches!(r, SvgRejection::ForbiddenElement { .. }), "{r:?}");
    let prefixed = format!(
        "<svg xmlns:svg='{NS}' width='64' height='24'><svg:defs>{}</svg:defs>{p}</svg>",
        m.replace("<marker", "<svg:marker")
            .replace("</marker>", "</svg:marker>")
    );
    rejected("inline svg:marker", &page(&prefixed));
    let data = format!("<svg xmlns='{NS}' width='64' height='24'><defs>{m}</defs>{p}</svg>");
    rejected("data: marker", &data_page(&data));
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// RED16 with one extra chunk (valid CRC) right after IHDR.
fn with_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = RED16[..33].to_vec();
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(payload);
    v.extend_from_slice(&body);
    v.extend_from_slice(&crc32(&body).to_be_bytes());
    v.extend_from_slice(&RED16[33..]);
    v
}

#[test]
fn png_chunks_inflated_on_every_paint_are_refused() {
    let img = |bytes: &[u8]| {
        format!(
            "<svg xmlns='{NS}' width='16' height='16'><image href='data:image/png;base64,{}' width='16' height='16'/></svg>",
            b64(bytes)
        )
    };
    accepted("plain red16", &page(&img(RED16)));
    accepted("red16 + tEXt", &page(&img(&with_chunk(b"tEXt", b"k\0v"))));
    let png = with_chunk(b"iCCP", b"x\0\0\x78\x9c\x03\0\0\0\0\x01");
    let r = rejected("inline iCCP", &page(&img(&png)));
    assert!(matches!(r, SvgRejection::CorruptImage { .. }), "{r:?}");
    rejected("data: iCCP", &data_page(&img(&png)));
}

#[test]
fn masks_patterns_and_nested_clip_paths_are_refused() {
    let r = "<rect width='4' height='4' fill='red'/>";
    for (what, svg) in [
        ("mask", format!("<svg width='64' height='24'><mask id='m'>{r}</mask><rect width='8' height='8' fill='red' mask='url(#m)'/></svg>")),
        ("pattern", format!("<svg width='64' height='24'><pattern id='p' width='4' height='4'>{r}</pattern><rect width='8' height='8' fill='url(#p)'/></svg>")),
        ("clip-path inside clipPath", format!("<svg width='64' height='24'><clipPath id='a'>{r}</clipPath><clipPath id='b'><rect width='4' height='4' clip-path='url(#a)'/></clipPath><rect width='8' height='8' fill='red' clip-path='url(#b)'/></svg>")),
        ("clip-path on clipPath", format!("<svg width='64' height='24'><clipPath id='a'>{r}</clipPath><clipPath id='b' clip-path='url(#a)'>{r}</clipPath><rect width='8' height='8' fill='red' clip-path='url(#b)'/></svg>")),
        ("clip-path style inside clipPath", format!("<svg width='64' height='24'><clipPath id='a'>{r}</clipPath><clipPath id='b'><rect width='4' height='4' style='clip-path:url(#a)'/></clipPath><rect width='8' height='8' fill='red' clip-path='url(#b)'/></svg>")),
        ("clip-path in a sheet", format!("<svg width='64' height='24'><style>rect{{clip-path:url(#a)}}</style><clipPath id='a'>{r}</clipPath><rect width='8' height='8' fill='red'/></svg>")),
    ] {
        rejected(&format!("inline {what}"), &page(&svg));
        let data = svg.replacen("<svg ", &format!("<svg xmlns='{NS}' "), 1);
        rejected(&format!("data: {what}"), &data_page(&data));
    }
    // A plain single-path clip still loads.
    let ok = format!("<svg width='64' height='24'><clipPath id='a'>{r}</clipPath><rect width='8' height='8' fill='red' clip-path='url(#a)'/></svg>");
    accepted("inline clip-path", &page(&ok));
    let data = ok.replacen("<svg ", &format!("<svg xmlns='{NS}' "), 1);
    accepted("data: clip-path", &data_page(&data));
}
