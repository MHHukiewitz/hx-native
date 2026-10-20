//! Core `Engine`: surfaces, load, sync, input, complete, paint.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use blitz_dom::{
    local_name, ns, qual_name, DocumentConfig, LocalName, NodeId, QualName, StyleThreading,
};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};

use crate::damage::{
    collect_style_damaged, compute_raw_damage, ink_box_device, merge_damage_rects, snap_outward,
    snapshot_bounds, BoundsSnapshot, DamagePlan, IntRect,
};
use crate::dialect::{validate_loaded_attrs, ElementBinding, SwapMode, SyncMode};
use crate::ellipsis::{prepare_ellipsis_for_paint, EllipsisCache, EllipsisShaper};
use crate::error::EngineError;
use crate::focus::{focusable_ids, ids_in_document_order, step_focus};
use crate::fonts::bundled_font_context;
use crate::form_args::{
    collect_action_args, is_number_input, is_numeric_input, set_range_value_from_x,
    step_numeric_value,
};
use crate::form_controls::{
    activate_checkable, control_is_disabled, form_control_at_point, select_options, select_radio,
    select_set_index, select_step, sync_outputs_for, toggle_checkbox, FormControlKind,
    FormControlRef,
};
use crate::memory::estimate_surface_engine_bytes;
use crate::paint::{paint_damage_into, paint_rgba};
use crate::pending::{PendingEntry, PendingState, PendingTimeout, QueuedRelease};
use crate::select_ui::{
    collect_select_nids, control_rect_of, list_row_at, open_select_state, paint_select_overlays,
    select_damage_rects, select_row_damage_rect, select_ua_stylesheet, OpenSelect, SELECT_MAX_ROWS,
};
use crate::text_edit::{
    apply_unfocused_display, backspace, caret_device_rect, commit_field, delete_forward,
    delete_selection, editor_text, filter_number_insert, has_selection, insert_text, is_disabled,
    is_editable_input, move_end, move_home, move_left, move_right, place_caret_at,
    prepare_focus_display, resolve_step_target, select_all, selected_text, step_and_regroup,
    step_direction, sync_value_from_editor, TextEditSession, CARET_BLINK_HALF_PERIOD,
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
    bindings: HashMap<String, ElementBinding>,
    /// section_id -> subscribed slice names
    sections: HashMap<String, Vec<String>>,
    /// last seen slice versions (subscribed only)
    slice_versions: HashMap<String, u64>,
    /// action name -> Disabled reason (from actions slice)
    disabled_actions: HashMap<String, String>,
    pending: PendingState,
    released_outcomes: Vec<InputOutcome>,
    hover_id: Option<String>,
    press: Option<PressState>,
    focus: FocusState,
    /// Physical viewport size used for layout / hit-testing (updated on paint).
    layout_viewport: (u32, u32),
    /// Last paint scale factor (hidpi). Full repaint when this changes.
    layout_scale: f64,
    needs_resolve: bool,
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
    /// Open Win95 `<select>` drop list (engine-drawn overlay).
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
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
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
        let size = SurfaceSize::from_viewport(&viewport);
        let mut live = LiveSurface::build(&self.declared_actions, html, size)?;
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
                let live =
                    LiveSurface::build(&self.declared_actions, &surface.source_html, surface.size)?;
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
        let last_used = self.touch_lru();
        self.frame_count = self.frame_count.saturating_add(1);
        let paint_mode = self.config.paint_mode;
        let timeout_frames = self.config.pending_timeout_frames;

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

        live.pending.tick_frames(timeout_frames);
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
            ..
        } = self;
        match surfaces.get_mut(&id).and_then(|s| s.live.as_mut()) {
            Some(live) => {
                let mut ctx = ActionCtx {
                    surface: id,
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
            ..
        } = self;
        let Some(live) = surfaces.get_mut(&owner).and_then(|s| s.live.as_mut()) else {
            requests.owners.remove(&request);
            return;
        };
        let mut ctx = ActionCtx {
            surface: owner,
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
        let timeout_frames = self.config.pending_timeout_frames;
        for live in self.surfaces.values_mut().filter_map(|s| s.live.as_mut()) {
            live.pending.tick_frames(timeout_frames);
        }
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
        InputEvent::PointerMove { .. } => InputOutcome::None,
    }
}

impl LiveSurface {
    /// Parse + validate + index + one style/layout pass.
    fn build(
        declared_actions: &HashSet<String>,
        html: &str,
        size: SurfaceSize,
    ) -> Result<Self, EngineError> {
        let layout_viewport = (size.width, size.height);
        let layout_scale = size.scale;
        let mut doc = parse_html_document_scaled(html, layout_viewport, layout_scale);

        // HTML contract §2: strict dialect on loaded HTML, then index bindings.
        let known_ids = collect_element_ids(&doc);
        validate_loaded_tree(declared_actions, &doc, &known_ids)?;
        let (bindings, sections) = index_bindings(declared_actions, &doc)?;

        doc.add_user_agent_stylesheet(select_ua_stylesheet());
        doc.resolve(0.0);
        let select_nids = collect_select_nids(&doc);
        Ok(Self {
            doc,
            bindings,
            sections,
            slice_versions: HashMap::new(),
            disabled_actions: HashMap::new(),
            pending: PendingState::default(),
            released_outcomes: Vec::new(),
            hover_id: None,
            press: None,
            focus: FocusState::default(),
            layout_viewport,
            layout_scale,
            needs_resolve: false,
            paint_dirty: true,
            force_full_repaint: true,
            cached_rgba: None,
            last_bounds: BoundsSnapshot::new(),
            pending_damage: Vec::new(),
            range_drag_id: None,
            range_drag_released: None,
            open_select: None,
            select_hover_row: None,
            select_nids,
            consume_escape_keyup: false,
            ellipsis_cache: EllipsisCache::default(),
            ellipsis_shaper: None,
            resolve_count: 1,
            fail_next_paint: false,
            panic_next_paint: false,
            text_edit: None,
        })
    }

    fn resolve_doc(&mut self) {
        self.doc.resolve(0.0);
        self.resolve_count = self.resolve_count.saturating_add(1);
    }

    fn mark_dom_dirty(&mut self) {
        self.needs_resolve = true;
        self.ellipsis_cache.clear();
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
                let (x, y, w, h) = bounds.ink;
                let r = snap_outward(x, y, w, h, sw, sh);
                self.push_pending_damage(r);
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
        let frag_doc = parse_html_document(html, self.layout_viewport);
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

        self.slice_versions = current;

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

        // Record old subtree ink from the last paint snapshot (no resolve here —
        // sync must not run style/layout). compute_plan merges these so old
        // boxes survive NodeId recycle after the swap.
        self.queue_pending_subtree_damage(target_id);

        match swap {
            SwapMode::InnerHtml => {
                let mut mutr = self.doc.mutate();
                mutr.set_inner_html(target_id, html);
            }
            SwapMode::OuterHtml => {
                let wrapper_id = {
                    let mut mutr = self.doc.mutate();
                    let wrapper_id = mutr.create_element(qual_name!("div"), vec![]);
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
            }
        }

        self.mark_dom_dirty();
        self.mark_paint_dirty();
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

    /// Apply greying from the reserved `actions` slice.
    ///
    /// `force_dom_pass` is set when section swaps (or the initial full render)
    /// may have replaced elements that still need the current greying written.
    /// When the desired disabled/reason map matches `disabled_actions` and no
    /// DOM pass is forced, this is a no-op: no attribute writes and no paint dirty.
    fn apply_action_states(&mut self, states: Vec<(String, ActionState)>, force_dom_pass: bool) {
        let mut new_disabled: HashMap<String, String> = HashMap::new();
        for (name, state) in &states {
            if let ActionState::Disabled { reason } = state {
                new_disabled.insert(name.clone(), reason.clone());
            }
        }

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

        {
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
        self.mark_dom_dirty();
        self.mark_paint_dirty();
        self.reconcile_open_select();
        self.reconcile_text_edit();
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
            self.resolve_doc();
            self.needs_resolve = false;
        }
    }

    fn input(&mut self, ctx: &mut ActionCtx<'_>, ev: InputEvent) -> InputOutcome {
        match ev {
            InputEvent::Text(s) => self.on_text(s),
            InputEvent::Paste(s) => self.on_paste(s),
            InputEvent::Copy => self.on_copy(false),
            InputEvent::Cut => self.on_copy(true),
            InputEvent::PointerLeave => self.on_pointer_leave(),
            InputEvent::PointerMove { x, y, .. } => self.on_pointer_move(x, y),
            InputEvent::PointerDown { x, y, button, mods } => {
                self.on_pointer_down(ctx, x, y, button, mods)
            }
            InputEvent::PointerUp { x, y, button, mods } => {
                self.on_pointer_up(ctx, x, y, button, mods)
            }
            InputEvent::Key(key) => self.on_key(ctx, key),
        }
    }

    fn hit_id(&mut self, x: f32, y: f32) -> Option<String> {
        self.ensure_resolved();
        let doc = &self.doc;
        let nid = doc.element_from_point(x, y)?;
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
        self.ensure_resolved();
        if let Some(ref rid) = self.range_drag_id.clone() {
            if !self.element_is_disabled_id(rid) {
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
        self.ensure_resolved();
        self.range_drag_released = None;

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
                self.press = Some(PressState {
                    id: eid.clone(),
                    button,
                    mods,
                });
                self.reconcile_open_select_after_focus();
                return InputOutcome::Press { id };
            }

            if editable {
                let focus_out = self.focus_text_input(ctx, eid);
                if let Some(nid) = self.doc.get_element_by_id(eid) {
                    place_caret_at(&mut self.doc, nid, x, y);
                    self.reset_caret_blink();
                    self.queue_text_field_damage(nid);
                }
                self.range_drag_id = None;
                self.press = Some(PressState {
                    id: eid.clone(),
                    button,
                    mods,
                });
                // Same class as #18 HOLD #3: close an open select when focus
                // moves to a text field (incl. Secondary / right-click).
                self.reconcile_open_select_after_focus();
                return focus_out.unwrap_or(InputOutcome::Press { id });
            }

            // Click outside a text field (or on another control): commit/blur edit.
            if self.text_edit.is_some()
                && self.text_edit.as_ref().map(|s| s.id.as_str()) != Some(eid.as_str())
            {
                self.blur_text_edit_queued(ctx);
            }

            if let Some(nid) = self.doc.get_element_by_id(eid) {
                self.doc.set_focus_to(nid);
                self.focus.focused_id = Some(eid.clone());
            }
        } else if let Some(ref c) = ctrl {
            if self.text_edit.is_some() {
                self.blur_text_edit_queued(ctx);
            }
            self.doc.set_focus_to(c.node_id);
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
                        self.apply_range_from_x(eid, x);
                        self.range_drag_id = Some(eid.clone());
                    }
                }
            }
        }

        if let Some(ref eid) = id {
            self.press = Some(PressState {
                id: eid.clone(),
                button,
                mods,
            });
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
        // Range drag commit: clear drag, fire change once, suppress click.
        let mut suppress_click = false;
        if let Some(ref rid) = self.range_drag_id.clone() {
            if button == PointerButton::Primary {
                if !self.element_is_disabled_id(rid) {
                    self.apply_range_from_x(rid, x);
                    let _ = sync_outputs_for(&mut self.doc, rid);
                    self.mark_dom_dirty();
                    self.mark_paint_dirty();
                }
                self.range_drag_released = Some(rid.clone());
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

        // Spin-box step arrows (data-hx-step): commit once per click via fire_change.
        if button == PointerButton::Primary {
            if let Some(out) = self.try_spin_step(ctx, &press.id) {
                return out;
            }
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
        let Some(session) = self.text_edit.clone() else {
            return;
        };
        let Some(nid) = self.doc.get_element_by_id(&session.id) else {
            self.text_edit = None;
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
            self.text_edit = None;
            if self.focus.focused_id.as_deref() == Some(session.id.as_str()) {
                self.focus.focused_id = None;
            }
            self.doc.clear_focus();
            return;
        }
        // Same-id replacement: rebind node_id; defer prepare_focus_display until
        // style/layout (sync must not resolve). Typing before paint must not wipe value.
        let value = self.attr(&session.id, "value").unwrap_or_default();
        self.focus.focused_id = Some(session.id.clone());
        if let Some(s) = self.text_edit.as_mut() {
            s.node_id = nid;
            s.value_at_focus = value;
            s.caret_visible = true;
            s.last_blink = Instant::now();
            s.last_caret_rect = None;
            s.needs_prepare = true;
        }
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
        self.doc.set_focus_to(nid);
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

    fn try_spin_step(&mut self, ctx: &mut ActionCtx<'_>, hit_id: &str) -> Option<InputOutcome> {
        let dir = step_direction(&self.doc, hit_id)?;
        let (target_nid, target_id) = resolve_step_target(&self.doc, hit_id)?;
        // Disabled target ignores clicks.
        if self.doc.get_node(target_nid).is_some_and(is_disabled) {
            return Some(InputOutcome::None);
        }
        let _ = step_and_regroup(&mut self.doc, target_nid, dir);
        self.mark_dom_dirty();
        self.queue_text_field_damage(target_nid);
        if let Some(ref tid) = target_id {
            return Some(self.fire_change(ctx, tid));
        }
        Some(InputOutcome::Clicked {
            id: hit_id.to_string(),
        })
    }

    fn focus_text_input(&mut self, ctx: &mut ActionCtx<'_>, id: &str) -> Option<InputOutcome> {
        let nid = self.doc.get_element_by_id(id)?;
        let node = self.doc.get_node(nid)?;
        if !is_editable_input(node) || is_disabled(node) {
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
    /// Fires `change` only when the committed value differs from `value_at_focus`.
    fn commit_text_edit(
        &mut self,
        ctx: &mut ActionCtx<'_>,
        clear_focus: bool,
    ) -> Option<InputOutcome> {
        self.ensure_text_edit_prepared();
        let session = self.text_edit.take()?;
        let _ = commit_field(&mut self.doc, session.node_id);
        self.mark_dom_dirty();
        self.queue_text_field_damage(session.node_id);
        let value_now = self.attr(&session.id, "value").unwrap_or_default();
        let change = if value_now != session.value_at_focus {
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
                value_at_focus: value_now,
                caret_visible: true,
                last_blink: Instant::now(),
                last_caret_rect: session.last_caret_rect,
                needs_prepare: false,
            });
            self.reset_caret_blink();
        }
        Some(change)
    }

    fn revert_text_edit(&mut self) {
        let Some(session) = self.text_edit.take() else {
            return;
        };
        let _ = crate::form_args::write_value_attr(
            &mut self.doc,
            session.node_id,
            &session.value_at_focus,
        );
        apply_unfocused_display(&mut self.doc, session.node_id);
        self.doc.clear_focus();
        self.focus.focused_id = None;
        self.mark_dom_dirty();
        self.queue_text_field_damage(session.node_id);
    }

    fn on_text(&mut self, s: String) -> InputOutcome {
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
            let existing = editor_text(&mut self.doc, nid).unwrap_or_default();
            let limits = self
                .doc
                .get_node(nid)
                .map(crate::form_args::numeric_limits)
                .unwrap_or(crate::form_args::NumericLimits {
                    min: None,
                    max: None,
                    step: 1.0,
                });
            // Commas accepted and ignored (not inserted while focused).
            let filtered = filter_number_insert(&existing, &filtered, &limits);
            if filtered.is_empty() {
                return InputOutcome::None;
            }
            insert_text(&mut self.doc, nid, &filtered);
        } else {
            insert_text(&mut self.doc, nid, &filtered);
        }
        let _ = sync_value_from_editor(&mut self.doc, nid, is_number, false);
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
        self.on_text(prepared)
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
            let _ = sync_value_from_editor(&mut self.doc, session.node_id, is_number, false);
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
        let _ = sync_value_from_editor(&mut self.doc, nid, is_number, false);
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
        if let Some(bounds) = self.last_bounds.get(&nid) {
            let (x, y, w, h) = bounds.ink;
            self.push_pending_damage(snap_outward(x, y, w, h, sw, sh));
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
        self.fire_on(
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
        )
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
        collect_action_args(&self.doc, source_id)
    }

    fn apply_range_from_x(&mut self, id: &str, x: f32) -> bool {
        self.ensure_resolved();
        let changed = set_range_value_from_x(&mut self.doc, id, x);
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
            step_and_regroup(&mut self.doc, nid, dir)
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
                "ArrowUp" | "ArrowRight" | "ArrowDown" | "ArrowLeft"
            )
        {
            if let Some(out) = self.try_step_focused_numeric(ctx, &key.key) {
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
                } else if let Some(nid) = self.doc.get_element_by_id(nid_str) {
                    self.doc.set_focus_to(nid);
                    self.focus.focused_id = Some(nid_str.clone());
                }
            } else {
                self.doc.clear_focus();
                self.focus.focused_id = None;
            }
            self.reconcile_open_select_after_focus();
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

        self.paint_dirty = false;
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

        self.doc.set_viewport(BlitzViewport::new(
            width,
            height,
            scale as f32,
            ColorScheme::Light,
        ));

        // Probe: public resolve_stylist first so Node::damage() is readable,
        // then full resolve (stylist is a no-op; layout + paint-tree run).
        let style_damaged = if force_full || !had_previous {
            Vec::new()
        } else {
            self.doc.resolve_stylist(0.0);
            collect_style_damaged(&self.doc)
        };

        // Style + layout once. A second pass is allowed only when the hovered
        // node changes (Graph: at most 2 passes total per paint).
        let hover_before = self.doc.get_hover_node_id();
        // #13: resolve_doc runs Blitz resolve + ellipsis cache bookkeeping.
        self.resolve_doc();
        let mut passes = 1;
        // resolve() ends with refresh_hover(); compare hover node to decide follow-up.
        let hover_changed = self.doc.get_hover_node_id() != hover_before;
        if hover_changed {
            self.resolve_doc();
            passes += 1;
        }
        self.needs_resolve = false;

        // Same-id swap: PlainEditor now exists — seed from value + caret at end
        // before damage planning so partial frames match a full repaint.
        if self.text_edit.as_ref().is_some_and(|s| s.needs_prepare) {
            self.ensure_text_edit_prepared();
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

fn parse_html_document(html: &str, layout_viewport: (u32, u32)) -> HtmlDocument {
    parse_html_document_scaled(html, layout_viewport, 1.0)
}

fn parse_html_document_scaled(html: &str, layout_viewport: (u32, u32), scale: f64) -> HtmlDocument {
    let font_ctx = bundled_font_context();
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
