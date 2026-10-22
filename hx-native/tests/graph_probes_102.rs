//! Graph's review probes for hx-native PR #102 (fixes #101):
//! `InputLimits::max: Option<f64>` and `InputLimits::at_least`, with
//! Designer's ruling (2026-10-22 12:45 America/Asuncion) that `at_least`
//! on a range is refused: nothing is applied and the range keeps (or gets
//! back) its markup `min`/`max`/`step`.

use std::collections::HashMap;
use std::time::Duration;

use hx_native::*;

fn vp() -> Viewport {
    Viewport::with_scale(400, 200, 1.0)
}

const STYLE: &str = "<style>html,body{margin:0;font:12px \"DejaVu Sans\"}\
#budget{position:absolute;left:0;top:0;width:200px;height:24px}\
#up{position:absolute;left:210px;top:0;width:24px;height:12px}\
#dn{position:absolute;left:210px;top:12px;width:24px;height:12px}\
#vol{position:absolute;left:0;top:40px;width:160px;height:20px}\
#apply{position:absolute;left:0;top:80px;width:60px;height:20px}\
#blur{position:absolute;left:0;top:120px;width:60px;height:20px}</style>";

fn budget_html(value: &str, max: Option<&str>) -> String {
    let max = max.map(|m| format!(" max=\"{m}\"")).unwrap_or_default();
    format!(
        "<input id=\"budget\" type=\"number\" name=\"budget\" value=\"{value}\" min=\"0\" step=\"1\"{max} \
         hx-post=\"services.budget\" hx-trigger=\"change\" />\
         <i id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"budget\">U</i>\
         <i id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"budget\">D</i>"
    )
}

fn range_html(value: &str, max: Option<&str>) -> String {
    let max = max.map(|m| format!(" max=\"{m}\"")).unwrap_or_default();
    format!("<input id=\"vol\" type=\"range\" name=\"vol\" value=\"{value}\" min=\"0\"{max} />")
}

struct Slices {
    versions: HashMap<String, u64>,
    limits: Vec<(String, String, InputLimits)>,
}
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        Vec::new()
    }
    fn input_limits(&self) -> Vec<(String, String, InputLimits)> {
        self.limits.clone()
    }
}

struct Frags(HashMap<String, String>);
impl FragmentSource for Frags {
    fn render(&mut self, id: &str, _slices: &[&str]) -> String {
        self.0.get(id).cloned().unwrap_or_default()
    }
}

struct H {
    e: Engine,
    s: Slices,
    f: Frags,
}

impl H {
    /// A form with section `sec` (the game redraws it) holding `inner`.
    fn new(inner: String, limits: Vec<(&str, InputLimits)>) -> H {
        let mut e = Engine::new(EngineConfig {
            pending_timeout_frames: 1000,
            paint_mode: PaintMode::Exact,
            ..EngineConfig::default()
        });
        e.declare_actions(&["services.apply", "services.budget"]);
        e.load_html(&format!(
            "<!DOCTYPE html><html><head>{STYLE}</head><body><form id=\"f\">\
             <div id=\"sec\" hx-trigger=\"slice:sec\" hx-target=\"#sec\" hx-swap=\"innerHTML\"></div>\
             <button id=\"apply\" type=\"button\" hx-post=\"services.apply\">Apply</button></form>\
             <input id=\"blur\" type=\"text\" value=\"x\" /></body></html>"
        ))
        .unwrap();
        let mut h = H {
            e,
            s: Slices {
                versions: [("actions".to_string(), 1), ("sec".to_string(), 1)].into(),
                limits: Vec::new(),
            },
            f: Frags([("sec".to_string(), inner)].into()),
        };
        h.set_limits(limits);
        h
    }

    fn sync(&mut self) {
        self.e.sync(&self.s, &mut self.f).unwrap();
        let _ = self.e.paint(vp());
    }

    fn set_limits(&mut self, limits: Vec<(&str, InputLimits)>) {
        self.s.limits = limits
            .into_iter()
            .map(|(i, l)| ("services.apply".to_string(), i.to_string(), l))
            .collect();
        *self.s.versions.get_mut("actions").unwrap() += 1;
        self.sync();
    }

    fn redraw(&mut self, inner: String) {
        self.f.0.insert("sec".into(), inner);
        *self.s.versions.get_mut("sec").unwrap() += 1;
        self.sync();
    }

    fn feed(&mut self, ev: InputEvent) -> Vec<InputOutcome> {
        let mut o = vec![self.e.input(ev)];
        o.extend(self.e.take_outcomes());
        let _ = self.e.paint(vp());
        o
    }

    fn key(&mut self, k: &str, mods: Modifiers) -> Vec<InputOutcome> {
        let mut o = Vec::new();
        for pressed in [true, false] {
            o.extend(self.feed(InputEvent::Key(KeyEvent {
                key: k.into(),
                code: k.into(),
                mods,
                repeat: false,
                pressed,
            })));
        }
        o
    }

    fn tap(&mut self, k: &str) -> Vec<InputOutcome> {
        self.key(k, Modifiers::default())
    }

    fn mid(&self, id: &str) -> (f32, f32) {
        let (x, y, w, h) = self.e.test_element_box(id).unwrap();
        (x + w / 2.0, y + h / 2.0)
    }

    fn down(&mut self, id: &str) -> Vec<InputOutcome> {
        let (x, y) = self.mid(id);
        self.feed(InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
    }

    fn up(&mut self, id: &str) -> Vec<InputOutcome> {
        let (x, y) = self.mid(id);
        self.feed(InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
            mods: Modifiers::default(),
        })
    }

    fn click(&mut self, id: &str) -> Vec<InputOutcome> {
        let mut o = self.down(id);
        o.extend(self.up(id));
        o
    }

    /// Replace the box's text with `s` and press Enter.
    fn type_enter(&mut self, s: &str) -> Vec<InputOutcome> {
        self.e.test_set_focus("budget");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let mut o = self.key("a", ctrl);
        for c in s.chars() {
            o.extend(self.feed(InputEvent::Text(c.to_string())));
        }
        o.extend(self.tap("Enter"));
        o
    }

    fn attr(&self, id: &str, a: &str) -> Option<String> {
        self.e.attr(id, a)
    }

    /// Complete every request in `outs` (the game takes the value).
    fn finish(&mut self, outs: &[InputOutcome]) {
        for o in outs {
            if let InputOutcome::Action { request, .. } = o {
                self.e.complete(*request, Ok(()));
            }
        }
        let _ = self.e.take_outcomes();
    }
}

fn sent(outs: &[InputOutcome]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { name, args, .. } if name == "services.budget" => args
                .iter()
                .find(|(k, _)| k == "budget")
                .map(|(_, v)| v.clone()),
            _ => None,
        })
        .collect()
}

const CEIL: &str = "9007199254740992";

fn budget(value: &str, markup_max: Option<&str>) -> H {
    H::new(
        budget_html(value, markup_max),
        vec![("budget", InputLimits::at_least(0.0, Some(1.0)))],
    )
}

/// Derives on the changed type: Clone, Debug, PartialEq as before (the
/// struct never derived Copy or Default, and still does not).
#[test]
fn input_limits_option_max_keeps_its_traits() {
    let a = InputLimits::at_least(5.0, None);
    let b = a.clone();
    assert_eq!(a, b);
    assert_ne!(a, InputLimits::new(5.0, 10.0, None));
    assert_eq!(InputLimits::new(5.0, 10.0, None).max, Some(10.0));
    assert_eq!(a.max, None);
    assert!(format!("{a:?}").contains("max: None"));
}

/// ArrowUp near the ±2^53 ceiling with no max: the value climbs to 2^53
/// and stops there; each real step sends one change and a step that does
/// not move sends nothing. Never formatted in exponent form.
#[test]
fn arrow_up_at_the_safe_ceiling_stops_and_sends_nothing_more() {
    let mut h = budget("9007199254740990", None);
    h.e.test_set_focus("budget");
    let mut all = Vec::new();
    for _ in 0..5 {
        let o = h.tap("ArrowUp");
        h.finish(&o);
        all.extend(sent(&o));
    }
    all.extend(sent(&h.click("blur")));
    assert_eq!(h.attr("budget", "value").as_deref(), Some(CEIL));
    assert_eq!(
        all,
        ["9007199254740991", CEIL],
        "one change per real step, nothing at the ceiling"
    );
    // ▲ clicks at the ceiling send nothing either.
    let o = h.click("up");
    assert!(sent(&o).is_empty(), "{o:?}");
}

/// A fractional step at a magnitude where it cannot move the value: no
/// change, no junk value.
#[test]
fn fractional_step_near_the_ceiling_is_a_no_op() {
    let mut h = H::new(
        budget_html("9007199254740000", None),
        vec![("budget", InputLimits::at_least(0.0, Some(0.01)))],
    );
    h.e.test_set_focus("budget");
    let mut all = Vec::new();
    for _ in 0..3 {
        let o = h.tap("ArrowUp");
        h.finish(&o);
        all.extend(sent(&o));
    }
    all.extend(sent(&h.click("blur")));
    let v: f64 = h.attr("budget", "value").unwrap().parse().unwrap();
    assert!(v.is_finite() && v <= 9007199254740992.0, "{v}");
    assert!(all.len() <= 1, "{all:?}");
}

/// Holding ▲ with no max: the repeat runs, nothing clamps it below the
/// ceiling, and the release sends one change.
#[test]
fn holding_the_arrow_with_no_max_sends_one_change() {
    let mut h = budget("1000", None);
    h.down("up");
    h.e.test_hold_spin_for(Duration::from_secs(5));
    let o = h.up("up");
    let mut all = sent(&o);
    all.extend(sent(&h.e.take_outcomes()));
    let v: f64 = h.attr("budget", "value").unwrap().parse().unwrap();
    assert!(v > 1001.0, "the hold repeated: {v}");
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].parse::<f64>().unwrap(), v);
    // Near the ceiling a long hold stops there.
    let mut h = budget("9007199254740900", None);
    h.down("up");
    h.e.test_hold_spin_for(Duration::from_secs(60));
    let o = h.up("up");
    assert_eq!(h.attr("budget", "value").as_deref(), Some(CEIL));
    assert_eq!(sent(&o), [CEIL]);
}

/// A typed value above the ceiling clamps to it, not to a max.
#[test]
fn typed_value_above_the_ceiling_clamps_to_it() {
    let mut h = budget("10", None);
    let o = h.type_enter("99999999999999999999");
    assert_eq!(h.attr("budget", "value").as_deref(), Some(CEIL));
    assert_eq!(sent(&o), [CEIL]);
}

/// #87 (compare by number) and #91 (empty puts back the last value sent)
/// with no max and large values.
#[test]
fn number_compare_and_restore_with_no_max() {
    let mut h = budget("10", None);
    let o = h.type_enter("123456789012");
    assert_eq!(sent(&o), ["123456789012"]);
    h.finish(&o);
    let o = h.type_enter("123456789012.00");
    assert!(sent(&o).is_empty(), "same number: {o:?}");
    // Empty box, blur: the last value sent comes back, nothing sent.
    h.e.test_set_focus("budget");
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    h.key("a", ctrl);
    h.tap("Backspace");
    let o = h.click("blur");
    assert!(sent(&o).is_empty(), "{o:?}");
    assert_eq!(h.attr("budget", "value").as_deref(), Some("123456789012"));
    // #92 / #84: ▲ on an emptied box steps from the last value sent.
    h.e.test_set_focus("budget");
    h.key("a", ctrl);
    h.tap("Backspace");
    let o = h.tap("ArrowUp");
    let mut all = sent(&o);
    all.extend(sent(&h.click("blur")));
    assert_eq!(all, ["123456789013"]);
}

/// A later redraw that brings a markup `max` on the number box: the slice
/// (no max) wins again, and a second sync writes nothing (no fight, the
/// next paint is skipped).
#[test]
fn number_box_markup_max_after_a_swap_is_cleared_once() {
    let mut h = budget("10", Some("50"));
    assert_eq!(h.attr("budget", "max"), None);
    h.redraw(budget_html("70", Some("500")));
    assert_eq!(h.attr("budget", "max"), None, "the slice wins after a swap");
    assert_eq!(h.attr("budget", "value").as_deref(), Some("70"));
    *h.s.versions.get_mut("actions").unwrap() += 1;
    h.e.sync(&h.s, &mut h.f).unwrap();
    let _ = h.e.paint(vp());
    *h.s.versions.get_mut("actions").unwrap() += 1;
    h.e.sync(&h.s, &mut h.f).unwrap();
    let out = h.e.paint(vp()).unwrap();
    assert!(out.skipped, "stable: nothing rewritten");
    // Switching to `new` writes its max, back to `at_least` clears it.
    h.set_limits(vec![("budget", InputLimits::new(0.0, 800.0, Some(1.0)))]);
    assert_eq!(h.attr("budget", "max").as_deref(), Some("800"));
    h.set_limits(vec![("budget", InputLimits::at_least(0.0, Some(1.0)))]);
    assert_eq!(h.attr("budget", "max"), None);
}

fn range_attrs(h: &H) -> [Option<String>; 4] {
    ["min", "max", "step", "value"].map(|a| h.attr("vol", a))
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

fn range_markup(value: &str, min: &str, max: &str, step: &str) -> String {
    format!(
        "<input id=\"vol\" type=\"range\" name=\"vol\" value=\"{value}\" min=\"{min}\" max=\"{max}\" step=\"{step}\" />"
    )
}

/// The ruling: `at_least` on a range is refused. Nothing is written, not
/// on the first apply and not on the re-apply after a swap; a redraw with
/// new markup keeps the new markup and its value (no clamp to the old max).
#[test]
fn range_at_least_is_refused_and_follows_its_markup_across_swaps() {
    let mut h = H::new(
        range_markup("40", "10", "50", "5"),
        vec![("vol", InputLimits::at_least(0.0, Some(1.0)))],
    );
    assert_eq!(range_attrs(&h), [s("10"), s("50"), s("5"), s("40")]);
    h.redraw(range_markup("70", "20", "80", "10"));
    assert_eq!(range_attrs(&h), [s("20"), s("80"), s("10"), s("70")]);
    // No markup limits at all: still nothing written (no 100 fallback).
    let h = H::new(
        range_html("40", None),
        vec![("vol", InputLimits::at_least(150.0, Some(1.0)))],
    );
    // `range_html` carries markup `min="0"` and no max/step.
    assert_eq!(range_attrs(&h), [s("0"), None, None, s("40")]);
}

/// `new` -> swap -> `at_least`: the range gets back the markup of its
/// latest redraw, not the first markup and not the `new` limits.
#[test]
fn range_new_then_swap_then_at_least_restores_the_latest_markup() {
    let mut h = H::new(
        range_markup("30", "10", "50", "5"),
        vec![("vol", InputLimits::new(0.0, 40.0, Some(1.0)))],
    );
    assert_eq!(range_attrs(&h), [s("0"), s("40"), s("1"), s("30")]);
    h.redraw(range_markup("35", "20", "80", "10"));
    assert_eq!(
        range_attrs(&h)[..3],
        [s("0"), s("40"), s("1")],
        "new re-applied"
    );
    h.set_limits(vec![("vol", InputLimits::at_least(0.0, Some(1.0)))]);
    assert_eq!(range_attrs(&h), [s("20"), s("80"), s("10"), s("35")]);
    // And back to `new`: written again, from the same markup snapshot.
    h.set_limits(vec![("vol", InputLimits::new(0.0, 60.0, Some(2.0)))]);
    assert_eq!(range_attrs(&h)[..3], [s("0"), s("60"), s("2")]);
    h.set_limits(vec![]);
    assert_eq!(
        range_attrs(&h)[..3],
        [s("20"), s("80"), s("10")],
        "removed: markup"
    );
}

/// The slice stops sending a refused `at_least` for a range: the range
/// must keep its markup. The engine never wrote over it, so there is
/// nothing to restore and nothing to clear.
#[test]
fn removing_a_refused_range_entry_keeps_the_markup() {
    let mut h = H::new(
        range_markup("40", "10", "50", "5"),
        vec![("vol", InputLimits::at_least(0.0, Some(1.0)))],
    );
    h.set_limits(vec![]);
    assert_eq!(range_attrs(&h), [s("10"), s("50"), s("5"), s("40")]);
}

/// One sync both redraws the input (new markup) and drops its limits from
/// the slice. The swap drops the snapshot (it no longer describes the DOM),
/// so removal must leave the fresh markup alone: no stale restore of the
/// first markup, and no clearing of the new one. Number box and range.
#[test]
fn swap_and_drop_in_one_sync_keeps_the_new_markup() {
    let mut h = H::new(
        range_markup("40", "10", "50", "5"),
        vec![("vol", InputLimits::new(0.0, 40.0, Some(1.0)))],
    );
    assert_eq!(range_attrs(&h)[..3], [s("0"), s("40"), s("1")]);
    h.f.0
        .insert("sec".into(), range_markup("70", "20", "80", "10"));
    *h.s.versions.get_mut("sec").unwrap() += 1;
    h.set_limits(vec![]);
    assert_eq!(range_attrs(&h), [s("20"), s("80"), s("10"), s("70")]);

    let mut h = H::new(
        budget_html("10", Some("50")),
        vec![("budget", InputLimits::at_least(5.0, Some(1.0)))],
    );
    assert_eq!(h.attr("budget", "max"), None);
    h.f.0.insert("sec".into(), budget_html("10", Some("90")));
    *h.s.versions.get_mut("sec").unwrap() += 1;
    h.set_limits(vec![]);
    assert_eq!(h.attr("budget", "max").as_deref(), Some("90"));
}

/// Non-range inputs are unaffected: in one slice, a refused range entry
/// next to a number box's `at_least` (either order) still gives the
/// number box min/step and no max.
#[test]
fn a_refused_range_does_not_touch_a_number_box_in_the_same_slice() {
    for range_first in [true, false] {
        let inner = format!(
            "{}{}",
            budget_html("10", Some("50")),
            range_markup("40", "10", "50", "5")
        );
        let mut entries = vec![
            ("vol", InputLimits::at_least(0.0, Some(1.0))),
            ("budget", InputLimits::at_least(5.0, Some(1.0))),
        ];
        if !range_first {
            entries.reverse();
        }
        let h = H::new(inner, entries);
        assert_eq!(h.attr("budget", "min").as_deref(), Some("5"));
        assert_eq!(h.attr("budget", "max"), None);
        assert_eq!(range_attrs(&h), [s("10"), s("50"), s("5"), s("40")]);
    }
}

/// Log cadence (counted from the test's stderr by the review script): one
/// refusal line per `input_limits()` refresh (an `actions` bump), none for
/// the 30 swaps in between.
#[test]
fn refusal_log_cadence() {
    let mut h = H::new(
        range_markup("40", "10", "50", "5"),
        vec![("vol", InputLimits::at_least(0.0, Some(1.0)))],
    );
    eprintln!("GRAPH-CADENCE-START");
    for i in 0..30 {
        h.redraw(range_markup(&format!("{}", 20 + i), "10", "50", "5"));
    }
    for _ in 0..3 {
        *h.s.versions.get_mut("actions").unwrap() += 1;
        h.sync();
    }
    eprintln!("GRAPH-CADENCE-END");
}

/// min > max with max None cannot happen for a number box; validation
/// still rejects min > max for `new`, and non-finite or out-of-range min
/// for `at_least`.
#[test]
fn validation_with_no_max() {
    let mut h = budget("10", None);
    for bad in [
        InputLimits::new(10.0, 5.0, None),
        InputLimits::at_least(f64::NAN, None),
        InputLimits::at_least(1e300, None),
        InputLimits::at_least(0.0, Some(0.0)),
    ] {
        h.s.limits = vec![("services.apply".into(), "budget".into(), bad.clone())];
        *h.s.versions.get_mut("actions").unwrap() += 1;
        assert!(h.e.sync(&h.s, &mut h.f).is_err(), "{bad:?}");
    }
    h.s.limits = vec![(
        "services.apply".into(),
        "budget".into(),
        InputLimits::at_least(-9007199254740992.0, None),
    )];
    *h.s.versions.get_mut("actions").unwrap() += 1;
    assert!(h.e.sync(&h.s, &mut h.f).is_ok());
}
