//! Issuer HTTP client
//!
//! Encapsulates the logic for sending protobuf requests to the Issuer `/register` endpoint.
//! Supports two registration modes:
//! - Package registration: authenticate with manifest_raw + publisher_signature
//! - Linked registration: authenticate with realm authorization
//!
//! Every `/register`, `/reissue` and `/renew` request carries an
//! `x-actr-client-proof` signed by the node's Realm client key
//! (`actr/protocols/realm-membership.md` section 6).
//!
//! Credential renewal is handled by the Credential Manager via `POST /issuer/renew`,
//! not by re-calling `/register` with a PSK.

use std::path::Path;
use std::time::Duration;

use prost::Message;
use tracing::{debug, error, info, warn};

use actr_protocol::realm_membership::{
    CLIENT_PROOF_HEADER, ClientProof, ProofOp, RealmClientKeyFile,
};
use actr_protocol::{
    ErrorResponse, RegisterRequest, RegisterResponse, ReissueCredentialRequest,
    RenewCredentialRequest, RenewCredentialResponse, renew_credential_response,
};

use crate::error::{HyperError, HyperResult};

/// Structured errors returned by POST /issuer/renew.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RenewError {
    #[error("invalid renewal request: {0}")]
    InvalidRequest(String),
    #[error("renewal token rejected")]
    TokenRejected,
    #[error("renewal attempt result expired; start a new attempt")]
    AttemptExpired,
    #[error("renewal attempt id conflicts with a different request")]
    AttemptConflict,
    #[error("renewal attempt is still in progress")]
    AttemptInProgress,
    #[error("realm unavailable")]
    RealmUnavailable,
    #[error("renewal rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("retryable renewal error: {0}")]
    Retryable(String),
    #[error("renewal protocol error: {0}")]
    Protocol(String),
}

/// Structured errors returned by POST /issuer/reissue.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ReissueError {
    #[error("reissue attempt result expired; start a new attempt")]
    AttemptExpired,
    #[error("reissue attempt id conflicts with a different request")]
    AttemptConflict,
    #[error("reissue attempt is still in progress")]
    AttemptInProgress,
    #[error("reissue rejected with HTTP status {0}")]
    Rejected(u16),
    #[error("retryable reissue error: {0}")]
    Retryable(String),
    #[error("reissue protocol error: {0}")]
    Protocol(String),
}

/// Signing boundary for OS-managed Realm keys. Implementations retain private
/// material in platform storage; the Host receives only public keys/signatures.
pub trait RealmKeySigner: Send + Sync {
    fn public_key(&self) -> [u8; 32];
    fn sign_payload(&self, payload: &[u8]) -> Result<[u8; 64], HyperError>;
}

impl RealmKeySigner for ed25519_dalek::SigningKey {
    fn public_key(&self) -> [u8; 32] {
        self.verifying_key().to_bytes()
    }
    fn sign_payload(&self, payload: &[u8]) -> Result<[u8; 64], HyperError> {
        Ok(ed25519_dalek::Signer::sign(self, payload).to_bytes())
    }
}

/// Mobile wall clocks can lead the Issuer by a few seconds. Backdate the
/// envelope while preserving the protocol's maximum five-minute lifetime;
/// this does not relax the Issuer's window or replay checks.
pub(crate) fn client_proof_window() -> (String, String) {
    let issued = chrono::Utc::now() - chrono::Duration::seconds(30);
    let expires =
        issued + chrono::Duration::seconds(actr_protocol::realm_management::MAX_VALIDITY_SECS);
    (
        issued.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// A Realm client key: signs the `x-actr-client-proof` of Issuer requests.
#[derive(Clone)]
pub struct RealmClientKey {
    realm: String,
    provider_id: String,
    public_key: [u8; 32],
    key: std::sync::Arc<dyn RealmKeySigner>,
}

impl std::fmt::Debug for RealmClientKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealmClientKey")
            .field("realm", &self.realm)
            .field("provider_id", &self.provider_id)
            .finish_non_exhaustive()
    }
}

impl RealmClientKey {
    pub fn new(
        realm: impl Into<String>,
        provider_id: impl Into<String>,
        key: ed25519_dalek::SigningKey,
    ) -> Self {
        Self {
            realm: realm.into(),
            provider_id: provider_id.into(),
            public_key: key.verifying_key().to_bytes(),
            key: std::sync::Arc::new(key),
        }
    }

    pub fn from_signer(
        realm: String,
        provider_id: String,
        key: std::sync::Arc<dyn RealmKeySigner>,
    ) -> Self {
        Self {
            realm,
            provider_id,
            public_key: key.public_key(),
            key,
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }
    pub fn key_id(&self) -> String {
        actr_protocol::realm_membership::client_key_id(&self.public_key())
    }

    pub(crate) fn sign_document(
        &self,
        mut document: serde_json::Value,
    ) -> HyperResult<serde_json::Value> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        document
            .as_object_mut()
            .ok_or_else(|| HyperError::Config("signing payload must be an object".into()))?
            .remove("signature");
        let payload = actr_protocol::realm_profile::jcs(&document);
        let signature = self
            .key
            .sign_payload(payload.as_bytes())
            .map_err(|_| HyperError::Config("Realm client signer unavailable".into()))?;
        let public_key = ed25519_dalek::VerifyingKey::from_bytes(&self.public_key())
            .map_err(|_| HyperError::Config("invalid Realm client public key".into()))?;
        public_key
            .verify_strict(
                payload.as_bytes(),
                &ed25519_dalek::Signature::from_bytes(&signature),
            )
            .map_err(|_| {
                HyperError::Config("Realm client signer returned an invalid signature".into())
            })?;
        document["signature"] = serde_json::json!({
            "key_id": self.key_id(), "algorithm": "Ed25519", "value": STANDARD.encode(signature),
        });
        Ok(document)
    }

    /// Load a key file written by `actr realm join`; it must be readable
    /// only by its owner ([`RealmClientKeyFile::read`]).
    pub fn load(path: &Path) -> HyperResult<Self> {
        let (file, key) = RealmClientKeyFile::read(path)
            .map_err(|error| HyperError::Config(error.to_string()))?;
        Ok(Self::new(file.realm, file.provider_id, key))
    }

    /// Load the key file configured for `realm`, if any; it must be a key
    /// of that Realm.
    pub fn load_for(path: Option<&Path>, realm: &str) -> HyperResult<Option<Self>> {
        let Some(path) = path else {
            return Ok(None);
        };
        let key = Self::load(path)?;
        if key.realm != realm {
            return Err(HyperError::Config(format!(
                "Realm client key {} is for Realm {}, not {realm}",
                path.display(),
                key.realm
            )));
        }
        Ok(Some(key))
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// A freshly signed proof for `body` of an `op` request on `realm`.
    pub(crate) fn proof(&self, op: ProofOp, realm: &str, body: &[u8]) -> HyperResult<String> {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        if realm != self.realm {
            return Err(HyperError::Config("Realm client key realm mismatch".into()));
        }
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let mut proof = ClientProof::new(op, realm, &self.provider_id, body, &nonce);
        (proof.issued_at, proof.expires_at) = client_proof_window();
        let document = serde_json::to_value(proof)
            .map_err(|_| HyperError::Config("invalid client proof".into()))?;
        Ok(URL_SAFE_NO_PAD.encode(self.sign_document(document)?.to_string()))
    }
}

enum ProtobufRequestError {
    HttpStatus(u16),
    Retryable(String),
    Protocol(String),
}

/// Issuer HTTP client
///
/// Encapsulates the logic for sending protobuf requests to the Issuer /register endpoint.
/// All requests use `application/x-protobuf` encoding.
pub struct IssuerClient {
    endpoint: String,
    http: reqwest::Client,
    /// Realm client key that signs each request's `x-actr-client-proof`
    client_key: Option<RealmClientKey>,
}

const CONTROL_ATTEMPT_RETRY_DEADLINE: Duration = Duration::from_secs(95);
const CONTROL_ATTEMPT_BASE_RETRY_DELAY: Duration = Duration::from_millis(100);
const CONTROL_ATTEMPT_MAX_RETRY_DELAY: Duration = Duration::from_secs(2);

pub(crate) fn new_control_attempt_id() -> prost::bytes::Bytes {
    prost::bytes::Bytes::copy_from_slice(uuid::Uuid::new_v4().as_bytes())
}

impl IssuerClient {
    /// Create a new Issuer client
    ///
    /// `endpoint` is the Issuer base URL, e.g. `"http://issuer.example.com:8080"`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        crate::timer::register_external(
            crate::timer::ids::EXTERNAL_ISSUER_HTTP_REQUEST,
            Duration::from_secs(30),
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest::Client build failed (should never happen)");
        Self {
            endpoint: endpoint.into(),
            http,
            client_key: None,
        }
    }

    /// Sign requests with the Realm client key `key`.
    pub fn with_client_key(mut self, key: RealmClientKey) -> Self {
        self.client_key = Some(key);
        self
    }

    /// The `x-actr-client-proof` header for an `op` request on `realm`.
    fn proof_header(
        &self,
        op: ProofOp,
        realm: &str,
        body: &[u8],
    ) -> HyperResult<Option<(&'static str, String)>> {
        self.client_key
            .as_ref()
            .map(|key| {
                key.proof(op, realm, body)
                    .map(|value| (CLIENT_PROOF_HEADER, value))
            })
            .transpose()
    }

    /// Initial registration: authenticate with publisher manifest
    ///
    /// Sends a RegisterRequest (containing manifest_raw + publisher_signature),
    /// receives a RegisterResponse.
    pub async fn register_with_manifest(
        &self,
        req: RegisterRequest,
    ) -> HyperResult<RegisterResponse> {
        info!(
            endpoint = %self.endpoint,
            "initial registration: registering with Issuer via publisher manifest"
        );
        self.do_register(req).await
    }

    /// Linked registration: authenticate with realm authorization.
    ///
    /// Sends a RegisterRequest marked as linked source mode. Issuer authorizes it
    /// by the client proof alone instead of publisher package identity.
    pub async fn register_linked(&self, req: RegisterRequest) -> HyperResult<RegisterResponse> {
        info!(
            endpoint = %self.endpoint,
            "linked registration: registering with Issuer via realm authorization"
        );
        self.do_register(req).await
    }

    /// Re-authenticate a workload and re-issue credentials for its existing,
    /// stable [`actr_protocol::ActrId`].
    ///
    /// Unlike `/register`, `/reissue` never allocates a new identity. It is the
    /// hard fallback when the renewal token is no longer usable.
    pub async fn reissue_credential(
        &self,
        req: ReissueCredentialRequest,
    ) -> HyperResult<RegisterResponse> {
        self.reissue_credential_classified(req)
            .await
            .map_err(|error| HyperError::IssuerBootstrapFailed(error.to_string()))
    }

    pub(crate) async fn reissue_credential_classified(
        &self,
        req: ReissueCredentialRequest,
    ) -> Result<RegisterResponse, ReissueError> {
        info!(
            endpoint = %self.endpoint,
            actor_id = %req.actr_id.to_string_repr(),
            "reissuing Issuer credential for stable actor identity"
        );
        let realm = req.registration.realm.clone();
        self.do_protobuf_request(ProofOp::Reissue, &realm, req.encode_to_vec(), true)
            .await
            .map_err(ReissueError::from)
    }

    /// Soft renewal: authenticate with the renewal token bound to the current ActrId.
    pub async fn renew_credential(
        &self,
        req: RenewCredentialRequest,
    ) -> Result<RenewCredentialResponse, RenewError> {
        let base = self.endpoint.to_string().trim_end_matches('/').to_string();
        let url = format!("{}/renew", base);
        let body = req.encode_to_vec();

        debug!(url = %url, body_len = body.len(), "sending Issuer renew request");

        // Spawn onto tokio worker to avoid stack overflow on GCD cooperative
        // queues (same root cause as do_register).
        let deadline = tokio::time::Instant::now() + CONTROL_ATTEMPT_RETRY_DEADLINE;
        let mut completed = None;
        let mut last_transport_error = None;
        let mut retry_count = 0u32;
        loop {
            let mut builder = self
                .http
                .post(&url)
                .header("Content-Type", "application/x-protobuf")
                .header("Accept", "application/x-protobuf");
            // Each try is signed anew: the proof's nonce is single-use.
            if let Some((name, value)) = self
                .proof_header(ProofOp::Renew, &req.actr_id.realm, &body)
                .map_err(|_| RenewError::Protocol("Realm client signer unavailable".into()))?
            {
                builder = builder.header(name, value);
            }
            let pending = builder.body(body.clone()).send();

            let response = match tokio::task::spawn(pending).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    last_transport_error = Some(format!("HTTP request failed: {error}"));
                    if retry_control_attempt(deadline, control_attempt_backoff(retry_count)).await {
                        retry_count = retry_count.saturating_add(1);
                        continue;
                    }
                    break;
                }
                Err(error) => {
                    last_transport_error = Some(format!("spawn failed: {error}"));
                    if retry_control_attempt(deadline, control_attempt_backoff(retry_count)).await {
                        retry_count = retry_count.saturating_add(1);
                        continue;
                    }
                    break;
                }
            };

            let status = response.status();
            let retry_after =
                parse_retry_after(response.headers().get(reqwest::header::RETRY_AFTER));
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    last_transport_error = Some(format!("failed to read response body: {error}"));
                    if retry_control_attempt(deadline, control_attempt_backoff(retry_count)).await {
                        retry_count = retry_count.saturating_add(1);
                        continue;
                    }
                    break;
                }
            };
            let retryable_status = status == reqwest::StatusCode::TOO_EARLY;
            if retryable_status {
                let delay = retry_after.unwrap_or_else(|| Duration::from_secs(1));
                if retry_control_attempt(deadline, delay).await {
                    retry_count = retry_count.saturating_add(1);
                    continue;
                }
            }
            completed = Some((status, retry_after, bytes));
            break;
        }
        let (status, retry_after, bytes) = completed.ok_or_else(|| {
            RenewError::Retryable(
                last_transport_error.unwrap_or_else(|| "request failed without response".into()),
            )
        })?;

        if !status.is_success() {
            // The message starts with a stable code (`client_revoked: …`); log
            // only that code, never the free-form remainder (issue #484).
            let code = RenewCredentialResponse::decode(bytes.as_ref())
                .ok()
                .and_then(|response| match response.result {
                    Some(renew_credential_response::Result::Error(error)) => error
                        .message
                        .split(':')
                        .next()
                        .map(|code| code.trim().to_owned()),
                    _ => None,
                })
                .unwrap_or_default();
            warn!(url = %url, status = %status, code = %code, "Issuer refused renewal");
            return Err(classify_renew_status(status.as_u16(), retry_after));
        }

        let decoded = RenewCredentialResponse::decode(bytes.as_ref())
            .map_err(|e| RenewError::Protocol(format!("response protobuf decode failed: {e}")))?;

        match decoded.result.as_ref() {
            Some(renew_credential_response::Result::Success(_)) => Ok(decoded),
            Some(renew_credential_response::Result::Error(err)) => Err(classify_renew_error(err)),
            None => Err(RenewError::Protocol(
                "renew response missing result".to_string(),
            )),
        }
    }

    /// Send POST /register request, common logic
    ///
    /// Encodes a RegisterRequest as protobuf and POSTs it to `{endpoint}/register`,
    /// then decodes the response as RegisterResponse.
    ///
    /// The HTTP call is spawned onto the tokio runtime to avoid stack overflow
    /// when polled from a GCD cooperative queue (UNIFFI async bridge context).
    /// reqwest/hyper's deeply nested combinator chain (~140 frames of generic
    /// state machines) exceeds GCD thread stack limits on iOS simulator.
    async fn do_register(&self, req: RegisterRequest) -> HyperResult<RegisterResponse> {
        // `/register` is intentionally non-idempotent: a second call requests
        // a new identity, so ambiguous transport failures are surfaced rather
        // than retried automatically.
        let realm = req.realm.clone();
        self.do_protobuf_request(ProofOp::Register, &realm, req.encode_to_vec(), false)
            .await
            .map_err(|error| HyperError::IssuerBootstrapFailed(error.to_string()))
    }

    /// Send a protobuf request that returns [`RegisterResponse`].
    async fn do_protobuf_request(
        &self,
        op: ProofOp,
        realm: &str,
        body: Vec<u8>,
        idempotent: bool,
    ) -> Result<RegisterResponse, ProtobufRequestError> {
        let path = match op {
            ProofOp::Register => "register",
            ProofOp::Reissue => "reissue",
            ProofOp::Renew => "renew",
            ProofOp::RelayIdentity => "relay-identity",
        };
        let base = self.endpoint.to_string().trim_end_matches('/').to_string();
        let url = format!("{base}/{path}");

        debug!(url = %url, body_len = body.len(), "sending Issuer protobuf request");

        let deadline = tokio::time::Instant::now() + CONTROL_ATTEMPT_RETRY_DEADLINE;
        let mut completed = None;
        let mut last_error = None;
        let mut retry_count = 0u32;
        loop {
            let mut request_builder = self
                .http
                .post(&url)
                .header("Content-Type", "application/x-protobuf")
                .header("Accept", "application/x-protobuf");

            if let Some((name, value)) = self.proof_header(op, realm, &body).map_err(|_| {
                ProtobufRequestError::Protocol("Realm client signer unavailable".into())
            })? {
                request_builder = request_builder.header(name, value);
            }

            let pending = request_builder.body(body.clone()).send();
            match tokio::task::spawn(pending).await {
                Ok(Ok(candidate)) => {
                    let status = candidate.status();
                    let retryable_status = status == reqwest::StatusCode::TOO_EARLY;
                    let retry_after =
                        parse_retry_after(candidate.headers().get(reqwest::header::RETRY_AFTER));
                    match candidate.bytes().await {
                        Ok(bytes) => {
                            if idempotent && retryable_status {
                                let delay = retry_after.unwrap_or_else(|| Duration::from_secs(1));
                                if retry_control_attempt(deadline, delay).await {
                                    retry_count = retry_count.saturating_add(1);
                                    continue;
                                }
                            }
                            completed = Some((status, bytes));
                            break;
                        }
                        Err(err) => {
                            last_error = Some(format!("failed to read response body: {err}"));
                        }
                    }
                }
                Ok(Err(err)) => {
                    last_error = Some(format!("HTTP request failed: {err}"));
                }
                Err(err) => {
                    last_error = Some(format!("spawn failed: {err}"));
                }
            }
            if idempotent
                && retry_control_attempt(deadline, control_attempt_backoff(retry_count)).await
            {
                retry_count = retry_count.saturating_add(1);
                continue;
            }
            break;
        }
        let (status, bytes) = completed.ok_or_else(|| {
            let message = last_error.unwrap_or_else(|| "request failed without response".into());
            error!(url = %url, error = %message, "Issuer HTTP request failed");
            ProtobufRequestError::Retryable(message)
        })?;

        if !status.is_success() {
            warn!(url = %url, status = %status, "Issuer returned non-2xx status");
            return Err(ProtobufRequestError::HttpStatus(status.as_u16()));
        }

        debug!(url = %url, response_len = bytes.len(), "received Issuer response");

        let resp = RegisterResponse::decode(bytes.as_ref()).map_err(|e| {
            error!(url = %url, error = %e, "failed to decode Issuer RegisterResponse");
            ProtobufRequestError::Protocol(format!("response protobuf decode failed: {e}"))
        })?;

        Ok(resp)
    }
}

impl std::fmt::Display for ProtobufRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpStatus(status) => write!(formatter, "Issuer returned error status: {status}"),
            Self::Retryable(message) | Self::Protocol(message) => formatter.write_str(message),
        }
    }
}

impl From<ProtobufRequestError> for ReissueError {
    fn from(error: ProtobufRequestError) -> Self {
        match error {
            ProtobufRequestError::HttpStatus(409) => Self::AttemptConflict,
            ProtobufRequestError::HttpStatus(410) => Self::AttemptExpired,
            ProtobufRequestError::HttpStatus(425) => Self::AttemptInProgress,
            ProtobufRequestError::HttpStatus(500 | 502 | 503 | 504) => {
                Self::Retryable(error.to_string())
            }
            ProtobufRequestError::HttpStatus(status) => Self::Rejected(status),
            ProtobufRequestError::Retryable(message) => Self::Retryable(message),
            ProtobufRequestError::Protocol(message) => Self::Protocol(message),
        }
    }
}

async fn retry_control_attempt(deadline: tokio::time::Instant, delay: Duration) -> bool {
    let now = tokio::time::Instant::now();
    if now >= deadline {
        return false;
    }
    let delay = delay.min(deadline.saturating_duration_since(now));
    crate::timer::sleep(crate::timer::ids::ISSUER_CONTROL_ATTEMPT_RETRY, delay).await;
    true
}

fn control_attempt_backoff(retry_count: u32) -> Duration {
    let multiplier = 1u32 << retry_count.min(4);
    (CONTROL_ATTEMPT_BASE_RETRY_DELAY * multiplier).min(CONTROL_ATTEMPT_MAX_RETRY_DELAY)
}

fn parse_retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    value
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn classify_renew_error(error: &ErrorResponse) -> RenewError {
    classify_renew_status(error.code as u16, None)
}

fn classify_renew_status(status: u16, retry_after: Option<Duration>) -> RenewError {
    match status {
        400 => RenewError::InvalidRequest("invalid renew request".to_string()),
        401 => RenewError::TokenRejected,
        403 => RenewError::RealmUnavailable,
        409 => RenewError::AttemptConflict,
        410 => RenewError::AttemptExpired,
        425 => RenewError::AttemptInProgress,
        429 => RenewError::RateLimited { retry_after },
        500 | 502 | 503 | 504 => RenewError::Retryable(format!("Issuer returned {status}")),
        other => RenewError::Protocol(format!("unexpected Issuer renew status {other}")),
    }
}

#[cfg(test)]
#[path = "issuer_client_tests.rs"]
mod tests;
