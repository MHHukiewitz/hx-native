//! Core `Engine`: surfaces, load, sync, input, complete, paint.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use blitz_dom::{
    local_name, ns, qual_name, DocumentConfig, LocalName, NodeId, QualName, StyleThreading,
    UserAgentStylesheet,
};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};
use style::invalidation::element::restyle_hints::RestyleHint;

use crate::damage::{
    collect_style_damaged, compute_raw_damage, element_border_box_css, ink_box_device,
    ink_damage_rect, merge_damage_rects, snap_outward, snapshot_bounds, BoundsSnapshot, DamagePlan,
    IntRect,
};
use crate::dialect::{validate_loaded_attrs, ElementBinding, SwapMode, SyncMode};
use crate::ellipsis::{prepare_ellipsis_for_paint, EllipsisCache, EllipsisShaper};
use crate::error::EngineError;
use crate::fields::{align_text_inputs, paint_field_bevels};
use crate::focus::{focusable_ids, ids_in_document_order, step_focus};
use crate::fonts::{bundled_font_context, HostFonts};
use crate::form_args::{
    capture_applied_controls, capture_form_controls, clamp_snap_value, clamp_value, closest_form,
    collect_action_args, collect_applied_control_nids, control_current_value,
    control_value_in_args, format_numeric, is_number_input, is_numeric_input, is_range_input,
    parse_f64, patch_control_arg, range_page, range_page_toward_x, range_to_end,
    set_range_value_from_x, step_numeric_value, value_control_kind, write_value_attr,
    NumericLimits, MAX_SAFE_NUMBER,
};
use crate::form_controls::{
    activate_checkable, checkbox_checked_by_name, control_is_disabled, form_control_at_point,
    named_controls_in_form, radios_in_group, select_options, select_radio, select_set_index,
    select_step, sync_outputs_for, toggle_checkbox, FormControlKind, FormControlRef,
};
use crate::memory::estimate_surface_engine_bytes;
use crate::paint::{paint_damage_into, paint_rgba};
use crate::pending::{CatchUp, PendingEntry, PendingState, PendingTimeout, QueuedRelease};
use crate::scroll::{
    all_bars, author_padding_right_css, bar_geometry, bar_hit, bar_unoccluded, can_scroll,
    container_damage_rect, containers_at, gutter_css, is_scroll_container, max_offset_y,
    offset_y as scroll_offset_y, padding_box_css as scroll_padding_box, page_px, paint_scrollbars,
    scroll_ancestors, scroll_containers, set_offset_y, visible_element_from_point, wants_gutter,
    BarGeom, BarPart, PressedArrow,
};
use crate::select_ui::{
    collect_select_nids, control_rect_of, list_row_at, open_select_state, paint_select_overlays,
    select_damage_rects, select_row_damage_rect, select_ua_stylesheet, OpenSelect, SELECT_MAX_ROWS,
};
use crate::slider::{
    all_sliders, grab_offset, paint_sliders, slider_geometry, slider_ua_stylesheet, SliderGeom,
};
use crate::text_edit::{
    apply_group_display_all, apply_unfocused_display, backspace, caret_device_rect, commit_field,
    delete_forward, delete_selection, editor_text, filter_number_insert, has_selection,
    insert_text, is_disabled, is_editable_input, move_end, move_home, move_left, move_right,
    number_is_sent, place_caret_at, prepare_focus_display, resolve_step_target, select_all,
    select_word_at, selected_text, set_editor_text, step_and_regroup, step_direction,
    sync_value_from_editor, text_outside_selection, TextEditSession, CARET_BLINK_HALF_PERIOD,
};
use crate::trigger::{
    filter_ctx_from_key, keydown_repeat_allowed, FilterCtx, FromTarget, TriggerEvent,
};
use crate::types::*;

/// Set of section ids that were dirty (re-rendered) during the last `sync`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirtySet {
    pub sections: Vec<String>,
    /// Nodes that carried layout damage after the swap mutation (before resolve),
    /// used by A5 to assert no full-document relayout.
    pub damaged_node_count: usize,
    pub total_element_count: usize,
}

impl DirtySet {
    fn absorb(&mut self, other: DirtySet) {
        self.sections.extend(other.sections);
        self.damaged_node_count += other.damaged_node_count;
        self.total_element_count += other.total_element_count;
    }
}

static NO_FOCUS: FocusState = FocusState { focused_id: None };

/// Issue #15 (Graph requirement): class the engine adds to a named form
/// control while its current value differs from its `RequiresChange`
/// baseline, removed once it matches again. Host classes are preserved.
const HX_CHANGED_CLASS: &str = "hx-changed";

/// Engine-wide RequestId counter plus the surface that owns each in-flight request.
#[derive(Debug)]
struct RequestLedger {
    next: u64,
    owners: HashMap<RequestId, SurfaceId>,
}

impl RequestLedger {
    fn issue(&mut self, surface: SurfaceId) -> RequestId {
        let request = RequestId(self.next);
        self.next += 1;
        self.owners.insert(request, surface);
        request
    }

    fn forget_surface(&mut self, surface: SurfaceId) {
        self.owners.retain(|_, owner| *owner != surface);
    }
}

/// Engine state a surface needs while handling input or completions.
struct ActionCtx<'a> {
    surface: SurfaceId,
    /// [`EngineConfig::key_releases`] (issue #81).
    key_releases: bool,
    declared_actions: &'a HashSet<String>,
    requests: &'a mut RequestLedger,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct SurfaceSize {
    width: u32,
    height: u32,
    scale: f64,
}

impl SurfaceSize {
    fn from_viewport(viewport: &Viewport) -> Self {
        Self {
            width: viewport.width,
            height: viewport.height,
            scale: if viewport.scale > 0.0 {
                viewport.scale
            } else {
                1.0
            },
        }
    }
}

impl Default for SurfaceSize {
    fn default() -> Self {
        Self {
            width: DEFAULT_SURFACE_WIDTH,
            height: DEFAULT_SURFACE_HEIGHT,
            scale: 1.0,
        }
    }
}

/// One host surface. `live` is `None` once evicted; the metadata here is kept so
/// `set_visible(true)` can rebuild it.
struct Surface {
    source_html: String,
    size: SurfaceSize,
    visible: bool,
    pinned: bool,
    /// LRU stamp from `Engine::lru_clock`.
    last_used: u64,
    /// Next visible paint must report full-surface damage (host dropped its texture).
    needs_full_host_upload: bool,
    live: Option<LiveSurface>,
}

impl Surface {
    fn estimated_bytes(&self) -> u64 {
        if self.live.is_some() {
            estimate_surface_engine_bytes(self.size.width, self.size.height, &self.source_html)
        } else {
            0
        }
    }

    fn is_evictable(&self) -> bool {
        !self.visible
            && !self.pinned
            && self
                .live
                .as_ref()
                .is_some_and(|live| !live.has_pending_work())
    }

    fn counts_against_hidden_budget(&self) -> bool {
        !self.visible && !self.pinned && self.live.is_some()
    }
}

/// Parsed document plus all per-document interaction and paint state.
struct LiveSurface {
    doc: HtmlDocument,
    /// Issue #28: this engine's validated host fonts (shared Blobs), used by
    /// the ellipsis shaper so truncation measures with the same faces.
    fonts: Arc<HostFonts>,
    bindings: HashMap<String, ElementBinding>,
    /// section_id -> subscribed slice names
    sections: HashMap<String, Vec<String>>,
    /// last seen slice versions (subscribed only)
    slice_versions: HashMap<String, u64>,
    /// action name -> Disabled reason (from actions slice, folded with any
    /// currently-dirty `RequiresChange` reasons; issue #15)
    disabled_actions: HashMap<String, String>,
    /// Last `action_states()` poll, cached so a value-edit recompute (no Game
    /// round trip) can re-evaluate `RequiresChange` without re-polling slices.
    last_action_states: Vec<(String, ActionState)>,
    /// Issue #15: per `<form id>` fallback baseline for `RequiresChange` /
    /// `hx-changed`, captured on first sight and whenever a swap replaces the
    /// form or its controls. A control carrying `data-hx-applied` overrides
    /// its own entry here at comparison time (Designer lock 2026-10-20).
    form_baselines: HashMap<String, Vec<(String, String)>>,
    /// Issue #24 (Designer lock 2026-10-20): node ids of every control
    /// carrying `data-hx-applied`, anywhere in the document (any form, or
    /// none) — the attribute alone opts a control into `hx-changed`,
    /// independent of `RequiresChange`. Refreshed on reindex (load / swap),
    /// not per edit (cost rule: only opted-in controls are ever compared).
    applied_control_nids: Vec<NodeId>,
    /// Issue #15: last `input_rules()` poll (cached; no Game round trip per edit).
    input_rules: Vec<(String, String, InputRule)>,
    /// Issue #15: dependent-input element id -> reason currently greyed by an
    /// `InputRule` the engine itself applied (never a host-authored `disabled`).
    rule_disabled: HashMap<String, String>,
    /// Issue #15 test hook: total `hx-changed` class attribute writes
    /// (toggle-on-flip-only). Must not grow on an edit that keeps the same
    /// changed/unchanged state.
    hx_changed_write_count: u64,
    /// Graph review (PR #24 follow-up): Blitz style damage observed by
    /// input-time resolves (`ensure_resolved`), carried into the next
    /// paint's damage plan so it is not silently dropped. Without this, an
    /// `ensure_resolved` triggered by a later input event (e.g. hit-testing
    /// a pointer move) consumes the restyle-damage flags a prior greying /
    /// `hx-changed` write set, before the host's next real `paint()` ever
    /// gets to read them via its own `resolve_stylist` probe.
    ///
    /// A `HashSet`, not a `Vec` (Graph re-review, PR #24 round 2): a host
    /// that drives many input-time resolves on the same handful of nodes
    /// without ever painting (e.g. rapid typing with no intervening frame)
    /// would otherwise grow this without bound — one entry per resolve, not
    /// per distinct node. Deduping by `NodeId` bounds it by the number of
    /// distinct nodes that can ever carry style damage, i.e. document size,
    /// not edit count.
    carried_style_damage: HashSet<NodeId>,
    pending: PendingState,
    released_outcomes: Vec<InputOutcome>,
    hover_id: Option<String>,
    press: Option<PressState>,
    /// Issue #81 (Designer ruling 2): a ▲/▼ press held on a number box's
    /// arrow, stepping on press and repeating until the release.
    spin_hold: Option<SpinHold>,
    /// Issue #81: the number box whose ArrowUp/
    /// ArrowDown steps sent no `change` yet; the key release reports once.
    key_hold: Option<String>,
    /// Issue #85 (Ruling 7): the last primary press in a text field, so the
    /// next one can count as a double- or triple-click.
    last_text_click: Option<TextClick>,
    /// The press clock frozen by a test (Graph review of #84/#86): when set,
    /// [`Self::press_clock`] reads it instead of the wall clock, so the
    /// click-count window and the arrow hold can't drift under load.
    test_clock: Option<Instant>,
    focus: FocusState,
    /// Physical viewport size used for layout / hit-testing (updated on paint).
    layout_viewport: (u32, u32),
    /// Last paint scale factor (hidpi). Full repaint when this changes.
    layout_scale: f64,
    needs_resolve: bool,
    /// Opt-in thousands grouping (`data-hx-group="on"`, issue #40) still has
    /// to be written into unfocused number editors. Set on load and after
    /// every swap; consumed right after the next style+layout pass, once
    /// Blitz has (re)built the `PlainEditor`s from their `value`.
    group_display_pending: bool,
    /// Spec: skip clean frames — only raster when something is damaged.
    paint_dirty: bool,
    /// Force a full-surface repaint (first frame, theme/stylesheet, etc.).
    force_full_repaint: bool,
    cached_rgba: Option<RgbaImage>,
    /// Per-node own/ink bounds from the last painted frame (device pixels).
    last_bounds: BoundsSnapshot,
    /// Damage rects that must be merged into the next paint plan (device pixels).
    /// Overlay close, fold, and similar paths push here so `compute_plan` cannot
    /// drop them. A full-surface pending rect forces `full_repaint`.
    pending_damage: Vec<IntRect>,
    /// Primary-button drag on an `<input type="range">`.
    range_drag_id: Option<String>,
    /// Set when a range drag was active on the press that is about to release,
    /// so `PointerUp` can fire `change` once and suppress `click`.
    range_drag_released: Option<String>,
    /// Issue #35: pointer offset from the thumb centre for the active range
    /// drag (a press on the thumb grabs it in place).
    range_grab: f32,
    /// Issue #35: the active range press landed on the groove and paged one
    /// block toward the pointer; moves and the release do not drag.
    range_paging: bool,
    /// Issue #35: the range's value when the press began; the release fires
    /// `change` only when the value differs (HTML: no change, no event).
    range_press_value: Option<String>,
    /// Issue #89 (Designer, HTML_CONTRACT `hx-sync`, Mainnet master
    /// 188feffd): value controls whose newest committed value the game has
    /// not answered yet, at most one per control, in commit order.
    catch_up: Vec<CatchUp>,
    /// Issue #89: the value control a `change` being fired came from (the
    /// trigger may bubble to its form), so a held `change` knows its box.
    change_origin: Option<String>,
    /// Issue #89: text the player was typing, not committed, in the box a
    /// redraw replaced: the rebound editor gets it back (box id, text).
    kept_typing: Option<(String, String)>,
    /// Issue #94 (Graph review): per radio group `name`, the value the last
    /// request a radio of that group sent carried. A radio's catch-up
    /// compares with it, since another radio of the group may have sent
    /// after the held one's request (b, then c, then b while b is pending).
    /// At most one entry per radio group in the document.
    radio_sent: HashMap<String, String>,
    /// Issue #35: engine-drawn slider geometry painted last frame; a change
    /// damages that slider's old and new rects only.
    last_sliders: Vec<SliderGeom>,
    /// Open `<select>` drop list (engine-drawn overlay).
    open_select: Option<OpenSelect>,
    /// Hovered row in the open list (document coords from last move).
    select_hover_row: Option<usize>,
    /// `<select>` node ids cached at index / reindex time.
    select_nids: Vec<NodeId>,
    /// Consume the next Esc key-up (key-down closed an open select).
    consume_escape_keyup: bool,
    /// Cached truncated paint layouts for `text-overflow: ellipsis` (issue #12).
    ellipsis_cache: EllipsisCache,
    /// Lazily created on the first ellipsis candidate (shared DejaVu bytes).
    ellipsis_shaper: Option<EllipsisShaper>,
    /// Total `doc.resolve` calls on this surface (tests: idle frames must not grow).
    resolve_count: u64,
    /// Test hook: next paint returns `EngineError::Paint` after ellipsis swap.
    fail_next_paint: bool,
    /// Test hook: panic after ellipsis swap (Drop must still restore).
    panic_next_paint: bool,
    /// Focused text/number edit session (issue #17).
    text_edit: Option<TextEditSession>,
    /// Cached `(action, input name, InputLimits)` from the last
    /// `SliceSource::input_limits` call (issue #16). Re-applied to matching
    /// inputs after a swap without calling `input_limits` again.
    limits_cache: Vec<(String, String, InputLimits)>,
    /// Markup `min`/`max`/`step` recorded the first time the engine
    /// overwrote them for a given `(action, input name)` (PR27 round 2,
    /// non-blocking item): restored when a later `input_limits()` no longer
    /// lists that key.
    limits_markup: HashMap<(String, String), MarkupSnapshot>,
    /// Issue #26: element id -> vertical scroll offset recorded just before
    /// a swap replaced (or refilled) that element; restored, clamped, at the
    /// next layout when an element with the same id exists.
    scroll_restore: HashMap<String, f32>,
    /// Issue #26: primary-button drag on a scrollbar thumb.
    scroll_drag: Option<ScrollDrag>,
    /// Issue #26: scrollbar arrow held down (pressed bevel until PointerUp).
    scroll_pressed: Option<PressedArrow>,
    /// Issue #26: the current primary press started on a scrollbar, so the
    /// matching `PointerUp` is consumed (no click).
    scroll_press_active: bool,
    /// Issue #26: scrollbar geometry + pressed state painted last frame; a
    /// change damages the old and new bar rects.
    last_bars: Vec<(BarGeom, Option<BarPart>)>,
    /// Issue #26 round 3: element id -> author `padding-right` (CSS text)
    /// for every container with an engine-reserved 16 px gutter. Sticky per
    /// id: kept across same-id swaps, dropped once no element has the id.
    gutters: BTreeMap<String, String>,
    /// The engine-owned gutter stylesheet, installed in the user-agent
    /// origin through Blitz's user-agent sheet API. Issue #100: not tied to
    /// a node, so Blitz's node-id ordering of `<style>` sheets never sees it.
    gutter_sheet: Option<UserAgentStylesheet>,
    /// A gutter made another container overflow: resolve again next time.
    gutter_followup: bool,
}

/// Thumb drag state (issue #26).
#[derive(Copy, Clone, Debug)]
struct ScrollDrag {
    nid: NodeId,
    /// Pointer y minus thumb top at press, CSS px.
    grab: f32,
}

/// Markup `min`/`max`/`step` attribute values an input had before the engine
/// first overwrote them with slice-supplied `InputLimits` (issue #16).
/// `None` for a field means the attribute was absent in markup.
#[derive(Clone, Debug, Default)]
struct MarkupSnapshot {
    min: Option<String>,
    max: Option<String>,
    step: Option<String>,
}

/// Issue #85: the last primary press in a text field. The next press
/// continues the run (double-, triple-click) on the same field within
/// [`MULTI_CLICK_SLOP_PX`] and [`MULTI_CLICK_MS`], as Blitz counts clicks.
#[derive(Clone, Debug)]
struct TextClick {
    id: String,
    x: f32,
    y: f32,
    at: Instant,
    count: u32,
}

/// Issue #85: how far apart (CSS px, each axis) presses of one click run may be.
const MULTI_CLICK_SLOP_PX: f32 = 2.0;
/// Issue #85: the most time between presses of one click run.
const MULTI_CLICK_MS: u128 = 500;

impl TextClick {
    fn continues(&self, id: &str, x: f32, y: f32, now: Instant) -> bool {
        self.id == id
            && (x - self.x).abs() <= MULTI_CLICK_SLOP_PX
            && (y - self.y).abs() <= MULTI_CLICK_SLOP_PX
            && now.saturating_duration_since(self.at).as_millis() <= MULTI_CLICK_MS
    }
}

/// Issue #81 (Designer ruling 2, HTML_CONTRACT §2): one ▲/▼ press held on
/// a number box's arrow. The press steps once; after
/// [`SPIN_REPEAT_DELAY`] the box steps again every [`SPIN_REPEAT_INTERVAL`]
/// (no speed-up) until the release, which sends the one `change`.
#[derive(Clone, Debug)]
struct SpinHold {
    target_nid: NodeId,
    target_id: Option<String>,
    dir: i32,
    pressed_at: Instant,
    /// Repeat steps done so far (the press step not counted).
    repeats: u32,
    /// The value the game last got for the box when the press began (the
    /// last `change`, or the value at focus): the release reports only a
    /// value that differs from it (issue #81, Ruling 5).
    reported: String,
    /// A repeat step found the box held at min or max: later repeats would
    /// move nothing, so the hold stops dirtying the box (Graph review).
    pinned: bool,
}

/// Issue #81: the wait before a held arrow starts repeating.
const SPIN_REPEAT_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
/// Issue #81: the time between repeat steps of a held arrow.
const SPIN_REPEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Repeat steps due `held` after the press (issue #81): none before
/// [`SPIN_REPEAT_DELAY`], then one at the delay and one per interval after.
fn spin_repeats_due(held: std::time::Duration) -> u32 {
    if held < SPIN_REPEAT_DELAY {
        return 0;
    }
    let past = (held - SPIN_REPEAT_DELAY).as_millis() / SPIN_REPEAT_INTERVAL.as_millis();
    u32::try_from(past).unwrap_or(u32::MAX - 1) + 1
}

#[derive(Clone, Debug)]
struct PressState {
    id: String,
    button: PointerButton,
    #[allow(dead_code)]
    mods: Modifiers,
}

/// Result of one `LiveSurface::render` call.
struct RenderResult {
    /// `None` when raster was skipped (clean frame).
    plan: Option<DamagePlan>,
    style_layout_passes: u64,
}

pub struct Engine {
    config: EngineConfig,
    /// Issue #28: validated `config.fonts`, decoded once; one Blob per face.
    fonts: Arc<HostFonts>,
    /// Issue #28: set when `Engine::new` got invalid `config.fonts`; returned
    /// by every surface build so the engine never paints with a partial set.
    font_error: Option<EngineError>,
    declared_actions: HashSet<String>,
    surfaces: BTreeMap<SurfaceId, Surface>,
    requests: RequestLedger,
    /// Surface that last received a pointer event; it gets a leave when another surface does.
    hover_surface: Option<SurfaceId>,
    lru_clock: u64,
    timings: Timings,
    paint_stats: PaintStats,
    /// Counts style+layout resolve passes that run inside `paint` (batching test).
    style_layout_passes: u64,
    /// Test harness: assert pending timeouts were observed.
    pub(crate) frame_count: u32,
    eviction_count: u64,
    last_eviction_ns: u64,
    rebuild_count: u64,
    last_rebuild_ns: u64,
    /// Set while pinned bytes are above [`PINNED_SURFACE_WARN_BYTES`] so the warning logs once per crossing.
    pinned_warned: bool,
}

impl Engine {
    /// Build an engine, validating [`EngineConfig::fonts`] (issue #28) up
    /// front: bytes that are not a readable font, an empty family, a weight
    /// outside `1..=1000`, a font collection, a duplicate (family
    /// case-insensitive, weight, style) or a tie with the bundled
    /// "DejaVu Sans" 400 normal return [`EngineError::InvalidFont`]. Prefer this over
    /// [`Engine::new`] when the config carries host fonts.
    pub fn try_new(config: EngineConfig) -> Result<Self, EngineError> {
        let fonts = HostFonts::build(&config.fonts)?;
        Ok(Self::with_fonts(config, Ok(fonts)))
    }

    /// Infallible constructor. With invalid [`EngineConfig::fonts`] it does
    /// not panic: the [`EngineError::InvalidFont`] is returned by every later
    /// `load_html` / `open_surface` (and a rebuild), so nothing ever paints
    /// with a partial font set. Use [`Engine::try_new`] to get it at build time.
    pub fn new(config: EngineConfig) -> Self {
        let fonts = HostFonts::build(&config.fonts);
        Self::with_fonts(config, fonts)
    }

    pub(crate) fn with_fonts(
        config: EngineConfig,
        fonts: Result<Arc<HostFonts>, EngineError>,
    ) -> Self {
        let (fonts, font_error) = match fonts {
            Ok(f) => (f, None),
            Err(e) => (Arc::new(HostFonts::default()), Some(e)),
        };
        Self {
            config,
            fonts,
            font_error,
            declared_actions: HashSet::new(),
            surfaces: BTreeMap::new(),
            requests: RequestLedger {
                next: 1,
                owners: HashMap::new(),
            },
            hover_surface: None,
            lru_clock: 0,
            timings: Timings::default(),
            paint_stats: PaintStats::default(),
            style_layout_passes: 0,
            frame_count: 0,
            eviction_count: 0,
            last_eviction_ns: 0,
            rebuild_count: 0,
            last_rebuild_ns: 0,
            pinned_warned: false,
        }
    }

    /// Host calls this after a theme or stylesheet change. Next paint of every
    /// live surface is a full repaint.
    pub fn mark_theme_or_stylesheet_changed(&mut self) {
        for live in self.surfaces.values_mut().filter_map(|s| s.live.as_mut()) {
            live.force_full_repaint = true;
            live.paint_dirty = true;
        }
    }

    pub fn declare_actions(&mut self, names: &[&str]) {
        for n in names {
            self.declared_actions.insert((*n).to_string());
        }
    }

    // ------------------------------------------------------------------
    // Surfaces
    // ------------------------------------------------------------------

    /// Open (or replace) surface `id` with `html` laid out at `viewport`.
    ///
    /// Parses, validates the dialect, and runs style + layout. A hidden surface
    /// (`opts.visible == false`) also runs its first raster here so it is
    /// pre-drawn when shown. Replacing an open id drops its pending requests,
    /// like `load_html`. On a dialect error the previous surface (if any) is kept.
    ///
    /// May evict least-recently-used hidden, unpinned surfaces (never `id`
    /// itself, never one with a pending action) to stay within
    /// [`EngineConfig::hidden_surface_budget_bytes`].
    pub fn open_surface(
        &mut self,
        id: SurfaceId,
        html: &str,
        viewport: Viewport,
        opts: SurfaceOpts,
    ) -> Result<(), EngineError> {
        if let Some(err) = &self.font_error {
            return Err(err.clone());
        }
        let size = SurfaceSize::from_viewport(&viewport);
        let mut live = LiveSurface::build(&self.declared_actions, html, size, &self.fonts)?;
        if !opts.visible {
            let result = live.render(size, self.config.paint_mode)?;
            self.style_layout_passes = self
                .style_layout_passes
                .saturating_add(result.style_layout_passes);
        }

        self.requests.forget_surface(id);
        if self.hover_surface == Some(id) {
            self.hover_surface = None;
        }
        let last_used = self.touch_lru();
        self.surfaces.insert(
            id,
            Surface {
                source_html: html.to_string(),
                size,
                visible: opts.visible,
                pinned: opts.pinned,
                last_used,
                needs_full_host_upload: true,
                live: Some(live),
            },
        );
        self.enforce_hidden_budget(Some(id));
        self.check_pinned_bytes();
        Ok(())
    }

    /// Show or hide a surface. Visibility, position and z-order are host-owned;
    /// hiding never repaints.
    ///
    /// Showing (`false` -> `true`) makes the next [`Self::paint_surface`] return
    /// **full-surface damage** with the whole RGBA buffer: the host should drop
    /// the GPU texture of a hidden surface and re-upload it on show. A pre-drawn
    /// surface with nothing dirty is not re-rastered; its cached buffer is returned.
    ///
    /// Showing an evicted surface rebuilds it cold from the HTML given to
    /// `open_surface` (parse + style + layout); slice versions start empty so
    /// the next `sync` re-renders every section from current slices, and the
    /// next paint is a full repaint.
    ///
    /// Hiding may evict least-recently-used hidden, unpinned surfaces without a
    /// pending action (including this one) to stay within
    /// [`EngineConfig::hidden_surface_budget_bytes`].
    pub fn set_visible(&mut self, id: SurfaceId, visible: bool) -> Result<(), EngineError> {
        let last_used = self.touch_lru();
        let surface = self
            .surfaces
            .get_mut(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        surface.last_used = last_used;
        if visible {
            if surface.visible {
                return Ok(());
            }
            if surface.live.is_none() {
                let t0 = Instant::now();
                let live = LiveSurface::build(
                    &self.declared_actions,
                    &surface.source_html,
                    surface.size,
                    &self.fonts,
                )?;
                surface.live = Some(live);
                self.rebuild_count = self.rebuild_count.saturating_add(1);
                self.last_rebuild_ns = t0.elapsed().as_nanos() as u64;
            }
            surface.visible = true;
            surface.needs_full_host_upload = true;
            self.check_pinned_bytes();
        } else {
            if surface.visible {
                surface.visible = false;
                surface.needs_full_host_upload = true;
            }
            self.enforce_hidden_budget(None);
        }
        Ok(())
    }

    /// Change a surface's layout size / scale. A changed size or scale makes the
    /// next paint a full repaint; an unchanged one is a no-op (moving a window
    /// never repaints). Evicted surfaces just record the new size.
    ///
    /// May evict least-recently-used hidden, unpinned surfaces (never `id`
    /// itself, never one with a pending action).
    pub fn resize_surface(&mut self, id: SurfaceId, viewport: Viewport) -> Result<(), EngineError> {
        let size = SurfaceSize::from_viewport(&viewport);
        let last_used = self.touch_lru();
        let surface = self
            .surfaces
            .get_mut(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        surface.last_used = last_used;
        if surface.size != size {
            surface.size = size;
            if let Some(live) = surface.live.as_mut() {
                live.force_full_repaint = true;
                live.paint_dirty = true;
            }
        }
        self.enforce_hidden_budget(Some(id));
        self.check_pinned_bytes();
        Ok(())
    }

    /// Close a surface: its document and RGBA buffer are dropped immediately,
    /// along with its pending requests (a later `complete` for them is a no-op).
    pub fn close_surface(&mut self, id: SurfaceId) -> Result<(), EngineError> {
        self.surfaces
            .remove(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        self.requests.forget_surface(id);
        if self.hover_surface == Some(id) {
            self.hover_surface = None;
        }
        self.check_pinned_bytes();
        Ok(())
    }

    /// Paint one surface into its CPU RGBA buffer (spec `paint(id)`).
    ///
    /// Clean frames skip raster and return `skipped: true`, `rgba: None`, empty
    /// `damage`. Never evicts.
    ///
    /// The first paint of a visible surface after it was opened, shown with
    /// [`Self::set_visible`], or rebuilt from eviction returns **full-surface
    /// damage** and the full buffer, even when nothing was re-rastered. The host
    /// is expected to have dropped the GPU texture while the surface was hidden.
    ///
    /// Painting a hidden surface keeps the pre-drawn buffer current; the full
    /// upload is still reported on the first paint after it becomes visible.
    pub fn paint_surface(&mut self, id: SurfaceId) -> Result<PaintOutput, EngineError> {
        let t0 = Instant::now();
        // Issue #89: a timeout runs the catch-up before this frame renders.
        self.tick_surface_pending(id);
        let last_used = self.touch_lru();
        self.frame_count = self.frame_count.saturating_add(1);
        let paint_mode = self.config.paint_mode;

        let surface = self
            .surfaces
            .get_mut(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        surface.last_used = last_used;
        let size = surface.size;
        let full_upload = surface.visible && surface.needs_full_host_upload;
        let live = surface
            .live
            .as_mut()
            .ok_or(EngineError::SurfaceEvicted(id))?;

        let result = live.render(size, paint_mode)?;
        self.style_layout_passes = self
            .style_layout_passes
            .saturating_add(result.style_layout_passes);

        if result.plan.is_none() && !full_upload {
            self.timings.paint_ns = t0.elapsed().as_nanos() as u64;
            self.timings.paint_skipped = true;
            self.paint_stats = PaintStats::default();
            return Ok(PaintOutput {
                rgba: None,
                texture: None,
                focus: live.focus.clone(),
                skipped: true,
                damage: Vec::new(),
            });
        }

        let rgba = live
            .cached_rgba
            .clone()
            .ok_or_else(|| EngineError::Internal(format!("{id} has no raster after paint")))?;
        let focus = live.focus.clone();
        if surface.visible {
            surface.needs_full_host_upload = false;
        }

        let (damage, stats) = if full_upload {
            let stats = match &result.plan {
                Some(plan) => PaintStats {
                    damage_rect_count: 1,
                    merged_to_bbox: plan.merged_to_bbox,
                    full_repaint_area_fallback: plan.full_repaint_area_fallback,
                    full_repaint: plan.full_repaint,
                    full_host_upload: true,
                },
                None => PaintStats {
                    damage_rect_count: 1,
                    full_host_upload: true,
                    ..PaintStats::default()
                },
            };
            (
                vec![IntRect::full_surface(rgba.width, rgba.height).to_rect()],
                stats,
            )
        } else {
            let plan = result
                .plan
                .as_ref()
                .ok_or_else(|| EngineError::Internal("missing damage plan".into()))?;
            (
                plan.rects.iter().map(|r| r.to_rect()).collect(),
                PaintStats {
                    damage_rect_count: plan.rects.len(),
                    merged_to_bbox: plan.merged_to_bbox,
                    full_repaint_area_fallback: plan.full_repaint_area_fallback,
                    full_repaint: plan.full_repaint,
                    full_host_upload: false,
                },
            )
        };

        self.timings.paint_ns = t0.elapsed().as_nanos() as u64;
        self.timings.paint_skipped = false;
        self.paint_stats = stats;
        Ok(PaintOutput {
            rgba: Some(rgba),
            texture: None,
            focus,
            skipped: false,
            damage,
        })
    }

    /// Surfaces whose next `paint_surface` would produce pixels. Cheap (no
    /// style/layout). Never reports hidden unpinned surfaces, nor evicted ones.
    pub fn dirty_surfaces(&self) -> Vec<SurfaceId> {
        self.surfaces
            .iter()
            .filter_map(|(id, s)| {
                let live = s.live.as_ref()?;
                if !s.visible && !s.pinned {
                    return None;
                }
                let dirty = live.paint_dirty
                    || live.force_full_repaint
                    || !live.pending_damage.is_empty()
                    || live.cached_rgba.is_none()
                    || live.layout_viewport != (s.size.width, s.size.height)
                    || (live.layout_scale - s.size.scale).abs() > f64::EPSILON
                    || (s.visible && s.needs_full_host_upload);
                dirty.then_some(*id)
            })
            .collect()
    }

    /// Run `sync` on one surface only.
    pub fn sync_surface(
        &mut self,
        id: SurfaceId,
        slices: &dyn SliceSource,
        frags: &mut dyn FragmentSource,
    ) -> Result<DirtySet, EngineError> {
        let t0 = Instant::now();
        let surface = self
            .surfaces
            .get_mut(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        let live = surface
            .live
            .as_mut()
            .ok_or(EngineError::SurfaceEvicted(id))?;
        let dirty = live.sync(&self.declared_actions, slices, frags)?;
        self.timings.sync_ns = t0.elapsed().as_nanos() as u64;
        Ok(dirty)
    }

    /// Deliver input the host routed to surface `id`. Pointer events on a
    /// different surface than the last one first send that surface a
    /// pointer-leave, so only one surface is hovered at a time.
    pub fn input_surface(&mut self, id: SurfaceId, ev: InputEvent) -> InputOutcome {
        let is_pointer = matches!(
            ev,
            InputEvent::PointerMove { .. }
                | InputEvent::PointerDown { .. }
                | InputEvent::PointerUp { .. }
        );
        if is_pointer && self.hover_surface != Some(id) {
            if let Some(prev) = self.hover_surface.take() {
                let _ = self.route_input(prev, InputEvent::PointerLeave);
            }
        }
        if is_pointer {
            self.hover_surface = Some(id);
        } else if matches!(ev, InputEvent::PointerLeave) && self.hover_surface == Some(id) {
            self.hover_surface = None;
        }
        self.route_input(id, ev)
    }

    fn route_input(&mut self, id: SurfaceId, ev: InputEvent) -> InputOutcome {
        let Self {
            surfaces,
            declared_actions,
            requests,
            config,
            ..
        } = self;
        match surfaces.get_mut(&id).and_then(|s| s.live.as_mut()) {
            Some(live) => {
                let mut ctx = ActionCtx {
                    surface: id,
                    key_releases: config.key_releases,
                    declared_actions,
                    requests,
                };
                live.input(&mut ctx, ev)
            }
            None => unrouted_outcome(ev),
        }
    }

    /// Queued releases emitted by `complete` on surface `id`.
    pub fn take_surface_outcomes(&mut self, id: SurfaceId) -> Vec<InputOutcome> {
        self.surfaces
            .get_mut(&id)
            .and_then(|s| s.live.as_mut())
            .map(|live| std::mem::take(&mut live.released_outcomes))
            .unwrap_or_default()
    }

    /// Borrowed view of surface `id`'s last rasterized frame, if any.
    pub fn surface_last_frame(&self, id: SurfaceId) -> Option<&RgbaImage> {
        self.live(id)?.cached_rgba.as_ref()
    }

    /// Read an attribute from an element by id on surface `id`.
    pub fn surface_attr(&self, id: SurfaceId, elem_id: &str, name: &str) -> Option<String> {
        self.live(id)?.attr(elem_id, name)
    }

    /// Serialize surface `id`'s document HTML (root element outerHTML).
    pub fn surface_serialize_html(&self, id: SurfaceId) -> Result<String, EngineError> {
        let surface = self
            .surfaces
            .get(&id)
            .ok_or(EngineError::UnknownSurface(id))?;
        let live = surface
            .live
            .as_ref()
            .ok_or(EngineError::SurfaceEvicted(id))?;
        Ok(live.doc.root_element().outer_html())
    }

    /// Open surface ids in ascending order (including evicted ones).
    pub fn surface_ids(&self) -> Vec<SurfaceId> {
        self.surfaces.keys().copied().collect()
    }

    pub fn surface_info(&self, id: SurfaceId) -> Option<SurfaceInfo> {
        let s = self.surfaces.get(&id)?;
        Some(SurfaceInfo {
            visible: s.visible,
            pinned: s.pinned,
            evicted: s.live.is_none(),
            width: s.size.width,
            height: s.size.height,
            scale: s.size.scale,
            estimated_bytes: s.estimated_bytes(),
        })
    }

    /// Estimated engine bytes of all live pinned surfaces. Above
    /// [`PINNED_SURFACE_WARN_BYTES`] the engine logs a warning.
    pub fn pinned_bytes(&self) -> u64 {
        self.surfaces
            .values()
            .filter(|s| s.pinned)
            .map(Surface::estimated_bytes)
            .sum()
    }

    pub fn surface_memory_stats(&self) -> SurfaceMemoryStats {
        let mut stats = SurfaceMemoryStats {
            hidden_budget_bytes: self.config.hidden_surface_budget_bytes,
            eviction_count: self.eviction_count,
            last_eviction_ns: self.last_eviction_ns,
            rebuild_count: self.rebuild_count,
            last_rebuild_ns: self.last_rebuild_ns,
            ..SurfaceMemoryStats::default()
        };
        for s in self.surfaces.values() {
            if s.live.is_none() {
                stats.evicted_surfaces += 1;
                continue;
            }
            let bytes = s.estimated_bytes();
            stats.live_surfaces += 1;
            stats.live_bytes += bytes;
            if s.pinned {
                stats.pinned_bytes += bytes;
            }
            if s.counts_against_hidden_budget() {
                stats.hidden_unpinned_bytes += bytes;
            }
        }
        stats
    }

    fn touch_lru(&mut self) -> u64 {
        self.lru_clock += 1;
        self.lru_clock
    }

    fn live(&self, id: SurfaceId) -> Option<&LiveSurface> {
        self.surfaces.get(&id)?.live.as_ref()
    }

    fn live_mut(&mut self, id: SurfaceId) -> Option<&mut LiveSurface> {
        self.surfaces.get_mut(&id)?.live.as_mut()
    }

    fn hidden_unpinned_bytes(&self) -> u64 {
        self.surfaces
            .values()
            .filter(|s| s.counts_against_hidden_budget())
            .map(Surface::estimated_bytes)
            .sum()
    }

    /// Evict LRU hidden, unpinned surfaces with no pending work until the
    /// hidden budget holds (or nothing evictable is left). Only called from
    /// `open_surface`, `set_visible(false)` and `resize_surface`.
    fn enforce_hidden_budget(&mut self, protect: Option<SurfaceId>) {
        let budget = self.config.hidden_surface_budget_bytes;
        let mut hidden = self.hidden_unpinned_bytes();
        while hidden > budget {
            let victim = self
                .surfaces
                .iter()
                .filter(|(id, s)| Some(**id) != protect && s.is_evictable())
                .min_by_key(|(_, s)| s.last_used)
                .map(|(id, _)| *id);
            let Some(victim) = victim else {
                break;
            };
            hidden = hidden.saturating_sub(self.evict(victim));
        }
    }

    /// Drop a surface's document and buffer, keeping HTML / size / opts. Returns
    /// the estimated bytes freed.
    fn evict(&mut self, id: SurfaceId) -> u64 {
        let t0 = Instant::now();
        let Some(surface) = self.surfaces.get_mut(&id) else {
            return 0;
        };
        let freed = surface.estimated_bytes();
        drop(surface.live.take());
        surface.needs_full_host_upload = true;
        self.last_eviction_ns = t0.elapsed().as_nanos() as u64;
        self.eviction_count = self.eviction_count.saturating_add(1);
        self.requests.forget_surface(id);
        if self.hover_surface == Some(id) {
            self.hover_surface = None;
        }
        freed
    }

    fn check_pinned_bytes(&mut self) {
        let pinned = self.pinned_bytes();
        if pinned > PINNED_SURFACE_WARN_BYTES {
            if !self.pinned_warned {
                eprintln!(
                    "hx_native: pinned surfaces hold ~{pinned} bytes (warn above \
                     {PINNED_SURFACE_WARN_BYTES}); pinned surfaces are never evicted"
                );
                self.pinned_warned = true;
            }
        } else {
            self.pinned_warned = false;
        }
    }

    // ------------------------------------------------------------------
    // Single-document API (operates on SurfaceId::DEFAULT)
    // ------------------------------------------------------------------

    /// Load (or replace) the default surface as visible + pinned. Drops all of
    /// its pending requests; the RequestId counter does not restart (only a new
    /// engine restarts it at 1).
    pub fn load_html(&mut self, html: &str) -> Result<(), EngineError> {
        let size = self
            .surfaces
            .get(&SurfaceId::DEFAULT)
            .map(|s| s.size)
            .unwrap_or_default();
        self.open_surface(
            SurfaceId::DEFAULT,
            html,
            Viewport::with_scale(size.width, size.height, size.scale),
            SurfaceOpts {
                visible: true,
                pinned: true,
            },
        )
    }

    /// Sync every live (non-evicted) surface in ascending id order against the
    /// same slices. The returned set aggregates all surfaces.
    pub fn sync(
        &mut self,
        slices: &dyn SliceSource,
        frags: &mut dyn FragmentSource,
    ) -> Result<DirtySet, EngineError> {
        let t0 = Instant::now();
        let mut total = DirtySet::default();
        let mut any_live = false;
        for live in self.surfaces.values_mut().filter_map(|s| s.live.as_mut()) {
            any_live = true;
            total.absorb(live.sync(&self.declared_actions, slices, frags)?);
        }
        if !any_live {
            return Err(EngineError::NotLoaded);
        }
        self.timings.sync_ns = t0.elapsed().as_nanos() as u64;
        Ok(total)
    }

    pub fn input(&mut self, ev: InputEvent) -> InputOutcome {
        self.input_surface(SurfaceId::DEFAULT, ev)
    }

    /// Complete a request emitted by any surface. Unknown requests are logged in
    /// debug builds; requests that timed out (or whose surface was closed) are no-ops.
    pub fn complete(&mut self, request: RequestId, result: Result<(), Reason>) {
        let Some(&owner) = self.requests.owners.get(&request) else {
            #[cfg(debug_assertions)]
            eprintln!("hx_native: complete() for unknown request {request}");
            return;
        };
        let Self {
            surfaces,
            declared_actions,
            requests,
            config,
            ..
        } = self;
        let Some(live) = surfaces.get_mut(&owner).and_then(|s| s.live.as_mut()) else {
            requests.owners.remove(&request);
            return;
        };
        let mut ctx = ActionCtx {
            surface: owner,
            key_releases: config.key_releases,
            declared_actions,
            requests,
        };
        live.complete(&mut ctx, request, result);
    }

    /// Drain queued releases emitted by `complete` across all surfaces, in
    /// ascending surface id order.
    pub fn take_outcomes(&mut self) -> Vec<InputOutcome> {
        self.surfaces
            .values_mut()
            .filter_map(|s| s.live.as_mut())
            .flat_map(|live| std::mem::take(&mut live.released_outcomes))
            .collect()
    }

    /// Borrowed view of the default surface's last rasterized frame, if any.
    ///
    /// Prefer this (or the host's own last texture) over expecting
    /// [`PaintOutput::rgba`] on a skipped paint — skipped paints return
    /// `rgba: None` so the engine never copies the buffer.
    pub fn last_frame(&self) -> Option<&RgbaImage> {
        self.surface_last_frame(SurfaceId::DEFAULT)
    }

    /// Paint the default surface at `viewport` (updates its stored size first).
    ///
    /// Clean frames (nothing dirty, viewport size/scale unchanged) skip
    /// raster and return [`PaintOutput`] with `skipped: true`, `rgba: None`, and
    /// empty `damage` — no buffer copy. The host must keep its previous texture;
    /// see [`Self::last_frame`] for a borrowed look at the cache.
    ///
    /// Style, layout and paint run once here. `sync` / swaps only record changes.
    pub fn paint(&mut self, viewport: Viewport) -> Result<PaintOutput, EngineError> {
        let _ = viewport.holes;
        let surface = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .ok_or(EngineError::NotLoaded)?;
        surface.size = SurfaceSize::from_viewport(&viewport);
        self.paint_surface(SurfaceId::DEFAULT)
    }

    pub fn last_timings(&self) -> Timings {
        self.timings.clone()
    }

    /// Diagnostics for the last non-skipped paint (merge / full-repaint fallbacks).
    pub fn last_paint_stats(&self) -> PaintStats {
        self.paint_stats.clone()
    }

    /// True when any live surface still has an in-flight or queued action.
    pub fn has_pending(&self) -> bool {
        self.surfaces
            .values()
            .any(|s| s.live.as_ref().is_some_and(|live| live.has_pending_work()))
    }

    /// Number of style+layout resolve passes executed inside `paint` (any
    /// surface, including hidden pre-draws) since engine creation.
    pub fn style_layout_pass_count(&self) -> u64 {
        self.style_layout_passes
    }

    /// Total `HtmlDocument::resolve` calls on the default surface (issue #12).
    /// Idle paints with a cached ellipsis must not grow this after the first frame.
    pub fn resolve_count(&self) -> u64 {
        self.live(SurfaceId::DEFAULT)
            .map(|l| l.resolve_count)
            .unwrap_or(0)
    }

    /// Test harness: truncated paint string cached for `#id`, if any.
    pub fn test_ellipsis_truncated(&self, elem_id: &str) -> Option<String> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(elem_id)?;
        live.ellipsis_cache.truncated_for(nid).map(str::to_string)
    }

    /// Test harness: force the next paint to error after the ellipsis swap
    /// (Drop must restore DOM text + inline layouts).
    #[doc(hidden)]
    pub fn test_fail_next_paint(&mut self) {
        if let Some(live) = self.live_mut(SurfaceId::DEFAULT) {
            live.fail_next_paint = true;
            live.paint_dirty = true;
        }
    }

    /// Test harness: panic on the next paint after the ellipsis swap.
    #[doc(hidden)]
    pub fn test_panic_next_paint(&mut self) {
        if let Some(live) = self.live_mut(SurfaceId::DEFAULT) {
            live.panic_next_paint = true;
            live.paint_dirty = true;
        }
    }

    /// Test harness: read text content of `#id` from the live DOM.
    #[doc(hidden)]
    pub fn test_element_text(&self, elem_id: &str) -> Option<String> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(elem_id)?;
        let node = live.doc.get_node(nid)?;
        Some(node.text_content())
    }

    /// Test harness: `inline_layout_data.text` for `#id` (full original after restore).
    #[doc(hidden)]
    pub fn test_inline_layout_text(&self, elem_id: &str) -> Option<String> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(elem_id)?;
        let el = live.doc.get_node(nid)?.element_data()?;
        Some(el.inline_layout_data.as_ref()?.text.clone())
    }

    /// Test harness: whether `#id` currently has inline_layout_data.
    #[doc(hidden)]
    pub fn test_has_inline_layout(&self, elem_id: &str) -> bool {
        let Some(live) = self.live(SurfaceId::DEFAULT) else {
            return false;
        };
        let Some(nid) = live.doc.get_element_by_id(elem_id) else {
            return false;
        };
        live.doc
            .get_node(nid)
            .and_then(|n| n.element_data())
            .is_some_and(|el| el.inline_layout_data.is_some())
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Test helper: serialize current document HTML (root element outerHTML).
    pub fn serialize_html(&self) -> Result<String, EngineError> {
        let live = self
            .live(SurfaceId::DEFAULT)
            .ok_or(EngineError::NotLoaded)?;
        Ok(live.doc.root_element().outer_html())
    }

    /// Test helper: read an attribute from an element by id.
    pub fn attr(&self, id: &str, name: &str) -> Option<String> {
        self.surface_attr(SurfaceId::DEFAULT, id, name)
    }

    /// Test helper: whether element has class token.
    pub fn has_class(&self, id: &str, class: &str) -> bool {
        self.attr(id, "class")
            .map(|c| class_tokens(&c).any(|t| t == class))
            .unwrap_or(false)
    }

    /// Test helper: pending timeout failures (A3c).
    pub fn pending_timeout_failures(&self) -> &[PendingTimeout] {
        self.live(SurfaceId::DEFAULT)
            .map(|live| live.pending.timeout_failures.as_slice())
            .unwrap_or(&[])
    }

    /// Test helper: force a frame tick on every live surface without painting
    /// (for timeout tests).
    pub fn test_tick_frame(&mut self) {
        self.frame_count = self.frame_count.saturating_add(1);
        let ids: Vec<SurfaceId> = self.surfaces.keys().copied().collect();
        for id in ids {
            self.tick_surface_pending(id);
        }
    }

    /// Age surface `id`'s pending requests one frame (A3c timeouts); a timed-
    /// out request still runs its catch-up (issue #89).
    fn tick_surface_pending(&mut self, id: SurfaceId) {
        let Self {
            surfaces,
            declared_actions,
            requests,
            config,
            ..
        } = self;
        let Some(live) = surfaces.get_mut(&id).and_then(|s| s.live.as_mut()) else {
            return;
        };
        let mut ctx = ActionCtx {
            surface: id,
            key_releases: config.key_releases,
            declared_actions,
            requests,
        };
        live.tick_pending(&mut ctx, config.pending_timeout_frames);
    }

    /// Test helper: current focus.
    pub fn focus_state(&self) -> &FocusState {
        self.live(SurfaceId::DEFAULT)
            .map(|live| &live.focus)
            .unwrap_or(&NO_FOCUS)
    }

    /// Test helper: set focus by element id (for keyboard tests without Tab walking).
    #[doc(hidden)]
    pub fn test_set_focus(&mut self, id: &str) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            if let Some(nid) = live.doc.get_element_by_id(id) {
                live.doc.set_focus_to(nid);
                live.focus.focused_id = Some(id.to_string());
                if live
                    .doc
                    .get_node(nid)
                    .is_some_and(|n| is_editable_input(n) && !is_disabled(n))
                {
                    let value_at_focus = live
                        .doc
                        .get_node(nid)
                        .and_then(|n| {
                            n.attrs().and_then(|attrs| {
                                attrs
                                    .iter()
                                    .find(|a| &*a.name.local == "value")
                                    .map(|a| a.value.to_string())
                            })
                        })
                        .unwrap_or_default();
                    prepare_focus_display(&mut live.doc, nid);
                    live.text_edit = Some(TextEditSession {
                        id: id.to_string(),
                        node_id: nid,
                        value_at_change: value_at_focus.clone(),
                        value_at_focus,
                        caret_visible: true,
                        last_blink: Instant::now(),
                        last_caret_rect: None,
                        needs_prepare: false,
                    });
                } else {
                    live.text_edit = None;
                }
                live.mark_paint_dirty();
                live.reconcile_open_select_after_focus();
            }
        }
    }

    /// Test helper: whether element `id` has a pending Stylo state snapshot
    /// (set by `snapshot_node_state_only` before a `:checked` flip).
    #[doc(hidden)]
    pub fn test_node_has_snapshot(&self, id: &str) -> bool {
        let live = self.live(SurfaceId::DEFAULT).expect("default surface");
        let Some(nid) = live.doc.get_element_by_id(id) else {
            return false;
        };
        live.doc.get_node(nid).is_some_and(|n| n.has_snapshot())
    }

    /// Test helper: whether an ancestor of `id` has `dirty_descendants`
    /// (set by `mark_ancestors_dirty` after a toggle).
    #[doc(hidden)]
    pub fn test_ancestor_has_dirty_descendants(&self, id: &str) -> bool {
        let live = self.live(SurfaceId::DEFAULT).expect("default surface");
        let Some(nid) = live.doc.get_element_by_id(id) else {
            return false;
        };
        let Some(node) = live.doc.get_node(nid) else {
            return false;
        };
        let mut parent = node.parent;
        while let Some(pid) = parent {
            let Some(p) = live.doc.get_node(pid) else {
                break;
            };
            if p.has_dirty_descendants() {
                return true;
            }
            parent = p.parent;
        }
        false
    }

    /// Test helper: toggle a checkbox by id without firing `change` (so snapshot
    /// / damage probes are not masked by pending-attribute writes).
    #[doc(hidden)]
    pub fn test_toggle_checkbox(&mut self, id: &str) -> bool {
        let live = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
            .expect("default surface");
        let Some(nid) = live.doc.get_element_by_id(id) else {
            return false;
        };
        let changed = toggle_checkbox(&mut live.doc, nid);
        if changed {
            live.mark_dom_dirty();
            live.mark_paint_dirty();
        }
        changed
    }

    /// Test helper: push a device-pixel damage rect that must be merged into the
    /// next paint plan (issue #20 pending-damage / fold full-surface case).
    #[doc(hidden)]
    pub fn test_push_pending_damage(&mut self, rect: Rect) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            live.push_pending_damage(IntRect {
                x: rect.x.floor() as i32,
                y: rect.y.floor() as i32,
                w: rect.width.ceil() as i32,
                h: rect.height.ceil() as i32,
            });
        }
    }

    /// Test helper: transform-aware border box in CSS pixels `(x, y, w, h)`.
    #[doc(hidden)]
    pub fn test_element_box(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(id)?;
        crate::damage::element_border_box_css(&live.doc, nid)
    }

    /// Test helper (issue #26): vertical scroll offset and max offset of the
    /// element `id` on the default surface, CSS px.
    #[doc(hidden)]
    pub fn test_scroll_top(&self, id: &str) -> Option<(f32, f32)> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(id)?;
        Some((
            crate::scroll::offset_y(&live.doc, nid),
            crate::scroll::max_offset_y(&live.doc, nid),
        ))
    }

    /// Test helper (issue #26 round 3): content box of `id` in CSS px
    /// `(x, y, w, h)` — border box minus border and padding (the reserved
    /// gutter is part of the padding).
    #[doc(hidden)]
    pub fn test_content_box(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(id)?;
        let (x, y, w, h) = crate::damage::element_border_box_css(&live.doc, nid)?;
        let l = live.doc.get_node(nid)?.final_layout();
        let (b, p) = (l.border, l.padding);
        Some((
            x + b.left + p.left,
            y + b.top + p.top,
            w - b.left - b.right - p.left - p.right,
            h - b.top - b.bottom - p.top - p.bottom,
        ))
    }

    /// Test helper (issue #26): engine-drawn scrollbar geometry for `id`
    /// (CSS px): `(up, down, track, thumb)` as `(x, y, w, h)`; `None` when no
    /// bar is shown.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn test_scrollbar(
        &self,
        id: &str,
    ) -> Option<(
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
        Option<(f32, f32, f32, f32)>,
    )> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(id)?;
        let g = crate::scroll::bar_geometry(&live.doc, nid)?;
        let t = |r: crate::select_ui::ControlRect| (r.x, r.y, r.w, r.h);
        Some((t(g.up), t(g.down), t(g.track), g.thumb.map(t)))
    }

    /// Test helper (issue #35): engine-drawn slider geometry for range input
    /// `id` (CSS px): `(content, track, thumb, disabled)` as `(x, y, w, h)`.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn test_slider(
        &self,
        id: &str,
    ) -> Option<(
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
        (f32, f32, f32, f32),
        bool,
    )> {
        let live = self.live(SurfaceId::DEFAULT)?;
        let nid = live.doc.get_element_by_id(id)?;
        let g = crate::slider::slider_geometry(&live.doc, nid)?;
        let t = |r: crate::select_ui::ControlRect| (r.x, r.y, r.w, r.h);
        Some((t(g.content), t(g.track), t(g.thumb), g.disabled))
    }

    /// Test helper (issue #15): total `hx-changed` class attribute writes on
    /// the default surface so far. Must not grow on an edit that keeps the
    /// same changed/unchanged state (flip-only mutation).
    #[doc(hidden)]
    pub fn test_hx_changed_write_count(&self) -> u64 {
        self.live(SurfaceId::DEFAULT)
            .map(|l| l.hx_changed_write_count)
            .unwrap_or(0)
    }

    /// Test helper: count of elements that currently carry non-empty Blitz
    /// restyle damage (or a damaged descendant) on the default surface,
    /// read **before** any `resolve`/`paint` clears the flags. Used to prove
    /// a `hx-changed` no-op edit does not touch extra nodes.
    #[doc(hidden)]
    pub fn test_damaged_element_count(&self) -> usize {
        self.live(SurfaceId::DEFAULT)
            .map(|l| l.count_damage().0)
            .unwrap_or(0)
    }

    /// Test helper (Graph re-review, PR #24 round 2): number of distinct
    /// nodes currently carried in `carried_style_damage` on the default
    /// surface — the input-time restyle damage an `ensure_resolved` call
    /// has stashed because no paint has drained it yet. Deduped by
    /// `NodeId`, so this must stay bounded by document size no matter how
    /// many edits land between paints.
    #[doc(hidden)]
    pub fn test_carried_style_damage_len(&self) -> usize {
        self.live(SurfaceId::DEFAULT)
            .map(|l| l.carried_style_damage.len())
            .unwrap_or(0)
    }

    /// Test helper (issues #81, #85; Graph review): freeze the press clock
    /// shared by the arrow hold and the click count, moved on by `by` (zero
    /// just freezes it). Later presses read the frozen time, so a test's
    /// clicks are exactly `by` apart, whatever the load on the machine.
    #[doc(hidden)]
    pub fn test_advance_clock(&mut self, by: std::time::Duration) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            live.test_clock = Some(live.press_clock() + by);
        }
    }

    /// Test helper (issue #81): make the held ▲/▼ press `held` old and run
    /// its repeat, as the next `sync` would after that long.
    #[doc(hidden)]
    pub fn test_hold_spin_for(&mut self, held: std::time::Duration) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            // Graph review of #84: the repeat count comes from `held` itself,
            // not from a wall-clock read, so the 400/500 ms boundaries in the
            // tests cannot drift under load.
            live.advance_spin_hold_by(Some(held));
        }
    }

    /// Test helper: force one caret blink phase flip (damage + paint dirty).
    pub fn test_force_caret_blink(&mut self) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            if let Some(session) = live.text_edit.as_mut() {
                session.last_blink = Instant::now() - CARET_BLINK_HALF_PERIOD * 2;
            }
            live.advance_caret_blink();
        }
    }

    /// Test helper: seed `last_caret_rect` so the next blink must damage it
    /// (catches mutations that drop previous-caret damage).
    #[doc(hidden)]
    pub fn test_set_last_caret_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            if let Some(session) = live.text_edit.as_mut() {
                session.last_caret_rect = Some(IntRect { x, y, w, h });
            }
        }
    }

    /// Test helper: whether the caret is in the visible blink phase.
    pub fn test_caret_visible(&self) -> Option<bool> {
        self.live(SurfaceId::DEFAULT)
            .and_then(|live| live.text_edit.as_ref().map(|s| s.caret_visible))
    }

    /// Test harness: current `PlainEditor` display text for `#id` (issue
    /// #16: verifies a clamp regroups the unfocused number display).
    #[doc(hidden)]
    pub fn test_editor_display(&mut self, elem_id: &str) -> Option<String> {
        let live = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())?;
        let nid = live.doc.get_element_by_id(elem_id)?;
        editor_text(&mut live.doc, nid)
    }

    /// Test harness (issue #68): the focused editor's selected text for
    /// `#id` (`None` when nothing is selected).
    #[doc(hidden)]
    pub fn test_selected_text(&mut self, elem_id: &str) -> Option<String> {
        let live = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())?;
        let nid = live.doc.get_element_by_id(elem_id)?;
        selected_text(&mut live.doc, nid).filter(|t| !t.is_empty())
    }

    /// Test helper: subscribed slice names the engine will ask `version` for.
    pub fn subscribed_slices(&self) -> HashSet<String> {
        let mut s = HashSet::new();
        s.insert("actions".into());
        if let Some(live) = self.live(SurfaceId::DEFAULT) {
            for subs in live.sections.values() {
                for n in subs {
                    s.insert(n.clone());
                }
            }
        }
        s
    }

    /// Test helper: section ids.
    pub fn section_ids(&self) -> Vec<String> {
        self.live(SurfaceId::DEFAULT)
            .map(|live| live.sections.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Test helper: engine focus order.
    pub fn focusable_ids_for_test(&self) -> Vec<String> {
        self.live(SurfaceId::DEFAULT)
            .map(|live| focusable_ids(&live.doc))
            .unwrap_or_default()
    }
}

/// Outcome for input routed to a surface that is not live.
fn unrouted_outcome(ev: InputEvent) -> InputOutcome {
    match ev {
        InputEvent::Key(key) => InputOutcome::Unhandled(key),
        InputEvent::Text(s) => InputOutcome::Unhandled(KeyEvent {
            key: s,
            code: String::new(),
            mods: Modifiers::default(),
            repeat: false,
            pressed: true,
        }),
        InputEvent::Paste(_) | InputEvent::Copy | InputEvent::Cut => InputOutcome::None,
        InputEvent::PointerLeave => InputOutcome::Hover { id: None },
        InputEvent::PointerDown { .. } | InputEvent::PointerUp { .. } => {
            InputOutcome::Press { id: None }
        }
        InputEvent::PointerMove { .. } | InputEvent::Wheel { .. } => InputOutcome::None,
    }
}

impl LiveSurface {
    /// Parse + validate + index + one style/layout pass.
    fn build(
        declared_actions: &HashSet<String>,
        html: &str,
        size: SurfaceSize,
        fonts: &Arc<HostFonts>,
    ) -> Result<Self, EngineError> {
        let layout_viewport = (size.width, size.height);
        let layout_scale = size.scale;
        let mut doc = parse_html_document_scaled(html, layout_viewport, layout_scale, fonts);

        // HTML contract §2: strict dialect on loaded HTML, then index bindings.
        let known_ids = collect_element_ids(&doc);
        validate_loaded_tree(declared_actions, &doc, &known_ids)?;
        let (bindings, sections) = index_bindings(declared_actions, &doc)?;

        doc.add_user_agent_stylesheet(select_ua_stylesheet());
        doc.add_user_agent_stylesheet(slider_ua_stylesheet());
        doc.resolve(0.0);
        align_text_inputs(&mut doc);
        let select_nids = collect_select_nids(&doc);
        let applied_control_nids = collect_applied_control_nids(&doc);
        let mut live = Self {
            doc,
            bindings,
            sections,
            slice_versions: HashMap::new(),
            disabled_actions: HashMap::new(),
            last_action_states: Vec::new(),
            form_baselines: HashMap::new(),
            applied_control_nids,
            input_rules: Vec::new(),
            rule_disabled: HashMap::new(),
            hx_changed_write_count: 0,
            carried_style_damage: HashSet::new(),
            pending: PendingState::default(),
            released_outcomes: Vec::new(),
            hover_id: None,
            press: None,
            spin_hold: None,
            key_hold: None,
            last_text_click: None,
            test_clock: None,
            focus: FocusState::default(),
            layout_viewport,
            layout_scale,
            needs_resolve: false,
            group_display_pending: true,
            paint_dirty: true,
            force_full_repaint: true,
            cached_rgba: None,
            last_bounds: BoundsSnapshot::new(),
            pending_damage: Vec::new(),
            range_drag_id: None,
            range_drag_released: None,
            range_grab: 0.0,
            range_paging: false,
            range_press_value: None,
            catch_up: Vec::new(),
            change_origin: None,
            kept_typing: None,
            radio_sent: HashMap::new(),
            last_sliders: Vec::new(),
            open_select: None,
            select_hover_row: None,
            select_nids,
            consume_escape_keyup: false,
            ellipsis_cache: EllipsisCache::default(),
            ellipsis_shaper: None,
            fonts: Arc::clone(fonts),
            resolve_count: 1,
            fail_next_paint: false,
            panic_next_paint: false,
            text_edit: None,
            limits_cache: Vec::new(),
            limits_markup: HashMap::new(),
            scroll_restore: HashMap::new(),
            scroll_drag: None,
            scroll_pressed: None,
            scroll_press_active: false,
            last_bars: Vec::new(),
            gutters: BTreeMap::new(),
            gutter_sheet: None,
            gutter_followup: false,
        };
        // `doc.resolve` above built the editors: show opt-in grouping on load.
        live.apply_pending_group_display();
        Ok(live)
    }

    /// Write grouped display text into every unfocused opt-in number input
    /// whose editor does not show it yet (issue #40: load, swap and blur all
    /// show the same thing). Queues damage only for fields that changed.
    fn apply_pending_group_display(&mut self) {
        if !self.group_display_pending {
            return;
        }
        self.group_display_pending = false;
        let focused = self.text_edit.as_ref().map(|s| s.node_id);
        for nid in apply_group_display_all(&mut self.doc, focused) {
            self.queue_text_field_damage(nid);
        }
    }

    /// Blitz resolve plus scroll bookkeeping. Returns the number of
    /// style+layout passes run: 1, or 2 on the frame where a scroll
    /// container first needs its gutter (issue #26 round 3).
    fn resolve_doc(&mut self) -> u64 {
        self.prune_gutters();
        self.doc.resolve(0.0);
        self.resolve_count = self.resolve_count.saturating_add(1);
        let mut passes = 1;
        // Gutters are sticky and only ever added, so this terminates; a
        // gutter that makes a nested container overflow (narrower content)
        // is reserved in the same frame. Bounded so a pathological tree
        // costs at most GUTTER_MAX_PASSES extra layouts per frame.
        const GUTTER_MAX_PASSES: u64 = 3;
        while passes <= GUTTER_MAX_PASSES && self.reserve_new_gutters() {
            self.doc.resolve(0.0);
            self.resolve_count = self.resolve_count.saturating_add(1);
            passes += 1;
        }
        if self.containers_needing_gutter().next().is_some() {
            self.gutter_followup = true;
            self.paint_dirty = true;
        }
        self.settle_scroll_offsets();
        // Issue #35: single-line typed fields with text-align right/center.
        align_text_inputs(&mut self.doc);
        passes
    }

    /// Scroll containers with an id that need a gutter but have none yet.
    fn containers_needing_gutter(&self) -> impl Iterator<Item = (NodeId, String)> + '_ {
        scroll_containers(&self.doc).into_iter().filter_map(|nid| {
            let id = node_id_attr(&self.doc, nid)?;
            (!self.gutters.contains_key(&id) && wants_gutter(&self.doc, nid)).then_some((nid, id))
        })
    }

    /// Issue #26 round 3: reserve the 16 px gutter on containers that first
    /// overflow (or are `overflow-y: scroll`) after a layout. Returns whether
    /// the gutter sheet changed (caller lays out once more).
    fn reserve_new_gutters(&mut self) -> bool {
        let new: Vec<(NodeId, String)> = self.containers_needing_gutter().collect();
        if new.is_empty() {
            return false;
        }
        for (nid, id) in &new {
            let base = author_padding_right_css(&self.doc, *nid);
            self.gutters.insert(id.clone(), base);
        }
        self.install_gutter_sheet();
        for (nid, _) in new {
            let r = container_damage_rect(&self.doc, nid, self.layout_scale);
            self.push_pending_damage(r);
        }
        self.mark_paint_dirty();
        true
    }

    /// Drop gutters whose id is gone from the document (element replaced by
    /// a new id). Runs before layout; no extra pass.
    fn prune_gutters(&mut self) {
        if self.gutters.is_empty() {
            return;
        }
        let before = self.gutters.len();
        let doc = &self.doc;
        self.gutters
            .retain(|id, _| doc.get_element_by_id(id).is_some());
        if self.gutters.len() != before {
            self.install_gutter_sheet();
        }
    }

    /// (Re)install the engine-owned gutter stylesheet as the last
    /// user-agent sheet (after the default, select and slider sheets).
    ///
    /// Issue #100: it used to sit on a detached `<style>` node through
    /// `add_stylesheet_for_node`, which orders node sheets by `NodeId` and
    /// inserts before the next higher id whatever its origin. After a swap
    /// freed node slots, the gutter node could reuse a slot below a later
    /// author `<style>`, and stylo panicked looking for that author sheet
    /// among the user-agent sheets. User-agent sheets are kept in their own
    /// ordered list, so this one is removed and appended instead.
    fn install_gutter_sheet(&mut self) {
        if let Some(old) = self.gutter_sheet.take() {
            self.doc.remove_user_agent_stylesheet(&old);
        }
        if self.gutters.is_empty() {
            return;
        }
        let sheet = UserAgentStylesheet::parse(&gutter_css(self.gutters.iter()));
        self.doc.add_user_agent_stylesheet(sheet.clone());
        self.gutter_sheet = Some(sheet);
    }

    /// Issue #26, after every layout: restore offsets recorded across a swap
    /// (by element id), then clamp every scroll container to its new
    /// `[0, max]`. Any change damages that container.
    fn settle_scroll_offsets(&mut self) {
        let restore = std::mem::take(&mut self.scroll_restore);
        for (id, y) in restore {
            if let Some(nid) = self.doc.get_element_by_id(&id) {
                if is_scroll_container(&self.doc, nid) {
                    self.apply_scroll(nid, y);
                }
            }
        }
        for nid in scroll_containers(&self.doc) {
            let off = scroll_offset_y(&self.doc, nid);
            let max = max_offset_y(&self.doc, nid);
            if off > max || off < 0.0 {
                self.apply_scroll(nid, off.clamp(0.0, max));
            }
        }
    }

    /// Issue #26: set a container's vertical offset (clamped by Blitz) and
    /// damage its border box. Returns whether it moved.
    fn apply_scroll(&mut self, nid: NodeId, y: f32) -> bool {
        let moved = set_offset_y(&mut self.doc, nid, y);
        if moved {
            let r = container_damage_rect(&self.doc, nid, self.layout_scale);
            self.push_pending_damage(r);
            self.mark_paint_dirty();
        }
        moved
    }

    fn scrolled_outcome(&self, nid: NodeId) -> InputOutcome {
        InputOutcome::Scrolled {
            id: self.element_id_of(nid),
        }
    }

    /// Issue #26: record offsets of scrolled elements with an id in the
    /// subtree a swap is about to replace (target included).
    fn record_scroll_offsets_in(&mut self, root: NodeId) {
        let mut stack = vec![root];
        while let Some(nid) = stack.pop() {
            let Some(node) = self.doc.get_node(nid) else {
                continue;
            };
            stack.extend(node.children.iter().copied());
            if !node.is_element() {
                continue;
            }
            let y = node.scroll_offset().y as f32;
            if y > 0.0 {
                if let Some(id) = node_id_attr(&self.doc, nid) {
                    self.scroll_restore.insert(id, y);
                }
            }
        }
    }

    /// Mark the DOM dirty without re-evaluating greying (issue #15). Used by
    /// the greying-write paths themselves (`apply_disabled_map`,
    /// `apply_input_rules`, `apply_changed_classes`) so they do not recurse
    /// into `recompute_greying`.
    fn mark_dom_dirty_raw(&mut self) {
        self.needs_resolve = true;
        self.ellipsis_cache.clear();
    }

    /// Mark the DOM dirty and, when any `RequiresChange` action or input rule
    /// is in play, re-evaluate greying / `hx-changed` locally (issue #15: no
    /// Game round trip per edit). Called after every value-changing mutation.
    fn mark_dom_dirty(&mut self) {
        self.mark_dom_dirty_raw();
        self.recompute_greying();
    }

    /// Re-evaluate `RequiresChange` / `EnabledWhen` / `hx-changed` against the
    /// current DOM, using cached slice data (no `SliceSource` call). Cheap
    /// no-op when none of the three is in use.
    fn recompute_greying(&mut self) {
        let has_requires_change = self
            .last_action_states
            .iter()
            .any(|(_, s)| matches!(s, ActionState::RequiresChange { .. }));
        if has_requires_change {
            let new_disabled = self.compute_disabled_map(&self.last_action_states.clone());
            self.apply_disabled_map(new_disabled, false);
            self.apply_all_changed_classes();
        }
        if !self.input_rules.is_empty() {
            self.apply_input_rules(false);
        }
        if !self.applied_control_nids.is_empty() {
            self.apply_standalone_changed_classes();
        }
    }

    /// Waiting on `complete()` (or a queued release the host has not taken yet).
    fn has_pending_work(&self) -> bool {
        !self.pending.by_source.is_empty()
            || !self.pending.queued.is_empty()
            || !self.released_outcomes.is_empty()
    }

    fn mark_paint_dirty(&mut self) {
        self.paint_dirty = true;
    }

    /// Single pending-damage list: clamp on push. Every source (swap old ink,
    /// closed overlay, fold full-surface, …) goes through here.
    fn push_pending_damage(&mut self, rect: IntRect) {
        let (sw, sh) = self.layout_viewport;
        let r = rect.clamp_to_surface(sw, sh);
        if !r.is_empty() {
            self.pending_damage.push(r);
            self.paint_dirty = true;
        }
    }

    /// Push last-painted ink for `root` and its current element descendants into
    /// `pending_damage`. Safe during sync (reads `last_bounds` only).
    fn queue_pending_subtree_damage(&mut self, root: NodeId) {
        if self.last_bounds.is_empty() {
            return;
        }
        let (sw, sh) = self.layout_viewport;
        let mut stack = vec![root];
        while let Some(nid) = stack.pop() {
            if let Some(bounds) = self.last_bounds.get(&nid) {
                if let Some(r) = ink_damage_rect(bounds.ink, sw, sh) {
                    self.push_pending_damage(r);
                }
            }
            let Some(node) = self.doc.get_node(nid) else {
                continue;
            };
            for &c in &node.children {
                stack.push(c);
            }
        }
    }

    /// Validate fragment HTML **before** applying a swap so a bad fragment is
    /// never silently inserted. Errors surface on `sync` via `EngineError`.
    ///
    /// `hx-target` / `hx-indicator` `#id` refs resolve against the document **as
    /// it will be after the swap**: current document ids, minus ids the swap
    /// removes, plus ids introduced by the fragment. Sibling indicators outside
    /// the swap target (e.g. `#tstrip-ind` next to `#tstrip-chips`) therefore
    /// resolve; ids that only lived inside the replaced subtree do not.
    fn validate_fragment_html(
        &self,
        declared_actions: &HashSet<String>,
        html: &str,
        target_nid: NodeId,
        swap: SwapMode,
    ) -> Result<(), EngineError> {
        // Validation-only parse (ids / dialect); never laid out or painted,
        // so it does not need the host fonts.
        let frag_doc = parse_html_document(html, self.layout_viewport, &HostFonts::default());
        let mut known_ids = collect_element_ids(&self.doc);
        for id in ids_removed_by_swap(&self.doc, target_nid, swap) {
            known_ids.remove(&id);
        }
        for id in collect_element_ids(&frag_doc) {
            known_ids.insert(id);
        }
        validate_loaded_tree(declared_actions, &frag_doc, &known_ids)
    }

    /// Re-index bindings after a DOM mutation (swap / attribute edits).
    fn reindex_bindings(&mut self, declared_actions: &HashSet<String>) -> Result<(), EngineError> {
        let (bindings, sections) = index_bindings(declared_actions, &self.doc)?;
        self.bindings = bindings;
        self.sections = sections;
        self.select_nids = collect_select_nids(&self.doc);
        self.applied_control_nids = collect_applied_control_nids(&self.doc);
        self.reconcile_open_select();
        self.reconcile_text_edit();
        Ok(())
    }

    fn sync(
        &mut self,
        declared_actions: &HashSet<String>,
        slices: &dyn SliceSource,
        frags: &mut dyn FragmentSource,
    ) -> Result<DirtySet, EngineError> {
        // Caret blink: only while a field is focused; unfocused pages stay idle.
        self.advance_caret_blink();
        // Issue #81: a held ▲/▼ repeats its step while the host keeps syncing.
        self.advance_spin_hold();
        // Always subscribe to `actions` for greying.
        let mut subscribed: HashSet<String> = HashSet::new();
        subscribed.insert("actions".into());
        for subs in self.sections.values() {
            for s in subs {
                subscribed.insert(s.clone());
            }
        }

        // Ask versions only for subscribed names.
        let mut current: HashMap<String, u64> = HashMap::new();
        for name in &subscribed {
            if let Some(v) = slices.version(name) {
                current.insert(name.clone(), v);
            }
        }

        let prev_actions_version = self.slice_versions.get("actions").copied();
        let initial = self.slice_versions.is_empty();
        let mut dirty_sections: Vec<String> = Vec::new();

        if initial {
            // One full render of every section right after load.
            let section_list: Vec<(String, Vec<String>)> = self
                .sections
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (section_id, subs) in section_list {
                let slice_refs: Vec<&str> = subs.iter().map(|s| s.as_str()).collect();
                let html = frags.render(&section_id, &slice_refs);
                self.swap_section(declared_actions, &section_id, &html)?;
                dirty_sections.push(section_id);
            }
        } else {
            let mut dirty_slices: HashSet<String> = HashSet::new();
            for (name, ver) in &current {
                let prev = self.slice_versions.get(name).copied();
                if prev != Some(*ver) {
                    dirty_slices.insert(name.clone());
                }
            }
            let section_list: Vec<(String, Vec<String>)> = self
                .sections
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (section_id, subs) in section_list {
                if subs.iter().any(|s| dirty_slices.contains(s)) {
                    let slice_refs: Vec<&str> = subs.iter().map(|s| s.as_str()).collect();
                    let html = frags.render(&section_id, &slice_refs);
                    self.swap_section(declared_actions, &section_id, &html)?;
                    dirty_sections.push(section_id);
                }
            }
        }

        let actions_changed = current.get("actions").copied() != prev_actions_version;

        // Issue #16: range/number limits from the `actions` slice. Cheap by
        // design (Game's request) — only re-pull `input_limits` on the
        // initial sync or when the `actions` slice version changes. A swap
        // elsewhere in this same sync still needs cached limits re-applied,
        // since fragment HTML never carries them (Designer lock: samples
        // omit min/max/step).
        //
        // PR27 round 2, blocker 3: this runs — and must fully succeed —
        // *before* `self.slice_versions = current` below. A failed
        // `refresh_input_limits` (`UnknownLimitInput` / `InvalidInputLimits`)
        // must not record the new `actions` version, so the next `sync`
        // sees `actions_changed` (or `initial`) again and re-calls
        // `input_limits` instead of silently skipping the retry.
        if initial || actions_changed {
            if let Err(err) = self.refresh_input_limits(slices) {
                // Fail closed: sections may already be swapped in above, so
                // give them the last good limits and this packet's greying
                // before reporting the error. The version is still not
                // recorded, so the next sync retries `input_limits`.
                // Crate-owner decision (PR #27 merge with #24): read this
                // packet's `input_rules` first, so the greying below never
                // uses the previous packet's rules.
                if !dirty_sections.is_empty() {
                    self.reapply_cached_limits();
                }
                self.input_rules = slices.input_rules();
                self.apply_action_states(slices.action_states(), true);
                return Err(err);
            }
        }

        self.slice_versions = current;

        // Issue #15: cache input rules (no re-poll per edit). Read after the
        // limits refresh (PR #27 merge with #24); the fail-closed error branch
        // above reads them itself before greying.
        self.input_rules = slices.input_rules();

        if !initial && !actions_changed && !dirty_sections.is_empty() {
            self.reapply_cached_limits();
        }

        // Apply actions-slice greying only when the desired state differs from
        // what is already on the DOM (or after a swap that replaced elements).
        // An unchanged actions slice must not write attributes or mark paint dirty.
        self.apply_action_states(
            slices.action_states(),
            !dirty_sections.is_empty() || initial,
        );

        // Count mutation damage before any resolve (A5). Style/layout run once
        // in `paint()` — sync and swaps only record changes (issue #8 step 1).
        let (damaged_node_count, total_element_count) = self.count_damage();

        // Belt-and-suspenders: a section swap must dirty paint even with no
        // hover (swap_section already marks; keep this so sync stays honest if
        // that path changes). Do NOT mark from `damaged_node_count` — Blitz can
        // leave stale damage flags after resolve, which made every clean sync
        // look dirty. Action-state writes mark dirty inside apply_action_states
        // when `node_updates` is non-empty.
        if !dirty_sections.is_empty() {
            self.mark_paint_dirty();
        }

        Ok(DirtySet {
            sections: dirty_sections,
            damaged_node_count,
            total_element_count,
        })
    }

    fn swap_section(
        &mut self,
        declared_actions: &HashSet<String>,
        section_id: &str,
        html: &str,
    ) -> Result<(), EngineError> {
        let binding = self
            .bindings
            .get(section_id)
            .cloned()
            .ok_or_else(|| EngineError::Internal(format!("unknown section `{section_id}`")))?;

        let target_sel = binding
            .hx_target
            .clone()
            .unwrap_or_else(|| format!("#{section_id}"));
        let target_id = self.resolve_selector(&target_sel)?;
        let swap = binding.hx_swap;

        // Same dialect rules as load_html; failure returns EngineError from sync
        // and leaves the previous DOM in place for this section.
        self.validate_fragment_html(declared_actions, html, target_id, swap)?;

        // Issue #15: a swap that replaces the form (or any of its controls)
        // resets the `RequiresChange` / `hx-changed` fallback baseline —
        // invalidate before mutating so the next evaluation recaptures it
        // from the post-swap values (unless `data-hx-applied` overrides it).
        self.invalidate_form_baselines_in(target_id);
        // Blocker 4 (Graph re-review, PR #24 round 2): same invalidate-
        // before-mutate treatment for rule-greyed element ids, so a
        // re-authored host `disabled` on the swapped-in markup is never
        // mistaken for a stale engine-owned grey.
        self.invalidate_rule_disabled_in(target_id);
        // Graph review of #102: the markup `min`/`max`/`step` snapshot of an
        // input this swap re-renders is retaken from its new markup, so a
        // range under `InputLimits::at_least` follows a new markup `max` and
        // removing the limits later restores the markup the game drew last.
        self.invalidate_limits_markup_in(target_id);

        // Record old subtree ink from the last paint snapshot (no resolve here —
        // sync must not run style/layout). compute_plan merges these so old
        // boxes survive NodeId recycle after the swap.
        self.queue_pending_subtree_damage(target_id);
        // Issue #26: an offset survives the swap when an element with the
        // same id does (restored + clamped at the next layout).
        self.record_scroll_offsets_in(target_id);
        // Issue #68 (Graph review of #69): only a swap that replaces the
        // focused field (or an ancestor) re-seeds its editor. Mark it before
        // mutating, while the old node is still in the tree: NodeIds recycle,
        // so `reconcile_text_edit` cannot tell a replacement from the id alone.
        let replaces_focused = self.text_edit.as_ref().is_some_and(|s| {
            let mut cur = Some(s.node_id);
            while let Some(nid) = cur {
                if nid == target_id {
                    return swap == SwapMode::OuterHtml || nid != s.node_id;
                }
                cur = self.doc.get_node(nid).and_then(|n| n.parent);
            }
            false
        });
        if replaces_focused {
            self.kept_typing = self.uncommitted_typing();
            if let Some(s) = self.text_edit.as_mut() {
                s.needs_prepare = true;
            }
        }

        let swapped_in: Vec<NodeId> = match swap {
            SwapMode::InnerHtml => {
                let mut mutr = self.doc.mutate();
                mutr.set_inner_html(target_id, html);
                drop(mutr);
                vec![target_id]
            }
            SwapMode::OuterHtml => {
                // Like the `outerHTML` setter: the fragment's context is the
                // target's parent, so a `<tr>` root parses under its
                // `<tbody>` and a root inside `<svg>` stays foreign. Without
                // an element parent, an HTML `<div>`.
                let context = self
                    .doc
                    .get_node(target_id)
                    .and_then(|n| n.parent)
                    .and_then(|p| self.doc.get_node(p))
                    .and_then(|p| p.element_data())
                    .map(|e| e.name.clone())
                    .filter(|q| !(q.ns == ns!(html) && q.local == local_name!("html")))
                    .unwrap_or_else(|| qual_name!("div", html));
                let wrapper_id = {
                    let mut mutr = self.doc.mutate();
                    // The wrapper is the fragment parser's context element,
                    // so it must be an HTML `<div>`. `qual_name!("div")`
                    // alone has no namespace, and html5ever then parses the
                    // fragment as foreign content: a root that is not a
                    // "breakout" tag (`<form>`, `<button>`, `<section>`, ...)
                    // and its children become namespace-less elements, so
                    // `<input>` is not void (it swallows its siblings) and no
                    // HTML user-agent style applies (#svc-tabs laid out 0 px
                    // tall with 0x0 buttons, #104).
                    let wrapper_id = mutr.create_element(context, vec![]);
                    mutr.set_inner_html(wrapper_id, html);
                    wrapper_id
                };
                let children: Vec<NodeId> = self
                    .doc
                    .get_node(wrapper_id)
                    .map(|n| n.children.to_vec())
                    .unwrap_or_default();
                let mut mutr = self.doc.mutate();
                if children.is_empty() {
                    mutr.remove_and_drop_node(target_id);
                } else {
                    mutr.replace_with_nodes(target_id, &children);
                }
                mutr.remove_and_drop_node(wrapper_id);
                drop(mutr);
                children
            }
        };
        // Issue #89: a redraw never overwrites a newer committed value.
        self.restore_catch_up_values(&swapped_in);

        self.mark_dom_dirty();
        self.mark_paint_dirty();
        // New number inputs get their editors on the next resolve; group then.
        self.group_display_pending = true;
        // Reindex after swap so new hx-* inside fragments are visible.
        self.reindex_bindings(declared_actions)?;
        Ok(())
    }

    fn resolve_selector(&self, sel: &str) -> Result<NodeId, EngineError> {
        let doc = &self.doc;
        let sel = sel.trim();
        if let Some(id) = sel.strip_prefix('#') {
            return doc
                .get_element_by_id(id)
                .ok_or_else(|| EngineError::Internal(format!("hx-target `#{id}` not found")));
        }
        if sel == "this" || sel.is_empty() {
            return Err(EngineError::Internal(
                "hx-target this without element context".into(),
            ));
        }
        doc.query_selector(sel)
            .map_err(|e| EngineError::Internal(format!("bad selector `{sel}`: {e:?}")))?
            .ok_or_else(|| EngineError::Internal(format!("hx-target `{sel}` matched nothing")))
    }

    /// Apply greying from the reserved `actions` slice (`Disabled` and
    /// `RequiresChange`, issue #15), then dependent-input `EnabledWhen` rules
    /// and `hx-changed` classes.
    ///
    /// `force_dom_pass` is set when section swaps (or the initial full render)
    /// may have replaced elements that still need the current greying written.
    fn apply_action_states(&mut self, states: Vec<(String, ActionState)>, force_dom_pass: bool) {
        self.last_action_states = states.clone();
        for (_, state) in &states {
            if let ActionState::RequiresChange { form, .. } = state {
                self.ensure_form_baseline(form);
            }
        }
        let new_disabled = self.compute_disabled_map(&states);
        self.apply_disabled_map(new_disabled, force_dom_pass);
        self.apply_input_rules(force_dom_pass);
        self.apply_all_changed_classes();
        self.apply_standalone_changed_classes();
    }

    /// Fold `Disabled` reasons with any currently-dirty `RequiresChange`
    /// reasons (issue #15) into one action-name -> reason map.
    fn compute_disabled_map(&self, states: &[(String, ActionState)]) -> HashMap<String, String> {
        let mut new_disabled: HashMap<String, String> = HashMap::new();
        for (name, state) in states {
            match state {
                ActionState::Disabled { reason } => {
                    new_disabled.insert(name.clone(), reason.clone());
                }
                ActionState::RequiresChange { form, reason } => {
                    if self.form_is_clean(form) {
                        new_disabled.insert(name.clone(), reason.clone());
                    }
                }
                ActionState::Enabled => {}
            }
        }
        new_disabled
    }

    /// Single seam for what counts as a form's `RequiresChange` / `hx-changed`
    /// baseline (issue #15). Per named control (document order):
    /// `data-hx-applied` (Designer lock 2026-10-20) wins when present;
    /// otherwise fall back to the value captured when the form was last
    /// loaded/re-rendered (`self.form_baselines`). Keeping the merge isolated
    /// here means a future change to the rule touches only this function.
    fn capture_form_baseline(&self, form_id: &str) -> Vec<(String, String)> {
        let controls = capture_form_controls(&self.doc, form_id);
        let fallback = self.form_baselines.get(form_id);
        controls
            .into_iter()
            .enumerate()
            .map(|(i, (_nid, name, current, applied))| {
                let value = applied.unwrap_or_else(|| {
                    fallback
                        .and_then(|f| f.get(i))
                        .map(|(_, v)| v.clone())
                        .unwrap_or(current)
                });
                (name, value)
            })
            .collect()
    }

    /// Plain current values (document order), ignoring `data-hx-applied` —
    /// used on the "live" side of the `RequiresChange` / `hx-changed` compare.
    fn capture_form_current(&self, form_id: &str) -> Vec<(String, String)> {
        capture_form_controls(&self.doc, form_id)
            .into_iter()
            .map(|(_nid, name, current, _applied)| (name, current))
            .collect()
    }

    /// Capture `form_id`'s fallback baseline if not already captured. A
    /// missing form captures nothing (treated as enabled elsewhere;
    /// debug-only warning).
    fn ensure_form_baseline(&mut self, form_id: &str) {
        if self.form_baselines.contains_key(form_id) {
            return;
        }
        if self.doc.get_element_by_id(form_id).is_none() {
            #[cfg(debug_assertions)]
            eprintln!(
                "hx_native: RequiresChange references unknown form `{form_id}`; action treated as enabled"
            );
            return;
        }
        let mut baseline = self.capture_form_current(form_id);
        // Issue #94 (Mainnet 5983ff17): a control with an unanswered commit
        // keeps the game's last value as its baseline, never the put-back,
        // so the form reads as changed until the game answers.
        if !self.catch_up.is_empty() {
            for (i, (nid, ..)) in capture_form_controls(&self.doc, form_id)
                .into_iter()
                .enumerate()
            {
                if let Some(e) = self
                    .catch_up
                    .iter()
                    .find(|e| self.doc.get_element_by_id(&e.control_id) == Some(nid))
                {
                    if let Some(slot) = baseline.get_mut(i) {
                        slot.1 = e.game.clone().unwrap_or_else(|| e.sent.clone());
                    }
                }
            }
        }
        self.form_baselines.insert(form_id.to_string(), baseline);
    }

    /// `true` (clean, so the action greys) when `form_id`'s live values equal
    /// its effective baseline. A form missing from the document, or one whose
    /// baseline was never captured, is treated as enabled (returns `false`).
    fn form_is_clean(&self, form_id: &str) -> bool {
        if self.doc.get_element_by_id(form_id).is_none() {
            return false;
        }
        if !self.form_baselines.contains_key(form_id) {
            return false;
        }
        self.capture_form_baseline(form_id) == self.capture_form_current(form_id)
    }

    /// Issue #15: forms containing an `[hx-post="action"]` control (deduped).
    fn forms_with_hx_post(&self, action: &str) -> HashSet<NodeId> {
        let mut out = HashSet::new();
        for b in self.bindings.values() {
            if b.hx_post.as_deref() != Some(action) {
                continue;
            }
            let Some(nid) = self.doc.get_element_by_id(&b.id) else {
                continue;
            };
            if let Some(form_nid) = closest_form(&self.doc, nid) {
                out.insert(form_nid);
            }
        }
        out
    }

    /// Write `new_disabled` onto every `[hx-post]` element (same three
    /// attributes as `Disabled`). Only writes diffs; no-op when the map is
    /// unchanged and no DOM pass is forced.
    fn apply_disabled_map(&mut self, new_disabled: HashMap<String, String>, force_dom_pass: bool) {
        let map_changed = new_disabled != self.disabled_actions;
        if !map_changed && !force_dom_pass {
            return;
        }

        // Desired reason per element with hx-post (None = enabled / clear greying).
        let desired: Vec<(String, NodeId, Option<String>)> = self
            .bindings
            .values()
            .filter_map(|b| {
                let action = b.hx_post.as_ref()?;
                let nid = self.doc.get_element_by_id(&b.id)?;
                let reason = new_disabled.get(action).cloned();
                Some((b.id.clone(), nid, reason))
            })
            .collect();

        // Compare against attributes already on the element; only write diffs.
        let mut node_updates: Vec<(NodeId, Option<String>)> = Vec::new();
        for (elem_id, nid, reason) in desired {
            let current_reason = self.attr(&elem_id, "data-hx-reason");
            let currently_disabled = self.attr(&elem_id, "disabled").is_some();
            let currently_aria = self.attr(&elem_id, "aria-disabled").as_deref() == Some("true");
            let already_matches = match &reason {
                Some(r) => {
                    currently_disabled
                        && currently_aria
                        && current_reason.as_deref() == Some(r.as_str())
                }
                // An element an InputRule currently greys stays greyed; the
                // rule pass owns clearing it (Graph review, PR #24 follow-up:
                // this pass must not undo greying it does not own).
                None if self.rule_disabled.contains_key(&elem_id) => true,
                None => !currently_disabled && !currently_aria && current_reason.is_none(),
            };
            if !already_matches {
                node_updates.push((nid, reason));
            }
        }

        self.disabled_actions = new_disabled;

        if node_updates.is_empty() {
            return;
        }

        self.write_greying_attrs(node_updates);
        self.mark_dom_dirty_raw();
        self.mark_paint_dirty();
        self.reconcile_open_select();
        self.reconcile_text_edit();
    }

    /// Issue #15: grey/ungrey dependent inputs per `self.input_rules`,
    /// combining correctly with `apply_disabled_map` (run first: an element
    /// that is both an `[hx-post]` target and a rule target stays greyed if
    /// either wants it greyed) and never touching a host-authored `disabled`
    /// (only elements this pass itself disabled, tracked in `rule_disabled`,
    /// are ever cleared).
    fn apply_input_rules(&mut self, force_dom_pass: bool) {
        // Desired: element id -> (node, reason).
        let mut desired: HashMap<String, (NodeId, String)> = HashMap::new();
        // Elements currently greyed by the actions slice (Disabled / clean
        // RequiresChange): element id -> action reason. Used both to skip a
        // genuinely host-authored `disabled` below and to hand a clearing
        // element back to the action's own greying instead of enabling it
        // outright (Graph review, PR #24 follow-up).
        let action_greyed: HashMap<String, String> = self
            .bindings
            .values()
            .filter_map(|b| {
                let r = self.disabled_actions.get(b.hx_post.as_ref()?)?;
                Some((b.id.clone(), r.clone()))
            })
            .collect();
        for (action, input_name, rule) in self.input_rules.clone() {
            let InputRule::EnabledWhen {
                input,
                checked,
                reason,
            } = &rule;
            for form_nid in self.forms_with_hx_post(&action) {
                let gate = checkbox_checked_by_name(&self.doc, form_nid, input);
                let violated = match gate {
                    Some(c) => c != *checked,
                    None => false,
                };
                if !violated {
                    continue;
                }
                for nid in named_controls_in_form(&self.doc, form_nid, &input_name) {
                    if let Some(eid) = self.element_id_of(nid) {
                        // Host-authored `disabled`: never touch it. An
                        // element already greyed by this pass or by the
                        // actions slice is not host-authored — only a
                        // `disabled` this engine did not itself set is.
                        let host_disabled = self.attr(&eid, "disabled").is_some()
                            && !self.rule_disabled.contains_key(&eid)
                            && !action_greyed.contains_key(&eid);
                        if host_disabled {
                            continue;
                        }
                        desired.insert(eid, (nid, reason.clone()));
                    }
                }
            }
        }

        let desired_reasons: HashMap<String, String> = desired
            .iter()
            .map(|(k, (_, r))| (k.clone(), r.clone()))
            .collect();
        let map_changed = desired_reasons != self.rule_disabled;
        // Keep going whenever anything is currently desired-greyed, even if
        // the desired-reasons map itself did not change: an earlier pass in
        // the same call (`apply_disabled_map`) may have wrongly cleared a
        // rule-greyed element before this pass got a chance to re-verify it
        // (Graph review, PR #24 follow-up) — the per-element `already_matches`
        // check below is what actually decides whether anything is rewritten.
        if !map_changed && !force_dom_pass && desired.is_empty() {
            return;
        }

        let mut node_updates: Vec<(NodeId, Option<String>)> = Vec::new();
        // Clear ids we previously greyed that are no longer desired. Hand
        // back to the actions slice if it still greys this element instead
        // of enabling it outright (Graph review, PR #24 follow-up).
        for elem_id in self.rule_disabled.keys() {
            if desired.contains_key(elem_id) {
                continue;
            }
            if let Some(nid) = self.doc.get_element_by_id(elem_id) {
                node_updates.push((nid, action_greyed.get(elem_id).cloned()));
            }
        }
        // Set/refresh ids that are desired and not already matching.
        for (elem_id, (nid, reason)) in &desired {
            let current_reason = self.attr(elem_id, "data-hx-reason");
            let currently_disabled = self.attr(elem_id, "disabled").is_some();
            let currently_aria = self.attr(elem_id, "aria-disabled").as_deref() == Some("true");
            let already_matches = currently_disabled
                && currently_aria
                && current_reason.as_deref() == Some(reason.as_str());
            if !already_matches {
                node_updates.push((*nid, Some(reason.clone())));
            }
        }

        self.rule_disabled = desired_reasons;

        if node_updates.is_empty() {
            return;
        }

        self.write_greying_attrs(node_updates);
        self.mark_dom_dirty_raw();
        self.mark_paint_dirty();
        self.reconcile_open_select();
        self.reconcile_text_edit();
    }

    // ------------------------------------------------------------------
    // Issue #16: range/number min/max/step from the `actions` slice.
    // ------------------------------------------------------------------

    /// Pull `SliceSource::input_limits`, validate it, write it onto every
    /// matching `<input type=range|number>`, clamp out-of-range values,
    /// remove limits for any key the new list no longer carries, and cache
    /// the result so later swaps can re-apply without calling `input_limits`
    /// again.
    ///
    /// PR27 round 2, blocker 3: every step here either fully applies or
    /// fully fails — on any error, nothing in this method has mutated the
    /// DOM, `limits_cache`, or `limits_markup` (the `?` on `validate_input_limits`
    /// runs before any DOM access at all; `apply_limits_to_doc` resolves
    /// every entry before writing any of them).
    fn refresh_input_limits(&mut self, slices: &dyn SliceSource) -> Result<(), EngineError> {
        let raw = slices.input_limits();
        for (action, input, limits) in &raw {
            validate_input_limits(action, input, limits)?;
        }
        self.apply_limits_to_doc(&raw, true)?;
        self.remove_stale_limits(&raw);
        self.limits_cache = raw;
        Ok(())
    }

    /// Re-apply the cached limits (no `input_limits` call). Best-effort: a
    /// swap's fresh fragment that no longer matches a cached entry is not an
    /// error here — only `refresh_input_limits` validates against the slice.
    fn reapply_cached_limits(&mut self) {
        if self.limits_cache.is_empty() {
            return;
        }
        let cached = self.limits_cache.clone();
        let _ = self.apply_limits_to_doc(&cached, false);
    }

    /// Write `entries` onto their matching inputs. `validate`: error
    /// (`UnknownLimitInput`) on an input name with no match inside a form
    /// that does exist for its action. Not an error when no form for the
    /// action is in the document at all (it may be on another surface, or
    /// not rendered yet).
    ///
    /// PR27 round 2, blocker 3: resolves **every** entry first, writing
    /// nothing, so a later entry's `UnknownLimitInput` can never leave an
    /// earlier entry half-applied (the old single-pass version wrote as it
    /// went and could error out partway through the list).
    fn apply_limits_to_doc(
        &mut self,
        entries: &[(String, String, InputLimits)],
        validate: bool,
    ) -> Result<(), EngineError> {
        let mut resolved: Vec<((String, String), NodeId, InputLimits)> = Vec::new();
        for (action, input_name, limits) in entries {
            let forms = self.forms_for_action(action);
            if forms.is_empty() {
                continue;
            }
            let mut matched = false;
            for form_nid in forms {
                for input_nid in self.range_or_number_inputs_named(form_nid, input_name) {
                    matched = true;
                    resolved.push((
                        (action.clone(), input_name.clone()),
                        input_nid,
                        limits.clone(),
                    ));
                }
            }
            if !matched && validate {
                return Err(EngineError::UnknownLimitInput {
                    action: action.clone(),
                    input: input_name.clone(),
                });
            }
        }

        let mut any_changed = false;
        for (key, input_nid, limits) in resolved {
            // Designer ruling (2026-10-22 12:45 America/Asuncion, #101): a
            // range must have a real max. `at_least` (no max) on a range is
            // refused: nothing is applied, the input keeps (or gets back) its
            // markup `min`/`max`/`step`, and the refusal is logged with the
            // element's id on each `input_limits()` refresh.
            if limits.max.is_none() && self.doc.get_node(input_nid).is_some_and(is_range_input) {
                if validate {
                    let id = node_id_attr(&self.doc, input_nid).unwrap_or_default();
                    eprintln!(
                        "hx_native: refused InputLimits::at_least (no max) for range input                          id=`{id}` (action `{}`, input `{}`): a range must have a real max;                          it keeps its markup min/max/step",
                        key.0, key.1
                    );
                }
                if let Some(snap) = self.limits_markup.get(&key).cloned() {
                    if self.restore_markup_onto(input_nid, Some(&snap)) {
                        any_changed = true;
                    }
                }
                continue;
            }
            if !self.limits_markup.contains_key(&key) {
                let snap = MarkupSnapshot {
                    min: node_attr(&self.doc, input_nid, "min"),
                    max: node_attr(&self.doc, input_nid, "max"),
                    step: node_attr(&self.doc, input_nid, "step"),
                };
                self.limits_markup.insert(key.clone(), snap);
            }
            if self.write_limits_onto(input_nid, &limits) {
                any_changed = true;
            }
        }
        if any_changed {
            self.mark_dom_dirty();
            self.mark_paint_dirty();
        }
        Ok(())
    }

    /// Restore (or clear) `min`/`max`/`step` for every `(action, input name)`
    /// key that `self.limits_cache` carried but `raw` no longer does (PR27
    /// round 2, non-blocking item): the slice stopped declaring limits for
    /// that input, so the engine's override must stop too. Restores the
    /// markup snapshot taken the first time the engine wrote over it. With
    /// no snapshot the engine never wrote over the current markup, so the
    /// input is left as it is.
    fn remove_stale_limits(&mut self, raw: &[(String, String, InputLimits)]) {
        let new_keys: HashSet<(String, String)> =
            raw.iter().map(|(a, i, _)| (a.clone(), i.clone())).collect();
        let removed: Vec<(String, String)> = self
            .limits_cache
            .iter()
            .map(|(a, i, _)| (a.clone(), i.clone()))
            .filter(|k| !new_keys.contains(k))
            .collect();
        if removed.is_empty() {
            return;
        }
        let mut any_changed = false;
        for key in removed {
            // Graph review of #102: no snapshot means the engine has not
            // written over this input's current markup (a refused range, or
            // a swap re-rendered it since), so there is nothing to undo, and
            // clearing would wipe the markup `min`/`max`/`step`.
            let Some(snap) = self.limits_markup.remove(&key) else {
                continue;
            };
            let (action, input_name) = key;
            for form_nid in self.forms_for_action(&action) {
                for input_nid in self.range_or_number_inputs_named(form_nid, &input_name) {
                    if self.restore_markup_onto(input_nid, Some(&snap)) {
                        any_changed = true;
                    }
                }
            }
        }
        if any_changed {
            self.mark_dom_dirty();
            self.mark_paint_dirty();
        }
    }

    /// Set `min`/`max`/`step` back to `snap` (or clear them when `snap` is
    /// `None` / a field was absent in markup). Returns `true` when any
    /// attribute changed.
    fn restore_markup_onto(&mut self, nid: NodeId, snap: Option<&MarkupSnapshot>) -> bool {
        let want_min = snap.and_then(|s| s.min.clone());
        let want_max = snap.and_then(|s| s.max.clone());
        let want_step = snap.and_then(|s| s.step.clone());
        let cur_min = node_attr(&self.doc, nid, "min");
        let cur_max = node_attr(&self.doc, nid, "max");
        let cur_step = node_attr(&self.doc, nid, "step");
        let mut changed = false;
        {
            let mut mutr = self.doc.mutate();
            if cur_min != want_min {
                match &want_min {
                    Some(v) => mutr.set_attribute(nid, qual_name!("min"), v),
                    None => mutr.clear_attribute(nid, qual_name!("min")),
                }
                changed = true;
            }
            if cur_max != want_max {
                match &want_max {
                    Some(v) => mutr.set_attribute(nid, qual_name!("max"), v),
                    None => mutr.clear_attribute(nid, qual_name!("max")),
                }
                changed = true;
            }
            if cur_step != want_step {
                match &want_step {
                    Some(v) => mutr.set_attribute(nid, qual_name!("step"), v),
                    None => mutr.clear_attribute(nid, qual_name!("step")),
                }
                changed = true;
            }
        }
        changed
    }

    /// Forms (by `NodeId`) that are, or contain, an element whose `hx-post`
    /// names `action`. Dedup-preserving; empty when `action` has no
    /// `hx-post` element anywhere in this document.
    fn forms_for_action(&self, action: &str) -> Vec<NodeId> {
        let mut forms = Vec::new();
        for b in self.bindings.values() {
            if b.hx_post.as_deref() != Some(action) {
                continue;
            }
            let Some(nid) = self.doc.get_element_by_id(&b.id) else {
                continue;
            };
            let is_form_itself = self
                .doc
                .get_node(nid)
                .and_then(|n| n.element_data())
                .is_some_and(|el| &*el.name.local == "form");
            let form_nid = if is_form_itself {
                Some(nid)
            } else {
                closest_form(&self.doc, nid)
            };
            if let Some(f) = form_nid {
                if !forms.contains(&f) {
                    forms.push(f);
                }
            }
        }
        forms
    }

    /// `<input type=range|number name=name>` descendants of `form_nid`.
    fn range_or_number_inputs_named(&self, form_nid: NodeId, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![form_nid];
        while let Some(nid) = stack.pop() {
            let Some(node) = self.doc.get_node(nid) else {
                continue;
            };
            for &c in node.children.iter().rev() {
                stack.push(c);
            }
            if is_numeric_input(node) && node_attr(&self.doc, nid, "name").as_deref() == Some(name)
            {
                out.push(nid);
            }
        }
        out
    }

    /// Write `min`/`max`/`step` onto `nid` (slice wins over markup; a
    /// debug-only warning logs when markup already disagreed) and
    /// clamp/snap its current value. Returns `true` when any attribute
    /// (including `value`) changed. Never fires `change` / `Action` — a
    /// clamp is a local value change only, never an edit the host sees.
    ///
    /// Issue #101: `limits.max: None` (`InputLimits::at_least`) writes no
    /// `max` on a number box and clears one from markup (the slice wins),
    /// so it clamps at `min` only. A range never gets here with no max
    /// (`apply_limits_to_doc` refuses it, Designer ruling).
    fn write_limits_onto(&mut self, nid: NodeId, limits: &InputLimits) -> bool {
        let min_s = format_numeric(limits.min);
        let want_max: Option<String> = limits.max.map(format_numeric);
        let clamp_max = limits.max;
        let step_s = limits
            .step
            .map(format_numeric)
            .unwrap_or_else(|| "any".to_string());

        let cur_min = node_attr(&self.doc, nid, "min");
        let cur_max = node_attr(&self.doc, nid, "max");
        let cur_step = node_attr(&self.doc, nid, "step");
        let is_number = self.doc.get_node(nid).is_some_and(is_number_input);

        #[cfg(debug_assertions)]
        {
            let markup_disagrees = (cur_min.is_some()
                && cur_min.as_deref() != Some(min_s.as_str()))
                || (cur_max.is_some() && cur_max != want_max)
                || (cur_step.is_some() && cur_step.as_deref() != Some(step_s.as_str()));
            if markup_disagrees {
                eprintln!(
                    "hx_native: actions-slice limits override markup min/max/step \
                     (slice is the source of truth, issue #16)"
                );
            }
        }

        let mut changed = false;
        {
            let mut mutr = self.doc.mutate();
            if cur_min.as_deref() != Some(min_s.as_str()) {
                mutr.set_attribute(nid, qual_name!("min"), &min_s);
                changed = true;
            }
            if cur_max != want_max {
                match &want_max {
                    Some(v) => mutr.set_attribute(nid, qual_name!("max"), v),
                    None => mutr.clear_attribute(nid, qual_name!("max")),
                }
                changed = true;
            }
            if cur_step.as_deref() != Some(step_s.as_str()) {
                mutr.set_attribute(nid, qual_name!("step"), &step_s);
                changed = true;
            }
        }

        let clamp_limits = NumericLimits {
            min: Some(limits.min),
            max: clamp_max,
            step: limits.step.unwrap_or(1.0),
            unconstrained: limits.step.is_none(),
        };
        let value_attr = node_attr(&self.doc, nid, "value");
        // Blocker 4 (PR27 round 2): an empty number field stays empty —
        // matches how HTML treats an empty `<input type=number>` (no value
        // to clamp). Range inputs cannot be meaningfully empty, so they keep
        // falling back to `limits.min` as before.
        let is_empty_number = is_number && value_attr.as_deref().unwrap_or("").trim().is_empty();
        // A limits refresh never rewrites the box being edited (Designer
        // call, #41 round 2): the new min/max/step attributes are already
        // written above, so they apply on its commit (Enter or blur).
        let focused_number = is_number && self.text_edit.as_ref().is_some_and(|s| s.node_id == nid);
        if !is_empty_number && !focused_number {
            let current = value_attr.and_then(|s| parse_f64(&s)).unwrap_or(limits.min);
            // Number inputs hold typed values, which clamp to [min, max] but
            // never snap to the step grid (issue #39); ranges keep the snap.
            let clamped = if is_number {
                clamp_value(current, &clamp_limits)
            } else {
                clamp_snap_value(current, &clamp_limits)
            };
            let clamped_s = format_numeric(clamped);
            if write_value_attr(&mut self.doc, nid, &clamped_s) {
                changed = true;
                let focused_here = self.text_edit.as_ref().is_some_and(|s| s.node_id == nid);
                if is_number && !focused_here {
                    apply_unfocused_display(&mut self.doc, nid);
                }
                if let Some(elem_id) = node_id_attr(&self.doc, nid) {
                    let _ = sync_outputs_for(&mut self.doc, &elem_id);
                }
                if is_number {
                    self.queue_text_field_damage(nid);
                } else {
                    self.mark_paint_dirty();
                }
            }
        }
        // A new `step` alone changes what the box shows (the step's decimals,
        // Designer lock, #49 round 2): step 1 -> 0.01 shows `4.00`, 0.01 -> 1
        // shows `4.1`, even when the value did not move (Graph review of #49 r2).
        if changed && is_number && !focused_number {
            let before = editor_text(&mut self.doc, nid);
            apply_unfocused_display(&mut self.doc, nid);
            if editor_text(&mut self.doc, nid) != before {
                self.queue_text_field_damage(nid);
            }
        }
        changed
    }

    /// Shared attribute write for both action-level and input-rule greying:
    /// `disabled` + `aria-disabled="true"` + `data-hx-reason` when `Some`,
    /// cleared when `None`.
    fn write_greying_attrs(&mut self, node_updates: Vec<(NodeId, Option<String>)>) {
        let mut mutr = self.doc.mutate();
        for (elem_id, reason) in node_updates {
            match reason {
                Some(r) => {
                    mutr.set_attribute(elem_id, qual_name!("disabled"), "");
                    mutr.set_attribute(
                        elem_id,
                        QualName::new(None, ns!(), LocalName::from("aria-disabled")),
                        "true",
                    );
                    mutr.set_attribute(
                        elem_id,
                        QualName::new(None, ns!(), LocalName::from("data-hx-reason")),
                        &r,
                    );
                }
                None => {
                    mutr.clear_attribute(elem_id, qual_name!("disabled"));
                    mutr.clear_attribute(
                        elem_id,
                        QualName::new(None, ns!(), LocalName::from("aria-disabled")),
                    );
                    mutr.clear_attribute(
                        elem_id,
                        QualName::new(None, ns!(), LocalName::from("data-hx-reason")),
                    );
                }
            }
        }
    }

    fn element_id_of(&self, nid: NodeId) -> Option<String> {
        let node = self.doc.get_node(nid)?;
        node.attrs().and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.local == "id")
                .map(|a| a.value.to_string())
                .filter(|s| !s.is_empty())
        })
    }

    /// Issue #15: drop any fallback baseline for a form inside `target_id`'s
    /// pre-swap subtree (or `target_id` itself), so the next evaluation
    /// recaptures it fresh from the post-swap values (unless overridden by
    /// `data-hx-applied` on a given control).
    fn invalidate_form_baselines_in(&mut self, target_id: NodeId) {
        if self.form_baselines.is_empty() {
            return;
        }
        let stale: Vec<String> = self
            .form_baselines
            .keys()
            .filter(|form_id| {
                self.doc
                    .get_element_by_id(form_id)
                    .is_some_and(|nid| nid == target_id || self.is_descendant(target_id, nid))
            })
            .cloned()
            .collect();
        for id in stale {
            self.form_baselines.remove(&id);
        }
    }

    /// Blocker 4 (Graph re-review, PR #24 round 2): drop any `rule_disabled`
    /// entry for an element id inside `target_id`'s pre-swap subtree (or
    /// `target_id` itself), so the first `apply_input_rules` pass after the
    /// swap reads the new markup's `disabled` (if any) as host-authored,
    /// not as a stale engine-owned grey left over from that id's pre-swap
    /// occupant. Mirrors `invalidate_form_baselines_in`'s pre-mutation
    /// invalidation (same filter, same call site). Without this, `dep`'s
    /// `host_disabled` check in `apply_input_rules`
    /// (`attr(disabled) && !rule_disabled.contains_key && …`) sees the
    /// stale `rule_disabled` entry for a freshly-swapped-in element that
    /// now carries a genuinely host-authored `disabled`, concludes it is
    /// *not* host-authored, takes ownership, and later strips it (along
    /// with `data-hx-reason`) the next time the rule clears.
    ///
    /// `disabled_actions` needs no equivalent: it is keyed by **action
    /// name**, not element id (`apply_disabled_map` always fully
    /// reassigns it from a fresh recompute every pass), and the
    /// action-name → element-id lookup it feeds goes through
    /// `self.bindings`, which `reindex_bindings` already rebuilds from
    /// scratch on every swap.
    fn invalidate_rule_disabled_in(&mut self, target_id: NodeId) {
        if self.rule_disabled.is_empty() {
            return;
        }
        let stale: Vec<String> = self
            .rule_disabled
            .keys()
            .filter(|elem_id| {
                self.doc
                    .get_element_by_id(elem_id)
                    .is_some_and(|nid| nid == target_id || self.is_descendant(target_id, nid))
            })
            .cloned()
            .collect();
        for id in stale {
            self.rule_disabled.remove(&id);
        }
    }

    /// Drop the `limits_markup` snapshot of every `(action, input)` whose
    /// inputs all sit inside `target_id` (the swap replaces their markup).
    /// A key with an input outside the swap keeps its snapshot: that input
    /// still carries the engine's attributes, not markup.
    fn invalidate_limits_markup_in(&mut self, target_id: NodeId) {
        if self.limits_markup.is_empty() {
            return;
        }
        let stale: Vec<(String, String)> = self
            .limits_markup
            .keys()
            .filter(|(action, input)| {
                let inputs: Vec<NodeId> = self
                    .forms_for_action(action)
                    .into_iter()
                    .flat_map(|f| self.range_or_number_inputs_named(f, input))
                    .collect();
                !inputs.is_empty()
                    && inputs
                        .iter()
                        .all(|&n| n == target_id || self.is_descendant(target_id, n))
            })
            .cloned()
            .collect();
        for key in stale {
            self.limits_markup.remove(&key);
        }
    }

    fn is_descendant(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut cur = self.doc.get_node(node).and_then(|n| n.parent);
        while let Some(pid) = cur {
            if pid == ancestor {
                return true;
            }
            cur = self.doc.get_node(pid).and_then(|n| n.parent);
        }
        false
    }

    /// Issue #15 (Graph requirement): refresh `hx-changed` on every control in
    /// every form currently referenced by a `RequiresChange` action (same
    /// forms `self.form_baselines` tracks). Scoped this way per the cost
    /// rule: only forms with `RequiresChange` are walked.
    fn apply_all_changed_classes(&mut self) {
        let mut forms: Vec<String> = Vec::new();
        for (_, state) in &self.last_action_states {
            if let ActionState::RequiresChange { form, .. } = state {
                if !forms.contains(form) {
                    forms.push(form.clone());
                }
            }
        }
        for form in forms {
            self.apply_changed_classes(&form);
        }
    }

    /// Add/remove class `hx-changed` on each named control in `form_id`
    /// whenever its current value differs from its effective baseline (same
    /// comparison as `RequiresChange`). Only mutates the class when the
    /// per-control changed state actually flips vs. what the live DOM already
    /// shows — never re-sets it on an edit that keeps the same state — so
    /// restyle/damage from a flip stays local to that one control.
    fn apply_changed_classes(&mut self, form_id: &str) {
        let controls = capture_form_controls(&self.doc, form_id);
        if controls.is_empty() {
            return;
        }
        let fallback = self.form_baselines.get(form_id).cloned();
        let mut updates: Vec<(NodeId, bool)> = Vec::new();
        for (i, (nid, _name, current, applied)) in controls.iter().enumerate() {
            let baseline_value = applied.clone().unwrap_or_else(|| {
                fallback
                    .as_ref()
                    .and_then(|f| f.get(i))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| current.clone())
            });
            let desired_changed = *current != baseline_value;
            if self.node_has_class(*nid, HX_CHANGED_CLASS) != desired_changed {
                updates.push((*nid, desired_changed));
            }
        }
        if updates.is_empty() {
            return;
        }
        for (nid, on) in updates {
            self.toggle_class(nid, HX_CHANGED_CLASS, on);
            self.hx_changed_write_count += 1;
        }
        self.mark_dom_dirty_raw();
        self.mark_paint_dirty();
    }

    /// Issue #24 (Designer lock 2026-10-20): `hx-changed` for every control
    /// carrying `data-hx-applied`, independent of any enclosing form or
    /// `RequiresChange` action — the attribute alone opts a control in (a
    /// plan row with no button of its own included). There is no fallback
    /// baseline here: being in `self.applied_control_nids` means the
    /// attribute is present, so it is always the comparison baseline (same
    /// override rule as `apply_changed_classes`, just with nothing to fall
    /// back to). A control also covered by a `RequiresChange` form's pass
    /// computes the identical decision there, so the flip-check keeps this
    /// idempotent — no double write.
    fn apply_standalone_changed_classes(&mut self) {
        let controls = capture_applied_controls(&self.doc, &self.applied_control_nids);
        if controls.is_empty() {
            return;
        }
        let mut updates: Vec<(NodeId, bool)> = Vec::new();
        for (nid, current, applied) in &controls {
            let desired_changed = current != applied;
            if self.node_has_class(*nid, HX_CHANGED_CLASS) != desired_changed {
                updates.push((*nid, desired_changed));
            }
        }
        if updates.is_empty() {
            return;
        }
        for (nid, on) in updates {
            self.toggle_class(nid, HX_CHANGED_CLASS, on);
            self.hx_changed_write_count += 1;
        }
        self.mark_dom_dirty_raw();
        self.mark_paint_dirty();
    }

    fn node_class(&self, nid: NodeId) -> String {
        self.doc
            .get_node(nid)
            .and_then(|n| n.attrs())
            .and_then(|attrs| attrs.iter().find(|a| &*a.name.local == "class"))
            .map(|a| a.value.to_string())
            .unwrap_or_default()
    }

    fn node_has_class(&self, nid: NodeId, class: &str) -> bool {
        class_tokens(&self.node_class(nid)).any(|t| t == class)
    }

    /// Preserve any host classes on the element; only add/remove `class`.
    fn toggle_class(&mut self, nid: NodeId, class: &str, on: bool) {
        let current = self.node_class(nid);
        let new_value = if on {
            add_class_token(&current, class)
        } else {
            remove_class_token(&current, class)
        };
        let mut mutr = self.doc.mutate();
        if new_value.is_empty() {
            mutr.clear_attribute(nid, qual_name!("class"));
        } else {
            mutr.set_attribute(nid, qual_name!("class"), &new_value);
        }
    }

    fn count_damage(&self) -> (usize, usize) {
        let doc = &self.doc;
        let mut damaged = 0usize;
        let mut total = 0usize;
        let root = doc.root_node().id;
        let mut stack = vec![root];
        while let Some(nid) = stack.pop() {
            let Some(node) = doc.get_node(nid) else {
                continue;
            };
            for &c in &node.children {
                stack.push(c);
            }
            if node.is_element() {
                total += 1;
                if node.damage().is_some_and(|d| !d.is_empty()) || node.has_damaged_descendants() {
                    damaged += 1;
                }
            }
        }
        (damaged, total)
    }

    fn ensure_resolved(&mut self) {
        if self.needs_resolve {
            // Graph review (PR #24 follow-up): read restyle damage before
            // this input-time resolve clears it, so attribute/class writes
            // (greying, hx-changed, checked) made before the next paint
            // still reach that paint's damage plan. Without this, the full
            // `resolve_doc` below consumes Blitz's one-shot restyle flags,
            // and `resolve_and_plan_damage`'s own `resolve_stylist` probe at
            // paint time finds nothing left to report.
            self.doc.resolve_stylist(0.0);
            let carried = collect_style_damaged(&self.doc);
            self.carried_style_damage.extend(carried);
            let _ = self.resolve_doc();
            self.needs_resolve = std::mem::take(&mut self.gutter_followup);
        }
        self.apply_pending_group_display();
    }

    fn input(&mut self, ctx: &mut ActionCtx<'_>, ev: InputEvent) -> InputOutcome {
        // Issue #88: every pointer event hit-tests the layout, so it first
        // resolves the DOM as it stands. A `sync` may have swapped sections
        // since the last paint; a release used to hit-test form controls
        // against the pre-swap layout (a stale click, or a panic in blitz-dom
        // when the pressed node was removed). The resolve carries its restyle
        // damage to the next paint (`ensure_resolved`), and that paint diffs
        // geometry against the last painted bounds, so partial repaint still
        // equals full repaint.
        if matches!(
            ev,
            InputEvent::PointerMove { .. }
                | InputEvent::PointerDown { .. }
                | InputEvent::PointerUp { .. }
                | InputEvent::Wheel { .. }
        ) {
            self.ensure_resolved();
        }
        match ev {
            InputEvent::Text(s) => self.on_text(s, false),
            InputEvent::Paste(s) => self.on_paste(s),
            InputEvent::Copy => self.on_copy(false),
            InputEvent::Cut => self.on_copy(true),
            InputEvent::PointerLeave => {
                // Graph review of #84: a host that loses the pointer (Mainnet
                // drops a release outside the panel and sends PointerLeave)
                // ends a held arrow like a release elsewhere (Q3): the repeat
                // stops and the one `change` is queued, never a repeat that
                // runs on with no release to stop it.
                if self.press_is_spin_arrow() {
                    let arrow = self.press.take().map(|p| p.id).unwrap_or_default();
                    let out = self.release_spin_hold(ctx, &arrow);
                    if !matches!(out, InputOutcome::None) {
                        self.released_outcomes.push(out);
                    }
                }
                self.on_pointer_leave()
            }
            InputEvent::PointerMove { x, y, .. } => self.on_pointer_move(x, y),
            InputEvent::PointerDown { x, y, button, mods } => {
                self.on_pointer_down(ctx, x, y, button, mods)
            }
            InputEvent::PointerUp { x, y, button, mods } => {
                self.on_pointer_up(ctx, x, y, button, mods)
            }
            InputEvent::Key(key) => self.on_key(ctx, key),
            InputEvent::Wheel { pos, delta, unit } => self.on_wheel(pos, delta, unit),
        }
    }

    /// Issue #26: wheel scrolls the innermost container under the pointer
    /// that can move that way. No chaining of a remainder to a parent.
    fn on_wheel(&mut self, pos: (f32, f32), delta: (f32, f32), unit: WheelUnit) -> InputOutcome {
        let (x, y) = pos;
        if !delta.1.is_finite() || delta.1 == 0.0 {
            return InputOutcome::None;
        }
        if self.open_select.is_some() {
            self.close_select();
        }
        for nid in containers_at(&self.doc, x, y) {
            let dy = match unit {
                WheelUnit::Pixel => -delta.1,
                WheelUnit::Line => -delta.1 * SCROLL_LINE_ROWS * SCROLL_ROW_PX,
                WheelUnit::Page => -delta.1 * page_px(&self.doc, nid),
            };
            if !can_scroll(&self.doc, nid, dy) {
                continue;
            }
            let target = scroll_offset_y(&self.doc, nid) + dy;
            if self.apply_scroll(nid, target) {
                return self.scrolled_outcome(nid);
            }
        }
        InputOutcome::None
    }

    /// Issue #26: primary press on an engine-drawn scrollbar.
    fn on_scrollbar_down(&mut self, geom: BarGeom, part: BarPart, y: f32) -> InputOutcome {
        let nid = geom.nid;
        self.scroll_press_active = true;
        self.press = None;
        let row = SCROLL_ROW_PX;
        let page = page_px(&self.doc, nid);
        match part {
            BarPart::Up | BarPart::Down => {
                self.scroll_pressed = Some(PressedArrow { nid, part });
                self.mark_paint_dirty();
                let d = if part == BarPart::Up { -row } else { row };
                self.apply_scroll(nid, geom.offset + d);
            }
            BarPart::TrackAbove => {
                self.apply_scroll(nid, geom.offset - page);
            }
            BarPart::TrackBelow => {
                self.apply_scroll(nid, geom.offset + page);
            }
            BarPart::Thumb => {
                let top = geom.thumb.map(|t| t.y).unwrap_or(geom.track.y);
                self.scroll_drag = Some(ScrollDrag { nid, grab: y - top });
            }
        }
        self.scrolled_outcome(nid)
    }

    /// Issue #26: thumb drag move. Offset follows the thumb top.
    fn on_scroll_drag_move(&mut self, drag: ScrollDrag, y: f32) -> InputOutcome {
        let Some(geom) = bar_geometry(&self.doc, drag.nid) else {
            return InputOutcome::None;
        };
        let target = geom.offset_for_thumb_top(y - drag.grab);
        if self.apply_scroll(drag.nid, target) {
            self.scrolled_outcome(drag.nid)
        } else {
            InputOutcome::None
        }
    }

    /// Issue #26: end a scrollbar press (arrow visual, thumb drag).
    fn end_scroll_press(&mut self) {
        if self.scroll_pressed.take().is_some() {
            self.mark_paint_dirty();
        }
        self.scroll_drag = None;
        self.scroll_press_active = false;
    }

    /// Issue #26: PageUp / PageDown / Home / End move the innermost scroll
    /// container around the focused element that can move that way.
    fn try_scroll_keys(&mut self, key: &KeyEvent) -> Option<InputOutcome> {
        if !key.pressed || key.mods.ctrl || key.mods.alt {
            return None;
        }
        if !matches!(key.key.as_str(), "PageUp" | "PageDown" | "Home" | "End") {
            return None;
        }
        self.ensure_resolved();
        let focused = self
            .focus
            .focused_id
            .as_deref()
            .and_then(|id| self.doc.get_element_by_id(id))?;
        // The focused element may itself be the container (tabindex).
        let mut chain = scroll_ancestors(&self.doc, focused);
        if is_scroll_container(&self.doc, focused) {
            chain.insert(0, focused);
        }
        for nid in chain {
            let off = scroll_offset_y(&self.doc, nid);
            let max = max_offset_y(&self.doc, nid);
            let target = match key.key.as_str() {
                "PageUp" => off - page_px(&self.doc, nid),
                "PageDown" => off + page_px(&self.doc, nid),
                "Home" => 0.0,
                _ => max,
            };
            if (target.clamp(0.0, max) - off).abs() < 0.01 {
                continue;
            }
            if self.apply_scroll(nid, target) {
                return Some(self.scrolled_outcome(nid));
            }
        }
        None
    }

    /// Issue #26: scroll each container around `nid` (innermost first) the
    /// least amount that shows the element ("nearest"; a box taller than the
    /// container aligns its top).
    fn scroll_into_view(&mut self, nid: NodeId) {
        self.ensure_resolved();
        for anc in scroll_ancestors(&self.doc, nid) {
            let (Some(el), Some(pb)) = (
                element_border_box_css(&self.doc, nid),
                scroll_padding_box(&self.doc, anc),
            ) else {
                continue;
            };
            let (top, h) = (el.1, el.3);
            let off = scroll_offset_y(&self.doc, anc);
            let target = if top < pb.y || h > pb.h {
                off - (pb.y - top)
            } else if top + h > pb.y + pb.h {
                off + (top + h - (pb.y + pb.h))
            } else {
                continue;
            };
            self.apply_scroll(anc, target);
        }
    }

    fn hit_id(&mut self, x: f32, y: f32) -> Option<String> {
        self.ensure_resolved();
        let doc = &self.doc;
        let nid = visible_element_from_point(doc, x, y)?;
        // Walk up to nearest element with an id that we know about, else any id.
        let mut cur = Some(nid);
        while let Some(id) = cur {
            let node = doc.get_node(id)?;
            if let Some(eid) = node.attrs().and_then(|attrs| {
                attrs
                    .iter()
                    .find(|a| &*a.name.local == "id")
                    .map(|a| a.value.to_string())
            }) {
                if !eid.is_empty() {
                    return Some(eid);
                }
            }
            cur = node.parent;
        }
        None
    }

    /// Forward pointer leave to Blitz so `:hover` styles and CSS tooltips clear.
    fn on_pointer_leave(&mut self) -> InputOutcome {
        self.end_scroll_press();
        self.press = None;
        self.range_drag_id = None;
        self.range_drag_released = None;
        self.select_hover_row = None;
        if self.doc.clear_hover() {
            self.mark_paint_dirty();
        }
        self.hover_id = None;
        InputOutcome::Hover { id: None }
    }

    /// Forward pointer move to Blitz `:hover` state. Paint is marked dirty only
    /// when the hovered element actually changes, so a still pointer keeps the
    /// clean-frame skip.
    fn on_pointer_move(&mut self, x: f32, y: f32) -> InputOutcome {
        if let Some(drag) = self.scroll_drag {
            return self.on_scroll_drag_move(drag, y);
        }
        if let Some(ref rid) = self.range_drag_id.clone() {
            if !self.range_paging && !self.element_is_disabled_id(rid) {
                let _ = self.apply_range_from_x(rid, x);
            }
        }
        // Open select: track hovered row for navy highlight; damage only old/new rows.
        if let Some(ref open) = self.open_select.clone() {
            let n = select_options(&self.doc, open.select_nid).len();
            let row = list_row_at(open, x, y, n);
            if row != self.select_hover_row {
                let scale = self.layout_scale;
                let old = self.select_hover_row.or(Some(open.highlight)).unwrap_or(0);
                self.push_pending_damage(select_row_damage_rect(open, old, scale));
                if let Some(r) = row {
                    self.push_pending_damage(select_row_damage_rect(open, r, scale));
                    if let Some(os) = self.open_select.as_mut() {
                        os.highlight = r;
                    }
                }
                self.select_hover_row = row;
                self.mark_paint_dirty();
            }
        }
        let (vw, vh) = self.layout_viewport;
        let out_of_bounds = x < 0.0 || y < 0.0 || x >= vw as f32 || y >= vh as f32;

        let hover_changed = if out_of_bounds {
            self.doc.clear_hover()
        } else {
            self.doc.set_hover_to(x, y)
        };
        if hover_changed {
            self.mark_paint_dirty();
        }

        let id = if out_of_bounds {
            None
        } else {
            self.hit_id(x, y)
        };
        if id != self.hover_id {
            self.hover_id = id.clone();
            InputOutcome::Hover { id }
        } else {
            InputOutcome::None
        }
    }

    fn on_pointer_down(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    ) -> InputOutcome {
        self.range_drag_released = None;
        // Issue #85: any press ends a click run unless it continues it below.
        // Arrow presses are not text clicks (Designer, HTML_CONTRACT §2
        // Ruling 7, Mainnet 04c95f50): `try_spin_press` steps on every press
        // and never records one, so a fast double-click on ▲ steps twice,
        // and an arrow press never counts toward a double- or triple-click.
        let prev_text_click = self.last_text_click.take();

        // Open select: press inside the list must not hit-test the element under
        // the overlay (focus + reconcile would close the list and steal the click).
        let press_in_open_list = button == PointerButton::Primary
            && self.open_select.as_ref().is_some_and(|open| {
                let n = select_options(&self.doc, open.select_nid).len();
                list_row_at(open, x, y, n).is_some()
            });
        if press_in_open_list {
            self.press = None;
            self.range_drag_id = None;
            return InputOutcome::Press {
                id: self.open_select.as_ref().and_then(|o| o.select_id.clone()),
            };
        }

        // Issue #26: engine-drawn scrollbar (arrows, track, thumb).
        if button == PointerButton::Primary {
            if let Some((geom, part)) =
                bar_hit(&self.doc, x, y).filter(|(g, _)| bar_unoccluded(&self.doc, g.nid, x, y))
            {
                if self.open_select.is_some() {
                    self.close_select();
                }
                self.range_drag_id = None;
                return self.on_scrollbar_down(geom, part, y);
            }
        }

        // Open select: click outside closes without change; click on the closed
        // face while open is handled on up.
        if button == PointerButton::Primary {
            if let Some(ref open) = self.open_select.clone() {
                if open.control_rect.contains(x, y) {
                    // Stay open; press tracked below.
                } else {
                    self.close_select();
                }
            }
        }

        let ctrl = form_control_at_point(&self.doc, x, y);
        let id = ctrl
            .as_ref()
            .and_then(|c| c.element_id.clone())
            .or_else(|| self.hit_id(x, y));

        if let Some(ref eid) = id {
            let editable = self
                .doc
                .get_element_by_id(eid)
                .and_then(|nid| self.doc.get_node(nid))
                .is_some_and(|n| is_editable_input(n) && !is_disabled(n));
            let disabled_edit = self
                .doc
                .get_element_by_id(eid)
                .and_then(|nid| self.doc.get_node(nid))
                .is_some_and(|n| is_editable_input(n) && is_disabled(n));

            if disabled_edit {
                self.blur_text_edit_queued(ctx);
                self.doc.clear_focus();
                self.focus.focused_id = None;
                self.set_press(eid, button, mods);
                self.reconcile_open_select_after_focus();
                return InputOutcome::Press { id };
            }

            if editable {
                let focus_out = self.focus_text_input(ctx, eid);
                if let Some(nid) = self.doc.get_element_by_id(eid) {
                    // Issue #85 (Designer, HTML_CONTRACT §2 Ruling 7): one
                    // click places the caret (#68), a double-click selects
                    // the word, and a triple-click or more selects the field.
                    // A number box's text is one number, so its "word" is the
                    // whole field: `.` and `,` don't split it.
                    let count = if button == PointerButton::Primary {
                        let now = self.press_clock();
                        let count = prev_text_click
                            .filter(|c| c.continues(eid, x, y, now))
                            .map_or(1, |c| c.count.saturating_add(1));
                        self.last_text_click = Some(TextClick {
                            id: eid.clone(),
                            x,
                            y,
                            at: now,
                            count,
                        });
                        count
                    } else {
                        1
                    };
                    let is_number = self.doc.get_node(nid).is_some_and(is_number_input);
                    match count {
                        1 => place_caret_at(&mut self.doc, nid, x, y),
                        2 if !is_number => select_word_at(&mut self.doc, nid, x, y),
                        _ => select_all(&mut self.doc, nid),
                    }
                    self.reset_caret_blink();
                    self.queue_text_field_damage(nid);
                }
                self.range_drag_id = None;
                self.set_press(eid, button, mods);
                // Same class as #18 HOLD #3: close an open select when focus
                // moves to a text field (incl. Secondary / right-click).
                self.reconcile_open_select_after_focus();
                return focus_out.unwrap_or(InputOutcome::Press { id });
            }

            // Issue #81 (Designer rulings 1-2): a press on ▲/▼ steps the box
            // at once and leaves the focus in the box, never on the arrow.
            if button == PointerButton::Primary {
                if let Some(out) = self.try_spin_press(ctx, eid) {
                    self.range_drag_id = None;
                    self.set_press(eid, button, mods);
                    self.reconcile_open_select_after_focus();
                    return out;
                }
            }

            // Click outside a text field (or on another control): commit/blur edit.
            if self.text_edit.is_some()
                && self.text_edit.as_ref().map(|s| s.id.as_str()) != Some(eid.as_str())
            {
                self.blur_text_edit_queued(ctx);
            }

            if let Some(nid) = self.doc.get_element_by_id(eid) {
                if self.doc.set_focus_to(nid) {
                    // Issue #35: a focus move alone (no value change) still
                    // repaints (`:focus` styles, the slider focus dots).
                    self.mark_dom_dirty_raw();
                    self.mark_paint_dirty();
                }
                self.focus.focused_id = Some(eid.clone());
            }
        } else if let Some(ref c) = ctrl {
            if self.text_edit.is_some() {
                self.blur_text_edit_queued(ctx);
            }
            if self.doc.set_focus_to(c.node_id) {
                self.mark_dom_dirty_raw();
                self.mark_paint_dirty();
            }
            self.focus.focused_id = c.element_id.clone();
        } else if self.text_edit.is_some() {
            self.blur_text_edit_queued(ctx);
        }
        self.reconcile_open_select_after_focus();

        self.range_drag_id = None;
        if button == PointerButton::Primary {
            if let Some(ref c) = ctrl {
                if c.kind == FormControlKind::Range && !control_is_disabled(&self.doc, c.node_id) {
                    if let Some(ref eid) = c.element_id {
                        // Issue #35: a press on the thumb grabs it in place
                        // and drags; a press on the groove pages one block
                        // toward the pointer (Win95 trackbar; one page per
                        // press, no hold-to-repeat in v1).
                        let geom = slider_geometry(&self.doc, c.node_id);
                        let on_thumb = geom.as_ref().is_some_and(|g| g.thumb_contains(x, y));
                        self.range_press_value = self.attr(eid, "value");
                        self.range_paging = !on_thumb;
                        if on_thumb {
                            self.range_grab = geom.map(|g| grab_offset(&g, x, y)).unwrap_or(0.0);
                            self.apply_range_from_x(eid, x);
                        } else {
                            self.range_grab = 0.0;
                            self.apply_range_page(eid, x);
                        }
                        self.range_drag_id = Some(eid.clone());
                    }
                }
            }
        }

        if let Some(ref eid) = id {
            self.set_press(eid, button, mods);
            InputOutcome::Press { id }
        } else {
            self.press = None;
            self.range_drag_id = None;
            InputOutcome::Press { id: None }
        }
    }

    fn on_pointer_up(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    ) -> InputOutcome {
        // Issue #26: a press that started on a scrollbar never clicks.
        if button == PointerButton::Primary && self.scroll_press_active {
            self.end_scroll_press();
            self.press = None;
            return InputOutcome::None;
        }
        // Issue #81: the release of a ▲/▼ press, wherever it lands, ends the
        // hold and sends its one `change`.
        if button == PointerButton::Primary && self.press_is_spin_arrow() {
            let arrow = self.press.take().map(|p| p.id).unwrap_or_default();
            return self.release_spin_hold(ctx, &arrow);
        }
        // Range drag commit: clear drag, fire change once, suppress click.
        let mut suppress_click = false;
        if let Some(ref rid) = self.range_drag_id.clone() {
            if button == PointerButton::Primary {
                // Issue #35: the release only repaints when the value moves
                // (`apply_range_from_x` marks dirty and syncs outputs then);
                // an unmoved release costs no frame.
                if !self.range_paging && !self.element_is_disabled_id(rid) {
                    self.apply_range_from_x(rid, x);
                }
                let now = self.attr(rid, "value");
                if now != self.range_press_value.take() {
                    self.range_drag_released = Some(rid.clone());
                }
                suppress_click = true;
            }
        }
        self.range_drag_id = None;

        // Open select: pick a row, or cancel outside.
        if button == PointerButton::Primary {
            if let Some(ref open) = self.open_select.clone() {
                let opts = select_options(&self.doc, open.select_nid);
                let n = opts.len();
                if let Some(row) = list_row_at(open, x, y, n) {
                    let pick = row.min(SELECT_MAX_ROWS - 1).min(n.saturating_sub(1));
                    if !opts[pick].disabled && !control_is_disabled(&self.doc, open.select_nid) {
                        let changed = select_set_index(&mut self.doc, open.select_nid, pick);
                        self.close_select();
                        self.press.take();
                        if changed {
                            if let Some(ref sid) = open.select_id.clone() {
                                let _ = sync_outputs_for(&mut self.doc, sid);
                                self.mark_dom_dirty();
                                self.mark_paint_dirty();
                                return self.fire_change(ctx, sid);
                            }
                        }
                        self.mark_dom_dirty();
                        self.mark_paint_dirty();
                        return InputOutcome::None;
                    }
                } else if !open.control_rect.contains(x, y) {
                    self.close_select();
                    self.press.take();
                    return InputOutcome::None;
                } else {
                    // Click on the closed face while open: leave open (or toggle closed).
                    self.close_select();
                    self.press.take();
                    return InputOutcome::None;
                }
            }
        }

        let up_ctrl = form_control_at_point(&self.doc, x, y);
        let up_id = up_ctrl
            .as_ref()
            .and_then(|c| c.element_id.clone())
            .or_else(|| self.hit_id(x, y));
        let press = self.press.take();

        // Fire change for a completed range drag even if press/up ids differ.
        if let Some(rid) = self.range_drag_released.take() {
            if button == PointerButton::Primary && !self.element_is_disabled_id(&rid) {
                let out = self.fire_change(ctx, &rid);
                if suppress_click {
                    return out;
                }
            }
        }

        let Some(press) = press else {
            return InputOutcome::Press { id: None };
        };
        if press.button != button {
            return InputOutcome::None;
        }
        // Release off target → Press{None}, no click.
        if up_id.as_deref() != Some(press.id.as_str()) {
            // Label/control id mismatch: still allow toggle when the control matches.
            if button == PointerButton::Primary {
                if let Some(out) = self.try_activate_control_at(ctx, x, y) {
                    return out;
                }
            }
            return InputOutcome::Press { id: None };
        }

        if button == PointerButton::Primary {
            if let Some(out) = self.try_activate_control_at(ctx, x, y) {
                return out;
            }
        }

        if suppress_click {
            return InputOutcome::None;
        }

        let event_name: &'static str = match button {
            PointerButton::Primary => "click",
            PointerButton::Secondary => "contextmenu",
            PointerButton::Middle => "auxclick",
        };
        let trigger_event = match button {
            PointerButton::Primary => TriggerEvent::Click,
            PointerButton::Secondary => TriggerEvent::ContextMenu,
            PointerButton::Middle => TriggerEvent::AuxClick,
        };

        let trigger_info = TriggerInfo::Pointer {
            event: event_name,
            button,
            mods,
        };

        self.fire_pointer_or_key_on(
            ctx,
            &press.id,
            trigger_event,
            FilterCtx {
                mods,
                repeat: false,
                key: None,
            },
            trigger_info,
            Some(InputOutcome::Clicked {
                id: press.id.clone(),
            }),
        )
    }

    /// Toggle checkbox/radio, open/step select, or no-op. Fires `change` when committed.
    fn try_activate_control_at(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        x: f32,
        y: f32,
    ) -> Option<InputOutcome> {
        let ctrl = form_control_at_point(&self.doc, x, y)?;
        if control_is_disabled(&self.doc, ctrl.node_id) {
            return Some(InputOutcome::None);
        }
        match ctrl.kind {
            FormControlKind::Checkbox => {
                if toggle_checkbox(&mut self.doc, ctrl.node_id) {
                    self.after_control_commit(ctx, &ctrl)
                } else {
                    Some(InputOutcome::None)
                }
            }
            FormControlKind::Radio => {
                if let Some(result) = select_radio(&mut self.doc, ctrl.node_id) {
                    self.damage_previous_radio(result.previous_checked, ctrl.node_id);
                    self.after_control_commit(ctx, &ctrl)
                } else {
                    Some(InputOutcome::None)
                }
            }
            FormControlKind::Select => {
                if self.open_select.is_some() {
                    self.close_select();
                    Some(InputOutcome::None)
                } else if let Some(open) = open_select_state(&self.doc, &ctrl) {
                    self.open_select = Some(open);
                    self.select_hover_row = None;
                    self.mark_paint_dirty();
                    Some(InputOutcome::None)
                } else {
                    Some(InputOutcome::None)
                }
            }
            FormControlKind::Range | FormControlKind::Number => None,
        }
    }

    fn after_control_commit(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        ctrl: &FormControlRef,
    ) -> Option<InputOutcome> {
        // Graph review (PR #24 follow-up): explicit damage for the committed
        // control itself. Checked/selected state is element state Blitz
        // paints directly at paint time, not necessarily something a CSS
        // restyle flags as damage — so style damage alone can miss a
        // checkbox/radio's own tick-mark redraw (unchecking then rechecking
        // a gate left a stale partial repaint). Despite the name, this ink-
        // box helper is generic, not text-specific (see its doc comment).
        self.queue_text_field_damage(ctrl.node_id);
        self.mark_dom_dirty();
        self.mark_paint_dirty();
        if let Some(ref sid) = ctrl.element_id {
            let _ = sync_outputs_for(&mut self.doc, sid);
            Some(self.fire_change(ctx, sid))
        } else {
            Some(InputOutcome::None)
        }
    }

    /// Close the open list on every path. Pushes control+list rects into the
    /// single `pending_damage` list so the next plan clears the overlay even
    /// when `open_select` is already `None` (pick / Enter) and when the list
    /// overflowed the surface.
    fn close_select(&mut self) {
        let Some(open) = self.open_select.take() else {
            return;
        };
        self.select_hover_row = None;
        let scale = self.layout_scale;
        for r in select_damage_rects(Some(&open), scale) {
            self.push_pending_damage(r);
        }
    }

    /// Damage the previously checked radio's box when exclusivity moves.
    fn damage_previous_radio(&mut self, previous: Option<NodeId>, new_nid: NodeId) {
        let Some(prev) = previous else {
            return;
        };
        if prev == new_nid {
            return;
        }
        self.ensure_resolved();
        if let Some(rect) = control_rect_of(&self.doc, prev) {
            self.push_pending_damage(rect.as_device(self.layout_scale));
        }
        self.mark_paint_dirty();
    }

    fn reconcile_text_edit(&mut self) {
        // Issue #94: greying (markup, actions slice or a rule) drops a held
        // change before the box being edited is reconciled.
        self.drop_greyed_catch_up();
        let Some(session) = self.text_edit.clone() else {
            return;
        };
        let Some(nid) = self.doc.get_element_by_id(&session.id) else {
            self.text_edit = None;
            self.kept_typing = None;
            if self.focus.focused_id.as_deref() == Some(session.id.as_str()) {
                self.focus.focused_id = None;
            }
            self.doc.clear_focus();
            return;
        };
        let ok = self
            .doc
            .get_node(nid)
            .is_some_and(|n| is_editable_input(n) && !is_disabled(n));
        if !ok {
            // Issue #81 (Designer ruling 3, HTML_CONTRACT §2): the box being
            // edited was greyed in place. The typed text is dropped and the
            // box shows the value the game last saw (the last `change`, or
            // the value at focus), with no `change`; the blur below keeps the
            // caret out. A replaced box keeps the value of its new markup.
            if nid == session.node_id
                && self
                    .doc
                    .get_node(nid)
                    .is_some_and(|n| is_editable_input(n) && is_disabled(n))
            {
                let restored = session.value_at_change.clone();
                let _ = crate::form_args::write_value_attr(&mut self.doc, nid, &restored);
                crate::text_edit::set_editor_text(&mut self.doc, nid, &restored);
                apply_unfocused_display(&mut self.doc, nid);
                let _ = sync_outputs_for(&mut self.doc, &session.id);
                self.mark_dom_dirty();
                self.queue_text_field_damage(nid);
                if self.spin_hold.as_ref().is_some_and(|h| h.target_nid == nid) {
                    self.spin_hold = None;
                }
                if self.key_hold.as_ref().is_some_and(|k| *k == session.id) {
                    self.key_hold = None;
                }
            }
            // Ruling 6: a greyed box drops the text being typed too.
            self.text_edit = None;
            self.kept_typing = None;
            if self.focus.focused_id.as_deref() == Some(session.id.as_str()) {
                self.focus.focused_id = None;
            }
            self.doc.clear_focus();
            return;
        }
        // Issue #68 (Graph review of #69): the same, unreplaced node (an
        // unrelated swap, or greying attributes written on other controls)
        // keeps its editor as it is: the Tab selection, the caret and the
        // value at focus (Esc and the commit's `change` compare against it).
        if nid == session.node_id && !session.needs_prepare {
            return;
        }
        // Same-id replacement: rebind node_id; defer prepare_focus_display until
        // style/layout (sync must not resolve). Typing before paint must not wipe value.
        let value = self.attr(&session.id, "value").unwrap_or_default();
        self.focus.focused_id = Some(session.id.clone());
        if let Some(s) = self.text_edit.as_mut() {
            s.node_id = nid;
            s.value_at_change = value.clone();
            s.value_at_focus = value;
            s.caret_visible = true;
            s.last_blink = Instant::now();
            s.last_caret_rect = None;
            s.needs_prepare = true;
        }
        // Issue #89: a box with a held or unanswered commit keeps the last
        // value actually sent as its step base and restore value.
        self.sync_session_sent();
        self.queue_text_field_damage(nid);
        self.mark_paint_dirty();
    }

    /// After swap/rebind: resolve if needed, seed PlainEditor from `value`,
    /// restore Blitz focus, caret at end.
    fn ensure_text_edit_prepared(&mut self) {
        let Some(session) = self.text_edit.clone() else {
            return;
        };
        if !session.needs_prepare {
            return;
        }
        self.ensure_resolved();
        let nid = session.node_id;
        if self.doc.get_node(nid).is_none() {
            self.text_edit = None;
            return;
        }
        prepare_focus_display(&mut self.doc, nid);
        // Issue #89 (Mainnet 188feffd): text the player was typing survives
        // the redraw; it stays uncommitted, as before the swap.
        if let Some((kid, text)) = self.kept_typing.take() {
            if kid == session.id {
                let is_number = self.doc.get_node(nid).is_some_and(is_number_input);
                let fallback = self
                    .text_edit
                    .as_ref()
                    .map(|s| s.value_at_focus.clone())
                    .unwrap_or_default();
                set_editor_text(&mut self.doc, nid, &text);
                let _ = sync_value_from_editor(&mut self.doc, nid, is_number, false, &fallback);
                self.doc.with_text_input(nid, |mut driver| {
                    driver.move_to_text_end();
                });
            }
        }
        // Issue #35: the rebound node gains FOCUS here, after this frame's
        // resolve; restyle now so its `:focus` look (outline) is in this
        // frame instead of one frame late (partial == full on the swap frame).
        if self.doc.set_focus_to(nid) {
            self.needs_resolve = true;
            self.ensure_resolved();
        }
        if let Some(s) = self.text_edit.as_mut() {
            s.needs_prepare = false;
            s.caret_visible = true;
            s.last_blink = Instant::now();
            s.last_caret_rect = None;
        }
        self.queue_text_field_damage(nid);
        self.mark_paint_dirty();
    }

    /// Close the open list when its node is gone, replaced, or disabled.
    fn reconcile_open_select(&mut self) {
        let Some(open) = self.open_select.clone() else {
            return;
        };
        let node_ok = self.doc.get_node(open.select_nid).is_some_and(|n| {
            n.element_data()
                .is_some_and(|el| &*el.name.local == "select")
        });
        let id_ok = match &open.select_id {
            Some(id) => self.doc.get_element_by_id(id) == Some(open.select_nid),
            None => node_ok,
        };
        let disabled = control_is_disabled(&self.doc, open.select_nid);
        if !node_ok || !id_ok || disabled {
            self.close_select();
        }
    }

    /// Close the open list when focus leaves its control.
    fn reconcile_open_select_after_focus(&mut self) {
        let Some(open) = self.open_select.clone() else {
            return;
        };
        let focused_on_select = match (&open.select_id, &self.focus.focused_id) {
            (Some(sid), Some(fid)) => sid == fid,
            (None, _) => false,
            (Some(_), None) => false,
        };
        if !focused_on_select {
            self.close_select();
        }
    }

    /// Issue #81 (Designer rulings 1-2, HTML_CONTRACT §2): a primary press
    /// on a `data-hx-step` arrow. The target box takes the focus (another box
    /// being edited commits, as on any focus move) with the caret at the end
    /// and nothing selected, and steps once now, exactly as ArrowUp/ArrowDown
    /// would (off-grid snaps to the grid in the arrow's direction, an on-grid
    /// value moves one step, never past min/max). The step is not reported
    /// yet: [`Self::advance_spin_hold`] repeats it while the press is held,
    /// and the release sends one `change` ([`Self::release_spin_hold`]).
    /// `None` when `hit_id` is not an arrow; an arrow of a disabled box is
    /// pressed without stepping or moving the focus.
    fn try_spin_press(&mut self, ctx: &mut ActionCtx<'_>, hit_id: &str) -> Option<InputOutcome> {
        let dir = step_direction(&self.doc, hit_id)?;
        let (target_nid, target_id) = resolve_step_target(&self.doc, hit_id)?;
        self.spin_hold = None;
        let pressed = InputOutcome::Press {
            id: Some(hit_id.to_string()),
        };
        if self.doc.get_node(target_nid).is_some_and(is_disabled) {
            // A press beside an edited box still ends that edit.
            self.blur_text_edit_queued(ctx);
            return Some(pressed);
        }
        let out = match target_id.as_deref() {
            Some(tid) => self.focus_text_input(ctx, tid).unwrap_or(pressed),
            None => {
                self.blur_text_edit_queued(ctx);
                pressed
            }
        };
        let reported = match self.text_edit.as_ref().filter(|s| s.node_id == target_nid) {
            Some(s) => s.value_at_change.clone(),
            None => target_id
                .as_deref()
                .and_then(|t| self.attr(t, "value"))
                .unwrap_or_default(),
        };
        self.spin_step_target(target_nid, target_id.as_deref(), dir);
        self.spin_hold = Some(SpinHold {
            target_nid,
            target_id,
            dir,
            pressed_at: self.press_clock(),
            repeats: 0,
            reported,
            pinned: false,
        });
        Some(out)
    }

    /// One ▲/▼ step of `nid` (issue #81): the value moves (clamped), a focused
    /// box keeps its raw text with the caret at the end and nothing selected,
    /// and only the box's own damage is queued (partial repaint equals full).
    fn spin_step_target(&mut self, nid: NodeId, id: Option<&str>, dir: i32) -> bool {
        let fallback = self.sent_value_of(nid);
        let changed = step_and_regroup(&mut self.doc, nid, dir, fallback.as_deref());
        if self.text_edit.as_ref().is_some_and(|s| s.node_id == nid) {
            prepare_focus_display(&mut self.doc, nid);
            self.reset_caret_blink();
        }
        if let Some(id) = id {
            let _ = sync_outputs_for(&mut self.doc, id);
        }
        self.mark_dom_dirty();
        self.queue_text_field_damage(nid);
        changed
    }

    /// The one clock for press timing: the arrow hold (#81) and the click
    /// count (#85). The wall clock, unless a test froze it.
    fn press_clock(&self) -> Instant {
        self.test_clock.unwrap_or_else(Instant::now)
    }

    /// Record the pointer press on `id` (the one place a press is built).
    fn set_press(&mut self, id: &str, button: PointerButton, mods: Modifiers) {
        self.press = Some(PressState {
            id: id.to_string(),
            button,
            mods,
        });
    }

    /// The last value sent for `nid` while it is the box being edited (the
    /// last `change`, or the value at focus): what an arrow on an empty or
    /// unparsable box steps from (issue #92, Ruling 5, Mainnet cea8a42a).
    fn sent_value_of(&self, nid: NodeId) -> Option<String> {
        self.text_edit
            .as_ref()
            .filter(|s| s.node_id == nid)
            .map(|s| s.value_at_change.clone())
    }

    /// Issue #89 (Designer, HTML_CONTRACT `hx-sync`, Mainnet master
    /// 188feffd): hold back the `change` value control `ctrl_id` committed
    /// while `source_id`'s request is pending. It is not marked as sent: the
    /// box keeps the last value actually sent as its step base and restore
    /// value, and [`Self::run_catch_up`] sends the newest committed value once
    /// the request finishes. `false` for a control a request cannot carry
    /// (no name, or no pair in the pending request), which keeps `hx-sync`.
    fn hold_change(&mut self, source_id: &str, ctrl_id: &str) -> bool {
        let Some(nid) = self.doc.get_element_by_id(ctrl_id) else {
            return false;
        };
        let Some(node) = self.doc.get_node(nid) else {
            return false;
        };
        let Some(committed) = control_current_value(&self.doc, node) else {
            return false;
        };
        if let Some(e) = self.catch_up.iter_mut().find(|e| e.control_id == ctrl_id) {
            e.source_id = source_id.to_string();
            e.committed = committed;
            e.held = true;
        } else {
            let Some(sent) = self
                .pending
                .by_source
                .get(source_id)
                .and_then(|p| control_value_in_args(node, &p.args))
            else {
                return false;
            };
            self.catch_up.push(CatchUp {
                control_id: ctrl_id.to_string(),
                source_id: source_id.to_string(),
                sent,
                committed,
                game: None,
                game_radio: None,
                held: true,
            });
        }
        // A radio's commit unticks its group; re-read the other controls
        // (not the box being typed in, nor a slider being dragged).
        let skip: Vec<String> = [
            Some(ctrl_id.to_string()),
            self.text_edit.as_ref().map(|s| s.id.clone()),
            self.range_drag_id.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        for i in 0..self.catch_up.len() {
            if skip.contains(&self.catch_up[i].control_id) {
                continue;
            }
            let now = self
                .doc
                .get_element_by_id(&self.catch_up[i].control_id)
                .and_then(|n| self.doc.get_node(n))
                .and_then(|n| control_current_value(&self.doc, n));
            if let Some(now) = now {
                self.catch_up[i].committed = now;
            }
        }
        self.sync_session_sent();
        true
    }

    /// Issue #89: the player committed `value` in box `id` (an arrow step,
    /// Enter, blur, a key release), sent or not.
    fn note_committed(&mut self, id: &str, value: &str) {
        if let Some(e) = self.catch_up.iter_mut().find(|e| e.control_id == id) {
            e.committed = value.to_string();
        }
        self.sync_session_sent();
    }

    /// Issue #89 (point 3 of Graph's review): the box being edited steps from
    /// and restores the last value actually sent, never a held one.
    fn sync_session_sent(&mut self) {
        let Some(s) = self.text_edit.as_mut() else {
            return;
        };
        if let Some(e) = self.catch_up.iter().find(|e| e.control_id == s.id) {
            s.value_at_change = e.sent.clone();
        }
    }

    /// Issue #89: `a` and `b` are the same value of control `id` (by number
    /// in a number box, #87).
    fn same_control_value(&self, id: &str, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        let is_number = self
            .doc
            .get_element_by_id(id)
            .and_then(|n| self.doc.get_node(n))
            .is_some_and(is_number_input);
        is_number && parse_f64(a).is_some() && parse_f64(b).is_some() && number_is_sent(a, b)
    }

    /// Issue #89 (Mainnet 188feffd): `source_id`'s request finished (done,
    /// failed or timed out). An entry with nothing new committed is answered
    /// and dropped. A held commit equal to the value sent needs nothing; any
    /// other sends one `change` with the newest committed values, so fast
    /// presses fold into it and the game ends on the box's value. Only a
    /// player's commit holds a change, so the game's own redraw (a clamp or
    /// a refusal) never starts another send.
    fn run_catch_up(&mut self, ctx: &mut ActionCtx<'_>, source_id: &str) {
        if self.pending.is_pending(source_id) {
            return;
        }
        let entries = std::mem::take(&mut self.catch_up);
        let mut kept = Vec::with_capacity(entries.len());
        for e in entries {
            if e.source_id == source_id
                && (!e.held || {
                    let sent = self.effective_sent(&e.control_id, &e.sent);
                    self.same_control_value(&e.control_id, &e.committed, &sent)
                })
            {
                continue;
            }
            kept.push(e);
        }
        self.catch_up = kept;
        if !self.catch_up.iter().any(|e| e.source_id == source_id) {
            self.sync_session_sent();
            return;
        }
        let action = self
            .bindings
            .get(source_id)
            .and_then(|b| b.hx_post.clone().map(|name| (name, b.hx_indicator.clone())));
        let Some((action_name, indicator)) = action else {
            self.catch_up.retain(|e| e.source_id != source_id);
            return;
        };
        if self.disabled_actions.contains_key(&action_name) {
            // Issue #94: a greyed control drops its held change.
            self.drop_greyed_catch_up();
            return;
        }
        let mut args = self.action_args_for(source_id);
        for e in self.catch_up.iter().filter(|e| e.source_id == source_id) {
            // The box being typed in posts its committed value, not the
            // text in progress.
            if let Some(node) = self
                .doc
                .get_element_by_id(&e.control_id)
                .and_then(|n| self.doc.get_node(n))
                .filter(|n| is_editable_input(n) || is_numeric_input(n))
            {
                patch_control_arg(node, &mut args, &e.committed);
            }
        }
        let out = self.emit_action(
            ctx,
            source_id,
            &action_name,
            args,
            TriggerInfo::Change,
            indicator,
        );
        for e in self
            .catch_up
            .iter_mut()
            .filter(|e| e.source_id == source_id)
        {
            e.held = false;
            e.sent = e.committed.clone();
        }
        self.sync_session_sent();
        self.released_outcomes.push(out);
    }

    /// Issue #89 (Mainnet 188feffd): a redraw (the answer to an older
    /// request) never overwrites a value the player committed after it: each
    /// control with a held or unanswered commit gets that value back. A
    /// control the redraw removed is dropped from the catch-up. Runs inside
    /// the swap, whose subtree is already damaged.
    ///
    /// Only a control inside the swapped-in nodes (`swapped_in`) was drawn
    /// by this redraw (Graph review of #98): a redraw of another section (a
    /// HUD on `time` redraws every tick) leaves the control, its put-back and
    /// the game's value (`game`) as they are.
    fn restore_catch_up_values(&mut self, swapped_in: &[NodeId]) {
        let entries = std::mem::take(&mut self.catch_up);
        let mut kept = Vec::with_capacity(entries.len());
        for e in entries {
            let Some(nid) = self.doc.get_element_by_id(&e.control_id) else {
                continue;
            };
            if !self.is_inside_any(nid, swapped_in) {
                kept.push(e);
                continue;
            }
            let Some(node) = self.doc.get_node(nid) else {
                continue;
            };
            let Some(now) = control_current_value(&self.doc, node) else {
                continue;
            };
            let mut e = e;
            if value_control_kind(node) == "radio" {
                // The radio of the group the game drew ticked ("" for none),
                // read before the put-back changes the group.
                let name = attr_of(node, "name").unwrap_or_default();
                e.game_radio = Some(
                    radios_in_group(&self.doc, &name)
                        .into_iter()
                        .filter_map(|r| self.doc.get_node(r))
                        .find(|r| control_current_value(&self.doc, r).as_deref() == Some("true"))
                        .map(|r| attr_of(r, "value").unwrap_or_else(|| "on".to_string()))
                        .unwrap_or_default(),
                );
            }
            e.game = Some(now.clone());
            if now != e.committed {
                self.put_control_value(nid, &e.control_id, &e.committed);
            }
            kept.push(e);
        }
        self.catch_up = kept;
    }

    /// `nid` is one of `roots` or inside one of them.
    fn is_inside_any(&self, nid: NodeId, roots: &[NodeId]) -> bool {
        let mut cur = Some(nid);
        while let Some(n) = cur {
            if roots.contains(&n) {
                return true;
            }
            cur = self.doc.get_node(n).and_then(|node| node.parent);
        }
        false
    }

    /// Issue #89: write `value` (in `control_current_value`'s encoding) into
    /// control `nid` (`id`): the selected option, the tick, the checked
    /// radio, or the `value` attribute.
    fn put_control_value(&mut self, nid: NodeId, id: &str, value: &str) {
        let Some(node) = self.doc.get_node(nid) else {
            return;
        };
        match value_control_kind(node) {
            "select" => {
                if let Some(i) = select_options(&self.doc, nid)
                    .iter()
                    .position(|o| o.value == value)
                {
                    select_set_index(&mut self.doc, nid, i);
                }
            }
            "checkbox" => {
                // Write the state and the `checked` attribute: a box a swap
                // just inserted has no checked state yet (Blitz creates it
                // from the attribute at layout), so a toggle alone is lost.
                // A greyed box is written too (drop to the game's value).
                self.put_checked(nid, value == "true");
            }
            "radio" => {
                if value == "true" {
                    let name = attr_of(node, "name").unwrap_or_default();
                    for other in radios_in_group(&self.doc, &name) {
                        if other != nid {
                            self.put_checked(other, false);
                        }
                    }
                    self.put_checked(nid, true);
                }
            }
            _ => {
                let _ = write_value_attr(&mut self.doc, nid, value);
            }
        }
        let _ = sync_outputs_for(&mut self.doc, id);
        self.mark_dom_dirty();
    }

    /// Issue #94 (Graph review): set checkbox or radio `nid` checked or not,
    /// in its live state when it has one and in its `checked` attribute, so
    /// a node a swap just inserted (no live state until layout) keeps it.
    fn put_checked(&mut self, nid: NodeId, checked: bool) {
        let has_attr = self
            .doc
            .get_node(nid)
            .is_some_and(|n| attr_of(n, "checked").is_some());
        let live = self
            .doc
            .get_node(nid)
            .and_then(|n| n.element_data())
            .and_then(|el| el.checkbox_input_checked());
        if has_attr == checked && live.is_none_or(|l| l == checked) {
            return;
        }
        self.doc.snapshot_node_state_only(nid);
        if has_attr != checked {
            let mut mutr = self.doc.mutate();
            if checked {
                mutr.set_attribute(nid, qual_name!("checked"), "");
            } else {
                mutr.clear_attribute(nid, qual_name!("checked"));
            }
        }
        if let Some(node) = self.doc.get_node_mut(nid) {
            if let Some(el) = node.element_data_mut() {
                if el.checkbox_input_checked().is_some() {
                    el.set_checkbox_input_checked(checked);
                }
            }
            node.mark_ancestors_dirty();
        }
    }

    /// Issue #94 (Graph review): what radio `id` carried in the last request
    /// any radio of its group sent, in `control_current_value`'s encoding;
    /// `fallback` for any other control or a group nothing has sent for.
    fn effective_sent(&self, id: &str, fallback: &str) -> String {
        let Some(node) = self
            .doc
            .get_element_by_id(id)
            .and_then(|n| self.doc.get_node(n))
        else {
            return fallback.to_string();
        };
        if value_control_kind(node) != "radio" {
            return fallback.to_string();
        }
        let name = attr_of(node, "name").unwrap_or_default();
        match self.radio_sent.get(&name) {
            Some(v) => {
                let mine = attr_of(node, "value").unwrap_or_else(|| "on".to_string());
                (*v == mine).to_string()
            }
            None => fallback.to_string(),
        }
    }

    /// Issue #94 (Designer, HTML_CONTRACT `hx-sync`, Mainnet master
    /// 5983ff17): when the game greys a control (its markup, an `actions`
    /// state or an input rule) while it holds a change, the held change is
    /// dropped, as Ruling 6 drops typed text, and the control shows the
    /// game's value: the value a redraw drew, else the last value sent.
    fn drop_greyed_catch_up(&mut self) {
        if self.catch_up.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.catch_up);
        let mut kept = Vec::with_capacity(entries.len());
        for e in entries {
            let Some(nid) = self.doc.get_element_by_id(&e.control_id) else {
                continue;
            };
            let action_greyed = self
                .bindings
                .get(&e.source_id)
                .and_then(|b| b.hx_post.as_ref())
                .is_some_and(|a| self.disabled_actions.contains_key(a));
            if !(action_greyed || control_is_disabled(&self.doc, nid)) {
                kept.push(e);
                continue;
            }
            let game = e
                .game
                .clone()
                .unwrap_or_else(|| self.effective_sent(&e.control_id, &e.sent));
            if game == "false"
                && self
                    .doc
                    .get_node(nid)
                    .is_some_and(|n| value_control_kind(n) == "radio")
            {
                // A held radio pick: the group shows the radio the game drew
                // or, with no redraw, the one the group last sent (Graph
                // review of #98). Writing "false" to the held radio alone
                // leaves it ticked.
                self.put_radio_game(nid, e.game.as_ref().and(e.game_radio.as_deref()));
                self.mark_paint_dirty();
                continue;
            }
            // Issue #96: the box being edited greys in place next, and
            // Ruling 6 (`reconcile_text_edit`) writes the session's
            // `value_at_change` back. After a redraw that is the game's drawn
            // value, not the last value sent, so the session takes it too.
            if let Some(s) = self.text_edit.as_mut().filter(|s| s.node_id == nid) {
                s.value_at_change = game.clone();
                s.value_at_focus = game.clone();
            }
            let shown = self
                .doc
                .get_node(nid)
                .and_then(|n| control_current_value(&self.doc, n));
            if shown.as_deref() != Some(game.as_str()) {
                self.put_control_value(nid, &e.control_id, &game);
                if self.text_edit.as_ref().is_some_and(|s| s.node_id == nid) {
                    set_editor_text(&mut self.doc, nid, &game);
                }
                apply_unfocused_display(&mut self.doc, nid);
                self.queue_text_field_damage(nid);
                self.mark_paint_dirty();
            }
        }
        self.catch_up = kept;
    }

    /// Graph review of #98: tick the radio of `nid`'s group that the game
    /// has, the one it drew (`drawn`, "" for none) or else the one the group
    /// last sent, and untick `nid` when the game has none ticked.
    fn put_radio_game(&mut self, nid: NodeId, drawn: Option<&str>) {
        let Some(node) = self.doc.get_node(nid) else {
            return;
        };
        let name = attr_of(node, "name").unwrap_or_default();
        let want = drawn
            .map(str::to_string)
            .or_else(|| self.radio_sent.get(&name).cloned());
        let target = want.and_then(|v| {
            radios_in_group(&self.doc, &name).into_iter().find(|&r| {
                self.doc
                    .get_node(r)
                    .is_some_and(|n| attr_of(n, "value").unwrap_or_else(|| "on".to_string()) == v)
            })
        });
        match target {
            Some(r) => {
                let id = self.element_id_of(r).unwrap_or_default();
                self.put_control_value(r, &id, "true");
            }
            None => {
                self.put_checked(nid, false);
                self.mark_dom_dirty();
            }
        }
    }

    /// Issue #89: the text being typed in the box being edited, when it is
    /// not the committed value (Ruling 6 still drops it if the box greys).
    /// A held arrow or key's steps are not typing: the replacement clears
    /// the repeat and shows the game's value (#84).
    fn uncommitted_typing(&mut self) -> Option<(String, String)> {
        let s = self.text_edit.clone()?;
        if self
            .spin_hold
            .as_ref()
            .is_some_and(|h| h.target_nid == s.node_id)
            || self.key_hold.as_deref() == Some(s.id.as_str())
        {
            return None;
        }
        let text = editor_text(&mut self.doc, s.node_id)?;
        let is_number = self.doc.get_node(s.node_id).is_some_and(is_number_input);
        let same = text == s.value_at_focus
            || (is_number
                && parse_f64(&text).is_some()
                && parse_f64(&s.value_at_focus).is_some()
                && number_is_sent(&text, &s.value_at_focus));
        (!same).then_some((s.id, text))
    }

    /// True while the primary press started on a `data-hx-step` arrow.
    fn press_is_spin_arrow(&self) -> bool {
        self.press.as_ref().is_some_and(|p| {
            p.button == PointerButton::Primary && step_direction(&self.doc, &p.id).is_some()
        })
    }

    /// Issue #81 (Designer ruling 2): the repeat steps a held ▲/▼ owes by now.
    /// Called from `sync`; stops on its own once the box is gone or greyed.
    fn advance_spin_hold(&mut self) {
        self.advance_spin_hold_by(None);
    }

    /// [`Self::advance_spin_hold`] with the time held given (tests) or read
    /// from the press time (`None`). A box that lost the focus since the
    /// press (Tab, a host focus move, a click that blurred it) stops
    /// repeating: its blur already reported the value (Graph review of #84).
    fn advance_spin_hold_by(&mut self, held: Option<std::time::Duration>) {
        let Some(hold) = self.spin_hold.clone() else {
            return;
        };
        let live = self
            .doc
            .get_node(hold.target_nid)
            .is_some_and(|n| is_number_input(n) && !is_disabled(n));
        let lost_focus = hold.target_id.is_some()
            && self.focus.focused_id.as_deref() != hold.target_id.as_deref();
        // No arrow press any more (a path that cleared `press` without the
        // release): nothing can end this hold, so end it now.
        if !live || lost_focus || !self.press_is_spin_arrow() {
            self.spin_hold = None;
            return;
        }
        // Pinned at min or max: further steps move nothing, so do not mark
        // the box dirty (and reset its caret blink) every 100 ms for nothing.
        if hold.pinned {
            return;
        }
        let due = spin_repeats_due(held.unwrap_or_else(|| {
            self.press_clock()
                .saturating_duration_since(hold.pressed_at)
        }));
        let mut done = hold.repeats;
        let mut pinned = false;
        while done < due {
            done += 1;
            if !self.spin_step_target(hold.target_nid, hold.target_id.as_deref(), hold.dir) {
                // Held at min or max: the remaining steps would not move it.
                done = due;
                pinned = true;
            }
        }
        if let Some(h) = self.spin_hold.as_mut() {
            h.repeats = done;
            h.pinned = pinned;
        }
    }

    /// Issue #81: the release of a ▲/▼ press. The box keeps the focus and the
    /// stepped value, which becomes the committed value (Esc and a refused
    /// comma come back to it), and one `change` reports it. A press or hold
    /// that ends on the last value sent to the game sends no `change`
    /// (Designer, HTML_CONTRACT §2 Ruling 5: no preview refresh for nothing).
    /// A box greyed during the hold sends nothing; a box without an `id`
    /// reports the arrow's click instead, as before.
    fn release_spin_hold(&mut self, ctx: &mut ActionCtx<'_>, arrow: &str) -> InputOutcome {
        let Some(hold) = self.spin_hold.take() else {
            return InputOutcome::None;
        };
        if self
            .doc
            .get_node(hold.target_nid)
            .is_none_or(|n| !is_number_input(n) || is_disabled(n))
        {
            return InputOutcome::None;
        }
        let Some(tid) = hold.target_id else {
            return InputOutcome::Clicked {
                id: arrow.to_string(),
            };
        };
        // The box being edited commits as Enter would (canonical text, held
        // inside min/max: a typed `12` above max 10 stepped ▲ commits `10`,
        // as ArrowUp then Enter does) and stays focused, caret at the end.
        if let Some(session) = self
            .text_edit
            .clone()
            .filter(|s| s.node_id == hold.target_nid)
        {
            let _ = commit_field(&mut self.doc, hold.target_nid, &session.value_at_change);
            prepare_focus_display(&mut self.doc, hold.target_nid);
            self.reset_caret_blink();
            let _ = sync_outputs_for(&mut self.doc, &tid);
            self.mark_dom_dirty();
            self.queue_text_field_damage(hold.target_nid);
        }
        // Ruling 5 (b6f2c892): the box's value against the last value sent.
        let now = self.attr(&tid, "value").unwrap_or_default();
        let already_sent = self.is_sent_value(hold.target_nid, &now, &hold.reported);
        self.mark_committed(hold.target_nid, &tid);
        if already_sent {
            return InputOutcome::None;
        }
        self.fire_change(ctx, &tid)
    }

    /// Issue #81 (Designer, HTML_CONTRACT §2 Ruling 5, Mainnet b6f2c892):
    /// the box being edited ends an arrow interaction on the last value sent
    /// to the game, so its `change` would report nothing new (no preview
    /// refresh). The test is the box's value against that sent value, never
    /// the typed text: typed `3` stepped ▼ to the sent `2` sends nothing, as
    /// does ▲ at max or ▼ at min.
    fn at_sent_value(&self, nid: NodeId, id: &str) -> bool {
        self.text_edit.as_ref().is_some_and(|s| {
            s.node_id == nid
                && self.is_sent_value(
                    nid,
                    &self.attr(id, "value").unwrap_or_default(),
                    &s.value_at_change,
                )
        })
    }

    /// Issue #87 (Designer, HTML_CONTRACT §2 Ruling 5, Mainnet master
    /// 04c95f50): whether `now` is the last value `sent` for `nid`. A number
    /// box compares by number ([`number_is_sent`]): `4.1` is a sent `4.10`,
    /// and text that does not parse sends nothing. Other boxes compare text.
    fn is_sent_value(&self, nid: NodeId, now: &str, sent: &str) -> bool {
        if self.doc.get_node(nid).is_some_and(is_number_input) {
            number_is_sent(now, sent)
        } else {
            now == sent
        }
    }

    /// Record `id`'s value as committed for the box being edited (issue #81):
    /// a `change` reported it, so Esc, a refused comma and the next commit's
    /// comparison all start from it.
    fn mark_committed(&mut self, nid: NodeId, id: &str) {
        let value = self.attr(id, "value").unwrap_or_default();
        if let Some(s) = self.text_edit.as_mut().filter(|s| s.node_id == nid) {
            s.value_at_change = value.clone();
            s.value_at_focus = value.clone();
        }
        self.note_committed(id, &value);
    }

    fn focus_text_input(&mut self, ctx: &mut ActionCtx<'_>, id: &str) -> Option<InputOutcome> {
        let nid = self.doc.get_element_by_id(id)?;
        let node = self.doc.get_node(nid)?;
        if !is_editable_input(node) || is_disabled(node) {
            return None;
        }
        // Graph review of #72: a click into the box already being edited
        // only moves the caret. Re-seeding here put the typed text through
        // the focus display (`7` became `7.00`) and reset `value_at_focus`
        // and `value_at_change` to the typed value, so the Enter or blur
        // that followed fired no `change` and Esc no longer reverted.
        if self
            .text_edit
            .as_ref()
            .is_some_and(|s| s.id == id && s.node_id == nid)
        {
            self.ensure_text_edit_prepared();
            return None;
        }
        // Commit previous field if switching.
        if self.text_edit.as_ref().map(|s| s.id.as_str()) != Some(id) {
            self.blur_text_edit_queued(ctx);
        }
        let value_at_focus = self.attr(id, "value").unwrap_or_default();
        prepare_focus_display(&mut self.doc, nid);
        self.doc.set_focus_to(nid);
        self.focus.focused_id = Some(id.to_string());
        self.text_edit = Some(TextEditSession {
            id: id.to_string(),
            node_id: nid,
            value_at_change: value_at_focus.clone(),
            value_at_focus,
            caret_visible: true,
            last_blink: Instant::now(),
            last_caret_rect: None,
            needs_prepare: false,
        });
        self.reset_caret_blink();
        self.mark_dom_dirty();
        self.queue_text_field_damage(nid);
        Some(InputOutcome::FocusChanged(self.focus.clone()))
    }

    fn blur_text_edit(&mut self, ctx: &mut ActionCtx<'_>) -> Option<InputOutcome> {
        self.commit_text_edit(ctx, true)
    }

    /// Commit/blur and, when the outcome is Action/Blocked, push it onto
    /// `released_outcomes` so hosts that only see the click/Tab return value
    /// still receive Change via `take_outcomes` (blocker 1).
    fn blur_text_edit_queued(&mut self, ctx: &mut ActionCtx<'_>) {
        if let Some(out) = self.blur_text_edit(ctx) {
            self.enqueue_deferred_outcome(out);
        }
    }

    fn enqueue_deferred_outcome(&mut self, out: InputOutcome) {
        match out {
            InputOutcome::Action { .. } | InputOutcome::Blocked { .. } => {
                self.released_outcomes.push(out);
            }
            _ => {}
        }
    }

    /// Commit the focused text/number field. If `clear_focus`, also clear focus.
    /// Fires `change` only when the committed value differs from
    /// `value_at_change` (the focus-time value, or the value the last arrow
    /// step's `change` already reported).
    fn commit_text_edit(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        clear_focus: bool,
    ) -> Option<InputOutcome> {
        self.ensure_text_edit_prepared();
        let session = self.text_edit.take()?;
        // Issue #81: this commit reports any key-repeat steps, so the key
        // release that may follow has nothing left to send.
        if self.key_hold.as_ref().is_some_and(|k| *k == session.id) {
            self.key_hold = None;
        }
        // The fallback for text that is no number is the last value sent
        // (Ruling 5, Mainnet 98ac51a9; issue #90).
        let _ = commit_field(&mut self.doc, session.node_id, &session.value_at_change);
        self.mark_dom_dirty();
        self.queue_text_field_damage(session.node_id);
        let value_now = self.attr(&session.id, "value").unwrap_or_default();
        self.note_committed(&session.id, &value_now);
        let change = if !self.is_sent_value(session.node_id, &value_now, &session.value_at_change) {
            self.fire_change(ctx, &session.id)
        } else {
            InputOutcome::None
        };
        if clear_focus {
            self.doc.clear_focus();
            if self.focus.focused_id.as_deref() == Some(session.id.as_str()) {
                self.focus.focused_id = None;
            }
            // Restore grouped display for number inputs.
            apply_unfocused_display(&mut self.doc, session.node_id);
        } else {
            // Keep editing session after Enter commit.
            prepare_focus_display(&mut self.doc, session.node_id);
            self.text_edit = Some(TextEditSession {
                id: session.id.clone(),
                node_id: session.node_id,
                value_at_change: value_now.clone(),
                value_at_focus: value_now,
                caret_visible: true,
                last_blink: Instant::now(),
                last_caret_rect: session.last_caret_rect,
                needs_prepare: false,
            });
            self.reset_caret_blink();
            // Issue #89: a held commit is not the value sent.
            self.sync_session_sent();
        }
        Some(change)
    }

    /// Issue #81: the key release after ArrowUp/ArrowDown steps that sent no
    /// `change` yet (key repeat, or every keydown under
    /// `EngineConfig::key_releases`). One `change` unless the box ends on the
    /// last value sent (Ruling 5); a blur or Enter before
    /// the release reports it instead and clears the hold.
    fn release_key_hold(&mut self, ctx: &mut ActionCtx<'_>) -> Option<InputOutcome> {
        let fid = self.key_hold.take()?;
        let session = self.text_edit.clone().filter(|s| s.id == fid)?;
        if self.element_is_disabled_id(&fid) {
            return Some(InputOutcome::None);
        }
        // Ruling 5: the box ends on the last value sent.
        if self.at_sent_value(session.node_id, &fid) {
            let value = self.attr(&fid, "value").unwrap_or_default();
            self.note_committed(&fid, &value);
            return Some(InputOutcome::None);
        }
        self.mark_committed(session.node_id, &fid);
        Some(self.fire_change(ctx, &fid))
    }

    fn hold_focused_number_to_commit(&mut self) {
        let Some((nid, fallback)) = self
            .text_edit
            .as_ref()
            .map(|s| (s.node_id, s.value_at_change.clone()))
        else {
            return;
        };
        if self.doc.get_node(nid).is_some_and(is_number_input)
            && editor_text(&mut self.doc, nid).is_some()
        {
            let _ =
                crate::text_edit::sync_value_from_editor(&mut self.doc, nid, true, true, &fallback);
        }
    }

    fn revert_text_edit(&mut self) {
        let Some(session) = self.text_edit.take() else {
            return;
        };
        // Esc restores the value at focus, held inside the limits as they are
        // now: a limits refresh while focused skipped this box, and Esc also
        // blurs, so it is the commit that applies them (Graph review of #41 r2).
        let restored = match self.doc.get_node(session.node_id) {
            Some(n) if is_number_input(n) && !session.value_at_focus.trim().is_empty() => {
                let limits = crate::form_args::numeric_limits(n);
                crate::text_edit::canonical_from_display(&session.value_at_focus, &limits, true)
            }
            _ => session.value_at_focus.clone(),
        };
        let _ = crate::form_args::write_value_attr(&mut self.doc, session.node_id, &restored);
        // The value attribute may already hold the restored value (a refused
        // comma text such as "3,5" never reached it, #51), in which case the
        // write above leaves the editor alone: rewrite it so the typed text
        // does not stay on screen after Esc.
        crate::text_edit::set_editor_text(&mut self.doc, session.node_id, &restored);
        apply_unfocused_display(&mut self.doc, session.node_id);
        self.doc.clear_focus();
        self.focus.focused_id = None;
        self.mark_dom_dirty();
        self.queue_text_field_damage(session.node_id);
    }

    fn on_text(&mut self, s: String, paste: bool) -> InputOutcome {
        self.ensure_text_edit_prepared();
        let Some(session) = self.text_edit.clone() else {
            return InputOutcome::Unhandled(KeyEvent {
                key: s,
                code: String::new(),
                mods: Modifiers::default(),
                repeat: false,
                pressed: true,
            });
        };
        let nid = session.node_id;
        // PlainEditor missing (pre-layout): do not sync empty → wipe value.
        if editor_text(&mut self.doc, nid).is_none() {
            return InputOutcome::None;
        }
        let is_number = self.doc.get_node(nid).is_some_and(is_number_input);
        let filtered = crate::text_edit::sanitize_text_insert(&s);
        if filtered.is_empty() {
            return InputOutcome::None;
        }
        if is_number {
            // Judge the insert against what survives the selection it
            // replaces, not the old buffer (issue #38: pasting "3.45" over a
            // selected "6.12" dropped the new '.').
            let existing = text_outside_selection(&mut self.doc, nid).unwrap_or_default();
            let limits = self
                .doc
                .get_node(nid)
                .map(crate::form_args::numeric_limits)
                .unwrap_or(crate::form_args::NumericLimits {
                    min: None,
                    max: None,
                    step: 1.0,
                    unconstrained: false,
                });
            let filtered = if paste {
                // A paste is all or nothing (Graph review of #41): judge the
                // whole text it would leave in the box, and refuse the paste
                // rather than drop characters and commit a different number
                // ("3.7" into a whole-month box must not become "37").
                let ignore = self
                    .doc
                    .get_node(nid)
                    .map(crate::text_edit::paste_ignore_chars)
                    .unwrap_or_default();
                let Some(cleaned) = crate::text_edit::clean_number_paste(&filtered, &ignore) else {
                    return InputOutcome::None;
                };
                let (head, tail) =
                    crate::text_edit::selection_split(&mut self.doc, nid).unwrap_or_default();
                if !crate::text_edit::number_text_valid(&format!("{head}{cleaned}{tail}"), &limits)
                {
                    return InputOutcome::None;
                }
                cleaned
            } else {
                // Commas are kept while typing; commit judges them (#51).
                filter_number_insert(&existing, &filtered, &limits)
            };
            if filtered.is_empty() {
                return InputOutcome::None;
            }
            insert_text(&mut self.doc, nid, &filtered);
        } else {
            insert_text(&mut self.doc, nid, &filtered);
        }
        let _ = sync_value_from_editor(
            &mut self.doc,
            nid,
            is_number,
            false,
            &session.value_at_focus,
        );
        self.reset_caret_blink();
        self.mark_dom_dirty();
        self.queue_text_field_damage(nid);
        InputOutcome::None
    }

    fn on_paste(&mut self, s: String) -> InputOutcome {
        if self.text_edit.is_none() {
            return InputOutcome::None;
        }
        let prepared = crate::text_edit::sanitize_paste(&s);
        self.on_text(prepared, true)
    }

    fn on_copy(&mut self, cut: bool) -> InputOutcome {
        let Some(session) = self.text_edit.clone() else {
            return InputOutcome::None;
        };
        let Some(text) = selected_text(&mut self.doc, session.node_id) else {
            return InputOutcome::None;
        };
        if cut {
            delete_selection(&mut self.doc, session.node_id);
            let is_number = self
                .doc
                .get_node(session.node_id)
                .is_some_and(is_number_input);
            let _ = sync_value_from_editor(
                &mut self.doc,
                session.node_id,
                is_number,
                false,
                &session.value_at_focus,
            );
            self.reset_caret_blink();
            self.mark_dom_dirty();
            self.queue_text_field_damage(session.node_id);
        }
        InputOutcome::ClipboardWrite { text }
    }

    fn try_text_edit_key(&mut self, key: &KeyEvent) -> Option<InputOutcome> {
        self.ensure_text_edit_prepared();
        let session = self.text_edit.clone()?;
        let nid = session.node_id;
        let action_mod = key.mods.ctrl || key.mods.meta;
        let select = key.mods.shift;

        if action_mod && matches!(key.key.as_str(), "a" | "A") {
            select_all(&mut self.doc, nid);
            self.reset_caret_blink();
            self.queue_text_field_damage(nid);
            return Some(InputOutcome::None);
        }

        match key.key.as_str() {
            "Backspace" => {
                backspace(&mut self.doc, nid);
            }
            "Delete" => {
                delete_forward(&mut self.doc, nid);
            }
            "ArrowLeft" => {
                move_left(&mut self.doc, nid, select);
                self.reset_caret_blink();
                self.queue_text_field_damage(nid);
                return Some(InputOutcome::None);
            }
            "ArrowRight" => {
                move_right(&mut self.doc, nid, select);
                self.reset_caret_blink();
                self.queue_text_field_damage(nid);
                return Some(InputOutcome::None);
            }
            "Home" => {
                move_home(&mut self.doc, nid, select);
                self.reset_caret_blink();
                self.queue_text_field_damage(nid);
                return Some(InputOutcome::None);
            }
            "End" => {
                move_end(&mut self.doc, nid, select);
                self.reset_caret_blink();
                self.queue_text_field_damage(nid);
                return Some(InputOutcome::None);
            }
            _ => return None,
        }
        let is_number = self.doc.get_node(nid).is_some_and(is_number_input);
        let _ = sync_value_from_editor(
            &mut self.doc,
            nid,
            is_number,
            false,
            &session.value_at_focus,
        );
        self.reset_caret_blink();
        self.mark_dom_dirty();
        self.queue_text_field_damage(nid);
        Some(InputOutcome::None)
    }

    fn reset_caret_blink(&mut self) {
        if let Some(session) = self.text_edit.as_mut() {
            session.caret_visible = true;
            session.last_blink = Instant::now();
        }
    }

    /// Queue transform-aware ink-box damage for a text/number field (outline +
    /// outline-offset + box-shadow). All text-edit / caret / selection damage
    /// goes through [`Self::push_pending_damage`].
    fn queue_text_field_damage(&mut self, nid: NodeId) {
        let scale = self.layout_scale;
        let (sw, sh) = self.layout_viewport;
        // Old ink (last paint) covers the previous :focus outline / caret.
        if let Some(r) = self
            .last_bounds
            .get(&nid)
            .and_then(|b| ink_damage_rect(b.ink, sw, sh))
        {
            self.push_pending_damage(r);
        }
        // Current ink includes outline width + outward outline-offset + box-shadow.
        if let Some((x, y, w, h)) = ink_box_device(&self.doc, nid, scale) {
            self.push_pending_damage(snap_outward(x, y, w, h, sw, sh));
        }
    }

    /// Advance caret blink; when the phase flips, queue a small damage rect.
    /// Selection present → keep caret visible (no blink) so Blitz keeps the
    /// selection highlight without a clear_focus dance (blocker 5).
    fn advance_caret_blink(&mut self) {
        let Some(session) = self.text_edit.clone() else {
            return;
        };
        let nid = session.node_id;
        if has_selection(&mut self.doc, nid) {
            if let Some(s) = self.text_edit.as_mut() {
                s.caret_visible = true;
                s.last_blink = Instant::now();
            }
            return;
        }
        let Some(session) = self.text_edit.as_mut() else {
            return;
        };
        if session.last_blink.elapsed() < CARET_BLINK_HALF_PERIOD {
            return;
        }
        session.last_blink = Instant::now();
        session.caret_visible = !session.caret_visible;
        let scale = self.layout_scale;
        let (sw, sh) = self.layout_viewport;
        let prev = session.last_caret_rect;
        let cur = caret_device_rect(&mut self.doc, nid, scale, sw, sh);
        let mut pushed = false;
        if let Some(r) = cur {
            if let Some(session) = self.text_edit.as_mut() {
                session.last_caret_rect = Some(r);
            }
            self.push_pending_damage(r);
            pushed = true;
        }
        if let Some(r) = prev {
            self.push_pending_damage(r);
            pushed = true;
        }
        // Fallback: if we have no caret geometry yet, damage the focused input box.
        if !pushed {
            self.queue_text_field_damage(nid);
        }
    }

    /// Single commit path for `hx-trigger="change"` (range release, toggle,
    /// select pick, arrow/spin step, typed Enter/blur).
    fn fire_change(&mut self, ctx: &mut ActionCtx<'_>, node_id: &str) -> InputOutcome {
        let outer = self.change_origin.replace(node_id.to_string());
        let out = self.fire_on(
            ctx,
            node_id,
            TriggerEvent::Change,
            FilterCtx {
                mods: Modifiers::default(),
                repeat: false,
                key: None,
            },
            TriggerInfo::Change,
            None,
            false,
        );
        self.change_origin = outer;
        out
    }

    fn element_is_disabled_id(&self, id: &str) -> bool {
        self.doc
            .get_element_by_id(id)
            .is_some_and(|nid| control_is_disabled(&self.doc, nid))
    }

    fn fire_pointer_or_key_on(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        source_id: &str,
        event: TriggerEvent,
        filter_ctx: FilterCtx<'_>,
        trigger_info: TriggerInfo,
        clicked_fallback: Option<InputOutcome>,
    ) -> InputOutcome {
        self.fire_on(
            ctx,
            source_id,
            event,
            filter_ctx,
            trigger_info,
            clicked_fallback,
            false,
        )
    }

    /// `allow_from_body`: when true, match triggers that declared `from:body`/`from:document`.
    #[allow(clippy::too_many_arguments)]
    fn fire_on(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        source_id: &str,
        event: TriggerEvent,
        filter_ctx: FilterCtx<'_>,
        trigger_info: TriggerInfo,
        clicked_fallback: Option<InputOutcome>,
        allow_from_body: bool,
    ) -> InputOutcome {
        let binding = match self.bindings.get(source_id).cloned() {
            Some(b) => b,
            None => {
                return clicked_fallback.unwrap_or(InputOutcome::None);
            }
        };

        // Find matching trigger on this element (bubbling: walk ancestors by id).
        let mut matched = None;
        for t in &binding.triggers {
            if t.event != event {
                continue;
            }
            if t.from.is_some() != allow_from_body {
                continue;
            }
            let ok = match &t.filter {
                None => true,
                Some(f) => f.eval(&filter_ctx),
            };
            if ok {
                matched = Some(t.clone());
                break;
            }
        }

        // Ancestor bubbling for key events (local triggers only).
        if matched.is_none()
            && !allow_from_body
            && matches!(event, TriggerEvent::KeyDown | TriggerEvent::KeyUp)
        {
            if let Some(parent_id) = self.parent_element_id(source_id) {
                return self.fire_on(
                    ctx,
                    &parent_id,
                    event,
                    filter_ctx,
                    trigger_info,
                    clicked_fallback,
                    false,
                );
            }
        }

        let Some(_trig) = matched else {
            return clicked_fallback.unwrap_or(InputOutcome::None);
        };

        let Some(action_name) = binding.hx_post.clone() else {
            return clicked_fallback.unwrap_or(InputOutcome::Clicked {
                id: source_id.to_string(),
            });
        };

        // Click-time undeclared check.
        if !ctx.declared_actions.contains(&action_name) {
            panic!("hx_native: unknown action `{action_name}` at click time (test failure)");
        }

        // Disabled → Blocked.
        if let Some(reason) = self.disabled_actions.get(&action_name).cloned() {
            return InputOutcome::Blocked {
                name: action_name,
                reason,
                source_id: source_id.to_string(),
                trigger: trigger_info,
            };
        }

        let args = self.action_args_for(source_id);

        // Pending / hx-sync.
        if self.pending.is_pending(source_id) {
            // Issue #89 (Mainnet 188feffd): `drop` and `queue` are for
            // clicks and actions; a value control's `change` is held and
            // caught up when the pending request finishes.
            if event == TriggerEvent::Change {
                if let Some(ctrl) = self.change_origin.clone() {
                    if self.hold_change(source_id, &ctrl) {
                        return InputOutcome::None;
                    }
                }
            }
            match binding.hx_sync {
                SyncMode::Drop => {
                    return InputOutcome::None;
                }
                SyncMode::Queue => {
                    self.pending.queued.insert(
                        source_id.to_string(),
                        QueuedRelease {
                            source_id: source_id.to_string(),
                            action_name,
                            args,
                            trigger: trigger_info,
                            indicator_id: binding.hx_indicator.clone(),
                        },
                    );
                    return InputOutcome::None;
                }
            }
        }

        self.emit_action(
            ctx,
            source_id,
            &action_name,
            args,
            trigger_info,
            binding.hx_indicator.clone(),
        )
    }

    fn action_args_for(&self, source_id: &str) -> Vec<(String, String)> {
        // Issue #15 (K1): an `EnabledWhen`-greyed input still posts its value.
        let rule_greyed: HashSet<NodeId> = self
            .rule_disabled
            .keys()
            .filter_map(|eid| self.doc.get_element_by_id(eid))
            .collect();
        collect_action_args(&self.doc, source_id, &rule_greyed)
    }

    /// Issue #35: a groove press pages the value one block toward `x`.
    fn apply_range_page(&mut self, id: &str, x: f32) -> bool {
        self.ensure_resolved();
        let changed = range_page_toward_x(&mut self.doc, id, x);
        if changed {
            let _ = sync_outputs_for(&mut self.doc, id);
            self.mark_dom_dirty();
            self.mark_paint_dirty();
        }
        changed
    }

    fn apply_range_from_x(&mut self, id: &str, x: f32) -> bool {
        self.ensure_resolved();
        let changed = set_range_value_from_x(&mut self.doc, id, x, self.range_grab);
        if changed {
            let _ = sync_outputs_for(&mut self.doc, id);
            self.mark_dom_dirty();
            self.mark_paint_dirty();
        }
        changed
    }

    /// Step focused number/range; returns `Some(outcome)` when handled (always
    /// consumes the key for a focused numeric, and fires `change` when the
    /// value actually moved). Number inputs: only ArrowUp/Down step (Left/Right
    /// move the caret via text edit). Range keeps Left/Right stepping.
    fn try_step_focused_numeric(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        key: &str,
        repeat: bool,
    ) -> Option<InputOutcome> {
        let fid = self.focus.focused_id.clone()?;
        if self.element_is_disabled_id(&fid) {
            return None;
        }
        let nid = self.doc.get_element_by_id(&fid)?;
        let node = self.doc.get_node(nid)?;
        if !is_numeric_input(node) {
            return None;
        }
        let is_number = is_number_input(node);
        // Issue #35: Home / End on a focused range jump to min / max (the
        // max snapped onto the step grid), committing like an arrow step.
        // Round 3: PageUp / PageDown page one block, the same as a groove
        // press. Handled here, before #34's scroll keys, so a slider inside
        // a scroll container moves instead of scrolling it.
        if !is_number && matches!(key, "Home" | "End" | "PageUp" | "PageDown") {
            let changed = match key {
                "PageUp" => range_page(&mut self.doc, nid, 1.0),
                "PageDown" => range_page(&mut self.doc, nid, -1.0),
                _ => range_to_end(&mut self.doc, nid, key == "End"),
            };
            if changed {
                let _ = sync_outputs_for(&mut self.doc, &fid);
                self.mark_dom_dirty();
                self.mark_paint_dirty();
                return Some(self.fire_change(ctx, &fid));
            }
            return Some(InputOutcome::None);
        }
        let dir = if is_number {
            match key {
                "ArrowUp" => 1,
                "ArrowDown" => -1,
                _ => return None,
            }
        } else {
            match key {
                "ArrowUp" | "ArrowRight" => 1,
                "ArrowDown" | "ArrowLeft" => -1,
                _ => return None,
            }
        };
        let changed = if is_number {
            let fallback = self.sent_value_of(nid);
            let changed = step_and_regroup(&mut self.doc, nid, dir, fallback.as_deref());
            // Focused: keep the raw editable text with the caret at the end,
            // not the grouped display (issue #40, Graph #42 review).
            if self.text_edit.as_ref().is_some_and(|s| s.node_id == nid) {
                prepare_focus_display(&mut self.doc, nid);
            }
            changed
        } else {
            step_numeric_value(&mut self.doc, &fid, dir)
        };
        if changed {
            let _ = sync_outputs_for(&mut self.doc, &fid);
            self.mark_dom_dirty();
            if is_number {
                self.queue_text_field_damage(nid);
            } else {
                self.mark_paint_dirty();
            }
            // Issue #81 (Designer rulings 2 and 5): key repeat steps a
            // number box without a `change`, and so does the first keydown
            // when the host forwards key releases (`EngineConfig::
            // key_releases`); the key release reports the value once.
            if is_number && (repeat || ctx.key_releases) {
                self.key_hold = Some(fid);
                return Some(InputOutcome::None);
            }
            // Issue #81 Ruling 5: a step that ends on the last value sent
            // (▲ at max, a typed `3` ArrowDown to the sent `2`) sends no
            // `change`.
            let unmoved = self.at_sent_value(nid, &fid);
            // Issue #62: this `change` reports the stepped value, so the
            // Enter / blur that commits it must not report it again, and
            // (issue #81) Esc and a refused comma come back to it.
            self.mark_committed(nid, &fid);
            if is_number && unmoved {
                return Some(InputOutcome::None);
            }
            Some(self.fire_change(ctx, &fid))
        } else {
            Some(InputOutcome::None)
        }
    }

    /// Closed select: ArrowUp/Down step value + change. Open select: move highlight.
    fn try_select_keys(&mut self, ctx: &mut ActionCtx<'_>, key: &KeyEvent) -> Option<InputOutcome> {
        // Esc key-up after a consumed Esc key-down (list was open).
        if !key.pressed && key.key == "Escape" && self.consume_escape_keyup {
            self.consume_escape_keyup = false;
            return Some(InputOutcome::None);
        }
        if !key.pressed {
            return None;
        }
        // Esc while open: consume key-down and the following key-up.
        if key.key == "Escape" {
            if self.open_select.is_some() {
                self.close_select();
                self.consume_escape_keyup = true;
                return Some(InputOutcome::None);
            }
            return None;
        }

        if let Some(ref open) = self.open_select.clone() {
            if control_is_disabled(&self.doc, open.select_nid) {
                return Some(InputOutcome::None);
            }
            let opts = select_options(&self.doc, open.select_nid);
            let rows = opts.len().min(SELECT_MAX_ROWS);
            match key.key.as_str() {
                "ArrowDown" => {
                    if let Some(os) = self.open_select.as_mut() {
                        if os.highlight + 1 < rows {
                            os.highlight += 1;
                        }
                    }
                    self.mark_paint_dirty();
                    return Some(InputOutcome::None);
                }
                "ArrowUp" => {
                    if let Some(os) = self.open_select.as_mut() {
                        os.highlight = os.highlight.saturating_sub(1);
                    }
                    self.mark_paint_dirty();
                    return Some(InputOutcome::None);
                }
                "Enter" => {
                    let pick = open.highlight.min(rows.saturating_sub(1));
                    if opts.get(pick).is_some_and(|o| !o.disabled) {
                        let changed = select_set_index(&mut self.doc, open.select_nid, pick);
                        self.close_select();
                        if changed {
                            if let Some(ref sid) = open.select_id.clone() {
                                let _ = sync_outputs_for(&mut self.doc, sid);
                                self.mark_dom_dirty();
                                self.mark_paint_dirty();
                                return Some(self.fire_change(ctx, sid));
                            }
                        }
                        self.mark_dom_dirty();
                        self.mark_paint_dirty();
                    }
                    return Some(InputOutcome::None);
                }
                _ => {}
            }
        }

        let fid = self.focus.focused_id.clone()?;
        if self.element_is_disabled_id(&fid) {
            return None;
        }
        let nid = self.doc.get_element_by_id(&fid)?;
        let is_select = self.doc.get_node(nid).is_some_and(|n| {
            n.element_data()
                .is_some_and(|el| &*el.name.local == "select")
        });
        if !is_select {
            return None;
        }

        // Alt+ArrowDown opens the list.
        if key.key == "ArrowDown" && key.mods.alt {
            let ctrl = FormControlRef {
                node_id: nid,
                element_id: Some(fid),
                kind: FormControlKind::Select,
            };
            if let Some(open) = open_select_state(&self.doc, &ctrl) {
                self.open_select = Some(open);
                self.select_hover_row = None;
                self.mark_paint_dirty();
            }
            return Some(InputOutcome::None);
        }

        let dir = match key.key.as_str() {
            "ArrowDown" | "ArrowRight" => 1,
            "ArrowUp" | "ArrowLeft" => -1,
            _ => return None,
        };
        if select_step(&mut self.doc, nid, dir) {
            let _ = sync_outputs_for(&mut self.doc, &fid);
            self.mark_dom_dirty();
            self.mark_paint_dirty();
            Some(self.fire_change(ctx, &fid))
        } else {
            Some(InputOutcome::None)
        }
    }

    fn parent_element_id(&self, id: &str) -> Option<String> {
        let doc = &self.doc;
        let nid = doc.get_element_by_id(id)?;
        let mut parent = doc.get_node(nid)?.parent;
        while let Some(pid) = parent {
            let node = doc.get_node(pid)?;
            if let Some(eid) = node.attrs().and_then(|attrs| {
                attrs
                    .iter()
                    .find(|a| &*a.name.local == "id")
                    .map(|a| a.value.to_string())
            }) {
                if !eid.is_empty() {
                    return Some(eid);
                }
            }
            parent = node.parent;
        }
        None
    }

    fn emit_action(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        source_id: &str,
        action_name: &str,
        args: Vec<(String, String)>,
        trigger: TriggerInfo,
        indicator_id: Option<String>,
    ) -> InputOutcome {
        let request = ctx.requests.issue(ctx.surface);
        self.note_radio_sent(source_id, &args);

        self.pending.insert(PendingEntry {
            request,
            source_id: source_id.to_string(),
            action_name: action_name.to_string(),
            args: args.clone(),
            trigger: trigger.clone(),
            indicator_id: indicator_id.clone(),
            frames_alive: 0,
        });

        self.set_pending_marks(source_id, indicator_id.as_deref(), true);

        InputOutcome::Action {
            request,
            name: action_name.to_string(),
            args,
            source_id: source_id.to_string(),
            trigger,
        }
    }

    /// Issue #94 (Graph review): a radio's request records what its group
    /// sent (see `radio_sent`); groups no longer in the document are pruned.
    fn note_radio_sent(&mut self, source_id: &str, args: &[(String, String)]) {
        let Some(node) = self
            .doc
            .get_element_by_id(source_id)
            .and_then(|n| self.doc.get_node(n))
        else {
            return;
        };
        if value_control_kind(node) != "radio" {
            return;
        }
        let Some(name) = attr_of(node, "name").filter(|n| !n.is_empty()) else {
            return;
        };
        let doc = &self.doc;
        self.radio_sent
            .retain(|g, _| !radios_in_group(doc, g).is_empty());
        match args.iter().find(|(k, _)| *k == name) {
            Some((_, v)) => {
                self.radio_sent.insert(name, v.clone());
            }
            None => {
                self.radio_sent.remove(&name);
            }
        }
    }

    fn set_pending_marks(&mut self, source_id: &str, indicator_id: Option<&str>, on: bool) {
        let ids: Vec<String> = {
            let mut v = vec![source_id.to_string()];
            if let Some(ind) = indicator_id {
                v.push(ind.to_string());
            }
            v
        };
        let doc = &mut self.doc;
        for eid in ids {
            let Some(nid) = doc.get_element_by_id(&eid) else {
                continue;
            };
            let current_class = doc
                .get_node(nid)
                .and_then(|n| {
                    n.attrs().and_then(|attrs| {
                        attrs
                            .iter()
                            .find(|a| &*a.name.local == "class")
                            .map(|a| a.value.to_string())
                    })
                })
                .unwrap_or_default();
            let new_class = if on {
                add_class_token(&current_class, "hx-request")
            } else {
                remove_class_token(&current_class, "hx-request")
            };
            let mut mutr = doc.mutate();
            if eid == source_id {
                if on {
                    mutr.set_attribute(
                        nid,
                        QualName::new(None, ns!(), LocalName::from("aria-busy")),
                        "true",
                    );
                } else {
                    mutr.clear_attribute(
                        nid,
                        QualName::new(None, ns!(), LocalName::from("aria-busy")),
                    );
                }
            }
            if new_class.is_empty() {
                mutr.clear_attribute(nid, qual_name!("class"));
            } else {
                mutr.set_attribute(nid, qual_name!("class"), &new_class);
            }
        }
        self.mark_dom_dirty();
        self.mark_paint_dirty();
    }

    fn on_key(&mut self, ctx: &mut ActionCtx<'_>, key: KeyEvent) -> InputOutcome {
        let event = if key.pressed {
            TriggerEvent::KeyDown
        } else {
            TriggerEvent::KeyUp
        };

        // Select open/closed keys (Esc consumed only while the list is open).
        if let Some(out) = self.try_select_keys(ctx, &key) {
            return out;
        }

        // Esc with a text field focused: revert to value-at-focus and consume
        // (exception to Input rule 4). Without focus, Esc stays Unhandled.
        if key.pressed && key.key == "Escape" && self.text_edit.is_some() {
            self.revert_text_edit();
            return InputOutcome::None;
        }

        // Editing keys on a focused text/number field (before action triggers).
        if key.pressed {
            if let Some(out) = self.try_text_edit_key(&key) {
                return out;
            }
        }

        // Enter on focused text/number: commit + fire_change (keep focus).
        if key.pressed && key.key == "Enter" && !key.repeat && self.text_edit.is_some() {
            if let Some(out) = self.commit_text_edit(ctx, false) {
                return out;
            }
            return InputOutcome::None;
        }

        // Number/range arrow editing (before action triggers) — fires change.
        if key.pressed
            && matches!(
                key.key.as_str(),
                "ArrowUp"
                    | "ArrowRight"
                    | "ArrowDown"
                    | "ArrowLeft"
                    | "Home"
                    | "End"
                    | "PageUp"
                    | "PageDown"
            )
        {
            if let Some(out) = self.try_step_focused_numeric(ctx, &key.key, key.repeat) {
                return out;
            }
        }

        // Issue #81: releasing the ArrowUp/ArrowDown whose key repeat stepped
        // a number box sends the one `change` for the repeat steps.
        if !key.pressed && matches!(key.key.as_str(), "ArrowUp" | "ArrowDown") {
            if let Some(out) = self.release_key_hold(ctx) {
                return out;
            }
        }

        // Space on focused checkbox/radio toggles and fires change.
        // Always consume Space on a checkable so (c) does not treat it as a button.
        if key.pressed && key.key == " " && !key.repeat {
            if let Some(ref fid) = self.focus.focused_id.clone() {
                if let Some(nid) = self.doc.get_element_by_id(fid) {
                    let is_checkable = self.doc.get_node(nid).is_some_and(|n| {
                        let ty = n
                            .attrs()
                            .and_then(|a| {
                                a.iter()
                                    .find(|a| &*a.name.local == "type")
                                    .map(|a| a.value.to_ascii_lowercase())
                            })
                            .unwrap_or_default();
                        n.element_data()
                            .is_some_and(|el| &*el.name.local == "input")
                            && matches!(ty.as_str(), "checkbox" | "radio")
                    });
                    if is_checkable {
                        if control_is_disabled(&self.doc, nid) {
                            return InputOutcome::None;
                        }
                        let input_ty = self.doc.get_node(nid).and_then(|n| {
                            n.attrs().and_then(|a| {
                                a.iter()
                                    .find(|a| &*a.name.local == "type")
                                    .map(|a| a.value.to_ascii_lowercase())
                            })
                        });
                        let changed = if input_ty.as_deref() == Some("radio") {
                            if let Some(result) = select_radio(&mut self.doc, nid) {
                                self.damage_previous_radio(result.previous_checked, nid);
                                true
                            } else {
                                false
                            }
                        } else {
                            activate_checkable(&mut self.doc, nid)
                        };
                        if changed {
                            // Same explicit commit damage as the pointer path
                            // (`after_control_commit`): the tick mark is element
                            // state, not guaranteed style damage.
                            self.queue_text_field_damage(nid);
                            let _ = sync_outputs_for(&mut self.doc, fid);
                            self.mark_dom_dirty();
                            self.mark_paint_dirty();
                            return self.fire_change(ctx, fid);
                        }
                        return InputOutcome::None;
                    }
                }
            }
        }

        // (a) Focused element first.
        if let Some(ref fid) = self.focus.focused_id.clone() {
            let triggers = self
                .bindings
                .get(fid)
                .map(|b| {
                    b.triggers
                        .iter()
                        .filter(|t| t.event == event && t.from.is_none())
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let filter_ctx = filter_ctx_from_key(&key);
            let mut matched_filter_ok = false;
            for t in &triggers {
                if matches!(event, TriggerEvent::KeyDown)
                    && key.repeat
                    && !keydown_repeat_allowed(&t.filter, true)
                {
                    continue;
                }
                let ok = t
                    .filter
                    .as_ref()
                    .map(|f| f.eval(&filter_ctx))
                    .unwrap_or(true);
                if ok {
                    matched_filter_ok = true;
                    break;
                }
            }
            if matched_filter_ok {
                let trigger_info = TriggerInfo::Key {
                    event: if key.pressed { "keydown" } else { "keyup" },
                    key: key.key.clone(),
                    code: key.code.clone(),
                    mods: key.mods,
                    repeat: key.repeat,
                };
                return self.fire_pointer_or_key_on(
                    ctx,
                    fid,
                    event.clone(),
                    filter_ctx,
                    trigger_info,
                    None,
                );
            }
        }

        // Issue #26: PageUp/PageDown/Home/End scroll the focused element's
        // container (after text-edit keys, which own Home/End in a field).
        if let Some(out) = self.try_scroll_keys(&key) {
            return out;
        }

        // (b) Tab / Shift+Tab focus order (engine-owned document-order walk).
        if key.pressed && key.key == "Tab" {
            self.ensure_resolved();
            // Save focused id before blur clears it, then step from that id.
            let from_id = self.focus.focused_id.clone();
            self.blur_text_edit_queued(ctx);
            let order = focusable_ids(&self.doc);
            let next = step_focus(&order, from_id.as_deref(), key.mods.shift);
            if let Some(ref nid_str) = next {
                let editable = self
                    .doc
                    .get_element_by_id(nid_str)
                    .and_then(|nid| self.doc.get_node(nid))
                    .is_some_and(|n| is_editable_input(n) && !is_disabled(n));
                if editable {
                    let _ = self.focus_text_input(ctx, nid_str);
                    // Issue #68 (Designer lock, HTML_CONTRACT §2): focus that
                    // arrives by Tab / Shift+Tab selects the whole text, so
                    // typing replaces the value (Win95 edit boxes, Chrome).
                    // Click focus places the caret; host / swap focus keeps
                    // the caret at the end with no selection. The field's
                    // damage is already queued by `focus_text_input`.
                    if let Some(nid) = self.doc.get_element_by_id(nid_str) {
                        select_all(&mut self.doc, nid);
                    }
                } else if let Some(nid) = self.doc.get_element_by_id(nid_str) {
                    self.doc.set_focus_to(nid);
                    self.focus.focused_id = Some(nid_str.clone());
                }
            } else {
                self.doc.clear_focus();
                self.focus.focused_id = None;
            }
            self.reconcile_open_select_after_focus();
            // Issue #26: focus scrolls into view.
            if let Some(nid) = self
                .focus
                .focused_id
                .as_deref()
                .and_then(|id| self.doc.get_element_by_id(id))
            {
                self.scroll_into_view(nid);
            }
            self.mark_paint_dirty();
            return InputOutcome::FocusChanged(self.focus.clone());
        }

        // (c) Enter / Space on focused hx-post button → like primary click.
        if key.pressed && (key.key == "Enter" || key.key == " ") && !key.repeat {
            if let Some(ref fid) = self.focus.focused_id.clone() {
                if self
                    .bindings
                    .get(fid)
                    .and_then(|b| b.hx_post.as_ref())
                    .is_some()
                {
                    let trigger_info = TriggerInfo::Key {
                        event: "keydown",
                        key: key.key.clone(),
                        code: key.code.clone(),
                        mods: key.mods,
                        repeat: key.repeat,
                    };
                    // Activate like click: use Click event matching on the element,
                    // but TriggerInfo::Key as required.
                    return self.activate_like_click(ctx, fid, trigger_info);
                }
            }
        }

        // (d) from:body / from:document triggers — document order; prefer enabled.
        // Among filter matches: first ENABLED (not greyed) emits Action; greyed
        // matches are skipped; if every match is greyed → Blocked with the first
        // greyed match's reason (Designer pause/resume Space pattern).
        {
            let filter_ctx = filter_ctx_from_key(&key);
            let order = ids_in_document_order(&self.doc);
            let trigger_info = TriggerInfo::Key {
                event: if key.pressed { "keydown" } else { "keyup" },
                key: key.key.clone(),
                code: key.code.clone(),
                mods: key.mods,
                repeat: key.repeat,
            };
            let mut first_greyed: Option<(String, String, String)> = None;
            for id in order {
                let Some(binding) = self.bindings.get(&id).cloned() else {
                    continue;
                };
                let Some(action_name) = binding.hx_post.clone() else {
                    continue;
                };
                let mut matched = false;
                for t in &binding.triggers {
                    if t.event != event || t.from != Some(FromTarget::Body) {
                        continue;
                    }
                    if matches!(event, TriggerEvent::KeyDown)
                        && key.repeat
                        && !keydown_repeat_allowed(&t.filter, true)
                    {
                        continue;
                    }
                    let ok = t
                        .filter
                        .as_ref()
                        .map(|f| f.eval(&filter_ctx))
                        .unwrap_or(true);
                    if ok {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    continue;
                }
                if let Some(reason) = self.disabled_actions.get(&action_name).cloned() {
                    if first_greyed.is_none() {
                        first_greyed = Some((id, action_name, reason));
                    }
                    continue;
                }
                // A body shortcut does not blur the focused box (a click
                // does), so hold its posted value to what Enter would commit:
                // rounded and inside the current limits, never the raw text
                // (Graph review of #41 round 2). Change still fires on commit.
                self.hold_focused_number_to_commit();
                return self.fire_on(ctx, &id, event, filter_ctx, trigger_info, None, true);
            }
            if let Some((source_id, name, reason)) = first_greyed {
                return InputOutcome::Blocked {
                    name,
                    reason,
                    source_id,
                    trigger: trigger_info,
                };
            }
        }

        // (e) Unhandled — Esc never swallowed by default.
        InputOutcome::Unhandled(key)
    }

    fn activate_like_click(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        source_id: &str,
        trigger_info: TriggerInfo,
    ) -> InputOutcome {
        let binding = match self.bindings.get(source_id).cloned() {
            Some(b) => b,
            None => return InputOutcome::None,
        };
        let Some(action_name) = binding.hx_post.clone() else {
            return InputOutcome::None;
        };
        if !ctx.declared_actions.contains(&action_name) {
            panic!("hx_native: unknown action `{action_name}` at activation time");
        }
        if let Some(reason) = self.disabled_actions.get(&action_name).cloned() {
            return InputOutcome::Blocked {
                name: action_name,
                reason,
                source_id: source_id.to_string(),
                trigger: trigger_info,
            };
        }
        let args = self.action_args_for(source_id);
        if self.pending.is_pending(source_id) {
            match binding.hx_sync {
                SyncMode::Drop => return InputOutcome::None,
                SyncMode::Queue => {
                    self.pending.queued.insert(
                        source_id.to_string(),
                        QueuedRelease {
                            source_id: source_id.to_string(),
                            action_name,
                            args,
                            trigger: trigger_info,
                            indicator_id: binding.hx_indicator.clone(),
                        },
                    );
                    return InputOutcome::None;
                }
            }
        }
        self.emit_action(
            ctx,
            source_id,
            &action_name,
            args,
            trigger_info,
            binding.hx_indicator.clone(),
        )
    }

    fn complete(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        request: RequestId,
        result: Result<(), Reason>,
    ) {
        if self.pending.timed_out.contains_key(&request) {
            // Late complete after timeout: no-op.
            return;
        }
        ctx.requests.owners.remove(&request);
        let Some(source_id) = self.pending.by_request.remove(&request) else {
            self.pending.unknown_completes.push(request);
            #[cfg(debug_assertions)]
            eprintln!("hx_native: complete() for unknown request {request}");
            return;
        };
        let entry = self.pending.by_source.remove(&source_id);
        let indicator = entry.as_ref().and_then(|e| e.indicator_id.clone());
        self.set_pending_marks(&source_id, indicator.as_deref(), false);

        if let Err(reason) = result {
            // Show like Blocked: set data-hx-reason on the source element.
            if let Some(nid) = self.doc.get_element_by_id(&source_id) {
                let mut mutr = self.doc.mutate();
                mutr.set_attribute(
                    nid,
                    QualName::new(None, ns!(), LocalName::from("data-hx-reason")),
                    &reason.0,
                );
            }
            self.mark_dom_dirty();
            self.mark_paint_dirty();
        }

        // Release queued action if any.
        if let Some(q) = self.pending.queued.remove(&source_id) {
            let outcome = self.emit_action(
                ctx,
                &q.source_id,
                &q.action_name,
                q.args,
                q.trigger,
                q.indicator_id,
            );
            self.released_outcomes.push(outcome);
        }
        // Issue #89: a finished request, failed or not, runs the catch-up.
        self.run_catch_up(ctx, &source_id);
    }

    /// Age pending requests by one frame; a timed-out request still runs
    /// its source's catch-up (issue #89).
    fn tick_pending(&mut self, ctx: &mut ActionCtx<'_>, timeout_frames: u32) {
        for source_id in self.pending.tick_frames(timeout_frames) {
            self.run_catch_up(ctx, &source_id);
        }
    }

    /// Style + layout + raster when dirty (or forced). `plan == None` means the
    /// frame was clean and raster was skipped.
    fn render(
        &mut self,
        size: SurfaceSize,
        paint_mode: PaintMode,
    ) -> Result<RenderResult, EngineError> {
        let SurfaceSize {
            width,
            height,
            scale,
        } = size;
        let size_changed = self.layout_viewport != (width, height);
        let scale_changed = (self.layout_scale - scale).abs() > f64::EPSILON;
        let first_frame = self.cached_rgba.is_none();
        let force_full = self.force_full_repaint || first_frame || size_changed || scale_changed;

        self.layout_viewport = (width, height);
        self.layout_scale = scale;

        // Spec: skip clean frames — no raster when nothing is damaged and size is unchanged.
        // Pending damage (swap old ink, closed overlay, fold full-surface) must not skip.
        if !self.paint_dirty && !force_full && self.pending_damage.is_empty() {
            return Ok(RenderResult {
                plan: None,
                style_layout_passes: 0,
            });
        }

        let (plan, passes) = self.resolve_and_plan_damage(width, height, scale, force_full);
        // resolve_and_plan_damage already drained pending_damage and replaced
        // last_bounds. If raster fails, force a full repaint next time so those
        // drained rects / bounds are not silently dropped.
        let rgba = match self.rasterize_plan(&plan, width, height, scale, paint_mode) {
            Ok(rgba) => rgba,
            Err(e) => {
                self.force_full_repaint = true;
                self.paint_dirty = true;
                return Err(e);
            }
        };

        // #34 round 5: a gutter chain longer than the per-frame pass cap
        // left a reserve follow-up (`resolve_and_plan_damage` moved it into
        // `needs_resolve`); keep the surface dirty so the next paint finishes
        // it without waiting for input.
        self.paint_dirty = self.needs_resolve;
        self.force_full_repaint = false;
        self.cached_rgba = Some(rgba);
        Ok(RenderResult {
            plan: Some(plan),
            style_layout_passes: passes,
        })
    }

    /// Run style + layout once, then compute a damage plan. Returns the plan and
    /// the number of style+layout passes run.
    fn resolve_and_plan_damage(
        &mut self,
        width: u32,
        height: u32,
        scale: f64,
        force_full: bool,
    ) -> (DamagePlan, u64) {
        let had_previous = !self.last_bounds.is_empty() && self.cached_rgba.is_some();
        let previous = std::mem::take(&mut self.last_bounds);
        let mut pending = std::mem::take(&mut self.pending_damage);
        // Open list must stay in the damage set while visible (and after pick
        // clears open_select, close_select already pushed via pending_damage).
        // Clamp like `push_pending_damage` — overflowing lists must not report
        // rects past the surface (hosts upload damage to the texture).
        pending.extend(
            select_damage_rects(self.open_select.as_ref(), scale)
                .into_iter()
                .map(|r| r.clamp_to_surface(width, height))
                .filter(|r| !r.is_empty()),
        );

        let styled_scale = self.doc.viewport().scale_f64();
        self.doc.set_viewport(BlitzViewport::new(
            width,
            height,
            scale as f32,
            ColorScheme::Light,
        ));
        // Issue #107: stylo snaps some computed values (a 1 px border becomes
        // whole device pixels) to the device scale at cascade time. Blitz only
        // re-shapes its text when the scale changes, so nodes styled at the
        // old scale (`load_html` parses at scale 1, a window moves to another
        // monitor) would keep the old numbers while a later swap styles new
        // nodes at the new scale. Recascade everything once, like Blitz does
        // for a colour-scheme change.
        if (styled_scale - scale as f32 as f64).abs() > f64::EPSILON {
            if let Some(root) = self.doc.try_root_element().map(|n| n.id) {
                if let Some(node) = self.doc.get_node_mut(root) {
                    node.set_restyle_hint(RestyleHint::recascade_subtree());
                }
            }
        }

        // Graph review (PR #24 follow-up): drain damage an input-time
        // `ensure_resolved` already carried (its one-shot restyle flags are
        // long gone by now). Merged into the probe below; a force-full /
        // first paint discards it since the whole surface repaints anyway.
        let carried = std::mem::take(&mut self.carried_style_damage);

        // Probe: public resolve_stylist first so Node::damage() is readable,
        // then full resolve (stylist is a no-op; layout + paint-tree run).
        let mut style_damaged = if force_full || !had_previous {
            Vec::new()
        } else {
            self.doc.resolve_stylist(0.0);
            let mut v = collect_style_damaged(&self.doc);
            v.extend(carried);
            v
        };

        // Style + layout once. A second pass is allowed only when the hovered
        // node changes (Graph: at most 2 passes total per paint).
        let hover_before = self.doc.get_hover_node_id();
        // #13: resolve_doc runs Blitz resolve + ellipsis cache bookkeeping.
        let mut passes = self.resolve_doc();
        // resolve() ends with refresh_hover(); compare hover node to decide follow-up.
        let hover_changed = self.doc.get_hover_node_id() != hover_before;
        if hover_changed {
            passes += self.resolve_doc();
        }
        self.needs_resolve = std::mem::take(&mut self.gutter_followup);

        // Issue #26: offsets restored / clamped by this resolve pushed their
        // container damage after `pending` was taken; a scrollbar whose
        // geometry or pressed state changed damages its old and new rects.
        let bars: Vec<(BarGeom, Option<BarPart>)> = all_bars(&self.doc)
            .into_iter()
            .map(|g| {
                let p = self
                    .scroll_pressed
                    .filter(|p| p.nid == g.nid)
                    .map(|p| p.part);
                (g, p)
            })
            .collect();
        if bars != self.last_bars {
            // Only bars that changed (old rect and new rect).
            let changed = self
                .last_bars
                .iter()
                .filter(|b| !bars.contains(b))
                .chain(bars.iter().filter(|b| !self.last_bars.contains(b)));
            for (g, _) in changed {
                let r = g.bar.as_device(scale).clamp_to_surface(width, height);
                if !r.is_empty() {
                    pending.push(r);
                }
            }
            self.last_bars = bars;
        }
        // Issue #35: an engine-drawn slider whose thumb / box / greying /
        // clip changed damages its old and new rects (nothing else).
        let sliders = all_sliders(&self.doc);
        if sliders != self.last_sliders {
            let changed = self
                .last_sliders
                .iter()
                .filter(|g| !sliders.contains(g))
                .chain(sliders.iter().filter(|g| !self.last_sliders.contains(g)));
            for g in changed {
                let r = g.damage_rect(scale).clamp_to_surface(width, height);
                if !r.is_empty() {
                    pending.push(r);
                }
            }
            self.last_sliders = sliders;
        }
        // Same-id swap: PlainEditor now exists — seed from value + caret at end
        // before damage planning so partial frames match a full repaint. Its
        // refocus restyle and field damage join this frame's plan.
        if self.text_edit.as_ref().is_some_and(|s| s.needs_prepare) {
            self.ensure_text_edit_prepared();
            if !force_full && had_previous {
                style_damaged.extend(self.carried_style_damage.drain());
            }
        }
        pending.append(&mut self.pending_damage);
        // Opt-in grouping for inputs a load/swap just (re)built (issue #40),
        // with its damage folded into this frame's plan.
        if self.group_display_pending {
            self.apply_pending_group_display();
            pending.extend(std::mem::take(&mut self.pending_damage));
        }

        let plan = self.compute_plan(
            width,
            height,
            scale,
            force_full,
            had_previous,
            &previous,
            &style_damaged,
            pending,
        );

        self.last_bounds = snapshot_bounds(&self.doc, scale);
        (plan, passes)
    }

    /// Merge detected damage with any pending rects (swap old boxes, closed
    /// overlays, fold full-surface, …). A pending full-surface rect forces
    /// `full_repaint` — never dropped in favour of a local rect.
    #[allow(clippy::too_many_arguments)]
    fn compute_plan(
        &self,
        width: u32,
        height: u32,
        scale: f64,
        force_full: bool,
        had_previous: bool,
        previous: &BoundsSnapshot,
        style_damaged: &[blitz_dom::NodeId],
        pending: Vec<IntRect>,
    ) -> DamagePlan {
        let pending_full = pending.iter().any(|r| r.is_full_surface(width, height));
        if force_full || !had_previous || pending_full {
            return DamagePlan::full(width, height);
        }

        let mut raw = compute_raw_damage(&self.doc, scale, width, height, previous, style_damaged);
        raw.extend(pending);
        let merged = merge_damage_rects(raw, width, height);
        // Dirty paint with no detected rects: fall back to full (safer than a no-op).
        if merged.rects.is_empty() {
            DamagePlan::full(width, height)
        } else {
            merged
        }
    }

    fn rasterize_plan(
        &mut self,
        plan: &DamagePlan,
        width: u32,
        height: u32,
        scale: f64,
        paint_mode: PaintMode,
    ) -> Result<RgbaImage, EngineError> {
        // Blink-off caret rect, computed against the full (pre-truncation)
        // layout before the ellipsis session below swaps any truncated
        // TextLayouts in. The node is the focused text input, never an
        // ellipsis-truncated box, so the untruncated geometry here matches
        // what the session will later restore.
        let blink_off_caret: Option<(NodeId, IntRect)> = self
            .text_edit
            .as_ref()
            .filter(|s| !s.caret_visible)
            .map(|s| s.node_id)
            .and_then(|nid| {
                if has_selection(&mut self.doc, nid) {
                    None
                } else {
                    caret_device_rect(&mut self.doc, nid, scale, width, height).map(|r| (nid, r))
                }
            });

        // Issue #12: swap cached truncated TextLayouts for paint only. DOM text
        // stays full; EllipsisPaintSession restores layouts on Drop (also on
        // `?` / panic) — no document needs_resolve and no truncated geometry.
        let mut session = prepare_ellipsis_for_paint(
            &mut self.doc,
            &mut self.ellipsis_cache,
            &mut self.ellipsis_shaper,
            &self.fonts,
        );

        if self.panic_next_paint {
            self.panic_next_paint = false;
            panic!("hx_native test: panic after ellipsis swap");
        }

        if self.fail_next_paint {
            self.fail_next_paint = false;
            // Session Drop restores layouts before this error propagates.
            return Err(EngineError::Paint("test forced paint error".into()));
        }

        let full = plan.full_repaint
            || plan.rects.len() == 1 && plan.rects[0] == IntRect::full_surface(width, height);

        let mut rgba = if full {
            paint_rgba(session.doc_mut(), width, height, scale)
        } else {
            let mut frame = self.cached_rgba.take().unwrap_or_else(|| RgbaImage {
                width,
                height,
                pixels: vec![0u8; (width as usize) * (height as usize) * 4],
            });
            if frame.width != width || frame.height != height {
                frame = RgbaImage {
                    width,
                    height,
                    pixels: vec![0u8; (width as usize) * (height as usize) * 4],
                };
            }
            paint_damage_into(
                session.doc_mut(),
                &mut frame,
                scale,
                &plan.rects,
                paint_mode,
            )?;
            Ok(frame)
        };

        // Blink-off: re-rasterize the caret damage rect with FOCUS cleared so
        // Blitz skips caret drawing (render.rs is_focussed check). No colour
        // overlay — that clipped glyphs and flattened gradients/stripes. Done
        // while the ellipsis session above is still installed: dropping the
        // session first would restore full (uncut) TextLayouts, so this redraw
        // would re-rasterize any ellipsis box overlapping the caret rect with
        // its full, uncut text instead of the truncated text the rest of the
        // frame shows.
        if let (Ok(frame), Some((nid, r))) = (rgba.as_mut(), blink_off_caret) {
            let mut suppress = SuppressCaretFocus::clear(session.doc_mut(), nid);
            let result = paint_damage_into(suppress.doc_mut(), frame, scale, &[r], paint_mode);
            // Drop restores ElementState::FOCUS before overlays / before the
            // ellipsis session restores the full text layouts below.
            drop(suppress);
            result?;
        }

        // Explicit drop before returning so restore order is obvious; also runs
        // on `?` / panic via Drop.
        drop(session);
        let mut frame = rgba?;

        // Issue #35: engine-drawn typed-field bevels and range sliders (under
        // the scrollbars, which own the reserved gutter strip).
        paint_field_bevels(&self.doc, &mut frame, scale);
        paint_sliders(&self.doc, &mut frame, scale);

        // Issue #26: engine-drawn scrollbars (under select overlays).
        paint_scrollbars(&self.doc, &mut frame, scale, self.scroll_pressed);

        // Engine-drawn select face button + open list (above Blitz content).
        paint_select_overlays(
            &self.doc,
            &mut frame,
            scale,
            self.open_select.as_ref(),
            self.select_hover_row
                .or_else(|| self.open_select.as_ref().map(|o| o.highlight)),
            &self.select_nids,
        );
        Ok(frame)
    }

    /// Read an attribute from an element by id.
    fn attr(&self, id: &str, name: &str) -> Option<String> {
        let nid = self.doc.get_element_by_id(id)?;
        let node = self.doc.get_node(nid)?;
        node.attrs().and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.local == name)
                .map(|a| a.value.to_string())
        })
    }
}

/// Temporarily clear `ElementState::FOCUS` for one paint so Blitz skips the
/// caret (`is_focussed()` at paint time) without `snapshot_node_and`, ancestor
/// dirty marks, or IME toggling. Restores FOCUS on Drop (incl. panic / `?`).
struct SuppressCaretFocus<'a> {
    doc: &'a mut HtmlDocument,
    nid: NodeId,
    had_focus: bool,
}

impl<'a> SuppressCaretFocus<'a> {
    fn clear(doc: &'a mut HtmlDocument, nid: NodeId) -> Self {
        let had_focus = doc
            .get_node_mut(nid)
            .and_then(|n| n.element_data_mut())
            .map(|el| {
                let had = el.element_state.contains(style_dom::ElementState::FOCUS);
                if had {
                    el.element_state.remove(style_dom::ElementState::FOCUS);
                }
                had
            })
            .unwrap_or(false);
        Self {
            doc,
            nid,
            had_focus,
        }
    }

    fn doc_mut(&mut self) -> &mut HtmlDocument {
        self.doc
    }
}

impl Drop for SuppressCaretFocus<'_> {
    fn drop(&mut self) {
        if !self.had_focus {
            return;
        }
        if let Some(el) = self
            .doc
            .get_node_mut(self.nid)
            .and_then(|n| n.element_data_mut())
        {
            el.element_state.insert(style_dom::ElementState::FOCUS);
        }
    }
}

/// Walk a freshly parsed tree (document or fragment) and enforce the locked
/// attribute dialect. Does **not** run on live-DOM reindex after the engine
/// writes `data-hx-reason`.
fn validate_loaded_tree(
    declared_actions: &HashSet<String>,
    doc: &HtmlDocument,
    known_ids: &HashSet<String>,
) -> Result<(), EngineError> {
    let root = doc.root_node().id;
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in &node.children {
            stack.push(child);
        }
        if !node.is_element() {
            continue;
        }
        let (id_attr, attrs) = element_id_and_attrs(node);
        let tag = node
            .element_data()
            .map(|el| el.name.local.to_string())
            .unwrap_or_default();
        validate_loaded_attrs(&id_attr, &attrs, known_ids, Some(&tag))?;
        let input_ty = attrs
            .iter()
            .find(|(k, _)| k == "type")
            .map(|(_, v)| v.as_str());
        if let Some(binding) =
            ElementBinding::from_attrs_with_tag(&id_attr, &attrs, Some(&tag), input_ty)?
        {
            if let Some(ref action) = binding.hx_post {
                if !declared_actions.contains(action) {
                    return Err(EngineError::UndeclaredAction {
                        name: action.clone(),
                        source_id: binding.id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

type BindingIndex = (
    HashMap<String, ElementBinding>,
    HashMap<String, Vec<String>>,
);

/// Index element bindings and sections (section_id -> subscribed slices).
fn index_bindings(
    declared_actions: &HashSet<String>,
    doc: &HtmlDocument,
) -> Result<BindingIndex, EngineError> {
    let mut bindings = HashMap::new();
    let mut sections = HashMap::new();
    let root = doc.root_node().id;
    let mut stack = vec![root];
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in &node.children {
            stack.push(child);
        }
        if !node.is_element() {
            continue;
        }
        let (id_attr, attrs) = element_id_and_attrs(node);
        let tag = node
            .element_data()
            .map(|el| el.name.local.to_string())
            .unwrap_or_default();
        let input_ty = attrs
            .iter()
            .find(|(k, _)| k == "type")
            .map(|(_, v)| v.as_str());
        if let Some(binding) =
            ElementBinding::from_attrs_with_tag(&id_attr, &attrs, Some(&tag), input_ty)?
        {
            if let Some(ref action) = binding.hx_post {
                if !declared_actions.contains(action) {
                    // Click-time unknown still fails loudly; keep load-time rule on reindex.
                    return Err(EngineError::UndeclaredAction {
                        name: action.clone(),
                        source_id: binding.id.clone(),
                    });
                }
            }
            if binding.is_section() {
                sections.insert(binding.id.clone(), binding.slice_subs.clone());
            }
            bindings.insert(binding.id.clone(), binding);
        }
    }
    Ok((bindings, sections))
}

fn parse_html_document(html: &str, layout_viewport: (u32, u32), fonts: &HostFonts) -> HtmlDocument {
    parse_html_document_scaled(html, layout_viewport, 1.0, fonts)
}

/// `net_provider: None`: CSS `@font-face` (and every other resource) is
/// never fetched (issue #28); host fonts come only from `EngineConfig::fonts`.
fn parse_html_document_scaled(
    html: &str,
    layout_viewport: (u32, u32),
    scale: f64,
    fonts: &HostFonts,
) -> HtmlDocument {
    let font_ctx = bundled_font_context(fonts);
    let (w, h) = layout_viewport;
    HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(BlitzViewport::new(w, h, scale as f32, ColorScheme::Light)),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            font_ctx: Some(font_ctx),
            style_threading: StyleThreading::Sequential,
            incremental: Some(true),
            net_provider: None,
            ..Default::default()
        },
    )
}

fn collect_element_ids(doc: &HtmlDocument) -> HashSet<String> {
    collect_subtree_ids(doc, doc.root_node().id, true)
}

/// Ids under `root` in document order walk. `include_root` controls whether
/// `root`'s own `id` is included (outerHTML removes the target; innerHTML keeps it).
fn collect_subtree_ids(doc: &HtmlDocument, root: NodeId, include_root: bool) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut stack = vec![root];
    let mut first = true;
    while let Some(nid) = stack.pop() {
        let Some(node) = doc.get_node(nid) else {
            continue;
        };
        for &child in &node.children {
            stack.push(child);
        }
        if !node.is_element() {
            continue;
        }
        let take = if first {
            first = false;
            include_root
        } else {
            true
        };
        if !take {
            continue;
        }
        if let Some(id) = node.attrs().and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.local == "id")
                .map(|a| a.value.to_string())
        }) {
            if !id.is_empty() {
                ids.insert(id);
            }
        }
    }
    ids
}

fn ids_removed_by_swap(doc: &HtmlDocument, target: NodeId, swap: SwapMode) -> HashSet<String> {
    match swap {
        // innerHTML replaces descendants only; the target element (and siblings) stay.
        SwapMode::InnerHtml => collect_subtree_ids(doc, target, false),
        // outerHTML replaces the target node itself.
        SwapMode::OuterHtml => collect_subtree_ids(doc, target, true),
    }
}

/// Validate one `SliceSource::input_limits` entry (issue #16): finite
/// bounds, `min <= max`, and `step` (when present) finite and `> 0`.
/// Floor for `InputLimits.step` when `Some` (PR27 round 2, blocker 2). Below
/// this, `step_decimal_places`'s capped decimal-digit count (10) can no
/// longer represent the step precisely, and formatting/snapping would be
/// guesswork rather than exact — so it is rejected at validation instead.
const MIN_STEP: f64 = 1e-10;

fn validate_input_limits(
    action: &str,
    input: &str,
    limits: &InputLimits,
) -> Result<(), EngineError> {
    // Issue #101: `max: None` (no max) skips the max checks.
    let lmax = limits.max.unwrap_or(limits.min);
    let reason = if !limits.min.is_finite() || !lmax.is_finite() {
        Some("min/max must be finite numbers".to_string())
    } else if limits.min.abs() > MAX_SAFE_NUMBER || lmax.abs() > MAX_SAFE_NUMBER {
        Some(format!(
            "min/max must stay within ±{MAX_SAFE_NUMBER} (±2^53, the safe integer range)"
        ))
    } else if limits.min > lmax {
        Some(format!("min ({}) must be <= max ({})", limits.min, lmax))
    } else if let Some(step) = limits.step {
        if !step.is_finite() || step <= 0.0 {
            Some(format!("step ({step}) must be > 0 when present"))
        } else if step < MIN_STEP {
            Some(format!("step ({step}) must be >= {MIN_STEP} when present"))
        } else {
            None
        }
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidInputLimits {
            action: action.to_string(),
            input: input.to_string(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Attribute `name` of node `nid`, if any (issue #16: inputs matched by
/// `name=` rather than a stable `id`).
fn node_attr(doc: &HtmlDocument, nid: NodeId, name: &str) -> Option<String> {
    doc.get_node(nid)?
        .attrs()?
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| a.value.to_string())
}

/// Stable `id` attribute of node `nid`, if any (non-empty).
fn node_id_attr(doc: &HtmlDocument, nid: NodeId) -> Option<String> {
    node_attr(doc, nid, "id").filter(|s| !s.is_empty())
}

fn element_id_and_attrs(node: &blitz_dom::Node) -> (String, Vec<(String, String)>) {
    let id_attr = node
        .attrs()
        .and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.local == "id")
                .map(|a| a.value.to_string())
        })
        .unwrap_or_default();
    let attrs: Vec<(String, String)> = node
        .attrs()
        .map(|list| {
            list.iter()
                .map(|a| (a.name.local.to_string(), a.value.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (id_attr, attrs)
}

fn class_tokens(class: &str) -> impl Iterator<Item = &str> {
    class.split_whitespace()
}

fn add_class_token(class: &str, token: &str) -> String {
    let mut tokens: Vec<&str> = class_tokens(class).collect();
    if !tokens.contains(&token) {
        tokens.push(token);
    }
    tokens.join(" ")
}

fn remove_class_token(class: &str, token: &str) -> String {
    class_tokens(class)
        .filter(|t| *t != token)
        .collect::<Vec<_>>()
        .join(" ")
}

// Silence unused import when local_name is only needed in some paths.
#[allow(unused_imports)]
use local_name as _;

/// Issue #94 (Graph review): attribute `name` of element `node`.
fn attr_of(node: &blitz_dom::node::Node, name: &str) -> Option<String> {
    node.attrs().and_then(|attrs| {
        attrs
            .iter()
            .find(|a| &*a.name.local == name)
            .map(|a| a.value.to_string())
    })
}
