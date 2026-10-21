//! Graph re-review of #27 at 5d0fef6: `merge_per_slice` must take a newer
//! packet's non-empty `actions` / `input_rules` even without the `"actions"`
//! version key, because `Engine::sync` re-reads both on every call. Pins the
//! wider replace condition (#24) that no other test covers.

use std::collections::HashMap;

use hx_native::*;

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

const HTML: &str = r#"<!DOCTYPE html><html><body>
<form id="f">
  <input id="gate" type="checkbox" name="gate" />
  <input id="dep" type="text" name="dep" value="x" />
  <button id="save" hx-post="settings.apply" hx-trigger="click">Save</button>
</form>
<button id="ob" hx-post="other.act" hx-trigger="click">Other</button>
</body></html>"#;

fn engine() -> Engine {
    let mut e = Engine::new(cfg());
    e.declare_actions(&["settings.apply", "other.act"]);
    e.load_html(HTML).unwrap();
    e
}

fn run(pkts: &[SyncPacket]) -> Engine {
    let mut e = engine();
    for p in pkts {
        let mut f = p.clone();
        e.sync(p, &mut f).unwrap();
    }
    e
}

fn state(e: &Engine) -> [Option<String>; 4] {
    [
        e.attr("ob", "disabled"),
        e.attr("ob", "data-hx-reason"),
        e.attr("dep", "disabled"),
        e.attr("dep", "data-hx-reason"),
    ]
}

fn p1() -> SyncPacket {
    let mut p = SyncPacket::default();
    p.versions = HashMap::from([("actions".into(), 1)]);
    p.actions = vec![(
        "other.act".into(),
        ActionState::Disabled {
            reason: "busy".into(),
        },
    )];
    p
}

fn gate_rule() -> Vec<(String, String, InputRule)> {
    vec![(
        "settings.apply".into(),
        "dep".into(),
        InputRule::EnabledWhen {
            input: "gate".into(),
            checked: true,
            reason: "gate off".into(),
        },
    )]
}

fn check(p2: SyncPacket, what: &str) {
    let seq = run(&[p1(), p2.clone()]);
    let mut merged = p1();
    merged.merge_per_slice(p2);
    let coalesced = run(&[merged]);
    assert_eq!(
        state(&seq),
        state(&coalesced),
        "{what}: coalesced != sequential"
    );
}

#[test]
fn merge_takes_actions_without_actions_key() {
    let mut p2 = SyncPacket::default();
    p2.versions = HashMap::from([("ticker".into(), 1)]);
    p2.actions = vec![(
        "other.act".into(),
        ActionState::Disabled {
            reason: "later".into(),
        },
    )];
    check(p2, "actions-only packet without the actions key");
}

#[test]
fn merge_takes_rules_without_actions_key() {
    let mut p2 = SyncPacket::default();
    p2.versions = HashMap::from([("ticker".into(), 1)]);
    p2.input_rules = gate_rule();
    check(p2, "rules-only packet without the actions key");
}
