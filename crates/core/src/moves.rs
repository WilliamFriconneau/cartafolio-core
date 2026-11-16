//! Moving notes from one space to another, by tag: what a person does when they make a sub-space (the cars of the
//! house) and want what already belongs to it to go there.
//!
//! A move is never an erasure: the note is written again in the target space, with its text, its tags, its dates and
//! its author, and **retired** in the source with the reason "moved to …", so the history of the source says where it
//! went. Files and imported passages are left where they are and counted, not moved.

use crate::brain::Brain;
use crate::error::Result;
use crate::facts::{self, NewFact, Remembered, Source};
use crate::space::Space;

/// What a move did, or would do.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Moved {
    /// Notes that match the tag.
    pub matched: usize,
    /// Notes moved (zero when only previewed).
    pub moved: usize,
    /// Notes of a file or an imported passage, left in place.
    pub left: usize,
    /// Notes the target refused (a near duplicate the person must decide on).
    pub failed: usize,
    /// The first few, for a preview.
    pub sample: Vec<String>,
}

fn extract(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

impl Brain {
    /// The notes of `from` filed under `tag` (or below it) go to `to`. With `dry_run` nothing changes and the report
    /// says what would.
    pub fn move_by_tag(
        &self,
        from: &Space,
        to: &Space,
        tag: &str,
        who: &str,
        dry_run: bool,
    ) -> Result<Moved> {
        let tag = facts::normalize_tag(tag)?;
        let src = self.open_space(from)?;
        let dst = self.open_space(to)?;
        let like = format!("{tag}/%");
        let ids: Vec<String> = {
            let mut st = src.prepare(
                "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.id IN
                   (SELECT fact_id FROM fact_tags WHERE tag = ?1 OR tag LIKE ?2)
                 AND f.id NOT IN
                   (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide'))
                 ORDER BY f.created_at",
            )?;
            st.query_map([&tag, &like], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Moved::default();
        for id in ids {
            let Some(f) = facts::read_fact(&src, &id)? else {
                continue;
            };
            out.matched += 1;
            if f.typ.as_deref() == Some("file") || f.source_kind != "agent" {
                out.left += 1;
                continue;
            }
            if out.sample.len() < 5 {
                out.sample.push(extract(&f.text, 120));
            }
            if dry_run {
                continue;
            }
            let mut new = NewFact::agent(
                &f.text,
                f.source_agent.as_deref().unwrap_or(f.author.as_str()),
            );
            new.kind = f.kind.clone();
            new.valid_from = Some(f.valid_from);
            new.valid_to = f.valid_to;
            new.tags = f.tags.clone();
            new.source = Source::Agent(f.source_agent.clone().unwrap_or_else(|| f.author.clone()));
            let mut decided: Vec<String> = vec![];
            let written = loop {
                match facts::remember(&dst, to, &f.author, new.clone(), &decided)? {
                    Remembered::Created(_) | Remembered::Duplicate(_) => break true,
                    Remembered::NeedsDecision(c) if decided.is_empty() => {
                        decided = c.into_iter().map(|c| c.id).collect();
                    }
                    Remembered::NeedsDecision(_) => break false,
                }
            };
            if !written {
                out.failed += 1;
                continue;
            }
            facts::retire(&src, who, &id, &format!("moved to {to}"))?;
            out.moved += 1;
        }
        Ok(out)
    }
}
