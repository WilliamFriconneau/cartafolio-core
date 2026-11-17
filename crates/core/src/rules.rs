//! The filing rules of a space: the tags and the keywords that send a note there. They are kept as one note of the
//! space tagged `rules` (a JSON document `{"version":1,"tags":[…],"keywords":[…]}`), so that they have a history.
//! Adding, editing and removing a keyword or a tag is a new version of that note; the old one stays in the history.

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::error::Result;
use crate::facts::{self, NewFact, Remembered};
use crate::space::Space;

const MAX_ITEMS: usize = 100;
const MAX_KEYWORD: usize = 40;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// The rules.
pub struct Rules {
    /// The tags.
    pub tags: Vec<String>,
    /// The keywords.
    pub keywords: Vec<String>,
}

/// What to add and what to remove. Words and tags are normalised; unknown ones to remove are ignored.
#[derive(Clone, Debug, Default)]
pub struct Edit {
    /// The add tags.
    pub add_tags: Vec<String>,
    /// The remove tags.
    pub remove_tags: Vec<String>,
    /// The add keywords.
    pub add_keywords: Vec<String>,
    /// The remove keywords.
    pub remove_keywords: Vec<String>,
}

/// Clean keyword.
pub fn clean_keyword(raw: &str) -> Option<String> {
    let k = raw.trim().trim_start_matches('#').to_lowercase();
    (!k.is_empty() && k.chars().count() <= MAX_KEYWORD).then_some(k)
}

fn tidy(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v.truncate(MAX_ITEMS);
    v
}

/// The id of the current rules note and the rules in it (empty when there is none).
pub fn read(conn: &Connection) -> (Option<String>, Rules) {
    let found = conn
        .prepare(
            "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
             (SELECT fact_id FROM fact_tags WHERE tag = 'rules') ORDER BY f.created_at DESC, f.rowid DESC LIMIT 1",
        )
        .ok()
        .and_then(|mut st| st.query_row([], |r| r.get::<_, String>(0)).ok());
    if let Some(id) = found
        && let Some(f) = facts::read_fact(conn, &id).ok().flatten()
        && let Ok(doc) = serde_json::from_str::<Value>(f.text.trim())
        && doc.get("version").and_then(Value::as_i64) == Some(1)
    {
        let list = |k: &str, tags: bool| -> Vec<String> {
            tidy(
                doc.get(k)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|t| {
                        if tags {
                            facts::normalize_tag(t).ok()
                        } else {
                            clean_keyword(t)
                        }
                    })
                    .collect(),
            )
        };
        return (
            Some(id),
            Rules {
                tags: list("tags", true),
                keywords: list("keywords", false),
            },
        );
    }
    (None, Rules::default())
}

/// Writes the rules as a new version of the note. Any other active rules note is retired, so there is one. Writing
/// what is already there changes nothing.
pub fn write(conn: &Connection, space: &Space, who: &str, rules: &Rules) -> Result<Rules> {
    let rules = Rules {
        tags: tidy(rules.tags.clone()),
        keywords: tidy(rules.keywords.clone()),
    };
    let (current, old) = read(conn);
    if current.is_some() && old == rules {
        return Ok(rules);
    }
    let doc = json!({ "version": 1, "tags": rules.tags, "keywords": rules.keywords }).to_string();
    let mut new = NewFact::agent(&doc, who);
    new.tags = vec!["rules".into()];
    let kept = match &current {
        Some(old_id) => facts::supersede(conn, space, who, old_id, new, Some("rules changed"))?.id,
        None => match facts::remember(conn, space, who, new, &[])? {
            Remembered::Created(f) | Remembered::Duplicate(f) => f.id,
            Remembered::NeedsDecision(c) => {
                // The settings note is never a near duplicate of a note: write it past the candidates.
                let ids: Vec<String> = c.into_iter().map(|c| c.id).collect();
                let mut again = NewFact::agent(&doc, who);
                again.tags = vec!["rules".into()];
                match facts::remember(conn, space, who, again, &ids)? {
                    Remembered::Created(f) | Remembered::Duplicate(f) => f.id,
                    Remembered::NeedsDecision(_) => return Ok(rules),
                }
            }
        },
    };
    let mut st = conn.prepare(
        "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id <> ?1 AND f.id IN
         (SELECT fact_id FROM fact_tags WHERE tag = 'rules')",
    )?;
    let extra: Vec<String> = st
        .query_map([&kept], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in extra {
        // A note that says something else under the tag `rules` is left alone.
        if facts::read_fact(conn, &id)?
            .is_some_and(|f| serde_json::from_str::<Value>(f.text.trim()).is_ok())
        {
            facts::retire(conn, who, &id, "rules merged into one note")?;
        }
    }
    Ok(rules)
}

/// Adds and removes keywords and tags in one new version.
pub fn edit(conn: &Connection, space: &Space, who: &str, e: &Edit) -> Result<Rules> {
    let (_, mut r) = read(conn);
    let tags = |v: &[String]| -> Vec<String> {
        v.iter()
            .filter_map(|t| facts::normalize_tag(t).ok())
            .collect()
    };
    let words =
        |v: &[String]| -> Vec<String> { v.iter().filter_map(|k| clean_keyword(k)).collect() };
    let (rt, rk) = (tags(&e.remove_tags), words(&e.remove_keywords));
    r.tags.retain(|t| !rt.contains(t));
    r.keywords.retain(|k| !rk.contains(k));
    r.tags.extend(tags(&e.add_tags));
    r.keywords.extend(words(&e.add_keywords));
    write(conn, space, who, &r)
}
