import Foundation
import ActriumCore

private let browserDataLogger = AppLogger.database
private let macOSHistoryBackfillKey = "apple_macos_local_history_backfilled_v1"
private let macOSBookmarkBackfillKey = "apple_macos_local_bookmarks_backfilled_v1"

final class BrowserDataRepository {
    static let shared = BrowserDataRepository()

    private let database: AppleBrowserStorageDatabase?
    private let syncScheduler: AppleBrowserStorageSyncScheduler?
    // Dedicated client for the local-folder cloud provider path. It shares
    // the opened database with the endpoint scheduler but runs the Rust
    // local-folder rounds directly instead of the reference-server cycle.
    private let localFolderSyncClient: AppleBrowserStorageSyncClient?
    private let providerSettingsStore = BrowserSyncProviderSettingsStore.shared
    private let historyReadLimit = 500
    private let bookmarkReadLimit = 500

    // Session-scoped provider state, mirroring the Windows coordinator: at
    // cold start no round has run, so the planner snapshot carries no error
    // kind; within a session it reflects the last round's provider error
    // classification. Serialized through an actor-free serial queue because
    // the local-folder cycle can be invoked from concurrent UI actions.
    private let providerStateQueue = DispatchQueue(label: "io.actrium.actro.browserSync.localFolderProvider")
    private var lastProviderErrorKindStorage: String?

    private init() {
        let path = Self.databasePath()
        do {
            let openedDatabase = try AppleBrowserStorageDatabase(path: path) { message in
                browserDataLogger.error("\(message)")
            }
            database = openedDatabase
            let syncCoordinator = AppleBrowserStorageSyncCoordinator(
                client: AppleBrowserStorageSyncClient(database: openedDatabase),
                credentialsProvider: BrowserSyncCredentialsProvider.shared
            )
            syncScheduler = AppleBrowserStorageSyncScheduler(coordinator: syncCoordinator)
            localFolderSyncClient = AppleBrowserStorageSyncClient(database: openedDatabase)
            browserDataLogger.notice("Initialized browser storage at: \(path)")
        } catch {
            database = nil
            syncScheduler = nil
            localFolderSyncClient = nil
            browserDataLogger.error("Failed to open browser storage at: \(path), error: \(String(describing: error))")
        }
    }

    func syncNow() async -> String? {
        await syncScheduler?.runNow()
    }

    func syncStatusJson() async -> String? {
        await syncScheduler?.statusJson()
    }

    func scheduleSyncAfterCredentialChange() async {
        await syncScheduler?.scheduleAfterLocalChange(delaySeconds: 0)
    }

    func cancelPendingSync() async {
        await syncScheduler?.cancelPendingSync()
    }

    // MARK: - Local-folder cloud provider sync

    func localFolderProviderSnapshot() -> BrowserSyncProviderSettingsSnapshot {
        providerSettingsStore.load()
    }

    @discardableResult
    func configureLocalFolderProvider(rootURL: URL, bookmarkData: Data) -> BrowserSyncProviderSettingsSnapshot {
        let snapshot = providerSettingsStore.configureLocalFolder(
            rootPath: rootURL.path,
            bookmarkData: bookmarkData
        )
        setLastProviderErrorKind(nil)
        browserDataLogger.notice("Configured local-folder browser sync provider at: \(rootURL.path)")
        return snapshot
    }

    func clearLocalFolderProvider() {
        _ = providerSettingsStore.disconnect()
        setLastProviderErrorKind(nil)
        browserDataLogger.notice("Cleared local-folder browser sync provider")
    }

    // Runs the Rust local-folder sync cycle through the connection planner,
    // mirroring the validated Windows coordinator sequence: plan -> optional
    // one-time membership join -> cycle round, applying each round's provider
    // error outcome. Returns nil only when no provider is configured (the
    // caller has nothing to run); every configured routing returns a result
    // envelope JSON string. The Rust round self-runs the capability and
    // compatibility preflights and does real file IO on the folder, so the
    // whole sequence is bracketed by security-scoped resource access.
    func runLocalFolderSyncCycleNow(observedAtUnixSeconds: Int64? = nil) async -> String? {
        guard let client = localFolderSyncClient else { return nil }
        let observedAt = observedAtUnixSeconds ?? Int64(Date().timeIntervalSince1970)

        var settings = providerSettingsStore.load()
        guard BrowserSyncProviderConnectionPlanner.isConfigured(settings) else { return nil }

        guard let rootURL = providerSettingsStore.resolveRootURL() else {
            browserDataLogger.error(
                "Local-folder browser sync configured but the folder bookmark could not be resolved"
            )
            return nil
        }

        let accessing = rootURL.startAccessingSecurityScopedResource()
        defer {
            if accessing {
                rootURL.stopAccessingSecurityScopedResource()
            }
        }
        let rootDir = rootURL.path

        var plan = BrowserSyncProviderConnectionPlanner.plan(
            settings: settings,
            lastProviderErrorKind: getLastProviderErrorKind(),
            retryAttempt: 0,
            observedAtUnixSeconds: observedAt
        )
        if plan.routing == .notConfigured {
            return nil
        }
        if plan.routing == .blocked {
            return providerBlockedResultJson(
                settings: settings,
                plan: plan,
                retryAttempt: 0,
                observedAtUnixSeconds: observedAt
            )
        }

        if plan.routing == .runLocalFolderMembership {
            let membershipResultJson = await client.runLocalFolderMembershipRound(
                rootDir: rootDir,
                requestJson: buildMembershipJoinRequestJson(
                    settings: settings,
                    generatedAtUnixSeconds: observedAt
                ),
                observedAtUnixSeconds: observedAt
            )
            let membershipEnvelope = BrowserSyncProviderConnectionPlanner.jsonObject(membershipResultJson)
            applyProviderRoundOutcome(membershipEnvelope)

            if BrowserSyncProviderConnectionPlanner.jsonBool(membershipEnvelope?["completed"]) != true {
                return membershipResultJson
            }

            // Persist the membership marker so later launches skip the join
            // round and go straight to the cycle.
            _ = providerSettingsStore.saveMembershipJoined(observedAt)
            settings = providerSettingsStore.load()
            plan = BrowserSyncProviderConnectionPlanner.plan(
                settings: settings,
                lastProviderErrorKind: getLastProviderErrorKind(),
                retryAttempt: 0,
                observedAtUnixSeconds: observedAt
            )
            if plan.routing != .runLocalFolderCycle {
                // Re-evaluation after a successful join is expected to say
                // "run the cycle"; anything else (fresh provider error, policy
                // change) is reported honestly instead of forcing a cycle the
                // policy no longer allows.
                return plan.routing == .blocked
                    ? providerBlockedResultJson(
                        settings: settings,
                        plan: plan,
                        retryAttempt: 0,
                        observedAtUnixSeconds: observedAt
                    )
                    : membershipResultJson
            }
        }

        let cycleResultJson = await client.runLocalFolderSyncCycle(
            rootDir: rootDir,
            scope: settings.scope,
            deviceId: settings.deviceId,
            generatedAtUnixSeconds: observedAt,
            retryAttempt: 0,
            observedAtUnixSeconds: observedAt
        )
        applyProviderRoundOutcome(BrowserSyncProviderConnectionPlanner.jsonObject(cycleResultJson))
        return cycleResultJson
    }

    // Reads the round's provider error classification into the session error
    // state and clears the persisted membership marker on membership- or
    // storage-scoped errors so the next run re-joins the cloud store.
    private func applyProviderRoundOutcome(_ roundEnvelope: [String: Any]?) {
        let diagnostics = roundEnvelope?["providerDiagnostics"] as? [String: Any]
        let providerErrorClassification = diagnostics?["providerErrorClassification"] as? String
        setLastProviderErrorKind(
            BrowserSyncProviderConnectionPlanner.mapProviderErrorClassification(providerErrorClassification)
        )
        if BrowserSyncProviderConnectionPlanner.shouldClearMembershipMarker(providerErrorClassification) {
            _ = providerSettingsStore.clearMembershipMarker()
            browserDataLogger.warning(
                "Local-folder browser sync round reported a membership/storage-scoped error (\(providerErrorClassification ?? "")); cleared the membership marker so the next run re-joins"
            )
        }
    }

    private func buildMembershipJoinRequestJson(
        settings: BrowserSyncProviderSettingsSnapshot,
        generatedAtUnixSeconds: Int64
    ) -> String {
        let request: [String: Any] = [
            "scope": settings.scope,
            "actorDeviceId": settings.deviceId,
            "memberDeviceId": settings.deviceId,
            "displayName": Host.current().localizedName ?? "Mac",
            "platform": "macOS",
            "membershipAction": "connect",
            "generatedAtUnixSeconds": generatedAtUnixSeconds,
        ]
        return BrowserSyncProviderConnectionPlanner.jsonString(request) ?? "{}"
    }

    // Blocked routing: no round runs and no envelope exists, so the
    // scheduleState is the planner's pure translation of the connection
    // status policy into the scheduler's field shape.
    private func providerBlockedResultJson(
        settings: BrowserSyncProviderSettingsSnapshot,
        plan: BrowserSyncProviderConnectionPlan,
        retryAttempt: UInt32,
        observedAtUnixSeconds: Int64
    ) -> String {
        let connectionPolicy = plan.connectionPolicy ?? [:]
        let scheduleState = BrowserSyncProviderConnectionPlanner.buildBlockedScheduleState(
            connectionPolicy: connectionPolicy,
            retryAttempt: retryAttempt,
            observedAtUnixSeconds: observedAtUnixSeconds
        )
        let payload: [String: Any] = [
            "version": 1,
            "storageSchemaVersion": AppleBrowserStorageDatabase.schemaVersion,
            "endpointType": "cloudProviderConnection",
            "credentialsAvailable": true,
            "providerKind": settings.providerKind ?? "",
            "syncResult": NSNull(),
            "classification": BrowserSyncProviderConnectionPlanner.jsonStringValue(connectionPolicy["classification"]),
            "shouldRetry": BrowserSyncProviderConnectionPlanner.jsonBool(connectionPolicy["shouldRetry"]),
            "clientAction": BrowserSyncProviderConnectionPlanner.jsonStringValue(connectionPolicy["providerAction"]),
            "statusPolicy": connectionPolicy,
            "scheduleState": scheduleState,
        ]
        guard JSONSerialization.isValidJSONObject(payload),
              let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]),
              let json = String(data: data, encoding: .utf8)
        else {
            return "null"
        }
        return json
    }

    private func getLastProviderErrorKind() -> String? {
        providerStateQueue.sync { lastProviderErrorKindStorage }
    }

    private func setLastProviderErrorKind(_ value: String?) {
        providerStateQueue.sync { lastProviderErrorKindStorage = value }
    }

    func recordVisit(tab: Tab) {
        guard let url = tab.url else { return }
        recordVisit(title: tab.title, url: url)
    }

    func recordVisit(item: HistoryItem) {
        guard let url = item.appURL else { return }
        recordVisit(title: item.title, url: url)
    }

    func recordVisit(title: String, url: URL) {
        guard Self.isBrowserPageURL(url) else { return }
        let item = AppleBrowserHistoryItem(
            title: normalizedTitle(title, url: url),
            address: url.absoluteString,
            visitedAtUnixSeconds: Int64(Date().timeIntervalSince1970),
            visitCount: 1
        )
        if database?.recordHistoryVisit(item) == true {
            scheduleSyncAfterLocalChange()
        }
    }

    func loadRecentHistory(limit: Int = 500) -> [HistoryItem] {
        database?.loadRecentHistory(limit: min(max(limit, 0), historyReadLimit)).compactMap(historyItem(from:)) ?? []
    }

    func backfillHistoryIfNeeded(_ items: [HistoryItem]) {
        guard let database, !database.hasMetaFlag(macOSHistoryBackfillKey) else { return }
        let browserItems = items.compactMap(browserHistoryItem(from:))
        guard !browserItems.isEmpty else {
            _ = database.setMetaFlag(macOSHistoryBackfillKey)
            return
        }

        let succeeded = browserItems.allSatisfy { database.upsertHistoryItem($0) }
        if succeeded {
            _ = database.setMetaFlag(macOSHistoryBackfillKey)
            scheduleSyncAfterLocalChange()
            browserDataLogger.notice("Backfilled \(browserItems.count) local history items into browser storage")
        }
    }

    func removeHistory(item: HistoryItem) {
        guard let url = item.appURL, Self.isBrowserPageURL(url) else { return }
        _ = database?.deleteHistory(address: url.absoluteString)
    }

    func clearHistory() {
        if database?.clearHistory() == true {
            scheduleSyncAfterLocalChange()
        }
    }

    func upsertBookmark(_ bookmark: Bookmark) {
        guard let item = browserBookmarkItem(from: bookmark) else { return }
        if database?.upsertBookmarkItem(item) == true {
            scheduleSyncAfterLocalChange()
        }
    }

    func loadBookmarks(limit: Int = 500) -> [Bookmark] {
        database?.loadRecentBookmarks(limit: min(max(limit, 0), bookmarkReadLimit)).compactMap(bookmark(from:)) ?? []
    }

    func backfillBookmarksIfNeeded(_ bookmarks: [Bookmark]) {
        guard let database, !database.hasMetaFlag(macOSBookmarkBackfillKey) else { return }
        let browserBookmarks = bookmarks.compactMap(browserBookmarkItem(from:))
        guard !browserBookmarks.isEmpty else {
            _ = database.setMetaFlag(macOSBookmarkBackfillKey)
            return
        }

        let succeeded = browserBookmarks.allSatisfy { database.upsertBookmarkItem($0) }
        if succeeded {
            _ = database.setMetaFlag(macOSBookmarkBackfillKey)
            scheduleSyncAfterLocalChange()
            browserDataLogger.notice("Backfilled \(browserBookmarks.count) local bookmarks into browser storage")
        }
    }

    func removeBookmark(_ bookmark: Bookmark) {
        guard let url = bookmark.appURL, Self.isBrowserPageURL(url) else { return }
        if database?.deleteBookmark(address: url.absoluteString) == true {
            scheduleSyncAfterLocalChange()
        }
    }

    func clearBookmarks() {
        if database?.clearBookmarks() == true {
            scheduleSyncAfterLocalChange()
        }
    }

    /// Restorable session tabs: web pages plus actr:// instance addresses and
    /// actro:// package addresses (15-B B7 — previously only http/https were
    /// persisted, so ActorIds never survived restarts).
    static func isSessionRestorableURL(_ url: URL) -> Bool {
        let scheme = url.scheme?.lowercased()
        return scheme == "http" || scheme == "https"
            || scheme == "actr" || scheme == "actro"
    }

    struct RestoredSessionTab {
        let title: String
        let address: String
    }

    struct RestoredSession {
        let tabs: [RestoredSessionTab]
        let activeTabIndex: Int
    }

    /// Loads the last persisted session (15-B B7 restart restore).
    func restoreSession() -> RestoredSession? {
        guard let snapshot = database?.loadSession(), !snapshot.tabs.isEmpty else { return nil }
        return RestoredSession(
            tabs: snapshot.tabs.map { RestoredSessionTab(title: $0.title, address: $0.address) },
            activeTabIndex: max(0, Int(snapshot.activeTabIndex))
        )
    }

    func saveSession(tabs: [Tab], currentTabId: UUID?) {
        let now = Int64(Date().timeIntervalSince1970)
        let browserTabs = tabs.compactMap { tab -> (id: UUID, item: AppleBrowserSessionTab)? in
            guard let url = tab.url, Self.isSessionRestorableURL(url) else { return nil }
            let item = AppleBrowserSessionTab(
                id: tab.id.uuidString,
                title: normalizedTitle(tab.title, url: url),
                address: url.absoluteString,
                createdAtUnixSeconds: now,
                updatedAtUnixSeconds: now
            )
            return (tab.id, item)
        }

        let activeTabIndex: Int32
        if let currentTabId,
           let index = browserTabs.firstIndex(where: { $0.id == currentTabId }) {
            activeTabIndex = Int32(index)
        } else {
            activeTabIndex = 0
        }

        let snapshot = AppleBrowserSessionSnapshot(
            activeTabIndex: activeTabIndex,
            tabs: browserTabs.map { $0.item },
            surfaceKind: "macos"
        )
        // Core's session normalization accepts only about/http/https/file
        // (browser_data.rs normalize_session_address); AppleCore therefore
        // persists snapshots containing actr:// / actro:// tabs verbatim
        // (AppleBrowserStorageDatabase.coreNormalizedSessionSnapshot returns
        // nil) so instance addresses survive restarts (15-B B7). Extending the
        // Core whitelist itself still belongs to the Rust side (A/组外).
        if database?.saveSession(snapshot: snapshot) == true {
            scheduleSyncAfterLocalChange()
        }
    }

    private func scheduleSyncAfterLocalChange() {
        guard let syncScheduler else { return }
        Task {
            await syncScheduler.scheduleAfterLocalChange()
        }
    }

    private static func databasePath() -> String {
        let fileManager = FileManager.default
        let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let appFolder = appSupport.appendingPathComponent("Actro", isDirectory: true)
        if !fileManager.fileExists(atPath: appFolder.path) {
            try? fileManager.createDirectory(at: appFolder, withIntermediateDirectories: true)
        }
        return AppleBrowserStorageDatabase.databasePath(in: appFolder)
    }

    static func isBrowserPageURL(_ url: URL) -> Bool {
        let scheme = url.scheme?.lowercased()
        return scheme == "http" || scheme == "https"
    }

    private func historyItem(from item: AppleBrowserHistoryItem) -> HistoryItem? {
        guard let url = URL(string: item.address), Self.isBrowserPageURL(url) else { return nil }
        return HistoryItem(
            id: stableUUID(for: "history:\(item.address)"),
            title: normalizedTitle(item.title, url: url),
            appURL: url,
            visitTime: Date(timeIntervalSince1970: TimeInterval(item.visitedAtUnixSeconds))
        )
    }

    private func browserHistoryItem(from item: HistoryItem) -> AppleBrowserHistoryItem? {
        guard let url = item.appURL, Self.isBrowserPageURL(url) else { return nil }
        return AppleBrowserHistoryItem(
            title: normalizedTitle(item.title, url: url),
            address: url.absoluteString,
            visitedAtUnixSeconds: Int64(item.visitTime.timeIntervalSince1970),
            visitCount: 1
        )
    }

    private func browserBookmarkItem(from bookmark: Bookmark) -> AppleBrowserBookmarkItem? {
        guard let url = bookmark.appURL, Self.isBrowserPageURL(url) else { return nil }
        let createdAt = Int64(bookmark.createdAt.timeIntervalSince1970)
        let updatedAt = Int64((bookmark.lastUsedAt ?? bookmark.createdAt).timeIntervalSince1970)
        return AppleBrowserBookmarkItem(
            id: bookmark.id.uuidString,
            title: normalizedTitle(bookmark.title, url: url),
            address: url.absoluteString,
            createdAtUnixSeconds: createdAt,
            updatedAtUnixSeconds: updatedAt,
            favicon: bookmark.favicon
        )
    }

    private func bookmark(from item: AppleBrowserBookmarkItem) -> Bookmark? {
        guard let url = URL(string: item.address), Self.isBrowserPageURL(url) else { return nil }
        return Bookmark(
            id: UUID(uuidString: item.id) ?? stableUUID(for: "bookmark:\(item.address)"),
            title: normalizedTitle(item.title, url: url),
            appURL: url,
            createdAt: Date(timeIntervalSince1970: TimeInterval(item.createdAtUnixSeconds)),
            lastUsedAt: Date(timeIntervalSince1970: TimeInterval(item.updatedAtUnixSeconds)),
            favicon: item.favicon ?? "bookmark.fill"
        )
    }

    private func normalizedTitle(_ title: String, url: URL) -> String {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        if let host = url.host, !host.isEmpty {
            return host
        }
        return url.absoluteString
    }

    private func stableUUID(for value: String) -> UUID {
        let bytes = stableIdentifierBytes(for: value)
        return UUID(uuid: (
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15]
        ))
    }

    private func stableIdentifierBytes(for value: String) -> [UInt8] {
        let first = fnv1a64(value.utf8)
        let second = fnv1a64(("actro:" + value).utf8)
        var bytes = [UInt8](repeating: 0, count: 16)
        for index in 0..<8 {
            bytes[index] = UInt8((first >> UInt64((7 - index) * 8)) & 0xff)
            bytes[index + 8] = UInt8((second >> UInt64((7 - index) * 8)) & 0xff)
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x50
        bytes[8] = (bytes[8] & 0x3f) | 0x80
        return bytes
    }

    private func fnv1a64<T: Sequence>(_ bytes: T) -> UInt64 where T.Element == UInt8 {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in bytes {
            hash ^= UInt64(byte)
            hash = hash &* 0x100000001b3
        }
        return hash
    }
}
