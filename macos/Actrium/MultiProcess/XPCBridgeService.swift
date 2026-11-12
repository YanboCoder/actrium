//
//  XPCBridgeService.swift
//  Actrium
//
//  Centralize all XPC Core components of service communications
//
//  Structure principles:
//  1. Single Data Source (SSOT) - All XPC The state of relevance is concentrated in this management
//  2. Unidirectional Data Flow - External passage API Request, internal processing time and readiness check
//  3. Automatic Defer - Automatic Cache when target is not ready, send automatically when ready
//  4. Pull Mode - XPC Active registration on startup of service, read state update from status manager when sending
//

import Foundation
import Combine
import AppKit

// MARK: - Process Separator identifier

/// Only identifier for main process, to separate multiple Actrium Examples
let ActriumHostProcessId = ProcessInfo.processInfo.processIdentifier

// MARK: - Data structure

/// Tab Configure Data
struct XPCTabConfig {
    let tabId: UUID
    let appId: String?
    let legacyActorBridge: Bool
    let title: String
    let backgroundColor: String
    let containerSize: CGSize
    let systemAppType: String?
    let unifiedScript: String?
    let capabilityManifestJSON: String?
    let leftPanelScript: String?
    let leftPanelVisible: Bool
    let rightPanelVisible: Bool
    let permissions: [String]
    let bookmarks: [[String: Any]]
    let recentHistory: [[String: Any]]
    let reloadKey: Int
    let serviceCallUrl: String?

    func toDictionary(isOverviewMode: Bool) -> [String: Any] {
        var dict: [String: Any] = [
            RuntimeBridgeContract.tabId: tabId.uuidString,
            RuntimeBridgeContract.title: title,
            "backgroundColor": backgroundColor,
            RuntimeBridgeContract.containerWidth: containerSize.width,
            RuntimeBridgeContract.containerHeight: containerSize.height,
            "isOverviewMode": isOverviewMode,
            "leftPanelVisible": leftPanelVisible,
            "rightPanelVisible": rightPanelVisible,
            "permissions": permissions,
            "bookmarks": bookmarks,
            "recentHistory": recentHistory,
            "reloadKey": reloadKey
        ]
        if let systemAppType = systemAppType {
            dict[RuntimeBridgeContract.systemAppType] = systemAppType
        }
        if let unifiedScript = unifiedScript {
            dict["unifiedScript"] = unifiedScript
        }
        if let capabilityManifestJSON = capabilityManifestJSON {
            dict["capabilityManifestJSON"] = capabilityManifestJSON
        }
        if let leftPanelScript = leftPanelScript {
            dict["leftPanelScript"] = leftPanelScript
        }
        if let serviceCallUrl = serviceCallUrl {
            dict["serviceCallUrl"] = serviceCallUrl
        }
        if let appId = appId {
            dict["appId"] = appId
        }
        if legacyActorBridge {
            dict["legacyActorBridge"] = true
        }
        return dict
    }
}

/// Tab Ready
struct XPCTabState {
    var serviceId: String?
    var isServiceReady: Bool = false
    var isConnected: Bool = false
    var lastSentSize: CGSize?
    var connectionId: String?

    var isReady: Bool {
        serviceId != nil && isServiceReady && isConnected
    }
}

/// Requests to be sent (cache when target is not ready)
enum XPCPendingRequest {
    case tabConfig(XPCTabConfig)
    case sizeChange(CGSize, String)  // size, requestId
}

// MARK: - XPCBridgeService

/// Centralize all XPC Core components of service communications
///
/// Usage:
/// 1. App Call on startup `bindToViewModel()` Subscriptions actriumMode Change
/// 2. Tab Call on creation `allocateService()` Allocation XPC Services
/// 3. Call after connection is created `markTabConnected()` Mark Ready
/// 4. Call on Send Configuration `sendTabConfig()`, auto-process readiness check and defer
/// 5. Tab Call on close `releaseTab()` Cleaning of resources
@MainActor
final class XPCBridgeService: ObservableObject {

    // MARK: - Singleton

    static let shared = XPCBridgeService()

    private init() {
        NSLog("📡 [XPCBridgeService] Initialized")
    }

    // MARK: - IPC Integration

    /// Adapter for IPC module
    private let ipcAdapter = IPCBridgeAdapter.shared

    // MARK: - Core state (single data source)

    /// Current Actrium Mode:false = Focus, true = Overview
    private(set) var isOverviewMode: Bool = false

    /// Every one. Tab It's... XPC Status
    private var tabStates: [UUID: XPCTabState] = [:]

    /// Request queue to be sent (cache when target is not ready)
    private var pendingRequests: [UUID: [XPCPendingRequest]] = [:]

    /// XPC Service readiness (by serviceId)
    private var serviceReadyStates: [String: Bool] = [:]

    // MARK: - Subscriptions

    private var actriumModeSubscription: AnyCancellable?

    // MARK: - Public API: Life cycle management

    /// ♪ Tie it to the BrowserViewModel,subscribe actriumMode Change
    func bindToViewModel(_ viewModel: ActriumModePublisher) {
        actriumModeSubscription?.cancel()

        let newOverviewMode = !viewModel.actriumMode.isInFocusMode
        NSLog("📡 [XPCBridgeService] bindToViewModel: isInFocusMode=%@, isOverviewMode=%@",
              viewModel.actriumMode.isInFocusMode ? "true" : "false",
              newOverviewMode ? "true" : "false")

        updateOverviewMode(newOverviewMode)

        actriumModeSubscription = viewModel.actriumModePublisher
            .dropFirst()
            .receive(on: DispatchQueue.main)
            .sink { [weak self] newMode in
                let isOverview = !newMode.isInFocusMode
                self?.updateOverviewMode(isOverview)
            }
    }

    /// Yes Tab Allocation XPC Services
    func allocateService(for tabId: UUID, fingerprint: String) throws -> String {
        if let existing = tabStates[tabId]?.serviceId {
            return existing
        }

        let serviceId = try RendererProcessPool.shared.allocateRenderer(for: tabId, fingerprint: fingerprint)

        var state = tabStates[tabId] ?? XPCTabState()
        state.serviceId = serviceId
        state.isServiceReady = serviceReadyStates[serviceId] ?? false
        tabStates[tabId] = state

        NSLog("📡 [XPCBridgeService] allocateService: tab=%@ service=%@ isServiceReady=%@",
              String(tabId.uuidString.prefix(8)), serviceId.components(separatedBy: ".").last ?? "",
              state.isServiceReady ? "true" : "false")

        return serviceId
    }

    /// Tags Tab Connection established
    func markTabConnected(_ tabId: UUID, connectionId: String? = nil) {
        var state = tabStates[tabId] ?? XPCTabState()
        state.isConnected = true
        state.connectionId = connectionId
        tabStates[tabId] = state

        NSLog("📡 [XPCBridgeService] markTabConnected: tab=%@ connectionId=%@ isReady=%@",
              String(tabId.uuidString.prefix(8)), connectionId ?? "nil",
              state.isReady ? "true" : "false")

        ipcAdapter.markTabConnected(tabId, connectionId: connectionId)
        flushPendingRequests(for: tabId)
    }

    /// Release Tab It's... XPC Resources
    func releaseTab(_ tabId: UUID) {
        guard let state = tabStates[tabId] else { return }

        if state.serviceId != nil {
            RendererProcessPool.shared.releaseRenderer(for: tabId)
        }

        ipcAdapter.releaseTab(tabId)

        tabStates.removeValue(forKey: tabId)
        pendingRequests.removeValue(forKey: tabId)

        NSLog("📡 [XPCBridgeService] releaseTab: tab=%@", String(tabId.uuidString.prefix(8)))
    }

    // MARK: - Public API: Send request

    /// Send Tab Configure
    func sendTabConfig(_ config: XPCTabConfig) {
        let tabId = config.tabId
        let tabIdShort = String(tabId.uuidString.prefix(8))

        guard let state = tabStates[tabId] else {
            NSLog("⚠️ [XPCBridgeService] sendTabConfig: tab=%@ NOT REGISTERED, deferring", tabIdShort)
            appendPendingRequest(tabId: tabId, request: .tabConfig(config))
            return
        }

        guard state.isReady else {
            NSLog("⚠️ [XPCBridgeService] sendTabConfig: tab=%@ NOT READY (service=%@, connected=%@, serviceReady=%@), deferring",
                  tabIdShort,
                  state.serviceId ?? "nil",
                  state.isConnected ? "true" : "false",
                  state.isServiceReady ? "true" : "false")
            appendPendingRequest(tabId: tabId, request: .tabConfig(config))
            return
        }

        doSendTabConfig(config)
    }

    /// Volume change of the sending container
    func sendContainerSizeChanged(tabId: UUID, size: CGSize, requestId: String = UUID().uuidString.prefix(8).description) {
        let tabIdShort = String(tabId.uuidString.prefix(8))

        guard let state = tabStates[tabId], state.isReady else {
            NSLog("⚠️ [XPCBridgeService] sendContainerSizeChanged: tab=%@ NOT READY, deferring", tabIdShort)
            appendPendingRequest(tabId: tabId, request: .sizeChange(size, requestId))
            return
        }

        if let lastSize = state.lastSentSize, lastSize == size {
            NSLog("📡 [XPCBridgeService] sendContainerSizeChanged: tab=%@ SKIP (same size)", tabIdShort)
            return
        }

        var updatedState = state
        updatedState.lastSentSize = size
        tabStates[tabId] = updatedState

        doSendContainerSizeChanged(tabId: tabId, size: size, requestId: requestId)
    }

    // MARK: - Public API: Status query

    var currentOverviewMode: Bool {
        isOverviewMode
    }

    func tabState(for tabId: UUID) -> XPCTabState? {
        tabStates[tabId]
    }

    func isTabReady(_ tabId: UUID) -> Bool {
        tabStates[tabId]?.isReady ?? false
    }

    func serviceId(for tabId: UUID) -> String? {
        tabStates[tabId]?.serviceId
    }

    // MARK: - Internal methodology: status update

    private func updateOverviewMode(_ newValue: Bool) {
        guard isOverviewMode != newValue else { return }

        isOverviewMode = newValue
        NSLog("📡 [XPCBridgeService] updateOverviewMode: %@", newValue ? "true" : "false")

        for (tabId, state) in tabStates where state.isReady {
            if let lastSize = state.lastSentSize {
                doSendContainerSizeChanged(
                    tabId: tabId,
                    size: lastSize,
                    requestId: "mode-\(UUID().uuidString.prefix(8))"
                )
            }
        }
    }

    func markServiceReady(_ serviceId: String) {
        guard serviceReadyStates[serviceId] != true else { return }

        serviceReadyStates[serviceId] = true
        NSLog("📡 [XPCBridgeService] markServiceReady: %@", serviceId.components(separatedBy: ".").last ?? "")

        ipcAdapter.markServiceReady(serviceId)

        for (tabId, var state) in tabStates where state.serviceId == serviceId {
            state.isServiceReady = true
            tabStates[tabId] = state

            if state.isReady {
                flushPendingRequests(for: tabId)
            }
        }
    }

    // MARK: - Internal method: Queue requested

    private func appendPendingRequest(tabId: UUID, request: XPCPendingRequest) {
        var requests = pendingRequests[tabId] ?? []

        if case .sizeChange = request {
            requests.removeAll { if case .sizeChange = $0 { return true } else { return false } }
        }

        requests.append(request)
        pendingRequests[tabId] = requests
    }

    private func flushPendingRequests(for tabId: UUID) {
        guard let requests = pendingRequests[tabId], !requests.isEmpty else { return }
        guard let state = tabStates[tabId], state.isReady else { return }

        NSLog("📡 [XPCBridgeService] flushPendingRequests: tab=%@ count=%d",
              String(tabId.uuidString.prefix(8)), requests.count)

        pendingRequests.removeValue(forKey: tabId)

        for request in requests {
            switch request {
            case .tabConfig(let config):
                doSendTabConfig(config)
            case .sizeChange(let size, let requestId):
                doSendContainerSizeChanged(tabId: tabId, size: size, requestId: requestId)
            }
        }
    }

    // MARK: - Internal method: actual delivery

    private func doSendTabConfig(_ config: XPCTabConfig) {
        let tabIdShort = String(config.tabId.uuidString.prefix(8))

        NSLog("📤 [XPCBridgeService] sendTabConfig: tab=%@ size=%.0fx%.0f isOverviewMode=%@",
              tabIdShort, config.containerSize.width, config.containerSize.height,
              isOverviewMode ? "true" : "false")

        ipcAdapter.sendTabConfig(config, isOverviewMode: isOverviewMode)
    }

    private func doSendContainerSizeChanged(tabId: UUID, size: CGSize, requestId: String) {
        let tabIdShort = String(tabId.uuidString.prefix(8))

        NSLog("📐 [XPCBridgeService] sendContainerSizeChanged: tab=%@ size=%.0fx%.0f isOverviewMode=%@",
              tabIdShort, size.width, size.height, isOverviewMode ? "true" : "false")

        let reqIdInt = Int(requestId.filter { $0.isNumber }) ?? 0
        ipcAdapter.sendContainerSizeChanged(tabId: tabId, size: size, requestId: reqIdInt)
    }
}

// MARK: - Backward compatibility:ViewBridgeCoordinatorManager Proxy

extension XPCBridgeService {
    func coordinator(for tabId: UUID) -> ViewBridgeCoordinator {
        ViewBridgeCoordinatorManager.shared.coordinator(for: tabId)
    }
}
