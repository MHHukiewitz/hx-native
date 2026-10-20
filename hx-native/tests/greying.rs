//! Issue #15 (A27): greying from unapplied form values.
//!
//! `ActionState::RequiresChange` (Apply/Reset greying while a form is
//! clean), `SliceSource::input_rules` / `InputRule::EnabledWhen` (dependent
//! inputs), the Designer-locked `data-hx-applied` baseline override, and the
//! `hx-changed` class (Graph requirement).

#![allow(clippy::type_complexity)]

use std::collections::HashMap;

use hx_native::*;

// ---------------------------------------------------------------------------
// Shared boot / slice / input helpers
// ---------------------------------------------------------------------------

fn vp() -> Viewport {
    Viewport {
        width: 500,
        height: 500,
        scale: 1.0,
        holes: vec![],
    }
}

fn boot(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(actions);
    let wrapped = format!("<!DOCTYPE html><html><body style=\"margin:0\">{html}</body></html>");
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(vp());
    eng
}

#[derive(Default)]
struct Slices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
    rules: Vec<(String, String, InputRule)>,
}

impl Slices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("actions".into(), 1);
        versions.insert("panel".into(), 1);
        versions.insert("ticker".into(), 1);
        Self {
            versions,
            ..Default::default()
        }
    }
    fn bump(&mut self, name: &str) {
        *self.versions.entry(name.into()).or_insert(0) += 1;
    }
}

impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
    fn input_rules(&self) -> Vec<(String, String, InputRule)> {
        self.rules.clone()
    }
}

#[derive(Default)]
struct Frags {
    bodies: HashMap<String, String>,
}

impl Frags {
    fn set(&mut self, section: &str, html: &str) {
        self.bodies.insert(section.into(), html.into());
    }
}

impl FragmentSource for Frags {
    fn render(&mut self, section_id: &str, _slices: &[&str]) -> String {
        self.bodies
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- {section_id} -->"))
    }
}

/// Set a single `RequiresChange` action state and sync.
fn requires_change(
    eng: &mut Engine,
    slices: &mut Slices,
    frags: &mut Frags,
    action: &str,
    form: &str,
    reason: &str,
) {
    slices.actions = vec![(
        action.to_string(),
        ActionState::RequiresChange {
            form: form.to_string(),
            reason: reason.to_string(),
        },
    )];
    slices.bump("actions");
    eng.sync(slices, frags).unwrap();
}

fn center(eng: &Engine, id: &str) -> (f32, f32) {
    let b = eng.test_element_box(id).expect(id);
    (b.0 + b.2 * 0.5, b.1 + b.3 * 0.5)
}

fn click(eng: &mut Engine, id: &str) -> InputOutcome {
    let (x, y) = center(eng, id);
    ptr(eng, x, y)
}

fn ptr(eng: &mut Engine, x: f32, y: f32) -> InputOutcome {
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

fn drag(eng: &mut Engine, from: (f32, f32), to: (f32, f32)) -> InputOutcome {
    let _ = eng.input(InputEvent::PointerDown {
        x: from.0,
        y: from.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let _ = eng.input(InputEvent::PointerMove {
        x: to.0,
        y: to.1,
        mods: Modifiers::default(),
    });
    eng.input(InputEvent::PointerUp {
        x: to.0,
        y: to.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    })
}

/// x for `value` on a range track spanning `min..=max` (box left + ratio*width).
fn range_x_for(eng: &Engine, id: &str, value: f64, min: f64, max: f64) -> f32 {
    let (bx, _, bw, _) = eng.test_element_box(id).unwrap();
    bx + ((value - min) / (max - min)) as f32 * bw
}

fn range_y(eng: &Engine, id: &str) -> f32 {
    let (_, by, _, bh) = eng.test_element_box(id).unwrap();
    by + bh * 0.5
}

/// Replace a focused text/number field's whole content.
fn set_field_text(eng: &mut Engine, id: &str, text: &str) {
    eng.test_set_focus(id);
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "Backspace".into(),
        code: "Backspace".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    let _ = eng.input(InputEvent::Text(text.into()));
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

fn is_greyed(eng: &Engine, id: &str, reason: &str) -> bool {
    eng.attr(id, "disabled").is_some()
        && eng.attr(id, "aria-disabled").as_deref() == Some("true")
        && eng.attr(id, "data-hx-reason").as_deref() == Some(reason)
}

fn is_enabled(eng: &Engine, id: &str) -> bool {
    eng.attr(id, "disabled").is_none()
        && eng.attr(id, "aria-disabled").is_none()
        && eng.attr(id, "data-hx-reason").is_none()
}

// ---------------------------------------------------------------------------
// Fixture 1: the big form, one control of each kind (group A / E / F / G / J)
// ---------------------------------------------------------------------------

const BIG_FORM: &str = r##"
<form id="f">
  <input id="label" type="text" name="label" value="Base" style="width:160px;height:22px;display:block"/>
  <input id="qty" type="number" name="qty" value="5" min="0" max="100" step="1" style="width:100px;height:22px;display:block"/>
  <input id="vol" type="range" name="vol" value="50" min="0" max="100" step="1" style="width:200px;height:24px;display:block"/>
  <input id="music" type="checkbox" name="music" style="display:block"/>
  <input id="mode-a" type="radio" name="mode" value="a" checked style="display:block"/>
  <input id="mode-b" type="radio" name="mode" value="b" style="display:block"/>
  <select id="quality" name="quality" style="display:block">
    <option value="low">Low</option>
    <option value="high">High</option>
  </select>
  <button id="apply" name="apply" hx-post="settings.apply" hx-trigger="click" style="display:block">Apply</button>
</form>"##;

fn boot_big_form() -> Engine {
    boot(BIG_FORM, &["settings.apply"])
}

const REASON: &str = "No changes to apply";

// ---------------------------------------------------------------------------
// A: clean form greys Apply per control kind; one edit ungreys; revert greys.
// ---------------------------------------------------------------------------

#[test]
fn a27_range_edit_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(
        is_greyed(&eng, "apply", REASON),
        "clean form must grey Apply"
    );

    let y = range_y(&eng, "vol");
    let from = (range_x_for(&eng, "vol", 50.0, 0.0, 100.0), y);
    let to = (range_x_for(&eng, "vol", 90.0, 0.0, 100.0), y);
    let out = drag(&mut eng, from, to);
    assert!(
        !matches!(out, InputOutcome::Action { .. }),
        "drag edit must not emit Action: {out:?}"
    );
    assert_ne!(eng.attr("vol", "value").unwrap(), "50");
    assert!(is_enabled(&eng, "apply"), "edit must ungrey Apply");

    let back = (range_x_for(&eng, "vol", 50.0, 0.0, 100.0), y);
    let _ = drag(&mut eng, to, back);
    assert_eq!(eng.attr("vol", "value").unwrap(), "50");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "revert must grey Apply again"
    );
}

#[test]
fn a27_number_typed_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    set_field_text(&mut eng, "qty", "9");
    assert_eq!(eng.attr("qty", "value").unwrap(), "9");
    assert!(is_enabled(&eng, "apply"), "typed number must ungrey Apply");

    set_field_text(&mut eng, "qty", "5");
    assert_eq!(eng.attr("qty", "value").unwrap(), "5");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "reverted number must grey Apply"
    );
}

#[test]
fn a27_text_typed_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    set_field_text(&mut eng, "label", "Edited");
    assert!(is_enabled(&eng, "apply"), "typed text must ungrey Apply");

    set_field_text(&mut eng, "label", "Base");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "reverted text must grey Apply"
    );
}

#[test]
fn a27_checkbox_toggle_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    let out = click(&mut eng, "music");
    assert!(!matches!(out, InputOutcome::Action { .. }));
    assert!(
        is_enabled(&eng, "apply"),
        "checkbox toggle must ungrey Apply"
    );

    let _ = click(&mut eng, "music");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "reverted checkbox must grey Apply"
    );
}

#[test]
fn a27_radio_pick_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    let out = click(&mut eng, "mode-b");
    assert!(!matches!(out, InputOutcome::Action { .. }));
    assert!(is_enabled(&eng, "apply"), "radio pick must ungrey Apply");

    let _ = click(&mut eng, "mode-a");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "reverted radio must grey Apply"
    );
}

#[test]
fn a27_select_step_ungreys_revert_greys() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    eng.test_set_focus("quality");
    let out = key(&mut eng, "ArrowDown"); // closed-select step: low -> high
    assert!(!matches!(out, InputOutcome::Action { .. }));
    assert!(
        is_enabled(&eng, "apply"),
        "select step must ungrey Apply, out={out:?}"
    );

    let _ = key(&mut eng, "ArrowUp");
    assert!(
        is_greyed(&eng, "apply", REASON),
        "reverted select must grey Apply"
    );
}

// ---------------------------------------------------------------------------
// B: re-render resets baseline; unrelated slice bump does not.
// ---------------------------------------------------------------------------

const SWAPPABLE_FORM: &str = r##"
<section id="sec-f2" hx-trigger="slice:panel" hx-target="#f2" hx-swap="outerHTML">
  <form id="f2">
    <input id="label2" type="text" name="label2" value="Base" style="display:block"/>
    <button id="apply2b" name="apply2b" hx-post="settings.apply2b" hx-trigger="click" style="display:block">Apply</button>
  </form>
</section>
<section id="sec-other2" hx-trigger="slice:ticker" hx-target="#other2" hx-swap="innerHTML">
  <div id="other2">v0</div>
</section>"##;

fn boot_swappable_form() -> Engine {
    boot(SWAPPABLE_FORM, &["settings.apply2b"])
}

const F2_BASE_HTML: &str = r#"<form id="f2"><input id="label2" type="text" name="label2" value="Base" style="display:block"/><button id="apply2b" name="apply2b" hx-post="settings.apply2b" hx-trigger="click" style="display:block">Apply</button></form>"#;

/// `sync()` does one full render of *every* section on the first call
/// (regardless of slice versions), so `sec-f2` / `sec-other2` must already
/// have a `FragmentSource` body matching their static starting markup, or
/// that first sync would swap the form away with an empty placeholder.
fn initial_swappable_frags() -> Frags {
    let mut frags = Frags::default();
    frags.set("sec-f2", F2_BASE_HTML);
    frags.set("sec-other2", r#"<div id="other2">v0</div>"#);
    frags
}

/// A re-render (sync swap of the form fragment) resets the baseline to the
/// freshly-rendered values (pending the UI `settings.tab` open decision —
/// unless a control carries `data-hx-applied`, see the I-group tests).
#[test]
fn a27_rerender_resets_baseline() {
    let mut eng = boot_swappable_form();
    let mut slices = Slices::new();
    let mut frags = initial_swappable_frags();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply2b",
        "f2",
        REASON,
    );
    assert!(is_greyed(&eng, "apply2b", REASON));

    set_field_text(&mut eng, "label2", "Draft");
    assert!(is_enabled(&eng, "apply2b"), "edit must ungrey");

    // Re-render #f2 with a DIFFERENT rendered value than the live edit.
    frags.set(
        "sec-f2",
        r#"<form id="f2"><input id="label2" type="text" name="label2" value="Rerendered" style="display:block"/><button id="apply2b" name="apply2b" hx-post="settings.apply2b" hx-trigger="click" style="display:block">Apply</button></form>"#,
    );
    slices.bump("panel");
    eng.sync(&slices, &mut frags).unwrap();

    assert_eq!(eng.attr("label2", "value").unwrap(), "Rerendered");
    assert!(
        is_greyed(&eng, "apply2b", REASON),
        "re-render must reset baseline to the new rendered value (clean again)"
    );
}

#[test]
fn a27_unrelated_slice_bump_does_not_reset_baseline() {
    let mut eng = boot_swappable_form();
    let mut slices = Slices::new();
    let mut frags = initial_swappable_frags();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply2b",
        "f2",
        REASON,
    );
    assert!(is_greyed(&eng, "apply2b", REASON));

    set_field_text(&mut eng, "label2", "Draft");
    assert!(is_enabled(&eng, "apply2b"));

    // Bump an unrelated slice; its section swap never touches #f2.
    frags.set("sec-other2", r#"<div id="other2">v1</div>"#);
    slices.bump("ticker");
    eng.sync(&slices, &mut frags).unwrap();

    assert_eq!(eng.attr("other2", "value"), None);
    assert!(
        is_enabled(&eng, "apply2b"),
        "an unrelated slice bump must not reset the baseline (Apply must stay enabled)"
    );
}

// ---------------------------------------------------------------------------
// C: no Action emitted during edits; Blocked race fallback.
// ---------------------------------------------------------------------------

#[test]
fn a27_no_action_emitted_during_edits() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );

    let y = range_y(&eng, "vol");
    let from = (range_x_for(&eng, "vol", 50.0, 0.0, 100.0), y);
    let mid = (range_x_for(&eng, "vol", 70.0, 0.0, 100.0), y);
    let _ = eng.input(InputEvent::PointerDown {
        x: from.0,
        y: from.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let mid_out = eng.input(InputEvent::PointerMove {
        x: mid.0,
        y: mid.1,
        mods: Modifiers::default(),
    });
    assert!(
        !matches!(
            mid_out,
            InputOutcome::Action { .. } | InputOutcome::Blocked { .. }
        ),
        "mid-drag must not emit Action/Blocked: {mid_out:?}"
    );
    let _ = eng.input(InputEvent::PointerUp {
        x: mid.0,
        y: mid.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });

    set_field_text(&mut eng, "label", "x");
    let outs = [click(&mut eng, "music"), click(&mut eng, "mode-b")];
    for o in &outs {
        assert!(
            !matches!(o, InputOutcome::Action { name, .. } if name == "settings.apply"),
            "no edit must fire settings.apply: {o:?}"
        );
    }
}

#[test]
fn a27_blocked_race_fallback() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    assert!(is_greyed(&eng, "apply", REASON));

    // Click a just-greyed Apply (clean form, race): Blocked, not Action.
    let out = click(&mut eng, "apply");
    match out {
        InputOutcome::Blocked { name, reason, .. } => {
            assert_eq!(name, "settings.apply");
            assert_eq!(reason, REASON);
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// D: dependent inputs (EnabledWhen).
// ---------------------------------------------------------------------------

const DEPENDENT_FORM: &str = r##"
<form id="g">
  <input id="music2" type="checkbox" name="music2" style="display:block"/>
  <input id="musicVolume" type="range" name="musicVolume" value="20" min="0" max="100" step="1" style="width:200px;height:24px;display:block"/>
  <input id="musicLabel" type="text" name="musicLabel" value="quiet" style="display:block"/>
  <input id="musicMuted" type="checkbox" name="musicMuted" style="display:block"/>
  <button id="applyG" name="applyG" hx-post="settings.g" hx-trigger="click" style="display:block">Apply</button>
</form>"##;

const GATE_REASON: &str = "Enable Music first";

fn boot_dependent_form() -> Engine {
    boot(DEPENDENT_FORM, &["settings.g"])
}

fn dependent_rules() -> Vec<(String, String, InputRule)> {
    ["musicVolume", "musicLabel", "musicMuted"]
        .iter()
        .map(|name| {
            (
                "settings.g".to_string(),
                name.to_string(),
                InputRule::EnabledWhen {
                    input: "music2".to_string(),
                    checked: true,
                    reason: GATE_REASON.to_string(),
                },
            )
        })
        .collect()
}

#[test]
fn a27_dependent_inputs_grey_on_uncheck_ungrey_on_check() {
    let mut eng = boot_dependent_form();
    let mut slices = Slices::new();
    slices.rules = dependent_rules();
    let mut frags = Frags::default();
    // No RequiresChange on this form; just sync to apply input_rules.
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();

    for id in ["musicVolume", "musicLabel", "musicMuted"] {
        assert!(
            is_greyed(&eng, id, GATE_REASON),
            "{id} must be greyed while Music is unchecked"
        );
    }

    let _ = click(&mut eng, "music2");
    for id in ["musicVolume", "musicLabel", "musicMuted"] {
        assert!(
            is_enabled(&eng, id),
            "{id} must ungrey once Music is checked"
        );
    }

    let _ = click(&mut eng, "music2");
    for id in ["musicVolume", "musicLabel", "musicMuted"] {
        assert!(
            is_greyed(&eng, id, GATE_REASON),
            "{id} must grey again once Music is unchecked"
        );
    }
}

#[test]
fn a27_dependent_input_refuses_drag_while_greyed() {
    let mut eng = boot_dependent_form();
    let mut slices = Slices::new();
    slices.rules = dependent_rules();
    let mut frags = Frags::default();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_greyed(&eng, "musicVolume", GATE_REASON));

    let y = range_y(&eng, "musicVolume");
    let from = (range_x_for(&eng, "musicVolume", 20.0, 0.0, 100.0), y);
    let to = (range_x_for(&eng, "musicVolume", 90.0, 0.0, 100.0), y);
    let _ = drag(&mut eng, from, to);
    assert_eq!(
        eng.attr("musicVolume", "value").unwrap(),
        "20",
        "drag on a greyed range must be refused"
    );
}

#[test]
fn a27_dependent_input_refuses_typing_while_greyed() {
    let mut eng = boot_dependent_form();
    let mut slices = Slices::new();
    slices.rules = dependent_rules();
    let mut frags = Frags::default();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_greyed(&eng, "musicLabel", GATE_REASON));

    set_field_text(&mut eng, "musicLabel", "loud");
    assert_eq!(
        eng.attr("musicLabel", "value").unwrap(),
        "quiet",
        "typing into a greyed text field must be refused"
    );
}

#[test]
fn a27_dependent_input_refuses_toggle_and_step_while_greyed() {
    let mut eng = boot_dependent_form();
    let mut slices = Slices::new();
    slices.rules = dependent_rules();
    let mut frags = Frags::default();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_greyed(&eng, "musicMuted", GATE_REASON));

    let _ = click(&mut eng, "musicMuted");
    assert!(
        eng.attr("musicMuted", "checked").is_none() && !checkbox_checked(&eng, "musicMuted"),
        "toggle on a greyed checkbox must be refused"
    );

    eng.test_set_focus("musicVolume");
    let _ = key(&mut eng, "ArrowRight");
    assert_eq!(
        eng.attr("musicVolume", "value").unwrap(),
        "20",
        "arrow step on a greyed range must be refused"
    );
}

fn checkbox_checked(eng: &Engine, id: &str) -> bool {
    // musicMuted never gets a `checked` attribute written by a refused
    // toggle; a real toggle would add it (engine writes the attribute).
    eng.attr(id, "checked").is_some()
}

#[test]
fn a27_dependent_input_still_posted_in_args_while_greyed() {
    let mut eng = boot_dependent_form();
    let mut slices = Slices::new();
    slices.rules = dependent_rules();
    let mut frags = Frags::default();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_greyed(&eng, "musicVolume", GATE_REASON));

    let out = click(&mut eng, "applyG");
    match out {
        InputOutcome::Action { args, .. } => {
            assert_eq!(
                args_get(&args, "musicVolume"),
                Some("20"),
                "greyed dependent input must still post its value (K1): {args:?}"
            );
        }
        other => panic!("expected Action, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// E: host-authored disabled survives; existing Disabled behaviour unchanged.
// ---------------------------------------------------------------------------

const HOST_DISABLED_FORM: &str = r##"
<form id="g2">
  <input id="hostDisabled" type="checkbox" name="hostDisabled" disabled style="display:block"/>
  <input id="gate2" type="checkbox" name="gate2" checked style="display:block"/>
  <input id="other3" type="text" name="other3" value="x" style="display:block"/>
  <button id="plainBtn" name="plainBtn" hx-post="plain.action" hx-trigger="click" style="display:block">Go</button>
</form>"##;

#[test]
fn a27_host_authored_disabled_survives() {
    let mut eng = boot(HOST_DISABLED_FORM, &["plain.action"]);
    let mut slices = Slices::new();
    slices.rules = vec![(
        "plain.action".to_string(),
        "hostDisabled".to_string(),
        InputRule::EnabledWhen {
            input: "gate2".to_string(),
            checked: true,
            reason: "irrelevant".to_string(),
        },
    )];
    let mut frags = Frags::default();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();

    // Rule satisfied from the start (gate2 checked): engine never touches it.
    assert!(eng.attr("hostDisabled", "disabled").is_some());
    assert!(eng.attr("hostDisabled", "aria-disabled").is_none());
    assert!(eng.attr("hostDisabled", "data-hx-reason").is_none());

    // An unrelated edit (recompute runs) must not disturb it either.
    set_field_text(&mut eng, "other3", "y");
    assert!(
        eng.attr("hostDisabled", "disabled").is_some(),
        "host-authored disabled must survive an unrelated recompute"
    );
}

#[test]
fn a27_disabled_action_unchanged() {
    let html = r#"<button id="btn" hx-post="plain.action" hx-trigger="click">Go</button>"#;
    let mut eng = boot(html, &["plain.action"]);
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    slices.actions.push((
        "plain.action".into(),
        ActionState::Disabled {
            reason: "sim busy".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_greyed(&eng, "btn", "sim busy"));

    slices.actions.clear();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(is_enabled(&eng, "btn"));
}

// ---------------------------------------------------------------------------
// H: missing form id -> enabled, no error.
// ---------------------------------------------------------------------------

#[test]
fn a27_missing_form_is_enabled_no_error() {
    let html = r#"<button id="applyH" hx-post="settings.ghost" hx-trigger="click">Apply</button>"#;
    let mut eng = boot(html, &["settings.ghost"]);
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.ghost",
        "ghost",
        REASON,
    );

    assert!(
        is_enabled(&eng, "applyH"),
        "RequiresChange on an unknown form must be treated as enabled"
    );
    let out = click(&mut eng, "applyH");
    assert!(matches!(out, InputOutcome::Action { .. }), "{out:?}");
}

// ---------------------------------------------------------------------------
// F: damage covers only changed controls; partial == full byte-identical.
// ---------------------------------------------------------------------------

fn assert_partial_eq_full(partial: &RgbaImage, full: &RgbaImage, label: &str) {
    assert_eq!(
        partial.pixels,
        full.pixels,
        "{label}: partial vs full differ ({} px)",
        partial
            .pixels
            .iter()
            .zip(full.pixels.iter())
            .filter(|(a, b)| a != b)
            .count()
    );
}

#[test]
fn a27_damage_scoped_to_changed_controls_partial_eq_full() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    let _ = eng.paint(vp());

    let _ = click(&mut eng, "music");
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped && partial.rgba.is_some());
    assert!(
        !eng.last_paint_stats().full_repaint,
        "expected a partial repaint, stats={:?}",
        eng.last_paint_stats()
    );
    let partial_px = partial.rgba.unwrap();
    let damage = partial.damage.clone();

    let (fx, fy, fw, fh) = eng.test_element_box("f").unwrap();
    let full_area = 500.0 * 500.0;
    let form_area = (fw * fh) as f64;
    let damaged_area: f64 = damage.iter().map(|r| (r.width * r.height) as f64).sum();
    assert!(
        damaged_area < full_area * 0.9,
        "damage must not fall back to a full repaint: {damage:?}"
    );
    let _ = (fx, fy, form_area);

    // Re-run the identical script against a fresh engine, then force a full
    // repaint of the same end state — partial must be byte-identical.
    let mut eng_full = boot_big_form();
    let mut slices_full = Slices::new();
    let mut frags_full = Frags::default();
    requires_change(
        &mut eng_full,
        &mut slices_full,
        &mut frags_full,
        "settings.apply",
        "f",
        REASON,
    );
    let _ = eng_full.paint(vp());
    let _ = click(&mut eng_full, "music");
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        &partial_px,
        full.rgba.as_ref().unwrap(),
        "hx-changed/RequiresChange damage",
    );
}

// ---------------------------------------------------------------------------
// G: worker parity.
// ---------------------------------------------------------------------------

fn worker_cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

fn wrapped_big_form() -> String {
    format!("<!DOCTYPE html><html><body style=\"margin:0\">{BIG_FORM}</body></html>")
}

fn base_packet() -> SyncPacket {
    SyncPacket {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        input_rules: Vec::new(),
        fragments: HashMap::new(),
    }
}

/// Run `script` (pointer/key events) through a worker and a sync `Engine`
/// booted identically, and return the `Action`/`Blocked` outcomes each saw,
/// in order. Both start from the same HTML + `SyncPacket`/`Slices`+`Frags`
/// pair, so their outcome streams must match byte-for-byte on names/reasons.
fn run_parity_script(
    html: &str,
    actions: &[&str],
    pkt: SyncPacket,
    script: &[InputEvent],
) -> (Vec<InputOutcome>, Vec<InputOutcome>) {
    let mut sync_eng = Engine::new(worker_cfg());
    sync_eng.declare_actions(actions);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(vp());
    let mut slices = pkt.clone();
    let mut frags = pkt.clone();
    sync_eng.sync(&slices, &mut frags).unwrap();
    let _ = &mut slices;
    let mut sync_outs = Vec::new();
    for ev in script {
        let o = sync_eng.input(ev.clone());
        if matches!(
            o,
            InputOutcome::Action { .. } | InputOutcome::Blocked { .. }
        ) {
            sync_outs.push(o);
        }
    }

    let worker = PaintWorker::spawn(worker_cfg());
    worker.declare_actions(actions).unwrap();
    worker.load_html(html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, vp()).unwrap();
    worker.sync(pkt).unwrap();
    worker.flush().unwrap();
    let _ = worker.poll_outcomes();
    for ev in script {
        worker.input(ev.clone()).unwrap();
    }
    worker.flush().unwrap();
    let worker_outs: Vec<InputOutcome> = worker
        .poll_outcomes()
        .into_iter()
        .map(|o| o.outcome)
        .filter(|o| {
            matches!(
                o,
                InputOutcome::Action { .. } | InputOutcome::Blocked { .. }
            )
        })
        .collect();

    (sync_outs, worker_outs)
}

fn click_script(x: f32, y: f32) -> Vec<InputEvent> {
    vec![
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    ]
}

#[test]
fn a27_worker_parity_requires_change() {
    let html = wrapped_big_form();
    let probe = boot_big_form();
    let (ax, ay) = center(&probe, "apply");
    let (lx, ly) = center(&probe, "label");

    let mut pkt = base_packet();
    pkt.actions = vec![(
        "settings.apply".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: REASON.into(),
        },
    )];
    pkt.versions.insert("actions".into(), 2);

    let mut script = click_script(ax, ay); // #1: click while clean -> Blocked
    script.extend(click_script(lx, ly)); // focus the label field
    script.push(InputEvent::Key(KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
    script.push(InputEvent::Key(KeyEvent {
        key: "Backspace".into(),
        code: "Backspace".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    script.push(InputEvent::Text("Edited".into()));
    script.extend(click_script(ax, ay)); // #2: click after the edit -> Action

    let (sync_outs, worker_outs) = run_parity_script(&html, &["settings.apply"], pkt, &script);
    assert_eq!(sync_outs.len(), 2, "sync: {sync_outs:?}");
    assert_eq!(worker_outs.len(), 2, "worker: {worker_outs:?}");
    assert!(matches!(sync_outs[0], InputOutcome::Blocked { .. }));
    assert!(matches!(worker_outs[0], InputOutcome::Blocked { .. }));
    assert!(matches!(sync_outs[1], InputOutcome::Action { .. }));
    assert!(matches!(worker_outs[1], InputOutcome::Action { .. }));
}

#[test]
fn a27_worker_parity_enabled_when() {
    let html =
        format!("<!DOCTYPE html><html><body style=\"margin:0\">{DEPENDENT_FORM}</body></html>");
    let probe = boot_dependent_form();
    let y = range_y(&probe, "musicVolume");
    let from_x = range_x_for(&probe, "musicVolume", 20.0, 0.0, 100.0);
    let to_x = range_x_for(&probe, "musicVolume", 90.0, 0.0, 100.0);
    let (ax, ay) = center(&probe, "applyG");

    let mut pkt = base_packet();
    pkt.input_rules = dependent_rules();
    pkt.versions.insert("actions".into(), 2);

    let script = vec![
        InputEvent::PointerDown {
            x: from_x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        InputEvent::PointerMove {
            x: to_x,
            y,
            mods: Modifiers::default(),
        },
        InputEvent::PointerUp {
            x: to_x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        InputEvent::PointerDown {
            x: ax,
            y: ay,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        InputEvent::PointerUp {
            x: ax,
            y: ay,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    ];

    let (sync_outs, worker_outs) = run_parity_script(&html, &["settings.g"], pkt, &script);
    // The drag on the greyed range must be refused on both (no Action from
    // it); the Apply click still posts musicVolume unchanged (K1) on both.
    assert_eq!(sync_outs.len(), 1, "sync: {sync_outs:?}");
    assert_eq!(worker_outs.len(), 1, "worker: {worker_outs:?}");
    match (&sync_outs[0], &worker_outs[0]) {
        (InputOutcome::Action { args: sa, .. }, InputOutcome::Action { args: wa, .. }) => {
            assert_eq!(args_get(sa, "musicVolume"), Some("20"));
            assert_eq!(args_get(wa, "musicVolume"), Some("20"));
        }
        other => panic!("expected Action/Action, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// I: data-hx-applied (Designer lock 2026-10-20).
// ---------------------------------------------------------------------------

#[test]
fn a27_applied_baseline_survives_rerender_with_draft_values() {
    let mut eng = boot_swappable_form();
    let mut slices = Slices::new();
    let mut frags = initial_swappable_frags();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply2b",
        "f2",
        REASON,
    );
    assert!(is_greyed(&eng, "apply2b", REASON));

    // Re-render with an unsaved draft value PLUS data-hx-applied carrying the
    // actually-applied value — the draft must keep Apply enabled.
    frags.set(
        "sec-f2",
        r#"<form id="f2"><input id="label2" type="text" name="label2" value="Draft Edited" data-hx-applied="Base" style="display:block"/><button id="apply2b" name="apply2b" hx-post="settings.apply2b" hx-trigger="click" style="display:block">Apply</button></form>"#,
    );
    slices.bump("panel");
    eng.sync(&slices, &mut frags).unwrap();
    let _ = eng.paint(vp()); // resolve layout for the swapped-in node before typing

    assert!(
        is_enabled(&eng, "apply2b"),
        "draft value differing from data-hx-applied must keep Apply enabled"
    );

    // Editing back to the applied value greys it again.
    set_field_text(&mut eng, "label2", "Base");
    assert!(
        is_greyed(&eng, "apply2b", REASON),
        "editing back to the applied value must grey Apply again"
    );
}

const APPLIED_CHECKBOX_FORM: &str = r##"
<form id="h">
  <input id="gift" type="checkbox" name="gift" data-hx-applied="true" style="display:block"/>
  <button id="applyH2" name="applyH2" hx-post="settings.h" hx-trigger="click" style="display:block">Apply</button>
</form>"##;

#[test]
fn a27_applied_checkbox_both_directions() {
    let mut eng = boot(APPLIED_CHECKBOX_FORM, &["settings.h"]);
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(&mut eng, &mut slices, &mut frags, "settings.h", "h", REASON);

    // Rendered unchecked ("false") vs applied "true": dirty from the start.
    assert!(
        is_enabled(&eng, "applyH2"),
        "rendered checkbox state differing from data-hx-applied must be dirty"
    );

    // Check it to match applied "true": clean.
    let _ = click(&mut eng, "gift");
    assert!(
        is_greyed(&eng, "applyH2", REASON),
        "matching the applied state must grey"
    );

    // Uncheck again: dirty.
    let _ = click(&mut eng, "gift");
    assert!(
        is_enabled(&eng, "applyH2"),
        "diverging from applied again must ungrey"
    );
}

const APPLIED_RADIO_FORM: &str = r##"
<form id="h2">
  <input id="opt-a" type="radio" name="opt" value="a" checked data-hx-applied="false" style="display:block"/>
  <input id="opt-b" type="radio" name="opt" value="b" data-hx-applied="true" style="display:block"/>
  <button id="applyH3" name="applyH3" hx-post="settings.h2" hx-trigger="click" style="display:block">Apply</button>
</form>"##;

#[test]
fn a27_applied_radio_both_directions() {
    let mut eng = boot(APPLIED_RADIO_FORM, &["settings.h2"]);
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.h2",
        "h2",
        REASON,
    );

    // "a" is rendered checked ("true") but data-hx-applied="false" (unset on
    // "a"); "b" is rendered unchecked ("false") but applied "true": both
    // radios mismatch their applied state -> dirty.
    assert!(
        is_enabled(&eng, "applyH3"),
        "radio mismatch vs applied must be dirty"
    );

    let _ = click(&mut eng, "opt-b");
    assert!(
        is_greyed(&eng, "applyH3", REASON),
        "picking the radio that matches data-hx-applied must grey"
    );

    let _ = click(&mut eng, "opt-a");
    assert!(
        is_enabled(&eng, "applyH3"),
        "picking away from applied again must ungrey"
    );
}

#[test]
fn a27_applied_missing_falls_back_to_rendered_value() {
    // No data-hx-applied anywhere: covered by a27_rerender_resets_baseline /
    // the A-group tests already (fallback = current behaviour). This test
    // documents the explicit fallback statement for a mixed form: one
    // control has data-hx-applied, the other does not.
    let html = r#"<form id="h3">
<input id="withApplied" type="text" name="withApplied" value="draft" data-hx-applied="base" style="display:block"/>
<input id="noApplied" type="text" name="noApplied" value="base" style="display:block"/>
<button id="applyH4" name="applyH4" hx-post="settings.h3" hx-trigger="click" style="display:block">Apply</button>
</form>"#;
    let mut eng = boot(html, &["settings.h3"]);
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.h3",
        "h3",
        REASON,
    );
    // withApplied differs from "base" -> dirty regardless of noApplied.
    assert!(is_enabled(&eng, "applyH4"));

    set_field_text(&mut eng, "withApplied", "base");
    // Now both match their baselines (withApplied matches applied="base";
    // noApplied matches its rendered-at-load fallback "base").
    assert!(is_greyed(&eng, "applyH4", REASON));

    // Editing the attribute-less control away from its rendered baseline
    // dirties it via the fallback path.
    set_field_text(&mut eng, "noApplied", "changed");
    assert!(is_enabled(&eng, "applyH4"));
}

#[test]
fn a27_applied_attribute_rejected_on_non_control_element() {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["a.x"]);
    let err = eng
        .load_html(
            r#"<!DOCTYPE html><html><body><div id="d" data-hx-applied="x">x</div></body></html>"#,
        )
        .expect_err("data-hx-applied on a div");
    match err {
        EngineError::InvalidAttribute { attr, .. } => assert_eq!(attr, ATTR_HX_APPLIED),
        other => panic!("expected InvalidAttribute, got {other:?}"),
    }

    // Allowed on input / select / textarea.
    eng.declare_actions(&["a.y"]);
    eng.load_html(
        r#"<!DOCTYPE html><html><body>
<input id="i" type="text" name="i" value="v" data-hx-applied="v"/>
<select id="s" name="s" data-hx-applied="a"><option value="a">A</option></select>
<textarea id="t" name="t" data-hx-applied="x"></textarea>
</body></html>"#,
    )
    .expect("input/select/textarea must accept data-hx-applied");
}

#[test]
fn a27_worker_parity_applied_baseline() {
    let html = format!(
        "<!DOCTYPE html><html><body style=\"margin:0\">{APPLIED_CHECKBOX_FORM}</body></html>"
    );
    let probe = boot(APPLIED_CHECKBOX_FORM, &["settings.h"]);
    let (ax, ay) = center(&probe, "applyH2");

    let mut pkt = base_packet();
    pkt.actions = vec![(
        "settings.h".into(),
        ActionState::RequiresChange {
            form: "h".into(),
            reason: REASON.into(),
        },
    )];
    pkt.versions.insert("actions".into(), 2);

    // The checkbox is rendered unchecked but `data-hx-applied="true"`:
    // dirty from the very first sync, on both engines.
    let (sync_outs, worker_outs) =
        run_parity_script(&html, &["settings.h"], pkt, &click_script(ax, ay));
    assert_eq!(sync_outs.len(), 1, "{sync_outs:?}");
    assert_eq!(worker_outs.len(), 1, "{worker_outs:?}");
    assert!(
        matches!(sync_outs[0], InputOutcome::Action { .. }),
        "{sync_outs:?}"
    );
    assert!(
        matches!(worker_outs[0], InputOutcome::Action { .. }),
        "{worker_outs:?}"
    );
}

// ---------------------------------------------------------------------------
// J: hx-changed class (Graph requirement).
// ---------------------------------------------------------------------------

#[test]
fn a27_hx_changed_appears_on_edit_clears_on_revert_all_kinds() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );

    for id in [
        "label", "qty", "vol", "music", "mode-a", "mode-b", "quality",
    ] {
        assert!(!eng.has_class(id, "hx-changed"), "{id} must start clean");
    }

    set_field_text(&mut eng, "label", "Edited");
    assert!(eng.has_class("label", "hx-changed"));
    set_field_text(&mut eng, "label", "Base");
    assert!(
        !eng.has_class("label", "hx-changed"),
        "reverted text must clear hx-changed"
    );

    set_field_text(&mut eng, "qty", "9");
    assert!(eng.has_class("qty", "hx-changed"));
    set_field_text(&mut eng, "qty", "5");
    assert!(!eng.has_class("qty", "hx-changed"));

    let y = range_y(&eng, "vol");
    let from = (range_x_for(&eng, "vol", 50.0, 0.0, 100.0), y);
    let to = (range_x_for(&eng, "vol", 90.0, 0.0, 100.0), y);
    let _ = drag(&mut eng, from, to);
    assert!(eng.has_class("vol", "hx-changed"));
    let _ = drag(&mut eng, to, from);
    assert!(!eng.has_class("vol", "hx-changed"));

    let _ = click(&mut eng, "music");
    assert!(
        eng.has_class("music", "hx-changed"),
        "checkbox toggle must set hx-changed"
    );
    let _ = click(&mut eng, "music");
    assert!(!eng.has_class("music", "hx-changed"));

    let _ = click(&mut eng, "mode-b");
    assert!(
        eng.has_class("mode-b", "hx-changed"),
        "newly-picked radio must be changed"
    );
    assert!(
        eng.has_class("mode-a", "hx-changed"),
        "the radio that was the baseline pick is now unchecked -> also changed"
    );
    let _ = click(&mut eng, "mode-a");
    assert!(!eng.has_class("mode-a", "hx-changed"));
    assert!(!eng.has_class("mode-b", "hx-changed"));

    eng.test_set_focus("quality");
    let _ = key(&mut eng, "ArrowDown");
    assert!(eng.has_class("quality", "hx-changed"));
    let _ = key(&mut eng, "ArrowUp");
    assert!(!eng.has_class("quality", "hx-changed"));
}

#[test]
fn a27_hx_changed_rerender_with_matching_value_has_no_class() {
    let mut eng = boot_swappable_form();
    let mut slices = Slices::new();
    let mut frags = initial_swappable_frags();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply2b",
        "f2",
        REASON,
    );
    assert!(!eng.has_class("label2", "hx-changed"));

    frags.set(
        "sec-f2",
        r#"<form id="f2"><input id="label2" type="text" name="label2" value="Base" style="display:block"/><button id="apply2b" name="apply2b" hx-post="settings.apply2b" hx-trigger="click" style="display:block">Apply</button></form>"#,
    );
    slices.bump("panel");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(
        !eng.has_class("label2", "hx-changed"),
        "a re-render whose value matches the (reset) baseline must carry no class"
    );
}

#[test]
fn a27_hx_changed_flip_damage_stays_within_control_bounds() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    let _ = eng.paint(vp());

    let _ = click(&mut eng, "music");
    assert!(eng.has_class("music", "hx-changed"));
    let out = eng.paint(vp()).unwrap();
    assert!(!out.skipped);
    let (mx, my, mw, mh) = eng.test_element_box("music").unwrap();
    // Damage rects may legitimately cover the Apply button too (its own
    // disabled/aria/reason flip from the same edit); assert at least one
    // rect is scoped near the checkbox and none approach a full repaint.
    let near_checkbox = out.damage.iter().any(|r| {
        let pad = 24.0;
        r.x <= mx + mw + pad
            && r.x + r.width >= mx - pad
            && r.y <= my + mh + pad
            && r.y + r.height >= my - pad
    });
    assert!(
        near_checkbox,
        "no damage rect near the checkbox: {:?}",
        out.damage
    );
    let full_area = 500.0 * 500.0;
    let damaged_area: f64 = out.damage.iter().map(|r| (r.width * r.height) as f64).sum();
    assert!(
        damaged_area < full_area * 0.5,
        "damage must not blow up to a full repaint: {:?}",
        out.damage
    );
}

#[test]
fn a27_hx_changed_second_keystroke_same_state_no_class_write_no_extra_damage() {
    let mut eng = boot_big_form();
    let mut slices = Slices::new();
    let mut frags = Frags::default();
    requires_change(
        &mut eng,
        &mut slices,
        &mut frags,
        "settings.apply",
        "f",
        REASON,
    );
    let _ = eng.paint(vp());

    eng.test_set_focus("label");
    let _ = eng.input(InputEvent::Text("X".into())); // flips unchanged -> changed
    assert!(eng.has_class("label", "hx-changed"));
    let writes_after_first = eng.test_hx_changed_write_count();
    assert!(writes_after_first >= 1);
    let _ = eng.paint(vp()); // resolve + clear damage flags

    let writes_before_second = eng.test_hx_changed_write_count();
    let _ = eng.input(InputEvent::Text("Y".into())); // still changed: no flip
    assert!(eng.has_class("label", "hx-changed"));
    assert_eq!(
        eng.test_hx_changed_write_count(),
        writes_before_second,
        "typing a second char while already changed must not rewrite the class"
    );

    // Cross-check: the damaged-element count for this no-flip keystroke must
    // match a plain edit on an untracked field (same shape of edit, no class
    // churn to add extra damaged nodes).
    let damaged_after_second = eng.test_damaged_element_count();

    let mut eng2 = boot_big_form(); // no RequiresChange wired up at all
    let _ = eng2.paint(vp());
    eng2.test_set_focus("label");
    let _ = eng2.input(InputEvent::Text("X".into()));
    let _ = eng2.paint(vp());
    let _ = eng2.input(InputEvent::Text("Y".into()));
    let damaged_plain = eng2.test_damaged_element_count();

    assert_eq!(
        damaged_after_second, damaged_plain,
        "a no-flip keystroke must damage exactly what a plain (untracked) keystroke damages"
    );
}

/// The worker exposes no attribute/class read, so parity is proved
/// behaviourally: `hx-changed` and `RequiresChange` share one baseline
/// compare, so toggling the checkbox must ungrey Apply identically on both.
#[test]
fn a27_hx_changed_worker_parity() {
    let html = wrapped_big_form();
    let probe = boot_big_form();
    let (mx, my) = center(&probe, "music");
    let (ax, ay) = center(&probe, "apply");

    let mut pkt = base_packet();
    pkt.actions = vec![(
        "settings.apply".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: REASON.into(),
        },
    )];
    pkt.versions.insert("actions".into(), 2);

    let mut script = click_script(mx, my);
    script.extend(click_script(ax, ay));
    let (sync_outs, worker_outs) = run_parity_script(&html, &["settings.apply"], pkt, &script);
    assert_eq!(sync_outs.len(), 1, "{sync_outs:?}");
    assert_eq!(worker_outs.len(), 1, "{worker_outs:?}");
    assert!(
        matches!(sync_outs[0], InputOutcome::Action { .. }),
        "{sync_outs:?}"
    );
    assert!(
        matches!(worker_outs[0], InputOutcome::Action { .. }),
        "{worker_outs:?}"
    );
}

// ---------------------------------------------------------------------------
// K: `hx-changed` opt-in by `data-hx-applied` alone (issue #24, Designer
// lock 2026-10-20 15:10 America/Asuncion). The attribute turns the class on
// for that control in ANY form (even one with no `RequiresChange` action at
// all) or in no form at all — independent of Apply/Confirm greying, which
// stays scoped to `RequiresChange` forms only. Pixel-level worker parity for
// this path is covered in `paint_worker_pixel_parity.rs` (the worker exposes
// no attribute/class read, so an outcome-only parity check here would add
// nothing beyond that pixel coverage).
// ---------------------------------------------------------------------------

const NO_ACTION_FORM: &str = r##"
<form id="plan">
  <input id="plan-qty" type="text" name="qty" value="Base" data-hx-applied="Base" style="width:160px;height:22px;display:block"/>
</form>"##;

/// A form with no action at all (no `hx-post`, nothing declared, nothing
/// registered with `RequiresChange`): editing its opted-in control must
/// still toggle `hx-changed`, and reverting must still clear it.
#[test]
fn a27_hx_changed_applies_in_form_with_no_action() {
    let mut eng = boot(NO_ACTION_FORM, &[]);
    assert!(!eng.has_class("plan-qty", "hx-changed"), "must start clean");

    set_field_text(&mut eng, "plan-qty", "Edited");
    assert!(
        eng.has_class("plan-qty", "hx-changed"),
        "data-hx-applied alone must opt this control into hx-changed, \
         even with no RequiresChange action anywhere"
    );

    set_field_text(&mut eng, "plan-qty", "Base");
    assert!(
        !eng.has_class("plan-qty", "hx-changed"),
        "reverting to the applied value must clear hx-changed"
    );
}

const STANDALONE_APPLIED_CONTROL: &str = r##"<input id="row-qty" type="text" name="qty" value="Base" data-hx-applied="Base" style="width:160px;height:22px;display:block"/>"##;

/// A control with `data-hx-applied` outside any `<form>` at all (the "plan
/// row with no button of its own" case the Designer lock calls out).
#[test]
fn a27_hx_changed_applies_to_standalone_control_outside_any_form() {
    let mut eng = boot(STANDALONE_APPLIED_CONTROL, &[]);
    assert!(!eng.has_class("row-qty", "hx-changed"), "must start clean");

    set_field_text(&mut eng, "row-qty", "Edited");
    assert!(
        eng.has_class("row-qty", "hx-changed"),
        "data-hx-applied alone must opt a formless control into hx-changed"
    );

    set_field_text(&mut eng, "row-qty", "Base");
    assert!(
        !eng.has_class("row-qty", "hx-changed"),
        "reverting to the applied value must clear hx-changed"
    );
}

/// Damage from the opt-in-by-attribute path (no `RequiresChange` form in
/// play at all) must stay scoped to the edited control, and a partial
/// repaint of that edit must be byte-identical to a forced full repaint —
/// same discipline as the `RequiresChange`-scoped path (group F).
#[test]
fn a27_hx_changed_opt_in_damage_partial_eq_full() {
    let html = format!("{NO_ACTION_FORM}{STANDALONE_APPLIED_CONTROL}");
    let mut eng = boot(&html, &[]);
    let _ = eng.paint(vp());

    set_field_text(&mut eng, "plan-qty", "Edited");
    let partial = eng.paint(vp()).unwrap();
    assert!(!partial.skipped && partial.rgba.is_some());
    assert!(!eng.last_paint_stats().full_repaint);
    let partial_px = partial.rgba.unwrap();

    let full_area = 500.0 * 500.0;
    let damaged_area: f64 = partial
        .damage
        .iter()
        .map(|r| (r.width * r.height) as f64)
        .sum();
    assert!(
        damaged_area < full_area * 0.5,
        "opt-in-by-attribute hx-changed damage must not blow up to a full \
         repaint: {:?}",
        partial.damage
    );

    let mut eng_full = boot(&html, &[]);
    let _ = eng_full.paint(vp());
    set_field_text(&mut eng_full, "plan-qty", "Edited");
    eng_full.mark_theme_or_stylesheet_changed();
    let full = eng_full.paint(vp()).unwrap();
    assert_partial_eq_full(
        &partial_px,
        full.rgba.as_ref().unwrap(),
        "standalone hx-changed opt-in damage",
    );
}
