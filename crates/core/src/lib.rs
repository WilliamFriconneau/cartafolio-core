#![warn(missing_docs)]
//! memcp-core: the memory itself. Dated, sourced facts stored in
//! isolated spaces (one SQLite file each) plus a control database for
//! connections and rights. No model inside, no network.
//!
//! The map of the crate is `docs/CORE-API.md`; the MCP tools are in `docs/TOOLS.md`.
//!
//! # Example: remember, recall, supersede, retire
//!
//! ```
//! use memcp_core::facts::{self, NewFact, RecallOptions, Remembered};
//! use memcp_core::text::Language;
//! use memcp_core::{Brain, Space};
//!
//! # let dir = std::env::temp_dir().join(format!("memcp-doc-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&dir);
//! let home = Space::new("home")?;
//! let brain = Brain::init(&dir, &[home.clone()], Language::English)?;
//! let conn = brain.open_space(&home)?;
//!
//! // Write a fact (the author is the connection's name).
//! let Remembered::Created(fact) =
//!     facts::remember(&conn, &home, "me", NewFact::agent("2026-11-16, boiler serviced in March.", "agent:me"), &[])?
//! else { panic!("expected a new fact") };
//!
//! // Find it by words.
//! let hits = facts::recall(&conn, "boiler", &RecallOptions::default(), 1_900_000_000)?;
//! assert_eq!(hits[0].fact.id, fact.id);
//!
//! // Correct it: the old version stays readable, as `replaced`.
//! let newer = facts::supersede(&conn, &home, "me", &fact.id, NewFact::agent("2026-11-16, boiler serviced in April.", "agent:me"), Some("wrong month"))?;
//! assert_eq!(facts::read_fact(&conn, &fact.id)?.map(|f| f.status), Some("replaced".into()));
//!
//! // Retire it: a reason is required, nothing is erased.
//! let retired = facts::retire(&conn, "me", &newer.id, "sold the boiler")?;
//! assert_eq!(retired.status, "retired");
//! assert_eq!(facts::history(&conn, &newer.id)?.len(), 2);
//! # drop(conn);
//! # drop(brain);
//! # let _ = std::fs::remove_dir_all(&dir);
//! # Ok::<(), memcp_core::Error>(())
//! ```

pub mod access;
pub mod autotag;
mod brain;
pub mod call;
pub mod catalogue;
pub mod directory;
mod error;
pub mod export;
pub mod facts;
pub mod files;
pub mod folders;
pub mod fsx;
pub mod guide;
pub mod import;
pub mod layout;
pub mod library;
pub mod lifecycle;
pub mod merge;
#[cfg(test)]
mod migration_tests;
mod moves;
pub mod origin;
pub mod pairing;
pub mod peers;
pub mod people;
pub mod policy;
pub mod rules;
mod schema;
mod space;
pub mod swarm;
pub mod sync;
pub mod text;
pub mod tools;
#[cfg(test)]
mod tools_tests;
pub mod transcription;
pub mod triage;
pub mod users;
/// Clock, random bytes and small helpers shared by the other modules.
pub mod util;
pub mod vocabulary;

pub use brain::{Brain, default_config};
pub use error::{Error, ErrorKind, Result};
pub use moves::Moved;
pub use schema::{CONTROL_SCHEMA_VERSION, SPACE_SCHEMA_VERSION};
pub use space::Space;
