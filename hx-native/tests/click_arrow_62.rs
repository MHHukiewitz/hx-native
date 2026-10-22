//! Issue #62 (found by UI on Mainnet #763, confirmed by Designer): clicking
//! the ▲/▼ spin arrows while a typed value is still uncommitted commits that
//! value and snaps it to the step grid in the arrow's direction, exactly like
//! ArrowUp/ArrowDown on the keyboard and Chrome's spinner (`3.7` ▲ gives `4`,
//! ▼ gives `3`). It never commits first and then adds a step on top. A value
//! that is already committed and on the grid steps by one, as before. One
//! `change` fires per click.

use hx_native::*;

const STYLE: &str = "<style>html,body{margin:0;background:#fff}\
#n{position:absolute;left:0;top:0;width:120px;height:24px;font-size:14px;font-family:\"DejaVu Sans\"}\
#up{position:absolute;left:130px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:130px;top:12px;width:24px;height:12px}</style>";

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

fn boot(attrs: &str, value: &str, mode: PaintMode) -> Engine {
    let mut e = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: mode,
        ..Default::default()
    });
    e.declare_actions(&["deal.n"]);
    e.load_html(&page(attrs, value)).unwrap();
    let _ = e.paint(vp()).unwrap();
    e
}

/// Feed one event, collect every `change` it produced (returned and queued),
/// complete each request so the next one is not held by `hx-sync`.
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

fn key_ev(k: &str, ctrl: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    })
}

fn click(e: &mut Engine, x: f32, y: f32, changes: &mut Vec<String>) {
    feed(
        e,
        InputEvent::PointerMove {
            x,
            y,
            mods: Modifiers::default(),
        },
        changes,
    );
    feed(
        e,
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        changes,
    );
    feed(
        e,
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        changes,
    );
}

/// Focus `#n`, select all and type `typed` (uncommitted).
fn type_into(e: &mut Engine, typed: &str, changes: &mut Vec<String>) {
    e.test_set_focus("n");
    feed(e, key_ev("a", true), changes);
    feed(e, InputEvent::Text(typed.into()), changes);
}

#[derive(Debug, PartialEq)]
struct Result62 {
    value: Option<String>,
    shown: Option<String>,
    changes: Vec<String>,
}

/// Keyboard: type, ArrowUp/Down, then Enter commits.
fn keyboard(attrs: &str, start: &str, typed: &str, up: bool) -> Result62 {
    let mut e = boot(attrs, start, PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut e, typed, &mut ch);
    feed(
        &mut e,
        key_ev(if up { "ArrowUp" } else { "ArrowDown" }, false),
        &mut ch,
    );
    feed(&mut e, key_ev("Enter", false), &mut ch);
    Result62 {
        value: e.attr("n", "value"),
        shown: e.test_editor_display("n"),
        changes: ch,
    }
}

/// Click: type, then click ▲/▼.
fn clicked(attrs: &str, start: &str, typed: &str, up: bool) -> Result62 {
    let mut e = boot(attrs, start, PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut e, typed, &mut ch);
    click(&mut e, 140.0, if up { 6.0 } else { 18.0 }, &mut ch);
    Result62 {
        value: e.attr("n", "value"),
        shown: e.test_editor_display("n"),
        changes: ch,
    }
}

const WHOLE: &str = r#"min="0" max="10" step="1""#;
const FEE: &str = r#"min="0" max="100.00" step="0.01""#;
const ANY: &str = r#"min="0" max="10" step="any""#;
const GROUPED: &str = r#"min="0" max="1000000" step="1" data-hx-group="on""#;

/// (case, attrs, committed start, typed, ▲?, expected value)
const TABLE: &[(&str, &str, &str, &str, bool, &str)] = &[
    ("on grid ▲", WHOLE, "2", "3", true, "4"),
    ("on grid ▼", WHOLE, "2", "3", false, "2"),
    ("off grid ▲ (3.7)", WHOLE, "2", "3.7", true, "4"),
    ("off grid ▼ (3.7)", WHOLE, "2", "3.7", false, "3"),
    ("off grid ▲ (3.2)", WHOLE, "2", "3.2", true, "4"),
    ("off grid ▼ (3.2)", WHOLE, "2", "3.2", false, "3"),
    ("at max ▲", WHOLE, "2", "10", true, "10"),
    ("at max ▼", WHOLE, "2", "10", false, "9"),
    ("above max ▲", WHOLE, "2", "12", true, "10"),
    ("above max ▼", WHOLE, "2", "12", false, "10"),
    ("at min ▼", WHOLE, "2", "0", false, "0"),
    ("at min ▲", WHOLE, "2", "0", true, "1"),
    ("fee on grid ▲", FEE, "4.1", "4.2", true, "4.21"),
    ("fee off grid ▲", FEE, "4.1", "4.105", true, "4.11"),
    ("fee off grid ▼", FEE, "4.1", "4.105", false, "4.1"),
    ("fee off max ▲", FEE, "4.1", "99.996", true, "100"),
    ("step=any ▲", ANY, "2", "3.7", true, "4.7"),
    ("step=any ▼", ANY, "2", "3.7", false, "2.7"),
    ("grouped off grid ▲", GROUPED, "2", "1234.5", true, "1235"),
    // #59 (issue #51): a typed comma that is not grouping is no number, so
    // the value is the focus-time one; both arrows step from there.
    (
        "typed grouping comma ▲",
        GROUPED,
        "2",
        "1,234.5",
        true,
        "1235",
    ),
    ("refused comma ▲ (3,5)", WHOLE, "2", "3,5", true, "3"),
    ("refused comma ▼ (3,5)", WHOLE, "2", "3,5", false, "1"),
];

/// The acceptance table: from a typed, uncommitted value, keyboard
/// (ArrowUp/Down, then Enter) and click (▲/▼) give the same committed value
/// and exactly one `change`, carrying that value.
#[test]
fn keyboard_and_click_agree_from_typed_values() {
    println!("| case | typed | want | keyboard | click | changes (key / click) |");
    for (name, attrs, start, typed, up, want) in TABLE {
        let k = keyboard(attrs, start, typed, *up);
        let c = clicked(attrs, start, typed, *up);
        println!(
            "| {name} | {typed} | {want} | {} | {} | {:?} / {:?} |",
            k.value.as_deref().unwrap_or("-"),
            c.value.as_deref().unwrap_or("-"),
            k.changes,
            c.changes
        );
        assert_eq!(k.value.as_deref(), Some(*want), "keyboard {name}");
        assert_eq!(c.value.as_deref(), Some(*want), "click {name}");
        assert_eq!(k.changes, vec![want.to_string()], "keyboard change {name}");
        assert_eq!(c.changes, vec![want.to_string()], "click change {name}");
    }
}

/// The click commits and (issue #81, Designer ruling 1) keeps the box
/// focused with its raw editable text; leaving the box then shows the
/// unfocused display of the committed value, with no second `change`.
/// Before #81 the click blurred, so the display was checked right away.
#[test]
fn click_commit_shows_the_unfocused_display() {
    for (attrs, start, typed, up, raw, shown, want) in [
        (GROUPED, "2", "1234.5", true, "1235", "1,235", "1235"),
        (FEE, "4.1", "4.105", false, "4.10", "4.10", "4.1"),
    ] {
        let mut e = boot(attrs, start, PaintMode::Exact);
        let mut ch = Vec::new();
        type_into(&mut e, typed, &mut ch);
        click(&mut e, 140.0, if up { 6.0 } else { 18.0 }, &mut ch);
        assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
        assert_eq!(e.test_editor_display("n").as_deref(), Some(raw));
        click(&mut e, 10.0, 46.0, &mut ch);
        assert_eq!(e.test_editor_display("n").as_deref(), Some(shown));
        assert_eq!(ch, [want]);
    }
}

/// A committed on-grid value steps one step per click, as before, focused
/// or not, with one `change` per click.
#[test]
fn committed_on_grid_value_steps_as_before() {
    let mut e = boot(WHOLE, "3", PaintMode::Exact);
    let mut ch = Vec::new();
    click(&mut e, 140.0, 6.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("4"));
    click(&mut e, 140.0, 6.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("5"));
    click(&mut e, 140.0, 18.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("4"));
    assert_eq!(ch, ["4", "5", "4"]);
    // Focused, nothing typed: still one step.
    let mut e = boot(WHOLE, "3", PaintMode::Exact);
    let mut ch = Vec::new();
    e.test_set_focus("n");
    click(&mut e, 140.0, 6.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("4"));
    assert_eq!(ch, ["4"]);
    // At max a click still reports its one `change`, as before.
    let mut e = boot(WHOLE, "10", PaintMode::Exact);
    let mut ch = Vec::new();
    e.test_set_focus("n");
    click(&mut e, 140.0, 6.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("10"));
    assert_eq!(ch, ["10"]);
}

/// Focus ends where a click on a committed value leaves it: in the box
/// (issue #81, Designer ruling 1; before #81 both left it on the arrow).
#[test]
fn focus_after_click_matches_a_committed_click() {
    let mut a = boot(WHOLE, "2", PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut a, "3.7", &mut ch);
    click(&mut a, 140.0, 6.0, &mut ch);
    let mut b = boot(WHOLE, "2", PaintMode::Exact);
    click(&mut b, 140.0, 6.0, &mut ch);
    assert_eq!(a.focus_state().focused_id.as_deref(), Some("n"));
    assert_eq!(
        a.focus_state().focused_id,
        b.focus_state().focused_id,
        "typed and committed clicks leave focus in the same place"
    );
}

/// Keyboard step then click: each gesture reports its own value once.
#[test]
fn arrow_key_then_click_reports_each_step_once() {
    let mut e = boot(WHOLE, "2", PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut e, "3.7", &mut ch);
    feed(&mut e, key_ev("ArrowUp", false), &mut ch);
    click(&mut e, 140.0, 6.0, &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("5"));
    assert_eq!(ch, ["4", "5"]);
}

/// Esc after an arrow step restores the last committed value, the one the
/// step's `change` reported (issue #81 item 1, Designer ruling). Before #81
/// it went back to the focus-time `2`, a value the game never got back.
#[test]
fn esc_after_arrow_restores_the_last_committed_value() {
    let mut e = boot(WHOLE, "2", PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut e, "3.7", &mut ch);
    feed(&mut e, key_ev("ArrowUp", false), &mut ch);
    feed(&mut e, InputEvent::Text("9".into()), &mut ch);
    feed(&mut e, key_ev("Escape", false), &mut ch);
    assert_eq!(e.attr("n", "value").as_deref(), Some("4"));
    assert_eq!(e.test_editor_display("n").as_deref(), Some("4"));
    assert_eq!(ch, ["4"]);
}

/// Issue #81 (Designer ruling 2): the press on ▲ steps, so releasing
/// elsewhere still ends the press with its one `change`, and the box keeps
/// the focus. Before #81 the step waited for a release on the arrow, and a
/// release elsewhere kept the typed value uncommitted.
#[test]
fn release_off_the_arrow_sends_the_press_step() {
    let mut e = boot(WHOLE, "2", PaintMode::Exact);
    let mut ch = Vec::new();
    type_into(&mut e, "3.7", &mut ch);
    feed(
        &mut e,
        InputEvent::PointerDown {
            x: 140.0,
            y: 6.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        &mut ch,
    );
    feed(
        &mut e,
        InputEvent::PointerUp {
            x: 190.0,
            y: 70.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        &mut ch,
    );
    assert_eq!(ch, ["4"]);
    assert_eq!(e.attr("n", "value").as_deref(), Some("4"));
    assert_eq!(e.focus_state().focused_id.as_deref(), Some("n"));
    feed(&mut e, key_ev("Enter", false), &mut ch);
    assert_eq!(ch, ["4"], "Enter does not report the step again");
}

/// Partial repaint after the click equals a forced full repaint.
#[test]
fn spin_click_on_typed_value_partial_equals_full() {
    for (attrs, start, typed, up) in [
        (WHOLE, "2", "3.7", true),
        (WHOLE, "2", "3.7", false),
        (GROUPED, "2", "1234.5", true),
        (FEE, "4.1", "4.105", true),
    ] {
        let mut e = boot(attrs, start, PaintMode::Exact);
        let mut ch = Vec::new();
        type_into(&mut e, typed, &mut ch);
        let _ = e.paint(vp()).unwrap();
        click(&mut e, 140.0, if up { 6.0 } else { 18.0 }, &mut ch);
        let out = e.paint(vp()).unwrap();
        assert!(!out.damage.is_empty(), "click must damage the box");
        assert!(
            !e.last_paint_stats().full_repaint,
            "must be a partial repaint"
        );
        let partial = out.rgba.unwrap_or_else(|| e.last_frame().unwrap().clone());
        e.mark_theme_or_stylesheet_changed();
        let full = e.paint(vp()).unwrap().rgba.expect("full");
        let diff = partial
            .pixels
            .chunks_exact(4)
            .zip(full.pixels.chunks_exact(4))
            .filter(|(p, q)| p != q)
            .count();
        assert_eq!(diff, 0, "partial must equal full: {attrs} {typed} {up}");
    }
}
