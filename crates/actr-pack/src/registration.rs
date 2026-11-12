//! The signed manifest identity shared by package Hosts and Issuer.
//!
//! Issuer receives the original manifest and signature, not a ZIP. This view
//! verifies their identity binding without pretending to verify payload bytes
//! or Host compatibility. Versioned manifests use the full v2 schema; the
//! unversioned path retains only the legacy registration fields.

use actr_protocol::ActrType;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;

use crate::{ActorPackageManifest, PackError, compute_key_id};

/// Same manifest budget used by the package readers.
pub const MAX_REGISTRATION_MANIFEST_BYTES: usize = 1024 * 1024;

/// Parsed identity tied to the exact bytes whose signature will be verified.
#[derive(Debug)]
pub struct RegistrationManifest<'a> {
    raw: &'a [u8],
    actor_type: ActrType,
    signing_key_id: Option<String>,
    target: Option<String>,
}

#[derive(Deserialize)]
struct LegacyIdentity {
    publisher: String,
    name: String,
    version: String,
    signing_key_id: Option<String>,
    binary: LegacyBinary,
}

#[derive(Deserialize)]
struct LegacyBinary {
    target: String,
}

impl<'a> RegistrationManifest<'a> {
    /// Parse the manifest without granting trust to its locator fields.
    ///
    /// A format declaration or v2 table selects the v2 parser exclusively;
    /// missing/unknown versions and mixed schemas never fall back to legacy.
    pub fn parse(raw: &'a [u8]) -> Result<Self, PackError> {
        if raw.len() > MAX_REGISTRATION_MANIFEST_BYTES {
            return Err(PackError::InvalidPackage(
                "registration manifest exceeds 1 MiB".to_string(),
            ));
        }
        let source = std::str::from_utf8(raw)
            .map_err(|error| PackError::ManifestParseError(error.to_string()))?;
        let table: toml::Table = toml::from_str(source)
            .map_err(|error| PackError::ManifestParseError(error.to_string()))?;
        let versioned = ["format", "actor", "workload", "host", "signature"]
            .iter()
            .any(|field| table.contains_key(*field));
        let (actor_type, signing_key_id, target) = if versioned {
            let manifest = ActorPackageManifest::from_toml(source)?;
            (
                ActrType {
                    publisher: manifest.actor.publisher,
                    name: manifest.actor.name,
                    version: manifest.actor.version,
                },
                Some(manifest.signature.key_id),
                None,
            )
        } else {
            let legacy: LegacyIdentity =
                toml::Value::Table(table)
                    .try_into()
                    .map_err(|error: toml::de::Error| {
                        PackError::ManifestParseError(error.to_string())
                    })?;
            if legacy.binary.target.is_empty() {
                return Err(PackError::InvalidPackage(
                    "legacy manifest requires binary.target".to_string(),
                ));
            }
            (
                ActrType {
                    publisher: legacy.publisher,
                    name: legacy.name,
                    version: legacy.version,
                },
                legacy.signing_key_id,
                Some(legacy.binary.target),
            )
        };
        // Legacy Issuer callers retain their existing canonicalization/error
        // policy; the HTTP boundary validates every requested Actor type.
        if versioned {
            actor_type
                .validate()
                .map_err(|error| PackError::InvalidPackage(error.to_string()))?;
        }
        Ok(Self {
            raw,
            actor_type,
            signing_key_id,
            target,
        })
    }

    pub fn actor_type(&self) -> &ActrType {
        &self.actor_type
    }

    /// V2 always declares a key id. Legacy parsing also represents unsigned
    /// manifests so callers can preserve their missing-signature error policy.
    pub fn signing_key_id(&self) -> Option<&str> {
        self.signing_key_id
            .as_deref()
            .filter(|key_id| !key_id.trim().is_empty())
    }

    /// Legacy binary target; v2 requests must omit their optional target field.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Original signed bytes; no serialization or normalization is performed.
    pub fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// Verify the original bytes and bind the declared key id to this key.
    /// The caller must independently establish whether the key is trusted.
    pub fn verify_signature(
        &self,
        signature: &[u8],
        public_key: &VerifyingKey,
    ) -> Result<(), PackError> {
        if self.signing_key_id() != Some(compute_key_id(public_key.as_bytes()).as_str()) {
            return Err(PackError::SignatureVerificationFailed(
                "manifest signing key id does not match verifying key".to_string(),
            ));
        }
        let signature = Signature::from_slice(signature)
            .map_err(|error| PackError::SignatureVerificationFailed(error.to_string()))?;
        public_key
            .verify_strict(self.raw, &signature)
            .map_err(|error| PackError::SignatureVerificationFailed(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    const V2: &str = include_str!("../tests/fixtures/registration-v2.toml");

    #[test]
    fn verifies_original_v2_bytes_with_comments_and_crlf() {
        let key = SigningKey::generate(&mut OsRng);
        let source = V2
            .replace(
                "SIGNING_KEY_ID",
                &compute_key_id(key.verifying_key().as_bytes()),
            )
            .replace('\n', "\r\n");
        let signature = key.sign(source.as_bytes());
        let parsed = RegistrationManifest::parse(source.as_bytes()).unwrap();
        assert_eq!(parsed.raw(), source.as_bytes());
        assert_eq!(parsed.actor_type().publisher, "v2-publisher.example");
        assert_eq!(parsed.target(), None);
        parsed
            .verify_signature(&signature.to_bytes(), &key.verifying_key())
            .unwrap();

        // Re-serializing the parsed TOML changes the signed bytes.
        let normalized = ActorPackageManifest::from_toml(&source)
            .unwrap()
            .to_toml()
            .unwrap();
        assert_ne!(normalized.as_bytes(), source.as_bytes());
        assert!(
            RegistrationManifest::parse(normalized.as_bytes())
                .unwrap()
                .verify_signature(&signature.to_bytes(), &key.verifying_key())
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_missing_and_mixed_versions_instead_of_legacy_fallback() {
        let legacy = "publisher = 'acme'\nname = 'echo'\nversion = '1.0.0'\n\
                      signing_key_id = 'key'\n[binary]\ntarget = 'wasm32-wasip2'\n";
        assert_eq!(
            RegistrationManifest::parse(legacy.as_bytes())
                .unwrap()
                .target(),
            Some("wasm32-wasip2")
        );
        for source in [
            format!("format = 'actr-package/3'\n{legacy}"),
            format!("format = 'actr-package/1'\n{legacy}"),
            format!("format = 2\n{legacy}"),
            format!("{legacy}\n[actor]\npublisher = 'acme'\n"),
            V2.replace("format = \"actr-package/2\"", ""),
            V2.replace("algorithm = \"ed25519\"", "algorithm = \"rsa\""),
        ] {
            assert!(RegistrationManifest::parse(source.as_bytes()).is_err());
        }
    }

    #[test]
    fn rejects_oversized_or_non_utf8_manifests() {
        assert!(
            RegistrationManifest::parse(&vec![b' '; MAX_REGISTRATION_MANIFEST_BYTES + 1]).is_err()
        );
        assert!(RegistrationManifest::parse(&[0xff]).is_err());
    }

    #[test]
    fn rejects_signed_key_id_mismatch_and_invalid_signature_length() {
        let key = SigningKey::generate(&mut OsRng);
        let source = V2.replace("SIGNING_KEY_ID", "different-key-id");
        let signature = key.sign(source.as_bytes());
        assert!(
            RegistrationManifest::parse(source.as_bytes())
                .unwrap()
                .verify_signature(&signature.to_bytes(), &key.verifying_key())
                .is_err()
        );
        let source = V2.replace(
            "SIGNING_KEY_ID",
            &compute_key_id(key.verifying_key().as_bytes()),
        );
        assert!(
            RegistrationManifest::parse(source.as_bytes())
                .unwrap()
                .verify_signature(&[0; 63], &key.verifying_key())
                .is_err()
        );
    }
}
