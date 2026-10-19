//! Core `Engine`: load, sync, input, complete, paint.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use blitz_dom::{
    local_name, ns, qual_name, DocumentConfig, LocalName, NodeId, QualName, StyleThreading,
};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};

use crate::dialect::{ElementBinding, SwapMode, SyncMode};
use crate::error::EngineError;
use crate::fonts::bundled_font_context;
use crate::paint::paint_rgba;
use crate::pending::{PendingEntry, PendingState, QueuedRelease};
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

pub struct Engine {
    config: EngineConfig,
    declared_actions: HashSet<String>,
    doc: Option<HtmlDocument>,
    bindings: HashMap<String, ElementBinding>,
    /// section_id -> subscribed slice names
    sections: HashMap<String, Vec<String>>,
    /// last seen slice versions (subscribed only)
    slice_versions: HashMap<String, u64>,
    /// action name -> Disabled reason (from actions slice)
    disabled_actions: HashMap<String, String>,
    next_request_id: u64,
    pending: PendingState,
    released_outcomes: Vec<InputOutcome>,
    hover_id: Option<String>,
    press: Option<PressState>,
    focus: FocusState,
    timings: Timings,
    /// Viewport used for layout / hit-testing (updated on paint).
    layout_viewport: (u32, u32),
    needs_resolve: bool,
    /// Test harness: assert pending timeouts were observed.
    pub(crate) frame_count: u32,
    loaded_html: Option<String>,
}

#[derive(Clone, Debug)]
struct PressState {
    id: String,
    button: PointerButton,
    #[allow(dead_code)]
    mods: Modifiers,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            declared_actions: HashSet::new(),
            doc: None,
            bindings: HashMap::new(),
            sections: HashMap::new(),
            slice_versions: HashMap::new(),
            disabled_actions: HashMap::new(),
            next_request_id: 1,
            pending: PendingState::default(),
            released_outcomes: Vec::new(),
            hover_id: None,
            press: None,
            focus: FocusState::default(),
            timings: Timings::default(),
            layout_viewport: (800, 120),
            needs_resolve: false,
            frame_count: 0,
            loaded_html: None,
        }
    }

    pub fn declare_actions(&mut self, names: &[&str]) {
        for n in names {
            self.declared_actions.insert((*n).to_string());
        }
    }

    pub fn load_html(&mut self, html: &str) -> Result<(), EngineError> {
        // Drop pending + restart request counter semantics: counter restarts only
        // in a *new* engine per spec ("counter restarts at 1 in a new engine").
        // load_html drops pending but keeps the same engine's counter? Spec:
        // "load_html drops all pending requests as part of its one full render,
        // and the counter restarts at 1 in a new engine."
        // So load_html does NOT reset the counter — only new Engine does.
        self.pending.clear_all();
        self.released_outcomes.clear();
        self.hover_id = None;
        self.press = None;
        self.focus = FocusState::default();
        self.slice_versions.clear();
        self.disabled_actions.clear();
        self.bindings.clear();
        self.sections.clear();

        let font_ctx = bundled_font_context();
        let (w, h) = self.layout_viewport;
        let mut doc = HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(BlitzViewport::new(w, h, 1.0, ColorScheme::Light)),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                font_ctx: Some(font_ctx),
                style_threading: StyleThreading::Sequential,
                incremental: Some(true),
                net_provider: None,
                ..Default::default()
            },
        );

        // Index dialect attributes; validate actions + triggers + swap/sync.
        self.index_document(&doc)?;

        doc.resolve(0.0);
        self.doc = Some(doc);
        self.loaded_html = Some(html.to_string());
        self.needs_resolve = false;
        Ok(())
    }

    fn index_document(&mut self, doc: &HtmlDocument) -> Result<(), EngineError> {
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

            if let Some(binding) = ElementBinding::from_attrs(&id_attr, &attrs)? {
                if let Some(ref action) = binding.hx_post {
                    if !self.declared_actions.contains(action) {
                        return Err(EngineError::UndeclaredAction {
                            name: action.clone(),
                            source_id: binding.id.clone(),
                        });
                    }
                }
                if binding.is_section() {
                    self.sections
                        .insert(binding.id.clone(), binding.slice_subs.clone());
                }
                self.bindings.insert(binding.id.clone(), binding);
            }
        }
        Ok(())
    }

    /// Re-index bindings after a DOM mutation (swap / attribute edits).
    fn reindex_bindings(&mut self) -> Result<(), EngineError> {
        let doc = self.doc.as_ref().ok_or(EngineError::NotLoaded)?;
        self.bindings.clear();
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
            if let Some(binding) = ElementBinding::from_attrs(&id_attr, &attrs)? {
                if let Some(ref action) = binding.hx_post {
                    if !self.declared_actions.contains(action) {
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
                self.bindings.insert(binding.id.clone(), binding);
            }
        }
        self.sections = sections;
        Ok(())
    }

    pub fn sync(
        &mut self,
        slices: &dyn SliceSource,
        frags: &mut dyn FragmentSource,
    ) -> Result<DirtySet, EngineError> {
        let t0 = Instant::now();
        if self.doc.is_none() {
            return Err(EngineError::NotLoaded);
        }

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
                self.swap_section(&section_id, &html)?;
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
                    self.swap_section(&section_id, &html)?;
                    dirty_sections.push(section_id);
                }
            }
        }

        self.slice_versions = current;

        // Apply actions slice greying.
        self.apply_action_states(slices.action_states())?;

        // Count damage before resolve (A5 instrumentation).
        let (damaged_node_count, total_element_count) = self.count_damage();

        self.ensure_resolved();

        self.timings.sync_ns = t0.elapsed().as_nanos() as u64;
        Ok(DirtySet {
            sections: dirty_sections,
            damaged_node_count,
            total_element_count,
        })
    }

    fn swap_section(&mut self, section_id: &str, html: &str) -> Result<(), EngineError> {
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

        match swap {
            SwapMode::InnerHtml => {
                let doc = self.doc.as_mut().unwrap();
                let mut mutr = doc.mutate();
                mutr.set_inner_html(target_id, html);
            }
            SwapMode::OuterHtml => {
                let wrapper_id = {
                    let doc = self.doc.as_mut().unwrap();
                    let mut mutr = doc.mutate();
                    let wrapper_id = mutr.create_element(qual_name!("div"), vec![]);
                    mutr.set_inner_html(wrapper_id, html);
                    wrapper_id
                };
                let children: Vec<NodeId> = self
                    .doc
                    .as_ref()
                    .unwrap()
                    .get_node(wrapper_id)
                    .map(|n| n.children.iter().copied().collect())
                    .unwrap_or_default();
                let doc = self.doc.as_mut().unwrap();
                let mut mutr = doc.mutate();
                if children.is_empty() {
                    mutr.remove_and_drop_node(target_id);
                } else {
                    mutr.replace_with_nodes(target_id, &children);
                }
                mutr.remove_and_drop_node(wrapper_id);
            }
        }

        self.needs_resolve = true;
        // Reindex after swap so new hx-* inside fragments are visible.
        self.reindex_bindings()?;
        Ok(())
    }

    fn resolve_selector(&self, sel: &str) -> Result<NodeId, EngineError> {
        let doc = self.doc.as_ref().ok_or(EngineError::NotLoaded)?;
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

    fn apply_action_states(
        &mut self,
        states: Vec<(String, ActionState)>,
    ) -> Result<(), EngineError> {
        self.disabled_actions.clear();
        for (name, state) in &states {
            if let ActionState::Disabled { reason } = state {
                self.disabled_actions.insert(name.clone(), reason.clone());
            }
        }

        // Collect element ids to update (avoid borrow issues).
        let updates: Vec<(String, Option<String>)> = self
            .bindings
            .values()
            .filter_map(|b| {
                let action = b.hx_post.as_ref()?;
                let reason = self.disabled_actions.get(action).cloned();
                Some((b.id.clone(), reason))
            })
            .collect();

        let _ = updates; // rebuilt below with NodeIds
        let node_updates: Vec<(NodeId, Option<String>)> = {
            let doc = self.doc.as_ref().unwrap();
            self.bindings
                .values()
                .filter_map(|b| {
                    let action = b.hx_post.as_ref()?;
                    let nid = doc.get_element_by_id(&b.id)?;
                    let reason = self.disabled_actions.get(action).cloned();
                    Some((nid, reason))
                })
                .collect()
        };
        {
            let doc = self.doc.as_mut().unwrap();
            let mut mutr = doc.mutate();
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
        Ok(())
    }

    fn count_damage(&self) -> (usize, usize) {
        let Some(doc) = self.doc.as_ref() else {
            return (0, 0);
        };
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
        if let Some(doc) = self.doc.as_mut() {
            if self.needs_resolve {
                doc.resolve(0.0);
                self.needs_resolve = false;
            }
        }
    }

    pub fn input(&mut self, ev: InputEvent) -> InputOutcome {
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
            InputEvent::PointerLeave => {
                self.hover_id = None;
                self.press = None;
                InputOutcome::Hover { id: None }
            }
            InputEvent::PointerMove { x, y, .. } => self.on_pointer_move(x, y),
            InputEvent::PointerDown { x, y, button, mods } => {
                self.on_pointer_down(x, y, button, mods)
            }
            InputEvent::PointerUp { x, y, button, mods } => self.on_pointer_up(x, y, button, mods),
            InputEvent::Key(key) => self.on_key(key),
        }
    }

    fn hit_id(&mut self, x: f32, y: f32) -> Option<String> {
        self.ensure_resolved();
        let doc = self.doc.as_ref()?;
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

    fn on_pointer_move(&mut self, x: f32, y: f32) -> InputOutcome {
        let id = self.hit_id(x, y);
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
            if let Some(doc) = self.doc.as_mut() {
                if let Some(nid) = doc.get_element_by_id(eid) {
                    doc.set_focus_to(nid);
                    self.focus.focused_id = Some(eid.clone());
                }
            }
            self.press = Some(PressState {
                id: eid.clone(),
                button,
                mods,
            });
            InputOutcome::Press { id }
        } else {
            self.press = None;
            InputOutcome::Press { id: None }
        }
    }

    fn on_pointer_up(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
        mods: Modifiers,
    ) -> InputOutcome {
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
        source_id: &str,
        event: TriggerEvent,
        filter_ctx: FilterCtx<'_>,
        trigger_info: TriggerInfo,
        clicked_fallback: Option<InputOutcome>,
    ) -> InputOutcome {
        self.fire_on(
            source_id,
            event,
            filter_ctx,
            trigger_info,
            clicked_fallback,
            false,
        )
    }

    /// `allow_from_body`: when true, match triggers that declared `from:body`/`from:document`.
    fn fire_on(
        &mut self,
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
        if !self.declared_actions.contains(&action_name) {
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
                            args: Vec::new(),
                            trigger: trigger_info,
                            indicator_id: binding.hx_indicator.clone(),
                        },
                    );
                    return InputOutcome::None;
                }
            }
        }

        self.emit_action(
            source_id,
            &action_name,
            Vec::new(),
            trigger_info,
            binding.hx_indicator.clone(),
        )
    }

    fn parent_element_id(&self, id: &str) -> Option<String> {
        let doc = self.doc.as_ref()?;
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
        source_id: &str,
        action_name: &str,
        args: Vec<(String, String)>,
        trigger: TriggerInfo,
        indicator_id: Option<String>,
    ) -> InputOutcome {
        let request = RequestId(self.next_request_id);
        self.next_request_id += 1;

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
        let doc = match self.doc.as_mut() {
            Some(d) => d,
            None => return,
        };
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
    }

    fn on_key(&mut self, key: KeyEvent) -> InputOutcome {
        let event = if key.pressed {
            TriggerEvent::KeyDown
        } else {
            TriggerEvent::KeyUp
        };

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

            let ctx = filter_ctx_from_key(&key);
            let mut matched_filter_ok = false;
            for t in &triggers {
                if matches!(event, TriggerEvent::KeyDown)
                    && key.repeat
                    && !keydown_repeat_allowed(&t.filter, true)
                {
                    continue;
                }
                let ok = t.filter.as_ref().map(|f| f.eval(&ctx)).unwrap_or(true);
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
                return self.fire_pointer_or_key_on(fid, event.clone(), ctx, trigger_info, None);
            }
        }

        // (b) Tab / Shift+Tab focus order.
        if key.pressed && key.key == "Tab" {
            self.ensure_resolved();
            let doc = self.doc.as_mut();
            if let Some(doc) = doc {
                let nid = if key.mods.shift {
                    doc.focus_prev_node()
                } else {
                    // If nothing focused, focus first focussable via next from root-ish.
                    if self.focus.focused_id.is_none() {
                        // Blitz focus_next needs an existing focus; set to root-ish first.
                        if let Some(body) = doc.query_selector("body").ok().flatten() {
                            doc.set_focus_to(body);
                        }
                    }
                    doc.focus_next_node()
                };
                if let Some(nid) = nid {
                    let id = doc.get_node(nid).and_then(|n| {
                        n.attrs().and_then(|attrs| {
                            attrs
                                .iter()
                                .find(|a| &*a.name.local == "id")
                                .map(|a| a.value.to_string())
                        })
                    });
                    self.focus.focused_id = id;
                    return InputOutcome::FocusChanged(self.focus.clone());
                }
            }
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
                    return self.activate_like_click(fid, trigger_info);
                }
            }
        }

        // (d) from:body / from:document triggers.
        {
            let ctx = filter_ctx_from_key(&key);
            let candidates: Vec<(String, _)> = self
                .bindings
                .iter()
                .flat_map(|(id, b)| {
                    b.triggers
                        .iter()
                        .filter(|t| t.event == event && t.from == Some(FromTarget::Body))
                        .map(|t| (id.clone(), t.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();
            for (id, t) in candidates {
                if matches!(event, TriggerEvent::KeyDown)
                    && key.repeat
                    && !keydown_repeat_allowed(&t.filter, true)
                {
                    continue;
                }
                let ok = t.filter.as_ref().map(|f| f.eval(&ctx)).unwrap_or(true);
                if ok {
                    let trigger_info = TriggerInfo::Key {
                        event: if key.pressed { "keydown" } else { "keyup" },
                        key: key.key.clone(),
                        code: key.code.clone(),
                        mods: key.mods,
                        repeat: key.repeat,
                    };
                    return self.fire_on(&id, event, ctx, trigger_info, None, true);
                }
            }
        }

        // (e) Unhandled — Esc never swallowed by default.
        InputOutcome::Unhandled(key)
    }

    fn activate_like_click(&mut self, source_id: &str, trigger_info: TriggerInfo) -> InputOutcome {
        let binding = match self.bindings.get(source_id).cloned() {
            Some(b) => b,
            None => return InputOutcome::None,
        };
        let Some(action_name) = binding.hx_post.clone() else {
            return InputOutcome::None;
        };
        if !self.declared_actions.contains(&action_name) {
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
        if self.pending.is_pending(source_id) {
            match binding.hx_sync {
                SyncMode::Drop => return InputOutcome::None,
                SyncMode::Queue => {
                    self.pending.queued.insert(
                        source_id.to_string(),
                        QueuedRelease {
                            source_id: source_id.to_string(),
                            action_name,
                            args: Vec::new(),
                            trigger: trigger_info,
                            indicator_id: binding.hx_indicator.clone(),
                        },
                    );
                    return InputOutcome::None;
                }
            }
        }
        self.emit_action(
            source_id,
            &action_name,
            Vec::new(),
            trigger_info,
            binding.hx_indicator.clone(),
        )
    }

    pub fn complete(&mut self, request: RequestId, result: Result<(), Reason>) {
        if self.pending.timed_out.contains_key(&request) {
            // Late complete after timeout: no-op.
            return;
        }
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
            if let Some(doc) = self.doc.as_mut() {
                if let Some(nid) = doc.get_element_by_id(&source_id) {
                    let mut mutr = doc.mutate();
                    mutr.set_attribute(
                        nid,
                        QualName::new(None, ns!(), LocalName::from("data-hx-reason")),
                        &reason.0,
                    );
                }
            }
            self.needs_resolve = true;
        }

        // Release queued action if any.
        if let Some(q) = self.pending.queued.remove(&source_id) {
            let outcome = self.emit_action(
                &q.source_id,
                &q.action_name,
                q.args,
                q.trigger,
                q.indicator_id,
            );
            self.released_outcomes.push(outcome);
        }
    }

    pub fn take_outcomes(&mut self) -> Vec<InputOutcome> {
        std::mem::take(&mut self.released_outcomes)
    }

    pub fn paint(&mut self, viewport: Viewport) -> Result<PaintOutput, EngineError> {
        let t0 = Instant::now();
        self.layout_viewport = (viewport.width, viewport.height);
        let doc = self.doc.as_mut().ok_or(EngineError::NotLoaded)?;
        doc.set_viewport(BlitzViewport::new(
            viewport.width,
            viewport.height,
            1.0,
            ColorScheme::Light,
        ));
        doc.resolve(0.0);
        self.needs_resolve = false;

        let rgba = paint_rgba(doc, viewport.width, viewport.height, 1.0)?;
        self.timings.paint_ns = t0.elapsed().as_nanos() as u64;

        // Frame tick for pending timeout backstop.
        self.frame_count = self.frame_count.saturating_add(1);
        self.pending.tick_frames(self.config.pending_timeout_frames);

        // Holes: Layer 1 crate unit does not composite host textures (UI owns that).
        let _ = viewport.holes;

        Ok(PaintOutput {
            rgba: Some(rgba),
            texture: None,
            focus: self.focus.clone(),
        })
    }

    pub fn last_timings(&self) -> Timings {
        self.timings.clone()
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Test helper: serialize current document HTML (root element outerHTML).
    pub fn serialize_html(&self) -> Result<String, EngineError> {
        let doc = self.doc.as_ref().ok_or(EngineError::NotLoaded)?;
        let root = doc.root_element();
        Ok(root.outer_html())
    }

    /// Test helper: read an attribute from an element by id.
    pub fn attr(&self, id: &str, name: &str) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let nid = doc.get_element_by_id(id)?;
        let node = doc.get_node(nid)?;
        node.attrs().and_then(|attrs| {
            attrs
                .iter()
                .find(|a| &*a.name.local == name)
                .map(|a| a.value.to_string())
        })
    }

    /// Test helper: whether element has class token.
    pub fn has_class(&self, id: &str, class: &str) -> bool {
        self.attr(id, "class")
            .map(|c| class_tokens(&c).any(|t| t == class))
            .unwrap_or(false)
    }

    /// Test helper: pending timeout failures (A3c).
    pub fn pending_timeout_failures(&self) -> &[crate::pending::PendingTimeout] {
        &self.pending.timeout_failures
    }

    /// Test helper: force a frame tick without painting (for timeout tests).
    pub fn test_tick_frame(&mut self) {
        self.frame_count = self.frame_count.saturating_add(1);
        self.pending.tick_frames(self.config.pending_timeout_frames);
    }

    /// Test helper: current focus.
    pub fn focus_state(&self) -> &FocusState {
        &self.focus
    }

    /// Test helper: set focus by element id (for keyboard tests without Tab walking).
    pub fn test_set_focus(&mut self, id: &str) {
        if let Some(doc) = self.doc.as_mut() {
            if let Some(nid) = doc.get_element_by_id(id) {
                doc.set_focus_to(nid);
                self.focus.focused_id = Some(id.to_string());
            }
        }
    }

    /// Test helper: subscribed slice names the engine will ask `version` for.
    pub fn subscribed_slices(&self) -> HashSet<String> {
        let mut s = HashSet::new();
        s.insert("actions".into());
        for subs in self.sections.values() {
            for n in subs {
                s.insert(n.clone());
            }
        }
        s
    }

    /// Test helper: section ids.
    pub fn section_ids(&self) -> Vec<String> {
        self.sections.keys().cloned().collect()
    }
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
