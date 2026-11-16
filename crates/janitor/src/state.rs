//! What the janitor remembers between two passes, in a small file of its own: how often filing a note failed, so that a
//! note the memory keeps refusing is left alone after a few tries. Losing this file loses nothing: the notes are tried
//! again.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub struct State {
    file: PathBuf,
    /// `{note_fails: {note_id: n}, learned: {vault: {...}}, alone: n, checks: n, keywords_fact: {vault: id}}`
    data: Value,
    /// Set when the file could not be written: said once by the pass.
    pub write_error: Option<String>,
}

impl State {
    pub fn open(dir: &Path) -> Result<State, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join("state.json");
        let data = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["note_fails"].is_object())
            .unwrap_or_else(|| json!({ "note_fails": {} }));
        Ok(State {
            file,
            data,
            write_error: None,
        })
    }

    fn save(&mut self) {
        let tmp = self.file.with_extension("tmp");
        let written = std::fs::write(&tmp, self.data.to_string())
            .and_then(|()| std::fs::rename(&tmp, &self.file));
        if let Err(e) = written {
            self.write_error = Some(format!("{}: {e}", self.file.display()));
        }
    }

    /// How many times filing this note failed after a model chose for it.
    pub fn note_fails(&self, note_id: &str) -> u64 {
        self.data["note_fails"][note_id].as_u64().unwrap_or(0)
    }

    /// What a vault has learned (see `learn.rs`); saved with [`State::saved`].
    pub fn learned(&mut self, vault: &str) -> crate::learn::Learned<'_> {
        if !self.data["learned"].is_object() {
            self.data["learned"] = json!({});
        }
        crate::learn::Learned {
            data: &mut self.data["learned"][vault],
        }
    }

    /// A counter of the state (`alone`, `checks`, `by_model`): its value after adding `by`.
    pub fn count(&mut self, key: &str, by: u64) -> u64 {
        let n = self.data[key].as_u64().unwrap_or(0) + by;
        self.data[key] = json!(n);
        n
    }

    /// The fact that holds the night's note of a vault, if one was written.
    pub fn keywords_fact(&self, vault: &str) -> Option<String> {
        self.data["keywords_fact"][vault].as_str().map(String::from)
    }

    pub fn set_keywords_fact(&mut self, vault: &str, id: &str) {
        self.data["keywords_fact"][vault] = json!(id);
        self.save();
    }

    /// The note the janitor keeps for `kind` and `space` (`waiting` in « À trier »), if one was written.
    pub fn note_id(&self, kind: &str, space: &str) -> Option<String> {
        self.data["note_ids"][kind][space].as_str().map(String::from)
    }

    /// Remembers the note of `kind` and `space`, or forgets it with `None`.
    pub fn set_note_id(&mut self, kind: &str, space: &str, id: Option<&str>) {
        self.data["note_ids"][kind][space] = id.map_or(Value::Null, |i| json!(i));
        self.save();
    }

    /// When (seconds) something was last done, 0 if never.
    pub fn stamp(&self, key: &str) -> u64 {
        self.data["stamps"][key].as_u64().unwrap_or(0)
    }

    pub fn set_stamp(&mut self, key: &str, at: u64) {
        self.data["stamps"][key] = json!(at);
        self.save();
    }

    /// Saves what changed.
    pub fn saved(&mut self) {
        self.save();
    }

    pub fn note_failed(&mut self, note_id: &str) {
        let n = self.note_fails(note_id) + 1;
        self.data["note_fails"][note_id] = json!(n);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::State;

    #[test]
    fn failures_are_counted_and_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("memcp-janitor-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = State::open(&dir).expect("open");
        s.note_failed("n1");
        s.note_failed("n1");
        assert_eq!(State::open(&dir).expect("reopen").note_fails("n1"), 2);
        assert_eq!(s.note_fails("n2"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
