import SwiftUI
import AppKit

// MARK: - Favicon Cache

/// Simple in-memory cache for favicons to avoid repeated network requests
/// Uses a thread-safe dictionary for synchronous access from main thread
private final class FaviconCache: @unchecked Sendable {
    static let shared = FaviconCache()

    private var cache: [String: NSImage] = [:]
    private let lock = NSLock()

    private init() {}

    /// Synchronous get - safe to call from main thread without await
    func getSync(_ host: String) -> NSImage? {
        lock.lock()
        defer { lock.unlock() }
        return cache[host]
    }

    /// Async get for compatibility
    func get(_ host: String) async -> NSImage? {
        getSync(host)
    }

    func set(_ host: String, image: NSImage) {
        lock.lock()
        defer { lock.unlock() }
        cache[host] = image
    }
}

// MARK: - Notification Names

extension Notification.Name {
    static let openNewWindowWithGroup = Notification.Name("openNewWindowWithGroup")
}

// MARK: - Outline Item Types

/// Represents an item in the outline view
enum TabOutlineItem: Equatable, Hashable {
    case group(TabGroup)
    case tab(Tab)

    var id: UUID {
        switch self {
        case .group(let group): return group.id
        case .tab(let tab): return tab.id
        }
    }

    static func == (lhs: TabOutlineItem, rhs: TabOutlineItem) -> Bool {
        lhs.id == rhs.id
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(id)
    }
}

// MARK: - Pasteboard Type

extension NSPasteboard.PasteboardType {
    static let tabOutlineItem = NSPasteboard.PasteboardType("com.actro.tabOutlineItem")
    static let tabCrossWindowData = NSPasteboard.PasteboardType("com.actro.tabCrossWindowData")
}

// Notification for cross-window tab transfer
extension Notification.Name {
    static let removeTabFromSourceWindow = Notification.Name("removeTabFromSourceWindow")
}

// Data structure for cross-window tab transfer
struct CrossWindowTabData: Codable {
    let sourceTabId: String
    let sourceViewModelId: String
    let title: String
    let urlString: String?
    let groupId: String?
    let unifiedScript: String?
    let capabilityManifestJSON: String?
    let permissions: [String]?
    let leftPanelScript: String?
    let rightPanelScript: String?
    let guestScript: String?
    let htmlContent: String?
    let backgroundColor: String?
}

// MARK: - NSViewRepresentable

struct TabOutlineView: NSViewRepresentable {
    @ObservedObject var viewModel: BrowserViewModel
    @ObservedObject var historyVM: HistoryViewModel

    func makeCoordinator() -> Coordinator {
        Coordinator(viewModel: viewModel, historyVM: historyVM)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSScrollView()
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = false

        let outlineView = NSOutlineView()
        outlineView.headerView = nil
        outlineView.style = .plain
        outlineView.backgroundColor = .clear  // Remove the default background
        // selectionHighlightStyle.sourceList is deprecated on macOS 12+. Outline style already provides source-list look.
        // outlineView.selectionHighlightStyle = .sourceList
        outlineView.rowSizeStyle = .default
        outlineView.indentationPerLevel = 16
        outlineView.indentationMarkerFollowsCell = true
        outlineView.autoresizesOutlineColumn = true
        outlineView.allowsMultipleSelection = false
        outlineView.floatsGroupRows = false
        outlineView.intercellSpacing = NSSize(width: 0, height: 10)

        // Setup column
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("TabColumn"))
        column.isEditable = false
        column.resizingMask = .autoresizingMask
        outlineView.addTableColumn(column)
        outlineView.outlineTableColumn = column

        // Set delegates
        outlineView.dataSource = context.coordinator
        outlineView.delegate = context.coordinator

        // Enable drag and drop (including cross-window)
        outlineView.registerForDraggedTypes([.tabOutlineItem, .tabCrossWindowData])
        outlineView.setDraggingSourceOperationMask(.move, forLocal: true)
        outlineView.setDraggingSourceOperationMask(.move, forLocal: false)  // Enable cross-window drag

        // Double click action
        outlineView.target = context.coordinator
        outlineView.doubleAction = #selector(Coordinator.doubleClickAction(_:))

        // Context menu
        let menu = NSMenu()
        menu.delegate = context.coordinator
        outlineView.menu = menu

        scrollView.documentView = outlineView
        context.coordinator.outlineView = outlineView

        return scrollView
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        context.coordinator.viewModel = viewModel
        context.coordinator.historyVM = historyVM
        context.coordinator.reloadData()
        context.coordinator.updateSelection()
    }

    // MARK: - Coordinator

    class Coordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate, NSMenuDelegate {
        var viewModel: BrowserViewModel
        var historyVM: HistoryViewModel
        weak var outlineView: NSOutlineView?

        private var isReloading = false
        private let viewModelId: String

        init(viewModel: BrowserViewModel, historyVM: HistoryViewModel) {
            self.viewModel = viewModel
            self.historyVM = historyVM
            self.viewModelId = ObjectIdentifier(viewModel).debugDescription
            super.init()

            // Listen for cross-window tab removal notifications
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(handleRemoveTabFromSource(_:)),
                name: .removeTabFromSourceWindow,
                object: nil
            )
        }

        deinit {
            NotificationCenter.default.removeObserver(self)
        }

        @objc private func handleRemoveTabFromSource(_ notification: Notification) {
            guard let userInfo = notification.userInfo,
                  let tabIdString = userInfo["tabId"] as? String,
                  let sourceViewModelId = userInfo["viewModelId"] as? String,
                  sourceViewModelId == viewModelId,
                  let tabId = UUID(uuidString: tabIdString) else {
                return
            }

            // This notification is for us - remove the tab (delayed to avoid SwiftUI update conflicts)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
                self?.viewModel.removeTabById(tabId)
                self?.reloadData()
            }
        }

        // MARK: - Data Management

        func reloadData() {
            guard !isReloading else { return }
            isReloading = true

            // Save expanded state
            var expandedGroupIds: Set<UUID> = []
            if let outlineView = outlineView {
                for groupData in viewModel.tabsOrganizedByGroups() {
                    if let group = groupData.group {
                        let item = TabOutlineItem.group(group)
                        if outlineView.isItemExpanded(item) {
                            expandedGroupIds.insert(group.id)
                        }
                    }
                }
            }

            outlineView?.reloadData()

            // Restore expanded state (expand by default unless collapsed)
            if let outlineView = outlineView {
                for groupData in viewModel.tabsOrganizedByGroups() {
                    if let group = groupData.group {
                        let item = TabOutlineItem.group(group)
                        if !group.isCollapsed {
                            outlineView.expandItem(item)
                        }
                    }
                }

                // Sync column width with the visible area so rows are
                // not clipped before the user resizes the sidebar.
                outlineView.sizeLastColumnToFit()
            }

            isReloading = false
        }

        func updateSelection() {
            guard let outlineView = outlineView,
                  let currentTabId = viewModel.currentTabId,
                  let currentTab = viewModel.tabs.first(where: { $0.id == currentTabId }) else {
                return
            }

            let item = TabOutlineItem.tab(currentTab)
            let row = outlineView.row(forItem: item)
            if row >= 0 && outlineView.selectedRow != row {
                outlineView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            }
        }

        // MARK: - NSOutlineViewDataSource

        func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
            if item == nil {
                // Root level: groups + ungrouped tabs
                return rootItems().count
            }

            if let outlineItem = item as? TabOutlineItem, case .group(let group) = outlineItem {
                return viewModel.tabs.filter { $0.groupId == group.id }.count
            }

            return 0
        }

        func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
            if item == nil {
                return rootItems()[index]
            }

            if let outlineItem = item as? TabOutlineItem, case .group(let group) = outlineItem {
                let groupTabs = viewModel.tabs.filter { $0.groupId == group.id }.reversed()
                return TabOutlineItem.tab(Array(groupTabs)[index])
            }

            return TabOutlineItem.tab(viewModel.tabs[0])
        }

        func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
            if let outlineItem = item as? TabOutlineItem, case .group = outlineItem {
                return true
            }
            return false
        }

        private func rootItems() -> [TabOutlineItem] {
            var items: [TabOutlineItem] = []
            var processedGroupIds: Set<UUID> = []

            // Reverse order: newest tabs at top
            for tab in viewModel.tabs.reversed() {
                if let groupId = tab.groupId {
                    if !processedGroupIds.contains(groupId) {
                        processedGroupIds.insert(groupId)
                        if let group = viewModel.tabGroups.first(where: { $0.id == groupId }) {
                            items.append(.group(group))
                        }
                    }
                } else {
                    items.append(.tab(tab))
                }
            }

            return items
        }

        // MARK: - NSOutlineViewDelegate

        func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
            guard let outlineItem = item as? TabOutlineItem else { return nil }

            switch outlineItem {
            case .group(let group):
                return makeGroupRowView(group: group, outlineView: outlineView)
            case .tab(let tab):
                let parentGroup = viewModel.tabGroups.first { $0.id == tab.groupId }
                return makeTabRowView(tab: tab, group: parentGroup, outlineView: outlineView)
            }
        }

        func outlineView(_ outlineView: NSOutlineView, heightOfRowByItem item: Any) -> CGFloat {
            return 28
        }

        func outlineView(_ outlineView: NSOutlineView, rowViewForItem item: Any) -> NSTableRowView? {
            return RoundedRowView()
        }

        func outlineView(_ outlineView: NSOutlineView, isGroupItem item: Any) -> Bool {
            return false // Don't use system group style, we customize it
        }

        func outlineViewSelectionDidChange(_ notification: Notification) {
            guard let outlineView = outlineView else { return }
            let selectedRow = outlineView.selectedRow
            guard selectedRow >= 0 else { return }

            if let item = outlineView.item(atRow: selectedRow) as? TabOutlineItem,
               case .tab(let tab) = item {
                if viewModel.currentTabId != tab.id {
                    viewModel.switchToTab(id: tab.id)
                }
            }
        }

        func outlineViewItemDidExpand(_ notification: Notification) {
            guard let item = notification.userInfo?["NSObject"] as? TabOutlineItem,
                  case .group(let group) = item else { return }

            if group.isCollapsed {
                viewModel.toggleGroupCollapsed(groupId: group.id)
            }
        }

        func outlineViewItemDidCollapse(_ notification: Notification) {
            guard let item = notification.userInfo?["NSObject"] as? TabOutlineItem,
                  case .group(let group) = item else { return }

            if !group.isCollapsed {
                viewModel.toggleGroupCollapsed(groupId: group.id)
            }
        }

        // MARK: - Drag and Drop

        func outlineView(_ outlineView: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
            guard let outlineItem = item as? TabOutlineItem,
                  case .tab(let tab) = outlineItem else {
                return nil // Only tabs can be dragged
            }

            let pasteboardItem = NSPasteboardItem()

            // Local tab ID for same-window drops
            pasteboardItem.setString(tab.id.uuidString, forType: .tabOutlineItem)

            // Serialized tab data for cross-window drops
            let tabData = CrossWindowTabData(
                sourceTabId: tab.id.uuidString,
                sourceViewModelId: ObjectIdentifier(viewModel).debugDescription,
                title: tab.title,
                urlString: tab.url?.absoluteString,
                groupId: tab.groupId?.uuidString,
                unifiedScript: tab.unifiedScript,
                capabilityManifestJSON: tab.capabilityManifestJSON,
                permissions: tab.permissions,
                leftPanelScript: tab.leftPanelScript,
                rightPanelScript: tab.rightPanelScript,
                guestScript: tab.guestScript,
                htmlContent: tab.htmlContent,
                backgroundColor: tab.backgroundColor
            )

            if let jsonData = try? JSONEncoder().encode(tabData),
               let jsonString = String(data: jsonData, encoding: .utf8) {
                pasteboardItem.setString(jsonString, forType: .tabCrossWindowData)
            }

            return pasteboardItem
        }

        func outlineView(_ outlineView: NSOutlineView, validateDrop info: NSDraggingInfo, proposedItem item: Any?, proposedChildIndex index: Int) -> NSDragOperation {
            // Get the dragged tab ID
            guard let pasteboardItem = info.draggingPasteboard.pasteboardItems?.first,
                  let tabIdString = pasteboardItem.string(forType: .tabOutlineItem),
                  let tabId = UUID(uuidString: tabIdString) else {
                return []
            }

            // Case 1: Dropping at root level (between groups/ungrouped tabs)
            if item == nil {
                // index >= 0 means inserting at a specific position
                // NSOutlineViewDropOnItemIndex means dropping "on" root (not useful here)
                return .move
            }

            // Case 2: Dropping on/into a group
            if let outlineItem = item as? TabOutlineItem, case .group(_) = outlineItem {
                // index == NSOutlineViewDropOnItemIndex: drop ON the group (add to group)
                // index >= 0: drop at specific position within group
                return .move
            }

            // Case 3: Dropping on a tab - retarget to proper location
            if let outlineItem = item as? TabOutlineItem, case .tab(let targetTab) = outlineItem {
                // Don't drop on self
                if targetTab.id == tabId {
                    return []
                }

                // Retarget: find where this tab is and set proper drop location
                if let groupId = targetTab.groupId,
                   let group = viewModel.tabGroups.first(where: { $0.id == groupId }) {
                    // Tab is in a group - retarget to group with position
                    let groupTabs = viewModel.tabs.filter { $0.groupId == groupId }
                    if let tabIndex = groupTabs.firstIndex(where: { $0.id == targetTab.id }) {
                        outlineView.setDropItem(TabOutlineItem.group(group), dropChildIndex: tabIndex)
                    }
                } else {
                    // Ungrouped tab - retarget to root with position
                    let rootItems = rootItems()
                    if let itemIndex = rootItems.firstIndex(where: {
                        if case .tab(let t) = $0 { return t.id == targetTab.id }
                        return false
                    }) {
                        outlineView.setDropItem(nil, dropChildIndex: itemIndex)
                    }
                }
                return .move
            }

            return .move
        }

        func outlineView(_ outlineView: NSOutlineView, acceptDrop info: NSDraggingInfo, item: Any?, childIndex index: Int) -> Bool {
            guard let pasteboardItem = info.draggingPasteboard.pasteboardItems?.first,
                  let tabIdString = pasteboardItem.string(forType: .tabOutlineItem),
                  let tabId = UUID(uuidString: tabIdString) else {
                return false
            }

            // Check if this is a local tab (same window)
            let isLocalTab = viewModel.tabs.contains(where: { $0.id == tabId })

            // Handle cross-window drop
            if !isLocalTab {
                guard let crossWindowDataString = pasteboardItem.string(forType: .tabCrossWindowData),
                      let crossWindowData = crossWindowDataString.data(using: .utf8),
                      let tabData = try? JSONDecoder().decode(CrossWindowTabData.self, from: crossWindowData) else {
                    return false
                }

                // Create new tab from cross-window data
                let newTab = Tab(
                    title: tabData.title,
                    url: tabData.urlString.flatMap { URL(string: $0) },
                    htmlContent: tabData.htmlContent,
                    guestScript: tabData.guestScript,
                    leftPanelScript: tabData.leftPanelScript,
                    rightPanelScript: tabData.rightPanelScript,
                    unifiedScript: tabData.unifiedScript,
                    capabilityManifestJSON: tabData.capabilityManifestJSON,
                    permissions: tabData.permissions ?? [],
                    backgroundColor: tabData.backgroundColor
                )

                // Determine target group
                var targetGroupId: UUID? = nil
                if let outlineItem = item as? TabOutlineItem, case .group(let group) = outlineItem {
                    targetGroupId = group.id
                }

                // Calculate target index
                let targetIndex = calculateDropTargetIndex(item: item, childIndex: index, targetGroupId: targetGroupId)

                // Insert the new tab (delayed to avoid SwiftUI update conflicts)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
                    self?.viewModel.insertTabFromCrossWindow(newTab, at: targetIndex, groupId: targetGroupId)
                    self?.reloadData()
                }

                // Notify source window to remove the tab (slightly more delayed)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                    NotificationCenter.default.post(
                        name: .removeTabFromSourceWindow,
                        object: nil,
                        userInfo: [
                            "tabId": tabData.sourceTabId,
                            "viewModelId": tabData.sourceViewModelId
                        ]
                    )
                }

                return true
            }

            // Local drop - existing logic
            guard let fromIndex = viewModel.tabs.firstIndex(where: { $0.id == tabId }) else {
                return false
            }

            // Case 1: Dropping at root level
            if item == nil {
                var targetIndex: Int

                if index == NSOutlineViewDropOnItemIndex || index < 0 {
                    // Drop at end
                    targetIndex = viewModel.tabs.count
                } else {
                    // Calculate target index based on rootItems position
                    targetIndex = calculateRootInsertIndex(atChildIndex: index)
                }

                // Adjust for the tab being moved (if moving forward)
                if fromIndex < targetIndex {
                    targetIndex -= 1
                }

                // Clamp to valid range
                targetIndex = max(0, min(targetIndex, viewModel.tabs.count - 1))

                viewModel.moveTabToPosition(tabId: tabId, toIndex: targetIndex, groupId: nil)
                return true
            }

            // Case 2: Dropping on/into a group
            if let outlineItem = item as? TabOutlineItem, case .group(let group) = outlineItem {
                var targetIndex: Int

                if index == NSOutlineViewDropOnItemIndex || index < 0 {
                    // Drop ON group - add at end of group
                    if let lastTabIndex = viewModel.tabs.lastIndex(where: { $0.groupId == group.id }) {
                        targetIndex = lastTabIndex + 1
                    } else {
                        // Empty group - find where group should be inserted
                        targetIndex = viewModel.tabs.count
                    }
                } else {
                    // Drop at specific position within group
                    let groupTabs = viewModel.tabs.enumerated().filter { $0.element.groupId == group.id }
                    if index < groupTabs.count {
                        targetIndex = groupTabs[index].offset
                    } else if let lastTab = groupTabs.last {
                        targetIndex = lastTab.offset + 1
                    } else {
                        targetIndex = viewModel.tabs.count
                    }
                }

                // Adjust for the tab being moved (if moving forward)
                if fromIndex < targetIndex {
                    targetIndex -= 1
                }

                // Clamp to valid range
                targetIndex = max(0, min(targetIndex, viewModel.tabs.count - 1))

                viewModel.moveTabToPosition(tabId: tabId, toIndex: targetIndex, groupId: group.id)
                return true
            }

            return false
        }

        /// Calculate target index for cross-window drop
        private func calculateDropTargetIndex(item: Any?, childIndex index: Int, targetGroupId: UUID?) -> Int {
            if item == nil {
                // Root level drop
                if index == NSOutlineViewDropOnItemIndex || index < 0 {
                    return viewModel.tabs.count
                } else {
                    return calculateRootInsertIndex(atChildIndex: index)
                }
            }

            if let groupId = targetGroupId {
                if index == NSOutlineViewDropOnItemIndex || index < 0 {
                    // Drop ON group - add at end
                    if let lastTabIndex = viewModel.tabs.lastIndex(where: { $0.groupId == groupId }) {
                        return lastTabIndex + 1
                    }
                    return viewModel.tabs.count
                } else {
                    // Drop at specific position within group
                    let groupTabs = viewModel.tabs.enumerated().filter { $0.element.groupId == groupId }
                    if index < groupTabs.count {
                        return groupTabs[index].offset
                    } else if let lastTab = groupTabs.last {
                        return lastTab.offset + 1
                    }
                    return viewModel.tabs.count
                }
            }

            return viewModel.tabs.count
        }

        /// Calculate the actual tabs array index for inserting at a root-level child position
        private func calculateRootInsertIndex(atChildIndex childIndex: Int) -> Int {
            let items = rootItems()

            // If childIndex is beyond items count, insert at end
            guard childIndex < items.count else {
                return viewModel.tabs.count
            }

            let targetItem = items[childIndex]

            switch targetItem {
            case .tab(let tab):
                // Find this tab's position in the tabs array
                if let tabIndex = viewModel.tabs.firstIndex(where: { $0.id == tab.id }) {
                    return tabIndex
                }
            case .group(let group):
                // Find the first tab of this group in the tabs array
                if let firstGroupTabIndex = viewModel.tabs.firstIndex(where: { $0.groupId == group.id }) {
                    return firstGroupTabIndex
                }
            }

            return viewModel.tabs.count
        }

        // MARK: - Double Click

        @objc func doubleClickAction(_ sender: NSOutlineView) {
            let clickedRow = sender.clickedRow

            // Double-click on empty space: create new tab
            if clickedRow < 0 {
                _ = viewModel.addNewTab()
                return
            }

            if let item = sender.item(atRow: clickedRow) as? TabOutlineItem {
                switch item {
                case .group(let group):
                    // Show group menu
                    showGroupEditMenu(for: group, at: clickedRow, in: sender)
                case .tab(let tab):
                    // Switch to tab
                    viewModel.switchToTab(id: tab.id)
                }
            }
        }

        private func showGroupEditMenu(for group: TabGroup, at row: Int, in outlineView: NSOutlineView) {
            let menu = NSMenu()
            menu.autoenablesItems = false

            // Header view with name field and color picker
            let headerView = GroupMenuHeaderView(group: group, viewModel: viewModel, outlineView: outlineView)
            let headerItem = NSMenuItem()
            headerItem.view = headerView
            menu.addItem(headerItem)

            menu.addItem(NSMenuItem.separator())

            // New Tab in Group
            let newTabItem = NSMenuItem(title: "New Tab in Group", action: #selector(newTabInGroup(_:)), keyEquivalent: "c")
            newTabItem.keyEquivalentModifierMask = [.control, .command]
            newTabItem.target = self
            newTabItem.representedObject = group
            newTabItem.image = NSImage(systemSymbolName: "plus.rectangle", accessibilityDescription: nil)
            menu.addItem(newTabItem)

            // Move Group to New Window
            let moveItem = NSMenuItem(title: "Move Group to New Window", action: #selector(moveGroupToNewWindow(_:)), keyEquivalent: "")
            moveItem.target = self
            moveItem.representedObject = group
            moveItem.image = NSImage(systemSymbolName: "rectangle.on.rectangle", accessibilityDescription: nil)
            // Disable if this group contains all tabs in the window
            let groupTabCount = viewModel.tabs.filter { $0.groupId == group.id }.count
            moveItem.isEnabled = groupTabCount < viewModel.tabs.count
            menu.addItem(moveItem)

            // Close Group
            let closeItem = NSMenuItem(title: "Close Group", action: #selector(closeGroupFromMenu(_:)), keyEquivalent: "w")
            closeItem.keyEquivalentModifierMask = [.control, .command]
            closeItem.target = self
            closeItem.representedObject = group
            closeItem.image = NSImage(systemSymbolName: "xmark.rectangle", accessibilityDescription: nil)
            menu.addItem(closeItem)

            menu.addItem(NSMenuItem.separator())

            // Fold / Unfold
            if group.isCollapsed {
                let unfoldItem = NSMenuItem(title: "Unfold Group", action: #selector(unfoldGroup(_:)), keyEquivalent: "")
                unfoldItem.target = self
                unfoldItem.representedObject = group
                unfoldItem.image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)
                menu.addItem(unfoldItem)
            } else {
                let foldItem = NSMenuItem(title: "Fold Group", action: #selector(foldGroup(_:)), keyEquivalent: "")
                foldItem.target = self
                foldItem.representedObject = group
                foldItem.image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)
                menu.addItem(foldItem)
            }

            menu.addItem(NSMenuItem.separator())

            // Dissolve Group
            let dissolveItem = NSMenuItem(title: "Dissolve Group", action: #selector(ungroupFromMenu(_:)), keyEquivalent: "")
            dissolveItem.target = self
            dissolveItem.representedObject = group
            dissolveItem.image = NSImage(systemSymbolName: "rectangle.3.group", accessibilityDescription: nil)
            menu.addItem(dissolveItem)

            // Show menu at mouse location (same as right-click)
            let mouseLocation = outlineView.window?.mouseLocationOutsideOfEventStream ?? .zero
            let localPoint = outlineView.convert(mouseLocation, from: nil)
            menu.popUp(positioning: nil, at: localPoint, in: outlineView)
        }

        @objc private func newTabInGroup(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup else { return }
            let newTab = viewModel.addNewTab()
            viewModel.addTabToGroup(tabId: newTab.id, groupId: group.id)
        }

        @objc private func moveGroupToNewWindow(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup else { return }

            // Get tabs in this group
            let groupTabs = viewModel.tabs.filter { $0.groupId == group.id }
            guard !groupTabs.isEmpty else { return }

            // Serialize full tab data for transfer
            let tabsData: [[String: Any]] = groupTabs.map { tab in
                var data: [String: Any] = [
                    "title": tab.title,
                ]
                if let appId = tab.appId { data["appId"] = appId }
                if tab.legacyActorBridge { data["legacyActorBridge"] = true }
                if let url = tab.url?.absoluteString { data["urlString"] = url }
                if let systemType = tab.systemTabType { data["systemTabType"] = systemType.rawValue }
                if let script = tab.unifiedScript { data["unifiedScript"] = script }
                if let manifest = tab.capabilityManifestJSON {
                    data["capabilityManifestJSON"] = manifest
                }
                if let script = tab.leftPanelScript { data["leftPanelScript"] = script }
                if let script = tab.rightPanelScript { data["rightPanelScript"] = script }
                if let script = tab.guestScript { data["guestScript"] = script }
                if let html = tab.htmlContent { data["htmlContent"] = html }
                data["backgroundColor"] = tab.backgroundColor
                if let name = tab.contractName { data["contractName"] = name }
                if let ver = tab.contractVersion { data["contractVersion"] = ver }
                if !tab.permissions.isEmpty { data["permissions"] = tab.permissions }
                data["rightPanelDefaultVisible"] = tab.rightPanelDefaultVisible
                return data
            }

            let groupData: [String: Any] = [
                "groupName": group.name,
                "groupColor": group.color.rawValue,
                "tabs": tabsData
            ]

            // Save to UserDefaults for new window to pick up
            UserDefaults.standard.set(groupData, forKey: "pendingGroupTransfer")

            // Post notification to open new window
            NotificationCenter.default.post(name: .openNewWindowWithGroup, object: nil)

            // Remove group from current window
            viewModel.closeGroup(groupId: group.id, historyVM: historyVM)
        }

        @objc private func closeGroupFromMenu(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup else { return }
            viewModel.closeGroup(groupId: group.id, historyVM: historyVM)
        }

        @objc private func ungroupFromMenu(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup else { return }
            viewModel.deleteGroup(groupId: group.id)
        }

        @objc private func deleteGroupFromMenu(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup else { return }
            // Close all tabs in group and delete the group
            let tabsInGroup = viewModel.tabs.filter { $0.groupId == group.id }
            for tab in tabsInGroup {
                viewModel.closeTab(id: tab.id)
            }
            viewModel.deleteGroup(groupId: group.id)
        }

        @objc private func foldGroup(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup,
                  let outlineView = outlineView else { return }

            // Find the item in the outline view
            for row in 0..<outlineView.numberOfRows {
                if let item = outlineView.item(atRow: row) as? TabOutlineItem,
                   case .group(let g) = item, g.id == group.id {
                    outlineView.collapseItem(item)
                    break
                }
            }
        }

        @objc private func unfoldGroup(_ sender: NSMenuItem) {
            guard let group = sender.representedObject as? TabGroup,
                  let outlineView = outlineView else { return }

            // Find the item in the outline view
            for row in 0..<outlineView.numberOfRows {
                if let item = outlineView.item(atRow: row) as? TabOutlineItem,
                   case .group(let g) = item, g.id == group.id {
                    outlineView.expandItem(item)
                    break
                }
            }
        }

        // MARK: - Row Views

        private func makeGroupRowView(group: TabGroup, outlineView: NSOutlineView) -> NSView {
            let identifier = NSUserInterfaceItemIdentifier("GroupRow")

            // Always create a new view to avoid reuse issues
            let rowView = NSView()
            rowView.identifier = identifier

            let stackView = NSStackView()
            stackView.orientation = .horizontal
            stackView.spacing = 6
            stackView.alignment = .centerY
            stackView.translatesAutoresizingMaskIntoConstraints = false

            // Color indicator
            let colorDot = NSView()
            colorDot.wantsLayer = true
            colorDot.layer?.backgroundColor = group.color.nsColor.cgColor
            colorDot.layer?.cornerRadius = 5
            colorDot.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                colorDot.widthAnchor.constraint(equalToConstant: 10),
                colorDot.heightAnchor.constraint(equalToConstant: 10)
            ])

            // Group name
            let nameLabel = NSTextField(labelWithString: group.name.isEmpty ? "Group" : group.name)
            nameLabel.font = NSFont.systemFont(ofSize: 12, weight: .medium)
            nameLabel.textColor = .labelColor
            nameLabel.lineBreakMode = .byTruncatingTail

            // Tab count
            let tabCount = viewModel.tabs.filter { $0.groupId == group.id }.count
            let countLabel = NSTextField(labelWithString: "\(tabCount)")
            countLabel.font = NSFont.systemFont(ofSize: 10)
            countLabel.textColor = .secondaryLabelColor
            countLabel.alignment = .center
            countLabel.wantsLayer = true
            countLabel.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
            countLabel.layer?.cornerRadius = 4

            stackView.addArrangedSubview(colorDot)
            stackView.addArrangedSubview(nameLabel)
            stackView.addArrangedSubview(NSView()) // Spacer
            stackView.addArrangedSubview(countLabel)

            rowView.addSubview(stackView)
            NSLayoutConstraint.activate([
                stackView.leadingAnchor.constraint(equalTo: rowView.leadingAnchor, constant: 4),
                stackView.trailingAnchor.constraint(equalTo: rowView.trailingAnchor, constant: -8),
                stackView.centerYAnchor.constraint(equalTo: rowView.centerYAnchor)
            ])

            return rowView
        }

        private func makeTabRowView(tab: Tab, group: TabGroup?, outlineView: NSOutlineView) -> NSView {
            let isSelected = viewModel.currentTabId == tab.id

            // Use custom TabRowView for hover tracking and close button
            let rowView = TabRowView(
                tab: tab,
                group: group,
                isSelected: isSelected,
                viewModel: viewModel
            )

            return rowView
        }

        // MARK: - NSMenuDelegate

        func menuNeedsUpdate(_ menu: NSMenu) {
            menu.removeAllItems()

            guard let outlineView = outlineView else { return }
            let clickedRow = outlineView.clickedRow

            // Empty space context menu
            if clickedRow < 0 {
                buildEmptySpaceMenu(menu)
                return
            }

            guard let item = outlineView.item(atRow: clickedRow) as? TabOutlineItem else {
                buildEmptySpaceMenu(menu)
                return
            }

            switch item {
            case .group(let group):
                // Header view with name field and color picker
                let headerView = GroupMenuHeaderView(group: group, viewModel: viewModel, outlineView: outlineView)
                let headerItem = NSMenuItem()
                headerItem.view = headerView
                menu.addItem(headerItem)

                menu.addItem(NSMenuItem.separator())

                // New Tab in Group
                let newTabItem = NSMenuItem(title: "New Tab in Group", action: #selector(newTabInGroup(_:)), keyEquivalent: "c")
                newTabItem.keyEquivalentModifierMask = [.control, .command]
                newTabItem.target = self
                newTabItem.representedObject = group
                newTabItem.image = NSImage(systemSymbolName: "plus.rectangle", accessibilityDescription: nil)
                menu.addItem(newTabItem)

                // Move Group to New Window
                let moveItem = NSMenuItem(title: "Move Group to New Window", action: #selector(moveGroupToNewWindow(_:)), keyEquivalent: "")
                moveItem.target = self
                moveItem.representedObject = group
                moveItem.image = NSImage(systemSymbolName: "rectangle.on.rectangle", accessibilityDescription: nil)
                // Disable if this group contains all tabs in the window
                let groupTabCount = viewModel.tabs.filter { $0.groupId == group.id }.count
                moveItem.isEnabled = groupTabCount < viewModel.tabs.count
                menu.addItem(moveItem)

                // Close Group
                let closeItem = NSMenuItem(title: "Close Group", action: #selector(closeGroupFromMenu(_:)), keyEquivalent: "w")
                closeItem.keyEquivalentModifierMask = [.control, .command]
                closeItem.target = self
                closeItem.representedObject = group
                closeItem.image = NSImage(systemSymbolName: "xmark.rectangle", accessibilityDescription: nil)
                menu.addItem(closeItem)

                menu.addItem(NSMenuItem.separator())

                // Fold / Unfold
                if group.isCollapsed {
                    let unfoldItem = NSMenuItem(title: "Unfold Group", action: #selector(unfoldGroup(_:)), keyEquivalent: "")
                    unfoldItem.target = self
                    unfoldItem.representedObject = group
                    unfoldItem.image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)
                    menu.addItem(unfoldItem)
                } else {
                    let foldItem = NSMenuItem(title: "Fold Group", action: #selector(foldGroup(_:)), keyEquivalent: "")
                    foldItem.target = self
                    foldItem.representedObject = group
                    foldItem.image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)
                    menu.addItem(foldItem)
                }

                menu.addItem(NSMenuItem.separator())

                // Dissolve Group
                let dissolveItem = NSMenuItem(title: "Dissolve Group", action: #selector(ungroupFromMenu(_:)), keyEquivalent: "")
                dissolveItem.target = self
                dissolveItem.representedObject = group
                dissolveItem.image = NSImage(systemSymbolName: "rectangle.3.group", accessibilityDescription: nil)
                menu.addItem(dissolveItem)

            case .tab(let tab):
                if tab.groupId != nil {
                    // Tab is in a group - show remove option
                    let removeItem = NSMenuItem(title: "Remove from Group", action: #selector(removeFromGroup(_:)), keyEquivalent: "")
                    removeItem.target = self
                    removeItem.image = NSImage(systemSymbolName: "rectangle.badge.minus", accessibilityDescription: nil)
                    menu.addItem(removeItem)
                } else {
                    // Tab is not in a group - show create group option
                    let createGroupItem = NSMenuItem(title: "Create New Group", action: #selector(createNewGroup(_:)), keyEquivalent: "")
                    createGroupItem.target = self
                    createGroupItem.image = NSImage(systemSymbolName: "rectangle.stack.badge.plus", accessibilityDescription: nil)
                    menu.addItem(createGroupItem)
                }

                menu.addItem(NSMenuItem.separator())

                let closeTabItem = NSMenuItem(title: "Close Tab", action: #selector(closeTab(_:)), keyEquivalent: "")
                closeTabItem.target = self
                closeTabItem.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: nil)
                menu.addItem(closeTabItem)
            }
        }

        // MARK: - Context Menu Actions

        @objc private func renameGroup(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .group(let group) = item else { return }

            showGroupEditMenu(for: group, at: row, in: outlineView)
        }

        @objc private func changeGroupColor(_ sender: NSMenuItem) {
            guard let outlineView = outlineView,
                  let color = sender.representedObject as? TabGroupColor else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .group(let group) = item else { return }

            viewModel.updateGroup(groupId: group.id, color: color)
        }

        @objc private func ungroupTabs(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .group(let group) = item else { return }

            viewModel.deleteGroup(groupId: group.id)
        }

        @objc private func closeGroup(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .group(let group) = item else { return }

            viewModel.closeGroup(groupId: group.id, historyVM: historyVM)
        }

        @objc private func removeFromGroup(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .tab(let tab) = item else { return }

            viewModel.removeTabFromGroup(tabId: tab.id)
        }

        @objc private func createNewGroup(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .tab(let tab) = item else { return }

            viewModel.createGroupFromTabs([tab.id], name: "", color: .gray)
        }

        @objc private func closeTab(_ sender: Any) {
            guard let outlineView = outlineView else { return }
            let row = outlineView.clickedRow
            guard row >= 0,
                  let item = outlineView.item(atRow: row) as? TabOutlineItem,
                  case .tab(let tab) = item else { return }

            viewModel.closeTab(id: tab.id)
        }

        // MARK: - Empty Space Context Menu

        private func buildEmptySpaceMenu(_ menu: NSMenu) {
            let newTabItem = NSMenuItem(title: "New Tab", action: #selector(newTabFromEmptySpace(_:)), keyEquivalent: "t")
            newTabItem.keyEquivalentModifierMask = .command
            newTabItem.target = self
            newTabItem.image = NSImage(systemSymbolName: "plus", accessibilityDescription: nil)
            menu.addItem(newTabItem)

            let reopenItem = NSMenuItem(title: "Reopen Closed Tab", action: #selector(reopenClosedTab(_:)), keyEquivalent: "")
            reopenItem.target = self
            reopenItem.image = NSImage(systemSymbolName: "arrow.uturn.backward", accessibilityDescription: nil)
            reopenItem.isEnabled = !historyVM.recentItems.isEmpty
            menu.addItem(reopenItem)

            menu.addItem(NSMenuItem.separator())

            let bookmarkAllItem = NSMenuItem(title: "Bookmark All Tabs...", action: #selector(bookmarkAllTabs(_:)), keyEquivalent: "")
            bookmarkAllItem.target = self
            bookmarkAllItem.image = NSImage(systemSymbolName: "bookmark", accessibilityDescription: nil)
            bookmarkAllItem.isEnabled = !viewModel.tabs.isEmpty
            menu.addItem(bookmarkAllItem)

            menu.addItem(NSMenuItem.separator())

            let newGroupItem = NSMenuItem(title: "New Tab Group", action: #selector(newEmptyGroup(_:)), keyEquivalent: "")
            newGroupItem.target = self
            newGroupItem.image = NSImage(systemSymbolName: "rectangle.stack.badge.plus", accessibilityDescription: nil)
            menu.addItem(newGroupItem)
        }

        @objc private func newTabFromEmptySpace(_ sender: Any) {
            _ = viewModel.addNewTab()
        }

        @objc private func reopenClosedTab(_ sender: Any) {
            // Reopen the most recent closed tab from history
            if let recentItem = historyVM.recentItems.first,
               let url = recentItem.appURL {
                viewModel.importACTRPackage(from: url)
                historyVM.recordVisit(item: recentItem)
            }
        }

        @objc private func bookmarkAllTabs(_ sender: Any) {
            // Open bookmarks panel
            WindowStateManager.shared.openBookmarks()
        }

        @objc private func newEmptyGroup(_ sender: Any) {
            // Create an empty group with a new tab
            let newTab = viewModel.addNewTab()
            viewModel.createGroupFromTabs([newTab.id], name: "", color: .gray)
        }
    }
}

// MARK: - Group Menu Header View

private class GroupMenuHeaderView: NSView, NSTextFieldDelegate {
    private let group: TabGroup
    private let viewModel: BrowserViewModel
    private weak var outlineView: NSOutlineView?

    private var nameTextField: NSTextField!
    private var colorViews: [ColorCircleView] = []

    init(group: TabGroup, viewModel: BrowserViewModel, outlineView: NSOutlineView?) {
        self.group = group
        self.viewModel = viewModel
        self.outlineView = outlineView
        // Match menu width, single row of 9 colors (height: 12+22+10+22+10=76, use 80)
        super.init(frame: NSRect(x: 0, y: 0, width: 290, height: 80))
        setupViews()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func setupViews() {
        // Name text field
        nameTextField = NSTextField(string: group.name)
        nameTextField.placeholderString = "Name This Group"
        nameTextField.font = NSFont.systemFont(ofSize: 13)
        nameTextField.bezelStyle = .roundedBezel
        nameTextField.translatesAutoresizingMaskIntoConstraints = false
        nameTextField.delegate = self
        addSubview(nameTextField)

        // Color picker - single row of 9 colors matching input box width
        let colorContainer = NSStackView()
        colorContainer.orientation = .horizontal
        colorContainer.spacing = 0  // Use distribution to handle spacing
        colorContainer.distribution = .equalSpacing
        colorContainer.translatesAutoresizingMaskIntoConstraints = false

        let allColors = TabGroupColor.allCases
        let circleSize: CGFloat = 22

        for color in allColors {
            let colorView = ColorCircleView(color: color, isSelected: color == group.color)
            colorView.tag = allColors.firstIndex(of: color) ?? 0
            colorView.target = self
            colorView.action = #selector(colorSelected(_:))

            colorContainer.addArrangedSubview(colorView)
            colorViews.append(colorView)

            NSLayoutConstraint.activate([
                colorView.widthAnchor.constraint(equalToConstant: circleSize),
                colorView.heightAnchor.constraint(equalToConstant: circleSize)
            ])
        }

        addSubview(colorContainer)

        NSLayoutConstraint.activate([
            nameTextField.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            nameTextField.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            nameTextField.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            nameTextField.heightAnchor.constraint(equalToConstant: 22),

            colorContainer.topAnchor.constraint(equalTo: nameTextField.bottomAnchor, constant: 10),
            colorContainer.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            colorContainer.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            colorContainer.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10)
        ])
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        // Focus on text field when menu appears
        DispatchQueue.main.async { [weak self] in
            self?.nameTextField.becomeFirstResponder()
            self?.nameTextField.selectText(nil)
        }
    }

    // MARK: - NSTextFieldDelegate

    func controlTextDidChange(_ obj: Notification) {
        // Real-time sync: update group name as user types
        viewModel.updateGroup(groupId: group.id, name: nameTextField.stringValue)
        // Note: Don't call reloadData here as menu is open
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(insertNewline(_:)) {
            // Apply and close menu
            viewModel.updateGroup(groupId: group.id, name: nameTextField.stringValue)
            if let menu = self.enclosingMenuItem?.menu {
                menu.cancelTracking()
            }
            return true
        }
        return false
    }

    @objc private func colorSelected(_ sender: ColorCircleView) {
        let colorIndex = sender.tag
        guard colorIndex < TabGroupColor.allCases.count else { return }
        let selectedColor = TabGroupColor.allCases[colorIndex]

        // Update selection UI
        for (index, colorView) in colorViews.enumerated() {
            colorView.isSelected = (index == colorIndex)
        }

        viewModel.updateGroup(groupId: group.id, color: selectedColor)
        // Note: Don't call reloadData here as menu is open
    }
}

// MARK: - Color Circle View

private class ColorCircleView: NSView {
    var color: TabGroupColor
    var isSelected: Bool {
        didSet {
            needsDisplay = true
        }
    }
    var colorTag: Int = 0

    weak var target: AnyObject?
    var action: Selector?

    override var tag: Int {
        get { colorTag }
        set { colorTag = newValue }
    }

    init(color: TabGroupColor, isSelected: Bool) {
        self.color = color
        self.isSelected = isSelected
        super.init(frame: NSRect(x: 0, y: 0, width: 18, height: 18))
        wantsLayer = true
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)

        let circleRect = bounds.insetBy(dx: 2, dy: 2)

        // Draw filled circle
        let circlePath = NSBezierPath(ovalIn: circleRect)
        color.nsColor.setFill()
        circlePath.fill()

        // Draw selection ring
        if isSelected {
            let ringRect = bounds.insetBy(dx: 1, dy: 1)
            let ringPath = NSBezierPath(ovalIn: ringRect)
            ringPath.lineWidth = 2
            NSColor.labelColor.setStroke()
            ringPath.stroke()
        }
    }

    override func mouseDown(with event: NSEvent) {
        if let target = target, let action = action {
            NSApp.sendAction(action, to: target, from: self)
        }
    }

    // Enable mouse tracking
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas {
            removeTrackingArea(area)
        }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .activeInKeyWindow],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
    }

    override func mouseEntered(with event: NSEvent) {
        NSCursor.pointingHand.push()
    }

    override func mouseExited(with event: NSEvent) {
        NSCursor.pop()
    }
}

// MARK: - Tab Row View with Hover Close Button

// MARK: - Rounded Selection Row View

private class RoundedRowView: NSTableRowView {
    private var isHovering = false

    /// Content rect excluding intercellSpacing
    private var contentRect: NSRect {
        let spacing = (superview as? NSTableView)?.intercellSpacing.height ?? 0
        let half = floor(spacing / 2) - 3
        return bounds.insetBy(dx: 0, dy: half)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self
        ))
    }

    override func mouseEntered(with event: NSEvent) {
        isHovering = true
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        isHovering = false
        needsDisplay = true
    }

    override func drawBackground(in dirtyRect: NSRect) {
        if !isSelected && isHovering {
            let path = NSBezierPath(roundedRect: contentRect, xRadius: 8, yRadius: 8)
            NSColor.white.withAlphaComponent(0.06).setFill()
            path.fill()
        }
    }

    override func drawSelection(in dirtyRect: NSRect) {
        guard isSelected else { return }
        let path = NSBezierPath(roundedRect: contentRect, xRadius: 8, yRadius: 8)
        NSColor.controlAccentColor.setFill()
        path.fill()
    }
}

private class TabRowView: NSView {
    private let tab: Tab
    private let group: TabGroup?
    private let isSelected: Bool
    private weak var viewModel: BrowserViewModel?

    private var closeButton: NSButton!
    private var isHovering = false {
        didSet {
            updateCloseButtonVisibility()
        }
    }

    init(tab: Tab, group: TabGroup?, isSelected: Bool, viewModel: BrowserViewModel) {
        self.tab = tab
        self.group = group
        self.isSelected = isSelected
        self.viewModel = viewModel
        super.init(frame: .zero)
        setupViews()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()

        // Remove existing tracking areas
        for area in trackingAreas {
            removeTrackingArea(area)
        }

        // Add new tracking area with current bounds
        let trackingArea = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(trackingArea)
    }

    private func setupViews() {
        // Group color bar (if in a group)
        let colorBarWidth: CGFloat = group != nil ? 2 : 0
        let colorBarLeading: CGFloat = 4

        if let group = group {
            let colorBar = NSView()
            colorBar.wantsLayer = true
            colorBar.layer?.backgroundColor = group.color.nsColor.cgColor
            colorBar.translatesAutoresizingMaskIntoConstraints = false
            addSubview(colorBar)
            NSLayoutConstraint.activate([
                colorBar.leadingAnchor.constraint(equalTo: leadingAnchor, constant: colorBarLeading),
                colorBar.widthAnchor.constraint(equalToConstant: colorBarWidth),
                colorBar.topAnchor.constraint(equalTo: topAnchor),
                colorBar.bottomAnchor.constraint(equalTo: bottomAnchor)
            ])
        }

        let stackView = NSStackView()
        stackView.orientation = .horizontal
        stackView.spacing = 8
        stackView.alignment = .centerY
        stackView.translatesAutoresizingMaskIntoConstraints = false

        // Icon - show favicon for web tabs, default icon for others
        let iconView = NSImageView()
        iconView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            iconView.widthAnchor.constraint(equalToConstant: 16),
            iconView.heightAnchor.constraint(equalToConstant: 16)
        ])

        // Try to get favicon from cache synchronously for web tabs
        var usedCachedFavicon = false
        if case .web(let url) = tab.contentType, let host = url.host {
            // Synchronous cache check - avoids flicker when favicon is already cached!
            if let cachedImage = FaviconCache.shared.getSync(host) {
                iconView.image = cachedImage
                iconView.contentTintColor = nil
                usedCachedFavicon = true
            }
        }

        // Set default icon if no cached favicon was found
        if !usedCachedFavicon {
            if let appIcon = tab.appIcon,
               let image = NSImage(systemSymbolName: appIcon, accessibilityDescription: nil) {
                iconView.image = image
                iconView.contentTintColor = isSelected ? .controlAccentColor : .secondaryLabelColor
            } else if let systemType = tab.systemTabType,
                      let image = NSImage(systemSymbolName: systemType.icon, accessibilityDescription: nil) {
                iconView.image = image
                iconView.contentTintColor = isSelected ? .controlAccentColor : .secondaryLabelColor
            } else {
                iconView.image = NSImage(systemSymbolName: isSelected ? "doc.text.fill" : "doc.text", accessibilityDescription: nil)
                iconView.contentTintColor = isSelected ? .controlAccentColor : .secondaryLabelColor
            }
        }

        // Fetch favicon asynchronously for web tabs if not cached
        if case .web(let url) = tab.contentType, let host = url.host, !usedCachedFavicon {
            Task {
                guard let faviconURL = URL(string: "https://www.google.com/s2/favicons?domain=\(host)&sz=32") else { return }
                do {
                    let (data, _) = try await URLSession.shared.data(from: faviconURL)
                    if let image = NSImage(data: data) {
                        FaviconCache.shared.set(host, image: image)
                        await MainActor.run {
                            iconView.image = image
                            iconView.contentTintColor = nil
                        }
                    }
                } catch {
                    // Keep default icon on failure
                }
            }
        }

        // Title
        let titleLabel = NSTextField(labelWithString: tab.title)
        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: isSelected ? .medium : .regular)
        titleLabel.textColor = .labelColor
        titleLabel.lineBreakMode = .byTruncatingTail
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        stackView.addArrangedSubview(iconView)
        stackView.addArrangedSubview(titleLabel)

        addSubview(stackView)

        // Close button - Safari style (x mark in circle)
        closeButton = NSButton(frame: .zero)
        closeButton.bezelStyle = .inline
        closeButton.isBordered = false
        closeButton.image = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Close tab")
        closeButton.imageScaling = .scaleProportionallyUpOrDown
        closeButton.contentTintColor = .secondaryLabelColor
        closeButton.target = self
        closeButton.action = #selector(closeTabClicked)
        closeButton.translatesAutoresizingMaskIntoConstraints = false
        closeButton.isHidden = !isSelected  // Show only when selected or hovering
        addSubview(closeButton)

        // Adjust leading constraint to account for color bar
        let stackLeading: CGFloat = group != nil ? (colorBarLeading + colorBarWidth + 6) : 2
        NSLayoutConstraint.activate([
            stackView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: stackLeading),
            stackView.trailingAnchor.constraint(equalTo: closeButton.leadingAnchor, constant: -4),
            stackView.centerYAnchor.constraint(equalTo: centerYAnchor),

            closeButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            closeButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            closeButton.widthAnchor.constraint(equalToConstant: 16),
            closeButton.heightAnchor.constraint(equalToConstant: 16)
        ])
    }

    private func updateCloseButtonVisibility() {
        closeButton.isHidden = !(isSelected || isHovering)
    }

    @objc private func closeTabClicked() {
        viewModel?.closeTab(id: tab.id)
    }

    // MARK: - Mouse Tracking

    override func mouseEntered(with event: NSEvent) {
        isHovering = true
    }

    override func mouseExited(with event: NSEvent) {
        isHovering = false
    }
}
