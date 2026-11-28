use crate::{
    notes::{Note, Pairs},
    rpc::{Client, Result},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Arguments, Write},
};

pub struct Report {
    source: String,
    notes: BTreeMap<String, Vec<Note>>,
    errors: Vec<String>,
    notices: Vec<String>,
    sent: usize,
    duplicates: usize,
    pub tags_dropped: usize,
    pub files_sent: usize,
    pub fallback_reads: usize,
}

impl Report {
    pub fn new(source: String) -> Self {
        Self {
            source,
            notes: BTreeMap::new(),
            errors: Vec::new(),
            notices: Vec::new(),
            sent: 0,
            duplicates: 0,
            tags_dropped: 0,
            files_sent: 0,
            fallback_reads: 0,
        }
    }

    pub fn error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    pub fn notice(&mut self, notice: String) {
        self.notices.push(notice);
    }

    pub fn add_target(&mut self, target: &str) {
        self.notes.entry(target.into()).or_default();
    }

    pub fn add_note(&mut self, target: &str, note: Note) {
        self.notes.entry(target.into()).or_default().push(note);
    }

    pub fn send_notes(
        &mut self,
        client: &Client<'_>,
        batch_size: usize,
        todos: &BTreeMap<String, String>,
    ) {
        let mut sent = 0;
        let mut duplicates = 0;
        let mut errors = Vec::new();
        let mut renamed = Vec::new();
        for (space, notes) in &mut self.notes {
            let active_todo: Vec<usize> = notes
                .iter()
                .enumerate()
                .filter(|(_, note)| !note.retired && has_todo(note))
                .map(|(index, _)| index)
                .collect();
            let superseded = match (active_todo.as_slice(), todos.get(space.as_str())) {
                ([index], Some(_)) => Some(*index),
                _ => None,
            };
            for (index, note) in notes.iter_mut().enumerate() {
                if Some(index) != superseded && has_todo(note) {
                    rename_todo(note);
                    renamed.push(note.id.clone());
                }
            }
            if let (Some(index), Some(todo_id)) = (superseded, todos.get(space.as_str())) {
                send_todo(
                    client,
                    &notes[index],
                    todo_id,
                    &mut sent,
                    &mut duplicates,
                    &mut errors,
                );
            }
            let remaining: Vec<Note> = notes
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != superseded)
                .map(|(_, note)| note.clone())
                .collect();
            for batch in remaining
                .split_inclusive(|note| note.retired)
                .flat_map(|group| group.chunks(batch_size))
            {
                send(
                    client,
                    space,
                    batch,
                    &mut sent,
                    &mut duplicates,
                    &mut errors,
                );
            }
        }
        self.sent += sent;
        self.duplicates += duplicates;
        self.errors.extend(errors);
        for id in renamed {
            self.notice(format!("todo tag renamed to todo-old for note {id:?}"));
        }
    }

    pub fn finish(
        &mut self,
        out: &mut String,
        readback: Option<&BTreeMap<String, Result<Pairs>>>,
        matched: &mut BTreeSet<(String, String)>,
    ) -> bool {
        let mut expected = BTreeMap::new();
        let mut actual = BTreeMap::new();
        if let Some(readback) = readback {
            for (target, notes) in &self.notes {
                match &readback[target] {
                    Ok(pairs) => {
                        for note in notes {
                            if let Some((text, retired)) = pairs.get(&note.id) {
                                if *retired != note.retired {
                                    self.errors
                                        .push(format!("Status differs for origin {:?}", note.id));
                                }
                                matched.insert((target.clone(), note.id.clone()));
                                actual.insert(note.id.as_str(), text.as_str());
                            }
                        }
                    }
                    Err(error) => self.errors.push(format!("Readback {target:?}: {error}")),
                }
            }
        }
        for note in self.notes.values().flatten() {
            expected.insert(note.id.as_str(), note.text.as_str());
        }
        if readback.is_some() {
            for (&id, &text) in &expected {
                match actual.get(id) {
                    None => self
                        .errors
                        .push(format!("Missing origin {id:?} in its destination")),
                    Some(&found) if found != text => {
                        self.errors.push(format!("Text differs for origin {id:?}"))
                    }
                    Some(_) => {}
                }
            }
        }
        let new_count = readback.map_or_else(|| "unchecked".into(), |_| actual.len().to_string());
        line(out, format_args!("{:?}: old count={} new count={new_count} notes sent={} duplicates={} tags dropped={} files sent={} errors={}",
            self.source, expected.len(), self.sent, self.duplicates, self.tags_dropped, self.files_sent, self.errors.len()));
        let new_hash = readback.map_or_else(|| "unchecked".into(), |_| checksum(&actual));
        line(
            out,
            format_args!(
                "integrity {:?}: old={} new={new_hash}",
                self.source,
                checksum(&expected)
            ),
        );
        for notice in &self.notices {
            line(out, format_args!("{:?}: {notice}", self.source));
        }
        for error in &self.errors {
            line(out, format_args!("{:?}: error: {error}", self.source));
        }
        self.errors.is_empty()
    }
}

fn has_todo(note: &Note) -> bool {
    note.tags.iter().any(|tag| tag == "todo")
}

fn rename_todo(note: &mut Note) {
    for tag in &mut note.tags {
        if tag == "todo" {
            *tag = "todo-old".into();
        }
    }
}

fn send_todo(
    client: &Client<'_>,
    note: &Note,
    todo_id: &str,
    sent: &mut usize,
    duplicates: &mut usize,
    errors: &mut Vec<String>,
) {
    let known = client
        .call("read", json!({"id": todo_id}))
        .ok()
        .is_some_and(|reply| reply.data["origin"] == json!([crate::origin(), note.id]));
    if known {
        *duplicates += 1;
        return;
    }
    let mut arguments = note.arguments();
    arguments["id"] = json!(todo_id);
    arguments["reason"] = json!("Migrated todo note");
    match client.call("supersede", arguments) {
        Ok(_) => *sent += 1,
        Err(error) => errors.push(format!("Destination todo note {:?}: {error}", note.id)),
    }
}

fn send(
    client: &Client<'_>,
    space: &str,
    batch: &[Note],
    sent: &mut usize,
    duplicates: &mut usize,
    errors: &mut Vec<String>,
) {
    let arguments: Vec<_> = batch.iter().map(Note::arguments).collect();
    let reply = client.call("remember_many", json!({"space":space,"notes":arguments}));
    match reply.and_then(|reply| outcomes(reply.data, batch.len())) {
        Ok(results) => {
            for (note, item) in batch.iter().zip(results) {
                if item["outcome"] == "created" {
                    *sent += 1;
                } else {
                    *duplicates += 1;
                }
                if note.retired {
                    if let Err(error) = retire(client, note, &item["fact"]) {
                        errors.push(format!("Destination note {:?}: {error}", note.id));
                    }
                }
            }
        }
        Err(error) if batch.len() > 1 && can_split(&error) => {
            let (left, right) = batch.split_at(batch.len() / 2);
            send(client, space, left, sent, duplicates, errors);
            send(client, space, right, sent, duplicates, errors);
        }
        Err(error) => {
            errors.extend(
                batch
                    .iter()
                    .map(|note| format!("Destination note {:?}: {error}", note.id)),
            );
        }
    }
}

fn can_split(error: &str) -> bool {
    matches!(
        error,
        "remember_many: Remote server rejected the request" | "remember_many: HTTP 413"
    )
}

fn outcomes(value: Value, count: usize) -> Result<Vec<Value>> {
    let Value::Array(results) = value else {
        return Err("Invalid destination batch response".into());
    };
    if results.len() != count
        || results.iter().any(|item| {
            !matches!(
                item["outcome"].as_str(),
                Some("created" | "duplicate" | "replayed")
            )
        })
    {
        return Err("Invalid destination batch outcomes".into());
    }
    Ok(results)
}

fn retire(client: &Client<'_>, note: &Note, fact: &Value) -> Result<()> {
    if fact["status"] == "retired" {
        return Ok(());
    }
    if fact["origin"] != json!([crate::origin(), note.id]) || fact["status"] != "active" {
        return Err("Cannot retire a note shared with a different origin or version".into());
    }
    let id = crate::rpc::text(&fact["id"])?;
    let reason = note.raw["status_reason"]
        .as_str()
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or("Retired on source");
    if let Err(error) = client.call("retire", json!({"id":id,"reason":reason})) {
        // A response can be lost after the note has already been retired.
        if client.call("read", json!({"id":id}))?.data["status"] != "retired" {
            return Err(error);
        }
    }
    Ok(())
}

fn checksum(pairs: &BTreeMap<&str, &str>) -> String {
    let mut hash = Sha256::new();
    for (id, text) in pairs {
        for value in [id, text] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}

pub fn line(out: &mut String, args: Arguments<'_>) {
    let _ = writeln!(out, "{args}");
}
