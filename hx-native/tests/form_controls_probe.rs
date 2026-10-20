//! Throwaway probe for issues #14/#15/#16 (Blitz 0.3.0-beta.4 + hx-native).
//! Not acceptance; documents API facts. Safe to delete with the probe branch.

use std::sync::Arc;
use std::time::Instant;

use blitz_dom::BaseDocument;
use blitz_dom::{local_name, qual_name, DocumentConfig, StyleThreading};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};
use hx_native::*;

fn doc_from(html: &str) -> HtmlDocument {
    HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(BlitzViewport::new(400, 200, 1.0, ColorScheme::Light)),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            style_threading: StyleThreading::Sequential,
            incremental: Some(true),
            ..Default::default()
        },
    )
}

fn attr_of(doc: &HtmlDocument, id: &str, name: &str) -> Option<String> {
    let nid = doc.get_element_by_id(id)?;
    let node = doc.get_node(nid)?;
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| a.value.to_string())
    })
}

fn boot_engine(html: &str) -> Engine {
    let mut eng = Engine::new(EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    });
    eng.declare_actions(&["probe.act"]);
    let wrapped = format!("<!DOCTYPE html><html><body>{html}</body></html>");
    eng.load_html(&wrapped).expect("load");
    let _ = eng.paint(Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    });
    eng
}

/// Blitz: checkbox state lives in `SpecialElementData::CheckboxInput`; toggle is public.
#[test]
fn probe_blitz_checkbox_toggle_api() {
    let mut doc = doc_from(
        r#"<!DOCTYPE html><html><body>
<input id="c" type="checkbox" name="music" checked="" />
</body></html>"#,
    );
    doc.resolve(0.0);
    let nid = doc.get_element_by_id("c").expect("id");
    {
        let node = doc.get_node(nid).unwrap();
        let el = node.element_data().unwrap();
        assert_eq!(el.checkbox_input_checked(), Some(true));
    }
    // Same API events/pointer.rs handle_click uses (without EventDriver).
    {
        let node = doc.get_node_mut(nid).unwrap();
        if let Some(el) = node.element_data_mut() {
            let now = BaseDocument::toggle_checkbox(el);
            assert!(!now);
        }
    }
    let el = doc.get_node(nid).unwrap().element_data().unwrap();
    assert_eq!(el.checkbox_input_checked(), Some(false));
}

/// Blitz: `<option>` is `display:none` in UA CSS; no select popup in handle_click.
#[test]
fn probe_blitz_select_options_are_dom_children() {
    let mut doc = doc_from(
        r#"<!DOCTYPE html><html><body>
<select id="lang" name="language">
  <option value="en" selected="">English</option>
  <option value="de">Deutsch</option>
</select>
</body></html>"#,
    );
    doc.resolve(0.0);
    let select = doc.get_element_by_id("lang").unwrap();
    let select_node = doc.get_node(select).unwrap();
    assert!(
        select_node.children.len() >= 2,
        "options exist as DOM children (UA CSS hides them; no dropdown handler)"
    );
}

/// Runtime min/max/step attrs stick after mutate (Blitz has no range widget).
#[test]
fn probe_runtime_min_max_step_mutation() {
    let mut doc = doc_from(
        r#"<!DOCTYPE html><html><body>
<input id="vol" type="range" name="music_volume" value="0.5" min="0" max="1" step="0.05" />
</body></html>"#,
    );
    doc.resolve(0.0);
    let nid = doc.get_element_by_id("vol").unwrap();
    {
        let mut mutr = doc.mutate();
        mutr.set_attribute(nid, qual_name!("min"), "0.1");
        mutr.set_attribute(nid, qual_name!("max"), "0.9");
        mutr.set_attribute(nid, qual_name!("step"), "0.1");
    }
    assert_eq!(attr_of(&doc, "vol", "min").as_deref(), Some("0.1"));
    assert_eq!(attr_of(&doc, "vol", "max").as_deref(), Some("0.9"));
    assert_eq!(attr_of(&doc, "vol", "step").as_deref(), Some("0.1"));
}

/// disabled via mutate; checked="" does NOT flip CheckboxInput (needs "true"/"false").
#[test]
fn probe_disabled_and_checked_attr_quirks() {
    let mut doc = doc_from(
        r#"<!DOCTYPE html><html><body>
<input id="c" type="checkbox" name="x" />
<input id="r" type="range" name="y" value="1" min="0" max="10" />
</body></html>"#,
    );
    doc.resolve(0.0);
    let cid = doc.get_element_by_id("c").unwrap();
    let rid = doc.get_element_by_id("r").unwrap();
    {
        let mut mutr = doc.mutate();
        mutr.set_attribute(cid, qual_name!("disabled"), "");
        mutr.set_attribute(rid, qual_name!("disabled"), "");
        mutr.set_attribute(cid, qual_name!("checked"), "");
    }
    let c = doc.get_node(cid).unwrap().element_data().unwrap();
    assert!(c.has_attr(local_name!("disabled")));
    assert_eq!(
        c.checkbox_input_checked(),
        Some(false),
        "checked=\"\" does not parse as bool; use toggle_checkbox or checked=\"true\""
    );
    {
        let mut mutr = doc.mutate();
        mutr.set_attribute(cid, qual_name!("checked"), "true");
    }
    let c = doc.get_node(cid).unwrap().element_data().unwrap();
    assert_eq!(c.checkbox_input_checked(), Some(true));
}

/// hx-native: arrow keys step a focused range (engine-owned, not Blitz).
#[test]
fn probe_hx_range_arrow_steps() {
    let html = r#"
<form id="f">
  <input id="spd" type="range" name="speed" value="0" min="0" max="4" step="1" />
  <button id="go" name="op" value="set" hx-post="probe.act">Go</button>
</form>"#;
    let mut eng = boot_engine(html);
    eng.test_set_focus("spd");
    let _ = eng.input(InputEvent::Key(KeyEvent {
        key: "ArrowRight".into(),
        code: "ArrowRight".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    assert_eq!(eng.attr("spd", "value").as_deref(), Some("1"));
}

/// Cheap form snapshot: ~15 controls via existing collect_action_args path.
#[test]
fn probe_form_snapshot_cost_fifteen_controls() {
    let mut inputs = String::new();
    for i in 0..15 {
        inputs.push_str(&format!(
            r#"<input type="hidden" name="k{i}" value="v{i}" />"#
        ));
    }
    let html = format!(
        r#"<form id="f">{inputs}<button id="go" name="op" value="1" hx-post="probe.act">Go</button></form>"#
    );
    let mut eng = boot_engine(&html);
    eng.test_set_focus("go");
    let t0 = Instant::now();
    let out = eng.input(InputEvent::Key(KeyEvent {
        key: "Enter".into(),
        code: "Enter".into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed: true,
    }));
    let ns = t0.elapsed().as_nanos();
    match out {
        InputOutcome::Action { args, .. } => {
            assert!(args.len() >= 16, "op + 15 hiddens, got {}", args.len());
        }
        other => panic!("expected Action, got {other:?}"),
    }
    assert!(
        ns < 5_000_000,
        "15-control form args should be well under 5ms, got {ns} ns"
    );
}
