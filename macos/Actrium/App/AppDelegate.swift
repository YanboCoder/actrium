import AppKit
import SwiftUI
import Combine

/// Application delegate - manages AppKit windows
final class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {

    // MARK: - Properties

    /// Shared view models (global across all windows)
    let historyVM = HistoryViewModel()
    let bookmarkVM = BookmarkViewModel()

    /// Active window controllers
    private var windowControllers: [MainWindowController] = []

    /// Currently focused browser view model
    @Published var focusedViewModel: BrowserViewModel?

    private var cancellables = Set<AnyCancellable>()

    // MARK: - NSApplicationDelegate

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Disable window state restoration
        UserDefaults.standard.set(false, forKey: "NSQuitAlwaysKeepsWindows")

        // Suppress common system warnings in development
        UserDefaults.standard.set(false, forKey: "NSConstraintBasedLayoutLogUnsatisfiable")

        // Initialize services
        initializeServices()

        // Setup notification listeners before creating windows
        setupNotifications()

        // Create initial window
        createNewWindow()

        // Ensure focusedViewModel is set after the run loop settles
        DispatchQueue.main.async { [weak self] in
            self?.refreshFocusedViewModel()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false  // Keep app running even when all windows are closed
    }

    func applicationWillTerminate(_ notification: Notification) {
        RendererProcessPool.shared.shutdown()
        VaultManager.shared.lock()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            createNewWindow()
        }
        return true
    }

    // MARK: - Initialization

    private func initializeServices() {
        // Setup crash handler
        CrashHandler.setup()

        // Initialize XPC notification bridge
        _ = XPCNotificationBridge.shared

        // Initialize TabProcessManager
        _ = TabProcessManager.shared

        // Prewarm the browser-style homogeneous Renderer pool.
        Task { @MainActor in
            _ = RendererProcessPool.shared.verifyRendererHelper()
            await RendererProcessPool.shared.prewarmInitialRenderersAsync()
        }

        // Initialize vault manager
        _ = VaultManager.shared

        // Pre-compile ad-blocking rules so they're ready when tabs open
        ContentBlockerManager.shared.loadRules { _ in }

        // Restore the local-folder browser sync provider selection
        SyncManager.shared.restoreOnLaunch()
    }

    private func setupNotifications() {
        // Listen for new window requests
        NotificationCenter.default.publisher(for: .openNewWindowWithGroup)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                self?.createNewWindow()
            }
            .store(in: &cancellables)

        // Track key window changes so SwiftUI menus re-evaluate
        for name in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            NotificationCenter.default.publisher(for: name)
                .receive(on: RunLoop.main)
                .sink { [weak self] _ in
                    self?.refreshFocusedViewModel()
                }
                .store(in: &cancellables)
        }
    }

    private func refreshFocusedViewModel() {
        focusedViewModel = windowControllers.first { $0.window?.isKeyWindow == true }?.viewModel
    }

    // MARK: - Window Management

    @objc func createNewWindow() {
        let viewModel = BrowserViewModel()
        viewModel.historyVM = historyVM

        let windowController = MainWindowController(
            viewModel: viewModel,
            historyVM: historyVM,
            bookmarkVM: bookmarkVM
        )

        windowControllers.append(windowController)
        windowController.showWindow()
        refreshFocusedViewModel()

        // Restart restore (15-B B7): rebuild the persisted session into the
        // first window; actr:// instance addresses rebind with the same ActorId.
        viewModel.restoreSessionIfNeeded()

        // Register with DevTools
        DevToolsManager.shared.activeBrowserViewModel = viewModel
    }

    func removeWindowController(_ controller: MainWindowController) {
        windowControllers.removeAll { $0 === controller }
        refreshFocusedViewModel()
    }

    func toggleNativeFullScreenForKeyWindow() {
        if let controller = windowControllers.first(where: { $0.window?.isKeyWindow == true }) {
            controller.toggleNativeFullScreen()
            return
        }

        if let keyWindow = NSApp.keyWindow,
           let controller = windowControllers.first(where: { $0.window === keyWindow }) {
            controller.toggleNativeFullScreen()
            return
        }

        NSApp.keyWindow?.toggleFullScreen(nil)
    }
}
