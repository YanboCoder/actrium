import ActriumIPC
import AppKit
import Darwin
import Foundation
import QuartzCore
import React

/// Entry-point and lifecycle owner for a manually spawned Renderer Helper.
@MainActor
final class RendererProcessController {
    nonisolated static let rendererIdArgument = "--renderer-id"
    nonisolated static let fingerprintArgument = "--fingerprint"
    nonisolated static let ipcStdinArgument = "--ipc-stdin"
    nonisolated static let hostPidArgument = "--host-pid"

    nonisolated static var isRendererProcess: Bool {
        ProcessInfo.processInfo.arguments.contains(rendererIdArgument)
    }

    /// Returns `true` when the caller is running in the Renderer process. In
    /// that case the legacy distributed-notification fallback must not run,
    /// even if the Broker has already disconnected.
    @discardableResult
    nonisolated static func sendHostEvent(name: String, userInfo: [String: Any] = [:]) -> Bool {
        guard isRendererProcess else { return false }
        guard JSONSerialization.isValidJSONObject(userInfo),
              let data = try? JSONSerialization.data(withJSONObject: userInfo),
            let json = String(data: data, encoding: .utf8)
        else {
            return true
        }
        // React Native may call from its module queue. Only immutable values
        // cross to the renderer's main-actor lifecycle and callback state.
        DispatchQueue.main.async {
            guard let controller = RendererProcessLifetime.shared.controller else { return }
            do {
                try controller.connection.sendHostEvent(name: name, userInfoJSON: json)
            } catch {
                NSLog("ActriumRenderer host event failed: %@ (%@)", name, error.localizedDescription)
            }
        }
        return true
    }

    @discardableResult
    nonisolated static func sendConsoleLog(level: String, source: String, message: String) -> Bool {
        guard isRendererProcess else { return false }
        DispatchQueue.main.async {
            guard let controller = RendererProcessLifetime.shared.controller else { return }
            do {
                try controller.connection.sendConsoleLog(
                    level: level,
                    message: message,
                    source: source
                )
            } catch {
                NSLog("ActriumRenderer console forwarding failed: %@", error.localizedDescription)
            }
        }
        return true
    }

    private struct Arguments {
        let rendererId: String
        let fingerprint: String
        let hostPid: Int32

        static func parse(_ values: [String]) -> Arguments? {
            func value(after key: String) -> String? {
                guard let index = values.firstIndex(of: key),
                      values.indices.contains(index + 1) else {
                    return nil
                }
                return values[index + 1]
            }

            guard let rendererId = value(after: rendererIdArgument),
                  let fingerprint = value(after: fingerprintArgument),
                  values.contains(ipcStdinArgument),
                  let hostPidString = value(after: hostPidArgument),
                  let hostPid = Int32(hostPidString) else {
                return nil
            }

            return Arguments(
                rendererId: rendererId,
                fingerprint: fingerprint,
                hostPid: hostPid
            )
        }
    }

    private let arguments: Arguments
    private let connection: IPCHelperConnection
    private let reactNativeHost = ServiceReactNativeHost.shared
    private var surfaces: [String: RendererTabSurface] = [:]

    private init(arguments: Arguments) {
        self.arguments = arguments
        connection = IPCHelperConnection(
            tabId: arguments.rendererId,
            connectedSocket: STDIN_FILENO
        )
        configureIPC()
    }

    static func runFromCommandLine() -> Never {
        guard let arguments = Arguments.parse(ProcessInfo.processInfo.arguments) else {
            fputs("ActriumRenderer: missing required Broker arguments\n", stderr)
            exit(64)
        }

        let application = NSApplication.shared
        application.setActivationPolicy(.prohibited)
        application.finishLaunching()

        let controller = RendererProcessController(arguments: arguments)
        RendererProcessLifetime.shared.controller = controller

        do {
            try controller.connection.connect()
            try controller.connection.sendReady(
                surfaceId: 0,
                width: 0,
                height: 0,
                rendererId: arguments.rendererId
            )
        } catch {
            fputs("ActriumRenderer: Broker connection failed: \(error)\n", stderr)
            exit(70)
        }

        NSLog(
            "ActriumRenderer ready: renderer=%@ fingerprint=%@ pid=%d host=%d",
            arguments.rendererId,
            arguments.fingerprint,
            ProcessInfo.processInfo.processIdentifier,
            arguments.hostPid
        )
        application.run()
        exit(0)
    }

    private func configureIPC() {
        connection.onLoadContent = { [weak self] request, reply in
            DispatchQueue.main.async {
                self?.loadContent(request, reply: reply)
            }
        }

        connection.onTabUpdateViewport = { [weak self] tabId, width, height, requestId, overview in
            DispatchQueue.main.async {
                self?.updateViewport(
                    tabId: tabId,
                    size: CGSize(width: width, height: height),
                    requestId: requestId,
                    isOverviewMode: overview
                )
            }
        }

        connection.onInputEvent = { [weak self] event in
            DispatchQueue.main.async {
                self?.surfaces[event.tabID]?.dispatch(event)
            }
        }

        connection.onTabGetSurfaceID = { [weak self] tabId, reply in
            DispatchQueue.main.async {
                reply(self?.surfaces[tabId]?.contextId ?? 0)
            }
        }

        connection.onDestroyTab = { [weak self] tabId in
            DispatchQueue.main.async {
                self?.destroyTab(tabId)
            }
        }

        connection.onGetAccessibilityTree = { [weak self] tabId, reply in
            DispatchQueue.main.async {
                reply(self?.surfaces[tabId]?.accessibilityTree() ?? Actrium_Ipc_AccessibilityTree())
            }
        }

        connection.onAccessibilityAction = { [weak self] request, reply in
            DispatchQueue.main.async {
                let ok = self?.surfaces[request.tabID]?.performAccessibility(
                    action: request.action,
                    nodeId: request.nodeID
                ) ?? false
                if ok {
                    self?.publishAccessibilityTree(tabId: request.tabID)
                }
                reply(ok, ok ? nil : "Accessibility action failed")
            }
        }

        connection.onGetMetrics = { reply in
            reply(RendererProcessController.metrics())
        }

        connection.onTabExecuteJS = { _, _, reply in
            reply(false, nil, "Renderer evaluateScript is not implemented")
        }

        connection.onTerminate = { [weak self] reason in
            DispatchQueue.main.async {
                self?.terminate(reason: reason)
            }
        }

        connection.onDisconnected = { [weak self] in
            DispatchQueue.main.async {
                self?.terminate(reason: "Broker disconnected")
            }
        }
    }

    private func loadContent(
        _ request: Actrium_Ipc_LoadContentRequest,
        reply: @escaping (Bool, String?) -> Void
    ) {
        let tabId = request.tabID
        guard !tabId.isEmpty else {
            reply(false, "Missing tabId")
            return
        }

        var config: [String: Any] = [:]
        if let data = request.configJson.data(using: .utf8),
           let decoded = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            config = decoded
        }

        config["tabId"] = tabId
        if config["title"] == nil {
            config["title"] = request.title
        }
        if config["backgroundColor"] == nil {
            config["backgroundColor"] = request.backgroundColor
        }
        if config["unifiedScript"] == nil, request.contentType == "javascript" {
            config["unifiedScript"] = request.content
        }

        let width = (config["containerWidth"] as? NSNumber)?.doubleValue ?? 1280
        let height = (config["containerHeight"] as? NSNumber)?.doubleValue ?? 720
        let size = CGSize(width: max(width, 1), height: max(height, 1))

        let surface: RendererTabSurface
        if let existing = surfaces[tabId] {
            surface = existing
            surface.resize(to: size)
        } else {
            let entry = reactNativeHost.createRendererEntry(tabId: tabId, size: size)
            guard let created = RendererTabSurface(tabId: tabId, entry: entry, size: size) else {
                reply(false, "Failed to create remote CoreAnimation context")
                return
            }
            surfaces[tabId] = created
            surface = created
        }

        reactNativeHost.applyRendererConfig(config) { [weak self, weak surface] success, error in
            guard let self, let surface else {
                reply(false, "Renderer surface released")
                return
            }

            if success {
                surface.commit()
                self.publishAccessibilityTree(tabId: tabId)
                try? self.connection.sendSurfaceUpdated(
                    surfaceId: surface.contextId,
                    tabId: tabId
                )
                NSLog(
                    "Renderer surface ready: renderer=%@ tab=%@ context=%u",
                    self.arguments.rendererId,
                    tabId,
                    surface.contextId
                )
            }
            reply(success, error)
        }
    }

    private func updateViewport(
        tabId: String,
        size: CGSize,
        requestId: UInt32,
        isOverviewMode: Bool
    ) {
        guard let surface = surfaces[tabId],
              size.width > 0,
              size.height > 0 else {
            return
        }

        surface.resize(to: size)
        reactNativeHost.resizeRendererEntry(
            tabId: tabId,
            size: size,
            requestId: requestId,
            isOverviewMode: isOverviewMode
        )
        surface.commit()
        try? connection.sendViewportApplied(
            tabId: tabId,
            requestId: requestId,
            width: size.width,
            height: size.height
        )
        publishAccessibilityTree(tabId: tabId)
    }

    /// Called from the Fabric surface-presenter observer after mount
    /// instructions are applied. Re-commits the matching tab surface so the
    /// new CAContext frame reaches the Host instead of waiting for a resize.
    func commitSurface(forRootTag rootTag: Int) {
        let surfaceSelector = NSSelectorFromString("surface")
        let rootTagSelector = NSSelectorFromString("rootTag")
        for surface in surfaces.values {
            guard let rootView = surface.entry.rootView else { continue }
            let rootObject = rootView as AnyObject
            guard rootObject.responds(to: surfaceSelector) else { continue }
            guard let surfaceObject = rootObject.value(forKey: "surface") as AnyObject?,
                  surfaceObject.responds(to: rootTagSelector) else { continue }
            let tagValue = surfaceObject.value(forKey: "rootTag")
            let tag = (tagValue as? NSNumber).map { Int(truncating: $0) } ?? (tagValue as? Int)
            if tag == rootTag {
                surface.commit()
            }
        }
    }

    func publishAccessibilityTree(tabId: String) {
        guard let surface = surfaces[tabId] else { return }
        try? connection.sendAccessibilityTree(surface.accessibilityTree())
    }

    func publishCaret(tabId: String, rect: CGRect?) {
        guard let encoded = try? JSONEncoder().encode(rect) else { return }
        try? connection.sendHostEvent(
            name: "com.actrium.rendererCaret",
            userInfoJSON: String(decoding: encoded, as: UTF8.self),
            tabId: tabId
        )
    }

    private func destroyTab(_ tabId: String) {
        surfaces.removeValue(forKey: tabId)?.close()
        reactNativeHost.destroyRendererEntry(tabId: tabId)
    }

    private func terminate(reason: String) {
        NSLog("ActriumRenderer terminating: %@", reason)
        let tabIds = Array(surfaces.keys)
        for tabId in tabIds {
            destroyTab(tabId)
        }
        connection.disconnect()
        NSApplication.shared.terminate(nil)
    }

    private static func metrics() -> Actrium_Ipc_MetricsResponse {
        var response = Actrium_Ipc_MetricsResponse()
        var usage = rusage()
        if getrusage(RUSAGE_SELF, &usage) == 0 {
            #if arch(arm64)
            response.memoryUsage = UInt64(max(usage.ru_maxrss, 0))
            #else
            response.memoryUsage = UInt64(max(usage.ru_maxrss, 0)) * 1024
            #endif
        }
        return response
    }
}

/// Keeps the top-level controller alive for the lifetime of NSApplication.
@MainActor
private final class RendererProcessLifetime {
    static let shared = RendererProcessLifetime()
    var controller: RendererProcessController?
}

/// Fabric mount observer: commit the matching Renderer surface after the
/// native mount instructions finish, so reload content is published without
/// waiting for a window resize.
final class RendererSurfaceMountObserver: NSObject {
    static let shared = RendererSurfaceMountObserver()

    @objc(didMountComponentsWithRootTag:)
    func didMountComponents(rootTag: Int) {
        Task { @MainActor in
            RendererProcessLifetime.shared.controller?.commitSurface(
                forRootTag: rootTag
            )
        }
    }
}

/// A background renderer window must look active to native text controls.
/// NSTextView uses the key-window state to install its insertion indicator;
/// activating the helper NSApplication itself would steal focus from the Host.
private final class RendererWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var isKeyWindow: Bool { true }
}

/// One Tab root and its remotely hosted CoreAnimation context.
@MainActor
private final class RendererTabSurface {
    let tabId: String
    let entry: TabViewControllerEntry
    let container: ResizableContainerView
    let window: NSWindow
    let context: RendererCAContext
    private var generation: UInt64 = 0
    private var hasCommitted = false
    private weak var focusedEditor: NSTextView?
    private var lastCaretRect: CGRect?
    private var accessibilityObservers: [NSObjectProtocol] = []
    private var accessibilityPublishWork: DispatchWorkItem?
    private var pressedRecognizers: [UInt32: [NSGestureRecognizer]] = [:]
    private var textInputPresses: Set<UInt32> = []

    var contextId: UInt32 {
        context.contextId
    }

    init?(tabId: String, entry: TabViewControllerEntry, size: CGSize) {
        guard let container = entry.containerView,
              let context = RendererCAContext() else {
            return nil
        }

        self.tabId = tabId
        self.entry = entry
        self.container = container
        self.context = context

        window = RendererWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        window.acceptsMouseMovedEvents = true
        window.contentView = container
        window.setFrameOrigin(NSPoint(x: -100_000, y: -100_000))
        window.orderOut(nil)

        container.frame = NSRect(origin: .zero, size: size)
        container.wantsLayer = true
        commit()
        observeAccessibility()
    }

    func accessibilityTree() -> Actrium_Ipc_AccessibilityTree {
        generation += 1
        return RendererAccessibility.snapshot(
            tabId: tabId,
            root: container,
            generation: generation
        )
    }

    func performAccessibility(action: String, nodeId: UInt64) -> Bool {
        RendererAccessibility.perform(action: action, nodeId: nodeId, in: container)
    }

    func resize(to size: CGSize) {
        guard size.width > 0, size.height > 0 else { return }
        window.setContentSize(size)
        container.applyHostSize(size)
        container.layer?.frame = CGRect(origin: .zero, size: size)
    }

    func commit() {
        if context.layer !== container.layer {
            context.layer = container.layer
        }
        // The first commit happens on an empty view tree, so lay it out once.
        // Later commits rely on AppKit/RN marking the container dirty
        // (addSubview, surface stage change, resize); Fabric sets frames
        // directly, so a subtree-only dirty state is intentionally not
        // remeasured here.
        if !hasCommitted || container.needsLayout {
            container.needsLayout = true
            container.layoutSubtreeIfNeeded()
            hasCommitted = true
        }
        publishCaret()
        CATransaction.flush()
    }

    func close() {
        accessibilityPublishWork?.cancel()
        accessibilityPublishWork = nil
        for observer in accessibilityObservers {
            NotificationCenter.default.removeObserver(observer)
        }
        accessibilityObservers.removeAll()
        context.layer = nil
        window.contentView = nil
        window.close()
    }

    private func observeAccessibility() {
        let names: [Notification.Name] = [
            Notification.Name(NSAccessibility.Notification.focusedUIElementChanged.rawValue),
            Notification.Name(NSAccessibility.Notification.layoutChanged.rawValue),
            Notification.Name(NSAccessibility.Notification.valueChanged.rawValue),
            Notification.Name(NSAccessibility.Notification.titleChanged.rawValue),
            Notification.Name(NSAccessibility.Notification.selectedChildrenChanged.rawValue)
        ]
        for name in names {
            let observer = NotificationCenter.default.addObserver(
                forName: name,
                object: nil,
                queue: .main
            ) { [weak self] notification in
                // NotificationCenter delivers this observer on OperationQueue.main.
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if let view = notification.object as? NSView, view.window !== self.window {
                        return
                    }
                    self.scheduleAccessibilityPublish()
                }
            }
            accessibilityObservers.append(observer)
        }
    }

    private func scheduleAccessibilityPublish() {
        accessibilityPublishWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.pushAccessibilityTree()
        }
        accessibilityPublishWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }

    private func pushAccessibilityTree() {
        guard let controller = RendererProcessLifetime.shared.controller else { return }
        controller.publishAccessibilityTree(tabId: tabId)
    }

    func dispatch(_ input: Actrium_Ipc_InputEvent) {
        let modifiers = modifierFlags(from: input.modifiers)
        // IPC coordinates are top-left based, while AppKit window events use
        // bottom-left based window coordinates.
        let location = container.convert(NSPoint(x: input.x, y: input.y), to: nil)
        let timestamp = ProcessInfo.processInfo.systemUptime

        switch input.type {
        case .mouseDown, .mouseUp, .mouseMove:
            let type: NSEvent.EventType
            if input.type == .mouseMove {
                type = .mouseMoved
            } else if input.button == 1 {
                type = input.type == .mouseDown ? .rightMouseDown : .rightMouseUp
            } else if input.button == 2 {
                type = input.type == .mouseDown ? .otherMouseDown : .otherMouseUp
            } else {
                type = input.type == .mouseDown ? .leftMouseDown : .leftMouseUp
            }

            guard let event = NSEvent.mouseEvent(
                with: type,
                location: location,
                modifierFlags: modifiers,
                timestamp: timestamp,
                windowNumber: window.windowNumber,
                context: nil,
                eventNumber: 0,
                clickCount: input.type == .mouseMove ? 0 : 1,
                pressure: input.type == .mouseDown ? 1 : 0
            ) else {
                return
            }

            let hitView = hitTest(at: location)
            // Editable RN controls keep AppKit's native focus path; routing
            // their clicks through the JS responder revokes first responder.
            if input.type == .mouseDown {
                let ancestors = sequence(first: hitView, next: { $0?.superview })
                    .prefix(while: { $0 != nil }).compactMap { $0 }
                let textInput = ancestors.first { $0 is NSTextView || $0 is NSTextField }
                if input.button == 0, let textInput {
                    textInputPresses.insert(input.button)
                    focusTextInput(textInput, at: location, modifiers: modifiers)
                    publishCaret()
                    // AppKit adds the field editor to the offscreen view
                    // hierarchy on focus. Republish after that update.
                    DispatchQueue.main.async { [weak self] in
                        guard let self, self.window.contentView === self.container else { return }
                        self.commit()
                    }
                    return
                }
                window.makeFirstResponder(hitView ?? container)
                let recognizers = ancestors.flatMap(\.gestureRecognizers)
                forwardMouse(event, to: recognizers)
                pressedRecognizers[input.button] = recognizers
            } else if input.type == .mouseUp {
                if textInputPresses.remove(input.button) != nil { return }
                if let recognizers = pressedRecognizers.removeValue(forKey: input.button) {
                    forwardMouse(event, to: recognizers)
                }
            }
            window.sendEvent(event)
            if input.type == .mouseDown { publishCaret() }

        case .scroll:
            guard let hitView = hitTest(at: location) else { return }
            let scrollView = sequence(first: hitView, next: \.superview)
                .compactMap { $0 as? NSScrollView }
                .first { candidate in
                    guard let rnScrollView = candidate as? RCTUIScrollView else { return true }
                    return rnScrollView.isScrollEnabled
                }
            guard let scrollView else { return }

            // An offscreen window cannot route a CGEvent-backed NSEvent to its
            // content. NSClipView scrolling also posts the bounds notification
            // that drives RN's native scrollViewDidScroll delegate path.
            let clipView = scrollView.contentView
            let origin = clipView.bounds.origin
            let target = NSPoint(
                x: origin.x - input.scrollDeltaX,
                y: origin.y - input.scrollDeltaY
            )
            let constrainedTarget = clipView.constrainBoundsRect(
                NSRect(origin: target, size: clipView.bounds.size)
            ).origin
            clipView.scroll(to: constrainedTarget)
            commit()

        case .keyDown, .keyUp:
            guard let event = NSEvent.keyEvent(
                with: input.type == .keyDown ? .keyDown : .keyUp,
                location: location,
                modifierFlags: modifiers,
                timestamp: timestamp,
                windowNumber: window.windowNumber,
                context: nil,
                characters: input.characters,
                charactersIgnoringModifiers: input.characters,
                isARepeat: false,
                keyCode: UInt16(input.keyCode)
            ) else {
                return
            }
            if let textView = window.firstResponder as? NSTextView {
                if input.type == .keyDown { textView.keyDown(with: event) }
                else { textView.keyUp(with: event) }
            } else {
                window.sendEvent(event)
            }
            if input.type == .keyDown {
                // Fabric publishes content changes after mounting. A caret
                // movement alone does not need to flush the remote surface.
                publishCaret()
            }

        case .mouseEntered, .mouseExited, .UNRECOGNIZED(_):
            break
        }
    }

    private func hitTest(at location: NSPoint) -> NSView? {
        let point = container.superview?.convert(location, from: nil) ?? location
        return container.hitTest(point)
    }

    private func focusTextInput(_ view: NSView, at location: NSPoint, modifiers: NSEvent.ModifierFlags) {
        // AppKit's native mouse tracking waits synchronously for mouse-up.
        // Renderer mouse-up arrives on the main queue over IPC.
        let focused = window.makeFirstResponder(view)
        let editor = (view as? NSTextField)?.currentEditor() as? NSTextView ?? view as? NSTextView
        focusedEditor = focused ? editor : nil
        guard focused, let editor, !modifiers.contains(.shift) else { return }
        let point = editor.convert(location, from: nil)
        let index = editor.characterIndexForInsertion(at: point)
        guard index != NSNotFound, index <= (editor.string as NSString).length else { return }
        editor.setSelectedRange(NSRange(location: index, length: 0))
    }

    private func publishCaret() {
        let rect = caretRect()
        guard rect != lastCaretRect else { return }
        lastCaretRect = rect
        RendererProcessLifetime.shared.controller?.publishCaret(tabId: tabId, rect: rect)
    }

    private func caretRect() -> CGRect? {
        guard let editor = focusedEditor, window.firstResponder === editor, editor.isEditable,
              editor.selectedRange().length == 0 else {
            return nil
        }
        let screenRect = editor.firstRect(forCharacterRange: editor.selectedRange(), actualRange: nil)
        let localRect: NSRect
        if editor.string.isEmpty {
            // AppKit can return an empty character rectangle for an empty field.
            let origin = editor.textContainerOrigin
            let inset = editor.textContainer?.lineFragmentPadding ?? 0
            let lineHeight = editor.font.map {
                editor.layoutManager?.defaultLineHeight(for: $0) ?? $0.pointSize
            } ?? NSFont.systemFontSize
            let height = min(editor.bounds.height, lineHeight)
            let rect = NSRect(x: origin.x + inset, y: editor.bounds.midY - height / 2,
                              width: 2, height: height)
            localRect = container.convert(rect, from: editor)
        } else {
            guard screenRect.height > 0 else {
                return nil
            }
            localRect = container.convert(window.convertFromScreen(screenRect), from: nil)
        }
        return CGRect(x: localRect.minX, y: localRect.minY, width: 2, height: localRect.height)
    }

    private func forwardMouse(_ event: NSEvent, to recognizers: [NSGestureRecognizer]) {
        // Synthetic AppKit events do not drive recognizers in this offscreen
        // window. Release only the recognizers that received the press.
        for recognizer in recognizers {
            switch event.type {
            case .leftMouseDown: recognizer.mouseDown(with: event)
            case .rightMouseDown: recognizer.rightMouseDown(with: event)
            case .otherMouseDown: recognizer.otherMouseDown(with: event)
            case .leftMouseUp: recognizer.mouseUp(with: event)
            case .rightMouseUp: recognizer.rightMouseUp(with: event)
            case .otherMouseUp: recognizer.otherMouseUp(with: event)
            default: break
            }
        }
    }

    private func modifierFlags(from encoded: UInt32) -> NSEvent.ModifierFlags {
        var result: NSEvent.ModifierFlags = []
        if encoded & 1 != 0 { result.insert(.shift) }
        if encoded & 2 != 0 { result.insert(.control) }
        if encoded & 4 != 0 { result.insert(.option) }
        if encoded & 8 != 0 { result.insert(.command) }
        if encoded & 16 != 0 { result.insert(.capsLock) }
        return result
    }
}

/// Dynamic wrapper for the private CAContext used by Chromium/WebKit style
/// cross-process layer hosting.
private final class RendererCAContext {
    private let object: NSObject

    var contextId: UInt32 {
        if let number = object.value(forKey: "contextId") as? NSNumber {
            return number.uint32Value
        }
        return object.value(forKey: "contextId") as? UInt32 ?? 0
    }

    var layer: CALayer? {
        get {
            object.value(forKey: "layer") as? CALayer
        }
        set {
            object.setValue(newValue, forKey: "layer")
        }
    }

    init?() {
        guard let contextClass = NSClassFromString("CAContext") else {
            return nil
        }

        let options: [String: Any] = [
            "colorSpace": CGColorSpaceCreateDeviceRGB(),
            "displayId": CGMainDisplayID()
        ]

        let connectionSelector = NSSelectorFromString("contextWithCGSConnection:options:")
        if contextClass.responds(to: connectionSelector),
           let symbol = dlsym(dlopen(nil, RTLD_LAZY), "CGSMainConnectionID") {
            typealias ConnectionFunction = @convention(c) () -> UInt32
            let connection = unsafeBitCast(symbol, to: ConnectionFunction.self)()
            let implementation = contextClass.method(for: connectionSelector)
            typealias Factory = @convention(c) (
                AnyClass,
                Selector,
                UInt32,
                [String: Any]
            ) -> NSObject?
            if let created = unsafeBitCast(implementation, to: Factory.self)(
                contextClass,
                connectionSelector,
                connection,
                options
            ) {
                object = created
                return
            }
        }

        let remoteSelector = NSSelectorFromString("remoteContextWithOptions:")
        if contextClass.responds(to: remoteSelector) {
            let implementation = contextClass.method(for: remoteSelector)
            typealias Factory = @convention(c) (
                AnyClass,
                Selector,
                [String: Any]
            ) -> NSObject?
            if let created = unsafeBitCast(implementation, to: Factory.self)(
                contextClass,
                remoteSelector,
                options
            ) {
                object = created
                return
            }
        }

        return nil
    }
}
