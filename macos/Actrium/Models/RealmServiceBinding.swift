import Actr
import ActrBindings
import ActriumCore
import ActriumIPC
import Foundation

struct RealmServiceError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

@MainActor
final class RealmServiceBindings {
    static let shared = RealmServiceBindings()
    private var bindings: [UUID: RealmServiceBinding] = [:]

    func attach(_ binding: RealmServiceBinding, to tabId: UUID) {
        remove(tabId)
        bindings[tabId] = binding
    }

    func remove(_ tabId: UUID) {
        guard let binding = bindings.removeValue(forKey: tabId) else { return }
        Task { await binding.stop() }
    }

    func call(_ request: RealmServiceCall, tabId: UUID) async throws -> Data {
        guard let binding = bindings[tabId] else {
            throw RealmServiceError(message: "The tab has no Realm Service.")
        }
        return try await binding.call(request)
    }
}

actor RealmServiceBinding {
    private struct Provider: Decodable {
        struct Signaling: Decodable {
            let url: String
            let priority: Int
        }

        struct Ice: Decodable {
            let urls: [String]
        }

        let signaling: [Signaling]
        let ice: [Ice]
    }

    private struct ProfileDocument: Decodable {
        let provider: Provider
    }

    private struct Capabilities: Decodable {
        struct Service: Decodable { let rpc: [String]? }
        let hostModules: [String: Service]
    }

    private let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("actrium-front-" + UUID().uuidString, isDirectory: true)
    private let profile: RealmProfileInfo
    private let front: Data
    private let service: Data
    private let configuration: String
    private let serviceType: ActrBindings.ActrType
    private let routes: Set<String>
    private var startup: (id: UUID, task: Task<ActrRefWrapper, Error>)?
    private var stopped = false

    init(
        profile: RealmProfileInfo,
        verifiedProfileBody: Data,
        front: Data,
        service: Data,
        keys: [Data],
        capabilitiesJSON: String
    ) throws {
        let document = try JSONDecoder().decode(ProfileDocument.self, from: verifiedProfileBody)
        let provider = document.provider
        guard let signaling = provider.signaling.sorted(by: { $0.priority < $1.priority }).first,
              URL(string: signaling.url)?.scheme?.lowercased() == "wss",
              let slash = profile.serviceType.firstIndex(of: "/"),
              let at = profile.serviceType.lastIndex(of: "@"), slash < at else {
            throw RealmServiceError(message: "The verified Realm profile has invalid Service or signaling information.")
        }
        let capabilities = try JSONDecoder().decode(Capabilities.self, from: Data(capabilitiesJSON.utf8))
        self.routes = Set(capabilities.hostModules["service"]?.rpc ?? capabilities.hostModules["host:service"]?.rpc ?? [])
        self.serviceType = ActrBindings.ActrType(
            publisher: String(profile.serviceType[..<slash]),
            name: String(profile.serviceType[profile.serviceType.index(after: slash)..<at]),
            version: String(profile.serviceType[profile.serviceType.index(after: at)...])
        )
        self.profile = profile
        self.front = front
        self.service = service

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        func literal<T: Encodable>(_ value: T) throws -> String {
            try String(decoding: encoder.encode(value), as: UTF8.self)
        }
        let iceURLs = provider.ice.flatMap(\.urls)
        let anchors = try Set(keys).map { key in
            try "[[trust]]\nkind = \"static\"\npubkey_b64 = " + literal(key.base64EncodedString())
        }
        self.configuration = try """
        edition = 1
        [signaling]
        url = \(literal(signaling.url))
        [issuer_endpoint]
        url = \(literal(profile.issuerUrl))
        [deployment]
        realm = \(literal(profile.realmName))
        [hyper]
        data_dir = \(literal(directory.path))
        [discovery]
        visible = false
        [webrtc]
        stun_urls = \(literal(iceURLs.filter { $0.hasPrefix("stun:") || $0.hasPrefix("stuns:") }))
        turn_urls = \(literal(iceURLs.filter { $0.hasPrefix("turn:") || $0.hasPrefix("turns:") }))
        [[acl.rules]]
        permission = "allow"
        type = \(literal(profile.serviceType))
        realm = "self"
        \(anchors.joined(separator: "\n"))
        """
    }

    func call(_ request: RealmServiceCall) async throws -> Data {
        guard !stopped, routes.contains(request.route),
              (1...300_000).contains(request.timeoutMs), request.payload.count <= 1_048_576 else {
            throw RealmServiceError(message: "The Service call is outside this tab's declared capabilities.")
        }
        let reference = try await connect()
        guard !stopped else { throw CancellationError() }
        // A Realm tab targets the selected Service type. A restarted Service
        // may register with a different ActorId while the Front stays running.
        guard let target = try await reference.discover(targetType: serviceType, count: 1).first else {
            throw RealmServiceError(message: "The Realm's selected Service is not online.")
        }
        guard !stopped else { throw CancellationError() }
        return try await reference.callPeer(
            target: target,
            routeKey: request.route,
            requestPayload: request.payload,
            timeoutMs: Int64(request.timeoutMs)
        )
    }

    private func connect() async throws -> ActrRefWrapper {
        if let startup { return try await startup.task.value }
        let id = UUID()
        let task = Task {
            let membership = try RealmIdentityStore().membership(
                realm: profile.realmName, providerId: profile.providerId, issuer: profile.issuerUrl
            )
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
            let configURL = directory.appendingPathComponent("actr.toml")
            try configuration.write(to: configURL, atomically: true, encoding: .utf8)
            let node = try await ActrBindings.ActrNode.newFromFrontWithMembership(
                configPath: configURL.path, frontArchive: front, serviceArchive: service, membership: membership
            )
            let reference = try await node.start()
            do {
                try Task.checkCancellation()
                return reference
            } catch {
                reference.shutdown()
                await reference.waitForShutdown()
                throw error
            }
        }
        startup = (id, task)
        do {
            return try await task.value
        } catch {
            if startup?.id == id {
                try? FileManager.default.removeItem(at: directory)
                startup = nil
            }
            throw error
        }
    }

    func stop() async {
        stopped = true
        if let startup {
            startup.task.cancel()
            if let reference = try? await startup.task.value {
                reference.shutdown()
                await reference.waitForShutdown()
            }
            self.startup = nil
        }
        try? FileManager.default.removeItem(at: directory)
    }
}
