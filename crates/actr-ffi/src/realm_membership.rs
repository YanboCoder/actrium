//! Device membership and the OS signing boundary. Invites and private keys
//! are never written to configuration or included in diagnostic errors.

use actr_hyper::realm_membership::{MembershipClientError, MembershipEndpoint};
use actr_hyper::{Attached, Node, RealmClientKey, RealmKeySigner};
use std::sync::Arc;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RealmMembershipError {
    #[error("invalid membership input: {reason}")]
    InvalidInput { reason: String },
    #[error("membership refused ({status}): {code}")]
    Rejected { status: u16, code: String },
    #[error("result unknown; keep the key and retry the same invite")]
    Uncertain,
    #[error("Realm client key unavailable")]
    KeyUnavailable,
}

impl From<MembershipClientError> for RealmMembershipError {
    fn from(error: MembershipClientError) -> Self {
        match error {
            MembershipClientError::InvalidInput(message) => Self::InvalidInput { reason: message },
            MembershipClientError::Rejected { status, code } => Self::Rejected { status, code },
            MembershipClientError::Uncertain => Self::Uncertain,
            MembershipClientError::KeyUnavailable => Self::KeyUnavailable,
        }
    }
}

/// Retain the private key in platform storage. The platform implementation
/// signs JCS payloads and returns a raw Ed25519 signature (64 bytes).
#[uniffi::export(callback_interface)]
pub trait RealmClientSigner: Send + Sync {
    fn public_key(&self) -> Result<Vec<u8>, RealmMembershipError>;
    fn sign(&self, payload: Vec<u8>) -> Result<Vec<u8>, RealmMembershipError>;
}

#[derive(uniffi::Record)]
pub struct RealmRuntimeConfiguration {
    pub realm: String,
    pub issuer: String,
}

/// Read public endpoint settings for an existing Host config. This does not
/// discover or establish trust in a Realm or its provider.
#[uniffi::export]
pub fn read_realm_runtime_configuration(
    config_path: String,
) -> Result<RealmRuntimeConfiguration, RealmMembershipError> {
    let invalid = || RealmMembershipError::InvalidInput {
        reason: "runtime must specify Realm and Issuer without a client key file".into(),
    };
    let config = actr_config::RuntimeRawConfig::from_file(config_path).map_err(|_| invalid())?;
    if config.deployment.realm_client_key.is_some() {
        return Err(invalid());
    }
    Ok(RealmRuntimeConfiguration {
        realm: config.deployment.realm.ok_or_else(invalid)?,
        issuer: config.issuer_endpoint.url.ok_or_else(invalid)?,
    })
}

struct PlatformSigner {
    public_key: [u8; 32],
    callback: Box<dyn RealmClientSigner>,
}

impl RealmKeySigner for PlatformSigner {
    fn public_key(&self) -> [u8; 32] {
        self.public_key
    }
    fn sign_payload(&self, payload: &[u8]) -> Result<[u8; 64], actr_hyper::HyperError> {
        self.callback
            .sign(payload.to_vec())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| actr_hyper::HyperError::Config("platform signer unavailable".into()))
    }
}

#[derive(uniffi::Object)]
pub struct RealmMembership {
    inner: actr_hyper::realm_membership::RealmMembershipClient,
}

#[uniffi::export(async_runtime = "tokio")]
impl RealmMembership {
    /// Supply endpoints from an accepted Realm Profile, or explicitly chosen
    /// development configuration. This constructor does not establish domain trust.
    #[uniffi::constructor]
    pub fn new(
        realm: String,
        provider_id: String,
        issuer: String,
        signer: Box<dyn RealmClientSigner>,
        allow_insecure_development: bool,
    ) -> Result<Arc<Self>, RealmMembershipError> {
        let public_key = signer
            .public_key()
            .map_err(|_| RealmMembershipError::KeyUnavailable)?
            .try_into()
            .map_err(|_| RealmMembershipError::KeyUnavailable)?;
        let key = RealmClientKey::from_signer(
            realm.clone(),
            provider_id.clone(),
            Arc::new(PlatformSigner {
                public_key,
                callback: signer,
            }),
        );
        let inner = actr_hyper::realm_membership::RealmMembershipClient::new(
            MembershipEndpoint {
                realm,
                provider_id,
                issuer,
            },
            key,
            allow_insecure_development,
        )
        .map_err(|_| RealmMembershipError::InvalidInput {
            reason: "invalid Realm or Issuer configuration".into(),
        })?;
        Ok(Arc::new(Self { inner }))
    }

    pub fn key_id(&self) -> String {
        self.inner.key_id()
    }

    /// JSON uses the existing membership wire format. Errors retain its
    /// stable codes without exposing a server/gateway response body.
    pub async fn join(
        &self,
        invite: String,
        label: String,
    ) -> Result<String, RealmMembershipError> {
        Ok(self.inner.join(&invite, &label).await?.to_string())
    }
    pub async fn members(&self) -> Result<String, RealmMembershipError> {
        Ok(self.inner.members().await?.to_string())
    }
    pub async fn invite_client(
        &self,
        member_id: String,
        expires_at: String,
    ) -> Result<String, RealmMembershipError> {
        Ok(self
            .inner
            .invite_client(&member_id, &expires_at)
            .await?
            .to_string())
    }
    pub async fn revoke_client(&self, key_id: String) -> Result<String, RealmMembershipError> {
        Ok(self.inner.revoke_client(&key_id).await?.to_string())
    }
    pub async fn revoke_invite(&self, invite_id: String) -> Result<String, RealmMembershipError> {
        Ok(self.inner.revoke_invite(&invite_id).await?.to_string())
    }
}

impl RealmMembership {
    pub(crate) fn bind(&self, node: Node<Attached>) -> crate::ActrResult<Node<Attached>> {
        if node.issuer_endpoint().trim_end_matches('/')
            != self.inner.endpoint().issuer.trim_end_matches('/')
        {
            return Err(crate::ActrError::Config {
                msg: "membership Issuer differs from runtime configuration".into(),
            });
        }
        node.with_realm_client_key(self.inner.client_key())
            .map_err(|_| crate::ActrError::Config {
                msg: "membership does not match runtime configuration".into(),
            })
    }
}

/// Software Ed25519 fallback for platforms whose secure hardware only
/// supports other curves. Callers must wrap this seed before persisting it.
#[uniffi::export]
pub fn realm_key_generate_seed() -> Vec<u8> {
    use rand::RngCore;
    let mut seed = vec![0; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    seed
}

fn key_from_seed(seed: Vec<u8>) -> Result<ed25519_dalek::SigningKey, RealmMembershipError> {
    let seed = zeroize::Zeroizing::new(seed);
    let bytes: &[u8; 32] = seed
        .as_slice()
        .try_into()
        .map_err(|_| RealmMembershipError::KeyUnavailable)?;
    Ok(ed25519_dalek::SigningKey::from_bytes(bytes))
}

#[uniffi::export]
pub fn realm_key_public(seed: Vec<u8>) -> Result<Vec<u8>, RealmMembershipError> {
    Ok(key_from_seed(seed)?.verifying_key().to_bytes().to_vec())
}

#[uniffi::export]
pub fn realm_key_sign(seed: Vec<u8>, payload: Vec<u8>) -> Result<Vec<u8>, RealmMembershipError> {
    use ed25519_dalek::Signer;
    Ok(key_from_seed(seed)?.sign(&payload).to_bytes().to_vec())
}
