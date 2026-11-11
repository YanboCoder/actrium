//! Integration tests for `MockLoranServer`.
//!
//! These tests treat the mock as an opaque black box (HTTP/WS client driving
//! it) and make sure it speaks the wire format expected by `actr-hyper`.

use actr_hyper::{IssuerClient, PublisherCertCache};
use actr_mock_loran::MockLoranServer;
use actr_protocol::prost::Message as _;
use actr_protocol::{ActrType, RegisterAuthMode, RegisterRequest, register_response};
use base64::Engine;
use ed25519_dalek::SigningKey;

fn test_register_request() -> RegisterRequest {
    RegisterRequest {
        actr_type: ActrType {
            publisher: "test-publisher".into(),
            name: "TestSvc".into(),
            version: "0.1.0".into(),
        },
        realm: "test42@example.com".to_string(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(b"manifest".to_vec().into()),
        publisher_signature: Some(vec![0u8; 64].into()),
        target: Some("wasm32-wasip2".into()),
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: None,
    }
}

#[tokio::test]
async fn http_register_returns_valid_protobuf() {
    let server = MockLoranServer::start().await.unwrap();
    let client = IssuerClient::new(server.http_url());
    let resp = client
        .register_with_manifest(test_register_request())
        .await
        .expect("register should succeed");

    let ok = match resp.result {
        Some(register_response::Result::Success(ok)) => ok,
        other => panic!("expected Success, got {other:?}"),
    };

    // Sanity check: allocated actr_id + credential signed by the mock's Issuer key.
    assert_eq!(ok.actr_id.realm, "test42@example.com");
    assert!(ok.actr_id.serial_number > 0);
    assert_eq!(ok.credential.key_id, 1);
    assert_eq!(
        ok.signing_pubkey.as_ref(),
        server.issuer_signing_key().verifying_key().as_bytes()
    );
    assert!(ok.renewal_token.is_some());
}

#[tokio::test]
async fn http_register_also_works_under_issuer_prefix() {
    let server = MockLoranServer::start().await.unwrap();
    // Configs in the wild use issuer_endpoint = "http://host:port/issuer" — make
    // sure IssuerClient's `{endpoint}/register` still lands on the mock.
    let client = IssuerClient::new(format!("{}/issuer", server.http_url()));
    let resp = client
        .register_with_manifest(test_register_request())
        .await
        .expect("register via /issuer prefix should succeed");
    assert!(matches!(
        resp.result,
        Some(register_response::Result::Success(_))
    ));
}

#[tokio::test]
async fn publisher_verifying_key_roundtrip_via_publisher_cert_cache() {
    let server = MockLoranServer::start().await.unwrap();

    // Seed an Publisher Registry; the verifying key we register here is what the cache must
    // return.
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let key_id = actr_pack::compute_key_id(&verifying_key.to_bytes());
    server.add_publisher("acme", verifying_key).await;

    let cache = PublisherCertCache::new(server.http_url());
    let fetched = cache.get_or_fetch("acme", Some(&key_id)).await.unwrap();
    assert_eq!(fetched.to_bytes(), verifying_key.to_bytes());

    // Second fetch hits the in-process cache (no way to assert 0 HTTP calls
    // without intercepting, but we can at least assert it still returns the
    // same key and doesn't panic).
    let fetched2 = cache.get_or_fetch("acme", Some(&key_id)).await.unwrap();
    assert_eq!(fetched2.to_bytes(), verifying_key.to_bytes());
}

#[tokio::test]
async fn publisher_verifying_key_404_for_unregistered_publisher() {
    let server = MockLoranServer::start().await.unwrap();

    let url = format!(
        "{}/registry/publishers/ghost/verifying-key",
        server.http_url()
    );
    let status = reqwest::get(&url).await.unwrap().status();
    assert_eq!(status, 404);
}

#[tokio::test]
async fn admin_publisher_seeds_verifying_key() {
    let server = MockLoranServer::start().await.unwrap();

    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let pubkey_b64 =
        base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/publisher", server.http_url()))
        .json(&serde_json::json!({
            "name": "seeded",
            "pubkey_b64": pubkey_b64,
            "contact": "dev@example.com",
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "admin/publisher should succeed");

    let cache = PublisherCertCache::new(server.http_url());
    let fetched = cache.get_or_fetch("seeded", None).await.unwrap();
    assert_eq!(fetched.to_bytes(), signing_key.verifying_key().to_bytes());
}

#[tokio::test]
async fn admin_realm_and_state_endpoints() {
    let server = MockLoranServer::start().await.unwrap();

    let client = reqwest::Client::new();
    client
        .post(format!("{}/admin/realms", server.http_url()))
        .json(&serde_json::json!({ "realm": "admin-seeded@example.com" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let snapshot: serde_json::Value = client
        .get(format!("{}/admin/state", server.http_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let realms = snapshot["realms"].as_array().unwrap();
    assert!(
        realms
            .iter()
            .any(|v| v.as_str() == Some("admin-seeded@example.com"))
    );
}

#[tokio::test]
async fn mock_renew_allows_consecutive_soft_renewals() {
    use actr_protocol::{
        RenewCredentialRequest, RenewCredentialResponse, renew_credential_response,
    };

    let server = MockLoranServer::start().await.unwrap();
    let client = IssuerClient::new(server.http_url());
    let resp = client
        .register_with_manifest(test_register_request())
        .await
        .expect("register should succeed");

    let ok = match resp.result {
        Some(register_response::Result::Success(ok)) => ok,
        other => panic!("expected Success, got {other:?}"),
    };
    let first_token = ok
        .renewal_token
        .clone()
        .expect("register should return renewal token");

    let http = reqwest::Client::new();
    let renew_url = format!("{}/renew", server.http_url());

    let first_req = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: first_token,
        attempt_id: bytes::Bytes::copy_from_slice(uuid::Uuid::new_v4().as_bytes()),
    };
    let first_resp = http
        .post(&renew_url)
        .header("content-type", "application/x-protobuf")
        .body(first_req.encode_to_vec())
        .send()
        .await
        .expect("first renew request should send");
    assert_eq!(first_resp.status(), reqwest::StatusCode::OK);
    let first_body = first_resp.bytes().await.unwrap();
    let first = RenewCredentialResponse::decode(first_body.as_ref()).unwrap();
    let first_ok = match first.result {
        Some(renew_credential_response::Result::Success(ok)) => ok,
        other => panic!("expected first renew success, got {other:?}"),
    };
    assert_eq!(first_ok.actr_id, ok.actr_id);

    let second_req = RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: first_ok
            .renewal_token
            .clone()
            .expect("renew should return next renewal token"),
        attempt_id: bytes::Bytes::copy_from_slice(uuid::Uuid::new_v4().as_bytes()),
    };
    let second_resp = http
        .post(&renew_url)
        .header("content-type", "application/x-protobuf")
        .body(second_req.encode_to_vec())
        .send()
        .await
        .expect("second renew request should send");
    assert_eq!(second_resp.status(), reqwest::StatusCode::OK);
    let second_body = second_resp.bytes().await.unwrap();
    let second = RenewCredentialResponse::decode(second_body.as_ref()).unwrap();
    let second_ok = match second.result {
        Some(renew_credential_response::Result::Success(ok)) => ok,
        other => panic!("expected second renew success, got {other:?}"),
    };
    assert_eq!(second_ok.actr_id, ok.actr_id);
}
