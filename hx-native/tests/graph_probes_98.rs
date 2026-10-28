//! Graph's review probes for hx-native PR #98 (fixes #96, Gap B of
//! Designer's review of #94), against Designer's `hx-sync` rulings in
//! Mainnet HTML_CONTRACT at 5983ff17 and the frame-order rule at 5204f3a2.
//!
//! The Gap B pattern: one value is sent, a newer one is held, and then the
//! game greys the control. The control must show the game's value: the
//! value a redraw drew while the change was held, or else the last value
//! sent. Probed for every greying source (markup, `actions` slice, input
//! rule) and every value control kind (number, text, select, range,
//! checkbox, radio group), with the variants: a redraw first, no redraw, a
//! redraw that draws the value sent, a redraw of an unrelated section, and
//! ungreying afterwards (nothing is sent, and the control then holds the
//! game's value).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hx_native::*;

fn vp() -> Viewport {
    Viewport::with_scale(320, 260, 1.0)
}

const STYLE: &str = "<style>*{box-sizing:border-box}html,body{margin:0;background:#fff;\
font:12px \"DejaVu Sans\"}input,select,button{display:block;width:160px;height:20px;margin:2px}\
input[type=checkbox],input[type=radio]{width:16px;height:16px}\
.spin{display:block;position:relative}.spin i{position:absolute;left:200px;width:20px;height:10px;\
font-style:normal;font-size:8px}.up{top:2px}.dn{top:12px}</style>";

#[derive(Default)]
struct Game {
    html: HashMap<String, String>,
    versions: HashMap<String, u64>,
    actions: Vec<(String, ActionState)>,
    rules: Vec<(String, String, InputRule)>,
}

struct Slices(Rc<RefCell<Game>>);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        self.0.borrow().versions.get(name).copied()
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.0.borrow().actions.clone()
    }
    fn input_rules(&self) -> Vec<(String, String, InputRule)> {
        self.0.borrow().rules.clone()
    }
}

struct Frags(Rc<RefCell<Game>>);
impl FragmentSource for Frags {
    fn render(&mut self, s: &str, _slices: &[&str]) -> String {
        self.0.borrow().html.get(s).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Number,
    Text,
    Select,
    Range,
    Checkbox,
    Radio,
}
use Kind::*;
const KINDS: [Kind; 6] = [Number, Text, Select, Range, Checkbox, Radio];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    Markup,
    Actions,
    Rule,
}
use Source::*;
const SOURCES: [Source; 3] = [Markup, Actions, Rule];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Before {
    /// Gap B: a redraw of the control's section draws a third value.
    RedrawDrawsOther,
    /// No redraw while the change is held.
    NoRedraw,
    /// A redraw draws the value sent.
    RedrawDrawsSent,
    /// A redraw of a section that does not hold the control (a HUD).
    UnrelatedRedraw,
}
use Before::*;
const BEFORES: [Before; 4] = [RedrawDrawsOther, NoRedraw, RedrawDrawsSent, UnrelatedRedraw];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Number => "fee",
            Text => "note",
            Select => "pick",
            Range => "vol",
            Checkbox => "tick",
            Radio => "r",
        }
    }
    fn action(self) -> String {
        format!("deal.{}", self.name())
    }
    /// (initial, sent, held, drawn) in the probe's encoding: the value, the
    /// option or radio value, or "on"/"" for the checkbox.
    fn values(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Number => ("4.69", "4.7", "4.71", "4.65"),
            Text => ("", "a", "ab", "z"),
            Select => ("a", "b", "c", "d"),
            Range => ("5", "6", "7", "3"),
            // Ticked is sent, the untick is held, and the game refuses the
            // tick: it draws the box unticked.
            Checkbox => ("", "on", "", ""),
            // b is sent, then c (its own request), then b again is held:
            // the last value sent for the group is c.
            Radio => ("a", "c", "b", "d"),
        }
    }
    fn html(self, v: &str, disabled: bool) -> String {
        let dis = if disabled { " disabled" } else { "" };
        let post = |n: &str| format!(" hx-post=\"deal.{n}\" hx-trigger=\"change\"{dis}");
        match self {
            Number => format!(
                "<span class=\"spin\"><input id=\"fee\" type=\"number\" name=\"fee\" value=\"{v}\" \
                 min=\"0\" max=\"100.00\" step=\"0.01\"{} />\
                 <i id=\"fee-up\" class=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">U</i>\
                 <i id=\"fee-dn\" class=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">D</i></span>",
                post("fee")
            ),
            Text => format!(
                "<input id=\"note\" type=\"text\" name=\"note\" value=\"{v}\"{} />",
                post("note")
            ),
            Select => {
                let opts: String = ["a", "b", "c", "d"]
                    .iter()
                    .map(|o| {
                        let sel = if *o == v { " selected" } else { "" };
                        format!("<option id=\"pick-{o}\" value=\"{o}\"{sel}>{o}</option>")
                    })
                    .collect();
                format!("<select id=\"pick\" name=\"pick\"{}>{opts}</select>", post("pick"))
            }
            Range => format!(
                "<input id=\"vol\" type=\"range\" name=\"vol\" min=\"0\" max=\"10\" step=\"1\" value=\"{v}\"{} />",
                post("vol")
            ),
            Checkbox => format!(
                "<input id=\"tick\" type=\"checkbox\" name=\"tick\" value=\"on\"{}{} />",
                if v == "on" { " checked" } else { "" },
                post("tick")
            ),
            Radio => ["a", "b", "c", "d"]
                .iter()
                .map(|o| {
                    format!(
                        "<input id=\"r{o}\" type=\"radio\" name=\"r\" value=\"{o}\"{}{} />",
                        if *o == v { " checked" } else { "" },
                        post("r")
                    )
                })
                .collect(),
        }
    }
}

fn gate_html(open: bool) -> String {
    format!(
        "<input id=\"open\" type=\"checkbox\" name=\"open\" value=\"on\"{} />",
        if open { " checked" } else { "" }
    )
}

struct H {
    e: Engine,
    g: Rc<RefCell<Game>>,
    k: Kind,
}

impl H {
    fn new(k: Kind) -> H {
        let g = Rc::new(RefCell::new(Game::default()));
        {
            let mut gm = g.borrow_mut();
            gm.html.insert("ctl".into(), k.html(k.values().0, false));
            gm.html.insert("gate".into(), gate_html(true));
            gm.html.insert("hud".into(), "<b id=\"clock\">0</b>".into());
            for s in ["ctl", "gate", "hud", "actions"] {
                gm.versions.insert(s.into(), 1);
            }
            gm.rules = vec![(
                "deal.go".into(),
                k.name().into(),
                InputRule::EnabledWhen {
                    input: "open".into(),
                    checked: true,
                    reason: "Closed".into(),
                },
            )];
        }
        let mut e = Engine::new(EngineConfig {
            paint_mode: PaintMode::Exact,
            pending_timeout_frames: 1000,
            ..Default::default()
        });
        e.declare_actions(&[
            "deal.fee",
            "deal.note",
            "deal.pick",
            "deal.vol",
            "deal.tick",
            "deal.r",
            "deal.form",
            "deal.go",
        ]);
        let sec = |s: &str| {
            format!("<section id=\"{s}\" hx-trigger=\"slice:{s}\" hx-target=\"#{s}\" hx-swap=\"innerHTML\"></section>")
        };
        e.load_html(&format!(
            "<!DOCTYPE html><html><head>{STYLE}</head><body>{}\
             <form id=\"f\">{}{}\
             <button id=\"go\" type=\"button\" hx-post=\"deal.go\">Go</button></form>\
             <input id=\"blur\" type=\"text\" value=\"x\" /></body></html>",
            sec("hud"),
            sec("gate"),
            sec("ctl")
        ))
        .unwrap();
        let _ = e.paint(vp()).unwrap();
        let mut h = H { e, g, k };
        h.sync();
        h
    }

    fn sync(&mut self) {
        self.e
            .sync(&Slices(self.g.clone()), &mut Frags(self.g.clone()))
            .unwrap();
        let _ = self.e.paint(vp()).unwrap();
    }

    fn redraw(&mut self, s: &str, html: String) {
        {
            let mut gm = self.g.borrow_mut();
            gm.html.insert(s.into(), html);
            *gm.versions.entry(s.into()).or_insert(0) += 1;
        }
        self.sync();
    }

    fn set_actions(&mut self, a: Vec<(String, ActionState)>) {
        {
            let mut gm = self.g.borrow_mut();
            gm.actions = a;
            *gm.versions.entry("actions".into()).or_insert(0) += 1;
        }
        self.sync();
    }

    fn feed(&mut self, ev: InputEvent) -> Vec<InputOutcome> {
        let mut out = vec![self.e.input(ev)];
        out.extend(self.e.take_outcomes());
        let _ = self.e.paint(vp());
        out
    }

    fn click(&mut self, id: &str) -> Vec<InputOutcome> {
        let b = self.e.test_element_box(id).unwrap();
        let (x, y) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
        let mods = Modifiers::default();
        let button = PointerButton::Primary;
        let mut o = self.feed(InputEvent::PointerDown { x, y, button, mods });
        o.extend(self.feed(InputEvent::PointerUp { x, y, button, mods }));
        o
    }

    fn key(&mut self, k: &str, pressed: bool) -> Vec<InputOutcome> {
        self.feed(InputEvent::Key(KeyEvent {
            key: k.into(),
            code: k.into(),
            mods: Modifiers::default(),
            repeat: false,
            pressed,
        }))
    }

    fn tap(&mut self, k: &str) -> Vec<InputOutcome> {
        let mut o = self.key(k, true);
        o.extend(self.key(k, false));
        o
    }

    fn text(&mut self, t: &str) -> Vec<InputOutcome> {
        self.feed(InputEvent::Text(t.into()))
    }

    fn finish(&mut self, outs: &[InputOutcome]) -> Vec<InputOutcome> {
        for r in requests(outs) {
            self.e.complete(r, Ok(()));
        }
        let o = self.e.take_outcomes();
        let _ = self.e.paint(vp());
        o
    }

    /// The player sends the first value and holds the second. Returns every
    /// outcome of the requests that went out (the held one did not).
    fn send_then_hold(&mut self) -> Vec<InputOutcome> {
        let (_, sent, held, _) = self.k.values();
        let n = self.k.name();
        let mut out = Vec::new();
        match self.k {
            Number => {
                out.extend(self.click("fee-up"));
                assert!(changes(&self.click("fee-up"), n).is_empty(), "held");
            }
            Text => {
                self.e.test_set_focus("note");
                self.text("a");
                out.extend(self.tap("Enter"));
                self.text("b");
                assert!(changes(&self.tap("Enter"), n).is_empty(), "held");
            }
            Select => {
                self.e.test_set_focus("pick");
                out.extend(self.tap("ArrowDown"));
                assert!(changes(&self.tap("ArrowDown"), n).is_empty(), "held");
            }
            Range => {
                self.e.test_set_focus("vol");
                out.extend(self.tap("ArrowRight"));
                assert!(changes(&self.tap("ArrowRight"), n).is_empty(), "held");
            }
            Checkbox => {
                out.extend(self.click("tick"));
                assert!(actions(&self.click("tick")).is_empty(), "held");
            }
            Radio => {
                out.extend(self.click("rb"));
                out.extend(self.click("rc"));
                assert!(actions(&self.click("rb")).is_empty(), "held");
            }
        }
        if self.k == Checkbox {
            assert_eq!(actions(&out).len(), 1);
            assert_eq!(arg(&actions(&out)[0], n).as_deref(), Some("on"));
        } else {
            assert_eq!(
                changes(&out, n).last().map(String::as_str),
                Some(sent),
                "{:?}",
                self.k
            );
        }
        if let Some(v) = self.shown() {
            assert_eq!(v, held, "{:?}: the box shows the held value", self.k);
        }
        out
    }

    /// What the control shows, in the probe's encoding. `None` for a
    /// checkbox or radio: a click sets the live state only and there is no
    /// public reader for it, so those are read from the form a request
    /// posts once the control is ungreyed.
    fn shown(&self) -> Option<String> {
        let a = |id: &str, n: &str| self.e.attr(id, n);
        Some(match self.k {
            Number | Text | Range => a(self.k.name(), "value").unwrap_or_default(),
            Select => ["a", "b", "c", "d"]
                .iter()
                .filter(|o| a(&format!("pick-{o}"), "selected").is_some())
                .map(|o| o.to_string())
                .collect::<Vec<_>>()
                .join(","),
            Checkbox | Radio => return None,
        })
    }

    fn greyed(&self) -> bool {
        let id = match self.k {
            Radio => "rb",
            k => k.name(),
        };
        self.e.attr(id, "disabled").is_some()
    }

    fn grey(&mut self, src: Source, game_value: &str) {
        match src {
            Markup => self.redraw("ctl", self.k.html(game_value, true)),
            Actions => self.set_actions(vec![(
                self.k.action(),
                ActionState::Disabled {
                    reason: "Locked".into(),
                },
            )]),
            Rule => self.redraw("gate", gate_html(false)),
        }
    }

    fn ungrey(&mut self, src: Source, game_value: &str) {
        match src {
            Markup => self.redraw("ctl", self.k.html(game_value, false)),
            Actions => self.set_actions(vec![]),
            Rule => self.redraw("gate", gate_html(true)),
        }
    }

    /// The form as a request would post it (the Go button), and every
    /// outcome of that click.
    fn post_form(&mut self) -> (Vec<InputOutcome>, Option<String>) {
        let o = self.click("go");
        let a = actions(&o);
        let v = a.last().and_then(|a| arg(a, self.k.name()));
        (o, v)
    }
}

fn requests(outs: &[InputOutcome]) -> Vec<RequestId> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .collect()
}

fn actions(outs: &[InputOutcome]) -> Vec<Vec<(String, String)>> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { args, .. } => Some(args.clone()),
            _ => None,
        })
        .collect()
}

fn action_names(outs: &[InputOutcome]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { name, .. } => Some(name.clone()),
            InputOutcome::Blocked { name, .. } => Some(format!("blocked:{name}")),
            _ => None,
        })
        .collect()
}

fn arg(args: &[(String, String)], k: &str) -> Option<String> {
    args.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
}

fn changes(outs: &[InputOutcome], name: &str) -> Vec<String> {
    actions(outs).iter().filter_map(|a| arg(a, name)).collect()
}

/// The value the game holds when it greys, by variant: the drawn value
/// after a redraw that drew one, else the last value sent.
fn expected(k: Kind, before: Before) -> &'static str {
    let (_, sent, _, drawn) = k.values();
    match before {
        RedrawDrawsOther => drawn,
        _ => sent,
    }
}

/// One cell of the matrix. Returns a failure line, or `None`.
fn run(k: Kind, src: Source, before: Before) -> Option<String> {
    let mut h = H::new(k);
    let tag = format!("{k:?}/{src:?}/{before:?}");
    let inflight = h.send_then_hold();
    let (_, sent, _, drawn) = k.values();
    match before {
        RedrawDrawsOther => h.redraw("ctl", k.html(drawn, false)),
        RedrawDrawsSent => h.redraw("ctl", k.html(sent, false)),
        UnrelatedRedraw => h.redraw("hud", "<b id=\"clock\">1</b>".into()),
        NoRedraw => {}
    }
    let want = expected(k, before);
    h.grey(src, want);
    let mut bad = Vec::new();
    // Markup greying of an `[hx-post]` control does not stick: the forced
    // `actions` pass after the swap clears it (pre-existing, see
    // `markup_disabled_on_an_hx_post_control_stays_greyed`). The held change
    // is still dropped while the markup greys it, which is what is checked.
    if !h.greyed() && src != Markup {
        bad.push("not greyed".to_string());
    }
    if h.shown().is_some_and(|v| v != want) {
        bad.push(format!("greyed shows {:?}, want {want:?}", h.shown()));
    }
    if matches!(k, Number | Text) {
        if let Some(d) = h.e.test_editor_display(k.name()) {
            if d != want && !(k == Number && d.parse::<f64>().ok() == want.parse::<f64>().ok()) {
                bad.push(format!("editor displays {d:?}, want {want:?}"));
            }
        }
        if h.e.focus_state().focused_id.as_deref() == Some(k.name()) {
            bad.push("greyed box keeps focus".into());
        }
        if h.e.test_caret_visible().is_some() {
            bad.push("greyed box keeps an edit session (caret)".into());
        }
    }
    let done = h.finish(&inflight);
    if !action_names(&done).is_empty() {
        bad.push(format!("complete sent {:?}", action_names(&done)));
    }
    h.ungrey(src, want);
    let o = h.e.take_outcomes();
    if !o.is_empty() {
        bad.push(format!("ungrey sent {o:?}"));
    }
    if h.shown().is_some_and(|v| v != want) {
        bad.push(format!("ungreyed shows {:?}, want {want:?}", h.shown()));
    }
    let (o, posted) = h.post_form();
    if action_names(&o) != ["deal.go"] {
        bad.push(format!("Go sent {:?}", action_names(&o)));
    }
    let posted_want = if k == Checkbox && want.is_empty() {
        None
    } else {
        Some(want.to_string())
    };
    let same = match (&posted, &posted_want) {
        (Some(a), Some(b)) if k == Number => a.parse::<f64>().ok() == b.parse::<f64>().ok(),
        (a, b) => a == b,
    };
    if !same {
        bad.push(format!("form posts {posted:?}, want {posted_want:?}"));
    }
    let blur = h.click("blur");
    if !actions(&blur).is_empty() {
        bad.push(format!("blur sent {:?}", action_names(&blur)));
    }
    (!bad.is_empty()).then(|| format!("{tag}: {}", bad.join("; ")))
}

fn matrix(befores: &[Before]) -> Vec<String> {
    let mut fails = Vec::new();
    for k in KINDS {
        for src in SOURCES {
            for b in befores {
                if let Some(f) = run(k, src, *b) {
                    fails.push(f);
                }
            }
        }
    }
    fails
}

/// Gap B (#96) itself, every kind and every greying source: a redraw draws
/// a third value while the change is held, then the control greys. It shows
/// the drawn value, sends nothing on complete, on ungrey or on blur, and
/// the form then carries the drawn value.
#[test]
fn gap_b_every_kind_every_source() {
    let fails = matrix(&[RedrawDrawsOther]);
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

/// No redraw before greying, and a redraw that draws the value sent: the
/// control shows the last value sent (for a radio group, the last value
/// any radio of the group sent).
#[test]
fn greyed_with_no_redraw_or_a_redraw_of_the_sent_value() {
    let fails = matrix(&[NoRedraw, RedrawDrawsSent]);
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

/// A redraw of a section that does not hold the control (a HUD on the
/// `time` slice redraws every tick) draws nothing in it. Greying then must
/// still show the last value sent, not the held value.
#[test]
fn greyed_after_an_unrelated_redraw_shows_the_value_sent() {
    let fails = matrix(&[UnrelatedRedraw]);
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

/// The full matrix in one list, for the review notes.
#[test]
fn full_matrix_summary() {
    let fails = matrix(&BEFORES);
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

// ---------- named cases ----------

/// The `actions` slice greys the control's action.
fn grey_fee(h: &mut H) {
    h.grey(Actions, "");
}

/// The regression in #98 at 8d1765d, by itself: 4.7 sent, 4.71 held, a HUD
/// section redraws (it does not hold the fee box), then the `actions` slice
/// greys the fee box while it has focus. The game has 4.7. At 768527d the
/// box showed 4.7 (Ruling 6 put the session's sent value back); at 8d1765d
/// it shows the held 4.71, which is never sent, so the box and the game
/// disagree after it ungreys.
#[test]
fn unrelated_redraw_then_grey_in_place_shows_the_value_sent() {
    let mut h = H::new(Number);
    let o1 = h.click("fee-up");
    h.click("fee-up");
    assert_eq!(h.e.focus_state().focused_id.as_deref(), Some("fee"));
    h.redraw("hud", "<b id=\"clock\">1</b>".into());
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.71"));
    grey_fee(&mut h);
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.7"));
    assert!(h.finish(&o1).is_empty());
    h.ungrey(Actions, "");
    assert!(actions(&h.click("blur")).is_empty());
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.7"));
}

/// The same root cause through an input rule: the rule greys on the gate's
/// own redraw, which is a redraw of another section.
#[test]
fn rule_greying_in_place_shows_the_value_sent() {
    let mut h = H::new(Number);
    let o1 = h.click("fee-up");
    h.click("fee-up");
    h.grey(Rule, "");
    assert!(h.greyed());
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.7"));
    assert!(h.finish(&o1).is_empty());
}

/// Ruling (5983ff17): the unapplied-change baseline is the game's last
/// value while a change is held. A redraw of another section draws nothing
/// in the form, so the held 4.71 must not become the baseline: the form
/// still reads as changed and Go stays enabled.
#[test]
fn baseline_is_not_taken_from_an_unrelated_redraw() {
    let mut h = H::new(Number);
    let _o1 = h.click("fee-up");
    h.click("fee-up");
    h.redraw("hud", "<b id=\"clock\">1</b>".into());
    h.set_actions(vec![(
        "deal.go".into(),
        ActionState::RequiresChange {
            form: "f".into(),
            reason: "Nothing to apply".into(),
        },
    )]);
    assert!(
        h.e.attr("go", "disabled").is_none(),
        "4.71 is not the game's value: the form reads as changed"
    );
}

/// Point 3: after Gap B greys and ungreys, the box steps from and restores
/// the game's drawn 4.65, never the stale 4.7 that was sent.
#[test]
fn after_gap_b_the_step_and_restore_base_is_the_games_value() {
    // ▲ on the box.
    let mut h = H::new(Number);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Number.html("4.65", false));
    grey_fee(&mut h);
    assert!(h.finish(&o1).is_empty());
    h.ungrey(Actions, "");
    let up = h.click("fee-up");
    assert_eq!(changes(&up, "fee"), ["4.66"], "steps from 4.65");

    // Emptied box: ▲ and the blur restore.
    let mut h = H::new(Number);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Number.html("4.65", false));
    grey_fee(&mut h);
    assert!(h.finish(&o1).is_empty());
    h.ungrey(Actions, "");
    h.e.test_set_focus("fee");
    for _ in 0..6 {
        h.tap("Backspace");
    }
    assert_eq!(h.e.test_editor_display("fee").as_deref(), Some(""));
    let blur = h.click("blur");
    assert!(actions(&blur).is_empty(), "{blur:?}");
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.65"), "restore");
    h.e.test_set_focus("fee");
    for _ in 0..6 {
        h.tap("Backspace");
    }
    let up = h.tap("ArrowUp");
    let mut sent = changes(&up, "fee");
    sent.extend(changes(&h.click("blur"), "fee"));
    assert_eq!(sent, ["4.66"], "ArrowUp on an empty box steps from 4.65");
}

/// Point 3, while the change is still held (no greying): an older redraw
/// draws 4.65 and puts 4.71 back; an emptied box still steps from and
/// restores the 4.7 actually sent (#90, #92), so #98 did not move the
/// session's base outside greying.
#[test]
fn while_held_the_step_and_restore_base_stays_the_value_sent() {
    let mut h = H::new(Number);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Number.html("4.65", false));
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.71"));
    for _ in 0..6 {
        h.tap("Backspace");
    }
    assert!(actions(&h.click("blur")).is_empty());
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.7"));
    let caught = h.finish(&o1);
    assert!(actions(&caught).is_empty(), "back on the sent value");

    let mut h = H::new(Number);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Number.html("4.65", false));
    for _ in 0..6 {
        h.tap("Backspace");
    }
    h.click("fee-up");
    assert_eq!(
        h.e.attr("fee", "value").as_deref(),
        Some("4.71"),
        "from 4.7"
    );
    let caught = h.finish(&o1);
    assert_eq!(changes(&caught, "fee"), ["4.71"]);
}

/// Greying with focus, caret and uncommitted typing after Gap B's redraw:
/// the typing (kept across the redraw) is dropped, the box shows 4.65 with
/// no caret, and nothing is sent on complete, ungrey or blur.
#[test]
fn gap_b_with_typing_in_progress_and_caret() {
    let mut h = H::new(Number);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Number.html("4.65", false));
    for _ in 0..6 {
        h.tap("Backspace");
    }
    h.text("9");
    h.redraw("hud", "<b id=\"clock\">2</b>".into());
    assert_eq!(h.e.test_editor_display("fee").as_deref(), Some("9"));
    h.e.test_force_caret_blink();
    grey_fee(&mut h);
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.65"));
    assert_eq!(h.e.test_editor_display("fee").as_deref(), Some("4.65"));
    assert_eq!(h.e.test_caret_visible(), None, "no edit session, no caret");
    assert_ne!(h.e.focus_state().focused_id.as_deref(), Some("fee"));
    assert!(h.finish(&o1).is_empty());
    h.ungrey(Actions, "");
    assert!(h.e.take_outcomes().is_empty());
    assert!(actions(&h.click("blur")).is_empty());
    assert_eq!(h.e.attr("fee", "value").as_deref(), Some("4.65"));
}

/// The same for a text box: "a" sent, "ab" held, the game draws "z", the
/// player types "c" (not committed), then the box greys in place.
#[test]
fn gap_b_text_box_with_typing_in_progress() {
    let mut h = H::new(Text);
    let o1 = h.send_then_hold();
    h.redraw("ctl", Text.html("z", false));
    assert_eq!(h.e.attr("note", "value").as_deref(), Some("ab"));
    h.text("c");
    grey_fee(&mut h);
    assert_eq!(h.e.attr("note", "value").as_deref(), Some("z"));
    assert_eq!(h.e.test_editor_display("note").as_deref(), Some("z"));
    assert_eq!(h.e.test_caret_visible(), None);
    assert!(h.finish(&o1).is_empty());
    h.ungrey(Actions, "");
    assert!(actions(&h.click("blur")).is_empty());
    let (_, posted) = h.post_form();
    assert_eq!(posted.as_deref(), Some("z"));
}

/// #94 patch 0002 still holds with #98: a radio group's held pick catches
/// up against the group's last sent value, and a held untick posts no pair.
/// (The original probes run in `graph_probes_94.rs`; these are the Gap B
/// shapes of the same two cases.)
#[test]
fn radio_greyed_while_held_shows_the_groups_game_value() {
    // No redraw: the group last sent c.
    let mut h = H::new(Radio);
    let o = h.send_then_hold();
    grey_fee(&mut h);
    assert!(h.finish(&o).is_empty());
    h.ungrey(Actions, "");
    assert_eq!(h.post_form().1.as_deref(), Some("c"));
    // A redraw drew d.
    let mut h = H::new(Radio);
    let o = h.send_then_hold();
    h.redraw("ctl", Radio.html("d", false));
    grey_fee(&mut h);
    assert!(h.finish(&o).is_empty());
    h.ungrey(Actions, "");
    assert_eq!(h.post_form().1.as_deref(), Some("d"));
    // Clicking b now sends b: the box really showed d.
    let o = h.click("rb");
    assert_eq!(changes(&o, "r"), ["b"]);
}

#[test]
fn checkbox_greyed_while_held_after_a_ticked_redraw_shows_ticked() {
    // Ticked (sent), untick held, the game draws it ticked (it took the
    // tick), then greys: the box is ticked, posted with its pair.
    let mut h = H::new(Checkbox);
    let o = h.send_then_hold();
    h.redraw("ctl", Checkbox.html("on", false));
    grey_fee(&mut h);
    assert!(h.e.attr("tick", "checked").is_some(), "checked attribute");
    assert!(h.finish(&o).is_empty());
    h.ungrey(Actions, "");
    assert_eq!(h.post_form().1.as_deref(), Some("on"));
}

/// Pre-existing, not #98: a `disabled` written in the game's markup on an
/// `[hx-post]` control is cleared by the forced `actions` pass after the
/// swap (`apply_disabled_map` writes "not greyed" for every `[hx-post]`
/// element its action does not grey). REQUIREMENTS says the engine never
/// strips a host-authored `disabled`, and Designer's 5983ff17 ruling names
/// greying in the markup. Ignored until Htmx and Designer decide which is
/// meant.
#[test]
#[ignore = "pre-existing: markup `disabled` on an [hx-post] control is cleared by the actions pass"]
fn markup_disabled_on_an_hx_post_control_stays_greyed() {
    let mut h = H::new(Number);
    h.redraw("ctl", Number.html("4.69", true));
    assert!(h.greyed());
}
