use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("access error: {0}")]
    Access(#[from] pensine_access::AccessError),
    #[error("{0}")]
    Invalid(String),
}
