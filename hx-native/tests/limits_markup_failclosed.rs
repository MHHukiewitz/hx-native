//! PR #27 round 3 (Graph re-review at 60e0753): host markup with bad numeric
//! bounds must never panic, a failed limits sync after a section swap must
//! fail closed, and a limits clamp must damage only the clamped field.

use hx_native::*;

fn cfg() -> EngineConfig {
    EngineConfig {
        max_paint_ns: 50_000_000,
        pending_timeout_frames: 5,
        paint_mode: PaintMode::Exact,
        ..EngineConfig::default()
    }
}

fn vp() -> Viewport {
    Viewport {
        width: 400,
        height: 200,
        scale: 1.0,
        holes: vec![],
    }
}

fn boot(body: &str) -> Engine {
    let mut e = Engine::new(cfg());
    e.declare_actions(&["settings.apply"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0;background:#fff}}</style></head><body>{body}</body></html>"
    ))
    .unwrap();
    let _ = e.paint(vp());
    e
}

fn key(e: &mut Engine, k: &str, ctrl: bool) {
    let _ = e.input(InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers {
            ctrl,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    }));
}

fn retype(e: &mut Engine, id: &str, t: &str) {
    e.test_set_focus(id);
    key(e, "a", true);
    key(e, "Backspace", false);
    let _ = e.input(InputEvent::Text(t.into()));
    key(e, "Enter", false);
}

fn packet(actions_ver: u64, panel_ver: u64, limits: Vec<(&str, &str, InputLimits)>) -> SyncPacket {
    let mut p = SyncPacket::default();
    p.versions.insert("actions".into(), actions_ver);
    p.versions.insert("panel".into(), panel_ver);
    p.input_limits = limits
        .into_iter()
        .map(|(a, i, l)| (a.into(), i.into(), l))
        .collect();
    p
}

/// Blocker 1: `f64::clamp` panicked on markup min>max, NaN or inf, on
/// ArrowUp (new in #27) and on a typed commit (already on main). Non-finite
/// markup bounds are dropped; max < min is treated as max = min (HTML).
#[test]
fn markup_bad_bounds_never_panic() {
    // (input type, markup attrs, event, expected value)
    let cases = [
        ("number", r#"min="10" max="0""#, "up", "10"),
        ("number", r#"min="10" max="0""#, "type", "10"),
        ("number", r#"min="NaN" max="10""#, "up", "6"),
        ("number", r#"min="0" max="NaN""#, "type", "3"),
        ("number", r#"min="inf" max="10""#, "up", "6"),
        ("number", r#"min="0" max="10" step="inf""#, "up", "6"),
        ("range", r#"min="10" max="0""#, "up", "10"),
    ];
    for (ty, attrs, ev, want) in cases {
        let mut e = boot(&format!(
            "<form id=\"f\"><input id=\"m\" type=\"{ty}\" name=\"m\" {attrs} value=\"5\" \
             style=\"position:absolute;left:10px;top:10px;width:120px;height:22px\"/>\
             <button id=\"save\" hx-post=\"settings.apply\" hx-trigger=\"click\">Save</button></form>"
        ));
        if ev == "up" {
            e.test_set_focus("m");
            key(&mut e, "ArrowUp", false);
        } else {
            retype(&mut e, "m", "3");
        }
        assert_eq!(
            e.attr("m", "value").as_deref(),
            Some(want),
            "{ty} {attrs} {ev}"
        );
    }
}

/// Typing -0 into a step "any" field must write "0", not "-0".
#[test]
fn typed_negative_zero_writes_zero() {
    let mut e = boot(
        "<form id=\"f\"><input id=\"fee\" type=\"number\" name=\"fee\" value=\"2\"/>\
         <button id=\"save\" hx-post=\"settings.apply\" hx-trigger=\"click\">Save</button></form>",
    );
    let p = packet(
        1,
        1,
        vec![("settings.apply", "fee", InputLimits::new(-10.0, 10.0, None))],
    );
    e.sync(&p, &mut p.clone()).unwrap();
    retype(&mut e, "fee", "-0");
    assert_eq!(e.attr("fee", "value").as_deref(), Some("0"));
}

/// Blocker 2: a sync that swaps a section and carries a bad limit must fail
/// closed. The swapped-in Disabled button stays greyed with its reason and
/// the swapped-in field keeps the last good limits.
#[test]
fn failed_limits_sync_after_section_swap_fails_closed() {
    let frag = "<input id=\"fee\" type=\"number\" name=\"fee\" value=\"5\"/>\
                <button id=\"save\" hx-post=\"settings.apply\" hx-trigger=\"click\">Save</button>";
    let mut e = boot(&format!(
        "<form id=\"f\"><section id=\"sec\" hx-trigger=\"slice:panel\">{frag}</section></form>"
    ));
    let mk = |av: u64, pv: u64, l: Vec<(&str, &str, InputLimits)>| {
        let mut p = packet(av, pv, l);
        p.actions = vec![(
            "settings.apply".into(),
            ActionState::Disabled {
                reason: "busy".into(),
            },
        )];
        p.fragments.insert("sec".into(), frag.into());
        p
    };
    let good = mk(
        1,
        1,
        vec![(
            "settings.apply",
            "fee",
            InputLimits::new(0.0, 10.0, Some(1.0)),
        )],
    );
    e.sync(&good, &mut good.clone()).unwrap();
    assert!(e.attr("save", "disabled").is_some());
    assert_eq!(e.attr("fee", "max").as_deref(), Some("10"));

    let bad_cases = [
        mk(
            2,
            2,
            vec![(
                "settings.apply",
                "nosuch",
                InputLimits::new(0.0, 10.0, Some(1.0)),
            )],
        ),
        mk(
            3,
            3,
            vec![(
                "settings.apply",
                "fee",
                InputLimits::new(0.0, 10.0, Some(f64::NAN)),
            )],
        ),
    ];
    for bad in bad_cases {
        e.sync(&bad, &mut bad.clone())
            .expect_err("bad limit must fail sync");
        assert!(
            e.attr("save", "disabled").is_some(),
            "swapped-in Disabled button must stay greyed after a failed sync"
        );
        assert_eq!(e.attr("save", "data-hx-reason").as_deref(), Some("busy"));
        assert_eq!(
            e.attr("fee", "max").as_deref(),
            Some("10"),
            "swapped-in field must keep the last good limits"
        );
    }
}

/// Upper bound on clamp damage: a limits clamp damages only the clamped
/// field, never the page (Graph's C1 mutation, missed by the suite).
#[test]
fn limits_clamp_damage_stays_within_field() {
    let mut e = boot(
        "<form id=\"f\"><input id=\"fee\" type=\"number\" name=\"fee\" value=\"7\" \
         style=\"position:absolute;left:10px;top:10px;width:120px;height:22px\"/>\
         <input id=\"far\" type=\"text\" name=\"far\" value=\"x\" \
         style=\"position:absolute;left:250px;top:150px;width:120px;height:22px\"/>\
         <button id=\"save\" hx-post=\"settings.apply\" hx-trigger=\"click\" \
         style=\"position:absolute;left:10px;top:150px\">Save</button></form>",
    );
    let p = packet(
        1,
        1,
        vec![(
            "settings.apply",
            "fee",
            InputLimits::new(0.0, 1.0, Some(1.0)),
        )],
    );
    e.sync(&p, &mut p.clone()).unwrap();
    let out = e.paint(vp()).unwrap();
    assert_eq!(e.attr("fee", "value").as_deref(), Some("1"));
    assert!(!out.damage.is_empty());
    let b = e.test_element_box("fee").unwrap();
    for r in &out.damage {
        assert!(
            r.x >= b.0 - 4.5
                && r.y >= b.1 - 4.5
                && r.x + r.width <= b.0 + b.2 + 4.5
                && r.y + r.height <= b.1 + b.3 + 4.5,
            "clamp damage {r:?} escapes the field box {b:?}"
        );
    }
}

/// Crate-owner decision (PR #27 merge with #24): the fail-closed error branch
/// reads this packet's `input_rules` before it greys, so a failing sync that
/// also changes the rules greys by the new rules, never the previous ones.
#[test]
fn failed_limits_sync_greys_by_the_new_input_rules() {
    let frag = "<input id=\"gate\" type=\"checkbox\" name=\"gate\"/>\
                <input id=\"dep\" type=\"text\" name=\"dep\" value=\"x\"/>\
                <input id=\"fee\" type=\"number\" name=\"fee\" value=\"5\"/>\
                <button id=\"save\" hx-post=\"settings.apply\" hx-trigger=\"click\">Save</button>";
    let mut e = boot(&format!(
        "<form id=\"f\"><section id=\"sec\" hx-trigger=\"slice:panel\">{frag}</section></form>"
    ));
    let gate_rule = || {
        vec![(
            "settings.apply".to_string(),
            "dep".to_string(),
            InputRule::EnabledWhen {
                input: "gate".into(),
                checked: true,
                reason: "gate off".into(),
            },
        )]
    };
    let mk = |av: u64, pv: u64, l: Vec<(&str, &str, InputLimits)>, rules| {
        let mut p = packet(av, pv, l);
        p.input_rules = rules;
        p.fragments.insert("sec".into(), frag.into());
        p
    };
    let fee = || {
        vec![(
            "settings.apply",
            "fee",
            InputLimits::new(0.0, 10.0, Some(1.0)),
        )]
    };
    let nosuch = || {
        vec![(
            "settings.apply",
            "nosuch",
            InputLimits::new(0.0, 10.0, Some(1.0)),
        )]
    };

    // Good sync, no rules: dep is enabled.
    let good = mk(1, 1, fee(), Vec::new());
    e.sync(&good, &mut good.clone()).unwrap();
    assert!(e.attr("dep", "disabled").is_none());

    // Failing sync (unknown limit input) that swaps the section and adds the
    // gate rule: dep must be greyed by the new rule.
    let bad = mk(2, 2, nosuch(), gate_rule());
    e.sync(&bad, &mut bad.clone())
        .expect_err("unknown limit input must fail sync");
    assert!(
        e.attr("dep", "disabled").is_some(),
        "failed sync must grey by this packet's new input_rules"
    );
    assert_eq!(e.attr("dep", "data-hx-reason").as_deref(), Some("gate off"));
    assert_eq!(e.attr("fee", "max").as_deref(), Some("10"));

    // Failing sync that drops the rule: the engine-owned grey must clear.
    let bad2 = mk(3, 3, nosuch(), Vec::new());
    e.sync(&bad2, &mut bad2.clone())
        .expect_err("unknown limit input must fail sync");
    assert!(
        e.attr("dep", "disabled").is_none(),
        "failed sync must stop greying by the previous packet's rules"
    );
    assert_eq!(e.attr("dep", "data-hx-reason"), None);
}
