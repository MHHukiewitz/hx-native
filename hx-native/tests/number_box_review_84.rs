//! Graph review of #84 (issue #81): a held arrow stops on PointerLeave and
//! on any focus loss; swaps, idle frames, scale 2 parity, Tab-select.
use hx_native::*;
use std::collections::HashMap;
use std::time::Duration;

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

fn boot_at(value: &str, scale: f64) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&format!("<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\"><section id=\"sec\" hx-trigger=\"slice:panel\">{}</section>\
        <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" /><div id=\"tick\" hx-trigger=\"slice:ticker\">t0</div></form></body></html>", boxes(value))).unwrap();
    // First sync records the slice versions (it renders every section
    // once), so later syncs swap only what they bump.
    e.sync(&sl(1, 1), &mut Fr(boxes(value))).unwrap();
    let _ = e
        .paint(Viewport::with_scale(
            (200.0 * scale) as u32,
            (80.0 * scale) as u32,
            scale,
        ))
        .unwrap();
    e
}
fn boot(v: &str) -> Engine {
    boot_at(v, 1.0)
}

fn feed(e: &mut Engine, ev: InputEvent, ch: &mut Vec<String>) {
    let d = e.input(ev);
    for o in std::iter::once(d).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            trigger: TriggerInfo::Change,
            args,
            ..
        } = o
        {
            ch.push(
                args.iter()
                    .find(|(k, _)| k == "n")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default(),
            );
            e.complete(request, Ok(()));
        }
    }
}
fn pdown(x: f32, y: f32) -> InputEvent {
    InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    }
}
fn pup(x: f32, y: f32) -> InputEvent {
    InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    }
}
fn key(k: &str, shift: bool, pressed: bool, repeat: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            shift,
            ..Modifiers::default()
        },
        repeat,
        pressed,
    })
}
fn val(e: &Engine) -> String {
    e.attr("n", "value").unwrap_or_default()
}
const UP: (f32, f32) = (140.0, 6.0);

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
fn sl(panel: u64, ticker: u64) -> Sl {
    let mut s = Sl::default();
    s.v.insert("actions".into(), 1);
    s.v.insert("panel".into(), panel);
    s.v.insert("ticker".into(), ticker);
    s
}

/// The host loses the pointer (PointerLeave, e.g. Mainnet drops a release
/// outside the panel): the repeat must stop.
#[test]
fn q1_pointer_leave_stops_the_repeat() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    assert_eq!(val(&e), "11");
    feed(&mut e, InputEvent::PointerLeave, &mut ch);
    e.test_hold_spin_for(Duration::from_millis(1000));
    let after_leave = val(&e);
    // A release that never reaches the engine; a later click elsewhere.
    feed(&mut e, pdown(10.0, 46.0), &mut ch);
    feed(&mut e, pup(10.0, 46.0), &mut ch);
    e.test_hold_spin_for(Duration::from_millis(3000));
    assert_eq!(after_leave, "11", "no repeat after PointerLeave");
    assert_eq!(val(&e), "11", "still no repeat after a later click");
    assert_eq!(ch, ["11"], "the leave sends the press's one change");
}

/// Tab away while the mouse is held: the box blurs; it must not keep stepping.
#[test]
fn q2_tab_away_mid_hold() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    feed(&mut e, key("Tab", false, true, false), &mut ch);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("other"));
    e.test_hold_spin_for(Duration::from_millis(1000));
    let v = val(&e);
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    assert_eq!(v, "11", "a blurred box does not keep stepping");
}

/// A swap replacing the box mid-hold clears the repeat; release sends nothing.
#[test]
fn q3_swap_replaces_box_mid_hold() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    e.test_hold_spin_for(Duration::from_millis(600));
    assert_eq!(val(&e), "14");
    e.sync(&sl(2, 1), &mut Fr(boxes("50"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
    let after_swap = val(&e);
    e.test_hold_spin_for(Duration::from_millis(1500));
    let after_more = val(&e);
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    assert_eq!(after_swap, "50");
    assert_eq!(after_more, "50", "repeat cleared by the replacement");
    assert!(ch.is_empty(), "{ch:?}");
}

/// An unrelated slice swap mid-hold keeps the hold and the committed value.
#[test]
fn q4_unrelated_swap_mid_hold_keeps_hold() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    e.sync(&sl(1, 2), &mut Fr(String::new())).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
    e.test_hold_spin_for(Duration::from_millis(600));
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    assert_eq!(ch, ["14"]);
    // #69: value_at_change kept across unrelated syncs: Esc goes back to 14.
    e.sync(&sl(1, 3), &mut Fr(String::new())).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 80, 1.0));
    feed(&mut e, InputEvent::Text("7".into()), &mut ch);
    feed(&mut e, key("Escape", false, true, false), &mut ch);
    assert_eq!(val(&e), "14");
    feed(&mut e, key("Tab", false, true, false), &mut ch);
    assert_eq!(ch, ["14"], "no second change");
}

/// Idle: no repaint while nothing changes, during a hold before 400 ms,
/// and after the release (caret blink aside).
#[test]
fn q5_idle_frames_are_skipped() {
    let mut e = boot("10");
    let mut ch = vec![];
    let vp = Viewport::with_scale(200, 80, 1.0);
    for _ in 0..3 {
        let o = e.paint(vp.clone()).unwrap();
        assert!(o.damage.is_empty(), "idle before");
    }
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    let _ = e.paint(vp.clone()).unwrap();
    e.test_hold_spin_for(Duration::from_millis(100));
    let o = e.paint(vp.clone()).unwrap();
    assert!(
        o.damage.is_empty(),
        "no repaint before the first repeat: {:?}",
        o.damage
    );
    e.test_hold_spin_for(Duration::from_millis(450));
    let o = e.paint(vp.clone()).unwrap();
    assert!(!o.damage.is_empty());
    e.test_hold_spin_for(Duration::from_millis(450));
    let o = e.paint(vp.clone()).unwrap();
    assert!(o.damage.is_empty(), "same due count: no repaint");
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    let _ = e.paint(vp.clone()).unwrap();
    for _ in 0..3 {
        let o = e.paint(vp.clone()).unwrap();
        assert!(o.damage.is_empty(), "idle after");
    }
}

fn partial_eq_full(e: &mut Engine, vp: &Viewport, what: &str) {
    let out = e.paint(vp.clone()).unwrap();
    if e.last_paint_stats().full_repaint {
        eprintln!(
            "{what}: full repaint (area fallback {})",
            e.last_paint_stats().full_repaint_area_fallback
        );
    }
    let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
    e.mark_theme_or_stylesheet_changed();
    let full = e.paint(vp.clone()).unwrap().rgba.expect("full");
    let d = partial
        .pixels
        .chunks_exact(4)
        .zip(full.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    assert_eq!(d, 0, "{what}: partial must equal full");
}

/// Focus, caret and value damage at scale 1 and 2 from an unfocused box,
/// a Tab selection, and a typed edit.
#[test]
fn q6_partial_equals_full_scale_1_and_2() {
    for scale in [1.0, 2.0] {
        for start in ["unfocused", "tab", "typed"] {
            let mut e = boot_at("10", scale);
            let vp = Viewport::with_scale((200.0 * scale) as u32, (80.0 * scale) as u32, scale);
            let mut ch = vec![];
            match start {
                "tab" => {
                    e.test_set_focus("other");
                    feed(&mut e, key("Tab", true, true, false), &mut ch);
                    assert!(e.test_selected_text("n").is_some());
                }
                "typed" => {
                    e.test_set_focus("n");
                    feed(&mut e, InputEvent::Text("5".into()), &mut ch);
                }
                _ => {}
            }
            let _ = e.paint(vp.clone()).unwrap();
            let tag = format!("{start} @{scale}");
            feed(&mut e, pdown(UP.0, UP.1), &mut ch);
            partial_eq_full(&mut e, &vp, &format!("{tag} press"));
            e.test_hold_spin_for(Duration::from_millis(700));
            partial_eq_full(&mut e, &vp, &format!("{tag} repeat"));
            feed(&mut e, pup(UP.0, UP.1), &mut ch);
            partial_eq_full(&mut e, &vp, &format!("{tag} release"));
            e.test_force_caret_blink();
            partial_eq_full(&mut e, &vp, &format!("{tag} blink"));
            feed(&mut e, key("Tab", false, true, false), &mut ch);
            partial_eq_full(&mut e, &vp, &format!("{tag} blur"));
            assert_eq!(ch.len(), 1, "{tag}: one change {ch:?}");
        }
    }
}

/// #69 Tab-select still works after the arrow work, and a Tab back in after
/// an arrow press selects everything again.
#[test]
fn q7_tab_select_after_arrow() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    feed(&mut e, key("Tab", false, true, false), &mut ch);
    feed(&mut e, key("Tab", true, true, false), &mut ch);
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
    assert_eq!(e.test_selected_text("n").as_deref(), Some("11"));
    feed(&mut e, InputEvent::Text("4".into()), &mut ch);
    feed(&mut e, key("Enter", false, true, false), &mut ch);
    assert_eq!(ch, ["11", "4"]);
}

/// Secondary button on an arrow: no step, no hold.
#[test]
fn q8_right_press_does_not_step() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(
        &mut e,
        InputEvent::PointerDown {
            x: UP.0,
            y: UP.1,
            button: PointerButton::Secondary,
            mods: Modifiers::default(),
        },
        &mut ch,
    );
    e.test_hold_spin_for(Duration::from_millis(1000));
    feed(
        &mut e,
        InputEvent::PointerUp {
            x: UP.0,
            y: UP.1,
            button: PointerButton::Secondary,
            mods: Modifiers::default(),
        },
        &mut ch,
    );
    assert_eq!(val(&e), "10");
    assert!(ch.is_empty());
}

/// Key repeat, then Esc before the release: back to the first press's value,
/// and the release then sends nothing.
#[test]
fn q9_key_repeat_then_esc() {
    let mut e = boot("10");
    let mut ch = vec![];
    e.test_set_focus("n");
    feed(&mut e, key("ArrowUp", false, true, false), &mut ch);
    for _ in 0..3 {
        feed(&mut e, key("ArrowUp", false, true, true), &mut ch);
    }
    assert_eq!(val(&e), "14");
    feed(&mut e, key("Escape", false, true, false), &mut ch);
    feed(&mut e, key("ArrowUp", false, false, false), &mut ch);
    assert_eq!(val(&e), "11");
    assert_eq!(ch, ["11"], "{ch:?}");
}

/// Host-side focus move (test_set_focus) mid-hold.
#[test]
fn q10_host_focus_move_mid_hold() {
    let mut e = boot("10");
    let mut ch = vec![];
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    e.test_set_focus("other");
    e.test_hold_spin_for(Duration::from_millis(1000));
    assert_eq!(val(&e), "11");
}

/// Held at max: once pinned, the repeat stops dirtying the box (no repaint
/// every 100 ms for an unchanged value), and the release still sends the
/// value the hold reached.
#[test]
fn q13_pinned_hold_is_idle() {
    let mut e = boot("98");
    let mut ch = vec![];
    let vp = Viewport::with_scale(200, 80, 1.0);
    feed(&mut e, pdown(UP.0, UP.1), &mut ch);
    e.test_hold_spin_for(Duration::from_millis(600));
    assert_eq!(val(&e), "100");
    let _ = e.paint(vp.clone()).unwrap();
    for ms in [700, 800, 900, 1500] {
        e.test_hold_spin_for(Duration::from_millis(ms));
        let o = e.paint(vp.clone()).unwrap();
        assert!(
            o.damage.is_empty(),
            "pinned at max, {ms} ms: {:?}",
            o.damage
        );
    }
    feed(&mut e, pup(UP.0, UP.1), &mut ch);
    assert_eq!(ch, ["100"]);
}
