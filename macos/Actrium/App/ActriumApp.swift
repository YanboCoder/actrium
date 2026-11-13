import SwiftUI
import Combine

private let logger = AppLogger.app

// MARK: - DistributedNotificationCenter Publisher Extension

extension DistributedNotificationCenter {
    /// Create a Combine publisher for distributed notifications
    func publisher(for name: NSNotification.Name) -> AnyPublisher<Notification, Never> {
        NotificationCenter.default.publisher(for: name)
            .eraseToAnyPublisher()
    }
}

/// Helper class to bridge DistributedNotificationCenter to NotificationCenter for SwiftUI
final class XPCNotificationBridge {
    static let shared = XPCNotificationBridge()

    // Local notification names (for SwiftUI .onReceive)
    static let openBookmark = Notification.Name("xpc.openBookmark")
    static let openHistoryItem = Notification.Name("xpc.openHistoryItem")
    static let deleteBookmark = Notification.Name("xpc.deleteBookmark")
    static let deleteHistoryItem = Notification.Name("xpc.deleteHistoryItem")
    static let restoreHistoryGroup = Notification.Name("xpc.restoreHistoryGroup")
    static let clearHistory = Notification.Name("xpc.clearHistory")
    static let openURLFromStartPage = Notification.Name("xpc.openURLFromStartPage")
    static let capabilityPermissionRequest = Notification.Name(
        "xpc.capabilityPermissionRequest"
    )
    static let xpcLog = Notification.Name("xpc.log")

    private var observers: [NSObjectProtocol] = []

    private init() {
        setupListeners()
    }

    private func setupListeners() {
        // Bridge distributed notifications to local NotificationCenter

        // Open bookmark
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.openBookmark"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.openBookmark, object: nil, userInfo: notification.userInfo)
        })

        // Open history item
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.openHistoryItem"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.openHistoryItem, object: nil, userInfo: notification.userInfo)
        })

        // Delete bookmark
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.deleteBookmark"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.deleteBookmark, object: nil, userInfo: notification.userInfo)
        })

        // Delete history item
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.deleteHistoryItem"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.deleteHistoryItem, object: nil, userInfo: notification.userInfo)
        })

        // Restore history group
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.restoreHistoryGroup"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.restoreHistoryGroup, object: nil, userInfo: notification.userInfo)
        })

        // Clear history
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.clearHistory"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.clearHistory, object: nil, userInfo: notification.userInfo)
        })

        // Open URL from Start Page
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.openURLFromStartPage"),
            object: nil,
            queue: .main
        ) { notification in
            NotificationCenter.default.post(name: Self.openURLFromStartPage, object: nil, userInfo: notification.userInfo)
        })

        // XPC Log (forward to DevTools)
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.actro.xpcLog"),
            object: nil,
            queue: .main
        ) { notification in
            guard let userInfo = notification.userInfo,
                  let level = userInfo["level"] as? String,
                  let source = userInfo["source"] as? String,
                  let message = userInfo["message"] as? String else { return }

            let logLevel: DevToolsLogEntry.LogLevel
            switch level {
            case "warn": logLevel = .warn
            case "error": logLevel = .error
            case "debug": logLevel = .debug
            default: logLevel = .info
            }

            Task { @MainActor in
                DevToolsManager.shared.log(message, level: logLevel, source: "[XPC] \(source)")
            }
        })

        logger.info("XPCNotificationBridge: Initialized with \(self.observers.count) listeners")
    }
}

// MARK: - Focused Value for Active Window's ViewModel

struct FocusedBrowserViewModelKey: FocusedValueKey {
    typealias Value = BrowserViewModel
}

extension FocusedValues {
    var browserViewModel: BrowserViewModel? {
        get { self[FocusedBrowserViewModelKey.self] }
        set { self[FocusedBrowserViewModelKey.self] = newValue }
    }
}

// MARK: - Debug Launch Configuration

/// Configuration for debugging launch with specific window/mode settings
/// Usage: Actrium.app --window-frame=100,100,800,600 --mode=focus --tab=startPage
struct DebugLaunchConfig {
    /// Window frame (x, y, width, height)
    var windowFrame: NSRect?
    /// Display mode: "focus" or "overview"
    var mode: String?
    /// Initial tab type
    var tabType: String?
    /// Log file path for redirecting output
    var logFile: String?

    /// Parse command line arguments
    static func parse(_ args: [String]) -> DebugLaunchConfig {
        var config = DebugLaunchConfig()

        for arg in args {
            if arg.hasPrefix("--window-frame=") {
                let value = String(arg.dropFirst("--window-frame=".count))
                let parts = value.split(separator: ",").compactMap { Double($0) }
                if parts.count == 4 {
                    config.windowFrame = NSRect(x: parts[0], y: parts[1], width: parts[2], height: parts[3])
                    NSLog("🔧 [DebugConfig] Window frame: \(config.windowFrame!)")
                }
            } else if arg.hasPrefix("--mode=") {
                config.mode = String(arg.dropFirst("--mode=".count))
                NSLog("🔧 [DebugConfig] Mode: \(config.mode!)")
            } else if arg.hasPrefix("--tab=") {
                config.tabType = String(arg.dropFirst("--tab=".count))
                NSLog("🔧 [DebugConfig] Tab: \(config.tabType!)")
            } else if arg.hasPrefix("--log-file=") {
                config.logFile = String(arg.dropFirst("--log-file=".count))
                NSLog("🔧 [DebugConfig] Log file: \(config.logFile!)")
                // Setup log file redirection
                setupLogFile(config.logFile!)
            }
        }

        return config
    }

    /// Redirect stderr/stdout to a log file
    private static func setupLogFile(_ path: String) {
        // Clear existing log file
        try? FileManager.default.removeItem(atPath: path)
        FileManager.default.createFile(atPath: path, contents: nil)

        // Redirect stderr to file (Swift print and fputs go here)
        freopen(path, "a", stderr)
        // Also redirect stdout
        freopen(path, "a", stdout)

        NSLog("🔧 [DebugConfig] Logs redirected to: \(path)")
    }

    /// Check if any debug options are set
    var hasDebugOptions: Bool {
        windowFrame != nil || mode != nil || tabType != nil
    }
}

// MARK: - App

@main
struct ActriumApp: App {
    // Use AppDelegate for window management
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    @ObservedObject private var devTools = DevToolsManager.shared
    @Environment(\.openWindow) private var openWindow

    // Access focused viewModel via AppDelegate
    private var focusedViewModel: BrowserViewModel? {
        appDelegate.focusedViewModel
    }

    private var historyVM: HistoryViewModel {
        appDelegate.historyVM
    }

    private var bookmarkVM: BookmarkViewModel {
        appDelegate.bookmarkVM
    }

    /// Start of the resolution from the command line parameter URL
    static var launchURL: URL? = {
        let args = CommandLine.arguments
        NSLog("🚀 Actro: Command line args: \(args)")
        if let urlIndex = args.firstIndex(where: { $0 == "--url" || $0 == "-url" }),
           urlIndex + 1 < args.count {
            let urlString = args[urlIndex + 1]
            if let url = URL(string: urlString) {
                NSLog("🚀 Actro: Launch URL from command line: \(urlString)")
                return url
            }
        }
        return nil
    }()

    /// Host-designated package allowed to use the legacy ACTR_CALL bridge.
    /// Supplied by the trusted launch configuration, never by package data.
    static let launchLegacyActorBridgePackage: String? = {
        let args = CommandLine.arguments
        guard let packageIndex = args.firstIndex(where: {
            $0 == "--legacy-actor-bridge-package"
        }), packageIndex + 1 < args.count else {
            return nil
        }
        let path = URL(fileURLWithPath: args[packageIndex + 1])
            .standardizedFileURL.path
        NSLog("🚀 Actrium: Legacy actor bridge package: \(path)")
        return path
    }()

    /// Whether `url` is the host-designated legacy ACTR_CALL package.
    static func isLegacyActorBridgePackage(_ url: URL) -> Bool {
        guard let trustedPath = launchLegacyActorBridgePackage else {
            return false
        }
        return url.standardizedFileURL.path == trustedPath
    }

    /// Debug launch configuration
    static let debugConfig: DebugLaunchConfig = DebugLaunchConfig.parse(CommandLine.arguments)

    var body: some Scene {
        // Minimal window group for menu commands
        // Main windows are managed by AppDelegate
        WindowGroup(id: "placeholder") {
            Color.clear
                .frame(width: 0, height: 0)
                .hidden()
        }
        .defaultLaunchBehavior(.suppressed)
        .restorationBehavior(.disabled)
        .commands {
            // New Window command
            CommandGroup(before: .newItem) {
                Button("New Window") {
                    openNewWindow()
                }
                .keyboardShortcut("n", modifiers: [.command, .shift])
            }

            // History menu
            CommandMenu("History") {
                Button("Show All History") {
                    focusedViewModel?.openSystemTab(.history)
                }
                .keyboardShortcut("y", modifiers: .command)

                Divider()

                if historyVM.recentItems.isEmpty {
                    Text("No Recent History")
                        .foregroundColor(.secondary)
                } else {
                    ForEach(historyVM.recentItems.prefix(10)) { item in
                        Button(item.title) {
                            openHistoryItem(item)
                        }
                    }
                }

                Divider()

                Button("Clear History...") {
                    historyVM.clearHistory()
                }
                .disabled(historyVM.recentItems.isEmpty)
            }

            // Bookmarks menu
            CommandMenu("Bookmarks") {
                Button("Add Bookmark...") {
                    // TODO: Show bookmark sheet for current tab
                }
                .keyboardShortcut("d", modifiers: .command)

                Button("Show All Bookmarks") {
                    focusedViewModel?.openSystemTab(.bookmarks)
                }
                .keyboardShortcut("b", modifiers: [.command, .option])

                Divider()

                if bookmarkVM.bookmarks.isEmpty {
                    Text("No Bookmarks")
                        .foregroundColor(.secondary)
                } else {
                    ForEach(bookmarkVM.bookmarks.prefix(10)) { bookmark in
                        Button(bookmark.title) {
                            openBookmark(bookmark)
                        }
                    }
                }
            }

            // Window menu additions
            CommandGroup(after: .windowList) {
                Divider()

                Button("App Manager") {
                    AppsManagerWindowController.shared.showWindow()
                }
                .keyboardShortcut("m", modifiers: [.command, .shift])
            }

            // DevTools menu
            CommandMenu("Developer") {
                // For Web tabs: use WebKit Inspector; for actr tabs: use built-in DevTools
                Button("Toggle DevTools") {
                    toggleDevToolsForCurrentTab()
                }
                .keyboardShortcut("i", modifiers: [.command, .option])

                Divider()

                // actr DevTools section
                Button("Toggle ACTR DevTools") {
                    devTools.toggle()
                }

                Button("Open ACTR DevTools in New Window") {
                    devTools.detach()
                    openWindow(id: "devtools")
                }
                .keyboardShortcut("i", modifiers: [.command, .option, .shift])

                Button("Dock ACTR DevTools") {
                    devTools.dock()
                }
                .disabled(devTools.displayMode != .detached)

                Divider()

                Menu("Dock Position") {
                    Button("Bottom") {
                        devTools.dockPosition = .bottom
                    }
                    .disabled(devTools.dockPosition == .bottom)

                    Button("Right") {
                        devTools.dockPosition = .right
                    }
                    .disabled(devTools.dockPosition == .right)
                }
                .disabled(devTools.displayMode != .docked)

                Divider()

                Button("Clear Console") {
                    devTools.clearConsoleLogs()
                }
                .keyboardShortcut("k", modifiers: [.command, .option])
            }

            // Settings (Cmd+,)
            CommandGroup(replacing: .appSettings) {
                Button("Settings...") {
                    SettingsWindowController.shared.showWindow()
                }
                .keyboardShortcut(",", modifiers: .command)
            }

            // Help menu
            CommandGroup(replacing: .help) {
                Button("About App Status") {
                    logger.notice("✅ App running normally")
                    logger.notice("✅ All views loaded")
                }
            }

            // Tab management shortcuts
            CommandGroup(after: .newItem) {
                Button("New Tab") {
                    withAnimation {
                        _ = focusedViewModel?.addNewTab()
                    }
                }
                .keyboardShortcut("t", modifiers: .command)

                Button("Close the current tab") {
                    // If a utility/panel window (e.g. Settings) is key, close it via standard ⌘W
                    if let keyWindow = NSApp.keyWindow, keyWindow != NSApp.mainWindow {
                        keyWindow.performClose(nil)
                    } else {
                        withAnimation {
                            focusedViewModel?.closeCurrentTab()
                        }
                    }
                }
                .keyboardShortcut("w", modifiers: .command)

                Divider()

                Button("Refresh the current tab") {
                    focusedViewModel?.reloadCurrentTab()
                }
                .keyboardShortcut("r", modifiers: .command)

                Divider()

                Button("Focus Address Bar") {
                    focusedViewModel?.shouldAutoFocusAddressBar = true
                }
                .keyboardShortcut("l", modifiers: .command)

                Divider()

                Button("Print...") {
                    printCurrentPage()
                }
                .keyboardShortcut("p", modifiers: .command)
            }

            // View menu - keep zoom shortcuts and route fullscreen toggle to active browser window.
            CommandGroup(replacing: .toolbar) {
                Button("Zoom in.") {
                    zoomIn()
                }
                .keyboardShortcut("+", modifiers: .command)

                Button("Zoom Out") {
                    zoomOut()
                }
                .keyboardShortcut("-", modifiers: .command)

                Button("Actual Size") {
                    resetZoom()
                }
                .keyboardShortcut("0", modifiers: .command)

                Divider()

                Button("Toggle Full Screen") {
                    toggleFullScreen()
                }
                .keyboardShortcut("f", modifiers: [.command, .control])
                            }

            // Tab switching shortcuts
            CommandGroup(after: .sidebar) {
                ForEach(1...8, id: \.self) { index in
                    Button("Switch to Tab \(index)") {
                        switchToTab(at: index - 1)
                    }
                    .keyboardShortcut(
                        KeyEquivalent(Character("\(index)")),
                        modifiers: .command
                    )
                                    }

                Button("Switch to Last Tab") {
                    switchToLastTab()
                }
                .keyboardShortcut("9", modifiers: .command)
                            }
        }

        // Detached DevTools window
        Window("DevTools", id: "devtools") {
            DevToolsWindowView(viewModel: focusedViewModel)
                .frame(minWidth: 600, minHeight: 400)
        }
        .windowResizability(.contentSize)
        .defaultSize(width: 900, height: 500)
    }

    private func openNewWindow() {
        appDelegate.createNewWindow()
    }

    private func switchToTab(at index: Int) {
        guard let vm = focusedViewModel,
              index >= 0, index < vm.tabs.count else { return }
        let tab = vm.tabs[index]
        withAnimation {
            vm.switchToTab(id: tab.id)
        }
    }

    private func switchToLastTab() {
        guard let vm = focusedViewModel,
              !vm.tabs.isEmpty else { return }
        let lastIndex = vm.tabs.count - 1
        let tab = vm.tabs[lastIndex]
        withAnimation {
            vm.switchToTab(id: tab.id)
        }
    }

    private func openHistoryItem(_ item: HistoryItem) {
        guard let vm = focusedViewModel else { return }
        if let url = item.appURL {
            // Use navigateCurrentTab which handles all URL schemes (actro://, http://, about:, etc.)
            vm.navigateCurrentTab(to: url.absoluteString)
            historyVM.recordVisit(item: item)
        }
    }

    private func openBookmark(_ bookmark: Bookmark) {
        guard let vm = focusedViewModel else { return }
        if let url = bookmark.appURL {
            // Unified by navigateCurrentTab Process
            vm.navigateCurrentTab(to: url.path)
            bookmarkVM.markAsUsed(bookmark)
            // Also record in history
            if let tab = vm.currentTab {
                historyVM.recordVisit(tab: tab)
            }
        }
    }

    // MARK: - DevTools

    /// Toggle DevTools based on current tab type:
    /// - Web tab: Open WebKit Web Inspector
    /// - ACTR tab: Toggle built-in DevTools panel
    private func toggleDevToolsForCurrentTab() {
        guard let vm = focusedViewModel,
              let tab = vm.currentTab else {
            // No tab, fallback to actr DevTools
            devTools.toggle()
            return
        }

        switch tab.contentType {
        case .web:
            // Web tab: open WebKit Inspector
            if let holder = WebViewHolderManager.shared.existingHolder(for: tab.id) {
                holder.showInspector()
            }
        case .reactNative, .empty, .native:
            // ACTR/RN, native, and empty tabs use built-in DevTools
            devTools.toggle()
        }
    }

    // MARK: - Print

    private func printCurrentPage() {
        guard let vm = focusedViewModel,
              let tab = vm.currentTab,
              case .web = tab.contentType,
              let holder = WebViewHolderManager.shared.existingHolder(for: tab.id) else {
            return
        }
        holder.print()
    }

    // MARK: - Zoom

    private func zoomIn() {
        guard let vm = focusedViewModel,
              let tab = vm.currentTab,
              case .web = tab.contentType,
              let holder = WebViewHolderManager.shared.existingHolder(for: tab.id) else {
            return
        }
        holder.zoomIn()
    }

    private func zoomOut() {
        guard let vm = focusedViewModel,
              let tab = vm.currentTab,
              case .web = tab.contentType,
              let holder = WebViewHolderManager.shared.existingHolder(for: tab.id) else {
            return
        }
        holder.zoomOut()
    }

    private func resetZoom() {
        guard let vm = focusedViewModel,
              let tab = vm.currentTab,
              case .web = tab.contentType,
              let holder = WebViewHolderManager.shared.existingHolder(for: tab.id) else {
            return
        }
        holder.resetZoom()
    }

    // MARK: - Fullscreen

    private func toggleFullScreen() {
        appDelegate.toggleNativeFullScreenForKeyWindow()
    }
}
