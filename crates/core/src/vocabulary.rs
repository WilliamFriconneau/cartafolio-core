//! Vocabularies: lists of terms a person brings (signs, symptoms, a project's words) that the router reads
//! after the `rules` note of the space. They live in a table of the space database, never as notes, so they add
//! nothing to the counts or the journal. Importing a vocabulary under a name that exists replaces its terms.

use rusqlite::{Connection, params};

use crate::error::{Error, Result};

/// The most terms one vocabulary keeps.
pub const MAX_TERMS: usize = 200_000;
/// The longest term, in characters.
pub const MAX_TERM_LEN: usize = 60;

/// One vocabulary of a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vocabulary {
    pub name: String,
    pub source: String,
    pub terms: usize,
    pub imported_at: i64,
}

/// A term as the router compares it: lower case, no leading `#`, spaces folded.
pub fn clean_term(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('#').to_lowercase();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty() && t.chars().count() <= MAX_TERM_LEN).then_some(t)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Reads a plain file: one term per line, or `term<TAB>group<TAB>code`; blank lines and `#` comments are skipped.
pub fn parse(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let mut p = l.split('\t');
            let term = clean_term(p.next()?)?;
            let group = p.next().unwrap_or("").trim().to_string();
            let code = p.next().unwrap_or("").trim().to_string();
            Some((term, group, code))
        })
        .collect()
}

/// Imports (replaces) the vocabulary `name`; the number of terms kept.
pub fn import(
    conn: &mut Connection,
    name: &str,
    source: &str,
    terms: &[(String, String, String)],
    now: i64,
) -> Result<usize> {
    if !valid_name(name) {
        return Err(Error::Invalid(
            "the name of a vocabulary: lower case letters, digits, - and _ (40 at most)".into(),
        ));
    }
    if terms.len() > MAX_TERMS {
        return Err(Error::Invalid(format!("at most {MAX_TERMS} terms")));
    }
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM vocabulary_terms WHERE name = ?1", [name])?;
    let mut kept = 0usize;
    {
        let mut st = tx.prepare(
            "INSERT OR IGNORE INTO vocabulary_terms (name, term, grp, code) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (term, group, code) in terms {
            kept += st.execute(params![name, term, group, code])?;
        }
    }
    tx.execute(
        "INSERT INTO vocabularies (name, source, imported_at, terms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET source = ?2, imported_at = ?3, terms = ?4",
        params![name, source, now, kept as i64],
    )?;
    tx.commit()?;
    Ok(kept)
}

/// The vocabularies of a space.
pub fn list(conn: &Connection) -> Result<Vec<Vocabulary>> {
    let mut st =
        conn.prepare("SELECT name, source, terms, imported_at FROM vocabularies ORDER BY name")?;
    let rows = st.query_map([], |r| {
        Ok(Vocabulary {
            name: r.get(0)?,
            source: r.get(1)?,
            terms: usize::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            imported_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Removes a vocabulary and its terms; whether it existed.
pub fn remove(conn: &mut Connection, name: &str) -> Result<bool> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM vocabulary_terms WHERE name = ?1", [name])?;
    let n = tx.execute("DELETE FROM vocabularies WHERE name = ?1", [name])?;
    tx.commit()?;
    Ok(n == 1)
}

/// Every distinct term of the space's vocabularies, up to `limit`, in alphabetical order.
pub fn terms(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut st =
        conn.prepare("SELECT DISTINCT term FROM vocabulary_terms ORDER BY term LIMIT ?1")?;
    let rows = st.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
