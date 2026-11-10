//! Realm Profiles of the Realms this Loran serves as Actr Facility Provider
//! (`actr/protocols/realm-profile.md`).
//!
//! For its own Provider domain Loran holds the Realm signing key in the
//! Signer (`realm-signing:<domain>`) and signs the Profiles itself. For other
//! domains it only keeps the Profiles their key holders signed, and refreshes
//! them together with the domain's pinned `realm_signing_keys`.
//!
//! Each Realm's issuer keys rotate as section 6.1 describes: a new key is
//! listed first, signs once a Profile listing it has been accepted for
//! [`PREPUBLISH_SECS`], and the old key stays listed as `retired` until the
//! last credential it signed has expired.

use actr_protocol::realm_profile::{
    ActrFacilityProvider, IceEndpoint, IssuerKey, RealmIssuer, RealmPackage, RealmPolicy,
    RealmProfile, RealmProfileError, RealmStatus as ProfileStatus, SignalingEndpoint, jcs,
    realm_profile_urls,
};
use actr_protocol::{
    AuthorityDescriptor, DetachedSignature, KeyStatus, RealmSigningKey, RealmSigningKeys,
    SIGNATURE_ALGORITHM,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use platform::net::FetchError;
use platform::realm::domain::member_from_descriptor;
use platform::realm::membership::Signer;
use platform::realm::{
    IssuerKeyState, Realm, RealmDomain, RealmError, RealmIssuerKey, RealmProfileRecord, RealmStatus,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};

use crate::signer_client_wrapper::SignerClientWrapper;

/// The only membership policy of version 1.
const MEMBERSHIP: &str = "invite-only";

/// Signer key name, and descriptor key id, of a domain's Realm signing key.
pub fn realm_signing_key_id(domain: &str) -> String {
    format!("realm-signing:{domain}")
}

/// Signer key name of a Realm's first issuer key.
pub fn issuer_key_name(realm: &str) -> String {
    format!("issuer:{realm}")
}

pub use platform::realm::issuer_key::PREPUBLISH_SECS;
/// How often the Realm issuer keys of Provider-domain Realms rotate.
pub const ROTATION_INTERVAL_SECS: i64 = 90 * 24 * 3600;
/// Clock skew allowed before a retired issuer key is removed.
const CLOCK_SKEW_SECS: i64 = 300;

/// The Realm's current issuer key: `(key_id, public key)`. The first one is
/// created in the Signer, as `issuer:<realm>`, on first use.
pub async fn current_issuer_key(
    signer: &SignerClientWrapper,
    realm: &str,
) -> Result<RealmIssuerKey, ProfileError> {
    if let Some(key) = RealmIssuerKey::current(realm).await? {
        return Ok(key);
    }
    let name = issuer_key_name(realm);
    let (key_id, public_key) = signer
        .ensure_named_key(&name)
        .await
        .map_err(|error| ProfileError::Unavailable(format!("Signer: {error}")))?;
    let now = chrono::Utc::now().timestamp();
    let key = RealmIssuerKey {
        realm: realm.to_string(),
        key_id,
        signer_name: name,
        public_key: BASE64.encode(public_key),
        state: IssuerKeyState::Current,
        created_at: now,
        published_at: None,
        current_since: Some(now),
        remove_after: None,
    };
    match key.insert().await {
        Ok(()) => Ok(key),
        // Created concurrently.
        Err(RealmError::Conflict(_)) => RealmIssuerKey::current(realm)
            .await?
            .ok_or_else(|| ProfileError::Unavailable(format!("{realm} has no issuer key"))),
        Err(error) => Err(error.into()),
    }
}

/// Record `key`, just created in the Signer, as one of its Realm's issuer
/// keys. When a concurrent rotation or the Realm's retirement wins the race,
/// the key is retired in the Signer instead of being left behind.
pub async fn list_issuer_key(
    signer: &SignerClientWrapper,
    key: &RealmIssuerKey,
) -> Result<(), ProfileError> {
    let Err(error) = key.insert().await else {
        return Ok(());
    };
    // Rotations started in the same millisecond share one Signer key name,
    // and so one key; the one listed by the winner stays.
    let listed: Result<bool, sqlx::Error> =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM realm_issuer_key WHERE signer_name = ?)")
            .bind(&key.signer_name)
            .fetch_one(platform::storage::db::get_database().get_pool())
            .await;
    if !matches!(listed, Ok(false)) {
        return Err(error.into());
    }
    if let Err(retire_error) = signer.retire_named_key(&key.signer_name).await {
        platform::recording::warn!(
            "Realm {}: unlisted issuer key {} was not retired: {retire_error}",
            key.realm,
            key.signer_name
        );
    }
    Err(error.into())
}

/// The Actr Facility Provider endpoints a Loran-signed Realm Profile names.
#[derive(Debug, Clone)]
pub struct ProviderEndpoints {
    /// Actr Facility Provider ID, also the domain whose Realm signing key Loran holds.
    pub provider_id: String,
    pub issuer_url: String,
    pub signaling: Vec<SignalingEndpoint>,
    pub ice: Vec<IceEndpoint>,
    /// `realms` location Loran serves, `<origin>/actr/realms/`.
    pub realms_base: String,
}

/// What a newly signed Realm Profile changes; unset members keep their
/// previous values.
#[derive(Debug, Clone, Default)]
pub struct ProfileChange {
    pub package: Option<RealmPackage>,
    pub display_name: Option<String>,
    pub retire: bool,
    /// The owner who requested the change. The Profile is stored only if,
    /// in the storing transaction, it still is the owner.
    pub owner: Option<Signer>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Realm(#[from] RealmError),
}

impl From<RealmProfileError> for ProfileError {
    fn from(error: RealmProfileError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// Signs and keeps Realm Profiles for the Realms this Loran serves.
pub struct RealmProfiles {
    endpoints: ProviderEndpoints,
    signer: SignerClientWrapper,
    /// Longest Actor credential lifetime, which a retired issuer key
    /// outlives.
    credential_ttl_secs: u64,
    /// Where the credential validator's key cache lives.
    sqlite_path: std::path::PathBuf,
    /// Signer key id and the self-signed `realm_signing_keys` member.
    member: OnceCell<(u32, RealmSigningKeys)>,
    /// One Profile signed at a time, so sequences never collide.
    signing: Mutex<()>,
}

impl RealmProfiles {
    pub fn new(
        endpoints: ProviderEndpoints,
        signer: SignerClientWrapper,
        credential_ttl_secs: u64,
        sqlite_path: std::path::PathBuf,
    ) -> Self {
        Self {
            endpoints,
            signer,
            credential_ttl_secs,
            sqlite_path,
            member: OnceCell::new(),
            signing: Mutex::new(()),
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.endpoints.provider_id
    }

    /// Whether Loran holds the Realm signing key of `realm`'s domain.
    pub fn holds_key_for(&self, realm: &str) -> bool {
        realm
            .rsplit_once('@')
            .is_some_and(|(_, domain)| domain == self.endpoints.provider_id)
    }

    /// The Provider domain's `realm_signing_keys` member, once
    /// [`Self::ensure_signing_keys`] has produced it. Never calls the Signer.
    pub fn signing_keys(&self) -> Option<&RealmSigningKeys> {
        self.member.get().map(|(_, member)| member)
    }

    /// Generate (first time) the Realm signing key and self-sign the
    /// one-key `realm_signing_keys` member.
    pub async fn ensure_signing_keys(&self) -> anyhow::Result<&RealmSigningKeys> {
        let (_, member) = self
            .member
            .get_or_try_init(|| async {
                let key_id = realm_signing_key_id(&self.endpoints.provider_id);
                let (signer_key_id, public_key) = self.signer.ensure_named_key(&key_id).await?;
                let (mut member, payload) = unsigned_realm_signing_keys(&key_id, &public_key);
                let signature = self.signer.sign(signer_key_id, payload.as_bytes()).await?;
                member.signature.value = BASE64.encode(signature);
                anyhow::Ok((signer_key_id, member))
            })
            .await?;
        Ok(member)
    }

    /// Sign the next Realm Profile of `realm`, a Realm under the Provider
    /// domain, and accept it. The first active Profile activates a pending
    /// Realm; a retired one retires it.
    pub async fn issue(&self, realm: &str, change: ProfileChange) -> Result<Realm, ProfileError> {
        if !self.holds_key_for(realm) {
            return Err(ProfileError::Invalid(format!(
                "Loran signs Realm Profiles only for Realms under {}",
                self.endpoints.provider_id
            )));
        }
        let Some((signer_key_id, keys)) = self.member.get() else {
            return Err(ProfileError::Unavailable(
                "Realm signing key is not ready".to_string(),
            ));
        };
        let _signing = self.signing.lock().await;

        let current = Realm::get(realm).await?.ok_or(RealmError::NotFound)?;
        if current.status == RealmStatus::Retired {
            return Err(RealmError::Retired.into());
        }
        let record = RealmProfileRecord::get(realm).await?;
        let previous = record
            .as_ref()
            .map(|record| record.verify(keys))
            .transpose()?;

        if let Some(package) = &change.package {
            package.check().map_err(ProfileError::Invalid)?;
            // Another program is another instance, not an upgrade.
            let previous_type = previous
                .as_ref()
                .and_then(|p| p.package.as_ref())
                .and_then(|package| package.actr_type().ok());
            let new_type = package.actr_type().map_err(ProfileError::Invalid)?;
            if let Some(previous_type) = previous_type
                && (previous_type.publisher != new_type.publisher
                    || previous_type.name != new_type.name)
            {
                return Err(ProfileError::Invalid(format!(
                    "package.type {} is another program than {}",
                    package.r#type,
                    previous_type.to_program_identity()
                )));
            }
        }
        let package = change
            .package
            .or_else(|| previous.as_ref().and_then(|p| p.package.clone()));
        if package.is_none() && !change.retire {
            return Err(ProfileError::Invalid(
                "an active Realm Profile needs package {href, sha256, type}".to_string(),
            ));
        }
        let display_name = change
            .display_name
            .or_else(|| previous.as_ref().map(|p| p.realm.display_name.clone()))
            .unwrap_or_else(|| realm.split('@').next().unwrap_or_default().to_string());
        let sequence = previous.as_ref().map_or(1, |p| p.sequence + 1);
        let mut document = self
            .draft(realm, sequence, &display_name, package, change.retire)
            .await?;
        let signature = self
            .signer
            .sign(*signer_key_id, jcs(&document).as_bytes())
            .await
            .map_err(|error| ProfileError::Unavailable(format!("Signer: {error}")))?;
        document["signature"] = json!(DetachedSignature {
            key_id: keys.signature.key_id.clone(),
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            value: BASE64.encode(signature),
        });
        let document = document.to_string();

        // Accept it exactly as an Actr Facility Provider or Actr Host would.
        let profile = RealmProfile::verify(document.as_bytes(), realm, keys)?;
        if let Some(previous) = &previous {
            previous.check_successor(&profile)?;
        }
        let digest = record.as_ref().map(|record| record.digest.as_str());
        let (realm, _, _) =
            RealmProfileRecord::store_issued(&profile, &document, digest, change.owner.as_ref())
                .await?;
        Ok(realm)
    }

    /// An unsigned Realm Profile of `realm` that names this Actr Facility Provider:
    /// its Issuer with the Realm's issuer key (generated in the Signer the
    /// first time) and its endpoints. `package` is required unless `retire`.
    pub async fn draft(
        &self,
        realm: &str,
        sequence: u64,
        display_name: &str,
        package: Option<RealmPackage>,
        retire: bool,
    ) -> Result<Value, ProfileError> {
        current_issuer_key(&self.signer, realm).await?;
        let issuer_keys = RealmIssuerKey::list(realm)
            .await?
            .into_iter()
            .map(|key| IssuerKey {
                id: key.key_id.to_string(),
                algorithm: SIGNATURE_ALGORITHM.to_string(),
                status: key.profile_status(),
                public_key: key.public_key,
            })
            .collect();

        let mut document = json!({
            "schema": actr_protocol::realm_profile::REALM_PROFILE_SCHEMA,
            "sequence": sequence,
            "realm": {
                "name": realm,
                "display_name": display_name,
                "status": if retire { "retired" } else { "active" },
            },
            "issuer": RealmIssuer {
                url: self.endpoints.issuer_url.clone(),
                keys: issuer_keys,
            },
            "provider": ActrFacilityProvider {
                id: self.endpoints.provider_id.clone(),
                signaling: self.endpoints.signaling.clone(),
                ice: self.endpoints.ice.clone(),
            },
            "policy": RealmPolicy { membership: MEMBERSHIP.to_string() },
            "issued_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
        if let Some(package) = package {
            document["package"] = json!(package);
        }
        Ok(document)
    }

    /// Refresh the Realms of other domains that have an accepted Profile:
    /// accept each domain's current `realm_signing_keys` under its pin, then
    /// take a newer Profile from the domain's `realms` locations.
    ///
    /// A Profile whose signing key the domain removed is dropped as a
    /// baseline; without a replacement the Realm is suspended.
    pub async fn refresh<F, Fut>(&self, fetch: F) -> Result<(), RealmError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<Arc<[u8]>, FetchError>>,
    {
        let mut domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for realm in Realm::get_all().await? {
            if realm.status == RealmStatus::Retired || self.holds_key_for(&realm.name) {
                continue;
            }
            if RealmProfileRecord::get(&realm.name).await?.is_none() {
                continue;
            }
            if let Some((_, domain)) = realm.name.rsplit_once('@') {
                domains
                    .entry(domain.to_string())
                    .or_default()
                    .push(realm.name.clone());
            }
        }

        for (domain, realms) in domains {
            let descriptor = match fetch(format!("https://{domain}/.well-known/actr")).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    platform::recording::warn!("Realm domain {domain} descriptor: {error}");
                    continue;
                }
            };
            let member = match member_from_descriptor(&domain, &descriptor) {
                Ok(member) => member,
                Err(error) => {
                    platform::recording::warn!("Realm domain {domain} descriptor: {error}");
                    continue;
                }
            };
            let keys = match RealmDomain::accept(&domain, &member).await {
                Ok(pinned) => pinned.keys,
                Err(error) => {
                    platform::recording::warn!("Realm domain {domain} keys not accepted: {error}");
                    match RealmDomain::get(&domain).await? {
                        Some(pinned) => pinned.keys,
                        None => continue,
                    }
                }
            };
            // Already validated by `member_from_descriptor`.
            let Ok(descriptor) = serde_json::from_slice::<AuthorityDescriptor>(&descriptor) else {
                continue;
            };
            for realm in realms {
                if let Err(error) = self.refresh_realm(&realm, &descriptor, &keys, &fetch).await {
                    platform::recording::warn!("Realm {realm} refresh: {error}");
                }
            }
        }
        Ok(())
    }

    async fn refresh_realm<F, Fut>(
        &self,
        realm: &str,
        descriptor: &AuthorityDescriptor,
        keys: &RealmSigningKeys,
        fetch: &F,
    ) -> Result<(), ProfileError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<Arc<[u8]>, FetchError>>,
    {
        let Some(record) = RealmProfileRecord::get(realm).await? else {
            return Ok(());
        };
        let accepted = match record.verify(keys) {
            Ok(profile) => Some(profile),
            Err(error) => {
                platform::recording::warn!(
                    "Accepted Realm Profile of {realm} no longer verifies: {error}"
                );
                None
            }
        };

        let mut fetched = None;
        for url in realm_profile_urls(descriptor, realm)? {
            // Loran already holds what it serves itself.
            if url.starts_with(&self.endpoints.realms_base) {
                continue;
            }
            let document = match fetch(url.clone()).await {
                Ok(document) => document,
                Err(error) => {
                    platform::recording::warn!("Realm Profile {url}: {error}");
                    continue;
                }
            };
            match RealmProfile::verify(&document, realm, keys) {
                Ok(profile) => {
                    fetched = Some((profile, document));
                    break;
                }
                Err(error) => platform::recording::warn!("Realm Profile {url}: {error}"),
            }
        }

        match (accepted, fetched) {
            (Some(accepted), Some((profile, document))) => {
                accepted.check_successor(&profile)?;
                if profile.sequence > accepted.sequence {
                    store_fetched(&profile, &document, &record.digest).await?;
                    self.suspend_unless_listed(realm, &profile).await?;
                }
            }
            (None, Some((profile, document))) if profile.provider.id == self.provider_id() => {
                store_fetched(&profile, &document, &record.digest).await?;
                self.suspend_unless_listed(realm, &profile).await?;
            }
            (None, _) => {
                platform::recording::warn!(
                    "Suspending Realm {realm}: no Realm Profile signed by a listed key"
                );
                Realm::set_enabled(realm, false).await?;
            }
            (Some(_), None) => {}
        }
        Ok(())
    }

    /// Stop issuing for a Realm whose new active Profile no longer lists its
    /// issuer key as active: the Realm is suspended.
    async fn suspend_unless_listed(
        &self,
        realm: &str,
        profile: &RealmProfile,
    ) -> Result<(), ProfileError> {
        if profile.realm.status != ProfileStatus::Active {
            return Ok(());
        }
        let public_key = current_issuer_key(&self.signer, realm).await?.public_key;
        let listed = profile
            .issuer
            .keys
            .iter()
            .any(|key| key.status == KeyStatus::Active && key.public_key == public_key);
        if !listed {
            platform::recording::warn!(
                "Suspending Realm {realm}: its Realm Profile no longer lists this issuer key"
            );
            Realm::set_enabled(realm, false).await?;
        }
        Ok(())
    }

    /// Start rotating `realm`'s issuer key: create the next key in the Signer
    /// and list it. For a Realm under the Provider domain a new Realm Profile
    /// lists it right away; for another domain the key holder picks it up
    /// with `draft_profile`. Returns the new key id.
    pub async fn rotate_issuer_key(&self, realm: &str) -> Result<u32, ProfileError> {
        let current = Realm::get(realm).await?.ok_or(RealmError::NotFound)?;
        if current.status != RealmStatus::Active {
            return Err(ProfileError::Invalid(format!(
                "Realm {realm} is not active"
            )));
        }
        current_issuer_key(&self.signer, realm).await?;
        let keys = RealmIssuerKey::list(realm).await?;
        if keys.iter().any(|key| key.state == IssuerKeyState::Next) {
            return Err(RealmError::Conflict(format!(
                "Realm {realm} is already rotating its issuer key"
            ))
            .into());
        }
        if keys.len() >= actr_protocol::realm_profile::MAX_ISSUER_KEYS {
            return Err(RealmError::Conflict(format!(
                "Realm {realm} still lists a retired issuer key; rotate after it is removed"
            ))
            .into());
        }

        let now = chrono::Utc::now();
        let name = format!("{}/{}", issuer_key_name(realm), now.timestamp_millis());
        let (key_id, public_key) = self
            .signer
            .ensure_named_key(&name)
            .await
            .map_err(|error| ProfileError::Unavailable(format!("Signer: {error}")))?;
        let key = RealmIssuerKey {
            realm: realm.to_string(),
            key_id,
            signer_name: name,
            public_key: BASE64.encode(public_key),
            state: IssuerKeyState::Next,
            created_at: now.timestamp(),
            published_at: None,
            current_since: None,
            remove_after: None,
        };
        list_issuer_key(&self.signer, &key).await?;
        platform::recording::info!("Realm {realm}: new issuer key {key_id} listed");
        self.republish(realm).await?;
        Ok(key_id)
    }

    /// Advance issuer key rotations at `now` (Unix seconds): switch to a
    /// next key listed long enough, remove retired keys whose credentials
    /// have all expired, and start the periodic rotation of Provider-domain
    /// Realms.
    pub async fn maintain_issuer_keys(&self, now: i64) -> Result<(), ProfileError> {
        for realm in Realm::get_all().await? {
            if realm.status != RealmStatus::Active {
                continue;
            }
            if let Err(error) = self.maintain_realm_keys(&realm.name, now).await {
                platform::recording::warn!("Realm {} issuer keys: {error}", realm.name);
            }
        }
        Ok(())
    }

    async fn maintain_realm_keys(&self, realm: &str, now: i64) -> Result<(), ProfileError> {
        let keys = RealmIssuerKey::list(realm).await?;
        // A next key whose Realm Profile could not be signed yet.
        let mut changed = self.holds_key_for(realm)
            && keys
                .iter()
                .any(|key| key.state == IssuerKeyState::Next && key.published_at.is_none());
        for key in &keys {
            match key.state {
                IssuerKeyState::Next
                    if key
                        .published_at
                        .is_some_and(|at| at + PREPUBLISH_SECS <= now) =>
                {
                    let remove_after = now + self.credential_ttl_secs as i64 + CLOCK_SKEW_SECS;
                    key.switch(now, remove_after).await?;
                    // The retired key verifies only until it is removed.
                    for old in keys.iter().filter(|k| k.state == IssuerKeyState::Current) {
                        self.cache_key(realm, old, remove_after as u64).await?;
                    }
                    platform::recording::info!(
                        "Realm {realm}: issuer key {} now signs",
                        key.key_id
                    );
                    changed = true;
                }
                IssuerKeyState::Retired
                    if let Some(remove_after) = key.remove_after.filter(|at| *at <= now) =>
                {
                    // An issuance that read this key before the switch may
                    // have cached it again without expiry.
                    self.cache_key(realm, key, remove_after as u64).await?;
                    self.signer
                        .retire_named_key(&key.signer_name)
                        .await
                        .map_err(|error| ProfileError::Unavailable(format!("Signer: {error}")))?;
                    RealmIssuerKey::remove(realm, key.key_id).await?;
                    platform::recording::info!(
                        "Realm {realm}: retired issuer key {} removed",
                        key.key_id
                    );
                    changed = true;
                }
                _ => {}
            }
        }
        // The ledger is durable, but `changed` is not. Reconcile even when a
        // previous process committed a switch/removal and lost publication.
        // External domains still publish with their own signing authority.
        if self.holds_key_for(realm)
            && let Some(record) = RealmProfileRecord::get(realm).await?
        {
            let signing_keys = self.signing_keys().ok_or_else(|| {
                ProfileError::Unavailable("Realm signing key is not ready".to_string())
            })?;
            let profile = record.verify(signing_keys)?;
            let current_keys = RealmIssuerKey::list(realm).await?;
            changed |= profile.issuer.keys.len() != current_keys.len()
                || current_keys.iter().any(|key| {
                    profile.issuer_key(key.key_id).is_none_or(|published| {
                        published.public_key != key.public_key
                            || published.status != key.profile_status()
                    })
                });
        }
        if changed {
            self.republish(realm).await?;
        }
        let rotating = keys.iter().any(|key| key.state == IssuerKeyState::Next);
        let due = keys.iter().any(|key| {
            key.state == IssuerKeyState::Current
                && key
                    .current_since
                    .is_some_and(|since| since + ROTATION_INTERVAL_SECS <= now)
        });
        if self.holds_key_for(realm) && due && !rotating && !changed {
            self.rotate_issuer_key(realm).await?;
        }
        Ok(())
    }

    /// Write `key` to the credential validator's cache, valid until
    /// `expires_at` (0: no expiry).
    async fn cache_key(
        &self,
        realm: &str,
        key: &RealmIssuerKey,
        expires_at: u64,
    ) -> Result<(), ProfileError> {
        let bytes: [u8; 32] = BASE64
            .decode(&key.public_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| ProfileError::Invalid(format!("issuer key {}", key.key_id)))?;
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
            .map_err(|error| ProfileError::Invalid(error.to_string()))?;
        platform::aid::credential::validator::AIdCredentialValidator::persist_key(
            &self.sqlite_path,
            realm,
            key.key_id,
            &verifying_key,
            expires_at,
        )
        .await
        .map_err(|error| ProfileError::Unavailable(format!("key cache: {error}")))
    }

    /// Sign a new Realm Profile with the current issuer keys, for a Realm
    /// under the Provider domain that has one.
    async fn republish(&self, realm: &str) -> Result<(), ProfileError> {
        if self.holds_key_for(realm) && RealmProfileRecord::get(realm).await?.is_some() {
            self.issue(realm, ProfileChange::default()).await?;
        }
        Ok(())
    }

    /// Run in the background until `cancel`: generate the Realm signing key
    /// (retrying while the Signer comes up), then every `interval` remove
    /// expired pending Realms and refresh other domains' Realms.
    pub fn spawn(
        self: &Arc<Self>,
        interval: Duration,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        let profiles = self.clone();
        tokio::spawn(async move {
            while let Err(error) = profiles.ensure_signing_keys().await {
                platform::recording::warn!("Realm signing key not ready: {error:#}");
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
            platform::recording::info!(
                "Realm signing key ready: {}",
                realm_signing_key_id(profiles.provider_id())
            );

            // Every round fetches one descriptor per domain and one Profile per
            // Realm, often from a single host. A per-host limit would leave the
            // Realms past it unrefreshed, and suspend them when a key is revoked.
            let fetcher = &platform::net::CachedFetcher::new(Duration::from_secs(300), u32::MAX);
            loop {
                match Realm::discard_expired_pending().await {
                    Ok(0) => {}
                    Ok(count) => {
                        platform::recording::info!("Removed {count} expired pending Realms")
                    }
                    Err(error) => {
                        platform::recording::warn!("Pending Realm cleanup failed: {error}")
                    }
                }
                let fetch = |url: String| async move { fetcher.get(&url).await };
                if let Err(error) = profiles.refresh(fetch).await {
                    platform::recording::warn!("Realm Profile refresh failed: {error}");
                }
                let now = chrono::Utc::now().timestamp();
                if let Err(error) = profiles.maintain_issuer_keys(now).await {
                    platform::recording::warn!("Realm issuer key rotation failed: {error}");
                }
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(interval) => {}
                }
            }
        });
    }
}

async fn store_fetched(
    profile: &RealmProfile,
    document: &[u8],
    previous_digest: &str,
) -> Result<(), ProfileError> {
    let document = std::str::from_utf8(document)
        .map_err(|_| ProfileError::Invalid("Realm Profile is not UTF-8".to_string()))?;
    RealmProfileRecord::store(profile, document, Some(previous_digest), None, None).await?;
    Ok(())
}

/// A one-key `realm_signing_keys` member with an empty signature value, and
/// the RFC 8785 JCS payload that the key must sign (the member without
/// `signature`).
pub fn unsigned_realm_signing_keys(
    key_id: &str,
    public_key: &[u8; 32],
) -> (RealmSigningKeys, String) {
    let member = RealmSigningKeys {
        sequence: 1,
        keys: vec![RealmSigningKey {
            id: key_id.to_string(),
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            public_key: BASE64.encode(public_key),
            status: KeyStatus::Active,
        }],
        rotations: None,
        signature: DetachedSignature {
            key_id: key_id.to_string(),
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            value: String::new(),
        },
    };
    let mut payload: Value = serde_json::to_value(&member).expect("member serializes");
    if let Some(members) = payload.as_object_mut() {
        members.remove("signature");
    }
    (member, jcs(&payload))
}
