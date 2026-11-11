import Dispatch
import ActrBindings
import Foundation
import Network
#if canImport(UIKit)
import UIKit
#endif
#if canImport(AppKit)
import AppKit
#endif

/// A high-level entry point for creating and starting a package-backed ACTR node.
public final class ActrNode: Sendable {
    private let inner: ActrBindings.ActrNode
    private let monitoringResource: any MobileMonitoringResource
    private let retainedWorkload: DynamicWorkload?
    private let retainedObservers: RuntimeObservers?

    /// Creates a package-backed node from config and package file paths.
    public static func from(packageConfig configPath: String, packagePath: String, observers: RuntimeObservers? = nil) async throws -> ActrNode {
        try await from(
            packageConfig: configPath,
            packagePath: packagePath,
            observers: observers,
            mobileLifecycleObservers: []
        )
    }

    /// Creates a package-backed node with additional mobile lifecycle observers.
    public static func from(
        packageConfig configPath: String,
        packagePath: String,
        observers: RuntimeObservers?,
        mobileLifecycleObservers: [any MobileLifecycleObserver],
        membership: ActrBindings.RealmMembership? = nil
    ) async throws -> ActrNode {
        do {
            let wrapper: ActrBindings.ActrNode
            if let membership {
                wrapper = try await ActrBindings.ActrNode.newFromPackageFileWithMembership(
                    configPath: configPath, packagePath: packagePath, membership: membership, observers: observers?.bridge)
            } else if let observers {
                wrapper = try await ActrBindings.ActrNode.newFromPackageFileWithObservers(
                    configPath: configPath,
                    packagePath: packagePath,
                    observers: observers.bridge
                )
            } else {
                wrapper = try await ActrBindings.ActrNode.newFromPackageFile(
                    configPath: configPath,
                    packagePath: packagePath
                )
            }
            let handle = try wrapper.createNetworkEventHandle()
            let eventEmitter = MobileLifecycleEventEmitter(observers: mobileLifecycleObservers)
            let monitor = NetworkEventMonitor(handle: handle, emitter: eventEmitter)
            let lifecycleMonitor = AppLifecycleMonitor(
                handle: handle,
                networkEventMonitor: monitor,
                emitter: eventEmitter
            )
            return ActrNode(
                inner: wrapper,
                monitoringResource: DefaultMobileMonitoringResource(
                    emitter: eventEmitter,
                    networkEventMonitor: monitor,
                    appLifecycleMonitor: lifecycleMonitor
                ),
                retainedWorkload: nil,
                retainedObservers: observers
            )
        } catch {
            throw ActrError(error: error)
        }
    }

    /// Creates a package-backed node from config and package file URLs.
    public static func from(packageConfig configURL: URL, packageURL: URL, observers: RuntimeObservers? = nil) async throws -> ActrNode {
        try await from(
            packageConfig: configURL,
            packageURL: packageURL,
            observers: observers,
            mobileLifecycleObservers: []
        )
    }

    /// Creates a package-backed node from file URLs with additional mobile lifecycle observers.
    public static func from(
        packageConfig configURL: URL,
        packageURL: URL,
        observers: RuntimeObservers?,
        mobileLifecycleObservers: [any MobileLifecycleObserver]
    ) async throws -> ActrNode {
        guard configURL.isFileURL else {
            throw ActrError.Config(msg: "packageConfig URL must be a file URL")
        }
        guard packageURL.isFileURL else {
            throw ActrError.Config(msg: "packageURL must be a file URL")
        }
        return try await from(
            packageConfig: configURL.path,
            packagePath: packageURL.path,
            observers: observers,
            mobileLifecycleObservers: mobileLifecycleObservers
        )
    }

    /// Creates a linked/static node from config, explicit actor identity, and a Swift-provided workload.
    /// Pass `manifest.toml` to enable dynamic dependency alias resolution; its
    /// sibling `actr.toml` supplies runtime configuration.
    public static func linked(config configPath: String, type actorType: ActrType, workload: DynamicWorkload) async throws -> ActrNode {
        try await linked(
            config: configPath,
            type: actorType,
            workload: workload,
            mobileLifecycleObservers: []
        )
    }

    /// Creates a linked/static node with additional mobile lifecycle observers.
    public static func linked(
        config configPath: String,
        type actorType: ActrType,
        workload: DynamicWorkload,
        mobileLifecycleObservers: [any MobileLifecycleObserver],
        membership: ActrBindings.RealmMembership? = nil
    ) async throws -> ActrNode {
        do {
            let wrapper: ActrBindings.ActrNode
            if let membership {
                wrapper = try await ActrBindings.ActrNode.newFromLinkedWorkloadWithMembership(
                    configPath: configPath, actorType: actorType.bridge, workload: workload.bridge, membership: membership)
            } else {
                wrapper = try await ActrBindings.ActrNode.newFromLinkedWorkload(
                    configPath: configPath, actorType: actorType.bridge, workload: workload.bridge)
            }
            let handle = try wrapper.createNetworkEventHandle()
            let eventEmitter = MobileLifecycleEventEmitter(observers: mobileLifecycleObservers)
            let monitor = NetworkEventMonitor(handle: handle, emitter: eventEmitter)
            let lifecycleMonitor = AppLifecycleMonitor(
                handle: handle,
                networkEventMonitor: monitor,
                emitter: eventEmitter
            )
            return ActrNode(
                inner: wrapper,
                monitoringResource: DefaultMobileMonitoringResource(
                    emitter: eventEmitter,
                    networkEventMonitor: monitor,
                    appLifecycleMonitor: lifecycleMonitor
                ),
                retainedWorkload: workload,
                retainedObservers: nil
            )
        } catch {
            throw ActrError(error: error)
        }
    }

    /// Creates a linked/static node from a config file URL, explicit actor identity, and a Swift-provided workload.
    public static func linked(config configURL: URL, type actorType: ActrType, workload: DynamicWorkload) async throws -> ActrNode {
        try await linked(
            config: configURL,
            type: actorType,
            workload: workload,
            mobileLifecycleObservers: []
        )
    }

    /// Creates a linked/static node from a config file URL with mobile lifecycle observers.
    public static func linked(
        config configURL: URL,
        type actorType: ActrType,
        workload: DynamicWorkload,
        mobileLifecycleObservers: [any MobileLifecycleObserver]
    ) async throws -> ActrNode {
        guard configURL.isFileURL else {
            throw ActrError.Config(msg: "config URL must be a file URL")
        }
        return try await linked(
            config: configURL.path,
            type: actorType,
            workload: workload,
            mobileLifecycleObservers: mobileLifecycleObservers
        )
    }

    /// Starts the package-backed actor and returns a running reference.
    public func start() async throws -> ActrRef {
        do {
            let refWrapper = try await inner.start()
            return ActrRef(
                inner: refWrapper,
                retainedWorkload: retainedWorkload,
                retainedObservers: retainedObservers,
                retainedMonitoringResource: monitoringResource
            )
        } catch {
            throw ActrError(error: error)
        }
    }

    fileprivate init(
        inner: ActrBindings.ActrNode,
        monitoringResource: any MobileMonitoringResource,
        retainedWorkload: DynamicWorkload?,
        retainedObservers: RuntimeObservers?
    ) {
        self.inner = inner
        self.monitoringResource = monitoringResource
        self.retainedWorkload = retainedWorkload
        self.retainedObservers = retainedObservers
    }
}

public extension ActrNode {
    /// Creates a package-backed node with mobile lifecycle observers.
    static func from(
        packageConfig configPath: String,
        packagePath: String,
        mobileLifecycleObservers: [any MobileLifecycleObserver]
    ) async throws -> ActrNode {
        try await from(
            packageConfig: configPath,
            packagePath: packagePath,
            observers: nil,
            mobileLifecycleObservers: mobileLifecycleObservers
        )
    }

    /// Creates a package-backed node from file URLs with mobile lifecycle observers.
    static func from(
        packageConfig configURL: URL,
        packageURL: URL,
        mobileLifecycleObservers: [any MobileLifecycleObserver]
    ) async throws -> ActrNode {
        try await from(
            packageConfig: configURL,
            packageURL: packageURL,
            observers: nil,
            mobileLifecycleObservers: mobileLifecycleObservers
        )
    }
}

private final class NetworkEventMonitor: @unchecked Sendable {
    private let monitor: NWPathMonitor
    private let queue: DispatchQueue
    private let deliverySession: MobileEventDeliverySession
    private let delivery: MobileEventDeliveryGate
    private var reducer = NetworkPathEventReducer()
    private let emitter: MobileLifecycleEventEmitter

    init(handle: NetworkEventHandleWrapper, emitter: MobileLifecycleEventEmitter) {
        self.emitter = emitter
        let deliverySession = MobileEventDeliverySession(handle: handle, emitter: emitter)
        self.deliverySession = deliverySession
        delivery = deliverySession.gate
        monitor = NWPathMonitor()
        queue = DispatchQueue(label: "actr.network.monitor")
        emitter.emit(.monitorInitialized(.network))
        monitor.pathUpdateHandler = { [weak self] path in
            self?.process(path: path, forceNotify: false)
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
        emitter.emit(.monitorDeinitialized(.network))
    }

    func notifyCurrentPath(after predecessor: Task<Void, Never>) {
        queue.async { [weak self] in
            guard let self else { return }
            self.process(
                path: self.monitor.currentPath,
                forceNotify: true,
                deliveryPredecessor: predecessor
            )
        }
    }

    private func process(
        path: NWPath,
        forceNotify: Bool,
        deliveryPredecessor: Task<Void, Never>? = nil
    ) {
        let transport = transportFlags(for: path)
        let reduction = reducer.reduce(
            NetworkPathObservation(
                status: path.status,
                transport: transport,
                isExpensive: path.isExpensive,
                isConstrained: path.isConstrained
            ),
            forceNotify: forceNotify
        )
        let snapshot = reduction.snapshot
        let timestamp = formattedTimestamp()
        emitter.emit(.networkPathObserved(snapshot))

        print("Network path update: time=\(timestamp), status=\(path.status), availability=\(snapshot.availability), wifi=\(transport.wifi), cellular=\(transport.cellular), ethernet=\(transport.ethernet), vpn=\(transport.vpn), other=\(transport.other), expensive=\(snapshot.isExpensive), constrained=\(snapshot.isConstrained)")

        if reduction.isInitial {
            emitter.emit(.networkInitialPathCaptured(snapshot, forceNotify: forceNotify))
            print("Network initial path captured: time=\(timestamp), forceNotify=\(forceNotify)")
        }

        guard reduction.shouldNotify else {
            return
        }

        print("Network path changed: time=\(timestamp), sequence=\(snapshot.sequence), availability=\(snapshot.availability)")
        emitter.emit(.networkPathChanged(snapshot))
        notifyPathChanged(snapshot: snapshot, after: deliveryPredecessor)
    }

    private func transportFlags(for path: NWPath) -> NetworkTransportFlags {
        NetworkTransportFlags(
            wifi: path.usesInterfaceType(.wifi),
            cellular: path.usesInterfaceType(.cellular),
            ethernet: path.usesInterfaceType(.wiredEthernet),
            // Network.framework has no dedicated VPN interface type. Keep the
            // value false rather than conflating `.other` (which also covers
            // loopback/unknown interfaces) with VPN. Rust consumers should
            // treat iOS VPN reporting as a documented platform limitation.
            vpn: false,
            other: path.usesInterfaceType(.other) || path.usesInterfaceType(.loopback)
        )
    }

    private func notifyPathChanged(
        snapshot: NetworkSnapshot,
        after predecessor: Task<Void, Never>?
    ) {
        Task { [delivery] in
            await delivery.sendNetworkPathChanged(
                snapshot: snapshot,
                after: predecessor
            )
        }
    }

    private func formattedTimestamp() -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: Date())
    }
}

private final class AppLifecycleMonitor: @unchecked Sendable {
    private let deliverySession: MobileEventDeliverySession
    private let delivery: MobileEventDeliveryGate
    private let lifecycleDelivery: SerializedLifecycleDelivery
    private weak var networkEventMonitor: NetworkEventMonitor?
    private let queue: DispatchQueue
    private var observers: [NSObjectProtocol] = []
    private let lifecycleEvents = AppLifecycleEventSource()
    private let emitter: MobileLifecycleEventEmitter

    init(
        handle: NetworkEventHandleWrapper,
        networkEventMonitor: NetworkEventMonitor,
        emitter: MobileLifecycleEventEmitter
    ) {
        self.emitter = emitter
        let deliverySession = MobileEventDeliverySession(handle: handle, emitter: emitter)
        let delivery = deliverySession.gate
        self.deliverySession = deliverySession
        self.delivery = delivery
        lifecycleDelivery = SerializedLifecycleDelivery(delivery: delivery)
        self.networkEventMonitor = networkEventMonitor
        self.queue = DispatchQueue(label: "actr.lifecycle.monitor")
        emitter.emit(.monitorInitialized(.appLifecycle))
        print("AppLifecycleMonitor initialized: time=\(formattedTimestamp())")
        registerObservers()
        injectInitialLifecycleState()
    }

    deinit {
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        emitter.emit(.monitorDeinitialized(.appLifecycle))
        print("AppLifecycleMonitor deinitialized: time=\(formattedTimestamp())")
    }

    private func registerObservers() {
#if canImport(UIKit)
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.queue.async { [weak self] in
                self?.handleDidEnterBackground()
            }
        })
        observers.append(center.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.queue.async { [weak self] in
                self?.handleWillEnterForeground()
            }
        })
        print("AppLifecycleMonitor registered observers: time=\(formattedTimestamp()), platform=iOS")
#elseif canImport(AppKit)
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.queue.async { [weak self] in
                self?.handleDidEnterBackground()
            }
        })
        observers.append(center.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.queue.async { [weak self] in
                self?.handleWillEnterForeground()
            }
        })
        print("AppLifecycleMonitor registered observers: time=\(formattedTimestamp()), platform=macOS")
#else
        print("⚠️ AppLifecycleMonitor: No platform support available: time=\(formattedTimestamp())")
#endif
    }

    private func injectInitialLifecycleState() {
#if canImport(UIKit)
        DispatchQueue.main.async { [weak self] in
            let isBackground = UIApplication.shared.applicationState == .background
            self?.queue.async { [weak self] in
                self?.handleInitialLifecycleState(isBackground: isBackground)
            }
        }
#elseif canImport(AppKit)
        DispatchQueue.main.async { [weak self] in
            let isBackground = !NSApplication.shared.isActive
            self?.queue.async { [weak self] in
                self?.handleInitialLifecycleState(isBackground: isBackground)
            }
        }
#endif
    }

    private func handleInitialLifecycleState(isBackground: Bool) {
        guard let state = lifecycleEvents.initializePhase(isBackground: isBackground) else {
            print("AppLifecycleMonitor skipped stale initial phase: time=\(formattedTimestamp())")
            return
        }
        emitter.emit(.appLifecycleChanged(state))
        print("AppLifecycleMonitor initial phase: time=\(formattedTimestamp()), background=\(isBackground)")
        let deliveryTask = notifyLifecycleChanged(state: state)
        if !isBackground {
            networkEventMonitor?.notifyCurrentPath(after: deliveryTask)
        }
    }

    private func handleDidEnterBackground() {
        let timestamp = formattedTimestamp()
        if let state = lifecycleEvents.didEnterBackground() {
            emitter.emit(.appLifecycleChanged(state))
            print("🔵 App entered background: time=\(timestamp)")
            notifyLifecycleChanged(state: state)
        } else {
            print("⚠️ App entered background (already backgrounded): time=\(timestamp)")
        }
    }

    private func handleWillEnterForeground() {
        let timestamp = formattedTimestamp()
        let state = lifecycleEvents.willEnterForeground()
        guard case let .foreground(backgroundDurationMs) = state else {
            return
        }
        if backgroundDurationMs == 0 {
            print("🟢 App entered foreground (no background timestamp): time=\(timestamp)")
        } else {
            print("🟢 App entered foreground: time=\(timestamp), backgroundDurationMs=\(backgroundDurationMs)")
        }
        emitter.emit(.appLifecycleChanged(state))
        let deliveryTask = notifyLifecycleChanged(state: state)
        networkEventMonitor?.notifyCurrentPath(after: deliveryTask)
    }

    @discardableResult
    private func notifyLifecycleChanged(
        state: AppLifecycleState
    ) -> Task<Void, Never> {
        lifecycleDelivery.enqueue(state)
    }

    private func formattedTimestamp() -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: Date())
    }
}

/// Owns the monitors and the shared lifecycle emitter for one node.
///
/// The monitors must be held strongly: nothing else references the lifecycle
/// monitor, so releasing it here would unregister its platform observers and
/// silence lifecycle delivery for the rest of the node's lifetime.
private final class DefaultMobileMonitoringResource: MobileMonitoringResource, @unchecked Sendable {
    private let emitter: MobileLifecycleEventEmitter
    private var networkEventMonitor: NetworkEventMonitor?
    private var appLifecycleMonitor: AppLifecycleMonitor?

    init(
        emitter: MobileLifecycleEventEmitter,
        networkEventMonitor: NetworkEventMonitor,
        appLifecycleMonitor: AppLifecycleMonitor
    ) {
        self.emitter = emitter
        self.networkEventMonitor = networkEventMonitor
        self.appLifecycleMonitor = appLifecycleMonitor
    }

    deinit {
        // Drop the monitors first so their `.monitorDeinitialized` fan-out
        // still reaches observers. Outstanding delivery gates keep the emitter
        // alive long enough to report callbacks discarded after gate close.
        networkEventMonitor = nil
        appLifecycleMonitor = nil
    }
}
