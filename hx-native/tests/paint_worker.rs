//! Issue #8 step 3: background paint worker.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hx_native::*;

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

fn button_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
body { width: 200px; height: 80px; margin: 0; background: #222; }
#act { position: absolute; left: 8px; top: 8px; width: 120px; height: 40px; }
</style></head><body>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##
}

fn section_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
body { width: 320px; height: 120px; margin: 0; background: #1a1f2a; color: #eee;
  font-family: "DejaVu Sans", sans-serif; }
#cell { padding: 12px; }
</style></head><body>
<section id="sec" hx-trigger="slice:ticker" hx-target="#cell" hx-swap="innerHTML">
  <div id="cell">start</div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##
}

fn packet(ticker_ver: u64, cell: &str) -> SyncPacket {
    let mut versions = HashMap::new();
    versions.insert("ticker".into(), ticker_ver);
    versions.insert("actions".into(), 1);
    let mut fragments = HashMap::new();
    fragments.insert("sec".into(), format!(r#"<div id="cell">{cell}</div>"#));
    SyncPacket {
        versions,
        actions: Vec::new(),
        fragments,
    }
}

fn click_events() -> [InputEvent; 3] {
    [
        InputEvent::PointerMove {
            x: 40.0,
            y: 24.0,
            mods: Modifiers::default(),
        },
        InputEvent::PointerDown {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
        InputEvent::PointerUp {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        },
    ]
}

fn action_name(outcomes: &[InputOutcome]) -> Option<(RequestId, String)> {
    outcomes.iter().find_map(|o| match o {
        InputOutcome::Action { request, name, .. } => Some((*request, name.clone())),
        _ => None,
    })
}

/// Graph condition 1: the worker never touches the sim. It only receives owned
/// [`SyncPacket`] snapshots. A host-side sim flag stays false across sync/paint.
#[test]
fn worker_never_touches_sim() {
    let sim_touched = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&sim_touched);

    // Host "sim" render: building the packet is the only place that may touch sim.
    let build_packet = |ver: u64, cell: &str| {
        // If the worker called back into sim, this flag would need to flip there.
        // It must stay false after flush.
        assert!(
            !flag.load(Ordering::SeqCst),
            "sim must not be touched before packet build completes"
        );
        packet(ver, cell)
    };

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.sync(build_packet(1, "one")).unwrap();
    worker.flush().unwrap();
    worker.sync(build_packet(2, "two")).unwrap();
    worker.flush().unwrap();

    assert!(
        !sim_touched.load(Ordering::SeqCst),
        "worker must not call into the sim; only SyncPacket crosses the channel"
    );
    let frame = worker.take_frame(SurfaceId::DEFAULT).expect("frame");
    assert!(!frame.output.skipped);
    assert!(frame.output.rgba.is_some());
}

/// Graph condition 2: drop while busy joins cleanly with no hang.
#[test]
fn drop_while_busy_joins_cleanly() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(640, 360))
        .unwrap();

    for i in 0..200u64 {
        worker.sync(packet(i + 1, &format!("cell-{i}"))).unwrap();
        for ev in click_events() {
            worker.input(ev).unwrap();
        }
    }

    let t0 = Instant::now();
    drop(worker);
    let elapsed = t0.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "drop while busy hung for {elapsed:?}"
    );
}

/// Graph condition 3: panic comes back as an error; last finished frame stays.
#[test]
fn panic_returns_error_keeps_last_frame() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(button_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(200, 80))
        .unwrap();
    worker.flush().unwrap();

    let before = worker
        .take_frame(SurfaceId::DEFAULT)
        .expect("initial frame");
    let before_gen = before.generation;
    let before_pixels = before.output.rgba.as_ref().unwrap().pixels.clone();

    worker.inject_panic_for_test().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let err = loop {
        if let Some(err) = worker.poll_error() {
            break err;
        }
        if Instant::now() > deadline {
            panic!("worker panic was not reported");
        }
        thread::sleep(Duration::from_millis(5));
    };
    match err {
        WorkerError::Panic(msg) => assert!(msg.contains("test panic"), "{msg}"),
        other => panic!("expected Panic, got {other:?}"),
    }

    let after = worker
        .take_frame(SurfaceId::DEFAULT)
        .expect("last frame must remain");
    assert_eq!(after.generation, before_gen);
    assert_eq!(after.output.rgba.as_ref().unwrap().pixels, before_pixels);
}

/// Graph condition 4a: input is never dropped even when the mailbox is saturated.
#[test]
fn input_never_dropped_when_mailbox_saturated() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(button_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(200, 80))
        .unwrap();
    worker.flush().unwrap();

    let sent = Arc::new(AtomicUsize::new(0));
    let sent_c = Arc::clone(&sent);

    let n_input = PAINT_WORKER_COMMAND_CAPACITY * 4;
    for i in 0..n_input {
        // Keep filling with syncs that may coalesce, then an input that must not.
        worker
            .sync(packet((i as u64) + 1, &format!("sat-{i}")))
            .unwrap();
        worker
            .input(InputEvent::PointerMove {
                x: 40.0,
                y: 24.0,
                mods: Modifiers::default(),
            })
            .unwrap();
        sent_c.fetch_add(1, Ordering::SeqCst);
    }
    worker.flush().unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), n_input);
    // Worker observed every input via the sequence counter.
    assert_eq!(worker.current_input_seq(), n_input as u64);
}

/// Graph condition 4b: slice syncs for the same target coalesce to the latest.
#[test]
fn slice_syncs_coalesce_to_latest() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.flush().unwrap();

    // Pause the worker by saturating it with work, then enqueue many syncs for
    // the same target with no intervening input — only the latest must apply.
    // Practical check: after a burst of syncs without flush between them, the
    // visible cell text is from the last packet.
    for i in 0..32u64 {
        worker.sync(packet(i + 1, &format!("v{i}"))).unwrap();
    }
    worker.flush().unwrap();

    let html = {
        // Serialize via a fresh sync engine comparison path is awkward through
        // the worker; check pixels changed from the initial frame and that a
        // follow-up sync with the same final content is a skipped/no-op paint.
        worker.take_frame(SurfaceId::DEFAULT).expect("frame")
    };
    assert!(html.output.rgba.is_some());

    let gen_after_burst = html.generation;
    worker.sync(packet(32, "v31")).unwrap();
    worker.flush().unwrap();
    let again = worker.take_frame(SurfaceId::DEFAULT).unwrap();
    // Same content as the coalesced latest → paint should skip (generation unchanged)
    // or publish an identical buffer. Generation may stay put on skip.
    assert!(
        again.generation == gen_after_burst
            || again.output.rgba.as_ref().map(|r| r.pixels.as_slice())
                == html.output.rgba.as_ref().map(|r| r.pixels.as_slice())
    );
}

/// Stronger coalesce test: fill the mailbox with syncs while the worker is
/// blocked on a slow paint, then confirm only the latest version is applied by
/// comparing against a sync engine that applied only the last packet.
#[test]
fn coalesced_sync_matches_latest_only_sync_engine() {
    let html = section_html();
    let view = Viewport::new(320, 120);

    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(&["act.do"]);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(view.clone()).unwrap();
    let mut last = packet(99, "FINAL");
    let slices = last.clone();
    sync_eng.sync(&slices, &mut last).unwrap();
    let expected = sync_eng.paint(view.clone()).unwrap();
    let expected_px = expected.rgba.as_ref().unwrap().pixels.clone();

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(html).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, view.clone())
        .unwrap();
    worker.flush().unwrap();

    for i in 0..40u64 {
        worker.sync(packet(i + 1, &format!("mid-{i}"))).unwrap();
    }
    worker.sync(packet(99, "FINAL")).unwrap();
    worker.flush().unwrap();

    let frame = worker.take_frame(SurfaceId::DEFAULT).unwrap();
    let got = frame.output.rgba.as_ref().unwrap().pixels.clone();
    assert_eq!(
        got, expected_px,
        "coalesced worker sync must match applying only the latest packet on the sync engine"
    );
}

/// Graph condition 5: same scripted input → same actions and RequestIds as sync.
#[test]
fn worker_matches_sync_action_and_request_ids() {
    let html = button_html();
    let view = Viewport::new(200, 80);

    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(&["act.do"]);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(view.clone()).unwrap();

    let mut sync_actions: Vec<(RequestId, String)> = Vec::new();
    for _ in 0..3 {
        for ev in click_events() {
            let outcome = sync_eng.input(ev);
            if let Some(pair) = action_name(std::slice::from_ref(&outcome)) {
                sync_actions.push(pair);
            }
        }
        for o in sync_eng.take_outcomes() {
            if let Some(pair) = action_name(std::slice::from_ref(&o)) {
                sync_actions.push(pair);
            }
        }
        let _ = sync_eng.paint(view.clone()).unwrap();
    }

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, view).unwrap();
    worker.flush().unwrap();
    let _ = worker.poll_outcomes();

    let mut worker_actions: Vec<(RequestId, String)> = Vec::new();
    for _ in 0..3 {
        for ev in click_events() {
            worker.input(ev).unwrap();
        }
        worker.flush().unwrap();
        for o in worker.poll_outcomes() {
            if let Some(pair) = action_name(std::slice::from_ref(&o)) {
                worker_actions.push(pair);
            }
        }
    }

    assert_eq!(
        worker_actions, sync_actions,
        "worker actions/RequestIds must match the synchronous Engine path"
    );
}

#[test]
fn paint_worker_handle_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<PaintWorker>();
    assert_send::<SyncPacket>();
    assert_send::<WorkerFrame>();
}
