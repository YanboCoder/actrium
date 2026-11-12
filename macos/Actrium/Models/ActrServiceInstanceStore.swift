import Foundation
import Actr

/// Link-time proof that the ActrBindings runtime chain (actr-ffi → ActrBindings)
/// is wired into the macOS app target alongside ActriumCore (15-B B5, two FFI chains).
///
/// Resolved 2026-11-11: the two chains now coexist by shipping libactr as a
/// **dynamic** library (ActrFFI.framework, cdylib) while ActriumCore stays
/// static — two rustc staticlibs still cannot share one Mach-O, but a dylib
/// has no static symbol collision with it.
@MainActor
enum ActrBindingsAvailability {
    static var actrNodeType: ActrNode.Type { ActrNode.self }
}

// MARK: - Installation identity (local realm address prefix, 04 §2)

/// Shell-owned local installation identity (26-char base32, a-z 2-7 —
/// validateInstallationId format). Generated once, persisted in App Support;
/// prefix of the local-realm instance addresses (actr://<installation>.actr.alt/…).
enum ActrInstallationIdentity {
    static var installationId: String {
        if let cached = loadCached() {
            return cached
        }
        let generated = generate()
        persist(generated)
        return generated
    }

    private static var identityFile: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        return (support ?? FileManager.default.temporaryDirectory)
            .appendingPathComponent("Actrium", isDirectory: true)
            .appendingPathComponent("installation-id", isDirectory: false)
    }

    private static let alphabet = Array("abcdefghijklmnopqrstuvwxyz234567")

    private static func generate() -> String {
        String((0..<26).map { _ in alphabet.randomElement()! })
    }

    private static func loadCached() -> String? {
        guard let raw = try? String(contentsOf: identityFile, encoding: .utf8),
              isValid(raw)
        else { return nil }
        return raw
    }

    /// Format check (26 chars, base32 a-z/2-7) mirroring validateInstallationId.
    static func isValid(_ id: String) -> Bool {
        id.count == 26 && id.allSatisfy(alphabet.contains)
    }

    private static func persist(_ id: String) {
        try? FileManager.default.createDirectory(
            at: identityFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? id.data(using: .utf8)?.write(to: identityFile, options: .atomic)
    }
}

// MARK: - Instance address (04 §2 local realm form)

/// Local realm instance address form: `actr://<installation-id>.actr.alt/<realm-id>`.
enum ActrInstanceAddress {
    static func instanceURL(actorId: String) -> URL? {
        let install = ActrInstallationIdentity.installationId
        // Short, shareable label derived from the persistent ActorId.
        let short = String(actorId.suffix(12))
        return URL(string: "actr://\(install).actr.alt/\(short)")
    }

    static func isLocalInstanceURL(_ url: URL) -> Bool {
        url.scheme?.lowercased() == "actr" && url.host?.lowercased().hasSuffix(".actr.alt") == true
    }
}

// MARK: - Instance creation confirm payload (15-B B6)

/// Package import awaiting the user's instance-creation confirmation.
/// Drives the confirm sheet; on confirm → serviceCreate → URI replace.
struct PendingInstanceCreation: Identifiable {
    let id = UUID()
    let tabId: UUID
    let packageTitle: String
    let packagePath: String
    let packageDigest: String
    let displayName: String?
    let permissions: [String]

    var digestShort: String { String(packageDigest.prefix(12)) }
    var proposedInstanceURL: URL? { ActrInstanceAddress.instanceURL(actorId: "") }
}

/// Result of serviceCreate / serviceRecover (subset of the IF-1 ServiceHandle shape).
struct ActrServiceHandle {
    /// Persistent instance identity; survives restarts (B7 recover matches it).
    let actorId: String
    let packageDigest: String
    let displayName: String?
    /// Loopback `/v1/call` endpoint the Renderer forwards `host:service` calls
    /// to. Nil until the real runtime has started the instance.
    let serviceCallUrl: String?
}

/// Errors aligned with the stable codes of 04 §7 / 12 §10.
enum ActrServiceInstanceError: LocalizedError {
    case idempotencyConflict
    case instanceNotFound(actorId: String)
    case digestMismatch(expected: String, actual: String)
    case packageCacheMiss(digest: String)
    case internalFailure(String)

    var errorDescription: String? {
        switch self {
        case .idempotencyConflict:
            return "Create request conflict (IDEMPOTENCY_CONFLICT)"
        case .instanceNotFound(let actorId):
            return "Instance not found (SERVICE_NOT_FOUND): \(actorId)"
        case .digestMismatch(let expected, let actual):
            return "Package digest mismatch (PACKAGE_DIGEST_MISMATCH): expected \(expected.prefix(12))…, actual \(actual.prefix(12))…"
        case .packageCacheMiss(let digest):
            return "Package cache miss for digest \(digest.prefix(12))… (SERVICE_NOT_FOUND)"
        case .internalFailure(let message):
            return "Service runtime failure (INTERNAL): \(message)"
        }
    }
}

/// Host-side service-instance operations consumed by the shell (15-B B6/B7).
/// Signature mirrors A's actr-ffi surface (plan 15 IF-1) so switching to
/// the real FFI backend is a one-line change for the shell.
protocol ActrServiceInstanceProviding: Sendable {
    /// Creates a local Realm instance for a cached, verified package.
    /// Same idempotency key retried within a create returns the same instance.
    func serviceCreate(
        packagePath: String,
        packageDigest: String,
        displayName: String?,
        idempotencyKey: String
    ) async throws -> ActrServiceHandle

    /// Rebinds a persisted ActorId after restart; digest must match the
    /// package the instance was created from.
    func serviceRecover(actorId: String, expectedPackageDigest: String) async throws -> ActrServiceHandle

    /// Resolves the persistent ActorId behind an instance address label
    /// (actr://<installation>.actr.alt/<label>) — restart restore (15-B B7).
    func resolveActorId(byAddressLabel label: String) -> String?

    /// Package digest the instance was created from (recover's expected digest).
    func knownDigest(actorId: String) -> String?

    /// Calls one route on a live instance. Used by host-side callers that hold
    /// the ActorId directly; the Renderer reaches the same actor over the
    /// instance's loopback endpoint.
    func serviceCall(
        actorId: String,
        route: String,
        payload: Data,
        timeoutMs: Int64
    ) async throws -> Data

    /// Fire-and-forget route delivery on a live instance.
    func serviceTell(actorId: String, route: String, payload: Data) async throws
}

/// Prototype mock (15 §5: the mock ActorId skeleton is the mainline; the
/// interface matches the real ID exactly).
/// Local persistent ActorIds with create/recover semantics; A's actr-ffi
/// skeleton replaces this class when delivered (shell code unchanged).
final class MockActrServiceInstanceStore: ActrServiceInstanceProviding {
    struct InstanceRecord: Codable {
        var actorId: String
        var packageDigest: String
        var packagePath: String
        var displayName: String?
        var createdAtUnix: Double
    }

    struct Store: Codable {
        var instances: [InstanceRecord] = []
        var pendingCreates: [String: String] = [:]  // idempotencyKey → actorId
    }

    private let storeURL: URL
    private let queue = DispatchQueue(label: "actrium.actr.service-instances")
    private var store: Store

    init(storeURL: URL? = nil) {
        if let storeURL {
            self.storeURL = storeURL
        } else {
            let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            self.storeURL = (support ?? FileManager.default.temporaryDirectory)
                .appendingPathComponent("Actrium", isDirectory: true)
                .appendingPathComponent("service-instances.json", isDirectory: false)
        }
        if let data = try? Data(contentsOf: self.storeURL),
           let decoded = try? JSONDecoder().decode(Store.self, from: data) {
            store = decoded
        } else {
            store = Store()
        }
    }

    func serviceCreate(
        packagePath: String,
        packageDigest: String,
        displayName: String?,
        idempotencyKey: String
    ) async throws -> ActrServiceHandle {
        try queue.sync {
            // Idempotent retry returns the same instance (15 §5).
            if let actorId = store.pendingCreates[idempotencyKey],
               let record = store.instances.first(where: { $0.actorId == actorId }) {
                return ActrServiceHandle(
                    actorId: record.actorId,
                    packageDigest: record.packageDigest,
                    displayName: record.displayName,
                    serviceCallUrl: nil
                )
            }

            let actorId = "actr-local-\(UUID().uuidString.lowercased())"
            let record = InstanceRecord(
                actorId: actorId,
                packageDigest: packageDigest,
                packagePath: packagePath,
                displayName: displayName,
                createdAtUnix: Date().timeIntervalSince1970
            )
            store.instances.append(record)
            store.pendingCreates[idempotencyKey] = actorId
            save()
            return ActrServiceHandle(
                actorId: actorId,
                packageDigest: packageDigest,
                displayName: displayName,
                serviceCallUrl: nil
            )
        }
    }

    func serviceRecover(actorId: String, expectedPackageDigest: String) async throws -> ActrServiceHandle {
        try queue.sync {
            guard let record = store.instances.first(where: { $0.actorId == actorId }) else {
                throw ActrServiceInstanceError.instanceNotFound(actorId: actorId)
            }
            guard record.packageDigest == expectedPackageDigest else {
                throw ActrServiceInstanceError.digestMismatch(
                    expected: expectedPackageDigest,
                    actual: record.packageDigest
                )
            }
            return ActrServiceHandle(
                actorId: record.actorId,
                packageDigest: record.packageDigest,
                displayName: record.displayName,
                serviceCallUrl: nil
            )
        }
    }

    func serviceCall(
        actorId: String,
        route: String,
        payload: Data,
        timeoutMs: Int64
    ) async throws -> Data {
        throw ActrServiceInstanceError.internalFailure(
            "mock instance store has no live Actr runtime for \(actorId)"
        )
    }

    func serviceTell(actorId: String, route: String, payload: Data) async throws {
        throw ActrServiceInstanceError.internalFailure(
            "mock instance store has no live Actr runtime for \(actorId)"
        )
    }

    func resolveActorId(byAddressLabel label: String) -> String? {
        queue.sync {
            // Instance addresses embed the ActorId tail as the label
            // (ActrInstanceAddress.instanceURL).
            store.instances.first(where: { $0.actorId.suffix(12) == label })?.actorId
        }
    }

    func knownDigest(actorId: String) -> String? {
        queue.sync {
            store.instances.first(where: { $0.actorId == actorId })?.packageDigest
        }
    }

    private func save() {
        try? FileManager.default.createDirectory(
            at: storeURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let data = try? JSONEncoder().encode(store) {
            try? data.write(to: storeURL, options: .atomic)
        }
    }
}
