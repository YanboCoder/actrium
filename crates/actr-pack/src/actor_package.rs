//! Versioned Actor Package (`actr-package/2`) support.
//!
//! This module implements the profile-neutral package shape documented in
//! `actr/protocols/actor-package.zh.md`.  The older flat binary manifest stays
//! available through [`crate::PackageManifest`] while existing native Actor
//! workloads migrate to the versioned shape.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::error::PackError;
use crate::util::{read_zip_entry_bounded, sha256_hex, sha256_zip_entry_bounded};
use crate::verify::{DEFAULT_MAX_VERIFIED_ENTRIES, DEFAULT_MAX_VERIFIED_ENTRY_BYTES};

pub const ACTOR_PACKAGE_FORMAT_V2: &str = "actr-package/2";
/// Superseded format identifier; rejected with a migration hint.
const ACTOR_PACKAGE_FORMAT_V1: &str = "actr-package/1";
pub const KEEL_GUEST_PROFILE_V1: &str = "keel.guest/1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorPackageManifest {
    pub format: String,
    pub actor: ActorDescriptor,
    pub workload: WorkloadDescriptor,
    pub host: HostRequirements,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, ActorDependency>,
    #[serde(default, skip_serializing_if = "PackageExtensions::is_empty")]
    pub extensions: PackageExtensions,
    pub signature: SignatureDescriptor,
    #[serde(default)]
    pub files: Vec<PackageFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorDescriptor {
    pub publisher: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadDescriptor {
    pub profile: String,
    pub entry: String,
    #[serde(
        default,
        rename = "rpc-contract",
        skip_serializing_if = "Option::is_none"
    )]
    pub rpc_contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keel: Option<KeelGuestDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeelGuestDescriptor {
    pub runtime: String,
    #[serde(rename = "guest-api")]
    pub guest_api: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequirements {
    pub profile: String,
    pub api: String,
    #[serde(default, rename = "required-capabilities")]
    pub required_capabilities: Vec<String>,
    #[serde(default, rename = "optional-capabilities")]
    pub optional_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorDependency {
    pub actor: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageExtensions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actrium: Option<ActriumExtension>,
}

impl PackageExtensions {
    fn is_empty(&self) -> bool {
        self.actrium.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActriumExtension {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(
        default,
        rename = "theme-color",
        skip_serializing_if = "Option::is_none"
    )]
    pub theme_color: Option<String>,
    #[serde(default, rename = "left-panel-default-visible")]
    pub left_panel_default_visible: bool,
    #[serde(default, rename = "right-panel-default-visible")]
    pub right_panel_default_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureDescriptor {
    pub algorithm: String,
    #[serde(rename = "key-id")]
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFileEntry {
    pub path: String,
    pub role: String,
    #[serde(rename = "media-type")]
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
}

/// Rejects `actr-package/1` manifests, which named the publisher field
/// differently, with an actionable error instead of a generic parse failure.
fn reject_legacy_manifest(source: &str) -> Result<(), PackError> {
    let Ok(value) = source.parse::<toml::Table>() else {
        return Ok(());
    };
    let legacy_key = value
        .get("actor")
        .and_then(toml::Value::as_table)
        .is_some_and(|actor| actor.contains_key(LEGACY_PUBLISHER_KEY));
    let legacy_format =
        value.get("format").and_then(toml::Value::as_str) == Some(ACTOR_PACKAGE_FORMAT_V1);
    if legacy_key || legacy_format {
        return Err(PackError::InvalidPackage(format!(
            "{ACTOR_PACKAGE_FORMAT_V1} manifests are no longer supported: set `format = \"{ACTOR_PACKAGE_FORMAT_V2}\"` and rename `actor.{LEGACY_PUBLISHER_KEY}` to `actor.publisher`"
        )));
    }
    Ok(())
}

const LEGACY_PUBLISHER_KEY: &str = "manufacturer";

impl ActorPackageManifest {
    pub fn from_toml(source: &str) -> Result<Self, PackError> {
        reject_legacy_manifest(source)?;
        let manifest: Self = toml::from_str(source)
            .map_err(|error| PackError::ManifestParseError(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_toml(&self) -> Result<String, PackError> {
        self.validate()?;
        toml::to_string_pretty(self)
            .map_err(|error| PackError::ManifestParseError(error.to_string()))
    }

    pub fn actor_type(&self) -> String {
        format!(
            "{}/{}@{}",
            self.actor.publisher, self.actor.name, self.actor.version
        )
    }

    pub fn workload_file(&self) -> Result<&PackageFileEntry, PackError> {
        self.files
            .iter()
            .find(|file| file.path == self.workload.entry)
            .ok_or_else(|| {
                PackError::InvalidPackage(format!(
                    "workload entry `{}` is not declared in files",
                    self.workload.entry
                ))
            })
    }

    /// Select the signed RPC Proto exported by a native Component.
    pub fn component_rpc_contract(&self) -> Result<Option<&str>, PackError> {
        let explicit = self.workload.rpc_contract.as_deref();
        if self.workload.profile != "actr.component/1" {
            return if explicit.is_some() {
                Err(PackError::InvalidPackage(
                    "workload.rpc-contract requires the actr.component/1 profile".into(),
                ))
            } else {
                Ok(None)
            };
        }
        let mut contracts = self.files.iter().filter_map(|file| {
            (file.role == "contract" && file.path.ends_with(".proto")).then_some(file.path.as_str())
        });
        if let Some(path) = explicit {
            return if contracts.any(|candidate| candidate == path) {
                Ok(Some(path))
            } else {
                Err(PackError::InvalidPackage(format!(
                    "workload.rpc-contract `{path}` must name a declared .proto with role `contract`"
                )))
            };
        }
        let mut local = contracts.filter(|path| {
            !self
                .dependencies
                .values()
                .any(|dependency| dependency.contract.as_deref() == Some(*path))
        });
        let contract = local.next();
        if local.next().is_some() {
            return Err(PackError::InvalidPackage(
                "multiple RPC Proto exports; set workload.rpc-contract to select one".into(),
            ));
        }
        Ok(contract)
    }

    /// Read a declared entry from the same archive bytes that were verified.
    pub fn read_declared_file(
        &self,
        bytes: &[u8],
        path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, PackError> {
        if !self.files.iter().any(|file| file.path == path) {
            return Err(PackError::InvalidPackage(format!(
                "file `{path}` is not declared by the verified manifest"
            )));
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        read_zip_entry_bounded(&mut archive, path, max_bytes)
    }

    pub fn validate(&self) -> Result<(), PackError> {
        if self.format != ACTOR_PACKAGE_FORMAT_V2 {
            return Err(PackError::InvalidPackage(format!(
                "unsupported package format `{}`",
                self.format
            )));
        }
        for (field, value) in [
            ("actor.publisher", self.actor.publisher.as_str()),
            ("actor.name", self.actor.name.as_str()),
            ("actor.version", self.actor.version.as_str()),
            ("workload.profile", self.workload.profile.as_str()),
            ("workload.entry", self.workload.entry.as_str()),
            ("host.profile", self.host.profile.as_str()),
            ("host.api", self.host.api.as_str()),
            ("signature.key-id", self.signature.key_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(PackError::InvalidPackage(format!(
                    "{field} must not be empty"
                )));
            }
        }
        if self.signature.algorithm != "ed25519" {
            return Err(PackError::InvalidPackage(
                "signature.algorithm must be `ed25519`".to_string(),
            ));
        }
        validate_package_path(&self.workload.entry)?;
        if self.workload.profile == KEEL_GUEST_PROFILE_V1 {
            let keel = self.workload.keel.as_ref().ok_or_else(|| {
                PackError::InvalidPackage("keel.guest/1 requires [workload.keel]".to_string())
            })?;
            if keel.guest_api != KEEL_GUEST_PROFILE_V1 {
                return Err(PackError::InvalidPackage(format!(
                    "workload.keel.guest-api must be `{KEEL_GUEST_PROFILE_V1}`"
                )));
            }
            if keel.runtime.trim().is_empty() {
                return Err(PackError::InvalidPackage(
                    "workload.keel.runtime must not be empty".to_string(),
                ));
            }
        } else if self.workload.keel.is_some() {
            return Err(PackError::InvalidPackage(
                "[workload.keel] requires the keel.guest/1 profile".to_string(),
            ));
        }

        if self.files.is_empty() {
            return Err(PackError::InvalidPackage(
                "files must declare at least the workload entry".to_string(),
            ));
        }
        if self.files.len() > DEFAULT_MAX_VERIFIED_ENTRIES {
            return Err(PackError::InvalidPackage(format!(
                "manifest declares {} files, exceeds limit {}",
                self.files.len(),
                DEFAULT_MAX_VERIFIED_ENTRIES
            )));
        }
        let mut file_roles = BTreeMap::new();
        for file in &self.files {
            validate_package_path(&file.path)?;
            if file_roles
                .insert(file.path.as_str(), file.role.as_str())
                .is_some()
            {
                return Err(PackError::InvalidPackage(format!(
                    "manifest declares duplicate file `{}`",
                    file.path
                )));
            }
            validate_sha256(&file.sha256)?;
        }
        let workload_file = self.workload_file()?;
        if workload_file.role != "workload" {
            return Err(PackError::InvalidPackage(format!(
                "workload entry `{}` must have role `workload`",
                workload_file.path
            )));
        }
        if self.workload.profile == "actr.component/1"
            && self
                .files
                .iter()
                .filter(|file| file.role == "workload")
                .count()
                != 1
        {
            return Err(PackError::InvalidPackage(
                "actr.component/1 requires exactly one file with role `workload`".to_string(),
            ));
        }
        let require_role = |path: Option<&str>, role: &str| {
            if let Some(path) = path
                && file_roles.get(path).copied() != Some(role)
            {
                return Err(PackError::InvalidPackage(format!(
                    "file `{path}` must be declared with role `{role}`"
                )));
            }
            Ok(())
        };
        if let Some(keel) = &self.workload.keel {
            require_role(keel.capabilities.as_deref(), "profile")?;
            require_role(keel.contract.as_deref(), "contract")?;
        }
        for dependency in self.dependencies.values() {
            require_role(dependency.contract.as_deref(), "contract")?;
        }
        self.component_rpc_contract()?;
        Ok(())
    }
}

fn validate_package_path(path: &str) -> Result<(), PackError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PackError::InvalidPackage(format!(
            "invalid package path `{path}`"
        )));
    }
    if matches!(path, "manifest.toml" | "manifest.sig") {
        return Err(PackError::InvalidPackage(format!(
            "reserved package path `{path}` cannot be a payload"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), PackError> {
    if value.len() != 64 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(PackError::InvalidPackage(
            "files[].sha256 must be a 64-character SHA-256 hex string".to_string(),
        ));
    }
    Ok(())
}

pub struct ActorPackOptions {
    pub manifest: ActorPackageManifest,
    pub payloads: BTreeMap<String, Vec<u8>>,
    pub signing_key: SigningKey,
}

pub fn pack_actor_package(options: &ActorPackOptions) -> Result<Vec<u8>, PackError> {
    let mut manifest = options.manifest.clone();
    let declared: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let supplied: BTreeSet<_> = options.payloads.keys().map(String::as_str).collect();
    if declared != supplied {
        return Err(PackError::InvalidPackage(
            "manifest files and supplied payload paths differ".to_string(),
        ));
    }
    for file in &mut manifest.files {
        let bytes = options
            .payloads
            .get(&file.path)
            .ok_or_else(|| PackError::InvalidPackage(format!("missing payload `{}`", file.path)))?;
        file.size = bytes.len() as u64;
        file.sha256 = sha256_hex(bytes);
    }
    manifest.signature.algorithm = "ed25519".to_string();
    manifest.signature.key_id =
        crate::compute_key_id(&options.signing_key.verifying_key().to_bytes());
    let manifest_toml = manifest.to_toml()?;
    let signature = options
        .signing_key
        .sign(manifest_toml.as_bytes())
        .to_bytes();

    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    archive.start_file("manifest.toml", stored)?;
    archive.write_all(manifest_toml.as_bytes())?;
    archive.start_file("manifest.sig", stored)?;
    archive.write_all(&signature)?;
    for file in &manifest.files {
        archive.start_file(&file.path, stored)?;
        archive.write_all(&options.payloads[&file.path])?;
    }
    Ok(archive.finish()?.into_inner())
}

#[derive(Debug, Clone)]
pub struct InspectedActorPackage {
    pub manifest: ActorPackageManifest,
    pub manifest_raw: Vec<u8>,
    pub signature_raw: Vec<u8>,
}

pub fn inspect_actor_package(bytes: &[u8]) -> Result<InspectedActorPackage, PackError> {
    inspect_actor_package_bounded(
        bytes,
        DEFAULT_MAX_VERIFIED_ENTRY_BYTES,
        DEFAULT_MAX_VERIFIED_ENTRY_BYTES,
    )
}

pub fn inspect_actor_package_bounded(
    bytes: &[u8],
    max_entry_bytes: usize,
    max_total_bytes: usize,
) -> Result<InspectedActorPackage, PackError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let manifest_raw = read_zip_entry_bounded(&mut archive, "manifest.toml", MAX_MANIFEST_BYTES)
        .map_err(required_manifest)?;
    let signature_raw = read_zip_entry_bounded(&mut archive, "manifest.sig", MAX_SIGNATURE_BYTES)
        .map_err(required_signature)?;
    if signature_raw.len() != 64 {
        return Err(PackError::SignatureVerificationFailed(format!(
            "manifest.sig must be exactly 64 bytes, got {}",
            signature_raw.len()
        )));
    }
    let manifest_source = std::str::from_utf8(&manifest_raw)
        .map_err(|error| PackError::ManifestParseError(error.to_string()))?;
    let manifest = ActorPackageManifest::from_toml(manifest_source)?;
    if archive.len() != manifest.files.len() + 2 {
        return Err(PackError::InvalidPackage(
            "ZIP payload entries do not exactly match manifest files".into(),
        ));
    }
    check_central_directory(bytes, archive.central_directory_start(), archive.len())?;

    let declared: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let mut archive_names = BTreeSet::new();
    let mut payload_names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        if entry.compression() != CompressionMethod::Stored {
            return Err(PackError::InvalidPackage(format!(
                "ZIP entry `{name}` must use STORE compression"
            )));
        }
        validate_archive_name(&name)?;
        if !archive_names.insert(name.clone()) {
            return Err(PackError::InvalidPackage(format!(
                "ZIP contains duplicate entry `{name}`"
            )));
        }
        if !matches!(name.as_str(), "manifest.toml" | "manifest.sig") {
            payload_names.insert(name);
        }
    }
    if declared != payload_names.iter().map(String::as_str).collect() {
        return Err(PackError::InvalidPackage(
            "ZIP payload entries do not exactly match manifest files".to_string(),
        ));
    }

    let mut consumed = 0usize;
    for file in &manifest.files {
        let remaining = max_total_bytes.saturating_sub(consumed);
        let allowed = max_entry_bytes.min(remaining);
        let (actual_hash, actual_size) =
            sha256_zip_entry_bounded(&mut archive, &file.path, allowed)?;
        if actual_size as u64 != file.size {
            return Err(PackError::InvalidPackage(format!(
                "file size mismatch for `{}`",
                file.path
            )));
        }
        if actual_hash != file.sha256.to_ascii_lowercase() {
            return Err(PackError::ResourceHashMismatch {
                path: file.path.clone(),
            });
        }
        consumed = consumed
            .checked_add(actual_size)
            .ok_or_else(|| PackError::InvalidPackage("payload size overflows usize".to_string()))?;
    }

    Ok(InspectedActorPackage {
        manifest,
        manifest_raw,
        signature_raw,
    })
}

// ZipArchive indexes entries by raw name. Count the records before trusting
// that index, since identical names would otherwise collapse into one entry.
fn check_central_directory(
    bytes: &[u8],
    offset: u64,
    unique_entries: usize,
) -> Result<(), PackError> {
    const SIGNATURE: &[u8] = b"PK\x01\x02";
    const HEADER_BYTES: usize = 46;
    let invalid_directory = || PackError::InvalidPackage("invalid ZIP central directory".into());
    let mut directory = usize::try_from(offset)
        .ok()
        .and_then(|offset| bytes.get(offset..))
        .ok_or_else(invalid_directory)?;
    for _ in 0..unique_entries {
        let header = directory
            .get(..HEADER_BYTES)
            .filter(|header| header.starts_with(SIGNATURE))
            .ok_or_else(invalid_directory)?;
        // The fixed header stores the lengths of the name, extra field and comment.
        let variable_bytes = [28, 30, 32]
            .into_iter()
            .map(|i| usize::from(u16::from_le_bytes([header[i], header[i + 1]])))
            .sum::<usize>();
        directory = directory
            .get(HEADER_BYTES + variable_bytes..)
            .ok_or_else(invalid_directory)?;
    }
    if directory.starts_with(SIGNATURE) {
        return Err(PackError::InvalidPackage(
            "ZIP contains duplicate central directory entries".into(),
        ));
    }
    Ok(())
}

pub fn verify_actor_package(
    bytes: &[u8],
    public_key: &VerifyingKey,
) -> Result<InspectedActorPackage, PackError> {
    verify_actor_package_bounded(bytes, public_key, DEFAULT_MAX_VERIFIED_ENTRY_BYTES)
}

pub(crate) fn verify_actor_package_bounded(
    bytes: &[u8],
    public_key: &VerifyingKey,
    max_payload_bytes: usize,
) -> Result<InspectedActorPackage, PackError> {
    let inspected = inspect_actor_package_bounded(bytes, max_payload_bytes, max_payload_bytes)?;
    let signature_bytes: [u8; 64] = inspected.signature_raw.clone().try_into().map_err(|_| {
        PackError::SignatureVerificationFailed("manifest.sig has invalid length".to_string())
    })?;
    public_key
        .verify_strict(
            &inspected.manifest_raw,
            &Signature::from_bytes(&signature_bytes),
        )
        .map_err(|error| {
            PackError::SignatureVerificationFailed(format!("Ed25519 verification failed: {error}"))
        })?;
    let actual_key_id = crate::compute_key_id(&public_key.to_bytes());
    if inspected.manifest.signature.key_id != actual_key_id {
        return Err(PackError::SignatureVerificationFailed(format!(
            "signature.key-id does not match verifying key: expected `{actual_key_id}`"
        )));
    }
    Ok(inspected)
}

pub fn read_actor_package_file(
    bytes: &[u8],
    inspected: &InspectedActorPackage,
    path: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, PackError> {
    inspected
        .manifest
        .read_declared_file(bytes, path, max_bytes)
}

pub fn read_actor_workload(
    bytes: &[u8],
    inspected: &InspectedActorPackage,
) -> Result<Vec<u8>, PackError> {
    read_actor_package_file(
        bytes,
        inspected,
        &inspected.manifest.workload.entry,
        DEFAULT_MAX_VERIFIED_ENTRY_BYTES,
    )
}

fn validate_archive_name(name: &str) -> Result<(), PackError> {
    if matches!(name, "manifest.toml" | "manifest.sig") {
        Ok(())
    } else {
        validate_package_path(name)
    }
}

fn required_manifest(error: PackError) -> PackError {
    match error {
        PackError::ZipError(zip::result::ZipError::FileNotFound) => PackError::ManifestNotFound,
        other => other,
    }
}

fn required_signature(error: PackError) -> PackError {
    match error {
        PackError::ZipError(zip::result::ZipError::FileNotFound) => PackError::SignatureNotFound,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn manifest() -> ActorPackageManifest {
        ActorPackageManifest {
            format: ACTOR_PACKAGE_FORMAT_V2.to_string(),
            actor: ActorDescriptor {
                publisher: "actrium".to_string(),
                name: "echo-ui".to_string(),
                version: "0.1.0".to_string(),
            },
            workload: WorkloadDescriptor {
                profile: KEEL_GUEST_PROFILE_V1.to_string(),
                entry: "workload/guest.js".to_string(),
                rpc_contract: None,
                keel: Some(KeelGuestDescriptor {
                    runtime: "js".to_string(),
                    guest_api: KEEL_GUEST_PROFILE_V1.to_string(),
                    capabilities: None,
                    contract: None,
                }),
            },
            host: HostRequirements {
                profile: "actr.host/1".to_string(),
                api: "^1.0.0".to_string(),
                required_capabilities: vec!["actor.call".to_string(), "ui.render".to_string()],
                optional_capabilities: Vec::new(),
            },
            dependencies: BTreeMap::new(),
            extensions: PackageExtensions {
                actrium: Some(ActriumExtension {
                    title: Some("Echo".to_string()),
                    ..ActriumExtension::default()
                }),
            },
            signature: SignatureDescriptor {
                algorithm: "ed25519".to_string(),
                key_id: "placeholder".to_string(),
            },
            files: vec![PackageFileEntry {
                path: "workload/guest.js".to_string(),
                role: "workload".to_string(),
                media_type: "text/javascript".to_string(),
                size: 0,
                sha256: "0".repeat(64),
            }],
        }
    }

    #[test]
    fn round_trip_keel_guest_package() {
        let key = SigningKey::generate(&mut OsRng);
        let options = ActorPackOptions {
            manifest: manifest(),
            payloads: BTreeMap::from([(
                "workload/guest.js".to_string(),
                b"globalThis.echo = true;".to_vec(),
            )]),
            signing_key: key.clone(),
        };
        let package = pack_actor_package(&options).unwrap();
        let verified = verify_actor_package(&package, &key.verifying_key()).unwrap();
        assert_eq!(verified.manifest.workload.profile, KEEL_GUEST_PROFILE_V1);
        assert_eq!(
            read_actor_workload(&package, &verified).unwrap(),
            b"globalThis.echo = true;"
        );
    }

    #[test]
    fn runtime_verification_preserves_signed_source_and_bounds_payloads() {
        let key = SigningKey::generate(&mut OsRng);
        let payload = b"globalThis.echo = true;";
        let mut manifest = manifest();
        manifest.signature.key_id = crate::compute_key_id(key.verifying_key().as_bytes());
        manifest.files[0].size = payload.len() as u64;
        manifest.files[0].sha256 = sha256_hex(payload);
        let source = format!(
            "# Keep this signed comment\n\n{}",
            manifest.to_toml().unwrap()
        );
        let signature = key.sign(source.as_bytes()).to_bytes();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (path, content) in [
            ("manifest.toml", source.as_bytes()),
            ("manifest.sig", signature.as_slice()),
            (manifest.workload.entry.as_str(), payload.as_slice()),
        ] {
            writer.start_file(path, options).unwrap();
            writer.write_all(content).unwrap();
        }
        let package = writer.finish().unwrap().into_inner();
        let verified =
            crate::verify_runtime_package_bounded(&package, &key.verifying_key(), payload.len())
                .unwrap();
        assert!(matches!(
            verified.manifest,
            crate::RuntimePackageManifest::V2(_)
        ));
        assert_eq!(verified.manifest_raw, source.as_bytes());
        assert_eq!(verified.sig_raw, signature);
        assert!(
            crate::verify_runtime_package_bounded(
                &package,
                &key.verifying_key(),
                payload.len() - 1
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_undeclared_payload() {
        let key = SigningKey::generate(&mut OsRng);
        let options = ActorPackOptions {
            manifest: manifest(),
            payloads: BTreeMap::from([("workload/guest.js".to_string(), b"guest".to_vec())]),
            signing_key: key,
        };
        let package = pack_actor_package(&options).unwrap();
        let archive = zip::ZipArchive::new(Cursor::new(package)).unwrap();
        assert_eq!(archive.len(), 3);
    }

    #[test]
    fn rejects_path_traversal() {
        let mut manifest = manifest();
        manifest.files[0].path = "../guest.js".to_string();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_legacy_publisher_key_with_rename_hint() {
        let source = manifest()
            .to_toml()
            .unwrap()
            .replace("publisher = ", "manufacturer = ");
        let error = ActorPackageManifest::from_toml(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains("actor.manufacturer"), "{error}");
        assert!(error.contains("actor.publisher"), "{error}");
    }

    #[test]
    fn rejects_superseded_format() {
        let source = manifest()
            .to_toml()
            .unwrap()
            .replace(ACTOR_PACKAGE_FORMAT_V2, ACTOR_PACKAGE_FORMAT_V1);
        let error = ActorPackageManifest::from_toml(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains(ACTOR_PACKAGE_FORMAT_V2), "{error}");
    }
}
