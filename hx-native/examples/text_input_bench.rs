//! Keystroke paint cost and caret-blink damage-rect size (issue #17).
//!
//! Usage: `cargo run -p hx-native --example text_input_bench --release`

use hx_native::*;

fn main() {
    let html = r#"
<style>
html,body{margin:0;background:#222}
#t{position:absolute;left:16px;top:16px;width:240px;height:28px;font-size:16px}
</style>
<input id="t" type="text" name="t" value="" />
"#;
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.load_html(html).expect("load");
    let vp = Viewport::new(400, 120);
    let _ = eng.paint(vp.clone()).expect("first paint");
    eng.test_set_focus("t");
    let _ = eng.paint(vp.clone()).expect("focus paint");

    const N: usize = 50;
    let mut keystroke_ns = Vec::with_capacity(N);
    for i in 0..N {
        let t0 = std::time::Instant::now();
        let _ = eng.input(InputEvent::Text(
            ((b'a' + (i % 26) as u8) as char).to_string(),
        ));
        let out = eng.paint(vp.clone()).expect("keystroke paint");
        keystroke_ns.push(t0.elapsed().as_nanos() as u64);
        assert!(!out.skipped, "keystroke must dirty paint");
    }
    keystroke_ns.sort_unstable();
    let mean = keystroke_ns.iter().sum::<u64>() / N as u64;
    let p95 = keystroke_ns[(N as f64 * 0.95) as usize];
    println!("keystroke+paint  n={N}  mean_ns={mean}  p95_ns={p95}");

    // Fresh focus so caret geometry is warm, then force one blink phase flip.
    eng.test_set_focus("t");
    let _ = eng.paint(vp.clone()).expect("pre-blink");
    eng.test_force_caret_blink();
    let blink = eng.paint(vp).expect("blink paint");
    let area: u64 = blink
        .damage
        .iter()
        .map(|r| (r.width.ceil() as u64) * (r.height.ceil() as u64))
        .sum();
    let surface = 400u64 * 120u64;
    let stats = eng.last_paint_stats();
    println!(
        "caret_blink  damage_rects={}  damage_area={area}  surface={surface}  full_repaint={}  damage={:?}",
        blink.damage.len(),
        stats.full_repaint,
        blink.damage
    );
    assert!(!stats.full_repaint, "blink must not full-repaint");
    assert!(
        area < surface / 2,
        "blink damage must be under half surface"
    );
}
