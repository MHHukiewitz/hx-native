//! Collect htmx-style form `(name, value)` args for an action trigger.
//!
//! Order: triggering element's own pair first (if named), then named controls
//! in the closest ancestor `<form>` in document order (trigger not repeated).
//!
//! Repeated names (e.g. several checked checkboxes with `name="state"`) each
//! contribute a `(name, value)` pair in document order — the same shape as
//! HTML form serialization and htmx. Use [`args_get`] for the first value and
//! [`args_get_all`] for every value. The args type stays `Vec<(String, String)>`
//! so Mainnet's existing consumers keep working.
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
    if let Some(name) = named_attr(source, "name") {
        args.push((name, trigger_element_value(doc, source)));
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
fn control_current_value(doc: &HtmlDocument, node: &Node) -> Option<String> {
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
                    // Trigger always contributes its value attr (htmx); default "on".
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
/// Uses integer scaled arithmetic so `2.20 + 0.01` stays `2.21` (no float drift).
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
    write_value_attr(doc, nid, &format_numeric_for_step(next, &limits))
}

fn clamp_safe_number(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(-MAX_SAFE_NUMBER, MAX_SAFE_NUMBER)
}

fn scale_to_i64(v: f64, scale: i64) -> Option<i64> {
    let scaled = clamp_safe_number(v) * scale as f64;
    if !scaled.is_finite() || scaled.abs() > i64::MAX as f64 {
        return None;
    }
    Some(scaled.round() as i64)
}

/// Step with integer-scaled math; clamp to min/max; snap to step grid from min.
/// Uses checked scale conversions — values beyond ±2^53 clamp; overflow yields
/// the clamped current value (never panics or wraps).
pub(crate) fn step_precise(current: f64, dir: i32, limits: &NumericLimits) -> f64 {
    let current = clamp_safe_number(current);
    let decimals = step_decimal_places(limits.step);
    let scale = 10i64.pow(decimals as u32);
    let Some(cur_i) = scale_to_i64(current, scale) else {
        return current;
    };
    let Some(step_i) = scale_to_i64(limits.step, scale) else {
        return current;
    };
    let Some(mut next_i) = cur_i.checked_add((dir as i64).checked_mul(step_i).unwrap_or(0)) else {
        return if dir > 0 {
            MAX_SAFE_NUMBER
        } else {
            -MAX_SAFE_NUMBER
        };
    };
    if let Some(min) = limits.min {
        if let Some(min_i) = scale_to_i64(min, scale) {
            if next_i < min_i {
                next_i = min_i;
            }
        }
    }
    if let Some(max) = limits.max {
        if let Some(max_i) = scale_to_i64(max, scale) {
            if next_i > max_i {
                next_i = max_i;
            }
        }
    }
    // Snap to step grid from min (or 0) in integer space.
    let base = limits.min.unwrap_or(0.0);
    if let (Some(base_i), true) = (scale_to_i64(base, scale), step_i > 0) {
        let delta = next_i - base_i;
        let n = if delta >= 0 {
            (delta + step_i / 2) / step_i
        } else {
            (delta - step_i / 2) / step_i
        };
        next_i = base_i + n * step_i;
        if let Some(min) = limits.min {
            if let Some(min_i) = scale_to_i64(min, scale) {
                if next_i < min_i {
                    next_i = min_i;
                }
            }
        }
        if let Some(max) = limits.max {
            if let Some(max_i) = scale_to_i64(max, scale) {
                if next_i > max_i {
                    next_i = max_i;
                }
            }
        }
    }
    clamp_safe_number(next_i as f64 / scale as f64)
}

pub(crate) fn step_decimal_places(step: f64) -> usize {
    if step <= 0.0 || !step.is_finite() {
        return 0;
    }
    // Count decimals in a stable rendering of step (max 10).
    let s = format!("{step:.10}");
    let s = s.trim_end_matches('0');
    match s.split_once('.') {
        Some((_, frac)) => frac.len().min(10),
        None => 0,
    }
}

pub(crate) fn format_numeric_for_step(v: f64, limits: &NumericLimits) -> String {
    let decimals = step_decimal_places(limits.step);
    if decimals == 0 {
        return format_numeric(v);
    }
    let s = format!("{v:.decimals$}");
    // Trim only trailing zeros beyond the step's natural decimals? Keep step precision.
    // "2.20" with step 0.01 stays "2.20"; "2.00" → "2.00" is fine for fee displays.
    s
}

/// Set a range control's value from pointer `x` (document coordinates).
/// Returns `true` when the attribute changed.
pub fn set_range_value_from_x(doc: &mut HtmlDocument, id: &str, x: f32) -> bool {
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
    let min = limits.min.unwrap_or(0.0);
    let max = limits.max.unwrap_or(100.0);
    // Transform-aware box (Pause / centered panels use CSS transform).
    let Some((box_x, _box_y, width, _h)) = crate::damage::element_border_box_css(doc, nid) else {
        return false;
    };
    let ratio = if width <= 0.0 {
        0.0
    } else {
        ((x - box_x) / width).clamp(0.0, 1.0) as f64
    };
    let raw = min + ratio * (max - min);
    let snapped = clamp_snap(raw, &limits);
    write_value_attr(doc, nid, &format_numeric(snapped))
}

pub(crate) struct NumericLimits {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: f64,
}

pub(crate) fn numeric_limits(node: &Node) -> NumericLimits {
    let is_range = is_range_input(node);
    let min = attr(node, "min")
        .and_then(|s| parse_f64(&s))
        .or(if is_range { Some(0.0) } else { None });
    let max = attr(node, "max")
        .and_then(|s| parse_f64(&s))
        .or(if is_range { Some(100.0) } else { None });
    let step = attr(node, "step")
        .and_then(|s| parse_f64(&s))
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0);
    NumericLimits { min, max, step }
}

fn clamp_snap(value: f64, limits: &NumericLimits) -> f64 {
    let value = clamp_safe_number(value);
    let min = limits.min.unwrap_or(-MAX_SAFE_NUMBER);
    let max = limits.max.unwrap_or(MAX_SAFE_NUMBER);
    let base = limits.min.unwrap_or(0.0);
    let stepped = if limits.step > 0.0 {
        base + ((value - base) / limits.step).round() * limits.step
    } else {
        value
    };
    clamp_safe_number(stepped.clamp(min, max))
}

/// Public clamp+snap for text-edit commit.
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

pub(crate) fn format_numeric(v: f64) -> String {
    if v.is_finite() && (v - v.round()).abs() < 1e-9 {
        return (v.round() as i64).to_string();
    }
    // Deterministic trim of trailing zeros.
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::clamp_snap;
    use super::format_numeric;
    use super::NumericLimits;

    #[test]
    fn snaps_to_step_from_min() {
        let lim = NumericLimits {
            min: Some(0.0),
            max: Some(10.0),
            step: 2.5,
        };
        assert_eq!(format_numeric(clamp_snap(3.0, &lim)), "2.5");
        assert_eq!(format_numeric(clamp_snap(11.0, &lim)), "10");
        assert_eq!(format_numeric(clamp_snap(-1.0, &lim)), "0");
    }
}
