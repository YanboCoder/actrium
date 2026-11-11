//! Exercise the SDK against real HTTP handlers and the real membership database.
use super::*;
use actr_hyper::issuer_client::{IssuerClient, RealmClientKey, RealmKeySigner};
use actr_hyper::realm_membership::{
    MembershipClientError, MembershipEndpoint, RealmMembershipClient,
};
use std::sync::Arc;

struct DeviceSigner(ed25519_dalek::SigningKey);
impl RealmKeySigner for DeviceSigner {
    fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    fn sign_payload(&self, bytes: &[u8]) -> Result<[u8; 64], actr_hyper::HyperError> {
        Ok(ed25519_dalek::Signer::sign(&self.0, bytes).to_bytes())
    }
}

fn device(endpoint: &str, realm: &str) -> RealmMembershipClient {
    let signer = DeviceSigner(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng));
    let key = RealmClientKey::from_signer(realm.into(), PROVIDER_ID.into(), Arc::new(signer));
    RealmMembershipClient::new(
        MembershipEndpoint {
            realm: realm.into(),
            provider_id: PROVIDER_ID.into(),
            issuer: endpoint.into(),
        },
        key,
        true,
    )
    .unwrap()
}

async fn invite(realm: &str) -> platform::realm::membership::IssuedInvite {
    membership::create_invite(
        &owner(realm).await,
        Some(Role::Editor),
        None,
        chrono::Utc::now().timestamp() + 3600,
        limits(),
    )
    .await
    .unwrap()
}

fn refused(result: Result<serde_json::Value, MembershipClientError>, expected: &str) {
    match result {
        Err(MembershipClientError::Rejected { code, .. }) => assert_eq!(code, expected),
        other => panic!("expected {expected}, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn sdk_devices_join_retry_register_renew_reissue_and_revoke() {
    use axum::{middleware, response::IntoResponse};
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut env = setup_test_environment().await;
    seed_realm(TEST_REALM, None).await;
    let lose_first = Arc::new(AtomicBool::new(true));
    let app = create_test_router(&env).await.layer(middleware::from_fn(
        move |request: Request<Body>, next: middleware::Next| {
            let lose_first = lose_first.clone();
            async move {
                let membership = request.uri().path() == "/membership";
                let response = next.run(request).await;
                if membership
                    && response.status().is_success()
                    && lose_first.swap(false, Ordering::SeqCst)
                {
                    // The transaction committed before the response was lost.
                    StatusCode::BAD_GATEWAY.into_response()
                } else {
                    response
                }
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
    let a = device(&endpoint, TEST_REALM);
    let b = device(&endpoint, TEST_REALM);
    assert_ne!(a.key_id(), b.key_id());
    assert_ne!(a.key_id(), device(&endpoint, "other@example.com").key_id());
    let invitation = invite(TEST_REALM).await;
    assert!(matches!(
        a.join(&invitation.invite, "A").await,
        Err(MembershipClientError::Uncertain)
    ));
    let joined = a.join(&invitation.invite, "A").await.unwrap();
    assert_eq!(
        a.join(&invitation.invite, "A retry").await.unwrap()["key_id"],
        joined["key_id"]
    );
    refused(b.join(&invitation.invite, "B").await, "invite_used");
    let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let add = a
        .invite_client(joined["member_id"].as_str().unwrap(), &expires)
        .await
        .unwrap();
    assert_eq!(
        b.join(add["invite"].as_str().unwrap(), "B").await.unwrap()["member_id"],
        joined["member_id"]
    );
    assert!(a.members().await.unwrap()["members"].is_array());

    let client_a = IssuerClient::new(&endpoint).with_client_key(a.client_key());
    let client_b = IssuerClient::new(&endpoint).with_client_key(b.client_key());
    let registration = linked_register_request();
    let registered_a = client_a
        .register_linked(registration.clone())
        .await
        .unwrap();
    let registered_b = client_b
        .register_linked(registration.clone())
        .await
        .unwrap();
    let success = |response: RegisterResponse| match response.result.unwrap() {
        register_response::Result::Success(ok) => ok,
        other => panic!("{other:?}"),
    };
    let aa = success(registered_a);
    let bb = success(registered_b);
    let renew = |ok: &register_response::RegisterOk| RenewCredentialRequest {
        actr_id: ok.actr_id.clone(),
        renewal_token: ok.renewal_token.clone().unwrap(),
        attempt_id: control_attempt_id(),
    };
    let renewed = client_a.renew_credential(renew(&aa)).await.unwrap();
    assert!(matches!(
        renewed.result,
        Some(renew_credential_response::Result::Success(_))
    ));
    b.revoke_client(&a.key_id()).await.unwrap();
    assert!(
        matches!(client_a.register_linked(registration.clone()).await.unwrap().result,
        Some(register_response::Result::Error(error)) if error.code == 403 && error.message.starts_with("client_revoked:"))
    );
    assert!(client_a.renew_credential(renew(&aa)).await.is_err());
    assert!(client_b.renew_credential(renew(&bb)).await.is_ok());
    assert!(client_b.register_linked(registration.clone()).await.is_ok());
    let reissue = ReissueCredentialRequest {
        actr_id: bb.actr_id.clone(),
        registration,
        renewal_proof: bb.renewal_token,
        attempt_id: control_attempt_id(),
    };
    assert_eq!(
        success(client_b.reissue_credential(reissue).await.unwrap()).actr_id,
        bb.actr_id
    );
    refused(a.members().await, "client_revoked");

    let c = device(&endpoint, TEST_REALM);
    let expired = invite(TEST_REALM).await;
    sqlx::query("UPDATE realm_invite SET expires_at = 1 WHERE invite_id = ?")
        .bind(&expired.invite_id)
        .execute(platform::storage::db::get_database().get_pool())
        .await
        .unwrap();
    refused(c.join(&expired.invite, "expired").await, "invite_expired");
    let revoked = invite(TEST_REALM).await;
    membership::revoke_invite(&owner(TEST_REALM).await, &revoked.invite_id)
        .await
        .unwrap();
    refused(c.join(&revoked.invite, "revoked").await, "invite_revoked");
    refused(
        device(&endpoint, "other@example.com")
            .join(&revoked.invite, "mismatch")
            .await,
        "invite_realm_mismatch",
    );
    server.abort();
    env.shutdown_signer().await;
}

/// Started explicitly by the mobile SDK integration harness. Secrets go only
/// to a mode-0600 file, never HTTP helper endpoints, logs or build resources.
#[cfg(unix)]
#[tokio::test]
#[serial]
#[ignore = "requires an installed mobile SDK test runner"]
async fn mobile_issuer_http_fixture() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let ready = std::env::var("ACTR_MOBILE_FIXTURE_READY").expect("fixture file");
    let mut env = setup_test_environment().await;
    seed_realm(TEST_REALM, None).await;
    let owner = owner(TEST_REALM).await;
    let invite = membership::create_invite(
        &owner,
        None,
        Some(&owner.member_id),
        chrono::Utc::now().timestamp() + 3600,
        limits(),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    // Only signaling is simulated. Membership, register/reissue/renew and
    // revocation use the production Issuer and embedded signing service.
    let mut signaling = actr_mock_loran::MockLoranServer::start().await.unwrap();
    signaling.warn_credential_expiring(true);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&ready)
        .unwrap();
    let fixture = serde_json::json!({"realm": TEST_REALM, "provider_id": PROVIDER_ID,
        "issuer": endpoint, "signaling": signaling.ws_url(), "invite": invite.invite});
    file.write_all(serde_json::to_string(&fixture).unwrap().as_bytes())
        .unwrap();
    let signer_client = create_signer_client(&env.signer_config, &env.shared_key)
        .await
        .unwrap();
    let mut config = default_issuer_config(&env.issuer_temp_dir);
    config.token_ttl_secs = 8;
    config.signaling_heartbeat_interval_secs = 1;
    let issuer = AIdIssuer::new(
        signer_client,
        config,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let app = create_router(IssuerState::new(issuer));
    let observations = Arc::new(Mutex::new(
        serde_json::Map::<String, serde_json::Value>::new(),
    ));
    let observed_file = format!("{ready}.observed");
    let app = app.layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let observations = observations.clone();
            let observed_file = observed_file.clone();
            async move {
                let op = request.uri().path().trim_start_matches('/').to_string();
                let key = request
                    .headers()
                    .get(CLIENT_PROOF_HEADER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| ClientProof::parse_header(h).ok())
                    .map(|p| p.signer_key_id().to_string());
                let response = next.run(request).await;
                if let Some(key) = key {
                    let mut observed = observations.lock().unwrap();
                    let row = observed.entry(key).or_insert(serde_json::json!({}));
                    let code = format!("{op}:{}", response.status().as_u16());
                    row[&code] = serde_json::json!(row[&code].as_u64().unwrap_or(0) + 1);
                    // Only public key IDs and counters; no proofs, tokens or invites.
                    std::fs::write(observed_file, serde_json::to_vec(&*observed).unwrap()).unwrap();
                }
                response
            }
        },
    ));
    let stop_path = format!("{ready}.stop");
    let stop = async move {
        for _ in 0..3600 {
            if std::path::Path::new(&stop_path).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stop)
    .await
    .unwrap();
    std::fs::remove_file(ready).unwrap();
    signaling.shutdown().await;
    env.shutdown_signer().await;
}
