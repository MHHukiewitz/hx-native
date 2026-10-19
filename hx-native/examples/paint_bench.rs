//! Week-speed paint bench for issue #8 step 2 (multi-surface).
//!
//! Runs a scripted sequence at 720p, 1080p and 1440p in Exact mode: ticker,
//! hover, action greying, slice swaps, and opening a window during a slice
//! update. Reports mean / p95 / p99 (eviction time is included in those
//! percentiles when eviction runs), eviction counts, pinned/live memory
//! estimates, and cold / pre-drawn / evicted-then-reopened open paths.
//!
//! Usage: `cargo run -p hx-native --example paint_bench --release`

use std::collections::HashMap;

use hx_native::*;

const WARMUP: usize = 10;
const FRAMES: usize = 200;

const CHROME: SurfaceId = SurfaceId(1);
const WIN: SurfaceId = SurfaceId(2);

struct MemSlices {
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
}

impl MemSlices {
    fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert("time".into(), 1);
        versions.insert("actions".into(), 1);
        versions.insert("hud".into(), 1);
        versions.insert("ticker".into(), 1);
        Self {
            versions,
            actions: Vec::new(),
        }
    }
    fn bump(&mut self, name: &str) {
        *self.versions.entry(name.into()).or_insert(0) += 1;
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
    fn new() -> Self {
        Self {
            bodies: HashMap::new(),
        }
    }
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

fn chrome_page(css_w: u32, css_h: u32) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #1a1f2a; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }}
#strip {{ position: absolute; left: 0; top: 0; width: {css_w}px; height: 80px; background: #222833; }}
#chips {{ position: absolute; left: 8px; top: 8px; height: 36px; }}
#chips button {{ height: 32px; margin-right: 4px; padding: 0 12px; background: #2d3648;
  color: #e8ecf4; border: 1px solid #556; position: relative; }}
#chips button.on {{ background: #3d7ea6; }}
#date {{ position: absolute; left: 8px; top: 48px; font-size: 14px; }}
#ticker-lane {{
  position: absolute; left: 0; top: {ticker_top}px; width: {css_w}px; height: 32px;
  overflow: hidden; background: #141820;
}}
#ticker-inner {{ position: absolute; left: 0; top: 4px; white-space: nowrap; font-size: 14px; }}
#act {{ position: absolute; left: {act_x}px; top: 20px; width: 120px; height: 32px;
  background: #2d3648; color: #e8ecf4; border: 1px solid #556; }}
</style></head><body>
<div id="strip"></div>
<section id="sec-chips" hx-trigger="slice:time" hx-target="#chips" hx-swap="innerHTML">
  <div id="chips"></div>
</section>
<section id="sec-date" hx-trigger="slice:time" hx-target="#date" hx-swap="innerHTML">
  <div id="date">DATE</div>
</section>
<section id="sec-ticker" hx-trigger="slice:ticker" hx-target="#ticker-inner" hx-swap="innerHTML">
  <div id="ticker-lane"><div id="ticker-inner">news</div></div>
</section>
<button id="act" hx-post="act.do" hx-trigger="click">Do</button>
</body></html>"##,
        css_w = css_w,
        css_h = css_h,
        ticker_top = css_h.saturating_sub(40),
        act_x = css_w.saturating_sub(160),
    )
}

fn win_page(cell: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: 560px; height: 360px; background: #2a3140; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; border: 1px solid #667; }}
table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
td {{ padding: 6px 10px; border-bottom: 1px solid #445; }}
</style></head><body>
<table>
<tr><td>{cell}</td><td>42</td></tr>
<tr><td>Beta</td><td>7</td></tr>
<tr><td>Gamma</td><td>3</td></tr>
</table>
</body></html>"##
    )
}

fn chips(active: &str) -> String {
    let mut s = String::new();
    for b in ["1x", "2x", "5x", "Week", "Month"] {
        let on = if b == active { " on" } else { "" };
        s.push_str(&format!(
            r#"<button id="chip-{b}" class="{on}" hx-post="time.set_speed" hx-trigger="click">{b}</button>"#
        ));
    }
    s
}

fn date(n: u32) -> String {
    format!(r#"<span id="date-label">DATE 2026-01-{n:02} · Week</span>"#)
}

fn ticker(offset: i32) -> String {
    format!(
        r#"<span id="ticker-text" style="margin-left:{offset}px">Breaking · Port delay · Week tick · Sim · Markets · News crawl · Extra headline</span>"#
    )
}

#[derive(Default)]
struct Acc {
    times_ns: Vec<u64>,
    merge_to_bbox: u32,
    full_repaint: u32,
    full_area_fallback: u32,
    frames: u32,
    evictions: u64,
    eviction_ns_samples: Vec<u64>,
}

impl Acc {
    fn record(&mut self, eng: &Engine, frame_ns: u64) {
        self.times_ns.push(frame_ns);
        self.frames += 1;
        let s = eng.last_paint_stats();
        if s.merged_to_bbox {
            self.merge_to_bbox += 1;
        }
        if s.full_repaint {
            self.full_repaint += 1;
        }
        if s.full_repaint_area_fallback {
            self.full_area_fallback += 1;
        }
        let mem = eng.surface_memory_stats();
        if mem.eviction_count > self.evictions {
            self.eviction_ns_samples.push(mem.last_eviction_ns);
            self.evictions = mem.eviction_count;
        }
    }

    fn report(&mut self, label: &str, eng: &Engine) {
        self.times_ns.sort_unstable();
        let n = self.times_ns.len();
        let mean = self.times_ns.iter().sum::<u64>() as f64 / n as f64;
        let p95 = self.times_ns[((n as f64) * 0.95) as usize].min(self.times_ns[n - 1]);
        let p99 = self.times_ns[((n as f64) * 0.99) as usize].min(self.times_ns[n - 1]);
        let mem = eng.surface_memory_stats();
        println!(
            "{label}: mean={:.3} ms  p95={:.3} ms  p99={:.3} ms  frames={}  merge_to_bbox={}  full_repaint={}  area_fallback={}  evictions={}  last_eviction={:.3} ms  pinned={:.2} MiB  live={:.2} MiB",
            mean / 1e6,
            p95 as f64 / 1e6,
            p99 as f64 / 1e6,
            self.frames,
            self.merge_to_bbox,
            self.full_repaint,
            self.full_area_fallback,
            self.evictions,
            mem.last_eviction_ns as f64 / 1e6,
            mem.pinned_bytes as f64 / (1024.0 * 1024.0),
            mem.live_bytes as f64 / (1024.0 * 1024.0),
        );
    }
}

/// Scripted Week-speed frame. Opening a window during a slice update uses a
/// separate surface (step 2). Frame time includes any eviction done in
/// open/hide/resize paths that run before paint.
fn step_sequence(
    i: usize,
    eng: &mut Engine,
    slices: &mut MemSlices,
    frags: &mut Frags,
    win_view: &Viewport,
) -> u64 {
    let t0 = std::time::Instant::now();
    match i % 8 {
        0 => {
            slices.bump("ticker");
            frags.set("sec-ticker", &ticker(-((i as i32 % 40) * 3)));
            eng.sync_surface(CHROME, slices, frags).unwrap();
        }
        1 => {
            let _ = eng.input_surface(
                CHROME,
                InputEvent::PointerMove {
                    x: 48.0,
                    y: 22.0,
                    mods: Modifiers::default(),
                },
            );
        }
        2 => {
            let _ = eng.input_surface(CHROME, InputEvent::PointerLeave);
        }
        3 => {
            slices.actions = vec![(
                "act.do".into(),
                ActionState::Disabled {
                    reason: "Sim holding".into(),
                },
            )];
            slices.bump("actions");
            eng.sync_surface(CHROME, slices, frags).unwrap();
        }
        4 => {
            slices.actions.clear();
            slices.bump("actions");
            eng.sync_surface(CHROME, slices, frags).unwrap();
        }
        5 => {
            let active = ["1x", "2x", "Week", "Month"][i % 4];
            slices.bump("time");
            frags.set("sec-chips", &chips(active));
            frags.set("sec-date", &date((i % 28) as u32 + 1));
            eng.sync_surface(CHROME, slices, frags).unwrap();
        }
        6 => {
            // Window opens during a sim tick (time bump + open_surface together).
            slices.bump("time");
            frags.set("sec-chips", &chips("Week"));
            frags.set("sec-date", &date(15));
            eng.sync_surface(CHROME, slices, frags).unwrap();
            let _ = eng.close_surface(WIN);
            eng.open_surface(
                WIN,
                &win_page(&format!("Cell {i}")),
                win_view.clone(),
                SurfaceOpts {
                    visible: true,
                    pinned: false,
                },
            )
            .unwrap();
        }
        _ => {
            // Hide the window (may evict under a tight budget in dedicated runs).
            if eng.surface_info(WIN).is_some() {
                eng.set_visible(WIN, false).unwrap();
            }
        }
    }
    for id in eng.dirty_surfaces() {
        let _ = eng.paint_surface(id).unwrap();
    }
    // Always paint chrome so skipped frames are still timed.
    if !eng.dirty_surfaces().contains(&CHROME) {
        let _ = eng.paint_surface(CHROME).unwrap();
    }
    t0.elapsed().as_nanos() as u64
}

fn boot_engine(phys_w: u32, phys_h: u32, budget: u64) -> (Engine, MemSlices, Frags, Viewport) {
    let css_w = phys_w;
    let css_h = phys_h;
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
        hidden_surface_budget_bytes: budget,
    });
    eng.declare_actions(&["time.set_speed", "act.do", "win.open"]);
    let chrome_view = Viewport::with_scale(phys_w, phys_h, 1.0);
    eng.open_surface(
        CHROME,
        &chrome_page(css_w, css_h),
        chrome_view,
        SurfaceOpts {
            visible: true,
            pinned: true,
        },
    )
    .unwrap();

    let slices = MemSlices::new();
    let mut frags = Frags::new();
    frags.set("sec-chips", &chips("1x"));
    frags.set("sec-date", &date(1));
    frags.set("sec-ticker", &ticker(0));
    eng.sync_surface(CHROME, &slices, &mut frags).unwrap();
    let _ = eng.paint_surface(CHROME).unwrap();

    let win_view = Viewport::with_scale(560, 360, 1.0);
    (eng, slices, frags, win_view)
}

fn bench_resolution(label: &str, phys_w: u32, phys_h: u32) {
    let (mut eng, mut slices, mut frags, win_view) =
        boot_engine(phys_w, phys_h, DEFAULT_HIDDEN_SURFACE_BUDGET_BYTES);

    let mut acc = Acc::default();
    for i in 0..(WARMUP + FRAMES) {
        let frame_ns = step_sequence(i, &mut eng, &mut slices, &mut frags, &win_view);
        if i >= WARMUP {
            acc.record(&eng, frame_ns);
        }
    }
    acc.report(label, &eng);
}

fn bench_open_paths(phys_w: u32, phys_h: u32) {
    let win_view = Viewport::with_scale(560, 360, 1.0);
    let html = win_page("open-path");
    const N: usize = 40;
    const W: usize = 5;

    let mut cold = Vec::new();
    let mut predrawn = Vec::new();
    let mut reopened = Vec::new();

    for i in 0..(W + N) {
        // Cold open + first paint.
        let mut eng = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        eng.declare_actions(&["act.do"]);
        let t0 = std::time::Instant::now();
        eng.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: true,
                pinned: false,
            },
        )
        .unwrap();
        let _ = eng.paint_surface(WIN).unwrap();
        let cold_ns = t0.elapsed().as_nanos() as u64;

        // Pre-drawn: open hidden (first raster inside open), then show + paint.
        let mut eng2 = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            ..Default::default()
        });
        eng2.declare_actions(&["act.do"]);
        eng2.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        let t1 = std::time::Instant::now();
        eng2.set_visible(WIN, true).unwrap();
        let _ = eng2.paint_surface(WIN).unwrap();
        let pre_ns = t1.elapsed().as_nanos() as u64;

        // Evicted then reopened: force eviction with a tiny budget, then show.
        let tiny = estimate_surface_engine_bytes(560, 360, &html) / 2;
        let mut eng3 = Engine::new(EngineConfig {
            max_paint_ns: 50_000_000,
            paint_mode: PaintMode::Exact,
            hidden_surface_budget_bytes: tiny.max(1),
            ..Default::default()
        });
        eng3.declare_actions(&["act.do"]);
        eng3.open_surface(
            WIN,
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        // Second surface forces eviction of WIN.
        eng3.open_surface(
            SurfaceId(9),
            &html,
            win_view.clone(),
            SurfaceOpts {
                visible: false,
                pinned: false,
            },
        )
        .unwrap();
        assert!(eng3.surface_info(WIN).unwrap().evicted);
        let t2 = std::time::Instant::now();
        eng3.set_visible(WIN, true).unwrap();
        let _ = eng3.paint_surface(WIN).unwrap();
        let reopen_ns = t2.elapsed().as_nanos() as u64;

        if i >= W {
            cold.push(cold_ns);
            predrawn.push(pre_ns);
            reopened.push(reopen_ns);
        }
        let _ = (eng, eng2, eng3, phys_w, phys_h);
    }

    fn report(name: &str, samples: &mut [u64]) {
        samples.sort_unstable();
        let n = samples.len();
        let mean = samples.iter().sum::<u64>() as f64 / n as f64;
        let p95 = samples[((n as f64) * 0.95) as usize].min(samples[n - 1]);
        let p99 = samples[((n as f64) * 0.99) as usize].min(samples[n - 1]);
        println!(
            "  {name}: mean={:.3} ms  p95={:.3} ms  p99={:.3} ms",
            mean / 1e6,
            p95 as f64 / 1e6,
            p99 as f64 / 1e6
        );
    }

    println!("Open paths at {phys_w}x{phys_h} window 560x360:");
    report("cold", &mut cold);
    report("pre-drawn show", &mut predrawn);
    report("evicted-then-reopened", &mut reopened);
}

fn bench_eviction_pressure(phys_w: u32, phys_h: u32) {
    // Tiny budget so hide/open paths pay eviction; times fold into p95/p99.
    let html = win_page("evac");
    let one = estimate_surface_engine_bytes(560, 360, &html);
    let (mut eng, mut slices, mut frags, win_view) = boot_engine(phys_w, phys_h, one + 1024);

    let mut acc = Acc::default();
    for i in 0..(WARMUP + FRAMES) {
        // Cycle several hidden windows to force LRU eviction every few frames.
        if i % 3 == 0 {
            let sid = SurfaceId(100 + (i % 5) as u64);
            let _ = eng.close_surface(sid);
            eng.open_surface(
                sid,
                &html,
                win_view.clone(),
                SurfaceOpts {
                    visible: false,
                    pinned: false,
                },
            )
            .unwrap();
        }
        let frame_ns = step_sequence(i, &mut eng, &mut slices, &mut frags, &win_view);
        if i >= WARMUP {
            acc.record(&eng, frame_ns);
        }
    }
    println!("Eviction-pressure run (budget ≈ one mid window):");
    acc.report(&format!("{phys_w}x{phys_h}"), &eng);
}

fn main() {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    println!("paint_bench step 2 (Exact mode, multi-surface) — CPU: {cpu}");
    println!("warmup={WARMUP} measured_frames={FRAMES}");
    println!();
    bench_resolution("720p  (1280x720)", 1280, 720);
    bench_resolution("1080p (1920x1080)", 1920, 1080);
    bench_resolution("1440p (2560x1440)", 2560, 1440);
    println!();
    bench_open_paths(1920, 1080);
    println!();
    bench_eviction_pressure(1920, 1080);
}
