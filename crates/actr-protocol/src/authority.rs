//! Public authority discovery data types.

use crate::realm_profile::{jcs, verify_signature};
use crate::uri::{ActrUri, is_link_prefix};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const AUTHORITY_DESCRIPTOR_VERSION: u32 = 1;
pub const ICE_CONFIG_PROTOCOL: &str = "actr-ice-config/1";
pub const PUBLISHER_PROFILE_SCHEMA: &str = "actr.publisher/v1";
pub const SIGNATURE_ALGORITHM: &str = "Ed25519";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityDescriptor {
    pub version: u32,
    pub authority: String,
    pub services: Vec<ServiceLink>,
    /// Path prefixes under `https://<domain>` that are Realm links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    /// Realm signing keys for Realms named `<realm_id>@<domain>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_signing_keys: Option<RealmSigningKeys>,
    /// Signed Publisher Profile of this authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<PublisherProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmSigningKeys {
    pub sequence: u64,
    pub keys: Vec<RealmSigningKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotations: Option<Vec<KeyRotation>>,
    pub signature: DetachedSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmSigningKey {
    pub id: String,
    pub algorithm: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    pub status: KeyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    Retired,
}

/// Signed by the `from` key over the JCS form of
/// `{from, to, public_key, sequence}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRotation {
    pub from: String,
    pub to: String,
    pub sequence: u64,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetachedSignature {
    pub key_id: String,
    pub algorithm: String,
    pub value: String,
}

/// Publisher Profile carried inline. Its full format belongs to the Actor
/// Package specification; only the members checked here are modeled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherProfile {
    pub schema: String,
    pub domain: String,
    pub signature: DetachedSignature,
    /// Remaining signed members, kept so the profile round-trips intact.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceLink {
    pub rel: String,
    pub href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

impl ServiceLink {
    pub fn new(rel: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            rel: rel.into(),
            href: href.into(),
            protocol: None,
        }
    }

    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum AuthorityDescriptorError {
    #[error("unsupported authority descriptor version {0}")]
    UnsupportedVersion(u32),
    #[error("authority descriptor contains no services")]
    MissingServices,
    #[error("authority descriptor is bound to '{actual}', expected '{expected}'")]
    AuthorityMismatch { expected: String, actual: String },
    #[error("invalid authority descriptor URI: {0}")]
    InvalidUri(String),
    #[error("unsupported service relation '{0}'")]
    InvalidRelation(String),
    #[error("required service relation '{0}' is missing")]
    MissingService(String),
    #[error("invalid Realm link prefix '{0}'")]
    InvalidLink(String),
    #[error("invalid realm_signing_keys: {0}")]
    InvalidRealmSigningKeys(String),
    #[error("invalid publisher: {0}")]
    InvalidPublisher(String),
}

const CORE_RELATIONS: &[&str] = &[
    "signaling",
    "identity",
    "discovery",
    "ice-config",
    "contracts",
    "realms",
    "realm-management",
];

fn is_absolute_uri(value: &str) -> bool {
    let Some((scheme, remainder)) = value.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && !remainder.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')))
        })
}

fn decodes_to(value: &str, len: usize) -> bool {
    BASE64.decode(value).is_ok_and(|bytes| bytes.len() == len)
}

fn check_signature(signature: &DetachedSignature) -> Result<(), String> {
    if signature.key_id.is_empty() {
        return Err("signature key_id is empty".into());
    }
    if signature.algorithm != SIGNATURE_ALGORITHM {
        return Err(format!(
            "unsupported signature algorithm '{}'",
            signature.algorithm
        ));
    }
    if !decodes_to(&signature.value, 64) {
        return Err("signature value is not a base64 Ed25519 signature".into());
    }
    Ok(())
}

/// Most `keys` and most `rotations` a signed key list may carry.
pub const MAX_SIGNING_KEYS: usize = 8;

/// Last known status of a pinned signing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinnedStatus {
    Active,
    Retired,
    /// No longer listed: revoked, and never accepted again.
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedKey {
    pub id: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    pub status: PinnedStatus,
}

/// What a client or Actr Facility Provider keeps for one domain's signing keys
/// (`actr/protocols/authority-discovery.md` section 4.4): every key it ever
/// accepted with its last status, and the accepted `sequence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPins {
    pub sequence: u64,
    /// SHA-256 hex of the accepted document's signed JCS payload, which an
    /// equal `sequence` must repeat.
    pub digest: String,
    pub keys: Vec<PinnedKey>,
}

/// Result of [`KeyPins::accept`] for a document that is not rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPinOutcome {
    /// The new pins.
    Accepted(KeyPins),
    /// The signing key does not chain to a pinned key. The pins are the
    /// document's own keys, to use once trust is confirmed again.
    IdentityChange(KeyPins),
}

/// A signed list of signing keys: a `realm_signing_keys` member, or the
/// `profile_signing_keys` of a Publisher Profile.
#[derive(Debug, Clone, Copy)]
pub struct SignedKeyList<'a> {
    pub sequence: u64,
    pub keys: &'a [RealmSigningKey],
    pub rotations: &'a [KeyRotation],
    pub signature: &'a DetachedSignature,
    /// RFC 8785 JCS form of the document without `signature`.
    pub payload: &'a str,
}

fn decode_key(public_key: &str) -> Result<ed25519_dalek::VerifyingKey, String> {
    let bytes: [u8; 32] = BASE64
        .decode(public_key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("public_key is not a base64 Ed25519 key")?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn verify_detached(public_key: &str, payload: &[u8], signature: &str) -> Result<(), String> {
    use ed25519_dalek::Verifier as _;
    let signature: [u8; 64] = BASE64
        .decode(signature)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("signature is not a base64 Ed25519 signature")?;
    decode_key(public_key)?
        .verify(payload, &ed25519_dalek::Signature::from_bytes(&signature))
        .map_err(|_| "signature verification failed".to_string())
}

/// The JCS payload a rotation's `from` key signs.
pub fn rotation_payload(rotation: &KeyRotation, public_key: &str) -> String {
    crate::realm_profile::jcs(&serde_json::json!({
        "from": rotation.from,
        "to": rotation.to,
        "public_key": public_key,
        "sequence": rotation.sequence,
    }))
}

impl KeyPins {
    /// Pins of a document accepted without a prior pin.
    fn first(list: &SignedKeyList<'_>) -> Self {
        Self {
            sequence: list.sequence,
            digest: payload_digest(list.payload),
            keys: list
                .keys
                .iter()
                .map(|key| PinnedKey {
                    id: key.id.clone(),
                    public_key: key.public_key.clone(),
                    status: match key.status {
                        KeyStatus::Active => PinnedStatus::Active,
                        KeyStatus::Retired => PinnedStatus::Retired,
                    },
                })
                .collect(),
        }
    }

    fn get(&self, id: &str) -> Option<&PinnedKey> {
        self.keys.iter().find(|key| key.id == id)
    }

    /// The pinned keys still listed, with their last status.
    pub fn signing_keys(&self) -> Vec<RealmSigningKey> {
        self.keys
            .iter()
            .filter_map(|key| {
                let status = match key.status {
                    PinnedStatus::Active => KeyStatus::Active,
                    PinnedStatus::Retired => KeyStatus::Retired,
                    PinnedStatus::Removed => return None,
                };
                Some(RealmSigningKey {
                    id: key.id.clone(),
                    algorithm: SIGNATURE_ALGORITHM.to_string(),
                    public_key: key.public_key.clone(),
                    status,
                })
            })
            .collect()
    }

    /// Run the acceptance procedure of `authority-discovery.md` section 4.4
    /// on `list`, structurally valid, against `pinned` (`None` for a first
    /// accept over HTTPS or an enterprise pin). `Err` means the document is
    /// rejected and `pinned` stays as it is.
    pub fn accept(
        pinned: Option<&KeyPins>,
        list: &SignedKeyList<'_>,
    ) -> Result<KeyPinOutcome, String> {
        let listed = |id: &str| list.keys.iter().find(|key| key.id == id);

        // A JCS number above 2^53 names more than one integer, so the
        // signatures would not bind the sequences.
        let exact = crate::realm_profile::MAX_EXACT_INTEGER;
        if list.sequence > exact || list.rotations.iter().any(|r| r.sequence > exact) {
            return Err(format!("sequences are limited to {exact}"));
        }

        // 1. Signed by an active listed key.
        let signer = listed(&list.signature.key_id)
            .filter(|key| key.status == KeyStatus::Active)
            .ok_or_else(|| format!("'{}' is not an active key", list.signature.key_id))?;
        verify_detached(
            &signer.public_key,
            list.payload.as_bytes(),
            &list.signature.value,
        )?;

        // 2. A pinned id keeps its key, a retired key stays retired, and a
        // removed key never comes back.
        if let Some(pinned) = pinned {
            for key in list.keys {
                let Some(saved) = pinned.get(&key.id) else {
                    continue;
                };
                if saved.public_key != key.public_key {
                    return Err(format!(
                        "key '{}' is listed with another public key",
                        key.id
                    ));
                }
                match (saved.status, key.status) {
                    (PinnedStatus::Removed, _) => {
                        return Err(format!("removed key '{}' is listed again", key.id));
                    }
                    (PinnedStatus::Retired, KeyStatus::Active) => {
                        return Err(format!("retired key '{}' is active again", key.id));
                    }
                    _ => {}
                }
            }
        }

        // 3. Every rotation between listed keys verifies.
        for rotation in list.rotations {
            if let (Some(from), Some(to)) = (listed(&rotation.from), listed(&rotation.to)) {
                verify_detached(
                    &from.public_key,
                    rotation_payload(rotation, &to.public_key).as_bytes(),
                    &rotation.signature,
                )
                .map_err(|error| format!("rotation '{}' -> '{}': {error}", from.id, to.id))?;
            }
        }

        let Some(pinned) = pinned else {
            return Ok(KeyPinOutcome::Accepted(Self::first(list)));
        };

        // 4. Trusted keys: pinned and still listed, plus keys a rotation from
        // a trusted, not retired key introduces after the accepted sequence.
        let mut trusted: Vec<&str> = list
            .keys
            .iter()
            .filter(|key| pinned.get(&key.id).is_some())
            .map(|key| key.id.as_str())
            .collect();
        loop {
            let introduced = list.rotations.iter().find(|rotation| {
                rotation.sequence > pinned.sequence
                    && trusted.contains(&rotation.from.as_str())
                    && !trusted.contains(&rotation.to.as_str())
                    && pinned.get(&rotation.to).is_none()
                    && listed(&rotation.to).is_some()
                    && pinned
                        .get(&rotation.from)
                        .is_none_or(|from| from.status != PinnedStatus::Retired)
            });
            match introduced {
                Some(rotation) => trusted.push(&rotation.to),
                None => break,
            }
        }

        // 6. A signing key outside the chain is an identity change.
        if !trusted.contains(&signer.id.as_str()) {
            return Ok(KeyPinOutcome::IdentityChange(Self::first(list)));
        }

        // 5. No rollback; an equal sequence repeats the accepted document.
        let digest = payload_digest(list.payload);
        if list.sequence < pinned.sequence
            || (list.sequence == pinned.sequence && digest != pinned.digest)
        {
            return Err(format!(
                "sequence {} rolls back accepted sequence {}",
                list.sequence, pinned.sequence
            ));
        }
        let mut keys: Vec<PinnedKey> = pinned
            .keys
            .iter()
            .map(|saved| PinnedKey {
                status: match listed(&saved.id) {
                    Some(key) if key.status == KeyStatus::Active => PinnedStatus::Active,
                    Some(_) => PinnedStatus::Retired,
                    None => PinnedStatus::Removed,
                },
                ..saved.clone()
            })
            .collect();
        for id in trusted {
            if pinned.get(id).is_none()
                && let Some(key) = listed(id)
            {
                keys.push(PinnedKey {
                    id: key.id.clone(),
                    public_key: key.public_key.clone(),
                    status: match key.status {
                        KeyStatus::Active => PinnedStatus::Active,
                        KeyStatus::Retired => PinnedStatus::Retired,
                    },
                });
            }
        }
        Ok(KeyPinOutcome::Accepted(Self {
            sequence: list.sequence,
            digest,
            keys,
        }))
    }
}

fn payload_digest(payload: &str) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(payload.as_bytes()))
}

impl RealmSigningKeys {
    /// Parse and check `member`, a `realm_signing_keys` member as received,
    /// and run [`KeyPins::accept`] on it.
    pub fn accept(
        member: &Value,
        pinned: Option<&KeyPins>,
    ) -> Result<(Self, KeyPinOutcome), AuthorityDescriptorError> {
        let invalid = AuthorityDescriptorError::InvalidRealmSigningKeys;
        let keys: Self = serde_json::from_value(member.clone())
            .map_err(|error| invalid(format!("malformed member: {error}")))?;
        keys.validate()?;
        let mut payload = member.clone();
        if let Value::Object(members) = &mut payload {
            members.remove("signature");
        }
        let payload = jcs(&payload);
        let outcome = KeyPins::accept(
            pinned,
            &SignedKeyList {
                sequence: keys.sequence,
                keys: &keys.keys,
                rotations: keys.rotations.as_deref().unwrap_or_default(),
                signature: &keys.signature,
                payload: &payload,
            },
        )
        .map_err(invalid)?;
        Ok((keys, outcome))
    }

    /// Structural checks only; signature verification and key pinning are
    /// left to the caller.
    pub fn validate(&self) -> Result<(), AuthorityDescriptorError> {
        self.check()
            .map_err(AuthorityDescriptorError::InvalidRealmSigningKeys)
    }

    fn check(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Err("keys is empty".into());
        }
        if self.keys.len() > MAX_SIGNING_KEYS
            || self
                .rotations
                .as_ref()
                .is_some_and(|r| r.len() > MAX_SIGNING_KEYS)
        {
            return Err(format!(
                "keys and rotations are limited to {MAX_SIGNING_KEYS} entries each"
            ));
        }
        for (index, key) in self.keys.iter().enumerate() {
            if key.id.is_empty() {
                return Err("key id is empty".into());
            }
            if self.keys[..index].iter().any(|other| other.id == key.id) {
                return Err(format!("duplicate key id '{}'", key.id));
            }
            if key.algorithm != SIGNATURE_ALGORITHM {
                return Err(format!(
                    "key '{}' has unsupported algorithm '{}'",
                    key.id, key.algorithm
                ));
            }
            if !decodes_to(&key.public_key, 32) {
                return Err(format!(
                    "key '{}' public_key is not a base64 Ed25519 key",
                    key.id
                ));
            }
        }
        if !self.keys.iter().any(|key| key.status == KeyStatus::Active) {
            return Err("no active key".into());
        }
        check_signature(&self.signature)?;
        if !self
            .keys
            .iter()
            .any(|key| key.id == self.signature.key_id && key.status == KeyStatus::Active)
        {
            return Err(format!(
                "signature key '{}' is not an active key",
                self.signature.key_id
            ));
        }
        for rotation in self.rotations.as_deref().unwrap_or_default() {
            if rotation.from.is_empty() || rotation.to.is_empty() || rotation.signature.is_empty() {
                return Err("rotation from/to/signature is empty".into());
            }
            // Rotations whose `from` is not in `keys` are ignored and do not
            // invalidate the member.
            let known = self.keys.iter().any(|key| key.id == rotation.from);
            if known && !decodes_to(&rotation.signature, 64) {
                return Err(format!(
                    "rotation '{}' -> '{}' signature is not a base64 Ed25519 signature",
                    rotation.from, rotation.to
                ));
            }
        }
        Ok(())
    }
}

impl PublisherProfile {
    /// Checks the descriptor-level rules: schema, `domain` equal to the
    /// authority host, and signature shape. The profile signature itself is
    /// verified under the Actor Package specification.
    pub fn validate_for(&self, domain: &str) -> Result<(), AuthorityDescriptorError> {
        let invalid = AuthorityDescriptorError::InvalidPublisher;
        if self.schema != PUBLISHER_PROFILE_SCHEMA {
            return Err(invalid(format!("unsupported schema '{}'", self.schema)));
        }
        if self.domain != domain {
            return Err(invalid(format!(
                "domain '{}' does not match authority host '{domain}'",
                self.domain
            )));
        }
        check_signature(&self.signature).map_err(invalid)
    }

    /// Verifies the profile for `domain` at `now`: [`Self::validate_for`],
    /// its key members, its validity period, and its signature by an
    /// `active` `profile_signing_keys` entry over the RFC 8785 JCS form of the
    /// member without `signature`. Pinning and `sequence` checks against an
    /// accepted profile are the caller's.
    pub fn verify(
        &self,
        domain: &str,
        now: DateTime<Utc>,
    ) -> Result<PublisherKeys, AuthorityDescriptorError> {
        self.verified(domain, now).map(|(keys, _)| keys)
    }

    /// [`Self::verify`], then [`KeyPins::accept`] on its
    /// `profile_signing_keys` and `rotations` against `pinned`, with the
    /// profile `sequence` as the member sequence (section 4.5).
    pub fn accept(
        &self,
        domain: &str,
        now: DateTime<Utc>,
        pinned: Option<&KeyPins>,
    ) -> Result<(PublisherKeys, KeyPinOutcome), AuthorityDescriptorError> {
        let (keys, payload) = self.verified(domain, now)?;
        let outcome = KeyPins::accept(
            pinned,
            &SignedKeyList {
                sequence: keys.sequence,
                keys: &keys.profile_signing_keys,
                rotations: keys.rotations.as_deref().unwrap_or_default(),
                signature: &self.signature,
                payload: &payload,
            },
        )
        .map_err(AuthorityDescriptorError::InvalidPublisher)?;
        Ok((keys, outcome))
    }

    fn verified(
        &self,
        domain: &str,
        now: DateTime<Utc>,
    ) -> Result<(PublisherKeys, String), AuthorityDescriptorError> {
        self.validate_for(domain)?;
        let invalid = AuthorityDescriptorError::InvalidPublisher;
        let mut payload = serde_json::to_value(self).map_err(|error| invalid(error.to_string()))?;
        if let Value::Object(members) = &mut payload {
            members.remove("signature");
        }
        let keys: PublisherKeys = serde_json::from_value(payload.clone())
            .map_err(|error| invalid(format!("malformed Publisher Profile: {error}")))?;
        keys.check(now).map_err(invalid)?;
        let payload = jcs(&payload);
        let key = verify_signature(
            &keys.profile_signing_keys,
            &self.signature,
            payload.as_bytes(),
        )
        .map_err(|error| invalid(error.to_string()))?;
        if key.status != KeyStatus::Active {
            return Err(invalid(format!(
                "Publisher Profile signing key '{}' is not active",
                key.id
            )));
        }
        Ok((keys, payload))
    }
}

/// Most package signing keys a Publisher Profile lists as `active`.
pub const MAX_ACTIVE_PACKAGE_KEYS: usize = 2;

/// Package signing key id: `actr-pub-` and the first 16 lowercase hex
/// characters of the SHA-256 of the raw 32-byte public key.
pub fn package_key_id(public_key: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(public_key));
    format!("actr-pub-{}", &digest[..16])
}

/// A package signing key listed in a Publisher Profile's `keys`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSigningKey {
    pub id: String,
    pub algorithm: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
    pub status: KeyStatus,
}

/// The key members of a Publisher Profile (`docs/product/actrium-v1` 05
/// section 2.2 and 12 section 2), returned by [`PublisherProfile::verify`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PublisherKeys {
    pub sequence: u64,
    /// Keys that sign the Publisher Profile; same members as Realm signing
    /// keys.
    pub profile_signing_keys: Vec<RealmSigningKey>,
    /// Package signing keys.
    pub keys: Vec<PackageSigningKey>,
    /// Rotation chain of `profile_signing_keys`.
    #[serde(default)]
    pub rotations: Option<Vec<KeyRotation>>,
    /// RFC 3339.
    pub profile_issued_at: String,
    /// RFC 3339.
    pub profile_expires_at: String,
}

impl PublisherKeys {
    fn check(&self, now: DateTime<Utc>) -> Result<(), String> {
        if self.profile_signing_keys.is_empty() {
            return Err("profile_signing_keys is empty".into());
        }
        if self.profile_signing_keys.len() > MAX_SIGNING_KEYS
            || self.keys.len() > MAX_SIGNING_KEYS
            || self
                .rotations
                .as_ref()
                .is_some_and(|r| r.len() > MAX_SIGNING_KEYS)
        {
            return Err(format!(
                "profile_signing_keys, keys and rotations are limited to {MAX_SIGNING_KEYS} entries each"
            ));
        }
        let active = self
            .keys
            .iter()
            .filter(|key| key.status == KeyStatus::Active)
            .count();
        if active > MAX_ACTIVE_PACKAGE_KEYS {
            return Err(format!(
                "at most {MAX_ACTIVE_PACKAGE_KEYS} package signing keys may be active"
            ));
        }
        for key in &self.keys {
            let expected = BASE64
                .decode(&key.public_key)
                .map(|bytes| package_key_id(&bytes))
                .unwrap_or_default();
            if key.id != expected {
                return Err(format!(
                    "package signing key id '{}' is not derived from its public key",
                    key.id
                ));
            }
        }
        let listed = self
            .profile_signing_keys
            .iter()
            .map(|key| (&key.id, &key.algorithm, &key.public_key))
            .chain(
                self.keys
                    .iter()
                    .map(|key| (&key.id, &key.algorithm, &key.public_key)),
            );
        let mut ids = Vec::new();
        for (id, algorithm, public_key) in listed {
            if id.is_empty() || ids.contains(&id) {
                return Err(format!("key id '{id}' is empty or listed twice"));
            }
            ids.push(id);
            if algorithm != SIGNATURE_ALGORITHM || !decodes_to(public_key, 32) {
                return Err(format!("key '{id}' is not a base64 Ed25519 key"));
            }
        }
        let time = |value: &str, member: &str| {
            DateTime::parse_from_rfc3339(value)
                .map(|time| time.with_timezone(&Utc))
                .map_err(|error| format!("{member}: {error}"))
        };
        let issued_at = time(&self.profile_issued_at, "profile_issued_at")?;
        let expires_at = time(&self.profile_expires_at, "profile_expires_at")?;
        if now < issued_at || now > expires_at {
            return Err("Publisher Profile is outside its validity period".into());
        }
        Ok(())
    }
}

/// Parses an `authority` member, which must be `actr://<domain>` with no
/// port, path, trailing slash, query, fragment, or user information.
pub fn parse_authority_member(value: &str) -> Result<ActrUri, AuthorityDescriptorError> {
    let invalid = |reason: &str| AuthorityDescriptorError::InvalidUri(format!("{value}: {reason}"));
    let domain = value
        .strip_prefix("actr://")
        .ok_or_else(|| invalid("expected actr://<domain>"))?;
    if domain.contains(['@', '/', '?', '#', ':']) {
        return Err(invalid("expected actr://<domain> without port or path"));
    }
    let uri = ActrUri::from_domain(domain).map_err(|error| invalid(&error.to_string()))?;
    if uri.is_local() {
        return Err(invalid("a local Realm domain has no authority descriptor"));
    }
    Ok(uri)
}

impl AuthorityDescriptor {
    pub fn validate_for(&self, requested: &ActrUri) -> Result<(), AuthorityDescriptorError> {
        if self.version != AUTHORITY_DESCRIPTOR_VERSION {
            return Err(AuthorityDescriptorError::UnsupportedVersion(self.version));
        }
        if self.services.is_empty() {
            return Err(AuthorityDescriptorError::MissingServices);
        }

        let described = parse_authority_member(&self.authority)?;
        if described.authority_uri() != requested.authority_uri() {
            return Err(AuthorityDescriptorError::AuthorityMismatch {
                expected: requested.authority_uri(),
                actual: described.authority_uri(),
            });
        }

        for service in &self.services {
            if !is_absolute_uri(&service.href) {
                return Err(AuthorityDescriptorError::InvalidUri(service.href.clone()));
            }
            if !CORE_RELATIONS.contains(&service.rel.as_str()) && !is_absolute_uri(&service.rel) {
                return Err(AuthorityDescriptorError::InvalidRelation(
                    service.rel.clone(),
                ));
            }
        }

        let links = self.links.as_deref().unwrap_or_default();
        for (index, link) in links.iter().enumerate() {
            if !is_link_prefix(link) || links[..index].contains(link) {
                return Err(AuthorityDescriptorError::InvalidLink(link.clone()));
            }
        }

        if let Some(keys) = &self.realm_signing_keys {
            keys.validate()?;
        }
        if let Some(publisher) = &self.publisher {
            publisher.validate_for(described.domain())?;
        }
        Ok(())
    }

    pub fn service(&self, relation: &str) -> Option<&ServiceLink> {
        self.services.iter().find(|service| service.rel == relation)
    }

    pub fn require_service(
        &self,
        relation: &str,
    ) -> Result<&ServiceLink, AuthorityDescriptorError> {
        self.service(relation)
            .ok_or_else(|| AuthorityDescriptorError::MissingService(relation.to_string()))
    }
}

/// Public, non-credential ICE information. Authorization-bearing deployments
/// can return short-lived credentials from the same endpoint after auth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceConfiguration {
    pub version: u32,
    pub ice_servers: Vec<IceServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(authority: &str) -> AuthorityDescriptor {
        AuthorityDescriptor {
            version: 1,
            authority: authority.to_string(),
            services: vec![
                ServiceLink::new("signaling", "wss://signal.example/actr")
                    .with_protocol("actr-signaling/1"),
            ],
            links: None,
            realm_signing_keys: None,
            publisher: None,
        }
    }

    fn b64(len: usize) -> String {
        BASE64.encode(vec![7u8; len])
    }

    fn signature(key_id: &str) -> DetachedSignature {
        DetachedSignature {
            key_id: key_id.to_string(),
            algorithm: "Ed25519".to_string(),
            value: b64(64),
        }
    }

    fn signing_key(id: &str, status: KeyStatus) -> RealmSigningKey {
        RealmSigningKey {
            id: id.to_string(),
            algorithm: "Ed25519".to_string(),
            public_key: b64(32),
            status,
        }
    }

    fn signing_keys() -> RealmSigningKeys {
        RealmSigningKeys {
            sequence: 3,
            keys: vec![
                signing_key("rs-2026-1", KeyStatus::Active),
                signing_key("rs-2025-1", KeyStatus::Retired),
            ],
            rotations: Some(vec![KeyRotation {
                from: "rs-2025-1".to_string(),
                to: "rs-2026-1".to_string(),
                sequence: 2,
                signature: b64(64),
            }]),
            signature: signature("rs-2026-1"),
        }
    }

    fn publisher(domain: &str) -> PublisherProfile {
        PublisherProfile {
            schema: "actr.publisher/v1".to_string(),
            domain: domain.to_string(),
            signature: signature("pub-1"),
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn validates_exact_authority_binding() {
        let uri = "actr://example.com/echo".parse::<ActrUri>().unwrap();
        descriptor("actr://example.com").validate_for(&uri).unwrap();

        let error = descriptor("actr://other.example")
            .validate_for(&uri)
            .unwrap_err();
        assert!(matches!(
            error,
            AuthorityDescriptorError::AuthorityMismatch { .. }
        ));
    }

    #[test]
    fn finds_service_by_relation() {
        let descriptor = descriptor("actr://example.com");
        let service = descriptor.require_service("signaling").unwrap();
        assert_eq!(service.protocol.as_deref(), Some("actr-signaling/1"));
        assert!(matches!(
            descriptor.require_service("contracts"),
            Err(AuthorityDescriptorError::MissingService(_))
        ));
    }

    #[test]
    fn rejects_retired_packages_relation() {
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        let mut descriptor = descriptor("actr://example.com");
        descriptor.services.push(ServiceLink::new(
            "packages",
            "https://packages.example/actr",
        ));
        assert!(matches!(
            descriptor.validate_for(&uri),
            Err(AuthorityDescriptorError::InvalidRelation(_))
        ));
    }

    #[test]
    fn rejects_noncanonical_authority_member() {
        let uri = "actr://example.com/echo".parse::<ActrUri>().unwrap();
        for authority in [
            "actr://example.com/not-an-authority",
            "actr://example.com/",
            "actr://example.com:443",
            "actr://example.com:8443",
            "actr://user@example.com",
            "actr://abcdefghijklmnopqrstuvwxyz.actr.alt",
            "actr://",
            "https://example.com",
        ] {
            let error = descriptor(authority).validate_for(&uri).unwrap_err();
            assert!(
                matches!(error, AuthorityDescriptorError::InvalidUri(_)),
                "{authority}: {error:?}"
            );
        }
    }

    #[test]
    fn validates_realm_link_prefixes() {
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        let mut descriptor = descriptor("actr://example.com");
        descriptor.links = Some(vec!["/i/".to_string(), "/apps/realms/".to_string()]);
        descriptor.validate_for(&uri).unwrap();

        for link in ["/", "i/", "/i", "//", "/i//", "/i?x/", "/i#x/"] {
            descriptor.links = Some(vec![link.to_string()]);
            assert!(
                matches!(
                    descriptor.validate_for(&uri),
                    Err(AuthorityDescriptorError::InvalidLink(_))
                ),
                "{link}"
            );
        }

        descriptor.links = Some(vec!["/i/".to_string(), "/i/".to_string()]);
        assert!(matches!(
            descriptor.validate_for(&uri),
            Err(AuthorityDescriptorError::InvalidLink(_))
        ));
    }

    #[test]
    fn accepts_realm_relations() {
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        let mut descriptor = descriptor("actr://example.com");
        descriptor.services.push(
            ServiceLink::new("realms", "https://realms.example/actr")
                .with_protocol("actr-realm-profile/1"),
        );
        descriptor.services.push(
            ServiceLink::new("realm-management", "https://realms.example/actr/manage")
                .with_protocol("actr-realm-management/1"),
        );
        descriptor.validate_for(&uri).unwrap();
    }

    #[test]
    fn ignores_unknown_members() {
        let parsed: AuthorityDescriptor = serde_json::from_value(serde_json::json!({
            "version": 1,
            "authority": "actr://example.com",
            "services": [{
                "rel": "signaling",
                "href": "wss://signal.example/actr",
                "x-extra": true
            }],
            "realm_signing_keys": {
                "sequence": 3,
                "keys": [{
                    "id": "rs-2026-1",
                    "algorithm": "Ed25519",
                    "public_key": b64(32),
                    "status": "active",
                    "x-extra": true
                }],
                "signature": { "key_id": "rs-2026-1", "algorithm": "Ed25519", "value": b64(64) },
                "x-extra": true
            },
            "publisher": {
                "schema": "actr.publisher/v1",
                "domain": "example.com",
                "name": "Example",
                "signature": { "key_id": "pub-1", "algorithm": "Ed25519", "value": b64(64) }
            },
            "x-future": 1
        }))
        .unwrap();
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        assert_eq!(parsed.validate_for(&uri), Ok(()));
        let publisher = parsed.publisher.unwrap();
        assert_eq!(publisher.domain, "example.com");
        assert_eq!(serde_json::to_value(&publisher).unwrap()["name"], "Example");
    }

    #[test]
    fn validates_realm_signing_keys() {
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        let mut descriptor = descriptor("actr://example.com");
        descriptor.realm_signing_keys = Some(signing_keys());
        descriptor.validate_for(&uri).unwrap();

        let cases: Vec<fn(&mut RealmSigningKeys)> = vec![
            |keys| keys.keys.clear(),
            |keys| keys.keys[1].id = "rs-2026-1".to_string(),
            |keys| keys.keys[0].id.clear(),
            |keys| keys.keys[0].algorithm = "RS256".to_string(),
            |keys| keys.keys[0].public_key = b64(31),
            |keys| keys.keys[0].public_key = "not base64!".to_string(),
            |keys| keys.keys[0].status = KeyStatus::Retired,
            |keys| keys.signature.key_id = "rs-2025-1".to_string(),
            |keys| keys.signature.key_id = "unknown".to_string(),
            |keys| keys.signature.algorithm = "RS256".to_string(),
            |keys| keys.signature.value = b64(32),
            |keys| keys.rotations.as_mut().unwrap()[0].to.clear(),
            |keys| keys.rotations.as_mut().unwrap()[0].signature = String::new(),
            |keys| keys.rotations.as_mut().unwrap()[0].signature = b64(32),
        ];
        for (index, mutate) in cases.into_iter().enumerate() {
            let mut keys = signing_keys();
            mutate(&mut keys);
            descriptor.realm_signing_keys = Some(keys);
            assert!(
                matches!(
                    descriptor.validate_for(&uri),
                    Err(AuthorityDescriptorError::InvalidRealmSigningKeys(_))
                ),
                "case {index}"
            );
        }
    }

    #[test]
    fn rotation_from_unknown_key_is_accepted() {
        let mut keys = signing_keys();
        keys.rotations.as_mut().unwrap()[0].from = "rs-2019-1".to_string();
        keys.validate().unwrap();
        keys.rotations.as_mut().unwrap()[0].signature = "x".to_string();
        keys.validate().unwrap();
    }

    #[test]
    fn realm_signing_keys_reject_negative_sequence() {
        let mut json = serde_json::to_value(signing_keys()).unwrap();
        json["sequence"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<RealmSigningKeys>(json).is_err());
    }

    #[test]
    fn publisher_domain_must_equal_authority_host() {
        let uri = "actr://example.com".parse::<ActrUri>().unwrap();
        let mut descriptor = descriptor("actr://example.com");
        descriptor.publisher = Some(publisher("example.com"));
        descriptor.validate_for(&uri).unwrap();

        for profile in [
            publisher("other.example"),
            publisher("sub.example.com"),
            publisher(""),
            PublisherProfile {
                schema: "actr.publisher/v2".to_string(),
                ..publisher("example.com")
            },
            PublisherProfile {
                signature: DetachedSignature {
                    value: "x".to_string(),
                    ..signature("pub-1")
                },
                ..publisher("example.com")
            },
        ] {
            descriptor.publisher = Some(profile);
            assert!(matches!(
                descriptor.validate_for(&uri),
                Err(AuthorityDescriptorError::InvalidPublisher(_))
            ));
        }
    }

    #[test]
    fn signed_members_are_optional_on_the_wire() {
        let json = serde_json::to_value(descriptor("actr://example.com")).unwrap();
        assert!(json.get("realm_signing_keys").is_none());
        assert!(json.get("publisher").is_none());

        let mut with_keys = descriptor("actr://example.com");
        with_keys.realm_signing_keys = Some(signing_keys());
        let json = serde_json::to_value(&with_keys).unwrap();
        assert_eq!(json["realm_signing_keys"]["keys"][0]["status"], "active");
        let parsed: AuthorityDescriptor = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, with_keys);
    }

    #[test]
    fn links_member_is_optional_on_the_wire() {
        let json = serde_json::to_value(descriptor("actr://example.com")).unwrap();
        assert!(json.get("links").is_none());

        let parsed: AuthorityDescriptor = serde_json::from_value(serde_json::json!({
            "version": 1,
            "authority": "actr://example.com",
            "services": [{ "rel": "signaling", "href": "wss://signal.example/actr" }],
            "links": ["/i/"]
        }))
        .unwrap();
        assert_eq!(parsed.links.as_deref(), Some(&["/i/".to_string()][..]));
    }

    /// A Publisher Profile for `domain` signed by the first of
    /// `signing_keys`.
    fn signed_publisher(
        domain: &str,
        signing_keys: &[(&str, &ed25519_dalek::SigningKey, KeyStatus)],
    ) -> PublisherProfile {
        let key_json = |id: &str, key: &ed25519_dalek::SigningKey, status: KeyStatus| {
            serde_json::json!({
                "id": id,
                "algorithm": "Ed25519",
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
                "status": status,
            })
        };
        let package_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let mut document = serde_json::json!({
            "schema": "actr.publisher/v1",
            "sequence": 1,
            "domain": domain,
            "profile_signing_keys": signing_keys
                .iter()
                .map(|(id, key, status)| key_json(id, key, *status))
                .collect::<Vec<_>>(),
            "keys": [key_json(
                &package_key_id(package_key.verifying_key().as_bytes()),
                &package_key,
                KeyStatus::Active,
            )],
            "security_contact": "mailto:security@example.com",
            "profile_issued_at": "2026-01-01T00:00:00Z",
            "profile_expires_at": "2027-01-01T00:00:00Z",
        });
        crate::realm_profile::sign_document(&mut document, signing_keys[0].0, signing_keys[0].1);
        serde_json::from_value(document).unwrap()
    }

    #[test]
    fn verifies_signed_publisher_profiles() {
        let root = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let now: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
        let profile = signed_publisher("example.com", &[("root-1", &root, KeyStatus::Active)]);
        let keys = profile.verify("example.com", now).unwrap();
        assert_eq!(keys.sequence, 1);
        assert!(keys.keys[0].id.starts_with("actr-pub-"));

        let rejected = |profile: &PublisherProfile, now: DateTime<Utc>| {
            assert!(matches!(
                profile.verify("example.com", now),
                Err(AuthorityDescriptorError::InvalidPublisher(_))
            ));
        };
        // Another domain, an expired profile, a changed member.
        assert!(profile.verify("other.example", now).is_err());
        rejected(&profile, "2027-06-01T00:00:00Z".parse().unwrap());
        let mut tampered = profile.clone();
        tampered.other.insert("sequence".into(), 2.into());
        rejected(&tampered, now);
        // Signed by a retired key.
        rejected(
            &signed_publisher("example.com", &[("root-1", &root, KeyStatus::Retired)]),
            now,
        );
        // A key id listed twice.
        let mut twice = serde_json::to_value(&profile).unwrap();
        twice["keys"][0]["id"] = "root-1".into();
        crate::realm_profile::sign_document(&mut twice, "root-1", &root);
        rejected(&serde_json::from_value(twice).unwrap(), now);
        // A package key id that is not derived from its public key.
        let mut renamed = serde_json::to_value(&profile).unwrap();
        renamed["keys"][0]["id"] = "package-1".into();
        crate::realm_profile::sign_document(&mut renamed, "root-1", &root);
        rejected(&serde_json::from_value(renamed).unwrap(), now);
    }

    /// A Publisher Profile for example.com at `sequence` with `package_keys`
    /// and `rotations`, signed by `signer`.
    fn publisher_with(
        sequence: u64,
        roots: &[(&str, &ed25519_dalek::SigningKey, KeyStatus)],
        signer: (&str, &ed25519_dalek::SigningKey),
        package_keys: &[(&ed25519_dalek::SigningKey, KeyStatus)],
        rotations: Value,
    ) -> PublisherProfile {
        let key_json = |id: &str, key: &ed25519_dalek::SigningKey, status: KeyStatus| {
            serde_json::json!({
                "id": id,
                "algorithm": "Ed25519",
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
                "status": status,
            })
        };
        let mut document = serde_json::json!({
            "schema": "actr.publisher/v1",
            "sequence": sequence,
            "domain": "example.com",
            "profile_signing_keys": roots
                .iter()
                .map(|(id, key, status)| key_json(id, key, *status))
                .collect::<Vec<_>>(),
            "keys": package_keys
                .iter()
                .map(|(key, status)| {
                    key_json(&package_key_id(key.verifying_key().as_bytes()), key, *status)
                })
                .collect::<Vec<_>>(),
            "profile_issued_at": "2026-01-01T00:00:00Z",
            "profile_expires_at": "2027-01-01T00:00:00Z",
        });
        if !rotations.is_null() {
            document["rotations"] = rotations;
        }
        crate::realm_profile::sign_document(&mut document, signer.0, signer.1);
        serde_json::from_value(document).unwrap()
    }

    #[test]
    fn publisher_profile_signing_keys_rotate_along_a_chain() {
        use ed25519_dalek::Signer as _;
        let now: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
        let old = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let new = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);
        let package = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let next_package = ed25519_dalek::SigningKey::from_bytes(&[10; 32]);
        use KeyStatus::{Active, Retired};

        let first = publisher_with(
            1,
            &[("root-1", &old, Active)],
            ("root-1", &old),
            &[(&package, Active)],
            Value::Null,
        );
        let (_, outcome) = first.accept("example.com", now, None).unwrap();
        let KeyPinOutcome::Accepted(pins) = outcome else {
            panic!("first profile not accepted");
        };

        // root-1 -> root-2 at sequence 2, signed by the new key, while two
        // package signing keys overlap.
        let rotation = KeyRotation {
            from: "root-1".into(),
            to: "root-2".into(),
            sequence: 2,
            signature: String::new(),
        };
        let new_public = BASE64.encode(new.verifying_key().to_bytes());
        let signature = old.sign(rotation_payload(&rotation, &new_public).as_bytes());
        let rotations = serde_json::json!([{
            "from": "root-1", "to": "root-2", "sequence": 2,
            "signature": BASE64.encode(signature.to_bytes()),
        }]);
        let second = publisher_with(
            2,
            &[("root-2", &new, Active), ("root-1", &old, Retired)],
            ("root-2", &new),
            &[(&package, Active), (&next_package, Active)],
            rotations.clone(),
        );
        let (keys, outcome) = second.accept("example.com", now, Some(&pins)).unwrap();
        assert_eq!(keys.keys.len(), 2);
        assert!(matches!(outcome, KeyPinOutcome::Accepted(_)));

        // Without the rotation the new key is an identity change.
        let unchained = publisher_with(
            2,
            &[("root-2", &new, Active), ("root-1", &old, Retired)],
            ("root-2", &new),
            &[(&package, Active)],
            Value::Null,
        );
        let (_, outcome) = unchained.accept("example.com", now, Some(&pins)).unwrap();
        assert!(matches!(outcome, KeyPinOutcome::IdentityChange(_)));

        // A third active package signing key is not allowed.
        let third = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        let crowded = publisher_with(
            2,
            &[("root-2", &new, Active), ("root-1", &old, Retired)],
            ("root-2", &new),
            &[
                (&package, Active),
                (&next_package, Active),
                (&third, Active),
            ],
            rotations,
        );
        assert!(crowded.accept("example.com", now, Some(&pins)).is_err());
    }

    #[test]
    fn signatures_bind_sequences_exactly() {
        // Above 2^53 a JCS number no longer names one integer: a signature
        // over 2^53 would also cover 2^53 + 1.
        let key = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let mut member = serde_json::json!({
            "sequence": 1u64 << 53,
            "keys": [{
                "id": "rs-1", "algorithm": "Ed25519", "status": "active",
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
            }],
        });
        crate::realm_profile::sign_document(&mut member, "rs-1", &key);
        let mut bumped = member.clone();
        bumped["sequence"] = ((1u64 << 53) + 1).into();
        for member in [member, bumped] {
            assert!(RealmSigningKeys::accept(&member, None).is_err());
        }

        let now: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
        let package = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let big = publisher_with(
            1 << 53,
            &[("root-1", &key, KeyStatus::Active)],
            ("root-1", &key),
            &[(&package, KeyStatus::Active)],
            Value::Null,
        );
        assert!(big.accept("example.com", now, None).is_err());
    }

    #[test]
    fn only_rotations_newer_than_the_pin_introduce_keys() {
        use ed25519_dalek::Signer as _;
        let old = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let new = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);
        let key = |id: &str, key: &ed25519_dalek::SigningKey, status: &str| {
            serde_json::json!({
                "id": id, "algorithm": "Ed25519", "status": status,
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
            })
        };
        let pinned = |sequence: u64| {
            let mut member = serde_json::json!({
                "sequence": sequence, "keys": [key("rs-1", &old, "active")],
            });
            crate::realm_profile::sign_document(&mut member, "rs-1", &old);
            match RealmSigningKeys::accept(&member, None).unwrap().1 {
                KeyPinOutcome::Accepted(pins) => pins,
                other => panic!("{other:?}"),
            }
        };
        // rs-1 -> rs-2 introduced at sequence 3, then signed by rs-2 at 4.
        let rotation = KeyRotation {
            from: "rs-1".into(),
            to: "rs-2".into(),
            sequence: 3,
            signature: String::new(),
        };
        let public_key = BASE64.encode(new.verifying_key().to_bytes());
        let signature = old.sign(rotation_payload(&rotation, &public_key).as_bytes());
        let mut member = serde_json::json!({
            "sequence": 4,
            "keys": [key("rs-2", &new, "active"), key("rs-1", &old, "retired")],
            "rotations": [{
                "from": "rs-1", "to": "rs-2", "sequence": 3,
                "signature": BASE64.encode(signature.to_bytes()),
            }],
        });
        crate::realm_profile::sign_document(&mut member, "rs-2", &new);

        let outcome = |pins: KeyPins| RealmSigningKeys::accept(&member, Some(&pins)).unwrap().1;
        assert!(matches!(outcome(pinned(2)), KeyPinOutcome::Accepted(_)));
        // A client that already accepted sequence 3 without rs-2 does not
        // take the rotation from it.
        assert!(matches!(
            outcome(pinned(3)),
            KeyPinOutcome::IdentityChange(_)
        ));
    }
}
