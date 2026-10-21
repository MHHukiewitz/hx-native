//! Issue #16: range/number `min`/`max`/`step` from the `actions` slice (A28).
//!
//! Designer lock: samples/mocks never hand-write `min`/`max`/`step`; the
//! `actions` slice (`actions.<action>.limits.<input>`) is the source of
//! truth and always wins over markup. Game: limits are fixed per build, so
//! the engine must not re-pull them on every sync. UI: the rendered value
//! stays clamped so a stale draft can never sit outside the range.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hx_native::*;

// ---------------------------------------------------------------------------
// Shared boot / slice / fragment helpers
// ---------------------------------------------------------------------------

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    }
}

fn vp_scale(scale: f64) -> Viewport {
    Viewport::with_scale(
        ((400.0 * scale).round()) as u32,
        ((200.0 * scale).round()) as u32,
        scale,
    )
}

fn boot(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(actions);
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(vp());
    eng
}

fn click(eng: &mut Engine, x: f32, y: f32) -> InputOutcome {
    let _ = eng.input(InputEvent::PointerMove {
        x,
        y,
        mods: Modifiers::default(),
    });
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

fn key(eng: &mut Engine, key: &str) -> InputOutcome {
    eng.input(InputEvent::Key(KeyEvent {
        key: key.into(),
        code: key.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }))
}

fn action_args(out: &InputOutcome) -> Option<&[(String, String)]> {
    match out {
        InputOutcome::Action { args, .. } => Some(args.as_slice()),
        _ => None,
    }
}

/// `SliceSource` that counts `input_limits` calls (test 2 / mutation L3).
#[derive(Clone, Default)]
struct LimitsSlices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
    limits: Vec<(String, String, InputLimits)>,
    limits_calls: Arc<Mutex<u32>>,
}

impl SliceSource for LimitsSlices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        *self.limits_calls.lock().unwrap() += 1;
        self.limits.clone()
    }
}

#[derive(Default)]
struct Frags {
    sections: HashMap<String, String>,
}

impl FragmentSource for Frags {
    fn render(&mut self, id: &str, _slices: &[&str]) -> String {
        self.sections.get(id).cloned().unwrap_or_default()
    }
}

fn limits_for(entries: Vec<(&str, &str, InputLimits)>) -> Vec<(String, String, InputLimits)> {
    entries
        .into_iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c))
        .collect()
}

fn one_limit(action: &str, input: &str, limits: InputLimits) -> LimitsSlices {
    LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits_for(vec![(action, input, limits)]),
        limits_calls: Arc::new(Mutex::new(0)),
    }
}

// ---------------------------------------------------------------------------
// 1. Limits from the slice appear on the inputs after `sync`.
// ---------------------------------------------------------------------------

#[test]
fn limits_from_slice_appear_on_range_and_number_inputs() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <input id="fee" type="number" name="fee" value="2.20" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits_for(vec![
            (
                "settings.apply",
                "volume",
                InputLimits::new(0.0, 10.0, None),
            ),
            (
                "settings.apply",
                "fee",
                InputLimits::new(0.0, 5.0, Some(0.01)),
            ),
        ]),
        limits_calls: Arc::new(Mutex::new(0)),
    };
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();

    assert_eq!(eng.attr("vol", "min").as_deref(), Some("0"));
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("10"));
    assert_eq!(
        eng.attr("vol", "step").as_deref(),
        Some("any"),
        "step: None must write step=\"any\""
    );

    assert_eq!(eng.attr("fee", "min").as_deref(), Some("0"));
    assert_eq!(eng.attr("fee", "max").as_deref(), Some("5"));
    assert_eq!(eng.attr("fee", "step").as_deref(), Some("0.01"));
}

// ---------------------------------------------------------------------------
// 2. Changing limits re-applies/re-clamps; unchanged version does not recall.
// ---------------------------------------------------------------------------

#[test]
fn changing_limits_reclamps_and_recalls_only_on_version_change() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let calls = Arc::new(Mutex::new(0));
    let slices = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits_for(vec![(
            "settings.apply",
            "volume",
            InputLimits::new(0.0, 10.0, None),
        )]),
        limits_calls: calls.clone(),
    };
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "initial sync calls input_limits once"
    );

    // Same actions version again: must not recall input_limits.
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "unchanged actions version must not recall input_limits"
    );

    // Bump the actions version and tighten max below the current value (5).
    let slices2 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 2)]),
        actions: Vec::new(),
        limits: limits_for(vec![(
            "settings.apply",
            "volume",
            InputLimits::new(0.0, 3.0, None),
        )]),
        limits_calls: calls.clone(),
    };
    eng.sync(&slices2, &mut frags).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        2,
        "actions version change must recall input_limits once"
    );
    assert_eq!(
        eng.attr("vol", "value").as_deref(),
        Some("3"),
        "current value 5 must clamp to the new max 3"
    );
}

// ---------------------------------------------------------------------------
// 3. A swap re-renders the form; cached limits re-apply with no new call.
// ---------------------------------------------------------------------------

#[test]
fn swap_reapplies_cached_limits_without_new_input_limits_call() {
    let html = r##"
<form id="f">
  <section id="panel" hx-trigger="slice:ui" hx-target="#panel" hx-swap="innerHTML">
    <input id="vol" type="range" name="volume" value="5" />
  </section>
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"##;
    let mut eng = boot(html, &["settings.apply"]);
    let calls = Arc::new(Mutex::new(0));
    let limits = limits_for(vec![(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 10.0, None),
    )]);
    let slices = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1), ("ui".into(), 1)]),
        actions: Vec::new(),
        limits: limits.clone(),
        limits_calls: calls.clone(),
    };
    let mut frags = Frags {
        sections: HashMap::from([(
            "panel".into(),
            r#"<input id="vol" type="range" name="volume" value="99" />"#.into(),
        )]),
    };
    // Initial full render swaps "panel" once, then limits apply: 99 clamps to 10.
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(eng.attr("vol", "min").as_deref(), Some("0"));
    assert_eq!(eng.attr("vol", "value").as_deref(), Some("10"));

    // Bump only `ui` (not `actions`): the fresh fragment omits limits by
    // design (Designer lock), so the engine must reapply the cache with no
    // new `input_limits` call.
    frags.sections.insert(
        "panel".into(),
        r#"<input id="vol" type="range" name="volume" value="77" />"#.into(),
    );
    let slices2 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1), ("ui".into(), 2)]),
        actions: Vec::new(),
        limits,
        limits_calls: calls.clone(),
    };
    eng.sync(&slices2, &mut frags).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "ui-only bump must not recall input_limits"
    );
    assert_eq!(
        eng.attr("vol", "min").as_deref(),
        Some("0"),
        "swap must reapply cached min onto the fresh fragment"
    );
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("10"));
    assert_eq!(
        eng.attr("vol", "value").as_deref(),
        Some("10"),
        "77 must clamp to the cached max 10 after reapply"
    );
}

// ---------------------------------------------------------------------------
// 4. Posted args stay within limits: drag past max, typed out-of-range
//    commit, arrow step, spin step.
// ---------------------------------------------------------------------------

#[test]
fn drag_past_max_clamps_posted_args_to_slice_limit() {
    // The range has no `hx-trigger` of its own (a realistic Settings form:
    // one Save button posts every field). The drag clamp must still land in
    // the `value` attribute, and the Save click must post the clamped value.
    let html = r#"
<style>html,body{margin:0}
#vol{position:absolute;left:0;top:0;width:200px;height:24px}
#save{position:absolute;left:0;top:40px;width:60px;height:24px}
</style>
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 10.0, None),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();

    let _ = eng.input(InputEvent::PointerDown {
        x: 10.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    // Drag far past the right edge of the 200px-wide control.
    let _ = eng.input(InputEvent::PointerMove {
        x: 5000.0,
        y: 12.0,
        mods: Modifiers::default(),
    });
    let _ = eng.input(InputEvent::PointerUp {
        x: 5000.0,
        y: 12.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    assert_eq!(
        eng.attr("vol", "value").as_deref(),
        Some("10"),
        "drag past max must clamp to the slice max, not the markup default of 100"
    );

    let out = click(&mut eng, 30.0, 52.0); // the Save button
    let args = action_args(&out).expect("Action on Save click");
    assert_eq!(
        args_get(args, "volume"),
        Some("10"),
        "posted args must carry the clamped value: {args:?}"
    );
}

#[test]
fn typed_out_of_range_commit_clamps_to_slice_limit() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="1" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "fee",
        InputLimits::new(0.0, 5.0, Some(1.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();

    eng.test_set_focus("fee");
    let _ = eng.input(InputEvent::Text("9".into()));
    let _ = eng.input(InputEvent::Text("9".into()));
    let _ = key(&mut eng, "Enter");
    assert_eq!(
        eng.attr("fee", "value").as_deref(),
        Some("5"),
        "typed 99 must clamp to the slice max 5 on commit"
    );
}

#[test]
fn arrow_step_stops_at_slice_bounds() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="4" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "fee",
        InputLimits::new(0.0, 5.0, Some(1.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();

    eng.test_set_focus("fee");
    let _ = key(&mut eng, "ArrowUp"); // 4 -> 5
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("5"));
    let _ = key(&mut eng, "ArrowUp"); // stays at the slice max
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("5"));
}

#[test]
fn spin_step_clamps_to_slice_bounds() {
    let html = r#"
<style>html,body{margin:0}
#fee{position:absolute;left:0;top:0;width:80px;height:24px}
#up{position:absolute;left:90px;top:0;width:24px;height:12px}
</style>
<form id="f">
  <input id="fee" type="number" name="fee" value="4" />
  <span id="up" data-hx-step="up" data-hx-step-for="fee">up</span>
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "fee",
        InputLimits::new(0.0, 5.0, Some(1.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();

    let _ = click(&mut eng, 100.0, 6.0); // 4 -> 5
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("5"));
    let _ = click(&mut eng, 100.0, 6.0); // stays at the slice max
    assert_eq!(eng.attr("fee", "value").as_deref(), Some("5"));
}

// ---------------------------------------------------------------------------
// 5. Unknown input name fails sync; missing form is not an error; invalid
//    limits fail sync.
// ---------------------------------------------------------------------------

#[test]
fn unknown_input_name_fails_sync() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit("settings.apply", "nope", InputLimits::new(0.0, 10.0, None));
    let mut frags = Frags::default();
    let err = eng
        .sync(&slices, &mut frags)
        .expect_err("unknown limit input name must fail sync");
    assert!(
        matches!(
            &err,
            EngineError::UnknownLimitInput { action, input }
                if action == "settings.apply" && input == "nope"
        ),
        "got {err:?}"
    );
}

#[test]
fn limits_for_action_with_no_rendered_form_is_not_an_error() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    // "other.action" is declared but has no hx-post anywhere in the document
    // (not rendered on this surface) — must not be an error.
    let mut eng = boot(html, &["settings.apply", "other.action"]);
    let slices = one_limit(
        "other.action",
        "whatever",
        InputLimits::new(0.0, 10.0, None),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags)
        .expect("no form for the action must not be an error");
}

#[test]
fn invalid_limits_fail_sync() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    const MAX_SAFE: f64 = 9_007_199_254_740_992.0; // 2^53
    let bad = [
        InputLimits::new(10.0, 0.0, None),          // min > max
        InputLimits::new(0.0, 10.0, Some(0.0)),     // step == 0
        InputLimits::new(0.0, 10.0, Some(-1.0)),    // step < 0
        InputLimits::new(f64::NAN, 10.0, None),     // non-finite min
        InputLimits::new(0.0, f64::INFINITY, None), // non-finite max
        // Blocker 2 (PR27 round 2): beyond ±2^53 is rejected at validation.
        // (`MAX_SAFE + 1.0` rounds right back to `MAX_SAFE` in f64 — not
        // actually beyond the boundary — so use a value clearly distinct.)
        InputLimits::new(0.0, MAX_SAFE * 2.0, None),
        InputLimits::new(-(MAX_SAFE * 2.0), 0.0, None),
        InputLimits::new(0.0, 1e300, None),
        // Blocker 2: below the 1e-10 step floor is rejected at validation.
        InputLimits::new(0.0, 10.0, Some(1e-11)),
    ];
    for limits in bad {
        let mut eng = boot(html, &["settings.apply"]);
        let slices = one_limit("settings.apply", "volume", limits.clone());
        let mut frags = Frags::default();
        let err = eng
            .sync(&slices, &mut frags)
            .expect_err(&format!("invalid limits must fail sync: {limits:?}"));
        assert!(
            matches!(err, EngineError::InvalidInputLimits { .. }),
            "got {err:?} for {limits:?}"
        );
    }
}

/// Blocker 2 (PR27 round 2): values and steps right at the accepted
/// boundaries (±2^53, step == 1e-10) must format correctly — no `i64`
/// wraparound, no "rounds to 0" precision loss for a tiny-but-valid step.
#[test]
fn limits_at_safe_boundaries_format_correctly() {
    const MAX_SAFE: f64 = 9_007_199_254_740_992.0; // 2^53
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "volume",
        InputLimits::new(-MAX_SAFE, MAX_SAFE, Some(1e-10)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags)
        .expect("boundary limits must be accepted");
    assert_eq!(eng.attr("vol", "min").as_deref(), Some("-9007199254740992"));
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("9007199254740992"));
    assert_eq!(eng.attr("vol", "step").as_deref(), Some("0.0000000001"));
}

// ---------------------------------------------------------------------------
// 6. Markup min/max/step is overwritten by the slice.
// ---------------------------------------------------------------------------

#[test]
fn slice_limits_overwrite_markup() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" min="0" max="10" step="1" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 200.0, Some(5.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(
        eng.attr("vol", "max").as_deref(),
        Some("200"),
        "slice must win over markup max"
    );
    assert_eq!(
        eng.attr("vol", "step").as_deref(),
        Some("5"),
        "slice must win over markup step"
    );
}

// ---------------------------------------------------------------------------
// 7. A clamp emits no Action / no change; display regroups for number inputs.
// ---------------------------------------------------------------------------

#[test]
fn clamp_on_limit_change_emits_no_action_and_regroups_display() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="180000" data-hx-group="on" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "fee",
        InputLimits::new(0.0, 1_000_000.0, None),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert!(
        !eng.has_pending(),
        "a clamp must never create a pending Action"
    );

    // Tighten the max below the current value: the clamp must fire silently.
    let slices2 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 2)]),
        actions: Vec::new(),
        limits: limits_for(vec![(
            "settings.apply",
            "fee",
            InputLimits::new(0.0, 50_000.0, None),
        )]),
        limits_calls: Arc::new(Mutex::new(0)),
    };
    eng.sync(&slices2, &mut frags).unwrap();
    assert_eq!(
        eng.attr("fee", "value").as_deref(),
        Some("50000"),
        "value must clamp to the new max"
    );
    assert!(
        !eng.has_pending(),
        "clamp must never create a pending Action / fire change"
    );
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("50,000"),
        "unfocused number display must regroup after the clamp"
    );
}

// ---------------------------------------------------------------------------
// 8. Damage is limited to the changed inputs; partial == full byte-identical
//    at scales 1.0 / 1.25 / 2.0.
// ---------------------------------------------------------------------------

fn damage_overlaps(damage: &[Rect], box_xywh: (f32, f32, f32, f32), scale: f64) -> bool {
    let (x, y, w, h) = box_xywh;
    let bx0 = x as f64 * scale;
    let by0 = y as f64 * scale;
    let bx1 = (x + w) as f64 * scale;
    let by1 = (y + h) as f64 * scale;
    damage.iter().any(|r| {
        let rx0 = r.x as f64;
        let ry0 = r.y as f64;
        let rx1 = (r.x + r.width) as f64;
        let ry1 = (r.y + r.height) as f64;
        rx0 < bx1 && rx1 > bx0 && ry0 < by1 && ry1 > by0
    })
}

fn limits_damage_html() -> &'static str {
    r#"
<style>html,body{margin:0}
#vol{position:absolute;left:0;top:0;width:200px;height:24px}
#decoy{position:absolute;left:300px;top:150px;width:50px;height:20px;background:#048}
</style>
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <div id="decoy">x</div>
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#
}

fn run_limits_damage_partial_eq_full(scale: f64) {
    let html = limits_damage_html();
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    let limits = limits_for(vec![(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 3.0, None),
    )]);

    let mut eng = Engine::new(cfg());
    eng.declare_actions(&["settings.apply"]);
    eng.load_html(&wrapped).unwrap();
    let _ = eng.paint(vp_scale(scale)).unwrap();
    let slices = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits.clone(),
        limits_calls: Arc::new(Mutex::new(0)),
    };
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap(); // clamps 5 -> 3

    let partial = eng.paint(vp_scale(scale)).unwrap();
    assert!(!partial.skipped, "limit clamp must paint, scale={scale}");
    assert!(
        !eng.last_paint_stats().full_repaint,
        "limit clamp damage should stay partial at scale={scale}, stats={:?}",
        eng.last_paint_stats()
    );
    let decoy_box = eng.test_element_box("decoy").unwrap();
    assert!(
        !damage_overlaps(&partial.damage, decoy_box, scale),
        "limit clamp damage must not cover unrelated content at scale={scale}: {:?}",
        partial.damage
    );
    let partial_px = partial.rgba.unwrap();

    let mut eng_full = Engine::new(cfg());
    eng_full.declare_actions(&["settings.apply"]);
    eng_full.load_html(&wrapped).unwrap();
    let _ = eng_full.paint(vp_scale(scale)).unwrap();
    let slices2 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits,
        limits_calls: Arc::new(Mutex::new(0)),
    };
    let mut frags2 = Frags::default();
    eng_full.sync(&slices2, &mut frags2).unwrap();
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp_scale(scale)).unwrap();
    assert_eq!(
        partial_px.pixels,
        full.rgba.unwrap().pixels,
        "partial vs full differ at scale={scale}"
    );
}

#[test]
fn limits_damage_partial_eq_full_scale_1() {
    run_limits_damage_partial_eq_full(1.0);
}

#[test]
fn limits_damage_partial_eq_full_scale_1_25() {
    run_limits_damage_partial_eq_full(1.25);
}

#[test]
fn limits_damage_partial_eq_full_scale_2() {
    run_limits_damage_partial_eq_full(2.0);
}

// ---------------------------------------------------------------------------
// 9. PaintWorker parity, including coalesced syncs (mutation L8 target).
// ---------------------------------------------------------------------------

fn worker_html() -> String {
    format!(
        "<!DOCTYPE html><html><body>{}</body></html>",
        r#"
<style>html,body{margin:0}#vol{position:absolute;left:0;top:0;width:200px;height:24px}</style>
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#
    )
}

fn limits_packet(actions_ver: u64, max: f64) -> SyncPacket {
    let mut versions = HashMap::new();
    versions.insert("actions".into(), actions_ver);
    let mut pkt = SyncPacket::default();
    pkt.versions = versions;
    pkt.input_limits = vec![(
        "settings.apply".into(),
        "volume".into(),
        InputLimits::new(0.0, max, None),
    )];
    pkt
}

/// Deterministic (no thread timing involved): `merge_per_slice` must keep an
/// older packet's non-empty `input_limits` when the newer packet carries none
/// (Game's "hand them over once" design), and must adopt a newer non-empty
/// `input_limits` over an older one.
#[test]
fn merge_per_slice_replaces_actions_slice_as_a_whole() {
    let mut older = SyncPacket::default();
    older.versions.insert("actions".into(), 1);
    older.actions = vec![(
        "other.act".into(),
        ActionState::Disabled { reason: "x".into() },
    )];
    older.input_limits = vec![(
        "settings.apply".into(),
        "volume".into(),
        InputLimits::new(0.0, 3.0, None),
    )];

    // A newer packet that never touches the "actions" key (e.g. a
    // ticker-only bump) leaves the reserved actions slice untouched.
    let mut newer_ticker_only = SyncPacket::default();
    newer_ticker_only.versions.insert("ticker".into(), 2);
    older.merge_per_slice(newer_ticker_only);
    assert_eq!(
        older.input_limits.len(),
        1,
        "ticker-only bump must not touch input_limits"
    );
    assert_eq!(
        older.actions.len(),
        1,
        "ticker-only bump must not touch actions"
    );

    // A newer packet that *does* bump "actions" but carries empty
    // actions/input_limits is a deliberate clear (PR27 round 2 fix for the
    // same keep-when-empty bug Graph found on #24) — both get wiped, not
    // kept.
    let mut newer_clear = SyncPacket::default();
    newer_clear.versions.insert("actions".into(), 2);
    older.merge_per_slice(newer_clear);
    assert!(
        older.input_limits.is_empty(),
        "an actions-version bump with empty input_limits must clear it, not keep the old list"
    );
    assert!(
        older.actions.is_empty(),
        "an actions-version bump with empty actions must clear it, not keep the old list"
    );

    // A newer non-empty packet (also bumping "actions") replaces both.
    let newer_with_limits = limits_packet(3, 9.0);
    older.merge_per_slice(newer_with_limits.clone());
    assert_eq!(
        older.input_limits, newer_with_limits.input_limits,
        "a newer non-empty input_limits must win"
    );
}

/// The task's own equivalence requirement: coalescing two queued packets
/// into one `merge_per_slice` call, then syncing once, must land the engine
/// in the same observable state (greyed attrs + min/max/step) as applying
/// the two packets one at a time through two separate `Engine::sync` calls.
#[test]
fn merge_per_slice_matches_sequential_engine_sync() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
<button id="ob" hx-post="other.act" hx-trigger="click">Other</button>
"#;
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");

    let mut p1 = SyncPacket::default();
    p1.versions.insert("actions".into(), 1);
    p1.actions = vec![(
        "other.act".into(),
        ActionState::Disabled {
            reason: "busy".into(),
        },
    )];
    p1.input_limits = vec![(
        "settings.apply".into(),
        "volume".into(),
        InputLimits::new(0.0, 10.0, None),
    )];

    let mut p2 = SyncPacket::default();
    p2.versions.insert("actions".into(), 2);
    p2.actions = Vec::new(); // clears the greying
    p2.input_limits = Vec::new(); // clears the limits too

    // Sequential: two separate Engine::sync calls.
    let mut seq_eng = Engine::new(cfg());
    seq_eng.declare_actions(&["settings.apply", "other.act"]);
    seq_eng.load_html(&wrapped).unwrap();
    let mut sp1 = p1.clone();
    let s1 = sp1.clone();
    seq_eng.sync(&s1, &mut sp1).unwrap();
    let mut sp2 = p2.clone();
    let s2 = sp2.clone();
    seq_eng.sync(&s2, &mut sp2).unwrap();

    // Coalesced: merge into one packet, one Engine::sync call.
    let mut merged = p1;
    merged.merge_per_slice(p2);
    let mut merge_eng = Engine::new(cfg());
    merge_eng.declare_actions(&["settings.apply", "other.act"]);
    merge_eng.load_html(&wrapped).unwrap();
    let mut mp = merged.clone();
    let ms = mp.clone();
    merge_eng.sync(&ms, &mut mp).unwrap();

    assert_eq!(
        seq_eng.attr("ob", "disabled"),
        merge_eng.attr("ob", "disabled"),
        "disabled attr must match between sequential and coalesced sync"
    );
    assert_eq!(
        seq_eng.attr("vol", "min"),
        merge_eng.attr("vol", "min"),
        "min attr must match between sequential and coalesced sync"
    );
    assert_eq!(
        seq_eng.attr("vol", "max"),
        merge_eng.attr("vol", "max"),
        "max attr must match between sequential and coalesced sync"
    );
    assert_eq!(
        seq_eng.attr("vol", "step"),
        merge_eng.attr("vol", "step"),
        "step attr must match between sequential and coalesced sync"
    );
}

#[test]
fn paint_worker_matches_sync_engine_after_limits_apply_and_clamp() {
    let wrapped = worker_html();
    let view = Viewport::new(400, 200);

    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(&["settings.apply"]);
    sync_eng.load_html(&wrapped).unwrap();
    let mut pkt = limits_packet(1, 3.0);
    let slices = pkt.clone();
    sync_eng.sync(&slices, &mut pkt).unwrap();
    let sync_img = sync_eng.paint(view.clone()).unwrap().rgba.unwrap();

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["settings.apply"]).unwrap();
    worker.load_html(&wrapped).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, view.clone())
        .unwrap();
    worker.sync(limits_packet(1, 3.0)).unwrap();
    worker.flush_timeout(Duration::from_secs(10)).unwrap();
    let frame = worker.latest_frame(SurfaceId::DEFAULT, 0).unwrap();
    let host_img = frame.output.rgba.as_ref().unwrap().clone();

    assert_eq!(
        sync_img.pixels, host_img.pixels,
        "worker must match the sync engine after limits apply + clamp"
    );
}

#[test]
fn worker_coalesced_syncs_keep_input_limits() {
    let wrapped = worker_html();
    let view = Viewport::new(400, 200);

    let mut baseline_pkt = SyncPacket::default();
    baseline_pkt.versions.insert("actions".into(), 1);
    let limited_pkt = limits_packet(1, 3.0);

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["settings.apply"]).unwrap();
    worker.load_html(&wrapped).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, view.clone())
        .unwrap();
    // Two syncs enqueued back-to-back before any flush: the mailbox may
    // coalesce them into one Sync command via `SyncPacket::merge_per_slice`,
    // which must not drop the limits carried by the second packet.
    worker.sync(baseline_pkt).unwrap();
    worker.sync(limited_pkt.clone()).unwrap();
    worker.flush_timeout(Duration::from_secs(10)).unwrap();
    let worker_img = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .unwrap()
        .output
        .rgba
        .as_ref()
        .unwrap()
        .clone();

    let mut eng = Engine::new(cfg());
    eng.declare_actions(&["settings.apply"]);
    eng.load_html(&wrapped).unwrap();
    let mut pkt = limited_pkt;
    let slices = pkt.clone();
    eng.sync(&slices, &mut pkt).unwrap();
    let eng_img = eng.paint(view).unwrap().rgba.unwrap();

    assert_eq!(
        worker_img.pixels, eng_img.pixels,
        "worker must keep coalesced input_limits (merge_per_slice must not drop them)"
    );
}

// ---------------------------------------------------------------------------
// PR27 round 2: blockers and non-blocking items.
// ---------------------------------------------------------------------------

/// Blocker 1: `step: None` writes `step="any"`, and typed decimal commits
/// must be accepted as typed, not snapped to an integer grid.
#[test]
fn step_none_accepts_typed_decimal_without_snapping() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="4" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit("settings.apply", "fee", InputLimits::new(0.0, 10.0, None));
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(eng.attr("fee", "step").as_deref(), Some("any"));

    eng.test_set_focus("fee");
    let _ = key(&mut eng, "End");
    let _ = eng.input(InputEvent::Text(".3".into()));
    assert_eq!(
        eng.test_editor_display("fee").as_deref(),
        Some("4.3"),
        "typed '.3' after 4 must be accepted as 4.3, the decimal point must not be rejected"
    );
    let _ = key(&mut eng, "Enter");
    assert_eq!(
        eng.attr("fee", "value").as_deref(),
        Some("4.3"),
        "committed value must stay 4.3, not snap to an integer grid"
    );
}

/// Blocker 1: arrow stepping still moves by 1 even when `step` is `None`,
/// and does not snap the result to any grid (preserves the fraction).
#[test]
fn step_none_arrow_still_steps_by_one_without_snapping() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="4.3" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit("settings.apply", "fee", InputLimits::new(0.0, 10.0, None));
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    eng.test_set_focus("fee");
    let _ = key(&mut eng, "ArrowUp");
    assert_eq!(
        eng.attr("fee", "value").as_deref(),
        Some("5.3"),
        "arrow must step by 1 and keep the fractional part (no grid snap)"
    );
}

/// Blocker 3: an unknown input alongside a valid one fails the whole sync
/// without half-applying the valid entry; a later corrected sync applies.
#[test]
fn unknown_input_error_is_atomic_then_corrected_sync_applies() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let bad = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits_for(vec![
            ("settings.apply", "volume", InputLimits::new(0.0, 3.0, None)),
            ("settings.apply", "nosuch", InputLimits::new(0.0, 3.0, None)),
        ]),
        limits_calls: Arc::new(Mutex::new(0)),
    };
    let mut frags = Frags::default();
    let err = eng
        .sync(&bad, &mut frags)
        .expect_err("unknown input must fail the whole sync");
    assert!(matches!(err, EngineError::UnknownLimitInput { .. }));
    assert_eq!(
        eng.attr("vol", "min"),
        None,
        "the valid entry must not be half-applied"
    );
    assert_eq!(eng.attr("vol", "max"), None, "nor its max");
    assert_eq!(
        eng.attr("vol", "value").as_deref(),
        Some("5"),
        "value must stay untouched"
    );

    let good = one_limit("settings.apply", "volume", InputLimits::new(0.0, 3.0, None));
    eng.sync(&good, &mut frags)
        .expect("a later corrected sync must apply");
    assert_eq!(eng.attr("vol", "min").as_deref(), Some("0"));
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("3"));
    assert_eq!(
        eng.attr("vol", "value").as_deref(),
        Some("3"),
        "5 must now clamp to the corrected max"
    );
}

/// Blocker 3: a failed sync must not record the `actions` version, so a
/// retry with the same (still-bad) packet re-calls `input_limits` again
/// instead of silently skipping it.
#[test]
fn failed_sync_does_not_record_actions_version() {
    let html = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let calls = Arc::new(Mutex::new(0));
    let bad = LimitsSlices {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        limits: limits_for(vec![(
            "settings.apply",
            "nosuch",
            InputLimits::new(0.0, 3.0, None),
        )]),
        limits_calls: calls.clone(),
    };
    let mut frags = Frags::default();
    let _ = eng
        .sync(&bad, &mut frags)
        .expect_err("unknown input must fail sync");
    assert_eq!(*calls.lock().unwrap(), 1);

    let _ = eng
        .sync(&bad, &mut frags)
        .expect_err("still fails: the input is still unknown");
    assert_eq!(
        *calls.lock().unwrap(),
        2,
        "a failed sync must not record the actions version, so the retry recalls input_limits"
    );
}

/// Blocker 4: an empty number field stays empty — applying limits must not
/// write `min` into it. `min`/`max`/`step` still apply so a later typed
/// value is validated against them.
#[test]
fn empty_number_field_stays_empty_after_limits_apply() {
    let html = r#"
<form id="f">
  <input id="fee" type="number" name="fee" value="" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "fee",
        InputLimits::new(3.0, 10.0, Some(1.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(
        eng.attr("fee", "value").as_deref(),
        Some(""),
        "an empty number field must stay empty, not get filled with min"
    );
    assert_eq!(eng.attr("fee", "min").as_deref(), Some("3"));
    assert_eq!(eng.attr("fee", "max").as_deref(), Some("10"));
}

/// Non-blocking: when a later `input_limits()` no longer lists an input
/// that earlier got limits, the engine removes them — restoring markup
/// min/max/step if it recorded any, or clearing the attributes otherwise.
#[test]
fn removed_limits_restore_markup_or_clear_when_absent() {
    let html_with_markup = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" min="1" max="9" step="2" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng = boot(html_with_markup, &["settings.apply"]);
    let slices = one_limit(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 100.0, Some(10.0)),
    );
    let mut frags = Frags::default();
    eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(eng.attr("vol", "max").as_deref(), Some("100"));

    let slices2 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 2)]),
        actions: Vec::new(),
        limits: Vec::new(), // "volume" is no longer listed
        limits_calls: Arc::new(Mutex::new(0)),
    };
    eng.sync(&slices2, &mut frags).unwrap();
    assert_eq!(
        eng.attr("vol", "min").as_deref(),
        Some("1"),
        "markup min must be restored"
    );
    assert_eq!(
        eng.attr("vol", "max").as_deref(),
        Some("9"),
        "markup max must be restored"
    );
    assert_eq!(
        eng.attr("vol", "step").as_deref(),
        Some("2"),
        "markup step must be restored"
    );

    let html_no_markup = r#"
<form id="f">
  <input id="vol" type="range" name="volume" value="5" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
"#;
    let mut eng2 = boot(html_no_markup, &["settings.apply"]);
    let slices3 = one_limit(
        "settings.apply",
        "volume",
        InputLimits::new(0.0, 100.0, Some(10.0)),
    );
    eng2.sync(&slices3, &mut frags).unwrap();
    assert_eq!(eng2.attr("vol", "max").as_deref(), Some("100"));
    let slices4 = LimitsSlices {
        versions: HashMap::from([("actions".into(), 2)]),
        actions: Vec::new(),
        limits: Vec::new(),
        limits_calls: Arc::new(Mutex::new(0)),
    };
    eng2.sync(&slices4, &mut frags).unwrap();
    assert_eq!(
        eng2.attr("vol", "min"),
        None,
        "no markup to restore to: min must be cleared"
    );
    assert_eq!(
        eng2.attr("vol", "max"),
        None,
        "no markup to restore to: max must be cleared"
    );
    assert_eq!(
        eng2.attr("vol", "step"),
        None,
        "no markup to restore to: step must be cleared"
    );
}
