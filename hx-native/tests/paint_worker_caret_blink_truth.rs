//! Issue #25: a `PaintWorker` version of `caret_blink_off_matches_transparent_caret_truth`
//! (`text_edit_controls.rs`). Graph drove a real `PaintWorker` through the caret
//! blink-off frames and got 0px difference against the `caret-color:transparent`
//! truth (probe evidence referenced in issue #25), but no test in the repo covered
//! that path. This ports the same fixture, fields (including the `ell` ellipsis
//! field), scales, and transformed/untransformed matrix through `PaintWorker`.
//!
//! `panic=abort` builds are out of scope (same as the sync-engine test's
//! `failpaint_off` scenario, which this port does not carry over — see the
//! module doc below for why).

use std::thread;
use std::time::{Duration, Instant};

use hx_native::*;

/// This fixture's first paint (gradients + ellipsis shaping, six fields) is
/// comfortably under budget in a release build, but can run past
/// `WORKER_STALL_TIMEOUT_NS` (100ms) in an unoptimized `cargo test` binary
/// under box-wide CPU contention — `flush()`'s own stall latches
/// permanently once tripped (by design: "a wedged worker"), so a single
/// `.unwrap()` right after `load_html` is not safe for this fixture's
/// first paint. Poll `latest_frame`'s generation directly instead (same
/// idiom `paint_worker.rs`'s `wait_frame` already uses) — `flush`'s
/// `Result` is advisory, not the source of truth for "is the frame ready".
fn wait_for_frame_after(
    worker: &PaintWorker,
    since_generation: u64,
    budget: Duration,
) -> FrameView {
    let deadline = Instant::now() + budget;
    loop {
        let _ = worker.flush();
        if let Some(v) = worker.latest_frame(SurfaceId::DEFAULT, since_generation) {
            if v.generation > since_generation {
                return v;
            }
        }
        if Instant::now() > deadline {
            panic!(
                "no new frame after generation {since_generation} within {budget:?} \
                 (worker error: {:?})",
                worker.poll_error()
            );
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn vp(s: f64) -> Viewport {
    Viewport {
        width: (400.0 * s).round() as u32,
        height: (300.0 * s).round() as u32,
        scale: s,
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

fn caret_truth_fields() -> String {
    r##"
<div id="pbg" style="position:absolute;left:0;top:0;width:290px;height:200px;background:linear-gradient(135deg,#204080,#c08020)"></div>
<input id="t" type="text" name="t" value="hello" style="position:absolute;left:13.3px;top:17.7px;width:120px;height:22px;font-size:14px"/>
<input id="g" type="text" name="g" value="hello" style="position:absolute;left:150px;top:17.7px;width:120px;height:22px;font-size:14px;background:linear-gradient(90deg,#ff4040,#4040ff)"/>
<input id="st" type="text" name="st" value="hello" style="position:absolute;left:13.3px;top:67.7px;width:120px;height:26px;font-size:14px;background:repeating-linear-gradient(0deg,#ffffff 0px,#ffffff 2px,#a0a0a0 2px,#a0a0a0 4px)"/>
<input id="al" type="text" name="al" value="hello" style="position:absolute;left:150px;top:67.7px;width:120px;height:22px;font-size:14px;background:rgba(255,255,255,0.35)"/>
<div id="ell_src" style="position:absolute;left:13.3px;top:107.7px;width:120px;height:22px;font-size:14px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;background:#fff">a quite very extremely long line of text here</div>
<input id="ell" type="text" name="ell" value="" style="position:absolute;left:115px;top:107.7px;width:18px;height:22px;font-size:14px;background:transparent;border:none;padding:0"/>
<button id="other" style="position:absolute;left:150px;top:140px">other</button>"##
        .to_string()
}

fn caret_truth_html(transformed: bool, oracle: bool) -> String {
    let inner = caret_truth_fields();
    let body = if transformed {
        format!(
            r#"<div id="panel" style="position:absolute;left:50%;top:50%;width:290px;height:200px;transform:translate(-50%,-50%)">{inner}</div>"#
        )
    } else {
        inner
    };
    let edge = r#"<input id="edge" type="text" name="edge" value="ab" style="position:absolute;left:372px;top:280px;width:120px;height:24px"/>"#;
    let style = if oracle {
        "<style>input{caret-color:transparent !important}</style>"
    } else {
        ""
    };
    format!(
        "<!DOCTYPE html><html><head>{style}</head><body style=\"margin:0;width:400px;height:300px;background:#fff\">{body}{edge}</body></html>"
    )
}

fn caret_truth_boot(transformed: bool, scale: f64, oracle: bool) -> Engine {
    let mut e = Engine::new(cfg());
    e.load_html(&caret_truth_html(transformed, oracle)).unwrap();
    let _ = e.paint(vp(scale)).unwrap();
    e
}

fn box_point(e: &Engine, id: &str, fx: f32) -> (f32, f32) {
    let b = e.test_element_box(id).unwrap();
    (b.0 + b.2 * fx, b.1 + b.3 * 0.5)
}

#[derive(Clone, Copy)]
enum CaretSt {
    Click(f32),
    Text(&'static str),
    Key(&'static str),
    Blink,
}
use CaretSt as CS;

fn click_events(x: f32, y: f32) -> Vec<InputEvent> {
    vec![
        InputEvent::PointerMove {
            x,
            y,
            mods: Modifiers::default(),
        },
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

fn key_event(k: &str) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    })
}

/// Best-effort wait for a frame generation past `since_generation`. Unlike
/// [`wait_for_frame_after`], never panics on timeout — some steps (e.g. a
/// redundant `Blink` while nothing is focused) legitimately paint nothing
/// new, so callers that cannot tell in advance just move on with whatever
/// generation is latest.
fn best_effort_wait(worker: &PaintWorker, since_generation: u64, budget: Duration) -> u64 {
    let deadline = Instant::now() + budget;
    loop {
        let _ = worker.flush();
        if let Some(v) = worker.latest_frame(SurfaceId::DEFAULT, 0) {
            if v.generation > since_generation {
                return v.generation;
            }
        }
        if Instant::now() > deadline {
            return since_generation;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// Run `steps` against the worker (field-box coordinates resolved against
/// `probe`, booted identically). `Blink` uses the worker-only test hook;
/// everything else is plain `PaintWorker::input`. Waits generously after
/// the *last* step (where correctness matters) and only briefly after
/// intermediate ones (where it does not, and where a legitimate no-op step
/// would otherwise burn the full budget every time).
fn caret_run_worker(worker: &PaintWorker, probe: &Engine, field: &str, steps: &[CS]) {
    let mut gen = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .map(|f| f.generation)
        .unwrap_or(0);
    for (i, st) in steps.iter().enumerate() {
        match *st {
            CS::Click(fx) => {
                let (x, y) = box_point(probe, field, if field == "edge" { 0.1 } else { fx });
                for ev in click_events(x, y) {
                    worker.input(ev).unwrap();
                }
            }
            CS::Text(t) => {
                worker.input(InputEvent::Text(t.into())).unwrap();
            }
            CS::Key(k) => {
                worker.input(key_event(k)).unwrap();
            }
            CS::Blink => {
                worker.force_caret_blink_for_test().unwrap();
            }
        }
        let budget = if i + 1 == steps.len() {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(300)
        };
        gen = best_effort_wait(worker, gen, budget);
    }
}

/// Same scenario, replayed on a plain sync `Engine` (for the oracle and for
/// the worker-vs-sync cross-check), `Blink` steps optionally dropped.
fn caret_run_sync(e: &mut Engine, s: f64, field: &str, steps: &[CS], drop_blink: bool) {
    for st in steps {
        match *st {
            CS::Click(fx) => {
                let (x, y) = box_point(e, field, if field == "edge" { 0.1 } else { fx });
                for ev in click_events(x, y) {
                    let _ = e.input(ev);
                }
            }
            CS::Text(t) => {
                let _ = e.input(InputEvent::Text(t.into()));
            }
            CS::Key(k) => {
                let _ = e.input(key_event(k));
            }
            CS::Blink => {
                if drop_blink {
                    continue;
                }
                e.test_force_caret_blink();
            }
        }
    }
    let _ = e.paint(vp(s)).unwrap();
}

fn rgba_diff_count(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.pixels
        .iter()
        .zip(b.pixels.iter())
        .filter(|(p, q)| p != q)
        .count()
}

fn scenarios() -> Vec<(&'static str, Vec<CS>)> {
    vec![
        ("off", vec![CS::Click(0.9), CS::Blink]),
        ("off_on", vec![CS::Click(0.9), CS::Blink, CS::Blink]),
        (
            "move_mid_off",
            vec![
                CS::Click(0.9),
                CS::Key("Home"),
                CS::Key("ArrowRight"),
                CS::Key("ArrowRight"),
                CS::Blink,
            ],
        ),
        (
            "type_during_off",
            vec![CS::Click(0.9), CS::Blink, CS::Text("k"), CS::Blink],
        ),
        ("home_off", vec![CS::Click(0.9), CS::Key("Home"), CS::Blink]),
    ]
}

/// Worker vs sync-partial vs `caret-color:transparent` truth (when
/// blink-off), for one (transformed, scale, field, scenario) point.
fn check_point(transformed: bool, scale: f64, field: &str, name: &str, steps: &[CS]) {
    let html = caret_truth_html(transformed, false);
    let probe = caret_truth_boot(transformed, scale, false);

    let worker = PaintWorker::spawn(cfg());
    worker.load_html(&html).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, vp(scale))
        .unwrap();
    // First paint (gradients + shaping across six fields) is the one step
    // generous enough to need the panic-on-timeout wait; see its doc comment.
    wait_for_frame_after(&worker, 0, Duration::from_secs(5));
    caret_run_worker(&worker, &probe, field, steps);
    let worker_frame = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .expect("worker frame");
    let worker_px = worker_frame.output.rgba.as_ref().expect("worker rgba");
    let caret = worker.test_caret_visible().unwrap();

    let mut sync_eng = caret_truth_boot(transformed, scale, false);
    caret_run_sync(&mut sync_eng, scale, field, steps, false);
    let sync_px = sync_eng.last_frame().expect("sync frame");
    let dp = rgba_diff_count(worker_px, sync_px);
    assert_eq!(
        dp, 0,
        "WORKER PARITY FAIL transformed={transformed} scale={scale} field={field} {name}: \
         worker vs sync differ by {dp}px"
    );

    if caret != Some(true) {
        let mut oracle = caret_truth_boot(transformed, scale, true);
        caret_run_sync(&mut oracle, scale, field, steps, true);
        assert!(
            oracle.test_caret_visible().unwrap_or(true),
            "oracle must keep Blitz focus while caret-color:transparent hides pixels"
        );
        let oracle_px = oracle.last_frame().expect("oracle frame");
        let dt = rgba_diff_count(worker_px, oracle_px);
        assert_eq!(
            dt, 0,
            "WORKER TRUTH FAIL transformed={transformed} scale={scale} field={field} {name}: \
             worker vs caret-color:transparent truth differ by {dt}px"
        );
    }
}

#[test]
fn paint_worker_caret_blink_off_matches_transparent_caret_truth() {
    for transformed in [false, true] {
        let scale = if transformed { 2.0 } else { 1.0 };
        for field in ["t", "g", "st", "al", "ell", "edge"] {
            for (name, steps) in scenarios() {
                check_point(transformed, scale, field, name, &steps);
            }
        }
    }
}

/// Dedicated check that scale 1.25 (the one scale not covered by the main
/// matrix above) is also exercised, both transformed and not.
#[test]
fn paint_worker_caret_blink_off_matches_truth_scale_1_25() {
    for transformed in [false, true] {
        for (name, steps) in scenarios() {
            check_point(transformed, 1.25, "t", name, &steps);
        }
    }
}

// ---------------------------------------------------------------------------
// Graph's extra listed gaps: blink-off with a selection, and a resize during
// blink-off. `panic=abort` builds are out of scope (not testable from a
// normal `cargo test` run; would need a separate `panic = "abort"` profile
// and process-level crash harness, which this suite does not have).
// ---------------------------------------------------------------------------

/// Blink-off with an active selection: the engine does not run the
/// FOCUS-suppress redraw at all when there is a selection (no caret line is
/// drawn with a selection in play, so there is nothing to suppress) — this
/// must still match the transparent-caret truth, through the worker.
#[test]
fn paint_worker_caret_blink_off_with_selection_matches_truth() {
    for transformed in [false, true] {
        let scale = if transformed { 2.0 } else { 1.0 };
        for field in ["t", "ell"] {
            let steps = vec![CS::Click(0.9), CS::Key("Home")];
            let html = caret_truth_html(transformed, false);
            let probe = caret_truth_boot(transformed, scale, false);

            let worker = PaintWorker::spawn(cfg());
            worker.load_html(&html).unwrap();
            worker
                .resize_surface(SurfaceId::DEFAULT, vp(scale))
                .unwrap();
            wait_for_frame_after(&worker, 0, Duration::from_secs(5));
            caret_run_worker(&worker, &probe, field, &steps);
            let gen_before_select = worker
                .latest_frame(SurfaceId::DEFAULT, 0)
                .map(|f| f.generation)
                .unwrap_or(0);
            // Select-all (ctrl+a), then force blink-off.
            worker
                .input(InputEvent::Key(KeyEvent {
                    key: "a".into(),
                    code: "KeyA".into(),
                    mods: Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                    repeat: false,
                    pressed: true,
                }))
                .unwrap();
            let gen = best_effort_wait(&worker, gen_before_select, Duration::from_millis(500));
            worker.force_caret_blink_for_test().unwrap();
            best_effort_wait(&worker, gen, Duration::from_secs(5));
            let caret = worker.test_caret_visible().unwrap();
            let worker_frame = worker
                .latest_frame(SurfaceId::DEFAULT, 0)
                .expect("worker frame");
            let worker_px = worker_frame.output.rgba.as_ref().expect("worker rgba");

            let mut sync_eng = caret_truth_boot(transformed, scale, false);
            caret_run_sync(&mut sync_eng, scale, field, &steps, false);
            let _ = sync_eng.input(InputEvent::Key(KeyEvent {
                key: "a".into(),
                code: "KeyA".into(),
                mods: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
                pressed: true,
            }));
            sync_eng.test_force_caret_blink();
            let _ = sync_eng.paint(vp(scale)).unwrap();
            let sync_px = sync_eng.last_frame().expect("sync frame");
            assert_eq!(
                rgba_diff_count(worker_px, sync_px),
                0,
                "selection blink-off: worker vs sync differ (transformed={transformed} field={field})"
            );

            if caret != Some(true) {
                let mut oracle = caret_truth_boot(transformed, scale, true);
                caret_run_sync(&mut oracle, scale, field, &steps, true);
                let _ = oracle.input(InputEvent::Key(KeyEvent {
                    key: "a".into(),
                    code: "KeyA".into(),
                    mods: Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                    repeat: false,
                    pressed: true,
                }));
                let _ = oracle.paint(vp(scale)).unwrap();
                let oracle_px = oracle.last_frame().expect("oracle frame");
                assert_eq!(
                    rgba_diff_count(worker_px, oracle_px),
                    0,
                    "selection blink-off: worker vs truth differ (transformed={transformed} field={field})"
                );
            }
        }
    }
}

/// A resize while the caret is in its blink-off phase must not leave the
/// stale suppressed-caret rect uncovered, or re-show the caret — the
/// resized frame must still match the transparent-caret truth at the new
/// viewport, through the worker.
#[test]
fn paint_worker_resize_during_blink_off_matches_truth() {
    for transformed in [false, true] {
        let scale = if transformed { 2.0 } else { 1.0 };
        let field = "t";
        let steps = vec![CS::Click(0.9), CS::Blink];
        let html = caret_truth_html(transformed, false);
        let probe = caret_truth_boot(transformed, scale, false);

        let worker = PaintWorker::spawn(cfg());
        worker.load_html(&html).unwrap();
        worker
            .resize_surface(SurfaceId::DEFAULT, vp(scale))
            .unwrap();
        wait_for_frame_after(&worker, 0, Duration::from_secs(5));
        caret_run_worker(&worker, &probe, field, &steps);
        let caret = worker.test_caret_visible().unwrap();
        assert_ne!(caret, Some(true), "expected blink-off before the resize");

        let gen_before_resize = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .map(|f| f.generation)
            .unwrap_or(0);
        let resized = Viewport {
            width: (500.0 * scale).round() as u32,
            height: (350.0 * scale).round() as u32,
            scale,
            holes: vec![],
        };
        worker
            .resize_surface(SurfaceId::DEFAULT, resized.clone())
            .unwrap();
        wait_for_frame_after(&worker, gen_before_resize, Duration::from_secs(5));
        let worker_frame = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .expect("worker frame");
        let worker_px = worker_frame.output.rgba.as_ref().expect("worker rgba");

        let mut sync_eng = caret_truth_boot(transformed, scale, false);
        caret_run_sync(&mut sync_eng, scale, field, &steps, false);
        let _ = sync_eng.paint(resized.clone()).unwrap();
        let sync_px = sync_eng.last_frame().expect("sync frame");
        assert_eq!(
            rgba_diff_count(worker_px, sync_px),
            0,
            "resize during blink-off: worker vs sync differ (transformed={transformed})"
        );

        let mut oracle = caret_truth_boot(transformed, scale, true);
        caret_run_sync(&mut oracle, scale, field, &steps, true);
        let _ = oracle.paint(resized).unwrap();
        let oracle_px = oracle.last_frame().expect("oracle frame");
        assert_eq!(
            rgba_diff_count(worker_px, oracle_px),
            0,
            "resize during blink-off: worker vs truth differ (transformed={transformed})"
        );
    }
}
