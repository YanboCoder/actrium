//! HTTP handlers for the mock loran server.
//!
//! Covers the REST endpoints that `actr-hyper` (`IssuerClient`, `PublisherCertCache`)
//! talk to, plus admin seeding routes
//! used by integration test scripts.

use std::sync::Arc;

use actr_protocol::prost::Message as ProstMessage;
use actr_protocol::{
    RegisterRequest, RegisterResponse, RenewCredentialRequest, RenewCredentialResponse,
    register_response, renew_credential_response,
};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use crate::signaling;
use crate::state::{MockState, PublisherEntry, RegisteredActor};

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

// ---------------------------------------------------------------------------
// Issuer `/register` — protobuf in, protobuf out.
// ---------------------------------------------------------------------------

/// `POST /register` and `POST /issuer/register`.
///
/// Shares the credential/ID allocation path with the WebSocket signaling
/// handler via [`signaling::build_register_ok`].
pub async fn register_handler(
    State(state): State<Arc<MockState>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req = RegisterRequest::decode(body.as_ref())
        .map_err(|e| ApiError::bad_request(format!("protobuf decode failed: {e}")))?;

    // Lazily register the realm so callers don't have to pre-seed.
    state.add_realm(req.realm.clone()).await;

    let register_ok = signaling::build_register_ok(&req, &state).await;

    // Track the HTTP-registered actor in the WS registry so route discovery
    // works once the actor opens its authenticated WebSocket. We record an
    // empty `client_id` placeholder that the WS upgrade handler fills in when
    // the peer connects.
    state.registry.write().await.push(RegisteredActor {
        actr_id: register_ok.actr_id.clone(),
        actr_type: req.actr_type.clone(),
        client_id: String::new(),
        ws_address: req.ws_address.clone(),
        service_spec: req.service_spec.clone(),
    });

    let response = RegisterResponse {
        result: Some(register_response::Result::Success(register_ok.clone())),
    };

    tracing::info!(
        serial = register_ok.actr_id.serial_number,
        publisher = req.actr_type.publisher,
        name = req.actr_type.name,
        "mock-loran: registered actor (http)"
    );

    let bytes = response.encode_to_vec();
    Ok((
        StatusCode::OK,
        [("content-type", "application/x-protobuf")],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Issuer `/renew` — protobuf in, protobuf out.
// ---------------------------------------------------------------------------

/// `POST /issuer/renew` and `POST /renew`.
///
/// Validates the renewal token against the actor's expected mock renewal token
/// (derived from the serial number), then re-issues credentials with the same
/// `ActrId`.
pub async fn renew_handler(
    State(state): State<Arc<MockState>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req = RenewCredentialRequest::decode(body.as_ref())
        .map_err(|e| ApiError::bad_request(format!("protobuf decode failed: {e}")))?;

    let actor_id = &req.actr_id;

    // Derive the expected mock renewal token from the serial number.
    let expected_token = signaling::mock_renewal_token(actor_id.serial_number);

    if req.renewal_token.as_ref() != expected_token.as_slice() {
        tracing::warn!(
            serial = actor_id.serial_number,
            "mock-loran: renewal token rejected"
        );
        let err_response = RenewCredentialResponse {
            result: Some(renew_credential_response::Result::Error(
                actr_protocol::ErrorResponse {
                    code: 401,
                    message: "Renewal token invalid or expired".to_string(),
                },
            )),
        };
        let bytes = err_response.encode_to_vec();
        return Ok((
            StatusCode::UNAUTHORIZED,
            [("content-type", "application/x-protobuf")],
            bytes,
        )
            .into_response());
    }

    // Build fresh credentials for the same ActrId. The Actor type is not part
    // of the ActrId; take the one recorded at registration.
    let actr_type = state
        .registry
        .read()
        .await
        .iter()
        .find(|entry| entry.actr_id == *actor_id)
        .map(|entry| entry.actr_type.clone())
        .unwrap_or_default();
    let fake_req = RegisterRequest {
        actr_type,
        realm: actor_id.realm.clone(),
        service: None,
        service_spec: None,
        acl: None,
        ws_address: None,
        manifest_raw: None,
        publisher_signature: None,
        target: None,
        auth_mode: Some(actr_protocol::RegisterAuthMode::Package as i32),
        signing_public_key: None,
    };

    let mut register_ok = signaling::build_register_ok(&fake_req, &state).await;
    // Preserve the original ActrId (mock assigns a new serial, so override).
    register_ok.actr_id = actor_id.clone();
    // Keep the mock renewal token bound to the preserved ActrId, not to the
    // temporary serial allocated by build_register_ok().
    register_ok.renewal_token = Some(expected_token.into());

    let response = RenewCredentialResponse {
        result: Some(renew_credential_response::Result::Success(
            register_ok.clone(),
        )),
    };

    tracing::info!(
        serial = register_ok.actr_id.serial_number,
        "mock-loran: renewed credentials (http)"
    );

    let bytes = response.encode_to_vec();
    Ok((
        StatusCode::OK,
        [("content-type", "application/x-protobuf")],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Publisher Registry verifying key — `GET /registry/publishers/{name}/verifying-key?key_id=X`
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VerifyingKeyQuery {
    // Issuer allows callers to pin a specific key_id; we don't key-rotate in the
    // mock but we accept the parameter for protocol compatibility.
    pub key_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct VerifyingKeyResponse {
    public_key: String,
}

pub async fn verifying_key_handler(
    State(state): State<Arc<MockState>>,
    Path(name): Path<String>,
    Query(_query): Query<VerifyingKeyQuery>,
) -> Result<Response, ApiError> {
    let key = match state.publisher_verifying_key(&name).await {
        Some(k) => k,
        None => {
            tracing::warn!(publisher = %name, "mock-loran: Publisher Registry not registered");
            return Err(ApiError::not_found(format!(
                "publisher '{name}' not registered"
            )));
        }
    };

    let body = VerifyingKeyResponse {
        public_key: base64::engine::general_purpose::STANDARD.encode(key.to_bytes()),
    };
    Ok(Json(body).into_response())
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Admin seeding — replaces register.sh sqlite3 INSERTs.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AdminRealmRequest {
    pub realm: String,
}

#[derive(Debug, Serialize)]
pub struct AdminOk {
    pub ok: bool,
}

pub async fn admin_realm_handler(
    State(state): State<Arc<MockState>>,
    Json(req): Json<AdminRealmRequest>,
) -> Json<AdminOk> {
    state.add_realm(req.realm).await;
    Json(AdminOk { ok: true })
}

#[derive(Debug, Deserialize)]
pub struct AdminPublisherRequest {
    pub name: String,
    pub pubkey_b64: String,
    #[serde(default)]
    pub contact: Option<String>,
}

pub async fn admin_publisher_registry_handler(
    State(state): State<Arc<MockState>>,
    Json(req): Json<AdminPublisherRequest>,
) -> Result<Json<AdminOk>, ApiError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(req.pubkey_b64.trim())
        .map_err(|e| ApiError::bad_request(format!("pubkey base64 decode failed: {e}")))?;
    let arr: [u8; 32] = key_bytes.try_into().map_err(|v: Vec<u8>| {
        ApiError::bad_request(format!("pubkey must be 32 bytes, got {}", v.len()))
    })?;
    let verifying_key = VerifyingKey::from_bytes(&arr)
        .map_err(|e| ApiError::bad_request(format!("invalid ed25519 pubkey: {e}")))?;

    state.publishers.write().await.insert(
        req.name.clone(),
        PublisherEntry {
            name: req.name.clone(),
            verifying_key,
            contact: req.contact,
        },
    );
    tracing::info!(name = %req.name, "mock-loran: Publisher Registry seeded via /admin/publisher");
    Ok(Json(AdminOk { ok: true }))
}

#[derive(Debug, Serialize)]
pub struct AdminStateSnapshot {
    pub realms: Vec<String>,
    pub publishers: Vec<String>,
}

pub async fn admin_state_handler(State(state): State<Arc<MockState>>) -> Json<AdminStateSnapshot> {
    let realms: Vec<String> = state.realms.read().await.keys().cloned().collect();
    let publishers: Vec<String> = state.publishers.read().await.keys().cloned().collect();
    Json(AdminStateSnapshot { realms, publishers })
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, Json(body)).into_response()
    }
}

pub async fn relay_identity_handler(
    State(state): State<Arc<MockState>>,
    Json(request): Json<actr_relay_security::IdentityRequest>,
) -> Result<Json<actr_relay_security::IssuedIdentity>, StatusCode> {
    if state
        .relay_enrollment_unavailable
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    use actr_relay_security::*;
    use ed25519_dalek::Signer;
    let actor = actr_protocol::ActrId::from_string_repr(&request.endpoint)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if actor.to_string_repr() != request.endpoint
        || request.public_key == [0; 32]
        || request.admission_commitment == [0; 32]
        || request.renewal_token != signaling::mock_renewal_token(actor.serial_number)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let actor_type = state
        .registry
        .read()
        .await
        .iter()
        .find(|entry| entry.actr_id == actor)
        .map(|entry| entry.actr_type.to_string_repr())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::to_vec(&IdentityClaims {
        domain: CERTIFICATE_DOMAIN.into(),
        endpoint: request.endpoint,
        actor_type,
        client_key: "mock-client-key".into(),
        public_key: request.public_key,
        admission_commitment: request.admission_commitment,
        expires_at: now + CERTIFICATE_TTL,
    })
    .unwrap();
    let signature = state
        .issuer_signing_key()
        .sign(&signing_bytes(&claims))
        .to_bytes()
        .to_vec();
    Ok(Json(IssuedIdentity {
        certificate: Certificate {
            key_id: state.issuer_signing_key_id(),
            claims,
            signature,
        },
        issuer: IssuerKey {
            key_id: state.issuer_signing_key_id(),
            public_key: *state.issuer_signing_key().verifying_key().as_bytes(),
            expires_at: now + CERTIFICATE_TTL,
        },
    }))
}

pub async fn relay_key_handler(
    State(state): State<Arc<MockState>>,
    Path((_realm, key_id)): Path<(String, u32)>,
) -> Result<Json<actr_relay_security::IssuerKey>, StatusCode> {
    if key_id != state.issuer_signing_key_id() {
        return Err(StatusCode::NOT_FOUND);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    Ok(Json(actr_relay_security::IssuerKey {
        key_id,
        public_key: *state.issuer_signing_key().verifying_key().as_bytes(),
        expires_at: now + actr_relay_security::CERTIFICATE_TTL,
    }))
}
