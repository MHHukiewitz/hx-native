//! Issue #107: at a fractional scale, a surface whose scale changed after it
//! was parsed (the usual host path: `load_html` at the default scale 1, then
//! `paint` at the real display scale) must look exactly like a surface opened
//! at that scale, and a partial repaint after a swap must equal a fresh full
//! paint. Everything here compares real painted pixels (and the layout boxes
//! behind them), never engine internals.
use hx_native::*;

const W: u32 = 320;
const H: u32 = 200;

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

fn cfg() -> EngineConfig {
    EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

/// The button `o` sits outside every swapped subtree. Buttons and fields carry 1 px borders: stylo snaps a border to whole
/// device pixels, so their size depends on the scale the style was computed at.
fn page(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;font:12px \"DejaVu Sans\"}}\
         .row{{display:flex;gap:4px}}button{{height:20px}}</style></head>\
         <body><h3>Head</h3><button id=\"o\">Out</button>{body}<p id=\"tail\">tail text</p></body></html>"
    )
}

fn panel(x: &str) -> String {
    format!(
        "<div id=\"root\" hx-trigger=\"slice:panel\" hx-swap=\"innerHTML\" class=\"row\">{x}</div>"
    )
}

const A: &str = "<button id=\"b\">A</button><input id=\"c\" value=\"v\"><span id=\"s\">one</span>";
const B: &str =
    "<button id=\"b\">AA</button><input id=\"c\" value=\"vv\"><span id=\"s\">two two</span>";

type Rect = (f32, f32, f32, f32);

fn boxes(e: &Engine) -> Vec<Option<Rect>> {
    ["o", "root", "b", "c", "s", "tail"]
        .iter()
        .map(|id| e.test_element_box(id))
        .collect()
}

fn vp(scale: f64) -> Viewport {
    Viewport::with_scale(W, H, scale)
}

/// Ground truth: the surface opened and painted at `scale` from the start.
fn opened_at(html: &str, scale: f64) -> Engine {
    let mut e = Engine::new(cfg());
    e.open_surface(
        SurfaceId::DEFAULT,
        html,
        vp(scale),
        SurfaceOpts {
            visible: true,
            pinned: true,
        },
    )
    .unwrap();
    e.paint(vp(scale)).unwrap();
    e
}

/// Number of differing RGBA pixels (a size mismatch counts every pixel).
fn diff_px(a: &Engine, b: &Engine) -> usize {
    let (fa, fb) = (a.last_frame().unwrap(), b.last_frame().unwrap());
    if fa.pixels.len() != fb.pixels.len() {
        return usize::MAX;
    }
    fa.pixels
        .chunks(4)
        .zip(fb.pixels.chunks(4))
        .filter(|(p, q)| p != q)
        .count()
}

fn assert_same(what: &str, got: &Engine, want: &Engine) {
    let d = diff_px(got, want);
    assert!(
        d == 0 && boxes(got) == boxes(want),
        "{what}: {d} pixels differ\n got  {:?}\n want {:?}",
        boxes(got),
        boxes(want)
    );
}

/// `load_html` lays out at the default scale 1; the first paint then runs at
/// the display scale. The result must equal a surface opened at that scale.
#[test]
fn first_paint_at_fractional_scale_matches_surface_opened_there() {
    for scale in [1.25, 1.5, 2.0] {
        let html = page(&panel(A));
        let mut e = Engine::new(cfg());
        e.load_html(&html).unwrap();
        e.paint(vp(scale)).unwrap();
        assert_same(&format!("scale {scale}"), &e, &opened_at(&html, scale));
    }
}

/// A scale change on a live, already painted surface (window moved to another
/// monitor) re-lays out like a surface opened at the new scale, both up and down.
#[test]
fn scale_change_after_paint_matches_surface_opened_at_new_scale() {
    for (from, to) in [
        (1.0, 1.25),
        (1.0, 1.5),
        (2.0, 1.25),
        (1.25, 1.5),
        (1.25, 1.0),
    ] {
        let html = page(&panel(A));
        let mut e = opened_at(&html, from);
        e.paint(vp(to)).unwrap();
        assert_same(&format!("{from} -> {to}"), &e, &opened_at(&html, to));
    }
}

/// The issue's case: swap after the scale change. The partial repaint of the
/// swapped subtree must equal a fresh full paint of the same markup, for
/// children-only swaps and for the whole root.
#[test]
fn partial_repaint_after_swap_equals_fresh_full_paint_at_fractional_scale() {
    for scale in [1.25, 1.5, 2.0] {
        for outer in [false, true] {
            let (first, second) = if outer {
                let root = |x: &str| {
                    format!("<div id=\"root\" hx-trigger=\"slice:panel\" hx-swap=\"outerHTML\" class=\"row\">{x}</div>")
                };
                (page(&root(A)), root(B))
            } else {
                (page(&panel(A)), B.to_string())
            };
            let mut e = Engine::new(cfg());
            e.load_html(&first).unwrap();
            e.paint(vp(scale)).unwrap();
            e.sync(&Sl(1), &mut Fr(second.clone())).unwrap();
            let out = e.paint(vp(scale)).unwrap();
            assert!(!out.skipped, "the swap must repaint");
            let want = if outer {
                opened_at(&page(&second), scale)
            } else {
                opened_at(&page(&panel(B)), scale)
            };
            assert_same(&format!("scale {scale} outer={outer}"), &e, &want);
        }
    }
}

/// Sanity: the compared frames are not blank (a vacuous equality would pass).
#[test]
fn compared_frames_hold_ink() {
    let e = opened_at(&page(&panel(A)), 1.25);
    let f = e.last_frame().unwrap();
    let first = &f.pixels[0..4];
    assert!(f.pixels.chunks(4).filter(|p| *p != first).count() > 200);
}
