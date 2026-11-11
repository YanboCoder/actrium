//! Shared server state for the mock loran.
//!
//! State is intentionally in-memory: the mock is rebuilt every test run and
//! persistence would only add brittleness.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use actr_protocol::{ActrId, ActrType, ServiceSpec, SignalingEnvelope};
use axum::extract::ws;
use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

/// One registered actor/session in the signaling registry.
#[derive(Clone, Debug)]
pub struct RegisteredActor {
    pub actr_id: ActrId,
    pub actr_type: ActrType,
    pub client_id: String,
    pub ws_address: Option<String>,
    pub service_spec: Option<ServiceSpec>,
}

/// Realm entry (seeded via `POST /admin/realms` or lazily by the signaling
/// `RegisterRequest`).
#[derive(Clone, Debug)]
pub struct RealmEntry {
    /// Realm name, `<realm_id>@<domain>`.
    pub realm: String,
}

/// Publisher entry — exposed via `GET /registry/publishers/{name}/verifying-key`.
#[derive(Clone, Debug)]
pub struct PublisherEntry {
    pub name: String,
    pub verifying_key: VerifyingKey,
    pub contact: Option<String>,
}

/// All shared state owned by the server.
pub struct MockState {
    // --- Signing material -------------------------------------------------
    issuer_signing_key: SigningKey,
    issuer_signing_key_id: u32,
    builtin_publisher_signing_key: SigningKey,

    /// Cancellation token that signaling connections watch in their recv
    /// loop: when cancelled, each open WS breaks out and drops the stream so
    /// the peer sees a close. Mirrors the old tokio-tungstenite mock's
    /// behavior where shutting down the server forcibly disconnected every
    /// connected actor.
    pub cancel: CancellationToken,

    // --- Signaling registry -----------------------------------------------
    pub clients: RwLock<HashMap<String, mpsc::UnboundedSender<ws::Message>>>,
    pub registry: RwLock<Vec<RegisteredActor>>,
    pub client_to_actr_id: RwLock<HashMap<String, ActrId>>,
    pub next_serial: AtomicU64,

    // --- Issuer/Publisher Registry registry -------------------------------------------------
    pub realms: RwLock<HashMap<String, RealmEntry>>,
    pub publishers: RwLock<HashMap<String, PublisherEntry>>,

    // --- Test observability -----------------------------------------------
    pub message_count: Arc<AtomicU32>,
    pub ice_restart_offer_count: Arc<AtomicU32>,
    pub ice_restart_request_count: Arc<AtomicU32>,
    pub ice_restart_request_drop_count: Arc<AtomicU32>,
    pub pause_forwarding: Arc<AtomicBool>,
    pub ice_candidate_drop_count: Arc<AtomicU32>,
    pub offer_drop_count: Arc<AtomicU32>,
    pub ice_candidate_delay_until: Arc<StdMutex<Option<Instant>>>,
    pub ice_candidate_delay_applied_count: Arc<AtomicU32>,
    pub blackhole_websocket_generation: Arc<AtomicU32>,
    pub websocket_generation: Arc<AtomicU32>,
    pub connection_count: Arc<AtomicU32>,
    pub disconnection_count: Arc<AtomicU32>,
    pub received_messages: Mutex<Vec<SignalingEnvelope>>,
    pub relay_enrollment_unavailable: AtomicBool,
}

impl MockState {
    pub fn new(
        issuer_signing_key: SigningKey,
        issuer_signing_key_id: u32,
        builtin_publisher_signing_key: SigningKey,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            issuer_signing_key,
            issuer_signing_key_id,
            builtin_publisher_signing_key,
            cancel,
            clients: RwLock::new(HashMap::new()),
            registry: RwLock::new(Vec::new()),
            client_to_actr_id: RwLock::new(HashMap::new()),
            next_serial: AtomicU64::new(1),
            realms: RwLock::new(HashMap::new()),
            publishers: RwLock::new(HashMap::new()),
            message_count: Arc::new(AtomicU32::new(0)),
            ice_restart_offer_count: Arc::new(AtomicU32::new(0)),
            ice_restart_request_count: Arc::new(AtomicU32::new(0)),
            ice_restart_request_drop_count: Arc::new(AtomicU32::new(0)),
            pause_forwarding: Arc::new(AtomicBool::new(false)),
            ice_candidate_drop_count: Arc::new(AtomicU32::new(0)),
            offer_drop_count: Arc::new(AtomicU32::new(0)),
            ice_candidate_delay_until: Arc::new(StdMutex::new(None)),
            ice_candidate_delay_applied_count: Arc::new(AtomicU32::new(0)),
            blackhole_websocket_generation: Arc::new(AtomicU32::new(0)),
            websocket_generation: Arc::new(AtomicU32::new(0)),
            connection_count: Arc::new(AtomicU32::new(0)),
            disconnection_count: Arc::new(AtomicU32::new(0)),
            received_messages: Mutex::new(Vec::new()),
            relay_enrollment_unavailable: AtomicBool::new(false),
        }
    }

    pub fn issuer_signing_key(&self) -> &SigningKey {
        &self.issuer_signing_key
    }

    pub fn issuer_signing_key_id(&self) -> u32 {
        self.issuer_signing_key_id
    }

    pub fn builtin_publisher_signing_key(&self) -> &SigningKey {
        &self.builtin_publisher_signing_key
    }

    pub async fn add_realm(&self, realm: String) {
        self.realms
            .write()
            .await
            .insert(realm.clone(), RealmEntry { realm });
    }

    pub async fn add_publisher(&self, name: String, verifying_key: VerifyingKey) {
        self.publishers.write().await.insert(
            name.clone(),
            PublisherEntry {
                name,
                verifying_key,
                contact: None,
            },
        );
    }

    /// Look up a publisher's verifying key, falling back to the built-in
    /// Publisher Registry key when no explicit entry exists. This makes the mock forgiving
    /// for tests that never register an Publisher Registry.
    pub async fn publisher_verifying_key(&self, name: &str) -> Option<VerifyingKey> {
        if let Some(record) = self.publishers.read().await.get(name) {
            return Some(record.verifying_key);
        }
        None
    }
}
