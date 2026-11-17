//! The layout of a person's spaces: a label, a parent and a position.
//!
//! The *name* of a space never changes: it is the file on disk and what every right refers to.
//! What a person organises (rename, nest, reorder) is layout stored beside it, so none of it can
//! lose a fact or change a right. Rights do not follow the tree: a parent grants nothing on its
//! children (see docs/SPACES.md).

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use crate::brain::Brain;
use crate::catalogue::{CUSTOM, vault_name};
use crate::error::{Error, Result};
use crate::space::Space;
use crate::users::is_space_owner;

/// The deepest a tree of spaces may go.
pub const MAX_DEPTH: usize = 8;
/// The longest label, in characters.
pub const MAX_LABEL: usize = 64;

type Row = (String, Option<String>, Option<String>, i64, String);

/// A space as its owner sees it in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The space.
    pub space: Space,
    /// What is shown: the label, or the name when none was given.
    pub label: String,
    /// Its parent in the tree, `None` at the top. A parent the person does not own is not shown: the space is at the top.
    pub parent: Option<Space>,
    /// Order among its brothers (smaller first; ties by label).
    pub position: i64,
    /// The kind of vault (a key of the catalogue, or `custom`).
    pub kind: String,
}

/// What to change. `None` leaves a field as it is; `parent: Some(None)` moves to the top.
#[derive(Clone, Debug, Default)]
pub struct Change {
    /// New display label (`Some("")` is refused; it must not be only spaces).
    pub label: Option<String>,
    /// `Some(Some(p))` moves under `p`, `Some(None)` moves to the top, `None` leaves it. A move that would make a cycle is refused.
    pub parent: Option<Option<Space>>,
    /// New position among the brothers.
    pub position: Option<i64>,
}

pub(crate) fn clean_label(label: &str) -> Result<String> {
    let l = label.trim();
    let n = l.chars().count();
    if n == 0 || n > MAX_LABEL || l.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "a label is 1 to {MAX_LABEL} characters, without control characters"
        )));
    }
    Ok(l.to_string())
}

/// A lowercase ASCII slug of a label, for the name of a new space.
fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ñ' => 'n',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' | 'ý' => 'y',
            c => c,
        };
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || !out.starts_with(|c: char| c.is_ascii_lowercase()) {
        format!("s{out}")
    } else {
        out
    }
}

impl Brain {
    /// The spaces a person owns, in tree order: top level first by position, each followed by its
    /// children. A space whose parent is no longer theirs is shown at the top.
    pub fn layout(&self, user_id: &str) -> Result<Vec<Place>> {
        let ctl = self.open_control()?;
        let mut st = ctl.prepare(
            "SELECT s.name, s.label, s.parent, s.position, s.kind FROM spaces s
             JOIN space_members m ON m.space = s.name AND m.owner = 1 AND m.user_id = ?1",
        )?;
        let rows: Vec<Row> = st
            .query_map([user_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let owned: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let mut places = Vec::new();
        for (name, label, parent, position, kind) in &rows {
            let parent = match parent {
                Some(p) if owned.contains(&p.as_str()) => Some(Space::new(p)?),
                _ => None,
            };
            places.push(Place {
                space: Space::new(name)?,
                label: label.clone().unwrap_or_else(|| name.clone()),
                parent,
                position: *position,
                kind: kind.clone(),
            });
        }
        let mut by_parent: BTreeMap<Option<String>, Vec<Place>> = BTreeMap::new();
        for p in places {
            by_parent
                .entry(p.parent.as_ref().map(|s| s.as_str().to_string()))
                .or_default()
                .push(p);
        }
        for v in by_parent.values_mut() {
            v.sort_by(|a, b| {
                (a.position, a.label.to_lowercase(), a.space.as_str()).cmp(&(
                    b.position,
                    b.label.to_lowercase(),
                    b.space.as_str(),
                ))
            });
        }
        let mut out = Vec::new();
        fn walk(
            key: Option<String>,
            by: &BTreeMap<Option<String>, Vec<Place>>,
            out: &mut Vec<Place>,
            depth: usize,
        ) {
            if depth > MAX_DEPTH + 1 {
                return;
            }
            for p in by.get(&key).into_iter().flatten() {
                out.push(p.clone());
                walk(Some(p.space.as_str().to_string()), by, out, depth + 1);
            }
        }
        walk(None, &by_parent, &mut out, 0);
        Ok(out)
    }

    /// Renames, moves or reorders one of a person's spaces. Only its owner may; nothing but layout
    /// changes.
    pub fn set_layout(&self, user_id: &str, space: &Space, change: &Change) -> Result<()> {
        let ctl = self.open_control()?;
        if !is_space_owner(&ctl, space, user_id)? {
            return Err(Error::Invalid(format!(
                "{space} is not a space of this person"
            )));
        }
        let label = change.label.as_deref().map(clean_label).transpose()?;
        if let Some(Some(parent)) = &change.parent {
            if parent == space {
                return Err(Error::Invalid("a space cannot be its own parent".into()));
            }
            if !is_space_owner(&ctl, parent, user_id)? {
                return Err(Error::Invalid(format!(
                    "{parent} is not a space of this person"
                )));
            }
            let all = self.layout(user_id)?;
            let parent_of: BTreeMap<&str, Option<&str>> = all
                .iter()
                .map(|p| (p.space.as_str(), p.parent.as_ref().map(Space::as_str)))
                .collect();
            // No cycle: the new parent must not sit under the space.
            let mut up = Some(parent.as_str());
            let mut depth_of_parent = 0usize;
            while let Some(cur) = up {
                if cur == space.as_str() {
                    return Err(Error::Invalid(
                        "a space cannot go under one of its own children".into(),
                    ));
                }
                depth_of_parent += 1;
                up = parent_of.get(cur).copied().flatten();
                if depth_of_parent > MAX_DEPTH + 1 {
                    break;
                }
            }
            // Depth: the parent's chain, the space, and the height of what hangs under it.
            fn height(n: &str, all: &[crate::layout::Place]) -> usize {
                1 + all
                    .iter()
                    .filter(|p| p.parent.as_ref().map(Space::as_str) == Some(n))
                    .map(|p| height(p.space.as_str(), all))
                    .max()
                    .unwrap_or(0)
            }
            if depth_of_parent + height(space.as_str(), &all) > MAX_DEPTH {
                return Err(Error::Invalid(format!(
                    "a tree of spaces goes at most {MAX_DEPTH} deep"
                )));
            }
        }
        let tx =
            rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
        if let Some(l) = &label {
            tx.execute(
                "UPDATE spaces SET label = ?2 WHERE name = ?1",
                params![space.as_str(), l],
            )?;
        }
        if let Some(parent) = &change.parent {
            tx.execute(
                "UPDATE spaces SET parent = ?2 WHERE name = ?1",
                params![space.as_str(), parent.as_ref().map(Space::as_str)],
            )?;
        }
        if let Some(pos) = change.position {
            tx.execute(
                "UPDATE spaces SET position = ?2 WHERE name = ?1",
                params![space.as_str(), pos],
            )?;
        }
        tx.commit()?;
        let log = self.open_space(space)?;
        if let Some(l) = &label {
            crate::sync::record_layout(&log, "label", serde_json::json!(l))?;
        }
        if let Some(parent) = &change.parent {
            let v = parent.as_ref().map(Space::as_str);
            crate::sync::record_layout(&log, "parent", serde_json::json!(v))?;
        }
        if let Some(pos) = change.position {
            crate::sync::record_layout(&log, "position", serde_json::json!(pos))?;
        }
        Ok(())
    }

    /// Creates a space of a person's own, of kind `custom`, with a label and perhaps under a
    /// parent. Its name is made from the person and the label, and made unique.
    pub fn add_custom_space(
        &self,
        user_id: &str,
        person: &str,
        label: &str,
        parent: Option<&Space>,
    ) -> Result<Space> {
        let label = clean_label(label)?;
        if let Some(p) = parent {
            let ctl = self.open_control()?;
            if !is_space_owner(&ctl, p, user_id)? {
                return Err(Error::Invalid(format!("{p} is not a space of this person")));
            }
            let depth = {
                let all = self.layout(user_id)?;
                let mut d = 1usize;
                let mut cur = Some(p.as_str().to_string());
                while let Some(c) = cur {
                    d += 1;
                    cur = all
                        .iter()
                        .find(|x| x.space.as_str() == c)
                        .and_then(|x| x.parent.as_ref().map(|s| s.as_str().to_string()));
                    if d > MAX_DEPTH + 1 {
                        break;
                    }
                }
                d
            };
            if depth > MAX_DEPTH {
                return Err(Error::Invalid(format!(
                    "a tree of spaces goes at most {MAX_DEPTH} deep"
                )));
            }
        }
        let base: String = slug(&label).chars().take(20).collect();
        let mut space = None;
        for n in 1..=99 {
            let word = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let candidate = vault_name(person, word.trim_end_matches('-'))?;
            if !self.has_space(&candidate)? {
                space = Some(candidate);
                break;
            }
        }
        let space = space.ok_or_else(|| Error::Invalid("no free name for this space".into()))?;
        self.add_space_for(&space, user_id)?;
        self.set_space_kind(&space, CUSTOM)?;
        let ctl = self.open_control()?;
        let next: i64 = ctl
            .query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM spaces WHERE parent IS ?1",
                [parent.map(Space::as_str)],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1);
        ctl.execute(
            "UPDATE spaces SET label = ?2, parent = ?3, position = ?4 WHERE name = ?1",
            params![space.as_str(), label, parent.map(Space::as_str), next],
        )?;
        let log = self.open_space(&space)?;
        crate::sync::record_layout(&log, "label", serde_json::json!(label))?;
        if let Some(p) = parent {
            crate::sync::record_layout(&log, "parent", serde_json::json!(p.as_str()))?;
        }
        crate::sync::record_layout(&log, "position", serde_json::json!(next))?;
        Ok(space)
    }
}
