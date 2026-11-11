//
//  ViewBridgeHostView.swift
//  Actrium
//
//  Host view that displays content from a ViewBridge XPC service.
//  Uses NSRemoteViewController to request and embed remote views.
//
//  New structure:Remote view size Follow TabPanel Actual size
//  - Focus Mode: Approach full screen size
//  - Overview Mode: Card size
//  - Animation process: Scalable Transition
//  - RN Custom Layout
//

import Cocoa
import SwiftUI
import Combine

/// Millisecond timestamp for cross-process log correlation
private func ts() -> String {
    let now = Date()
    let formatter = DateFormatter()
    formatter.dateFormat = "HH:mm:ss.SSS"
    return formatter.string(from: now)
}

/// Write to shared debug log file (for cross-process log correlation)
private func ipcLog(_ message: String) {
    let timestamp = ts()
    let line = "[\(timestamp)] [HOST] \(message)\n"
    NSLog("[\(timestamp)] [HOST] %@", message)
    // Also write to shared file
    let logPath = "/tmp/actrium_ipc_debug.log"
    if let handle = FileHandle(forWritingAtPath: logPath) {
        handle.seekToEndOfFile()
        if let data = line.data(using: .utf8) {
            handle.write(data)
        }
        handle.closeFile()
    } else {
        FileManager.default.createFile(atPath: logPath, contents: line.data(using: .utf8))
    }
}

// MARK: - ViewBridge Host View

/// A view that hosts content from a ViewBridge XPC service.
/// Remote view size Follow TabPanel Real size,RN Self-adapted layout.
public class ViewBridgeHostView: NSView {

    // MARK: - Properties

    /// The remote view controller received from the service
    private var remoteViewController: NSViewController?

    /// The service bundle identifier
    private let serviceBundleIdentifier: String

    /// Public accessor for service bundle identifier (used by coordinator for re-sending pendingTabId)
    var serviceId: String { serviceBundleIdentifier }

    /// The view controller class name in the service
    private let serviceViewControllerClassName: String

    /// Loading state
    private var isLoading = false

    /// Current content size - Follow TabPanel Actual size
    /// Accessible by coordinator for sending size notifications
    var currentContentSize: CGSize = CGSize(width: 1280, height: 720)

    /// Callback when XPC connection is established
    public var onConnectionEstablished: (() -> Void)?

    /// Check if remote view controller is connected (public accessor)
    public var hasRemoteViewController: Bool {
        remoteViewController != nil
    }

    /// Reference to coordinator for lifecycle management
    /// When SwiftUI replaces the view, we only disconnect if coordinator.hostView still points to us
    weak var coordinator: ViewBridgeCoordinator?

    /// Anti-duplication: Size last sent
    private var lastSentNotificationSize: CGSize?

    /// Resize start size capture
    private var resizeStartContentSize: NSSize?

    /// Use flipped Coordinate system, with ContainerView and SwiftUI Consistency
    /// Ensure remoteView From Top Left (0,0) Start the correct layout
    public override var isFlipped: Bool { true }

    /// Allow hit testing(Overview Mode to falseLet's get the gestures through the lower level. Grid)
    /// From coordinator.isOverviewMode Get Status
    var allowsHitTesting: Bool = true {
        didSet {
            // 🔑 Yes. Overview Disable drag target in mode to allow drag event to penetrate lower level Grid
            if !allowsHitTesting {
                unregisterDraggedTypes()
            }
        }
    }

    /// Override hitTest to allow events to pass through in Overview mode
    /// This enables tab dragging in Overview mode
    public override func hitTest(_ point: NSPoint) -> NSView? {
        // In Overview mode, let events pass through to the underlying Grid
        if !allowsHitTesting {
            return nil
        }
        return super.hitTest(point)
    }

    // MARK: - Initialization

    /// Initialize with service configuration
    public init(
        serviceBundleIdentifier bundleIdentifier: String,
        viewControllerClassName className: String
    ) {
        self.serviceBundleIdentifier = bundleIdentifier
        self.serviceViewControllerClassName = className
        super.init(frame: .zero)
        setupView()
        ipcLog("🆕 ViewBridgeHostView init: service=\(bundleIdentifier.components(separatedBy: ".").last ?? "?")")
    }

    /// Convenience initializer with default Actrium service
    public convenience init() {
        self.init(
            serviceBundleIdentifier: "io.actrium.app.viewservice",
            viewControllerClassName: "ActriumServiceViewController"
        )
    }

    /// Convenience initializer with initial content size
    public convenience init(initialContentSize: CGSize) {
        self.init()
        self.currentContentSize = initialContentSize
        NSLog("ViewBridgeHostView: init with contentSize=%.1fx%.1f", initialContentSize.width, initialContentSize.height)
    }

    /// Convenience initializer with custom bundle identifier and content size
    /// Used by XPC service pool for multi-process isolation
    public convenience init(serviceBundleIdentifier: String, initialContentSize: CGSize) {
        self.init(
            serviceBundleIdentifier: serviceBundleIdentifier,
            viewControllerClassName: "ActriumServiceViewController"
        )
        self.currentContentSize = initialContentSize
        ipcLog("📐 ViewBridgeHostView: contentSize=\(Int(initialContentSize.width))x\(Int(initialContentSize.height))")
    }

    required init?(coder: NSCoder) {
        self.serviceBundleIdentifier = "io.actrium.app.viewservice"
        self.serviceViewControllerClassName = "ActriumServiceViewController"
        super.init(coder: coder)
        setupView()
    }

    // MARK: - Coordinate System

    // Note:ViewBridgeHostView Use criteria AppKit Coordinate System
    // ContainerView Yes. flipped ♪ Yeah, so ♪ hostView They'll be left in the top left corner.
    // But... hostView We're using standard coordinates, so... NSRemoteView ..to be correctly displayed

    // MARK: - Setup

    private func setupView() {
        wantsLayer = true
        layer?.masksToBounds = true  // Crop content beyond the boundary to avoid tailing
        // Disable layer Invisible Animations
        layer?.actions = ["bounds": NSNull(), "position": NSNull(), "frame": NSNull()]

        // Disable automatic subview resizing to allow manual control in updateRemoteViewLayout
        autoresizesSubviews = false

        // Register for live resize notifications to capture start size
        NotificationCenter.default.addObserver(forName: .windowLiveResizeStarted, object: nil, queue: .main) { [weak self] _ in
            guard let self = self else { return }
            // Capture size at start of drag
            self.resizeStartContentSize = self.bounds.size
            // Also ensure we set this immediately to prevent initial jump
            if let remoteView = self.remoteViewController?.view {
                // Force sync immediate state
                let current = remoteView.frame.size
                if current.width > 0 && current.height > 0 {
                     self.resizeStartContentSize = current
                }
            }
        }

        NotificationCenter.default.addObserver(forName: .windowLiveResizeEnded, object: nil, queue: .main) { [weak self] _ in
            self?.resizeStartContentSize = nil
            // Force a final layout update to restore full size
            self?.needsLayout = true
        }

        NSLog("ViewBridgeHostView: Initialized for service \(serviceBundleIdentifier)")
        ipcLog("🆕 ViewBridgeHostView init for service=\(serviceBundleIdentifier.components(separatedBy: ".").last ?? "?")")
    }

    // MARK: - Size Control

    /// Back intrinsicContentSize
    public override var intrinsicContentSize: NSSize {
        return currentContentSize
    }

    /// Make sure you're all set. remoteView Fill the whole thing. hostView
    public override func layout() {
        super.layout()
        // 🔑 Force Use bounds.size Sync remoteView, don't depend currentContentSize
        if let remoteView = remoteViewController?.view {
            let targetSize = bounds.size
            if targetSize.width > 0 && targetSize.height > 0 {
                let expectedFrame = NSRect(origin: .zero, size: targetSize)
                if remoteView.frame != expectedFrame {
                    // Disable hidden animation
                    CATransaction.begin()
                    CATransaction.setDisableActions(true)
                    remoteView.frame = expectedFrame
                    CATransaction.commit()
                }
            }
        }
    }

    /// Capture Direct Settings frame Situation
    /// 🔑 Key: Every time frame Change is now synchronized. remoteViewNo more stifling.
    /// Safari/Chrome Use process replicating without the problem, we have to synchronize as quickly as possible
    public override var frame: NSRect {
        didSet {
            // Any size changes are synchronized immediately. remoteView
            let remoteFrame = remoteViewController?.view.frame ?? .zero
            if abs(remoteFrame.width - frame.width) > 0.5 || abs(remoteFrame.height - frame.height) > 0.5 {
                // Disable hidden animation
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                remoteViewController?.view.frame = NSRect(origin: .zero, size: frame.size)
                CATransaction.commit()

                // Update simultaneously currentContentSizeMake sure next time. updateRemoteViewLayout Use the correct value
                currentContentSize = frame.size
            }
        }
    }

    /// Accept External Size Settings
    public override func setFrameSize(_ newSize: NSSize) {
        let oldSize = frame.size
        let remoteOldSize = remoteViewController?.view.frame.size ?? .zero

        // 🔑 Critical repairs: tested and rejected @2x Pixels Size
        // AppKit In certain cases (inwantsLayer=true) will be imported @2x Pixels Size
        // If you accept this size,hostView It'll take over. @2x Space, but remoteView Just... 1x
        let scaleFactor = window?.backingScaleFactor ?? 2.0  // Default Assumptions Retina

        // Detection: If newSize Obviously greater than currentContentSize(over) 1.5x) Maybe. @2x
        // Use superview.bounds As a reference (superview The size should be right. point Dimensions)
        // 🔑 Amendments: If superview.bounds Too small (e.g. not updated) to be used as a reference
        // ♪ Only if ♪ superview.bounds Looks like..."Reasonable."(No, it's not. 0 And it's not like that. currentContentSize It's too small for you to use.
        let referenceSize = superview?.bounds.size ?? currentContentSize
        let isReferenceValid = referenceSize.width > 10 && referenceSize.width >= currentContentSize.width * 0.8
        let effectiveReference = isReferenceValid ? referenceSize : currentContentSize

        let looksLikePixels = scaleFactor > 1.0 &&
            effectiveReference.width > 10 &&
            newSize.width > effectiveReference.width * 1.5 &&
            newSize.height > effectiveReference.height * 1.5

        if looksLikePixels {
            // Reject @2x Dimensions, use superview.bounds As the right size
            // If reference Not available, maintain currentContentSize or newSize / scale
            let fallbackSize: NSSize
            if isReferenceValid {
                fallbackSize = effectiveReference
            } else {
                fallbackSize = NSSize(width: newSize.width / scaleFactor, height: newSize.height / scaleFactor)
            }
            ipcLog("⚠️ setFrameSize: rejected @2x size \(Int(newSize.width))x\(Int(newSize.height)), using fallback \(Int(fallbackSize.width))x\(Int(fallbackSize.height))")

            CATransaction.begin()
            CATransaction.setDisableActions(true)
            super.setFrameSize(fallbackSize)
            CATransaction.commit()
        } else {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            super.setFrameSize(newSize)
            CATransaction.commit()
        }

        // Use bounds.size Update remoteView(bounds Yes. point Dimensions)
        updateRemoteViewLayoutWithBounds()

        // 🔍 Diagnosis log: Inspection setFrameSize Call(s)
        let remoteNewSize = remoteViewController?.view.frame.size ?? .zero
        if abs(newSize.width - oldSize.width) > 1 || abs(newSize.height - oldSize.height) > 1 {
            ipcLog("🔧 setFrameSize: \(Int(oldSize.width))x\(Int(oldSize.height)) → \(Int(newSize.width))x\(Int(newSize.height)), bounds=\(Int(bounds.width))x\(Int(bounds.height)), remote: \(Int(remoteOldSize.width))x\(Int(remoteOldSize.height)) → \(Int(remoteNewSize.width))x\(Int(remoteNewSize.height)), currentContentSize=\(Int(currentContentSize.width))x\(Int(currentContentSize.height))")
        }
    }

    /// Use bounds.size Update remoteView frame(used for setFrameSize Rewinding)
    private func updateRemoteViewLayoutWithBounds() {
        guard let remoteView = remoteViewController?.view else { return }

        // bounds.size Yes. point Dimensions (with frame.size Same, but more reliable)
        let targetSize = bounds.size
        guard targetSize.width > 0 && targetSize.height > 0 else { return }

        let expectedFrame = NSRect(origin: .zero, size: targetSize)
        if remoteView.frame != expectedFrame {
            ipcLog("🔧 updateRemoteViewLayoutWithBounds: resizing remoteView \(Int(remoteView.frame.width))x\(Int(remoteView.frame.height)) → \(Int(targetSize.width))x\(Int(targetSize.height))")

            // Disable hidden animation
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            remoteView.frame = expectedFrame
            CATransaction.commit()

            // Ensure currentContentSize Synchronise Update
            if currentContentSize != targetSize {
                currentContentSize = targetSize
            }
        }
    }

    /// Update remote view frame to fill bounds
    private func updateRemoteViewLayout() {
        guard let remoteView = remoteViewController?.view else { return }

        // 🔑 Critical repairs: use currentContentSize Not bounds.size
        // bounds.size Yes. Retina Could return on monitor @2x Pixels Size
        // currentContentSize By SwiftUI Correctly calculated point Dimensions
        let targetSize = currentContentSize

        // Ignore invalid dimensions
        guard targetSize.width > 0 && targetSize.height > 0 else {
            return
        }

        // Force from (0,0) Start, make sure it's full. hostView
        let expectedFrame = NSRect(origin: .zero, size: targetSize)
        if remoteView.frame != expectedFrame {
            // Disable hidden animation
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            remoteView.frame = expectedFrame
            CATransaction.commit()
        }
    }

    // MARK: - View Lifecycle

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()

        if window != nil && remoteViewController == nil && !isLoading {
            requestRemoteViewController()
        }
    }

    // MARK: - Remote View Controller Request

    /// Request the remote view controller from the ViewBridge service
    public func requestRemoteViewController() {
        guard !isLoading else {
            NSLog("ViewBridgeHostView: Already loading")
            return
        }

        isLoading = true

        NSLog("ViewBridgeHostView: Requesting \(serviceViewControllerClassName) from \(serviceBundleIdentifier)")

        // Live tabs use RendererProcessPool. Keep the legacy ViewBridge
        // request path for leftover snapshot helpers only.
        requestRemoteViewControllerDirect()
    }

    /// For capturing new connections internalId
    private var pendingConnectionInternalId: String?
    private var connectionCreatedObserverForDirect: NSObjectProtocol?

    /// Direct Request Remote ViewController(no use) prewarm)
    private func requestRemoteViewControllerDirect() {
        NSLog("ViewBridgeHostView: Requesting remote ViewController directly... (no prewarm)")

        // 🔑 Set up a listening device to capture the new connection internalId
        // Even if it's like this. prewarm It's already been created. entryWe can pass. bindTab Quite binding.
        setupConnectionCreatedListener()

        // Load ViewBridge framework
        let frameworkPath = "/System/Library/PrivateFrameworks/ViewBridge.framework/ViewBridge"
        guard dlopen(frameworkPath, RTLD_NOW) != nil else {
            handleError("Failed to load ViewBridge framework")
            return
        }

        // Get NSRemoteViewController class
        guard let remoteVCClass = NSClassFromString("NSRemoteViewController") else {
            handleError("NSRemoteViewController class not found")
            return
        }

        // Call requestViewController:fromServiceWithBundleIdentifier:connectionHandler:
        let selector = NSSelectorFromString("requestViewController:fromServiceWithBundleIdentifier:connectionHandler:")

        guard remoteVCClass.responds(to: selector) else {
            handleError("requestViewController method not found")
            return
        }

        // Create connection handler block
        let handler: @convention(block) (NSViewController?, Error?) -> Void = { [weak self] viewController, error in
            DispatchQueue.main.async {
                self?.handleConnectionResultWithBinding(viewController: viewController, error: error)
            }
        }

        // Invoke the class method
        let method = remoteVCClass.method(for: selector)
        typealias RequestFunc = @convention(c) (AnyClass, Selector, String, String, Any) -> Void
        let requestFunc = unsafeBitCast(method, to: RequestFunc.self)

        requestFunc(
            remoteVCClass,
            selector,
            serviceViewControllerClassName,
            serviceBundleIdentifier,
            handler
        )
    }

    /// Set up the listening to capture the new connection internalId
    private func setupConnectionCreatedListener() {
        // Remove old listening devices
        if let observer = connectionCreatedObserverForDirect {
            DistributedNotificationCenter.default().removeObserver(observer)
        }

        pendingConnectionInternalId = nil

        connectionCreatedObserverForDirect = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.xpcConnectionCreated"),
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self = self,
                  let userInfo = notification.userInfo,
                  let bundleId = userInfo["bundleId"] as? String,
                  let internalId = userInfo["internalId"] as? String else { return }

            // Filter: Only this HostView Corresponding service
            guard bundleId == self.serviceBundleIdentifier else { return }

            // Filter: Only notifications that belong to this process
            if let hostPid = userInfo["hostProcessId"] as? Int32,
               hostPid != ActriumHostProcessId {
                return
            }

            NSLog("ViewBridgeHostView: 🔗 Captured internalId=%@ for direct connection", String(internalId.prefix(8)))
            self.pendingConnectionInternalId = internalId

            // Remove the listening device (only once caught)
            if let observer = self.connectionCreatedObserverForDirect {
                DistributedNotificationCenter.default().removeObserver(observer)
                self.connectionCreatedObserverForDirect = nil
            }
        }
    }

    /// Process new connection results and send them bindTab Announcements
    private func handleConnectionResultWithBinding(viewController: NSViewController?, error: Error?) {
        // Wait a little while. xpcConnectionCreated Notifications of arrival
        // Use RunLoop Ensure that notifications are processed
        let startTime = Date()
        while pendingConnectionInternalId == nil && Date().timeIntervalSince(startTime) < 0.5 {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
        }

        // Send bindTab Notification (if any) internalId)
        if let internalId = pendingConnectionInternalId, let tabId = coordinator?.tabId {
            let bindName = "com.actro.bindTab.\(ActriumHostProcessId).\(serviceBundleIdentifier)"
            DistributedNotificationCenter.default().postNotificationName(
                NSNotification.Name(bindName),
                object: nil,
                userInfo: [
                    RuntimeBridgeContract.tabId: tabId.uuidString,
                    "internalId": internalId,
                    "hostProcessId": ActriumHostProcessId
                ],
                deliverImmediately: true
            )
            NSLog("ViewBridgeHostView: 📤 Sent bindTab (direct): tabId=%@, internalId=%@",
                  String(tabId.uuidString.prefix(8)), String(internalId.prefix(8)))
        } else {
            NSLog("⚠️ ViewBridgeHostView: No internalId captured for direct connection, relying on FIFO")
        }

        // Cleaning
        pendingConnectionInternalId = nil
        if let observer = connectionCreatedObserverForDirect {
            DistributedNotificationCenter.default().removeObserver(observer)
            connectionCreatedObserverForDirect = nil
        }

        // Keep working on it.
        handleConnectionResult(viewController: viewController, error: error)
    }

    // MARK: - Connection Handling

    private func handleConnectionResult(viewController: NSViewController?, error: Error?) {
        isLoading = false

        if let error = error {
            handleError("Connection failed: \(error.localizedDescription)")
            return
        }

        guard let vc = viewController else {
            handleError("No view controller returned")
            return
        }

        NSLog("ViewBridgeHostView: Received remote view controller: \(type(of: vc))")

        // Embed the remote view controller's view
        remoteViewController = vc

        let remoteView = vc.view

        // Use autoresizingMask Not Auto Layout
        // Auto Layout with flipped The coordinates are probably having an interactive problem.
        let targetFrame = NSRect(origin: .zero, size: currentContentSize)
        remoteView.frame = targetFrame
        // Disable autoresizing mask as we manage layout manually
        remoteView.autoresizingMask = []

        // 🔑 Ensure remoteView It's... layer Coordinates Match flipped Parent View
        remoteView.wantsLayer = true
        remoteView.layer?.isGeometryFlipped = true
        // Disable remoteView layer A hidden animation
        remoteView.layer?.actions = ["bounds": NSNull(), "position": NSNull(), "frame": NSNull()]

        addSubview(remoteView)

        // Configure ViewBridge content size range
        let maxSize = CGSize(width: 4096, height: 4096)
        let minSize = CGSize(width: 50, height: 50)
        let updateSelector = NSSelectorFromString("updateContentMinSize:maxSize:")

        // Try to set size range on VC
        if vc.responds(to: updateSelector) {
            let method = (vc as AnyObject).method(for: updateSelector)
            typealias UpdateSizeFunc = @convention(c) (AnyObject, Selector, CGSize, CGSize) -> Void
            let updateFunc = unsafeBitCast(method, to: UpdateSizeFunc.self)
            updateFunc(vc as AnyObject, updateSelector, minSize, maxSize)
        }

        // Try to set size range on View
        if remoteView.responds(to: updateSelector) {
            let method = remoteView.method(for: updateSelector)
            typealias UpdateSizeFunc = @convention(c) (AnyObject, Selector, CGSize, CGSize) -> Void
            let updateFunc = unsafeBitCast(method, to: UpdateSizeFunc.self)
            updateFunc(remoteView, updateSelector, minSize, maxSize)
        }

        // Try to set on VC.remoteView property
        let remoteViewSelector = NSSelectorFromString("remoteView")
        if vc.responds(to: remoteViewSelector),
           let actualRemoteView = vc.perform(remoteViewSelector)?.takeUnretainedValue() as? NSView,
           actualRemoteView.responds(to: updateSelector) {
            let method = actualRemoteView.method(for: updateSelector)
            typealias UpdateSizeFunc = @convention(c) (AnyObject, Selector, CGSize, CGSize) -> Void
            let updateFunc = unsafeBitCast(method, to: UpdateSizeFunc.self)
            updateFunc(actualRemoteView, updateSelector, minSize, maxSize)
        }

        // Ensure ViewBridgeHostView I'm using the right size.
        super.setFrameSize(currentContentSize)

        NSLog("ViewBridgeHostView: Remote view embedded, size=%.0fx%.0f", currentContentSize.width, currentContentSize.height)

        // Notifier connection established - coordinator It'll handle the re-issuance. pendingTabId
        onConnectionEstablished?()
    }

    // MARK: - Content Size Management

    /// Update Local frame and preferredContentSize
    /// By coordinator.applyViewportSize Call, do not send notification (notice by coordinator (Umnibus processing)
    func applyFrameSize(_ newSize: CGSize) {
        currentContentSize = newSize

        // 🔑 Critical repairs: synchronized updates hostView and remoteView It's... frame
        // Updates before only remoteView, resulting in remoteView Over hostView Borders are cut
        // Now update it at the same time. hostView It's... frameEnsuring consistency between the two
        let newFrame = NSRect(origin: frame.origin, size: newSize)
        if frame.size != newSize {
            frame = newFrame
        }

        // Update remoteView frame
        updateRemoteViewLayout()

        let isLiveResizing = WindowResizeState.shared.isLiveResizing

        // Sync to NSRemoteViewController"Always update, let Service I know the target size.
        if let vc = remoteViewController {
            let current = vc.preferredContentSize
            if abs(current.width - newSize.width) > 0.5 || abs(current.height - newSize.height) > 0.5 {
                vc.preferredContentSize = newSize
            }
        }

        // Debug Log - Organisation origin Information to diagnose location problems
        let remoteFrame = remoteViewController?.view.frame ?? .zero
        let remoteBounds = remoteViewController?.view.bounds ?? .zero
        let selfFrame = frame
        let selfBounds = bounds
        let superFrame = superview?.frame ?? .zero
        let superBounds = superview?.bounds ?? .zero
        let windowFrame = window?.frame ?? .zero
        let convertedOrigin = superview?.convert(frame.origin, to: nil) ?? .zero  // Convert to Window Coordinates

        // 🔍 Inspection layer Status
        let remoteLayer = remoteViewController?.view.layer
        let layerBounds = remoteLayer?.bounds ?? .zero
        let layerFrame = remoteLayer?.frame ?? .zero
        let layerTransform = remoteLayer?.transform ?? CATransform3DIdentity
        let isIdentityTransform = CATransform3DIsIdentity(layerTransform)

        ipcLog("📐 applyFrameSize: newSize=\(Int(newSize.width))x\(Int(newSize.height)) self.frame=(\(Int(selfFrame.origin.x)),\(Int(selfFrame.origin.y)),\(Int(selfFrame.width))x\(Int(selfFrame.height))) self.bounds=\(Int(selfBounds.width))x\(Int(selfBounds.height)) remoteFrame=(\(Int(remoteFrame.origin.x)),\(Int(remoteFrame.origin.y)),\(Int(remoteFrame.width))x\(Int(remoteFrame.height))) remoteBounds=\(Int(remoteBounds.width))x\(Int(remoteBounds.height)) layer.frame=\(Int(layerFrame.width))x\(Int(layerFrame.height)) layer.bounds=\(Int(layerBounds.width))x\(Int(layerBounds.height)) identityTransform=\(isIdentityTransform)")

        // 🔑 Force Refresh remoteViewMake sure. NSRemoteView Render
        // It's in the window. zoom It's especially important after animation.
        forceRemoteViewRefresh()

        // 🔍 Delayed inspection: See if retrofitted
        let expectedSize = newSize
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            let actualSelfFrame = self.frame
            let actualRemoteFrame = self.remoteViewController?.view.frame ?? .zero
            if abs(actualSelfFrame.width - expectedSize.width) > 1 ||
               abs(actualSelfFrame.height - expectedSize.height) > 1 ||
               abs(actualRemoteFrame.width - expectedSize.width) > 1 ||
               abs(actualRemoteFrame.height - expectedSize.height) > 1 {
                ipcLog("⚠️ POST-applyFrameSize: SIZE CHANGED! expected=\(Int(expectedSize.width))x\(Int(expectedSize.height)) actual.self=\(Int(actualSelfFrame.width))x\(Int(actualSelfFrame.height)) actual.remote=\(Int(actualRemoteFrame.width))x\(Int(actualRemoteFrame.height))")
            }
        }
    }

    private func handleError(_ message: String) {
        NSLog("ViewBridgeHostView: Error - \(message)")
        isLoading = false
    }

    // MARK: - Cleanup

    public func disconnect() {
        if let vc = remoteViewController {
            // Try to call disconnect on the remote VC
            let selector = NSSelectorFromString("disconnect")
            if vc.responds(to: selector) {
                vc.perform(selector)
            }
            vc.view.removeFromSuperview()
            remoteViewController = nil
            NSLog("ViewBridgeHostView: Disconnected from remote service")
        }
    }

    deinit {
        // Only disconnect if the coordinator still points to us
        // When SwiftUI replaces the view, coordinator.hostView will point to the new view
        if coordinator == nil || coordinator?.hostView === self {
            disconnect()
        } else {
            NSLog("ViewBridgeHostView: deinit skipped disconnect (replaced by new view)")
        }
    }

    // MARK: - Diagnostics

    /// Force Refresh remoteView Show
    /// Use a variety of strategies to ensure that NSRemoteView Render its contents
    func forceRemoteViewRefresh() {
        guard let remoteView = remoteViewController?.view else { return }

        let targetSize = bounds.size
        let remoteFrame = remoteView.frame
        ipcLog("🔄 forceRemoteViewRefresh: hostBounds=\(Int(targetSize.width))x\(Int(targetSize.height)) remoteFrame=\(Int(remoteFrame.width))x\(Int(remoteFrame.height))")

        // 🔑 Policy 0: "Frame Jitter" - Set different sizes before setting the right size
        // It's mandatory. NSRemoteView Resynchronise its content and break the possible cache
        let jitterSize = CGSize(width: targetSize.width - 1, height: targetSize.height - 1)
        remoteView.frame = NSRect(origin: .zero, size: jitterSize)
        remoteView.frame = NSRect(origin: .zero, size: targetSize)
        ipcLog("🔧 forceRemoteViewRefresh: jitter applied, final frame=\(Int(targetSize.width))x\(Int(targetSize.height))")

        // Policy 1: Tags view Re-show and Layout Required
        remoteView.needsDisplay = true
        remoteView.needsLayout = true

        // Policy 2: Refreshing at the same time layer
        if let layer = remoteView.layer {
            layer.setNeedsDisplay()
            layer.setNeedsLayout()
        }

        // Policy 3: Through the spectrospecies, force them to refresh.
        for subview in remoteView.subviews {
            subview.needsDisplay = true
            subview.needsLayout = true
            subview.layer?.setNeedsDisplay()
        }

        // Policy 4: Trigger window Redraw
        remoteView.window?.displayIfNeeded()

        // Policy 5: Delay in re-upholstering (after ensuring the animation is completed)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
            guard let self = self else { return }
            guard let remoteView = self.remoteViewController?.view else { return }
            let finalSize = self.bounds.size
            // Again. jitter
            let jitter = CGSize(width: finalSize.width - 1, height: finalSize.height - 1)
            remoteView.frame = NSRect(origin: .zero, size: jitter)
            remoteView.frame = NSRect(origin: .zero, size: finalSize)
            ipcLog("🔧 forceRemoteViewRefresh (delayed): jitter applied, final frame=\(Int(finalSize.width))x\(Int(finalSize.height))")
            remoteView.needsDisplay = true
            remoteView.needsLayout = true
            remoteView.layer?.setNeedsDisplay()
            remoteView.window?.displayIfNeeded()
        }
    }

    /// Print View Level frame Status (for debugging) resize (Issues)
    func logViewHierarchy(prefix: String) {
        let containerFrame = superview?.frame ?? .zero
        let hostFrame = frame
        let remoteFrame = remoteViewController?.view.frame ?? .zero
        let remoteBounds = remoteViewController?.view.bounds ?? .zero
        // Inspection remoteView Subview
        var subviewInfo = ""
        if let remoteView = remoteViewController?.view {
            for (i, subview) in remoteView.subviews.enumerated() {
                subviewInfo += " sub[\(i)]=(\(Int(subview.frame.origin.x)),\(Int(subview.frame.origin.y)),\(Int(subview.frame.width))x\(Int(subview.frame.height)))"
            }
        }
        ipcLog("🔍 \(prefix): container=(\(Int(containerFrame.origin.x)),\(Int(containerFrame.origin.y)),\(Int(containerFrame.width))x\(Int(containerFrame.height))) host=(\(Int(hostFrame.origin.x)),\(Int(hostFrame.origin.y)),\(Int(hostFrame.width))x\(Int(hostFrame.height))) remote=(\(Int(remoteFrame.origin.x)),\(Int(remoteFrame.origin.y)),\(Int(remoteFrame.width))x\(Int(remoteFrame.height))) remoteBounds=\(Int(remoteBounds.width))x\(Int(remoteBounds.height))\(subviewInfo)")
    }

    // MARK: - Snapshot

    /// Capture a snapshot of the current view content
    public func captureSnapshot() -> NSImage? {
        let size = currentContentSize
        guard size.width > 0 && size.height > 0 else { return nil }

        // Use Priority NSRemoteView Private snapshot API(More reliable capture of cross-process elements)
        if let remoteView = remoteViewController?.view as? NSObject {
            let snapshotSelector = NSSelectorFromString("snapshot")
            if remoteView.responds(to: snapshotSelector),
               let result = remoteView.perform(snapshotSelector)?.takeUnretainedValue() {
                let cgImage = result as! CGImage
                let image = NSImage(cgImage: cgImage, size: size)
                NSLog("ViewBridgeHostView: Captured NSRemoteView snapshot \(size)")
                return image
            }
        }

        // Back: Rendering layer"Sometimes I can't get it. remote content)
        guard let layer = layer else { return nil }
        let image = NSImage(size: size)
        image.lockFocus()
        if let context = NSGraphicsContext.current?.cgContext {
            layer.render(in: context)
        }
        image.unlockFocus()

        NSLog("ViewBridgeHostView: Captured layer snapshot \(size)")
        return image
    }
}

// MARK: - ViewBridge Availability Check

extension ViewBridgeHostView {

    /// Check if ViewBridge is available on this system
    public static var isAvailable: Bool {
        let frameworkPath = "/System/Library/PrivateFrameworks/ViewBridge.framework/ViewBridge"
        guard dlopen(frameworkPath, RTLD_NOW) != nil else {
            return false
        }
        return NSClassFromString("NSRemoteViewController") != nil
    }
}

// MARK: - SwiftUI Wrapper

// MARK: - Live Resize State

/// Global Window resize Status - Insisting process while dragging IPC Announcements
final class WindowResizeState: ObservableObject {
    static let shared = WindowResizeState()
    private init() {}

    /// Is this going on? live resize(Drawing window edge)
    @Published var isLiveResizing: Bool = false

    /// Current resize It's... tab ID(For matching mask display)- Focus Mode
    @Published var activeTabId: UUID?

    /// Whether or not to wait Service Port sizeApplied Announcements
    /// true = The photo mask is also available.false = Hide Mask
    @Published var isWaitingForSizeApplied: Bool = false
}

extension Notification.Name {
    static let windowLiveResizeStarted = Notification.Name("windowLiveResizeStarted")
    static let windowLiveResizeEnded = Notification.Name("windowLiveResizeEnded")
}

/// Coordinator that holds reference to ViewBridgeHostView
/// Management XPC Connect status, life cycle by ViewBridgeCoordinatorManager Management
/// Focus on all size sync logic - This is... Host End size update only entry
final class ViewBridgeCoordinator {
    weak var hostView: ViewBridgeHostView?
    /// Browser-style Renderer host. During migration this coexists with the old
    /// ViewBridge reference, but a Tab owns at most one active rendering path.
    weak var rendererHostView: RendererSurfaceHostView?
    /// Callback when XPC connection is established
    var onConnectionEstablished: (() -> Void)?

    // XPC Status - Move here to avoid SwiftUI @State Resetd
    // 🔑 Send the initials when all four conditions are met config:
    //    1. allocatedServiceId != nil(distributed) XPC Service)
    //    2. xpcServiceReady == true(Service Already ready)
    //    3. hasConnection == true(ViewBridge Connection established)
    //    4. currentViewportSize Effective (%)> 10x10)

    /// Distribution XPC Service ID
    /// Use didSet Auto try to send the initial after the distribution config
    var allocatedServiceId: String? {
        didSet {
            if allocatedServiceId != nil && oldValue == nil {
                let tabIdShort = tabId?.uuidString.prefix(8) ?? "nil"
                let serviceShort = allocatedServiceId?.components(separatedBy: ".").last ?? "nil"
                ipcLog("🎯 allocatedServiceId=\(serviceShort) for tab=\(tabIdShort)")
                trySendInitialConfig()
            }
        }
    }

    /// XPC Service Did you? ready(Received) xpcServiceReady (Notification)
    /// Use didSet Auto-try to send the initials when status changes config
    var xpcServiceReady: Bool = false {
        didSet {
            if xpcServiceReady && !oldValue {
                let tabIdShort = tabId?.uuidString.prefix(8) ?? "nil"
                ipcLog("📡 xpcServiceReady=true for tab=\(tabIdShort)")
                trySendInitialConfig()
            }
        }
    }

    /// ViewBridge Connections Created
    /// Use didSet Auto-try to send the initials when status changes config
    var hasConnection: Bool = false {
        didSet {
            if hasConnection && !oldValue {
                let tabIdShort = tabId?.uuidString.prefix(8) ?? "nil"
                ipcLog("🔗 hasConnection=true for tab=\(tabIdShort)")

                // 🔑 Send as soon as connection is established bootstrap tabConfig(Not with PID)
                // This notice is Jean. Service Access hostProcessId , and set the tape PID The listening device.
                // Organisation tabIdReplace the one before. pendingTabId Mechanisms
                sendBootstrapTabConfig()

                trySendInitialConfig()
            }
        }
    }

    /// Send bootstrap tabConfig(Not with hostProcessId Prefix)
    /// Used in ViewBridge Notification as soon as connection is established Service:
    /// 1. hostProcessId - Jean. Service Can set the tape PID The listening device.
    /// 2. tabId - Jean. Service Tie pending entry
    /// 3. Basic Configuration - Jean. Service You can start replicating.
    private func sendBootstrapTabConfig() {
        // Renderer IPC has an explicit request/response handshake and does not
        // need the legacy DistributedNotification bootstrap.
        if rendererHostView != nil {
            return
        }
        guard let tabId = tabId,
              let serviceId = allocatedServiceId else {
            ipcLog("⚠️ sendBootstrapTabConfig: missing tabId or allocatedServiceId")
            return
        }

        let size = currentViewportSize
        let tabIdShort = String(tabId.uuidString.prefix(8))

        // Build bootstrap config(includes hostProcessId)
        var config: [String: Any] = [
            RuntimeBridgeContract.tabId: tabId.uuidString,
            "hostProcessId": ActriumHostProcessId,
            RuntimeBridgeContract.containerWidth: size.width,
            RuntimeBridgeContract.containerHeight: size.height,
            "isOverviewMode": isOverviewMode,
            "isBootstrap": true  // Mark this. bootstrap config
        ]

        // Use coordinator Complete storage tab Configure
        let unifiedScriptPreview = tabConfig?.unifiedScript.map { String($0.prefix(50)) + "..." } ?? "nil"
        ipcLog("📋 sendBootstrapTabConfig: tabConfig=\(tabConfig != nil ? "SET" : "NIL"), systemAppType=\(tabConfig?.systemAppType ?? "nil"), unifiedScript=\(unifiedScriptPreview), isOverviewMode=\(isOverviewMode), bookmarks=\(tabConfig?.bookmarks.count ?? 0), history=\(tabConfig?.recentHistory.count ?? 0)")

        // 🔑 If tabConfig Not set yet. Mark it as to be sent. Wait. tabConfig Set and then send
        guard let tabConfig = self.tabConfig else {
            pendingBootstrap = true
            ipcLog("⏳ sendBootstrapTabConfig: tabConfig not set, marking pending")
            return
        }

        if true {  // keep scope for tabConfig
            config[RuntimeBridgeContract.title] = tabConfig.title
            config["backgroundColor"] = tabConfig.backgroundColor
            if let systemAppType = tabConfig.systemAppType {
                config[RuntimeBridgeContract.systemAppType] = systemAppType
            }
            if let script = tabConfig.unifiedScript { config["unifiedScript"] = script }
            if let script = tabConfig.leftPanelScript { config["leftPanelScript"] = script }
            config["leftPanelVisible"] = tabConfig.leftPanelVisible
            config["rightPanelVisible"] = tabConfig.rightPanelVisible
            config["permissions"] = tabConfig.permissions
            config["reloadKey"] = tabConfig.reloadKey
            // 🔑 StartPage Data required
            if !tabConfig.bookmarks.isEmpty { config["bookmarks"] = tabConfig.bookmarks }
            if !tabConfig.recentHistory.isEmpty { config["recentHistory"] = tabConfig.recentHistory }
        }

        guard let jsonData = try? JSONSerialization.data(withJSONObject: config),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            ipcLog("⚠️ sendBootstrapTabConfig: JSON serialization failed")
            return
        }

        // 🔑 Use without PID * The name of the notification, ensuring that Service We're on it.
        let notificationName = "com.actro.tabConfig.\(serviceId)"
        DistributedNotificationCenter.default().postNotificationName(
            NSNotification.Name(notificationName),
            object: nil,
            userInfo: ["config": jsonString],
            deliverImmediately: true
        )

        ipcLog("📤 Sent bootstrap tabConfig: tab=\(tabIdShort) size=\(Int(size.width))x\(Int(size.height)) notificationName=\(notificationName)")
    }
    var xpcReadyObserver: NSObjectProtocol?
    var connectionCreatedObserver: NSObjectProtocol?
    var liveResizeEndedObserver: NSObjectProtocol?
    /// Observer for Service's "listener ready" notification (to resolve race condition)
    var listenerReadyObserver: NSObjectProtocol?
    var tabId: UUID?

    /// Tab Configure Data - For Sending bootstrap tabConfig
    /// By ViewBridgeMultiProcessTabView Settings on Initialization
    var tabConfig: XPCTabConfig? {
        didSet {
            // 🔑 If bootstrap I was missing it. tabConfig And then I failed. Now try again.
            if tabConfig != nil && pendingBootstrap && hasConnection {
                pendingBootstrap = false
                ipcLog("📤 Retrying bootstrap after tabConfig set")
                sendBootstrapTabConfig()
            }
        }
    }

    /// Is the tag yet to be sent bootstrap♪ Because ♪ tabConfig I'm not set yet and I'm delayed.
    private var pendingBootstrap = false

    /// Whether the tab is in Overview mode (for compact view in system apps)
    /// Default to false since BrowserViewModel.init sets actriumMode = .focus
    var isOverviewMode: Bool = false

    /// Current viewport Dimensions - Store in coordinator Center to access the latest value in closed package
    /// Default value 50x50 Reflect Overview Possible small card sizes in mode
    /// Use didSet Try to send the initial when the size becomes effective for the first time config
    var currentViewportSize: CGSize = CGSize(width: 50, height: 50) {
        didSet {
            // Only when the dimensions become invalid and effective (avoid frequent triggers)
            let wasValid = oldValue.width > 10 && oldValue.height > 10
            let isValid = currentViewportSize.width > 10 && currentViewportSize.height > 10
            if isValid && !wasValid {
                let tabIdShort = tabId?.uuidString.prefix(8) ?? "nil"
                ipcLog("📐 viewportSize valid for tab=\(tabIdShort): \(Int(currentViewportSize.width))x\(Int(currentViewportSize.height))")
                trySendInitialConfig()
            }
        }
    }
    /// Last sent tabConfig Size of time - To prevent duplicates
    var lastSentConfigSize: CGSize?
    /// Last sent tabConfig It's... reloadKey - To prevent duplicates
    var lastSentReloadKey: Int?
    /// Last sent containerSizeChanged Size of time - To prevent duplicates
    private var lastSentContainerSize: CGSize?

    /// Reset lastSentContainerSize to force re-send notification
    /// Called when isOverviewMode changes to ensure Service receives the update
    func resetLastSentContainerSize() {
        lastSentContainerSize = nil
    }

    /// 🔑 Try to send initial tab config
    /// Check four conditions:allocatedServiceId, xpcServiceReady, hasConnection, viewportSize
    /// Any one of them. didSet This method is called when triggered and sent when all conditions are met config
    func trySendInitialConfig() {
        let tabIdShort = tabId?.uuidString.prefix(8) ?? "nil"
        let size = currentViewportSize
        let sizeValid = size.width > 10 && size.height > 10

        // Print Current Status
        ipcLog("🔍 trySendInitialConfig tab=\(tabIdShort): serviceId=\(allocatedServiceId != nil ? "✓" : "✗"), xpcReady=\(xpcServiceReady ? "✓" : "✗"), conn=\(hasConnection ? "✓" : "✗"), size=\(sizeValid ? "✓" : "✗")(\(Int(size.width))x\(Int(size.height)))")

        // Check four conditions.
        guard allocatedServiceId != nil else { return }
        guard hasConnection else { return }
        guard xpcServiceReady else { return }
        guard sizeValid else { return }

        // Prevent duplicates
        guard size != lastSentContainerSize else {
            ipcLog("ℹ️ trySendInitialConfig tab=\(tabIdShort): already sent, skipping")
            return
        }

        ipcLog("🚀 trySendInitialConfig: ALL CONDITIONS MET for tab=\(tabIdShort), size=\(Int(size.width))x\(Int(size.height))")
        lastSentContainerSize = size
        sendContainerSizeNotification(size: size)
    }

    /// Fluid: Last update
    private var lastUpdateTime: Date = .distantPast
    /// Fluorine: up-to-date size to be processed
    private var pendingSize: CGSize?
    /// Fluid: Timer
    private var throttleTimer: Timer?
    /// Spacing - Limit frequency of transmission (approximately) 40fps)
    private static let throttleInterval: TimeInterval = 0.025  // 25ms

    // MARK: - Focused Size Update Entry

    /// Live resize Start the listening.
    private var liveResizeStartedObserver: NSObjectProtocol?

    /// Settings live resize Listener (set once only)
    private func setupLiveResizeObserverIfNeeded() {
        // Settings resize Start the listening.
        if liveResizeStartedObserver == nil {
            liveResizeStartedObserver = NotificationCenter.default.addObserver(
                forName: .windowLiveResizeStarted,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let self = self,
                      let hostView = self.hostView,
                      let tabId = self.tabId,
                      hostView.window != nil else { return }  // Only visible. hostView I'm not gonna take a snapshot until I get it.

                // Yes. resize Started off with a snapshot. TabSnapshotCacheManager
                // Sniper storage (based on current visual size, usually large view) for re-use mode switching = Slot A)
                if let snapshot = hostView.captureSnapshot() {
                    TabSnapshotCacheManager.shared.setFocusSnapshot(snapshot, for: tabId)
                    WindowResizeState.shared.activeTabId = tabId
                }
            }
        }

        // Settings resize End the wiretaps.
        // Not just read the size!SwiftUI layout Maybe it's not finished yet.
        // It's a start. debounce Mechanism, wait for size to stabilize before sending
        if liveResizeEndedObserver == nil {
            liveResizeEndedObserver = NotificationCenter.default.addObserver(
                forName: .windowLiveResizeEnded,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                guard let self = self else { return }

                ipcLog("🔔 windowLiveResizeEnded received, tab=\(self.tabId?.uuidString.prefix(8) ?? "nil")")

                // 🔍 Diagnosis: delay 500ms Post-print full view hierarchy state
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                    guard let self = self, let hostView = self.hostView else { return }
                    hostView.logViewHierarchy(prefix: "DIAG 500ms")
                }

                // Only resize After completion, wait state, avoid live resize It's stuck in the old snapshot.
                if let tabId = self.tabId {
                    WindowResizeState.shared.activeTabId = tabId
                    WindowResizeState.shared.isWaitingForSizeApplied = true
                }

                // Mark Entry debounce Status
                self.isWaitingForFinalSize = true

                // Start NSView frame Change listening device
                // This can catch. SwiftUI GeometryReader Could be missing. frame Update
                self.setupFrameChangeObserverIfNeeded()

                // Start debounce Timer
                self.scheduleFinalSizeDebounce()
            }
        }

        // Settings sizeApplied Listener (Voice)Service ♪ Clean up the snapshot after the size update is completed ♪
        setupSizeAppliedListenerIfNeeded()
    }

    /// sizeApplied Listener (Voice)Service (Notification upon completion of the re-sizing)
    private var sizeAppliedObserver: NSObjectProtocol?

    /// hostView frame Change listening (capture) SwiftUI GeometryReader (Possible omissions)
    private var frameDidChangeObserver: NSObjectProtocol?

    /// Is it in or out? resize After that. debounce Period
    private var isWaitingForFinalSize = false
    /// Final Size debounce Timer
    private var finalSizeDebounceTimer: Timer?
    /// Debounce Time (waiting for size to stabilize)
    private static let finalSizeDebounceInterval: TimeInterval = 0.10  // 100ms - Reduce waiting time

    /// Last sent size time (for testing for re-issuance)
    private var lastSizeNotificationTime: Date = .distantPast
    /// Re-sale window period (received new size changes within this time period, need to be re-send)
    private static let resendWindowInterval: TimeInterval = 1.0  // 1 Change in seconds needs to be re-decorated.

    /// Settings hostView frame Change listening device
    /// Yes. debounce Any catch during the period frame Change and Reset debounce Timer
    private func setupFrameChangeObserverIfNeeded() {
        // Remove old listening devices
        if let observer = frameDidChangeObserver {
            NotificationCenter.default.removeObserver(observer)
            frameDidChangeObserver = nil
        }

        guard let hostView = hostView else { return }

        // Enable frame Notification of changes
        hostView.postsFrameChangedNotifications = true

        frameDidChangeObserver = NotificationCenter.default.addObserver(
            forName: NSView.frameDidChangeNotification,
            object: hostView,
            queue: .main
        ) { [weak self] notification in
            guard let self = self,
                  self.isWaitingForFinalSize,
                  let view = notification.object as? NSView else { return }

            // 🔑 Rehabilitation @2x Question:bounds Possible return pixel size to need conversion to point Dimensions
            let rawBounds = view.bounds.size
            let scaleFactor = view.window?.backingScaleFactor ?? 1.0

            // Check if it's a yes or no. @2x: If different from the current size 2 Multiply, conversion is required
            let newSize: CGSize
            if scaleFactor > 1.0 && rawBounds.width > self.currentViewportSize.width * 1.8 {
                // bounds It's pixel size. point Dimensions
                newSize = CGSize(width: rawBounds.width / scaleFactor, height: rawBounds.height / scaleFactor)
            } else {
                newSize = rawBounds
            }

            // Ignore invalid dimensions
            guard newSize.width > 50 && newSize.height > 50 else { return }

            // Frame Changed, reset. debounce Timer
            self.currentViewportSize = newSize
            self.scheduleFinalSizeDebounce()
        }
    }

    /// Remove frame Change listening device
    private func removeFrameChangeObserver() {
        if let observer = frameDidChangeObserver {
            NotificationCenter.default.removeObserver(observer)
            frameDidChangeObserver = nil
        }
    }

    /// Settings sizeApplied Listener (incident-driven cleanup snapshot)
    private func setupSizeAppliedListenerIfNeeded() {
        guard sizeAppliedObserver == nil,
              let serviceId = allocatedServiceId else { return }

        let notificationName = "com.actro.sizeApplied.\(ActriumHostProcessId).\(serviceId)"
        sizeAppliedObserver = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name(notificationName),
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self = self,
                  let userInfo = notification.userInfo,
                  let notifyTabId = userInfo[RuntimeBridgeContract.tabId] as? String,
                  notifyTabId == self.tabId?.uuidString else { return }

            // Extract requestId for log correlation
            let requestId = userInfo["requestId"] as? String ?? "?"
            let width = userInfo["width"] as? Double ?? 0
            let height = userInfo["height"] as? Double ?? 0
            NSLog("📐 [HOST←SVC] sizeApplied req=%@ size=%.0fx%.0f", requestId, width, height)

            // Cancel Timeout (if any)
            self.snapshotTimeoutTimer?.invalidate()
            self.snapshotTimeoutTimer = nil

            // Service We've finished the size update. Close the snapshot mask.
            if WindowResizeState.shared.isWaitingForSizeApplied {
                WindowResizeState.shared.isWaitingForSizeApplied = false
            }
        }
    }

    /// Scattered timer (supplied)
    private var snapshotTimeoutTimer: Timer?

    /// Start the snapshot timer (suspend, prevent Service (Notification lost)
    private func scheduleSnapshotTimeout() {
        snapshotTimeoutTimer?.invalidate()
        snapshotTimeoutTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: false) { _ in
            if WindowResizeState.shared.isWaitingForSizeApplied {
                WindowResizeState.shared.isWaitingForSizeApplied = false
            }
        }
    }

    /// Start final dimension debounce Timer
    /// Yes. debounce The timer will be reset if new dimensions change during the period
    /// When the timer triggers (stable size), send the final dimension to Service
    private func scheduleFinalSizeDebounce() {
        finalSizeDebounceTimer?.invalidate()
        ipcLog("⏱️ scheduleFinalSizeDebounce: starting \(Int(Self.finalSizeDebounceInterval * 1000))ms debounce timer")
        finalSizeDebounceTimer = Timer.scheduledTimer(withTimeInterval: Self.finalSizeDebounceInterval, repeats: false) { [weak self] _ in
            guard let self = self else { return }

            self.isWaitingForFinalSize = false
            ipcLog("⏱️ scheduleFinalSizeDebounce: timer fired")

            // 🔑 Critical repairs: use hostView Actual frame As final dimensions
            // Used before currentViewportSizeBut it could be with hostView.frame Inconsistencies
            // (SwiftUI Maybe it's finally changed. hostView.frame But no update currentViewportSize)
            let hostViewSize = self.hostView?.frame.size ?? self.currentViewportSize
            let finalSize = CGSize(
                width: floor(hostViewSize.width),
                height: floor(hostViewSize.height)
            )

            ipcLog("⏱️ scheduleFinalSizeDebounce: hostViewSize=\(Int(hostViewSize.width))x\(Int(hostViewSize.height)), currentViewportSize=\(Int(self.currentViewportSize.width))x\(Int(self.currentViewportSize.height)), finalSize=\(Int(finalSize.width))x\(Int(finalSize.height)), hasServiceId=\(self.allocatedServiceId != nil), hasConn=\(self.hasConnection), xpcReady=\(self.xpcServiceReady)")

            // Verify and Send
            if finalSize.width > 50 && finalSize.height > 50 {
                // Synchronize Update coordinator Status
                self.currentViewportSize = finalSize

                // Update Locally frame
                self.hostView?.applyFrameSize(finalSize)

                // 🔑 Check that all four conditions are met before you can send a notice.
                guard self.allocatedServiceId != nil,
                      self.hasConnection,
                      self.xpcServiceReady else {
                    ipcLog("⚠️ scheduleFinalSizeDebounce: conditions not met, skipping notification")
                    return
                }

                self.lastSentContainerSize = finalSize
                self.lastSizeNotificationTime = Date()
                ipcLog("📤 scheduleFinalSizeDebounce: sending containerSizeNotification size=\(Int(finalSize.width))x\(Int(finalSize.height))")
                self.sendContainerSizeNotification(size: finalSize)

                // 🔑 Force remoteView Refresh Display
                // NSRemoteView Sometimes it doesn't automatically refresh its content.
                self.hostView?.forceRemoteViewRefresh()
            }

            // Remove frame Change listening device (debounce ♪ End of ♪
            self.removeFrameChangeObserver()

            // Start sizeApplied Timer
            self.scheduleSnapshotTimeout()
        }
    }

    /// Harmonized size update method - All size changes should be done this way.
    /// Use a throttle-restriction mechanism to limit the frequency of transmission and apply the major changes immediately
    /// Must be called in the main line
    func updateViewportSize(_ newSize: CGSize) {
        // Make sure the listening is set
        setupLiveResizeObserverIfNeeded()
        // Ensure implementation of the main route
        guard Thread.isMainThread else {
            DispatchQueue.main.async { [weak self] in
                self?.updateViewportSize(newSize)
            }
            return
        }

        // 🔑 Use floor() Take care of it, and WorkspaceMainViewport/WorkspaceView Consistency
        // Avoid round() vs floor() Variances attributable to 1px Shake
        let roundedSize = CGSize(
            width: floor(newSize.width),
            height: floor(newSize.height)
        )

        // Validate size validity
        guard roundedSize.width >= 50 && roundedSize.height >= 50 &&
              roundedSize.width <= 4096 && roundedSize.height <= 4096 else {
            return
        }

        // Ignore same size (take-down comparison)
        guard roundedSize != currentViewportSize else {
            // If you're in debounce The state is still unchanged, but the size is still to be reset. debounce Timer
            // This is done. SwiftUI Possible multiple transmissions of the same size
            if isWaitingForFinalSize {
                scheduleFinalSizeDebounce()
            }
            return
        }

        // Update Local Status
        let previousSize = currentViewportSize
        currentViewportSize = roundedSize

        // 🔑 Key change: update immediately and always hostView frame + Send Notification To Service
        // Before. debounce/The arterial logic leads to Host and Service Size does not sync
        // Simplification to: apply all size changes immediately
        applyViewportSize(roundedSize)
    }

    /// Update after schedule saver (must be called on main line)
    private func scheduleThrottledUpdate() {
        guard throttleTimer == nil else { return }

        // Ensure Timer It's on the main line. RunLoop Up schedule
        throttleTimer = Timer.scheduledTimer(withTimeInterval: Self.throttleInterval, repeats: false) { [weak self] _ in
            DispatchQueue.main.async {
                guard let self = self, let pendingSize = self.pendingSize else { return }
                self.throttleTimer = nil
                self.pendingSize = nil
                self.applyViewportSize(pendingSize)
            }
        }
    }

    /// Practical application of size update (must be called in the main line)
    private func applyViewportSize(_ size: CGSize) {
        lastUpdateTime = Date()

        // 1. Update hostView Local frame(Present)
        hostView?.applyFrameSize(size)
        rendererHostView?.apply(size: size, isOverviewMode: isOverviewMode)

        // 2. 🔑 Check that all four conditions are met before you can send a notice.
        //    Otherwise by didSet Triggered trySendInitialConfig I'll handle it.
        guard allocatedServiceId != nil else { return }
        guard hasConnection else { return }
        guard xpcServiceReady else { return }

        // 3. Send containerSizeChanged Notified Service(Preventing duplication)
        //    🔑 resize Send it also during the period, Jean RN Layer Dynamic Response Dimension Change
        guard size != lastSentContainerSize else { return }
        lastSentContainerSize = size
        sendContainerSizeNotification(size: size)
    }

    func captureSnapshot() -> NSImage? {
        rendererHostView?.captureSnapshot() ?? hostView?.captureSnapshot()
    }

    /// Send a containerSizeChanged notification to the XPC service
    /// Internal approach - By applyViewportSize Call
    /// 🔑 Send directly DistributedNotification♪ I'm not leaving ♪ IPCBridgeAdapter, because tabStates (not registered)
    private func sendContainerSizeNotification(size: CGSize) {
        guard let tabId = tabId,
              let serviceId = allocatedServiceId else {
            NSLog("⚠️ [Coordinator] Cannot send containerSizeChanged - no tabId or serviceId")
            return
        }

        if rendererHostView != nil {
            RendererProcessPool.shared.updateViewport(
                tabId: tabId,
                size: size,
                isOverviewMode: isOverviewMode
            )
            return
        }

        // Generate request ID for log correlation
        let requestId = Int.random(in: 10000...99999)

        // Build sizeInfo Data
        let sizeInfo: [String: Any] = [
            RuntimeBridgeContract.tabId: tabId.uuidString,
            "width": size.width,
            "height": size.height,
            "requestId": requestId,
            "isOverviewMode": isOverviewMode
        ]

        guard let jsonData = try? JSONSerialization.data(withJSONObject: sizeInfo),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            NSLog("⚠️ [Coordinator] Failed to serialize sizeInfo")
            return
        }

        // 🔑 Send directly DistributedNotification,Format with Service Peer-to-penetrating match
        let notificationName = "com.actro.containerSizeChanged.\(ActriumHostProcessId).\(serviceId)"

        ipcLog("📐 [HOST→SVC] containerSizeChanged req=\(requestId) tab=\(String(tabId.uuidString.prefix(8))) size=\(Int(size.width))x\(Int(size.height)) isOverviewMode=\(isOverviewMode) name=\(notificationName)")

        DistributedNotificationCenter.default().postNotificationName(
            NSNotification.Name(notificationName),
            object: nil,
            userInfo: ["sizeInfo": jsonString],
            deliverImmediately: true
        )
    }

    /// Send left panel visibility change to XPC service via DistributedNotification
    /// Called when user toggles the left panel via address bar menu icon
    func sendLeftPanelVisibilityChanged(_ visible: Bool) {
        guard let tabId = tabId,
              let serviceId = allocatedServiceId else {
            ipcLog("⚠️ sendLeftPanelVisibilityChanged: missing tabId or allocatedServiceId")
            return
        }

        if let rendererHostView, let tabConfig {
            var config = tabConfig.toDictionary(isOverviewMode: isOverviewMode)
            config["leftPanelVisible"] = visible
            rendererHostView.apply(config: config)
            return
        }

        let payload: [String: Any] = [
            RuntimeBridgeContract.tabId: tabId.uuidString,
            "isLeftPanelVisible": visible
        ]

        guard let jsonData = try? JSONSerialization.data(withJSONObject: payload),
              let jsonString = String(data: jsonData, encoding: .utf8) else {
            ipcLog("⚠️ sendLeftPanelVisibilityChanged: failed to serialize payload")
            return
        }

        let notificationName = "com.actro.uiStateChanged.\(ActriumHostProcessId).\(serviceId)"

        ipcLog("📤 [HOST→SVC] leftPanelVisibility tab=\(String(tabId.uuidString.prefix(8))) visible=\(visible) name=\(notificationName)")

        DistributedNotificationCenter.default().postNotificationName(
            NSNotification.Name(notificationName),
            object: nil,
            userInfo: ["payload": jsonString],
            deliverImmediately: true
        )
    }

    /// Cleaning XPC Resources
    func cleanup() {
        // Clean up the throttle timer
        throttleTimer?.invalidate()
        throttleTimer = nil
        pendingSize = nil

        // Cleaning final size debounce Timer
        finalSizeDebounceTimer?.invalidate()
        finalSizeDebounceTimer = nil
        isWaitingForFinalSize = false

        // Cleaning frame Change listening device
        removeFrameChangeObserver()

        if let observer = xpcReadyObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
            xpcReadyObserver = nil
        }
        if let observer = connectionCreatedObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
            connectionCreatedObserver = nil
        }
        if let observer = listenerReadyObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
            listenerReadyObserver = nil
        }
        if let observer = liveResizeStartedObserver {
            NotificationCenter.default.removeObserver(observer)
            liveResizeStartedObserver = nil
        }
        if let observer = liveResizeEndedObserver {
            NotificationCenter.default.removeObserver(observer)
            liveResizeEndedObserver = nil
        }
        if let observer = sizeAppliedObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
            sizeAppliedObserver = nil
        }
        snapshotTimeoutTimer?.invalidate()
        snapshotTimeoutTimer = nil
        // Disconnect ViewBridge Connection
        hostView?.disconnect()
        hostView = nil
        rendererHostView?.prepareForReuse()
        rendererHostView = nil

        if let tabId = tabId, allocatedServiceId != nil {
            RendererProcessPool.shared.releaseRenderer(for: tabId)
            ipcLog("📦 Coordinator cleanup: Tab \(String(tabId.uuidString.prefix(8))) released renderer")
        }
        allocatedServiceId = nil
        xpcServiceReady = false
        hasConnection = false
    }

    deinit {
        cleanup()
    }
}

/// Manager that holds coordinators for all tabs
/// Centralized management isOverviewMode Status, Subscriptions BrowserViewModel.actriumMode Change
/// All components should be retrieved from here./Update isOverviewMode, not decentralized
final class ViewBridgeCoordinatorManager: ObservableObject {
    // Do Not Use @Published - coordinators ♪ There's no need to trigger the change ♪ UI Update
    private var coordinators: [UUID: ViewBridgeCoordinator] = [:]

    /// Global overview mode state - used when creating new coordinators
    /// By actriumMode Subscription auto-update, external should not be changed directly
    /// Default false Because... BrowserViewModel.init Medium Settings actriumMode = .focus
    private(set) var currentOverviewMode: Bool = false

    /// Combine subscription for actriumMode changes
    private var actriumModeSubscription: AnyCancellable?

    /// Singleton for global access
    static let shared = ViewBridgeCoordinatorManager()

    /// Write debug message to IPC log file
    private func writeToIPCLog(_ message: String) {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss.SSS"
        let timestamp = formatter.string(from: Date())
        let line = "[\(timestamp)] [HOST] \(message)\n"
        let logPath = "/tmp/actrium_ipc_debug.log"
        if let handle = FileHandle(forWritingAtPath: logPath) {
            handle.seekToEndOfFile()
            if let data = line.data(using: .utf8) {
                handle.write(data)
            }
            handle.closeFile()
        } else {
            FileManager.default.createFile(atPath: logPath, contents: line.data(using: .utf8))
        }
    }

    /// ♪ Tie it to the BrowserViewModel,subscribe actriumMode Change
    /// I think so. app Call once on startup
    func bindToViewModel(_ viewModel: any ActriumModePublisher) {
        // Unsubscribed before (if any)
        actriumModeSubscription?.cancel()

        // Sync the current status immediately (Focus Mode → isOverviewMode = false)
        // Note: Must call updateOverviewMode To update existing coordinators
        let isOverview = !viewModel.actriumMode.isInFocusMode
        writeToIPCLog("📐 [ViewBridgeCoordinatorManager] bindToViewModel: isInFocusMode=\(viewModel.actriumMode.isInFocusMode), isOverview=\(isOverview)")
        NSLog("📐 [ViewBridgeCoordinatorManager] bindToViewModel: actriumMode.isInFocusMode=%@, isOverview=%@, currentOverviewMode=%@",
              viewModel.actriumMode.isInFocusMode ? "true" : "false",
              isOverview ? "true" : "false",
              currentOverviewMode ? "true" : "false")

        // Force update (even with the same value) to ensure that existing coordinators Updated
        forceUpdateOverviewMode(isOverview)

        // Subscriptions actriumMode Change
        actriumModeSubscription = viewModel.actriumModePublisher
            .dropFirst() // Skip initial value (synced above)
            .sink { [weak self] (newMode: ActriumMode) in
                let isOverview = !newMode.isInFocusMode
                self?.writeToIPCLog("📐 [actriumModeSubscription] newMode=\(newMode), isOverview=\(isOverview)")
                self?.updateOverviewMode(isOverview)
            }

        writeToIPCLog("📐 [ViewBridgeCoordinatorManager] Bound to ViewModel, isOverviewMode=\(isOverview)")
        NSLog("📐 [ViewBridgeCoordinatorManager] Bound to ViewModel, isOverviewMode=%@",
              isOverview ? "true" : "false")
    }

    func coordinator(for tabId: UUID) -> ViewBridgeCoordinator {
        if let existing = coordinators[tabId] {
            return existing
        }
        let newCoordinator = ViewBridgeCoordinator()
        // Set isOverviewMode based on current global state
        newCoordinator.isOverviewMode = currentOverviewMode
        coordinators[tabId] = newCoordinator
        return newCoordinator
    }

    func removeCoordinator(for tabId: UUID) {
        coordinators.removeValue(forKey: tabId)
    }

    /// Internal approach: update all coordinators It's... isOverviewMode and notify services
    /// By actriumMode Subscriptions to AutoCall, not directly from the outside
    private func updateOverviewMode(_ isOverview: Bool) {
        // Avoidance of updates
        guard currentOverviewMode != isOverview else { return }
        applyOverviewMode(isOverview)
    }

    /// Force Update All coordinators It's... isOverviewMode(no current value checked)
    /// To ensure that there is an existing presence at the time of initial binding coordinators Updated
    private func forceUpdateOverviewMode(_ isOverview: Bool) {
        applyOverviewMode(isOverview)
    }

    /// Actual implementation isOverviewMode Updated Core Logic
    /// 🔑 Note: This method is only updated isOverviewMode Marks and Resets lastSentContainerSize
    ///    Not sent here containerSizeChanged Notification, because at this time coordinator.currentViewportSize
    ///    It's still old. Focus Mode size. The correct size will be notified SwiftUI Passes while re-rendering
    ///    onChange(of: currentSize) → updateViewportSize → sendContainerSizeNotification Send
    private func applyOverviewMode(_ isOverview: Bool) {
        // Update global state for newly created coordinators
        currentOverviewMode = isOverview
        // Write to IPC log file for debugging
        writeToIPCLog("📐 [applyOverviewMode] isOverview=\(isOverview) coordinatorCount=\(coordinators.count)")
        NSLog("📐 [applyOverviewMode] isOverview=%@ coordinatorCount=%d", isOverview ? "true" : "false", coordinators.count)

        for (tabId, coordinator) in coordinators {
            writeToIPCLog("📐 [applyOverviewMode] tab=\(tabId.uuidString.prefix(8)) hasConnection=\(coordinator.hasConnection) serviceId=\(coordinator.allocatedServiceId ?? "nil")")
            NSLog("📐 [applyOverviewMode] tab=%@ hasConnection=%@ serviceId=%@",
                  String(tabId.uuidString.prefix(8)),
                  coordinator.hasConnection ? "true" : "false",
                  coordinator.allocatedServiceId ?? "nil")
            coordinator.isOverviewMode = isOverview
            // Reset lastSentContainerSize to force re-send notification when SwiftUI updates the view
            // with the correct cardSize. This ensures the next updateViewportSize() call sends the notification.
            coordinator.resetLastSentContainerSize()
            // 🔑 Not sent here!coordinator.currentViewportSize It's old. Focus Mode Dimensions
            // SwiftUI It's going through when it's re-demonstrating. onChange(of: currentSize) Send Right cardSize
        }
    }
}

// MARK: - ViewBridge Host View Manager

/// Global Management ViewBridgeHostView Example, avoid loss of view reconstruction XPC Connection
/// Similar WebViewHolderManager Model
@MainActor
final class ViewBridgeHostViewManager {
    static let shared = ViewBridgeHostViewManager()
    private var hostViews: [UUID: ViewBridgeHostView] = [:]
    /// Cached snapshots for Overview mode
    private var snapshots: [UUID: NSImage] = [:]

    private init() {}

    /// Get or create a ViewBridgeHostView for a tab
    func hostView(for tabId: UUID, serviceBundleIdentifier: String, coordinator: ViewBridgeCoordinator, contentSize: CGSize, allowsHitTesting: Bool = true) -> ViewBridgeHostView {
        if let existing = hostViews[tabId] {
            // Update coordinator reference
            existing.coordinator = coordinator
            coordinator.hostView = existing
            // Update hit testing state (Overview mode disables hit testing)
            existing.allowsHitTesting = allowsHitTesting
            // Pass. coordinator Harmonized treatment of size updates (including notifications) Service)
            coordinator.updateViewportSize(contentSize)
            return existing
        }

        let new = ViewBridgeHostView(serviceBundleIdentifier: serviceBundleIdentifier, initialContentSize: contentSize)
        new.coordinator = coordinator
        new.allowsHitTesting = allowsHitTesting
        coordinator.hostView = new
        hostViews[tabId] = new
        return new
    }

    /// Get existing host view without creating a new one
    func existingHostView(for tabId: UUID) -> ViewBridgeHostView? {
        hostViews[tabId]
    }

    func remove(tabId: UUID) {
        if let removed = hostViews.removeValue(forKey: tabId) {
            removed.disconnect()
        }
        snapshots.removeValue(forKey: tabId)
    }

    // MARK: - Snapshot Management

    /// Take a snapshot of the ViewBridge content for Overview mode
    /// Note: TabSnapshotCacheManager is now managed by WorkspaceMainView.exitToOverview()
    func takeSnapshot(for tabId: UUID) {
        guard let hostView = hostViews[tabId] else { return }
        if let snapshot = hostView.captureSnapshot() {
            // Legacy: store in local cache only
            snapshots[tabId] = snapshot
        }
    }

    /// Get cached snapshot for Overview mode
    /// Returns from TabSnapshotCacheManager (V3.0) if available, falls back to local cache
    func snapshot(for tabId: UUID) -> NSImage? {
        // V3.0: prefer filtered thumbnail from TabSnapshotCacheManager
        if let filtered = TabSnapshotCacheManager.shared.filteredThumbnail(for: tabId) {
            return filtered
        }
        // Fallback to local cache (legacy)
        return snapshots[tabId]
    }
}

// MARK: - Container View

/// Container View (Use flipped Coordinates to Match SwiftUI)
/// SwiftUI Using the coordinates of the original point at the top left corner,NSView Need matching
private class ContainerView: NSView {
    override var isFlipped: Bool { true }

    /// Allow hit testing(by) SwiftUI Pass. userInteractionEnabled Control)
    /// Overview Mode to falseLet's get the gestures through the lower floor. Grid
    var allowsHitTesting: Bool = true {
        didSet {
            // 🔑 Yes. Overview Disable drag target in mode to allow drag event to penetrate lower level Grid
            if !allowsHitTesting {
                unregisterDraggedTypes()
            }
        }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.masksToBounds = true
        layer?.actions = ["bounds": NSNull(), "position": NSNull(), "frame": NSNull()]
        // Let subviews follow the container resize
        autoresizesSubviews = true
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)

        // Disable hidden animations, ensure one step in place
        CATransaction.begin()
        CATransaction.setDisableActions(true)

        // 🔑 Force synchronisation of all subviews frame(autoresizingMask Unreliable)
        for subview in subviews {
            if subview.frame.size != newSize {
                subview.frame = NSRect(origin: .zero, size: newSize)
            }
        }

        CATransaction.commit()
    }

    /// Rewrite hitTest Support for conditional gesture penetration.
    /// Overview Return under Mode nil, to pass events to the lower view
    override func hitTest(_ point: NSPoint) -> NSView? {
        if !allowsHitTesting {
            return nil  // Through the lower floor.
        }
        return super.hitTest(point)
    }
}

// MARK: - Persistent ViewBridge View

/// It's a long one. ViewBridge View Packer
/// Use ViewBridgeHostViewManager Get Lasting hostViewAvoid SwiftUI Lost in reconstruction XPC Connection
/// Similar PersistentWebView Model
struct PersistentViewBridgeView: NSViewRepresentable {
    let tabId: UUID
    let serviceBundleIdentifier: String
    let coordinator: ViewBridgeCoordinator
    let size: CGSize
    /// Allow hit testing(Overview Mode to falseLet the gestures through)
    var allowsHitTesting: Bool = true

    func makeNSView(context: Context) -> NSView {
        let container = ContainerView(frame: NSRect(origin: .zero, size: size))
        container.allowsHitTesting = allowsHitTesting
        coordinator.tabId = tabId

        // From Manager Get Lasting hostView
        let hostView = ViewBridgeHostViewManager.shared.hostView(
            for: tabId,
            serviceBundleIdentifier: serviceBundleIdentifier,
            coordinator: coordinator,
            contentSize: size,
            allowsHitTesting: allowsHitTesting
        )

        // Set Connection to Create Backup
        // 🔑 Set Right Here hasConnection, because onAppear Medium setupConnectionCallback
        // Possible execution after connection is established (time-series problem)
        // 🔑 Call at the same time trySendInitialConfig()♪ Make sure even onConnectionEstablished Yes nil It can be sent. config
        hostView.onConnectionEstablished = { [weak coordinator] in
            guard let coordinator = coordinator else { return }
            NSLog("🔗 [PersistentView] Connection established for tab %@, setting hasConnection=true",
                  String(tabId.uuidString.prefix(8)))
            coordinator.hasConnection = true

            // 🔑 Announcements IPCBridgeAdapter, sync to IPCInteractionManager
            IPCBridgeAdapter.shared.markTabConnected(tabId)

            // Try sending it directly first. config(Resolutions onConnectionEstablished Maybe. nil (on time series issues)
            coordinator.trySendInitialConfig()
            // Call the optional backshow (in %2)UnifiedReactNativeLayer Could have set up extra processing)
            coordinator.onConnectionEstablished?()
        }

        // If hostView Existing superview(reused) Remove
        let wasReused = hostView.superview != nil
        if wasReused {
            NSLog("🔄 [PersistentView] hostView reused for tab %@, old superview=%@",
                  String(tabId.uuidString.prefix(8)), String(describing: hostView.superview))
            hostView.removeFromSuperview()
        }

        // Use autoresizingMask - Force from (0,0) Action.
        // 🔑 Use size Parameters (%2)point No, I'm fine. container.bounds.size"Maybe." @2x Pixels Size)
        hostView.frame = NSRect(origin: .zero, size: size)
        hostView.autoresizingMask = [.width, .height]
        container.addSubview(hostView)

        NSLog("🔧 [PersistentView] makeNSView: tab=%@, containerSize=%@, hostView.frame=%@, hasRemote=%d",
              String(tabId.uuidString.prefix(8)), NSStringFromSize(size),
              NSStringFromRect(hostView.frame), hostView.hasRemoteViewController ? 1 : 0)

        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        // Ensure coordinator.tabId Set
        if coordinator.tabId == nil {
            coordinator.tabId = tabId
        }

        // 🔑 Update hit testing Status (%1)Overview Disable mode, let the gesture go through)
        if let containerView = container as? ContainerView {
            containerView.allowsHitTesting = allowsHitTesting
        }

        // 🔑 Critical repairs: use of imported size Parameters, not container.bounds.size
        // SwiftUI Use .position() It's probably not updated when you're located. container.bounds
        // Incoming. size Parameters are the right target size (Overview Under mode. cardSize)
        let targetSize = size

        // 🔑 Rehabilitation @2x Question:container.bounds Yes. Retina Possible pixel size return on monitor
        // Need to detect and convert to point Dimensions
        let rawBounds = container.bounds.size
        let scaleFactor = container.window?.backingScaleFactor ?? 1.0

        // Test bounds Is it? @2x: If bounds About equal targetSize * scaleFactor, the conversion is required
        let boundsLooksLikePixels = scaleFactor > 1.0 &&
            abs(rawBounds.width - targetSize.width * scaleFactor) < 2 &&
            abs(rawBounds.height - targetSize.height * scaleFactor) < 2

        let containerBounds: CGSize
        if boundsLooksLikePixels {
            // bounds It's pixel size. point Dimensions
            containerBounds = CGSize(width: rawBounds.width / scaleFactor, height: rawBounds.height / scaleFactor)
            ipcLog("⚠️ updateNSView: converted @\(Int(scaleFactor))x bounds: \(Int(rawBounds.width))x\(Int(rawBounds.height)) → \(Int(containerBounds.width))x\(Int(containerBounds.height))")
        } else {
            containerBounds = rawBounds
        }

        let isLiveResizing = WindowResizeState.shared.isLiveResizing

        // Debug Log
        ipcLog("📐 updateNSView: tab=\(String(tabId.uuidString.prefix(8))), targetSize=\(Int(targetSize.width))x\(Int(targetSize.height)), containerBounds=\(Int(containerBounds.width))x\(Int(containerBounds.height)), rawBounds=\(Int(rawBounds.width))x\(Int(rawBounds.height)), scale=\(scaleFactor), allowsHitTesting=\(allowsHitTesting)")

        guard targetSize.width > 10 && targetSize.height > 10 else {
            ipcLog("⚠️ updateNSView: skipping - targetSize too small")
            return
        }

        // 🔑 Simplified logic: always preferred SwiftUI Incoming. targetSize
        // ♪ Try before ♪ live resize Use during bounds, but bounds Possible Return @2x Pixels size, causing problems.
        // targetSize By SwiftUI Correctly calculated point Size, more reliable.
        let effectiveSize = targetSize

        // 🔑 Key: Whatever. hostView Existence, update coordinator Size
        // That's to make sure. currentViewportSize It's always right. point Dimensions
        // I was just... hostView Update only when it exists, resulting in initialization currentViewportSize Could possibly set another code path to @2x
        coordinator.updateViewportSize(effectiveSize)

        // 🔑 Key improvements: always updated hostView frameincluding live resize Period
        // I was here. live resize Skip updates during this period, which may result in resize After the end
        // hostView.frame with SwiftUI The desired sizes are not consistent.
        if let hostView = container.subviews.first as? ViewBridgeHostView {
            let hostSize = hostView.frame.size
            let sizeDiff = abs(hostSize.width - effectiveSize.width) + abs(hostSize.height - effectiveSize.height)
            if sizeDiff > 0.5 {
                // Use CATransaction Avoid hidden animations
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                hostView.frame = NSRect(origin: .zero, size: effectiveSize)
                CATransaction.commit()
            }
            // Update hit testing state on hostView as well
            hostView.allowsHitTesting = allowsHitTesting
        }
    }

    static func dismantleNSView(_ container: NSView, coordinator: ()) {
        // Do Not Remove hostView - It's by Manager Hold
        // Next time. makeNSView It's the same thing. hostView
        ipcLog("⚠️ PersistentViewBridgeView.dismantleNSView called (hostView retained by manager)")
    }
}

// MARK: - Nine Grid Debug Overlay

/// Nine-gauge debug view - For visual viewing position
struct NineGridDebugOverlay: View {
    let size: CGSize

    var body: some View {
        ZStack {
            // Grid Line
            GeometryReader { proxy in
                let drawingSize = proxy.size
                let cellWidth = drawingSize.width / 3
                let cellHeight = drawingSize.height / 3

                ZStack {
                    Path { path in
                        for i in 1..<3 {
                            let x = cellWidth * CGFloat(i)
                            path.move(to: CGPoint(x: x, y: 0))
                            path.addLine(to: CGPoint(x: x, y: drawingSize.height))
                        }
                        for i in 1..<3 {
                            let y = cellHeight * CGFloat(i)
                            path.move(to: CGPoint(x: 0, y: y))
                            path.addLine(to: CGPoint(x: drawingSize.width, y: y))
                        }
                    }
                    .stroke(.red.opacity(0.5), lineWidth: 2)

                    Path(CGRect(x: 1, y: 1, width: drawingSize.width - 2, height: drawingSize.height - 2))
                        .stroke(.red, lineWidth: 2)

                    Path { path in
                        let centerX = drawingSize.width / 2
                        let centerY = drawingSize.height / 2
                        path.move(to: CGPoint(x: centerX - 20, y: centerY))
                        path.addLine(to: CGPoint(x: centerX + 20, y: centerY))
                        path.move(to: CGPoint(x: centerX, y: centerY - 20))
                        path.addLine(to: CGPoint(x: centerX, y: centerY + 20))
                    }
                    .stroke(.blue, lineWidth: 2)
                }
            }

            // Grid Numbering
            VStack(spacing: 0) {
                ForEach(0..<3) { row in
                    HStack(spacing: 0) {
                        ForEach(0..<3) { col in
                            let index = row * 3 + col + 1
                            Text("\(index)")
                                .font(.system(size: 48, weight: .bold))
                                .foregroundColor(.red.opacity(0.3))
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                        }
                    }
                }
            }

            // Size Information
            VStack {
                HStack {
                    Spacer()
                    Text(String(format: "%.0f × %.0f", size.width, size.height))
                        .font(.system(size: 14, weight: .medium, design: .monospaced))
                        .foregroundColor(.white)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.red.opacity(0.7))
                        .cornerRadius(4)
                        .padding(8)
                }
                Spacer()
            }
        }
        // Constraints frame Yes sizeAvoids the size of the parent view resulting in an over-gravity of the grid
        .frame(width: size.width, height: size.height)
        .allowsHitTesting(false) // Do not stop interaction
    }
}

// MARK: - Unified React Native Layer

/// Unified ViewBridge Content Layer - Always remixing, never destroying.
///
// MARK: - Conditional Position Modifier

/// Conditional application `.position()` Modifier
/// - Focus Mode: Do Not Apply position, natural fill of the parent container in view (avoiding coordinate calculation problems)
/// - Overview Mode: Apply position Position to card
private struct ConditionalPositionModifier: ViewModifier {
    let usePosition: Bool
    let x: CGFloat
    let y: CGFloat

    func body(content: Content) -> some View {
        if usePosition {
            // Overview/Transition mode: use position I'm sure we can get a hold of the card.
            content.position(x: x, y: y)
        } else {
            // Focus Mode: Fill parent container, align to upper left corner
            // 🔑 Must use frame(maxWidth/maxHeight) Jean. NSViewRepresentable Getting the right bounds
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }
}

/// 🔑 Core design: When the solution mode switch ViewBridge The problem of disconnection
/// - PersistentViewBridgeView Always mount,hostView Never Remove From Containers
/// - scale Always 1,no scaleEffect
/// - Size by Mode (Media Query Style,Service Rendering the size)
/// - Focus Mode:size=viewportSize, position=(0,0)
/// - Overview Mode:size=cardSize, position=cardPosition
/// - Animation transitions:opacity=0by Animation Scattered photo mask
struct UnifiedReactNativeLayer: View {
    let tab: Tab
    let fingerprint: String
    let viewportSize: CGSize      // Focus Size of Mode (fullscreen)
    let cardSize: CGSize          // Overview Size of Mode (Card Size)
    let cardPosition: CGPoint     // Overview Location of mode (top left of card)
    let isFocused: Bool           // Current tab Is it in or out? Focus Status
    let isOverviewMode: Bool      // Current position or position Overview Mode (for border display)
    let isTransitioning: Bool     // Is the transition animation ongoing
    let isDragSource: Bool        // Whether to drag source (Blue Flank Line Border)
    let isDropTarget: Bool        // Whether to drag or drop target (green solid border)
    let coordinator: ViewBridgeCoordinator

    // System app data (for StartPageApp)
    var bookmarks: [[String: Any]] = []
    var recentHistory: [[String: Any]] = []

    init(
        tab: Tab,
        fingerprint: String,
        viewportSize: CGSize,
        cardSize: CGSize,
        cardPosition: CGPoint,
        isFocused: Bool,
        isOverviewMode: Bool,
        isTransitioning: Bool,
        isDragSource: Bool = false,
        isDropTarget: Bool = false,
        coordinator: ViewBridgeCoordinator,
        bookmarks: [[String: Any]] = [],
        recentHistory: [[String: Any]] = []
    ) {
        self.tab = tab
        self.fingerprint = fingerprint
        self.viewportSize = viewportSize
        self.cardSize = cardSize
        self.cardPosition = cardPosition
        self.isFocused = isFocused
        self.isOverviewMode = isOverviewMode
        self.isTransitioning = isTransitioning
        self.isDragSource = isDragSource
        self.isDropTarget = isDropTarget
        self.coordinator = coordinator
        self.bookmarks = bookmarks
        self.recentHistory = recentHistory
        // Note: coordinator setup moved to onAppear to avoid redundant side effects
        // during SwiftUI body re-evaluation (init called multiple times per tab)
    }

    /// Current Size
    /// 🔑 Maintained during the transition period Focus Size, avoid flashing.
    private var currentSize: CGSize {
        // During transition period: keep large sizes to cover the animation
        // Non-transitional period: based on isFocused Select Size
        if isTransitioning {
            return viewportSize  // Always Focus Dimensions
        }
        return isFocused ? viewportSize : cardSize
    }

    /// Current position (top left)
    /// 🔑 Maintained during the transition period Focus Position, avoid flashing.
    private var currentPosition: CGPoint {
        // During transition period: maintain position of origin and allow animated layer to cover
        // Non-transitional period: based on isFocused Organisation
        if isTransitioning {
            return .zero  // Always Focus Location
        }
        return isFocused ? .zero : cardPosition
    }

    // XPC Status
    @State private var isLoading = true
    @State private var errorMessage: String?
    @State private var hasValidConfigSent = false
    /// 🔑 Trigger SwiftUI Rendering state (%2)coordinator Change does not automatically trigger)
    @State private var serviceAllocated = false

    // Listening resize Status, for displaying a snapshot mask
    @ObservedObject private var resizeState = WindowResizeState.shared

    // Debug Mode Switches
    private static let showDebugGrid = false

    /// Inspection service Distribution (used) @State Trigger SwiftUI Rendering)
    private var isServiceAllocated: Bool {
        serviceAllocated && coordinator.allocatedServiceId != nil
    }

    var body: some View {
        // 🔍 Debug Log
        let _ = NSLog("🔍 [UnifiedRN] body: tab=%@, isFocused=%d, transitioning=%d, size=%.0fx%.0f, pos=(%.0f,%.0f)",
                      String(tab.id.uuidString.prefix(8)),
                      isFocused ? 1 : 0,
                      isTransitioning ? 1 : 0,
                      currentSize.width, currentSize.height,
                      currentPosition.x, currentPosition.y)

        ZStack(alignment: .topLeading) {
            // Renderer surface stays mounted while Focus/Overview only changes
            // its viewport and placement.
            if isServiceAllocated, coordinator.allocatedServiceId != nil {
                PersistentRendererProcessView(
                    tabId: tab.id,
                    fingerprint: fingerprint,
                    coordinator: coordinator,
                    size: currentSize,
                    allowsHitTesting: isFocused && !isTransitioning,
                    config: currentRendererConfig
                )
                .frame(width: currentSize.width, height: currentSize.height, alignment: .topLeading)
                .onAppear {
                    isLoading = false
                    coordinator.hasConnection = true
                }
            } else if let error = errorMessage {
                Text(error)
                    .foregroundColor(.red)
                    .frame(width: currentSize.width, height: currentSize.height)
            } else {
                ProgressView("Allocating process...")
                    .controlSize(.regular)
                    .frame(width: currentSize.width, height: currentSize.height)
            }

            // Nine-gauge debugging overlay(only) Focus (Model and non-transitional)
            if Self.showDebugGrid && isFocused && !isTransitioning {
                NineGridDebugOverlay(size: currentSize)
            }

            // 🔑 resize No cover is required for the period because the size changes will be passed to the RN Layer
            // Keep Only sizeApplied A mask for the waiting period (for resize (in the short period of time after its conclusion)
            if isFocused && !isTransitioning,
               resizeState.isWaitingForSizeApplied,
               resizeState.activeTabId == tab.id,
               let snapshot = TabSnapshotCacheManager.shared.focusSnapshot(for: tab.id) {
                Image(nsImage: snapshot)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fill)
                    .frame(width: currentSize.width, height: currentSize.height)
                    .clipped()
                    .allowsHitTesting(false)
            }

            // 🔑 Overview Card Border in Mode (Placed) ZStack Internal to ensure the correct size)
            // Use CardStyle.cornerRadius with WebTab Card Alignment
            // Border Styles and SlotView Consistency: Drag source blue lines, drag target green lines, default white lines
            if isOverviewMode && !isTransitioning {
                if isDragSource {
                    // Drag source: Blue dotted line (with SlotView (unanimously)
                    RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                        .stroke(
                            Color.blue.opacity(0.6),
                            style: StrokeStyle(lineWidth: 2, dash: [5, 5])
                        )
                        .allowsHitTesting(false)
                } else if isDropTarget {
                    // Drag-and-drop target: green line (and SlotView (unanimously)
                    RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                        .stroke(Color.green.opacity(0.7), lineWidth: 2)
                        .allowsHitTesting(false)
                } else {
                    // Default: White Line Border
                    RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                        .stroke(Color.white.opacity(0.15), lineWidth: 1)
                        .allowsHitTesting(false)
                }
            }
        }
        .frame(width: currentSize.width, height: currentSize.height, alignment: .topLeading)
        // 🔑 Overview Mode to add round and shadows, and TabPanel/NewTabPanel Harmonization
        .cornerRadius(isFocused ? 0 : CardStyle.cornerRadius)
        .shadow(color: isFocused ? .clear : Color.black.opacity(0.3), radius: 5, x: 0, y: 2)
        // 🔑 Positioning policy:
        // - Focus Mode: Do Not Use position, pass frame(maxWidth/maxHeight) Naturally fill parent packagings
        // - Overview/Transition mode: Use position Position to card position (%1)position Use center point coordinates)
        .modifier(ConditionalPositionModifier(
            usePosition: !isFocused || isTransitioning,
            x: currentPosition.x + currentSize.width / 2,
            y: currentPosition.y + currentSize.height / 2
        ))
        // 🔑 Disable Live All hidden animations on the layers (by Animation ♪ The sun is on the ground ♪
        .animation(nil, value: currentSize)
        .animation(nil, value: currentPosition)
        .animation(nil, value: isTransitioning)
        // 🔑 Full transparency during transition period and only after animation
        .opacity(isTransitioning ? 0 : 1)
        // 🔑 z-index Control:
        // - Focus Mode: zIndex(100) At the top, make sure that the interaction is at the top.
        // - Overview Mode: zIndex(50) Yes. actrium The front, the contents are visible.
        //   Pass. NSView It's... hitTest() Back nil Let the drag through. TabGridView
        .zIndex(isFocused ? 100 : 50)
        // Focus Allow interactive interaction in mode
        .allowsHitTesting(isFocused && !isTransitioning)
        .onAppear {
            // Coordinator setup (moved from init to avoid redundant calls during SwiftUI body re-evaluation)
            coordinator.tabId = tab.id
            coordinator.isOverviewMode = !isFocused
            coordinator.tabConfig = XPCTabConfig(
                tabId: tab.id,
                title: tab.title,
                backgroundColor: tab.backgroundColor,
                containerSize: isFocused ? viewportSize : cardSize,
                systemAppType: tab.systemTabType?.rawValue,
                unifiedScript: tab.unifiedScript,
                leftPanelScript: tab.leftPanelScript,
                leftPanelVisible: tab.leftPanelDefaultVisible,
                rightPanelVisible: tab.rightPanelDefaultVisible,
                permissions: tab.permissions,
                bookmarks: bookmarks,
                recentHistory: recentHistory,
                reloadKey: tab.reloadNonce
            )
            coordinator.currentViewportSize = currentSize
            ipcLog("📱 UnifiedReactNativeLayer.onAppear: tab=\(String(tab.id.uuidString.prefix(8))), systemTabType=\(tab.systemTabType?.rawValue ?? "nil"), isFocused=\(isFocused)")
            setupConnectionCallback()
            allocateServiceIfNeeded()
            if coordinator.hasConnection && coordinator.xpcServiceReady && isCoordinatorViewportSizeValid {
                sendTabConfigIfReady()
            }
        }
        .onChange(of: tab.systemTabType) { _, newValue in
            // 🔑 System Tab When a type changes (e.g. from startPage Switch to history) Re-send config
            guard coordinator.hasConnection && coordinator.xpcServiceReady else { return }
            NSLog("🔄 [UnifiedRN] systemTabType changed to %@, resending config",
                  newValue?.rawValue ?? "nil")
            // 🔑 Reset lastSentConfigSize Force re-send.
            coordinator.lastSentConfigSize = nil
            sendTabConfigIfReady()
        }
        .onChange(of: tab.unifiedScript) { oldValue, newValue in
            // 🔑 unifiedScript When change occurs (e.g. from startPage Switch to ACTR app) Re-send config
            let oldPreview = oldValue.map { String($0.prefix(30)) + "..." } ?? "nil"
            let newPreview = newValue.map { String($0.prefix(30)) + "..." } ?? "nil"
            NSLog("🔄 [UnifiedRN] onChange(unifiedScript): old=\(oldPreview), new=\(newPreview), hasConn=\(coordinator.hasConnection), xpcReady=\(coordinator.xpcServiceReady)")
            guard coordinator.hasConnection && coordinator.xpcServiceReady else {
                NSLog("⚠️ [UnifiedRN] unifiedScript changed but connection not ready, skipping")
                return
            }
            NSLog("🔄 [UnifiedRN] unifiedScript changed, resending config")
            coordinator.lastSentConfigSize = nil
            sendTabConfigIfReady()
        }
        .onChange(of: tab.reloadNonce) { _, _ in
            // 🔑 Reload Trigger: Force new sending config(includes reloadKey)
            coordinator.tabConfig = XPCTabConfig(
                tabId: tab.id,
                title: tab.title,
                backgroundColor: tab.backgroundColor,
                containerSize: isFocused ? viewportSize : cardSize,
                systemAppType: tab.systemTabType?.rawValue,
                unifiedScript: tab.unifiedScript,
                leftPanelScript: tab.leftPanelScript,
                leftPanelVisible: tab.leftPanelDefaultVisible,
                rightPanelVisible: tab.rightPanelDefaultVisible,
                permissions: tab.permissions,
                bookmarks: bookmarks,
                recentHistory: recentHistory,
                reloadKey: tab.reloadNonce
            )
            coordinator.lastSentConfigSize = nil
            coordinator.lastSentReloadKey = nil
            sendTabConfigIfReady()
        }
        // Size changes by PersistentRendererProcessView.updateNSView Uniform
        // No more. onChange Listen, avoid multi-source competition leading to a twitch.
    }

    private var currentRendererConfig: [String: Any] {
        var config: [String: Any] = [
            RuntimeBridgeContract.tabId: tab.id.uuidString,
            RuntimeBridgeContract.title: tab.title,
            "backgroundColor": tab.backgroundColor,
            RuntimeBridgeContract.containerWidth: currentSize.width,
            RuntimeBridgeContract.containerHeight: currentSize.height,
            "isOverviewMode": !isFocused,
            "leftPanelVisible": tab.leftPanelDefaultVisible,
            "rightPanelVisible": tab.rightPanelDefaultVisible,
            "permissions": tab.permissions,
            "bookmarks": bookmarks,
            "recentHistory": recentHistory,
            "reloadKey": tab.reloadNonce
        ]
        if let systemAppType = tab.systemTabType?.rawValue {
            config[RuntimeBridgeContract.systemAppType] = systemAppType
        }
        if let unifiedScript = tab.unifiedScript {
            config["unifiedScript"] = unifiedScript
        }
        // 15-B B4: verified-bytes source (digest-addressed extraction dir + entry).
        // The renderer prefers reading the guest bytes from this cache directory
        // and falls back to the inline unifiedScript text above.
        if let extractedPath = tab.guestExtractedPath {
            config["guestExtractedPath"] = extractedPath
            if let entry = tab.guestEntry {
                config["guestEntry"] = entry
            }
        }
        if let leftPanelScript = tab.leftPanelScript {
            config["leftPanelScript"] = leftPanelScript
        }
        return config
    }

    /// Inspection viewportSize Validity (not initial) 100x100)- For direct access view It's... viewportSize
    private var isViewportSizeValid: Bool {
        viewportSize.width > 100 && viewportSize.height > 100
    }

    /// Inspection coordinator Medium viewportSize Validity - For access to latest values in closed packages
    /// Note: Use of a threshold of easing (> 10)Because... Overview Mode cardSize It's probably very small.
    /// But it still needs to be sent. tabConfig Jean. RN Renders content
    private var isCoordinatorViewportSizeValid: Bool {
        coordinator.currentViewportSize.width > 10 && coordinator.currentViewportSize.height > 10
    }

    private func allocateServiceIfNeeded() {
        if coordinator.allocatedServiceId != nil {
            // Distributions made to ensure @State Sync
            if !serviceAllocated {
                serviceAllocated = true
            }
            // Fast Path: Check for preheat Renderer Has it been connected.
            checkPrewarmStatusAndSetReady()
            return
        }

        do {
            let serviceId = try RendererProcessPool.shared.allocateRenderer(
                for: tab.id,
                fingerprint: fingerprint
            )
            coordinator.allocatedServiceId = serviceId
            // 🔑 Trigger SwiftUI Render
            serviceAllocated = true
            checkPrewarmStatusAndSetReady()
        } catch let error as RendererProcessPoolError {
            errorMessage = error.localizedDescription
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// 🔥 Fast Path: If Service Preheated, direct tag. ready And try to send config
    private func checkPrewarmStatusAndSetReady() {
        guard let serviceId = coordinator.allocatedServiceId else { return }

        if RendererProcessPool.shared.isRendererReady(identifier: serviceId) {
            if !coordinator.xpcServiceReady {
                coordinator.xpcServiceReady = true
                NSLog("🔥 [Prewarm] Renderer %@ already ready, fast path enabled",
                      serviceId.components(separatedBy: ".").last ?? "")
            }
            sendTabConfigIfReady()
        }
    }

    private func setupXPCReadyListener() {
        guard coordinator.xpcReadyObserver == nil,
              let serviceId = coordinator.allocatedServiceId else {
            return
        }

        // 🔥 Quick Path Already checkPrewarmStatusAndSetReady Processing
        // Only listeners are set up here as backup (processors) Service Relaunch, etc.)

        // Listening com.actro.xpcServiceReady(no suffix) pass userInfo.bundleId and hostProcessId Filter
        coordinator.xpcReadyObserver = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.xpcServiceReady"),
            object: nil,
            queue: .main
        ) { [serviceId] notification in
            // Only from the target. service Notification
            guard let userInfo = notification.userInfo,
                  let bundleId = userInfo["bundleId"] as? String,
                  bundleId == serviceId else { return }
            // Filter: Only notifications that belong to this process
            if let hostPid = userInfo["hostProcessId"] as? Int32,
               hostPid != ActriumHostProcessId {
                return
            }

            // 🔑 Announcements IPCBridgeAdapter, sync to IPCInteractionManager
            IPCBridgeAdapter.shared.markServiceReady(serviceId)
            coordinator.xpcServiceReady = true
            sendTabConfigIfReady()
        }

        // 🔑 Settings listenerReady Listener - Addressing competitive conditions
        // Service Set the belt. PID The wire will be sent after. listenerReady Announcements
        // Host Retransmit after receipt containerSizeChanged
        setupListenerReadyListener(serviceId: serviceId)
    }

    /// Settings Service "listener ready" Call the wires.
    /// Dealing with competing conditions:Service Maybe. Host Send containerSizeChanged Then we set up the wire.
    private func setupListenerReadyListener(serviceId: String) {
        guard coordinator.listenerReadyObserver == nil else { return }

        let listenerReadyName = "com.actro.listenerReady.\(ActriumHostProcessId).\(serviceId)"
        ipcLog("🎧 Setting up listenerReady listener: \(listenerReadyName)")

        coordinator.listenerReadyObserver = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name(listenerReadyName),
            object: nil,
            queue: .main
        ) { [weak coordinator] notification in
            guard let coordinator = coordinator else { return }
            guard let userInfo = notification.userInfo,
                  let notifyingServiceId = userInfo["serviceId"] as? String,
                  notifyingServiceId == serviceId else { return }

            let tabIdShort = coordinator.tabId?.uuidString.prefix(8) ?? "nil"
            ipcLog("📥 [HOST] Received listenerReady for tab=\(tabIdShort), re-sending containerSizeChanged")

            // 🔑 Reset lastSentContainerSize Force Remittance
            coordinator.resetLastSentContainerSize()
            coordinator.trySendInitialConfig()
        }
    }

    private func setupConnectionCallback() {
        coordinator.onConnectionEstablished = {
            coordinator.hasConnection = true
            sendTabConfigIfReady()
        }

        // Check if a connection has been established (through hasRemoteViewController (Joseph)
        if coordinator.hostView?.hasRemoteViewController == true && !coordinator.hasConnection {
            coordinator.onConnectionEstablished?()
        }

        // 🔑 If hasConnection It's already been done. true(by) PersistentViewBridgeView.makeNSView (Setting),
        // and xpcServiceReadySend it directly. tabConfig
        if coordinator.hasConnection && coordinator.xpcServiceReady {
            NSLog("🔗 [setupConnectionCallback] Connection already established, sending config")
            sendTabConfigIfReady()
        }
    }

    private func sendTabConfigIfReady() {
        // Use coordinator Medium viewportSize, because this method could be called from the closed package
        let currentSize = coordinator.currentViewportSize
        let tabIdShort = String(tab.id.uuidString.prefix(8))

        // Inspection coordinator Medium viewportSize Validity (not initial) 100x100)
        guard isCoordinatorViewportSizeValid else {
            NSLog("⚠️ [sendTabConfig] tab=%@ SKIP: viewportSize invalid (%.0fx%.0f)",
                  tabIdShort, currentSize.width, currentSize.height)
            return
        }

        guard coordinator.hasConnection,
              let serviceId = coordinator.allocatedServiceId else {
            NSLog("⚠️ [sendTabConfig] tab=%@ SKIP: hasConnection=%d, xpcReady=%d, serviceId=%@",
                  tabIdShort, coordinator.hasConnection ? 1 : 0,
                  coordinator.xpcServiceReady ? 1 : 0,
                  coordinator.allocatedServiceId ?? "nil")
            return
        }

        let reloadKey = tab.reloadNonce
        // Prevents the re-sending of the same size and reloadKey Unchanged config
        if coordinator.lastSentConfigSize == currentSize && coordinator.lastSentReloadKey == reloadKey {
            return
        }
        coordinator.lastSentConfigSize = currentSize
        coordinator.lastSentReloadKey = reloadKey

        // Send tab config - Use coordinator the latest size of the
        var tabConfig: [String: Any] = [
            RuntimeBridgeContract.tabId: tab.id.uuidString,
            RuntimeBridgeContract.title: tab.title,
            "backgroundColor": tab.backgroundColor,
            RuntimeBridgeContract.containerWidth: currentSize.width,
            RuntimeBridgeContract.containerHeight: currentSize.height,
            // 🔑 Read state update from status manager instead of coordinator Local Cache
            "isOverviewMode": ViewBridgeCoordinatorManager.shared.currentOverviewMode,
            "reloadKey": reloadKey
        ]

        // Add optional scripts
        if let unifiedScript = tab.unifiedScript {
            tabConfig["unifiedScript"] = unifiedScript
        }
        // 15-B B4: verified-bytes source (digest-addressed extraction dir + entry).
        if let extractedPath = tab.guestExtractedPath {
            tabConfig["guestExtractedPath"] = extractedPath
            if let entry = tab.guestEntry {
                tabConfig["guestEntry"] = entry
            }
        }
        if let systemTabType = tab.systemTabType {
            tabConfig[RuntimeBridgeContract.systemAppType] = systemTabType.rawValue
        }
        if !tab.permissions.isEmpty {
            tabConfig["permissions"] = tab.permissions
        }

        // Add system app data (for StartPageApp)
        if !bookmarks.isEmpty {
            tabConfig["bookmarks"] = bookmarks
        }
        if !recentHistory.isEmpty {
            tabConfig["recentHistory"] = recentHistory
        }

        NSLog("📤 [sendTabConfig] tab=%@ service=%@ size=%.0fx%.0f systemAppType=%@ bookmarks=%d history=%d",
              tabIdShort, serviceId.components(separatedBy: ".").last ?? "",
              currentSize.width, currentSize.height,
              tab.systemTabType?.rawValue ?? "nil",
              bookmarks.count, recentHistory.count)

        coordinator.rendererHostView?.apply(config: tabConfig)
    }
}

/// SwiftUI wrapper for ViewBridgeHostView
/// Remote view size Follow TabPanel Actual size
/// Support for the allocation of independence from the process pool XPC service
struct ViewBridgeTabView: NSViewRepresentable {
    let tabId: UUID
    let contentSize: CGSize  // TabPanel Actual size
    let coordinator: ViewBridgeCoordinator
    let serviceBundleIdentifier: String  // From the process pool bundle id

    init(tabId: UUID, contentSize: CGSize, coordinator: ViewBridgeCoordinator, serviceBundleIdentifier: String) {
        self.tabId = tabId
        self.contentSize = contentSize
        self.coordinator = coordinator
        self.serviceBundleIdentifier = serviceBundleIdentifier
    }

    func makeNSView(context: Context) -> ViewBridgeHostView {
        NSLog("🆕 ViewBridgeTabView.makeNSView: tab=%@, service=%@, size=%.0fx%.0f",
              String(tabId.uuidString.prefix(8)),
              serviceBundleIdentifier.components(separatedBy: ".").last ?? "",
              contentSize.width, contentSize.height)
        let view = ViewBridgeHostView(serviceBundleIdentifier: serviceBundleIdentifier, initialContentSize: contentSize)
        // Set coordinator reference for lifecycle management
        view.coordinator = coordinator
        coordinator.hostView = view
        // Set Connection to Create Backup
        // 🔑 Set Right Here hasConnection, because onAppear Medium setupConnectionCallback
        // Possible execution after connection is established (time-series problem)
        view.onConnectionEstablished = { [weak coordinator, tabId] in
            guard let coordinator = coordinator else { return }
            NSLog("🔗 [TabView] Connection established, setting hasConnection=true")
            coordinator.hasConnection = true
            // 🔑 Announcements IPCBridgeAdapter, sync to IPCInteractionManager
            IPCBridgeAdapter.shared.markTabConnected(tabId)
            coordinator.onConnectionEstablished?()
        }
        return view
    }

    func updateNSView(_ nsView: ViewBridgeHostView, context: Context) {
        ipcLog("📐 ViewBridgeTabView.updateNSView: contentSize=\(Int(contentSize.width))x\(Int(contentSize.height))")
        // Ensure coordinator reference is set
        nsView.coordinator = coordinator
        coordinator.hostView = nsView
        // Unified by coordinator Send containerSizeChanged(HostView Only local size updates)
        coordinator.updateViewportSize(contentSize)
    }

    static func dismantleNSView(_ nsView: ViewBridgeHostView, coordinator: ()) {
        // Not cut in here!SwiftUI It could be called more often because of size changes. dismantle
        // Connection life cycle by ViewBridgeCoordinator.deinit Management
        NSLog("⚠️ ViewBridgeTabView.dismantleNSView called (NOT disconnecting - managed by coordinator)")
        // Note: Old design calls. nsView.disconnect(), but this will accidentally disconnect the connection when size changes.
    }
}
