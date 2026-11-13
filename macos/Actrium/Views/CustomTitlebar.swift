import AppKit
import SwiftUI
import Combine

// MARK: - Notification Names

extension Notification.Name {
    static let toggleSidebarRequested = Notification.Name("toggleSidebarRequested")
}

// MARK: - Toolbar Layout State

/// Shared state for toolbar layout, allowing AppKit to communicate with SwiftUI
@MainActor
final class ToolbarLayoutState: ObservableObject {
    /// Background leading X position (where the black background starts)
    @Published var backgroundLeadingX: CGFloat = 0
    /// Whether sidebar is collapsed
    @Published var isSidebarCollapsed: Bool = false
}

// MARK: - Custom Titlebar Manager

@MainActor
final class TitlebarManager: NSObject {
    static let shared = TitlebarManager()

    private var toolbarContainerView: ToolbarContainerView?
    private var cancellables = Set<AnyCancellable>()
    private weak var viewModel: BrowserViewModel?
    private weak var bookmarkVM: BookmarkViewModel?

    /// Shared layout state for toolbar
    private let layoutState = ToolbarLayoutState()

    private override init() {
        super.init()
    }

    private weak var window: NSWindow?
    private weak var titlebarView: NSView?

    // MARK: - Sidebar State
    private var currentSidebarWidth: CGFloat = 250
    private var isSidebarCollapsed: Bool = false
    /// Precise content leading X from SwiftUI measurement
    private var measuredContentLeadingX: CGFloat?

    /// Content edge padding (matches main workspace padding)
    private let contentPadding: CGFloat = 4

    func setupTitlebar(for window: NSWindow, viewModel: BrowserViewModel, bookmarkVM: BookmarkViewModel) {
        self.viewModel = viewModel
        self.bookmarkVM = bookmarkVM
        self.window = window

        // Configure window for unified style
        window.styleMask.insert(.unifiedTitleAndToolbar)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.toolbarStyle = .unified

        // Create empty toolbar (required for unified style & sidebar integration)
        let toolbar = NSToolbar(identifier: "EmptyToolbar")
        toolbar.displayMode = .iconOnly
        toolbar.showsBaselineSeparator = false
        window.toolbar = toolbar

        // Find the titlebar container view in window hierarchy
        guard let titlebarView = findTitlebarContainerView(in: window) else {
            print("⚠️ Could not find titlebar container view")
            return
        }
        self.titlebarView = titlebarView

        // Create toolbar container (full width, handles background + content)
        let containerView = ToolbarContainerView(
            viewModel: viewModel,
            bookmarkVM: bookmarkVM,
            layoutState: layoutState,
            onOverview: { [weak self] in self?.onToggleFocusOverview?() },
            onToggleSidebar: { [weak self] in self?.toggleSidebar() }
        )
        containerView.translatesAutoresizingMaskIntoConstraints = false
        self.toolbarContainerView = containerView

        // Add to titlebar - must be on top for hit testing
        titlebarView.addSubview(containerView)

        // Constraints with 4px margin on top and right (matching window edge inset)
        NSLayoutConstraint.activate([
            containerView.leadingAnchor.constraint(equalTo: titlebarView.leadingAnchor),
            containerView.trailingAnchor.constraint(equalTo: titlebarView.trailingAnchor, constant: -4),
            containerView.topAnchor.constraint(equalTo: titlebarView.topAnchor, constant: 4),
            containerView.bottomAnchor.constraint(equalTo: titlebarView.bottomAnchor),
        ])

        // Initial background position
        updateBackgroundPosition()

        // Observe viewModel changes
        setupObservers(viewModel: viewModel, bookmarkVM: bookmarkVM)
    }

    // MARK: - Sidebar Updates

    /// Called from SwiftUI when sidebar width changes
    func updateSidebarWidth(_ width: CGFloat) {
        guard !isSidebarCollapsed else { return }
        if width <= 0.5 { return }
        currentSidebarWidth = width
        updateBackgroundPosition()
    }

    /// Called from SwiftUI when sidebar visibility changes
    func updateSidebarVisibility(_ visible: Bool) {
        let collapsed = !visible
        if isSidebarCollapsed != collapsed {
            isSidebarCollapsed = collapsed
            layoutState.isSidebarCollapsed = collapsed
            updateBackgroundPosition()
        }
    }

    /// Update content leading X from SwiftUI measurement (precise alignment)
    func updateContentLeadingXInWindow(_ xInWindow: CGFloat) {
        guard xInWindow.isFinite, xInWindow >= 0 else { return }

        // Store measured value (window coordinates = toolbar coordinates since both start at 0)
        measuredContentLeadingX = xInWindow

        // Always follow the measured value - this keeps toolbar background
        // in sync with NavigationSplitView animation during sidebar toggle
        let newLeading = max(contentPadding, xInWindow)
        toolbarContainerView?.updateBackgroundLeading(newLeading)
        layoutState.backgroundLeadingX = newLeading
    }

    private func updateBackgroundPosition() {
        let backgroundLeadingX: CGFloat
        if isSidebarCollapsed {
            // Collapsed: 4px left padding to match main workspace
            backgroundLeadingX = contentPadding
        } else if let measured = measuredContentLeadingX, measured.isFinite {
            // Use precise measurement from SwiftUI
            backgroundLeadingX = max(0, measured)
        } else {
            // Fallback: estimate from sidebar width + padding
            backgroundLeadingX = currentSidebarWidth + contentPadding
        }
        toolbarContainerView?.updateBackgroundLeading(backgroundLeadingX)
        layoutState.backgroundLeadingX = backgroundLeadingX
    }

    @objc private func toggleSidebar() {
        isSidebarCollapsed.toggle()
        layoutState.isSidebarCollapsed = isSidebarCollapsed
        // Post notification for ContentView to handle sidebar toggle
        NotificationCenter.default.post(name: .toggleSidebarRequested, object: nil)
    }

    private func findTitlebarContainerView(in window: NSWindow) -> NSView? {
        guard let themeFrame = window.contentView?.superview else { return nil }

        for subview in themeFrame.subviews {
            let className = String(describing: type(of: subview))
            if className.contains("TitlebarContainerView") || className.contains("Titlebar") {
                for innerView in subview.subviews {
                    let innerClassName = String(describing: type(of: innerView))
                    if innerClassName.contains("TitlebarView") {
                        return innerView
                    }
                }
                return subview
            }
        }
        return themeFrame
    }

    private func setupObservers(viewModel: BrowserViewModel, bookmarkVM: BookmarkViewModel) {
        viewModel.objectWillChange
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                self?.toolbarContainerView?.updateState()
            }
            .store(in: &cancellables)
    }

    // MARK: - Callbacks
    var onToggleFocusOverview: (() -> Void)?

    var onToggleBookmark: (() -> Void)? {
        get { toolbarContainerView?.onBookmarkTap }
        set { toolbarContainerView?.onBookmarkTap = newValue }
    }

    var onAddressBarTap: (() -> Void)? {
        get { toolbarContainerView?.onAddressBarTap }
        set { toolbarContainerView?.onAddressBarTap = newValue }
    }
}

// MARK: - Toolbar Container View

private class ToolbarContainerView: NSView {

    weak var viewModel: BrowserViewModel?
    weak var bookmarkVM: BookmarkViewModel?
    private var hostingView: NSHostingView<AnyView>?
    private var backgroundLayer: CAShapeLayer?
    private let layoutState: ToolbarLayoutState

    /// Current background leading X position
    private var backgroundLeadingX: CGFloat = 0

    // Callbacks
    var onAddressBarTap: (() -> Void)?
    var onBookmarkTap: (() -> Void)?
    var onOverview: (() -> Void)?
    var onToggleSidebar: (() -> Void)?

    init(viewModel: BrowserViewModel, bookmarkVM: BookmarkViewModel, layoutState: ToolbarLayoutState, onOverview: @escaping () -> Void, onToggleSidebar: @escaping () -> Void) {
        self.viewModel = viewModel
        self.bookmarkVM = bookmarkVM
        self.layoutState = layoutState
        self.onOverview = onOverview
        self.onToggleSidebar = onToggleSidebar

        super.init(frame: .zero)

        setupBackgroundLayer()
        setupSwiftUIView()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func setupBackgroundLayer() {
        wantsLayer = true
        layer?.masksToBounds = false

        let bgLayer = CAShapeLayer()
        bgLayer.fillColor = AppColors.contentBackground.cgColor
        layer?.insertSublayer(bgLayer, at: 0)
        self.backgroundLayer = bgLayer
    }

    // MARK: - Event Passthrough

    override func hitTest(_ point: NSPoint) -> NSView? {
        // Traffic lights area - passthrough
        if let trafficRect = trafficLightsUnionRectInSelf(padding: 6), trafficRect.contains(point) {
            return nil
        }

        // Outside background area (sidebar region when expanded) - passthrough for dragging
        if point.x < backgroundLeadingX {
            // But still handle our buttons in this region
            if let hostingView, hostingView.frame.contains(point) {
                return super.hitTest(point)
            }
            return nil
        }

        return super.hitTest(point)
    }

    private func trafficLightsUnionRectInSelf(padding: CGFloat) -> CGRect? {
        guard let window else { return nil }
        let buttons: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        let views = buttons.compactMap { window.standardWindowButton($0) }
        guard !views.isEmpty else { return nil }

        var union = CGRect.null
        for v in views {
            let rectInWindow = v.convert(v.bounds, to: nil)
            let rectInSelf = convert(rectInWindow, from: nil)
            union = union.union(rectInSelf)
        }

        guard !union.isNull,
              union.origin.x.isFinite && union.origin.y.isFinite &&
              union.size.width.isFinite && union.size.height.isFinite else {
            return nil
        }
        return union.insetBy(dx: -padding, dy: -padding)
    }

    private func setupSwiftUIView() {
        guard let viewModel = viewModel, let bookmarkVM = bookmarkVM else { return }

        let swiftUIView = ToolbarContentView(
            viewModel: viewModel,
            bookmarkVM: bookmarkVM,
            layoutState: layoutState,
            onOverview: { [weak self] in self?.onOverview?() },
            onBookmarkTap: { [weak self] in self?.onBookmarkTap?() },
            onToggleSidebar: { [weak self] in self?.onToggleSidebar?() }
        )

        let hosting = NSHostingView(rootView: AnyView(swiftUIView))
        hosting.translatesAutoresizingMaskIntoConstraints = false
        hosting.wantsLayer = true
        hosting.layer?.backgroundColor = .clear
        addSubview(hosting)

        NSLayoutConstraint.activate([
            hosting.leadingAnchor.constraint(equalTo: leadingAnchor),
            hosting.trailingAnchor.constraint(equalTo: trailingAnchor),
            hosting.topAnchor.constraint(equalTo: topAnchor),
            hosting.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        self.hostingView = hosting
    }

    func updateBackgroundLeading(_ leading: CGFloat) {
        backgroundLeadingX = leading
        updateBackgroundPath()
    }

    func updateState() {
        // SwiftUI handles updates via @ObservedObject
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        updateBackgroundPath()
    }

    override func layout() {
        super.layout()
        updateBackgroundPath()
    }

    private func updateBackgroundPath() {
        guard let bgLayer = backgroundLayer else { return }

        let cornerRadius: CGFloat = 16

        // Background fills the view (view itself is already inset by constraints)
        let adjustedRect = CGRect(
            x: backgroundLeadingX,
            y: 0,
            width: bounds.width - backgroundLeadingX,
            height: bounds.height
        )

        guard adjustedRect.width > 0 else {
            bgLayer.path = nil
            return
        }

        // Path with top-left and top-right rounded corners
        let path = CGMutablePath()

        // Bottom-left (square corner)
        path.move(to: CGPoint(x: adjustedRect.minX, y: adjustedRect.minY))
        // Bottom edge
        path.addLine(to: CGPoint(x: adjustedRect.maxX, y: adjustedRect.minY))
        // Right edge up to corner
        path.addLine(to: CGPoint(x: adjustedRect.maxX, y: adjustedRect.maxY - cornerRadius))
        // Top-right corner
        path.addArc(center: CGPoint(x: adjustedRect.maxX - cornerRadius, y: adjustedRect.maxY - cornerRadius),
                    radius: cornerRadius,
                    startAngle: 0,
                    endAngle: .pi / 2,
                    clockwise: false)
        // Top edge
        path.addLine(to: CGPoint(x: adjustedRect.minX + cornerRadius, y: adjustedRect.maxY))
        // Top-left corner
        path.addArc(center: CGPoint(x: adjustedRect.minX + cornerRadius, y: adjustedRect.maxY - cornerRadius),
                    radius: cornerRadius,
                    startAngle: .pi / 2,
                    endAngle: .pi,
                    clockwise: false)
        path.closeSubpath()

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        bgLayer.path = path
        CATransaction.commit()
    }
}

// MARK: - Toolbar Content View (SwiftUI)

private struct ToolbarContentView: View {
    @ObservedObject var viewModel: BrowserViewModel
    @ObservedObject var bookmarkVM: BookmarkViewModel
    @ObservedObject var layoutState: ToolbarLayoutState
    var onOverview: () -> Void
    var onBookmarkTap: () -> Void
    var onToggleSidebar: () -> Void

    @State private var addressText: String = ""
    @State private var isAddressFocused: Bool = false
    @State private var showingBookmarkPopover: Bool = false
    @State private var focusRingScaleX: CGFloat = 1.0
    @State private var focusRingScaleY: CGFloat = 1.0
    @State private var hasInputActivity: Bool = false

    // MARK: - Layout Constants

    /// Traffic lights right edge (approximate)
    private let trafficLightsRightEdge: CGFloat = 78
    /// Minimum spacing from traffic lights to first button
    private let trafficLightsMinSpacing: CGFloat = 24
    /// Icon button size
    private let iconButtonSize: CGFloat = 30
    /// Icon font size
    private let iconPointSize: CGFloat = 16
    /// Minimum spacing between icons
    private let minIconSpacing: CGFloat = 8
    /// Standard icon spacing
    private let standardIconSpacing: CGFloat = 16
    /// Padding inside background area
    private let backgroundInnerPadding: CGFloat = 12
    /// Right edge padding
    private let rightPadding: CGFloat = 10
    /// Minimum address bar width
    private let minAddressBarWidth: CGFloat = 150

    var body: some View {
        GeometryReader { geo in
            let totalWidth = geo.size.width
            let bgLeading = layoutState.backgroundLeadingX

            // Calculate available width for main toolbar (from background start to right edge)
            let mainToolbarWidth = totalWidth - bgLeading - rightPadding

            // Adaptive layout calculations
            let layout = calculateLayout(mainToolbarWidth: mainToolbarWidth, totalWidth: totalWidth)

            // === All toolbar buttons in single HStack, fixed order ===
            // Order: [+] [Overview] [ToggleSidebar] [NavButtons] [AddressBar] [Share]
            // Only X positions change based on sidebar state, never the order
            mainToolbarContent(
                layout: layout,
                bgLeading: bgLeading,
                mainToolbarWidth: mainToolbarWidth,
                totalWidth: totalWidth
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onChange(of: viewModel.currentTab?.url) { _, _ in updateDisplayText() }
        .onChange(of: viewModel.currentTab?.title) { _, _ in updateDisplayText() }
        .onChange(of: viewModel.currentTabId) { _, _ in updateDisplayText() }
        .onChange(of: isAddressFocused) { _, focused in
            if !focused { updateAddressTextFromTab() }
        }
        .onAppear { updateDisplayText() }
        .onChange(of: viewModel.shouldAutoFocusAddressBar) { _, shouldFocus in
            if shouldFocus {
                enterEditingMode()
                viewModel.shouldAutoFocusAddressBar = false
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSNotification.Name("com.actro.dismissAddressBarFocus"))) { _ in
            exitEditingMode()
        }
    }

    // MARK: - Layout Calculation

    private struct LayoutConfig {
        var leftButtonsLeadingX: CGFloat = 0
        var showNavButtons: Bool = true
        var showShareButton: Bool = true
        var iconSpacing: CGFloat = 16
        var addressBarFlex: CGFloat = 0.5 // Fraction of available space
        var isCompact: Bool = false
        var isVeryCompact: Bool = false
    }

    private func calculateLayout(mainToolbarWidth: CGFloat, totalWidth: CGFloat) -> LayoutConfig {
        var config = LayoutConfig()

        // Left buttons position (in sidebar area)
        config.leftButtonsLeadingX = trafficLightsRightEdge + trafficLightsMinSpacing

        // Thresholds for different layout modes
        let veryCompactThreshold: CGFloat = 320
        let compactThreshold: CGFloat = 450
        let normalThreshold: CGFloat = 550

        if mainToolbarWidth < veryCompactThreshold {
            // Very compact: hide nav buttons, use overflow menu
            config.isVeryCompact = true
            config.isCompact = true
            config.showNavButtons = false
            config.showShareButton = false
            config.iconSpacing = minIconSpacing
            config.addressBarFlex = 0.6
        } else if mainToolbarWidth < compactThreshold {
            // Compact: show nav buttons, hide share, tighter spacing
            config.isCompact = true
            config.showNavButtons = true
            config.showShareButton = false
            config.iconSpacing = minIconSpacing + 4
            config.addressBarFlex = 0.55
        } else if mainToolbarWidth < normalThreshold {
            // Medium: show all, slightly tighter
            config.showNavButtons = true
            config.showShareButton = true
            config.iconSpacing = standardIconSpacing - 4
            config.addressBarFlex = 0.5
        } else {
            // Normal/Wide: full layout
            config.showNavButtons = true
            config.showShareButton = true
            config.iconSpacing = standardIconSpacing
            config.addressBarFlex = 0.45
        }

        return config
    }

    // MARK: - Main Toolbar Content

    /// Unified toolbar layout - positions animate based on bgLeading changes
    /// All buttons stay in same view hierarchy to enable smooth animations
    @ViewBuilder
    private func mainToolbarContent(layout: LayoutConfig, bgLeading: CGFloat, mainToolbarWidth: CGFloat, totalWidth: CGFloat) -> some View {
        let leftSectionLeading: CGFloat = trafficLightsRightEdge + trafficLightsMinSpacing

        // Calculate [+] [Overview] position:
        // - Expanded: right-aligned to bgLeading (in sidebar area)
        // - Collapsed: left-aligned after traffic lights
        let leftButtonsWidth: CGFloat = iconButtonSize * 2 + standardIconSpacing
        let leftButtonsLeading: CGFloat = {
            if layoutState.isSidebarCollapsed {
                // Collapsed: start after traffic lights
                return leftSectionLeading
            } else {
                // Expanded: right-align to background edge
                return max(leftSectionLeading, bgLeading - backgroundInnerPadding - leftButtonsWidth)
            }
        }()

        // Calculate spacing between [Overview] and [ToggleSidebar]
        let gapToNavGroup: CGFloat = {
            if layoutState.isSidebarCollapsed {
                return layout.iconSpacing
            } else {
                // In expanded mode, they're in different sections (gap handled by bgLeading)
                return 0
            }
        }()

        ZStack(alignment: .leading) {
            // === Left buttons: [+] [Overview] ===
            HStack(spacing: standardIconSpacing) {
                newTabButton
                overviewButton
            }
            .padding(.leading, leftButtonsLeading)

            // === Nav group + main content: [ToggleSidebar] [<] [>] [AddressBar] [Share] ===
            HStack(spacing: layout.iconSpacing) {
                // Nav group
                HStack(spacing: layout.isCompact ? 4 : 8) {
                    sidebarToggleButton

                    if layout.showNavButtons {
                        navButtonsGroup
                    }
                }

                Spacer(minLength: layout.isCompact ? 4 : 8)

                // Address bar
                addressBarView
                    .frame(minWidth: minAddressBarWidth)
                    .frame(maxWidth: mainToolbarWidth * layout.addressBarFlex)

                Spacer(minLength: layout.isCompact ? 4 : 8)

                // Right side
                if layout.showShareButton {
                    shareButton
                } else {
                    overflowMenuButton(showNavActions: !layout.showNavButtons)
                }
            }
            .padding(.leading, layoutState.isSidebarCollapsed
                ? leftButtonsLeading + leftButtonsWidth + gapToNavGroup
                : bgLeading + backgroundInnerPadding)
            .padding(.trailing, rightPadding)
        }
        .animation(.easeInOut(duration: 0.25), value: layoutState.isSidebarCollapsed)
        .animation(.easeInOut(duration: 0.25), value: bgLeading)
    }

    // MARK: - Buttons

    private var newTabButton: some View {
        Button(action: {
            NSLog("🔘 [Toolbar] New tab button tapped")
            _ = viewModel.addNewTab()
        }) {
            Image(systemName: "plus")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    private var overviewButton: some View {
        Button(action: {
            NSLog("🔘 [Toolbar] Overview button tapped")
            onOverview()
        }) {
            Image(systemName: "square.on.square")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    private var sidebarToggleButton: some View {
        Button(action: onToggleSidebar) {
            Image(systemName: "sidebar.left")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .help("Toggle Sidebar (⌃⌘S)")
    }

    private var navButtonsGroup: some View {
        HStack(spacing: 0) {
            Button(action: {
                NSLog("🔘 [Toolbar] Back button tapped")
                viewModel.goBack()
            }) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: iconButtonSize, height: iconButtonSize)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(!viewModel.canGoBack)

            Divider()
                .frame(height: 18)
                .padding(.horizontal, 1)

            Button(action: {
                NSLog("🔘 [Toolbar] Forward button tapped")
                viewModel.goForward()
            }) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: iconButtonSize, height: iconButtonSize)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(!viewModel.canGoForward)
        }
        .foregroundStyle(.secondary)
    }

    // MARK: - Address Bar

    private var addressBarView: some View {
        let mode = addressBarMode

        return HStack(spacing: 0) {
            // Left icon
            if mode.showsLoading && viewModel.isCurrentTabLoading {
                ArcSpinner()
                    .frame(width: 32, height: 32)
            } else if let icon = currentLeftIcon {
                Button(action: {
                    NSLog("🔘 [Toolbar] Left button tapped (mode: \(mode))")
                    withAnimation(.easeInOut(duration: 0.2)) {
                        viewModel.toggleLeftPanelForCurrentTab()
                    }
                    let tabId: UUID? = {
                        if case .focus(let focusTabId) = viewModel.actriumMode {
                            return focusTabId
                        }
                        return viewModel.currentTabId
                    }()
                    if let tabId = tabId {
                        let newVisible = viewModel.isLeftPanelVisible(for: tabId)
                        let coordinator = ViewBridgeCoordinatorManager.shared.coordinator(for: tabId)
                        coordinator.sendLeftPanelVisibilityChanged(newVisible)
                    }
                }) {
                    Image(systemName: icon)
                        .font(.system(size: 14, weight: .light))
                        .frame(width: 32, height: 32)
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.secondary.opacity(0.5))
            } else {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(Color.secondary.opacity(0.5))
                    .frame(width: 32, height: 32)
            }

            // Address text
            if isAddressFocused {
                ZStack(alignment: .leading) {
                    if addressText.isEmpty && !hasInputActivity {
                        Text("Search or enter website name")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .allowsHitTesting(false)
                    }
                    AddressInputField(
                        text: $addressText,
                        onSubmit: submitAddress,
                        onBlur: handleBlur,
                        onInputActivity: { hasInputActivity = true }
                    )
                }
                .frame(maxWidth: .infinity, maxHeight: 22)
                .padding(.horizontal, 8)
            } else {
                Group {
                    if addressText.isEmpty {
                        Text("Search or enter website name")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                    } else {
                        Text(addressText)
                            .font(.system(size: 13))
                            .foregroundStyle(.primary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: 22)
                .padding(.horizontal, 8)
                .contentShape(Rectangle())
                .onTapGesture { enterEditingMode() }
            }

            // Bookmark button
            if mode.showsBookmark {
                Button(action: {
                    NSLog("🔘 [Toolbar] Bookmark button tapped, isBookmarked=\(isBookmarked)")
                    if !isBookmarked, let tab = viewModel.currentTab {
                        let bookmark = Bookmark.from(tab: tab, publisher: nil)
                        bookmarkVM.addBookmark(bookmark)
                    }
                    showingBookmarkPopover = true
                }) {
                    Image(systemName: isBookmarked ? "star.fill" : "star")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 32, height: 32)
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(isBookmarked ? Color.secondary : Color.secondary.opacity(0.5))
                .popover(isPresented: $showingBookmarkPopover, arrowEdge: .bottom) {
                    bookmarkPopoverContent
                }
            }

            // Refresh button
            if mode.showsRefresh {
                Button(action: {
                    NSLog("🔘 [Toolbar] Refresh button tapped")
                    viewModel.reloadCurrentTab()
                }) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 32, height: 32)
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.secondary.opacity(0.5))
            }
        }
        .frame(height: 30)
        .padding(.horizontal, 4)
        .padding(.vertical, 3)
        .background(Color.white.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(Color.blue.opacity(0.75), lineWidth: 3)
                .opacity(isAddressFocused ? 1 : 0)
                .animation(.easeOut(duration: 0.12), value: isAddressFocused)
        }
    }

    // MARK: - Right Side Buttons

    private var shareButton: some View {
        ShareLink(item: viewModel.currentTab?.url ?? URL(string: "about:blank")!) {
            Image(systemName: "square.and.arrow.up")
                .font(.system(size: 15, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    @ViewBuilder
    private func overflowMenuButton(showNavActions: Bool) -> some View {
        Menu {
            if showNavActions {
                Button(action: { viewModel.goBack() }) {
                    Label("Go Back", systemImage: "chevron.left")
                }
                .disabled(!viewModel.canGoBack)

                Button(action: { viewModel.goForward() }) {
                    Label("Go Forward", systemImage: "chevron.right")
                }
                .disabled(!viewModel.canGoForward)

                Divider()
            }

            if let url = viewModel.currentTab?.url {
                Button(action: {
                    let picker = NSSharingServicePicker(items: [url])
                    if let window = NSApp.keyWindow,
                       let contentView = window.contentView {
                        picker.show(relativeTo: .zero, of: contentView, preferredEdge: .maxY)
                    }
                }) {
                    Label("Share...", systemImage: "square.and.arrow.up")
                }
            }

            Button(action: { _ = viewModel.addNewTab() }) {
                Label("New Tab", systemImage: "plus")
            }

            Button(action: onOverview) {
                Label("Tab Overview", systemImage: "square.on.square")
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 14, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
                .contentShape(Circle())
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .foregroundStyle(.secondary)
    }

    // MARK: - Helpers

    private func displayText(for tab: Tab?) -> String {
        guard let tab = tab else { return "" }
        if let systemType = tab.systemTabType {
            return systemType.aboutURL.absoluteString
        }
        if case .web = tab.contentType, let url = tab.url {
            let host = url.host ?? url.absoluteString
            return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
        }
        return tab.title
    }

    private func editingText(for tab: Tab?) -> String {
        guard let tab = tab else { return "" }
        if let systemType = tab.systemTabType {
            return systemType.aboutURL.absoluteString
        }
        if let url = tab.url {
            return url.absoluteString
        }
        return displayText(for: tab)
    }

    private func enterEditingMode() {
        addressText = editingText(for: viewModel.currentTab)
        guard !isAddressFocused else { return }

        var noAnimation = Transaction()
        noAnimation.disablesAnimations = true
        withTransaction(noAnimation) {
            focusRingScaleX = 1.1
            focusRingScaleY = 1.25
        }

        isAddressFocused = true

        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.75)) {
                self.focusRingScaleX = 1.0
                self.focusRingScaleY = 1.0
            }
        }
    }

    private func exitEditingMode() {
        isAddressFocused = false
        var noAnimation = Transaction()
        noAnimation.disablesAnimations = true
        withTransaction(noAnimation) {
            focusRingScaleX = 1.0
            focusRingScaleY = 1.0
        }
        hasInputActivity = false
    }

    private func updateAddressTextFromTab() {
        guard let tab = viewModel.currentTab else {
            addressText = ""
            return
        }
        addressText = displayText(for: tab)
    }

    private func updateDisplayText() {
        guard !isAddressFocused else { return }
        updateAddressTextFromTab()
    }

    private func handleBlur() {
        exitEditingMode()
    }

    private func submitAddress() {
        let input = addressText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !input.isEmpty else { return }
        viewModel.navigateCurrentTab(to: input)
        exitEditingMode()
    }

    private enum AddressBarMode {
        case web, app, empty

        var leftIcon: String? {
            switch self {
            case .web: return "globe"
            case .app: return "list.bullet"
            case .empty: return nil
            }
        }

        var showsLoading: Bool { self != .empty }
        var showsBookmark: Bool { self != .empty }
        var showsRefresh: Bool { self != .empty }
    }

    private var addressBarMode: AddressBarMode {
        guard let tab = viewModel.currentTab else { return .empty }
        switch tab.contentType {
        case .web: return .web
        case .reactNative, .native: return .app
        case .empty: return .empty
        }
    }

    private var currentLeftIcon: String? {
        let mode = addressBarMode
        guard let tab = viewModel.currentTab else { return mode.leftIcon }
        if case .app = mode, let customIcon = tab.appIcon {
            return customIcon
        }
        return mode.leftIcon
    }

    private var isBookmarked: Bool {
        guard let tab = viewModel.currentTab else { return false }
        return bookmarkVM.isBookmarked(tab: tab)
    }

    @ViewBuilder
    private var bookmarkPopoverContent: some View {
        if let tab = viewModel.currentTab {
            let existingBookmark = bookmarkVM.getBookmark(for: tab)
            BookmarkEditSheet(
                viewModel: bookmarkVM,
                tab: tab,
                existingBookmark: existingBookmark
            )
        } else {
            Text("No tab selected")
                .padding()
        }
    }
}

// MARK: - Address Input Field

private struct AddressInputField: NSViewRepresentable {
    @Binding var text: String
    var onSubmit: () -> Void
    var onBlur: () -> Void
    var onInputActivity: (() -> Void)?

    func makeNSView(context: Context) -> NSTextField {
        let textField = NSTextField()
        textField.font = .systemFont(ofSize: 13)
        textField.isBordered = false
        textField.drawsBackground = false
        textField.focusRingType = .none
        textField.delegate = context.coordinator
        textField.alignment = .left
        textField.lineBreakMode = .byTruncatingTail
        textField.maximumNumberOfLines = 1
        textField.cell?.isScrollable = true
        textField.cell?.wraps = false
        textField.usesSingleLineMode = true
        textField.isAutomaticTextCompletionEnabled = false
        textField.stringValue = text
        context.coordinator.textField = textField

        // SwiftUI mounts the NSView into the window during layout, which can
        // happen after this runloop tick — a single async focus attempt loses
        // the race (window == nil → makeFirstResponder silently skipped).
        // Retry until attached (same fix as SelfDrawnAddressInputField).
        AddressInputField.focusWhenAttached(
            textField: textField,
            coordinator: context.coordinator,
            attempt: 0
        )

        return textField
    }

    func updateNSView(_ nsView: NSTextField, context: Context) {}

    private static func focusWhenAttached(
        textField: NSTextField,
        coordinator: Coordinator,
        attempt: Int
    ) {
        DispatchQueue.main.async {
            guard let window = textField.window else {
                if attempt < 30 {
                    AddressInputField.focusWhenAttached(
                        textField: textField,
                        coordinator: coordinator,
                        attempt: attempt + 1
                    )
                }
                return
            }
            let _ = window.makeFirstResponder(textField)
            textField.currentEditor()?.selectAll(nil)
            coordinator.startObservingFirstResponder()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: AddressInputField
        weak var textField: NSTextField?
        private var firstResponderObservation: NSKeyValueObservation?

        init(_ parent: AddressInputField) {
            self.parent = parent
            super.init()
        }

        deinit {
            stopObservingFirstResponder()
        }

        func startObservingFirstResponder() {
            guard let window = textField?.window else { return }
            firstResponderObservation = window.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
                self?.checkFirstResponder()
            }
        }

        func stopObservingFirstResponder() {
            firstResponderObservation?.invalidate()
            firstResponderObservation = nil
        }

        private func checkFirstResponder() {
            guard let textField = textField, let window = textField.window else { return }

            let firstResponder = window.firstResponder
            let isOurTextField = (firstResponder === textField)
            let isFieldEditor = (firstResponder as? NSTextView)?.delegate === textField

            if !isOurTextField && !isFieldEditor {
                DispatchQueue.main.async {
                    self.parent.onBlur()
                }
            }
        }

        func controlTextDidEndEditing(_ obj: Notification) {
            stopObservingFirstResponder()
            parent.onBlur()
        }

        func controlTextDidBeginEditing(_ obj: Notification) {
            startObservingFirstResponder()
            parent.onInputActivity?()
        }

        func controlTextDidChange(_ obj: Notification) {
            guard let tf = obj.object as? NSTextField else { return }
            parent.text = tf.stringValue
            parent.onInputActivity?()
        }

        func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
            if commandSelector == #selector(NSResponder.insertNewline(_:)) {
                parent.onSubmit()
                textField?.window?.makeFirstResponder(nil)
                return true
            }
            if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
                textField?.window?.makeFirstResponder(nil)
                return true
            }
            return false
        }
    }
}

// MARK: - Arc Spinner

private struct ArcSpinner: View {
    var body: some View {
        TimelineView(.animation) { timeline in
            let seconds = timeline.date.timeIntervalSinceReferenceDate
            let rotation = (seconds.truncatingRemainder(dividingBy: 0.8)) / 0.8 * 360

            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(Color.secondary, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .frame(width: 14, height: 14)
                .rotationEffect(.degrees(rotation))
        }
    }
}
