import Foundation
import AppKit // Import AppKit for NSImage
import Combine
import ActriumCore  // AppleActrPackageSignatureError (15-B B8 error pages)

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

    // MARK: - Package Direct-Link Download (15-B B1)
    // Confirm-before-connect flow: confirm → download (size cap) → SHA-256 → import.

    /// Package link awaiting user confirmation (confirm sheet is presented).
    @Published var pendingPackageDownload: PendingPackageDownload?

    /// In-flight or failed package download (progress sheet is presented).
    @Published var activePackageDownload: ActivePackageDownload?

    /// Notice for formal actr:// addresses (realm resolution pending in prototype).
    @Published var actrUriNotice: ActrUriNotice?

    /// actr:// link awaiting invite-code redemption (temporary-flow sheet presented).
    @Published var pendingInviteRedeem: PendingInviteRedeem?

    /// Realms this installation has already redeemed an invite for. Opening
    /// their actr:// links skips the invite sheet (re-asking would dead-end
    /// in `invite_used`). Persisted across launches.
    private var redeemedRealms: Set<String> {
        get { Set(UserDefaults.standard.stringArray(forKey: "actr.redeemedRealms") ?? []) }
        set { UserDefaults.standard.set(Array(newValue), forKey: "actr.redeemedRealms") }
    }

    private func markRedeemed(_ realmName: String) {
        var updated = redeemedRealms
        updated.insert(realmName)
        redeemedRealms = updated
    }

    /// Current download task handle, kept for cancellation.
    private var packageDownloadTask: Task<Void, Never>?

    // MARK: - Instance Creation (15-B B6)
    // Confirm → serviceCreate → URI replace (replace, not push; history keeps only the
    // final instance address; on failure the package URL stays unchanged).

    /// Package import awaiting instance-creation confirmation (confirm sheet).
    @Published var pendingInstanceCreation: PendingInstanceCreation?

    /// Prototype instance store (15 §5: the mock ActorId skeleton keeps the
    /// interface identical to the real ID; switch to the real actr-ffi
    /// backend once delivered — shell call sites unchanged).
    private let serviceInstanceStore: ActrServiceInstanceProviding = MockActrServiceInstanceStore()

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

    func resolveCapabilityPermission(
        tabId: UUID,
        capability: String,
        choice: CapabilityPermissionChoice
    ) {
        guard let index = tabs.firstIndex(where: { $0.id == tabId }) else {
            return
        }
        let tab = tabs[index]
        let stored = CapabilityPermissionStore.shared.decide(
            tabId: tabId,
            appId: tab.appId,
            version: nil,
            capability: capability,
            choice: choice
        )
        guard stored else { return }
        tabs[index].capabilityRevision += 1
        objectWillChange.send()
        persistBrowserPageSession()
    }

    func closeTab(id: UUID) {
        RealmServiceBindings.shared.remove(id)
        CapabilityPermissionStore.shared.clearSessionGrants(tabId: id)
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

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
        } else {
            // Package archive mode — digest-addressed cache (15-B B2, 05 §15).
            do {
                let digest = try ActrPackageDownloadService.sha256FileSync(at: resolvedURL)
                let archiveURL = try ActrPackageCache.installArchive(at: resolvedURL, digest: digest)
                rootDir = try ActrPackageCache.extractArchiveIfNeeded(archiveURL, digest: digest) {
                    try self.validateExtractedPackage(at: $0)
                }
            } catch {
                logger.error(" [restoreGroup] .actr Loading failed:\(error.localizedDescription)")
                return
            }
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir, requireSignature: !isDirectory.boolValue)
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
                capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
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
        // Address-bar navigation must always land somewhere: create a tab
        // when the window has none (e.g. right after a relaunch).
        if currentTabId == nil || tabs.firstIndex(where: { $0.id == currentTabId }) == nil {
            addNewTab(focusAddressBar: false)
        }
        guard let currentTabId = currentTabId,
              let index = tabs.firstIndex(where: { $0.id == currentTabId }) else {
            logger.warning("⚠️ [navigateCurrentTab] No Current tab after creating one")
            return
        }

        let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)

        // 0. actr:// Realm links — the core parser owns the decision
        // (case-insensitive scheme, format rules, local-Realm ownership).
        // Only actr-scheme input enters this branch; every other input falls
        // through to the existing navigation handlers below (local paths,
        // about:, web, package direct links).
        if trimmed.lowercased().hasPrefix("actr://") {
            do {
                let info = try ActriumCore.parseActrUri(
                    uri: trimmed,
                    installationId: InstallationIdProvider.getOrCreate()
                )
                // Local instance links skip the invite gate: there is no remote
                // issuer to redeem with for a local Realm, and the parser has
                // already validated that the address belongs to this
                // installation (UriLocalRealmForeign otherwise).
                if info.isLocal {
                    handleResolvedRealmLink(info, input: trimmed, tabIndex: index)
                    return
                }
                // Temporary flow: opening a remote actr:// link first requires
                // an invite code; a Realm already redeemed goes straight
                // through (its RealmMembership identity is in the Keychain).
                let realmName = [info.realmId, info.domain].compactMap { $0 }.joined(separator: "@")
                guard pendingInviteRedeem == nil else { return }
                guard !redeemedRealms.contains(realmName) else {
                    handleResolvedRealmLink(info, input: trimmed, tabIndex: index)
                    return
                }
                pendingInviteRedeem = PendingInviteRedeem(uri: trimmed, domain: info.domain)
                return
            } catch {
                logger.warning("⚠️ [navigateCurrentTab] actr:// input rejected: \(error)")
                presentRealmLinkError(error)
                return
            }
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
                RealmServiceBindings.shared.remove(currentTabId)
                // Update Tab to system app
                tabs[index].systemTabType = systemType
                tabs[index].url = url
                tabs[index].title = systemType.title
                // Clear web/RN content
                tabs[index].unifiedScript = nil
                tabs[index].capabilityManifestJSON = nil
                tabs[index].leftPanelScript = nil
                tabs[index].rightPanelScript = nil
                tabs[index].guestScript = nil
                tabs[index].guestExtractedPath = nil
                tabs[index].guestEntry = nil
                tabs[index].htmlContent = nil
                tabs[index].contractName = nil
                tabs[index].legacyActorBridge = false
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

            // 3. Package direct link (https URL pointing at an .actr archive):
            // enter the confirm-before-connect download flow instead of loading
            // it as a web page (15-B B1).
            if ActrPackageLinkPolicy.isPackageDownloadURL(url) {
                logger.notice("📦 [navigateCurrentTab] Package direct link: \(url.absoluteString)")
                // Cache hit for a previously downloaded link (15-B B2): the same
                // package reopens without downloading or re-extracting.
                if let digest = ActrPackageCache.cachedDigest(forDownloadURL: url) {
                    logger.notice("📦 [navigateCurrentTab] Cache hit, sha256=\(digest)")
                    importACTRPackage(
                        from: ActrPackageCache.archiveURL(forDigest: digest),
                        knownDigest: digest)
                } else {
                    pendingPackageDownload = PendingPackageDownload(url: url)
                }
                return
            }

            logger.notice("🌐 [navigateCurrentTab] Navigator to URL: \(url.absoluteString)")
            RealmServiceBindings.shared.remove(currentTabId)
            // Update Tab Yes Web Contents
            tabs[index].url = url
            tabs[index].title = url.host ?? url.absoluteString
            // Clear RN Relevant
            tabs[index].unifiedScript = nil
            tabs[index].capabilityManifestJSON = nil
            tabs[index].leftPanelScript = nil
            tabs[index].rightPanelScript = nil
            tabs[index].guestScript = nil
            tabs[index].guestExtractedPath = nil
            tabs[index].guestEntry = nil
            tabs[index].systemTabType = nil
            tabs[index].contractName = nil
            tabs[index].legacyActorBridge = false
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

    // MARK: - Package Direct-Link Download (15-B B1)
    // confirm → download (size cap) → SHA-256 digest → hand over to the import pipeline.

    func confirmPendingPackageDownload() {
        guard let pending = pendingPackageDownload else { return }
        pendingPackageDownload = nil
        beginPackageDownload(pending)
    }

    func cancelPendingPackageDownload() {
        pendingPackageDownload = nil
    }

    func cancelActivePackageDownload() {
        packageDownloadTask?.cancel()
        packageDownloadTask = nil
        activePackageDownload = nil
    }

    func retryActivePackageDownload() {
        guard let active = activePackageDownload else { return }
        let pending = active.pending
        activePackageDownload = nil
        beginPackageDownload(pending)
    }

    private func beginPackageDownload(_ pending: PendingPackageDownload) {
        let active = ActivePackageDownload(pending: pending)
        activePackageDownload = active
        packageDownloadTask = Task { [weak self] in
            await self?.performPackageDownload(active)
        }
    }

    // MARK: - Instance Creation (15-B B6)

    private func offerInstanceCreation(
        for tab: Tab,
        packageTitle: String,
        packagePath: String,
        packageDigest: String,
        displayName: String?,
        permissions: [String]
    ) {
        guard pendingInstanceCreation == nil else { return }
        pendingInstanceCreation = PendingInstanceCreation(
            tabId: tab.id,
            packageTitle: packageTitle,
            packagePath: packagePath,
            packageDigest: packageDigest,
            displayName: displayName,
            permissions: permissions
        )
    }

    func cancelInstanceCreation() {
        pendingInstanceCreation = nil
    }

    // MARK: - Invite Code Redemption (temporary flow)
    // Address-bar actr:// links are gated behind an invite-code sheet; redeem
    // hands over to the Rust-provided method for the follow-up flow.

    func cancelInviteRedeem() {
        pendingInviteRedeem = nil
    }

    /// Redeem: run the full redemption flow through RealmJoinService (decode
    /// → verify → device identity → join); on success mark the Realm as
    /// redeemed and open the instance link automatically.
    func confirmInviteRedeem(code: String) {
        guard let pending = pendingInviteRedeem else { return }
        pendingInviteRedeem = nil
        let trimmedCode = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedCode.isEmpty else { return }

        Task { @MainActor in
            do {
                // The invite must name the same Realm as the requested link:
                // join sends this device's public key and the invite secret to
                // the Realm's issuer, so a mismatched code would leak both to
                // the wrong Realm before any server-side check fires.
                let invite = try ActriumCore.decodeActrInvite(inviteText: trimmedCode)
                let requested = try ActriumCore.parseActrUri(
                    uri: pending.uri,
                    installationId: InstallationIdProvider.getOrCreate()
                )
                let requestedRealm = [requested.realmId, requested.domain]
                    .compactMap { $0 }.joined(separator: "@")
                guard invite.realmName == requestedRealm else {
                    throw NSError(
                        domain: "ActrJoin", code: 4,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "This invite belongs to another Realm.",
                            comment: "Actr join realm mismatch error")])
                }

                let result = try await RealmJoinService.joinWithInvite(
                    inviteText: trimmedCode,
                    label: Host.current().localizedName ?? "Mac"
                )
                markRedeemed(result.realmName)
                logger.notice("🎟️ [InviteRedeem] joined \(result.realmName): member=\(result.memberId), role=\(result.role), key=\(result.keyId)")
                // On a redeemed-realm hit the invite sheet is skipped and the link opens directly.
                navigateCurrentTab(to: result.instanceURL.absoluteString)
            } catch {
                logger.warning("⚠️ [InviteRedeem] redemption failed: \(error)")
                presentRealmLinkError(error)
            }
        }
    }

    func confirmInstanceCreation() {
        guard let pending = pendingInstanceCreation else { return }
        pendingInstanceCreation = nil
        // The idempotency key covers retries within this single create; pressing
        // Creating again later means a new instance = a new key (15 §5).
        let idempotencyKey = UUID().uuidString
        Task { [weak self] in
            guard let self else { return }
            do {
                let handle = try await self.serviceInstanceStore.serviceCreate(
                    packagePath: pending.packagePath,
                    packageDigest: pending.packageDigest,
                    displayName: pending.displayName,
                    idempotencyKey: idempotencyKey
                )
                self.applyInstanceAddress(pending: pending, handle: handle)
            } catch {
                // Red line (15-B): on failure the URI stays the package address —
                // no silent degradation, the tab keeps rendering the package.
                logger.error("❌ [InstanceCreation] failed: \(error.localizedDescription)")
            }
        }
    }

    /// URI replace (replace, not push, 04 §4): the address bar swaps to the instance
    /// address; back does not return to the confirm page (the shell address is
    /// state, not a navigation stack); history records only the final address.
    private func applyInstanceAddress(pending: PendingInstanceCreation, handle: ActrServiceHandle) {
        guard let index = tabs.firstIndex(where: { $0.id == pending.tabId }),
              let instanceURL = ActrInstanceAddress.instanceURL(actorId: handle.actorId)
        else { return }
        let oldURL = tabs[index].url?.absoluteString ?? "nil"
        tabs[index].url = instanceURL
        if let displayName = pending.displayName, !displayName.isEmpty {
            tabs[index].title = displayName
        }
        logger.notice(
            "🔁 [InstanceCreation] URI replace: \(oldURL) → \(instanceURL.absoluteString), actorId=\(handle.actorId)"
        )
        historyVM?.recordVisit(tab: tabs[index])
        objectWillChange.send()
        persistBrowserPageSession()
    }

    private func performPackageDownload(_ active: ActivePackageDownload) async {
        do {
            let localURL = try await ActrPackageDownloadService.download(from: active.pending.url)
            if activePackageDownload?.id == active.id {
                activePackageDownload?.phase = .verifying
            }
            let digest = try await ActrPackageDownloadService.sha256Digest(of: localURL)
            // User cancelled while verifying — keep the cancelled state.
            guard activePackageDownload?.id == active.id else { return }
            logger.notice("✅ [PackageDownload] \(active.pending.filename) downloaded, sha256=\(digest)")

            // Keep the archive in the digest-addressed cache (15-B B2, 05 §15) and
            // remember the link → digest mapping so re-opening the same link skips
            // the download; the import pipeline reuses the cached extraction.
            let cachedArchiveURL = try ActrPackageCache.installArchive(at: localURL, digest: digest)
            // installArchive copies into the cache; the download scratch file
            // is no longer needed.
            try? FileManager.default.removeItem(at: localURL)
            ActrPackageCache.rememberDownloadURL(active.pending.url, digest: digest)

            // Hand over to the existing import pipeline (manifest → runtime tab).
            // ed25519 verification (15-B B3) hooks into this chain next.
            activePackageDownload = nil
            importACTRPackage(from: cachedArchiveURL, knownDigest: digest)
        } catch {
            // Download already cancelled/replaced by the user — keep the new state.
            guard activePackageDownload?.id == active.id else { return }
            let message = (error as? ActrPackageDownloadError)?.errorDescription
                ?? error.localizedDescription
            logger.error("❌ [PackageDownload] failed: \(message)")
            activePackageDownload?.phase = .failed
            activePackageDownload?.errorMessage = message
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
            // Package archive mode — digest-addressed cache (15-B B2, 05 §15).
            do {
                let digest = try ActrPackageDownloadService.sha256FileSync(at: resolvedURL)
                let archiveURL = try ActrPackageCache.installArchive(at: resolvedURL, digest: digest)
                rootDir = try ActrPackageCache.extractArchiveIfNeeded(archiveURL, digest: digest) {
                    try self.validateExtractedPackage(at: $0)
                }
            } catch {
                logger.error("❌ [loadActrPackageIntoCurrentTab] Unpressure failed: \(error.localizedDescription)")
                return
            }
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir, requireSignature: !isDirectory.boolValue)
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

            RealmServiceBindings.shared.remove(tabs[tabIndex].id)
            // Update current tab in-place (like about: URL handling)
            logger.notice("📱 [loadActrPackageIntoCurrentTab] Update Current Tab: \(tabTitle), url=\(appURL.absoluteString)")

            tabs[tabIndex].title = tabTitle
            tabs[tabIndex].url = appURL
            let previousAppId = tabs[tabIndex].appId
            tabs[tabIndex].appId = manifestInfo.appId
            if previousAppId != manifestInfo.appId {
                CapabilityPermissionStore.shared.clearSessionGrants(tabId: tabs[tabIndex].id)
            }
            tabs[tabIndex].legacyActorBridge = ActriumApp.isLegacyActorBridgePackage(resolvedURL)
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
            } else {
                tabs[tabIndex].unifiedScript = nil
                tabs[tabIndex].leftPanelScript = nil
                tabs[tabIndex].rightPanelScript = nil
                tabs[tabIndex].guestScript = guestScript
            }

            tabs[tabIndex].contractName = manifestInfo.contractName
            tabs[tabIndex].contractVersion = manifestInfo.contractVersion
            tabs[tabIndex].permissions = manifestInfo.permissions
            tabs[tabIndex].capabilityManifestJSON = manifestInfo.capabilityManifestJSON
            tabs[tabIndex].leftPanelDefaultVisible = manifestInfo.leftPanelDefaultVisible
            tabs[tabIndex].rightPanelDefaultVisible = manifestInfo.rightPanelDefaultVisible

            // Replace any previous package's extracted guest source: the
            // renderer must only execute bytes from the newly loaded package
            // (same shape as the archive/dev-directory branch above).
            let usesExtractedEntry = !isDirectory.boolValue && unifiedScript != nil
            tabs[tabIndex].guestExtractedPath = usesExtractedEntry ? rootDir.path : nil
            tabs[tabIndex].guestEntry = usesExtractedEntry ? manifestInfo.unifiedScript : nil

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
    /// dispatch). A local instance address focuses an open tab or recovers
    /// the instance through the instance store; a remote link kicks off
    /// authority/Realm Profile discovery, and the tab also updates to the
    /// resolved address.
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

        // 04 §3: a local instance address opens the same instance — focus it
        // when already open, otherwise recover it through the instance store.
        if info.isLocal {
            if let existing = tabs.first(where: { $0.url?.absoluteString == input }) {
                switchToTab(id: existing.id)
                return
            }
            guard let address = URL(string: input) else { return }
            if serviceInstanceStore.resolveActorId(byAddressLabel: realmId) != nil {
                restoreInstanceTab(address: address, title: realmDisplay, isActiveTab: true)
            } else {
                logger.error("❌ [navigateCurrentTab] local instance not found: \(input)")
                actrUriNotice = ActrUriNotice(
                    uri: input,
                    domain: info.domain,
                    detail: "The local instance was not found or has been cleared (SERVICE_NOT_FOUND); the address cannot be opened."
                )
            }
            return
        }

        RealmServiceBindings.shared.remove(tabs[tabIndex].id)
        tabs[tabIndex].reloadNonce &+= 1
        if let discoveryURL = info.discoveryUrl, let url = URL(string: discoveryURL) {
            discoverAuthority(
                url: url,
                domain: info.domain,
                realmId: realmId,
                address: input,
                replacingTabId: tabs[tabIndex].id,
                navigationNonce: tabs[tabIndex].reloadNonce
            )
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
        tabs[tabIndex].guestExtractedPath = nil
        tabs[tabIndex].guestEntry = nil
        tabs[tabIndex].contractName = nil
        objectWillChange.send()
        persistBrowserPageSession()
    }

    /// Realm discovery (verification-funnel layer 1) followed by Realm
    /// Profile discovery (layer 2): fetch the authority descriptor, validate
    /// it, then fetch and verify the Realm Profile it points to. The package
    /// target that comes out feeds package acquisition: the archive digest is
    /// compared against the profile's authenticated `packageSha256` and the
    /// manifest signature is verified with the publisher keys from the
    /// profile funnel (15-B B3) before the package is cached, imported in the
    /// originating tab (same-tab replace) and its publisher keys pinned for
    /// restart restore.
    private func discoverAuthority(
        url: URL,
        domain: String,
        realmId: String,
        address: String,
        replacingTabId: UUID,
        navigationNonce: Int
    ) {
        Task { @MainActor in
            do {
                let data = try await Self.fetchBoundedDocument(
                    from: url,
                    insecureMessage: NSLocalizedString(
                        "The authority information was served over an insecure connection.",
                        comment: "Actr authority insecure transport error"))
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
                let profileData = try await Self.fetchBoundedDocument(
                    from: profileURL,
                    insecureMessage: NSLocalizedString(
                        "The Realm Profile was served over an insecure connection.",
                        comment: "Actr realm profile insecure transport error"))
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

                // Package acquisition: the publisher identity is the domain
                // prefix of the actor type; fetch its signing keys, then the
                // package itself. Verification is the next funnel step.
                let publisherDomain = profile.packageType
                    .split(separator: "/").first.map(String.init) ?? ""
                guard !publisherDomain.isEmpty,
                      let publisherURL = URL(string: "https://\(publisherDomain)/.well-known/actr") else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 8,
                        userInfo: [NSLocalizedDescriptionKey: String(
                            format: NSLocalizedString(
                                "The package type '%@' does not name a publisher domain.",
                                comment: "Actr publisher domain error"),
                            profile.packageType)])
                }
                let publisherData = try await Self.fetchBoundedDocument(
                    from: publisherURL,
                    insecureMessage: NSLocalizedString(
                        "The publisher information was served over an insecure connection.",
                        comment: "Actr publisher insecure transport error"))
                guard let publisherBody = String(data: publisherData, encoding: .utf8) else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 9,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The publisher information is not valid text.",
                            comment: "Actr publisher encoding error")])
                }
                let publisher = try ActriumCore.acceptPublisherProfile(
                    body: publisherBody,
                    domain: publisherDomain
                )
                let activePackageKeys = publisher.packageSigningKeys
                    .filter { $0.status == "active" }
                logger.notice("🔑 [navigateCurrentTab] Publisher accepted for \(publisherDomain): profile sequence=\(publisher.profileSequence), active package keys=\(activePackageKeys.count)")

                guard let packageURL = URL(string: profile.packageHref),
                      packageURL.scheme?.lowercased() == "https" else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 10,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "The package location is invalid.",
                            comment: "Actr package URL error")])
                }
                // 15-B B3: the package is downloaded through the streaming
                // path (64 MiB abort, redirect-host guard), then verified
                // before anything is cached or executed.
                let downloadedURL = try await ActrPackageDownloadService.download(from: packageURL)
                defer { try? FileManager.default.removeItem(at: downloadedURL) }
                let digest = try await ActrPackageDownloadService.sha256Digest(of: downloadedURL)
                // 04 §3: fail closed when the bytes do not match the digest
                // authenticated by the Realm Profile.
                guard digest == profile.packageSha256.lowercased() else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 25,
                        userInfo: [NSLocalizedDescriptionKey: String(
                            format: NSLocalizedString(
                                "The downloaded package does not match the digest in the Realm Profile (PACKAGE_DIGEST_MISMATCH): expected %@…, got %@…",
                                comment: "Actr package digest mismatch"),
                            String(profile.packageSha256.prefix(12)),
                            String(digest.prefix(12)))])
                }

                // Full archive verification via actr-pack (FFI): ZIP
                // structural safety, per-file digests and the manifest
                // Ed25519 signature, anchored on the publisher key the
                // profile funnel authenticated.
                guard let activeKey = activePackageKeys.first(where: { $0.status == "active" }),
                      let trustedKey = Data(base64Encoded: activeKey.publicKeyBase64),
                      trustedKey.count == 32 else {
                    throw NSError(
                        domain: "ActrDiscovery", code: 26,
                        userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                            "No active publisher key is available to verify the package.",
                            comment: "Actr publisher key missing error")])
                }
                let packageBytes = try Data(contentsOf: downloadedURL)
                let keyFingerprint = trustedKey.prefix(8).map { String(format: "%02x", $0) }.joined()
                logger.notice("🔐 [navigateCurrentTab] actr-pack verify: archive=\(packageBytes.count)B, trusted key=\(keyFingerprint)…, profile sha256=\(profile.packageSha256.prefix(12))…")
                let verified = try ActriumCore.verifyActrPackage(
                    archive: packageBytes,
                    trustedPublicKey: trustedKey
                )
                guard verified.manifest.appId == profile.packageType else {
                    throw RealmServiceError(message: "The Front does not match the Actor type in the Realm profile.")
                }
                logger.notice("🔐 [navigateCurrentTab] actr-pack verified: manifest=\(verified.manifest.name) v\(verified.manifest.version), entry=\(verified.manifest.entry), workload profile=\(verified.workloadProfile), workload bytes=\(verified.workloadBytes.count), capabilities=\(verified.manifest.capabilities ?? "nil")")

                let cachedArchiveURL = try ActrPackageCache.installArchive(
                    at: downloadedURL,
                    digest: digest
                )
                try? FileManager.default.removeItem(at: downloadedURL)
                ActrPackageCache.rememberDownloadURL(packageURL, digest: digest)

                // Verify the manifest signature with the publisher keys the
                // profile funnel authenticated, then pin them per publisher
                // domain so a restart restore re-verifies with the same keys.
                let publisherKeys: [Data] = activePackageKeys.compactMap { key in
                    guard let data = Data(base64Encoded: key.publicKeyBase64),
                          data.count == 32
                    else { return nil }
                    return data
                }
                let frontDirectory = try ActrPackageCache.extractArchiveIfNeeded(cachedArchiveURL, digest: digest) {
                    try self.validateExtractedPackage(
                        at: $0,
                        requireSignature: true,
                        additionalTrustedKeys: publisherKeys
                    )
                }
                ActrPackageTrustStore.recordVerifiedPublisherKeys(
                    domain: publisherDomain,
                    keys: activePackageKeys.compactMap { key in
                        guard let data = Data(base64Encoded: key.publicKeyBase64),
                              data.count == 32
                        else { return nil }
                        return ActrPackageTrustStore.TrustedKey(
                            publicKey: data,
                            keyId: key.id,
                            origin: "discovery:\(publisherDomain)"
                        )
                    }
                )
                logger.notice("🔐 [navigateCurrentTab] Package verified: digest matches the Realm Profile, manifest signature verified with \(publisherKeys.count) publisher key(s)")

                let service = try await fetchRealmService(profile)
                let manifest = try readPackageManifest(fromRoot: frontDirectory)
                let binding = try manifest.capabilityManifestJSON.map { capabilities in
                    try RealmServiceBinding(
                        profile: profile,
                        verifiedProfileBody: profileData,
                        front: packageBytes,
                        service: service.archive,
                        keys: [trustedKey, service.key],
                        capabilitiesJSON: capabilities
                    )
                }
                guard let tabIndex = tabs.firstIndex(where: {
                    $0.id == replacingTabId && $0.url == URL(string: address)
                        && $0.reloadNonce == navigationNonce
                }) else { return }
                guard importACTRPackage(
                    from: cachedArchiveURL,
                    knownDigest: digest,
                    presentInstanceCreation: false,
                    requireSignature: true,
                    additionalTrustedKeys: publisherKeys,
                    replacingTabId: replacingTabId
                ) != nil else { return }
                tabs[tabIndex].url = URL(string: address)
                if let binding { RealmServiceBindings.shared.attach(binding, to: replacingTabId) }
                objectWillChange.send()
                persistBrowserPageSession()
            } catch {
                guard tabs.contains(where: {
                    $0.id == replacingTabId && $0.url == URL(string: address)
                        && $0.reloadNonce == navigationNonce
                }) else { return }
                logger.warning("⚠️ [navigateCurrentTab] Realm discovery failed: \(error)")
                presentRealmLinkError(error)
            }
        }
    }

    private func fetchRealmService(_ profile: RealmProfileInfo) async throws -> (archive: Data, key: Data) {
        guard let domain = profile.serviceType.split(separator: "/").first.map(String.init),
              let publisherURL = URL(string: "https://\(domain)/.well-known/actr"),
              let serviceURL = URL(string: profile.serviceHref), serviceURL.scheme == "https" else {
            throw RealmServiceError(message: "The Realm Service location is invalid.")
        }
        let descriptor = try await Self.fetchBoundedDocument(
            from: publisherURL, insecureMessage: "The Service Publisher requires HTTPS."
        )
        let publisher = try ActriumCore.acceptPublisherProfile(
            body: String(decoding: descriptor, as: UTF8.self), domain: domain
        )
        let archiveURL = try await ActrPackageDownloadService.download(from: serviceURL)
        defer { try? FileManager.default.removeItem(at: archiveURL) }
        guard try await ActrPackageDownloadService.sha256Digest(of: archiveURL) == profile.serviceSha256.lowercased() else {
            throw RealmServiceError(message: "The Service archive digest does not match the Realm profile.")
        }
        let archive = try Data(contentsOf: archiveURL)
        for key in publisher.packageSigningKeys where key.status == "active" {
            if let trustedKey = Data(base64Encoded: key.publicKeyBase64), trustedKey.count == 32,
               let verified = try? ActriumCore.verifyActrPackage(archive: archive, trustedPublicKey: trustedKey),
               verified.manifest.appId == profile.serviceType {
                return (archive, trustedKey)
            }
        }
        throw RealmServiceError(message: "The selected Service archive failed Publisher verification.")
    }

    /// Transport checks shared by every discovery fetch.
    private static func requireSuccess(_ response: URLResponse, insecureMessage: String) throws {
        guard let http = response as? HTTPURLResponse,
              (200 ..< 300).contains(http.statusCode) else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? -1
            throw NSError(
                domain: "ActrDiscovery", code: 20,
                userInfo: [NSLocalizedDescriptionKey: String(
                    format: NSLocalizedString(
                        "The request failed (HTTP %ld).",
                        comment: "Actr fetch failure"),
                    status)])
        }
        guard response.url?.scheme?.lowercased() == "https" else {
            throw NSError(
                domain: "ActrDiscovery", code: 21,
                userInfo: [NSLocalizedDescriptionKey: insecureMessage])
        }
    }

    /// Small bounded fetch for discovery documents (authority descriptor,
    /// Realm Profile, Publisher Profile): 2xx + HTTPS via `requireSuccess`,
    /// and a size cap so a hostile endpoint cannot exhaust memory.
    private static func fetchBoundedDocument(
        from url: URL,
        insecureMessage: String
    ) async throws -> Data {
        let (data, response) = try await URLSession.shared.data(from: url)
        try requireSuccess(response, insecureMessage: insecureMessage)
        let maxBytes = 4 * 1024 * 1024
        guard data.count <= maxBytes else {
            throw NSError(
                domain: "ActrDiscovery", code: 26,
                userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                    "The discovery document exceeds the size limit.",
                    comment: "Actr discovery document size error")])
        }
        return data
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
        let scheme = URLComponents(string: input)?.scheme?.lowercased()
        if scheme == "actr" || scheme == "https" || scheme == "http" {
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
        let scheme = URLComponents(string: input)?.scheme?.lowercased()
        if scheme == "http" || scheme == "https" ||
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

        if sourceURL.scheme?.lowercased() == "actr",
           let info = try? ActriumCore.parseActrUri(
               uri: sourceURL.absoluteString, installationId: InstallationIdProvider.getOrCreate()
           ), !info.isLocal {
            navigateCurrentTab(to: sourceURL.absoluteString)
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
            // Package archive mode — digest-addressed cache (15-B B2, 05 §15).
            do {
                let digest = try ActrPackageDownloadService.sha256FileSync(at: resolvedURL)
                let archiveURL = try ActrPackageCache.installArchive(at: resolvedURL, digest: digest)
                rootDir = try ActrPackageCache.extractArchiveIfNeeded(archiveURL, digest: digest) {
                    try self.validateExtractedPackage(at: $0)
                }
            } catch {
                logger.error(" [reloadCurrentTab] Depression failed:\(error.localizedDescription)")
                return
            }
        }

        do {
            let manifestInfo = try readPackageManifest(fromRoot: rootDir, requireSignature: !isDirectory.boolValue)

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
            tabs[index].capabilityManifestJSON = manifestInfo.capabilityManifestJSON
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

    /// From Local .actr Package to create a new tab and load basic information.
    /// - Parameter presentInstanceCreation: archive imports present the instance
    ///   creation confirm sheet after rendering (15-B B6); restart restore passes
    ///   false — the instance already exists.
    /// - Parameter requireSignature: verify manifest.sig before importing an
    ///   archive (15-B B3). Pass false only for the download funnel while the
    ///   publisher-key verification step is still unwired; payload integrity is
    ///   still checked by the manifest reader.
    /// - Returns: the newly created runtime tab, or nil when the import failed.
    @discardableResult
    func importACTRPackage(
        from fileURL: URL,
        knownDigest: String? = nil,
        presentInstanceCreation: Bool = true,
        requireSignature: Bool = true,
        additionalTrustedKeys: [Data] = [],
        replacingTabId: UUID? = nil
    ) -> Tab? {
        let resolvedURL = fileURL.standardizedFileURL
        guard FileManager.default.fileExists(atPath: resolvedURL.path) else {
            logger.error(" Not found .actr Documents:\(resolvedURL.path)")
            return nil
        }

        // Check if it's a directory (dev mode) or archive
        var isDirectory: ObjCBool = false
        FileManager.default.fileExists(atPath: resolvedURL.path, isDirectory: &isDirectory)

        let rootDir: URL
        var packageDigest: String?
        var cachedArchiveURL: URL?

        if isDirectory.boolValue {
            // Directory mode - use directly
            rootDir = resolvedURL
        } else {
            // Package archive mode — digest-addressed cache (15-B B2, 05 §15):
            // archive kept at cache/packages/sha256/<digest>/package.actr,
            // extraction at cache/extracted/sha256/<digest>/ (reused on reopen).
            do {
                let digest = try knownDigest ?? ActrPackageDownloadService.sha256FileSync(at: resolvedURL)
                let archiveURL = try ActrPackageCache.installArchive(at: resolvedURL, digest: digest)
                rootDir = try ActrPackageCache.extractArchiveIfNeeded(archiveURL, digest: digest) {
                    try self.validateExtractedPackage(
                        at: $0,
                        requireSignature: requireSignature,
                        additionalTrustedKeys: additionalTrustedKeys
                    )
                }
                packageDigest = digest
                cachedArchiveURL = archiveURL
            } catch {
                logger.error(" .actr Loading failed:\(error.localizedDescription)")
                return nil
            }
        }

        do {
            let manifestInfo = try readPackageManifest(
                fromRoot: rootDir,
                requireSignature: !isDirectory.boolValue && requireSignature,
                additionalTrustedKeys: additionalTrustedKeys
            )
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
                let newTab = appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    unifiedScript: unified,
                    // 15-B B4: for package archives, hand the renderer the
                    // digest-addressed extraction dir + entry so it can read the
                    // verified bytes directly.
                    guestExtractedPath: isDirectory.boolValue ? nil : rootDir.path,
                    guestEntry: isDirectory.boolValue ? nil : manifestInfo.unifiedScript,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
                    leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                    rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                    themeColor: manifestInfo.themeColor,
                    // Archive imports: history records only the final instance
                    // address after creation (04 §4, 15-B B6).
                    recordHistoryVisit: isDirectory.boolValue,
                    replacingTabId: replacingTabId
                )
                // 15-B B6: after rendering a package archive, offer instance
                // creation (confirm page → serviceCreate → URI replace).
                if presentInstanceCreation, !isDirectory.boolValue,
                   let packageDigest, let cachedArchiveURL {
                    offerInstanceCreation(
                        for: newTab,
                        packageTitle: tabTitle,
                        packagePath: cachedArchiveURL.path,
                        packageDigest: packageDigest,
                        displayName: manifestInfo.name,
                        permissions: manifestInfo.permissions
                    )
                }
                return newTab
            }
            // Legacy: Double Engine Structure
            else if leftPanelScript != nil || rightPanelScript != nil {
                logger.debug("🔄 [BrowserViewModel] Use Legacy Double Engine Structure")
                return appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    leftPanelScript: leftPanelScript,
                    rightPanelScript: rightPanelScript,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
                    leftPanelDefaultVisible: manifestInfo.leftPanelDefaultVisible,
                    rightPanelDefaultVisible: manifestInfo.rightPanelDefaultVisible,
                    themeColor: manifestInfo.themeColor,
                    replacingTabId: replacingTabId
                )
            } else if let script = guestScript {
                // Old architecture: one compatible guest.js
                logger.debug("🔄 [BrowserViewModel] Use compatible mode (guestScript)")
                return appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    guestScript: script,
                    contractName: manifestInfo.contractName,
                    contractVersion: manifestInfo.contractVersion,
                    permissions: manifestInfo.permissions,
                    capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
                    themeColor: manifestInfo.themeColor,
                    replacingTabId: replacingTabId
                )
            } else {
                // No script, display application information page
                let html = renderACTRInfoHTML(
                    name: manifestInfo.name ?? "Unknown App",
                    version: manifestInfo.version ?? "0.0.0",
                    description: manifestInfo.description ?? "No description provided.",
                    sourcePath: resolvedURL.path
                )
                return appendRuntimeTab(
                    title: tabTitle,
                    sourceURL: resolvedURL,
                    appId: manifestInfo.appId,
                    htmlContent: html,
                    replacingTabId: replacingTabId
                )
            }
        } catch {
            // 15-B B8: signature failures get a dedicated, actionable error page
            // (04 §7 PACKAGE_SIGNATURE_INVALID) — never a generic "cannot connect".
            if let signatureError = error as? AppleActrPackageSignatureError {
                logger.error("🔒 [importACTRPackage] \(signatureError.stableCode): \(signatureError.localizedDescription)")
                let errorHTML = renderSignatureErrorHTML(signatureError, sourcePath: resolvedURL.path)
                return appendRuntimeTab(
                    title: resolvedURL.deletingPathExtension().lastPathComponent,
                    sourceURL: resolvedURL,
                    htmlContent: errorHTML,
                    replacingTabId: replacingTabId
                )
            }
            logger.error(" .actr Loading failed:\(error.localizedDescription)")
            let fallbackHTML = renderACTRInfoHTML(
                name: resolvedURL.deletingPathExtension().lastPathComponent,
                version: "unknown",
                description: "Loading failed:\(error.localizedDescription)",
                sourcePath: resolvedURL.path
            )
            return appendRuntimeTab(
                title: resolvedURL.deletingPathExtension().lastPathComponent,
                sourceURL: resolvedURL,
                htmlContent: fallbackHTML,
                replacingTabId: replacingTabId
            )
        }
    }

    /// Dedicated signature-verification error page (15-B B8, 04 §7): shows the
    /// stable code and an actionable suggestion per failure mode.
    private func renderSignatureErrorHTML(_ error: AppleActrPackageSignatureError, sourcePath: String) -> String {
        let suggestion: String
        switch error {
        case .signatureMissing:
            suggestion = "The package is missing its manifest.sig signature. Re-download it; for a local package, re-pack it with actr pkg pack."
        case .noTrustedKeys:
            suggestion = "No trusted public keys are available. Run actr pkg keygen to generate a key, or configure trusted public keys in Actrium/trusted-keys.json and retry."
        case .signatureInvalid:
            suggestion = "Signature verification failed: the package may be tampered with or signed by another key. Re-download it or contact the publisher."
        }
        return """
        <!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8">
            <style>
                body {
                    font-family: -apple-system, BlinkMacSystemFont, sans-serif;
                    padding: 48px; margin: 0; min-height: 100vh;
                    color: #e6e8ee;
                    background: linear-gradient(135deg, #1b0f0f 0%, #111827 60%, #0b1220 100%);
                }
                .card {
                    max-width: 640px; margin: 0 auto; padding: 32px;
                    background: rgba(255, 255, 255, 0.04);
                    border: 1px solid rgba(239, 108, 96, 0.35);
                    border-radius: 16px;
                }
                .code {
                    display: inline-block; padding: 6px 10px; margin-bottom: 14px;
                    border-radius: 10px; background: rgba(239, 108, 96, 0.14);
                    color: #f8b4ae; font-family: monospace; font-size: 12px;
                }
                h1 { margin: 0 0 10px 0; font-size: 24px; }
                .message { color: #d1d5db; line-height: 1.6; margin-bottom: 18px; }
                .suggestion {
                    line-height: 1.6; color: #e5e7eb;
                    background: rgba(255,255,255,0.05); padding: 14px 16px;
                    border-radius: 10px; margin-bottom: 16px;
                }
                .path {
                    font-family: 'Menlo', ui-monospace, monospace; font-size: 11px;
                    color: #9ca3af; word-break: break-all;
                }
            </style>
        </head>
        <body>
            <div class="card">
                <div class="code">PACKAGE_SIGNATURE_INVALID</div>
                <h1>Package signature verification failed</h1>
                <div class="message">\(error.localizedDescription)</div>
                <div class="suggestion">\(suggestion)</div>
                <div class="path">\(escapeHTML(sourcePath))</div>
            </div>
        </body>
        </html>
        """
    }

    // MARK: - Restart Restore (15-B B7)
    // Session tabs (http/https + actr:// instance + actro:// package) are
    // persisted by BrowserDataRepository.saveSession; on launch the session is
    // rebuilt, actr:// addresses rebind via serviceRecover with the same ActorId.

    private var didRestoreSession = false

    /// One-shot launch restore; called from AppDelegate after startup.
    func restoreSessionIfNeeded() {
        guard !didRestoreSession else { return }
        didRestoreSession = true
        guard let session = browserDataRepository.restoreSession(), !session.tabs.isEmpty else { return }
        logger.notice("🔄 [RestoreSession] restoring \(session.tabs.count) session tab(s)")

        // The persisted session is the source of truth on launch: drop the
        // placeholder Start Page created in init() so restoring does not leave
        // an extra blank tab next to the restored ones.
        if tabs.count == 1, let only = tabs.first,
           only.systemTabType == .startPage, only.url == nil {
            tabs.removeAll()
            tabSlotPositions.removeAll()
            currentTabId = nil
        }

        var restoredTabIds: [UUID] = []
        var activeTabId: UUID?

        for (index, sessionTab) in session.tabs.enumerated() {
            guard let url = URL(string: sessionTab.address) else { continue }
            let tabId: UUID?
            switch url.scheme?.lowercased() {
            case "http", "https":
                // Web tab: plain URL restore.
                let tab = Tab(title: sessionTab.title, url: url)
                tabs.append(tab)
                tabSlotPositions[tab.id] = tabs.count - 1
                restoredTabIds.append(tab.id)
                tabId = tab.id
            case "actr":
                if let info = try? ActriumCore.parseActrUri(
                    uri: url.absoluteString, installationId: InstallationIdProvider.getOrCreate()
                ), !info.isLocal, let realm = info.realmId,
                   redeemedRealms.contains("\(realm)@\(info.domain)") {
                    let tab = Tab(title: sessionTab.title, url: url)
                    tabs.append(tab)
                    tabSlotPositions[tab.id] = tabs.count - 1
                    restoredTabIds.append(tab.id)
                    tabId = tab.id
                    handleResolvedRealmLink(info, input: url.absoluteString, tabIndex: tabs.count - 1)
                } else {
                    tabId = nil
                    restoreInstanceTab(address: url, title: sessionTab.title, isActiveTab: index == session.activeTabIndex)
                }
            case "actro":
                // Package address: reuse the existing package load chain.
                let path = actroSourcePath(for: url)
                if let path, FileManager.default.fileExists(atPath: path) {
                    let tab = importACTRPackage(
                        from: URL(fileURLWithPath: path),
                        presentInstanceCreation: false
                    )
                    if let tab {
                        restoredTabIds.append(tab.id)
                        tabId = tab.id
                    } else {
                        tabId = nil
                    }
                } else {
                    tabId = nil
                }
            default:
                tabId = nil
            }
            if index == session.activeTabIndex {
                activeTabId = tabId
            }
        }

        // Focus the persisted active tab (fall back to the last restored tab).
        if activeTabId == nil {
            activeTabId = restoredTabIds.last
        }
        if let activeTabId {
            currentTabId = activeTabId
            actriumMode = .focus(tabId: activeTabId)
        }
        if !restoredTabIds.isEmpty {
            persistBrowserPageSession()
        }
        objectWillChange.send()
    }

    /// actro://<bundleId> resolves via AppRepository; actro:///path is a direct path.
    private func actroSourcePath(for url: URL) -> String? {
        if let host = url.host, !host.isEmpty {
            return AppRepository().getByBundleId(host)?.sourcePath
        }
        return url.path
    }

    /// Recovers one actr:// instance tab: serviceRecover (same ActorId) →
    /// re-render from the digest cache → restore the instance address.
    private func restoreInstanceTab(address: URL, title: String, isActiveTab: Bool) {
        // `URL.pathComponents` starts with "/" for a hierarchical URL, so the
        // address label is the last path component (04 §2).
        let label = address.lastPathComponent
        guard !label.isEmpty,
              let actorId = serviceInstanceStore.resolveActorId(byAddressLabel: label),
              let expectedDigest = serviceInstanceStore.knownDigest(actorId: actorId)
        else {
            logger.error("❌ [RestoreSession] instance record missing for \(address.absoluteString) (SERVICE_NOT_FOUND)")
            return
        }
        Task { [weak self] in
            guard let self else { return }
            do {
                let handle = try await self.serviceInstanceStore.serviceRecover(
                    actorId: actorId,
                    expectedPackageDigest: expectedDigest
                )
                let archiveURL = ActrPackageCache.archiveURL(forDigest: handle.packageDigest)
                guard FileManager.default.fileExists(atPath: archiveURL.path) else {
                    logger.error("❌ [RestoreSession] package cache miss (digest \(handle.packageDigest.prefix(12))…)")
                    return
                }
                guard let tab = self.importACTRPackage(
                    from: archiveURL,
                    knownDigest: handle.packageDigest,
                    presentInstanceCreation: false
                ) else { return }
                // Restore the instance address (not the actro:// package address).
                if let index = self.tabs.firstIndex(where: { $0.id == tab.id }) {
                    self.tabs[index].url = address
                    self.tabs[index].title = title
                }
                if isActiveTab {
                    self.currentTabId = tab.id
                    self.actriumMode = .focus(tabId: tab.id)
                }
                logger.notice("✅ [RestoreSession] recovered instance \(actorId) (sha256=\(handle.packageDigest.prefix(12))…)")
                self.persistBrowserPageSession()
                self.objectWillChange.send()
            } catch {
                logger.error("❌ [RestoreSession] recover failed: \(error.localizedDescription)")
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
                        capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
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
                        capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
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
                        capabilityManifestJSON: manifestInfo.capabilityManifestJSON,
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

    @discardableResult
    private func appendRuntimeTab(
        title: String,
        sourceURL: URL,
        appId: String? = nil,
        htmlContent: String? = nil,
        guestScript: String? = nil,
        leftPanelScript: String? = nil,
        rightPanelScript: String? = nil,
        unifiedScript: String? = nil,
        guestExtractedPath: String? = nil,
        guestEntry: String? = nil,
        contractName: String? = nil,
        contractVersion: Int? = nil,
        permissions: [String] = [],
        capabilityManifestJSON: String? = nil,
        leftPanelDefaultVisible: Bool = true,
        rightPanelDefaultVisible: Bool = false,
        themeColor: String? = nil,
        recordHistoryVisit: Bool = true,
        replacingTabId: UUID? = nil
    ) -> Tab {
        // Use actro:// URL for History and address bar
        let appURL = ActriumAppURLScheme.appURL(filePath: sourceURL.path) ?? sourceURL

        // Same-tab replace (actr:// discovery funnel, 15-B B6): update the
        // originating tab in place so the address bar replaces instead of a
        // second tab appearing. Falls through to append when the tab is gone.
        if let replacingTabId,
           let index = tabs.firstIndex(where: { $0.id == replacingTabId }) {
            tabs[index].title = title
            tabs[index].url = appURL
            tabs[index].appId = appId
            tabs[index].legacyActorBridge = ActriumApp.isLegacyActorBridgePackage(sourceURL)
            tabs[index].htmlContent = htmlContent
            tabs[index].systemTabType = nil
            tabs[index].guestScript = guestScript
            tabs[index].leftPanelScript = leftPanelScript
            tabs[index].rightPanelScript = rightPanelScript
            tabs[index].unifiedScript = unifiedScript
            tabs[index].guestExtractedPath = guestExtractedPath
            tabs[index].guestEntry = guestEntry
            tabs[index].contractName = contractName
            tabs[index].contractVersion = contractVersion
            tabs[index].permissions = permissions
            tabs[index].capabilityManifestJSON = capabilityManifestJSON
            tabs[index].leftPanelDefaultVisible = leftPanelDefaultVisible
            tabs[index].rightPanelDefaultVisible = rightPanelDefaultVisible
            if let themeColor { tabs[index].backgroundColor = themeColor }
            leftPanelVisibility[tabs[index].id] = leftPanelDefaultVisible
            // Force the renderer to pick up the replaced content.
            tabs[index].reloadNonce &+= 1
            currentTabId = tabs[index].id

            logger.notice(" Loaded .actr (replaced tab): \(title)")
            if recordHistoryVisit {
                historyVM?.recordVisit(tab: tabs[index])
            }
            objectWillChange.send()
            persistBrowserPageSession()
            return tabs[index]
        }

        let newTab = Tab(
            title: title,
            url: appURL,
            appId: appId,
            legacyActorBridge: ActriumApp.isLegacyActorBridgePackage(sourceURL),
            htmlContent: htmlContent,
            guestScript: guestScript,
            leftPanelScript: leftPanelScript,
            rightPanelScript: rightPanelScript,
            unifiedScript: unifiedScript,
            guestExtractedPath: guestExtractedPath,
            guestEntry: guestEntry,
            capabilityManifestJSON: capabilityManifestJSON,
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

        // Record History visit for ACTR app. Package-archive imports pass
        // false here: after instance creation the URI is replaced with the
        // instance address and only that final address is recorded (04 §4,
        // 15-B B6) — history keeps only the final address.
        if recordHistoryVisit {
            historyVM?.recordVisit(tab: newTab)
        }
        persistBrowserPageSession()
        return newTab
    }

    private func readPackageManifest(
        fromRoot root: URL,
        requireSignature: Bool = false,
        additionalTrustedKeys: [Data] = []
    ) throws -> ActrPackageManifestInfo {
        try ActrPackageManifestReader.read(
            fromRoot: root,
            requireSignature: requireSignature,
            additionalTrustedKeys: additionalTrustedKeys
        )
    }

    /// Escapes text interpolated into the static HTML error/info pages.
    private func escapeHTML(_ text: String) -> String {
        text
            .replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
            .replacingOccurrences(of: "'", with: "&#39;")
    }

    /// Validates a staging extraction before it enters the digest cache
    /// (05 §15): manifest signature (actr-package/2) plus per-file SHA-256
    /// integrity. Throws on failure so the cache never keeps unverified bytes.
    private func validateExtractedPackage(
        at root: URL,
        requireSignature: Bool = true,
        additionalTrustedKeys: [Data] = []
    ) throws {
        _ = try readPackageManifest(
            fromRoot: root,
            requireSignature: requireSignature,
            additionalTrustedKeys: additionalTrustedKeys
        )
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
                <h1>🧩 \(escapeHTML(name))</h1>
                <div class="meta">
                    <span class="pill">version \(escapeHTML(version))</span>
                    <span class="pill">ACTR package</span>
                </div>
                <div class="desc">\(escapeHTML(description))</div>
                <div class="path">\(escapeHTML(sourcePath))</div>
            </div>
        </body>
        </html>
        """
    }
}
