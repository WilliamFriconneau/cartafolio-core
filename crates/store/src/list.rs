use crate::cursor::{Cursor, CursorCodec};
use crate::fact::{fact_from_row, Fact, FactStatus, COLUMNS};
use crate::tags::{fold_tag, normalize_tags};
use crate::{Store, StoreError};
use rusqlite::named_params;
use std::collections::HashMap;

pub type OriginAliases = HashMap<String, Vec<[String; 2]>>;

#[derive(Clone, Debug, serde::Serialize)]
pub struct ListQuery {
    pub spaces: Vec<String>,
    pub tags_any: Vec<String>,
    pub tags_all: Vec<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub status: Option<FactStatus>,
    pub pinned: Option<bool>,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for ListQuery {
    fn default() -> Self {
        Self {
            spaces: Vec::new(),
            tags_any: Vec::new(),
            tags_all: Vec::new(),
            since: None,
            until: None,
            status: None,
            pinned: None,
            limit: 50,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl Store {
    /// Reads the additional origins assigned to deduplicated facts.
    pub fn origin_aliases(&self, ids: &[String]) -> Result<OriginAliases, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT fact_id, origin_app, origin_id FROM duplicate_origins
             WHERE fact_id IN (SELECT value FROM json_each(?1))
             ORDER BY origin_app, origin_id",
        )?;
        let mut rows = stmt.query([json_array(ids)?])?;
        let mut aliases = OriginAliases::new();
        while let Some(row) = rows.next()? {
            aliases
                .entry(row.get(0)?)
                .or_default()
                .push([row.get(1)?, row.get(2)?]);
        }
        Ok(aliases)
    }

    pub fn list(&self, query: ListQuery) -> Result<Page<Fact>, StoreError> {
        if !(1..=200).contains(&query.limit) {
            return Err(StoreError::Invalid("List limit must be 1 to 200".into()));
        }
        let sort = if query.pinned == Some(true) {
            "pinned_at"
        } else {
            "created_at"
        };
        let codec = CursorCodec::new(&self.cursor_key, "facts", &query)?;
        let cursor = query
            .cursor
            .as_deref()
            .map(|value| codec.decode(value))
            .transpose()?;
        let spaces = json_array(&query.spaces)?;
        let tags_any = tag_filter(&query.tags_any)?;
        let tags_all = tag_filter(&query.tags_all)?;
        let status = query.status.unwrap_or(FactStatus::Active).as_str();
        let conn = self.reader()?;
        // The rowid ceiling excludes later inserts, including offline captures
        // whose timestamps would otherwise put them on a subsequent page.
        let visible = "status = :status
                  AND (:pinned IS NULL OR pinned = :pinned)
               AND (:since IS NULL OR created_at >= :since)
               AND (:until IS NULL OR created_at <= :until)
               AND (:spaces = '[]' OR space IN (SELECT value FROM json_each(:spaces)))
               AND (:tags_any = '[]' OR EXISTS (
                   SELECT 1 FROM fact_tags
                   WHERE fact_id = facts.id AND tag IN (SELECT value FROM json_each(:tags_any))
               ))
               AND (:tags_all = '[]' OR NOT EXISTS (
                   SELECT 1 FROM json_each(:tags_all) AS required
                   WHERE NOT EXISTS (
                       SELECT 1 FROM fact_tags WHERE fact_id = facts.id AND tag = required.value
                   )
               ))";
        let mut stmt = conn.prepare_cached(&format!(
            "WITH boundary(snapshot) AS MATERIALIZED (
                SELECT CASE WHEN :snapshot IS NULL
                            THEN (SELECT max(rowid) FROM facts WHERE {visible})
                            ELSE (SELECT rowid FROM facts WHERE id = :snapshot) END
             )
             SELECT {COLUMNS}, (SELECT id FROM facts WHERE rowid = (SELECT snapshot FROM boundary)), {sort}
             FROM facts
             WHERE {visible}
               AND facts.rowid <= (SELECT snapshot FROM boundary)
               AND (:last IS NULL OR ({sort}, facts.rowid) <
                   (:at, (SELECT rowid FROM facts WHERE id = :last)))
             ORDER BY {sort} DESC, facts.rowid DESC
             LIMIT :limit"
        ))?;
        let mut rows = stmt.query(named_params! {
            ":snapshot": cursor.as_ref().map(|c| c.snapshot.as_str()),
            ":at": cursor.as_ref().map(|c| c.at),
            ":last": cursor.as_ref().map(|c| c.last.as_str()),
            ":status": status,
            ":pinned": query.pinned,
            ":since": query.since,
            ":until": query.until,
            ":spaces": spaces,
            ":tags_any": tags_any,
            ":tags_all": tags_all,
            ":limit": query.limit + 1,
        })?;
        let mut items = Vec::new();
        let mut last = None;
        while let Some(row) = rows.next()? {
            if items.len() == query.limit {
                return Ok(Page {
                    items,
                    next_cursor: last
                        .as_ref()
                        .map(|cursor| codec.encode(cursor))
                        .transpose()?,
                });
            }
            let fact = fact_from_row(row)?;
            last = Some(Cursor {
                snapshot: row.get(13)?,
                at: row.get(14)?,
                last: fact.id.clone(),
            });
            items.push(fact);
        }
        Ok(Page {
            items,
            next_cursor: None,
        })
    }
}

pub(crate) fn tag_filter(tags: &[String]) -> Result<String, StoreError> {
    let folded: Vec<_> = normalize_tags(tags)?
        .iter()
        .map(|tag| fold_tag(tag))
        .collect();
    json_array(&folded)
}

pub(crate) fn json_array(values: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(values).map_err(|error| StoreError::Invalid(error.to_string()))
}
