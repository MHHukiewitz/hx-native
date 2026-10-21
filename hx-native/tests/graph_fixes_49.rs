//! Graph review of #49: arrow edge cases next to the #48 rounding.
use hx_native::*;

fn engine() -> Engine {
    Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    })
}

fn boot(attrs: &str, value: &str) -> Engine {
    let mut eng = engine();
    eng.declare_actions(&["p.n"]);
    eng.load_html(&format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0}} input{{display:block;width:400px;height:24px;font-size:14px;margin:0;padding:0;border:0}}</style></head><body><form id="f"><input id="n" type="number" name="n" value="{value}" {attrs} hx-post="p.n" hx-trigger="change" /><input id="other" type="text" value="x" /></form></body></html>"#
    ))
    .unwrap();
    let _ = eng.paint(Viewport::with_scale(500, 200, 1.0)).unwrap();
    eng.test_set_focus("n");
    eng
}

fn up(eng: &mut Engine, k: &str) -> (Option<String>, Option<String>) {
    let out = eng.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    let arg = match out {
        InputOutcome::Action { args, .. } => {
            args.into_iter().find(|(k, _)| k == "n").map(|(_, v)| v)
        }
        _ => None,
    };
    (eng.attr("n", "value"), arg)
}

#[test]
fn step_any_keeps_the_values_decimals_with_a_decimal_min() {
    let mut eng = boot(r#"min="0.5" step="any""#, "0.75");
    assert_eq!(
        up(&mut eng, "ArrowUp"),
        (Some("1.75".into()), Some("1.75".into()))
    );
    let mut eng = boot(r#"min="0.01" step="any""#, "3.14159");
    assert_eq!(up(&mut eng, "ArrowUp").0.as_deref(), Some("4.14159"));
}

#[test]
fn arrow_to_zero_never_writes_negative_zero() {
    // -0.9 + 3 × 0.3 rounds to -0.0 in f64.
    let mut eng = boot(r#"min="-0.9" step="0.3""#, "-0.3");
    // Designer lock (#49 round 2): stored and posted as the plain `0`; the
    // box shows the step's decimals, `0.0`.
    assert_eq!(
        up(&mut eng, "ArrowUp"),
        (Some("0".into()), Some("0".into()))
    );
    assert_eq!(eng.test_editor_display("n").as_deref(), Some("0.0"));
    for (min, step) in [
        ("-1.8", "0.3"),
        ("-6.9", "0.3"),
        ("-0.7", "0.1"),
        ("-0.21", "0.07"),
    ] {
        let mut eng = boot(
            &format!(r#"min="{min}" step="{step}""#),
            &format!("-{step}"),
        );
        let v = up(&mut eng, "ArrowUp").0.unwrap();
        assert!(!v.starts_with('-'), "min {min} step {step}: {v}");
    }
}
