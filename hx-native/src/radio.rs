//! Native radio buttons: Chrome baseline and scale-invariant ring (issue #116).
//!
//! Two pin-sensitive workarounds for Blitz 0.3.0-beta.4 (see BLITZ_PIN.md):
//!
//! 1. **Baseline.** A radio has no baseline of its own in Blitz, so parley puts
//!    the *margin-box* bottom on the line baseline. Chrome puts the
//!    *border-box* bottom there and lets `margin-bottom` only extend the line
//!    box below it. [`RadioShifts`] moves every in-flow inline radio down by
//!    its resolved `margin-bottom` after each layout (both the unrounded and
//!    the final layout, so paint, hit tests and boxes agree). The shift is
//!    undone right before the next layout, so Blitz's layout cache never sees
//!    a shifted value and the pass is idempotent. Radios with
//!    `vertical-align: middle | text-top | text-bottom | top | bottom |
//!    center` align by their margin box in both engines and are left alone.
//!
//! 2. **Ring scale.** `blitz-paint` sizes the ring from
//!    `(min_device_dim - 4) / 16`, so the `4` is a device-px constant and the
//!    ring is 10 CSS px at scale 1 but 12 at scale 2. [`RadioRings`] records
//!    where each radio paints and rescales the three circles (ring, gap, dot)
//!    to `(min_css_dim - 4) / 16 * scale`, i.e. the scale 1 geometry in CSS px
//!    at every scale. A circle that does not match a recorded radio paints
//!    unchanged.
//!
//! Not covered: checkboxes (same Chrome gap, left alone), radios that are
//! flex/grid/block-level items (no inline baseline involved), and the
//! scrollable overflow of ancestors (not extended by the shift).

use blitz_dom::node::Node;
use blitz_dom::NodeId;
use blitz_html::HtmlDocument;
use kurbo::{Affine, Circle, Point, Shape};
use parley::InlineBoxKind;
use style::values::computed::{AlignmentBaseline, BaselineShift};
use style::values::generics::box_::BaselineShiftKeyword;

use crate::damage::document_affine_device;

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attrs()?
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| &*a.value)
}

/// A rendered `<input type=radio>` (the only kind `draw_input` paints as one).
fn is_radio(node: &Node) -> bool {
    node.is_element()
        && attr(node, "type") == Some("radio")
        && node
            .element_data()
            .is_some_and(|e| &*e.name.local == "input" && e.checkbox_input_checked().is_some())
}

fn radios(doc: &HtmlDocument) -> Vec<NodeId> {
    doc.tree()
        .iter()
        .filter(|(_, n)| is_radio(n))
        .map(|(_, n)| n.id)
        .collect()
}

/// Whether `nid` is an in-flow inline box of an inline formatting context.
fn in_inline_flow(doc: &HtmlDocument, nid: NodeId) -> bool {
    let mut cur = doc.get_node(nid).and_then(|n| n.layout_parent.get());
    while let Some(pid) = cur {
        let Some(parent) = doc.get_node(pid) else {
            return false;
        };
        if let Some(il) = parent
            .element_data()
            .and_then(|e| e.inline_layout_data.as_ref())
        {
            return il
                .layout
                .inline_boxes()
                .any(|b| b.id == nid.as_u64() && b.kind == InlineBoxKind::InFlow);
        }
        cur = parent.layout_parent.get();
    }
    false
}

/// Alignments that use the baseline of the box (so Chrome's border-box
/// baseline matters). Everything else aligns by the margin box.
fn aligns_by_baseline(node: &Node) -> bool {
    let Some(style) = node.primary_styles() else {
        return false;
    };
    let b = style.get_box();
    b.clone_alignment_baseline() == AlignmentBaseline::Baseline
        && !matches!(
            b.clone_baseline_shift(),
            BaselineShift::Keyword(
                BaselineShiftKeyword::Top
                    | BaselineShiftKeyword::Bottom
                    | BaselineShiftKeyword::Center
            )
        )
}

#[derive(Debug, Clone, Copy)]
struct Applied {
    nid: NodeId,
    /// Layout `y` values as written by [`RadioShifts::apply`].
    unrounded_y: f32,
    final_y: f32,
    /// What was added to each.
    d_unrounded: f32,
    d_final: f32,
}

/// Post-layout baseline shifts currently applied to the document.
#[derive(Debug, Default)]
pub(crate) struct RadioShifts {
    applied: Vec<Applied>,
}

impl RadioShifts {
    /// Blitz layout with the radio baseline shifts undone before and redone
    /// after, so the layout cache only ever holds Blitz's own values.
    pub(crate) fn resolve(&mut self, doc: &mut HtmlDocument) {
        self.undo(doc);
        doc.resolve(0.0);
        self.apply(doc);
    }

    fn undo(&mut self, doc: &mut HtmlDocument) {
        for a in std::mem::take(&mut self.applied) {
            let Some(node) = doc.get_node_mut(a.nid) else {
                continue;
            };
            // Only undo what is still exactly what we wrote (the node may
            // have been replaced by a swap that reused the id).
            if node.unrounded_layout().location.y == a.unrounded_y
                && node.final_layout().location.y == a.final_y
            {
                node.unrounded_layout_mut().location.y -= a.d_unrounded;
                node.final_layout_mut().location.y -= a.d_final;
            }
        }
    }

    fn apply(&mut self, doc: &mut HtmlDocument) {
        let mut todo: Vec<(NodeId, f32)> = Vec::new();
        for nid in radios(doc) {
            let Some(node) = doc.get_node(nid) else {
                continue;
            };
            let mb = node.unrounded_layout().margin.bottom;
            if mb == 0.0 || !aligns_by_baseline(node) || !in_inline_flow(doc, nid) {
                continue;
            }
            todo.push((nid, mb));
        }
        for (nid, mb) in todo {
            let Some(node) = doc.get_node_mut(nid) else {
                continue;
            };
            // Layout positions are whole CSS px; keep them so.
            let d_final = mb.round();
            node.unrounded_layout_mut().location.y += mb;
            node.final_layout_mut().location.y += d_final;
            self.applied.push(Applied {
                nid,
                unrounded_y: node.unrounded_layout().location.y,
                final_y: node.final_layout().location.y,
                d_unrounded: mb,
                d_final,
            });
        }
    }
}

/// Max distance (device px) between a recorded and a painted ring centre.
const CENTRE_TOLERANCE: f64 = 1.0;

#[derive(Debug, Clone, Copy)]
struct Ring {
    /// Device-space centre of the painted radio.
    centre: Point,
    /// Radius multiplier: wanted ring scale over Blitz's.
    ratio: f64,
    /// Blitz's ring scale: radii are `8k`, `6k`, `4k` in the local frame.
    k: f64,
}

/// Radios of one document at one paint scale.
#[derive(Debug, Default)]
pub(crate) struct RadioRings {
    rings: Vec<Ring>,
}

impl RadioRings {
    pub(crate) fn build(doc: &HtmlDocument, scale: f64) -> Self {
        let mut rings = Vec::new();
        for nid in radios(doc) {
            let Some(node) = doc.get_node(nid) else {
                continue;
            };
            let l = node.final_layout();
            let (w, h) = (l.size.width as f64, l.size.height as f64);
            // Blitz: scale = (min device dim - 4) / 16; wanted: (min css dim - 4) / 16 * scale.
            let k = ((w.min(h) * scale) - 4.0).max(0.0) / 16.0;
            let want = (w.min(h) - 4.0).max(0.0) / 16.0 * scale;
            if k <= 0.0 || (want - k).abs() < 1e-9 {
                continue;
            }
            let Some(xf) = document_affine_device(doc, nid, scale) else {
                continue;
            };
            let centre = xf * Point::new(w * scale / 2.0, h * scale / 2.0);
            rings.push(Ring {
                centre,
                ratio: want / k,
                k,
            });
        }
        Self { rings }
    }

    /// The circle to paint instead of `shape`, if it is one of a recorded
    /// radio's ring, gap or dot circles.
    pub(crate) fn rescale(&self, transform: Affine, shape: &impl Shape) -> Option<Circle> {
        if self.rings.is_empty() {
            return None;
        }
        let c = shape.as_circle()?;
        let at = transform * c.center;
        let ring = self.rings.iter().find(|r| {
            (r.centre.x - at.x).abs() <= CENTRE_TOLERANCE
                && (r.centre.y - at.y).abs() <= CENTRE_TOLERANCE
                && [8.0, 6.0, 4.0]
                    .iter()
                    .any(|m| (c.radius - m * r.k).abs() < 1e-6)
        })?;
        Some(Circle::new(c.center, c.radius * ring.ratio))
    }
}
