import Foundation
import Testing
@testable import Actrium

/// Exercises the package-backed Service runtime seam with an injected backend,
/// so create/recover, digest enforcement, and the per-instance loopback
/// transport are covered without a real Loran/issuer stack.
@MainActor
struct ActrServiceRuntimeTests {

    private final class FakeBackend: ActrServiceRuntimeBackend, @unchecked Sendable {
        let actorId: String

        init(actorId: String) {
            self.actorId = actorId
        }

        func call(route: String, payload: Data, timeoutMs: Int64) async throws -> Data {
            payload
        }

        func tell(route: String, payload: Data) async throws {}

        func stop() async {}
    }

    private func makePackage() throws -> (url: URL, digest: String) {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("actr-runtime-\(UUID().uuidString).actr")
        try Data("verified-package-bytes".utf8).write(to: url)
        let digest = try ActrPackageDownloadService.sha256FileSync(at: url)
        return (url, digest)
    }

    private func makeStore(
        backendFactory: @escaping (String) async throws -> ActrServiceRuntimeBackend
    ) -> (ActrServiceRuntime, URL) {
        let storeURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("actr-runtime-store-\(UUID().uuidString).json")
        let store = ActrServiceRuntime(
            storeURL: storeURL,
            backendFactory: backendFactory
        )
        return (store, storeURL)
    }

    @Test func serviceCreate_startsBackendAndReturnsLoopbackEndpoint() async throws {
        let package = try makePackage()
        let actorId = "local@actr.alt#123456789012"
        let (store, storeURL) = makeStore { _ in FakeBackend(actorId: actorId) }
        defer {
            try? FileManager.default.removeItem(at: package.url)
            try? FileManager.default.removeItem(at: storeURL)
        }

        let handle = try await store.serviceCreate(
            packagePath: package.url.path,
            packageDigest: package.digest,
            displayName: "Echo",
            idempotencyKey: "create-1"
        )

        #expect(handle.actorId == actorId)
        #expect(handle.serviceCallUrl != nil)
        #expect(store.knownDigest(actorId: actorId) == package.digest)
        #expect(store.resolveActorId(byAddressLabel: String(actorId.suffix(12))) == actorId)
    }

    @Test func loopbackEndpoint_roundTripsCallAndTell() async throws {
        let package = try makePackage()
        let (store, storeURL) = makeStore { _ in FakeBackend(actorId: "local@actr.alt#000000000001") }
        defer {
            try? FileManager.default.removeItem(at: package.url)
            try? FileManager.default.removeItem(at: storeURL)
        }

        let handle = try await store.serviceCreate(
            packagePath: package.url.path,
            packageDigest: package.digest,
            displayName: nil,
            idempotencyKey: "create-1"
        )
        let serviceCallUrl = try #require(handle.serviceCallUrl)
        let callURL = try #require(URL(string: serviceCallUrl))

        let response = try await post(
            to: callURL,
            body: [
                "routeKey": "echo.EchoService.Echo",
                "payloadBase64": Data([1, 2, 3]).base64EncodedString(),
                "timeoutMs": 5_000,
            ]
        )
        #expect(response["success"] as? Bool == true)
        #expect(response["payloadBase64"] as? String == Data([1, 2, 3]).base64EncodedString())

        var tellComponents = URLComponents(url: callURL, resolvingAgainstBaseURL: false)
        tellComponents?.path = "/v1/tell"
        let tellURL = try #require(tellComponents?.url)
        let tellResponse = try await post(
            to: tellURL,
            body: [
                "routeKey": "echo.EchoService.Echo",
                "payloadBase64": Data([4]).base64EncodedString(),
            ]
        )
        #expect(tellResponse["success"] as? Bool == true)
    }

    @Test func serviceCreate_rejectsDigestMismatchAndNeverStartsBackend() async throws {
        let package = try makePackage()
        let (store, storeURL) = makeStore { _ in FakeBackend(actorId: "local@actr.alt#000000000002") }
        defer {
            try? FileManager.default.removeItem(at: package.url)
            try? FileManager.default.removeItem(at: storeURL)
        }

        await #expect(throws: ActrServiceInstanceError.self) {
            try await store.serviceCreate(
                packagePath: package.url.path,
                packageDigest: String(repeating: "0", count: 64),
                displayName: nil,
                idempotencyKey: "create-1"
            )
        }
    }

    @Test func serviceCreate_failsClosedWhenBackendCannotStart() async throws {
        let package = try makePackage()
        let (store, storeURL) = makeStore { _ in
            throw ActrServiceInstanceError.internalFailure("issuer unreachable")
        }
        defer {
            try? FileManager.default.removeItem(at: package.url)
            try? FileManager.default.removeItem(at: storeURL)
        }

        await #expect(throws: ActrServiceInstanceError.self) {
            try await store.serviceCreate(
                packagePath: package.url.path,
                packageDigest: package.digest,
                displayName: nil,
                idempotencyKey: "create-1"
            )
        }
    }

    @Test func serviceRecover_rebindsPersistedActorAndDigest() async throws {
        let package = try makePackage()
        let actorId = "local@actr.alt#000000000003"
        let (store, storeURL) = makeStore { _ in FakeBackend(actorId: actorId) }
        defer {
            try? FileManager.default.removeItem(at: package.url)
            try? FileManager.default.removeItem(at: storeURL)
        }

        _ = try await store.serviceCreate(
            packagePath: package.url.path,
            packageDigest: package.digest,
            displayName: nil,
            idempotencyKey: "create-1"
        )

        // A fresh process reopens the persisted store and recovers by ActorId.
        let reopened = ActrServiceRuntime(
            storeURL: storeURL,
            backendFactory: { _ in FakeBackend(actorId: actorId) }
        )
        let recovered = try await reopened.serviceRecover(
            actorId: actorId,
            expectedPackageDigest: package.digest
        )
        #expect(recovered.actorId == actorId)
        #expect(recovered.serviceCallUrl != nil)

        await #expect(throws: ActrServiceInstanceError.self) {
            try await reopened.serviceRecover(
                actorId: actorId,
                expectedPackageDigest: String(repeating: "f", count: 64)
            )
        }
    }

    private func post(to url: URL, body: [String: Any]) async throws -> [String: Any] {
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request)
        #expect((response as? HTTPURLResponse)?.statusCode == 200)
        return try #require(
            try JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
    }
}
