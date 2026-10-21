//! Engine-drawn Win95 `<select>` drop list (issue #14).
//!
//! Markup stays a plain `<select>` / `<option>`. Blitz beta.4 hides options
//! (`option { display: none }`) via the UA sheet from [`select_ua_stylesheet`]
//! and has no click arm, so the engine owns the closed face (selected text + ▼)
//! and the open list overlay. See `docs/BLITZ_PIN.md` for the pin-sensitivity
//! note on that `option{display:none}` assumption.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use blitz_dom::{DocumentConfig, NodeId, StyleThreading, UserAgentStylesheet};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};

use crate::damage::IntRect;
use crate::form_controls::{
    select_options, select_selected_index, select_selected_text, FormControlRef,
};
use crate::paint::paint_rgba;
use crate::types::RgbaImage;

type SelectRowCacheKey = (String, String, String, u32, u32);
type SelectRowCache = HashMap<SelectRowCacheKey, RgbaImage>;

/// Cache of rasterised select-row labels (text + colours + size → RGBA).
static SELECT_ROW_CACHE: LazyLock<Mutex<SelectRowCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Max visible rows in the open drop list (no scrolling in v1).
pub const SELECT_MAX_ROWS: usize = 8;

/// Row height in CSS pixels (Win95-ish).
pub const SELECT_ROW_H: f32 = 18.0;

/// Width of the ▼ button strip on the closed control.
pub const SELECT_BUTTON_W: f32 = 16.0;

/// Win95 palette as RGBA8.
const NAVY: [u8; 4] = [0x00, 0x00, 0x80, 0xFF];
const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const FACE: [u8; 4] = [0xC0, 0xC0, 0xC0, 0xFF];
const BLACK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
const WHITE_FACE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const SHADOW: [u8; 4] = [0x80, 0x80, 0x80, 0xFF];

/// Extra UA CSS so the selected option's text shows inside the closed control.
pub fn select_ua_stylesheet() -> UserAgentStylesheet {
    UserAgentStylesheet::parse(
        r#"
select {
    display: inline-block;
    box-sizing: border-box;
    min-height: 20px;
    min-width: 64px;
    padding: 1px 18px 1px 4px;
    border-top: 2px solid #808080;
    border-left: 2px solid #808080;
    border-right: 2px solid #ffffff;
    border-bottom: 2px solid #ffffff;
    background-color: #ffffff;
    color: #000000;
    font-family: sans-serif;
    font-size: 12px;
    vertical-align: middle;
    overflow: hidden;
    white-space: nowrap;
}
option {
    display: none !important;
}
option[selected] {
    display: inline !important;
}
"#,
    )
}

/// Open drop-list state for one surface.
#[derive(Clone, Debug)]
pub struct OpenSelect {
    pub select_nid: NodeId,
    pub select_id: Option<String>,
    /// Highlighted row index into the option list (capped to first 8).
    pub highlight: usize,
    /// Document-space rect of the closed control (for positioning the list).
    pub control_rect: ControlRect,
    /// Document-space rect of the open list.
    pub list_rect: ControlRect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ControlRect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn as_device(self, scale: f64) -> IntRect {
        let x0 = (self.x as f64 * scale).floor() as i32;
        let y0 = (self.y as f64 * scale).floor() as i32;
        let x1 = ((self.x + self.w) as f64 * scale).ceil() as i32;
        let y1 = ((self.y + self.h) as f64 * scale).ceil() as i32;
        IntRect {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0),
            h: (y1 - y0).max(0),
        }
    }
}

pub fn control_rect_of(doc: &HtmlDocument, nid: NodeId) -> Option<ControlRect> {
    // Transform-aware (Pause / centered panels use CSS `transform`).
    // `absolute_position` alone ignores transforms — see `element_border_box_css`.
    let (x, y, w, h) = crate::damage::element_border_box_css(doc, nid)?;
    Some(ControlRect {
        x,
        y,
        w,
        h: h.max(SELECT_ROW_H),
    })
}

pub fn list_rect_below(control: ControlRect, option_count: usize) -> ControlRect {
    let rows = option_count.min(SELECT_MAX_ROWS) as f32;
    ControlRect {
        x: control.x,
        y: control.y + control.h,
        w: control.w.max(64.0),
        h: rows * SELECT_ROW_H + 2.0, // +2 for border
    }
}

/// Row index under `(x,y)` in the open list, if any.
pub fn list_row_at(open: &OpenSelect, x: f32, y: f32, option_count: usize) -> Option<usize> {
    if !open.list_rect.contains(x, y) {
        return None;
    }
    let rows = option_count.min(SELECT_MAX_ROWS);
    let local_y = y - open.list_rect.y - 1.0;
    if local_y < 0.0 {
        return None;
    }
    let idx = (local_y / SELECT_ROW_H) as usize;
    if idx < rows {
        Some(idx)
    } else {
        None
    }
}

/// Damage rect covering the closed control and (if open) the list.
///
/// Tests assert this is non-empty while a list is open; returning nothing must
/// fail the select partial-vs-full parity suite.
pub fn select_damage_rects(open: Option<&OpenSelect>, scale: f64) -> Vec<IntRect> {
    let Some(open) = open else {
        return Vec::new();
    };
    let mut rects = vec![open.control_rect.as_device(scale)];
    rects.push(open.list_rect.as_device(scale));
    rects
}

/// Device-pixel damage for one list row (highlight change).
pub fn select_row_damage_rect(open: &OpenSelect, row: usize, scale: f64) -> IntRect {
    let y = open.list_rect.y + 1.0 + row as f32 * SELECT_ROW_H;
    ControlRect {
        x: open.list_rect.x,
        y,
        w: open.list_rect.w,
        h: SELECT_ROW_H,
    }
    .as_device(scale)
}

/// Collect every `<select>` node id (document order, depth-first).
pub fn collect_select_nids(doc: &HtmlDocument) -> Vec<NodeId> {
    let mut out = Vec::new();
    let root = doc.root_node().id;
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in &node.children {
            stack.push(child);
        }
        if node
            .element_data()
            .is_some_and(|el| &*el.name.local == "select")
        {
            out.push(nid);
        }
    }
    out
}

/// Paint the ▼ button strip on every `<select>`, and the open list overlay.
///
/// Called after Blitz raster so the list paints above other content.
/// `select_nids` is the index-time cache (falls back to a live walk when empty).
pub fn paint_select_overlays(
    doc: &HtmlDocument,
    frame: &mut RgbaImage,
    scale: f64,
    open: Option<&OpenSelect>,
    hover_row: Option<usize>,
    select_nids: &[NodeId],
) {
    let owned;
    let selects: &[NodeId] = if select_nids.is_empty() {
        owned = collect_select_nids(doc);
        &owned
    } else {
        select_nids
    };
    for &nid in selects {
        if let Some(rect) = control_rect_of(doc, nid) {
            paint_dropdown_button(frame, &rect, scale);
        }
    }

    let Some(open) = open else {
        return;
    };
    let opts = select_options(doc, open.select_nid);
    let rows = opts.len().min(SELECT_MAX_ROWS);
    paint_open_list(frame, open, &opts[..rows], scale, hover_row);
}

fn paint_dropdown_button(frame: &mut RgbaImage, control: &ControlRect, scale: f64) {
    let btn = ControlRect {
        x: control.x + control.w - SELECT_BUTTON_W,
        y: control.y,
        w: SELECT_BUTTON_W,
        h: control.h,
    };
    fill_rect(frame, &btn, scale, FACE);
    // Raised border
    stroke_raised(frame, &btn, scale);
    // ▼ glyph as a small filled triangle (no text shaping dependency).
    let cx = btn.x + btn.w * 0.5;
    let cy = btn.y + btn.h * 0.5;
    fill_triangle_down(frame, cx, cy, 4.0, scale, BLACK);
}

fn paint_open_list(
    frame: &mut RgbaImage,
    open: &OpenSelect,
    opts: &[crate::form_controls::SelectOption],
    scale: f64,
    hover_row: Option<usize>,
) {
    let list = open.list_rect;
    fill_rect(frame, &list, scale, WHITE_FACE);
    // Sunken outer border
    draw_hline(frame, list.x, list.y, list.w, scale, SHADOW);
    draw_vline(frame, list.x, list.y, list.h, scale, SHADOW);
    draw_hline(frame, list.x, list.y + list.h - 1.0, list.w, scale, WHITE);
    draw_vline(frame, list.x + list.w - 1.0, list.y, list.h, scale, WHITE);

    for (i, opt) in opts.iter().enumerate() {
        let row = ControlRect {
            x: list.x + 1.0,
            y: list.y + 1.0 + i as f32 * SELECT_ROW_H,
            w: list.w - 2.0,
            h: SELECT_ROW_H,
        };
        let highlighted = Some(i) == hover_row || i == open.highlight;
        if highlighted {
            fill_rect(frame, &row, scale, NAVY);
        }
        let fg = if highlighted { "#ffffff" } else { "#000000" };
        let bg = if highlighted { "#000080" } else { "#ffffff" };
        blit_label(frame, &row, scale, &opt.text, bg, fg);
    }
}

/// Rasterize a short label via a throwaway Blitz document and blit into `frame`.
/// Rows are cached by (text, bg, fg, size) so hover highlight swaps stay cheap.
fn blit_label(
    frame: &mut RgbaImage,
    row: &ControlRect,
    scale: f64,
    text: &str,
    bg: &str,
    fg: &str,
) {
    let dw = ((row.w as f64 * scale).ceil() as u32).max(1);
    let dh = ((row.h as f64 * scale).ceil() as u32).max(1);
    let key = (text.to_string(), bg.to_string(), fg.to_string(), dw, dh);
    {
        let cache = SELECT_ROW_CACHE.lock().expect("select row cache");
        if let Some(img) = cache.get(&key) {
            blit_image(frame, row, scale, img);
            return;
        }
    }
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = format!(
        r#"<!DOCTYPE html><html><body style="margin:0;padding:0 4px;background:{bg};color:{fg};font-family:sans-serif;font-size:12px;line-height:{h}px;white-space:nowrap;overflow:hidden">{escaped}</body></html>"#,
        bg = bg,
        fg = fg,
        h = row.h as i32,
        escaped = escaped,
    );
    let mut mini = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            viewport: Some(BlitzViewport::new(dw, dh, scale as f32, ColorScheme::Light)),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            // Rows always paint in bundled `sans-serif` (DejaVu); host fonts
            // (issue #28) are not used for the open list. The row cache is
            // process-wide, so it must not depend on one engine's fonts.
            font_ctx: Some(crate::fonts::bundled_font_context(
                &crate::fonts::HostFonts::default(),
            )),
            style_threading: StyleThreading::Sequential,
            incremental: Some(false),
            ..Default::default()
        },
    );
    mini.resolve(0.0);
    let Ok(img) = paint_rgba(&mut mini, dw, dh, scale) else {
        return;
    };
    blit_image(frame, row, scale, &img);
    SELECT_ROW_CACHE
        .lock()
        .expect("select row cache")
        .insert(key, img);
}

fn blit_image(frame: &mut RgbaImage, row: &ControlRect, scale: f64, img: &RgbaImage) {
    let x0 = (row.x as f64 * scale).round() as i32;
    let y0 = (row.y as f64 * scale).round() as i32;
    for dy in 0..img.height as i32 {
        for dx in 0..img.width as i32 {
            let x = x0 + dx;
            let y = y0 + dy;
            if x < 0 || y < 0 || x as u32 >= frame.width || y as u32 >= frame.height {
                continue;
            }
            let si = ((dy as u32 * img.width + dx as u32) * 4) as usize;
            let di = ((y as u32 * frame.width + x as u32) * 4) as usize;
            frame.pixels[di..di + 4].copy_from_slice(&img.pixels[si..si + 4]);
        }
    }
}

fn fill_rect(frame: &mut RgbaImage, rect: &ControlRect, scale: f64, color: [u8; 4]) {
    let r = rect.as_device(scale);
    for y in r.y.max(0)..(r.y + r.h).min(frame.height as i32) {
        for x in r.x.max(0)..(r.x + r.w).min(frame.width as i32) {
            let di = ((y as u32 * frame.width + x as u32) * 4) as usize;
            frame.pixels[di..di + 4].copy_from_slice(&color);
        }
    }
}

fn stroke_raised(frame: &mut RgbaImage, rect: &ControlRect, scale: f64) {
    draw_hline(frame, rect.x, rect.y, rect.w, scale, WHITE);
    draw_vline(frame, rect.x, rect.y, rect.h, scale, WHITE);
    draw_hline(frame, rect.x, rect.y + rect.h - 1.0, rect.w, scale, SHADOW);
    draw_vline(frame, rect.x + rect.w - 1.0, rect.y, rect.h, scale, SHADOW);
}

fn draw_hline(frame: &mut RgbaImage, x: f32, y: f32, w: f32, scale: f64, color: [u8; 4]) {
    fill_rect(frame, &ControlRect { x, y, w, h: 1.0 }, scale, color);
}

fn draw_vline(frame: &mut RgbaImage, x: f32, y: f32, h: f32, scale: f64, color: [u8; 4]) {
    fill_rect(frame, &ControlRect { x, y, w: 1.0, h }, scale, color);
}

fn fill_triangle_down(
    frame: &mut RgbaImage,
    cx: f32,
    cy: f32,
    half: f32,
    scale: f64,
    color: [u8; 4],
) {
    let top = ((cy - half * 0.4) as f64 * scale) as i32;
    let bot = ((cy + half * 0.6) as f64 * scale) as i32;
    let mid_x = (cx as f64 * scale) as i32;
    let half_px = (half as f64 * scale) as i32;
    for y in top..=bot {
        let t = if bot == top {
            0.0
        } else {
            (y - top) as f32 / (bot - top) as f32
        };
        let hw = (half_px as f32 * t) as i32;
        for x in (mid_x - hw)..=(mid_x + hw) {
            if x < 0 || y < 0 || x as u32 >= frame.width || y as u32 >= frame.height {
                continue;
            }
            let di = ((y as u32 * frame.width + x as u32) * 4) as usize;
            frame.pixels[di..di + 4].copy_from_slice(&color);
        }
    }
}

/// Build an [`OpenSelect`] for `ctrl` (must be a select).
pub fn open_select_state(doc: &HtmlDocument, ctrl: &FormControlRef) -> Option<OpenSelect> {
    let rect = control_rect_of(doc, ctrl.node_id)?;
    let opts = select_options(doc, ctrl.node_id);
    if opts.is_empty() {
        return None;
    }
    let highlight = select_selected_index(&opts).min(opts.len().min(SELECT_MAX_ROWS) - 1);
    let list_rect = list_rect_below(rect, opts.len());
    Some(OpenSelect {
        select_nid: ctrl.node_id,
        select_id: ctrl.element_id.clone(),
        highlight,
        control_rect: rect,
        list_rect,
    })
}

#[allow(dead_code)]
pub fn selected_label(doc: &HtmlDocument, select_nid: NodeId) -> String {
    doc.get_node(select_nid)
        .map(|n| select_selected_text(doc, n))
        .unwrap_or_default()
}
