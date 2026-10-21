//! Scratch repro (Mainnet #770 design review): four Win95 paint gaps.
//! Rules copied from Mainnet docs/design/mocks/target/_tokens.css, desk98.css
//! and 53-desk-exit-lease-confirm.html at 097b1928. Writes raw RGBA dumps to
//! $PAINT_GAPS_OUT (default /tmp/paint-gaps) for the Chrome comparison.

use std::sync::Arc;
use hx_native::*;

const REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("fonts/DejaVuSans-Bold.ttf");

const KIT: &str = ":root{--bg:#c0c0c0;--hi:#ffffff;--edge-hi:#ffffff;--edge-lt:#dfdfdf;--edge-sh:#808080;--edge-dk:#000000;\
--press:inset -1px -1px var(--edge-hi),inset 1px 1px var(--edge-dk),inset -2px -2px var(--edge-lt),inset 2px 2px var(--edge-sh);\
--dither:var(--bg) repeating-conic-gradient(var(--edge-hi) 0 25%,var(--bg) 0 50%) 0 0/2px 2px;--toggle-on:var(--dither);\
--font:\"MS Sans Serif\",Arial,sans-serif;--dis-ink:#808080;--dis-etch:1px 1px 0 var(--edge-hi)}\
html,body{margin:0;background:var(--bg)}body{font-family:var(--font);font-size:11px;color:#000}\
button{display:block;margin:0;border:0;padding:0;font:inherit;color:inherit;background:var(--bg)}";

fn cfg(fonts: Vec<FontFace>) -> EngineConfig {
    EngineConfig { max_paint_ns: 200_000_000, pending_timeout_frames: 5, paint_mode: PaintMode::Exact, fonts, ..EngineConfig::default() }
}

fn paint(fonts: Vec<FontFace>, w: u32, h: u32, css: &str, body: &str) -> RgbaImage {
    let html = format!("<!DOCTYPE html><html><head><style>{KIT}{css}</style></head><body>{body}</body></html>");
    let mut e = Engine::try_new(cfg(fonts)).expect("engine");
    e.load_html(&html).unwrap();
    let out = e.paint(Viewport { width: w, height: h, scale: 1.0, holes: vec![] }).unwrap();
    out.rgba.unwrap_or_else(|| e.last_frame().expect("frame").clone())
}

fn dump(name: &str, img: &RgbaImage) {
    let dir = std::env::var("PAINT_GAPS_OUT").unwrap_or_else(|_| "/tmp/paint-gaps".into());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(format!("{dir}/{name}.{}x{}.rgba", img.width, img.height), &img.pixels).unwrap();
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
}

/// Count of each colour inside the rect.
fn census(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<([u8; 3], usize)> {
    let mut m: std::collections::BTreeMap<[u8; 3], usize> = Default::default();
    for y in y0..y1 { for x in x0..x1 { *m.entry(px(img, x, y)).or_default() += 1; } }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// (a)/(b) Dither: `button.on{background:var(--toggle-on)}` on a 28x18 ⋯
/// button (126 tiles) and a 40x20 Pause chip (200 tiles); the same dither with
/// already-increasing stops; and the kit stops on a 200x18 bar (900 tiles).
#[test]
fn a_dither() {
    let css = ".b{position:absolute;box-shadow:var(--press)}.on{background:var(--toggle-on)}\
        .mono{background:var(--bg) repeating-conic-gradient(var(--edge-hi) 0deg 90deg,var(--bg) 90deg 180deg) 0 0/2px 2px}";
    let body = "<button class='b on' style='left:4px;top:4px;width:28px;height:18px'></button>\
        <button class='b on' style='left:40px;top:4px;width:40px;height:20px'></button>\
        <button class='b mono' style='left:88px;top:4px;width:28px;height:18px'></button>\
        <button class='b on' style='left:4px;top:30px;width:200px;height:18px'></button>\
        <button class='b mono' style='left:4px;top:52px;width:200px;height:18px'></button>";
    let img = paint(vec![], 220, 76, css, body);
    dump("a_dither", &img);
    // Interior (inside the 2px bevel) of each box.
    for (name, r) in [("more.on 28x18 kit stops", (6, 6, 30, 20)), ("pause.on 40x20 kit stops", (42, 6, 78, 22)),
        ("28x18 increasing stops", (90, 6, 114, 20)), ("200x18 kit stops", (6, 32, 202, 46)), ("200x18 increasing stops", (6, 54, 202, 68))] {
        println!("{name}: {:?}", census(&img, r.0, r.1, r.2, r.3));
    }
}

/// (c) Bold: `.ask-q{font-weight:700}` in the kit family stack, with the
/// engine config Mainnet uses today (no host fonts), with only a regular host
/// face, and with a real 700 face registered.
#[test]
fn c_bold() {
    let css = "p{margin:2px 4px;font-size:11px}";
    let body = "<p>Exit this lease?</p><p style='font-weight:700'>Exit this lease?</p>";
    let ink = |img: &RgbaImage, y0, y1| census(img, 0, y0, img.width, y1).iter().filter(|(c, _)| c[0] < 96).map(|(_, n)| n).sum::<usize>();
    let face = |w, b: &'static [u8]| FontFace::new("MS Sans Serif", w, FontStyle::Normal, Arc::<[u8]>::from(b));
    for (tag, name, fonts) in [("none", "no host fonts (Mainnet today)", vec![]), ("r400", "host 400 only", vec![face(400, REGULAR)]),
        ("r400b700", "host 400+700", vec![face(400, REGULAR), face(700, BOLD)])] {
        let img = paint(fonts, 120, 34, css, body);
        dump(&format!("c_bold_{tag}"), &img);
        println!("{name}: regular ink {}, bold ink {}", ink(&img, 0, 17), ink(&img, 17, 34));
    }
}

/// (d) Emboss: `button[aria-disabled=true]{color:var(--ink4);text-shadow:1px 1px var(--hi)}`
/// and desk98 `button:disabled{text-shadow:var(--dis-etch)}`.
#[test]
fn d_emboss() {
    let css = "button{position:absolute;left:4px;padding:2px 6px;font-size:11px}\
        button[aria-disabled=true]{color:#808080;text-shadow:1px 1px var(--hi)}\
        button:disabled{color:var(--dis-ink);text-shadow:var(--dis-etch)}";
    let body = "<button aria-disabled='true' style='top:4px'>Confirm exit</button>\
        <button disabled style='top:24px'>Confirm exit</button>\
        <button style='top:44px;color:#808080'>Confirm exit</button>";
    let img = paint(vec![], 100, 64, css, body);
    dump("d_emboss", &img);
    for (name, y0) in [("aria-disabled + text-shadow", 4), (":disabled + --dis-etch", 24), ("grey, no shadow", 44)] {
        let c = census(&img, 4, y0, 100, y0 + 18);
        let white = c.iter().filter(|(k, _)| *k == [255, 255, 255]).map(|(_, n)| n).sum::<usize>();
        println!("{name}: white px {white}, colours {:?}", &c[..c.len().min(4)]);
    }
}
