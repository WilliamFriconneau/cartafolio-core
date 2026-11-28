use crate::fact::{fact_from_row, COLUMNS};
use crate::list::{json_array, tag_filter};
use crate::{Fact, Store, StoreError};
use rusqlite::{named_params, Connection};
use std::collections::HashSet;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

#[derive(Clone, Debug)]
pub struct RecallQuery {
    pub text: String,
    pub spaces: Vec<String>,
    pub tags_any: Vec<String>,
    pub since: Option<i64>,
    pub limit: usize,
    pub include_retired: bool,
}

impl Default for RecallQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            spaces: Vec::new(),
            tags_any: Vec::new(),
            since: None,
            limit: 50,
            include_retired: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Hit {
    pub fact: Fact,
    /// FTS5 bm25 score, multiplied by 1.5 for pinned facts. Lower scores rank first.
    pub score: f64,
    pub snippet: String,
}

impl Store {
    pub fn recall(&self, query: RecallQuery) -> Result<Vec<Hit>, StoreError> {
        self.recall_with_pin_filter(query, None)
    }

    /// Searches with a pin filter applied before ranking and limiting the results.
    pub fn recall_with_pin_filter(
        &self,
        query: RecallQuery,
        pinned: Option<bool>,
    ) -> Result<Vec<Hit>, StoreError> {
        let mut terms = HashSet::new();
        let expression = words(&query.text)
            .filter_map(|(_, word)| terms.insert(word.clone()).then(|| quote(&word)))
            .collect::<Vec<_>>()
            .join(" AND ");
        if expression.is_empty() {
            return Ok(Vec::new());
        }
        if !(1..=50).contains(&query.limit) {
            return Err(StoreError::Invalid("Recall limit must be 1 to 50".into()));
        }

        let conn = self.reader()?;
        let mut stmt = conn.prepare_cached(&format!(
            "WITH ranked AS MATERIALIZED (
                SELECT facts.rowid AS fact_rowid, facts.created_at AS fact_created_at,
                       bm25(fts) * CASE WHEN facts.pinned THEN 1.5 ELSE 1.0 END AS score
                FROM fts
                CROSS JOIN search_facts ON search_facts.rowid = fts.rowid
                CROSS JOIN facts ON facts.id = search_facts.fact_id
                WHERE fts MATCH :expression
                  AND (:include_retired OR facts.status = 'active')
                  AND (:pinned IS NULL OR facts.pinned = :pinned)
                  AND (:since IS NULL OR facts.created_at >= :since)
                  AND (:spaces = '[]' OR facts.space IN (SELECT value FROM json_each(:spaces)))
                  AND (:tags_any = '[]' OR EXISTS (
                      SELECT 1 FROM fact_tags
                      WHERE fact_id = facts.id AND tag IN (SELECT value FROM json_each(:tags_any))
                  ))
                ORDER BY score, fact_created_at DESC, fact_rowid DESC
                LIMIT :limit
             )
             SELECT {COLUMNS}, ranked.score
             FROM ranked CROSS JOIN facts ON facts.rowid = ranked.fact_rowid
             ORDER BY ranked.score, ranked.fact_created_at DESC, ranked.fact_rowid DESC"
        ))?;
        let rows = stmt.query_map(
            named_params! {
                ":expression": expression,
                ":include_retired": query.include_retired,
                ":since": query.since,
                ":pinned": pinned,
                ":spaces": json_array(&query.spaces)?,
                ":tags_any": tag_filter(&query.tags_any)?,
                ":limit": query.limit,
            },
            |row| Ok((fact_from_row(row)?, row.get(13)?)),
        )?;
        let matches = rows.collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        drop(conn);

        Ok(matches
            .into_iter()
            .map(|(fact, score)| Hit {
                snippet: snippet(&fact.text, &terms),
                fact,
                score,
            })
            .collect())
    }
}

pub(crate) fn index(
    conn: &Connection,
    id: &str,
    text: &str,
    tags: &[String],
) -> Result<(), StoreError> {
    let folded = std::iter::once(text)
        .chain(tags.iter().map(String::as_str))
        .flat_map(words)
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" ");
    let rowid: i64 = conn
        .prepare_cached("INSERT INTO search_facts (fact_id) VALUES (?1) RETURNING rowid")?
        .query_row([id], |row| row.get(0))?;
    conn.prepare_cached("INSERT INTO fts (rowid, fact_id, text) VALUES (?1, ?2, ?3)")?
        .execute((rowid, id, folded))?;
    Ok(())
}

pub(crate) fn backfill(conn: &Connection) -> Result<(), StoreError> {
    let mut stmt = conn.prepare("SELECT id, text, tags_json FROM facts")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let tags_json: String = row.get(2)?;
        let id: String = row.get(0)?;
        let text: String = row.get(1)?;
        let tags: Vec<String> = serde_json::from_str(&tags_json)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        index(conn, &id, &text, &tags)?;
    }
    Ok(())
}

fn quote(word: &str) -> String {
    format!("\"{}\"", word.replace('"', "\"\""))
}

fn words(text: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    // Keep original byte offsets through folding so snippets preserve the note text.
    let mut chars = text.char_indices().flat_map(move |(offset, ch)| {
        text[offset..offset + ch.len_utf8()]
            .nfkd()
            .flat_map(char::to_lowercase)
            .filter(|&ch| !is_combining_mark(ch))
            .map(move |ch| (offset, ch))
    });
    std::iter::from_fn(move || {
        let (start, first) = chars.find(|&(_, ch)| ch.is_alphanumeric())?;
        let mut word = String::from(first);
        for (_, ch) in chars.by_ref() {
            if !ch.is_alphanumeric() {
                break;
            }
            word.push(ch);
        }
        if word.ends_with("aux")
            && word.chars().count() >= 5
            && word.chars().all(char::is_alphabetic)
        {
            word.truncate(word.len() - 3);
            word.push_str("al");
        } else if word.ends_with(['s', 'x'])
            && word.chars().count() >= 5
            && word.chars().all(char::is_alphabetic)
        {
            word.pop();
        }
        Some((start, word))
    })
}

fn snippet(text: &str, terms: &HashSet<String>) -> String {
    const LIMIT: usize = 200;
    let offset = words(text)
        .find(|(_, word)| terms.contains(word))
        .map_or(0, |(offset, _)| offset);
    let start = text[..offset]
        .char_indices()
        .rev()
        .nth(LIMIT / 2 - 1)
        .map_or(0, |(start, _)| start);
    text[start..].chars().take(LIMIT).collect()
}
