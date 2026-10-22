//! A release after a slice swap that moved a form control, and before any
//! paint, must be hit-tested against the new layout (#88's root cause:
//! `on_pointer_up` hit-tested form controls before resolving, so a release
//! where the box *was* still ticked it). Found while investigating Mainnet
//! #811 (not its cause).
use hx_native::*;
use std::collections::HashMap;

const STYLE: &str = "<style>html,body{margin:0;background:#fff;font:14px \"DejaVu Sans\"}\
#meet{width:200px;line-height:20px}#st{margin:0;width:16px;height:16px}.row{height:24px}</style>";
const SHORT: &str = "meeting soon";
const LONG: &str = "Game says the next meeting is in about 2 weeks, after the board";

fn meet(t: &str) -> String {
    format!("<span id=\"line\">{t}</span>")
}
fn page(t: &str) -> String {
    format!("<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
<div id=\"meet\" hx-trigger=\"slice:meet\">{}</div>\
<div class=\"row\"><input type=\"checkbox\" id=\"st\" name=\"state\" value=\"tx\" hx-post=\"deal.st\" hx-trigger=\"change\" /></div>\
<div class=\"row\"><button type=\"button\" id=\"add\" hx-post=\"deal.add\" hx-trigger=\"click\">+ Add</button></div>\
</form></body></html>", meet(t))
}
#[derive(Default)]
struct Sl(HashMap<String, u64>);
impl SliceSource for Sl {
    fn version(&self, n: &str) -> Option<u64> {
        self.0.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
}
fn sl(v: u64) -> Sl {
    let mut s = Sl::default();
    s.0.insert("actions".into(), 1);
    s.0.insert("meet".into(), v);
    s
}
struct Fr(&'static str);
impl FragmentSource for Fr {
    fn render(&mut self, _: &str, _: &[&str]) -> String {
        meet(self.0)
    }
}
fn vp() -> Viewport {
    Viewport::with_scale(240, 160, 1.0)
}

fn boot(t: &'static str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        pending_timeout_frames: 5,
        ..Default::default()
    });
    e.declare_actions(&["deal.st", "deal.add"]);
    e.load_html(&page(t)).unwrap();
    e.sync(&sl(1), &mut Fr(t)).unwrap();
    e.paint(vp()).unwrap();
    e
}
fn centre(e: &Engine, id: &str) -> (f32, f32) {
    let (x, y, w, h) = e.test_element_box(id).unwrap();
    (x + w / 2.0, y + h / 2.0)
}
fn press(e: &mut Engine, (x, y): (f32, f32)) {
    e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
}
fn release(e: &mut Engine, (x, y): (f32, f32)) -> Vec<String> {
    let o = e.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let mut acts = vec![];
    for o in std::iter::once(o).chain(e.take_outcomes()) {
        if let InputOutcome::Action { request, name, .. } = o {
            acts.push(name);
            e.complete(request, Ok(()));
        }
    }
    acts
}

/// Press the box where it is painted; the line above grows (slice swap);
/// release on the same spot, which now shows the meeting text, not the box.
fn old_spot(paint_between: bool) -> (Vec<String>, Option<String>) {
    let mut e = boot(SHORT);
    let at = centre(&e, "st");
    press(&mut e, at);
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    if paint_between {
        e.paint(vp()).unwrap();
    }
    let moved = centre(&boot(LONG), "st");
    assert!(
        moved.1 - at.1 > 16.0,
        "fixture: the box moved off the pressed spot ({at:?} -> {moved:?})"
    );
    (release(&mut e, at), e.attr("st", "checked"))
}

/// Press the box; the line above grows; release on the box where it is now
/// painted (press and release on the same control): it ticks.
fn new_spot(paint_between: bool) -> (Vec<String>, Option<String>) {
    let mut e = boot(SHORT);
    let at = centre(&e, "st");
    press(&mut e, at);
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    if paint_between {
        e.paint(vp()).unwrap();
    }
    let to = centre(&boot(LONG), "st");
    (release(&mut e, to), e.attr("st", "checked"))
}

#[test]
fn release_off_the_moved_box_without_a_paint_does_not_tick_it() {
    let (acts, checked) = old_spot(false);
    assert!(
        acts.is_empty() && checked.is_none(),
        "release over the meeting line ticked the box: {acts:?}, checked={checked:?}"
    );
}
#[test]
fn release_off_the_moved_box_with_a_paint_does_not_tick_it() {
    let (acts, checked) = old_spot(true);
    assert!(
        acts.is_empty() && checked.is_none(),
        "{acts:?}, checked={checked:?}"
    );
}
#[test]
fn release_on_the_moved_box_without_a_paint_ticks_it() {
    let (acts, checked) = new_spot(false);
    assert_eq!(acts, vec!["deal.st".to_string()], "checked={checked:?}");
}
#[test]
fn release_on_the_moved_box_with_a_paint_ticks_it() {
    let (acts, checked) = new_spot(true);
    assert_eq!(acts, vec!["deal.st".to_string()], "checked={checked:?}");
}
