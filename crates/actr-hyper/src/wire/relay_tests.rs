use super::*;
use crate::inbound::DataChunkRegistry;
use crate::lifecycle::{node::PeerTypes, session_state::SessionSnapshot};
use crate::transport::correlation::RpcCorrelation;
use actr_relay_security::{
    CERTIFICATE_DOMAIN, Certificate, IdentityClaims, IssuedIdentity, signing_bytes,
};
use actr_runtime_mailbox::{Mailbox, MailboxStats, MessagePriority, MessageRecord, StorageResult};
use ed25519_dalek::{Signer, SigningKey};
use uuid::Uuid;

struct EmptyMailbox;

#[tokio::test]
async fn unavailable_platform_signer_never_sends_relay_identity_request() {
    struct UnavailableSigner;
    impl crate::RealmKeySigner for UnavailableSigner {
        fn public_key(&self) -> [u8; 32] {
            SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes()
        }
        fn sign_payload(&self, _: &[u8]) -> Result<[u8; 64], crate::HyperError> {
            Err(crate::HyperError::Config(
                "sensitive platform reason".into(),
            ))
        }
    }
    let mut server = mockito::Server::new_async().await;
    let no_request = server
        .mock("POST", "/relay-identity")
        .expect(0)
        .create_async()
        .await;
    let fixture = Fixture::new().await;
    let key = crate::RealmClientKey::from_signer(
        fixture.local.realm.clone(),
        "provider.example".into(),
        Arc::new(UnavailableSigner),
    );
    let client = RelayClient::new(
        "ws://unused".into(),
        server.url(),
        Some(key),
        fixture.client.session.clone(),
        fixture.client.gate.clone(),
        fixture.shutdown.clone(),
    );
    let error = client
        .certified_identity(&fixture.local.to_string_repr(), b"fixture renewal proof")
        .await
        .err()
        .expect("an unavailable signer must fail closed");
    assert!(matches!(error, NetworkError::ConnectionError(message)
        if message == "Realm client signer unavailable"));
    no_request.assert_async().await;
}

#[async_trait]
impl Mailbox for EmptyMailbox {
    async fn enqueue(
        &self,
        _: Vec<u8>,
        _: Option<Vec<u8>>,
        _: Vec<u8>,
        _: MessagePriority,
    ) -> StorageResult<Uuid> {
        Ok(Uuid::new_v4())
    }
    async fn dequeue(&self) -> StorageResult<Vec<MessageRecord>> {
        Ok(vec![])
    }
    async fn ack(&self, _: Uuid) -> StorageResult<()> {
        Ok(())
    }
    async fn status(&self) -> StorageResult<MailboxStats> {
        Ok(MailboxStats {
            queued_messages: 0,
            inflight_messages: 0,
            queued_by_priority: Default::default(),
        })
    }
}

fn identity(endpoint: &str, now: u64) -> LocalIdentity {
    let key = IdentityKey::generate().unwrap();
    let signer = SigningKey::from_bytes(&[7; 32]);
    let claims = serde_json::to_vec(&IdentityClaims {
        domain: CERTIFICATE_DOMAIN.into(),
        endpoint: endpoint.into(),
        actor_type: "example/Client@1.0.0".into(),
        client_key: "fixture".into(),
        public_key: key.public,
        admission_commitment: key.admission_commitment(),
        expires_at: now + 120,
    })
    .unwrap();
    let signature = signer.sign(&signing_bytes(&claims)).to_bytes().to_vec();
    let issued = IssuedIdentity {
        certificate: Certificate {
            key_id: 1,
            claims,
            signature,
        },
        issuer: IssuerKey {
            key_id: 1,
            public_key: *signer.verifying_key().as_bytes(),
            expires_at: now + 600,
        },
    };
    key.certify(issued, endpoint, now).unwrap()
}

struct Fixture {
    client: Arc<RelayClient>,
    transport: Transport,
    captured: mpsc::UnboundedReceiver<Message>,
    local: ActrId,
    remote: ActrId,
    remote_identity: LocalIdentity,
    issuer: IssuerKey,
    now: u64,
    shutdown: CancellationToken,
}

impl Fixture {
    async fn new() -> Self {
        Self::new_with_mailbox(true).await
    }

    async fn new_with_mailbox(publish_mailbox: bool) -> Self {
        let now = now_secs();
        let local = ActrId::from_string_repr("2.test@example.com").unwrap();
        let remote = ActrId::from_string_repr("1.test@example.com").unwrap();
        let local_identity = identity(&local.to_string_repr(), now);
        let remote_identity = identity(&remote.to_string_repr(), now);
        let issuer = local_identity.issued.issuer.clone();
        let gate = Arc::new(WebSocketGate::new_relay(
            Arc::new(RpcCorrelation::new()),
            Arc::new(DataChunkRegistry::new()),
            PeerTypes::default(),
        ));
        if publish_mailbox {
            gate.start_receive_loop(Arc::new(EmptyMailbox))
                .await
                .unwrap();
        }
        let session = SessionState::new(SessionSnapshot::empty_with_id(local.clone(), 1));
        let shutdown = CancellationToken::new();
        let client = RelayClient::new(
            "ws://unused".into(),
            "http://unused".into(),
            None,
            session,
            gate,
            shutdown.clone(),
        );
        client.issuers.write().await.insert(1, issuer.clone());
        let (tx, mut outgoing) =
            mpsc::channel::<Outgoing>(OUTGOING_QUEUE_SLOTS + CLOSE_QUEUE_SLOTS);
        let (captured_tx, captured) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(out) = outgoing.recv().await {
                let Outgoing { message, ack, .. } = out;
                if captured_tx.send(message).is_err() {
                    break;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(Ok(()));
                }
            }
        });
        let transport = Transport {
            tx,
            budget: Arc::new(Semaphore::new(QUEUE_BYTES)),
            outgoing_slots: Arc::new(Semaphore::new(OUTGOING_QUEUE_SLOTS)),
            close_slots: Arc::new(Semaphore::new(CLOSE_QUEUE_SLOTS)),
            pending_closes: Arc::new(std::sync::Mutex::new(HashSet::new())),
            encryption_budget: Arc::new(Semaphore::new(QUEUE_BYTES)),
            cancel: shutdown.child_token(),
            identity: Arc::new(local_identity),
        };
        client.status.send_replace(Status::Ready(transport.clone()));
        Self {
            client,
            transport,
            captured,
            local,
            remote,
            remote_identity,
            issuer,
            now,
            shutdown,
        }
    }

    async fn next_message(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(3), self.captured.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn next_binary(&mut self) -> Bytes {
        self.next_binary_on(42).await
    }

    async fn next_binary_on(&mut self, expected_channel: u64) -> Bytes {
        let message = self.next_message().await;
        let Message::Binary(frame) = message else {
            panic!("expected binary frame: {message:?}");
        };
        let (channel, payload) = decode_data(frame, MAX_FRAME).unwrap();
        assert_eq!(channel, expected_channel);
        payload
    }

    async fn start_dial(
        &mut self,
        target: ActrId,
    ) -> (
        u64,
        tokio::task::JoinHandle<NetworkResult<(u64, WebSocketConnection)>>,
    ) {
        let client = self.client.clone();
        let expected = target.to_string_repr();
        let dial = tokio::spawn(async move { client.connect_peer(target).await });
        let Message::Text(text) = self.next_message().await else {
            panic!("expected connect control");
        };
        let ClientControl::Connect { request, peer } = serde_json::from_str(&text).unwrap() else {
            panic!("expected connect control");
        };
        assert_eq!(peer, expected);
        (request, dial)
    }

    async fn connected(&self, request: Option<u64>) -> NetworkResult<()> {
        self.client
            .handle_control(
                ServerControl::Connected {
                    channel: 42,
                    request,
                    peer: self.remote.to_string_repr(),
                    metadata: String::new(),
                },
                &self.transport,
            )
            .await
    }

    async fn handshake(&mut self) -> SecureSession {
        self.handshake_on(42).await
    }

    async fn handshake_on(&mut self, channel: u64) -> SecureSession {
        let budget = Arc::new(Semaphore::new(QUEUE_BYTES));
        let mut remote = SecureSession::new(
            &self.remote_identity,
            &self.local.to_string_repr(),
            channel,
            self.now,
        )
        .unwrap();
        let first = remote.start().unwrap().unwrap();
        self.client
            .receive_secure(channel, Bytes::from(first), &self.transport, &budget)
            .await
            .unwrap();
        let second = self.next_binary_on(channel).await;
        let third = remote
            .receive(&second, Some(&self.issuer), self.now)
            .unwrap()
            .outbound
            .pop()
            .unwrap();
        self.client
            .receive_secure(channel, Bytes::from(third), &self.transport, &budget)
            .await
            .unwrap();
        let fourth = self.next_binary_on(channel).await;
        let fifth = remote
            .receive(&fourth, None, self.now)
            .unwrap()
            .outbound
            .pop()
            .unwrap();
        self.client
            .receive_secure(channel, Bytes::from(fifth), &self.transport, &budget)
            .await
            .unwrap();
        remote
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[tokio::test]
async fn connected_response_cannot_replace_the_requested_peer() {
    let mut fixture = Fixture::new().await;
    let (first_request, first_dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(first_request)).await.unwrap();
    let mut remote = fixture.handshake().await;
    let (_, connection) = first_dial.await.unwrap().unwrap();

    let target = ActrId::from_string_repr("3.test@example.com").unwrap();
    let (request, dial) = fixture.start_dial(target).await;
    fixture.connected(Some(request)).await.unwrap();
    assert!(dial.await.unwrap().is_err());
    assert!(!fixture.transport.cancel.is_cancelled());
    assert!(connection.is_connected());
    assert!(fixture.client.channels.read().await.contains_key(&42));

    let lane = connection
        .get_lane(PayloadType::StreamReliable)
        .await
        .unwrap();
    lane.send(Bytes::from_static(b"unrelated channel survives"))
        .await
        .unwrap();
    let ciphertext = fixture.next_binary().await;
    let plaintext = remote
        .receive(&ciphertext, None, fixture.now)
        .unwrap()
        .application
        .unwrap();
    assert!(plaintext.ends_with(b"unrelated channel survives"));

    // Even a response naming the requested peer must not reuse a channel
    // that is already bound to a different, still-connected peer.
    let other = ActrId::from_string_repr("4.test@example.com").unwrap();
    let (request, dial) = fixture.start_dial(other.clone()).await;
    fixture
        .client
        .handle_control(
            ServerControl::Connected {
                channel: 42,
                request: Some(request),
                peer: other.to_string_repr(),
                metadata: String::new(),
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    assert!(dial.await.unwrap().is_err());
    assert!(connection.is_connected());
    assert!(!fixture.transport.cancel.is_cancelled());
}

#[tokio::test]
async fn channel_close_survives_exhausted_data_budget_and_queue() {
    let mut fixture = Fixture::new().await;
    let budget = fixture
        .transport
        .budget
        .clone()
        .acquire_many_owned(QUEUE_BYTES as u32)
        .await
        .unwrap();
    let slots = fixture
        .transport
        .outgoing_slots
        .clone()
        .acquire_many_owned(OUTGOING_QUEUE_SLOTS as u32)
        .await
        .unwrap();

    fixture.transport.try_close(42);
    let Message::Text(text) = fixture.next_message().await else {
        panic!("expected a channel close control");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&text).unwrap(),
        ClientControl::Close { channel: 42 }
    ));
    assert!(!fixture.transport.cancel.is_cancelled());

    drop(slots);
    drop(budget);
    fixture
        .transport
        .control(ClientControl::Connect {
            request: 7,
            peer: fixture.remote.to_string_repr(),
        })
        .await
        .unwrap();
    let Message::Text(text) = fixture.next_message().await else {
        panic!("expected another peer's connect control");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&text).unwrap(),
        ClientControl::Connect { request: 7, .. }
    ));
}

#[tokio::test]
async fn closing_unverified_channel_interrupts_its_waiting_sink() {
    let mut fixture = Fixture::new().await;
    let entry = fixture
        .client
        .install(42, fixture.remote.to_string_repr(), &fixture.transport)
        .await
        .unwrap();
    let sink = entry.conn.sink();
    let (started_tx, started_rx) = oneshot::channel();
    let sending = tokio::spawn(async move {
        let mut sink = sink.lock().await;
        let _ = started_tx.send(());
        sink.as_mut()
            .unwrap()
            .send(Message::Binary(Bytes::from_static(b"waiting")))
            .await
    });
    started_rx.await.unwrap();
    tokio::task::yield_now().await;

    tokio::time::timeout(Duration::from_secs(1), entry.conn.close())
        .await
        .expect("closing must interrupt the unverified sink")
        .unwrap();
    assert!(sending.await.unwrap().is_err());
    let Message::Text(text) = fixture.next_message().await else {
        panic!("expected a channel close control");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&text).unwrap(),
        ClientControl::Close { channel: 42 }
    ));
    assert!(!fixture.transport.cancel.is_cancelled());
}

#[tokio::test]
async fn shutdown_interrupts_handshake_waiting_for_ingress_mailbox() {
    let mut fixture = Fixture::new_with_mailbox(false).await;
    fixture.connected(None).await.unwrap();
    let budget = Arc::new(Semaphore::new(QUEUE_BYTES));
    let mut remote = SecureSession::new(
        &fixture.remote_identity,
        &fixture.local.to_string_repr(),
        42,
        fixture.now,
    )
    .unwrap();
    let first = remote.start().unwrap().unwrap();
    fixture
        .client
        .receive_secure(42, Bytes::from(first), &fixture.transport, &budget)
        .await
        .unwrap();
    let second = fixture.next_binary().await;
    let third = remote
        .receive(&second, Some(&fixture.issuer), fixture.now)
        .unwrap()
        .outbound
        .pop()
        .unwrap();
    fixture
        .client
        .receive_secure(42, Bytes::from(third), &fixture.transport, &budget)
        .await
        .unwrap();
    let fourth = fixture.next_binary().await;
    let fifth = remote
        .receive(&fourth, None, fixture.now)
        .unwrap()
        .outbound
        .pop()
        .unwrap();
    let client = fixture.client.clone();
    let transport = fixture.transport.clone();
    let receiving = tokio::spawn(async move {
        client
            .receive_secure(42, Bytes::from(fifth), &transport, &budget)
            .await
    });
    let entry = fixture.client.channels.read().await[&42].clone();
    tokio::time::timeout(Duration::from_secs(1), async {
        while entry.conn.secure_channel_state() != SecureChannelState::Verified {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handshake should reach the mailbox attachment");
    assert!(!receiving.is_finished());
    fixture.shutdown.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), receiving)
        .await
        .expect("shutdown must interrupt the mailbox wait")
        .unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn matching_outbound_peer_completes_handshake_and_carries_data() {
    let mut fixture = Fixture::new().await;
    let (request, dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let mut remote = fixture.handshake().await;
    let (_, connection) = tokio::time::timeout(Duration::from_secs(3), dial)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let lane = connection
        .get_lane(PayloadType::StreamReliable)
        .await
        .unwrap();
    lane.send(Bytes::from_static(b"valid peer")).await.unwrap();
    let ciphertext = fixture.next_binary().await;
    let plaintext = remote
        .receive(&ciphertext, None, fixture.now)
        .unwrap()
        .application
        .unwrap();
    assert!(plaintext.ends_with(b"valid peer"));

    // An empty lane payload still carries the five-byte transport header.
    lane.send(Bytes::new()).await.unwrap();
    let ciphertext = fixture.next_binary().await;
    let plaintext = remote
        .receive(&ciphertext, None, fixture.now)
        .unwrap()
        .application
        .unwrap();
    assert_eq!(plaintext, [PayloadType::StreamReliable as u8, 0, 0, 0, 0]);
    assert!(connection.is_connected());
}

#[tokio::test]
async fn reused_ready_channel_completes_the_pending_dial() {
    let mut fixture = Fixture::new().await;
    let (request, dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture
        .client
        .handle_control(
            ServerControl::Incoming {
                channel: 42,
                peer: fixture.remote.to_string_repr(),
                metadata: String::new(),
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    let Message::Text(text) = fixture.next_message().await else {
        panic!("expected accept control");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&text).unwrap(),
        ClientControl::Accept { channel: 42 }
    ));
    fixture.connected(None).await.unwrap();
    let _remote = fixture.handshake().await;
    assert!(*fixture.client.channels.read().await[&42].ready.borrow());
    fixture.connected(Some(request)).await.unwrap();
    let (id, _) = tokio::time::timeout(Duration::from_secs(3), dial)
        .await
        .expect("reused ready channel must resolve the pending dial")
        .unwrap()
        .unwrap();
    assert_eq!(id, 42);
}

#[tokio::test]
async fn reconnect_replaces_an_accepted_channel_while_its_sink_is_closing() {
    let mut fixture = Fixture::new().await;
    let (request, first_dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let _remote = fixture.handshake().await;
    let (_, connection) = first_dial.await.unwrap().unwrap();
    let sink = connection.sink();
    let sink_guard = sink.lock().await;
    let closing_connection = connection.clone();
    let closing = tokio::spawn(async move { closing_connection.close().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while connection.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let (request, redial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let Message::Text(close) = fixture.next_message().await else {
        panic!("expected stale channel close");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&close).unwrap(),
        ClientControl::Close { channel: 42 }
    ));
    let Message::Text(retry) = fixture.next_message().await else {
        panic!("expected connect retry");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&retry).unwrap(),
        ClientControl::Connect { request: id, peer } if id == request && peer == fixture.remote.to_string_repr()
    ));
    drop(sink_guard);
    closing.await.unwrap().unwrap();
    fixture
        .client
        .handle_control(
            ServerControl::Connected {
                channel: 43,
                request: Some(request),
                peer: fixture.remote.to_string_repr(),
                metadata: String::new(),
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    let _new_remote = fixture.handshake_on(43).await;
    let (id, replacement) = tokio::time::timeout(Duration::from_secs(3), redial)
        .await
        .expect("replacement dial must complete")
        .unwrap()
        .unwrap();
    assert_eq!(id, 43);
    assert!(replacement.is_connected());
    fixture
        .client
        .handle_control(
            ServerControl::Closed {
                channel: 42,
                request: None,
                reason: loran_relay_protocol::CloseReason::Closed,
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.client.peers.read().await.get(&fixture.remote),
        Some(&43)
    );
}

#[tokio::test]
async fn stale_handshake_retry_keeps_the_original_pending_dial() {
    let mut fixture = Fixture::new().await;
    let (request, dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let connection = fixture.client.channels.read().await[&42].conn.clone();
    let sink = connection.sink();
    let sink_guard = sink.lock().await;
    let closing_connection = connection.clone();
    let closing = tokio::spawn(async move { closing_connection.close().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while connection.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.connected(Some(request)).await.unwrap();
    let Message::Text(close) = fixture.next_message().await else {
        panic!("expected stale channel close");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&close).unwrap(),
        ClientControl::Close { channel: 42 }
    ));
    let Message::Text(retry) = fixture.next_message().await else {
        panic!("expected connect retry");
    };
    assert!(matches!(
        serde_json::from_str::<ClientControl>(&retry).unwrap(),
        ClientControl::Connect { request: id, .. } if id == request
    ));
    drop(sink_guard);
    closing.await.unwrap().unwrap();
    fixture
        .client
        .handle_control(
            ServerControl::Connected {
                channel: 43,
                request: Some(request),
                peer: fixture.remote.to_string_repr(),
                metadata: String::new(),
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    let _remote = fixture.handshake_on(43).await;
    let (id, replacement) = tokio::time::timeout(Duration::from_secs(3), dial)
        .await
        .expect("replacement dial must complete")
        .unwrap()
        .unwrap();
    assert_eq!(id, 43);
    assert!(replacement.is_connected());
}

#[tokio::test]
async fn relay_wire_status_waits_for_a_contended_connection_lock() {
    let mut fixture = Fixture::new().await;
    let (request, dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let _remote = fixture.handshake().await;
    let (_, connection) = dial.await.unwrap().unwrap();
    let expected_epoch = connection.secure_epoch();
    let wire = Arc::new(RelayWire {
        client: fixture.client.clone(),
        peer: fixture.remote.clone(),
        generation: 1,
        connection: SyncRwLock::new(Some(connection)),
    });
    let guard = wire.connection.write().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let reader = wire.clone();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        result_tx
            .send((
                reader.is_connected(),
                reader.secure_channel_state(),
                reader.secure_epoch(),
            ))
            .unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    drop(guard);
    let (connected, state, epoch) = result_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(connected);
    assert_eq!(state, SecureChannelState::Verified);
    assert_eq!(epoch, expected_epoch);
    thread.join().unwrap();
}

#[tokio::test]
async fn closing_a_relay_wire_preserves_a_concurrent_replacement() {
    let mut fixture = Fixture::new().await;
    let (request, dial) = fixture.start_dial(fixture.remote.clone()).await;
    fixture.connected(Some(request)).await.unwrap();
    let _remote = fixture.handshake().await;
    let (_, connection) = dial.await.unwrap().unwrap();
    let replacement = fixture
        .client
        .install(43, fixture.remote.to_string_repr(), &fixture.transport)
        .await
        .unwrap()
        .conn
        .clone();
    let sink = connection.sink();
    let sink_guard = sink.lock().await;
    let wire = Arc::new(RelayWire {
        client: fixture.client.clone(),
        peer: fixture.remote.clone(),
        generation: 1,
        connection: SyncRwLock::new(Some(connection.clone())),
    });
    let closing_wire = wire.clone();
    let closing = tokio::spawn(async move { closing_wire.close().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while connection.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    *wire.connection.write().unwrap() = Some(replacement.clone());
    drop(sink_guard);
    closing.await.unwrap().unwrap();
    assert!(wire.is_connected());
    assert!(Arc::ptr_eq(
        &wire.connection.read().unwrap().as_ref().unwrap().sink(),
        &replacement.sink()
    ));
}

#[tokio::test]
async fn cancelled_request_does_not_break_an_existing_incoming_channel() {
    let mut fixture = Fixture::new().await;
    fixture
        .client
        .handle_control(
            ServerControl::Incoming {
                channel: 42,
                peer: fixture.remote.to_string_repr(),
                metadata: String::new(),
            },
            &fixture.transport,
        )
        .await
        .unwrap();
    let _accept = fixture.next_message().await;
    fixture.connected(Some(999)).await.unwrap();
    let _remote = fixture.handshake().await;
    assert!(*fixture.client.channels.read().await[&42].ready.borrow());
}

#[tokio::test]
async fn historical_failed_peers_do_not_exhaust_capacity() {
    let fixture = Fixture::new().await;
    fixture.shutdown.cancel();
    for serial in 1..=MAX_DESIRED_PEERS + 1 {
        let peer = ActrId {
            realm: "test@example.com".into(),
            serial_number: serial as u64 + 100,
        };
        let error = fixture.client.connect_peer(peer).await.unwrap_err();
        assert!(!error.to_string().contains("capacity exhausted"));
    }
    assert_eq!(fixture.client.desired.lock().await.len(), MAX_DESIRED_PEERS);
    assert!(fixture.client.peer_locks.lock().await.len() <= 1);
}

#[tokio::test]
async fn capacity_eviction_preserves_active_and_dialing_peers() {
    let fixture = Fixture::new().await;
    let peers: Vec<_> = (0..=MAX_DESIRED_PEERS)
        .map(|serial| ActrId {
            realm: "test@example.com".into(),
            serial_number: serial as u64 + 100,
        })
        .collect();
    for peer in &peers[..MAX_DESIRED_PEERS] {
        fixture.client.remember_peer(peer).await.unwrap();
    }
    fixture
        .client
        .peers
        .write()
        .await
        .insert(peers[0].clone(), 42);
    let dialing = Arc::new(Mutex::new(()));
    fixture
        .client
        .peer_locks
        .lock()
        .await
        .insert(peers[1].clone(), Arc::downgrade(&dialing));

    fixture
        .client
        .remember_peer(&peers[MAX_DESIRED_PEERS])
        .await
        .unwrap();
    let desired = fixture.client.desired.lock().await;
    assert!(desired.contains(&peers[0]));
    assert!(desired.contains(&peers[1]));
    assert!(!desired.contains(&peers[2]));
}
