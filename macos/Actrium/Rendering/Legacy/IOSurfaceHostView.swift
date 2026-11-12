import SwiftUI
import AppKit
import QuartzCore
import Combine

// MARK: - CALayerHost View (SwiftUI)

// Note: CALayerHostViewCache is defined in TabSurfaceManager.swift

/// SwiftUI wrapper for displaying remote CALayer content via CALayerHost
struct IOSurfaceHostView: NSViewRepresentable {
    let surfaceID: UInt32  // Actually contextId now
    let width: CGFloat
    let height: CGFloat
    let tabId: UUID

    func makeNSView(context: Context) -> CALayerHostDisplayView {
        // Use cached view to persist across parent view recreations
        let view = CALayerHostViewCache.shared.getOrCreate(
            for: tabId,
            frame: NSRect(x: 0, y: 0, width: width, height: height)
        )
        view.tabId = tabId
        view.surfaceWidth = width
        view.surfaceHeight = height
        view.updateContext(id: surfaceID)
        return view
    }

    func updateNSView(_ nsView: CALayerHostDisplayView, context: Context) {
        nsView.tabId = tabId
        nsView.surfaceWidth = width
        nsView.surfaceHeight = height
        nsView.updateContext(id: surfaceID)
    }

    static func dismantleNSView(_ nsView: CALayerHostDisplayView, coordinator: ()) {
        // Don't remove from cache immediately - let the view persist
        // The cache entry will be cleaned up when the tab is closed
        NSLog("🖥️ [CACHE] dismantleNSView called for tabId=\(nsView.tabId?.uuidString ?? "nil"), keeping in cache")
    }
}

// MARK: - CALayerHost Display View (AppKit)

/// AppKit view that displays remote CALayer content using CALayerHost
class CALayerHostDisplayView: NSView {
    private var hostLayer: CALayer?  // The actual CALayerHost instance
    private var currentContextId: UInt32 = 0

    /// Tab ID for event forwarding
    var tabId: UUID? {
        didSet {
            if oldValue != tabId {
                setupNotificationObserver()
            }
        }
    }

    /// Expected surface dimensions for intrinsicContentSize
    var surfaceWidth: CGFloat = 0
    var surfaceHeight: CGFloat = 0

    /// Tracking area for mouse movement events
    private var trackingArea: NSTrackingArea?

    /// Notification observer for context ID updates
    private var contextObserver: NSObjectProtocol?

    /// Return the expected size so SwiftUI can properly size this view
    override var intrinsicContentSize: NSSize {
        return NSSize(width: surfaceWidth, height: surfaceHeight)
    }

    private static var instanceCount: Int = 0
    private static let layerHostClass: AnyClass? = NSClassFromString("CALayerHost")
    private var diagnosticTimer: Timer?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        CALayerHostDisplayView.instanceCount += 1
        NSLog("CALayerHostDisplayView: init, total instances: \(CALayerHostDisplayView.instanceCount)")
        setupLayer()
        setupTracking()
        startDiagnosticTimer()
    }

    private func startDiagnosticTimer() {
        // Log layer state every 2 seconds for debugging
        diagnosticTimer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            self?.logLayerState()
        }
    }

    private func logLayerState() {
        guard let hostLayer = hostLayer else {
            NSLog("🔍 [LAYER-CHECK] No hostLayer")
            return
        }

        let hasContents = hostLayer.contents != nil
        let sublayerCount = hostLayer.sublayers?.count ?? 0

        // Get CALayerHost-specific properties via KVC
        let actualContextId = hostLayer.value(forKey: "contextId") as? UInt32 ?? 0
        let connected = hostLayer.value(forKey: "connected") as? Bool

        // Check layer class name
        let className = String(describing: type(of: hostLayer))

        NSLog("🔍 [LAYER-CHECK] class=\(className), contextId=\(actualContextId), connected=\(connected ?? false), hasContents=\(hasContents), sublayers=\(sublayerCount)")

        // Log layer hierarchy
        if let parentLayer = layer {
            NSLog("🔍 [LAYER-CHECK] parent.sublayers=\(parentLayer.sublayers?.count ?? 0), parent.isHidden=\(parentLayer.isHidden)")
        }
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setupLayer()
        setupTracking()
    }

    deinit {
        NSLog("CALayerHostDisplayView: deinit called for tabId \(tabId?.uuidString ?? "nil")")
        diagnosticTimer?.invalidate()
        diagnosticTimer = nil
        if let observer = contextObserver {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        NSLog("🖥️ [DIAG] viewDidMoveToWindow: window=\(window?.frame.debugDescription ?? "nil"), bounds=\(bounds)")

        // Re-setup the host layer when we get attached to a window
        // This ensures proper configuration after view hierarchy changes
        if window != nil && currentContextId != 0 {
            setupHostLayer(contextId: currentContextId)
        }
    }

    /// Set up notification observer for context ID changes
    private func setupNotificationObserver() {
        if let observer = contextObserver {
            NotificationCenter.default.removeObserver(observer)
            contextObserver = nil
        }

        guard let tabId = tabId else {
            NSLog("🖥️ [DIAG] setupNotificationObserver: tabId is nil, skipping")
            return
        }

        NSLog("🖥️ [DIAG] setupNotificationObserver: Setting up observer for tabId=\(tabId)")

        contextObserver = NotificationCenter.default.addObserver(
            forName: .tabSurfaceUpdated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self = self else { return }
            guard let notifTabId = notification.userInfo?["tabId"] as? UUID,
                  notifTabId == tabId,
                  let newContextId = notification.userInfo?["surfaceID"] as? UInt32,
                  newContextId != 0 else { return }

            if newContextId != self.currentContextId {
                NSLog("🖥️ [DIAG] Context ID changed via notification: \(self.currentContextId) -> \(newContextId)")
                self.updateContext(id: newContextId)
            }
        }
    }

    private func setupLayer() {
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.12, alpha: 1.0).cgColor
    }

    private func setupTracking() {
        trackingArea = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(trackingArea!)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let existing = trackingArea {
            removeTrackingArea(existing)
        }
        trackingArea = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(trackingArea!)
    }

    // MARK: - Context Update

    /// Update the CALayerHost with a new context ID
    func updateContext(id: UInt32) {
        guard id != 0 else {
            showPlaceholder()
            return
        }

        guard id != currentContextId else { return }

        currentContextId = id
        setupHostLayer(contextId: id)
    }

    private func setupHostLayer(contextId: UInt32) {
        // Remove existing host layer
        hostLayer?.removeFromSuperlayer()
        hostLayer = nil

        // Create CALayerHost using private API
        guard let layerHostClass = CALayerHostDisplayView.layerHostClass else {
            NSLog("🖥️ [ERROR] CALayerHost class not available")
            return
        }

        guard let newHostLayer = layerHostClass.alloc() as? CALayer else {
            NSLog("🖥️ [ERROR] Failed to allocate CALayerHost")
            return
        }

        // Initialize the CALayerHost
        _ = newHostLayer.perform(NSSelectorFromString("init"))

        // Set the contextId to connect to the remote CAContext
        newHostLayer.setValue(NSNumber(value: contextId), forKey: "contextId")

        let scale = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2.0

        // Configure the layer
        newHostLayer.anchorPoint = CGPoint.zero
        newHostLayer.frame = bounds
        newHostLayer.contentsScale = scale

        // Disable implicit animations on the host layer
        newHostLayer.actions = [
            "bounds": NSNull(),
            "position": NSNull(),
            "contents": NSNull(),
            "sublayers": NSNull(),
            "transform": NSNull()
        ]

        newHostLayer.isOpaque = true
        newHostLayer.masksToBounds = false

        // Add to our layer
        if let parentLayer = layer {
            parentLayer.addSublayer(newHostLayer)

            NSLog("🖥️ [DIAG] CALayerHost setup:")
            NSLog("🖥️ [DIAG]   contextId = \(contextId)")
            NSLog("🖥️ [DIAG]   frame = \(newHostLayer.frame)")
            NSLog("🖥️ [DIAG]   bounds = \(bounds)")
            NSLog("🖥️ [DIAG]   scale = \(scale)")
        } else {
            NSLog("🖥️ [ERROR] No parent layer available")
        }

        hostLayer = newHostLayer
    }

    private func showPlaceholder() {
        currentContextId = 0
        hostLayer?.removeFromSuperlayer()
        hostLayer = nil
        layer?.backgroundColor = NSColor(white: 0.12, alpha: 1.0).cgColor
    }

    override func layout() {
        super.layout()
        hostLayer?.frame = bounds
    }

    override var isFlipped: Bool {
        return true
    }

    // MARK: - Mouse Events

    override func mouseDown(with event: NSEvent) {
        forwardMouseEvent(event, type: .mouseDown)
    }

    override func mouseUp(with event: NSEvent) {
        forwardMouseEvent(event, type: .mouseUp)
    }

    override func mouseMoved(with event: NSEvent) {
        forwardMouseEvent(event, type: .mouseMoved)
    }

    override func mouseDragged(with event: NSEvent) {
        forwardMouseEvent(event, type: .mouseDragged)
    }

    override func scrollWheel(with event: NSEvent) {
        guard let tabId = tabId else { return }
        let location = convert(event.locationInWindow, from: nil)
        let xpcEvent = TabXPCEvent(
            eventType: .scrollWheel,
            locationX: Double(location.x),
            locationY: Double(location.y),
            modifierFlags: event.modifierFlags.rawValue,
            scrollDeltaX: event.scrollingDeltaX,
            scrollDeltaY: event.scrollingDeltaY,
            viewWidth: Double(bounds.width),
            viewHeight: Double(bounds.height)
        )
        LegacyRendererProcessManager.shared.forwardEvent(xpcEvent, to: tabId)
    }

    private func forwardMouseEvent(_ event: NSEvent, type: TabXPCEvent.EventType) {
        guard let tabId = tabId else { return }
        let location = convert(event.locationInWindow, from: nil)
        let xpcEvent = TabXPCEvent(
            eventType: type,
            locationX: Double(location.x),
            locationY: Double(location.y),
            modifierFlags: event.modifierFlags.rawValue,
            viewWidth: Double(bounds.width),
            viewHeight: Double(bounds.height)
        )
        LegacyRendererProcessManager.shared.forwardEvent(xpcEvent, to: tabId)
    }

    // MARK: - Keyboard Events

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        forwardKeyEvent(event, type: .keyDown)
    }

    override func keyUp(with event: NSEvent) {
        forwardKeyEvent(event, type: .keyUp)
    }

    override func flagsChanged(with event: NSEvent) {
        guard let tabId = tabId else { return }
        let xpcEvent = TabXPCEvent(
            eventType: .flagsChanged,
            modifierFlags: event.modifierFlags.rawValue,
            keyCode: event.keyCode
        )
        LegacyRendererProcessManager.shared.forwardEvent(xpcEvent, to: tabId)
    }

    private func forwardKeyEvent(_ event: NSEvent, type: TabXPCEvent.EventType) {
        guard let tabId = tabId else { return }
        let xpcEvent = TabXPCEvent(
            eventType: type,
            modifierFlags: event.modifierFlags.rawValue,
            keyCode: event.keyCode,
            characters: event.characters
        )
        LegacyRendererProcessManager.shared.forwardEvent(xpcEvent, to: tabId)
    }

    // MARK: - Hit Testing

    override func hitTest(_ point: NSPoint) -> NSView? {
        if bounds.contains(point) {
            return self
        }
        return nil
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        return true
    }

    /// Check if CALayerHost is available on this system
    static var isAvailable: Bool {
        return layerHostClass != nil
    }
}

// MARK: - Renderer Startup Tracker

/// Tracks which tabs have started or are starting their renderers (persists across view recreations)
class RendererStartupTracker {
    static let shared = RendererStartupTracker()

    private var startingTabs: Set<UUID> = []
    private var startedTabs: Set<UUID> = []
    private let lock = NSLock()

    func markStarting(_ tabId: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }

        // If already starting or started, don't start again
        if startingTabs.contains(tabId) || startedTabs.contains(tabId) {
            NSLog("🟡 [TRACKER] Tab \(tabId) already starting/started, skipping")
            return false
        }

        startingTabs.insert(tabId)
        NSLog("🟢 [TRACKER] Tab \(tabId) marked as starting")
        return true
    }

    func markStarted(_ tabId: UUID) {
        lock.lock()
        defer { lock.unlock() }
        startingTabs.remove(tabId)
        startedTabs.insert(tabId)
        NSLog("🟢 [TRACKER] Tab \(tabId) marked as started")
    }

    func markFailed(_ tabId: UUID) {
        lock.lock()
        defer { lock.unlock() }
        startingTabs.remove(tabId)
        NSLog("🔴 [TRACKER] Tab \(tabId) marked as failed")
    }

    func isStartedOrStarting(_ tabId: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return startingTabs.contains(tabId) || startedTabs.contains(tabId)
    }

    func reset(_ tabId: UUID) {
        lock.lock()
        defer { lock.unlock() }
        startingTabs.remove(tabId)
        startedTabs.remove(tabId)
    }
}

// MARK: - Multi-Process Tab View

/// A view that displays tab content from a renderer process
struct MultiProcessTabView: View {
    let tab: Tab
    let containerSize: CGSize

    private let processManager = LegacyRendererProcessManager.shared

    @State private var surfaceID: UInt32 = 0  // Actually contextId
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?
    @State private var activeTabId: UUID?
    @State private var crashReason: String?

    private var tabId: UUID { tab.id }

    var body: some View {
        let _ = NSLog("🔄 [BODY] MultiProcessTabView.body: surfaceID=%u, activeTabId=%@, isLoading=%d, crashReason=%@", surfaceID, activeTabId?.uuidString ?? "nil", isLoading ? 1 : 0, crashReason ?? "nil")
        ZStack {
            if let reason = crashReason {
                crashedView(reason: reason)
            } else if surfaceID != 0, let activeId = activeTabId {
                IOSurfaceHostView(
                    surfaceID: surfaceID,
                    width: containerSize.width,
                    height: containerSize.height,
                    tabId: activeId
                )
                .id("\(activeId)-\(surfaceID)")  // Stable identity based on tab and surface
                .frame(width: containerSize.width, height: containerSize.height)
            } else if isLoading {
                loadingView
            } else if let error = errorMessage {
                errorView(error)
            } else {
                placeholderView
            }
        }
        .onChange(of: containerSize) { oldSize, newSize in
            NSLog("📐 [SIZE-CHANGE] containerSize changed: \(Int(oldSize.width))x\(Int(oldSize.height)) -> \(Int(newSize.width))x\(Int(newSize.height)) for tab \(tabId)")
            updateViewportSize(newSize)
        }
        .onAppear {
            NSLog("🔵 [DIAG] MultiProcessTabView.onAppear: tabId=%@, contextId=%u, isLoading=%d", tabId.uuidString, surfaceID, isLoading ? 1 : 0)
            startRenderer()
        }
        .onDisappear {
            NSLog("🔴 [DIAG] MultiProcessTabView.onDisappear: tabId=%@, contextId=%u", tabId.uuidString, surfaceID)
        }
        .onReceive(NotificationCenter.default.publisher(for: .tabSurfaceUpdated)) { notification in
            if let notifTabId = notification.userInfo?["tabId"] as? UUID, notifTabId == tabId,
               let newContextId = notification.userInfo?["surfaceID"] as? UInt32,
               newContextId != 0 {
                if surfaceID == 0 {
                    NSLog("📐 [DIAG] First valid contextId received: \(newContextId)")
                    activeTabId = tabId
                    surfaceID = newContextId
                    isLoading = false
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .tabRendererCrashed)) { notification in
            if let notifTabId = notification.userInfo?["tabId"] as? UUID, notifTabId == activeTabId {
                let reason = notification.userInfo?["reason"] as? String ?? "Unknown error"
                handleCrash(reason: reason)
            }
        }
        .id(tab.id)
    }

    private var loadingView: some View {
        VStack(spacing: 12) {
            Text("Starting renderer process...")
                .font(.system(size: 12))
                .foregroundColor(.secondary)
        }
        .frame(minWidth: 180)
    }

    private func errorView(_ message: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 32))
                .foregroundColor(.orange)
            Text("Renderer Error")
                .font(.headline)
            Text(message)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal)

            Button("Retry") {
                startRenderer()
            }
            .buttonStyle(.bordered)
        }
    }

    private var placeholderView: some View {
        VStack(spacing: 12) {
            Image(systemName: "rectangle.dashed")
                .font(.system(size: 32))
                .foregroundColor(.secondary)
            Text("No content")
                .font(.system(size: 12))
                .foregroundColor(.secondary)
        }
    }

    private func crashedView(reason: String) -> some View {
        VStack(spacing: 16) {
            ZStack {
                Circle()
                    .fill(Color.gray.opacity(0.1))
                    .frame(width: 80, height: 80)
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 36))
                    .foregroundColor(.orange)
            }

            Text("This tab has crashed")
                .font(.system(size: 16, weight: .semibold))

            Text(reason)
                .font(.system(size: 12, design: .monospaced))
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)
                .lineLimit(3)

            Button(action: {
                reloadAfterCrash()
            }) {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.clockwise")
                    Text("Reload Tab")
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.regular)

            Text("The renderer process terminated unexpectedly.\nReloading will restart it in a new process.")
                .font(.system(size: 10))
                .foregroundColor(Color(nsColor: .tertiaryLabelColor))
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func handleCrash(reason: String) {
        crashReason = reason
        surfaceID = 0
        isLoading = false
    }

    private func reloadAfterCrash() {
        crashReason = nil
        surfaceID = 0
        activeTabId = nil
        // Reset the tracker so we can start fresh
        RendererStartupTracker.shared.reset(tabId)
        startRenderer()
    }

    private func startRenderer() {
        let processTabId = tabId

        // Use static tracker to prevent multiple starts (persists across view recreations)
        guard RendererStartupTracker.shared.markStarting(processTabId) else {
            // Already starting or started - just update local state from existing process
            if let existing = processManager.processes[processTabId], existing.isRunning, existing.surfaceID != 0 {
                activeTabId = processTabId
                surfaceID = existing.surfaceID
                isLoading = false
            }
            return
        }

        crashReason = nil
        isLoading = true
        errorMessage = nil

        Task {
            do {
                if let existing = processManager.processes[processTabId], existing.isRunning {
                    if let id = await processManager.getIOSurfaceID(for: processTabId), id != 0 {
                        await MainActor.run {
                            activeTabId = processTabId
                            surfaceID = id
                            isLoading = false
                        }
                        RendererStartupTracker.shared.markStarted(processTabId)
                        return
                    }
                }

                NSLog("🔵 [DEBUG] MultiProcessTabView: Creating config with leftPanelDefaultVisible=\(tab.leftPanelDefaultVisible), rightPanelDefaultVisible=\(tab.rightPanelDefaultVisible)")
                let config = TabXPCConfig(
                    tabId: processTabId.uuidString,
                    title: tab.title.isEmpty ? "New Tab" : tab.title,
                    url: tab.url?.absoluteString,
                    htmlContent: tab.htmlContent,
                    unifiedScript: tab.unifiedScript,
                    capabilityManifestJSON: tab.capabilityManifestJSON,
                    leftPanelScript: tab.leftPanelScript,
                    rightPanelScript: tab.rightPanelScript,
                    backgroundColor: tab.backgroundColor,
                    leftPanelVisible: tab.leftPanelDefaultVisible,
                    rightPanelVisible: tab.rightPanelDefaultVisible,
                    reloadKey: tab.reloadNonce,
                    contractName: tab.contractName,
                    contractVersion: tab.contractVersion,
                    permissions: tab.permissions
                )

                _ = try await processManager.getRenderer(for: processTabId, config: config, viewportSize: containerSize)

                // Only update state if notification handler hasn't already set it
                // This prevents redundant state updates that cause view recreation
                if let id = await processManager.getIOSurfaceID(for: processTabId) {
                    await MainActor.run {
                        if self.surfaceID == 0 {
                            NSLog("🔵 [DIAG] startRenderer: Setting state from getIOSurfaceID (contextId=\(id))")
                            activeTabId = processTabId
                            surfaceID = id
                            isLoading = false
                        } else {
                            NSLog("🟡 [DIAG] startRenderer: State already set by notification (current=\(self.surfaceID)), skipping")
                        }
                    }
                }

                RendererStartupTracker.shared.markStarted(processTabId)
            } catch {
                RendererStartupTracker.shared.markFailed(processTabId)
                await MainActor.run {
                    errorMessage = error.localizedDescription
                    isLoading = false
                }
            }
        }
    }

    private func stopRenderer() {
        if let activeId = activeTabId {
            processManager.terminateProcess(for: activeId)
        }
        surfaceID = 0
        activeTabId = nil
    }

    private func refreshSurface() {
        guard let activeId = activeTabId else { return }
        Task {
            if let id = await processManager.getIOSurfaceID(for: activeId) {
                if id != surfaceID {
                    NSLog("📐 [DIAG] refreshSurface: contextId changed \(surfaceID) -> \(id), updating state")
                    await MainActor.run {
                        surfaceID = id
                    }
                }
            }
        }
    }

    private func updateViewportSize(_ size: CGSize) {
        guard let activeId = activeTabId else { return }
        NSLog("📐 [DIAG] updateViewportSize: \(Int(size.width))x\(Int(size.height)) for tab \(activeId)")
        processManager.updateViewport(width: size.width, height: size.height, for: activeId)
    }
}

// MARK: - Preview

#Preview {
    MultiProcessTabView(
        tab: Tab(title: "Preview Tab"),
        containerSize: CGSize(width: 800, height: 600)
    )
    .frame(width: 800, height: 600)
}
