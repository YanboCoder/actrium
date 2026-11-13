//
//  IPCMessageHandler.swift
//  Actrium
//
//  High-level IPC message handling with async/await support.
//

import Foundation
import SwiftProtobuf

// MARK: - IPC Connection (Actrium side)

/// Manages IPC connection to a Helper process. Lifecycle, requests, and
/// callback registration are isolated to the main actor.
@MainActor
public final class IPCConnection {
    private let server: IPCServer
    private let tabId: String
    private let filesystemSocketPath: String?
    private var pendingRequests: [UInt32: CheckedContinuation<Actrium_Ipc_IPCMessage, Error>] = [:]
    private var nextRequestId: UInt32 = 1

    public var onSurfaceUpdated: ((UInt32) -> Void)?
    public var onTitleChanged: ((String) -> Void)?
    public var onNavigationRequest: ((String, Bool) -> Void)?
    public var onConsoleLog: ((String, String, String) -> Void)?
    public var onCrash: ((String) -> Void)?
    public var onDisconnected: (() -> Void)?
    public var onRendererReady: ((String, Int32) -> Void)?
    public var onTabSurfaceUpdated: ((String, UInt32) -> Void)?
    public var onTabTitleChanged: ((String, String) -> Void)?
    public var onTabNavigationRequest: ((String, String, Bool) -> Void)?
    public var onTabConsoleLog: ((String, String, String, String) -> Void)?
    public var onTabCrash: ((String, String) -> Void)?
    public var onViewportApplied: ((String, UInt32, Double, Double) -> Void)?
    public var onHostEvent: ((String, String, String) -> Void)?
    public var onAccessibilityTree: ((Actrium_Ipc_AccessibilityTree) -> Void)?

    public init(tabId: String) {
        let socketPath = IPCSocketPath.forTab(tabId)
        self.tabId = tabId
        self.filesystemSocketPath = socketPath
        self.server = IPCServer(socketPath: socketPath)

        configureServerCallbacks()
    }

    /// Creates the Broker side of a connection over an already-connected Unix
    /// socket. This is used when the Renderer inherits the peer descriptor at
    /// launch, avoiding any filesystem IPC path.
    public init(tabId: String, connectedSocket: Int32) {
        self.tabId = tabId
        self.filesystemSocketPath = nil
        self.server = IPCServer(connectedSocket: connectedSocket)

        configureServerCallbacks()
    }

    private func configureServerCallbacks() {
        server.onMessage = { [weak self] data in
            self?.handleMessage(data)
        }

        server.onDisconnected = { [weak self] in
            self?.cancelPendingRequests()
            self?.onDisconnected?()
        }
    }

    public var socketPath: String {
        filesystemSocketPath ?? ""
    }

    /// Start listening for Helper connection
    public func start() throws {
        try server.start()
    }

    /// Stop the connection
    public func stop() {
        server.stop()

        cancelPendingRequests()
    }

    private func cancelPendingRequests() {
        let pending = pendingRequests
        pendingRequests.removeAll()

        for (_, continuation) in pending {
            continuation.resume(throwing: IPCError.connectionClosed)
        }
    }

    // MARK: - Send Requests

    /// Load content in the Helper
    public func loadContent(
        type: String,
        content: String,
        backgroundColor: String,
        tabId: String = "",
        title: String = "",
        leftPanelVisible: Bool = true,
        rightPanelVisible: Bool = false,
        reloadKey: Int = 0,
        contractName: String? = nil,
        contractVersion: Int? = nil,
        permissions: [String]? = nil,
        configJSON: String = ""
    ) async throws -> Bool {
        var msg = Actrium_Ipc_IPCMessage()
        msg.loadContent = Actrium_Ipc_LoadContentRequest()
        msg.loadContent.contentType = type
        msg.loadContent.content = content
        msg.loadContent.backgroundColor = backgroundColor
        msg.loadContent.tabID = tabId
        msg.loadContent.title = title
        msg.loadContent.leftPanelVisible = leftPanelVisible
        msg.loadContent.rightPanelVisible = rightPanelVisible
        msg.loadContent.reloadKey = Int32(reloadKey)

        if let name = contractName {
            msg.loadContent.contractName = name
        }
        if let version = contractVersion {
            msg.loadContent.contractVersion = Int32(version)
        }
        if let perms = permissions {
            msg.loadContent.permissions = perms
        }
        msg.loadContent.configJson = configJSON

        let response = try await sendRequest(msg)
        guard case .loadContentResponse(let resp) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        if !resp.success {
            throw IPCMessageError.loadFailed(resp.errorMessage)
        }
        return true
    }

    /// Update viewport size
    public func updateViewport(width: Double, height: Double) throws {
        try updateViewport(
            tabId: "",
            width: width,
            height: height,
            requestId: 0,
            isOverviewMode: false
        )
    }

    /// Update a specific Tab viewport in a multiplexed Renderer process.
    public func updateViewport(
        tabId: String,
        width: Double,
        height: Double,
        requestId: UInt32,
        isOverviewMode: Bool
    ) throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.updateViewport = Actrium_Ipc_UpdateViewportRequest()
        msg.updateViewport.width = width
        msg.updateViewport.height = height
        msg.updateViewport.tabID = tabId
        msg.updateViewport.requestID = requestId
        msg.updateViewport.isOverviewMode = isOverviewMode

        try sendOneWay(msg)
    }

    /// Send input event
    public func sendInputEvent(_ event: Actrium_Ipc_InputEvent) throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.inputEvent = event

        try sendOneWay(msg)
    }

    public func sendHostEvent(name: String, userInfoJSON: String, tabId: String) throws {
        var message = Actrium_Ipc_IPCMessage()
        message.hostEvent.name = name
        message.hostEvent.userInfoJson = userInfoJSON
        message.hostEvent.tabID = tabId
        try sendOneWay(message)
    }

    /// Execute JavaScript
    public func executeJS(_ code: String, tabId: String = "") async throws -> String {
        var msg = Actrium_Ipc_IPCMessage()
        msg.executeJs = Actrium_Ipc_ExecuteJSRequest()
        msg.executeJs.code = code
        msg.executeJs.tabID = tabId

        let response = try await sendRequest(msg)
        guard case .executeJsResponse(let resp) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        if !resp.success {
            throw IPCMessageError.jsFailed(resp.errorMessage)
        }
        return resp.result
    }

    /// Get current IOSurface ID
    public func getSurfaceID(tabId: String = "") async throws -> UInt32 {
        var msg = Actrium_Ipc_IPCMessage()
        msg.getSurfaceID = Actrium_Ipc_GetSurfaceIDRequest()
        msg.getSurfaceID.tabID = tabId

        let response = try await sendRequest(msg)
        guard case .surfaceIDResponse(let resp) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        return resp.surfaceID
    }

    /// Get metrics
    public func getMetrics() async throws -> Actrium_Ipc_MetricsResponse {
        var msg = Actrium_Ipc_IPCMessage()
        msg.getMetrics = Actrium_Ipc_GetMetricsRequest()

        let response = try await sendRequest(msg)
        guard case .metricsResponse(let resp) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        return resp
    }

    /// Terminate Helper
    public func terminate(reason: String) {
        var msg = Actrium_Ipc_IPCMessage()
        msg.terminate = Actrium_Ipc_TerminateRequest()
        msg.terminate.reason = reason

        try? sendOneWay(msg)
    }

    /// Destroy one Tab root while keeping its Renderer process alive.
    public func destroyTab(_ tabId: String) {
        var msg = Actrium_Ipc_IPCMessage()
        msg.destroyTab = Actrium_Ipc_DestroyTabRequest()
        msg.destroyTab.tabID = tabId
        try? sendOneWay(msg)
    }

    public func getAccessibilityTree(tabId: String) async throws -> Actrium_Ipc_AccessibilityTree {
        var msg = Actrium_Ipc_IPCMessage()
        msg.getAccessibilityTree = Actrium_Ipc_GetAccessibilityTreeRequest()
        msg.getAccessibilityTree.tabID = tabId
        let response = try await sendRequest(msg)
        guard case .accessibilityTree(let tree) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        return tree
    }

    public func sendAccessibilityAction(
        tabId: String,
        nodeId: UInt64,
        action: String
    ) async throws -> Bool {
        var msg = Actrium_Ipc_IPCMessage()
        msg.accessibilityAction = Actrium_Ipc_AccessibilityActionRequest()
        msg.accessibilityAction.tabID = tabId
        msg.accessibilityAction.nodeID = nodeId
        msg.accessibilityAction.action = action
        let response = try await sendRequest(msg)
        guard case .accessibilityActionResponse(let result) = response.payload else {
            throw IPCError.unexpectedResponse
        }
        if !result.success {
            throw IPCMessageError.loadFailed(result.errorMessage)
        }
        return true
    }

    // MARK: - Private

    private func sendRequest(_ message: Actrium_Ipc_IPCMessage) async throws -> Actrium_Ipc_IPCMessage {
        var msg = message

        let requestId = nextRequestId
        nextRequestId += 1

        msg.id = requestId

        return try await withCheckedThrowingContinuation { continuation in
            pendingRequests[requestId] = continuation

            do {
                let data = try msg.serializedData()
                try server.send(data)
            } catch {
                pendingRequests.removeValue(forKey: requestId)
                continuation.resume(throwing: error)
            }

            // Timeout, replies, and stop all remove requests on the main actor.
            DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in
                self?.pendingRequests.removeValue(forKey: requestId)?.resume(throwing: IPCError.timeout)
            }
        }
    }

    private func sendOneWay(_ message: Actrium_Ipc_IPCMessage) throws {
        let data = try message.serializedData()
        try server.send(data)
    }

    private func handleMessage(_ data: Data) {
        do {
            let msg = try Actrium_Ipc_IPCMessage(serializedBytes: data)

            // Check if it's a response to a pending request
            if msg.id != 0 {
                if let continuation = pendingRequests.removeValue(forKey: msg.id) {
                    continuation.resume(returning: msg)
                    return
                }
            }

            // Handle push messages from Helper
            switch msg.payload {
            case .ready(let ready):
                NSLog("IPCConnection: Helper ready, surfaceId=\(ready.surfaceID)")
                onRendererReady?(ready.rendererID, ready.processID)
                onSurfaceUpdated?(ready.surfaceID)

            case .surfaceUpdated(let update):
                onSurfaceUpdated?(update.surfaceID)
                onTabSurfaceUpdated?(update.tabID, update.surfaceID)

            case .titleChanged(let title):
                onTitleChanged?(title.title)
                onTabTitleChanged?(title.tabID, title.title)

            case .navigationRequest(let nav):
                onNavigationRequest?(nav.url, nav.newTab)
                onTabNavigationRequest?(nav.tabID, nav.url, nav.newTab)

            case .consoleLog(let log):
                onConsoleLog?(log.level, log.message, log.source)
                onTabConsoleLog?(log.tabID, log.level, log.message, log.source)

            case .crashReport(let crash):
                onCrash?(crash.reason)
                onTabCrash?(crash.tabID, crash.reason)

            case .viewportApplied(let applied):
                onViewportApplied?(applied.tabID, applied.requestID, applied.width, applied.height)

            case .hostEvent(let event):
                onHostEvent?(event.name, event.userInfoJson, event.tabID)

            case .accessibilityTree(let tree):
                onAccessibilityTree?(tree)

            default:
                NSLog("IPCConnection: Unhandled message: \(msg)")
            }
        } catch {
            NSLog("IPCConnection: Failed to parse message: \(error)")
        }
    }
}

// MARK: - IPC Helper Connection (Helper side)

/// Manages IPC connection from Helper to Actrium. Lifecycle and callbacks use
/// the main actor; outbound events may be sent from any thread.
@MainActor
public final class IPCHelperConnection {
    private let client: IPCClient
    private let tabId: String

    public var onLoadContent: ((Actrium_Ipc_LoadContentRequest, @escaping (Bool, String?) -> Void) -> Void)?
    public var onUpdateViewport: ((Double, Double) -> Void)?
    public var onInputEvent: ((Actrium_Ipc_InputEvent) -> Void)?
    public var onExecuteJS: ((String, @escaping (Bool, String?, String?) -> Void) -> Void)?
    public var onGetSurfaceID: ((@escaping (UInt32) -> Void) -> Void)?
    public var onGetMetrics: ((@escaping (Actrium_Ipc_MetricsResponse) -> Void) -> Void)?
    public var onTerminate: ((String) -> Void)?
    public var onDisconnected: (() -> Void)?
    public var onTabUpdateViewport: ((String, Double, Double, UInt32, Bool) -> Void)?
    public var onTabExecuteJS: ((String, String, @escaping (Bool, String?, String?) -> Void) -> Void)?
    public var onTabGetSurfaceID: ((String, @escaping (UInt32) -> Void) -> Void)?
    public var onDestroyTab: ((String) -> Void)?
    public var onGetAccessibilityTree: ((String, @escaping (Actrium_Ipc_AccessibilityTree) -> Void) -> Void)?
    public var onAccessibilityAction: ((Actrium_Ipc_AccessibilityActionRequest, @escaping (Bool, String?) -> Void) -> Void)?
    public var onHostEvent: ((String, String, String) -> Void)?

    public init(tabId: String, socketPath: String) {
        self.tabId = tabId
        self.client = IPCClient(socketPath: socketPath)

        configureClientCallbacks()
    }

    /// Creates the Helper side over an inherited, already-connected Unix
    /// socket descriptor.
    public init(tabId: String, connectedSocket: Int32) {
        self.tabId = tabId
        self.client = IPCClient(connectedSocket: connectedSocket)

        configureClientCallbacks()
    }

    private func configureClientCallbacks() {
        client.onMessage = { [weak self] data in
            self?.handleMessage(data)
        }

        client.onDisconnected = { [weak self] in
            self?.onDisconnected?()
        }
    }

    /// Connect to Actrium
    public func connect() throws {
        try client.connect()
    }

    /// Disconnect
    public func disconnect() {
        client.disconnect()
    }

    // MARK: - Send Messages to Actrium

    /// Send ready notification
    nonisolated public func sendReady(
        surfaceId: UInt32,
        width: UInt32,
        height: UInt32,
        rendererId: String = "",
        processId: Int32 = ProcessInfo.processInfo.processIdentifier
    ) throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.ready = Actrium_Ipc_ReadyResponse()
        msg.ready.surfaceID = surfaceId
        msg.ready.width = width
        msg.ready.height = height
        msg.ready.rendererID = rendererId
        msg.ready.processID = processId

        try send(msg)
    }

    /// Send surface updated notification
    nonisolated public func sendSurfaceUpdated(surfaceId: UInt32, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.surfaceUpdated = Actrium_Ipc_SurfaceUpdated()
        msg.surfaceUpdated.surfaceID = surfaceId
        msg.surfaceUpdated.tabID = tabId

        try send(msg)
    }

    /// Send title changed notification
    nonisolated public func sendTitleChanged(_ title: String, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.titleChanged = Actrium_Ipc_TitleChanged()
        msg.titleChanged.title = title
        msg.titleChanged.tabID = tabId

        try send(msg)
    }

    /// Send navigation request
    nonisolated public func sendNavigationRequest(url: String, newTab: Bool, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.navigationRequest = Actrium_Ipc_NavigationRequest()
        msg.navigationRequest.url = url
        msg.navigationRequest.newTab = newTab
        msg.navigationRequest.tabID = tabId

        try send(msg)
    }

    /// Send console log
    nonisolated public func sendConsoleLog(level: String, message: String, source: String, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.consoleLog = Actrium_Ipc_ConsoleLog()
        msg.consoleLog.level = level
        msg.consoleLog.message = message
        msg.consoleLog.source = source
        msg.consoleLog.tabID = tabId

        try send(msg)
    }

    /// Forward an allow-listed business action to the Browser Broker.
    nonisolated public func sendHostEvent(name: String, userInfoJSON: String, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.hostEvent = Actrium_Ipc_HostEvent()
        msg.hostEvent.name = name
        msg.hostEvent.userInfoJson = userInfoJSON
        msg.hostEvent.tabID = tabId
        try send(msg)
    }

    /// Send crash report
    nonisolated public func sendCrash(reason: String, tabId: String = "") throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.crashReport = Actrium_Ipc_CrashReport()
        msg.crashReport.reason = reason
        msg.crashReport.tabID = tabId

        try send(msg)
    }

    /// Acknowledge that a Tab viewport update has been applied.
    nonisolated public func sendViewportApplied(
        tabId: String,
        requestId: UInt32,
        width: Double,
        height: Double
    ) throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.viewportApplied = Actrium_Ipc_ViewportApplied()
        msg.viewportApplied.tabID = tabId
        msg.viewportApplied.requestID = requestId
        msg.viewportApplied.width = width
        msg.viewportApplied.height = height
        try send(msg)
    }

    nonisolated public func sendAccessibilityTree(_ tree: Actrium_Ipc_AccessibilityTree) throws {
        var msg = Actrium_Ipc_IPCMessage()
        msg.accessibilityTree = tree
        try send(msg)
    }

    // MARK: - Private

    nonisolated private func send(_ message: Actrium_Ipc_IPCMessage) throws {
        let data = try message.serializedData()
        try client.send(data)
    }

    nonisolated private func sendResponse(requestId: UInt32, _ message: Actrium_Ipc_IPCMessage) throws {
        var msg = message
        msg.id = requestId
        try send(msg)
    }

    private func handleMessage(_ data: Data) {
        do {
            let msg = try Actrium_Ipc_IPCMessage(serializedBytes: data)
            let requestId = msg.id

            switch msg.payload {
            case .loadContent(let req):
                onLoadContent?(req) { [weak self] success, error in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.loadContentResponse = Actrium_Ipc_LoadContentResponse()
                    resp.loadContentResponse.requestID = requestId
                    resp.loadContentResponse.success = success
                    resp.loadContentResponse.errorMessage = error ?? ""
                    try? self?.sendResponse(requestId: requestId, resp)
                }

            case .updateViewport(let req):
                onUpdateViewport?(req.width, req.height)
                onTabUpdateViewport?(
                    req.tabID,
                    req.width,
                    req.height,
                    req.requestID,
                    req.isOverviewMode
                )

            case .inputEvent(let event):
                onInputEvent?(event)

            case .hostEvent(let event):
                onHostEvent?(event.name, event.userInfoJson, event.tabID)

            case .executeJs(let req):
                let reply: (Bool, String?, String?) -> Void = { [weak self] success, result, error in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.executeJsResponse = Actrium_Ipc_ExecuteJSResponse()
                    resp.executeJsResponse.requestID = requestId
                    resp.executeJsResponse.success = success
                    resp.executeJsResponse.result = result ?? ""
                    resp.executeJsResponse.errorMessage = error ?? ""
                    resp.executeJsResponse.tabID = req.tabID
                    try? self?.sendResponse(requestId: requestId, resp)
                }
                if let onTabExecuteJS {
                    onTabExecuteJS(req.tabID, req.code, reply)
                } else {
                    onExecuteJS?(req.code, reply)
                }

            case .getSurfaceID:
                let tabId = msg.getSurfaceID.tabID
                let reply: (UInt32) -> Void = { [weak self] surfaceId in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.surfaceIDResponse = Actrium_Ipc_SurfaceIDResponse()
                    resp.surfaceIDResponse.requestID = requestId
                    resp.surfaceIDResponse.surfaceID = surfaceId
                    resp.surfaceIDResponse.tabID = tabId
                    try? self?.sendResponse(requestId: requestId, resp)
                }
                if let onTabGetSurfaceID {
                    onTabGetSurfaceID(tabId, reply)
                } else {
                    onGetSurfaceID?(reply)
                }

            case .getMetrics:
                onGetMetrics? { [weak self] metrics in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.metricsResponse = metrics
                    resp.metricsResponse.requestID = requestId
                    try? self?.sendResponse(requestId: requestId, resp)
                }

            case .terminate(let req):
                onTerminate?(req.reason)

            case .destroyTab(let req):
                onDestroyTab?(req.tabID)

            case .getAccessibilityTree(let req):
                onGetAccessibilityTree?(req.tabID) { [weak self] tree in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.accessibilityTree = tree
                    try? self?.sendResponse(requestId: requestId, resp)
                }

            case .accessibilityAction(let req):
                onAccessibilityAction?(req) { [weak self] success, error in
                    var resp = Actrium_Ipc_IPCMessage()
                    resp.accessibilityActionResponse = Actrium_Ipc_AccessibilityActionResponse()
                    resp.accessibilityActionResponse.requestID = requestId
                    resp.accessibilityActionResponse.tabID = req.tabID
                    resp.accessibilityActionResponse.success = success
                    resp.accessibilityActionResponse.errorMessage = error ?? ""
                    try? self?.sendResponse(requestId: requestId, resp)
                }

            default:
                NSLog("IPCHelperConnection: Unhandled message: \(msg)")
            }
        } catch {
            NSLog("IPCHelperConnection: Failed to parse message: \(error)")
        }
    }
}

// MARK: - Additional Errors

public enum IPCMessageError: Error, LocalizedError {
    case loadFailed(String)
    case jsFailed(String)
    case unexpectedResponse

    public var errorDescription: String? {
        switch self {
        case .loadFailed(let msg): return "Load failed: \(msg)"
        case .jsFailed(let msg): return "JS execution failed: \(msg)"
        case .unexpectedResponse: return "Unexpected response"
        }
    }
}

extension IPCError {
    static var timeout: IPCError { .connectionClosed }
    static var unexpectedResponse: IPCError { .invalidFrameLength }
}
