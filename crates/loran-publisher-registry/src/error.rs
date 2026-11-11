use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PublisherRegistryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    #[error("key revoked: {0}")]
    KeyRevoked(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("reserved name: {0}")]
    ReservedName(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid status for this operation: {0}")]
    InvalidStatus(String),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("certificate expired: signing key has expired, please renew")]
    CertificateExpired,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
}

impl IntoResponse for PublisherRegistryError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            PublisherRegistryError::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            PublisherRegistryError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            PublisherRegistryError::ReservedName(_)
            | PublisherRegistryError::InvalidName(_)
            | PublisherRegistryError::InvalidRequest(_) => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            PublisherRegistryError::InvalidStatus(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            PublisherRegistryError::VerificationFailed(_) => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            PublisherRegistryError::KeyRevoked(_) | PublisherRegistryError::CertificateExpired => {
                (StatusCode::FORBIDDEN, self.to_string())
            }
            PublisherRegistryError::QuotaExceeded(_) => {
                (StatusCode::TOO_MANY_REQUESTS, self.to_string())
            }
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(json!({"error": message}))).into_response()
    }
}
