import AppKit
import SwiftUI
import Combine

// MARK: - Root View (layout-cycle traffic light correction)

/// Custom root view that repositions traffic lights in its `layout()` override.
///
/// AppKit's layout cycle is top-down: the window's theme-frame lays out
/// the NSTitlebarContainerView (which positions the standard window buttons
/// at their default location) **before** laying out this content view.
/// By calling `adjustTrafficLights()` in our `layout()`, we always run
/// after the system and override its positioning — within the same frame,
/// no deferred dispatch, no visual flash.
///
/// `viewDidMoveToWindow()` installs a KVO observation on `window.title`.
/// Setting `window.title` is the trigger that causes the system to
/// re-layout the titlebar; the KVO fires synchronously in the setter,
/// marks this view for layout, and the layout system does the rest.
private class TrafficLightAwareRootView: NSView {
    weak var controller: RootContentViewController?
    private var titleObservation: NSKeyValueObservation?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let window {
            titleObservation = window.observe(\.title, options: []) { [weak self] _, _ in
                self?.needsLayout = true
            }
        } else {
            titleObservation = nil
        }
    }

    override func layout() {
        super.layout()
        controller?.handleRootViewLayout()
    }
}

/// Root content view controller - manages 4px edge inset and contains toolbar + split view
final class RootContentViewController: NSViewController {

    // MARK: - Properties

    private let viewModel: BrowserViewModel
    private let historyVM: HistoryViewModel
    private let bookmarkVM: BookmarkViewModel

    /// Edge inset (transparent border around content)
    private let edgeInset: CGFloat = 4

    /// Corner radius for content area
    private let cornerRadius: CGFloat = 12

    /// Toolbar height (further reduced by 1pt for optical centering)
    private let toolbarHeight: CGFloat = 45

    /// Window background color (TODO: make configurable via settings)
    private let windowBackgroundColor = NSColor(red: 0.1, green: 0.2, blue: 0.4, alpha: 1.0)

    /// Traffic lights position (absolute, in our view's coordinates)
    private let trafficLightPosition = NSPoint(x: 15, y: 0)
    private let trafficLightSpacing: CGFloat = 9  // Gap between buttons (nudged by +1px)
    private let trafficLightTopInset: CGFloat = 15  // Distance from top (nudged up by 2pt)
    private let trafficLightButtonTypes: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]

    /// Keep original system superviews so fullscreen can restore native behavior.
    private var trafficLightOriginalSuperviews: [NSWindow.ButtonType: NSView] = [:]
    private var trafficLightConstraints: [NSWindow.ButtonType: [NSLayoutConstraint]] = [:]

    // Child views
    private var roundedContainer: NSView!
    private var toolbarHostingView: NSHostingView<AnyView>!
    private var splitViewController: MainSplitViewController!
    private let sidebarLayoutState = SidebarLayoutState()

    // DEBUG: dashed border layer for roundedContainer
    private var debugBorderLayer: CAShapeLayer?

    // Mask layer for content clipping (separate from debug border)
    private var contentMaskLayer: CAShapeLayer?


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
        let rootView = TrafficLightAwareRootView()
        rootView.controller = self
        rootView.wantsLayer = true
        rootView.layer?.backgroundColor = windowBackgroundColor.cgColor
        self.view = rootView
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        setupRoundedContainer()
        setupToolbar()
        setupSplitView()
        setupConstraints()
    }

    // MARK: - Setup

    private func setupRoundedContainer() {
        // Container that holds all content with rounded corners on all sides
        roundedContainer = NSView()
        roundedContainer.wantsLayer = true
        roundedContainer.layer?.backgroundColor = NSColor.clear.cgColor  // Transparent - content views provide their own background
        roundedContainer.translatesAutoresizingMaskIntoConstraints = false

        // Content mask for rounded corners (clip content but not debug border)
        let maskLayer = CAShapeLayer()
        roundedContainer.layer?.mask = maskLayer
        contentMaskLayer = maskLayer

        // DEBUG: Add dashed border using CAShapeLayer (added to parent so it's not clipped)
        let borderLayer = CAShapeLayer()
        borderLayer.strokeColor = NSColor.red.cgColor
        borderLayer.fillColor = nil
        borderLayer.lineWidth = 1
        borderLayer.lineDashPattern = [4, 4]  // 4px dash, 4px gap
        debugBorderLayer = borderLayer

        view.addSubview(roundedContainer)

        // Add debug border to parent view (will be positioned over roundedContainer)
        view.layer?.addSublayer(borderLayer)
    }

    private func setupToolbar() {
        // Create SwiftUI toolbar
        let toolbarView = SelfDrawnToolbarView(
            viewModel: viewModel,
            bookmarkVM: bookmarkVM,
            layoutState: sidebarLayoutState,
            onToggleSidebar: { [weak self] in
                self?.splitViewController?.toggleSidebar(nil)
            }
        )

        toolbarHostingView = DraggableToolbarHostingView(rootView: AnyView(toolbarView))
        toolbarHostingView.translatesAutoresizingMaskIntoConstraints = false
        toolbarHostingView.wantsLayer = true
        // Disable titlebar safe area — we manage our own toolbar layout;
        // the system titlebar is only meaningful in fullscreen.
        toolbarHostingView.safeAreaRegions = []
        toolbarHostingView.layer?.borderColor = NSColor.systemGreen.cgColor  // DEBUG
        toolbarHostingView.layer?.borderWidth = 1  // DEBUG
        roundedContainer.addSubview(toolbarHostingView)

        // Double-click toolbar to zoom/minimize (respects system preference)
        let doubleClick = NSClickGestureRecognizer(target: self, action: #selector(toolbarDoubleClicked(_:)))
        doubleClick.numberOfClicksRequired = 2
        toolbarHostingView.addGestureRecognizer(doubleClick)
    }

    @objc private func toolbarDoubleClicked(_ sender: NSClickGestureRecognizer) {
        guard let window = view.window else { return }
        let action = UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") ?? "Maximize"
        switch action {
        case "Minimize":
            window.miniaturize(nil)
        case "None":
            break
        default:  // "Maximize"
            window.zoom(nil)
        }
    }

    private func setupSplitView() {
        // Create split view controller
        splitViewController = MainSplitViewController(
            viewModel: viewModel,
            historyVM: historyVM,
            bookmarkVM: bookmarkVM,
            edgeInset: 0,  // No additional inset needed, container handles it
            sidebarLayoutState: sidebarLayoutState
        )

        addChild(splitViewController)

        let splitView = splitViewController.view
        splitView.translatesAutoresizingMaskIntoConstraints = false
        splitView.wantsLayer = true
        splitView.layer?.borderColor = NSColor.systemYellow.cgColor  // DEBUG
        splitView.layer?.borderWidth = 1  // DEBUG
        roundedContainer.addSubview(splitView)
    }

    private func setupConstraints() {
        NSLayoutConstraint.activate([
            // Rounded container: 4px inset on all sides
            roundedContainer.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: edgeInset),
            roundedContainer.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -edgeInset),
            roundedContainer.topAnchor.constraint(equalTo: view.topAnchor, constant: edgeInset),
            roundedContainer.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -edgeInset),

            // Toolbar: top of container, full width, fixed height
            toolbarHostingView.leadingAnchor.constraint(equalTo: roundedContainer.leadingAnchor),
            toolbarHostingView.trailingAnchor.constraint(equalTo: roundedContainer.trailingAnchor),
            toolbarHostingView.topAnchor.constraint(equalTo: roundedContainer.topAnchor),
            toolbarHostingView.heightAnchor.constraint(equalToConstant: toolbarHeight),

            // Split view: below toolbar, fills remaining space
            splitViewController.view.leadingAnchor.constraint(equalTo: roundedContainer.leadingAnchor),
            splitViewController.view.trailingAnchor.constraint(equalTo: roundedContainer.trailingAnchor),
            splitViewController.view.topAnchor.constraint(equalTo: toolbarHostingView.bottomAnchor),
            splitViewController.view.bottomAnchor.constraint(equalTo: roundedContainer.bottomAnchor),
        ])
    }

    // MARK: - Layout Updates

    /// Called from `TrafficLightAwareRootView.layout()` — guaranteed to run
    /// after the system's titlebar layout in the same layout pass.
    fileprivate func handleRootViewLayout() {
        // Update content mask to match roundedContainer bounds
        if let maskLayer = contentMaskLayer {
            let bounds = roundedContainer.bounds
            let path = CGPath(roundedRect: bounds, cornerWidth: cornerRadius, cornerHeight: cornerRadius, transform: nil)
            maskLayer.path = path
        }

        // DEBUG: Update dashed border path to match roundedContainer frame (in parent coordinates)
        if let borderLayer = debugBorderLayer {
            let frame = roundedContainer.frame
            let path = CGPath(roundedRect: frame, cornerWidth: cornerRadius, cornerHeight: cornerRadius, transform: nil)
            borderLayer.path = path
        }

        adjustTrafficLights()
    }

    // MARK: - Traffic Lights

    private func adjustTrafficLights() {
        guard let window = view.window else { return }

        if window.styleMask.contains(.fullScreen) {
            // Fullscreen traffic lights are handled by MainWindowController.
            return
        }

        var xOffset: CGFloat = 0

        for buttonType in trafficLightButtonTypes {
            guard let button = window.standardWindowButton(buttonType) else { continue }

            if trafficLightOriginalSuperviews[buttonType] == nil, let original = button.superview {
                trafficLightOriginalSuperviews[buttonType] = original
            }

            let leadingConstant = trafficLightPosition.x + xOffset

            if button.superview != roundedContainer {
                button.removeFromSuperview()
                button.translatesAutoresizingMaskIntoConstraints = false
                roundedContainer.addSubview(button)

                let constraints = [
                    button.leadingAnchor.constraint(equalTo: roundedContainer.leadingAnchor, constant: leadingConstant),
                    button.topAnchor.constraint(equalTo: roundedContainer.topAnchor, constant: trafficLightTopInset)
                ]
                trafficLightConstraints[buttonType] = constraints
                NSLayoutConstraint.activate(constraints)
            } else if let constraints = trafficLightConstraints[buttonType], constraints.count == 2 {
                constraints[0].constant = leadingConstant
                constraints[1].constant = trafficLightTopInset
            }

            // Next button position: current position + button width + gap
            let buttonWidth = max(button.frame.width, button.fittingSize.width)
            xOffset += buttonWidth + trafficLightSpacing
        }
    }

    private func restoreTrafficLightsToSystemContainer(_ window: NSWindow) {
        for buttonType in trafficLightButtonTypes {
            guard let button = window.standardWindowButton(buttonType) else { continue }
            guard let originalSuperview = trafficLightOriginalSuperviews[buttonType] else { continue }

            if let constraints = trafficLightConstraints[buttonType] {
                NSLayoutConstraint.deactivate(constraints)
                trafficLightConstraints[buttonType] = nil
            }

            if button.superview != originalSuperview {
                button.removeFromSuperview()
                button.translatesAutoresizingMaskIntoConstraints = true
                originalSuperview.addSubview(button)
            }
        }
    }

    // MARK: - External Layout State

    func updateWindowFullScreenState(_ isFullScreen: Bool) {
        sidebarLayoutState.updateWindowFullScreen(isFullScreen)
    }
}

// MARK: - Draggable Toolbar Hosting View

/// NSHostingView subclass that initiates window drag on `mouseDown` when the
/// click lands on a non-interactive area (background, empty space).
/// SwiftUI buttons / text fields handle their own hits normally; everything
/// else becomes a window-drag handle — matching standard titlebar behaviour.
private final class DraggableToolbarHostingView: NSHostingView<AnyView> {
    override func mouseDown(with event: NSEvent) {
        // If the deepest hit-tested view is ourself (no SwiftUI control claimed it),
        // start a window drag instead of consuming the event.
        guard let window else { return super.mouseDown(with: event) }

        let locationInSelf = convert(event.locationInWindow, from: nil)
        let hitView = super.hitTest(locationInSelf)

        // hitView being self or nil means no interactive SwiftUI element is there.
        if hitView === self || hitView == nil {
            window.performDrag(with: event)
        } else {
            super.mouseDown(with: event)
        }
    }
}

// MARK: - Self-Drawn Toolbar View

struct SelfDrawnToolbarView: View {
    @ObservedObject var viewModel: BrowserViewModel
    @ObservedObject var bookmarkVM: BookmarkViewModel
    @ObservedObject var layoutState: SidebarLayoutState
    var onToggleSidebar: () -> Void

    // Address bar editing state
    @State private var isAddressFocused: Bool = false
    @State private var addressText: String = ""
    @State private var hasInputActivity: Bool = false

    // Layout constants
    private let iconButtonSize: CGFloat = 30
    private let iconPointSize: CGFloat = 16
    private let iconVerticalNudge: CGFloat = -2

    var body: some View {
        GeometryReader { geo in
            let metrics = ToolbarLayoutMetrics(state: layoutState)
            let totalWidth = max(0, geo.size.width)
            let sidebarWidth = min(totalWidth, max(0, metrics.sidebarSectionWidth))
            let contentLeading = min(
                max(0, metrics.contentLeadingX),
                max(0, totalWidth - 120)
            )
            let contentWidth = max(0, totalWidth - contentLeading)

            ZStack(alignment: .topLeading) {
                // Background in same coordinate space; cancel button
                // padding so it fills from the hosting view's top edge.
                toolbarBackground(metrics: metrics)
                    .padding(.top, -(9 + iconVerticalNudge))

                sidebarToolbarSection(metrics: metrics)
                    .frame(width: sidebarWidth, alignment: .leading)

                contentToolbarSection
                    .frame(width: contentWidth, alignment: .leading)
                    .offset(x: contentLeading)
            }
            // Align button centers with traffic lights (center ≈ 22px):
            // trafficLightTopInset=15, ~14px tall → center at 22px
            // iconButtonSize=30 → top at 22-15 = 7px
            .padding(.top, 9 + iconVerticalNudge)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .overlay {
            // DEBUG: border for SwiftUI toolbar content bounds
            Rectangle()
                .stroke(Color.blue.opacity(0.8), lineWidth: 1)
                .allowsHitTesting(false)
        }
        .onAppear { updateDisplayText() }
        .onChange(of: viewModel.currentTab?.url) { _, _ in updateDisplayText() }
        .onChange(of: viewModel.currentTab?.title) { _, _ in updateDisplayText() }
        .onChange(of: viewModel.currentTabId) { _, _ in updateDisplayText() }
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

    // MARK: - Sidebar Section

    private func sidebarToolbarSection(metrics: ToolbarLayoutMetrics) -> some View {
        HStack(spacing: 0) {
            // Non-fullscreen: reserve space for native traffic lights region.
            // Height must be constrained to avoid expanding the HStack to fill
            // the ZStack's full height, which would vertically center the buttons.
            Color.clear
                .frame(width: metrics.trafficLightsReservedWidth, height: 0)

            Spacer(minLength: metrics.sidebarMinGap)

            HStack(spacing: metrics.sidebarButtonsSpacing) {
                newTabButton
                overviewButton
            }
        }
        .padding(.trailing, metrics.sidebarButtonsTrailingInset)
    }

    // MARK: - Content Section

    private var contentToolbarSection: some View {
        HStack(spacing: 12) {
            sidebarToggleButton

            // Navigation buttons
            HStack(spacing: 4) {
                Button(action: { viewModel.goBack() }) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 14, weight: .semibold))
                        .frame(width: iconButtonSize, height: iconButtonSize)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .disabled(!viewModel.canGoBack)

                Button(action: { viewModel.goForward() }) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 14, weight: .semibold))
                        .frame(width: iconButtonSize, height: iconButtonSize)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .disabled(!viewModel.canGoForward)
            }

            // Address bar
            addressBar
                .frame(maxWidth: 400)

            Spacer()

            // Right side actions
            rightActions
        }
        .padding(.trailing, 12)
    }

    // MARK: - Address Bar

    private var addressBar: some View {
        HStack(spacing: 4) {
            // Left icon
            Image(systemName: "line.3.horizontal")
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)

            // Address text / input field
            if isAddressFocused {
                ZStack(alignment: .leading) {
                    if addressText.isEmpty && !hasInputActivity {
                        Text("Search or enter website name")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .allowsHitTesting(false)
                    }
                    SelfDrawnAddressInputField(
                        text: $addressText,
                        onSubmit: submitAddress,
                        onBlur: exitEditingMode,
                        onInputActivity: { hasInputActivity = true }
                    )
                }
                .frame(maxWidth: .infinity)
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
                            .truncationMode(.middle)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture { enterEditingMode() }
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: iconButtonSize)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(Color.blue.opacity(0.75), lineWidth: 2)
                .opacity(isAddressFocused ? 1 : 0)
                .animation(.easeOut(duration: 0.12), value: isAddressFocused)
        }
    }

    // MARK: - Address Bar Helpers

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
        isAddressFocused = true
    }

    private func exitEditingMode() {
        isAddressFocused = false
        hasInputActivity = false
        updateDisplayText()
    }

    private func submitAddress() {
        let input = addressText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !input.isEmpty else { return }
        viewModel.navigateCurrentTab(to: input)
        exitEditingMode()
    }

    private func updateDisplayText() {
        guard !isAddressFocused else { return }
        addressText = displayText(for: viewModel.currentTab)
    }

    // MARK: - Right Actions

    private var rightActions: some View {
        HStack(spacing: 8) {
            // Bookmark button
            Button(action: { toggleBookmark() }) {
                Image(systemName: isBookmarked ? "star.fill" : "star")
                    .font(.system(size: 14, weight: .medium))
                    .frame(width: iconButtonSize, height: iconButtonSize)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .opacity(isBookmarked ? 1 : 0.5)

            // Refresh button
            Button(action: { viewModel.reloadCurrentTab() }) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 14, weight: .medium))
                    .frame(width: iconButtonSize, height: iconButtonSize)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)

            // More button
            Button(action: {}) {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .medium))
                    .frame(width: iconButtonSize, height: iconButtonSize)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
    }

    // MARK: - Background

    private func toolbarBackground(metrics: ToolbarLayoutMetrics) -> some View {
        // Content background (dark with top-left rounded corner only)
        UnevenRoundedRectangle(
            topLeadingRadius: 12,
            bottomLeadingRadius: 0,
            bottomTrailingRadius: 0,
            topTrailingRadius: 0  // roundedContainer mask handles right corner
        )
        .fill(AppColors.contentBackgroundColor)
        .padding(.leading, metrics.backgroundLeadingX)
        .allowsHitTesting(false)
    }

    // MARK: - Actions

    private var isBookmarked: Bool {
        guard let url = viewModel.currentTab?.url else { return false }
        return bookmarkVM.bookmarks.contains(where: { $0.appURL == url })
    }

    private func toggleSidebar() {
        onToggleSidebar()
    }

    private func toggleOverview() {
        NotificationCenter.default.post(name: .toggleFocusOverviewRequested, object: nil)
    }

    private func toggleBookmark() {
        guard let tab = viewModel.currentTab else { return }

        if isBookmarked {
            if let url = tab.url,
               let bookmark = bookmarkVM.bookmarks.first(where: { $0.appURL == url }) {
                bookmarkVM.removeBookmark(id: bookmark.id)
            }
        } else {
            bookmarkVM.addBookmark(from: tab)
        }
    }

    // MARK: - Buttons

    private var newTabButton: some View {
        Button(action: { _ = viewModel.addNewTab() }) {
            Image(systemName: "plus")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    private var overviewButton: some View {
        Button(action: { toggleOverview() }) {
            Image(systemName: "square.on.square")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    private var sidebarToggleButton: some View {
        Button(action: { toggleSidebar() }) {
            Image(systemName: "sidebar.left")
                .font(.system(size: iconPointSize, weight: .medium))
                .frame(width: iconButtonSize, height: iconButtonSize)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }

    // MARK: - Layout Metrics

    private struct ToolbarLayoutMetrics {
        let sidebarSectionWidth: CGFloat
        let trafficLightsReservedWidth: CGFloat
        let sidebarButtonsSpacing: CGFloat
        let sidebarButtonsTrailingInset: CGFloat
        let sidebarMinGap: CGFloat
        let backgroundLeadingX: CGFloat
        let contentLeadingX: CGFloat

        init(state: SidebarLayoutState) {
            let widthRef = max(640, state.windowWidth)
            let t = Self.normalized(widthRef, min: 780, max: 1680)
            let splitBoundaryX = max(0, state.splitBoundaryX)

            trafficLightsReservedWidth = state.isWindowFullScreen
                ? 0
                : Self.lerp(64, 76, t)
            sidebarButtonsSpacing = Self.lerp(8, 14, t)
            sidebarButtonsTrailingInset = Self.lerp(8, 20, t)
            sidebarMinGap = Self.lerp(8, 18, t)

            let collapsedWidth = state.collapsedToolbarWidth
            let sidebarButtonsWidth = (Self.toolbarButtonSize * 2) + sidebarButtonsSpacing
            let minCollapsedSidebarSectionWidth = trafficLightsReservedWidth
                + sidebarMinGap
                + sidebarButtonsWidth
                + sidebarButtonsTrailingInset
            sidebarSectionWidth = state.isSidebarCollapsed
                ? max(collapsedWidth, minCollapsedSidebarSectionWidth)
                : splitBoundaryX

            backgroundLeadingX = state.isSidebarCollapsed
                ? 0
                : splitBoundaryX

            // Keep ToggleSidebar anchored to background-leading + fixed inset,
            // while avoiding overlap with [+]/[Overview] section.
            let toggleInsetFromBackground = Self.lerp(10, 16, t)
            let minSeparationFromSidebarSection = Self.lerp(8, 14, t)
            contentLeadingX = max(
                backgroundLeadingX + toggleInsetFromBackground,
                sidebarSectionWidth + minSeparationFromSidebarSection
            )
        }

        private static func normalized(_ value: CGFloat, min: CGFloat, max: CGFloat) -> CGFloat {
            guard max > min else { return 0 }
            return Swift.max(0, Swift.min(1, (value - min) / (max - min)))
        }

        private static func lerp(_ start: CGFloat, _ end: CGFloat, _ t: CGFloat) -> CGFloat {
            start + (end - start) * t
        }

        private static let toolbarButtonSize: CGFloat = 30
    }
}

// MARK: - Address Input Field (NSViewRepresentable)

private struct SelfDrawnAddressInputField: NSViewRepresentable {
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
        // the race (window == nil → makeFirstResponder silently skipped) and
        // the field never gains focus. Retry until attached.
        SelfDrawnAddressInputField.focusWhenAttached(
            textField: textField,
            coordinator: context.coordinator,
            attempt: 0
        )

        return textField
    }

    private static func focusWhenAttached(
        textField: NSTextField,
        coordinator: Coordinator,
        attempt: Int
    ) {
        DispatchQueue.main.async {
            guard let window = textField.window else {
                if attempt < 30 {  // ~0.5s of runloop ticks, well within mount time
                    SelfDrawnAddressInputField.focusWhenAttached(
                        textField: textField,
                        coordinator: coordinator,
                        attempt: attempt + 1
                    )
                }
                return
            }
            let ok = window.makeFirstResponder(textField)
            NSLog("🩺 [SelfDrawnAddressInputField] focus after \(attempt) retry(ies): makeFR=%@",
                  ok ? "ok" : "FAIL")
            textField.currentEditor()?.selectAll(nil)
            coordinator.startObservingFirstResponder()
        }
    }

    func updateNSView(_ nsView: NSTextField, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: SelfDrawnAddressInputField
        weak var textField: NSTextField?
        private var firstResponderObservation: NSKeyValueObservation?

        init(_ parent: SelfDrawnAddressInputField) {
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
                NSLog("🩺 [SelfDrawnAddressInputField] KVO blur: firstResponder=%@",
                      String(describing: firstResponder))
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
            NSLog("🩺 [SelfDrawnAddressInputField] beginEditing: keyboard input reached field editor")
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
