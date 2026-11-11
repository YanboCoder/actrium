import Foundation

// NOTE (15-B B5, link-time proof parked): the previous `ActrBindingsAvailability`
// enum referenced `ActrNode.self` to prove at link time that the ActrBindings
// runtime chain is wired into the app alongside ActriumCore. Since main's
// PR 417/418 refactor, linking BOTH Rust static libraries (libactr.a from
// dist/ActrFFI.xcframework and libactrium_core.a) into one binary trips 544
// duplicate Rust std/core symbols — two rustc-built staticlibs cannot coexist
// in one Mach-O until the FFI chains are merged or one becomes dynamic
// (IF-1 decision, 15 号计划 §FFI 链路红线). Restore the proof once that lands.

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
}

/// Errors aligned with the stable codes of 04 §7 / 12 §10.
enum ActrServiceInstanceError: LocalizedError {
    case idempotencyConflict
    case instanceNotFound(actorId: String)
    case digestMismatch(expected: String, actual: String)

    var errorDescription: String? {
        switch self {
        case .idempotencyConflict:
            return "创建请求冲突（IDEMPOTENCY_CONFLICT）"
        case .instanceNotFound(let actorId):
            return "实例不存在（SERVICE_NOT_FOUND）：\(actorId)"
        case .digestMismatch(let expected, let actual):
            return "包指纹不一致（PACKAGE_DIGEST_MISMATCH）：期望 \(expected.prefix(12))…，实际 \(actual.prefix(12))…"
        }
    }
}

/// Host-side service-instance operations consumed by the shell (15-B B6/B7).
/// Signature mirrors A's actr-ffi surface (15 号计划 IF-1) so switching to
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
}

/// Prototype mock (15 号 §5:模拟 ActorId 骨架为主线，接口与真 ID 完全一致)。
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
            // Idempotent retry returns the same instance (15 号计划 §5).
            if let actorId = store.pendingCreates[idempotencyKey],
               let record = store.instances.first(where: { $0.actorId == actorId }) {
                return ActrServiceHandle(
                    actorId: record.actorId,
                    packageDigest: record.packageDigest,
                    displayName: record.displayName
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
                displayName: displayName
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
                displayName: record.displayName
            )
        }
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
