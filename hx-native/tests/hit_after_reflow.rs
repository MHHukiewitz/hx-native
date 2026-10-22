//! Mainnet #811 (UI, hx-native e89c782): a long meeting line wraps, the Deals
//! form paints 25 px lower, and a click on the painted ▼ reaches
//! "+ Add to Line access" instead. Hit-testing must use the layout of the
//! DOM as it stands (what the next/last paint shows), never a layout from
//! before a slice swap.
use hx_native::*;
use std::collections::HashMap;

const STYLE: &str = "<style>html,body{margin:0;background:#fff;font:14px \"DejaVu Sans\"}\
#meet{width:200px;line-height:20px}\
.spin{display:inline-block;position:relative;width:80px;height:24px;vertical-align:top}\
#n{position:absolute;left:0;top:0;width:56px;height:24px;font-size:14px}\
.ud{position:absolute;left:58px;top:0;width:20px;height:24px}\
.ud i{display:block;height:12px;font-style:normal;font-size:9px;line-height:12px}\
.kidadd{height:24px}#add{height:22px;font-size:12px}</style>";

const SHORT: &str = "meeting soon";
const LONG: &str = "Game says the next meeting is in about 2 weeks, after the board";

fn meet(text: &str) -> String {
    format!("<span id=\"line\">{text}</span>")
}

fn html(text: &str) -> String {
    format!("<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
<div id=\"meet\" hx-trigger=\"slice:meet\">{}</div>\
<div class=\"ed\"><span class=\"spin\"><input id=\"n\" type=\"number\" name=\"n\" value=\"10\" min=\"0\" max=\"100\" step=\"1\" hx-post=\"deal.n\" hx-trigger=\"change\" />\
<span class=\"ud\"><i id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</i><i id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</i></span></span></div>\
<div class=\"kidadd\"><button type=\"button\" id=\"add\" hx-post=\"deal.add\" hx-trigger=\"click\">+ Add to Line access ▼</button></div>\
</form></body></html>", meet(text))
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
    fn render(&mut self, _id: &str, _s: &[&str]) -> String {
        meet(self.0)
    }
}

const VP: (u32, u32) = (240, 160);
fn vp() -> Viewport {
    Viewport::with_scale(VP.0, VP.1, 1.0)
}

fn boot(text: &'static str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        pending_timeout_frames: 5,
        ..Default::default()
    });
    e.declare_actions(&["deal.n", "deal.add"]);
    e.load_html(&html(text)).unwrap();
    e.sync(&sl(1), &mut Fr(text)).unwrap();
    e.paint(vp()).unwrap();
    e
}

fn centre(e: &Engine, id: &str) -> (f32, f32) {
    let (x, y, w, h) = e.test_element_box(id).unwrap();
    (x + w / 2.0, y + h / 2.0)
}

/// Where ▼ is painted once the long line is in: a fresh engine that loaded
/// the long line and painted it (the full-repaint truth).
fn painted_dn_with_long_line() -> (f32, f32) {
    centre(&boot(LONG), "dn")
}

/// Press id on down, the actions fired on down+up, and the box value after.
fn click(e: &mut Engine, (x, y): (f32, f32)) -> (Option<String>, Vec<String>, String) {
    let m = Modifiers::default();
    let mut actions = vec![];
    let mut press = None;
    for ev in [
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: m,
        },
    ] {
        let is_down = matches!(ev, InputEvent::PointerDown { .. });
        let o = e.input(ev);
        for o in std::iter::once(o).chain(e.take_outcomes()) {
            match o {
                InputOutcome::Press { id } if is_down => press = id,
                InputOutcome::Action { request, name, .. } => {
                    actions.push(name);
                    e.complete(request, Ok(()));
                }
                _ => {}
            }
        }
    }
    (press, actions, e.attr("n", "value").unwrap_or_default())
}

#[test]
fn the_long_line_really_moves_the_form_down() {
    let short = centre(&boot(SHORT), "dn");
    let long = painted_dn_with_long_line();
    assert!(
        long.1 - short.1 >= 15.0,
        "fixture: wrap must push ▼ down, short {short:?} long {long:?}"
    );
    // And the old ▼ spot is then on "+ Add to Line access"'s row or above it.
}

fn assert_hits_down_arrow(label: &str, r: (Option<String>, Vec<String>, String)) {
    let (press, actions, value) = r;
    assert!(
        !actions.iter().any(|a| a == "deal.add"),
        "{label}: click on painted ▼ fired deal.add ({actions:?})"
    );
    let _ = press; // a spin press answers FocusChanged (the box keeps focus), not Press
    assert_eq!(value, "9", "{label}: ▼ steps 10 -> 9");
}

#[test]
fn click_after_a_reflowing_swap_without_a_paint_hits_the_moved_arrow() {
    let target = painted_dn_with_long_line();
    let mut e = boot(SHORT);
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    let r = click(&mut e, target);
    assert_hits_down_arrow("no paint between", r);
}

#[test]
fn click_after_a_reflowing_swap_and_a_paint_hits_the_moved_arrow() {
    let target = painted_dn_with_long_line();
    let mut e = boot(SHORT);
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    e.paint(vp()).unwrap();
    assert_eq!(
        centre(&e, "dn"),
        target,
        "after the paint, the live layout is the painted one"
    );
    let r = click(&mut e, target);
    assert_hits_down_arrow("paint between", r);
}

/// Mainnet's frame order (src/ui/hx_deals/mod.rs `step`): input, sync,
/// resize_surface, paint_surface, every frame. The click arrives the frame
/// after the swap was painted.
#[test]
fn mainnet_frame_order_click_on_the_next_frame_hits_the_moved_arrow() {
    let target = painted_dn_with_long_line();
    let mut e = boot(SHORT);
    // frame k: no input, sync swaps the long line in, paint.
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    e.paint(vp()).unwrap();
    // frame k+1: the click on the painted ▼, then sync (nothing new), paint.
    let r = click(&mut e, target);
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    e.paint(vp()).unwrap();
    assert_hits_down_arrow("mainnet frame order", r);
}

// ---- Press, then the slice swap, then the release (the #88 window). ----
// The release must be hit-tested against the DOM after the swap, painted or
// not (see release_stale_layout.rs for the form-control case).

fn down(e: &mut Engine, (x, y): (f32, f32)) -> InputOutcome {
    e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}
fn up_actions(e: &mut Engine, (x, y): (f32, f32)) -> Vec<String> {
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

/// Press "+ Add" where it is painted; the line above grows; release at the
/// same spot, which now holds the row above "+ Add". No click may fire.
fn release_on_the_old_spot(paint_between: bool) -> Vec<String> {
    let mut e = boot(SHORT);
    let old_add = centre(&e, "add");
    assert_eq!(
        down(&mut e, old_add),
        InputOutcome::Press {
            id: Some("add".into())
        }
    );
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    if paint_between {
        e.paint(vp()).unwrap();
    }
    let new_add = painted_add_with_long_line();
    assert!(
        new_add.1 - old_add.1 > 13.0,
        "fixture: + Add moved off the pressed spot"
    );
    up_actions(&mut e, old_add)
}
fn painted_add_with_long_line() -> (f32, f32) {
    centre(&boot(LONG), "add")
}

/// Press "+ Add"; the line above grows; release on "+ Add" where it is now
/// painted. Press and release are on the same element: the click fires.
fn release_on_the_new_spot(paint_between: bool) -> Vec<String> {
    let mut e = boot(SHORT);
    let old_add = centre(&e, "add");
    assert_eq!(
        down(&mut e, old_add),
        InputOutcome::Press {
            id: Some("add".into())
        }
    );
    e.sync(&sl(2), &mut Fr(LONG)).unwrap();
    if paint_between {
        e.paint(vp()).unwrap();
    }
    up_actions(&mut e, painted_add_with_long_line())
}

#[test]
fn release_after_a_swap_without_a_paint_is_hit_tested_against_the_new_layout_old_spot() {
    let acts = release_on_the_old_spot(false);
    assert!(
        acts.is_empty(),
        "release over the row above + Add fired {acts:?} (hit-tested against the pre-swap layout)"
    );
}
#[test]
fn release_after_a_swap_and_a_paint_old_spot() {
    let acts = release_on_the_old_spot(true);
    assert!(acts.is_empty(), "fired {acts:?}");
}
#[test]
fn release_after_a_swap_without_a_paint_is_hit_tested_against_the_new_layout_new_spot() {
    let acts = release_on_the_new_spot(false);
    assert_eq!(
        acts,
        vec!["deal.add".to_string()],
        "release on + Add as painted must click it"
    );
}
#[test]
fn release_after_a_swap_and_a_paint_new_spot() {
    let acts = release_on_the_new_spot(true);
    assert_eq!(acts, vec!["deal.add".to_string()]);
}
