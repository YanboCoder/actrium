//! Validate native v2 package requirements before instantiating the workload.

use std::str::FromStr;

use actr_config::lock::LockFile;
use actr_pack::ActorPackageManifest;
use actr_protocol::ActrType;
use semver::{Version, VersionReq};

use crate::error::{HyperError, HyperResult};

pub(crate) fn validate_component_host(manifest: &ActorPackageManifest) -> HyperResult<()> {
    if manifest.workload.profile != "actr.component/1" || manifest.host.profile != "actr.host/1" {
        return Err(HyperError::InvalidManifest(format!(
            "unsupported native workload/host profile combination: {} / {}",
            manifest.workload.profile, manifest.host.profile
        )));
    }
    let host_api = Version::new(1, 0, 0);
    let requirement = VersionReq::parse(&manifest.host.api)
        .map_err(|e| HyperError::InvalidManifest(format!("invalid host.api: {e}")))?;
    if !requirement.matches(&host_api) {
        return Err(HyperError::InvalidManifest(format!(
            "native host API {host_api} does not satisfy {}",
            manifest.host.api
        )));
    }
    if !manifest.host.required_capabilities.is_empty() {
        return Err(HyperError::InvalidManifest(format!(
            "native Component packages cannot require capabilities: {}",
            manifest.host.required_capabilities.join(", ")
        )));
    }
    Ok(())
}

pub(crate) fn parse_lock(bytes: &[u8]) -> HyperResult<LockFile> {
    let raw = std::str::from_utf8(bytes).map_err(|e| {
        HyperError::InvalidManifest(format!("manifest.lock.toml is not valid UTF-8: {e}"))
    })?;
    LockFile::from_str(raw).map_err(|e| {
        HyperError::InvalidManifest(format!("failed to parse manifest.lock.toml: {e}"))
    })
}

pub(crate) fn read_v2_lock(
    bytes: &[u8],
    manifest: &ActorPackageManifest,
    max_bytes: usize,
) -> HyperResult<Option<LockFile>> {
    const LOCK_PATH: &str = "manifest.lock.toml";
    let mut lock = None;
    for file in &manifest.files {
        if file.path == LOCK_PATH && file.role == "lock" {
            let raw = manifest
                .read_declared_file(bytes, LOCK_PATH, max_bytes)
                .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
            lock = Some(parse_lock(&raw)?);
        } else if file.path == LOCK_PATH || file.role == "lock" {
            return Err(HyperError::InvalidManifest(
                "v2 dependency lock must be manifest.lock.toml with role `lock`".into(),
            ));
        }
    }
    if let Some(lock) = &lock {
        lock.validate_aliases()
            .map_err(|e| HyperError::InvalidManifest(format!("invalid packaged lock: {e}")))?;
        for entry in &lock.dependencies {
            let declared = manifest.dependencies.get(&entry.alias).ok_or_else(|| {
                HyperError::InvalidManifest(format!(
                    "undeclared dependency alias `{}` in lock",
                    entry.alias
                ))
            })?;
            let actor_type = ActrType::from_string_repr(&entry.actr_type)
                .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
            let version = Version::parse(&actor_type.version)
                .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
            let constraint = VersionReq::parse(&declared.version).map_err(|e| {
                HyperError::InvalidManifest(format!(
                    "invalid dependency `{}` version: {e}",
                    entry.alias
                ))
            })?;
            if declared.actor != format!("{}/{}", actor_type.publisher, actor_type.name)
                || !constraint.matches(&version)
            {
                return Err(HyperError::InvalidManifest(format!(
                    "locked actor `{}` does not satisfy dependency `{}`",
                    entry.actr_type, entry.alias
                )));
            }
            if entry.fingerprint.trim().is_empty() {
                return Err(HyperError::InvalidManifest(format!(
                    "dependency `{}` is missing its service fingerprint",
                    entry.alias
                )));
            }
            if let Some(path) = &declared.contract {
                let [locked_proto] = entry.files.as_slice() else {
                    return Err(HyperError::InvalidManifest(format!(
                        "dependency `{}` must lock one self-contained RPC Proto",
                        entry.alias
                    )));
                };
                let content = manifest
                    .read_declared_file(bytes, path, max_bytes)
                    .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
                let content = String::from_utf8(content).map_err(|e| {
                    HyperError::InvalidManifest(format!("read contract `{path}`: {e}"))
                })?;
                let spec = actr_service_compat::build_self_contained_service_spec(
                    &entry.name,
                    actr_service_compat::ProtoFile {
                        name: locked_proto
                            .path
                            .rsplit('/')
                            .next()
                            .unwrap_or(&locked_proto.path)
                            .to_owned(),
                        content,
                        path: None,
                    },
                )
                .map_err(|e| {
                    HyperError::InvalidManifest(format!(
                        "dependency `{}` contract: {e}",
                        entry.alias
                    ))
                })?;
                if spec.fingerprint != entry.fingerprint
                    || spec.protobufs[0].fingerprint != locked_proto.fingerprint
                {
                    return Err(HyperError::InvalidManifest(format!(
                        "dependency `{}` contract does not match its signed lock fingerprints",
                        entry.alias
                    )));
                }
            }
        }
    }
    for (alias, dependency) in &manifest.dependencies {
        if dependency.required
            && !lock
                .as_ref()
                .is_some_and(|lock| lock.dependencies.iter().any(|entry| &entry.alias == alias))
        {
            return Err(HyperError::InvalidManifest(format!(
                "required dependency `{alias}` is missing from the signed package lock"
            )));
        }
    }
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actr_pack::{ActorPackOptions, PackageFileEntry, SignatureDescriptor, WorkloadDescriptor};
    use actr_service_compat::{ProtoFile, build_self_contained_service_spec};
    use std::collections::BTreeMap;

    const CONTRACT: &str = r#"syntax = "proto3";
package echo;
message Request { string message = 1; }
service Echo { rpc Echo(Request) returns (Request); }
"#;

    #[test]
    fn issuer_fixture_matches_the_supported_native_profile() {
        let source = include_str!("../../actr-pack/tests/fixtures/registration-v2.toml");
        let manifest = ActorPackageManifest::from_toml(source).unwrap();
        let registration = actr_pack::RegistrationManifest::parse(source.as_bytes()).unwrap();
        validate_component_host(&manifest).unwrap();
        assert_eq!(registration.target(), None);
        assert_eq!(
            registration.actor_type().to_program_identity(),
            manifest.actor_type()
        );

        let mut unsupported = manifest.clone();
        unsupported
            .host
            .required_capabilities
            .push("unsupported.permission".into());
        assert!(validate_component_host(&unsupported).is_err());
        unsupported = manifest.clone();
        unsupported.host.api = ">=2.0.0".into();
        assert!(validate_component_host(&unsupported).is_err());
        unsupported = manifest;
        unsupported.workload.profile = "keel.guest/1".into();
        assert!(validate_component_host(&unsupported).is_err());
    }

    fn fixture_lock() -> LockFile {
        let spec = build_self_contained_service_spec(
            "Echo",
            ProtoFile {
                name: "echo.proto".into(),
                content: CONTRACT.into(),
                path: None,
            },
        )
        .unwrap();
        LockFile::from_str(&format!(
            r#"
[metadata]
version = 2
generated_at = "2026-11-12T00:00:00Z"
[[dependency]]
name = "Echo"
alias = "echo"
actr_type = "example.com/Echo@1.0.0"
fingerprint = "{}"
cached_at = "2026-11-12T00:00:00Z"
files = [{{ path = "echo/echo.proto", fingerprint = "{}" }}]
"#,
            spec.fingerprint, spec.protobufs[0].fingerprint
        ))
        .unwrap()
    }

    fn signed_fixture(
        lock: LockFile,
        rpc_contract: Option<&str>,
    ) -> (Vec<u8>, ActorPackageManifest) {
        let payloads = BTreeMap::from([
            ("workload/main.wasm".to_owned(), Vec::new()),
            (
                "contracts/dependency.proto".to_owned(),
                CONTRACT.as_bytes().to_vec(),
            ),
            (
                "manifest.lock.toml".to_owned(),
                toml::to_string(&lock).unwrap().into_bytes(),
            ),
        ]);
        let manifest = ActorPackageManifest {
            format: actr_pack::ACTOR_PACKAGE_FORMAT_V2.into(),
            actor: actr_pack::ActorDescriptor {
                publisher: "example.com".into(),
                name: "Caller".into(),
                version: "1.0.0".into(),
            },
            workload: WorkloadDescriptor {
                profile: "actr.component/1".into(),
                entry: "workload/main.wasm".into(),
                rpc_contract: rpc_contract.map(str::to_owned),
                keel: None,
            },
            host: actr_pack::HostRequirements {
                profile: "actr.host/1".into(),
                api: "^1.0.0".into(),
                required_capabilities: vec![],
                optional_capabilities: vec![],
            },
            dependencies: BTreeMap::from([(
                "echo".into(),
                actr_pack::ActorDependency {
                    actor: "example.com/Echo".into(),
                    version: "^1.0.0".into(),
                    contract: Some("contracts/dependency.proto".into()),
                    required: true,
                },
            )]),
            extensions: Default::default(),
            signature: SignatureDescriptor {
                algorithm: "ed25519".into(),
                key_id: "test".into(),
            },
            files: [
                ("workload/main.wasm", "workload"),
                ("contracts/dependency.proto", "contract"),
                ("manifest.lock.toml", "lock"),
            ]
            .into_iter()
            .map(|(path, role)| PackageFileEntry {
                path: path.into(),
                role: role.into(),
                media_type: "application/octet-stream".into(),
                size: 0,
                sha256: "0".repeat(64),
            })
            .collect(),
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let bytes = actr_pack::pack_actor_package(&ActorPackOptions {
            manifest,
            payloads,
            signing_key: key.clone(),
        })
        .unwrap();
        let verified = actr_pack::verify_actor_package(&bytes, &key.verifying_key()).unwrap();
        (bytes, verified.manifest)
    }

    fn verify_lock(lock: LockFile) -> HyperResult<Option<LockFile>> {
        let (bytes, manifest) = signed_fixture(lock, None);
        read_v2_lock(
            &bytes,
            &manifest,
            actr_pack::DEFAULT_MAX_VERIFIED_ENTRY_BYTES,
        )
    }

    #[test]
    fn binds_dependency_contract_to_locked_fingerprints() {
        // The package may rename the contract while retaining its logical lock filename.
        assert!(verify_lock(fixture_lock()).unwrap().is_some());
        for fingerprint in ["", "service_semantic:stale"] {
            let mut lock = fixture_lock();
            lock.dependencies[0].fingerprint = fingerprint.into();
            assert!(verify_lock(lock).is_err());
        }
        let mut lock = fixture_lock();
        lock.dependencies[0].files[0].fingerprint = "semantic:stale".into();
        assert!(verify_lock(lock).is_err());
    }

    #[test]
    fn exports_an_explicit_rpc_contract_shared_with_a_dependency() {
        use crate::service_spec::calculate_v2_service_spec;

        let limit = actr_pack::DEFAULT_MAX_VERIFIED_ENTRY_BYTES;
        let (bytes, manifest) = signed_fixture(fixture_lock(), Some("contracts/dependency.proto"));
        assert!(read_v2_lock(&bytes, &manifest, limit).unwrap().is_some());
        let spec = calculate_v2_service_spec(&bytes, &manifest, limit)
            .unwrap()
            .unwrap();
        assert_eq!(spec.name, "Caller");
        assert_eq!(spec.protobufs[0].content, CONTRACT);

        let (bytes, manifest) = signed_fixture(fixture_lock(), None);
        assert!(
            calculate_v2_service_spec(&bytes, &manifest, limit)
                .unwrap()
                .is_none()
        );
    }
}
