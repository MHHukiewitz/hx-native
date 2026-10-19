//! Issue #8 step 2 probe: memory and open cost per surface (one Engine ≈ one surface).
//!
//! Measures engine RGBA buffer + estimated GPU texture (w×h×4), Blitz/engine heap via a
//! counting global allocator (example-only), cold vs pre-drawn open cost, close free, and
//! compile-time `Send` checks for `BaseDocument` / paint state.
//!
//! Usage: `cargo run -p hx-native --example surface_probe --release`
//!
//! No library API change. Surfaces are simulated as separate `Engine` instances sized to
//! the window. Pre-drawn = load+paint while "hidden", then show cost is a clean skipped paint.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use hx_native::*;

/// Tracks live heap bytes by forwarding to the system allocator.
struct CountingAlloc;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: GlobalAlloc must be implemented with unsafe; we only add size tracking around
// System's proven allocator and never invent pointers or change layout rules.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::SeqCst);
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::SeqCst);
            LIVE_BYTES.fetch_add(new_size, Ordering::SeqCst);
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::SeqCst)
}

const ITERATIONS: usize = 200;
const WARMUP: usize = 5;

/// Compile-time Send probe (issue #8 / Graph step 3 prerequisite).
fn assert_send<T: Send>() {}

fn probe_send() {
    // Paint-state types that ARE Send — fail the build if that regresses.
    assert_send::<RgbaImage>();
    assert_send::<PaintOutput>();
    assert_send::<PaintStats>();
    assert_send::<Timings>();
    assert_send::<Viewport>();
    assert_send::<Rect>();

    // BaseDocument / HtmlDocument / Engine are NOT Send: stylo Device holds
    // `Box<dyn FontMetricsProvider>` without a Send bound. Confirmed by a
    // separate compile that `assert_send::<blitz_dom::BaseDocument>()` fails.
    // Uncomment to re-check (expected compile error):
    // assert_send::<blitz_dom::BaseDocument>();
    // assert_send::<blitz_html::HtmlDocument>();
    // assert_send::<Engine>();

    println!();
    println!("## Send probe (compile-time assert_send)");
    println!("| Type | Send | Notes |");
    println!("|---|---|---|");
    println!(
        "| `blitz_dom::BaseDocument` | **no** | stylo `Device` / `FontMetricsProvider` not Send |"
    );
    println!("| `blitz_html::HtmlDocument` | **no** | wraps `BaseDocument` |");
    println!("| `hx_native::Engine` | **no** | holds `HtmlDocument` |");
    println!("| `hx_native::RgbaImage` | **yes** | assert_send compiled |");
    println!("| `hx_native::PaintOutput` | **yes** | assert_send compiled |");
    println!("| `hx_native::PaintStats` | **yes** | assert_send compiled |");
    println!("| `hx_native::Timings` | **yes** | assert_send compiled |");
    println!();
    println!(
        "Step 3 implication: create documents on the worker thread; do not move them. \
         Finished frames (`RgbaImage` / damage) can cross threads."
    );
}

#[derive(Clone, Copy)]
struct SurfaceKind {
    name: &'static str,
    /// CSS (logical) width at scale 1.0; for time-strip, width follows screen.
    css_w: Option<u32>,
    css_h: u32,
}

const TIME_STRIP: SurfaceKind = SurfaceKind {
    name: "time_strip",
    css_w: None, // full screen width
    css_h: 64,
};

const MID_WIN: SurfaceKind = SurfaceKind {
    name: "mid_window",
    css_w: Some(640),
    css_h: 480,
};

const LARGE_WIN: SurfaceKind = SurfaceKind {
    name: "large_window",
    css_w: Some(1280),
    css_h: 800,
};

fn css_size(kind: SurfaceKind, screen_w: u32) -> (u32, u32) {
    let w = kind.css_w.unwrap_or(screen_w);
    (w, kind.css_h)
}

fn phys_size(css_w: u32, css_h: u32, scale: f64) -> (u32, u32) {
    (
        ((css_w as f64) * scale).round() as u32,
        ((css_h as f64) * scale).round() as u32,
    )
}

fn surface_html(kind: SurfaceKind, css_w: u32, css_h: u32) -> String {
    match kind.name {
        "time_strip" => format!(
            r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #222833; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; }}
#chips {{ position: absolute; left: 8px; top: 8px; }}
#chips button {{ height: 28px; margin-right: 4px; padding: 0 10px; background: #2d3648;
  color: #e8ecf4; border: 1px solid #556; }}
#date {{ position: absolute; left: 8px; top: 40px; font-size: 13px; }}
</style></head><body>
<div id="chips">
  <button id="chip-1x" hx-post="time.set_speed" hx-trigger="click">1x</button>
  <button id="chip-Week" class="on" hx-post="time.set_speed" hx-trigger="click">Week</button>
</div>
<div id="date">DATE 2026-01-15 · Week</div>
</body></html>"##
        ),
        _ => format!(
            r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ width: {css_w}px; height: {css_h}px; background: #2a3140; color: #e8ecf4;
  font-family: "DejaVu Sans", sans-serif; overflow: hidden; border: 1px solid #667; }}
h1 {{ font-size: 16px; padding: 10px 12px; border-bottom: 1px solid #445; }}
table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
td {{ padding: 6px 10px; border-bottom: 1px solid #445; }}
</style></head><body>
<h1 id="title">{title}</h1>
<table>
<tr><td>Alpha</td><td>42</td></tr>
<tr><td>Beta</td><td>7</td></tr>
<tr><td>Gamma</td><td>3</td></tr>
<tr><td>Delta</td><td>19</td></tr>
<tr><td>Epsilon</td><td>5</td></tr>
</table>
<button id="act" hx-post="act.do" hx-trigger="click" style="margin:12px;padding:6px 14px">Do</button>
</body></html>"##,
            title = kind.name,
            css_w = css_w,
            css_h = css_h,
        ),
    }
}

fn make_engine(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 120,
        paint_mode: PaintMode::Exact,
    });
    eng.declare_actions(&["time.set_speed", "act.do"]);
    eng.load_html(html).unwrap();
    eng
}

fn first_paint(eng: &mut Engine, phys_w: u32, phys_h: u32, scale: f64) -> PaintOutput {
    eng.paint(Viewport::with_scale(phys_w, phys_h, scale))
        .unwrap()
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn percentile(sorted_ns: &[u64], p: f64) -> u64 {
    if sorted_ns.is_empty() {
        return 0;
    }
    let idx = ((sorted_ns.len() as f64) * p) as usize;
    sorted_ns[idx.min(sorted_ns.len() - 1)]
}

fn mean_ns(samples: &[u64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<u64>() as f64 / samples.len() as f64
}

struct MemRow {
    label: String,
    phys_w: u32,
    phys_h: u32,
    scale: f64,
    buffer_bytes: usize,
    gpu_est_bytes: usize,
    heap_delta_bytes: usize,
    doc_est_bytes: usize,
    hidden: bool,
}

fn measure_memory(
    kind: SurfaceKind,
    screen_w: u32,
    screen_label: &str,
    scale: f64,
    hidden: bool,
) -> MemRow {
    let (css_w, css_h) = css_size(kind, screen_w);
    let (phys_w, phys_h) = phys_size(css_w, css_h, scale);
    let html = surface_html(kind, css_w, css_h);

    // Let transient allocs settle.
    let before = live_bytes();
    let mut eng = make_engine(&html);
    // Drop PaintOutput so its RGBA clone is not counted twice (engine keeps cached_rgba).
    drop(first_paint(&mut eng, phys_w, phys_h, scale));
    let after = live_bytes();

    let buffer_bytes = eng.last_frame().map(|f| f.pixels.len()).unwrap_or(0);
    // Hidden pre-drawn: engine keeps the RGBA buffer; host can defer the GPU texture
    // until set_visible(true). Visible surfaces pay both sides.
    let gpu_est_bytes = if hidden {
        0
    } else {
        (phys_w as usize) * (phys_h as usize) * 4
    };
    let heap_delta = after.saturating_sub(before);
    let doc_est = heap_delta.saturating_sub(buffer_bytes);

    let visibility = if hidden { "hidden" } else { "visible" };
    let label = format!(
        "{screen_label} scale={scale:.2} {} {phys_w}x{phys_h} ({visibility})",
        kind.name
    );

    // Keep `eng` alive until after measurement (hidden = still held).
    let row = MemRow {
        label,
        phys_w,
        phys_h,
        scale,
        buffer_bytes,
        gpu_est_bytes,
        heap_delta_bytes: heap_delta,
        doc_est_bytes: doc_est,
        hidden,
    };
    drop(eng);
    row
}

fn measure_open_cost(
    kind: SurfaceKind,
    screen_w: u32,
    screen_label: &str,
    scale: f64,
) -> (String, Vec<u64>, Vec<u64>) {
    let (css_w, css_h) = css_size(kind, screen_w);
    let (phys_w, phys_h) = phys_size(css_w, css_h, scale);
    let html = surface_html(kind, css_w, css_h);
    let label = format!(
        "{screen_label} scale={scale:.2} {} {phys_w}x{phys_h}",
        kind.name
    );

    let mut cold_ns = Vec::with_capacity(ITERATIONS);
    let mut show_ns = Vec::with_capacity(ITERATIONS);

    for i in 0..(WARMUP + ITERATIONS) {
        // Cold: create + parse/style/layout + first full raster on the open frame.
        let t0 = Instant::now();
        let mut eng = make_engine(&html);
        let _out = first_paint(&mut eng, phys_w, phys_h, scale);
        let cold = t0.elapsed().as_nanos() as u64;

        // Pre-drawn path: surface already painted while hidden; "show" is a clean paint
        // (skipped raster) — the open-frame cost after pre-draw.
        let t1 = Instant::now();
        let _skipped = eng
            .paint(Viewport::with_scale(phys_w, phys_h, scale))
            .unwrap();
        let show = t1.elapsed().as_nanos() as u64;
        // Confirm pre-drawn buffer is still available without re-raster.
        let _ = eng.last_frame().expect("pre-drawn buffer present");
        drop(eng);

        if i >= WARMUP {
            cold_ns.push(cold);
            show_ns.push(show);
        }
    }

    (label, cold_ns, show_ns)
}

fn measure_close(kind: SurfaceKind, screen_w: u32, scale: f64) {
    let (css_w, css_h) = css_size(kind, screen_w);
    let (phys_w, phys_h) = phys_size(css_w, css_h, scale);
    let html = surface_html(kind, css_w, css_h);

    let baseline = live_bytes();
    let mut eng = make_engine(&html);
    let _ = first_paint(&mut eng, phys_w, phys_h, scale);
    let after_open = live_bytes();
    let buffer = eng.last_frame().map(|f| f.pixels.len()).unwrap_or(0);
    drop(eng);
    let after_close = live_bytes();

    let freed = after_open.saturating_sub(after_close);
    let residual = after_close.saturating_sub(baseline);

    println!();
    println!("## Close / free (counting allocator)");
    println!(
        "Surface: {} {}x{} scale={scale:.2}",
        kind.name, phys_w, phys_h
    );
    println!(
        "| Phase | Live heap | Notes |"
    );
    println!("|---|---|---|");
    println!("| baseline | {} B ({:.3} MiB) | before create |", baseline, mb(baseline));
    println!(
        "| after open+paint | {} B ({:.3} MiB) | +{} B vs baseline; buffer={} B |",
        after_open,
        mb(after_open),
        after_open.saturating_sub(baseline),
        buffer
    );
    println!(
        "| after drop | {} B ({:.3} MiB) | freed {} B ({:.3} MiB); residual vs baseline {} B |",
        after_close,
        mb(after_close),
        freed,
        mb(freed),
        residual
    );
    if freed >= buffer && residual < buffer / 4 {
        println!(
            "Result: **buffer freed on drop** (freed ≥ buffer bytes; residual small)."
        );
    } else {
        println!(
            "Result: check residual — freed={freed} buffer={buffer} residual={residual}."
        );
    }
}

fn report_open(label: &str, cold: &mut [u64], show: &mut [u64]) {
    cold.sort_unstable();
    show.sort_unstable();
    println!(
        "| {label} | cold | {:.3} | {:.3} | {:.3} |",
        mean_ns(cold) / 1e6,
        percentile(cold, 0.95) as f64 / 1e6,
        percentile(cold, 0.99) as f64 / 1e6,
    );
    println!(
        "| {label} | pre-drawn show | {:.3} | {:.3} | {:.3} |",
        mean_ns(show) / 1e6,
        percentile(show, 0.95) as f64 / 1e6,
        percentile(show, 0.99) as f64 / 1e6,
    );
}

fn cpu_info() -> (String, usize) {
    let model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let vcpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    (model, vcpus)
}

fn main() {
    let (cpu, vcpus) = cpu_info();
    println!("# hx-native surface probe (issue #8 step 2)");
    println!();
    println!("CPU: {cpu}");
    println!("vCPUs: {vcpus}");
    println!("Build: release");
    println!("Iterations: {ITERATIONS} (warmup {WARMUP})");
    println!("Blitz: =0.3.0-beta.4 (pinned)");
    println!();
    println!(
        "One surface ≈ one `Engine` (current API). Buffer = engine RGBA (`w*h*4`). \
         GPU texture est. = same size (Rgba8) when visible; 0 when hidden (host can \
         defer upload). Doc heap est. = counting-allocator delta minus buffer."
    );

    probe_send();

    // --- Memory ---
    println!();
    println!("## Memory per surface");
    println!(
        "| Surface | Phys px | Scale | Hidden | Engine RGBA | GPU tex est. | Heap Δ | Doc/engine est. |"
    );
    println!("|---|---|---|---|---|---|---|---|");

    let screens = [("1080p", 1920u32), ("1440p", 2560u32)];
    let scales = [1.0_f64, 1.25];
    let kinds = [TIME_STRIP, MID_WIN, LARGE_WIN];

    let mut mem_rows: Vec<MemRow> = Vec::new();
    for (screen_label, screen_w) in screens {
        for &scale in &scales {
            for kind in kinds {
                let row = measure_memory(kind, screen_w, screen_label, scale, false);
                println!(
                    "| {} | {}x{} | {:.2} | no | {:.3} MiB | {:.3} MiB | {:.3} MiB | {:.3} MiB |",
                    kind.name,
                    row.phys_w,
                    row.phys_h,
                    scale,
                    mb(row.buffer_bytes),
                    mb(row.gpu_est_bytes),
                    mb(row.heap_delta_bytes),
                    mb(row.doc_est_bytes),
                );
                mem_rows.push(row);

                let hidden = measure_memory(kind, screen_w, screen_label, scale, true);
                println!(
                    "| {} | {}x{} | {:.2} | **yes** | {:.3} MiB | {:.3} MiB | {:.3} MiB | {:.3} MiB |",
                    kind.name,
                    hidden.phys_w,
                    hidden.phys_h,
                    scale,
                    mb(hidden.buffer_bytes),
                    mb(hidden.gpu_est_bytes),
                    mb(hidden.heap_delta_bytes),
                    mb(hidden.doc_est_bytes),
                );
                mem_rows.push(hidden);
            }
        }
    }

    // Typical set: chrome (strip) + 3 mid windows at 1080p / scale 1.0, all visible.
    // Worst allowed: chrome + 3 large at 1440p / scale 1.25, plus one hidden large pre-drawn.
    println!();
    println!("## Typical / worst set totals (engine RGBA + GPU est. per surface)");
    fn sum_pair(rows: &[&MemRow]) -> (usize, usize, usize) {
        let buf: usize = rows.iter().map(|r| r.buffer_bytes).sum();
        let gpu: usize = rows.iter().map(|r| r.gpu_est_bytes).sum();
        let heap: usize = rows.iter().map(|r| r.heap_delta_bytes).sum();
        (buf, gpu, heap)
    }

    let pick = |name: &str, screen: &str, scale: f64, hidden: bool| -> &MemRow {
        mem_rows
            .iter()
            .find(|r| {
                r.label.contains(name)
                    && r.label.contains(screen)
                    && (r.scale - scale).abs() < 1e-9
                    && r.hidden == hidden
            })
            .unwrap_or_else(|| panic!("missing row {name} {screen} {scale} hidden={hidden}"))
    };

    let typical = [
        pick("time_strip", "1080p", 1.0, false),
        pick("mid_window", "1080p", 1.0, false),
        pick("mid_window", "1080p", 1.0, false),
        pick("mid_window", "1080p", 1.0, false),
    ];
    let (t_buf, t_gpu, t_heap) = sum_pair(&typical);
    println!(
        "- **Typical** (1080p scale 1.0: strip + 3 mid): RGBA {:.3} MiB + GPU {:.3} MiB = **{:.3} MiB** host+GPU; heap Δ {:.3} MiB",
        mb(t_buf),
        mb(t_gpu),
        mb(t_buf + t_gpu),
        mb(t_heap),
    );

    let worst = [
        pick("time_strip", "1440p", 1.25, false),
        pick("large_window", "1440p", 1.25, false),
        pick("large_window", "1440p", 1.25, false),
        pick("large_window", "1440p", 1.25, false),
        pick("large_window", "1440p", 1.25, true), // one pre-drawn hidden
    ];
    let (w_buf, w_gpu, w_heap) = sum_pair(&worst);
    println!(
        "- **Worst** (1440p scale 1.25: strip + 3 large visible + 1 large hidden pre-drawn): RGBA {:.3} MiB + GPU {:.3} MiB = **{:.3} MiB** host+GPU; heap Δ {:.3} MiB",
        mb(w_buf),
        mb(w_gpu),
        mb(w_buf + w_gpu),
        mb(w_heap),
    );

    // --- Open cost ---
    println!();
    println!("## Open cost (cold create+paint vs pre-drawn show)");
    println!("| Surface | Path | mean ms | p95 ms | p99 ms |");
    println!("|---|---|---|---|---|");

    for (screen_label, screen_w) in screens {
        for &scale in &scales {
            for kind in [MID_WIN, LARGE_WIN, TIME_STRIP] {
                let (label, mut cold, mut show) =
                    measure_open_cost(kind, screen_w, screen_label, scale);
                report_open(&label, &mut cold, &mut show);
            }
        }
    }

    // --- Close ---
    measure_close(MID_WIN, 1920, 1.0);
    measure_close(LARGE_WIN, 2560, 1.25);

    println!();
    println!("## API shape recommendation (not implemented)");
    println!(
        "Recommend: `Engine::open_surface(id, html, viewport, visible) -> Result<()>`, \
         `set_visible(id, bool)`, `resize_surface(id, viewport)`, `close_surface(id)`, \
         `paint(id) -> PaintOutput` (or `paint_all` returning z-ordered outputs). \
         Fixed chrome is one surface; each HTML window is another. Host owns position/z-order. \
         `visible: false` still runs style/layout/first raster (pre-draw). \
         `close_surface` drops document + RGBA immediately (no pool unless reopen bench says so). \
         Because `BaseDocument` is not `Send`, step 3 should create surfaces on the worker."
    );
}
