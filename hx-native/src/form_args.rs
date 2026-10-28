//! Collect htmx-style form `(name, value)` args for an action trigger.
//!
//! Order: triggering element's own pair first (if named), then named controls
//! in the closest ancestor `<form>` in document order (trigger not repeated).
//!
//! Repeated names (e.g. several checked checkboxes with `name="state"`) each
//! contribute a `(name, value)` pair in document order — the same shape as
//! HTML form serialization and htmx. Use [`args_get`] for the first value and
//! [`args_get_all`] for every value. The args type stays `Vec<(String, String)>`
//! so Mainnet's existing consumers keep working. An unchecked checkbox or
//! radio contributes nothing, even when it is the element that fired (issue
//! #82: an untick posts the form without the box, as htmx does).
//!
//! When the triggering element has `name=` / `value=` and the form also has a
//! field with that name, both appear: the trigger's value first, then the
//! form's (htmx parity).

use std::collections::HashSet;

use blitz_dom::node::Node;
use blitz_dom::{qual_name, NodeId};
use blitz_html::HtmlDocument;

use crate::dialect::ATTR_HX_APPLIED;

/// IEEE-754 safe integer limit (±2^53). Beyond this, number fields clamp at commit
/// and step math refuses to proceed with wrapping/`as i64` casts.
pub(crate) const MAX_SAFE_NUMBER: f64 = 9_007_199_254_740_992.0; // 2^53

/// First value for `name`, if any (htmx / HTML single-value accessor).
pub fn args_get<'a>(args: &'a [(String, String)], name: &str) -> Option<&'a str> {
    args.iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Every value for `name`, in args order (document order for form fields).
pub fn args_get_all<'a>(args: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    args.iter()
        .filter(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .collect()
}

/// Collect args for an action fired from `source_id`.
///
/// `rule_greyed` (issue #15) is the set of input nodes currently disabled by
/// an `InputRule::EnabledWhen` (engine-applied, not host-authored): those
/// still contribute their value (Game ignores/validates it, K1), unlike a
/// plain host-authored `disabled` control, which is skipped as before.
pub fn collect_action_args(
    doc: &HtmlDocument,
    source_id: &str,
    rule_greyed: &HashSet<NodeId>,
) -> Vec<(String, String)> {
    let mut args = Vec::new();
    let Some(source_nid) = doc.get_element_by_id(source_id) else {
        return args;
    };
    let Some(source) = doc.get_node(source_nid) else {
        return args;
    };

    // (1) Triggering element first, if it has a non-empty name.
    // Any element (button, tr, input, select, …) with name= contributes a
    // pair, matching htmx. Form controls use their control value (selected
    // option, checkbox value, …); other elements use the `value` attribute.
    // Issue #82 (Mainnet #791): a checkbox or radio is posted only while it is
    // checked, the trigger included (htmx `shouldInclude`), so an untick sends
    // the form without the box.
    if let Some(name) = named_attr(source, "name") {
        if !is_unchecked_checkable(source) {
            args.push((name, trigger_element_value(doc, source)));
        }
    }

    // Then named controls in the closest ancestor form (document order).
    let Some(form_nid) = closest_form(doc, source_nid) else {
        return args;
    };
    for nid in descendants_document_order(doc, form_nid) {
        if nid == source_nid {
            continue; // do not repeat the trigger
        }
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if let Some(pair) = form_control_pair(doc, node, rule_greyed.contains(&nid)) {
            args.push(pair);
        }
    }
    args
}

/// Issue #15: one entry per named form control (document order, under
/// `form_id`): its node id, name, current value, and — when present —
/// its `data-hx-applied` value (Designer lock 2026-10-20), decoded into the
/// same encoding as the current value: checkbox/radio as `"true"`/`"false"`;
/// everything else the literal control value (canonical for numbers). Always
/// one entry per control regardless of `disabled` (an `EnabledWhen` toggle
/// must not by itself change `RequiresChange` dirtiness). Empty when
/// `form_id` is not in the document.
pub(crate) fn capture_form_controls(
    doc: &HtmlDocument,
    form_id: &str,
) -> Vec<(NodeId, String, String, Option<String>)> {
    let Some(form_nid) = doc.get_element_by_id(form_id) else {
        return Vec::new();
    };
    descendants_document_order(doc, form_nid)
        .into_iter()
        .filter_map(|nid| {
            let node = doc.get_node(nid)?;
            let (name, current) = control_baseline_pair(doc, node)?;
            Some((nid, name, current, applied_value(node)))
        })
        .collect()
}

/// This control's `data-hx-applied` value decoded into the same encoding as
/// [`capture_form_controls`]'s current value (`"true"`/`"false"` for
/// checkbox/radio, the literal value otherwise). `None` when absent.
fn applied_value(node: &Node) -> Option<String> {
    let raw = attr(node, ATTR_HX_APPLIED)?;
    let tag = tag_name(node);
    if tag == "input" && matches!(input_type(node).as_str(), "checkbox" | "radio") {
        return Some(if raw.trim().eq_ignore_ascii_case("true") {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }
    Some(raw)
}

fn control_baseline_pair(doc: &HtmlDocument, node: &Node) -> Option<(String, String)> {
    let name = named_attr(node, "name")?;
    let current = control_current_value(doc, node)?;
    Some((name, current))
}

/// Current value for a supported control, same encoding used throughout this
/// module (checkbox/radio as `"true"`/`"false"`, everything else the literal
/// value). Unlike [`control_baseline_pair`], does not require a `name` —
/// standalone `data-hx-applied` controls (Designer lock 2026-10-20) opt into
/// `hx-changed` by the attribute alone, with or without a form or a name.
pub(crate) fn control_current_value(doc: &HtmlDocument, node: &Node) -> Option<String> {
    match tag_name(node).as_str() {
        "input" => match input_type(node).as_str() {
            "checkbox" | "radio" => Some(is_checked(node).to_string()),
            "number" | "range" | "hidden" | "text" => Some(control_value(node)),
            _ => None,
        },
        "select" => Some(select_value(doc, node)),
        _ => None,
    }
}

/// Issue #24 (Designer lock 2026-10-20): node ids of every element anywhere
/// in the document carrying `data-hx-applied` — the attribute alone opts a
/// control into `hx-changed` tracking, independent of any enclosing form or
/// `RequiresChange` action. Walked once per structural DOM change (reindex),
/// not per sync/edit, so the per-edit recompute stays cheap (issue #15 cost
/// rule: only opted-in controls are ever compared).
pub(crate) fn collect_applied_control_nids(doc: &HtmlDocument) -> Vec<NodeId> {
    let root = doc.root_node().id;
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        if node.is_element() && attr(node, ATTR_HX_APPLIED).is_some() {
            out.push(nid);
        }
    }
    out
}

/// Issue #24: `(node id, current value, applied value)` for each id in
/// `nids` (from [`collect_applied_control_nids`]) still present and still a
/// supported control. The applied value is always present by construction —
/// that is what being in `nids` means — so there is no fallback baseline to
/// track or invalidate on re-render (data-hx-applied always wins).
pub(crate) fn capture_applied_controls(
    doc: &HtmlDocument,
    nids: &[NodeId],
) -> Vec<(NodeId, String, String)> {
    nids.iter()
        .filter_map(|&nid| {
            let node = doc.get_node(nid)?;
            let current = control_current_value(doc, node)?;
            let applied = applied_value(node)?;
            Some((nid, current, applied))
        })
        .collect()
}

/// Issue #89 (Designer, HTML_CONTRACT `hx-sync`, Mainnet master 188feffd):
/// what value control `node` contributed to a request that carried `args`,
/// in [`control_current_value`]'s encoding (checkbox/radio `"true"` when its
/// pair was posted, `"false"` otherwise). `None` for a control the request
/// could not carry: no name, a hidden input, or a named value control with
/// no pair in `args`.
pub(crate) fn control_value_in_args(node: &Node, args: &[(String, String)]) -> Option<String> {
    let name = named_attr(node, "name")?;
    match tag_name(node).as_str() {
        "input" => match input_type(node).as_str() {
            "checkbox" | "radio" => {
                let v = attr(node, "value").unwrap_or_else(|| "on".to_string());
                Some(args.iter().any(|(k, a)| *k == name && *a == v).to_string())
            }
            "number" | "range" | "text" | "" => args
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone()),
            _ => None,
        },
        "select" => args
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone()),
        _ => None,
    }
}

/// Issue #89: how a redraw puts a committed value back into `node`:
/// `"select"`, `"checkbox"`, `"radio"`, or `"value"` (the `value` attribute).
pub(crate) fn value_control_kind(node: &Node) -> &'static str {
    match tag_name(node).as_str() {
        "select" => "select",
        "input" => match input_type(node).as_str() {
            "checkbox" => "checkbox",
            "radio" => "radio",
            _ => "value",
        },
        _ => "value",
    }
}

/// Issue #89: replace the value `node` posts in `args` (the first pair with
/// its name) with `value`, for a text, number or range box whose live
/// `value` holds text the player has not committed yet.
pub(crate) fn patch_control_arg(node: &Node, args: &mut [(String, String)], value: &str) {
    let Some(name) = named_attr(node, "name") else {
        return;
    };
    if let Some(pair) = args.iter_mut().find(|(k, _)| *k == name) {
        pair.1 = value.to_string();
    }
}

/// Value for a triggering element: real control value for form controls,
/// otherwise the `value` attribute (empty when missing).
fn trigger_element_value(doc: &HtmlDocument, node: &Node) -> String {
    let tag = tag_name(node);
    match tag.as_str() {
        "select" => select_value(doc, node),
        "input" => {
            let ty = input_type(node);
            match ty.as_str() {
                "checkbox" | "radio" => {
                    // A checked trigger contributes its value attr (htmx);
                    // default "on". Unchecked ones never get here (#82).
                    match attr(node, "value") {
                        None => "on".to_string(),
                        Some(v) => v,
                    }
                }
                _ => attr(node, "value").unwrap_or_default(),
            }
        }
        _ => attr(node, "value").unwrap_or_default(),
    }
}

/// True when `node` is an `<input type="number|range">`.
pub fn is_numeric_input(node: &Node) -> bool {
    tag_name(node) == "input" && matches!(input_type(node).as_str(), "number" | "range")
}

/// True when `node` is an `<input type="number">` (not range).
pub fn is_number_input(node: &Node) -> bool {
    tag_name(node) == "input" && input_type(node).as_str() == "number"
}

/// True when `node` is an `<input type="text">` (or missing type → text).
pub fn is_text_input(node: &Node) -> bool {
    tag_name(node) == "input" && matches!(input_type(node).as_str(), "text" | "")
}

/// True when `node` is an `<input type="range">`.
pub fn is_range_input(node: &Node) -> bool {
    tag_name(node) == "input" && input_type(node).as_str() == "range"
}

/// Step a number/range control by `dir` (±1) × `step`, clamp, snap, write `value`.
/// Returns `true` when the attribute changed.
///
/// Uses decimal-rounded math so `2.20 + 0.01` stays `2.21` (no float drift),
/// without ever going through an integer-scaled (`i64`) intermediate.
pub fn step_numeric_value(doc: &mut HtmlDocument, id: &str, dir: i32) -> bool {
    let Some(nid) = doc.get_element_by_id(id) else {
        return false;
    };
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    if !is_numeric_input(node) {
        return false;
    }
    let limits = numeric_limits(node);
    let current = attr(node, "value")
        .and_then(|s| parse_f64(&s))
        .unwrap_or(limits.min.unwrap_or(0.0));
    let next = step_precise(current, dir, &limits);
    write_value_attr(doc, nid, &format_numeric(next))
}

fn clamp_safe_number(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(-MAX_SAFE_NUMBER, MAX_SAFE_NUMBER)
}

/// Most decimal places any engine-computed number carries.
const MAX_DECIMALS: usize = 10;

/// Round `v` to `decimals` places via a decimal-string round-trip: the
/// result is the `f64` nearest that decimal, so its shortest form (what
/// [`format_numeric`] prints) has no binary noise. No `i64` intermediate
/// anywhere, so there is no overflow/wraparound boundary to hit for any
/// finite `v` — `format!` on `f64` never panics or saturates.
fn round_decimal(v: f64, decimals: usize) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let d = decimals.min(MAX_DECIMALS);
    format!("{v:.d$}").parse::<f64>().unwrap_or(v)
}

/// Decimal places in `v`'s shortest round-trip form (at most
/// [`MAX_DECIMALS`]): `0.01` → 2, `1234567.89` → 2, `5` → 0.
pub(crate) fn decimals_of(v: f64) -> usize {
    if !v.is_finite() {
        return 0;
    }
    let s = format!("{}", v.abs());
    s.split_once('.')
        .map_or(0, |(_, frac)| frac.len().min(MAX_DECIMALS))
}

/// Issue #48: the decimals a computed value may carry: those of `step`
/// (unless `step="any"`), of `min` (the grid's origin) and of `operand`
/// (the typed or current value), whichever is greatest. Every step, snap
/// and clamp result is rounded to this before it is stored, so
/// `410 × 0.01` is stored as `4.1`, never `4.1000000000000005`.
pub(crate) fn result_decimals(limits: &NumericLimits, operand: f64) -> usize {
    let step = if limits.unconstrained {
        0
    } else {
        step_decimal_places(limits.step)
    };
    step.max(limits.min.map_or(0, decimals_of))
        .max(decimals_of(operand))
}

/// Step by `dir` × `step`, clamp to `[min, max]`, and (unless `unconstrained`)
/// snap to the step grid from `min` (or 0) — all in decimal-rounded `f64`
/// math (round-trip through a formatted string at the step's own decimal
/// precision), never through an integer-scaled (`i64`) intermediate. Values
/// stay within ±2^53 throughout (`clamp_safe_number`).
///
/// `unconstrained` (`step="any"`, i.e. `InputLimits.step: None`): no grid to
/// snap to — just add `step` (a sane default of `1` even though it is
/// otherwise unused) and clamp.
pub(crate) fn step_precise(current: f64, dir: i32, limits: &NumericLimits) -> f64 {
    let current = clamp_safe_number(current);
    let min = limits.min.unwrap_or(-MAX_SAFE_NUMBER);
    let max = limits.max.unwrap_or(MAX_SAFE_NUMBER);
    let max = max.max(min);

    if limits.unconstrained {
        let next = round_decimal(
            current + dir as f64 * limits.step,
            result_decimals(limits, current).max(decimals_of(limits.step)),
        );
        return clamp_safe_number(next.clamp(min, max));
    }

    // On the grid the result has the grid's decimals (step, min); the
    // current value's own decimals never matter there.
    let decimals = result_decimals(limits, 0.0);
    let step = limits.step;
    if step <= 0.0 {
        return clamp_safe_number(current.clamp(min, max));
    }
    // HTML stepUp/stepDown: from a value on the grid, move one step; from an
    // off-grid value (now common, since typed values are not snapped, #39),
    // land on the nearest grid value in the arrow's direction, so 151234
    // with step 1000 goes up to 152000 and down to 151000 (Graph review of #41).
    let base = limits.min.unwrap_or(0.0);
    let k = (current - base) / step;
    let on_grid = (k - k.round()).abs() < 1e-7;
    let n = if on_grid {
        k.round() + dir as f64
    } else if dir > 0 {
        k.ceil()
    } else {
        k.floor()
    };
    // An arrow never leaves `[min, max]` and stays on the grid, like HTML
    // stepUp/stepDown with an off-grid max: it lands on the highest grid
    // value at or below `max` (max 99.996, step 0.01: 99.99). If that would
    // move the value against the arrow (already past it), nothing changes.
    let mut grid_max = round_decimal(base + ((max - base) / step + 1e-7).floor() * step, decimals);
    // The `1e-7` slack absorbs float error in the division, but a max within
    // 1e-7 steps below a grid value (max 4.99999999, step 1) would round up
    // past it: step back once (Graph review of #49 r2).
    if grid_max > max {
        grid_max = round_decimal(grid_max - step, decimals);
    }
    let grid_max = if grid_max < min { min } else { grid_max };
    let next = round_decimal(base + n * step, decimals).clamp(min, grid_max);
    if (dir > 0 && next < current) || (dir < 0 && next > current) {
        return no_negative_zero(current);
    }
    no_negative_zero(clamp_safe_number(next))
}

/// `-0.0` (a tiny negative rounded to the grid, e.g. `-0.9 + 3 × 0.3`) is
/// stored and sent as `0`, never `-0.0` (Graph review of #49).
fn no_negative_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

pub(crate) fn step_decimal_places(step: f64) -> usize {
    if step <= 0.0 || !step.is_finite() {
        return 0;
    }
    decimals_of(step)
}

/// Set a range control's value from pointer `x` (document coordinates) with
/// the thumb held `grab` px right of its centre (issue #35: the engine-drawn
/// thumb's centre follows the pointer; see `slider::ratio_for_x`). The value
/// snaps to the `step` grid, rounded at the step's decimals (no float
/// noise), and is written in the same shortest form as before. Returns `true` when the attribute changed.
pub fn set_range_value_from_x(doc: &mut HtmlDocument, id: &str, x: f32, grab: f32) -> bool {
    let Some(nid) = doc.get_element_by_id(id) else {
        return false;
    };
    let limits = {
        let Some(node) = doc.get_node(nid) else {
            return false;
        };
        if !is_range_input(node) {
            return false;
        }
        numeric_limits(node)
    };
    let Some(geom) = crate::slider::slider_geometry(doc, nid) else {
        return false;
    };
    let min = limits.min.unwrap_or(0.0);
    let max = limits.max.unwrap_or(100.0);
    let ratio = crate::slider::ratio_for_x(&geom, x, grab);
    let raw = min + ratio * (max - min);
    let snapped = range_snap(raw, &limits);
    write_value_attr(doc, nid, &format_numeric(snapped))
}

/// Issue #35: the page block of a range: `max(step, (max - min) / 10)`,
/// the tenth snapped to a whole number of steps (Win95 trackbar paging).
pub(crate) fn range_page_block(limits: &NumericLimits) -> f64 {
    let min = limits.min.unwrap_or(0.0);
    let max = limits.max.unwrap_or(100.0).max(min);
    let tenth = (max - min) / 10.0;
    if limits.unconstrained || limits.step.is_nan() || limits.step <= 0.0 {
        return tenth;
    }
    let step = limits.step;
    ((tenth / step).round() * step).max(step)
}

/// Issue #35: a press on the groove left / right of the thumb moves the
/// value one page block toward `x` (document CSS px), clamped and snapped.
/// A press on the thumb's own column does nothing. Returns `true` when the
/// attribute changed.
pub fn range_page_toward_x(doc: &mut HtmlDocument, id: &str, x: f32) -> bool {
    let Some(nid) = doc.get_element_by_id(id) else {
        return false;
    };
    let (limits, current) = {
        let Some(node) = doc.get_node(nid) else {
            return false;
        };
        if !is_range_input(node) {
            return false;
        }
        let limits = numeric_limits(node);
        let current = crate::slider::range_value(node, &limits);
        (limits, current)
    };
    let Some(geom) = crate::slider::slider_geometry(doc, nid) else {
        return false;
    };
    let dir = if x < geom.thumb.x {
        -1.0
    } else if x >= geom.thumb.x + geom.thumb.w {
        1.0
    } else {
        return false;
    };
    let block = range_page_block(&limits);
    let next = range_snap(current + dir * block, &limits);
    write_value_attr(doc, nid, &format_numeric(next))
}

/// Issue #35 round 3: PageUp / PageDown on a focused range move one page
/// block (`dir` = +1 / -1), on the grid and within `[min, max]`, like a
/// groove press. Returns `true` when the attribute changed.
pub(crate) fn range_page(doc: &mut HtmlDocument, nid: NodeId, dir: f64) -> bool {
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    if !is_range_input(node) {
        return false;
    }
    let limits = numeric_limits(node);
    let current = crate::slider::range_value(node, &limits);
    let next = range_snap(current + dir * range_page_block(&limits), &limits);
    write_value_attr(doc, nid, &format_numeric(next))
}

/// Issue #35: Home / End on a range — write `min` (or `max` snapped onto
/// the step grid). Returns `true` when the attribute changed.
pub(crate) fn range_to_end(doc: &mut HtmlDocument, nid: NodeId, to_max: bool) -> bool {
    let Some(node) = doc.get_node(nid) else {
        return false;
    };
    if !is_range_input(node) {
        return false;
    }
    let limits = numeric_limits(node);
    let target = if to_max {
        limits.max.unwrap_or(100.0)
    } else {
        limits.min.unwrap_or(0.0)
    };
    write_value_attr(doc, nid, &format_numeric(range_snap(target, &limits)))
}

/// Clamp + snap a range value to its grid, rounded at the step's decimals so
/// `0.05 × 23` lands on `1.15`, not `1.1500000000000001`.
pub(crate) fn range_snap(raw: f64, limits: &NumericLimits) -> f64 {
    let snapped = clamp_snap(raw, limits);
    if limits.unconstrained {
        return snapped;
    }
    let min = limits.min.unwrap_or(-MAX_SAFE_NUMBER);
    let max = limits.max.unwrap_or(MAX_SAFE_NUMBER).max(min);
    // The grid's decimals are those of `step` and of its origin `min`
    // (`min=0.5 step=1` has values 0.5, 1.5, …), as in `step_precise`.
    let places = result_decimals(limits, 0.0);
    let v = round_decimal(snapped, places).clamp(min, max);
    // A clamp to a max that is off the step grid (0..10 step 3) leaves the
    // grid; like HTML, take the last on-grid value at or below max (9).
    if v >= max && limits.step > 0.0 {
        let base = limits.min.unwrap_or(0.0);
        // Same slack as `step_precise`: `(1234567.89 - 1000000) / 0.01` comes
        // out just under 23456789, and a 1e-9 slack floored it one step short
        // of an on-grid max. A max just below a grid value can round up past
        // it, so step back once.
        let k = ((max - base) / limits.step + 1e-7).floor();
        let mut top = round_decimal(base + k * limits.step, places);
        if top > max {
            top = round_decimal(top - limits.step, places);
        }
        return top.clamp(min, max);
    }
    v
}

pub(crate) struct NumericLimits {
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Arrow/spin step size. Defaults to `1` both when the markup `step` is
    /// absent/invalid and when it is the literal `"any"` — `unconstrained`
    /// is what actually marks the "any value" case; `step` just keeps a
    /// sane default increment for arrow keys / spin boxes either way.
    pub step: f64,
    /// `true` for the literal HTML5 `step="any"` (issue #16: written for
    /// `InputLimits { step: None, .. }`): no grid to snap to on clamp/commit,
    /// and the decimal point is allowed when typing, regardless of whether
    /// `step` happens to look like a whole number.
    pub unconstrained: bool,
}

pub(crate) fn numeric_limits(node: &Node) -> NumericLimits {
    let is_range = is_range_input(node);
    // Markup is host input: `parse::<f64>` accepts "NaN" / "inf", and
    // nothing orders min/max. Drop non-finite values and, like HTML, treat a
    // max below min as max = min, so `f64::clamp` below can never panic.
    let min = attr(node, "min")
        .and_then(|s| parse_f64(&s))
        .filter(|v| v.is_finite())
        .or(if is_range { Some(0.0) } else { None });
    let max = attr(node, "max")
        .and_then(|s| parse_f64(&s))
        .filter(|v| v.is_finite())
        .or(if is_range { Some(100.0) } else { None });
    let max = match (min, max) {
        (Some(lo), Some(hi)) if hi < lo => Some(lo),
        _ => max,
    };
    let step_attr = attr(node, "step");
    let unconstrained = step_attr.as_deref() == Some("any");
    let step = step_attr
        .and_then(|s| parse_f64(&s))
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    NumericLimits {
        min,
        max,
        step,
        unconstrained,
    }
}

fn clamp_snap(value: f64, limits: &NumericLimits) -> f64 {
    let value = clamp_safe_number(value);
    let min = limits.min.unwrap_or(-MAX_SAFE_NUMBER);
    let max = limits.max.unwrap_or(MAX_SAFE_NUMBER);
    let max = max.max(min);
    let base = limits.min.unwrap_or(0.0);
    let stepped = if limits.step > 0.0 && !limits.unconstrained {
        // Issue #48: back on the grid's decimals (step, min), no float noise.
        round_decimal(
            base + ((value - base) / limits.step).round() * limits.step,
            result_decimals(limits, 0.0),
        )
    } else {
        value
    };
    clamp_safe_number(stepped.clamp(min, max))
}

/// True for a whole-number box: an integer `step` (not `step="any"`).
/// Typed values there round to an integer on commit (#41 round 2).
pub(crate) fn whole_number_step(limits: &NumericLimits) -> bool {
    !limits.unconstrained && (limits.step - limits.step.round()).abs() < 1e-12
}

/// Clamp to `[min, max]` with no step-grid snap. Used for typed number
/// commits and for re-clamping number inputs when limits change: a typed
/// `151234` with `step=1000` stays `151234` (issue #39).
pub(crate) fn clamp_value(value: f64, limits: &NumericLimits) -> f64 {
    let value = clamp_safe_number(value);
    let min = limits.min.unwrap_or(-MAX_SAFE_NUMBER);
    let max = limits.max.unwrap_or(MAX_SAFE_NUMBER);
    let max = max.max(min);
    clamp_safe_number(value.clamp(min, max))
}

/// Clamp + step-grid snap (range inputs).
pub(crate) fn clamp_snap_value(value: f64, limits: &NumericLimits) -> f64 {
    clamp_snap(value, limits)
}

pub(crate) fn write_value_attr(doc: &mut HtmlDocument, nid: NodeId, new_value: &str) -> bool {
    let old = doc
        .get_node(nid)
        .and_then(|n| attr(n, "value"))
        .unwrap_or_default();
    if old == new_value {
        return false;
    }
    let mut mutr = doc.mutate();
    mutr.set_attribute(nid, qual_name!("value"), new_value);
    true
}

/// `force_include` keeps a `disabled` control in the args anyway — used for
/// an `EnabledWhen`-greyed input (issue #15, K1): still posted with its
/// current value.
fn form_control_pair(
    doc: &HtmlDocument,
    node: &Node,
    force_include: bool,
) -> Option<(String, String)> {
    if !force_include && attr(node, "disabled").is_some() {
        return None;
    }
    let name = named_attr(node, "name")?;
    let tag = tag_name(node);
    match tag.as_str() {
        "input" => {
            let ty = input_type(node);
            match ty.as_str() {
                "number" | "range" | "hidden" | "text" => Some((name, control_value(node))),
                "checkbox" | "radio" => {
                    if !is_checked(node) {
                        return None;
                    }
                    let value = match attr(node, "value") {
                        None => "on".to_string(),
                        Some(v) => v,
                    };
                    Some((name, value))
                }
                // submit / button / file / … stay out of form args.
                _ => None,
            }
        }
        "select" => Some((name, select_value(doc, node))),
        // Buttons in the form are never included (trigger handled separately).
        "button" => None,
        _ => None,
    }
}

fn control_value(node: &Node) -> String {
    attr(node, "value").unwrap_or_default()
}

fn select_value(doc: &HtmlDocument, select: &Node) -> String {
    let mut first: Option<String> = None;
    let mut selected: Option<String> = None;
    for nid in descendants_document_order(doc, select.id) {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if tag_name(node) != "option" {
            continue;
        }
        let val = option_value(node);
        if first.is_none() {
            first = Some(val.clone());
        }
        if attr(node, "selected").is_some() && selected.is_none() {
            selected = Some(val);
        }
    }
    selected.or(first).unwrap_or_default()
}

fn option_value(node: &Node) -> String {
    if let Some(v) = attr(node, "value") {
        return v;
    }
    // HTML: missing value attribute → option's text.
    node.text_content()
}

/// An `<input type="checkbox|radio">` that is not checked (issue #82).
fn is_unchecked_checkable(node: &Node) -> bool {
    tag_name(node) == "input"
        && matches!(input_type(node).as_str(), "checkbox" | "radio")
        && !is_checked(node)
}

fn is_checked(node: &Node) -> bool {
    if let Some(el) = node.element_data() {
        if let Some(checked) = el.checkbox_input_checked() {
            return checked;
        }
    }
    attr(node, "checked").is_some()
}

pub(crate) fn closest_form(doc: &HtmlDocument, nid: NodeId) -> Option<NodeId> {
    let mut parent = doc.get_node(nid)?.parent;
    while let Some(pid) = parent {
        let node = doc.get_node(pid)?;
        if tag_name(node) == "form" {
            return Some(pid);
        }
        parent = node.parent;
    }
    None
}

fn descendants_document_order(doc: &HtmlDocument, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut first = true;
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in node.children.iter().rev() {
            stack.push(child);
        }
        if first {
            first = false;
            continue; // skip the root itself (form or select)
        }
        if node.is_element() {
            out.push(nid);
        }
    }
    out
}

fn tag_name(node: &Node) -> String {
    node.element_data()
        .map(|el| el.name.local.to_string())
        .unwrap_or_default()
}

pub(crate) fn input_type(node: &Node) -> String {
    attr(node, "type")
        .unwrap_or_else(|| "text".to_string())
        .to_ascii_lowercase()
}

fn attr(node: &Node, name: &str) -> Option<String> {
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| a.value.to_string())
    })
}

fn named_attr(node: &Node, name: &str) -> Option<String> {
    attr(node, name).filter(|s| !s.is_empty())
}

pub(crate) fn parse_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Never goes through `i64`: the integer fast path formats the rounded
/// `f64` directly (`{:.0}`, exact for any magnitude up to `MAX_SAFE_NUMBER`,
/// since every integer in that range has an exact `f64` representation), and
/// the decimal path formats the `f64` directly too. The fuzz threshold for
/// "this is basically an integer" is tight (`1e-12`) so a deliberately tiny
/// accepted value (the validated step floor is `1e-10`) is never mistaken
/// for `0`.
pub(crate) fn format_numeric(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let rounded = v.round();
    if (v - rounded).abs() < 1e-12 && rounded.abs() <= MAX_SAFE_NUMBER {
        // `{:.0}` keeps the sign of -0.0 ("-0"); the old i64 path wrote "0".
        if rounded == 0.0 {
            return "0".to_string();
        }
        return format!("{rounded:.0}");
    }
    // Issue #48: the shortest decimal that round-trips, after rounding to
    // MAX_DECIMALS places. A value parsed from `1234567.89` prints exactly
    // that; the old fixed `{:.10}` printed `1234567.8899999999` (the f64's
    // own binary digits). Rust's `f64` Display never uses an exponent.
    let s = format!("{}", round_decimal(v, MAX_DECIMALS));
    if s == "-0" {
        return "0".to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::clamp_snap;
    use super::format_numeric;
    use super::range_snap;
    use super::NumericLimits;
    use super::MAX_SAFE_NUMBER;

    #[test]
    fn snaps_to_step_from_min() {
        let lim = NumericLimits {
            min: Some(0.0),
            max: Some(10.0),
            step: 2.5,
            unconstrained: false,
        };
        assert_eq!(format_numeric(clamp_snap(3.0, &lim)), "2.5");
        assert_eq!(format_numeric(clamp_snap(11.0, &lim)), "10");
        assert_eq!(format_numeric(clamp_snap(-1.0, &lim)), "0");
    }

    /// Blocker 1 (PR27 round 2): `unconstrained` (`step="any"`) skips the
    /// grid-snap entirely — only `[min, max]` clamping applies.
    #[test]
    fn unconstrained_skips_grid_snap() {
        let lim = NumericLimits {
            min: Some(0.0),
            max: Some(10.0),
            step: 1.0,
            unconstrained: true,
        };
        assert_eq!(format_numeric(clamp_snap(4.3, &lim)), "4.3");
        assert_eq!(format_numeric(clamp_snap(11.0, &lim)), "10");
        assert_eq!(format_numeric(clamp_snap(-1.0, &lim)), "0");
    }

    /// Issue #48: no binary float noise from parsing, snapping or stepping.
    #[test]
    fn results_carry_no_float_noise() {
        assert_eq!(format_numeric(1234567.89), "1234567.89");
        assert_eq!(format_numeric(-1234567.89), "-1234567.89");
        assert_eq!(format_numeric(0.1 + 0.2), "0.3");
        let fee = NumericLimits {
            min: Some(0.0),
            max: Some(100.0),
            step: 0.01,
            unconstrained: false,
        };
        // 410 × 0.01 is 4.1000000000000005 in f64.
        assert_eq!(clamp_snap(4.1, &fee), 4.1);
        assert_eq!(format_numeric(clamp_snap(4.1, &fee)), "4.1");
        let mut v = 0.1;
        let tenth = NumericLimits {
            min: None,
            max: None,
            step: 0.1,
            unconstrained: false,
        };
        let mut seen = Vec::new();
        for _ in 0..3 {
            v = super::step_precise(v, 1, &tenth);
            seen.push(v);
        }
        // 3 × 0.1 is 0.30000000000000004 in f64.
        assert_eq!(seen, [0.2, 0.3, 0.4]);
        assert_eq!(super::step_precise(4.09, 1, &fee), 4.1);
        // Min with more decimals than step: the grid keeps min's decimals.
        let off = NumericLimits {
            min: Some(0.005),
            max: Some(1.0),
            step: 0.01,
            unconstrained: false,
        };
        assert_eq!(format_numeric(clamp_snap(0.016, &off)), "0.015");
        assert_eq!(format_numeric(super::step_precise(0.005, 1, &off)), "0.015");
        // step="any": the operand's own decimals survive the step.
        let any = NumericLimits {
            min: None,
            max: None,
            step: 1.0,
            unconstrained: true,
        };
        assert_eq!(super::step_precise(0.7, 1, &any), 1.7);
        // 0.14 + 1 is 1.1400000000000001 in f64.
        assert_eq!(super::step_precise(0.14, 1, &any), 1.14);
        assert_eq!(super::step_precise(1234567.89, 1, &any), 1234568.89);
    }

    /// Graph review of #49 r2: the `-0` guard and the off-grid max guard,
    /// seen on the `f64` itself (the engine's formatter hides `-0`).
    #[test]
    fn range_snap_reaches_an_on_grid_max_and_never_passes_an_off_grid_one() {
        let lim = |min: f64, max: f64, step: f64| NumericLimits {
            min: Some(min),
            max: Some(max),
            step,
            unconstrained: false,
        };
        // On-grid max on a large fractional range (float_noise_48 drag).
        let big = lim(1_000_000.0, 1_234_567.89, 0.01);
        assert_eq!(format_numeric(range_snap(2_000_000.0, &big)), "1234567.89");
        assert_eq!(format_numeric(range_snap(1_234_567.89, &big)), "1234567.89");
        // Off-grid max: the last grid value at or below it.
        assert_eq!(format_numeric(range_snap(10.0, &lim(0.0, 10.0, 3.0))), "9");
        assert_eq!(
            format_numeric(range_snap(5.0, &lim(0.0, 4.99999999, 1.0))),
            "4"
        );
    }

    /// Graph review of #37 r4: the grid starts at `min`, so a `min` with more
    /// decimals than `step` keeps them (0.5, 1.5, …, 9.5), like the arrows.
    #[test]
    fn range_snap_keeps_the_decimals_of_min() {
        let lim = |min: f64, max: f64, step: f64| NumericLimits {
            min: Some(min),
            max: Some(max),
            step,
            unconstrained: false,
        };
        for (l, raw, want) in [
            (lim(0.5, 10.0, 1.0), 1.4, "1.5"),
            (lim(0.5, 10.0, 1.0), 10.0, "9.5"),
            (lim(0.25, 5.0, 0.5), 0.6, "0.75"),
            (lim(0.25, 5.0, 0.5), 5.0, "4.75"),
            (lim(0.05, 1.0, 0.1), 0.05, "0.05"),
            (lim(1.5, 9.0, 2.0), 9.0, "7.5"),
        ] {
            assert_eq!(format_numeric(range_snap(raw, &l)), want, "{raw}");
        }
        let l = lim(0.5, 10.0, 1.0);
        let mut v = 0.5;
        loop {
            let n = super::step_precise(v, 1, &l);
            if n == v {
                break;
            }
            v = n;
        }
        assert_eq!(range_snap(10.0, &l), v, "End agrees with the arrows' top");
    }

    #[test]
    fn step_precise_never_returns_negative_zero_or_passes_max() {
        let lim = NumericLimits {
            min: Some(-0.9),
            max: None,
            step: 0.3,
            unconstrained: false,
        };
        let v = super::step_precise(-0.3, 1, &lim);
        assert_eq!(v, 0.0);
        assert!(v.is_sign_positive(), "-0.0 stored");
        for (max, step, from, want) in [
            (4.99999999, 1.0, 4.0, 4.0),
            (99.9999999999, 0.01, 99.99, 99.99),
            (0.3, 0.1, 0.2, 0.3),
            (9.7, 1.0, 8.5, 9.5),
        ] {
            let lim = NumericLimits {
                min: Some(if step == 1.0 && from == 8.5 { 0.5 } else { 0.0 }),
                max: Some(max),
                step,
                unconstrained: false,
            };
            assert_eq!(super::step_precise(from, 1, &lim), want, "max {max}");
        }
    }

    /// Blocker 2 (PR27 round 2): no `i64` cast, so values at/near the safe
    /// integer boundary and tiny-but-nonzero values both format correctly.
    #[test]
    fn format_numeric_boundaries_no_i64() {
        assert_eq!(format_numeric(MAX_SAFE_NUMBER), "9007199254740992");
        assert_eq!(format_numeric(-MAX_SAFE_NUMBER), "-9007199254740992");
        assert_eq!(format_numeric(1e-10), "0.0000000001");
        assert_eq!(format_numeric(-0.0), "0");
        assert_eq!(format_numeric(-1e-13), "0");
    }

    /// Graph re-review (60e0753): a reversed range must clamp, not panic.
    #[test]
    fn clamp_snap_reversed_bounds_do_not_panic() {
        let lim = NumericLimits {
            min: Some(10.0),
            max: Some(0.0),
            step: 1.0,
            unconstrained: false,
        };
        assert_eq!(format_numeric(clamp_snap(3.0, &lim)), "10");
        assert_eq!(format_numeric(super::step_precise(3.0, 1, &lim)), "10");
    }
}
