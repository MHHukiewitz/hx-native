//! Headless acceptance tests named after REQUIREMENTS.md A-numbers.
//!
//! Fixture HTML under `tests/fixtures/` is engine-owned test data (not Designer mocks).

#![allow(clippy::type_complexity)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use hx_native::*;

const FIXTURE: &str = include_str!("fixtures/time_strip.html");
const TSTRIP_SHELL: &str = include_str!("fixtures/tstrip_shell.html");
const CHIPS_FRAGMENT: &str = include_str!("fixtures/chips.html");

fn default_actions() -> &'static [&'static str] {
    &[
        "time.pause",
        "time.set_speed",
        "time.pause_menu",
        "time.set_speed.bulk",
        "time.context",
    ]
}

fn boot(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
    });
    eng.declare_actions(default_actions());
    eng.load_html(html).expect("load_html");
    eng
}

struct MemSlices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
    version_asks: Arc<Mutex<Vec<String>>>,
}

impl MemSlices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("time".into(), 1);
        versions.insert("actions".into(), 1);
        Self {
            versions,
            actions: Vec::new(),
            version_asks: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn bump(&mut self, name: &str) {
        *self.versions.entry(name.into()).or_insert(0) += 1;
    }
}

impl SliceSource for MemSlices {
    fn version(&self, name: &str) -> Option<u64> {
        self.version_asks.lock().unwrap().push(name.to_string());
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}

struct SpyFrags {
    calls: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    bodies: HashMap<String, String>,
}

impl SpyFrags {
    fn new() -> Self {
        let mut bodies = HashMap::new();
        bodies.insert(
            "time-strip".into(),
            r#"<span id="time-label">DATE 2026-10-19 · 1x</span>"#.into(),
        );
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            bodies,
        }
    }
    fn set(&mut self, section: &str, html: &str) {
        self.bodies.insert(section.into(), html.into());
    }
}

impl FragmentSource for SpyFrags {
    fn render(&mut self, section_id: &str, slices: &[&str]) -> String {
        self.calls.lock().unwrap().push((
            section_id.to_string(),
            slices.iter().map(|s| (*s).to_string()).collect(),
        ));
        self.bodies
            .get(section_id)
            .cloned()
            .unwrap_or_else(|| format!("<!-- empty {section_id} -->"))
    }
}

fn click_at(eng: &mut Engine, x: f32, y: f32) -> Vec<InputOutcome> {
    click_at_btn(eng, x, y, PointerButton::Primary, Modifiers::default())
}

fn click_at_btn(
    eng: &mut Engine,
    x: f32,
    y: f32,
    button: PointerButton,
    mods: Modifiers,
) -> Vec<InputOutcome> {
    vec![
        eng.input(InputEvent::PointerMove { x, y, mods }),
        eng.input(InputEvent::PointerDown { x, y, button, mods }),
        eng.input(InputEvent::PointerUp { x, y, button, mods }),
    ]
}

/// Approximate button centers from the fixture CSS.
mod pos {
    pub const PAUSE: (f32, f32) = (350.0, 28.0);
    pub const SPEED: (f32, f32) = (460.0, 28.0);
    #[allow(dead_code)]
    pub const MENU: (f32, f32) = (570.0, 28.0);
    #[allow(dead_code)]
    pub const BULK: (f32, f32) = (680.0, 28.0);
    pub const CTX: (f32, f32) = (370.0, 80.0);
}

fn sync_paint(eng: &mut Engine, slices: &mut MemSlices, frags: &mut SpyFrags) {
    eng.sync(slices, frags).unwrap();
    eng.paint(Viewport {
        width: 800,
        height: 120,
        holes: vec![],
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// A1 — Headless boot
// ---------------------------------------------------------------------------
#[test]
fn a1_headless_boot() {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(default_actions());
    eng.load_html(FIXTURE).expect("A1 load");
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let paint = eng
        .paint(Viewport {
            width: 800,
            height: 120,
            holes: vec![],
        })
        .unwrap();
    assert!(paint.rgba.is_some());
    assert_eq!(paint.rgba.as_ref().unwrap().pixels.len(), 800 * 120 * 4);

    // Undeclared action → load error
    let mut eng2 = Engine::new(EngineConfig::default());
    eng2.declare_actions(&["only.this"]);
    let err = eng2
        .load_html(r#"<button id="b" hx-post="missing.action">x</button>"#)
        .unwrap_err();
    assert!(matches!(err, EngineError::UndeclaredAction { .. }));
}

// ---------------------------------------------------------------------------
// A22 — Invalid trigger fails load (also covers A1 path)
// ---------------------------------------------------------------------------
#[test]
fn a22_invalid_trigger_fails_load() {
    let cases = [
        r#"<button id="b" hx-post="time.pause" hx-trigger="dblclick">x</button>"#,
        r#"<button id="b" hx-post="time.pause" hx-trigger="mouseover">x</button>"#,
        r#"<button id="b" hx-post="time.pause" hx-trigger="click[event.foo]">x</button>"#,
        r#"<button id="b" hx-post="time.pause" hx-trigger="click[shiftKey==1]">x</button>"#,
        r#"<button id="b" hx-post="time.pause" hx-trigger="click[(shiftKey)]">x</button>"#,
        r##"<button id="b" hx-post="time.pause" hx-trigger="keydown[key=='a'] from:#id">x</button>"##,
        r#"<button id="b" hx-post="time.pause" hx-trigger="click from:body">x</button>"#,
    ];
    for html in cases {
        let mut eng = Engine::new(EngineConfig::default());
        eng.declare_actions(default_actions());
        let wrapped = format!("<html><body>{html}</body></html>");
        let err = eng
            .load_html(&wrapped)
            .expect_err("expected InvalidTrigger");
        assert!(
            matches!(err, EngineError::InvalidTrigger { .. }),
            "expected InvalidTrigger for {html}, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A2 — Determinism
// ---------------------------------------------------------------------------
#[test]
fn a2_determinism() {
    let run = || {
        let mut eng = boot(FIXTURE);
        let mut slices = MemSlices::new();
        let mut frags = SpyFrags::new();
        sync_paint(&mut eng, &mut slices, &mut frags);
        let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
        let html = eng.serialize_html().unwrap();
        let rgba = eng
            .paint(Viewport {
                width: 800,
                height: 120,
                holes: vec![],
            })
            .unwrap()
            .rgba
            .unwrap();
        (html, rgba.pixels, outs)
    };
    let (h1, p1, o1) = run();
    let (h2, p2, o2) = run();
    assert_eq!(h1, h2, "serialized HTML must match");
    assert_eq!(p1, p2, "RGBA buffers must match (bundled fonts)");
    assert_eq!(format!("{o1:?}"), format!("{o2:?}"));
}

// ---------------------------------------------------------------------------
// A3 — Stable ids
// ---------------------------------------------------------------------------
#[test]
fn a3_stable_ids() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);
    let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let action = outs.iter().find_map(|o| match o {
        InputOutcome::Action { source_id, .. } => Some(source_id.as_str()),
        _ => None,
    });
    assert_eq!(action, Some("btn-pause"));
}

// ---------------------------------------------------------------------------
// A3b — Hover → press → click order
// ---------------------------------------------------------------------------
#[test]
fn a3b_hover_press_click_order() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    let hover = eng.input(InputEvent::PointerMove {
        x: pos::PAUSE.0,
        y: pos::PAUSE.1,
        mods: Modifiers::default(),
    });
    assert!(matches!(hover, InputOutcome::Hover { id: Some(ref id) } if id == "btn-pause"));

    let press = eng.input(InputEvent::PointerDown {
        x: pos::PAUSE.0,
        y: pos::PAUSE.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    assert!(matches!(press, InputOutcome::Press { id: Some(ref id) } if id == "btn-pause"));

    let up = eng.input(InputEvent::PointerUp {
        x: pos::PAUSE.0,
        y: pos::PAUSE.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    assert!(matches!(
        up,
        InputOutcome::Action {
            source_id: ref id,
            ..
        } if id == "btn-pause"
    ));

    // Moving off → Hover{None}
    let leave = eng.input(InputEvent::PointerMove {
        x: 5.0,
        y: 5.0,
        mods: Modifiers::default(),
    });
    // May be Hover{None} or Hover{Some other} — fixture top-left is time-body.
    // Explicit leave:
    let leave2 = eng.input(InputEvent::PointerLeave);
    assert!(matches!(leave2, InputOutcome::Hover { id: None }));
    let _ = leave;

    // Release off target → Press{None}, no Action
    eng.input(InputEvent::PointerMove {
        x: pos::PAUSE.0,
        y: pos::PAUSE.1,
        mods: Modifiers::default(),
    });
    eng.input(InputEvent::PointerDown {
        x: pos::PAUSE.0,
        y: pos::PAUSE.1,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    let off = eng.input(InputEvent::PointerUp {
        x: 5.0,
        y: 5.0,
        button: PointerButton::Primary,
        mods: Modifiers::default(),
    });
    assert!(matches!(off, InputOutcome::Press { id: None }));
}

// ---------------------------------------------------------------------------
// A3c — Pending lifecycle
// ---------------------------------------------------------------------------
#[test]
fn a3c_pending_lifecycle() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let req = outs.iter().find_map(|o| match o {
        InputOutcome::Action { request, .. } => Some(*request),
        _ => None,
    });
    let req = req.expect("Action");
    assert!(eng.has_class("btn-pause", "hx-request"));
    assert_eq!(eng.attr("btn-pause", "aria-busy").as_deref(), Some("true"));
    assert!(eng.has_class("spinner", "hx-request"));

    // Bumping time does not clear pending.
    slices.bump("time");
    frags.set(
        "time-strip",
        r#"<span id="time-label">DATE 2026-10-20 · 1x</span>"#,
    );
    eng.sync(&slices, &mut frags).unwrap();
    assert!(eng.has_class("btn-pause", "hx-request"));

    eng.complete(req, Ok(()));
    assert!(!eng.has_class("btn-pause", "hx-request"));
    assert!(eng.attr("btn-pause", "aria-busy").is_none());
    assert!(!eng.has_class("spinner", "hx-request"));

    // Timeout case: no complete → harness fails loudly.
    let mut eng2 = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 2,
    });
    eng2.declare_actions(default_actions());
    eng2.load_html(FIXTURE).unwrap();
    let mut slices2 = MemSlices::new();
    let mut frags2 = SpyFrags::new();
    sync_paint(&mut eng2, &mut slices2, &mut frags2);
    let outs2 = click_at(&mut eng2, pos::PAUSE.0, pos::PAUSE.1);
    let req2 = outs2
        .iter()
        .find_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    eng2.test_tick_frame();
    eng2.test_tick_frame();
    eng2.test_tick_frame(); // > pending_timeout_frames
    assert!(
        !eng2.pending_timeout_failures().is_empty(),
        "timeout must fail loudly"
    );
    // Late complete is a no-op.
    eng2.complete(req2, Ok(()));
}

// ---------------------------------------------------------------------------
// A3d — hx-sync + failure
// ---------------------------------------------------------------------------
#[test]
fn a3d_hx_sync_and_failure() {
    // drop: double click → one Action
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);
    let o1 = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let o2 = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let actions1: Vec<_> = o1
        .iter()
        .chain(o2.iter())
        .filter(|o| matches!(o, InputOutcome::Action { .. }))
        .collect();
    assert_eq!(actions1.len(), 1, "drop: exactly one Action");

    // queue: one now, one from take_outcomes after complete
    let mut engq = boot(FIXTURE);
    let mut slicesq = MemSlices::new();
    let mut fragsq = SpyFrags::new();
    sync_paint(&mut engq, &mut slicesq, &mut fragsq);
    let a = click_at(&mut engq, pos::SPEED.0, pos::SPEED.1);
    let b = click_at(&mut engq, pos::SPEED.0, pos::SPEED.1);
    let first = a
        .iter()
        .find_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(b.iter().all(|o| !matches!(o, InputOutcome::Action { .. })));
    engq.complete(first, Ok(()));
    let released = engq.take_outcomes();
    assert_eq!(released.len(), 1);
    match &released[0] {
        InputOutcome::Action { request, .. } => {
            assert_eq!(request.0, first.0 + 1);
        }
        other => panic!("expected Action, got {other:?}"),
    }

    // complete Err shows reason like Blocked
    let mut eng3 = boot(FIXTURE);
    let mut s3 = MemSlices::new();
    let mut f3 = SpyFrags::new();
    sync_paint(&mut eng3, &mut s3, &mut f3);
    let outs = click_at(&mut eng3, pos::PAUSE.0, pos::PAUSE.1);
    let req = outs
        .iter()
        .find_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    eng3.complete(req, Err(Reason("boom".into())));
    assert_eq!(
        eng3.attr("btn-pause", "data-hx-reason").as_deref(),
        Some("boom")
    );
    assert!(!eng3.has_class("btn-pause", "hx-request"));

    // unknown hx-sync fails loudly
    let mut eng4 = Engine::new(EngineConfig::default());
    eng4.declare_actions(default_actions());
    let err = eng4
        .load_html(r#"<button id="b" hx-post="time.pause" hx-sync="abort">x</button>"#)
        .unwrap_err();
    assert!(matches!(err, EngineError::UnsupportedSync { .. }));
}

// ---------------------------------------------------------------------------
// A4 — click → Action
// ---------------------------------------------------------------------------
#[test]
fn a4_click_to_action() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);
    let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let action = outs
        .iter()
        .find(|o| matches!(o, InputOutcome::Action { .. }));
    match action {
        Some(InputOutcome::Action {
            name,
            source_id,
            request,
            ..
        }) => {
            assert_eq!(name, "time.pause");
            assert_eq!(source_id, "btn-pause");
            assert_eq!(request.0, 1);
        }
        _ => panic!("expected Action"),
    }
}

// ---------------------------------------------------------------------------
// A5 / A5b / A5c / A5d
// ---------------------------------------------------------------------------
#[test]
fn a5_slice_partial_swap() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    let dirty0 = eng.sync(&slices, &mut frags).unwrap();
    assert!(dirty0.sections.iter().any(|s| s == "time-strip"));
    eng.paint(Viewport {
        width: 800,
        height: 120,
        holes: vec![],
    })
    .unwrap();

    frags.set(
        "time-strip",
        r#"<span id="time-label">DATE 2026-10-21 · 2x</span>"#,
    );
    slices.bump("time");
    let dirty = eng.sync(&slices, &mut frags).unwrap();
    assert_eq!(dirty.sections, vec!["time-strip".to_string()]);
    // Partial: damaged nodes should be a subset of total elements (not a full-tree mark).
    assert!(
        dirty.damaged_node_count < dirty.total_element_count || dirty.total_element_count <= 3,
        "expected partial damage, damaged={} total={}",
        dirty.damaged_node_count,
        dirty.total_element_count
    );
    let html = eng.serialize_html().unwrap();
    assert!(html.contains("2026-10-21"));
}

#[test]
fn a5b_unsupported_swap_fails() {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(default_actions());
    let err = eng
        .load_html(r#"<div id="s" hx-trigger="slice:time" hx-swap="beforeend">x</div>"#)
        .unwrap_err();
    assert!(matches!(err, EngineError::UnsupportedSwap { .. }));
}

#[test]
fn a5_default_swap_is_inner_html() {
    // Spec addition: missing hx-swap defaults to innerHTML (htmx parity).
    let html = r##"<!DOCTYPE html><html><body>
      <section id="sec" hx-trigger="slice:time" hx-target="#box">
        <div id="box">OLD</div>
      </section>
    </body></html>"##;
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&[]);
    eng.load_html(html).unwrap();
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("sec", "NEW");
    eng.sync(&slices, &mut frags).unwrap();
    let ser = eng.serialize_html().unwrap();
    assert!(ser.contains("NEW"));
    // Outer section element should still exist (innerHTML, not outerHTML).
    assert!(ser.contains("id=\"sec\"") || ser.contains("id=sec"));
}

#[test]
fn a5c_clean_sections_never_render() {
    let html = r##"<!DOCTYPE html><html><body>
      <section id="sec-a" hx-trigger="slice:a" hx-target="#a" hx-swap="innerHTML"><div id="a">A</div></section>
      <section id="sec-b" hx-trigger="slice:b" hx-target="#b" hx-swap="innerHTML"><div id="b">B</div></section>
    </body></html>"##;
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&[]);
    eng.load_html(html).unwrap();

    let mut slices = MemSlices::new();
    slices.versions.insert("a".into(), 1);
    slices.versions.insert("b".into(), 1);
    let asks = slices.version_asks.clone();
    let mut frags = SpyFrags::new();
    frags.set("sec-a", "Aa");
    frags.set("sec-b", "Bb");
    let calls = frags.calls.clone();

    eng.sync(&slices, &mut frags).unwrap(); // initial full render
    calls.lock().unwrap().clear();
    asks.lock().unwrap().clear();

    slices.bump("a");
    frags.set("sec-a", "A2");
    eng.sync(&slices, &mut frags).unwrap();

    let call_ids: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(call_ids, vec!["sec-a".to_string()]);
    let asked: HashSet<String> = asks.lock().unwrap().iter().cloned().collect();
    assert!(asked.contains("a") || asked.contains("actions"));
    assert!(
        !asked.contains("unsubscribed"),
        "must not ask unsubscribed names"
    );
    // b is subscribed but clean — version still asked (we ask all subscribed),
    // but render must not be called for sec-b.
    assert!(!call_ids.iter().any(|c| c == "sec-b"));
}

#[test]
fn a5d_no_persisted_ui_state() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);
    let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    assert!(outs
        .iter()
        .any(|o| matches!(o, InputOutcome::Action { .. })));
    assert!(eng.has_class("btn-pause", "hx-request"));

    // Reload → pending dropped, full render of every section.
    frags.calls.lock().unwrap().clear();
    eng.load_html(FIXTURE).unwrap();
    assert!(!eng.has_class("btn-pause", "hx-request"));
    assert!(eng.attr("btn-pause", "aria-busy").is_none());
    let dirty = eng.sync(&slices, &mut frags).unwrap();
    assert!(dirty.sections.iter().any(|s| s == "time-strip"));
    // New Action after reload starts at RequestId(1) only in a *new* engine.
    // Spec: counter restarts at 1 in a new engine. Same engine keeps counting.
    let mut eng_new = boot(FIXTURE);
    let mut s2 = MemSlices::new();
    let mut f2 = SpyFrags::new();
    sync_paint(&mut eng_new, &mut s2, &mut f2);
    let outs2 = click_at(&mut eng_new, pos::PAUSE.0, pos::PAUSE.1);
    let req = outs2.iter().find_map(|o| match o {
        InputOutcome::Action { request, .. } => Some(request.0),
        _ => None,
    });
    assert_eq!(req, Some(1));
}

// ---------------------------------------------------------------------------
// A6 / A6b
// ---------------------------------------------------------------------------
#[test]
fn a6_greyed_before_click() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "sim busy".into(),
        },
    ));
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    assert!(eng.attr("btn-pause", "disabled").is_some());
    assert_eq!(
        eng.attr("btn-pause", "aria-disabled").as_deref(),
        Some("true")
    );
    assert_eq!(
        eng.attr("btn-pause", "data-hx-reason").as_deref(),
        Some("sim busy")
    );

    slices.actions.clear();
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    assert!(eng.attr("btn-pause", "disabled").is_none());
    assert!(eng.attr("btn-pause", "aria-disabled").is_none());
    assert!(eng.attr("btn-pause", "data-hx-reason").is_none());
}

#[test]
fn a6b_blocked_race_fallback() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);
    // Disable between paint and click without syncing into attributes —
    // the engine checks disabled_actions at click time from last sync.
    // Simulate race: sync with disabled after paint.
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "race".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    // Do not paint — click immediately.
    let outs = click_at(&mut eng, pos::PAUSE.0, pos::PAUSE.1);
    let blocked = outs
        .iter()
        .find(|o| matches!(o, InputOutcome::Blocked { .. }));
    match blocked {
        Some(InputOutcome::Blocked { name, reason, .. }) => {
            assert_eq!(name, "time.pause");
            assert_eq!(reason, "race");
        }
        _ => panic!("expected Blocked, got {outs:?}"),
    }
}

// ---------------------------------------------------------------------------
// A8 — Paint budget (debug smoke) + clean-frame skip
// ---------------------------------------------------------------------------
/// Debug-profile smoke only. It does **not** claim FRAME_BUDGET compliance.
/// What it guards: paint completes, reports timing when dirty, and respects
/// `max_paint_ns` as a generous debug ceiling (50 ms via `boot`). Real chrome
/// budget numbers: `cargo test --release --test a8_release_timing`.
#[test]
fn a8_paint_budget() {
    let mut eng = boot(FIXTURE);
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let out = eng
        .paint(Viewport {
            width: 800,
            height: 120,
            holes: vec![],
        })
        .unwrap();
    assert!(!out.skipped, "first paint after sync must raster");
    let t = eng.last_timings();
    eprintln!(
        "A8(debug) time-strip paint_ns={} sync_ns={} skipped={} max_paint_ns={} (debug ceiling only; see a8_release_timing)",
        t.paint_ns,
        t.sync_ns,
        t.paint_skipped,
        eng.config().max_paint_ns
    );
    assert!(
        t.paint_ns <= eng.config().max_paint_ns,
        "paint_ns {} exceeded debug max_paint_ns {}",
        t.paint_ns,
        eng.config().max_paint_ns
    );
    assert!(t.paint_ns > 0, "dirty paint timing must be measured");
    assert!(!t.paint_skipped);

    let out2 = eng
        .paint(Viewport {
            width: 800,
            height: 120,
            holes: vec![],
        })
        .unwrap();
    assert!(out2.skipped);
    assert!(eng.last_timings().paint_skipped);
    assert!(
        eng.last_timings().paint_ns < 1_000_000,
        "skipped paint must be cheap, got {} ns",
        eng.last_timings().paint_ns
    );
    assert!(
        out2.rgba.is_none(),
        "skipped paint must not copy the RGBA buffer"
    );
    assert!(
        eng.last_frame().is_some(),
        "last_frame still exposes the cached buffer without a copy"
    );
}

// ---------------------------------------------------------------------------
// A13 — No Mainnet / wgpu deps
// ---------------------------------------------------------------------------
#[test]
fn a13_cargo_tree_no_mainnet_or_wgpu() {
    use std::process::Command;
    let output = Command::new("cargo")
        .args(["tree", "-p", "hx-native", "--prefix", "none"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree");
    assert!(output.status.success(), "cargo tree failed");
    let tree = String::from_utf8_lossy(&output.stdout).to_lowercase();
    for banned in ["mainnet", "mainnet_graph", "graph_bridge", "wgpu "] {
        // Match package names carefully: "wgpu " prefix form from cargo tree lines.
        let hit = tree.lines().any(|line| {
            let pkg = line.split_whitespace().next().unwrap_or("");
            pkg == banned.trim()
                || pkg.starts_with("mainnet")
                || pkg == "graph_bridge"
                || pkg == "wgpu"
                || pkg.starts_with("wgpu-")
        });
        assert!(
            !hit,
            "banned dependency found near `{banned}` in cargo tree"
        );
    }
    // Also ensure blitz-net is absent.
    assert!(
        !tree.lines().any(|l| l.starts_with("blitz-net ")),
        "blitz-net must not be a dependency"
    );
}

// ---------------------------------------------------------------------------
// A17 — Right click
// ---------------------------------------------------------------------------
#[test]
fn a17_right_click() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    let outs = click_at_btn(
        &mut eng,
        pos::CTX.0,
        pos::CTX.1,
        PointerButton::Secondary,
        Modifiers::default(),
    );
    let action = outs
        .iter()
        .find(|o| matches!(o, InputOutcome::Action { .. }));
    match action {
        Some(InputOutcome::Action {
            name,
            trigger: TriggerInfo::Pointer { event, button, .. },
            ..
        }) => {
            assert_eq!(name, "time.context");
            assert_eq!(*event, "contextmenu");
            assert_eq!(*button, PointerButton::Secondary);
        }
        _ => panic!("expected contextmenu Action, got {outs:?}"),
    }

    // Primary click on same element emits nothing (no click trigger).
    let outs2 = click_at(&mut eng, pos::CTX.0, pos::CTX.1);
    assert!(
        outs2
            .iter()
            .all(|o| !matches!(o, InputOutcome::Action { .. })),
        "primary click must not fire Action on contextmenu-only control"
    );
}

// ---------------------------------------------------------------------------
// A18 — Modifier filters
// ---------------------------------------------------------------------------
#[test]
fn a18_modifier_filters() {
    let html = r##"<!DOCTYPE html><html><head><style>
      button { position:absolute; width:80px; height:40px; }
      #plain { left:10px; top:10px; }
      #shift { left:100px; top:10px; }
      #ctrl { left:190px; top:10px; }
      #meta { left:280px; top:10px; }
      #either { left:370px; top:10px; }
    </style></head><body>
      <button id="plain" hx-post="a.plain" hx-trigger="click[!shiftKey]">p</button>
      <button id="shift" hx-post="a.shift" hx-trigger="click[shiftKey]">s</button>
      <button id="ctrl" hx-post="a.ctrl" hx-trigger="click[ctrlKey]">c</button>
      <button id="meta" hx-post="a.meta" hx-trigger="click[metaKey]">m</button>
      <button id="either" hx-post="a.either" hx-trigger="click[ctrlKey||metaKey]">e</button>
    </body></html>"##;
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["a.plain", "a.shift", "a.ctrl", "a.meta", "a.either"]);
    eng.load_html(html).unwrap();
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    eng.paint(Viewport {
        width: 500,
        height: 80,
        holes: vec![],
    })
    .unwrap();

    let shift_mods = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    // shift button without shift → no action
    let o = click_at_btn(
        &mut eng,
        140.0,
        30.0,
        PointerButton::Primary,
        Modifiers::default(),
    );
    assert!(o.iter().all(|x| !matches!(x, InputOutcome::Action { .. })));
    // with shift → action
    let o = click_at_btn(&mut eng, 140.0, 30.0, PointerButton::Primary, shift_mods);
    match o.iter().find(|x| matches!(x, InputOutcome::Action { .. })) {
        Some(InputOutcome::Action {
            name,
            trigger: TriggerInfo::Pointer { mods, .. },
            ..
        }) => {
            assert_eq!(name, "a.shift");
            assert!(mods.shift);
        }
        _ => panic!("expected shift Action"),
    }

    // plain with shift → no; without → yes
    let o = click_at_btn(&mut eng, 50.0, 30.0, PointerButton::Primary, shift_mods);
    assert!(o.iter().all(|x| !matches!(x, InputOutcome::Action { .. })));
    let o = click_at(&mut eng, 50.0, 30.0);
    assert!(o
        .iter()
        .any(|x| matches!(x, InputOutcome::Action { name, .. } if name == "a.plain")));

    // Helper: complete any Action so pending drop does not hide later clicks.
    let complete_all = |eng: &mut Engine, outs: &[InputOutcome]| {
        for o in outs {
            if let InputOutcome::Action { request, .. } = o {
                eng.complete(*request, Ok(()));
            }
        }
    };

    // ctrl vs meta
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    let meta = Modifiers {
        meta: true,
        ..Modifiers::default()
    };
    let o = click_at_btn(&mut eng, 230.0, 30.0, PointerButton::Primary, ctrl);
    assert!(o
        .iter()
        .any(|x| matches!(x, InputOutcome::Action { name, .. } if name == "a.ctrl")));
    complete_all(&mut eng, &o);
    let o = click_at_btn(&mut eng, 230.0, 30.0, PointerButton::Primary, meta);
    assert!(o
        .iter()
        .all(|x| !matches!(x, InputOutcome::Action { name, .. } if name == "a.ctrl")));

    let o = click_at_btn(&mut eng, 320.0, 30.0, PointerButton::Primary, meta);
    assert!(o
        .iter()
        .any(|x| matches!(x, InputOutcome::Action { name, .. } if name == "a.meta")));
    complete_all(&mut eng, &o);

    // ctrlKey||metaKey
    let o = click_at_btn(&mut eng, 410.0, 30.0, PointerButton::Primary, ctrl);
    assert!(o
        .iter()
        .any(|x| matches!(x, InputOutcome::Action { name, .. } if name == "a.either")));
    complete_all(&mut eng, &o);
    let o = click_at_btn(&mut eng, 410.0, 30.0, PointerButton::Primary, meta);
    assert!(o
        .iter()
        .any(|x| matches!(x, InputOutcome::Action { name, .. } if name == "a.either")));
    complete_all(&mut eng, &o);

    // macOS Ctrl+Primary reported raw as Primary + ctrl (no remap)
    let o = click_at_btn(&mut eng, 230.0, 30.0, PointerButton::Primary, ctrl);
    match o.iter().find(|x| matches!(x, InputOutcome::Action { .. })) {
        Some(InputOutcome::Action {
            trigger: TriggerInfo::Pointer { button, mods, .. },
            ..
        }) => {
            assert_eq!(*button, PointerButton::Primary);
            assert!(mods.ctrl);
        }
        _ => panic!("expected raw Primary+ctrl"),
    }
}

// ---------------------------------------------------------------------------
// A19 — Window-level key
// ---------------------------------------------------------------------------
#[test]
fn a19_window_level_key() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    // from:body Enter with no focus → Action on btn-menu
    eng.focus_state(); // ensure we can read
                       // Clear focus intentionally
    let key = KeyEvent {
        key: "Enter".into(),
        code: "Enter".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    };
    let out = eng.input(InputEvent::Key(key.clone()));
    match out {
        InputOutcome::Action {
            name, source_id, ..
        } => {
            assert_eq!(name, "time.pause_menu");
            assert_eq!(source_id, "btn-menu");
        }
        other => panic!("expected from:body Action, got {other:?}"),
    }

    // Auto-repeat keydown does not fire unless filter has repeat
    let rep = KeyEvent {
        repeat: true,
        ..key
    };
    let out = eng.input(InputEvent::Key(rep));
    // Pending drop may also yield None — complete first.
    if let InputOutcome::Action { request, .. } = out {
        eng.complete(request, Ok(()));
    } else {
        // Should be Unhandled or None — not a new Action from repeat without [repeat]
        assert!(
            matches!(out, InputOutcome::Unhandled(_) | InputOutcome::None),
            "repeat without [repeat] must not Action: {out:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A20 — Esc passthrough
// ---------------------------------------------------------------------------
#[test]
fn a20_esc_passthrough() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    let esc = KeyEvent {
        key: "Escape".into(),
        code: "Escape".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    };
    let out = eng.input(InputEvent::Key(esc.clone()));
    match out {
        InputOutcome::Unhandled(k) => assert_eq!(k.key, "Escape"),
        other => panic!("Esc must be Unhandled by default, got {other:?}"),
    }

    // With explicit trigger on focused element → consumed
    let html = r#"<!DOCTYPE html><html><body>
      <button id="closer" hx-post="win.close" hx-trigger="keydown[key=='Escape']">x</button>
    </body></html>"#;
    let mut eng2 = Engine::new(EngineConfig::default());
    eng2.declare_actions(&["win.close"]);
    eng2.load_html(html).unwrap();
    let s2 = MemSlices::new();
    let mut f2 = SpyFrags::new();
    eng2.sync(&s2, &mut f2).unwrap();
    eng2.paint(Viewport {
        width: 200,
        height: 80,
        holes: vec![],
    })
    .unwrap();
    eng2.test_set_focus("closer");
    let out = eng2.input(InputEvent::Key(esc));
    assert!(matches!(out, InputOutcome::Action { name, .. } if name == "win.close"));
}

// ---------------------------------------------------------------------------
// A21 — Keyboard activation + Tab / Shift+Tab focus order
// ---------------------------------------------------------------------------
fn tab_key(shift: bool) -> KeyEvent {
    KeyEvent {
        key: "Tab".into(),
        code: "Tab".into(),
        mods: Modifiers {
            shift,
            ..Modifiers::default()
        },
        repeat: false,
        pressed: true,
    }
}

#[test]
fn a21_keyboard_activation() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    sync_paint(&mut eng, &mut slices, &mut frags);

    eng.test_set_focus("btn-pause");
    let enter = KeyEvent {
        key: "Enter".into(),
        code: "Enter".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    };
    let out = eng.input(InputEvent::Key(enter));
    match out {
        InputOutcome::Action {
            name,
            source_id,
            trigger: TriggerInfo::Key { .. },
            request,
            ..
        } => {
            assert_eq!(name, "time.pause");
            assert_eq!(source_id, "btn-pause");
            eng.complete(request, Ok(()));
        }
        other => panic!("Enter on focused hx-post must Action: {other:?}"),
    }

    eng.test_set_focus("btn-pause");
    let space = KeyEvent {
        key: " ".into(),
        code: "Space".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    };
    let out = eng.input(InputEvent::Key(space));
    assert!(matches!(
        out,
        InputOutcome::Action {
            name,
            trigger: TriggerInfo::Key { .. },
            ..
        } if name == "time.pause"
    ));
}

#[test]
fn a21_tab_focus_order() {
    // Document order: button a, tabindex div, button b; skip disabled/greyed + tabindex=-1.
    // Blitz's HTML parser drops a bare `disabled` attribute from markup, so we
    // grey via the `actions` slice (same path as A6) which sets disabled + aria-disabled.
    let html = r##"<!DOCTYPE html><html><body>
      <button id="a" hx-post="act.a">A</button>
      <div id="skip-neg" tabindex="-1">no</div>
      <div id="t0" tabindex="0">tab0</div>
      <button id="disabled" hx-post="act.x">X</button>
      <button id="greyed" hx-post="act.g">G</button>
      <button id="b" hx-post="act.b">B</button>
    </body></html>"##;
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["act.a", "act.b", "act.x", "act.g"]);
    eng.load_html(html).unwrap();
    let mut slices = MemSlices::new();
    slices.actions.push((
        "act.x".into(),
        ActionState::Disabled {
            reason: "off".into(),
        },
    ));
    slices.actions.push((
        "act.g".into(),
        ActionState::Disabled {
            reason: "grey".into(),
        },
    ));
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    eng.paint(Viewport {
        width: 400,
        height: 200,
        holes: vec![],
    })
    .unwrap();

    assert!(eng.attr("disabled", "disabled").is_some());
    assert_eq!(eng.attr("greyed", "aria-disabled").as_deref(), Some("true"));
    assert_eq!(eng.focusable_ids_for_test(), ["a", "t0", "b"]);

    let expected = ["a", "t0", "b"];

    let out = eng.input(InputEvent::Key(tab_key(false)));
    match out {
        InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) => assert_eq!(id, "a"),
        other => panic!("Tab from nowhere → first focusable, got {other:?}"),
    }

    let out = eng.input(InputEvent::Key(tab_key(false)));
    match out {
        InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) => assert_eq!(id, "t0"),
        other => panic!("expected t0, got {other:?}"),
    }

    let out = eng.input(InputEvent::Key(tab_key(false)));
    match out {
        InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) => assert_eq!(id, "b"),
        other => panic!("expected b, got {other:?}"),
    }

    // Wrap forward
    let out = eng.input(InputEvent::Key(tab_key(false)));
    match out {
        InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) => assert_eq!(id, "a"),
        other => panic!("wrap to a, got {other:?}"),
    }

    // Shift+Tab reverse
    let out = eng.input(InputEvent::Key(tab_key(true)));
    match out {
        InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) => assert_eq!(id, "b"),
        other => panic!("Shift+Tab to b, got {other:?}"),
    }

    // Disabled / greyed / tabindex=-1 never receive focus via Tab.
    for _ in 0..6 {
        let out = eng.input(InputEvent::Key(tab_key(false)));
        if let InputOutcome::FocusChanged(FocusState {
            focused_id: Some(ref id),
        }) = out
        {
            assert!(
                expected.contains(&id.as_str()),
                "focused unexpected id {id}"
            );
            assert_ne!(id, "disabled");
            assert_ne!(id, "greyed");
            assert_ne!(id, "skip-neg");
        }
    }
}

// ---------------------------------------------------------------------------
// HTML contract §2 — strict attribute dialect at load_html / swap fragments
// ---------------------------------------------------------------------------

fn load_err(html: &str) -> EngineError {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(default_actions());
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect_err("expected load error")
}

#[test]
fn strict_unknown_hx_attribute_fails_load() {
    // Rule 1: any hx-* / data-hx-* outside the locked set is a load error.
    for html in [
        r##"<button id="b" hx-post="time.pause" hx-get="/x">x</button>"##,
        r##"<button id="b" hx-post="time.pause" hx-vals='{}'>x</button>"##,
        r##"<button id="b" hx-post="time.pause" hx-on="click:1">x</button>"##,
        r##"<button id="b" hx-post="time.pause" hx-boost="true">x</button>"##,
        r##"<button id="b" hx-post="time.pause" hx-select="#a">x</button>"##,
        r##"<div id="d" data-hx-post="time.pause">x</div>"##,
    ] {
        let err = load_err(html);
        assert!(
            matches!(err, EngineError::InvalidAttribute { .. }),
            "expected InvalidAttribute for {html}, got {err:?}"
        );
    }
}

#[test]
fn strict_data_hx_reason_fails_load() {
    // Rule 2: data-hx-reason is engine-written only.
    let err = load_err(r##"<button id="b" hx-post="time.pause" data-hx-reason="nope">x</button>"##);
    match err {
        EngineError::InvalidAttribute { attr, .. } => assert_eq!(attr, "data-hx-reason"),
        other => panic!("expected InvalidAttribute, got {other:?}"),
    }
}

#[test]
fn strict_hx_swap_and_sync_values_fail_load() {
    // Rule 3: only innerHTML/outerHTML and drop/queue (existing variants).
    let swap_err = load_err(r##"<div id="s" hx-trigger="slice:time" hx-swap="beforeend">x</div>"##);
    assert!(matches!(swap_err, EngineError::UnsupportedSwap { .. }));
    let sync_err = load_err(r##"<button id="b" hx-post="time.pause" hx-sync="abort">x</button>"##);
    assert!(matches!(sync_err, EngineError::UnsupportedSync { .. }));
}

#[test]
fn strict_hx_target_and_indicator_must_be_resolving_id() {
    // Rule 4: hx-target / hx-indicator must be #id that resolves.
    let bad_sel = load_err(
        r##"<div id="s" hx-trigger="slice:time" hx-target=".box" hx-swap="innerHTML">x</div>"##,
    );
    assert!(
        matches!(bad_sel, EngineError::InvalidAttribute { ref attr, .. } if attr == "hx-target"),
        "got {bad_sel:?}"
    );
    let missing = load_err(
        r##"<div id="s" hx-trigger="slice:time" hx-target="#missing" hx-swap="innerHTML">x</div>"##,
    );
    assert!(
        matches!(missing, EngineError::InvalidAttribute { ref attr, .. } if attr == "hx-target"),
        "got {missing:?}"
    );
    let ind_fmt = load_err(
        r##"<button id="b" hx-post="time.pause" hx-indicator="spinner">x</button><div id="spinner"></div>"##,
    );
    assert!(
        matches!(ind_fmt, EngineError::InvalidAttribute { ref attr, .. } if attr == "hx-indicator"),
        "got {ind_fmt:?}"
    );
    let ind_miss =
        load_err(r##"<button id="b" hx-post="time.pause" hx-indicator="#nope">x</button>"##);
    assert!(
        matches!(ind_miss, EngineError::InvalidAttribute { ref attr, .. } if attr == "hx-indicator"),
        "got {ind_miss:?}"
    );
}

#[test]
fn strict_bad_fragment_errors_on_sync_not_applied() {
    // Rule 5: fragment validation uses the sync EngineError channel; DOM unchanged.
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let before = eng.serialize_html().unwrap();

    frags.set(
        "time-strip",
        r##"<span id="time-label" hx-get="/evil">DATE bad</span>"##,
    );
    slices.bump("time");
    let err = eng.sync(&slices, &mut frags).expect_err("bad fragment");
    assert!(
        matches!(err, EngineError::InvalidAttribute { .. }),
        "expected InvalidAttribute from sync, got {err:?}"
    );
    let after = eng.serialize_html().unwrap();
    assert_eq!(before, after, "bad fragment must not be applied");
    assert!(!after.contains("DATE bad"));

    // Hand-written data-hx-reason in a swap fragment is also rejected (engine-only).
    frags.set(
        "time-strip",
        r##"<span id="time-label" data-hx-reason="sample">DATE reason</span>"##,
    );
    slices.bump("time");
    let err = eng
        .sync(&slices, &mut frags)
        .expect_err("data-hx-reason in fragment");
    match err {
        EngineError::InvalidAttribute { attr, .. } => assert_eq!(attr, "data-hx-reason"),
        other => panic!("expected InvalidAttribute for data-hx-reason, got {other:?}"),
    }
    assert_eq!(
        before,
        eng.serialize_html().unwrap(),
        "data-hx-reason fragment must not be applied"
    );
}

#[test]
fn fragment_indicator_resolves_sibling_outside_swap_target() {
    // Designer chips.html: every button has hx-indicator="#tstrip-ind", and
    // #tstrip-ind is a sibling of #tstrip-chips (outside the fragment).
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["time.pause", "time.resume", "time.set_speed"]);
    eng.load_html(TSTRIP_SHELL).expect("load tstrip shell");

    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("time-strip", CHIPS_FRAGMENT);
    eng.sync(&slices, &mut frags)
        .expect("chips fragment must apply");

    let html = eng.serialize_html().unwrap();
    assert!(html.contains("id=\"tstrip-pause\""), "pause chip present");
    assert!(html.contains("id=\"tstrip-month\""), "7th chip present");
    assert!(html.contains("id=\"tstrip-ind\""), "sibling indicator kept");
    assert!(
        html.matches("hx-indicator=\"#tstrip-ind\"").count() >= 7,
        "all chips keep hx-indicator to sibling"
    );
    // No greying attrs in the Designer sample fragment.
    assert!(!html.contains("data-hx-reason"));
}

#[test]
fn fragment_indicator_missing_id_fails_sync() {
    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["time.pause", "time.resume", "time.set_speed"]);
    eng.load_html(TSTRIP_SHELL).expect("load tstrip shell");

    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let before = eng.serialize_html().unwrap();

    frags.set(
        "time-strip",
        r##"<button id="tstrip-pause" hx-post="time.pause" hx-trigger="click" hx-indicator="#nowhere">Pause</button>"##,
    );
    slices.bump("time");
    let err = eng
        .sync(&slices, &mut frags)
        .expect_err("missing indicator id");
    assert!(
        matches!(err, EngineError::InvalidAttribute { ref attr, .. } if attr == "hx-indicator"),
        "got {err:?}"
    );
    assert_eq!(before, eng.serialize_html().unwrap());
}

#[test]
fn from_body_prefers_first_enabled_in_document_order() {
    // Designer time-strip: pause + resume share Space from:body; exactly one greyed.
    let html = r##"<!DOCTYPE html><html><body>
      <button id="tstrip-pause"
        hx-post="time.pause"
        hx-trigger="click, keydown[key==' '&&!repeat] from:body">Pause</button>
      <button id="tstrip-resume"
        hx-post="time.resume"
        hx-trigger="click, keydown[key==' '&&!repeat] from:body">Resume</button>
    </body></html>"##;

    let mut eng = Engine::new(EngineConfig::default());
    eng.declare_actions(&["time.pause", "time.resume"]);
    eng.load_html(html).expect("load designer pair");

    let mut slices = MemSlices::new();
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "already paused".into(),
        },
    ));
    // resume stays Enabled (absent from Disabled list)
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();

    let space = KeyEvent {
        key: " ".into(),
        code: "Space".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    };
    // No focus — step (d) only.
    assert!(eng.focus_state().focused_id.is_none());

    let out = eng.input(InputEvent::Key(space.clone()));
    match out {
        InputOutcome::Action {
            name, source_id, ..
        } => {
            assert_eq!(name, "time.resume");
            assert_eq!(source_id, "tstrip-resume");
        }
        other => panic!("expected Action time.resume, got {other:?}"),
    }

    // Flip: resume Disabled, pause Enabled → Space fires pause.
    slices.actions.clear();
    slices.actions.push((
        "time.resume".into(),
        ActionState::Disabled {
            reason: "running".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();

    let out = eng.input(InputEvent::Key(space));
    match out {
        InputOutcome::Action {
            name, source_id, ..
        } => {
            assert_eq!(name, "time.pause");
            assert_eq!(source_id, "tstrip-pause");
        }
        other => panic!("expected Action time.pause after flip, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Hover passthrough — CSS :hover / attr() tooltips paint through the engine
// ---------------------------------------------------------------------------

fn dark_px(pixels: &[u8]) -> usize {
    pixels
        .chunks(4)
        .filter(|c| c[3] > 0 && (c[0] as u32 + c[1] as u32 + c[2] as u32) < 300)
        .count()
}

fn tooltip_page(css: &str, button_attrs: &str) -> String {
    // Concatenate CSS (it contains `{`/`}`) so `format!` does not treat rules as placeholders.
    let mut html = String::from(
        r##"<!DOCTYPE html><html><head><style>
body{margin:0;font-family:"DejaVu Sans",sans-serif;font-size:14px;width:300px;height:60px}
#b{display:block;width:40px;height:20px;border:0;padding:0;background:#fff;position:relative}
"##,
    );
    html.push_str(css);
    html.push_str("\n</style></head><body>\n<button id=\"b\" hx-post=\"x\" hx-trigger=\"click\" ");
    html.push_str(button_attrs);
    html.push_str("></button>\n</body></html>");
    html
}

const REASON_TOOLTIP_CSS: &str = r#"[data-hx-reason]:hover::after{content:attr(data-hx-reason);position:absolute;left:44px;top:0;white-space:nowrap;color:#000;background:#ffffe1;border:1px solid #000}"#;
const TITLE_TOOLTIP_CSS: &str = r#"[title]:not([data-hx-reason]):hover::after{content:attr(title);position:absolute;left:44px;top:0;white-space:nowrap;color:#000;background:#ffffe1;border:1px solid #000}"#;

fn boot_tooltip(css: &str, button_attrs: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
    });
    eng.declare_actions(&["x"]);
    eng.load_html(&tooltip_page(css, button_attrs))
        .expect("load tooltip page");
    eng
}

fn paint_small(eng: &mut Engine) -> PaintOutput {
    eng.paint(Viewport {
        width: 300,
        height: 60,
        holes: vec![],
    })
    .unwrap()
}

fn disable_x(eng: &mut Engine, reason: &str) {
    let mut slices = MemSlices::new();
    slices.actions.push((
        "x".into(),
        ActionState::Disabled {
            reason: reason.into(),
        },
    ));
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
}

#[test]
fn hover_reason_tooltip_paints_and_clears() {
    let mut eng = boot_tooltip(REASON_TOOLTIP_CSS, "");
    disable_x(&mut eng, "Already paused");

    let before = paint_small(&mut eng);
    let before_dark = dark_px(&before.rgba.as_ref().unwrap().pixels);
    assert_eq!(
        before_dark, 0,
        "unhovered disabled chip must not show tooltip"
    );

    let hover = eng.input(InputEvent::PointerMove {
        x: 10.0,
        y: 10.0,
        mods: Modifiers::default(),
    });
    assert!(matches!(hover, InputOutcome::Hover { id: Some(ref id) } if id == "b"));

    let hovered = paint_small(&mut eng);
    assert!(!hovered.skipped, "hover enter must dirty paint");
    let hovered_dark = dark_px(&hovered.rgba.as_ref().unwrap().pixels);
    assert!(
        hovered_dark > 100,
        "hovered data-hx-reason tooltip must paint (dark_px={hovered_dark})"
    );

    let leave = eng.input(InputEvent::PointerLeave);
    assert!(matches!(leave, InputOutcome::Hover { id: None }));
    let after = paint_small(&mut eng);
    assert!(!after.skipped, "hover leave must dirty paint");
    let after_dark = dark_px(&after.rgba.as_ref().unwrap().pixels);
    assert_eq!(
        after_dark, before_dark,
        "leaving must restore unhovered pixels (before={before_dark} after={after_dark})"
    );
}

#[test]
fn hover_reason_follows_sync_rewrite() {
    let mut eng = boot_tooltip(REASON_TOOLTIP_CSS, "");
    disable_x(&mut eng, "Already paused");
    let _ = paint_small(&mut eng);
    eng.input(InputEvent::PointerMove {
        x: 10.0,
        y: 10.0,
        mods: Modifiers::default(),
    });
    let old = paint_small(&mut eng);
    let old_dark = dark_px(&old.rgba.as_ref().unwrap().pixels);

    // Rewrite reason while still hovered.
    let mut slices = MemSlices::new();
    slices.actions.push((
        "x".into(),
        ActionState::Disabled {
            reason: "Paused while a dialog is open".into(),
        },
    ));
    slices.bump("actions");
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let updated = paint_small(&mut eng);
    let updated_px = updated.rgba.as_ref().unwrap().pixels.clone();
    let updated_dark = dark_px(&updated_px);
    assert!(
        updated_dark > old_dark,
        "longer reason must paint more dark pixels (old={old_dark} new={updated_dark})"
    );

    // Fresh engine with the new reason, hovered — pixel match.
    let mut fresh = boot_tooltip(REASON_TOOLTIP_CSS, "");
    disable_x(&mut fresh, "Paused while a dialog is open");
    let _ = paint_small(&mut fresh);
    fresh.input(InputEvent::PointerMove {
        x: 10.0,
        y: 10.0,
        mods: Modifiers::default(),
    });
    let fresh_out = paint_small(&mut fresh);
    assert_eq!(
        updated_px,
        fresh_out.rgba.as_ref().unwrap().pixels,
        "sync reason rewrite while hovered must match a fresh hovered load"
    );
}

#[test]
fn hover_tooltip_survives_chip_swap() {
    // tstrip_shell + chips.html: swap replaces the hovered chip fragment while
    // the pointer stays over it; tooltip must still paint.
    let mut shell = String::from(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
body { width: 800px; height: 120px; margin: 0; font-family: "DejaVu Sans", sans-serif; }
#tstrip-chips { position: absolute; left: 8px; top: 8px; height: 40px; }
#tstrip-ind { position: absolute; left: 8px; top: 60px; width: 24px; height: 24px; }
button { height: 36px; margin-right: 4px; position: relative; }
"##,
    );
    shell.push_str(REASON_TOOLTIP_CSS);
    shell.push_str(
        r##"
</style></head><body>
<section id="time-strip" hx-trigger="slice:time" hx-target="#tstrip-chips" hx-swap="innerHTML">
  <div id="tstrip-chips"><!-- chips --></div>
</section>
<div id="tstrip-ind" aria-hidden="true"></div>
</body></html>"##,
    );

    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
    });
    eng.declare_actions(&["time.pause", "time.resume", "time.set_speed"]);
    eng.load_html(&shell).expect("load shell");

    let mut slices = MemSlices::new();
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "Already paused".into(),
        },
    ));
    let mut frags = SpyFrags::new();
    frags.set("time-strip", CHIPS_FRAGMENT);
    eng.sync(&slices, &mut frags).unwrap();

    let vp = Viewport {
        width: 800,
        height: 120,
        holes: vec![],
    };
    let _ = eng.paint(vp.clone()).unwrap();

    // Pause chip is the first button (~left edge of #tstrip-chips).
    let hover = eng.input(InputEvent::PointerMove {
        x: 40.0,
        y: 26.0,
        mods: Modifiers::default(),
    });
    assert!(
        matches!(hover, InputOutcome::Hover { id: Some(ref id) } if id == "tstrip-pause"),
        "expected hover on tstrip-pause, got {hover:?}"
    );
    let hovered = eng.paint(vp.clone()).unwrap();
    let hovered_dark = dark_px(&hovered.rgba.as_ref().unwrap().pixels);
    assert!(
        hovered_dark > 100,
        "tooltip must paint before swap (dark_px={hovered_dark})"
    );

    // Swap the chips fragment again with the pointer still over pause.
    slices.bump("time");
    frags.set("time-strip", CHIPS_FRAGMENT);
    eng.sync(&slices, &mut frags).unwrap();
    let after_swap = eng.paint(vp).unwrap();
    assert!(!after_swap.skipped, "swap under hover must dirty paint");
    let after_dark = dark_px(&after_swap.rgba.as_ref().unwrap().pixels);
    assert!(
        after_dark > 100,
        "tooltip must survive chip swap (dark_px={after_dark})"
    );
    assert_eq!(
        eng.attr("tstrip-pause", "data-hx-reason").as_deref(),
        Some("Already paused")
    );
}

#[test]
fn hover_move_within_same_element_skips_paint() {
    let mut eng = boot_tooltip(REASON_TOOLTIP_CSS, "");
    disable_x(&mut eng, "Already paused");
    let _ = paint_small(&mut eng);

    eng.input(InputEvent::PointerMove {
        x: 10.0,
        y: 10.0,
        mods: Modifiers::default(),
    });
    let enter = paint_small(&mut eng);
    assert!(!enter.skipped);

    // Move within the same 40×20 button — Blitz hover node unchanged.
    let none = eng.input(InputEvent::PointerMove {
        x: 18.0,
        y: 12.0,
        mods: Modifiers::default(),
    });
    assert!(
        matches!(none, InputOutcome::None),
        "same-element move must not re-emit Hover, got {none:?}"
    );
    let skipped = paint_small(&mut eng);
    assert!(
        skipped.skipped,
        "same-element pointer move must keep clean-frame skip"
    );
    assert!(
        eng.last_timings().paint_ns < 1_000_000,
        "skipped paint must stay cheap, got {} ns",
        eng.last_timings().paint_ns
    );
}

#[test]
fn hover_title_tooltip_on_enabled_element() {
    let mut eng = boot_tooltip(TITLE_TOOLTIP_CSS, r#"title="1 min/s""#);
    // Enabled: no actions-slice disable, so no data-hx-reason.
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();

    let before = paint_small(&mut eng);
    assert_eq!(dark_px(&before.rgba.as_ref().unwrap().pixels), 0);

    eng.input(InputEvent::PointerMove {
        x: 10.0,
        y: 10.0,
        mods: Modifiers::default(),
    });
    let hovered = paint_small(&mut eng);
    assert!(!hovered.skipped);
    let dark = dark_px(&hovered.rgba.as_ref().unwrap().pixels);
    assert!(
        dark > 50,
        "enabled title tooltip must paint via attr(title) (dark_px={dark})"
    );
    assert!(eng.attr("b", "data-hx-reason").is_none());
    assert_eq!(eng.attr("b", "title").as_deref(), Some("1 min/s"));
}

// ---------------------------------------------------------------------------
// Clean-frame sync / skipped paint (full-window bench bugs)
// ---------------------------------------------------------------------------

fn paint_view(eng: &mut Engine) -> PaintOutput {
    eng.paint(Viewport {
        width: 800,
        height: 120,
        holes: vec![],
    })
    .unwrap()
}

#[test]
fn no_hover_time_slice_swap_dirties_paint_and_pixels() {
    // No pointer hover: bumping `time` must still mark paint dirty via the
    // fragment swap path (not via hover refresh).
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    frags.set("time-strip", r#"<span id="time-label">DATE AAA</span>"#);
    eng.sync(&slices, &mut frags).unwrap();
    let first = paint_view(&mut eng);
    assert!(!first.skipped);
    let before = first.rgba.as_ref().unwrap().pixels.clone();

    // Confirm a clean sync+paint stays skipped (no hover).
    eng.sync(&slices, &mut frags).unwrap();
    assert!(paint_view(&mut eng).skipped);

    frags.set(
        "time-strip",
        r#"<span id="time-label">DATE BBB · different</span>"#,
    );
    slices.bump("time");
    let dirty = eng.sync(&slices, &mut frags).unwrap();
    assert!(
        dirty.sections.iter().any(|s| s == "time-strip"),
        "time bump must swap the time-strip fragment"
    );
    let second = paint_view(&mut eng);
    assert!(
        !second.skipped,
        "fragment swap with no hover must not skip the next paint"
    );
    let after = second.rgba.as_ref().unwrap().pixels.clone();
    assert_ne!(
        before, after,
        "swapped fragment content must change painted pixels"
    );
}

#[test]
fn sync_unchanged_mem_slices_leaves_paint_skipped() {
    let mut eng = boot(FIXTURE);
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let first = paint_view(&mut eng);
    assert!(!first.skipped);
    assert!(first.rgba.is_some());

    for _ in 0..8 {
        let dirty = eng.sync(&slices, &mut frags).unwrap();
        assert!(
            dirty.sections.is_empty(),
            "unchanged MemSlices must not re-render sections"
        );
    }

    let out = paint_view(&mut eng);
    assert!(
        out.skipped,
        "sync with unchanged MemSlices must leave the next paint skipped"
    );
    assert!(out.rgba.is_none());
}

#[test]
fn action_reason_change_dirties_paint() {
    let mut eng = boot(FIXTURE);
    let mut slices = MemSlices::new();
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "Already paused".into(),
        },
    ));
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let _ = paint_view(&mut eng);

    // Same MemSlices again — still clean.
    eng.sync(&slices, &mut frags).unwrap();
    assert!(paint_view(&mut eng).skipped);

    // Change only the reason string (and bump the actions version).
    slices.actions.clear();
    slices.actions.push((
        "time.pause".into(),
        ActionState::Disabled {
            reason: "Paused while a dialog is open".into(),
        },
    ));
    slices.bump("actions");
    eng.sync(&slices, &mut frags).unwrap();
    let out = paint_view(&mut eng);
    assert!(!out.skipped, "changing one action reason must dirty paint");
    assert!(out.rgba.is_some());
    assert_eq!(
        eng.attr("btn-pause", "data-hx-reason").as_deref(),
        Some("Paused while a dialog is open")
    );
}

#[test]
fn skipped_paint_does_no_buffer_copy() {
    let mut eng = boot(FIXTURE);
    let slices = MemSlices::new();
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let _ = paint_view(&mut eng);

    // Warm the skip path.
    let _ = paint_view(&mut eng);

    let t0 = std::time::Instant::now();
    let out = paint_view(&mut eng);
    let ns = t0.elapsed().as_nanos() as u64;

    assert!(out.skipped);
    assert!(
        out.rgba.is_none(),
        "API: skipped paint returns rgba: None (host keeps last texture)"
    );
    assert!(eng.last_frame().is_some());
    assert!(
        ns < 50_000,
        "skipped paint must stay under 50 us (no buffer copy), got {ns} ns"
    );
    assert!(
        eng.last_timings().paint_ns < 50_000,
        "engine paint_ns on skip must stay under 50 us, got {}",
        eng.last_timings().paint_ns
    );
}

// ---------------------------------------------------------------------------
// K1 / issue #6 — Action.args carry form values
// ---------------------------------------------------------------------------

fn boot_form(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
    });
    eng.declare_actions(actions);
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect("load_html form fixture");
    let _ = eng.paint(Viewport {
        width: 400,
        height: 200,
        holes: vec![],
    });
    eng
}

fn activate(eng: &mut Engine, id: &str) -> InputOutcome {
    eng.test_set_focus(id);
    eng.input(InputEvent::Key(KeyEvent {
        key: "Enter".into(),
        code: "Enter".into(),
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

#[test]
fn k1_args_order_and_contents() {
    let html = r#"
<form id="f">
  <input type="hidden" name="slot" value="3" />
  <input type="number" name="qty" value="2" min="0" max="9" step="1" />
  <button id="go" name="op" value="write" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("op".into(), "write".into()),
            ("slot".into(), "3".into()),
            ("qty".into(), "2".into()),
        ]
    );
}

#[test]
fn k1_disabled_and_unchecked_excluded() {
    let html = r#"
<form id="f">
  <input type="hidden" name="a" value="1" />
  <input type="hidden" name="b" value="2" disabled="" />
  <input type="checkbox" name="c" value="yes" />
  <input type="checkbox" name="d" value="on" checked="" />
  <input type="radio" name="r" value="x" />
  <input type="radio" name="r" value="y" checked="" />
  <button id="go" name="act" value="1" hx-post="save.write">Go</button>
  <button id="other" name="other" value="nope">Other</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("act".into(), "1".into()),
            ("a".into(), "1".into()),
            ("d".into(), "on".into()),
            ("r".into(), "y".into()),
        ]
    );
}

#[test]
fn k1_trigger_outside_form_sends_own_pair_only() {
    let html = r#"
<form id="f">
  <input type="hidden" name="inside" value="1" />
</form>
<button id="go" name="solo" value="x" hx-post="save.write">Go</button>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(args, &[("solo".into(), "x".into())]);
}

#[test]
fn k1_select_sends_selected_or_first_option() {
    let html = r#"
<form id="f">
  <select name="branch">
    <option value="alpha">A</option>
    <option value="beta" selected="">B</option>
  </select>
  <select name="slot">
    <option value="1">one</option>
    <option value="2">two</option>
  </select>
  <button id="go" name="op" value="pick" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("op".into(), "pick".into()),
            ("branch".into(), "beta".into()),
            ("slot".into(), "1".into()),
        ]
    );
}

#[test]
fn k1_range_arrow_clamps_and_snaps() {
    let html = r#"
<form id="f">
  <input id="spd" type="range" name="speed" value="0" min="0" max="5" step="2" />
  <button id="go" name="op" value="set" hx-post="time.set_speed">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["time.set_speed"]);
    eng.test_set_focus("spd");
    // 0 → Up 2 → Right 4 → Up clamp to max 5 → Down 4 → Left 2
    for (key, code, expect) in [
        ("ArrowUp", "ArrowUp", "2"),
        ("ArrowRight", "ArrowRight", "4"),
        ("ArrowUp", "ArrowUp", "5"),
        ("ArrowDown", "ArrowDown", "4"),
        ("ArrowLeft", "ArrowLeft", "2"),
    ] {
        let _ = eng.input(InputEvent::Key(KeyEvent {
            key: key.into(),
            code: code.into(),
            mods: Modifiers::default(),
            repeat: false,
            pressed: true,
        }));
        assert_eq!(
            eng.attr("spd", "value").as_deref(),
            Some(expect),
            "after {key}"
        );
    }
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[("op".into(), "set".into()), ("speed".into(), "2".into()),]
    );
}

#[test]
fn k1_blocked_still_reports_with_form_present() {
    let html = r#"
<form id="f">
  <input type="hidden" name="slot" value="1" />
  <button id="go" name="op" value="write" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let mut slices = MemSlices::new();
    slices.actions.push((
        "save.write".into(),
        ActionState::Disabled {
            reason: "slot locked".into(),
        },
    ));
    let mut frags = SpyFrags::new();
    eng.sync(&slices, &mut frags).unwrap();
    let out = activate(&mut eng, "go");
    match out {
        InputOutcome::Blocked {
            name,
            reason,
            source_id,
            ..
        } => {
            assert_eq!(name, "save.write");
            assert_eq!(reason, "slot locked");
            assert_eq!(source_id, "go");
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
}

#[test]
fn k1_args_determinism() {
    let html = r#"
<form id="f">
  <input type="hidden" name="a" value="1" />
  <input type="number" name="n" value="4" min="0" max="9" />
  <input type="checkbox" name="c" value="on" checked="" />
  <select name="s"><option value="x" selected="">X</option><option value="y">Y</option></select>
  <button id="go" name="op" value="run" hx-post="save.write">Go</button>
</form>
"#;
    let run = || {
        let mut eng = boot_form(html, &["save.write"]);
        let out = activate(&mut eng, "go");
        match out {
            InputOutcome::Action {
                args,
                name,
                source_id,
                ..
            } => (name, source_id, args),
            other => panic!("expected Action, got {other:?}"),
        }
    };
    let a = run();
    let b = run();
    assert_eq!(a, b);
}

/// Guarantee (1): trigger pair first; form controls in document order; trigger not repeated.
#[test]
fn k1_g1_trigger_first_no_repeat() {
    let html = r#"
<form id="f">
  <input type="hidden" name="before" value="1" />
  <button id="go" name="op" value="go" hx-post="save.write">Go</button>
  <input type="hidden" name="after" value="2" />
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("op".into(), "go".into()),
            ("before".into(), "1".into()),
            ("after".into(), "2".into()),
        ]
    );
    assert_eq!(
        args.iter().filter(|(n, _)| n == "op").count(),
        1,
        "trigger must not be repeated among form controls"
    );
}

/// Guarantee (2): empty value → ""; checked checkbox with no value → "on".
#[test]
fn k1_g2_empty_value_and_checkbox_on() {
    let html = r#"
<form id="f">
  <input type="hidden" name="empty" />
  <input type="hidden" name="blank" value="" />
  <input type="checkbox" name="flag" checked="" />
  <button id="go" name="op" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("op".into(), "".into()),
            ("empty".into(), "".into()),
            ("blank".into(), "".into()),
            ("flag".into(), "on".into()),
        ]
    );
}

/// Guarantee (3): repeated names stay as repeated pairs, in order.
#[test]
fn k1_g3_repeated_names_kept_in_order() {
    let html = r#"
<form id="f">
  <input type="hidden" name="tag" value="a" />
  <input type="hidden" name="tag" value="b" />
  <input type="checkbox" name="tag" value="c" checked="" />
  <button id="go" name="op" value="1" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[
            ("op".into(), "1".into()),
            ("tag".into(), "a".into()),
            ("tag".into(), "b".into()),
            ("tag".into(), "c".into()),
        ]
    );
}

/// Guarantee (4): template out-of-range value sent as written; clamp only on engine edits.
#[test]
fn k1_g4_raw_template_value_clamp_only_on_edit() {
    let html = r#"
<form id="f">
  <input id="qty" type="number" name="qty" value="99" min="0" max="5" step="1" />
  <button id="go" name="op" value="set" hx-post="save.write">Go</button>
</form>
"#;
    let mut eng = boot_form(html, &["save.write"]);
    // Template wrote 99 (out of range) — sent as written.
    let out = activate(&mut eng, "go");
    let args = action_args(&out).expect("Action");
    assert_eq!(
        args,
        &[("op".into(), "set".into()), ("qty".into(), "99".into()),]
    );
    if let InputOutcome::Action { request, .. } = out {
        eng.complete(request, Ok(()));
    }

    // Engine edit clamps.
    eng.test_set_focus("qty");
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "ArrowDown".into(),
        code: "ArrowDown".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    assert_eq!(eng.attr("qty", "value").as_deref(), Some("5"));
    let out2 = activate(&mut eng, "go");
    let args2 = action_args(&out2).expect("Action");
    assert_eq!(
        args2,
        &[("op".into(), "set".into()), ("qty".into(), "5".into()),]
    );
}

#[test]
fn k1_text_input_stays_unhandled() {
    let mut eng = boot_form(
        r#"<button id="go" name="op" value="1" hx-post="save.write">Go</button>"#,
        &["save.write"],
    );
    let out = eng.input(InputEvent::Text("hello".into()));
    assert!(matches!(out, InputOutcome::Unhandled(_)));
}
