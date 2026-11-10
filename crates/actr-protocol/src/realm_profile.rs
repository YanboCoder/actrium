//! Realm Profile (`actr/protocols/realm-profile.md`).
//!
//! A Realm Profile is a signed public document naming the package a Realm
//! runs and the Issuer and Actr Facility Provider that serve it. It is signed by a
//! Realm signing key listed in the domain's `realm_signing_keys` member.

use crate::authority::{
    AuthorityDescriptor, DetachedSignature, KeyStatus, RealmSigningKey, RealmSigningKeys,
    SIGNATURE_ALGORITHM,
};
use crate::uri::ActrUri;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use fluent_uri::{Uri, UriRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const REALM_PROFILE_SCHEMA: &str = "actr.realm/v1";
pub const REALM_PROFILE_PROTOCOL: &str = "actr-realm-profile/1";

/// Most keys `issuer.keys` lists.
pub const MAX_ISSUER_KEYS: usize = 3;

/// The credential `key_id` a Realm Profile issuer key id names: its decimal
/// form without leading zeros.
pub fn parse_issuer_key_id(id: &str) -> Option<u32> {
    if id.is_empty()
        || (id.len() > 1 && id.starts_with('0'))
        || !id.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    id.parse().ok()
}

/// Largest integer JCS (RFC 8785, via I-JSON) represents exactly.
pub(crate) const MAX_EXACT_INTEGER: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmProfile {
    pub schema: String,
    pub sequence: u64,
    pub realm: RealmInfo,
    /// Required when `realm.status` is `active`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<RealmPackage>,
    pub issuer: RealmIssuer,
    pub provider: ActrFacilityProvider,
    pub policy: RealmPolicy,
    /// RFC 3339 signing time, informational only.
    pub issued_at: String,
    pub signature: DetachedSignature,
    /// JCS form of the verified document without `signature`, unknown members
    /// included; compares content at an equal sequence.
    #[serde(skip)]
    signed_payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmInfo {
    pub name: String,
    pub display_name: String,
    pub status: RealmStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RealmStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmPackage {
    /// HTTPS download URL.
    pub href: String,
    /// Lowercase hex SHA-256 of the whole package.
    pub sha256: String,
    /// Actor type of the package manifest, `publisher/program@version`.
    pub r#type: String,
}

impl RealmPackage {
    /// Check `href`, `sha256` and `type`.
    pub fn check(&self) -> Result<(), String> {
        check_endpoint_url(&self.href, "https")
            .map_err(|error| format!("package.href: {error}"))?;
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("package.sha256 is not lowercase hex SHA-256".into());
        }
        self.actr_type()?;
        Ok(())
    }

    /// The parsed `type`.
    pub fn actr_type(&self) -> Result<crate::ActrType, String> {
        crate::ActrType::from_program_identity(&self.r#type)
            .map_err(|error| format!("package.type: {error}"))
    }
}

// Parse the original RFC 3986 URI without repairing missing authorities (for
// example, treating https:///path as https://path would accept an empty host).
fn check_endpoint_url(value: &str, scheme: &str) -> Result<(), String> {
    let uri = Uri::parse(value).map_err(|error| format!("invalid URI: {error}"))?;
    if uri.scheme().as_str() != scheme || uri.authority().is_none_or(|auth| auth.host().is_empty())
    {
        return Err(format!("must be a {scheme} URI with a nonempty host"));
    }
    Ok(())
}

fn check_ice_uri(value: &str) -> Result<(), String> {
    let invalid = || "must be a STUN/TURN URI with a nonempty host".to_string();
    let (scheme, address) = value.split_once(':').ok_or_else(invalid)?;
    if !matches!(scheme, "stun" | "stuns" | "turn" | "turns") {
        return Err(invalid());
    }
    // RFC 7064/7065 use scheme:host[:port], not scheme://authority/path.
    // Parse only their shared RFC 3986 host/port components as an authority;
    // this also validates bracketed IPv6, which is not a generic URI path.
    let reference = format!("//{address}");
    let uri = UriRef::parse(reference.as_str()).map_err(|_| invalid())?;
    let auth = uri.authority().ok_or_else(invalid)?;
    let valid_query = uri.query().is_none_or(|query| {
        matches!(scheme, "turn" | "turns")
            && query
                .as_str()
                .strip_prefix("transport=")
                .is_some_and(|transport| {
                    !transport.is_empty()
                        && transport
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                })
    });
    if auth.host().is_empty()
        || auth.has_userinfo()
        || !uri.path().is_empty()
        || uri.has_fragment()
        || !valid_query
    {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmIssuer {
    pub url: String,
    pub keys: Vec<IssuerKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerKey {
    pub id: String,
    pub algorithm: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    pub status: KeyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActrFacilityProvider {
    /// Actr Facility Provider ID: the provider's normalized DNS authority.
    pub id: String,
    pub signaling: Vec<SignalingEndpoint>,
    pub ice: Vec<IceEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalingEndpoint {
    pub url: String,
    pub priority: u32,
    pub control_plane_id: String,
    pub ticket_audience: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceEndpoint {
    pub urls: Vec<String>,
    /// Audience of short-lived TURN credentials; set on TURN entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_audience: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmPolicy {
    pub membership: String,
}

/// Realm Profile error. Each variant maps to one error code; see
/// [`RealmProfileError::code`].
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum RealmProfileError {
    #[error("invalid Realm Profile: {0}")]
    Invalid(String),
    #[error("Realm Profile sequence {candidate} rolls back accepted sequence {accepted}")]
    Rollback { accepted: u64, candidate: u64 },
    #[error("Realm Profile provider '{candidate}' differs from accepted provider '{accepted}'")]
    ProviderMismatch { accepted: String, candidate: String },
    #[error("Realm '{0}' is retired")]
    Retired(String),
    #[error("invalid realm_signing_keys: {0}")]
    KeysInvalid(String),
    /// No `realms` location produced a valid Realm Profile.
    #[error("Realm Profile unavailable: {0}")]
    Unavailable(String),
}

impl RealmProfileError {
    /// Stable error code for logs, tests, and developer tools.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "REALM_PROFILE_INVALID",
            Self::Rollback { .. } => "REALM_PROFILE_ROLLBACK",
            Self::ProviderMismatch { .. } => "ACTR_FACILITY_PROVIDER_MISMATCH",
            Self::Retired(_) => "REALM_RETIRED",
            Self::KeysInvalid(_) => "REALM_KEYS_INVALID",
            Self::Unavailable(_) => "REALM_PROFILE_UNAVAILABLE",
        }
    }
}

/// Parses a Realm name that can have a Realm Profile. Local Realms have none.
fn parse_realm_name(name: &str) -> Result<ActrUri, String> {
    let uri = crate::uri::parse_realm_name(name).map_err(|error| error.to_string())?;
    if uri.is_local() {
        return Err(format!("local Realm '{name}' has no Realm Profile"));
    }
    Ok(uri)
}

/// Realm Profile URLs for `realm_name`, in document order (section 3): each
/// `realms` link with protocol `actr-realm-profile/1`, a trailing `/` removed,
/// followed by `/<realm_id>`. Links with a query or fragment are skipped.
pub fn realm_profile_urls(
    descriptor: &AuthorityDescriptor,
    realm_name: &str,
) -> Result<Vec<String>, RealmProfileError> {
    let uri = parse_realm_name(realm_name).map_err(RealmProfileError::Invalid)?;
    let realm_id = uri.realm_id().unwrap_or_default();
    Ok(descriptor
        .services
        .iter()
        .filter(|link| {
            link.rel == "realms"
                && link.protocol.as_deref() == Some(REALM_PROFILE_PROTOCOL)
                && link.href.starts_with("https://")
                && !link.href.contains(['?', '#'])
        })
        .map(|link| {
            let href = link.href.strip_suffix('/').unwrap_or(&link.href);
            format!("{href}/{realm_id}")
        })
        .collect())
}

impl RealmProfile {
    /// Parses and verifies a Realm Profile document for `realm_name`
    /// (sections 4 and 5, checks 1 and 2).
    ///
    /// `keys` is the domain's accepted `realm_signing_keys` member; its own
    /// signature and pinning are the caller's responsibility. The signature is
    /// checked over the RFC 8785 JCS form of the received document without its
    /// `signature` member, so unknown members are covered.
    pub fn verify(
        document: &[u8],
        realm_name: &str,
        keys: &RealmSigningKeys,
    ) -> Result<Self, RealmProfileError> {
        let invalid = RealmProfileError::Invalid;
        keys.validate()
            .map_err(|error| RealmProfileError::KeysInvalid(error.to_string()))?;

        let mut value: Value = serde_json::from_slice(document)
            .map_err(|error| invalid(format!("not valid JSON: {error}")))?;
        let profile: Self = serde_json::from_value(value.clone())
            .map_err(|error| invalid(format!("malformed document: {error}")))?;
        profile.check(realm_name).map_err(invalid)?;

        if let Value::Object(members) = &mut value {
            members.remove("signature");
        }
        let signed_payload = jcs(&value);
        verify_signature(&keys.keys, &profile.signature, signed_payload.as_bytes())?;
        Ok(Self {
            signed_payload,
            ..profile
        })
    }

    /// The Realm issuer key that verifies credentials with header `key_id`
    /// (section 6.1), `active` or `retired`.
    pub fn issuer_key(&self, key_id: u32) -> Option<&IssuerKey> {
        self.issuer
            .keys
            .iter()
            .find(|key| parse_issuer_key_id(&key.id) == Some(key_id))
    }

    /// Checks a verified `candidate` against the accepted Realm Profile of the
    /// same Realm (section 5, check 3). Both must come from [`Self::verify`].
    pub fn check_successor(&self, candidate: &Self) -> Result<(), RealmProfileError> {
        if candidate.sequence < self.sequence
            || (candidate.sequence == self.sequence
                && candidate.signed_payload != self.signed_payload)
        {
            return Err(RealmProfileError::Rollback {
                accepted: self.sequence,
                candidate: candidate.sequence,
            });
        }
        if candidate.provider.id != self.provider.id {
            return Err(RealmProfileError::ProviderMismatch {
                accepted: self.provider.id.clone(),
                candidate: candidate.provider.id.clone(),
            });
        }
        if self.realm.status == RealmStatus::Retired {
            return Err(RealmProfileError::Retired(self.realm.name.clone()));
        }
        Ok(())
    }

    fn check(&self, realm_name: &str) -> Result<(), String> {
        if self.schema != REALM_PROFILE_SCHEMA {
            return Err(format!("unsupported schema '{}'", self.schema));
        }
        // Beyond this the signed JCS form no longer pins the sequence.
        if self.sequence > MAX_EXACT_INTEGER {
            return Err(format!("sequence {} exceeds 2^53 - 1", self.sequence));
        }
        let expected = parse_realm_name(realm_name)?.realm_name();
        if Some(&self.realm.name) != expected.as_ref() {
            return Err(format!(
                "realm.name '{}' does not match '{realm_name}'",
                self.realm.name
            ));
        }
        match (&self.package, self.realm.status) {
            (None, RealmStatus::Active) => return Err("active Realm has no package".into()),
            (Some(package), _) => package.check()?,
            (None, RealmStatus::Retired) => {}
        }
        check_endpoint_url(&self.issuer.url, "https")
            .map_err(|error| format!("issuer.url: {error}"))?;
        for (index, endpoint) in self.provider.signaling.iter().enumerate() {
            check_endpoint_url(&endpoint.url, "wss")
                .map_err(|error| format!("provider.signaling[{index}].url: {error}"))?;
        }
        for (index, endpoint) in self.provider.ice.iter().enumerate() {
            if endpoint.urls.is_empty() {
                return Err(format!("provider.ice[{index}].urls must not be empty"));
            }
            for (url_index, url) in endpoint.urls.iter().enumerate() {
                check_ice_uri(url)
                    .map_err(|error| format!("provider.ice[{index}].urls[{url_index}]: {error}"))?;
            }
        }
        let provider = ActrUri::from_domain(&self.provider.id)
            .map_err(|error| format!("provider.id: {error}"))?;
        if provider.domain() != self.provider.id || provider.is_local() {
            return Err(format!(
                "provider.id '{}' is not a normalized DNS authority",
                self.provider.id
            ));
        }
        chrono::DateTime::parse_from_rfc3339(&self.issued_at)
            .map_err(|error| format!("issued_at: {error}"))?;
        let keys = &self.issuer.keys;
        if keys.is_empty() || keys.len() > MAX_ISSUER_KEYS {
            return Err(format!("issuer.keys must list 1 to {MAX_ISSUER_KEYS} keys"));
        }
        for (index, key) in keys.iter().enumerate() {
            if parse_issuer_key_id(&key.id).is_none()
                || keys[..index].iter().any(|k| k.id == key.id)
            {
                return Err(format!(
                    "issuer key id '{}' is not a unique decimal credential key_id",
                    key.id
                ));
            }
            let valid_key = BASE64
                .decode(&key.public_key)
                .is_ok_and(|bytes| bytes.len() == 32);
            if key.algorithm != SIGNATURE_ALGORITHM || !valid_key {
                return Err(format!(
                    "issuer key '{}' is not a base64 Ed25519 key",
                    key.id
                ));
            }
        }
        if self.signature.algorithm != SIGNATURE_ALGORITHM {
            return Err(format!(
                "unsupported signature algorithm '{}'",
                self.signature.algorithm
            ));
        }
        Ok(())
    }
}

/// Verify a detached `signature` over `payload` with the listed key it
/// names, and return that key. Whether the key must be `active` is the
/// caller's rule.
pub(crate) fn verify_signature<'k>(
    keys: &'k [RealmSigningKey],
    signature: &DetachedSignature,
    payload: &[u8],
) -> Result<&'k RealmSigningKey, RealmProfileError> {
    let invalid = RealmProfileError::Invalid;
    if signature.algorithm != SIGNATURE_ALGORITHM {
        return Err(invalid(format!(
            "unsupported signature algorithm '{}'",
            signature.algorithm
        )));
    }
    let key = keys
        .iter()
        .find(|key| key.id == signature.key_id)
        .ok_or_else(|| {
            invalid(format!(
                "signature key '{}' is not a listed signing key",
                signature.key_id
            ))
        })?;
    let public_key: [u8; 32] = BASE64
        .decode(&key.public_key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| RealmProfileError::KeysInvalid(format!("key '{}'", key.id)))?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|error| RealmProfileError::KeysInvalid(format!("key '{}': {error}", key.id)))?;
    let value: [u8; 64] = BASE64
        .decode(&signature.value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid("signature value is not a base64 Ed25519 signature".into()))?;
    verifying_key
        .verify(payload, &Signature::from_bytes(&value))
        .map_err(|_| invalid("signature verification failed".into()))?;
    Ok(key)
}

/// Sign `document`, a JSON object, as Realm Profiles, `realm_signing_keys`
/// members and Actr management envelopes are signed: `signature` becomes a
/// detached Ed25519 signature by `key_id` over the RFC 8785 JCS form of the
/// document without `signature`.
pub fn sign_document(document: &mut Value, key_id: &str, key: &SigningKey) {
    if let Value::Object(members) = document {
        members.remove("signature");
    }
    let signature = key.sign(jcs(document).as_bytes());
    if let Value::Object(members) = document {
        members.insert(
            "signature".to_string(),
            serde_json::json!({
                "key_id": key_id,
                "algorithm": SIGNATURE_ALGORITHM,
                "value": BASE64.encode(signature.to_bytes()),
            }),
        );
    }
}

/// RFC 8785 JSON Canonicalization Scheme serialization.
pub fn jcs(value: &Value) -> String {
    let mut out = String::new();
    write_jcs(value, &mut out);
    out
}

fn write_jcs(value: &Value, out: &mut String) {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => {
            // serde_json escapes strings as JCS requires: only `"`, `\` and
            // control characters, with lowercase `\u00xx`.
            out.push_str(&value.to_string());
        }
        // JCS serializes every number as an IEEE 754 double, integers included.
        Value::Number(number) => match number.as_f64() {
            Some(n) => out.push_str(&es_number(n)),
            None => out.push_str(&number.to_string()),
        },
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_jcs(item, out);
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut entries: Vec<_> = members.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_jcs(item, out);
            }
            out.push('}');
        }
    }
}

/// ECMAScript `Number.prototype.toString` for a finite double.
fn es_number(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest round-trip digits, as ECMAScript does.
    let formatted = format!("{:e}", n.abs());
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n_exp = exponent.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= n_exp && n_exp <= 21 {
        format!("{digits}{}", "0".repeat((n_exp - k) as usize))
    } else if 0 < n_exp && n_exp <= 21 {
        let (int, frac) = digits.split_at(n_exp as usize);
        format!("{int}.{frac}")
    } else if -6 < n_exp && n_exp <= 0 {
        format!("0.{}{digits}", "0".repeat((-n_exp) as usize))
    } else {
        let (first, rest) = digits.split_at(1);
        let point = if rest.is_empty() { "" } else { "." };
        let exp_sign = if n_exp - 1 < 0 { "-" } else { "+" };
        format!("{first}{point}{rest}e{exp_sign}{}", (n_exp - 1).abs())
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{RealmSigningKey, ServiceLink};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    const NAME: &str = "k7m2q9xh@acme.com";

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&rand::random::<[u8; 32]>())
    }

    fn keys(listed: &[(&str, &SigningKey, KeyStatus)]) -> RealmSigningKeys {
        RealmSigningKeys {
            sequence: 1,
            keys: listed
                .iter()
                .map(|(id, key, status)| RealmSigningKey {
                    id: id.to_string(),
                    algorithm: "Ed25519".into(),
                    public_key: BASE64.encode(key.verifying_key().to_bytes()),
                    status: *status,
                })
                .collect(),
            rotations: None,
            // Structural placeholder: member signatures are verified elsewhere.
            signature: DetachedSignature {
                key_id: listed[0].0.to_string(),
                algorithm: "Ed25519".into(),
                value: BASE64.encode([0u8; 64]),
            },
        }
    }

    fn unsigned(sequence: u64) -> Value {
        json!({
            "schema": "actr.realm/v1",
            "sequence": sequence,
            "realm": { "name": NAME, "display_name": "Acme Notes", "status": "active" },
            "package": {
                "href": "https://cdn.notes.example/team-notes-1.4.2.actr",
                "sha256": "ab".repeat(32),
                "type": "notes.example/team-notes@1.4.2"
            },
            "issuer": {
                "url": "https://identity.provider.example/realms/acme.com/k7m2q9xh",
                "keys": [{
                    "id": "7", "algorithm": "Ed25519",
                    "public_key": BASE64.encode([1u8; 32]), "status": "active"
                }]
            },
            "provider": {
                "id": "provider.example",
                "signaling": [{
                    "url": "wss://signal-a.provider.example/ws", "priority": 10,
                    "control_plane_id": "provider.example/v1", "ticket_audience": "signal-a"
                }],
                "ice": [
                    { "urls": ["stun:stun.provider.example:3478"] },
                    { "urls": ["turns:turn.provider.example:5349"], "credential_audience": "turn-pool-1" }
                ]
            },
            "policy": { "membership": "invite-only" },
            "issued_at": "2026-07-29T00:00:00Z",
            "x-extension": { "b": 1, "a": [1.5, "\u{e9}"] }
        })
    }

    fn sign(mut profile: Value, key_id: &str, key: &SigningKey) -> Vec<u8> {
        let signature = key.sign(jcs(&profile).as_bytes());
        profile["signature"] = json!({
            "key_id": key_id,
            "algorithm": "Ed25519",
            "value": BASE64.encode(signature.to_bytes()),
        });
        serde_json::to_vec_pretty(&profile).unwrap()
    }

    #[test]
    fn verifies_signed_profile() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let profile = RealmProfile::verify(&sign(unsigned(31), "rs-1", &key), NAME, &keys).unwrap();
        assert_eq!(profile.sequence, 31);
        assert_eq!(profile.provider.id, "provider.example");
        assert_eq!(profile.realm.status, RealmStatus::Active);
    }

    #[test]
    fn accepts_retired_listed_key() {
        let old = signing_key();
        let new = signing_key();
        let keys = keys(&[
            ("rs-2", &new, KeyStatus::Active),
            ("rs-1", &old, KeyStatus::Retired),
        ]);
        assert!(RealmProfile::verify(&sign(unsigned(1), "rs-1", &old), NAME, &keys).is_ok());
    }

    #[test]
    fn rejects_tampered_profile() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let mut document: Value = serde_json::from_slice(&sign(unsigned(1), "rs-1", &key)).unwrap();

        let tampers: [fn(&mut Value); 3] = [
            |d: &mut Value| d["package"]["href"] = json!("https://evil.example/x.actr"),
            |d: &mut Value| d["sequence"] = json!(99),
            |d: &mut Value| d["x-extension"]["b"] = json!(2),
        ];
        for tamper in tampers {
            let mut tampered = document.clone();
            tamper(&mut tampered);
            let error = RealmProfile::verify(&serde_json::to_vec(&tampered).unwrap(), NAME, &keys)
                .unwrap_err();
            assert_eq!(error.code(), "REALM_PROFILE_INVALID", "{error}");
        }

        document["signature"]["value"] = json!(BASE64.encode([0u8; 64]));
        let error =
            RealmProfile::verify(&serde_json::to_vec(&document).unwrap(), NAME, &keys).unwrap_err();
        assert_eq!(error.code(), "REALM_PROFILE_INVALID");
    }

    #[test]
    fn rejects_unlisted_or_foreign_key() {
        let key = signing_key();
        let other = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        for document in [
            sign(unsigned(1), "rs-9", &key),
            sign(unsigned(1), "rs-1", &other),
        ] {
            let error = RealmProfile::verify(&document, NAME, &keys).unwrap_err();
            assert_eq!(error.code(), "REALM_PROFILE_INVALID");
        }
    }

    #[test]
    fn rejects_invalid_members() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        type Mutate = fn(&mut Value);
        let cases: Vec<(&str, Mutate)> = vec![
            ("schema", |p| p["schema"] = json!("actr.realm/v2")),
            ("name", |p| p["realm"]["name"] = json!("other@acme.com")),
            ("name form", |p| {
                p["realm"]["name"] = json!("k7m2q9xh@ACME.com")
            }),
            ("package", |p| {
                p.as_object_mut().unwrap().remove("package");
            }),
            ("sha256", |p| {
                p["package"]["sha256"] = json!("AB".repeat(32))
            }),
            ("href", |p| {
                p["package"]["href"] = json!("http://cdn.example/x")
            }),
            ("type", |p| p["package"]["type"] = json!("team-notes@1.4.2")),
            ("type publisher", |p| {
                p["package"]["type"] = json!("local/team-notes@1.4.2")
            }),
            ("provider", |p| {
                p["provider"]["id"] = json!("Provider.example")
            }),
            ("issued_at", |p| p["issued_at"] = json!("yesterday")),
            ("status", |p| p["realm"]["status"] = json!("pending")),
            ("sequence", |p| p["sequence"] = json!(-1)),
            ("sequence range", |p| p["sequence"] = json!(1u64 << 53)),
            ("issuer key id", |p| {
                p["issuer"]["keys"][0]["id"] = json!("issuer-7")
            }),
            ("issuer key id zero", |p| {
                p["issuer"]["keys"][0]["id"] = json!("07")
            }),
            ("issuer key id range", |p| {
                p["issuer"]["keys"][0]["id"] = json!("4294967296")
            }),
            ("issuer keys empty", |p| p["issuer"]["keys"] = json!([])),
            ("issuer keys", |p| {
                let key = p["issuer"]["keys"][0].clone();
                let keys: Vec<Value> = (1..=4)
                    .map(|id| {
                        let mut key = key.clone();
                        key["id"] = json!(id.to_string());
                        key
                    })
                    .collect();
                p["issuer"]["keys"] = json!(keys);
            }),
            ("issuer key", |p| {
                p["issuer"]["keys"][0]["public_key"] = json!("short")
            }),
        ];
        for (label, mutate) in cases {
            let mut profile = unsigned(1);
            mutate(&mut profile);
            let error =
                RealmProfile::verify(&sign(profile, "rs-1", &key), NAME, &keys).unwrap_err();
            assert_eq!(error.code(), "REALM_PROFILE_INVALID", "{label}: {error}");
        }
    }

    #[test]
    fn finds_issuer_keys_by_credential_key_id() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let mut profile = unsigned(1);
        let mut old = profile["issuer"]["keys"][0].clone();
        old["id"] = json!("4294967295");
        old["status"] = json!("retired");
        profile["issuer"]["keys"].as_array_mut().unwrap().push(old);
        let profile = RealmProfile::verify(&sign(profile, "rs-1", &key), NAME, &keys).unwrap();
        assert_eq!(profile.issuer_key(7).unwrap().status, KeyStatus::Active);
        assert_eq!(
            profile.issuer_key(u32::MAX).unwrap().status,
            KeyStatus::Retired
        );
        assert!(profile.issuer_key(70).is_none());
        assert_eq!(parse_issuer_key_id("0"), Some(0));
        assert_eq!(parse_issuer_key_id("+7"), None);
    }

    #[test]
    fn retired_profile_needs_no_package() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let mut profile = unsigned(2);
        profile["realm"]["status"] = json!("retired");
        profile.as_object_mut().unwrap().remove("package");
        let profile = RealmProfile::verify(&sign(profile, "rs-1", &key), NAME, &keys).unwrap();
        assert_eq!(profile.realm.status, RealmStatus::Retired);
    }

    #[test]
    fn rejects_bad_names_and_keys() {
        let key = signing_key();
        let keys_ok = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let document = sign(unsigned(1), "rs-1", &key);
        for name in [
            "k7m2q9xh",
            "K7M2Q9XH@acme.com",
            "a.b@acme.com",
            "%6b7m2q9xh@acme.com",
            "k7m2q9xh@acme.com:443",
            "k7m2q9xh@ACME.com",
        ] {
            let error = RealmProfile::verify(&document, name, &keys_ok).unwrap_err();
            assert_eq!(error.code(), "REALM_PROFILE_INVALID", "{name}");
        }
        let local = format!("h3x8p2wq@{}.actr.alt", "a".repeat(26));
        assert!(RealmProfile::verify(&document, &local, &keys_ok).is_err());

        let mut broken = keys_ok.clone();
        broken.keys[0].public_key = "not base64".into();
        let error = RealmProfile::verify(&document, NAME, &broken).unwrap_err();
        assert_eq!(error.code(), "REALM_KEYS_INVALID");
    }

    #[test]
    fn checks_successor() {
        let key = signing_key();
        let keys = keys(&[("rs-1", &key, KeyStatus::Active)]);
        let verify = |profile: Value| {
            RealmProfile::verify(&sign(profile, "rs-1", &key), NAME, &keys).unwrap()
        };
        let accepted = verify(unsigned(31));

        assert!(accepted.check_successor(&verify(unsigned(32))).is_ok());
        assert!(accepted.check_successor(&verify(unsigned(31))).is_ok());

        let error = accepted.check_successor(&verify(unsigned(30))).unwrap_err();
        assert_eq!(error.code(), "REALM_PROFILE_ROLLBACK");

        let mut same_sequence = unsigned(31);
        same_sequence["realm"]["display_name"] = json!("Changed");
        let error = accepted
            .check_successor(&verify(same_sequence))
            .unwrap_err();
        assert_eq!(error.code(), "REALM_PROFILE_ROLLBACK");

        let mut same_sequence = unsigned(31);
        same_sequence["x-extension"] = json!("changed");
        let error = accepted
            .check_successor(&verify(same_sequence))
            .unwrap_err();
        assert_eq!(error.code(), "REALM_PROFILE_ROLLBACK");

        let mut moved = unsigned(32);
        moved["provider"]["id"] = json!("other.example");
        let error = accepted.check_successor(&verify(moved)).unwrap_err();
        assert_eq!(error.code(), "ACTR_FACILITY_PROVIDER_MISMATCH");

        let mut retired = unsigned(33);
        retired["realm"]["status"] = json!("retired");
        let retired = verify(retired);
        assert!(accepted.check_successor(&retired).is_ok());
        let error = retired.check_successor(&verify(unsigned(34))).unwrap_err();
        assert_eq!(error.code(), "REALM_RETIRED");
    }

    #[test]
    fn builds_profile_urls() {
        let mut descriptor = AuthorityDescriptor {
            version: 1,
            authority: "actr://acme.com".into(),
            services: vec![
                ServiceLink::new("realms", "https://acme.com/realms/")
                    .with_protocol(REALM_PROFILE_PROTOCOL),
                ServiceLink::new("realms", "https://acme.com/double//")
                    .with_protocol(REALM_PROFILE_PROTOCOL),
                ServiceLink::new("realms", "https://p.example:8443/r?x=1")
                    .with_protocol(REALM_PROFILE_PROTOCOL),
                ServiceLink::new("realms", "https://other.example/r"),
                ServiceLink::new("realms", "https://p.example:8443/r")
                    .with_protocol(REALM_PROFILE_PROTOCOL),
            ],
            links: None,
            realm_signing_keys: None,
            publisher: None,
        };
        assert_eq!(
            realm_profile_urls(&descriptor, NAME).unwrap(),
            vec![
                "https://acme.com/realms/k7m2q9xh",
                "https://acme.com/double//k7m2q9xh",
                "https://p.example:8443/r/k7m2q9xh"
            ]
        );
        descriptor.services.clear();
        assert!(realm_profile_urls(&descriptor, NAME).unwrap().is_empty());
    }

    #[test]
    fn canonicalizes_per_rfc_8785() {
        // Numbers from the RFC 8785 section 3.2.2 example, parsed as JSON text.
        let numbers: Value = serde_json::from_str(
            "[333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001]",
        )
        .unwrap();
        let value = json!({
            "numbers": numbers,
            "string": "\u{20ac}$\u{000F}\u{000a}A'\u{0042}\u{0022}\u{005c}\\\"",
            "literals": [null, true, false],
            "\u{20ac}": 1, "\r": 2, "\u{1f600}": 3, "\u{fb33}": 4, "1": 5
        });
        assert_eq!(
            jcs(&value),
            concat!(
                r#"{"\r":2,"1":5,"literals":[null,true,false],"#,
                r#""numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"#,
                "\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"\",",
                "\"\u{20ac}\":1,\"\u{1f600}\":3,\"\u{fb33}\":4}"
            )
        );
        // Integers beyond 2^53 serialize as the nearest double.
        let large: Value = serde_json::from_str("[9007199254740993, -9007199254740993]").unwrap();
        assert_eq!(jcs(&large), "[9007199254740992,-9007199254740992]");
        assert_eq!(
            jcs(&json!([0, -0, 31, u64::MAX])),
            "[0,0,31,18446744073709552000]"
        );
        assert_eq!(es_number(-0.5), "-0.5");
        assert_eq!(es_number(1e21), "1e+21");
        assert_eq!(es_number(123e18), "123000000000000000000");
        assert_eq!(es_number(1e-7), "1e-7");
        assert_eq!(es_number(0.000001), "0.000001");
    }
}
