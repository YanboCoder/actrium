//! Validated actor identity and storage namespace derivation.
//!
//! A package signature authenticates *provenance*, not *path safety* (CWE-22 /
//! CWE-73). [`ValidatedActorIdentity`] is the trust-boundary type: it wraps a
//! manifest identity that has passed the protocol naming rules
//! ([`actr_protocol::ActrType::validate`]), and derives the opaque
//! [`StorageNamespaceId`] used as the on-disk storage location.
//!
//! The raw `publisher`/`name`/`version` strings are **never** used as
//! filesystem path components; the only path component derived from the identity
//! is the lowercase-hex SHA-256 of a domain-separated, length-prefixed encoding.
//! This makes the storage path immune to `..` traversal, absolute/drive/UNC
//! prefixes, Windows reserved names, and case/Unicode-normalization folding.

use actr_protocol::ActrType;
use sha2::{Digest, Sha256};

use crate::manifest::PackageManifest;

/// Domain separator for the storage-namespace hash. The trailing `v1` is the
/// layout version: changing the encoding or layout must bump it so namespaces
/// computed under different rules never collide.
const NAMESPACE_DOMAIN: &[u8] = b"actr-storage-namespace\0v1";

/// An actor identity that has passed the protocol naming rules.
///
/// Construct via [`ValidatedActorIdentity::from_manifest`]. The inner strings
/// are guaranteed to satisfy [`actr_protocol::ActrType::validate`], so the same
/// identity is valid as an `ActrType`, a registry locator, and a storage
/// namespace source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidatedActorIdentity {
    publisher: String,
    name: String,
    version: String,
}

/// Opaque storage namespace identifier (a SHA-256 digest).
///
/// This is the only value derived from the identity that may appear in a
/// storage path or opaque provider key. Render with [`StorageNamespaceId::to_hex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StorageNamespaceId([u8; 32]);

impl StorageNamespaceId {
    /// Raw 32-byte digest.
    pub fn into_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Borrow the raw 32-byte digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex form (64 chars), safe as a single path component on any
    /// filesystem (ASCII, lowercase, so case-insensitive filesystems never fold
    /// two distinct namespaces).
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl ValidatedActorIdentity {
    /// Validate the identity carried by a package manifest.
    ///
    /// This is the trust-boundary check: call it on `VerifiedPackage.manifest`
    /// (or before a registry key lookup) before the identity is ever used. It
    /// delegates to [`actr_protocol::ActrType::validate`] so there is exactly one
    /// identity-validity rule across parsing, verification, and storage.
    pub fn from_manifest(manifest: &PackageManifest) -> Result<Self, actr_protocol::ActrIdError> {
        Self::from_actor_type(ActrType {
            publisher: manifest.publisher.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
        })
    }

    pub fn from_actor_type(actr_type: ActrType) -> Result<Self, actr_protocol::ActrIdError> {
        actr_type.validate()?;
        Ok(Self {
            publisher: actr_type.publisher,
            name: actr_type.name,
            version: actr_type.version,
        })
    }

    pub fn publisher(&self) -> &str {
        &self.publisher
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Derive the opaque storage namespace for this identity.
    ///
    /// Encoding is domain-separated and uses a per-field tag + length prefix so
    /// distinct field splits never collide (e.g. `("ab", "c")` vs `("a", "bc")`).
    /// The hash input is the validated *raw* UTF-8 — no lowercasing, no Unicode
    /// NFC/NFD normalization — so `Evil`/`evil` and NFC/NFD forms map to distinct
    /// namespaces.
    pub fn namespace_id(&self) -> StorageNamespaceId {
        let mut hasher = Sha256::new();
        hasher.update(NAMESPACE_DOMAIN);
        update_field(&mut hasher, 1, self.publisher.as_bytes());
        update_field(&mut hasher, 2, self.name.as_bytes());
        update_field(&mut hasher, 3, self.version.as_bytes());
        StorageNamespaceId(hasher.finalize().into())
    }
}

/// Write one length-prefixed, tagged field into the hasher.
fn update_field(hasher: &mut Sha256, tag: u8, bytes: &[u8]) {
    hasher.update([tag]);
    hasher.update((bytes.len() as u32).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BinaryEntry, ManifestMetadata, PackageManifest};

    fn manifest(publisher: &str, name: &str, version: &str) -> PackageManifest {
        PackageManifest {
            publisher: publisher.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            binary: BinaryEntry {
                path: "bin/actor.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                hash: String::new(),
                size: None,
                kind: None,
            },
            signature_algorithm: "ed25519".to_string(),
            signing_key_id: None,
            resources: Vec::new(),
            proto_files: Vec::new(),
            lock_file: None,
            metadata: ManifestMetadata::default(),
        }
    }

    #[test]
    fn from_manifest_accepts_valid_identity() {
        assert!(ValidatedActorIdentity::from_manifest(&manifest("acme", "echo", "1.0.0")).is_ok());
        assert!(ValidatedActorIdentity::from_manifest(&manifest("acme", "x..y", "1.0.0")).is_ok());
    }

    #[test]
    fn from_manifest_rejects_invalid_identity() {
        // traversal attempt in version
        assert!(
            ValidatedActorIdentity::from_manifest(&manifest("evilco", "Loader", "../../victim"))
                .is_err()
        );
        // digit-start publisher (Name rule)
        assert!(
            ValidatedActorIdentity::from_manifest(&manifest("1acme", "echo", "1.0.0")).is_err()
        );
        // separator in name
        assert!(ValidatedActorIdentity::from_manifest(&manifest("acme", "a/b", "1.0.0")).is_err());
        // empty
        assert!(ValidatedActorIdentity::from_manifest(&manifest("", "echo", "1.0.0")).is_err());
    }

    #[test]
    fn namespace_id_is_deterministic() {
        let a = ValidatedActorIdentity::from_manifest(&manifest("acme", "echo", "1.0.0")).unwrap();
        let b = ValidatedActorIdentity::from_manifest(&manifest("acme", "echo", "1.0.0")).unwrap();
        assert_eq!(a.namespace_id(), b.namespace_id());
    }

    #[test]
    fn namespace_id_avoids_concat_ambiguity() {
        // ("ab","c") must differ from ("a","bc") — proves length-prefixing works.
        let x = ValidatedActorIdentity::from_manifest(&manifest("ab", "c", "1")).unwrap();
        let y = ValidatedActorIdentity::from_manifest(&manifest("a", "bc", "1")).unwrap();
        assert_ne!(x.namespace_id(), y.namespace_id());
    }

    #[test]
    fn namespace_id_distinguishes_case_and_field() {
        let upper =
            ValidatedActorIdentity::from_manifest(&manifest("Evil", "echo", "1.0.0")).unwrap();
        let lower =
            ValidatedActorIdentity::from_manifest(&manifest("evil", "echo", "1.0.0")).unwrap();
        assert_ne!(upper.namespace_id(), lower.namespace_id());

        let diff_version =
            ValidatedActorIdentity::from_manifest(&manifest("evil", "echo", "1.0.1")).unwrap();
        assert_ne!(lower.namespace_id(), diff_version.namespace_id());
    }

    #[test]
    fn namespace_hex_is_64_lowercase_ascii() {
        let id = ValidatedActorIdentity::from_manifest(&manifest("acme", "echo", "1.0.0")).unwrap();
        let hex = id.namespace_id().to_hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn namespace_id_known_answer_vector() {
        // Pins the v1 namespace layout: if the domain separator, field tags, or
        // length-prefix encoding ever change, this known-answer vector changes
        // too — surfacing any accidental layout change that would orphan existing
        // actor storage. Computed independently as:
        //   SHA-256(b"actr-storage-namespace\0v1"
        //           || 0x01||be32(4)||"acme" || 0x02||be32(4)||"echo" || 0x03||be32(5)||"1.0.0")
        let id = ValidatedActorIdentity::from_manifest(&manifest("acme", "echo", "1.0.0")).unwrap();
        assert_eq!(
            id.namespace_id().to_hex(),
            "dd44a25e560add15370d99de7ef0a62313316e86773d7f6e3e899b12f16efdfd"
        );
    }
}
