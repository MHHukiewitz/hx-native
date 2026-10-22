//! Issue #101: a number box with no max (`InputLimits::at_least`).
//! Mainnet Services' Marketing budget has no max by game rule. With
//! `max: None` the engine writes no `max` attribute (and clears one from
//! markup: the slice wins), ▲ / ArrowUp step up with no clamp, and typed or
//! pasted values clamp only at `min`. A range slider still needs a max: it
//! keeps the markup `max`, or HTML's default of 100. `InputLimits::new`
//! behaves exactly as before.

use std::collections::HashMap;

use hx_native::*;

fn vp() -> Viewport {
    Viewport::with_scale(400, 200, 1.0)
}

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    eng.declare_actions(&["services.apply"]);
    let wrapped = format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0}}\
         #budget{{position:absolute;left:0;top:0;width:160px;height:24px}}\
         #up{{position:absolute;left:170px;top:0;width:24px;height:12px}}\
         #vol{{position:absolute;left:0;top:40px;width:160px;height:20px}}</style></head>\
         <body>{html}</body></html>"
    );
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(vp());
    eng
}

struct Slices {
    version: u64,
    limits: Vec<(String, String, InputLimits)>,
}

impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        (name == "actions").then_some(self.version)
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.limits.clone()
    }
}

#[derive(Default)]
struct Frags(HashMap<String, String>);
impl FragmentSource for Frags {
    fn render(&mut self, id: &str, _slices: &[&str]) -> String {
        self.0.get(id).cloned().unwrap_or_default()
    }
}

fn set(
    eng: &mut Engine,
    version: u64,
    input: &str,
    limits: InputLimits,
) -> Result<(), EngineError> {
    let s = Slices {
        version,
        limits: vec![("services.apply".into(), input.into(), limits)],
    };
    eng.sync(&s, &mut Frags::default()).map(|_| ())
}

fn key(eng: &mut Engine, k: &str) {
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: false,
    }));
    let _ = eng.paint(vp());
}

fn click(eng: &mut Engine, x: f32, y: f32) {
    let mods = Modifiers::default();
    let button = PointerButton::Primary;
    let _ = eng.input(InputEvent::PointerMove { x, y, mods });
    let _ = eng.input(InputEvent::PointerDown { x, y, button, mods });
    let _ = eng.input(InputEvent::PointerUp { x, y, button, mods });
    let _ = eng.paint(vp());
}

/// Select all of the box and type or paste `s`, then press Enter.
fn enter(eng: &mut Engine, s: &str, paste: bool) {
    eng.test_set_focus("budget");
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }));
    if paste {
        let _ = eng.input(InputEvent::Paste(s.into()));
    } else {
        for c in s.chars() {
            let _ = eng.input(InputEvent::Text(c.to_string()));
        }
    }
    key(eng, "Enter");
}

fn budget(value: &str, extra: &str) -> String {
    format!(
        "<form id=\"f\"><input id=\"budget\" type=\"number\" name=\"budget\" value=\"{value}\"{extra} />\
         <span id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"budget\">up</span>\
         <button id=\"save\" hx-post=\"services.apply\">Apply</button></form>"
    )
}

fn val(eng: &Engine, id: &str) -> Option<String> {
    eng.attr(id, "value")
}

#[test]
fn at_least_sets_no_max() {
    let l = InputLimits::at_least(0.0, Some(1.0));
    assert_eq!(l.min, 0.0);
    assert_eq!(l.max, None);
    assert_eq!(l.step, Some(1.0));
    assert_eq!(InputLimits::new(0.0, 5.0, None).max, Some(5.0));
}

#[test]
fn at_least_number_box_writes_no_max_and_clears_the_markup_max() {
    let mut eng = boot(&budget("7", " min=\"1\" max=\"10\""));
    set(&mut eng, 1, "budget", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("budget", "min").as_deref(), Some("0"));
    assert_eq!(eng.attr("budget", "step").as_deref(), Some("1"));
    assert_eq!(eng.attr("budget", "max"), None, "no max attribute");
    assert_eq!(val(&eng, "budget").as_deref(), Some("7"));
}

#[test]
fn at_least_keeps_a_large_value_when_applied() {
    let mut eng = boot(&budget("5000000", " max=\"10\""));
    set(&mut eng, 1, "budget", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(
        val(&eng, "budget").as_deref(),
        Some("5000000"),
        "no clamp at a max"
    );
}

#[test]
fn at_least_arrow_and_spin_step_up_past_a_large_value() {
    let mut eng = boot(&budget("1000000", ""));
    set(&mut eng, 1, "budget", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    click(&mut eng, 182.0, 6.0); // ▲
    assert_eq!(val(&eng, "budget").as_deref(), Some("1000001"));
    eng.test_set_focus("budget");
    key(&mut eng, "ArrowUp");
    assert_eq!(val(&eng, "budget").as_deref(), Some("1000002"));
    assert_eq!(eng.attr("budget", "max"), None);
}

#[test]
fn at_least_typed_or_pasted_huge_value_stays() {
    let mut eng = boot(&budget("1", " max=\"10\""));
    set(&mut eng, 1, "budget", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    enter(&mut eng, "123456789012", false);
    assert_eq!(
        val(&eng, "budget").as_deref(),
        Some("123456789012"),
        "typed"
    );
    enter(&mut eng, "98765432100", true);
    assert_eq!(
        val(&eng, "budget").as_deref(),
        Some("98765432100"),
        "pasted"
    );
}

#[test]
fn at_least_typed_or_pasted_value_below_min_clamps_to_min() {
    let mut eng = boot(&budget("9", ""));
    set(&mut eng, 1, "budget", InputLimits::at_least(5.0, Some(1.0))).unwrap();
    enter(&mut eng, "2", false);
    assert_eq!(val(&eng, "budget").as_deref(), Some("5"), "typed");
    enter(&mut eng, "9", false);
    assert_eq!(val(&eng, "budget").as_deref(), Some("9"));
    enter(&mut eng, "3", true);
    assert_eq!(val(&eng, "budget").as_deref(), Some("5"), "pasted");
}

#[test]
fn at_least_applied_below_min_clamps_to_min() {
    let mut eng = boot(&budget("2", ""));
    set(&mut eng, 1, "budget", InputLimits::at_least(5.0, Some(1.0))).unwrap();
    assert_eq!(val(&eng, "budget").as_deref(), Some("5"));
}

#[test]
fn new_behaves_as_before() {
    let mut eng = boot(&budget("4", " max=\"50\""));
    set(&mut eng, 1, "budget", InputLimits::new(0.0, 5.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("budget", "max").as_deref(), Some("5"));
    click(&mut eng, 182.0, 6.0);
    assert_eq!(val(&eng, "budget").as_deref(), Some("5"));
    click(&mut eng, 182.0, 6.0);
    assert_eq!(val(&eng, "budget").as_deref(), Some("5"), "stops at max");
    enter(&mut eng, "99", false);
    assert_eq!(
        val(&eng, "budget").as_deref(),
        Some("5"),
        "typed clamps at max"
    );
    let mut eng = boot(&budget("40", ""));
    set(&mut eng, 1, "budget", InputLimits::new(0.0, 5.0, Some(1.0))).unwrap();
    assert_eq!(
        val(&eng, "budget").as_deref(),
        Some("5"),
        "applied clamps at max"
    );
}

#[test]
fn switching_new_to_at_least_clears_the_written_max() {
    let mut eng = boot(&budget("4", ""));
    set(&mut eng, 1, "budget", InputLimits::new(0.0, 5.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("budget", "max").as_deref(), Some("5"));
    set(&mut eng, 2, "budget", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("budget", "max"), None);
    click(&mut eng, 182.0, 6.0);
    click(&mut eng, 182.0, 6.0);
    assert_eq!(val(&eng, "budget").as_deref(), Some("6"));
}

#[test]
fn at_least_validates_min_and_step() {
    let mut eng = boot(&budget("1", ""));
    assert!(matches!(
        set(
            &mut eng,
            1,
            "budget",
            InputLimits::at_least(f64::NAN, Some(1.0))
        ),
        Err(EngineError::InvalidInputLimits { .. })
    ));
    assert!(matches!(
        set(
            &mut eng,
            2,
            "budget",
            InputLimits::at_least(1e300, Some(1.0))
        ),
        Err(EngineError::InvalidInputLimits { .. })
    ));
    assert!(matches!(
        set(&mut eng, 3, "budget", InputLimits::at_least(0.0, Some(0.0))),
        Err(EngineError::InvalidInputLimits { .. })
    ));
    set(&mut eng, 4, "budget", InputLimits::at_least(-3.0, None)).unwrap();
}

fn range(value: &str, extra: &str) -> String {
    format!(
        "<form id=\"f\"><input id=\"vol\" type=\"range\" name=\"vol\" value=\"{value}\"{extra} />\
         <button id=\"save\" hx-post=\"services.apply\">Apply</button></form>"
    )
}

#[test]
fn at_least_range_keeps_the_markup_max() {
    let mut eng = boot(&range("70", " max=\"50\""));
    set(&mut eng, 1, "vol", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("50"));
    assert_eq!(
        val(&eng, "vol").as_deref(),
        Some("50"),
        "clamps at the markup max"
    );
    eng.test_set_focus("vol");
    key(&mut eng, "ArrowUp");
    assert_eq!(val(&eng, "vol").as_deref(), Some("50"));
}

#[test]
fn at_least_range_without_markup_max_uses_100() {
    let mut eng = boot(&range("150", ""));
    set(&mut eng, 1, "vol", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("vol", "max"), None, "HTML's default max");
    assert_eq!(val(&eng, "vol").as_deref(), Some("100"));
}

#[test]
fn at_least_range_after_new_goes_back_to_the_markup_max() {
    let mut eng = boot(&range("30", " max=\"50\""));
    set(&mut eng, 1, "vol", InputLimits::new(0.0, 10.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("10"));
    set(&mut eng, 2, "vol", InputLimits::at_least(0.0, Some(1.0))).unwrap();
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("50"));
}
