// `lifecycle::node::Inner::start` boxes a deliberately large startup state
// machine here so downstream crates never need to lay out its nested loops.
#![recursion_limit = "256"]

//! # actr-hyper
//!
//! Hyper — Actor platform layer + runtime infrastructure
//!
//! ## Positioning
//!
//! Hyper is the operating system for Actors: it defines boundaries (Sandbox), provides platform
//! primitives, and carries the full runtime infrastructure (transport, routing, lifecycle management).
//!
//! An Actor cannot open a database on its own, cannot hold its own private key, and cannot claim
//! to be a certain type — everything must go through Hyper's controlled interfaces.
//!
//! ## Responsibilities
//!
//! ### Platform Layer (formerly Hyper)
//!
//! - Package signature verification (binary_hash + publisher signature)
//! - Actor bootstrap (registers with Issuer on behalf of the Actor, obtains credential)
//! - Storage namespace isolation (independent SQLite space per Actor)
//! - Cryptographic primitives (Ed25519 sign/verify, Actor does not hold raw private keys)
//! - Runtime lifecycle management (ActrNode lifecycle for Executor execution bodies)
//!
//! ### Runtime Infrastructure (formerly actr-runtime)
//!
//! - **Actor Lifecycle**: system init, node start/stop (ActrNode / ActrRef)
//! - **Message Transport**: layered architecture (Wire -> Transport -> Gate -> Dispatch)
//! - **Communication Modes**: in-process (zero-copy) and cross-process (WebRTC / WebSocket)
//! - **Message Persistence**: SQLite-backed Mailbox (ACID guarantees)
//! - **Observability**: logging, distributed tracing (OpenTelemetry, optional feature)
//! - **WASM Engine**: WASM actor execution (optional feature)
//!
//! ## Architecture Layers
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │  Platform (Hyper)                                   │  Issuer Bootstrap
//! │  Sandbox / Verify / Storage / KeyCache              │  Package Verify
//! ├─────────────────────────────────────────────────────┤
//! │  Lifecycle Management (ActrNode → ActrRef)
//! ├─────────────────────────────────────────────────────┤
//! │  Layer 3: Inbound Dispatch                          │  DataChunkRegistry
//! │           (Fast Path Routing)                       │  MediaFrameRegistry
//! ├─────────────────────────────────────────────────────┤
//! │  Layer 2: Outbound Adapters (internal)             │  HostGate
//! │           (Message Sending)                         │  PeerGate
//! ├─────────────────────────────────────────────────────┤
//! │  Layer 1: Transport                                 │  Lane (core abstraction)
//! │           (Channel Management)                      │  HostTransport
//! │                                                     │  PeerTransport
//! ├─────────────────────────────────────────────────────┤
//! │  Layer 0: Wire                                      │  RtcPeerController
//! │           (Physical Connections)                     │  RtcConnectionManager
//! │                                                     │  SignalingClient
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! ## Non-Goals
//!
//! Hyper does not understand business logic, does not perform business-level message routing,
//! and is unaware of business relationships between Actors.
//! The `hyper_send`/`hyper_recv` provided in WASM mode are network I/O primitives;
//! routing decisions are made by the ActrNode running inside the WASM.

// ═══════════════════════════════════════════════════════════════════════════════
// Platform modules (cross-platform)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod config;
pub mod error;

// Runtime error re-exports (from actr_protocol, distinct from HyperError)
pub mod runtime_error;

// RFC-0400 audited timer facade and source-controlled inventory. Production
// code must not construct Tokio timers directly outside this module.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod timer;

// Verify module: TrustProvider trait + built-in verifiers (native-only).
// The verified manifest / package types live in `actr_pack` and are
// re-exported below for downstream consumers.
pub mod verify;

// ═══════════════════════════════════════════════════════════════════════════════
// Native-only modules (excluded on wasm32)
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(not(target_arch = "wasm32"))]
pub mod actr_ref;
#[cfg(not(target_arch = "wasm32"))]
pub mod issuer_client;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod key_cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod storage;

// Runtime infrastructure modules (native-only)
#[cfg(all(not(target_arch = "wasm32"), feature = "test-utils"))]
pub mod inbound;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "test-utils")))]
pub(crate) mod inbound;
#[cfg(not(target_arch = "wasm32"))]
pub mod lifecycle;
#[cfg(all(not(target_arch = "wasm32"), feature = "test-utils"))]
pub mod outbound;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "test-utils")))]
pub(crate) mod outbound;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;
#[cfg(not(target_arch = "wasm32"))]
pub mod wire;

// Shared helpers for integration tests (native-only)
#[cfg(all(not(target_arch = "wasm32"), feature = "test-utils"))]
pub mod test_support;

// Per-hop RPC cancellation scopes: cooperative cancel/deadline
// propagation for linked-guest calls (#390).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod request_scope;

// Context (native-only, depends on transport/wire)
#[cfg(not(target_arch = "wasm32"))]
pub mod context;

// Service lookup rules for opening an instance
pub mod service_lookup;

// Runtime workload abstraction (native-only, WASM/dynclib host)
#[cfg(not(target_arch = "wasm32"))]
pub mod workload;

// Per-actor serial command runner: replaces the node-global
// `Arc<Mutex<Workload>>` with a command channel feeding one owning task.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod executor;

// In-memory conflict-key dispatch scheduling layer (B2): sits between the node
// entry loops and the actor runner. The gate defaults on, but a keyless actor
// never creates the scheduler and remains on the serial path.
#[cfg(not(target_arch = "wasm32"))]
pub mod dispatch;

// ServiceSpec derivation from a verified package (native-only; pulls
// actr-service-compat/proto-fingerprint).
#[cfg(not(target_arch = "wasm32"))]
mod package_runtime;
#[cfg(not(target_arch = "wasm32"))]
mod service_spec;

// WASM actor execution engine (optional, native-only)
#[cfg(all(not(target_arch = "wasm32"), feature = "wasm-engine"))]
pub mod wasm;

// Dynclib actor execution engine (optional, native-only)
#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
pub mod dynclib;

// Observability is public so bindings can bootstrap tracing. Monitoring
// and resource management are reserved scaffolding; they stay crate-private.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod monitoring;
#[cfg(not(target_arch = "wasm32"))]
pub mod observability;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod resource;

// ═══════════════════════════════════════════════════════════════════════════════
// Re-exports: Cross-platform
// ═══════════════════════════════════════════════════════════════════════════════

pub use actr_pack::{
    PackageManifest, RuntimePackageManifest, ValidatedActorIdentity, VerifiedPackage,
    VerifiedRuntimePackage,
};
pub use config::HyperConfig;
pub use error::HyperError;
pub(crate) use error::HyperResult;

// Core protocol types
pub use actr_protocol::{Acl, ActrId, ActrType, ServiceSpec};

// Re-export MediaSample and MediaType from framework (dependency inversion)
pub use actr_framework::{MediaSample, MediaType};

// Runtime error types (distinct from HyperError — these are actor-facing errors)
pub use runtime_error::{ActorResult, ActrError, Classify, ErrorKind};

// Platform traits re-exports
pub use actr_platform_traits::{
    CryptoProvider, KvStore, PlatformError, PlatformProvider, StorageNamespace,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Re-exports: Native-only
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(not(target_arch = "wasm32"))]
pub use issuer_client::{IssuerClient, RealmClientKey, RealmKeySigner};
#[cfg(not(target_arch = "wasm32"))]
pub mod realm_membership;
// Conflict-key dispatch scheduling (B2).
#[cfg(not(target_arch = "wasm32"))]
pub use config::DispatchConcurrency;
#[cfg(not(target_arch = "wasm32"))]
pub use dispatch::{
    ConflictKeyError, ConflictKeySpec, ConflictKeySpecBuilder, KeySource, PayloadFieldKind,
};
#[cfg(not(target_arch = "wasm32"))]
pub use storage::{ActorStore, SecureStorageRoot};
#[cfg(not(target_arch = "wasm32"))]
pub use verify::{ChainTrust, PublisherCertCache, RegistryTrust, StaticTrust, TrustProvider};

// Observability
#[cfg(not(target_arch = "wasm32"))]
pub use observability::{ObservabilityGuard, init_observability};

#[cfg(not(target_arch = "wasm32"))]
pub use actr_ref::ActrRef;
// Runtime core structures
#[cfg(not(target_arch = "wasm32"))]
pub use lifecycle::{CredentialState, NetworkEventHandle};

// Layer 1: Transport layer
#[cfg(all(not(target_arch = "wasm32"), feature = "test-utils"))]
pub use transport::{
    ConnType, DataLane, DefaultWireBuilder, DefaultWireBuilderConfig, HostTransport, PeerTransport,
    WireBuilder, WireHandle,
};
#[cfg(not(target_arch = "wasm32"))]
pub use transport::{Dest, ExponentialBackoff, NetworkError, NetworkResult};

// Layer 0: Wire layer
#[cfg(all(not(target_arch = "wasm32"), feature = "test-utils"))]
pub use wire::WebSocketSignalingClient;
#[cfg(not(target_arch = "wasm32"))]
pub use wire::{
    AuthConfig, AuthType, DisconnectReason, InboundQueueConfig, ReconnectConfig, SignalingClient,
    SignalingConfig, SignalingEvent, SignalingStats, WebRtcConfig,
};

// Mailbox (from actr-runtime-mailbox crate)
#[cfg(not(target_arch = "wasm32"))]
pub use actr_runtime_mailbox::{
    Mailbox, MailboxStats, MessagePriority, MessageRecord, MessageStatus,
};

// Bootstrap context builder (lifecycle hooks + ActrRef app-side context) is
// crate-internal; consumers go through the Node / ActrRef lifecycle.

// Runtime workload abstraction
#[cfg(not(target_arch = "wasm32"))]
pub use workload::{HostAbiFn, HostOperation, HostOperationResult, InvocationContext};

// ═══════════════════════════════════════════════════════════════════════════════
// Prelude
// ═══════════════════════════════════════════════════════════════════════════════

pub mod prelude {
    //! Convenience prelude module
    //!
    //! Re-exports commonly used types and traits for quick imports:
    //!
    //! ```rust
    //! use actr_hyper::prelude::*;
    //! ```

    // ── Platform types (cross-platform) ─────────────────────────────────────
    pub use crate::verify::{ChainTrust, RegistryTrust, StaticTrust, TrustProvider};
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::{Attached, Hyper, Init, Node, Registered, storage::ActorStore};
    pub use crate::{HyperConfig, HyperError};
    pub use actr_pack::{PackageManifest, VerifiedPackage};

    // ── Core structures (native-only) ───────────────────────────────────────
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::actr_ref::ActrRef;

    // Re-export MediaSample and MediaType from framework (dependency inversion)
    pub use actr_framework::{MediaSample, MediaType};

    // ── Layer 0: Wire / WebRTC (native-only) ────────────────────────────────
    #[cfg(feature = "test-utils")]
    pub use crate::wire::webrtc::WebSocketSignalingClient;
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::wire::webrtc::{
        AuthConfig, AuthType, DisconnectReason, InboundQueueConfig, ReconnectConfig,
        SignalingClient, SignalingConfig, SignalingEvent, SignalingStats, WebRtcConfig,
    };

    // ── Mailbox (native-only) ───────────────────────────────────────────────
    #[cfg(not(target_arch = "wasm32"))]
    pub use actr_runtime_mailbox::{
        Mailbox, MailboxStats, MessagePriority, MessageRecord, MessageStatus,
    };

    // ── Layer 1: Transport (native-only) ────────────────────────────────────
    #[cfg(feature = "test-utils")]
    pub use crate::transport::{
        ConnType, DataLane, DefaultWireBuilder, DefaultWireBuilderConfig, HostTransport,
        PeerTransport, WireBuilder, WireHandle,
    };
    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::transport::{Dest, NetworkError, NetworkResult};

    // ── Error types ─────────────────────────────────────────────────────────
    pub use crate::runtime_error::{ActorResult, ActrError};

    // ── Base types ──────────────────────────────────────────────────────────
    pub use actr_protocol::ActrId;

    // ── Framework traits (for implementing Workload) ────────────────────────
    pub use actr_framework::{Context, Workload};

    // ── Async trait support ─────────────────────────────────────────────────
    pub use async_trait::async_trait;

    // ── Common utilities ────────────────────────────────────────────────────
    pub use anyhow::{Context as AnyhowContext, Result as AnyhowResult};
    pub use chrono::{DateTime, Utc};
    pub use uuid::Uuid;

    // ── Tokio runtime primitives ────────────────────────────────────────────
    pub use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};
    #[cfg(not(target_arch = "wasm32"))]
    pub use tokio::time::{Duration, Instant, sleep, timeout};

    // ── Logging ─────────────────────────────────────────────────────────────
    pub use tracing::{debug, error, info, trace, warn};
}

// ═══════════════════════════════════════════════════════════════════════════════
// Hyper runtime instance (platform singleton) — native-only
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
use std::io::Write;
#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use prost::Message;
#[cfg(not(target_arch = "wasm32"))]
use tracing::{debug, error, info, warn};
#[cfg(not(target_arch = "wasm32"))]
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
use actr_platform_traits::KvOp;
#[cfg(not(target_arch = "wasm32"))]
use actr_protocol::{RegisterAuthMode, RegisterRequest, register_response};

#[cfg(not(target_arch = "wasm32"))]
/// Compile-time state marker: a [`Node`] has been born from a [`Hyper`]
/// plus a [`actr_config::RuntimeConfig`], but no attachment path has been
/// chosen yet. Transition to [`Attached`] via [`Node::attach`] or
/// [`Node::link`].
pub struct Init;
#[cfg(not(target_arch = "wasm32"))]
/// Compile-time state marker: a package has been verified and attached; Issuer credential still pending.
pub struct Attached;
#[cfg(not(target_arch = "wasm32"))]
/// Compile-time state marker: Issuer credential has been obtained and injected; ready to start.
pub struct Registered;

#[cfg(not(target_arch = "wasm32"))]
mod node_state_sealed {
    pub trait Sealed {}
    impl Sealed for super::Init {}
    impl Sealed for super::Attached {}
    impl Sealed for super::Registered {}
}

#[cfg(not(target_arch = "wasm32"))]
/// Sealed trait describing valid [`Node`] lifecycle states.
pub trait NodeState: node_state_sealed::Sealed {}
#[cfg(not(target_arch = "wasm32"))]
impl NodeState for Init {}
#[cfg(not(target_arch = "wasm32"))]
impl NodeState for Attached {}
#[cfg(not(target_arch = "wasm32"))]
impl NodeState for Registered {}

#[cfg(not(target_arch = "wasm32"))]
/// Hyper — pre-workload framework infrastructure.
///
/// `Hyper` is the operating system that runs an Actor: it owns configuration,
/// instance identity, trust material, and the package verifier. It is
/// deliberately generic-free and has no knowledge of a specific workload.
///
/// User code constructs Hyper only in the escape-hatch path
/// ([`Node::from_hyper`]); prefer [`Node::from_config_file`] for the common
/// case where config lives in `actr.toml`. The full typestate chain is:
///
/// ```text
/// Node::from_config_file(path)    -> Node<Init>              (framework only)
/// Node::from_hyper(hyper, config) -> Node<Init>              (escape hatch)
///     .attach(package)            -> Node<Attached>          (attach: wasm / dyn lib)
///     .link(workload)             -> Node<Attached>          (link: static lib)
///     .register(issuer_endpoint)     -> Node<Registered>        (credential obtained)
///     .start()                    -> ActrRef                 (running node)
/// ```
///
/// Once you call `attach`, you no longer have a `Hyper`: you have a `Node`,
/// which is "Hyper wired to a workload". `register` and `start` live on
/// `Node`, not on `Hyper`.
pub struct Hyper {
    inner: Arc<HyperInner>,
}

#[cfg(not(target_arch = "wasm32"))]
struct HyperInner {
    config: HyperConfig,
    /// Private runtime gate for on-demand membership recovery. Keeping it out
    /// of public `HyperConfig` preserves existing external struct literals.
    membership_controller_enabled: bool,
    /// Locally unique ID generated and persisted on first startup
    instance_id: String,
    /// Optional platform provider for cross-platform abstraction
    platform: Option<Arc<dyn PlatformProvider>>,
    /// Secured per-actor storage root. `Some` only for the direct-filesystem
    /// branch; `None` when a platform provider owns the real root.
    secure_storage_root: Option<storage::SecureStorageRoot>,
}

#[cfg(not(target_arch = "wasm32"))]
/// Carries state-dependent data for an attached [`Node`].
struct Attachment {
    node: crate::lifecycle::node::Inner,
    /// Verified package retained for Issuer bootstrap: the manifest plus the raw
    /// manifest bytes and signature that Issuer may need to re-verify upstream.
    ///
    /// `None` for linked attachments, which have no verified package
    /// metadata attached. In that case `Node::register*` falls back to the
    /// runtime config's actor metadata instead of package-derived
    /// registration inputs.
    verified: Option<VerifiedRuntimePackage>,
    v2_service_spec: Option<ServiceSpec>,
    package_bytes: bytes::Bytes,
}

#[cfg(not(target_arch = "wasm32"))]
/// Node — Hyper wired to a runtime configuration (and optionally a workload).
///
/// A `Node<Init>` is produced by [`Node::from_config_file`] or
/// [`Node::from_hyper`]; it carries `Hyper` + [`actr_config::RuntimeConfig`]
/// but has not yet been attached. Call one of the attach methods to progress
/// into `Node<Attached>`, then `register().start()` into a running
/// [`ActrRef`]:
///
/// ```text
/// Node::from_config_file(path) -> Node<Init>
///     .attach(package)         -> Node<Attached>   (attach: wasm / dyn lib)
///     .link(workload)          -> Node<Attached>   (link: static lib)
///
/// Node<Attached>.register(issuer) -> Node<Registered>
/// Node<Registered>.start()     -> ActrRef
/// ```
///
/// The default type parameter `Attached` means writing `Node` unqualified
/// refers to the attached state; `start()` only exists on `Node<Registered>`.
pub struct Node<S: NodeState = Attached> {
    hyper: Arc<HyperInner>,
    /// Present on `Node<Attached>` and `Node<Registered>`; `None` on
    /// `Node<Init>`, which holds `pending_runtime_config` instead.
    attachment: Option<Attachment>,
    /// Pending runtime configuration for `Node<Init>`; consumed by attach
    /// methods. `None` on `Attached` / `Registered`.
    pending_runtime_config: Option<actr_config::RuntimeConfig>,
    /// Explicit dynamic manifest for a linked workload. Package attachment
    /// never consults this field.
    pending_linked_manifest: Option<actr_config::ManifestConfig>,
    /// Conflict-key projection spec declared on `Node<Init>` via
    /// [`Node::with_conflict_keys`], consumed by attach / link into the dispatch
    /// scheduler. `None` = no declarations = fully serial dispatch.
    pending_conflict_keys: Option<crate::dispatch::ConflictKeySpec>,
    /// Test-only directed fault registries declared on `Node<Init>` via
    /// [`Node::with_test_faults`], consumed by attach / link into the node
    /// inner so `start()` wraps the v0.20 RTC runtime and signaling transport.
    #[cfg(feature = "test-utils")]
    pending_test_faults: Option<crate::wire::webrtc::test_runtime::TestFaultRegistries>,
    _state: std::marker::PhantomData<S>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
pub(crate) struct LinkedDependencyMetadata {
    aliases: std::collections::HashMap<String, actr_protocol::ActrType>,
    fingerprints: std::collections::HashMap<actr_protocol::ActrType, String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_linked_dependency_metadata(
    manifest: &actr_config::ManifestConfig,
) -> HyperResult<LinkedDependencyMetadata> {
    let mut aliases = std::collections::HashMap::with_capacity(manifest.dependencies.len());
    let mut actr_types = std::collections::HashSet::with_capacity(manifest.dependencies.len());
    let mut fingerprints = std::collections::HashMap::with_capacity(manifest.dependencies.len());
    for dependency in &manifest.dependencies {
        let alias = dependency.alias.trim();
        if alias.is_empty() || alias != dependency.alias {
            return Err(HyperError::Config(format!(
                "linked manifest dependency alias `{}` must be non-empty and contain no surrounding whitespace",
                dependency.alias
            )));
        }
        if aliases.contains_key(alias) {
            return Err(HyperError::Config(format!(
                "linked manifest dependency alias `{alias}` is declared more than once"
            )));
        }
        let actr_type = dependency.actr_type.clone().ok_or_else(|| {
            HyperError::Config(format!(
                "linked manifest dependency `{alias}` must declare a complete actr_type for runtime alias resolution"
            ))
        })?;
        let canonical_type = actr_type.to_string_repr();
        if !actr_types.insert(canonical_type.clone()) {
            return Err(HyperError::Config(format!(
                "linked manifest ActrType `{canonical_type}` is assigned to more than one dependency alias"
            )));
        }
        if let Some(service) = &dependency.service {
            fingerprints.insert(actr_type.clone(), service.fingerprint.clone());
        }
        aliases.insert(alias.to_string(), actr_type);
    }
    Ok(LinkedDependencyMetadata {
        aliases,
        fingerprints,
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// Execution backend selected from a verified `.actr` package target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryKind {
    /// Execute the package binary with the WASM runtime.
    Wasm,
    /// Execute the package binary as a C-ABI dynamic library (`cdylib`).
    DynClib,
}

#[cfg(not(target_arch = "wasm32"))]
/// Public `.actr` package input object consumed by Hyper.
#[derive(Debug, Clone)]
pub struct WorkloadPackage {
    bytes: bytes::Bytes,
}

#[cfg(not(target_arch = "wasm32"))]
impl WorkloadPackage {
    /// Wrap already-loaded package bytes.
    pub fn new(bytes: impl Into<bytes::Bytes>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// Load a `.actr` package from the filesystem in one call.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self {
            bytes: bytes.into(),
        })
    }

    /// Raw `.actr` bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Parse and return the package manifest (unverified).
    ///
    /// This reads the manifest TOML embedded in the `.actr` ZIP without checking
    /// the signature. Use [`Hyper::verify_package`] to obtain a verified manifest.
    /// Re-parses on every call — cache externally if you need it hot.
    pub fn manifest(&self) -> HyperResult<RuntimePackageManifest> {
        actr_pack::read_runtime_manifest(&self.bytes)
            .map_err(|e| HyperError::InvalidManifest(e.to_string()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Result of verifying a package and preparing a runtime workload from it.
pub(crate) struct LoadedWorkload {
    /// Verified package retained for downstream bootstrap and storage
    /// operations — carries the parsed manifest plus the raw manifest bytes
    /// and signature needed for transparent forwarding to Issuer.
    pub verified: VerifiedRuntimePackage,
    pub packaged_lock: Option<actr_config::lock::LockFile>,
    pub v2_service_spec: Option<ServiceSpec>,
    /// Backend selected from the signed workload profile or legacy target.
    pub binary_kind: BinaryKind,
    /// Ready-to-attach runtime workload.
    pub workload: crate::workload::Workload,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for LoadedWorkload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedWorkload")
            .field("manifest", &self.verified.manifest)
            .field("backend", &self.binary_kind)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Hyper {
    /// Construct a Hyper with native defaults (uses `tokio::fs` / `ActorStore`).
    ///
    /// - Parse configuration
    /// - Load or generate instance_id (persisted to data_dir)
    /// - Initialize package verifier
    pub async fn new(config: HyperConfig) -> HyperResult<Self> {
        Self::init_inner(config, None).await
    }

    /// Construct a Hyper with an injected platform provider (cross-platform / embedded).
    ///
    /// When a `PlatformProvider` is injected:
    /// - instance UID comes from `platform.instance_uid()` (and its backing store)
    /// - `bootstrap_credential` uses `platform.secret_store()` instead of `ActorStore::open()`
    /// - `TrustProvider` verifies `.actr` package signatures using whatever
    ///   mechanism the injected provider implements
    pub async fn with_platform(
        config: HyperConfig,
        platform: Arc<dyn PlatformProvider>,
    ) -> HyperResult<Self> {
        Self::init_inner(config, Some(platform)).await
    }

    async fn init_inner(
        config: HyperConfig,
        platform: Option<Arc<dyn PlatformProvider>>,
    ) -> HyperResult<Self> {
        info!(
            data_dir = %config.data_dir.display(),
            "Hyper initializing"
        );

        // Resolve an instance UID + ensure data_dir exists. When a platform
        // provider is injected, delegate to it; otherwise fall back to direct
        // tokio::fs calls so this crate stays free of an actr-platform-native
        // dependency (which would be circular).
        let instance_id = if let Some(ref p) = platform {
            p.instance_uid()
                .await
                .map_err(|e| HyperError::Storage(format!("failed to load instance_uid: {e}")))?
        } else {
            tokio::fs::create_dir_all(&config.data_dir)
                .await
                .map_err(|e| {
                    HyperError::Config(format!(
                        "failed to create data_dir `{}`: {e}",
                        config.data_dir.display()
                    ))
                })?;
            load_or_create_instance_uid_local(&config.data_dir).await?
        };
        debug!(instance_id, "Hyper instance_uid ready");

        // Build the secured per-actor storage root only for the direct-filesystem
        // branch; an injected platform provider owns and protects its own root.
        let secure_storage_root = if platform.is_none() {
            Some(storage::SecureStorageRoot::new(&config.storage_root).await?)
        } else {
            None
        };

        Ok(Self {
            inner: Arc::new(HyperInner {
                config,
                membership_controller_enabled: false,
                instance_id,
                platform,
                secure_storage_root,
            }),
        })
    }

    /// Enable or disable on-demand membership recovery.
    ///
    /// This builder lives on `Hyper` instead of adding a required field to the
    /// public [`HyperConfig`] struct.
    pub fn with_membership_controller_enabled(mut self, enabled: bool) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("Hyper is uniquely owned before Node construction")
            .membership_controller_enabled = enabled;
        self
    }

    /// Verify a [`WorkloadPackage`] and return the verified package bundle
    /// (parsed manifest + raw manifest bytes + signature).
    ///
    /// Delegates entirely to the configured [`crate::verify::TrustProvider`];
    /// the provider decides how to authenticate the package (static key,
    /// registry lookup, keyless transparency log, etc).
    pub async fn verify_package(
        &self,
        package: &WorkloadPackage,
    ) -> HyperResult<VerifiedRuntimePackage> {
        verify_package_for_hyper(&self.inner, package.bytes()).await
    }

    /// Verify a package, select its execution backend, and prepare the workload.
    ///
    /// Internal helper used by attachment flow and test-support shims.
    #[cfg(feature = "test-utils")]
    pub(crate) async fn load_workload_package(
        &self,
        package: &WorkloadPackage,
    ) -> HyperResult<LoadedWorkload> {
        load_workload_package_inner(&self.inner, package).await
    }
}

// ── Node entry methods (unparameterized) ─────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
impl Node {
    /// Load `actr.toml` from disk, build the underlying [`Hyper`] from the
    /// `[hyper]` section (or an explicit `[[trust]]` / `[hyper.trust]`
    /// anchor set), and return a [`Node<Init>`] ready to attach a workload.
    ///
    /// The caller is expected to drive the typestate chain themselves:
    ///
    /// ```ignore
    /// let actr_ref = Node::from_config_file("actr.toml").await?
    ///     .attach(&package).await?
    ///     .register(&issuer_endpoint).await?
    ///     .start().await?;
    /// ```
    ///
    /// For a one-shot sugar covering the entire chain see
    /// [`Node::run_from_config`].
    pub async fn from_config_file(path: impl AsRef<std::path::Path>) -> HyperResult<Node<Init>> {
        config::node_from_config_file(path.as_ref()).await
    }

    /// Like [`Node::from_config_file`] but lets the caller pin the
    /// node's identity by supplying the [`actr_config::PackageInfo`]
    /// derived from a sibling `manifest.toml`.
    ///
    /// This is the path the language bindings use: they already parsed
    /// `manifest.toml` to find the runtime config dir and want their
    /// `[package]` honoured rather than collapsed to the
    /// `local/Client@0.0.0` placeholder that `from_config_file` synthesises.
    pub async fn from_config_with_package(
        path: impl AsRef<std::path::Path>,
        package_info: actr_config::PackageInfo,
    ) -> HyperResult<Node<Init>> {
        config::node_from_config_file_with_package(path.as_ref(), Some(package_info)).await
    }

    /// Load runtime configuration together with the exact dynamic manifest
    /// that supplies identity and dependency metadata for a linked workload.
    pub async fn from_config_with_manifest(
        path: impl AsRef<std::path::Path>,
        manifest: actr_config::ManifestConfig,
    ) -> HyperResult<Node<Init>> {
        let mut node = config::node_from_config_file_with_package(
            path.as_ref(),
            Some(manifest.package.clone()),
        )
        .await?;
        node.pending_linked_manifest = Some(manifest);
        Ok(node)
    }

    /// Escape-hatch constructor: wrap an already-built [`Hyper`] plus a
    /// pre-loaded [`actr_config::RuntimeConfig`] into a [`Node<Init>`].
    ///
    /// Use this when you need direct control over `HyperConfig`
    /// construction (custom trust chain, injected platform provider, etc.)
    /// and cannot drive the whole flow through
    /// [`Node::from_config_file`].
    pub fn from_hyper(hyper: Hyper, runtime_config: actr_config::RuntimeConfig) -> Node<Init> {
        Node {
            hyper: hyper.inner,
            attachment: None,
            pending_runtime_config: Some(runtime_config),
            pending_linked_manifest: None,
            pending_conflict_keys: None,
            #[cfg(feature = "test-utils")]
            pending_test_faults: None,
            _state: std::marker::PhantomData,
        }
    }

    /// One-shot sugar: `from_config_file(path).attach(package).register().start()`.
    ///
    /// Loads the runtime configuration from `path`, attaches the given
    /// workload package, registers with Issuer at the `[issuer_endpoint]` URL
    /// from the config, and starts the node, returning a live
    /// [`ActrRef`]. Use the typestate chain directly when you need to
    /// interleave `create_network_event_handle` or swap in a custom
    /// `service_spec` via `register_with`.
    pub async fn run_from_config(
        path: impl AsRef<std::path::Path>,
        package: &WorkloadPackage,
    ) -> HyperResult<ActrRef> {
        let init = Self::from_config_file(path).await?;
        let issuer_endpoint = init
            .pending_runtime_config
            .as_ref()
            .map(|c| c.issuer_endpoint.clone())
            .expect("Node<Init> without pending runtime config");
        let attached = init.attach(package).await?;
        let registered = attached.register(&issuer_endpoint).await?;
        registered
            .start()
            .await
            .map_err(|e| HyperError::Runtime(format!("failed to start node: {e}")))
    }
}

// ── Node<Init> accessors + state transition: Init → Attached ─────────────────

#[cfg(not(target_arch = "wasm32"))]
impl Node<Init> {
    /// Attach an already parsed dynamic manifest before linking. The manifest
    /// becomes authoritative for both actor identity and dependencies.
    pub fn with_linked_manifest(mut self, manifest: actr_config::ManifestConfig) -> Self {
        if let Some(runtime) = self.pending_runtime_config.as_mut() {
            runtime.package = manifest.package.clone();
        }
        self.pending_linked_manifest = Some(manifest);
        self
    }

    /// Read-only view of the runtime configuration pending attachment.
    /// Useful for callers that need to configure observability /
    /// tracing subscribers from the config before driving `attach`.
    pub fn runtime_config(&self) -> &actr_config::RuntimeConfig {
        self.pending_runtime_config
            .as_ref()
            .expect("Node<Init> without pending runtime config")
    }

    /// Override the runtime actor type before attaching or linking a workload.
    ///
    /// `Node::from_config_file` can synthesize a placeholder actor type when
    /// the runtime config has no package manifest. Linked/static hosts use this
    /// method to provide the concrete actor identity used for Issuer registration.
    pub fn with_actor_type(mut self, actor_type: actr_protocol::ActrType) -> Self {
        if let Some(manifest) = self.pending_linked_manifest.as_ref()
            && manifest.package.actr_type != actor_type
        {
            tracing::warn!(
                requested = %actor_type.to_string_repr(),
                manifest = %manifest.package.actr_type.to_string_repr(),
                "ignoring actor type override because the explicit linked manifest owns runtime identity"
            );
            return self;
        }
        let runtime_config = self
            .pending_runtime_config
            .as_mut()
            .expect("Node<Init> without pending runtime config");
        runtime_config.package.name = actor_type.name.clone();
        runtime_config.package.actr_type = actor_type;
        self
    }

    /// Declare per-method conflict keys for dispatch scheduling (B2).
    ///
    /// This is data, evaluated host-side once per message by projecting a few
    /// bytes of the request — no per-message guest crossing. It only takes
    /// effect when dispatch concurrency is enabled
    /// ([`crate::config::HyperConfig::with_dispatch_concurrency`]); with the gate
    /// off, or for methods left undeclared, dispatch stays fully serial.
    ///
    /// Declaring a key asserts that distinct-key invocations of that method may
    /// run concurrently — see [`crate::dispatch::ConflictKeySpec`] for the
    /// consumer-side concurrency contract. Execution concurrency is available
    /// only to native `Linked` workloads and 0.4.0 async-world WASM guests.
    /// The retired 0.1.0 sync-world is rejected at load. `DynClib` is the only
    /// supported packaged backend that stays serial regardless of the gate or
    /// budget: its key affects scheduler routing but provides no
    /// dispatch-throughput benefit.
    pub fn with_conflict_keys(mut self, spec: crate::dispatch::ConflictKeySpec) -> Self {
        self.pending_conflict_keys = Some(spec);
        self
    }

    /// Install test-only directed fault registries (`test-utils` builds only).
    ///
    /// When set, `Node::start` wraps the v0.20 `PeerConnectionFactory` runtime
    /// with the RTC registry (generation-scoped UDP packet rules) and the
    /// signaling transport with the signaling registry (per-class message
    /// rules). The host keeps its own clone of [`TestFaultRegistries`] for
    /// rule installation and hit observation — both registries share state
    /// across clones. Production builds without the `test-utils` feature do
    /// not expose this method and never wrap anything.
    #[cfg(feature = "test-utils")]
    pub fn with_test_faults(
        mut self,
        faults: crate::wire::webrtc::test_runtime::TestFaultRegistries,
    ) -> Self {
        self.pending_test_faults = Some(faults);
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Node<Init> {
    /// Bind a verified [`WorkloadPackage`] to this node.
    ///
    /// Equivalent to the former `Hyper::attach` — verifies the package
    /// signature through the configured `TrustProvider`, loads its guest
    /// binary (WASM or dynclib), and advances the node to
    /// `Node<Attached>`.
    /// Attach a packaged workload (`wasm` / `dyn lib`) to this node.
    pub async fn attach(self, package: &WorkloadPackage) -> HyperResult<Node<Attached>> {
        let mut runtime_config = self
            .pending_runtime_config
            .expect("Node<Init> without pending runtime config");
        let conflict_keys = self.pending_conflict_keys;
        #[cfg(feature = "test-utils")]
        let test_faults = self.pending_test_faults;
        let hyper_inner = self.hyper;
        let loaded = load_workload_package_inner(&hyper_inner, package).await?;
        if matches!(&loaded.verified.manifest, RuntimePackageManifest::V2(_)) {
            let actor_type = loaded.verified.manifest.actor_type();
            runtime_config.package.name.clone_from(&actor_type.name);
            runtime_config.package.actr_type = actor_type;
        }
        let mailbox_backpressure_threshold =
            hyper_inner.config.resolved_mailbox_backpressure_threshold();
        let credential_expiry_warning = hyper_inner.config.credential_expiry_warning;
        let dispatch_concurrency = hyper_inner.config.resolved_dispatch_concurrency();
        let membership_controller_enabled = hyper_inner.membership_controller_enabled;
        let mut node_inner = crate::lifecycle::node::Inner::build(
            runtime_config,
            loaded.workload,
            Some(loaded.verified.manifest.clone()),
            loaded.packaged_lock,
            None,
            mailbox_backpressure_threshold,
            credential_expiry_warning,
            dispatch_concurrency,
            conflict_keys,
            membership_controller_enabled,
        )
        .await
        .map_err(|e| HyperError::Runtime(e.to_string()))?;
        let observer: Arc<dyn crate::lifecycle::hooks::WorkloadHookObserver> =
            Arc::new(crate::workload::PackageHookObserver {
                workload_dispatch: node_inner.workload_dispatch.clone(),
            });
        node_inner.hook_observer = Some(observer);
        #[cfg(feature = "test-utils")]
        {
            node_inner.test_faults = test_faults;
        }
        Ok(Node {
            hyper: hyper_inner,
            attachment: Some(Attachment {
                node: node_inner,
                verified: Some(loaded.verified),
                v2_service_spec: loaded.v2_service_spec,
                package_bytes: package.bytes.clone(),
            }),
            pending_runtime_config: None,
            pending_linked_manifest: None,
            pending_conflict_keys: None,
            #[cfg(feature = "test-utils")]
            pending_test_faults: None,
            _state: std::marker::PhantomData,
        })
    }

    /// Bind an internal workload handle for the `link` path to this node.
    ///
    /// No package is loaded; the host process *is* the workload. The
    /// handle provides both observation hooks and an inbound-dispatch
    /// entry point (see [`workload::LinkedWorkloadHandle::dispatch`]).
    ///
    /// Prefer [`Node::link`] when you already have a generic
    /// [`actr_framework::Workload`] implementation — it wraps the
    /// workload in a [`workload::WorkloadAdapter`] automatically.
    pub(crate) async fn link_handle(
        self,
        handle: Arc<dyn workload::LinkedWorkloadHandle>,
    ) -> HyperResult<Node<Attached>> {
        let dependency_metadata = self
            .pending_linked_manifest
            .as_ref()
            .map(resolve_linked_dependency_metadata)
            .transpose()?
            .unwrap_or_default();
        let runtime_config = self
            .pending_runtime_config
            .expect("Node<Init> without pending runtime config");
        let conflict_keys = self.pending_conflict_keys;
        #[cfg(feature = "test-utils")]
        let test_faults = self.pending_test_faults;
        let hyper_inner = self.hyper;
        let mailbox_backpressure_threshold =
            hyper_inner.config.resolved_mailbox_backpressure_threshold();
        let credential_expiry_warning = hyper_inner.config.credential_expiry_warning;
        let dispatch_concurrency = hyper_inner.config.resolved_dispatch_concurrency();
        let membership_controller_enabled = hyper_inner.membership_controller_enabled;
        let mut node_inner = crate::lifecycle::node::Inner::build(
            runtime_config,
            crate::workload::Workload::Linked(handle.clone()),
            None,
            None,
            Some(dependency_metadata),
            mailbox_backpressure_threshold,
            credential_expiry_warning,
            dispatch_concurrency,
            conflict_keys,
            membership_controller_enabled,
        )
        .await
        .map_err(|e| HyperError::Runtime(e.to_string()))?;
        let observer: Arc<dyn crate::lifecycle::hooks::WorkloadHookObserver> =
            Arc::new(crate::workload::LinkedHandleObserver { handle });
        node_inner.hook_observer = Some(observer);
        #[cfg(feature = "test-utils")]
        {
            node_inner.test_faults = test_faults;
        }
        Ok(Node {
            hyper: hyper_inner,
            attachment: Some(Attachment {
                node: node_inner,
                verified: None,
                v2_service_spec: None,
                package_bytes: bytes::Bytes::new(),
            }),
            pending_runtime_config: None,
            pending_linked_manifest: None,
            pending_conflict_keys: None,
            #[cfg(feature = "test-utils")]
            pending_test_faults: None,
            _state: std::marker::PhantomData,
        })
    }

    /// Link a generic [`actr_framework::Workload`] implementation
    /// (`static lib`) into this node.
    ///
    /// This is the preferred `link` path for Rust hosts: the
    /// workload is wrapped in a [`workload::WorkloadAdapter`] so that its
    /// associated [`actr_framework::MessageDispatcher`] drives inbound
    /// RPC dispatch and its hook methods are bridged into the node's
    /// observer plumbing.
    pub async fn link<W: actr_framework::Workload>(
        self,
        workload: W,
    ) -> HyperResult<Node<Attached>> {
        let handle: Arc<dyn workload::LinkedWorkloadHandle> =
            workload::WorkloadAdapter::new(workload);
        self.link_handle(handle).await
    }
}

// ── State transition: Attached → Registered ──────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
impl Node<Attached> {
    /// Use a platform-managed membership key for initial registration and all
    /// renew/reissue/re-registration proofs. No private key file is needed.
    pub fn with_realm_client_key(mut self, key: RealmClientKey) -> HyperResult<Self> {
        let node = &mut self.attachment.as_mut().expect("attached node").node;
        if key.realm() != node.config.realm {
            return Err(HyperError::Config("Realm client key realm mismatch".into()));
        }
        if node.config.realm_client_key.is_some() {
            return Err(HyperError::Config(
                "configure either a platform signer or a key file".into(),
            ));
        }
        node.realm_client_key = Some(key);
        Ok(self)
    }

    /// Add a host-side observer to an already attached node.
    ///
    /// Package-backed nodes keep their guest hook observer installed; this
    /// method chains the supplied host observer after it so shells such as
    /// mobile bindings can watch signaling/WebRTC readiness without replacing
    /// package-delivered hooks.
    pub fn with_hook_observer<W: actr_framework::Workload>(mut self, observer: W) -> Self {
        let attachment = self
            .attachment
            .as_mut()
            .expect("Node<Attached> without attachment");
        let handle: Arc<dyn workload::LinkedWorkloadHandle> =
            workload::WorkloadAdapter::new(observer);
        let observer: Arc<dyn crate::lifecycle::hooks::WorkloadHookObserver> =
            Arc::new(crate::workload::LinkedHandleObserver { handle });
        attachment.node.hook_observer = crate::lifecycle::hooks::chain_observers(
            attachment.node.hook_observer.take(),
            Some(observer),
        );
        self
    }

    /// Register with Issuer, obtain an AId credential, and inject it into this
    /// attached node. Consumes `Node<Attached>` and returns `Node<Registered>`.
    ///
    /// `realm`, `acl`, and `realm_client_key` come from the attached
    /// [`RuntimeConfig`]; `service_spec` is derived from the package's proto
    /// exports when a package-backed attach was used. Linked attachments
    /// register from the runtime config's actor metadata instead.
    ///
    /// A package registers with its signed manifest and signing key; Issuer
    /// decides from them whether it runs as its Publisher or as a development
    /// identity. No Publisher private key is involved.
    pub async fn register(self, issuer_endpoint: &str) -> HyperResult<Node<Registered>> {
        let service_spec = self.derived_service_spec()?;
        self.register_with_inner(issuer_endpoint, service_spec)
            .await
    }

    /// Adopt an already-issued credential without registering a second time.
    /// Both package and linked attachments retain their registration inputs
    /// for subsequent credential recovery.
    #[cfg(feature = "test-utils")]
    pub fn with_preregistered_credential(
        self,
        register_ok: actr_protocol::register_response::RegisterOk,
    ) -> Node<Registered> {
        self.try_with_preregistered_credential(register_ok)
            .expect("failed to derive registration context")
    }

    /// Fallible pre-registration path, including package service-spec validation.
    #[cfg(feature = "test-utils")]
    pub fn try_with_preregistered_credential(
        self,
        register_ok: actr_protocol::register_response::RegisterOk,
    ) -> HyperResult<Node<Registered>> {
        let service_spec = self.derived_service_spec()?;
        let context = self.registration_context(service_spec)?;
        Ok(self.finish_registration(register_ok, context))
    }

    fn derived_service_spec(&self) -> HyperResult<Option<ServiceSpec>> {
        let attachment = self
            .attachment
            .as_ref()
            .expect("Node<Attached> without attachment");
        match attachment
            .verified
            .as_ref()
            .map(|package| &package.manifest)
        {
            Some(RuntimePackageManifest::V2(_)) => Ok(attachment.v2_service_spec.clone()),
            Some(RuntimePackageManifest::Legacy(manifest)) => {
                crate::service_spec::calculate_service_spec_from_package(
                    &attachment.package_bytes,
                    manifest,
                )
            }
            None => Ok(None),
        }
    }

    fn registration_context(
        &self,
        service_spec: Option<ServiceSpec>,
    ) -> HyperResult<crate::lifecycle::credential_manager::RegistrationContext> {
        use crate::lifecycle::credential_manager::RegistrationContext;
        let attachment = self
            .attachment
            .as_ref()
            .expect("Node<Attached> without attachment");
        let config = &attachment.node.config;
        Ok(match attachment.verified.as_ref() {
            Some(verified) => RegistrationContext::Package {
                request: build_package_register_request(
                    verified,
                    &config.realm,
                    service_spec,
                    config.acl.clone(),
                ),
            },
            None => RegistrationContext::Linked {
                request: build_linked_register_request(config, service_spec),
            },
        })
    }

    fn finish_registration(
        mut self,
        register_ok: actr_protocol::register_response::RegisterOk,
        context: crate::lifecycle::credential_manager::RegistrationContext,
    ) -> Node<Registered> {
        let attachment = self
            .attachment
            .as_mut()
            .expect("Node<Attached> without attachment");
        attachment
            .node
            .set_preregistered_registration_context(context);
        attachment.node.set_preregistered_credential(register_ok);
        Node {
            hyper: self.hyper,
            attachment: self.attachment,
            pending_runtime_config: None,
            pending_linked_manifest: None,
            pending_conflict_keys: None,
            #[cfg(feature = "test-utils")]
            pending_test_faults: None,
            _state: std::marker::PhantomData,
        }
    }

    /// Register with Issuer using an explicit `service_spec`.
    /// The same spec is retained for credential recovery in both attachment modes.
    pub async fn register_with(
        self,
        issuer_endpoint: &str,
        service_spec: Option<ServiceSpec>,
    ) -> HyperResult<Node<Registered>> {
        if let Some(verified) = self.attachment.as_ref().and_then(|a| a.verified.as_ref())
            && matches!(&verified.manifest, RuntimePackageManifest::V2(_))
            && service_spec != self.derived_service_spec()?
        {
            return Err(HyperError::InvalidManifest(
                "v2 ServiceSpec must come from the signed package contract".into(),
            ));
        }
        self.register_with_inner(issuer_endpoint, service_spec)
            .await
    }

    async fn register_with_inner(
        self,
        issuer_endpoint: &str,
        service_spec: Option<ServiceSpec>,
    ) -> HyperResult<Node<Registered>> {
        let context = self.registration_context(service_spec.clone())?;
        let attachment = self
            .attachment
            .as_ref()
            .expect("Node<Attached> without attachment");
        let config = &attachment.node.config;
        let register_ok = if let Some(verified) = attachment.verified.as_ref() {
            bootstrap_credential_inner(
                &self.hyper,
                verified,
                CredentialBootstrapRequest {
                    issuer_endpoint,
                    realm: &config.realm,
                    service_spec,
                    acl: config.acl.clone(),
                    client_key: attachment.node.resolved_realm_client_key()?,
                },
            )
            .await?
        } else {
            bootstrap_linked_credential_inner(
                config,
                issuer_endpoint,
                service_spec,
                attachment.node.resolved_realm_client_key()?,
            )
            .await?
        };
        Ok(self.finish_registration(register_ok, context))
    }

    /// Create a network event handle for platform callbacks. Must be called
    /// before [`Node::start`].
    pub fn create_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.attachment
            .as_mut()
            .expect("Node<Attached> without attachment")
            .node
            .create_network_event_handle(debounce_ms)
    }

    /// Create a lifecycle-gated network event handle for mobile embeddings.
    /// The binding must inject an authoritative initial app phase before
    /// recovery work becomes eligible.
    pub fn create_gated_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.attachment
            .as_mut()
            .expect("Node<Attached> without attachment")
            .node
            .create_gated_network_event_handle(debounce_ms)
    }

    /// Issuer endpoint URL resolved from the attached [`RuntimeConfig`].
    /// Convenience accessor for callers that just drove `from_config_file`
    /// + `attach` and need the endpoint to pass into `register`.
    pub fn issuer_endpoint(&self) -> &str {
        &self
            .attachment
            .as_ref()
            .expect("Node<Attached> without attachment")
            .node
            .config
            .issuer_endpoint
    }
}

// ── State transition: Registered → ActrRef ───────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
impl Node<Registered> {
    /// Start the attached, registered node and return the live [`ActrRef`].
    pub async fn start(self) -> actr_protocol::ActorResult<crate::actr_ref::ActrRef> {
        let Attachment { node, .. } = self
            .attachment
            .expect("Node<Registered> without attachment");
        node.start().await
    }

    /// Create a network event handle for platform callbacks. Must be called
    /// before [`Node::start`].
    pub fn create_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.attachment
            .as_mut()
            .expect("Node<Registered> without attachment")
            .node
            .create_network_event_handle(debounce_ms)
    }

    /// Create a lifecycle-gated network event handle for mobile embeddings.
    /// The binding must inject an authoritative initial app phase before
    /// recovery work becomes eligible.
    pub fn create_gated_network_event_handle(
        &mut self,
        debounce_ms: u64,
    ) -> crate::lifecycle::NetworkEventHandle {
        self.attachment
            .as_mut()
            .expect("Node<Registered> without attachment")
            .node
            .create_gated_network_event_handle(debounce_ms)
    }
}

// ── Helpers available in all states ──────────────────────────────────────────

// ── Hyper common helpers (framework operations that don't require attachment) ─

#[cfg(not(target_arch = "wasm32"))]
impl Hyper {
    /// Resolve the storage namespace path for a verified manifest.
    ///
    /// The path is fixed here; all subsequent storage operations are isolated based on this path.
    pub fn resolve_storage_path(&self, manifest: &RuntimePackageManifest) -> HyperResult<PathBuf> {
        resolve_storage_path_for(&self.inner, manifest)
    }

    /// Bootstrap credential registration with Issuer.
    ///
    /// Hyper completes registration bootstrap on behalf of the Actor and returns the full Issuer
    /// registration payload.
    ///
    /// ## Registration Logic
    ///
    /// Package bootstrap always authenticates with the publisher-signed manifest.
    /// Credential renewal uses the renewal token returned by Issuer through
    /// `POST /issuer/renew`, not a follow-up `/register` call.
    ///
    /// ## Parameters
    ///
    /// - `verified`: verified package bundle (from `verify_package`) — carries
    ///   the parsed manifest plus the raw manifest bytes and signature needed
    ///   for phase-1 registration with Issuer.
    /// - `issuer_endpoint`: Issuer HTTP address, e.g. `"http://issuer.example.com:8080"`
    /// - `realm`: target Realm name `<realm_id>@<domain>`
    /// - `service_spec`: optional protobuf API metadata published to discovery
    /// - `acl`: optional access-control policy attached to the actor
    /// - `client_key`: the Realm client key that signs the request's client
    ///   proof
    pub async fn bootstrap_credential(
        &self,
        verified: &VerifiedRuntimePackage,
        issuer_endpoint: &str,
        realm: &str,
        service_spec: Option<ServiceSpec>,
        acl: Option<Acl>,
        client_key: Option<RealmClientKey>,
    ) -> HyperResult<register_response::RegisterOk> {
        bootstrap_credential_inner(
            &self.inner,
            verified,
            CredentialBootstrapRequest {
                issuer_endpoint,
                realm,
                service_spec,
                acl,
                client_key,
            },
        )
        .await
    }

    /// Current instance_id
    pub fn instance_id(&self) -> &str {
        &self.inner.instance_id
    }

    /// Current configuration
    pub fn config(&self) -> &HyperConfig {
        &self.inner.config
    }
}

/// Derive the opaque storage namespace for a manifest's actor identity.
///
/// This is the trust-boundary check: it validates the identity against the
/// protocol rules (`ActrType::validate`) and hashes it, so the returned value is
/// the only thing ever used to locate storage — never the raw identity strings
/// (CWE-22 / CWE-73). Shared by the direct-filesystem branch
/// ([`ActorStore::open_under`]) and the platform branch (`secret_store`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn storage_namespace_for(
    manifest: &RuntimePackageManifest,
) -> HyperResult<StorageNamespace> {
    let identity = ValidatedActorIdentity::from_actor_type(manifest.actor_type())
        .map_err(|error| HyperError::InvalidManifest(error.to_string()))?;
    Ok(StorageNamespace::actr_v1(
        identity.namespace_id().into_bytes(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_storage_path_for(
    inner: &HyperInner,
    manifest: &RuntimePackageManifest,
) -> HyperResult<PathBuf> {
    let namespace = storage_namespace_for(manifest)?;
    // Platform-backed nodes have no local SecureStorageRoot; this diagnostic
    // path only exists for the direct-filesystem branch.
    let root = inner.secure_storage_root.as_ref().ok_or_else(|| {
        HyperError::Config(
            "resolve_storage_path is only available for direct-filesystem storage".to_string(),
        )
    })?;
    root.path_for(&namespace)
}

/// Single internal entry that verifies a package AND enforces the identity
/// trust boundary, closing the hole where a custom [`TrustProvider`] returns a
/// [`VerifiedRuntimePackage`] whose manifest identity was never validated.
///
/// Both `Hyper::verify_package` and `load_workload_package_inner` must use this
/// so a malicious or buggy provider cannot smuggle an unsafe identity into
/// binary detection / workload loading / storage derivation (CWE-22 / CWE-73).
#[cfg(not(target_arch = "wasm32"))]
async fn verify_package_for_hyper(
    inner: &HyperInner,
    bytes: &[u8],
) -> HyperResult<VerifiedRuntimePackage> {
    let max_entry_bytes = inner.config.resolved_wasm_limits().max_component_bytes;
    let verified = inner
        .config
        .trust_provider
        .verify_package_bounded(bytes, max_entry_bytes)
        .await?;

    ValidatedActorIdentity::from_actor_type(verified.manifest.actor_type())
        .map_err(|error| HyperError::InvalidManifest(error.to_string()))?;

    Ok(verified)
}

/// Free-function counterpart of [`Hyper::load_workload_package`] —
/// shared by both [`Hyper::attach`] and `Node<Init>::attach` without
/// needing a `Hyper` handle to own the call.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn load_workload_package_inner(
    inner: &HyperInner,
    package: &WorkloadPackage,
) -> HyperResult<LoadedWorkload> {
    let bytes = package.bytes();
    let verified = verify_package_for_hyper(inner, bytes).await?;
    let max_bytes = inner.config.resolved_wasm_limits().max_component_bytes;
    let (binary_kind, workload, packaged_lock, v2_service_spec) = match &verified.manifest {
        RuntimePackageManifest::Legacy(manifest) => {
            let binary_kind = detect_binary_kind(manifest)?;
            let packaged_lock = if let Some(lock_file) = &manifest.lock_file {
                actr_pack::read_lock_file_bounded(bytes, &lock_file.path, max_bytes)
                    .map_err(|e| HyperError::InvalidManifest(e.to_string()))?
                    .map(|bytes| package_runtime::parse_lock(&bytes))
                    .transpose()?
            } else {
                None
            };
            let workload = match binary_kind {
                BinaryKind::Wasm => load_wasm_workload_inner(inner, bytes, manifest).await?,
                BinaryKind::DynClib => load_dynclib_workload_inner(inner, bytes, manifest)?,
            };
            (binary_kind, workload, packaged_lock, None)
        }
        RuntimePackageManifest::V2(manifest) => {
            package_runtime::validate_component_host(manifest)?;
            let packaged_lock = package_runtime::read_v2_lock(bytes, manifest, max_bytes)?;
            let service_spec = service_spec::calculate_v2_service_spec(bytes, manifest, max_bytes)?;
            let wasm_bytes = manifest
                .read_declared_file(bytes, &manifest.workload.entry, max_bytes)
                .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
            let workload = instantiate_wasm_component(
                inner,
                wasm_bytes,
                &manifest.workload.entry,
                manifest.actor_type(),
            )
            .await?;
            (BinaryKind::Wasm, workload, packaged_lock, service_spec)
        }
    };
    Ok(LoadedWorkload {
        verified,
        binary_kind,
        workload,
        packaged_lock,
        v2_service_spec,
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn load_wasm_workload_inner(
    inner: &HyperInner,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> HyperResult<crate::workload::Workload> {
    // Refuse legacy core-module packages before attempting to compile
    // them — `Component::from_binary` already rejects them downstream
    // with an opaque "unknown binary format" error, so catching the
    // case here produces a migration-pointing message instead.
    if matches!(
        manifest.binary.resolved_kind(),
        actr_pack::BinaryKind::CoreModule
    ) {
        return Err(HyperError::InvalidManifest(format!(
            "package `{}` uses the legacy core wasm module format, which was retired in Phase 1. \
                 Rebuild with actr 0.2+ (`actr build`, target wasm32-wasip2 + wasm-component-ld 0.5.22+) \
                 to produce a Component Model binary, and set `binary.kind = \"component\"` in manifest.toml.",
            manifest.actr_type_str()
        )));
    }

    let wasm_bytes = actr_pack::load_binary_bounded(
        bytes,
        inner.config.resolved_wasm_limits().max_component_bytes,
    )
    .map_err(|e| {
        HyperError::Runtime(format!(
            "failed to extract package binary `{}` for target `{}`: {e}",
            manifest.binary.path, manifest.binary.target
        ))
    })?;
    instantiate_wasm_component(
        inner,
        wasm_bytes,
        &manifest.binary.target,
        manifest.actr_type_str(),
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn instantiate_wasm_component(
    inner: &HyperInner,
    wasm_bytes: Vec<u8>,
    entry: &str,
    actr_type: String,
) -> HyperResult<crate::workload::Workload> {
    #[cfg(feature = "wasm-engine")]
    {
        let limits = inner.config.resolved_wasm_limits();
        limits.validate()?;
        // issue #346: resource limits (fuel/epoch/memory/aggregate) drive the
        // engine config + per-store limiter + per-entry re-seed; the byte cap
        // also rejects oversized components before compilation.
        let host = tokio::task::spawn_blocking(move || {
            crate::wasm::WasmHost::compile_with_limits(&wasm_bytes, &limits)
        })
        .await
        .map_err(|e| HyperError::Runtime(format!("WASM compiler task failed for `{entry}`: {e}")))?
        .map_err(|e| {
            HyperError::Runtime(format!("failed to compile WASM component `{entry}`: {e}"))
        })?;
        let mut instance = host.instantiate().await.map_err(|e| {
            HyperError::Runtime(format!(
                "failed to instantiate WASM component `{entry}`: {e}"
            ))
        })?;
        instance
            .init(&actr_framework::guest::dynclib_abi::InitPayloadV1 {
                version: actr_framework::guest::dynclib_abi::version::V1,
                actr_type,
                credential: Vec::new(),
                actor_id: Vec::new(),
                realm: String::new(),
            })
            .map_err(|e| {
                HyperError::Runtime(format!(
                    "failed to initialize WASM component `{entry}`: {e}"
                ))
            })?;
        Ok(crate::workload::Workload::Wasm(instance))
    }

    #[cfg(not(feature = "wasm-engine"))]
    {
        let _ = (inner, wasm_bytes, entry, actr_type);
        Err(HyperError::Runtime(
            "package target requires the `wasm-engine` feature, but it is not enabled".to_string(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_dynclib_workload_inner(
    _inner: &HyperInner,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> HyperResult<crate::workload::Workload> {
    #[cfg(feature = "dynclib-engine")]
    {
        let cache_path = ensure_dynclib_cache_path(&_inner.config.data_dir, bytes, manifest)?;
        let host = load_dynclib_host_with_rebuild(&cache_path, bytes, manifest)?;
        let instance = host
            .instantiate(&actr_framework::guest::dynclib_abi::InitPayloadV1 {
                version: actr_framework::guest::dynclib_abi::version::V1,
                actr_type: manifest.actr_type_str(),
                credential: Vec::new(),
                actor_id: Vec::new(),
                realm: String::new(),
            })
            .map_err(|e| {
                HyperError::Runtime(format!(
                    "failed to initialize dynclib package target `{}`: {e}",
                    manifest.binary.target
                ))
            })?;

        Ok(crate::workload::Workload::DynClib(
            crate::dynclib::DynClibWorkload::new(host, instance),
        ))
    }

    #[cfg(not(feature = "dynclib-engine"))]
    {
        let _ = (bytes, manifest);
        Err(HyperError::Runtime(
            "package target requires the `dynclib-engine` feature, but it is not enabled"
                .to_string(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct CredentialBootstrapRequest<'a> {
    issuer_endpoint: &'a str,
    realm: &'a str,
    service_spec: Option<ServiceSpec>,
    acl: Option<Acl>,
    client_key: Option<RealmClientKey>,
}

#[cfg(not(target_arch = "wasm32"))]
async fn bootstrap_credential_inner(
    inner: &HyperInner,
    verified: &VerifiedRuntimePackage,
    request: CredentialBootstrapRequest<'_>,
) -> HyperResult<register_response::RegisterOk> {
    let CredentialBootstrapRequest {
        issuer_endpoint,
        realm,
        service_spec,
        acl,
        client_key,
    } = request;
    let manifest = &verified.manifest;
    let actr_type = manifest.actor_type().to_program_identity();
    info!(
        actr_type,
        issuer_endpoint, realm, "starting credential bootstrap with Issuer"
    );

    // 1. Derive and validate the opaque namespace (pure; no I/O, no database).
    // The raw identity never becomes a path component. The store is opened only
    // AFTER the Issuer registration succeeds, so a rejected identity leaves no
    // database behind (issue #350 acceptance; repeated rejected identities must
    // not accumulate empty databases).
    let namespace = storage_namespace_for(manifest)?;

    // 2. Build RegisterRequest and send to Issuer.
    // Package bootstrap always authenticates with manifest + publisher signature.
    // (Legacy PSK renewal path removed — credential renewal now goes through
    // the Credential Manager via POST /issuer/renew.)
    let mut issuer = IssuerClient::new(issuer_endpoint);
    if let Some(key) = client_key {
        issuer = issuer.with_client_key(key);
    }

    info!(
        actr_type,
        "registering with Issuer using publisher manifest"
    );

    let req = build_package_register_request(verified, realm, service_spec, acl);
    let response = issuer.register_with_manifest(req).await?;

    // 3. Process Issuer response
    let ok = match response.result {
        Some(register_response::Result::Success(ok)) => ok,
        Some(register_response::Result::Error(e)) => {
            error!(
                actr_type,
                error_code = e.code,
                error_message = %e.message,
                "Issuer registration returned error"
            );
            return Err(HyperError::IssuerBootstrapFailed(format!(
                "Issuer rejected registration (code={}): {}",
                e.code, e.message
            )));
        }
        None => {
            error!(actr_type, "Issuer response missing result field");
            return Err(HyperError::IssuerBootstrapFailed(
                "Issuer response missing result field".to_string(),
            ));
        }
    };

    // 4. Registration succeeded — only now open the actor's secret store, so a
    // rejected registration never creates a database.
    let store: Arc<dyn KvStore> = if let Some(ref platform) = inner.platform {
        platform
            .secret_store(&namespace)
            .await
            .map_err(|e| HyperError::Storage(format!("failed to open secret store: {e}")))?
    } else {
        let root = inner.secure_storage_root.as_ref().ok_or_else(|| {
            HyperError::Storage(
                "no secure storage root available for direct-filesystem branch".to_string(),
            )
        })?;
        Arc::new(ActorStore::open_under(root, &namespace).await?)
    };

    // 5. Store signing_pubkey + signing_key_id (for IssuerKeyCache use)
    let pubkey_bytes = ok.signing_pubkey.to_vec();
    let key_id_bytes = ok.signing_key_id.to_le_bytes().to_vec();
    store
        .batch(vec![
            KvOp::Set {
                key: "hyper:issuer:signing_pubkey".to_string(),
                value: pubkey_bytes,
            },
            KvOp::Set {
                key: "hyper:issuer:signing_key_id".to_string(),
                value: key_id_bytes,
            },
        ])
        .await
        .map_err(|e| HyperError::Storage(format!("failed to store signing key: {e}")))?;
    debug!(
        actr_type,
        signing_key_id = ok.signing_key_id,
        "Issuer signing public key persisted to ActorStore"
    );

    info!(
        actr_type,
        credential_len = ok.credential.encode_to_vec().len(),
        "Issuer credential bootstrap succeeded"
    );

    Ok(ok)
}

#[cfg(not(target_arch = "wasm32"))]
async fn bootstrap_linked_credential_inner(
    config: &actr_config::RuntimeConfig,
    issuer_endpoint: &str,
    service_spec: Option<ServiceSpec>,
    client_key: Option<RealmClientKey>,
) -> HyperResult<register_response::RegisterOk> {
    let mut issuer = IssuerClient::new(issuer_endpoint);
    if let Some(key) = client_key {
        issuer = issuer.with_client_key(key);
    }

    let req = build_linked_register_request(config, service_spec);
    let response = issuer.register_linked(req).await?;
    match response.result {
        Some(register_response::Result::Success(ok)) => Ok(ok),
        Some(register_response::Result::Error(e)) => {
            Err(HyperError::IssuerBootstrapFailed(format!(
                "Issuer rejected registration (code={}): {}",
                e.code, e.message
            )))
        }
        None => Err(HyperError::IssuerBootstrapFailed(
            "Issuer response missing result field".to_string(),
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_package_register_request(
    verified: &VerifiedRuntimePackage,
    realm: &str,
    service_spec: Option<ServiceSpec>,
    acl: Option<Acl>,
) -> RegisterRequest {
    RegisterRequest {
        actr_type: verified.manifest.actor_type(),
        realm: realm.to_string(),
        service_spec,
        acl,
        service: None,
        ws_address: None,
        manifest_raw: Some(verified.manifest_raw.clone().into()),
        publisher_signature: Some(verified.sig_raw.clone().into()),
        target: match &verified.manifest {
            RuntimePackageManifest::V2(_) => None,
            RuntimePackageManifest::Legacy(manifest) => Some(manifest.binary.target.clone()),
        },
        auth_mode: Some(RegisterAuthMode::Package as i32),
        signing_public_key: Some(verified.signing_public_key.to_vec().into()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_linked_register_request(
    config: &actr_config::RuntimeConfig,
    service_spec: Option<ServiceSpec>,
) -> RegisterRequest {
    RegisterRequest {
        actr_type: config.actr_type().clone(),
        realm: config.realm.clone(),
        service_spec,
        acl: config.acl.clone(),
        service: None,
        ws_address: None,
        auth_mode: Some(RegisterAuthMode::Linked as i32),
        ..Default::default()
    }
}

// ─── Helper functions (native-only) ──────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[cfg(not(target_arch = "wasm32"))]
fn detect_binary_kind(manifest: &PackageManifest) -> HyperResult<BinaryKind> {
    if manifest.binary.is_wasm_target() {
        return Ok(BinaryKind::Wasm);
    }

    if is_compatible_native_target(&manifest.binary.target) {
        return Ok(BinaryKind::DynClib);
    }

    Err(HyperError::InvalidManifest(format!(
        "unsupported binary target `{}` for host `{}-{}`; expected `wasm32-*` or a native target matching this host",
        manifest.binary.target,
        std::env::consts::ARCH,
        std::env::consts::OS,
    )))
}

/// Check that `target` is a valid Rust target triple compatible with the current host.
///
/// A target triple has at least 3 segments (arch-vendor-os or arch-vendor-os-env).
/// We verify that the arch and OS components match the running host to reject
/// cross-platform cdylib packages early, rather than failing at `dlopen` time.
#[cfg(not(target_arch = "wasm32"))]
fn is_compatible_native_target(target: &str) -> bool {
    let segments: Vec<&str> = target.split('-').filter(|s| !s.is_empty()).collect();
    if segments.len() < 3 {
        return false;
    }

    let target_arch = segments[0];
    // OS is typically the third segment (arch-vendor-os[-env]).
    let target_os = segments[2];

    // Normalize arch names: Rust target triples use different names than std::env::consts::ARCH.
    let arch_matches = match (target_arch, std::env::consts::ARCH) {
        (a, b) if a == b => true,
        ("x86_64", "x86_64") => true,
        ("aarch64", "aarch64") => true,
        _ => false,
    };

    // Normalize OS names: Rust target triples use e.g. "darwin" while consts::OS is "macos".
    let os_matches = match (target_os, std::env::consts::OS) {
        (a, b) if a == b => true,
        ("darwin", "macos") | ("macos", "darwin") => true,
        _ => false,
    };

    arch_matches && os_matches
}

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "dynclib-engine",
    target_os = "macos"
))]
fn dynclib_tempfile_suffix() -> &'static str {
    ".dylib"
}

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "dynclib-engine",
    target_os = "linux"
))]
fn dynclib_tempfile_suffix() -> &'static str {
    ".so"
}

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "dynclib-engine",
    target_os = "windows"
))]
fn dynclib_tempfile_suffix() -> &'static str {
    ".dll"
}

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "dynclib-engine",
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
fn dynclib_tempfile_suffix() -> &'static str {
    ".dynlib"
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
const DYNCLIB_CACHE_DIR: &str = "dynclib-cache";

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn dynclib_cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DYNCLIB_CACHE_DIR)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn dynclib_cache_path(data_dir: &Path, binary_hash: &[u8; 32]) -> PathBuf {
    dynclib_cache_dir(data_dir).join(format!(
        "{}{}",
        hex::encode(binary_hash),
        dynclib_tempfile_suffix()
    ))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn extract_dynclib_binary(bytes: &[u8], manifest: &PackageManifest) -> HyperResult<Vec<u8>> {
    actr_pack::load_binary(bytes).map_err(|e| {
        HyperError::Runtime(format!(
            "failed to extract package binary `{}` for target `{}`: {e}",
            manifest.binary.path, manifest.binary.target
        ))
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn write_dynclib_cache_file(cache_path: &Path, binary_bytes: &[u8]) -> HyperResult<()> {
    let cache_dir = cache_path.parent().ok_or_else(|| {
        HyperError::Runtime("dynclib cache path has no parent directory".to_string())
    })?;
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        HyperError::Runtime(format!(
            "failed to create dynclib cache directory `{}`: {e}",
            cache_dir.display()
        ))
    })?;

    let mut temp_file = tempfile::Builder::new()
        .prefix("actr-dynclib-")
        .tempfile_in(cache_dir)
        .map_err(|e| {
            HyperError::Runtime(format!(
                "failed to allocate dynclib cache temp file in `{}`: {e}",
                cache_dir.display()
            ))
        })?;

    temp_file.write_all(binary_bytes).map_err(|e| {
        HyperError::Runtime(format!(
            "failed to write dynclib cache temp file `{}`: {e}",
            temp_file.path().display()
        ))
    })?;
    temp_file.flush().map_err(|e| {
        HyperError::Runtime(format!(
            "failed to flush dynclib cache temp file `{}`: {e}",
            temp_file.path().display()
        ))
    })?;

    match temp_file.persist_noclobber(cache_path) {
        Ok(_) => Ok(()),
        Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(HyperError::Runtime(format!(
            "failed to persist dynclib cache file `{}`: {}",
            cache_path.display(),
            err.error
        ))),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn ensure_dynclib_cache_path(
    data_dir: &Path,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> HyperResult<PathBuf> {
    let binary_hash = manifest
        .binary
        .hash_bytes()
        .map_err(|e| HyperError::InvalidManifest(e.to_string()))?;
    let cache_path = dynclib_cache_path(data_dir, &binary_hash);
    if cache_path.exists() {
        return Ok(cache_path);
    }

    let binary_bytes = extract_dynclib_binary(bytes, manifest)?;
    write_dynclib_cache_file(&cache_path, &binary_bytes)?;
    Ok(cache_path)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn rebuild_dynclib_cache_file(
    cache_path: &Path,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> HyperResult<()> {
    match std::fs::remove_file(cache_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(HyperError::Runtime(format!(
                "failed to remove corrupt dynclib cache file `{}`: {err}",
                cache_path.display()
            )));
        }
    }

    let binary_bytes = extract_dynclib_binary(bytes, manifest)?;
    write_dynclib_cache_file(cache_path, &binary_bytes)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "dynclib-engine"))]
fn load_dynclib_host_with_rebuild(
    cache_path: &Path,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> HyperResult<crate::dynclib::DynclibHost> {
    match crate::dynclib::DynclibHost::load(cache_path) {
        Ok(host) => Ok(host),
        Err(first_err) => {
            warn!(
                path = %cache_path.display(),
                target = %manifest.binary.target,
                error = %first_err,
                "cached dynclib load failed, rebuilding cache once"
            );
            rebuild_dynclib_cache_file(cache_path, bytes, manifest)?;
            crate::dynclib::DynclibHost::load(cache_path).map_err(|second_err| {
                HyperError::Runtime(format!(
                    "failed to load dynclib package target `{}` from cache `{}` after rebuild; first load error: {first_err}; second load error: {second_err}",
                    manifest.binary.target,
                    cache_path.display()
                ))
            })
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Load an existing `instance_uid` or generate and persist a new one.
///
/// Used only when no `PlatformProvider` is injected; otherwise the provider's
/// `instance_uid()` is the source of truth.
async fn load_or_create_instance_uid_local(data_dir: &std::path::Path) -> HyperResult<String> {
    let id_file = data_dir.join(".hyper-instance-uid");

    if id_file.exists() {
        let id = tokio::fs::read_to_string(&id_file)
            .await
            .map_err(|e| HyperError::Storage(format!("failed to read instance_uid file: {e}")))?;
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
        warn!("instance_uid file is empty; generating a new one");
    }

    let new_id = Uuid::new_v4().to_string();
    tokio::fs::write(&id_file, &new_id)
        .await
        .map_err(|e| HyperError::Storage(format!("failed to write instance_uid file: {e}")))?;
    info!(instance_uid = %new_id, "generated a new Hyper instance_uid");
    Ok(new_id)
}

#[cfg(all(not(target_arch = "wasm32"), test))]
#[path = "lib_tests.rs"]
mod tests;

// The connection-governance suites exercise private production seams (for
// example, physical-session retirement) and therefore run as in-crate tests.
// Keeping their sources under `tests/support` avoids exposing those controls
// through the feature-selectable `test_support` module.
#[cfg(all(not(target_arch = "wasm32"), test, feature = "test-utils"))]
extern crate self as actr_hyper;

#[cfg(all(not(target_arch = "wasm32"), test, feature = "test-utils"))]
#[path = "../tests/support/governance.rs"]
mod connection_governance_support;

#[cfg(all(not(target_arch = "wasm32"), test, feature = "test-utils"))]
#[path = "../tests/support/connection_governance_matrix.rs"]
mod connection_governance_matrix;

#[cfg(all(not(target_arch = "wasm32"), test, feature = "test-utils"))]
#[path = "../tests/support/connection_governance_production.rs"]
mod connection_governance_production;

#[cfg(all(not(target_arch = "wasm32"), test, feature = "test-utils"))]
#[path = "../tests/support/connection_governance_inflight.rs"]
mod connection_governance_inflight;
