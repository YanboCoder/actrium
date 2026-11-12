//! Versioned package manifests and full verification for native runtimes.

use actr_protocol::ActrType;
use ed25519_dalek::VerifyingKey;

use crate::{
    ACTOR_PACKAGE_FORMAT_V2, ActorPackageManifest, PackError, PackageManifest,
    ValidatedActorIdentity, VerifiedPackage, actor_package::verify_actor_package_bounded,
    read_manifest_raw, verify_bounded,
};

/// A v2 Actor Package or an unversioned package used by the original loader.
#[derive(Debug, Clone)]
pub enum RuntimePackageManifest {
    V2(ActorPackageManifest),
    Legacy(PackageManifest),
}

impl RuntimePackageManifest {
    pub fn as_legacy(&self) -> Option<&PackageManifest> {
        match self {
            Self::V2(_) => None,
            Self::Legacy(manifest) => Some(manifest),
        }
    }

    pub fn actor_type(&self) -> ActrType {
        let (publisher, name, version) = match self {
            Self::V2(manifest) => (
                &manifest.actor.publisher,
                &manifest.actor.name,
                &manifest.actor.version,
            ),
            Self::Legacy(manifest) => (&manifest.publisher, &manifest.name, &manifest.version),
        };
        ActrType {
            publisher: publisher.clone(),
            name: name.clone(),
            version: version.clone(),
        }
    }

    pub fn signing_key_id(&self) -> Option<&str> {
        match self {
            Self::V2(manifest) => Some(&manifest.signature.key_id),
            Self::Legacy(manifest) => manifest.signing_key_id.as_deref(),
        }
    }
}

pub type VerifiedRuntimePackage = VerifiedPackage<RuntimePackageManifest>;

impl From<VerifiedPackage> for VerifiedRuntimePackage {
    fn from(verified: VerifiedPackage) -> Self {
        Self {
            manifest: RuntimePackageManifest::Legacy(verified.manifest),
            manifest_raw: verified.manifest_raw,
            sig_raw: verified.sig_raw,
            signing_public_key: verified.signing_public_key,
        }
    }
}

fn is_v2_manifest(source: &str) -> Result<bool, PackError> {
    let table = toml::from_str::<toml::Table>(source)
        .map_err(|error| PackError::ManifestParseError(error.to_string()))?;
    match table.get("format") {
        Some(format) if format.as_str() == Some(ACTOR_PACKAGE_FORMAT_V2) => Ok(true),
        Some(format) => Err(PackError::InvalidPackage(format!(
            "unsupported package format {format}; expected `{ACTOR_PACKAGE_FORMAT_V2}`"
        ))),
        None if ["actor", "workload", "host", "signature", "files"]
            .iter()
            .any(|key| table.contains_key(*key)) =>
        {
            Err(PackError::InvalidPackage(format!(
                "versioned manifest requires format = \"{ACTOR_PACKAGE_FORMAT_V2}\""
            )))
        }
        None => Ok(false),
    }
}

/// Read a bounded, unverified manifest. An explicit format selects its parser
/// before any legacy fields can be interpreted.
pub fn read_runtime_manifest(bytes: &[u8]) -> Result<RuntimePackageManifest, PackError> {
    let source = read_manifest_raw(bytes)?;
    if is_v2_manifest(&source)? {
        ActorPackageManifest::from_toml(&source).map(RuntimePackageManifest::V2)
    } else {
        PackageManifest::from_toml(&source).map(RuntimePackageManifest::Legacy)
    }
}

/// Authenticate the exact manifest and verify all payloads within the runtime's
/// per-entry and aggregate limit.
pub fn verify_runtime_package_bounded(
    bytes: &[u8],
    key: &VerifyingKey,
    max_payload_bytes: usize,
) -> Result<VerifiedRuntimePackage, PackError> {
    if is_v2_manifest(&read_manifest_raw(bytes)?)? {
        let verified = verify_actor_package_bounded(bytes, key, max_payload_bytes)?;
        let manifest = RuntimePackageManifest::V2(verified.manifest);
        ValidatedActorIdentity::from_actor_type(manifest.actor_type())
            .map_err(|error| PackError::InvalidPackage(error.to_string()))?;
        Ok(VerifiedRuntimePackage {
            manifest,
            manifest_raw: verified.manifest_raw,
            sig_raw: verified.signature_raw,
            signing_public_key: key.to_bytes(),
        })
    } else {
        verify_bounded(bytes, key, max_payload_bytes).map(Into::into)
    }
}
