//! The tools that tell what there is: the spaces a connection reads, and the machines holding the memory.

use super::*;

#[allow(unused_variables)]
pub(super) fn peers(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let ctl = brain.open_control()?;
    let list: Vec<Value> = crate::access::list(&ctl)?
        .into_iter()
        .filter(|c| c.user_id == caller.user_id && c.agent.starts_with("peer-"))
        .map(|c| {
            json!({
                "name": c.agent,
                "created_at": crate::util::iso(c.created_at),
                "last_used_at": c.last_used_at.map(crate::util::iso),
                "allowed": c.revoked_at.is_none(),
            })
        })
        .collect();
    let window = crate::sync::window_status(brain)?.to_json();
    Ok(json!({ "peers": list, "window": window }))
}

#[allow(unused_variables)]
pub(super) fn spaces(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let ctl = brain.open_control()?;
    let mut out = Vec::new();
    for s in caller.readable() {
        let row: Option<(Option<String>, Option<String>, i64, String)> = ctl
            .query_row(
                "SELECT label, parent, position, kind FROM spaces WHERE name = ?1",
                [s.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| ToolError::Invalid(e.to_string()))?;
        let Some((label, parent, position, kind)) = row else {
            continue;
        };
        let conn = brain.open_space(&s)?;
        let facts = facts::counts(&conn)?["active"].as_u64().unwrap_or(0);
        out.push(json!({
            "name": s.as_str(),
            "label": label.unwrap_or_else(|| s.as_str().into()),
            "parent": parent,
            "position": position,
            "kind": kind,
            "writable": caller.can_write(&s),
            "facts": facts,
        }));
    }
    // A parent this connection cannot read is not shown: that space stands at the top.
    let names: Vec<String> = out
        .iter()
        .filter_map(|v| v["name"].as_str().map(String::from))
        .collect();
    for v in out.iter_mut() {
        let hidden = v["parent"]
            .as_str()
            .is_some_and(|p| !names.iter().any(|n| n == p));
        if hidden {
            v["parent"] = Value::Null;
        }
    }
    out.sort_by_key(|v| {
        (
            v["position"].as_i64().unwrap_or(0),
            v["label"].as_str().unwrap_or("").to_lowercase(),
        )
    });
    Ok(json!({ "spaces": out }))
}
