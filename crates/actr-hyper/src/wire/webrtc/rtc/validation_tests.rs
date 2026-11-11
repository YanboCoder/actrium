//! Stage 1 validation harness for the webrtc-rs path.
//!
//! These tests prove the RTC native capabilities the RFC Stage 1 exit
//! criteria require, using only the real `webrtc` crate (no 0.14, no
//! callback adapter):
//!
//! * `rtc_offer_answer_bidirectional_close_loop` — two real `PeerConnection`s
//!   complete offer/answer over loopback, open the `RpcReliable` readiness
//!   channel, send bidirectionally, then close explicitly with task/socket
//!   convergence (the primary Stage 1 exit criterion).
//! * `rtc_datachannel_backpressure_basics` — `try_send` / `writable` /
//!   `outstanding_bytes` engage against a full send buffer.
//! * `mailbox_try_publish_is_nonblocking_and_counts_overflow` — the handler
//!   mailbox never blocks and surfaces overflow (deterministic, no network).
//! * `event_handler_tags_generation_and_forwards_facts` — the handler is a pure,
//!   non-blocking translator (deterministic, no network).

use std::sync::Arc;
use std::time::Duration;

use actr_protocol::ActrId;
use bytes::BytesMut;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use webrtc::data_channel::{DataChannelEvent, RTCDataChannelInit};
use webrtc::peer_connection::{
    PeerConnection, RTCIceConnectionState, RTCIceGatheringState, RTCPeerConnectionIceErrorEvent,
    RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSignalingState,
};

use super::address::DesktopLocalAddressProvider;
use super::event_handler::WebRtcEventHandler;
use super::events::{PeerDriverEvent, PeerEventMailboxReceiver, peer_event_mailbox};
use super::factory::PeerConnectionFactory;
use super::peer::adapter::{LaneDataChannel, RealLaneDataChannel};
use super::runtime::RtcRuntimeConfig;

/// DataChannel label for the RPC reliable lane, mirroring the 0.14 wire label
/// (`PayloadType::RpcReliable.as_str_name()`). Kept as a literal so the RTC
/// validation asserts the same wire contract the production path uses.
const RPC_RELIABLE_LABEL: &str = "RPC_RELIABLE";

const OP_TIMEOUT: Duration = Duration::from_secs(15);

fn test_actor_id(serial_number: u64) -> ActrId {
    ActrId {
        realm: "test@example.com".to_string(),
        serial_number,
    }
}

/// A factory that binds only the loopback interface, so two connections built
/// from it on one host produce usable `127.0.0.1` host candidates.
fn loopback_factory(runtime_config: RtcRuntimeConfig) -> PeerConnectionFactory {
    PeerConnectionFactory::new(
        runtime_config,
        Arc::new(DesktopLocalAddressProvider::with_override(vec![
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        ])),
        Vec::new(), // no STUN/TURN: loopback host candidates are sufficient
    )
}

/// Shared state a forwarder task reports back to the test.
struct PeerSignals {
    conn_state_rx: watch::Receiver<RTCPeerConnectionState>,
    ice_state_rx: watch::Receiver<RTCIceConnectionState>,
    incoming_dc_rx: mpsc::Receiver<Arc<dyn LaneDataChannel>>,
}

/// Consume a peer's driver events after both remote descriptions are installed,
/// trickling candidates and reporting state / incoming channels back to the test.
/// This is the Stage 1 stand-in for the controller's event loop: it performs no
/// SDP, no hooks, and no recovery decisions.
fn spawn_event_forwarder(
    mut rx: PeerEventMailboxReceiver,
    remote_pc: Arc<dyn PeerConnection>,
    resources: super::peer::session::SessionResourceRegistry,
) -> (PeerSignals, JoinHandle<()>) {
    let (conn_state_tx, conn_state_rx) = watch::channel(RTCPeerConnectionState::Unspecified);
    let (ice_state_tx, ice_state_rx) = watch::channel(RTCIceConnectionState::Unspecified);
    let (incoming_dc_tx, incoming_dc_rx) = mpsc::channel(8);

    let mut state_rx = rx.state();
    let handle = tokio::spawn(async move {
        let mut last_state_seq = 0;
        loop {
            tokio::select! {
                // Structural events (bounded queue): candidates + incoming channels.
                event = rx.recv_structural() => {
                    match event {
                        Some(PeerDriverEvent::LocalIceCandidate { candidate, .. }) => {
                            let init = candidate.to_json().expect("serialize local ICE candidate");
                            remote_pc
                                .add_ice_candidate(init)
                                .await
                                .expect("add ICE candidate after remote SDP");
                        }
                        Some(PeerDriverEvent::DataChannelDiscovered { resource_id, .. }) => {
                            if let Some(channel) = resources.take_data_channel(resource_id) {
                                let _ = incoming_dc_tx.send(channel).await;
                            }
                        }
                        Some(_) => {}
                        None => break, // structural queue closed; peer is gone
                    }
                }
                // Mergeable state events (latest-value watch): PC / ICE state.
                changed = state_rx.changed() => {
                    if changed.is_err() {
                        break; // state sender dropped; peer is gone
                    }
                    let book = state_rx.borrow_and_update().clone();
                    let ticks = book.ticks_after(last_state_seq);
                    last_state_seq = book.sequence();
                    for tick in ticks {
                        if let Some(state) = tick.pc_state {
                            let _ = conn_state_tx.send(state);
                        }
                        if let Some(state) = tick.ice_state {
                            let _ = ice_state_tx.send(state);
                        }
                    }
                }
            }
        }
    });

    (
        PeerSignals {
            conn_state_rx,
            ice_state_rx,
            incoming_dc_rx,
        },
        handle,
    )
}

/// Wait until the peer-connection state reaches `target` (or a superset that
/// implies it, e.g. Connected), watching the forwarder's state channel.
async fn wait_for_pc_state(
    rx: &mut watch::Receiver<RTCPeerConnectionState>,
    target: RTCPeerConnectionState,
) {
    timeout(OP_TIMEOUT, async {
        loop {
            if *rx.borrow() == target {
                return;
            }
            rx.changed().await.expect("state channel closed");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for peer connection state {target:?}"));
}

/// Poll a data channel until it reports `OnOpen`.
async fn wait_dc_open(dc: &Arc<dyn LaneDataChannel>, who: &str) {
    timeout(OP_TIMEOUT, async {
        loop {
            match dc.poll().await {
                Some(DataChannelEvent::OnOpen) => return,
                Some(_) => continue,
                None => panic!("{who}: data channel event stream ended before OnOpen"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{who}: timed out waiting for data channel OnOpen"));
}

/// Poll a data channel until a text message arrives, returning it.
async fn recv_text(dc: &Arc<dyn LaneDataChannel>, who: &str) -> String {
    timeout(OP_TIMEOUT, async {
        loop {
            match dc.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    return String::from_utf8(msg.data.to_vec())
                        .expect("message should be valid UTF-8");
                }
                Some(_) => continue,
                None => panic!("{who}: data channel event stream ended before a message"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{who}: timed out waiting for a data channel message"))
}

/// Build a connected pair of RTC peer connections plus the opened
/// `RpcReliable` channel ends (offerer's `dc_a`, answerer's `dc_b`).
async fn build_connected_pair(
    send_buffer_limit: usize,
) -> (
    Arc<dyn PeerConnection>,
    Arc<dyn PeerConnection>,
    Arc<dyn LaneDataChannel>,
    Arc<dyn LaneDataChannel>,
    PeerSignals,
    PeerSignals,
) {
    let factory_a = loopback_factory(
        RtcRuntimeConfig::desktop_default().with_data_channel_send_buffer_limit(send_buffer_limit),
    );
    let factory_b = loopback_factory(
        RtcRuntimeConfig::desktop_default().with_data_channel_send_buffer_limit(send_buffer_limit),
    );

    let (mb_a, rx_a) = peer_event_mailbox();
    let (mb_b, rx_b) = peer_event_mailbox();
    let handler_a = Arc::new(WebRtcEventHandler::new(test_actor_id(1), 1, mb_a));
    let handler_b = Arc::new(WebRtcEventHandler::new(test_actor_id(2), 1, mb_b));
    let resources_a = handler_a.resources();
    let resources_b = handler_b.resources();

    let pc_a = factory_a
        .build(handler_a)
        .await
        .expect("build peer connection A");
    let pc_b = factory_b
        .build(handler_b)
        .await
        .expect("build peer connection B");

    // The offerer creates the RpcReliable readiness channel *before* the offer
    // so it is negotiated in the SDP; the answerer receives it via on_data_channel.
    let dc_a = pc_a
        .create_data_channel(RPC_RELIABLE_LABEL, Some(RTCDataChannelInit::default()))
        .await
        .expect("create RpcReliable data channel");
    let dc_a: Arc<dyn LaneDataChannel> = Arc::new(RealLaneDataChannel::new(dc_a));

    // Offer / answer.
    let offer = pc_a.create_offer(None).await.expect("create offer");
    pc_a.set_local_description(offer.clone())
        .await
        .expect("set local description (offer)");
    pc_b.set_remote_description(offer)
        .await
        .expect("set remote description (offer)");
    let answer = pc_b.create_answer(None).await.expect("create answer");
    pc_b.set_local_description(answer.clone())
        .await
        .expect("set local description (answer)");
    pc_a.set_remote_description(answer)
        .await
        .expect("set remote description (answer)");

    // ICE gathering can finish before the remote SDP is installed. Keep those
    // candidates in the mailbox until both peers can accept them; an early
    // add_ice_candidate would reject and permanently lose the host candidate.
    let (signals_a, _fwd_a) = spawn_event_forwarder(rx_a, pc_b.clone(), resources_a);
    let (mut signals_b, _fwd_b) = spawn_event_forwarder(rx_b, pc_a.clone(), resources_b);

    // Wait for the peer connection to come up on both ends.
    let mut conn_a = signals_a.conn_state_rx.clone();
    let mut conn_b = signals_b.conn_state_rx.clone();
    wait_for_pc_state(&mut conn_a, RTCPeerConnectionState::Connected).await;
    wait_for_pc_state(&mut conn_b, RTCPeerConnectionState::Connected).await;

    // The answerer must have received the RpcReliable channel.
    let dc_b = timeout(OP_TIMEOUT, signals_b.incoming_dc_rx.recv())
        .await
        .expect("timed out waiting for incoming RpcReliable channel on answerer")
        .expect("incoming channel stream closed");
    assert_eq!(
        dc_b.label().await.expect("read channel label"),
        RPC_RELIABLE_LABEL,
        "answerer received a channel with the wrong label"
    );

    // Both ends of the RpcReliable channel open.
    wait_dc_open(&dc_a, "offerer").await;
    wait_dc_open(&dc_b, "answerer").await;

    (pc_a, pc_b, dc_a, dc_b, signals_a, signals_b)
}

/// Primary Stage 1 exit criterion: a two-connection offer/answer/bidirectional
/// send/explicit close loop is stable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rtc_offer_answer_bidirectional_close_loop() {
    let (pc_a, pc_b, dc_a, dc_b, _signals_a, _signals_b) =
        build_connected_pair(16 * 1024 * 1024).await;

    // Bidirectional application send on the RpcReliable channel.
    dc_a.send_text("ping-from-a").await.expect("A -> B send");
    assert_eq!(recv_text(&dc_b, "answerer").await, "ping-from-a");

    dc_b.send_text("pong-from-b").await.expect("B -> A send");
    assert_eq!(recv_text(&dc_a, "offerer").await, "pong-from-b");

    // Binary payload too, exercising `send`/`BytesMut` rather than `send_text`.
    dc_a.send(BytesMut::from(&b"\x01\x02\x03"[..]))
        .await
        .expect("A -> B binary send");
    let binary = timeout(OP_TIMEOUT, async {
        loop {
            match dc_b.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => return msg.data.to_vec(),
                Some(_) => continue,
                None => panic!("binary message stream ended early"),
            }
        }
    })
    .await
    .expect("timed out waiting for binary message");
    assert_eq!(binary, vec![0x01, 0x02, 0x03]);

    // Explicit close on both connections; this is what drives task/socket
    // convergence in RTC (the driver handle is joined/aborted inside close()).
    pc_a.close().await.expect("close peer connection A");
    pc_b.close().await.expect("close peer connection B");

    // RTC's `close()` sets the connection's `closing` flag synchronously and
    // aborts the driver rather than running a graceful per-channel SCTP shutdown,
    // so a `poll()` may never observe `OnClose` — the deterministic, observable
    // convergence signal is that the send/back-pressure paths now fail fast with
    // `ErrDataChannelClosed` (the driver no longer drains the channel).
    let send_after_close = dc_a.send_text("must-fail").await;
    assert!(
        matches!(
            send_after_close,
            Err(webrtc::error::Error::ErrDataChannelClosed)
        ),
        "send on a closed connection must fail with ErrDataChannelClosed, got {send_after_close:?}"
    );
    let writable_after_close = dc_a.writable().await;
    assert!(
        matches!(
            writable_after_close,
            Err(webrtc::error::Error::ErrDataChannelClosed)
        ),
        "writable on a closed connection must fail with ErrDataChannelClosed, got {writable_after_close:?}"
    );
}

/// Stage 1 exit criterion #7: DataChannel back-pressure basics —
/// `outstanding_bytes`, `try_send` (fast-fail on a full buffer) and `writable`
/// (await capacity) against a configured send-buffer limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rtc_datachannel_backpressure_basics() {
    // A small per-channel send-buffer limit so the test can fill it.
    const SEND_LIMIT: usize = 8 * 1024;
    let (_pc_a, _pc_b, dc_a, dc_b, _sa, _sb) = build_connected_pair(SEND_LIMIT).await;

    // Below the limit the channel is writable and reports no/little outstanding
    // data; `try_send` succeeds.
    dc_a.writable().await.expect("channel writable when empty");
    dc_a.try_send(BytesMut::from(&b"warmup"[..]))
        .await
        .expect("try_send succeeds with room");

    // Fill the buffer faster than the loopback peer can drain it. We do not
    // poll `dc_b` here, so once the peer's receive window fills it stops
    // acking and our outstanding bytes climb to the limit.
    let chunk = vec![0xABu8; 1024];
    let mut saw_buffer_full = false;
    let mut saw_outstanding = false;
    for _ in 0..4096 {
        if dc_a
            .outstanding_bytes()
            .await
            .expect("read outstanding_bytes")
            > 0
        {
            saw_outstanding = true;
        }
        match dc_a.try_send(BytesMut::from(&chunk[..])).await {
            Ok(()) => continue,
            Err(webrtc::error::Error::ErrSendBufferFull) => {
                saw_buffer_full = true;
                break;
            }
            Err(e) => panic!("unexpected try_send error: {e:?}"),
        }
    }
    assert!(
        saw_outstanding,
        "outstanding_bytes should report in-flight send data"
    );
    assert!(
        saw_buffer_full,
        "try_send must fail fast with ErrSendBufferFull once the {SEND_LIMIT}-byte buffer fills"
    );

    // `writable` must not resolve while the buffer is pinned full; drain the
    // peer and it should make progress again. We bound the wait so the test
    // cannot hang: start draining, then expect writable to resolve.
    let drain = tokio::spawn(async move {
        // Consume everything the answerer received so its receive window opens
        // up and the offerer's outstanding bytes are acked.
        let mut drained = 0usize;
        while drained < 4096 {
            match timeout(OP_TIMEOUT, dc_b.poll()).await {
                Ok(Some(DataChannelEvent::OnMessage(_))) => drained += 1,
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        drained
    });

    timeout(OP_TIMEOUT, dc_a.writable())
        .await
        .expect("writable did not resolve after draining the peer")
        .expect("writable returned an error");
    let drained = drain.await.expect("drain task panicked");
    assert!(
        drained > 0,
        "the answerer should have drained some messages"
    );

    // Explicit close so this test also converges its tasks/sockets.
    _pc_a.close().await.expect("close A");
    _pc_b.close().await.expect("close B");
}

/// The handler mailbox is non-blocking: `try_publish` never waits for capacity
/// and instead counts overflow (RFC hard rule). Deterministic, no network.
#[tokio::test]
async fn mailbox_try_publish_is_nonblocking_and_counts_overflow() {
    use super::events::peer_event_mailbox_with_capacity;

    let (tx, mut rx) = peer_event_mailbox_with_capacity(2);

    // Fill the mailbox to capacity; these must not block.
    for _ in 0..2 {
        tx.try_publish(PeerDriverEvent::NegotiationNeeded { generation: 7 });
    }
    assert_eq!(tx.overflow_count(), 0);

    // The next publish overflows: still non-blocking, but counted.
    tx.try_publish(PeerDriverEvent::NegotiationNeeded { generation: 7 });
    assert_eq!(tx.overflow_count(), 1);
    tx.try_publish(PeerDriverEvent::NegotiationNeeded { generation: 7 });
    assert_eq!(tx.overflow_count(), 2);

    // Draining one slot lets a subsequent publish succeed again.
    let first = rx
        .recv_structural()
        .await
        .expect("mailbox should have an event");
    assert_eq!(first.generation(), 7);
    assert_eq!(rx.overflow_count(), 2);
}

/// The handler is a pure, non-blocking translator: it stamps every event with
/// its generation and routes mergeable state to the coalescing watch while
/// forwarding structural facts to the bounded queue — without doing SDP/close/
/// hooks. Deterministic, no network (drives the trait methods directly).
#[tokio::test]
async fn event_handler_tags_generation_and_forwards_facts() {
    use super::events::peer_event_mailbox;
    use webrtc::peer_connection::PeerConnectionEventHandler;

    let (mb, mut rx) = peer_event_mailbox();
    let handler = WebRtcEventHandler::new(test_actor_id(42), 9, mb);

    // Drive all nine RTC callbacks directly (they are async trait methods).
    PeerConnectionEventHandler::on_connection_state_change(
        &handler,
        RTCPeerConnectionState::Connected,
    )
    .await;
    PeerConnectionEventHandler::on_ice_connection_state_change(
        &handler,
        RTCIceConnectionState::Connected,
    )
    .await;
    PeerConnectionEventHandler::on_ice_gathering_state_change(
        &handler,
        RTCIceGatheringState::Complete,
    )
    .await;
    PeerConnectionEventHandler::on_signaling_state_change(&handler, RTCSignalingState::Stable)
        .await;
    PeerConnectionEventHandler::on_ice_candidate(&handler, RTCPeerConnectionIceEvent::default())
        .await;
    PeerConnectionEventHandler::on_ice_candidate_error(
        &handler,
        RTCPeerConnectionIceErrorEvent {
            error_text: "candidate failed".to_string(),
            ..Default::default()
        },
    )
    .await;
    handler
        .on_test_data_channel(super::peer::effects::test_support::fake_data_channel(
            RPC_RELIABLE_LABEL,
        ))
        .await;
    PeerConnectionEventHandler::on_track(
        &handler,
        super::media::test_support::scripted_track("remote-video", "video/VP8", Vec::new()),
    )
    .await;
    PeerConnectionEventHandler::on_negotiation_needed(&handler).await;

    // Mergeable state events are coalesced into the latest-value state watch.
    let tick = rx.latest_state();
    assert_eq!(tick.generation, 9);
    assert_eq!(tick.pc_state, Some(RTCPeerConnectionState::Connected));
    assert_eq!(tick.ice_state, Some(RTCIceConnectionState::Connected));
    assert_eq!(tick.gathering_state, Some(RTCIceGatheringState::Complete));
    assert_eq!(tick.signaling_state, Some(RTCSignalingState::Stable));

    // Structural callbacks may finish their finite metadata normalization in a
    // different order. Collect all five mappings and prove that raw resources
    // remained in the generation registry rather than crossing the mailbox.
    let mut candidate = false;
    let mut candidate_error = false;
    let mut data_channel = false;
    let mut track = false;
    let mut negotiation = false;
    timeout(Duration::from_secs(2), async {
        while !(candidate && candidate_error && data_channel && track && negotiation) {
            match rx.recv_structural().await.expect("structural event") {
                PeerDriverEvent::LocalIceCandidate { generation: 9, .. } => candidate = true,
                PeerDriverEvent::IceCandidateError {
                    generation: 9,
                    error_text,
                } => {
                    assert_eq!(error_text, "candidate failed");
                    candidate_error = true;
                }
                PeerDriverEvent::DataChannelDiscovered {
                    generation: 9,
                    descriptor,
                    ..
                } => {
                    assert_eq!(descriptor.label, RPC_RELIABLE_LABEL);
                    assert!(!descriptor.negotiated);
                    data_channel = true;
                }
                PeerDriverEvent::TrackDiscovered {
                    generation: 9,
                    descriptor,
                    ..
                } => {
                    assert_eq!(descriptor.meta.track_id, "remote-video");
                    assert_eq!(descriptor.meta.codec, "VP8");
                    track = true;
                }
                PeerDriverEvent::NegotiationNeeded { generation: 9 } => negotiation = true,
                other => panic!("unexpected callback mapping: {other:?}"),
            }
        }
    })
    .await
    .expect("all nine callback mappings should arrive");
    assert_eq!(
        handler.resources().len(),
        2,
        "raw channel/track are tokenized"
    );
}

/// Mergeable state changes are coalesced to the latest value (O1): a burst of
/// intermediate PC states collapses to the newest, so a state storm cannot
/// accumulate in a queue. Deterministic, no network.
#[tokio::test]
async fn mergeable_state_events_coalesce_to_latest_value() {
    use super::events::peer_event_mailbox;

    let (mb, rx) = peer_event_mailbox();
    for state in [
        RTCPeerConnectionState::New,
        RTCPeerConnectionState::Connecting,
        RTCPeerConnectionState::Connected,
    ] {
        mb.try_publish(PeerDriverEvent::PeerConnectionStateChanged {
            generation: 3,
            state,
        });
    }

    // Only the latest state is retained; nothing queues up or overflows.
    let tick = rx.latest_state();
    assert_eq!(tick.pc_state, Some(RTCPeerConnectionState::Connected));
    assert_eq!(tick.seq, 3, "each publish advanced the coalesced sequence");
    assert_eq!(rx.overflow_count(), 0, "state events never touch the queue");
}

/// A closing old PC and a newly-built PC share one peer mailbox, but never one
/// coalesced state tick. In particular, an old `Failed`/`Closed` field cannot be
/// re-tagged as the new generation by a signaling/gathering callback.
#[tokio::test]
async fn mergeable_state_is_isolated_per_generation() {
    use super::events::peer_event_mailbox;

    let (mb, rx) = peer_event_mailbox();
    mb.try_publish(PeerDriverEvent::PeerConnectionStateChanged {
        generation: 7,
        state: RTCPeerConnectionState::Failed,
    });
    mb.try_publish(PeerDriverEvent::IceConnectionStateChanged {
        generation: 7,
        state: RTCIceConnectionState::Failed,
    });
    mb.try_publish(PeerDriverEvent::SignalingStateChanged {
        generation: 8,
        state: RTCSignalingState::HaveLocalOffer,
    });

    let fresh = rx
        .latest_state_for_generation(8)
        .expect("fresh generation tick");
    assert_eq!(
        fresh.signaling_state,
        Some(RTCSignalingState::HaveLocalOffer)
    );
    assert_eq!(
        fresh.pc_state, None,
        "old PC Failed must not cross generation"
    );
    assert_eq!(
        fresh.ice_state, None,
        "old ICE Failed must not cross generation"
    );

    // A late close callback from generation 7 updates only generation 7 and
    // cannot overwrite the already-retained facts of generation 8.
    mb.try_publish(PeerDriverEvent::PeerConnectionStateChanged {
        generation: 7,
        state: RTCPeerConnectionState::Closed,
    });
    let fresh_after_old_close = rx
        .latest_state_for_generation(8)
        .expect("fresh generation remains retained");
    assert_eq!(fresh_after_old_close.signaling_state, fresh.signaling_state);
    assert_eq!(fresh_after_old_close.pc_state, None);
    assert_eq!(fresh_after_old_close.ice_state, None);
}

/// Structural overflow is a health fact for the generation that lost the
/// event, not for whichever generation happens to be active when the
/// controller next polls the mailbox.
#[tokio::test]
async fn structural_overflow_is_attributed_to_its_generation() {
    use super::events::peer_event_mailbox_with_capacity;

    let (mb, rx) = peer_event_mailbox_with_capacity(1);
    mb.try_publish(PeerDriverEvent::NegotiationNeeded { generation: 7 });
    mb.try_publish(PeerDriverEvent::NegotiationNeeded { generation: 8 });

    assert_eq!(rx.overflow_count(), 1);
    assert_eq!(
        rx.latest_state_for_generation(8)
            .expect("overflow generation tick")
            .overflow_count,
        1
    );
    assert!(
        rx.latest_state_for_generation(7)
            .is_none_or(|tick| tick.overflow_count == 0),
        "the queued old generation must not inherit generation 8 overflow"
    );
}

/// S6-7b: the factory registers the default codec set + interceptor registry
/// (webrtc 0.20 auto-registers neither). A video transceiver's offer SDP must
/// carry rtpmap codec lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn factory_registers_default_codecs_and_interceptors() {
    let factory = loopback_factory(RtcRuntimeConfig::desktop_default());
    let (mb, _rx) = crate::wire::webrtc::rtc::events::peer_event_mailbox();
    let handler = Arc::new(WebRtcEventHandler::new(test_actor_id(1), 1, mb));
    let pc = factory.build(handler).await.expect("build pc");

    pc.add_transceiver_from_kind(rtc::rtp_transceiver::rtp_sender::RtpCodecKind::Video, None)
        .await
        .expect("add video transceiver");
    let offer = pc.create_offer(None).await.expect("create offer");

    assert!(
        offer.sdp.contains("a=rtpmap:"),
        "offer SDP must carry codec rtpmap lines (got none):\n{}",
        offer.sdp
    );
    assert!(
        offer.sdp.contains("VP8") || offer.sdp.contains("H264"),
        "default codec set should include VP8/H264:\n{}",
        offer.sdp
    );
    pc.close().await.expect("close");
}
