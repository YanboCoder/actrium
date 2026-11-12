//! Integration tests: .actr package sign → Hyper::verify_package full flow
//!
//! Scenarios:
//! 1. .actr package: pack → verify → manifest fields match
//! 2. .actr package: tampered binary → hash mismatch
//! 3. .actr package: wrong key → signature verification failed
//! 4. Unsigned bytes → InvalidManifest (unrecognized format)

#[cfg(all(feature = "wasm-engine", actr_wasm_fixture_available))]
use actr_hyper::BinaryKind;
use actr_hyper::test_support::inspect_workload_package;
use actr_hyper::{Hyper, HyperConfig, HyperError, StaticTrust, WorkloadPackage};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::sync::Arc;
use tempfile::TempDir;

// ─── Utility functions ───────────────────────────────────────────────────────

fn minimal_wasm() -> Vec<u8> {
    b"\0asm\x01\x00\x00\x00".to_vec()
}

#[cfg(all(feature = "wasm-engine", actr_wasm_fixture_available))]
mod wasm_actor_fixture;

/// Returns the Phase-1 Component Model fixture bytes embedded in
/// `wasm_actor_fixture.rs`. Previously this was a hand-rolled core wasm
/// module exposing `actr_alloc` / `actr_handle` — obsolete since
/// Commit 2 switched the host to `Component::from_binary`. The fresh
/// Component exposes the `actr:workload/workload@0.8.0` world and loads
/// cleanly through `WasmHost::compile`.
#[cfg(all(feature = "wasm-engine", actr_wasm_fixture_available))]
fn echo_guest_wasm() -> Vec<u8> {
    wasm_actor_fixture::WASM_ACTOR_FIXTURE.to_vec()
}

/// Build an .actr ZIP package.
///
/// Defaults to a legacy `wasm32-wasip1` target string so the simpler
/// signing/verification tests keep their pre-Component manifests. Callers
/// that want a Component-capable manifest (loading through the actual
/// wasm backend) go through [`build_actr_package_with_target`].
fn build_actr_package(
    binary: &[u8],
    publisher: &str,
    name: &str,
    version: &str,
    signing_key: &SigningKey,
) -> Vec<u8> {
    build_actr_package_with_target(
        binary,
        publisher,
        name,
        version,
        "wasm32-wasip1",
        signing_key,
    )
}

fn build_actr_package_with_target(
    binary: &[u8],
    publisher: &str,
    name: &str,
    version: &str,
    target: &str,
    signing_key: &SigningKey,
) -> Vec<u8> {
    // Tests that care about the kind opt in through the target triple:
    // `wasm32-wasip2` implies a Component binary, everything else leaves
    // the field unset so the resolver falls back to the legacy default.
    let kind = if target == "wasm32-wasip2" {
        Some(actr_pack::BinaryKind::Component)
    } else {
        None
    };
    let manifest = actr_pack::PackageManifest {
        publisher: publisher.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        binary: actr_pack::BinaryEntry {
            path: "bin/actor.wasm".to_string(),
            target: target.to_string(),
            hash: String::new(),
            size: None,
            kind,
        },
        signature_algorithm: "ed25519".to_string(),
        signing_key_id: None,
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        metadata: actr_pack::ManifestMetadata::default(),
    };
    let opts = actr_pack::PackOptions {
        manifest,
        binary_bytes: binary.to_vec(),
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        signing_key: signing_key.clone(),
    };
    actr_pack::pack(&opts).unwrap()
}

fn dev_config_with_key(dir: &TempDir, verifying_key: &ed25519_dalek::VerifyingKey) -> HyperConfig {
    HyperConfig::new(
        dir.path(),
        Arc::new(StaticTrust::new(verifying_key.to_bytes()).unwrap()),
    )
}

// ─── .actr package tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn actr_package_roundtrip() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let wasm = minimal_wasm();
    let package = build_actr_package(&wasm, "test-publisher", "MyActor", "1.2.3", &signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();

    let verified = hyper
        .verify_package(&WorkloadPackage::new(package))
        .await
        .unwrap();
    assert_eq!(verified.manifest.actor_type().publisher, "test-publisher");
    assert_eq!(verified.manifest.actor_type().name, "MyActor");
    assert_eq!(verified.manifest.actor_type().version, "1.2.3");
}

#[tokio::test]
async fn actr_package_tampered_binary() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let package = build_actr_package(b"original wasm", "publisher", "A", "1.0", &signing_key);

    // Tamper: find "original wasm" in the ZIP and modify it
    let mut tampered = package.clone();
    if let Some(pos) = tampered.windows(13).position(|w| w == b"original wasm") {
        tampered[pos] ^= 0xFF;
    }

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();
    let result = hyper.verify_package(&WorkloadPackage::new(tampered)).await;
    assert!(
        result.is_err(),
        "tampered .actr package should fail verification"
    );
}

#[tokio::test]
async fn actr_package_wrong_key() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let wrong_key = SigningKey::generate(&mut OsRng);
    let wrong_verifying = wrong_key.verifying_key();

    let package = build_actr_package(b"wasm", "publisher", "A", "1.0", &signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &wrong_verifying))
        .await
        .unwrap();
    let result = hyper.verify_package(&WorkloadPackage::new(package)).await;
    assert!(
        matches!(result, Err(HyperError::SignatureVerificationFailed(_))),
        "wrong key should return SignatureVerificationFailed, got: {result:?}"
    );
}

#[tokio::test]
async fn verify_rejects_unknown_format() {
    let dir = TempDir::new().unwrap();
    let signing_key = SigningKey::generate(&mut OsRng);
    let hyper = Hyper::new(dev_config_with_key(&dir, &signing_key.verifying_key()))
        .await
        .unwrap();

    let result = hyper
        .verify_package(&WorkloadPackage::new(b"this is not a binary".to_vec()))
        .await;
    assert!(matches!(result, Err(HyperError::InvalidManifest(_))));
}

#[cfg(all(feature = "wasm-engine", actr_wasm_fixture_available))]
#[tokio::test]
async fn load_workload_package_selects_wasm_backend() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    // The fixture is a real Component Model binary targeting
    // `wasm32-wasip2` — the target actr settled on in Phase 1.
    let package = build_actr_package_with_target(
        &echo_guest_wasm(),
        "test-publisher",
        "Echo",
        "1.0.0",
        "wasm32-wasip2",
        &signing_key,
    );

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();

    let loaded = inspect_workload_package(&hyper, &WorkloadPackage::new(package))
        .await
        .unwrap();

    assert_eq!(loaded.binary_kind, BinaryKind::Wasm);
    assert_eq!(
        loaded.manifest().as_legacy().unwrap().binary.target,
        "wasm32-wasip2"
    );
}

#[cfg(feature = "wasm-engine")]
#[tokio::test]
async fn load_workload_package_rejects_legacy_core_module() {
    // A pre-Phase-1 .actr package identifies itself by having a wasm
    // target (wasm32-*) and either no `binary.kind` field (legacy
    // default) or an explicit `core-module` marker. The loader should
    // refuse with a migration-pointing error long before wasmtime's
    // `Component::from_binary` gets its hands on the bytes.
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Build a legacy-style package: wasm target, no kind marker.
    let package = build_actr_package(
        &minimal_wasm(),
        "legacy-publisher",
        "LegacyEcho",
        "0.1.0",
        &signing_key,
    );

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();

    let result = inspect_workload_package(&hyper, &WorkloadPackage::new(package)).await;
    let err = result.expect_err("legacy core-module package must be refused");
    match err {
        HyperError::InvalidManifest(msg) => {
            assert!(
                msg.contains("legacy core wasm module format"),
                "error message must mention the format migration: {msg}"
            );
            assert!(
                msg.contains("wasm32-wasip2"),
                "error message must point at the new target: {msg}"
            );
        }
        other => panic!("expected InvalidManifest, got {other:?}"),
    }
}

#[tokio::test]
async fn load_workload_package_rejects_invalid_target() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let manifest = actr_pack::PackageManifest {
        publisher: "test-publisher".to_string(),
        name: "BrokenActor".to_string(),
        version: "1.0.0".to_string(),
        binary: actr_pack::BinaryEntry {
            path: "bin/actor.wasm".to_string(),
            target: "invalid-target".to_string(),
            hash: String::new(),
            size: None,
            kind: None,
        },
        signature_algorithm: "ed25519".to_string(),
        signing_key_id: None,
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        metadata: actr_pack::ManifestMetadata::default(),
    };
    let package = actr_pack::pack(&actr_pack::PackOptions {
        manifest,
        binary_bytes: minimal_wasm(),
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        signing_key: signing_key.clone(),
    })
    .unwrap();

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();

    let result = inspect_workload_package(&hyper, &WorkloadPackage::new(package)).await;
    assert!(
        matches!(result, Err(HyperError::InvalidManifest(ref msg)) if msg.contains("unsupported binary target")),
        "invalid target should be rejected, got: {result:?}"
    );
}
