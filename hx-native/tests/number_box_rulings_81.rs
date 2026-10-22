//! Issue #81: Designer's rulings on the number box (comment 6335457293,
//! HTML_CONTRACT §2 Rulings 4-6, Mainnet master 28f3a7b1), one test each.
//!
//! 1. Esc after an arrow step reverts to the last committed value.
//! 2. A comma refused after a step fires no `change`.
//! 3. After an arrow click the focus stays in the box, caret at the end,
//!    nothing selected (a Tab selection included).
//! 4. Holding an arrow steps once on press, then every 100 ms after 400 ms,
//!    with no speed-up, clamped at min/max, and one `change` on release.
//!    Keyboard ArrowUp/ArrowDown key repeat follows the same rule.
//! 5. A box greyed mid-edit drops the typed text, shows the game's value,
//!    sends nothing and blurs.

use std::collections::HashMap;
use std::time::Duration;

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#n{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}</style>";

const WHOLE: &str = r#"min="0" max="10" step="1""#;
const WIDE: &str = r#"min="0" max="100" step="1""#;

fn page(attrs: &str, value: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
         <input id=\"n\" type=\"number\" name=\"n\" value=\"{value}\" {attrs} hx-post=\"deal.n\" hx-trigger=\"change\" />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"n\">▲</span>\
         <span id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"n\">▼</span>\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"x\" style=\"position:absolute;left:0;top:40px\" />\
         </form></body></html>"
    )
}

fn vp() -> Viewport {
    Viewport::with_scale(200, 80, 1.0)
}

fn boot(attrs: &str, value: &str) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&page(attrs, value)).unwrap();
    let _ = e.paint(vp()).unwrap();
    e
}

/// Feed one event and collect the `n` value of every `change` it produced.
fn feed(e: &mut Engine, ev: InputEvent, changes: &mut Vec<String>) {
    let direct = e.input(ev);
    for o in std::iter::once(direct).chain(e.take_outcomes()) {
        if let InputOutcome::Action {
            request,
            trigger: TriggerInfo::Change,
            args,
            ..
        } = o
        {
            let v = args
                .iter()
                .find(|(k, _)| k == "n")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            changes.push(v);
            e.complete(request, Ok(()));
        }
    }
}

fn key_ev(k: &str, ctrl: bool, pressed: bool, repeat: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            ..Modifiers::default()
        },
        repeat,
        pressed,
    })
}

fn press(k: &str) -> InputEvent {
    key_ev(k, false, true, false)
}

const UP: (f32, f32) = (140.0, 6.0);
const DOWN: (f32, f32) = (140.0, 18.0);
const OTHER: (f32, f32) = (10.0, 46.0);

fn down(e: &mut Engine, at: (f32, f32), ch: &mut Vec<String>) {
    feed(
        e,
        InputEvent::PointerDown {
            x: at.0,
            y: at.1,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        ch,
    );
}

fn up(e: &mut Engine, at: (f32, f32), ch: &mut Vec<String>) {
    feed(
        e,
        InputEvent::PointerUp {
            x: at.0,
            y: at.1,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        ch,
    );
}

fn click(e: &mut Engine, at: (f32, f32), ch: &mut Vec<String>) {
    down(e, at, ch);
    up(e, at, ch);
}

/// Focus `#n`, select all and type `typed` (uncommitted).
fn type_into(e: &mut Engine, typed: &str, ch: &mut Vec<String>) {
    e.test_set_focus("n");
    feed(e, key_ev("a", true, true, false), ch);
    feed(e, InputEvent::Text(typed.into()), ch);
}

/// Select all and type `typed` into the box already being edited (no
/// re-focus, which would start a new edit).
fn retype(e: &mut Engine, typed: &str, ch: &mut Vec<String>) {
    feed(e, key_ev("a", true, true, false), ch);
    feed(e, InputEvent::Text(typed.into()), ch);
}

fn val(e: &Engine) -> String {
    e.attr("n", "value").unwrap_or_default()
}

// ---------- 1. Esc ----------

/// Ruling: Esc after an arrow click reverts to the value its `change`
/// reported, not to the value at focus.
#[test]
fn esc_after_click_step_reverts_to_the_last_committed_value() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    type_into(&mut e, "3.7", &mut ch);
    click(&mut e, UP, &mut ch);
    assert_eq!(ch, ["4"]);
    feed(&mut e, InputEvent::Text("9".into()), &mut ch);
    feed(&mut e, press("Escape"), &mut ch);
    assert_eq!(val(&e), "4");
    assert_eq!(e.test_editor_display("n").as_deref(), Some("4"));
    assert_eq!(ch, ["4"], "Esc sends nothing");
}

/// Ruling, keyboard: Esc after ArrowUp and more typing reverts to the
/// stepped value the ArrowUp's `change` reported.
#[test]
fn esc_after_key_step_reverts_to_the_last_committed_value() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    e.test_set_focus("n");
    feed(&mut e, press("ArrowUp"), &mut ch);
    retype(&mut e, "9", &mut ch);
    feed(&mut e, press("Escape"), &mut ch);
    assert_eq!(val(&e), "3");
    assert_eq!(e.test_editor_display("n").as_deref(), Some("3"));
    assert_eq!(ch, ["3"], "Esc sends nothing");
}

/// With no step, Esc still goes back to the value at focus.
#[test]
fn esc_without_a_step_still_restores_the_focus_value() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    type_into(&mut e, "7", &mut ch);
    feed(&mut e, press("Escape"), &mut ch);
    assert_eq!(val(&e), "2");
    assert!(ch.is_empty());
}

// ---------- 2. refused comma ----------

/// Ruling: a comma typed after a step is refused back to the stepped value,
/// which the step's `change` already reported, so no `change` fires.
#[test]
fn refused_comma_after_a_step_fires_no_change() {
    // Keyboard step, then Enter.
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    e.test_set_focus("n");
    feed(&mut e, press("ArrowUp"), &mut ch);
    assert_eq!(ch, ["3"]);
    retype(&mut e, "3,5", &mut ch);
    feed(&mut e, press("Enter"), &mut ch);
    assert_eq!(val(&e), "3");
    assert_eq!(ch, ["3"], "Enter after a refused comma");
    // Arrow click, then blur.
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    e.test_set_focus("n");
    click(&mut e, UP, &mut ch);
    assert_eq!(ch, ["3"]);
    retype(&mut e, "3,5", &mut ch);
    click(&mut e, OTHER, &mut ch);
    assert_eq!(val(&e), "3");
    assert_eq!(ch, ["3"], "blur after a refused comma");
}

// ---------- 3. focus ----------

/// Ruling: after an arrow click the box keeps the focus with the caret at
/// the end and nothing selected, from an unfocused box, a typed edit, or a
/// Tab selection; typing then appends.
#[test]
fn arrow_click_keeps_focus_in_the_box_caret_at_end() {
    for start in ["unfocused", "typed", "tab"] {
        let mut e = boot(WHOLE, "2");
        let mut ch = Vec::new();
        match start {
            "typed" => type_into(&mut e, "3", &mut ch),
            "tab" => {
                // Shift+Tab from the text box back into the number box.
                e.test_set_focus("other");
                let back = InputEvent::Key(KeyEvent {
                    key: "Tab".into(),
                    code: "Tab".into(),
                    mods: Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    },
                    repeat: false,
                    pressed: true,
                });
                feed(&mut e, back, &mut ch);
                assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
                assert!(e.test_selected_text("n").is_some(), "Tab selects");
            }
            _ => {}
        }
        click(&mut e, UP, &mut ch);
        assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"), "{start}");
        assert_eq!(e.test_selected_text("n"), None, "{start}: nothing selected");
        let stepped = val(&e);
        feed(&mut e, InputEvent::Text("5".into()), &mut ch);
        assert_eq!(
            e.test_editor_display("n"),
            Some(format!("{stepped}5")),
            "{start}: caret at the end"
        );
    }
}

/// An arrow click leaves another box being edited the way any focus move
/// does: that box commits with its own `change`.
#[test]
fn arrow_click_commits_another_box_being_edited() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    e.test_set_focus("other");
    feed(&mut e, InputEvent::Text("y".into()), &mut ch);
    click(&mut e, UP, &mut ch);
    assert_eq!(e.attr("other", "value").as_deref(), Some("xy"));
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
    assert_eq!(val(&e), "3");
}

// ---------- 4. hold ----------

/// Ruling: one step on press, none more before 400 ms, then one every
/// 100 ms with no speed-up, and one `change` with the final value on
/// release.
#[test]
fn held_arrow_repeats_after_400ms_every_100ms() {
    let mut e = boot(WIDE, "10");
    let mut ch = Vec::new();
    down(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "11", "the press steps once");
    e.test_hold_spin_for(Duration::from_millis(399));
    assert_eq!(val(&e), "11", "no repeat before 400 ms");
    e.test_hold_spin_for(Duration::from_millis(400));
    assert_eq!(val(&e), "12", "first repeat at 400 ms");
    e.test_hold_spin_for(Duration::from_millis(499));
    assert_eq!(val(&e), "12");
    e.test_hold_spin_for(Duration::from_millis(500));
    assert_eq!(val(&e), "13");
    // No acceleration: 2 s held is 1 + (2000 - 400) / 100 + 1 = 18 steps.
    e.test_hold_spin_for(Duration::from_millis(2000));
    assert_eq!(val(&e), "28");
    assert!(ch.is_empty(), "no change while held: {ch:?}");
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
    up(&mut e, UP, &mut ch);
    assert_eq!(ch, ["28"], "one change on release");
    // Nothing more repeats after the release.
    e.test_hold_spin_for(Duration::from_millis(3000));
    assert_eq!(val(&e), "28");
}

/// Ruling: the hold stops at max (and min) like a single click, still with
/// one `change` on release.
#[test]
fn held_arrow_clamps_at_max_and_min() {
    let mut e = boot(WHOLE, "8");
    let mut ch = Vec::new();
    down(&mut e, UP, &mut ch);
    e.test_hold_spin_for(Duration::from_millis(1500));
    assert_eq!(val(&e), "10");
    up(&mut e, UP, &mut ch);
    assert_eq!(ch, ["10"]);
    down(&mut e, DOWN, &mut ch);
    e.test_hold_spin_for(Duration::from_millis(3000));
    assert_eq!(val(&e), "0");
    up(&mut e, DOWN, &mut ch);
    assert_eq!(ch, ["10", "0"]);
}

/// Keyboard: the first ArrowUp steps with its `change` as before; key
/// repeat steps without one; the key release sends one for the repeats.
#[test]
fn key_repeat_sends_one_change_on_key_release() {
    let mut e = boot(WIDE, "10");
    let mut ch = Vec::new();
    e.test_set_focus("n");
    feed(&mut e, press("ArrowUp"), &mut ch);
    assert_eq!(ch, ["11"]);
    for _ in 0..4 {
        feed(&mut e, key_ev("ArrowUp", false, true, true), &mut ch);
    }
    assert_eq!(val(&e), "15");
    assert_eq!(ch, ["11"], "no change per repeat");
    feed(&mut e, key_ev("ArrowUp", false, false, false), &mut ch);
    assert_eq!(ch, ["11", "15"]);
    // A tap (no repeat) still reports once, on the press only.
    feed(&mut e, press("ArrowDown"), &mut ch);
    feed(&mut e, key_ev("ArrowDown", false, false, false), &mut ch);
    assert_eq!(ch, ["11", "15", "14"]);
    // A host without key releases: the blur reports the repeat steps.
    feed(&mut e, key_ev("ArrowUp", false, true, true), &mut ch);
    click(&mut e, OTHER, &mut ch);
    assert_eq!(ch, ["11", "15", "14", "15"]);
}

/// Partial repaint during and after a hold equals a forced full repaint.
#[test]
fn held_arrow_partial_equals_full() {
    let mut e = boot(WIDE, "10");
    let mut ch = Vec::new();
    let _ = e.paint(vp()).unwrap();
    let check = |e: &mut Engine, what: &str| {
        let out = e.paint(vp()).unwrap();
        assert!(!out.damage.is_empty(), "{what}: must damage the box");
        assert!(!e.last_paint_stats().full_repaint, "{what}: partial");
        let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
        e.mark_theme_or_stylesheet_changed();
        let full = e.paint(vp()).unwrap().rgba.expect("full");
        let diff = partial
            .pixels
            .chunks_exact(4)
            .zip(full.pixels.chunks_exact(4))
            .filter(|(p, q)| p != q)
            .count();
        assert_eq!(diff, 0, "{what}: partial must equal full");
    };
    down(&mut e, UP, &mut ch);
    check(&mut e, "press");
    e.test_hold_spin_for(Duration::from_millis(900));
    check(&mut e, "repeat");
    up(&mut e, UP, &mut ch);
    feed(&mut e, InputEvent::Text("7".into()), &mut ch);
    check(&mut e, "typing after release");
    assert_eq!(ch, ["17"]);
}

// ---------- 5. greyed mid-edit ----------

#[derive(Default)]
struct Slices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}

impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}

struct NoFrags;

impl FragmentSource for NoFrags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        format!("<!-- {section_id} -->")
    }
}

fn grey(e: &mut Engine) {
    let mut s = Slices::default();
    s.versions.insert("actions".into(), 2);
    s.actions = vec![(
        "deal.n".into(),
        ActionState::Disabled {
            reason: "Deal closed".into(),
        },
    )];
    e.sync(&s, &mut NoFrags).unwrap();
}

/// Ruling: greyed while typing, the box drops the typed text, shows the
/// game's value, sends nothing and blurs; the form no longer carries the
/// typed text.
#[test]
fn greyed_mid_edit_drops_the_typed_value_and_blurs() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    type_into(&mut e, "7", &mut ch);
    assert_eq!(val(&e), "7");
    grey(&mut e);
    assert!(e.attr("n", "disabled").is_some(), "greyed");
    assert_eq!(val(&e), "2", "the game's value");
    assert_eq!(e.test_editor_display("n").as_deref(), Some("2"));
    assert_eq!(e.focus_state().focused_id, None, "blurred");
    feed(&mut e, InputEvent::Text("9".into()), &mut ch);
    feed(&mut e, press("Enter"), &mut ch);
    click(&mut e, OTHER, &mut ch);
    assert_eq!(val(&e), "2");
    assert!(ch.is_empty(), "nothing sent: {ch:?}");
}

/// The game's value is the last one it was sent: after a stepped `change`,
/// greying mid-edit shows the stepped value, and a hold in progress stops
/// without a `change`.
#[test]
fn greyed_mid_edit_after_a_step_shows_the_reported_value() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    e.test_set_focus("n");
    click(&mut e, UP, &mut ch);
    feed(&mut e, InputEvent::Text("5".into()), &mut ch);
    assert_eq!(ch, ["3"]);
    grey(&mut e);
    assert_eq!(val(&e), "3");
    assert_eq!(e.test_editor_display("n").as_deref(), Some("3"));

    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    down(&mut e, UP, &mut ch);
    e.test_hold_spin_for(Duration::from_millis(600));
    assert_eq!(val(&e), "6");
    grey(&mut e);
    assert_eq!(val(&e), "2", "unsent hold steps are dropped");
    e.test_hold_spin_for(Duration::from_millis(900));
    up(&mut e, UP, &mut ch);
    assert_eq!(val(&e), "2");
    assert!(ch.is_empty(), "nothing sent: {ch:?}");
    assert_eq!(e.focus_state().focused_id, None);
}

/// Partial repaint after greying mid-edit equals a forced full repaint.
#[test]
fn greyed_mid_edit_partial_equals_full() {
    let mut e = boot(WHOLE, "2");
    let mut ch = Vec::new();
    type_into(&mut e, "7", &mut ch);
    let _ = e.paint(vp()).unwrap();
    grey(&mut e);
    let partial = e.paint(vp()).unwrap();
    let partial = partial
        .rgba
        .unwrap_or_else(|| e.last_frame().unwrap().clone());
    e.mark_theme_or_stylesheet_changed();
    let full = e.paint(vp()).unwrap().rgba.expect("full");
    let diff = partial
        .pixels
        .chunks_exact(4)
        .zip(full.pixels.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    assert_eq!(diff, 0, "partial must equal full");
}
