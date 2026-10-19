//! Attribute dialect: swap / sync modes and element attribute index.

use crate::error::EngineError;
use crate::trigger::{parse_triggers, ParsedTrigger};

/// Locked swap strategies. Missing `hx-swap` defaults to `InnerHtml` (htmx parity).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwapMode {
    InnerHtml,
    OuterHtml,
}

impl SwapMode {
    pub fn parse(raw: Option<&str>, source_id: &str) -> Result<Self, EngineError> {
        match raw {
            None | Some("") | Some("innerHTML") => Ok(SwapMode::InnerHtml),
            Some("outerHTML") => Ok(SwapMode::OuterHtml),
            Some(other) => Err(EngineError::UnsupportedSwap {
                value: other.to_string(),
                source_id: source_id.to_string(),
            }),
        }
    }
}

/// Locked `hx-sync` modes. Missing attribute defaults to `Drop`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SyncMode {
    #[default]
    Drop,
    Queue,
}

impl SyncMode {
    pub fn parse(raw: Option<&str>, source_id: &str) -> Result<Self, EngineError> {
        match raw {
            None | Some("") | Some("drop") => Ok(SyncMode::Drop),
            Some("queue") => Ok(SyncMode::Queue),
            Some(other) => Err(EngineError::UnsupportedSync {
                value: other.to_string(),
                source_id: source_id.to_string(),
            }),
        }
    }
}

/// One interactive / section element discovered at `load_html`.
#[derive(Clone, Debug)]
pub struct ElementBinding {
    pub id: String,
    pub hx_post: Option<String>,
    pub hx_target: Option<String>,
    pub hx_swap: SwapMode,
    pub hx_indicator: Option<String>,
    pub hx_sync: SyncMode,
    pub triggers: Vec<ParsedTrigger>,
    /// Slice names this element subscribes to (from `slice:` triggers).
    pub slice_subs: Vec<String>,
}

impl ElementBinding {
    pub fn from_attrs(id: &str, attrs: &[(String, String)]) -> Result<Option<Self>, EngineError> {
        let get = |name: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };

        let hx_post = get("hx-post").map(str::to_string);
        let hx_trigger_raw = get("hx-trigger");
        let hx_target = get("hx-target").map(str::to_string);
        let hx_indicator = get("hx-indicator").map(|s| s.trim_start_matches('#').to_string());

        // Only index elements that participate in the dialect.
        if hx_post.is_none() && hx_trigger_raw.is_none() && hx_target.is_none() {
            return Ok(None);
        }

        if id.is_empty() {
            return Err(EngineError::Internal(
                "interactive element missing stable id".into(),
            ));
        }

        let hx_swap = SwapMode::parse(get("hx-swap"), id)?;
        let hx_sync = SyncMode::parse(get("hx-sync"), id)?;

        let triggers = match hx_trigger_raw {
            Some(raw) => parse_triggers(raw, id)?,
            None => {
                // Default trigger for hx-post without hx-trigger is click (htmx-like).
                if hx_post.is_some() {
                    parse_triggers("click", id)?
                } else {
                    Vec::new()
                }
            }
        };

        let slice_subs: Vec<String> = triggers
            .iter()
            .filter_map(|t| match &t.event {
                crate::trigger::TriggerEvent::Slice(name) => Some(name.clone()),
                _ => None,
            })
            .collect();

        Ok(Some(ElementBinding {
            id: id.to_string(),
            hx_post,
            hx_target,
            hx_swap,
            hx_indicator,
            hx_sync,
            triggers,
            slice_subs,
        }))
    }

    pub fn is_section(&self) -> bool {
        !self.slice_subs.is_empty()
    }
}
