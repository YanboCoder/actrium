import Foundation
import XCTest
@testable import ActriumCore

/// 15-B B7: session snapshots containing actr:// / actro:// tabs must survive
/// a save/load round-trip. The core's address normalization only understands
/// about/http/https/file, so AppleBrowserStorageDatabase falls back to verbatim
/// persistence for those snapshots.
final class AppleBrowserSessionActrTabTests: XCTestCase {
    private var databaseURL: URL!

    override func setUp() {
        super.setUp()
        databaseURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("actrium-session-tests-\(UUID().uuidString).db")
    }

    override func tearDown() {
        if let databaseURL {
            let fileManager = FileManager.default
            try? fileManager.removeItem(at: databaseURL)
            try? fileManager.removeItem(at: URL(fileURLWithPath: databaseURL.path + "-shm"))
            try? fileManager.removeItem(at: URL(fileURLWithPath: databaseURL.path + "-wal"))
        }
        super.tearDown()
    }

    func testMixedSessionKeepsActrAndActroTabs() throws {
        let database = try AppleBrowserStorageDatabase(path: databaseURL.path)
        let snapshot = makeSnapshot(
            activeTabIndex: 1,
            addresses: [
                "https://example.com/",
                "actr://abc.actr.alt/1234567890ab",
                "actro://io.actrium.demo",
            ]
        )

        XCTAssertTrue(database.saveSession(snapshot: snapshot))

        let loaded = try XCTUnwrap(database.loadSession())
        XCTAssertEqual(loaded.tabs.map(\.address), [
            "https://example.com/",
            "actr://abc.actr.alt/1234567890ab",
            "actro://io.actrium.demo",
        ])
        XCTAssertEqual(loaded.activeTabIndex, 1)
    }

    func testActrOnlySessionPersistsVerbatim() throws {
        let database = try AppleBrowserStorageDatabase(path: databaseURL.path)
        let snapshot = makeSnapshot(
            activeTabIndex: 0,
            addresses: ["actr://abc.actr.alt/1234567890ab"]
        )

        XCTAssertTrue(database.saveSession(snapshot: snapshot))

        let loaded = try XCTUnwrap(database.loadSession())
        XCTAssertEqual(loaded.tabs.map(\.address), ["actr://abc.actr.alt/1234567890ab"])
    }

    func testWebOnlySessionStillUsesCoreNormalization() throws {
        let database = try AppleBrowserStorageDatabase(path: databaseURL.path)
        let snapshot = makeSnapshot(
            activeTabIndex: 1,
            addresses: [
                "about:Blank",
                "HTTPS://Example.COM:443/Path",
            ]
        )

        XCTAssertTrue(database.saveSession(snapshot: snapshot))

        let loaded = try XCTUnwrap(database.loadSession())
        // The verbatim fallback is scoped to actr/actro tabs; ordinary web
        // sessions keep the core's canonical form.
        XCTAssertEqual(loaded.tabs.map(\.address), [
            "about:blank",
            "https://example.com/Path",
        ])
    }

    private func makeSnapshot(
        activeTabIndex: Int32,
        addresses: [String]
    ) -> AppleBrowserSessionSnapshot {
        let now: Int64 = 1_700_000_000
        return AppleBrowserSessionSnapshot(
            activeTabIndex: activeTabIndex,
            tabs: addresses.enumerated().map { index, address in
                AppleBrowserSessionTab(
                    id: "tab-\(index)",
                    title: "Tab \(index)",
                    address: address,
                    createdAtUnixSeconds: now,
                    updatedAtUnixSeconds: now
                )
            },
            surfaceKind: "macos"
        )
    }
}
