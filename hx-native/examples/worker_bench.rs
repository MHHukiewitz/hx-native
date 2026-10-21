//! Issue #8 step 3: host frame cost, worker paint time, input→frame latency.
//!
//! Reports mean / p95 / p99 at 720p, 1080p and 1440p with other CPU cores idle
//! and with them busy (spin threads on every other core to simulate the sim).
//! Also reports a sync [`Engine`] paint row at each resolution in the same
//! session for comparison.
//!
//! Usage: `cargo run -p hx-native --example worker_bench --release`

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// AtomicBool/Ordering used by LoadGuard and FAF busy-control bench.

use hx_native::*;

/// Discarded frames after open / before samples. Raised from 10 after the
/// 1440p idle p95≈15 ms outlier: that spike was the first cold 1440p pass in
/// the session (fonts / raster / CPU clocks), not a real idle>busy effect.
const WARMUP: usize = 30;
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
        input_rules: Vec::new(),
        fragments,
    }
}

struct MemSlices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}

impl MemSlices {
    fn from_packet(p: &SyncPacket) -> Self {
        Self {
            versions: p.versions.clone(),
            actions: p.actions.clone(),
        }
    }
}

impl SliceSource for MemSlices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.actions.clone()
    }
}

struct Frags {
    bodies: HashMap<String, String>,
}

impl Frags {
    fn from_packet(p: &SyncPacket) -> Self {
        Self {
            bodies: p.fragments.clone(),
        }
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
                    let mut x = 0u64;
                    for i in 0..10_000u64 {
                        x = x.wrapping_mul(i).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                }
            }));
        }
        // Let the package leave deep idle before the first measured paint.
        thread::sleep(Duration::from_millis(50));
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

struct WorkerStats {
    host_ns: Vec<u64>,
    paint_ns: Vec<u64>,
    input_latency_frames: Vec<u64>,
}

fn run_worker(width: u32, height: u32, busy: bool) -> WorkerStats {
    let _load = if busy {
        Some(LoadGuard::spawn_busy())
    } else {
        None
    };

    let worker = PaintWorker::spawn(EngineConfig {
        max_paint_ns: 50_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    worker.declare_actions(&["act.do"]).unwrap();
    worker
        .open_surface(
            SurfaceId::DEFAULT,
            &section_page(width, height),
            Viewport::new(width, height),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
    worker.flush().unwrap();
    let mut last_gen = worker
        .latest_frame(SurfaceId::DEFAULT, 0)
        .map(|f| f.generation)
        .unwrap_or(0);

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
            let _ = worker.latest_frame(SurfaceId::DEFAULT, last_gen);
            let host_cost = t_host0.elapsed().as_nanos() as u64;

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
                host_ns.push(host_cost);
            }
        } else {
            let _ = worker.latest_frame(SurfaceId::DEFAULT, last_gen);
            let host_cost = t_host0.elapsed().as_nanos() as u64;
            worker.flush().ok();
            if i >= WARMUP {
                host_ns.push(host_cost);
            }
        }

        if let Some(frame) = worker.latest_frame(SurfaceId::DEFAULT, last_gen) {
            if frame.generation > last_gen {
                last_gen = frame.generation;
                if i >= WARMUP {
                    paint_ns.push(frame.paint_ns);
                }
            }
        }
    }

    drop(worker);
    WorkerStats {
        host_ns,
        paint_ns,
        input_latency_frames,
    }
}

fn run_sync_engine(width: u32, height: u32) -> Vec<u64> {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    eng.declare_actions(&["act.do"]);
    eng.open_surface(
        SurfaceId::DEFAULT,
        &section_page(width, height),
        Viewport::new(width, height),
        SurfaceOpts {
            visible: true,
            pinned: true,
        },
    )
    .unwrap();
    let _ = eng.paint(Viewport::new(width, height)).unwrap();

    let mut paint_ns = Vec::with_capacity(FRAMES);
    for i in 0..(WARMUP + FRAMES) {
        let ver = (i as u64) + 1;
        let pkt = packet(ver, -((i as i32 % 40) * 3));
        let slices = MemSlices::from_packet(&pkt);
        let mut frags = Frags::from_packet(&pkt);
        eng.sync(&slices, &mut frags).unwrap();
        if i % 8 == 0 {
            let _ = eng.input(InputEvent::PointerMove {
                x: 40.0,
                y: 60.0,
                mods: Modifiers::default(),
            });
        }
        let t0 = Instant::now();
        let out = eng.paint(Viewport::new(width, height)).unwrap();
        let ns = t0.elapsed().as_nanos() as u64;
        if i >= WARMUP && !out.skipped {
            paint_ns.push(ns);
        }
    }
    paint_ns
}

fn print_worker(label: &str, width: u32, height: u32, busy: bool, stats: &WorkerStats) {
    println!();
    println!("### {label} ({width}x{height}, busy_cores={busy})");
    println!("| Metric | n | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");
    report_row("host frame (enqueue + latest_frame)", &stats.host_ns);
    report_row("worker paint (new gens)", &stats.paint_ns);
    if !stats.input_latency_frames.is_empty() {
        println!(
            "input→visible latency (frames): mean={:.2}  p95={}  p99={}  n={}",
            mean(&stats.input_latency_frames),
            pct(&stats.input_latency_frames, 0.95),
            pct(&stats.input_latency_frames, 0.99),
            stats.input_latency_frames.len(),
        );
    }
}

/// Host-side cost of fire-and-forget open / pre-drawn show / close while the
/// worker is busy painting (target: under 0.1 ms each).
fn bench_faf_controls_while_busy() {
    let worker = PaintWorker::spawn(EngineConfig {
        max_paint_ns: 50_000_000,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    });
    worker.declare_actions(&["act.do"]).unwrap();
    worker
        .open_surface(
            SurfaceId::DEFAULT,
            &section_page(1920, 1080),
            Viewport::new(1920, 1080),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
        .unwrap();
    worker.flush().unwrap();

    // Keep the worker busy with continuous syncs on a side thread.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop);
    // PaintWorker is not Clone; flood from this thread interleaved with timing.
    let mut open_ns = Vec::new();
    let mut show_ns = Vec::new();
    let mut close_ns = Vec::new();
    for i in 0..40u64 {
        worker.sync(packet(i + 1, -((i as i32 % 20) * 3))).unwrap();
        let id = SurfaceId(10 + (i % 5));
        let t0 = Instant::now();
        worker
            .open_surface(
                id,
                &section_page(640, 360),
                Viewport::new(640, 360),
                SurfaceOpts {
                    visible: false,
                    pinned: false,
                },
            )
            .unwrap();
        open_ns.push(t0.elapsed().as_nanos() as u64);

        let t1 = Instant::now();
        worker.set_visible(id, true).unwrap();
        show_ns.push(t1.elapsed().as_nanos() as u64);

        let t2 = Instant::now();
        worker.close_surface(id).unwrap();
        close_ns.push(t2.elapsed().as_nanos() as u64);
    }
    stop_c.store(true, Ordering::Relaxed);
    let _ = stop;
    worker.flush().ok();
    drop(worker);

    println!();
    println!("## Fire-and-forget host cost while worker busy (target < 0.1 ms)");
    println!("| Call | n | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");
    report_row("open_surface (hidden pre-draw)", &open_ns);
    report_row("set_visible(true) pre-drawn show", &show_ns);
    report_row("close_surface", &close_ns);
}

fn main() {
    println!("hx-native worker_bench (issue #8 step 3)");
    println!(
        "cores={}  command_capacity={}  frame_slots={}  warmup={}  frames={}",
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        PAINT_WORKER_COMMAND_CAPACITY,
        PAINT_WORKER_FRAME_SLOTS,
        WARMUP,
        FRAMES,
    );
    println!(
        "Order: per resolution, discarded prime → idle worker → busy worker → sync Engine \
         (avoids counting the first cold 1440p pass as the idle result)."
    );

    let resolutions = [
        ("720p", 1280u32, 720u32),
        ("1080p", 1920, 1080),
        ("1440p", 2560, 1440),
    ];

    for (name, w, h) in resolutions {
        println!();
        println!("## {name}");

        // Discarded prime at this resolution so idle is not the first cold pass.
        let _prime = run_worker(w, h, false);

        let idle = run_worker(w, h, false);
        print_worker(name, w, h, false, &idle);

        let busy = run_worker(w, h, true);
        print_worker(name, w, h, true, &busy);

        let sync_ns = run_sync_engine(w, h);
        println!();
        println!("### {name} sync Engine (same session)");
        println!("| Metric | n | mean ms | p95 ms | p99 ms |");
        println!("|---|---|---|---|---|");
        report_row("sync Engine paint", &sync_ns);
    }

    bench_faf_controls_while_busy();
}
