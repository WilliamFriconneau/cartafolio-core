//! What leaves for a kept note: one delivery per (note, space, connector),
//! with a key so that sending again never makes a duplicate. The app keeps
//! them (SQLite on the phone) and sends them; the Mac bridge can keep them
//! its own way.

use crate::space::Tree;

/// One delivery to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub space_id: String,
    pub connector_id: String,
    /// `note/space/connector`: the same key again is the same delivery.
    pub key: String,
}

/// The deliveries of `note_id` toward `spaces`; none for a distress note,
/// whatever was asked (law S1). Unknown spaces and local connectors give none.
pub fn plan(tree: &Tree, note_id: &str, spaces: &[String], distress: bool) -> Vec<Planned> {
    if distress {
        return vec![];
    }
    let mut out = vec![];
    for id in spaces {
        let Some(n) = tree.get(id) else { continue };
        for c in n.connectors.iter().filter(|c| c.delivers()) {
            let key = format!("{note_id}/{}/{}", n.id, c.id);
            if !out.iter().any(|p: &Planned| p.key == key) {
                out.push(Planned {
                    space_id: n.id.clone(),
                    connector_id: c.id.clone(),
                    key,
                });
            }
        }
    }
    out
}

/// How long to wait before the next try, after `attempts` failures: one
/// minute, doubling, at most six hours.
pub fn retry_after_seconds(attempts: u32) -> u64 {
    (60u64 << attempts.min(9)).min(6 * 3600)
}
