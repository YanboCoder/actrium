#![cfg(feature = "dynclib-engine")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use actr_hyper::test_support::inspect_workload_package;
use actr_hyper::{BinaryKind, Hyper, HyperConfig, StaticTrust, WorkloadPackage};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::sync::Arc;
use tempfile::TempDir;

fn fixture_so_path() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("tests/dynclib_actor_fixture");

    let status = Command::new("cargo")
        .args(["build"])
        .current_dir(&fixture_dir)
        .status()
        .expect("failed to build dynclib fixture");
    assert!(status.success(), "dynclib fixture build failed");

    let target_dir = manifest_dir.join("../../target/core-hyper-tests-dynclib-actor-fixture/debug");
    if cfg!(target_os = "linux") {
        target_dir.join("libdynclib_actor_fixture.so")
    } else if cfg!(target_os = "macos") {
        target_dir.join("libdynclib_actor_fixture.dylib")
    } else {
        target_dir.join("dynclib_actor_fixture.dll")
    }
}

fn current_native_target() -> String {
    format!(
        "{}-unknown-{}",
        std::env::consts::ARCH,
        if std::env::consts::OS == "macos" {
            "darwin"
        } else {
            std::env::consts::OS
        }
    )
}

fn dynclib_suffix() -> &'static str {
    if cfg!(target_os = "linux") {
        ".so"
    } else if cfg!(target_os = "macos") {
        ".dylib"
    } else if cfg!(target_os = "windows") {
        ".dll"
    } else {
        ".dynlib"
    }
}

fn build_dynclib_package(binary: &[u8], signing_key: &SigningKey) -> Vec<u8> {
    let manifest = actr_pack::PackageManifest {
        publisher: "test-publisher".to_string(),
        name: "DynActor".to_string(),
        version: "1.0.0".to_string(),
        binary: actr_pack::BinaryEntry {
            path: format!("bin/actor{}", dynclib_suffix()),
            target: current_native_target(),
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

    actr_pack::pack(&actr_pack::PackOptions {
        manifest,
        binary_bytes: binary.to_vec(),
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        signing_key: signing_key.clone(),
    })
    .unwrap()
}

fn dev_config_with_key(dir: &TempDir, verifying_key: &ed25519_dalek::VerifyingKey) -> HyperConfig {
    HyperConfig::new(
        dir.path(),
        Arc::new(StaticTrust::new(verifying_key.to_bytes()).unwrap()),
    )
}

fn cache_path(data_dir: &Path, binary_hash: &[u8; 32]) -> PathBuf {
    data_dir
        .join("dynclib-cache")
        .join(format!("{}{}", hex::encode(binary_hash), dynclib_suffix()))
}

#[tokio::test]
async fn dynclib_cache_is_created_on_first_load() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let dylib_bytes = fs::read(fixture_so_path()).unwrap();
    let package = WorkloadPackage::new(build_dynclib_package(&dylib_bytes, &signing_key));

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();

    let first = inspect_workload_package(&hyper, &package).await.unwrap();
    assert_eq!(first.binary_kind, BinaryKind::DynClib);
    let binary_hash = first
        .manifest()
        .as_legacy()
        .unwrap()
        .binary
        .hash_bytes()
        .unwrap();
    let cache_file = cache_path(dir.path(), &binary_hash);
    assert_eq!(fs::read(&cache_file).unwrap(), dylib_bytes);
}

#[tokio::test]
async fn dynclib_cache_rebuilds_after_corruption() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let dylib_bytes = fs::read(fixture_so_path()).unwrap();
    let package = WorkloadPackage::new(build_dynclib_package(&dylib_bytes, &signing_key));

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(dev_config_with_key(&dir, &verifying_key))
        .await
        .unwrap();
    let verified = hyper.verify_package(&package).await.unwrap();
    let binary_hash = verified
        .manifest
        .as_legacy()
        .unwrap()
        .binary
        .hash_bytes()
        .unwrap();
    let cache_file = cache_path(dir.path(), &binary_hash);
    fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
    fs::write(&cache_file, b"corrupted dynclib bytes").unwrap();

    let loaded = inspect_workload_package(&hyper, &package).await.unwrap();
    assert_eq!(loaded.binary_kind, BinaryKind::DynClib);
    assert_eq!(fs::read(&cache_file).unwrap(), dylib_bytes);
}
