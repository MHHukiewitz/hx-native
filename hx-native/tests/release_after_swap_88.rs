//! Issue #88 (Graph probe q12): press ▲, then a `sync` re-renders the section
//! without the box or its arrows, then release before any paint. The release
//! used to hit-test Blitz's pre-swap layout tree and panic in blitz-dom
//! (node.rs:1483). It must be hit-tested against the DOM as it stands.
use std::collections::HashMap;

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#n{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#other{position:absolute;left:0;top:40px}</style>";

fn boxes(value: &str) -> String {
    format!("<input id=\"n\" type=\"number\" name=\"n\" value=\"{value}\" min=\"0\" max=\"100\" step=\"1\" hx-post=\"deal.n\" hx-trigger=\"change\" />\
     <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</span>\
     <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</span>")
}

#[derive(Default)]
struct Sl {
    v: HashMap<String, u64>,
}
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
fn sl(panel: u64) -> Sl {
    let mut s = Sl::default();
    s.v.insert("actions".into(), 1);
    s.v.insert("panel".into(), panel);
    s.v.insert("ticker".into(), 1);
    s
}
struct Fr(String);
impl FragmentSource for Fr {
    fn render(&mut self, id: &str, _s: &[&str]) -> String {
        if id == "sec" {
            self.0.clone()
        } else {
            format!("t-{id}")
        }
    }
}

fn boot(value: &str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\"><section id=\"sec\" hx-trigger=\"slice:panel\">{}</section>\
        <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" /><div id=\"tick\" hx-trigger=\"slice:ticker\">t0</div></form></body></html>",
        boxes(value)
    ))
    .unwrap();
    e.sync(&sl(1), &mut Fr(boxes(value))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
    e
}

fn pointer(down: bool, (x, y): (f32, f32)) -> InputEvent {
    let (button, mods) = (PointerButton::Primary, Modifiers::default());
    if down {
        InputEvent::PointerDown { x, y, button, mods }
    } else {
        InputEvent::PointerUp { x, y, button, mods }
    }
}

const UP: (f32, f32) = (140.0, 6.0);

#[test]
fn release_after_a_sync_that_removed_the_pressed_arrow_does_not_panic() {
    let mut e = boot("10");
    e.input(pointer(true, UP));
    // The swap removes the pressed arrow and its box; no paint before the release.
    e.sync(&sl(2), &mut Fr(String::new())).unwrap();
    let out = e.input(pointer(false, UP));
    let rest = e.take_outcomes();
    assert!(
        !matches!(out, InputOutcome::Clicked { .. }),
        "nothing is left under the pointer to click: {out:?}"
    );
    assert!(
        rest.iter()
            .all(|o| !matches!(o, InputOutcome::Clicked { .. })),
        "{rest:?}"
    );
    // And the surface still paints.
    e.paint(Viewport::with_scale(200, 80, 1.0)).unwrap();
}
