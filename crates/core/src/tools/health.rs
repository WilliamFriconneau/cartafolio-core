//! The health referential for apps: a compact read of the signs, measures, vital constants and red flags the node knows
//! (see `memcp-health`, docs/HEALTH.md). It reads no space and writes nothing.

use memcp_health::Lang;

use super::*;

pub(super) fn health_referential(
    _brain: &Brain,
    _caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let kind = args.get("kind").and_then(Value::as_str);
    if let Some(k) = kind
        && !["manifestations", "measures", "vitals", "red_flags"].contains(&k)
    {
        return Err(ToolError::Invalid(
            "kind: manifestations, measures, vitals or red_flags".into(),
        ));
    }
    let lang = match args.get("lang").and_then(Value::as_str) {
        None => Lang::Auto,
        Some(l) => {
            Lang::parse(l).ok_or_else(|| ToolError::Invalid("lang: auto, fr, en or es".into()))?
        }
    };
    let query = args.get("query").and_then(Value::as_str);
    let mut out = memcp_health::builtin().compact(kind, query, lang);
    out["notice"] = json!(
        "A suggestion for the person to confirm, never a diagnosis. The French vocabulary is complete; English is partial; there is no Spanish vocabulary."
    );
    Ok(out)
}
