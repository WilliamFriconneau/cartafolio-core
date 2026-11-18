//! The tools of disputes: what two nodes changed differently, shown with every alternative, and the person's choice.

use std::collections::HashMap;

use super::*;
use crate::access::Kind;

/// What an assistant is told with a dispute: it shows the alternatives and never picks one.
const NEVER_PICK: &str = "Each fact here was changed differently on two nodes. Show the person every alternative; never pick one yourself. Resolving is the person's choice.";

#[allow(unused_variables)]
pub(super) fn disputes(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let mut out = Vec::new();
    for s in whole_spaces(caller, read_spaces(caller, args)?) {
        let conn = brain.open_space(&s)?;
        for d in crate::sync::open_disputes(&conn)? {
            let fact = facts::read_fact(&conn, &d.fact)?.map(|f| f.to_json());
            out.push(json!({
                "space": s.as_str(),
                "fact": fact,
                "since": util::iso(d.since),
                "alternatives": d.alternatives.iter().map(crate::sync::Alternative::to_json).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(json!({
        "notice": NOTICE,
        "message": NEVER_PICK,
        "count": out.len(),
        "disputes": out,
    }))
}

/// Who may resolve: the person, on their page (a console connection), or an assistant that says the person chose
/// (`confirmed_by_person`). A machine of the person (a peer) never does: resolving is an act of the person.
#[allow(unused_variables)]
pub(super) fn dispute_resolve(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    if caller.kind == Kind::Peer {
        return Err(ToolError::Denied(
            "a machine does not resolve a dispute: the person does".into(),
        ));
    }
    let confirmed = args.get("confirmed_by_person").and_then(Value::as_bool) == Some(true);
    if caller.kind != Kind::Console && !confirmed {
        return Err(ToolError::Denied(
            "resolving a dispute is the person's choice: show them the alternatives (`disputes`), and call again with confirmed_by_person: true once they picked one".into(),
        ));
    }
    let id = str_arg(args, "id")?;
    let keep = str_arg(args, "keep")?;
    let spaces = match args.get("space").and_then(Value::as_str) {
        Some(_) => vec![write_space(brain, caller, args)?],
        None => caller.writable(),
    };
    let spaces: Vec<Space> = spaces
        .into_iter()
        .filter(|s| !caller.paths.contains_key(s))
        .collect();
    let Some((space, conn, _)) = locate(brain, &spaces, id)? else {
        if locate(brain, &caller.readable(), id)?.is_some() {
            return Err(ToolError::Denied(
                "no write right on the space of this fact".into(),
            ));
        }
        return Err(ToolError::Invalid("unknown fact".into()));
    };
    // The id of an alternative names its dispute as well as the fact in dispute does.
    let fact = crate::sync::dispute_marks(&conn)?
        .remove(id)
        .map_or_else(|| id.to_string(), |(f, _)| f);
    let done = crate::sync::resolve(&conn, &fact, keep, &caller.agent)?;
    Ok(json!({
        "result": "resolved",
        "space": space.as_str(),
        "fact": done.fact,
        "kept": done.kept,
        "retired": done.retired,
        "message": "Resolved here; every node gets the same at its next exchange. Nothing was erased: what lost is retired, in the history.",
    }))
}

/// Flags the facts that take part in a dispute (`disputed`, `dispute_of`, `alternatives`), so an assistant that reads
/// them sees it. Each fact is a [`facts::Fact::to_json`] value; the others are left as they are.
pub(super) fn flag(brain: &Brain, facts: &mut [Value]) -> Result<(), ToolError> {
    let mut marks: HashMap<String, HashMap<String, (String, Vec<String>)>> = HashMap::new();
    for f in facts.iter_mut() {
        let (Some(space), Some(id)) = (f["space"].as_str(), f["id"].as_str()) else {
            continue;
        };
        if !marks.contains_key(space) {
            let m = match Space::new(space) {
                Ok(s) => crate::sync::dispute_marks(&brain.open_space(&s)?)?,
                Err(_) => HashMap::new(),
            };
            marks.insert(space.to_string(), m);
        }
        if let Some((of, alts)) = marks.get(space).and_then(|m| m.get(id)).cloned() {
            f["disputed"] = json!(true);
            f["dispute_of"] = json!(of);
            f["alternatives"] = json!(alts);
        }
    }
    Ok(())
}
