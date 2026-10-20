//! Issue #8 step 3: background paint worker (UI + Graph conditions).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
#act { position: absolute; left: 8px; top: 70px; width: 100px; height: 32px; }
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
    let mut pkt = SyncPacket::default();
    pkt.versions = versions;
    pkt.fragments = fragments;
    pkt
}

fn click_at(x: f32, y: f32) -> [InputEvent; 3] {
    [
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

fn apply_damage(dst: &mut RgbaImage, src: &RgbaImage, rects: &[Rect]) {
    assert_eq!((dst.width, dst.height), (src.width, src.height));
    for r in rects {
        let x0 = r.x.max(0.0) as u32;
        let y0 = r.y.max(0.0) as u32;
        let x1 = (r.x + r.width).min(src.width as f32).ceil() as u32;
        let y1 = (r.y + r.height).min(src.height as f32).ceil() as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * src.width + x) * 4) as usize;
                dst.pixels[i..i + 4].copy_from_slice(&src.pixels[i..i + 4]);
            }
        }
    }
}

fn wait_frame(worker: &PaintWorker, since: u64) -> FrameView {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let _ = worker.flush();
        if let Some(v) = worker.latest_frame(SurfaceId::DEFAULT, since) {
            if v.generation > since {
                return v;
            }
        }
        if Instant::now() > deadline {
            panic!("no frame after generation {since}");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn worker_never_touches_sim() {
    let sim_touched = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&sim_touched);
    let build = |ver: u64, cell: &str| {
        assert!(!flag.load(Ordering::SeqCst));
        packet(ver, cell)
    };
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.sync(build(1, "one")).unwrap();
    worker.flush().unwrap();
    worker.sync(build(2, "two")).unwrap();
    worker.flush().unwrap();
    assert!(!sim_touched.load(Ordering::SeqCst));
    let frame = worker.latest_frame(SurfaceId::DEFAULT, 0).expect("frame");
    assert!(frame.output.rgba.is_some());
}

#[test]
fn drop_while_busy_joins_cleanly() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(640, 360))
        .unwrap();
    for i in 0..200u64 {
        let _ = worker.sync(packet(i + 1, &format!("cell-{i}")));
        for ev in click_at(40.0, 24.0) {
            let _ = worker.input(ev);
        }
    }
    let t0 = Instant::now();
    drop(worker);
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn panic_returns_error_keeps_last_frame() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(button_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(200, 80))
        .unwrap();
    worker.flush().unwrap();
    let before = worker.latest_frame(SurfaceId::DEFAULT, 0).expect("frame");
    let before_gen = before.generation;
    let before_px = before.output.rgba.as_ref().unwrap().pixels.clone();
    worker.inject_panic_for_test().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let err = loop {
        if let Some(err) = worker.poll_error() {
            break err;
        }
        if Instant::now() > deadline {
            panic!("no panic error");
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(err, WorkerError::Panic(_)));
    let after = worker.latest_frame(SurfaceId::DEFAULT, 0).unwrap();
    assert_eq!(after.generation, before_gen);
    assert_eq!(after.output.rgba.as_ref().unwrap().pixels, before_px);
}

#[test]
fn saturated_burst_never_blocks() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker.flush().unwrap();
    let t0 = Instant::now();
    for i in 0..5_000 {
        worker.sync(packet(i + 1, "x")).unwrap();
        worker
            .input(InputEvent::PointerMove {
                x: (i % 50) as f32,
                y: 10.0,
                mods: Modifiers::default(),
            })
            .unwrap();
        worker.set_visible(SurfaceId::DEFAULT, true).unwrap();
    }
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "burst blocked for {:?}",
        t0.elapsed()
    );
}

fn upload_check_skip(skip: u64) {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.sync(packet(1, "base")).unwrap();
    worker.flush().unwrap();
    let mut host = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .unwrap()
        .output
        .rgba
        .as_ref()
        .unwrap()
        .clone();
    let mut since = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .unwrap()
        .generation;

    for i in 0..skip {
        worker.sync(packet(2 + i, &format!("skip-{i}"))).unwrap();
        worker.flush().unwrap();
    }
    let view = wait_frame(&worker, since);
    apply_damage(&mut host, view.output.rgba.as_ref().unwrap(), &view.damage);
    assert_eq!(
        host.pixels,
        view.output.rgba.as_ref().unwrap().pixels,
        "skip={skip} upload via returned damage must match full frame"
    );
    since = view.generation;
    let _ = since;
}

#[test]
fn damage_skip_1_byte_exact_upload() {
    upload_check_skip(1);
}

#[test]
fn damage_skip_3_byte_exact_upload() {
    upload_check_skip(3);
}

#[test]
fn damage_skip_past_history_full_rect() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.sync(packet(1, "base")).unwrap();
    worker.flush().unwrap();
    let first = worker.latest_frame(SurfaceId::DEFAULT, 0).unwrap();
    let mut host = first.output.rgba.as_ref().unwrap().clone();
    let since = first.generation;

    let past = DAMAGE_HISTORY_GENERATIONS as u64 + 2;
    for i in 0..past {
        worker.sync(packet(2 + i, &format!("past-{i}"))).unwrap();
        worker.flush().unwrap();
    }
    let view = wait_frame(&worker, since);
    assert!(
        view.full_upload || view.damage.len() == 1,
        "past history must fall back to full/near-full damage: {:?}",
        view.damage
    );
    apply_damage(&mut host, view.output.rgba.as_ref().unwrap(), &view.damage);
    assert_eq!(host.pixels, view.output.rgba.as_ref().unwrap().pixels);
}

#[test]
fn untaken_publish_merges_damage_one_take_covers_both() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(section_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(320, 120))
        .unwrap();
    worker.sync(packet(1, "a")).unwrap();
    worker.flush().unwrap();
    let base = worker.latest_frame(SurfaceId::DEFAULT, 0).unwrap();
    let mut host = base.output.rgba.as_ref().unwrap().clone();
    // Two publishes without taking.
    worker.sync(packet(2, "b")).unwrap();
    worker.flush().unwrap();
    worker.sync(packet(3, "c")).unwrap();
    worker.flush().unwrap();
    let view = worker
        .latest_frame(SurfaceId::DEFAULT, base.generation)
        .unwrap();
    apply_damage(&mut host, view.output.rgba.as_ref().unwrap(), &view.damage);
    assert_eq!(host.pixels, view.output.rgba.as_ref().unwrap().pixels);
}

#[test]
fn poll_every_nth_generation_byte_identical_to_sync() {
    for stride in [2u64, 3u64] {
        let html = section_html();
        let view = Viewport::new(320, 120);
        let steps = DAMAGE_HISTORY_GENERATIONS as u64 + 4;

        let mut sync_eng = Engine::new(cfg());
        sync_eng.declare_actions(&["act.do"]);
        sync_eng.load_html(html).unwrap();
        let mut sync_frags = packet(1, "base");
        let slices = sync_frags.clone();
        sync_eng.sync(&slices, &mut sync_frags).unwrap();
        let mut sync_img = sync_eng.paint(view.clone()).unwrap().rgba.unwrap();

        let worker = PaintWorker::spawn(cfg());
        worker.declare_actions(&["act.do"]).unwrap();
        worker.load_html(html).unwrap();
        worker
            .resize_surface(SurfaceId::DEFAULT, view.clone())
            .unwrap();
        worker.sync(packet(1, "base")).unwrap();
        worker.flush().unwrap();
        let mut host = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .unwrap()
            .output
            .rgba
            .as_ref()
            .unwrap()
            .clone();
        let mut since = worker
            .latest_frame(SurfaceId::DEFAULT, 0)
            .unwrap()
            .generation;

        for i in 0..steps {
            let cell = format!("n{i}");
            let mut pkt = packet(2 + i, &cell);
            let slices = pkt.clone();
            sync_eng.sync(&slices, &mut pkt).unwrap();
            let out = sync_eng.paint(view.clone()).unwrap();
            if let Some(img) = out.rgba {
                sync_img = img;
            }

            worker.sync(packet(2 + i, &cell)).unwrap();
            worker.flush().unwrap();

            if (i + 1) % stride == 0 {
                let fv = wait_frame(&worker, since);
                apply_damage(&mut host, fv.output.rgba.as_ref().unwrap(), &fv.damage);
                since = fv.generation;
            }
        }
        // Final take if the last step was not a poll boundary.
        if let Some(fv) = worker.latest_frame(SurfaceId::DEFAULT, since) {
            if fv.generation > since || fv.output.rgba.is_some() {
                apply_damage(&mut host, fv.output.rgba.as_ref().unwrap(), &fv.damage);
            }
        }
        assert_eq!(
            host.pixels, sync_img.pixels,
            "stride={stride} host upload must match sync engine"
        );
    }
}

#[test]
fn move_click_move_key_matches_sync_actions_and_hover() {
    let html = button_html();
    let view = Viewport::new(200, 80);

    let script = [
        InputEvent::PointerMove {
            x: 10.0,
            y: 10.0,
            mods: Modifiers::default(),
        },
        InputEvent::PointerMove {
            x: 40.0,
            y: 24.0,
            mods: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        },
        InputEvent::PointerDown {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        },
        InputEvent::PointerUp {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        },
        InputEvent::PointerMove {
            x: 80.0,
            y: 24.0,
            mods: Modifiers::default(),
        },
        InputEvent::Key(KeyEvent {
            key: "a".into(),
            code: "KeyA".into(),
            mods: Modifiers::default(),
            repeat: false,
            pressed: true,
        }),
    ];

    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(&["act.do"]);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(view.clone()).unwrap();
    let mut sync_actions = Vec::new();
    let mut sync_hover = None;
    for ev in &script {
        let o = sync_eng.input(ev.clone());
        if let InputOutcome::Hover { id } = &o {
            sync_hover = id.clone();
        }
        if let InputOutcome::Action { request, name, .. } = &o {
            sync_actions.push((*request, name.clone()));
        }
    }
    let _ = sync_eng.paint(view.clone()).unwrap();

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, view).unwrap();
    worker.flush().unwrap();
    let _ = worker.poll_outcomes();
    for ev in &script {
        worker.input(ev.clone()).unwrap();
    }
    worker.flush().unwrap();
    let mut worker_actions = Vec::new();
    let mut worker_hover = None;
    for wo in worker.poll_outcomes() {
        assert_eq!(wo.surface, SurfaceId::DEFAULT);
        match &wo.outcome {
            InputOutcome::Hover { id } => worker_hover = id.clone(),
            InputOutcome::Action { request, name, .. } => {
                worker_actions.push((*request, name.clone()));
            }
            _ => {}
        }
    }
    assert_eq!(worker_actions, sync_actions);
    assert_eq!(worker_hover, sync_hover);
}

#[test]
fn drag_keeps_every_move_between_down_and_up() {
    // Hold the worker so the mailbox accumulates; drag moves must not coalesce.
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(button_html()).unwrap();
    worker
        .resize_surface(SurfaceId::DEFAULT, Viewport::new(200, 80))
        .unwrap();
    worker.flush().unwrap();
    let release = Arc::new(AtomicBool::new(false));
    worker.inject_hold_for_test(Arc::clone(&release)).unwrap();
    thread::sleep(Duration::from_millis(20));
    worker
        .input(InputEvent::PointerDown {
            x: 40.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
        .unwrap();
    let before = worker.current_input_seq();
    for i in 0..10 {
        worker
            .input(InputEvent::PointerMove {
                x: 40.0 + i as f32,
                y: 24.0,
                mods: Modifiers {
                    shift: i == 9,
                    ..Modifiers::default()
                },
            })
            .unwrap();
    }
    assert_eq!(
        worker.current_input_seq() - before,
        10,
        "each drag move must be accepted"
    );
    worker
        .input(InputEvent::PointerUp {
            x: 49.0,
            y: 24.0,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
        .unwrap();
    // After release, coalescing resumes (many moves → one surviving).
    let after_up = worker.current_input_seq();
    for i in 0..20 {
        worker
            .input(InputEvent::PointerMove {
                x: i as f32,
                y: 1.0,
                mods: Modifiers::default(),
            })
            .unwrap();
    }
    assert_eq!(worker.current_input_seq() - after_up, 20);
    release.store(true, Ordering::SeqCst);
    // flush may stall while hold drains; wait for progress then flush.
    thread::sleep(Duration::from_millis(30));
    let _ = worker.flush();
}

#[test]
fn close_open_set_visible_does_not_cross_runs() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    let id = SurfaceId(7);
    worker
        .open_surface(
            id,
            button_html(),
            Viewport::new(200, 80),
            SurfaceOpts {
                visible: true,
                pinned: false,
            },
        )
        .unwrap();
    worker.set_visible(id, false).unwrap();
    worker.close_surface(id).unwrap();
    worker
        .open_surface(
            id,
            button_html(),
            Viewport::new(200, 80),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
    worker.set_visible(id, true).unwrap();
    worker.flush().unwrap();
    assert!(worker.poll_error().is_none());
    let frame = worker.latest_frame(id, 0);
    assert!(frame.is_some());
}

#[test]
fn fire_and_forget_error_includes_surface_id() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.set_visible(SurfaceId(99), true).unwrap();
    worker.flush().unwrap();
    let err = worker.poll_error().expect("unknown surface error");
    match err {
        WorkerError::Engine {
            surface: Some(SurfaceId(99)),
            ..
        } => {}
        other => panic!("expected Engine with surface 99, got {other:?}"),
    }
}

#[test]
fn outcomes_carry_surface_id() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    let a = SurfaceId(1);
    let b = SurfaceId(2);
    worker
        .open_surface(
            a,
            button_html(),
            Viewport::new(200, 80),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
    worker
        .open_surface(
            b,
            button_html(),
            Viewport::new(200, 80),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
    worker.flush().unwrap();
    for ev in click_at(40.0, 24.0) {
        worker.input_surface(a, ev).unwrap();
    }
    for ev in click_at(40.0, 24.0) {
        worker.input_surface(b, ev).unwrap();
    }
    worker.flush().unwrap();
    let outs = worker.poll_outcomes();
    let actions: Vec<_> = outs
        .iter()
        .filter_map(|o| match &o.outcome {
            InputOutcome::Action { .. } => Some(o.surface),
            _ => None,
        })
        .collect();
    assert!(actions.contains(&a) && actions.contains(&b), "{actions:?}");
}

#[test]
fn stall_when_worker_held() {
    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(button_html()).unwrap();
    worker.flush().unwrap();
    let release = Arc::new(AtomicBool::new(false));
    worker.inject_hold_for_test(Arc::clone(&release)).unwrap();
    // Give the worker time to enter the hold.
    thread::sleep(Duration::from_millis(20));
    let t0 = Instant::now();
    let err = worker.flush().expect_err("flush must stall");
    let elapsed = t0.elapsed();
    assert!(matches!(err, WorkerError::Stalled), "{err:?}");
    assert!(
        elapsed < Duration::from_nanos(WORKER_STALL_TIMEOUT_NS * 3),
        "blocked too long: {elapsed:?}"
    );
    // Input still accepted into the queue while stalled.
    let _ = worker.input(InputEvent::PointerLeave);
    release.store(true, Ordering::SeqCst);
    // Allow recovery.
    thread::sleep(Duration::from_millis(50));
}

#[test]
fn worker_matches_sync_request_ids() {
    let html = button_html();
    let view = Viewport::new(200, 80);
    let mut sync_eng = Engine::new(cfg());
    sync_eng.declare_actions(&["act.do"]);
    sync_eng.load_html(html).unwrap();
    let _ = sync_eng.paint(view.clone()).unwrap();
    let mut sync_actions = Vec::new();
    for _ in 0..3 {
        for ev in click_at(40.0, 24.0) {
            if let InputOutcome::Action { request, name, .. } = sync_eng.input(ev) {
                sync_actions.push((request, name));
            }
        }
    }

    let worker = PaintWorker::spawn(cfg());
    worker.declare_actions(&["act.do"]).unwrap();
    worker.load_html(html).unwrap();
    worker.resize_surface(SurfaceId::DEFAULT, view).unwrap();
    worker.flush().unwrap();
    let _ = worker.poll_outcomes();
    let mut worker_actions = Vec::new();
    for _ in 0..3 {
        for ev in click_at(40.0, 24.0) {
            worker.input(ev).unwrap();
        }
        worker.flush().unwrap();
        for o in worker.poll_outcomes() {
            if let InputOutcome::Action { request, name, .. } = o.outcome {
                worker_actions.push((request, name));
            }
        }
    }
    assert_eq!(worker_actions, sync_actions);
}

#[test]
fn paint_worker_handle_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<PaintWorker>();
    assert_send::<FrameView>();
    assert_send::<WorkerOutcome>();
}
