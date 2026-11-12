//! actr-pack -- .actr package format
//!
//! Provides reading, writing, signing and verification of .actr ZIP STORE packages.
//!
//! ## Package structure
//!
//! ```text
//! {publisher}-{name}-{version}-{target}.actr
//! +-- manifest.toml       # manifest (TOML, signed payload)
//! +-- manifest.sig        # Ed25519 signature (64 bytes raw)
//! +-- manifest.lock.toml  # dependency lock (optional)
//! +-- bin/actor.wasm      # binary (STORE mode, uncompressed)
//! +-- proto/*.proto       # exported proto files (optional)
//! ```
//!
//! ## Signing chain
//!
//! ```text
//! binary bytes -> SHA-256 -> manifest.toml[binary.hash]
//!                                    |
//!                          manifest.toml bytes -> Ed25519 sign -> manifest.sig
//! ```

pub mod actor_package;
pub mod error;
pub mod identity;
pub mod load;
pub mod manifest;
pub mod pack;
pub mod registration;
pub mod verify;

mod runtime;
mod util;

pub use actor_package::{
    ACTOR_PACKAGE_FORMAT_V2, ActorDependency, ActorDescriptor, ActorPackOptions,
    ActorPackageManifest, ActriumExtension, HostRequirements, InspectedActorPackage,
    KEEL_GUEST_PROFILE_V1, KeelGuestDescriptor, PackageExtensions, PackageFileEntry,
    SignatureDescriptor, WorkloadDescriptor, inspect_actor_package, inspect_actor_package_bounded,
    pack_actor_package, read_actor_package_file, read_actor_workload, verify_actor_package,
};
pub use error::PackError;
pub use identity::{StorageNamespaceId, ValidatedActorIdentity};
pub use load::{
    load_binary, load_binary_bounded, read_glue_js, read_glue_js_bounded, read_lock_file,
    read_lock_file_bounded, read_manifest, read_manifest_raw, read_proto_files,
    read_proto_files_bounded, read_signature,
};
pub use manifest::{
    BinaryEntry, BinaryKind, LockFileEntry, ManifestMetadata, PackageManifest, ProtoFileEntry,
    ResourceEntry,
};
pub use pack::{PackOptions, pack};
pub use registration::{MAX_REGISTRATION_MANIFEST_BYTES, RegistrationManifest};
pub use verify::{
    DEFAULT_MAX_VERIFIED_ENTRIES, DEFAULT_MAX_VERIFIED_ENTRY_BYTES, PackageVerificationLimits,
    VerifiedPackage, verify, verify_bounded, verify_with_limits,
};

pub use runtime::{
    RuntimePackageManifest, VerifiedRuntimePackage, read_runtime_manifest,
    verify_runtime_package_bounded,
};

/// Compute deterministic key_id from Ed25519 public key bytes.
///
/// Algorithm: `"actr-pub-" + hex(sha256(public_key_bytes))[..16]`
///
/// This MUST match the server-side implementation in `loran-publisher-registry::crypto::compute_key_id`.
pub fn compute_key_id(public_key_bytes: &[u8]) -> String {
    let hex_str = util::sha256_hex(public_key_bytes);
    format!("actr-pub-{}", &hex_str[..16])
}
