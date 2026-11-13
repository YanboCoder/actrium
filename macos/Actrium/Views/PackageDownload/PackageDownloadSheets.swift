import SwiftUI

/// "Confirm before connect" sheet (04 §3): shows the domain and target of a
/// package direct link; the download starts only after the user confirms.
struct PackageDownloadConfirmSheet: View {
    let pending: PendingPackageDownload
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Download Actrium Package")
                    .font(.headline)
                Spacer()
                Button(action: onCancel) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding()

            Divider()

            VStack(alignment: .leading, spacing: 14) {
                // Domain — the trust root the user is asked to confirm
                VStack(alignment: .leading, spacing: 4) {
                    Text("Source Domain")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.host)
                        .font(.title3.weight(.semibold))
                        .textSelection(.enabled)
                }

                // Target — file name + full URL
                VStack(alignment: .leading, spacing: 4) {
                    Text("Target File")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.filename)
                        .font(.body.weight(.medium))
                    Text(pending.url.absoluteString)
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }

                Label {
                    Text("Confirm before connecting: the download starts after confirmation (size cap \(pending.sizeLimitText))")
                        .font(.caption)
                        .foregroundColor(.secondary)
                } icon: {
                    Image(systemName: "lock.shield")
                        .foregroundColor(.secondary)
                }
            }
            .padding()

            Divider()

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Download", action: onConfirm)
                    .keyboardShortcut(.defaultAction)
            }
            .padding()
        }
        .frame(width: 460)
    }
}

/// Progress sheet: downloading → SHA-256 verifying, or failed with retry.
struct ActivePackageDownloadSheet: View {
    let active: ActivePackageDownload
    let onRetry: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Downloading: \(active.pending.filename)")
                    .font(.headline)
                Spacer()
                if active.phase != .failed {
                    Button(action: onCancel) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.title2)
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding()

            Divider()

            VStack(alignment: .leading, spacing: 14) {
                switch active.phase {
                case .downloading:
                    HStack(spacing: 10) {
                        ProgressView()
                            .controlSize(.small)
                        Text("Downloading from \(active.pending.host)…")
                            .foregroundColor(.secondary)
                    }
                    Text(active.pending.url.absoluteString)
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                case .verifying:
                    HStack(spacing: 10) {
                        ProgressView()
                            .controlSize(.small)
                        Text("Computing SHA-256 checksum…")
                            .foregroundColor(.secondary)
                    }
                case .failed:
                    Label {
                        Text(active.errorMessage ?? "Download failed")
                            .foregroundColor(.primary)
                    } icon: {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.orange)
                    }
                    Text("Check the network and address, then retry.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()

            Divider()

            HStack {
                Spacer()
                if active.phase == .failed {
                    Button("Cancel", action: onCancel)
                        .keyboardShortcut(.cancelAction)
                    Button("Retry", action: onRetry)
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Cancel Download", action: onCancel)
                        .keyboardShortcut(.cancelAction)
                }
            }
            .padding()
        }
        .frame(width: 460)
    }
}

/// Instance-creation confirm sheet (15-B B6): shows the package
/// identity, declared permissions and the instance address that will replace
/// the package URL; confirm → serviceCreate → URI replace.
struct PackageInstanceConfirmSheet: View {
    let pending: PendingInstanceCreation
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Create App Instance")
                    .font(.headline)
                Spacer()
                Button(action: onCancel) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding()

            Divider()

            VStack(alignment: .leading, spacing: 14) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Package")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.packageTitle)
                        .font(.title3.weight(.semibold))
                    Text("sha256 \(pending.digestShort)…")
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                        .textSelection(.enabled)
                }

                // Declared permissions shown on the creation confirm sheet (03 §5)
                VStack(alignment: .leading, spacing: 6) {
                    Text("Declared Permissions")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    if pending.permissions.isEmpty {
                        Text("No permissions declared")
                            .font(.body)
                            .foregroundColor(.secondary)
                    } else {
                        ForEach(pending.permissions, id: \.self) { permission in
                            Label {
                                Text(permission)
                                    .font(.callout.monospaced())
                            } icon: {
                                Image(systemName: "checkmark.shield")
                                    .foregroundColor(.accentColor)
                            }
                        }
                    }
                }

                if let instanceURL = pending.proposedInstanceURL {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("After creation the address is replaced by the instance address")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(instanceURL.absoluteString)
                            .font(.caption.monospaced())
                            .lineLimit(2)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding()

            Divider()

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Create Instance", action: onConfirm)
                    .keyboardShortcut(.defaultAction)
            }
            .padding()
        }
        .frame(width: 460)
    }
}

/// Notice shown for formal `actr://` addresses in the prototype period:
/// Realm Profile resolution is pending (04 §3 open flow), so the address
/// cannot be opened yet; the notice says what works instead.
struct ActrUriNoticeSheet: View {
    let notice: ActrUriNotice
    let onDismiss: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Actrium Instance Address")
                    .font(.headline)
                Spacer()
                Button(action: onDismiss) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding()

            Divider()

            VStack(alignment: .leading, spacing: 12) {
                Label {
                    Text(notice.uri)
                        .font(.body.weight(.medium))
                        .textSelection(.enabled)
                } icon: {
                    Image(systemName: "link.badge.plus")
                        .foregroundColor(.accentColor)
                }

                if let detail = notice.detail {
                    Text(detail)
                } else {
                    Text("Formal instance addresses (domain \(notice.domain.isEmpty ? "unknown" : notice.domain)) require Realm Profile resolution, which the prototype does not support yet.")
                    Text("During the prototype, use a package direct link (an https URL ending in .actr) or a local .actr file path.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()

            Divider()

            HStack {
                Spacer()
                Button("Got It", action: onDismiss)
                    .keyboardShortcut(.defaultAction)
            }
            .padding()
        }
        .frame(width: 460)
    }
}

/// Invite-code redeem sheet (temporary flow): an actr:// link entered in the
/// address bar first presents this sheet for the invite code; "Redeem" hands
/// over to the Rust-provided method for the follow-up flow.
struct InviteCodeRedeemSheet: View {
    let pending: PendingInviteRedeem
    let onConfirm: (String) -> Void
    let onCancel: () -> Void

    @State private var inviteCode: String = ""

    private var trimmedCode: String {
        inviteCode.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Invite Code Redemption")
                    .font(.headline)
                Spacer()
                Button(action: onCancel) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding()

            Divider()

            VStack(alignment: .leading, spacing: 14) {
                // Domain — the trust root shown for confirmation (04 §3)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Source Domain")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.domain.isEmpty ? "unknown" : pending.domain)
                        .font(.title3.weight(.semibold))
                        .textSelection(.enabled)
                }

                // Target — the actr:// link being redeemed
                VStack(alignment: .leading, spacing: 4) {
                    Text("Target Address")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.uri)
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }

                // Invite code input
                VStack(alignment: .leading, spacing: 6) {
                    Text("Invite Code")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    TextField("Enter invite code", text: $inviteCode)
                        .textFieldStyle(.roundedBorder)
                        .font(.body.monospaced())
                        .autocorrectionDisabled()
                        .onSubmit { confirm() }
                }

                Label {
                    Text("Opening this address requires an invite code. After you choose Redeem, Actrium verifies the invite and continues opening.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                } icon: {
                    Image(systemName: "ticket")
                        .foregroundColor(.secondary)
                }
            }
            .padding()

            Divider()

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Redeem", action: confirm)
                    .keyboardShortcut(.defaultAction)
                    .disabled(trimmedCode.isEmpty)
            }
            .padding()
        }
        .frame(width: 460)
    }

    private func confirm() {
        guard !trimmedCode.isEmpty else { return }
        onConfirm(trimmedCode)
    }
}
