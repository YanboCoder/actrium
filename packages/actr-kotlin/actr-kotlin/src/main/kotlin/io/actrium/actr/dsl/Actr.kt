/**
 * Actrium Kotlin SDK
 *
 * A Kotlin-idiomatic wrapper for the Actrium framework.
 *
 * Example usage:
 * ```kotlin
 * // Create and start a package-backed actor
 * val node = ActrNode.fromPackageFile("config.toml", "dist/app.actr")
 * val ref = node.start()
 *
 * // Discover and call remote services
 * val echoService = ref.discoverOne("acme/EchoService@1.0.0")
 * val response = ref.call("echo.EchoService.Echo", request)
 *
 * // Send data stream
 * ref.sendStream(target) {
 *     streamId = "stream-001"
 *     sequence = 0uL
 *     payload = data
 *     metadata {
 *         "content-type" to "application/octet-stream"
 *     }
 * }
 *
 * // Clean shutdown
 * ref.stop()
 * ```
 */
package io.actrium.actr.dsl

import android.content.Context
import io.actrium.actr.ActrException
import io.actrium.actr.ActrId
import io.actrium.actr.ActrRefWrapper
import io.actrium.actr.ActrType
import io.actrium.actr.CleanupReason
import io.actrium.actr.PayloadType
import io.actrium.actr.ReconnectReason
import io.actrium.actr.ActrNode as ActrNodeGenerated
import kotlinx.coroutines.CoroutineScope
import java.net.URL

// ============================================================================
// Runtime Observers — package-backed host observer convenience
// ============================================================================

// ---------------------------------------------------------------------------
// Maintenance: runtimeObservers(...) below and dynamicWorkload(...) in Workload.kt
// hand-maintain the same 5 observer slots (signaling/websocket/webrtc/credential/mailbox).
// Any new UniFFI observer slot must be added to both so the factory shapes stay aligned.
// ---------------------------------------------------------------------------

/**
 * Construct host-side [RuntimeObservers] for a package-backed runtime.
 *
 * Mobile shells observe transport readiness for UI state and intent retry
 * decisions while the package guest continues to own actor dispatch. Pass the
 * result to [ActrNode.fromPackageFile] or [ActrNode.fromPackageFileWithMonitoring].
 *
 * Each slot is optional; omit (or pass `null`) for observers you do not need.
 *
 * Example:
 * ```kotlin
 * val observers = runtimeObservers(
 *     webrtc = object : WebRtcObserver {
 *         override suspend fun onConnecting(ctx: ActrContext, event: PeerEvent) { /* … */ }
 *         override suspend fun onConnected(ctx: ActrContext, event: PeerEvent) { /* … */ }
 *         override suspend fun onDisconnected(ctx: ActrContext, event: PeerEvent) { /* … */ }
 *     },
 * )
 * val node = ActrNode.fromPackageFile("config.toml", "dist/app.actr", observers = observers)
 * ```
 *
 * @param signaling Optional signaling-layer observer
 * @param websocket Optional WebSocket peer observer
 * @param webrtc Optional WebRTC peer observer
 * @param credential Optional credential lifecycle observer
 * @param mailbox Optional mailbox backpressure observer
 * @return A new [RuntimeObservers] instance
 */
fun runtimeObservers(
    signaling: SignalingObserver? = null,
    websocket: WebSocketObserver? = null,
    webrtc: WebRtcObserver? = null,
    credential: CredentialObserver? = null,
    mailbox: MailboxObserver? = null,
): RuntimeObservers =
    RuntimeObservers(
        signaling = signaling,
        websocket = websocket,
        webrtc = webrtc,
        credential = credential,
        mailbox = mailbox,
    )

// ============================================================================
// ActrNode — high-level wrapper with workload retention
// ============================================================================

/**
 * Entry point for creating and starting ACTR nodes.
 *
 * This is a high-level wrapper around the UniFFI-generated [ActrNodeGenerated]
 * that manages workload lifecycle and retains references to prevent premature
 * garbage collection.
 *
 * Use [ActrNode.fromPackageFile] or [ActrNode.linked] to create an instance.
 */
class ActrNode private constructor(
    private val inner: ActrNodeGenerated,
    private val retainedWorkload: DynamicWorkload? = null,
    /** Defense-in-depth retention mirroring [retainedWorkload]. UniFFI's callback-interface
     *  handle map is what actually keeps host observer callbacks alive, so this field is not
     *  required for callback liveness — it is retained for lifecycle symmetry with the workload
     *  model and follows the wrapper object's lifetime (close() does not clear it). */
    private val retainedObservers: RuntimeObservers? = null,
    private val networkResources: ManagedNetworkResources? = null,
) : AutoCloseable {
    /** Close the underlying node, releasing native resources. */
    override fun close() {
        networkResources?.close()
        inner.close()
    }

    companion object {
        /**
         * Create a package-backed node from config and package file paths.
         *
         * Example:
         * ```kotlin
         * val node = ActrNode.fromPackageFile("config.toml", "dist/app.actr")
         * val ref = node.start()
         * ```
         *
         * @param configPath Path to the package runtime `actr.toml`
         * @param packagePath Path to the `.actr` package file
         * @param observers Optional host-side [RuntimeObservers] for transport readiness; when
         * non-null the node is built with the observer-aware constructor so the package guest
         * keeps owning dispatch while the shell observes signaling/WebRTC/etc.
         * @return A new ActrNode instance
         * @throws ActrException.Config if the config file is invalid
         */
        suspend fun fromPackageFile(
            configPath: String,
            packagePath: String,
            observers: RuntimeObservers? = null,
            membership: io.actrium.actr.RealmMembership? = null,
        ): ActrNode {
            val inner = if (membership == null) createPackageBackedNode(configPath, packagePath, observers)
                else ActrNodeGenerated.newFromPackageFileWithMembership(configPath, packagePath, membership, observers)
            return ActrNode(inner, retainedObservers = observers)
        }

        /**
         * Create a package-backed node and start Android network monitoring.
         *
         * The returned [ActrNode] retains the [NetworkEventHandle] and
         * [NetworkMonitor], so callers do not need to manually create or hold
         * those objects.
         *
         * @param observers Optional host-side [RuntimeObservers] for transport readiness
         */
        suspend fun fromPackageFileWithMonitoring(
            configPath: String,
            packagePath: String,
            context: Context,
            scope: CoroutineScope,
            onNetworkStatusLog: ((String) -> Unit)? = null,
            observers: RuntimeObservers? = null,
            membership: io.actrium.actr.RealmMembership? = null,
        ): ActrNode {
            val inner = if (membership == null) createPackageBackedNode(configPath, packagePath, observers)
                else ActrNodeGenerated.newFromPackageFileWithMembership(configPath, packagePath, membership, observers)
            return withNetworkMonitoring(
                inner = inner,
                retainedWorkload = null,
                retainedObservers = observers,
                context = context,
                scope = scope,
                onNetworkStatusLog = onNetworkStatusLog,
            )
        }

        /**
         * Create a linked/static node from config, explicit actor identity, and a
         * Kotlin-provided workload.
         *
         * Use this when workload logic lives in Kotlin instead of a packaged `.actr`
         * guest. The returned [ActrNode] retains the [workload] reference to prevent
         * premature garbage collection.
         *
         * Example:
         * ```kotlin
         * val workload = dynamicWorkload(myLifecycle)
         * val node = ActrNode.linked("manifest.toml", myType, workload)
         * val ref = node.start()
         * ```
         *
         * @param configPath Path to `manifest.toml` for dynamic dependency
         * resolution (its sibling `actr.toml` supplies runtime settings), or
         * directly to `actr.toml` for a dependency-free linked workload
         * @param actorType The actor's type identity
         * @param workload The composed workload (lifecycle + optional observers)
         * @return A new ActrNode instance that retains the workload
         * @throws ActrException.Config if the config file is invalid
         */
        suspend fun linked(
            configPath: String,
            actorType: ActrType,
            workload: DynamicWorkload,
            membership: io.actrium.actr.RealmMembership? = null,
        ): ActrNode {
            val inner = if (membership == null) ActrNodeGenerated.newFromLinkedWorkload(configPath, actorType, workload)
                else ActrNodeGenerated.newFromLinkedWorkloadWithMembership(configPath, actorType, workload, membership)
            return ActrNode(inner, workload)
        }

        /**
         * Create a linked/static node and start Android network monitoring.
         *
         * Use this when workload logic lives in Kotlin and you want the node to
         * own the network event handle and Android network monitor.
         */
        suspend fun linkedWithMonitoring(
            configPath: String,
            actorType: ActrType,
            workload: DynamicWorkload,
            context: Context,
            scope: CoroutineScope,
            onNetworkStatusLog: ((String) -> Unit)? = null,
            membership: io.actrium.actr.RealmMembership? = null,
        ): ActrNode {
            val inner = if (membership == null) ActrNodeGenerated.newFromLinkedWorkload(configPath, actorType, workload)
                else ActrNodeGenerated.newFromLinkedWorkloadWithMembership(configPath, actorType, workload, membership)
            return withNetworkMonitoring(
                inner = inner,
                retainedWorkload = workload,
                retainedObservers = null,
                context = context,
                scope = scope,
                onNetworkStatusLog = onNetworkStatusLog,
            )
        }

        /**
         * Create a package-backed node from config and package file URLs.
         *
         * Validates that both URLs are file URLs before delegating to
         * [fromPackageFile] with the URL paths.
         *
         * @param configURL File URL to the package runtime `actr.toml`
         * @param packageURL File URL to the `.actr` package file
         * @param observers Optional host-side [RuntimeObservers] for transport readiness; passed through to the path-based overload
         * @return A new ActrNode instance
         * @throws IllegalArgumentException if either URL is not a file URL
         */
        suspend fun fromPackageFile(
            configURL: URL,
            packageURL: URL,
            observers: RuntimeObservers? = null,
        ): ActrNode {
            require(configURL.protocol == "file") {
                "configURL must be a file URL, got: $configURL"
            }
            require(packageURL.protocol == "file") {
                "packageURL must be a file URL, got: $packageURL"
            }
            return fromPackageFile(configURL.path, packageURL.path, observers)
        }

        /**
         * Create a monitored package-backed node from config and package file URLs.
         *
         * @param observers Optional host-side [RuntimeObservers] for transport readiness; passed through to the path-based overload
         */
        suspend fun fromPackageFileWithMonitoring(
            configURL: URL,
            packageURL: URL,
            context: Context,
            scope: CoroutineScope,
            onNetworkStatusLog: ((String) -> Unit)? = null,
            observers: RuntimeObservers? = null,
        ): ActrNode {
            require(configURL.protocol == "file") {
                "configURL must be a file URL, got: $configURL"
            }
            require(packageURL.protocol == "file") {
                "packageURL must be a file URL, got: $packageURL"
            }
            return fromPackageFileWithMonitoring(
                configPath = configURL.path,
                packagePath = packageURL.path,
                context = context,
                scope = scope,
                onNetworkStatusLog = onNetworkStatusLog,
                observers = observers,
            )
        }

        /**
         * Create a linked node from a config file URL.
         *
         * @param configURL File URL to `manifest.toml`, or to `actr.toml` for
         * a dependency-free linked workload
         * @param actorType The actor's type identity
         * @param workload The composed workload
         * @return A new ActrNode instance that retains the workload
         * @throws IllegalArgumentException if the URL is not a file URL
         */
        suspend fun linked(
            configURL: URL,
            actorType: ActrType,
            workload: DynamicWorkload,
        ): ActrNode {
            require(configURL.protocol == "file") {
                "config URL must be a file URL, got: $configURL"
            }
            return linked(configURL.path, actorType, workload)
        }

        /**
         * Create a monitored linked/static node from a config file URL.
         */
        suspend fun linkedWithMonitoring(
            configURL: URL,
            actorType: ActrType,
            workload: DynamicWorkload,
            context: Context,
            scope: CoroutineScope,
            onNetworkStatusLog: ((String) -> Unit)? = null,
        ): ActrNode {
            require(configURL.protocol == "file") {
                "config URL must be a file URL, got: $configURL"
            }
            return linkedWithMonitoring(
                configPath = configURL.path,
                actorType = actorType,
                workload = workload,
                context = context,
                scope = scope,
                onNetworkStatusLog = onNetworkStatusLog,
            )
        }

        /**
         * Build the native package-backed node, routing through the observer-aware
         * constructor when [observers] is supplied. Shared by [fromPackageFile] and
         * [fromPackageFileWithMonitoring] so the dispatch lives in one place.
         */
        private suspend fun createPackageBackedNode(
            configPath: String,
            packagePath: String,
            observers: RuntimeObservers?,
        ): ActrNodeGenerated =
            if (observers != null) {
                ActrNodeGenerated.newFromPackageFileWithObservers(configPath, packagePath, observers)
            } else {
                ActrNodeGenerated.newFromPackageFile(configPath, packagePath)
            }

        private suspend fun withNetworkMonitoring(
            inner: ActrNodeGenerated,
            retainedWorkload: DynamicWorkload?,
            retainedObservers: RuntimeObservers?,
            context: Context,
            scope: CoroutineScope,
            onNetworkStatusLog: ((String) -> Unit)?,
        ): ActrNode =
            try {
                val handle = inner.createNetworkEventHandle()
                val monitor =
                    NetworkMonitor.createWithHandle(
                        context = context,
                        scope = scope,
                        getHandle = { handle },
                        onNetworkStatusLog = onNetworkStatusLog,
                    )
                monitor.startMonitoring()
                ActrNode(
                    inner = inner,
                    retainedWorkload = retainedWorkload,
                    retainedObservers = retainedObservers,
                    networkResources =
                        ManagedNetworkResources(
                            handle = handle,
                            monitor = NetworkMonitorLifecycleAdapter(monitor),
                        ),
                )
            } catch (error: Throwable) {
                inner.close()
                throw error
            }
    }

    /**
     * Create a network event handle for platform callbacks.
     *
     * This handle is used to notify the actor runtime about network state changes,
     * app lifecycle transitions, and explicit cleanup/reconnect operations, which
     * are important for WebRTC connection management on mobile platforms.
     *
     * Example:
     * ```kotlin
     * val networkHandle = node.createNetworkEventHandle()
     *
     * // Notify full network path change
     * networkHandle.handleNetworkPathChanged(
     *     NetworkSnapshot(
     *         sequence = 1uL,
     *         availability = NetworkAvailability.AVAILABLE,
     *         transport = NetworkTransportFlags(wifi = true, cellular = false, ethernet = false, vpn = false, other = false),
     *         isExpensive = false,
     *         isConstrained = false,
     *     )
     * )
     * ```
     *
     * @return A new NetworkEventHandle instance
     * @throws ActrException if the handle cannot be created
     */
    suspend fun createNetworkEventHandle(): NetworkEventHandle =
        networkResources?.handle ?: inner.createNetworkEventHandle()

    /**
     * Allocate one source epoch for a newly attached/restarted Android monitor.
     *
     * Unlike [createNetworkEventHandle], this intentionally bypasses the
     * retained monitor handle so each monitor incarnation receives a fresh
     * `(source_epoch, sequence)` domain.
     */
    internal suspend fun createNetworkMonitorEventHandle(): NetworkEventHandle =
        inner.createNetworkEventHandle()

    /** Notify the retained Android monitor that the app moved to background. */
    fun onAppBackground() {
        networkResources?.onAppBackground()
    }

    /** Notify the retained Android monitor that the app returned to foreground. */
    fun onAppForeground() {
        networkResources?.onAppForeground()
    }

    /** Request cleanup on the retained network event handle. */
    fun cleanupConnections(reason: CleanupReason = CleanupReason.MANUAL_RESET) {
        networkResources?.cleanupConnections(reason)
    }

    /** Request cleanup and reconnect on the retained network event handle. */
    fun forceReconnect(reason: ReconnectReason = ReconnectReason.MANUAL_RECONNECT) {
        networkResources?.forceReconnect(reason)
    }

    /** Trigger an immediate network snapshot from the retained Android monitor. */
    fun triggerNetworkCheck() {
        networkResources?.triggerNetworkCheck()
    }

    /** Return the retained Android monitor's current network status, if present. */
    fun getCurrentNetworkStatus(): String? = networkResources?.getCurrentNetworkStatus()

    /**
     * Start the actor and return a running reference.
     *
     * The returned [ActrRef] retains the workload (if any) to prevent premature
     * garbage collection.
     *
     * @return A running [ActrRef] instance
     * @throws ActrException if startup fails
     */
    suspend fun start(): ActrRef {
        val ref = inner.start()
        return ActrRef(ref, retainedWorkload, retainedObservers, networkResources)
    }

    /**
     * Execute a block with a started actor, ensuring proper cleanup.
     *
     * The actor is automatically shut down after the block completes, even if
     * an exception is thrown.
     *
     * Example:
     * ```kotlin
     * node.withStartedActor { ref ->
     *     val target = ref.discoverOne("acme/EchoService@1.0.0")
     *     val response = ref.call("echo.EchoService.Echo", payload)
     * }
     * // Actor is automatically shut down after the block
     * ```
     */
    suspend fun <T> withStartedActor(block: suspend (ActrRef) -> T): T {
        val ref = start()
        return try {
            block(ref)
        } finally {
            try {
                ref.stop()
            } catch (_: Exception) {
                // Ignore cleanup errors
            }
        }
    }
}

// ============================================================================
// ActrRef — high-level wrapper with workload retention
// ============================================================================

/**
 * Reference to a running actor.
 *
 * This is a high-level wrapper around the UniFFI-generated [ActrRefWrapper]
 * that provides:
 * - Convenience methods with default parameters
 * - Workload retention to prevent premature garbage collection
 * - Scoped lifecycle helpers
 *
 * Methods:
 * - [call] / [tell] — RPC communication
 * - [discover] / [discoverOne] — Service discovery
 * - [stop] / [shutdown] — Graceful shutdown
 */
class ActrRef internal constructor(
    private val inner: ActrRefWrapper,
    internal val retainedWorkload: DynamicWorkload? = null,
    /** Defense-in-depth retention mirroring [retainedWorkload]. UniFFI's callback-interface
     *  handle map is what actually keeps host observer callbacks alive, so this field is not
     *  required for callback liveness — it is retained for lifecycle symmetry with the workload
     *  model. See [ActrNode.retainedObservers]. */
    internal val retainedObservers: RuntimeObservers? = null,
    private val retainedNetworkResources: ManagedNetworkResources? = null,
) : AutoCloseable {
    /** Close the underlying reference, releasing native resources. */
    override fun close() {
        retainedNetworkResources?.close()
        inner.close()
    }

    /** Get the actor's unique identifier. */
    fun actorId(): ActrId = inner.actorId()

    /**
     * Perform an RPC call with explicit parameters.
     *
     * For most use cases, prefer the convenience overload:
     * ```kotlin
     * ref.call("echo.EchoService.Echo", requestPayload)
     * ```
     */
    suspend fun call(
        routeKey: String,
        payloadType: PayloadType,
        requestPayload: ByteArray,
        timeoutMs: Long,
    ): ByteArray = inner.call(routeKey, payloadType, requestPayload, timeoutMs)

    /**
     * Send a one-way message (fire-and-forget) with explicit parameters.
     *
     * For most use cases, prefer the convenience overload:
     * ```kotlin
     * ref.tell("echo.EchoService.Notify", messagePayload)
     * ```
     */
    suspend fun tell(
        routeKey: String,
        payloadType: PayloadType,
        messagePayload: ByteArray,
    ) = inner.tell(routeKey, payloadType, messagePayload)

    /** Discover actors of the specified type. */
    suspend fun discover(
        targetType: ActrType,
        count: UInt,
    ): List<ActrId> = inner.discover(targetType, count)

    /** Resolve a dependency alias declared by the workload metadata. */
    suspend fun resolveDependency(alias: String): ActrType = inner.resolveDependency(alias)

    /** Check if the actor is shutting down. */
    fun isShuttingDown(): Boolean = inner.isShuttingDown()

    /** Whether this actor reference is still valid (not destroyed). */
    val isActive: Boolean
        get() = !isShuttingDown()

    /** Trigger shutdown. */
    fun shutdown() = inner.shutdown()

    /** Wait for shutdown to complete. */
    suspend fun waitForShutdown() = inner.waitForShutdown()

    /** Notify the retained Android monitor that the app moved to background. */
    fun onAppBackground() {
        retainedNetworkResources?.onAppBackground()
    }

    /** Notify the retained Android monitor that the app returned to foreground. */
    fun onAppForeground() {
        retainedNetworkResources?.onAppForeground()
    }

    /** Request cleanup on the retained network event handle. */
    fun cleanupConnections(reason: CleanupReason = CleanupReason.MANUAL_RESET) {
        retainedNetworkResources?.cleanupConnections(reason)
    }

    /** Request cleanup and reconnect on the retained network event handle. */
    fun forceReconnect(reason: ReconnectReason = ReconnectReason.MANUAL_RECONNECT) {
        retainedNetworkResources?.forceReconnect(reason)
    }

    /** Trigger an immediate network snapshot from the retained Android monitor. */
    fun triggerNetworkCheck() {
        retainedNetworkResources?.triggerNetworkCheck()
    }

    /** Return the retained Android monitor's current network status, if present. */
    fun getCurrentNetworkStatus(): String? = retainedNetworkResources?.getCurrentNetworkStatus()

    /**
     * Shut down the actor and wait for it to terminate.
     *
     * This is the recommended way to stop an actor. Equivalent to:
     * ```kotlin
     * ref.shutdown()
     * ref.waitForShutdown()
     * ```
     */
    suspend fun stop() {
        try {
            shutdown()
            waitForShutdown()
        } finally {
            retainedNetworkResources?.close()
        }
    }
}

// ============================================================================
// Global Log Callback
// ============================================================================

/**
 * Set or clear the global log callback.
 *
 * Must be called **before** the actr node is created. The tracing subscriber
 * is locked during node initialization; calls after that point are ignored.
 * Pass `null` to disable forwarding.
 *
 * Example:
 * ```kotlin
 * setLogCallback(object : LogCallback {
 *     override fun onLog(level: String, target: String, message: String, timestampMs: Long) {
 *         Log.d("actr", "[$level] $target: $message")
 *     }
 * })
 * ```
 *
 * @param callback The log callback implementation, or null to clear
 */
fun setLogCallback(callback: LogCallback?) {
    io.actrium.actr.setLogCallback(callback)
}

// ============================================================================
// Top-Level Convenience Functions
// ============================================================================

/**
 * Create an ActrNode from a config file and package file (top-level function).
 *
 * Example:
 * ```kotlin
 * val node = createActrNode("config.toml", "dist/app.actr")
 * ```
 *
 * @param configPath Path to the package runtime `actr.toml`
 * @param packagePath Path to the `.actr` package file
 * @param observers Optional host-side [RuntimeObservers] for transport readiness
 * @return A new ActrNode instance
 * @throws ActrException.Config if the config file is invalid
 */
suspend fun createActrNode(
    configPath: String,
    packagePath: String,
    observers: RuntimeObservers? = null,
): ActrNode = ActrNode.fromPackageFile(configPath, packagePath, observers)

/**
 * Create a monitored ActrNode from a config file and package file.
 *
 * @param observers Optional host-side [RuntimeObservers] for transport readiness
 */
suspend fun createActrNodeWithMonitoring(
    configPath: String,
    packagePath: String,
    context: Context,
    scope: CoroutineScope,
    onNetworkStatusLog: ((String) -> Unit)? = null,
    observers: RuntimeObservers? = null,
): ActrNode =
    ActrNode.fromPackageFileWithMonitoring(
        configPath = configPath,
        packagePath = packagePath,
        context = context,
        scope = scope,
        onNetworkStatusLog = onNetworkStatusLog,
        observers = observers,
    )

/**
 * Create an ActrNode from config and package file URLs (top-level function).
 *
 * @param configURL File URL to the TOML configuration file
 * @param packageURL File URL to the `.actr` package file
 * @param observers Optional host-side [RuntimeObservers] for transport readiness
 * @return A new ActrNode instance
 * @throws IllegalArgumentException if either URL is not a file URL
 */
suspend fun createActrNode(
    configURL: URL,
    packageURL: URL,
    observers: RuntimeObservers? = null,
): ActrNode = ActrNode.fromPackageFile(configURL, packageURL, observers)

/**
 * Create a monitored ActrNode from config and package file URLs.
 *
 * @param observers Optional host-side [RuntimeObservers] for transport readiness
 */
suspend fun createActrNodeWithMonitoring(
    configURL: URL,
    packageURL: URL,
    context: Context,
    scope: CoroutineScope,
    onNetworkStatusLog: ((String) -> Unit)? = null,
    observers: RuntimeObservers? = null,
): ActrNode =
    ActrNode.fromPackageFileWithMonitoring(
        configURL = configURL,
        packageURL = packageURL,
        context = context,
        scope = scope,
        onNetworkStatusLog = onNetworkStatusLog,
        observers = observers,
    )

/**
 * Create an ActrNode backed by a linked dynamic workload (top-level function).
 *
 * @param configPath Path to `manifest.toml`, or to `actr.toml` for a
 * dependency-free linked workload
 * @param actorType The actor's type identity
 * @param workload The composed workload
 * @return A new ActrNode instance that retains the workload
 */
suspend fun linked(
    configPath: String,
    actorType: ActrType,
    workload: DynamicWorkload,
    membership: io.actrium.actr.RealmMembership? = null,
): ActrNode = ActrNode.linked(configPath, actorType, workload, membership)

/**
 * Create a monitored ActrNode backed by a linked dynamic workload.
 */
suspend fun linkedWithMonitoring(
    configPath: String,
    actorType: ActrType,
    workload: DynamicWorkload,
    context: Context,
    scope: CoroutineScope,
    onNetworkStatusLog: ((String) -> Unit)? = null,
    membership: io.actrium.actr.RealmMembership? = null,
): ActrNode =
    ActrNode.linkedWithMonitoring(
        configPath = configPath,
        actorType = actorType,
        workload = workload,
        context = context,
        scope = scope,
        onNetworkStatusLog = onNetworkStatusLog,
        membership = membership,
    )

// ============================================================================
// ActrRef Extensions
// ============================================================================

/**
 * Discover actors of the specified type using a type string.
 *
 * @param typeString Actor type in "publisher/program@version" format (e.g., "acme/EchoService@1.0.0")
 * @param count Maximum number of candidates to return (default: 1)
 * @return List of discovered actor IDs
 */
suspend fun ActrRef.discover(
    typeString: String,
    count: UInt = 1u,
): List<ActrId> = discover(typeString.toActrType(), count)

/**
 * Discover a single actor of the specified type.
 *
 * @param typeString Actor type in "publisher/program@version" format
 * @return The first discovered actor ID, or null if none found
 */
suspend fun ActrRef.discoverOne(typeString: String): ActrId? = discover(typeString, 1u).firstOrNull()

/**
 * Discover a single actor of the specified type.
 *
 * @param type Actor type
 * @return The first discovered actor ID, or null if none found
 */
suspend fun ActrRef.discoverOne(type: ActrType): ActrId? = discover(type, 1u).firstOrNull()

/** Await shutdown completion. Alias for [ActrRef.waitForShutdown]. */
suspend fun ActrRef.awaitShutdown() {
    waitForShutdown()
}

// ============================================================================
// SimpleWorkload Extensions
// ============================================================================

/**
 * Send a DataChunk built with DSL syntax.
 *
 * Example:
 * ```kotlin
 * workload.sendStream(targetId) {
 *     streamId = "my-stream"
 *     sequence = 0uL
 *     payload = "Hello".toByteArray()
 *     metadata {
 *         "key1" to "value1"
 *         "key2" to "value2"
 *     }
 * }
 * ```
 */
suspend fun SimpleWorkload.sendStream(
    target: ActrId,
    builder: DataChunkBuilder.() -> Unit,
) {
    val dataChunk = DataChunkBuilder().apply(builder).build()
    sendDataChunk(target, dataChunk)
}
