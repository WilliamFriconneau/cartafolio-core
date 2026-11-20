//! Maintenance retags on the machine of the node: taking tags off notes in bulk (tags a tagger put on the keys of a JSON
//! dump, say). Each note is replaced by itself without those tags, the way `tags_set` does it: the old version stays in the
//! history and the new one keeps the date, author, source and origin of the note.

use rusqlite::params;

use crate::error::Result;
use crate::facts::{self, NewFact};
use crate::{Brain, Space};

/// What a bulk tag removal did, or would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Untagged {
    /// Space worked on.
    pub space: String,
    /// Active notes that carry one of the tags (and the `only` tag, when given).
    pub matched: usize,
    /// Of those, notes changed (a note that would be left with no tag is left alone and counted in `kept`).
    pub changed: usize,
    /// Notes left alone because they would have no tag left.
    pub kept: usize,
}

/// Takes `remove` off the active notes of `space` that carry any of them (and, when `only` is given, that carry that tag
/// too). `apply` false only counts.
pub fn remove_tags(
    brain: &Brain,
    space: &Space,
    remove: &[String],
    only: Option<&str>,
    apply: bool,
) -> Result<Untagged> {
    let conn = brain.open_space(space)?;
    let mut out = Untagged {
        space: space.as_str().to_string(),
        ..Untagged::default()
    };
    let mut ids: Vec<String> = Vec::new();
    for tag in remove {
        let mut st = conn.prepare(
            "SELECT f.id FROM facts f JOIN fact_tags t ON t.fact_id = f.id
             WHERE f.status = 'active' AND t.tag = ?1
               AND (?2 IS NULL OR EXISTS (SELECT 1 FROM fact_tags o WHERE o.fact_id = f.id AND o.tag = ?2))",
        )?;
        for id in st
            .query_map(params![tag, only], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    out.matched = ids.len();
    for id in ids {
        let Some(old) = facts::read_fact(&conn, &id)? else {
            continue;
        };
        let tags: Vec<String> = old
            .tags
            .iter()
            .filter(|t| !remove.contains(t))
            .cloned()
            .collect();
        if tags.is_empty() {
            out.kept += 1;
            continue;
        }
        if !apply {
            out.changed += 1;
            continue;
        }
        let mut new = NewFact::agent(&old.text, "agent:maintenance");
        new.kind = Some(old.kind.clone());
        new.valid_from = Some(old.valid_from);
        new.valid_to = old.valid_to;
        new.tags = tags;
        let f = facts::supersede(
            &conn,
            space,
            "maintenance",
            &id,
            new,
            Some("maintenance: tags taken off"),
        )?;
        facts::carry_provenance(&conn, &id, &f.id)?;
        out.changed += 1;
    }
    Ok(out)
}
