import AppKit
import SwiftUI

// MARK: - Divider-Aware Container

/// Content container that can expose a narrow leading-edge passthrough zone,
/// allowing divider drag gestures to reach NSSplitView when content overlays
/// (e.g. RN/XPC host views) aggressively capture mouse events.
private final class DividerAwareContainerView: NSView {
    var leadingPassthroughWidth: CGFloat = 0

    override func hitTest(_ point: NSPoint) -> NSView? {
        if leadingPassthroughWidth > 0, point.x <= leadingPassthroughWidth {
            return nil
        }
        return super.hitTest(point)
    }
}

/// Content view controller - hosts SwiftUI WorkspaceView with rounded corners
final class ContentViewController: NSViewController {

    // MARK: - Properties

    private let viewModel: BrowserViewModel
    private let historyVM: HistoryViewModel
    private let bookmarkVM: BookmarkViewModel

    private var hostingView: NSHostingView<ContentAreaView>!

    /// Corner radius for content area (matches roundedContainer)
    private let cornerRadius: CGFloat = 12
    private var containerView: DividerAwareContainerView?

    // MARK: - Initialization

    init(viewModel: BrowserViewModel, historyVM: HistoryViewModel, bookmarkVM: BookmarkViewModel) {
        self.viewModel = viewModel
        self.historyVM = historyVM
        self.bookmarkVM = bookmarkVM

        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    // MARK: - Lifecycle

    override func loadView() {
        // Create SwiftUI content
        let contentView = ContentAreaView(
            viewModel: viewModel,
            historyVM: historyVM,
            bookmarkVM: bookmarkVM,
            cornerRadius: cornerRadius
        )

        // Create hosting view
        hostingView = NSHostingView(rootView: contentView)
        hostingView.translatesAutoresizingMaskIntoConstraints = false

        // Container view with rounded corners only on bottom (top connects to toolbar)
        let container = DividerAwareContainerView()
        container.wantsLayer = true
        container.layer?.cornerRadius = cornerRadius
        // CALayer corner constants use geometric minY/maxY; on macOS non-flipped layer
        // bottom corners are MinY (not MaxY).
        container.layer?.maskedCorners = [.layerMinXMinYCorner, .layerMaxXMinYCorner]
        container.layer?.masksToBounds = true
        container.addSubview(hostingView)

        NSLayoutConstraint.activate([
            hostingView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            hostingView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            hostingView.topAnchor.constraint(equalTo: container.topAnchor),
            hostingView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])

        containerView = container
        self.view = container
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        // Set background color
        view.layer?.backgroundColor = AppColors.contentBackground.cgColor
    }

    // MARK: - Divider Hit Passthrough

    func setLeadingDividerPassthroughWidth(_ width: CGFloat) {
        let normalized = max(0, width)
        containerView?.leadingPassthroughWidth = normalized
        NSLog("🧭 [DividerDebug] contentLeadingPassthroughWidth=%.1f", normalized)
    }
}

// MARK: - SwiftUI Content Area

/// SwiftUI view for main content area with XPC notification handling
private struct ContentAreaView: View {
    @ObservedObject var viewModel: BrowserViewModel
    @ObservedObject var historyVM: HistoryViewModel
    @ObservedObject var bookmarkVM: BookmarkViewModel
    @ObservedObject var devTools: DevToolsManager = .shared

    @State private var hasHandledLaunchURL = false
    @State private var hasAppliedDebugConfig = false

    let cornerRadius: CGFloat

    var body: some View {
        WorkspaceView(devTools: devTools, viewModel: viewModel) {
            WorkspaceMainView(
                viewModel: viewModel,
                historyVM: historyVM,
                bookmarkVM: bookmarkVM,
                useNativeToolbar: true
            )
        }
        .onAppear {
            // Inject historyVM for ACTR app history recording
            viewModel.historyVM = historyVM
            // Check for pending group transfer
            checkPendingGroupTransfer()
            // Handle launch URL (only once)
            handleLaunchURLIfNeeded()
            // Apply debug configuration (only once)
            applyDebugConfigIfNeeded()
        }
        // Handle bookmark open from XPC service
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.openBookmark)) { notification in
            guard let userInfo = notification.userInfo,
                  let urlString = userInfo["url"] as? String else { return }
            viewModel.navigateCurrentTab(to: urlString)

            // Mark bookmark as used if we can find it
            if let bookmarkId = userInfo["bookmarkId"] as? String,
               let uuid = UUID(uuidString: bookmarkId),
               let bookmark = bookmarkVM.bookmarks.first(where: { $0.id == uuid }) {
                bookmarkVM.markAsUsed(bookmark)
            }
        }
        // Handle history item open from XPC service
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.openHistoryItem)) { notification in
            guard let userInfo = notification.userInfo,
                  let urlString = userInfo["url"] as? String else { return }
            viewModel.navigateCurrentTab(to: urlString)
        }
        // Handle bookmark delete from XPC service
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.deleteBookmark)) { notification in
            guard let userInfo = notification.userInfo,
                  let bookmarkId = userInfo["bookmarkId"] as? String,
                  let uuid = UUID(uuidString: bookmarkId) else { return }
            bookmarkVM.removeBookmark(id: uuid)
        }
        // Handle history item delete from XPC service
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.deleteHistoryItem)) { notification in
            guard let userInfo = notification.userInfo,
                  let itemId = userInfo["itemId"] as? String,
                  let uuid = UUID(uuidString: itemId),
                  let item = historyVM.recentItems.first(where: { $0.id == uuid }) else { return }
            historyVM.deleteItem(item)
        }
        // Handle clear history from XPC service
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.clearHistory)) { _ in
            historyVM.clearHistory()
        }
        // Handle internal URI navigation (e.g., actro://bookmarks/tags/{tagId})
        .onReceive(NotificationCenter.default.publisher(for: NSNotification.Name("com.actro.openURI"))) { notification in
            guard let userInfo = notification.userInfo,
                  let uri = userInfo["uri"] as? String else { return }

            if uri.hasPrefix("actro://bookmarks") {
                _ = viewModel.openSystemTab(.bookmarks)
                if let tagId = userInfo["tagId"] as? String,
                   let tagName = userInfo["tagName"] as? String {
                    NSLog("🏷️ Opening bookmarks with tag filter: \(tagName) (\(tagId))")
                }
            }
        }
        // Handle URL open from Start Page (XPC service)
        .onReceive(NotificationCenter.default.publisher(for: XPCNotificationBridge.openURLFromStartPage)) { notification in
            guard let userInfo = notification.userInfo,
                  let urlString = userInfo["url"] as? String else { return }
            viewModel.navigateCurrentTab(to: urlString)
        }
    }

    // MARK: - Pending Group Transfer

    private func checkPendingGroupTransfer() {
        guard let groupData = UserDefaults.standard.dictionary(forKey: "pendingGroupTransfer"),
              let tabsData = groupData["tabs"] as? [[String: Any]],
              !tabsData.isEmpty else {
            return
        }

        // Clear the pending transfer immediately
        UserDefaults.standard.removeObject(forKey: "pendingGroupTransfer")

        let groupName = groupData["groupName"] as? String ?? ""
        let colorRaw = groupData["groupColor"] as? String ?? "gray"
        let color = TabGroupColor(rawValue: colorRaw) ?? .gray

        // Create tabs from serialized data
        var newTabIds: [UUID] = []
        for tabData in tabsData {
            let title = tabData["title"] as? String ?? "Untitled"
            let urlString = tabData["urlString"] as? String
            let url = urlString.flatMap { URL(string: $0) }
            let systemTypeRaw = tabData["systemTabType"] as? String
            let systemTabType = systemTypeRaw.flatMap { SystemTabType(rawValue: $0) }

            let tab = Tab(
                title: title,
                url: url,
                appId: tabData["appId"] as? String,
                legacyActorBridge: tabData["legacyActorBridge"] as? Bool ?? false,
                htmlContent: tabData["htmlContent"] as? String,
                systemTabType: systemTabType,
                guestScript: tabData["guestScript"] as? String,
                leftPanelScript: tabData["leftPanelScript"] as? String,
                rightPanelScript: tabData["rightPanelScript"] as? String,
                unifiedScript: tabData["unifiedScript"] as? String,
                capabilityManifestJSON: tabData["capabilityManifestJSON"] as? String,
                contractName: tabData["contractName"] as? String,
                contractVersion: tabData["contractVersion"] as? Int,
                permissions: tabData["permissions"] as? [String] ?? [],
                rightPanelDefaultVisible: tabData["rightPanelDefaultVisible"] as? Bool ?? false,
                backgroundColor: tabData["backgroundColor"] as? String
            )

            viewModel.tabs.append(tab)
            viewModel.tabSlotPositions[tab.id] = viewModel.tabs.count - 1
            newTabIds.append(tab.id)
        }

        // Set current tab to first imported tab
        if let firstId = newTabIds.first {
            viewModel.currentTabId = firstId
        }

        // Create group from imported tabs
        if !newTabIds.isEmpty {
            viewModel.createGroupFromTabs(newTabIds, name: groupName, color: color)
        }
    }

    // MARK: - Launch URL Handling

    private func handleLaunchURLIfNeeded() {
        guard !hasHandledLaunchURL,
              let launchURL = ActriumApp.launchURL else { return }
        hasHandledLaunchURL = true

        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            let tab = viewModel.addNewTab(focusAddressBar: false)
            viewModel.navigateCurrentTab(to: launchURL.absoluteString)

            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                viewModel.actriumMode = .focus(tabId: tab.id)
            }
        }
    }

    // MARK: - Debug Config

    private func applyDebugConfigIfNeeded() {
        guard !hasAppliedDebugConfig else { return }
        hasAppliedDebugConfig = true

        let config = ActriumApp.debugConfig
        guard config.hasDebugOptions else { return }

        NSLog("🔧 [Debug] Applying debug configuration...")

        if let frame = config.windowFrame {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                if let window = NSApp.windows.first(where: { $0.isKeyWindow || $0.isMainWindow }) {
                    window.setFrame(frame, display: true, animate: false)
                    NSLog("🔧 [Debug] Window frame set to: \(frame)")
                }
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            if let tabType = config.tabType {
                switch tabType {
                case "startPage":
                    _ = viewModel.openSystemTab(.startPage)
                case "history":
                    _ = viewModel.openSystemTab(.history)
                case "bookmarks":
                    _ = viewModel.openSystemTab(.bookmarks)
                default:
                    NSLog("🔧 [Debug] Unknown tab type: \(tabType)")
                }
            }

            if let mode = config.mode {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    switch mode {
                    case "focus":
                        if let currentTab = viewModel.currentTab {
                            viewModel.actriumMode = .focus(tabId: currentTab.id)
                        }
                    case "overview":
                        viewModel.actriumMode = .overview
                    default:
                        NSLog("🔧 [Debug] Unknown mode: \(mode)")
                    }
                }
            }
        }
    }
}
