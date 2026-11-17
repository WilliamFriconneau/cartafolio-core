use std::fmt;
use std::path::PathBuf;

/// Every failure of the core. Servers map each variant to one HTTP status and one
/// MCP text (see `docs/CORE-API.md`, "Error vocabulary", and [`Error::kind`]).
#[derive(Debug)]
pub enum Error {
    /// A file or folder could not be read, written or created.
    Io(std::io::Error),
    /// SQLite refused a statement or could not open a file.
    Sqlite(rusqlite::Error),
    /// `init` on a directory that already holds memcp files.
    AlreadyInitialized(PathBuf),
    /// `open` on a directory without a control database.
    NotInitialized(PathBuf),
    /// Data directory readable by users other than its owner.
    DirectoryTooOpen {
        /// The folder that is too open.
        path: PathBuf,
        /// Its Unix permission bits.
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

/// The closed vocabulary of failure kinds shared by the core, the HTTP routes and the MCP tools.
///
/// Every [`Error`] and every [`crate::tools::ToolError`] has exactly one kind
/// ([`Error::kind`], [`crate::tools::ToolError::kind`]); a kind has one HTTP status and one
/// stable code. The table is in `docs/CORE-API.md` and a test keeps the two in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The request is malformed or breaks a rule of the memory. HTTP 400.
    Invalid,
    /// No valid credential was presented. HTTP 401.
    Unauthorized,
    /// A valid credential without the right needed on that space or action. HTTP 403.
    Denied,
    /// The fact, space, file or peer does not exist (or is not visible to the caller). HTTP 404.
    NotFound,
    /// The request is valid but clashes with the current state (already initialised, already used). HTTP 409.
    Conflict,
    /// The body is larger than the endpoint accepts. HTTP 413.
    TooLarge,
    /// A quota, a length limit or a rate limit was reached. HTTP 429.
    Limited,
    /// A bug or a storage failure on the node; the caller can do nothing. HTTP 500.
    Internal,
    /// The node is not ready (not initialised, shutting down). HTTP 503.
    Unavailable,
    /// The tool name is not one this connection sees. A JSON-RPC error `-32602`, not an HTTP failure.
    UnknownTool,
}

impl ErrorKind {
    /// Every kind, in table order.
    pub const ALL: [ErrorKind; 10] = [
        ErrorKind::Invalid,
        ErrorKind::Unauthorized,
        ErrorKind::Denied,
        ErrorKind::NotFound,
        ErrorKind::Conflict,
        ErrorKind::TooLarge,
        ErrorKind::Limited,
        ErrorKind::Internal,
        ErrorKind::Unavailable,
        ErrorKind::UnknownTool,
    ];

    /// The HTTP status a route answers with (`200` for [`ErrorKind::UnknownTool`], which
    /// travels as a JSON-RPC error inside a normal response).
    pub fn http_status(self) -> u16 {
        match self {
            ErrorKind::Invalid => 400,
            ErrorKind::Unauthorized => 401,
            ErrorKind::Denied => 403,
            ErrorKind::NotFound => 404,
            ErrorKind::Conflict => 409,
            ErrorKind::TooLarge => 413,
            ErrorKind::Limited => 429,
            ErrorKind::Internal => 500,
            ErrorKind::Unavailable => 503,
            ErrorKind::UnknownTool => 200,
        }
    }

    /// The stable lowercase code of the kind (`"invalid"`, `"not_found"`...).
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::Invalid => "invalid",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Denied => "denied",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::TooLarge => "too_large",
            ErrorKind::Limited => "limited",
            ErrorKind::Internal => "internal",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::UnknownTool => "unknown_tool",
        }
    }
}

impl Error {
    /// The kind of this failure (see [`ErrorKind`]).
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Io(_) | Error::Sqlite(_) | Error::Schema(_) | Error::DirectoryTooOpen { .. } => {
                ErrorKind::Internal
            }
            Error::AlreadyInitialized(_) => ErrorKind::Conflict,
            Error::NotInitialized(_) => ErrorKind::Unavailable,
            Error::Invalid(_) => ErrorKind::Invalid,
            Error::UnknownFact(_) | Error::UnknownSpace(_) => ErrorKind::NotFound,
        }
    }
}

/// Result of every fallible core call.
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
