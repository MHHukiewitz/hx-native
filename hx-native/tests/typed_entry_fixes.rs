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
    // so a pasted '.' is still dropped (one decimal point rule unchanged).
    let html =
        r#"<input id="fee" type="number" name="fee" value="6.12" min="0" max="100" step="0.01" />"#;
    let mut eng = boot(html);
    eng.test_set_focus("fee");
    let _ = eng.input(InputEvent::Paste(".3".into()));
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("6.123"));
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
