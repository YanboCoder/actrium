import Foundation
import CryptoKit
import Testing
@testable import Actrium

/// Covers the shell-side package pipeline and instance-store contracts added
/// for the 15-B prototype: link policy, key id, instance addresses,
/// create/recover semantics, and the 05 §15 cache validation ordering.
@MainActor
struct ActrPackagePipelineTests {

    // MARK: - Direct-link policy

    @Test func packageLinkPolicy_acceptsHTTPSActrArchive() {
        #expect(ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "https://cdn.example.com/app.actr")!
        ))
        #expect(ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "https://cdn.example.com/app.ACTR")!
        ))
    }

    @Test func packageLinkPolicy_rejectsNonActrPath() {
        #expect(!ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "https://cdn.example.com/app.zip")!
        ))
        #expect(!ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "https://cdn.example.com/app.actr.zip")!
        ))
    }

    @Test func packageLinkPolicy_allowsLoopbackHTTPOnly() {
        #expect(ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "http://localhost:8080/app.actr")!
        ))
        #expect(ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "http://127.0.0.1/app.actr")!
        ))
        #expect(!ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "http://cdn.example.com/app.actr")!
        ))
    }

    @Test func packageLinkPolicy_rejectsUnsupportedSchemes() {
        #expect(!ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "ftp://cdn.example.com/app.actr")!
        ))
        #expect(!ActrPackageLinkPolicy.isPackageDownloadURL(
            URL(string: "file:///tmp/app.actr")!
        ))
    }

    // MARK: - Trust store key id

    @Test func keyId_matchesActrPackPrefix() {
        let key = Data((0..<32).map { UInt8($0) })
        let hex = SHA256.hash(data: key).map { String(format: "%02x", $0) }.joined()
        let expected = "actr-pub-" + String(hex.prefix(16))

        let keyId = ActrPackageTrustStore.keyId(for: key)
        #expect(keyId == expected)
        // "actr-pub-" (9) + 16 hex characters — not 32.
        #expect(keyId.count == 25)
    }

    // MARK: - Instance address

    @Test func localInstanceAddress_usesLastPathComponentAsLabel() throws {
        let address = try #require(URL(string: "actr://abc.actr.alt/1234567890ab"))
        #expect(ActrInstanceAddress.isLocalInstanceURL(address))
        #expect(address.lastPathComponent == "1234567890ab")
        // Regression guard: pathComponents.first is "/" for hierarchical URLs.
        #expect(address.pathComponents.first == "/")
    }

    @Test func formalRealmAndHTTPSAddressesAreNotLocalInstances() {
        #expect(!ActrInstanceAddress.isLocalInstanceURL(
            URL(string: "actr://acme.com/k7m2q9xh")!
        ))
        #expect(!ActrInstanceAddress.isLocalInstanceURL(
            URL(string: "https://abc.actr.alt/1234567890ab")!
        ))
    }

    // MARK: - Instance store (mock skeleton, 15-B B6/B7)

    @Test func serviceCreate_isIdempotentPerKey() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let first = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: "A",
            idempotencyKey: "create-1"
        )
        let retried = try await store.serviceCreate(
            packagePath: "/tmp/b.actr",
            packageDigest: "sha256-b",
            displayName: "B",
            idempotencyKey: "create-1"
        )

        #expect(first.actorId == retried.actorId)
        #expect(retried.packageDigest == "sha256-a")
        #expect(store.knownDigest(actorId: first.actorId) == "sha256-a")
    }

    @Test func serviceCreate_newKeyCreatesDistinctInstance() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let first = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: nil,
            idempotencyKey: "create-1"
        )
        let second = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: nil,
            idempotencyKey: "create-2"
        )

        #expect(first.actorId != second.actorId)
        #expect(first.actorId.hasPrefix("actr-local-"))
    }

    @Test func serviceRecover_matchesPersistedActorAndDigest() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let created = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: "A",
            idempotencyKey: "create-1"
        )
        let recovered = try await store.serviceRecover(
            actorId: created.actorId,
            expectedPackageDigest: "sha256-a"
        )

        #expect(recovered.actorId == created.actorId)
        #expect(recovered.packageDigest == "sha256-a")
    }

    @Test func serviceRecover_rejectsDigestMismatch() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let created = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: nil,
            idempotencyKey: "create-1"
        )

        await #expect(throws: ActrServiceInstanceError.self) {
            try await store.serviceRecover(
                actorId: created.actorId,
                expectedPackageDigest: "sha256-other"
            )
        }
    }

    @Test func serviceRecover_unknownActorThrowsServiceNotFound() async {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        await #expect(throws: ActrServiceInstanceError.self) {
            try await store.serviceRecover(
                actorId: "actr-local-missing",
                expectedPackageDigest: "sha256-a"
            )
        }
    }

    @Test func resolveActorId_usesAddressLabelTail() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let created = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: nil,
            idempotencyKey: "create-1"
        )
        let address = try #require(URL(string:
            "actr://test.actr.alt/\(created.actorId.suffix(12))"
        ))

        #expect(store.resolveActorId(byAddressLabel: address.lastPathComponent) == created.actorId)
        // The old `pathComponents.first` bug must not resolve anything.
        #expect(store.resolveActorId(byAddressLabel: address.pathComponents.first ?? "") == nil)
    }

    @Test func instanceStore_persistsAcrossReopen() async throws {
        let (store, storeURL) = makeStore()
        defer { try? FileManager.default.removeItem(at: storeURL) }

        let created = try await store.serviceCreate(
            packagePath: "/tmp/a.actr",
            packageDigest: "sha256-a",
            displayName: nil,
            idempotencyKey: "create-1"
        )

        let reopened = MockActrServiceInstanceStore(storeURL: storeURL)
        #expect(reopened.knownDigest(actorId: created.actorId) == "sha256-a")
        #expect(reopened.resolveActorId(byAddressLabel: String(created.actorId.suffix(12))) == created.actorId)
    }

    @Test func errorDescriptions_useStableCodes() {
        #expect(ActrServiceInstanceError.instanceNotFound(actorId: "a")
            .localizedDescription.contains("SERVICE_NOT_FOUND"))
        #expect(ActrServiceInstanceError.digestMismatch(expected: "a", actual: "b")
            .localizedDescription.contains("PACKAGE_DIGEST_MISMATCH"))
    }

    // MARK: - Cache extraction validation (05 §15)

    @Test func extractValidation_commitsOnlyAfterPassing() throws {
        let digest = uniqueDigest()
        let archive = try makeArchive()
        defer { cleanupCache(digest); try? FileManager.default.removeItem(at: archive) }

        var destinationExistedDuringValidation = true
        var stagingExisted = false
        let root = try ActrPackageCache.extractArchiveIfNeeded(archive, digest: digest) { staging in
            stagingExisted = FileManager.default.fileExists(atPath: staging.appendingPathComponent("manifest.toml").path)
            destinationExistedDuringValidation = FileManager.default.fileExists(
                atPath: ActrPackageCache.extractedURL(forDigest: digest).path
            )
        }

        #expect(stagingExisted)
        #expect(!destinationExistedDuringValidation)
        #expect(FileManager.default.fileExists(atPath: root.appendingPathComponent("manifest.toml").path))
    }

    @Test func extractValidation_failureLeavesNoCachedExtraction() throws {
        let digest = uniqueDigest()
        let archive = try makeArchive()
        defer { cleanupCache(digest); try? FileManager.default.removeItem(at: archive) }

        #expect(throws: ValidationFailure.self) {
            _ = try ActrPackageCache.extractArchiveIfNeeded(archive, digest: digest) { _ in
                throw ValidationFailure()
            }
        }

        #expect(!FileManager.default.fileExists(
            atPath: ActrPackageCache.extractedURL(forDigest: digest).path
        ))
    }

    @Test func extractValidation_runsAgainOnCacheHit() throws {
        let digest = uniqueDigest()
        let archive = try makeArchive()
        defer { cleanupCache(digest); try? FileManager.default.removeItem(at: archive) }

        let first = try ActrPackageCache.extractArchiveIfNeeded(archive, digest: digest)
        var validatedRoots: [URL] = []
        let second = try ActrPackageCache.extractArchiveIfNeeded(archive, digest: digest) { root in
            validatedRoots.append(root)
        }

        #expect(first == second)
        #expect(validatedRoots.count == 1)
        #expect(validatedRoots.first?.standardizedFileURL == first.standardizedFileURL)
    }

    @Test func installArchive_isContentAddressedAndDeduplicated() throws {
        let digest = uniqueDigest()
        let archive = try makeArchive()
        defer { cleanupCache(digest); try? FileManager.default.removeItem(at: archive) }

        let first = try ActrPackageCache.installArchive(at: archive, digest: digest)
        let second = try ActrPackageCache.installArchive(at: archive, digest: digest)

        #expect(first == second)
        #expect(first.lastPathComponent == "package.actr")
    }

    // MARK: - Helpers

    private func makeStore() -> (MockActrServiceInstanceStore, URL) {
        let storeURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("actr-instances-\(UUID().uuidString).json")
        return (MockActrServiceInstanceStore(storeURL: storeURL), storeURL)
    }

    private func uniqueDigest() -> String {
        UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
    }

    private func makeArchive() throws -> URL {
        let fm = FileManager.default
        let source = fm.temporaryDirectory
            .appendingPathComponent("actr-cache-src-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: source, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: source) }
        try "name = \"demo\"\n".write(
            to: source.appendingPathComponent("manifest.toml"),
            atomically: true,
            encoding: .utf8
        )
        try "console.log('demo')\n".write(
            to: source.appendingPathComponent("unified-app.js"),
            atomically: true,
            encoding: .utf8
        )

        let archive = fm.temporaryDirectory
            .appendingPathComponent("actr-cache-\(UUID().uuidString).actr")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        // No --keepParent: the archive must expose manifest.toml at its root,
        // matching how `.actr` packages are laid out.
        process.arguments = ["-c", "-k", "--sequesterRsrc", source.path, archive.path]
        try process.run()
        process.waitUntilExit()
        #expect(process.terminationStatus == 0)
        return archive
    }

    private func cleanupCache(_ digest: String) {
        let fm = FileManager.default
        try? fm.removeItem(at: ActrPackageCache.extractedURL(forDigest: digest))
        try? fm.removeItem(at: ActrPackageCache.archiveURL(forDigest: digest).deletingLastPathComponent())
    }
}

private struct ValidationFailure: Error {}
