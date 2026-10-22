//! A swapped section whose root is a `<form>` must lay out like the same
//! markup loaded statically (UI's report on Mainnet `ui/805-services-html`
//! @ 8e56ae9e: Designer's Services detail tab row `#svc-tabs`, an
//! `hx-swap="outerHTML"` form with a hidden `service` input and the tab
//! buttons, laid out 0 px tall after the swap, with 0x0 buttons).
use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;font:12px \"DejaVu Sans\"}\
.btnrow{display:flex;gap:4px;padding:2px}.btnrow button{height:20px;padding:0 6px}</style>";

fn tabs(on: &str) -> String {
    let mut s = String::from("<form class=\"btnrow tabsr\" id=\"svc-tabs\" role=\"tablist\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"service\" value=\"fiber\">");
    for t in ["overview", "access", "capacity", "economics"] {
        let cls = if t == on { " class=\"on\"" } else { "" };
        s += &format!("<button type=\"button\" id=\"svc-tab-{t}\"{cls} hx-post=\"service.tab\" hx-trigger=\"click\" hx-sync=\"drop\" name=\"tab\" value=\"{t}\" role=\"tab\">{t}</button>");
    }
    s + "</form>"
}

/// The tab row in a panel, below a heading, like the Services detail column.
fn page(row: &str) -> String {
    format!("<!DOCTYPE html><html><head>{STYLE}</head><body><div id=\"detail\"><h3>Fiber</h3>{row}<div id=\"svc-body\">body</div></div></body></html>")
}

struct Sl(u64);
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        match n {
            "actions" => Some(1),
            "service.detail" => Some(self.0),
            _ => None,
        }
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
struct Fr(fn(&str) -> String, &'static str);
impl FragmentSource for Fr {
    fn render(&mut self, _id: &str, _s: &[&str]) -> String {
        (self.0)(self.1)
    }
}

fn vp() -> Viewport {
    Viewport::with_scale(400, 120, 1.0)
}

fn engine(html: &str) -> Engine {
    let mut e = Engine::new(EngineConfig::default());
    e.declare_actions(&["service.tab"]);
    e.load_html(html).unwrap();
    e
}

/// (form box, [button boxes]) after a paint.
/// `(x, y, width, height)` of a laid-out box.
type Rect = (f32, f32, f32, f32);

fn boxes(e: &Engine) -> (Rect, Vec<Rect>) {
    let form = e.test_element_box("svc-tabs").expect("form");
    let buttons = ["overview", "access", "capacity", "economics"]
        .iter()
        .map(|t| e.test_element_box(&format!("svc-tab-{t}")).expect("button"))
        .collect();
    (form, buttons)
}

fn assert_laid_out(label: &str, e: &Engine) {
    let (form, buttons) = boxes(e);
    assert!(
        form.3 > 0.0,
        "{label}: form #svc-tabs is {form:?}, expected nonzero height"
    );
    for (i, b) in buttons.iter().enumerate() {
        assert!(
            b.2 > 0.0 && b.3 > 0.0,
            "{label}: button {i} is {b:?}, expected a nonzero hit box"
        );
    }
}

fn click_tab(e: &mut Engine, tab: &str) -> Vec<(String, String)> {
    let (x, y, w, h) = e.test_element_box(&format!("svc-tab-{tab}")).unwrap();
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    let m = Modifiers::default();
    e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: m,
    });
    let o = e.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: m,
    });
    let mut args = vec![];
    for o in std::iter::once(o).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            name,
            args: a,
            ..
        } = o
        {
            assert_eq!(name, "service.tab");
            args = a;
            e.complete(request, Ok(()));
        }
    }
    args
}

#[test]
fn control_static_load_lays_out_the_form_and_its_buttons() {
    let mut e = engine(&page(&tabs("overview")));
    e.paint(vp()).unwrap();
    assert_laid_out("static", &e);
}

#[test]
fn outer_html_swap_of_a_form_root_lays_out_the_form_and_its_buttons() {
    let mut e = engine(&page(&tabs("overview")));
    e.sync(&Sl(1), &mut Fr(tabs, "overview")).unwrap();
    e.paint(vp()).unwrap();
    assert_laid_out("after first sync", &e);
    e.sync(&Sl(2), &mut Fr(tabs, "access")).unwrap();
    e.paint(vp()).unwrap();
    assert_laid_out("after a second swap", &e);
}

#[test]
fn swapped_form_matches_the_static_layout() {
    let mut s = engine(&page(&tabs("access")));
    s.paint(vp()).unwrap();
    let mut e = engine(&page(&tabs("overview")));
    e.sync(&Sl(1), &mut Fr(tabs, "overview")).unwrap();
    e.sync(&Sl(2), &mut Fr(tabs, "access")).unwrap();
    e.paint(vp()).unwrap();
    assert_eq!(boxes(&e), boxes(&s), "swapped layout == static layout");
}

/// UI tried a wrapping `<div>`; the form inside it still collapsed.
fn wrapped(on: &str) -> String {
    format!("<div id=\"svc-tabs-wrap\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\">{}</div>", tabs(on).replace(" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\"", ""))
}

#[test]
fn outer_html_swap_of_a_div_wrapping_the_form_lays_it_out() {
    let mut e = engine(&page(&wrapped("overview")));
    e.sync(&Sl(1), &mut Fr(wrapped, "overview")).unwrap();
    e.sync(&Sl(2), &mut Fr(wrapped, "access")).unwrap();
    e.paint(vp()).unwrap();
    assert_laid_out("wrapped", &e);
}

#[test]
fn a_click_on_a_tab_after_the_swap_posts_that_tab() {
    let mut e = engine(&page(&tabs("overview")));
    e.sync(&Sl(1), &mut Fr(tabs, "overview")).unwrap();
    e.paint(vp()).unwrap();
    let args = click_tab(&mut e, "capacity");
    assert!(
        args.contains(&("tab".into(), "capacity".into())),
        "{args:?}"
    );
    assert!(
        args.contains(&("service".into(), "fiber".into())),
        "{args:?}"
    );
}

/// The cause is not specific to `<form>`: any outerHTML fragment whose root
/// is not an HTML "breakout" tag (`div`, `p`, ...) was parsed as foreign
/// content, because the fragment's context element had no namespace. Each
/// root here must lay out after a swap exactly as it does when loaded.
const ROOTS: &[(&str, &str)] = &[
    ("button", "<button type=\"button\" id=\"root\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\" style=\"height:20px\">Tab</button>"),
    ("span with an input", "<span id=\"root\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"s\" value=\"1\"><button type=\"button\" id=\"b\">Go</button></span>"),
    ("label with a checkbox", "<label id=\"root\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\"><input type=\"checkbox\" id=\"b\" name=\"c\" value=\"1\">Tick</label>"),
    ("section", "<section id=\"root\" hx-trigger=\"slice:service.detail\" hx-swap=\"outerHTML\"><input type=\"hidden\" name=\"s\" value=\"1\"><button type=\"button\" id=\"b\">Go</button></section>"),
];

fn root_boxes(e: &Engine) -> Vec<Option<Rect>> {
    vec![e.test_element_box("root"), e.test_element_box("b")]
}

#[test]
fn any_outer_html_root_lays_out_like_its_static_load() {
    let mut bad = vec![];
    for (label, html) in ROOTS {
        let mut s = engine(&page(html));
        s.paint(vp()).unwrap();
        let mut e = engine(&page(html));
        struct One(&'static str);
        impl FragmentSource for One {
            fn render(&mut self, _id: &str, _s: &[&str]) -> String {
                self.0.to_string()
            }
        }
        e.sync(&Sl(1), &mut One(html)).unwrap();
        e.paint(vp()).unwrap();
        if root_boxes(&e) != root_boxes(&s) {
            bad.push(format!(
                "{label}: swapped {:?} static {:?}",
                root_boxes(&e),
                root_boxes(&s)
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Partial repaint == full repaint across the swap that fixes the layout.
#[test]
fn swapped_form_partial_frame_equals_a_full_repaint() {
    let mut e = engine(&page(&tabs("overview")));
    e.sync(&Sl(1), &mut Fr(tabs, "overview")).unwrap();
    e.paint(vp()).unwrap();
    e.sync(&Sl(2), &mut Fr(tabs, "access")).unwrap();
    e.paint(vp()).unwrap();
    let mut full = engine(&page(&tabs("access")));
    full.paint(vp()).unwrap();
    assert!(
        e.last_frame().unwrap().pixels == full.last_frame().unwrap().pixels,
        "partial frame after the swap != full repaint of the same markup"
    );
}
