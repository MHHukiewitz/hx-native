//! Issue #121: a mouse wheel over an open `<select>` list scrolls the list
//! (or is swallowed when everything fits) and never closes it. A wheel
//! elsewhere still closes the list. Lists longer than 8 rows scroll with the
//! wheel and the arrow keys, and a row picked after scrolling is the option
//! under the pointer, not the one in the same slot of the first page.

use hx_native::*;

fn many_options(n: usize) -> String {
    (0..n)
        .map(|i| {
            let sel = if i == 0 { r#" selected="""# } else { "" };
            format!(r#"<option value="v{i}"{sel}>Option {i}</option>"#)
        })
        .collect()
}

fn html(n: usize) -> String {
    format!(
        r#"
<select id="qty" name="qty" hx-post="set.qty" hx-trigger="change"
        style="position:absolute;left:8px;top:8px;width:128px;height:22px">{}</select>
<div id="log" style="position:absolute;left:200px;top:40px;width:150px;height:100px;overflow-y:auto">{}</div>
"#,
        many_options(n),
        (0..30)
            .map(|i| format!(r#"<div style="height:20px">row {i}</div>"#))
            .collect::<String>()
    )
}

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 300,
        scale: 1.0,
        holes: vec![],
    }
}

fn boot(n: usize) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(&["set.qty"]);
    let wrapped = format!("<!DOCTYPE html><html><body>{}</body></html>", html(n));
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(vp()).expect("first paint");
    eng
}

fn key(eng: &mut Engine, key: &str, mods: Modifiers) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods,
        repeat: false,
        pressed: true,
    }))
}

fn open_list(eng: &mut Engine) {
    eng.test_set_focus("qty");
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    let _ = key(eng, "ArrowDown", alt);
    assert!(eng.test_open_select().is_some(), "list must open");
}

fn click(eng: &mut Engine, x: f32, y: f32) -> InputOutcome {
    let _ = eng.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    eng.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}

/// Wheel `notches` lines over the list (positive scrolls the list down).
fn wheel_lines(eng: &mut Engine, x: f32, y: f32, notches: f32) -> InputOutcome {
    eng.input(InputEvent::Wheel {
        pos: (x, y),
        delta: (0.0, -notches),
        unit: WheelUnit::Line,
    })
}

/// A point inside the open list's first visible row.
fn first_row_point(eng: &Engine) -> (f32, f32) {
    let (x, y, _, _) = eng.test_open_select_rect().expect("open");
    (x + 20.0, y + 1.0 + 9.0)
}

fn change_value(out: &InputOutcome) -> Option<String> {
    match out {
        InputOutcome::Action {
            trigger: TriggerInfo::Change,
            args,
            ..
        } => args
            .iter()
            .find(|(k, _)| k == "qty")
            .map(|(_, v)| v.clone()),
        _ => None,
    }
}

#[test]
fn wheel_over_open_list_scrolls_it_and_keeps_it_open() {
    let mut eng = boot(20);
    open_list(&mut eng);
    assert_eq!(eng.test_open_select().unwrap().0, 0);
    let (x, y) = first_row_point(&eng);

    // One line = 3 rows. Not closed, scrolled, reported as consumed.
    let out = wheel_lines(&mut eng, x, y, 1.0);
    assert!(
        matches!(out, InputOutcome::Scrolled { id: Some(ref id) } if id == "qty"),
        "wheel over the list is consumed, got {out:?}"
    );
    assert_eq!(eng.test_open_select().expect("still open").0, 3);

    // Back up one line.
    let _ = wheel_lines(&mut eng, x, y, -1.0);
    assert_eq!(eng.test_open_select().expect("still open").0, 0);

    // Down past the end: clamps at 20 - 8 = 12, still open, still consumed.
    for _ in 0..10 {
        let out = wheel_lines(&mut eng, x, y, 1.0);
        assert!(matches!(out, InputOutcome::Scrolled { .. }), "{out:?}");
    }
    assert_eq!(eng.test_open_select().expect("still open").0, 12);

    // Up past the start clamps at 0.
    for _ in 0..10 {
        let _ = wheel_lines(&mut eng, x, y, -1.0);
    }
    assert_eq!(eng.test_open_select().expect("still open").0, 0);
}

#[test]
fn wheel_pixels_accumulate_and_page_moves_a_window() {
    let mut eng = boot(30);
    open_list(&mut eng);
    let (x, y) = first_row_point(&eng);

    // Trackpad: 9 px is half an 18 px row; the fraction carries to the next.
    let half_row = |eng: &mut Engine| {
        let _ = eng.input(InputEvent::Wheel {
            pos: (x, y),
            delta: (0.0, -9.0),
            unit: WheelUnit::Pixel,
        });
    };
    half_row(&mut eng);
    assert_eq!(eng.test_open_select().expect("open").0, 0);
    half_row(&mut eng);
    assert_eq!(eng.test_open_select().expect("open").0, 1);

    // A page is the visible rows less one.
    let _ = eng.input(InputEvent::Wheel {
        pos: (x, y),
        delta: (0.0, -1.0),
        unit: WheelUnit::Page,
    });
    assert_eq!(eng.test_open_select().expect("open").0, 1 + 7);
}

#[test]
fn row_picked_after_scrolling_is_the_option_under_the_pointer() {
    let mut eng = boot(20);
    open_list(&mut eng);
    let (x, y) = first_row_point(&eng);
    let _ = wheel_lines(&mut eng, x, y, 2.0); // top = 6
    assert_eq!(eng.test_open_select().unwrap().0, 6);
    let out = click(&mut eng, x, y);
    assert_eq!(change_value(&out).as_deref(), Some("v6"), "{out:?}");
    assert!(eng.test_open_select().is_none(), "a pick closes the list");
}

#[test]
fn short_list_swallows_the_wheel_and_stays_open() {
    let mut eng = boot(3);
    open_list(&mut eng);
    let (x, y) = first_row_point(&eng);
    let out = wheel_lines(&mut eng, x, y, 1.0);
    assert!(
        matches!(out, InputOutcome::Scrolled { id: Some(ref id) } if id == "qty"),
        "swallowed, not handed to the host: {out:?}"
    );
    assert_eq!(eng.test_open_select().expect("still open").0, 0);
}

#[test]
fn wheel_outside_the_list_still_closes_it() {
    // Over nothing: closes, nothing moves.
    let mut eng = boot(20);
    open_list(&mut eng);
    let out = wheel_lines(&mut eng, 380.0, 280.0, 1.0);
    assert!(matches!(out, InputOutcome::None), "{out:?}");
    assert!(eng.test_open_select().is_none(), "outside wheel closes");

    // Over a scroll container: closes the list and scrolls the container.
    let mut eng = boot(20);
    open_list(&mut eng);
    let out = wheel_lines(&mut eng, 250.0, 80.0, 1.0);
    assert!(
        matches!(out, InputOutcome::Scrolled { id: Some(ref id) } if id == "log"),
        "{out:?}"
    );
    assert!(eng.test_open_select().is_none(), "outside wheel closes");
    assert_eq!(eng.test_scroll_top("log").unwrap().0, 60.0);
}

#[test]
fn closed_select_wheel_is_unchanged() {
    let mut eng = boot(20);
    let out = wheel_lines(&mut eng, 20.0, 20.0, 1.0);
    assert!(matches!(out, InputOutcome::None), "{out:?}");
    assert!(eng.test_open_select().is_none());
}

#[test]
fn arrow_keys_scroll_the_list_and_enter_picks_past_row_eight() {
    let mut eng = boot(20);
    open_list(&mut eng);
    for _ in 0..9 {
        let _ = key(&mut eng, "ArrowDown", Modifiers::default());
    }
    let (top, hl) = eng.test_open_select().expect("open");
    assert_eq!(
        (top, hl),
        (2, 9),
        "highlight 9 scrolls the window to show it"
    );
    let out = key(&mut eng, "Enter", Modifiers::default());
    assert_eq!(change_value(&out).as_deref(), Some("v9"), "{out:?}");

    // Re-opening on option 9 shows it.
    if let InputOutcome::Action { request, .. } = &out {
        eng.complete(*request, Ok(()));
    }
    open_list(&mut eng);
    let (top, hl) = eng.test_open_select().expect("open");
    assert_eq!((top, hl), (2, 9));
}

#[test]
fn scrolled_list_paints_partial_equal_to_full() {
    let run = |eng: &mut Engine| {
        open_list(eng);
        let _ = eng.paint(vp()).unwrap();
        let (x, y) = first_row_point(eng);
        let _ = wheel_lines(eng, x, y, 1.0);
    };
    let mut eng = boot(20);
    run(&mut eng);
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped && partial.rgba.is_some());
    assert!(
        !eng.last_paint_stats().full_repaint,
        "stats={:?}",
        eng.last_paint_stats()
    );
    let partial_px = partial.rgba.unwrap();

    let mut full_eng = boot(20);
    run(&mut full_eng);
    full_eng.mark_theme_or_stylesheet_changed();
    let full = full_eng.paint(vp()).unwrap();
    assert_eq!(
        partial_px.pixels,
        full.rgba.unwrap().pixels,
        "partial vs full differ"
    );
}
