@testable import Actr
import Foundation
import CryptoKit
import Security
import Testing

@Suite(.serialized)
struct RealmIdentityStoreTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["ACTR_MOBILE_FIXTURE"] != nil,
        "Requires the real Issuer fixture; run tests/e2e/actr/mobile-membership/run.sh ios"))
    func realIssuerJoinHostRegisterAndRevoke() async throws {
        let path = try #require(ProcessInfo.processInfo.environment["ACTR_MOBILE_FIXTURE"])
        let file = path.hasPrefix("/") ? URL(fileURLWithPath: path) :
            try FileManager.default.url(for: .cachesDirectory, in: .userDomainMask,
                appropriateFor: nil, create: true).appendingPathComponent("issuer-fixture.json")
        let fixture = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: file))
        if !path.hasPrefix("/") { try FileManager.default.removeItem(at: file) }
        let realm = try #require(fixture["realm"])
        let issuer = try #require(fixture["issuer"])
        let provider = try #require(fixture["provider_id"])
        let signaling = try #require(fixture["signaling"])
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let service = "io.actrium.membership-test." + UUID().uuidString
        let a = RealmIdentityStore(directory: root.appendingPathComponent("a"), service: service)
        let b = RealmIdentityStore(directory: root.appendingPathComponent("b"), service: service)
        defer {
            try? a.forgetForRejoin(realm: realm); try? b.forgetForRejoin(realm: realm)
            try? FileManager.default.removeItem(at: root)
        }
        let ma = try a.membership(realm: realm, providerId: provider, issuer: issuer, allowInsecureDevelopment: true)
        let mb = try b.membership(realm: realm, providerId: provider, issuer: issuer, allowInsecureDevelopment: true)
        func json(_ text: String) throws -> [String: Any] {
            try #require(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any])
        }
        let invitation = try #require(fixture["invite"])
        let joined = try json(await ma.join(invite: invitation, label: "Swift A"))
        #expect(try json(await ma.join(invite: invitation, label: "retry"))["key_id"] as? String == ma.keyId())
        let memberId = try #require(joined["member_id"] as? String)
        let add = try json(await ma.inviteClient(memberId: memberId,
            expiresAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(900))))
        _ = try await mb.join(invite: #require(add["invite"] as? String), label: "Swift B")
        #expect(ma.keyId() != mb.keyId())
        func register(_ member: RealmMembership, _ observer: (any CredentialObserver)? = nil) async throws -> ActrNode {
            let file = root.appendingPathComponent("runtime.toml")
            try """
            [deployment]
            realm = "\(realm)"
            [issuer_endpoint]
            url = "\(issuer)"
            [signaling]
            url = "\(signaling)"
            [hyper]
            data_dir = "\(root.appendingPathComponent(member.keyId()).path)"
            [hyper.trust]
            kind = "dev_only"
            """.write(to: file, atomically: true, encoding: .utf8)
            return try await ActrNode.linked(config: file.path, type: ActrType(publisher: "sdk", name: "device", version: "1.0.0"),
                workload: dynamicWorkload(lifecycle: MembershipWorkload(), credential: observer), mobileLifecycleObservers: [], membership: member)
        }
        let observerA = RenewalObserver()
        let observerB = RenewalObserver()
        let refA = try await register(ma, observerA).start()
        let refB = try await register(mb, observerB).start()
        for _ in 0..<600 {
            let countA = await observerA.count
            let countB = await observerB.count
            if countA >= 2 && countB >= 2 { break }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        #expect(await observerA.count >= 2)
        #expect(await observerB.count >= 2)
        let beforeRevoke = await observerB.count
        _ = try await mb.revokeClient(keyId: ma.keyId())
        do {
            _ = try await ma.members()
            Issue.record("revoked client accepted")
        } catch RealmMembershipError.Rejected(_, let code) {
            #expect(code == "client_revoked")
        }
        do {
            _ = try await register(ma)
            Issue.record("revoked client registered")
        } catch is ActrError { }
        _ = try await register(mb)
        _ = try await mb.members()
        for _ in 0..<600 {
            if await observerB.count > beforeRevoke { break }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        #expect(await observerB.count > beforeRevoke)
        try await Task.sleep(nanoseconds: 10_000_000_000)
        await refA.stop(); await refB.stop()
    }
    @Test func installationRealmRestartResetAndBackupIsolation() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let service = "io.actrium.membership-test." + UUID().uuidString
        defer {
            try? FileManager.default.removeItem(at: root)
            SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: service] as CFDictionary)
        }
        let a = RealmIdentityStore(directory: root.appendingPathComponent("a"), service: service)
        let b = RealmIdentityStore(directory: root.appendingPathComponent("b"), service: service)
        func member(_ store: RealmIdentityStore, _ realm: String = "one@example.com") throws -> RealmMembership {
            try store.membership(realm: realm, providerId: "provider.example", issuer: "https://issuer.example")
        }
        let old = try member(a)
        #expect(old.keyId() == (try member(RealmIdentityStore(directory: root.appendingPathComponent("a"), service: service))).keyId())
        #expect(old.keyId() != (try member(b)).keyId())
        #expect(old.keyId() != (try member(a, "two@example.com")).keyId())
        let excluded = try root.appendingPathComponent("a").resourceValues(forKeys: [.isExcludedFromBackupKey])
        #expect(excluded.isExcludedFromBackup == true)
        try a.forgetForRejoin(realm: "one@example.com")
        #expect(old.keyId() != (try member(a)).keyId())
        do {
            _ = try await old.members()
            Issue.record("stale signer must not sign with replacement key")
        } catch RealmMembershipError.KeyUnavailable { }
        // Losing the Keychain entry while keeping installation data fails closed.
        let installation = try String(contentsOf: root.appendingPathComponent("a/installation"), encoding: .utf8)
        let digest = SHA256.hash(data: Data("one@example.com".utf8)).map { String(format: "%02x", $0) }.joined()
        let deleted = SecItemDelete([kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: installation + "." + digest,
            kSecAttrService as String: service, kSecAttrSynchronizable as String: false] as CFDictionary)
        #expect(deleted == errSecSuccess)
        #expect(throws: (any Error).self) { try member(a) }
    }
}

private final class MembershipWorkload: Workload, @unchecked Sendable {
    func onStart(ctx: Context) async throws {}
    func onReady(ctx: Context) async throws {}
    func onStop(ctx: Context) async throws {}
    func onError(ctx: Context, event: ErrorEvent) async throws {}
    func dispatch(ctx: Context, envelope: RpcEnvelope) async throws -> Data { Data() }
}

private actor RenewalObserver: CredentialObserver {
    private(set) var count = 0
    func onRenewed(ctx: Context, event: CredentialEvent) async { count += 1 }
    func onExpiring(ctx: Context, event: CredentialEvent) async { }
}
