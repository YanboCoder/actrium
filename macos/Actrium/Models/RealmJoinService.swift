import Foundation
import ActriumCore
import Actr
import ActrBindings

/// The outcome of a successful invite redemption.
struct RealmJoinResult {
    /// Canonical Realm name `<realm_id>@<domain>`.
    let realmName: String
    let memberId: String
    let role: String
    let keyId: String
    /// `actr://<domain>/<realm_id>` — feed it to the existing open-instance
    /// flow (URI parsing → authority → profile → package).
    let instanceURL: URL
}

/// The complete invite redemption flow: decode the `ri_` text, verify the
/// Realm through the existing authority/profile funnel, create this
/// installation's device identity, and exchange the invite.
///
/// Reuses the same core verification entry points as the address-bar
/// discovery flow (`acceptAuthorityDescriptor`, `acceptRealmProfile`) and
/// the same device identity storage as #417 (`RealmIdentityStore`); it
/// changes nothing about how those steps verify.
enum RealmJoinService {
    /// Redeem `inviteText` (an `ri_…` invite) and join as a new device.
    ///
    /// - Parameters:
    ///   - inviteText: the `ri_…` invite as received out of band.
    ///   - label: a human-readable client label (length-limited by the Issuer).
    ///   - allowInsecureDevelopment: opt-in HTTP/plaintext for dev stacks only.
    /// - Throws: `CoreError` for decode/verification failures, and
    ///   `RealmMembershipError` for redemption failures (`Rejected` carries
    ///   the stable codes `invite_used` / `invite_expired` / `invite_revoked`
    ///   / `invite_realm_mismatch`; `Uncertain` means the same key may retry
    ///   the same invite).
    static func joinWithInvite(
        inviteText: String,
        label: String,
        allowInsecureDevelopment: Bool = false
    ) async throws -> RealmJoinResult {
        // 1. Decode the invite locally: learn the Realm name. The secret
        //    stays inside the invite text; `join` consumes it in Rust.
        let invite = try ActriumCore.decodeActrInvite(inviteText: inviteText)
        let realmName = invite.realmName

        guard let at = realmName.firstIndex(of: "@") else {
            throw joinError("The invite names a malformed Realm.")
        }
        let realmId = String(realmName[..<at])
        let domain = String(realmName[realmName.index(after: at)...])
        guard !realmId.isEmpty, !domain.isEmpty else {
            throw joinError("The invite names a malformed Realm.")
        }

        // 2. Realm discovery: the authority descriptor for the Realm domain.
        guard let discoveryURL = URL(string: "https://\(domain)/.well-known/actr") else {
            throw joinError("The Realm domain in the invite is invalid.")
        }
        let (descriptorBody, descriptorResponse) =
            try await URLSession.shared.data(from: discoveryURL)
        try requireHTTPS(descriptorResponse)
        try requireSuccess(descriptorResponse)
        guard let descriptorText = String(data: descriptorBody, encoding: .utf8) else {
            throw joinError("The authority information is not valid text.")
        }
        let descriptor = try ActriumCore.acceptAuthorityDescriptor(
            body: descriptorText,
            domain: domain
        )

        // 3. Realm Profile: verify it with the keys accepted in step 2 and
        //    learn the membership endpoints (issuer, provider id).
        var profilePath = descriptor.realmsHref
        if profilePath.hasSuffix("/") { profilePath.removeLast() }
        profilePath += "/\(realmId)"
        guard let profileURL = URL(string: profilePath) else {
            throw joinError("The Realm Profile location is invalid.")
        }
        let (profileBody, profileResponse) = try await URLSession.shared.data(from: profileURL)
        try requireHTTPS(profileResponse)
        try requireSuccess(profileResponse)
        guard let profileText = String(data: profileBody, encoding: .utf8) else {
            throw joinError("The Realm Profile is not valid text.")
        }
        let profile = try ActriumCore.acceptRealmProfile(
            body: profileText,
            realmName: realmName,
            signingKeysJson: descriptor.signingKeysJson
        )

        // 4. Device identity (created and persisted on first use) and the
        //    invite exchange. The same signer later registers the Host.
        let store = try RealmIdentityStore()
        let membership = try store.membership(
            realm: profile.realmName,
            providerId: profile.providerId,
            issuer: profile.issuerUrl,
            allowInsecureDevelopment: allowInsecureDevelopment
        )
        let memberJSON = try await membership.join(invite: inviteText, label: label)

        // 5. Surface the redemption result and the instance address.
        guard let member = try JSONSerialization.jsonObject(with: Data(memberJSON.utf8))
                as? [String: Any],
            let memberId = member["member_id"] as? String,
            let role = member["role"] as? String,
            let keyId = member["key_id"] as? String else {
            throw joinError("The join response could not be read.")
        }
        guard let instanceURL = URL(string: "actr://\(domain)/\(realmId)") else {
            throw joinError("The instance address could not be built.")
        }
        return RealmJoinResult(
            realmName: profile.realmName,
            memberId: memberId,
            role: role,
            keyId: keyId,
            instanceURL: instanceURL
        )
    }

    // MARK: - Helpers

    private static func joinError(_ message: String) -> NSError {
        NSError(
            domain: "ActrJoin", code: 1,
            userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                message, comment: "Actr join error")])
    }

    private static func requireSuccess(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse,
              (200 ..< 300).contains(http.statusCode) else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? -1
            throw NSError(
                domain: "ActrJoin", code: 2,
                userInfo: [NSLocalizedDescriptionKey: String(
                    format: NSLocalizedString(
                        "The request failed (HTTP %ld).",
                        comment: "Actr join fetch failure"),
                    status)])
        }
    }

    private static func requireHTTPS(_ response: URLResponse) throws {
        guard response.url?.scheme?.lowercased() == "https" else {
            throw NSError(
                domain: "ActrJoin", code: 3,
                userInfo: [NSLocalizedDescriptionKey: NSLocalizedString(
                    "The Realm information was served over an insecure connection.",
                    comment: "Actr join insecure transport error")])
        }
    }
}
