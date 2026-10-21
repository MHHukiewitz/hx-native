//! Issue #48: step / clamp / commit maths must never leave binary float
//! noise in a number's stored `value`, its action args or its display.
//! `1234567.89` typed with `step="0.01"` used to store and show
//! `1234567.8899999999`; every path (typed commit, arrows, the range slider,
//! #16 limits clamping) now rounds to the decimals in play (step, min, the
//! value itself) and formats the shortest decimal that round-trips.

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
    format!("<!DOCTYPE html><html><head><style>html,body{{margin:0}} input{{display:block;width:400px;height:24px;font-size:14px;margin:0;padding:0;border:0}}</style></head><body>{body}</body></html>")
}

fn vp() -> Viewport {
    Viewport::with_scale(500, 200, 1.0)
}

fn boot(body: &str, actions: &[&str]) -> Engine {
    let mut eng = engine();
    eng.declare_actions(actions);
    eng.load_html(&wrap(body)).unwrap();
    let _ = eng.paint(vp()).unwrap();
    eng
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

/// Focus `id`, replace its text with `text`, commit with Enter.
fn type_commit(eng: &mut Engine, id: &str, text: &str) -> InputOutcome {
    eng.test_set_focus(id);
    let _ = key(eng, "a", true);
    let _ = eng.input(InputEvent::Text(text.into()));
    key(eng, "Enter", false)
}

fn arg<'a>(out: &'a InputOutcome, name: &str) -> Option<&'a str> {
    match out {
        InputOutcome::Action { args, .. } => args
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str()),
        _ => None,
    }
}

/// A plain decimal with at most `max_dec` fraction digits (no noise).
fn clean(s: &str, max_dec: usize) -> bool {
    let t = s.trim_start_matches('-');
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    !int.is_empty()
        && int.chars().all(|c| c.is_ascii_digit() || c == ',')
        && frac.chars().all(|c| c.is_ascii_digit())
        && frac.len() <= max_dec
}

const FEE: &str = r#"<form id="f"><input id="fee" type="number" name="fee" value="0" min="0" max="100.00" step="0.01" hx-post="deal.fee" hx-trigger="change" /></form>"#;

#[test]
fn fee_box_typed_4_10_stores_exactly_4_1_and_shows_it_clean() {
    for group in ["", r#" data-hx-group="on""#] {
        let body = FEE.replace(
            " /></form>",
            &format!("{group} /><input id=\"other\" type=\"text\" value=\"x\" /></form>"),
        );
        let mut eng = boot(&body, &["deal.fee"]);
        let out = type_commit(&mut eng, "fee", "4.10");
        let stored = eng.attr("fee", "value").unwrap();
        assert_eq!(
            stored, "4.1",
            "typed commit stores the shortest form{group}"
        );
        assert_eq!(stored.parse::<f64>().unwrap(), 4.1);
        assert_eq!(arg(&out, "fee"), Some("4.1"), "args{group}");
        let _ = key(&mut eng, "Tab", false);
        assert_eq!(eng.test_editor_display("fee").as_deref(), Some("4.1"));
        // Arrows keep the step's precision (current rules): 4.09 → 4.10.
        let _ = type_commit(&mut eng, "fee", "4.09");
        let _ = key(&mut eng, "ArrowUp", false);
        let stepped = eng.attr("fee", "value").unwrap();
        assert_eq!(stepped, "4.10", "arrow step keeps step precision{group}");
        assert_eq!(stepped.parse::<f64>().unwrap(), 4.1);
    }
}

#[test]
fn typed_1234567_89_is_clean_grouped_and_ungrouped() {
    for (group, shown) in [
        ("", "1234567.89"),
        (r#" data-hx-group="on""#, "1,234,567.89"),
    ] {
        let body = format!(
            r#"<form id="f"><input id="amt" type="number" name="amt" value="0" step="0.01" hx-post="deal.amt" hx-trigger="change"{group} /><input id="other" type="text" value="x" /></form>"#
        );
        let mut eng = boot(&body, &["deal.amt"]);
        let out = type_commit(&mut eng, "amt", "1234567.89");
        assert_eq!(
            eng.attr("amt", "value").as_deref(),
            Some("1234567.89"),
            "stored{group}"
        );
        assert_eq!(arg(&out, "amt"), Some("1234567.89"), "args{group}");
        let _ = key(&mut eng, "Tab", false);
        assert_eq!(
            eng.test_editor_display("amt").as_deref(),
            Some(shown),
            "display{group}"
        );
        let _ = eng.paint(vp()).unwrap();
        assert_eq!(eng.test_editor_display("amt").as_deref(), Some(shown));
    }
}

#[test]
fn arrow_steps_from_0_1_by_0_1_three_times_give_0_4() {
    for group in ["", r#" data-hx-group="on""#] {
        let mut eng = boot(
            &format!(
                r#"<input id="s" type="number" name="s" value="0.1" step="0.1"{group} /><input id="other" type="text" value="x" />"#
            ),
            &[],
        );
        eng.test_set_focus("s");
        let mut seen = Vec::new();
        for _ in 0..3 {
            let _ = key(&mut eng, "ArrowUp", false);
            seen.push(eng.attr("s", "value").unwrap());
        }
        assert_eq!(seen, ["0.2", "0.3", "0.4"], "{group}");
        let _ = key(&mut eng, "Tab", false);
        assert_eq!(eng.test_editor_display("s").as_deref(), Some("0.4"));
    }
}

#[test]
fn large_values_step_clean_by_0_01_both_ways() {
    let mut eng = boot(
        r#"<input id="s" type="number" name="s" value="1234567.88" step="0.01" data-hx-group="on" /><input id="other" type="text" value="x" />"#,
        &[],
    );
    eng.test_set_focus("s");
    let _ = key(&mut eng, "ArrowUp", false);
    assert_eq!(eng.attr("s", "value").as_deref(), Some("1234567.89"));
    let _ = key(&mut eng, "ArrowUp", false);
    assert_eq!(eng.attr("s", "value").as_deref(), Some("1234567.90"));
    let _ = key(&mut eng, "ArrowDown", false);
    assert_eq!(eng.attr("s", "value").as_deref(), Some("1234567.89"));
    let _ = key(&mut eng, "Tab", false);
    assert_eq!(
        eng.test_editor_display("s").as_deref(),
        Some("1,234,567.89")
    );
}

#[test]
fn range_slider_values_are_clean_on_a_large_fractional_range() {
    // 400 px wide at x = 0; min/max carry two decimals, step 0.01.
    for x in 1..=60 {
        let mut eng = boot(
            r#"<input id="r" type="range" name="r" min="1000000" max="1234567.89" step="0.01" value="1000000" />"#,
            &[],
        );
        let xf = x as f32 * 6.5;
        let _ = eng.input(InputEvent::PointerDown {
            x: xf,
            y: 12.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        });
        let _ = eng.input(InputEvent::PointerUp {
            x: xf,
            y: 12.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        });
        let v = eng.attr("r", "value").unwrap();
        assert!(clean(&v, 2), "slider at x={xf}: {v}");
    }
    // Dragged past the far end it lands exactly on max.
    let mut eng = boot(
        r#"<input id="r" type="range" name="r" min="1000000" max="1234567.89" step="0.01" value="1000000" />"#,
        &[],
    );
    let _ = eng.input(InputEvent::PointerDown {
        x: 390.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let _ = eng.input(InputEvent::PointerMove {
        x: 1000.0,
        y: 12.0,
        mods: Modifiers::default(),
    });
    assert_eq!(eng.attr("r", "value").as_deref(), Some("1234567.89"));
}

#[test]
fn fee_range_slider_steps_clean_at_every_pixel() {
    let mut eng = boot(
        r#"<input id="r" type="range" name="r" min="0" max="100.00" step="0.01" value="0" />"#,
        &[],
    );
    for x in 0..400 {
        let xf = x as f32 + 0.5;
        let _ = eng.input(InputEvent::PointerDown {
            x: xf,
            y: 12.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        });
        let _ = eng.input(InputEvent::PointerUp {
            x: xf,
            y: 12.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        });
        let v = eng.attr("r", "value").unwrap();
        assert!(clean(&v, 2), "fee slider at x={xf}: {v}");
    }
}

struct Src {
    versions: HashMap<String, u64>,
    limits: Vec<(String, String, InputLimits)>,
}

impl SliceSource for Src {
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

#[test]
fn slice_limits_write_and_clamp_clean_values_grouped_and_ungrouped() {
    for (group, shown) in [
        ("", "1234567.89"),
        (r#" data-hx-group="on""#, "1,234,567.89"),
    ] {
        let body = format!(
            r#"<form id="f"><input id="amt" type="number" name="amt" value="9999999"{group} /><input id="r" type="range" name="r" value="9999999" /><button id="go" hx-post="deal.save">Save</button></form>"#
        );
        let mut eng = boot(&body, &["deal.save"]);
        let src = Src {
            versions: HashMap::from([("actions".into(), 1)]),
            limits: vec![
                (
                    "deal.save".into(),
                    "amt".into(),
                    InputLimits::new(0.01, 1234567.89, Some(0.01)),
                ),
                (
                    "deal.save".into(),
                    "r".into(),
                    InputLimits::new(0.01, 1234567.89, Some(0.01)),
                ),
            ],
        };
        eng.sync(&src, &mut NoFrags).unwrap();
        assert_eq!(
            eng.attr("amt", "max").as_deref(),
            Some("1234567.89"),
            "max attr{group}"
        );
        assert_eq!(eng.attr("amt", "min").as_deref(), Some("0.01"));
        assert_eq!(eng.attr("amt", "step").as_deref(), Some("0.01"));
        assert_eq!(
            eng.attr("amt", "value").as_deref(),
            Some("1234567.89"),
            "clamped{group}"
        );
        assert_eq!(
            eng.attr("r", "value").as_deref(),
            Some("1234567.89"),
            "range clamped{group}"
        );
        let _ = eng.paint(vp()).unwrap();
        assert_eq!(
            eng.test_editor_display("amt").as_deref(),
            Some(shown),
            "display{group}"
        );
    }
}

#[test]
fn paste_always_strips_commas_so_a_comma_in_paste_ignore_is_a_no_op() {
    for ignore in ["$", "$,"] {
        let mut eng = boot(
            &format!(
                r#"<input id="p" type="number" name="p" value="0" data-hx-paste-ignore="{ignore}" /><input id="other" type="text" value="x" />"#
            ),
            &[],
        );
        eng.test_set_focus("p");
        let _ = key(&mut eng, "a", true);
        let _ = eng.input(InputEvent::Paste("$1,234".into()));
        let _ = key(&mut eng, "Tab", false);
        assert_eq!(
            eng.attr("p", "value").as_deref(),
            Some("1234"),
            "ignore={ignore:?}"
        );
    }
}
