//! Graph #52 round 2: prefixed names, dashes and depth must not get past the SVG gate.
use hx_native::*;

const NS: &str = "http://www.w3.org/2000/svg";
const XL: &str = "http://www.w3.org/1999/xlink";
const FILE: &str = "/tmp/hx-native-gate-probe.png";
const RED16: &[u8] = include_bytes!("fixtures/img/red16.png");

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

fn rejected(what: &str, html: &str) {
    assert!(
        matches!(load(html), Err(EngineError::InvalidSvg { .. })),
        "{what} must be rejected"
    );
}

#[test]
fn prefixed_names_are_rejected() {
    std::fs::write(FILE, RED16).unwrap();
    let t = "x='0' y='20' font-size='20'";
    rejected(
        "data: é:text",
        &data_page(&format!(
            "<svg xmlns='{NS}' xmlns:é='{NS}' width='64' height='24'><é:text {t}>H</é:text></svg>"
        )),
    );
    rejected("data: a·b:text", &data_page(&format!("<svg xmlns='{NS}' xmlns:a·b='{NS}' width='64' height='24'><a·b:text {t}>H</a·b:text></svg>")));
    rejected("data: é:image file", &data_page(&format!("<svg xmlns='{NS}' xmlns:é='{NS}' width='64' height='24'><é:image href='{FILE}' width='16' height='16'/></svg>")));
    rejected(
        "inline svg:text",
        &page(&format!(
            "<svg xmlns:svg='{NS}' width='64' height='24'><svg:text {t}>H</svg:text></svg>"
        )),
    );
    rejected("inline svg:image file", &page(&format!("<svg xmlns:svg='{NS}' width='64' height='24'><svg:image href='{FILE}' width='16' height='16'/></svg>")));
    rejected("inline q:href file", &page(&format!("<svg xmlns:q='{XL}' width='64' height='24'><image q:href='{FILE}' width='16' height='16'/></svg>")));
    rejected("inline q:href via use", &page(&format!("<svg xmlns:q='{XL}' width='64' height='24'><defs><image id='i' q:href='{FILE}' width='16' height='16'/></defs><use href='#i'/></svg>")));
    // A good data: image under a prefixed href still loads.
    let ok = format!("data:image/png;base64,{}", b64(RED16));
    assert!(load(&page(&format!("<svg xmlns:q='{XL}' width='64' height='24'><image q:href='{ok}' width='16' height='16'/></svg>"))).is_ok());
}

#[test]
fn dashes_and_depth_are_rejected() {
    let dash = format!("<svg xmlns='{NS}' width='64' height='24'><path d='M0 0L1e7 1e7' stroke='red' stroke-dasharray='0.01'/></svg>");
    rejected("data: dasharray", &data_page(&dash));
    rejected("inline dasharray", &page(&dash));
    rejected("inline dash in style", &page("<svg width='64' height='24'><path d='M0 0L9 9' style='stroke:red;stroke-dasharray:0.01'/></svg>"));
    rejected("inline dash in <style>", &page("<svg width='64' height='24'><style>path{stroke-dasharray:0.01}</style><path d='M0 0L9 9'/></svg>"));
    let nest = |n: usize| {
        format!(
            "<svg xmlns='{NS}' width='64' height='24'>{}<rect width='9' height='9'/>{}</svg>",
            "<g>".repeat(n),
            "</g>".repeat(n)
        )
    };
    rejected("data: 300 nested <g>", &data_page(&nest(300)));
    rejected("inline 300 nested <g>", &page(&nest(300)));
    assert!(load(&data_page(&nest(32))).is_ok());
    assert!(load(&page(&nest(32))).is_ok());
}

// Htmx (#52 round 4): the routes Graph's HOLD 6327790962 asked to be checked
// against 1f02907, beyond the cases above.

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn pct_page(svg: &str) -> String {
    page(&format!(
        r#"<div id="d" style="width:64px;height:24px;background-image:url(&quot;data:image/svg+xml,{}&quot;)"></div>"#,
        pct(svg)
    ))
}

#[test]
fn xlink_bound_hrefs_relative_paths_and_percent_encoded_prefixes_are_rejected() {
    std::fs::write(FILE, RED16).unwrap();
    let img = |href: &str| format!("<image q:href='{href}' width='16' height='16'/>");
    for href in [
        FILE,
        "red.png",
        "./red.png",
        "../tmp/red.png",
        "file:///tmp/r.png",
    ] {
        let inline = format!(
            "<svg xmlns:q='{XL}' width='64' height='24'>{}</svg>",
            img(href)
        );
        rejected(&format!("inline q:href={href}"), &page(&inline));
        let used = format!(
            "<svg xmlns:q='{XL}' width='64' height='24'><defs>{}</defs><use href='#i'/></svg>",
            img(href).replace("<image", "<image id='i'")
        );
        rejected(&format!("inline q:href={href} via use"), &page(&used));
        let data = format!(
            "<svg xmlns='{NS}' xmlns:q='{XL}' width='64' height='24'>{}</svg>",
            img(href)
        );
        rejected(&format!("data: q:href={href}"), &data_page(&data));
        rejected(&format!("pct data: q:href={href}"), &pct_page(&data));
        // Any element, not only <image>.
        let a = format!("<svg xmlns:q='{XL}' width='64' height='24'><a q:href='{href}'><rect width='9' height='9'/></a></svg>");
        rejected(&format!("inline <a q:href={href}>"), &page(&a));
    }
    rejected(
        "inline svg:image relative",
        &page(&format!("<svg xmlns:svg='{NS}' width='64' height='24'><svg:image href='red.png' width='16' height='16'/></svg>")),
    );
    let t = "x='0' y='20' font-size='20'";
    for (decl, el) in [
        (format!("xmlns:é='{NS}'"), "é:text"),
        (format!("xmlns:svg='{NS}'"), "svg:text"),
        (String::new(), "text"),
        (String::new(), "TEXT"),
    ] {
        let svg =
            format!("<svg xmlns='{NS}' {decl} width='64' height='24'><{el} {t}>H</{el}></svg>");
        rejected(&format!("pct data: <{el}>"), &pct_page(&svg));
    }
}
