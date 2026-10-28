//! Issue #100: the engine-owned scroll-gutter stylesheet (issue #26) was
//! registered through blitz-dom's per-node `add_stylesheet_for_node` on a
//! detached `<style>` node. blitz-dom orders node sheets by `NodeId` and
//! inserts before the next higher id regardless of origin. Once a `sync`
//! swap frees node slots, the gutter node can reuse a slot below a later
//! author `<style>`, and stylo panics looking for that author sheet in the
//! user-agent set (`stylesheet_set.rs:269`, "`before_sheet` stylesheet not
//! found"). The paint that first needs a gutter must not panic, on the
//! paint path and on the pointer-release path (#88/#99 resolve before the
//! release hit test), and the gutter must still be reserved.
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

const BOX: &str = "<div id=\"box\" style=\"overflow:auto;height:40px;width:100px\"><div style=\"height:200px\">tall</div></div>";

fn vp() -> Viewport {
    Viewport::with_scale(200, 100, 1.0)
}

/// The issue's repro (case B): a `<style>` after a section the first sync
/// swaps, then a container that needs a gutter on the first paint.
#[test]
fn style_after_a_swapped_section_then_gutter_paint_does_not_panic() {
    let mut e = Engine::new(EngineConfig::default());
    e.load_html(&format!(
        "<!DOCTYPE html><html><head><style>p{{margin:0}}</style></head><body>\
<section id=\"s\" hx-trigger=\"slice:panel\"><p>old</p><p>old</p></section>\
<style>b{{color:red}}</style>{BOX}</body></html>"
    ))
    .unwrap();
    e.sync(&Sl(1), &mut Fr("<p>new</p>".into())).unwrap();
    e.paint(vp()).unwrap();
}

/// Release-path variant: the page paints with no gutter, a swap frees nodes
/// and brings in a container that overflows, then a pointer release lands
/// before any paint. The release resolves the DOM (#99), which reserves the
/// gutter; the following paint must agree.
#[test]
fn release_after_a_swap_that_adds_a_gutter_does_not_panic() {
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.load_html(
        "<!DOCTYPE html><html><head><style>html,body{margin:0}p{margin:0}</style></head><body>\
<section id=\"s\" hx-trigger=\"slice:panel\"><p>old</p><p>old</p><p>old</p></section>\
<style>b{color:red}</style><p id=\"tail\">tail</p></body></html>",
    )
    .unwrap();
    e.paint(vp()).unwrap();
    e.sync(&Sl(1), &mut Fr(BOX.into())).unwrap();
    let (button, mods) = (PointerButton::Primary, Modifiers::default());
    e.input(InputEvent::PointerDown {
        x: 20.0,
        y: 10.0,
        button,
        mods,
    });
    e.input(InputEvent::PointerUp {
        x: 20.0,
        y: 10.0,
        button,
        mods,
    });
    let _ = e.take_outcomes();
    e.paint(vp()).unwrap();
    let partial = e.last_frame().unwrap().pixels.clone();
    let mut full = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    full.load_html(&format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0}}p{{margin:0}}</style></head><body>\
<section id=\"s\" hx-trigger=\"slice:panel\">{BOX}</section>\
<style>b{{color:red}}</style><p id=\"tail\">tail</p></body></html>"
    ))
    .unwrap();
    full.paint(vp()).unwrap();
    let fresh = full.last_frame().unwrap().pixels.clone();
    assert!(
        partial == fresh,
        "partial repaint differs from a fresh full paint"
    );
}
