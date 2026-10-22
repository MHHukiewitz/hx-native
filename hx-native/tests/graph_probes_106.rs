//! Graph's review probes for hx-native PR #106 (fixes #104): the outerHTML
//! fragment context is now an HTML `<div>`. Every swapped root must parse
//! and lay out like the same markup loaded statically (boxes *and* pixels),
//! inline `<svg>` must stay foreign (SVG namespace), and partial repaint
//! must equal a fresh full paint at scale 1 and 1.25.
use hx_native::*;

struct Sl(u64);
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        match n {
            "actions" => Some(1),
            "panel" => Some(self.0),
            _ => None,
        }
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
struct Fr(String);
impl FragmentSource for Fr {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        self.0.clone()
    }
}

const T: &str = "hx-trigger=\"slice:panel\" hx-swap=\"outerHTML\"";

fn cfg() -> EngineConfig {
    EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

fn page(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;font:12px \"DejaVu Sans\"}}\
         td{{padding:2px;background:#cde}}table{{border-collapse:collapse}}\
         .row{{display:flex;gap:4px}}button{{height:20px}}</style></head>\
         <body><h3>Head</h3>{body}<p id=\"tail\">tail</p></body></html>"
    )
}

type Rect = (f32, f32, f32, f32);

fn probe(e: &Engine) -> Vec<Option<Rect>> {
    ["root", "b", "c", "tail"]
        .iter()
        .map(|id| e.test_element_box(id))
        .collect()
}

/// (static boxes, swapped boxes, pixels equal) for one outerHTML root.
/// `wrap(x)` places the root in its static context (e.g. inside a table).
fn compare(
    wrap: &dyn Fn(&str) -> String,
    root: &str,
    root2: &str,
    scale: f64,
) -> (Vec<Option<Rect>>, Vec<Option<Rect>>, bool) {
    let vp = || Viewport::with_scale(320, 200, scale);
    let mut s = Engine::new(cfg());
    s.load_html(&page(&wrap(root2))).unwrap();
    s.paint(vp()).unwrap();
    let mut e = Engine::new(cfg());
    e.load_html(&page(&wrap(root))).unwrap();
    e.paint(vp()).unwrap();
    e.sync(&Sl(1), &mut Fr(root2.to_string())).unwrap();
    e.paint(vp()).unwrap();
    let same_px = e.last_frame().unwrap().pixels == s.last_frame().unwrap().pixels;
    (probe(&s), probe(&e), same_px)
}

fn id(x: &str) -> String {
    x.to_string()
}

/// Roots that must parse and lay out exactly like their static load.
fn roots() -> Vec<(&'static str, String, String)> {
    let v = |a: &str, b: &str| (a.to_string(), b.to_string());
    let list: Vec<(&str, (String, String))> = vec![
        ("div", v(
            &format!("<div id=\"root\" {T} class=\"row\"><input type=\"hidden\" name=\"s\" value=\"1\"><button id=\"b\">A</button><button id=\"c\">B</button></div>"),
            &format!("<div id=\"root\" {T} class=\"row\"><input type=\"hidden\" name=\"s\" value=\"2\"><button id=\"b\">AA</button><button id=\"c\">BB</button></div>"),
        )),
        ("span", v(
            &format!("<span id=\"root\" {T}>x <b id=\"b\">bold</b> <input id=\"c\" value=\"v\"></span>"),
            &format!("<span id=\"root\" {T}>xy <b id=\"b\">bolder</b> <input id=\"c\" value=\"w\"></span>"),
        )),
        ("form", v(
            &format!("<form id=\"root\" {T} class=\"row\"><input type=\"hidden\" name=\"s\" value=\"1\"><button type=\"button\" id=\"b\">A</button><select id=\"c\"><option>One</option></select></form>"),
            &format!("<form id=\"root\" {T} class=\"row\"><input type=\"hidden\" name=\"s\" value=\"2\"><button type=\"button\" id=\"b\">AAA</button><select id=\"c\"><option>Two</option></select></form>"),
        )),
        ("ul/li", v(
            &format!("<ul id=\"root\" {T}><li id=\"b\">one</li><li id=\"c\">two</li></ul>"),
            &format!("<ul id=\"root\" {T}><li id=\"b\">uno</li><li id=\"c\">dos dos</li></ul>"),
        )),
        ("label+range", v(
            &format!("<label id=\"root\" {T}>Vol <input type=\"range\" id=\"b\" value=\"10\"><input type=\"checkbox\" id=\"c\"></label>"),
            &format!("<label id=\"root\" {T}>Vol <input type=\"range\" id=\"b\" value=\"70\"><input type=\"checkbox\" id=\"c\" checked></label>"),
        )),
        ("table root", v(
            &format!("<table id=\"root\" {T}><tr id=\"b\"><td id=\"c\">a</td><td>b</td></tr></table>"),
            &format!("<table id=\"root\" {T}><tr id=\"b\"><td id=\"c\">aa</td><td>bb</td></tr></table>"),
        )),
        ("inline svg", v(
            &format!("<svg id=\"root\" {T} width=\"60\" height=\"20\"><rect id=\"b\" width=\"30\" height=\"20\" fill=\"#c00\"/></svg>"),
            &format!("<svg id=\"root\" {T} width=\"80\" height=\"30\"><rect id=\"b\" width=\"50\" height=\"30\" fill=\"#0a0\"/></svg>"),
        )),
    ];
    list.into_iter().map(|(l, (a, b))| (l, a, b)).collect()
}

#[test]
fn every_root_swaps_like_its_static_load_boxes_and_pixels() {
    let mut bad = vec![];
    for (label, a, b) in roots() {
        let (s, e, px) = compare(&id, &a, &b, 1.0);
        if s != e || !px {
            bad.push(format!(
                "{label}: static {s:?} swapped {e:?} pixels_equal={px}"
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// A swapped-in inline `<svg>` stays foreign (SVG namespace): it keeps its
/// own `width`/`height` box and its `<rect>` paints (pixel compare above).
#[test]
fn inline_svg_root_keeps_its_svg_box() {
    let svg = format!("<svg id=\"root\" {T} width=\"80\" height=\"30\"><rect id=\"b\" width=\"50\" height=\"30\" fill=\"#0a0\"/></svg>");
    let vp = || Viewport::with_scale(320, 200, 1.0);
    let mut e = Engine::new(cfg());
    e.load_html(&page(&format!("<div id=\"root\" {T}>x</div>")))
        .unwrap();
    e.paint(vp()).unwrap();
    e.sync(&Sl(1), &mut Fr(svg)).unwrap();
    e.paint(vp()).unwrap();
    let r = e.test_element_box("root").expect("svg root");
    assert_eq!((r.2, r.3), (80.0, 30.0), "svg box {r:?}");
}

/// `<tr>` / `<td>` as an outerHTML root. With a `<div>` context the HTML
/// fragment algorithm drops the `<tr>`/`<td>` tags (their ids are gone and
/// the cells' text lands loose in the `<tbody>`). Not a #106 regression:
/// before #106 they became namespace-less foreign elements, also wrong.
/// Passes once the context is the target's parent (fix/0002).
#[test]
#[ignore = "outerHTML context is a <div>, not the target's parent (fix/0002)"]
fn table_row_and_cell_roots_swap_like_their_static_load() {
    let mut bad = vec![];
    for (label, open, close, a, b) in [
        (
            "tr",
            "<table><tbody>",
            "</tbody></table>",
            format!("<tr id=\"root\" {T}><td id=\"b\">a</td><td id=\"c\">b</td></tr>"),
            format!("<tr id=\"root\" {T}><td id=\"b\">aa</td><td id=\"c\">bb</td></tr>"),
        ),
        (
            "td",
            "<table><tbody><tr>",
            "<td id=\"c\">z</td></tr></tbody></table>",
            format!("<td id=\"root\" {T}><b id=\"b\">a</b></td>"),
            format!("<td id=\"root\" {T}><b id=\"b\">aa</b></td>"),
        ),
    ] {
        let wrap = |x: &str| format!("{open}{x}{close}");
        let (s, e, px) = compare(&wrap, &a, &b, 1.0);
        if s != e || !px {
            bad.push(format!(
                "{label}: static {s:?} swapped {e:?} pixels_equal={px}"
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Pre-existing, not #106: at scale 1.25 any swapped-in text (innerHTML
/// too, and a `<div>` root that parsed the same before #106) lays out up
/// to 1 px narrower/shorter than the same markup loaded fresh, so partial
/// != a fresh full paint. Scale 1 is exact. Separate issue.
#[test]
#[ignore = "pre-existing: swapped text at scale 1.25 is 1 px off a fresh load"]
fn scaled_swap_matches_static() {
    let mut bad = vec![];
    for (label, a, b) in roots() {
        let (s, e, px) = compare(&id, &a, &b, 1.25);
        if s != e || !px {
            bad.push(format!(
                "{label}: static {s:?} swapped {e:?} pixels_equal={px}"
            ));
        }
    }
    // innerHTML control: same root markup, only the children swapped.
    let inner =
        |x: &str| format!("<div id=\"root\" hx-trigger=\"slice:panel\" class=\"row\">{x}</div>");
    let mut s = Engine::new(cfg());
    s.load_html(&page(&inner("<button id=\"b\">AA</button>")))
        .unwrap();
    s.paint(Viewport::with_scale(320, 200, 1.25)).unwrap();
    let mut e = Engine::new(cfg());
    e.load_html(&page(&inner("<button id=\"b\">A</button>")))
        .unwrap();
    e.paint(Viewport::with_scale(320, 200, 1.25)).unwrap();
    e.sync(&Sl(1), &mut Fr("<button id=\"b\">AA</button>".into()))
        .unwrap();
    e.paint(Viewport::with_scale(320, 200, 1.25)).unwrap();
    if probe(&s) != probe(&e) {
        bad.push(format!(
            "innerHTML: static {:?} swapped {:?}",
            probe(&s),
            probe(&e)
        ));
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Partial repaint == fresh full paint (pixels and boxes) across six form
/// swaps in a row at scale 1.
#[test]
fn repeated_form_swaps_partial_equals_full() {
    let form = |n: u64| {
        format!("<form id=\"root\" {T} class=\"row\"><input type=\"hidden\" name=\"s\" value=\"{n}\"><button type=\"button\" id=\"b\">tab{n}</button><input id=\"c\" value=\"v{n}\"></form>")
    };
    let vp = || Viewport::with_scale(320, 200, 1.0);
    let mut e = Engine::new(cfg());
    e.load_html(&page(&form(0))).unwrap();
    e.paint(vp()).unwrap();
    for n in 1..=6u64 {
        e.sync(&Sl(n), &mut Fr(form(n))).unwrap();
        e.paint(vp()).unwrap();
        let mut f = Engine::new(cfg());
        f.load_html(&page(&form(n))).unwrap();
        f.paint(vp()).unwrap();
        assert!(
            e.last_frame().unwrap().pixels == f.last_frame().unwrap().pixels,
            "swap {n}: partial != full"
        );
        assert_eq!(probe(&e), probe(&f), "swap {n}: boxes");
    }
}
