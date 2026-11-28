use crate::fact::{fact_from_row, Fact, FactStatus, COLUMNS};
use crate::tags::{fold_tag, normalize_tags};
use crate::{Store, StoreError};
use rusqlite::named_params;

#[derive(Clone, Debug)]
pub struct ListQuery {
    pub spaces: Vec<String>,
    pub tags_any: Vec<String>,
    pub tags_all: Vec<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub status: Option<FactStatus>,
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
            limit: 50,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

struct Cursor {
    snapshot: i64,
    created_at: i64,
    rowid: i64,
}

impl Cursor {
    fn parse(value: &str) -> Result<Self, StoreError> {
        let invalid = || StoreError::Invalid("Invalid list cursor".into());
        let parts: Vec<_> = value.split('.').collect();
        let ["1", snapshot, created_at, rowid] = parts.as_slice() else {
            return Err(invalid());
        };
        let cursor = Self {
            snapshot: snapshot.parse().map_err(|_| invalid())?,
            created_at: created_at.parse().map_err(|_| invalid())?,
            rowid: rowid.parse().map_err(|_| invalid())?,
        };
        if cursor.rowid < 1 || cursor.rowid > cursor.snapshot {
            return Err(invalid());
        }
        Ok(cursor)
    }

    fn encode(&self) -> String {
        format!("1.{}.{}.{}", self.snapshot, self.created_at, self.rowid)
    }
}

impl Store {
    pub fn list(&self, query: ListQuery) -> Result<Page<Fact>, StoreError> {
        if !(1..=200).contains(&query.limit) {
            return Err(StoreError::Invalid("List limit must be 1 to 200".into()));
        }
        let cursor = query.cursor.as_deref().map(Cursor::parse).transpose()?;
        let spaces = json_array(&query.spaces)?;
        let tags_any = tag_filter(&query.tags_any)?;
        let tags_all = tag_filter(&query.tags_all)?;
        let status = query.status.unwrap_or(FactStatus::Active).as_str();
        let conn = self.reader()?;
        // The rowid ceiling excludes later inserts, including offline captures
        // whose timestamps would otherwise put them on a subsequent page.
        let mut stmt = conn.prepare(&format!(
            "WITH boundary(snapshot) AS (
                SELECT coalesce(:snapshot, (SELECT coalesce(max(rowid), 0) FROM facts))
             )
             SELECT {COLUMNS}, facts.rowid, boundary.snapshot
             FROM facts CROSS JOIN boundary
             WHERE status = :status AND facts.rowid <= boundary.snapshot
               AND (:rowid IS NULL OR (created_at, facts.rowid) < (:at, :rowid))
               AND (:since IS NULL OR created_at >= :since)
               AND (:until IS NULL OR created_at <= :until)
               AND (:spaces = '[]' OR space IN (SELECT value FROM json_each(:spaces)))
               AND (:tags_any = '[]' OR EXISTS (
                   SELECT 1 FROM fact_tags
                   WHERE fact_id = facts.id AND tag IN (SELECT value FROM json_each(:tags_any))
               ))
               AND NOT EXISTS (
                   SELECT 1 FROM json_each(:tags_all) AS required
                   WHERE NOT EXISTS (
                       SELECT 1 FROM fact_tags WHERE fact_id = facts.id AND tag = required.value
                   )
               )
             ORDER BY created_at DESC, facts.rowid DESC
             LIMIT :limit"
        ))?;
        let mut rows = stmt.query(named_params! {
            ":snapshot": cursor.as_ref().map(|c| c.snapshot),
            ":at": cursor.as_ref().map(|c| c.created_at),
            ":rowid": cursor.as_ref().map(|c| c.rowid),
            ":status": status,
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
                    next_cursor: last.as_ref().map(Cursor::encode),
                });
            }
            let fact = fact_from_row(row)?;
            last = Some(Cursor {
                snapshot: row.get(13)?,
                created_at: fact.created_at,
                rowid: row.get(12)?,
            });
            items.push(fact);
        }
        Ok(Page {
            items,
            next_cursor: None,
        })
    }
}

fn tag_filter(tags: &[String]) -> Result<String, StoreError> {
    let folded: Vec<_> = normalize_tags(tags)?
        .iter()
        .map(|tag| fold_tag(tag))
        .collect();
    json_array(&folded)
}

fn json_array(values: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(values).map_err(|error| StoreError::Invalid(error.to_string()))
}
