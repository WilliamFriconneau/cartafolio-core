#![warn(missing_docs)]
//! pensine-core: the memory itself. Dated, sourced facts stored in
//! isolated spaces (one SQLite file each) plus a control database for
//! connections and rights. No model inside, no network.
//!
//! The map of the crate is `docs/CORE-API.md`; the MCP tools are in `docs/TOOLS.md`.
//!
//! # Example: remember, recall, supersede, retire
//!
//! ```
//! use pensine_core::facts::{self, NewFact, RecallOptions, Remembered};
//! use pensine_core::text::Language;
//! use pensine_core::{Brain, Space};
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
//! # Ok::<(), pensine_core::Error>(())
//! ```

pub mod access;
pub mod autotag;
mod brain;
pub mod call;
pub mod catalogue;
pub mod chunks;
pub mod directory;
mod error;
pub mod export;
pub mod facts;
#[doc(hidden)]
pub mod failpoint;
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
pub mod moves;
pub mod origin;
pub mod pairing;
pub mod peers;
pub mod people;
pub mod policy;
pub mod recover;
pub mod retag;
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
pub mod view;
pub mod vocabulary;

pub use brain::{Brain, default_config};
pub use error::{Error, ErrorKind, Result};
pub use moves::Moved;
pub use recover::Recovered;
pub use schema::{CONTROL_SCHEMA_VERSION, SPACE_SCHEMA_VERSION};
pub use space::Space;

/// Environment variable aliases for Pensine.
pub mod env {
    use std::env::{self, VarError};
    use std::ffi::OsString;

    fn lookup(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
        let prefixes = ["PENSINE_", "SILICE_", "MEMCP_"];
        if let Some(suffix) = prefixes.iter().find_map(|prefix| name.strip_prefix(prefix)) {
            prefixes.iter().find_map(|prefix| {
                get(&format!("{prefix}{suffix}")).filter(|value| !value.is_empty())
            })
        } else {
            get(name)
        }
    }

    /// Reads the first nonempty value in PENSINE_, SILICE_, MEMCP_ order.
    pub fn var_os(name: &str) -> Option<OsString> {
        lookup(name, |key| env::var_os(key))
    }

    /// Reads a Unicode variable with the same precedence as [var_os].
    pub fn var(name: &str) -> Result<String, VarError> {
        var_os(name)
            .ok_or(VarError::NotPresent)?
            .into_string()
            .map_err(VarError::NotUnicode)
    }

    /// Sets all three spellings on a child command.
    pub fn set_child<'a>(
        command: &'a mut std::process::Command,
        suffix: &str,
        value: impl AsRef<std::ffi::OsStr>,
    ) -> &'a mut std::process::Command {
        command
            .env(format!("MEMCP_{suffix}"), value.as_ref())
            .env(format!("SILICE_{suffix}"), value.as_ref())
            .env(format!("PENSINE_{suffix}"), value.as_ref())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn environment_alias_precedence() {
            for (pensine, silice, memcp, expected) in [
                (Some("new"), Some("middle"), Some("old"), Some("new")),
                (None, Some("middle"), Some("old"), Some("middle")),
                (Some(""), Some("middle"), Some("old"), Some("middle")),
                (None, Some(""), Some("old"), Some("old")),
                (None, None, Some("old"), Some("old")),
                (Some(""), None, Some(""), None),
            ] {
                for name in ["PENSINE_TEST", "SILICE_TEST", "MEMCP_TEST"] {
                    assert_eq!(
                        lookup(name, |key| match key {
                            "PENSINE_TEST" => pensine.map(Into::into),
                            "SILICE_TEST" => silice.map(Into::into),
                            "MEMCP_TEST" => memcp.map(Into::into),
                            _ => None,
                        }),
                        expected.map(Into::into),
                    );
                }
            }
        }
    }
}

/// The command name for messages, based on the filename used to invoke the program.
pub fn program_name(
    argv0: &std::path::Path,
    legacy: &'static str,
    silice: &'static str,
    pensine: &'static str,
) -> &'static str {
    let name = argv0
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if name.starts_with("pensine") {
        pensine
    } else if name.starts_with("silice") {
        silice
    } else {
        legacy
    }
}

/// The command name for the current process.
pub fn invoked_as(
    legacy: &'static str,
    silice: &'static str,
    pensine: &'static str,
) -> &'static str {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    program_name(std::path::Path::new(&argv0), legacy, silice, pensine)
}

#[cfg(test)]
mod program_name_tests {
    use super::*;

    #[test]
    fn invocation_uses_the_filename_prefix() {
        for (legacy, silice, pensine) in [
            ("memcp", "silice", "pensine"),
            ("pensine-manager", "silice-manager", "pensine-manager"),
            ("memcp-sync", "silice-sync", "pensine-sync"),
        ] {
            for (argv0, expected) in [
                (format!("/bin/{pensine}.exe"), pensine),
                ("PENSINE-custom.EXE".into(), pensine),
                (format!("/bin/{}.EXE", silice.to_ascii_uppercase()), silice),
                ("silice-custom".into(), silice),
                (format!("/pensine/silice/{legacy}"), legacy),
                ("renamed".into(), legacy),
                (String::new(), legacy),
            ] {
                assert_eq!(
                    program_name(argv0.as_ref(), legacy, silice, pensine),
                    expected
                );
            }
        }
    }
}
