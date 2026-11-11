//! WebSocket signaling handling.
//!
//! This module implements the signaling-server side of the WebSocket
//! protocol defined in `actr-protocol`. It processes `SignalingEnvelope`
//! frames, handles registration / route discovery / ping-pong / relay
//! forwarding, and keeps an in-memory registry of connected actors.
//!
//! All low-level framing (accept/upgrade/split/ping) is delegated to axum's
//! `WebSocketUpgrade`; this module is purely protocol logic so the HTTP
//! server and the signaling server can share a single TCP listener.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use actr_protocol::prost::Message as ProstMessage;
use actr_protocol::{
    AIdCredential, ActrId, ActrRelay, RegisterRequest, RegisterResponse, RelayDeliveryResult,
    RelayDeliveryStatus, RouteCandidatesResponse, SIGNALING_ENVELOPE_VERSION, SignalingEnvelope,
    SignalingToActr, TurnCredential, actr_relay, actr_to_signaling, peer_to_signaling,
    register_response, route_candidates_response, signaling_envelope, signaling_to_actr,
};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header::SEC_WEBSOCKET_PROTOCOL};
use axum::response::IntoResponse;
use base64::Engine as _;
use bytes::Bytes;
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::state::{MockState, RegisteredActor};

/// Legacy test-only query parameter. Production clients carry their actor
/// identity in the `x-actr-id` upgrade header.
#[derive(Debug, Default, Deserialize)]
pub struct WsQuery {
    actor_id: Option<String>,
    /// Base64 `IdentityClaims`, sent by native clients next to `actor_id`.
    claims: Option<String>,
}

/// Axum handler: upgrades the request to a WebSocket and hands it to the
/// signaling loop.
pub async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsQuery>,
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
) -> impl IntoResponse {
    let actor_id_param = headers
        .get("x-actr-id")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
        .or_else(|| browser_actor_id(&headers))
        .or(params.actor_id);
    let claimed_type = params.claims.as_deref().and_then(claims_actor_type);
    ws.protocols(["actr.v1"])
        .on_upgrade(move |socket| handle_connection(socket, state, actor_id_param, claimed_type))
}

/// Actor type carried in the credential claims. The mock does not verify the
/// signature; it only needs the type for directly connected actors.
fn claims_actor_type(claims_b64: &str) -> Option<actr_protocol::ActrType> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(claims_b64)
        .ok()?;
    actr_protocol::IdentityClaims::decode(bytes.as_slice())
        .ok()
        .map(|claims| claims.actor_type)
}

fn browser_actor_id(headers: &HeaderMap) -> Option<String> {
    let encoded = headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(str::trim))
        .find_map(|protocol| protocol.strip_prefix("actr.id."))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

async fn handle_connection(
    socket: WebSocket,
    state: Arc<MockState>,
    actor_id_param: Option<String>,
    claimed_type: Option<actr_protocol::ActrType>,
) {
    state.connection_count.fetch_add(1, Ordering::SeqCst);
    let connection_generation = state.websocket_generation.fetch_add(1, Ordering::SeqCst) + 1;

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<Message>();
    let client_id = uuid::Uuid::new_v4().to_string();
    let cancel = state.cancel.clone();

    state
        .clients
        .write()
        .await
        .insert(client_id.clone(), client_tx);

    // If the peer supplied an Actor ID through native upgrade headers or the
    // browser subprotocol transport, bind its registry entry to this WS
    // client_id so relays can reach it.
    //
    // If the actor was *not* previously HTTP-registered, lazily create a
    // registry stub. Production Issuer bootstraps via HTTP first; integration
    // tests skip that step and connect WS directly with a synthetic credential.
    // Auto-binding restores parity with the legacy mock-signaling broadcast
    // fallback without weakening the rebind diagnostic below.
    if let Some(actor_id_str) = actor_id_param.as_deref() {
        if let Some(actr_id) = parse_actor_id(actor_id_str) {
            let mut registry = state.registry.write().await;
            let mut rebound_from: Option<String> = None;
            let mut found = false;
            for entry in registry.iter_mut() {
                if entry.actr_id == actr_id {
                    // TD-004 defensive WARN: overwriting a non-empty client_id
                    // means a second WS is rebinding the same registry row;
                    // the previous session silently loses its relay binding.
                    // Useful for spotting Issuer credential reuse across tabs.
                    if !entry.client_id.is_empty() && entry.client_id != client_id {
                        rebound_from = Some(entry.client_id.clone());
                    }
                    entry.client_id = client_id.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                registry.push(crate::state::RegisteredActor {
                    actr_id: actr_id.clone(),
                    actr_type: claimed_type.unwrap_or_default(),
                    client_id: client_id.clone(),
                    ws_address: None,
                    service_spec: None,
                });
            }
            drop(registry);
            bind_client_to_actor(&state, &client_id, actr_id).await;
            if let Some(old_ws) = rebound_from.as_deref() {
                tracing::warn!(
                    actor_id = %actor_id_str,
                    old_ws = %old_ws,
                    new_ws = %client_id,
                    "mock-loran: WS actor rebound — previous session loses this binding"
                );
            } else if found {
                tracing::info!(
                    actor_id = %actor_id_str,
                    "mock-loran: WS bound to HTTP-registered actor"
                );
            } else {
                tracing::debug!(
                    actor_id = %actor_id_str,
                    "mock-loran: WS bound to lazily-created registry entry (no prior HTTP register)"
                );
            }
        }
    }

    loop {
        let blackhole_generation = state.blackhole_websocket_generation.load(Ordering::Acquire);
        if blackhole_generation > 0 && connection_generation <= blackhole_generation {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = ws_tx.send(Message::Close(None)).await;
                    break;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
            continue;
        }

        tokio::select! {
            // Server-wide shutdown: close the socket so the peer notices.
            _ = cancel.cancelled() => {
                let _ = ws_tx.send(Message::Close(None)).await;
                break;
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        state.message_count.fetch_add(1, Ordering::Relaxed);
                        if let Ok(envelope) =
                            <SignalingEnvelope as ProstMessage>::decode(&data[..])
                        {
                            state.received_messages.lock().await.push(envelope.clone());
                            process_envelope(envelope, &client_id, &state).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(err)) => {
                        tracing::debug!(%err, "mock-loran: ws recv error");
                        break;
                    }
                    _ => {}
                }
            }
            out = client_rx.recv() => {
                match out {
                    Some(msg) => {
                        if ws_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // Cleanup: drop the WS sender and clear this client's binding on every
    // registry entry it owned. We keep the registry rows themselves so that
    // HTTP-registered actors remain discoverable across reconnects (their
    // `client_id` is simply reset to the empty string).
    state.clients.write().await.remove(&client_id);
    {
        let mut registry = state.registry.write().await;
        for entry in registry.iter_mut() {
            if entry.client_id == client_id {
                entry.client_id.clear();
            }
        }
    }
    state.client_to_actr_id.write().await.remove(&client_id);
    state.disconnection_count.fetch_add(1, Ordering::SeqCst);
}

async fn process_envelope(envelope: SignalingEnvelope, sender_id: &str, state: &Arc<MockState>) {
    let Some(flow) = envelope.flow.as_ref() else {
        return;
    };

    match flow {
        signaling_envelope::Flow::PeerToServer(peer_msg) => {
            handle_peer_to_server(&envelope, peer_msg, sender_id, state).await;
        }
        signaling_envelope::Flow::ActrToServer(actr_msg) => {
            handle_actr_to_server(&envelope, actr_msg, sender_id, state).await;
        }
        signaling_envelope::Flow::ActrRelay(relay) => {
            handle_actr_relay(&envelope, relay, sender_id, state).await;
        }
        _ => {}
    }
}

async fn handle_peer_to_server(
    envelope: &SignalingEnvelope,
    peer_msg: &actr_protocol::PeerToSignaling,
    sender_id: &str,
    state: &Arc<MockState>,
) {
    let Some(payload) = peer_msg.payload.as_ref() else {
        return;
    };

    match payload {
        peer_to_signaling::Payload::RegisterRequest(req) => {
            let register_ok = build_register_ok(req, state).await;

            // Store registration
            {
                let entry = RegisteredActor {
                    actr_id: register_ok.actr_id.clone(),
                    actr_type: req.actr_type.clone(),
                    client_id: sender_id.to_string(),
                    ws_address: req.ws_address.clone(),
                    service_spec: req.service_spec.clone(),
                };
                state.registry.write().await.push(entry);
                bind_client_to_actor(state, sender_id, register_ok.actr_id.clone()).await;
            }

            let response = RegisterResponse {
                result: Some(register_response::Result::Success(register_ok.clone())),
            };

            let response_envelope = SignalingEnvelope {
                envelope_version: SIGNALING_ENVELOPE_VERSION,
                envelope_id: uuid::Uuid::new_v4().to_string(),
                reply_for: Some(envelope.envelope_id.clone()),
                flow: Some(signaling_envelope::Flow::ServerToActr(SignalingToActr {
                    target: register_ok.actr_id.clone(),
                    payload: Some(signaling_to_actr::Payload::RegisterResponse(response)),
                })),
                traceparent: None,
                tracestate: None,
            };

            send_to_client(sender_id, &response_envelope, state).await;

            tracing::info!(
                serial = register_ok.actr_id.serial_number,
                publisher = req.actr_type.publisher,
                name = req.actr_type.name,
                "mock-loran: registered actor (ws)"
            );
        }
    }
}

/// Build a `RegisterOk` for the given `RegisterRequest`.
///
/// This is the shared code path used by both the WS signaling handler and the
/// `POST /register` HTTP handler (so `IssuerClient::register_with_manifest`
/// and the WebSocket bootstrap always see identical responses).
pub async fn build_register_ok(
    req: &RegisterRequest,
    state: &Arc<MockState>,
) -> register_response::RegisterOk {
    let serial = state.next_serial.fetch_add(1, Ordering::SeqCst);

    let actr_id = ActrId {
        realm: req.realm.clone(),
        serial_number: serial,
    };

    let claims = actr_protocol::IdentityClaims {
        realm: req.realm.clone(),
        protocol_version: 1,
        issuer: req.realm.clone(),
        provider: "provider.example".to_string(),
        audience: "actr.signaling".to_string(),
        // Use the full ActrId string representation so WebSocketGate's
        // credential verifier can match it against the X-Actr-Source-ID header.
        actor_id: actr_id.to_string_repr(),
        expires_at: chrono::Utc::now().timestamp() as u64 + 86400,
        actor_type: req.actr_type.clone(),
        member: "member-1".to_string(),
        client_key: "0000000000000000".to_string(),
        role: actr_protocol::MemberRole::Owner as i32,
        development: None,
    };
    let claims_bytes = claims.encode_to_vec();
    let signature = state.issuer_signing_key().sign(&claims_bytes);
    let verifying_key = state.issuer_signing_key().verifying_key();

    let credential = AIdCredential {
        key_id: state.issuer_signing_key_id(),
        claims: claims_bytes.into(),
        signature: signature.to_bytes().to_vec().into(),
    };

    let turn_credential = TurnCredential {
        protocol_version: 1,
        realm: req.realm.clone(),
        provider: "provider.example".to_string(),
        audience: "actr.turn".to_string(),
        username: format!("{}:{}", chrono::Utc::now().timestamp() + 86400, serial),
        password: format!("mock-turn-{serial:016x}"),
        expires_at: chrono::Utc::now().timestamp() as u64 + 86400,
    };

    register_response::RegisterOk {
        actr_id,
        credential,
        turn_credential,
        credential_expires_at: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp() + 86400,
            nanos: 0,
        }),
        signaling_heartbeat_interval_secs: 30,
        signing_pubkey: verifying_key.to_bytes().to_vec().into(),
        signing_key_id: state.issuer_signing_key_id(),
        renewal_token: Some(mock_renewal_token(serial).into()),
        renewal_token_expires_at: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp() + 86400,
            nanos: 0,
        }),
    }
}

pub(crate) fn mock_renewal_token(serial: u64) -> Vec<u8> {
    use sha2::{Digest as _, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"actr-mock-loran-renewal-token-v1");
    hasher.update(serial.to_be_bytes());
    hasher.finalize().to_vec()
}

async fn handle_actr_to_server(
    envelope: &SignalingEnvelope,
    actr_msg: &actr_protocol::ActrToSignaling,
    sender_id: &str,
    state: &Arc<MockState>,
) {
    let Some(payload) = actr_msg.payload.as_ref() else {
        return;
    };

    match payload {
        actr_to_signaling::Payload::Ping(_ping) => {
            let pong = actr_protocol::Pong {
                seq: state.message_count.load(Ordering::Relaxed) as u64,
                suggest_interval_secs: Some(30),
                credential_warning: state.credential_warning.load(Ordering::Relaxed).then(|| {
                    actr_protocol::CredentialWarning {
                        r#type: 0,
                        message: "test requested credential renewal".into(),
                    }
                }),
            };

            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(signaling_to_actr::Payload::Pong(pong)),
                },
                state,
            )
            .await;
        }

        actr_to_signaling::Payload::RouteCandidatesRequest(req) => {
            let registry = state.registry.read().await;
            let (mut candidates, mut ws_address_map, skipped_unbound) =
                collect_reachable_route_candidates(&registry, &actr_msg.source, &req.target_type);

            if let Some(criteria) = &req.criteria {
                let max = criteria.candidate_count as usize;
                candidates.truncate(max);
                ws_address_map
                    .retain(|entry| candidates.iter().any(|id| id == &entry.candidate_id));
            }

            tracing::info!(
                count = candidates.len(),
                skipped_unbound,
                target_type = format!("{}.{}", req.target_type.publisher, req.target_type.name),
                "mock-loran: route candidates response"
            );

            let ok = route_candidates_response::RouteCandidatesOk {
                candidates,
                ws_address_map,
            };

            let response = RouteCandidatesResponse {
                result: Some(route_candidates_response::Result::Success(ok)),
            };

            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(signaling_to_actr::Payload::RouteCandidatesResponse(
                        response,
                    )),
                },
                state,
            )
            .await;
        }

        actr_to_signaling::Payload::UnregisterRequest(_) => {
            {
                let mut registry = state.registry.write().await;
                registry.retain(|a| a.client_id != sender_id);
            }

            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(signaling_to_actr::Payload::UnregisterResponse(
                        actr_protocol::UnregisterResponse {
                            result: Some(actr_protocol::unregister_response::Result::Success(
                                actr_protocol::unregister_response::UnregisterOk {},
                            )),
                        },
                    )),
                },
                state,
            )
            .await;
        }

        actr_to_signaling::Payload::GetSigningKeyRequest(req) => {
            let verifying_key = state.issuer_signing_key().verifying_key();
            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(signaling_to_actr::Payload::GetSigningKeyResponse(
                        actr_protocol::GetSigningKeyResponse {
                            key_id: req.key_id,
                            pubkey: verifying_key.to_bytes().to_vec().into(),
                        },
                    )),
                },
                state,
            )
            .await;
        }

        actr_to_signaling::Payload::DiscoveryRequest(req) => {
            let registry = state.registry.read().await;

            let entries: Vec<_> = registry
                .iter()
                .filter(|e| {
                    req.publisher
                        .as_ref()
                        .is_none_or(|m| &e.actr_type.publisher == m)
                })
                .map(|e| {
                    let fingerprint = e
                        .service_spec
                        .as_ref()
                        .map_or(String::new(), |s| s.fingerprint.clone());
                    actr_protocol::discovery_response::TypeEntry {
                        actr_type: e.actr_type.clone(),
                        name: e
                            .service_spec
                            .as_ref()
                            .map_or(e.actr_type.name.clone(), |s| s.name.clone()),
                        description: e.service_spec.as_ref().and_then(|s| s.description.clone()),
                        service_fingerprint: fingerprint,
                        published_at: e.service_spec.as_ref().and_then(|s| s.published_at),
                        tags: e.service_spec.as_ref().map_or(vec![], |s| s.tags.clone()),
                    }
                })
                .collect();

            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(signaling_to_actr::Payload::DiscoveryResponse(
                        actr_protocol::DiscoveryResponse {
                            result: Some(actr_protocol::discovery_response::Result::Success(
                                actr_protocol::discovery_response::DiscoveryOk { entries },
                            )),
                        },
                    )),
                },
                state,
            )
            .await;
        }

        actr_to_signaling::Payload::GetServiceSpecRequest(req) => {
            let registry = state.registry.read().await;

            let spec = registry.iter().find_map(|e| {
                if let Some(spec) = &e.service_spec {
                    if spec.name == req.name {
                        return Some(spec.clone());
                    }
                }
                None
            });

            let payload = match spec {
                Some(spec) => signaling_to_actr::Payload::GetServiceSpecResponse(
                    actr_protocol::GetServiceSpecResponse {
                        result: Some(actr_protocol::get_service_spec_response::Result::Success(
                            spec,
                        )),
                    },
                ),
                None => signaling_to_actr::Payload::GetServiceSpecResponse(
                    actr_protocol::GetServiceSpecResponse {
                        result: Some(actr_protocol::get_service_spec_response::Result::Error(
                            actr_protocol::ErrorResponse {
                                code: 404,
                                message: format!("service '{}' not found", req.name),
                            },
                        )),
                    },
                ),
            };

            send_response(
                sender_id,
                envelope,
                SignalingToActr {
                    target: actr_msg.source.clone(),
                    payload: Some(payload),
                },
                state,
            )
            .await;
        }

        _ => {
            tracing::debug!("mock-loran: ignoring ActrToSignaling payload");
        }
    }
}

async fn handle_actr_relay(
    envelope: &SignalingEnvelope,
    relay: &ActrRelay,
    sender_id: &str,
    state: &Arc<MockState>,
) {
    if let Some(actr_relay::Payload::SessionDescription(sd)) = relay.payload.as_ref() {
        if sd.r#type == 3 {
            state.ice_restart_offer_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    if let Some(actr_relay::Payload::IceRestartRequest(_)) = relay.payload.as_ref() {
        state
            .ice_restart_request_count
            .fetch_add(1, Ordering::SeqCst);

        if state
            .ice_restart_request_drop_count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                if count > 0 { Some(count - 1) } else { None }
            })
            .is_ok()
        {
            tracing::warn!("🧪 Dropping test ICE restart request relay");
            return;
        }
    }

    if state.pause_forwarding.load(Ordering::Acquire) {
        return;
    }

    if let Some(actr_relay::Payload::IceCandidate(_)) = relay.payload.as_ref() {
        let delay = {
            let delay_until = *state
                .ice_candidate_delay_until
                .lock()
                .expect("ICE candidate delay mutex poisoned");
            delay_until.and_then(|deadline| deadline.checked_duration_since(Instant::now()))
        };

        if let Some(delay) = delay
            && delay > Duration::ZERO
        {
            state
                .ice_candidate_delay_applied_count
                .fetch_add(1, Ordering::SeqCst);
            tracing::warn!("🧪 Delaying test ICE candidate relay by {:?}", delay);
            let delayed_envelope = envelope.clone();
            let delayed_relay = relay.clone();
            let delayed_sender_id = sender_id.to_string();
            let delayed_state = Arc::clone(state);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                forward_relay_to_target(
                    &delayed_envelope,
                    &delayed_relay,
                    &delayed_sender_id,
                    &delayed_state,
                )
                .await;
            });
            return;
        }
    }

    if let Some(actr_relay::Payload::IceCandidate(_)) = relay.payload.as_ref()
        && state
            .ice_candidate_drop_count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                if count > 0 { Some(count - 1) } else { None }
            })
            .is_ok()
    {
        tracing::warn!("🧪 Dropping test ICE candidate relay");
        return;
    }

    if let Some(actr_relay::Payload::SessionDescription(sd)) = relay.payload.as_ref()
        && sd.r#type == actr_protocol::session_description::Type::Offer as i32
        && state
            .offer_drop_count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                if count > 0 { Some(count - 1) } else { None }
            })
            .is_ok()
    {
        tracing::warn!("🧪 Dropping test SDP offer relay");
        return;
    }

    // Role negotiation: server decides offerer/answerer by serial ordering.
    if let Some(actr_relay::Payload::RoleNegotiation(role_neg)) = relay.payload.as_ref() {
        let from_is_offerer = role_neg.from.serial_number < role_neg.to.serial_number;

        let envelope_for_from = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: uuid::Uuid::new_v4().to_string(),
            reply_for: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(ActrRelay {
                source: role_neg.to.clone(),
                credential: AIdCredential::default(),
                target: role_neg.from.clone(),
                delivery_context: None,
                source_type: registered_type(state, &role_neg.to).await,
                payload: Some(actr_relay::Payload::RoleAssignment(
                    actr_protocol::RoleAssignment {
                        is_offerer: from_is_offerer,
                        remote_fixed: None,
                    },
                )),
            })),
            traceparent: None,
            tracestate: None,
        };

        let envelope_for_to = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: uuid::Uuid::new_v4().to_string(),
            reply_for: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(ActrRelay {
                source: role_neg.from.clone(),
                credential: AIdCredential::default(),
                target: role_neg.to.clone(),
                delivery_context: None,
                source_type: registered_type(state, &role_neg.from).await,
                payload: Some(actr_relay::Payload::RoleAssignment(
                    actr_protocol::RoleAssignment {
                        is_offerer: !from_is_offerer,
                        remote_fixed: None,
                    },
                )),
            })),
            traceparent: None,
            tracestate: None,
        };

        let (from_client_id, to_client_id) = {
            let client_map = state.client_to_actr_id.read().await;
            resolve_role_negotiation_targets(&client_map, sender_id, &role_neg.from, &role_neg.to)
        };

        tracing::info!(
            from_actor = %role_neg.from.to_string_repr(),
            to_actor = %role_neg.to.to_string_repr(),
            from_client_id = ?from_client_id,
            to_client_id = ?to_client_id,
            "mock-loran: role negotiation routed pairwise"
        );

        if let Some(target_client_id) = to_client_id.as_deref() {
            let _ = send_to_client(&from_client_id, &envelope_for_from, state).await;
            // Production keys the delivery disposition on the assignment
            // sent to `to`: the target session exists, so a failed egress
            // is an ordinary rejection, not unavailability.
            let target_sent = send_to_client(target_client_id, &envelope_for_to, state).await;
            send_relay_delivery_result(
                sender_id,
                envelope,
                relay,
                if target_sent {
                    RelayDeliveryStatus::RelayDeliveryAccepted
                } else {
                    RelayDeliveryStatus::RelayDeliveryRejected
                },
                state,
            )
            .await;
        } else {
            tracing::warn!(
                from_actor = %role_neg.from.to_string_repr(),
                to_actor = %role_neg.to.to_string_repr(),
                "mock-loran: role negotiation target has no bound websocket"
            );
            send_relay_delivery_result(
                sender_id,
                envelope,
                relay,
                RelayDeliveryStatus::RelayDeliveryTargetUnavailable,
                state,
            )
            .await;
        }
        return;
    }

    let egress = forward_relay_to_target(envelope, relay, sender_id, state).await;
    send_relay_delivery_result(sender_id, envelope, relay, egress.wire_status(), state).await;
}

/// Egress disposition of one relay in the mock, mirroring production's
/// queue-rejection vs target-unavailable split so delivery results stay
/// faithful to Loran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MockRelayEgress {
    Delivered,
    Rejected,
    TargetUnavailable,
}

impl MockRelayEgress {
    fn wire_status(self) -> RelayDeliveryStatus {
        match self {
            MockRelayEgress::Delivered => RelayDeliveryStatus::RelayDeliveryAccepted,
            MockRelayEgress::Rejected => RelayDeliveryStatus::RelayDeliveryRejected,
            MockRelayEgress::TargetUnavailable => {
                RelayDeliveryStatus::RelayDeliveryTargetUnavailable
            }
        }
    }
}

async fn forward_relay_to_target(
    envelope: &SignalingEnvelope,
    relay: &ActrRelay,
    sender_id: &str,
    state: &Arc<MockState>,
) -> MockRelayEgress {
    // Like Signaling, stamp the source Actor type from the registry; the
    // sender's value is discarded.
    let mut relay = relay.clone();
    relay.source_type = registered_type(state, &relay.source).await;

    // Forward relay to target by ActrId lookup.
    let target_id = &relay.target;
    let client_map = state.client_to_actr_id.read().await;
    let clients = state.clients.read().await;

    let target_client_id = client_map.iter().find_map(|(cid, aid)| {
        if aid == target_id {
            Some(cid.clone())
        } else {
            None
        }
    });

    if let Some(target_cid) = target_client_id {
        if let Some(tx) = clients.get(&target_cid) {
            let forwarded = SignalingEnvelope {
                envelope_version: SIGNALING_ENVELOPE_VERSION,
                envelope_id: uuid::Uuid::new_v4().to_string(),
                reply_for: None,
                flow: Some(signaling_envelope::Flow::ActrRelay(relay.clone())),
                traceparent: envelope.traceparent.clone(),
                tracestate: envelope.tracestate.clone(),
            };
            let encoded = forwarded.encode_to_vec();
            return match tx.send(Message::Binary(Bytes::from(encoded))) {
                Ok(()) => MockRelayEgress::Delivered,
                Err(_) => {
                    tracing::warn!(
                        source = %relay.source.to_string_repr(),
                        target = %relay.target.to_string_repr(),
                        target_client_id = %target_cid,
                        "mock-loran: relay target websocket sender closed"
                    );
                    MockRelayEgress::Rejected
                }
            };
        }
        tracing::warn!(
            source = %relay.source.to_string_repr(),
            target = %relay.target.to_string_repr(),
            target_client_id = %target_cid,
            "mock-loran: relay target client missing websocket sender"
        );
        MockRelayEgress::TargetUnavailable
    } else {
        tracing::warn!(
            source = %relay.source.to_string_repr(),
            target = %relay.target.to_string_repr(),
            sender_id = %sender_id,
            "mock-loran: dropping relay for unbound target instead of broadcasting"
        );
        MockRelayEgress::TargetUnavailable
    }
}

async fn send_relay_delivery_result(
    sender_id: &str,
    original_envelope: &SignalingEnvelope,
    relay: &ActrRelay,
    status: RelayDeliveryStatus,
    state: &Arc<MockState>,
) {
    let Some(context) = relay.delivery_context else {
        return;
    };
    send_response(
        sender_id,
        original_envelope,
        SignalingToActr {
            target: relay.source.clone(),
            payload: Some(signaling_to_actr::Payload::RelayDeliveryResult(
                RelayDeliveryResult {
                    target: relay.target.clone(),
                    context,
                    status: status as i32,
                },
            )),
        },
        state,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_response(
    client_id: &str,
    original_envelope: &SignalingEnvelope,
    payload: SignalingToActr,
    state: &Arc<MockState>,
) {
    let response_envelope = SignalingEnvelope {
        envelope_version: SIGNALING_ENVELOPE_VERSION,
        envelope_id: uuid::Uuid::new_v4().to_string(),
        reply_for: Some(original_envelope.envelope_id.clone()),
        flow: Some(signaling_envelope::Flow::ServerToActr(payload)),
        traceparent: None,
        tracestate: None,
    };
    send_to_client(client_id, &response_envelope, state).await;
}

/// Whether one egress send reached its client's WS channel. Production
/// Loran reports a full/closed target queue as `Rejected`, distinct from an
/// exact `ActrId` with no bound session (`TargetUnavailable`); the mock
/// mirrors that split so e2e recovery tests stay faithful.
async fn send_to_client(
    client_id: &str,
    envelope: &SignalingEnvelope,
    state: &Arc<MockState>,
) -> bool {
    let encoded = envelope.encode_to_vec();
    let clients = state.clients.read().await;
    match clients.get(client_id) {
        Some(tx) => tx.send(Message::Binary(Bytes::from(encoded))).is_ok(),
        None => false,
    }
}

async fn bind_client_to_actor(state: &Arc<MockState>, client_id: &str, actr_id: ActrId) {
    let mut client_map = state.client_to_actr_id.write().await;
    client_map.retain(|existing_client_id, existing_actor_id| {
        existing_client_id == client_id || existing_actor_id != &actr_id
    });
    client_map.insert(client_id.to_string(), actr_id);
}

fn resolve_role_negotiation_targets(
    client_map: &std::collections::HashMap<String, ActrId>,
    sender_id: &str,
    from: &ActrId,
    to: &ActrId,
) -> (String, Option<String>) {
    let from_client_id = client_map
        .iter()
        .find_map(|(client_id, actor_id)| (actor_id == from).then(|| client_id.clone()))
        .unwrap_or_else(|| sender_id.to_string());
    let to_client_id = client_map
        .iter()
        .find_map(|(client_id, actor_id)| (actor_id == to).then(|| client_id.clone()));
    (from_client_id, to_client_id)
}

fn collect_reachable_route_candidates(
    registry: &[RegisteredActor],
    source: &ActrId,
    target_type: &actr_protocol::ActrType,
) -> (Vec<ActrId>, Vec<actr_protocol::WsAddressEntry>, usize) {
    let mut candidates = Vec::new();
    let mut ws_address_map = Vec::new();
    let mut skipped_unbound = 0;

    // Prefer the newest live registration. During page-refresh / cross-suite
    // churn the mock can temporarily retain older WS-bound rows, and choosing
    // the oldest entry makes fresh clients rediscover dead sessions.
    for entry in registry.iter().rev() {
        if entry.actr_type != *target_type {
            continue;
        }
        if entry.actr_id.serial_number == source.serial_number {
            continue;
        }
        if entry.client_id.is_empty() {
            skipped_unbound += 1;
            continue;
        }

        candidates.push(entry.actr_id.clone());
        if let Some(ws_addr) = &entry.ws_address {
            ws_address_map.push(actr_protocol::WsAddressEntry {
                candidate_id: entry.actr_id.clone(),
                ws_address: Some(ws_addr.clone()),
            });
        }
    }

    (candidates, ws_address_map, skipped_unbound)
}

/// Actor type recorded for `actr_id`, newest registration first.
async fn registered_type(
    state: &Arc<MockState>,
    actr_id: &ActrId,
) -> Option<actr_protocol::ActrType> {
    state
        .registry
        .read()
        .await
        .iter()
        .rev()
        .find(|entry| entry.actr_id == *actr_id)
        .map(|entry| entry.actr_type.clone())
}

/// Parse an `ActrId` from the `actor_id` query parameter sent by clients
/// after HTTP registration.
fn parse_actor_id(s: &str) -> Option<ActrId> {
    ActrId::from_string_repr(s).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        MockRelayEgress, build_register_ok, collect_reachable_route_candidates,
        forward_relay_to_target, resolve_role_negotiation_targets, send_relay_delivery_result,
    };
    use crate::state::{MockState, RegisteredActor};
    use actr_protocol::prost::Message as ProstMessage;
    use actr_protocol::{
        AIdCredential, ActrId, ActrRelay, ActrType, RegisterRequest, RelayDeliveryContext,
        RelayDeliveryStatus, RoleNegotiation, SIGNALING_ENVELOPE_VERSION, SessionDescription,
        SignalingEnvelope, SignalingToActr, actr_relay, session_description::Type as SdpType,
        signaling_envelope, signaling_to_actr,
    };
    use axum::extract::ws::Message;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn actor(serial: u64) -> ActrId {
        ActrId {
            realm: "mock@example.com".to_string(),
            serial_number: serial,
        }
    }

    fn registered_actor(serial: u64, name: &str, client_id: &str) -> RegisteredActor {
        RegisteredActor {
            actr_id: actor(serial),
            actr_type: ActrType {
                publisher: "acme".to_string(),
                name: name.to_string(),
                version: "0.1.0".to_string(),
            },
            client_id: client_id.to_string(),
            ws_address: Some(format!("ws://example.invalid/{serial}")),
            service_spec: None,
        }
    }

    fn test_state() -> Arc<MockState> {
        Arc::new(MockState::new(
            SigningKey::from_bytes(&[42u8; 32]),
            7,
            SigningKey::from_bytes(&[43u8; 32]),
            CancellationToken::new(),
        ))
    }

    #[tokio::test]
    async fn register_ok_uses_32_byte_renewal_token() {
        let state = test_state();
        let req = RegisterRequest {
            actr_type: ActrType {
                publisher: "acme".to_string(),
                name: "EchoService".to_string(),
                version: "0.1.0".to_string(),
            },
            realm: "mock@example.com".to_string(),
            service_spec: None,
            acl: None,
            service: None,
            ws_address: None,
            manifest_raw: None,
            publisher_signature: None,
            target: None,
            auth_mode: None,
            signing_public_key: None,
        };

        let ok = build_register_ok(&req, &state).await;
        let token = ok
            .renewal_token
            .expect("mock registration should include renewal token");

        assert_eq!(token.len(), 32);
    }

    #[test]
    fn role_negotiation_targets_only_from_and_to() {
        let from = actor(1);
        let to = actor(2);
        let bystander = actor(3);

        let mut client_map = HashMap::new();
        client_map.insert("ws-from".to_string(), from.clone());
        client_map.insert("ws-to".to_string(), to.clone());
        client_map.insert("ws-bystander".to_string(), bystander);

        let (from_client_id, to_client_id) =
            resolve_role_negotiation_targets(&client_map, "ws-from", &from, &to);

        assert_eq!(from_client_id, "ws-from");
        assert_eq!(to_client_id.as_deref(), Some("ws-to"));
    }

    #[test]
    fn role_negotiation_falls_back_to_sender_when_from_not_bound() {
        let from = actor(1);
        let to = actor(2);

        let mut client_map = HashMap::new();
        client_map.insert("ws-to".to_string(), to.clone());

        let (from_client_id, to_client_id) =
            resolve_role_negotiation_targets(&client_map, "ws-sender", &from, &to);

        assert_eq!(from_client_id, "ws-sender");
        assert_eq!(to_client_id.as_deref(), Some("ws-to"));
    }

    #[test]
    fn route_candidates_skip_unbound_history_rows() {
        let source = actor(10);
        let target_type = ActrType {
            publisher: "acme".to_string(),
            name: "EchoService".to_string(),
            version: "0.1.0".to_string(),
        };
        let registry = vec![
            registered_actor(1, "EchoService", ""),
            registered_actor(4, "EchoService", "ws-live"),
            registered_actor(5, "EchoService", ""),
            registered_actor(6, "OtherService", "ws-other"),
        ];

        let (candidates, ws_address_map, skipped_unbound) =
            collect_reachable_route_candidates(&registry, &source, &target_type);

        assert_eq!(candidates, vec![actor(4)]);
        assert_eq!(ws_address_map.len(), 1);
        assert_eq!(ws_address_map[0].candidate_id, actor(4));
        assert_eq!(skipped_unbound, 2);
    }

    #[tokio::test]
    async fn relay_forwarding_re_envelopes_but_preserves_sdp_exchange_id() {
        let state = test_state();
        let source = actor(1);
        let target = actor(2);
        let (tx, mut rx) = mpsc::unbounded_channel();

        state
            .client_to_actr_id
            .write()
            .await
            .insert("target-ws".to_string(), target.clone());
        state
            .clients
            .write()
            .await
            .insert("target-ws".to_string(), tx);
        state
            .registry
            .write()
            .await
            .push(registered_actor(1, "client", "source-ws"));

        let relay = ActrRelay {
            source: source.clone(),
            credential: AIdCredential::default(),
            target: target.clone(),
            // The sender's value is discarded and replaced from the registry.
            source_type: Some(ActrType::default()),
            delivery_context: None,
            payload: Some(actr_relay::Payload::SessionDescription(
                SessionDescription {
                    r#type: SdpType::Answer as i32,
                    sdp: "answer-sdp".to_string(),
                    sdp_exchange_id: Some("exchange-1".to_string()),
                },
            )),
        };
        let original = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: "original-envelope".to_string(),
            reply_for: Some("hop-local-reply".to_string()),
            traceparent: None,
            tracestate: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(relay.clone())),
        };

        forward_relay_to_target(&original, &relay, "source-ws", &state).await;

        let msg = rx.recv().await.expect("target should receive relay");
        let Message::Binary(bytes) = msg else {
            panic!("expected binary signaling envelope");
        };
        let forwarded =
            SignalingEnvelope::decode(bytes.as_ref()).expect("forwarded envelope should decode");

        assert_ne!(forwarded.envelope_id, "original-envelope");
        assert!(forwarded.reply_for.is_none());
        let Some(signaling_envelope::Flow::ActrRelay(forwarded_relay)) = forwarded.flow else {
            panic!("expected forwarded ActrRelay");
        };
        assert_eq!(
            forwarded_relay.source_type.map(|t| t.name),
            Some("client".to_string())
        );
        let Some(actr_relay::Payload::SessionDescription(sd)) = forwarded_relay.payload else {
            panic!("expected forwarded SessionDescription");
        };
        assert_eq!(sd.sdp_exchange_id.as_deref(), Some("exchange-1"));
    }

    #[tokio::test]
    async fn delivery_result_echoes_context_and_correlates_to_request() {
        let state = test_state();
        let source = actor(1);
        let target = actor(2);
        let (sender_tx, mut sender_rx) = mpsc::unbounded_channel();
        state
            .client_to_actr_id
            .write()
            .await
            .insert("source-ws".to_string(), source.clone());
        state
            .clients
            .write()
            .await
            .insert("source-ws".to_string(), sender_tx);

        let relay = ActrRelay {
            source: source.clone(),
            credential: AIdCredential::default(),
            target: target.clone(),
            source_type: None,
            delivery_context: Some(RelayDeliveryContext {
                controller_lifecycle_id: 7,
                transport_epoch: 11,
                pc_generation: 13,
                negotiation_epoch: Some(29),
            }),
            payload: Some(actr_relay::Payload::RoleNegotiation(RoleNegotiation {
                from: source.clone(),
                to: target.clone(),
            })),
        };
        let original = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: "request-envelope".to_string(),
            reply_for: None,
            traceparent: None,
            tracestate: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(relay.clone())),
        };

        send_relay_delivery_result(
            "source-ws",
            &original,
            &relay,
            RelayDeliveryStatus::RelayDeliveryTargetUnavailable,
            &state,
        )
        .await;

        let msg = sender_rx
            .recv()
            .await
            .expect("delivery result should reach the sender");
        let Message::Binary(bytes) = msg else {
            panic!("expected binary signaling envelope");
        };
        let result_envelope =
            SignalingEnvelope::decode(bytes.as_ref()).expect("result envelope should decode");
        assert_eq!(
            result_envelope.reply_for.as_deref(),
            Some("request-envelope"),
            "the result must correlate to the requesting envelope"
        );
        let Some(signaling_envelope::Flow::ServerToActr(SignalingToActr {
            target: result_target,
            payload: Some(signaling_to_actr::Payload::RelayDeliveryResult(result)),
        })) = result_envelope.flow
        else {
            panic!("expected relay delivery result");
        };
        assert_eq!(result_target, source);
        assert_eq!(result.target, target);
        assert_eq!(
            result.status,
            RelayDeliveryStatus::RelayDeliveryTargetUnavailable as i32
        );
        let context = result.context;
        assert_eq!(context.controller_lifecycle_id, 7);
        assert_eq!(context.transport_epoch, 11);
        assert_eq!(context.pc_generation, 13);
        assert_eq!(context.negotiation_epoch, Some(29));
    }

    #[tokio::test]
    async fn relay_forwarding_classifies_unbound_target_as_unavailable() {
        let state = test_state();
        let source = actor(1);
        let target = actor(2);
        let relay = ActrRelay {
            source: source.clone(),
            credential: AIdCredential::default(),
            target: target.clone(),
            source_type: None,
            delivery_context: None,
            payload: Some(actr_relay::Payload::SessionDescription(
                SessionDescription {
                    r#type: SdpType::Offer as i32,
                    sdp: "offer-sdp".to_string(),
                    sdp_exchange_id: None,
                },
            )),
        };
        let original = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: "original-envelope".to_string(),
            reply_for: None,
            traceparent: None,
            tracestate: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(relay.clone())),
        };

        assert_eq!(
            forward_relay_to_target(&original, &relay, "source-ws", &state).await,
            MockRelayEgress::TargetUnavailable
        );
    }

    #[tokio::test]
    async fn relay_forwarding_classifies_send_failure_as_rejection_not_unavailable() {
        let state = test_state();
        let source = actor(1);
        let target = actor(2);
        // The target session is bound but its channel is closed: the egress
        // failed, which production reports as an ordinary rejection.
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        state
            .client_to_actr_id
            .write()
            .await
            .insert("target-ws".to_string(), target.clone());
        state
            .clients
            .write()
            .await
            .insert("target-ws".to_string(), tx);
        state
            .registry
            .write()
            .await
            .push(registered_actor(1, "client", "source-ws"));

        let relay = ActrRelay {
            source: source.clone(),
            credential: AIdCredential::default(),
            target: target.clone(),
            // The sender's value is discarded and replaced from the registry.
            source_type: Some(ActrType::default()),
            delivery_context: None,
            payload: Some(actr_relay::Payload::SessionDescription(
                SessionDescription {
                    r#type: SdpType::Offer as i32,
                    sdp: "offer-sdp".to_string(),
                    sdp_exchange_id: None,
                },
            )),
        };
        let original = SignalingEnvelope {
            envelope_version: SIGNALING_ENVELOPE_VERSION,
            envelope_id: "original-envelope".to_string(),
            reply_for: None,
            traceparent: None,
            tracestate: None,
            flow: Some(signaling_envelope::Flow::ActrRelay(relay.clone())),
        };

        assert_eq!(
            forward_relay_to_target(&original, &relay, "source-ws", &state).await,
            MockRelayEgress::Rejected
        );
    }
}
