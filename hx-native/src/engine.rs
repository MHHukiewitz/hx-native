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
    collect_style_damaged, compute_raw_damage, merge_damage_rects, snapshot_bounds, BoundsSnapshot,
    DamagePlan, IntRect,
};
use crate::dialect::{validate_loaded_attrs, ElementBinding, SwapMode, SyncMode};
use crate::error::EngineError;
use crate::focus::{focusable_ids, ids_in_document_order, step_focus};
use crate::fonts::bundled_font_context;
use crate::form_args::{
    collect_action_args, is_numeric_input, is_range_input, set_range_value_from_x,
    step_numeric_value,
};
use crate::memory::estimate_surface_engine_bytes;
use crate::paint::{paint_damage_into, paint_rgba};
use crate::pending::{PendingEntry, PendingState, PendingTimeout, QueuedRelease};
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
    /// Primary-button drag on an `<input type="range">`.
    range_drag_id: Option<String>,
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

    /// Number of style+layout resolve passes executed inside `paint` (any
    /// surface, including hidden pre-draws) since engine creation.
    pub fn style_layout_pass_count(&self) -> u64 {
        self.style_layout_passes
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
    pub fn test_set_focus(&mut self, id: &str) {
        if let Some(live) = self
            .surfaces
            .get_mut(&SurfaceId::DEFAULT)
            .and_then(|s| s.live.as_mut())
        {
            if let Some(nid) = live.doc.get_element_by_id(id) {
                live.doc.set_focus_to(nid);
                live.focus.focused_id = Some(id.to_string());
            }
        }
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
        let mut doc = parse_html_document(html, layout_viewport);

        // HTML contract §2: strict dialect on loaded HTML, then index bindings.
        let known_ids = collect_element_ids(&doc);
        validate_loaded_tree(declared_actions, &doc, &known_ids)?;
        let (bindings, sections) = index_bindings(declared_actions, &doc)?;

        doc.resolve(0.0);
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
            layout_scale: 1.0,
            needs_resolve: false,
            paint_dirty: true,
            force_full_repaint: true,
            cached_rgba: None,
            last_bounds: BoundsSnapshot::new(),
            range_drag_id: None,
        })
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
        Ok(())
    }

    fn sync(
        &mut self,
        declared_actions: &HashSet<String>,
        slices: &dyn SliceSource,
        frags: &mut dyn FragmentSource,
    ) -> Result<DirtySet, EngineError> {
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

        self.needs_resolve = true;
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
        self.needs_resolve = true;
        self.mark_paint_dirty();
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
            self.doc.resolve(0.0);
            self.needs_resolve = false;
        }
    }

    fn input(&mut self, ctx: &mut ActionCtx<'_>, ev: InputEvent) -> InputOutcome {
        match ev {
            InputEvent::Text(s) => {
                // Reserved — first scope has no text fields.
                InputOutcome::Unhandled(KeyEvent {
                    key: s,
                    code: String::new(),
                    mods: Modifiers::default(),
                    repeat: false,
                    pressed: true,
                })
            }
            InputEvent::PointerLeave => self.on_pointer_leave(),
            InputEvent::PointerMove { x, y, .. } => self.on_pointer_move(x, y),
            InputEvent::PointerDown { x, y, button, mods } => {
                self.on_pointer_down(x, y, button, mods)
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
            let _ = self.apply_range_from_x(rid, x);
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
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    ) -> InputOutcome {
        let id = self.hit_id(x, y);
        if let Some(ref eid) = id {
            // Focus the element on press.
            if let Some(nid) = self.doc.get_element_by_id(eid) {
                self.doc.set_focus_to(nid);
                self.focus.focused_id = Some(eid.clone());
            }
            // Primary press on a range: set value from x and begin drag.
            self.range_drag_id = None;
            if button == PointerButton::Primary && self.element_is_range(eid) {
                self.apply_range_from_x(eid, x);
                self.range_drag_id = Some(eid.clone());
            }
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
        if let Some(ref rid) = self.range_drag_id.clone() {
            if button == PointerButton::Primary {
                self.apply_range_from_x(rid, x);
            }
        }
        self.range_drag_id = None;
        let up_id = self.hit_id(x, y);
        let press = self.press.take();
        let Some(press) = press else {
            return InputOutcome::Press { id: None };
        };
        if press.button != button {
            return InputOutcome::None;
        }
        // Release off target → Press{None}, no click.
        if up_id.as_deref() != Some(press.id.as_str()) {
            return InputOutcome::Press { id: None };
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

    fn element_is_range(&self, id: &str) -> bool {
        self.doc
            .get_element_by_id(id)
            .and_then(|nid| self.doc.get_node(nid))
            .is_some_and(is_range_input)
    }

    fn apply_range_from_x(&mut self, id: &str, x: f32) -> bool {
        self.ensure_resolved();
        let changed = set_range_value_from_x(&mut self.doc, id, x);
        if changed {
            self.needs_resolve = true;
            self.mark_paint_dirty();
        }
        changed
    }

    fn try_step_focused_numeric(&mut self, key: &str) -> bool {
        let Some(fid) = self.focus.focused_id.clone() else {
            return false;
        };
        let is_numeric = self
            .doc
            .get_element_by_id(&fid)
            .and_then(|nid| self.doc.get_node(nid))
            .is_some_and(is_numeric_input);
        if !is_numeric {
            return false;
        }
        let dir = match key {
            "ArrowUp" | "ArrowRight" => 1,
            "ArrowDown" | "ArrowLeft" => -1,
            _ => return false,
        };
        if step_numeric_value(&mut self.doc, &fid, dir) {
            self.needs_resolve = true;
            self.mark_paint_dirty();
        }
        true
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
        self.needs_resolve = true;
        self.mark_paint_dirty();
    }

    fn on_key(&mut self, ctx: &mut ActionCtx<'_>, key: KeyEvent) -> InputOutcome {
        let event = if key.pressed {
            TriggerEvent::KeyDown
        } else {
            TriggerEvent::KeyUp
        };

        // Number/range arrow editing (before action triggers).
        if key.pressed
            && matches!(
                key.key.as_str(),
                "ArrowUp" | "ArrowRight" | "ArrowDown" | "ArrowLeft"
            )
            && self.try_step_focused_numeric(&key.key)
        {
            return InputOutcome::None;
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
            let order = focusable_ids(&self.doc);
            let next = step_focus(&order, self.focus.focused_id.as_deref(), key.mods.shift);
            if let Some(ref nid_str) = next {
                if let Some(nid) = self.doc.get_element_by_id(nid_str) {
                    self.doc.set_focus_to(nid);
                }
            } else {
                self.doc.clear_focus();
            }
            self.focus.focused_id = next;
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
            self.needs_resolve = true;
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
        if !self.paint_dirty && !force_full {
            return Ok(RenderResult {
                plan: None,
                style_layout_passes: 0,
            });
        }

        let (plan, passes) = self.resolve_and_plan_damage(width, height, scale, force_full);
        let rgba = self.rasterize_plan(&plan, width, height, scale, paint_mode)?;

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

        let doc = &mut self.doc;
        doc.set_viewport(BlitzViewport::new(
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
            doc.resolve_stylist(0.0);
            collect_style_damaged(doc)
        };

        // Style + layout once. A second pass is allowed only when the hovered
        // node changes (Graph: at most 2 passes total per paint).
        let hover_before = doc.get_hover_node_id();
        doc.resolve(0.0);
        let mut passes = 1;
        // resolve() ends with refresh_hover(); compare hover node to decide follow-up.
        let hover_changed = doc.get_hover_node_id() != hover_before;
        if hover_changed {
            doc.resolve(0.0);
            passes += 1;
        }
        self.needs_resolve = false;

        let plan = if force_full || !had_previous {
            DamagePlan::full(width, height)
        } else {
            let raw = compute_raw_damage(doc, scale, width, height, &previous, &style_damaged);
            let merged = merge_damage_rects(raw, width, height);
            // Dirty paint with no detected rects: fall back to full (safer than a no-op).
            if merged.rects.is_empty() {
                DamagePlan::full(width, height)
            } else {
                merged
            }
        };

        self.last_bounds = snapshot_bounds(doc, scale);
        (plan, passes)
    }

    fn rasterize_plan(
        &mut self,
        plan: &DamagePlan,
        width: u32,
        height: u32,
        scale: f64,
        paint_mode: PaintMode,
    ) -> Result<RgbaImage, EngineError> {
        if plan.full_repaint
            || plan.rects.len() == 1 && plan.rects[0] == IntRect::full_surface(width, height)
        {
            return paint_rgba(&mut self.doc, width, height, scale);
        }

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
        paint_damage_into(&mut self.doc, &mut frame, scale, &plan.rects, paint_mode)?;
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
        validate_loaded_attrs(&id_attr, &attrs, known_ids)?;
        if let Some(binding) = ElementBinding::from_attrs(&id_attr, &attrs)? {
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
        if let Some(binding) = ElementBinding::from_attrs(&id_attr, &attrs)? {
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
