//! AId token issuer.
//!
//! # Responsibilities
//!
//! Handles `RegisterRequest` and builds `RegisterResponse`, including:
//! - Serial number allocation with Snowflake.
//! - Signed access credential issuance.
//! - Time-limited TURN credential generation.
//! - Renewal token issuance and hash persistence.
//!
//! # Signing keys
//!
//! Each Realm's credentials are signed with that Realm's current issuer key
//! in the Signer ([`AIdIssuer::realm_key`]; the first is `issuer:<realm>`),
//! one of the keys its Realm Profile lists under `issuer.keys`. The keys
//! rotate as `realm-profile.md` section 6.1 describes; the private keys never
//! leave the Signer, which signs through its Sign RPC. Verifiers cache the
//! verifying keys under `(realm, key_id)`. When a Realm is retired its keys
//! are retired in the Signer and never sign again.
//!
//! # Example
//!
//! ```no_run
//! use issuer::issuer::{AIdIssuer, IssuerConfig};
//! use issuer::signer_client_wrapper::{SignerClientWrapper, create_signer_client};
//! use platform::config::signer::SignerClientConfig;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let signer_config = SignerClientConfig {
//!     endpoint: "http://localhost:8080".to_string(),
//!     timeout_seconds: 30,
//!     enable_tls: false,
//!     tls_domain: None,
//!     ca_cert: None,
//!     client_cert: None,
//!     client_key: None,
//! };
//! let signer_client = create_signer_client(&signer_config, "shared-key").await?;
//! let config = IssuerConfig::default();
//!
//! let issuer = AIdIssuer::new(signer_client, config, tokio_util::sync::CancellationToken::new()).await?;
//!
//! // Process a registration request
//! // let response = issuer.issue_credential(&request).await?;
//! # Ok(())
//! # }
//! ```

use crate::publisher_profiles::PublisherProfileRefresher;
use crate::signer_client_wrapper::SignerClientWrapper;
use crate::sn::{AIdSerialNumberIssuer, SerialNumber};

use actr_protocol::{
    AIdCredential, ActrId, ActrType, ErrorResponse, IdentityClaims, RegisterRequest,
    RegisterResponse, register_response,
};
use base64::prelude::*;
use ed25519_dalek::VerifyingKey;
use hmac::{Hmac, Mac};
use platform::aid::AidError;
use platform::realm::membership::Signer;
use prost::Message as ProstMessage;
use prost::bytes::Bytes;
use prost_types::Timestamp;
use sha1::Sha1;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

type HmacSha1 = Hmac<Sha1>;

/// Wire code carried in `RegisterResponse::Error` when a `/reissue` caller fails
/// to prove possession of the requested `ActrId` (a missing, unknown, or
/// grace-expired binding proof).
///
/// Deliberately distinct from the realm-denial `403`: a possession failure is a
/// per-actor identity problem, not a realm-membership decision. Conflating the
/// two behind `403` made the client treat a recoverable possession failure as a
/// terminal realm denial and permanently stop recovery. The hyper membership
/// controller maps this code to a retryable / identity-loss outcome, while `403`
/// stays reserved for a genuine realm denial that terminates automatic recovery.
pub const REISSUE_POSSESSION_NOT_PROVEN_CODE: u32 = 422;

fn aid_error_to_code(err: &AidError) -> u32 {
    match err {
        AidError::InvalidFormat
        | AidError::InvalidPrefix
        | AidError::EmptyId
        | AidError::InvalidTimestamp(_)
        | AidError::Base64DecodeError(_)
        | AidError::HexDecodeError(_) => 400,
        AidError::Expired => 401,
        AidError::RealmError(_) | AidError::PublisherNotVerified | AidError::Membership(_) => 403,
        AidError::GenerationFailed(msg) => {
            if msg.contains("Signer unavailable") || msg.contains("Signer service") {
                503
            } else {
                500
            }
        }
        AidError::InvalidSignature(_) | AidError::DecodeFailure(_) => 500,
        // AidError is #[non_exhaustive]; future variants default to 5xx.
        _ => 500,
    }
}

/// Failure of a credential reissue attempt.
///
/// Possession failures are deliberately kept Issuer-internal instead of widening
/// [`AidError`]: that enum is publicly re-exported and exhaustive, so adding a
/// variant would break downstream matches. The endpoint boundary maps this to
/// the wire status instead.
enum ReissueFailure {
    /// Anything the shared credential machinery already classifies.
    Aid(AidError),
    /// The caller did not prove possession of the requested `ActrId`.
    PossessionNotProven,
}

impl From<AidError> for ReissueFailure {
    fn from(err: AidError) -> Self {
        Self::Aid(err)
    }
}

/// A possession proof that matched an issuer-recorded binding and has not
/// been consumed yet.
struct VerifiedPossession {
    proof_hash: Vec<u8>,
    binding_expires_at: i64,
    /// The token derived from the proof and its recorded expiry; its row, once
    /// written, marks the proof as consumed.
    successor: Vec<u8>,
}

/// Where a verified package registration comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provenance {
    /// Signed by a trusted key of the Publisher named in the manifest.
    Publisher,
    /// Signed by a key that is not a trusted Publisher key: a local
    /// development identity.
    Development,
}

/// What Issuer recorded when it issued an Actor identity.
pub(crate) struct IssuedActor {
    pub(crate) actor_type: ActrType,
    pub(crate) auth_mode: Option<i32>,
    pub(crate) development: bool,
}

/// Reissue work prepared without writing identity state. The attempt journal
/// commits it together with the response
/// (`ControlAttemptStore::complete_reissue`).
pub(crate) struct PreparedReissue {
    /// Signed credentials; the renewal token is attached on commit.
    pub(crate) register_ok: register_response::RegisterOk,
    pub(crate) successor_token: Vec<u8>,
    pub(crate) successor_hash: Vec<u8>,
    pub(crate) successor_expires_at: i64,
    pub(crate) proof_hash: Vec<u8>,
    pub(crate) binding_expires_at: i64,
    /// Package-auth reissue: the target to record.
    pub(crate) package: bool,
    pub(crate) target: Option<String>,
    /// The key the Actor is bound to once the reissue commits.
    pub(crate) signer: Signer,
}

/// Result of preparing a reissue.
pub(crate) enum ReissuePreparation {
    Ready(Box<PreparedReissue>),
    /// A stable rejection, kept as the attempt's response.
    Rejected(RegisterResponse),
    /// An infrastructure failure; the attempt is released so the same body can
    /// be retried.
    Retryable(RegisterResponse),
}

impl ReissueFailure {
    /// Map to the `(code, message)` pair carried in `RegisterResponse`.
    fn to_response(&self) -> (u32, String) {
        match self {
            Self::Aid(err) => (aid_error_to_code(err), err.to_string()),
            Self::PossessionNotProven => (
                REISSUE_POSSESSION_NOT_PROVEN_CODE,
                "actor identity possession not proven".to_string(),
            ),
        }
    }
}

/// AId Token issuer configuration.
#[derive(Debug, Clone)]
pub struct IssuerConfig {
    /// Stable Snowflake worker identity (0..=31).
    pub worker_id: u8,
    /// Token TTL (seconds).
    pub token_ttl_secs: u64,
    /// Signaling server heartbeat interval (seconds).
    pub signaling_heartbeat_interval_secs: u32,
    /// TURN shared secret (shared with the TURN server, used to generate time-limited credentials).
    pub turn_secret: String,
    /// Actr Facility Provider ID bound into credentials and TURN usernames.
    pub provider_id: String,
    /// SQLite data directory (shared with signaling, used to write the verifying key
    /// so signaling can verify credentials).
    ///
    /// Issuer writes each Realm issuer key's verifying key to
    /// `signaling_key_cache.db` under this directory, allowing the signaling
    /// `AIdCredentialValidator` to verify credentials.
    pub sqlite_path: std::path::PathBuf,
    /// Renewal token TTL in seconds.
    pub renewal_token_ttl_secs: u64,
    /// Renewal rotation window in seconds.
    pub renewal_rotation_window_secs: u64,
    /// Base64-decoded renewal token secret (at least 32 bytes).
    pub renewal_token_secret: Vec<u8>,
}

impl Default for IssuerConfig {
    fn default() -> Self {
        Self {
            worker_id: 0,
            token_ttl_secs: 3600,                  // 1 hour
            signaling_heartbeat_interval_secs: 30, // 30 seconds
            turn_secret: String::new(),
            provider_id: String::new(),
            sqlite_path: std::path::PathBuf::from("."),
            renewal_token_ttl_secs: 86400,
            renewal_rotation_window_secs: 21600,
            renewal_token_secret: Vec::new(),
        }
    }
}

/// Current Unix time in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// AId Token issuer - focused on issuing new Actor Identity Tokens
pub struct AIdIssuer {
    signer_client: SignerClientWrapper,
    /// Each Realm's current issuer key, which signs that Realm's
    /// credentials: Signer key id and verifying key.
    realm_keys: Arc<RwLock<HashMap<String, (u32, VerifyingKey)>>>,
    /// Issuer configuration (public for renewal handler access).
    pub config: IssuerConfig,
    /// Registration-time Publisher Profile freshness; `None` leaves package
    /// provenance to the Publisher Registry row alone (isolated test setups).
    publisher_profiles: Option<Arc<PublisherProfileRefresher>>,
}

impl AIdIssuer {
    /// Create a new AIdIssuer.
    pub async fn new(
        signer_client: SignerClientWrapper,
        config: IssuerConfig,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Self, AidError> {
        if config.turn_secret.len() < 32 {
            return Err(AidError::GenerationFailed(
                "TURN secret must contain at least 32 bytes".to_string(),
            ));
        }
        if config.renewal_token_secret.len() < 32 {
            return Err(AidError::GenerationFailed(
                "renewal token secret must contain at least 32 bytes".to_string(),
            ));
        }
        SerialNumber::configure_worker_id(u64::from(config.worker_id))
            .map_err(|error| AidError::GenerationFailed(error.to_string()))?;

        let issuer = Self {
            signer_client,
            realm_keys: Arc::new(RwLock::new(HashMap::new())),
            config,
            publisher_profiles: None,
        };

        // Retirements are read from the Realm database, which callers
        // without one (isolated tests) do not set up.
        if !cancel.is_cancelled() && platform::storage::db::is_database_initialized() {
            tokio::spawn(follow_realm_retirements(
                issuer.signer_client.clone(),
                issuer.realm_keys.clone(),
                cancel,
            ));
        }

        Ok(issuer)
    }

    /// Wire the registration-time Publisher Profile freshness check. The
    /// server composition always wires one; without it, package provenance
    /// falls back to the Publisher Registry row alone.
    pub fn with_publisher_profiles(mut self, refresher: Arc<PublisherProfileRefresher>) -> Self {
        self.publisher_profiles = Some(refresher);
        self
    }

    /// Process a register request from `signer`, whose Realm client key
    /// proof the handler checked, and issue a credential.
    pub async fn issue_credential(
        &self,
        request: &RegisterRequest,
        signer: &Signer,
    ) -> Result<RegisterResponse, AidError> {
        match self.issue_credential_inner(request, signer).await {
            Ok(register_ok) => Ok(RegisterResponse {
                result: Some(register_response::Result::Success(register_ok)),
            }),
            Err(err) => Ok(RegisterResponse {
                result: Some(register_response::Result::Error(ErrorResponse {
                    code: aid_error_to_code(&err),
                    message: err.to_string(),
                })),
            }),
        }
    }

    /// Re-authenticate a registration context and prepare fresh credentials
    /// for an existing actor identity, without writing identity state.
    ///
    /// This is deliberately separate from registration: callers must provide an
    /// `ActrId` whose realm and type exactly match the authenticated registration
    /// request, and no new serial number is allocated.
    ///
    /// `renewal_proof` is the caller's possession proof for exactly this
    /// `actor_id`: the raw renewal token most recently issued to that actor.
    /// Registration authentication alone only proves the caller may run this
    /// package/type in this realm — it must never be enough to mint
    /// credentials for an arbitrary serial number.
    pub(crate) async fn prepare_reissue(
        &self,
        actor_id: &ActrId,
        request: &RegisterRequest,
        renewal_proof: Option<&[u8]>,
        signer: &Signer,
    ) -> ReissuePreparation {
        match self
            .prepare_reissue_inner(actor_id, request, renewal_proof, signer)
            .await
        {
            Ok(prepared) => ReissuePreparation::Ready(Box::new(prepared)),
            Err(err) => {
                let (code, message) = err.to_response();
                let response = RegisterResponse {
                    result: Some(register_response::Result::Error(ErrorResponse {
                        code,
                        message,
                    })),
                };
                if code >= 500 {
                    ReissuePreparation::Retryable(response)
                } else {
                    ReissuePreparation::Rejected(response)
                }
            }
        }
    }

    async fn prepare_reissue_inner(
        &self,
        actor_id: &ActrId,
        request: &RegisterRequest,
        renewal_proof: Option<&[u8]>,
        signer: &Signer,
    ) -> Result<PreparedReissue, ReissueFailure> {
        if actor_id.realm != request.realm {
            platform::recording::warn!(
                "Credential reissue Realm mismatch: actor={}, requested_realm={}",
                actor_id.to_string_repr(),
                request.realm
            );
            return Err(AidError::InvalidFormat.into());
        }

        let provenance = self.verify_registration_identity(request).await?;
        let proof = self
            .verify_actor_possession(actor_id, renewal_proof)
            .await?;
        // A member moves only its own Actors to another of its keys.
        let binding = platform::realm::membership::actor_binding_on(
            &mut *Self::db_connection().await?,
            actor_id,
        )
        .await
        .map_err(|e| AidError::GenerationFailed(format!("actor binding lookup failed: {e}")))?;
        if binding.is_none_or(|binding| binding.member_id != signer.member_id) {
            return Err(AidError::Membership(
                "client_unknown: the Actor belongs to another member".to_string(),
            )
            .into());
        }
        // The requested type must be the one this identity was issued for.
        let issued = Self::issued_actor(&mut *Self::db_connection().await?, actor_id).await?;
        if issued.actor_type != request.actr_type {
            platform::recording::warn!(
                "Credential reissue type mismatch: actor={}, requested_type={}",
                actor_id.to_string_repr(),
                request.actr_type.to_string_repr()
            );
            return Err(AidError::InvalidFormat.into());
        }
        // The provenance class stays fixed: the recheck below refuses a
        // development identity whose publisher name is now in the registry,
        // and a Publisher identity whose publisher left it.

        let register_ok = self.issue_credential_for_actor(actor_id, signer).await?;

        // Reissue accepts proofs that already expired (anything inside the
        // binding grace window), so anchoring the successor on the proof's
        // expiry alone could mint an already-dead token.
        let anchor = proof.binding_expires_at.max(crate::renewal::now_secs());
        Ok(PreparedReissue {
            register_ok,
            successor_hash: crate::renewal::hash_token(&proof.successor),
            successor_token: proof.successor,
            successor_expires_at: crate::renewal::successor_expires_at(
                anchor,
                self.config.renewal_token_ttl_secs,
            ),
            proof_hash: proof.proof_hash,
            binding_expires_at: proof.binding_expires_at,
            package: provenance.is_some(),
            target: request.target.clone(),
            signer: signer.clone(),
        })
    }

    /// Verify that the reissue caller actually owns `actor_id`.
    ///
    /// `verify_registration_identity` authenticates the workload context
    /// (package proof or realm authorization) but does not bind the caller to
    /// `actor_id.serial_number`. The binding proof is the renewal token Issuer
    /// itself issued to that actor: its hash must match an issuer-recorded
    /// token for exactly this `ActrId`. Expired tokens remain acceptable for
    /// [`crate::renewal::REISSUE_BINDING_GRACE_SECS`] so long-offline nodes can
    /// still recover their stable identity; unknown or unbound IDs are rejected
    /// with a distinct possession-failure code ([`REISSUE_POSSESSION_NOT_PROVEN_CODE`],
    /// not the realm-denial `403`) and no credential is signed.
    async fn verify_actor_possession(
        &self,
        actor_id: &ActrId,
        renewal_proof: Option<&[u8]>,
    ) -> Result<VerifiedPossession, ReissueFailure> {
        let Some(proof) = renewal_proof.filter(|proof| !proof.is_empty()) else {
            platform::recording::warn!(
                "Credential reissue rejected: missing possession proof for actor {}; refusing to sign for an unproven serial",
                actor_id.to_string_repr()
            );
            return Err(ReissueFailure::PossessionNotProven);
        };

        if proof.len() != 32 {
            platform::recording::warn!(
                "Credential reissue rejected: possession proof has invalid length {} for actor {}",
                proof.len(),
                actor_id.to_string_repr()
            );
            // A wrong-length proof cannot match any recorded binding, so it is
            // a possession failure (422) — same class as a missing/unknown
            // proof — not a malformed-request 400.
            return Err(ReissueFailure::PossessionNotProven);
        }

        let proof_hash = crate::renewal::hash_token(proof);
        let binding_expires_at = crate::renewal::find_binding_token_expiry(actor_id, &proof_hash)
            .await
            .map_err(|e| {
                AidError::GenerationFailed(format!("reissue binding lookup failed: {e}"))
            })?;

        let Some(binding_expires_at) = binding_expires_at else {
            // Never log the proof or its hash — treat them like credentials.
            platform::recording::warn!(
                "Credential reissue rejected: possession proof does not match any recorded binding for actor {}; possible cross-actor credential minting attempt",
                actor_id.to_string_repr()
            );
            return Err(ReissueFailure::PossessionNotProven);
        };

        // A proof is good for one reissue. The successor derived from it (the
        // same one `/renew` rotates to) marks it as consumed; a lost response
        // is recovered by retrying the same attempt, which replays the stored
        // response instead of reaching here. This keeps a leaked proof from
        // being chained into further successors.
        let successor = crate::renewal::derive_successor_token(
            &self.config.renewal_token_secret,
            proof,
            actor_id,
            binding_expires_at,
        );
        let consumed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM issuer_renewal_token WHERE token_hash = ?1)",
        )
        .bind(crate::renewal::hash_token(&successor))
        .fetch_one(platform::storage::db::get_database().get_pool())
        .await
        .map_err(|e| {
            AidError::GenerationFailed(format!("reissue consumption lookup failed: {e}"))
        })?;
        if consumed {
            platform::recording::warn!(
                "Credential reissue rejected: possession proof already consumed for actor {}",
                actor_id.to_string_repr()
            );
            return Err(ReissueFailure::PossessionNotProven);
        }

        Ok(VerifiedPossession {
            proof_hash,
            binding_expires_at,
            successor,
        })
    }

    /// Internal processing logic (Ed25519 signing via the Signer Sign RPC).
    async fn issue_credential_inner(
        &self,
        request: &RegisterRequest,
        signer: &Signer,
    ) -> Result<register_response::RegisterOk, AidError> {
        // The handler's Realm client proof (single-use nonce) authorized the
        // request; this verifies only where the workload comes from.
        let provenance = self.verify_registration_identity(request).await?;
        let development = provenance == Some(Provenance::Development);

        // Reserve the identity and its optional legacy target. V2 manifests
        // describe execution through workload/host profiles and have no target.
        let actr_id = self
            .generate_actr_id(
                &request.actr_type,
                &request.realm,
                request.target.as_deref(),
                if request.auth_mode == Some(actr_protocol::RegisterAuthMode::Linked as i32) {
                    actr_protocol::RegisterAuthMode::Linked
                } else {
                    actr_protocol::RegisterAuthMode::Package
                },
                development,
                signer,
            )
            .await?;

        // Build IdentityClaims (proto type, plaintext).
        let (key_id, verifying_key) = self.realm_key(&actr_id.realm).await?;
        let expr_time = now_secs().saturating_add(self.config.token_ttl_secs);
        let claims_proto = IdentityClaims {
            realm: actr_id.realm.clone(),
            actor_id: actr_id.to_string_repr(),
            expires_at: expr_time,
            protocol_version: 1,
            issuer: actr_id.realm.clone(),
            provider: self.config.provider_id.clone(),
            audience: "actr.signaling".to_string(),
            actor_type: request.actr_type.clone(),
            member: signer.member_id.clone(),
            client_key: signer.key_id.clone(),
            role: actr_protocol::MemberRole::from(signer.role) as i32,
            development: development.then_some(true),
        };

        // Proto-encode the claims bytes.
        let claims_bytes = claims_proto.encode_to_vec();

        // Sign via the Signer Sign RPC (the private key never leaves the Signer).
        let signature_bytes = self
            .signer_client
            .sign(key_id, &claims_bytes)
            .await
            .map_err(|e| AidError::GenerationFailed(format!("Signer sign failed: {e}")))?;

        // Create an AIdCredential (Ed25519 format).
        let credential = AIdCredential {
            key_id,
            claims: Bytes::from(claims_bytes),
            signature: Bytes::from(signature_bytes),
        };

        // Create the expiry Timestamp.
        let credential_expires_at = Some(Timestamp {
            seconds: expr_time as i64,
            nanos: 0,
        });

        // Generate a time-limited TURN credential compatible with coturn --use-auth-secret.
        let turn_credential = self.generate_turn_credential(&actr_id, &signer.key_id, expr_time);

        let (renewal_token, renewal_token_expires_at) =
            self.issue_fresh_renewal_token(&actr_id).await?;

        Ok(register_response::RegisterOk {
            actr_id,
            credential,
            turn_credential,
            credential_expires_at,
            signaling_heartbeat_interval_secs: self.config.signaling_heartbeat_interval_secs,
            signing_pubkey: Bytes::from(verifying_key.as_bytes().to_vec()),
            signing_key_id: key_id,
            renewal_token: Some(renewal_token),
            renewal_token_expires_at: Some(renewal_token_expires_at),
        })
    }

    /// Verify the registration identity according to the explicit request source.
    ///
    /// Returns the package provenance for a package registration, `None` for
    /// a Linked one. Authorization is not decided here: the Realm client
    /// proof, membership and Service ownership are checked by the handler.
    async fn verify_registration_identity(
        &self,
        request: &RegisterRequest,
    ) -> Result<Option<Provenance>, AidError> {
        let auth_mode = match request.auth_mode {
            Some(value) if value == actr_protocol::RegisterAuthMode::Linked as i32 => {
                actr_protocol::RegisterAuthMode::Linked
            }
            Some(value) if value == actr_protocol::RegisterAuthMode::Package as i32 => {
                actr_protocol::RegisterAuthMode::Package
            }
            _ => actr_protocol::RegisterAuthMode::Unspecified,
        };

        match auth_mode {
            actr_protocol::RegisterAuthMode::Unspecified
            | actr_protocol::RegisterAuthMode::Package => {
                self.verify_package_identity(request).await.map(Some)
            }
            actr_protocol::RegisterAuthMode::Linked => {
                self.verify_linked_identity(request)?;
                Ok(None)
            }
        }
    }

    /// A Linked registration is authorized by the Realm client key proof
    /// the handler checked; it only may not claim a publisher domain.
    fn verify_linked_identity(&self, request: &RegisterRequest) -> Result<(), AidError> {
        if request.actr_type.version.is_empty()
            || request.manifest_raw.is_some()
            || request.publisher_signature.is_some()
            || request.signing_public_key.is_some()
        {
            return Err(AidError::InvalidFormat);
        }

        // Linked workloads must not claim a DNS-style publisher namespace: dotted
        // names (e.g. "apps.example.com") are reserved for Package identities whose
        // domain ownership is cryptographically verified. Realm membership is not
        // proof of owning a publisher domain, so allowing a dotted publisher
        // here would let any realm workload impersonate a registered Publisher's
        // service type and overwrite its ACL (AIS-01 / SIG-05).
        if request.actr_type.publisher.contains('.') {
            platform::recording::warn!(
                "Linked registration rejected: DNS-style publisher namespace is reserved for verified publishers, realm={}, type={}",
                request.realm,
                request.actr_type.to_string_repr()
            );
            return Err(AidError::PublisherNotVerified);
        }
        Ok(())
    }

    /// Verify where a package registration comes from.
    ///
    /// The signed manifest must name exactly the requested type. Unversioned
    /// packages also bind `binary.target`; v2 must omit the request target.
    /// Its signing key id then decides the provenance: a key of a publisher
    /// in this Loran's Publisher Registry gives the Publisher's identity; a
    /// publisher name the registry does not hold gives a development identity,
    /// verified with the `signing_public_key` the request carries. A name the
    /// registry holds is never a development identity, so a development key
    /// cannot claim a trusted Publisher. When a
    /// [`PublisherProfileRefresher`] is wired, the registry row is re-checked
    /// against the publisher domain's current Publisher Profile first
    /// ([`Self::resolve_package_key`]).
    ///
    async fn verify_package_identity(
        &self,
        request: &RegisterRequest,
    ) -> Result<Provenance, AidError> {
        let actr_type = &request.actr_type;
        if actr_type.version.is_empty() {
            return Err(AidError::InvalidFormat);
        }
        let type_str = actr_type.to_program_identity();
        let manifest_bytes = request
            .manifest_raw
            .as_ref()
            .filter(|manifest| !manifest.is_empty())
            .ok_or_else(|| {
                platform::recording::warn!(
                    "Package verification failed: registration requires manifest_raw; type_str={}",
                    type_str
                );
                AidError::InvalidFormat
            })?;
        // This shared parser selects the full v2 schema when versioned tables
        // are present. An invalid v2 never retries the legacy/linked path.
        let manifest = actr_pack::RegistrationManifest::parse(manifest_bytes).map_err(|_| {
            platform::recording::warn!("Package manifest format rejected: type_str={}", type_str);
            AidError::InvalidFormat
        })?;
        // A valid signed manifest of one type must not register another.
        if manifest.actor_type() != actr_type {
            platform::recording::warn!(
                "Package manifest identity mismatch: manifest={}, request={}",
                manifest.actor_type().to_program_identity(),
                type_str
            );
            return Err(AidError::PublisherNotVerified);
        }
        match (manifest.target(), request.target.as_deref()) {
            (None, None) => {} // v2: signed profiles, not a binary target triple
            (None, Some(_)) | (Some(_), None | Some("")) => {
                return Err(AidError::InvalidFormat);
            }
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(AidError::PublisherNotVerified);
            }
            (Some(_), Some(_)) => {}
        }

        let signature = request.publisher_signature.as_ref().ok_or_else(|| {
            platform::recording::warn!(
                "Package verification failed: registration requires publisher_signature; type_str={}",
                type_str
            );
            AidError::PublisherNotVerified
        })?;
        let signing_key_id = manifest.signing_key_id().ok_or(AidError::InvalidFormat)?;

        let pool = platform::storage::db::get_database().get_pool().clone();
        let manager = loran_publisher_registry::PublisherRegistryManager::new(pool);
        let (public_key, provenance) = match self
            .resolve_package_key(&manager, &actr_type.publisher, signing_key_id, &type_str)
            .await?
        {
            Some(public_key) => (public_key, Provenance::Publisher),
            None => {
                let key = request
                    .signing_public_key
                    .as_ref()
                    .filter(|key| {
                        loran_publisher_registry::crypto::compute_key_id(key) == signing_key_id
                    })
                    .ok_or_else(|| {
                        platform::recording::warn!(
                            "Package verification failed: development package needs the signing_public_key of key {}; type_str={}",
                            signing_key_id,
                            type_str
                        );
                        AidError::PublisherNotVerified
                    })?;
                (BASE64_STANDARD.encode(key), Provenance::Development)
            }
        };

        let key_bytes: [u8; 32] = BASE64_STANDARD
            .decode(&public_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(AidError::PublisherNotVerified)?;
        let public_key =
            VerifyingKey::from_bytes(&key_bytes).map_err(|_| AidError::PublisherNotVerified)?;
        manifest
            .verify_signature(signature, &public_key)
            .map_err(|_| AidError::PublisherNotVerified)?;

        platform::recording::debug!(
            "Package provenance verified: type_str={}, target={:?}, key_id={}, provenance={:?}",
            type_str,
            manifest.target(),
            signing_key_id,
            provenance
        );
        Ok(provenance)
    }

    /// Resolve the package signing key `signing_key_id` of `publisher` to a
    /// public key of a trusted Publisher identity.
    ///
    /// The Publisher Registry row is a projection captured by
    /// `register_publisher`; when a refresher is wired, the publisher's
    /// domain is re-checked first so a key the domain has removed is revoked
    /// here on this registration instead of waiting for a re-registration.
    /// `Ok(None)` means no registry record holds the publisher name; the
    /// caller may then fall back to a development identity. A refresh that
    /// finds the key gone is a final refusal, never a downgrade.
    async fn resolve_package_key(
        &self,
        manager: &loran_publisher_registry::PublisherRegistryManager,
        publisher: &str,
        signing_key_id: &str,
        type_str: &str,
    ) -> Result<Option<String>, AidError> {
        use loran_publisher_registry::manager::PackageSigningKey;

        let resolved = manager.package_signing_key(publisher, signing_key_id).await;
        let refresh = match resolved {
            Ok(PackageSigningKey::Untrusted) => return Ok(None),
            Ok(PackageSigningKey::Trusted(info)) => {
                let Some(refresher) = &self.publisher_profiles else {
                    return Ok(Some(info.public_key));
                };
                refresher.ensure_current(publisher).await
            }
            Err(loran_publisher_registry::PublisherRegistryError::NotFound)
                if self.publisher_profiles.is_some() =>
            {
                // The publisher exists, but its registry projection may not
                // contain a key newly added to the domain's Profile.
                self.publisher_profiles
                    .as_ref()
                    .unwrap()
                    .refresh_missing_key(publisher)
                    .await
            }
            Err(error) => return Err(Self::map_publisher_verification_error(error)),
        };
        refresh.map_err(|error| {
                platform::recording::warn!(
                    "Package verification failed: Publisher Profile refresh: {error}; type_str={type_str}"
                );
                AidError::GenerationFailed(format!("Publisher Profile refresh: {error}"))
            })?;
        match manager
            .package_signing_key(publisher, signing_key_id)
            .await
            .map_err(Self::map_publisher_verification_error)?
        {
            PackageSigningKey::Trusted(info) => Ok(Some(info.public_key)),
            PackageSigningKey::Untrusted => {
                platform::recording::warn!(
                    "Package verification rejected: the Publisher Profile of {publisher} no longer \
                     trusts key {signing_key_id}; type_str={type_str}"
                );
                Err(AidError::PublisherNotVerified)
            }
        }
    }

    fn map_publisher_verification_error(
        error: loran_publisher_registry::PublisherRegistryError,
    ) -> AidError {
        let message = error.to_string();
        match error {
            loran_publisher_registry::PublisherRegistryError::NotFound
            | loran_publisher_registry::PublisherRegistryError::KeyRevoked(_)
            | loran_publisher_registry::PublisherRegistryError::InvalidStatus(_)
            | loran_publisher_registry::PublisherRegistryError::CertificateExpired
            | loran_publisher_registry::PublisherRegistryError::InvalidRequest(_) => {
                platform::recording::warn!("Publisher Registry verification rejected: {}", message);
                AidError::PublisherNotVerified
            }
            other => {
                platform::recording::warn!(
                    "Publisher Registry verification failed internally: {}",
                    other
                );
                AidError::GenerationFailed(format!(
                    "Publisher Registry verification failed: {other}"
                ))
            }
        }
    }

    /// Generate ActrId.
    async fn generate_actr_id(
        &self,
        actr_type: &ActrType,
        realm: &str,
        target: Option<&str>,
        auth_mode: actr_protocol::RegisterAuthMode,
        development: bool,
        signer: &Signer,
    ) -> Result<ActrId, AidError> {
        let pool = platform::storage::db::get_database().get_pool();
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let actor_type = actr_type.to_program_identity();

        // A collision can only occur after a worker lease error, process
        // restart in the same millisecond, or clock rollback. Never overwrite
        // the earlier identity: advance Snowflake state and retry.
        for _ in 0..1024 {
            let serial_number = SerialNumber::sn().value();
            let reserved = sqlx::query(
                "INSERT OR IGNORE INTO issued_actor_identity
                 (realm, serial_number, actor_type, target, issued_at, auth_mode,
                  development, member_id, client_key_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(realm)
            .bind(serial_number as i64)
            .bind(&actor_type)
            .bind(target)
            .bind(issued_at)
            .bind(auth_mode as i32)
            .bind(development)
            .bind(&signer.member_id)
            .bind(&signer.key_id)
            .execute(pool)
            .await
            .map_err(|error| {
                AidError::GenerationFailed(format!(
                    "failed to reserve Actor instance identity: {error}"
                ))
            })?
            .rows_affected();
            if reserved == 1 {
                return Ok(ActrId {
                    realm: realm.to_string(),
                    serial_number,
                });
            }
        }

        Err(AidError::GenerationFailed(
            "failed to reserve a unique Actor ID after 1024 Snowflake attempts".to_string(),
        ))
    }

    /// The Realm's current issuer key (`realm-profile.md` section 6.1),
    /// created in the Signer on first use. Its verifying key is written to
    /// the credential validator's cache under the Realm, so it verifies only
    /// that Realm's credentials.
    pub async fn realm_key(&self, realm: &str) -> Result<(u32, VerifyingKey), AidError> {
        let current = crate::realm_profile::current_issuer_key(&self.signer_client, realm)
            .await
            .map_err(|e| AidError::GenerationFailed(format!("Realm issuer key: {e}")))?;
        if let Some(key) = self.realm_keys.read().await.get(realm)
            && key.0 == current.key_id
        {
            return Ok(*key);
        }
        let public_key: [u8; 32] = base64::prelude::BASE64_STANDARD
            .decode(&current.public_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| AidError::GenerationFailed("invalid Realm issuer key".to_string()))?;
        let verifying_key = VerifyingKey::from_bytes(&public_key).map_err(|e| {
            AidError::GenerationFailed(format!("Invalid verifying key from Signer: {e}"))
        })?;
        platform::aid::credential::validator::AIdCredentialValidator::persist_key(
            &self.config.sqlite_path,
            realm,
            current.key_id,
            &verifying_key,
            0,
        )
        .await?;
        self.realm_keys
            .write()
            .await
            .insert(realm.to_string(), (current.key_id, verifying_key));
        Ok((current.key_id, verifying_key))
    }

    /// Compute the renewal token expiry time (Unix timestamp, seconds, i64 for SQLite).
    fn calculate_renewal_expiry_time(&self) -> i64 {
        (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + self.config.renewal_token_ttl_secs) as i64
    }

    /// Generate a 32-byte renewal token using a CSPRNG.
    fn generate_renewal_token(&self) -> Vec<u8> {
        let mut token = vec![0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut token);
        token
    }

    async fn issue_fresh_renewal_token(
        &self,
        actor_id: &ActrId,
    ) -> Result<(Bytes, Timestamp), AidError> {
        if self.config.renewal_token_secret.is_empty() {
            return Err(AidError::GenerationFailed(
                "renewal_token_secret is required".to_string(),
            ));
        }

        let token = self.generate_renewal_token();
        let expires_at = self.calculate_renewal_expiry_time();
        let token_hash = crate::renewal::hash_token(&token);
        crate::renewal::insert_renewal_token(actor_id, &token_hash, expires_at)
            .await
            .map_err(|e| {
                AidError::GenerationFailed(format!("Failed to persist renewal token: {e}"))
            })?;

        let _ = crate::renewal::gc_expired_tokens().await;
        crate::renewal::trim_oldest_tokens_for_actor(actor_id)
            .await
            .map_err(|e| {
                AidError::GenerationFailed(format!("Failed to trim renewal tokens: {e}"))
            })?;

        Ok((
            Bytes::from(token),
            Timestamp {
                seconds: expires_at,
                nanos: 0,
            },
        ))
    }

    /// Recheck, before a stored credential is replayed, that `actor_id` may
    /// still receive credentials: a replay never hands out what the Issuer
    /// would refuse now.
    pub(crate) async fn recheck_before_replay(
        &self,
        actor_id: &ActrId,
        signer: &Signer,
    ) -> Result<(), AidError> {
        self.recheck_existing_actor(actor_id, signer)
            .await
            .map(|_| ())
    }

    async fn db_connection() -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, AidError> {
        platform::storage::db::get_database()
            .get_pool()
            .acquire()
            .await
            .map_err(|e| AidError::GenerationFailed(format!("database connection failed: {e}")))
    }

    /// Load what was recorded when `actor_id` was issued.
    async fn issued_actor(
        conn: &mut sqlx::SqliteConnection,
        actor_id: &ActrId,
    ) -> Result<IssuedActor, AidError> {
        let row: Option<(String, Option<i32>, bool)> = sqlx::query_as(
            "SELECT actor_type, auth_mode, development FROM issued_actor_identity
             WHERE realm = ? AND serial_number = ?",
        )
        .bind(&actor_id.realm)
        .bind(actor_id.serial_number as i64)
        .fetch_optional(conn)
        .await
        .map_err(|e| AidError::GenerationFailed(format!("actor identity lookup failed: {e}")))?;
        let Some((actor_type, auth_mode, development)) = row else {
            platform::recording::warn!(
                "Credential issuance rejected: actor {} was not issued by this Realm",
                actor_id.to_string_repr()
            );
            return Err(AidError::InvalidFormat);
        };
        let actor_type = ActrType::from_string_repr(&actor_type).map_err(|e| {
            AidError::GenerationFailed(format!("stored actor type is invalid: {e}"))
        })?;
        Ok(IssuedActor {
            actor_type,
            auth_mode,
            development,
        })
    }

    /// Re-validate that an existing actor may still receive credentials.
    ///
    /// Runs on the renew and reissue paths only (never on fresh registration,
    /// which allocates a brand-new serial that cannot be revoked yet). The renew
    /// path previously trusted the renewal token alone and never re-checked the
    /// surrounding realm/package/publisher state, so a workload that lost its
    /// authorization after issuance could keep renewing indefinitely. Three
    /// lightweight checks, in order:
    ///
    /// - AIS-04: the actor must not be on the explicit revocation list.
    /// - AIS-03: the realm must still exist and be in a usable state.
    /// - AIS-03: a Publisher identity requires its publisher to stay active;
    ///   a development identity requires the name to stay absent from the
    ///   registry. Explicit Linked identities skip publisher checks.
    async fn recheck_existing_actor(
        &self,
        actor_id: &ActrId,
        signer: &Signer,
    ) -> Result<IssuedActor, AidError> {
        Self::recheck_actor_on(&mut *Self::db_connection().await?, actor_id, signer).await
    }

    /// [`AIdIssuer::recheck_existing_actor`] on `conn`. The renew and reissue
    /// commits run it inside their transaction, so an Actor revocation, Realm change, or
    /// Publisher change that lands after the first check still
    /// stops the credential.
    pub(crate) async fn recheck_actor_on(
        conn: &mut sqlx::SqliteConnection,
        actor_id: &ActrId,
        signer: &Signer,
    ) -> Result<IssuedActor, AidError> {
        // AIS-04: an explicitly revoked identity is terminal, regardless of any
        // still-valid renewal token it may hold.
        if crate::renewal::is_actor_revoked(&mut *conn, actor_id)
            .await
            .map_err(|e| {
                AidError::GenerationFailed(format!("actor revocation lookup failed: {e}"))
            })?
        {
            platform::recording::warn!(
                "Credential issuance rejected: actor {} is on the revocation list",
                actor_id.to_string_repr()
            );
            return Err(AidError::RealmError(
                platform::realm::RealmError::ValidationError(
                    "actor identity has been revoked".to_string(),
                ),
            ));
        }

        // The requesting Realm client key must be unrevoked and its member
        // active (`realm-membership.md` section 6). For a renewal it is the
        // Actor's key; a reissue moves the Actor to it.
        platform::realm::membership::check_key_on(&mut *conn, &actor_id.realm, &signer.key_id)
            .await
            .map_err(|error| match error.code {
                platform::realm::membership::Code::Unavailable => {
                    AidError::GenerationFailed(error.to_string())
                }
                _ => AidError::Membership(error.to_string()),
            })?;

        // AIS-03(a): the realm must still be usable (exists, enabled, not
        // expired, Active status).
        platform::realm::Realm::validate_realm_on(&mut *conn, &actor_id.realm)
            .await
            .map_err(|e| match e {
                platform::realm::RealmValidationError::RealmUnavailable { reason, .. } => {
                    AidError::RealmError(platform::realm::RealmError::ValidationError(reason))
                }
                platform::realm::RealmValidationError::StoreError { message, .. } => {
                    AidError::GenerationFailed(message)
                }
                // RealmValidationError is #[non_exhaustive].
                _ => AidError::GenerationFailed(format!("realm validation failed: {e:?}")),
            })?;

        // AIS-03(b): recheck Publisher status for package identities.
        // The Actor type is the one recorded when the identity was issued.
        let issued = Self::issued_actor(&mut *conn, actor_id).await?;
        let (actor_type, auth_mode) = (&issued.actor_type, issued.auth_mode);
        let type_str = actor_type.to_program_identity();
        if auth_mode == Some(actr_protocol::RegisterAuthMode::Linked as i32) {
            return Ok(issued);
        }

        let publisher_name =
            loran_publisher_registry::reserved::canonical_publisher_domain(&actor_type.publisher)
                .unwrap_or_else(|_| actor_type.publisher.to_ascii_lowercase());
        let publisher: Option<String> =
            sqlx::query_scalar("SELECT status FROM publisher WHERE name = ?")
                .bind(&publisher_name)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| {
                    AidError::GenerationFailed(format!("publisher status lookup failed: {e}"))
                })?;
        // A development identity keeps renewing only while no registry record
        // holds its publisher name.
        if issued.development {
            return match publisher {
                None => Ok(issued),
                Some(_) => {
                    platform::recording::warn!(
                        "Credential issuance rejected: development identity {} claims publisher {}, now held by the Publisher Registry",
                        actor_id.to_string_repr(),
                        actor_type.publisher
                    );
                    Err(AidError::PublisherNotVerified)
                }
            };
        }
        // Pre-upgrade rows have no auth mode. A DNS-style identity is always a
        // package identity; a non-domain identity without a publisher
        // is the legacy linked case. Known package identities never take this fallback.
        if auth_mode.is_none() && publisher.is_none() && !actor_type.publisher.contains('.') {
            return Ok(issued);
        }
        // A Publisher identity requires its publisher to stay active.
        match publisher {
            Some(status)
                if status
                    == loran_publisher_registry::model::PublisherStatus::Active.to_string() =>
            {
                Ok(issued)
            }
            Some(status) => {
                platform::recording::warn!(
                    "Credential issuance rejected: publisher {} for type {} is not active (status={})",
                    actor_type.publisher,
                    type_str,
                    status
                );
                Err(AidError::PublisherNotVerified)
            }
            None => {
                platform::recording::warn!(
                    "Credential issuance rejected: publisher {} for package type {} no longer exists",
                    actor_type.publisher,
                    type_str
                );
                Err(AidError::PublisherNotVerified)
            }
        }
    }

    /// Re-issue a credential for an existing ActrId (used by /issuer/renew).
    ///
    /// Takes an already-verified `ActrId`; it neither calls `generate_actr_id()`
    /// nor re-runs registration authentication.
    pub async fn issue_credential_for_actor(
        &self,
        actor_id: &ActrId,
        signer: &Signer,
    ) -> Result<register_response::RegisterOk, AidError> {
        // Re-validate Realm and Publisher state and the revocation list
        // before signing (AIS-03 / AIS-04). Covers both /renew and /reissue,
        // which are the only callers of this method.
        let issued = self.recheck_existing_actor(actor_id, signer).await?;

        // Build IdentityClaims.
        let (key_id, verifying_key) = self.realm_key(&actor_id.realm).await?;
        let expr_time = now_secs().saturating_add(self.config.token_ttl_secs);
        let claims_proto = IdentityClaims {
            realm: actor_id.realm.clone(),
            actor_id: actor_id.to_string_repr(),
            expires_at: expr_time,
            protocol_version: 1,
            issuer: actor_id.realm.clone(),
            provider: self.config.provider_id.clone(),
            audience: "actr.signaling".to_string(),
            actor_type: issued.actor_type,
            member: signer.member_id.clone(),
            client_key: signer.key_id.clone(),
            role: actr_protocol::MemberRole::from(signer.role) as i32,
            development: issued.development.then_some(true),
        };

        let claims_bytes = claims_proto.encode_to_vec();

        // Sign via the Signer Sign RPC.
        let signature_bytes = self
            .signer_client
            .sign(key_id, &claims_bytes)
            .await
            .map_err(|e| AidError::GenerationFailed(format!("Signer sign failed: {e}")))?;

        let credential = AIdCredential {
            key_id,
            claims: Bytes::from(claims_bytes),
            signature: Bytes::from(signature_bytes),
        };

        let credential_expires_at = Some(Timestamp {
            seconds: expr_time as i64,
            nanos: 0,
        });

        // Generate a time-limited TURN credential.
        let turn_credential = self.generate_turn_credential(actor_id, &signer.key_id, expr_time);

        Ok(register_response::RegisterOk {
            actr_id: actor_id.clone(),
            credential,
            turn_credential,
            credential_expires_at,
            signaling_heartbeat_interval_secs: self.config.signaling_heartbeat_interval_secs,
            signing_pubkey: Bytes::from(verifying_key.as_bytes().to_vec()),
            signing_key_id: key_id,
            renewal_token: None,
            renewal_token_expires_at: None,
        })
    }

    /// Generate a time-limited TURN credential (coturn --use-auth-secret compatible format).
    ///
    /// The username and HMAC bind the issuing Realm client key as well as the Actor.
    /// `password = base64(HMAC-SHA1(realm_turn_secret, username))`, keyed by
    /// the Realm's secret derived from the master `turn_secret`.
    fn generate_turn_credential(
        &self,
        actor_id: &ActrId,
        client_key: &str,
        expires_at: u64,
    ) -> actr_protocol::TurnCredential {
        let realm = actor_id.realm.clone();
        let actor_id = actor_id.to_string_repr();
        let provider = &self.config.provider_id;
        let audience = "actr.turn";
        let username = format!(
            "{expires_at}:realm={realm}:provider={provider}:audience={audience}:client_key={client_key}:actor={actor_id}"
        );
        let secret = platform::realm::realm_turn_secret(
            self.config.turn_secret.as_bytes(),
            &realm,
            provider,
        );
        let mut mac = HmacSha1::new_from_slice(&secret).expect("HMAC-SHA1 accepts any key length");
        mac.update(username.as_bytes());
        let result = mac.finalize();
        let password = BASE64_STANDARD.encode(result.into_bytes());

        actr_protocol::TurnCredential {
            username,
            password,
            expires_at,
            protocol_version: 1,
            realm,
            provider: provider.clone(),
            audience: audience.to_string(),
        }
    }

    // ========== Health check methods ==========

    /// Check the database that holds registrations and renewal tokens.
    pub async fn check_database_health(&self) -> Result<(), AidError> {
        sqlx::query("SELECT 1")
            .execute(platform::storage::db::get_database().get_pool())
            .await
            .map(|_| ())
            .map_err(|e| AidError::GenerationFailed(format!("Database unhealthy: {e}")))
    }

    /// Check Signer service health status.
    ///
    /// Credentials are signed by the Signer with each Realm's issuer key, so
    /// signing works while the Signer answers.
    pub async fn check_signer_health(&self) -> Result<(), AidError> {
        self.signer_client
            .health_check()
            .await
            .map(|_| ())
            .map_err(|e| AidError::GenerationFailed(format!("Signer service unhealthy: {e}")))
    }

    /// Number of Realm issuer keys loaded so far.
    pub async fn realm_key_count(&self) -> usize {
        self.realm_keys.read().await.len()
    }
}

/// How often a Realm issuer key whose retirement failed is retried
const KEY_RETIREMENT_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

/// Retire the issuer key of every Realm that retires, so it never signs
/// again: first for Realms retired before this process started, then as
/// retirements are announced. A failed Signer call is retried.
async fn follow_realm_retirements(
    signer: SignerClientWrapper,
    realm_keys: Arc<RwLock<HashMap<String, (u32, VerifyingKey)>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    use tokio::sync::broadcast::error::RecvError;

    let mut changes = platform::realm::changes::subscribe();
    let mut pending = retired_realms().await;
    let mut retry = tokio::time::interval(KEY_RETIREMENT_RETRY);
    loop {
        pending = retire_realm_keys(&signer, &realm_keys, pending).await;
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = retry.tick() => {}
            change = changes.recv() => match change {
                Ok(realm) => {
                    if retired_realm(&realm).await {
                        pending.insert(realm);
                    }
                }
                Err(RecvError::Lagged(_)) => pending.extend(retired_realms().await),
                Err(RecvError::Closed) => break,
            },
        }
    }
}

async fn retired_realm(realm: &str) -> bool {
    matches!(
        platform::realm::Realm::get(realm).await,
        Ok(Some(r)) if r.status == platform::realm::RealmStatus::Retired
    )
}

async fn retired_realms() -> HashSet<String> {
    match platform::realm::Realm::get_all().await {
        Ok(realms) => realms
            .into_iter()
            .filter(|r| r.status == platform::realm::RealmStatus::Retired)
            .map(|r| r.name)
            .collect(),
        Err(e) => {
            platform::recording::warn!("Failed to list retired Realms: {}", e);
            HashSet::new()
        }
    }
}

/// Retire the issuer keys of `realms` in the Signer; returns the Realms whose
/// retirement failed.
async fn retire_realm_keys(
    signer: &SignerClientWrapper,
    realm_keys: &RwLock<HashMap<String, (u32, VerifyingKey)>>,
    realms: HashSet<String>,
) -> HashSet<String> {
    let mut failed = HashSet::new();
    for realm in realms {
        realm_keys.write().await.remove(&realm);
        let mut names = vec![crate::realm_profile::issuer_key_name(&realm)];
        match platform::realm::RealmIssuerKey::list(&realm).await {
            Ok(keys) => names.extend(keys.into_iter().map(|key| key.signer_name)),
            Err(e) => {
                platform::recording::warn!(
                    "Failed to list the issuer keys of Realm {}, will retry: {}",
                    realm,
                    e
                );
                failed.insert(realm);
                continue;
            }
        }
        names.dedup();
        for name in names {
            match signer.retire_named_key(&name).await {
                Ok(key_id) => platform::recording::info!(
                    "Retired issuer key {} of Realm {} (key_id {})",
                    name,
                    realm,
                    key_id
                ),
                Err(e) => {
                    platform::recording::warn!(
                        "Failed to retire issuer key {} of Realm {}, will retry: {}",
                        name,
                        realm,
                        e
                    );
                    failed.insert(realm.clone());
                }
            }
        }
    }
    failed
}

impl AIdIssuer {
    /// Revalidate a Realm-bound actor before certifying its relay key.
    pub(crate) async fn relay_actor_type(
        &self,
        actor: &ActrId,
        signer: &Signer,
    ) -> Result<ActrType, AidError> {
        Ok(self.recheck_existing_actor(actor, signer).await?.actor_type)
    }

    pub(crate) async fn mint_relay_identity(
        &self,
        actor: &ActrId,
        actor_type: &ActrType,
        signer: &Signer,
        public_key: [u8; 32],
        admission_commitment: [u8; 32],
        proof_expires_at: u64,
    ) -> Result<actr_relay_security::IssuedIdentity, AidError> {
        use actr_relay_security::{
            CERTIFICATE_DOMAIN, CERTIFICATE_TTL, Certificate, IdentityClaims, IssuedIdentity,
            IssuerKey, signing_bytes,
        };
        let (key_id, verifying_key) = self.realm_key(&actor.realm).await?;
        let now = crate::renewal::now_secs() as u64;
        let expires_at = now.saturating_add(CERTIFICATE_TTL).min(proof_expires_at);
        if expires_at <= now {
            return Err(AidError::InvalidFormat);
        }
        let claims = serde_json::to_vec(&IdentityClaims {
            domain: CERTIFICATE_DOMAIN.into(),
            endpoint: actor.to_string_repr(),
            actor_type: actor_type.to_string_repr(),
            client_key: signer.key_id.clone(),
            public_key,
            admission_commitment,
            expires_at,
        })
        .map_err(|_| AidError::InvalidFormat)?;
        let signature = self
            .signer_client
            .sign(key_id, &signing_bytes(&claims))
            .await
            .map_err(|error| {
                AidError::GenerationFailed(format!("Relay certificate signing failed: {error}"))
            })?;
        Ok(IssuedIdentity {
            certificate: Certificate {
                key_id,
                claims,
                signature,
            },
            issuer: IssuerKey {
                key_id,
                public_key: *verifying_key.as_bytes(),
                expires_at: now.saturating_add(CERTIFICATE_TTL),
            },
        })
    }
}
