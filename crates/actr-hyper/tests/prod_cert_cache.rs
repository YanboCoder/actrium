//! Integration tests: production mode Publisher Registry certificate cache + verify_package full flow
//!
//! Covered scenarios:
//! 1. Cache miss -> HTTP fetch Publisher Registry public key -> verification passes
//! 2. Cache hit -> no HTTP triggered -> verification passes
//! 3. Publisher Registry not registered -> Issuer returns 404 -> UntrustedPublisher
//! 4. Different publishers -> independent caches
//! 5. HTTP request body and response format validation

use actr_hyper::{
    Hyper, HyperConfig, HyperError, PublisherCertCache, RegistryTrust, WorkloadPackage,
};
use base64::Engine;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::sync::Arc;
use tempfile::TempDir;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn minimal_wasm() -> Vec<u8> {
    b"\0asm\x01\x00\x00\x00".to_vec()
}

/// Production mode HyperConfig pointing to a mock Issuer endpoint
fn prod_config(dir: &TempDir, issuer_endpoint: &str) -> HyperConfig {
    HyperConfig::new(dir.path(), Arc::new(RegistryTrust::new(issuer_endpoint)))
}

/// Build a signed .actr package for the given publisher
fn make_signed_package(
    publisher: &str,
    actr_name: &str,
    version: &str,
    signing_key: &SigningKey,
) -> Vec<u8> {
    let wasm = minimal_wasm();
    let key_id = actr_pack::compute_key_id(&signing_key.verifying_key().to_bytes());
    let manifest = actr_pack::PackageManifest {
        publisher: publisher.to_string(),
        name: actr_name.to_string(),
        version: version.to_string(),
        binary: actr_pack::BinaryEntry {
            path: "bin/actor.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            hash: String::new(),
            size: None,
            kind: None,
        },
        signature_algorithm: "ed25519".to_string(),
        signing_key_id: Some(key_id),
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        metadata: actr_pack::ManifestMetadata::default(),
    };
    let opts = actr_pack::PackOptions {
        manifest,
        binary_bytes: wasm,
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        signing_key: signing_key.clone(),
    };
    actr_pack::pack(&opts).unwrap()
}

/// Build loran Publisher Registry verifying_key response body
fn verifying_key_response(verifying_key: &ed25519_dalek::VerifyingKey) -> String {
    let key_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes());
    format!(r#"{{"public_key":"{key_b64}"}}"#)
}

// ─── Test cases ─────────────────────────────────────────────────────────────────

/// Scenario 1: production mode, first verification -> fetch Publisher Registry public key from Issuer -> passes
#[tokio::test]
async fn production_mode_fetches_publisher_key_and_verifies() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let key_id = actr_pack::compute_key_id(&verifying_key.to_bytes());

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "GET",
            format!("/registry/publishers/acme/verifying-key?key_id={}", key_id).as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&verifying_key))
        .expect(1) // called exactly once
        .create_async()
        .await;

    let package = make_signed_package("acme", "Sensor", "1.0.0", &signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    let verified = hyper
        .verify_package(&WorkloadPackage::new(package))
        .await
        .unwrap();

    mock.assert_async().await;
    assert_eq!(verified.manifest.actor_type().publisher, "acme");
    assert_eq!(verified.manifest.actor_type().name, "Sensor");
    assert_eq!(verified.manifest.actor_type().version, "1.0.0");
}

/// Scenario 2: two consecutive verifications for the same publisher -> second uses cache, no HTTP
#[tokio::test]
async fn production_mode_caches_publisher_key_on_second_verify() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let key_id = actr_pack::compute_key_id(&verifying_key.to_bytes());

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "GET",
            format!(
                "/registry/publishers/cached-publisher/verifying-key?key_id={}",
                key_id
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&verifying_key))
        .expect(1) // called exactly once; second time uses cache
        .create_async()
        .await;

    let package = make_signed_package("cached-publisher", "App", "1.0.0", &signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    // First: miss -> HTTP
    hyper
        .verify_package(&WorkloadPackage::new(package.clone()))
        .await
        .unwrap();
    // Second: hit -> no HTTP
    hyper
        .verify_package(&WorkloadPackage::new(package))
        .await
        .unwrap();

    mock.assert_async().await; // verify it was called only once
}

/// Scenario 3: Publisher Registry not registered (Issuer returns 404) -> UntrustedPublisher
#[tokio::test]
async fn production_mode_returns_untrusted_for_unknown_publisher() {
    let signing_key = SigningKey::generate(&mut OsRng);

    let key_id = actr_pack::compute_key_id(&signing_key.verifying_key().to_bytes());

    let mut server = mockito::Server::new_async().await;
    server
        .mock(
            "GET",
            format!(
                "/registry/publishers/unknown-publisher/verifying-key?key_id={}",
                key_id
            )
            .as_str(),
        )
        .with_status(404)
        .create_async()
        .await;

    let package = make_signed_package("unknown-publisher", "App", "1.0.0", &signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    let result = hyper.verify_package(&WorkloadPackage::new(package)).await;

    assert!(
        matches!(result, Err(HyperError::UntrustedPublisher(_))),
        "unknown Publisher Registry should return UntrustedPublisher, got: {result:?}"
    );
}

/// Scenario 4: correct Publisher Registry public key -> passes; wrong key (cached) -> signature mismatch
#[tokio::test]
async fn production_mode_rejects_wrong_cached_key() {
    let real_signing_key = SigningKey::generate(&mut OsRng);
    let wrong_key = SigningKey::generate(&mut OsRng); // different key

    let key_id = actr_pack::compute_key_id(&real_signing_key.verifying_key().to_bytes());
    let mut server = mockito::Server::new_async().await;

    // Issuer returns wrong_key's public key
    server
        .mock(
            "GET",
            format!(
                "/registry/publishers/publisher-x/verifying-key?key_id={}",
                key_id
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&wrong_key.verifying_key()))
        .create_async()
        .await;

    // Package signed with real_signing_key
    let package = make_signed_package("publisher-x", "X", "1.0.0", &real_signing_key);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    let result = hyper.verify_package(&WorkloadPackage::new(package)).await;

    assert!(
        matches!(result, Err(HyperError::SignatureVerificationFailed(_))),
        "wrong public key should return SignatureVerificationFailed, got: {result:?}"
    );
}

/// Scenario 5: two different publishers with independent keys -> each verifies independently
#[tokio::test]
async fn production_mode_independent_caches_per_publisher() {
    let key_a = SigningKey::generate(&mut OsRng);
    let key_b = SigningKey::generate(&mut OsRng);

    let key_id_a = actr_pack::compute_key_id(&key_a.verifying_key().to_bytes());
    let key_id_b = actr_pack::compute_key_id(&key_b.verifying_key().to_bytes());

    let mut server = mockito::Server::new_async().await;
    server
        .mock(
            "GET",
            format!(
                "/registry/publishers/publisher-a/verifying-key?key_id={}",
                key_id_a
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&key_a.verifying_key()))
        .create_async()
        .await;
    server
        .mock(
            "GET",
            format!(
                "/registry/publishers/publisher-b/verifying-key?key_id={}",
                key_id_b
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&key_b.verifying_key()))
        .create_async()
        .await;

    let pkg_a = make_signed_package("publisher-a", "ActorA", "1.0.0", &key_a);
    let pkg_b = make_signed_package("publisher-b", "ActorB", "1.0.0", &key_b);

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    let manifest_a = hyper
        .verify_package(&WorkloadPackage::new(pkg_a))
        .await
        .unwrap();
    let manifest_b = hyper
        .verify_package(&WorkloadPackage::new(pkg_b))
        .await
        .unwrap();

    assert_eq!(manifest_a.manifest.actor_type().publisher, "publisher-a");
    assert_eq!(manifest_b.manifest.actor_type().publisher, "publisher-b");
}

/// Standalone PublisherCertCache test: get_from_cache is synchronously readable after prefetch
#[tokio::test]
async fn cert_cache_get_from_cache_after_prefetch() {
    use ed25519_dalek::VerifyingKey;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let key_id = actr_pack::compute_key_id(&verifying_key.to_bytes());

    let mut server = mockito::Server::new_async().await;
    server
        .mock(
            "GET",
            format!(
                "/registry/publishers/sync-publisher/verifying-key?key_id={}",
                key_id
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(verifying_key_response(&verifying_key))
        .create_async()
        .await;

    let cache = PublisherCertCache::new(server.url());

    // get_from_cache returns None before prefetch
    let before = cache.get_from_cache("sync-publisher", Some(&key_id));
    assert!(before.is_none(), "cache should be empty before prefetch");

    // prefetch
    cache
        .get_or_fetch("sync-publisher", Some(&key_id))
        .await
        .unwrap();

    // get_from_cache now synchronously returns the public key
    let after: Option<VerifyingKey> = cache.get_from_cache("sync-publisher", Some(&key_id));
    assert!(after.is_some(), "cache should hit after prefetch");
    assert_eq!(
        after.unwrap().to_bytes(),
        verifying_key.to_bytes(),
        "cached public key should match the signing key"
    );
}

/// Non-.actr bytes -> InvalidManifest (no HTTP triggered)
#[tokio::test]
async fn production_mode_no_http_for_unknown_format() {
    let server = mockito::Server::new_async().await;
    // No mock endpoints set; if HTTP is triggered, the test fails

    let dir = TempDir::new().unwrap();
    let hyper = Hyper::new(prod_config(&dir, &server.url())).await.unwrap();

    let result = hyper
        .verify_package(&WorkloadPackage::new(b"this is not a package".to_vec()))
        .await;
    assert!(
        matches!(result, Err(HyperError::InvalidManifest(_))),
        "unknown format should return InvalidManifest, got: {result:?}"
    );
}
