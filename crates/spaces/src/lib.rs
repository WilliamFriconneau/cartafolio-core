//! Spaces: where a note goes. A space has a name, a place in a tree
//! (`travail/client-a`), its unique words (they decide alone), its ordinary
//! words (they only suggest), whether it is sensitive, and its connectors.
//! The router reads the note's text on the device and says where it goes
//! (`docs/specs/2026-11-16-espaces-design.md`). No iOS, no health: the
//! caller says whether a note is a distress note.

mod config;
mod connector;
mod ids;
mod markdown;
mod normalize;
mod organize;
mod outbox;
mod router;
mod rules;
mod sorting;
mod space;
mod sync;
pub mod ui;

pub use config::{ConfigError, SPACES_FORMAT, load_spaces, parse_spaces, save_spaces, spaces_text};
pub use connector::{ConnectorConfig, ConnectorKind, Mode};
pub use ids::{Hlc, new_id, now_ms, uuid_v7_at};
pub use markdown::{MarkdownFile, file_name, note_file};
pub use normalize::normalize;
pub use outbox::{Planned, plan, retry_after_seconds};
pub use router::{Routing, route, shared_word_spaces, word_also_in};
pub use rules::{AssistantLevel, Change, ChangeState, DAILY_CHANGES, Rule, RuleKind};
pub use sorting::{LEASE_MS, Lease, lease_holder, may_sort};
pub use space::{DEFAULT_SPACE, Space, SpaceError, Tree};
pub use sync::{
    WriteDecision, conflict_name, content_hash, is_conflict_copy, is_ignored, write_decision,
};
