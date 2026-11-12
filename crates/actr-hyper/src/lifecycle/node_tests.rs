use super::*;

#[tokio::test]
async fn inv20_empty_mailbox_keeps_driving_inflight_reply_and_ack_tails() {
    let shutdown = CancellationToken::new();
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut inflight = futures_util::stream::FuturesUnordered::<MailboxTail>::new();
    inflight.push(Box::pin({
        let completed = Arc::clone(&completed);
        async move {
            completed.store(true, std::sync::atomic::Ordering::Release);
        }
    }));

    let outcome = tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_mailbox_idle(&shutdown, &mut inflight, std::future::pending()),
    )
    .await
    .expect("an in-flight reply/ack tail must beat an idle mailbox wake");

    assert_eq!(outcome, MailboxIdleOutcome::Progress);
    assert!(
        completed.load(std::sync::atomic::Ordering::Acquire),
        "empty durable storage must not starve an admitted reply/ack tail"
    );
    assert!(inflight.is_empty());
}

#[tokio::test]
async fn empty_mailbox_idle_wait_honors_pre_cancelled_shutdown() {
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let mut inflight = futures_util::stream::FuturesUnordered::<MailboxTail>::new();

    let outcome =
        wait_for_mailbox_idle(&shutdown, &mut inflight, std::future::pending::<()>()).await;

    assert_eq!(outcome, MailboxIdleOutcome::Shutdown);
}

#[tokio::test]
async fn empty_mailbox_idle_wait_honors_ready_enqueue_wake() {
    let shutdown = CancellationToken::new();
    let mut inflight = futures_util::stream::FuturesUnordered::<MailboxTail>::new();

    let outcome = wait_for_mailbox_idle(&shutdown, &mut inflight, std::future::ready(())).await;

    assert_eq!(outcome, MailboxIdleOutcome::Progress);
}

#[tokio::test(start_paused = true)]
async fn sqlite_enqueue_observer_wakes_idle_dequeue_without_poll_delay() {
    use actr_runtime_mailbox::{Mailbox as _, MessagePriority, SqliteMailbox};

    let mailbox = SqliteMailbox::new(":memory:")
        .await
        .expect("in-memory mailbox should open");
    let wake = Arc::new(tokio::sync::Notify::new());
    let observed_depth = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_depth_for_callback = Arc::clone(&observed_depth);
    let observer = Arc::new(EnqueueObserver {
        fire: Arc::new(move |depth| {
            observed_depth_for_callback.store(depth, std::sync::atomic::Ordering::Release);
        }),
        wake: Arc::clone(&wake),
    });
    assert!(
        mailbox.set_depth_observer(observer),
        "SQLite must select the production push-notification path"
    );

    // Match the production subscribe-before-dequeue ordering that closes the
    // empty-dequeue/enqueue race.
    let mut enqueued = Box::pin(Arc::clone(&wake).notified_owned());
    enqueued.as_mut().enable();
    let before = tokio::time::Instant::now();
    mailbox
        .enqueue(vec![1], None, vec![2], MessagePriority::Normal)
        .await
        .expect("test message should enqueue");

    mailbox_empty_wake(true, enqueued).await;

    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "push wake must not consume the 10 ms compatibility poll delay"
    );
    assert_eq!(
        observed_depth.load(std::sync::atomic::Ordering::Acquire),
        1,
        "the same observer must feed the backpressure depth callback"
    );
    assert_eq!(
        mailbox
            .dequeue()
            .await
            .expect("test dequeue should succeed")
            .len(),
        1,
        "work must be visible immediately after the push wake"
    );
}

#[tokio::test(start_paused = true)]
async fn mailbox_without_observer_uses_compatibility_poll_delay() {
    let before = tokio::time::Instant::now();

    mailbox_empty_wake(false, std::future::ready(())).await;

    assert_eq!(
        tokio::time::Instant::now().duration_since(before),
        Duration::from_millis(10),
        "only backends that decline the observer should take the polling branch"
    );
}

// ── Strategy A: keyless zero-overhead scheduler gate ────────────────────────
//
// `scheduler_engaged` is the single predicate the node uses to decide whether
// to spawn the interleaved runner + conflict-key scheduler. Strategy A's whole
// promise lives here: default-on (gate on) with NO declared key must NOT engage
// the scheduler, so a keyless actor stays bit-for-bit the serial M4 path.

#[test]
fn scheduler_engaged_only_with_gate_on_and_declared_keys() {
    // Default-on + keyless ⇒ NOT engaged (the strategy A zero-overhead promise).
    assert!(
        !scheduler_engaged(true, false),
        "keyless actor must stay serial (no scheduler) even with the gate on"
    );
    // Gate on + at least one key ⇒ engaged.
    assert!(
        scheduler_engaged(true, true),
        "a declared conflict key with the gate on must engage the scheduler"
    );
    // Gate off ⇒ never engaged regardless of keys.
    assert!(!scheduler_engaged(false, false));
    assert!(!scheduler_engaged(false, true));
}

#[test]
fn empty_conflict_key_spec_is_reported_empty() {
    // The node's keyless predicate rides on `ConflictKeySpec::is_empty`.
    let keyless = crate::dispatch::ConflictKeySpec::builder()
        .build()
        .expect("build empty spec");
    assert!(keyless.is_empty(), "a spec with no rules must report empty");

    let keyed = crate::dispatch::ConflictKeySpec::builder()
        .method("svc/method", crate::dispatch::KeySource::Sender)
        .build()
        .expect("build keyed spec");
    assert!(
        !keyed.is_empty(),
        "a spec with a rule must report non-empty"
    );
}

#[derive(Debug, PartialEq, Eq)]
enum ShutdownOrderEvent {
    FirstStarted,
    FirstFinished,
    SecondStarted,
    OnStop,
}

async fn next_shutdown_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ShutdownOrderEvent>,
) -> ShutdownOrderEvent {
    tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .expect("shutdown-order event watchdog")
        .expect("shutdown-order event channel open")
}

/// Linked actor used to prove that graceful teardown drains a durable mailbox
/// batch's same-key tail before the lifecycle barrier is allowed to run.
struct ShutdownOrderHandle {
    events: tokio::sync::mpsc::UnboundedSender<ShutdownOrderEvent>,
    release_first: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl LinkedWorkloadHandle for ShutdownOrderHandle {
    async fn dispatch(
        &self,
        envelope: RpcEnvelope,
        _ctx: Arc<crate::context::RuntimeContext>,
    ) -> ActorResult<bytes::Bytes> {
        match envelope.request_id.as_str() {
            "first" => {
                let _ = self.events.send(ShutdownOrderEvent::FirstStarted);
                self.release_first
                    .acquire()
                    .await
                    .expect("release semaphore open")
                    .forget();
                let _ = self.events.send(ShutdownOrderEvent::FirstFinished);
            }
            "second" => {
                let _ = self.events.send(ShutdownOrderEvent::SecondStarted);
            }
            other => panic!("unexpected dispatch {other}"),
        }
        Ok(bytes::Bytes::new())
    }

    async fn on_stop(&self, _ctx: &crate::context::RuntimeContext) -> ActorResult<()> {
        let _ = self.events.send(ShutdownOrderEvent::OnStop);
        Ok(())
    }
}

fn shutdown_order_dispatch(
    runner: Arc<crate::executor::ActorHandle>,
    ctx: crate::context::RuntimeContext,
    request_id: &'static str,
) -> crate::dispatch::scheduler::DispatchFn {
    Box::new(move || {
        Box::pin(async move {
            let envelope = RpcEnvelope {
                request_id: request_id.to_string(),
                ..RpcEnvelope::default()
            };
            let invocation = crate::workload::InvocationContext {
                self_id: ActrId::default(),
                caller_id: None,
                request_id: request_id.to_string(),
            };
            let host_abi: crate::workload::HostAbiFn =
                Arc::new(|_| Box::pin(async { HostOperationResult::Done }));
            runner
                .dispatch_envelope(envelope, ctx, invocation, &host_abi)
                .await
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_admits_active_mailbox_batch_before_on_stop() {
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let release_first = Arc::new(tokio::sync::Semaphore::new(0));
    let workload = Workload::Linked(Arc::new(ShutdownOrderHandle {
        events: events_tx,
        release_first: release_first.clone(),
    }) as Arc<dyn LinkedWorkloadHandle>);
    let runner = Arc::new(crate::executor::spawn_runner_with_mode(
        workload,
        crate::executor::RunnerMode::Interleaved,
        None,
    ));
    let scheduler = Arc::new(crate::dispatch::scheduler::SchedulerHandle::spawn(2, 8));
    let ctx =
        runtime_context_with_host_transport(ActrId::default(), Arc::new(HostTransport::new()));
    let key = crate::dispatch::conflict_key::ConflictKey::Scoped {
        domain: Arc::from("shutdown-order"),
        value: bytes::Bytes::from_static(b"same"),
    };
    let mailbox_batch_admission = MailboxBatchAdmission::new();
    // Models one successful dequeue: every record in the returned batch is
    // already Inflight even though only the first has reached the scheduler.
    let active_batch = mailbox_batch_admission
        .enter()
        .await
        .expect("mailbox admission starts open");

    let first = scheduler
        .submit(
            key.clone(),
            shutdown_order_dispatch(runner.clone(), ctx.clone(), "first"),
        )
        .await;
    assert_eq!(
        next_shutdown_event(&mut events_rx).await,
        ShutdownOrderEvent::FirstStarted
    );

    let stop_runner = runner.clone();
    let stop_ctx = ctx.clone();
    let stop = async move {
        let invocation = crate::workload::InvocationContext {
            self_id: ActrId::default(),
            caller_id: None,
            request_id: "lifecycle:on_stop".to_string(),
        };
        let host_abi: crate::workload::HostAbiFn =
            Arc::new(|_| Box::pin(async { HostOperationResult::Done }));
        stop_runner
            .on_stop(stop_ctx, invocation, &host_abi)
            .await
            .expect("on_stop succeeds");
    };
    let shutdown_runner = runner.clone();
    let shutdown_scheduler = scheduler.clone();
    let shutdown_mailbox_admission = mailbox_batch_admission.clone();
    let (prelude_started_tx, prelude_started_rx) = tokio::sync::oneshot::channel();
    let (admission_closed_tx, admission_closed_rx) = tokio::sync::oneshot::channel();
    let shutdown = tokio::spawn(async move {
        shutdown_actor_runner(
            Some(shutdown_scheduler),
            shutdown_runner,
            Box::pin(async move {
                let _ = prelude_started_tx.send(());
                shutdown_mailbox_admission.close().await;
                let _ = admission_closed_tx.send(());
            }),
            Box::pin(stop),
        )
        .await;
    });

    prelude_started_rx
        .await
        .expect("shutdown prelude started while the batch is active");
    // This is the tail record from the same dequeue batch. Shutdown is already
    // running its admission-closing prelude, but scheduler intake must remain
    // open until the complete batch has been submitted.
    let second = scheduler
        .submit(
            key,
            shutdown_order_dispatch(runner.clone(), ctx.clone(), "second"),
        )
        .await;
    drop(active_batch);
    tokio::time::timeout(std::time::Duration::from_secs(5), admission_closed_rx)
        .await
        .expect("mailbox admission close watchdog")
        .expect("mailbox admission closed after the active batch completed");
    assert!(
        mailbox_batch_admission.enter().await.is_none(),
        "a batch starting after shutdown must not dequeue more durable messages"
    );

    release_first.add_permits(1);
    assert_eq!(
        next_shutdown_event(&mut events_rx).await,
        ShutdownOrderEvent::FirstFinished
    );
    assert_eq!(
        next_shutdown_event(&mut events_rx).await,
        ShutdownOrderEvent::SecondStarted,
        "the admitted same-key tail must execute during graceful drain"
    );
    assert_eq!(
        next_shutdown_event(&mut events_rx).await,
        ShutdownOrderEvent::OnStop,
        "on_stop must follow every admitted scheduler job"
    );
    assert!(first.await.expect("scheduler alive").is_ok());
    assert!(second.await.expect("scheduler alive").is_ok());
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
        .await
        .expect("shutdown coordinator watchdog")
        .expect("shutdown coordinator task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_shutdown_prelude_aborts_runner() {
    let workload = Workload::Linked(Arc::new(DummyLinkedHandle) as Arc<dyn LinkedWorkloadHandle>);
    let runner = Arc::new(crate::executor::spawn_runner(workload));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let pre_shutdown = async move {
        let _ = entered_tx.send(());
        std::future::pending::<()>().await;
    };

    let shutdown_runner = runner.clone();
    let shutdown = tokio::spawn(async move {
        shutdown_actor_runner(
            None,
            shutdown_runner,
            Box::pin(pre_shutdown),
            Box::pin(async {}),
        )
        .await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
        .await
        .expect("shutdown prelude did not start")
        .expect("shutdown prelude signal dropped");

    shutdown.abort();
    let _ = shutdown.await;
    tokio::time::timeout(std::time::Duration::from_secs(2), runner.join())
        .await
        .expect("cancelled shutdown coordinator detached the actor runner");
}

#[test]
fn connection_not_ready_has_distinct_wire_code() {
    let err = ActrError::ConnectionNotReady(ConnectionNotReadyInfo::new(1200, 6000));

    assert_eq!(protocol_error_to_code(&err), 10011);
}

#[test]
fn connection_not_ready_wire_code_roundtrips_variant_and_retry_hint() {
    let original = ActrError::ConnectionNotReady(ConnectionNotReadyInfo::new(1200, 6000));
    let err = wire_code_to_actr_error(protocol_error_to_code(&original), original.to_string());

    match err {
        ActrError::ConnectionNotReady(info) => {
            assert_eq!(info.retry_after_ms, Some(4800));
        }
        other => panic!("expected ConnectionNotReady, got {other:?}"),
    }
}

#[test]
fn connection_not_ready_wire_code_handles_missing_retry_hint() {
    let original = ActrError::ConnectionNotReady(ConnectionNotReadyInfo::without_retry_hint());
    let err = wire_code_to_actr_error(protocol_error_to_code(&original), original.to_string());

    match err {
        ActrError::ConnectionNotReady(info) => {
            assert_eq!(info.retry_after_ms, None);
        }
        other => panic!("expected ConnectionNotReady, got {other:?}"),
    }
}

// ── host_operation_handler dispatch coverage ────────────────────────────
//
// `host_operation_handler` is the private router invoked when a guest
// workload issues a host call. It is compiled under default features but
// only reached at runtime via the wasm/dynclib engines, so it shows as
// uncovered unless we drive it directly. These tests construct a real
// RuntimeContext (in-process HostTransport, unreachable signaling) and a
// dummy linked workload, then assert the routing outcome for each
// HostOperation variant.

use crate::test_support::runtime_context_with_host_transport;
use crate::transport::HostTransport;
use crate::workload::{HostOperation, HostOperationResult, LinkedWorkloadHandle, Workload};
use actr_framework::guest::dynclib_abi::{
    DestV1, HostCallRawV1, HostCallV1, HostDiscoverV1, HostRegisterStreamV1,
    HostResolveDependencyV1, HostSendDataChunkV1, HostTellV1, HostUnregisterStreamV1,
    code as abi_code,
};
use actr_protocol::{AIdCredential, ActrId, PayloadType};
use std::sync::Arc;

/// Trivial linked handle that accepts every default hook and rejects
/// dispatch — sufficient because host_operation_handler only touches the
/// workload for the DataChunk callback path (not exercised here).
struct DummyLinkedHandle;
#[async_trait::async_trait]
impl LinkedWorkloadHandle for DummyLinkedHandle {}

async fn harness() -> (
    crate::context::RuntimeContext,
    Arc<crate::executor::ActorHandle>,
) {
    let ctx =
        runtime_context_with_host_transport(ActrId::default(), Arc::new(HostTransport::new()));
    let wl = Workload::Linked(Arc::new(DummyLinkedHandle) as Arc<dyn LinkedWorkloadHandle>);
    (ctx, Arc::new(crate::executor::spawn_runner(wl)))
}

async fn actorless_harness() -> (
    crate::context::RuntimeContext,
    Arc<crate::executor::ActorHandle>,
) {
    use crate::inbound::{DataChunkRegistry, MediaFrameRegistry};
    use crate::outbound::{Gate, HostGate};
    use crate::wire::webrtc::{
        InboundQueueConfig, ReconnectConfig, SignalingConfig, WebSocketSignalingClient,
    };

    let host = Arc::new(HostTransport::new());
    let inproc = Gate::Host(Arc::new(HostGate::new(host)));
    let ctx = crate::context::RuntimeContext::new(
        ActrId::default(),
        None,
        "req".into(),
        inproc,
        None,
        Arc::new(DataChunkRegistry::new()),
        Arc::new(MediaFrameRegistry::new()),
        Arc::new(WebSocketSignalingClient::new(SignalingConfig {
            server_url: url::Url::parse("ws://127.0.0.1:9").unwrap(),
            connection_timeout: 1,
            heartbeat_interval: 30,
            reconnect_config: ReconnectConfig::default(),
            auth_config: None,
            webrtc_role: None,
            inbound_queue: InboundQueueConfig::default(),
        })) as Arc<dyn SignalingClient>,
        AIdCredential::default(),
        None,
        None,
        0,
    );
    let wl = Workload::Linked(Arc::new(DummyLinkedHandle) as Arc<dyn LinkedWorkloadHandle>);
    (ctx, Arc::new(crate::executor::spawn_runner(wl)))
}

fn expect_abi_error_code(res: HostOperationResult, expected: i32) {
    match res {
        HostOperationResult::AbiError(code) => assert_eq!(code, expected),
        other => panic!("expected HostOperationResult::AbiError({expected}), got {other:?}"),
    }
}

fn expect_runtime_error(res: HostOperationResult) -> ActrError {
    match res {
        HostOperationResult::Error(error) => error,
        other => panic!("expected structured runtime error, got {other:?}"),
    }
}

fn weak_runner(
    runner: &Arc<crate::executor::ActorHandle>,
) -> std::sync::Weak<crate::executor::ActorHandle> {
    Arc::downgrade(runner)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callraw_without_peer_returns_internal() {
    let (ctx, wl) = actorless_harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        // A positive timeout is required to pass sender-side validation so the
        // call reaches gate selection, which is the path under test here.
        HostOperation::CallRaw(HostCallRawV1 {
            timeout_ms: 30_000,
            ..Default::default()
        }),
    )
    .await;
    // No remote actor gate is installed, so routing fails immediately.
    assert!(matches!(expect_runtime_error(res), ActrError::Internal(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_with_default_dest_returns_error() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::Call(HostCallV1::default()),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tell_with_default_dest_returns_error() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::Tell(HostTellV1::default()),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_without_realm_returns_unavailable() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::Discover(HostDiscoverV1::default()),
    )
    .await;
    // Default ActrType with unreachable signaling → discover errors.
    assert!(matches!(
        expect_runtime_error(res),
        ActrError::Unavailable(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_dependency_alias_preserves_structured_error() {
    let (ctx, workload) = harness().await;
    let result = host_operation_handler(
        ctx,
        weak_runner(&workload),
        HostOperation::ResolveDependency(HostResolveDependencyV1 {
            alias: "missing".into(),
        }),
    )
    .await;
    assert!(matches!(
        expect_runtime_error(result),
        ActrError::DependencyNotFound { service_name, .. } if service_name == "missing"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_and_unregister_stream_roundtrip() {
    let (ctx, wl) = harness().await;
    let register = host_operation_handler(
        ctx.clone(),
        weak_runner(&wl),
        HostOperation::RegisterStream(HostRegisterStreamV1 {
            stream_id: "stream-1".to_string(),
        }),
    )
    .await;
    assert!(
        matches!(register, HostOperationResult::Done),
        "register_stream should succeed against the in-process transport: {register:?}"
    );

    let unregister = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::UnregisterStream(HostUnregisterStreamV1 {
            stream_id: "stream-1".to_string(),
        }),
    )
    .await;
    assert!(
        matches!(unregister, HostOperationResult::Done),
        "unregister_stream should succeed after register: {unregister:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_teardown_releases_registered_stream_callback_cycle() {
    let (ctx, wl) = harness().await;
    let registry = ctx.data_chunk_registry_for_test();
    let weak_registry = Arc::downgrade(&registry);
    let cleanup = DataChunkRegistryCleanupGuard::new(&registry);

    let register = host_operation_handler(
        ctx.clone(),
        weak_runner(&wl),
        HostOperation::RegisterStream(HostRegisterStreamV1 {
            stream_id: "cycle-stream".to_string(),
        }),
    )
    .await;
    assert!(matches!(register, HostOperationResult::Done));

    // Execute the exact callback installed by the host bridge. A linked
    // workload intentionally reports NotImplemented for package stream
    // callbacks, which proves the callback reached the live actor runner.
    let callback_result = registry
        .invoke_for_test(
            actr_protocol::DataChunk {
                stream_id: "cycle-stream".to_string(),
                sequence: 1,
                ..Default::default()
            },
            ActrId::default(),
        )
        .await
        .expect("registered callback");
    assert!(matches!(callback_result, Err(ActrError::NotImplemented(_))));

    drop(ctx);
    // `ActorHandle::drop` only schedules cancellation. Join the runner before
    // inspecting registry ownership so its completed DataChunk command cannot
    // retain a transient HostAbiFn -> RuntimeContext -> registry reference.
    wl.abort_and_join().await;
    drop(wl);
    drop(registry);
    assert!(
        weak_registry.upgrade().is_some(),
        "the regression fixture must retain the registry through its callback"
    );

    // `Inner` owns this guard in production. Dropping it clears callbacks,
    // which drops the captured RuntimeContext and releases the registry.
    drop(cleanup);
    assert!(
        weak_registry.upgrade().is_none(),
        "node teardown must break the stream callback ownership cycle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregister_unknown_stream_is_idempotent_done() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::UnregisterStream(HostUnregisterStreamV1 {
            stream_id: "never-registered".to_string(),
        }),
    )
    .await;
    // unregister_stream is idempotent: unknown ids resolve to Done, not Error.
    assert!(
        matches!(res, HostOperationResult::Done),
        "unregister of unknown stream should be idempotent Done: {res:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_data_chunk_rejects_non_stream_payload_type() {
    let (ctx, wl) = harness().await;
    // RpcReliable is a valid PayloadType but not a stream type → PROTOCOL_ERROR.
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: PayloadType::RpcReliable as i32,
            ..Default::default()
        }),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_data_chunk_rejects_unknown_payload_type() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: 9999,
            ..Default::default()
        }),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_data_chunk_valid_type_routes_and_errors() {
    let (ctx, wl) = harness().await;
    // Valid stream payload type, but no route to the default dest → routing error.
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: PayloadType::StreamReliable as i32,
            ..Default::default()
        }),
    )
    .await;
    assert!(matches!(
        expect_runtime_error(res),
        ActrError::Unavailable(_)
    ));
}

// ── stream_callback_host_operation_handler ──────────────────────────────
//
// The stream-callback variant rejects RegisterStream (UNSUPPORTED_OP) and
// otherwise mirrors the main handler. It takes no workload_dispatch, only a
// context + the pending HostOperation.

async fn ctx_only() -> crate::context::RuntimeContext {
    harness().await.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_callraw_without_peer_errors() {
    let ctx = actorless_harness().await.0;
    let res = stream_callback_host_operation_handler(
        ctx,
        // A positive timeout is required to pass sender-side validation so the
        // call reaches gate selection, which is the path under test here.
        HostOperation::CallRaw(HostCallRawV1 {
            timeout_ms: 30_000,
            ..Default::default()
        }),
    )
    .await;
    assert!(matches!(expect_runtime_error(res), ActrError::Internal(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_call_with_default_dest_errors() {
    let ctx = ctx_only().await;
    let res =
        stream_callback_host_operation_handler(ctx, HostOperation::Call(HostCallV1::default()))
            .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_tell_with_default_dest_errors() {
    let ctx = ctx_only().await;
    let res =
        stream_callback_host_operation_handler(ctx, HostOperation::Tell(HostTellV1::default()))
            .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_discover_errors() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::Discover(HostDiscoverV1::default()),
    )
    .await;
    assert!(matches!(
        expect_runtime_error(res),
        ActrError::Unavailable(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_register_stream_is_unsupported() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::RegisterStream(HostRegisterStreamV1 {
            stream_id: "s".to_string(),
        }),
    )
    .await;
    // Registering a stream from inside a stream callback is rejected.
    expect_abi_error_code(res, abi_code::UNSUPPORTED_OP);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_unregister_is_idempotent_done() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::UnregisterStream(HostUnregisterStreamV1 {
            stream_id: "never".to_string(),
        }),
    )
    .await;
    assert!(
        matches!(res, HostOperationResult::Done),
        "stream-callback unregister should be idempotent Done: {res:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_send_rejects_non_stream_payload_type() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: PayloadType::RpcReliable as i32,
            ..Default::default()
        }),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_send_rejects_unknown_payload_type() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: 9999,
            ..Default::default()
        }),
    )
    .await;
    expect_abi_error_code(res, abi_code::PROTOCOL_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_callback_send_valid_type_routes_and_errors() {
    let ctx = ctx_only().await;
    let res = stream_callback_host_operation_handler(
        ctx,
        HostOperation::SendDataChunk(HostSendDataChunkV1 {
            dest: DestV1::workload(),
            payload_type: PayloadType::StreamLatencyFirst as i32,
            ..Default::default()
        }),
    )
    .await;
    assert!(matches!(
        expect_runtime_error(res),
        ActrError::Unavailable(_)
    ));
}

// ── duplicate_wait_timeout / wait_for_inflight_duplicate ────────────────

#[test]
fn duplicate_wait_timeout_uses_explicit_positive_ms() {
    assert_eq!(
        Inner::duplicate_wait_timeout(5000),
        Duration::from_millis(5000)
    );
}

#[test]
fn duplicate_wait_timeout_falls_back_to_dedup_ttl_for_nonpositive() {
    assert_eq!(Inner::duplicate_wait_timeout(0), DEDUP_TTL);
    assert_eq!(Inner::duplicate_wait_timeout(-1), DEDUP_TTL);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_inflight_duplicate_returns_completed_result() {
    use actr_framework::Bytes;
    let (tx, rx) = tokio::sync::watch::channel(None);
    let waiter: DedupWaiter = rx;

    tx.send(Some(Ok(Bytes::from_static(b"done")))).ok();
    let res = Inner::wait_for_inflight_duplicate(waiter, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(res, Bytes::from_static(b"done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_inflight_duplicate_times_out() {
    let (_tx, rx) = tokio::sync::watch::channel(None);
    let waiter: DedupWaiter = rx;
    let err = Inner::wait_for_inflight_duplicate(waiter, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ActrError::Unavailable(_)),
        "expected Unavailable timeout, got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_inflight_duplicate_propagates_error_result() {
    let (tx, rx) = tokio::sync::watch::channel(None);
    let waiter: DedupWaiter = rx;
    tx.send(Some(Err(ActrError::NotFound("missing".into()))))
        .ok();
    let err = Inner::wait_for_inflight_duplicate(waiter, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(matches!(err, ActrError::NotFound(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_inflight_duplicate_observes_late_completion() {
    let (tx, rx) = tokio::sync::watch::channel(None);
    let waiter: DedupWaiter = rx;
    let tx2 = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx2.send(Some(Ok(actr_framework::Bytes::from_static(b"late"))))
            .ok();
    });
    let res = Inner::wait_for_inflight_duplicate(waiter, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(res, actr_framework::Bytes::from_static(b"late"));
}

// ── wire_code_to_actr_error: every code arm ────────────────────────────

#[test]
fn wire_code_to_actr_error_maps_known_codes() {
    let m = "msg".to_string();
    assert!(matches!(
        wire_code_to_actr_error(10001, m.clone()),
        ActrError::Unavailable(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10002, m.clone()),
        ActrError::TimedOut
    ));
    assert!(matches!(
        wire_code_to_actr_error(10003, m.clone()),
        ActrError::NotFound(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10004, m.clone()),
        ActrError::PermissionDenied(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10005, m.clone()),
        ActrError::InvalidArgument(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10006, m.clone()),
        ActrError::UnknownRoute(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10007, m.clone()),
        ActrError::DependencyNotFound { .. }
    ));
    assert!(matches!(
        wire_code_to_actr_error(10008, m.clone()),
        ActrError::DecodeFailure(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10009, m.clone()),
        ActrError::NotImplemented(_)
    ));
    assert!(matches!(
        wire_code_to_actr_error(10010, m.clone()),
        ActrError::Internal(_)
    ));
}

#[test]
fn wire_code_to_actr_error_unknown_code_falls_back_to_unavailable() {
    let err = wire_code_to_actr_error(99999, "boom".into());
    match err {
        ActrError::Unavailable(msg) => {
            assert!(msg.contains("rpc error 99999"), "got: {msg}");
            assert!(msg.contains("boom"));
        }
        other => panic!("expected Unavailable fallback, got {other:?}"),
    }
}

#[test]
fn parse_connection_not_ready_retry_hint_extracts_value() {
    assert_eq!(
        parse_connection_not_ready_retry_hint("connection not ready: retry after 4800ms"),
        Some(4800)
    );
    // Mixed-version peers may still send the previous Display form.
    assert_eq!(
        parse_connection_not_ready_retry_hint("connection not ready: retry_after_ms=Some(4800)"),
        Some(4800)
    );
    assert_eq!(
        parse_connection_not_ready_retry_hint("connection not ready: retry after 0ms"),
        Some(0)
    );
    assert_eq!(
        parse_connection_not_ready_retry_hint(
            "connection not ready: retry after 18446744073709551615ms"
        ),
        Some(u64::MAX)
    );
}

#[test]
fn parse_connection_not_ready_retry_hint_returns_none_without_marker() {
    assert_eq!(parse_connection_not_ready_retry_hint("plain message"), None);
    assert_eq!(parse_connection_not_ready_retry_hint(""), None);
    for message in [
        "connection not ready: no retry hint",
        "connection not ready: retry_after_ms=None",
        "connection not ready: retry after 42",
        "connection not ready: retry after invalidms",
        "connection not ready: retry after 18446744073709551616ms",
    ] {
        assert_eq!(parse_connection_not_ready_retry_hint(message), None);
    }
}

#[test]
fn protocol_error_to_code_maps_all_variants() {
    // Every ActrError variant must map to its documented wire code.
    assert_eq!(
        protocol_error_to_code(&ActrError::Unavailable("x".into())),
        10001
    );
    assert_eq!(protocol_error_to_code(&ActrError::TimedOut), 10002);
    assert_eq!(
        protocol_error_to_code(&ActrError::NotFound("x".into())),
        10003
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::PermissionDenied("x".into())),
        10004
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::InvalidArgument("x".into())),
        10005
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::UnknownRoute("x".into())),
        10006
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::DependencyNotFound {
            service_name: String::new(),
            message: "x".into()
        }),
        10007
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::DecodeFailure("x".into())),
        10008
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::NotImplemented("x".into())),
        10009
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::Internal("x".into())),
        10010
    );
    assert_eq!(
        protocol_error_to_code(&ActrError::ConnectionNotReady(
            ConnectionNotReadyInfo::without_retry_hint()
        )),
        10011
    );
}

// ── direction-based dispatch routing helpers ────────────────────────────

#[test]
fn dispatchable_direction_accepts_request_and_tell_only() {
    use actr_protocol::Direction;

    assert_eq!(
        Inner::dispatchable_direction(Some(Direction::Request as i32)),
        Some(Direction::Request)
    );
    assert_eq!(
        Inner::dispatchable_direction(Some(Direction::Tell as i32)),
        Some(Direction::Tell)
    );
    // Response is routed to pending maps by the gates; in a dispatch loop it
    // is a mislabel and must be dropped.
    assert_eq!(
        Inner::dispatchable_direction(Some(Direction::Response as i32)),
        None
    );
    assert_eq!(
        Inner::dispatchable_direction(Some(Direction::Unspecified as i32)),
        None
    );
    assert_eq!(Inner::dispatchable_direction(Some(99)), None);
    assert_eq!(Inner::dispatchable_direction(None), None);
}

#[test]
fn envelope_is_tell_matches_only_explicit_tell_label() {
    use actr_protocol::Direction;

    let mut envelope = RpcEnvelope {
        request_id: "tell-detect".to_string(),
        route_key: "pkg.Service.Method".to_string(),
        direction: Some(Direction::Tell as i32),
        ..Default::default()
    };
    assert!(Inner::envelope_is_tell(&envelope));

    // A zero timeout alone is NOT a tell marker anymore.
    envelope.direction = Some(Direction::Request as i32);
    envelope.timeout_ms = 0;
    assert!(!Inner::envelope_is_tell(&envelope));

    envelope.direction = None;
    assert!(!Inner::envelope_is_tell(&envelope));
}

#[test]
fn build_response_envelope_pins_zero_timeout_per_contract() {
    use actr_protocol::Direction;

    // Success RESPONSE.
    let ok_env = Inner::build_response_envelope(
        "req-ok".to_string(),
        "pkg.Service.Method".to_string(),
        Some(Bytes::from_static(b"resp")),
        None,
        None,
        None,
    );
    assert_eq!(ok_env.direction, Some(Direction::Response as i32));
    assert_eq!(
        ok_env.timeout_ms, 0,
        "RESPONSE envelopes must carry timeout_ms=0 per package.proto contract"
    );

    // Error RESPONSE also uses 0; payload/error are mutually exclusive.
    let err_env = Inner::build_response_envelope(
        "req-err".to_string(),
        "pkg.Service.Method".to_string(),
        None,
        Some(actr_protocol::ErrorResponse {
            code: 1,
            message: "boom".to_string(),
        }),
        None,
        None,
    );
    assert_eq!(err_env.timeout_ms, 0);
    assert!(err_env.payload.is_none());
    assert!(err_env.error.is_some());
}

async fn response_route_test_gate(
    peer: &ActrId,
) -> (
    Option<Arc<crate::wire::websocket::WebSocketGate>>,
    tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let sink = futures_util::sink::unfold(tx, |tx, message| async move {
        tx.send(message)
            .map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)?;
        Ok(tx)
    });
    let sink: crate::transport::WsSink = Arc::new(tokio::sync::Mutex::new(Some(Box::pin(sink))));
    let gate = Arc::new(crate::wire::websocket::WebSocketGate::new_relay(
        Arc::new(crate::transport::correlation::RpcCorrelation::new()),
        Arc::new(DataChunkRegistry::new()),
        PeerTypes::default(),
    ));
    gate.install_response_sink_for_test(peer.clone(), sink)
        .await;
    (Some(gate), rx)
}

#[tokio::test]
async fn response_uses_ready_relay_after_proven_pre_send_rtc_failure() {
    use crate::wire::webrtc::rtc::peer::send::{RtcSendAttempt, RtcSendError};
    use actr_protocol::prost::Message as _;

    for error in [
        RtcSendError::ConnectionNotReady,
        RtcSendError::DataChannelClosed,
    ] {
        let peer = ActrId {
            realm: "test@example.com".into(),
            serial_number: 9001,
        };
        let (gate, mut sent) = response_route_test_gate(&peer).await;
        let response = RpcEnvelope {
            request_id: "response-fallback".into(),
            route_key: "test.Response".into(),
            payload: Some(Bytes::from_static(b"reply")),
            ..Default::default()
        };
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let direct = async move {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            RtcSendAttempt::from_result(Err(error))
        };
        assert!(
            try_direct_or_relay(
                Some(direct),
                &gate,
                &peer,
                &response,
                "response-fallback",
                "test.Response"
            )
            .await
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let tokio_tungstenite::tungstenite::Message::Binary(frame) =
            sent.try_recv().expect("relay must receive the response")
        else {
            panic!("expected a binary relay response");
        };
        assert_eq!(frame[0], PayloadType::RpcReliable as u8);
        assert_eq!(
            u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize,
            frame.len() - 5
        );
        let decoded = RpcEnvelope::decode(&frame[5..]).unwrap();
        assert_eq!(decoded.request_id, response.request_id);
        assert_eq!(decoded.payload, response.payload);
        assert_eq!(
            decoded.direction,
            Some(actr_protocol::Direction::Response as i32)
        );
        assert!(matches!(
            sent.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn response_never_replays_a_successful_or_uncertain_rtc_send() {
    use crate::wire::webrtc::rtc::peer::send::{RtcSendAttempt, RtcSendError};

    for result in [Ok(()), Err(RtcSendError::DataChunkDeliveryUncertain)] {
        let peer = ActrId {
            realm: "test@example.com".into(),
            serial_number: 9002,
        };
        let (gate, mut sent) = response_route_test_gate(&peer).await;
        let response = RpcEnvelope {
            request_id: "response-no-replay".into(),
            ..Default::default()
        };
        let direct = async move { RtcSendAttempt::from_result(result) };
        assert!(
            try_direct_or_relay(
                Some(direct),
                &gate,
                &peer,
                &response,
                "response-no-replay",
                "test.Response"
            )
            .await
        );
        assert!(matches!(
            sent.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn response_requires_both_pre_send_facts_before_using_relay() {
    use crate::transport::DeliveryProgress;
    use crate::wire::webrtc::rtc::peer::send::{RtcSendAttempt, RtcSendError};

    for (progress, proven_pre_send) in [
        (DeliveryProgress::Unknown, true),
        (DeliveryProgress::NotAttempted, false),
    ] {
        let peer = ActrId {
            realm: "test@example.com".into(),
            serial_number: 9003,
        };
        let (gate, mut sent) = response_route_test_gate(&peer).await;
        let response = RpcEnvelope {
            request_id: "response-no-proof".into(),
            ..Default::default()
        };
        let direct = async move {
            RtcSendAttempt {
                result: Err(RtcSendError::ConnectionNotReady),
                progress,
                proven_pre_send,
            }
        };
        assert!(
            try_direct_or_relay(
                Some(direct),
                &gate,
                &peer,
                &response,
                "response-no-proof",
                "test.Response"
            )
            .await
        );
        assert!(matches!(
            sent.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}

struct ResponsePathSessionOps;

#[async_trait::async_trait]
impl crate::wire::webrtc::rtc::peer::effects::SessionOps for ResponsePathSessionOps {
    async fn create_offer(
        &self,
    ) -> Result<webrtc::peer_connection::RTCSessionDescription, webrtc::error::Error> {
        let mut description = webrtc::peer_connection::RTCSessionDescription::default();
        description.sdp_type = webrtc::peer_connection::RTCSdpType::Offer;
        Ok(description)
    }

    async fn create_answer(
        &self,
    ) -> Result<webrtc::peer_connection::RTCSessionDescription, webrtc::error::Error> {
        let mut description = webrtc::peer_connection::RTCSessionDescription::default();
        description.sdp_type = webrtc::peer_connection::RTCSdpType::Answer;
        Ok(description)
    }

    async fn set_local_description(
        &self,
        _description: webrtc::peer_connection::RTCSessionDescription,
    ) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn set_remote_description(
        &self,
        _description: webrtc::peer_connection::RTCSessionDescription,
    ) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn add_ice_candidate(
        &self,
        _candidate: webrtc::peer_connection::RTCIceCandidateInit,
    ) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn restart_ice(&self) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn set_configuration(
        &self,
        _config: webrtc::peer_connection::RTCConfiguration,
    ) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn create_data_channel(
        &self,
        label: &str,
        _init: webrtc::data_channel::RTCDataChannelInit,
    ) -> Result<
        Arc<dyn crate::wire::webrtc::rtc::peer::adapter::LaneDataChannel>,
        webrtc::error::Error,
    > {
        Ok(crate::wire::webrtc::rtc::peer::effects::test_support::fake_data_channel(label))
    }

    async fn add_track(
        &self,
        _track: Arc<dyn webrtc::media_stream::track_local::TrackLocal>,
    ) -> Result<Arc<dyn crate::wire::webrtc::rtc::peer::adapter::ActrRtpSender>, webrtc::error::Error>
    {
        Err(webrtc::error::Error::Other(
            "response-path test has no media".into(),
        ))
    }

    async fn remove_track(
        &self,
        _sender: &Arc<dyn crate::wire::webrtc::rtc::peer::adapter::ActrRtpSender>,
    ) -> Result<(), webrtc::error::Error> {
        Ok(())
    }

    async fn get_senders(
        &self,
    ) -> Vec<Arc<dyn crate::wire::webrtc::rtc::peer::adapter::ActrRtpSender>> {
        Vec::new()
    }

    async fn get_stats(&self) -> webrtc::peer_connection::RTCStatsReport {
        Default::default()
    }

    async fn close(&self) -> Result<(), webrtc::error::Error> {
        Ok(())
    }
}

struct ResponsePathSessionFactory;

#[async_trait::async_trait]
impl crate::wire::webrtc::rtc::peer::effects::SessionFactory for ResponsePathSessionFactory {
    async fn build(
        &self,
        _handler: Arc<crate::wire::webrtc::rtc::event_handler::WebRtcEventHandler>,
    ) -> Result<Arc<dyn crate::wire::webrtc::rtc::peer::effects::SessionOps>, webrtc::error::Error>
    {
        Ok(Arc::new(ResponsePathSessionOps))
    }
}

#[tokio::test(start_paused = true)]
async fn response_send_timeout_submits_recovery_hint_to_current_peer() {
    use crate::wire::webrtc::rtc::manager::RtcConnectionManager;

    let (signaling_tx, mut signaling_rx) = tokio::sync::mpsc::channel(16);
    let manager = Arc::new(
        RtcConnectionManager::new(Arc::new(ResponsePathSessionFactory), signaling_tx)
            .with_policy_clock_for_test(Arc::new(
                crate::wire::webrtc::rtc::peer::policy_clock::ManualPolicyClock::default(),
            )),
    );
    manager.supervisor().set_signaling_generation(Some(0));

    let caller = ActrId {
        realm: "test@example.com".to_string(),
        serial_number: 9001,
    };
    let handle = manager.ensure_controller(caller.clone()).await;
    handle.connect().await.expect("connect command");
    handle
        .apply_role_assignment(true)
        .await
        .expect("role assignment command");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let dispatch = signaling_rx
                .recv()
                .await
                .expect("controller signaling channel open");
            if matches!(
                dispatch.message,
                crate::wire::webrtc::rtc::peer::reducer::SignalingMessage::Offer { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("response-path test session did not publish its Offer");

    let mut snapshots = handle.subscribe();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if snapshots.borrow().pc_generation > 0 {
                break;
            }
            snapshots
                .changed()
                .await
                .expect("controller snapshot channel open");
        }
    })
    .await
    .expect("response-path test session did not build");
    assert!(!handle.snapshot().sendable);
    assert_eq!(handle.snapshot().ice_generation, 0);

    let route = Some(manager.clone());
    let send = tokio::spawn(async move {
        send_envelope_to_caller(
            &None,
            &route,
            &caller,
            RpcEnvelope {
                request_id: "response-timeout-hint".to_string(),
                route_key: "test.Response".to_string(),
                ..Default::default()
            },
            "response-timeout-hint",
            "test.Response",
        )
        .await;
    });

    // Let the production response path enter its sendability wait, then drive
    // the audited ten-second deadline without making the unit test sleep.
    tokio::task::yield_now().await;
    tokio::time::advance(CALLER_SENDABLE_WAIT).await;
    send.await.expect("response send task must not panic");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = handle.snapshot();
            if snapshot.ice_generation > 0 {
                break;
            }
            snapshots
                .changed()
                .await
                .expect("controller snapshot channel open");
        }
    })
    .await
    .expect("response timeout must submit a recovery hint that starts Soft Recovery");

    assert_eq!(handle.snapshot().ice_generation, 1);
    manager.close_all().await;
}

// ── handle_incoming TELL dedup semantics ────────────────────────────────
//
// These tests build a real `Inner` (in-memory mailbox, unreachable
// signaling) around a gated linked workload so the in-flight window is
// under deterministic test control — no sleeps.

use crate::lifecycle::node::CredentialState;
use actr_framework::Bytes as FrameworkBytes;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Linked handle whose dispatch blocks until `release` flips to true,
/// signalling entry through `entered_tx` for deterministic sequencing.
struct GatedLinkedHandle {
    entered_tx: tokio::sync::mpsc::UnboundedSender<()>,
    release: tokio::sync::watch::Receiver<bool>,
    runs: Arc<AtomicUsize>,
    result: fn() -> ActorResult<FrameworkBytes>,
}

#[async_trait::async_trait]
impl LinkedWorkloadHandle for GatedLinkedHandle {
    async fn dispatch(
        &self,
        _envelope: RpcEnvelope,
        _ctx: Arc<crate::context::RuntimeContext>,
    ) -> ActorResult<FrameworkBytes> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let _ = self.entered_tx.send(());
        let mut release = self.release.clone();
        while !*release.borrow() {
            if release.changed().await.is_err() {
                break;
            }
        }
        (self.result)()
    }
}

fn dedup_test_config(dir: &tempfile::TempDir) -> actr_config::RuntimeConfig {
    actr_config::RuntimeConfig {
        package: actr_config::PackageInfo {
            name: "DedupTellActor".to_string(),
            actr_type: actr_protocol::ActrType {
                publisher: "test-publisher".to_string(),
                name: "DedupTellActor".to_string(),
                version: "0.1.0".to_string(),
            },
            description: None,
            authors: vec![],
            license: None,
        },
        signaling_url: url::Url::parse("ws://127.0.0.1:9/signaling/ws").unwrap(),
        realm: "test7@example.com".to_string(),
        issuer_endpoint: "http://127.0.0.1:9/issuer".to_string(),
        realm_client_key: None,
        visible_in_discovery: false,
        acl: None,
        mailbox_path: None,
        scripts: std::collections::HashMap::new(),
        webrtc: actr_config::WebRtcConfig::default(),
        observability: actr_config::ObservabilityConfig {
            filter_level: "info".to_string(),
            tracing_enabled: false,
            tracing_endpoint: "http://127.0.0.1:9".to_string(),
            tracing_service_name: "dedup-tell-test".to_string(),
            logs_enabled: false,
            ca_cert_path: None,
        },
        config_dir: dir.path().to_path_buf(),
        trust: vec![],
        package_path: None,
        web: None,
        storage_root: None,
    }
}

#[tokio::test]
async fn inner_shares_prebuilt_alias_map_with_all_runtime_contexts() {
    let dir = tempfile::TempDir::new().unwrap();
    let workload = Workload::Linked(Arc::new(DummyLinkedHandle) as Arc<dyn LinkedWorkloadHandle>);
    let aliases = HashMap::from([(
        "sensor".to_string(),
        actr_protocol::ActrType::from_string_repr("acme/Sensor@1.0.0").unwrap(),
    )]);
    let sensor_type = aliases["sensor"].clone();
    let inner = Inner::build(
        dedup_test_config(&dir),
        workload,
        None,
        None,
        Some(crate::LinkedDependencyMetadata {
            aliases,
            fingerprints: HashMap::from([(sensor_type.clone(), "service_semantic:sensor".into())]),
        }),
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed with in-memory mailbox");

    let actor_id = ActrId::default();
    let credential = AIdCredential::default();
    let request_ctx = inner.make_runtime_context(&actor_id, None, "request-id", &credential);
    let bootstrap_ctx = inner
        .bootstrap_ctx_builder()
        .build_bootstrap(&actor_id, &credential);

    assert!(Arc::ptr_eq(
        &inner.alias_map,
        &request_ctx.alias_map_for_test()
    ));
    assert_eq!(
        inner
            .dependency_fingerprints
            .get(&sensor_type)
            .map(String::as_str),
        Some("service_semantic:sensor")
    );
    assert!(Arc::ptr_eq(
        &inner.alias_map,
        &bootstrap_ctx.alias_map_for_test()
    ));
    assert_eq!(
        request_ctx
            .alias_map()
            .get("sensor")
            .map(actr_protocol::ActrType::to_string_repr)
            .as_deref(),
        Some("acme/Sensor@1.0.0")
    );
}

struct TellDedupHarness {
    inner: Arc<Inner>,
    entered_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    release_tx: tokio::sync::watch::Sender<bool>,
    runs: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn tell_dedup_harness(result: fn() -> ActorResult<FrameworkBytes>) -> TellDedupHarness {
    let dir = tempfile::TempDir::new().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let runs = Arc::new(AtomicUsize::new(0));

    let handle = GatedLinkedHandle {
        entered_tx,
        release: release_rx,
        runs: runs.clone(),
        result,
    };
    let workload = Workload::Linked(Arc::new(handle) as Arc<dyn LinkedWorkloadHandle>);
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };

    let mut inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed with in-memory mailbox");
    // handle_incoming requires post-start identity/credential state; set it
    // directly instead of driving the full registration path.
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));

    TellDedupHarness {
        inner: Arc::new(inner),
        entered_rx,
        release_tx,
        runs,
        _dir: dir,
    }
}

fn tell_envelope(request_id: &str) -> RpcEnvelope {
    RpcEnvelope {
        request_id: request_id.to_string(),
        route_key: "pkg.Service.Method".to_string(),
        payload: Some(FrameworkBytes::from_static(b"tell-payload")),
        error: None,
        direction: Some(Direction::Tell as i32),
        traceparent: None,
        tracestate: None,
        metadata: vec![],
        // Documented filler for TELL; must not influence dedup behavior.
        timeout_ms: 0,
    }
}

/// A duplicate TELL arriving while the original is still in flight is
/// dropped immediately — it must not wait `duplicate_wait_timeout` (which
/// maps 0 to the 30 s DEDUP_TTL) and the handler must run exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_inflight_tell_is_dropped_immediately() {
    let mut h = tell_dedup_harness(|| Ok(FrameworkBytes::from_static(b"unsent-response"))).await;
    let envelope = tell_envelope("dup-tell-inflight");

    // Task A: original tell, blocks inside the handler on the release gate.
    let inner_a = h.inner.clone();
    let env_a = envelope.clone();
    let original = tokio::spawn(async move {
        match inner_a.handle_incoming(env_a, None, None, None).await {
            IncomingOutcome::Replied(r) => r,
            IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
        }
    });

    // Deterministic: wait until the handler has actually been entered.
    tokio::time::timeout(Duration::from_secs(5), h.entered_rx.recv())
        .await
        .expect("handler must be entered")
        .expect("entered channel must stay open");

    // Duplicate while in flight: must resolve without waiting for the
    // original (the release gate is still closed, so any wait would hang
    // until this test's own 5 s guard fires).
    let duplicate = tokio::time::timeout(Duration::from_secs(5), async {
        match h
            .inner
            .handle_incoming(envelope.clone(), None, None, None)
            .await
        {
            IncomingOutcome::Replied(r) => r,
            IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
        }
    })
    .await
    .expect("duplicate in-flight tell must return immediately, not wait for the original")
    .expect("duplicate tell drop must resolve Ok");
    assert!(
        duplicate.is_empty(),
        "dropped duplicate tell must carry no payload"
    );
    assert_eq!(
        h.runs.load(Ordering::SeqCst),
        1,
        "duplicate tell must not re-enter the handler"
    );

    // Release the original and let it complete.
    h.release_tx.send(true).unwrap();
    let original_result = original.await.unwrap().unwrap();
    assert_eq!(
        original_result,
        FrameworkBytes::from_static(b"unsent-response"),
        "the original tell still returns the handler result to its loop"
    );

    // Post-completion duplicate: served from the dedup cache, which stores
    // EMPTY bytes for a successful tell (the response is never sent, so the
    // unsent payload is not retained for the DEDUP_TTL).
    let cached = match h.inner.handle_incoming(envelope, None, None, None).await {
        IncomingOutcome::Replied(r) => r.unwrap(),
        IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
    };
    assert!(
        cached.is_empty(),
        "completed tell must cache empty bytes, not the unsent response"
    );
    assert_eq!(
        h.runs.load(Ordering::SeqCst),
        1,
        "post-completion duplicate tell must not re-run the handler"
    );
}

/// A TELL whose handler fails still completes its dedup entry (with the
/// error), so a retry with the same request_id observes the failure instead
/// of re-running the handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tell_handler_error_completes_dedup_entry() {
    let mut h =
        tell_dedup_harness(|| Err(ActrError::Internal("tell handler failed".to_string()))).await;
    let envelope = tell_envelope("dup-tell-error");

    let inner_a = h.inner.clone();
    let env_a = envelope.clone();
    let original = tokio::spawn(async move {
        match inner_a.handle_incoming(env_a, None, None, None).await {
            IncomingOutcome::Replied(r) => r,
            IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
        }
    });

    tokio::time::timeout(Duration::from_secs(5), h.entered_rx.recv())
        .await
        .expect("handler must be entered")
        .expect("entered channel must stay open");
    h.release_tx.send(true).unwrap();

    let original_err = original.await.unwrap().unwrap_err();
    assert!(matches!(original_err, ActrError::Internal(_)));

    // Retry with the same request_id: served from the dedup cache (the
    // recorded error), handler not re-entered.
    let retried = match h.inner.handle_incoming(envelope, None, None, None).await {
        IncomingOutcome::Replied(r) => r,
        IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
    };
    assert!(
        matches!(retried, Err(ActrError::Internal(_))),
        "retried tell must observe the cached handler error, got {retried:?}"
    );
    assert_eq!(
        h.runs.load(Ordering::SeqCst),
        1,
        "retried tell must not re-run the failed handler"
    );
}

/// Interop pin: a REQUEST arriving with `timeout_ms == 0` (buggy or
/// pre-contract sender) is still dispatched and answered — receiver-side
/// permissiveness is part of the #254 contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_with_zero_timeout_is_still_dispatched() {
    let mut h = tell_dedup_harness(|| Ok(FrameworkBytes::from_static(b"answered"))).await;
    let mut envelope = tell_envelope("zero-timeout-request");
    envelope.direction = Some(Direction::Request as i32);
    envelope.timeout_ms = 0;

    let inner_a = h.inner.clone();
    let env_a = envelope.clone();
    let call = tokio::spawn(async move {
        match inner_a.handle_incoming(env_a, None, None, None).await {
            IncomingOutcome::Replied(r) => r,
            IncomingOutcome::Deferred => unreachable!("no EXPLICIT in dedup tests"),
        }
    });

    tokio::time::timeout(Duration::from_secs(5), h.entered_rx.recv())
        .await
        .expect("zero-timeout request must still reach the handler")
        .expect("entered channel must stay open");
    h.release_tx.send(true).unwrap();

    let response = call.await.unwrap().unwrap();
    assert_eq!(
        response,
        FrameworkBytes::from_static(b"answered"),
        "a zero-timeout REQUEST must still be answered"
    );
    assert_eq!(h.runs.load(Ordering::SeqCst), 1);
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// RFC-0323 EXPLICIT dispatch e2e (ack-at-return + counter balance)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
use actr_framework::Context as _;

/// A `LinkedWorkloadHandle` whose route declares `ReplyMode::Explicit` and
/// whose `dispatch` captures the `ReplyDeliver` set on the ctx (instead of
/// building a `Reply<T>` and sending) — lets a test drive the real
/// `admit_incoming` → `ReplyDeliver::deliver_async` path without proto
/// codegen.
struct ExplicitCaptureHandle {
    captured: Arc<std::sync::Mutex<Option<Arc<dyn actr_framework::Deliver>>>>,
}

#[async_trait::async_trait]
impl LinkedWorkloadHandle for ExplicitCaptureHandle {
    fn reply_mode_for_route(&self, route_key: &str) -> actr_protocol::ReplyMode {
        if route_key == "pkg.Svc.ExplicitMethod" {
            actr_protocol::ReplyMode::Explicit
        } else {
            actr_protocol::ReplyMode::Return
        }
    }
    async fn dispatch(
        &self,
        _envelope: RpcEnvelope,
        ctx: Arc<crate::context::RuntimeContext>,
    ) -> ActorResult<FrameworkBytes> {
        if let Some(d) = ctx.reply_deliver() {
            *self.captured.lock().unwrap() = Some(d);
        }
        // EXPLICIT: the dispatch return value is ignored — the reply is
        // deferred to the captured Deliver.
        Ok(FrameworkBytes::new())
    }
}

async fn explicit_capture_harness() -> (
    Arc<Inner>,
    Arc<std::sync::Mutex<Option<Arc<dyn actr_framework::Deliver>>>>,
    tempfile::TempDir,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let captured: Arc<std::sync::Mutex<Option<Arc<dyn actr_framework::Deliver>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let handle = ExplicitCaptureHandle {
        captured: captured.clone(),
    };
    let workload = Workload::Linked(Arc::new(handle) as Arc<dyn LinkedWorkloadHandle>);
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let mut inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed");
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    (Arc::new(inner), captured, dir)
}

fn explicit_envelope(request_id: &str) -> RpcEnvelope {
    RpcEnvelope {
        request_id: request_id.to_string(),
        route_key: "pkg.Svc.ExplicitMethod".to_string(),
        payload: Some(FrameworkBytes::from_static(b"explicit-payload")),
        error: None,
        direction: Some(Direction::Request as i32),
        traceparent: None,
        tracestate: None,
        metadata: vec![],
        timeout_ms: 1000,
    }
}

fn component_package_manifest_for_test() -> actr_pack::PackageManifest {
    actr_pack::PackageManifest {
        publisher: "test".to_string(),
        name: "ComponentEcho".to_string(),
        version: "0.1.0".to_string(),
        binary: actr_pack::BinaryEntry {
            path: "bin/actor.wasm".to_string(),
            target: "wasm32-wasip2".to_string(),
            hash: String::new(),
            size: None,
            kind: Some(actr_pack::BinaryKind::Component),
        },
        signature_algorithm: "ed25519".to_string(),
        signing_key_id: None,
        resources: vec![],
        proto_files: vec![],
        lock_file: None,
        metadata: actr_pack::ManifestMetadata::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn component_explicit_boundary_non_empty_return_falls_back_to_return() {
    struct RawComponentReturnHandle;
    #[async_trait::async_trait]
    impl LinkedWorkloadHandle for RawComponentReturnHandle {
        async fn dispatch(
            &self,
            _envelope: RpcEnvelope,
            _ctx: Arc<crate::context::RuntimeContext>,
        ) -> ActorResult<FrameworkBytes> {
            Ok(FrameworkBytes::from_static(b"raw-component-return"))
        }
    }

    let dir = tempfile::TempDir::new().unwrap();
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let workload =
        Workload::Linked(Arc::new(RawComponentReturnHandle) as Arc<dyn LinkedWorkloadHandle>);
    let mut inner = Inner::build(
        config,
        workload,
        Some(actr_pack::RuntimePackageManifest::Legacy(
            component_package_manifest_for_test(),
        )),
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed");
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    // Simulate the packaged Component path: the host cannot query a Rust
    // dispatcher and therefore selects the Component explicit reply boundary.
    inner.linked_handle = None;
    let inner = Arc::new(inner);

    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
        caller: Err("test-caller".to_string()),
        msg_id: uuid::Uuid::new_v4(),
        span: tracing::Span::none(),
    };
    let outcome = inner
        .handle_incoming(
            explicit_envelope("component-return-fallback"),
            None,
            Some(reply_sink),
            None,
        )
        .await;

    match outcome {
        IncomingOutcome::Replied(Ok(bytes)) => {
            assert_eq!(bytes, FrameworkBytes::from_static(b"raw-component-return"));
        }
        other => panic!("non-empty Component return must not become Deferred: {other:?}"),
    }
    assert_eq!(
        inner.outstanding_replies.len(),
        0,
        "RETURN fallback must release the EXPLICIT outstanding slot"
    );
}

/// RFC-0323 §4/§5: an EXPLICIT dispatch completes `Deferred` (ack-at-return),
/// the `outstanding_replies` gauge rises to 1, and firing the captured
/// `Deliver` releases it back to 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_deferred_balances_outstanding_counter_on_deliver() {
    let (inner, captured, _dir) = explicit_capture_harness().await;
    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
        caller: Err("test-caller".to_string()),
        msg_id: uuid::Uuid::new_v4(),
        span: tracing::Span::none(),
    };
    let outcome = inner
        .handle_incoming(
            explicit_envelope("explicit-bal"),
            None,
            Some(reply_sink),
            None,
        )
        .await;
    assert!(matches!(outcome, IncomingOutcome::Deferred));
    assert_eq!(
        inner.outstanding_replies.len(),
        1,
        "gauge must rise while the EXPLICIT reply is pending"
    );

    let deliver = captured
        .lock()
        .unwrap()
        .clone()
        .expect("ReplyDeliver must be set on ctx for EXPLICIT dispatch");
    deliver.deliver(actr_framework::ReplyOutcome {
        request_id: "explicit-bal".to_string(),
        result: Ok(FrameworkBytes::from_static(b"explicit-reply")),
        emit_response: true,
    });

    // deliver() spawns deliver_async; let it run.
    tokio::time::timeout(Duration::from_secs(2), async {
        while inner.outstanding_replies.len() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("gauge must return to 0 after the Deliver fires");

    assert_eq!(inner.outstanding_replies.len(), 0);
}

/// The legacy `.Service.Method` alias must be normalized before reply-mode
/// selection, not merely at the guest dispatch boundary. Otherwise a linked
/// EXPLICIT route is admitted as RETURN and receives no ReplyDeliver.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_dotted_route_alias_uses_explicit_reply_mode() {
    let (inner, captured, _dir) = explicit_capture_harness().await;
    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
        caller: Err("test-caller".to_string()),
        msg_id: uuid::Uuid::new_v4(),
        span: tracing::Span::none(),
    };
    let mut envelope = explicit_envelope("explicit-dotted-route");
    envelope.route_key = ".pkg.Svc.ExplicitMethod".to_string();

    let outcome = inner
        .handle_incoming(envelope, None, Some(reply_sink), None)
        .await;
    assert!(matches!(outcome, IncomingOutcome::Deferred));
    assert!(
        captured.lock().unwrap().is_some(),
        "canonical reply-mode lookup must provide EXPLICIT routes a ReplyDeliver"
    );
}

/// RFC-0323 §5: an EXPLICIT dispatch that returns `Err` is NOT surfaced as
/// `Replied(Err)` inline — the pump routes the error through the same
/// first-write-wins `Deliver` as a spawned `Reply::send`, so the outcome is
/// `Deferred` (ack-at-return) and the gauge is released by `deliver_async`
/// (not inline). This is the fix for the §5 double-reply race: the Err and a
/// late `Reply::send` now share one `deliver` CAS gate instead of the pump
/// writing `Done(Err)` inline while a late send overwrites it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_handler_err_routes_through_deliver_and_defers() {
    // Handle that does NOT read ctx.reply_deliver() (so no Reply is
    // constructed) and returns Err: simulates a decode/early failure.
    struct ErrNoReplyHandle;
    #[async_trait::async_trait]
    impl LinkedWorkloadHandle for ErrNoReplyHandle {
        fn reply_mode_for_route(&self, _route_key: &str) -> actr_protocol::ReplyMode {
            actr_protocol::ReplyMode::Explicit
        }
        async fn dispatch(
            &self,
            _envelope: RpcEnvelope,
            _ctx: Arc<crate::context::RuntimeContext>,
        ) -> ActorResult<FrameworkBytes> {
            Err(actr_protocol::ActrError::Internal(
                "early failure".to_string(),
            ))
        }
    }

    let dir = tempfile::TempDir::new().unwrap();
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let workload = Workload::Linked(Arc::new(ErrNoReplyHandle) as Arc<dyn LinkedWorkloadHandle>);
    let mut inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed");
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    let inner = Arc::new(inner);

    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
        caller: Err("test-caller".to_string()),
        msg_id: uuid::Uuid::new_v4(),
        span: tracing::Span::none(),
    };
    let outcome = inner
        .handle_incoming(
            explicit_envelope("explicit-err"),
            None,
            Some(reply_sink),
            None,
        )
        .await;
    assert!(
        matches!(outcome, IncomingOutcome::Deferred),
        "EXPLICIT Err must route through the Deliver → Deferred (not Replied(Err)), got {outcome:?}"
    );

    // The pump fired deliver(Err) inside wrap_outcome; deliver_async runs in a
    // spawned task and releases the gauge. It is NOT inline.
    tokio::time::timeout(Duration::from_secs(2), async {
        while inner.outstanding_replies.len() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("gauge must return to 0 after deliver_async fires the Err");
    assert_eq!(inner.outstanding_replies.len(), 0);
}

/// RFC-0323 §5 first-write-wins: an EXPLICIT handler that captures the
/// `Reply`-backed `Deliver` and returns `Err` must have its pump-fired
/// `deliver(Err)` win the CAS. A later `reply.send(Ok)` (simulated by firing
/// the captured Deliver with Ok) degrades to a no-op: no second wire send, no
/// `Done(Err)`→`Done(Ok)` cache flip. Pre-fix this overwrote the dedup entry
/// and emitted a second RESPONSE.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_handler_err_wins_against_late_send_first_write_wins() {
    // Handle captures the Deliver (as a spawned `Reply::send` would receive
    // it) AND returns Err — the §5 race setup.
    struct ErrCaptureHandle {
        captured: Arc<std::sync::Mutex<Option<Arc<dyn actr_framework::Deliver>>>>,
    }
    #[async_trait::async_trait]
    impl LinkedWorkloadHandle for ErrCaptureHandle {
        fn reply_mode_for_route(&self, _route_key: &str) -> actr_protocol::ReplyMode {
            actr_protocol::ReplyMode::Explicit
        }
        async fn dispatch(
            &self,
            _envelope: RpcEnvelope,
            ctx: Arc<crate::context::RuntimeContext>,
        ) -> ActorResult<FrameworkBytes> {
            if let Some(d) = ctx.reply_deliver() {
                *self.captured.lock().unwrap() = Some(d);
            }
            Err(actr_protocol::ActrError::Internal(
                "handler failure".to_string(),
            ))
        }
    }

    let dir = tempfile::TempDir::new().unwrap();
    let captured: Arc<std::sync::Mutex<Option<Arc<dyn actr_framework::Deliver>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let handle = ErrCaptureHandle {
        captured: captured.clone(),
    };
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let workload = Workload::Linked(Arc::new(handle) as Arc<dyn LinkedWorkloadHandle>);
    let mut inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed");
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    let inner = Arc::new(inner);

    let reply_sink = crate::lifecycle::reply_deliver::ReplySink::Mailbox {
        caller: Err("test-caller".to_string()),
        msg_id: uuid::Uuid::new_v4(),
        span: tracing::Span::none(),
    };
    let outcome = inner
        .handle_incoming(
            explicit_envelope("explicit-race"),
            None,
            Some(reply_sink),
            None,
        )
        .await;
    assert!(
        matches!(outcome, IncomingOutcome::Deferred),
        "EXPLICIT Err must route through Deliver → Deferred, got {outcome:?}"
    );

    // The pump already fired deliver(Err) inside wrap_outcome; let
    // deliver_async complete dedup + release the gauge.
    tokio::time::timeout(Duration::from_secs(2), async {
        while inner.outstanding_replies.len() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("gauge must return to 0 after the pump's deliver(Err)");

    // A late spawned `reply.send(Ok)` fires the captured Deliver. The pump
    // already won the `delivered` CAS, so this MUST degrade to a no-op — no
    // second deliver_async, no dedup overwrite, no gauge bump.
    let late_deliver = captured
        .lock()
        .unwrap()
        .clone()
        .expect("Deliver must be captured for the late-send race");
    late_deliver.deliver(actr_framework::ReplyOutcome {
        request_id: "explicit-race".to_string(),
        result: Ok(FrameworkBytes::from_static(b"late-ok")),
        emit_response: true,
    });
    // Give any (spurious) spawned task a chance to run; there must be none.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // dedup Done must be the pump's Err — the late Ok must NOT have flipped it.
    let dedup_outcome = inner
        .dedup_state
        .lock()
        .await
        .check_or_mark("explicit-race");
    match dedup_outcome {
        crate::lifecycle::dedup::DedupOutcome::Duplicate(Err(
            actr_protocol::ActrError::Internal(_),
        )) => {}
        other => panic!(
            "expected Duplicate(Err(Internal)) — pump's Err must win first-write; got {other:?}"
        ),
    }
    assert_eq!(
        inner.outstanding_replies.len(),
        0,
        "late no-op deliver must not bump the gauge"
    );
}

struct ImmediateNetworkProcessor;

#[async_trait::async_trait]
impl crate::lifecycle::NetworkEventProcessor for ImmediateNetworkProcessor {
    async fn process_network_available(&self) -> Result<(), String> {
        Ok(())
    }

    async fn process_network_lost(&self) -> Result<(), String> {
        Ok(())
    }

    async fn process_network_type_changed(
        &self,
        _is_wifi: bool,
        _is_cellular: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn cleanup_connections(&self) -> Result<(), String> {
        Ok(())
    }
}

async fn profile_test_inner() -> (Inner, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temporary profile-test directory");
    let config = dedup_test_config(&dir);
    let workload = Workload::Linked(Arc::new(DummyLinkedHandle) as Arc<dyn LinkedWorkloadHandle>);
    let inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("profile-test Inner should build");
    (inner, dir)
}

fn online_profile_snapshot(sequence: u64) -> crate::lifecycle::NetworkSnapshot {
    crate::lifecycle::NetworkSnapshot {
        sequence,
        availability: crate::lifecycle::NetworkAvailability::Available,
        transport: crate::lifecycle::NetworkTransportFlags {
            wifi: true,
            ..Default::default()
        },
        is_expensive: false,
        is_constrained: false,
    }
}

#[tokio::test]
async fn node_surface_wires_gated_profile_and_signaling_sink_to_one_supervisor() {
    let (mut inner, _dir) = profile_test_inner().await;
    let handle = inner.create_gated_network_event_handle(0);
    let event_rx = inner
        .network_event_rx
        .take()
        .expect("node surface should install event receiver");
    let internal = inner
        .network_event_internal
        .take()
        .expect("node surface should install supervisor channel");
    let shutdown = CancellationToken::new();
    let (status_tx, mut status_rx) =
        tokio::sync::watch::channel(crate::lifecycle::SupervisorStatus::default());
    let reconciler = tokio::spawn(
        crate::lifecycle::network_event::run_network_event_reconciler_with_channel_and_status(
            event_rx,
            internal,
            Arc::new(ImmediateNetworkProcessor),
            shutdown.clone(),
            status_tx,
        ),
    );

    handle
        .handle_network_path_changed(online_profile_snapshot(1))
        .await
        .expect("online fact should be accepted");
    let unknown_status = tokio::time::timeout(
        Duration::from_secs(1),
        status_rx.wait_for(|status| status.policy_revision > 0),
    )
    .await
    .expect("Gated online status should be published")
    .expect("status stream should remain open")
    .clone();
    assert_eq!(
        unknown_status.last_action_id, None,
        "Gated node surface must not start recovery before authoritative app phase"
    );

    // Exercise the concrete signaling client's production emitter. Reaching
    // the same status stream proves set_supervisor_fact_sink() received a clone
    // of the channel consumed by this reconciler.
    inner
        .signaling_client
        .emit_test_signaling_committed(17, crate::lifecycle::SignalingFactOrigin::External);
    let signaling_status = tokio::time::timeout(
        Duration::from_secs(1),
        status_rx.wait_for(|status| status.policy_revision > unknown_status.policy_revision),
    )
    .await
    .expect("signaling fact should reach the node-owned supervisor")
    .expect("status stream should remain open")
    .clone();

    handle
        .handle_app_lifecycle_changed(crate::lifecycle::AppLifecycleState::Foreground {
            background_duration_ms: 0,
        })
        .await
        .expect("foreground fact should be accepted");
    let foreground_status = tokio::time::timeout(
        Duration::from_secs(1),
        status_rx.wait_for(|status| {
            status.policy_revision > signaling_status.policy_revision
                && status.last_action_id.is_some()
        }),
    )
    .await
    .expect("foreground status should be published")
    .expect("status stream should remain open")
    .clone();
    assert!(
        foreground_status.last_action_id.is_some(),
        "authoritative foreground must release Gated recovery eligibility"
    );

    shutdown.cancel();
    reconciler
        .await
        .expect("node-surface reconciler should shut down cleanly");
}

#[tokio::test]
async fn plain_node_surface_keeps_headless_profile_ungated() {
    let (mut inner, _dir) = profile_test_inner().await;
    let handle = inner.create_network_event_handle(0);
    let event_rx = inner
        .network_event_rx
        .take()
        .expect("plain node surface should install event receiver");
    let internal = inner
        .network_event_internal
        .take()
        .expect("plain node surface should install supervisor channel");
    let shutdown = CancellationToken::new();
    let (status_tx, mut status_rx) =
        tokio::sync::watch::channel(crate::lifecycle::SupervisorStatus::default());
    let reconciler = tokio::spawn(
        crate::lifecycle::network_event::run_network_event_reconciler_with_channel_and_status(
            event_rx,
            internal,
            Arc::new(ImmediateNetworkProcessor),
            shutdown.clone(),
            status_tx,
        ),
    );

    handle
        .handle_network_path_changed(online_profile_snapshot(1))
        .await
        .expect("online fact should be accepted");
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        status_rx.wait_for(|status| status.policy_revision > 0),
    )
    .await
    .expect("Ungated online status should be published")
    .expect("status stream should remain open")
    .clone();
    assert!(
        status.last_action_id.is_some(),
        "plain Rust/headless surface must remain Ungated"
    );

    shutdown.cancel();
    reconciler
        .await
        .expect("plain node reconciler should shut down cleanly");
}

// ── S7A-1: W6 rtc config fail-fast ────────────────────────────────────────

mod rtc_config_guard {
    use actr_config::{IceServer, WebRtcConfig};

    fn config_with_url(url: &str) -> WebRtcConfig {
        WebRtcConfig {
            ice_servers: vec![IceServer {
                urls: vec![url.to_string()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn turns_tls_url_is_a_hard_error() {
        let err =
            super::super::validate_rtc_config(&config_with_url("turns:turn.example.com:5349"))
                .expect_err("turns: must fail fast");
        assert!(err.to_string().contains("issue #848"), "got: {err}");
    }

    #[test]
    fn turn_tcp_transport_is_a_hard_error() {
        let err = super::super::validate_rtc_config(&config_with_url(
            "turn:turn.example.com:3478?transport=tcp",
        ))
        .expect_err("transport=tcp must fail fast");
        assert!(err.to_string().contains("issue #848"), "got: {err}");
    }

    #[test]
    fn udp_mux_port_is_a_hard_error() {
        let mut config = WebRtcConfig::default();
        config.advanced.ice_udp_mux_port = Some(5000);
        let err = super::super::validate_rtc_config(&config)
            .expect_err("ice_udp_mux_port must fail fast");
        assert!(err.to_string().contains("ice_udp_mux_port"), "got: {err}");
    }

    #[test]
    fn udp_port_range_is_a_hard_error() {
        let mut config = WebRtcConfig::default();
        config.advanced.udp_ports = Some((40_000, 40_100));
        let err =
            super::super::validate_rtc_config(&config).expect_err("UDP port range must fail fast");
        assert!(err.to_string().contains("port ranges"), "got: {err}");
    }

    #[test]
    fn plain_udp_turn_and_stun_pass() {
        let mut config = config_with_url("turn:turn.example.com:3478?transport=udp");
        config.ice_servers.push(IceServer {
            urls: vec!["stun:stun.example.com:3478".to_string()],
            ..Default::default()
        });
        super::super::validate_rtc_config(&config).expect("UDP TURN/STUN is supported");
    }
}

#[test]
fn guest_rpc_payload_type_maps_known_lanes_and_degrades_unknown() {
    assert_eq!(guest_rpc_payload_type(0), PayloadType::RpcReliable);
    assert_eq!(guest_rpc_payload_type(1), PayloadType::RpcSignal);
    // A known non-RPC lane passes through untouched; ensure_rpc_payload_type
    // rejects it downstream as InvalidArgument.
    assert_eq!(guest_rpc_payload_type(2), PayloadType::StreamReliable);
    // An i32 unknown to this host (guest built against a newer protocol)
    // degrades to the lane pre-lane guests always got.
    assert_eq!(guest_rpc_payload_type(99), PayloadType::RpcReliable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_with_stream_lane_is_rejected_as_invalid_argument() {
    let (ctx, wl) = harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        // A guest that (incorrectly) declares a stream lane for a typed call
        // must surface InvalidArgument from ensure_rpc_payload_type, not have
        // the lane silently dropped onto the reliable lane.
        HostOperation::Call(HostCallV1 {
            dest: DestV1 {
                kind: Some(actr_framework::guest::dynclib_abi::DestKind::Host(true)),
            },
            // A positive timeout is required to pass sender-side validation
            // so the call reaches the lane guard under test.
            timeout_ms: 30_000,
            payload_type: PayloadType::StreamReliable as i32,
            ..Default::default()
        }),
    )
    .await;
    assert!(matches!(
        expect_runtime_error(res),
        ActrError::InvalidArgument(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_with_signal_lane_passes_lane_validation() {
    let (ctx, wl) = actorless_harness().await;
    let res = host_operation_handler(
        ctx,
        weak_runner(&wl),
        // RpcSignal is a valid RPC lane: lane validation passes and the call
        // proceeds to gate selection, which fails here only because no remote
        // actor gate is installed (same outcome as the reliable-lane case).
        HostOperation::CallRaw(HostCallRawV1 {
            timeout_ms: 30_000,
            payload_type: PayloadType::RpcSignal as i32,
            ..Default::default()
        }),
    )
    .await;
    assert!(matches!(expect_runtime_error(res), ActrError::Internal(_)));
}

// ── #390: scoped request cancellation at admission ──────────────────────────

/// Linked handle capturing the RuntimeContext's inherited request scope so
/// tests can assert scope handoff and deadline propagation into the guest.
struct ScopeCaptureHandle {
    captured: std::sync::Mutex<Vec<Arc<crate::request_scope::RequestScope>>>,
}

#[async_trait::async_trait]
impl LinkedWorkloadHandle for ScopeCaptureHandle {
    async fn dispatch(
        &self,
        _envelope: RpcEnvelope,
        ctx: Arc<crate::context::RuntimeContext>,
    ) -> ActorResult<FrameworkBytes> {
        if let Some(scope) = ctx.scope.clone() {
            self.captured.lock().unwrap().push(scope);
        }
        Ok(FrameworkBytes::new())
    }
}

async fn scope_test_inner(
    handle: Arc<dyn LinkedWorkloadHandle>,
) -> std::sync::Arc<crate::lifecycle::node::Inner> {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dedup_test_config(&dir);
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let mut inner = crate::lifecycle::node::Inner::build(
        config,
        Workload::Linked(handle),
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed with in-memory mailbox");
    // handle_incoming requires post-start identity/credential state; set it
    // directly instead of driving the full registration path.
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    std::sync::Arc::new(inner)
}

#[tokio::test]
async fn cancelled_scope_rejects_request_before_handler_runs() {
    let handle = Arc::new(ScopeCaptureHandle {
        captured: std::sync::Mutex::new(Vec::new()),
    });
    let inner = scope_test_inner(handle.clone()).await;

    let scope = crate::request_scope::RequestScope::root(30_000);
    scope.cancel(crate::request_scope::CancelReason::CallerDropped);

    let envelope = RpcEnvelope {
        route_key: "test/anything".to_string(),
        payload: Some(FrameworkBytes::new()),
        direction: Some(actr_protocol::Direction::Request as i32),
        timeout_ms: 30_000,
        ..Default::default()
    };
    let outcome = inner
        .handle_incoming(envelope, None, None, Some(scope))
        .await;

    match outcome {
        crate::lifecycle::node::IncomingOutcome::Replied(Err(ActrError::TimedOut)) => {}
        other => panic!("expected Replied(Err(TimedOut)), got {other:?}"),
    }
    assert!(
        handle.captured.lock().unwrap().is_empty(),
        "handler must not run for a request whose scope already ended"
    );
}

#[tokio::test]
async fn claimed_scope_reaches_guest_and_bounds_child_deadlines() {
    let handle = Arc::new(ScopeCaptureHandle {
        captured: std::sync::Mutex::new(Vec::new()),
    });
    let inner = scope_test_inner(handle.clone()).await;

    // Root budget of 200 ms: every derived child must fit inside it, even
    // when the child requests for a much larger per-hop timeout.
    let scope = crate::request_scope::RequestScope::root(200);

    let envelope = RpcEnvelope {
        route_key: "test/anything".to_string(),
        payload: Some(FrameworkBytes::new()),
        direction: Some(actr_protocol::Direction::Request as i32),
        timeout_ms: 200,
        ..Default::default()
    };
    let outcome = inner
        .handle_incoming(envelope, None, None, Some(scope))
        .await;

    match outcome {
        crate::lifecycle::node::IncomingOutcome::Replied(Ok(_)) => {}
        other => panic!("expected Replied(Ok), got {other:?}"),
    }

    let captured = handle.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1, "handler must run exactly once");
    let inherited = &captured[0];
    assert!(
        inherited.remaining_ms().is_some_and(|ms| ms <= 200),
        "inherited scope must share the root budget"
    );
    let child = inherited.child(60_000);
    assert!(
        child.remaining_ms().is_some_and(|ms| ms <= 200),
        "child scope deadline must be min(parent, now+timeout), not a fresh budget"
    );
    // The real #390 property: cancelling the root token cascades to children.
    inherited.token().cancel();
    assert!(
        child.token().is_cancelled(),
        "child scopes must sit under the root cancellation token"
    );
}

#[test]
fn delivery_uncertain_wire_code_round_trips_typed_fields() {
    use super::*;
    let original = ActrError::DeliveryUncertain {
        // Both fields contain the legacy delimiter and the previous typed
        // encoding's record separator; the length-prefixed encoding must
        // preserve them verbatim.
        operation_id: "upload: \u{001e}part 3".to_string(),
        cause: "ambiguous partial write: \u{001e}stream stalled".to_string(),
    };
    let code = protocol_error_to_code(&original);
    assert_eq!(code, 10012, "DeliveryUncertain owns a dedicated wire code");
    let encoded = encode_delivery_uncertain_for_wire(&original);
    let round_tripped = wire_code_to_actr_error(code, encoded);
    assert!(
        matches!(
            &round_tripped,
            ActrError::DeliveryUncertain { operation_id, cause }
                if operation_id == "upload: \u{001e}part 3"
                    && cause == "ambiguous partial write: \u{001e}stream stalled"
        ),
        "the typed encoding must round-trip ids and causes containing ': ': {round_tripped:?}"
    );
}

#[test]
fn delivery_uncertain_wire_code_legacy_display_falls_back() {
    use super::*;
    // A peer built before the typed encoding sends the Display form.
    let legacy = wire_code_to_actr_error(
        10012,
        "delivery uncertain for operation stream-1#4: write may have entered transport".to_string(),
    );
    assert!(matches!(
        &legacy,
        ActrError::DeliveryUncertain { operation_id, cause }
            if operation_id == "stream-1#4" && cause == "write may have entered transport"
    ));
    // A malformed/unknown-shaped message keeps the variant with a
    // placeholder id instead of collapsing to Internal.
    let malformed = wire_code_to_actr_error(10012, "garbage".to_string());
    assert!(matches!(
        &malformed,
        ActrError::DeliveryUncertain { operation_id, cause }
            if operation_id == "unknown" && cause == "garbage"
    ));
    // Non-DeliveryUncertain codes still bypass the parser entirely.
    assert!(matches!(
        wire_code_to_actr_error(10010, "bug".to_string()),
        ActrError::Internal(_)
    ));
}

#[test]
fn delivery_uncertain_wire_code_rejects_ambiguous_legacy_separator_payload() {
    use super::*;
    let malformed =
        wire_code_to_actr_error(10012, "operation\u{001e}cause\u{001e}tail".to_string());
    assert!(matches!(
        &malformed,
        ActrError::DeliveryUncertain { operation_id, cause }
            if operation_id == "unknown" && cause == "operation\u{001e}cause\u{001e}tail"
    ));
}

#[test]
fn delivery_uncertain_wire_code_does_not_reinterpret_malformed_v1_payload() {
    use super::*;
    let malformed = wire_code_to_actr_error(
        10012,
        "delivery-uncertain-v1:3:3:ab\u{001e}tail".to_string(),
    );
    assert!(matches!(
        &malformed,
        ActrError::DeliveryUncertain { operation_id, cause }
            if operation_id == "unknown"
                && cause == "delivery-uncertain-v1:3:3:ab\u{001e}tail"
    ));
}

// ── inbound ACL against the sender type recorded with the message ──────

fn acl_client_type() -> actr_protocol::ActrType {
    actr_protocol::ActrType {
        publisher: "acme".to_string(),
        name: "client".to_string(),
        version: "0.1.0".to_string(),
    }
}

fn acl_peer(serial_number: u64) -> ActrId {
    ActrId {
        realm: "test7@example.com".to_string(),
        serial_number,
    }
}

/// An `Inner` whose ACL allows only `acme/client@0.1.0` and whose handler
/// answers every call.
async fn acl_inner(dir: &tempfile::TempDir) -> Arc<Inner> {
    let (entered_tx, _entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_release_tx, release) = tokio::sync::watch::channel(true);
    let handle = GatedLinkedHandle {
        entered_tx,
        release,
        runs: Arc::new(AtomicUsize::new(0)),
        result: || Ok(FrameworkBytes::from_static(b"ok")),
    };
    let workload = Workload::Linked(Arc::new(handle) as Arc<dyn LinkedWorkloadHandle>);
    let mut config = dedup_test_config(dir);
    config.acl = Some(actr_protocol::Acl {
        rules: vec![actr_protocol::AclRule {
            permission: actr_protocol::acl_rule::Permission::Allow as i32,
            from_type: acl_client_type(),
            source_realm: None,
        }],
    });
    let actor_id = ActrId {
        realm: config.realm.clone(),
        serial_number: 1,
    };
    let mut inner = Inner::build(
        config,
        workload,
        None,
        None,
        None,
        100,
        Duration::from_secs(60),
        crate::config::DispatchConcurrency::default(),
        None,
        false,
    )
    .await
    .expect("Inner::build must succeed with in-memory mailbox");
    inner.actor_id = Some(actor_id);
    inner.credential_state = Some(CredentialState::new(AIdCredential::default(), None, None));
    Arc::new(inner)
}

/// Deliver a tell from `caller` through the production inbound dispatcher.
async fn deliver_tell(
    mailbox: &Arc<dyn actr_runtime_mailbox::Mailbox>,
    peer_types: &PeerTypes,
    caller: &ActrId,
    request_id: &str,
) {
    use actr_protocol::prost::Message as _;
    let dispatcher = crate::inbound::dispatcher::InboundDispatcher::new(
        Arc::new(crate::transport::correlation::RpcCorrelation::new()),
        Arc::new(crate::inbound::DataChunkRegistry::new()),
        peer_types.clone(),
    );
    dispatcher
        .dispatch(
            caller.encode_to_vec(),
            FrameworkBytes::from(tell_envelope(request_id).encode_to_vec()),
            PayloadType::RpcReliable,
            mailbox,
        )
        .await;
}

/// Admit a dequeued message the way the mailbox loop does.
async fn admit_dequeued(
    inner: &Inner,
    record: &actr_runtime_mailbox::MessageRecord,
) -> ActorResult<FrameworkBytes> {
    use actr_protocol::prost::Message as _;
    let envelope = RpcEnvelope::decode(&record.payload[..]).expect("stored envelope");
    let caller = ActrId::decode(&record.from[..]).expect("stored caller");
    let caller_type = recorded_caller_type(record);
    match inner
        .handle_incoming(envelope, Some((&caller, caller_type.as_ref())), None, None)
        .await
    {
        IncomingOutcome::Replied(result) => result,
        IncomingOutcome::Deferred => unreachable!("no EXPLICIT in ACL tests"),
    }
}

async fn dequeue_by_request(
    mailbox: &Arc<dyn actr_runtime_mailbox::Mailbox>,
) -> HashMap<String, actr_runtime_mailbox::MessageRecord> {
    use actr_protocol::prost::Message as _;
    mailbox
        .dequeue()
        .await
        .expect("dequeue")
        .into_iter()
        .map(|record| {
            let request_id = RpcEnvelope::decode(&record.payload[..])
                .expect("stored envelope")
                .request_id;
            (request_id, record)
        })
        .collect()
}

#[tokio::test]
async fn a_queued_message_keeps_its_sender_type_after_the_sender_disconnects() {
    let dir = tempfile::TempDir::new().unwrap();
    let inner = acl_inner(&dir).await;
    let mailbox: Arc<dyn actr_runtime_mailbox::Mailbox> = Arc::new(
        actr_runtime_mailbox::SqliteMailbox::new(dir.path().join("mailbox.db"))
            .await
            .unwrap(),
    );
    let peer_types = PeerTypes::default();
    let client = acl_peer(2);
    let stranger = acl_peer(3);

    peer_types.record(client.clone(), acl_client_type());
    peer_types.connection_opened(&client);
    deliver_tell(&mailbox, &peer_types, &client, "from-client").await;
    deliver_tell(&mailbox, &peer_types, &stranger, "from-stranger").await;

    // The client disconnects before its message is dequeued.
    peer_types.connection_closed(&client);
    assert_eq!(peer_types.get(&client), None);

    let records = dequeue_by_request(&mailbox).await;
    assert_eq!(
        admit_dequeued(&inner, &records["from-client"])
            .await
            .expect("the client's recorded type passes the ACL"),
        FrameworkBytes::from_static(b"ok")
    );
    assert!(matches!(
        admit_dequeued(&inner, &records["from-stranger"]).await,
        Err(ActrError::PermissionDenied(_))
    ));
}

#[tokio::test]
async fn a_recovered_message_is_judged_by_its_recorded_sender_type() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mailbox.db");
    let client = acl_peer(2);
    {
        let mailbox: Arc<dyn actr_runtime_mailbox::Mailbox> = Arc::new(
            actr_runtime_mailbox::SqliteMailbox::new(&path)
                .await
                .unwrap(),
        );
        let peer_types = PeerTypes::default();
        peer_types.record(client.clone(), acl_client_type());
        peer_types.connection_opened(&client);
        deliver_tell(&mailbox, &peer_types, &client, "before-restart").await;
    }

    // After a restart nothing about the client is known in memory.
    let inner = acl_inner(&dir).await;
    let mailbox: Arc<dyn actr_runtime_mailbox::Mailbox> = Arc::new(
        actr_runtime_mailbox::SqliteMailbox::new(&path)
            .await
            .unwrap(),
    );
    let records = dequeue_by_request(&mailbox).await;
    assert_eq!(
        admit_dequeued(&inner, &records["before-restart"])
            .await
            .expect("the recovered message passes the ACL"),
        FrameworkBytes::from_static(b"ok")
    );
}

/// A reconnecting peer's new offer is relayed (recording its type) while its
/// old WebRTC connection is still closing; the new connection is counted only
/// after the old one is dropped. The type recorded for the new connection
/// survives the old one's close, and a type whose connection never opens is
/// dropped later.
#[tokio::test(start_paused = true)]
async fn a_type_recorded_while_the_old_connection_closes_is_kept() {
    let peer_types = PeerTypes::default();
    let peer = acl_peer(3);

    peer_types.connection_opened(&peer); // the old connection
    peer_types.record(peer.clone(), acl_client_type()); // the new offer
    peer_types.connection_closed(&peer); // the old connection is dropped
    assert_eq!(peer_types.get(&peer), Some(acl_client_type()));

    peer_types.connection_opened(&peer); // the new connection
    assert_eq!(peer_types.get(&peer), Some(acl_client_type()));
    peer_types.connection_closed(&peer);
    assert_eq!(peer_types.get(&peer), None);

    // A relayed type whose connection never opens.
    peer_types.record(peer.clone(), acl_client_type());
    tokio::time::advance(Duration::from_secs(PEER_TYPE_SETUP_SECS - 1)).await;
    peer_types.record(acl_peer(4), acl_client_type());
    assert_eq!(peer_types.get(&peer), Some(acl_client_type()));
    tokio::time::advance(Duration::from_secs(1)).await;
    peer_types.record(acl_peer(4), acl_client_type());
    assert_eq!(peer_types.get(&peer), None);
}

#[tokio::test]
async fn a_peer_type_lasts_until_its_last_connection_closes() {
    use crate::wire::webrtc::rtc::manager::RtcConnectionManager;

    async fn wait_for(what: &str, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    let (signaling_tx, _signaling_rx) = tokio::sync::mpsc::channel(16);
    let manager = Arc::new(RtcConnectionManager::new(
        Arc::new(ResponsePathSessionFactory),
        signaling_tx,
    ));
    let peer_types = PeerTypes::default();
    let shutdown = CancellationToken::new();
    let follow = tokio::spawn(
        peer_types
            .clone()
            .follow_webrtc_peers(manager.clone(), shutdown.clone()),
    );
    let peer = acl_peer(2);

    // A WebSocket connection and a WebRTC connection to the same peer.
    peer_types.record(peer.clone(), acl_client_type());
    peer_types.connection_opened(&peer);
    manager.ensure_controller(peer.clone()).await;
    wait_for("the WebRTC connection to be counted", || {
        peer_types.connections(&peer) == 2
    })
    .await;

    // The WebSocket connection closes; the WebRTC one still holds the type.
    peer_types.connection_closed(&peer);
    assert_eq!(peer_types.get(&peer), Some(acl_client_type()));

    // The WebRTC connection closes too.
    manager.close_peer(&peer).await;
    wait_for("the peer's type to be dropped", || {
        peer_types.get(&peer).is_none()
    })
    .await;
    assert_eq!(peer_types.connections(&peer), 0);

    shutdown.cancel();
    follow.await.unwrap();
    manager.close_all().await;
}
