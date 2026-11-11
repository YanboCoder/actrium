import SwiftUI
import WebKit

private let logger = AppLogger.ui
import AppKit
import UniformTypeIdentifiers

// MARK: - Notification Names

extension Notification.Name {
    static let toggleFocusOverviewRequested = Notification.Name("toggleFocusOverviewRequested")
}

// MARK: - Main View

/// Large canvas structure - Unified Actrium View
///
/// Core concepts:
/// - All Tab The cards are all on a large canvas, fixed.
/// - Overview Mode: narrow canvas to show all card grids
/// - Focus Mode: Magnify the canvas to fill the screen with the target card
/// - Only pass scale + offset Convert Implementation Mode Switch
struct WorkspaceMainView: View {
    @ObservedObject var viewModel: BrowserViewModel
    @ObservedObject var historyVM: HistoryViewModel
    @ObservedObject var bookmarkVM: BookmarkViewModel
    var useNativeToolbar: Bool = false  // Use original NSToolbar Hide SwiftUI toolbar

    // Viewport size from WorkspaceMainViewport (via Environment)
    @Environment(\.viewportSize) private var viewportSize

    @State private var isACTRDropTarget: Bool = false
    @State private var showingBookmarkSheet: Bool = false

    // Toolbar Status
    @State private var addressInputText: String = ""
    @State private var isAddressBarEditing: Bool = false
    @State private var isAddressBarFirstResponder: Bool = false
    @State private var keyEventMonitor: Any?
    @State private var mouseEventMonitor: Any?
    @State private var magnifyEventMonitor: Any?
    @State private var cumulativeMagnification: CGFloat = 0

    // Web loading state
    @State private var isWebLoading: Bool = false

    // Search Tabs Status
    @State private var searchTabsText: String = ""
    @FocusState private var isSearchTabsFocused: Bool
    @State private var shouldAutoFocusSearchTabs: Bool = false

    // Find in Page Status
    @State private var showFindBar: Bool = false
    @State private var findText: String = ""
    @State private var findResultCount: Int = 0
    @State private var findCurrentIndex: Int = 0
    @FocusState private var isFindBarFocused: Bool
    // Use shared singleton to ensure setOverviewMode works correctly
    private var viewBridgeCoordinatorManager: ViewBridgeCoordinatorManager { ViewBridgeCoordinatorManager.shared }

    // Mode Switch Transition controller
    @StateObject private var modeTransitionController = ModeTransitionController()

    // 🔑 Drag State (from) TabGridView Raise, Jean. UnifiedReactNativeLayer I can also visit)
    @State private var draggingTabId: UUID?
    @State private var dropTargetTabId: UUID?  // Current drop target It's... tab ID

    // Filter by Searching Text tabs
    private var filteredTabs: [Tab] {
        guard !searchTabsText.isEmpty else {
            return viewModel.tabs
        }
        return viewModel.tabs.filter { tab in
            tab.title.localizedCaseInsensitiveContains(searchTabsText)
        }
    }

    // For Filtered tabs Create a new position map (rearranged from 0 Start continuous)
    private var filteredTabPositions: [UUID: Int] {
        guard !searchTabsText.isEmpty else {
            return viewModel.tabSlotPositions
        }
        var newPositions: [UUID: Int] = [:]
        for (index, tab) in filteredTabs.enumerated() {
            newPositions[tab.id] = index
        }
        return newPositions
    }

    var body: some View {
        // viewportSize comes from Environment (set by WorkspaceMainViewport)
        let _ = {
            NSLog("📐 [VIEWPORT] viewportSize=%.1fx%.1f, mode=%@",
                  viewportSize.width, viewportSize.height, String(describing: viewModel.actriumMode))
        }()

        // Safe viewport size (avoid zero/small dimensions that cause layout issues)
        // Use a greater minimum value to avoid extreme width ratio cardHeight It's huge.
        // When? viewportSize.width=0 I'm not sure if I'm going to be able to do this.cardAspectRatio=1/100=0.01, cardHeight=120/0.01=12000
        // It's gonna lead to... ViewBridge It's... NSServiceViewControllerWindow Layout loop error
        let safeViewportSize = CGSize(
            width: max(viewportSize.width, 100),
            height: max(viewportSize.height, 100)
        )

        // Toolbar height estimation (for grid layout calculations)
        let toolbarHeight: CGFloat = 52
        let contentSize = safeViewportSize
            // Titlebar removed, card ratio = Portrait ratio
            let titleBarRatio: CGFloat = 0
            let cardAspectRatio = safeViewportSize.width / safeViewportSize.height
            let gridResult = gridLayout(
                for: contentSize,
                cardAspectRatio: cardAspectRatio,
                cardCount: filteredTabs.count
            )
            let actriumSize = gridResult.actriumSize

            // Calculate Changes
            // Use safeViewportSize Avoid viewportSize=0 Result scale=0.001
            // scale=0.001 It's gonna cause... ViewBridge Window calculates extreme size
            let transform: (scale: CGFloat, offset: CGPoint) = {
                switch viewModel.actriumMode {
                case .overview:
                    return calculateOverviewTransform(
                        actriumSize: actriumSize,
                        viewportSize: safeViewportSize
                    )
                case .focus(let tabId):
                    return calculateFocusTransform(
                        for: tabId,
                        gridResult: gridResult,
                        viewportSize: safeViewportSize,
                        titleBarRatio: titleBarRatio
                    )
                }
            }()

            ZStack(alignment: .topTrailing) {
                // Safari Style: Deep Grey Content Regional Background (not extended to toolbar)
                Color(nsColor: NSColor(white: 0.12, alpha: 1.0))

                // Actrium View - Include All Tab Card
                // ViewBridge Always. Actrium Inside, smooth animation through snapshots
                TabGridView(
                    tabs: filteredTabs,
                    tabSlotPositions: filteredTabPositions,
                    positions: gridResult.positions,
                    cardWidth: gridResult.cardWidth,
                    cardHeight: gridResult.cardHeight,
                    titleBarRatio: titleBarRatio,
                    toolbarHeight: toolbarHeight,
                    currentScale: transform.scale,
                    mode: viewModel.actriumMode,
                    showNewTabButton: searchTabsText.isEmpty,
                    viewportSize: safeViewportSize,
                    viewBridgeCoordinatorManager: viewBridgeCoordinatorManager,
                    bookmarkVM: bookmarkVM,
                    historyVM: historyVM,
                    // ━━━━ Mode Switching Trigger #3: Card Click ━━━━
                    onSelectTab: { tabId in
                        enterFocus(tabId: tabId)
                    },
                    onSwapPositions: { tabId1, tabId2 in
                        viewModel.swapSlotPositions(tabId1: tabId1, tabId2: tabId2)
                    },
                    onNewTab: {
                        viewModel.addNewTab()
                    },
                    onCloseTab: { tabId in
                        withAnimation {
                            viewModel.closeTab(id: tabId)
                        }
                    },
                    onNavigate: { tabId, url in
                        // Navigate the tab to the URL from Start Page
                        viewModel.currentTabId = tabId
                        viewModel.navigateCurrentTab(to: url.absoluteString)
                        enterFocus(tabId: tabId)
                    },
                    draggingTabId: $draggingTabId,
                    dropTargetTabId: $dropTargetTabId
                )
                .frame(width: actriumSize.width, height: actriumSize.height)
                // Prevention scale Yes 0 It leads to a strange matrix.
                .scaleEffect(max(transform.scale, 0.001), anchor: .topLeading)
                .offset(x: transform.offset.x, y: transform.offset.y)
                // 🔑 Disable SwiftUI Default Animation, by ModeTransitionController Manual Control
                .animation(nil, value: viewModel.actriumMode)
                .contentShape(Rectangle())
                // The address bar exit focus has been wired by global mouse events (mouseEventMonitor)

                // Focus Under Mode Native Content Layer (By)actrium Changed)
                // Native SwiftUI views need to render at viewportSize in Focus mode,
                // otherwise they would be scaled up (blurry) by actrium transform
                if case .focus(let focusedTabId) = viewModel.actriumMode,
                   let focusedTab = viewModel.tabs.first(where: { $0.id == focusedTabId }),
                   case .native(let nativeType) = focusedTab.contentType {
                    switch nativeType {
                    case .startPage:
                        StartPageView(
                            bookmarks: bookmarkVM.bookmarks,
                            recentHistory: historyVM.recentItems,
                            containerSize: safeViewportSize,
                            isCompactMode: false,
                            onNavigate: { url in
                                viewModel.navigateCurrentTab(to: url.absoluteString)
                            }
                        )
                        .frame(width: safeViewportSize.width, height: safeViewportSize.height)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    }
                }

                // Focus Under Mode Web Content Layer (By)actrium Changed)
                // scaleEffect Yeah. NSViewRepresentable It's not working, so... Web Content needs independent rendering
                if case .focus(let focusedTabId) = viewModel.actriumMode,
                   let focusedTab = viewModel.tabs.first(where: { $0.id == focusedTabId }),
                   case .web(let url) = focusedTab.contentType {
                    ZStack(alignment: .top) {
                        FocusedWebOverlay(
                            tab: focusedTab,
                            url: url,
                            viewportSize: safeViewportSize,
                            onNavigationFinished: { title, newURL in
                                // Update tab title and record history
                                if let title = title, !title.isEmpty {
                                    viewModel.updateTabTitle(for: focusedTabId, title: title)
                                }
                                if let newURL = newURL {
                                    viewModel.updateTabURL(for: focusedTabId, url: newURL)
                                    // Record to history
                                    if let tab = viewModel.tabs.first(where: { $0.id == focusedTabId }) {
                                        historyVM.recordVisit(tab: tab)
                                    }
                                }
                                // Update snapshot for Overview mode (delayed to allow rendering)
                                Task {
                                    try? await Task.sleep(nanoseconds: 500_000_000) // 0.5s delay
                                    await WebViewHolderManager.shared.takeSnapshot(for: focusedTabId)
                                }
                            },
                            onURLChanged: { newURL in
                                // Update tab URL when page uses History.pushState/replaceState
                                if let newURL = newURL {
                                    NSLog("🔗 WorkspaceMainView: Updating tab \(focusedTabId) URL to: \(newURL.absoluteString)")
                                    viewModel.updateTabURL(for: focusedTabId, url: newURL)
                                }
                            },
                            onLoadingStateChanged: { loading in
                                isWebLoading = loading
                                viewModel.isCurrentTabLoading = loading
                            },
                            onOpenNewWindow: { newURL in
                                // Open target="_blank" links in a new tab (don't focus address bar)
                                let newTab = viewModel.addNewTab(focusAddressBar: false)
                                viewModel.navigateCurrentTab(to: newURL.absoluteString)
                                // Enter focus mode for the new tab
                                enterFocus(tabId: newTab.id)
                            },
                            onThemeColorChanged: { color in
                                viewModel.updateTabBackgroundColor(for: focusedTabId, color: color)
                            },
                            onProgressChanged: { progress in
                                viewModel.loadingProgress = progress
                            }
                        )
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

                        // Safari-style loading progress bar
                        if viewModel.isCurrentTabLoading {
                            GeometryReader { geo in
                                Rectangle()
                                    .fill(Color.accentColor)
                                    .frame(width: geo.size.width * viewModel.loadingProgress, height: 2)
                                    .animation(.linear(duration: 0.2), value: viewModel.loadingProgress)
                            }
                            .frame(height: 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .transition(.opacity)
                        }

                        SavePasswordBannerView()
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                            .padding(12)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }

                // 🔑 Live Layer: Unified ViewBridge Content Layer (By)actrium Changed)
                // Always replicating, whatever. Focus/Overview Mode, avoid disconnection when mode switching
                // Note:Live The floor. actrium External, needs to be actrium Coordinate to Port of Vision
                // 🔑 Use Fixed frame It's... ZStack Limit the coordinates, or Father ZStack It's... actriumSize It's gonna cause... .position() Error
                ZStack(alignment: .topLeading) {
                    ForEach(viewModel.tabs.filter { tab in
                        if case .reactNative = tab.contentType { return true }
                        return false
                    }) { tab in
                        if case .reactNative(let fingerprint) = tab.contentType {
                            let isFocused = {
                                if case .focus(let focusedId) = viewModel.actriumMode {
                                    return focusedId == tab.id
                                }
                                return false
                            }()

                            let isOverviewMode = {
                                if case .overview = viewModel.actriumMode {
                                    return true
                                }
                                return false
                            }()

                            // Calculating Cards in actrium Position and dimensions in the coordinates system
                            let slotPosition = filteredTabPositions[tab.id] ?? 0
                            let actriumCardTopLeft = slotPosition < gridResult.positions.count
                                ? gridResult.positions[slotPosition]
                                : .zero
                            let actriumCardSize = CGSize(
                                width: gridResult.cardWidth,
                                height: gridResult.cardHeight
                            )

                            // 🔑 Will actrium Coordinate to Port of Vision
                            // Live The floor. actrium External. It needs manual application. actrium transform
                            let viewportCardPosition = CGPoint(
                                x: actriumCardTopLeft.x * transform.scale + transform.offset.x,
                                y: actriumCardTopLeft.y * transform.scale + transform.offset.y
                            )
                            let viewportCardSize = CGSize(
                                width: actriumCardSize.width * transform.scale,
                                height: actriumCardSize.height * transform.scale
                            )

                            UnifiedReactNativeLayer(
                                tab: tab,
                                fingerprint: fingerprint,
                                viewportSize: safeViewportSize,
                                cardSize: viewportCardSize,
                                cardPosition: viewportCardPosition,
                                isFocused: isFocused,
                                isOverviewMode: isOverviewMode,
                                isTransitioning: modeTransitionController.phase.isTransitioning,
                                isDragSource: draggingTabId == tab.id,
                                isDropTarget: dropTargetTabId == tab.id,
                                coordinator: viewBridgeCoordinatorManager.coordinator(for: tab.id),
                                bookmarks: bookmarksForRN,
                                recentHistory: recentHistoryForRN
                            )
                        }
                    }
                }
                .frame(width: safeViewportSize.width, height: safeViewportSize.height)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .clipped()

                // Search Tabs Search Box - Only Overview Mode Display
                // Put it on. AnimationLayer Before, let the animation cover it.
                if case .overview = viewModel.actriumMode {
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass")
                            .foregroundColor(.secondary)
                            .font(.system(size: 14))

                        TextField("Search Tabs", text: $searchTabsText)
                            .textFieldStyle(.plain)
                            .frame(width: 150)
                            .focused($isSearchTabsFocused)
                            .onAppear {
                                if shouldAutoFocusSearchTabs {
                                    shouldAutoFocusSearchTabs = false
                                    isSearchTabsFocused = true
                                }
                            }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(Color(nsColor: NSColor.controlBackgroundColor).opacity(0.9))
                    .cornerRadius(8)
                    .padding(.top, 10)
                    .padding(.trailing, 10)
                }

                // 🔑 Animation Layer:VirtualGrid Programme
                // And the truth. Grid The layout is completely consistent with the virtual grid, using Tab Thumbnail
                // Overwrite reality during animation Grid and Search Tabs, hide after finished
                // Note: Actual change by ModeTransitionController Pass. CVDisplayLink Driver
                // Note:actriumSize I have to. TabGridView It's... frame Unanimously (in millions of dollars)gridResult.actriumSize)
                AnimationLayerRepresentable(
                    controller: modeTransitionController,
                    actriumSize: actriumSize,  // Use gridResult.actriumSizewith TabGridView Unanimously.
                    cardSize: CGSize(width: gridResult.cardWidth, height: gridResult.cardHeight),
                    cards: buildVirtualGridCards(
                        tabs: filteredTabs,
                        positions: gridResult.positions,
                        tabSlotPositions: filteredTabPositions,
                        cardWidth: gridResult.cardWidth,
                        cardHeight: gridResult.cardHeight
                    ),
                    isVisible: modeTransitionController.phase.isTransitioning
                )
                .frame(width: safeViewportSize.width, height: safeViewportSize.height)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .allowsHitTesting(false)
                // 🔑 Ensure that the animation layer is above all other content (including Live Layer NSView)
                .zIndex(1000)

                // Glass View Cover processing drag (transparent layer,hitTest Back nil ♪ Let the click through ♪
                GlassViewOnlyRepresentable(
                    onDropACTR: { url in
                        Task { @MainActor in
                            if case .overview = viewModel.actriumMode {
                                searchTabsText = ""
                            }

                            // Check if it's a directory or .actr file
                            var isDirectory: ObjCBool = false
                            if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) {
                                if isDirectory.boolValue {
                                    viewModel.loadFromDevDirectory(url)
                                } else {
                                    viewModel.importACTRPackage(from: url)
                                }
                            }

                            // ━━━━ Mode Switching Trigger #5: Import ACTR ━━━━
                            if let newTabId = viewModel.currentTabId {
                                enterFocus(tabId: newTabId)
                            }
                        }
                    }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)

                // Toolbar Bottom segment line (top layer, crossbar)
                VStack {
                    Rectangle()
                        .fill(Color(nsColor: NSColor.separatorColor))
                        .frame(height: 1)
                    Spacer()
                }
                .allowsHitTesting(false)
            }
            .toolbar {
                // Use original NSToolbar Hide SwiftUI toolbar Contents
                if !useNativeToolbar {
                    // Left: Navigation button
                    ToolbarItemGroup(placement: .navigation) {
                    Button(action: { viewModel.goBack() }) {
                        Image(systemName: "chevron.left")
                    }
                    .disabled(!viewModel.canGoBack)
                    .help("Back")

                    Button(action: { viewModel.goForward() }) {
                        Image(systemName: "chevron.right")
                    }
                    .disabled(!viewModel.canGoForward)
                    .help("Forward!")
                }

                // Middle: Address Bar ()50% viewportSize width, but limit maximum width to keep it medium)
                ToolbarItem(placement: .principal) {
                    // Left-button group around 100px, needs to be reserved space to stay in the center
                    // Maximum width to centreable = viewportSize.width - 2 * The width of the larger side button
                    let sideButtonsWidth: CGFloat = 140
                    let maxCenterableWidth = max(200, viewportSize.width - 2 * sideButtonsWidth)
                    // Minimum width 200Make sure the address bar is always visible
                    let addressBarWidth = max(200, min(viewportSize.width * 0.5, maxCenterableWidth))

                    if case .overview = viewModel.actriumMode {
                        Color.clear
                            .frame(width: addressBarWidth, height: 28)
                    } else {
                        let leftIconState: AddressBarLeftIconState = {
                            guard let tab = viewModel.currentTab else { return .none }
                            switch tab.contentType {
                            case .reactNative: return .menu
                            case .web: return isWebLoading ? .loading : .none
                            case .native, .empty: return .none
                            }
                        }()

                        let currentURLString = viewModel.currentTab?.url?.absoluteString ?? ""

                        let displayTitle: String = {
                            guard let tab = viewModel.currentTab else { return "New Tab" }
                            if case .web = tab.contentType, let host = tab.url?.host {
                                return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
                            }
                            return tab.title
                        }()

                        AddressBarContainer(
                            titleText: .constant(displayTitle),
                            urlText: currentURLString,
                            inputText: $addressInputText,
                            isEditing: $isAddressBarEditing,
                            isFirstResponder: $isAddressBarFirstResponder,
                            isBookmarked: viewModel.currentTab.map { bookmarkVM.isBookmarked(tab: $0) } ?? false,
                            leftIconState: leftIconState,
                            toggleLeftPanel: {
                                withAnimation(.easeInOut(duration: 0.2)) {
                                    viewModel.toggleLeftPanelForCurrentTab()
                                }
                                // Send visibility change to XPC service
                                let tabId: UUID? = {
                                    if case .focus(let focusTabId) = viewModel.actriumMode {
                                        return focusTabId
                                    }
                                    return viewModel.currentTabId
                                }()
                                if let tabId = tabId {
                                    let newVisible = viewModel.isLeftPanelVisible(for: tabId)
                                    let coordinator = viewBridgeCoordinatorManager.coordinator(for: tabId)
                                    coordinator.sendLeftPanelVisibilityChanged(newVisible)
                                }
                            },
                            showingBookmarkPopover: $showingBookmarkSheet,
                            shouldAutoFocus: $viewModel.shouldAutoFocusAddressBar,
                            refreshAction: { viewModel.reloadCurrentTab() },
                            onNavigate: { input in
                                viewModel.navigateCurrentTab(to: input)
                                isAddressBarFirstResponder = false
                            },
                            bookmarkPopoverContent: {
                                if let tab = viewModel.currentTab {
                                    let existingBookmark = bookmarkVM.getBookmark(for: tab)
                                    BookmarkEditSheet(
                                        viewModel: bookmarkVM,
                                        tab: existingBookmark == nil ? tab : nil,
                                        existingBookmark: existingBookmark
                                    )
                                }
                            }
                        )
                        .frame(width: addressBarWidth)
                    }
                }

                // Right: Operation button
                ToolbarItemGroup(placement: .primaryAction) {
                    DownloadToolbarButton()

                    if case .focus = viewModel.actriumMode,
                       let currentTab = viewModel.currentTab,
                       currentTab.url != nil {
                        ShareToolbarButton(url: currentTab.url)
                        QRCodeToolbarButton(url: currentTab.url)
                    }

                    Button(action: {
                        withAnimation(.spring(response: 0.35, dampingFraction: 1.0, blendDuration: 0)) {
                            _ = viewModel.addNewTab()
                        }
                    }) {
                        Image(systemName: "plus")
                    }
                    .help("New Tab")

                    Button(action: toggleFocusOverview) {
                        Image(systemName: "square.on.square")
                    }
                    .help("Tab Overview (⌘⇧\\)")
                }
                }  // end if !useNativeToolbar
            }
        // 🔑 Treatment of raw toolbar button to switch the mode request (through ModeTransitionController)
        .onReceive(NotificationCenter.default.publisher(for: .toggleFocusOverviewRequested)) { _ in
            toggleFocusOverview()
        }
        .onChange(of: viewModel.actriumMode) { oldValue, newValue in
            // From Focus Switch to Overview , first screen the current page
            if case .focus(let focusedTabId) = oldValue, case .overview = newValue {
                if let tab = viewModel.tabs.first(where: { $0.id == focusedTabId }) {
                    switch tab.contentType {
                    case .web:
                        // Web tab: Stepshot
                        Task {
                            await WebViewHolderManager.shared.takeSnapshot(for: focusedTabId)
                        }
                    case .reactNative:
                        // ViewBridge tab: Synchronize screenshots
                        ViewBridgeHostViewManager.shared.takeSnapshot(for: focusedTabId)
                    case .native, .empty:
                        // Native SwiftUI and empty tabs don't need snapshots
                        break
                    }
                }
            }

            // Enter Overview The tags need automatic focus search boxes when mode is ticked
            if case .overview = newValue {
                shouldAutoFocusSearchTabs = true
            } else {
                // Leave Overview Empty search text while mode
                searchTabsText = ""
                isSearchTabsFocused = false
            }
            // Note: isOverviewMode By ViewBridgeCoordinatorManager Subscriptions actriumMode AutoSync
        }
        // shouldAutoFocusAddressBar Move processing to AddressBarContainer It's... onAppear Medium
        // This will only be triggered when the address bar is actually rendered, without delay.
        .onAppear {
            // Setup native titlebar toggle callback
            // 🔑 Send Notification By view Deal with it, make sure it's passed. ModeTransitionController Transition
            TitlebarManager.shared.onToggleFocusOverview = {
                NotificationCenter.default.post(name: .toggleFocusOverviewRequested, object: nil)
            }

            // 🔑 Tie XPCBridgeService Present. ViewModel(Concentration management)
            // XPCBridgeService Subscriptions actriumMode Change, AutoSync isOverviewMode
            XPCBridgeService.shared.bindToViewModel(viewModel)
            // And keep it together. ViewBridgeCoordinatorManager binding (reward compatibility, gradual migration)
            ViewBridgeCoordinatorManager.shared.bindToViewModel(viewModel)

            // ━━━━ Mode Switching Trigger #1: Keyboard Shortcuts ━━━━
            // Cmd+Shift+\ (keyCode 42) → Organisation → toggleFocusOverview()
            // 🔑 Important: Not directly callable toggleFocusOverview(), because closure The catch is the old one. View Status
            // It's necessary to notify him. .onReceive Yes. SwiftUI Call in update cycle to ensure use of the latest available viewportSize
            // See MARK: - Mode Switching (Unified entrance)
            keyEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                let cmdShift: NSEvent.ModifierFlags = [.command, .shift]
                if event.keyCode == 42 && event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(cmdShift) {
                    // Send a notification, use the same code path as the original toolbar button
                    NotificationCenter.default.post(name: .toggleFocusOverviewRequested, object: nil)
                    return nil
                }

                // Cmd+F → Toggle find bar (only in Focus mode with web tab)
                if event.keyCode == 3 && event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(.command) {
                    if case .focus(let tabId) = viewModel.actriumMode,
                       let tab = viewModel.tabs.first(where: { $0.id == tabId }),
                       case .web = tab.contentType {
                        showFindBar.toggle()
                        if showFindBar {
                            isFindBarFocused = true
                        } else {
                            findText = ""
                            // Clear find highlights
                            if let holder = WebViewHolderManager.shared.existingHolder(for: tabId) {
                                holder.webView.evaluateJavaScript("window.getSelection().removeAllRanges()")
                            }
                        }
                        return nil
                    }
                }

                // Escape → Close find bar
                if event.keyCode == 53 && showFindBar {
                    showFindBar = false
                    findText = ""
                    // Clear find highlights
                    if case .focus(let tabId) = viewModel.actriumMode {
                        if let holder = WebViewHolderManager.shared.existingHolder(for: tabId) {
                            holder.webView.evaluateJavaScript("window.getSelection().removeAllRanges()")
                        }
                    }
                    return nil
                }

                return event
            }

            // Exiting address bar edit status when clicking on the main window area
            mouseEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { event in
                // Announcements LiquidGlassToolbar Unfocus (for native titlebar)
                let shouldNotifyDismiss: Bool = {
                    if useNativeToolbar {
                        if let window = event.window ?? NSApp.keyWindow,
                           let contentView = window.contentView {
                            let pointInContent = contentView.convert(event.locationInWindow, from: nil)
                            return contentView.bounds.contains(pointInContent)
                        }
                        return false
                    } else {
                        return true
                    }
                }()

                if shouldNotifyDismiss {
                    NotificationCenter.default.post(name: NSNotification.Name("com.actro.dismissAddressBarFocus"), object: nil)
                }

                // Old logic:SwiftUI toolbar Address Bar
                if !useNativeToolbar && isAddressBarEditing {
                    // Delay implementation to allow the system to process the clicks first
                    // Just set isFirstResponder=false, onChange You will exit editing mode automatically
                    DispatchQueue.main.async {
                        isAddressBarFirstResponder = false
                    }
                }
                return event
            }

            // ━━━━ Mode Switching Trigger #4: Double finger. ━━━━
            // - ACTR/RN Contents: By ActriumViewService End-processing and sending DistributedNotification
            // - Web Contents: By Local magnify Event listening device processing
            magnifyEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .magnify) { [self] event in
                guard case .focus = viewModel.actriumMode else { return event }

                switch event.phase {
                case .began:
                    cumulativeMagnification = 0
                case .changed:
                    cumulativeMagnification += event.magnification
                    // Pinch-in (zoom out) threshold to enter Overview
                    if cumulativeMagnification < -0.3 {
                        cumulativeMagnification = 0
                        fputs("🤏 Local magnify threshold reached → exitToOverview()\n", stderr)
                        exitToOverview()
                    }
                case .ended, .cancelled:
                    cumulativeMagnification = 0
                default:
                    break
                }
                return event
            }
        }
        .onDisappear {
            // Clear event listening
            if let monitor = keyEventMonitor {
                NSEvent.removeMonitor(monitor)
            }
            if let monitor = mouseEventMonitor {
                NSEvent.removeMonitor(monitor)
            }
            if let monitor = magnifyEventMonitor {
                NSEvent.removeMonitor(monitor)
            }
        }
        // ━━━━ Mode Switching Trigger #4: Double finger. (Distributive Notifications Interception) ━━━━
        // See MARK: - Mode Switching (Unified entrance)
        .onReceive(
            DistributedNotificationCenter.default().publisher(for: Notification.Name("com.actro.pinchToOverview"))
        ) { _ in
            guard case .focus = viewModel.actriumMode else { return }
            fputs("🤏 Received pinchToOverview → exitToOverview()\n", stderr)
            exitToOverview()
        }
        // ━━━━ Package direct-link download sheets (15-B B1) ━━━━
        .sheet(item: $viewModel.pendingPackageDownload) { pending in
            PackageDownloadConfirmSheet(
                pending: pending,
                onConfirm: { viewModel.confirmPendingPackageDownload() },
                onCancel: { viewModel.cancelPendingPackageDownload() }
            )
        }
        // ━━━━ Instance creation confirm sheet (15-B B6) ━━━━
        .sheet(item: $viewModel.pendingInstanceCreation) { pending in
            PackageInstanceConfirmSheet(
                pending: pending,
                onConfirm: { viewModel.confirmInstanceCreation() },
                onCancel: { viewModel.cancelInstanceCreation() }
            )
        }
        .sheet(item: $viewModel.activePackageDownload) { active in
            ActivePackageDownloadSheet(
                active: active,
                onRetry: { viewModel.retryActivePackageDownload() },
                onCancel: { viewModel.cancelActivePackageDownload() }
            )
        }
        .sheet(item: $viewModel.actrUriNotice) { notice in
            ActrUriNoticeSheet(notice: notice) {
                viewModel.actrUriNotice = nil
            }
        }
    }

    // MARK: - Mode Switching (Unified entrance)
    //
    // ┌─────────────────────────────────────────────────────────────────────┐
    // │                   Focus ↔ Overview Mode Switch Structure                       │
    // ├─────────────────────────────────────────────────────────────────────┤
    // │ Trigger Method:                                                            │
    // │   1. Cmd+Shift+\    - Shortcut keys,Toggle Switch                            │
    // │   2. Toolbar button    - Click on the grid icon,Toggle Switch                       │
    // │   3. Card Click        - Overview Click on card entry in mode Focus               │
    // │   4. Double finger.        - Focus Crushing the gestures under the mode to exit to Overview              │
    // │                      (By ActriumViewService End-processing and distribution notifications)     │
    // │   5. Import ACTR       - Auto-Entry after Import Focus Mode                       │
    // ├─────────────────────────────────────────────────────────────────────┤
    // │ Core Functions (All switches must be adopted by one of the following:):                                    │
    // │   - toggleFocusOverview() : Toggle Switch                              │
    // │   - enterFocus(tabId:)    : Enter Focus Mode                          │
    // │   - exitToOverview()      : Quit To Overview Mode                      │
    // ├─────────────────────────────────────────────────────────────────────┤
    // │ Uniform: Wreckproof.(0.5s)Animated,lastFocusedTabId Records, Logs                │
    // └─────────────────────────────────────────────────────────────────────┘

    /// Last time. mode Switch time, use it for the shake-off.
    @State private var lastModeChangeTime: Date = .distantPast

    /// Last time. focused It's... tab ID, for quick return
    @State private var lastFocusedTabId: UUID?

    /// Toggle Switch Focus <-> Overview Mode
    /// - From Focus → Overview: Remember the current tabSwitch to Overview
    /// - From Overview → Focus: Switch to determineFocusTarget() Sure. tab
    private func toggleFocusOverview() {
        switch viewModel.actriumMode {
        case .focus:
            exitToOverview()
        case .overview:
            if let targetTabId = determineFocusTarget() {
                enterFocus(tabId: targetTabId)
            }
        }
    }

    /// Enter Focus Mode
    /// - Parameter tabId: Yes. focus It's... tab ID
    /// - Parameter skipDebounce: Whether to skip the tremors (for initialization only)
    private func enterFocus(tabId: UUID, skipDebounce: Bool = false) {
        // Already on target. Focus Status, no switch
        guard viewModel.actriumMode != .focus(tabId: tabId) else { return }

        // Transition ongoing, ignored
        guard !modeTransitionController.phase.isTransitioning else {
            fputs("🟢 enterFocus() IGNORED (transition in progress)\n", stderr)
            return
        }

        // Wret protection: Ignore 0.5 Repeating Switch in seconds
        if !skipDebounce {
            let now = Date()
            guard now.timeIntervalSince(lastModeChangeTime) > 0.5 else {
                fputs("🟢 enterFocus() DEBOUNCED (too fast)\n", stderr)
                return
            }
            lastModeChangeTime = now
        }

        fputs("🟢 enterFocus(\(tabId.uuidString.prefix(8)))\n", stderr)

        // Last recorded focused It's... tab
        lastFocusedTabId = tabId

        // Sync Settings currentTabId, make sure the address bar and content are the same tab
        viewModel.currentTabId = tabId

        // 🔑 Calculates the position of the card in the eye Zoom Animation)
        let safeViewportSize = CGSize(
            width: max(viewportSize.width, 100),
            height: max(viewportSize.height, 100)
        )
        let cardAspectRatio = safeViewportSize.width / safeViewportSize.height
        let gridResult = gridLayout(
            for: safeViewportSize,
            cardAspectRatio: cardAspectRatio,
            cardCount: filteredTabs.count
        )

        // Calculating Cards in actrium Coordinate System rect(VirtualGrid Use actrium Coordinates)
        let slotIndex = filteredTabPositions[tabId] ?? 0
        let actriumCardPosition = slotIndex < gridResult.positions.count
            ? gridResult.positions[slotIndex]
            : .zero
        let actriumCardRect = CGRect(
            x: actriumCardPosition.x,
            y: actriumCardPosition.y,
            width: gridResult.cardWidth,
            height: gridResult.cardHeight
        )

        // 🔑 Fetch from Cache Overview Quickshot exitToOverview , and then the card size is taken from the card.
        // Note: Do not intercept viewport, because Overview Mode viewport The whole grid is shown, not a single card
        let overviewSnapshot = TabSnapshotCacheManager.shared.overviewSnapshot(for: tabId)

        // 🔑 Calculate Current Overview Mode transform(For animation starting point)
        let overviewTransform = calculateOverviewTransform(
            actriumSize: gridResult.actriumSize,
            viewportSize: safeViewportSize
        )

        // 🔑 Use ModeTransitionController Manage transition animations
        // Pass actrium Coordinate cardRectwith VirtualGrid Coordinates are consistent.
        // Note:actriumMode Switch after animation is completed, avoiding a trigger during animation SwiftUI Redraw
        modeTransitionController.startOverviewToFocusTransition(
            tabId: tabId,
            cardRect: actriumCardRect,
            viewportSize: safeViewportSize,
            overviewTransform: overviewTransform,
            overviewSnapshot: overviewSnapshot
        ) { [weak viewModel] in
            // Switch after animation actriumMode
            viewModel?.actriumMode = .focus(tabId: tabId)
        }
    }

    /// Quit To Overview Mode
    private func exitToOverview() {
        // Remember the current focused It's... tab(before switching)
        guard case .focus(let currentTabId) = viewModel.actriumMode else {
            // Already. Overview Mode
            return
        }
        lastFocusedTabId = currentTabId

        // Transition ongoing, ignored
        guard !modeTransitionController.phase.isTransitioning else {
            fputs("🔴 exitToOverview() IGNORED (transition in progress)\n", stderr)
            return
        }

        // Wret protection: Ignore 0.5 Repeating Switch in seconds
        let now = Date()
        guard now.timeIntervalSince(lastModeChangeTime) > 0.5 else {
            fputs("🔴 exitToOverview() DEBOUNCED (too fast)\n", stderr)
            return
        }
        lastModeChangeTime = now

        fputs("🔴 exitToOverview()\n", stderr)

        // 🔑 Intercept Current Focus Under Mode Tab Content snapshot (large chart)
        // This is... Focus → Overview Animation Starts
        // Note:applyBlur=false Ensure that the settings are synchronized and are available immediately from the start of the animation
        let focusSnapshot = captureViewportSnapshot(viewportSize: viewportSize)
        if let snapshot = focusSnapshot {
            TabSnapshotCacheManager.shared.setFocusSnapshot(snapshot, for: currentTabId, applyBlur: false)

            // Setup simultaneously Overview Quickshot (Scalp to Card Size)
            // Count the card size first.
            let safeViewportSizeForCard = CGSize(
                width: max(viewportSize.width, 100),
                height: max(viewportSize.height, 100)
            )
            let cardAspectRatio = safeViewportSizeForCard.width / safeViewportSizeForCard.height
            let gridResultForCard = gridLayout(
                for: safeViewportSizeForCard,
                cardAspectRatio: cardAspectRatio,
                cardCount: filteredTabs.count
            )
            let cardSize = CGSize(width: gridResultForCard.cardWidth, height: gridResultForCard.cardHeight)
            // applyBlur=false Ensure Sync settings
            TabSnapshotCacheManager.shared.setOverviewSnapshot(snapshot, for: currentTabId, targetSize: cardSize, applyBlur: false)
        }

        // 🔑 Calculating Cards in Overview Position in the lower viewer of mode (used for Zoom Animation)
        let safeViewportSize = CGSize(
            width: max(viewportSize.width, 100),
            height: max(viewportSize.height, 100)
        )
        let cardAspectRatio = safeViewportSize.width / safeViewportSize.height
        let gridResult = gridLayout(
            for: safeViewportSize,
            cardAspectRatio: cardAspectRatio,
            cardCount: filteredTabs.count
        )

        // Calculating Cards in actrium Coordinate System rect(VirtualGrid Use actrium Coordinates)
        let slotIndex = filteredTabPositions[currentTabId] ?? 0
        let actriumCardPosition = slotIndex < gridResult.positions.count
            ? gridResult.positions[slotIndex]
            : .zero
        let actriumCardRect = CGRect(
            x: actriumCardPosition.x,
            y: actriumCardPosition.y,
            width: gridResult.cardWidth,
            height: gridResult.cardHeight
        )

        // 🔑 Calculate Overview Mode transform(For animated endpoints)
        let overviewTransform = calculateOverviewTransform(
            actriumSize: gridResult.actriumSize,
            viewportSize: safeViewportSize
        )

        // 🔑 Use ModeTransitionController Manage transition animations
        // Note:actriumMode Switch after animation is completed, avoiding a trigger during animation SwiftUI Redraw
        // It's with enterFocus() A consistent model
        modeTransitionController.startFocusToOverviewTransition(
            tabId: currentTabId,
            cardRect: actriumCardRect,
            viewportSize: safeViewportSize,
            overviewTransform: overviewTransform,
            overviewSnapshot: nil  // VirtualGrid Programme uses thumbnails, no cut-off
        ) { [weak viewModel] in
            // Switch after animation actriumMode
            viewModel?.actriumMode = .overview
        }
    }

    /// Intercept current view snapshots (for transition animation)
    /// Only the view area, excluding the sidebar and the toolbar
    /// Optimization: Direct access to view areas to avoid cutting through the entire window
    /// - Parameter viewportSize: Portage Dimensions
    private func captureViewportSnapshot(viewportSize: CGSize) -> NSImage? {
        let startTime = CACurrentMediaTime()

        guard let window = NSApp.keyWindow ?? NSApp.mainWindow,
              let contentView = window.contentView else {
            return nil
        }

        // Calculating the area of the view in the content view of the window
        let windowSize = contentView.bounds.size
        let sidebarWidth = windowSize.width - viewportSize.width

        // contentView Possible Include toolbar (Height differential = toolbar Height)
        // Need to be based on contentView The coordinates are the right angle. y Location
        let toolbarHeight = windowSize.height - viewportSize.height
        let isFlipped = contentView.isFlipped

        // Exclude Top 1px It's... toolbar Separator
        let separatorHeight: CGFloat = 1

        // I'm a visualist. y Coordinates:
        // - Not flipped (Bottom Left Point of Origin): y = 0 (The eye is at the bottom.), the top segment is to be removed. height
        // - flipped (Top Left Point of Origin): y = toolbarHeight + 1 (Skip the partition line)
        let viewportY: CGFloat = isFlipped ? (toolbarHeight + separatorHeight) : 0
        let captureHeight: CGFloat = viewportSize.height - separatorHeight

        let captureRect = CGRect(
            x: sidebarWidth,
            y: viewportY,
            width: viewportSize.width,
            height: captureHeight
        )

        NSLog("📸 captureViewportSnapshot: windowSize=%.0fx%.0f, viewportSize=%.0fx%.0f, toolbarHeight=%.0f, isFlipped=%d, captureRect=(%.0f,%.0f,%.0f,%.0f)",
              windowSize.width, windowSize.height,
              viewportSize.width, viewportSize.height,
              toolbarHeight, isFlipped ? 1 : 0,
              captureRect.origin.x, captureRect.origin.y, captureRect.width, captureRect.height)

        // Creates a direct access area bitmap
        guard let bitmapRep = contentView.bitmapImageRepForCachingDisplay(in: captureRect) else {
            return nil
        }

        let cacheTime = CACurrentMediaTime()
        contentView.cacheDisplay(in: captureRect, to: bitmapRep)
        let captureTime = CACurrentMediaTime()

        // Create Full viewportSize , and stretch full when drawing (excluded) 1px Almost invisible)
        let image = NSImage(size: viewportSize)
        image.lockFocus()
        if let ctx = NSGraphicsContext.current {
            ctx.imageInterpolation = .high
            bitmapRep.draw(in: NSRect(origin: .zero, size: viewportSize))
        }
        image.unlockFocus()

        let endTime = CACurrentMediaTime()
        NSLog("📸 captureViewportSnapshot: %.1fms (cache: %.1fms)",
              (endTime - startTime) * 1000,
              (captureTime - cacheTime) * 1000)

        return image
    }

    /// Sure. Focus Objective tab
    /// Priority: Last focused It's... tab > Current Selected tab > The only tab
    /// If no clear objectives are identified (multiple) tab But I can't tell which one to choose. nil
    private func determineFocusTarget() -> UUID? {
        let tabs = viewModel.tabs

        // No, I don't. tabI can't. focus
        guard !tabs.isEmpty else { return nil }

        // Only one. tabDirectly. focus It's...
        if tabs.count == 1 {
            return tabs.first?.id
        }

        // Last time. focused It's... tab It's still there. Return it.
        if let lastId = lastFocusedTabId, tabs.contains(where: { $0.id == lastId }) {
            return lastId
        }

        // Current Selected tab There, back it up.
        if let currentId = viewModel.currentTabId, tabs.contains(where: { $0.id == currentId }) {
            return currentId
        }

        // Multiple tab But there's no clear target. It's not automatic. focus
        return nil
    }

    // MARK: - Find in Page

    /// Perform find in current web page
    private func performFind(text: String, forward: Bool) {
        guard !text.isEmpty,
              case .focus(let tabId) = viewModel.actriumMode,
              let holder = WebViewHolderManager.shared.existingHolder(for: tabId) else {
            return
        }

        // Use WKWebView's find API
        let config = WKFindConfiguration()
        config.backwards = !forward
        config.wraps = true

        holder.webView.find(text, configuration: config) { [self] result in
            Task { @MainActor in
                self.findResultCount = result.matchFound ? 1 : 0  // WKFindResult doesn't expose count
                // Note: WKWebView's find API doesn't expose the match count or current index
                // For a more complete implementation, JavaScript-based find would be needed
            }
        }
    }

    /// Clear find highlights
    private func clearFindHighlights() {
        guard case .focus(let tabId) = viewModel.actriumMode,
              let holder = WebViewHolderManager.shared.existingHolder(for: tabId) else {
            return
        }

        // Clear selection
        holder.webView.evaluateJavaScript("window.getSelection().removeAllRanges()")
    }

    // Judgement Tab Need any? ViewBridge Rendering Contents
    private func hasRNContentForTab(_ tab: Tab) -> Bool {
        tab.systemTabType != nil ||
        tab.unifiedScript != nil ||
        tab.leftPanelScript != nil ||
        tab.guestScript != nil
    }

    // MARK: - Data Conversion for React Native

    /// Convert bookmarks to dictionary array for RN layer
    private var bookmarksForRN: [[String: Any]] {
        bookmarkVM.bookmarks.map { bookmark in
            [
                "id": bookmark.id.uuidString,
                "title": bookmark.title,
                "appURL": bookmark.appURL?.absoluteString ?? "",
                "createdAt": Int(bookmark.createdAt.timeIntervalSince1970 * 1000)
            ] as [String: Any]
        }
    }

    /// Convert recent history to dictionary array for RN layer
    private var recentHistoryForRN: [[String: Any]] {
        historyVM.recentItems.map { item in
            [
                "id": item.id.uuidString,
                "title": item.title,
                "appURL": item.appURL?.absoluteString ?? "",
                "visitTime": Int(item.visitTime.timeIntervalSince1970 * 1000)
            ] as [String: Any]
        }
    }

    // MARK: - Transform Calculations

    /// Overview Mode change: Scale the canvas to fit the view, and centered it
    private func calculateOverviewTransform(
        actriumSize: CGSize,
        viewportSize: CGSize
    ) -> (scale: CGFloat, offset: CGPoint) {
        guard actriumSize.width > 0 && actriumSize.height > 0 else {
            return (1.0, .zero)
        }

        // Scale: Jean Actrium Adaptive vision (no zoom in, only shrink in)
        let scaleX = viewportSize.width / actriumSize.width
        let scaleY = viewportSize.height / actriumSize.height
        let scale = max(min(scaleX, scaleY, 1.0), 0.001)

        // Centred
        let scaledWidth = actriumSize.width * scale
        let scaledHeight = actriumSize.height * scale
        let offsetX = (viewportSize.width - scaledWidth) / 2
        let offsetY = (viewportSize.height - scaledHeight) / 2

        guard offsetX.isFinite && offsetY.isFinite && scale.isFinite else {
            return (1.0, .zero)
        }

        return (scale, CGPoint(x: offsetX, y: offsetY))
    }

    /// Focus Mode change: Magnify the canvas to fill the target card with a visual window
    private func calculateFocusTransform(
        for tabId: UUID,
        gridResult: (positions: [CGPoint], cardWidth: CGFloat, cardHeight: CGFloat, actriumSize: CGSize),
        viewportSize: CGSize,
        titleBarRatio: CGFloat
    ) -> (scale: CGFloat, offset: CGPoint) {
        guard let position = viewModel.tabSlotPositions[tabId],
              position < gridResult.positions.count else {
            return (1.0, .zero)
        }

        let cardPosition = gridResult.positions[position]
        let cardFullRect = CGRect(
            x: cardPosition.x,
            y: cardPosition.y,
            width: gridResult.cardWidth,
            height: gridResult.cardHeight
        )

        // Card content area (leaf title bar)
        // Use imported titleBarRatio, the force multiplier is already included.
        let titleBarHeight = cardFullRect.height * titleBarRatio
        let cardContentRect = CGRect(
            x: cardFullRect.origin.x,
            y: cardFullRect.origin.y + titleBarHeight,
            width: cardFullRect.width,
            height: cardFullRect.height - titleBarHeight
        )

        guard cardContentRect.width > 0 else {
            return (1.0, .zero)
        }

        // Scale: keep the card content area width = The width of the view
        let scale = max(viewportSize.width / cardContentRect.width, 0.001)
        NSLog("📐 [FOCUS] viewportSize=%.1fx%.1f, cardRect=%.1fx%.1f, scale=%.3f",
              viewportSize.width, viewportSize.height, cardContentRect.width, cardContentRect.height, scale)

        // Offset: Align Top Left
        let offsetX = -cardContentRect.origin.x * scale
        let offsetY = -cardContentRect.origin.y * scale

        guard offsetX.isFinite && offsetY.isFinite && scale.isFinite else {
            return (1.0, .zero)
        }

        return (scale, CGPoint(x: offsetX, y: offsetY))
    }

    // MARK: - Grid Layout Calculations

    /// Aligning title bar height (inOverview Shows on top of the card in mode)- Use shared styles
    private var floatingTitleBarHeight: CGFloat { CardStyle.titleBarHeight }

    /// Calculate spacing (between cards) and vertical margins
    // MARK: - VirtualGrid Helper

    /// Build VirtualGrid Card Info Numerator
    /// - Parameters:
    ///   - tabs: Tab List
    ///   - positions: Grid Layout Locations
    ///   - tabSlotPositions: Tab ID Map to slot index
    /// - Returns: VirtualGridCardInfo Array
    /// [+] Button Fixed UUID
    private static let addButtonUUID = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!

    /// Cache [+] Button Image and its Dimensions (avoid each time) body Recreated during assessment)
    private static var cachedAddButtonImage: (image: NSImage, size: CGSize)?

    private func buildVirtualGridCards(
        tabs: [Tab],
        positions: [CGPoint],
        tabSlotPositions: [UUID: Int],
        cardWidth: CGFloat,
        cardHeight: CGFloat
    ) -> [VirtualGridCardInfo] {
        let cacheManager = TabSnapshotCacheManager.shared

        var cards = tabs.compactMap { tab -> VirtualGridCardInfo? in
            guard let slotIndex = tabSlotPositions[tab.id],
                  slotIndex < positions.count else {
                return nil
            }

            // From TabSnapshotCache Get a double buffer.
            let focusSnapshot = cacheManager.focusSnapshot(for: tab.id)
            let overviewSnapshot = cacheManager.overviewSnapshot(for: tab.id)

            return VirtualGridCardInfo(
                tabId: tab.id,
                position: positions[slotIndex],
                focusSnapshot: focusSnapshot,
                overviewSnapshot: overviewSnapshot,
                title: tab.title
            )
        }

        // Add [+] Button Card (at last position)
        // [+] Buttons without title bar
        let addButtonIndex = tabs.count
        if addButtonIndex < positions.count {
            let addButtonImage = createAddButtonImage(width: cardWidth, height: cardHeight)
            cards.append(VirtualGridCardInfo(
                tabId: Self.addButtonUUID,
                position: positions[addButtonIndex],
                focusSnapshot: addButtonImage,
                overviewSnapshot: addButtonImage,
                title: ""  // [+] Button Untitled
            ))
        }

        return cards
    }

    /// Create [+] Button Images (Choose NewTabPanel )
    /// Use static caches to recreate only when size changes
    /// - Parameters:
    ///   - width: Card width
    ///   - height: Card height
    /// - Returns: Image after Rendering
    private func createAddButtonImage(width: CGFloat, height: CGFloat) -> NSImage {
        let size = CGSize(width: width, height: height)

        // Check Cache: If size equal, return the cache image directly
        if let cached = Self.cachedAddButtonImage,
           abs(cached.size.width - size.width) < 1 && abs(cached.size.height - size.height) < 1 {
            return cached.image
        }

        // Create new image
        let image = NSImage(size: size)
        image.lockFocus()

        let rect = NSRect(origin: .zero, size: size)
        let cornerRadius = CardStyle.cornerRadius

        // Background: Dark grey (Match NSColor(white: 0.15, alpha: 1.0))
        let backgroundPath = NSBezierPath(roundedRect: rect, xRadius: cornerRadius, yRadius: cornerRadius)
        NSColor(white: 0.15, alpha: 1.0).setFill()
        backgroundPath.fill()

        // Border: White 0.15 Transparency
        NSColor.white.withAlphaComponent(0.15).setStroke()
        backgroundPath.lineWidth = 1.0
        backgroundPath.stroke()

        // + No. : Usage SF Symbol "plus"
        // Calculate + Size: with reality NewTabPanel It's... 48pt Font Size Proportional
        // Real cards are usually about 300px Wing,48pt About 16%
        let plusSize = min(width, height) * 0.16
        let plusConfig = NSImage.SymbolConfiguration(pointSize: plusSize, weight: .regular)
        if let plusImage = NSImage(systemSymbolName: "plus", accessibilityDescription: nil)?
            .withSymbolConfiguration(plusConfig) {
            // Centred Drawing
            let plusRect = NSRect(
                x: (size.width - plusImage.size.width) / 2,
                y: (size.height - plusImage.size.height) / 2,
                width: plusImage.size.width,
                height: plusImage.size.height
            )
            // Set White 0.5 Transparency
            NSColor.white.withAlphaComponent(0.5).set()
            plusImage.draw(in: plusRect, from: .zero, operation: .sourceOver, fraction: 0.5)
        }

        image.unlockFocus()

        // Cache newly created images
        Self.cachedAddButtonImage = (image: image, size: size)

        return image
    }

    // MARK: - Spacing Calculation

    /// - Horizontal Space = windowWidth / (Number of cards × 3) × 2.5Lower limit 40px(2.5x Original value (USD)
    /// - Vertical Space = Base Space + Altitude of the suspended title bar (space for the title bar)
    /// - Vertical Margins = Base Space × 1.25 + Aligning title bar height
    private func calculateSpacing(windowWidth: CGFloat, totalCards: Int) -> (hSpacing: CGFloat, vSpacing: CGFloat, verticalMargin: CGFloat) {
        let effectiveCards = max(totalCards, 4)
        // Base spacing for vertical direction and margin calculations
        let baseSpacing = max(16, windowWidth / (CGFloat(effectiveCards) * 7))
        // Horizontal spacing (lower limit to make the card larger)
        let hSpacing = max(25, baseSpacing * 2.0)
        let vSpacing = baseSpacing + floatingTitleBarHeight  // Vertical spacing contains the height of the titlebar
        // Top margin: Include title bar height + Small extra space avoided and Search Tabs Overlap
        let verticalMargin = baseSpacing * 1.25 + floatingTitleBarHeight + 15
        return (hSpacing, vSpacing, verticalMargin)
    }

    /// Grid layout calculation
    /// - Initial capacity 4(2×2) Number of cards ≤ 4 Force 2×2 Layout
    /// - Horizontal Margin = spacing × 1.25(Ensures by card size)
    private func gridLayout(
        for size: CGSize,
        cardAspectRatio: CGFloat,
        cardCount: Int
    ) -> (positions: [CGPoint], cardWidth: CGFloat, cardHeight: CGFloat, actriumSize: CGSize) {
        let totalCards = cardCount + 1  // Organisation [+] button
        let minCardWidth: CGFloat = 120  // Prevents the card width of windows to be as low as 0 It causes the transformation matrix to deteriorate.

        guard cardAspectRatio.isFinite && cardAspectRatio > 0 else {
            return gridLayout(for: size, cardAspectRatio: 16.0 / 9.0, cardCount: cardCount)
        }
        guard totalCards > 0 else {
            return ([], 0, 0, .zero)
        }

        let initialCapacity = 4
        let isForced2x2 = totalCards <= initialCapacity
        let (hSpacing, vSpacing, verticalMargin) = calculateSpacing(windowWidth: size.width, totalCards: max(totalCards, initialCapacity))

        // Compute Best Layout
        func calculateOptimalLayout() -> (columns: Int, rows: Int, cardWidth: CGFloat, cardHeight: CGFloat) {
            var bestColumns = 1
            var bestCardWidth: CGFloat = 0
            var bestCardHeight: CGFloat = 0

            let minColumns = isForced2x2 ? 2 : 1
            let maxColumns = isForced2x2 ? 2 : totalCards

            for columns in minColumns...maxColumns {
                let rows = isForced2x2 ? 2 : Int(ceil(Double(totalCards) / Double(columns)))

                // Horizontal bound: Margins = 1.25 × hSpacing
                // cardWidth = (width - (columns + 1.5) × hSpacing) / columns
                let cardWidthFromH = (size.width - CGFloat(columns + 1) * hSpacing - 0.5 * hSpacing) / CGFloat(columns)

                // Vertical bound (use) vSpacing, with suspended title bar space)
                let availableHeight = size.height - verticalMargin * 2
                let cardHeightFromV = (availableHeight - CGFloat(rows - 1) * vSpacing) / CGFloat(rows)
                let cardWidthFromV = cardHeightFromV * cardAspectRatio

                let cardWidth = min(cardWidthFromH, cardWidthFromV)
                let cardHeight = cardWidth / cardAspectRatio

                if cardWidth > bestCardWidth {
                    bestColumns = columns
                    bestCardWidth = cardWidth
                    bestCardHeight = cardHeight
                }
            }

            let rows = isForced2x2 ? 2 : Int(ceil(Double(totalCards) / Double(bestColumns)))
            return (bestColumns, rows, bestCardWidth, bestCardHeight)
        }

        let layout = calculateOptimalLayout()
        let columns = layout.columns
        let rows = layout.rows
        // When Window is too narrow cardWidth Could be closer to 0, resulting in scale Matrix Odd;Land Bottom
        let cardWidth = max(layout.cardWidth, minCardWidth)
        let cardHeight = cardWidth / cardAspectRatio

        // 2×2 Keep while forced layout 4 Position
        let positionCount = isForced2x2 ? initialCapacity : totalCards

        // Horizontal margin: Fill the width of the view mouth and eliminate middle deviation
        let horizontalMargin = max((size.width - CGFloat(columns) * cardWidth - CGFloat(columns - 1) * hSpacing) / 2, 0)

        // Calculating card position (use vSpacing As line spacing)
        var positions: [CGPoint] = []
        for index in 0..<positionCount {
            let col = index % columns
            let row = index / columns
            let x = horizontalMargin + CGFloat(col) * (cardWidth + hSpacing)
            let y = verticalMargin + CGFloat(row) * (cardHeight + vSpacing)
            positions.append(CGPoint(x: x, y: y))
        }

        // Actrium Width = viewport Width
        let actriumSize = CGSize(
            width: size.width,
            height: verticalMargin * 2 + CGFloat(rows) * cardHeight + CGFloat(rows - 1) * vSpacing
        )

        return (positions, cardWidth, cardHeight, actriumSize)
    }

}

// MARK: - Actrium View

private struct TabGridView: View {
    let tabs: [Tab]
    let tabSlotPositions: [UUID: Int]
    let positions: [CGPoint]
    let cardWidth: CGFloat
    let cardHeight: CGFloat
    let titleBarRatio: CGFloat  // Headbar height as a percentage of card height
    let toolbarHeight: CGFloat  // toolbar Actual height
    let currentScale: CGFloat
    let mode: ActriumMode
    let showNewTabButton: Bool  // Whether to show [+] button (hidden while searching for filters)
    let viewportSize: CGSize    // Visual dimensions for RN Render
    let viewBridgeCoordinatorManager: ViewBridgeCoordinatorManager
    @ObservedObject var bookmarkVM: BookmarkViewModel
    @ObservedObject var historyVM: HistoryViewModel
    let onSelectTab: (UUID) -> Void
    let onSwapPositions: (UUID?, UUID?) -> Void
    let onNewTab: () -> Void
    let onCloseTab: (UUID) -> Void
    var onNavigate: ((UUID, URL) -> Void)?  // tabId, url

    // 🔑 Drag-state binding (from WorkspaceMainView Into, Jean. UnifiedReactNativeLayer I can also visit)
    @Binding var draggingTabId: UUID?
    @Binding var dropTargetTabId: UUID?

    @State private var hoveringOverTabId: UUID?
    @State private var swappingTabId: UUID?  // The card in the exchange (for faded animation)
    @State private var isDraggingActive = false  // Tracks whether drag is ongoing (for inhibiting clicks)

    // Fixed slot index (to be recorded at the start of drag, regardless of tabs (In exchange for change)
    @State private var dragSourceSlotIndexFixed: Int?  // Drag Source a
    @State private var dropTargetSlotIndexFixed: Int?  // Target slot b

    // Build Show Item List (Sorted by Location)
    // [+] Buttons to the last. tabLocation = tabs.count)
    // 2×2 Example layout (in1 tab): Tab - In position. 0, [+] - In position. 1Location 2-3 empty
    private var displayItems: [(position: Int, item: DisplayItem)] {
        var items: [(Int, DisplayItem)] = []

        // Add All tabs
        for tab in tabs {
            if let position = tabSlotPositions[tab.id] {
                items.append((position, .tab(tab)))
            }
        }

        // Sort by Location
        let sorted = items.sorted { $0.0 < $1.0 }

        // [+] Buttons to the last. tab(only shown if not searched)
        var result = sorted
        if showNewTabButton {
            result.append((tabs.count, .newTabButton))
        }

        return result
    }


    var body: some View {
        let _ = NSLog("🔄 [ACTRIUM-BODY] TabGridView.body evaluated: tabs=%d, mode=%@", tabs.count, String(describing: mode))
        ZStack(alignment: .topLeading) {
            // Painting background
            Color.clear

            // First floor: Card layer (mobility, power)
            ForEach(displayItems, id: \.item.id) { displayItem in
                let itemPosition = displayItem.position
                let position = positions[itemPosition]

                switch displayItem.item {
                case .tab(let tab):
                    let isBeingDragged = draggingTabId == tab.id
                    let isHovering = hoveringOverTabId == tab.id
                    let isSwapping = swappingTabId == tab.id

                    TabPanel(
                        tab: tab,
                        cardWidth: cardWidth,
                        cardHeight: cardHeight,
                        titleBarRatio: titleBarRatio,
                        toolbarHeight: toolbarHeight,
                        currentScale: currentScale,
                        isFocused: {
                            if case .focus(let tabId) = mode {
                                return tabId == tab.id
                            }
                            return false
                        }(),
                        isBeingDragged: isBeingDragged,
                        isHovering: isHovering,
                        isSwapping: isSwapping,
                        viewportSize: viewportSize,
                        viewBridgeCoordinator: viewBridgeCoordinatorManager.coordinator(for: tab.id),
                        bookmarkVM: bookmarkVM,
                        historyVM: historyVM,
                        onSelect: {
                            onSelectTab(tab.id)
                        },
                        onDragStart: {
                            // Clear all old state (prevent residues)
                            swappingTabId = nil
                            dropTargetSlotIndexFixed = nil
                            hoveringOverTabId = nil

                            // Sets a new drag state
                            isDraggingActive = true
                            draggingTabId = tab.id
                            dragSourceSlotIndexFixed = tabSlotPositions[tab.id]
                            return NSItemProvider(object: tab.id.uuidString as NSString)
                        },
                        onClose: {
                            onCloseTab(tab.id)
                        },
                        onNavigate: { url in
                            onNavigate?(tab.id, url)
                        }
                    )
                    .zIndex(isBeingDragged ? 1000 : (draggingTabId != nil ? 100 : Double(itemPosition)))
                    .onDrop(of: [.text], delegate: TabSwapDropDelegate(
                        targetTabId: tab.id,
                        draggingTabId: $draggingTabId,
                        hoveringOverTabId: $hoveringOverTabId,
                        dropTargetSlotIndexFixed: $dropTargetSlotIndexFixed,
                        dropTargetTabId: $dropTargetTabId,
                        tabSlotPositions: tabSlotPositions,
                        onSwapPositions: { tabId1, tabId2 in
                            // Mark target card in exchange (trigger faded animation)
                            // Note: At this time, hoveringOverTabId Or is it? tabId2♪ So so ♪ B The card is still translucent.
                            swappingTabId = tabId2

                            // Use animated to execute the exchange (B From b Move To a, 300ms + From 0.5 Dwindle To 1.0)
                            withAnimation(.spring(response: 0.35, dampingFraction: 1.0, blendDuration: 0)) {
                                onSwapPositions(tabId1, tabId2)
                                // Clear hovering Status, let B The card starts to fade.
                                hoveringOverTabId = nil
                            }

                            // Delayed clearance status
                            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                                isDraggingActive = false
                                draggingTabId = nil
                                swappingTabId = nil
                                dragSourceSlotIndexFixed = nil
                                dropTargetSlotIndexFixed = nil
                                dropTargetTabId = nil
                            }
                        }
                    ))
                    .position(
                        x: position.x + cardWidth / 2,
                        y: position.y + cardHeight / 2
                    )

                case .newTabButton:
                    NewTabPanel(
                        cardWidth: cardWidth,
                        cardHeight: cardHeight,
                        onTap: onNewTab
                    )
                    // Process to Drag to NewTab Situation on the card, clean up.
                    .onDrop(of: [.text], delegate: NoOpDropDelegate(
                        draggingTabId: $draggingTabId,
                        hoveringOverTabId: $hoveringOverTabId
                    ))
                    .position(
                        x: position.x + cardWidth / 2,
                        y: position.y + cardHeight / 2
                    )
                }
            }

            // Second level: slot border layer (top level, not covered by cards)
            // Disable all animations: slots and borders should not have any animation effects
            ForEach(0..<positions.count, id: \.self) { slotIndex in
                SlotView(
                    cardWidth: cardWidth,
                    cardHeight: cardHeight,
                    isDragSource: dragSourceSlotIndexFixed == slotIndex,
                    isDropTarget: dropTargetSlotIndexFixed == slotIndex
                )
                .position(
                    x: positions[slotIndex].x + cardWidth / 2,
                    y: positions[slotIndex].y + cardHeight / 2
                )
                .zIndex(10000)  // Super tall. zIndexMake sure the border remains at the top of the top floor, not covered by any card
                .animation(nil, value: dragSourceSlotIndexFixed)
                .animation(nil, value: dropTargetSlotIndexFixed)
                .allowsHitTesting(false)  // Border does not respond to the click, lets the click go through the lower card
            }
        }
        // We're gonna drag and tap the whole place, and we're gonna get to the blank area.
        .onDrop(of: [.text], delegate: CancelDragDropDelegate(
            draggingTabId: $draggingTabId,
            hoveringOverTabId: $hoveringOverTabId
        ))
    }
}

// MARK: - Slot View (Slot View, show the frame when drag)

private struct SlotView: View {
    let cardWidth: CGFloat
    let cardHeight: CGFloat
    let isDragSource: Bool  // Whether to drag source slots (show a dotted border)
    let isDropTarget: Bool  // Whether or not to target slots (show solid line borders)

    var body: some View {
        Rectangle()
            .fill(Color.clear)
            .frame(width: cardWidth, height: cardHeight)
            .overlay(
                Group {
                    if isDragSource {
                        // Drag source: Blue dash
                        RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                            .strokeBorder(
                                Color.blue.opacity(0.6),
                                style: StrokeStyle(lineWidth: 2, dash: [5, 5])
                            )
                    } else if isDropTarget {
                        // Target: Green line
                        RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                            .strokeBorder(
                                Color.green.opacity(0.7),
                                style: StrokeStyle(lineWidth: 2)
                            )
                    }
                }
            )
            .transaction { tx in
                // Disable all hidden animations (replaces abandoned ones) .animation(nil))
                tx.animation = nil
            }
    }
}

// Show item type
private enum DisplayItem {
    case tab(Tab)
    case newTabButton

    // Provide stability IDJean. SwiftUI Can track view moves instead of content replacement
    var id: String {
        switch self {
        case .tab(let tab):
            return tab.id.uuidString
        case .newTabButton:
            return "newTabButton"
        }
    }
}

// MARK: - Tab Swap Drop Delegate

private struct TabSwapDropDelegate: DropDelegate {
    let targetTabId: UUID?
    @Binding var draggingTabId: UUID?
    @Binding var hoveringOverTabId: UUID?
    @Binding var dropTargetSlotIndexFixed: Int?
    @Binding var dropTargetTabId: UUID?  // 🔑 Add: For RN Layer Display Borders
    let tabSlotPositions: [UUID: Int]
    let onSwapPositions: (UUID?, UUID?) -> Void

    func validateDrop(info: DropInfo) -> Bool {
        // Always return true To ensure that performDrop It's being used to clean up the situation.
        // Actual validity check in performDrop In progress
        return draggingTabId != nil
    }

    func performDrop(info: DropInfo) -> Bool {
        guard let draggingId = draggingTabId else {
            return false
        }

        // Drag yourself, not carry out the exchange.
        if let targetId = targetTabId, targetId == draggingId {
            clearAllStates()
            return false
        }

        // Execute exchange (drive)
        // Note:onSwapPositions Reconcile will handle state clean-up and animation.
        onSwapPositions(draggingId, targetTabId)

        // Not here to clear. hoveringOverTabId, because it is necessary to keep a translucent status for the diademing animation
        // hoveringOverTabId Yes. onSwapPositions Clear in the retraced animation block

        return true
    }

    func dropEntered(info: DropInfo) {
        if let targetId = targetTabId,
           let draggingId = draggingTabId,
           targetId != draggingId {  // I'm not sure I'm gonna be able to do that.
            hoveringOverTabId = targetId
            // Update target slot index (following) hover change in target)
            dropTargetSlotIndexFixed = tabSlotPositions[targetId]
            // 🔑 Update dropTargetTabId For RN Layer Border
            dropTargetTabId = targetId
        }
    }

    func dropExited(info: DropInfo) {
        if let targetId = targetTabId {
            if hoveringOverTabId == targetId {
                hoveringOverTabId = nil
                // Clear Target Slot Mark
                dropTargetSlotIndexFixed = nil
                // 🔑 Clear dropTargetTabId
                dropTargetTabId = nil
            }
        }
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        // bug3 Rehabilitation: return .move Operation, avoid displaying duplicate pointers
        return DropProposal(operation: .move)
    }

    func dropSessionDidEnd(_ session: DropSession) {
        // Clear Suspended Status (Present)draggingTabId By onSwapPositions Revert delay clearance)
        hoveringOverTabId = nil
        dropTargetTabId = nil
    }

    private func clearAllStates() {
        draggingTabId = nil
        hoveringOverTabId = nil
        dropTargetSlotIndexFixed = nil
        dropTargetTabId = nil
    }
}

// MARK: - No-Op Drop Delegate (bug3 Fix: Empty area does not respond to drag-and-drop, and maintains original mouse style)

private struct NoOpDropDelegate: DropDelegate {
    @Binding var draggingTabId: UUID?
    @Binding var hoveringOverTabId: UUID?

    func validateDrop(info: DropInfo) -> Bool {
        return draggingTabId != nil
    }

    func performDrop(info: DropInfo) -> Bool {
        draggingTabId = nil
        hoveringOverTabId = nil
        return false
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        return DropProposal(operation: .move)
    }

    func dropSessionDidEnd(_ session: DropSession) {
        draggingTabId = nil
        hoveringOverTabId = nil
    }
}

// MARK: - Cancel Drag Drop Delegate (Blank area drag-and-drop processing)

private struct CancelDragDropDelegate: DropDelegate {
    @Binding var draggingTabId: UUID?
    @Binding var hoveringOverTabId: UUID?

    func validateDrop(info: DropInfo) -> Bool {
        return true
    }

    func performDrop(info: DropInfo) -> Bool {
        draggingTabId = nil
        hoveringOverTabId = nil
        return false
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        return DropProposal(operation: .move)
    }

    func dropSessionDidEnd(_ session: DropSession) {
        draggingTabId = nil
        hoveringOverTabId = nil
    }

    // Note: Not in dropExited Clear state!
    // Because when drags from parent to child view, parent view dropExited It'll be called.
    // This could lead to a emptiness of the state.
}

// MARK: - Address Bar Left Icon State

/// State for address bar left icon
private enum AddressBarLeftIconState {
    case none      // No icon (web tab not loading)
    case loading   // Loading spinner (web tab loading)
    case menu      // Menu icon (RN app tab)
}

/// Left icon view for address bar
private struct AddressBarLeftIcon: View {
    let state: AddressBarLeftIconState
    let action: () -> Void

    @State private var rotation: Double = 0

    var body: some View {
        switch state {
        case .none:
            // Empty space to maintain layout
            Color.clear.frame(width: 16, height: 16)
        case .loading:
            // Spinning progress ring
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(
                    Color.white.opacity(0.7),
                    style: StrokeStyle(lineWidth: 1.2, lineCap: .round)
                )
                .frame(width: 12, height: 12)
                .rotationEffect(.degrees(rotation))
                .onAppear {
                    withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                }
        case .menu:
            // Menu button for RN apps
            Button(action: action) {
                Image(systemName: "list.bullet")
                    .foregroundColor(.white.opacity(0.7))
            }
            .buttonStyle(.borderless)
            .help("Toggle Left Panel")
        }
    }
}

// MARK: - Address Bar Container

private struct AddressBarContainer<BookmarkContent: View>: View {
    @Binding var titleText: String
    var urlText: String  // URL for edit mode
    @Binding var inputText: String
    @Binding var isEditing: Bool
    @Binding var isFirstResponder: Bool
    var isBookmarked: Bool
    var leftIconState: AddressBarLeftIconState
    var toggleLeftPanel: () -> Void
    @Binding var showingBookmarkPopover: Bool
    @Binding var shouldAutoFocus: Bool  // Autofocus sign
    var refreshAction: () -> Void
    var onNavigate: ((String) -> Void)?  // URL Navigational echo
    @ViewBuilder var bookmarkPopoverContent: () -> BookmarkContent

    private let placeholder = "Enter URL (http/https) or app id"
    @State private var focusRingScale: CGFloat = 1.0
    @State private var focusRingOpacity: Double = 0.0

    var body: some View {
        Group {
            if isEditing {
                // Edit Status: Magnifying Mirror + Input Box + Right button
                HStack(spacing: 10) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(Color.white.opacity(0.7))
                        .frame(width: 16, height: 16)

                    FocusableTextField(
                        text: $inputText,
                        placeholder: placeholder,
                        isFirstResponder: $isFirstResponder,
                        onSubmit: { text in
                            onNavigate?(text)
                        }
                    )

                    // Keep Right button
                    Button(action: { showingBookmarkPopover.toggle() }) {
                        Image(systemName: isBookmarked ? "star.fill" : "star")
                            .foregroundColor(isBookmarked ? .yellow : .white.opacity(0.7))
                    }
                    .buttonStyle(.borderless)
                    .help(isBookmarked ? "Edit Bookmark" : "Add Bookmark")
                    .popover(isPresented: $showingBookmarkPopover, arrowEdge: .bottom) {
                        bookmarkPopoverContent()
                    }

                    Button(action: refreshAction) {
                        Image(systemName: "arrow.clockwise")
                            .foregroundColor(.white.opacity(0.7))
                    }
                    .buttonStyle(.borderless)
                    .help("Refresh")
                }
            } else {
                // Non-edited status: left icon + Intermediate title + Right Icon
                HStack(spacing: 10) {
                    AddressBarLeftIcon(state: leftIconState, action: toggleLeftPanel)

                    Text(titleText)
                        .foregroundColor(.white)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            if !isEditing {
                                // Settings isEditing, onChange It's automatic. URL Copy & Focus
                                isEditing = true
                            }
                        }

                    // Star button for bookmarking
                    Button(action: { showingBookmarkPopover.toggle() }) {
                        Image(systemName: isBookmarked ? "star.fill" : "star")
                            .foregroundColor(isBookmarked ? .yellow : .white.opacity(0.7))
                    }
                    .buttonStyle(.borderless)
                    .help(isBookmarked ? "Edit Bookmark" : "Add Bookmark")
                    .popover(isPresented: $showingBookmarkPopover, arrowEdge: .bottom) {
                        bookmarkPopoverContent()
                    }

                    Button(action: refreshAction) {
                        Image(systemName: "arrow.clockwise")
                    }
                    .buttonStyle(.borderless)
                    .help("Refresh")
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity)
        .frame(height: 28)
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(
                    Color(red: 0.18, green: 0.56, blue: 0.96)
                        .opacity(focusRingOpacity),
                    lineWidth: 2.8
                )
                .scaleEffect(focusRingScale)
        )
        .onChange(of: isFirstResponder) { _, newValue in
            if !newValue && isEditing {
                // Exit editing mode when focal is lost
                isEditing = false
            }
        }
        .onChange(of: isEditing) { _, newValue in
            if newValue {
                // Copy when entering edit mode URL and delay setting focus (waiting) TextField Rendering)
                inputText = urlText
                // Safari Style: Shrink from larger blue circles to border
                // 1. Set the initial state (large circle, full visible)
                focusRingScale = 1.2
                focusRingOpacity = 1.0
                // 2. Delay one frame to start a condensing animation to ensure that the initial state is rendered
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.02) {
                    withAnimation(.easeOut(duration: 0.2)) {
                        focusRingScale = 1.0
                    }
                }
                // Delay one frame, make sure. FocusableTextField Rendered
                DispatchQueue.main.async {
                    isFirstResponder = true
                }
            } else {
                withAnimation(.easeOut(duration: 0.15)) {
                    focusRingOpacity = 0.0
                }
            }
        }
        .onAppear {
            // Check if automatic focus is required after address bar render is completed
            if shouldAutoFocus {
                shouldAutoFocus = false
                isEditing = true
            }
        }
    }

    private func endEditing() {
        isEditing = false
        isFirstResponder = false
    }
}

// MARK: - Focusable TextField

private struct FocusableTextField: NSViewRepresentable {
    @Binding var text: String
    var placeholder: String
    @Binding var isFirstResponder: Bool
    var onSubmit: ((String) -> Void)?  // Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to Back to the Back to Back to Back to Back to Back to Back to Back to the Back to Back to Back to Back to Back to Back to Back to the Back to Back to Back to Back to Back to the Back to Back to Back to

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.isBezeled = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
        field.placeholderString = placeholder
        field.delegate = context.coordinator
        field.usesSingleLineMode = true
        field.maximumNumberOfLines = 1
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        // Settings action To Receive Back
        field.target = context.coordinator
        field.action = #selector(Coordinator.handleAction(_:))
        return field
    }

    func updateNSView(_ nsView: NSTextField, context: Context) {
        if nsView.stringValue != text {
            nsView.stringValue = text
        }
        nsView.placeholderString = placeholder

        if isFirstResponder,
           let window = nsView.window,
           window.firstResponder != nsView.currentEditor() {
            window.makeFirstResponder(nsView)
            // Select All Texts
            DispatchQueue.main.async {
                if let editor = nsView.currentEditor() {
                    editor.selectAll(nil)
                }
            }
        }
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: FocusableTextField

        init(_ parent: FocusableTextField) {
            self.parent = parent
        }

        func controlTextDidChange(_ obj: Notification) {
            guard let field = obj.object as? NSTextField else { return }
            // Filter out line break and keep as single line content
            let raw = field.stringValue
            let cleaned = raw
                .components(separatedBy: .newlines)
                .joined(separator: " ")
            if cleaned != raw {
                field.stringValue = cleaned
            }
            parent.text = cleaned
        }

        func controlTextDidEndEditing(_ notification: Notification) {
            parent.isFirstResponder = false
        }

        @objc func handleAction(_ sender: NSTextField) {
            let text = sender.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                parent.onSubmit?(text)
            }
            parent.isFirstResponder = false
        }
    }
}

// MARK: - Tab Card View

private struct TabPanel: View {
    let tab: Tab
    let cardWidth: CGFloat
    let cardHeight: CGFloat
    let titleBarRatio: CGFloat  // Accurate proportion of the height of the title bar
    let toolbarHeight: CGFloat  // toolbar Actual height, used as a scaling baseline
    let currentScale: CGFloat
    let isFocused: Bool
    let isBeingDragged: Bool
    let isHovering: Bool
    let isSwapping: Bool  // Exchangeing (for faded animation)
    let viewportSize: CGSize  // Visual dimensions for RN Render
    let viewBridgeCoordinator: ViewBridgeCoordinator  // Incoming from outside to ensure stability
    @ObservedObject var bookmarkVM: BookmarkViewModel
    @ObservedObject var historyVM: HistoryViewModel
    let onSelect: () -> Void
    let onDragStart: () -> NSItemProvider
    let onClose: () -> Void
    var onNavigate: ((URL) -> Void)?

    @State private var isLocalDragging = false  // Local drag state to suppress click gestures

    // Aligning title bar height - Use shared styles
    private var floatingTitleBarHeight: CGFloat { CardStyle.titleBarHeight }

    var body: some View {
        let _ = NSLog("🔄 [TABCARD-BODY] TabPanel.body: tabId=%@, isFocused=%d", tab.id.uuidString, isFocused ? 1 : 0)

        // Outer dimension: always used cardWidth/cardHeight
        // Actrium The change is in. Focus Zoom down to Mode viewportSize
        // If this is used here viewportSize♪ Will be with actrium scale The superimposed cause over-expanding.
        let panelWidth = cardWidth
        let panelHeight = cardHeight

        // Card body (pure content)
        // Round corner only Overview The pattern shows thatFocus Mode does not round corner
        // Use overlay Place the titlebar so that the titlebar is separate hit test Regional
        contentView
            .frame(width: panelWidth, height: panelHeight)
            .cornerRadius(isFocused ? 0 : CardStyle.cornerRadius)
            // 🔑 Overview Modes Harmonize Border Styles (with NewTabPanel, UnifiedReactNativeLayer (unanimously)
            .overlay {
                if !isFocused {
                    RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                        .stroke(Color.white.opacity(0.15), lineWidth: 1)
                }
            }
            .shadow(color: isFocused ? .clear : Color.black.opacity(0.3), radius: 5, x: 0, y: 2)
            // Only the card's main subject responded. onTapGesture(excluding title bar)
            .contentShape(Rectangle())
            .onTapGesture {
                // Insisting click gestures during drag
                if !isFocused && !isBeingDragged && !isLocalDragging {
                    onSelect()
                }
            }
            // Suspended Title Bar - Use overlay, the title bar is independent above the card hit test Regional
            .overlay(alignment: .top) {
                if !isFocused {
                    floatingTitleBarView
                        .frame(width: cardWidth, height: floatingTitleBarHeight)
                        // Move title bar above card (outside content area)
                        .offset(y: -(floatingTitleBarHeight + CardStyle.titleBarGap))
                }
            }
        // Draged card: hidden (as shown by drag preview)
        .opacity(isBeingDragged ? 0.001 : 1.0)
        // hover Time and transparency, and the exchange is completely non-transparent.
        .opacity({
            if isSwapping {
                return 1.0  // In exchange: fade to total opacity
            } else if isHovering && !isBeingDragged && !isFocused {
                return 0.5  // hover Time-and-Semi-Transparent
            } else {
                return 1.0  // Default is completely non-transparent
            }
        }())
        // The whole card as a drag source (Overview Long by drag)
        .onDrag {
            if isFocused {
                return NSItemProvider()
            }
            // Set local drag state to suppress click gestures
            isLocalDragging = true
            return onDragStart()
        } preview: {
            // Custom drag preview: whole card contents
            dragPreviewView
        }
        // Listen to external drag changes and reset local drag
        .onChange(of: isBeingDragged) { _, newValue in
            if !newValue {
                // Reset local drag status when external drag ends
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                    isLocalDragging = false
                }
            }
        }
    }

    // Suspended Title Bar View (Simplified version, only with title and off buttons)- Use shared styles
    private var floatingTitleBarView: some View {
        CardStyle.floatingTitleBar(title: tab.title, onClose: onClose)
    }

    // Drag Preview View - Show cards + Suspended Title Bar (using shared styles)
    private var dragPreviewView: some View {
        ZStack(alignment: .top) {
            // Card Subject
            ZStack {
                Color(hslString: tab.backgroundColor)
                Image(systemName: "globe")
                    .font(.system(size: 32))
                    .foregroundColor(.white.opacity(0.3))
            }
            .frame(width: cardWidth, height: cardHeight)
            .cornerRadius(CardStyle.cornerRadius)
            .overlay(
                RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                    .stroke(Color.white.opacity(0.3), lineWidth: 1)
            )
            .shadow(color: Color.black.opacity(0.5), radius: 10, x: 0, y: 5)

            // Suspended Title Bar - Use shared styles
            HStack(spacing: CardStyle.titleBarItemSpacing) {
                CardStyle.closeIcon()
                CardStyle.titleText(tab.title)
                Spacer()
            }
            .padding(.horizontal, CardStyle.titleBarHorizontalPadding)
            .frame(width: cardWidth, height: floatingTitleBarHeight)
            .offset(y: -floatingTitleBarHeight - CardStyle.titleBarGap)
        }
    }

    // Size of card contents (size in canvas coordinates)
    private var cardContentSize: CGSize {
        CGSize(width: cardWidth, height: cardHeight)
    }

    // Content Area View - Multi-Purpose
    // Based on Tab.contentType I'm gonna use it. ViewBridge (RN) Or is it? WKWebView (Web)
    private var contentView: some View {
        // ViewBridge XPC Actual rendering size required:
        // - Focus Mode: Use viewportSize♪ Because ♪ XPC The process of independence is replete, not protected. Actrium (As a result of the change)
        // - Overview Mode: Use cardSize
        // Attention: Here. panelSize Yes, it's for you. ViewBridge Render size, no. SwiftUI frame
        let renderSize: CGSize
        if isFocused {
            renderSize = CGSize(width: max(viewportSize.width, 1), height: max(viewportSize.height, 1))
        } else {
            renderSize = CGSize(width: max(cardWidth, 1), height: max(cardHeight, 1))
        }

        let _ = NSLog("📐 TabPanel.contentView: type=%@, isFocused=%d, viewportSize=%.0fx%.0f, cardSize=%.0fx%.0f, renderSize=%.0fx%.0f",
                      String(describing: tab.contentType), isFocused ? 1 : 0,
                      viewportSize.width, viewportSize.height,
                      cardWidth, cardHeight,
                      renderSize.width, renderSize.height)

        return ZStack(alignment: .topLeading) {
            // Bottom Background - Use Tab Random background colour
            // 🔑 React Native Tab in Overview Mode uses transparent background
            //    Because... UnifiedReactNativeLayer Yes. actrium External zIndex(50) Render
            //    It needs to be seen in a transparent context. RN Contents
            //    Border moved to TabPanel body Field Unified Processing
            if case .reactNative = tab.contentType, !isFocused {
                // Overview Mode: Transparent background, RN Contents Display
                Color.clear
            } else {
                Color(hslString: tab.backgroundColor)
            }

            // Multi-state rendering: based on contentType Select Rendering Engine
            switch tab.contentType {
            case .reactNative(let fingerprint):
                // React Native Apply - Use ViewBridge XPC
                // Media Query style of double-layer structure:
                // - Overview Mode:UnifiedReactNativeLayer Yes. actrium External zIndex(50) Render
                //   Content is displayed in a transparent card background
                // - Focus Mode:UnifiedReactNativeLayer Yes. actrium External zIndex(100) Render
                //   Overwrite above card
                OverviewViewBridgeContainer(
                    tab: tab,
                    fingerprint: fingerprint,
                    renderSize: renderSize,
                    isFocused: isFocused,
                    coordinator: viewBridgeCoordinator
                )
                .frame(width: renderSize.width, height: renderSize.height)

            case .web(let url):
                // Web Contents - Use WKWebView
                // Focus Mode by FocusedWebOverlay Independent rendering (in %2)actrium Changed)
                WebViewMultiProcessTabView(
                    tab: tab,
                    url: url,
                    containerSize: renderSize,
                    isFocused: isFocused
                )

            case .native(let nativeType):
                // Native SwiftUI content - rendered in main process, no XPC
                // 🔑 Focus mode: rendered by FocusedNativeOverlay (outside actrium transform)
                // 🔑 Overview mode: rendered here at cardSize (will be shown as thumbnail)
                if isFocused {
                    // Focus mode: placeholder only, actual content rendered by FocusedNativeOverlay
                    Color.clear
                } else {
                    // Overview mode: render at cardSize for thumbnail
                    let nativeSize = CGSize(width: max(cardWidth, 1), height: max(cardHeight, 1))
                    switch nativeType {
                    case .startPage:
                        StartPageView(
                            bookmarks: bookmarkVM.bookmarks,
                            recentHistory: historyVM.recentItems,
                            containerSize: nativeSize,
                            isCompactMode: true,  // Always compact in Overview
                            onNavigate: { url in
                                onNavigate?(url)
                            }
                        )
                        .frame(width: nativeSize.width, height: nativeSize.height)
                    }
                }

            case .empty:
                // Empty placeholder - new tabs now use system startPage
                EmptyView()
            }
        }
        .frame(maxHeight: .infinity)
        // Note: Do not add here .onTapGestureIt'll be with the outside. .onDrag Conflict
        // tap The gesture is... TabPanel body Base Process
    }
}

// MARK: - Close Button View

private struct CloseButtonView: View {
    let iconSize: CGFloat
    let containerWidth: CGFloat
    let onClose: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: {
            onClose()
        }) {
            Image(systemName: "xmark")
                .font(.system(size: iconSize, weight: .semibold))
                .foregroundColor(isHovering ? .white : .white.opacity(0.6))
                .frame(width: iconSize * 1.6, height: iconSize * 1.6)
                .background(
                    Circle()
                        .fill(isHovering ? Color.white.opacity(0.15) : Color.clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(width: containerWidth)
        .onHover { hovering in
            isHovering = hovering
        }
    }
}

// MARK: - New Tab Card View

private struct NewTabPanel: View {
    let cardWidth: CGFloat
    let cardHeight: CGFloat
    var onTap: (() -> Void)? = nil

    var body: some View {
        ZStack {
            Rectangle()
                .fill(Color(nsColor: NSColor(white: 0.15, alpha: 1.0)))

            Image(systemName: "plus")
                .font(.system(size: 48))
                .foregroundColor(.white.opacity(0.5))
        }
        .frame(width: cardWidth, height: cardHeight)
        .cornerRadius(CardStyle.cornerRadius)
        .overlay(
            RoundedRectangle(cornerRadius: CardStyle.cornerRadius)
                .stroke(Color.white.opacity(0.15), lineWidth: 1)
        )
        .shadow(color: Color.black.opacity(0.3), radius: 5, x: 0, y: 2)
        .contentShape(Rectangle())
        .onTapGesture {
            onTap?()
        }
    }
}

// MARK: - Find in Page Bar

private struct FindInPageBar: View {
    @Binding var findText: String
    let resultCount: Int
    let currentIndex: Int
    var isFocused: FocusState<Bool>.Binding
    let onFind: (String) -> Void
    let onFindNext: () -> Void
    let onFindPrevious: () -> Void
    let onClose: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            // Search field
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)

                TextField("Find in page", text: $findText)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .focused(isFocused)
                    .onSubmit {
                        onFind(findText)
                    }
                    .onChange(of: findText) { _, newValue in
                        if !newValue.isEmpty {
                            onFind(newValue)
                        }
                    }

                if !findText.isEmpty {
                    Button(action: {
                        findText = ""
                    }) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 12))
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(Color(nsColor: .controlBackgroundColor))
            .cornerRadius(6)
            .frame(width: 220)

            // Navigation buttons
            Button(action: onFindPrevious) {
                Image(systemName: "chevron.up")
                    .font(.system(size: 12, weight: .medium))
            }
            .buttonStyle(.borderless)
            .disabled(findText.isEmpty)

            Button(action: onFindNext) {
                Image(systemName: "chevron.down")
                    .font(.system(size: 12, weight: .medium))
            }
            .buttonStyle(.borderless)
            .disabled(findText.isEmpty)

            Spacer()

            // Close button
            Button(action: onClose) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
        .shadow(color: .black.opacity(0.15), radius: 4, y: 2)
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .frame(maxWidth: .infinity, alignment: .top)
    }
}

// MARK: - Share Button (System Picker)

/// Share toolbar button using system NSSharingServicePicker
struct ShareToolbarButton: View {
    let url: URL?
    @State private var triggerShare = false

    var body: some View {
        Button(action: { triggerShare.toggle() }) {
            Image(systemName: "square.and.arrow.up")
        }
        .help("Share")
        .background(ShareAnchor(url: url, trigger: $triggerShare))
    }
}

/// Hidden anchor that shows share picker when triggered
private struct ShareAnchor: NSViewRepresentable {
    let url: URL?
    @Binding var trigger: Bool

    func makeNSView(context: Context) -> NSView {
        NSView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        // When trigger changes, show the picker
        if trigger, let url = url {
            DispatchQueue.main.async {
                self.trigger = false
                let picker = NSSharingServicePicker(items: [url])
                picker.show(relativeTo: nsView.bounds, of: nsView, preferredEdge: .minY)
            }
        }
    }
}

// MARK: - QR Code Button

/// QR Code toolbar button with popover
struct QRCodeToolbarButton: View {
    let url: URL?

    @State private var showingPopover = false
    @State private var qrImage: NSImage?

    var body: some View {
        Button(action: { showingPopover.toggle() }) {
            Image("qr-icon")
                .renderingMode(.template)
                .resizable()
                .aspectRatio(contentMode: .fit)
                .frame(width: 16, height: 16)
                .offset(y: 1)
        }
        .help("QR Code")
        .popover(isPresented: $showingPopover, arrowEdge: .bottom) {
            if let url = url {
                VStack(spacing: 12) {
                    if let qrImage = qrImage {
                        Image(nsImage: qrImage)
                            .interpolation(.none)
                            .resizable()
                            .frame(width: 160, height: 160)
                            .background(Color.white)
                            .cornerRadius(8)
                    }

                    Text(url.host ?? url.absoluteString)
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .lineLimit(1)

                    Button("Copy QR Image") {
                        if let qrImage = qrImage {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.writeObjects([qrImage])
                        }
                        showingPopover = false
                    }
                    .buttonStyle(.borderless)
                }
                .padding()
                .onAppear {
                    generateQRCode(for: url)
                }
            }
        }
    }

    private func generateQRCode(for url: URL) {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return }
        let data = url.absoluteString.data(using: .utf8)
        filter.setValue(data, forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")

        guard let ciImage = filter.outputImage else { return }

        let scale = 10.0
        let transform = CGAffineTransform(scaleX: scale, y: scale)
        let scaledImage = ciImage.transformed(by: transform)

        let rep = NSCIImageRep(ciImage: scaledImage)
        let nsImage = NSImage(size: rep.size)
        nsImage.addRepresentation(rep)
        qrImage = nsImage
    }
}

// MARK: - Overview ViewBridge Container

/// Overview Under Mode ViewBridge Seat container
///
/// 🔑 New architecture: no more rendering PersistentViewBridgeView
/// ViewBridge Content by UnifiedReactNativeLayer Yes. actrium Unique rendering
/// This container shows only background colour as position, ensuring that the card has the right visual boundaries
private struct OverviewViewBridgeContainer: View {
    let tab: Tab
    let fingerprint: String
    let renderSize: CGSize
    let isFocused: Bool
    let coordinator: ViewBridgeCoordinator

    var body: some View {
        // 🔑 Key: No Rendering PersistentViewBridgeView
        // ViewBridge Connection by UnifiedReactNativeLayer Keep it up. It's just a seat.
        Group {
            if isFocused {
                // Focus Mode: Transparent placeholder
                Color.clear
            } else {
                // Overview Mode: Transparent placeholder
                // Actual ViewBridge The content is in actrium External UnifiedReactNativeLayer Render
                Color.clear
            }
        }
        .onAppear {
            allocateServiceIfNeeded()
        }
    }

    private func allocateServiceIfNeeded() {
        // Allocation service - By UnifiedReactNativeLayer Use
        guard coordinator.allocatedServiceId == nil else { return }

        do {
            let serviceId = try RendererProcessPool.shared.allocateRenderer(
                for: tab.id,
                fingerprint: fingerprint
            )
            coordinator.allocatedServiceId = serviceId
            coordinator.tabId = tab.id
        } catch {
            NSLog("⚠️ OverviewViewBridgeContainer: Failed to allocate service: \(error)")
        }
    }
}

#Preview {
    WorkspaceMainView(viewModel: BrowserViewModel(), historyVM: HistoryViewModel(), bookmarkVM: BookmarkViewModel())
}
