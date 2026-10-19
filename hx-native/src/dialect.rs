//! Attribute dialect: swap / sync modes and element attribute index.
//!
//! Mainnet HTML contract §2 (locked 2026-10-19): only the supported `hx-*`
//! attributes exist. Anything else is a load error, not ignored.

use std::collections::HashSet;

use crate::error::EngineError;
use crate::trigger::{parse_triggers, ParsedTrigger};

/// Locked dialect attributes (exact names). `data-hx-*` aliases are not accepted.
pub const SUPPORTED_HX_ATTRIBUTES: &[&str] = &[
    "hx-post",
    "hx-target",
    "hx-swap",
    "hx-trigger",
    "hx-sync",
    "hx-indicator",
];

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

fn attr_get<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Parse a locked `#id` selector. Rejects anything that is not `#` + non-empty id.
pub fn parse_id_selector(elem_id: &str, attr: &str, raw: &str) -> Result<String, EngineError> {
    let raw = raw.trim();
    let Some(id) = raw.strip_prefix('#') else {
        return Err(EngineError::InvalidAttribute {
            id: elem_id.to_string(),
            attr: attr.to_string(),
            reason: format!("must be an id selector `#id`, got `{raw}`"),
        });
    };
    if id.is_empty()
        || id
            .chars()
            .any(|c| c.is_whitespace() || c == '#' || c == '.')
    {
        return Err(EngineError::InvalidAttribute {
            id: elem_id.to_string(),
            attr: attr.to_string(),
            reason: format!("must be an id selector `#id`, got `{raw}`"),
        });
    }
    Ok(id.to_string())
}

/// Strict dialect checks for one element's attributes (HTML contract §2).
///
/// Call this on HTML being **loaded** (`load_html` or a swap fragment), not on
/// the live DOM after the engine writes `data-hx-reason`.
pub fn validate_loaded_attrs(
    elem_id: &str,
    attrs: &[(String, String)],
    known_ids: &HashSet<String>,
) -> Result<(), EngineError> {
    for (name, _) in attrs {
        if name == "data-hx-reason" {
            return Err(EngineError::InvalidAttribute {
                id: elem_id.to_string(),
                attr: name.clone(),
                reason: "engine-written only; must not appear in loaded HTML".into(),
            });
        }
        if (name.starts_with("hx-") || name.starts_with("data-hx-"))
            && !SUPPORTED_HX_ATTRIBUTES.contains(&name.as_str())
        {
            return Err(EngineError::InvalidAttribute {
                id: elem_id.to_string(),
                attr: name.clone(),
                reason: format!(
                    "not in the locked dialect (only {})",
                    SUPPORTED_HX_ATTRIBUTES.join(", ")
                ),
            });
        }
    }

    // Value checks at load (swap/sync use existing variants).
    if attrs.iter().any(|(k, _)| k == "hx-swap") {
        SwapMode::parse(attr_get(attrs, "hx-swap"), elem_id)?;
    }
    if attrs.iter().any(|(k, _)| k == "hx-sync") {
        SyncMode::parse(attr_get(attrs, "hx-sync"), elem_id)?;
    }

    if let Some(raw) = attr_get(attrs, "hx-target") {
        let target_id = parse_id_selector(elem_id, "hx-target", raw)?;
        if !known_ids.contains(&target_id) {
            return Err(EngineError::InvalidAttribute {
                id: elem_id.to_string(),
                attr: "hx-target".into(),
                reason: format!("id `#{target_id}` does not resolve in the document or fragment"),
            });
        }
    }

    if let Some(raw) = attr_get(attrs, "hx-indicator") {
        let indicator_id = parse_id_selector(elem_id, "hx-indicator", raw)?;
        if !known_ids.contains(&indicator_id) {
            return Err(EngineError::InvalidAttribute {
                id: elem_id.to_string(),
                attr: "hx-indicator".into(),
                reason: format!(
                    "id `#{indicator_id}` does not resolve in the document or fragment"
                ),
            });
        }
    }

    Ok(())
}

impl ElementBinding {
    pub fn from_attrs(id: &str, attrs: &[(String, String)]) -> Result<Option<Self>, EngineError> {
        let hx_post = attr_get(attrs, "hx-post").map(str::to_string);
        let hx_trigger_raw = attr_get(attrs, "hx-trigger");
        let hx_target = attr_get(attrs, "hx-target").map(str::to_string);
        let hx_indicator = attr_get(attrs, "hx-indicator")
            .map(|s| parse_id_selector(id, "hx-indicator", s))
            .transpose()?;

        // Only index elements that participate in the dialect.
        if hx_post.is_none() && hx_trigger_raw.is_none() && hx_target.is_none() {
            return Ok(None);
        }

        if id.is_empty() {
            return Err(EngineError::Internal(
                "interactive element missing stable id".into(),
            ));
        }

        // Normalize hx-target to require #id when present (binding path).
        if let Some(ref t) = hx_target {
            let _ = parse_id_selector(id, "hx-target", t)?;
        }

        let hx_swap = SwapMode::parse(attr_get(attrs, "hx-swap"), id)?;
        let hx_sync = SyncMode::parse(attr_get(attrs, "hx-sync"), id)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_hx_attr() {
        let ids = HashSet::new();
        let attrs = vec![("hx-get".into(), "/x".into())];
        let err = validate_loaded_attrs("e", &attrs, &ids).unwrap_err();
        assert!(matches!(err, EngineError::InvalidAttribute { attr, .. } if attr == "hx-get"));
    }

    #[test]
    fn rejects_data_hx_reason() {
        let ids = HashSet::new();
        let attrs = vec![("data-hx-reason".into(), "nope".into())];
        let err = validate_loaded_attrs("e", &attrs, &ids).unwrap_err();
        assert!(matches!(
            err,
            EngineError::InvalidAttribute { attr, .. } if attr == "data-hx-reason"
        ));
    }

    #[test]
    fn accepts_locked_attrs_with_resolving_ids() {
        let mut ids = HashSet::new();
        ids.insert("box".into());
        ids.insert("spin".into());
        let attrs = vec![
            ("hx-post".into(), "a".into()),
            ("hx-target".into(), "#box".into()),
            ("hx-indicator".into(), "#spin".into()),
            ("hx-swap".into(), "innerHTML".into()),
            ("hx-sync".into(), "drop".into()),
            ("hx-trigger".into(), "click".into()),
        ];
        validate_loaded_attrs("btn", &attrs, &ids).unwrap();
    }
}
