use crate::{
    PublisherRegistryError, crypto,
    model::{Publisher, PublisherKeyHistory, PublisherStatus},
    reserved::canonical_publisher_domain,
};
use actr_protocol::realm_profile::jcs;
use actr_protocol::{KeyPinOutcome, KeyPins, KeyStatus, PublisherKeys, PublisherProfile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

/// Trust decision for a package signing key, see
/// [`PublisherRegistryManager::package_signing_key`].
#[derive(Debug)]
pub enum PackageSigningKey {
    /// A non-revoked key of an active publisher in this registry.
    Trusted(PublisherPublicInfo),
    /// No registry record holds the publisher name.
    Untrusted,
}

/// Default validity of a package signing key without `not_after`.
const DEFAULT_KEY_LIFETIME_SECS: i64 = 365 * 24 * 3600;

#[cfg(test)]
#[derive(Debug, Serialize, Deserialize)]
pub struct PublisherVerificationResponse {
    pub certificate: PublisherCertificate,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublisherCertificate {
    pub key_id: String,
    pub publisher: String,
    pub public_key: String,
    pub issued_at: i64,
    /// The key's declared validity limit. Natural expiry does not invalidate
    /// signatures made by this key.
    pub expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublisherPublicInfo {
    pub id: i64,
    pub key_id: String,
    pub name: String,
    pub public_key: String,
    pub certificate: PublisherCertificate,
    /// The accepted Publisher Profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
}

pub struct PublisherRegistryManager {
    pool: SqlitePool,
    /// Most publishers `register_publisher` registers; `None`: no limit.
    max_publishers: Option<u32>,
}

impl PublisherRegistryManager {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            max_publishers: None,
        }
    }

    /// Refuse to register a new publisher once `max` are registered.
    pub fn with_max_publishers(mut self, max: u32) -> Self {
        self.max_publishers = Some(max);
        self
    }

    /// Access the underlying database pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ========== Publish Nonce Challenge-Response ==========

    /// Register `publisher` from `profile`, its Publisher Profile verified
    /// by [`PublisherProfile::verify`] with `keys`, or accept a later profile
    /// of a registered publisher (`register_publisher`,
    /// `actr/protocols/authority-discovery.md` section 4.5).
    ///
    /// The profile's `profile_signing_keys` go through the acceptance
    /// procedure of section 4.4 against the pins of the accepted profile:
    /// the first profile pins them, later ones may retire, remove or, along
    /// a rotation chain, introduce keys; an identity change is refused.
    ///
    /// The profile lists one or two `active` package signing keys, each
    /// within its `not_before`/`not_after`; all of them verify packages. The current
    /// key stays current while it is active; a key that is no longer active
    /// is archived as retired; a key the profile no longer lists is revoked,
    /// and a key used before is never reinstated.
    pub async fn register_publisher(
        &self,
        publisher: &str,
        profile: &PublisherProfile,
        keys: &PublisherKeys,
    ) -> Result<Publisher, PublisherRegistryError> {
        self.register(publisher, profile, keys, None, false).await
    }

    /// Refresh an existing registration under its stored pins. A concurrent
    /// admin deletion must not turn a refresh into a new trust bootstrap.
    pub async fn refresh_publisher(
        &self,
        publisher: &str,
        profile: &PublisherProfile,
        keys: &PublisherKeys,
    ) -> Result<Publisher, PublisherRegistryError> {
        self.register(publisher, profile, keys, None, true).await
    }

    /// Accept `profile` of a registered publisher as its new baseline after
    /// the operator reviewed it (`authority-discovery.md` section 4.4 step
    /// 6): the pins are replaced with the profile's own signing keys, then
    /// the profile is taken as [`Self::register_publisher`] would. Only for a
    /// profile that is valid on its own but not accepted under the pins, and
    /// only the one whose digest ([`KeyPins::digest`] of its own pins) the
    /// operator confirmed.
    pub async fn confirm_publisher_identity(
        &self,
        publisher: &str,
        profile: &PublisherProfile,
        keys: &PublisherKeys,
        digest: &str,
    ) -> Result<Publisher, PublisherRegistryError> {
        self.register(publisher, profile, keys, Some(digest), false)
            .await
    }

    async fn register(
        &self,
        publisher: &str,
        profile: &PublisherProfile,
        keys: &PublisherKeys,
        confirmed: Option<&str>,
        must_exist: bool,
    ) -> Result<Publisher, PublisherRegistryError> {
        let invalid = PublisherRegistryError::InvalidRequest;
        if canonical_publisher_domain(publisher)? != publisher || profile.domain != publisher {
            return Err(invalid(format!(
                "'{publisher}' is not the normalized domain of this Publisher Profile"
            )));
        }
        let now = chrono::Utc::now().timestamp();
        let time = |value: &Option<String>, member: &str| {
            value
                .as_deref()
                .map(|value| {
                    chrono::DateTime::parse_from_rfc3339(value)
                        .map(|time| time.timestamp())
                        .map_err(|error| invalid(format!("{member}: {error}")))
                })
                .transpose()
        };
        // (key_id, public_key, expires_at) of each active package signing key.
        let mut active = Vec::new();
        for key in keys
            .keys
            .iter()
            .filter(|key| key.status == KeyStatus::Active)
        {
            crypto::validate_public_key(&key.public_key)
                .map_err(|error| invalid(format!("package signing key: {error}")))?;
            let key_id = crypto::compute_key_id_from_b64(&key.public_key)?;
            let not_before = time(&key.not_before, "not_before")?;
            let expires_at =
                time(&key.not_after, "not_after")?.unwrap_or(now + DEFAULT_KEY_LIFETIME_SECS);
            if not_before.is_some_and(|not_before| now < not_before) || now >= expires_at {
                return Err(invalid(format!(
                    "package signing key '{}' is outside its validity period",
                    key.id
                )));
            }
            active.push((key_id, key.public_key.clone(), expires_at));
        }
        if active.is_empty() {
            return Err(invalid(
                "a Publisher Profile must list an active package signing key".into(),
            ));
        }
        let document = serde_json::to_value(profile)
            .map_err(|error| invalid(format!("Publisher Profile: {error}")))?;
        let profile_json = jcs(&document);
        let contact = document["security_contact"].as_str();

        // IMMEDIATE takes the write lock before the read, so concurrent
        // registrations of one publisher run one after another instead of
        // failing on a stale WAL snapshot.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query_as::<_, Publisher>("SELECT * FROM publisher WHERE name = ?")
            .bind(publisher)
            .fetch_optional(&mut *tx)
            .await?;
        if must_exist && existing.is_none() {
            return Err(PublisherRegistryError::NotFound);
        }
        if let Some(record) = &existing {
            if record.status != PublisherStatus::Active {
                return Err(PublisherRegistryError::InvalidStatus(format!(
                    "Publisher '{publisher}' is {}",
                    record.status
                )));
            }
            // A confirmation goes on to find nothing to confirm.
            if record.profile_json == profile_json && confirmed.is_none() {
                // Repair profiles accepted before listed retired keys were
                // imported, without replacing any recorded revocation.
                Self::import_retired_keys(&mut tx, record.id, keys, now).await?;
                tx.commit().await?;
                return Ok(record.clone());
            }
        }
        let pinned = existing
            .as_ref()
            .and_then(|record| record.profile_pins.as_deref())
            .map(serde_json::from_str::<KeyPins>)
            .transpose()
            .map_err(|error| invalid(format!("stored key pins: {error}")))?;
        let rejected = |message: String| PublisherRegistryError::VerificationFailed(message);
        let outcome = profile.accept(publisher, chrono::Utc::now(), pinned.as_ref());
        let pins = match (outcome, confirmed) {
            (Ok((_, KeyPinOutcome::Accepted(pins))), None) => pins,
            (Ok((_, KeyPinOutcome::IdentityChange(_))), None) => {
                return Err(rejected(
                    "the Publisher Profile is not signed by a key chained to a pinned key".into(),
                ));
            }
            (Err(error), None) => return Err(rejected(error.to_string())),
            (Ok((_, KeyPinOutcome::Accepted(_))), Some(_)) => {
                return Err(PublisherRegistryError::InvalidStatus(format!(
                    "the Publisher Profile of '{publisher}' is accepted under the pinned keys; \
                     nothing to confirm"
                )));
            }
            (_, Some(digest)) => {
                if existing.is_none() {
                    return Err(PublisherRegistryError::NotFound);
                }
                let pins = match profile
                    .accept(publisher, chrono::Utc::now(), None)
                    .map_err(|error| rejected(error.to_string()))?
                {
                    (_, KeyPinOutcome::Accepted(pins)) => pins,
                    (_, KeyPinOutcome::IdentityChange(_)) => {
                        unreachable!("no pins, no identity change")
                    }
                };
                if pins.digest != digest {
                    return Err(PublisherRegistryError::InvalidStatus(format!(
                        "the Publisher Profile of '{publisher}' changed since it was reviewed"
                    )));
                }
                pins
            }
        };
        let pins_json =
            serde_json::to_string(&pins).map_err(|error| invalid(format!("key pins: {error}")))?;

        // The current key stays current while it is active.
        let held = existing.as_ref().map_or(Vec::new(), |record| {
            let mut held = vec![record.key_id.clone()];
            held.extend(record.next_key_id.clone());
            held
        });
        if let Some(position) = active.iter().position(|(id, ..)| held.first() == Some(id)) {
            active.swap(0, position);
        } else if let Some(position) = active.iter().position(|(id, ..)| held.contains(id)) {
            active.swap(0, position);
        }
        let (key_id, public_key, key_expires_at) = active[0].clone();
        let next = active.get(1).cloned();

        let Some(record) = existing else {
            if let Some(max) = self.max_publishers {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publisher")
                    .fetch_one(&mut *tx)
                    .await?;
                if count >= i64::from(max) {
                    return Err(PublisherRegistryError::QuotaExceeded(format!(
                        "no more publishers: max_publishers = {max}"
                    )));
                }
            }
            let inserted = sqlx::query(
                "INSERT INTO publisher
                    (name, public_key, key_id, profile_json, contact, status,
                     created_at, updated_at, verified_at, key_expires_at,
                     next_public_key, next_key_id, next_key_expires_at, profile_pins)
                 VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(publisher)
            .bind(&public_key)
            .bind(&key_id)
            .bind(&profile_json)
            .bind(contact)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(key_expires_at)
            .bind(next.as_ref().map(|(_, key, _)| key))
            .bind(next.as_ref().map(|(id, ..)| id))
            .bind(next.as_ref().map(|(.., expires)| expires))
            .bind(&pins_json)
            .execute(&mut *tx)
            .await?;
            Self::import_retired_keys(&mut tx, inserted.last_insert_rowid(), keys, now).await?;
            tx.commit().await?;
            platform::recording::info!("Publisher registered: name={publisher}, key_id={key_id}");
            return Publisher::get_by_name(&self.pool, publisher)
                .await?
                .ok_or(PublisherRegistryError::NotFound);
        };

        // A key archived or revoked before never becomes active again.
        for (id, ..) in &active {
            if !held.contains(id)
                && PublisherKeyHistory::key_id_ever_used(&mut *tx, record.id, id).await?
            {
                return Err(invalid(
                    "a previously used publisher key must not be reinstated".into(),
                ));
            }
        }
        // Keys that stop being active: retired while the profile still lists
        // them, revoked once it does not.
        let leaving = std::iter::once((&record.key_id, &record.public_key)).chain(
            record
                .next_key_id
                .as_ref()
                .zip(record.next_public_key.as_ref()),
        );
        for (old_id, old_key) in leaving {
            if old_id.is_empty() || active.iter().any(|(id, ..)| id == old_id) {
                continue;
            }
            let listed = keys.keys.iter().any(|key| &key.public_key == old_key);
            sqlx::query(
                "INSERT INTO publisher_key_history
                 (publisher_id, key_id, public_key, status, created_at, retired_at)
                 SELECT ?, ?, ?, ?, ?, ?
                 WHERE NOT EXISTS (
                     SELECT 1 FROM publisher_key_history WHERE publisher_id = ? AND key_id = ?
                 )",
            )
            .bind(record.id)
            .bind(old_id)
            .bind(old_key)
            .bind(if listed { "retired" } else { "revoked" })
            .bind(record.verified_at.unwrap_or(record.created_at))
            .bind(now)
            .bind(record.id)
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        }
        // A retired key the profile no longer lists is revoked.
        for history in sqlx::query_as::<_, PublisherKeyHistory>(
            "SELECT * FROM publisher_key_history WHERE publisher_id = ? AND status = 'retired'",
        )
        .bind(record.id)
        .fetch_all(&mut *tx)
        .await?
        {
            if !keys
                .keys
                .iter()
                .any(|key| key.public_key == history.public_key)
            {
                sqlx::query("UPDATE publisher_key_history SET status = 'revoked' WHERE id = ?")
                    .bind(history.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        Self::import_retired_keys(&mut tx, record.id, keys, now).await?;
        let updated = sqlx::query(
            "UPDATE publisher
             SET public_key = ?, key_id = ?, key_expires_at = ?, next_public_key = ?,
                 next_key_id = ?, next_key_expires_at = ?, profile_json = ?,
                 profile_pins = ?, contact = ?, updated_at = ?
             WHERE id = ? AND status = 'active' AND profile_json = ?",
        )
        .bind(&public_key)
        .bind(&key_id)
        .bind(key_expires_at)
        .bind(next.as_ref().map(|(_, key, _)| key))
        .bind(next.as_ref().map(|(id, ..)| id))
        .bind(next.as_ref().map(|(.., expires)| expires))
        .bind(&profile_json)
        .bind(&pins_json)
        .bind(contact)
        .bind(now)
        .bind(record.id)
        .bind(&record.profile_json)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(PublisherRegistryError::InvalidStatus(
                "publisher changed while its profile was registered".to_string(),
            ));
        }
        tx.commit().await?;
        platform::recording::info!(
            "Publisher Profile accepted: name={publisher}, sequence={}, key_id={key_id}",
            keys.sequence
        );
        Publisher::get(&self.pool, record.id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)
    }

    /// An accepted Profile also authorizes verification with retired package
    /// keys this Registry has never seen active. Import them in the same write
    /// transaction; an existing history row, especially a revocation, wins.
    async fn import_retired_keys(
        connection: &mut sqlx::SqliteConnection,
        publisher_id: i64,
        keys: &PublisherKeys,
        now: i64,
    ) -> Result<(), PublisherRegistryError> {
        for key in keys
            .keys
            .iter()
            .filter(|key| key.status == KeyStatus::Retired)
        {
            crypto::validate_public_key(&key.public_key)?;
            let key_id = crypto::compute_key_id_from_b64(&key.public_key)?;
            // Profiles do not carry a retirement timestamp. Record when this
            // Registry accepted the historical key, not an invented lifetime.
            sqlx::query(
                "INSERT INTO publisher_key_history
                 (publisher_id, key_id, public_key, status, created_at, retired_at)
                 SELECT ?, ?, ?, 'retired', ?, ?
                 WHERE NOT EXISTS (
                     SELECT 1 FROM publisher_key_history WHERE publisher_id = ? AND key_id = ?
                 )",
            )
            .bind(publisher_id)
            .bind(&key_id)
            .bind(&key.public_key)
            .bind(now)
            .bind(now)
            .bind(publisher_id)
            .bind(&key_id)
            .execute(&mut *connection)
            .await?;
        }
        Ok(())
    }

    /// Test-only shortcut for provisioning publisher records without external
    /// HTTPS discovery.
    ///
    /// If no public key is supplied, the helper creates a disposable test key
    /// and immediately discards its private half. Production code cannot call
    /// this path.
    #[cfg(test)]
    pub async fn admin_approve(
        &self,
        publisher_id: i64,
        user_public_key: Option<&str>,
    ) -> Result<PublisherVerificationResponse, PublisherRegistryError> {
        let mut publisher_record = Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;

        let generated_public_key;
        let public_key = match user_public_key {
            Some(public_key) => public_key,
            None => {
                generated_public_key = crypto::generate_keypair().1;
                &generated_public_key
            }
        };
        let response = self
            .activate_with_key(&mut publisher_record, public_key)
            .await?;
        platform::recording::info!(
            "test publisher approved: publisher_id={}, name={}, key_id={}",
            publisher_id,
            publisher_record.name,
            response.certificate.key_id
        );
        Ok(response)
    }

    /// Bind a publisher-supplied public key after ownership verification.
    #[cfg(test)]
    async fn activate_with_key(
        &self,
        publisher_record: &mut Publisher,
        public_key: &str,
    ) -> Result<PublisherVerificationResponse, PublisherRegistryError> {
        crypto::validate_public_key(public_key)?;

        publisher_record
            .activate(&self.pool, public_key.to_string())
            .await?;

        let expires_at = publisher_record
            .key_expires_at
            .ok_or(PublisherRegistryError::CertificateExpired)?;
        Ok(PublisherVerificationResponse {
            certificate: PublisherCertificate {
                key_id: publisher_record.key_id.clone(),
                publisher: publisher_record.name.clone(),
                public_key: publisher_record.public_key.clone(),
                issued_at: publisher_record
                    .verified_at
                    .unwrap_or(publisher_record.created_at),
                expires_at,
            },
        })
    }

    pub async fn get_status(&self, publisher_id: i64) -> Result<Publisher, PublisherRegistryError> {
        Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)
    }

    pub async fn resolve_by_name(
        &self,
        name: &str,
    ) -> Result<PublisherPublicInfo, PublisherRegistryError> {
        let publisher_record = Publisher::get_by_name(&self.pool, name)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        if publisher_record.status != PublisherStatus::Active {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "Publisher '{}' is not active",
                name
            )));
        }
        let expires_at = publisher_record
            .key_expires_at
            .ok_or(PublisherRegistryError::CertificateExpired)?;
        let cert = PublisherCertificate {
            key_id: publisher_record.key_id.clone(),
            publisher: publisher_record.name.clone(),
            public_key: publisher_record.public_key.clone(),
            issued_at: publisher_record
                .verified_at
                .unwrap_or(publisher_record.created_at),
            expires_at,
        };
        let profile = accepted_profile(&publisher_record.profile_json);
        Ok(PublisherPublicInfo {
            id: publisher_record.id,
            key_id: publisher_record.key_id.clone(),
            name: publisher_record.name,
            public_key: publisher_record.public_key,
            certificate: cert,
            profile,
        })
    }

    /// Resolve a specific historical (or active) key by exact key_id.
    pub async fn resolve_key_by_id(
        &self,
        name: &str,
        key_id: &str,
    ) -> Result<PublisherPublicInfo, PublisherRegistryError> {
        let publisher_record = Publisher::get_by_name(&self.pool, name)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        if publisher_record.status != PublisherStatus::Active {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "Publisher '{}' is not active",
                name
            )));
        }

        // Fail closed even on the current-key fast path: if this exact key_id
        // carries a revoked history row, refuse to serve it before the
        // short-circuit below could hand back the live public key.
        if PublisherKeyHistory::is_key_id_revoked(&self.pool, publisher_record.id, key_id).await? {
            platform::recording::warn!(
                "rejected resolution of revoked key on current-key path: \
                 publisher_id={}, key_id={}",
                publisher_record.id,
                key_id
            );
            return Err(PublisherRegistryError::KeyRevoked(key_id.to_string()));
        }

        // Is it the current key?
        if publisher_record.key_id == key_id {
            return self.resolve_by_name(name).await;
        }
        // The second active key while the publisher rotates.
        if publisher_record.next_key_id.as_deref() == Some(key_id)
            && let (Some(public_key), Some(expires_at)) = (
                publisher_record.next_public_key.clone(),
                publisher_record.next_key_expires_at,
            )
        {
            let profile = accepted_profile(&publisher_record.profile_json);
            return Ok(PublisherPublicInfo {
                id: publisher_record.id,
                key_id: key_id.to_string(),
                certificate: PublisherCertificate {
                    key_id: key_id.to_string(),
                    publisher: publisher_record.name.clone(),
                    public_key: public_key.clone(),
                    issued_at: publisher_record
                        .updated_at
                        .unwrap_or(publisher_record.created_at),
                    expires_at,
                },
                name: publisher_record.name,
                public_key,
                profile,
            });
        }

        let history = PublisherKeyHistory::get_by_key_id(&self.pool, publisher_record.id, key_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;

        let cert = PublisherCertificate {
            key_id: history.key_id.clone(),
            publisher: publisher_record.name.clone(),
            public_key: history.public_key.clone(),
            issued_at: history.created_at,
            expires_at: history.retired_at, // Old keys technically "expired" at retirement time
        };
        let profile = accepted_profile(&publisher_record.profile_json);

        Ok(PublisherPublicInfo {
            id: publisher_record.id,
            key_id: history.key_id.clone(),
            name: publisher_record.name,
            public_key: history.public_key,
            certificate: cert,
            profile,
        })
    }

    /// The trust decision for the package signing key `key_id` that a
    /// manifest of publisher `name` declares.
    ///
    /// A name this registry holds is reserved for its record: the key must be
    /// a non-revoked key of that active publisher, spelled exactly as the
    /// record. Any other outcome is an error, never [`PackageSigningKey::Untrusted`].
    /// Only a name with no record at all yields `Untrusted`.
    pub async fn package_signing_key(
        &self,
        name: &str,
        key_id: &str,
    ) -> Result<PackageSigningKey, PublisherRegistryError> {
        let canonical =
            canonical_publisher_domain(name).unwrap_or_else(|_| name.to_ascii_lowercase());
        let Some(record) = Publisher::get_by_name(&self.pool, &canonical).await? else {
            return Ok(PackageSigningKey::Untrusted);
        };
        if record.name != name {
            return Err(PublisherRegistryError::InvalidRequest(format!(
                "publisher '{name}' is spelled differently from its registry record '{}'",
                record.name
            )));
        }
        self.resolve_key_by_id(name, key_id)
            .await
            .map(PackageSigningKey::Trusted)
    }

    // Admin methods
    pub async fn admin_list(
        &self,
        status: Option<PublisherStatus>,
    ) -> Result<Vec<Publisher>, PublisherRegistryError> {
        Publisher::list(&self.pool, status).await
    }

    pub async fn admin_suspend(&self, publisher_id: i64) -> Result<(), PublisherRegistryError> {
        let mut publisher_record = Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        publisher_record.suspend(&self.pool).await?;
        platform::recording::warn!(
            "Publisher suspended by admin: publisher_id={}, name={}",
            publisher_id,
            publisher_record.name
        );
        Ok(())
    }

    pub async fn admin_reinstate(&self, publisher_id: i64) -> Result<(), PublisherRegistryError> {
        let mut publisher_record = Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        publisher_record.reinstate(&self.pool).await?;
        platform::recording::info!(
            "Publisher reinstated by admin: publisher_id={}, name={}",
            publisher_id,
            publisher_record.name
        );
        Ok(())
    }

    pub async fn admin_list_keys(
        &self,
        publisher_id: i64,
    ) -> Result<Vec<crate::model::key_history::PublisherKeyHistory>, PublisherRegistryError> {
        // Ensure the Publisher exists.
        let _ = Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        crate::model::key_history::PublisherKeyHistory::list_by_publisher(&self.pool, publisher_id)
            .await
    }

    pub async fn admin_revoke_historical_key(
        &self,
        history_id: i64,
    ) -> Result<(), PublisherRegistryError> {
        crate::model::key_history::PublisherKeyHistory::revoke(&self.pool, history_id).await?;
        platform::recording::warn!(
            "Publisher historical key revoked: history_id={}",
            history_id
        );
        Ok(())
    }

    /// Emergency-revoke a publisher's CURRENTLY ACTIVE signing key.
    ///
    /// `admin_revoke_historical_key` only covers keys already rotated away; this
    /// handles a compromised live key. Afterwards the publisher holds no usable
    /// key (key resolution fails closed) and must register a later
    /// Publisher Profile with a fresh key - the rollback blacklist prevents
    /// reinstating the revoked one. See `Publisher::revoke_active_key` for the exact post-state.
    pub async fn admin_revoke_active_key(
        &self,
        publisher_id: i64,
    ) -> Result<(), PublisherRegistryError> {
        let mut publisher_record = Publisher::get(&self.pool, publisher_id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)?;
        publisher_record.revoke_active_key(&self.pool).await?;
        platform::recording::warn!(
            "Publisher active signing key revoked by admin: publisher_id={}, name={}",
            publisher_id,
            publisher_record.name
        );
        Ok(())
    }

    pub async fn admin_delete(&self, publisher_id: i64) -> Result<(), PublisherRegistryError> {
        Publisher::delete(&self.pool, publisher_id).await?;
        platform::recording::warn!("Publisher deleted by admin: publisher_id={}", publisher_id);
        Ok(())
    }
}

/// The stored Publisher Profile, if one was accepted.
fn accepted_profile(profile_json: &str) -> Option<Value> {
    serde_json::from_str::<Value>(profile_json)
        .ok()
        .filter(|profile| profile.get("signature").is_some())
}
