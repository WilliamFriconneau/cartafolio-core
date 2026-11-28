use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("Conflict")]
    Conflict,
    #[error("Store I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Store database error: {0}")]
    Db(#[from] rusqlite::Error),
}
