//! `hx-trigger` grammar: events, filter whitelist, `from:body` / `from:document`.

use crate::error::EngineError;
use crate::types::{KeyEvent, Modifiers};

/// DOM event names accepted by the dialect.
///
/// `#[non_exhaustive]` so adding events (e.g. `Change` in #14) does not break
/// exhaustive matches downstream.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerEvent {
    Click,
    ContextMenu,
    AuxClick,
    KeyDown,
    KeyUp,
    /// Commit-only form control event (range release, checkbox/radio toggle,
    /// select pick, arrow step). Takes no filters.
    Change,
    Slice(String),
}

impl TriggerEvent {
    pub fn as_static_name(&self) -> &'static str {
        match self {
            TriggerEvent::Click => "click",
            TriggerEvent::ContextMenu => "contextmenu",
            TriggerEvent::AuxClick => "auxclick",
            TriggerEvent::KeyDown => "keydown",
            TriggerEvent::KeyUp => "keyup",
            TriggerEvent::Change => "change",
            TriggerEvent::Slice(_) => "slice",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FromTarget {
    /// `from:body` and `from:document` are equivalent (window-level).
    Body,
}

/// Filter expression AST. `&&` binds tighter than `||`. No parentheses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterExpr {
    /// `shiftKey` / `ctrlKey` / `altKey` / `metaKey` / `repeat`
    Flag {
        name: FlagName,
        negated: bool,
    },
    /// `key=='X'` (single-quoted literal), optionally negated with `!`
    KeyEq {
        value: String,
        negated: bool,
    },
    And(Box<FilterExpr>, Box<FilterExpr>),
    Or(Box<FilterExpr>, Box<FilterExpr>),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlagName {
    ShiftKey,
    CtrlKey,
    AltKey,
    MetaKey,
    Repeat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTrigger {
    pub event: TriggerEvent,
    pub filter: Option<FilterExpr>,
    pub from: Option<FromTarget>,
}

/// Parse a full `hx-trigger` attribute (comma-separated triggers).
pub fn parse_triggers(raw: &str, source_id: &str) -> Result<Vec<ParsedTrigger>, EngineError> {
    let parts = split_top_level_commas(raw);
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(parse_one_trigger(part, source_id)?);
    }
    if out.is_empty() {
        return Err(invalid(source_id, "empty hx-trigger"));
    }
    Ok(out)
}

fn invalid(source_id: &str, detail: impl Into<String>) -> EngineError {
    EngineError::InvalidTrigger {
        source_id: source_id.to_string(),
        detail: detail.into(),
    }
}

fn split_top_level_commas(raw: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let bytes = raw.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&raw[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&raw[start..]);
    parts
}

fn parse_one_trigger(raw: &str, source_id: &str) -> Result<ParsedTrigger, EngineError> {
    // Grammar: EVENT [ FILTER ] [ from:TARGET ]
    let mut rest = raw.trim();

    // Optional trailing `from:...`
    let mut from = None;
    if let Some(idx) = find_from_keyword(rest) {
        let (before, after) = rest.split_at(idx);
        let after = after.trim();
        let target = after
            .strip_prefix("from:")
            .ok_or_else(|| invalid(source_id, format!("bad from: in `{raw}`")))?
            .trim();
        from = Some(match target {
            "body" | "document" => FromTarget::Body,
            other => {
                return Err(invalid(
                    source_id,
                    format!("unsupported from:{other} (only from:body / from:document)"),
                ));
            }
        });
        rest = before.trim();
    }

    // Optional `[filter]`
    let (event_name, filter) = if let Some(open) = rest.find('[') {
        if !rest.ends_with(']') {
            return Err(invalid(source_id, format!("unclosed filter in `{raw}`")));
        }
        if rest.contains('(') || rest.contains(')') {
            return Err(invalid(source_id, "parentheses are not allowed in filters"));
        }
        let event_name = rest[..open].trim();
        let filter_src = &rest[open + 1..rest.len() - 1];
        let filter = parse_filter(filter_src, source_id)?;
        (event_name, Some(filter))
    } else {
        (rest.trim(), None)
    };

    if event_name.is_empty() {
        return Err(invalid(source_id, "missing event name"));
    }

    let event = parse_event_name(event_name, source_id)?;

    if from.is_some() {
        match event {
            TriggerEvent::KeyDown | TriggerEvent::KeyUp => {}
            _ => {
                return Err(invalid(
                    source_id,
                    "from:body / from:document only allowed on keydown/keyup",
                ));
            }
        }
    }

    // `change` is commit-only and takes no filters (issue #14).
    if event == TriggerEvent::Change {
        if filter.is_some() {
            return Err(invalid(source_id, "change takes no filters"));
        }
        if from.is_some() {
            return Err(invalid(source_id, "change does not allow from:"));
        }
    }

    Ok(ParsedTrigger {
        event,
        filter,
        from,
    })
}

fn find_from_keyword(s: &str) -> Option<usize> {
    // Find ` from:` at top level (not inside [...]).
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'f' if depth == 0 => {
                if s[i..].starts_with("from:") {
                    // Require start or whitespace before from:
                    if i == 0 || bytes[i - 1].is_ascii_whitespace() {
                        return Some(i);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_event_name(name: &str, source_id: &str) -> Result<TriggerEvent, EngineError> {
    if let Some(slice) = name.strip_prefix("slice:") {
        if slice.is_empty() {
            return Err(invalid(source_id, "empty slice name"));
        }
        return Ok(TriggerEvent::Slice(slice.to_string()));
    }
    match name {
        "click" => Ok(TriggerEvent::Click),
        "contextmenu" => Ok(TriggerEvent::ContextMenu),
        "auxclick" => Ok(TriggerEvent::AuxClick),
        "keydown" => Ok(TriggerEvent::KeyDown),
        "keyup" => Ok(TriggerEvent::KeyUp),
        "change" => Ok(TriggerEvent::Change),
        "dblclick" => Err(invalid(
            source_id,
            "dblclick is deferred (load error until added)",
        )),
        other => Err(invalid(source_id, format!("unknown event `{other}`"))),
    }
}

fn parse_filter(raw: &str, source_id: &str) -> Result<FilterExpr, EngineError> {
    parse_or(raw.trim(), source_id)
}

fn parse_or(raw: &str, source_id: &str) -> Result<FilterExpr, EngineError> {
    let parts = split_op(raw, "||");
    let mut exprs = Vec::new();
    for p in parts {
        exprs.push(parse_and(p.trim(), source_id)?);
    }
    let mut it = exprs.into_iter();
    let mut acc = it
        .next()
        .ok_or_else(|| invalid(source_id, "empty filter"))?;
    for e in it {
        acc = FilterExpr::Or(Box::new(acc), Box::new(e));
    }
    Ok(acc)
}

fn parse_and(raw: &str, source_id: &str) -> Result<FilterExpr, EngineError> {
    let parts = split_op(raw, "&&");
    let mut exprs = Vec::new();
    for p in parts {
        exprs.push(parse_atom(p.trim(), source_id)?);
    }
    let mut it = exprs.into_iter();
    let mut acc = it
        .next()
        .ok_or_else(|| invalid(source_id, "empty filter atom"))?;
    for e in it {
        acc = FilterExpr::And(Box::new(acc), Box::new(e));
    }
    Ok(acc)
}

fn split_op<'a>(raw: &'a str, op: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let bytes = raw.as_bytes();
    let op_b = op.as_bytes();
    while i + op_b.len() <= bytes.len() {
        if &bytes[i..i + op_b.len()] == op_b {
            parts.push(&raw[start..i]);
            i += op_b.len();
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&raw[start..]);
    parts
}

fn parse_atom(raw: &str, source_id: &str) -> Result<FilterExpr, EngineError> {
    let mut negated = false;
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix('!') {
        negated = true;
        s = rest.trim();
    }
    if s.is_empty() {
        return Err(invalid(source_id, "empty filter atom"));
    }
    // key=='X'
    if let Some(rest) = s.strip_prefix("key==") {
        let rest = rest.trim();
        if rest.len() >= 2 && rest.starts_with('\'') && rest.ends_with('\'') {
            let value = rest[1..rest.len() - 1].to_string();
            return Ok(FilterExpr::KeyEq { value, negated });
        }
        return Err(invalid(
            source_id,
            format!("key filter must be key=='X' (single-quoted), got `{s}`"),
        ));
    }
    // Reject things like shiftKey==1, event.foo
    if s.contains("==") || s.contains('.') {
        return Err(invalid(
            source_id,
            format!("unknown or illegal filter `{s}`"),
        ));
    }
    let name = match s {
        "shiftKey" => FlagName::ShiftKey,
        "ctrlKey" => FlagName::CtrlKey,
        "altKey" => FlagName::AltKey,
        "metaKey" => FlagName::MetaKey,
        "repeat" => FlagName::Repeat,
        other => {
            return Err(invalid(source_id, format!("unknown filter `{other}`")));
        }
    };
    Ok(FilterExpr::Flag { name, negated })
}

/// Evaluate a filter against pointer/key context.
pub struct FilterCtx<'a> {
    pub mods: Modifiers,
    pub repeat: bool,
    pub key: Option<&'a str>,
}

impl FilterExpr {
    pub fn eval(&self, ctx: &FilterCtx<'_>) -> bool {
        match self {
            FilterExpr::Flag { name, negated } => {
                let v = match name {
                    FlagName::ShiftKey => ctx.mods.shift,
                    FlagName::CtrlKey => ctx.mods.ctrl,
                    FlagName::AltKey => ctx.mods.alt,
                    FlagName::MetaKey => ctx.mods.meta,
                    FlagName::Repeat => ctx.repeat,
                };
                if *negated {
                    !v
                } else {
                    v
                }
            }
            FilterExpr::KeyEq { value, negated } => {
                let hit = ctx.key == Some(value.as_str());
                if *negated {
                    !hit
                } else {
                    hit
                }
            }
            FilterExpr::And(a, b) => a.eval(ctx) && b.eval(ctx),
            FilterExpr::Or(a, b) => a.eval(ctx) || b.eval(ctx),
        }
    }
}

/// Default keydown rule: do not fire on OS auto-repeat unless filter opts in.
pub fn keydown_repeat_allowed(filter: &Option<FilterExpr>, repeat: bool) -> bool {
    if !repeat {
        return true;
    }
    // Only fire on repeat if the filter can be true when repeat is true.
    // Spec: `[repeat]` fires on repeats only; `[repeat||!repeat]` for both.
    // Default (no filter): do NOT fire on repeat.
    match filter {
        None => false,
        Some(f) => {
            f.eval(&FilterCtx {
                mods: Modifiers::default(),
                repeat: true,
                key: None,
            }) || {
                // If filter mentions repeat positively in some branch, allow evaluation
                // with the real ctx later — here we only gate the default.
                filter_mentions_repeat(f)
            }
        }
    }
}

fn filter_mentions_repeat(f: &FilterExpr) -> bool {
    match f {
        FilterExpr::Flag {
            name: FlagName::Repeat,
            ..
        } => true,
        FilterExpr::And(a, b) | FilterExpr::Or(a, b) => {
            filter_mentions_repeat(a) || filter_mentions_repeat(b)
        }
        _ => false,
    }
}

pub fn filter_ctx_from_key(ev: &KeyEvent) -> FilterCtx<'_> {
    FilterCtx {
        mods: ev.mods,
        repeat: ev.repeat,
        key: Some(ev.key.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_click_shift() {
        let t = parse_triggers("click[shiftKey]", "x").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].event, TriggerEvent::Click);
    }

    #[test]
    fn rejects_dblclick() {
        let err = parse_triggers("dblclick", "x").unwrap_err();
        assert!(matches!(err, EngineError::InvalidTrigger { .. }));
    }

    #[test]
    fn from_body_only_on_key() {
        let err = parse_triggers("click from:body", "x").unwrap_err();
        assert!(matches!(err, EngineError::InvalidTrigger { .. }));
        let ok = parse_triggers("keydown[key=='Enter'] from:body", "x").unwrap();
        assert_eq!(ok[0].from, Some(FromTarget::Body));
    }
}
