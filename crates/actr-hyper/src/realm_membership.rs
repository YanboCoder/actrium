//! Device membership for an already trusted Realm. Discovery and first-domain
//! trust remain the caller's responsibility. Keys are supplied by the platform
//! signer and must be durably stored before redeeming an invite.

use crate::error::{HyperError, HyperResult};
use crate::issuer_client::RealmClientKey;
use actr_protocol::realm_membership::{MembershipOp, MembershipRequest, RealmInvite};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum MembershipClientError {
    #[error("invalid membership input: {0}")]
    InvalidInput(String),
    #[error("membership refused ({status}): {code}")]
    Rejected { status: u16, code: String },
    /// Keep the same key and retry the same invite. The server may already
    /// have consumed it; a new key would lose the idempotent redemption.
    #[error("membership result unknown; retain the key and retry the same invite")]
    Uncertain,
    #[error("the Realm client key is unavailable")]
    KeyUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RealmKeySigner;
    use std::sync::Arc;

    fn client(url: &str, allow_http: bool) -> HyperResult<RealmMembershipClient> {
        RealmMembershipClient::new(
            MembershipEndpoint {
                realm: "r@example.com".into(),
                provider_id: "provider.example".into(),
                issuer: url.into(),
            },
            RealmClientKey::new(
                "r@example.com",
                "provider.example",
                ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
            ),
            allow_http,
        )
    }

    #[test]
    fn trusted_endpoints_require_https_and_no_embedded_credentials() {
        for url in [
            "http://issuer.example",
            "https://user:pass@issuer.example",
            "https://issuer.example?secret=value",
            "https://issuer.example#fragment",
        ] {
            assert!(client(url, false).is_err());
        }
        assert!(client("https://issuer.example", false).is_ok());
        assert!(client("http://127.0.0.1:8080", true).is_ok());
    }

    #[tokio::test]
    async fn redirects_and_unrecognized_errors_are_uncertain_and_redacted() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&server.url(), true).unwrap();
        let redirect = server
            .mock("POST", "/membership")
            .with_status(307)
            .with_header("location", "/leak")
            .with_body("sensitive server echo")
            .create_async()
            .await;
        let leaked = server.mock("POST", "/leak").expect(0).create_async().await;
        let error = client.members().await.unwrap_err();
        assert!(matches!(error, MembershipClientError::Uncertain));
        assert!(!error.to_string().contains("sensitive"));
        redirect.assert_async().await;
        leaked.assert_async().await;
    }

    struct BrokenSigner;
    impl RealmKeySigner for BrokenSigner {
        fn public_key(&self) -> [u8; 32] {
            ed25519_dalek::SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .to_bytes()
        }
        fn sign_payload(&self, _: &[u8]) -> Result<[u8; 64], HyperError> {
            Ok([0; 64])
        }
    }

    #[tokio::test]
    async fn invalid_platform_signatures_never_reach_the_issuer() {
        let mut server = mockito::Server::new_async().await;
        let no_request = server
            .mock("POST", "/membership")
            .expect(0)
            .create_async()
            .await;
        let mut client = client(&server.url(), true).unwrap();
        client.key = RealmClientKey::from_signer(
            "r@example.com".into(),
            "provider.example".into(),
            Arc::new(BrokenSigner),
        );
        assert!(matches!(
            client.members().await,
            Err(MembershipClientError::KeyUnavailable)
        ));
        no_request.assert_async().await;
    }
}

/// No secret fields: safe to persist as ordinary configuration after the
/// caller accepts the Realm Profile or explicitly configures a development Realm.
#[derive(Clone, Debug)]
pub struct MembershipEndpoint {
    pub realm: String,
    pub provider_id: String,
    pub issuer: String,
}

pub struct RealmMembershipClient {
    endpoint: MembershipEndpoint,
    key: RealmClientKey,
    http: reqwest::Client,
}

impl RealmMembershipClient {
    pub fn new(
        endpoint: MembershipEndpoint,
        key: RealmClientKey,
        allow_insecure_development: bool,
    ) -> HyperResult<Self> {
        actr_protocol::uri::parse_realm_name(&endpoint.realm)
            .map_err(|_| HyperError::Config("invalid Realm name".into()))?;
        let url = reqwest::Url::parse(&endpoint.issuer)
            .map_err(|_| HyperError::Config("invalid Issuer URL".into()))?;
        if key.realm() != endpoint.realm || key.provider_id() != endpoint.provider_id {
            return Err(HyperError::Config(
                "membership key belongs to another Realm or provider".into(),
            ));
        }
        if url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https" || (allow_insecure_development && url.scheme() == "http"))
        {
            return Err(HyperError::Config(
                "Issuer requires HTTPS without credentials, query or fragment".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| HyperError::Config("cannot create membership HTTP client".into()))?;
        Ok(Self {
            endpoint,
            key,
            http,
        })
    }

    pub fn endpoint(&self) -> &MembershipEndpoint {
        &self.endpoint
    }
    pub fn client_key(&self) -> RealmClientKey {
        self.key.clone()
    }
    pub fn key_id(&self) -> String {
        self.key.key_id()
    }

    fn request(&self, op: MembershipOp) -> MembershipRequest {
        let mut request = MembershipRequest::new(
            op,
            &self.endpoint.realm,
            &self.endpoint.provider_id,
            &uuid::Uuid::new_v4().simple().to_string(),
        );
        (request.issued_at, request.expires_at) = crate::issuer_client::client_proof_window();
        request
    }

    /// Retries use this same signer and a fresh envelope nonce. The invite
    /// is never logged, included in an error, or persisted by this client.
    pub async fn join(&self, invite: &str, label: &str) -> Result<Value, MembershipClientError> {
        let invite = RealmInvite::decode(invite)
            .map_err(|_| MembershipClientError::InvalidInput("invalid invite".into()))?;
        if invite.realm != self.endpoint.realm {
            return Err(MembershipClientError::Rejected {
                status: 409,
                code: "invite_realm_mismatch".into(),
            });
        }
        if label.chars().count() > actr_protocol::realm_membership::MAX_LABEL_CHARS {
            return Err(MembershipClientError::InvalidInput(
                "client label is too long".into(),
            ));
        }
        let mut request = self.request(MembershipOp::Join);
        request.invite = Some(invite.secret);
        request.public_key = Some(BASE64.encode(self.key.public_key()));
        request.label = Some(label.to_string());
        let result = self.send(request).await?;
        if result["realm"] != self.endpoint.realm
            || result["key_id"] != self.key_id()
            || result["member_id"].as_str().is_none()
            || !matches!(result["role"].as_str(), Some("owner" | "editor" | "viewer"))
        {
            return Err(MembershipClientError::Uncertain);
        }
        Ok(result)
    }

    /// Lists visible members and their clients; the Issuer enforces visibility.
    pub async fn members(&self) -> Result<Value, MembershipClientError> {
        self.send(self.request(MembershipOp::ListMembers)).await
    }

    /// Ask the Issuer for a one-time invitation to add a client of this member.
    pub async fn invite_client(
        &self,
        member_id: &str,
        expires_at: &str,
    ) -> Result<Value, MembershipClientError> {
        let mut request = self.request(MembershipOp::CreateInvite);
        request.member_id = Some(member_id.to_string());
        request.invite_expires_at = Some(expires_at.to_string());
        self.send(request).await
    }

    pub async fn revoke_client(&self, key_id: &str) -> Result<Value, MembershipClientError> {
        let mut request = self.request(MembershipOp::RevokeClient);
        request.key_id = Some(key_id.to_string());
        self.send(request).await
    }

    pub async fn revoke_invite(&self, invite_id: &str) -> Result<Value, MembershipClientError> {
        let mut request = self.request(MembershipOp::RevokeInvite);
        request.invite_id = Some(invite_id.to_string());
        self.send(request).await
    }

    async fn send(&self, request: MembershipRequest) -> Result<Value, MembershipClientError> {
        let document = self
            .key
            .sign_document(
                serde_json::to_value(request)
                    .map_err(|_| MembershipClientError::InvalidInput("invalid request".into()))?,
            )
            .map_err(|_| MembershipClientError::KeyUnavailable)?;
        let mut response = self
            .http
            .post(format!(
                "{}/membership",
                self.endpoint.issuer.trim_end_matches('/')
            ))
            .header(reqwest::header::CACHE_CONTROL, "no-store")
            .json(&document)
            .send()
            .await
            .map_err(|_| MembershipClientError::Uncertain)?;
        let status = response.status().as_u16();
        // A proxy error may follow a committed redemption. Do not echo its
        // body (which can contain the invite) or infer a definitive refusal.
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| MembershipClientError::Uncertain)?
        {
            if body.len() + chunk.len() > 1024 * 1024 {
                return Err(MembershipClientError::Uncertain);
            }
            body.extend_from_slice(&chunk);
        }
        let reply: Value =
            serde_json::from_slice(&body).map_err(|_| MembershipClientError::Uncertain)?;
        if (200..300).contains(&status) {
            return Ok(reply);
        }
        let code = reply["error"].as_str().unwrap_or_default();
        let definitive = matches!(
            (status, code),
            (400, "invalid_request")
                | (
                    403,
                    "client_unknown"
                        | "client_revoked"
                        | "member_removed"
                        | "forbidden"
                        | "realm_unavailable"
                )
                | (404, "invite_unknown" | "not_found")
                | (
                    409,
                    "invite_realm_mismatch"
                        | "owner_last_client"
                        | "owner_must_transfer"
                        | "conflict"
                )
                | (410, "invite_expired" | "invite_used" | "invite_revoked")
                | (429, "quota")
        );
        if definitive {
            Err(MembershipClientError::Rejected {
                status,
                code: code.into(),
            })
        } else {
            Err(MembershipClientError::Uncertain)
        }
    }
}
