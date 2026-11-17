//! The configuration of the spaces, a data file of the node: `spaces.json`,
//! format `kalios.spaces/1` (`docs/schema/kalios.spaces-1.md`). Written
//! whole and atomically, in the node's data folder, never in a synchronised
//! one. No secret: they stay in the platform's keychain.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::rules::{Change, Rule};
use crate::space::{DEFAULT_SPACE, Space, Tree};

pub const SPACES_FORMAT: &str = "kalios.spaces/1";
/// The label the default space had before it became the Inbox; a tree that still holds it is read as « Inbox ».
pub const FORMER_DEFAULT_LABEL: &str = "À trier";

#[derive(Debug, Serialize, Deserialize)]
struct File {
    format: String,
    spaces: Vec<Space>,
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    changes: Vec<Change>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Unreadable(String),
    Invalid(String),
}

/// The tree read from `text`, with the default space added when missing.
pub fn parse_spaces(text: &str, default_name: &str) -> Result<Tree, ConfigError> {
    let file: File =
        serde_json::from_str(text).map_err(|e| ConfigError::Unreadable(e.to_string()))?;
    if file.format != SPACES_FORMAT {
        return Err(ConfigError::Unreadable(format!("format {}", file.format)));
    }
    let mut spaces = file.spaces;
    // The default space was labelled « À trier » before it became the Inbox: a tree saved then shows the new label.
    // Only the label changes; its id and its path stay, so nothing else moves.
    for s in spaces.iter_mut() {
        if s.id == DEFAULT_SPACE && s.name == FORMER_DEFAULT_LABEL {
            s.name = "Inbox".into();
        }
    }
    if !spaces.iter().any(|s| s.id == DEFAULT_SPACE) {
        spaces.insert(0, Space::default_notes(default_name));
    }
    let mut tree = Tree::new(spaces).map_err(|e| ConfigError::Invalid(format!("{e:?}")))?;
    for r in file.rules {
        tree.put_rule(r);
    }
    tree.changes = file.changes;
    Ok(tree)
}

/// The file's text, keys in a stable order, readable by a person.
pub fn spaces_text(tree: &Tree) -> String {
    let file = File {
        format: SPACES_FORMAT.into(),
        spaces: tree.spaces().to_vec(),
        rules: tree.rules.clone(),
        changes: tree.changes.clone(),
    };
    serde_json::to_string_pretty(&file).unwrap_or_default() + "\n"
}

/// Reads the file; a missing file is a tree with the default space only.
pub fn load_spaces(path: &Path, default_name: &str) -> Result<Tree, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => parse_spaces(&text, default_name),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Tree::new(vec![Space::default_notes(default_name)])
                .map_err(|e| ConfigError::Invalid(format!("{e:?}")))
        }
        Err(e) => Err(ConfigError::Io(e)),
    }
}

/// Writes the file whole: a temporary file beside it, then a rename.
pub fn save_spaces(path: &Path, tree: &Tree) -> Result<(), ConfigError> {
    let tmp = path.with_extension(format!("json.{}.tmp", crate::ids::new_id()));
    fs::write(&tmp, spaces_text(tree)).map_err(ConfigError::Io)?;
    fs::rename(&tmp, path).map_err(ConfigError::Io)
}
