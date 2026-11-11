import Foundation
import AppKit // Import AppKit for NSImage
import Combine
import ActriumCore

private let logger = AppLogger.viewModel

/// Actrium display mode - stored in ViewModel to survive view recreation
enum ActriumMode: Equatable {
    case overview
    case focus(tabId: UUID)

    var isInFocusMode: Bool {
        if case .focus = self { return true }
        return false
    }
}

/// Protocol for objects that publish actriumMode changes
/// Used by ViewBridgeCoordinatorManager to subscribe to mode changes
protocol ActriumModePublisher: AnyObject {
    var actriumMode: ActriumMode { get }
    var actriumModePublisher: AnyPublisher<ActriumMode, Never> { get }
}

@MainActor
class BrowserViewModel: ObservableObject, ActriumModePublisher {
    // MARK: - ActriumModePublisher conformance
    var actriumModePublisher: AnyPublisher<ActriumMode, Never> {
        $actriumMode.eraseToAnyPublisher()
    }
    @Published var tabs: [Tab] = []
    @Published var currentTabId: UUID?
    // Default that all tabs are left-side panel visible; overrides are recorded only when user switching
    @Published private var leftPanelVisibility: [UUID: Bool] = [:]

    // MARK: - Tab Groups
    @Published var tabGroups: [TabGroup] = []

    // 🔴 Development debugging: Go straight into the action Focus Mode
    @Published var shouldStartInFocusMode: Bool = true

    /// Actrium display mode (Overview vs Focus)
    /// Stored here to survive view recreation when DevTools layout changes
    @Published var actriumMode: ActriumMode = .overview

    /// Flag to trigger address bar focus (set by addNewTab, consumed by view)
    @Published var shouldAutoFocusAddressBar: Bool = false

    /// Current tab loading state (for web content)
    @Published var isCurrentTabLoading: Bool = false

    /// Current tab loading progress (0.0 to 1.0)
    @Published var loadingProgress: Double = 0

    // MARK: - Grid Slot Position Management

    /// Tab ID → grid slot index mapping
    @Published var tabSlotPositions: [UUID: Int] = [:]

    /// Toolbar Actual height (from view measurements)
    @Published var toolbarHeight: CGFloat = 52 // Default value, updated in view

    /// History view model reference for recording ACTR app visits
    weak var historyVM: HistoryViewModel?
    private let browserDataRepository = BrowserDataRepository.shared

    var currentTab: Tab? {
        guard let currentTabId = currentTabId else { return nil }
        return tabs.first(where: { $0.id == currentTabId })
    }

    init() {
        // Initialize slot positions first
        initializeSlotPositions()

        // Create initial StartUp tab
        let startTab = Tab(
            title: "Start Page",
            systemTabType: .startPage,
            backgroundColor: "#1e1e1e" // Fixed dark color
        )
        tabs.append(startTab)
        currentTabId = startTab.id
        tabSlotPositions[startTab.id] = 0
        actriumMode = .focus(tabId: startTab.id)
    }

    private func persistBrowserPageSession() {
        browserDataRepository.saveSession(tabs: tabs, currentTabId: currentTabId)
    }

    @discardableResult
    func addNewTab(focusAddressBar: Bool = true) -> Tab {
        logger.debug("🆕 Start New Tab...")
        let newTab = Tab(
            title: "New Tab",
            systemTabType: .startPage,
            backgroundColor: "#1e1e1e" // Fixed dark color
        )
        tabs.append(newTab)
        currentTabId = newTab.id

        // Simplification: Add directly to the end
        tabSlotPositions[newTab.id] = tabs.count - 1

        // New Tab Direct access. Focus Mode
        actriumMode = .focus(tabId: newTab.id)
        // Only a pure new blank Tab , and not focus when opening links
        shouldAutoFocusAddressBar = focusAddressBar

        logger.notice(" New Tab Created: \(newTab.title), Location: \(self.tabs.count - 1)")
        persistBrowserPageSession()
        return newTab
    }

    /// Open or switch to a system tab (History, Bookmarks)
    /// Returns the existing tab if already open, otherwise creates a new one
    @discardableResult
    func openSystemTab(_ type: SystemTabType) -> Tab {
        // Check if a system tab of this type is already open
        if let existingTab = tabs.first(where: { $0.systemTabType == type }) {
            // Switch to existing tab
            currentTabId = existingTab.id
            actriumMode = .focus(tabId: existingTab.id)
            logger.debug("🔄 Switch to an existing system label: \(type.title)")
            persistBrowserPageSession()
            return existingTab
        }

        // Create new system tab with about: URL
        let newTab = Tab(
            title: type.title,
            url: type.aboutURL,
            systemTabType: type,
            backgroundColor: "#1e1e1e"  // Dark background for system tabs
        )
        tabs.append(newTab)
        currentTabId = newTab.id
        tabSlotPositions[newTab.id] = tabs.count - 1
        actriumMode = .focus(tabId: newTab.id)

        logger.notice("📱 Created System Label: \(type.title) (\(type.aboutURL.absoluteString)), Location: \(self.tabs.count - 1)")
        persistBrowserPageSession()
        return newTab
    }

    func switchToTab(id: UUID) {
        if let tab = tabs.first(where: { $0.id == id }) {
            logger.debug("🔄 Switch to Tab: \(tab.title)")
        }
        currentTabId = id
        // Ensure actrium is in focus mode when switching tabs
        actriumMode = .focus(tabId: id)
        persistBrowserPageSession()
    }

    func closeTab(id: UUID) {
        // Get the slot position of the tab being closed
        guard let closedPosition = tabSlotPositions[id] else {
            logger.warning(" Cannot find tab slot position")
            return
        }

        // If this is the last tab, exit application after closing
        if tabs.count == 1 {
            logger.debug("🚪 Close last tab, apply the exit is about to be completed")
            browserDataRepository.saveSession(tabs: [], currentTabId: nil)
            NSApplication.shared.terminate(nil)
            return
        }

        // Switch to the adjacent label if the current tab is closed
        if currentTabId == id {
            let wasInFocusMode = actriumMode.isInFocusMode
            if let index = tabs.firstIndex(where: { $0.id == id }) {
                var newTabId: UUID?
                if index > 0 {
                    newTabId = tabs[index - 1].id
                } else if index < tabs.count - 1 {
                    newTabId = tabs[index + 1].id
                }
                if let newTabId {
                    currentTabId = newTabId
                    // Preserve Focus mode when closing current tab
                    if wasInFocusMode {
                        actriumMode = .focus(tabId: newTabId)
                    }
                }
            }
        }

        // Remove Tab
        tabs.removeAll(where: { $0.id == id })

        // Remove the location map of this tab
        tabSlotPositions.removeValue(forKey: id)

        // Disable the window of the life cycle when you close the tab
        RendererLifecycleManager.shared.tabDidClose(id)

        // Reorganization position: reduce all positions larger than the deleted position1
        for tabId in tabs.map({ $0.id }) {
            if let position = tabSlotPositions[tabId], position > closedPosition {
                tabSlotPositions[tabId] = position - 1
            }
        }

        logger.debug("🗑️ Close Tab, Location \(closedPosition)Remaining \(self.tabs.count) Tab")
        persistBrowserPageSession()
    }

    func reorderTab(fromIndex source: Int, toIndex destination: Int) {
        guard source >= 0, source < tabs.count,
              destination >= 0, destination <= tabs.count,
              source != destination else { return }

        let movedTab = tabs.remove(at: source)
        let insertIndex = destination > source ? destination - 1 : destination
        tabs.insert(movedTab, at: insertIndex)

        logger.debug("🔄 Tab Reorder: From Index \(source) Move To \(insertIndex)")
        persistBrowserPageSession()
    }

    // MARK: - Tab Group Management

    /// Create a new tab group
    @discardableResult
    func createGroup(name: String = "", color: TabGroupColor = .gray) -> TabGroup {
        let group = TabGroup(name: name, color: color)
        tabGroups.append(group)
        logger.debug("📁 Create New Group: \(group.id), Name: \(name.isEmpty ? "(No name)" : name)")
        return group
    }

    /// Move a tab to a specific index in the tabs array
    /// - Parameters:
    ///   - tabId: The tab to move
    ///   - toIndex: The target index (will be clamped to valid range)
    func moveTab(tabId: UUID, toIndex: Int) {
        guard let fromIndex = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        guard fromIndex != toIndex else { return }

        let tab = tabs.remove(at: fromIndex)
        let targetIndex = min(max(0, toIndex), tabs.count)
        tabs.insert(tab, at: targetIndex)
        logger.debug("📍 Tab Move: \(tab.title) From Location \(fromIndex) Present. \(targetIndex)")
        persistBrowserPageSession()
    }

    /// Move a tab to a specific position and optionally change its group
    /// - Parameters:
    ///   - tabId: The tab to move
    ///   - toIndex: Target index in the tabs array
    ///   - groupId: New group ID (nil to ungroup)
    func moveTabToPosition(tabId: UUID, toIndex: Int, groupId: UUID?) {
        guard let fromIndex = tabs.firstIndex(where: { $0.id == tabId }) else { return }

        let oldGroupId = tabs[fromIndex].groupId

        // Update group membership
        tabs[fromIndex].groupId = groupId

        // Move to new position
        let tab = tabs.remove(at: fromIndex)
        let targetIndex = min(max(0, toIndex), tabs.count)
        tabs.insert(tab, at: targetIndex)

        logger.debug("📍 Tab Move: \(tab.title) To Location \(targetIndex), Group: \(groupId?.uuidString ?? "None")")

        // Clean up old empty group if needed
        if let oldId = oldGroupId, oldId != groupId {
            cleanupEmptyGroup(oldId)
        }
        persistBrowserPageSession()
    }

    /// Insert a tab received from another window (cross-window drag)
    func insertTabFromCrossWindow(_ tab: Tab, at index: Int, groupId: UUID?) {
        var newTab = tab
        newTab.groupId = groupId

        let targetIndex = min(max(0, index), tabs.count)
        tabs.insert(newTab, at: targetIndex)
        tabSlotPositions[newTab.id] = tabs.count - 1
        currentTabId = newTab.id

        logger.notice(" Insert across Window Tab: \(newTab.title) To Location \(targetIndex), Group: \(groupId?.uuidString ?? "None")")
        persistBrowserPageSession()
    }

    /// Remove a tab by ID (used for cross-window drag source cleanup)
    func removeTabById(_ tabId: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        let tab = tabs[index]
        let oldGroupId = tab.groupId

        tabs.remove(at: index)
        tabSlotPositions.removeValue(forKey: tabId)

        // Update current tab if needed
        if currentTabId == tabId {
            currentTabId = tabs.first?.id
        }

        // Clean up empty group
        if let oldId = oldGroupId {
            cleanupEmptyGroup(oldId)
        }

        logger.debug("🗑️ Remove across Window Tab: \(tab.title)")
        persistBrowserPageSession()
    }

    /// Calculate the index for inserting a tab into a group at a specific child position
    /// - Parameters:
    ///   - groupId: The target group
    ///   - childIndex: Position within the group's children (NSOutlineView child index)
    /// - Returns: The absolute index in the tabs array
    func indexForInsertingIntoGroup(groupId: UUID, atChildIndex childIndex: Int) -> Int {
        let groupTabs = tabs.enumerated().filter { $0.element.groupId == groupId }
        if childIndex >= groupTabs.count {
            // Insert at end of group
            if let lastGroupTab = groupTabs.last {
                return lastGroupTab.offset + 1
            }
        } else if childIndex >= 0 && childIndex < groupTabs.count {
            // Insert before the child at childIndex
            return groupTabs[childIndex].offset
        }
        // Fallback: find first tab of this group
        return tabs.firstIndex(where: { $0.groupId == groupId }) ?? tabs.count
    }

    /// Calculate the index for inserting at root level (ungrouped)
    /// - Parameter rootChildIndex: The child index at root level (from NSOutlineView)
    /// - Returns: The absolute index in the tabs array
    func indexForInsertingAtRoot(atChildIndex rootChildIndex: Int) -> Int {
        let organized = tabsOrganizedByGroups()
        var currentIndex = 0

        for (i, groupData) in organized.enumerated() {
            if i == rootChildIndex {
                return currentIndex
            }
            currentIndex += groupData.tabs.count
        }

        // Insert at end
        return tabs.count
    }

    /// Add a tab to a group
    func addTabToGroup(tabId: UUID, groupId: UUID) {
        guard let tabIndex = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        tabs[tabIndex].groupId = groupId
        logger.debug("📁 Tab \(tabId) Add Group \(groupId)")
    }

    /// Remove a tab from its group
    func removeTabFromGroup(tabId: UUID) {
        guard let tabIndex = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        let oldGroupId = tabs[tabIndex].groupId
        tabs[tabIndex].groupId = nil
        logger.debug("📁 Tab \(tabId) From Group \(String(describing: oldGroupId)) Remove")

        // Clean up empty groups
        if let groupId = oldGroupId {
            cleanupEmptyGroup(groupId)
        }
    }

    /// Delete a group (removes all tabs from the group but keeps the tabs)
    func deleteGroup(groupId: UUID) {
        // Remove all tabs from this group
        for i in tabs.indices {
            if tabs[i].groupId == groupId {
                tabs[i].groupId = nil
            }
        }
        // Remove the group
        tabGroups.removeAll { $0.id == groupId }
        logger.debug("🗑️ Group \(groupId) Deleted")
    }

    /// Close all tabs in a group
    /// - Parameters:
    ///   - groupId: The group to close
    ///   - historyVM: Optional HistoryViewModel to record the closed group for restore
    func closeGroup(groupId: UUID, historyVM: HistoryViewModel? = nil) {
        guard let group = tabGroups.first(where: { $0.id == groupId }) else { return }
        let tabsToClose = tabs.filter { $0.groupId == groupId }

        // Record to history before closing
        if let historyVM = historyVM {
            historyVM.recordClosedGroup(group, tabs: tabsToClose)
        }

        for tab in tabsToClose {
            closeTab(id: tab.id)
        }
        // Remove the group
        tabGroups.removeAll { $0.id == groupId }
        logger.debug("🗑️ Group \(groupId) And all of it. Tab Closed")
    }

    /// Restore a group from history
    func restoreGroup(from historyGroup: HistoryGroup) {
        // Create a new group with the same properties
        let newGroup = createGroup(name: historyGroup.name, color: historyGroup.color)

        // Restore each tab in the group
        for historyTab in historyGroup.tabs {
            if let appURL = historyTab.appURL {
                // Import the ACTR package
                importActrPackageToGroup(from: appURL, groupId: newGroup.id)
            }
        }

        logger.debug("♻️ Grouping resumed: \(historyGroup.name.isEmpty ? "Unnamed" : historyGroup.name) Organisation \(historyGroup.tabs.count) individual Tab")
    }

    /// Import ACTR package and add to a specific group
    private func importActrPackageToGroup(from fileURL: URL, groupId: UUID) {
        let resolvedURL = fileURL.standardizedFileURL
        guard FileManager.default.fileExists(atPath: resolvedURL.path) else {
            logger.error(" Not found .actr Documents:\(resolvedURL.path)")
            return
        }

        // Check if it's a directory (dev mode) or archive
        var isDirectory: ObjCBool = false
        FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory)

        let rootDir: URL
        var tempDir: URL? = nil

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
            tempDir = nil
        } else {
            // Package archive mode - extract to temp
            let temp = FileManager.default.temporaryDirectory
                .appendingPathComponent("actr-\(UUID().uuidString)", isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true)
                try unzipActrPackage(archive: resolvedURL, to: temp)
            } catch {
                logger.error(" [restoreGroup] .actr Loading failed:\(error.localizedDescription)")
                return
            }
            rootDir = temp
            tempDir = temp
        }

        // Mark as read to silence "written to, but never read" warnings.
        if tempDir != nil { }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir)
            let tabTitle = manifestInfo.name ?? resolvedURL.deletingPathExtension().lastPathComponent

            let guestScript = readGuestScript(fromRoot: rootDir)
            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: rootDir, fileName: unifiedFileName)
            }

            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil
            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: rootDir, fileName: leftFileName)
            }
            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: rootDir, fileName: rightFileName)
            }

            // Create tab with groupId
            let newTab = Tab(
                title: tabTitle,
                url: resolvedURL,
                appId: manifestInfo.appId,
                groupId: groupId,
                guestScript: guestScript,
                leftPanelScript: leftPanelScript,
                rightPanelScript: rightPanelScript,
                unifiedScript: unifiedScript,
                contractName: manifestInfo.contractName,
                contractVersion: manifestInfo.contractVersion,
                permissions: manifestInfo.permissions,
                leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible
            )
            tabs.append(newTab)
            tabSlotPositions[newTab.id] = tabs.count - 1
            leftPanelVisibility[newTab.id] = newTab.leftPanelDefaultVisible
            logger.notice(" [restoreGroup] Loaded .actr To Group: \(tabTitle)")
            persistBrowserPageSession()

        } catch {
            logger.error(" [restoreGroup] .actr Loading failed:\(error.localizedDescription)")
        }
    }

    /// Update group properties
    func updateGroup(groupId: UUID, name: String? = nil, color: TabGroupColor? = nil, isCollapsed: Bool? = nil) {
        guard let index = tabGroups.firstIndex(where: { $0.id == groupId }) else { return }
        if let name = name {
            tabGroups[index].name = name
        }
        if let color = color {
            tabGroups[index].color = color
        }
        if let isCollapsed = isCollapsed {
            tabGroups[index].isCollapsed = isCollapsed
        }
        logger.debug("📝 Group \(groupId) Updated")
    }

    /// Toggle group collapsed state
    func toggleGroupCollapsed(groupId: UUID) {
        guard let index = tabGroups.firstIndex(where: { $0.id == groupId }) else { return }
        tabGroups[index].isCollapsed.toggle()
        logger.debug("📁 Group \(groupId) Collapse Status: \(self.tabGroups[index].isCollapsed)")
    }

    /// Get group for a tab
    func group(for tabId: UUID) -> TabGroup? {
        guard let tab = tabs.first(where: { $0.id == tabId }),
              let groupId = tab.groupId else { return nil }
        return tabGroups.first { $0.id == groupId }
    }

    /// Get all tabs in a group
    func tabs(in groupId: UUID) -> [Tab] {
        return tabs.filter { $0.groupId == groupId }
    }

    /// Get tabs organized by group while maintaining original tab order
    /// Groups appear at the position of their first member tab
    func tabsOrganizedByGroups() -> [TabGroupWithTabs] {
        var result: [TabGroupWithTabs] = []
        var processedGroupIds: Set<UUID> = []
        var currentUngroupedTabs: [Tab] = []

        for tab in tabs {
            if let groupId = tab.groupId {
                // Flush any pending ungrouped tabs first
                if !currentUngroupedTabs.isEmpty {
                    result.append(TabGroupWithTabs(group: nil, tabs: currentUngroupedTabs))
                    currentUngroupedTabs = []
                }

                // If this is the first tab of this group, add the whole group
                if !processedGroupIds.contains(groupId) {
                    processedGroupIds.insert(groupId)
                    if let group = tabGroups.first(where: { $0.id == groupId }) {
                        // Get all tabs in this group (maintaining their relative order)
                        let groupTabs = tabs.filter { $0.groupId == groupId }
                        result.append(TabGroupWithTabs(group: group, tabs: groupTabs))
                    }
                }
                // Skip - already added with group
            } else {
                // Ungrouped tab - collect them
                currentUngroupedTabs.append(tab)
            }
        }

        // Flush remaining ungrouped tabs
        if !currentUngroupedTabs.isEmpty {
            result.append(TabGroupWithTabs(group: nil, tabs: currentUngroupedTabs))
        }

        return result
    }

    /// Clean up empty group (called after removing a tab from a group)
    private func cleanupEmptyGroup(_ groupId: UUID) {
        let hasTabsInGroup = tabs.contains { $0.groupId == groupId }
        if !hasTabsInGroup {
            tabGroups.removeAll { $0.id == groupId }
            logger.debug("🧹 Empty Group \(groupId) Automatically deleted")
        }
    }

    /// Create a group from selected tabs
    func createGroupFromTabs(_ tabIds: [UUID], name: String = "", color: TabGroupColor = .gray) {
        let group = createGroup(name: name, color: color)
        for tabId in tabIds {
            addTabToGroup(tabId: tabId, groupId: group.id)
        }
    }

    // MARK: - Grid Slot Position Methods

    /// Initialize slot positions for all tabs
    func initializeSlotPositions() {
        for (index, tab) in tabs.enumerated() {
            tabSlotPositions[tab.id] = index
        }
        logger.debug("📍 Initialized slot positions: \(self.tabs.count) tabs")
    }

    /// Swap slot positions between two tabs
    func swapSlotPositions(tabId1: UUID?, tabId2: UUID?) {
        guard let id1 = tabId1, let id2 = tabId2 else {
            logger.warning(" Cannot swap: need two valid tab IDs")
            return
        }

        guard let pos1 = tabSlotPositions[id1],
              let pos2 = tabSlotPositions[id2] else {
            logger.warning(" Cannot swap: slot position not found")
            return
        }

        tabSlotPositions[id1] = pos2
        tabSlotPositions[id2] = pos1
        logger.debug("🔄 Swapped slot positions: \(pos1) ↔ \(pos2)")
    }

    /// Get tabs sorted by their slot position
    func tabsSortedBySlotPosition() -> [(tab: Tab, position: Int)] {
        return tabs.compactMap { tab in
            guard let position = tabSlotPositions[tab.id] else { return nil }
            return (tab, position)
        }.sorted { $0.position < $1.position }
    }

    func updateCurrentTabTitle(_ title: String) {
        guard let currentTabId = currentTabId,
              let index = tabs.firstIndex(where: { $0.id == currentTabId }) else { return }
        tabs[index].title = title
        persistBrowserPageSession()
    }

    /// Update title for a specific tab
    func updateTabTitle(for tabId: UUID, title: String) {
        guard let index = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        tabs[index].title = title
        persistBrowserPageSession()
    }

    /// Update URL for a specific tab (for web navigation)
    func updateTabURL(for tabId: UUID, url: URL) {
        guard let index = tabs.firstIndex(where: { $0.id == tabId }) else {
            logger.warning("⚠️ [updateTabURL] Tab not found: \(tabId)")
            return
        }
        let oldURL = tabs[index].url?.absoluteString ?? "nil"
        tabs[index].url = url
        logger.notice("🔗 [updateTabURL] Tab \(tabId) URL updated from \(oldURL) to \(url.absoluteString)")
        persistBrowserPageSession()
    }

    /// Update background color for a specific tab
    func updateTabBackgroundColor(for tabId: UUID, color: String) {
        guard let index = tabs.firstIndex(where: { $0.id == tabId }) else { return }
        // Only update if changed
        if tabs[index].backgroundColor != color {
            logger.notice("🎨 [BrowserViewModel] Updating background color for tab \(self.tabs[index].title): \(color)")
            tabs[index].backgroundColor = color
        }
    }

    /// Navigation Current Tab To Assign URL Or apply ID
    /// Unified navigation portal - Process all types of input
    /// - If the input is about: URLNavigator to the corresponding system application
    /// - If the input is actro:// URLLoad Actrium Apply
    /// - If the input is http/https URL, as Web Load
    /// - Load if input is a local file path ACTR Apply
    /// - Otherwise, as app id Process
    func navigateCurrentTab(to input: String) {
        guard let currentTabId = currentTabId,
              let index = tabs.firstIndex(where: { $0.id == currentTabId }) else {
            logger.warning("⚠️ [navigateCurrentTab] No Current tab")
            return
        }

        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)

        // 0. Realm link resolution — the address bar only accepts actr://
        // links; the core parser owns that decision (case-insensitive scheme,
        // format rules, local-Realm ownership) and every other input is
        // rejected as invalid.
        do {
            let info = try ActriumCore.parseActrUri(
                uri: trimmed,
                installationId: InstallationIdProvider.getOrCreate()
            )
            handleResolvedRealmLink(info, input: trimmed, tabIndex: index)
            return
        } catch {
            logger.warning("⚠️ [navigateCurrentTab] input rejected: \(error)")
            presentRealmLinkError(error)
            return
        }

        // 1. Check for local file path
        if isLocalFilePath(trimmed) {
            let fileURL = URL(fileURLWithPath: trimmed).standardizedFileURL
            var isDirectory: ObjCBool = false
            if FileManager.default.fileExists(atPath: fileURL.path, isDirectory: &isDirectory) {
                if isDirectory.boolValue {
                    logger.notice("📂 [navigateCurrentTab] Load Development Directory: \(fileURL.path)")
                    loadFromDevDirectory(fileURL)
                } else {
                    logger.notice("📦 [navigateCurrentTab] Load ACTR Package: \(fileURL.path)")
                    importACTRPackage(from: fileURL)
                }
                return
            } else {
                logger.warning("⚠️ [navigateCurrentTab] File does not exist: \(fileURL.path)")
                return
            }
        }

        // 2. Check for yes or no URL
        if let url = parseURL(trimmed) {
            // Check if it's an about: URL for system apps
            if let systemType = SystemTabType.from(url: url) {
                logger.notice("📱 [navigateCurrentTab] Navigation to System Application: \(url.absoluteString)")
                // Update Tab to system app
                tabs[index].systemTabType = systemType
                tabs[index].url = url
                tabs[index].title = systemType.title
                // Clear web/RN content
                tabs[index].unifiedScript = nil
                tabs[index].leftPanelScript = nil
                tabs[index].rightPanelScript = nil
                tabs[index].guestScript = nil
                tabs[index].htmlContent = nil
                tabs[index].contractName = nil
                objectWillChange.send()
                persistBrowserPageSession()
                return
            }

            // Check if it's an actro:// URL for Actrium apps.
            // Legacy app-scheme links are still accepted as aliases.
            // Two formats supported:
            // - actro://bundleId - lookup by bundleId in database
            // - actro:///absolute/path - local file path (three slashes)
            if ActriumAppURLScheme.isSupported(url.scheme) {
                var sourcePath: String?

                // Check if it's a local path (actro:///path/to/app)
                // URL parsing: actro:///path -> host is nil/empty, path is /path
                if let host = url.host, !host.isEmpty {
                    // bundleId mode: actro://com.example.app - lookup in database
                    logger.notice("📱 [navigateCurrentTab] Navigator to Actrium Apply (bundleId): \(host)")
                    let appRepo = AppRepository()
                    if let app = appRepo.getByBundleId(host) {
                        sourcePath = app.sourcePath
                        appRepo.updateLastUsed(bundleId: host)
                    } else {
                        logger.warning("⚠️ [navigateCurrentTab] No applications found: \(host)")
                        return
                    }
                } else {
                    // local path mode: actro:///Users/example-dev-2/path/to/app
                    sourcePath = url.path
                    logger.notice("📂 [navigateCurrentTab] Navigator to Local Actrium Apply: \(sourcePath ?? "")")
                }

                // Load ACTR app into current tab
                if let path = sourcePath {
                    loadActrPackageIntoCurrentTab(from: URL(fileURLWithPath: path), tabIndex: index)
                }
                return
            }

            logger.notice("🌐 [navigateCurrentTab] Navigator to URL: \(url.absoluteString)")
            // Update Tab Yes Web Contents
            tabs[index].url = url
            tabs[index].title = url.host ?? url.absoluteString
            // Clear RN Relevant
            tabs[index].unifiedScript = nil
            tabs[index].leftPanelScript = nil
            tabs[index].rightPanelScript = nil
            tabs[index].guestScript = nil
            tabs[index].systemTabType = nil
            tabs[index].contractName = nil
            objectWillChange.send()
            persistBrowserPageSession()
        } else {
            // Try to load as app bundle ID directly
            logger.notice("📱 [navigateCurrentTab] Try as bundle ID Load: \(trimmed)")
            let appRepo = AppRepository()
            if let app = appRepo.getByBundleId(trimmed) {
                appRepo.updateLastUsed(bundleId: trimmed)
                loadActrPackageIntoCurrentTab(from: URL(fileURLWithPath: app.sourcePath), tabIndex: index)
            } else {
                logger.warning("⚠️ [navigateCurrentTab] No applications found: \(trimmed)")
            }
        }
    }

    /// Load ACTR app into the current tab (in-place update, not creating new tab)
    /// Used by actro:// URL navigation to update current tab content
    private func loadActrPackageIntoCurrentTab(from fileURL: URL, tabIndex: Int) {
        let resolvedURL = fileURL.standardizedFileURL

        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory) else {
            logger.error("❌ [loadActrPackageIntoCurrentTab] Path does not exist: \(resolvedURL.path)")
            return
        }

        let rootDir: URL
        if isDirectory.boolValue {
            rootDir = resolvedURL
        } else {
            // Package archive mode - extract to temp
            let tempDir = FileManager.default.temporaryDirectory
                .appendingPathComponent("actr-\(UUID().uuidString)", isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
                try unzipActrPackage(archive: resolvedURL, to: tempDir)
            } catch {
                logger.error("❌ [loadActrPackageIntoCurrentTab] Unpressure failed: \(error.localizedDescription)")
                return
            }
            rootDir = tempDir
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir)
            let tabTitle = manifestInfo.name ?? resolvedURL.deletingPathExtension().lastPathComponent

            // Register app if appId is provided.
            if let appId = manifestInfo.appId {
                let sourceType: AppSourceType = isDirectory.boolValue ? .localDev : .downloaded
                registerApp(
                    name: tabTitle,
                    bundleId: appId,
                    version: manifestInfo.version,
                    description: manifestInfo.description,
                    sourceType: sourceType,
                    sourcePath: resolvedURL.path,
                    icon: manifestInfo.icon
                )
            }

            // Read scripts
            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: rootDir, fileName: unifiedFileName)
            }

            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil
            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: rootDir, fileName: leftFileName)
            }
            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: rootDir, fileName: rightFileName)
            }

            let guestScript = readGuestScript(fromRoot: rootDir)

            // Construct actro:// URL for History and address bar
            let appURL: URL
            if let appId = manifestInfo.appId,
               let url = URL(string: "\(ActriumAppURLScheme.primary)://\(appId)") {
                appURL = url
            } else {
                // Local dev path: actro:///absolute/path
                appURL = ActriumAppURLScheme.appURL(filePath: resolvedURL.path) ?? resolvedURL
            }

            // Update current tab in-place (like about: URL handling)
            logger.notice("📱 [loadActrPackageIntoCurrentTab] Update Current Tab: \(tabTitle), url=\(appURL.absoluteString)")

            tabs[tabIndex].title = tabTitle
            tabs[tabIndex].url = appURL
            tabs[tabIndex].appId = manifestInfo.appId
            tabs[tabIndex].systemTabType = nil
            tabs[tabIndex].htmlContent = nil

            // Set scripts based on available content
            if let unified = unifiedScript {
                tabs[tabIndex].unifiedScript = unified
                tabs[tabIndex].leftPanelScript = nil
                tabs[tabIndex].rightPanelScript = nil
                tabs[tabIndex].guestScript = nil
            } else if leftPanelScript != nil || rightPanelScript != nil {
                tabs[tabIndex].unifiedScript = nil
                tabs[tabIndex].leftPanelScript = leftPanelScript
                tabs[tabIndex].rightPanelScript = rightPanelScript
                tabs[tabIndex].guestScript = nil
            } else if let script = guestScript {
                tabs[tabIndex].unifiedScript = nil
                tabs[tabIndex].leftPanelScript = nil
                tabs[tabIndex].rightPanelScript = nil
                tabs[tabIndex].guestScript = script
            }

            tabs[tabIndex].contractName = manifestInfo.contractName
            tabs[tabIndex].contractVersion = manifestInfo.contractVersion
            tabs[tabIndex].permissions = manifestInfo.permissions
            tabs[tabIndex].leftPanelDefaultVisible = manifestInfo.leftPanelDefaultVisible
            tabs[tabIndex].rightPanelDefaultVisible = manifestInfo.rightPanelDefaultVisible

            // Update panel visibility
            leftPanelVisibility[tabs[tabIndex].id] = manifestInfo.leftPanelDefaultVisible

            objectWillChange.send()
            persistBrowserPageSession()

            // Record History visit for ACTR app
            historyVM?.recordVisit(tab: tabs[tabIndex])

        } catch {
            logger.error("❌ [loadActrPackageIntoCurrentTab] Loading failed: \(error.localizedDescription)")
        }
    }

    /// Handle a Realm link already resolved by `ActriumCore.parseActrUri`
    /// (which delegates to `realm_link::open_realm_link` for ownership and
    /// dispatch). Realm discovery and package download are out of scope for
    /// this milestone: a resolved link currently updates the tab and logs
    /// the follow-up step. Opening a local Realm instance is also not wired
    /// yet and is reported as such.
    private func handleResolvedRealmLink(
        _ info: ActrUriInfo,
        input: String,
        tabIndex: Int
    ) {
        // The core parser guarantees a RealmId for every resolved link.
        guard let realmId = info.realmId else { return }

        let realmDisplay: String
        if let serial = info.serial {
            realmDisplay = "\(serial).\(realmId)@\(info.domain)"
        } else {
            realmDisplay = "\(realmId)@\(info.domain)"
        }
        logger.notice("🔗 [navigateCurrentTab] Resolved actr:// link: \(realmDisplay), local=\(info.isLocal)")
        if info.isLocal {
            logger.notice("ℹ️ [navigateCurrentTab] Opening a local Realm instance is not wired yet.")
        } else if let discoveryURL = info.discoveryUrl, let url = URL(string: discoveryURL) {
            discoverAuthority(url: url, domain: info.domain, realmId: realmId)
        }

        tabs[tabIndex].title = realmDisplay
        tabs[tabIndex].url = URL(string: input)
        tabs[tabIndex].appId = nil
        tabs[tabIndex].systemTabType = nil
        tabs[tabIndex].htmlContent = nil
        tabs[tabIndex].unifiedScript = nil
        tabs[tabIndex].leftPanelScript = nil
        tabs[tabIndex].rightPanelScript = nil
        tabs[tabIndex].guestScript = nil
        tabs[tabIndex].contractName = nil
        objectWillChange.send()
        persistBrowserPageSession()
    }

    /// Realm discovery (verification-funnel layer 1) followed by Realm
    /// Profile discovery (layer 2): fetch the authority descriptor, validate
    /// it, then fetch and verify the Realm Profile it points to. The package
    /// target that comes out feeds package acquisition; downloads are not
    /// wired in this milestone.
    private func discoverAuthority(url: URL, domain: String, realmId: String) {
        Task { @MainActor in
            do {
                let (data, response) = try await URLSession.shared.data(from: url)
                guard let http = response as? HTTPURLResponse,
                      (200 ..< 300).contains(http.statusCode) else {
                    let status = (response as? HTTPURLResponse)?.statusCode ?? -1
                    throw NSError(
                        domain: "ActrDiscovery", code: 1,
                        userInfo: [NSLocalizedDescriptionKey: String(
                            format: NSLocalizedString(
                                "The authority information for this domain is unavailable (HTTP %ld).",
                                comment: "Actr authority fetch failure"),
                            status)])
                }
                // Every hop of a redirect chain must stay on HTTPS; the
                // default session follows redirects, so checking the final
                // response URL is the minimal enforcement here.
                guard let finalURL = response.url, finalURL.scheme?.lowercased() == "https" else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 2,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The authority information was served over an insecure connection.",
                            comment: "Actr authority insecure transport error")])
                }
                guard let body = String(data: data, encoding: .utf8) else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 3,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The authority information is not valid text.",
                            comment: "Actr authority encoding error")])
                }

                let descriptor = try ActriumCore.acceptAuthorityDescriptor(
                    body: body,
                    domain: domain
                )
                let activeKeys = descriptor.signingKeys.filter { $0.status == "active" }
                logger.notice("🌐 [navigateCurrentTab] Authority accepted for \(domain): realms=\(descriptor.realmsHref), keys sequence=\(descriptor.keysSequence), active keys=\(activeKeys.count)")

                // Realm Profile discovery: GET <realms_href>/<realm_id>.
                var profilePath = descriptor.realmsHref
                if profilePath.hasSuffix("/") { profilePath.removeLast() }
                profilePath += "/\(realmId)"
                guard let profileURL = URL(string: profilePath) else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 4,
                        userInfo: [NSLocalizedDescriptionKey: String(
                            format: NSLocalizedString(
                                "The Realm Profile location '%@' is invalid.",
                                comment: "Actr realm profile URL error"),
                            profilePath)])
                }
                let (profileData, profileResponse) =
                    try await URLSession.shared.data(from: profileURL)
                guard let profileHTTP = profileResponse as? HTTPURLResponse,
                      (200 ..< 300).contains(profileHTTP.statusCode) else {
                    let status = (profileResponse as? HTTPURLResponse)?.statusCode ?? -1
                    throw NSError(
                        domain: "ActrDiscovery", code: 5,
                        userInfo: [NSLocalizedDescriptionKey: String(
                            format: NSLocalizedString(
                                "The Realm Profile for this instance is unavailable (HTTP %ld).",
                                comment: "Actr realm profile fetch failure"),
                            status)])
                }
                guard let profileFinal = profileResponse.url,
                      profileFinal.scheme?.lowercased() == "https" else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 6,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The Realm Profile was served over an insecure connection.",
                            comment: "Actr realm profile insecure transport error")])
                }
                guard let profileBody = String(data: profileData, encoding: .utf8) else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 7,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The Realm Profile is not valid text.",
                            comment: "Actr realm profile encoding error")])
                }

                let profile = try ActriumCore.acceptRealmProfile(
                    body: profileBody,
                    realmName: "\(realmId)@\(domain)",
                    signingKeysJson: descriptor.signingKeysJson
                )
                logger.notice("📄 [navigateCurrentTab] Realm Profile accepted: \(profile.displayName) (\(profile.realmName)), sequence=\(profile.sequence), package=\(profile.packageHref)")
                logger.notice("ℹ️ [navigateCurrentTab] Package download is not wired yet; next step would fetch \(profile.packageHref) and verify sha256=\(profile.packageSha256.prefix(12))…")
            } catch {
                logger.warning("⚠️ [navigateCurrentTab] Realm discovery failed: \(error)")
                presentRealmLinkError(error)
            }
        }
    }

    /// Surface a rejected actr:// link with variant-specific copy.
    private func presentRealmLinkError(_ error: Error) {
        let message: String
        switch error {
        case CoreError.AuthorityDescriptorInvalid:
            message = NSLocalizedString(
                "The authority information for this domain is invalid.",
                comment: "Actr authority descriptor error")
        case CoreError.RealmProfileInvalid:
            message = NSLocalizedString(
                "The Realm Profile for this instance is invalid.",
                comment: "Actr realm profile error")
        case CoreError.RealmRetired:
            message = NSLocalizedString(
                "This Realm instance has been retired.",
                comment: "Actr realm retired error")
        case CoreError.UriSchemeInvalid:
            message = NSLocalizedString(
                "Only actr:// links are supported here.",
                comment: "Actr URI scheme error")
        case CoreError.UriDomainInvalid:
            message = NSLocalizedString(
                "The domain in this link is not a valid Realm domain.",
                comment: "Actr URI domain error")
        case CoreError.UriLabelInvalid:
            message = NSLocalizedString(
                "The RealmId or ActorId in this link is missing or invalid.",
                comment: "Actr URI label error")
        case CoreError.UriLegacyForm:
            message = NSLocalizedString(
                "This link uses an outdated address format that is no longer supported.",
                comment: "Actr URI legacy form error")
        case CoreError.UriLocalRealmForeign:
            message = NSLocalizedString(
                "This local Realm belongs to a different installation.",
                comment: "Actr URI foreign local Realm error")
        default:
            message = "\(error)"
        }
        presentRealmLinkMessage(message)
    }

    private func presentRealmLinkMessage(_ message: String) {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("Invalid Actr Link", comment: "Actr URI error title")
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.runModal()
    }

    /// Checks whether input is a local file path
    func isLocalFilePath(_ input: String) -> Bool {
        // actro://... may itself point at an .actr archive. It must be parsed as
        // an application URL instead of being treated as a literal filesystem path.
        if ActriumAppURLScheme.hasSupportedPrefix(input) {
            return false
        }
        // Here. / The first is the absolute path.
        if input.hasPrefix("/") {
            return true
        }
        // Here. ~ Start with a user directory
        if input.hasPrefix("~") {
            return true
        }
        // Here. file:// Start
        if input.hasPrefix("file://") {
            return true
        }
        // Here. .actr End
        if input.hasSuffix(".actr") {
            return true
        }
        return false
    }

    /// Parsing URL String
    /// - If about:/http/https/actrium, Directly decipher
    /// - If it looks like a domain name (includes) . and does not contain spaces) add https:// Prefix
    func parseURL(_ input: String) -> URL? {
        // It's complete. URL (http/https/about/actrium)
        if input.hasPrefix("http://") || input.hasPrefix("https://") ||
           input.hasPrefix("about:") ||
           ActriumAppURLScheme.hasSupportedPrefix(input) {
            return URL(string: input)
        }

        // Looks like a domain name (includes) . and does not contain spaces)
        if input.contains(".") && !input.contains(" ") {
            return URL(string: "https://\(input)")
        }

        // Fallback: treat as search query using configured search engine
        return SearchEngine.current.searchURL(for: input)
    }

    func closeCurrentTab() {
        guard let id = currentTabId else { return }
        closeTab(id: id)
    }

    // MARK: - Web Navigation

    /// Check if current web tab can go back
    var canGoBack: Bool {
        guard let tabId = currentTabId else { return false }
        return WebViewHolderManager.shared.existingHolder(for: tabId)?.canGoBack ?? false
    }

    /// Check if current web tab can go forward
    var canGoForward: Bool {
        guard let tabId = currentTabId else { return false }
        return WebViewHolderManager.shared.existingHolder(for: tabId)?.canGoForward ?? false
    }

    /// Navigate back in web history
    func goBack() {
        guard let tabId = currentTabId else { return }
        WebViewHolderManager.shared.existingHolder(for: tabId)?.goBack()
    }

    /// Navigate forward in web history
    func goForward() {
        guard let tabId = currentTabId else { return }
        WebViewHolderManager.shared.existingHolder(for: tabId)?.goForward()
    }

    /// Reload the current tab (web page or .actr package)
    func reloadCurrentTab() {
        guard let id = currentTabId,
              let index = tabs.firstIndex(where: { $0.id == id }) else {
            logger.warning("⚠️ [reloadCurrentTab] Cannot Refresh: No Current tab")
            return
        }

        logger.notice("🔄 [reloadCurrentTab] Start Refresh tab: \(self.tabs[index].title), reloadNonce: \(self.tabs[index].reloadNonce)")

        // If there is no source URL, only add reloadNonce Force RN Remount
        guard let sourceURL = tabs[index].url else {
            logger.notice("🔄 [reloadCurrentTab] Unsourced URL, only mandatory RN remount")
            tabs[index].reloadNonce &+= 1
            objectWillChange.send()
            return
        }

        // For web content (http/https), use WebView's reload
        if sourceURL.scheme == "http" || sourceURL.scheme == "https" {
            logger.notice("🔄 [reloadCurrentTab] Web Content, Call WebView reload")
            WebViewHolderManager.shared.existingHolder(for: id)?.reload()
            return
        }

        logger.debug("🔄 [reloadCurrentTab] Local package, read again")

        // Re-read the .actr package
        let resolvedURL = sourceURL.standardizedFileURL
        guard FileManager.default.fileExists(atPath: resolvedURL.path) else {
            logger.error(" [reloadCurrentTab] Source file does not exist:\(resolvedURL.path)")
            return
        }

        // Check if it's a directory (dev mode) or archive
        var isDirectory: ObjCBool = false
        FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory)

        let rootDir: URL

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
        } else {
            // Package archive mode - extract to temp
            let tempDir = FileManager.default.temporaryDirectory
                .appendingPathComponent("actr-\(UUID().uuidString)", isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
                try unzipActrPackage(archive: resolvedURL, to: tempDir)
            } catch {
                logger.error(" [reloadCurrentTab] Depression failed:\(error.localizedDescription)")
                return
            }
            rootDir = tempDir
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir)

            // Read scripts
            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: rootDir, fileName: unifiedFileName)
            }

            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil
            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: rootDir, fileName: leftFileName)
            }
            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: rootDir, fileName: rightFileName)
            }

            let guestScript = readGuestScript(fromRoot: rootDir)

            // Update the tab in place
            tabs[index].unifiedScript = unifiedScript
            tabs[index].leftPanelScript = leftPanelScript
            tabs[index].rightPanelScript = rightPanelScript
            tabs[index].guestScript = guestScript
            tabs[index].appId = manifestInfo.appId
            tabs[index].contractName = manifestInfo.contractName
            tabs[index].contractVersion = manifestInfo.contractVersion
            tabs[index].permissions = manifestInfo.permissions
            tabs[index].leftPanelDefaultVisible = manifestInfo.leftPanelDefaultVisible
            tabs[index].rightPanelDefaultVisible = manifestInfo.rightPanelDefaultVisible
            leftPanelVisibility[tabs[index].id] = manifestInfo.leftPanelDefaultVisible

            if let name = manifestInfo.name {
                tabs[index].title = name
            }

            logger.notice(" [reloadCurrentTab] Refresh finished: \(self.tabs[index].title)")

            // Add Refresh nonce,Alarm RN Side remount
            tabs[index].reloadNonce &+= 1

            // Force UI update by triggering objectWillChange
            objectWillChange.send()

        } catch {
            logger.error(" [reloadCurrentTab] Synchronising: \(error.localizedDescription)")
        }
    }

    /// Stop loading the current tab
    func stopCurrentTab() {
        guard let tabId = currentTabId else { return }
        WebViewHolderManager.shared.existingHolder(for: tabId)?.stopLoading()
    }

    // MARK: - Per-Tab Left Panel Visibility

    func isLeftPanelVisible(for tabId: UUID?) -> Bool {
        guard let id = tabId else { return true }
        let visible = leftPanelVisibility[id] ?? true
        logger.debug("🔍 [isLeftPanelVisible] Tab \(id.uuidString.prefix(8)): \(visible)")
        return visible
    }

    func toggleLeftPanelForCurrentTab() {
        // Focus Use in Mode focus tabId, or use currentTabId
        let id: UUID
        if case .focus(let focusTabId) = actriumMode {
            id = focusTabId
        } else if let currentId = currentTabId {
            id = currentId
        } else {
            logger.warning(" [toggleLeftPanel] Could not close temporary folder: %s tab, cannot be switched")
            return
        }

        var visibility = leftPanelVisibility
        let current = visibility[id] ?? true
        visibility[id] = !current
        leftPanelVisibility = visibility  // Revalue the entire dictionary to ensure that it triggers @Published
        logger.debug("🔄 [toggleLeftPanel] Tab \(id.uuidString.prefix(8)) Left Panel: \(current) -> \(!current)")
    }

    // MARK: - .actr Apply Load

    /// From Local .actr Package to create a new tab and load basic information
    func importACTRPackage(from fileURL: URL) {
        let resolvedURL = fileURL.standardizedFileURL
        guard FileManager.default.fileExists(atPath: resolvedURL.path) else {
            logger.error(" Not found .actr Documents:\(resolvedURL.path)")
            return
        }

        // Check if it's a directory (dev mode) or archive
        var isDirectory: ObjCBool = false
        FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory)

        let rootDir: URL

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
        } else {
            // Package archive mode - extract to temp
            let tempDir = FileManager.default.temporaryDirectory
                .appendingPathComponent("actr-\(UUID().uuidString)", isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
                try unzipActrPackage(archive: resolvedURL, to: tempDir)
            } catch {
                logger.error(" .actr Loading failed:\(error.localizedDescription)")
                return
            }
            rootDir = tempDir
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir)
            let tabTitle = manifestInfo.name ?? resolvedURL.deletingPathExtension().lastPathComponent

            // Register as downloaded app if appId is provided.
            if let appId = manifestInfo.appId {
                registerApp(
                    name: tabTitle,
                    bundleId: appId,
                    version: manifestInfo.version,
                    description: manifestInfo.description,
                    sourceType: .downloaded,
                    sourcePath: resolvedURL.path,
                    icon: manifestInfo.icon
                )
            }

            // Read guest Script
            let guestScript = readGuestScript(fromRoot: rootDir)

            // v2.0: Read Priority unified script (Single Engine Structure)
            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: rootDir, fileName: unifiedFileName)
                logger.notice(" [BrowserViewModel] From manifest.toml Read unified: \(unifiedFileName), Script exists: \(unifiedScript != nil)")
            }

            // From manifest.toml Read Panel Script File Name
            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil

            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: rootDir, fileName: leftFileName)
                logger.debug("🔍 [BrowserViewModel] From manifest.toml Read leftPanel: \(leftFileName), Script exists: \(leftPanelScript != nil)")
            }

            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: rootDir, fileName: rightFileName)
                logger.debug("🔍 [BrowserViewModel] From manifest.toml Read rightPanel: \(rightFileName), Script exists: \(rightPanelScript != nil)")
            }

            // v2.0: Use Priority Single Engine Structure
            if let unified = unifiedScript {
                logger.debug("🚀 [BrowserViewModel] Use Single Engine Structure (unified bundle)")
                appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    unifiedScript: unified,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                    rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                    themeColor: manifestInfo.themeColor
                )
            }
            // Legacy: Double Engine Structure
            else if leftPanelScript != nil || rightPanelScript != nil {
                logger.debug("🔄 [BrowserViewModel] Use Legacy Double Engine Structure")
                appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    leftPanelScript: leftPanelScript,
                    rightPanelScript: rightPanelScript,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                    rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                    themeColor: manifestInfo.themeColor
                )
            } else if let script = guestScript {
                // Old architecture: one compatible guest.js
                logger.debug("🔄 [BrowserViewModel] Use compatible mode (guestScript)")
                appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    guestScript: script,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    themeColor: manifestInfo.themeColor
                )
            } else {
                // No script, display application information page
                let html = renderACTRInfoHTML(
                    name: manifestInfo.name ?? "Unknown App",
                    version: manifestInfo.version ?? "0.0.0",
                    description: manifestInfo.description ?? "No description provided.",
                    sourcePath: resolvedURL.path
                )
                appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    htmlContent: html
                )
            }
        } catch {
            logger.error(" .actr Loading failed:\(error.localizedDescription)")
            let fallbackHTML = renderACTRInfoHTML(
                name: resolvedURL.deletingPathExtension().lastPathComponent,
                version: "unknown",
                description: "Loading failed:\(error.localizedDescription)",
                sourcePath: resolvedURL.path
            )
            appendRuntimeTab(
                title: resolvedURL.deletingPathExtension().lastPathComponent,
                sourceURL: resolvedURL,
                htmlContent: fallbackHTML
            )
        }
    }

    /// Async version of importACTRPackage - runs file I/O on background thread
    /// Call this from background thread, it will dispatch UI updates to main thread
    private func importACTRPackageAsync(from fileURL: URL) {
        let resolvedURL = fileURL.standardizedFileURL
        guard FileManager.default.fileExists(atPath: resolvedURL.path) else {
            logger.error(" Not found .actr Documents:\(resolvedURL.path)")
            return
        }

        // Check if it's a directory (dev mode) or archive
        var isDirectory: ObjCBool = false
        FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory)

        let rootDir: URL

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
        } else {
            // Package archive mode - extract to temp
            let tempDir = FileManager.default.temporaryDirectory
                .appendingPathComponent("actr-\(UUID().uuidString)", isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
                try unzipActrPackage(archive: resolvedURL, to: tempDir)
            } catch {
                logger.error(" .actr Loading failed:\(error.localizedDescription)")
                return
            }
            rootDir = tempDir
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir)
            let tabTitle = manifestInfo.name ?? resolvedURL.deletingPathExtension().lastPathComponent

            let guestScript = readGuestScript(fromRoot: rootDir)

            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: rootDir, fileName: unifiedFileName)
            }

            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil
            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: rootDir, fileName: leftFileName)
            }
            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: rootDir, fileName: rightFileName)
            }

            // Update UI on main thread
            DispatchQueue.main.async { [weak self] in
                if let unified = unifiedScript {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: resolvedURL,
                        appId: manifestInfo.appId,
                        unifiedScript: unified,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                        rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                        themeColor: manifestInfo.themeColor
                    )
                } else if leftPanelScript != nil || rightPanelScript != nil {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: resolvedURL,
                        appId: manifestInfo.appId,
                        leftPanelScript: leftPanelScript,
                        rightPanelScript: rightPanelScript,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                        rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                        themeColor: manifestInfo.themeColor
                    )
                } else if let script = guestScript {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: resolvedURL,
                        appId: manifestInfo.appId,
                        guestScript: script,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        themeColor: manifestInfo.themeColor
                    )
                } else {
                    let html = self?.renderACTRInfoHTML(
                        name: manifestInfo.name ?? "Unknown App",
                        version: manifestInfo.version ?? "0.0.0",
                        description: manifestInfo.description ?? "No description provided.",
                        sourcePath: resolvedURL.path
                    ) ?? ""
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: resolvedURL,
                        appId: manifestInfo.appId,
                        htmlContent: html
                    )
                }
            }
        } catch {
            logger.error(" .actr Loading failed:\(error.localizedDescription)")
            DispatchQueue.main.async { [weak self] in
                let fallbackHTML = self?.renderACTRInfoHTML(
                    name: resolvedURL.deletingPathExtension().lastPathComponent,
                    version: "unknown",
                    description: "Loading failed:\(error.localizedDescription)",
                    sourcePath: resolvedURL.path
                ) ?? ""
                self?.appendRuntimeTab(
                    title: resolvedURL.deletingPathExtension().lastPathComponent,
                    sourceURL: resolvedURL,
                    htmlContent: fallbackHTML
                )
            }
        }
    }

    /// Load from development directory (not packaged .actr)
    /// Used for hot-reload development workflow and bookmark/history loading
    func loadFromDevDirectory(_ directoryURL: URL) {
        guard FileManager.default.fileExists(atPath: directoryURL.path) else {
            logger.error(" Can not get folder: %s: %s\(directoryURL.path)")
            return
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: directoryURL)
            let tabTitle = manifestInfo.name ?? directoryURL.lastPathComponent
            logger.debug("📂 [loadFromDevDirectory] Load Development Directory: \(tabTitle)")

            // Register as local dev app if appId is provided.
            if let appId = manifestInfo.appId {
                registerApp(
                    name: tabTitle,
                    bundleId: appId,
                    version: manifestInfo.version,
                    description: manifestInfo.description,
                    sourceType: .localDev,
                    sourcePath: directoryURL.path,
                    icon: manifestInfo.icon
                )
            }

            let guestScript = readGuestScript(fromRoot: directoryURL)

            var unifiedScript: String? = nil
            if let unifiedFileName = manifestInfo.unifiedScript {
                unifiedScript = readScript(fromRoot: directoryURL, fileName: unifiedFileName)
                logger.notice(" [loadFromDevDirectory] Read unified: \(unifiedFileName), Length: \(unifiedScript?.count ?? 0)")
            }

            var leftPanelScript: String? = nil
            var rightPanelScript: String? = nil
            if let leftFileName = manifestInfo.leftPanelScript {
                leftPanelScript = readScript(fromRoot: directoryURL, fileName: leftFileName)
                logger.notice(" [loadFromDevDirectory] Read leftPanel: \(leftFileName), Length: \(leftPanelScript?.count ?? 0)")
            }
            if let rightFileName = manifestInfo.rightPanelScript {
                rightPanelScript = readScript(fromRoot: directoryURL, fileName: rightFileName)
                logger.notice(" [loadFromDevDirectory] Read rightPanel: \(rightFileName), Length: \(rightPanelScript?.count ?? 0)")
            }

            // Update UI on main thread
            DispatchQueue.main.async { [weak self] in
                // System app type takes priority (no scripts needed)
                if let systemType = manifestInfo.systemAppType {
                    logger.notice(" [loadFromDevDirectory] System Applications: \(tabTitle) (type: \(systemType.rawValue))")
                    self?.appendSystemTab(
                        title: tabTitle,
                        systemType: systemType,
                        leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                        rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible
                    )
                } else if let unified = unifiedScript {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: directoryURL,
                        appId: manifestInfo.appId,
                        unifiedScript: unified,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                        rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                        themeColor: manifestInfo.themeColor
                    )
                } else if leftPanelScript != nil || rightPanelScript != nil {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: directoryURL,
                        appId: manifestInfo.appId,
                        leftPanelScript: leftPanelScript,
                        rightPanelScript: rightPanelScript,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                        rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                        themeColor: manifestInfo.themeColor
                    )
                } else if let script = guestScript {
                    self?.appendRuntimeTab(
                        title: tabTitle,
                        sourceURL: directoryURL,
                        appId: manifestInfo.appId,
                        guestScript: script,
                        contractName: manifestInfo.contractName,
                        contractVersion: manifestInfo.contractVersion,
                        permissions: manifestInfo.permissions,
                        themeColor: manifestInfo.themeColor
                    )
                } else {
                    logger.warning(" [loadFromDevDirectory] No script file found")
                }
            }
        } catch {
            logger.error(" Development directory load failed:\(error.localizedDescription)")
        }
    }

    /// Register an Actrium app in the database
    /// This allows the app to be opened via actro://bundleId URL
    private func registerApp(
        name: String,
        bundleId: String,
        version: String?,
        description: String?,
        sourceType: AppSourceType,
        sourcePath: String,
        icon: String?
    ) {
        let appRepo = AppRepository()

        // Check if already registered
        if let existing = appRepo.getByBundleId(bundleId) {
            // Update existing entry
            var updated = existing
            updated.sourcePath = sourcePath
            if let v = version { updated.version = v }
            if let d = description { updated.description = d }
            if let i = icon { updated.icon = i }
            appRepo.update(updated)
            logger.notice("📱 [registerApp] Update application: \(bundleId)")
        } else {
            // Create new entry
            let app = InstalledActriumApp(
                name: name,
                bundleId: bundleId,
                version: version,
                description: description,
                sourceType: sourceType,
                sourcePath: sourcePath,
                icon: icon
            )
            appRepo.add(app)
            logger.notice("📱 [registerApp] Register new applications: \(bundleId) -> \(sourcePath)")
        }
    }

    /// Append a system tab (debug, history, bookmarks)
    private func appendSystemTab(
        title: String,
        systemType: SystemTabType,
        leftPanelDefaultVisible: Bool = false,
        rightPanelDefaultVisible: Bool = false
    ) {
        let newTab = Tab(
            title: title,
            systemTabType: systemType,
            leftPanelDefaultVisible: leftPanelDefaultVisible,
            rightPanelDefaultVisible: rightPanelDefaultVisible
        )
        tabs.append(newTab)
        tabSlotPositions[newTab.id] = tabs.count - 1
        leftPanelVisibility[newTab.id] = leftPanelDefaultVisible
        logger.notice(" Loaded system applications:\(title)")
        persistBrowserPageSession()
    }

    private func appendRuntimeTab(
        title: String,
        sourceURL: URL,
        appId: String? = nil,
        htmlContent: String? = nil,
        guestScript: String? = nil,
        leftPanelScript: String? = nil,
        rightPanelScript: String? = nil,
        unifiedScript: String? = nil,
        contractName: String? = nil,
        contractVersion: Int? = nil,
        permissions: [String] = [],
        leftPanelDefaultVisible: Bool = true,
        rightPanelDefaultVisible: Bool = false,
        themeColor: String? = nil
    ) {
        // Use actro:// URL for History and address bar
        let appURL = ActriumAppURLScheme.appURL(filePath: sourceURL.path) ?? sourceURL
        let newTab = Tab(
            title: title,
            url: appURL,
            appId: appId,
            htmlContent: htmlContent,
            guestScript: guestScript,
            leftPanelScript: leftPanelScript,
            rightPanelScript: rightPanelScript,
            unifiedScript: unifiedScript,
            contractName: contractName,
            contractVersion: contractVersion,
            permissions: permissions,
            leftPanelDefaultVisible: leftPanelDefaultVisible,
            rightPanelDefaultVisible: rightPanelDefaultVisible,
            backgroundColor: themeColor
        )
        logger.debug("📝 [appendRuntimeTab] Create New Tab: \(title), ID=\(newTab.id.uuidString.prefix(8)), bg=\(newTab.backgroundColor)")
        tabs.append(newTab)
        currentTabId = newTab.id
        tabSlotPositions[newTab.id] = tabs.count - 1
        leftPanelVisibility[newTab.id] = leftPanelDefaultVisible

        logger.notice(" Loaded .actr: \(title)")

        if let unified = unifiedScript {
            logger.debug("  ├─ Unified script (Single Engine): \(unified.count) Bytes")
            logger.debug("  ├─ Default for left panel\(leftPanelDefaultVisible ? "Show" : "Hide")")
            logger.debug("  └─ Default for right panel\(rightPanelDefaultVisible ? "Show" : "Hide")")
        } else if let leftScript = leftPanelScript {
            logger.debug("  ├─ Left Panel Script (Legacy): \(leftScript.count) Bytes")
            if let rightScript = rightPanelScript {
                logger.debug("  ├─ Right Extension Panel Script (Legacy): \(rightScript.count) Bytes")
                logger.debug("  └─ Default for right panel\(rightPanelDefaultVisible ? "Show" : "Hide")")
            }
        }

        // Record History visit for ACTR app
        historyVM?.recordVisit(tab: newTab)
        persistBrowserPageSession()
    }

    private func unzipActrPackage(archive: URL, to destination: URL) throws {
        // Use /usr/bin/ditto for more reliable unzipping on macOS
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-xk", archive.path, destination.path]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        try process.run()
        process.waitUntilExit()

        if process.terminationStatus != 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let output = String(data: data, encoding: .utf8) ?? ""
            throw NSError(
                domain: "ACTRUnzipError",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: "Unpressure failed: \(output)"]
            )
        }
    }

    private func readPackageManifest(fromRoot root: URL) throws -> ActrPackageManifestInfo {
        try ActrPackageManifestReader.read(fromRoot: root)
    }

    /// Universal script reading method
    private func readScript(fromRoot root: URL, fileName: String) -> String? {
        let fm = FileManager.default

        // If fileName Include path (e. g.) "dist/unified-app.js") Directly spell-read
        if fileName.contains("/") {
            let fullPath = root.appendingPathComponent(fileName)
            if let script = try? String(contentsOf: fullPath, encoding: .utf8) {
                logger.notice(" I found it. \(fileName): \(fullPath.path)")
                return script
            }
        }

        // First look in root directory
        if let direct = try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil).first(where: { $0.lastPathComponent == fileName }) {
            if let script = try? String(contentsOf: direct, encoding: .utf8) {
                logger.notice(" I found it. \(fileName): \(direct.path)")
                return script
            }
        }

        // If root directory is not available, recursively search
        if let enumerator = fm.enumerator(at: root, includingPropertiesForKeys: nil) {
            for case let fileURL as URL in enumerator {
                if fileURL.lastPathComponent == fileName {
                    if let script = try? String(contentsOf: fileURL, encoding: .utf8) {
                        logger.notice(" I found it. \(fileName): \(fileURL.path)")
                        return script
                    }
                }
            }
        }

        return nil
    }

    /// Read guest.js (Backward compatibility)
    private func readGuestScript(fromRoot root: URL) -> String? {
        return readScript(fromRoot: root, fileName: "guest.js")
    }

    private func renderACTRInfoHTML(name: String, version: String, description: String, sourcePath: String) -> String {
        """
        <!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <style>
                body {
                    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
                    padding: 32px;
                    color: #e6e8ee;
                    margin: 0;
                    min-height: 100vh;
                    background: linear-gradient(135deg, #0f172a 0%, #111827 50%, #0b1220 100%);
                }
                .card {
                    max-width: 860px;
                    margin: 0 auto;
                    padding: 28px;
                    background: rgba(255, 255, 255, 0.04);
                    border: 1px solid rgba(255, 255, 255, 0.08);
                    border-radius: 16px;
                    box-shadow: 0 15px 50px rgba(0,0,0,0.35);
                    backdrop-filter: blur(10px);
                }
                h1 { margin: 0 0 12px 0; font-size: 28px; letter-spacing: -0.02em; }
                .meta { color: #9ca3af; margin-bottom: 18px; }
                .pill { display: inline-block; padding: 6px 10px; margin-right: 8px;
                        border-radius: 10px; background: rgba(96, 165, 250, 0.12); color: #bfdbfe; font-size: 12px; }
                .desc { line-height: 1.6; color: #d1d5db; margin-bottom: 20px; }
                .path { font-family: 'Menlo', 'SFMono-Regular', ui-monospace, monospace;
                        background: rgba(255,255,255,0.05); padding: 10px 12px; border-radius: 10px; }
            </style>
        </head>
        <body>
            <div class="card">
                <h1>🧩 \(name)</h1>
                <div class="meta">
                    <span class="pill">version \(version)</span>
                    <span class="pill">ACTR package</span>
                </div>
                <div class="desc">\(description)</div>
                <div class="path">\(sourcePath)</div>
            </div>
        </body>
        </html>
        """
    }
}
