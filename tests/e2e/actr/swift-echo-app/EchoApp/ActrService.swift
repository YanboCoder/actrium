import Actr
import Foundation
import SwiftProtobuf
import SwiftUI

// ACTR: mutable scaffold
// User-owned business scaffold. Keep EchoApp/Generated immutable and customize this file.

@MainActor
final class ActrService: ObservableObject {
    @Published var status = "Starting ACTR node..."
    @Published var errorMessage: String?
    @Published var membershipStatus = ""
    @Published var membershipBusy = false
    @Published var invitationForDevice = ""

    private var actrNode: ActrNode?
    private var actorRef: ActrRef?
    private var linkedWorkload: DynamicWorkload?
    private var hasAutoSent = false
    private var isStarting = false

    var isReady: Bool {
        actorRef != nil
    }

    func manageMembership(_ operation: String, invite: String, provider: String, target: String) async {
        guard !membershipBusy, !isStarting else { return }
        membershipBusy = true
        defer { membershipBusy = false }
        await stop()
        do {
            let manifest = try materializeRuntimeConfig()
            let configURL = manifest.deletingLastPathComponent().appendingPathComponent("actr.toml")
            let config = try readRealmRuntimeConfiguration(configPath: configURL.path)
            if operation == "reset" {
                try RealmIdentityStore().forgetForRejoin(realm: config.realm)
                invitationForDevice = ""
                membershipStatus = "Saved identity removed. Use a new invitation to rejoin."
                return
            }
            let member = try await RealmEnrollment.membership(config: configURL,
                invite: operation == "join" ? invite : nil, provider: provider.isEmpty ? nil : provider)
            switch operation {
            case "join":
                membershipStatus = "Joined. Client key: \(member.keyId())"
            case "revoke":
                _ = try await member.revokeClient(keyId: target.isEmpty ? member.keyId() : target)
                membershipStatus = "Client revoked"
            case "invite":
                let members = try JSONSerialization.jsonObject(with: Data(try await member.members().utf8)) as? [String: Any]
                let own = (members?["members"] as? [[String: Any]])?.first { row in
                    (row["clients"] as? [[String: Any]])?.contains { $0["key_id"] as? String == member.keyId() } == true
                }
                guard let memberId = own?["member_id"] as? String else { return }
                let result = try await member.inviteClient(memberId: memberId,
                    expiresAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(900)))
                let json = try JSONSerialization.jsonObject(with: Data(result.utf8)) as? [String: Any]
                invitationForDevice = json?["invite"] as? String ?? ""
                membershipStatus = "One-time invitation expires in 15 minutes."
            default:
                membershipStatus = try await member.members()
            }
        } catch RealmMembershipError.Rejected(_, let code) {
            membershipStatus = code
        } catch RealmMembershipError.Uncertain {
            membershipStatus = "Result unknown. Retry the same invitation; the saved key is unchanged."
        } catch {
            membershipStatus = "Membership unavailable. Check the trusted provider and invitation. No key was replaced."
        }
    }

    func startIfNeeded() async {
        guard actorRef == nil, !isStarting else { return }
        isStarting = true
        defer { isStarting = false }

        do {
            let manifestURL = try materializeRuntimeConfig()
            let membership = try await RealmEnrollment.membership(config: manifestURL.deletingLastPathComponent().appendingPathComponent("actr.toml"))
            let actorType = try resolveManifestPackageActrType(manifestPath: manifestURL.path)

            let handler = LocalEchoServiceHandlerImpl()
            let workload = Actr.dynamicWorkload(
                lifecycle: LocalEchoServiceLifecycleAdapter(handler: handler)
            )
            linkedWorkload = workload
            let node = try await ActrNode.linked(config: manifestURL.path, type: actorType, workload: workload, mobileLifecycleObservers: [], membership: membership)

            actorRef = try await node.start()
            actrNode = node
            status = "Ready: \(actorType.toStringRepr())"
        } catch {
            status = "ACTR startup failed: \(error)"
            errorMessage = String(describing: error)
        }
    }

    func stop() async {
        guard let actorRef else { return }
        await actorRef.stop()
        self.actorRef = nil
        actrNode = nil
    }

    func sendEcho(_ input: String) async throws -> String {
        guard let actorRef else {
            throw EchoAppError.actorUnavailable
        }

        var localRequest = Echoapp_LocalEchoRequest()
        localRequest.message = input
        let requestData = try EchoappLocalEchoServiceSendRpc.encodeRequest(localRequest)
        let responseData = try await actorRef.call(
            routeKey: EchoappLocalEchoServiceSendRpc.routeKey,
            payloadType: EchoappLocalEchoServiceSendRpc.payloadType,
            requestPayload: requestData,
            timeoutMs: 30_000
        )
        let response = try EchoappLocalEchoServiceSendRpc.decodeResponse(responseData)
        autoMarkSent()
        return response.reply
    }

    func shouldAutoSend() -> Bool {
        ProcessInfo.processInfo.environment["ACTR_ECHOAPP_AUTO_SEND"] == "1" && !hasAutoSent
    }

    private func autoMarkSent() {
        hasAutoSent = true
    }

    private func materializeRuntimeConfig() throws -> URL {
        guard let templateURL = Bundle.main.url(forResource: "actr", withExtension: "toml") else {
            throw EchoAppError.missingConfigTemplate
        }
        guard let bundledManifestURL = Bundle.main.url(forResource: "manifest", withExtension: "toml") else {
            throw EchoAppError.missingManifest
        }

        let fileManager = FileManager.default
        let supportURL = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let appURL = supportURL.appendingPathComponent("EchoApp", isDirectory: true)
        let dataURL = appURL.appendingPathComponent("hyper", isDirectory: true)
        try fileManager.createDirectory(at: dataURL, withIntermediateDirectories: true)

        var config = try String(contentsOf: templateURL, encoding: .utf8)
        config += """

        [hyper]
        data_dir = "\(dataURL.path)"

        [hyper.trust]
        kind = "dev_only"
        """

        let configURL = appURL.appendingPathComponent("actr.toml")
        try config.write(to: configURL, atomically: true, encoding: .utf8)
        let manifestURL = appURL.appendingPathComponent("manifest.toml")
        try Data(contentsOf: bundledManifestURL).write(to: manifestURL, options: .atomic)
        return manifestURL
    }

}

private enum EchoAppError: Error {
    case missingConfigTemplate
    case missingManifest
    case actorUnavailable
    case remoteActorUnavailable
}

/// Safe: no mutable state. All fields are value types or immutable references.
private final class LocalEchoServiceHandlerImpl: ActrDispatchHandler, @unchecked Sendable {
    private let remoteEchoRoute = "echo.EchoService.Echo"

    init() {}

    func send(
        req localRequest: Echoapp_LocalEchoRequest,
        ctx: any ActrContext
    ) async throws(ActrError) -> Echoapp_LocalEchoResponse {
        let forwardedMessage = "swift-local:\(localRequest.message)"

        var remoteRequest = Echo_EchoRequest()
        remoteRequest.message = forwardedMessage
        let targetType = try await ctx.resolveDependency(alias: "echo")
        let targetId = try await ctx.discover(targetType: targetType)
        let requestData: Data
        do {
            requestData = try remoteRequest.serializedData()
        } catch {
            throw ActrError.DecodeFailure(msg: "Failed to encode echo request: \(error)")
        }
        let remotePayload = try await ctx.call(
            target: targetId,
            routeKey: remoteEchoRoute,
            payload: requestData,
            timeoutMs: 30_000
        )

        let remoteResponse: Echo_EchoResponse
        do {
            remoteResponse = try Echo_EchoResponse(serializedBytes: remotePayload)
        } catch {
            throw ActrError.DecodeFailure(msg: "Failed to decode echo response: \(error)")
        }
        var localResponse = Echoapp_LocalEchoResponse()
        localResponse.reply = remoteResponse.reply
        localResponse.forwardedMessage = forwardedMessage
        return localResponse
    }
}

/// Safe: no mutable state. `handler` is an immutable reference.
private final class LocalEchoServiceLifecycleAdapter: Workload, @unchecked Sendable {
    private let handler: LocalEchoServiceHandlerImpl

    init(handler: LocalEchoServiceHandlerImpl) {
        self.handler = handler
    }

    func onStart(ctx: Context) async throws {}

    func onReady(ctx: Context) async throws {}

    func onStop(ctx: Context) async throws {}

    func onError(ctx: Context, event: ErrorEvent) async throws {
        print("LocalEchoServiceLifecycleAdapter error: \(event)")
    }

    func dispatch(ctx: Context, envelope: RpcEnvelope) async throws -> Data {
        return try await actrDispatch(
            ctx: ctx,
            handler: handler,
            routeKey: envelope.routeKey,
            payload: envelope.payload,
            timeoutMs: 30_000
        )
    }
}
