import SwiftUI

struct ContentView: View {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var actrService = ActrService()
    @State private var input = ProcessInfo.processInfo.environment["ACTR_ECHOAPP_TEST_INPUT"] ?? "hello"
    @State private var output = ""
    @State private var isSending = false
    @State private var observedBackground = false
    @State private var foregroundCycle = 0
    @State private var showMembership = false
    @State private var providerId = ""
    @State private var realmInvite = ""
    @State private var revokeKeyId = ""
    @State private var confirmReset = false

    private let lifecycleE2EEnabled =
        ProcessInfo.processInfo.environment["ACTR_ECHOAPP_LIFECYCLE_E2E"] == "1"

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("EchoApp")
                .font(.largeTitle.bold())

            Text(actrService.status)
                .font(.footnote)
                .foregroundStyle(actrService.isReady ? Color.green : Color.secondary)

            Button("Realm membership") { showMembership = true }

            TextField("Message", text: $input)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()

            Button {
                Task {
                    await sendEcho()
                }
            } label: {
                if isSending {
                    ProgressView()
                } else {
                    Text("Send")
                        .frame(maxWidth: .infinity)
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(!actrService.isReady || input.isEmpty || isSending)

            Text("Reply")
                .font(.headline)
            Text(output.isEmpty ? "No reply yet" : output)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
                .background(.thinMaterial)
                .clipShape(RoundedRectangle(cornerRadius: 12))

            Spacer()
        }
        .padding(24)
        .sheet(isPresented: $showMembership, onDismiss: {
            realmInvite = ""
            actrService.invitationForDevice = ""
            Task { await actrService.startIfNeeded() }
        }) {
            Form {
                TextField("Trusted provider ID", text: $providerId)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("One-time invitation", text: $realmInvite)
                    .privacySensitive()
                Button("Join / retry") {
                    Task {
                        await actrService.manageMembership("join", invite: realmInvite, provider: providerId, target: "")
                        if actrService.membershipStatus.hasPrefix("Joined.") { realmInvite = "" }
                    }
                }
                Button("Restore / list devices") { membershipAction("list") }
                Button("Invite another device") { membershipAction("invite") }
                if !actrService.invitationForDevice.isEmpty {
                    Text(actrService.invitationForDevice).privacySensitive().textSelection(.enabled)
                }
                TextField("Client key ID (empty = this device)", text: $revokeKeyId)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                Button("Revoke client", role: .destructive) { membershipAction("revoke") }
                Button("New identity after revocation", role: .destructive) { confirmReset = true }
                Text(actrService.membershipStatus).font(.footnote)
                Button("Done") { showMembership = false }
            }
            .disabled(actrService.membershipBusy)
            .confirmationDialog("Discard this device's identity? A new invitation is required.", isPresented: $confirmReset) {
                Button("Reset identity", role: .destructive) { membershipAction("reset") }
            }
        }
        .task {
            await actrService.startIfNeeded()
            if actrService.shouldAutoSend() && !input.isEmpty {
                await sendEcho()
            }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background { realmInvite = ""; actrService.invitationForDevice = "" }
            guard lifecycleE2EEnabled else { return }

            switch phase {
            case .background:
                observedBackground = true
                emitE2EPhase("background")
            case .active where observedBackground:
                observedBackground = false
                foregroundCycle += 1
                let cycle = foregroundCycle
                emitE2EPhase("foreground-\(cycle)")
                Task {
                    await sendRecoveryEcho("\(input)-foreground-\(cycle)")
                }
            default:
                break
            }
        }
    }

    private func membershipAction(_ operation: String) {
        Task { await actrService.manageMembership(operation, invite: "", provider: providerId, target: revokeKeyId) }
    }

    private func sendEcho(_ message: String? = nil) async {
        isSending = true
        output = ""

        defer { isSending = false }
        do {
            output = try await actrService.sendEcho(message ?? input)
            emitE2EResult(output)
        } catch {
            output = "Echo failed: \(error)"
            emitE2EResult(output)
        }
    }

    private func emitE2EResult(_ result: String) {
        print("ACTR_E2E_RESULT:\(result)")
        FileHandle.standardError.write(Data("ACTR_E2E_RESULT:\(result)\n".utf8))
    }

    private func emitE2EPhase(_ phase: String) {
        print("ACTR_E2E_PHASE:\(phase)")
        FileHandle.standardError.write(Data("ACTR_E2E_PHASE:\(phase)\n".utf8))
    }

    private func sendRecoveryEcho(_ message: String) async {
        isSending = true
        output = ""

        defer { isSending = false }
        do {
            // Call owns route readiness and transport retry; the UI does not
            // replay the request from a connection Hook or fallback timer.
            output = try await actrService.sendEcho(message)
            emitE2EResult(output)
        } catch {
            output = "Echo failed: \(error)"
            emitE2EResult(output)
        }
    }
}
