//! The answer to a request in a `text/event-stream` body: the first event that carries the id.

use std::io::BufRead;

use serde_json::{Value, json};

/// The data of each event, as soon as its blank line arrives.
fn events<R: BufRead>(reader: R) -> impl Iterator<Item = String> {
    let mut lines = reader.lines();
    std::iter::from_fn(move || {
        let mut data: Vec<String> = Vec::new();
        for line in lines.by_ref() {
            let Ok(line) = line else { return None };
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if data.is_empty() {
                    continue;
                }
                return Some(data.join("\n"));
            }
            if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            }
        }
        (!data.is_empty()).then(|| data.join("\n"))
    })
}

pub fn answer_to<R: BufRead>(reader: R, id: u64) -> Option<String> {
    events(reader).find(|e| {
        serde_json::from_str::<Value>(e).is_ok_and(|v| {
            v["id"] == json!(id) && (v.get("result").is_some() || v.get("error").is_some())
        })
    })
}
