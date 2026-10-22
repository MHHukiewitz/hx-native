//! Graph review of #49 round 2: a limits refresh that changes only `step`
//! re-pads the unfocused box, and an arrow never passes a max that sits just
//! below a grid value.
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

fn boot(attrs: &str, value: &str) -> Engine {
    let mut eng = engine();
    eng.declare_actions(&["p.n"]);
    eng.load_html(&format!(
        r#"<!DOCTYPE html><html><head><style>html,body{{margin:0}} input{{display:block;width:300px;height:24px;font-size:14px}}</style></head><body><form id="f"><input id="n" type="number" name="n" value="{value}" {attrs} hx-post="p.n" hx-trigger="change" /><input id="other" type="text" value="x" /></form></body></html>"#
    ))
    .unwrap();
    let _ = eng.paint(Viewport::with_scale(400, 200, 1.0)).unwrap();
    eng
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
fn a_step_only_limits_refresh_repads_the_unfocused_box() {
    for (value, step0, step1, shown) in [
        ("4", "1", 0.01, "4.00"),
        ("4.1", "0.01", 1.0, "4.1"),
        ("4.1", "0.01", 0.001, "4.100"),
    ] {
        let mut eng = boot(&format!(r#"min="0" max="100" step="{step0}""#), value);
        let src = Src {
            versions: HashMap::from([("actions".to_string(), 1)]),
            limits: vec![(
                "p.n".into(),
                "n".into(),
                InputLimits::new(0.0, 100.0, Some(step1)),
            )],
        };
        eng.sync(&src, &mut NoFrags).unwrap();
        let _ = eng.paint(Viewport::with_scale(400, 200, 1.0)).unwrap();
        assert_eq!(eng.attr("n", "value").as_deref(), Some(value));
        assert_eq!(
            eng.test_editor_display("n").as_deref(),
            Some(shown),
            "{value}: step {step0} -> {step1}"
        );
    }
}

#[test]
fn an_arrow_never_passes_a_max_just_below_a_grid_value() {
    for (attrs, from, want) in [
        (r#"min="0" max="4.99999999" step="1""#, "4", "4"),
        (
            r#"min="0" max="99.9999999999" step="0.01""#,
            "99.99",
            "99.99",
        ),
        (r#"min="0.5" max="9.7" step="1""#, "8.5", "9.5"),
        (r#"min="0" max="0.3" step="0.1""#, "0.2", "0.3"),
    ] {
        let mut eng = boot(attrs, from);
        eng.test_set_focus("n");
        for _ in 0..2 {
            let _ = eng.input(InputEvent::Key(KeyEvent {
                key: "ArrowUp".into(),
                code: "ArrowUp".into(),
                mods: Modifiers::default(),
                repeat: false,
                pressed: true,
            }));
        }
        assert_eq!(eng.attr("n", "value").as_deref(), Some(want), "{attrs}");
    }
}
