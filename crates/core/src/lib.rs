//! memcp-core: the memory itself. Dated, sourced facts stored in
//! isolated spaces (one SQLite file each) plus a control database for
//! connections and rights. No model inside, no network.

pub mod access;
mod brain;
pub mod catalogue;
mod error;
pub mod export;
pub mod facts;
pub mod files;
pub mod fsx;
pub mod guide;
pub mod import;
pub mod layout;
pub mod library;
pub mod merge;
#[cfg(test)]
mod migration_tests;
mod moves;
pub mod origin;
pub mod pairing;
mod schema;
mod space;
pub mod sync;
pub mod text;
pub mod tools;
#[cfg(test)]
mod tools_tests;
pub mod triage;
pub mod users;
pub mod util;
pub mod vocabulary;

pub use brain::{Brain, default_config};
pub use error::{Error, Result};
pub use moves::Moved;
pub use schema::{CONTROL_SCHEMA_VERSION, SPACE_SCHEMA_VERSION};
pub use space::Space;
