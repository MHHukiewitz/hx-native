//! Issue #89 (Designer, HTML_CONTRACT `hx-sync`, Mainnet master 188feffd):
//! `drop` is for clicks and actions. A value control never loses the
//! player's input: a `change` committed while a request is pending is not
//! marked as sent, and when the request finishes (done, failed or timed
//! out) the box sends one `change` if its value differs from the last value
//! sent. A redraw from an older request never overwrites a newer committed
//! value, the game's answer to a sent value stands, and text being typed
//! survives a redraw. Graph's review points 1-4.

use std::cell::RefCell;
use std::rc::Rc;

use hx_native::*;

fn vp() -> Viewport {
    Viewport::with_scale(300, 160, 1.0)
}

const STYLE: &str = "<style>*{box-sizing:border-box}html,body{margin:0;background:#fff;\
font:12px \"DejaVu Sans\"}input,select{display:block;width:200px;height:22px;margin:4px}\
#spin{display:block;position:relative}#spin i{position:absolute;left:220px;width:20px;height:11px;\
font-style:normal;font-size:8px}#up{top:4px}#dn{top:15px}</style>";

/// The game's side: what each section renders, and a version to bump.
#[derive(Default, Clone)]
struct Game {
    fee: String,
    version: u64,
    sync_attr: &'static str,
    /// The game greys the fee in its markup (#94).
    disabled: bool,
    /// The `actions` slice and its version (#94).
    actions: Vec<(String, ActionState)>,
    actions_version: u64,
}

struct Slices(Game);
impl SliceSource for Slices {
    fn version(&self, name: &str) -> Option<u64> {
        match name {
            "deal" => Some(self.0.version),
            "actions" => Some(self.0.actions_version),
            _ => None,
        }
    }
    fn action_states(&self) -> Vec<(String, ActionState)> {
        self.0.actions.clone()
    }
}

struct Frags(Rc<RefCell<Game>>);
impl FragmentSource for Frags {
    fn render(&mut self, _s: &str, _slices: &[&str]) -> String {
        let g = self.0.borrow();
        let grey = if g.disabled { " disabled" } else { "" };
        fee_html(&g.fee, &format!("{}{grey}", g.sync_attr))
    }
}

/// The deal form the game draws: the fee box with its arrows, and an
/// Apply button (#94's `RequiresChange` baseline).
fn fee_html(value: &str, extra_attrs: &str) -> String {
    format!(
        "<form id=\"df\"><span id=\"spin\"><input id=\"fee\" type=\"number\" name=\"fee\" value=\"{value}\" min=\"0\" \
         max=\"100.00\" step=\"0.01\" hx-post=\"deal.fee\" hx-trigger=\"change\"{extra_attrs} />\
         <i id=\"up\" data-hx-step=\"up\" data-hx-step-for=\"fee\">U</i>\
         <i id=\"dn\" data-hx-step=\"down\" data-hx-step-for=\"fee\">D</i></span>\
         <button id=\"apply\" hx-post=\"deal.apply\">Apply</button></form>"
    )
}

struct Deal {
    e: Engine,
    game: Rc<RefCell<Game>>,
}

/// The fee box inside a section the game redraws on `slice:deal`, and a
/// text box `#b` to blur to. `sync_attr` adds e.g. ` hx-sync="queue"`.
fn deal_with(sync_attr: &'static str, timeout_frames: u32) -> Deal {
    let game = Rc::new(RefCell::new(Game {
        fee: "4.69".into(),
        version: 1,
        sync_attr,
        actions_version: 1,
        ..Default::default()
    }));
    let mut e = Engine::new(EngineConfig {
        paint_mode: PaintMode::Exact,
        pending_timeout_frames: timeout_frames,
        ..Default::default()
    });
    e.declare_actions(&["deal.fee", "deal.apply"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body>\
         <section id=\"deal\" hx-trigger=\"slice:deal\" hx-target=\"#deal\" hx-swap=\"innerHTML\">{}</section>\
         <input id=\"b\" type=\"text\" name=\"b\" value=\"x\" /></body></html>",
        fee_html("4.69", sync_attr)
    ))
    .unwrap();
    let _ = e.paint(vp()).unwrap();
    let mut d = Deal { e, game };
    d.redraw();
    d
}

fn deal() -> Deal {
    deal_with("", 1000)
}

impl Deal {
    /// The game redraws the deal section with its own fee.
    fn redraw(&mut self) {
        self.sync_only();
        self.game.borrow_mut().version += 1;
    }

    /// Sync without redrawing the deal section (e.g. a new `actions` slice).
    fn sync_only(&mut self) {
        let g = self.game.borrow().clone();
        self.e
            .sync(&Slices(g), &mut Frags(self.game.clone()))
            .unwrap();
        let _ = self.e.paint(vp()).unwrap();
    }

    /// The game publishes `actions` (greying) without a redraw.
    fn set_actions(&mut self, actions: Vec<(String, ActionState)>) {
        {
            let mut g = self.game.borrow_mut();
            g.actions = actions;
            g.actions_version += 1;
        }
        self.sync_only();
    }

    /// The game answers `fee` with `answer` (its own value, e.g. a clamp)
    /// and redraws.
    fn answer_redraw(&mut self, answer: &str) {
        self.game.borrow_mut().fee = answer.into();
        self.redraw();
    }

    fn mid(&self, id: &str) -> (f32, f32) {
        let b = self.e.test_element_box(id).unwrap();
        (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0)
    }

    fn feed(&mut self, ev: InputEvent) -> Vec<InputOutcome> {
        let mut out = vec![self.e.input(ev)];
        out.extend(self.e.take_outcomes());
        let _ = self.e.paint(vp());
        out
    }

    fn click(&mut self, id: &str) -> Vec<InputOutcome> {
        let (x, y) = self.mid(id);
        let mods = Modifiers::default();
        let button = PointerButton::Primary;
        let mut out = self.feed(InputEvent::PointerDown { x, y, button, mods });
        out.extend(self.feed(InputEvent::PointerUp { x, y, button, mods }));
        out
    }

    fn key(&mut self, k: &str) -> Vec<InputOutcome> {
        let mut out = self.feed(key_ev(k, true));
        out.extend(self.feed(key_ev(k, false)));
        out
    }

    fn fee(&self) -> String {
        self.e.attr("fee", "value").unwrap_or_default()
    }

    fn shown(&mut self) -> Option<String> {
        self.e.test_editor_display("fee")
    }

    /// The host finishes `outs`' requests; returns what that released.
    fn complete(&mut self, outs: &[InputOutcome], ok: bool) -> Vec<InputOutcome> {
        for r in requests(outs) {
            let res = if ok {
                Ok(())
            } else {
                Err(Reason("Not now".into()))
            };
            self.e.complete(r, res);
        }
        let out = self.e.take_outcomes();
        let _ = self.e.paint(vp());
        out
    }
}

fn key_ev(k: &str, pressed: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: k.into(),
        code: k.into(),
        mods: Modifiers::default(),
        repeat: false,
        pressed,
    })
}

fn requests(outs: &[InputOutcome]) -> Vec<RequestId> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action { request, .. } => Some(*request),
            _ => None,
        })
        .collect()
}

/// The `name` arg of every `change` in `outs`.
fn changes_of(outs: &[InputOutcome], name: &str) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            InputOutcome::Action {
                trigger: TriggerInfo::Change,
                args,
                ..
            } => args.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()),
            _ => None,
        })
        .collect()
}

fn changes(outs: &[InputOutcome]) -> Vec<String> {
    changes_of(outs, "fee")
}

/// Required: three fast ▲ presses send exactly two changes, 4.7 then 4.72,
/// and the box and the game end equal. The same under `hx-sync="queue"`.
#[test]
fn three_fast_presses_send_the_first_and_then_one_catch_up() {
    for sync_attr in ["", " hx-sync=\"drop\"", " hx-sync=\"queue\""] {
        let mut d = deal_with(sync_attr, 1000);
        let o1 = d.click("up");
        let o2 = d.click("up");
        let o3 = d.click("up");
        assert_eq!(d.fee(), "4.72", "{sync_attr}");
        assert_eq!(changes(&o1), ["4.7"], "{sync_attr}");
        assert!(
            changes(&o2).is_empty() && changes(&o3).is_empty(),
            "{sync_attr}"
        );
        let caught = d.complete(&o1, true);
        assert_eq!(changes(&caught), ["4.72"], "{sync_attr}: one catch-up");
        let after = d.complete(&caught, true);
        assert!(after.is_empty(), "{sync_attr}: {after:?}");
        let blur = d.click("b");
        assert!(
            changes(&blur).is_empty(),
            "{sync_attr}: nothing left to send"
        );
        assert_eq!(d.fee(), "4.72", "{sync_attr}: the game has the box's value");
    }
}

/// Required: typed `9` and Enter while the arrow's change is pending.
#[test]
fn typed_enter_while_pending_catches_up() {
    let mut d = deal();
    let o1 = d.click("up");
    assert_eq!(changes(&o1), ["4.7"]);
    let mut ctrl_a = KeyEvent {
        key: "a".into(),
        code: "KeyA".into(),
        mods: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
        pressed: true,
    };
    d.feed(InputEvent::Key(ctrl_a.clone()));
    ctrl_a.pressed = false;
    d.feed(InputEvent::Key(ctrl_a));
    d.feed(InputEvent::Text("9".into()));
    let enter = d.key("Enter");
    assert_eq!(d.fee(), "9");
    assert!(changes(&enter).is_empty(), "held while 4.7 is pending");
    let caught = d.complete(&o1, true);
    assert_eq!(changes(&caught), ["9"]);
    d.complete(&caught, true);
    assert!(changes(&d.click("b")).is_empty());
}

/// Point 2: a failed request, or a timed-out one, still runs the catch-up.
#[test]
fn a_failed_or_timed_out_request_still_catches_up() {
    let mut d = deal();
    let o1 = d.click("up");
    d.click("up");
    let caught = d.complete(&o1, false);
    assert_eq!(changes(&caught), ["4.71"], "after an error");

    // 20 frames: the three clicks paint fewer than that.
    let mut d = deal_with("", 20);
    d.click("up");
    d.click("up");
    d.click("up");
    let mut caught = Vec::new();
    for _ in 0..30 {
        d.e.test_tick_frame();
        caught.extend(d.e.take_outcomes());
        if !caught.is_empty() {
            break;
        }
    }
    assert_eq!(changes(&caught), ["4.72"], "after a timeout");
    assert_eq!(d.e.pending_timeout_failures().len(), 1, "the first request");
    // The catch-up timing out too sends nothing more: 4.72 was sent.
    for _ in 0..30 {
        d.e.test_tick_frame();
        assert!(d.e.take_outcomes().is_empty());
    }
    assert_eq!(d.e.pending_timeout_failures().len(), 2);
}

/// Point 1: only a player's commit holds a change. When the game clamps
/// (or refuses) the caught-up value and redraws the box with its own value,
/// that answer stands and nothing is sent again.
#[test]
fn the_games_clamp_after_the_catch_up_stands_and_nothing_resends() {
    let mut d = deal();
    let o1 = d.click("up");
    d.click("up");
    d.click("up");
    let caught = d.complete(&o1, true);
    assert_eq!(changes(&caught), ["4.72"]);
    let done = d.complete(&caught, true);
    assert!(done.is_empty());
    d.answer_redraw("4.70");
    assert_eq!(d.fee(), "4.70", "the game's answer stands");
    let quiet = d.e.take_outcomes();
    assert!(quiet.is_empty(), "{quiet:?}");
    let blur = d.click("b");
    assert!(changes(&blur).is_empty(), "no resend on blur");
    d.answer_redraw("4.70");
    assert!(d.e.take_outcomes().is_empty());
}

/// Point 3: the step base for an arrow on an emptied box (#92) and the value
/// a blur puts back (#90) are the last value actually sent (4.7), never a
/// held one, after three fast presses.
#[test]
fn step_base_and_restore_use_the_value_actually_sent() {
    // Restore: empty the box and blur while 4.7 is pending.
    let mut d = deal();
    let o1 = d.click("up");
    d.click("up");
    d.click("up");
    for _ in 0..4 {
        d.key("Backspace");
    }
    assert_eq!(d.shown().as_deref(), Some(""));
    let blur = d.click("b");
    assert!(changes(&blur).is_empty());
    assert_eq!(
        d.fee(),
        "4.7",
        "blur puts back the value sent, not the held 4.72"
    );
    let caught = d.complete(&o1, true);
    assert!(
        changes(&caught).is_empty(),
        "the box is back on the sent value"
    );

    // Step base: empty the box, then ▲ while 4.7 is pending.
    let mut d = deal();
    let o1 = d.click("up");
    d.click("up");
    d.click("up");
    for _ in 0..4 {
        d.key("Backspace");
    }
    d.click("up");
    assert_eq!(d.fee(), "4.71", "▲ steps from the sent 4.7");
    let caught = d.complete(&o1, true);
    assert_eq!(changes(&caught), ["4.71"]);
}

/// Steering point 1 (Mainnet 188feffd): the player commits 4.72 while 4.71
/// is pending; the redraw that answers 4.71 never overwrites 4.72, the
/// catch-up sends 4.72, and the game's answer to that one wins.
#[test]
fn a_redraw_answering_an_older_request_keeps_the_newer_commit() {
    let mut d = deal();
    let o1 = d.click("up"); // 4.70 sent
    d.click("up"); // 4.71 held
    let r2 = d.complete(&o1, true);
    assert_eq!(changes(&r2), ["4.71"], "4.71 caught up");
    d.answer_redraw("4.70"); // the answer to 4.70 arrives while 4.71 is pending
    assert_eq!(
        d.fee(),
        "4.71",
        "a redraw never overwrites the sent-but-unanswered 4.71"
    );
    d.click("up"); // 4.72 held while 4.71 is pending
    assert_eq!(d.fee(), "4.72");
    d.answer_redraw("4.71"); // still before 4.71 completes
    assert_eq!(d.fee(), "4.72", "held 4.72 survives the redraw");
    let r3 = d.complete(&r2, true);
    assert_eq!(changes(&r3), ["4.72"], "the catch-up sends 4.72");
    d.answer_redraw("4.71"); // the reply to 4.71, after it completed
    assert_eq!(d.fee(), "4.72", "4.72 is still unanswered");
    assert!(d.complete(&r3, true).is_empty());
    d.answer_redraw("4.72");
    assert_eq!(d.fee(), "4.72");
    d.answer_redraw("4.65"); // a later game value wins once 4.72 is answered
    assert_eq!(d.fee(), "4.65");
    assert!(d.e.take_outcomes().is_empty());
}

/// Steering point 3: text being typed (not committed) survives a redraw of
/// its box; Enter then commits it.
#[test]
fn uncommitted_typing_survives_a_redraw() {
    let mut d = deal();
    d.e.test_set_focus("fee");
    d.key("End");
    d.feed(InputEvent::Text("5".into()));
    assert_eq!(d.shown().as_deref(), Some("4.695"));
    d.answer_redraw("4.69");
    assert_eq!(d.shown().as_deref(), Some("4.695"), "typed text survives");
    assert_eq!(d.e.focus_state().focused_id.as_deref(), Some("fee"));
    d.feed(InputEvent::Text("1".into()));
    assert_eq!(
        d.shown().as_deref(),
        Some("4.6951"),
        "the caret stayed at the end"
    );
    let enter = d.key("Enter");
    assert_eq!(changes(&enter), ["4.6951"]);
}

/// Required cross-check: whatever the presses, once every request has
/// completed and the box blurs, the last change sent is the box's value.
#[test]
fn the_last_change_sent_is_the_box_value() {
    for presses in 1..=5 {
        for down_at in [None, Some(2)] {
            let mut d = deal();
            let mut sent: Vec<String> = Vec::new();
            let mut inflight: Vec<InputOutcome> = Vec::new();
            for i in 0..presses {
                let arrow = if Some(i) == down_at { "dn" } else { "up" };
                let o = d.click(arrow);
                sent.extend(changes(&o));
                inflight.extend(o);
            }
            while !requests(&inflight).is_empty() {
                let next = d.complete(&inflight, true);
                sent.extend(changes(&next));
                inflight = next;
            }
            let blur = d.click("b");
            sent.extend(changes(&blur));
            let box_value = d.fee();
            let last = sent.last().cloned().unwrap_or_default();
            assert!(
                last == box_value || last.parse::<f64>().ok() == box_value.parse::<f64>().ok(),
                "{presses} presses, ▼ at {down_at:?}: sent {sent:?}, box {box_value}"
            );
        }
    }
}

fn controls_page(e: &mut Engine) {
    e.declare_actions(&["deal.note", "deal.pick", "deal.vol"]);
    e.load_html(&format!(
        "<!DOCTYPE html><html><head>{STYLE}</head><body>\
         <input id=\"note\" type=\"text\" name=\"note\" value=\"\" hx-post=\"deal.note\" hx-trigger=\"change\" />\
         <select id=\"pick\" name=\"pick\" hx-post=\"deal.pick\" hx-trigger=\"change\">\
         <option value=\"a\" selected>A</option><option value=\"b\">B</option><option value=\"c\">C</option></select>\
         <input id=\"vol\" type=\"range\" name=\"vol\" min=\"0\" max=\"10\" step=\"1\" value=\"5\" hx-post=\"deal.vol\" hx-trigger=\"change\" />\
         <input id=\"other\" type=\"text\" name=\"other\" value=\"\" /></body></html>"
    ))
    .unwrap();
    let _ = e.paint(vp()).unwrap();
}

fn press(e: &mut Engine, ev: InputEvent) -> Vec<InputOutcome> {
    let mut out = vec![e.input(ev)];
    out.extend(e.take_outcomes());
    let _ = e.paint(vp());
    out
}

fn tap(e: &mut Engine, k: &str) -> Vec<InputOutcome> {
    let mut out = press(e, key_ev(k, true));
    out.extend(press(e, key_ev(k, false)));
    out
}

fn finish(e: &mut Engine, outs: &[InputOutcome]) -> Vec<InputOutcome> {
    for r in requests(outs) {
        e.complete(r, Ok(()));
    }
    e.take_outcomes()
}

/// Required: a text field. `a` + Enter sends; `b` + Enter while it is
/// pending is held, and caught up as `ab`.
#[test]
fn a_text_field_catches_up() {
    let mut e = Engine::new(EngineConfig::default());
    controls_page(&mut e);
    e.test_set_focus("note");
    press(&mut e, InputEvent::Text("a".into()));
    let o1 = tap(&mut e, "Enter");
    assert_eq!(changes_of(&o1, "note"), ["a"]);
    press(&mut e, InputEvent::Text("b".into()));
    let o2 = tap(&mut e, "Enter");
    assert!(changes_of(&o2, "note").is_empty());
    let caught = finish(&mut e, &o1);
    assert_eq!(changes_of(&caught, "note"), ["ab"]);
    assert!(finish(&mut e, &caught).is_empty());
    e.test_set_focus("other");
    assert_eq!(e.attr("note", "value").as_deref(), Some("ab"));
}

/// Required: a select and a range. Two keyboard steps while the first is
/// pending: the second value is caught up.
#[test]
fn a_select_and_a_range_catch_up() {
    let mut e = Engine::new(EngineConfig::default());
    controls_page(&mut e);
    e.test_set_focus("pick");
    let o1 = tap(&mut e, "ArrowDown");
    assert_eq!(changes_of(&o1, "pick"), ["b"]);
    let o2 = tap(&mut e, "ArrowDown");
    assert!(changes_of(&o2, "pick").is_empty());
    let caught = finish(&mut e, &o1);
    assert_eq!(changes_of(&caught, "pick"), ["c"], "select");

    e.test_set_focus("vol");
    let o1 = tap(&mut e, "ArrowRight");
    assert_eq!(changes_of(&o1, "vol"), ["6"]);
    tap(&mut e, "ArrowRight");
    tap(&mut e, "ArrowRight");
    let caught = finish(&mut e, &o1);
    assert_eq!(changes_of(&caught, "vol"), ["8"], "range");
    assert!(finish(&mut e, &caught).is_empty());
}

/// #94 (Designer, Mainnet 5983ff17): the game greys the fee while 4.71 is
/// held: the held change is dropped (no catch-up) and the box shows the
/// game's value, by the `actions` slice (the last value sent, 4.7) or by a
/// greyed redraw (the value it draws, 4.65).
#[test]
fn greyed_while_held_drops_the_held_change() {
    let mut d = deal();
    // #96: take the redraw `deal()` leaves due, so this half greys by the
    // `actions` slice alone (a redraw first is the drawn-value case).
    d.sync_only();
    let o1 = d.click("up"); // 4.7 sent
    d.click("up"); // 4.71 held
    d.set_actions(vec![(
        "deal.fee".into(),
        ActionState::Disabled {
            reason: "Locked".into(),
        },
    )]);
    assert_eq!(d.fee(), "4.7", "the game's value: the last value sent");
    let after = d.complete(&o1, true);
    assert!(changes(&after).is_empty(), "no catch-up: {after:?}");
    assert!(after.is_empty(), "nothing at all: {after:?}");

    let mut d = deal();
    let o1 = d.click("up");
    d.click("up");
    d.game.borrow_mut().disabled = true;
    d.answer_redraw("4.65");
    assert_eq!(d.fee(), "4.65", "the greyed redraw's value");
    let after = d.complete(&o1, true);
    assert!(after.is_empty(), "{after:?}");
    d.game.borrow_mut().disabled = false;
    d.redraw();
    assert_eq!(d.fee(), "4.65");
    assert!(d.e.take_outcomes().is_empty());
}

/// #94: the unapplied-change baseline stays at the game's last value while a
/// change is held: a redraw that answers 4.7 while 4.71 is held puts 4.71
/// back, and the form still reads as changed (Apply ungreyed) until the
/// game answers 4.71. (The game publishes its `actions` slice with each
/// redraw, which is when the redrawn form's baseline is taken.)
#[test]
fn the_baseline_stays_at_the_games_value_while_a_change_is_held() {
    let mut d = deal();
    let apply = vec![(
        "deal.apply".to_string(),
        ActionState::RequiresChange {
            form: "df".into(),
            reason: "Nothing to apply".into(),
        },
    )];
    d.set_actions(apply.clone());
    assert!(d.e.attr("apply", "disabled").is_some(), "clean at load");
    let o1 = d.click("up"); // 4.7 sent
    d.answer_redraw("4.7"); // the game draws 4.7 while it is still pending
    d.set_actions(apply.clone());
    assert!(d.e.attr("apply", "disabled").is_some(), "the game drew 4.7");
    d.click("up"); // 4.71 held
    assert!(d.e.attr("apply", "disabled").is_none(), "4.71 is a change");
    d.answer_redraw("4.7"); // an older redraw: 4.71 is put back
    d.set_actions(apply.clone());
    assert_eq!(d.fee(), "4.71");
    assert!(
        d.e.attr("apply", "disabled").is_none(),
        "the put-back is not the baseline: the form still reads as changed"
    );
    let caught = d.complete(&o1, true);
    assert_eq!(changes(&caught), ["4.71"]);
    d.answer_redraw("4.7"); // still the older answer: 4.71 is unanswered
    d.set_actions(apply.clone());
    assert!(
        d.e.attr("apply", "disabled").is_none(),
        "until the game answers"
    );
    d.complete(&caught, true);
    d.answer_redraw("4.71"); // the game's answer
    d.set_actions(apply);
    assert!(
        d.e.attr("apply", "disabled").is_some(),
        "answered: clean again"
    );
}

/// A checkbox catches up too: ticked (sent), unticked while that is
/// pending (held), and the catch-up posts the form without the box. Ticked
/// back before the answer, it needs no catch-up.
#[test]
fn a_checkbox_catches_up() {
    let page = "<!DOCTYPE html><html><head></head><body>\
         <input id=\"tick\" type=\"checkbox\" name=\"tick\" value=\"yes\" hx-post=\"deal.tick\" hx-trigger=\"change\" style=\"display:block;width:20px;height:20px\" />\
         </body></html>";
    let ticks = |outs: &[InputOutcome]| -> Vec<Option<String>> {
        outs.iter()
            .filter_map(|o| match o {
                InputOutcome::Action {
                    trigger: TriggerInfo::Change,
                    args,
                    ..
                } => Some(
                    args.iter()
                        .find(|(k, _)| k == "tick")
                        .map(|(_, v)| v.clone()),
                ),
                _ => None,
            })
            .collect()
    };
    for clicks in [2, 3] {
        let mut e = Engine::new(EngineConfig::default());
        e.declare_actions(&["deal.tick"]);
        e.load_html(page).unwrap();
        let _ = e.paint(vp()).unwrap();
        let b = e.test_element_box("tick").unwrap();
        let (x, y) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
        let mut first = Vec::new();
        for i in 0..clicks {
            let mods = Modifiers::default();
            let button = PointerButton::Primary;
            let mut o = press(&mut e, InputEvent::PointerDown { x, y, button, mods });
            o.extend(press(&mut e, InputEvent::PointerUp { x, y, button, mods }));
            if i == 0 {
                first = o;
            } else {
                assert!(ticks(&o).is_empty(), "click {i} is held");
            }
        }
        assert_eq!(ticks(&first), [Some("yes".to_string())]);
        let caught = finish(&mut e, &first);
        if clicks == 2 {
            assert_eq!(ticks(&caught), [None], "unticked: posted without the box");
            assert!(finish(&mut e, &caught).is_empty());
        } else {
            assert!(
                caught.is_empty(),
                "ticked again: the sent value, {caught:?}"
            );
        }
    }
}

/// #96 (Gap B of Designer's #94 review): 4.7 sent, 4.71 held, a redraw
/// that does not grey draws 4.65 (4.71 is put back), then the `actions`
/// slice greys the focused box in place. The held change is dropped and the
/// box shows the game's last drawn value, 4.65, not the last value sent
/// (4.7); nothing is sent on complete or on a later blur.
#[test]
fn greyed_in_place_after_a_redraw_shows_the_games_drawn_value() {
    let mut d = deal();
    let o1 = d.click("up"); // 4.7 sent
    d.click("up"); // 4.71 held
    d.answer_redraw("4.65");
    assert_eq!(d.fee(), "4.71", "the held 4.71 is put back");
    assert_eq!(d.e.focus_state().focused_id.as_deref(), Some("fee"));
    d.set_actions(vec![(
        "deal.fee".into(),
        ActionState::Disabled {
            reason: "Locked".into(),
        },
    )]);
    assert_eq!(d.fee(), "4.65", "the game's drawn value, not the 4.7 sent");
    assert_eq!(d.shown().as_deref(), Some("4.65"));
    let after = d.complete(&o1, true);
    assert!(after.is_empty(), "{after:?}");
    let blur = d.click("b");
    assert!(changes(&blur).is_empty(), "{blur:?}");
    assert_eq!(d.fee(), "4.65");
}
