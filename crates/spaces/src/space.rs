//! A space and the tree of spaces.

use serde::{Deserialize, Serialize};

use crate::connector::ConnectorConfig;
use crate::rules::{AssistantLevel, Change, Rule};

/// The id of the default space, always there: it gets what nothing decided.
pub const DEFAULT_SPACE: &str = "notes";

/// A destination set by the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    /// Stable id.
    pub id: String,
    /// Shown as is.
    pub name: String,
    /// Place in the tree, `/`-separated (`travail/client-a`); its parent is `travail`.
    pub path: String,
    /// How far an assistant may change this space's rules (words and tags).
    #[serde(default)]
    pub assistant_level: AssistantLevel,
    /// Health by default: wins over the others, never sent to a remote model.
    #[serde(default)]
    pub sensitive: bool,
    /// For a sensitive space: the spaces that may get its notes too (empty by
    /// default: its notes go only to it).
    #[serde(default)]
    pub shares_with: Vec<String>,
    /// Also receives the notes routed to its sub-spaces.
    #[serde(default)]
    pub includes_children: bool,
    /// Also sends Apple Health data; off by default.
    #[serde(default)]
    pub sends_apple_health: bool,
    #[serde(default)]
    pub connectors: Vec<ConnectorConfig>,
    /// Also writes to the calendar, to the reminders; keeps a state file (step 3).
    #[serde(default)]
    pub sends_calendar: bool,
    #[serde(default)]
    pub sends_reminders: bool,
    #[serde(default)]
    pub state_file: bool,
}

impl Space {
    /// The default space « Notes », with no connector: the app adds the
    /// folder chosen at first launch.
    pub fn default_notes(name: &str) -> Space {
        Space {
            id: DEFAULT_SPACE.into(),
            name: name.into(),
            path: DEFAULT_SPACE.into(),
            assistant_level: AssistantLevel::default(),
            sensitive: false,
            shares_with: vec![],
            includes_children: false,
            sends_apple_health: false,
            connectors: vec![],
            sends_calendar: false,
            sends_reminders: false,
            state_file: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceError {
    EmptyName,
    BadPath(String),
    DuplicatePath(String),
    DuplicateId(String),
    /// A sensitive space cannot go to iCloud Drive (rule 5.1.3(ii)).
    SensitiveOnIcloud,
    DuplicateConnector(String),
    NotFound(String),
    /// The default space cannot be deleted, nor merged away.
    DefaultSpace,
    /// A sensitive space is never swallowed by one that is not.
    SensitiveIntoOpen,
    /// A space cannot go under itself or into one of its own sub-spaces.
    IntoItself,
}

/// The spaces of the person, with at most one per path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub(crate) spaces: Vec<Space>,
    /// The words and tags of every space, shared by the nodes (`rules.rs`).
    pub(crate) rules: Vec<Rule>,
    /// What assistants changed or proposed, newest last.
    pub(crate) changes: Vec<Change>,
}

pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|p| !p.trim().is_empty() && p.trim() == p)
}

impl Tree {
    pub fn new(spaces: Vec<Space>) -> Result<Self, SpaceError> {
        let mut tree = Tree::default();
        for n in spaces {
            tree.add(n)?;
        }
        Ok(tree)
    }

    /// Checks a space alone, before it joins a tree.
    pub fn check(space: &Space) -> Result<(), SpaceError> {
        if space.name.trim().is_empty() {
            return Err(SpaceError::EmptyName);
        }
        if !valid_path(&space.path) {
            return Err(SpaceError::BadPath(space.path.clone()));
        }
        if space.sensitive && space.connectors.iter().any(|c| c.icloud) {
            return Err(SpaceError::SensitiveOnIcloud);
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(c) = space.connectors.iter().find(|c| !seen.insert(&c.id)) {
            return Err(SpaceError::DuplicateConnector(c.id.clone()));
        }
        Ok(())
    }

    pub fn add(&mut self, space: Space) -> Result<(), SpaceError> {
        Self::check(&space)?;
        if self.spaces.iter().any(|n| n.path == space.path) {
            return Err(SpaceError::DuplicatePath(space.path));
        }
        if self.spaces.iter().any(|n| n.id == space.id) {
            return Err(SpaceError::DuplicateId(space.id));
        }
        self.spaces.push(space);
        Ok(())
    }

    pub fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    pub fn get(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|n| n.id == id)
    }

    /// The spaces above `path`, nearest first (`a/b/c`: `a/b`, then `a`).
    pub fn ancestors(&self, path: &str) -> Vec<&Space> {
        let mut out = vec![];
        let mut rest = path;
        while let Some((parent, _)) = rest.rsplit_once('/') {
            if let Some(n) = self.spaces.iter().find(|n| n.path == parent) {
                out.push(n);
            }
            rest = parent;
        }
        out
    }
}
