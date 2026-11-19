//! The living boards: `living_get` and `living_set`.

use super::*;

#[allow(unused_variables)]
pub(super) fn living(
    brain: &Brain,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let author = caller.agent.as_str();
    // A living page is a board of cards, each a one-line note tagged `living/<page>/<column>`, so it is read
    // straight from the space, synchronised like any note, and every card keeps its own history. Moving a card
    // supersedes it into its new column. The page's last pass (when, by whom, how far it read) is a small note
    // `living-meta/<page>`; its delta is the notes filed since. Cards are added, moved or dropped one by one:
    // two writers never overwrite each other.
    let writing = name == "living_set";
    let space_name = str_arg(args, "space")?.to_string();
    let page = str_arg(args, "page")?.to_string();
    let columns: &[&str] = match page.as_str() {
        "next" => &["now", "next", "later", "done"],
        "ideas" => &["new", "kept", "done"],
        "targets" => &["open", "reached"],
        "index" => &["current"],
        // A named list of the space (`list-shopping`, `list-books`…): open, then done.
        p if p.strip_prefix("list-").is_some_and(|n| {
            !n.is_empty()
                && n.len() <= 40
                && n.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }) =>
        {
            &["open", "done"]
        }
        _ => {
            return Err(ToolError::Invalid(
                "page: next, ideas, targets, index, or a list `list-<name>`".into(),
            ));
        }
    };
    let space = Space::new(&space_name)?;
    let allowed = if writing {
        caller.can_write(&space)
    } else {
        caller.readable().contains(&space)
    };
    if !allowed || caller.read_paths(&space).is_some() {
        return Err(ToolError::Invalid(format!("no access to {space_name}")));
    }
    let health_targets =
        page == "targets" && brain.space_kind(&space).is_ok_and(|kind| kind == "health");
    let sign_id = |name: &str| -> Option<String> {
        let slug = sign_slug(name);
        if !health_targets {
            return Some(slug);
        }
        let slug = memcp_health::text::fold(&slug);
        memcp_health::builtin()
            .manifestations()
            .iter()
            .find(|m| {
                slug == sign_slug(&m.id)
                    || slug == sign_slug(&m.tag)
                    || m.labels
                        .values()
                        .any(|label| slug == sign_slug(&memcp_health::text::fold(label)))
            })
            .map(|m| m.id.clone())
    };
    let sign_tags = |signs: &[String]| {
        signs
            .iter()
            .filter_map(|s| sign_id(s))
            .filter_map(|id| facts::normalize_tag(&format!("sign/{id}")).ok())
            .collect::<Vec<_>>()
    };
    let meta_tag = format!("living-meta/{page}");
    let conn = brain.open_space(&space)?;
    let meta: Option<(String, Value)> = conn
                .query_row(
                    "SELECT f.id, f.text FROM facts f WHERE f.status = 'active' AND f.id IN
                     (SELECT fact_id FROM fact_tags WHERE tag = ?1) ORDER BY f.created_at DESC LIMIT 1",
                    [&meta_tag],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|e| ToolError::Invalid(e.to_string()))?
                .map(|(id, t)| (id, serde_json::from_str::<Value>(&t).unwrap_or(Value::Null)));
    let since = meta
        .as_ref()
        .and_then(|(_, v)| v["covers"].as_i64())
        .unwrap_or(0);
    const OUTSIDE: &str = "SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide') OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%'";
    let card_column = |id: &str| -> Option<(String, String)> {
        let f = facts::read_fact(&conn, id).ok()??;
        if f.status != "active" {
            return None;
        }
        let col = f
            .tags
            .iter()
            .find_map(|t| t.strip_prefix(&format!("living/{page}/")).map(String::from))?;
        Some((f.text, col))
    };
    if !writing {
        let mut board = serde_json::Map::new();
        for col in columns {
            let mut st = conn
                        .prepare(
                            "SELECT f.id, f.text, f.created_at, f.author FROM facts f WHERE f.status = 'active' AND f.id IN
                             (SELECT fact_id FROM fact_tags WHERE tag = ?1) ORDER BY f.created_at ASC LIMIT 300",
                        )
                        .map_err(|e| ToolError::Invalid(e.to_string()))?;
            let mut cards: Vec<Value> = st
                        .query_map([format!("living/{page}/{col}")], |r| {
                            Ok(json!({ "id": r.get::<_, String>(0)?, "text": r.get::<_, String>(1)?,
                                       "created": crate::util::iso(r.get::<_, i64>(2)?), "author": r.get::<_, String>(3)? }))
                        })
                        .map_err(|e| ToolError::Invalid(e.to_string()))?
                        .flatten()
                        .collect();
            // A target's signs and symptoms: its `sign/<name>` tags.
            for card in &mut cards {
                let id = card["id"].as_str().unwrap_or("").to_string();
                let signs: Vec<String> = facts::read_fact(&conn, &id)
                    .ok()
                    .flatten()
                    .map(|f| {
                        f.tags
                            .iter()
                            .filter_map(|t| t.strip_prefix("sign/").and_then(sign_id))
                            .collect()
                    })
                    .unwrap_or_default();
                card["signs"] = json!(signs);
                let tags = facts::read_fact(&conn, &id)
                    .ok()
                    .flatten()
                    .map(|f| f.tags)
                    .unwrap_or_default();
                card["fields"] = crate::autotag::fields_json(&tags);
                card["tags"] = json!(tags);
            }
            board.insert((*col).to_string(), Value::Array(cards));
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_i64)
            .unwrap_or(50)
            .clamp(1, 200);
        // A board's first pass (no pass yet), or `order: "newest"`: the newest notes, so a memory that starts with
        // a large import is not read from its archive first. Otherwise the oldest after the pass, so nothing is
        // skipped. Either way the delta comes out oldest first.
        let newest = since == 0 || args.get("order").and_then(Value::as_str) == Some("newest");
        let order = if newest { "DESC" } else { "ASC" };
        let mut st = conn
                    .prepare(&format!(
                        "SELECT f.id, f.text, f.created_at FROM facts f WHERE f.status = 'active' AND f.created_at > ?1
                         AND f.id NOT IN ({OUTSIDE}) ORDER BY f.created_at {order}, f.id {order} LIMIT ?2"
                    ))
                    .map_err(|e| ToolError::Invalid(e.to_string()))?;
        let mut delta: Vec<Value> = st
                    .query_map(rusqlite::params![since, limit], |r| {
                        Ok(json!({ "id": r.get::<_, String>(0)?, "text": r.get::<_, String>(1)?,
                                   "created": crate::util::iso(r.get::<_, i64>(2)?), "at": r.get::<_, i64>(2)? }))
                    })
                    .map_err(|e| ToolError::Invalid(e.to_string()))?
                    .flatten()
                    .collect();
        if newest {
            delta.reverse();
        }
        // Each note of the delta comes with its tags (the tag of its space, its signs and measures) and its fields.
        for d in &mut delta {
            let tags = d["id"]
                .as_str()
                .and_then(|id| facts::read_fact(&conn, id).ok().flatten())
                .map(|f| f.tags)
                .unwrap_or_default();
            d["fields"] = crate::autotag::fields_json(&tags);
            d["tags"] = json!(tags);
        }
        let behind: i64 = conn
                    .query_row(
                        &format!("SELECT COUNT(*) FROM facts f WHERE f.status = 'active' AND f.created_at > ?1 AND f.id NOT IN ({OUTSIDE})"),
                        [since],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
        let last = meta.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null);
        return Ok(json!({
            "space": space_name, "page": page, "columns": columns, "board": board,
            "covers": since, "last_pass": { "at": last["at"], "by": last["by"] },
            "behind": behind, "first_pass": since == 0, "delta": delta,
        }));
    }
    let write_card = |text: &str,
                      col: &str,
                      replacing: Option<&str>,
                      signs: &[String]|
     -> Result<String, ToolError> {
        let mut new = NewFact::agent(text, &caller.agent);
        new.tags = vec![format!("living/{page}/{col}")];
        // A target groups signs and symptoms: `sign/<name>`, the same tags the notes carry.
        new.tags.extend(sign_tags(signs));
        if let Some(old) = replacing {
            return Ok(facts::supersede(
                &conn,
                &space,
                &caller.agent,
                old,
                new,
                Some(&format!("moved to {col}")),
            )?
            .id);
        }
        // A card is its own note, even next to a similar one.
        match facts::remember(&conn, &space, &caller.agent, new, &[])? {
            facts::Remembered::Created(f) | facts::Remembered::Duplicate(f) => Ok(f.id),
            facts::Remembered::NeedsDecision(c) => {
                let ids: Vec<String> = c.into_iter().map(|c| c.id).collect();
                let mut again = NewFact::agent(text, &caller.agent);
                again.tags = vec![format!("living/{page}/{col}")];
                again.tags.extend(sign_tags(signs));
                match facts::remember(&conn, &space, &caller.agent, again, &ids)? {
                    facts::Remembered::Created(f) | facts::Remembered::Duplicate(f) => Ok(f.id),
                    facts::Remembered::NeedsDecision(_) => {
                        Err(ToolError::Invalid("the card could not be written".into()))
                    }
                }
            }
        }
    };
    let valid = |col: &str| -> Result<(), ToolError> {
        if columns.contains(&col) {
            Ok(())
        } else {
            Err(ToolError::Invalid(format!(
                "column: one of {}",
                columns.join(", ")
            )))
        }
    };
    let mut added = vec![];
    for card in args
        .get("add")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let text = card
            .as_str()
            .map(String::from)
            .or_else(|| card["text"].as_str().map(String::from))
            .unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let col = card["column"].as_str().unwrap_or(columns[0]).to_string();
        valid(&col)?;
        let signs: Vec<String> = card["signs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        added.push(write_card(&text, &col, None, &signs)?);
    }
    let mut moved = 0;
    for m in args
        .get("move")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let (Some(id), Some(col)) = (m["id"].as_str(), m["column"].as_str()) else {
            continue;
        };
        valid(col)?;
        if let Some((text, now_in)) = card_column(id)
            && now_in != col
        {
            let signs: Vec<String> = facts::read_fact(&conn, id)?
                .map(|f| {
                    f.tags
                        .iter()
                        .filter_map(|t| t.strip_prefix("sign/").map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            write_card(&text, col, Some(id), &signs)?;
            moved += 1;
        }
    }
    let mut dropped = 0;
    for id in args
        .get("drop")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if card_column(id).is_some() {
            facts::retire(&conn, &caller.agent, id, "dropped from the board")?;
            dropped += 1;
        }
    }
    // The pass: when, by whom, and how far it read (only forward).
    let covers = match args.get("covers") {
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(parse_time))
            .ok_or_else(|| {
                ToolError::Invalid("covers: seconds, or YYYY-MM-DDTHH:MM:SS+00:00".into())
            })?
            .max(since),
        // A pass that does not say how far it read has read up to the moment it writes: an app that writes a board
        // has seen the notes it was given. Without this, a board written by an app would stay « behind » by every note
        // of the space forever (covers stuck at 0).
        None => crate::util::now().max(since),
    };
    let doc =
        json!({ "covers": covers, "at": crate::util::iso(crate::util::now()), "by": caller.agent })
            .to_string();
    let mut new = NewFact::agent(&doc, &caller.agent);
    new.tags = vec![meta_tag.clone()];
    match &meta {
        // The same pass in the same second: nothing to record.
        Some((_, v)) if serde_json::from_str::<Value>(&doc).ok().as_ref() == Some(v) => {}
        Some((old, _)) => {
            facts::supersede(&conn, &space, &caller.agent, old, new, Some("a pass"))?;
        }
        None => {
            if let facts::Remembered::NeedsDecision(c) =
                facts::remember(&conn, &space, &caller.agent, new, &[])?
            {
                let ids: Vec<String> = c.into_iter().map(|c| c.id).collect();
                let mut again = NewFact::agent(&doc, &caller.agent);
                again.tags = vec![meta_tag.clone()];
                facts::remember(&conn, &space, &caller.agent, again, &ids)?;
            }
        }
    }
    Ok(
        json!({ "space": space_name, "page": page, "added": added, "moved": moved, "dropped": dropped, "covers": covers }),
    )
}
