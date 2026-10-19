//! Issue #8 step 3: host frame cost, worker paint time, input→frame latency.
//!
//! Reports mean / p95 / p99 at 720p, 1080p and 1440p with other CPU cores idle
//! and with them busy (spin threads on every other core to simulate the sim).
//!
//! Usage: `cargo run -p hx-native --example worker_bench --release`

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hx_native::*;

const WARMUP: usize = 10;
const FRAMES: usize = 200;

fn section_page(css_w: u32, css_h: u32) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #1a1f2a; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }}
#cell {{ position: absolute; left: 8px; top: 8px; font-size: 18px; }}
#act {{ position: absolute; left: 8px; top: 48px; width: 120px; height: 32px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }}
#ticker-lane {{
  position: absolute; left: 0; top: {ticker_top}px; width: {css_w}px; height: 32px;
  overflow: hidden; background: #141820;
}}
</style></head><body>
<section id="sec-cell" hx-trigger="slice:ticker" hx-target="#cell" hx-swap="innerHTML">
  <div id="cell">ready</div>
</section>
<section id="sec-ticker" hx-trigger="slice:ticker" hx-target="#ticker-lane" hx-swap="innerHTML">
  <div id="ticker-lane">news</div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##,
        css_w = css_w,
        css_h = css_h,
        ticker_top = css_h.saturating_sub(40),
    )
}

fn packet(ver: u64, offset: i32) -> SyncPacket {
    let mut versions = HashMap::new();
    versions.insert("ticker".into(), ver);
    versions.insert("actions".into(), 1);
    let mut fragments = HashMap::new();
    fragments.insert(
        "sec-cell".into(),
        format!(r#"<div id="cell">tick {ver}</div>"#),
    );
    fragments.insert(
        "sec-ticker".into(),
        format!(
            r#"<div id="ticker-lane"><span style="margin-left:{offset}px">Breaking · Port delay · Week tick · Sim</span></div>"#
        ),
    );
    SyncPacket {
        versions,
        actions: Vec::new(),
        fragments,
    }
}

fn pct(samples: &[u64], p: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    let idx = ((v.len() as f64) * p) as usize;
    v[idx.min(v.len() - 1)]
}

fn mean(samples: &[u64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<u64>() as f64 / samples.len() as f64
}

fn report_row(label: &str, samples: &[u64]) {
    if samples.is_empty() {
        println!("| {label} | 0 | — | — | — |");
        return;
    }
    println!(
        "| {label} | {} | {:.3} | {:.3} | {:.3} |",
        samples.len(),
        mean(samples) / 1e6,
        pct(samples, 0.95) as f64 / 1e6,
        pct(samples, 0.99) as f64 / 1e6,
    );
}

struct LoadGuard {
    stop: Arc<AtomicBool>,
    joins: Vec<thread::JoinHandle<()>>,
}

impl LoadGuard {
    fn spawn_busy() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let cores = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(1)
            .max(1);
        let mut joins = Vec::with_capacity(cores);
        for _ in 0..cores {
            let stop = Arc::clone(&stop);
            joins.push(thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // Spin to occupy the core (synthetic sim load).
                    let mut x = 0u64;
                    for i in 0..10_000u64 {
                        x = x.wrapping_mul(i).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                }
            }));
        }
        Self { stop, joins }
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for j in self.joins.drain(..) {
            let _ = j.join();
        }
    }
}

fn run_res(label: &str, width: u32, height: u32, busy: bool) {
    let _load = if busy {
        Some(LoadGuard::spawn_busy())
    } else {
        None
    };

    let css_w = width;
    let css_h = height;
    let worker = PaintWorker::spawn(EngineConfig {
        max_paint_ns: 50_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    worker.declare_actions(&["act.do"]).unwrap();
    worker
        .open_surface(
            SurfaceId::DEFAULT,
            &section_page(css_w, css_h),
            Viewport::new(width, height),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
    worker.flush().unwrap();
    let _ = worker.take_frame(SurfaceId::DEFAULT);

    let mut host_ns = Vec::with_capacity(FRAMES);
    let mut paint_ns = Vec::with_capacity(FRAMES);
    let mut input_latency_frames = Vec::with_capacity(FRAMES / 8);

    for i in 0..(WARMUP + FRAMES) {
        let ver = (i as u64) + 1;
        let t_host0 = Instant::now();
        worker.sync(packet(ver, -((i as i32 % 40) * 3))).unwrap();
        if i % 8 == 0 {
            let seq_before = worker.current_input_seq();
            worker
                .input(InputEvent::PointerMove {
                    x: 40.0,
                    y: 60.0,
                    mods: Modifiers::default(),
                })
                .unwrap();
            // Non-blocking host path: take the latest finished frame now.
            let _ = worker.take_frame(SurfaceId::DEFAULT);
            let host_cost = t_host0.elapsed().as_nanos() as u64;

            // Wait for the worker to publish a frame that observed this input
            // (bench only — Mainnet does not wait).
            let mut frames_waited = 0u64;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                worker.flush().ok();
                if worker.last_frame_input_seq() > seq_before {
                    break;
                }
                if Instant::now() > deadline {
                    break;
                }
                frames_waited += 1;
                thread::sleep(Duration::from_millis(1));
            }
            if i >= WARMUP {
                input_latency_frames.push(frames_waited.max(1));
            }
            if i >= WARMUP {
                host_ns.push(host_cost);
            }
        } else {
            let _ = worker.take_frame(SurfaceId::DEFAULT);
            let host_cost = t_host0.elapsed().as_nanos() as u64;
            worker.flush().ok();
            if i >= WARMUP {
                host_ns.push(host_cost);
            }
        }

        if let Some(frame) = worker.take_frame(SurfaceId::DEFAULT) {
            if i >= WARMUP {
                paint_ns.push(frame.paint_ns);
            }
        }
    }

    println!();
    println!("### {label} ({width}x{height}, busy_cores={busy})");
    println!("| Metric | n | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");
    report_row("host frame (enqueue + take_frame)", &host_ns);
    report_row("worker paint", &paint_ns);
    if !input_latency_frames.is_empty() {
        println!(
            "input→visible latency (frames): mean={:.2}  p95={}  p99={}  n={}",
            mean(&input_latency_frames),
            pct(&input_latency_frames, 0.95),
            pct(&input_latency_frames, 0.99),
            input_latency_frames.len(),
        );
    }

    drop(worker);
}

fn main() {
    println!("hx-native worker_bench (issue #8 step 3)");
    println!(
        "cores={}  command_capacity={}  frame_slots={}",
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        PAINT_WORKER_COMMAND_CAPACITY,
        PAINT_WORKER_FRAME_SLOTS,
    );

    let resolutions = [
        ("720p", 1280u32, 720u32),
        ("1080p", 1920, 1080),
        ("1440p", 2560, 1440),
    ];

    for busy in [false, true] {
        let load = if busy { "busy" } else { "idle" };
        println!();
        println!("## Other cores {load}");
        for (name, w, h) in resolutions {
            run_res(name, w, h, busy);
        }
    }
}
