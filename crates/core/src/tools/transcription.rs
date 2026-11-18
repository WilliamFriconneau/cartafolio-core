//! The tool that tells a speech model what to expect (`transcription_hints`): see [`crate::transcription`].

use super::*;

pub(super) fn transcription_hints(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let named = args.get("space").and_then(Value::as_str);
    // A space reached only by paths is not read whole: its notes are not mixed into a list.
    let spaces: Vec<Space> = super::read_spaces(caller, args)?
        .into_iter()
        .filter(|s| caller.read_paths(s).is_none())
        .collect();
    if let Some(name) = named
        && spaces.is_empty()
    {
        return Err(ToolError::Denied(format!("no read right on {name}")));
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(crate::transcription::DEFAULT_LIMIT, |n| n as usize);
    let lang = args.get("lang").and_then(Value::as_str);
    let hints = crate::transcription::hints(brain, &caller.user_id, &spaces, limit, lang)?;
    let mut out = hints.to_json();
    out["space"] = json!(named);
    out["lang"] = json!(lang);
    Ok(out)
}
