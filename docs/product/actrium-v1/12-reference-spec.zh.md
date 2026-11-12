# 12. 参考协议、样例与术语

> 本文件给出可讨论的 v1 wire shape。字段名和 endpoint path 在 P0 schema 冻结前可以调整；对象边界、身份与安全语义不得改变。

## 1. Realm 链接样例

语法见 [Actr URI](../../../actr/protocols/uri.zh.md)。

```text
actr://acme.com/k7m2q9xh                         ; 打开实例 k7m2q9xh@acme.com
actr://acme.com/notes/today?filter=assigned      ; 显式可读 RealmId，带页面路径与 query
actr://acme.com/2af01.k7m2q9xh                   ; 实例里的某个 Actor（ActorId 2af01.k7m2q9xh@acme.com）
actr://<installation-id>.actr.alt/h3x8p2wq       ; 本机 Realm，不可分享
```

RealmId 默认由 Actrium 随机生成（base32 小写短串，不含程序名），可读名称写入 Realm Profile 的 `display_name`；用户可在新建确认页显式改用可读 RealmId，如上例的 `notes`。

实例里的 Actor 以 ActorId 标识，例如 `2af01.k7m2q9xh@acme.com`：去掉 `2af01.` 即 Realm 名。它的链接形式是 `actr://acme.com/2af01.k7m2q9xh`；程序升级替换 Actor 后这类链接失效，分享时默认用实例链接。程序身份 `publisher/program@version` 由包 manifest 与 Actor credential 携带。

旧格式（`actr://notes.example/team-notes@1.4.2?service=…`、只含数值 `realm_id` 的 `2af01@42/notes.example:team-notes:1.4.2`）一律拒绝，只能经显式迁移工具导入。

## 2. Publisher Profile

Publisher Profile 是 authority descriptor 的可选成员 `publisher`（已在 Actr Authority Discovery §4.5 定义），入口为 Publisher 域名唯一的 bootstrap 资源：

```text
https://notes.example/.well-known/actr
```

样例（`publisher` 成员使用 snake_case；`domain` 必须与 `authority` 的主机名一致）：

```json
{
  "version": 1,
  "authority": "actr://notes.example",
  "services": [
    {
      "rel": "contracts",
      "href": "https://notes.example/actr/contracts"
    }
  ],
  "publisher": {
    "schema": "actr.publisher/v1",
    "sequence": 17,
    "domain": "notes.example",
    "profile_signing_keys": [
      {
        "id": "publisher-root-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "active"
      }
    ],
    "keys": [
      {
        "id": "actr-pub-3f9a1c0d2b7e4a61",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "not_before": "2026-04-01T00:00:00Z",
        "not_after": "2027-04-01T00:00:00Z",
        "status": "active"
      },
      {
        "id": "actr-pub-8c2e5b7d1a4f9e03",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "not_before": "2025-01-01T00:00:00Z",
        "not_after": "2026-06-30T00:00:00Z",
        "status": "retired"
      }
    ],
    "security_contact": "mailto:security@notes.example",
    "profile_issued_at": "2026-07-29T00:00:00Z",
    "profile_expires_at": "2026-08-29T00:00:00Z",
    "signature": {
      "key_id": "publisher-root-1",
      "algorithm": "Ed25519",
      "value": "<base64>"
    }
  }
}
```

`publisher.signature` 对象本身不进入 payload；`publisher` 成员的其余 JSON 按 RFC 8785 JCS 规范化后做 detached Ed25519 验证。`sequence` 必须单调递增。descriptor 中 `publisher` 以外的部分不签名，不参与 Publisher 信任判断。`profile_signing_keys` 与可选的 `rotations` 的格式、接受与轮换规则同 `realm_signing_keys`（Actr Authority Discovery §4.4）。包签名 key 的 `id` 由公钥算出，与 manifest `signature.key-id` 相同；`keys` 至多 8 项、其中 `active` 至多 2 项，轮换时先发布新 key、至少 24 小时后再用它签包（[Actor Package](../../../actr/protocols/actor-package.zh.md) §6.8）。

## 3. 包下载

从已验证的 Realm Profile 解析 `primary_package` 和 `front_package`。逐包以不带凭据的 HTTPS 获取 `href`，核对整包 SHA-256，并用精确 Actor 类型所属 Publisher 的公钥验证 `manifest.sig`。加载前核对 Front 必需的 Service 依赖和 RPC 契约。完整示例见 [§6.2](#62-realm-profile)。

## 4. `.actr` Manifest

Team Notes 为每个 Workload 分发一个独立签名的 `actr-package/2` 包。文件大小、摘要和 key id 为示例值。

### 4.1 Service 包

WASM Component 是该包的 Workload。示例中的唯一 Proto 是 Service 的 RPC 契约。

```toml
format = "actr-package/2"

[actor]
publisher = "notes.example"
name = "team-notes"
version = "1.4.2"

[workload]
profile = "actr.component/1"
entry = "workload/service.wasm"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = []
optional-capabilities = []

[workspace]
requirement = "required"
access = "read-write"
providers = ["managed-store", "local-folder", "cloud-drive"]

[signature]
algorithm = "ed25519"
key-id = "actr-pub-3f9a1c0d2b7e4a61"

[[files]]
path = "workload/service.wasm"
role = "workload"
media-type = "application/wasm"
size = 1832000
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/service.proto"
role = "contract"
media-type = "text/plain"
size = 1204
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"

[[files]]
path = "contracts/privacy.toml"
role = "contract"
media-type = "application/toml"
size = 530
sha256 = "50037be0a33ed2265fe5a79b3f6a0b36762d81de6bff066ea33c6a9d21f0f171"
```

### 4.2 Front 包

Keel Guest 是该包的 Workload。`workload.keel.contract` 指向 Guest 自身的 Keel 契约。
`dependencies.service.contract` 指向生成 Service RPC 编解码代码时使用的 Proto。

```toml
format = "actr-package/2"

[actor]
publisher = "notes.example"
name = "team-notes-front"
version = "1.4.2"

[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[workload.keel]
runtime = "js"
guest-api = "keel.guest/1"
capabilities = "workload/capabilities.json"
contract = "contracts/notes.json"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = ["actor.call", "ui.render"]
optional-capabilities = []

[dependencies.service]
actor = "notes.example/team-notes"
version = "=1.4.2"
contract = "contracts/service.proto"
required = true

[signature]
algorithm = "ed25519"
key-id = "actr-pub-3f9a1c0d2b7e4a61"

[[files]]
path = "workload/guest.js"
role = "workload"
media-type = "text/javascript"
size = 18432
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "workload/capabilities.json"
role = "profile"
media-type = "application/json"
size = 746
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/notes.json"
role = "contract"
media-type = "application/json"
size = 512
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/service.proto"
role = "contract"
media-type = "text/plain"
size = 1204
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
```

两个示例使用同一份 `contracts/service.proto`。每个归档各自包含 `manifest.sig`。
Realm、信令与信任等部署参数由 `actr.toml` 提供。

## 5. Keel Capability Manifest

```json
{
  "schemaVersion": 1,
  "keelApi": "1.0",
  "guestEntry": "workload/guest.js",
  "imports": [
    "react",
    "react/jsx-runtime",
    "keel/guest",
    "host:service",
    "host:clipboard"
  ],
  "hostModules": {
    "service": {
      "rpc": [
        "notes.v1.NoteService.List",
        "notes.v1.NoteService.Get",
        "notes.v1.NoteService.Save"
      ],
      "events": [
        "notes.v1.changed"
      ]
    },
    "clipboard": {
      "rpc": [
        "writeText"
      ]
    }
  },
  "components": [
    "View",
    "Text",
    "TextInput",
    "Button",
    "FlatList",
    "ImageAsset"
  ]
}
```

Actrium 不能只信这个文件；安装分析结果必须与它一致。

## 6. Realm 签名 key 与 Realm Profile

### 6.1 Realm 签名 key：authority descriptor 扩展

域名的 Realm 签名 key 发布在该域名的 descriptor 中：

```text
https://acme.com/.well-known/actr
```

样例（`realm_signing_keys` 成员与 `realms` relation 已在 Actr Authority Discovery §4.3–4.4 定义）：

```json
{
  "version": 1,
  "authority": "actr://acme.com",
  "services": [
    {
      "rel": "signaling",
      "href": "wss://signal.provider.example/actr",
      "protocol": "actr-signaling/1"
    },
    {
      "rel": "ice-config",
      "href": "https://rtc.provider.example/actr/ice",
      "protocol": "actr-ice-config/1"
    },
    {
      "rel": "realms",
      "href": "https://realms.provider.example/acme.com",
      "protocol": "actr-realm-profile/1"
    }
  ],
  "links": ["/i/"],
  "realm_signing_keys": {
    "sequence": 3,
    "keys": [
      {
        "id": "rs-2026-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "active"
      },
      {
        "id": "rs-2025-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "retired"
      }
    ],
    "rotations": [
      {
        "from": "rs-2025-1",
        "to": "rs-2026-1",
        "sequence": 2,
        "signature": "<base64>"
      }
    ],
    "signature": {
      "key_id": "rs-2026-1",
      "algorithm": "Ed25519",
      "value": "<base64>"
    }
  }
}
```

`realm_signing_keys.keys` 是该域名下所有 Realm 的长期信任根；`rotations` 中每一项由 `from` key 对 `{from, to, public_key, sequence}` 的 JCS 形式签名，`sequence` 为引入该轮换时的成员 `sequence`；从 `keys` 移除某个 key 即吊销它签过的全部 Realm Profile；`realm_signing_keys.signature` 由某个 `active` key 对去掉 `signature` 后的该成员 JCS 规范化结果签名，覆盖 `sequence`。descriptor 其余部分不签名，也不列出 Realm 清单。`links` 作为 descriptor 元数据保留（Actr Authority Discovery §4.6），但 Actrium 不通过它解析或分享 Realm 链接；Realm 链接的输入与分享只使用 `actr://`。`services` 中的 signaling、ice-config 等服务于该 authority 自身，与该域名下的 Realm 无关；访问 Realm 只使用其 Realm Profile 指定的 Actr Facility Provider，Realm Profile 不可用、无效或退役时不得使用任何其他 Actr Facility Provider。Actr Facility Provider 自身的 descriptor（`https://provider.example/.well-known/actr`）另以 `realm-management` link（protocol `actr-realm-management/1`）公布管理请求端点。

### 6.2 Realm Profile

[Echo 的完整签名示例](../../../actr/protocols/examples/realm-profile.json)用 `primary_package: "echo"` 选择 Service，用 `front_package: "ui"` 选择 Front。Realm 签署整份 Profile，更换选择时递增 `sequence` 并重签。测试公钥和字段定义见 [Realm Profile §4](../../../actr/protocols/realm-profile.zh.md#4-realm-profile-格式)。

## 7. Service Inventory Record

```json
{
  "realm": "notes@acme.com",
  "serviceActorId": "2af01.notes@acme.com",
  "program": {
    "publisher": "notes.example",
    "name": "team-notes",
    "version": "1.4.2",
    "packageDigest": "sha256:..."
  },
  "workspaceId": "ws_01...",
  "runtime": {
    "placement": "local",
    "hostDeviceId": "device_...",
    "lifecycle": "running",
    "desiredState": "running",
    "reachability": "online",
    "health": "healthy"
  },
  "attachment": {
    "state": "leased",
    "policy": "exclusive"
  },
  "createdAt": "2026-07-29T10:00:00Z",
  "lastOpenedAt": "2026-07-29T11:00:00Z"
}
```

不包含 actor private key、Workspace credential 或 signaling/TURN token。

## 8. Permission Grant

```json
{
  "subject": {
    "publisher": "notes.example",
    "program": "team-notes",
    "majorVersion": 1,
    "packageDigest": "sha256:..."
  },
  "capability": "host:clipboard.write",
  "scope": {
    "serviceActorId": "2af01.notes@acme.com",
    "workspaceId": "ws_01..."
  },
  "decision": "allow-while-open",
  "source": "user",
  "grantedAt": "2026-07-29T10:00:00Z",
  "expiresAt": null
}
```

## 9. Front Launch Context

Guest 只读：

```json
{
  "program": {
    "publisher": "notes.example",
    "name": "team-notes",
    "version": "1.4.2"
  },
  "route": "/today",
  "query": {
    "filter": "assigned"
  },
  "session": {
    "connection": "p2p",
    "realmDisplayName": "Acme Notes"
  },
  "workspace": {
    "displayName": "Team Notes",
    "mode": "read-write"
  },
  "theme": "system",
  "locale": "zh-CN"
}
```

不提供：

- Front/Service credential；
- session id/secret；
- Workspace path；
- Actor arbitrary call API；
- Realm 客户端 key；
- signaling/TURN endpoint credential。

### 9.1 隐私状态上下文

Guest 只能看到最小化的用户可见摘要：

```json
{
  "privacy": {
    "color": "green",
    "ready": true,
    "policyId": "actrium.consumer.default",
    "workloadKind": "wasm-component",
    "applicationStateDigest": "…",
    "expiresAt": "2026-07-29T12:01:15Z"
  }
}
```

Guest 不获得 Actor 图、原始 evidence、attestation token 或被隐藏的 ActorId。完整对象见 [13](13-privacy-state-and-aggregation.md)；语义见 [docs/security](../../architecture/security/actrium/README.zh.md)。

## 10. 系统协议错误码

### URI

```text
URI_SCHEME_INVALID
URI_DOMAIN_INVALID
URI_LABEL_INVALID
URI_LEGACY_FORM
URI_LOCAL_REALM_FOREIGN
```

### Package

```text
PUBLISHER_PROFILE_INVALID
PUBLISHER_PROFILE_EXPIRED
PUBLISHER_PROFILE_ROLLBACK
PUBLISHER_IDENTITY_CHANGED
PACKAGE_DOWNLOAD_FAILED
PACKAGE_DIGEST_MISMATCH
PACKAGE_EQUIVOCATION
PACKAGE_SIGNATURE_INVALID
PACKAGE_KEY_REVOKED
PACKAGE_ARCHIVE_INVALID
PACKAGE_ABI_UNSUPPORTED
PACKAGE_REVOKED
GUEST_POLICY_INVALID
SERVICE_IMPORT_FORBIDDEN
```

### Realm/Identity

```text
REALM_KEYS_INVALID
REALM_DOMAIN_UNTRUSTED
REALM_SIGNING_KEY_CHANGED
REALM_PROFILE_UNAVAILABLE
REALM_PROFILE_INVALID
ACTR_FACILITY_PROVIDER_MISMATCH
REALM_PROFILE_ROLLBACK
REALM_RETIRED
REALM_LOGIN_REQUIRED
REALM_MEMBERSHIP_REVOKED
CLIENT_REVOKED
INVITE_EXPIRED
INVITE_ALREADY_USED
INVITE_REVOKED
INVITE_REALM_MISMATCH
```

### Service/Workspace

```text
SERVICE_CREATE_FAILED
SERVICE_IDENTITY_RECOVERY_FAILED
SERVICE_STOPPED
SERVICE_SUSPENDED
SERVICE_OFFLINE
SERVICE_UNREACHABLE
SERVICE_NOT_FOUND
SERVICE_ACTOR_TYPE_MISMATCH
SERVICE_MIGRATING
SERVICE_TRASHED
SERVICE_DELETED
WORKSPACE_REQUIRED
WORKSPACE_UNAVAILABLE
WORKSPACE_PERMISSION_DENIED
WORKSPACE_WRITE_LEASE_CONFLICT
WORKSPACE_QUOTA_EXCEEDED
```

### Session

```text
ATTACH_PERMISSION_DENIED
ATTACH_IDENTITY_MISMATCH
ATTACH_PROGRAM_MISMATCH
ATTACH_DIGEST_MISMATCH
ATTACH_PROTOCOL_MISMATCH
ATTACH_SESSION_OCCUPIED
ATTACH_REALM_MISMATCH
SESSION_EXPIRED
SESSION_REPLAYED
SESSION_TAKEOVER_DENIED
```

### Guest/Capability

```text
GUEST_LOAD_FAILED
GUEST_RUNTIME_TIMEOUT
GUEST_HEAP_EXCEEDED
GUEST_RENDER_LIMIT_EXCEEDED
CAPABILITY_NOT_DECLARED
CAPABILITY_NOT_GRANTED
CAPABILITY_PLATFORM_DENIED
CAPABILITY_RATE_LIMITED
CONTRACT_INPUT_INVALID
CONTRACT_OUTPUT_INVALID
```

### Assurance

```text
ASSURANCE_INVALID
ASSURANCE_EXPIRED
ASSURANCE_REVOKED
ASSURANCE_SUBJECT_MISMATCH
ASSURANCE_SELF_SIGNED
ASSURANCE_CONFLICT_WEAKEST
POLICY_UNSUPPORTED
GRAPH_INCOMPLETE
SNAPSHOT_NOT_CURRENT
GRAPH_LIMIT_EXCEEDED
ACTOR_SUSPENDED
UNDECLARED_EGRESS
GENERATION_VIOLATION
TERMINAL_NOT_AUTHORIZED
NATIVE_NOT_MEASURABLE
CHANNEL_INVALID
```

## 11. 状态枚举

### Program Release

```text
discovered
resolving
downloading
verifying
installed
update-available
identity-changed
revoked
quarantined
incompatible
```

### Service lifecycle

```text
creating
stopped
starting
running
suspended
stopping
migrating
archived
trashed
deleting
deleted
```

### Service desired state

```text
running
stopped
archived
```

### Service reachability

```text
online
offline
unreachable
unknown
```

### Service health

```text
healthy
degraded
failed
unknown
```

### Attachment

```text
idle
leased
occupied
```

### Application color

```text
green
yellow
red
unavailable
```

### Privacy enforcement

```text
allow
block
```

### Shell navigation

```text
parsing
resolving-package
awaiting-confirmation
creating-service
attaching
negotiating
loading-guest
failed
```

### Resident Front

```text
loading
running
suspended
failed
closing
closed
```

## 12. 术语表

本文档使用的术语统一定义在[术语表](../../GLOSSARY.zh.md)，此处不再重复。

## 13. 规范与实现参考

### Actrium

- [Actr repository](https://github.com/Actrium/actr)
- [Keel repository](https://github.com/Actrium/keel)
- [Actr ActorId/ActrType formatting](https://github.com/Actrium/actr/blob/main/core/protocol/src/actr_ext.rs)
- [Actr package manifest](https://github.com/Actrium/actr/blob/main/core/pack/src/manifest.rs)
- [Actr package builder](https://github.com/Actrium/actr/blob/main/cli/src/commands/package_build.rs)
- [Keel Guest–Host interaction](https://github.com/Actrium/keel/blob/main/docs/architecture/guest-host-interaction.md)
- [Keel Guest runtime](https://github.com/Actrium/keel/blob/main/docs/architecture/guest-runtime.md)
- [Keel contract](https://github.com/Actrium/keel/blob/main/src/contract/index.ts)

### Internet standards

- [RFC 3986: URI Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986)
- [RFC 7595: URI Scheme Guidelines](https://www.rfc-editor.org/rfc/rfc7595)
- [RFC 5890: IDNA Definitions](https://www.rfc-editor.org/rfc/rfc5890)
- [RFC 8785: JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)
- [RFC 8489: STUN](https://www.rfc-editor.org/rfc/rfc8489)
- [RFC 8656: TURN](https://www.rfc-editor.org/rfc/rfc8656)
- [RFC 9530: TURN Third-Party Authorization](https://www.rfc-editor.org/rfc/rfc9530)
- [The Update Framework Specification](https://theupdateframework.github.io/specification/latest/)
