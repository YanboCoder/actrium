import Foundation

public struct AppleBrowserHistoryItem: Equatable, Sendable {
    public var title: String
    public var address: String
    public var visitedAtUnixSeconds: Int64
    public var visitCount: Int32
    public var favicon: String?

    public init(
        title: String,
        address: String,
        visitedAtUnixSeconds: Int64,
        visitCount: Int32 = 1,
        favicon: String? = nil
    ) {
        self.title = title
        self.address = address
        self.visitedAtUnixSeconds = visitedAtUnixSeconds
        self.visitCount = visitCount
        self.favicon = favicon
    }
}

public struct AppleBrowserBookmarkItem: Equatable, Sendable {
    public var id: String
    public var folderId: String?
    public var title: String
    public var address: String
    public var createdAtUnixSeconds: Int64
    public var updatedAtUnixSeconds: Int64
    public var sortOrder: Int32
    public var favicon: String?
    public var ownerKind: String
    public var ownerId: String

    public init(
        id: String = UUID().uuidString,
        folderId: String? = nil,
        title: String,
        address: String,
        createdAtUnixSeconds: Int64,
        updatedAtUnixSeconds: Int64? = nil,
        sortOrder: Int32 = 0,
        favicon: String? = nil,
        ownerKind: String = "user",
        ownerId: String = ""
    ) {
        self.id = id
        self.folderId = folderId
        self.title = title
        self.address = address
        self.createdAtUnixSeconds = createdAtUnixSeconds
        self.updatedAtUnixSeconds = updatedAtUnixSeconds ?? createdAtUnixSeconds
        self.sortOrder = sortOrder
        self.favicon = favicon
        self.ownerKind = ownerKind
        self.ownerId = ownerId
    }
}

public struct AppleBrowserSessionTab: Equatable, Sendable {
    public var id: String
    public var title: String
    public var address: String
    public var createdAtUnixSeconds: Int64
    public var updatedAtUnixSeconds: Int64

    public init(
        id: String = UUID().uuidString,
        title: String,
        address: String,
        createdAtUnixSeconds: Int64,
        updatedAtUnixSeconds: Int64? = nil
    ) {
        self.id = id
        self.title = title
        self.address = address
        self.createdAtUnixSeconds = createdAtUnixSeconds
        self.updatedAtUnixSeconds = updatedAtUnixSeconds ?? createdAtUnixSeconds
    }
}

public struct AppleBrowserSessionSnapshot: Equatable, Sendable {
    public var activeTabIndex: Int32
    public var tabs: [AppleBrowserSessionTab]
    public var ownerKind: String
    public var ownerId: String
    public var surfaceKind: String
    public var surfaceId: String

    public init(
        activeTabIndex: Int32,
        tabs: [AppleBrowserSessionTab],
        ownerKind: String = "device",
        ownerId: String = "",
        surfaceKind: String = "browser",
        surfaceId: String = "default"
    ) {
        self.activeTabIndex = activeTabIndex
        self.tabs = tabs
        self.ownerKind = ownerKind
        self.ownerId = ownerId
        self.surfaceKind = surfaceKind
        self.surfaceId = surfaceId
    }
}

public final class AppleBrowserStorageDatabase {
    public static var databaseName: String {
        browserStorageDatabaseName()
    }

    public static var schemaVersion: UInt32 {
        browserStorageSchemaVersion()
    }

    private let database: AppleSQLiteDatabase
    private let sql: BrowserStorageRepositorySqlInfo
    private let syncSql: BrowserStorageSyncSqlInfo
    private let schema: BrowserStorageSchemaInfo

    public init(path: String, onError: AppleSQLiteDatabase.ErrorHandler? = nil) throws {
        database = try AppleSQLiteDatabase(path: path, onError: onError)
        sql = browserStorageRepositorySql()
        syncSql = browserStorageSyncSql()
        schema = browserStorageSchema()

        initializeSchema()
    }

    public static func databasePath(in directory: URL) -> String {
        directory.appendingPathComponent(databaseName).path
    }

    public var tableNames: [String] {
        schema.tableNames
    }

    public func hasMetaFlag(_ key: String) -> Bool {
        let rows = database.query(
            "SELECT value FROM browser_schema_meta WHERE key = ? LIMIT 1",
            params: [key]
        )
        return rows.first != nil
    }

    @discardableResult
    public func setMetaFlag(_ key: String, value: String = "1") -> Bool {
        database.execute(
            "INSERT OR REPLACE INTO browser_schema_meta (key, value) VALUES (?, ?)",
            params: [key, value]
        )
    }

    public func loadRecentHistory(limit: Int = 500) -> [AppleBrowserHistoryItem] {
        database.query(sql.selectHistoryRecent, params: [limit]).map { row in
            AppleBrowserHistoryItem(
                title: stringValue(row["title"]),
                address: stringValue(row["address"]),
                visitedAtUnixSeconds: int64Value(row["last_visited_at"]),
                visitCount: int32Value(row["visit_count"]),
                favicon: optionalStringValue(row["favicon"])
            )
        }
    }

    @discardableResult
    public func saveHistory(_ items: [AppleBrowserHistoryItem]) -> Bool {
        transact {
            guard database.execute(sql.deleteAllHistory) else { return false }
            for item in items {
                guard upsertHistoryPage(item) else { return false }
            }
            return true
        }
    }

    @discardableResult
    public func upsertHistoryItem(_ item: AppleBrowserHistoryItem) -> Bool {
        upsertHistoryPage(item)
    }

    @discardableResult
    public func recordHistoryVisit(_ item: AppleBrowserHistoryItem, transition: String = "typed") -> Bool {
        transact {
            guard upsertHistoryPage(item) else { return false }
            return database.execute(
                sql.insertHistoryVisit,
                params: [
                    UUID().uuidString,
                    item.address,
                    item.title,
                    item.visitedAtUnixSeconds,
                    transition,
                ]
            )
        }
    }

    @discardableResult
    public func deleteHistory(address: String) -> Bool {
        database.execute(sql.deleteHistoryByAddress, params: [address])
    }

    @discardableResult
    public func clearHistory() -> Bool {
        database.execute(sql.deleteAllHistory)
    }

    public func loadRecentBookmarks(limit: Int = 500) -> [AppleBrowserBookmarkItem] {
        database.query(sql.selectBookmarksRecent, params: [limit]).map { row in
            AppleBrowserBookmarkItem(
                id: stringValue(row["id"]),
                folderId: optionalStringValue(row["folder_id"]),
                title: stringValue(row["title"]),
                address: stringValue(row["address"]),
                createdAtUnixSeconds: int64Value(row["created_at"]),
                updatedAtUnixSeconds: int64Value(row["updated_at"]),
                sortOrder: int32Value(row["sort_order"]),
                favicon: optionalStringValue(row["favicon"]),
                ownerKind: optionalStringValue(row["owner_kind"]) ?? "user",
                ownerId: optionalStringValue(row["owner_id"]) ?? ""
            )
        }
    }

    @discardableResult
    public func saveBookmarks(_ items: [AppleBrowserBookmarkItem]) -> Bool {
        transact {
            guard database.execute(sql.deleteAllBookmarks) else { return false }
            for (index, item) in items.enumerated() {
                var persistedItem = item
                persistedItem.sortOrder = Int32(index)
                guard upsertBookmark(persistedItem) else { return false }
            }
            return true
        }
    }

    @discardableResult
    public func upsertBookmarkItem(_ item: AppleBrowserBookmarkItem) -> Bool {
        upsertBookmark(item)
    }

    @discardableResult
    public func deleteBookmark(address: String) -> Bool {
        database.execute(sql.deleteBookmarkByAddress, params: [address])
    }

    @discardableResult
    public func deleteBookmark(id: String) -> Bool {
        (database.executeReturningChanges(sql.deleteBookmarkById, params: [id]) ?? 0) > 0
    }

    @discardableResult
    public func clearBookmarks() -> Bool {
        database.execute(sql.deleteAllBookmarks)
    }

    public func loadSession(id: String = "default") -> AppleBrowserSessionSnapshot? {
        let sessionRows = database.query(sql.selectSession, params: [id])
        guard let sessionRow = sessionRows.first else { return nil }

        let tabRows = database.query(sql.selectSessionTabs, params: [id])
        let tabIds = sessionTabIdPlanIds(tabRows.enumerated().map { index, row in
            (id: optionalStringValue(row["tab_id"]), index: index, address: stringValue(row["address"]))
        })
        let tabs = tabRows.enumerated().map { index, row in
            AppleBrowserSessionTab(
                id: tabIds[index],
                title: stringValue(row["title"]),
                address: stringValue(row["address"]),
                createdAtUnixSeconds: int64Value(row["created_at"]),
                updatedAtUnixSeconds: int64Value(row["updated_at"])
            )
        }

        return AppleBrowserSessionSnapshot(
            activeTabIndex: int32Value(sessionRow["active_tab_index"]),
            tabs: tabs,
            ownerKind: optionalStringValue(sessionRow["owner_kind"]) ?? "device",
            ownerId: optionalStringValue(sessionRow["owner_id"]) ?? "",
            surfaceKind: optionalStringValue(sessionRow["surface_kind"]) ?? "browser",
            surfaceId: optionalStringValue(sessionRow["surface_id"]) ?? "default"
        )
    }

    @discardableResult
    public func saveSession(id: String = "default", snapshot: AppleBrowserSessionSnapshot) -> Bool {
        let now = currentUnixSeconds()
        // The Rust core owns whole-snapshot session normalization
        // (`browser_session_snapshot_normalize_json`): canonical addresses,
        // title fallback/truncation, tab cap, active-index clamp, and stable
        // tab ids. Its output is persisted verbatim. When the core resolves
        // the snapshot to no persistable tabs (or the JSON handoff fails),
        // the original snapshot is persisted with plan-resolved tab ids so
        // empty-session saves keep clearing the stored tabs.
        let persisted: AppleBrowserSessionSnapshot
        let tabIds: [String]
        if let normalized = coreNormalizedSessionSnapshot(snapshot) {
            persisted = normalized
            tabIds = normalized.tabs.map(\.id)
        } else {
            persisted = snapshot
            tabIds = sessionTabIdPlanIds(snapshot.tabs.enumerated().map { index, tab in
                (id: tab.id, index: index, address: tab.address)
            })
        }
        return transact {
            guard database.execute(
                sql.upsertSession,
                params: [
                    id,
                    persisted.activeTabIndex,
                    persisted.ownerKind,
                    persisted.ownerId,
                    persisted.surfaceKind,
                    persisted.surfaceId,
                    now,
                    now,
                ]
            ) else {
                return false
            }

            guard database.execute(sql.deleteSessionTabs, params: [id]) else {
                return false
            }

            for (index, tab) in persisted.tabs.enumerated() {
                guard database.execute(
                    sql.insertSessionTab,
                    params: [
                        id,
                        index,
                        tabIds[index],
                        tab.title,
                        tab.address,
                        tab.createdAtUnixSeconds,
                        tab.updatedAtUnixSeconds,
                    ]
                ) else {
                    return false
                }
            }

            return true
        }
    }

    public func applyRemoteSyncBatch(_ batchJson: String) -> String {
        let planJson = browserStorageSyncRemoteApplyPlanJson(batchJson: batchJson)
        guard planJson != "null" else { return "null" }
        return applyRemoteSyncPlan(planJson)
    }

    public func applyRemoteSyncPlan(_ planJson: String) -> String {
        guard let plan = BrowserStorageSyncNativePlan(json: planJson) else {
            return "null"
        }

        guard let resultJson = remoteApplyResultJson(
            plan: plan,
            results: executeRemoteApplyPlan(plan)
        ) else {
            return "null"
        }

        return browserStorageSyncRemoteApplyResultJson(
            planJson: planJson,
            resultJson: resultJson
        )
    }

    public func applySyncAckBatch(_ ackJson: String) -> Bool {
        let planJson = browserStorageSyncAckPlanJson(ackJson: ackJson)
        guard planJson != "null" else { return false }
        return applySyncAckPlan(planJson)
    }

    public func applySyncAckPlan(_ planJson: String) -> Bool {
        guard let plan = BrowserStorageSyncNativePlan(json: planJson) else {
            return false
        }

        return executeSyncAckPlan(plan)
    }

    public func applySyncCycleResponse(_ responseJson: String) -> String {
        let cyclePlanJson = browserStorageSyncClientCyclePlanJson(responseJson: responseJson)
        guard cyclePlanJson != "null",
              let cyclePlan = browserStorageJsonObject(cyclePlanJson),
              let remoteApplyPlanValue = cyclePlan["remoteApplyPlan"],
              let remoteApplyPlanJson = browserStorageJsonStringValue(remoteApplyPlanValue),
              let remoteApplyPlan = BrowserStorageSyncNativePlan(json: remoteApplyPlanJson)
        else {
            return "null"
        }

        var remoteApplyResultJson = "null"
        if !remoteApplyPlan.operations.isEmpty {
            remoteApplyResultJson = applyRemoteSyncPlan(remoteApplyPlanJson)
        }

        let ackJson = browserStorageSyncClientAckBatchAfterRemoteApplyJson(
            responseJson: responseJson,
            remoteApplyResultJson: remoteApplyResultJson
        )
        guard ackJson != "null" else {
            return "null"
        }

        let ackApplied = applySyncAckBatch(ackJson)
        return browserStorageSyncClientCycleApplyResultJson(
            responseJson: responseJson,
            remoteApplyResultJson: remoteApplyResultJson,
            ackApplied: ackApplied
        )
    }

    public func applyCloudCycleClientPlan(_ cycleRoundJson: String) -> String {
        let clientPlanJson = browserStorageSyncCloudCycleClientPlanJson(cycleRoundJson: cycleRoundJson)
        guard clientPlanJson != "null",
              let clientPlan = browserStorageJsonObject(clientPlanJson)
        else {
            return "null"
        }

        var stepResults: [[String: Any]] = []
        var remoteApplyResultJson = "null"

        if let steps = clientPlan["steps"] as? [[String: Any]] {
            for step in steps {
                let stepType = browserStorageJsonString(step["stepType"])
                let required = step["required"].map(browserStorageJsonBool) ?? true
                var success = true

                if stepType == "applyLocalAck" {
                    if let ackPlanValue = clientPlan["ackPlan"],
                       let ackPlanJson = browserStorageJsonStringValue(ackPlanValue) {
                        success = applySyncAckPlan(ackPlanJson)
                    } else {
                        success = false
                    }
                } else if stepType == "applyRemoteBatch" {
                    if let remoteApplyPlanValue = clientPlan["remoteApplyPlan"],
                       let remoteApplyPlanJson = browserStorageJsonStringValue(remoteApplyPlanValue) {
                        remoteApplyResultJson = applyRemoteSyncPlan(remoteApplyPlanJson)
                        success = remoteApplyResultJson != "null"
                    } else {
                        success = false
                    }
                }

                stepResults.append([
                    "stepType": stepType,
                    "success": success,
                    "error": NSNull(),
                ])

                if !success && required {
                    break
                }
            }
        }

        guard JSONSerialization.isValidJSONObject(stepResults),
              let stepResultsData = try? JSONSerialization.data(
                  withJSONObject: stepResults,
                  options: [.sortedKeys]
              ),
              let stepResultsJson = String(data: stepResultsData, encoding: .utf8)
        else {
            return "null"
        }

        return browserStorageSyncCloudCycleClientApplyResultJson(
            cycleRoundJson: cycleRoundJson,
            stepResultsJson: stepResultsJson,
            remoteApplyResultJson: remoteApplyResultJson
        )
    }

    public func exportDirtySyncBatch(
        scope: String = "default",
        deviceId: String,
        generatedAtUnixSeconds: Int64? = nil,
        limitPerCollection: Int = 200
    ) -> String {
        guard let rowsJson = dirtySyncRowsJson(limitPerCollection: limitPerCollection) else {
            return "null"
        }

        return browserStorageSyncBatchFromDirtyRowsJson(
            scope: scope,
            deviceId: deviceId,
            generatedAtUnixSeconds: generatedAtUnixSeconds ?? currentUnixSeconds(),
            rowsJson: rowsJson
        )
    }

    public func exportDirtySyncPushRequest(
        scope: String = "default",
        deviceId: String,
        generatedAtUnixSeconds: Int64? = nil,
        limitPerCollection: Int = 200
    ) -> String {
        guard let rowsJson = dirtySyncRowsJson(limitPerCollection: limitPerCollection) else {
            return "null"
        }

        return browserStorageSyncPushRequestFromDirtyRowsJson(
            scope: scope,
            deviceId: deviceId,
            generatedAtUnixSeconds: generatedAtUnixSeconds ?? currentUnixSeconds(),
            rowsJson: rowsJson
        )
    }

    public func syncCursorsJson(scope: String = "default") -> String {
        let rows = database.query(
            syncSql.selectSyncCursorsForScope,
            params: [scope]
        )
        let cursors: [[String: Any]] = rows.map { row in
            var cursor: [String: Any] = [
                "collection": stringValue(row["collection"]),
                "cursor": stringValue(row["cursor"]),
            ]
            if let lastSyncedAt = row["last_synced_at"], !(lastSyncedAt is NSNull) {
                cursor["lastSyncedAtUnixSeconds"] = int64Value(lastSyncedAt)
            }
            return cursor
        }
        guard JSONSerialization.isValidJSONObject(cursors),
              let data = try? JSONSerialization.data(withJSONObject: cursors, options: [.sortedKeys])
        else {
            return "[]"
        }
        return String(data: data, encoding: .utf8) ?? "[]"
    }

    public func exportSyncCycleRequestPlan(
        baseUrl: String,
        accessToken: String,
        scope: String = "default",
        deviceId: String,
        generatedAtUnixSeconds: Int64? = nil,
        limitPerCollection: Int = 200
    ) -> String {
        let pushRequestJson = exportDirtySyncPushRequest(
            scope: scope,
            deviceId: deviceId,
            generatedAtUnixSeconds: generatedAtUnixSeconds,
            limitPerCollection: limitPerCollection
        )
        guard pushRequestJson != "null" else { return "null" }
        return browserStorageSyncCycleRequestPlanJson(
            baseUrl: baseUrl,
            accessToken: accessToken,
            pushRequestJson: pushRequestJson,
            cursorsJson: syncCursorsJson(scope: scope)
        )
    }

    private func initializeSchema() {
        for statement in schema.statements {
            database.execute(statement)
        }
        migrateBrowserStorageSchemaIfNeeded()
    }

    /// Replays the Rust-owned incremental schema migration plan
    /// (`browser_storage_schema_migration_plan_json`). All DDL literals,
    /// their order, and the schema version stamp live in the Rust core so
    /// migrated databases stay byte-identical across platforms; this adapter
    /// only performs `PRAGMA table_info` column introspection and statement
    /// execution.
    private func migrateBrowserStorageSchemaIfNeeded() {
        guard let plan = browserStorageJsonObject(browserStorageSchemaMigrationPlanJson()) else {
            return
        }
        if let addColumns = plan["addColumns"] as? [[String: Any]] {
            for entry in addColumns {
                guard let table = entry["table"] as? String,
                      let column = entry["column"] as? String,
                      let statement = entry["statement"] as? String,
                      !table.isEmpty, !column.isEmpty, !statement.isEmpty else {
                    continue
                }
                if !columnExists(column, in: table) {
                    _ = database.execute(statement)
                }
            }
        }
        if let statements = plan["idempotentStatements"] as? [String] {
            for statement in statements {
                _ = database.execute(statement)
            }
        }
    }

    private func columnExists(_ column: String, in table: String) -> Bool {
        database.query("PRAGMA table_info(\(table))").contains { row in
            optionalStringValue(row["name"]) == column
        }
    }

    /// Normalizes a whole session snapshot in one batched Rust call
    /// (`browser_session_snapshot_normalize_json`) so canonical addresses,
    /// title rules, the tab cap, active-index clamping, and stable tab ids
    /// stay byte-identical across all platforms. Tab timestamps and the
    /// owner/surface fields are platform-owned columns outside the Rust
    /// contract and are re-attached from the input snapshot. Returns nil when
    /// the core resolves the snapshot to no persistable tabs or the JSON
    /// handoff fails; callers then persist the input snapshot unchanged.
    private func coreNormalizedSessionSnapshot(
        _ snapshot: AppleBrowserSessionSnapshot
    ) -> AppleBrowserSessionSnapshot? {
        // The core's session address normalization only accepts about/http/
        // https/file (browser_data.rs `normalize_session_address`). Actrium
        // instance (actr://) and package (actro://) tabs would be silently
        // dropped from a mixed snapshot, so persist those snapshots verbatim
        // instead of partially normalizing them (15-B B7).
        let hasNonCoreScheme = snapshot.tabs.contains { tab in
            guard let scheme = URLComponents(string: tab.address)?.scheme?.lowercased() else {
                return false
            }
            return scheme == "actr" || scheme == "actro"
        }
        if hasNonCoreScheme {
            return nil
        }

        let request: [String: Any] = [
            // Browser-data snapshot contract version; the core stamps its own
            // version on the normalized output.
            "version": 1,
            "activeTabIndex": Int(snapshot.activeTabIndex),
            "tabs": snapshot.tabs.map { tab in
                ["id": tab.id, "title": tab.title, "address": tab.address]
            },
        ]
        guard JSONSerialization.isValidJSONObject(request),
              let requestData = try? JSONSerialization.data(withJSONObject: request),
              let requestJson = String(data: requestData, encoding: .utf8) else {
            return nil
        }
        let normalizedJson = browserSessionSnapshotNormalizeJson(snapshotJson: requestJson)
        guard let normalized = browserStorageJsonObject(normalizedJson),
              let normalizedTabs = normalized["tabs"] as? [[String: Any]],
              !normalizedTabs.isEmpty else {
            return nil
        }

        var timestampsByTabId: [String: (createdAt: Int64, updatedAt: Int64)] = [:]
        for tab in snapshot.tabs {
            let key = tab.id.trimmingCharacters(in: .whitespacesAndNewlines)
            if !key.isEmpty, timestampsByTabId[key] == nil {
                timestampsByTabId[key] = (tab.createdAtUnixSeconds, tab.updatedAtUnixSeconds)
            }
        }
        // Normalization preserves tab order, so when no tab was dropped the
        // same index maps back to the input tab (covers tabs whose empty id
        // was replaced by a Rust stable id).
        let indexAligned = normalizedTabs.count == snapshot.tabs.count
        let now = currentUnixSeconds()
        let tabs = normalizedTabs.enumerated().map { index, tab -> AppleBrowserSessionTab in
            let tabId = browserStorageJsonString(tab["id"])
            let timestamps = timestampsByTabId[tabId]
                ?? (indexAligned
                    ? (createdAt: snapshot.tabs[index].createdAtUnixSeconds,
                       updatedAt: snapshot.tabs[index].updatedAtUnixSeconds)
                    : nil)
            return AppleBrowserSessionTab(
                id: tabId,
                title: browserStorageJsonString(tab["title"]),
                address: browserStorageJsonString(tab["address"]),
                createdAtUnixSeconds: timestamps?.createdAt ?? now,
                updatedAtUnixSeconds: timestamps?.updatedAt ?? now
            )
        }
        return AppleBrowserSessionSnapshot(
            activeTabIndex: Int32(browserStorageJsonInt(normalized["activeTabIndex"])),
            tabs: tabs,
            ownerKind: snapshot.ownerKind,
            ownerId: snapshot.ownerId,
            surfaceKind: snapshot.surfaceKind,
            surfaceId: snapshot.surfaceId
        )
    }

    /// Resolves stable session tab ids for a whole session in one batched
    /// Rust call (`browser_storage_session_tab_id_plan_json`), so the id
    /// fallback derivation stays byte-identical across all platforms. The
    /// returned array is always index-aligned with `tabs`; if the plan is
    /// unavailable the raw trimmed ids pass through unchanged.
    private func sessionTabIdPlanIds(_ tabs: [(id: String?, index: Int, address: String)]) -> [String] {
        let fallbackIds = tabs.map {
            ($0.id ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard !tabs.isEmpty else { return fallbackIds }
        let request: [[String: Any]] = tabs.map {
            ["id": $0.id ?? "", "index": $0.index, "address": $0.address]
        }
        guard let requestData = try? JSONSerialization.data(withJSONObject: request),
              let requestJson = String(data: requestData, encoding: .utf8) else {
            return fallbackIds
        }
        let planJson = browserStorageSessionTabIdPlanJson(tabsJson: requestJson)
        guard let planData = planJson.data(using: .utf8),
              let plan = (try? JSONSerialization.jsonObject(with: planData)) as? [String: Any],
              let tabIds = plan["tabIds"] as? [String],
              tabIds.count == tabs.count else {
            return fallbackIds
        }
        return tabIds
    }

    private func upsertHistoryPage(_ item: AppleBrowserHistoryItem) -> Bool {
        database.execute(
            sql.upsertHistoryPage,
            params: [
                item.address,
                item.title,
                max(1, item.visitCount),
                item.visitedAtUnixSeconds,
                item.visitedAtUnixSeconds,
                item.visitedAtUnixSeconds,
                item.favicon,
            ]
        )
    }

    private func upsertBookmark(_ item: AppleBrowserBookmarkItem) -> Bool {
        database.execute(
            sql.upsertBookmark,
            params: [
                item.id,
                item.folderId,
                item.title,
                item.address,
                item.createdAtUnixSeconds,
                item.updatedAtUnixSeconds,
                item.sortOrder,
                item.favicon,
                item.ownerKind,
                item.ownerId,
            ]
        )
    }

    private func executeRemoteApplyPlan(_ plan: BrowserStorageSyncNativePlan) -> [[String: Any]] {
        guard database.execute("BEGIN IMMEDIATE TRANSACTION") else {
            return remoteApplyFailureResults(plan, failedOrdinal: nil, message: "begin transaction failed")
        }

        var results: [[String: Any]] = []
        for (ordinal, operation) in plan.operations.enumerated() {
            guard let statement = remoteApplyStatement(for: operation.statementKey) else {
                database.execute("ROLLBACK")
                return remoteApplyFailureResults(
                    plan,
                    failedOrdinal: ordinal,
                    message: "unknown statement key: \(operation.statementKey)"
                )
            }

            guard let rowsAffected = database.executeReturningChanges(statement, params: operation.params) else {
                database.execute("ROLLBACK")
                return remoteApplyFailureResults(
                    plan,
                    failedOrdinal: ordinal,
                    message: "statement execution failed: \(operation.statementKey)"
                )
            }

            results.append([
                "operationIndex": operation.index,
                "rowsAffected": rowsAffected,
            ])
        }

        guard database.execute("COMMIT") else {
            database.execute("ROLLBACK")
            return remoteApplyFailureResults(plan, failedOrdinal: nil, message: "commit transaction failed")
        }

        return results
    }

    private func executeSyncAckPlan(_ plan: BrowserStorageSyncNativePlan) -> Bool {
        guard database.execute("BEGIN IMMEDIATE TRANSACTION") else { return false }

        for operation in plan.operations {
            guard let statement = syncAckStatement(for: operation.statementKey),
                  database.executeReturningChanges(statement, params: operation.params) != nil else {
                database.execute("ROLLBACK")
                return false
            }
        }

        guard database.execute("COMMIT") else {
            database.execute("ROLLBACK")
            return false
        }

        return true
    }

    private func remoteApplyFailureResults(
        _ plan: BrowserStorageSyncNativePlan,
        failedOrdinal: Int?,
        message: String
    ) -> [[String: Any]] {
        plan.operations.enumerated().map { ordinal, operation in
            var error = message
            if let failedOrdinal {
                if ordinal < failedOrdinal {
                    error = "rolled back"
                } else if ordinal > failedOrdinal {
                    error = "not executed"
                }
            }

            return [
                "operationIndex": operation.index,
                "rowsAffected": Int64(0),
                "error": error,
            ]
        }
    }

    private func dirtySyncRowsJson(limitPerCollection: Int) -> String? {
        let limit = max(1, min(limitPerCollection, 1_000))
        let statements = [
            syncSql.selectDirtyHistoryPages,
            syncSql.selectDirtyHistoryVisits,
            syncSql.selectDirtyBookmarkFolders,
            syncSql.selectDirtyBookmarks,
            syncSql.selectDirtySessions,
            syncSql.selectDirtySessionTabs,
        ]
        let rows = statements.flatMap { statement in
            database.query(statement, params: [limit])
        }

        guard JSONSerialization.isValidJSONObject(rows),
              let data = try? JSONSerialization.data(withJSONObject: rows)
        else {
            return nil
        }

        return String(data: data, encoding: .utf8)
    }

    private func remoteApplyResultJson(
        plan: BrowserStorageSyncNativePlan,
        results: [[String: Any]]
    ) -> String? {
        let payload: [String: Any] = [
            "version": plan.version,
            "storageSchemaVersion": plan.storageSchemaVersion,
            "scope": plan.scope,
            "deviceId": plan.deviceId,
            "generatedAtUnixSeconds": plan.generatedAtUnixSeconds,
            "results": results,
        ]

        guard JSONSerialization.isValidJSONObject(payload),
              let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) else {
            return nil
        }

        return String(data: data, encoding: .utf8)
    }

    private func remoteApplyStatement(for statementKey: String) -> String? {
        switch statementKey {
        case "applyRemoteHistoryPage":
            return syncSql.applyRemoteHistoryPage
        case "applyRemoteHistoryVisit":
            return syncSql.applyRemoteHistoryVisit
        case "applyRemoteBookmarkFolder":
            return syncSql.applyRemoteBookmarkFolder
        case "applyRemoteBookmark":
            return syncSql.applyRemoteBookmark
        case "applyRemoteSession":
            return syncSql.applyRemoteSession
        case "applyRemoteSessionTab":
            return syncSql.applyRemoteSessionTab
        default:
            return nil
        }
    }

    private func syncAckStatement(for statementKey: String) -> String? {
        switch statementKey {
        case "markHistoryPageSynced":
            return syncSql.markHistoryPageSynced
        case "markHistoryVisitSynced":
            return syncSql.markHistoryVisitSynced
        case "markBookmarkFolderSynced":
            return syncSql.markBookmarkFolderSynced
        case "markBookmarkSynced":
            return syncSql.markBookmarkSynced
        case "markSessionSynced":
            return syncSql.markSessionSynced
        case "markSessionTabSynced":
            return syncSql.markSessionTabSynced
        case "upsertSyncCursor":
            return syncSql.upsertSyncCursor
        default:
            return nil
        }
    }

    private func transact(_ work: () -> Bool) -> Bool {
        guard database.execute("BEGIN IMMEDIATE TRANSACTION") else { return false }
        guard work() else {
            database.execute("ROLLBACK")
            return false
        }

        guard database.execute("COMMIT") else {
            database.execute("ROLLBACK")
            return false
        }

        return true
    }
}

private struct BrowserStorageSyncNativePlan {
    let version: Int
    let storageSchemaVersion: Int
    let scope: String
    let deviceId: String
    let generatedAtUnixSeconds: Int64
    let operations: [BrowserStorageSyncNativeOperation]

    init?(json: String) {
        guard let data = json.data(using: .utf8),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let operationsJson = root["operations"] as? [Any] else {
            return nil
        }

        let operations = operationsJson.enumerated().compactMap { index, value in
            BrowserStorageSyncNativeOperation(index: index, json: value)
        }

        guard operations.count == operationsJson.count else {
            return nil
        }

        version = browserStorageJsonInt(root["version"])
        storageSchemaVersion = browserStorageJsonInt(root["storageSchemaVersion"])
        scope = browserStorageJsonString(root["scope"])
        deviceId = browserStorageJsonString(root["deviceId"])
        generatedAtUnixSeconds = browserStorageJsonInt64(root["generatedAtUnixSeconds"])
        self.operations = operations
    }
}

private struct BrowserStorageSyncNativeOperation {
    let index: Int
    let statementKey: String
    let params: [Any?]

    init?(index: Int, json: Any) {
        guard let root = json as? [String: Any],
              let statementKey = root["statementKey"] as? String,
              let paramsJson = root["params"] as? [Any] else {
            return nil
        }

        self.index = index
        self.statementKey = statementKey
        params = paramsJson.map(browserStorageSqliteParam)
    }
}

private func currentUnixSeconds() -> Int64 {
    Int64(Date().timeIntervalSince1970)
}

private func browserStorageJsonString(_ value: Any?) -> String {
    value as? String ?? ""
}

private func browserStorageJsonBool(_ value: Any?) -> Bool {
    if let value = value as? Bool {
        return value
    }
    if let value = value as? NSNumber {
        return value.boolValue
    }
    return false
}

private func browserStorageJsonObject(_ json: String) -> [String: Any]? {
    guard let data = json.data(using: .utf8) else { return nil }
    return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
}

private func browserStorageJsonStringValue(_ value: Any) -> String? {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    else {
        return nil
    }

    return String(data: data, encoding: .utf8)
}

private func browserStorageJsonInt(_ value: Any?) -> Int {
    if let value = value as? Int {
        return value
    }
    if let value = value as? Int64 {
        return Int(value)
    }
    if let value = value as? NSNumber {
        return value.intValue
    }
    return 0
}

private func browserStorageJsonInt64(_ value: Any?) -> Int64 {
    if let value = value as? Int64 {
        return value
    }
    if let value = value as? Int {
        return Int64(value)
    }
    if let value = value as? NSNumber {
        return value.int64Value
    }
    return 0
}

private func browserStorageSqliteParam(_ value: Any?) -> Any? {
    guard let value, !(value is NSNull) else {
        return nil
    }

    if let string = value as? String {
        return string
    }

    if let number = value as? NSNumber {
        let doubleValue = number.doubleValue
        if doubleValue.rounded(.towardZero) == doubleValue {
            return number.int64Value
        }
        return doubleValue
    }

    return value
}

private func stringValue(_ value: Any?) -> String {
    optionalStringValue(value) ?? ""
}

private func optionalStringValue(_ value: Any?) -> String? {
    guard !(value is NSNull) else { return nil }
    return value as? String
}

private func int64Value(_ value: Any?) -> Int64 {
    if let value = value as? Int64 {
        return value
    }
    if let value = value as? Int {
        return Int64(value)
    }
    if let value = value as? Int32 {
        return Int64(value)
    }
    return 0
}

private func int32Value(_ value: Any?) -> Int32 {
    let value = int64Value(value)
    if value > Int64(Int32.max) {
        return Int32.max
    }
    if value < Int64(Int32.min) {
        return Int32.min
    }
    return Int32(value)
}
