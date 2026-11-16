use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    /// `init` on a directory that already holds memcp files.
    AlreadyInitialized(PathBuf),
    /// `open` on a directory without a control database.
    NotInitialized(PathBuf),
    /// Data directory readable by users other than its owner.
    DirectoryTooOpen {
        path: PathBuf,
        mode: u32,
    },
    /// Inconsistent database: wrong declared space, newer than the program.
    Schema(String),
    /// Request refused because it is malformed or breaks a rule.
    Invalid(String),
    /// Unknown fact identifier (or one from another space: the answer is the same).
    UnknownFact(String),
    /// Unknown space (or one the caller may not see: the answer is the same).
    UnknownSpace(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "file error: {e}"),
            Error::Sqlite(e) => write!(f, "SQLite error: {e}"),
            Error::AlreadyInitialized(p) => {
                write!(
                    f,
                    "{} is already initialised, nothing was changed",
                    p.display()
                )
            }
            Error::NotInitialized(p) => {
                write!(f, "{} is not initialised (run `memcp init`)", p.display())
            }
            Error::DirectoryTooOpen { path, mode } => write!(
                f,
                "{} is too open (mode {mode:04o}): it must be 0700",
                path.display()
            ),
            Error::Schema(m) => write!(f, "inconsistent database: {m}"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::UnknownFact(id) => write!(f, "unknown fact: {id}"),
            Error::UnknownSpace(s) => write!(f, "unknown space: {s}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}
