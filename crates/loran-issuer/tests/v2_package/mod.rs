//! Real Issuer/Signer/SQLite coverage of the v2 wire contract shared with #422.
use super::*;
use actr_hyper::issuer_client::{IssuerClient, RealmClientKey};
use actr_pack::{ActorPackOptions, ActorPackageManifest, RegistrationManifest};
use ed25519_dalek::{Signer as _, SigningKey};
use rand::rngs::OsRng;
use std::collections::BTreeMap;

const PUBLISHER: &str = "v2-publisher.example";
const TEMPLATE: &str = include_str!("../../../actr-pack/tests/fixtures/registration-v2.toml");

/// Produce the public material a Host forwards, using the real packer/verifier.
/// Normally this uses an empty container fixture. Joint acceptance supplies
/// the exact real Component bytes already executed by the runtime scenario.
fn v2_request(realm: &str, key: &SigningKey) -> RegisterRequest {
    v2_request_for_publisher(realm, PUBLISHER, key)
}

fn v2_request_for_publisher(realm: &str, publisher: &str, key: &SigningKey) -> RegisterRequest {
    let mut manifest = ActorPackageManifest::from_toml(TEMPLATE).unwrap();
    manifest.actor.publisher = publisher.into();
    let component = match std::env::var_os("ACTR_V2_COMPONENT_FIXTURE") {
        Some(path) => {
            let bytes = std::fs::read(path).expect("read the real E2E Component fixture");
            assert!(
                bytes.starts_with(b"\0asm\x0d\0\x01\0"),
                "fixture must be a Component"
            );
            bytes
        }
        None => b"\0asm\x0d\0\x01\0".to_vec(),
    };
    let package = actr_pack::pack_actor_package(&ActorPackOptions {
        manifest,
        payloads: BTreeMap::from([("workload/echo.wasm".into(), component)]),
        signing_key: key.clone(),
    })
    .unwrap();
    let verified = actr_pack::verify_actor_package(&package, &key.verifying_key()).unwrap();
    let identity = RegistrationManifest::parse(&verified.manifest_raw).unwrap();
    RegisterRequest {
        actr_type: identity.actor_type().clone(),
        realm: realm.into(),
        service_spec: None,
        acl: None,
        service: None,
        ws_address: None,
        manifest_raw: Some(verified.manifest_raw.into()),
        publisher_signature: Some(verified.signature_raw.into()),
        target: None,
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: Some(key.verifying_key().to_bytes().to_vec().into()),
    }
}

fn success(response: RegisterResponse) -> register_response::RegisterOk {
    match response.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("expected successful v2 registration: {other:?}"),
    }
}

fn resign(
    request: &RegisterRequest,
    source: impl AsRef<[u8]>,
    key: &SigningKey,
) -> RegisterRequest {
    let mut changed = request.clone();
    changed.manifest_raw = Some(source.as_ref().to_vec().into());
    changed.publisher_signature = Some(key.sign(source.as_ref()).to_bytes().to_vec().into());
    changed
}

async fn seed_main_service(realm: &str) {
    sqlx::query(
        "INSERT INTO realm_profile (realm, sequence, digest, document, accepted_at)
         VALUES (?, 1, 'v2-fixture', ?, 1)",
    )
    .bind(realm)
    .bind(serde_json::json!({"package": {"type": format!("{PUBLISHER}/echo@1.0.0")}}).to_string())
    .execute(platform::storage::db::get_database().get_pool())
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn v2_package_sdk_register_renew_reissue_preserves_raw_bytes_and_identity() {
    use axum::middleware;

    let mut env = setup_test_environment().await;
    let realm = "v2-sdk@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    seed_main_service(realm).await;
    let pool = platform::storage::db::get_database().get_pool();
    let publisher_key = SigningKey::generate(&mut OsRng);
    seed_publisher_with_key(pool, PUBLISHER, &publisher_key).await;
    let mut registration = v2_request(realm, &publisher_key);
    // Preserve Publisher comments/CRLF too: signatures are over bytes, not TOML values.
    let source = String::from_utf8(registration.manifest_raw.as_ref().unwrap().to_vec())
        .unwrap()
        .replace('\n', "\r\n");
    registration = resign(
        &registration,
        format!("# Publisher bytes\r\n{source}"),
        &publisher_key,
    );
    drop(publisher_key); // The SDK only holds public package material and its device key.

    let captured = Arc::new(Mutex::new(Vec::<RegisterRequest>::new()));
    let record = captured.clone();
    let app = create_test_router(&env).await.layer(middleware::from_fn(
        move |mut request: Request<Body>, next: middleware::Next| {
            let record = record.clone();
            async move {
                let path = request.uri().path().to_string();
                if matches!(path.as_str(), "/register" | "/reissue") {
                    let body = std::mem::replace(request.body_mut(), Body::empty());
                    let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                    let registration = if path == "/register" {
                        RegisterRequest::decode(bytes.clone()).unwrap()
                    } else {
                        // Retain public registration only, never the possession token.
                        ReissueCredentialRequest::decode(bytes.clone())
                            .unwrap()
                            .registration
                    };
                    record.lock().unwrap().push(registration);
                    *request.body_mut() = Body::from(bytes);
                }
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let client = IssuerClient::new(&endpoint).with_client_key(RealmClientKey::new(
        realm,
        PROVIDER_ID,
        key_for(ROUTE_KEY_LABEL),
    ));
    let registered = success(
        client
            .register_with_manifest(registration.clone())
            .await
            .unwrap(),
    );
    assert_eq!(registered_type(&registered), registration.actr_type);
    assert_eq!(development(&registered), None);
    let claims = actr_protocol::IdentityClaims::decode(&registered.credential.claims[..]).unwrap();
    assert_eq!(
        claims.client_key,
        client_key_id(key_for(ROUTE_KEY_LABEL).verifying_key().as_bytes())
    );
    assert_eq!(claims.role(), actr_protocol::MemberRole::Owner);

    let renew_request = RenewCredentialRequest {
        actr_id: registered.actr_id.clone(),
        renewal_token: registered.renewal_token.clone().unwrap(),
        attempt_id: control_attempt_id(),
    };
    let renew_response = client
        .renew_credential(renew_request.clone())
        .await
        .unwrap();
    let replay = client.renew_credential(renew_request).await.unwrap();
    assert_eq!(renew_response.encode_to_vec(), replay.encode_to_vec());
    let renewed = renewed(renew_response);
    assert_eq!(renewed.actr_id, registered.actr_id);

    // A second device of the same member can recover the Actor after the first
    // device is revoked. It still needs the original Actor possession proof.
    let owner = owner(realm).await;
    let recovery_key = key_for("v2-recovery-device");
    let invitation = membership::create_invite(
        &owner,
        None,
        Some(&owner.member_id),
        chrono::Utc::now().timestamp() + 3600,
        limits(),
    )
    .await
    .unwrap();
    let secret = RealmInvite::decode(&invitation.invite).unwrap().secret;
    membership::join(
        realm,
        &invite_secret_hash(&secret).unwrap(),
        recovery_key.verifying_key().as_bytes(),
        "v2 recovery device",
        limits(),
    )
    .await
    .unwrap();
    membership::revoke_client(&owner, &owner.key_id)
        .await
        .unwrap();
    let recovery_client = IssuerClient::new(&endpoint).with_client_key(RealmClientKey::new(
        realm,
        PROVIDER_ID,
        recovery_key.clone(),
    ));
    let reissue_request = ReissueCredentialRequest {
        actr_id: registered.actr_id.clone(),
        registration: registration.clone(),
        renewal_proof: renewed.renewal_token,
        attempt_id: control_attempt_id(),
    };
    let reissue_response = recovery_client
        .reissue_credential(reissue_request.clone())
        .await
        .unwrap();
    let replay = recovery_client
        .reissue_credential(reissue_request)
        .await
        .unwrap();
    assert_eq!(reissue_response.encode_to_vec(), replay.encode_to_vec());
    let reissued = success(reissue_response);
    assert_eq!(reissued.actr_id, registered.actr_id);
    assert_eq!(registered_type(&reissued), registration.actr_type);
    assert_eq!(development(&reissued), None);
    let claims = actr_protocol::IdentityClaims::decode(&reissued.credential.claims[..]).unwrap();
    assert_eq!(
        claims.client_key,
        client_key_id(recovery_key.verifying_key().as_bytes())
    );
    {
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for actual in requests.iter() {
            assert_eq!(
                actual, &registration,
                "the wire preserves the original registration"
            );
        }
    }
    let (target, auth_mode): (Option<String>, i32) = sqlx::query_as(
        "SELECT target, auth_mode FROM issued_actor_identity WHERE realm = ? AND serial_number = ?",
    )
    .bind(realm)
    .bind(registered.actr_id.serial_number as i64)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(target, None);
    assert_eq!(auth_mode, RegisterAuthMode::Package as i32);
    let publications: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='publisher_package'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(publications, 0);
    server.abort();
    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn v2_package_rejects_schema_target_identity_and_signature_confusion() {
    let mut env = setup_test_environment().await;
    let realm = "v2-reject@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let key = SigningKey::generate(&mut OsRng);
    seed_publisher_with_key(
        platform::storage::db::get_database().get_pool(),
        PUBLISHER,
        &key,
    )
    .await;
    let request = v2_request(realm, &key);
    let source = std::str::from_utf8(request.manifest_raw.as_ref().unwrap()).unwrap();
    let mut cases = Vec::new();
    for target in ["", "wasm32-wasip2"] {
        let mut changed = request.clone();
        changed.target = Some(target.into());
        cases.push(("v2 target must be absent", changed, 400));
    }
    for changed in [
        source.replace("actr-package/2", "actr-package/3"),
        source.replace("actr-package/2", "actr-package/1"),
        source.replace("format = \"actr-package/2\"", "format = 2"),
        source.replace("format = \"actr-package/2\"", ""),
        source.replace("algorithm = \"ed25519\"", "algorithm = \"rsa\""),
        source.replace(
            "entry = \"workload/echo.wasm\"",
            "entry = \"../escape.wasm\"",
        ),
        source.replace("[host]", "[wrong_host]"),
        format!("publisher = '{PUBLISHER}'\nname = 'echo'\nversion = '1.0.0'\n{source}"),
    ] {
        cases.push((
            "invalid/mixed v2 schema",
            resign(&request, changed, &key),
            400,
        ));
    }
    let mut changed = request.clone();
    changed.manifest_raw = Some(format!("{source}\n# unsigned edit").into_bytes().into());
    cases.push(("raw bytes changed without a new signature", changed, 403));
    let mut changed = request.clone();
    changed.actr_type.name = "other-actor".into();
    cases.push((
        "request identity differs from signed identity",
        changed,
        403,
    ));
    let wrong_key = SigningKey::generate(&mut OsRng);
    let mut changed = request.clone();
    changed.publisher_signature =
        Some(wrong_key.sign(source.as_bytes()).to_bytes().to_vec().into());
    changed.signing_public_key = Some(wrong_key.verifying_key().to_bytes().to_vec().into());
    cases.push((
        "request key cannot replace trusted Publisher key",
        changed,
        403,
    ));
    let wrong_id = source.replace(
        &actr_pack::compute_key_id(key.verifying_key().as_bytes()),
        "unknown-key",
    );
    cases.push((
        "unknown key id cannot downgrade a Publisher",
        resign(&request, wrong_id, &key),
        403,
    ));
    let mut changed = request.clone();
    changed.publisher_signature = Some(vec![0; 63].into());
    cases.push(("short signature", changed, 403));
    let mut changed = request.clone();
    changed.manifest_raw = Some(vec![0xff].into());
    cases.push(("non UTF-8", changed, 400));
    let mut changed = request.clone();
    changed.manifest_raw = Some(vec![b' '; actr_pack::MAX_REGISTRATION_MANIFEST_BYTES + 1].into());
    cases.push(("oversized manifest", changed, 400));
    let mut changed = request.clone();
    changed.auth_mode = Some(RegisterAuthMode::Linked as i32);
    cases.push((
        "package cannot impersonate linked registration",
        changed,
        400,
    ));

    let app = create_test_router(&env).await;
    for (reason, request, expected) in cases {
        let response = post_register(app.clone(), request, Some(ROUTE_KEY_LABEL)).await;
        assert_eq!(
            error_code(&response),
            Some(expected),
            "{reason}: {response:?}"
        );
    }
    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn v2_package_keeps_membership_owner_and_revocation_checks() {
    let mut env = setup_test_environment().await;
    let realm = "v2-membership@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    seed_main_service(realm).await;
    let key = SigningKey::generate(&mut OsRng);
    seed_publisher_with_key(
        platform::storage::db::get_database().get_pool(),
        PUBLISHER,
        &key,
    )
    .await;
    let request = v2_request(realm, &key);
    let viewer = add_client(realm, "v2-viewer", Role::Viewer).await;
    let app = create_test_router(&env).await;
    for label in ["v2-unknown-device", "v2-viewer"] {
        let response = post_register(app.clone(), request.clone(), Some(label)).await;
        assert_eq!(error_code(&response), Some(403));
    }
    let owner = owner(realm).await;
    membership::revoke_client(&owner, &viewer.key_id)
        .await
        .unwrap();
    let response = post_register(app.clone(), request.clone(), Some("v2-viewer")).await;
    assert_eq!(error_code(&response), Some(403));
    let registered =
        success(post_register(app.clone(), request.clone(), Some(ROUTE_KEY_LABEL)).await);

    // The v2 verifier must not bypass the existing possession/member/type checks.
    let attempt = ReissueCredentialRequest {
        actr_id: registered.actr_id.clone(),
        registration: request.clone(),
        renewal_proof: Some(vec![7; 32].into()),
        attempt_id: control_attempt_id(),
    };
    let refused = post_reissue(app.clone(), attempt, Some(ROUTE_KEY_LABEL)).await;
    assert_eq!(
        error_code(&refused),
        Some(REISSUE_POSSESSION_NOT_PROVEN_CODE)
    );
    let source = std::str::from_utf8(request.manifest_raw.as_ref().unwrap()).unwrap();
    let mut changed = resign(
        &request,
        source.replace("name = \"echo\"", "name = \"other\""),
        &key,
    );
    changed.actr_type.name = "other".into();
    let refused = post_reissue(
        app.clone(),
        ReissueCredentialRequest {
            actr_id: registered.actr_id.clone(),
            registration: changed,
            renewal_proof: registered.renewal_token.clone(),
            attempt_id: control_attempt_id(),
        },
        Some(ROUTE_KEY_LABEL),
    )
    .await;
    assert_eq!(error_code(&refused), Some(400));
    let foreign = add_client(realm, "v2-other-member", Role::Editor).await;
    assert_ne!(foreign.member_id, owner.member_id);
    let refused = post_reissue(
        app,
        ReissueCredentialRequest {
            actr_id: registered.actr_id,
            registration: request,
            renewal_proof: registered.renewal_token,
            attempt_id: control_attempt_id(),
        },
        Some("v2-other-member"),
    )
    .await;
    assert_eq!(error_code(&refused), Some(403));
    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn v2_package_development_identity_cannot_become_a_registered_publisher() {
    let mut env = setup_test_environment().await;
    let realm = "v2-development@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let key = SigningKey::generate(&mut OsRng);
    let publisher = "v2-development-publisher.example";
    let request = v2_request_for_publisher(realm, publisher, &key);
    let app = create_test_router(&env).await;
    let registered =
        success(post_register(app.clone(), request.clone(), Some(ROUTE_KEY_LABEL)).await);
    assert_eq!(development(&registered), Some(true));
    seed_publisher_with_key(
        platform::storage::db::get_database().get_pool(),
        publisher,
        &key,
    )
    .await;
    let (status, response) = post_renew_raw(
        app.clone(),
        RenewCredentialRequest {
            actr_id: registered.actr_id.clone(),
            renewal_token: registered.renewal_token.clone().unwrap(),
            attempt_id: control_attempt_id(),
        },
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        matches!(response.result, Some(renew_credential_response::Result::Error(e)) if e.code == 403)
    );
    let response = post_reissue(
        app.clone(),
        ReissueCredentialRequest {
            actr_id: registered.actr_id,
            registration: request,
            renewal_proof: registered.renewal_token,
            attempt_id: control_attempt_id(),
        },
        Some(ROUTE_KEY_LABEL),
    )
    .await;
    assert_eq!(error_code(&response), Some(403));
    let attacker = SigningKey::generate(&mut OsRng);
    let response = post_register(
        app,
        v2_request_for_publisher(realm, publisher, &attacker),
        Some(ROUTE_KEY_LABEL),
    )
    .await;
    assert_eq!(error_code(&response), Some(403));
    env.shutdown_signer().await;
}

#[tokio::test]
#[serial]
async fn v2_reissue_clears_a_legacy_target_without_changing_actor_id() {
    let mut env = setup_test_environment().await;
    let realm = "v2-upgrade@example.com";
    seed_realm(realm, Some(ROUTE_KEY_LABEL)).await;
    let key = SigningKey::generate(&mut OsRng);
    let pool = platform::storage::db::get_database().get_pool();
    seed_publisher_with_key(pool, PUBLISHER, &key).await;
    let app = create_test_router(&env).await;
    let legacy = signed_package_request(PUBLISHER, "echo", "1.0.0", realm, "wasm32-wasip2", &key);
    let registered = success(post_register(app.clone(), legacy, Some(ROUTE_KEY_LABEL)).await);
    let reissued = success(
        post_reissue(
            app.clone(),
            ReissueCredentialRequest {
                actr_id: registered.actr_id.clone(),
                registration: v2_request(realm, &key),
                renewal_proof: registered.renewal_token,
                attempt_id: control_attempt_id(),
            },
            Some(ROUTE_KEY_LABEL),
        )
        .await,
    );
    assert_eq!(reissued.actr_id, registered.actr_id);
    let target: Option<String> = sqlx::query_scalar(
        "SELECT target FROM issued_actor_identity WHERE realm = ? AND serial_number = ?",
    )
    .bind(realm)
    .bind(registered.actr_id.serial_number as i64)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        target, None,
        "v2 reissue must not keep a stale legacy target"
    );
    let renewed = renewed(
        post_renew(
            app,
            RenewCredentialRequest {
                actr_id: reissued.actr_id.clone(),
                renewal_token: reissued.renewal_token.unwrap(),
                attempt_id: control_attempt_id(),
            },
        )
        .await,
    );
    assert_eq!(renewed.actr_id, registered.actr_id);
    env.shutdown_signer().await;
}
