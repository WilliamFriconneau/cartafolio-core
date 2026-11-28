use thiserror::Error;

#[derive(Debug, Error)]
pub enum AccessError {
    #[error("not found")]
    NotFound,
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Conflict")]
    Conflict,
    #[error("{0}")]
    Invalid(String),
    #[error("Access I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Access database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Store(#[from] pensine_store::StoreError),
}
