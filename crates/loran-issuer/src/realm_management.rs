//! Realm management requests (`actr/protocols/realm-profile.md` section 8)
//! for Realms of other domains: `create_realm`, `publish_profile`,
//! `reissue_owner_invite` and `draft_profile`, each an Actr management
//! envelope signed by an `active` Realm signing key of the Realm's domain.
//!
//! The same envelope carries `register_publisher`
//! (`actr/protocols/authority-discovery.md` section 4.5), signed by an
//! `active` Publisher Profile signing key of the publisher's domain.
//!
//! Realms under the Provider domain are created by the operator and managed
//! by their owner with membership requests (`update_realm`).

use actr_protocol::realm_management::{ManagementOp, ManagementRequest};
use actr_protocol::realm_profile::{RealmPackage, RealmProfile, RealmStatus as ProfileStatus};
use actr_protocol::uri::ActrUri;
use actr_protocol::{AuthorityDescriptor, PublisherProfile};
use axum::http::StatusCode;
use chrono::Utc;
use loran_publisher_registry::{PublisherRegistryError, PublisherRegistryManager};
use platform::config::RealmConfig;
use platform::net::FetchError;
use platform::realm::domain::member_from_descriptor;
use platform::realm::management::use_nonce;
use platform::realm::membership::{self, BY_HOLDER};
use platform::realm::{Realm, RealmDomain, RealmError, RealmProfileRecord, RealmStatus};
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;

use crate::realm_profile::{ProfileError, RealmProfiles};

/// Longest accepted nonce.
const MAX_NONCE_LEN: usize = 128;

#[derive(Debug, thiserror::Error)]
pub enum ManagementError {
    #[error("{0}")]
    Invalid(String),
    /// The Realm signing key or the signature is not accepted.
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Quota(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
}

impl ManagementError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Quota(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<RealmError> for ManagementError {
    fn from(error: RealmError) -> Self {
        match error {
            RealmError::NotFound => Self::NotFound("Realm not found".to_string()),
            RealmError::Retired | RealmError::AlreadyExists | RealmError::Conflict(_) => {
                Self::Conflict(error.to_string())
            }
            RealmError::QuotaExceeded(_) => Self::Quota(error.to_string()),
            RealmError::ValidationError(message) => Self::Invalid(message),
            error => Self::Internal(error.to_string()),
        }
    }
}

impl From<PublisherRegistryError> for ManagementError {
    fn from(error: PublisherRegistryError) -> Self {
        match error {
            PublisherRegistryError::InvalidRequest(_)
            | PublisherRegistryError::InvalidName(_)
            | PublisherRegistryError::ReservedName(_) => Self::Invalid(error.to_string()),
            PublisherRegistryError::VerificationFailed(_) => Self::Forbidden(error.to_string()),
            PublisherRegistryError::InvalidStatus(_) => Self::Conflict(error.to_string()),
            PublisherRegistryError::QuotaExceeded(_) => Self::Quota(error.to_string()),
            PublisherRegistryError::NotFound => Self::NotFound(error.to_string()),
            error => Self::Internal(error.to_string()),
        }
    }
}

impl From<ProfileError> for ManagementError {
    fn from(error: ProfileError) -> Self {
        match error {
            ProfileError::Invalid(message) => Self::Invalid(message),
            ProfileError::Unavailable(message) => Self::Unavailable(message),
            ProfileError::Realm(error) => error.into(),
        }
    }
}

/// Handle one Realm management request, `document` as received. `fetch`
/// gets the Realm domain's authority descriptor; the caller rate-limits and
/// caches it, since it runs before any signature is checked.
pub async fn handle<F, Fut>(
    profiles: &RealmProfiles,
    config: &RealmConfig,
    document: &[u8],
    fetch: F,
) -> Result<Value, ManagementError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<Arc<[u8]>, FetchError>>,
{
    let invalid = |error: &dyn std::fmt::Display| ManagementError::Invalid(error.to_string());
    let request = ManagementRequest::parse(document).map_err(|e| invalid(&e))?;
    let provider_id = profiles.provider_id();
    if request.provider_id != provider_id {
        return Err(ManagementError::Invalid(format!(
            "provider_id '{}' is not this Actr Facility Provider",
            request.provider_id
        )));
    }
    request.check_window(Utc::now()).map_err(|e| invalid(&e))?;
    if request.nonce.is_empty() || request.nonce.len() > MAX_NONCE_LEN {
        return Err(ManagementError::Invalid(format!(
            "nonce must have 1 to {MAX_NONCE_LEN} characters"
        )));
    }
    if request.op == ManagementOp::RegisterPublisher {
        return register_publisher(&request, config.max_publishers(), fetch).await;
    }
    let domain = realm_domain(&request.realm)?;
    if domain == provider_id || domain.ends_with(&format!(".{provider_id}")) {
        return Err(ManagementError::Forbidden(format!(
            "Realms under {provider_id} are created by this Actr Facility Provider's operator"
        )));
    }

    // The domain's Realm signing keys, accepted under its pin.
    let descriptor = fetch(format!("https://{domain}/.well-known/actr"))
        .await
        .map_err(|error| ManagementError::Unavailable(format!("{domain}: {error}")))?;
    let member = member_from_descriptor(&domain, &descriptor)
        .map_err(|error| ManagementError::Forbidden(error.to_string()))?;
    let keys = RealmDomain::accept(&domain, &member)
        .await
        .map_err(|error| ManagementError::Forbidden(error.to_string()))?
        .keys;
    request
        .verify(&keys.keys)
        .map_err(|error| ManagementError::Forbidden(error.to_string()))?;

    use_request_nonce(&request).await?;
    Realm::discard_expired_pending().await?;

    match request.op {
        ManagementOp::CreateRealm => create_realm(profiles, config, &request).await,
        ManagementOp::PublishProfile => publish_profile(profiles, &request, &keys).await,
        ManagementOp::ReissueOwnerInvite => {
            let invite = membership::issue_owner_invite(&request.realm, BY_HOLDER)
                .await
                .map_err(|error| match error.code {
                    membership::Code::NotFound => ManagementError::NotFound(error.message),
                    membership::Code::Conflict => ManagementError::Conflict(error.message),
                    _ => ManagementError::Internal(error.message),
                })?;
            Ok(json!({
                "realm": request.realm,
                "owner_invite": invite.invite,
                "expires_at": membership::rfc3339(invite.expires_at),
            }))
        }
        ManagementOp::DraftProfile => draft_profile(profiles, &request, &keys).await,
        ManagementOp::RegisterPublisher => unreachable!("handled above"),
    }
}

/// `draft_profile`: the next unsigned Realm Profile, with this Actr Facility
/// Provider's current Issuer keys and endpoints and the rest taken from the
/// accepted Realm Profile.
async fn draft_profile(
    profiles: &RealmProfiles,
    request: &ManagementRequest,
    keys: &actr_protocol::RealmSigningKeys,
) -> Result<Value, ManagementError> {
    let name = request.realm.as_str();
    let realm = Realm::get(name)
        .await?
        .ok_or_else(|| ManagementError::NotFound(format!("Realm {name} not found")))?;
    if realm.status != RealmStatus::Active {
        return Err(ManagementError::Conflict(format!(
            "Realm {name} is not active"
        )));
    }
    let accepted = RealmProfileRecord::get(name)
        .await?
        .and_then(|record| record.verify(keys).ok())
        .ok_or_else(|| {
            ManagementError::Conflict(format!("Realm {name} has no accepted Realm Profile"))
        })?;
    let mut draft = profiles
        .draft(
            name,
            accepted.sequence + 1,
            &accepted.realm.display_name,
            accepted.package.clone(),
            false,
        )
        .await?;
    draft["policy"] = json!(accepted.policy);
    Ok(json!({ "realm": name, "profile": draft }))
}

/// Record the nonce of a signed request; only signed requests reach the
/// nonce table.
async fn use_request_nonce(request: &ManagementRequest) -> Result<(), ManagementError> {
    let expires_at = chrono::DateTime::parse_from_rfc3339(&request.expires_at)
        .map_err(|error| ManagementError::Invalid(error.to_string()))?
        .timestamp();
    if !use_nonce(&request.nonce, expires_at).await? {
        return Err(ManagementError::Invalid(
            "nonce was already used".to_string(),
        ));
    }
    Ok(())
}

/// `register_publisher`: register the publisher from the Publisher Profile in
/// its domain's authority descriptor. The request must be signed by an
/// `active` `profile_signing_keys` entry of that profile.
async fn register_publisher<F, Fut>(
    request: &ManagementRequest,
    max_publishers: u32,
    fetch: F,
) -> Result<Value, ManagementError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<Arc<[u8]>, FetchError>>,
{
    let domain = request.publisher.as_str();
    let normalized = loran_publisher_registry::reserved::canonical_publisher_domain(domain)
        .map_err(|error| ManagementError::Invalid(format!("publisher: {error}")))?;
    if normalized != domain || !request.realm.is_empty() {
        return Err(ManagementError::Invalid(
            "register_publisher names a normalized publisher domain and no realm".to_string(),
        ));
    }

    let descriptor = fetch(format!("https://{domain}/.well-known/actr"))
        .await
        .map_err(|error| ManagementError::Unavailable(format!("{domain}: {error}")))?;
    let forbidden = |error: &dyn std::fmt::Display| ManagementError::Forbidden(error.to_string());
    let profile = publisher_from_descriptor(domain, &descriptor).map_err(|e| forbidden(&e))?;
    let keys = profile
        .verify(domain, Utc::now())
        .map_err(|e| forbidden(&e))?;
    request
        .verify(&keys.profile_signing_keys)
        .map_err(|e| forbidden(&e))?;
    use_request_nonce(request).await?;

    let pool = platform::storage::db::get_database().get_pool().clone();
    let publisher = PublisherRegistryManager::new(pool)
        .with_max_publishers(max_publishers)
        .register_publisher(domain, &profile, &keys)
        .await?;
    Ok(json!({
        "publisher": publisher.name,
        "status": publisher.status.to_string(),
        "sequence": keys.sequence,
    }))
}

/// The `publisher` member of `domain`'s authority descriptor, which must be
/// valid for `actr://<domain>`.
pub(crate) fn publisher_from_descriptor(
    domain: &str,
    document: &[u8],
) -> Result<PublisherProfile, String> {
    let descriptor: AuthorityDescriptor = serde_json::from_slice(document)
        .map_err(|error| format!("malformed descriptor: {error}"))?;
    let authority = ActrUri::from_domain(domain).map_err(|error| error.to_string())?;
    descriptor
        .validate_for(&authority)
        .map_err(|error| error.to_string())?;
    descriptor
        .publisher
        .ok_or_else(|| format!("{domain} publishes no publisher member"))
}

/// The domain of a normalized Realm name that has a Realm Profile.
fn realm_domain(name: &str) -> Result<String, ManagementError> {
    let uri = actr_protocol::uri::parse_realm_name(name)
        .map_err(|error| ManagementError::Invalid(format!("realm: {error}")))?;
    if uri.is_local() || uri.realm_name().as_deref() != Some(name) {
        return Err(ManagementError::Invalid(format!(
            "realm '{name}' is not a normalized Realm name with an Actr Facility Provider"
        )));
    }
    Ok(uri.domain().to_string())
}

fn realm_id(name: &str) -> &str {
    name.split('@').next().unwrap_or_default()
}

async fn create_realm(
    profiles: &RealmProfiles,
    config: &RealmConfig,
    request: &ManagementRequest,
) -> Result<Value, ManagementError> {
    let name = request.realm.as_str();
    let package = request
        .package
        .clone()
        .ok_or_else(|| ManagementError::Invalid("create_realm needs package".to_string()))?;
    check_package(&package)?;
    let requester = request.requester.clone().unwrap_or_default();

    let created = Realm {
        name: name.to_string(),
        status: RealmStatus::Pending,
        requester: requester.clone(),
        pending_expires_at: Some(Utc::now().timestamp() as u64 + config.pending_ttl_secs()),
        ..Default::default()
    }
    .insert_requested(config)
    .await;
    let realm = match created {
        Ok(realm) => realm,
        // Idempotent on (realm, requester).
        Err(RealmError::AlreadyExists) => {
            let existing = Realm::get(name).await?.ok_or_else(|| {
                ManagementError::Conflict(format!("Realm {name} changed concurrently"))
            })?;
            if existing.status == RealmStatus::Retired || existing.requester != requester {
                return Err(ManagementError::Conflict(format!(
                    "Realm {name} already exists"
                )));
            }
            if existing.status == RealmStatus::Active {
                return Ok(json!({ "realm": name, "status": "active" }));
            }
            existing
        }
        Err(error) => return Err(error.into()),
    };

    let draft = match profiles
        .draft(name, 1, realm_id(name), Some(package), false)
        .await
    {
        Ok(draft) => draft,
        Err(error) => {
            if let Err(discard) = Realm::discard_pending(name).await {
                platform::recording::warn!("Pending realm {name} not discarded: {discard}");
            }
            return Err(error.into());
        }
    };
    Ok(json!({
        "realm": name,
        "status": "pending",
        "pending_expires_at": realm.pending_expires_at,
        "profile": draft,
    }))
}

async fn publish_profile(
    profiles: &RealmProfiles,
    request: &ManagementRequest,
    keys: &actr_protocol::RealmSigningKeys,
) -> Result<Value, ManagementError> {
    let name = request.realm.as_str();
    let document = request
        .profile
        .as_ref()
        .ok_or_else(|| ManagementError::Invalid("publish_profile needs profile".to_string()))?
        .to_string();
    let realm = Realm::get(name)
        .await?
        .ok_or_else(|| ManagementError::NotFound(format!("Realm {name} not found")))?;
    if realm.status == RealmStatus::Retired {
        return Err(RealmError::Retired.into());
    }

    let profile = RealmProfile::verify(document.as_bytes(), name, keys)
        .map_err(|error| ManagementError::Invalid(error.to_string()))?;
    if profile.provider.id != profiles.provider_id() {
        return Err(ManagementError::Invalid(format!(
            "provider.id '{}' is not this Actr Facility Provider",
            profile.provider.id
        )));
    }
    if profile.realm.status == ProfileStatus::Active {
        // An active Realm Profile names exactly this Actr Facility Provider's Issuer,
        // the Realm's issuer key and this Actr Facility Provider's endpoints.
        let expected = profiles
            .draft(name, profile.sequence, "", None, false)
            .await?;
        let named = |member: &str, value: Value| expected[member] == value;
        if !named("issuer", json!(profile.issuer)) || !named("provider", json!(profile.provider)) {
            return Err(ManagementError::Invalid(
                "the Realm Profile must name this Actr Facility Provider's issuer and endpoints"
                    .to_string(),
            ));
        }
    }

    // A Profile the domain's current keys no longer verify is no baseline.
    let record = RealmProfileRecord::get(name).await?;
    let previous = record.as_ref().and_then(|record| record.verify(keys).ok());
    if let Some(previous) = &previous {
        previous
            .check_successor(&profile)
            .map_err(|error| ManagementError::Conflict(error.to_string()))?;
        // Only a higher sequence is accepted; a repeated publish conflicts.
        if profile.sequence == previous.sequence {
            return Err(ManagementError::Conflict(format!(
                "Realm Profile sequence {} is already accepted",
                profile.sequence
            )));
        }
    }

    let (stored, _, owner_invite) = RealmProfileRecord::store(
        &profile,
        &document,
        record.as_ref().map(|record| record.digest.as_str()),
        Some(BY_HOLDER),
        None,
    )
    .await?;
    let mut reply = json!({
        "realm": name,
        "status": stored.status.to_string(),
        "sequence": profile.sequence,
    });
    // The first activation hands the owner invite over, once.
    if let Some(invite) = owner_invite {
        reply["owner_invite"] = json!(invite.invite);
        reply["owner_invite_expires_at"] = json!(membership::rfc3339(invite.expires_at));
    }
    Ok(reply)
}

fn check_package(package: &RealmPackage) -> Result<(), ManagementError> {
    package.check().map_err(ManagementError::Invalid)
}
