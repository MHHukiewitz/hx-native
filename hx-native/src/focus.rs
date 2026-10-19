//! Document-order focus walking (Tab / Shift+Tab).
//!
//! Engine-owned list: buttons and `[tabindex>=0]`, skipping disabled / greyed /
//! `tabindex="-1"`. Complements Blitz's `is_focussable` when discovery is thin.

use blitz_dom::node::Node;
use blitz_html::HtmlDocument;

/// Collect focusable element ids in document (tree) order.
pub fn focusable_ids(doc: &HtmlDocument) -> Vec<String> {
    let mut out = Vec::new();
    let root = doc.root_node().id;
    let mut stack = vec![root];
    // DFS preorder via explicit stack (children reverse-pushed for left-to-right).
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in node.children.iter().rev() {
            stack.push(child);
        }
        if !node.is_element() {
            continue;
        }
        if !is_focusable_element(node) {
            continue;
        }
        if let Some(id) = element_id(node) {
            if !id.is_empty() {
                out.push(id);
            }
        }
    }
    out
}

fn element_id(node: &Node) -> Option<String> {
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == "id")
            .map(|a| a.value.to_string())
    })
}

fn attr(node: &Node, name: &str) -> Option<String> {
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| a.value.to_string())
    })
}

fn tag_name(node: &Node) -> String {
    node.element_data()
        .map(|el| el.name.local.to_string())
        .unwrap_or_default()
}

/// Focusable per A21 contract: buttons / tabindex>=0, not disabled/greyed.
pub fn is_focusable_element(node: &Node) -> bool {
    // Disabled / greyed (aria-disabled=true or disabled attribute present).
    if attr(node, "disabled").is_some() {
        return false;
    }
    if attr(node, "aria-disabled").as_deref() == Some("true") {
        return false;
    }

    let tabindex = attr(node, "tabindex").and_then(|s| s.parse::<i32>().ok());
    if let Some(idx) = tabindex {
        return idx >= 0;
    }

    // Default focusable without tabindex: button (and a few HTML controls).
    let tag = tag_name(node);
    matches!(
        tag.as_str(),
        "button" | "input" | "select" | "textarea" | "summary"
    ) || (tag == "a" && attr(node, "href").is_some())
}

/// Next / previous focusable id after `current` (None = before first / after last).
pub fn step_focus(order: &[String], current: Option<&str>, backward: bool) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    match current {
        None => {
            if backward {
                order.last().cloned()
            } else {
                order.first().cloned()
            }
        }
        Some(cur) => {
            let pos = order.iter().position(|id| id == cur);
            match pos {
                None => {
                    if backward {
                        order.last().cloned()
                    } else {
                        order.first().cloned()
                    }
                }
                Some(i) => {
                    if backward {
                        let next = if i == 0 { order.len() - 1 } else { i - 1 };
                        Some(order[next].clone())
                    } else {
                        let next = (i + 1) % order.len();
                        Some(order[next].clone())
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::step_focus;

    #[test]
    fn steps_forward_and_wraps() {
        let order = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(step_focus(&order, None, false).as_deref(), Some("a"));
        assert_eq!(step_focus(&order, Some("a"), false).as_deref(), Some("b"));
        assert_eq!(step_focus(&order, Some("c"), false).as_deref(), Some("a"));
        assert_eq!(step_focus(&order, Some("a"), true).as_deref(), Some("c"));
    }
}
