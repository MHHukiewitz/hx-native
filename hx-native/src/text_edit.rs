//! Engine-owned single-line text / number editing for `<input type=text|number>`.
//!
//! Drives Blitz's public `PlainEditor` via [`BaseDocument::with_text_input`]
//! without Blitz's `EventDriver`. Policy (number filter, grouping, Esc revert,
//! host clipboard, caret blink damage) lives here.

use std::time::{Duration, Instant};

use blitz_dom::NodeId;
use blitz_html::HtmlDocument;

use crate::damage::{snap_outward, IntRect};
use crate::form_args::{
    clamp_snap_value, format_numeric, is_number_input, is_text_input, numeric_limits, parse_f64,
    write_value_attr, NumericLimits,
};

/// Default caret blink half-period (on or off).
pub const CARET_BLINK_HALF_PERIOD: Duration = Duration::from_millis(530);

/// True for characters the engine drops from `InputEvent::Text` / `Paste`.
///
/// Drops Unicode general category **Cc** (control, including `\r` `\n` `\t` DEL)
/// and bidi override / embedding controls `U+202A..=U+202E` and `U+2066..=U+2069`.
/// Hosts forward egui text raw; hx-native filters here.
pub fn is_dropped_insert_char(c: char) -> bool {
    if c.is_control() {
        return true;
    }
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Filter committed text for insertion: drop control and bidi override chars.
pub fn sanitize_text_insert(s: &str) -> String {
    s.chars().filter(|c| !is_dropped_insert_char(*c)).collect()
}

/// Prepare paste text for a single-line field: each run of `\r`/`\n` becomes one
/// space, then the same control/bidi filter as [`sanitize_text_insert`].
pub fn sanitize_paste(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' || c == '\n' {
            while matches!(chars.peek(), Some('\r' | '\n')) {
                chars.next();
            }
            out.push(' ');
            continue;
        }
        if !is_dropped_insert_char(c) {
            out.push(c);
        }
    }
    out
}

/// Active edit session for one focused text/number input.
#[derive(Clone, Debug)]
pub struct TextEditSession {
    pub id: String,
    pub node_id: NodeId,
    /// Canonical `value` at focus time (Esc restores this).
    pub value_at_focus: String,
    /// Caret drawn this frame (toggles on blink).
    pub caret_visible: bool,
    pub last_blink: Instant,
    /// Last known caret ink in device pixels (for blink damage).
    pub last_caret_rect: Option<IntRect>,
    /// After a same-id swap, PlainEditor exists only once style/layout has run.
    /// Set by reconcile; cleared after prepare_focus_display.
    pub needs_prepare: bool,
}

/// True when grouping separators should be shown for an unfocused number input.
///
/// Grouping is **opt-in** (issue #40): only `data-hx-group="on"` groups.
/// Without it (or with any other value) the box shows the raw number.
pub fn grouping_enabled(node: &blitz_dom::node::Node) -> bool {
    if !is_number_input(node) {
        return false;
    }
    attr(node, "data-hx-group").as_deref() == Some("on")
}

/// Show grouped text in every unfocused opt-in number input whose editor
/// does not already show it. Returns the inputs that changed.
pub fn apply_group_display_all(doc: &mut HtmlDocument, focused: Option<NodeId>) -> Vec<NodeId> {
    let root = doc.root_node().id;
    let mut changed = Vec::new();
    for (nid, _) in number_inputs_under(doc, root) {
        if Some(nid) == focused {
            continue;
        }
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if !grouping_enabled(node) {
            continue;
        }
        let want = format_grouped(&attr(node, "value").unwrap_or_default());
        match editor_text(doc, nid) {
            Some(shown) if shown != want => {
                set_editor_text(doc, nid, &want);
                changed.push(nid);
            }
            _ => {}
        }
    }
    changed
}

/// Format a canonical number string with `,` thousands separators in the integer part.
pub fn format_grouped(canonical: &str) -> String {
    let (sign, rest) = strip_sign(canonical.trim());
    let (int_part, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    // Only a plain decimal is grouped. Anything else (`1e5`, `abc`) is shown
    // as written rather than having its non-digits dropped (`1e5` -> `15`).
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || frac.is_some_and(|f| !f.chars().all(|c| c.is_ascii_digit()))
    {
        return canonical.to_string();
    }
    let digits: String = int_part.to_string();
    if digits.is_empty() {
        return canonical.to_string();
    }
    let grouped = group_int(&digits);
    let mut out = String::new();
    if sign {
        out.push('-');
    }
    out.push_str(&grouped);
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// Strip grouping commas (and accept them) then parse. Returns `None` if empty/invalid.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_number_display(text: &str) -> Option<f64> {
    let stripped: String = text.chars().filter(|c| *c != ',').collect();
    let t = stripped.trim();
    if t.is_empty() || t == "-" || t == "+" || t == "." || t == "-." || t == "+." {
        return None;
    }
    parse_f64(t)
}

/// Canonical form for the value attribute / action args (no separators).
pub fn canonical_from_display(text: &str, limits: &NumericLimits, clamp: bool) -> String {
    let stripped: String = text.chars().filter(|c| *c != ',').collect();
    let t = stripped.trim();
    if t.is_empty() || t == "-" || t == "+" {
        return String::new();
    }
    match parse_f64(t) {
        Some(v) => {
            let v = if clamp {
                clamp_snap_value(v, limits)
            } else {
                v
            };
            format_numeric(v)
        }
        None => t.to_string(),
    }
}

/// Whether `ch` may be inserted into a number field (before rounding/clamp).
/// Commas are accepted by the caller as a no-op insert (not stored while focused).
pub fn number_char_allowed(existing: &str, ch: char, limits: &NumericLimits) -> bool {
    if ch == ',' {
        return true;
    }
    if ch.is_ascii_digit() {
        return true;
    }
    if ch == '-' || ch == '+' {
        // Sign only at start, and only if min allows negative (or min unset).
        if !existing.is_empty() {
            return false;
        }
        if ch == '+' {
            return true;
        }
        return limits.min.is_none_or(|m| m < 0.0);
    }
    if ch == '.' {
        // One decimal point; allow when step is not an integer (or step has
        // fraction), or when step is "any" (issue #16: no grid constraint at
        // all, so decimals are always allowed regardless of `step`'s shape).
        if existing.contains('.') {
            return false;
        }
        if limits.unconstrained {
            return true;
        }
        let step = limits.step;
        let step_is_int = (step - step.round()).abs() < 1e-12;
        return !step_is_int;
    }
    false
}

/// Filter a multi-char paste for a number field.
pub fn filter_number_insert(existing: &str, insert: &str, limits: &NumericLimits) -> String {
    let mut out = String::new();
    let mut probe = existing.to_string();
    for ch in insert.chars() {
        if ch == ',' {
            continue; // accepted, ignored in stored text while focused
        }
        if number_char_allowed(&probe, ch, limits) {
            out.push(ch);
            probe.push(ch);
        }
    }
    out
}

fn strip_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    }
}

fn group_int(digits: &str) -> String {
    let b = digits.as_bytes();
    let mut out = String::with_capacity(b.len() + b.len() / 3);
    let rem = b.len() % 3;
    if rem > 0 {
        out.push_str(std::str::from_utf8(&b[..rem]).unwrap_or(""));
    }
    let mut i = rem;
    while i < b.len() {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(std::str::from_utf8(&b[i..i + 3]).unwrap_or(""));
        i += 3;
    }
    out
}

fn attr(node: &blitz_dom::node::Node, name: &str) -> Option<String> {
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| a.value.to_string())
    })
}

/// Read the PlainEditor buffer text.
pub fn editor_text(doc: &mut HtmlDocument, nid: NodeId) -> Option<String> {
    let mut out = None;
    doc.with_text_input(nid, |driver| {
        out = Some(driver.editor.text().to_string());
    });
    out
}

/// Replace the PlainEditor buffer (does not write the value attribute).
pub fn set_editor_text(doc: &mut HtmlDocument, nid: NodeId, text: &str) {
    doc.with_text_input(nid, |mut driver| {
        driver.editor.set_text(text);
        driver.refresh_layout();
    });
}

/// Insert text at the caret / replace selection.
pub fn insert_text(doc: &mut HtmlDocument, nid: NodeId, text: &str) {
    if text.is_empty() {
        return;
    }
    doc.with_text_input(nid, |mut driver| {
        driver.insert_or_replace_selection(text);
    });
}

pub fn backspace(doc: &mut HtmlDocument, nid: NodeId) {
    doc.with_text_input(nid, |mut driver| {
        driver.backdelete();
    });
}

pub fn delete_forward(doc: &mut HtmlDocument, nid: NodeId) {
    doc.with_text_input(nid, |mut driver| {
        driver.delete();
    });
}

pub fn move_left(doc: &mut HtmlDocument, nid: NodeId, select: bool) {
    doc.with_text_input(nid, |mut driver| {
        if select {
            driver.select_left();
        } else {
            driver.move_left();
        }
    });
}

pub fn move_right(doc: &mut HtmlDocument, nid: NodeId, select: bool) {
    doc.with_text_input(nid, |mut driver| {
        if select {
            driver.select_right();
        } else {
            driver.move_right();
        }
    });
}

pub fn move_home(doc: &mut HtmlDocument, nid: NodeId, select: bool) {
    doc.with_text_input(nid, |mut driver| {
        if select {
            driver.select_to_line_start();
        } else {
            driver.move_to_line_start();
        }
    });
}

pub fn move_end(doc: &mut HtmlDocument, nid: NodeId, select: bool) {
    doc.with_text_input(nid, |mut driver| {
        if select {
            driver.select_to_line_end();
        } else {
            driver.move_to_line_end();
        }
    });
}

pub fn select_all(doc: &mut HtmlDocument, nid: NodeId) {
    doc.with_text_input(nid, |mut driver| {
        driver.select_all();
    });
}

pub fn selected_text(doc: &mut HtmlDocument, nid: NodeId) -> Option<String> {
    let mut out = None;
    doc.with_text_input(nid, |driver| {
        out = driver.editor.selected_text().map(str::to_string);
    });
    out
}

/// True when the editor has a non-empty selection (caret blink stays on).
pub fn has_selection(doc: &mut HtmlDocument, nid: NodeId) -> bool {
    selected_text(doc, nid).is_some_and(|s| !s.is_empty())
}

pub fn delete_selection(doc: &mut HtmlDocument, nid: NodeId) {
    doc.with_text_input(nid, |mut driver| {
        driver.delete_selection();
    });
}

/// Place the caret from a document-space click (CSS px).
///
/// Uses [`element_border_box_css`] so clicks land correctly inside a
/// `transform: translate(-50%, -50%)` panel (absolute_position alone ignores
/// transforms).
pub fn place_caret_at(doc: &mut HtmlDocument, nid: NodeId, x: f32, y: f32) {
    let Some(node) = doc.get_node(nid) else {
        return;
    };
    let Some((bx, by, _, _)) = crate::damage::element_border_box_css(doc, nid) else {
        return;
    };
    let layout = node.final_layout();
    let pad_l = layout.padding.left + layout.border.left;
    let pad_t = layout.padding.top + layout.border.top;
    let mut y_off = 0.0f32;
    let mut scroll_x = 0.0f64;
    if let Some(input) = node.element_data().and_then(|e| e.text_input_data()) {
        if !input.is_multiline {
            if let Some(ed_layout) = input.editor.try_layout() {
                let content_h = layout.content_box_height();
                let input_h = ed_layout.height() / ed_layout.scale();
                y_off = ((content_h - input_h) / 2.0).max(0.0);
            }
            scroll_x = input.scroll_offset as f64 * doc.viewport().scale_f64();
        }
    }
    let scale = doc.viewport().scale_f64();
    let local_x = (x - bx - pad_l) as f64 * scale + scroll_x;
    let local_y = (y - by - pad_t - y_off) as f64 * scale;
    doc.with_text_input(nid, |mut driver| {
        driver.move_to_point(local_x as f32, local_y as f32);
    });
}

/// Caret ink rect in device pixels, or `None` when there is no caret geometry.
///
/// Origin comes from [`element_border_box_device`] (transform-aware), not
/// `absolute_position`.
pub fn caret_device_rect(
    doc: &mut HtmlDocument,
    nid: NodeId,
    scale: f64,
    surface_w: u32,
    surface_h: u32,
) -> Option<IntRect> {
    // Refresh layout so cursor_geometry is available after edits.
    doc.with_text_input(nid, |mut driver| {
        driver.refresh_layout();
    });
    let (bx, by, _, _) = crate::damage::element_border_box_device(doc, nid, scale)?;
    let node = doc.get_node(nid)?;
    let layout = node.final_layout();
    let pad_l = layout.padding.left + layout.border.left;
    let pad_t = layout.padding.top + layout.border.top;
    let input = node.element_data()?.text_input_data()?;
    let ed_layout = input.editor.try_layout()?;
    let caret = input.editor.cursor_geometry(1.5)?;
    let y_off = if input.is_multiline {
        0.0
    } else {
        let content_h = layout.content_box_height();
        let input_h = ed_layout.height() / ed_layout.scale();
        ((content_h - input_h) / 2.0).max(0.0)
    };
    let scroll = input.scroll_offset as f64 * scale;
    let (sx, sy) = if input.is_multiline {
        (0.0, scroll)
    } else {
        (scroll, 0.0)
    };
    // caret is in scaled (device) space relative to the editor origin.
    let x0 = bx + pad_l as f64 * scale - sx + caret.x0;
    let y0 = by + (pad_t + y_off) as f64 * scale - sy + caret.y0;
    let w = (caret.x1 - caret.x0).max(1.0);
    let h = (caret.y1 - caret.y0).max(1.0);
    Some(snap_outward(x0, y0, w, h, surface_w, surface_h))
}

/// Sync the value attribute from the editor buffer (canonical, no grouping).
/// Restores the focused display text in the editor afterwards.
pub fn sync_value_from_editor(
    doc: &mut HtmlDocument,
    nid: NodeId,
    is_number: bool,
    clamp: bool,
) -> bool {
    let display = editor_text(doc, nid).unwrap_or_default();
    let canonical = if is_number {
        let limits = doc
            .get_node(nid)
            .map(numeric_limits)
            .unwrap_or(NumericLimits {
                min: None,
                max: None,
                step: 1.0,
                unconstrained: false,
            });
        canonical_from_display(&display, &limits, clamp)
    } else {
        display.clone()
    };
    let changed = write_value_attr(doc, nid, &canonical);
    // set_attribute also writes the editor to canonical — restore display.
    set_editor_text(doc, nid, &display);
    changed
}

/// Commit a number/text field: clamp number, write canonical value, set unfocused display.
pub fn commit_field(doc: &mut HtmlDocument, nid: NodeId) -> bool {
    let (is_number, group, prev_value, limits) = {
        let Some(node) = doc.get_node(nid) else {
            return false;
        };
        let is_number = is_number_input(node);
        (
            is_number,
            grouping_enabled(node),
            attr(node, "value").unwrap_or_else(|| "0".into()),
            if is_number {
                Some(numeric_limits(node))
            } else {
                None
            },
        )
    };
    let display = editor_text(doc, nid).unwrap_or_default();
    let canonical = if let Some(ref limits) = limits {
        let parsed = canonical_from_display(&display, limits, true);
        if parsed.is_empty() {
            prev_value
        } else {
            parsed
        }
    } else {
        display
    };
    let changed = write_value_attr(doc, nid, &canonical);
    let shown = if is_number && group {
        format_grouped(&canonical)
    } else {
        canonical.clone()
    };
    set_editor_text(doc, nid, &shown);
    changed
}

/// Prepare display text when focusing a field (raw / ungrouped for editing).
pub fn prepare_focus_display(doc: &mut HtmlDocument, nid: NodeId) {
    let Some(node) = doc.get_node(nid) else {
        return;
    };
    let value = attr(node, "value").unwrap_or_default();
    set_editor_text(doc, nid, &value);
    // Place caret at end so typed text appends.
    doc.with_text_input(nid, |mut driver| {
        driver.move_to_text_end();
    });
}

/// After load / blur styling: show grouped text for unfocused number inputs.
pub fn apply_unfocused_display(doc: &mut HtmlDocument, nid: NodeId) {
    let Some(node) = doc.get_node(nid) else {
        return;
    };
    if !grouping_enabled(node) {
        return;
    }
    let value = attr(node, "value").unwrap_or_default();
    set_editor_text(doc, nid, &format_grouped(&value));
}

pub fn is_editable_input(node: &blitz_dom::node::Node) -> bool {
    is_text_input(node) || is_number_input(node)
}

pub fn is_disabled(node: &blitz_dom::node::Node) -> bool {
    attr(node, "disabled").is_some() || attr(node, "aria-disabled").as_deref() == Some("true")
}

/// Resolve the number input targeted by a spin control (`data-hx-step`).
///
/// Prefer `data-hx-step-for="id"` when present. Otherwise walk ancestors until
/// one contains exactly one `<input type=number>`, and use that.
/// Returns `(node_id, element id if any)`.
pub fn resolve_step_target(
    doc: &HtmlDocument,
    hit_element_id: &str,
) -> Option<(NodeId, Option<String>)> {
    use crate::dialect::ATTR_HX_STEP_FOR;

    let nid = doc.get_element_by_id(hit_element_id)?;
    let (owner_nid, owner) = find_step_owner(doc, nid)?;

    if let Some(for_id) = attr(owner, ATTR_HX_STEP_FOR) {
        if for_id.is_empty() {
            return None;
        }
        let target_nid = doc.get_element_by_id(&for_id)?;
        let target = doc.get_node(target_nid)?;
        if is_number_input(target) {
            return Some((target_nid, Some(for_id)));
        }
        return None;
    }

    // Walk from the step owner upward; first ancestor with exactly one number input.
    let mut cur = Some(owner_nid);
    while let Some(cid) = cur {
        let numbers = number_inputs_under(doc, cid);
        if numbers.len() == 1 {
            let (tnid, tid) = numbers[0].clone();
            return Some((tnid, tid));
        }
        cur = doc.get_node(cid).and_then(|n| n.parent);
    }
    None
}

fn find_step_owner(doc: &HtmlDocument, start: NodeId) -> Option<(NodeId, &blitz_dom::node::Node)> {
    use crate::dialect::ATTR_HX_STEP;
    let mut cur = Some(start);
    while let Some(cid) = cur {
        let node = doc.get_node(cid)?;
        if let Some(v) = attr(node, ATTR_HX_STEP) {
            if v == "up" || v == "down" {
                return Some((cid, node));
            }
        }
        cur = node.parent;
    }
    None
}

/// Direction for a step control click: +1 up, -1 down.
pub fn step_direction(doc: &HtmlDocument, hit_id: &str) -> Option<i32> {
    use crate::dialect::ATTR_HX_STEP;
    let nid = doc.get_element_by_id(hit_id)?;
    let (_, owner) = find_step_owner(doc, nid)?;
    match attr(owner, ATTR_HX_STEP).as_deref() {
        Some("up") => Some(1),
        Some("down") => Some(-1),
        _ => None,
    }
}

fn number_inputs_under(doc: &HtmlDocument, root: NodeId) -> Vec<(NodeId, Option<String>)> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in node.children.iter().rev() {
            stack.push(child);
        }
        if is_number_input(node) {
            let id = attr(node, "id").filter(|s| !s.is_empty());
            out.push((nid, id));
        }
    }
    out
}

/// Step a number input by `dir`, write canonical value, regroup display if enabled.
pub fn step_and_regroup(doc: &mut HtmlDocument, nid: NodeId, dir: i32) -> bool {
    use crate::form_args::{format_numeric_for_step, step_precise};

    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    if !is_number_input(node) || is_disabled(node) {
        return false;
    }
    let limits = numeric_limits(node);
    let group = grouping_enabled(node);
    let current = attr(node, "value")
        .and_then(|s| parse_f64(&s))
        .unwrap_or(limits.min.unwrap_or(0.0));
    let next = step_precise(current, dir, &limits);
    let canonical = format_numeric_for_step(next, &limits);
    let changed = write_value_attr(doc, nid, &canonical);
    let shown = if group {
        format_grouped(&canonical)
    } else {
        canonical.clone()
    };
    set_editor_text(doc, nid, &shown);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form_args::NumericLimits;

    #[test]
    fn groups_integer_and_keeps_fraction() {
        assert_eq!(format_grouped("180000"), "180,000");
        assert_eq!(format_grouped("2.20"), "2.20");
        assert_eq!(format_grouped("-1234567.89"), "-1,234,567.89");
        assert_eq!(format_grouped("12"), "12");
    }

    #[test]
    fn parse_ignores_commas() {
        assert_eq!(parse_number_display("180,000").unwrap(), 180_000.0);
        assert_eq!(parse_number_display("1,234.5").unwrap(), 1234.5);
    }

    #[test]
    fn number_rejects_letters() {
        let lim = NumericLimits {
            min: Some(0.0),
            max: Some(9.0),
            step: 1.0,
            unconstrained: false,
        };
        assert!(!number_char_allowed("", 'a', &lim));
        assert!(number_char_allowed("", '5', &lim));
        assert!(number_char_allowed("1", ',', &lim));
        assert!(!number_char_allowed("1", '.', &lim)); // integer step
        let lim2 = NumericLimits {
            min: Some(0.0),
            max: Some(9.0),
            step: 0.01,
            unconstrained: false,
        };
        assert!(number_char_allowed("1", '.', &lim2));
    }

    /// Blocker 1 (PR27 round 2): `step="any"` (`unconstrained`) allows a
    /// decimal point even though the fallback `step` (1.0) looks integer.
    #[test]
    fn number_allows_decimal_when_unconstrained() {
        let lim = NumericLimits {
            min: Some(0.0),
            max: Some(10.0),
            step: 1.0,
            unconstrained: true,
        };
        assert!(number_char_allowed("4", '.', &lim));
        assert!(number_char_allowed("4.", '3', &lim));
    }
}

#[cfg(test)]
mod sanitize_tests {
    use super::{sanitize_paste, sanitize_text_insert};

    #[test]
    fn sanitize_text_drops_controls_and_keeps_printable() {
        assert_eq!(sanitize_text_insert("a\u{7}b"), "ab");
        assert_eq!(sanitize_text_insert("x\ty"), "xy");
    }

    #[test]
    fn sanitize_paste_collapses_newlines_then_filters() {
        assert_eq!(sanitize_paste("12\r\n34"), "12 34");
        assert_eq!(sanitize_paste("a\r\r\nb"), "a b");
    }

    #[test]
    fn sanitize_drops_bidi_overrides() {
        assert_eq!(sanitize_text_insert("ab\u{202E}cd"), "abcd");
        assert_eq!(sanitize_paste("a\u{2066}b"), "ab");
    }
}
