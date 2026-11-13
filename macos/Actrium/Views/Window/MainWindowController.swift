import AppKit
import SwiftUI
import Combine

// MARK: - Custom Window (titlebar double-click zoom support)

/// NSWindow subclass that restores double-click-to-zoom on the titlebar
/// when a custom toolbar overlay intercepts mouse events.
final class ActriumWindow: NSWindow {

    override func sendEvent(_ event: NSEvent) {
        guard event.type == .leftMouseUp, event.clickCount == 2,
              event.locationInWindow.y >= contentLayoutRect.maxY else {
            super.sendEvent(event)
            return
        }

        // Skip interactive controls (address bar, buttons, etc.)
        if let cv = contentView {
            let pt = cv.convert(event.locationInWindow, from: nil)
            if let hit = cv.hitTest(pt), hit is NSControl {
                super.sendEvent(event)
                return
            }
        }

        // Let the system handle the event first (it may trigger zoom internally)
        let frameBefore = frame
        super.sendEvent(event)

        // Only zoom ourselves if the system didn't already do it
        guard frame == frameBefore else { return }

        let action = UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") ?? "Maximize"
        switch action {
        case "Minimize": miniaturize(nil)
        case "None":     break
        default:         zoom(nil)
        }
    }
}

/// Main window controller - manages window chrome with self-drawn toolbar
final class MainWindowController: NSWindowController, NSWindowDelegate {

    // MARK: - Properties

    let viewModel: BrowserViewModel  // Public for AppDelegate access
    private let historyVM: HistoryViewModel
    private let bookmarkVM: BookmarkViewModel

    private var rootViewController: RootContentViewController!
    private var cancellables = Set<AnyCancellable>()
    private var fullScreenTitleLabel: NSTextField?
    private let fullScreenTrafficLightLeftInset: CGFloat = 18
    private let fullScreenTrafficLightGap: CGFloat = 9
    private let fullScreenTrafficLightVerticalNudge: CGFloat = 2
    private let fullScreenTitleGapAfterTrafficLights: CGFloat = 12
    private let fullScreenTitleFontSize: CGFloat = 12
    private let fullScreenTitleLineHeight: CGFloat = 22
    private let fullScreenTitleVerticalOffset: CGFloat = 3

    // MARK: - Initialization

    init(viewModel: BrowserViewModel, historyVM: HistoryViewModel, bookmarkVM: BookmarkViewModel) {
        self.viewModel = viewModel
        self.historyVM = historyVM
        self.bookmarkVM = bookmarkVM

        // Create window with transparent titlebar
        let window = ActriumWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1200, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        super.init(window: window)

        configureWindow(window)
        setupContentViewController()
        bindWindowTitle()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }


    // MARK: - Window Configuration

    private func configureWindow(_ window: NSWindow) {
        window.delegate = self

        // Transparent titlebar - traffic lights float over content
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.title = resolvedWindowTitle()
        window.backgroundColor = .clear

        // Hide titlebar separator line
        window.titlebarSeparatorStyle = .none

        // No system toolbar - we draw our own
        window.toolbar = nil

        // Use native fullscreen so macOS can place window in a dedicated Space/Desktop.
        var behavior = window.collectionBehavior
        behavior.remove([.fullScreenNone, .fullScreenDisallowsTiling])
        behavior.insert([.fullScreenPrimary, .fullScreenAllowsTiling])
        window.collectionBehavior = behavior

        // Minimum size
        window.minSize = NSSize(width: 640, height: 480)

        // Center on screen
        window.center()
    }

    func toggleNativeFullScreen() {
        guard let window else { return }
        window.toggleFullScreen(nil)
    }

    private func bindWindowTitle() {
        Publishers.CombineLatest(viewModel.$tabs, viewModel.$currentTabId)
            .receive(on: RunLoop.main)
            .sink { [weak self] _, _ in
                guard let self else { return }
                let title = self.resolvedWindowTitle()
                self.window?.title = title
                guard self.window?.styleMask.contains(.fullScreen) == true else { return }
                self.fullScreenTitleLabel?.stringValue = title
                self.updateFullScreenLeftAlignedTitleFrame()
            }
            .store(in: &cancellables)
    }

    private func resolvedWindowTitle() -> String {
        guard let tab = viewModel.currentTab else { return "Actrium" }
        let title = tab.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return title.isEmpty ? "New Tab" : title
    }

    // MARK: - Content Setup

    private func setupContentViewController() {
        guard let window = window else { return }

        // Create root content view controller with 4px edge inset
        rootViewController = RootContentViewController(
            viewModel: viewModel,
            historyVM: historyVM,
            bookmarkVM: bookmarkVM
        )

        // Set as window's content view controller
        window.contentViewController = rootViewController
        rootViewController?.updateWindowFullScreenState(window.styleMask.contains(.fullScreen))
    }

    // MARK: - Public Methods

    func showWindow() {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    // MARK: - NSWindowDelegate

    func window(
        _ window: NSWindow,
        willUseFullScreenPresentationOptions proposedOptions: NSApplication.PresentationOptions
    ) -> NSApplication.PresentationOptions {
        // Keep native fullscreen behavior while ensuring menu bar / Dock auto-hide.
        proposedOptions.union([.autoHideMenuBar, .autoHideDock])
    }

    func windowWillEnterFullScreen(_ notification: Notification) {
        guard let window else { return }
        window.title = resolvedWindowTitle()
        window.titlebarAppearsTransparent = false
        window.titleVisibility = .hidden
        rootViewController?.updateWindowFullScreenState(true)
        rootViewController?.view.needsLayout = true
    }

    func windowWillExitFullScreen(_ notification: Notification) {
        removeFullScreenLeftAlignedTitle()
        window?.titleVisibility = .hidden
        window?.titlebarAppearsTransparent = true
        rootViewController?.updateWindowFullScreenState(false)
        rootViewController?.view.needsLayout = true
    }

    func windowDidEnterFullScreen(_ notification: Notification) {
        rootViewController?.view.layoutSubtreeIfNeeded()
        rootViewController?.updateWindowFullScreenState(true)
        ensureFullScreenLeftAlignedTitle()
        updateFullScreenLeftAlignedTitleFrame()
        rootViewController?.view.needsLayout = true
    }

    func windowDidExitFullScreen(_ notification: Notification) {
        removeFullScreenLeftAlignedTitle()
        rootViewController?.updateWindowFullScreenState(false)
        rootViewController?.view.needsLayout = true
    }

    func windowDidResize(_ notification: Notification) {
        updateFullScreenLeftAlignedTitleFrame()
    }

    func windowWillClose(_ notification: Notification) {
        for tab in viewModel.tabs { RealmServiceBindings.shared.remove(tab.id) }
    }

    func windowDidBecomeKey(_ notification: Notification) {
        // Update DevTools reference
        DevToolsManager.shared.activeBrowserViewModel = viewModel
    }

    private func ensureFullScreenLeftAlignedTitle() {
        guard let window else { return }
        guard window.styleMask.contains(.fullScreen) else { return }
        guard let closeButton = window.standardWindowButton(.closeButton) else { return }
        guard let titlebarView = closeButton.superview else { return }

        let label: NSTextField
        if let existing = fullScreenTitleLabel {
            label = existing
        } else {
            let created = NSTextField(labelWithString: "")
            created.translatesAutoresizingMaskIntoConstraints = false
            created.lineBreakMode = .byTruncatingTail
            created.maximumNumberOfLines = 1
            created.font = NSFont.systemFont(ofSize: fullScreenTitleFontSize, weight: .medium)
            created.textColor = NSColor.labelColor.withAlphaComponent(0.9)
            created.alignment = .left
            fullScreenTitleLabel = created
            label = created
        }

        label.stringValue = resolvedWindowTitle()
        label.translatesAutoresizingMaskIntoConstraints = true
        if label.superview !== titlebarView {
            label.removeFromSuperview()
            titlebarView.addSubview(label)
        }
    }

    private func updateFullScreenLeftAlignedTitleFrame() {
        guard let window else { return }
        guard window.styleMask.contains(.fullScreen) else { return }
        guard let label = fullScreenTitleLabel else { return }
        guard let closeButton = window.standardWindowButton(.closeButton) else { return }
        guard let titlebarView = closeButton.superview else { return }

        if label.superview !== titlebarView {
            label.removeFromSuperview()
            titlebarView.addSubview(label)
        }

        let trafficLightsRightEdge = layoutFullScreenTrafficLights(in: titlebarView, window: window)
        let rightPadding: CGFloat = 16
        let x = trafficLightsRightEdge + fullScreenTitleGapAfterTrafficLights
        let availableWidth = max(80, titlebarView.bounds.width - x - rightPadding)
        let height: CGFloat = min(fullScreenTitleLineHeight, max(16, titlebarView.bounds.height - 2))
        let centeredY = floor((titlebarView.bounds.height - height) / 2)
        let y = titlebarView.isFlipped
            ? (centeredY + fullScreenTitleVerticalOffset)
            : (centeredY - fullScreenTitleVerticalOffset)
        label.frame = NSRect(x: x, y: y, width: availableWidth, height: height)
    }

    private func removeFullScreenLeftAlignedTitle() {
        guard let label = fullScreenTitleLabel else { return }
        label.removeFromSuperview()
    }

    @discardableResult
    private func layoutFullScreenTrafficLights(in titlebarView: NSView, window: NSWindow) -> CGFloat {
        let buttonTypes: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        let buttons = buttonTypes.compactMap { window.standardWindowButton($0) }
        guard !buttons.isEmpty else { return fullScreenTrafficLightLeftInset + 44 }

        let fallbackY = max(0, (titlebarView.bounds.height - 14) / 2)
        var x = fullScreenTrafficLightLeftInset
        var rightEdge = fullScreenTrafficLightLeftInset
        for button in buttons {
            button.translatesAutoresizingMaskIntoConstraints = true
            if button.superview !== titlebarView {
                button.removeFromSuperview()
                titlebarView.addSubview(button)
            }
            let size = button.frame.size == .zero ? button.fittingSize : button.frame.size
            let width = max(size.width, 12)
            let centeredY = floor((titlebarView.bounds.height - max(size.height, 12)) / 2)
            let baseY = centeredY.isFinite ? centeredY : fallbackY
            let y = titlebarView.isFlipped
                ? (baseY - fullScreenTrafficLightVerticalNudge)
                : (baseY + fullScreenTrafficLightVerticalNudge)
            button.setFrameOrigin(NSPoint(x: x, y: y))
            rightEdge = max(rightEdge, x + width)
            x += width + fullScreenTrafficLightGap
        }
        return rightEdge
    }
}
