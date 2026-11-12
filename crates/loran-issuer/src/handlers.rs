//! Issuer (Actor Identity Service) HTTP Handler

use crate::{
    idempotency::{
        AttemptError, AttemptLease, ClaimOutcome, ControlAttemptStore, Operation,
        register_ok_response_expiry,
    },
    issuer::{AIdIssuer, ReissuePreparation},
    ratelimit::ip_rate_limiter,
};
use actr_protocol::realm_membership::{CLIENT_PROOF_HEADER, ClientProof, ProofOp, Role};
use actr_protocol::{
    ErrorResponse, RegisterRequest, RegisterResponse, ReissueCredentialRequest,
    RenewCredentialRequest, RenewCredentialResponse, register_response, renew_credential_response,
};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
        header::{ACCEPT, CONTENT_TYPE, RETRY_AFTER},
    },
    response::{IntoResponse, Json, Response},
    routing::post,
};
use platform::aid::AidError;
use platform::monitoring::ServiceCounters;
use platform::realm::membership::{self, Limits, Signer};
use platform::realm::{Realm as RealmEntity, RealmProfileRecord, acl::ActorAcl};
use prost::Message;
use serde_json::{Value, json};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

/// Issuer service state
#[derive(Clone)]
pub struct IssuerState {
    pub issuer: Arc<AIdIssuer>,
    idempotency: Arc<ControlAttemptStore>,
    /// Service-level counters for metrics collection.
    pub counters: Option<Arc<ServiceCounters>>,
    /// Membership limits (`[realm]`).
    pub(crate) limits: Limits,
    /// Signs Realm Profiles for `update_realm` when this Loran is an Actr Facility
    /// Provider.
    pub(crate) profiles: Option<Arc<crate::realm_profile::RealmProfiles>>,
}

impl IssuerState {
    pub fn new(issuer: AIdIssuer) -> Self {
        Self::from_shared_issuer(Arc::new(issuer))
    }

    /// Build state around an issuer that is already shared with another
    /// consumer (e.g. the control plane holding `Arc<AIdIssuer>`).
    pub fn from_shared_issuer(issuer: Arc<AIdIssuer>) -> Self {
        let idempotency = Arc::new(ControlAttemptStore::new(
            &issuer.config.renewal_token_secret,
        ));
        Self {
            issuer,
            idempotency,
            counters: None,
            limits: Limits::from(&platform::config::RealmConfig::default()),
            profiles: None,
        }
    }

    /// Membership limits and, when this Loran is an Actr Facility Provider, the Realm
    /// Profiles `update_realm` signs.
    pub fn with_membership(
        mut self,
        config: &platform::config::RealmConfig,
        profiles: Option<Arc<crate::realm_profile::RealmProfiles>>,
    ) -> Self {
        self.limits = Limits::from(config);
        self.profiles = profiles;
        self
    }

    pub fn with_counters(mut self, counters: Arc<ServiceCounters>) -> Self {
        self.counters = Some(counters);
        self
    }
}

/// Create the Issuer service router
///
/// Applies rate limiting middleware:
/// - IP level: 100 req/min (prevents DoS attacks from a single IP)
pub fn create_router(state: IssuerState) -> Router {
    Router::new()
        .route("/register", post(register_actr))
        .route("/renew", post(renew_credential))
        .route(
            "/relay-identity",
            post(relay_identity).layer(axum::extract::DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/relay-key/{realm}/{key_id}",
            axum::routing::get(relay_signing_key),
        )
        .route("/reissue", post(reissue_credential))
        .route("/membership", post(crate::membership::post_membership))
        .route("/health", axum::routing::get(health_check))
        .layer(ip_rate_limiter())
        // These public APIs authenticate signed client proofs, not cookies.
        // Browser clients may be hosted on any origin; keep CORS scoped to
        // this router so it never exposes the admin/control plane.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([
                    CONTENT_TYPE,
                    ACCEPT,
                    HeaderName::from_static(CLIENT_PROOF_HEADER),
                ])
                .expose_headers([RETRY_AFTER]),
        )
        .with_state(state)
}

/// ActrId registration handler - returns RegisterResponse strictly per the proto definition
/// RegisterRequest -> RegisterResponse
async fn register_actr(State(state): State<IssuerState>, headers: HeaderMap, body: Bytes) -> Bytes {
    let start = std::time::Instant::now();

    // Decode the protobuf request
    let request = match RegisterRequest::decode(body.clone()) {
        Ok(req) => req,
        Err(err) => {
            platform::recording::error!("Failed to decode protobuf request: {}", err);
            let error_result = RegisterResponse {
                result: Some(register_response::Result::Error(ErrorResponse {
                    code: 400, // Bad Request
                    message: format!("Invalid protobuf: {err}"),
                })),
            };
            return encode_result(error_result);
        }
    };

    platform::recording::debug!(
        "Received register request for realm {}, type {}",
        request.realm,
        request.actr_type.to_string_repr()
    );

    // Validate the identity fields before minting an ActrId or deriving ACL keys.
    // ActrType is a protobuf type with public fields, so a caller can construct
    // values (e.g. publisher "x:y") that collide when formatted into the same
    // type string and silently overwrite another actor's ACL rules, or that never
    // re-parse downstream. Reject early.
    if let Err(err) = request.actr_type.validate() {
        let error_result = RegisterResponse {
            result: Some(register_response::Result::Error(ErrorResponse {
                code: 400,
                message: format!("Invalid actor identity: {err}"),
            })),
        };
        return encode_result(error_result);
    }

    if let Err(message) = check_acl_source_realms(&request) {
        return encode_result(RegisterResponse {
            result: Some(register_response::Result::Error(ErrorResponse {
                code: 400,
                message,
            })),
        });
    }

    // Authenticate before saying anything about the Realm.
    let signer =
        match authenticate_client(&state, &headers, ProofOp::Register, &request.realm, &body).await
        {
            Ok(signer) => signer,
            Err(error) => {
                return encode_result(RegisterResponse {
                    result: Some(register_response::Result::Error(error)),
                });
            }
        };

    // Validate that the Realm exists, is in a healthy state, and has not expired
    if let Err(validation_err) = RealmEntity::validate_realm(&request.realm).await {
        let (code, message) = RealmEntity::map_validation_error(validation_err);
        let error_result = RegisterResponse {
            result: Some(register_response::Result::Error(ErrorResponse {
                code,
                message: format!("Realm validation failed: {message}"),
            })),
        };
        return encode_result(error_result);
    }

    let pool = platform::storage::db::get_database().get_pool();
    if let Err(error) = check_service_owner(pool, &request, &signer).await {
        return encode_result(RegisterResponse {
            result: Some(register_response::Result::Error(error)),
        });
    }

    // Invoke the issuer to issue a credential
    let result = match state.issuer.issue_credential(&request, &signer).await {
        Ok(response) => {
            if let Some(register_response::Result::Success(ref register_ok)) = response.result {
                platform::recording::debug!(
                    "Successfully registered ActrId: {}, type={}",
                    register_ok.actr_id,
                    request.actr_type.to_string_repr()
                );

                // ACL policy and pending signaling metadata commit together.
                if let Err((code, message)) =
                    persist_registration_state(&request, register_ok, &signer).await
                {
                    if let Some(ref ctr) = state.counters {
                        ctr.record_request(false, start.elapsed().as_secs_f64() * 1000.0)
                            .await;
                    }
                    return encode_result(RegisterResponse {
                        result: Some(register_response::Result::Error(ErrorResponse {
                            code,
                            message,
                        })),
                    });
                }
                platform::realm::changes::announce_actor(&register_ok.actr_id);
            }
            if let Some(ref ctr) = state.counters {
                ctr.record_request(true, start.elapsed().as_secs_f64() * 1000.0)
                    .await;
            }
            response
        }
        Err(err) => {
            platform::recording::error!("Failed to register ActrId: {}", err);
            if let Some(ref ctr) = state.counters {
                ctr.record_request(false, start.elapsed().as_secs_f64() * 1000.0)
                    .await;
            }
            RegisterResponse {
                result: Some(register_response::Result::Error(
                    aid_error_to_error_response(err),
                )),
            }
        }
    };

    encode_result(result)
}

fn membership_error(code: &str, message: impl std::fmt::Display) -> ErrorResponse {
    let status = match code {
        "invalid_request" => 400,
        "unavailable" => 503,
        _ => 403,
    };
    ErrorResponse {
        code: status,
        message: format!("{code}: {message}"),
    }
}

fn from_membership(error: membership::MembershipError) -> ErrorResponse {
    membership_error(error.code.as_str(), error.message)
}

/// The Actr Facility Provider ID a request for `realm` may name: this Actr Facility
/// Provider's, or for a Realm without a Realm Profile (served by a Loran
/// that is no Actr Facility Provider) the authority of the Issuer URL, the request's
/// `Host`.
pub(crate) async fn expected_provider_id(
    state: &IssuerState,
    realm: &str,
    claimed: &str,
    headers: &HeaderMap,
) -> String {
    let own = &state.issuer.config.provider_id;
    if claimed == own {
        return own.clone();
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase);
    if host.as_deref() == Some(claimed) && matches!(RealmProfileRecord::get(realm).await, Ok(None))
    {
        return claimed.to_string();
    }
    own.clone()
}

/// Check the `x-actr-client-proof` of a request for `op` on `realm` with
/// `body` (`realm-membership.md` section 6): the proof itself, then that its
/// Realm client key is registered, unrevoked and of an active member. Runs
/// before anything about the Realm is revealed.
async fn authenticate_client(
    state: &IssuerState,
    headers: &HeaderMap,
    op: ProofOp,
    realm: &str,
    body: &[u8],
) -> Result<Signer, ErrorResponse> {
    let invalid = |message: &dyn std::fmt::Display| membership_error("invalid_request", message);
    let header = headers
        .get(CLIENT_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| invalid(&format!("missing {CLIENT_PROOF_HEADER}")))?;
    let proof = ClientProof::parse_header(header).map_err(|e| invalid(&e))?;
    let provider = expected_provider_id(state, realm, &proof.provider_id, headers).await;
    proof
        .check(op, realm, &provider, body, chrono::Utc::now())
        .map_err(|e| invalid(&e))?;
    let unknown = || membership_error("client_unknown", "the Realm client key is not registered");
    let key = membership::client_key(realm, proof.signer_key_id())
        .await
        .map_err(from_membership)?
        .ok_or_else(unknown)?;
    proof.verify(&key.public_key).map_err(|_| unknown())?;
    let signer = membership::authenticate(key)
        .await
        .map_err(from_membership)?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&proof.expires_at)
        .map_err(|e| invalid(&e))?
        .timestamp();
    match platform::realm::management::use_nonce(&proof.nonce, expires_at).await {
        Ok(true) => Ok(signer),
        Ok(false) => Err(invalid(&"nonce was already used")),
        Err(error) => Err(membership_error("unavailable", error)),
    }
}

/// Only the owner registers the Realm's Service: an Actor whose publisher
/// and program are those of the Realm Profile `package.type`, any version.
/// Reads the Profile on `db`, which may be the registration transaction.
async fn check_service_owner<'e>(
    db: impl sqlx::SqliteExecutor<'e>,
    request: &RegisterRequest,
    signer: &Signer,
) -> Result<(), ErrorResponse> {
    if signer.role == Role::Owner {
        return Ok(());
    }
    let document: Option<String> =
        sqlx::query_scalar("SELECT document FROM realm_profile WHERE realm = ?")
            .bind(&request.realm)
            .fetch_optional(db)
            .await
            .map_err(|error| membership_error("unavailable", error))?;
    let service = document
        .and_then(|document| serde_json::from_str::<Value>(&document).ok())
        .and_then(|profile| profile["package"]["type"].as_str().map(str::to_string))
        .and_then(|text| actr_protocol::ActrType::from_program_identity(&text).ok());
    match service {
        Some(service)
            if service.publisher == request.actr_type.publisher
                && service.name == request.actr_type.name =>
        {
            Err(membership_error(
                "service_owner_only",
                "only the Realm owner registers the Realm's Service",
            ))
        }
        _ => Ok(()),
    }
}

/// The actor must be bound to `signer`'s member (reissue) or to exactly its
/// key (renew).
async fn check_binding(
    actor_id: &actr_protocol::ActrId,
    signer: &Signer,
    same_key: bool,
) -> Result<(), ErrorResponse> {
    let mut conn = platform::storage::db::get_database()
        .get_pool()
        .acquire()
        .await
        .map_err(|error| membership_error("unavailable", error))?;
    let binding = membership::actor_binding_on(&mut conn, actor_id)
        .await
        .map_err(|error| membership_error("unavailable", error))?;
    let bound = binding.is_some_and(|binding| {
        binding.member_id == signer.member_id
            && (!same_key || binding.client_key_id == signer.key_id)
    });
    if !bound {
        return Err(membership_error(
            "client_unknown",
            "the Actor is not bound to this Realm client key",
        ));
    }
    Ok(())
}

/// Connection ACLs of version 1 act only within the Actor's own Realm: a
/// rule's `source_realm` may be unset, `any_realm` (which matches only the
/// own Realm) or the Actor's own Realm name. A rule naming another Realm, or
/// a malformed Realm name, is rejected.
fn check_acl_source_realms(request: &RegisterRequest) -> Result<(), String> {
    use actr_protocol::acl_rule::SourceRealm;

    let rules = request.acl.iter().flat_map(|acl| &acl.rules);
    for rule in rules {
        if let Some(SourceRealm::Realm(name)) = &rule.source_realm {
            actr_protocol::uri::parse_realm_name(name)
                .map_err(|error| format!("Invalid ACL source_realm: {error}"))?;
            if *name != request.realm {
                return Err(format!(
                    "ACL source_realm {name} is another Realm; cross-Realm rules are not supported"
                ));
            }
        }
    }
    Ok(())
}

/// Persist the registration's ACL policy and pending signaling metadata in
/// one transaction, and drop pending rows whose retention has passed.
///
/// Nothing is kept when the Realm retired after the request was
/// authenticated: retirement removes these rows, and a registration that
/// raced it must not bring them back.
///
/// On failure returns the client-facing error code and message.
async fn persist_registration_state(
    request: &RegisterRequest,
    register_ok: &register_response::RegisterOk,
    signer: &Signer,
) -> Result<(), (u32, String)> {
    let state_error = || (500, "Failed to persist registration state".to_string());

    let pool = platform::storage::db::get_database().get_pool();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(|error| {
        platform::recording::error!("Failed to begin registration transaction: {}", error);
        state_error()
    })?;

    // A revocation that committed after the proof was checked stops the
    // registration; one that commits later disables the Actor recorded here.
    // Likewise an ownership transfer: the Service is registered only by
    // whoever is the owner now, under the current Profile's `package.type`.
    let signer = match membership::check_key_on(&mut tx, &signer.realm, &signer.key_id).await {
        Ok(signer) => signer,
        Err(error) => return Err((error.code.status().into(), error.to_string())),
    };
    check_service_owner(&mut *tx, request, &signer)
        .await
        .map_err(|error| (error.code, error.message))?;

    if let Some(ref acl) = request.acl {
        use actr_protocol::acl_rule::SourceRealm;

        let realm = &register_ok.actr_id.realm;
        let my_type = request.actr_type.to_string_repr();

        let replacement = acl
            .rules
            .iter()
            .map(|rule| {
                let allow = rule.permission == actr_protocol::acl_rule::Permission::Allow as i32;
                let source_realm = match &rule.source_realm {
                    Some(SourceRealm::AnyRealm(_)) => None,
                    Some(SourceRealm::Realm(name)) => Some(name.clone()),
                    None => Some(realm.clone()), // fallback: treat missing as self-realm
                };
                ActorAcl::new_with_source_realm(
                    realm.clone(),
                    source_realm,
                    rule.from_type.to_string_repr(),
                    my_type.clone(),
                    allow,
                )
            })
            .collect::<Vec<_>>();

        ActorAcl::replace_for_target_in_transaction(&mut tx, realm, &my_type, &replacement)
            .await
            .map_err(|error| {
                platform::recording::error!(
                    "Failed to atomically persist ACL rules for {}: {}",
                    my_type,
                    error
                );
                (500, "Failed to persist registration policy".to_string())
            })?;
    }

    let now = crate::renewal::now_secs();

    // Store pending registration (service_spec) for signaling to pick up.
    // It is kept as long as the renewal-token binding that /reissue accepts.
    if request.service_spec.is_some() || request.ws_address.is_some() {
        let spec_blob = request
            .service_spec
            .as_ref()
            .map(prost::Message::encode_to_vec);
        let expires_at = register_ok
            .renewal_token_expires_at
            .map(|timestamp| timestamp.seconds)
            .unwrap_or(now)
            .saturating_add(crate::renewal::REISSUE_BINDING_GRACE_SECS);
        sqlx::query(
            "INSERT OR REPLACE INTO pending_registration \
             (realm, serial_number, service_spec_blob, ws_address, created_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&register_ok.actr_id.realm)
        .bind(register_ok.actr_id.serial_number as i64)
        .bind(spec_blob)
        .bind(request.ws_address.clone())
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            platform::recording::error!(
                "Failed to persist pending registration metadata: {}",
                error
            );
            (500, "Failed to persist registration metadata".to_string())
        })?;
    }

    sqlx::query(
        "DELETE FROM pending_registration WHERE expires_at IS NOT NULL AND expires_at <= ?",
    )
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        platform::recording::error!(
            "Failed to clean expired pending registration metadata: {}",
            error
        );
        (500, "Failed to persist registration metadata".to_string())
    })?;

    // Checked after the writes above, which hold the write lock: a
    // retirement either committed before them and shows here, or commits
    // after this transaction and removes its rows.
    let retired: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM realm WHERE name = ? AND status = 'retired')",
    )
    .bind(&register_ok.actr_id.realm)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| {
        platform::recording::error!("Failed to read Realm state: {}", error);
        state_error()
    })?;
    if retired {
        return Err((403, "Realm is retired".to_string()));
    }

    tx.commit().await.map_err(|error| {
        platform::recording::error!("Failed to commit registration state: {}", error);
        state_error()
    })
}

/// POST /issuer/reissue — re-authenticate and issue credentials for an existing
/// stable ActrId without allocating a new serial number.
///
/// Outcomes travel in the `RegisterResponse` body with HTTP 200, except the
/// attempt contract (`409`, `410`, `425`, see [`crate::idempotency`]).
async fn reissue_credential(
    State(state): State<IssuerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start = std::time::Instant::now();
    let request = match ReissueCredentialRequest::decode(body.clone()) {
        Ok(request) => request,
        Err(err) => {
            platform::recording::error!("Failed to decode ReissueCredentialRequest: {}", err);
            return register_error(400, format!("Invalid protobuf: {err}"));
        }
    };

    // Validate the identity fields before any signing path (same guard as /register).
    if let Err(err) = request.registration.actr_type.validate() {
        return register_error(400, format!("Invalid actor identity: {err}"));
    }

    let registration = &request.registration;
    // Authenticate before saying anything about the Realm or the attempt.
    let signer = match authenticate_client(
        &state,
        &headers,
        ProofOp::Reissue,
        &registration.realm,
        &body,
    )
    .await
    {
        Ok(signer) => signer,
        Err(error) => return register_error(error.code, error.message),
    };

    let lease = match state
        .idempotency
        .claim(
            Operation::Reissue,
            &request.attempt_id,
            &request.actr_id,
            &body,
        )
        .await
    {
        Ok(ClaimOutcome::Acquired(lease)) => lease,
        Ok(ClaimOutcome::Replay(cached)) => {
            let issued = matches!(
                RegisterResponse::decode(cached.body.as_slice()),
                Ok(RegisterResponse {
                    result: Some(register_response::Result::Success(_))
                })
            );
            if issued {
                if let Err(error) = check_binding(&request.actr_id, &signer, false).await {
                    return register_error(error.code, error.message);
                }
                if let Err(err) = state
                    .issuer
                    .recheck_before_replay(&request.actr_id, &signer)
                    .await
                {
                    let (code, message) = aid_error_to_response(err);
                    return register_error(code, message);
                }
            }
            return protobuf_response(cached.status, cached.body);
        }
        Err(error) => {
            return attempt_error_response(error, register_error_body);
        }
    };

    if let Err(validation_err) = RealmEntity::validate_realm(&registration.realm).await {
        let (code, message) = RealmEntity::map_validation_error(validation_err);
        let body = register_error_body(code, format!("Realm validation failed: {message}"));
        return complete_attempt(&state, &lease, StatusCode::OK, body, register_error_body).await;
    }

    let preparation = state
        .issuer
        .prepare_reissue(
            &request.actr_id,
            registration,
            request.renewal_proof.as_deref(),
            &signer,
        )
        .await;

    let (response, succeeded) = match preparation {
        ReissuePreparation::Ready(prepared) => match state
            .idempotency
            .complete_reissue(&lease, &request.actr_id, *prepared)
            .await
        {
            Ok(cached) => (protobuf_response(cached.status, cached.body), true),
            Err(AttemptError::Refused(err)) => {
                release_attempt(&state, &lease).await;
                let (code, message) = aid_error_to_response(err);
                (register_error(code, message), false)
            }
            Err(error) => {
                release_attempt(&state, &lease).await;
                (attempt_error_response(error, register_error_body), false)
            }
        },
        ReissuePreparation::Rejected(response) => {
            let body = response.encode_to_vec();
            let response =
                complete_attempt(&state, &lease, StatusCode::OK, body, register_error_body).await;
            (response, false)
        }
        ReissuePreparation::Retryable(response) => {
            release_attempt(&state, &lease).await;
            (protobuf_response(200, response.encode_to_vec()), false)
        }
    };

    if let Some(ref counters) = state.counters {
        counters
            .record_request(succeeded, start.elapsed().as_secs_f64() * 1000.0)
            .await;
    }
    if succeeded {
        platform::recording::info!(
            "Credential reissued for actor {}",
            request.actr_id.to_string_repr()
        );
        // The Actor may have moved to this key from a revoked one.
        platform::realm::changes::announce_actor(&request.actr_id);
    }
    response
}

/// POST /issuer/renew — renew credentials for an existing ActrId
///
/// Processing flow:
/// 1. Decode `RenewCredentialRequest` and claim its attempt.
/// 2. Validate the token length.
/// 3. Validate the renewal token (read-only) and prepare its rotation.
/// 4. Check the Realm state; an unauthenticated caller never learns it.
/// 5. Re-issue access and TURN credentials for the original ActrId.
/// 6. Commit the rotation together with the response, rechecking the actor.
async fn renew_credential(
    State(state): State<IssuerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start = std::time::Instant::now();

    // 1. Decode protobuf request and claim the attempt.
    let request = match RenewCredentialRequest::decode(body.clone()) {
        Ok(req) => req,
        Err(err) => {
            platform::recording::error!("Failed to decode RenewCredentialRequest: {}", err);
            return renew_error(400, format!("Invalid protobuf: {err}"));
        }
    };
    let actor_id = &request.actr_id;

    // The client proof is checked before the renewal token.
    let signer =
        match authenticate_client(&state, &headers, ProofOp::Renew, &actor_id.realm, &body).await {
            Ok(signer) => signer,
            Err(error) => return renew_error(error.code, error.message),
        };

    let lease = match state
        .idempotency
        .claim(Operation::Renew, &request.attempt_id, actor_id, &body)
        .await
    {
        Ok(ClaimOutcome::Acquired(lease)) => lease,
        Ok(ClaimOutcome::Replay(cached)) => {
            // Only a stored credential is rechecked; a stored rejection is
            // replayed as is, so it tells nothing new about the actor.
            let issued = matches!(
                RenewCredentialResponse::decode(cached.body.as_slice()),
                Ok(RenewCredentialResponse {
                    result: Some(renew_credential_response::Result::Success(_))
                })
            );
            if issued {
                if let Err(error) = check_binding(actor_id, &signer, true).await {
                    return renew_error(error.code, error.message);
                }
                if let Err(err) = state.issuer.recheck_before_replay(actor_id, &signer).await {
                    let (code, message) = aid_error_to_response(err);
                    return renew_error(code, message);
                }
            }
            return protobuf_response(cached.status, cached.body);
        }
        Err(error) => {
            return attempt_error_response(error, renew_error_body);
        }
    };

    // 2. Validate token length (32 bytes expected).
    if request.renewal_token.len() != 32 {
        platform::recording::warn!(
            "Renewal token has invalid length: {} bytes",
            request.renewal_token.len()
        );
        let body = renew_error_body(400, "Invalid renewal token length".to_string());
        return complete_attempt(
            &state,
            &lease,
            StatusCode::BAD_REQUEST,
            body,
            renew_error_body,
        )
        .await;
    }

    platform::recording::debug!(
        "Received renewal request for actor {}",
        actor_id.to_string_repr()
    );

    // 3. Validate the token and prepare its rotation; nothing is written yet.
    let issuer = &state.issuer;
    let prepared = match crate::renewal::prepare_renewal_token_rotation(
        actor_id,
        &request.renewal_token,
        &issuer.config.renewal_token_secret,
        issuer.config.renewal_rotation_window_secs,
        issuer.config.renewal_token_ttl_secs,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(crate::renewal::RenewalError::TokenRejected) => {
            platform::recording::warn!(
                "Renewal token rejected for actor {}",
                actor_id.to_string_repr()
            );
            let body = renew_error_body(401, "Renewal token invalid or expired".into());
            return complete_attempt(
                &state,
                &lease,
                StatusCode::UNAUTHORIZED,
                body,
                renew_error_body,
            )
            .await;
        }
        Err(crate::renewal::RenewalError::StoreError(msg)) => {
            platform::recording::error!(
                "Renewal store error for actor {}: {}",
                actor_id.to_string_repr(),
                msg
            );
            release_attempt(&state, &lease).await;
            return renew_error(500, "Internal renewal store error".to_string());
        }
    };

    // Only the key the Actor is bound to renews it.
    if let Err(error) = check_binding(actor_id, &signer, true).await {
        release_attempt(&state, &lease).await;
        return renew_error(error.code, error.message);
    }

    // 4. Only an authenticated caller learns the Realm state.
    if let Err(validation_err) = RealmEntity::validate_realm(&actor_id.realm).await {
        let (code, message) = RealmEntity::map_validation_error(validation_err);
        let body = renew_error_body(code, format!("Realm validation failed: {message}"));
        return complete_attempt(
            &state,
            &lease,
            status_for_error_code(code),
            body,
            renew_error_body,
        )
        .await;
    }

    // 5. Re-issue access and TURN credentials for the original ActrId, after
    // rechecking its recorded identity, provenance and current authorization.
    let mut register_ok = match issuer.issue_credential_for_actor(actor_id, &signer).await {
        Ok(ok) => ok,
        Err(err) => {
            platform::recording::error!(
                "Failed to re-issue credential for actor {}: {}",
                actor_id.to_string_repr(),
                err
            );
            release_attempt(&state, &lease).await;
            let (code, message) = aid_error_to_response(err);
            return renew_error(code, message);
        }
    };

    // 6. Attach the renewal token and commit it with the response.
    let (token, expires_at) = match &prepared.outcome {
        crate::renewal::RotationOutcome::Unchanged { token, expires_at }
        | crate::renewal::RotationOutcome::Rotated { token, expires_at } => {
            (token.clone(), *expires_at)
        }
    };
    register_ok.renewal_token = Some(token);
    register_ok.renewal_token_expires_at = Some(prost_types::Timestamp {
        seconds: expires_at,
        nanos: 0,
    });

    let response_expires_at = register_ok_response_expiry(&register_ok);
    let body = RenewCredentialResponse {
        result: Some(renew_credential_response::Result::Success(register_ok)),
    }
    .encode_to_vec();
    if let Err(error) = state
        .idempotency
        .complete_renewal(
            &lease,
            actor_id,
            &prepared,
            &body,
            response_expires_at,
            &signer,
        )
        .await
    {
        platform::recording::warn!(
            "Failed to commit credential renewal for actor {}: {}",
            actor_id.to_string_repr(),
            error
        );
        release_attempt(&state, &lease).await;
        if let AttemptError::Refused(err) = error {
            let (code, message) = aid_error_to_response(err);
            return renew_error(code, message);
        }
        return attempt_error_response(error, renew_error_body);
    }

    if let Some(ref ctr) = state.counters {
        ctr.record_request(true, start.elapsed().as_secs_f64() * 1000.0)
            .await;
    }

    platform::recording::info!("Credential renewed for actor {}", actor_id.to_string_repr());

    protobuf_response(StatusCode::OK.as_u16(), body)
}

/// Health check endpoint
///
/// Checks the database and the Signer, which signs every credential with
/// the Realm's issuer key, and reports how many Realm issuer keys are loaded.
async fn health_check(State(state): State<IssuerState>) -> Json<Value> {
    let mut checks = json!({
        "service": "issuer",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "healthy"
    });

    // Check database connection
    let db_status = match state.issuer.check_database_health().await {
        Ok(()) => "ok",
        Err(e) => {
            platform::recording::error!("Database health check failed: {}", e);
            checks["status"] = json!("degraded");
            "failed"
        }
    };
    checks["database"] = json!(db_status);

    // Check Signer service connectivity
    let signer_status = match state.issuer.check_signer_health().await {
        Ok(()) => "ok",
        Err(e) => {
            platform::recording::error!("Signer health check failed: {}", e);
            checks["status"] = json!("degraded");
            "failed"
        }
    };
    checks["signer_service"] = json!(signer_status);
    checks["realm_keys"] = json!(state.issuer.realm_key_count().await);

    Json(checks)
}

/// Encode a RegisterResponse as protobuf bytes
fn encode_result(result: RegisterResponse) -> Bytes {
    let mut buf = Vec::new();
    if let Err(err) = result.encode(&mut buf) {
        platform::recording::error!("Failed to encode RegisterResponse: {}", err);
        // Return an ErrorResponse for encoding failure
        let error_result = RegisterResponse {
            result: Some(register_response::Result::Error(ErrorResponse {
                code: 500,
                message: format!("Failed to encode response: {err}"),
            })),
        };
        let mut fallback_buf = Vec::new();
        let _ = error_result.encode(&mut fallback_buf);
        return Bytes::from(fallback_buf);
    }
    Bytes::from(buf)
}

fn protobuf_response(status: u16, body: Vec<u8>) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(CONTENT_TYPE, "application/x-protobuf")],
        Bytes::from(body),
    )
        .into_response()
}

fn register_error_body(code: u32, message: String) -> Vec<u8> {
    RegisterResponse {
        result: Some(register_response::Result::Error(ErrorResponse {
            code,
            message,
        })),
    }
    .encode_to_vec()
}

fn renew_error_body(code: u32, message: String) -> Vec<u8> {
    RenewCredentialResponse {
        result: Some(renew_credential_response::Result::Error(ErrorResponse {
            code,
            message,
        })),
    }
    .encode_to_vec()
}

/// `/reissue` reports errors in the body with HTTP 200.
fn register_error(code: u32, message: String) -> Response {
    protobuf_response(200, register_error_body(code, message))
}

/// `/renew` reports errors in the body and as the HTTP status.
fn renew_error(code: u32, message: String) -> Response {
    protobuf_response(
        status_for_error_code(code).as_u16(),
        renew_error_body(code, message),
    )
}

/// How long a rejection stays the answer to its attempt.
const REJECTED_ATTEMPT_RESPONSE_TTL_SECS: i64 = 5 * 60;

/// Keep a stable rejection as the attempt's response and return it.
/// `error_body` builds the route's error message should that fail.
async fn complete_attempt(
    state: &IssuerState,
    lease: &AttemptLease,
    status: StatusCode,
    body: Vec<u8>,
    error_body: fn(u32, String) -> Vec<u8>,
) -> Response {
    let expires_at = crate::renewal::now_secs().saturating_add(REJECTED_ATTEMPT_RESPONSE_TTL_SECS);
    match state
        .idempotency
        .complete_response(lease, status.as_u16(), &body, expires_at)
        .await
    {
        Ok(()) => protobuf_response(status.as_u16(), body),
        Err(error) => {
            release_attempt(state, lease).await;
            attempt_error_response(error, error_body)
        }
    }
}

/// Let the same attempt be retried after a failure that changed nothing.
async fn release_attempt(state: &IssuerState, lease: &AttemptLease) {
    if let Err(error) = state.idempotency.release(lease).await {
        platform::recording::warn!("Failed to release control attempt: {}", error);
    }
}

/// Map an attempt failure to its HTTP status, with the error in a body built
/// by `error_body` (the route's response message).
fn attempt_error_response(error: AttemptError, error_body: fn(u32, String) -> Vec<u8>) -> Response {
    let (status, message, retry_after_secs) = match &error {
        AttemptError::InvalidAttemptId => (
            StatusCode::BAD_REQUEST,
            "attempt_id must contain exactly 16 UUID bytes",
            None,
        ),
        AttemptError::Conflict => (
            StatusCode::CONFLICT,
            "attempt_id was already used with a different request",
            None,
        ),
        AttemptError::Expired => (
            StatusCode::GONE,
            "idempotency result retention has expired; start a new attempt",
            None,
        ),
        AttemptError::InProgress { retry_after_secs } => (
            StatusCode::TOO_EARLY,
            "the same control-plane attempt is still in progress",
            Some(*retry_after_secs),
        ),
        AttemptError::LeaseLost => (
            StatusCode::TOO_EARLY,
            "the same control-plane attempt is still in progress",
            Some(1),
        ),
        AttemptError::TokenRejected => (
            StatusCode::UNAUTHORIZED,
            "renewal token invalid or expired",
            None,
        ),
        AttemptError::PossessionNotProven => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "actor identity possession not proven",
            None,
        ),
        AttemptError::Refused(_) => (StatusCode::FORBIDDEN, "credential refused", None),
        AttemptError::Store(_) | AttemptError::Crypto(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal control-attempt store error",
            None,
        ),
    };
    if status.is_server_error() {
        platform::recording::error!("Control-attempt failure: {}", error);
    }
    let mut response = protobuf_response(
        status.as_u16(),
        error_body(u32::from(status.as_u16()), message.to_string()),
    );
    if let Some(seconds) = retry_after_secs
        && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
    {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn status_for_error_code(code: u32) -> StatusCode {
    match code {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        500 => StatusCode::INTERNAL_SERVER_ERROR,
        502 => StatusCode::BAD_GATEWAY,
        503 => StatusCode::SERVICE_UNAVAILABLE,
        504 => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Convert an AidError into a (code, message) tuple
fn aid_error_to_response(err: AidError) -> (u32, String) {
    let code = match &err {
        // Client errors (4xx)
        AidError::InvalidFormat => 400,
        AidError::InvalidPrefix => 400,
        AidError::EmptyId => 400,
        AidError::InvalidTimestamp(_) => 400,
        AidError::Base64DecodeError(_) => 400,
        AidError::HexDecodeError(_) => 400,
        AidError::Expired => 401,
        AidError::RealmError(_) => 403, // Forbidden
        AidError::PublisherNotVerified => 403,
        AidError::Membership(_) => 403,

        // Server errors (5xx)
        AidError::GenerationFailed(msg) => {
            // If the Signer is unavailable, return 503 (Service Unavailable)
            if msg.contains("Signer unavailable") || msg.contains("Signer service") {
                503
            } else {
                500
            }
        }
        AidError::InvalidSignature(_) => 500,
        AidError::DecodeFailure(_) => 500,
        // AidError is #[non_exhaustive]; future variants default to 5xx.
        _ => 500,
    };
    (code, err.to_string())
}

/// Convert an AidError into a proto ErrorResponse
///
/// Error code mapping strategy:
/// - 4xx: client errors (format, expired, validation failed)
/// - 5xx: server errors (generation failed, internal error)
fn aid_error_to_error_response(err: AidError) -> ErrorResponse {
    let (code, message) = aid_error_to_response(err);
    ErrorResponse { code, message }
}

/// The enrollment request proves both possession of the Realm client key and
/// the actor's private renewal token. Neither proof is sent to the relay.
async fn relay_identity(
    State(state): State<IssuerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<actr_relay_security::IssuedIdentity>, StatusCode> {
    let request: actr_relay_security::IdentityRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let actor = actr_protocol::ActrId::from_string_repr(&request.endpoint)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.endpoint.len() > 512
        || actor.to_string_repr() != request.endpoint
        || request.public_key == [0; 32]
        || request.admission_commitment == [0; 32]
        || request.renewal_token.len() != 32
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let signer = authenticate_client(
        &state,
        &headers,
        ProofOp::RelayIdentity,
        &actor.realm,
        &body,
    )
    .await
    .map_err(|_| StatusCode::UNAUTHORIZED)?;
    check_binding(&actor, &signer, true)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let actor_type = state
        .issuer
        .relay_actor_type(&actor, &signer)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let hash = crate::renewal::hash_token(&request.renewal_token);
    let expires = crate::renewal::find_binding_token_expiry(&actor, &hash)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter(|expires| *expires > crate::renewal::now_secs())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let issued = state
        .issuer
        .mint_relay_identity(
            &actor,
            &actor_type,
            &signer,
            request.public_key,
            request.admission_commitment,
            expires as u64,
        )
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Recheck after signer I/O so revocation and proof rotation fail closed.
    check_binding(&actor, &signer, true)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let current_type = state
        .issuer
        .relay_actor_type(&actor, &signer)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if current_type != actor_type {
        return Err(StatusCode::FORBIDDEN);
    }
    if crate::renewal::find_binding_token_expiry(&actor, &hash)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none_or(|expires| expires <= crate::renewal::now_secs())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(issued))
}

async fn relay_signing_key(
    State(state): State<IssuerState>,
    axum::extract::Path((realm, key_id)): axum::extract::Path<(String, u32)>,
) -> Result<Json<actr_relay_security::IssuerKey>, StatusCode> {
    use platform::aid::credential::AIdCredentialValidator;
    AIdCredentialValidator::init(&state.issuer.config.sqlite_path)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (key, expires_at) = AIdCredentialValidator::get_verifying_key(&realm, key_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(actr_relay_security::IssuerKey {
        key_id,
        public_key: *key.as_bytes(),
        expires_at: expires_at.min(
            (crate::renewal::now_secs() as u64)
                .saturating_add(actr_relay_security::CERTIFICATE_TTL),
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actr_protocol::{AIdCredential, ActrType, RegisterAuthMode};
    use prost::bytes::Bytes as ProstBytes;

    #[test]
    fn test_protobuf_request_encoding_decoding() {
        // Test full protobuf request encoding/decoding
        let actr_type = ActrType {
            publisher: "apple".to_string(),
            name: "iPhone15".to_string(),
            version: "1.0.0".to_string(),
        };

        let realm = "k7m2q9xh@example.com".to_string();

        let request = RegisterRequest {
            actr_type,
            realm,
            service: None,
            service_spec: None,
            acl: None,
            ws_address: None,
            manifest_raw: None,
            publisher_signature: None,
            target: None,
            auth_mode: Some(RegisterAuthMode::Package as i32),
            signing_public_key: None,
        };

        // Encode
        let mut buf = Vec::new();
        request.encode(&mut buf).unwrap();

        // Decode
        let decoded_request = RegisterRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded_request.realm, "k7m2q9xh@example.com");
        assert_eq!(decoded_request.actr_type.publisher, "apple");
        assert_eq!(decoded_request.actr_type.name, "iPhone15");
    }

    #[tokio::test]
    async fn registration_state_is_not_kept_for_a_realm_retired_meanwhile() {
        use actr_protocol::{AclRule, acl_rule::Permission};

        crate::idempotency::tests::setup_db().await;
        let realm = "raced@example.com";
        RealmEntity::create(realm.to_string()).await.unwrap();
        let signer = crate::test_support::owner_of(realm, 1).await;
        // The request was authenticated; then the Realm retires.
        RealmEntity::retire(realm).await.unwrap();

        let actr_type = ActrType {
            publisher: "acme".to_string(),
            name: "worker".to_string(),
            version: "1.0.0".to_string(),
        };
        let request = RegisterRequest {
            actr_type: actr_type.clone(),
            realm: realm.to_string(),
            acl: Some(actr_protocol::Acl {
                rules: vec![AclRule {
                    permission: Permission::Allow as i32,
                    from_type: actr_type,
                    source_realm: None,
                }],
            }),
            ws_address: Some("ws://127.0.0.1:1".to_string()),
            ..Default::default()
        };
        let register_ok = register_response::RegisterOk {
            actr_id: actr_protocol::ActrId {
                realm: realm.to_string(),
                serial_number: 7,
            },
            ..Default::default()
        };

        assert_eq!(
            persist_registration_state(&request, &register_ok, &signer).await,
            Err((403, "Realm is retired".to_string()))
        );
        let pool = platform::storage::db::get_database().get_pool();
        for statement in [
            "SELECT COUNT(*) FROM actoracl WHERE realm = ?",
            "SELECT COUNT(*) FROM pending_registration WHERE realm = ?",
        ] {
            let rows: i64 = sqlx::query_scalar(statement)
                .bind(realm)
                .fetch_one(pool)
                .await
                .unwrap();
            assert_eq!(rows, 0, "{statement}");
        }
    }

    #[tokio::test]
    async fn service_registration_needs_the_current_owner() {
        use actr_protocol::{AclRule, acl_rule::Permission};

        crate::idempotency::tests::setup_db().await;
        let realm = "handover@example.com";
        RealmEntity::create(realm.to_string()).await.unwrap();
        let owner = crate::test_support::owner_of(realm, 1).await;
        let editor = crate::test_support::member_of(&owner, Role::Editor, 2).await;
        let pool = platform::storage::db::get_database().get_pool();
        let profile = json!({ "package": { "type": "acme.com/notes@1.0.0" } });
        sqlx::query(
            "INSERT INTO realm_profile (realm, sequence, digest, document, accepted_at)
             VALUES (?, 1, 'd', ?, 0)",
        )
        .bind(realm)
        .bind(profile.to_string())
        .execute(pool)
        .await
        .unwrap();

        let service = ActrType {
            publisher: "acme.com".to_string(),
            name: "notes".to_string(),
            version: "1.0.0".to_string(),
        };
        let request = RegisterRequest {
            actr_type: service.clone(),
            realm: realm.to_string(),
            acl: Some(actr_protocol::Acl {
                rules: vec![AclRule {
                    permission: Permission::Allow as i32,
                    from_type: service,
                    source_realm: None,
                }],
            }),
            ws_address: Some("ws://127.0.0.1:1".to_string()),
            ..Default::default()
        };
        let register_ok = register_response::RegisterOk {
            actr_id: actr_protocol::ActrId {
                realm: realm.to_string(),
                serial_number: 7,
            },
            ..Default::default()
        };
        let rows = || async {
            let mut counts = Vec::new();
            for statement in [
                "SELECT COUNT(*) FROM actoracl WHERE realm = ?",
                "SELECT COUNT(*) FROM pending_registration WHERE realm = ?",
            ] {
                let count: i64 = sqlx::query_scalar(statement)
                    .bind(realm)
                    .fetch_one(pool)
                    .await
                    .unwrap();
                counts.push(count);
            }
            counts
        };

        // The owner's request passed the entry check; then ownership moves.
        assert!(check_service_owner(pool, &request, &owner).await.is_ok());
        membership::transfer_owner(&owner, &editor.member_id, Role::Editor)
            .await
            .unwrap();

        let (code, message) = persist_registration_state(&request, &register_ok, &owner)
            .await
            .unwrap_err();
        assert_eq!(code, 403);
        assert!(message.contains("service_owner_only"), "{message}");
        assert_eq!(rows().await, [0, 0]);

        // The new owner registers it, though authenticated as an editor.
        persist_registration_state(&request, &register_ok, &editor)
            .await
            .unwrap();
        assert_eq!(rows().await, [1, 1]);
    }

    #[test]
    fn acl_rules_may_not_name_another_realm() {
        use actr_protocol::acl_rule::{Permission, SourceRealm};

        let request = |source_realm: Option<SourceRealm>| RegisterRequest {
            actr_type: ActrType {
                publisher: "test".to_string(),
                name: "actor".to_string(),
                version: "1.0.0".to_string(),
            },
            realm: "k7m2q9xh@example.com".to_string(),
            acl: Some(actr_protocol::Acl {
                rules: vec![actr_protocol::AclRule {
                    permission: Permission::Allow as i32,
                    from_type: ActrType {
                        publisher: "test".to_string(),
                        name: "client".to_string(),
                        version: "1.0.0".to_string(),
                    },
                    source_realm,
                }],
            }),
            ..Default::default()
        };

        for accepted in [
            None,
            Some(SourceRealm::AnyRealm(true)),
            Some(SourceRealm::Realm("k7m2q9xh@example.com".to_string())),
        ] {
            assert!(check_acl_source_realms(&request(accepted)).is_ok());
        }
        assert!(check_acl_source_realms(&RegisterRequest::default()).is_ok());
        for rejected in [
            "p3n8w2rt@example.com",
            "k7m2q9xh@other.example",
            "K7M2Q9XH@example.com",
            "not a realm",
            "",
        ] {
            assert!(
                check_acl_source_realms(&request(Some(SourceRealm::Realm(rejected.to_string()))))
                    .is_err(),
                "{rejected:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_protobuf_minimal_request() {
        // Test a minimal-fields protobuf request
        let request = RegisterRequest {
            actr_type: ActrType {
                publisher: "test".to_string(),
                name: "actor".to_string(),
                version: "1.0.0".to_string(),
            },
            realm: "test@example.com".to_string(),
            service: None,
            service_spec: None,
            acl: None,
            ws_address: None,
            manifest_raw: None,
            publisher_signature: None,
            target: None,
            auth_mode: Some(RegisterAuthMode::Package as i32),
            signing_public_key: None,
        };

        // Encode-decode round-trip
        let mut buf = Vec::new();
        request.encode(&mut buf).unwrap();
        let decoded_request = RegisterRequest::decode(buf.as_slice()).unwrap();

        assert_eq!(decoded_request.realm, "test@example.com");
        assert_eq!(decoded_request.actr_type.publisher, "test");
        assert_eq!(decoded_request.actr_type.name, "actor");
    }

    #[test]
    fn test_register_response_success() {
        use actr_protocol::{ActrId, register_response::RegisterOk};
        use prost_types::Timestamp;

        // Test a successful RegisterResponse
        let register_ok = RegisterOk {
            actr_id: ActrId {
                realm: "test@example.com".to_string(),
                serial_number: 123456,
            },
            credential: AIdCredential {
                key_id: 42,
                claims: ProstBytes::from(vec![1, 2, 3]),
                signature: ProstBytes::from(vec![0u8; 64]),
            },
            turn_credential: actr_protocol::TurnCredential::default(),
            signing_pubkey: ProstBytes::from(vec![0u8; 32]),
            signing_key_id: 42,
            credential_expires_at: Some(Timestamp {
                seconds: 1234567890,
                nanos: 0,
            }),
            signaling_heartbeat_interval_secs: 30,
            renewal_token: None,
            renewal_token_expires_at: None,
        };

        let response = RegisterResponse {
            result: Some(register_response::Result::Success(register_ok)),
        };

        // Encode-decode round-trip
        let mut buf = Vec::new();
        response.encode(&mut buf).unwrap();
        let decoded_response = RegisterResponse::decode(buf.as_slice()).unwrap();

        assert!(decoded_response.result.is_some());
        if let Some(register_response::Result::Success(resp)) = decoded_response.result {
            assert_eq!(resp.actr_id.realm, "test@example.com");
            assert_eq!(resp.actr_id.serial_number, 123456);
            assert_eq!(resp.credential.key_id, 42);
            assert_eq!(resp.signaling_heartbeat_interval_secs, 30);
        } else {
            panic!("Expected success result");
        }
    }

    #[test]
    fn test_register_response_error() {
        // Test an error RegisterResponse
        let error = ErrorResponse {
            code: 400,
            message: "Bad request".to_string(),
        };

        let response = RegisterResponse {
            result: Some(register_response::Result::Error(error)),
        };

        // Encode-decode round-trip
        let mut buf = Vec::new();
        response.encode(&mut buf).unwrap();
        let decoded_response = RegisterResponse::decode(buf.as_slice()).unwrap();

        assert!(decoded_response.result.is_some());
        if let Some(register_response::Result::Error(err)) = decoded_response.result {
            assert_eq!(err.code, 400);
            assert_eq!(err.message, "Bad request");
        } else {
            panic!("Expected error result");
        }
    }

    #[test]
    fn test_invalid_format_maps_to_400() {
        let error = aid_error_to_error_response(AidError::InvalidFormat);
        assert_eq!(error.code, 400);
    }
}
