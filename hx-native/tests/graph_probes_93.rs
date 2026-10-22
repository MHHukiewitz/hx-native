//! Graph review of #93 (issue #92, Designer, HTML_CONTRACT §2 Ruling 5,
//! Mainnet master cea8a42a): every arrow path on a number box that is empty
//! or does not parse steps from the last value sent, and the box, the
//! `change` and the next form post agree.

use hx_native::*;
use std::collections::HashMap;
use std::time::Duration;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#fee{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}\
#note{position:absolute;left:0;top:30px;width:120px;height:20px}\
#go{position:absolute;left:0;top:56px;width:60px;height:20px}\
#pick{position:absolute;left:0;top:80px;width:60px;height:20px}\
#other{position:absolute;left:100px;top:56px;width:60px;height:20px}</style>";

const UP: (f32, f32) = (140.0, 6.0);
const DOWN: (f32, f32) = (140.0, 18.0);

/// The fee box with `extra` attributes (min/max/step/group).
fn fee_html_with(value: &str, extra: &str) -> String {
    format!(
        "<input class=\"n\" id=\"fee\" type=\"number\" name=\"fee\" value=\"{value}\" {extra} hx-post=\"deal.fee\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">▼</span>"
    )
}

const FEE: &str = "min=\"0\" max=\"100\" step=\"0.01\"";

fn fee_html(value: &str) -> String {
    fee_html_with(value, FEE)
}

fn boot_full(value: &str, extra: &str, key_releases: bool) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 50,
        paint_mode: PaintMode::Exact,
        key_releases,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.send", "deal.note", "deal.pick"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <div id=\"sec\" hx-trigger=\"slice:fee\" hx-target=\"#sec\" hx-swap=\"innerHTML\">{}</div>\
         <input id=\"note\" type=\"text\" name=\"note\" value=\"hi\" hx-post=\"deal.note\" hx-trigger=\"change\" />\
         <button id=\"go\" type=\"button\" hx-post=\"deal.send\" hx-trigger=\"click, keydown[key=='F2'] from:body\">Send</button>\
         <select id=\"pick\" name=\"pick\" hx-post=\"deal.pick\" hx-trigger=\"change\"><option value=\"a\">a</option><option value=\"b\" selected>b</option><option value=\"c\">c</option></select>\
         </form><div id=\"other\" hx-trigger=\"slice:other\" hx-target=\"#other\" hx-swap=\"innerHTML\">x</div></body></html>",
        fee_html_with(value, extra)
    ))
    .unwrap();
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0)).unwrap();
    e
}

fn boot(value: &str, key_releases: bool) -> Engine {
    boot_full(value, FEE, key_releases)
}

type Sent = (String, bool, Vec<(String, String)>, RequestId);

fn feed_opt(e: &mut Engine, ev: InputEvent, out: &mut Vec<Sent>, complete: bool) {
    let d = e.input(ev);
    for o in std::iter::once(d).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            name,
            trigger,
            args,
            ..
        } = o
        {
            out.push((name, matches!(trigger, TriggerInfo::Change), args, request));
            if complete {
                e.complete(request, Ok(()));
            }
        }
    }
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
}
fn feed(e: &mut Engine, ev: InputEvent, out: &mut Vec<Sent>) {
    feed_opt(e, ev, out, true)
}
fn key_ev(k: &str, pressed: bool, repeat: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat,
        pressed,
    })
}
fn press_key(e: &mut Engine, k: &str, out: &mut Vec<Sent>) {
    feed(e, key_ev(k, true, false), out);
    feed(e, key_ev(k, false, false), out);
}
fn pdown(e: &mut Engine, (x, y): (f32, f32), out: &mut Vec<Sent>, complete: bool) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed_opt(
        e,
        InputEvent::PointerDown { x, y, button, mods },
        out,
        complete,
    );
}
fn pup(e: &mut Engine, (x, y): (f32, f32), out: &mut Vec<Sent>, complete: bool) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    feed_opt(
        e,
        InputEvent::PointerUp { x, y, button, mods },
        out,
        complete,
    );
}
fn click_opt(e: &mut Engine, p: (f32, f32), out: &mut Vec<Sent>, complete: bool) {
    pdown(e, p, out, complete);
    pup(e, p, out, complete);
}
fn click(e: &mut Engine, p: (f32, f32), out: &mut Vec<Sent>) {
    click_opt(e, p, out, true)
}
fn center(e: &Engine, id: &str) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
}
/// Focus the fee (click) and replace everything with `typed` ("" empties).
fn empty_fee(e: &mut Engine, typed: &str, out: &mut Vec<Sent>) {
    if e.focus_state().focused_id.as_deref() != Some("fee") {
        let p = center(e, "fee");
        click(e, p, out);
    }
    feed(
        e,
        InputEvent::Key(KeyEvent {
            key: "a".into(),
            code: "KeyA".into(),
            mods: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
            pressed: true,
        }),
        out,
    );
    if typed.is_empty() {
        press_key(e, "Backspace", out);
    } else {
        feed(e, InputEvent::Text(typed.into()), out);
    }
    assert_eq!(e.test_editor_display("fee").as_deref(), Some(typed));
    out.clear();
}
fn val(e: &Engine, id: &str) -> String {
    e.attr(id, "value").unwrap_or_default()
}
/// The `fee` of every `change` the fee box sent.
fn fee_changes(out: &[Sent]) -> Vec<String> {
    out.iter()
        .filter(|(n, c, _, _)| *c && n == "deal.fee")
        .map(|(_, _, a, _)| {
            a.iter()
                .find(|(k, _)| k == "fee")
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        })
        .collect()
}
fn post_fee(out: &[Sent]) -> Option<String> {
    out.iter()
        .find(|(n, _, _, _)| n == "deal.send")
        .and_then(|(_, _, a, _)| a.iter().find(|(k, _)| k == "fee").map(|(_, v)| v.clone()))
}
fn shown(e: &mut Engine) -> Option<String> {
    e.test_editor_display("fee")
}

struct Slices {
    v: HashMap<String, u64>,
    l: Vec<(String, String, InputLimits)>,
}
impl SliceSource for Slices {
    fn version(&self, n: &str) -> Option<u64> {
        self.v.get(n).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        vec![]
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.l.clone()
    }
}
struct Frags(String);
impl FragmentSource for Frags {
    fn render(&mut self, section: &str, _: &[&str]) -> String {
        if section == "sec" {
            self.0.clone()
        } else {
            "y".into()
        }
    }
}

// --- Press, hold, release --------------------------------------------

/// Press, hold with repeats, release off the arrow: one change from the
/// sent value, and nothing before the release.
#[test]
fn h1_hold_then_release_off_the_arrow() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    pdown(&mut e, DOWN, &mut out, true);
    assert_eq!(val(&e, "fee"), "4.09");
    e.test_hold_spin_for(Duration::from_millis(700));
    assert_eq!(val(&e, "fee"), "4.05", "400 ms then every 100 ms");
    assert!(fee_changes(&out).is_empty(), "{out:?}");
    pup(&mut e, (5.0, 100.0), &mut out, true);
    assert_eq!(fee_changes(&out), ["4.05"]);
    assert_eq!(shown(&mut e).as_deref(), Some("4.05"));
}

/// Held to max from an emptied box: pinned at max, one change.
#[test]
fn h2_hold_from_empty_pins_at_max() {
    let mut e = boot("99.98", false);
    let mut out = vec![];
    empty_fee(&mut e, ".", &mut out);
    pdown(&mut e, UP, &mut out, true);
    e.test_hold_spin_for(Duration::from_millis(1500));
    assert_eq!(val(&e, "fee"), "100");
    pup(&mut e, UP, &mut out, true);
    assert_eq!(fee_changes(&out), ["100"]);
}

// --- Keys, key_releases on and off -----------------------------------

/// key_releases = false: the first keydown steps from the sent value and
/// sends at once; repeats step silently; the release sends the repeats.
#[test]
fn h3_key_releases_false_keydown_repeat_release() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    feed(&mut e, key_ev("ArrowUp", true, false), &mut out);
    assert_eq!(val(&e, "fee"), "4.11");
    assert_eq!(fee_changes(&out), ["4.11"], "keydown sends at once");
    feed(&mut e, key_ev("ArrowUp", true, true), &mut out);
    feed(&mut e, key_ev("ArrowUp", true, true), &mut out);
    assert_eq!(val(&e, "fee"), "4.13");
    assert_eq!(fee_changes(&out), ["4.11"], "repeats send nothing");
    feed(&mut e, key_ev("ArrowUp", false, false), &mut out);
    assert_eq!(fee_changes(&out), ["4.11", "4.13"]);
}

/// key_releases = true: keydown and repeats send nothing; the release
/// sends one change, stepped from the sent value.
#[test]
fn h4_key_releases_true_hold_then_release() {
    for typed in ["", ".", "3,5"] {
        let mut e = boot("4.1", true);
        let mut out = vec![];
        empty_fee(&mut e, typed, &mut out);
        feed(&mut e, key_ev("ArrowDown", true, false), &mut out);
        feed(&mut e, key_ev("ArrowDown", true, true), &mut out);
        assert_eq!(val(&e, "fee"), "4.08", "{typed:?}");
        assert!(fee_changes(&out).is_empty(), "{typed:?}: {out:?}");
        feed(&mut e, key_ev("ArrowDown", false, false), &mut out);
        assert_eq!(fee_changes(&out), ["4.08"], "{typed:?}");
    }
}

/// key_releases = true, Enter before the release reports the step; the
/// release then sends nothing.
#[test]
fn h5_key_releases_true_enter_before_release() {
    let mut e = boot("4.1", true);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    feed(&mut e, key_ev("ArrowUp", true, false), &mut out);
    press_key(&mut e, "Enter", &mut out);
    assert_eq!(fee_changes(&out), ["4.11"]);
    feed(&mut e, key_ev("ArrowUp", false, false), &mut out);
    assert_eq!(fee_changes(&out), ["4.11"], "release after Enter: nothing");
}

/// key_releases = true, cleared from max: the keydown stays at max and the
/// release sends nothing (by number: host "100.00").
#[test]
fn h6_key_releases_true_at_max_sends_nothing() {
    let mut e = boot("100.00", true);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "ArrowUp", &mut out);
    assert_eq!(val(&e, "fee"), "100");
    assert!(fee_changes(&out).is_empty(), "{out:?}");
}

/// The box is emptied again between keydown and release (key_releases).
#[test]
fn h7_key_releases_true_emptied_before_release() {
    let mut e = boot("4.1", true);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    feed(&mut e, key_ev("ArrowUp", true, false), &mut out);
    assert_eq!(val(&e, "fee"), "4.11");
    for _ in 0..4 {
        press_key(&mut e, "Backspace", &mut out);
    }
    eprintln!(
        "H7 after re-emptying: shown={:?} value={}",
        shown(&mut e),
        val(&e, "fee")
    );
    feed(&mut e, key_ev("ArrowUp", false, false), &mut out);
    eprintln!(
        "H7 release: shown={:?} value={} changes={:?}",
        shown(&mut e),
        val(&e, "fee"),
        fee_changes(&out)
    );
    let n = out.len();
    press_key(&mut e, "Tab", &mut out);
    eprintln!(
        "H7 tab: value={} changes={:?}",
        val(&e, "fee"),
        fee_changes(&out[n..])
    );
    let go = center(&e, "go");
    click(&mut e, go, &mut out);
    eprintln!("H7 post fee={:?}", post_fee(&out));
    assert_ne!(
        post_fee(&out).as_deref(),
        Some(""),
        "never posts an empty fee"
    );
}

// --- Limits, integer boxes, #87 --------------------------------------

/// Down from an emptied box whose sent value is at min, written `4.10`
/// with min `4.1`: no change (compare by number, #87).
#[test]
fn h8_step_onto_the_sent_value_by_number_sends_nothing() {
    for kr in [false, true] {
        let mut e = boot_full("4.10", "min=\"4.1\" max=\"100\" step=\"0.01\"", kr);
        let mut out = vec![];
        empty_fee(&mut e, "", &mut out);
        click(&mut e, DOWN, &mut out);
        press_key(&mut e, "ArrowDown", &mut out);
        assert!(fee_changes(&out).is_empty(), "kr {kr}: {out:?}");
        assert!(["4.1", "4.10"].contains(&val(&e, "fee").as_str()));
        let n = out.len();
        press_key(&mut e, "Tab", &mut out);
        assert!(fee_changes(&out[n..]).is_empty(), "kr {kr}: {out:?}");
    }
}

/// A whole-number box steps by its step from the sent value.
#[test]
fn h9_integer_box() {
    let mut e = boot_full("7", "min=\"0\" max=\"10\" step=\"1\"", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    assert_eq!(val(&e, "fee"), "8");
    assert_eq!(shown(&mut e).as_deref(), Some("8"));
    empty_fee(&mut e, ".", &mut out);
    press_key(&mut e, "ArrowDown", &mut out);
    assert_eq!(fee_changes(&out), ["7"], "from the sent 8");
    // Off-grid sent value (host 151234, step 1000) snaps in the arrow's
    // direction, as from a typed value.
    let mut e = boot_full("151234", "min=\"0\" max=\"900000\" step=\"1000\"", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    assert_eq!(fee_changes(&out), ["152000"]);
}

/// The step from the sent value respects a limits refresh that arrived
/// while focused and empty (new max 3 below the sent 4.1).
#[test]
fn h10_limits_refresh_while_empty_then_arrow() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    let s = Slices {
        v: HashMap::from([("actions".into(), 2)]),
        l: vec![(
            "deal.fee".into(),
            "fee".into(),
            InputLimits::new(0.0, 3.0, Some(0.01)),
        )],
    };
    e.sync(&s, &mut Frags(fee_html("4.1"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
    click(&mut e, DOWN, &mut out);
    eprintln!(
        "H10 ▼ after max→3: value={} changes={:?}",
        val(&e, "fee"),
        fee_changes(&out)
    );
    let v: f64 = val(&e, "fee").parse().unwrap();
    assert!(v <= 3.0, "never above the new max");
}

// --- #89, a press while an earlier change is pending -----------------

/// ▲ sent 4.11 and the game has not answered; the box is emptied and ▲
/// pressed again. The base is the sent (unanswered) 4.11, the same value
/// Tab would put back; whether the second change goes out is #89's open
/// question, not #92's.
#[test]
fn h11_pending_change_then_empty_then_arrow() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    click_opt(&mut e, UP, &mut out, false);
    assert_eq!(fee_changes(&out), ["4.11"]);
    let first = out[0].3;
    empty_fee(&mut e, "", &mut out);
    click_opt(&mut e, UP, &mut out, false);
    assert_eq!(val(&e, "fee"), "4.12", "base is the sent 4.11");
    eprintln!(
        "H11 second ▲ while pending: changes={:?}",
        fee_changes(&out)
    );
    e.complete(first, Ok(()));
    let late: Vec<_> = e.take_outcomes();
    eprintln!("H11 after complete: {late:?}");
}

// --- Shortcut and blur after a step (#91, value_at_change) -----------

/// After ▲ from empty, empty again: the shortcut posts the stepped value
/// (now the last sent), and Tab puts it back with no change.
#[test]
fn h12_shortcut_and_blur_after_a_step_from_empty() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    assert_eq!(fee_changes(&out), ["4.11"]);
    empty_fee(&mut e, "", &mut out);
    press_key(&mut e, "F2", &mut out);
    assert_eq!(post_fee(&out).as_deref(), Some("4.11"), "{out:?}");
    out.clear();
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(val(&e, "fee"), "4.11");
    assert!(fee_changes(&out).is_empty(), "{out:?}");
}

/// key_releases = true: a shortcut between the keydown step and the release
/// posts the stepped value, never "".
#[test]
fn h13_shortcut_between_keydown_and_release() {
    let mut e = boot("4.1", true);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    feed(&mut e, key_ev("ArrowUp", true, false), &mut out);
    press_key(&mut e, "F2", &mut out);
    assert_eq!(post_fee(&out).as_deref(), Some("4.11"), "{out:?}");
    feed(&mut e, key_ev("ArrowUp", false, false), &mut out);
    assert_eq!(fee_changes(&out), ["4.11"]);
}

// --- What the box shows, and the next post ---------------------------

/// Grouped box: ▲ from empty shows the raw value while focused, the
/// grouped one after blur; the post carries the canonical number.
#[test]
fn h14_grouped_box_display_and_post() {
    let mut e = boot_full(
        "151000",
        "min=\"0\" max=\"900000\" step=\"1000\" data-hx-group=\"on\"",
        false,
    );
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    assert_eq!(shown(&mut e).as_deref(), Some("152000"));
    press_key(&mut e, "Tab", &mut out);
    assert_eq!(shown(&mut e).as_deref(), Some("152,000"));
    let go = center(&e, "go");
    click(&mut e, go, &mut out);
    assert_eq!(post_fee(&out).as_deref(), Some("152000"));
    assert_eq!(fee_changes(&out), ["152000"]);
}

/// Fee box: ▲ from empty shows 4.11 while focused and after blur; the
/// next post carries 4.11.
#[test]
fn h15_fee_display_and_post() {
    let mut e = boot("4.10", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    click(&mut e, UP, &mut out);
    assert_eq!(shown(&mut e).as_deref(), Some("4.11"));
    let go = center(&e, "go");
    click(&mut e, go, &mut out);
    assert_eq!(post_fee(&out).as_deref(), Some("4.11"));
    eprintln!("H15 after blur shown={:?}", shown(&mut e));
}

// --- Swaps and an empty starting value -------------------------------

/// Unrelated swap while empty: still steps from the sent value.
#[test]
fn h16_unrelated_swap_while_empty() {
    let mut e = boot("4.1", false);
    let mut out = vec![];
    let s = Slices {
        v: HashMap::from([("fee".into(), 1), ("other".into(), 1)]),
        l: vec![],
    };
    e.sync(&s, &mut Frags(fee_html("4.1"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
    empty_fee(&mut e, "", &mut out);
    let s = Slices {
        v: HashMap::from([("fee".into(), 1), ("other".into(), 2)]),
        l: vec![],
    };
    e.sync(&s, &mut Frags(fee_html("9"))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
    assert_eq!(shown(&mut e).as_deref(), Some(""));
    click(&mut e, UP, &mut out);
    assert_eq!(fee_changes(&out), ["4.11"]);
}

/// The box itself is re-rendered while empty: the new markup's value is the
/// game's and the step starts from it.
#[test]
fn h17_same_id_rerender_while_empty() {
    for (game, kr) in [("5", false), ("5", true)] {
        let mut e = boot("4.1", kr);
        let mut out = vec![];
        empty_fee(&mut e, "", &mut out);
        let s = Slices {
            v: HashMap::from([("fee".into(), 2)]),
            l: vec![],
        };
        e.sync(&s, &mut Frags(fee_html(game))).unwrap();
        let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
        eprintln!(
            "H17 after re-render shown={:?} value={}",
            shown(&mut e),
            val(&e, "fee")
        );
        press_key(&mut e, "ArrowUp", &mut out);
        assert_eq!(fee_changes(&out), ["5.01"], "kr {kr}");
    }
}

/// Re-rendered with an empty value while focused, and a host page that
/// starts empty: nothing was ever a number, so the step falls back to min
/// (0.01 here), as before #93. Pinned so a change is deliberate.
#[test]
fn h18_empty_starting_value_steps_from_min() {
    let mut e = boot("", false);
    let mut out = vec![];
    click(&mut e, UP, &mut out);
    assert_eq!(val(&e, "fee"), "0.01");
    assert_eq!(fee_changes(&out), ["0.01"]);
    let mut e = boot("4.1", false);
    let mut out = vec![];
    empty_fee(&mut e, "", &mut out);
    let s = Slices {
        v: HashMap::from([("fee".into(), 2)]),
        l: vec![],
    };
    e.sync(&s, &mut Frags(fee_html(""))).unwrap();
    let _ = e.paint(Viewport::with_scale(200, 110, 1.0));
    click(&mut e, UP, &mut out);
    eprintln!(
        "H18 re-rendered empty then ▲: value={} changes={:?}",
        val(&e, "fee"),
        fee_changes(&out)
    );
    assert_eq!(val(&e, "fee"), "0.01");
}

// --- Text boxes and selects ------------------------------------------

/// ArrowUp/Down in an emptied text box do nothing to it; Tab commits "".
#[test]
fn h19_text_box_unaffected() {
    for kr in [false, true] {
        let mut e = boot("4.1", kr);
        let mut out = vec![];
        let p = center(&e, "note");
        click(&mut e, p, &mut out);
        press_key(&mut e, "End", &mut out);
        press_key(&mut e, "Backspace", &mut out);
        press_key(&mut e, "Backspace", &mut out);
        press_key(&mut e, "ArrowUp", &mut out);
        press_key(&mut e, "ArrowDown", &mut out);
        assert_eq!(val(&e, "note"), "", "kr {kr}");
        press_key(&mut e, "Tab", &mut out);
        let notes: Vec<_> = out
            .iter()
            .filter(|(n, c, _, _)| *c && n == "deal.note")
            .collect();
        assert_eq!(notes.len(), 1, "kr {kr}: {out:?}");
        assert_eq!(val(&e, "note"), "");
    }
}

/// A focused closed select still steps its option with the arrows.
#[test]
fn h20_select_unaffected() {
    let mut e = boot("4.1", true);
    let mut out = vec![];
    e.test_set_focus("pick");
    press_key(&mut e, "ArrowDown", &mut out);
    let picks: Vec<_> = out
        .iter()
        .filter(|(n, c, _, _)| *c && n == "deal.pick")
        .map(|(_, _, a, _)| a.iter().find(|(k, _)| k == "pick").map(|(_, v)| v.clone()))
        .collect();
    assert_eq!(picks, [Some("c".to_string())], "{out:?}");
    assert!(fee_changes(&out).is_empty());
}
