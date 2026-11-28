use crate::{
    notes::Note,
    rpc::{array, parse, Client, Result},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct OldSpace {
    name: String,
    label: Option<String>,
    parent: Option<String>,
    kind: Option<String>,
}

pub struct Space {
    pub old: String,
    pub new: String,
    label: String,
    parent: Option<String>,
    pub selected: bool,
    pub raw: Value,
    inbox: bool,
}

impl Space {
    pub fn routes_to_inbox(&self) -> bool {
        self.inbox
    }

    pub fn note_target(&self, note: &Note) -> &str {
        if self.inbox && note.tags.iter().any(|tag| tag == "inbox") {
            "inbox"
        } else {
            &self.new
        }
    }
}

pub fn valid_target(name: &str) -> bool {
    (2..=40).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

pub fn plan(
    inventory: &Value,
    only: &[String],
    mappings: &[(String, String)],
) -> Result<Vec<Space>> {
    let mut old = BTreeMap::new();
    for value in array(&inventory["spaces"])? {
        let space: OldSpace = parse(value)?;
        if old.insert(space.name.clone(), (space, value)).is_some() {
            return Err("Duplicate space in source inventory".into());
        }
    }
    let mapped: BTreeMap<_, _> = mappings
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    if mapped.len() != mappings.len() {
        return Err("A source space has more than one mapping".into());
    }
    for name in only
        .iter()
        .map(String::as_str)
        .chain(mapped.keys().copied())
    {
        if !old.contains_key(name) {
            return Err(format!("Unknown source space {name:?}"));
        }
    }
    let mut order = Vec::new();
    let mut added = BTreeSet::new();
    for name in old
        .keys()
        .filter(|name| only.is_empty() || only.contains(name))
    {
        let mut chain = Vec::new();
        let mut current = name.as_str();
        while !added.contains(current) {
            if chain.contains(&current) {
                return Err("Source space tree contains a cycle".into());
            }
            chain.push(current);
            let (space, _) = old
                .get(current)
                .ok_or("Source space tree has a missing parent")?;
            let Some(parent) = space.parent.as_deref() else {
                break;
            };
            current = parent;
        }
        for name in chain.into_iter().rev() {
            added.insert(name);
            order.push(name);
        }
    }
    let target = |name| mapped.get(name).copied().unwrap_or(name).to_owned();
    let mut plan = Vec::new();
    for name in order {
        let (space, raw) = &old[name];
        let new = target(name);
        if !valid_target(&new) {
            return Err(format!(
                "Source space {name:?} needs a valid --map destination"
            ));
        }
        plan.push(Space {
            old: name.to_owned(),
            raw: (*raw).clone(),
            label: space.label.clone().unwrap_or_else(|| new.clone()),
            parent: space
                .parent
                .as_deref()
                .map(target)
                .filter(|parent| parent != &new),
            new,
            selected: only.is_empty() || only.iter().any(|s| s == name),
            inbox: !mapped.contains_key(name)
                && (space.kind.as_deref() == Some("inbox")
                    || space
                        .label
                        .as_deref()
                        .is_some_and(|label| label.eq_ignore_ascii_case("inbox"))
                    || name == "inbox"),
        });
    }
    Ok(plan)
}

pub fn create(client: &Client<'_>, space: &Space, existing: &mut BTreeSet<String>) -> Result<()> {
    ensure(
        client,
        &space.new,
        &space.label,
        space.parent.as_deref(),
        existing,
    )
}

pub fn create_inbox(
    client: &Client<'_>,
    target: &str,
    existing: &mut BTreeSet<String>,
) -> Result<()> {
    if target == "inbox" {
        ensure(client, target, "Inbox", None, existing)?;
    }
    Ok(())
}

fn ensure(
    client: &Client<'_>,
    name: &str,
    label: &str,
    parent: Option<&str>,
    existing: &mut BTreeSet<String>,
) -> Result<()> {
    if existing.contains(name) {
        return Ok(());
    }
    let mut args = json!({"name":name,"label":label});
    if let Some(parent) = parent {
        args["parent"] = json!(parent);
    }
    if let Err(error) = client.call("space_create", args) {
        // A retried create can conflict if the first response was lost.
        let spaces = client.call("spaces", json!({}))?.data;
        if !array(&spaces)?.iter().any(|space| space["name"] == name) {
            return Err(format!("Destination space {name:?}: {error}"));
        }
    }
    existing.insert(name.to_owned());
    Ok(())
}
