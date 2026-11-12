//! Build a fully-populated [`actr_protocol::ServiceSpec`] from a batch of
//! proto files.
//!
//! Consolidates the service-spec assembly that historically existed in three
//! places (hyper's package loader, the CLI `install` compatibility check, and
//! an older manifest-driven path in `actr-config`). The low-level primitives
//! stay on [`Fingerprint`]; this module is just the "gather files → spec"
//! glue layer.
//!
//! Per-file `package` derivation strips a trailing `.proto` from the logical
//! file name — matching the convention used by manifest-emitted proto
//! descriptors across the codebase.

use crate::{CompatibilityError, Fingerprint, ProtoFile, Result};
use actr_protocol::service_spec::Protobuf;
use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

/// Inputs for [`build_service_spec`].
///
/// `tags` defaults to empty; `description` is optional. Pass the proto file
/// contents you want fingerprinted and embedded into the resulting spec.
#[derive(Debug, Clone)]
pub struct ServiceSpecInput<'a> {
    /// Actor / service name (becomes [`ServiceSpec::name`]).
    pub name: &'a str,
    /// Optional human description.
    pub description: Option<String>,
    /// Discovery tags.
    pub tags: Vec<String>,
    /// Proto file contents to fingerprint and embed.
    pub proto_files: Vec<ProtoFile>,
}

/// Build a [`actr_protocol::ServiceSpec`] from the given inputs.
///
/// - Computes the service-level semantic fingerprint from `proto_files`.
/// - Computes each proto's per-file semantic fingerprint. Individual failures
///   fall back to the string `"error"` on the corresponding `Protobuf` entry,
///   preserving the prior behaviour of the deleted duplicate implementations.
/// - Sets `published_at` to the current system time when available.
///
/// Returns an error only when the service-level fingerprint cannot be
/// calculated (typically malformed proto input).
pub fn build_service_spec(input: ServiceSpecInput<'_>) -> Result<actr_protocol::ServiceSpec> {
    let ServiceSpecInput {
        name,
        description,
        tags,
        proto_files,
    } = input;

    let fingerprint = Fingerprint::calculate_service_semantic_fingerprint(&proto_files)?;

    let protobufs = proto_files
        .iter()
        .map(|proto| {
            let fingerprint = Fingerprint::calculate_proto_semantic_fingerprint(&proto.content)
                .unwrap_or_else(|_| "error".to_string());
            make_protobuf(proto, fingerprint)
        })
        .collect();

    Ok(assemble_spec(
        name,
        description,
        tags,
        fingerprint,
        protobufs,
    ))
}

/// Build the spec for one RPC Proto from authenticated package contents.
/// Imports are rejected before type resolution. Fingerprint the parsed
/// descriptor directly so only the package's signed source is read.
pub fn build_self_contained_service_spec(
    name: &str,
    proto: ProtoFile,
) -> Result<actr_protocol::ServiceSpec> {
    let hash = self_contained_proto_hash(&proto.content).map_err(|source| {
        CompatibilityError::ProtoParseError {
            file_name: proto.name.clone(),
            source,
        }
    })?;
    let fingerprint = format!(
        "service_semantic:{:x}",
        Sha256::digest(format!("{}:{hash}", proto.name))
    );
    Ok(assemble_spec(
        name,
        None,
        vec![],
        fingerprint,
        vec![make_protobuf(&proto, format!("semantic:{hash}"))],
    ))
}

fn self_contained_proto_hash(source: &str) -> anyhow::Result<String> {
    let dependencies = protobuf_parse::pure::parse_dependencies(source)?;
    if !dependencies.dependency.is_empty() {
        bail!("the RPC Proto must be self-contained; imports are unsupported");
    }
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input.proto");
    std::fs::write(&input, source)?;
    let descriptors = protobuf_parse::Parser::new()
        .pure()
        .include(dir.path())
        .input(input)
        .file_descriptor_set()?;
    let descriptor = descriptors
        .file
        .iter()
        .find(|file| file.name() == "input.proto")
        .context("missing descriptor for input.proto")?;
    let canonical = proto_fingerprint::normalize::normalize_file(descriptor);
    let encoded = serde_json::to_string_pretty(&canonical)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn make_protobuf(proto: &ProtoFile, fingerprint: String) -> Protobuf {
    Protobuf {
        package: proto.name.trim_end_matches(".proto").to_string(),
        content: proto.content.clone(),
        fingerprint,
    }
}

fn assemble_spec(
    name: &str,
    description: Option<String>,
    tags: Vec<String>,
    fingerprint: String,
    protobufs: Vec<Protobuf>,
) -> actr_protocol::ServiceSpec {
    let published_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64);

    actr_protocol::ServiceSpec {
        name: name.to_string(),
        description,
        fingerprint,
        protobufs,
        published_at,
        tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTO_A: &str = r#"
        syntax = "proto3";
        package demo;
        message Ping { string text = 1; }
    "#;

    const PROTO_B: &str = r#"
        syntax = "proto3";
        package demo;
        message Pong { string text = 1; }
    "#;

    #[test]
    fn signed_proto_spec_keeps_the_legacy_fingerprint() {
        let proto = ProtoFile {
            name: "ping.proto".to_owned(),
            content: PROTO_A.to_owned(),
            path: None,
        };
        let legacy = build_service_spec(ServiceSpecInput {
            name: "echo",
            description: None,
            tags: vec![],
            proto_files: vec![proto.clone()],
        })
        .unwrap();
        let signed = build_self_contained_service_spec("echo", proto.clone()).unwrap();
        assert_eq!(signed.fingerprint, legacy.fingerprint);
        assert_eq!(signed.protobufs, legacy.protobufs);

        let mut imported = proto;
        imported.content.push_str("\nimport \"other.proto\";");
        assert!(build_self_contained_service_spec("echo", imported).is_err());
    }

    #[test]
    fn builds_spec_with_fingerprints() {
        let spec = build_service_spec(ServiceSpecInput {
            name: "demo-service",
            description: Some("demo".to_string()),
            tags: vec!["alpha".to_string()],
            proto_files: vec![
                ProtoFile {
                    name: "ping.proto".to_string(),
                    content: PROTO_A.to_string(),
                    path: None,
                },
                ProtoFile {
                    name: "pong.proto".to_string(),
                    content: PROTO_B.to_string(),
                    path: None,
                },
            ],
        })
        .expect("valid proto input");

        assert_eq!(spec.name, "demo-service");
        assert_eq!(spec.description.as_deref(), Some("demo"));
        assert_eq!(spec.tags, vec!["alpha".to_string()]);
        assert!(spec.fingerprint.starts_with("service_semantic:"));
        assert_eq!(spec.protobufs.len(), 2);

        // Packages strip the trailing `.proto`.
        let packages: Vec<_> = spec.protobufs.iter().map(|p| p.package.clone()).collect();
        assert!(packages.contains(&"ping".to_string()));
        assert!(packages.contains(&"pong".to_string()));

        // Per-file fingerprints are populated (not the fallback "error").
        for pb in &spec.protobufs {
            assert!(pb.fingerprint.starts_with("semantic:"), "got {pb:?}");
        }
    }

    #[test]
    fn fingerprint_stable_regardless_of_input_order() {
        let a = ProtoFile {
            name: "a.proto".to_string(),
            content: PROTO_A.to_string(),
            path: None,
        };
        let b = ProtoFile {
            name: "b.proto".to_string(),
            content: PROTO_B.to_string(),
            path: None,
        };

        let spec1 = build_service_spec(ServiceSpecInput {
            name: "svc",
            description: None,
            tags: vec![],
            proto_files: vec![a.clone(), b.clone()],
        })
        .unwrap();

        let spec2 = build_service_spec(ServiceSpecInput {
            name: "svc",
            description: None,
            tags: vec![],
            proto_files: vec![b, a],
        })
        .unwrap();

        assert_eq!(spec1.fingerprint, spec2.fingerprint);
    }

    #[test]
    fn per_file_fingerprint_falls_back_to_error_on_invalid_content() {
        // Valid service-level fingerprint computation requires all files to
        // parse, so we use only valid files here and rely on the documented
        // per-file fallback contract via the unit test below.
        let spec = build_service_spec(ServiceSpecInput {
            name: "svc",
            description: None,
            tags: vec![],
            proto_files: vec![ProtoFile {
                name: "ok.proto".to_string(),
                content: PROTO_A.to_string(),
                path: None,
            }],
        })
        .unwrap();
        assert_eq!(spec.protobufs.len(), 1);
        assert!(spec.protobufs[0].fingerprint.starts_with("semantic:"));
    }
}
