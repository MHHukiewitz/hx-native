//! Regressions for typed number entry (issues #17/#19 follow-ups).
//!
//! - Paste over a selection is filtered against the text that survives the
//!   selection, not the old buffer (see #38).
//! - A typed number commit clamps to `[min, max]` but never snaps to `step`
//!   (Designer lock; see #39). Arrows / spin boxes still step.

use hx_native::*;

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(&[]);
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect("load_html fixture");
    let _ = eng.paint(Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    });
    eng
}

fn key(eng: &mut Engine, key: &str, code: &str, ctrl: bool) {
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: code.into(),
        mods: Modifiers {
            ctrl,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }));
}

fn select_all(eng: &mut Engine) {
    key(eng, "a", "KeyA", true);
}

fn enter(eng: &mut Engine) {
    key(eng, "Enter", "Enter", false);
}

#[test]
fn paste_decimal_over_selection_containing_point_keeps_new_point() {
    let html =
        r#"<input id="fee" type="number" name="fee" value="6.12" min="0" max="100" step="0.01" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("fee");
    select_all(&mut eng);
    let _ = eng.input(InputEvent::Paste("3.45".into()));
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("3.45"),
        "pasting 3.45 over a selected 6.12 must keep the pasted '.'"
    );
    enter(&mut eng);
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("3.45"));
}

#[test]
fn paste_decimal_over_partial_selection_with_point_keeps_new_point() {
    // Select only ".12" (Shift+Left x3 from the end) and paste ".5".
    let html =
        r#"<input id="fee" type="number" name="fee" value="6.12" min="0" max="100" step="0.01" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("fee");
    for _ in 0..3 {
        let _ = eng.input(InputEvent::Key(KeyEvent {
            key: "ArrowLeft".into(),
            code: "ArrowLeft".into(),
            mods: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false,
            pressed: true,
        }));
    }
    let _ = eng.input(InputEvent::Paste(".5".into()));
    enter(&mut eng);
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("6.5"));
}

#[test]
fn paste_still_rejects_second_point_outside_selection() {
    // Caret at the end of "6.12", nothing selected: the existing '.' survives,
    // so "6.12.3" is not a number and the whole paste is refused (a paste
    // never drops characters to make a different number).
    let html =
        r#"<input id="fee" type="number" name="fee" value="6.12" min="0" max="100" step="0.01" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("fee");
    let _ = eng.input(InputEvent::Paste(".3".into()));
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("6.12"));
}

#[test]
fn paste_sign_over_full_selection_is_allowed() {
    let html = r#"<input id="d" type="number" name="d" value="5" min="-100" max="100" step="1" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("d");
    select_all(&mut eng);
    let _ = eng.input(InputEvent::Paste("-7".into()));
    enter(&mut eng);
    assert_eq!(eng.attr("d", "value").as_deref(), Some("-7"));
}

#[test]
fn typed_commit_clamps_but_does_not_snap_to_step() {
    let html = r#"<input id="up" type="number" name="upfront" value="150000" min="0" max="1000000" step="1000" data-hx-group="off" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("up");
    select_all(&mut eng);
    let _ = eng.input(InputEvent::Text("151234".into()));
    enter(&mut eng);
    assert_eq!(
        eng.attr("up", "value").as_deref(),
        Some("151234"),
        "typed value must stay off-grid (clamp, don't snap)"
    );

    select_all(&mut eng);
    let _ = eng.input(InputEvent::Text("2000000".into()));
    enter(&mut eng);
    assert_eq!(
        eng.attr("up", "value").as_deref(),
        Some("1000000"),
        "out-of-range typed value still clamps to max"
    );
}

fn paste_all(eng: &mut Engine, id: &str, text: &str) -> String {
    eng.test_set_focus(id);
    select_all(eng);
    let _ = eng.input(InputEvent::Paste(text.into()));
    enter(eng);
    eng.attr(id, "value").unwrap_or_default()
}

#[test]
fn paste_that_is_not_a_number_for_the_box_is_refused_whole() {
    // All or nothing (Graph review of #41): the old value stays, nothing is
    // mangled into a different number.
    let fee =
        r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" />"#;
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="50000000000" step="1000" data-hx-group="off" />"#;
    assert_eq!(
        paste_all(&mut boot(fee), "f", "-5.5"),
        "6.12",
        "min 0: not 5.5"
    );
    assert_eq!(paste_all(&mut boot(fee), "f", "1.2.3"), "6.12", "not 1.23");
    assert_eq!(paste_all(&mut boot(fee), "f", "1e5"), "6.12", "not 15");
    assert_eq!(paste_all(&mut boot(fee), "f", "abc"), "6.12");
    // No data-hx-paste-ignore: nothing is cleaned up, so '$' and outer
    // spaces make the paste invalid (commas are part of the number grammar,
    // exactly as when typed).
    assert_eq!(
        paste_all(&mut boot(up), "u", "$1,234"),
        "150000",
        "no ignore list"
    );
    assert_eq!(
        paste_all(&mut boot(up), "u", " 151234 "),
        "150000",
        "no trimming"
    );
    assert_eq!(paste_all(&mut boot(up), "u", "151,234"), "151234");
}

#[test]
fn whole_number_box_rounds_a_pasted_decimal_on_commit() {
    // Designer call (#41 round 2): a pasted number that parses rounds half
    // away from zero in a whole-number (integer-step) box.
    let term =
        r#"<input id="t" type="number" name="t" value="12" min="-120" max="120" step="1" />"#;
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="50000000000" step="1000" data-hx-group="off" />"#;
    assert_eq!(paste_all(&mut boot(term), "t", "3.7"), "4", "not 37");
    assert_eq!(paste_all(&mut boot(term), "t", "2.5"), "3");
    assert_eq!(paste_all(&mut boot(term), "t", "-2.5"), "-3");
    assert_eq!(
        paste_all(&mut boot(up), "u", "151234.50"),
        "151235",
        "not 15123450"
    );
}

#[test]
fn whole_number_box_accepts_typed_point_and_rounds_on_commit() {
    let term = r#"<input id="t" type="number" name="t" value="12" min="1" max="120" step="1" />"#;
    let mut eng = boot(term);
    eng.test_set_focus("t");
    select_all(&mut eng);
    for ch in ["3", ".", "7"] {
        let _ = eng.input(InputEvent::Text(ch.into()));
    }
    assert_eq!(
        eng.test_editor_display("t").as_deref(),
        Some("3.7"),
        "the '.' key is accepted while typing"
    );
    enter(&mut eng);
    assert_eq!(eng.attr("t", "value").as_deref(), Some("4"), "not 37");
}

#[test]
fn fractional_step_box_does_not_round_on_commit() {
    let fee =
        r#"<input id="f" type="number" name="f" value="6.12" min="0" max="100" step="0.01" />"#;
    assert_eq!(paste_all(&mut boot(fee), "f", "3.7"), "3.7");
}

#[test]
fn paste_ignore_attribute_strips_listed_characters_only() {
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="50000000000" step="1000" data-hx-group="off" data-hx-paste-ignore="$," />"#;
    assert_eq!(paste_all(&mut boot(up), "u", "$1,234"), "1234");
    assert_eq!(paste_all(&mut boot(up), "u", "$151,379,336"), "151379336");
    assert_eq!(
        paste_all(&mut boot(up), "u", "$1,234 USD"),
        "150000",
        "leftover text that is not a number is still refused"
    );
}

#[test]
fn paste_sign_over_a_selected_leading_digit_is_allowed() {
    let html =
        r#"<input id="f" type="number" name="f" value="6.12" min="-100" max="100" step="0.01" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("f");
    key(&mut eng, "Home", "Home", false);
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "ArrowRight".into(),
        code: "ArrowRight".into(),
        mods: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }));
    let _ = eng.input(InputEvent::Paste("-9".into()));
    assert_eq!(eng.test_editor_display("f").as_deref(), Some("-9.12"));
}

#[test]
fn arrows_from_an_off_grid_value_land_on_the_next_grid_value_each_way() {
    let up = r#"<input id="u" type="number" name="u" value="150000" min="0" max="1000000" step="1000" data-hx-group="off" />"#;
    for (start, k, want) in [
        ("151234", "ArrowUp", "152000"),
        ("151234", "ArrowDown", "151000"),
        ("151634", "ArrowUp", "152000"),
        ("151634", "ArrowDown", "151000"),
        ("151000", "ArrowUp", "152000"),
        ("151000", "ArrowDown", "150000"),
    ] {
        let mut eng = boot(up);
        eng.test_set_focus("u");
        select_all(&mut eng);
        let _ = eng.input(InputEvent::Text(start.into()));
        key(&mut eng, k, k, false);
        assert_eq!(eng.attr("u", "value").as_deref(), Some(want), "{start} {k}");
    }
}
