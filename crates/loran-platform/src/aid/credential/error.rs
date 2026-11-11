//! AId error type definitions
//!
//! Defines error types for various AId Token operations

use crate::realm::error::RealmError;
use thiserror::Error;

/// AId Token error types
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AidError {
    #[error("Invalid credential format")]
    InvalidFormat,

    #[error("Token has expired")]
    Expired,

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Base64 decode error: {0}")]
    Base64DecodeError(#[from] base64::DecodeError),

    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Proto decode failure: {0}")]
    DecodeFailure(String),

    #[error("Token generation failed: {0}")]
    GenerationFailed(String),

    #[error("Invalid credential prefix, expected 'ar-'")]
    InvalidPrefix,

    #[error("Empty credential ID")]
    EmptyId,

    #[error("Hex decode error: {0}")]
    HexDecodeError(String),

    #[error("Realm error: {0}")]
    RealmError(#[from] RealmError),

    #[error("publisher not verified or not registered")]
    PublisherNotVerified,

    /// A Realm membership refusal; the message starts with its stable code
    /// (`client_unknown`, `client_revoked`, `member_removed`,
    /// `service_owner_only`).
    #[error("{0}")]
    Membership(String),
}
