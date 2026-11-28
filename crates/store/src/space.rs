use crate::{Store, StoreError};
use rusqlite::Connection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceInfo {
    pub name: String,
    pub label: String,
    pub parent: Option<String>,
    pub position: i64,
    pub notes: u64,
    pub files: u64,
}

impl Store {
    pub fn space_create(
        &self,
        name: &str,
        label: &str,
        parent: Option<&str>,
    ) -> Result<(), StoreError> {
        if !(2..=40).contains(&name.len())
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(StoreError::Invalid(
                "A space name must be 2 to 40 lowercase ASCII letters, digits, or hyphens".into(),
            ));
        }
        self.write(|tx| {
            if let Some(parent) = parent {
                require_space(tx, parent)?;
            }
            let inserted = tx.execute(
                "INSERT INTO spaces (name, label, parent, position)
                 VALUES (?1, ?2, ?3, (SELECT coalesce(max(position), 0) + 1 FROM spaces))
                 ON CONFLICT(name) DO NOTHING",
                (name, label, parent),
            )?;
            if inserted == 0 {
                return Err(StoreError::Conflict);
            }
            Ok(())
        })
    }

    /// Pass `Some(None)` to move a space to the root.
    pub fn space_move(
        &self,
        name: &str,
        label: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<(), StoreError> {
        self.write(|tx| {
            require_space(tx, name)?;
            if let Some(Some(parent)) = parent {
                require_space(tx, parent)?;
                check_cycle(tx, name, parent)?;
            }
            tx.execute(
                "UPDATE spaces
                 SET label = coalesce(?2, label), parent = CASE WHEN ?3 THEN ?4 ELSE parent END
                 WHERE name = ?1",
                (name, label, parent.is_some(), parent.flatten()),
            )?;
            Ok(())
        })
    }

    pub fn spaces(&self) -> Result<Vec<SpaceInfo>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "WITH RECURSIVE
             ranked AS (
                 SELECT *, row_number() OVER (ORDER BY position, name) AS rank FROM spaces
             ),
             tree(name, path) AS (
                 SELECT name, printf('%020d', rank) FROM ranked WHERE parent IS NULL
                 UNION ALL
                 SELECT s.name, t.path || '/' || printf('%020d', s.rank)
                 FROM ranked s JOIN tree t ON s.parent = t.name
             ),
             active_files AS (
                 SELECT space, count(*) AS total FROM files WHERE status = 'active' GROUP BY space
             )
             SELECT s.name, s.label, s.parent, s.position,
                    (SELECT count(*) FROM facts WHERE space = s.name AND status = 'active'),
                    coalesce(f.total, 0)
             FROM tree t JOIN spaces s USING (name)
             LEFT JOIN active_files f ON f.space = s.name ORDER BY t.path",
        )?;
        let spaces = stmt
            .query_map([], |row| {
                Ok(SpaceInfo {
                    name: row.get(0)?,
                    label: row.get(1)?,
                    parent: row.get(2)?,
                    position: row.get(3)?,
                    notes: row.get(4)?,
                    files: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(spaces)
    }
}

pub(crate) fn require_space(conn: &Connection, name: &str) -> Result<(), StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM spaces WHERE name = ?1)",
        [name],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

fn check_cycle(conn: &Connection, name: &str, parent: &str) -> Result<(), StoreError> {
    let cycle = conn.query_row(
        "WITH RECURSIVE ancestors(name, parent) AS (
             SELECT name, parent FROM spaces WHERE name = ?1
             UNION
             SELECT s.name, s.parent FROM spaces s JOIN ancestors a ON s.name = a.parent
         )
         SELECT EXISTS(SELECT 1 FROM ancestors WHERE name = ?2)",
        (parent, name),
        |row| row.get::<_, bool>(0),
    )?;
    if cycle {
        return Err(StoreError::Invalid(
            "A space cannot be its own ancestor".into(),
        ));
    }
    Ok(())
}
