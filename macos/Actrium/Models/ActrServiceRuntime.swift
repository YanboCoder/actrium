import Foundation
import Network
import Actr

// MARK: - Backend seam

/// The live Actor behind one Service instance.
///
/// Production uses `ActrRefServiceRuntimeBackend`; tests inject fakes so the
/// create/recover/transport contracts can be exercised without Loran. The
/// backend is per-instance, so callers never pass a tab or actor id.
protocol ActrServiceRuntimeBackend: AnyObject {
    var actorId: String { get }
    func call(route: String, payload: Data, timeoutMs: Int64) async throws -> Data
    func tell(route: String, payload: Data) async throws
    func stop() async
}

/// Wraps a started `ActrRef` — the real package-backed runtime.
final class ActrRefServiceRuntimeBackend: ActrServiceRuntimeBackend, @unchecked Sendable {
    private let ref: ActrRef

    init(ref: ActrRef) {
        self.ref = ref
    }

    var actorId: String {
        let id = ref.actorId()
        return "\(id.realm)#\(id.serialNumber)"
    }

    func call(route: String, payload: Data, timeoutMs: Int64) async throws -> Data {
        try await ref.call(
            routeKey: route,
            payloadType: .rpcReliable,
            requestPayload: payload,
            timeoutMs: timeoutMs
        )
    }

    func tell(route: String, payload: Data) async throws {
        try await ref.tell(
            routeKey: route,
            payloadType: .rpcReliable,
            messagePayload: payload
        )
    }

    func stop() async {
        await ref.stop()
    }
}

// MARK: - Per-instance loopback endpoint

/// Minimal HTTP/1.1 server bound to 127.0.0.1 that exposes one instance's
/// `/v1/call`, `/v1/tell`, and `/health`. The Renderer reaches the actor only
/// through this endpoint, so it never needs the Actr runtime or credentials.
final class LoopbackServiceServer: @unchecked Sendable {
    typealias CallHandler = (String, Data, Int64) async throws -> Data
    typealias TellHandler = (String, Data) async throws -> Void

    private let listener: NWListener
    private let queue = DispatchQueue(label: "actrium.actr.loopback")
    private let callHandler: CallHandler
    private let tellHandler: TellHandler

    private static let maxRequestBytes = 16 * 1024 * 1024

    init(call: @escaping CallHandler, tell: @escaping TellHandler) throws {
        let parameters = NWParameters.tcp
        parameters.allowLocalEndpointReuse = true
        parameters.requiredLocalEndpoint = NWEndpoint.hostPort(
            host: .ipv4(.loopback),
            port: .any
        )
        self.listener = try NWListener(using: parameters)
        self.callHandler = call
        self.tellHandler = tell
    }

    /// Starts listening and returns the `/v1/call` URL.
    func start() async throws -> String {
        let port: UInt16 = try await withCheckedThrowingContinuation { continuation in
            var resumed = false
            let resume: (Result<UInt16, Error>) -> Void = { result in
                guard !resumed else { return }
                resumed = true
                continuation.resume(with: result)
            }

            listener.stateUpdateHandler = { [weak self] state in
                switch state {
                case .ready:
                    if let port = self?.listener.port?.rawValue {
                        resume(.success(port))
                    } else {
                        resume(.failure(ActrServiceInstanceError.internalFailure(
                            "loopback listener has no port"
                        )))
                    }
                case .failed(let error):
                    resume(.failure(error))
                default:
                    break
                }
            }
            listener.newConnectionHandler = { [weak self] connection in
                self?.accept(connection)
            }
            listener.start(queue: queue)
        }
        return "http://127.0.0.1:\(port)/v1/call"
    }

    func stop() {
        listener.cancel()
    }

    // MARK: Connections

    private func accept(_ connection: NWConnection) {
        connection.start(queue: queue)
        receive(on: connection, buffer: Data())
    }

    private func receive(on connection: NWConnection, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) {
            [weak self] data, _, isComplete, error in
            guard let self else {
                connection.cancel()
                return
            }
            var buffer = buffer
            if let data {
                buffer.append(data)
            }
            if buffer.count > Self.maxRequestBytes {
                connection.cancel()
                return
            }
            if let request = HTTPRequest.parse(buffer) {
                self.respond(to: request, on: connection)
                return
            }
            if error != nil || isComplete {
                connection.cancel()
                return
            }
            self.receive(on: connection, buffer: buffer)
        }
    }

    private func respond(to request: HTTPRequest, on connection: NWConnection) {
        Task {
            let (status, payload) = await route(request)
            let body = (try? JSONSerialization.data(withJSONObject: payload)) ?? Data()
            let header = """
            HTTP/1.1 \(status) \(status == 200 ? "OK" : "Error")\r
            Content-Type: application/json\r
            Content-Length: \(body.count)\r
            Connection: close\r
            \r

            """
            var response = Data(header.utf8)
            response.append(body)
            connection.send(content: response, completion: .contentProcessed { _ in
                connection.cancel()
            })
        }
    }

    private func route(_ request: HTTPRequest) async -> (Int, [String: Any]) {
        switch (request.method, request.path) {
        case ("GET", "/health"):
            return (200, ["status": "ok"])

        case ("POST", "/v1/call"):
            guard let body = request.jsonBody,
                  let routeKey = body["routeKey"] as? String,
                  let payloadBase64 = body["payloadBase64"] as? String,
                  let payload = Data(base64Encoded: payloadBase64)
            else {
                return (400, ["success": false, "error": "INVALID_REQUEST"])
            }
            let timeoutMs = Self.timeoutMs(from: body)
            do {
                let response = try await callHandler(routeKey, payload, timeoutMs)
                return (200, [
                    "success": true,
                    "payloadBase64": response.base64EncodedString(),
                ])
            } catch {
                return (200, ["success": false, "error": error.localizedDescription])
            }

        case ("POST", "/v1/tell"):
            guard let body = request.jsonBody,
                  let routeKey = body["routeKey"] as? String,
                  let payloadBase64 = body["payloadBase64"] as? String,
                  let payload = Data(base64Encoded: payloadBase64)
            else {
                return (400, ["success": false, "error": "INVALID_REQUEST"])
            }
            do {
                try await tellHandler(routeKey, payload)
                return (200, ["success": true])
            } catch {
                return (200, ["success": false, "error": error.localizedDescription])
            }

        default:
            return (404, ["success": false, "error": "SERVICE_NOT_FOUND"])
        }
    }

    private static func timeoutMs(from body: [String: Any]) -> Int64 {
        let raw = (body["timeoutMs"] as? NSNumber)?.int64Value ?? 30_000
        return min(max(raw, 1), 120_000)
    }
}

private struct HTTPRequest {
    let method: String
    let path: String
    let body: Data

    var jsonBody: [String: Any]? {
        guard !body.isEmpty else { return nil }
        return try? JSONSerialization.jsonObject(with: body) as? [String: Any]
    }

    /// Returns nil until the full header + declared body have arrived.
    static func parse(_ data: Data) -> HTTPRequest? {
        let headerDelimiter = Data("\r\n\r\n".utf8)
        guard let headerRange = data.range(of: headerDelimiter) else { return nil }
        guard let headerText = String(
            data: data[..<headerRange.lowerBound],
            encoding: .utf8
        ) else { return nil }

        var lines = headerText.components(separatedBy: "\r\n")
        guard let requestLine = lines.first else { return nil }
        let parts = requestLine.split(separator: " ")
        guard parts.count >= 2 else { return nil }
        let method = String(parts[0])
        let path = parts[1].split(separator: "?").first.map(String.init) ?? String(parts[1])

        var contentLength = 0
        for line in lines.dropFirst() {
            let pair = line.split(separator: ":", maxSplits: 1)
            guard pair.count == 2,
                  pair[0].lowercased() == "content-length"
            else { continue }
            contentLength = Int(pair[1].trimmingCharacters(in: .whitespaces)) ?? 0
        }

        let bodyStart = headerRange.upperBound
        let available = data.count - bodyStart
        guard available >= contentLength else { return nil }
        let body = data.subdata(in: bodyStart..<(bodyStart + contentLength))
        return HTTPRequest(method: method, path: path, body: body)
    }
}

// MARK: - Runtime

/// Package-backed Service runtime (15-B B9).
///
/// Starts a real `ActrNode` per instance, keeps the `ActrRef` alive, and
/// exposes each instance through its own loopback endpoint. There is no mock
/// fallback: a failed start or a missing cache entry surfaces as a stable
/// error so the Host can fail closed.
final class ActrServiceRuntime: ActrServiceInstanceProviding, @unchecked Sendable {
    struct InstanceRecord: Codable {
        var actorId: String
        var packageDigest: String
        var packagePath: String
        var displayName: String?
        var createdAtUnix: Double
    }

    private struct Store: Codable {
        var instances: [InstanceRecord] = []
        var pendingCreates: [String: String] = [:]
    }

    private struct LiveInstance {
        let backend: ActrServiceRuntimeBackend
        let server: LoopbackServiceServer
        let actorId: String
        let serviceCallUrl: String
    }

    private let storeURL: URL
    private let lock = NSLock()
    private var store: Store
    private var live: [String: LiveInstance] = [:]
    private let backendFactory: (String) async throws -> ActrServiceRuntimeBackend

    init(
        storeURL: URL? = nil,
        backendFactory: ((String) async throws -> ActrServiceRuntimeBackend)? = nil
    ) {
        if let storeURL {
            self.storeURL = storeURL
        } else {
            let support = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first
            self.storeURL = (support ?? FileManager.default.temporaryDirectory)
                .appendingPathComponent("Actrium", isDirectory: true)
                .appendingPathComponent("service-instances.json", isDirectory: false)
        }
        self.backendFactory = backendFactory ?? { packagePath in
            try await ActrServiceRuntime.makeActrBackend(packagePath: packagePath)
        }

        if let data = try? Data(contentsOf: self.storeURL),
           let decoded = try? JSONDecoder().decode(Store.self, from: data) {
            store = decoded
        } else {
            store = Store()
        }
    }

    // MARK: ActrServiceInstanceProviding

    func serviceCreate(
        packagePath: String,
        packageDigest: String,
        displayName: String?,
        idempotencyKey: String
    ) async throws -> ActrServiceHandle {
        // Idempotent retries return the previously committed instance and do
        // not re-validate the new request's digest.
        if let record = record(forCreateKey: idempotencyKey) {
            let instance = try await ensureLive(record: record, actorIdHint: record.actorId)
            return makeHandle(record, serviceCallUrl: instance.serviceCallUrl)
        }

        try verifyDigest(expected: packageDigest, at: packagePath)

        let started = try await startInstance(
            packagePath: packagePath,
            actorIdHint: nil
        )
        let record = InstanceRecord(
            actorId: started.actorId,
            packageDigest: packageDigest,
            packagePath: packagePath,
            displayName: displayName,
            createdAtUnix: Date().timeIntervalSince1970
        )

        // Commit. A concurrent create that registered the same ActorId wins;
        // the duplicate runtime is stopped so it holds no loopback port.
        lock.lock()
        if let existing = live[started.actorId],
           existing.serviceCallUrl != started.serviceCallUrl {
            if store.pendingCreates[idempotencyKey] == nil {
                store.pendingCreates[idempotencyKey] = started.actorId
                saveLocked()
            }
            let committed = store.instances.first(where: { $0.actorId == started.actorId }) ?? record
            lock.unlock()
            await started.backend.stop()
            started.server.stop()
            return makeHandle(committed, serviceCallUrl: existing.serviceCallUrl)
        }
        live[started.actorId] = started
        if !store.instances.contains(where: { $0.actorId == started.actorId }) {
            store.instances.append(record)
        }
        store.pendingCreates[idempotencyKey] = started.actorId
        let committed = store.instances.first(where: { $0.actorId == started.actorId }) ?? record
        saveLocked()
        lock.unlock()

        return makeHandle(committed, serviceCallUrl: started.serviceCallUrl)
    }

    func serviceRecover(
        actorId: String,
        expectedPackageDigest: String
    ) async throws -> ActrServiceHandle {
        guard let record = record(for: actorId) else {
            throw ActrServiceInstanceError.instanceNotFound(actorId: actorId)
        }
        guard record.packageDigest == expectedPackageDigest else {
            throw ActrServiceInstanceError.digestMismatch(
                expected: expectedPackageDigest,
                actual: record.packageDigest
            )
        }
        let instance = try await ensureLive(record: record, actorIdHint: actorId)
        return makeHandle(record, serviceCallUrl: instance.serviceCallUrl)
    }

    func serviceCall(
        actorId: String,
        route: String,
        payload: Data,
        timeoutMs: Int64
    ) async throws -> Data {
        guard let instance = liveInstance(for: actorId) else {
            throw ActrServiceInstanceError.instanceNotFound(actorId: actorId)
        }
        return try await instance.backend.call(
            route: route,
            payload: payload,
            timeoutMs: timeoutMs
        )
    }

    func serviceTell(actorId: String, route: String, payload: Data) async throws {
        guard let instance = liveInstance(for: actorId) else {
            throw ActrServiceInstanceError.instanceNotFound(actorId: actorId)
        }
        try await instance.backend.tell(route: route, payload: payload)
    }

    func resolveActorId(byAddressLabel label: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return store.instances
            .first(where: { $0.actorId.suffix(12) == label })?
            .actorId
    }

    func knownDigest(actorId: String) -> String? {
        record(for: actorId)?.packageDigest
    }

    // MARK: Runtime lifecycle

    private func ensureLive(
        record: InstanceRecord,
        actorIdHint: String
    ) async throws -> LiveInstance {
        if let existing = liveInstance(for: actorIdHint) {
            return existing
        }
        guard FileManager.default.fileExists(atPath: record.packagePath) else {
            throw ActrServiceInstanceError.packageCacheMiss(digest: record.packageDigest)
        }

        let started = try await startInstance(
            packagePath: record.packagePath,
            actorIdHint: actorIdHint
        )

        lock.lock()
        if let existing = live[actorIdHint] {
            lock.unlock()
            await started.backend.stop()
            started.server.stop()
            return existing
        }
        live[actorIdHint] = started
        lock.unlock()
        return started
    }

    private func startInstance(
        packagePath: String,
        actorIdHint: String?
    ) async throws -> LiveInstance {
        let backend = try await backendFactory(packagePath)

        let server = try LoopbackServiceServer(
            call: { route, payload, timeoutMs in
                try await backend.call(
                    route: route,
                    payload: payload,
                    timeoutMs: timeoutMs
                )
            },
            tell: { route, payload in
                try await backend.tell(route: route, payload: payload)
            }
        )
        let serviceCallUrl = try await server.start()

        return LiveInstance(
            backend: backend,
            server: server,
            actorId: actorIdHint ?? backend.actorId,
            serviceCallUrl: serviceCallUrl
        )
    }

    private static func makeActrBackend(
        packagePath: String
    ) async throws -> ActrServiceRuntimeBackend {
        let configURL: URL
        do {
            configURL = try ActrRuntimeConfiguration.configFileURL()
        } catch {
            throw ActrServiceInstanceError.internalFailure(
                "cannot write runtime config: \(error.localizedDescription)"
            )
        }
        do {
            let node = try await ActrNode.from(
                packageConfig: configURL,
                packageURL: URL(fileURLWithPath: packagePath)
            )
            let ref = try await node.start()
            return ActrRefServiceRuntimeBackend(ref: ref)
        } catch {
            throw ActrServiceInstanceError.internalFailure(
                "failed to start actor: \(error.localizedDescription)"
            )
        }
    }

    // MARK: Digest verification

    private func verifyDigest(expected: String, at packagePath: String) throws {
        guard FileManager.default.fileExists(atPath: packagePath) else {
            throw ActrServiceInstanceError.packageCacheMiss(digest: expected)
        }
        let actual = try ActrPackageDownloadService.sha256FileSync(
            at: URL(fileURLWithPath: packagePath)
        )
        guard actual.caseInsensitiveCompare(expected) == .orderedSame else {
            throw ActrServiceInstanceError.digestMismatch(
                expected: expected,
                actual: actual
            )
        }
    }

    // MARK: Store access (lock-protected)

    private func record(forCreateKey key: String) -> InstanceRecord? {
        lock.lock()
        defer { lock.unlock() }
        guard let actorId = store.pendingCreates[key] else { return nil }
        return store.instances.first(where: { $0.actorId == actorId })
    }

    private func record(for actorId: String, lockHeld: Bool = false) -> InstanceRecord? {
        if !lockHeld { lock.lock() }
        defer { if !lockHeld { lock.unlock() } }
        return store.instances.first(where: { $0.actorId == actorId })
    }

    private func liveInstance(for actorId: String) -> LiveInstance? {
        lock.lock()
        defer { lock.unlock() }
        return live[actorId]
    }

    private func makeHandle(
        _ record: InstanceRecord,
        serviceCallUrl: String?
    ) -> ActrServiceHandle {
        ActrServiceHandle(
            actorId: record.actorId,
            packageDigest: record.packageDigest,
            displayName: record.displayName,
            serviceCallUrl: serviceCallUrl
        )
    }

    private func saveLocked() {
        try? FileManager.default.createDirectory(
            at: storeURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        if let data = try? JSONEncoder().encode(store) {
            try? data.write(to: storeURL, options: .atomic)
        }
    }
}
