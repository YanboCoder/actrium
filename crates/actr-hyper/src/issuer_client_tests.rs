use super::*;
use actr_protocol::ErrorResponse;
use reqwest::header::HeaderValue;

// ── client construction (no network) ────────────────────────────────────

#[test]
fn client_new_holds_endpoint_and_no_key() {
    let c = IssuerClient::new("http://issuer.example.com:8080");
    assert_eq!(c.endpoint, "http://issuer.example.com:8080");
    assert!(c.client_key.is_none());
    assert!(
        c.proof_header(ProofOp::Register, "r@acme.com", b"x")
            .unwrap()
            .is_none()
    );
}

#[test]
fn client_key_signs_a_fresh_proof_of_the_body() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let public_key = key.verifying_key().to_bytes();
    let c = IssuerClient::new("http://issuer").with_client_key(RealmClientKey::new(
        "k7m2q9xh@acme.com",
        "issuer:8081",
        key,
    ));
    let (name, first) = c
        .proof_header(ProofOp::Renew, "k7m2q9xh@acme.com", b"body")
        .unwrap()
        .unwrap();
    assert_eq!(name, "x-actr-client-proof");
    let proof = ClientProof::parse_header(&first).unwrap();
    proof.verify(&public_key).unwrap();
    proof
        .check(
            ProofOp::Renew,
            "k7m2q9xh@acme.com",
            "issuer:8081",
            b"body",
            chrono::Utc::now(),
        )
        .unwrap();
    let (_, second) = c
        .proof_header(ProofOp::Renew, "k7m2q9xh@acme.com", b"body")
        .unwrap()
        .unwrap();
    assert_ne!(first, second, "each request is signed with a new nonce");
}

#[test]
fn client_key_file_must_be_of_the_configured_realm() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.json");
    let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let file = RealmClientKeyFile::new("k7m2q9xh@acme.com", "issuer:8081", "http://issuer", &key);
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let set = |mode| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
        set(0o644).unwrap();
        let error = RealmClientKey::load_for(Some(&path), "k7m2q9xh@acme.com").unwrap_err();
        assert!(error.to_string().contains("chmod 600"), "{error}");
        set(0o600).unwrap();
    }

    let loaded = RealmClientKey::load_for(Some(&path), "k7m2q9xh@acme.com")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.realm(), "k7m2q9xh@acme.com");
    assert!(RealmClientKey::load_for(Some(&path), "other@acme.com").is_err());
    assert!(
        RealmClientKey::load_for(None, "k7m2q9xh@acme.com")
            .unwrap()
            .is_none()
    );
}

#[test]
fn control_attempt_ids_are_raw_unique_uuids() {
    let first = new_control_attempt_id();
    let second = new_control_attempt_id();
    assert_eq!(first.len(), 16);
    assert_eq!(second.len(), 16);
    assert_ne!(first, second);
}

#[test]
fn mobile_proofs_allow_small_positive_clock_skew_without_extending_lifetime() {
    let key = RealmClientKey::new(
        "r@example.com",
        "provider.example",
        ed25519_dalek::SigningKey::from_bytes(&[8; 32]),
    );
    for op in [
        ProofOp::Register,
        ProofOp::Renew,
        ProofOp::Reissue,
        ProofOp::RelayIdentity,
    ] {
        let proof =
            ClientProof::parse_header(&key.proof(op, "r@example.com", b"body").unwrap()).unwrap();
        proof
            .check(
                op,
                "r@example.com",
                "provider.example",
                b"body",
                chrono::Utc::now() - chrono::Duration::seconds(20),
            )
            .unwrap();
        let issued = chrono::DateTime::parse_from_rfc3339(&proof.issued_at).unwrap();
        let expires = chrono::DateTime::parse_from_rfc3339(&proof.expires_at).unwrap();
        assert_eq!(
            (expires - issued).num_seconds(),
            actr_protocol::realm_management::MAX_VALIDITY_SECS
        );
        assert!(
            proof
                .check(
                    op,
                    "r@example.com",
                    "provider.example",
                    b"body",
                    chrono::Utc::now() + chrono::Duration::minutes(5)
                )
                .is_err()
        );
    }
}

#[test]
fn control_attempt_retry_backoff_is_bounded() {
    assert_eq!(control_attempt_backoff(0), Duration::from_millis(100));
    assert_eq!(control_attempt_backoff(4), Duration::from_millis(1600));
    assert_eq!(control_attempt_backoff(40), Duration::from_millis(1600));
}

// ── parse_retry_after ───────────────────────────────────────────────────

#[test]
fn parse_retry_after_valid_seconds() {
    let h = HeaderValue::from_static("120");
    assert_eq!(parse_retry_after(Some(&h)), Some(Duration::from_secs(120)));
}

#[test]
fn parse_retry_after_none() {
    assert_eq!(parse_retry_after(None), None);
}

#[test]
fn parse_retry_after_non_numeric_is_none() {
    // HTTP-date / non-numeric values are not supported → None.
    let h = HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT");
    assert_eq!(parse_retry_after(Some(&h)), None);

    let h2 = HeaderValue::from_static("abc");
    assert_eq!(parse_retry_after(Some(&h2)), None);
}

#[test]
fn parse_retry_after_zero() {
    let h = HeaderValue::from_static("0");
    assert_eq!(parse_retry_after(Some(&h)), Some(Duration::from_secs(0)));
}

// ── classify_renew_status ───────────────────────────────────────────────

#[test]
fn classify_status_client_errors() {
    assert!(matches!(
        classify_renew_status(400, None),
        RenewError::InvalidRequest(_)
    ));
    assert!(matches!(
        classify_renew_status(401, None),
        RenewError::TokenRejected
    ));
    assert!(matches!(
        classify_renew_status(403, None),
        RenewError::RealmUnavailable
    ));
    assert!(matches!(
        classify_renew_status(409, None),
        RenewError::AttemptConflict
    ));
    assert!(matches!(
        classify_renew_status(410, None),
        RenewError::AttemptExpired
    ));
    assert!(matches!(
        classify_renew_status(425, None),
        RenewError::AttemptInProgress
    ));
}

#[test]
fn classify_status_rate_limited_carries_retry_after() {
    // With header present.
    assert!(matches!(
        classify_renew_status(429, Some(Duration::from_secs(30))),
        RenewError::RateLimited { retry_after: Some(d) } if d == Duration::from_secs(30)
    ));
    // Without header.
    assert!(matches!(
        classify_renew_status(429, None),
        RenewError::RateLimited { retry_after: None }
    ));
}

#[test]
fn classify_status_5xx_retryable() {
    for code in [500u16, 502, 503, 504] {
        assert!(
            matches!(classify_renew_status(code, None), RenewError::Retryable(_)),
            "{code} should be Retryable"
        );
    }
}

#[test]
fn classify_status_unknown_is_protocol() {
    assert!(matches!(
        classify_renew_status(418, None),
        RenewError::Protocol(_)
    ));
}

// ── classify_renew_error (delegates to status, retry_after=None) ─────────

#[test]
fn classify_error_maps_code_to_variant() {
    // 429 via ErrorResponse path never carries retry_after (header lost).
    let rate = ErrorResponse {
        code: 429,
        message: "slow down".into(),
    };
    assert!(matches!(
        classify_renew_error(&rate),
        RenewError::RateLimited { retry_after: None }
    ));

    let unauth = ErrorResponse {
        code: 401,
        message: "bad token".into(),
    };
    assert!(matches!(
        classify_renew_error(&unauth),
        RenewError::TokenRejected
    ));

    let boom = ErrorResponse {
        code: 503,
        message: "down".into(),
    };
    assert!(matches!(
        classify_renew_error(&boom),
        RenewError::Retryable(_)
    ));
}

#[test]
fn renew_error_display_messages() {
    assert!(format!("{}", RenewError::TokenRejected).contains("rejected"));
    assert!(format!("{}", RenewError::RealmUnavailable).contains("unavailable"));
    assert!(format!("{}", RenewError::InvalidRequest("bad".into())).contains("bad"));
}
