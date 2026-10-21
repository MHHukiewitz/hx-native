//! Graph re-review of PR #24 at 7466279: three suite gaps the round-2 tests
//! leave open (ancestor-swap host `disabled`, unrelated swap keeps an
//! engine-owned grey, commit damage upper bound).
use std::collections::HashMap;

use hx_native::*;

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 300,
        scale: 1.0,
        holes: vec![],
    }
}

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..Default::default()
    }
}

const CSS: &str = "<style>input:disabled{background:#888}</style>";

fn dep(host_disabled: bool) -> String {
    format!(
        "<div id=\"box\"><input id=\"dep\" type=\"text\" name=\"dep\" value=\"x\" {} \
         style=\"position:absolute;left:10px;top:50px;width:120px;height:22px\"/></div>",
        if host_disabled { "disabled" } else { "" }
    )
}

fn html() -> String {
    format!(
        "<!DOCTYPE html><html><head>{CSS}</head><body style=\"margin:0;width:400px;height:300px;background:#fff\">\
         <form id=\"g\"><input id=\"gate\" type=\"checkbox\" name=\"gate\" style=\"position:absolute;left:10px;top:10px\"/>\
         <section id=\"sec\" hx-trigger=\"slice:panel\">{}</section>\
         <section id=\"sec2\" hx-trigger=\"slice:other\"><span id=\"o\">other</span></section>\
         <input id=\"solo\" type=\"checkbox\" name=\"solo\" style=\"position:absolute;left:300px;top:200px\"/>\
         <button id=\"save\" hx-post=\"settings.save\" hx-trigger=\"click\" \
         style=\"position:absolute;left:10px;top:90px;width:80px;height:30px\">Save</button></form></body></html>",
        dep(false)
    )
}

fn pkt(panel: u64, other: u64, sec: &str, sec2: &str) -> SyncPacket {
    // `SyncPacket` is `#[non_exhaustive]` (#27): default() plus assignment.
    let mut p = SyncPacket::default();
    p.versions = HashMap::from([
        ("actions".into(), 1),
        ("panel".into(), panel),
        ("other".into(), other),
    ]);
    p.input_rules = vec![(
        "settings.save".into(),
        "dep".into(),
        InputRule::EnabledWhen {
            input: "gate".into(),
            checked: true,
            reason: "gate off".into(),
        },
    )];
    p.fragments.insert("sec".into(), sec.into());
    p.fragments.insert("sec2".into(), sec2.into());
    p
}

fn sync(e: &mut Engine, p: SyncPacket) {
    e.sync(&p.clone(), &mut p.clone()).unwrap();
    let _ = e.paint(vp());
}

fn click(e: &mut Engine, id: &str) {
    let b = e.test_element_box(id).unwrap();
    let (x, y) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
    let m = Modifiers::default();
    let _ = e.input(InputEvent::PointerDown {
        x,
        y,
        button: PointerButton::Primary,
        mods: m,
    });
    let _ = e.input(InputEvent::PointerUp {
        x,
        y,
        button: PointerButton::Primary,
        mods: m,
    });
}

fn boot() -> Engine {
    let mut e = Engine::new(cfg());
    e.declare_actions(&["settings.save"]);
    e.load_html(&html()).unwrap();
    let _ = e.paint(vp());
    sync(
        &mut e,
        pkt(1, 1, &dep(false), "<span id=\"o\">other</span>"),
    );
    assert!(
        e.attr("dep", "disabled").is_some(),
        "rule greys dep while gate is off"
    );
    e
}

/// Blocker 4, ancestor shape: the swap target is the section around `#dep`,
/// not `#dep` itself (Mainnet's usual innerHTML section swap).
#[test]
fn r3_host_disabled_survives_rule_clear_after_ancestor_swap() {
    let mut e = boot();
    sync(&mut e, pkt(2, 1, &dep(true), "<span id=\"o\">other</span>"));
    click(&mut e, "gate");
    let _ = e.paint(vp());
    // The checkbox state lives in element state, not the `checked` attribute.
    assert!(
        e.attr("dep", "disabled").is_some(),
        "host-authored disabled must survive the rule clearing"
    );
    assert_eq!(e.attr("dep", "data-hx-reason"), None);
}

/// A swap of an unrelated section must not turn the engine's grey into a
/// "host-authored" one that the rule can then never clear.
#[test]
fn r3_unrelated_swap_keeps_rule_grey_engine_owned() {
    let mut e = boot();
    sync(
        &mut e,
        pkt(1, 2, &dep(false), "<span id=\"o\">changed</span>"),
    );
    assert!(e.attr("dep", "disabled").is_some());
    assert_eq!(e.attr("dep", "data-hx-reason").as_deref(), Some("gate off"));
    click(&mut e, "gate");
    let _ = e.paint(vp());
    assert!(
        e.attr("dep", "disabled").is_none(),
        "rule clear must ungrey an engine-owned grey"
    );
}

/// Upper bound on commit damage: toggling a checkbox that feeds no rule must
/// damage only that checkbox, never the page.
#[test]
fn r3_checkbox_commit_damage_stays_within_control() {
    let mut e = boot();
    let b = e.test_element_box("solo").unwrap();
    for _ in 0..2 {
        click(&mut e, "solo");
        let out = e.paint(vp()).unwrap();
        assert!(!out.damage.is_empty());
        for r in &out.damage {
            assert!(
                r.x >= b.0 - 4.0
                    && r.y >= b.1 - 4.0
                    && r.x + r.width <= b.0 + b.2 + 4.0
                    && r.y + r.height <= b.1 + b.3 + 4.0,
                "commit damage {r:?} escapes the control box {b:?}"
            );
        }
    }
}
