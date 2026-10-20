//! Issue #24 / #25 (Graph follow-up): pixel-level `PaintWorker` parity.
//!
//! The existing worker parity tests (`greying.rs`) only compare the order
//! of `InputOutcome::Action` / `Blocked` results — they would not notice a
//! worker that computed the right click outcome but never actually painted
//! the greying / `hx-changed` visual change. These tests drive the same
//! input sequence through `PaintWorker` and the sync `Engine` and assert the
//! RGBA frames are byte-identical at each checkpoint, and that the sync
//! side also matches a forced full repaint of the same end state.

use std::collections::HashMap;
use std::time::Duration;

use hx_native::*;

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    }
}

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

/// Visible CSS so greying / `hx-changed` produce a real pixel difference —
/// Blitz does not dim a disabled control by default, so without this a
/// pixel-identity bug could hide behind "nothing visually changed anyway".
const PIXEL_CSS: &str = "<style>\
button:disabled{background:#cc0000;color:#ffffff}\
input:disabled{background:#888888}\
.hx-changed{outline:3px solid #00aa00;outline-offset:1px;background:#ffffdd}\
</style>";

fn center(eng: &Engine, id: &str) -> (f32, f32) {
    let b = eng.test_element_box(id).expect(id);
    (b.0 + b.2 * 0.5, b.1 + b.3 * 0.5)
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

fn base_packet() -> SyncPacket {
    SyncPacket {
        versions: HashMap::from([("actions".into(), 1)]),
        actions: Vec::new(),
        input_rules: Vec::new(),
        fragments: HashMap::new(),
    }
}

fn probe(html: &str, actions: &[&str]) -> Engine {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(actions);
    eng.load_html(html).unwrap();
    let _ = eng.paint(vp());
    eng
}

/// Crop `img` to the device-pixel box `(x, y, w, h)`, row-major RGBA bytes.
/// Used to compare *just* one control's region — comparing whole frames
/// would trivially differ whenever a click's own control (e.g. a checkbox's
/// native tick mark) repaints, masking a mutation that specifically drops
/// some *other* control's greying.
fn region_pixels(img: &RgbaImage, x: f32, y: f32, w: f32, h: f32) -> Vec<u8> {
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = (x + w).min(img.width as f32) as u32;
    let y1 = (y + h).min(img.height as f32) as u32;
    let mut out = Vec::new();
    for yy in y0..y1 {
        for xx in x0..x1 {
            let i = ((yy * img.width + xx) * 4) as usize;
            out.extend_from_slice(&img.pixels[i..i + 4]);
        }
    }
    out
}

/// Run `steps` (each a batch of `InputEvent`s for one host-driven "frame")
/// through a worker and a sync `Engine` booted identically from `html` with
/// `pkt` already applied. Paints both after each step and captures the
/// full RGBA frame. Returns the pre-`steps` baseline frame (ungreyed —
/// callers use this to prove a checkpoint's greying is actually *visible*,
/// not just identical on both sides because neither side applied it) plus
/// one frame per step for each side.
fn run_pixel_checkpoints(
    html: &str,
    actions: &[&str],
    pkt: SyncPacket,
    steps: &[Vec<InputEvent>],
) -> (RgbaImage, Vec<RgbaImage>, Vec<RgbaImage>) {
    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(actions);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(vp());
    let mut slices = pkt.clone();
    let mut frags = pkt.clone();
    sync_eng.sync(&slices, &mut frags).unwrap();
    let _ = &mut slices;
    let _ = sync_eng.paint(vp());
    let baseline = sync_eng.last_frame().expect("baseline frame").clone();

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(actions).unwrap();
    worker.load_html(html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, vp()).unwrap();
    worker.sync(pkt).unwrap();
    // 10s, not flush()'s default 100ms — see the comment on the analogous
    // call in greying.rs's run_parity_script (PR #24 round 2 CI gate fix).
    worker.flush_timeout(Duration::from_secs(10)).unwrap();

    let mut sync_frames = Vec::new();
    let mut worker_frames = Vec::new();
    for step in steps {
        for ev in step {
            let _ = sync_eng.input(ev.clone());
            worker.input(ev.clone()).unwrap();
        }
        let sync_out = sync_eng.paint(vp()).unwrap();
        worker.flush_timeout(Duration::from_secs(10)).unwrap();
        let worker_frame = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .expect("worker frame");

        // A clean (skipped) paint carries no `rgba`; fall back to the
        // cached last frame, same image either way.
        let sync_img = sync_out
            .rgba
            .unwrap_or_else(|| sync_eng.last_frame().expect("sync frame").clone());
        sync_frames.push(sync_img);
        worker_frames.push(
            worker_frame
                .output
                .rgba
                .as_ref()
                .expect("worker rgba")
                .clone(),
        );
    }
    (baseline, worker_frames, sync_frames)
}

/// Replay `steps` against a fresh sync `Engine`, force a full repaint at the
/// end, and return that frame — the "ground truth" a correctly-scoped
/// partial repaint must match byte-for-byte.
fn full_repaint_frame(
    html: &str,
    actions: &[&str],
    pkt: SyncPacket,
    steps: &[Vec<InputEvent>],
) -> RgbaImage {
    let mut eng = Engine::new(cfg());
    eng.declare_actions(actions);
    eng.load_html(html).unwrap();
    let _ = eng.paint(vp());
    let mut slices = pkt.clone();
    let mut frags = pkt;
    eng.sync(&slices, &mut frags).unwrap();
    let _ = &mut slices;
    let _ = eng.paint(vp());
    for step in steps {
        for ev in step {
            let _ = eng.input(ev.clone());
        }
    }
    eng.mark_theme_or_stylesheet_changed();
    eng.paint(vp()).unwrap().rgba.expect("full repaint rgba")
}

fn assert_all_checkpoints_match(worker_frames: &[RgbaImage], sync_frames: &[RgbaImage]) {
    assert_eq!(worker_frames.len(), sync_frames.len());
    for (i, (w, s)) in worker_frames.iter().zip(sync_frames.iter()).enumerate() {
        assert_eq!(
            w.pixels, s.pixels,
            "checkpoint {i}: worker vs sync Engine must be byte-identical"
        );
    }
}

// ---------------------------------------------------------------------------
// RequiresChange: Apply grey/ungrey.
// ---------------------------------------------------------------------------

fn requires_change_form_html() -> String {
    format!(
        "<!DOCTYPE html><html><head>{PIXEL_CSS}</head><body style=\"margin:0;width:400px;height:200px;background:#fff\">\
<form id=\"f\">\
<input id=\"music\" type=\"checkbox\" name=\"music\" style=\"position:absolute;left:10px;top:10px\"/>\
<button id=\"apply\" name=\"apply\" hx-post=\"settings.apply\" hx-trigger=\"click\" \
style=\"position:absolute;left:10px;top:50px;width:80px;height:30px\">Apply</button>\
</form></body></html>"
    )
}

#[test]
fn pixel_parity_requires_change_apply_grey_ungrey() {
    let html = requires_change_form_html();
    let actions = ["settings.apply"];
    let probe_eng = probe(&html, &actions);
    let (mx, my) = center(&probe_eng, "music");

    let mut pkt = base_packet();
    pkt.actions = vec![(
        "settings.apply".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: "No changes".into(),
        },
    )];
    pkt.versions.insert("actions".into(), 2);

    let steps = vec![
        click_script(mx, my), // ungreys Apply
        click_script(mx, my), // reverts; regreys Apply
    ];

    let (baseline, worker_frames, sync_frames) =
        run_pixel_checkpoints(&html, &actions, pkt.clone(), &steps);
    assert_all_checkpoints_match(&worker_frames, &sync_frames);
    // Test gap (Graph 18:55 re-review): a worker/sync diff of 0 also passes
    // vacuously if neither side ever applied the ungrey at all — and a
    // whole-frame diff against the baseline would trivially pass anyway
    // since clicking `music` repaints its own tick mark regardless of
    // Apply's greying. Crop to *just* Apply's own region to isolate it.
    let (ax, ay, aw, ah) = probe_eng.test_element_box("apply").unwrap();
    assert_ne!(
        region_pixels(&sync_frames[0], ax, ay, aw, ah),
        region_pixels(&baseline, ax, ay, aw, ah),
        "ungreying Apply must be visible in Apply's own region: checkpoint 0 must differ \
         from the pre-click baseline there"
    );

    let full = full_repaint_frame(&html, &actions, pkt, &steps);
    assert_eq!(
        sync_frames.last().unwrap().pixels,
        full.pixels,
        "final checkpoint must match a forced full repaint"
    );
}

// ---------------------------------------------------------------------------
// EnabledWhen: dependent input grey/ungrey.
// ---------------------------------------------------------------------------

fn enabled_when_form_html() -> String {
    format!(
        "<!DOCTYPE html><html><head>{PIXEL_CSS}</head><body style=\"margin:0;width:400px;height:200px;background:#fff\">\
<form id=\"g\">\
<input id=\"gate\" type=\"checkbox\" name=\"gate\" checked style=\"position:absolute;left:10px;top:10px\"/>\
<input id=\"dep\" type=\"text\" name=\"dep\" value=\"x\" \
style=\"position:absolute;left:10px;top:50px;width:120px;height:22px\"/>\
<button id=\"save\" name=\"save\" hx-post=\"settings.save\" hx-trigger=\"click\" \
style=\"position:absolute;left:10px;top:90px;width:80px;height:30px\">Save</button>\
</form></body></html>"
    )
}

fn enabled_when_rules() -> Vec<(String, String, InputRule)> {
    vec![(
        "settings.save".into(),
        "dep".into(),
        InputRule::EnabledWhen {
            input: "gate".into(),
            checked: true,
            reason: "Enable gate first".into(),
        },
    )]
}

#[test]
fn pixel_parity_enabled_when_dependent_input() {
    let html = enabled_when_form_html();
    let actions = ["settings.save"];
    let probe_eng = probe(&html, &actions);
    let (gx, gy) = center(&probe_eng, "gate");

    let mut pkt = base_packet();
    pkt.input_rules = enabled_when_rules();
    pkt.versions.insert("actions".into(), 2);

    let steps = vec![
        click_script(gx, gy), // gate off: dep greys
        click_script(gx, gy), // gate on: dep ungreys
    ];

    let (baseline, worker_frames, sync_frames) =
        run_pixel_checkpoints(&html, &actions, pkt.clone(), &steps);
    assert_all_checkpoints_match(&worker_frames, &sync_frames);
    // Test gap (Graph 18:55 re-review): prove the rule-driven grey is
    // actually visible in dep's own region, not just identical on both
    // sides because neither applied it (a whole-frame diff would trivially
    // pass anyway since clicking the gate repaints its own tick mark).
    // Gate starts checked (dep enabled); checkpoint 0 unchecks the gate,
    // which must grey dep.
    let (dx, dy, dw, dh) = probe_eng.test_element_box("dep").unwrap();
    assert_ne!(
        region_pixels(&sync_frames[0], dx, dy, dw, dh),
        region_pixels(&baseline, dx, dy, dw, dh),
        "unchecking the gate must visibly grey dep: checkpoint 0 must differ from the \
         baseline in dep's own region"
    );

    let full = full_repaint_frame(&html, &actions, pkt, &steps);
    assert_eq!(
        sync_frames.last().unwrap().pixels,
        full.pixels,
        "final checkpoint must match a forced full repaint"
    );
}

// ---------------------------------------------------------------------------
// hx-changed flips: a RequiresChange-tracked control and a standalone
// data-hx-applied control outside any form (issue #24 opt-in).
// ---------------------------------------------------------------------------

fn hx_changed_form_html() -> String {
    format!(
        "<!DOCTYPE html><html><head>{PIXEL_CSS}</head><body style=\"margin:0;width:400px;height:200px;background:#fff\">\
<form id=\"f\">\
<input id=\"label\" type=\"text\" name=\"label\" value=\"Base\" \
style=\"position:absolute;left:10px;top:10px;width:160px;height:22px\"/>\
<button id=\"apply\" name=\"apply\" hx-post=\"settings.apply\" hx-trigger=\"click\" \
style=\"position:absolute;left:10px;top:50px;width:80px;height:30px\">Apply</button>\
</form>\
<input id=\"row\" type=\"text\" name=\"row\" value=\"Base\" data-hx-applied=\"Base\" \
style=\"position:absolute;left:10px;top:100px;width:160px;height:22px\"/>\
</body></html>"
    )
}

fn set_text_script(x: f32, y: f32, text: &'static str) -> Vec<InputEvent> {
    let mut v = click_script(x, y);
    v.push(InputEvent::Key(KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
    v.push(InputEvent::Key(KeyEvent {
        key: "Backspace".into(),
        code: "Backspace".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    v.push(InputEvent::Text(text.into()));
    v
}

/// Issue #24: the standalone `data-hx-applied` opt-in needs no form, no
/// action, and no sync packet content at all (it is re-evaluated purely
/// from local edits) — exercised on its own, with an entirely empty packet,
/// so this checkpoint cannot be confused with a `RequiresChange`-coupled
/// path.
fn standalone_hx_changed_form_html() -> String {
    format!(
        "<!DOCTYPE html><html><head>{PIXEL_CSS}</head><body style=\"margin:0;width:400px;height:200px;background:#fff\">\
<input id=\"row\" type=\"text\" name=\"row\" value=\"Base\" data-hx-applied=\"Base\" \
style=\"position:absolute;left:10px;top:10px;width:160px;height:22px\"/>\
</body></html>"
    )
}

#[test]
fn pixel_parity_hx_changed_standalone_only_no_action_no_sync_content() {
    let html = standalone_hx_changed_form_html();
    let actions: [&str; 0] = [];
    let probe_eng = probe(&html, &actions);
    let (rx, ry) = center(&probe_eng, "row");

    let pkt = base_packet(); // no actions, no input_rules, no fragments

    let steps = vec![
        set_text_script(rx, ry, "Edited"),
        set_text_script(rx, ry, "Base"),
    ];

    let (_baseline, worker_frames, sync_frames) =
        run_pixel_checkpoints(&html, &actions, pkt.clone(), &steps);
    assert_all_checkpoints_match(&worker_frames, &sync_frames);

    let full = full_repaint_frame(&html, &actions, pkt, &steps);
    assert_eq!(
        sync_frames.last().unwrap().pixels,
        full.pixels,
        "final checkpoint must match a forced full repaint"
    );
}

#[test]
fn pixel_parity_hx_changed_flips_in_form_and_standalone() {
    let html = hx_changed_form_html();
    let actions = ["settings.apply"];
    let probe_eng = probe(&html, &actions);
    let (lx, ly) = center(&probe_eng, "label");
    let (rx, ry) = center(&probe_eng, "row");

    let mut pkt = base_packet();
    pkt.actions = vec![(
        "settings.apply".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: "No changes".into(),
        },
    )];
    pkt.versions.insert("actions".into(), 2);

    let steps = vec![
        set_text_script(lx, ly, "Edited"), // label hx-changed on (RequiresChange form)
        set_text_script(lx, ly, "Base"),   // label hx-changed off
        set_text_script(rx, ry, "Edited"), // row hx-changed on (standalone opt-in)
        set_text_script(rx, ry, "Base"),   // row hx-changed off
    ];

    let (baseline, worker_frames, sync_frames) =
        run_pixel_checkpoints(&html, &actions, pkt.clone(), &steps);
    assert_all_checkpoints_match(&worker_frames, &sync_frames);
    // Sanity check only (not isolating the `hx-changed` class's own visual
    // contribution specifically — editing label's value always repaints its
    // glyphs regardless of the class, so this cannot distinguish "class
    // applied" from "class dropped"; that isolation is
    // `r2_hx_changed_select_flip_has_no_redundant_damage_source_partial_eq_full`'s
    // job, via damage rects on a control whose edit has no other visual
    // side effect). Just confirms the edit reached the screen at all.
    assert_ne!(
        sync_frames[0].pixels, baseline.pixels,
        "editing label must change the painted frame: checkpoint 0 must differ from the baseline"
    );

    let full = full_repaint_frame(&html, &actions, pkt, &steps);
    assert_eq!(
        sync_frames.last().unwrap().pixels,
        full.pixels,
        "final checkpoint must match a forced full repaint"
    );
}
