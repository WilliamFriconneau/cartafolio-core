//! The person organises her spaces at any time, and no note is lost:
//! create, change, rename, move in the tree, reorder, merge, delete.
//! The notes follow in the app's store; here, the tree.

use crate::connector::ConnectorConfig;
use crate::space::{DEFAULT_SPACE, Space, SpaceError, Tree};

fn under(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

impl Tree {
    fn index(&self, id: &str) -> Result<usize, SpaceError> {
        self.spaces
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| SpaceError::NotFound(id.into()))
    }

    /// The space and all those below it, in the tree's order.
    pub fn subtree(&self, id: &str) -> Vec<&Space> {
        let Some(root) = self.get(id) else {
            return vec![];
        };
        self.spaces
            .iter()
            .filter(|s| under(&s.path, &root.path))
            .collect()
    }

    /// The spaces whose notes are seen from `id`: itself and its sub-spaces,
    /// except a sensitive one below a space that is not (it stays apart).
    pub fn seen_from(&self, id: &str) -> Vec<String> {
        let Some(root) = self.get(id) else {
            return vec![];
        };
        self.subtree(id)
            .into_iter()
            .filter(|s| s.id == root.id || root.sensitive || !s.sensitive)
            .map(|s| s.id.clone())
            .collect()
    }

    /// Adds a space, or replaces the one with the same id (same place in the order).
    pub fn save(&mut self, space: Space) -> Result<(), SpaceError> {
        Self::check(&space)?;
        if self
            .spaces
            .iter()
            .any(|s| s.path == space.path && s.id != space.id)
        {
            return Err(SpaceError::DuplicatePath(space.path));
        }
        match self.spaces.iter().position(|s| s.id == space.id) {
            Some(i) => self.spaces[i] = space,
            None => self.spaces.push(space),
        }
        Ok(())
    }

    /// The delivering connectors of `id` and of its sub-spaces: their place
    /// changes with a rename or a move (the app asks first; what was sent stays).
    fn moved_connectors(&self, id: &str) -> Vec<ConnectorConfig> {
        self.subtree(id)
            .into_iter()
            .flat_map(|s| s.connectors.iter().filter(|c| c.delivers()).cloned())
            .collect()
    }

    /// Renames a space: its name and the last part of its path, for it and its
    /// sub-spaces. Returns the connectors whose place changes.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<Vec<ConnectorConfig>, SpaceError> {
        let i = self.index(id)?;
        if name.trim().is_empty() {
            return Err(SpaceError::EmptyName);
        }
        let old = self.spaces[i].path.clone();
        let parent = old.rsplit_once('/').map(|(p, _)| p);
        let leaf = crate::normalize::normalize(name).join("-");
        let leaf = if leaf.is_empty() {
            self.spaces[i].id.clone()
        } else {
            leaf
        };
        let new = match parent {
            Some(p) => format!("{p}/{leaf}"),
            None => leaf,
        };
        self.spaces[i].name = name.trim().into();
        if new == old {
            return Ok(vec![]);
        }
        self.move_to(id, &new)
    }

    /// Moves a space and its sub-spaces to `new_path` (any depth). Returns the
    /// connectors whose place changes.
    pub fn move_to(
        &mut self,
        id: &str,
        new_path: &str,
    ) -> Result<Vec<ConnectorConfig>, SpaceError> {
        let i = self.index(id)?;
        let old = self.spaces[i].path.clone();
        if !crate::space::valid_path(new_path) {
            return Err(SpaceError::BadPath(new_path.into()));
        }
        if new_path == old {
            return Ok(vec![]);
        }
        if under(new_path, &old) {
            return Err(SpaceError::IntoItself);
        }
        let moving: Vec<String> = self.subtree(id).iter().map(|s| s.id.clone()).collect();
        // A sensitive space never goes under an open space that takes its sub-spaces' notes.
        let has_sensitive = self.subtree(id).iter().any(|s| s.sensitive);
        if has_sensitive
            && self
                .ancestors(new_path)
                .iter()
                .any(|p| !p.sensitive && p.includes_children)
        {
            return Err(SpaceError::SensitiveIntoOpen);
        }
        let renamed = |p: &str| format!("{new_path}{}", &p[old.len()..]);
        for s in &self.spaces {
            if !moving.contains(&s.id)
                && moving
                    .iter()
                    .any(|m| self.get(m).is_some_and(|x| renamed(&x.path) == s.path))
            {
                return Err(SpaceError::DuplicatePath(s.path.clone()));
            }
        }
        let connectors = self.moved_connectors(id);
        for s in self.spaces.iter_mut().filter(|s| moving.contains(&s.id)) {
            s.path = renamed(&s.path);
        }
        Ok(connectors)
    }

    /// Puts the spaces in this order; those not named keep theirs, after.
    pub fn reorder(&mut self, ids: &[String]) {
        let mut ordered: Vec<Space> = ids.iter().filter_map(|id| self.get(id).cloned()).collect();
        ordered.dedup_by(|a, b| a.id == b.id);
        let rest: Vec<Space> = self
            .spaces
            .iter()
            .filter(|s| !ordered.iter().any(|o| o.id == s.id))
            .cloned()
            .collect();
        ordered.extend(rest);
        self.spaces = ordered;
    }

    fn can_receive(&self, from: &str, into: &str) -> Result<(), SpaceError> {
        let f = self
            .get(from)
            .ok_or_else(|| SpaceError::NotFound(from.into()))?;
        let t = self
            .get(into)
            .ok_or_else(|| SpaceError::NotFound(into.into()))?;
        if f.id == t.id || under(&t.path, &f.path) {
            return Err(SpaceError::IntoItself);
        }
        if f.sensitive && !t.sensitive {
            return Err(SpaceError::SensitiveIntoOpen);
        }
        Ok(())
    }

    /// Removes `id` and gives what it had to `into`: its sub-spaces move
    /// under `into`, its words join `into`'s. The store moves the notes.
    fn absorb(&mut self, id: &str, into: &str) -> Result<(), SpaceError> {
        self.can_receive(id, into)?;
        let old = self.get(id).map(|s| s.path.clone()).unwrap_or_default();
        let target = self.get(into).map(|t| t.path.clone()).unwrap_or_default();
        let below: Vec<String> = self
            .spaces
            .iter()
            .filter(|s| s.path != old && under(&s.path, &old))
            .map(|s| s.path.clone())
            .collect();
        if let Some(taken) = below
            .iter()
            .map(|p| format!("{target}{}", &p[old.len()..]))
            .find(|n| self.spaces.iter().any(|s| &s.path == n))
        {
            return Err(SpaceError::DuplicatePath(taken));
        }
        if self.subtree(id).iter().any(|s| s.sensitive)
            && self
                .ancestors(&format!("{target}/x"))
                .iter()
                .any(|p| !p.sensitive && p.includes_children)
        {
            return Err(SpaceError::SensitiveIntoOpen);
        }
        for s in self
            .spaces
            .iter_mut()
            .filter(|s| s.path != old && under(&s.path, &old))
        {
            s.path = format!("{target}{}", &s.path[old.len()..]);
        }
        let i = self.index(id)?;
        self.spaces.remove(i);
        Ok(())
    }

    /// Merges `from` into `into`: nothing is lost (notes, sub-spaces, words
    /// and tags); `from` disappears. `stamp` gives a clock per rule moved.
    pub fn merge(
        &mut self,
        from: &str,
        into: &str,
        stamp: impl FnMut() -> String,
    ) -> Result<(), SpaceError> {
        if from == DEFAULT_SPACE {
            return Err(SpaceError::DefaultSpace);
        }
        self.absorb(from, into)?;
        self.hand_rules(from, Some(into), stamp);
        Ok(())
    }

    /// Deletes `id`; its notes and its sub-spaces go to `notes_to`, its words
    /// and tags are retired.
    pub fn delete(
        &mut self,
        id: &str,
        notes_to: &str,
        stamp: impl FnMut() -> String,
    ) -> Result<(), SpaceError> {
        if id == DEFAULT_SPACE {
            return Err(SpaceError::DefaultSpace);
        }
        self.absorb(id, notes_to)?;
        self.hand_rules(id, None, stamp);
        Ok(())
    }
}
