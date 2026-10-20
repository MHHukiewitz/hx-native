//! Engine-owned form controls: checkbox/radio toggle, label activation,
//! `<output for>`, disabled gates, and select option helpers (issue #14).

use blitz_dom::node::Node;
use blitz_dom::{qual_name, BaseDocument, NodeId};
use blitz_html::HtmlDocument;

/// Kind of interactive form control under the pointer / focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormControlKind {
    Range,
    Number,
    Checkbox,
    Radio,
    Select,
}

#[derive(Clone, Debug)]
pub struct FormControlRef {
    pub node_id: NodeId,
    /// Element `id` when present (needed to fire `hx-trigger` actions).
    pub element_id: Option<String>,
    pub kind: FormControlKind,
}

/// True when the element carries a `disabled` attribute.
pub fn is_disabled(node: &Node) -> bool {
    attr(node, "disabled").is_some()
}

/// Resolve the form control at document point `(x, y)`.
///
/// Walks from the hit node upward so a click on a `<label>` or its decorative
/// child (e.g. `<i>`) finds the associated control even when the input itself
/// is `pointer-events:none` / zero-size (`.vh` in mock 27).
pub fn form_control_at_point(doc: &HtmlDocument, x: f32, y: f32) -> Option<FormControlRef> {
    let hit = doc.element_from_point(x, y)?;
    form_control_from_node(doc, hit)
}

/// Resolve a form control from a node (or an ancestor / associated label).
pub fn form_control_from_node(doc: &HtmlDocument, start: NodeId) -> Option<FormControlRef> {
    let mut cur = Some(start);
    while let Some(nid) = cur {
        let Some(node) = doc.get_node(nid) else {
            break;
        };
        if let Some(ctrl) = classify_control(doc, node) {
            return Some(ctrl);
        }
        if tag_name(node) == "label" {
            if let Some(ctrl) = label_control(doc, nid, node) {
                return Some(ctrl);
            }
        }
        cur = node.parent;
    }
    None
}

fn classify_control(_doc: &HtmlDocument, node: &Node) -> Option<FormControlRef> {
    let tag = tag_name(node);
    let kind = match tag.as_str() {
        "input" => match input_type(node).as_str() {
            "range" => FormControlKind::Range,
            "number" => FormControlKind::Number,
            "checkbox" => FormControlKind::Checkbox,
            "radio" => FormControlKind::Radio,
            _ => return None,
        },
        "select" => FormControlKind::Select,
        _ => return None,
    };
    Some(FormControlRef {
        node_id: node.id,
        element_id: element_id(node),
        kind,
    })
}

/// Associated control for a `<label>`: `for="id"` target, else first labelable descendant.
fn label_control(doc: &HtmlDocument, _label_nid: NodeId, label: &Node) -> Option<FormControlRef> {
    if let Some(for_id) = named_attr(label, "for") {
        let nid = doc.get_element_by_id(&for_id)?;
        let node = doc.get_node(nid)?;
        return classify_control(doc, node);
    }
    for nid in descendants_document_order(doc, label.id) {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if let Some(ctrl) = classify_control(doc, node) {
            return Some(ctrl);
        }
    }
    None
}

/// Toggle a checkbox. Returns `true` when the checked state flipped.
pub fn toggle_checkbox(doc: &mut HtmlDocument, node_id: NodeId) -> bool {
    let Some(node) = doc.get_node(node_id) else {
        return false;
    };
    if is_disabled(node) {
        return false;
    }
    if !matches!(
        classify_control(doc, node).map(|c| c.kind),
        Some(FormControlKind::Checkbox)
    ) {
        return false;
    }
    // Snapshot so `:checked` (and e.g. `input:checked + i`) invalidates.
    doc.snapshot_node_state_only(node_id);
    let Some(node) = doc.get_node_mut(node_id) else {
        return false;
    };
    let Some(el) = node.element_data_mut() else {
        return false;
    };
    let before = el.checkbox_input_checked();
    let after = BaseDocument::toggle_checkbox(el);
    node.mark_ancestors_dirty();
    before != Some(after)
}

/// Result of selecting a radio: whether state changed, and the previously
/// checked radio in the same `name` group (for damage).
#[derive(Clone, Debug)]
pub struct RadioSelectResult {
    pub previous_checked: Option<NodeId>,
}

/// Currently checked radio with `name` (document-wide, matching Blitz
/// `toggle_radio` which ignores form owner).
pub fn checked_radio_in_group(doc: &HtmlDocument, name: &str) -> Option<NodeId> {
    if name.is_empty() {
        return None;
    }
    let root = doc.root_node().id;
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in &node.children {
            stack.push(child);
        }
        if !matches!(
            classify_control(doc, node).map(|c| c.kind),
            Some(FormControlKind::Radio)
        ) {
            continue;
        }
        if attr(node, "name").as_deref() != Some(name) {
            continue;
        }
        if node
            .element_data()
            .and_then(|el| el.checkbox_input_checked())
            .unwrap_or(false)
        {
            return Some(nid);
        }
    }
    None
}

/// Select a radio in its name group (exclusive).
///
/// Returns `Some(RadioSelectResult)` when state changed, `None` otherwise.
/// Caller must damage `previous_checked` when it differs from `node_id`.
pub fn select_radio(doc: &mut HtmlDocument, node_id: NodeId) -> Option<RadioSelectResult> {
    let node = doc.get_node(node_id)?;
    if is_disabled(node) {
        return None;
    }
    if !matches!(
        classify_control(doc, node).map(|c| c.kind),
        Some(FormControlKind::Radio)
    ) {
        return None;
    }
    let already = node
        .element_data()
        .and_then(|el| el.checkbox_input_checked())
        .unwrap_or(false);
    if already {
        return None;
    }
    let name = attr(node, "name").unwrap_or_default();
    let previous_checked = if name.is_empty() {
        None
    } else {
        checked_radio_in_group(doc, &name)
    };
    if name.is_empty() {
        doc.snapshot_node_state_only(node_id);
        let node = doc.get_node_mut(node_id)?;
        if let Some(el) = node.element_data_mut() {
            el.set_checkbox_input_checked(true);
        }
        node.mark_ancestors_dirty();
        return Some(RadioSelectResult { previous_checked });
    }
    doc.toggle_radio(name, node_id);
    Some(RadioSelectResult { previous_checked })
}

/// Space on a focused checkbox/radio toggles or selects it.
/// Prefer [`select_radio`] when the previous checked radio must be damaged.
pub fn activate_checkable(doc: &mut HtmlDocument, node_id: NodeId) -> bool {
    let Some(node) = doc.get_node(node_id) else {
        return false;
    };
    match classify_control(doc, node).map(|c| c.kind) {
        Some(FormControlKind::Checkbox) => toggle_checkbox(doc, node_id),
        Some(FormControlKind::Radio) => select_radio(doc, node_id).is_some(),
        _ => false,
    }
}

/// Update every `<output for="id">` to show the control's current display value.
/// Returns `true` when any output text changed.
pub fn sync_outputs_for(doc: &mut HtmlDocument, control_id: &str) -> bool {
    let Some(ctrl_nid) = doc.get_element_by_id(control_id) else {
        return false;
    };
    let Some(ctrl) = doc.get_node(ctrl_nid) else {
        return false;
    };
    let display = output_display_value(doc, ctrl);
    let targets: Vec<NodeId> = {
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
            if !node.is_element() {
                continue;
            }
            if tag_name(node) != "output" {
                continue;
            }
            if attr(node, "for").as_deref() == Some(control_id) {
                out.push(nid);
            }
        }
        out
    };
    let mut changed = false;
    for out_nid in targets {
        if set_element_text(doc, out_nid, &display) {
            changed = true;
        }
    }
    changed
}

fn output_display_value(doc: &HtmlDocument, ctrl: &Node) -> String {
    let tag = tag_name(ctrl);
    match tag.as_str() {
        "input" => {
            let ty = input_type(ctrl);
            match ty.as_str() {
                "range" | "number" => {
                    let raw = attr(ctrl, "value").unwrap_or_default();
                    // Settings sample formats volume as `55%`.
                    if ty == "range" {
                        if let Ok(v) = raw.parse::<f64>() {
                            let max = attr(ctrl, "max")
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(100.0);
                            if (max - 1.0).abs() < 1e-9 {
                                return format!("{}%", (v * 100.0).round() as i64);
                            }
                            if (max - 100.0).abs() < 1e-9 {
                                return format!("{}%", v.round() as i64);
                            }
                        }
                    }
                    raw
                }
                "checkbox" | "radio" => {
                    let checked = ctrl
                        .element_data()
                        .and_then(|el| el.checkbox_input_checked())
                        .unwrap_or(false);
                    if checked {
                        attr(ctrl, "value").unwrap_or_else(|| "on".into())
                    } else {
                        String::new()
                    }
                }
                _ => attr(ctrl, "value").unwrap_or_default(),
            }
        }
        "select" => select_selected_text(doc, ctrl),
        _ => attr(ctrl, "value").unwrap_or_default(),
    }
}

fn set_element_text(doc: &mut HtmlDocument, nid: NodeId, text: &str) -> bool {
    let old = doc
        .get_node(nid)
        .map(|n| n.text_content())
        .unwrap_or_default();
    if old == text {
        return false;
    }
    {
        let mut mutr = doc.mutate();
        mutr.remove_and_drop_all_children(nid);
        let text_id = mutr.create_text_node(text);
        mutr.append_children(nid, &[text_id]);
    }
    true
}

/// Option entries of a `<select>` (value + display text), document order.
#[derive(Clone, Debug)]
pub struct SelectOption {
    pub node_id: NodeId,
    #[allow(dead_code)]
    pub value: String,
    pub text: String,
    pub selected: bool,
    pub disabled: bool,
}

pub fn select_options(doc: &HtmlDocument, select_nid: NodeId) -> Vec<SelectOption> {
    let mut out = Vec::new();
    for nid in descendants_document_order(doc, select_nid) {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        if tag_name(node) != "option" {
            continue;
        }
        let value = option_value(node);
        let text = node.text_content();
        let selected = attr(node, "selected").is_some();
        let disabled = is_disabled(node);
        out.push(SelectOption {
            node_id: nid,
            value,
            text,
            selected,
            disabled,
        });
    }
    out
}

pub fn select_selected_index(options: &[SelectOption]) -> usize {
    options
        .iter()
        .position(|o| o.selected)
        .unwrap_or(0)
        .min(options.len().saturating_sub(1))
}

pub fn select_selected_text(doc: &HtmlDocument, select: &Node) -> String {
    let opts = select_options(doc, select.id);
    if opts.is_empty() {
        return String::new();
    }
    let i = select_selected_index(&opts);
    opts[i].text.clone()
}

/// Set the selected option by index. Clears other `selected` attrs.
/// Returns `true` when the selection changed.
pub fn select_set_index(doc: &mut HtmlDocument, select_nid: NodeId, index: usize) -> bool {
    let opts = select_options(doc, select_nid);
    if opts.is_empty() || index >= opts.len() {
        return false;
    }
    if opts[index].disabled {
        return false;
    }
    let prev = select_selected_index(&opts);
    if prev == index && opts[index].selected {
        return false;
    }
    {
        let mut mutr = doc.mutate();
        for (i, opt) in opts.iter().enumerate() {
            if i == index {
                mutr.set_attribute(opt.node_id, qual_name!("selected"), "");
            } else if opt.selected {
                mutr.clear_attribute(opt.node_id, qual_name!("selected"));
            }
        }
    }
    true
}

/// Step the closed select by `dir` (±1), wrapping at ends among enabled options.
pub fn select_step(doc: &mut HtmlDocument, select_nid: NodeId, dir: i32) -> bool {
    let opts = select_options(doc, select_nid);
    if opts.is_empty() {
        return false;
    }
    let cur = select_selected_index(&opts) as i32;
    let n = opts.len() as i32;
    for step in 1..=n {
        let next = (cur + dir * step).rem_euclid(n) as usize;
        if !opts[next].disabled {
            return select_set_index(doc, select_nid, next);
        }
    }
    false
}

pub fn control_is_disabled(doc: &HtmlDocument, node_id: NodeId) -> bool {
    doc.get_node(node_id).is_some_and(is_disabled)
}

/// Issue #15: checked state of the first `<input type=checkbox name=name>`
/// inside `form_nid` (document order). `None` when no such checkbox exists.
pub(crate) fn checkbox_checked_by_name(
    doc: &HtmlDocument,
    form_nid: NodeId,
    name: &str,
) -> Option<bool> {
    for nid in descendants_document_order(doc, form_nid) {
        let node = doc.get_node(nid)?;
        if tag_name(node) != "input" || input_type(node) != "checkbox" {
            continue;
        }
        if attr(node, "name").as_deref() != Some(name) {
            continue;
        }
        return Some(checkbox_checked_state(node));
    }
    None
}

fn checkbox_checked_state(node: &Node) -> bool {
    node.element_data()
        .and_then(|el| el.checkbox_input_checked())
        .unwrap_or_else(|| attr(node, "checked").is_some())
}

/// Issue #15: every `<input>` / `<select>` with `name == name` inside
/// `form_nid` (document order) — the dependent-input targets of an
/// `InputRule`.
pub(crate) fn named_controls_in_form(doc: &HtmlDocument, form_nid: NodeId, name: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    for nid in descendants_document_order(doc, form_nid) {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        let tag = tag_name(node);
        if !matches!(tag.as_str(), "input" | "select") {
            continue;
        }
        if attr(node, "name").as_deref() == Some(name) {
            out.push(nid);
        }
    }
    out
}

pub fn is_change_eligible_tag(tag: &str, input_ty: Option<&str>) -> bool {
    match tag {
        "select" | "textarea" => true,
        "input" => matches!(
            input_ty.unwrap_or("text"),
            "range" | "number" | "checkbox" | "radio" | "text" | "password" | "search"
        ),
        _ => false,
    }
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
            continue;
        }
        if node.is_element() {
            out.push(nid);
        }
    }
    out
}

fn element_id(node: &Node) -> Option<String> {
    named_attr(node, "id")
}

fn option_value(node: &Node) -> String {
    if let Some(v) = attr(node, "value") {
        return v;
    }
    node.text_content()
}

fn tag_name(node: &Node) -> String {
    node.element_data()
        .map(|el| el.name.local.to_string())
        .unwrap_or_default()
}

fn input_type(node: &Node) -> String {
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
