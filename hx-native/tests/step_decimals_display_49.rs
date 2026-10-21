//! #49 round 2, Designer lock (Floor, 2026-10-21 17:38; Designer's
//! HTML_CONTRACT): a number box always shows the step's decimal places.
//! The fee box (`step="0.01"`) shows `4.10` after a typed commit, an arrow,
//! a blur, a load or a swap, never `4.1`; typing `100` or `150` shows
//! `100.00`. The `value` attribute and the posted args stay the plain
//! number (`4.1`). `step="any"` keeps the value's own decimals, grouping
//! still applies (`1,234,567.89`), and a steady re-render of the same value
//! shows the same text with no extra paint.

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
    format!("<!DOCTYPE html><html><head><style>html,body{{margin:0}} input{{display:block;width:200px;height:24px;font-size:14px}}</style></head><body>{body}</body></html>")
}

fn vp() -> Viewport {
    Viewport::with_scale(400, 200, 1.0)
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

fn arg(out: &InputOutcome, name: &str) -> Option<String> {
    match out {
        InputOutcome::Action { args, .. } => {
            args.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
        }
        _ => None,
    }
}

fn fee(value: &str, extra: &str) -> String {
    format!(
        r#"<form id="f"><input id="fee" type="number" name="fee" value="{value}" min="0" max="100.00" step="0.01" {extra} hx-post="deal.fee" hx-trigger="change" /><input id="other" type="text" value="x" /></form>"#
    )
}

fn boot(body: &str) -> Engine {
    let mut eng = engine();
    eng.declare_actions(&["deal.fee"]);
    eng.load_html(&wrap(body)).unwrap();
    let _ = eng.paint(vp()).unwrap();
    eng
}

fn show(eng: &mut Engine) -> Option<String> {
    eng.test_editor_display("fee")
}

fn val(eng: &Engine) -> Option<String> {
    eng.attr("fee", "value")
}

/// Typed commits (Enter while focused, then blur), grouped and not.
#[test]
fn typed_commit_shows_the_steps_decimals_and_posts_the_plain_number() {
    for extra in ["", r#"data-hx-group="on""#] {
        for (typed, stored, shown) in [
            ("4.10", "4.1", "4.10"),
            ("4.1", "4.1", "4.10"),
            ("100", "100", "100.00"),
            ("150", "100", "100.00"),
            ("7", "7", "7.00"),
            ("0", "0", "0.00"),
        ] {
            let mut eng = boot(&fee("1", extra));
            eng.test_set_focus("fee");
            let _ = key(&mut eng, "a", true);
            let _ = eng.input(InputEvent::Text(typed.into()));
            let out = key(&mut eng, "Enter", false);
            assert_eq!(val(&eng).as_deref(), Some(stored), "{typed} {extra}");
            assert_eq!(arg(&out, "fee").as_deref(), Some(stored), "{typed} {extra}");
            assert_eq!(
                show(&mut eng).as_deref(),
                Some(shown),
                "focused after Enter: {typed} {extra}"
            );
            let _ = key(&mut eng, "Tab", false);
            assert_eq!(
                show(&mut eng).as_deref(),
                Some(shown),
                "after blur: {typed} {extra}"
            );
            assert_eq!(val(&eng).as_deref(), Some(stored));
        }
    }
}

/// Arrow keys: 4.09 ↑ stores 4.1, shows 4.10 (focused and after blur).
#[test]
fn arrow_shows_the_steps_decimals_and_posts_the_plain_number() {
    for extra in ["", r#"data-hx-group="on""#] {
        let mut eng = boot(&fee("4.09", extra));
        eng.test_set_focus("fee");
        assert_eq!(show(&mut eng).as_deref(), Some("4.09"));
        let out = key(&mut eng, "ArrowUp", false);
        assert_eq!(val(&eng).as_deref(), Some("4.1"), "{extra}");
        assert_eq!(arg(&out, "fee").as_deref(), Some("4.1"), "{extra}");
        assert_eq!(show(&mut eng).as_deref(), Some("4.10"), "focused {extra}");
        // The next digit appends to what is shown, as in any box.
        let _ = key(&mut eng, "Tab", false);
        assert_eq!(show(&mut eng).as_deref(), Some("4.10"), "blur {extra}");
        // Up to the max: 100, shown 100.00.
        let mut eng = boot(&fee("99.99", extra));
        eng.test_set_focus("fee");
        let _ = key(&mut eng, "ArrowUp", false);
        assert_eq!(val(&eng).as_deref(), Some("100"));
        assert_eq!(show(&mut eng).as_deref(), Some("100.00"));
    }
}

/// Focus shows the padded text too, so focusing never changes the text.
#[test]
fn focus_does_not_change_the_text() {
    let mut eng = boot(&fee("4.1", ""));
    assert_eq!(show(&mut eng).as_deref(), Some("4.10"));
    eng.test_set_focus("fee");
    assert_eq!(show(&mut eng).as_deref(), Some("4.10"));
    assert_eq!(val(&eng).as_deref(), Some("4.1"));
}

/// Grouping still applies on top: 1234567.89 → 1,234,567.89; 1234567.8 →
/// 1,234,567.80 (raw 1234567.80 while focused).
#[test]
fn grouping_applies_on_top_of_the_steps_decimals() {
    let body = r#"<input id="fee" type="number" name="fee" value="1234567.89" step="0.01" data-hx-group="on" /><input id="b" type="number" name="b" value="1234567.8" step="0.01" data-hx-group="on" /><input id="other" type="text" value="x" />"#;
    let mut eng = boot(body);
    assert_eq!(show(&mut eng).as_deref(), Some("1,234,567.89"));
    assert_eq!(
        eng.test_editor_display("b").as_deref(),
        Some("1,234,567.80")
    );
    assert_eq!(eng.attr("b", "value").as_deref(), Some("1234567.8"));
    eng.test_set_focus("b");
    assert_eq!(eng.test_editor_display("b").as_deref(), Some("1234567.80"));
    let _ = key(&mut eng, "Tab", false);
    assert_eq!(
        eng.test_editor_display("b").as_deref(),
        Some("1,234,567.80")
    );
}

/// `step="any"` has no grid: the value's own decimals, as written.
#[test]
fn step_any_keeps_the_values_own_decimals() {
    let body = r#"<input id="fee" type="number" name="fee" value="4.1" step="any" /><input id="m" type="number" name="m" value="0.75" min="0.50" step="any" /><input id="w" type="number" name="w" value="3" /><input id="other" type="text" value="x" />"#;
    let mut eng = boot(body);
    assert_eq!(show(&mut eng).as_deref(), Some("4.1"));
    assert_eq!(eng.test_editor_display("m").as_deref(), Some("0.75"));
    assert_eq!(
        eng.test_editor_display("w").as_deref(),
        Some("3"),
        "whole-number box"
    );
    eng.test_set_focus("m");
    let _ = key(&mut eng, "ArrowUp", false);
    assert_eq!(eng.attr("m", "value").as_deref(), Some("1.75"));
    assert_eq!(eng.test_editor_display("m").as_deref(), Some("1.75"));
}

// ── load / swap parity and steady-state re-render ─────────────────────

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

const SECTION: &str = r##"<section id="deal" hx-trigger="slice:deal" hx-target="#deal" hx-swap="innerHTML">BODY</section>"##;

fn frag(value: &str) -> String {
    format!(
        r#"<input id="fee" type="number" name="fee" value="{value}" min="0" max="100.00" step="0.01" /><input id="g" type="number" name="g" value="1234567.8" step="0.01" data-hx-group="on" />"#
    )
}

/// The same box shows the same text after `load_html` and after a swap.
#[test]
fn load_and_swap_show_the_same_text() {
    for value in ["4.1", "4.10", "100", "0"] {
        let mut loaded = engine();
        loaded.declare_actions(&[]);
        loaded
            .load_html(&wrap(&SECTION.replace("BODY", &frag(value))))
            .unwrap();
        let _ = loaded.paint(vp()).unwrap();

        let mut swapped = engine();
        swapped.declare_actions(&[]);
        swapped
            .load_html(&wrap(&SECTION.replace("BODY", "<p>loading</p>")))
            .unwrap();
        let mut slices = Slices {
            versions: HashMap::from([("deal".to_string(), 1)]),
        };
        let mut frags = Frags {
            body: "<p>loading</p>".into(),
        };
        swapped.sync(&slices, &mut frags).unwrap();
        let _ = swapped.paint(vp()).unwrap();
        slices.versions.insert("deal".into(), 2);
        frags.body = frag(value);
        swapped.sync(&slices, &mut frags).unwrap();
        let out = swapped.paint(vp()).unwrap();
        assert!(!out.skipped);

        for id in ["fee", "g"] {
            assert_eq!(
                loaded.test_editor_display(id),
                swapped.test_editor_display(id),
                "{id} value={value}"
            );
        }
        let want = match value {
            "100" => "100.00",
            "0" => "0.00",
            _ => "4.10",
        };
        assert_eq!(swapped.test_editor_display("fee").as_deref(), Some(want));
        assert_eq!(
            swapped.test_editor_display("g").as_deref(),
            Some("1,234,567.80")
        );
        // The host's markup value is left as written.
        assert_eq!(swapped.attr("fee", "value").as_deref(), Some(value));
        // The display write is folded into the swap frame: the next paint is idle.
        assert!(swapped.paint(vp()).unwrap().skipped, "value={value}");
        // A full repaint of the swapped page matches the load.
        loaded.mark_theme_or_stylesheet_changed();
        swapped.mark_theme_or_stylesheet_changed();
        let a = loaded.paint(vp()).unwrap().rgba.unwrap();
        let b = swapped.paint(vp()).unwrap().rgba.unwrap();
        assert!(a.pixels == b.pixels, "load vs swap pixels, value={value}");
    }
}

/// A steady-state re-render (same slice version again, then the same
/// markup swapped in again) keeps the same value and text; the unchanged
/// sync paints nothing, and the re-swap draws the same pixels.
#[test]
fn steady_state_re_render_does_not_fight_the_display() {
    let mut eng = engine();
    eng.declare_actions(&[]);
    eng.load_html(&wrap(&SECTION.replace("BODY", "<p>loading</p>")))
        .unwrap();
    let mut slices = Slices {
        versions: HashMap::from([("deal".to_string(), 1)]),
    };
    let mut frags = Frags { body: frag("4.1") };
    eng.sync(&slices, &mut frags).unwrap();
    let _ = eng.paint(vp()).unwrap();
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("4.10"));
    eng.mark_theme_or_stylesheet_changed();
    let before = eng.paint(vp()).unwrap().rgba.unwrap();

    // Same version: nothing to do, nothing painted.
    eng.sync(&slices, &mut frags).unwrap();
    assert!(
        eng.paint(vp()).unwrap().skipped,
        "unchanged sync stays idle"
    );
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("4.10"));

    // A new version that re-renders the same value: same text, same pixels,
    // and no follow-up paint.
    slices.versions.insert("deal".into(), 2);
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng.paint(vp()).unwrap();
    assert_eq!(eng.test_editor_display("fee").as_deref(), Some("4.10"));
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("4.1"));
    if let Some(after) = out.rgba {
        assert!(after.pixels == before.pixels, "re-render changes no pixel");
    }
    assert!(eng.paint(vp()).unwrap().skipped, "no follow-up paint");
}

/// Only `step` sets the places: with `min="0.005" step="0.01"` a typed `1`
/// shows `1.00` (not `1.000`), and the grid value `0.015` shows as is.
#[test]
fn only_the_step_sets_the_places() {
    let body = r#"<input id="fee" type="number" name="fee" value="0.015" min="0.005" step="0.01" /><input id="other" type="text" value="x" />"#;
    let mut eng = boot(body);
    assert_eq!(show(&mut eng).as_deref(), Some("0.015"));
    eng.test_set_focus("fee");
    let _ = key(&mut eng, "a", true);
    let _ = eng.input(InputEvent::Text("1".into()));
    let _ = key(&mut eng, "Tab", false);
    assert_eq!(val(&eng).as_deref(), Some("1"));
    assert_eq!(show(&mut eng).as_deref(), Some("1.00"));
}

struct LimitsSrc {
    versions: HashMap<String, u64>,
    limits: Vec<(String, String, InputLimits)>,
}

impl SliceSource for LimitsSrc {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.limits.clone()
    }
}

struct NoFrags;
impl FragmentSource for NoFrags {
    fn render(&mut self, _section_id: &str, _slices: &[&str]) -> String {
        String::new()
    }
}

/// A limits clamp of an unfocused box shows the step's decimals too
/// (`150` clamped to max 100 shows `100.00`), grouped or not.
#[test]
fn limits_clamp_shows_the_steps_decimals() {
    for (extra, shown) in [("", "100.00"), (r#"data-hx-group="on""#, "100.00")] {
        let body = format!(
            r#"<form id="f"><input id="fee" type="number" name="fee" value="150" {extra} /><button id="go" hx-post="deal.fee">Save</button></form>"#
        );
        let mut eng = boot(&body);
        let src = LimitsSrc {
            versions: HashMap::from([("actions".into(), 1)]),
            limits: vec![(
                "deal.fee".into(),
                "fee".into(),
                InputLimits::new(0.0, 100.0, Some(0.01)),
            )],
        };
        eng.sync(&src, &mut NoFrags).unwrap();
        let _ = eng.paint(vp()).unwrap();
        assert_eq!(val(&eng).as_deref(), Some("100"), "{extra}");
        assert_eq!(show(&mut eng).as_deref(), Some(shown), "{extra}");
    }
}
