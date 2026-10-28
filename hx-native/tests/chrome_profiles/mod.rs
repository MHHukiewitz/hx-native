//! Chrome reference pixels for hard inset bevels (issue #125).
//!
//! `fixtures/bevel125_chrome.txt` holds short pixel profiles of the fixture
//! pages as headless Chrome paints them at 1, 1.25, 1.5 and 2 (made by
//! `fixtures/bevel125_chrome.py`): from each edge's middle inward and along
//! each corner's diagonal, for boxes on whole device pixels at that scale.

#![allow(dead_code)]

const DATA: &str = include_str!("../fixtures/bevel125_chrome.txt");

/// Per-channel tolerance: Chrome (Skia) and vello round blends differently.
pub const TOLERANCE: u8 = 3;

pub struct Profile {
    pub page: &'static str,
    pub name: &'static str,
    pub scale: f64,
    pub edge: &'static str,
    pub x: i64,
    pub y: i64,
    pub dx: i64,
    pub dy: i64,
    pub colours: Vec<[u8; 3]>,
}

fn hex(s: &str) -> [u8; 3] {
    let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap();
    [v(0), v(2), v(4)]
}

/// Every profile of `page` at `scale`.
pub fn profiles(page: &str, scale: f64) -> Vec<Profile> {
    DATA.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&'static str> = l.split_whitespace().collect();
            let n = |i: usize| f[i].parse::<i64>().unwrap();
            Profile {
                page: f[0],
                name: f[1],
                scale: f[2].parse().unwrap(),
                edge: f[3],
                x: n(4),
                y: n(5),
                dx: n(6),
                dy: n(7),
                colours: f[8..].iter().map(|c| hex(c)).collect(),
            }
        })
        .filter(|p| p.page == page && p.scale == scale)
        .collect()
}

/// The frame's colours along `p` (RGBA frame, `width` px wide).
pub fn sample(p: &Profile, frame: &[u8], width: usize) -> Vec<[u8; 3]> {
    (0..p.colours.len() as i64)
        .map(|i| {
            let (x, y) = ((p.x + i * p.dx) as usize, (p.y + i * p.dy) as usize);
            let o = (y * width + x) * 4;
            [frame[o], frame[o + 1], frame[o + 2]]
        })
        .collect()
}

pub fn hexes(c: &[[u8; 3]]) -> String {
    c.iter()
        .map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
        .collect::<Vec<_>>()
        .join(" ")
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|k| a[k].abs_diff(b[k]) <= TOLERANCE)
}

/// Pixels from the outer edge that differ (beyond [`TOLERANCE`]) from the
/// profile's innermost one: the bevel width in device px along it. The last
/// sample is left out, since the window's title bar starts there (4 CSS px
/// in) and the profile is `ceil(4 * scale) + 1` deep.
pub fn width(c: &[[u8; 3]]) -> usize {
    let c = &c[..c.len() - 1];
    let inner = c[c.len() - 1];
    c.iter()
        .rposition(|&p| !near(p, inner))
        .map_or(0, |i| i + 1)
}

/// One line per profile of `page` at `scale` (boxes in `only`, or all when
/// empty) where the frame is off Chrome by more than [`TOLERANCE`].
pub fn mismatches(
    page: &str,
    scale: f64,
    only: &[&str],
    frame: &[u8],
    width: usize,
) -> Vec<String> {
    profiles(page, scale)
        .iter()
        .filter(|p| only.is_empty() || only.contains(&p.name))
        .filter_map(|p| {
            let got = sample(p, frame, width);
            let off = got.iter().zip(&p.colours).any(|(&a, &b)| !near(a, b));
            off.then(|| {
                format!(
                    "{} {} @{}: hx [{}] chrome [{}]",
                    p.name,
                    p.edge,
                    p.scale,
                    hexes(&got),
                    hexes(&p.colours)
                )
            })
        })
        .collect()
}
