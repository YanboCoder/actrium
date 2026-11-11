//! Node runtime inner — holds all running-state fields for an attached node.
//!
//! This module is the internal implementation backing the public
//! `Node<Attached>` / `Node<Registered>` typestate chain defined in
//! `crate::lib`. The struct itself is crate-private; consumers interact with
//! it indirectly through `Node<S>` → `ActrRef` transitions.

use crate::actr_ref::{ActrRef, ActrRefShared};
use crate::context::{
    BootstrapContextBuilder, RuntimeContext, build_alias_map, build_dependency_fingerprints,
};
use crate::inbound::{DataChunkRegistry, MediaFrameRegistry};
use crate::issuer_client::IssuerClient;
use crate::lifecycle::credential_manager::{
    CredentialManager, HardRebindHandles, RegistrationContext,
};
use crate::lifecycle::dedup::{DEDUP_TTL, DedupOutcome, DedupState, DedupWaiter};
use crate::lifecycle::recovery_policy::translate::LifecycleProfile;
use crate::lifecycle::session_state::{SessionSnapshot, SessionState};
use crate::outbound::Gate;
use crate::transport::HostTransport;
use crate::wire::webrtc::SignalingClient;
#[cfg(feature = "opentelemetry")]
use crate::wire::webrtc::trace::{inject_span_context_to_rpc, set_parent_from_rpc_envelope};
use actr_framework::{Bytes, Deliver};
use actr_protocol::prost::Message as ProstMessage;
use actr_protocol::{
    AIdCredential, ActorResult, ActrError, ActrId, ActrType, ConnectionNotReadyInfo, Direction,
    PayloadType, RegisterAuthMode, RegisterRequest, RpcEnvelope, TurnCredential, register_response,
};
use actr_runtime::check_acl_permission;
use actr_runtime_mailbox::{DeadLetterQueue, Mailbox};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, watch};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "opentelemetry")]
use tracing::Instrument as _;

/// The outcome of an inbound RPC's dispatch continuation (RFC-0323).
///
/// - `Replied(result)` — the RETURN path: dispatch produced result bytes;
///   the receive loop sends the reply + ACKs + the continuation already
///   completed dedup via `finish`. Byte-identical to the pre-RFC path.
/// - `Deferred` — the EXPLICIT path: the handler received a `Reply<T>`
///   handle and the reply + dedup completion + ACK are deferred to whenever
///   `reply.send()` fires (via the `Deliver` impl). The receive loop does
///   nothing inline.
#[derive(Debug)]
pub(crate) enum IncomingOutcome {
    Replied(ActorResult<Bytes>),
    Deferred,
}

/// The remainder of an inbound RPC after synchronous admission has run.
///
/// See [`Inner::admit_incoming`]. `'static + Send` so an entry loop may push it
/// into a `FuturesUnordered` and keep dequeuing.
pub(crate) type IncomingContinuation = Pin<Box<dyn Future<Output = IncomingOutcome> + Send>>;

/// Build the code/message pair used by every RPC error reply. Keeping the
/// numeric code and the DeliveryUncertain field encoding together prevents a
/// future reply path from accidentally sending a lossy Display string.
fn error_response_for_wire(error: &ActrError) -> actr_protocol::ErrorResponse {
    actr_protocol::ErrorResponse {
        code: protocol_error_to_code(error),
        message: encode_delivery_uncertain_for_wire(error),
    }
}

/// Verified Actor types of connected peers, keyed by `ActrId`.
///
/// Filled from `ActrRelay.source_type` (written by Signaling from the
/// sender's verified credential) and from `IdentityClaims.actor_type` on
/// locally verified WebSocket connections. An inbound message takes its
/// sender's type from here when it is enqueued and keeps it in the mailbox
/// row; the inbound ACL checks that recorded type. A peer with no known type
/// matches no ACL rule.
///
/// Each peer also carries a count of its open connections (inbound WebSocket
/// connections and the WebRTC connection together); the entry is dropped
/// when the last one closes, unless its type was recorded within
/// [`PEER_TYPE_SETUP_SECS`]: Signaling relays a connection's type before the
/// connection is counted, possibly while the peer's previous connection is
/// still closing. Such an entry, and one whose connection never opens, is
/// dropped by a later [`PeerTypes::record`] once that time has passed.
#[derive(Clone, Default)]
pub(crate) struct PeerTypes(Arc<std::sync::Mutex<HashMap<ActrId, PeerTypeEntry>>>);

/// How long a recorded type waits for its connection.
pub(crate) const PEER_TYPE_SETUP_SECS: u64 = 60;

#[derive(Default)]
struct PeerTypeEntry {
    actor_type: Option<ActrType>,
    connections: usize,
    recorded_at: Option<tokio::time::Instant>,
}

impl PeerTypeEntry {
    /// No open connection and no connection being set up.
    fn unused(&self) -> bool {
        self.connections == 0
            && self.recorded_at.is_none_or(|recorded_at| {
                recorded_at.elapsed() >= Duration::from_secs(PEER_TYPE_SETUP_SECS)
            })
    }
}

impl PeerTypes {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<ActrId, PeerTypeEntry>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record the verified type of `peer`.
    pub(crate) fn record(&self, peer: ActrId, actor_type: ActrType) {
        let mut entries = self.entries();
        entries.retain(|_, entry| !entry.unused());
        let entry = entries.entry(peer).or_default();
        entry.actor_type = Some(actor_type);
        entry.recorded_at = Some(tokio::time::Instant::now());
    }

    /// The verified type of `peer`, if known.
    pub(crate) fn get(&self, peer: &ActrId) -> Option<ActrType> {
        self.entries()
            .get(peer)
            .and_then(|entry| entry.actor_type.clone())
    }

    /// A connection to `peer` opened.
    pub(crate) fn connection_opened(&self, peer: &ActrId) {
        let mut entries = self.entries();
        let entry = entries.entry(peer.clone()).or_default();
        entry.connections += 1;
        entry.recorded_at = None;
    }

    /// A connection to `peer` closed; the last one drops the entry unless a
    /// connection is being set up.
    pub(crate) fn connection_closed(&self, peer: &ActrId) {
        let mut entries = self.entries();
        if let Some(entry) = entries.get_mut(peer) {
            entry.connections = entry.connections.saturating_sub(1);
            if entry.unused() {
                entries.remove(peer);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn connections(&self, peer: &ActrId) -> usize {
        self.entries()
            .get(peer)
            .map_or(0, |entry| entry.connections)
    }

    /// Count every peer the WebRTC manager holds as one open connection,
    /// until the manager lets it go, and stop at `shutdown`.
    pub(crate) async fn follow_webrtc_peers(
        self,
        manager: Arc<crate::wire::webrtc::rtc::manager::RtcConnectionManager>,
        shutdown: CancellationToken,
    ) {
        let mut changes = manager.subscribe_peers();
        let mut counted = std::collections::HashSet::new();
        loop {
            let current: std::collections::HashSet<ActrId> =
                manager.peer_ids().await.into_iter().collect();
            for peer in current.difference(&counted) {
                self.connection_opened(peer);
            }
            for peer in counted.difference(&current) {
                self.connection_closed(peer);
            }
            counted = current;
            tokio::select! {
                _ = shutdown.cancelled() => return,
                changed = changes.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// The caller of an inbound message: its `ActrId` and, when known, the
/// verified `ActrType` recorded when the message arrived.
pub type IncomingCaller<'a> = (&'a ActrId, Option<&'a ActrType>);

/// The sender type recorded with a mailbox message; an undecodable one
/// counts as unknown.
fn recorded_caller_type(record: &actr_runtime_mailbox::MessageRecord) -> Option<ActrType> {
    record
        .from_type
        .as_deref()
        .and_then(|bytes| ActrType::decode(bytes).ok())
}

pub(crate) type MailboxTail = Pin<Box<dyn Future<Output = ()> + Send>>;
type MailboxDepthCallback = Arc<dyn Fn(usize) + Send + Sync + 'static>;

struct EnqueueObserver {
    fire: MailboxDepthCallback,
    wake: Arc<tokio::sync::Notify>,
}

impl actr_runtime_mailbox::MailboxDepthObserver for EnqueueObserver {
    fn on_depth_change(&self, queued_messages: usize) {
        self.wake.notify_one();
        (self.fire)(queued_messages);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailboxIdleOutcome {
    Shutdown,
    Progress,
}

/// While durable storage is empty, keep polling the reply/ack tails that were
/// already admitted. Waiting only for a future enqueue would leave completed
/// tails parked indefinitely on an otherwise idle mailbox.
pub(crate) async fn wait_for_mailbox_idle<F>(
    shutdown: &CancellationToken,
    inflight: &mut futures_util::stream::FuturesUnordered<MailboxTail>,
    idle_wake: F,
) -> MailboxIdleOutcome
where
    F: Future<Output = ()>,
{
    use futures_util::StreamExt as _;

    tokio::pin!(idle_wake);
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => MailboxIdleOutcome::Shutdown,
        Some(()) = inflight.next(), if !inflight.is_empty() => MailboxIdleOutcome::Progress,
        _ = &mut idle_wake => MailboxIdleOutcome::Progress,
    }
}

/// Select the zero-delay enqueue notification for push-capable mailboxes, or
/// retain the compatibility polling delay for custom backends without an
/// observer. `enqueued` is armed before `dequeue()` so an enqueue in the
/// dequeue/await gap cannot be lost.
async fn mailbox_empty_wake<F>(mailbox_has_push_observer: bool, enqueued: F)
where
    F: Future<Output = ()>,
{
    if mailbox_has_push_observer {
        enqueued.await;
    } else {
        crate::timer::sleep(
            crate::timer::ids::MAILBOX_EMPTY_FALLBACK,
            Duration::from_millis(10),
        )
        .await;
    }
}

/// Type-erased startup future kept behind the crate-private node boundary.
///
/// `Inner::start_inner` intentionally contains several large background-loop
/// state machines. Erasing that concrete layout here keeps it from propagating
/// into every downstream caller of the public typestate API.
pub(crate) type NodeStartFuture =
    Pin<Box<dyn Future<Output = ActorResult<ActrRef>> + Send + 'static>>;

/// Wrap an already-known result as an immediately-ready continuation.
fn ready_continuation(result: ActorResult<Bytes>) -> IncomingContinuation {
    Box::pin(async move { IncomingOutcome::Replied(result) })
}

/// Send the reply for one in-proc (Shell → Guest) request back over the
/// Guest → Shell channel.
///
/// This is the "reply tail" of the in-proc receive loop, extracted so both the
/// gate-off inline path and the gate-on continuation (pushed into the loop's
/// `FuturesUnordered`) run bit-for-bit identical logic. A `tell`
/// (`expects_response == false`) runs for side effects only and sends nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn inproc_send_reply(
    response_tx: Arc<HostTransport>,
    route_key: String,
    request_id: String,
    traceparent: Option<String>,
    tracestate: Option<String>,
    expects_response: bool,
    result: ActorResult<Bytes>,
    span: tracing::Span,
) {
    let _ = &span; // consumed only under the opentelemetry feature
    match result {
        Ok(response_bytes) => {
            if expects_response {
                #[cfg_attr(not(feature = "opentelemetry"), allow(unused_mut))]
                let mut response_envelope = Inner::build_response_envelope(
                    request_id.clone(),
                    route_key,
                    Some(response_bytes),
                    None,
                    None,
                    None,
                );
                #[cfg(feature = "opentelemetry")]
                inject_span_context_to_rpc(&span, &mut response_envelope);

                let send_response_fut =
                    response_tx.send_message(PayloadType::RpcReliable, None, response_envelope);
                #[cfg(feature = "opentelemetry")]
                let send_response_fut = send_response_fut.instrument(span);
                if let Err(e) = send_response_fut.await {
                    tracing::error!(
                        severity = 7,
                        error_category = "transport_error",
                        request_id = %request_id,
                        "❌ Failed to send RESPONSE to Shell: {:?}",
                        e
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!(
                severity = 6,
                error_category = "handler_error",
                request_id = %request_id,
                "❌ Guest message handling failed: {:?}",
                e
            );
            // Keep the local error log above for every failure, but skip sending
            // an error envelope for a `tell`: the caller registered no pending
            // entry to receive it (#262).
            if expects_response {
                let error_response = error_response_for_wire(&e);
                #[cfg_attr(not(feature = "opentelemetry"), allow(unused_mut))]
                let mut error_envelope = Inner::build_response_envelope(
                    request_id.clone(),
                    route_key,
                    None,
                    Some(error_response),
                    traceparent,
                    tracestate,
                );
                #[cfg(feature = "opentelemetry")]
                inject_span_context_to_rpc(&span, &mut error_envelope);

                let send_error_response_fut =
                    response_tx.send_message(PayloadType::RpcReliable, None, error_envelope);
                #[cfg(feature = "opentelemetry")]
                let send_error_response_fut = send_error_response_fut.instrument(span);
                if let Err(send_err) = send_error_response_fut.await {
                    tracing::error!(
                        severity = 7,
                        error_category = "transport_error",
                        request_id = %request_id,
                        "❌ Failed to send ERROR response to Shell: {:?}",
                        send_err
                    );
                }
            }
        }
    }
}

/// RTC response route, populated when the node's RTC manager is initialized.
pub(crate) type RtcResponseRoute =
    Option<Arc<crate::wire::webrtc::rtc::manager::RtcConnectionManager>>;

/// Send `response_envelope` back to `caller` via the best available transport.
///
/// Priority: ready direct RTC → accepted relay → other ready RTC.
/// Module-level so both the inline mailbox path and the gate-on continuation
/// share one implementation.
/// How long the RTC response path waits for the caller's peer to become
/// sendable before falling through to the gate-off delivery path. Named (like
/// DATA_CHUNK_SEND_TIMEOUT / TELL_SEND_TIMEOUT) so the budget stays tunable
/// and greppable; the wait is an `.await` inside the mailbox reply path, so
/// the "Gate off" fallback serializes replies behind it by design.
const CALLER_SENDABLE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

async fn send_envelope_to_caller(
    ws_gate: &Option<Arc<crate::wire::websocket::WebSocketGate>>,
    rtc_route: &RtcResponseRoute,
    caller: &ActrId,
    response_envelope: RpcEnvelope,
    request_id: &str,
    route_key: &str,
) {
    // #251 response-path diagnostics: the host has the guest's response and
    // is about to push it to the caller. Info-level so the default
    // RUST_LOG=info of the polyglot e2e captures the send attempt — without
    // it, a caller-side timeout cannot be told apart from a lost send.
    tracing::info!(
        target: "actr_response_send",
        request_id = %request_id,
        route_key = %route_key,
        payload_type = "RpcReliable",
        target_actor = %caller,
        response_len = response_envelope.payload.as_ref().map_or(0, |p| p.len()),
        error_code = ?response_envelope.error.as_ref().map(|e| e.code),
        "response send attempted: routing guest response back to caller"
    );
    // Promote responses too: retaining the relay receive path must not pin
    // every reply to WSS after direct ICE becomes usable.
    let direct = if let Some(manager) = rtc_route {
        manager.get_controller(caller).await.filter(|handle| {
            let snapshot = handle.snapshot();
            snapshot.sendable
                && snapshot.network_path == crate::wire::webrtc::rtc::status::NetworkPath::Direct
        })
    } else {
        None
    };
    let direct_send = direct.map(|handle| {
        let payload = Bytes::from(response_envelope.encode_to_vec());
        async move {
            handle
                .send_with_progress(
                    crate::wire::webrtc::rtc::status::LaneKind::RpcReliable,
                    payload,
                    false,
                )
                .await
        }
    });
    if try_direct_or_relay(
        direct_send,
        ws_gate,
        caller,
        &response_envelope,
        request_id,
        route_key,
    )
    .await
    {
        return;
    }

    // 2. RTC backend: route the response through the peer's reliable lane.
    // The request arrived over this lane, so the peer session exists and is
    // sendable (RpcReliable IS the readiness anchor); the bounded wait only
    // covers snapshot projection lag on a freshly connected pair.
    if let Some(manager) = rtc_route {
        if let Some(handle) = manager.get_controller(caller).await {
            let mut rx = handle.subscribe();
            let waited = crate::timer::timeout(
                crate::timer::ids::WEBRTC_PEER_SENDABLE,
                CALLER_SENDABLE_WAIT,
                async {
                    loop {
                        if rx.borrow().sendable {
                            return;
                        }
                        if rx.changed().await.is_err() {
                            return;
                        }
                    }
                },
            )
            .await;
            let sendable = handle.snapshot().sendable;
            if waited.is_ok() && sendable {
                let mut buf = Vec::new();
                match response_envelope.encode(&mut buf) {
                    Ok(()) => {
                        match handle
                            .send(
                                crate::wire::webrtc::rtc::status::LaneKind::RpcReliable,
                                bytes::Bytes::from(buf),
                            )
                            .await
                        {
                            Ok(()) => {
                                tracing::info!(
                                    target: "actr_response_send",
                                    request_id = %request_id,
                                    route_key = %route_key,
                                    payload_type = "RpcReliable",
                                    target_actor = %caller,
                                    "response send succeeded via RTC reliable lane"
                                );
                                return;
                            }
                            Err(e) => {
                                tracing::error!(
                                    severity = 7,
                                    error_category = "transport_error",
                                    request_id = request_id,
                                    route_key = %route_key,
                                    target_actor = %caller,
                                    "❌ RTC response send failed: {e:?}"
                                );
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            severity = 7,
                            error_category = "transport_error",
                            request_id = request_id,
                            route_key = %route_key,
                            target_actor = %caller,
                            "❌ RTC response encode failed: {e}"
                        );
                        return;
                    }
                }
            }
            if waited.is_err() || !sendable {
                let submitted = manager.submit_recovery_hint(caller, request_id).await;
                if submitted {
                    tracing::warn!(
                        target: "actr_response_send",
                        request_id = %request_id,
                        route_key = %route_key,
                        target_actor = %caller,
                        sendability_wait_elapsed = waited.is_err(),
                        peer_sendable = sendable,
                        recovery_hint_submitted = true,
                        "response peer remained non-sendable; recovery hint submitted"
                    );
                } else {
                    tracing::warn!(
                        target: "actr_response_send",
                        request_id = %request_id,
                        route_key = %route_key,
                        target_actor = %caller,
                        sendability_wait_elapsed = waited.is_err(),
                        peer_sendable = sendable,
                        recovery_hint_submitted = false,
                        "response peer remained non-sendable; recovery hint was not accepted"
                    );
                }
            }
            tracing::error!(
                severity = 7,
                error_category = "transport_error",
                request_id = request_id,
                route_key = %route_key,
                target_actor = %caller,
                "❌ RTC peer not sendable for response within the wait window"
            );
            return;
        }
        tracing::error!(
            severity = 7,
            error_category = "transport_error",
            request_id = request_id,
            route_key = %route_key,
            target_actor = %caller,
            "❌ no RTC controller for response caller"
        );
    }
}

/// Returns `true` when delivery succeeded or may have been attempted, so the
/// caller must not retry. Returns `false` only when another route is safe.
async fn try_direct_or_relay<F>(
    direct_send: Option<F>,
    ws_gate: &Option<Arc<crate::wire::websocket::WebSocketGate>>,
    caller: &ActrId,
    response_envelope: &RpcEnvelope,
    request_id: &str,
    route_key: &str,
) -> bool
where
    F: std::future::Future<Output = crate::wire::webrtc::rtc::peer::send::RtcSendAttempt>,
{
    if let Some(send) = direct_send {
        let attempt = send.await;
        match attempt.result {
            Ok(()) => {
                tracing::info!(
                    target: "actr_response_send",
                    request_id = %request_id,
                    target_actor = %caller,
                    "response send succeeded via direct RTC reliable lane"
                );
                return true;
            }
            Err(error)
                if attempt.proven_pre_send
                    && attempt.progress == crate::transport::DeliveryProgress::NotAttempted =>
            {
                tracing::warn!(
                    target: "actr_response_send",
                    request_id = %request_id,
                    target_actor = %caller,
                    error = ?error,
                    "direct RTC response failed before submission; falling back to relay"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target: "actr_response_send",
                    request_id = %request_id,
                    target_actor = %caller,
                    error = ?error,
                    "direct RTC response submission failed; delivery may be uncertain"
                );
                return true;
            }
        }
    }

    // Try the already-accepted relay without waiting for ICE.
    if let Some(wsg) = ws_gate {
        match wsg.send_response(caller, response_envelope.clone()).await {
            Ok(true) => {
                tracing::info!(
                    target: "actr_response_send",
                    request_id = %request_id,
                    route_key = %route_key,
                    payload_type = "RpcReliable",
                    target_actor = %caller,
                    "response send succeeded via relay gate"
                );
                return true;
            }
            Ok(false) => {
                tracing::debug!(
                    request_id = request_id,
                    caller = %caller,
                    "No sendable relay connection for caller; falling back to WebRTC gate"
                );
            }
            Err(e) => {
                tracing::warn!(
                    severity = 5,
                    error_category = "transport_error",
                    request_id = request_id,
                    "Relay response submission failed; delivery may be uncertain: {:?}",
                    e
                );
                return true;
            }
        }
    }
    false
}

/// Send the RESPONSE envelope for an inbound mailbox call (Ok or Err path).
/// Does NOT ACK the durable mailbox message — the caller (RETURN receive
/// loop or EXPLICIT `ReplyDeliver`) is responsible for acking separately so
/// that EXPLICIT can ACK at dispatch-completion (RFC-0323 §4: ack-at-return)
/// rather than at reply time.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn mailbox_send_reply(
    ws_gate: Option<Arc<crate::wire::websocket::WebSocketGate>>,
    rtc_route: RtcResponseRoute,
    caller: Result<ActrId, String>,
    request_id: String,
    route_key: String,
    traceparent: Option<String>,
    tracestate: Option<String>,
    expects_response: bool,
    result: ActorResult<Bytes>,
    span: tracing::Span,
) {
    let _ = &span; // consumed only under the opentelemetry feature
    match result {
        Ok(response_bytes) => match caller {
            Ok(caller) if expects_response => {
                #[cfg_attr(not(feature = "opentelemetry"), allow(unused_mut))]
                let mut response_envelope = Inner::build_response_envelope(
                    request_id.clone(),
                    route_key.clone(),
                    Some(response_bytes),
                    None,
                    traceparent,
                    tracestate,
                );
                #[cfg(feature = "opentelemetry")]
                inject_span_context_to_rpc(&span, &mut response_envelope);

                #[cfg(feature = "opentelemetry")]
                let send_fut = send_envelope_to_caller(
                    &ws_gate,
                    &rtc_route,
                    &caller,
                    response_envelope,
                    &request_id,
                    &route_key,
                )
                .instrument(span);
                #[cfg(not(feature = "opentelemetry"))]
                let send_fut = send_envelope_to_caller(
                    &ws_gate,
                    &rtc_route,
                    &caller,
                    response_envelope,
                    &request_id,
                    &route_key,
                );
                send_fut.await;
            }
            // `tell` handled successfully: side effects ran, no reply is sent.
            Ok(_) => {}
            Err(e) => {
                tracing::error!(
                    severity = 8,
                    error_category = "protobuf_decode",
                    request_id = %request_id,
                    "❌ Failed to decode caller_id: {}",
                    e
                );
            }
        },
        Err(e) => {
            tracing::error!(
                severity = 6,
                error_category = "handler_error",
                request_id = %request_id,
                route_key = %route_key,
                "❌ handle_incoming failed: {:?}", e
            );

            // Send error envelope back to caller so it receives a structured
            // error rather than waiting until its deadline fires. A `tell`
            // (timeout_ms == 0) registered no pending entry, so skip the reply —
            // the local error log above still records the failure (#262).
            if let (true, Ok(caller)) = (expects_response, caller) {
                let error_response = error_response_for_wire(&e);
                #[cfg_attr(not(feature = "opentelemetry"), allow(unused_mut))]
                let mut error_envelope = Inner::build_response_envelope(
                    request_id.clone(),
                    route_key.clone(),
                    None,
                    Some(error_response),
                    traceparent,
                    tracestate,
                );
                #[cfg(feature = "opentelemetry")]
                inject_span_context_to_rpc(&span, &mut error_envelope);

                send_envelope_to_caller(
                    &ws_gate,
                    &rtc_route,
                    &caller,
                    error_envelope,
                    &request_id,
                    &route_key,
                )
                .await;
            }
        }
    }
}

/// ACK a durable mailbox message. Split out from `mailbox_send_reply` so the
/// EXPLICIT pump can ACK at dispatch-completion (RFC-0323 §4) independently of
/// when the reply is actually sent.
pub(crate) async fn mailbox_ack(mailbox: Arc<dyn Mailbox>, msg_id: uuid::Uuid, request_id: &str) {
    if let Err(e) = mailbox.ack(msg_id).await {
        tracing::error!(
            severity = 9,
            error_category = "mailbox_error",
            request_id = %request_id,
            message_id = %msg_id,
            "❌ Mailbox ACK failed: {:?}",
            e
        );
    }
}

/// RETURN-mode mailbox tail: send the reply, then ACK. For EXPLICIT the ACK
/// moves to dispatch-completion (`mailbox_ack` called from the continuation)
/// and `ReplyDeliver` calls `mailbox_send_reply` only.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn mailbox_reply_and_ack(
    ws_gate: Option<Arc<crate::wire::websocket::WebSocketGate>>,
    rtc_route: RtcResponseRoute,
    mailbox: Arc<dyn Mailbox>,
    caller: Result<ActrId, String>,
    request_id: String,
    route_key: String,
    traceparent: Option<String>,
    tracestate: Option<String>,
    expects_response: bool,
    msg_id: uuid::Uuid,
    result: ActorResult<Bytes>,
    span: tracing::Span,
) {
    mailbox_send_reply(
        ws_gate,
        rtc_route,
        caller,
        request_id.clone(),
        route_key,
        traceparent,
        tracestate,
        expects_response,
        result,
        span,
    )
    .await;
    mailbox_ack(mailbox, msg_id, &request_id).await;
}

/// Force-closes stream callbacks when the node's owning state is released.
struct DataChunkRegistryCleanupGuard {
    registry: Weak<DataChunkRegistry>,
}

impl DataChunkRegistryCleanupGuard {
    fn new(registry: &Arc<DataChunkRegistry>) -> Self {
        Self {
            registry: Arc::downgrade(registry),
        }
    }
}

impl Drop for DataChunkRegistryCleanupGuard {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.clear();
        }
    }
}

/// Internal running-state of an attached node.
///
/// Holds every field required to run a workload after `Hyper::attach` has
/// bound a package. Kept private to the crate: external callers use the
/// public `Node<S>` wrappers in `crate::lib` and the `ActrRef` handle
/// returned by `Node::start`.
pub(crate) struct Inner {
    pub(crate) realm_client_key: Option<crate::issuer_client::RealmClientKey>,
    /// Runtime configuration
    pub(crate) config: actr_config::RuntimeConfig,

    /// SQLite persistent mailbox
    pub(crate) mailbox: Arc<dyn Mailbox>,

    /// Dead Letter Queue for poison messages
    pub(crate) dlq: Arc<dyn DeadLetterQueue>,

    /// In-process gate for `Dest::Host` / `Dest::Workload` calls.
    ///
    /// Created in `build()` together with `shell_to_workload` so the inproc
    /// lane is usable as soon as the node exists, even before registration.
    pub(crate) inproc_gate: Gate,

    /// Cross-process gate for `Dest::Peer(_)` calls.
    ///
    /// `None` until `start()` finishes WebRTC / PeerGate initialization. Any
    /// outbound call issued before that point returns `Internal("PeerGate
    /// not initialized yet")` — see `RuntimeContext::select_gate`.
    pub(crate) outproc_gate: Option<Gate>,

    /// Breaks `registry -> callback -> RuntimeContext -> registry` ownership
    /// cycles on every `Inner` exit path, including startup failures. It is
    /// declared before the registry so callbacks are force-closed first on
    /// drop.
    _data_chunk_registry_cleanup: DataChunkRegistryCleanupGuard,

    /// DataChunk callback registry shared between the inbound WebRTC / WS
    /// gates (which dispatch into it) and `RuntimeContext`
    /// (register_stream / send_data_chunk).
    pub(crate) data_chunk_registry: Arc<DataChunkRegistry>,

    /// MediaTrack callback registry shared between WebRTC media tracks and
    /// `RuntimeContext` (register_media_track / send_media_sample).
    pub(crate) media_frame_registry: Arc<MediaFrameRegistry>,

    /// Signaling client
    pub(crate) signaling_client: Arc<dyn SignalingClient>,

    /// Actor ID (obtained after startup)
    pub(crate) actor_id: Option<ActrId>,

    /// Actor Credential (obtained after startup, used for subsequent authentication messages)
    pub(crate) credential_state: Option<CredentialState>,

    /// Unified identity/credential snapshot for renewal and hard rebind.
    pub(crate) session_state: Option<SessionState>,

    /// Test-only directed fault registries (`test-utils` builds only), set
    /// by attach/link from [`crate::Node::with_test_faults`]. When present,
    /// `start()` wraps the RTC factory and signaling transport with the
    /// registries. Production builds never see this field.
    #[cfg(feature = "test-utils")]
    pub(crate) test_faults: Option<crate::wire::webrtc::test_runtime::TestFaultRegistries>,

    /// RTC connection manager (created after startup; RTC backend only).
    /// Backs the node-level RTC status query/list/subscribe APIs (S6-4).
    pub(crate) rtc_manager: Option<Arc<crate::wire::webrtc::rtc::manager::RtcConnectionManager>>,

    /// RTC inbound mailbox slot (RTC backend only): the late-bound mailbox
    /// reference the W3b inbound bridge reads to route DataChannel payloads
    /// into the node mailbox. Filled when the receive loops start.
    pub(crate) rtc_inbound_mailbox_slot:
        Option<crate::wire::webrtc::rtc::inbound_bridge::MailboxSlot>,

    /// Peer transport manager (created after startup)
    pub(crate) peer_transport: Option<Arc<crate::transport::PeerTransport>>,

    /// WebSocket Gate (direct-connect mode inbound, optional)
    pub(crate) websocket_gate: Option<Arc<crate::wire::websocket::WebSocketGate>>,

    /// Shell → Workload transport (REQUEST direction)
    ///
    /// Workload receives REQUEST from Shell (zero serialization, direct RpcEnvelope passing)
    pub(crate) shell_to_workload: Option<Arc<HostTransport>>,

    /// Workload → Shell transport (RESPONSE direction)
    ///
    /// Workload sends RESPONSE to Shell (separate pending_requests from Shell's)
    pub(crate) workload_to_shell: Option<Arc<HostTransport>>,

    /// Shutdown token for graceful shutdown
    pub(crate) shutdown_token: CancellationToken,

    /// Admission barrier spanning one complete durable-mailbox dequeue batch.
    ///
    /// `dequeue()` marks the whole batch Inflight before the node processes its
    /// records. Holding this gate's read side until every record has either
    /// been admitted or handled inline prevents shutdown from closing the
    /// scheduler / runner between records and ACKing the unexecuted tail.
    mailbox_batch_admission: MailboxBatchAdmission,

    /// Packaged manifest.lock.toml content loaded at startup for fingerprint lookups.
    ///
    /// Wrapped in `Arc` so per-request `RuntimeContext` clones only bump a refcount
    /// instead of deep-cloning the dependency vector.
    pub(crate) actr_lock: Option<Arc<actr_config::lock::LockFile>>,
    /// Runtime dependency aliases built once from the source appropriate to
    /// this workload: authenticated lock for packages, dynamic manifest for
    /// linked workloads.
    pub(crate) alias_map: Arc<HashMap<String, ActrType>>,
    pub(crate) dependency_fingerprints: Arc<HashMap<ActrType, String>>,
    /// Network event receiver (from NetworkEventHandle)
    pub(crate) network_event_rx:
        Option<tokio::sync::mpsc::Receiver<crate::lifecycle::network_event::NetworkEventRequest>>,

    /// Network event debounce configuration
    pub(crate) network_event_debounce_config:
        Option<crate::lifecycle::network_event::DebounceConfig>,

    /// The supervisor's internal input channel, created alongside the network
    /// event handle so a fact sink over the same sender can feed normalized
    /// signaling and session facts into the reconciler. Taken at `start()`.
    pub(crate) network_event_internal:
        Option<crate::lifecycle::network_event::SupervisorInternalChannel>,

    /// Live outbound-send projection subscribed from the recovery supervisor.
    /// Taken when `PeerGate` is constructed during `start()`.
    pub(crate) send_projection_rx:
        Option<tokio::sync::watch::Receiver<crate::lifecycle::SendProjection>>,

    /// Fact sink over the supervisor's internal input channel, handed to the
    /// signaling client and session state so resource owners can report facts.
    pub(crate) supervisor_fact_sink: Option<crate::lifecycle::network_event::SupervisorFactSink>,

    /// Request deduplication state (15 s TTL response cache, prevents double-processing on retry)
    pub(crate) dedup_state: Arc<Mutex<DedupState>>,

    /// Outstanding EXPLICIT replies awaiting a later `Reply::send`/`Drop`
    /// (RFC-0323 §5/§7). Backs the `outstanding_replies` gauge, the
    /// `max_pending_replies` backpressure guard, and the leak-safety TTL sweep.
    pub(crate) outstanding_replies: Arc<crate::lifecycle::outstanding_replies::OutstandingReplies>,

    /// Cap on outstanding EXPLICIT replies before the pump pauses dequeue
    /// (RFC-0323 §7 backpressure). Default 1024 (RFC recommends a lenient cap).
    pub(crate) max_pending_replies: usize,

    /// Verified package manifest for package-backed nodes.
    #[allow(dead_code)]
    pub(crate) package_manifest: Option<actr_pack::PackageManifest>,

    /// Pre-issued registration credential injected by the Hyper layer during
    /// the `Attached → Registered` state transition. `start()` uses it directly
    /// instead of re-registering with the signaling server.
    pub(crate) preregistered_credential: Option<actr_protocol::register_response::RegisterOk>,

    /// Registration context matching `preregistered_credential`, used for
    /// hard rebind if renewal token is no longer usable.
    pub(crate) preregistered_registration_context: Option<RegistrationContext>,

    /// Verified Actor types of peers, read by the inbound ACL.
    pub(crate) peer_types: PeerTypes,

    /// Handle to the per-actor serial command runner.
    ///
    /// `handle_incoming` dispatches through this handle. The runner owns the
    /// `Workload` (WASM `Store` / native guest ABI) and processes commands one
    /// at a time, run-to-completion, so the single-threaded guest instance is
    /// never entered concurrently — the same invariant the old node-global
    /// `Arc<Mutex<Workload>>` provided, now enforced by a single-consumer
    /// command channel instead of a shared lock. Dispatch, data-stream, hook,
    /// and lifecycle commands all travel the same channel (= the old single
    /// lock); transport and other observation hooks reach linked workloads
    /// through `hook_observer` without touching this handle.
    pub(crate) workload_dispatch: Arc<crate::executor::ActorHandle>,

    /// Clone of the linked workload handle, retained so the pump can query
    /// `reply_mode_for_route` before dispatch (RFC-0323). Foreign linked
    /// workloads report `Explicit` for the FFI delivery boundary, while
    /// Component Model packages use the same host-side mode because their
    /// generated dispatchers call the WIT `reply` import for both RETURN and
    /// EXPLICIT routes. DynClib remains on the legacy RETURN boundary until
    /// its ABI grows an equivalent reply operation.
    pub(crate) linked_handle: Option<Arc<dyn crate::workload::LinkedWorkloadHandle>>,

    /// Component packages opt into the explicit reply boundary only when the
    /// compiled component imports `actr:workload/host@0.6.0`. V2 components
    /// with no such import retain the direct dispatch-return contract.
    pub(crate) package_uses_explicit_reply_boundary: bool,

    /// In-memory conflict-key dispatch scheduler (B2). `None` = the scheduler is
    /// not engaged (gate off, **or** — under strategy A default-on — a keyless
    /// actor), so dispatch goes straight to `workload_dispatch` exactly as B1
    /// did. `Some(scheduler)` routes RPC dispatch through per-key serial +
    /// budgeted concurrent scheduling before it reaches the runner.
    pub(crate) dispatch_scheduler: Option<Arc<crate::dispatch::scheduler::SchedulerHandle>>,

    /// Conflict-key projection table. Empty (default) = every method projects to
    /// the global serial key, so even with the gate on an app that declared
    /// nothing stays fully serial.
    pub(crate) conflict_keys: Arc<crate::dispatch::ConflictKeySpec>,

    /// Optional shell-side observer that receives linked-workload transport /
    /// credential / mailbox hook invocations.
    ///
    /// `None` means "no observer installed"; the built-in tracing defaults
    /// still fire from the event-source wiring sites. When `Some`, hook
    /// invocations are dispatched through the ordered hook dispatcher so
    /// panics in observer code cannot unwind into the event source.
    #[allow(dead_code)]
    pub(crate) hook_observer: Option<crate::lifecycle::hooks::WorkloadHookObserverRef>,

    /// Queue-length threshold at which the mailbox backpressure
    /// watchdog fires the framework `on_mailbox_backpressure` hook.
    ///
    /// Resolved from [`HyperConfig`] at node construction time so the
    /// runtime loop does not need to hold a reference back to `HyperConfig`.
    pub(crate) mailbox_backpressure_threshold: usize,

    /// Lead time before credential expiry at which the framework fires
    /// the `on_credential_expiring` hook. Resolved from [`HyperConfig`]
    /// at node construction time.
    #[allow(dead_code)]
    pub(crate) credential_expiry_warning: Duration,

    /// Whether `start()` spawns the membership controller for on-demand
    /// credential recovery. Resolved from the private runtime configuration at
    /// construction. The default `false` preserves the legacy lifecycle.
    pub(crate) membership_controller_enabled: bool,
}

/// Credential state for shared access between tasks
#[derive(Clone)]
pub struct CredentialState {
    inner: Arc<RwLock<CredentialStateInner>>,
    turn_revision_tx: watch::Sender<u64>,
}

#[derive(Clone)]
struct CredentialStateInner {
    credential: AIdCredential,
    expires_at: Option<prost_types::Timestamp>,
    /// HMAC time-limited TURN credential, updated together with credential on registration/renewal
    turn_credential: Option<TurnCredential>,
    turn_revision: u64,
}

impl CredentialState {
    /// Create a new CredentialState with TURN credential
    pub fn new(
        credential: AIdCredential,
        expires_at: Option<prost_types::Timestamp>,
        turn_credential: Option<TurnCredential>,
    ) -> Self {
        let (turn_revision_tx, _) = watch::channel(0);
        Self {
            inner: Arc::new(RwLock::new(CredentialStateInner {
                credential,
                expires_at,
                turn_credential,
                turn_revision: 0,
            })),
            turn_revision_tx,
        }
    }

    pub async fn credential(&self) -> AIdCredential {
        self.inner.read().await.credential.clone()
    }

    pub async fn expires_at(&self) -> Option<prost_types::Timestamp> {
        self.inner.read().await.expires_at
    }

    /// Get TURN credential (HMAC time-limited credential)
    pub async fn turn_credential(&self) -> Option<TurnCredential> {
        self.inner.read().await.turn_credential.clone()
    }

    pub(crate) fn subscribe_turn_revision(&self) -> watch::Receiver<u64> {
        self.turn_revision_tx.subscribe()
    }

    /// Update credential and TURN credential
    ///
    /// Called on credential renewal; only overwrites the old TURN credential when the new one is not empty
    pub(crate) async fn update(
        &self,
        credential: AIdCredential,
        expires_at: Option<prost_types::Timestamp>,
        turn_credential: Option<TurnCredential>,
    ) {
        let mut guard = self.inner.write().await;
        guard.credential = credential;
        guard.expires_at = expires_at;
        let turn_changed = turn_credential
            .as_ref()
            .is_some_and(|next| guard.turn_credential.as_ref() != Some(next));
        if let Some(turn_credential) = turn_credential {
            guard.turn_credential = Some(turn_credential);
        }
        let revision = turn_changed.then(|| {
            guard.turn_revision = guard.turn_revision.wrapping_add(1);
            guard.turn_revision
        });
        drop(guard);
        if let Some(revision) = revision {
            self.turn_revision_tx.send_replace(revision);
        }
    }

    /// Test-only entry point mirroring `update`, so integration tests can
    /// simulate the in-place credential write a soft renewal performs on the
    /// shared `HardRebindHandles.credential_state`. See #423.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn update_for_test(
        &self,
        credential: AIdCredential,
        expires_at: Option<prost_types::Timestamp>,
        turn_credential: Option<TurnCredential>,
    ) {
        self.update(credential, expires_at, turn_credential).await;
    }
}

/// Resolve the lane a guest declared for an outbound RPC. The ABI field
/// defaults to 0 (`RpcReliable`) for pre-lane guests, which matches the
/// lane legacy hosts hard-coded. An i32 that is not a known `PayloadType`
/// (a guest built against a newer protocol) degrades to `RpcReliable`;
/// known non-RPC lanes are rejected downstream by
/// `ensure_rpc_payload_type`.
fn guest_rpc_payload_type(raw: i32) -> actr_protocol::PayloadType {
    match actr_protocol::PayloadType::try_from(raw) {
        Ok(payload_type) => payload_type,
        Err(_) => {
            tracing::warn!(
                payload_type = raw,
                "unrecognised payload_type from guest; falling back to RpcReliable"
            );
            actr_protocol::PayloadType::RpcReliable
        }
    }
}

/// Host operation executor - routes guest outbound calls through RuntimeContext
///
/// Called by the workload dispatch path in `handle_incoming`.
async fn host_operation_handler(
    ctx: crate::context::RuntimeContext,
    workload_dispatch: std::sync::Weak<crate::executor::ActorHandle>,
    pending: crate::workload::HostOperation,
) -> crate::workload::HostOperationResult {
    use crate::workload::{HostOperation, HostOperationResult, decode_dest};
    use actr_framework::guest::dynclib_abi::code as abi_code;
    use actr_framework::{Context as _, Dest};
    use actr_protocol::{DataChunk, PayloadType};

    match pending {
        HostOperation::CallRaw(req) => {
            match ctx
                .call_raw(
                    &Dest::Peer(req.target),
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                    req.timeout_ms,
                )
                .await
            {
                Ok(resp) => HostOperationResult::Bytes(resp.to_vec()),
                Err(e) => {
                    tracing::error!("call_raw routing failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }

        HostOperation::Reply(payload) => {
            use actr_framework::{Context as _, ReplyOutcome};
            let Some(deliver) = ctx.reply_deliver() else {
                return HostOperationResult::Error(ActrError::Internal(
                    "guest reply requires an inbound RPC reply context".to_string(),
                ));
            };
            deliver.deliver(ReplyOutcome {
                request_id: ctx.request_id().to_string(),
                result: Ok(bytes::Bytes::from(payload)),
                emit_response: ctx.expects_response(),
            });
            HostOperationResult::Done
        }

        HostOperation::ReplyError(error) => {
            use actr_framework::{Context as _, ReplyOutcome};
            let Some(deliver) = ctx.reply_deliver() else {
                return HostOperationResult::Error(ActrError::Internal(
                    "guest reply requires an inbound RPC reply context".to_string(),
                ));
            };
            deliver.deliver(ReplyOutcome {
                request_id: ctx.request_id().to_string(),
                result: Err(error),
                emit_response: ctx.expects_response(),
            });
            HostOperationResult::Done
        }

        HostOperation::Call(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => {
                    tracing::error!(route_key = req.route_key, "call: dest decode failed");
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
            };
            match ctx
                .call_raw(
                    &dest,
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                    req.timeout_ms,
                )
                .await
            {
                Ok(resp) => HostOperationResult::Bytes(resp.to_vec()),
                Err(e) => {
                    tracing::error!("call routing failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }

        HostOperation::Tell(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => {
                    tracing::error!(route_key = req.route_key, "tell: dest decode failed");
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
            };
            match ctx
                .tell_raw(
                    &dest,
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                )
                .await
            {
                Ok(()) => HostOperationResult::Done,
                Err(e) => {
                    tracing::error!("tell routing failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }

        HostOperation::Discover(req) => {
            match ctx.discover_route_candidate(&req.target_type).await {
                Ok(id) => HostOperationResult::Bytes(id.encode_to_vec()),
                Err(e) => {
                    tracing::error!("discover failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }

        HostOperation::ResolveDependency(req) => match ctx.resolve_dependency(&req.alias).await {
            Ok(actr_type) => HostOperationResult::Bytes(actr_type.encode_to_vec()),
            Err(error) => HostOperationResult::Error(error),
        },

        HostOperation::RegisterStream(req) => {
            let stream_id = req.stream_id;
            let callback_ctx = ctx.clone();
            let callback_workload_dispatch = workload_dispatch.clone();
            match ctx
                .register_stream(stream_id, move |chunk: DataChunk, sender| {
                    let ctx_for_executor = callback_ctx.clone();
                    let workload_dispatch = callback_workload_dispatch.clone();
                    Box::pin(async move {
                        let Some(workload_dispatch) = workload_dispatch.upgrade() else {
                            return Err(ActrError::Unavailable(
                                "actor runner terminated".to_string(),
                            ));
                        };
                        let invocation = crate::workload::InvocationContext {
                            self_id: actr_framework::Context::self_id(&ctx_for_executor).clone(),
                            caller_id: Some(sender.clone()),
                            request_id: format!(
                                "data-chunk:{}:{}",
                                chunk.stream_id, chunk.sequence
                            ),
                        };
                        let call_executor: crate::workload::HostAbiFn =
                            std::sync::Arc::new(move |pending| {
                                let ctx = ctx_for_executor.clone();
                                Box::pin(async move {
                                    stream_callback_host_operation_handler(ctx, pending).await
                                })
                            });
                        // Self-lock elimination: this callback fires later (on
                        // chunk arrival), never during the dispatch that
                        // registered it, so appending a DataChunk command to
                        // the runner's channel and awaiting it cannot deadlock.
                        workload_dispatch
                            .dispatch_data_chunk(chunk, sender, invocation, &call_executor)
                            .await
                    })
                })
                .await
            {
                Ok(()) => HostOperationResult::Done,
                Err(e) => {
                    tracing::error!("register_stream failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }

        HostOperation::UnregisterStream(req) => match ctx.unregister_stream(&req.stream_id).await {
            Ok(()) => HostOperationResult::Done,
            Err(e) => {
                tracing::error!("unregister_stream failed: {e:?}");
                HostOperationResult::Error(e)
            }
        },

        HostOperation::SendDataChunk(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => {
                    tracing::error!("send_data_chunk: dest decode failed");
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
            };
            let payload_type = match PayloadType::try_from(req.payload_type) {
                Ok(PayloadType::StreamReliable | PayloadType::StreamLatencyFirst) => {
                    PayloadType::try_from(req.payload_type).expect("checked payload type")
                }
                Ok(other) => {
                    tracing::error!(?other, "send_data_chunk: invalid stream payload type");
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
                Err(_) => {
                    tracing::error!(
                        payload_type = req.payload_type,
                        "send_data_chunk: unknown payload type"
                    );
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
            };
            match ctx.send_data_chunk(&dest, req.chunk, payload_type).await {
                Ok(()) => HostOperationResult::Done,
                Err(e) => {
                    tracing::error!("send_data_chunk failed: {e:?}");
                    HostOperationResult::Error(e)
                }
            }
        }
    }
}

fn lifecycle_invocation(
    actor_id: &ActrId,
    request_id: &'static str,
) -> crate::workload::InvocationContext {
    crate::workload::InvocationContext {
        self_id: actor_id.clone(),
        caller_id: None,
        request_id: request_id.to_string(),
    }
}

pub(crate) fn lifecycle_host_abi(
    ctx: crate::context::RuntimeContext,
    workload_dispatch: Arc<crate::executor::ActorHandle>,
) -> crate::workload::HostAbiFn {
    let workload_dispatch = Arc::downgrade(&workload_dispatch);
    std::sync::Arc::new(move |pending| {
        let ctx = ctx.clone();
        let workload_dispatch = workload_dispatch.clone();
        Box::pin(async move { host_operation_handler(ctx, workload_dispatch, pending).await })
    })
}

async fn stream_callback_host_operation_handler(
    ctx: crate::context::RuntimeContext,
    pending: crate::workload::HostOperation,
) -> crate::workload::HostOperationResult {
    use crate::workload::{HostOperation, HostOperationResult, decode_dest};
    use actr_framework::guest::dynclib_abi::code as abi_code;
    use actr_framework::{Context as _, Dest};
    use actr_protocol::PayloadType;

    match pending {
        HostOperation::CallRaw(req) => {
            match ctx
                .call_raw(
                    &Dest::Peer(req.target),
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                    req.timeout_ms,
                )
                .await
            {
                Ok(resp) => HostOperationResult::Bytes(resp.to_vec()),
                Err(e) => HostOperationResult::Error(e),
            }
        }
        HostOperation::Reply(_) => HostOperationResult::AbiError(abi_code::UNSUPPORTED_OP),
        HostOperation::ReplyError(_) => HostOperationResult::AbiError(abi_code::UNSUPPORTED_OP),
        HostOperation::Call(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR),
            };
            match ctx
                .call_raw(
                    &dest,
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                    req.timeout_ms,
                )
                .await
            {
                Ok(resp) => HostOperationResult::Bytes(resp.to_vec()),
                Err(e) => HostOperationResult::Error(e),
            }
        }
        HostOperation::Tell(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR),
            };
            match ctx
                .tell_raw(
                    &dest,
                    req.route_key,
                    guest_rpc_payload_type(req.payload_type),
                    bytes::Bytes::from(req.payload),
                )
                .await
            {
                Ok(()) => HostOperationResult::Done,
                Err(e) => HostOperationResult::Error(e),
            }
        }
        HostOperation::Discover(req) => {
            match ctx.discover_route_candidate(&req.target_type).await {
                Ok(id) => HostOperationResult::Bytes(id.encode_to_vec()),
                Err(e) => HostOperationResult::Error(e),
            }
        }
        HostOperation::ResolveDependency(req) => match ctx.resolve_dependency(&req.alias).await {
            Ok(actr_type) => HostOperationResult::Bytes(actr_type.encode_to_vec()),
            Err(error) => HostOperationResult::Error(error),
        },
        HostOperation::RegisterStream(_) => {
            tracing::error!("register_stream from inside a stream callback is not supported");
            HostOperationResult::AbiError(abi_code::UNSUPPORTED_OP)
        }
        HostOperation::UnregisterStream(req) => match ctx.unregister_stream(&req.stream_id).await {
            Ok(()) => HostOperationResult::Done,
            Err(e) => HostOperationResult::Error(e),
        },
        HostOperation::SendDataChunk(req) => {
            let dest = match decode_dest(&req.dest) {
                Some(d) => d,
                None => return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR),
            };
            let payload_type = match PayloadType::try_from(req.payload_type) {
                Ok(PayloadType::StreamReliable | PayloadType::StreamLatencyFirst) => {
                    PayloadType::try_from(req.payload_type).expect("checked payload type")
                }
                Ok(_) | Err(_) => {
                    return HostOperationResult::AbiError(abi_code::PROTOCOL_ERROR);
                }
            };
            match ctx.send_data_chunk(&dest, req.chunk, payload_type).await {
                Ok(()) => HostOperationResult::Done,
                Err(e) => HostOperationResult::Error(e),
            }
        }
    }
}

/// Map `ActrError` to a stable, unique numeric code for the wire
/// `ErrorResponse`.  Each variant has a distinct code so
/// `wire_code_to_actr_error` can reconstruct the exact variant on the
/// receiving side.
///
/// Code allocation (100xx namespace to avoid collisions with HTTP):
///
/// | code  | variant            |
/// |-------|--------------------|
/// | 10001 | Unavailable        |
/// | 10002 | TimedOut           |
/// | 10003 | NotFound           |
/// | 10004 | PermissionDenied   |
/// | 10005 | InvalidArgument    |
/// | 10006 | UnknownRoute       |
/// | 10007 | DependencyNotFound |
/// | 10008 | DecodeFailure      |
/// | 10009 | NotImplemented     |
/// | 10010 | Internal           |
/// | 10011 | ConnectionNotReady |
/// | 10012 | DeliveryUncertain  |
pub(crate) fn protocol_error_to_code(err: &ActrError) -> u32 {
    match err {
        ActrError::Unavailable(_) => 10001,
        ActrError::ConnectionNotReady(_) => 10011,
        ActrError::TimedOut => 10002,
        ActrError::NotFound(_) => 10003,
        ActrError::PermissionDenied(_) => 10004,
        ActrError::InvalidArgument(_) => 10005,
        ActrError::UnknownRoute(_) => 10006,
        ActrError::DependencyNotFound { .. } => 10007,
        ActrError::DecodeFailure(_) => 10008,
        ActrError::NotImplemented(_) => 10009,
        ActrError::Internal(_) => 10010,
        // Dedicated code so a responder's uncertain delivery round-trips as
        // the structured variant instead of collapsing into Internal: the
        // caller's idempotency correlation depends on `operation_id`
        // surviving this boundary.
        ActrError::DeliveryUncertain { .. } => 10012,
        // `ActrError` is `#[non_exhaustive]`: unmapped future variants fall
        // back to the generic Internal code so they round-trip as an
        // identifiable error rather than panicking at this boundary.
        _ => 10010,
    }
}

/// Reconstruct an `ActrError` from a wire code + message pair.
///
/// This is the inverse of `protocol_error_to_code`.  Unknown codes fall
/// back to `ActrError::Unavailable` to avoid silent data loss.
pub(crate) fn wire_code_to_actr_error(code: u32, message: String) -> ActrError {
    match code {
        10001 => ActrError::Unavailable(message),
        10002 => ActrError::TimedOut,
        10003 => ActrError::NotFound(message),
        10004 => ActrError::PermissionDenied(message),
        10005 => ActrError::InvalidArgument(message),
        10006 => ActrError::UnknownRoute(message),
        10007 => ActrError::DependencyNotFound {
            service_name: String::new(),
            message,
        },
        10008 => ActrError::DecodeFailure(message),
        10009 => ActrError::NotImplemented(message),
        10010 => ActrError::Internal(message),
        10011 => ActrError::ConnectionNotReady(ConnectionNotReadyInfo {
            retry_after_ms: parse_connection_not_ready_retry_hint(&message),
        }),
        10012 => parse_delivery_uncertain(&message),
        // Legacy HTTP-ish codes emitted before this scheme was introduced,
        // or any unknown future code: treat as Unavailable.
        _ => ActrError::Unavailable(format!("rpc error {code}: {message}")),
    }
}

/// Encode a `DeliveryUncertain` for the code+message wire boundary with
/// byte-length-prefixed fields. Operation/request IDs and causes are
/// unrestricted UTF-8 strings, so a length prefix preserves every character,
/// including control characters and both historical delimiters.
/// `wire_code_to_actr_error` also accepts the previous record-separator format
/// and the legacy Display form for mixed-version peers.
const DELIVERY_UNCERTAIN_WIRE_PREFIX: &str = "delivery-uncertain-v1:";

fn encode_delivery_uncertain_for_wire(err: &ActrError) -> String {
    match err {
        ActrError::DeliveryUncertain {
            operation_id,
            cause,
        } => {
            format!(
                "{DELIVERY_UNCERTAIN_WIRE_PREFIX}{}:{}:{operation_id}{cause}",
                operation_id.len(),
                cause.len(),
            )
        }
        other => other.to_string(),
    }
}

/// Reconstruct the structured `DeliveryUncertain` from the versioned wire
/// payload, the previous record-separator payload, or the legacy Display
/// string. Malformed input keeps the typed variant with a placeholder ID.
fn parse_delivery_uncertain(message: &str) -> ActrError {
    // A message carrying the v1 prefix must be interpreted only as v1. If its
    // length framing is malformed, falling through to the legacy separator
    // format could fabricate an operation ID from attacker-controlled text.
    if message.starts_with(DELIVERY_UNCERTAIN_WIRE_PREFIX) {
        return parse_length_prefixed_delivery_uncertain(message).unwrap_or_else(|| {
            ActrError::DeliveryUncertain {
                operation_id: "unknown".to_string(),
                cause: message.to_string(),
            }
        });
    }

    // Compatibility with the immediately previous typed encoding. It used a
    // record separator without escaping; reject a second separator instead of
    // silently assigning part of a field to the other field.
    if let Some((operation_id, cause)) = message.split_once('\u{001E}') {
        if !cause.contains('\u{001E}') {
            return ActrError::DeliveryUncertain {
                operation_id: operation_id.to_string(),
                cause: cause.to_string(),
            };
        }
    }
    // Legacy Display form from a peer built before the typed encoding:
    // best-effort split on the first `": "` (lossy when the operation id
    // itself contains `": "`), keeping the variant intact.
    const PREFIX: &str = "delivery uncertain for operation ";
    if let Some(rest) = message.strip_prefix(PREFIX) {
        if let Some((operation_id, cause)) = rest.split_once(": ") {
            return ActrError::DeliveryUncertain {
                operation_id: operation_id.to_string(),
                cause: cause.to_string(),
            };
        }
    }
    ActrError::DeliveryUncertain {
        operation_id: "unknown".to_string(),
        cause: message.to_string(),
    }
}

fn parse_length_prefixed_delivery_uncertain(message: &str) -> Option<ActrError> {
    let rest = message.strip_prefix(DELIVERY_UNCERTAIN_WIRE_PREFIX)?;
    let (operation_id_len, rest) = rest.split_once(':')?;
    let (cause_len, payload) = rest.split_once(':')?;
    let operation_id_len = operation_id_len.parse::<usize>().ok()?;
    let cause_len = cause_len.parse::<usize>().ok()?;
    let total_len = operation_id_len.checked_add(cause_len)?;
    if payload.len() != total_len || !payload.is_char_boundary(operation_id_len) {
        return None;
    }
    let (operation_id, cause) = payload.split_at(operation_id_len);
    Some(ActrError::DeliveryUncertain {
        operation_id: operation_id.to_string(),
        cause: cause.to_string(),
    })
}

fn parse_connection_not_ready_retry_hint(message: &str) -> Option<u64> {
    // ErrorResponse currently carries only a code and display string. Keep this
    // parser narrow and best-effort until the wire error payload grows typed details.
    if let Some(rest) = message.strip_prefix("connection not ready: retry after ") {
        return rest.strip_suffix("ms")?.parse().ok();
    }
    // Accept the previous Display form from older peers as well.
    let marker = "retry_after_ms=Some(";
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find(')')?;
    rest[..end].parse().ok()
}

/// Strategy A gate: decide whether the resident concurrency machinery (the
/// interleaved runner + conflict-key scheduler) should be engaged for an actor.
///
/// It is engaged **only** when the dispatch gate is on *and* the actor declares
/// at least one conflict key. A keyless actor — even with the default-on gate —
/// returns `false`, so the node keeps it on the serial `run_loop` with no
/// scheduler: default-on is bit-for-bit the M4 path for anything unkeyed, at
/// zero scheduling cost. This is a pure function so the routing promise is unit
/// testable without standing up a node.
fn scheduler_engaged(gate_on: bool, has_conflict_keys: bool) -> bool {
    gate_on && has_conflict_keys
}

/// Writer-preferring admission gate for durable mailbox batches.
///
/// The mailbox loop enters before `dequeue()` and retains the returned guard
/// through the entire batch. Shutdown closes the gate before scheduler / runner
/// intake, so Tokio's fair `RwLock` first blocks any later batch and then waits
/// for the already-dequeued batch to finish admission.
#[derive(Clone, Debug)]
struct MailboxBatchAdmission {
    open: Arc<RwLock<bool>>,
}

impl MailboxBatchAdmission {
    fn new() -> Self {
        Self {
            open: Arc::new(RwLock::new(true)),
        }
    }

    async fn enter(&self) -> Option<tokio::sync::OwnedRwLockReadGuard<bool>> {
        let guard = self.open.clone().read_owned().await;
        if *guard { Some(guard) } else { None }
    }

    async fn close(&self) {
        let mut open = self.open.write().await;
        *open = false;
    }
}

/// Abort fallback armed for the complete shutdown sequence. `ActrRef` bounds
/// background-task shutdown; if that outer task is aborted while a guest is
/// stuck, dropping this guard aborts the detached owning runner as well.
struct AbortActorRunnerOnDrop {
    runner: Arc<crate::executor::ActorHandle>,
    armed: bool,
}

impl AbortActorRunnerOnDrop {
    fn new(runner: Arc<crate::executor::ActorHandle>) -> Self {
        Self {
            runner,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AbortActorRunnerOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.runner.abort();
        }
    }
}

type ShutdownStep = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Ordered actor teardown:
///
/// 1. run the cancellation-sensitive prelude (quiesce durable mailbox batches,
///    then stop stream workers);
/// 2. stop scheduler intake and drain all admitted queued + in-flight work;
/// 3. close direct dispatch admission and wait for earlier sends to enqueue;
/// 4. invoke `on_stop` as a runner barrier;
/// 5. stop and join the owning runner task.
///
/// The abort guard makes cancellation of this coordinator deterministic too.
async fn shutdown_actor_runner(
    scheduler: Option<Arc<crate::dispatch::scheduler::SchedulerHandle>>,
    runner: Arc<crate::executor::ActorHandle>,
    pre_shutdown: ShutdownStep,
    on_stop: ShutdownStep,
) {
    let mut abort_guard = AbortActorRunnerOnDrop::new(runner.clone());
    pre_shutdown.await;
    if let Some(scheduler) = scheduler {
        scheduler.shutdown().await;
    }
    runner.stop_intake().await;
    on_stop.await;
    runner.shutdown().await;
    abort_guard.disarm();
}

/// Spawn the workload stop coordinator behind a boxed future boundary.
///
/// Keeping this concrete async state machine out of [`Inner::start_inner`]
/// also keeps the boxed startup future smaller and easier for older compilers
/// to lay out.
fn spawn_workload_stop_task(
    node: Arc<Inner>,
    actor_id: ActrId,
    credential_state: CredentialState,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let task: ShutdownStep = Box::pin(async move {
        shutdown.cancelled().await;
        let data_chunk_registry = node.data_chunk_registry.clone();
        let mailbox_batch_admission = node.mailbox_batch_admission.clone();
        let scheduler = node.dispatch_scheduler.clone();
        let runner = node.workload_dispatch.clone();
        let runner_for_stop = runner.clone();
        let quiesce_ingress = async move {
            // `dequeue()` marks a complete batch Inflight. Wait until every
            // record in an active batch has reached scheduler / runner
            // admission before either intake is closed; otherwise the tail
            // would resolve as terminated and be permanently ACKed unhandled.
            mailbox_batch_admission.close().await;
            // Once this returns, no stream callback can still enqueue actor
            // work or retain its RuntimeContext ownership chain.
            data_chunk_registry.shutdown().await;
        };
        let stop = async move {
            let stop_ctx = node
                .bootstrap_ctx_builder()
                .build_bootstrap(&actor_id, &credential_state.credential().await);
            let invocation = lifecycle_invocation(&actor_id, "lifecycle:on_stop");
            let call_executor = lifecycle_host_abi(stop_ctx.clone(), runner_for_stop.clone());
            if let Err(e) = crate::lifecycle::hooks::call_lifecycle_hook(
                "on_stop",
                runner_for_stop.on_stop(stop_ctx, invocation, &call_executor),
            )
            .await
            {
                tracing::warn!(error = %e, "workload on_stop returned Err");
            }
        };
        shutdown_actor_runner(scheduler, runner, Box::pin(quiesce_ingress), Box::pin(stop)).await;
    });
    tokio::spawn(task)
}

/// W6 config guard for the RTC backend (Stage 7 Phase A decision): reject
/// unsupported capabilities at startup instead of degrading silently.
/// `ice_udp_mux_port` has no upstream RTC equivalent; TURN over TLS/TCP is
/// silently skipped by webrtc-rs (issue #848), so it is a hard error —
/// the operator must fix the config, not discover a skipped server at runtime.
fn validate_rtc_config(config: &actr_config::WebRtcConfig) -> ActorResult<()> {
    if config.advanced.ice_udp_mux_port.is_some() {
        return Err(ActrError::Internal(
            "ice_udp_mux_port is not supported in the RTC implementation \
             (upstream webrtc-rs has no shared ICE UDP mux)"
                .to_string(),
        ));
    }
    if config.advanced.udp_ports.is_some() {
        return Err(ActrError::Internal(
            "WebRTC UDP port ranges are not supported in the RTC implementation \
             (webrtc-rs exposes explicit bind addresses but no EphemeralUDP range)"
                .to_string(),
        ));
    }
    for server in &config.ice_servers {
        for url in &server.urls {
            if url.starts_with("turns:") || url.contains("transport=tcp") {
                return Err(ActrError::Internal(format!(
                    "TURN over TLS/TCP is not supported in the RTC implementation \
                     (webrtc-rs issue #848): {url}"
                )));
            }
        }
    }
    Ok(())
}

impl Inner {
    #[allow(dead_code)]
    pub(crate) fn package_manifest(&self) -> Option<&actr_pack::PackageManifest> {
        self.package_manifest.as_ref()
    }

    /// Whether the inbound envelope is an explicit fire-and-forget tell.
    ///
    /// This is the ONLY tell marker: receivers never infer tell-ness from
    /// `timeout_ms == 0` (see the wire contract in `package.proto`).
    pub(crate) fn envelope_is_tell(envelope: &RpcEnvelope) -> bool {
        envelope.direction == Some(Direction::Tell as i32)
    }

    /// Direction-based routing decision for the server dispatch loops.
    ///
    /// Returns `Some(direction)` for the two dispatchable kinds (`Request`,
    /// `Tell`). Everything else — missing, `Unspecified`, `Response`
    /// (mislabel: responses are routed to pending maps by the gates and
    /// never reach a dispatch loop), or unknown future values — yields
    /// `None`, which the loops warn about and drop.
    pub(crate) fn dispatchable_direction(raw_direction: Option<i32>) -> Option<Direction> {
        match crate::wire::direction_for_routing(raw_direction) {
            // Response is routed to pending maps by the gates, never
            // dispatched; missing / Unspecified / unknown are invalid here.
            Ok(direction @ (Direction::Request | Direction::Tell)) => Some(direction),
            _ => None,
        }
    }

    /// Whether an inbound envelope expects a reply, decided solely from its
    /// direction label: `Request` → yes, `Tell` → no (fire-and-forget). Any
    /// non-dispatchable direction yields `None` so the caller can warn + drop.
    ///
    /// Single source of truth for the "does this dispatch expect a response?"
    /// policy shared by the Shell→Guest and mailbox receive loops.
    pub(crate) fn dispatch_expects_response(raw_direction: Option<i32>) -> Option<bool> {
        Self::dispatchable_direction(raw_direction).map(|direction| direction != Direction::Tell)
    }

    /// Build a RESPONSE envelope for a handled request.
    ///
    /// `timeout_ms` is always 0: per the `package.proto` wire contract only
    /// `DIRECTION_REQUEST` carries a positive deadline — RESPONSE (and TELL)
    /// use 0 as documented filler, and receivers MUST ignore it. Centralizing
    /// RESPONSE construction here pins that contract in one tested place.
    pub(crate) fn build_response_envelope(
        request_id: String,
        route_key: String,
        payload: Option<Bytes>,
        error: Option<actr_protocol::ErrorResponse>,
        traceparent: Option<String>,
        tracestate: Option<String>,
    ) -> RpcEnvelope {
        RpcEnvelope {
            request_id,
            route_key,
            payload,
            error,
            direction: Some(Direction::Response as i32),
            traceparent,
            tracestate,
            metadata: Vec::new(),
            timeout_ms: 0,
        }
    }

    fn duplicate_wait_timeout(timeout_ms: i64) -> Duration {
        if timeout_ms > 0 {
            Duration::from_millis(timeout_ms as u64)
        } else {
            DEDUP_TTL
        }
    }

    async fn wait_for_inflight_duplicate(
        mut waiter: DedupWaiter,
        timeout: Duration,
    ) -> ActorResult<Bytes> {
        let wait_for_result = async {
            loop {
                if let Some(result) = waiter.borrow().clone() {
                    return result;
                }

                if waiter.changed().await.is_err() {
                    if let Some(result) = waiter.borrow().clone() {
                        return result;
                    }
                    return Err(ActrError::Unavailable(
                        "duplicate request result unavailable".to_string(),
                    ));
                }
            }
        };

        match crate::timer::timeout(
            crate::timer::ids::NODE_RPC_RESPONSE,
            timeout,
            wait_for_result,
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ActrError::Unavailable(format!(
                "duplicate request in-flight timed out after {}ms",
                timeout.as_millis()
            ))),
        }
    }

    /// - Single-hop calls: effectively identical
    /// - Multi-hop calls: trace_id spans all hops, request_id per hop
    #[cfg_attr(
        feature = "opentelemetry",
        tracing::instrument(
            skip_all,
            name = "ActrNode.handle_incoming",
            fields(
                actr_id = %self.actor_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                route_key = %envelope.route_key,
                request_id = %envelope.request_id,
            )
        )
    )]
    pub async fn handle_incoming(
        &self,
        envelope: RpcEnvelope,
        caller: Option<IncomingCaller<'_>>,
        reply_sink: Option<crate::lifecycle::reply_deliver::ReplySink>,
        scope: Option<std::sync::Arc<crate::request_scope::RequestScope>>,
    ) -> IncomingOutcome {
        // Thin wrapper preserving the original flow: run synchronous
        // admission, then await the continuation to completion inline. This is
        // bit-for-bit the pre-B2 path (ACL → dedup → ctx → dispatch → dedup
        // complete), used directly by the mailbox lane and by the in-proc lane
        // when the dispatch gate is off.
        self.admit_incoming(envelope, caller, reply_sink, scope)
            .await
            .await
    }

    /// Synchronous admission of an inbound RPC, returning the continuation that
    /// finishes it.
    ///
    /// Everything that must happen *in arrival order* runs before this returns:
    /// ACL, dedup `check_or_mark`, context construction, conflict-key
    /// extraction, and — when the gate is on — the scheduler submission (whose
    /// queue semaphore is the back-pressure point). The returned
    /// [`IncomingContinuation`] is `'static + Send`, so an entry loop can push
    /// it into a `FuturesUnordered` and keep dequeuing while distinct-key
    /// dispatches run concurrently. Awaiting it yields the dispatch result and
    /// writes the dedup completion at the same point the old inline model did.
    ///
    /// The three pre-dispatch fast paths (ACL deny, dedup in-flight wait, dedup
    /// cache hit) return an already-resolved continuation — they never mark a
    /// dedup completion, exactly as before.
    ///
    /// ## RFC-0323 (EXPLICIT) continuation contract
    ///
    /// For `ReplyMode::Explicit`, the continuation routes handler errors
    /// through the shared `ReplyDeliver` first-write-wins gate:
    ///
    /// - **`Ok`, or `Err` after the `Reply<T>` was constructed and `Drop` fired
    ///   the Deliver** → `IncomingOutcome::Deferred`. The durable mailbox
    ///   message is **ACKed at dispatch-completion (handler return)** here,
    ///   *before* the reply is produced (§4 ack-at-return). The reply half
    ///   (RESPONSE envelope + dedup completion) runs later from
    ///   `ReplyDeliver::deliver_async`, which also releases the
    ///   `outstanding_replies` gauge slot.
    /// - **`Err` before the `Reply<T>` was constructed** (e.g. decode failure)
    ///   still uses the pump-owned `ReplyDeliver` when a reply sink exists, so
    ///   it follows the same deferred error path. If no sink exists, the
    ///   continuation falls back to the normal inline `Replied(Err)` path.
    ///
    /// **Deadlock rule (§5):** a `Return` handler must not synchronously call
    /// back into itself (it would block on its own still-held dispatch slot);
    /// such recursion requires `Explicit` so the handler can move the `Reply`
    /// aside and release the slot first.
    pub(crate) async fn admit_incoming(
        &self,
        mut envelope: RpcEnvelope,
        caller: Option<IncomingCaller<'_>>,
        reply_sink: Option<crate::lifecycle::reply_deliver::ReplySink>,
        scope: Option<std::sync::Arc<crate::request_scope::RequestScope>>,
    ) -> IncomingContinuation {
        let caller_id = caller.map(|(id, _)| id);
        let caller_type = caller.and_then(|(_, actor_type)| actor_type);
        // Package-less routes emitted by older generators had one leading
        // dot (`.Service.Method`). Normalize it before every admission-stage
        // consumer (reply-mode lookup, scheduler, and all runner variants)
        // sees the envelope. The guest boundary keeps the same normalization
        // as a defensive compatibility layer for direct internal callers.
        crate::workload::normalize_legacy_route_key(&mut envelope.route_key);

        // Log received message
        if let Some(caller) = caller_id {
            tracing::debug!(
                "📨 Handling incoming message: route_key={}, caller={}, request_id={}",
                envelope.route_key,
                caller,
                envelope.request_id
            );
        } else {
            tracing::debug!(
                "📨 Handling incoming message: route_key={}, request_id={}",
                envelope.route_key,
                envelope.request_id
            );
        }

        // 0. Get actor_id early for ACL check
        let actor_id = match self.actor_id.as_ref() {
            Some(id) => id,
            None => {
                return ready_continuation(Err(ActrError::Internal(
                    "Actor ID not set - node must be started before handling messages".to_string(),
                )));
            }
        };

        // 0.1. ACL Permission Check (before processing message), against the
        // caller type recorded when the message arrived.
        let acl_allowed = match check_acl_permission(
            caller_id,
            caller_type,
            actor_id,
            self.config.acl.as_ref(),
        ) {
            Ok(allowed) => allowed,
            Err(err_msg) => {
                return ready_continuation(Err(ActrError::Internal(format!(
                    "ACL check failed: {}",
                    err_msg
                ))));
            }
        };

        if !acl_allowed {
            tracing::warn!(
                severity = 5,
                error_category = "acl_denied",
                request_id = %envelope.request_id,
                route_key = %envelope.route_key,
                caller = %caller_id
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                "🚫 ACL: Permission denied"
            );

            let denied = ActrError::PermissionDenied(format!(
                "ACL denied: {} is not allowed to call {}",
                caller_id
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                actor_id
            ));
            return ready_continuation(Err(denied));
        }

        // 0.2. Deduplication: return cached response for retried request_ids.
        //
        // TELL envelopes keep their dedup entry (retry protection for
        // RpcReliable's up-to-5 same-request_id attempts), but their
        // duplicate handling differs: no caller is waiting for a reply, so
        // a duplicate arriving while the original is still in flight is
        // dropped immediately instead of blocking the receive loop for
        // `duplicate_wait_timeout` (which maps 0 to the 30 s DEDUP_TTL).
        let is_tell = Self::envelope_is_tell(&envelope);
        let outcome = {
            self.dedup_state
                .lock()
                .await
                .check_or_mark(&envelope.request_id)
        };
        match outcome {
            DedupOutcome::Fresh => {} // proceed normally
            DedupOutcome::InFlight(waiter) => {
                if is_tell {
                    tracing::debug!(
                        request_id = %envelope.request_id,
                        route_key = %envelope.route_key,
                        "duplicate tell in-flight; dropping duplicate immediately (fire-and-forget)"
                    );
                    return ready_continuation(Ok(Bytes::new()));
                }
                tracing::debug!(
                    request_id = %envelope.request_id,
                    route_key = %envelope.route_key,
                    "duplicate request in-flight; waiting for original result"
                );
                let timeout = Self::duplicate_wait_timeout(envelope.timeout_ms);
                return Box::pin(async move {
                    IncomingOutcome::Replied(
                        Self::wait_for_inflight_duplicate(waiter, timeout).await,
                    )
                });
            }
            DedupOutcome::Duplicate(cached) => {
                tracing::debug!(
                    request_id = %envelope.request_id,
                    route_key = %envelope.route_key,
                    "♻️ returning cached response for duplicate request_id"
                );
                return ready_continuation(cached);
            }
        }

        // Scoped-cancellation check: if the inherited request scope is already
        // terminal (caller dropped or root deadline expired), reject before any
        // handler work. The dedup mark above is left to expire via DEDUP_TTL —
        // same as the credential-missing path — since the caller is gone and
        // no response can be delivered anyway.
        if let Some(scope) = scope.as_ref() {
            if !scope.is_active() {
                tracing::debug!(
                    request_id = %envelope.request_id,
                    root_request_id = %scope.root_request_id(),
                    "request scope already cancelled before admission; rejecting"
                );
                return ready_continuation(Err(ActrError::TimedOut));
            }
        }

        // 1. Create Context with caller_id from transport layer
        let credential_state = match self.credential_state.clone() {
            Some(cs) => cs,
            None => {
                return ready_continuation(Err(ActrError::Internal(
                    "Credential not set - node must be started before handling messages"
                        .to_string(),
                )));
            }
        };
        let ctx = self.make_runtime_context(
            actor_id,
            caller_id, // caller_id from transport layer (MessageRecord.from)
            &envelope.request_id,
            &credential_state.credential().await,
        );
        // Inherit the claimed request scope (if any) so nested `call_raw` from
        // the guest derives child scopes under the same root deadline/token.
        let mut ctx = match scope.clone() {
            Some(s) => ctx.with_scope(s),
            None => ctx,
        };

        // RFC-0323: query the route's ReplyMode. Linked Rust dispatchers carry
        // the generated route match. Foreign linked workloads and Component
        // Model packages use an explicit host delivery boundary: generated
        // RETURN routes submit their encoded return value through `reply_raw`,
        // while generated EXPLICIT routes own delivery through `Reply<T>`.
        // This keeps the runtime from maintaining a second proto route table.
        let reply_mode = self
            .linked_handle
            .as_ref()
            .map(|h| h.reply_mode_for_route(&envelope.route_key))
            .unwrap_or_else(|| {
                if self.package_uses_explicit_reply_boundary {
                    actr_protocol::ReplyMode::Explicit
                } else {
                    actr_protocol::ReplyMode::Return
                }
            });
        let expects_response = Inner::dispatch_expects_response(envelope.direction).unwrap_or(true);

        // For EXPLICIT routes, build the `ReplyDeliver`, stash it on the
        // context (the generated dispatch arm reads it via `ctx.reply_deliver()`),
        // and keep a pump-side clone so the continuation can route handler-Err
        // outcomes through the same first-write-wins `deliver` (RFC-0323 §5).
        //
        // RFC-0323 §4 (ack-at-return): for the mailbox path, the durable
        // message is ACKed at dispatch-completion (handler return), NOT at
        // reply time — so the pump can keep admitting while the reply is still
        // pending. The ack handle is captured here and fired from the
        // continuation's `Deferred` branch; `ReplyDeliver::deliver_async`
        // sends only the reply (no ack).
        let mailbox_ack_handle: Option<(Arc<dyn Mailbox>, uuid::Uuid)> = match &reply_sink {
            Some(crate::lifecycle::reply_deliver::ReplySink::Mailbox { msg_id, .. }) => {
                Some((self.mailbox.clone(), *msg_id))
            }
            _ => None,
        };
        let mut explicit_attempt_id = None;
        let explicit_deliver: Option<crate::lifecycle::reply_deliver::ReplyDeliver> =
            if reply_mode == actr_protocol::ReplyMode::Explicit {
                match reply_sink {
                    Some(sink) => {
                        // RFC-0323: plumb the caller deadline (arrival +
                        // `envelope.timeout_ms`) onto the context so the
                        // generated EXPLICIT arm builds `Reply<T>` with a
                        // real deadline instead of `None`. `timeout_ms == 0`
                        // means "no timeout" → `None` (a TELL or an
                        // unbounded call). The wall-clock read lives in the
                        // exempt dedup module — this deadline is a
                        // caller-side bound, not a tokio-await deadline.
                        let reply_deadline =
                            crate::lifecycle::dedup::caller_reply_deadline(envelope.timeout_ms);
                        let rtc_route: RtcResponseRoute = self.rtc_manager.clone();
                        let attempt_id = self.outstanding_replies.allocate_attempt_id();
                        explicit_attempt_id = Some(attempt_id);
                        let (deliver, _flag) = crate::lifecycle::reply_deliver::ReplyDeliver::new(
                            self.websocket_gate.clone(),
                            rtc_route,
                            self.dedup_state.clone(),
                            self.outstanding_replies.clone(),
                            attempt_id,
                            envelope.request_id.clone(),
                            envelope.route_key.clone(),
                            envelope.traceparent.clone(),
                            envelope.tracestate.clone(),
                            sink,
                            expects_response,
                        );
                        // Start every admission unclaimed. Component dispatchers
                        // return a reserved deferred marker only after selecting
                        // an EXPLICIT route; ordinary zero-byte return payloads
                        // must still use the normal return-byte fallback.
                        let reply_claimed =
                            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        // Register the exact deliver for this admission. The
                        // TTL sweep uses its first-write-wins gate, and all
                        // asynchronous completion uses the attempt ID so an
                        // old same-ID reply cannot release or complete a retry.
                        self.outstanding_replies
                            .register_with_deliver(
                                &envelope.request_id,
                                attempt_id,
                                deliver.clone(),
                                reply_deadline,
                            )
                            .await;
                        // Pump-side clone: lets `wrap_outcome` route a
                        // handler-Err through the same first-write-wins
                        // `deliver` as a spawned `Reply::send` / `Drop`.
                        let pump_deliver = deliver.clone();
                        ctx.set_explicit_reply(
                            std::sync::Arc::new(deliver),
                            expects_response,
                            reply_deadline,
                            reply_claimed,
                        );
                        Some(pump_deliver)
                    }
                    None => None,
                }
            } else {
                None
            };

        // 2. Dispatch
        let dispatch_ctx = crate::workload::InvocationContext {
            self_id: actor_id.clone(),
            caller_id: caller_id.cloned(),
            request_id: envelope.request_id.clone(),
        };
        let ctx_for_executor = ctx.clone();
        let call_executor = lifecycle_host_abi(ctx_for_executor, self.workload_dispatch.clone());

        // The continuation logs its result and writes the dedup completion at the
        // dispatch completion point — same write-back timing as the old inline
        // lock model, for both the gate-on and gate-off branches below.
        let dedup_state = self.dedup_state.clone();
        let request_id = envelope.request_id.clone();
        let route_key = envelope.route_key.clone();
        // `request_id_err` / `expects_response_err` feed the EXPLICIT-Err
        // `ReplyOutcome` the pump hands to `deliver` inside `wrap_outcome`.
        // `finish` moves the originals into its own capture, so the wrapper
        // needs its own copies to avoid a use-after-move.
        let request_id_err = envelope.request_id.clone();
        let expects_response_err = expects_response;
        let outstanding_replies_for_fallback = self.outstanding_replies.clone();
        let allow_component_return_fallback = self.package_uses_explicit_reply_boundary;
        let finish = move |result: ActorResult<Bytes>| async move {
            match &result {
                Ok(_) => tracing::debug!(
                    request_id = %request_id,
                    route_key = %route_key,
                    "✅ Message handled successfully"
                ),
                Err(e) => tracing::error!(
                    severity = 6,
                    error_category = "handler_error",
                    request_id = %request_id,
                    route_key = %route_key,
                    "❌ Message handling failed: {:?}", e
                ),
            }
            // Completion happens on handler Err too, so a retried request (or
            // tell) observes the recorded failure instead of re-running the
            // handler. For a successful TELL the response bytes are never
            // sent, so cache empty bytes instead of retaining the unsent
            // payload for the full DEDUP_TTL.
            let cached = match (&result, is_tell) {
                (Ok(_), true) => Ok(Bytes::new()),
                _ => result.clone(),
            };
            dedup_state.lock().await.complete(&request_id, cached);
            result
        };

        // EXPLICIT continuation: dispatch, then route the outcome through the
        // first-write-wins `deliver` (handler-Err) / defer to the Reply<T>
        // (Ok), ACKing the durable mailbox at dispatch-completion (RFC-0323
        // §4). RETURN continuation: dispatch → finish → Replied. Shared by
        // both gate-on and gate-off branches.
        let wrap_outcome = move |result: ActorResult<Bytes>| async move {
            if let Some(deliver) = &explicit_deliver {
                if allow_component_return_fallback
                    && matches!(&result, Ok(bytes) if !bytes.is_empty())
                {
                    if let Some(attempt_id) = explicit_attempt_id {
                        outstanding_replies_for_fallback
                            .complete_attempt(attempt_id)
                            .await;
                    }
                    return IncomingOutcome::Replied(finish(result).await);
                }
                // EXPLICIT. For Ok the Reply<T> owns the success reply
                // (send/Drop fires `deliver`); the pump does NOT deliver.
                // For Err the pump routes the error through `deliver` so a
                // spawned `Reply::send` / Drop that already won the CAS
                // makes this a no-op, and otherwise the Deliver sends the
                // error RESPONSE + completes dedup + releases the gauge —
                // no double reply, no Done(Err)→Done(Ok) cache flip.
                if let Err(e) = &result {
                    deliver.deliver(actr_framework::ReplyOutcome {
                        request_id: request_id_err.clone(),
                        result: Err(e.clone()),
                        emit_response: expects_response_err,
                    });
                }
                // RFC-0323 §4: ACK the durable mailbox message at
                // dispatch-completion (handler return), before the reply
                // follows from the Deliver.
                if let Some((mb, mid)) = &mailbox_ack_handle {
                    crate::lifecycle::node::mailbox_ack(mb.clone(), *mid, &request_id_err).await;
                }
                IncomingOutcome::Deferred
            } else {
                IncomingOutcome::Replied(finish(result).await)
            }
        };

        // Route through the conflict-key scheduler when the gate is on; otherwise
        // the continuation dispatches straight to the runner — bit-for-bit the
        // B1 path once awaited.
        if let Some(scheduler) = &self.dispatch_scheduler {
            let key = self.conflict_keys.extract(
                &envelope.route_key,
                caller_id,
                envelope.payload.as_deref().unwrap_or_default(),
            );
            let workload = self.workload_dispatch.clone();
            let env = envelope.clone();
            let ctx_run = ctx.clone();
            let invocation = dispatch_ctx;
            let executor = call_executor;
            let run: crate::dispatch::scheduler::DispatchFn = Box::new(move || {
                Box::pin(async move {
                    if let Some(scope) = ctx_run.scope.as_ref() {
                        if !scope.is_active() {
                            tracing::debug!(
                                request_id = %env.request_id,
                                "request scope cancelled while queued in dispatch scheduler; skipping"
                            );
                            return Err(ActrError::TimedOut);
                        }
                    }
                    tracing::debug!(
                        "dispatch scheduler started job: route_key={}, request_id={}",
                        env.route_key,
                        env.request_id
                    );
                    workload
                        .dispatch_envelope(env, ctx_run, invocation, &executor)
                        .await
                })
            });
            // Submission (queue-slot acquisition + arrival-order enqueue) happens
            // now, before returning — this `.await` is the back-pressure point.
            tracing::debug!(
                "admitting inbound RPC to dispatch scheduler: route_key={}, \
                 caller={:?}, conflict_key={:?}",
                envelope.route_key,
                caller_id,
                key
            );
            let rx = scheduler.submit(key, run).await;
            Box::pin(async move {
                let result = rx.await.unwrap_or_else(|_| {
                    Err(ActrError::Unavailable(
                        "dispatch scheduler terminated".to_string(),
                    ))
                });
                wrap_outcome(result).await
            })
        } else {
            let workload = self.workload_dispatch.clone();
            Box::pin(async move {
                if let Some(scope) = ctx.scope.as_ref() {
                    if !scope.is_active() {
                        tracing::debug!(
                            request_id = %envelope.request_id,
                            "request scope cancelled before dispatch; rejecting"
                        );
                        return wrap_outcome(Err(ActrError::TimedOut)).await;
                    }
                }
                let result = workload
                    .dispatch_envelope(envelope, ctx, dispatch_ctx, &call_executor)
                    .await;
                wrap_outcome(result).await
            })
        }
    }

    /// Build a new `Inner` from config and runtime workload.
    ///
    /// This is the internal constructor behind the public node builders and
    /// Hyper package attach helpers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn build(
        config: actr_config::RuntimeConfig,
        workload: crate::workload::Workload,
        package_manifest: Option<actr_pack::PackageManifest>,
        packaged_lock: Option<actr_config::lock::LockFile>,
        linked_dependencies: Option<crate::LinkedDependencyMetadata>,
        mailbox_backpressure_threshold: usize,
        credential_expiry_warning: Duration,
        dispatch_concurrency: crate::config::DispatchConcurrency,
        conflict_keys: Option<crate::dispatch::ConflictKeySpec>,
        membership_controller_enabled: bool,
    ) -> ActorResult<Self> {
        use crate::outbound::{Gate, HostGate};
        use crate::wire::webrtc::{ReconnectConfig, SignalingConfig, WebSocketSignalingClient};

        tracing::info!("🚀 Initializing ActrNode");

        // Initialize Mailbox
        let mailbox_path = config
            .mailbox_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ":memory:".to_string());

        tracing::info!("📂 Mailbox database path: {}", mailbox_path);

        let mailbox: Arc<dyn actr_runtime_mailbox::Mailbox> = Arc::new(
            actr_runtime_mailbox::SqliteMailbox::new(&mailbox_path)
                .await
                .map_err(|e| {
                    actr_protocol::ActrError::Unavailable(format!("Mailbox init failed: {e}"))
                })?,
        );

        // Initialize Dead Letter Queue
        let dlq_path = if mailbox_path == ":memory:" {
            ":memory:".to_string()
        } else {
            format!("{mailbox_path}.dlq")
        };

        let dlq: Arc<dyn actr_runtime_mailbox::DeadLetterQueue> = Arc::new(
            actr_runtime_mailbox::SqliteDeadLetterQueue::new_standalone(&dlq_path)
                .await
                .map_err(|e| {
                    actr_protocol::ActrError::Unavailable(format!("DLQ init failed: {e}"))
                })?,
        );
        tracing::info!("✅ Dead Letter Queue initialized");

        // Initialize signaling client
        let webrtc_role = if config.webrtc.advanced.prefer_answerer() {
            Some("answer".to_string())
        } else {
            None
        };

        let signaling_config = SignalingConfig {
            server_url: config.signaling_url.clone(),
            connection_timeout: 30,
            heartbeat_interval: 30,
            reconnect_config: ReconnectConfig::default(),
            auth_config: None,
            webrtc_role,
            inbound_queue: crate::wire::webrtc::InboundQueueConfig::default(),
        };

        let client = Arc::new(WebSocketSignalingClient::new(signaling_config));
        // The recovery supervisor is present for every node, including
        // headless runtimes that never create a public NetworkEventHandle
        // (#568). Resource facts may queue before `start()` spawns the
        // reconciler; `create_network_event_handle` only retargets the
        // profile on this eagerly created channel.
        let (supervisor_fact_sink, network_event_internal) =
            crate::lifecycle::network_event::supervisor_internal_channel_with_profile(
                LifecycleProfile::Ungated,
            );
        let send_projection_rx = network_event_internal.subscribe_send_projection();
        client.set_supervisor_fact_sink(supervisor_fact_sink.clone());
        let signaling_client: Arc<dyn crate::wire::webrtc::SignalingClient> = client;

        // Initialize inproc infrastructure (Shell ↔ Guest)
        let shell_to_workload = Arc::new(HostTransport::new());
        let workload_to_shell = Arc::new(HostTransport::new());
        let inproc_gate = Gate::Host(Arc::new(HostGate::new(shell_to_workload.clone())));

        // Root shutdown token; the data chunk registry gets a child so a
        // node-wide shutdown drains all per-stream workers (no orphans).
        let shutdown_token = CancellationToken::new();
        let data_chunk_registry = Arc::new(DataChunkRegistry::with_shutdown(
            shutdown_token.child_token(),
        ));
        let media_frame_registry = Arc::new(MediaFrameRegistry::new());

        tracing::info!("✅ Inproc infrastructure initialized (bidirectional Shell ↔ Guest)");

        if packaged_lock.is_some() && package_manifest.is_none() {
            return Err(ActrError::InvalidArgument(
                "package dependency locks require an authenticated package manifest".to_string(),
            ));
        }
        if packaged_lock.is_some() && linked_dependencies.is_some() {
            return Err(ActrError::InvalidArgument(
                "a runtime cannot combine package-lock and linked-manifest dependency aliases"
                    .to_string(),
            ));
        }
        let package_backed = package_manifest.is_some();
        let actr_lock = if let Some(lock) = packaged_lock {
            lock.validate_aliases().map_err(|error| {
                actr_protocol::ActrError::InvalidArgument(format!(
                    "invalid packaged manifest.lock.toml: {error}"
                ))
            })?;
            tracing::info!(
                "📋 Loaded packaged manifest.lock.toml with {} dependencies",
                lock.dependencies.len()
            );
            Some(Arc::new(lock))
        } else {
            if package_backed {
                tracing::warn!(
                    "⚠️ manifest.lock.toml not found in package. Continuing without dependency fingerprints."
                );
            }
            None
        };
        let alias_map = Arc::new(match linked_dependencies.as_ref() {
            Some(metadata) => metadata.aliases.clone(),
            None => actr_lock
                .as_deref()
                .map(build_alias_map)
                .unwrap_or_default(),
        });
        let dependency_fingerprints = Arc::new(match linked_dependencies {
            Some(metadata) => metadata.fingerprints,
            None => actr_lock
                .as_deref()
                .map(build_dependency_fingerprints)
                .unwrap_or_default(),
        });

        // ── Dispatch scheduling layer (B2 native / M5 wasm) ─────────────────
        // Strategy A (default-on, keyless zero-overhead): the gate now defaults
        // *on*, but the resident concurrency machinery (interleaved runner +
        // scheduler) is engaged **only when the gate is on AND at least one
        // conflict key is declared** ([`scheduler_engaged`]). A keyless actor —
        // even with the gate on — stays on the serial `run_loop` with no
        // scheduler spawned, bit-for-bit the M4 path: default-on costs it
        // nothing. When engaged, the runner mode is `Interleaved` regardless of
        // backend; the executor then routes by the workload's actual
        // concurrency capability:
        //   * `Linked`  → native `&self` concurrency (B2)
        //   * `Wasm`    → resident V2 `run_concurrent` region (M5)
        //   * `DynClib` → serial `run_loop`
        // The 0.1.0 sync-world WASM ABI is retired and rejected at load, so
        // DynClib is the only packaged serial fallback. The second safety net
        // is orthogonal to the gate default: whenever a scheduler *is* running,
        // an undeclared method still projects to the global
        // `ConflictKey::Serial` barrier and can never interleave.
        let conflict_keys = Arc::new(conflict_keys.unwrap_or_default());
        let gate_on = dispatch_concurrency.enabled;
        let scheduler_on = scheduler_engaged(gate_on, !conflict_keys.is_empty());
        let is_linked = matches!(workload, crate::workload::Workload::Linked(_));

        // Retain a clone of the linked handle so the pump can query
        // `reply_mode_for_route` before dispatch (RFC-0323). Only the `Linked`
        // variant carries a host-callable dispatcher; Component Model guests
        // use the explicit host boundary selected above, while DynClib keeps
        // its legacy RETURN boundary.
        let linked_handle = match &workload {
            crate::workload::Workload::Linked(handle) => Some(handle.clone()),
            #[cfg(feature = "wasm-engine")]
            crate::workload::Workload::Wasm(_) => None,
            #[cfg(feature = "dynclib-engine")]
            crate::workload::Workload::DynClib(_) => None,
        };

        let package_uses_explicit_reply_boundary = workload.uses_explicit_reply_boundary();

        let runner_mode = if scheduler_on {
            crate::executor::RunnerMode::Interleaved
        } else {
            crate::executor::RunnerMode::Serial
        };
        let workload_dispatch = crate::executor::spawn_runner_with_mode(
            workload,
            runner_mode,
            dispatch_concurrency.dispatch_timeout,
        );

        let dispatch_scheduler = if scheduler_on {
            tracing::info!(
                budget = dispatch_concurrency.budget,
                queue_cap = dispatch_concurrency.queue_cap,
                dispatch_timeout_ms = dispatch_concurrency
                    .dispatch_timeout
                    .map(|d| d.as_millis() as u64),
                is_linked,
                "dispatch concurrency enabled"
            );
            Some(Arc::new(
                crate::dispatch::scheduler::SchedulerHandle::spawn(
                    dispatch_concurrency.budget,
                    dispatch_concurrency.queue_cap,
                ),
            ))
        } else {
            None
        };

        tracing::info!("✅ ActrNode initialized");

        Ok(Self {
            config,
            mailbox,
            dlq,
            inproc_gate,
            outproc_gate: None, // Populated in start() once WebRTC / PeerGate is ready.
            _data_chunk_registry_cleanup: DataChunkRegistryCleanupGuard::new(&data_chunk_registry),
            data_chunk_registry,
            media_frame_registry,
            signaling_client,
            actor_id: None,
            credential_state: None,
            session_state: None,
            #[cfg(feature = "test-utils")]
            test_faults: None,
            rtc_manager: None,
            rtc_inbound_mailbox_slot: None,
            peer_transport: None,
            websocket_gate: None,
            shell_to_workload: Some(shell_to_workload),
            workload_to_shell: Some(workload_to_shell),
            shutdown_token,
            mailbox_batch_admission: MailboxBatchAdmission::new(),
            actr_lock,
            alias_map,
            dependency_fingerprints,
            network_event_rx: None,
            network_event_debounce_config: None,
            network_event_internal: Some(network_event_internal),
            send_projection_rx: Some(send_projection_rx),
            supervisor_fact_sink: Some(supervisor_fact_sink),
            dedup_state: Arc::new(Mutex::new(DedupState::new())),
            outstanding_replies: Arc::new(
                crate::lifecycle::outstanding_replies::OutstandingReplies::new(),
            ),
            max_pending_replies: 1024,
            package_manifest,
            realm_client_key: None,
            preregistered_credential: None,
            preregistered_registration_context: None,
            peer_types: PeerTypes::default(),
            workload_dispatch: Arc::new(workload_dispatch),
            linked_handle,
            package_uses_explicit_reply_boundary,
            dispatch_scheduler,
            conflict_keys,
            hook_observer: None,
            mailbox_backpressure_threshold,
            credential_expiry_warning,
            membership_controller_enabled,
        })
    }

    /// Snapshot the current runtime handles into a `BootstrapContextBuilder`.
    ///
    /// The returned builder is cloned into long-lived hook closures and into
    /// `ActrRefShared` so those paths can materialize bootstrap contexts
    /// without retaining a reference back to `Inner`. The snapshot freezes
    /// `outproc_gate` and `actr_lock` at call time — callers that want to
    /// observe a later-initialized `outproc_gate` must rebuild.
    pub(crate) fn bootstrap_ctx_builder(&self) -> BootstrapContextBuilder {
        BootstrapContextBuilder::new(
            self.inproc_gate.clone(),
            self.outproc_gate.clone(),
            self.data_chunk_registry.clone(),
            self.media_frame_registry.clone(),
            self.signaling_client.clone(),
            self.actr_lock.clone(),
            self.alias_map.clone(),
            self.dependency_fingerprints.clone(),
            self.session_state.clone(),
            self.session_state
                .as_ref()
                .and_then(SessionState::generation_sync)
                .unwrap_or(0),
        )
    }

    /// Build a `RuntimeContext` for the per-request dispatch path.
    ///
    /// Unlike `BootstrapContextBuilder::build_bootstrap`, this carries the
    /// envelope's caller identity and request id through into the context.
    pub(crate) fn make_runtime_context(
        &self,
        self_id: &ActrId,
        caller_id: Option<&ActrId>,
        request_id: &str,
        credential: &AIdCredential,
    ) -> RuntimeContext {
        RuntimeContext::new_with_alias_map(
            self_id.clone(),
            caller_id.cloned(),
            request_id.to_string(),
            self.inproc_gate.clone(),
            self.outproc_gate.clone(),
            self.data_chunk_registry.clone(),
            self.media_frame_registry.clone(),
            self.signaling_client.clone(),
            credential.clone(),
            self.actr_lock.clone(),
            self.alias_map.clone(),
            self.dependency_fingerprints.clone(),
            self.session_state.clone(),
            self.session_state
                .as_ref()
                .and_then(SessionState::generation_sync)
                .unwrap_or(0),
        )
    }

    /// Create network event processing infrastructure (called on demand, before `start()`).
    ///
    /// # Parameters
    /// - `debounce_ms`: Debounce window in milliseconds. If 0, no debounce.
    ///
    /// # Panics
    /// Panics if called more than once.
    pub fn create_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.create_network_event_handle_with_profile(debounce_ms, LifecycleProfile::Ungated)
    }

    /// Create network event infrastructure whose recovery eligibility is gated
    /// by authoritative app lifecycle events. Mobile bindings use this entry
    /// point and must inject the initial foreground/background state.
    pub fn create_gated_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.create_network_event_handle_with_profile(debounce_ms, LifecycleProfile::Gated)
    }

    fn create_network_event_handle_with_profile(
        &mut self,
        debounce_ms: u64,
        profile: LifecycleProfile,
    ) -> crate::lifecycle::NetworkEventHandle {
        if self.network_event_rx.is_some() {
            panic!("create_network_event_handle() can only be called once");
        }

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(100);

        let debounce_config = if debounce_ms > 0 {
            Some(crate::lifecycle::network_event::DebounceConfig {
                window: std::time::Duration::from_millis(debounce_ms),
            })
        } else {
            None
        };

        // The internal channel was created eagerly with `Ungated` at node
        // construction (#568): a fact sink over its sender is already wired
        // into the signaling client and session state, so signaling and
        // session facts share the supervisor's policy-ordered input queue.
        // Creating the handle only retargets the profile in place.
        self.network_event_internal
            .as_mut()
            .expect("recovery supervisor channel must exist before start")
            .set_profile(profile);

        self.network_event_rx = Some(event_rx);
        self.network_event_debounce_config = debounce_config;

        tracing::info!(
            debounce_ms,
            ?profile,
            channel_capacity = 100_u64,
            "network_event.node.handle_created"
        );

        crate::lifecycle::NetworkEventHandle::new_with_fact_sink(
            event_tx,
            self.supervisor_fact_sink
                .clone()
                .expect("recovery supervisor fact sink must exist before start"),
        )
    }

    /// Use the same platform signer throughout the credential lifecycle,
    /// retaining the existing key-file path for desktop Hosts.
    pub(crate) fn resolved_realm_client_key(
        &self,
    ) -> Result<Option<crate::issuer_client::RealmClientKey>, crate::error::HyperError> {
        match &self.realm_client_key {
            Some(key) => Ok(Some(key.clone())),
            None => crate::issuer_client::RealmClientKey::load_for(
                self.config.realm_client_key.as_deref(),
                &self.config.realm,
            ),
        }
    }

    /// Attach a credential already issued by Issuer so that `start()` can skip
    /// the signaling registration step.
    ///
    /// Called by the Hyper layer between `Hyper::register()` and `Hyper::start()`.
    pub fn set_preregistered_credential(&mut self, register_ok: register_response::RegisterOk) {
        tracing::debug!(
            "Pre-registered credential attached; start() will skip Issuer registration"
        );
        self.preregistered_credential = Some(register_ok);
    }

    pub(crate) fn set_preregistered_registration_context(&mut self, ctx: RegistrationContext) {
        self.preregistered_registration_context = Some(ctx);
    }

    /// Start the system behind a type-erased future boundary.
    pub fn start(self) -> NodeStartFuture {
        Box::pin(self.start_inner())
    }

    async fn start_inner(mut self) -> ActorResult<ActrRef> {
        tracing::info!("🚀 Starting ActrNode");
        tracing::info!("Actr Rust version: {}", env!("CARGO_PKG_VERSION"));

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 1. Build RegisterRequest
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // Get ActrType from configuration
        let actr_type = self.config.actr_type().clone();
        tracing::info!("📋 Actor type: {}", actr_type);

        // ServiceSpec is derived by the Hyper layer from the verified package
        // (see `service_spec::calculate_service_spec_from_package`). The raw
        // ActrNode::start() path has no package context and always sends None
        // on its own RegisterRequest; callers that need a spec must go
        // through `Hyper::register()`.
        let service_spec = None;

        let register_request = RegisterRequest {
            actr_type: actr_type.clone(),
            realm: self.config.realm.clone(),
            service_spec,
            acl: self.config.acl.clone(),
            service: None,
            ws_address: None,
            auth_mode: Some(RegisterAuthMode::Linked as i32),
            ..Default::default()
        };

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 1. Obtain registration info (Hyper pre-injected or Issuer HTTP)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        let client_key = self
            .resolved_realm_client_key()
            .map_err(|e| ActrError::InvalidArgument(e.to_string()))?;
        let register_ok = if let Some(injected) = self.preregistered_credential.take() {
            tracing::info!(
                "Using Hyper pre-injected registration credential; skipping Issuer registration"
            );
            injected
        } else {
            let issuer_endpoint = &self.config.issuer_endpoint;
            tracing::info!(
                issuer_endpoint = %issuer_endpoint,
                "Registering actor with Issuer via HTTP"
            );
            let mut issuer = IssuerClient::new(issuer_endpoint);
            if let Some(key) = client_key.clone() {
                issuer = issuer.with_client_key(key);
            }
            let resp = issuer
                .register_linked(register_request.clone())
                .await
                .map_err(|e| ActrError::Unavailable(format!("Issuer registration failed: {e}")))?;
            match resp.result {
                Some(register_response::Result::Success(ok)) => {
                    tracing::info!("✅ Issuer HTTP registration successful");
                    ok
                }
                Some(register_response::Result::Error(error)) => {
                    tracing::error!(
                        severity = 10,
                        error_category = "registration_error",
                        error_code = error.code,
                        "❌ Issuer registration failed: code={}, message={}",
                        error.code,
                        error.message
                    );
                    return Err(ActrError::Unavailable(format!(
                        "Issuer registration rejected: {} (code: {})",
                        error.message, error.code
                    )));
                }
                None => {
                    tracing::error!(
                        severity = 10,
                        error_category = "registration_error",
                        "❌ Issuer registration response missing result"
                    );
                    return Err(ActrError::Unavailable(
                        "Invalid Issuer registration response: missing result".to_string(),
                    ));
                }
            }
        };

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 3. Set credential on signaling client, then connect signaling WS
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // The signaling server requires credential params in the WS URL for
        // authentication. We must set actor_id + credential BEFORE connecting
        // so that the signaling handshake includes them in authenticated headers.
        let pre_connect_credential_state = {
            let actor_id = register_ok.actr_id.clone();
            let credential_state = CredentialState::new(
                register_ok.credential.clone(),
                register_ok.credential_expires_at,
                Some(register_ok.turn_credential.clone()),
            );
            self.signaling_client.set_actor_id(actor_id).await;
            self.signaling_client
                .set_credential_state(credential_state.clone())
                .await;
            credential_state
        };

        // Install the signaling-side hook callback so that
        // SignalingConnectStart / Connected / Disconnected events flow
        // through the framework tracing defaults and into a
        // user-installed observer. Done BEFORE connect() so the initial
        // attempt produces a SignalingConnectStart event.
        {
            let actor_id = register_ok.actr_id.clone();
            let credential_state = pre_connect_credential_state.clone();
            // Snapshot at this point — outproc_gate is still None here, so
            // signaling-event contexts will carry None for outproc_gate
            // (matching the pre-existing behavior prior to B13 refactor).
            let ctx_builder_snapshot = self.bootstrap_ctx_builder();
            let ctx_builder: crate::lifecycle::hooks::HookContextBuilder = Arc::new(move || {
                let snapshot = ctx_builder_snapshot.clone();
                let actor_id = actor_id.clone();
                let credential_state = credential_state.clone();
                Box::pin(async move {
                    Some(snapshot.build_bootstrap(&actor_id, &credential_state.credential().await))
                })
            });
            let cb = crate::lifecycle::hooks::build_hook_callback(
                self.hook_observer.clone(),
                ctx_builder,
            );
            self.signaling_client.set_hook_callback(cb);
        }

        tracing::info!("📡 Connecting to signaling server (with credential)");
        // A typed auth verdict on the INITIAL handshake (401 credential
        // rejection or 403 realm denial) must reach the same membership-recovery
        // path as the connected case. Before the controller is installed the
        // explicit connect loop cannot report it, so when the controller is
        // enabled we capture the verdict here and hand it to the controller once
        // it is spawned (below) instead of failing startup on a credential the
        // recovery engine could refresh. With the controller disabled, behavior
        // is unchanged: any connect failure aborts start.
        let mut deferred_initial_verdict: Option<crate::transport::AuthVerdict> = None;
        match self.signaling_client.connect().await {
            Ok(()) => tracing::info!("✅ Connected to signaling server"),
            Err(e) => match e.auth_verdict() {
                Some(verdict) if self.membership_controller_enabled => {
                    tracing::warn!(
                        ?verdict,
                        error = %e,
                        "📡 Initial signaling handshake returned a typed auth verdict; deferring to membership recovery instead of failing startup"
                    );
                    deferred_initial_verdict = Some(verdict);
                }
                _ => {
                    return Err(ActrError::Unavailable(format!(
                        "Signaling connect failed: {e}"
                    )));
                }
            },
        }

        // Collect background task handles so they can be managed by ActrRefShared later.
        let mut task_handles = Vec::new();
        let relay_client_to_start: Arc<crate::wire::relay::RelayClient>;

        // Node-level hook callback, built inside the registration
        // setup block below and published back out into this wider
        // scope so the mailbox backpressure watchdog can subscribe.
        let node_hook_callback: Option<crate::wire::webrtc::HookCallback>;
        let session_state: SessionState;
        let credential_manager: CredentialManager;
        // Set when the membership controller is enabled; handed to the heartbeat
        // task so credential events report to the owner instead of triggering
        // the legacy fire-and-forget renewal directly.
        let mut membership_handle: Option<crate::lifecycle::membership::MembershipHandle> = None;
        // Construct and wire the controller before `on_start`, but do not spawn
        // it until the fallible hook succeeds. This prevents a failed start from
        // detaching credential-management tasks.
        let mut pending_membership_controller: Option<
            crate::lifecycle::membership::MembershipController,
        > = None;

        {
            let actor_id = register_ok.actr_id;
            let credential = register_ok.credential;
            let credential_expires_at = register_ok.credential_expires_at;

            tracing::info!("🆔 Assigned ActrId: {}", actor_id);
            tracing::info!("🔐 Received credential (key_id: {})", credential.key_id);
            tracing::info!(
                "💓 Signaling heartbeat interval: {} seconds",
                register_ok.signaling_heartbeat_interval_secs
            );

            // TurnCredential is a required field; should always be present under normal registration.
            tracing::debug!("TurnCredential received, TURN authentication ready");

            if let Some(expires_at) = &register_ok.credential_expires_at {
                tracing::debug!("⏰ Credential expires at: {}s", expires_at.seconds);
            }

            // Store ActrId and credential state.
            //
            // Reuse the SAME `CredentialState` instance installed on the
            // signaling client above (`pre_connect_credential_state`) rather
            // than constructing a second one. `CredentialState::clone()`
            // shares the inner `Arc<RwLock<...>>`, so this single shared
            // authority is what every reader — signaling reconnect URL
            // construction, soft renewal, hard rebind, heartbeat — observes.
            //
            // This is the fix for #423: a second `CredentialState::new` here
            // would create an independent `Arc`, leaving soft-renewal updates
            // (which write to `HardRebindHandles.credential_state`) invisible to
            // the signaling client. After a soft renewal, a signaling
            // disconnect would then reconnect with the stale initial
            // credential and loop on 401. Sharing one instance makes a
            // successful soft renewal visible to the very next handshake.
            // Do NOT reintroduce a second `CredentialState::new` on this path.
            self.actor_id = Some(actor_id.clone());
            let credential_state = pre_connect_credential_state.clone();
            self.credential_state = Some(credential_state.clone());
            session_state = SessionState::new(SessionSnapshot {
                actor_id: actor_id.clone(),
                credential,
                credential_expires_at: credential_expires_at.unwrap_or_default(),
                turn_credential: register_ok.turn_credential.clone(),
                renewal_token: register_ok.renewal_token.clone().unwrap_or_default(),
                renewal_token_expires_at: register_ok.renewal_token_expires_at.unwrap_or_default(),
                generation: 1,
            });
            self.session_state = Some(session_state.clone());
            // Wire the supervisor fact sink so a hard-rebind generation commit
            // reports `SessionActivated` to the connection supervisor.
            if let Some(sink) = self.supervisor_fact_sink.clone() {
                session_state.set_fact_sink(sink).await;
            }
            let registration_context = self
                .preregistered_registration_context
                .take()
                .unwrap_or_else(|| RegistrationContext::Linked {
                    request: register_request.clone(),
                });
            credential_manager = CredentialManager::new(
                session_state.clone(),
                registration_context,
                self.config.issuer_endpoint.clone(),
                client_key.clone(),
            );

            // Build the node-level lifecycle hook callback once: it is
            // reused for the initial `on_credential_renewed`, handed to
            // the heartbeat task for subsequent credential events, and
            // handed to the mailbox backpressure watchdog for
            // `on_mailbox_backpressure` on rising-edge crossings.
            //
            // The signaling layer already has its own callback installed
            // above — this second callback only carries credential and
            // mailbox-backpressure events, so no overlap with the
            // signaling-event plumbing.
            node_hook_callback =
                {
                    let actor_id_for_hook = actor_id.clone();
                    let credential_state_for_hook = credential_state.clone();
                    // Snapshot at this point — outproc_gate is still None
                    // here; credential / mailbox hook contexts inherit that
                    // and therefore cannot issue Dest::Peer(_) calls (same
                    // behavior as before B13 refactor).
                    let ctx_builder_snapshot = self.bootstrap_ctx_builder();
                    let ctx_builder: crate::lifecycle::hooks::HookContextBuilder =
                        Arc::new(move || {
                            let snapshot = ctx_builder_snapshot.clone();
                            let actor_id = actor_id_for_hook.clone();
                            let credential_state = credential_state_for_hook.clone();
                            Box::pin(async move {
                                Some(snapshot.build_bootstrap(
                                    &actor_id,
                                    &credential_state.credential().await,
                                ))
                            })
                        });
                    Some(crate::lifecycle::hooks::build_hook_callback(
                        self.hook_observer.clone(),
                        ctx_builder,
                    ))
                };
            credential_manager
                .install_hook_callback(node_hook_callback.clone())
                .await;

            // Fire `on_credential_renewed` at initial registration: the
            // credential is considered "renewed" from "nothing" to the
            // value just issued by Issuer. Subsequent renewals fire the
            // same hook from `lifecycle::heartbeat`.
            if let Some(expires_at) = &register_ok.credential_expires_at {
                if let Some(new_expiry) =
                    crate::lifecycle::credential_manager::credential_expiry_to_system_time(
                        expires_at,
                    )
                {
                    if let Some(cb) = node_hook_callback.as_ref() {
                        cb(crate::wire::webrtc::HookEvent::CredentialRenewed { new_expiry }).await;
                    } else {
                        tracing::info!(new_expiry = ?new_expiry, "credential renewed");
                    }
                } else {
                    tracing::warn!(
                        expiry_seconds = expires_at.seconds,
                        "Skipping credential-renewed hook because expiry is outside the protobuf or platform range"
                    );
                }
            }

            // Note: actor_id and credential_state were already set on signaling_client
            // before connect (step 3 above), so reconnect URLs already carry correct auth.

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.3. Inproc transports were filled in during `build()`; nothing
            //      to stage here now that ContextFactory has been removed.
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            tracing::info!("✅ Inproc infrastructure already ready (created in ActrNode::build())");

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.5. Create WebRTC infrastructure
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            tracing::info!("🌐 Initializing WebRTC infrastructure");

            // Fail fast on WebRTC options the RTC integration cannot honor
            // (file-based config is already rejected at parse time; this
            // covers programmatically built configs). A fixed UDP port is a
            // deployment precondition, so silently ignoring it would present
            // as "node healthy but unreachable".
            self.config
                .webrtc
                .advanced
                .validate()
                .map_err(|e| ActrError::InvalidArgument(e.to_string()))?;

            #[allow(unused_variables)] // registry is consumed by the RTC inbound bridge
            let media_frame_registry = self.media_frame_registry.clone();

            // Pre-allocate the pending-requests map so it can be shared between
            // DefaultWireBuilder (for outbound WS response reader tasks), the
            // RTC inbound dispatcher, and PeerGate (request/response
            // matching).
            let pending_requests: crate::outbound::PendingRequestsMap =
                std::sync::Arc::new(crate::transport::correlation::RpcCorrelation::new());
            let handover = Arc::new(crate::transport::handover::HandoverBook::default());

            // Construct the RTC manager, controllers, and recovery stack as
            // one unit sharing the node's identity and inbound dispatch.
            let (rtc_manager, rtc_local_id, rtc_mailbox_slot): (
                Arc<crate::wire::webrtc::rtc::manager::RtcConnectionManager>,
                crate::wire::webrtc::rtc::signaling::SharedLocalId,
                crate::wire::webrtc::rtc::inbound_bridge::MailboxSlot,
            ) = {
                use crate::wire::webrtc::rtc;

                // W6: fail-fast on unsupported RTC config capabilities
                // (capability matrix). Don't silently degrade.
                validate_rtc_config(&self.config.webrtc)?;

                // Create the RTC factory with production config.
                let buffer_config = rtc::config::RtcBufferConfig::default();
                buffer_config.validate().map_err(|error| {
                    ActrError::Internal(format!("invalid RTC resource configuration: {error}"))
                })?;
                let rtc_local_id = rtc::signaling::SharedLocalId::new(actor_id.clone());
                let runtime_config = rtc::runtime::RtcRuntimeConfig::desktop_default()
                    .with_buffer_config(buffer_config.clone());
                let ice_servers: Vec<webrtc::peer_connection::RTCIceServer> = self
                    .config
                    .webrtc
                    .ice_servers
                    .iter()
                    .map(|s| webrtc::peer_connection::RTCIceServer {
                        urls: s.urls.clone(),
                        username: s.username.clone().unwrap_or_default(),
                        credential: s.credential.clone().unwrap_or_default(),
                    })
                    .collect();
                let ice_transport_policy = match self.config.webrtc.ice_transport_policy {
                    actr_config::IceTransportPolicy::All => {
                        webrtc::peer_connection::RTCIceTransportPolicy::All
                    }
                    actr_config::IceTransportPolicy::Relay => {
                        webrtc::peer_connection::RTCIceTransportPolicy::Relay
                    }
                };
                let rtc_configuration = webrtc::peer_connection::RTCConfigurationBuilder::default()
                    .with_ice_servers(ice_servers)
                    .with_ice_transport_policy(ice_transport_policy)
                    .with_bundle_policy(webrtc::peer_connection::RTCBundlePolicy::Balanced)
                    .with_rtcp_mux_policy(webrtc::peer_connection::RTCRtcpMuxPolicy::Require)
                    .with_ice_candidate_pool_size(0)
                    .build();
                let setting_engine = webrtc::peer_connection::SettingEngineBuilder::new()
                    .with_host_acceptance_min_wait(Some(std::time::Duration::from_millis(
                        self.config.webrtc.advanced.ice_host_acceptance_min_wait,
                    )))
                    .with_srflx_acceptance_min_wait(Some(std::time::Duration::from_millis(
                        self.config.webrtc.advanced.ice_srflx_acceptance_min_wait,
                    )))
                    .with_prflx_acceptance_min_wait(Some(std::time::Duration::from_millis(
                        self.config.webrtc.advanced.ice_prflx_acceptance_min_wait,
                    )))
                    .with_relay_acceptance_min_wait(Some(std::time::Duration::from_millis(
                        self.config.webrtc.advanced.ice_relay_acceptance_min_wait,
                    )))
                    .with_discard_local_candidates_during_ice_restart(true)
                    .with_sctp_max_receive_buffer_size(runtime_config.sctp_receive_buffer_size())
                    .build();
                // Inventory marker: the four acceptance waits above are
                // protocol-selection timers executed inside RTC's ICE agent.
                let _ = crate::timer::ids::EXTERNAL_ICE_CANDIDATE_SELECTION;
                let factory =
                    rtc::factory::PeerConnectionFactory::new_with_configuration_and_wildcard_bind(
                        runtime_config,
                        rtc_configuration,
                    )
                    .with_turn_credential_provider(Arc::new(credential_state.clone()))
                    .with_setting_engine(setting_engine);
                // Test-only seam: every generation built by this real Node
                // consults the same directed RTC registry as the harness.
                #[cfg(feature = "test-utils")]
                let factory = match self.test_faults.as_ref() {
                    Some(faults) => faults.wrap_factory(factory, rtc_local_id.clone()),
                    None => factory,
                };
                let factory = Arc::new(factory);

                // Create the RTC signaling channel + manager + router.
                // CF-1: the local identity lives in a shared, rebind-updatable
                // cell so hard rebind can adopt the fresh ActrId (0.14 parity:
                // coordinator.set_local_id). Stage 6: the node media registry
                // is attached so session track pumps dispatch received RTP to
                // the same business surface as the 0.14 stack.
                let (signaling_tx, signaling_rx) = tokio::sync::mpsc::channel(256);
                // W3b production wiring: route inbound RTC data-plane
                // payloads through the shared InboundDispatcher (RPC → mailbox
                // / pending_requests, DataChunk → DataChunkRegistry). Without
                // this the manager's default `InboundRouter::discard()` drops
                // every inbound payload. The mailbox is late-bound (filled
                // when the receive loops start).
                let inbound_mailbox_slot: rtc::inbound_bridge::MailboxSlot =
                    std::sync::Arc::new(tokio::sync::RwLock::new(None));
                let inbound_dispatcher =
                    Arc::new(crate::inbound::dispatcher::InboundDispatcher::new(
                        pending_requests.clone(),
                        self.data_chunk_registry.clone(),
                        self.peer_types.clone(),
                    ));
                let (inbound_handler, inbound_dispatch_tasks) =
                    rtc::inbound_bridge::dispatcher_bridge(
                        {
                            inbound_dispatcher.set_handover(handover.clone());
                            inbound_dispatcher
                        },
                        inbound_mailbox_slot.clone(),
                        self.shutdown_token.child_token(),
                    );
                task_handles.push(inbound_dispatch_tasks);
                let rtc_manager = Arc::new(
                    rtc::manager::RtcConnectionManager::with_inbound_config(
                        factory.clone(),
                        signaling_tx,
                        Arc::new(rtc::inbound::InboundRouter::new_with_config(
                            inbound_handler,
                            buffer_config.clone(),
                        )),
                        buffer_config,
                    )
                    .with_media_registry(media_frame_registry.clone()),
                );
                // Issuer rotates the HMAC TURN credential together with the actor
                // credential. Apply the new value to live RTC sessions via
                // set_configuration(); every future Factory build also reads
                // the same shared provider, so idle/new generations cannot
                // retain the bootstrap credential.
                {
                    let mut turn_revision_rx = credential_state.subscribe_turn_revision();
                    let factory_for_turn_refresh = factory.clone();
                    let manager_for_turn_refresh = rtc_manager.clone();
                    let turn_refresh_shutdown = self.shutdown_token.child_token();
                    let turn_refresh_handle = tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = turn_refresh_shutdown.cancelled() => break,
                                changed = turn_revision_rx.changed() => {
                                    if changed.is_err() {
                                        break;
                                    }
                                    let revision = *turn_revision_rx.borrow_and_update();
                                    match factory_for_turn_refresh.current_configuration().await {
                                        Ok(configuration) => {
                                            let refreshed_peers = manager_for_turn_refresh
                                                .refresh_all_configurations(configuration)
                                                .await;
                                            tracing::info!(
                                                revision,
                                                refreshed_peers,
                                                "RTC TURN credential configuration refreshed"
                                            );
                                        }
                                        Err(error) => tracing::error!(
                                            revision,
                                            error = %error,
                                            "RTC TURN credential refresh could not materialize RTC configuration"
                                        ),
                                    }
                                }
                            }
                        }
                    });
                    task_handles.push(turn_refresh_handle);
                }
                let rtc_transport = rtc::signaling::ws_transport::RtcSignalingTransport::new(
                    self.signaling_client.clone(),
                    rtc_local_id.clone(),
                    credential_state.clone(),
                    self.peer_types.clone(),
                );
                let rtc_router = rtc::signaling::router::SignalingRouter::new(
                    rtc_manager.clone(),
                    rtc_local_id.clone(),
                );
                // Teardown-time cleanup of the router's per-peer dispatch
                // locks needs the manager to see the same map instance.
                rtc_manager.set_signaling_dispatch_chains(rtc_router.dispatch_chains());
                let router_shutdown = self.shutdown_token.child_token();
                #[cfg(feature = "test-utils")]
                let router_test_faults = self.test_faults.clone();
                #[cfg(feature = "test-utils")]
                let router_local_id = rtc_local_id.clone();
                let router_handle = tokio::spawn(async move {
                    let run_router = async move {
                        #[cfg(feature = "test-utils")]
                        if let Some(faults) = router_test_faults {
                            let transport = faults.wrap_signaling(rtc_transport, router_local_id);
                            rtc_router.run(transport, signaling_rx).await;
                            return;
                        }
                        rtc_router.run(rtc_transport, signaling_rx).await;
                    };
                    tokio::select! {
                        _ = router_shutdown.cancelled() => {}
                        _ = run_router => {}
                    }
                });
                task_handles.push(router_handle);
                task_handles.push(tokio::spawn(self.peer_types.clone().follow_webrtc_peers(
                    rtc_manager.clone(),
                    self.shutdown_token.child_token(),
                )));

                // Pull forward the snapshot→hook projection (B3): subscribe
                // to each peer's snapshot and fire the 3 hooks on transitions.
                // Without this the cutover silently drops business-visible hooks.
                {
                    let hook_observer = self.hook_observer.clone();
                    let actor_id_for_hook = actor_id.clone();
                    let credential_state_for_hook = credential_state.clone();
                    let ctx_builder_snapshot = self.bootstrap_ctx_builder();
                    let ctx_builder: crate::lifecycle::hooks::HookContextBuilder =
                        Arc::new(move || {
                            let snapshot = ctx_builder_snapshot.clone();
                            let actor_id = actor_id_for_hook.clone();
                            let credential_state = credential_state_for_hook.clone();
                            Box::pin(async move {
                                Some(snapshot.build_bootstrap(
                                    &actor_id,
                                    &credential_state.credential().await,
                                ))
                            })
                        });
                    let manager_for_hooks = rtc_manager.clone();
                    let hook_shutdown = self.shutdown_token.child_token();
                    let hook_projection_handle = tokio::spawn(async move {
                        rtc::notification::run_hook_projection(
                            manager_for_hooks,
                            hook_observer,
                            ctx_builder,
                            hook_shutdown,
                        )
                        .await;
                    });
                    task_handles.push(hook_projection_handle);
                }

                (rtc_manager, rtc_local_id, inbound_mailbox_slot)
            };

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.6. Create PeerTransport + PeerGate (new architecture)
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            tracing::info!("🏗️  Creating PeerTransport with WebRTC support");

            // Create DefaultWireBuilder with the configured wire transports
            use crate::transport::{DefaultWireBuilder, DefaultWireBuilderConfig};

            let relay_gate = Arc::new(crate::wire::websocket::WebSocketGate::new_relay(
                pending_requests.clone(),
                self.data_chunk_registry.clone(),
                self.peer_types.clone(),
            ));
            let relay_client = crate::wire::relay::RelayClient::new(
                self.config.signaling_url.to_string(),
                self.config.issuer_endpoint.clone(),
                client_key.clone(),
                session_state.clone(),
                relay_gate.clone(),
                self.shutdown_token.clone(),
            );
            relay_gate.set_handover(handover.clone());
            let wire_builder = Arc::new(
                DefaultWireBuilder::new(DefaultWireBuilderConfig::default())
                    .with_rtc_manager(rtc_manager.clone())
                    .with_relay(relay_client.clone()),
            );
            self.websocket_gate = Some(relay_gate.clone());

            // Create PeerTransport
            use crate::transport::PeerTransport;
            let transport_manager = Arc::new(
                PeerTransport::new(actor_id.clone(), wire_builder).with_handover(handover.clone()),
            );
            handover.bind(&transport_manager);
            relay_client.set_transport(&transport_manager);
            self.peer_transport = Some(transport_manager.clone());

            // Create PeerGate with the pre-allocated pending_requests map.
            // The RTC manager is the only send authority (W5); nodes
            // without a network-event handle get the always-Normal projection
            // default inside the constructor.
            use crate::outbound::PeerGate;
            let outproc_gate = Arc::new(PeerGate::with_rtc_manager(
                transport_manager.clone(),
                rtc_manager.clone(),
                pending_requests.clone(),
                self.send_projection_rx.take(),
            ));
            let outproc_gate_enum = Gate::Peer(outproc_gate.clone());
            tracing::info!("PeerTransport + PeerGate initialized");

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.7. Wire the outproc gate into Inner so subsequent
            //      `make_runtime_context` / `bootstrap_ctx_builder` calls
            //      observe it. All per-request contexts created by
            //      `handle_incoming` go through this field live.
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            tracing::info!("🔧 Wiring outproc_gate into node");
            self.outproc_gate = Some(outproc_gate_enum);
            tracing::info!("✅ Node runtime gates fully initialized (inproc + outproc)");

            // Install the outbound delivery-uncertainty hook: a synchronous
            // `DeliveryUncertain` returned by the send path must also reach
            // workload hook observers. Snapshot taken after the gate is live
            // so hook contexts can issue `Dest::Peer(_)` calls.
            //
            // Unconditional: WebSocket lanes carry peer traffic in every
            // supported configuration
            // and can classify a post-submission failure as
            // `DeliveryProgress::Unknown`, so the hook must be installed in
            // all of them.
            {
                let actor_id_for_hook = actor_id.clone();
                let credential_state_for_hook = credential_state.clone();
                let ctx_builder_snapshot = self.bootstrap_ctx_builder();
                let ctx_builder: crate::lifecycle::hooks::HookContextBuilder =
                    Arc::new(move || {
                        let snapshot = ctx_builder_snapshot.clone();
                        let actor_id = actor_id_for_hook.clone();
                        let credential_state = credential_state_for_hook.clone();
                        Box::pin(async move {
                            Some(
                                snapshot.build_bootstrap(
                                    &actor_id,
                                    &credential_state.credential().await,
                                ),
                            )
                        })
                    });
                let cb = crate::lifecycle::hooks::build_hook_callback(
                    self.hook_observer.clone(),
                    ctx_builder,
                );
                outproc_gate.set_hook_callback(cb);
            }

            // Save references. The RTC manager backs the node-level status
            // APIs (S6-4).
            {
                self.rtc_manager = Some(rtc_manager.clone());
                self.rtc_inbound_mailbox_slot = Some(rtc_mailbox_slot.clone());
            }
            credential_manager
                .install_hard_rebind_handles(HardRebindHandles {
                    signaling_client: self.signaling_client.clone(),
                    credential_state: credential_state.clone(),
                    peer_transport: Some(transport_manager.clone()),
                    rtc_manager: Some(rtc_manager.clone()),
                    rtc_local_id: Some(rtc_local_id.clone()),
                })
                .await;
            tracing::info!("✅ WebRTC infrastructure initialized");

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.7.5. Membership controller (self-healing credential lifecycle)
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // When enabled, the controller becomes the SOLE owner of the
            // credential: it seeds the `watch` with the just-registered gen-1
            // credential, installs the two typed channels on the signaling
            // client, and stages its on-demand coordinator loop.
            // The signaling client stops reading the stored credential and
            // instead builds handshake URLs from the watch and reports auth
            // verdicts (including handshake 401s) over the mpsc — closing the
            // gap where a handshake 401 never reached the recovery engine.
            if self.membership_controller_enabled {
                use crate::lifecycle::membership::MembershipController;

                let seed = credential_manager.published_credential().await;
                tracing::info!(
                    revision = seed.revision,
                    "🪪 Membership controller enabled; seeding credential watch"
                );

                // Publish happens-before the identity-bearing supervisor fact.
                // The credential owner never drives or wakes a client-local
                // reconnect loop.
                let fact_sink = self
                    .supervisor_fact_sink
                    .clone()
                    .expect("membership requires the always-on recovery supervisor");
                let wake_socket: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |revision| {
                    fact_sink.signaling_credential_published(revision);
                });

                let (controller, handle) = MembershipController::new(
                    credential_manager.clone(),
                    seed,
                    wake_socket,
                    self.shutdown_token.clone(),
                );

                // Install the two typed channels on the signaling client.
                self.signaling_client
                    .install_membership(handle.clone())
                    .await;

                pending_membership_controller = Some(controller);
                membership_handle = Some(handle);
                tracing::info!("✅ Membership controller staged for post-on_start spawn");
            }

            // Fire `on_start` once the runtime context can see the initialized
            // gates, before starting request-accepting/background loops. Its
            // Err/panic aborts Node::start.
            {
                let startup_ctx = self
                    .bootstrap_ctx_builder()
                    .build_bootstrap(&actor_id, &credential_state.credential().await);
                let invocation = lifecycle_invocation(&actor_id, "lifecycle:on_start");
                let call_executor =
                    lifecycle_host_abi(startup_ctx.clone(), self.workload_dispatch.clone());
                crate::lifecycle::hooks::call_lifecycle_hook(
                    "on_start",
                    self.workload_dispatch
                        .on_start(startup_ctx, invocation, &call_executor),
                )
                .await?;
            }

            if let Some(controller) = pending_membership_controller.take() {
                task_handles.push(crate::lifecycle::membership::spawn_membership_controller(
                    controller,
                ));
                tracing::info!("✅ Membership controller spawned (credential owner online)");
            }

            // If the initial handshake was answered with a typed auth verdict,
            // hand it to the freshly-spawned controller now. This routes the
            // startup-time verdict through the exact same on-demand recovery the
            // connected path uses: a `Rejected` verdict re-acquires and
            // republishes a credential (waking a reconnect), while `RealmDenied`
            // marks the realm terminally unavailable.
            if let Some(verdict) = deferred_initial_verdict.take()
                && let Some(handle) = membership_handle.as_ref()
            {
                let stale_revision = handle.current_revision();
                tracing::warn!(
                    ?verdict,
                    stale_revision,
                    "📡 Reporting deferred initial-handshake auth verdict to membership controller for recovery"
                );
                handle.report(verdict, stale_revision).await;
            }

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.7.6. Outbound WSS relay (no application-side listener).
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            {
                let actor_id_for_hook = actor_id.clone();
                let credential_state_for_hook = credential_state.clone();
                let ctx_builder_snapshot = self.bootstrap_ctx_builder();
                let ctx_builder: crate::lifecycle::hooks::HookContextBuilder =
                    Arc::new(move || {
                        let snapshot = ctx_builder_snapshot.clone();
                        let actor_id = actor_id_for_hook.clone();
                        let credential_state = credential_state_for_hook.clone();
                        Box::pin(async move {
                            Some(
                                snapshot.build_bootstrap(
                                    &actor_id,
                                    &credential_state.credential().await,
                                ),
                            )
                        })
                    });
                relay_gate.set_hook_callback(crate::lifecycle::hooks::build_hook_callback(
                    self.hook_observer.clone(),
                    ctx_builder,
                ));
                // Start only after the ingress mailbox has been published.
                // A later startup error must not strand the relay read loop
                // waiting for a mailbox that will never become available.
                relay_client_to_start = relay_client;
            }

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.7.5. Create shared state for credential management
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // Shared credential state initialized above; reused across tasks

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.8. Spawn heartbeat task (periodic Ping to signaling server)
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            {
                let shutdown = self.shutdown_token.clone();
                let client = self.signaling_client.clone();
                let actor_id_for_heartbeat = actor_id.clone();
                let credential_state_for_heartbeat = credential_state.clone();
                let mailbox_for_heartbeat = self.mailbox.clone();
                let register_request_for_heartbeat = register_request.clone();
                let credential_manager_for_heartbeat = credential_manager.clone();
                let membership_handle_for_heartbeat = membership_handle.clone();

                // Use interval from registration response, default to 30s
                let heartbeat_interval_secs = register_ok.signaling_heartbeat_interval_secs;
                let heartbeat_interval = if heartbeat_interval_secs > 0 {
                    Duration::from_secs(heartbeat_interval_secs as u64)
                } else {
                    Duration::from_secs(30)
                };
                let issuer_endpoint_for_heartbeat = self.config.issuer_endpoint.clone();
                let client_key_for_heartbeat = client_key.clone();
                let heartbeat_handle = tokio::spawn(crate::lifecycle::heartbeat::heartbeat_task(
                    shutdown,
                    client,
                    actor_id_for_heartbeat,
                    credential_state_for_heartbeat,
                    mailbox_for_heartbeat,
                    heartbeat_interval,
                    register_request_for_heartbeat,
                    issuer_endpoint_for_heartbeat,
                    client_key_for_heartbeat,
                    Some(credential_manager_for_heartbeat),
                    node_hook_callback.clone(),
                    membership_handle_for_heartbeat,
                ));
                task_handles.push(heartbeat_handle);
            }
            tracing::info!(
                "✅ Heartbeat task started (interval: {}s)",
                register_ok.signaling_heartbeat_interval_secs
            );

            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            // 1.8.5. Spawn network event processing loop
            // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
            {
                use crate::lifecycle::network_event::DefaultNetworkEventProcessor;

                // A closed external receiver is intentional for headless
                // deployments: signaling/session facts still drive the
                // always-on lifecycle supervisor.
                let event_rx = self.network_event_rx.take().unwrap_or_else(|| {
                    let (_event_tx, event_rx) = tokio::sync::mpsc::channel(1);
                    event_rx
                });

                let event_processor =
                    if let Some(config) = self.network_event_debounce_config.clone() {
                        DefaultNetworkEventProcessor::new_with_debounce_and_peer_transport(
                            self.signaling_client.clone(),
                            config,
                            self.peer_transport.clone(),
                        )
                    } else {
                        DefaultNetworkEventProcessor::new_with_peer_transport(
                            self.signaling_client.clone(),
                            self.peer_transport.clone(),
                        )
                    }
                    .with_session_state(session_state.clone());
                let event_processor = event_processor.with_rtc_manager(self.rtc_manager.clone());
                let event_processor = Arc::new(event_processor);

                let shutdown = self.shutdown_token.clone();
                // Accepted network/app facts are republished to RTC
                // controllers through the manager's recovery supervisor.
                let rtc_recovery_feed = self
                    .rtc_manager
                    .as_ref()
                    .map(|manager| manager.supervisor().clone())
                    .expect("RTC manager must exist after infrastructure initialization");
                let internal_channel = self
                    .network_event_internal
                    .take()
                    .expect("always-on recovery supervisor channel missing at start");
                let network_event_handle = tokio::spawn(async move {
                    crate::lifecycle::network_event::run_network_event_reconciler_with_channel_and_rtc_recovery(
                        event_rx,
                        internal_channel,
                        event_processor,
                        shutdown,
                        rtc_recovery_feed,
                    )
                    .await;
                });
                task_handles.push(network_event_handle);
                tracing::info!("network_event.node.loop_started");
            }

            {
                // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
                // 1.9. Spawn dedicated Unregister task (best-effort, with timeout)
                // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
                //
                // This task:
                // - Waits for shutdown_token to be cancelled (e.g., wait_for_ctrl_c_and_shutdown)
                // - Then sends UnregisterRequest via signaling client with a timeout
                //
                // NOTE: we push its JoinHandle into task_handles so it can be aborted
                // by ActrRefShared::Drop if needed.
                let shutdown = self.shutdown_token.clone();
                let client = self.signaling_client.clone();
                let actor_id_for_unreg = actor_id.clone();
                let credential_state_for_unreg = credential_state.clone();
                let rtc_manager_for_unreg = self.rtc_manager.clone();

                let unregister_handle = tokio::spawn(async move {
                    // Wait for shutdown signal
                    shutdown.cancelled().await;
                    tracing::info!(
                        "📡 Shutdown signal received, sending UnregisterRequest for Actor {}",
                        actor_id_for_unreg
                    );

                    // 1. Close all WebRTC peer connections first (if any).
                    // The RTC manager's close_all is bounded by its internal
                    // deadline; the 0.14 coordinator leg is gone.
                    if let Some(manager) = rtc_manager_for_unreg {
                        manager.close_all().await;
                        tracing::info!("✅ All WebRTC peers closed before UnregisterRequest");
                    }

                    // 2. Then send UnregisterRequest with a timeout (e.g. 5 seconds)
                    let result = crate::timer::timeout(
                        crate::timer::ids::NODE_UNREGISTER,
                        Duration::from_secs(5),
                        client.send_unregister_request(
                            actor_id_for_unreg.clone(),
                            credential_state_for_unreg.credential().await,
                            Some("Graceful shutdown".to_string()),
                        ),
                    )
                    .await;
                    tracing::info!("UnregisterRequest result: {:?}", result);
                    match result {
                        Ok(Ok(_)) => {
                            tracing::info!(
                                "✅ UnregisterRequest sent to signaling server for Actor {}",
                                actor_id_for_unreg
                            );
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(
                                "⚠️ Failed to send UnregisterRequest for Actor {}: {}",
                                actor_id_for_unreg,
                                e
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                "⚠️ UnregisterRequest timeout (5s) for Actor {}",
                                actor_id_for_unreg
                            );
                        }
                    }
                });

                task_handles.push(unregister_handle);
            }
        } // end registration setup block

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 2. Transport layer initialization (completed via WebRTC infrastructure)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        tracing::info!("✅ Transport layer initialized via WebRTC infrastructure");

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 3.1 Convert to Arc (before starting background loops)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // Clone actor_id before moving self into Arc
        let actor_id = self
            .actor_id
            .as_ref()
            .ok_or_else(|| ActrError::Internal("Actor ID not set".to_string()))?
            .clone();
        // Snapshot now that outproc_gate has been wired above; this builder
        // is shared between on_start / on_stop hooks and the ActrRefShared
        // handle returned to the caller.
        let mut bootstrap_ctx_builder = self.bootstrap_ctx_builder();
        bootstrap_ctx_builder.set_session_state(Some(session_state.clone()));
        bootstrap_ctx_builder.set_generation(session_state.generation().await);
        let credential_state = self
            .credential_state
            .clone()
            .expect("CredentialState must be initialized in start()");
        let session_state = credential_manager.session_state();
        let shutdown_token = self.shutdown_token.clone();
        // S6-4: capture the RTC manager before `self` moves into the Arc so
        // ActrRefShared can expose the status APIs.
        let rtc_manager = self.rtc_manager.clone();
        let node_ref = Arc::new(self);

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 3.2. Register workload-level stop hook.
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        {
            let on_stop_handle = spawn_workload_stop_task(
                node_ref.clone(),
                actor_id.clone(),
                credential_state.clone(),
                shutdown_token.clone(),
            );
            task_handles.push(on_stop_handle);
        }

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 3.5. Start WebRTC background loops
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        tracing::info!("🚀 Starting WebRTC background loops");

        // Start the RTC signaling router; the lifecycle supervisor owns the
        // signaling connection itself.
        // RTC backend (W3b): publish the mailbox to the inbound bridge so
        // RTC DataChannel payloads route into the node mailbox.
        if let Some(slot) = &node_ref.rtc_inbound_mailbox_slot {
            *slot.write().await = Some(node_ref.mailbox.clone());
        }

        // Publish the relay ingress mailbox before acknowledging peer channels.
        if let Some(ws_gate) = &node_ref.websocket_gate {
            ws_gate
                .start_receive_loop(node_ref.mailbox.clone())
                .await
                .map_err(|e| {
                    ActrError::Unavailable(format!("WebSocketGate receive loop start failed: {e}"))
                })?;
            task_handles.push(relay_client_to_start.start());
            tracing::info!("✅ WebSocketGate → Mailbox routing started");
        }
        tracing::info!("✅ WebRTC background loops started");

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 4.6. Start Inproc receive loop (Shell → Guest)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        if let Some(shell_to_workload) = &node_ref.shell_to_workload {
            tracing::info!("🔄 Starting Inproc receive loop (Shell → Guest)");
            // Start Guest receive loop (Shell → Guest REQUEST)
            if let Some(workload_to_shell) = &node_ref.workload_to_shell {
                let node = node_ref.clone();
                let request_rx_lane = shell_to_workload
                    .get_lane(PayloadType::RpcReliable, None)
                    .await
                    .map_err(|e| {
                        ActrError::Unavailable(format!("Failed to get guest receive lane: {e}"))
                    })?;
                let response_tx = workload_to_shell.clone();
                let shutdown = shutdown_token.clone();
                let outstanding_replies = node_ref.outstanding_replies.clone();
                let max_pending_replies = node_ref.max_pending_replies;
                let hook_cb = node_hook_callback.clone();
                // #390: shared with the sending side (`call_raw` attaches the
                // handoff marker through the same registry).
                let scope_registry = shell_to_workload.scope_registry().clone();

                let inproc_handle = tokio::spawn(async move {
                    use futures_util::StreamExt as _;
                    use futures_util::stream::FuturesUnordered;
                    use std::sync::atomic::{AtomicBool, Ordering};

                    // Concurrency gate: only when the dispatch scheduler is on do
                    // we overlap messages. No scheduler (a keyless actor, or the
                    // gate forced off) = one message at a time, bit-for-bit the
                    // pre-B2 loop.
                    let scheduler_on = node.dispatch_scheduler.is_some();
                    // Bound on continuations the loop keeps in flight; a full set
                    // stops us dequeuing, which back-pressures the lane. The
                    // scheduler's own queue (`M`) is the finer bound underneath.
                    const INPROC_INFLIGHT_CAP: usize = 256;
                    // RFC-0323 §7: rising-edge flag for the `max_pending_replies`
                    // backpressure signal (outstanding EXPLICIT replies).
                    let pending_saturated = AtomicBool::new(false);
                    let mut inflight: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send>>> =
                        FuturesUnordered::new();

                    loop {
                        // RFC-0323 §7 rising-edge backpressure signal. Fires
                        // the framework `on_mailbox_backpressure` hook on the
                        // rising edge (falling back to a log line when no hook
                        // is installed), mirroring the §4.9 mailbox-depth
                        // watchdog.
                        {
                            let pending = outstanding_replies.len();
                            if pending >= max_pending_replies {
                                if !pending_saturated.swap(true, Ordering::AcqRel) {
                                    if let Some(cb) = hook_cb.as_ref() {
                                        let cb = cb.clone();
                                        tokio::spawn(async move {
                                            cb(crate::wire::webrtc::HookEvent::MailboxBackpressure {
                                                queue_len: pending,
                                                threshold: max_pending_replies,
                                            })
                                            .await;
                                        });
                                    } else {
                                        tracing::warn!(
                                            outstanding_replies = pending,
                                            max_pending_replies,
                                            "explicit-reply backpressure: inproc dequeue paused",
                                        );
                                    }
                                }
                            } else if pending_saturated.swap(false, Ordering::AcqRel) {
                                tracing::info!(
                                    outstanding_replies = pending,
                                    max_pending_replies,
                                    "explicit-reply backpressure cleared",
                                );
                            }
                        }
                        tokio::select! {
                            biased;
                            _ = shutdown.cancelled() => {
                                tracing::info!("📭 Guest receive loop (Shell → Guest) received shutdown signal");
                                break;
                            }
                            // Drain finished reply continuations (gate-on only;
                            // inert when `inflight` is empty).
                            Some(()) = inflight.next(), if !inflight.is_empty() => {}
                            // Accept a new request only while under the in-flight
                            // cap (back-pressure). With the gate off `inflight` is
                            // always empty so the guard is always true. RFC-0323
                            // §7: also gate on outstanding EXPLICIT replies.
                            envelope_result = request_rx_lane.recv_envelope(),
                                if inflight.len() < INPROC_INFLIGHT_CAP
                                    && outstanding_replies.len() < max_pending_replies => {
                                match envelope_result {
                                    Ok(mut envelope) => {
                                        let request_id = envelope.request_id.clone();
                                        tracing::debug!("📨 Guest received REQUEST from Shell: request_id={}", request_id);

                                        // Route strictly on the explicit direction label:
                                        // Request runs the handler and sends a response;
                                        // Tell runs the handler for its side effects but
                                        // sends nothing — an unwanted reply becomes an
                                        // orphan response on the caller (#262). Anything
                                        // else (missing / Unspecified / Response / unknown)
                                        // is invalid on a dispatch lane: warn and drop.
                                        // timeout_ms is never consulted for tell-ness.
                                        let expects_response = match Inner::dispatch_expects_response(envelope.direction) {
                                            Some(v) => v,
                                            None => {
                                                tracing::warn!(
                                                    request_id = %request_id,
                                                    route_key = %envelope.route_key,
                                                    direction = ?envelope.direction,
                                                    "rpc.invalid_direction_dropped: non-dispatchable RpcEnvelope.direction on Shell → Guest lane; dropping"
                                                );
                                                continue;
                                            }
                                        };

                                        // #390: strip the in-process scope handoff marker
                                        // and claim the root scope before ACL, dedup, or
                                        // dispatch ever see the envelope. An abandoned
                                        // handoff (caller cancelled between send and
                                        // intake) or an invalid/duplicate marker is
                                        // dropped: cancelled work must never enter guest
                                        // code.
                                        let scope = match crate::request_scope::strip_and_claim(
                                            &mut envelope,
                                            &scope_registry,
                                        ) {
                                            crate::request_scope::ScopeClaim::Unmarked => None,
                                            crate::request_scope::ScopeClaim::Claimed(scope) => {
                                                Some(scope)
                                            }
                                            crate::request_scope::ScopeClaim::Abandoned => {
                                                tracing::debug!(
                                                    request_id = %request_id,
                                                    route_key = %envelope.route_key,
                                                    "rpc.scope_handoff_abandoned: caller cancelled before intake; dropping"
                                                );
                                                continue;
                                            }
                                            crate::request_scope::ScopeClaim::Invalid => {
                                                tracing::warn!(
                                                    request_id = %request_id,
                                                    route_key = %envelope.route_key,
                                                    "rpc.scope_handoff_invalid: malformed or duplicate scope marker; dropping"
                                                );
                                                continue;
                                            }
                                        };

                                        // Extract and set tracing context from envelope
                                        #[cfg(feature = "opentelemetry")]
                                        let span = {
                                            let actr_id_str = node.actor_id.as_ref().map(|id| id.to_string()).unwrap_or_default();
                                            let span = tracing::info_span!("ActrNode.lane_receive", actr_id = %actr_id_str, request_id = %request_id);
                                            set_parent_from_rpc_envelope(&span, &envelope);
                                            span
                                        };
                                        #[cfg(not(feature = "opentelemetry"))]
                                        let span = tracing::Span::none();

                                        let route_key = envelope.route_key.clone();
                                        let traceparent = envelope.traceparent.clone();
                                        let tracestate = envelope.tracestate.clone();

                                        if scheduler_on {
                                            // Synchronous admission preserves arrival
                                            // order + back-pressure; the continuation
                                            // (dispatch result → reply) overlaps with
                                            // later messages. Shell calls have no caller_id.
                                            let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Inproc {
                                                response_tx: response_tx.clone(),
                                                span: span.clone(),
                                            };
                                            let cont = node.admit_incoming(envelope, None, Some(reply_sink), scope).await;
                                            let response_tx = response_tx.clone();
                                            inflight.push(Box::pin(async move {
                                                match cont.await {
                                                    IncomingOutcome::Replied(result) => {
                                                        inproc_send_reply(
                                                            response_tx, route_key, request_id,
                                                            traceparent, tracestate, expects_response,
                                                            result, span,
                                                        ).await;
                                                    }
                                                    IncomingOutcome::Deferred => {}
                                                }
                                            }));
                                        } else {
                                            // Gate off: run to completion inline.
                                            let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Inproc {
                                                response_tx: response_tx.clone(),
                                                span: span.clone(),
                                            };
                                            let handle_incoming_fut = node.handle_incoming(envelope, None, Some(reply_sink), scope);
                                            #[cfg(feature = "opentelemetry")]
                                            let handle_incoming_fut = handle_incoming_fut.instrument(span.clone());
                                            match handle_incoming_fut.await {
                                                IncomingOutcome::Replied(result) => {
                                                    inproc_send_reply(
                                                        response_tx.clone(), route_key, request_id,
                                                        traceparent, tracestate, expects_response,
                                                        result, span,
                                                    ).await;
                                                }
                                                IncomingOutcome::Deferred => {}
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            severity = 8,
                                            error_category = "transport_error",
                                            "❌ Failed to receive from Shell → Guest lane: {:?}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // Drain any remaining reply continuations before exiting so no
                    // in-flight reply is orphaned.
                    while inflight.next().await.is_some() {}
                    tracing::info!("✅ Guest receive loop (Shell → Guest) terminated gracefully");
                });
                task_handles.push(inproc_handle);
            }
        }
        tracing::info!("✅ Guest receive loop (Shell → Guest REQUEST) started");

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 4.7. Start Shell receive loop (Guest → Shell RESPONSE)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        tracing::info!("🔄 Starting Shell receive loop (Guest → Shell RESPONSE)");
        if let Some(workload_to_shell) = &node_ref.workload_to_shell {
            // Start Shell receive loop (Guest → Shell RESPONSE)
            if let Some(shell_to_workload) = &node_ref.shell_to_workload {
                let response_rx_lane = workload_to_shell
                    .get_lane(PayloadType::RpcReliable, None)
                    .await
                    .map_err(|e| {
                        ActrError::Unavailable(format!("Failed to get shell receive lane: {e}"))
                    })?;
                let request_mgr = shell_to_workload.clone();
                let shutdown = shutdown_token.clone();

                let shell_receive_handle = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = shutdown.cancelled() => {
                                tracing::info!("📭 Shell receive loop (Guest → Shell) received shutdown signal");
                                break;
                            }
                            envelope_result = response_rx_lane.recv_envelope() => {
                                match envelope_result {
                                    Ok(envelope) => {
                                        tracing::debug!(
                                            "📨 Shell received RESPONSE from Guest: request_id={}",
                                            envelope.request_id
                                        );

                                        // Check if response is success or error
                                        match (envelope.payload, envelope.error) {
                                            (Some(payload), None) => {
                                                // Success response
                                                if let Err(e) = request_mgr
                                                    .complete_response(&envelope.request_id, payload)
                                                    .await
                                                {
                                                    tracing::warn!(
                                                        severity = 4,
                                                        error_category = "orphan_response",
                                                        request_id = %envelope.request_id,
                                                        "⚠️  No pending request found for response: {:?}",
                                                        e
                                                    );
                                                }
                                            }
                                            (None, Some(error)) => {
                                                // Error response — reconstruct the precise ActrError variant
                                                // from the wire code so binding-visible classification
                                                // (UnknownRoute / PermissionDenied / TimedOut / …) is preserved
                                                // instead of collapsing every error into Unavailable.
                                                let actr_err = wire_code_to_actr_error(error.code, error.message);
                                                if let Err(e) = request_mgr
                                                    .complete_error(&envelope.request_id, actr_err)
                                                    .await
                                                {
                                                    tracing::warn!(
                                                        severity = 4,
                                                        error_category = "orphan_response",
                                                        request_id = %envelope.request_id,
                                                        "⚠️  No pending request found for error response: {:?}",
                                                        e
                                                    );
                                                }
                                            }
                                            _ => {
                                                tracing::error!(
                                                    severity = 7,
                                                    error_category = "protocol_error",
                                                    request_id = %envelope.request_id,
                                                    "❌ Invalid RpcEnvelope: both payload and error are present or both absent"
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            severity = 8,
                                            error_category = "transport_error",
                                            "❌ Failed to receive from Guest → Shell lane: {:?}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    tracing::info!("✅ Shell receive loop (Guest → Shell) terminated gracefully");
                });
                task_handles.push(shell_receive_handle);
            }
        }
        tracing::info!("✅ Shell receive loop (Guest → Shell RESPONSE) started");

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 4.9. Mailbox backpressure watchdog
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        //
        // Emits the framework `on_mailbox_backpressure` hook once per
        // rising-edge crossing of the configured threshold.
        //
        // Preferred path: a push-based notification from the mailbox
        // backend via [`Mailbox::set_depth_observer`], which runs
        // synchronously on every enqueue and has zero worst-case delay.
        //
        // Fallback path: mailbox backends without depth support (or
        // which can't cheaply compute depth on every enqueue) keep
        // using a 1 Hz poll of [`Mailbox::status`].
        let mailbox_enqueue_wake = Arc::new(tokio::sync::Notify::new());
        let mailbox_has_push_observer;
        let backpressure_threshold = node_ref.mailbox_backpressure_threshold;
        {
            use std::sync::atomic::{AtomicBool, Ordering};
            let mailbox = node_ref.mailbox.clone();
            let shutdown = shutdown_token.clone();
            let hook_cb = node_hook_callback.clone();
            let triggered = Arc::new(AtomicBool::new(false));

            // Shared rising-edge state + hook-firing closure used by
            // both the push and polling code paths.
            let fire_if_rising = {
                let triggered = triggered.clone();
                let hook_cb = hook_cb.clone();
                Arc::new(move |queue_len: usize| {
                    if queue_len >= backpressure_threshold {
                        if !triggered.swap(true, Ordering::AcqRel) {
                            if let Some(cb) = hook_cb.as_ref() {
                                let cb = cb.clone();
                                tokio::spawn(async move {
                                    cb(crate::wire::webrtc::HookEvent::MailboxBackpressure {
                                        queue_len,
                                        threshold: backpressure_threshold,
                                    })
                                    .await;
                                });
                            } else {
                                tracing::warn!(
                                    queue_len,
                                    threshold = backpressure_threshold,
                                    "mailbox backpressure",
                                );
                            }
                        }
                    } else if triggered.swap(false, Ordering::AcqRel) {
                        tracing::info!(
                            queue_len,
                            threshold = backpressure_threshold,
                            "mailbox backpressure cleared",
                        );
                    }
                })
            };

            // Try the push path first. The observer installs only if
            // the backend supports it; otherwise `installed` is `false`
            // and we fall through to polling.
            let installed = {
                let observer: Arc<dyn actr_runtime_mailbox::MailboxDepthObserver> =
                    Arc::new(EnqueueObserver {
                        fire: fire_if_rising.clone(),
                        wake: mailbox_enqueue_wake.clone(),
                    });
                mailbox.set_depth_observer(observer)
            };
            mailbox_has_push_observer = installed;

            if installed {
                tracing::debug!("mailbox backpressure watchdog: push notifications enabled");
            } else {
                tracing::debug!(
                    "mailbox backpressure watchdog: backend does not support push, falling back to 1 Hz polling"
                );
                let mailbox_for_poll = mailbox.clone();
                let shutdown_for_poll = shutdown.clone();
                let fire_for_poll = fire_if_rising.clone();
                let watchdog_handle = tokio::spawn(async move {
                    let mut ticker = crate::timer::interval(
                        crate::timer::ids::MAILBOX_DEPTH_FALLBACK,
                        Duration::from_secs(1),
                    );
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        tokio::select! {
                            _ = shutdown_for_poll.cancelled() => {
                                tracing::debug!(
                                    "mailbox backpressure watchdog shutting down"
                                );
                                break;
                            }
                            _ = ticker.tick() => {
                                let status = match mailbox_for_poll.status().await {
                                    Ok(s) => s,
                                    Err(e) => {
                                        tracing::debug!(?e, "mailbox status poll failed");
                                        continue;
                                    }
                                };
                                fire_for_poll(status.queued_messages as usize);
                            }
                        }
                    }
                });
                task_handles.push(watchdog_handle);
            }
        }

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 4.10. In-flight TTL sweep (RFC-0323 §5/§7)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        //
        // A periodic background sweep that does NOT rely on `check_or_mark`
        // traffic being driven: an EXPLICIT `Reply` parked in a waiter map
        // forever would otherwise pin its InFlight dedup marker (every retry
        // for that `request_id` hangs) and the `outstanding_replies` gauge
        // forever (backpressure never lifts). Every `INFLIGHT_TTL` this evicts
        // aged InFlight entries (notifying duplicate waiters with
        // `ActrError::TimedOut`) and aged outstanding-reply registry entries
        // (decrementing the gauge).
        {
            let dedup_state = node_ref.dedup_state.clone();
            let outstanding_replies = node_ref.outstanding_replies.clone();
            let shutdown = shutdown_token.clone();
            let sweep_handle = tokio::spawn(async move {
                use crate::lifecycle::dedup::INFLIGHT_TTL;
                let mut ticker = crate::timer::interval(
                    crate::timer::ids::INFLIGHT_TTL_SWEEP,
                    Duration::from_secs(5),
                );
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            tracing::debug!("in-flight TTL sweep shutting down");
                            break;
                        }
                        _ = ticker.tick() => {
                            // Couple the two sweeps on the same tick so a
                            // leaked EXPLICIT reply is freed on both sides
                            // together. Only EXPLICIT-originated InFlight
                            // entries (exactly the ids in the
                            // outstanding_replies registry) are swept from
                            // dedup — RETURN InFlight entries are retained
                            // until `complete` (RFC-0323 §5: retain RETURN).
                            // The clock reads stay inside the exempt
                            // dedup/outstanding_replies modules.
                            let leaked =
                                outstanding_replies.sweep_expired_now(INFLIGHT_TTL).await;
                            {
                                let mut d = dedup_state.lock().await;
                                d.evict_done_expired_now();
                                if !leaked.is_empty() {
                                    d.expire_inflight_explicit(&leaked);
                                }
                            }
                        }
                    }
                }
            });
            task_handles.push(sweep_handle);
        }

        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        // 5. Start Mailbox processing loop (State Path)
        // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
        tracing::info!("🔄 Starting Mailbox processing loop (State Path)");
        {
            let node = node_ref.clone();
            let mailbox = node_ref.mailbox.clone();
            let ws_gate = node_ref.websocket_gate.clone();
            let rtc_route: RtcResponseRoute = node_ref.rtc_manager.clone();
            let shutdown = shutdown_token.clone();
            let mailbox_enqueue_wake = mailbox_enqueue_wake.clone();
            let outstanding_replies = node_ref.outstanding_replies.clone();
            let max_pending_replies = node_ref.max_pending_replies;
            let hook_cb = node_hook_callback.clone();

            let mailbox_handle = tokio::spawn(async move {
                use futures_util::StreamExt as _;
                use futures_util::stream::FuturesUnordered;
                use std::sync::atomic::{AtomicBool, Ordering};

                // Concurrency gate: only overlap messages when the dispatch
                // scheduler is on. No scheduler (a keyless actor, or the gate
                // forced off) = one message at a time, bit-for-bit the pre-B2
                // loop.
                let scheduler_on = node.dispatch_scheduler.is_some();
                const MAILBOX_INFLIGHT_CAP: usize = 256;
                // RFC-0323 §7: rising-edge flag for the `max_pending_replies`
                // backpressure signal. Distinct from the mailbox-queue-depth
                // watchdog (§4.9) — this guards in-memory outstanding EXPLICIT
                // replies, not durable mailbox depth.
                let pending_saturated = AtomicBool::new(false);
                let mut inflight: FuturesUnordered<MailboxTail> = FuturesUnordered::new();

                loop {
                    // RFC-0323 §7: rising-edge backpressure signal for
                    // outstanding EXPLICIT replies. Distinct from the
                    // mailbox-queue-depth watchdog in §4.9 — this guards
                    // in-memory pending replies, not durable mailbox depth.
                    // Fires the framework `on_mailbox_backpressure` hook on
                    // the rising edge (falling back to a log line when no
                    // hook is installed), mirroring the §4.9 watchdog.
                    {
                        let pending = outstanding_replies.len();
                        if pending >= max_pending_replies {
                            if !pending_saturated.swap(true, Ordering::AcqRel) {
                                if let Some(cb) = hook_cb.as_ref() {
                                    let cb = cb.clone();
                                    tokio::spawn(async move {
                                        cb(crate::wire::webrtc::HookEvent::MailboxBackpressure {
                                            queue_len: pending,
                                            threshold: max_pending_replies,
                                        })
                                        .await;
                                    });
                                } else {
                                    tracing::warn!(
                                        outstanding_replies = pending,
                                        max_pending_replies,
                                        "explicit-reply backpressure: dequeue paused",
                                    );
                                }
                            }
                        } else if pending_saturated.swap(false, Ordering::AcqRel) {
                            tracing::info!(
                                outstanding_replies = pending,
                                max_pending_replies,
                                "explicit-reply backpressure cleared",
                            );
                        }
                    }
                    tokio::select! {
                        biased;
                        // Listen for shutdown signal
                        _ = shutdown.cancelled() => {
                            tracing::info!("📭 Mailbox loop received shutdown signal");
                            break;
                        }
                        // Drain finished reply+ack continuations (gate-on only;
                        // inert when `inflight` is empty).
                        Some(()) = inflight.next(), if !inflight.is_empty() => {}
                        // Enter admission before dequeue marks a complete batch
                        // Inflight, then retain the guard until every record in
                        // that batch has reached scheduler / runner admission.
                        // Pause while the continuation set is full so
                        // back-pressure reaches the durable mailbox.
                        // RFC-0323 §7: also pause while outstanding EXPLICIT
                        // replies are at the `max_pending_replies` cap — a
                        // stalled `Reply::send` must not let the pump keep
                        // admitting work it cannot eventually reply to.
                        batch = async {
                            let mut enqueued =
                                Box::pin(mailbox_enqueue_wake.clone().notified_owned());
                            enqueued.as_mut().enable();
                            let admission = node.mailbox_batch_admission.enter().await?;
                            Some((admission, mailbox.dequeue().await, enqueued))
                        }, if inflight.len() < MAILBOX_INFLIGHT_CAP
                            && outstanding_replies.len() < max_pending_replies => {
                            let Some((batch_admission, result, enqueued)) = batch else {
                                tracing::info!("📭 Mailbox batch admission closed");
                                break;
                            };
                            match result {
                                Ok(messages) => {
                                    if messages.is_empty() {
                                        drop(batch_admission);
                                        let idle_wake = mailbox_empty_wake(
                                            mailbox_has_push_observer,
                                            enqueued,
                                        );
                                        if wait_for_mailbox_idle(
                                            &shutdown,
                                            &mut inflight,
                                            idle_wake,
                                        )
                                        .await
                                            == MailboxIdleOutcome::Shutdown
                                        {
                                            break;
                                        }
                                        continue;
                                    }
                                    tracing::debug!("📬 Mailbox dequeue: {} messages", messages.len());

                                    // Process messages one by one
                                    for msg_record in messages {
                                        // Deserialize RpcEnvelope (Protobuf)
                                        match RpcEnvelope::decode(&msg_record.payload[..]) {
                                            Ok(envelope) => {
                                                let request_id = envelope.request_id.clone();
                                                // Wall-clock on purpose: enqueue stamps are persisted in the
                                                // SQLite mailbox, so only wall time is comparable across
                                                // restarts. Observational only; clamped non-negative against
                                                // clock jumps.
                                                let queue_latency_ms = (chrono::Utc::now() - msg_record.created_at).num_milliseconds().max(0);
                                                tracing::info!(request_id = %request_id, queue_latency_ms = queue_latency_ms, "rpc.mailbox.dequeued");

                                                tracing::debug!("📦 Processing message: request_id={}", request_id);

                                                // Route strictly on the explicit direction label:
                                                // Request runs the handler and sends a response;
                                                // Tell runs the handler but suppresses the reply —
                                                // an unwanted reply becomes an orphan response on
                                                // the caller (#262). Anything else (missing /
                                                // Unspecified / Response / unknown) is invalid in
                                                // the dispatch mailbox: warn, ack (to avoid
                                                // redelivery), and drop. timeout_ms is never
                                                // consulted for tell-ness.
                                                let expects_response = match Inner::dispatch_expects_response(envelope.direction) {
                                                    Some(v) => v,
                                                    None => {
                                                        tracing::warn!(
                                                            request_id = %request_id,
                                                            route_key = %envelope.route_key,
                                                            direction = ?envelope.direction,
                                                            "rpc.invalid_direction_dropped: non-dispatchable RpcEnvelope.direction in mailbox; dropping"
                                                        );
                                                        if let Err(e) = mailbox.ack(msg_record.id).await {
                                                            tracing::error!(
                                                                severity = 9,
                                                                error_category = "mailbox_error",
                                                                request_id = %request_id,
                                                                message_id = %msg_record.id,
                                                                "❌ Mailbox ACK failed for dropped envelope: {:?}",
                                                                e
                                                            );
                                                        }
                                                        continue;
                                                    }
                                                };

                                                #[cfg(feature = "opentelemetry")]
                                                let span = {
                                                    let actr_id_str = node.actor_id.as_ref().map(|id| id.to_string()).unwrap_or_default();
                                                    let span = tracing::info_span!("ActrNode.mailbox_receive", actr_id = %actr_id_str, request_id = %request_id, queue_wait_ms = queue_latency_ms);
                                                    set_parent_from_rpc_envelope(&span, &envelope);
                                                    span
                                                };

                                                // Decode caller_id from MessageRecord.from (transport layer)
                                                let caller_id_result = ActrId::decode(&msg_record.from[..]);
                                                let caller_type = recorded_caller_type(&msg_record);
                                                let caller_ref = caller_id_result
                                                    .as_ref()
                                                    .ok()
                                                    .map(|id| (id, caller_type.as_ref()));

                                                if caller_ref.is_none() {
                                                    tracing::warn!(
                                                        request_id = %request_id,
                                                        "⚠️  Failed to decode caller_id from MessageRecord.from"
                                                    );
                                                }

                                                let route_key = envelope.route_key.clone();
                                                let traceparent = envelope.traceparent.clone();
                                                let tracestate = envelope.tracestate.clone();
                                                let msg_id = msg_record.id;
                                                let ws_gate = ws_gate.clone();
                                                let rtc_route = rtc_route.clone();
                                                let mailbox_c = mailbox.clone();
                                                #[cfg(not(feature = "opentelemetry"))]
                                                let span = tracing::Span::none();

                                                if scheduler_on {
                                                    // Synchronous admission preserves arrival order
                                                    // + back-pressure; the reply+ack continuation
                                                    // overlaps with later messages. Per-message
                                                    // reply→ack order is unchanged inside the tail.
                                                    let caller_owned = caller_id_result
                                                        .clone()
                                                        .map_err(|e| e.to_string());
                                                    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
                                                        caller: caller_owned.clone(),
                                                        msg_id,
                                                        span: span.clone(),
                                                    };
                                                    let cont = node
                                                        .admit_incoming(envelope.clone(), caller_ref, Some(reply_sink), None)
                                                        .await;
                                                    inflight.push(Box::pin(async move {
                                                        match cont.await {
                                                            IncomingOutcome::Replied(result) => {
                                                                mailbox_reply_and_ack(
                                                                    ws_gate, rtc_route, mailbox_c,
                                                                    caller_owned, request_id, route_key,
                                                                    traceparent, tracestate, expects_response,
                                                                    msg_id, result, span,
                                                                )
                                                                .await;
                                                            }
                                                            IncomingOutcome::Deferred => {}
                                                        }
                                                    }));
                                                } else {
                                                    // Gate off: run to completion inline —
                                                    // bit-for-bit the pre-B2 mailbox path.
                                                    let caller_owned =
                                                        caller_id_result.clone().map_err(|e| e.to_string());
                                                    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
                                                        caller: caller_owned.clone(),
                                                        msg_id,
                                                        span: span.clone(),
                                                    };
                                                    let handle_incoming_fut =
                                                        node.handle_incoming(envelope.clone(), caller_ref, Some(reply_sink), None);
                                                    #[cfg(feature = "opentelemetry")]
                                                    let handle_incoming_fut =
                                                        handle_incoming_fut.instrument(span.clone());
                                                    match handle_incoming_fut.await {
                                                        IncomingOutcome::Replied(result) => {
                                                            mailbox_reply_and_ack(
                                                                ws_gate, rtc_route, mailbox_c,
                                                                caller_owned, request_id, route_key,
                                                                traceparent, tracestate, expects_response,
                                                                msg_id, result, span,
                                                            )
                                                            .await;
                                                        }
                                                        IncomingOutcome::Deferred => {}
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                // Poison message - cannot decode RpcEnvelope
                                                tracing::error!(
                                                    severity = 9,
                                                    error_category = "protobuf_decode",
                                                    message_id = %msg_record.id,
                                                    "❌ Poison message: Failed to deserialize RpcEnvelope: {:?}",
                                                    e
                                                );

                                                // Write to Dead Letter Queue
                                                use actr_runtime_mailbox::DlqRecord;
                                                use chrono::Utc;
                                                use uuid::Uuid;

                                                let dlq_record = DlqRecord {
                                                    id: Uuid::new_v4(),
                                                    original_message_id: Some(msg_record.id.to_string()),
                                                    from: Some(msg_record.from.clone()),
                                                    to: node.actor_id.as_ref().map(|id| {
                                                        let mut buf = Vec::new();
                                                        id.encode(&mut buf).unwrap();
                                                        buf
                                                    }),
                                                    raw_bytes: msg_record.payload.clone(),
                                                    error_message: format!("Protobuf decode failed: {e}"),
                                                    error_category: "protobuf_decode".to_string(),
                                                    trace_id: format!("mailbox-{}", msg_record.id),
                                                    request_id: None,
                                                    created_at: Utc::now(),
                                                    redrive_attempts: 0,
                                                    last_redrive_at: None,
                                                    context: Some(format!(
                                                        r#"{{"source":"mailbox","priority":"{}"}}"#,
                                                        match msg_record.priority {
                                                            actr_runtime_mailbox::MessagePriority::High => "high",
                                                            actr_runtime_mailbox::MessagePriority::Normal => "normal",
                                                        }
                                                    )),
                                                };

                                                if let Err(dlq_err) = node.dlq.enqueue(dlq_record).await {
                                                    tracing::error!(
                                                        severity = 10,
                                                        "❌ CRITICAL: Failed to write poison message to DLQ: {:?}",
                                                        dlq_err
                                                    );
                                                } else {
                                                    tracing::warn!(
                                                        severity = 9,
                                                        "☠️ Poison message moved to DLQ: message_id={}",
                                                        msg_record.id
                                                    );
                                                }

                                                // ACK the poison message to remove from mailbox
                                                let _ = mailbox.ack(msg_record.id).await;
                                            }
                                        }
                                    }
                                    // Keep the read guard through the complete
                                    // batch, including inline serial handling.
                                    // Shutdown can close scheduler / runner
                                    // intake only after this point.
                                    drop(batch_admission);
                                }
                                Err(e) => {
                                    // A failed dequeue marked no batch Inflight,
                                    // so it need not delay graceful shutdown
                                    // during the retry backoff.
                                    drop(batch_admission);
                                    tracing::error!(
                                        severity = 9,
                                        error_category = "mailbox_error",
                                        "❌ Mailbox dequeue failed: {:?}", e
                                    );
                                    tokio::select! {
                                        biased;
                                        _ = shutdown.cancelled() => break,
                                        _ = crate::timer::sleep(
                                            crate::timer::ids::MAILBOX_ERROR_BACKOFF,
                                            Duration::from_secs(1),
                                        ) => {}
                                    }
                                }
                            }
                        }
                    }
                }
                // Drain any remaining reply+ack continuations so no in-flight
                // reply is orphaned on shutdown.
                while inflight.next().await.is_some() {}
                tracing::info!("✅ Mailbox processing loop terminated gracefully");
            });

            task_handles.push(mailbox_handle);
        }
        tracing::info!("✅ Mailbox processing loop started");
        tracing::info!("✅ ActrNode started successfully");

        {
            let ready_ctx = bootstrap_ctx_builder
                .build_bootstrap(&actor_id, &credential_state.credential().await);
            let invocation = lifecycle_invocation(&actor_id, "lifecycle:on_ready");
            let call_executor =
                lifecycle_host_abi(ready_ctx.clone(), node_ref.workload_dispatch.clone());
            if let Err(e) = crate::lifecycle::hooks::call_lifecycle_hook(
                "on_ready",
                node_ref
                    .workload_dispatch
                    .on_ready(ready_ctx, invocation, &call_executor),
            )
            .await
            {
                tracing::warn!(error = %e, "workload on_ready returned Err");
            }
        }

        // Create ActrRefShared
        let shared = Arc::new(ActrRefShared {
            actor_id,
            bootstrap_ctx_builder,
            credential_state,
            session_state: Some(session_state),
            shutdown_token,
            task_handles: Mutex::new(task_handles),
            rtc_manager: rtc_manager.clone(),
        });

        // Create ActrRef
        tracing::info!("✅ ActrRef created (Shell → Guest communication handle)");

        Ok(ActrRef { shared })
    }
}

#[cfg(test)]
#[path = "node_tests.rs"]
mod tests;
