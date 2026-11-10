//! Realm membership (`actr/protocols/realm-membership.md`): Realm client key
//! and invite ids, the invite text, membership requests and the
//! `x-actr-client-proof` header.
//!
//! Membership requests and client proofs are Actr management envelopes
//! (`realm-profile.md` section 8) signed by a Realm client key: the members
//! plus a detached Ed25519 `signature` over the RFC 8785 JCS form of the
//! envelope without `signature`.

use crate::authority::{DetachedSignature, KeyStatus, RealmSigningKey, SIGNATURE_ALGORITHM};
use crate::realm_management::check_window;
use crate::realm_profile::{RealmPackage, jcs, sign_document, verify_signature};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const REALM_MEMBERSHIP_PROTOCOL: &str = "actr-realm-membership/1";

/// HTTP request header carrying the client proof.
pub const CLIENT_PROOF_HEADER: &str = "x-actr-client-proof";

/// `schema` of a Realm invite.
pub const REALM_INVITE_SCHEMA: &str = "actr.realm-invite/v1";

/// Prefix of the invite text.
pub const REALM_INVITE_PREFIX: &str = "ri_";

/// Longest accepted nonce.
pub const MAX_NONCE_LEN: usize = 128;

/// Longest client `label`, in characters.
pub const MAX_LABEL_CHARS: usize = 64;

/// Longest invite lifetime, in seconds.
pub const MAX_INVITE_SECS: i64 = 30 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MembershipError(pub String);

fn invalid(message: impl Into<String>) -> MembershipError {
    MembershipError(message.into())
}

/// Lowercase hex of the first eight bytes of SHA-256 of `bytes`.
fn short_id(bytes: &[u8]) -> String {
    hex::encode(&Sha256::digest(bytes)[..8])
}

/// `key_id` of a Realm client key: the first 16 lowercase hex characters of
/// the SHA-256 of its 32-byte Ed25519 public key.
pub fn client_key_id(public_key: &[u8; 32]) -> String {
    short_id(public_key)
}

/// The 32 bytes of an invite `secret` (base64url without padding).
pub fn invite_secret_bytes(secret: &str) -> Result<[u8; 32], MembershipError> {
    URL_SAFE_NO_PAD
        .decode(secret)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid("invite secret is not 32 bytes of base64url"))
}

/// The stored hash of an invite `secret`: SHA-256 of its bytes, 64
/// lowercase hex characters.
pub fn invite_secret_hash(secret: &str) -> Result<String, MembershipError> {
    Ok(hex::encode(Sha256::digest(invite_secret_bytes(secret)?)))
}

/// `invite_id` of an invite `secret`: the first 16 hex characters of its
/// hash.
pub fn invite_id(secret: &str) -> Result<String, MembershipError> {
    Ok(short_id(&invite_secret_bytes(secret)?))
}

/// Lowercase hex SHA-256 of `body`, the `body_sha256` of a client proof.
pub fn body_sha256(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// A Realm invite (section 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RealmInvite {
    pub schema: String,
    pub realm: String,
    /// 32 random bytes, base64url without padding.
    pub secret: String,
    /// Optional embedded Realm Profile; verified like a fetched one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
}

impl RealmInvite {
    pub fn new(realm: &str, secret: &str) -> Self {
        Self {
            schema: REALM_INVITE_SCHEMA.to_string(),
            realm: realm.to_string(),
            secret: secret.to_string(),
            profile: None,
        }
    }

    /// The text form: `ri_` and the base64url (no padding) of the JSON.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("invite serializes");
        format!("{REALM_INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    /// Parse and check the text form. An embedded `profile` must name the
    /// same Realm; its signature is checked by the caller.
    pub fn decode(text: &str) -> Result<Self, MembershipError> {
        let encoded = text
            .trim()
            .strip_prefix(REALM_INVITE_PREFIX)
            .ok_or_else(|| invalid("invite does not start with ri_"))?;
        let json = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|error| invalid(format!("invite is not base64url: {error}")))?;
        let invite: Self = serde_json::from_slice(&json)
            .map_err(|error| invalid(format!("malformed invite: {error}")))?;
        if invite.schema != REALM_INVITE_SCHEMA {
            return Err(invalid(format!(
                "unsupported invite schema '{}'",
                invite.schema
            )));
        }
        check_realm_name(&invite.realm)?;
        invite_secret_bytes(&invite.secret)?;
        if let Some(profile) = &invite.profile
            && profile["realm"]["name"].as_str() != Some(invite.realm.as_str())
        {
            return Err(invalid("the embedded Realm Profile names another Realm"));
        }
        Ok(invite)
    }
}

/// A normalized Realm name under a DNS domain.
fn check_realm_name(name: &str) -> Result<(), MembershipError> {
    let uri =
        crate::uri::parse_realm_name(name).map_err(|error| invalid(format!("realm: {error}")))?;
    if uri.is_local() || uri.realm_name().as_deref() != Some(name) {
        return Err(invalid(format!(
            "realm '{name}' is not a normalized Realm name under a DNS domain"
        )));
    }
    Ok(())
}

/// Member role (section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Editor => "editor",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "owner" => Some(Self::Owner),
            "editor" => Some(Self::Editor),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

impl From<Role> for crate::MemberRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Owner => Self::Owner,
            Role::Editor => Self::Editor,
            Role::Viewer => Self::Viewer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipOp {
    Join,
    CreateInvite,
    RevokeInvite,
    ListMembers,
    RevokeClient,
    RemoveMember,
    SetRole,
    TransferOwner,
    Leave,
    UpdateRealm,
}

/// A membership request (section 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipRequest {
    pub op: MembershipOp,
    pub realm: String,
    /// Actr Facility Provider ID of the receiver.
    pub provider_id: String,
    /// `join`: the invite secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
    /// `join`: the new Realm client key, base64 Ed25519.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// `join`: client name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `create_invite` (new member), `set_role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// `create_invite` (new client), `remove_member`, `set_role`,
    /// `transfer_owner`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    /// `create_invite`: RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_expires_at: Option<String>,
    /// `revoke_invite`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_id: Option<String>,
    /// `revoke_client`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// `transfer_owner`: the previous owner's new role, `editor` if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_owner_role: Option<Role>,
    /// `update_realm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<RealmPackage>,
    /// `update_realm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// `update_realm`: only `retired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub nonce: String,
    /// RFC 3339.
    pub issued_at: String,
    /// RFC 3339, at most five minutes after `issued_at`.
    pub expires_at: String,
    /// Absent before signing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<DetachedSignature>,
    /// JCS form of the received request without `signature`.
    #[serde(skip)]
    signed_payload: String,
}

fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Parse a signed envelope `document`: the value and the JCS form of it
/// without `signature`.
fn parse_envelope<T: serde::de::DeserializeOwned>(
    document: &[u8],
) -> Result<(T, String), MembershipError> {
    let mut value: Value = serde_json::from_slice(document)
        .map_err(|error| invalid(format!("not valid JSON: {error}")))?;
    let parsed: T = serde_json::from_value(value.clone())
        .map_err(|error| invalid(format!("malformed request: {error}")))?;
    if let Value::Object(members) = &mut value {
        members.remove("signature");
    }
    Ok((parsed, jcs(&value)))
}

/// Check the envelope members every request shares.
fn check_common(
    provider_id: &str,
    expected_provider: &str,
    nonce: &str,
    issued_at: &str,
    expires_at: &str,
    now: DateTime<Utc>,
) -> Result<(), MembershipError> {
    if provider_id != expected_provider {
        return Err(invalid(format!(
            "provider_id '{provider_id}' is not this Actr Facility Provider"
        )));
    }
    if nonce.is_empty() || nonce.len() > MAX_NONCE_LEN {
        return Err(invalid(format!(
            "nonce must have 1 to {MAX_NONCE_LEN} characters"
        )));
    }
    check_window(issued_at, expires_at, now).map_err(invalid)
}

/// Verify `signature` over `payload` with the Realm client key `public_key`,
/// whose `key_id` the signature must name.
fn verify_client_signature(
    signature: Option<&DetachedSignature>,
    payload: &str,
    public_key: &[u8; 32],
) -> Result<(), MembershipError> {
    let signature = signature.ok_or_else(|| invalid("request is not signed"))?;
    let key = RealmSigningKey {
        id: client_key_id(public_key),
        algorithm: SIGNATURE_ALGORITHM.to_string(),
        public_key: BASE64.encode(public_key),
        status: KeyStatus::Active,
    };
    verify_signature(std::slice::from_ref(&key), signature, payload.as_bytes())
        .map(|_| ())
        .map_err(|error| invalid(error.to_string()))
}

/// Decode a base64 Ed25519 public key.
pub fn decode_public_key(text: &str) -> Result<[u8; 32], MembershipError> {
    BASE64
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .filter(|bytes| ed25519_dalek::VerifyingKey::from_bytes(bytes).is_ok())
        .ok_or_else(|| invalid("public_key is not a base64 Ed25519 public key"))
}

impl MembershipRequest {
    /// An unsigned request valid for five minutes from now. `nonce` must be
    /// random and never reused.
    pub fn new(op: MembershipOp, realm: &str, provider_id: &str, nonce: &str) -> Self {
        let now = Utc::now();
        Self {
            op,
            realm: realm.to_string(),
            provider_id: provider_id.to_string(),
            invite: None,
            public_key: None,
            label: None,
            role: None,
            member_id: None,
            invite_expires_at: None,
            invite_id: None,
            key_id: None,
            previous_owner_role: None,
            package: None,
            display_name: None,
            status: None,
            nonce: nonce.to_string(),
            issued_at: rfc3339(now),
            expires_at: rfc3339(
                now + chrono::Duration::seconds(crate::realm_management::MAX_VALIDITY_SECS),
            ),
            signature: None,
            signed_payload: String::new(),
        }
    }

    /// The signed envelope, signed by the Realm client key `key`.
    pub fn sign(&self, key: &SigningKey) -> Value {
        let mut document = serde_json::to_value(self).expect("request serializes");
        let key_id = client_key_id(&key.verifying_key().to_bytes());
        sign_document(&mut document, &key_id, key);
        document
    }

    /// Parse a signed envelope.
    pub fn parse(document: &[u8]) -> Result<Self, MembershipError> {
        let (request, signed_payload): (Self, String) = parse_envelope(document)?;
        if request.signature.is_none() {
            return Err(invalid("request is not signed"));
        }
        Ok(Self {
            signed_payload,
            ..request
        })
    }

    /// The `key_id` the signature names.
    pub fn signer_key_id(&self) -> &str {
        self.signature
            .as_ref()
            .map(|signature| signature.key_id.as_str())
            .unwrap_or_default()
    }

    /// Check `provider_id`, `nonce`, the validity window and the Realm name.
    pub fn check(&self, provider_id: &str, now: DateTime<Utc>) -> Result<(), MembershipError> {
        check_realm_name(&self.realm)?;
        check_common(
            &self.provider_id,
            provider_id,
            &self.nonce,
            &self.issued_at,
            &self.expires_at,
            now,
        )
    }

    /// Verify the signature with the registered Realm client key
    /// `public_key`.
    pub fn verify(&self, public_key: &[u8; 32]) -> Result<(), MembershipError> {
        verify_client_signature(self.signature.as_ref(), &self.signed_payload, public_key)
    }

    /// `join`: verify the signature with the new key in `public_key`, whose
    /// `key_id` the signature must name; returns that key.
    pub fn verify_join(&self) -> Result<[u8; 32], MembershipError> {
        let public_key = decode_public_key(
            self.public_key
                .as_deref()
                .ok_or_else(|| invalid("join needs public_key"))?,
        )?;
        if self.signer_key_id() != client_key_id(&public_key) {
            return Err(invalid("signature key_id does not match public_key"));
        }
        self.verify(&public_key)?;
        Ok(public_key)
    }
}

/// Which Issuer entry a client proof is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofOp {
    Register,
    Reissue,
    Renew,
    RelayIdentity,
}

/// The `x-actr-client-proof` envelope (section 6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientProof {
    pub op: ProofOp,
    pub realm: String,
    pub provider_id: String,
    /// Lowercase hex SHA-256 of the HTTP request body.
    pub body_sha256: String,
    pub nonce: String,
    pub issued_at: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<DetachedSignature>,
    #[serde(skip)]
    signed_payload: String,
}

impl ClientProof {
    /// An unsigned proof for `body`, valid for five minutes from now.
    pub fn new(op: ProofOp, realm: &str, provider_id: &str, body: &[u8], nonce: &str) -> Self {
        let now = Utc::now();
        Self {
            op,
            realm: realm.to_string(),
            provider_id: provider_id.to_string(),
            body_sha256: body_sha256(body),
            nonce: nonce.to_string(),
            issued_at: rfc3339(now),
            expires_at: rfc3339(
                now + chrono::Duration::seconds(crate::realm_management::MAX_VALIDITY_SECS),
            ),
            signature: None,
            signed_payload: String::new(),
        }
    }

    /// The signed envelope.
    pub fn sign(&self, key: &SigningKey) -> Value {
        let mut document = serde_json::to_value(self).expect("proof serializes");
        let key_id = client_key_id(&key.verifying_key().to_bytes());
        sign_document(&mut document, &key_id, key);
        document
    }

    /// The header value: base64url (no padding) of the signed envelope.
    pub fn header_value(&self, key: &SigningKey) -> String {
        URL_SAFE_NO_PAD.encode(self.sign(key).to_string())
    }

    /// Parse a header value.
    pub fn parse_header(value: &str) -> Result<Self, MembershipError> {
        let document = URL_SAFE_NO_PAD
            .decode(value.trim())
            .map_err(|error| invalid(format!("client proof is not base64url: {error}")))?;
        let (proof, signed_payload): (Self, String) = parse_envelope(&document)?;
        if proof.signature.is_none() {
            return Err(invalid("client proof is not signed"));
        }
        Ok(Self {
            signed_payload,
            ..proof
        })
    }

    /// The `key_id` the signature names.
    pub fn signer_key_id(&self) -> &str {
        self.signature
            .as_ref()
            .map(|signature| signature.key_id.as_str())
            .unwrap_or_default()
    }

    /// Check that the proof is for `op` on `realm` with request `body`, for
    /// this Actr Facility Provider and within its validity window.
    pub fn check(
        &self,
        op: ProofOp,
        realm: &str,
        provider_id: &str,
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), MembershipError> {
        if self.op != op {
            return Err(invalid("client proof op does not match the request"));
        }
        if self.realm != realm {
            return Err(invalid("client proof realm does not match the request"));
        }
        if self.body_sha256 != body_sha256(body) {
            return Err(invalid("client proof body_sha256 does not match the body"));
        }
        check_common(
            &self.provider_id,
            provider_id,
            &self.nonce,
            &self.issued_at,
            &self.expires_at,
            now,
        )
    }

    /// Verify the signature with the Realm client key `public_key`.
    pub fn verify(&self, public_key: &[u8; 32]) -> Result<(), MembershipError> {
        verify_client_signature(self.signature.as_ref(), &self.signed_payload, public_key)
    }
}

/// A Realm client key as a file (JSON, readable only by its owner): what
/// `actr realm join` writes and an Actr Host signs its client proofs and
/// membership requests with.
#[derive(Clone, Serialize, Deserialize)]
pub struct RealmClientKeyFile {
    pub realm: String,
    /// Actr Facility Provider ID the requests name (`provider_id`).
    pub provider_id: String,
    /// Issuer URL (`issuer.url`); membership requests go to
    /// `<issuer>/membership`.
    pub issuer: String,
    pub key_id: String,
    /// Base64 Ed25519 private key (32 bytes).
    pub private_key: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
}

impl std::fmt::Debug for RealmClientKeyFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealmClientKeyFile")
            .field("realm", &self.realm)
            .field("provider_id", &self.provider_id)
            .field("issuer", &self.issuer)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl RealmClientKeyFile {
    pub fn new(realm: &str, provider_id: &str, issuer: &str, key: &SigningKey) -> Self {
        let public_key = key.verifying_key().to_bytes();
        Self {
            realm: realm.to_string(),
            provider_id: provider_id.to_string(),
            issuer: issuer.to_string(),
            key_id: client_key_id(&public_key),
            private_key: BASE64.encode(key.to_bytes()),
            public_key: BASE64.encode(public_key),
        }
    }

    /// Parse a key file and return it with its signing key; `key_id` and
    /// `public_key` must match the private key.
    pub fn parse(text: &str) -> Result<(Self, SigningKey), MembershipError> {
        let file: Self = serde_json::from_str(text)
            .map_err(|error| invalid(format!("not a Realm client key file: {error}")))?;
        let bytes: [u8; 32] = BASE64
            .decode(&file.private_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| invalid("private_key is not a base64 Ed25519 key"))?;
        let key = SigningKey::from_bytes(&bytes);
        let public_key = key.verifying_key().to_bytes();
        if file.key_id != client_key_id(&public_key) || file.public_key != BASE64.encode(public_key)
        {
            return Err(invalid("key_id or public_key does not match private_key"));
        }
        check_realm_name(&file.realm)?;
        Ok((file, key))
    }

    /// Read and [`parse`](Self::parse) the key file at `path`, refusing a
    /// file others may access (see [`read_private_key_file`]).
    pub fn read(path: &std::path::Path) -> Result<(Self, SigningKey), MembershipError> {
        let shown = path.display();
        let text = read_private_key_file(path, "Realm client key").map_err(invalid)?;
        Self::parse(&text).map_err(|error| invalid(format!("Realm client key {shown}: {error}")))
    }
}

/// Read a private key file. Like ssh with a private key, a file that group or
/// others may access is refused on Unix. Android and iOS are left out: their
/// app sandboxes, not file modes, keep other apps away. `what` names the file
/// in errors.
pub fn read_private_key_file(path: &std::path::Path, what: &str) -> Result<String, String> {
    use std::io::Read as _;
    let shown = path.display();
    let cannot_read = |error: std::io::Error| format!("cannot read {what} {shown}: {error}");
    // The mode is checked on the opened file, so it is the file read.
    let mut file = std::fs::File::open(path).map_err(cannot_read)?;
    #[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = file.metadata().map_err(cannot_read)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{what} {shown} is accessible by group or others (mode {:03o}); \
                 run `chmod 600 {shown}`",
                mode & 0o777
            ));
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(cannot_read)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REALM: &str = "k7m2q9xh@acme.com";

    #[test]
    fn ids_are_short_sha256_prefixes() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public_key = key.verifying_key().to_bytes();
        let id = client_key_id(&public_key);
        assert_eq!(id.len(), 16);
        assert_eq!(id, hex::encode(Sha256::digest(public_key))[..16]);

        let secret = URL_SAFE_NO_PAD.encode([9u8; 32]);
        let hash = invite_secret_hash(&secret).unwrap();
        assert_eq!(invite_id(&secret).unwrap(), hash[..16]);
        assert!(invite_secret_hash("short").is_err());
    }

    #[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
    #[test]
    fn a_key_file_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let key = SigningKey::from_bytes(&[5; 32]);
        let file = RealmClientKeyFile::new(REALM, "p.example", "https://p.example/issuer", &key);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("realm-client.json");
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        for mode in [0o644, 0o640, 0o604, 0o660] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = RealmClientKeyFile::read(&path).unwrap_err().to_string();
            assert!(error.contains("chmod 600"), "{mode:o}: {error}");
        }
        for mode in [0o600, 0o400] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                RealmClientKeyFile::read(&path).unwrap().1.to_bytes(),
                key.to_bytes()
            );
        }
    }

    #[test]
    fn invite_text_round_trips() {
        let invite = RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([1u8; 32]));
        let text = invite.encode();
        assert!(text.starts_with("ri_"));
        assert_eq!(RealmInvite::decode(&text).unwrap(), invite);

        let mut other = invite.clone();
        other.profile = Some(serde_json::json!({ "realm": { "name": "x@acme.com" } }));
        assert!(RealmInvite::decode(&other.encode()).is_err());
        other.realm = "k7m2q9xh@ACME.com".into();
        other.profile = None;
        assert!(RealmInvite::decode(&other.encode()).is_err());
        assert!(RealmInvite::decode("rx_abc").is_err());
    }

    #[test]
    fn join_verifies_with_its_own_key() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let mut request = MembershipRequest::new(MembershipOp::Join, REALM, "p.example", "n-1");
        request.invite = Some(URL_SAFE_NO_PAD.encode([1u8; 32]));
        request.public_key = Some(BASE64.encode(key.verifying_key().to_bytes()));
        let document = request.sign(&key);
        let parsed = MembershipRequest::parse(document.to_string().as_bytes()).unwrap();
        parsed.check("p.example", Utc::now()).unwrap();
        assert!(parsed.check("q.example", Utc::now()).is_err());
        assert_eq!(
            parsed.verify_join().unwrap(),
            key.verifying_key().to_bytes()
        );

        // Signed by another key than the one it registers.
        let other = SigningKey::from_bytes(&[4; 32]);
        let document = request.sign(&other);
        let parsed = MembershipRequest::parse(document.to_string().as_bytes()).unwrap();
        assert!(parsed.verify_join().is_err());
    }

    #[test]
    fn client_proof_binds_the_body() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let body = b"register body";
        let proof = ClientProof::new(ProofOp::Register, REALM, "p.example", body, "n-2");
        let header = proof.header_value(&key);
        let parsed = ClientProof::parse_header(&header).unwrap();
        let public_key = key.verifying_key().to_bytes();
        assert_eq!(parsed.signer_key_id(), client_key_id(&public_key));
        parsed.verify(&public_key).unwrap();
        parsed
            .check(ProofOp::Register, REALM, "p.example", body, Utc::now())
            .unwrap();
        assert!(
            parsed
                .check(ProofOp::Register, REALM, "p.example", b"other", Utc::now())
                .is_err()
        );
        assert!(
            parsed
                .check(ProofOp::Renew, REALM, "p.example", body, Utc::now())
                .is_err()
        );
        let other = SigningKey::from_bytes(&[6; 32]);
        assert!(parsed.verify(&other.verifying_key().to_bytes()).is_err());
    }

    #[test]
    fn key_file_round_trips_and_checks_its_key_id() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let file = RealmClientKeyFile::new(REALM, "p.example", "https://p.example/issuer", &key);
        let text = serde_json::to_string(&file).unwrap();
        let (parsed, parsed_key) = RealmClientKeyFile::parse(&text).unwrap();
        assert_eq!(
            parsed.key_id,
            client_key_id(&key.verifying_key().to_bytes())
        );
        assert_eq!(parsed_key.to_bytes(), key.to_bytes());
        assert!(!format!("{parsed:?}").contains(&parsed.private_key));

        let mut other = file.clone();
        other.key_id = "0000000000000000".into();
        assert!(RealmClientKeyFile::parse(&serde_json::to_string(&other).unwrap()).is_err());
    }
}
