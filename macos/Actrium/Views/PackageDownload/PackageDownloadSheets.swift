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
                Text("下载 Actrium 程序包")
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
                    Text("来源域名")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.host)
                        .font(.title3.weight(.semibold))
                        .textSelection(.enabled)
                }

                // Target — file name + full URL
                VStack(alignment: .leading, spacing: 4) {
                    Text("目标文件")
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
                    Text("连接前需确认：确认后将开始下载（大小上限 \(pending.sizeLimitText)）")
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
                Button("取消", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("下载", action: onConfirm)
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
                Text("正在下载：\(active.pending.filename)")
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
                        Text("正在从 \(active.pending.host) 下载…")
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
                        Text("正在计算 SHA-256 校验值…")
                            .foregroundColor(.secondary)
                    }
                case .failed:
                    Label {
                        Text(active.errorMessage ?? "下载失败")
                            .foregroundColor(.primary)
                    } icon: {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.orange)
                    }
                    Text("请检查网络与地址后重试。")
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
                    Button("取消", action: onCancel)
                        .keyboardShortcut(.cancelAction)
                    Button("重试", action: onRetry)
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("取消下载", action: onCancel)
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
                Text("创建应用实例")
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
                    Text("程序包")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(pending.packageTitle)
                        .font(.title3.weight(.semibold))
                    Text("sha256 \(pending.digestShort)…")
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                        .textSelection(.enabled)
                }

                // Declared permissions (创建确认页的权限声明展示, 03 §5)
                VStack(alignment: .leading, spacing: 6) {
                    Text("权限声明")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    if pending.permissions.isEmpty {
                        Text("未声明任何权限")
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
                        Text("创建后地址将替换为实例地址")
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
                Button("取消", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("创建实例", action: onConfirm)
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
                Text("Actrium 实例地址")
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
                    Text("正式实例地址（域名 \(notice.domain.isEmpty ? "未知" : notice.domain)）需要 Realm Profile 解析，当前原型暂未支持。")
                    Text("原型期可使用包直链（https 地址，以 .actr 结尾）或本地 .actr 文件路径。")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()

            Divider()

            HStack {
                Spacer()
                Button("知道了", action: onDismiss)
                    .keyboardShortcut(.defaultAction)
            }
            .padding()
        }
        .frame(width: 460)
    }
}
