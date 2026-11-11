//! Mock loran server for Actor-RTC integration tests.
//!
//! Provides a drop-in replacement for a real `loran` instance, covering the
//! endpoints that `actr`'s `Hyper` runtime and the `actr` CLI talk to:
//!
//! - WebSocket signaling at `/signaling/ws` (also `/` for legacy tests)
//!   handles `PeerToSignaling(RegisterRequest)`,
//!   `ActrToSignaling(Ping|RouteCandidatesRequest|Unregister|...)` and
//!   `ActrRelay` forwarding.
//! - HTTP Issuer endpoints at `/issuer/register` + `/register` (protobuf body).
//! - HTTP Publisher Registry registry endpoints at `/issuer/registry/publishers/{name}/verifying-key` +
//!   `/registry/publishers/{name}/verifying-key` for package verification.
//! - Admin seeding endpoints under `/admin/*` (replaces the old
//!   `register.sh` that used `sqlite3` directly on loran.db).
//!
//! All state lives in memory and is shared between the WS and HTTP surfaces so
//! an Publisher Registry that was registered via `POST /admin/publisher` is immediately visible to
//! `GET /registry/publishers/{name}/verifying-key`, etc.

pub mod http;
mod relay;
pub mod signaling;
pub mod state;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use axum::Router;
use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use state::{MockState, PublisherEntry, RealmEntry};

/// Mock loran server with WebSocket signaling and HTTP Issuer/Publisher Registry endpoints.
///
/// WebSocket and HTTP share a single TCP listener (axum's WS upgrade handler)
/// so callers only need one port.
pub struct MockLoranServer {
    port: u16,
    state: Arc<MockState>,
    cancel: tokio_util::sync::CancellationToken,
    is_running: Arc<AtomicBool>,
    message_count: Arc<AtomicU32>,
    ice_restart_offer_count: Arc<AtomicU32>,
    ice_restart_request_count: Arc<AtomicU32>,
    ice_restart_request_drop_count: Arc<AtomicU32>,
    pause_forwarding: Arc<AtomicBool>,
    ice_candidate_drop_count: Arc<AtomicU32>,
    connection_count: Arc<AtomicU32>,
    disconnection_count: Arc<AtomicU32>,
    relay: Option<Arc<loran_relay::Relay>>,
    relay_frames: Arc<AtomicU32>,
    relay_discovery_enabled: Arc<AtomicBool>,
    relay_discovery_requests: Arc<AtomicU32>,
}

struct RelayFrameObserver(Arc<AtomicU32>);
impl loran_relay::Observer for RelayFrameObserver {
    fn received_frame(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl MockLoranServer {
    /// Start the mock server on a random available port.
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Self::start_with_listener(listener).await
    }

    pub async fn start_with_relay() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Self::start_with_options(listener, true).await
    }

    /// Start the mock server on the specified port.
    pub async fn start_on_port(port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
        Self::start_with_listener(listener).await
    }

    pub async fn start_with_listener(listener: TcpListener) -> anyhow::Result<Self> {
        Self::start_with_options(listener, false).await
    }

    async fn start_with_options(listener: TcpListener, enable_relay: bool) -> anyhow::Result<Self> {
        let port = listener.local_addr()?.port();

        // Deterministic keys so tests can reproduce signatures.
        let issuer_signing_key = SigningKey::from_bytes(&[42u8; 32]);
        let publisher_signing_key = SigningKey::from_bytes(&[43u8; 32]);

        let cancel = tokio_util::sync::CancellationToken::new();
        let state = Arc::new(MockState::new(
            issuer_signing_key,
            1,
            publisher_signing_key,
            cancel.clone(),
        ));

        let is_running = Arc::new(AtomicBool::new(true));

        let message_count = state.message_count.clone();
        let ice_restart_offer_count = state.ice_restart_offer_count.clone();
        let ice_restart_request_count = state.ice_restart_request_count.clone();
        let ice_restart_request_drop_count = state.ice_restart_request_drop_count.clone();
        let pause_forwarding = state.pause_forwarding.clone();
        let ice_candidate_drop_count = state.ice_candidate_drop_count.clone();
        let connection_count = state.connection_count.clone();
        let disconnection_count = state.disconnection_count.clone();

        let mut app = build_router(state.clone());
        let relay_frames = Arc::new(AtomicU32::new(0));
        let relay_discovery_enabled = Arc::new(AtomicBool::new(enable_relay));
        let relay_discovery_requests = Arc::new(AtomicU32::new(0));
        let relay = if enable_relay {
            let relay = loran_relay::Relay::with_observer(
                Default::default(),
                Arc::new(relay::RelayAuth(state.clone())),
                cancel.child_token(),
                Arc::new(RelayFrameObserver(relay_frames.clone())),
            )
            .map_err(anyhow::Error::msg)?;
            let advertised = relay_discovery_enabled.clone();
            let requests = relay_discovery_requests.clone();
            let router = relay.router().route(
                "/config",
                axum::routing::get(move || {
                    let enabled = advertised.load(Ordering::Relaxed);
                    requests.fetch_add(1, Ordering::Relaxed);
                    async move {
                        axum::Json(loran_relay_protocol::RelayDiscovery {
                            version: 1,
                            enabled,
                            url: None,
                        })
                    }
                }),
            );
            // Keep discovery available to browser fixtures on another origin.
            app = app.nest(
                "/relay",
                router.layer(tower_http::cors::CorsLayer::permissive()),
            );
            Some(relay)
        } else {
            None
        };

        let cancel_clone = cancel.clone();
        let is_running_clone = is_running.clone();
        let (ready_tx, ready_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let _ = ready_tx.send(());
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    cancel_clone.cancelled().await;
                })
                .await;
            if let Err(err) = result {
                tracing::error!(%err, "mock-loran axum server exited with error");
            }
            is_running_clone.store(false, Ordering::Release);
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx)
            .await
            .map_err(|_| anyhow::anyhow!("mock-loran server failed to start on port {port}"))?
            .map_err(|_| anyhow::anyhow!("mock-loran startup task exited early"))?;

        tracing::info!(port, "mock-loran listening on 127.0.0.1:{port}");

        Ok(Self {
            port,
            state,
            cancel,
            is_running,
            message_count,
            ice_restart_offer_count,
            ice_restart_request_count,
            ice_restart_request_drop_count,
            pause_forwarding,
            ice_candidate_drop_count,
            connection_count,
            disconnection_count,
            relay,
            relay_frames,
            relay_discovery_enabled,
            relay_discovery_requests,
        })
    }

    /// HTTP base URL (e.g. `http://127.0.0.1:PORT`). This is what callers pass
    /// to `IssuerClient::new()` / `PublisherCertCache::new()`; the server accepts both
    /// `/register` and `/issuer/register` so it also works when callers append
    /// `/issuer` to the endpoint.
    pub fn http_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// WebSocket signaling URL (`ws://127.0.0.1:PORT/signaling/ws`).
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/signaling/ws", self.port)
    }

    /// Legacy helper used by existing hyper integration tests: returns the base
    /// WS URL without the `/signaling/ws` path. The mock server accepts WS
    /// upgrades on both `/` and `/signaling/ws` so this keeps working.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Return the bound port.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn relay_connection_count(&self) -> usize {
        self.relay
            .as_ref()
            .map_or(0, |relay| relay.connection_count())
    }

    pub fn relay_frame_count(&self) -> u32 {
        self.relay_frames.load(Ordering::Relaxed)
    }

    pub fn set_relay_discovery_enabled(&self, enabled: bool) {
        self.relay_discovery_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub fn relay_discovery_requests(&self) -> u32 {
        self.relay_discovery_requests.load(Ordering::Relaxed)
    }

    pub fn disconnect_relay_connections(&self) {
        self.relay
            .as_ref()
            .expect("relay fixture enabled")
            .disconnect_all();
    }

    /// Simulate a temporary certificate enrollment outage without taking the
    /// relay or trusted issuer-key endpoint down.
    pub fn pause_relay_enrollment(&self) {
        self.state
            .relay_enrollment_unavailable
            .store(true, Ordering::Relaxed);
    }

    /// Deterministic Issuer signing key (`SigningKey::from_bytes(&[42u8; 32])`).
    ///
    /// Used by tests that need to pre-verify credentials issued by the mock.
    pub fn issuer_signing_key(&self) -> &SigningKey {
        self.state.issuer_signing_key()
    }

    /// Deterministic built-in Publisher Registry signing key
    /// (`SigningKey::from_bytes(&[43u8; 32])`).
    ///
    /// This is the key the mock will hand out as the default publisher's
    /// verifying key when no Publisher Registry is explicitly registered via
    /// [`add_publisher`](Self::add_publisher).
    pub fn publisher_signing_key(&self) -> &SigningKey {
        self.state.builtin_publisher_signing_key()
    }

    /// Register a Realm by name (`<realm_id>@<domain>`); it must match
    /// `realm` in the caller's config.
    pub async fn add_realm(&self, realm: &str) {
        self.state.add_realm(realm.to_string()).await;
    }

    /// Register a publisher. The `verifying_key` will be returned by
    /// `GET /registry/publishers/{name}/verifying-key`.
    pub async fn add_publisher(&self, name: &str, verifying_key: VerifyingKey) {
        self.state
            .add_publisher(name.to_string(), verifying_key)
            .await;
    }

    /// Shutdown the server.
    pub async fn shutdown(&mut self) {
        if !self.cancel.is_cancelled() {
            self.cancel.cancel();
        }
    }

    pub fn pause_forwarding(&self) {
        self.pause_forwarding.store(true, Ordering::Release);
    }

    /// Trigger the Host's normal soft-renewal path on heartbeat. The mock
    /// still does not verify credentials; tests may use a real external Issuer.
    pub fn warn_credential_expiring(&self, enabled: bool) {
        self.state
            .credential_warning
            .store(enabled, Ordering::Relaxed);
    }

    pub fn resume_forwarding(&self) {
        self.pause_forwarding.store(false, Ordering::Release);
    }

    /// Stop reading/writing frames for WebSocket connections that already
    /// exist, while allowing later reconnects to establish a fresh socket.
    pub fn blackhole_websocket_io(&self) {
        tracing::warn!("🕳️  Blackholing existing WebSocket IO");
        let current_generation = self.state.websocket_generation.load(Ordering::Acquire);
        self.state
            .blackhole_websocket_generation
            .store(current_generation, Ordering::Release);
    }

    /// Resume reading/writing frames for blackholed WebSocket connections.
    pub fn restore_websocket_io(&self) {
        tracing::info!("🟢 Restoring WebSocket IO");
        self.state
            .blackhole_websocket_generation
            .store(0, Ordering::Release);
    }

    /// Drop the next N Answerer ICE restart requests while keeping every other
    /// signaling payload flowing.
    pub fn drop_next_ice_restart_requests(&self, count: u32) {
        tracing::warn!(
            "🧪 Dropping the next {} ICE restart request relay message(s)",
            count
        );
        self.ice_restart_request_drop_count
            .store(count, Ordering::SeqCst);
    }

    /// Drop the next N ICE candidate relay messages.
    ///
    /// This is a test-only hook for reproducing a post-cleanup negotiation
    /// where SDP arrives but trickle ICE is interrupted.
    pub fn drop_next_ice_candidates(&self, count: u32) {
        tracing::warn!(
            "🧪 Dropping the next {} ICE candidate relay message(s)",
            count
        );
        self.ice_candidate_drop_count.store(count, Ordering::SeqCst);
    }

    /// Drop the next N ICE candidate relay messages for a bounded duration.
    pub fn drop_next_ice_candidates_for(&self, count: u32, duration: std::time::Duration) {
        self.drop_next_ice_candidates(count);
        let ice_candidate_drop_count = self.ice_candidate_drop_count.clone();
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            ice_candidate_drop_count.store(0, Ordering::SeqCst);
        });
    }

    /// Delay ICE candidate forwarding for a bounded duration.
    pub fn delay_ice_candidates_for(&self, duration: std::time::Duration) {
        tracing::warn!(
            "🧪 Delaying ICE candidate relay messages for {:?}",
            duration
        );
        *self
            .state
            .ice_candidate_delay_until
            .lock()
            .expect("ICE candidate delay mutex poisoned") =
            Some(std::time::Instant::now() + duration);
    }

    /// Number of ICE candidates delayed by `delay_ice_candidates_for`.
    pub fn delayed_ice_candidate_count(&self) -> u32 {
        self.state
            .ice_candidate_delay_applied_count
            .load(Ordering::SeqCst)
    }

    /// Drop the next N initial SDP offer relay messages.
    pub fn drop_next_offers(&self, count: u32) {
        tracing::warn!("🧪 Dropping the next {} SDP offer relay message(s)", count);
        self.state.offer_drop_count.store(count, Ordering::SeqCst);
    }

    pub fn message_count(&self) -> u32 {
        self.message_count.load(Ordering::Relaxed)
    }

    /// Snapshot all decoded signaling envelopes received by the server.
    pub async fn received_messages(&self) -> Vec<actr_protocol::SignalingEnvelope> {
        self.state.received_messages.lock().await.clone()
    }

    pub fn ice_restart_count(&self) -> u32 {
        self.ice_restart_offer_count.load(Ordering::SeqCst)
    }

    pub fn ice_restart_request_count(&self) -> u32 {
        self.ice_restart_request_count.load(Ordering::SeqCst)
    }

    pub fn connection_count(&self) -> u32 {
        self.connection_count.load(Ordering::SeqCst)
    }

    pub fn disconnection_count(&self) -> u32 {
        self.disconnection_count.load(Ordering::SeqCst)
    }

    pub fn reset_counters(&self) {
        self.message_count.store(0, Ordering::Relaxed);
        self.ice_restart_offer_count.store(0, Ordering::SeqCst);
        self.ice_restart_request_count.store(0, Ordering::SeqCst);
        self.ice_restart_request_drop_count
            .store(0, Ordering::SeqCst);
        self.ice_candidate_drop_count.store(0, Ordering::SeqCst);
        self.state.offer_drop_count.store(0, Ordering::SeqCst);
        self.connection_count.store(0, Ordering::SeqCst);
        self.disconnection_count.store(0, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }
}

impl Drop for MockLoranServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Build the axum router that multiplexes WebSocket signaling and HTTP
/// endpoints on the same port.
fn build_router(state: Arc<MockState>) -> Router {
    use axum::routing::{get, post};
    use tower_http::cors::{Any, CorsLayer};

    let issuer_routes = Router::new()
        .route("/register", post(http::register_handler))
        .route("/renew", post(http::renew_handler))
        .route("/relay-identity", post(http::relay_identity_handler))
        .route("/relay-key/{realm}/{key_id}", get(http::relay_key_handler))
        .route(
            "/registry/publishers/{name}/verifying-key",
            get(http::verifying_key_handler),
        )
        .with_state(state.clone());

    Router::new()
        // WebSocket signaling — real loran path and legacy root alias.
        .route("/signaling/ws", get(signaling::ws_upgrade_handler))
        .route("/", get(signaling::ws_upgrade_handler))
        .route("/signaling/health", get(http::health_handler))
        .route("/health", get(http::health_handler))
        // Issuer endpoints at the root (for callers that use the root URL as
        // their Issuer endpoint base).
        .route("/register", post(http::register_handler))
        .route("/renew", post(http::renew_handler))
        .route("/relay-identity", post(http::relay_identity_handler))
        .route("/relay-key/{realm}/{key_id}", get(http::relay_key_handler))
        .route(
            "/registry/publishers/{name}/verifying-key",
            get(http::verifying_key_handler),
        )
        // Admin seeding API (replaces register.sh sqlite3 INSERTs).
        .route("/admin/realms", post(http::admin_realm_handler))
        .route(
            "/admin/publisher",
            post(http::admin_publisher_registry_handler),
        )
        .route("/admin/state", get(http::admin_state_handler))
        .with_state(state)
        // Issuer routes mounted under `/issuer/*` for callers whose configured Issuer
        // endpoint is e.g. `http://localhost:8081/issuer`.
        .merge(Router::new().nest("/issuer", issuer_routes))
        // Permissive CORS so browser-resident Service Workers (served from
        // e.g. `http://localhost:5173`) can POST to Issuer / Publisher Registry endpoints.
        // Dev-only; a production signaling stack would restrict origins.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}
