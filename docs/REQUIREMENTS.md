# `hx-native` fragment engine requirements (LOCKED)

**Status:** **Locked by Mike, 2026-10-19 12:51 (UTC-3, America/Asuncion).** Floor consensus merged the same day. The input additions are locked in scope; Designer cleared them on 2026-10-19 (trigger syntax, `[!repeat]` in Canvas mocks, A23 parity owned by Designer, Esc stays with Game unless a mock opts in). Game cleared them on 2026-10-19 with two additions (Input rules 9–10). The UI review is still pending.  
**Owner agent:** Htmx  
**Crate:** `hx-native` (Rust ident `hx_native`). Decided by Mike, 2026-10-19.  
**Repo:** public [`github.com/MHHukiewitz/hx-native`](https://github.com/MHHukiewitz/hx-native). Decided by Mike, 2026-10-19.  
**First unit:** this document only (no crate code until Mike locks).  
**Host game:** [MHHukiewitz/Mainnet](https://github.com/MHHukiewitz/Mainnet)  
**First spike window:** the **time strip** (today: egui `src/ui/time_controls.rs`; related PR/issue trail `#600` / `#603` full-width time strip + modal hold).  
**Related locks / contracts (do not contradict):**
- Frame budget: `docs/design/FRAME_BUDGET.md` (Mike lock 2026-09-22; floor ≥40 FPS, ideal ≥60; Week/Month ticks must not stall the next painted frame)
- Player-facing screen rules: `docs/design/UI_SCREEN_CONTRACT.md` (and workspace copy `/workspace/UI_SCREEN_CONTRACT.md`)
- Esc / window stack: Mainnet `#530` (Mike lock 2026-10-12: Esc closes top inner dialog first, then building — Win95; game owns stack; pairs with modal hold `#603`)

**Mike standing rules honored here:** never remove functionality or content; think long-term; migrate chrome in layer order (see Goals).

---

## Goals

1. **In-process HTMX-style fragment engine for Mainnet chrome.** Rust runs typed actions, renders HTML fragments, honors an HTMX-like attribute dialect (`hx-post` / `hx-target` / `hx-swap` / `hx-trigger`) against **named data slices**, and swaps those fragments into a paint-to-texture HTML/CSS surface that the game composites.
2. **Offline only.** No outbound network in the game path. No JavaScript in the game loop. The game binary never loads `htmx.js`.
3. **Same HTML in Canvas and game.** Designer HTML (plus sample-data schema) is the contract. Canvas may use real `htmx.js` + a sample slice file for authoring; that path is tooling only.
4. **Partial updates.** Named data slices carry versions. Only dirty sections re-render and swap. Layer-1 prove bar: a partial DOM swap while the game runs **without** a full-document relayout.
5. **Hard per-frame paint budget** so Week/Month sim ticks stay protected (align with `FRAME_BUDGET.md`). Built-in timing in the crate.
6. **Layered migration (Mike):**  
   1. renderer + alpha + wgpu frames  
   2. HTML ↔ Rust hooks  
   3. move logic into hooks  
   4. browser-window primitives  
   First spike window is the **time strip**.
7. **Long-term:** crate stays host-agnostic so Mainnet can grow more HTML chrome without baking GameState into the engine. Never drop existing player-facing chrome/content while migrating — replace by proving parity, then cut over.

## Non-goals

- Not a web-server crate. Not a browser. No cookies, fetch, websockets, or localStorage in the game path.
- No JavaScript execution inside the game loop or crate runtime used by the game.
- No dependency on `mainnet_graph`, `graph_bridge`, Mainnet `GameState`, or any Mainnet crate. The host plugs actions and slices.
- No baked Mainnet templates inside the crate. Templates are host-supplied (rendered HTML fragments or host-chosen template engine).
- Crate does **not** own the window stack, Esc routing, modal hold, or pause. Game owns those (`#530`, `#600`/`#603`). Crate draws and reports clicks / focus only.
- Viewport “holes” (clear rectangles where the 3D/globe shows through) are **opaque textures from wgpu** supplied by UI. Game supplies data and draws nothing. The crate composites size + texture only; it does not render the globe.
- No Designer CSS subset or whitelist is maintained. Mike said full CSS is OK. If a locked mock needs CSS that Blitz lacks, that is either a crate bug (Htmx fixes it or works around it) or a trigger for the engine fallback. It is never a reason to restrict Designer.
- Re-deciding product forks here. Crate name and repo home are decided. Engine, templates and license are proposed picks (see **Open decisions**) and lock together with this doc.

## Ownership split

| Role | Owns | Does not own |
|------|------|----------------|
| **Htmx** | Crate, public API, attribute dialect semantics, headless tests, frame-timing hooks, partial-swap prove in crate tests | Mainnet GameState, graph, window stack/Esc, Designer mocks, live chrome wiring |
| **Game** | Typed actions + `declare_actions` list, the command queue that runs `InputOutcome::Action` between ticks, the `actions` slice (Enabled/Disabled+reason), `SliceSource` versions, `FragmentSource` (askama), named versioned **slices contract text** and slice **values** (must match Designer's sample shape), the replay test (A2b), calling `Engine::complete()` from the command queue after each queued command runs or fails, publishing a finished per-slice snapshot at end of tick (async rule), sim/frame loop integration, window stack / Esc / modal hold, greyed-action **reason strings** | HTML look, crate internals, wgpu globe fill |
| **Designer** | HTML contract attributes, mocks for every window (in parallel, not waiting on the time-strip spike), sample-data **shape** (`samples/*.json`: slice names, field types, which `hx-trigger="slice:..."` each fragment listens to) + pre-rendered sample snippets, CSS tokens as vars, Canvas authoring (may use real htmx.js), CSS for the `.hx-request` pending state and `hx-indicator` elements, choosing `hx-sync` / `hx-indicator` per control | Crate code, Game actions |
| **UI** | Wire Mainnet chrome to the crate, paint texture compositing (alpha + clear viewport holes), viewport-hole textures (UI only: draws and composites globe/floor/holes on wgpu), input path (routes input to `Engine::input` and `Action`s to Game's queue; UI-only actions such as open window or tab switch can call `complete()` in the same frame), the wgpu 25→30 bump PR (only once Mike okays it), prove **≥90% look vs mock** on spike windows | Crate API design, slice schema authorship |
| **Input additions** *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | Designer: `hx-trigger` event/filter syntax in mocks. UI: convert egui/winit input to `InputEvent` (buttons, modifiers, keys, repeat), apply any platform mapping (e.g. macOS Ctrl+click), and forward `Unhandled` keys. Game: hotkeys and the Esc order (`#530`) for every `Unhandled` key, and reading `trigger` info (e.g. shift-click = bulk) | Htmx: no platform remapping and no Esc handling in the crate |
| **Graph** | Reviews any new bridge read if a viewport needs one; otherwise out | Fragment engine, HTML, viewport-hole textures (UI-owned) |

## Attribute dialect

Steal HTMX *shape*, not the web-server model. Attributes are data on host HTML; the Rust engine interprets them against named slices and typed actions.

**Strict dialect (Mainnet HTML contract §2, locked by Mike 2026-10-19):** **Only these attributes exist.** Anything else is a **load error**, not ignored. The locked set is exactly: `hx-post`, `hx-target`, `hx-swap`, `hx-trigger`, `hx-sync`, `hx-indicator`. Any other attribute whose name starts with `hx-` or `data-hx-` (including htmx extras such as `hx-get`, `hx-vals`, `hx-on`, `hx-boost`, `hx-select`, and `data-hx-*` aliases of the locked names) makes `load_html` return `EngineError::InvalidAttribute`. **`data-hx-reason` must not appear in loaded HTML or swap fragments** — the engine writes it when greying / failing an action; presence in host HTML is `InvalidAttribute`. The same checks run on **fragments** returned from `FragmentSource` before a swap is applied: a bad fragment makes `sync` return `EngineError` and is **never** silently inserted. For fragments, `hx-target` / `hx-indicator` `#id` refs resolve against the document **after** the swap (fragment ids ∪ remaining document ids, minus ids the swap removes) — so a sibling like `#tstrip-ind` outside `#tstrip-chips` is valid.

| Attribute | Meaning in this crate |
|-----------|------------------------|
| `hx-post="<action>"` | On trigger, the engine returns `InputOutcome::Action` for `<action>` (the host runs it later). The name must have been passed to `declare_actions` first: `load_html` rejects undeclared names, and a click-time check also fails loudly. |
| `hx-target="#id"` | Swap destination: **must** be an id selector `#id` that resolves in the loaded document, or — for fragment HTML — in the **post-swap** document (fragment ids plus the rest of the document, minus whatever the swap removes). Any other selector form, or an id that does not resolve, is a load/`sync` error (`EngineError::InvalidAttribute`). |
| `hx-swap="<strategy>"` | **First swap set (Designer lock, 2026-10-19): `innerHTML` and `outerHTML` only.** An element with **no** `hx-swap` attribute **defaults to `innerHTML`** (same as htmx; keeps Canvas parity — decided 2026-10-19). `beforeend` / `afterbegin` are added only when a mock needs them. Any other or unsupported swap mode **fails at `load_html`** (`EngineError::UnsupportedSwap`), not only at swap time. |
| `hx-trigger="<event>"` | When to fire: at least `click`, plus host-driven slice events (e.g. `slice:time` when the `time` slice version changes). |
| `hx-trigger` events, filters, `from:` *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit); **`change` + form controls locked 2026-10-19 issue #14**)* | **Events** (DOM names): `click` (primary button), `contextmenu` (secondary / right click; there is no native menu to suppress), `auxclick` (middle; optional), `keydown`, `keyup`, **`change`** (commit-only form controls; see below), plus the existing `slice:<name>`. `dblclick` is **deferred** (load error until added). **Several triggers** are comma-separated, as in htmx: `hx-trigger="click, keydown[key=='Enter'] from:body"`. **Filters** in `[...]` come from a fixed whitelist, with no JS eval: `shiftKey`, `ctrlKey`, `altKey`, `metaKey`, `repeat`, `key=='X'` (single-quoted literal), each optionally negated with `!`, combined with `&&` and `\|\|` (`&&` binds tighter; no parentheses). **`from:`** only `from:body` / `from:document` (equivalent; window-level shortcut that fires without focus), and only on `keydown`/`keyup`. **Parsed at `load_html`:** an unknown event, filter, operator or `from:` target is a load error (`EngineError::InvalidTrigger`, like `UndeclaredAction`). See **Input: buttons, modifiers, keyboard**. |
| `hx-trigger="change"` *(issue #14, locked 2026-10-19)* | Valid only on `<input>` (range/number/checkbox/radio/text…), `<select>`, and `<textarea>`. **Takes no filters** (`change[...]` is a load error). Fires **only at a commit point**: range pointer-release (once; that release does **not** also fire `click`), arrow-key step on a focused range/number, checkbox/radio toggle, select pick (or closed-select arrow step). Never mid-drag. Args are the same as a click (`collect_action_args`). Emits `TriggerInfo::Change`. |
| `<output for="id">` *(issue #14)* | After a committed or dragged value change, the engine updates the text of every `<output for="<input id>">` locally (no host round-trip). Format matches the sample (e.g. `55%` for a 0–1 range). |
| Engine-owned checkbox, radio, select *(issue #14)* | Blitz beta.4 does not toggle checkbox/radio through hx-native's input path and has no interactive `<select>`. The engine owns: checkbox toggle (`BaseDocument::toggle_checkbox`), radio exclusivity in a `name` group, label activation (click on `<label>` or its child toggles the wrapped / `for=` control once even when the input is `.vh` / `pointer-events:none`), and a Win95 drop list for `<select>` (closed face + ▼; open list ≤8 rows, no scrolling; navy highlight; Esc or outside click cancels; Esc is **consumed only while open** — the one exception to Input rule 4; closed Esc stays `Unhandled`). Disabled controls refuse drag, toggle, open and step. |
| `hx-indicator="#id"` | While this element's action is pending, the engine adds class `hx-request` to the element `#id` (e.g. a spinner). **Must** be an id selector `#id` that resolves in the document, or — for fragment HTML — in the **post-swap** document (so a sibling indicator outside the swapped subtree is fine). Designer styles it. See **Async actions and pending state**. |
| `hx-sync="drop"` \| `"queue"` | What happens when the trigger fires again while this element's action is pending. **Locked modes:** `drop` (default; repeat triggers are ignored, never a duplicate `Action`) and `queue` (keep one; it fires after the pending one finishes). Any other value **fails at `load_html`** (`EngineError::UnsupportedSync`). |

**Stable ids:** every interactive element has a stable `id` (Designer contract). Tests and Game hooks key off ids, not ephemeral DOM order.

**Greyed / blocked actions:** host publishes `Disabled { reason }` in the `actions` slice, and the engine marks matching `hx-post` elements before any click (see API notes). One plain reason string; the crate does not invent copy.

**Tokens:** visual tokens are CSS custom properties (`var(--…)`) so Canvas and game share one stylesheet language.

**Sample data (schema artifact):** the shape is defined by checked-in example files, starting with `samples/time.strip.json`. Live slices must match that shape, and a **drift test** fails if a live slice and the sample differ in slice names or field types. Designer owns the shape: slice names, field types, and which `hx-trigger="slice:..."` each fragment listens to. Game fills in the values.

**Askama / Canvas rule (Designer):**
- The Canvas **shell HTML contains no askama**: only `hx-*` attributes, stable ids, CSS tokens, and static structure.
- Askama renders **fragments on the Mainnet host only**, from typed slice structs. `hx_native` only ever sees finished HTML strings.
- In Canvas, a fake server (real `htmx.js`) returns **pre-rendered snippet HTML** from the sample pack next to the sample JSON (e.g. `samples/time.strip/*.html` beside `samples/time.strip.json`).
- Askama syntax inside Designer files would need an explicit offline render step. That is not the default.

**Parallel mocks:** Designer's mocks for every window go ahead in parallel. They don't wait on the time-strip spike, which only proves the engine.

## Crate API sketch (illustrative Rust)

Crate `hx_native` (package `hx-native`). Type and function names below are sketches for review; nothing is published yet.

```rust
// crate: hx_native  (use hx_native::{Engine, EngineConfig, ...};)

/// Host hook: renders HTML for one section. The engine calls it ONLY for dirty
/// sections (a subscribed slice's version changed), plus once for every section
/// right after load. Clean sections are never rendered. Mainnet runs askama
/// inside this, from typed slice structs, and returns finished HTML.
pub trait FragmentSource {
    fn render(&mut self, section_id: &str, slices: &[&str]) -> String;
}

/// Host-owned slice versions, read from the PUBLISHED end-of-tick snapshot (never
/// from a tick in progress). The engine calls `version` only for slice names
/// the loaded page subscribes to (`hx-trigger="slice:<name>"`, plus `actions`).
/// Versions are NOT persisted: after `load_html` the engine does one full
/// render, so game saves carry no UI state.
pub trait SliceSource {
    fn version(&self, name: &str) -> Option<u64>;
    /// Only for the reserved `actions` slice (greyed-before-click).
    fn action_states(&self) -> Vec<(String, ActionState)>;
}

/// Published by the host as the reserved `actions` slice.
pub enum ActionState {
    Enabled,
    Disabled { reason: String }, // one plain string; the crate never invents copy
}

/// Engine configuration: offline, no network, no JS.
pub struct EngineConfig {
    pub max_paint_ns: u64, // hard per-frame budget; host aligns with FRAME_BUDGET
    /// Backstop for a missing `complete()`: after this many frames a pending
    /// request is dropped. Debug builds warn; test harnesses fail loudly.
    pub pending_timeout_frames: u32,
}

pub struct Engine { /* … */ }

impl Engine {
    pub fn new(config: EngineConfig) -> Self { /* … */ }

    /// Must be called before `load_html`: every action name the host can run.
    pub fn declare_actions(&mut self, names: &[&str]) { /* … */ }

    /// Load a full HTML document or shell (Designer file). No network fetch.
    /// Errors (EngineError::UndeclaredAction) if any `hx-post` names an action
    /// not passed to `declare_actions`, (EngineError::InvalidTrigger) on any
    /// unknown `hx-trigger` event, filter, operator or `from:` target, and
    /// (EngineError::InvalidAttribute) for any `hx-*`/`data-hx-*` outside the
    /// locked set, for `data-hx-reason` in host HTML, or for `hx-target` /
    /// `hx-indicator` that are not a resolving `#id`. Unsupported `hx-swap` /
    /// `hx-sync` values use UnsupportedSwap / UnsupportedSync. Any of these
    /// fails A1 headless boot.
    pub fn load_html(&mut self, html: &str) -> Result<(), EngineError> { /* … */ }

    /// Polls `version` for subscribed slices, re-renders only dirty sections via
    /// `FragmentSource`, swaps them in, and applies `actions` slice states.
    /// Fragment HTML from `FragmentSource` is validated with the same dialect
    /// rules as `load_html` before each swap; a bad fragment returns
    /// `EngineError` (e.g. `InvalidAttribute`) and is not applied.
    pub fn sync(&mut self, slices: &dyn SliceSource, frags: &mut dyn FragmentSource)
        -> Result<DirtySet, EngineError> { /* … */ }

    /// Deliver pointer/keyboard that the host routed to this surface.
    /// Never calls game code: it only returns outcomes.
    pub fn input(&mut self, ev: InputEvent) -> InputOutcome { /* … */ }

    /// Host reports that the queued command for `request` finished (Ok) or
    /// failed (Err(reason) shows like Blocked). Clears pending state. Never blocks.
    pub fn complete(&mut self, request: RequestId, result: Result<(), Reason>) { /* … */ }

    /// Actions released since the last call (e.g. an `hx-sync="queue"` trigger
    /// let go after `complete`). May be called more than once per frame. See the
    /// per-frame order in the Async section.
    pub fn take_outcomes(&mut self) -> Vec<InputOutcome> { /* … */ }

    /// Paint into a CPU/GPU buffer the host composites. Headless builds may skip GPU.
    pub fn paint(&mut self, viewport: Viewport) -> Result<PaintOutput, EngineError> { /* … */ }

    /// Built-in timing for the last sync/paint (ns). Host asserts against budget.
    pub fn last_timings(&self) -> Timings { /* … */ }
}

/// Deterministic: starts at 1 per engine, +1 per emitted Action (incl. queued
/// releases), never reused, never persisted.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct RequestId(pub u64);

/// One plain host string (the same rule as Disabled/Blocked reasons).
pub struct Reason(pub String);

/// Input the host (UI) converts from egui/winit. Raw: no platform remapping.
pub enum InputEvent {
    PointerMove { x: f32, y: f32, mods: Modifiers },
    PointerDown { x: f32, y: f32, button: PointerButton, mods: Modifiers },
    PointerUp   { x: f32, y: f32, button: PointerButton, mods: Modifiers },
    PointerLeave,
    Key(KeyEvent),
    /// Committed text for a focused text field. Reserved: no text fields in the
    /// first scope (Open decision 7); returned as Unhandled until then.
    Text(String),
}

pub enum PointerButton { Primary, Secondary, Middle }

#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Modifiers { pub shift: bool, pub ctrl: bool, pub alt: bool, pub meta: bool }

pub struct KeyEvent {
    pub key: String,    // DOM `key` value, e.g. "Enter", "Escape", "a", "A"
    pub code: String,   // DOM `code` value, e.g. "KeyA" (layout-independent)
    pub mods: Modifiers,
    pub repeat: bool,   // OS auto-repeat
    pub pressed: bool,  // true = keydown, false = keyup
}

/// What fired an Action/Blocked, so Game can tell e.g. shift-click (bulk) apart.
/// `#[non_exhaustive]` — adding variants (e.g. `Change`) must not break
/// exhaustive matches in Mainnet.
#[non_exhaustive]
pub enum TriggerInfo {
    Pointer { event: &'static str /* "click" | "contextmenu" | "auxclick" */, button: PointerButton, mods: Modifiers },
    Key { event: &'static str /* "keydown" | "keyup" */, key: String, code: String, mods: Modifiers, repeat: bool },
    Change, // commit-only form control (issue #14)
    Slice { name: String },
}

pub struct PaintOutput {
    pub rgba: Option<RgbaImage>,           // headless / software path (G1)
    pub texture: Option<HostTextureRef>, // wgpu path (G2): size + handle only
    pub focus: FocusState,               // so Esc stays in Rust / Game
}

/// Reported in pointer order: no-click -> Hover -> Press -> Clicked/Action.
/// `Hover`/`Press` carry `None` when the pointer leaves / releases off target.
pub enum InputOutcome {
    None,
    Hover { id: Option<String> },
    Press { id: Option<String> },
    Clicked { id: String },
    /// The ONLY action path. The host queues it and runs it between ticks.
    /// `request` is a monotonic per-engine id; host answers with `Engine::complete`.
    Action { request: RequestId, name: String, args: Vec<(String, String)>, source_id: String, trigger: TriggerInfo },
    /// Race fallback: the click landed on an action that was Disabled in the
    /// latest `actions` slice, so no Action is emitted.
    Blocked { name: String, reason: String, source_id: String, trigger: TriggerInfo },
    FocusChanged(FocusState),
    /// A key the engine did not consume (no matching trigger, not Tab/Enter/Space
    /// on a focused control). The host routes it to game hotkeys / the Esc stack.
    Unhandled(KeyEvent),
}

pub struct Viewport {
    pub width: u32,
    pub height: u32,
    /// UI-supplied opaque hole textures (globe/3D). Crate composites rect + texture only.
    pub holes: Vec<Hole>,
}

pub struct Hole {
    pub rect: Rect,
    pub texture: HostTextureRef,
}
```

**Single action path:** there is no action callback. `input` only ever returns `InputOutcome::Action { request, name, args, source_id }`, and returns immediately. Mainnet pushes it onto its own command queue and runs it **between ticks**, in a deterministic order that can be logged and replayed. The engine never calls game code during input handling.

**Greyed before click:** the host publishes the reserved `actions` slice (action name → `Enabled` | `Disabled { reason }`). On each `sync`, every element whose `hx-post` names a Disabled action gets `disabled` + `aria-disabled="true"` plus the reason in the attribute the HTML contract names (proposed: `data-hx-reason`, which Designer can show as a tooltip). Enabling an action removes all three. If the state flips between paint and click, the click yields `InputOutcome::Blocked { reason }` instead of `Action`.

**Determinism:** same HTML + same slices + same input sequence → same HTML tree / same pixel buffer in headless software paint (accepting font raster fuzz only if explicitly documented).

**Templates:** the crate gets HTML only through `FragmentSource::render` (finished strings), and only for dirty sections. The crate does not depend on any template crate. Proposed: `hx_native` takes rendered HTML strings, and Mainnet renders them with askama (see Open decisions).

## Async actions and pending state

*(Reviewed: Game cleared 2026-10-19 with three changes folded in: complete-only clearing, no persistence, deterministic ids. UI cleared 2026-10-19 and locked the per-frame order in rule 7.)* Mike flagged that heavy sim ticks may not apply a click's effect right away. The engine must stay responsive and show that something is in progress, without ever waiting on the game.

1. **Request ids.** Every `InputOutcome::Action` carries `request: RequestId`, a monotonic `u64` per engine. The counter is **deterministic**: it starts at 1 in every engine and goes up by 1 per emitted `Action`, including `hx-sync="queue"` releases. So the same input sequence gives the same ids, and A2b replay is byte-identical. After the queued command runs or fails, the host calls `Engine::complete(request, Ok(()) | Err(Reason))`. The engine never waits: `input()` returns immediately, and nothing in `sync`/`paint` blocks on the game.
2. **Pending marks.** While a request is pending, the engine adds class **`hx-request`** (htmx's class name) and `aria-busy="true"` to the source element. If the source has `hx-indicator="#id"`, the indicator element also gets class `hx-request`. Designer CSS styles both, which keeps Canvas (real htmx.js uses the same class) and the game identical.
3. **`hx-sync`.** Locked modes are `drop` (default) and `queue`.
   - `drop`: repeat triggers on the same element while it's pending are ignored, and no duplicate `Action` is ever emitted.
   - `queue`: holds at most one repeat. It's released as a new `Action` (new `RequestId`) through `take_outcomes()` once the pending one clears.
4. **When pending clears.** Pending clears **only** on `complete(request, …)`. Slice bumps never clear it, because subscribed slices like `time` bump every tick. **Backstop:** `EngineConfig::pending_timeout_frames`. If a request is still pending after that many frames, the engine drops it: debug builds log a warning, and test harnesses fail loudly. A late `complete` is a no-op **only** for a request already dropped by that timeout. Any other `complete` for an unknown id is a host bug (debug warn, test fail). `complete(request, Err(reason))` applies the reason exactly like `Blocked`: the same reason attribute and the same plain string, then it clears.
5. **Host snapshot rule.** `SliceSource` reads a **published, finished snapshot** per slice, double-buffered and published at end of tick. So `sync`/`paint` never block on a tick in progress and never tear with one. UI-only actions (open window, tab switch) don't need the sim, so UI may call `complete()` in the same frame.
6. **No persistence.** Request ids and pending state are never saved. `load_html` drops all pending requests as part of its one full render, and the counter restarts at 1 in a new engine.
7. **Per-frame order (UI-locked, 2026-10-19):** `input()` → enqueue its `Action`s on Game's queue → `complete()` for any UI-only actions → `take_outcomes()` (forward released `hx-sync="queue"` Actions to Game's queue) → `sync` → `paint`. `take_outcomes()` may be called more than once per frame (e.g. again after `complete()` calls); each call returns only what was released since the previous call.

## Input: buttons, modifiers, keyboard

*(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))*. Mike asked (2026-10-19, before lock) for right click, shift/ctrl click and keyboard input.

1. **Pointer buttons.** `InputEvent::PointerDown/Up` carry `button` (`Primary` / `Secondary` / `Middle`) and `mods`. Primary press+release on the same element is `click`. Secondary is `contextmenu`, Middle is `auxclick`. The Hover → Press → Clicked order (A3b) is unchanged for every button.
2. **Modifiers and trigger info.** Every `Action`/`Blocked` carries `trigger: TriggerInfo` (button + modifiers, or key/code/modifiers/repeat, or the slice name). That way Game can map e.g. shift-click to "bulk / queue ×5" without a second action name. A Designer may also split it into two actions with filters (`click[!shiftKey]` vs `click[shiftKey]`). Splitting is **required** when the two can differ in availability (rule 10).
3. **Raw reporting, host maps platform.** The engine reports exactly what it gets and does **no** platform remapping. In particular, **macOS Ctrl+Primary is reported as Primary with `ctrl`**. If Mainnet wants the macOS "Ctrl+click = right click" convention, UI converts it to `Secondary` before calling `input()` (Open decision 8). **Cmd vs Ctrl** (Mike is on macOS): there is no `cmdOrCtrl` filter, so that htmx.js stays compatible. Designer writes `click[ctrlKey||metaKey]` (or two comma-separated triggers).
4. **Keyboard focus and routing.** The order of precedence for a key event:
   - (a) The **focused element** gets it first. A matching `keydown`/`keyup` trigger on it (or its ancestors, DOM bubbling) consumes it.
   - (b) Otherwise **Tab / Shift+Tab** move focus through the focus order (Blitz `focus_next_node`/`focus_prev_node`) and emit `FocusChanged`.
   - (c) Otherwise **Enter or Space on a focused button** (or element with `hx-post`) activates it exactly like a primary `click`: same Action, same pending/`hx-sync` rules, with `TriggerInfo::Key`.
   - (d) Otherwise `from:body`/`from:document` triggers whose filter matches consume it, **even with no focus**. **Deterministic tie-break (Designer time-strip Space):** walk matching elements in **document order**. Emit `Action` for the **first ENABLED** match (action not `Disabled` in the `actions` slice). **Skip** greyed matches. If **every** match is greyed, return `Blocked` with the **first** greyed match's reason (and that element's `source_id`). Example: `#tstrip-pause` (`time.pause`) and `#tstrip-resume` (`time.resume`) both declare `hx-trigger="click, keydown[key==' '&&!repeat] from:body"`; exactly one is greyed at a time, so Space with no focus always fires the enabled one.
   - (e) Otherwise the engine returns **`InputOutcome::Unhandled(key)`** and the host routes it to game hotkeys.
   - **The engine never swallows Esc** unless an element has an explicit matching trigger (e.g. `keydown[key=='Escape']`), **or an engine-owned `<select>` drop list is open** (issue #14: Esc closes the list without changing the value and is consumed; when the list is closed, Esc stays `Unhandled`). By default, Esc is `Unhandled`, so Game's `#530` Win95 order (close top inner dialog first, then building) stays in Rust.
5. **Key repeat.** Default: `keydown` triggers **do not fire on OS auto-repeat**. A trigger opts in with the filter `[repeat]`, which fires on repeats only; use `[repeat||!repeat]` for both. Unconsumed repeats are still returned as `Unhandled`, so Game hotkeys can repeat. **Canvas note:** htmx.js fires `keydown` on repeats, so mocks that care should write `[!repeat]` explicitly. That way both behave identically.
6. **Pending and `hx-sync` apply identically to all trigger kinds.** A key, right click or modified click goes through the same `RequestId`, `hx-request`/`aria-busy`, `hx-indicator`, `drop`/`queue` and `complete()` rules as a primary click.
7. **Text input.** `InputEvent::Text` is reserved. The first scope has no text fields, so `Text` comes back as `Unhandled` (Open decision 7).
8. **Canvas parity.** htmx.js evaluates trigger filters as **JavaScript** against the DOM event. Our whitelist (`shiftKey`, `ctrlKey`, `altKey`, `metaKey`, `repeat`, `key=='X'`, `!`, `&&`, `||`) is a strict subset of valid JS on DOM `KeyboardEvent`/`MouseEvent` properties, and `from:body`/`from:document` and the event names mean the same in htmx. So a Canvas mock with real htmx.js behaves identically. Two known differences: (i) in a browser, `contextmenu` also opens the native menu unless the mock prevents it (the game has none); (ii) the repeat default (rule 5).

9. **Element-level Esc still goes through Game (Game, 2026-10-19).** A mock's `keydown[key=='Escape']` trigger only makes the engine enqueue an `Action` on Game's queue. Game runs it only if that window is top of the stack. Otherwise Game does the normal `#530` close of the top dialog. The engine never reorders close behaviour. Game tests this on its side, and A20 is unchanged.
10. **Greying is per action name (Game, 2026-10-19).** The `actions` slice greys by action **name**. If shift-click and plain click can ever differ in availability, they must be **two action names**, split with filters (`click[!shiftKey]` → `x`, `click[shiftKey]` → `x.bulk`). Use one name plus `trigger` info only when availability is always identical. A2b already records `trigger`.

## Mainnet integration seams

1. **Chrome shell HTML** from Designer (same file Canvas uses). First spike: time strip replacing egui `time_controls` face (date, speed chips, pause affordance) without removing pause/menu behaviour owned by Game/UI.
2. **Slices** (Game-owned contract text), examples for the spike: `time` (date, speed, paused), later `hud`, desk bodies, etc. Game implements `SliceSource::version` (asked only for subscribed names) and `FragmentSource::render` (askama, dirty sections only). Version bumps drive `hx-trigger` slice events. Versions are not saved; after load the engine does one full render.
3. **Actions** (Game): e.g. `time.set_speed`, `time.pause_menu` (names illustrative). Game calls `declare_actions` before `load_html`. `InputOutcome::Action` goes onto Mainnet's command queue and runs between ticks; the engine never calls game code. Game publishes the `actions` slice for greying, calls `complete(request, …)` from the command queue after each command runs or fails, and publishes slice snapshots at end of tick (Async section). Undeclared action → load error (A1); unknown at click → test failure (A4).
4. **UI wiring:** create engine. Convert egui/winit pointer and key events to `InputEvent` (button, modifiers, key/code, repeat, pressed), raw except for any platform mapping UI owns (Open decision 8). Route every `Unhandled` key to Game hotkeys / the Esc stack *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))*. **Locked per-frame order:** `input()` → enqueue its `Action`s on Game's queue → `complete()` for any UI-only actions → `take_outcomes()` (forward released `hx-sync="queue"` Actions to Game's queue) → `sync` → `paint` → composite texture with alpha. `take_outcomes()` may be called more than once per frame; clear/fill viewport holes with UI-owned wgpu textures (Game supplies data only); route pointer through no-click → hover → press; report focus so Esc / `#530` stay in Rust.
5. **Look prove:** UI shows ≥90% match to Designer mock for the spike window at contract sizes (including 1280×720 from `UI_SCREEN_CONTRACT`).
6. **No graph_bridge / mainnet_graph** in the crate. Holes are host textures.
7. **Migration order:** do not skip to browser-window primitives before renderer+alpha+wgpu and HTML↔Rust hooks are proven on the time strip.

## Performance budget

- Honor `docs/design/FRAME_BUDGET.md`: sustained ≥40 FPS floor (ideal ≥60); input→chrome on the next drawn frame; Week/Month sim work must not block paint.
- Crate exposes `max_paint_ns` + `last_timings()`. Host fails CI/playprove if chrome paint blows the budget on the spike path.
- Partial swap must **not** force full-page relayout. Dirty slice → dirty subtree only.
- Headless tests run without GPU; optional wgpu path for Mainnet composite.

### Layer-1 gate G1: CPU raster + upload (wgpu-version independent)

Mainnet is on **wgpu 25** (its `Cargo.toml`). Blitz/Vello use **wgpu 30**. So the Layer-1 spike **does not share a GPU device**:
- `hx_native` paints with **`vello_cpu`** (CPU raster, RGBA with alpha) and hands the host the RGBA buffer plus its dirty size.
- UI uploads that buffer into a **wgpu 25 texture** on Mainnet's own device (`queue.write_texture`) and composites it with alpha.
- Neither side needs wgpu 30 in Mainnet's dependency tree, so `hx_native`'s CPU path must not bring in a `wgpu` dependency.
- **G1 passes** when A8/A14 hold on this path: CPU paint time plus upload stays within `max_paint_ns` / `FRAME_BUDGET` on the time strip.

**G2 (later, gated):** the shared-device GPU path (Vello `render_to_texture` straight onto the game's device and texture) only happens after Mainnet bumps to **wgpu 30**. That is a **separate PR owned by UI**, done only once Mike says yes (Open decision 6). Until then, the GPU path is out of scope.

### Risks

| Risk | Mitigation |
|------|------------|
| wgpu 25 (Mainnet) vs wgpu 30 (Blitz/Vello) blocks device sharing | G1 CPU raster + upload. G2 waits on Mike's yes and UI's bump PR. Keep `wgpu` out of `hx_native`'s default features. |
| CPU raster + upload of a large chrome surface costs too much each frame | Paint only when something is damaged. Upload only the dirty rect. Port ps-blitz paint damage. Measure in G1 before scaling past the time strip. |
| Upstream Blitz re-encodes the whole scene every paint | Same as above: skip clean frames, add dirty-region paint. The engine fallback stays available if the budget is still missed. |
| Blitz is beta, with exact `=0.3.0-beta.4` pins across its crates and Rust 1.91 / edition 2024 | Pin the version, bump deliberately, and fork at the tag if needed. |
| Determinism (Stylo's parallel styling pool, system fonts) | Sequential styling and bundled fonts in tests. |
| Heavy ticks delay a click's effect (Mike) | Async pending state: `hx-request`/`aria-busy`/`hx-indicator` show progress right away. `hx-sync="drop"` stops duplicate commands. Pending clears only on `complete()`; `pending_timeout_frames` is the backstop. Snapshots keep `sync`/`paint` non-blocking. |
| Host forgets `complete()` and an element stays pending | `pending_timeout_frames` drops it: debug builds warn, tests fail loudly (A3c). Game's queue always calls `complete` on both success and failure. |
| Engine swallows a key Game needs (Esc, hotkeys) *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | Strict precedence (Input rule 4). Esc is never consumed by default. Every unconsumed key is returned as `Unhandled`. A20 covers it. |
| Filter whitelist drifts from htmx.js semantics | The whitelist is a strict JS subset. A23 runs the Canvas parity check. Anything outside it is a load error, never silently ignored. |
| Action latency from queue-between-ticks (click runs a tick later) | Run the command queue at the start of the next frame/tick, and show press feedback straight away from Press/Hover. Measure input→chrome per `FRAME_BUDGET`. |
| `actions` slice gets stale (greyed state lags game) | `Blocked` race fallback (A6b). Game bumps the `actions` version whenever an availability changes. |
| A mock needs CSS that Blitz lacks | Treat it as a crate bug or an engine-fallback trigger (see Non-goals). Never a Designer CSS restriction. |

### Render-engine facts (Blitz — verify, do not invent)

Candidate surface: [DioxusLabs/blitz](https://github.com/DioxusLabs/blitz) (modular HTML/CSS engine; uses [Stylo](https://github.com/servo/stylo) for CSS).

| Fact | Source |
|------|--------|
| Status: **beta**; many no-JS sites render; bugs/missing features remain | [Blitz README](https://github.com/dioxuslabs/blitz/blob/main/README.md) |
| WGPU texture integration example: `cargo run -rp wgpu_texture` | same README |
| Incremental **layout** damage / optional incremental resolve | Blitz docs: damage flags + `incremental_layout` ([DeepWiki resolve cycle](https://deepwiki.com/dioxuslabs/blitz/2.1.3-document-resolution-and-update-cycle)) |
| Upstream Blitz repaints the whole scene every frame (`blitz-paint::paint_scene` re-encodes all draw commands; only paint-tree topology is cached). **Paint-damage tracking is not upstream**; it exists in the `ps-blitz` fork (`ps-blitz-dom` `src/paint_damage.rs`, MIT/Apache), which we can port | [blitz-paint](https://github.com/DioxusLabs/blitz/tree/main/packages/blitz-paint), [ps-blitz](https://github.com/pathscale/ps-blitz); see `research/CRATE_SURVEY.md` §2 |
| CSS variables / modern layout are intentional goals | Blitz README Goals |

**Implication for Layer-1 prove:** partial *DOM/layout* incrementalism is in-tree; full dirty-region *paint* must be explicitly enabled/verified. Spike must measure real paint ns, not assume browser-like dirty rects. Alternatives remain an Open decision if Blitz cannot meet the budget on the time strip.

## Acceptance tests

Numbered so agents can tick them on the GitHub issue once this doc is locked.

| ID | Test | Pass bar |
|----|------|----------|
| **A1** | Headless boot | `declare_actions` then `load_html` loads Designer HTML + sample slices with **no GPU, no network, no JS**. Any `hx-post` naming an undeclared action makes `load_html` error, and the test fails. |
| **A2** | Determinism | Fixed HTML + slices + input → identical serialized HTML (and software RGBA within documented tolerance). |
| **A2b** | Replay (Game-owned) | Record a session's `InputOutcome::Action` stream (as queued between ticks), replay it into a headless game → same game state **and** byte-identical HTML, including identical `RequestId`s (deterministic counter). HTML is compared only after **all requests have completed** (no leftover `hx-request` / `aria-busy`). |
| **A3b** | Hover → press → click order | Headless pointer script (move onto control → down → up) reports `Hover{id}` → `Press{id}` → `Clicked{id}`/`Action` in that order. Moving off reports `Hover{None}`. Releasing off target reports `Press{None}` and no click. ids are stable `id`s. |
| **A3** | Stable ids | Every interactive control in the time-strip contract has a stable `id`; click reports that id. |
| **A3c** | Pending lifecycle | Headless: click → `Action{request}` → source has class `hx-request` + `aria-busy="true"` (and the `hx-indicator` target has `hx-request`) → `complete(request, Ok)` → all cleared. Bumping a subscribed slice (e.g. `time` every tick) does **not** clear pending. Timeout case: with no `complete`, after `pending_timeout_frames` the harness fails loudly (asserted with an expect-failure test), and a later `complete` for that id is a no-op. `input()` never blocks. |
| **A3d** | `hx-sync` + failure | `drop`: a double click while pending yields exactly **one** `Action`. `queue`: a double click yields one `Action` now and one more from `take_outcomes()` after `complete`, with the next `RequestId`. `complete(request, Err(reason))` shows that exact reason like `Blocked`, then clears pending. An unknown `hx-sync` value fails loudly. |
| **A17** | Right click *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | `hx-trigger="contextmenu"` + Secondary down/up → `Action` with `TriggerInfo::Pointer { event: "contextmenu", button: Secondary, .. }`. A primary click on the same element emits nothing. |
| **A18** | Modifier filters *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | `click[shiftKey]` fires only with `mods.shift`. `click[!shiftKey]` fires only without it. `click[ctrlKey]` vs `click[metaKey]` each fire only for their own modifier. `click[ctrlKey\|\|metaKey]` fires for either. `trigger.mods` matches the input. macOS Ctrl+Primary is reported raw as Primary + `ctrl`. |
| **A19** | Window-level key *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | `keydown[key=='Enter'] from:body` fires with **no focused element**. Without `from:body`, it fires only when the element (or a descendant) has focus. An auto-repeat keydown does not fire unless the filter has `repeat`. |
| **A20** | Esc passthrough *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | Esc with no matching trigger → `InputOutcome::Unhandled(Escape)` (never consumed). With `keydown[key=='Escape']` on the focused element → consumed, `Action`. Game's `#530` order is tested against the `Unhandled` path. |
| **A21** | Keyboard activation *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | Tab/Shift+Tab move focus in order with `FocusChanged`. Enter and Space on a focused `hx-post` button yield the same `Action` as a primary click (with `TriggerInfo::Key`), with the same pending/`hx-sync` behaviour (A3c/A3d). |
| **A22** | Invalid trigger fails load *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* | `load_html` returns `InvalidTrigger` for: an unknown event (e.g. `dblclick` while deferred, `mouseover`), an unknown filter (`[event.foo]`, `[shiftKey==1]`, parentheses), `from:#id`, or `from:body` on `click`. A1 fails on it. |
| **A23** | Canvas trigger parity *(locked in scope; Designer and Game reviews done 2026-10-19; pending seat review: UI (input wiring from egui/winit))* (owner: Designer) | The same mock with real htmx.js: each whitelisted filter and trigger in A17–A21 fires on the same input in Canvas as in the headless engine (manual/Playwright check by Designer). Known differences (native context menu, repeat default) are listed in Input rule 8. |
| **A24** | `change` trigger *(issue #14)* | `hx-trigger="change"` on range/checkbox/radio/select: fires once on range release (no click on that release), on arrow step, on toggle, on select pick; never mid-drag. Args include the enclosing form. `TriggerInfo::Change`. Disabled control is a no-op. `change[...]` and `change` on a `<div>` fail load (`InvalidTrigger`). `<output for>` updates locally. Label click (wrapped or `for=`) toggles once even with `.vh` / `pointer-events:none`. After toggle, `:checked` styles (e.g. `input:checked + i`) repaint in the same frame with damage covering the restyled node; partial paint is byte-identical to full. Worker matches sync engine. |
| **A25** | Engine `<select>` *(issue #14)* | Plain `<select>`/`<option>`: closed face shows selected text + ▼; click opens ≤8-row list; row click / Enter picks and fires `change`; Esc or outside click cancels; Esc consumed only while open; closed ArrowUp/Down step + `change`; open list is in damage rects / paints above content. |
| **Form args (K1 / #14)** | Multi-value + trigger `name`/`value` | Documented contract (not tests-only): `Action.args` stays `Vec<(String, String)>`. Repeated checkbox names (e.g. several `name="state"`) each contribute a pair in document order; unchecked boxes contribute nothing. `args_get` returns the first value; `args_get_all` returns every value. Any triggering element with `name=`/`value=` (button, `<tr>`, …) contributes that pair first; if the enclosing form also has that name, **both** appear (trigger first — htmx parity). |
| **A4** | Dialect: click → action | `hx-post` + `hx-trigger="click"` returns `InputOutcome::Action { name, args, source_id }`. The engine calls no host code during `input`. The click-time check still fails loudly on an unknown name. |
| **A5** | Dialect: slice → swap | Bumping one slice version re-renders only the dirty target (`hx-target` + `hx-swap` `innerHTML` and `outerHTML`); assert **no full-document relayout** (instrument layout roots / damage flags). |
| **A5b** | Unsupported swap fails | Any `hx-swap` value outside the locked set (today `innerHTML`, `outerHTML`) fails the test loudly. |
| **A5c** | Clean sections never render | A spy `FragmentSource` records calls. After the initial full render, bumping slice `a` calls `render` only for sections subscribed to `a`. A section with no dirty subscription gets **zero** calls. `version` is only asked for subscribed names. |
| **A5d** | No persisted UI state | Reload the page → one full render of every section. Restoring a save restores no engine/UI state: versions, request ids and pending state aren't saved. A load with requests pending drops them all during the full render; no `hx-request`/`aria-busy` remains. |
| **A15** | Sample drift | Live `time` slice(s) from Game match the shape of `samples/time.strip.json` (slice names, field types, `slice:` trigger names). Any mismatch fails. |
| **A16** | No askama in Canvas shell | The Canvas shell HTML contains no askama/Jinja syntax (`{{`, `{%`). Canvas fake server serves the pre-rendered sample snippets. |
| **A6** | Greyed before click | With `actions` slice `X: Disabled { reason }`, after `sync` every `[hx-post="X"]` has `disabled`, `aria-disabled="true"` and the contract reason attribute equal to that exact string, **before any click**. Flipping to `Enabled` clears all three. |
| **A6b** | Blocked race fallback | Disable an action between paint and click → the click yields `Blocked { reason }`, not `Action`, and UI shows that one plain string. |
| **A7** | Focus → Esc seam | Engine reports focus; Game still owns Esc stack (`#530`); crate does not close windows itself. Unconsumed Esc comes back as `Unhandled` (see A20). |
| **A8** | Paint budget | `last_timings().paint_ns` (+ upload time on G1) ≤ configured `max_paint_ns` on the time-strip spike, both headless and on the Mainnet G1 path (`vello_cpu` → wgpu 25 texture upload). |
| **A9** | Hole composite | UI-supplied hole texture appears in the clear rect; crate does not call graph_bridge. |
| **A10** | Resize / reflow | Contract widths (incl. 1280×720) reflow without clipping the time chip (per `UI_SCREEN_CONTRACT`). |
| **A11** | Canvas parity path | Same HTML file works in Canvas with sample slice file; game path never loads `htmx.js`. |
| **A12** | Look parity | UI prove ≥90% vs Designer mock for the time strip (screenshots on thread). |
| **A13** | No Mainnet deps | `cargo tree` / crate manifest has no `mainnet`, `mainnet_graph`, or `graph_bridge`. |
| **A14** | Layer-1 prove in-game | Partial DOM swap while game runs (time strip ticking) without full-document relayout; Week/Month tick still frame-safe per `FRAME_BUDGET`. |

## Open decisions (product forks for Mike)

Evidence for every pick below: `research/CRATE_SURVEY.md` (2026-10-19). Mike said he's not opinionated on items 3–5, so Htmx's recommendations are the proposed picks, and they lock when this doc locks.

1. **Crate name**: **DECIDED (Mike, 2026-10-19):** `hx-native`, Rust ident `hx_native`. Re-checked 2026-10-19: both `hx-native` and `hx_native` return 404 on crates.io (free), and `MHHukiewitz/hx-native` doesn't exist yet.
2. **Repo home**: **DECIDED (Mike, 2026-10-19):** its own public GitHub repo, `github.com/MHHukiewitz/hx-native` (same pattern as [dued](https://github.com/MHHukiewitz/dued)). Not created yet.
3. **Render engine**: **Proposed, Mike deferred to Htmx; locks with the doc.** Blitz (`blitz-dom` + `blitz-html` + `blitz-paint`, pinned `=0.3.0-beta.4`, no `blitz-net`, bundled fonts, sequential styling) + anyrender/Vello: `vello_cpu` for headless tests, and `vello_cpu` → wgpu 25 texture upload on Mainnet for Layer 1 (gate G1). Vello `render_to_texture` on the game device only after the wgpu 30 bump (G2). Reason: it's the only permissive, JS-free engine with damage-driven incremental relayout, hit-testing, and wgpu texture holes. *Fallback:* fork Blitz at that tag and own the paint layer (port ps-blitz paint damage). Last resort: litehtml (BSD-3, C++) on the CPU.
4. **Template engine**: **Proposed, Mike deferred to Htmx; locks with the doc.** `hx_native` takes rendered HTML strings and depends on no template crate. Mainnet renders them with **askama** (compile-time, typed, deterministic). *Fallback:* minijinja (runtime-loaded files, `preserve_order`).
5. **License**: **Proposed, Mike deferred to Htmx; locks with the doc.** `MIT OR Apache-2.0` (dual). Stylo/cssparser (MPL-2.0) are dependencies only, never copied. Any MPL file that does get copied keeps its MPL header under `vendor/mpl/`.
6. **Open: Mike yes/no on bumping Mainnet wgpu 25 → 30.** Mainnet is on wgpu 25 and Blitz/Vello are on wgpu 30. **Layer 1 does not depend on this answer** (G1 uses CPU raster + upload). A yes allows a separate UI-owned PR that bumps Mainnet to wgpu 30, which unlocks G2 (shared-device GPU paint). A no means `hx_native` stays on the G1 path.

7. **RESOLVED by default at lock (Mike, 2026-10-19): no typed text in the spike.** `InputEvent::Text` is reserved, but no text fields are in the first scope. Do any spike windows need typed text (e.g. a search or name field)? If yes, add IME, caret and selection handling (Blitz has text-input support) as a later unit. The default stands, so `Text` returns `Unhandled`. Reopen only when a window needs a text field.
8. **RESOLVED by default at lock (Mike, 2026-10-19): no macOS Ctrl+click remap.** The engine reports raw (Primary + `ctrl`). Should UI map macOS Ctrl+Primary to `Secondary` (the macOS right-click convention)? **Htmx recommends no mapping by default**: two-finger/trackpad right click already arrives as Secondary, and a raw Ctrl+click stays available to `click[ctrlKey]` triggers. The default stands: UI does not remap, and Ctrl+Primary arrives raw.
9. **Open (Designer): `dblclick`.** Deferred (load error) until a mock needs it. It needs a double-click time threshold, which UI would supply from the OS setting.

Locked 2026-10-19 12:51 (UTC-3). Crate code starts with Layer 1 (time-strip spike, gate G1) in the `hx-native` repo. Open items still standing: 6 (wgpu 30 bump, Mike yes/no) and 9 (`dblclick`, deferred).

---

## Context appendix (research, 2026-10-19)

Not normative — breadcrumbs for reviewers:

| Artifact | One line |
|----------|----------|
| No prior locked fragment-engine requirements doc found on box or in Mainnet issues/files | Search covered `/workspace`, `/home/box`, `/cursor/stores`, GitHub code/issues for htmx/Blitz/fragment/paint-to-texture |
| Repo | `MHHukiewitz/Mainnet` (private) |
| Time strip today | `src/ui/time_controls.rs` (egui chips + date); PR `#603` “modal hold + full-width time strip” |
| Esc | Issue `#530` (open, owner:ui) — game-owned stack |
| Designer HTML contract | `docs/design/UI_SCREEN_CONTRACT.md`; mocks under `/workspace/mocks/**/*.html` |
| Frame budget | `docs/design/FRAME_BUDGET.md` / `/workspace/briefs/2026-09-22-frame-budget.md` |
| Own-repo precedent | `MHHukiewitz/dued` |
| HTML chrome migration issues dated 2026-10-19 | **None found** on GitHub at research time — this draft is the starting SoT for the Floor to attach to a new issue |
