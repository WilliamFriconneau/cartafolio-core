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
pub mod profession;
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

/// Environment variable aliases for Silice.
pub mod env {
    use std::env::{self, VarError};
    use std::ffi::OsString;

    fn lookup(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
        if let Some(suffix) = name
            .strip_prefix("MEMCP_")
            .or_else(|| name.strip_prefix("SILICE_"))
        {
            get(&format!("SILICE_{suffix}")).or_else(|| get(&format!("MEMCP_{suffix}")))
        } else {
            get(name)
        }
    }

    /// Reads a variable, preferring its `SILICE_` spelling over `MEMCP_`.
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

    /// Sets both spellings on a child command.
    pub fn set_child<'a>(
        command: &'a mut std::process::Command,
        suffix: &str,
        value: impl AsRef<std::ffi::OsStr>,
    ) -> &'a mut std::process::Command {
        command
            .env(format!("MEMCP_{suffix}"), value.as_ref())
            .env(format!("SILICE_{suffix}"), value.as_ref())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn silice_environment_aliases() {
            for (new, old, expected) in [
                (Some("new"), Some("old"), "new"),
                (Some("new"), None, "new"),
                (None, Some("old"), "old"),
                (Some(""), Some("old"), ""),
            ] {
                for name in ["MEMCP_TEST", "SILICE_TEST"] {
                    assert_eq!(
                        lookup(name, |key| match key {
                            "SILICE_TEST" => new.map(Into::into),
                            "MEMCP_TEST" => old.map(Into::into),
                            _ => None,
                        }),
                        Some(expected.into()),
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
    alias: &'static str,
) -> &'static str {
    if argv0
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("silice"))
    {
        alias
    } else {
        legacy
    }
}

/// The command name for the current process.
pub fn invoked_as(legacy: &'static str, alias: &'static str) -> &'static str {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    program_name(std::path::Path::new(&argv0), legacy, alias)
}

#[cfg(test)]
mod program_name_tests {
    use super::*;

    #[test]
    fn silice_invocation_uses_the_filename_prefix() {
        for (legacy, alias) in [
            ("memcp", "silice"),
            ("memcp-manager", "silice-manager"),
            ("memcp-sync", "silice-sync"),
        ] {
            for (argv0, expected) in [
                (format!("/bin/{alias}.exe"), alias),
                ("silice-custom".into(), alias),
                (format!("/silice/{legacy}"), legacy),
                ("renamed".into(), legacy),
                (String::new(), legacy),
            ] {
                assert_eq!(program_name(argv0.as_ref(), legacy, alias), expected);
            }
        }
    }
}
