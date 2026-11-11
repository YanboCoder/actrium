import Actr
import Foundation

/// Development fixture only. The harness delivers an invitation after install
/// into backup-excluded Caches; it is never compiled into the application.
@MainActor
enum RealmEnrollment {
    static func membership(config: URL, invite: String? = nil, provider: String? = nil) async throws -> RealmMembership {
        let endpoint = try readRealmRuntimeConfiguration(configPath: config.path)
        let cache = try FileManager.default.url(for: .cachesDirectory, in: .userDomainMask,
            appropriateFor: nil, create: true).appendingPathComponent("actr-enrollment.json")
        var fixture: [String: String] = [:]
        if FileManager.default.fileExists(atPath: cache.path) {
            fixture = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: cache))
        }
        let providerKey = "realm-provider:" + endpoint.realm
        guard let providerId = provider ?? fixture["provider_id"] ?? UserDefaults.standard.string(forKey: providerKey) else {
            throw RealmMembershipError.InvalidInput(reason: "Join this Realm first")
        }
        let membership = try RealmIdentityStore().membership(realm: endpoint.realm, providerId: providerId,
            issuer: endpoint.issuer, allowInsecureDevelopment: true)
        if let invitation = invite ?? fixture["invite"] {
            _ = try await membership.join(invite: invitation, label: "Swift device")
            if FileManager.default.fileExists(atPath: cache.path) { try FileManager.default.removeItem(at: cache) }
            UserDefaults.standard.set(providerId, forKey: providerKey)
        }
        _ = try await membership.members()
        return membership
    }
}
