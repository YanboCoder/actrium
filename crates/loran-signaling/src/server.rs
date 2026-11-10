//! Loran Signaling Server - based on protobuf SignalingEnvelope
//!
//! Fully based on the protobuf protocol, using WebSocket Binary message transport
//!
//! # Feature Overview
//!
//! ## Core Features Implemented
//!
//! ### Basic Signaling Flow
//! - ✅ Actor unregistration (`UnregisterRequest`)
//! - ✅ Heartbeat mechanism (`Ping` / `Pong`)
//! - ✅ WebRTC signaling relay (`ActrRelay` - ICE / SDP)
//!
//! ### Extended Features
//! - ✅ Service discovery (`DiscoveryRequest` / `DiscoveryResponse`)
//! - ✅ Load-balanced routing (`RouteCandidatesRequest` / `RouteCandidatesResponse`)
//!   - Multi-factor ranking: power reserve, mailbox backlog, geographic distance, client stickiness
//!   - Exact match priority strategy
//! - ✅ Presence subscription (`SubscribeActrUpRequest` / `ActrUpEvent`)
//! - ❌ Credential refresh has been migrated to Issuer HTTP
//! - ✅ Load metric storage (`handle_ping()` - stored in ActorRegistry for load balancing)
//!
//! ## Pending Features (optional enhancements)
//!
//! 1. **Credential validation** (optional security enhancement)
//!    - `handle_actr_to_server()` - validate credential in Actor messages
//!    - `handle_actr_relay()` - validate credential in relay messages
//!
//! 2. **ServiceSpec and ACL persistence** (optional access control)
//!    - `handle_register_request()` - persist service specs and access control rules
//!    - Used for fine-grained inter-service access control

use actr_protocol::{
    AIdCredential, ActrId, ActrRelay, ActrToSignaling, ActrType, ErrorResponse, IdentityClaims,
    PeerToSignaling, Ping, Pong, RegisterResponse, RelayDeliveryContext, RelayDeliveryResult,
    RelayDeliveryStatus, RoleAssignment, RoleNegotiation, SIGNALING_ENVELOPE_VERSION,
    SignalingEnvelope, SignalingToActr, actr_relay, actr_to_signaling, peer_to_signaling,
    register_response, signaling_envelope, signaling_to_actr,
};
use futures_util::{SinkExt, StreamExt};
use platform::aid::credential::validator::AIdCredentialValidator;
use platform::config::signaling::EgressQueueConfig;
use platform::realm::{Realm as RealmEntity, RealmStatus};
use prost::Message as ProstMessage;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::time::timeout;
use tracing::info_span;
use uuid::Uuid;

// Axum WebSocket
use axum::extract::ws::{Message as WsMessage, WebSocket};

use crate::actor_registry::ActorRegistry;
use crate::load_balancer::LoadBalancer;
use crate::presence::PresenceManager;
#[cfg(feature = "opentelemetry")]
use crate::trace::{extract_trace_context, inject_trace_context};
use tracing::Instrument;
#[cfg(feature = "opentelemetry")]
use tracing::instrument;

fn type_key(actor_type: &ActrType) -> String {
    actor_type.to_string_repr()
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Per-client egress queue bound (issue #351)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Signaling queue observability counters (no credentials or payloads).
///
/// These counters are incremented atomically on the hot path to expose queue
/// depth/bytes, saturation events, slow-consumer disconnects, oversized-frame
#[derive(Debug, Default)]
pub struct SignalingQueueMetrics {
    /// Total data messages successfully enqueued
    pub egress_enqueued: AtomicU64,
    /// Number of times enqueue exceeded its deadline (queue full)
    pub egress_saturation_events: AtomicU64,
    /// Connections actively disconnected by the server for slow consumption
    pub slow_consumer_disconnects: AtomicU64,
    /// Inbound frames rejected for exceeding the max frame size
    pub oversized_frame_rejections: AtomicU64,
    /// Messages dropped for exceeding the per-target aggregate rate/byte budget
    pub target_budget_rejections: AtomicU64,
    /// Control frames dropped because the control queue was full
    pub control_dropped: AtomicU64,
}

impl SignalingQueueMetrics {
    /// Current snapshot (for tests and metrics export)
    pub fn snapshot(&self) -> SignalingQueueMetricsSnapshot {
        SignalingQueueMetricsSnapshot {
            egress_enqueued: self.egress_enqueued.load(Ordering::Relaxed),
            egress_saturation_events: self.egress_saturation_events.load(Ordering::Relaxed),
            slow_consumer_disconnects: self.slow_consumer_disconnects.load(Ordering::Relaxed),
            oversized_frame_rejections: self.oversized_frame_rejections.load(Ordering::Relaxed),
            target_budget_rejections: self.target_budget_rejections.load(Ordering::Relaxed),
            control_dropped: self.control_dropped.load(Ordering::Relaxed),
        }
    }
}

/// Instantaneous snapshot of [`SignalingQueueMetrics`]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SignalingQueueMetricsSnapshot {
    pub egress_enqueued: u64,
    pub egress_saturation_events: u64,
    pub slow_consumer_disconnects: u64,
    pub oversized_frame_rejections: u64,
    pub target_budget_rejections: u64,
    pub control_dropped: u64,
}

/// Why a data message could not be enqueued to a client egress queue
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnqueueError {
    /// The queue stayed full past the deadline (slow consumer); the caller should close and remove the client
    Saturated,
    /// The client queue is closed (the connection is being torn down)
    Closed,
}

/// A queued data frame and its byte-budget permit.
///
/// The permit is returned to the byte budget when this value is dropped (on send
/// completion, or on connection teardown/reset), so teardown never leaks a byte reservation.
pub(crate) struct QueuedFrame {
    msg: WsMessage,
    byte_len: usize,
    _permit: OwnedSemaphorePermit,
}

/// Per-client bounded egress handle (issue #351).
///
/// Data (Binary) and control (Ping/Pong/Close) messages use two independent bounded queues,
/// and the write task drains the control queue first so control messages are not starved by data. The data queue is bounded by both
/// message count and encoded byte count.
#[derive(Debug, Clone)]
pub struct ClientEgress {
    /// Data (Binary) queue sender (bounded by message count)
    data_tx: tokio::sync::mpsc::Sender<QueuedFrame>,
    /// Control (Ping/Pong/Close) queue sender (bounded, small capacity)
    control_tx: tokio::sync::mpsc::Sender<WsMessage>,
    /// Data queue byte budget
    byte_budget: Arc<Semaphore>,
    /// Cancelled when the server closes the connection but could not queue
    /// a Close frame; the write task then ends the connection at once.
    closed: tokio_util::sync::CancellationToken,
    /// The bound configuration applied to this client
    config: EgressQueueConfig,
    /// Shared observability counters
    metrics: Arc<SignalingQueueMetrics>,
}

impl ClientEgress {
    /// Build a bounded egress handle and its two consumers for a new connection
    pub(crate) fn new(
        config: EgressQueueConfig,
        metrics: Arc<SignalingQueueMetrics>,
    ) -> (
        Self,
        tokio::sync::mpsc::Receiver<QueuedFrame>,
        tokio::sync::mpsc::Receiver<WsMessage>,
    ) {
        let (data_tx, data_rx) = tokio::sync::mpsc::channel(config.max_messages.max(1) as usize);
        let (control_tx, control_rx) =
            tokio::sync::mpsc::channel(config.max_control_messages.max(1) as usize);
        let egress = Self {
            data_tx,
            control_tx,
            byte_budget: Arc::new(Semaphore::new(config.max_bytes as usize)),
            closed: tokio_util::sync::CancellationToken::new(),
            config,
            metrics,
        };
        (egress, data_rx, control_rx)
    }

    /// Reserve byte budget and a message slot, then enqueue the data frame.
    ///
    /// Returns `Saturated` if capacity is not available within `enqueue_timeout` (the caller closes the slow-consumer
    /// client); returns `Closed` if the channel is closed. The caller is never blocked while holding a lock.
    async fn send_data(&self, msg: WsMessage, byte_len: usize) -> Result<(), EnqueueError> {
        let enqueue_timeout = Duration::from_millis(self.config.enqueue_timeout_ms as u64);
        let reserve = async {
            // byte_len is bounded by max_frame_bytes (<= max_bytes), so it always fits in a u32 permit count
            let permits = u32::try_from(byte_len).unwrap_or(u32::MAX);
            let permit = self
                .byte_budget
                .clone()
                .acquire_many_owned(permits)
                .await
                .map_err(|_| EnqueueError::Closed)?;
            self.data_tx
                .send(QueuedFrame {
                    msg,
                    byte_len,
                    _permit: permit,
                })
                .await
                .map_err(|_| EnqueueError::Closed)
        };

        match timeout(enqueue_timeout, reserve).await {
            Ok(result) => {
                if result.is_ok() {
                    self.metrics.egress_enqueued.fetch_add(1, Ordering::Relaxed);
                }
                result
            }
            Err(_) => {
                self.metrics
                    .egress_saturation_events
                    .fetch_add(1, Ordering::Relaxed);
                Err(EnqueueError::Saturated)
            }
        }
    }

    /// Enqueue a control message (Ping/Pong/Close) without blocking.
    ///
    /// The control queue is independent and bounded; if full the message is dropped and counted (the control write task has a write deadline,
    /// so a slow-consuming connection is eventually torn down by the write timeout).
    fn try_send_control(
        &self,
        msg: WsMessage,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<WsMessage>> {
        self.control_tx.try_send(msg).inspect_err(|_| {
            self.metrics.control_dropped.fetch_add(1, Ordering::Relaxed);
        })
    }

    /// Close the connection: queue a Close frame, or, when the control queue
    /// is full or closed, have the write task end the connection without one.
    /// Returns whether the Close frame was queued.
    fn close(&self, close_frame: axum::extract::ws::CloseFrame) -> bool {
        let queued = self
            .try_send_control(WsMessage::Close(Some(close_frame)))
            .is_ok();
        if !queued {
            self.closed.cancel();
        }
        queued
    }

    /// Current number of queued data messages (observability gauge)
    #[allow(dead_code)]
    fn depth(&self) -> usize {
        self.data_tx.max_capacity() - self.data_tx.capacity()
    }

    /// Current number of queued data bytes (observability gauge)
    #[allow(dead_code)]
    fn queued_bytes(&self) -> usize {
        (self.config.max_bytes as usize).saturating_sub(self.byte_budget.available_permits())
    }
}

/// Signaling server state
#[derive(Debug)]
pub struct SignalingServer {
    /// Connected clients
    pub clients: Arc<RwLock<HashMap<String, ClientConnection>>>,
    /// Index for looking up client_id by ActorId
    pub actor_id_index: Arc<RwLock<HashMap<ActrId, String>>>,
    /// Actor registry
    pub actor_registry: Arc<RwLock<ActorRegistry>>,
    /// Presence subscription manager
    pub presence_manager: Arc<RwLock<PresenceManager>>,
    /// Connection rate limiter
    pub connection_rate_limiter: Option<Arc<crate::ratelimit::ConnectionRateLimiter>>,
    /// Message rate limiter
    pub message_rate_limiter: Option<Arc<crate::ratelimit::MessageRateLimiter>>,
    /// Per-client egress queue bound configuration (issue #351)
    pub egress_config: EgressQueueConfig,
    /// Per-target aggregate rate/byte limiter (issue #351)
    pub target_rate_limiter: Arc<crate::ratelimit::TargetRateLimiter>,
    /// Signaling queue observability counters (issue #351)
    pub queue_metrics: Arc<SignalingQueueMetrics>,
    /// Actr Facility Provider ID that Actor credentials must carry
    pub provider_id: String,
}

/// Client connection info
#[derive(Debug)]
pub struct ClientConnection {
    pub id: String,
    pub actor_id: Option<ActrId>,
    pub credential: Option<AIdCredential>,
    /// Bounded egress handle (data + control dual queues, issue #351)
    pub egress: ClientEgress,
    pub client_ip: Option<std::net::IpAddr>,
    /// WebRTC role: "answer" or None (defaults to offer)
    pub webrtc_role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupMode {
    ConnectionClosed,
    ExplicitUnregister,
}

/// Maximum number of route candidates encoded for any wire request.
const MAX_ROUTE_CANDIDATES: u32 = 64;

struct StaleClient {
    id: String,
    egress: ClientEgress,
}

/// Signaling server handle - used to operate the server in async tasks
#[derive(Debug, Clone)]
pub struct SignalingServerHandle {
    pub clients: Arc<RwLock<HashMap<String, ClientConnection>>>,
    pub actor_id_index: Arc<RwLock<HashMap<ActrId, String>>>,
    pub actor_registry: Arc<RwLock<ActorRegistry>>,
    pub presence_manager: Arc<RwLock<PresenceManager>>,
    pub connection_rate_limiter: Option<Arc<crate::ratelimit::ConnectionRateLimiter>>,
    pub message_rate_limiter: Option<Arc<crate::ratelimit::MessageRateLimiter>>,
    /// Per-client egress queue bound configuration (issue #351)
    pub egress_config: EgressQueueConfig,
    /// Per-target aggregate rate/byte limiter (issue #351)
    pub target_rate_limiter: Arc<crate::ratelimit::TargetRateLimiter>,
    /// Signaling queue observability counters (issue #351)
    pub queue_metrics: Arc<SignalingQueueMetrics>,
    /// Actr Facility Provider ID that Actor credentials must carry
    pub provider_id: String,
}
impl SignalingServerHandle {
    /// Create a SignalingEnvelope
    #[cfg_attr(
        feature = "opentelemetry",
        instrument(level = "debug", skip_all, fields(reply_for))
    )]
    fn create_envelope(
        &self,
        flow: signaling_envelope::Flow,
        reply_for: Option<&str>,
    ) -> SignalingEnvelope {
        #[allow(unused_mut)]
        let mut envelope = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: Uuid::new_v4().to_string(),
            reply_for: reply_for.map(|id| id.to_string()),
            traceparent: None,
            tracestate: None,
            flow: Some(flow),
        };
        platform::recording::debug!(
            "Created envelope: envelope_id={}, reply_for={reply_for:?}",
            envelope.envelope_id,
        );
        envelope
    }

    #[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all))]
    fn create_new_envelope(&self, flow: signaling_envelope::Flow) -> SignalingEnvelope {
        self.create_envelope(flow, None)
    }
}

impl Default for SignalingServer {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalingServer {
    pub fn new() -> Self {
        Self::with_egress_config(EgressQueueConfig::default())
    }

    /// Create a server with the given egress queue bound configuration (issue #351)
    pub fn with_egress_config(egress_config: EgressQueueConfig) -> Self {
        let target_rate_limiter = Arc::new(crate::ratelimit::TargetRateLimiter::new(
            egress_config.target_rate_per_second,
            egress_config.target_bytes_per_second,
        ));
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            actor_id_index: Arc::new(RwLock::new(HashMap::new())),
            actor_registry: Arc::new(RwLock::new(ActorRegistry::new())),
            presence_manager: Arc::new(RwLock::new(PresenceManager::new())),
            connection_rate_limiter: None, // Initialized in axum_router based on config
            message_rate_limiter: None,    // Initialized in axum_router based on config
            egress_config,
            target_rate_limiter,
            queue_metrics: Arc::new(SignalingQueueMetrics::default()),
            provider_id: String::new(),
        }
    }

    /// A handle sharing this server's state
    pub fn handle(&self) -> SignalingServerHandle {
        SignalingServerHandle {
            clients: self.clients.clone(),
            actor_id_index: self.actor_id_index.clone(),
            actor_registry: self.actor_registry.clone(),
            presence_manager: self.presence_manager.clone(),
            connection_rate_limiter: self.connection_rate_limiter.clone(),
            message_rate_limiter: self.message_rate_limiter.clone(),
            egress_config: self.egress_config.clone(),
            target_rate_limiter: self.target_rate_limiter.clone(),
            queue_metrics: self.queue_metrics.clone(),
            provider_id: self.provider_id.clone(),
        }
    }
}

/// Follow Realm state changes and drop the state of every Realm that
/// retires: first for Realms retired before this process started, then as
/// retirements are announced. Likewise drop every Actor disabled by a
/// revoked or replaced Realm client key.
///
/// `cancel` retires this server (a configuration reload builds a new one),
/// but its connections stay open, so the task keeps following until the
/// last of them is gone.
pub async fn follow_realm_retirements(
    server: SignalingServerHandle,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut changes = platform::realm::changes::subscribe();
    let mut actor_changes = platform::realm::changes::subscribe_actors();
    retire_known_retired_realms(&server).await;
    loop {
        let change = tokio::select! {
            _ = no_connections_after(&server, &cancel) => break,
            change = changes.recv() => change,
            actor = actor_changes.recv() => {
                match actor {
                    Ok(actor) => {
                        disable_actor(&server, &actor).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        revalidate_known_actor_connections(&server).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                continue;
            }
        };
        match change {
            Ok(realm) => {
                if is_retired(&realm).await {
                    retire_realm(&server, &realm).await;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                retire_known_retired_realms(&server).await;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Resolves once `cancel` fired and the server has no connections left.
async fn no_connections_after(
    server: &SignalingServerHandle,
    cancel: &tokio_util::sync::CancellationToken,
) {
    cancel.cancelled().await;
    while !server.clients.read().await.is_empty() {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn is_retired(realm: &str) -> bool {
    matches!(RealmEntity::get(realm).await, Ok(Some(r)) if r.status == RealmStatus::Retired)
}

/// Retire every Realm with registry rows or connections here that is retired.
async fn retire_known_retired_realms(server: &SignalingServerHandle) {
    let mut realms = server.actor_registry.read().await.realm_names();
    realms.extend(
        server
            .clients
            .read()
            .await
            .values()
            .filter_map(|client| client.actor_id.as_ref().map(|id| id.realm.clone())),
    );
    realms.sort();
    realms.dedup();
    for realm in realms {
        if is_retired(&realm).await {
            retire_realm(&server.clone(), &realm).await;
        }
    }
}

/// Drop everything a retired Realm holds here: its Actor Registry rows (in
/// memory and in the cache), presence subscriptions and live connections.
///
/// The registry and presence refuse the Realm first, so a connection or
/// registration racing this call either is removed here or is refused and
/// closes itself. Idempotent.
pub async fn retire_realm(server: &SignalingServerHandle, realm: &str) {
    platform::recording::info!("Realm {} retired: dropping its Signaling state", realm);
    let storage = server.actor_registry.write().await.retire_realm(realm);
    if let Some(storage) = storage
        && let Err(e) = storage.delete_realm(realm).await
    {
        platform::recording::warn!("Failed to delete cached services of Realm {}: {}", realm, e);
    }
    server.presence_manager.write().await.retire_realm(realm);

    let connections: Vec<(String, ClientEgress)> = server
        .clients
        .read()
        .await
        .iter()
        .filter(|(_, client)| {
            client
                .actor_id
                .as_ref()
                .is_some_and(|actor| actor.realm == realm)
        })
        .map(|(id, client)| (id.clone(), client.egress.clone()))
        .collect();
    for (client_id, egress) in connections {
        let close_frame = axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: "realm retired".into(),
        };
        if !egress.close(close_frame) {
            platform::recording::warn!(
                "Connection {} control queue full or closed; closing without a Close frame",
                client_id
            );
        }
        cleanup_client(&client_id, server, CleanupMode::ExplicitUnregister).await;
    }
}

/// Recheck every live credential after losing actor-change notifications.
async fn revalidate_known_actor_connections(server: &SignalingServerHandle) {
    let mut actors: Vec<ActrId> = server
        .clients
        .read()
        .await
        .values()
        .filter_map(|client| client.actor_id.clone())
        .collect();
    actors.sort();
    actors.dedup();
    for actor in actors {
        disable_actor(server, &actor).await;
    }
}

/// Close connections whose client key was revoked or replaced. Revalidate
/// each credential so an actor-change notification preserves valid replacements.
pub async fn disable_actor(server: &SignalingServerHandle, actor: &ActrId) {
    let connections: Vec<(String, Option<AIdCredential>)> = server
        .clients
        .read()
        .await
        .iter()
        .filter(|(_, client)| client.actor_id.as_ref() == Some(actor))
        .map(|(id, client)| (id.clone(), client.credential.clone()))
        .collect();
    let mut refused = false;
    for (client_id, credential) in connections {
        if client_credential_refused(actor, credential.as_ref()).await {
            refused = true;
            close_refused_connection(server, &client_id).await;
        }
    }
    if refused
        || platform::realm::membership::actor_disabled(actor)
            .await
            .unwrap_or(false)
    {
        cleanup_refused_actor(server, actor).await;
    }
}

async fn close_refused_connection(server: &SignalingServerHandle, client_id: &str) {
    let connection = server
        .clients
        .read()
        .await
        .get(client_id)
        .map(|client| (client.egress.clone(), client.actor_id.clone()));
    if let Some((egress, actor)) = connection {
        let close_frame = axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: "client revoked or replaced".into(),
        };
        if !egress.close(close_frame) {
            platform::recording::warn!(
                "Connection {} control queue full or closed; closing without a Close frame",
                client_id
            );
        }
        cleanup_client(client_id, server, CleanupMode::ExplicitUnregister).await;
        if let Some(actor) = actor {
            cleanup_refused_actor(server, &actor).await;
        }
    }
}

async fn cleanup_refused_actor(server: &SignalingServerHandle, actor: &ActrId) {
    // Hold this guard through cleanup: a new connection cannot install its
    // routing/presence state between the replacement check and deletion.
    let clients = server.clients.read().await;
    if clients
        .values()
        .any(|client| client.actor_id.as_ref() == Some(actor))
    {
        return;
    }
    if let Err(e) = server
        .actor_registry
        .write()
        .await
        .unregister_actor_durable(actor)
        .await
    {
        platform::recording::warn!(
            "Failed to delete cached services of Actor {}: {}",
            actor.to_string_repr(),
            e
        );
    }
    server.presence_manager.write().await.unsubscribe_all(actor);
}

/// Fail closed if a credential's durable binding changed since URL authentication.
async fn client_credential_refused(actor: &ActrId, credential: Option<&AIdCredential>) -> bool {
    let Some(claims) =
        credential.and_then(|credential| IdentityClaims::decode(credential.claims.as_ref()).ok())
    else {
        return true;
    };
    match platform::realm::membership::credential_refused(actor, &claims.client_key).await {
        Ok(refused) => refused,
        Err(error) => {
            platform::recording::warn!("WebSocket Actor state recheck failed: {error}");
            true
        }
    }
}

/// Handle WebSocket connection
pub async fn handle_websocket_connection(
    websocket: WebSocket,
    server: SignalingServerHandle,
    client_ip: Option<std::net::IpAddr>,
    url_identity: Option<(ActrId, ActrType, AIdCredential)>,
    webrtc_role: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    handle_websocket_connection_inner(
        websocket,
        server,
        client_ip,
        url_identity,
        webrtc_role,
        true,
    )
    .await
}

pub(crate) async fn handle_websocket_connection_inner(
    websocket: WebSocket,
    server: SignalingServerHandle,
    client_ip: Option<std::net::IpAddr>,
    url_identity: Option<(ActrId, ActrType, AIdCredential)>,
    webrtc_role: Option<String>,
    require_client_auth: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let client_id = Uuid::new_v4().to_string();
    platform::recording::info!(
        "🔗 New WebSocket client connection: {} (IP: {:?})",
        client_id,
        client_ip
    );

    // Split read and write streams
    let (mut ws_sender, mut ws_receiver) = websocket.split();

    // Create a bounded egress queue for this connection (data + control dual channels + byte budget, issue #351)
    let (egress, data_rx, control_rx) =
        ClientEgress::new(server.egress_config.clone(), server.queue_metrics.clone());
    let write_timeout = Duration::from_millis(server.egress_config.write_timeout_ms as u64);
    let control_tx = egress.clone();
    let closed = egress.closed.clone();

    // Register the client (including the bounded egress handle)
    let (actor_id, actor_type, credential) = match url_identity {
        Some((actor_id, actor_type, credential)) => {
            (Some(actor_id), Some(actor_type), Some(credential))
        }
        None => (None, None, None),
    };
    let actor_id_for_registry = actor_id.clone().zip(actor_type);
    let connection_credential = credential.clone();
    let connection_realm = actor_id.as_ref().map(|actor| actor.realm.clone());
    let stale_clients = {
        let mut clients_guard = server.clients.write().await;

        // Serialize this check with connection replacement. An old handshake
        // must be rejected before it can remove, reroute, or close a healthy
        // incumbent that connected using the new binding while it was delayed.
        if require_client_auth
            && let Some(actor) = actor_id.as_ref()
            && client_credential_refused(actor, credential.as_ref()).await
        {
            drop(clients_guard);
            let close_frame = axum::extract::ws::CloseFrame {
                code: axum::extract::ws::close_code::POLICY,
                reason: "client revoked or replaced".into(),
            };
            let _ = ws_sender.send(WsMessage::Close(Some(close_frame))).await;
            return Ok(());
        }

        // Remove existing connections for the same actor (to avoid stale mappings), and send a WS Close frame to let the old connection exit cleanly.
        let stale_clients = if let Some(ref aid) = actor_id {
            let stale_clients = remove_stale_actor_clients(&mut clients_guard, aid);

            // Synchronously update actor_id_index: acquire actor_id_index write lock while holding clients write lock
            // Maintain lock order: clients -> actor_id_index
            let mut actor_index = server.actor_id_index.write().await;
            actor_index.insert(aid.clone(), client_id.clone());

            stale_clients
        } else {
            Vec::new()
        };

        clients_guard.insert(
            client_id.clone(),
            ClientConnection {
                id: client_id.clone(),
                actor_id,
                credential,
                egress,
                client_ip,
                webrtc_role: webrtc_role.clone(),
            },
        );

        stale_clients
    };

    close_stale_clients(stale_clients, &server).await;

    // Register actor in actor registry for discovery/routing (only when URL identity is provided)
    if let Some((actor_id_for_registry, actor_type)) = actor_id_for_registry {
        {
            // Load pending service_spec and ws_address from DB (written by Issuer during registration)
            let (service_spec, ws_address) = {
                let db = platform::storage::db::get_database();
                let pool = db.get_pool();
                let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
                    "SELECT service_spec_blob, ws_address FROM pending_registration \
                     WHERE serial_number = ?1 AND realm = ?2 \
                       AND (expires_at IS NULL OR expires_at > unixepoch())",
                )
                .bind(actor_id_for_registry.serial_number as i64)
                .bind(&actor_id_for_registry.realm)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
                match row {
                    Some((blob, ws)) => (
                        blob.and_then(|b| actr_protocol::ServiceSpec::decode(b.as_slice()).ok()),
                        ws,
                    ),
                    None => (None, None),
                }
            };

            let service_name = format!("{}:{}", actor_type.publisher, actor_type.name);
            // A delayed registration must not overwrite a replacement. Use the
            // same lock order as installation and durable unregister: clients -> registry.
            let clients = server.clients.read().await;
            if clients
                .get(&client_id)
                .is_none_or(|client| client.actor_id.as_ref() != Some(&actor_id_for_registry))
            {
                return Ok(());
            }
            let mut registry = server.actor_registry.write().await;
            if let Err(e) = registry.register_service_full(
                actor_id_for_registry.clone(),
                actor_type.clone(),
                service_name,
                vec![],
                None,
                service_spec,
                None,
                ws_address,
            ) {
                platform::recording::warn!("Failed to register actor in actor registry: {}", e);
            }
        }
        // Authentication and the HTTP upgrade precede registration. A revoke
        // or reissue can commit and its subscriber can finish in that gap.
        // Once listed, subsequent revocations can find this connection; now
        // recheck durable state before announcing it or accepting messages.
        if require_client_auth {
            let refused =
                client_credential_refused(&actor_id_for_registry, connection_credential.as_ref())
                    .await;
            if refused {
                cleanup_client(&client_id, &server, CleanupMode::ExplicitUnregister).await;
                // The subscriber may have removed the connection before the
                // registry write above. Remove that late row too, while
                // preserving any replacement connection's registration.
                let clients = server.clients.read().await;
                if !clients
                    .values()
                    .any(|client| client.actor_id.as_ref() == Some(&actor_id_for_registry))
                {
                    server
                        .actor_registry
                        .write()
                        .await
                        .unregister_actor_durable(&actor_id_for_registry)
                        .await?;
                    server
                        .presence_manager
                        .write()
                        .await
                        .unsubscribe_all(&actor_id_for_registry);
                }
                drop(clients);
                let close_frame = axum::extract::ws::CloseFrame {
                    code: axum::extract::ws::close_code::POLICY,
                    reason: "client revoked or replaced".into(),
                };
                let _ = ws_sender.send(WsMessage::Close(Some(close_frame))).await;
                return Ok(());
            }
        }
        // Notify presence subscribers about the new actor
        {
            let actor_id_for_presence = actor_id_for_registry.clone();
            let presence = server.presence_manager.read().await;
            let subscribers = presence
                .get_subscribers_with_acl(&actor_id_for_presence, &actor_type)
                .await;
            drop(presence);

            for subscriber_id in &subscribers {
                // Find the client_id for this subscriber
                let sub_client_id = {
                    let idx = server.actor_id_index.read().await;
                    idx.get(subscriber_id).cloned()
                };
                if let Some(sub_cid) = sub_client_id {
                    let event = actr_protocol::ActrUpEvent {
                        actor_id: actor_id_for_presence.clone(),
                    };
                    let flow =
                        signaling_envelope::Flow::ServerToActr(actr_protocol::SignalingToActr {
                            target: subscriber_id.clone(),
                            payload: Some(actr_protocol::signaling_to_actr::Payload::ActrUpEvent(
                                event,
                            )),
                        });
                    let envelope = server.create_envelope(flow, None);
                    if let Err(e) = send_envelope_to_client(&sub_cid, envelope, &server).await {
                        platform::recording::warn!(
                            "Failed to send ActrUp notification to {}: {}",
                            subscriber_id.to_string_repr(),
                            e
                        );
                    }
                }
            }
        }
    }

    // The Realm retired while this connection was set up: either its state
    // was dropped before this connection was listed, or the registry refused
    // the registration above. Either way the connection must not stay.
    if let Some(realm) = connection_realm
        && server.actor_registry.read().await.is_retired(&realm)
    {
        cleanup_client(&client_id, &server, CleanupMode::ExplicitUnregister).await;
        let close_frame = axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: "realm retired".into(),
        };
        let _ = ws_sender.send(WsMessage::Close(Some(close_frame))).await;
        return Ok(());
    }

    // Task for handling client messages
    let server_for_receive = server.clone();
    let client_id_for_receive = client_id.clone();
    let max_frame_bytes = server.egress_config.max_frame_bytes as usize;
    let metrics_for_receive = server.queue_metrics.clone();

    let receive_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(WsMessage::Binary(data)) => {
                    // Reject oversized frames before protobuf decode/alloc (issue #351)
                    if data.len() > max_frame_bytes {
                        metrics_for_receive
                            .oversized_frame_rejections
                            .fetch_add(1, Ordering::Relaxed);
                        platform::recording::warn!(
                            "🚫 Rejecting oversized signaling frame: {} bytes (limit {}), from connection {}",
                            data.len(),
                            max_frame_bytes,
                            client_id_for_receive
                        );
                        continue;
                    }
                    if let Err(e) =
                        handle_client_envelope(&data, &client_id_for_receive, &server_for_receive)
                            .await
                    {
                        platform::recording::error!("Error handling client signaling: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    platform::recording::error!("WebSocket error: {}", e);
                    break;
                }
                Ok(message) => {
                    if !handle_websocket_control_message(
                        message,
                        &control_tx,
                        &client_id_for_receive,
                    ) {
                        break;
                    }
                }
            }
        }

        // Clean up client
        cleanup_client(
            &client_id_for_receive,
            &server_for_receive,
            CleanupMode::ConnectionClosed,
        )
        .await;
    });

    // Task for sending messages
    let mut send_task = tokio::spawn(write_egress(
        ws_sender,
        data_rx,
        control_rx,
        closed,
        write_timeout,
    ));
    let mut receive_task = receive_task;

    // Wait for either task to complete
    tokio::select! {
        result = &mut receive_task => {
            if let Err(e) = result {
                platform::recording::warn!(
                    "WebSocket receive task join error for client {}: {}",
                    client_id,
                    e
                );
            }
            send_task.abort();
            let _ = send_task.await;
        },
        result = &mut send_task => {
            if let Err(e) = result {
                platform::recording::warn!(
                    "WebSocket send task join error for client {}: {}",
                    client_id,
                    e
                );
            }
            receive_task.abort();
            let _ = receive_task.await;
        },
    }

    // Clean up client connection
    cleanup_client(&client_id, &server, CleanupMode::ConnectionClosed).await;
    platform::recording::info!("🔌 Client {} disconnected", client_id);

    Ok(())
}

/// Write a connection's queued frames until the connection ends.
///
/// Drains the control queue first (biased select) so Close/Ping/Pong are not
/// starved by data messages; every write has a write deadline, so slow or
/// non-reading clients are torn down on timeout (issue #351). Returns, ending
/// the connection, after writing a Close frame, on a write failure, when the
/// queues close, or when `closed` is cancelled.
async fn write_egress<S>(
    mut ws_sender: S,
    mut data_rx: tokio::sync::mpsc::Receiver<QueuedFrame>,
    mut control_rx: tokio::sync::mpsc::Receiver<WsMessage>,
    closed: tokio_util::sync::CancellationToken,
    write_timeout: Duration,
) where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    loop {
        tokio::select! {
            biased;
            // The server closed the connection but could not queue a Close frame
            _ = closed.cancelled() => break,
            // Control messages first
            ctrl = control_rx.recv() => {
                match ctrl {
                    Some(message) => {
                        let should_close = matches!(message, WsMessage::Close(_));
                        match timeout(write_timeout, ws_sender.send(message)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                platform::recording::warn!(
                                    "WebSocket control frame write failed: {}",
                                    e
                                );
                                break;
                            }
                            Err(_) => {
                                platform::recording::warn!(
                                    "WebSocket control frame write timed out ({}ms); closing slow-consumer connection",
                                    write_timeout.as_millis()
                                );
                                break;
                            }
                        }
                        if should_close {
                            break;
                        }
                    }
                    None => break,
                }
            }
            // Data messages
            data = data_rx.recv() => {
                match data {
                    Some(queued) => {
                        let QueuedFrame { msg, byte_len, _permit } = queued;
                        // Hold the byte permit during the write and release it after (applies accurate backpressure to slow consumers)
                        let send_result = timeout(write_timeout, ws_sender.send(msg)).await;
                        drop(_permit);
                        match send_result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                platform::recording::warn!(
                                    "WebSocket data frame write failed ({} bytes): {}",
                                    byte_len,
                                    e
                                );
                                break;
                            }
                            Err(_) => {
                                platform::recording::warn!(
                                    "WebSocket data frame write timed out ({}ms, {} bytes); closing slow-consumer connection",
                                    write_timeout.as_millis(),
                                    byte_len
                                );
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

fn remove_stale_actor_clients(
    clients: &mut HashMap<String, ClientConnection>,
    actor_id: &ActrId,
) -> Vec<StaleClient> {
    let stale_client_ids = clients
        .iter()
        .filter_map(|(cid, conn)| (conn.actor_id.as_ref() == Some(actor_id)).then_some(cid.clone()))
        .collect::<Vec<_>>();

    stale_client_ids
        .into_iter()
        .filter_map(|cid| {
            clients.remove(&cid).map(|client| StaleClient {
                id: cid,
                egress: client.egress,
            })
        })
        .collect()
}

async fn close_stale_clients(stale_clients: Vec<StaleClient>, server: &SignalingServerHandle) {
    for stale_client in stale_clients {
        let close_frame = axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: "displaced by new connection".into(),
        };

        if !stale_client.egress.close(close_frame) {
            platform::recording::debug!(
                "Stale WebSocket client {} control queue full or closed; closing without a Close frame",
                stale_client.id
            );
        }

        platform::recording::info!(
            "🧹 Closing stale WebSocket client {} for actor reconnect",
            stale_client.id
        );

        if let Some(ref limiter) = server.message_rate_limiter {
            limiter.remove_connection(&stale_client.id).await;
        }
        server
            .target_rate_limiter
            .remove_target(&stale_client.id)
            .await;
    }
}

fn handle_websocket_control_message(
    message: WsMessage,
    control_tx: &ClientEgress,
    client_id: &str,
) -> bool {
    match message {
        WsMessage::Ping(payload) => {
            platform::recording::debug!("Received WebSocket Ping, replying Pong: {}", client_id);
            // The control queue is bounded: drop the Pong when full (the peer retransmits the Ping, and the write deadline covers slow connections),
            // and only terminate the receive loop when the channel is closed (send task exited).
            match control_tx.try_send_control(WsMessage::Pong(payload)) {
                Ok(()) => true,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    platform::recording::warn!(
                        "WebSocket control queue full; dropping Pong for client {}",
                        client_id
                    );
                    true
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    platform::recording::warn!(
                        "WebSocket Pong send queue closed for client {}",
                        client_id
                    );
                    false
                }
            }
        }
        WsMessage::Pong(_) => {
            platform::recording::debug!("Received WebSocket Pong: {}", client_id);
            true
        }
        WsMessage::Close(_) => {
            platform::recording::info!("Client {} initiated disconnect", client_id);
            false
        }
        WsMessage::Text(_) => {
            platform::recording::warn!("Received Text WebSocket message, ignoring: {}", client_id);
            true
        }
        _ => {
            platform::recording::warn!(
                "Received unsupported non-Binary WebSocket message, ignoring: {}",
                client_id
            );
            true
        }
    }
}

/// Handle a SignalingEnvelope sent by the client
async fn handle_client_envelope(
    data: &[u8],
    client_id: &str,
    server: &SignalingServerHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = {
        let clients = server.clients.read().await;
        let Some(client) = clients.get(client_id) else {
            return Ok(());
        };
        client.actor_id.clone().zip(client.credential.clone())
    };
    // Check message rate limit
    if let Some(ref limiter) = server.message_rate_limiter
        && let Err(e) = limiter.check_message(client_id).await
    {
        platform::recording::warn!(
            "🚫 Connection {} message rate limit triggered: {}",
            client_id,
            e
        );
        // Send error response
        let error_response = ErrorResponse {
            code: 429,
            message: e,
        };
        let error_envelope =
            server.create_new_envelope(signaling_envelope::Flow::EnvelopeError(error_response));
        send_envelope_to_client(client_id, error_envelope, server).await?;
        return Ok(());
    }

    // Admission must precede database authorization work, including for malformed
    // binary frames. Admitted messages still recheck the live durable binding.
    if let Some((actor, credential)) = identity
        && client_credential_refused(&actor, Some(&credential)).await
    {
        close_refused_connection(server, client_id).await;
        return Ok(());
    }

    // Decode protobuf message
    let envelope = SignalingEnvelope::decode(data)?;

    #[cfg(feature = "opentelemetry")]
    let remote_context = extract_trace_context(&envelope);

    let span = info_span!(
        "signaling.handle_envelope",
        envelope_id = %envelope.envelope_id,
        client_id = %client_id
    );
    #[cfg(feature = "opentelemetry")]
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let _ = span.set_parent(remote_context.clone());
    }

    async move {
        platform::recording::debug!(
            "📨 Received signaling message envelope_id={}",
            envelope.envelope_id
        );

        // Handle message based on flow direction
        match envelope.flow {
            Some(signaling_envelope::Flow::PeerToServer(peer_to_server)) => {
                handle_peer_to_server(peer_to_server, client_id, server, &envelope.envelope_id)
                    .await
            }
            Some(signaling_envelope::Flow::ActrToServer(actr_to_server)) => {
                if !is_connection_actor(client_id, server, &actr_to_server.source).await {
                    platform::recording::warn!(
                        "Rejecting envelope whose source is not the connection's Actor: client_id={}, source={}",
                        client_id,
                        actr_to_server.source.to_string_repr()
                    );
                    return send_error_response(
                        client_id,
                        &actr_to_server.source,
                        403,
                        "Envelope source is not this connection's Actor",
                        server,
                        Some(&envelope.envelope_id),
                    )
                    .await;
                }
                handle_actr_to_server(actr_to_server, client_id, server, &envelope.envelope_id)
                    .await
            }
            Some(signaling_envelope::Flow::ActrRelay(ref relay)) => {
                if !in_connection_realm(client_id, server, &[&relay.source, &relay.target]).await {
                    return reject_outside_connection_realm(
                        client_id,
                        &relay.source,
                        server,
                        &envelope.envelope_id,
                    )
                    .await;
                }
                #[cfg(feature = "opentelemetry")]
                {
                    handle_actr_relay(
                        relay.clone(),
                        client_id,
                        server,
                        &envelope.envelope_id,
                        remote_context,
                    )
                    .await
                }
                #[cfg(not(feature = "opentelemetry"))]
                {
                    handle_actr_relay(relay.clone(), client_id, server, &envelope.envelope_id).await
                }
            }
            Some(signaling_envelope::Flow::EnvelopeError(error)) => {
                platform::recording::error!(
                    "Received envelope error: code={}, message={}",
                    error.code,
                    error.message
                );
                Ok(())
            }
            _ => {
                platform::recording::warn!("Unknown signaling flow direction");
                Ok(())
            }
        }
    }
    .instrument(span)
    .await
}

/// A signed message must still belong to the actor's current client key,
/// including when it arrives over an otherwise valid replacement connection.
async fn check_message_binding(
    client_id: &str,
    actor: &ActrId,
    client_key: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    if platform::realm::membership::credential_refused(actor, client_key).await? {
        send_error_response(
            client_id,
            actor,
            401,
            "Credential client key was revoked or replaced",
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(false);
    }
    Ok(true)
}

/// Whether `source` is the Actor the connection authenticated as.
async fn is_connection_actor(
    client_id: &str,
    server: &SignalingServerHandle,
    source: &ActrId,
) -> bool {
    let clients = server.clients.read().await;
    clients
        .get(client_id)
        .is_some_and(|c| c.actor_id.as_ref() == Some(source))
}

/// Whether every Actor an envelope names belongs to the connection's Realm.
///
/// A connection is bound to the Realm of the identity it authenticated with;
/// one without an identity belongs to no Realm.
async fn in_connection_realm(
    client_id: &str,
    server: &SignalingServerHandle,
    actors: &[&ActrId],
) -> bool {
    let clients = server.clients.read().await;
    let Some(connection) = clients.get(client_id).and_then(|c| c.actor_id.as_ref()) else {
        return false;
    };
    actors.iter().all(|actor| actor.realm == connection.realm)
}

async fn reject_outside_connection_realm(
    client_id: &str,
    source: &ActrId,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::warn!(
        "Rejecting envelope outside the connection's Realm: client_id={}, source={}",
        client_id,
        source.to_string_repr()
    );
    send_error_response(
        client_id,
        source,
        403,
        "Envelope names an Actor outside this connection's Realm",
        server,
        Some(request_envelope_id),
    )
    .await
}

/// Handle PeerToSignaling flow (pre-registration)
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_peer_to_server(
    peer_to_server: PeerToSignaling,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match peer_to_server.payload {
        Some(peer_to_signaling::Payload::RegisterRequest(_)) => {
            // Registration has been migrated to Issuer HTTP; signaling no longer relays registration requests
            platform::recording::warn!(
                "⚠️  RegisterRequest rejected: use /issuer/register HTTP endpoint instead"
            );
            send_register_error(
                client_id,
                410,
                "Registration via signaling is no longer supported; use /issuer/register HTTP endpoint",
                server,
                request_envelope_id,
            )
            .await?;
        }
        None => {
            platform::recording::warn!("PeerToSignaling message missing payload");
        }
    }
    Ok(())
}

/// Send registration error response
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn send_register_error(
    client_id: &str,
    code: u32,
    message: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let error_response = ErrorResponse {
        code,
        message: message.to_string(),
    };

    let response = RegisterResponse {
        result: Some(register_response::Result::Error(error_response)),
    };

    // Create temporary ActrId (for response)
    let temp_actor_id = ActrId {
        realm: String::new(),
        serial_number: 0,
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: temp_actor_id,
        payload: Some(signaling_to_actr::Payload::RegisterResponse(response)),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));

    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle ActrToSignaling flow (after registration)
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id, actr_id = %actr_to_server.source.to_string_repr())))]
async fn handle_actr_to_server(
    actr_to_server: ActrToSignaling,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = actr_to_server.source.clone();

    platform::recording::info!(
        "📬 Processing message from Actor {}",
        source.to_string_repr()
    );

    // Validate that Realm exists, is not expired, and is in normal state
    if let Err(e) = RealmEntity::validate_realm(&source.realm).await {
        let (code, msg) = RealmEntity::map_validation_error(e);
        platform::recording::warn!(
            "⚠️  Actor {} realm validation failed: {}",
            source.to_string_repr(),
            msg
        );
        send_error_response(
            client_id,
            &source,
            code,
            &format!("Realm validation failed: {msg}"),
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(());
    }

    // Validate credential and get tolerance period status
    let (claims, in_tolerance_period) = match AIdCredentialValidator::check(
        &actr_to_server.credential,
        &source.realm,
        &server.provider_id,
        "actr.signaling",
    )
    .await
    {
        Ok(checked) => checked,
        Err(e) => {
            platform::recording::warn!(
                "⚠️  Actor {} credential validation failed: {}",
                source.to_string_repr(),
                e
            );
            // Send error response
            send_error_response(
                client_id,
                &source,
                401,
                &format!("Credential validation failed: {e}"),
                server,
                Some(request_envelope_id),
            )
            .await?;
            return Ok(());
        }
    };

    // The credential must belong to the source; its claims carry the source's Actor type.
    if claims.actor_id != source.to_string_repr() {
        platform::recording::warn!(
            "source does not match credential actor_id: source={}, credential={}",
            source.to_string_repr(),
            claims.actor_id
        );
        send_error_response(
            client_id,
            &source,
            403,
            "Source identity mismatch: source does not match credential",
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(());
    }
    let source_type = claims.actor_type;

    if !check_message_binding(
        client_id,
        &source,
        &claims.client_key,
        server,
        request_envelope_id,
    )
    .await?
    {
        return Ok(());
    }

    match actr_to_server.payload {
        Some(actr_to_signaling::Payload::Ping(ping)) => {
            handle_ping(
                source,
                ping,
                client_id,
                server,
                request_envelope_id,
                in_tolerance_period,
            )
            .await?;
        }
        Some(actr_to_signaling::Payload::UnregisterRequest(req)) => {
            handle_unregister(source, req, client_id, server, request_envelope_id).await?;
        }
        Some(actr_to_signaling::Payload::DiscoveryRequest(req)) => {
            handle_discovery_request(
                source,
                &source_type,
                req,
                client_id,
                server,
                request_envelope_id,
            )
            .await?;
        }
        Some(actr_to_signaling::Payload::RouteCandidatesRequest(req)) => {
            handle_route_candidates_request(
                source,
                &source_type,
                req,
                client_id,
                server,
                request_envelope_id,
            )
            .await?;
        }
        Some(actr_to_signaling::Payload::GetServiceSpecRequest(req)) => {
            handle_get_service_spec_request(source, req, client_id, server, request_envelope_id)
                .await?;
        }
        Some(actr_to_signaling::Payload::SubscribeActrUpRequest(req)) => {
            handle_subscribe_actr_up(
                source,
                source_type,
                req,
                client_id,
                server,
                request_envelope_id,
            )
            .await?;
        }
        Some(actr_to_signaling::Payload::UnsubscribeActrUpRequest(req)) => {
            handle_unsubscribe_actr_up(source, req, client_id, server, request_envelope_id).await?;
        }
        Some(actr_to_signaling::Payload::GetSigningKeyRequest(req)) => {
            handle_get_signing_key_request(source, req, client_id, server, request_envelope_id)
                .await?;
        }
        Some(actr_to_signaling::Payload::Error(error)) => {
            platform::recording::error!(
                "Received client error report (Actor {}): code={}, message={}",
                source.to_string_repr(),
                error.code,
                error.message
            );
        }
        None => {
            platform::recording::warn!("ActrToSignaling message missing payload");
        }
    }

    Ok(())
}

/// Handle heartbeat (Ping)
async fn handle_ping(
    source: ActrId,
    ping: Ping,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
    in_tolerance_period: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "💓 Received Actor {} heartbeat: availability={}, power_reserve={:.2}, mailbox_backlog={:.2}, sticky_clients={}{}",
        source.to_string_repr(),
        ping.availability,
        ping.power_reserve,
        ping.mailbox_backlog,
        ping.sticky_client_ids.len(),
        if in_tolerance_period {
            " [⚠️ Key in tolerance period]"
        } else {
            ""
        }
    );

    // Store load metrics in ActorRegistry
    let mut registry = server.actor_registry.write().await;
    if let Err(e) = registry.update_load_metrics(
        &source,
        ping.availability,
        ping.power_reserve,
        ping.mailbox_backlog,
    ) {
        platform::recording::warn!(
            "Failed to update load metrics for Actor {}: {}, attempting to restore service from database",
            source.to_string_repr(),
            e
        );

        // Attempt to restore service from database
        match registry.restore_service_from_storage(&source).await {
            Ok(true) => {
                platform::recording::info!(
                    "✅ Successfully restored service registration for Actor {} from database",
                    source.to_string_repr()
                );

                // Retry load metrics update after restoration
                if let Err(e2) = registry.update_load_metrics(
                    &source,
                    ping.availability,
                    ping.power_reserve,
                    ping.mailbox_backlog,
                ) {
                    platform::recording::error!(
                        "❌ Still unable to update load metrics for Actor {} after database restoration: {}",
                        source.to_string_repr(),
                        e2
                    );
                } else {
                    platform::recording::info!(
                        "✅ Load metrics updated successfully after service restoration for Actor {}",
                        source.to_string_repr()
                    );
                }
            }
            Ok(false) => {
                platform::recording::warn!(
                    "⚠️  Service info for Actor {} not found in database (may have expired or was never registered)",
                    source.to_string_repr()
                );
                // TODO: optional - add a warning in Pong response to prompt client to re-register
            }
            Err(e) => {
                platform::recording::error!(
                    "❌ Failed to restore service for Actor {} from database: {}",
                    source.to_string_repr(),
                    e
                );
            }
        }
    }
    drop(registry);

    // Create Pong response
    let mut pong = Pong {
        // Wall-clock seconds used as an opaque echo value only. This is not a
        // sequence number: clock adjustments can make it repeat or go
        // backwards, so consumers must not rely on it for ordering.
        seq: chrono::Utc::now().timestamp() as u64,
        suggest_interval_secs: Some(30),
        credential_warning: None,
    };

    // If credential key is in tolerance period, add a warning
    if in_tolerance_period {
        platform::recording::warn!(
            "⚠️  Actor {} credential key is in tolerance period",
            source.to_string_repr()
        );
        pong.credential_warning = Some(actr_protocol::CredentialWarning {
            r#type: actr_protocol::credential_warning::WarningType::KeyInTolerancePeriod as i32,
            message:
                "Your credential key is in tolerance period. Please update your credential soon."
                    .to_string(),
        });
    }

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::Pong(pong)),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));

    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle unregistration
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_unregister(
    source: ActrId,
    req: actr_protocol::UnregisterRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "👋 Actor {} unregistered: reason={:?}",
        source.to_string_repr(),
        req.reason.as_deref().unwrap_or("not provided")
    );

    // Serialize ownership and deletion with replacement installation. An old
    // handler may have passed authentication before the new connection arrived.
    let clients = server.clients.read().await;
    if clients
        .get(client_id)
        .is_none_or(|client| client.actor_id.as_ref() != Some(&source))
    {
        return Ok(());
    }
    // The storage DELETE must complete before Success is acknowledged: the
    // peer treats UnregisterOk as "the removal survives a server restart".
    let durable = server
        .actor_registry
        .write()
        .await
        .unregister_actor_durable(&source)
        .await;
    drop(clients);

    let response = match durable {
        Ok(()) => actr_protocol::UnregisterResponse {
            result: Some(actr_protocol::unregister_response::Result::Success(
                actr_protocol::unregister_response::UnregisterOk {},
            )),
        },
        Err(error) => {
            platform::recording::error!(
                "durable unregister failed for Actor {}: {error}",
                source.to_string_repr()
            );
            actr_protocol::UnregisterResponse {
                result: Some(actr_protocol::unregister_response::Result::Error(
                    actr_protocol::ErrorResponse {
                        code: 500,
                        message: error,
                    },
                )),
            }
        }
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::UnregisterResponse(response)),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    // Clean up client connection
    cleanup_client(client_id, server, CleanupMode::ExplicitUnregister).await;

    Ok(())
}

/// Resolve client_id via actor_id_index, keeping the index in sync with clients
#[allow(dead_code)]
async fn resolve_client_id_by_actor_id(
    actor_id: &ActrId,
    server: &SignalingServerHandle,
) -> Result<String, String> {
    let clients_guard = server.clients.read().await;
    let index_guard = server.actor_id_index.read().await;
    let client_id = index_guard.get(actor_id).cloned();

    let client_id = match client_id {
        Some(id) => id,
        None => {
            platform::recording::warn!(
                "⚠️  Actor {} missing client_id index, may not be registered yet or already cleaned up",
                actor_id.to_string_repr()
            );
            return Err("client_id not found for actor_id".into());
        }
    };

    if !clients_guard.contains_key(&client_id) {
        platform::recording::warn!(
            "⚠️  Actor {} index points to non-existent client {}, index may be stale",
            actor_id.to_string_repr(),
            client_id
        );
        return Err("actor_id_index stale for actor_id".into());
    }

    Ok(client_id)
}

#[derive(Debug, PartialEq, Eq)]
enum RelaySourceConnectionStatus {
    Current,
    ClientMissing,
    ClientActorMismatch,
    ActorIndexMissing,
    Displaced { current_client_id: String },
}

async fn relay_source_connection_status(
    client_id: &str,
    source: &ActrId,
    server: &SignalingServerHandle,
) -> RelaySourceConnectionStatus {
    let clients_guard = server.clients.read().await;
    let Some(client) = clients_guard.get(client_id) else {
        return RelaySourceConnectionStatus::ClientMissing;
    };

    if client.actor_id.as_ref() != Some(source) {
        return RelaySourceConnectionStatus::ClientActorMismatch;
    }

    let actor_index = server.actor_id_index.read().await;
    match actor_index.get(source) {
        Some(current_client_id) if current_client_id == client_id => {
            RelaySourceConnectionStatus::Current
        }
        Some(current_client_id) => RelaySourceConnectionStatus::Displaced {
            current_client_id: current_client_id.clone(),
        },
        None => RelaySourceConnectionStatus::ActorIndexMissing,
    }
}

async fn send_relay_delivery_result_if_requested(
    source_client_id: &str,
    source: &ActrId,
    target: &ActrId,
    context: Option<&RelayDeliveryContext>,
    disposition: RelayEgressDisposition,
    request_envelope_id: &str,
    server: &SignalingServerHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(context) = context else {
        return Ok(());
    };
    let result = RelayDeliveryResult {
        target: target.clone(),
        context: *context,
        status: disposition.wire_status() as i32,
    };
    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source.clone(),
        payload: Some(signaling_to_actr::Payload::RelayDeliveryResult(result)),
    });
    let envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(source_client_id, envelope, server).await
}

/// Handle ActrRelay (WebRTC signaling relay)
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id, actr_id = %relay.source.to_string_repr(), target_id = %relay.target.to_string_repr())))]
async fn handle_actr_relay(
    relay: ActrRelay,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
    #[cfg(feature = "opentelemetry")] remote_context: opentelemetry::Context,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = relay.source.clone();
    let target = relay.target.clone();
    let delivery_context = relay.delivery_context;
    // Validate source Actor realm (exists, not expired, and in normal state)
    if let Err(e) = RealmEntity::validate_realm(&source.realm).await {
        let (code, msg) = RealmEntity::map_validation_error(e);
        platform::recording::warn!(
            "⚠️  Actor {} realm validation failed: {}",
            source.to_string_repr(),
            msg
        );
        send_error_response(
            client_id,
            &source,
            code,
            &format!("Realm validation failed: {msg}"),
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(());
    }

    platform::recording::info!(
        "🔀 Relay signaling: {} -> {}",
        source.to_string_repr(),
        target.to_string_repr()
    );

    platform::recording::debug!("handle_actr_relay: relay={:?}", relay);

    // Validate credential before the ACL check: the source type used by the ACL
    // comes from the verified claims.
    let claims = match AIdCredentialValidator::check(
        &relay.credential,
        &source.realm,
        &server.provider_id,
        "actr.signaling",
    )
    .await
    {
        Ok((claims, _)) => claims,
        Err(e) => {
            platform::recording::warn!(
                "Actor {} credential validation failed: {}",
                source.to_string_repr(),
                e
            );
            send_error_response(
                client_id,
                &source,
                401,
                &format!("Credential validation failed: {e}"),
                server,
                Some(request_envelope_id),
            )
            .await?;
            return Ok(());
        }
    };

    // Verify that the actor_id bound to the credential matches relay.source,
    // preventing clients from forging their source identity.
    let source_repr = source.to_string_repr();
    if claims.actor_id != source_repr {
        platform::recording::warn!(
            "relay.source does not match credential actor_id: source={}, credential={}",
            source_repr,
            claims.actor_id
        );
        send_error_response(
            client_id,
            &source,
            403,
            "Source identity mismatch: relay.source does not match credential",
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(());
    }
    let source_type = claims.actor_type;
    if !check_message_binding(
        client_id,
        &source,
        &claims.client_key,
        server,
        request_envelope_id,
    )
    .await?
    {
        return Ok(());
    }

    // ACL check: can source relay to target? Same-realm / cross-realm both go
    // through the same rule query. The target type comes from the registry
    // record of the target, never from the sender (SIG-02).
    use platform::realm::acl::ActorAcl;
    let target_type = server
        .actor_registry
        .read()
        .await
        .get_actor_type(&target)
        .cloned();
    let Some(target_type) = target_type else {
        platform::recording::warn!(
            "⚠️ Relay target Actor {} is not registered",
            target.to_string_repr()
        );
        send_relay_delivery_result_if_requested(
            client_id,
            &source,
            &target,
            delivery_context.as_ref(),
            RelayEgressDisposition::TargetUnavailable,
            request_envelope_id,
            server,
        )
        .await?;
        return Ok(());
    };

    let can_relay = ActorAcl::can_discover(
        &source.realm,
        &target.realm,
        &type_key(&source_type),
        &type_key(&target_type),
    )
    .await
    .unwrap_or(false);

    if !can_relay {
        platform::recording::warn!(
            "⚠️  ACL denied relay: {} -> {}",
            source.to_string_repr(),
            target.to_string_repr()
        );
        send_error_response(
            client_id,
            &source,
            403,
            "ACL policy denies relay to target actor",
            server,
            Some(request_envelope_id),
        )
        .await?;
        return Ok(());
    }

    let connection_status = relay_source_connection_status(client_id, &source, server).await;
    if connection_status != RelaySourceConnectionStatus::Current {
        platform::recording::warn!(
            "Dropping ActrRelay from non-current WebSocket: client_id={}, source={}, status={:?}",
            client_id,
            source.to_string_repr(),
            connection_status
        );
        return Ok(());
    }

    // Role negotiation: server decides offerer/answerer and notifies both parties
    if let Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation { from, to, .. })) =
        relay.payload.clone()
    {
        // The ACL above covers only source -> target, so the negotiation must
        // name exactly that pair.
        if from != source || to != target {
            platform::recording::warn!(
                "RoleNegotiation does not match relay endpoints: source={}, target={}, from={}, to={}",
                source.to_string_repr(),
                target.to_string_repr(),
                from.to_string_repr(),
                to.to_string_repr()
            );
            send_error_response(
                client_id,
                &source,
                403,
                "RoleNegotiation must be from the relay source to the relay target",
                server,
                Some(request_envelope_id),
            )
            .await?;
            return Ok(());
        }
        if resolve_client_id_by_actor_id(&to, server).await.is_err() {
            platform::recording::warn!(
                "⚠️ Role negotiation target Actor {} is unavailable",
                to.to_string_repr()
            );
            send_relay_delivery_result_if_requested(
                client_id,
                &source,
                &to,
                delivery_context.as_ref(),
                RelayEgressDisposition::TargetUnavailable,
                request_envelope_id,
                server,
            )
            .await?;
            return Ok(());
        }
        let clients_guard = server.clients.read().await;

        // Use determine_webrtc_role function to determine role
        let is_offerer = determine_webrtc_role(&from, &to, &clients_guard);

        drop(clients_guard);

        // Both assignments are server-to-client notifications: the receiver only
        // consumes `is_offerer`, so the sender's bearer credential must not ride
        // along -- `to` is a peer and must never receive the initiator's live
        // credential.
        let empty_credential = AIdCredential::default();

        // Send RoleAssignment to from; `from` is the verified source.
        let from_type = Some(source_type.clone());

        let new_relay = ActrRelay {
            // source: peer actor, target: receiver of this assignment
            source: from.clone(),
            credential: empty_credential.clone(),
            target: to.clone(),
            delivery_context: None,
            source_type: from_type.clone(),
            payload: Some(actr_relay::Payload::RoleAssignment(RoleAssignment {
                is_offerer,
                remote_fixed: None,
            })),
        };
        send_role_assignment(
            &from,
            server,
            new_relay.clone(),
            #[cfg(feature = "opentelemetry")]
            remote_context.clone(),
        )
        .await?;

        // Send RoleAssignment to to
        let new_relay = ActrRelay {
            // source: peer actor, target: receiver of this assignment
            source: from.clone(),
            credential: empty_credential,
            target: to.clone(),
            delivery_context: None,
            source_type: from_type,
            payload: Some(actr_relay::Payload::RoleAssignment(RoleAssignment {
                is_offerer: !is_offerer,
                remote_fixed: None,
            })),
        };

        let target_disposition = send_role_assignment(
            &to,
            server,
            new_relay,
            #[cfg(feature = "opentelemetry")]
            remote_context,
        )
        .await?;

        send_relay_delivery_result_if_requested(
            client_id,
            &source,
            &to,
            delivery_context.as_ref(),
            target_disposition,
            request_envelope_id,
            server,
        )
        .await?;

        return Ok(());
    }

    // Find the target client and forward other relay messages (resolve client_id under a short read lock, then release before awaiting
    // delivery -- the delivery path may take a write lock to clean up a slow-consumer connection, so holding the read lock would deadlock).
    let target_client_id = {
        let clients_guard = server.clients.read().await;
        clients_guard.iter().find_map(|(id, client)| {
            (client.actor_id.as_ref() == Some(&target)).then(|| id.clone())
        })
    };

    if let Some(target_client_id) = target_client_id {
        // Strip the sender's credential before forwarding to the peer: the credential is
        // a bearer token, and forwarding it would let the peer impersonate the sender.
        // The peer discards this field on receipt, so clearing it has no observable
        // effect on behavior beyond closing the credential-leak vector.
        // The source type is overwritten with the verified claims value; whatever
        // the sender put there is discarded.
        let mut relay = relay;
        relay.credential = Default::default();
        relay.source_type = Some(source_type);

        // Reconstruct envelope and forward
        let flow = signaling_envelope::Flow::ActrRelay(relay);
        #[allow(unused_mut)]
        let mut forward_envelope = server.create_new_envelope(flow);

        // Inject the original trace context into the forwarded envelope to ensure end-to-end tracing
        #[cfg(feature = "opentelemetry")]
        inject_trace_context(&remote_context, &mut forward_envelope);
        let disposition =
            send_envelope_to_client_with_disposition(&target_client_id, forward_envelope, server)
                .await?;

        send_relay_delivery_result_if_requested(
            client_id,
            &source,
            &target,
            delivery_context.as_ref(),
            disposition,
            request_envelope_id,
            server,
        )
        .await?;

        platform::recording::info!("✅ Signaling relay successful");
    } else {
        platform::recording::warn!("⚠️ Target Actor {} not found", target.to_string_repr());
        send_relay_delivery_result_if_requested(
            client_id,
            &source,
            &target,
            delivery_context.as_ref(),
            RelayEgressDisposition::TargetUnavailable,
            request_envelope_id,
            server,
        )
        .await?;
    }

    Ok(())
}

// Compute ActorId key for sorting, ensuring role assignment is deterministic
fn actor_order_key(id: &ActrId) -> (&str, u64) {
    (&id.realm, id.serial_number)
}

/// Determine whether the initiator is the offerer based on both parties role preferences and ActorId
///
/// # Role determination rules:
/// 1. If one party explicitly requests to be "answer" and the other does not, satisfy that request
/// 2. If both parties have the same preference (both want "answer" or neither specifies), fall back to ActrId static sorting logic
///
/// # Arguments
/// * `from` - initiator ActorId
/// * `to` - receiver ActorId
/// * `clients` - client connection map, used to look up role preferences
///
/// # Returns
/// * `true` - initiator should be offerer
/// * `false` - initiator should be answerer
fn determine_webrtc_role(
    from: &ActrId,
    to: &ActrId,
    clients: &HashMap<String, ClientConnection>,
) -> bool {
    // Look up both parties role preferences from client connections
    let from_role = clients
        .values()
        .find(|c| c.actor_id.as_ref() == Some(from))
        .and_then(|c| c.webrtc_role.as_deref());

    let to_role = clients
        .values()
        .find(|c| c.actor_id.as_ref() == Some(to))
        .and_then(|c| c.webrtc_role.as_deref());

    let is_offerer = if from_role == Some("answer") && to_role != Some("answer") {
        false
    } else if to_role == Some("answer") && from_role != Some("answer") {
        true
    } else {
        // Other cases (both prefer answer or both have no preference), use ActorId reverse ordering
        actor_order_key(from) > actor_order_key(to)
    };

    platform::recording::info!(
        "⚖️ Role negotiation complete: {} (role={:?}) -> {} (role={:?}), is_offerer={}",
        from.to_string_repr(),
        from_role,
        to.to_string_repr(),
        to_role,
        is_offerer
    );

    is_offerer
}

#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all))]
async fn send_role_assignment(
    target_actor: &ActrId,
    server: &SignalingServerHandle,
    relay: ActrRelay,
    #[cfg(feature = "opentelemetry")] remote_context: opentelemetry::Context,
) -> Result<RelayEgressDisposition, Box<dyn std::error::Error>> {
    let flow = signaling_envelope::Flow::ActrRelay(relay);
    #[allow(unused_mut)]
    let mut envelope = server.create_new_envelope(flow);

    #[cfg(feature = "opentelemetry")]
    inject_trace_context(&remote_context, &mut envelope);

    let mut buf = Vec::new();
    envelope.encode(&mut buf)?;

    // Resolve the target client_id (short read lock), then release the lock and deliver via the unified bounded egress path.
    let target_client_id = {
        let clients_guard = server.clients.read().await;
        clients_guard.iter().find_map(|(id, client)| {
            (client.actor_id.as_ref() == Some(target_actor)).then(|| id.clone())
        })
    };

    if let Some(target_client_id) = target_client_id {
        platform::recording::debug!(
            "send_role_assignment: sending envelope to client {}",
            target_client_id
        );
        enqueue_binary_to_client(&target_client_id, buf, server).await
    } else {
        platform::recording::warn!(
            "⚠️ send_role_assignment: target Actor {} not found",
            target_actor.to_string_repr()
        );
        Ok(RelayEgressDisposition::TargetUnavailable)
    }
}

/// Send SignalingEnvelope to client
///
/// Target-side delivery problems (target missing / queue saturated / queue closed) are handled internally (logged + counted +
/// slow-consumer target closed) and are **not** propagated to the sender as Err, so a healthy sender is not disconnected
/// because of a slow/vanished target (issue #351 acceptance criterion 4). Err is returned only on encode failure (an internal bug).
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = envelope.envelope_id)))]
async fn send_envelope_to_client(
    client_id: &str,
    envelope: SignalingEnvelope,
    server: &SignalingServerHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    send_envelope_to_client_with_disposition(client_id, envelope, server)
        .await
        .map(|_| ())
}

async fn send_envelope_to_client_with_disposition(
    client_id: &str,
    #[allow(unused_mut)] mut envelope: SignalingEnvelope,
    server: &SignalingServerHandle,
) -> Result<RelayEgressDisposition, Box<dyn std::error::Error>> {
    #[cfg(feature = "opentelemetry")]
    {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let context = tracing::Span::current().context();
        inject_trace_context(&context, &mut envelope);
    }

    // Encode protobuf
    let mut buf = Vec::new();
    envelope.encode(&mut buf)?;

    enqueue_binary_to_client(client_id, buf, server).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayEgressDisposition {
    Accepted,
    TargetUnavailable,
    Rejected,
}

impl RelayEgressDisposition {
    fn wire_status(self) -> RelayDeliveryStatus {
        match self {
            Self::Accepted => RelayDeliveryStatus::RelayDeliveryAccepted,
            Self::TargetUnavailable => RelayDeliveryStatus::RelayDeliveryTargetUnavailable,
            Self::Rejected => RelayDeliveryStatus::RelayDeliveryRejected,
        }
    }
}

/// Deliver an encoded Binary frame to the target client's bounded egress queue (issue #351).
///
/// - Clone the egress handle under a short read lock, release it immediately, then await capacity (never block while holding the lock).
/// - Apply the per-target aggregate rate/byte budget (across all senders).
/// - If the queue stays full past the deadline, close and remove the slow-consumer client; a missing/closed target is only logged.
///
/// Target-side problems always return Ok (handled internally), so the sender is never implicated.
async fn enqueue_binary_to_client(
    client_id: &str,
    buf: Vec<u8>,
    server: &SignalingServerHandle,
) -> Result<RelayEgressDisposition, Box<dyn std::error::Error>> {
    let byte_len = buf.len();

    // Defense: a single message larger than the whole per-client byte budget can never be enqueued (acquire would block
    // until the timeout and wrongly kill a healthy client). Drop and count it directly, rather than disconnecting.
    if byte_len > server.egress_config.max_bytes as usize {
        server
            .queue_metrics
            .target_budget_rejections
            .fetch_add(1, Ordering::Relaxed);
        platform::recording::warn!(
            "🚫 Dropping outbound message that exceeds the per-client byte budget: {} bytes (budget {}), target {}",
            byte_len,
            server.egress_config.max_bytes,
            client_id
        );
        return Ok(RelayEgressDisposition::Rejected);
    }

    // Clone the egress handle (short read lock), then release the lock before awaiting capacity
    let egress = {
        let clients_guard = server.clients.read().await;
        clients_guard.get(client_id).map(|c| c.egress.clone())
    };
    let Some(egress) = egress else {
        platform::recording::warn!("⚠️ Client {} not found", client_id);
        return Ok(RelayEgressDisposition::TargetUnavailable);
    };

    // Per-target aggregate rate/byte budget (across all senders, issue #351)
    if let Err(e) = server
        .target_rate_limiter
        .check_target(client_id, byte_len as u32)
        .await
    {
        server
            .queue_metrics
            .target_budget_rejections
            .fetch_add(1, Ordering::Relaxed);
        platform::recording::warn!(
            "🚫 Target {} aggregate budget exceeded, dropping message: {}",
            client_id,
            e
        );
        return Ok(RelayEgressDisposition::Rejected);
    }

    match egress
        .send_data(WsMessage::Binary(buf.into()), byte_len)
        .await
    {
        Ok(()) => {
            platform::recording::debug!("✅ Enqueued {} bytes to client {}", byte_len, client_id);
            Ok(RelayEgressDisposition::Accepted)
        }
        Err(EnqueueError::Saturated) => {
            // Slow consumer: close and remove the target connection (healthy clients unaffected)
            close_slow_client(client_id, server, &egress).await;
            Ok(RelayEgressDisposition::TargetUnavailable)
        }
        Err(EnqueueError::Closed) => {
            platform::recording::debug!(
                "Client {} egress queue closed, dropping message",
                client_id
            );
            cleanup_client(client_id, server, CleanupMode::ConnectionClosed).await;
            Ok(RelayEgressDisposition::TargetUnavailable)
        }
    }
}

/// Close and remove a slow-consumer client connection (issue #351).
///
/// Best-effort send a Close frame over the independent control queue, and remove the client from the routing table
/// (`cleanup_client` is idempotent, so the connection task's later second cleanup is a no-op). The write task's write
/// deadline completes the teardown as a fallback. Other healthy clients are unaffected.
async fn close_slow_client(client_id: &str, server: &SignalingServerHandle, egress: &ClientEgress) {
    server
        .queue_metrics
        .slow_consumer_disconnects
        .fetch_add(1, Ordering::Relaxed);
    platform::recording::warn!(
        "🔌 Closing slow-consumer client {} (egress queue stayed full for {}ms)",
        client_id,
        server.egress_config.enqueue_timeout_ms
    );

    let close_frame = axum::extract::ws::CloseFrame {
        code: axum::extract::ws::close_code::ERROR,
        reason: "slow consumer: egress queue saturated".into(),
    };
    egress.close(close_frame);

    cleanup_client(client_id, server, CleanupMode::ConnectionClosed).await;
}

/// Clean up client connection
async fn cleanup_client(client_id: &str, server: &SignalingServerHandle, mode: CleanupMode) {
    let (removed_client, actor_cleanup) = {
        let mut clients_guard = server.clients.write().await;
        let removed_client = clients_guard.remove(client_id);

        let actor_cleanup = removed_client
            .as_ref()
            .and_then(|client| client.actor_id.clone())
            .map(|actor_id| {
                let replacement_client_id = clients_guard
                    .iter()
                    .find(|(id, client)| {
                        id.as_str() != client_id && client.actor_id.as_ref() == Some(&actor_id)
                    })
                    .map(|(id, _)| id.clone());

                (actor_id, replacement_client_id)
            });

        if let Some((actor_id, replacement_client_id)) = actor_cleanup.as_ref() {
            let mut actor_index = server.actor_id_index.write().await;
            match actor_index.get(actor_id).cloned() {
                Some(mapped_client) if mapped_client == client_id => {
                    if let Some(replacement_client_id) = replacement_client_id {
                        actor_index.insert(actor_id.clone(), replacement_client_id.clone());
                        platform::recording::info!(
                            "🔁 Actor {} switched to new client {}, preserving index",
                            actor_id.to_string_repr(),
                            replacement_client_id
                        );
                    } else {
                        actor_index.remove(actor_id);
                    }
                }
                Some(mapped_client) => {
                    platform::recording::warn!(
                        "⚠️  Actor {} index points to another client {}, preserving index",
                        actor_id.to_string_repr(),
                        mapped_client
                    );
                }
                None => {
                    if let Some(replacement_client_id) = replacement_client_id {
                        actor_index.insert(actor_id.clone(), replacement_client_id.clone());
                        platform::recording::warn!(
                            "⚠️  Actor {} index missing during cleanup, fixed to new client {}",
                            actor_id.to_string_repr(),
                            replacement_client_id
                        );
                    } else {
                        platform::recording::warn!(
                            "⚠️  Actor {} no index entry found during cleanup",
                            actor_id.to_string_repr()
                        );
                    }
                }
            }
        }

        (removed_client, actor_cleanup)
    };

    if let Some(_client) = removed_client {
        if let Some((actor_id, replacement_client_id)) = actor_cleanup {
            platform::recording::info!(
                "🧹 Cleaning up Actor {} connection",
                actor_id.to_string_repr()
            );

            if let Some(replacement_client_id) = replacement_client_id {
                platform::recording::info!(
                    "⏭️ Actor {} already has new connection {}, skipping service unregistration",
                    actor_id.to_string_repr(),
                    replacement_client_id
                );
            } else {
                // A replacement may have connected after the snapshot above.
                let clients = server.clients.read().await;
                if !clients
                    .values()
                    .any(|client| client.actor_id.as_ref() == Some(&actor_id))
                {
                    let mut registry = server.actor_registry.write().await;
                    match mode {
                        CleanupMode::ConnectionClosed => {
                            registry.unregister_actor_memory_only(&actor_id);
                        }
                        CleanupMode::ExplicitUnregister => {
                            if let Err(error) = registry.unregister_actor_durable(&actor_id).await {
                                platform::recording::warn!(
                                    "Failed to unregister Actor during connection cleanup: {error}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Remove message rate limiter
        if let Some(ref limiter) = server.message_rate_limiter {
            limiter.remove_connection(client_id).await;
        }

        // Remove the per-target aggregate limiter entry (issue #351)
        server.target_rate_limiter.remove_target(client_id).await;
    }
}

/// Handle service discovery request
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_discovery_request(
    source: ActrId,
    source_type: &ActrType,
    req: actr_protocol::DiscoveryRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "🔍 Processing Discovery request from Actor {}: publisher={:?}, limit={}",
        source.to_string_repr(),
        req.publisher.as_deref().unwrap_or("*"),
        req.limit.unwrap_or(64)
    );

    // Query all services of the requester's Realm from ActorRegistry
    let registry = server.actor_registry.read().await;
    let services = registry.discover_all(&source.realm, req.publisher.as_deref());
    let total_count = services.len(); // Save count before moving

    // Apply ACL filtering (if ACL is enabled)
    use platform::realm::acl::ActorAcl;
    let source_realm = &source.realm;
    let source_type = type_key(source_type);

    let mut acl_filtered_services = Vec::new();

    // ACL always enabled: filter services based on ACL rules
    for service in services {
        let target_realm = &service.actor_id.realm;
        let target_type = type_key(&service.actor_type);

        match ActorAcl::can_discover(source_realm, target_realm, &source_type, &target_type).await {
            Ok(true) => acl_filtered_services.push(service),
            Ok(false) => {
                platform::recording::debug!(
                    "ACL denied discovery: {} (realm {}) cannot discover {} (realm {})",
                    source.to_string_repr(),
                    source_realm,
                    service.actor_id.to_string_repr(),
                    target_realm
                );
            }
            Err(e) => {
                platform::recording::warn!(
                    "ACL check failed for {} (realm {}) -> {} (realm {}): {}",
                    source.to_string_repr(),
                    source_realm,
                    service.actor_id.to_string_repr(),
                    target_realm,
                    e
                );
            }
        }
    }
    platform::recording::info!(
        "ACL filtering: {} -> {} services",
        total_count,
        acl_filtered_services.len()
    );

    // Aggregate services by ActrType (using HashMap for deduplication)
    use std::collections::HashMap;
    let mut type_map: HashMap<String, actr_protocol::discovery_response::TypeEntry> =
        HashMap::new();

    for service in acl_filtered_services {
        let type_key = type_key(&service.actor_type);

        // If this type has not been added yet, create a new entry
        type_map.entry(type_key).or_insert_with(|| {
            let (fingerprint, description, published_at, tags) = service
                .service_spec
                .as_ref()
                .map(|spec| {
                    (
                        spec.fingerprint.clone(),
                        spec.description.clone(),
                        spec.published_at,
                        spec.tags.clone(),
                    )
                })
                .unwrap_or_else(|| ("unknown".to_string(), None, None, Vec::new()));

            actr_protocol::discovery_response::TypeEntry {
                actr_type: service.actor_type.clone(),
                name: service.service_name.clone(),
                description,
                service_fingerprint: fingerprint,
                published_at,
                tags,
            }
        });
    }

    // Convert to Vec and apply limit
    let mut entries: Vec<_> = type_map.into_values().collect();
    let limit = req.limit.unwrap_or(64) as usize;
    entries.truncate(limit);

    drop(registry);

    platform::recording::info!(
        "✅ Returned {} service types for Actor {}",
        source.to_string_repr(),
        entries.len()
    );

    let response = actr_protocol::DiscoveryResponse {
        result: Some(actr_protocol::discovery_response::Result::Success(
            actr_protocol::discovery_response::DiscoveryOk { entries },
        )),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::DiscoveryResponse(response)),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle route candidates request (load balancing)
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_route_candidates_request(
    source: ActrId,
    source_type: &ActrType,
    req: actr_protocol::RouteCandidatesRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "🎯 Processing RouteCandidates request from Actor {}: target_type={}/{}",
        source.to_string_repr(),
        req.target_type.publisher,
        req.target_type.name,
    );

    // Query all instances of the requester's Realm matching target_type from ActorRegistry
    let registry = server.actor_registry.read().await;
    let candidates_raw = registry.find_by_actr_type(&source.realm, &req.target_type);
    drop(registry);

    // Filter out ghost candidates with no active WebSocket connection.
    // ActorRegistry may briefly retain offline entries due to network disruptions,
    // Cross-checking against the clients map eliminates them.
    let connected_actor_ids: std::collections::HashSet<ActrId> = {
        let clients = server.clients.read().await;
        clients
            .values()
            .filter_map(|c| c.actor_id.clone())
            .collect()
    };
    let candidates: Vec<_> = candidates_raw
        .into_iter()
        .filter(|c| {
            let connected = connected_actor_ids.contains(&c.actor_id);
            if !connected {
                platform::recording::warn!(
                    "🚫 Filtered ghost candidate: Actor {} (no active connection)",
                    c.actor_id.to_string_repr()
                );
            }
            connected
        })
        .collect();

    let total_candidates = candidates.len();

    if candidates.is_empty() {
        platform::recording::info!(
            "⚠️  No service instances found for type {}/{}",
            req.target_type.publisher,
            req.target_type.name
        );
    } else {
        platform::recording::info!(
            "📋 Found {} candidate instances for type {}/{}",
            total_candidates,
            req.target_type.publisher,
            req.target_type.name
        );
    }

    // Apply ACL filtering
    use platform::realm::acl::ActorAcl;
    let source_realm = &source.realm;
    let source_type = type_key(source_type);

    let mut acl_filtered_candidates = Vec::new();
    for candidate in candidates {
        let target_realm = &candidate.actor_id.realm;
        let target_type = type_key(&candidate.actor_type);

        match ActorAcl::can_discover(source_realm, target_realm, &source_type, &target_type).await {
            Ok(true) => acl_filtered_candidates.push(candidate),
            Ok(false) => {
                platform::recording::debug!(
                    "ACL denied route candidate: {} (realm {}) cannot access {} (realm {})",
                    source.to_string_repr(),
                    source_realm,
                    candidate.actor_id.to_string_repr(),
                    target_realm
                );
            }
            Err(e) => {
                platform::recording::warn!(
                    "ACL check failed for {} (realm {}) -> {} (realm {}): {}",
                    source.to_string_repr(),
                    source_realm,
                    candidate.actor_id.to_string_repr(),
                    target_realm,
                    e
                );
            }
        }
    }
    platform::recording::info!(
        "ACL filtering for route candidates: {} -> {} candidates",
        total_candidates,
        acl_filtered_candidates.len()
    );

    // Extract client location from request (if provided)
    let client_location = req.client_location.as_ref().and_then(|loc| {
        if let (Some(lat), Some(lon)) = (loc.latitude, loc.longitude) {
            Some((lat, lon))
        } else {
            None
        }
    });

    // Extract ws_address (rank_candidates moves candidates, collect beforehand)
    let ws_address_map: std::collections::HashMap<ActrId, Option<String>> = acl_filtered_candidates
        .iter()
        .map(|c| (c.actor_id.clone(), c.ws_address.clone()))
        .collect();

    // Ranking: ACL filtered, same ActrType means protocol compatible, apply ranking_factors in order
    // is_exact_match flag handled internally by LB via client_fingerprint
    // Cap arbitrary wire requests before ranking/encoding response candidates.
    let mut criteria = req.criteria.clone().unwrap_or_else(|| {
        actr_protocol::route_candidates_request::NodeSelectionCriteria {
            candidate_count: MAX_ROUTE_CANDIDATES,
            ..Default::default()
        }
    });
    criteria.candidate_count = criteria.candidate_count.min(MAX_ROUTE_CANDIDATES);
    let ranked_actor_ids = LoadBalancer::rank_candidates(
        &acl_filtered_candidates,
        Some(&criteria),
        req.client_fingerprint.trim(),
        Some(client_id),
        client_location,
    );

    platform::recording::info!(
        "✅ Returned {} candidates for Actor {}",
        source.to_string_repr(),
        ranked_actor_ids.len(),
    );

    let ws_address_map: Vec<actr_protocol::WsAddressEntry> = ranked_actor_ids
        .iter()
        .filter_map(|id| {
            ws_address_map
                .get(id)
                .map(|ws| actr_protocol::WsAddressEntry {
                    candidate_id: id.clone(),
                    ws_address: ws.clone(),
                })
        })
        .collect();

    let response = actr_protocol::RouteCandidatesResponse {
        result: Some(actr_protocol::route_candidates_response::Result::Success(
            actr_protocol::route_candidates_response::RouteCandidatesOk {
                candidates: ranked_actor_ids,
                ws_address_map,
            },
        )),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::RouteCandidatesResponse(
            response,
        )),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle GetServiceSpec request
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_get_service_spec_request(
    source: ActrId,
    req: actr_protocol::GetServiceSpecRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let service_name = req.name.as_str();
    platform::recording::info!(
        "Handle GetServiceSpec request for Actor {} name={}",
        source.to_string_repr(),
        service_name
    );

    // Find matching ServiceSpec in ActorRegistry by spec name, only among
    // services of the requester's own Realm.
    let result = server
        .actor_registry
        .read()
        .await
        .discover_by_spec_name(&source.realm, service_name)
        .into_iter()
        .find_map(|service| service.service_spec.clone())
        .map(actr_protocol::get_service_spec_response::Result::Success)
        .unwrap_or_else(|| {
            actr_protocol::get_service_spec_response::Result::Error(ErrorResponse {
                code: 404,
                message: format!("Service specification not found for name={service_name}"),
            })
        });

    let response = actr_protocol::GetServiceSpecResponse {
        result: Some(result),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::GetServiceSpecResponse(response)),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle GetSigningKey request — proxies key lookup via local KeyCache, within the
/// requester's Realm
async fn handle_get_signing_key_request(
    source: ActrId,
    req: actr_protocol::GetSigningKeyRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let key_id = req.key_id;
    platform::recording::debug!(
        "Actor {} requested signing key: key_id={}",
        source.to_string_repr(),
        key_id
    );

    // Only the requester's own Realm's issuer keys are served.
    let response = match AIdCredentialValidator::get_key_bytes(&source.realm, key_id).await {
        Ok(Some(pubkey_bytes)) => actr_protocol::GetSigningKeyResponse {
            key_id,
            pubkey: pubkey_bytes.into(),
        },
        Ok(None) => {
            platform::recording::warn!(
                "GetSigningKeyRequest: key_id={} not found in cache",
                key_id
            );
            send_error_response(
                client_id,
                &source,
                404,
                &format!("signing key not found: key_id={key_id}"),
                server,
                Some(request_envelope_id),
            )
            .await?;
            return Ok(());
        }
        Err(e) => {
            platform::recording::error!(
                "GetSigningKeyRequest: key_id={} lookup error: {}",
                key_id,
                e
            );
            send_error_response(
                client_id,
                &source,
                500,
                "internal error looking up signing key",
                server,
                Some(request_envelope_id),
            )
            .await?;
            return Ok(());
        }
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::GetSigningKeyResponse(response)),
    });
    let envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, envelope, server).await?;

    Ok(())
}

/// Handle subscribe to Actor up events
#[cfg_attr(feature = "opentelemetry", instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_subscribe_actr_up(
    source: ActrId,
    source_type: ActrType,
    req: actr_protocol::SubscribeActrUpRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "📢 Actor {} subscribed to service up events: target_type={}/{}",
        source.to_string_repr(),
        req.target_type.publisher,
        req.target_type.name
    );

    // Add subscription to PresenceManager
    let mut presence = server.presence_manager.write().await;
    presence.subscribe(source.clone(), source_type, req.target_type);
    drop(presence);

    let response = actr_protocol::SubscribeActrUpResponse {
        result: Some(actr_protocol::subscribe_actr_up_response::Result::Success(
            actr_protocol::subscribe_actr_up_response::SubscribeOk {},
        )),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::SubscribeActrUpResponse(
            response,
        )),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Handle unsubscribe from Actor up events
#[cfg_attr(feature = "opentelemetry", tracing::instrument(level = "debug", skip_all, fields(client_id, envelope_id = request_envelope_id)))]
async fn handle_unsubscribe_actr_up(
    source: ActrId,
    req: actr_protocol::UnsubscribeActrUpRequest,
    client_id: &str,
    server: &SignalingServerHandle,
    request_envelope_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    platform::recording::info!(
        "🔕 Actor {} unsubscribed from service up events: target_type={}/{}",
        source.to_string_repr(),
        req.target_type.publisher,
        req.target_type.name
    );

    // Remove subscription from PresenceManager
    let mut presence = server.presence_manager.write().await;
    let removed = presence.unsubscribe(&source, &req.target_type);
    drop(presence);

    if !removed {
        platform::recording::warn!(
            "Actor {} was not subscribed to {}/{}",
            source.to_string_repr(),
            req.target_type.publisher,
            req.target_type.name
        );
    }

    let response = actr_protocol::UnsubscribeActrUpResponse {
        result: Some(
            actr_protocol::unsubscribe_actr_up_response::Result::Success(
                actr_protocol::unsubscribe_actr_up_response::UnsubscribeOk {},
            ),
        ),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: source,
        payload: Some(signaling_to_actr::Payload::UnsubscribeActrUpResponse(
            response,
        )),
    });

    let response_envelope = server.create_envelope(flow, Some(request_envelope_id));
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

/// Send generic error response
#[cfg_attr(feature = "opentelemetry", tracing::instrument(level = "debug", skip_all, fields(client_id, reply_for = ?reply_for, target = ?target)))]
async fn send_error_response(
    client_id: &str,
    target: &ActrId,
    code: u32,
    message: &str,
    server: &SignalingServerHandle,
    reply_for: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let error_response = ErrorResponse {
        code,
        message: message.to_string(),
    };

    let flow = signaling_envelope::Flow::ServerToActr(SignalingToActr {
        target: target.clone(),
        payload: Some(signaling_to_actr::Payload::Error(error_response)),
    });

    let response_envelope = server.create_envelope(flow, reply_for);
    send_envelope_to_client(client_id, response_envelope, server).await?;

    Ok(())
}

// Main function removed - SignalingServer can now be instantiated and started from other modules

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Create test ActrId
    fn create_test_actr_id(serial: u64) -> ActrId {
        ActrId {
            serial_number: serial,
            realm: "test@example.com".to_string(),
        }
    }

    fn create_test_actr_type() -> ActrType {
        ActrType {
            publisher: "test".to_string(),
            name: "device".to_string(),
            version: "1.0.0".to_string(),
        }
    }

    /// Create test ClientConnection
    fn create_test_client(actor_id: ActrId, webrtc_role: Option<String>) -> ClientConnection {
        create_test_client_with_id(&uuid::Uuid::new_v4().to_string(), actor_id, webrtc_role)
    }

    /// Build a test egress handle that a background task continuously drains (simulating a healthy consumer).
    fn create_test_egress() -> ClientEgress {
        create_test_egress_with_config(EgressQueueConfig::default())
    }

    fn create_test_egress_with_config(config: EgressQueueConfig) -> ClientEgress {
        let (egress, mut data_rx, mut control_rx) =
            ClientEgress::new(config, Arc::new(SignalingQueueMetrics::default()));
        if tokio::runtime::Handle::try_current().is_ok() {
            // Async test: a background task drains continuously, simulating a healthy consumer.
            tokio::spawn(async move { while data_rx.recv().await.is_some() {} });
            tokio::spawn(async move { while control_rx.recv().await.is_some() {} });
        } else {
            // Sync test (no runtime): leak the receiver to keep the channel open (without actually consuming).
            std::mem::forget(data_rx);
            std::mem::forget(control_rx);
        }
        egress
    }

    fn create_test_client_with_id(
        id: &str,
        actor_id: ActrId,
        webrtc_role: Option<String>,
    ) -> ClientConnection {
        ClientConnection {
            id: id.to_string(),
            actor_id: Some(actor_id),
            credential: None,
            egress: create_test_egress(),
            client_ip: None,
            webrtc_role,
        }
    }

    fn create_test_server_handle() -> SignalingServerHandle {
        create_test_server_handle_with_config(EgressQueueConfig::default())
    }

    fn create_test_server_handle_with_config(
        egress_config: EgressQueueConfig,
    ) -> SignalingServerHandle {
        let mut server = SignalingServer::with_egress_config(egress_config);
        server.provider_id = "provider.example".to_string();
        server.handle()
    }

    async fn register_test_service(server: &SignalingServerHandle, actor_id: &ActrId) {
        server
            .actor_registry
            .write()
            .await
            .register_service(
                actor_id.clone(),
                create_test_actr_type(),
                "test_service".to_string(),
                vec!["TestMessage".to_string()],
                None,
            )
            .unwrap();
    }

    #[tokio::test]
    async fn displaced_unregister_preserves_replacement_and_durable_discovery() {
        use crate::actor_registry_storage::ActorRegistryStorage;
        let server = create_test_server_handle();
        let actor = create_test_actr_id(812);
        server.clients.write().await.insert(
            "old".into(),
            create_test_client_with_id("old", actor.clone(), None),
        );
        register_test_service(&server, &actor).await;
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            ActorRegistryStorage::new(dir.path().join("actors.db"), None)
                .await
                .unwrap(),
        );

        // Pause the actual old handler at its ownership lock, then install a
        // replacement before releasing it. No timing sleeps or scheduler races.
        let mut clients = server.clients.write().await;
        let mut old_request = Box::pin(handle_unregister(
            actor.clone(),
            Default::default(),
            "old",
            &server,
            "old-request",
        ));
        assert!(futures_util::poll!(&mut old_request).is_pending());
        clients.remove("old");
        clients.insert(
            "new".into(),
            create_test_client_with_id("new", actor.clone(), None),
        );
        server
            .actor_id_index
            .write()
            .await
            .insert(actor.clone(), "new".into());
        register_test_service(&server, &actor).await;
        let service = server
            .actor_registry
            .read()
            .await
            .find_by_actr_type(&actor.realm, &create_test_actr_type())[0]
            .clone();
        storage.save_service(&service).await.unwrap();
        server
            .actor_registry
            .write()
            .await
            .set_storage(storage.clone());
        drop(clients);
        old_request.await.unwrap();
        assert!(server.clients.read().await.contains_key("new"));
        assert_eq!(
            server
                .actor_id_index
                .read()
                .await
                .get(&actor)
                .map(String::as_str),
            Some("new")
        );
        assert_eq!(
            server.actor_registry.read().await.get_actor_type(&actor),
            Some(&create_test_actr_type())
        );
        assert_eq!(
            storage
                .load_services_by_actor_id(&actor)
                .await
                .unwrap()
                .len(),
            1
        );

        handle_unregister(
            actor.clone(),
            Default::default(),
            "new",
            &server,
            "current-request",
        )
        .await
        .unwrap();
        assert!(
            server
                .actor_registry
                .read()
                .await
                .get_actor_type(&actor)
                .is_none()
        );
        assert!(
            storage
                .load_services_by_actor_id(&actor)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn over_limit_messages_do_not_run_connection_authorization() {
        let mut server = create_test_server_handle();
        let limiter = Arc::new(crate::ratelimit::MessageRateLimiter::new(
            platform::config::signaling::MessageRateLimit {
                enabled: true,
                per_second: 1,
                burst_size: 1,
            },
        ));
        limiter.check_message("limited").await.unwrap();
        server.message_rate_limiter = Some(limiter);
        let (egress, mut data_rx, mut control_rx) =
            create_blocked_egress(EgressQueueConfig::default(), server.queue_metrics.clone());
        server.clients.write().await.insert(
            "limited".into(),
            ClientConnection {
                id: "limited".into(),
                actor_id: Some(create_test_actr_id(813)),
                credential: Some(AIdCredential {
                    claims: vec![0xff].into(),
                    ..Default::default()
                }),
                egress,
                client_ip: None,
                webrtc_role: None,
            },
        );
        handle_client_envelope(&[], "limited", &server)
            .await
            .unwrap();
        let queued = data_rx.recv().await.unwrap();
        let WsMessage::Binary(bytes) = queued.msg else {
            panic!("expected binary error")
        };
        let envelope = SignalingEnvelope::decode(bytes.as_ref()).unwrap();
        assert!(matches!(
            envelope.flow,
            Some(signaling_envelope::Flow::EnvelopeError(ErrorResponse {
                code: 429,
                ..
            }))
        ));
        assert!(server.clients.read().await.contains_key("limited"));
        assert!(control_rx.try_recv().is_err());
        // Once admitted, the same invalid connection must still be refused.
        server.message_rate_limiter = None;
        handle_client_envelope(&[], "limited", &server)
            .await
            .unwrap();
        assert!(!server.clients.read().await.contains_key("limited"));
        assert!(matches!(control_rx.recv().await, Some(WsMessage::Close(_))));
    }

    #[test]
    fn test_websocket_ping_enqueues_pong() {
        let (egress, _data_rx, mut control_rx) = ClientEgress::new(
            EgressQueueConfig::default(),
            Arc::new(SignalingQueueMetrics::default()),
        );

        assert!(handle_websocket_control_message(
            WsMessage::Ping(Vec::from("probe").into()),
            &egress,
            "client",
        ));

        match control_rx.try_recv().expect("pong should be queued") {
            WsMessage::Pong(payload) => assert_eq!(payload.as_ref(), b"probe"),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_reconnect_closes_removed_stale_client() {
        let server = create_test_server_handle();
        let actor_id = create_test_actr_id(1);
        let (egress, _data_rx, mut control_rx) = ClientEgress::new(
            EgressQueueConfig::default(),
            Arc::new(SignalingQueueMetrics::default()),
        );

        let mut clients = HashMap::new();
        clients.insert(
            "old-client".to_string(),
            ClientConnection {
                id: "old-client".to_string(),
                actor_id: Some(actor_id.clone()),
                credential: None,
                egress,
                client_ip: None,
                webrtc_role: None,
            },
        );

        let stale_clients = remove_stale_actor_clients(&mut clients, &actor_id);
        assert!(
            !clients.contains_key("old-client"),
            "stale client should be removed from routing map"
        );

        close_stale_clients(stale_clients, &server).await;

        assert!(
            matches!(control_rx.try_recv(), Ok(WsMessage::Close(_))),
            "stale client should receive a WebSocket Close frame"
        );
    }

    #[tokio::test]
    async fn test_cleanup_stale_connection_keeps_replacement_service() {
        let server = create_test_server_handle();
        let actor_id = create_test_actr_id(1);
        register_test_service(&server, &actor_id).await;

        {
            let mut clients = server.clients.write().await;
            clients.insert(
                "old-client".to_string(),
                create_test_client_with_id("old-client", actor_id.clone(), None),
            );
            clients.insert(
                "new-client".to_string(),
                create_test_client_with_id("new-client", actor_id.clone(), None),
            );
        }
        server
            .actor_id_index
            .write()
            .await
            .insert(actor_id.clone(), "new-client".to_string());

        cleanup_client("old-client", &server, CleanupMode::ConnectionClosed).await;

        assert!(
            server.clients.read().await.contains_key("new-client"),
            "replacement client should remain connected"
        );
        assert_eq!(
            server.actor_id_index.read().await.get(&actor_id).cloned(),
            Some("new-client".to_string())
        );
        assert_eq!(
            server
                .actor_registry
                .read()
                .await
                .discover_by_service_name(&actor_id.realm, "test_service")
                .len(),
            1,
            "stale connection cleanup must not unregister a live replacement"
        );
    }

    #[tokio::test]
    async fn disabling_an_actor_drops_only_its_state() {
        let server = create_test_server_handle();
        let disabled = create_test_actr_id(1);
        let other = create_test_actr_id(2);
        for (client_id, actor) in [("disabled", &disabled), ("other", &other)] {
            register_test_service(&server, actor).await;
            server.clients.write().await.insert(
                client_id.to_string(),
                create_test_client_with_id(client_id, actor.clone(), None),
            );
            server
                .actor_id_index
                .write()
                .await
                .insert(actor.clone(), client_id.to_string());
            server.presence_manager.write().await.subscribe(
                actor.clone(),
                create_test_actr_type(),
                create_test_actr_type(),
            );
        }

        disable_actor(&server, &disabled).await;

        assert!(!server.clients.read().await.contains_key("disabled"));
        assert!(server.clients.read().await.contains_key("other"));
        let registry = server.actor_registry.read().await;
        let services = registry.discover_by_service_name(&disabled.realm, "test_service");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].actor_id, other);
        drop(registry);
        let presence = server.presence_manager.read().await;
        let subscribers = presence.get_subscribers(&disabled.realm, &create_test_actr_type());
        assert_eq!(subscribers, vec![&other]);
    }

    #[tokio::test]
    async fn retiring_a_realm_drops_its_state_and_refuses_it_afterwards() {
        let server = create_test_server_handle();
        let retired = create_test_actr_id(1);
        let other = ActrId {
            serial_number: 1,
            realm: "other@example.com".to_string(),
        };
        for (client_id, actor) in [("retired", &retired), ("other", &other)] {
            register_test_service(&server, actor).await;
            server.clients.write().await.insert(
                client_id.to_string(),
                create_test_client_with_id(client_id, actor.clone(), None),
            );
            server
                .actor_id_index
                .write()
                .await
                .insert(actor.clone(), client_id.to_string());
            server.presence_manager.write().await.subscribe(
                actor.clone(),
                create_test_actr_type(),
                create_test_actr_type(),
            );
        }

        retire_realm(&server, &retired.realm).await;
        // Idempotent.
        retire_realm(&server, &retired.realm).await;

        assert!(!server.clients.read().await.contains_key("retired"));
        assert!(server.clients.read().await.contains_key("other"));
        let registry = server.actor_registry.read().await;
        assert!(
            registry
                .discover_by_service_name(&retired.realm, "test_service")
                .is_empty()
        );
        assert_eq!(
            registry
                .discover_by_service_name(&other.realm, "test_service")
                .len(),
            1
        );
        drop(registry);
        let presence = server.presence_manager.read().await;
        assert!(
            presence
                .get_subscribers(&retired.realm, &create_test_actr_type())
                .is_empty()
        );
        assert_eq!(
            presence
                .get_subscribers(&other.realm, &create_test_actr_type())
                .len(),
            1
        );
        drop(presence);

        // A registration or subscription racing the retirement is refused.
        let late = create_test_actr_id(2);
        assert!(
            server
                .actor_registry
                .write()
                .await
                .register_service(
                    late.clone(),
                    create_test_actr_type(),
                    "test_service".to_string(),
                    vec![],
                    None,
                )
                .is_err()
        );
        server.presence_manager.write().await.subscribe(
            late,
            create_test_actr_type(),
            create_test_actr_type(),
        );
        assert!(
            server
                .presence_manager
                .read()
                .await
                .get_subscribers(&retired.realm, &create_test_actr_type())
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_connection_cleanup_removes_service_from_memory_only() {
        let server = create_test_server_handle();
        let actor_id = create_test_actr_id(1);
        register_test_service(&server, &actor_id).await;

        server.clients.write().await.insert(
            "client".to_string(),
            create_test_client_with_id("client", actor_id.clone(), None),
        );
        server
            .actor_id_index
            .write()
            .await
            .insert(actor_id.clone(), "client".to_string());

        cleanup_client("client", &server, CleanupMode::ConnectionClosed).await;

        assert_eq!(
            server
                .actor_registry
                .read()
                .await
                .discover_by_service_name(&actor_id.realm, "test_service")
                .len(),
            0,
            "connection cleanup should remove offline services from memory"
        );
        assert!(
            !server.actor_id_index.read().await.contains_key(&actor_id),
            "connection cleanup should clear the old actor index"
        );
    }

    #[tokio::test]
    async fn test_relay_source_connection_status_allows_current_client() {
        let server = create_test_server_handle();
        let actor_id = create_test_actr_id(1);

        server.clients.write().await.insert(
            "client".to_string(),
            create_test_client_with_id("client", actor_id.clone(), None),
        );
        server
            .actor_id_index
            .write()
            .await
            .insert(actor_id.clone(), "client".to_string());

        assert_eq!(
            relay_source_connection_status("client", &actor_id, &server).await,
            RelaySourceConnectionStatus::Current
        );
    }

    #[tokio::test]
    async fn test_relay_source_connection_status_rejects_displaced_client() {
        let server = create_test_server_handle();
        let actor_id = create_test_actr_id(1);

        {
            let mut clients = server.clients.write().await;
            clients.insert(
                "old-client".to_string(),
                create_test_client_with_id("old-client", actor_id.clone(), None),
            );
            clients.insert(
                "new-client".to_string(),
                create_test_client_with_id("new-client", actor_id.clone(), None),
            );
        }
        server
            .actor_id_index
            .write()
            .await
            .insert(actor_id.clone(), "new-client".to_string());

        assert_eq!(
            relay_source_connection_status("old-client", &actor_id, &server).await,
            RelaySourceConnectionStatus::Displaced {
                current_client_id: "new-client".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_relay_source_connection_status_rejects_actor_mismatch() {
        let server = create_test_server_handle();
        let client_actor = create_test_actr_id(1);
        let relay_source = create_test_actr_id(2);

        server.clients.write().await.insert(
            "client".to_string(),
            create_test_client_with_id("client", client_actor, None),
        );
        server
            .actor_id_index
            .write()
            .await
            .insert(relay_source.clone(), "client".to_string());

        assert_eq!(
            relay_source_connection_status("client", &relay_source, &server).await,
            RelaySourceConnectionStatus::ClientActorMismatch
        );
    }

    #[test]
    fn test_determine_webrtc_role() {
        // Test data: (from_serial, to_serial, from_role, to_role, expected_is_offerer, description)
        let test_cases = vec![
            (
                2,
                1,
                None,
                None,
                true,
                "Neither has preference, use ActorId reverse order",
            ),
            (
                2,
                1,
                None,
                Some("answer"),
                true,
                "Receiver prefers answer, initiator should be offerer",
            ),
            (
                2,
                1,
                Some("answer"),
                None,
                false,
                "Initiator prefers answer, initiator should be answerer",
            ),
            (
                2,
                1,
                Some("answer"),
                Some("answer"),
                true,
                "Both prefer answer, fall back to ActorId reverse order",
            ),
            (
                1,
                2,
                None,
                None,
                false,
                "serial 1 < serial 2, initiator should be answerer",
            ),
            (
                1,
                2,
                Some("answer"),
                Some("answer"),
                false,
                "Both prefer answer, serial 1 should be answerer",
            ),
        ];

        for (from_serial, to_serial, from_role, to_role, expected, desc) in test_cases {
            let from = create_test_actr_id(from_serial);
            let to = create_test_actr_id(to_serial);

            // Create clients map
            let mut clients = HashMap::new();
            clients.insert(
                format!("client_{from_serial}"),
                create_test_client(from.clone(), from_role.map(|s| s.to_string())),
            );
            clients.insert(
                format!("client_{to_serial}"),
                create_test_client(to.clone(), to_role.map(|s| s.to_string())),
            );

            let is_offerer = determine_webrtc_role(&from, &to, &clients);

            assert_eq!(
                is_offerer, expected,
                "Test failed: {desc} (from={from_serial}, to={to_serial}, from_role={from_role:?}, to_role={to_role:?})"
            );
        }
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Bounded egress queue (issue #351)
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// Configuration with a high egress bound + high aggregate budget (the queue bound is the only constraint, for saturation/disconnect tests).
    fn bounded_egress_config() -> EgressQueueConfig {
        EgressQueueConfig {
            max_messages: 2,
            max_bytes: 1 << 20,
            max_control_messages: 4,
            max_frame_bytes: 256 << 10,
            enqueue_timeout_ms: 50,
            write_timeout_ms: 100,
            target_rate_per_second: 100_000,
            target_bytes_per_second: 1 << 24,
        }
    }

    /// Build an egress handle that is never drained (simulating a blocked/slow consumer); the receiver is returned for the test to hold.
    fn create_blocked_egress(
        config: EgressQueueConfig,
        metrics: Arc<SignalingQueueMetrics>,
    ) -> (
        ClientEgress,
        tokio::sync::mpsc::Receiver<QueuedFrame>,
        tokio::sync::mpsc::Receiver<WsMessage>,
    ) {
        ClientEgress::new(config, metrics)
    }

    /// Build an egress handle that is continuously drained and counted (simulating a healthy consumer).
    fn create_counting_egress(
        config: EgressQueueConfig,
        metrics: Arc<SignalingQueueMetrics>,
    ) -> (ClientEgress, Arc<AtomicU64>) {
        let (egress, mut data_rx, mut control_rx) = ClientEgress::new(config, metrics);
        let received = Arc::new(AtomicU64::new(0));
        let r = received.clone();
        tokio::spawn(async move {
            while data_rx.recv().await.is_some() {
                r.fetch_add(1, Ordering::Relaxed);
            }
        });
        tokio::spawn(async move { while control_rx.recv().await.is_some() {} });
        (egress, received)
    }

    async fn insert_client(server: &SignalingServerHandle, client_id: &str, egress: ClientEgress) {
        server.clients.write().await.insert(
            client_id.to_string(),
            ClientConnection {
                id: client_id.to_string(),
                actor_id: None,
                credential: None,
                egress,
                client_ip: None,
                webrtc_role: None,
            },
        );
    }

    /// Sustained flood: a blocked consumer cannot push the queue past the configured message-count bound; once over, enqueue reports saturation.
    #[tokio::test]
    async fn test_egress_bounded_by_message_count() {
        let metrics = Arc::new(SignalingQueueMetrics::default());
        let (egress, _data_rx, _control_rx) =
            create_blocked_egress(bounded_egress_config(), metrics.clone());

        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();
        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();

        // The third message exceeds max_messages=2; the blocked consumer does not recover within 50ms -> saturated
        let result = egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await;
        assert_eq!(result, Err(EnqueueError::Saturated));
        let snap = metrics.snapshot();
        assert_eq!(snap.egress_saturation_events, 1);
        assert_eq!(snap.egress_enqueued, 2);
    }

    /// The byte budget takes effect before the message-count bound.
    #[tokio::test]
    async fn test_egress_bounded_by_byte_budget() {
        let metrics = Arc::new(SignalingQueueMetrics::default());
        let mut config = bounded_egress_config();
        config.max_messages = 16;
        config.max_bytes = 100;
        let (egress, _data_rx, _control_rx) = create_blocked_egress(config, metrics.clone());

        egress
            .send_data(WsMessage::Binary(vec![0u8; 60].into()), 60)
            .await
            .unwrap();
        // A second 60-byte frame would bring the total to 120 > 100: blocked by the byte budget (even though only 1/16 slots are used)
        let result = egress
            .send_data(WsMessage::Binary(vec![0u8; 60].into()), 60)
            .await;
        assert_eq!(result, Err(EnqueueError::Saturated));
    }

    /// When the data queue is full, control messages (Pong) still enter the independent control queue (not starved by data).
    #[tokio::test]
    async fn test_control_messages_not_starved_by_full_data_queue() {
        let metrics = Arc::new(SignalingQueueMetrics::default());
        let (egress, _data_rx, mut control_rx) =
            create_blocked_egress(bounded_egress_config(), metrics);

        // Fill the data queue
        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();
        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();

        // Even with the data queue full, the Pong for a Ping still enters the control queue
        assert!(handle_websocket_control_message(
            WsMessage::Ping(Vec::from("probe").into()),
            &egress,
            "client"
        ));
        match control_rx.try_recv() {
            Ok(WsMessage::Pong(payload)) => assert_eq!(payload.as_ref(), b"probe"),
            other => panic!("expected queued Pong despite full data queue, got {other:?}"),
        }
    }

    /// A replaced or retired connection whose control queue is full still
    /// ends: no Close frame fits, so the write task stops at once instead of
    /// leaving the socket open until the client speaks.
    #[tokio::test]
    async fn a_close_that_cannot_be_queued_still_ends_the_connection() {
        let server = create_test_server_handle();
        let config = EgressQueueConfig {
            max_control_messages: 1,
            ..EgressQueueConfig::default()
        };
        let full_control_queue = || {
            let (egress, data_rx, control_rx) =
                ClientEgress::new(config.clone(), Arc::new(SignalingQueueMetrics::default()));
            egress
                .try_send_control(WsMessage::Pong(Vec::from("queued").into()))
                .unwrap();
            (egress, data_rx, control_rx)
        };

        // Replaced by a new connection of the same actor.
        let (replaced, data_rx, control_rx) = full_control_queue();
        close_stale_clients(
            vec![StaleClient {
                id: "replaced".to_string(),
                egress: replaced.clone(),
            }],
            &server,
        )
        .await;
        tokio::time::timeout(
            Duration::from_secs(1),
            write_egress(
                futures_util::sink::drain(),
                data_rx,
                control_rx,
                replaced.closed.clone(),
                Duration::from_secs(1),
            ),
        )
        .await
        .expect("the write task ends although the replaced client never reads or speaks");

        // Its Realm retired.
        let actor = create_test_actr_id(1);
        let (retired, _data_rx, _control_rx) = full_control_queue();
        server.clients.write().await.insert(
            "retired".to_string(),
            ClientConnection {
                id: "retired".to_string(),
                actor_id: Some(actor.clone()),
                credential: None,
                egress: retired.clone(),
                client_ip: None,
                webrtc_role: None,
            },
        );
        retire_realm(&server, &actor.realm).await;
        assert!(retired.closed.is_cancelled());

        // A Close frame that fits is sent instead, and ends the connection.
        let (egress, data_rx, mut control_rx) =
            ClientEgress::new(config.clone(), Arc::new(SignalingQueueMetrics::default()));
        assert!(egress.close(axum::extract::ws::CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: "test".into(),
        }));
        assert!(!egress.closed.is_cancelled());
        assert!(matches!(control_rx.try_recv(), Ok(WsMessage::Close(_))));
        drop(data_rx);
    }

    /// Blocked socket: a slow-consumer client is closed after the enqueue timeout and removed from the routing table (acceptance criteria 3+4).
    #[tokio::test]
    async fn test_slow_consumer_disconnected_and_removed() {
        let server = create_test_server_handle_with_config(bounded_egress_config());
        let (egress, _data_rx, _control_rx) =
            create_blocked_egress(server.egress_config.clone(), server.queue_metrics.clone());
        insert_client(&server, "slow", egress).await;

        // Fill the queue (2 messages); the third triggers saturation -> close_slow_client
        for _ in 0..2 {
            enqueue_binary_to_client("slow", vec![0u8; 8], &server)
                .await
                .unwrap();
        }
        enqueue_binary_to_client("slow", vec![0u8; 8], &server)
            .await
            .unwrap();

        assert!(
            server.clients.read().await.get("slow").is_none(),
            "slow consumer should be removed from the routing map"
        );
        let snap = server.queue_metrics.snapshot();
        assert_eq!(snap.slow_consumer_disconnects, 1);
        assert!(snap.egress_saturation_events >= 1);
    }

    /// When a slow-consumer target is disconnected, healthy targets are unaffected (acceptance criterion 4).
    #[tokio::test]
    async fn test_healthy_client_unaffected_by_slow_peer() {
        let server = create_test_server_handle_with_config(bounded_egress_config());

        // Slow (blocked) target
        let (slow_egress, _sd, _sc) =
            create_blocked_egress(server.egress_config.clone(), server.queue_metrics.clone());
        insert_client(&server, "slow", slow_egress).await;

        // Healthy (continuously drained + counted) target
        let (healthy_egress, healthy_received) =
            create_counting_egress(server.egress_config.clone(), server.queue_metrics.clone());
        insert_client(&server, "healthy", healthy_egress).await;

        // Saturate the slow target so it is removed
        for _ in 0..3 {
            enqueue_binary_to_client("slow", vec![0u8; 8], &server)
                .await
                .unwrap();
        }
        assert!(server.clients.read().await.get("slow").is_none());

        // The healthy target still receives messages and stays in the routing table
        for _ in 0..5 {
            enqueue_binary_to_client("healthy", vec![0u8; 8], &server)
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(server.clients.read().await.get("healthy").is_some());
        assert_eq!(healthy_received.load(Ordering::Relaxed), 5);
    }

    /// Multiple senders hitting the same target: the per-target aggregate budget rate-limits them (acceptance criterion 6).
    #[tokio::test]
    async fn test_multi_sender_aggregate_target_budget() {
        // The queue bound is relaxed; the only constraint is the per-target aggregate rate of 3 msg/s
        let mut config = bounded_egress_config();
        config.max_messages = 1000;
        config.target_rate_per_second = 3;
        config.target_bytes_per_second = 1 << 24;
        let server = create_test_server_handle_with_config(config.clone());

        let (egress, received) = create_counting_egress(config, server.queue_metrics.clone());
        insert_client(&server, "target", egress).await;

        // Simulate multiple senders rapidly sending 10 messages (~8 bytes each) to the same target
        for _ in 0..10 {
            enqueue_binary_to_client("target", vec![0u8; 8], &server)
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let snap = server.queue_metrics.snapshot();
        let got = received.load(Ordering::Relaxed);
        // Aggregate budget 3/s (burst 3): most are rejected; the target receives only a few
        assert!(
            snap.target_budget_rejections >= 5,
            "expected aggregate budget to reject most messages, got rejections={}",
            snap.target_budget_rejections
        );
        assert!(
            got <= 5,
            "target should receive only a few messages, got {got}"
        );
        assert_eq!(got + snap.target_budget_rejections, 10);
    }

    /// Recovery after saturation: once the consumer drains, the producer can enqueue again (acceptance criterion 10).
    #[tokio::test]
    async fn test_egress_recovers_after_consumer_drains() {
        let metrics = Arc::new(SignalingQueueMetrics::default());
        let (egress, mut data_rx, _control_rx) =
            create_blocked_egress(bounded_egress_config(), metrics);

        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();
        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();
        assert_eq!(
            egress
                .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
                .await,
            Err(EnqueueError::Saturated)
        );

        // Consumer drains -> slots and byte budget recover
        let _ = data_rx.recv().await;
        egress
            .send_data(WsMessage::Binary(vec![0u8; 8].into()), 8)
            .await
            .unwrap();
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Relay: credential before ACL, source type from claims
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    mod relay {
        use super::*;
        use actr_protocol::IdentityClaims;
        use ed25519_dalek::{Signer, SigningKey};
        use platform::realm::acl::ActorAcl;
        use serial_test::serial;
        use tokio::sync::OnceCell;

        const REALM: &str = "test@example.com";
        const KEY_ID: u32 = 41;

        fn edge() -> ActrType {
            ActrType {
                publisher: "acme".to_string(),
                name: "edge".to_string(),
                version: "1.0.0".to_string(),
            }
        }

        fn worker() -> ActrType {
            ActrType {
                publisher: "acme".to_string(),
                name: "worker".to_string(),
                version: "1.0.0".to_string(),
            }
        }

        fn signing_key() -> SigningKey {
            SigningKey::from_bytes(&[41u8; 32])
        }

        fn now() -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        }

        async fn setup() {
            static INIT: OnceCell<tempfile::TempDir> = OnceCell::const_new();
            INIT.get_or_init(|| async {
                let dir = tempfile::tempdir().unwrap();
                if let Err(e) = platform::storage::db::set_db_path(dir.path()).await {
                    assert!(e.to_string().contains("already"), "{e}");
                }
                AIdCredentialValidator::init(dir.path()).await.unwrap();
                AIdCredentialValidator::populate_key(
                    REALM,
                    KEY_ID,
                    &signing_key().verifying_key(),
                    now() + 3600,
                )
                .await
                .unwrap();
                if RealmEntity::get(REALM).await.unwrap().is_none() {
                    RealmEntity::create(REALM.to_string()).await.unwrap();
                }
                let rule = ActorAcl::new(
                    REALM.to_string(),
                    edge().to_string_repr(),
                    worker().to_string_repr(),
                    true,
                );
                ActorAcl::replace_for_target(REALM, &worker().to_string_repr(), &[rule])
                    .await
                    .unwrap();
                dir
            })
            .await;
        }

        #[tokio::test]
        #[serial]
        async fn route_candidate_responses_are_bounded_before_encoding() {
            setup().await;
            let server = create_test_server_handle();
            let mut source = connect(&server, 900, "lookup-source").await;
            for n in 0..80 {
                let peer = connect(&server, 1000 + n, &format!("candidate-{n}")).await;
                register(&server, &peer.id, worker()).await;
            }
            for count in [2, u32::MAX] {
                handle_route_candidates_request(
                    source.id.clone(),
                    &edge(),
                    actr_protocol::RouteCandidatesRequest {
                        target_type: worker(),
                        criteria: Some(
                            actr_protocol::route_candidates_request::NodeSelectionCriteria {
                                candidate_count: count,
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    },
                    "lookup-source",
                    &server,
                    "lookup-request",
                )
                .await
                .unwrap();
                let Some(signaling_envelope::Flow::ServerToActr(SignalingToActr {
                    payload: Some(signaling_to_actr::Payload::RouteCandidatesResponse(response)),
                    ..
                })) = next_flow(&mut source)
                else {
                    panic!("missing response")
                };
                let Some(actr_protocol::route_candidates_response::Result::Success(ok)) =
                    response.result
                else {
                    panic!("lookup failed")
                };
                assert_eq!(
                    ok.candidates.len(),
                    count.min(MAX_ROUTE_CANDIDATES) as usize
                );
                assert_eq!(ok.ws_address_map.len(), ok.candidates.len());
            }
        }

        fn credential(actor: &ActrId, actor_type: ActrType) -> AIdCredential {
            let claims = IdentityClaims {
                realm: REALM.to_string(),
                actor_id: actor.to_string_repr(),
                expires_at: now() + 3600,
                protocol_version: 1,
                issuer: REALM.to_string(),
                provider: "provider.example".to_string(),
                audience: "actr.signaling".to_string(),
                actor_type,
                member: "member-1".to_string(),
                client_key: "0000000000000000".to_string(),
                role: actr_protocol::MemberRole::Owner as i32,
                development: None,
            };
            let bytes = claims.encode_to_vec();
            AIdCredential {
                key_id: KEY_ID,
                signature: signing_key().sign(&bytes).to_bytes().to_vec().into(),
                claims: bytes.into(),
            }
        }

        struct Peer {
            id: ActrId,
            data_rx: tokio::sync::mpsc::Receiver<QueuedFrame>,
            _control_rx: tokio::sync::mpsc::Receiver<WsMessage>,
        }

        async fn connect(server: &SignalingServerHandle, serial: u64, client_id: &str) -> Peer {
            let id = create_test_actr_id(serial);
            let (egress, data_rx, control_rx) =
                create_blocked_egress(EgressQueueConfig::default(), server.queue_metrics.clone());
            server.clients.write().await.insert(
                client_id.to_string(),
                ClientConnection {
                    id: client_id.to_string(),
                    actor_id: Some(id.clone()),
                    credential: None,
                    egress,
                    client_ip: None,
                    webrtc_role: None,
                },
            );
            server
                .actor_id_index
                .write()
                .await
                .insert(id.clone(), client_id.to_string());
            Peer {
                id,
                data_rx,
                _control_rx: control_rx,
            }
        }

        async fn register(server: &SignalingServerHandle, id: &ActrId, actor_type: ActrType) {
            server
                .actor_registry
                .write()
                .await
                .register_service(id.clone(), actor_type, "svc".to_string(), vec![], None)
                .unwrap();
        }

        fn next_flow(peer: &mut Peer) -> Option<signaling_envelope::Flow> {
            let frame = peer.data_rx.try_recv().ok()?;
            let WsMessage::Binary(bytes) = frame.msg else {
                panic!("expected a binary frame");
            };
            SignalingEnvelope::decode(bytes.as_ref()).unwrap().flow
        }

        fn error_code(flow: Option<signaling_envelope::Flow>) -> u32 {
            match flow {
                Some(signaling_envelope::Flow::ServerToActr(SignalingToActr {
                    payload: Some(signaling_to_actr::Payload::Error(error)),
                    ..
                })) => error.code,
                other => panic!("expected an error response, got {other:?}"),
            }
        }

        fn relay(source: &ActrId, target: &ActrId, credential: AIdCredential) -> ActrRelay {
            ActrRelay {
                source: source.clone(),
                credential,
                target: target.clone(),
                delivery_context: None,
                source_type: Some(worker()),
                payload: Some(actr_relay::Payload::IceCandidate(
                    actr_protocol::IceCandidate {
                        candidate: "candidate".to_string(),
                        ..Default::default()
                    },
                )),
            }
        }

        async fn send(server: &SignalingServerHandle, client_id: &str, relay: ActrRelay) {
            handle_actr_relay(
                relay,
                client_id,
                server,
                "req",
                #[cfg(feature = "opentelemetry")]
                opentelemetry::Context::new(),
            )
            .await
            .unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn invalid_credential_is_rejected_before_acl() {
            setup().await;
            let server = create_test_server_handle();
            let mut source = connect(&server, 1, "source").await;
            let target = connect(&server, 2, "target").await;
            // No ACL rule allows edge -> this type, so an ACL-first path would answer 403.
            register(&server, &target.id, edge()).await;

            let mut forged = credential(&source.id, edge());
            forged.signature = vec![0u8; 64].into();
            send(&server, "source", relay(&source.id, &target.id, forged)).await;

            assert_eq!(error_code(next_flow(&mut source)), 401);
        }

        #[tokio::test]
        #[serial]
        async fn acl_uses_claims_type_and_registry_target_type() {
            setup().await;
            let server = create_test_server_handle();
            let mut source = connect(&server, 1, "source").await;
            let mut target = connect(&server, 2, "target").await;
            register(&server, &target.id, edge()).await;

            // The claims say worker; there is no worker -> edge rule.
            let cred = credential(&source.id, worker());
            send(&server, "source", relay(&source.id, &target.id, cred)).await;

            assert_eq!(error_code(next_flow(&mut source)), 403);
            assert!(next_flow(&mut target).is_none());
        }

        #[tokio::test]
        #[serial]
        async fn forwarded_relay_carries_claims_source_type() {
            setup().await;
            let server = create_test_server_handle();
            let mut source = connect(&server, 1, "source").await;
            let mut target = connect(&server, 2, "target").await;
            register(&server, &target.id, worker()).await;

            let cred = credential(&source.id, edge());
            send(&server, "source", relay(&source.id, &target.id, cred)).await;

            assert!(next_flow(&mut source).is_none());
            let Some(signaling_envelope::Flow::ActrRelay(forwarded)) = next_flow(&mut target)
            else {
                panic!("expected the relay to be forwarded");
            };
            assert_eq!(forwarded.source, source.id);
            assert_eq!(forwarded.source_type, Some(edge()));
            assert_eq!(forwarded.credential, AIdCredential::default());
        }

        #[tokio::test]
        #[serial]
        async fn envelopes_naming_another_actor_or_realm_are_rejected() {
            setup().await;
            let server = create_test_server_handle();
            let mut source = connect(&server, 1, "source").await;
            let mut target = connect(&server, 2, "target").await;
            register(&server, &target.id, worker()).await;
            // Same serial as the source, another Realm.
            let elsewhere = ActrId {
                realm: "other@example.com".to_string(),
                serial_number: 1,
            };

            let envelopes = [
                signaling_envelope::Flow::ActrRelay(relay(
                    &source.id,
                    &elsewhere,
                    credential(&source.id, edge()),
                )),
                signaling_envelope::Flow::ActrRelay(relay(
                    &elsewhere,
                    &target.id,
                    credential(&source.id, edge()),
                )),
                signaling_envelope::Flow::ActrToServer(ActrToSignaling {
                    source: elsewhere.clone(),
                    credential: credential(&source.id, edge()),
                    payload: Some(actr_to_signaling::Payload::DiscoveryRequest(
                        actr_protocol::DiscoveryRequest::default(),
                    )),
                }),
                // Another Actor of the same Realm, with its own valid credential.
                signaling_envelope::Flow::ActrToServer(ActrToSignaling {
                    source: target.id.clone(),
                    credential: credential(&target.id, worker()),
                    payload: Some(actr_to_signaling::Payload::DiscoveryRequest(
                        actr_protocol::DiscoveryRequest::default(),
                    )),
                }),
            ];
            for flow in envelopes {
                let bytes = server.create_new_envelope(flow).encode_to_vec();
                handle_client_envelope(&bytes, "source", &server)
                    .await
                    .unwrap();
                assert_eq!(error_code(next_flow(&mut source)), 403);
            }
            assert!(next_flow(&mut target).is_none());
        }

        #[tokio::test]
        #[serial]
        async fn a_replaced_server_still_closes_connections_of_retired_realms() {
            setup().await;
            let realm = "replaced@example.com";
            RealmEntity::create(realm.to_string()).await.unwrap();
            let server = create_test_server_handle();
            let actor = ActrId {
                serial_number: 1,
                realm: realm.to_string(),
            };
            server.clients.write().await.insert(
                "old".to_string(),
                create_test_client_with_id("old", actor, None),
            );

            // A configuration reload cancels this server; its connection stays.
            let cancel = tokio_util::sync::CancellationToken::new();
            let follower = tokio::spawn(follow_realm_retirements(server.clone(), cancel.clone()));
            cancel.cancel();
            tokio::task::yield_now().await;

            RealmEntity::retire(realm).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while server.clients.read().await.contains_key("old") {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the retired Realm's connection is dropped");
            // With no connections left, the follower ends.
            tokio::time::timeout(Duration::from_secs(5), follower)
                .await
                .expect("the follower ends")
                .unwrap();
        }
    }
}
