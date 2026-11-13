import ActriumIPC
import AppKit
import QuartzCore
import SwiftUI

/// SwiftUI bridge for a remotely hosted CoreAnimation layer produced by an
/// ActriumRenderer process.
struct PersistentRendererProcessView: NSViewRepresentable {
    let tabId: UUID
    let fingerprint: String
    let coordinator: ViewBridgeCoordinator
    let size: CGSize
    let allowsHitTesting: Bool
    let config: [String: Any]

    func makeNSView(context: Context) -> RendererSurfaceHostView {
        let view = RendererSurfaceHostView(frame: NSRect(origin: .zero, size: size))
        view.tabId = tabId
        view.fingerprint = fingerprint
        view.allowsInput = allowsHitTesting
        view.coordinator = coordinator
        coordinator.rendererHostView = view
        coordinator.hasConnection = true
        view.apply(size: size, isOverviewMode: coordinator.isOverviewMode)
        view.apply(config: config)
        return view
    }

    func updateNSView(_ view: RendererSurfaceHostView, context: Context) {
        view.tabId = tabId
        view.fingerprint = fingerprint
        view.coordinator = coordinator
        view.allowsInput = allowsHitTesting
        coordinator.rendererHostView = view
        coordinator.currentViewportSize = size
        view.apply(size: size, isOverviewMode: coordinator.isOverviewMode)
        view.apply(config: config)
    }

    static func dismantleNSView(
        _ view: RendererSurfaceHostView,
        coordinator: ()
    ) {
        // The coordinator owns the Renderer lifecycle. SwiftUI may dismantle the
        // representable during transitions, so do not release the Tab here.
        view.prepareForReuse()
    }
}

final class RendererSurfaceHostView: NSView {
    weak var coordinator: ViewBridgeCoordinator?
    var tabId: UUID?
    var fingerprint = ""
    var allowsInput = true

    private var hostLayer: CALayer?
    private var contextId: UInt32 = 0
    private var trackingAreaReference: NSTrackingArea?
    private var surfaceObserver: NSObjectProtocol?
    private var crashObserver: NSObjectProtocol?
    private var configureTask: Task<Void, Never>?
    private var lastConfig: [String: Any]?
    private var lastConfigData: Data?
    private var lastConfiguredFingerprint: String?
    private var lastSentSize: CGSize?
    private var recoveryAttempt = 0
    private var accessibilityObserver: NSObjectProtocol?
    private var caretObserver: NSObjectProtocol?
    private let caretLayer = CALayer()
    private var accessibilityElements: [RemoteAccessibilityElement] = []
    private var accessibilityById: [UInt64: RemoteAccessibilityElement] = [:]
    private var accessibilityTree = Actrium_Ipc_AccessibilityTree()

    private static let layerHostClass: AnyClass? = NSClassFromString("CALayerHost")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setup()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setup()
    }

    deinit {
        configureTask?.cancel()
        if let surfaceObserver {
            NotificationCenter.default.removeObserver(surfaceObserver)
        }
        if let crashObserver {
            NotificationCenter.default.removeObserver(crashObserver)
        }
        if let accessibilityObserver {
            NotificationCenter.default.removeObserver(accessibilityObserver)
        }
        if let caretObserver {
            NotificationCenter.default.removeObserver(caretObserver)
        }
    }

    private func setup() {
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.10, alpha: 1).cgColor
        layer?.masksToBounds = true
        caretLayer.backgroundColor = NSColor.controlAccentColor.cgColor
        caretLayer.zPosition = 1
        caretLayer.isHidden = true
        layer?.addSublayer(caretLayer)
        caretObserver = NotificationCenter.default.addObserver(
            forName: .rendererCaretUpdated, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self, let tabId = self.tabId, notification.userInfo?["tabId"] as? UUID == tabId else { return }
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            defer { CATransaction.commit() }
            guard let rect = notification.userInfo?["rect"] as? CGRect, self.window?.firstResponder === self,
                  rect.width > 0, rect.height > 0, self.bounds.intersects(rect) else {
                self.caretLayer.isHidden = true
                return
            }
            self.caretLayer.frame = self.convertToLayer(rect)
            self.caretLayer.isHidden = false
        }

        surfaceObserver = NotificationCenter.default.addObserver(
            forName: .rendererSurfaceUpdated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self,
                  let tabId = notification.userInfo?["tabId"] as? UUID,
                  tabId == self.tabId,
                  let contextId = notification.userInfo?["contextId"] as? UInt32 else {
                return
            }
            self.installContext(contextId)
        }

        accessibilityObserver = NotificationCenter.default.addObserver(
            forName: .rendererAccessibilityUpdated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self,
                  let tabId = notification.userInfo?["tabId"] as? UUID,
                  tabId == self.tabId,
                  let tree = notification.userInfo?["tree"] as? Actrium_Ipc_AccessibilityTree else {
                return
            }
            self.applyAccessibilityTree(tree)
        }

        crashObserver = NotificationCenter.default.addObserver(
            forName: .rendererProcessCrashed,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self,
                  let tabId = notification.userInfo?["tabId"] as? UUID,
                  tabId == self.tabId else {
                return
            }
            self.showPlaceholder()
            self.coordinator?.hasConnection = false
            self.coordinator?.xpcServiceReady = false
            self.coordinator?.allocatedServiceId = nil
            self.recoverRendererAfterCrash()
        }
    }

    func apply(config: [String: Any]) {
        guard let tabId,
              !fingerprint.isEmpty,
              JSONSerialization.isValidJSONObject(config),
              let data = try? JSONSerialization.data(
                  withJSONObject: config,
                  options: [.sortedKeys]
              ) else {
            return
        }

        let requestedFingerprint = fingerprint
        guard data != lastConfigData
                || requestedFingerprint != lastConfiguredFingerprint else {
            return
        }

        lastConfig = config
        lastConfigData = data
        lastConfiguredFingerprint = requestedFingerprint
        configureTask?.cancel()
        configureTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                // A Tab can navigate from a built-in page to an ordinary app
                // without SwiftUI recreating this NSView. Always reconcile the
                // allocation before loading so an identity change cannot retain
                // the previous process.
                let previousRendererId = self.coordinator?.allocatedServiceId
                let rendererId = try RendererProcessPool.shared.allocateRenderer(
                    for: tabId,
                    fingerprint: requestedFingerprint
                )
                guard !Task.isCancelled,
                      self.tabId == tabId,
                      self.fingerprint == requestedFingerprint else {
                    return
                }

                if previousRendererId != nil, previousRendererId != rendererId {
                    self.showPlaceholder()
                    self.coordinator?.xpcServiceReady = false
                    self.coordinator?.hasConnection = false
                }
                self.coordinator?.allocatedServiceId = rendererId

                let contextId = try await RendererProcessPool.shared.prepareTab(
                    tabId: tabId,
                    config: config
                )
                guard !Task.isCancelled,
                      self.tabId == tabId,
                      self.fingerprint == requestedFingerprint else {
                    return
                }
                self.installContext(contextId)
                RendererProcessPool.shared.requestAccessibilityTree(for: tabId)
                RendererProcessPool.shared.updateViewport(
                    tabId: tabId,
                    size: self.bounds.size,
                    isOverviewMode: self.coordinator?.isOverviewMode ?? false
                )
                self.recoveryAttempt = 0
                self.coordinator?.xpcServiceReady = true
                self.coordinator?.hasConnection = true
                // Do not call onConnectionEstablished here. This runs after
                // every successful config apply, and its persistent closure
                // captures the tab from the first SwiftUI render, so it would
                // resend stale unifiedScript/reloadKey over the new config.
            } catch is CancellationError {
                return
            } catch {
                NSLog(
                    "Renderer configuration failed for %@: %@",
                    tabId.uuidString,
                    error.localizedDescription
                )
                if self.fingerprint == requestedFingerprint {
                    self.lastConfigData = nil
                    self.lastConfiguredFingerprint = nil
                }
                self.coordinator?.xpcServiceReady = false
            }
        }
    }

    func apply(size: CGSize, isOverviewMode: Bool) {
        guard size.width > 0, size.height > 0 else { return }
        setFrameSize(size)
        hostLayer?.frame = bounds
        guard size != lastSentSize, let tabId else { return }
        lastSentSize = size
        RendererProcessPool.shared.updateViewport(
            tabId: tabId,
            size: size,
            isOverviewMode: isOverviewMode
        )
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        configureTask?.cancel()
        configureTask = nil
    }

    func captureSnapshot() -> NSImage? {
        guard bounds.width > 0, bounds.height > 0,
              let bitmap = bitmapImageRepForCachingDisplay(in: bounds) else {
            return nil
        }
        cacheDisplay(in: bounds, to: bitmap)
        let image = NSImage(size: bounds.size)
        image.addRepresentation(bitmap)
        return image
    }

    private func installContext(_ newContextId: UInt32) {
        guard newContextId != 0, newContextId != contextId else { return }
        guard let layerHostClass = Self.layerHostClass,
              let newLayer = layerHostClass.alloc() as? CALayer else {
            NSLog("CALayerHost is unavailable")
            return
        }

        hostLayer?.removeFromSuperlayer()
        _ = newLayer.perform(NSSelectorFromString("init"))
        newLayer.setValue(NSNumber(value: newContextId), forKey: "contextId")
        newLayer.anchorPoint = .zero
        newLayer.frame = bounds
        newLayer.contentsScale = window?.backingScaleFactor
            ?? NSScreen.main?.backingScaleFactor
            ?? 2
        newLayer.masksToBounds = true
        newLayer.actions = [
            "bounds": NSNull(),
            "position": NSNull(),
            "contents": NSNull(),
            "sublayers": NSNull(),
            "transform": NSNull()
        ]
        layer?.addSublayer(newLayer)
        hostLayer = newLayer
        contextId = newContextId
        if let tabId {
            NSLog(
                "Renderer surface installed: tab=%@ context=%u",
                tabId.uuidString,
                newContextId
            )
        }
    }

    private func showPlaceholder() {
        contextId = 0
        caretLayer.isHidden = true
        hostLayer?.removeFromSuperlayer()
        hostLayer = nil
    }

    private func recoverRendererAfterCrash() {
        guard recoveryAttempt < 3,
              let tabId,
              !fingerprint.isEmpty,
              let config = lastConfig else {
            return
        }

        recoveryAttempt += 1
        let attempt = recoveryAttempt
        configureTask?.cancel()
        configureTask = Task { @MainActor [weak self] in
            guard let self else { return }
            if attempt > 1 {
                try? await Task.sleep(for: .milliseconds(200 * attempt))
            }
            guard !Task.isCancelled, self.tabId == tabId else { return }

            do {
                let rendererId = try RendererProcessPool.shared.allocateRenderer(
                    for: tabId,
                    fingerprint: self.fingerprint
                )
                self.coordinator?.allocatedServiceId = rendererId
                self.lastConfigData = nil
                self.apply(config: config)
                NSLog(
                    "Renderer recovery scheduled: tab=%@ attempt=%d renderer=%@",
                    tabId.uuidString,
                    attempt,
                    rendererId
                )
            } catch {
                NSLog(
                    "Renderer recovery failed: tab=%@ attempt=%d error=%@",
                    tabId.uuidString,
                    attempt,
                    error.localizedDescription
                )
                self.recoverRendererAfterCrash()
            }
        }
    }

    override func layout() {
        super.layout()
        hostLayer?.frame = bounds
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingAreaReference {
            removeTrackingArea(trackingAreaReference)
        }
        let area = NSTrackingArea(
            rect: bounds,
            options: [
                .mouseEnteredAndExited,
                .mouseMoved,
                .activeInKeyWindow,
                .inVisibleRect
            ],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        trackingAreaReference = area
    }

    override var acceptsFirstResponder: Bool { true }
    override func resignFirstResponder() -> Bool {
        caretLayer.isHidden = true
        return super.resignFirstResponder()
    }
    override var isFlipped: Bool { true }

    override func isAccessibilityElement() -> Bool { true }

    override func accessibilityRole() -> NSAccessibility.Role { .group }

    override func accessibilityLabel() -> String? {
        let title = coordinator?.tabConfig?.title ?? ""
        return title.isEmpty ? "Application content" : title
    }

    override func accessibilityChildren() -> [Any]? {
        accessibilityElements
    }

    override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        guard let window else { return self }
        let inWindow = window.convertFromScreen(NSRect(origin: point, size: .zero)).origin
        let local = convert(inWindow, from: nil)
        if let nodeId = RemoteAccessibilitySnapshot.hitTest(accessibilityTree, point: local),
           let element = accessibilityById[nodeId] {
            return element
        }
        return self
    }

    override var accessibilityFocusedUIElement: Any? {
        if accessibilityTree.focusedID != 0 {
            return accessibilityById[accessibilityTree.focusedID]
        }
        return self
    }

    func performRemoteAccessibilityAction(_ action: String, nodeId: UInt64) -> Bool {
        guard let tabId else { return false }
        return RendererProcessPool.shared.performAccessibilityAction(
            tabId: tabId,
            nodeId: nodeId,
            action: action
        )
    }

    private func applyAccessibilityTree(_ tree: Actrium_Ipc_AccessibilityTree) {
        accessibilityTree = tree
        var byId: [UInt64: RemoteAccessibilityElement] = [:]
        for node in tree.nodes {
            let element = RemoteAccessibilityElement(node: node, host: self)
            byId[node.nodeID] = element
        }
        for node in tree.nodes {
            guard let element = byId[node.nodeID] else { continue }
            element.setChildren(node.childIds.compactMap { byId[$0] })
        }
        let roots = tree.nodes.filter { $0.parentID == 0 }
        let hostChildren: [RemoteAccessibilityElement]
        if roots.count == 1, let root = roots.first, !root.childIds.isEmpty {
            hostChildren = root.childIds.compactMap { byId[$0] }
        } else {
            hostChildren = roots.compactMap { byId[$0.nodeID] }
        }
        for element in hostChildren {
            element.setAccessibilityParent(self)
        }
        accessibilityElements = hostChildren
        accessibilityById = byId
        NSAccessibility.post(element: self, notification: .layoutChanged)
        if tree.focusedID != 0, let focused = byId[tree.focusedID] {
            NSAccessibility.post(element: focused, notification: .focusedUIElementChanged)
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard allowsInput, super.hitTest(point) != nil else { return nil }
        return self
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        allowsInput
    }

    override func mouseDown(with event: NSEvent) {
        let focused = window?.makeFirstResponder(self) == true
        #if DEBUG
        if let tabId {
            let point = convert(event.locationInWindow, from: nil)
            NSLog("Renderer mouse down: tab=%@ x=%.0f y=%.0f focused=%d",
                  tabId.uuidString, point.x, point.y, focused ? 1 : 0)
        }
        #endif
        forwardMouse(event, type: .mouseDown)
    }

    override func mouseUp(with event: NSEvent) {
        forwardMouse(event, type: .mouseUp)
    }

    override func rightMouseDown(with event: NSEvent) {
        forwardMouse(event, type: .mouseDown, button: 1)
    }

    override func rightMouseUp(with event: NSEvent) {
        forwardMouse(event, type: .mouseUp, button: 1)
    }

    override func otherMouseDown(with event: NSEvent) {
        forwardMouse(event, type: .mouseDown, button: 2)
    }

    override func otherMouseUp(with event: NSEvent) {
        forwardMouse(event, type: .mouseUp, button: 2)
    }

    override func mouseMoved(with event: NSEvent) {
        forwardMouse(event, type: .mouseMove)
    }

    override func mouseDragged(with event: NSEvent) {
        forwardMouse(event, type: .mouseMove)
    }

    override func rightMouseDragged(with event: NSEvent) {
        forwardMouse(event, type: .mouseMove, button: 1)
    }

    override func otherMouseDragged(with event: NSEvent) {
        forwardMouse(event, type: .mouseMove, button: 2)
    }

    override func mouseEntered(with event: NSEvent) {
        forwardMouse(event, type: .mouseEntered)
    }

    override func mouseExited(with event: NSEvent) {
        forwardMouse(event, type: .mouseExited)
    }

    override func scrollWheel(with event: NSEvent) {
        guard let tabId else { return }
        let point = convert(event.locationInWindow, from: nil)
        var input = Actrium_Ipc_InputEvent()
        input.type = .scroll
        input.x = point.x
        input.y = point.y
        input.scrollDeltaX = event.scrollingDeltaX
        input.scrollDeltaY = event.scrollingDeltaY
        input.modifiers = encodedModifiers(event.modifierFlags)
        RendererProcessPool.shared.sendInput(input, to: tabId)
    }

    override func keyDown(with event: NSEvent) {
        forwardKey(event, type: .keyDown)
    }

    override func keyUp(with event: NSEvent) {
        forwardKey(event, type: .keyUp)
    }

    private func forwardMouse(
        _ event: NSEvent,
        type: Actrium_Ipc_InputEvent.EventType,
        button: UInt32 = 0
    ) {
        guard allowsInput, let tabId else { return }
        let point = convert(event.locationInWindow, from: nil)
        var input = Actrium_Ipc_InputEvent()
        input.type = type
        input.x = point.x
        input.y = point.y
        input.button = button
        input.modifiers = encodedModifiers(event.modifierFlags)
        RendererProcessPool.shared.sendInput(input, to: tabId)
    }

    private func forwardKey(
        _ event: NSEvent,
        type: Actrium_Ipc_InputEvent.EventType
    ) {
        guard allowsInput, let tabId else { return }
        var input = Actrium_Ipc_InputEvent()
        input.type = type
        input.keyCode = UInt32(event.keyCode)
        input.characters = event.characters ?? ""
        input.modifiers = encodedModifiers(event.modifierFlags)
        RendererProcessPool.shared.sendInput(input, to: tabId)
    }

    private func encodedModifiers(_ flags: NSEvent.ModifierFlags) -> UInt32 {
        var value: UInt32 = 0
        if flags.contains(.shift) { value |= 1 }
        if flags.contains(.control) { value |= 2 }
        if flags.contains(.option) { value |= 4 }
        if flags.contains(.command) { value |= 8 }
        if flags.contains(.capsLock) { value |= 16 }
        return value
    }
}
