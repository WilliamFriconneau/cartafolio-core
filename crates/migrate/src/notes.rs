use crate::{
    report::Report,
    rpc::{array, next, parse, text, Client, Result},
    ORIGIN,
};
use pensine_store::normalize_tag;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Deserialize)]
struct Fact {
    id: String,
    text: String,
    tags: Vec<String>,
    author: String,
    created_at: i64,
    status: String,
}

pub struct Note {
    pub id: String,
    pub text: String,
    pub tags: Vec<String>,
    author: String,
    at: i64,
    pub raw: Value,
    pub retired: bool,
}

impl Note {
    pub fn arguments(&self) -> Value {
        json!({"text":self.text,"tags":self.tags,"author":self.author,"at":self.at,"origin":[ORIGIN,self.id]})
    }
}

fn decode(value: Value, report: &mut Report) -> Result<Option<Note>> {
    let fact: Fact = parse(&value)?;
    if !matches!(fact.status.as_str(), "active" | "retired") || value["type"] == "file" {
        return Ok(None);
    }
    let mut tags = Vec::new();
    for tag in fact.tags {
        match normalize_tag(&tag) {
            Ok(tag) => tags.push(tag),
            Err(_) => report.tags_dropped += 1,
        }
    }
    Ok(Some(Note {
        id: fact.id,
        text: fact.text,
        tags,
        author: fact.author,
        at: fact.created_at,
        retired: fact.status == "retired",
        raw: value,
    }))
}

pub fn retired_count(stats: Option<&Value>, space: &str) -> Option<u64> {
    let space = stats?.get(space)?;
    let counts = space.get("counts").unwrap_or(space);
    if counts.is_object() && space["restricted_to"].is_null() {
        Some(counts["retired"].as_u64().unwrap_or(0))
    } else {
        None
    }
}

pub fn read(
    client: &Client<'_>,
    space: &str,
    retired: bool,
    retired_count: Option<u64>,
    report: &mut Report,
) -> Vec<Note> {
    let mut notes = Vec::new();
    if let Err(error) = active(client, space, report, &mut notes) {
        report.error(error);
    }
    if retired {
        if let Err(error) = read_retired(client, space, retired_count, report, &mut notes) {
            report.error(error);
        }
    }
    notes.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    notes
}

fn active(
    client: &Client<'_>,
    space: &str,
    report: &mut Report,
    notes: &mut Vec<Note>,
) -> Result<()> {
    let mut args =
        json!({"space":space,"view":"chronology","type":"note","limit":200,"text_limit":1});
    let mut cursors = HashSet::new();
    let mut ids = HashSet::new();
    loop {
        let page = client.call("space_view", args.clone())?.data;
        for item in array(&page["items"])? {
            if item["type"] == "movement" || item["type"] == "file" || item["status"] == "retired" {
                continue;
            }
            let id = text(&item["id"])?;
            if !ids.insert(id.to_owned()) {
                report.error(format!("Duplicate source note {id:?}"));
                continue;
            }
            let note = client
                .call("read_fact", json!({"id":id}))
                .and_then(|reply| {
                    let raw = reply.data["fact"].clone();
                    if raw["id"] != id || raw["space"] != space || raw["status"] != "active" {
                        return Err("Source note changed or belongs to a different space".into());
                    }
                    decode(raw, report)
                });
            match note {
                Ok(Some(note)) => notes.push(note),
                Ok(None) => {}
                Err(error) => report.error(format!("Source note {id:?}: {error}")),
            }
        }
        let Some(cursor) = next(&page["page"]["next_cursor"], &mut cursors)? else {
            if page["page"]["has_more"] == true {
                return Err("Source chronology omitted its next cursor".into());
            }
            return Ok(());
        };
        args["cursor"] = json!(cursor);
    }
}

fn read_retired(
    client: &Client<'_>,
    space: &str,
    expected: Option<u64>,
    report: &mut Report,
    notes: &mut Vec<Note>,
) -> Result<()> {
    let result = client
        .call(
            "recall",
            json!({
                "space":space,"query":"","limit":200,"include_retired":true,"include_expired":true
            }),
        )?
        .data;
    let items = array(&result["results"])?;
    let mut seen = HashSet::new();
    for item in items {
        if item["space"] == space && item["status"] == "retired" {
            let id = text(&item["id"])?;
            if seen.insert(id) {
                match decode(item.clone(), report) {
                    Ok(Some(note)) => notes.push(note),
                    Ok(None) => {}
                    Err(error) => report.error(format!("Retired source note {id:?}: {error}")),
                }
            }
        }
    }
    if let Some(expected) = expected {
        if seen.len() as u64 != expected {
            return Err(format!(
                "Old recall returned {} of {expected} retired facts",
                seen.len()
            ));
        }
    } else if items.len() >= 200 {
        return Err("The old recall window may omit retired notes (200-result cap)".into());
    }
    Ok(())
}

pub type Pairs = BTreeMap<String, (String, bool)>;

pub fn read_back(client: &Client<'_>, space: &str, include_retired: bool) -> Result<Pairs> {
    let mut pairs = Pairs::new();
    list_status(client, space, false, &mut pairs)?;
    if include_retired {
        list_status(client, space, true, &mut pairs)?;
    }
    Ok(pairs)
}

fn list_status(client: &Client<'_>, space: &str, retired: bool, pairs: &mut Pairs) -> Result<()> {
    let status = if retired { "retired" } else { "active" };
    let mut args = json!({"space":space,"limit":200,"status":status,"origin_aliases":true});
    let mut cursors = HashSet::new();
    loop {
        let page = client.call("list", args.clone())?.data;
        for fact in array(&page["items"])? {
            let id = text(&fact["id"])?;
            let aliases = page["origin_aliases"][id].as_array();
            let primary = fact.get("origin").filter(|origin| !origin.is_null());
            for origin in primary.into_iter().chain(aliases.into_iter().flatten()) {
                let [app, id] = array(origin)?.as_slice() else {
                    return Err("Invalid destination origin".into());
                };
                if app == ORIGIN
                    && pairs
                        .insert(text(id)?.into(), (text(&fact["text"])?.into(), retired))
                        .is_some()
                {
                    return Err("Duplicate origin in destination".into());
                }
            }
        }
        let Some(cursor) = next(&page["next_cursor"], &mut cursors)? else {
            return Ok(());
        };
        args["cursor"] = json!(cursor);
    }
}
