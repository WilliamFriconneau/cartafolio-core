use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("access error: {0}")]
    Access(#[from] pensine_access::AccessError),
    #[error("store error: {0}")]
    Store(#[from] pensine_store::StoreError),
    #[error("{0}")]
    Invalid(String),
}

impl axum::response::IntoResponse for ServerError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!(error = %self, "request failed");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error",
        )
            .into_response()
    }
}
