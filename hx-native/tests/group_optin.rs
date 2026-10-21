//! Issue #40: thousands grouping on `<input type=number>` is opt-in with
//! `data-hx-group="on"`. The default shows the raw number everywhere. With
//! the opt-in, an unfocused box shows `73,480` the same way after load, a
//! swap and a blur; while focused it shows the raw editable text; the
//! `value` attribute and action args always stay canonical (`73480`).

use std::collections::HashMap;

use hx_native::*;

fn engine() -> Engine {
    Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    })
}

fn wrap(body: &str) -> String {
    format!("<!DOCTYPE html><html><head><style>html,body{{margin:0}} input{{display:block;width:160px;height:24px;font-size:14px}}</style></head><body>{body}</body></html>")
}

fn vp(scale: f64) -> Viewport {
    Viewport::with_scale(
        (400.0 * scale).round() as u32,
        (200.0 * scale).round() as u32,
        scale,
    )
}

fn key(eng: &mut Engine, k: &str, ctrl: bool) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }))
}

struct Slices {
    versions: HashMap<String, u64>,
}

impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
}

struct Frags {
    body: String,
}

impl FragmentSource for Frags {
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        self.body.clone()
    }
}

#[test]
fn default_number_box_shows_raw_on_load_focus_and_blur() {
    let mut eng = engine();
    eng.declare_actions(&[]);
    eng.load_html(&wrap(
        r#"<input id="fee" type="number" name="fee" value="73480" /><input id="other" type="text" value="x" />"#,
    ))
    .unwrap();
    let _ = eng.paint(vp(1.0)).unwrap();
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("73480"));

    eng.test_set_focus("fee");
    key(&mut eng, "a", true);
    let _ = eng.input(InputEvent::Text("1234567".into()));
    let _ = key(&mut eng, "Tab", false);
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("1234567"));
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("1234567"),
        "without data-hx-group=\"on\" a blurred box stays raw"
    );
}

#[test]
fn opt_in_groups_on_load_raw_while_focused_grouped_after_blur() {
    let mut eng = engine();
    eng.declare_actions(&["deal.save"]);
    eng.load_html(&wrap(
        r#"<form id="f"><input id="fee" type="number" name="fee" value="73480" data-hx-group="on" /><input id="other" type="text" value="x" /><button id="go" hx-post="deal.save">Go</button></form>"#,
    ))
    .unwrap();
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("73,480"),
        "opt-in box is grouped straight after load_html"
    );
    let _ = eng.paint(vp(1.0)).unwrap();
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("73,480"));
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("73480"));

    eng.test_set_focus("fee");
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("73480"),
        "raw text while focused"
    );
    key(&mut eng, "a", true);
    let _ = eng.input(InputEvent::Text("1234567".into()));
    let _ = key(&mut eng, "Tab", false);
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("1234567"));
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("1,234,567"),
        "grouped again after blur"
    );

    eng.test_set_focus("go");
    let out = key(&mut eng, "Enter", false);
    match out {
        InputOutcome::Action { args, .. } => assert!(
            args.iter().any(|(n, v)| n == "fee" && v == "1234567"),
            "args stay canonical, got {args:?}"
        ),
        other => panic!("expected Action, got {other:?}"),
    }
}

fn swap_page() -> String {
    wrap(
        r##"<section id="deal" hx-trigger="slice:deal" hx-target="#deal" hx-swap="innerHTML"><p>loading</p></section>"##,
    )
}

const SWAPPED: &str = r#"<input id="fee" type="number" name="fee" value="73480" data-hx-group="on" /><input id="raw" type="number" name="raw" value="73480" />"#;

fn swapped_engine(scale: f64, paint_before: bool) -> (Engine, PaintOutput) {
    let mut eng = engine();
    eng.declare_actions(&[]);
    eng.load_html(&swap_page()).unwrap();
    let mut slices = Slices {
        versions: HashMap::from([("deal".to_string(), 1)]),
    };
    let mut frags = Frags {
        body: "<p>loading</p>".into(),
    };
    if paint_before {
        eng.sync(&slices, &mut frags).unwrap();
        let _ = eng.paint(vp(scale)).unwrap();
        slices.versions.insert("deal".into(), 2);
    }
    frags.body = SWAPPED.into();
    eng.sync(&slices, &mut frags).unwrap();
    if !paint_before {
        eng.mark_theme_or_stylesheet_changed();
    }
    let out = eng.paint(vp(scale)).unwrap();
    (eng, out)
}

#[test]
fn opt_in_groups_after_swap_and_default_stays_raw() {
    let (mut eng, _) = swapped_engine(1.0, true);
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("73,480"));
    assert_eq!(eng.test_editor_display("raw").as_deref(), Some("73480"));
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("73480"));
}

fn assert_swap_partial_matches_full(scale: f64) {
    let (_, partial) = swapped_engine(scale, true);
    let (_, full) = swapped_engine(scale, false);
    assert!(!partial.skipped);
    let a = partial.rgba.as_ref().unwrap();
    let b = full.rgba.as_ref().unwrap();
    let diffs = a
        .pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(x, y)| x != y)
        .count();
    assert_eq!(
        diffs, 0,
        "grouped swap partial must equal full repaint at scale={scale}"
    );
}

#[test]
fn opt_in_swap_partial_matches_full_scale_1() {
    assert_swap_partial_matches_full(1.0);
}

#[test]
fn opt_in_swap_partial_matches_full_scale_2() {
    assert_swap_partial_matches_full(2.0);
}
