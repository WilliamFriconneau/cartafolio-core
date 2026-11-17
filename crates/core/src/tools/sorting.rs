//! The tools that sort: the reservation of notes of « À trier », the filing rules of a space and its vocabularies.

use super::*;

#[allow(unused_variables)]
pub(super) fn triage_claim(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 50) as usize;
    let minutes = args
        .get("minutes")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 60);
    let now = crate::util::now();
    let mut leases = LEASES.lock().unwrap_or_else(|e| e.into_inner());
    leases.retain(|_, (_, until)| *until > now);
    let mut claimed = Vec::new();
    for s in caller.writable() {
        if !crate::triage::is_triage(brain, &s) {
            continue;
        }
        let waiting = crate::triage::waiting(brain, &s, 500)?;
        for n in waiting["notes"].as_array().into_iter().flatten() {
            let Some(id) = n["id"].as_str() else { continue };
            let mine = leases.get(id).is_some_and(|(who, _)| *who == caller.agent);
            if (leases.contains_key(id) && !mine) || claimed.len() >= limit {
                continue;
            }
            leases.insert(
                id.to_string(),
                (caller.agent.clone(), now + 60 * minutes as i64),
            );
            let mut note = n.clone();
            note["space"] = json!(s.as_str());
            claimed.push(note);
        }
    }
    Ok(json!({ "claimed": claimed, "lease_minutes": minutes }))
}

#[allow(unused_variables)]
pub(super) fn triage_release(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let ids: Vec<String> = args
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut leases = LEASES.lock().unwrap_or_else(|e| e.into_inner());
    let before = leases.len();
    leases.retain(|id, (who, _)| !(*who == caller.agent && (ids.is_empty() || ids.contains(id))));
    Ok(json!({ "released": before - leases.len() }))
}

#[allow(unused_variables)]
pub(super) fn vocabulary_terms(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let space_name = args
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Invalid("space: the name of a space".into()))?;
    let space = Space::new(space_name)?;
    if !caller.readable().contains(&space) || caller.read_paths(&space).is_some() {
        return Err(ToolError::Invalid(format!("no access to {space_name}")));
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .clamp(1, 50_000) as usize;
    let conn = brain.open_space(&space)?;
    let vocabularies: Vec<Value> = crate::vocabulary::list(&conn)?
                .into_iter()
                .map(|v| json!({ "name": v.name, "source": v.source, "terms": v.terms, "imported_at": crate::util::iso(v.imported_at) }))
                .collect();
    Ok(
        json!({ "space": space_name, "vocabularies": vocabularies, "terms": crate::vocabulary::terms(&conn, limit)? }),
    )
}

#[allow(unused_variables)]
pub(super) fn rules(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    let writing = name == "rules_set";
    let space_name = args
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Invalid("space: the name of a space".into()))?;
    let space = Space::new(space_name)?;
    let allowed = if writing {
        caller.can_write(&space)
    } else {
        caller.readable().contains(&space)
    };
    if !allowed || caller.read_paths(&space).is_some() {
        return Err(ToolError::Invalid(format!("no access to {space_name}")));
    }
    let conn = brain.open_space(&space)?;
    let found: Option<String> = conn
                .query_row(
                    "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
                     (SELECT fact_id FROM fact_tags WHERE tag = 'rules') ORDER BY f.created_at DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| ToolError::Invalid(e.to_string()))?;
    let current = found
        .as_deref()
        .and_then(|id| facts::read_fact(&conn, id).ok().flatten())
        .and_then(|f| serde_json::from_str::<Value>(f.text.trim()).ok())
        .filter(|d| d.get("version").and_then(Value::as_i64) == Some(1))
        .unwrap_or_else(|| json!({ "version": 1, "tags": [], "keywords": [] }));
    if !writing {
        return Ok(json!({ "space": space_name, "id": found, "rules": current }));
    }
    let clean = |key: &str, tags: bool| -> Vec<String> {
        let src = args.get(key).unwrap_or(&current[key]);
        let mut v: Vec<String> = src
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|t| {
                if tags {
                    facts::normalize_tag(t).ok()
                } else {
                    let k = t.trim().trim_start_matches('#').to_lowercase();
                    (!k.is_empty() && k.chars().count() <= 40).then_some(k)
                }
            })
            .collect();
        v.sort();
        v.dedup();
        v.truncate(100);
        v
    };
    let doc =
        json!({ "version": 1, "tags": clean("tags", true), "keywords": clean("keywords", false) })
            .to_string();
    let mut new = NewFact::agent(&doc, &caller.agent);
    new.tags = vec!["rules".into()];
    let unchanged =
        found.is_some() && serde_json::from_str::<Value>(&doc).ok() == Some(current.clone());
    match found {
        // The same rules again: nothing to write (a supersede with an identical text is refused).
        Some(_) if unchanged => {}
        Some(old) => {
            facts::supersede(
                &conn,
                &space,
                &caller.agent,
                &old,
                new,
                Some("rules changed"),
            )?;
        }
        None => {
            if let facts::Remembered::NeedsDecision(c) =
                facts::remember(&conn, &space, &caller.agent, new, &[])?
            {
                let ids: Vec<String> = c.into_iter().map(|c| c.id).collect();
                let mut again = NewFact::agent(&doc, &caller.agent);
                again.tags = vec!["rules".into()];
                facts::remember(&conn, &space, &caller.agent, again, &ids)?;
            }
        }
    }
    Ok(
        json!({ "space": space_name, "rules": serde_json::from_str::<Value>(&doc).unwrap_or(Value::Null) }),
    )
}

#[allow(unused_variables)]
pub(super) fn triage_waiting(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    let mut out = Vec::new();
    for s in caller.readable() {
        if crate::triage::is_triage(brain, &s) && caller.read_paths(&s).is_none() {
            out.push(crate::triage::waiting(brain, &s, limit)?);
        }
    }
    Ok(json!({ "triage": out, "notice": NOTICE }))
}

#[allow(unused_variables)]
pub(super) fn triage_send(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = str_arg(args, "id")?;
    let to = Space::new(str_arg(args, "to")?)?;
    if !caller.can_write(&to) {
        return Err(ToolError::Invalid(format!("no access to {to}")));
    }
    for from in caller.writable() {
        if !crate::triage::is_triage(brain, &from) {
            continue;
        }
        let holds = crate::files::info(brain, &from, id)?.is_some_and(|f| f.status == "active")
            || facts::read_fact(&brain.open_space(&from)?, id)?
                .is_some_and(|f| f.status == "active");
        if holds {
            let new = crate::triage::send(brain, &from, id, &to, &caller.agent, &caller.user_id)?;
            return Ok(json!({ "result": "sent", "id": new, "to": to.as_str() }));
        }
    }
    Err(ToolError::Invalid("nothing waits under this id".into()))
}

#[allow(unused_variables)]
pub(super) fn transcription_failed(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = str_arg(args, "id")?;
    let failed = args.get("failed").and_then(Value::as_bool).unwrap_or(true);
    let Some(space) = locate_file(brain, &caller.writable(), id)? else {
        return Err(ToolError::Invalid("unknown file".into()));
    };
    let f = crate::files::mark_transcription_failed(brain, &space, id, failed)?;
    Ok(json!({ "result": if failed { "marked" } else { "cleared" }, "file": f.to_json() }))
}
