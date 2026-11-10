# Actr Realm Profile

语言：[English](realm-profile.md) | **简体中文**

状态：初始草案。

本文定义 Actr Host 或 Actr Facility Provider（AFP）如何把 Realm 名解析为经过验证的 Realm Profile、
`actr.realm/v1` Realm Profile 格式，以及 Actr Facility Provider 接受的 Realm 管理请求。

## 1. 范围

Realm 是一个应用实例的信任边界。创建一个应用实例就创建一个新的 Realm。Realm 名写作
`<realm_id>@<domain>`，例如 `k7m2q9xh@acme.com`，语法见 [Actr URI](uri.zh.md) 第 5.1 节。

Realm Profile 是一份签名的公开文档，说明该实例运行哪个包，以及由哪个 Issuer 和哪个
Actr Facility Provider 服务该 Realm。它由该 Realm 域名的 Realm 签名 key 签署，因此可以存放在任何地方：域名自己的静态
托管、Actr Facility Provider，或内嵌在 Realm 邀请中。取得它的位置不影响其有效性。

Realm 在整个生命周期内只由一个 Actr Facility Provider 服务。Actr Facility Provider 是由某个运营方运行的
部署，为 Realm 提供 signaling、TURN 与 Issuer。把应用迁到另一个运营方，意味着在那里新建
Realm 并迁移应用数据，旧 Realm 退役。

本文不定义 Actr Facility Provider 提供的 signaling、identity 或 ICE 协议，不定义 Actor credential
格式，也不定义应用实例内部的应用权限。Realm membership（成员、Realm 邀请与 Realm 客户端 key）见
[Actr Realm Membership](realm-membership.zh.md)。

本文中的大写关键词 **MUST**、**MUST NOT**、**SHOULD**、**SHOULD NOT** 和 **MAY**
按 BCP 14 解释。

## 2. Realm 域名

每个 Realm 都在某个 Realm 域名之下。域名只作命名空间根：一个现成的、全局唯一且归属可验证
的名字。没有域名就没有 Realm，本机 Realm（第 2.2 节）除外。

### 2.1 DNS 域名

DNS Realm 域名即 `actr://<domain>` 中的域名。域名所有者即
**Realm 签名 key 持有方**：其 Ed25519 Realm 签名 key 是该域名下所有 Realm 的长期信任根，
发布、轮换与回滚保护由 [Actr Authority Discovery](authority-discovery.zh.md) 第 4.4 节
定义。域名只需公布一次 Realm 签名 key；创建 Realm 不改动域名托管的任何内容。

域名所有者可以自己持有 key（通常放在签名服务中），也可以委托 Actr Facility Provider 代持。没有
自己域名的个人，在某个 Actr Facility Provider 的域名下获得 Realm；此时该 Actr Facility Provider 就是域名
所有者和 key 持有方。谁持有 key，谁就是该域名下 Realm 的信任根。

在规范化 A-label 上比较，Realm 域名等于其 Realm Profile 的 Actr Facility Provider ID，或以 `"."`
加该 Actr Facility Provider ID 结尾（按 label 边界）时，该 Realm 属于 **Actr Facility Provider 域名下的
Realm**。只有这种情况下，Actr Host 显示该 Actr Facility Provider 为信任根；其他情况显示 Realm 域名。
域名所有者委托 Actr Facility Provider 代持 key 的情况对 Actr Host 不可见（第 9 节）。

### 2.2 本机 Realm

本机 Realm 使用 RFC 9476 `.alt` 后缀下的伪域名 `<installation-id>.actr.alt`。
`installation-id` 是安装时生成的 128 位随机数，按 RFC 4648 base32 小写编码为 26 个字符，
不带 padding。

Actr Host **MUST NOT** 通过 DNS 解析 `.actr.alt` 名称，也不得把它发给任何 Actr Facility
Provider。本机 Realm 没有 Realm Profile、没有 Actr Facility Provider，也不使用 Realm 管理请求；
Actr Host 为每个本机 Realm 保存本地记录，其中至少含实例运行的包（下载地址或本地文件，加
SHA-256）。本机 Realm 名只在该安装上可用。本文其余部分只
适用于 DNS Realm 域名。

## 3. 定位 Realm Profile

对 Realm 名 `<realm_id>@<domain>`，Actr Host：

1. 按 Actr Authority Discovery 取得并验证 `actr://<domain>` 的 authority descriptor 及其
   `realm_signing_keys` member；
2. 选出 `rel` 为 `realms`、protocol 为 `actr-realm-profile/1` 的 service link；
3. 按文档顺序逐条处理：去掉 `href` 末尾的 `/`，以 `Accept: application/json` 发送
   `GET <href>/<realm_id>`，得到有效 Realm Profile 即停止。

`realms` 的 href **MUST NOT** 含 query 或 fragment；**MAY** 指向域名自己的托管或 Actr Facility
Provider，**MAY** 带显式端口。所有 link 都未产出有效 Realm Profile（`404 Not Found`、
其他错误、超时或无效文档）时，视为 Realm Profile 不可用。

该请求是公开的。Actr Host **MUST** 使用 HTTPS，**MUST NOT** 发送 `Authorization`
header、cookie 或其他 credential。redirect 遵循 Actr Authority Discovery 的规则：最多五次，
每一跳都使用 HTTPS。Actr Facility Provider 等特权进程 **MUST** 在每一跳额外执行 SSRF 检查：拒绝
非公网地址，把每一跳钉住到预检查过的地址，并把响应限制在 64 KiB。

Realm Profile 也 **MAY** 通过带外方式交付，例如内嵌在 Realm 邀请中；Actr Host 对这类
Realm Profile 的验证与通过 HTTPS 取得的完全相同。

## 4. Realm Profile 格式

Realm Profile 是 UTF-8 JSON object，member 名使用 snake_case。

```json
{
  "schema": "actr.realm/v1",
  "sequence": 31,
  "realm": {
    "name": "k7m2q9xh@acme.com",
    "display_name": "Acme Notes",
    "status": "active"
  },
  "package": {
    "href": "https://cdn.notes.example/team-notes-1.4.2.actr",
    "sha256": "<64 hex>",
    "type": "notes.example/team-notes@1.4.2"
  },
  "issuer": {
    "url": "https://identity.provider.example/realms/acme.com/k7m2q9xh",
    "keys": [
      { "id": "7", "algorithm": "Ed25519", "public_key": "<base64>", "status": "active" }
    ]
  },
  "provider": {
    "id": "provider.example",
    "signaling": [
      {
        "url": "wss://signal-a.provider.example/ws",
        "priority": 10,
        "control_plane_id": "provider.example/v1",
        "ticket_audience": "signal-a"
      }
    ],
    "ice": [
      { "urls": ["stun:stun.provider.example:3478"] },
      { "urls": ["turns:turn.provider.example:5349"], "credential_audience": "turn-pool-1" }
    ]
  },
  "policy": {
    "membership": "invite-only"
  },
  "issued_at": "2026-07-29T00:00:00Z",
  "signature": { "key_id": "rs-2026-1", "algorithm": "Ed25519", "value": "<base64>" }
}
```

| Member | 含义 |
| --- | --- |
| `schema` | **MUST** 为 `actr.realm/v1`。 |
| `sequence` | Realm Profile sequence：非负整数，该 Realm 每发布一次 Realm Profile 都递增。 |
| `realm.name` | Realm 名，**MUST** 与正在解析的名称一致。 |
| `realm.display_name` | 人类可读名称，不是标识符。 |
| `realm.status` | `active` 或 `retired`。 |
| `package.href` | 该实例所运行包的 HTTPS 下载地址，可以在任何位置。 |
| `package.sha256` | 该包全部字节的 SHA-256（小写十六进制）。 |
| `package.type` | 该包 manifest 声明的 Actor 类型 `publisher/program@version`，即该实例 Service 的类型。 |
| `issuer.url` | Issuer 的注册与验证端点。 |
| `issuer.keys` | 用于验证 Actor credential 的 Realm issuer 公钥，1 至 3 项（第 6.1 节）。 |
| `provider.id` | Actr Facility Provider ID：Actr Facility Provider 的规范化 DNS authority，在 Realm 生命周期内不变。 |
| `provider.signaling` | signaling 端点。共享同一 `control_plane_id` 的条目是同一 Actr Facility Provider control plane 的副本或故障切换入口，不是 federation。 |
| `provider.ice` | STUN 与 TURN 端点。TURN 条目标明短期 TURN credential 的 `credential_audience`。 |
| `policy` | 该 Realm 的加入策略。 |
| `issued_at` | Realm Profile 的签署时间（RFC 3339），仅供参考；Realm Profile 不过期。 |
| `signature` | Realm 签名 key 的 detached 签名。 |

`realm.status` 为 `active` 时 **MUST** 包含 `package`。

所有端点 **MUST** 包含非空 host。`package.href` 和 `issuer.url` **MUST** 使用 HTTPS，
`provider.signaling[].url` **MUST** 使用 WSS。每个 `provider.ice[].urls` 列表
**MUST** 非空，并包含遵循 [RFC 7064](https://www.rfc-editor.org/rfc/rfc7064) 或
[RFC 7065](https://www.rfc-editor.org/rfc/rfc7065) 的 STUN/TURN 地址：
`scheme:host[:port]`，IP literal 用方括号包裹，仅 TURN 可附加 `?transport=...`。
ICE 地址 **MUST NOT** 包含 `//`、userinfo、path 或 fragment。
即使 Profile 签名有效，消费者也 **MUST** 拒绝格式错误的端点。

Realm Profile **MUST NOT** 包含 secret、私钥、access token、成员列表或用户专属数据。
Actr Host **MUST** 忽略未知 member。

## 5. 验证

`signature` 是对去掉 `signature` member 后的 Realm Profile 按 RFC 8785 JCS 规范化结果所做
的 detached Ed25519 签名。Realm 签名 key 持有方 **MUST** 使用 `active` Realm 签名 key。

Actr Host 或 Actr Facility Provider 只在以下条件全部满足时接受 Realm Profile：

1. `schema` 为 `actr.realm/v1`，且 `realm.name` 与正在解析的 Realm 名一致；
2. `signature.key_id` 指向该 Realm 域名已接受的 `realm_signing_keys` member 中列出的 key
   （Actr Authority Discovery 第 4.4 节），且用该 key 验签通过；
3. 若此前已接受过该 Realm 的 Realm Profile：
   - Realm Profile sequence 不低于已接受的最高值，等于时内容必须与已接受的 Realm Profile
     完全相同；
   - `provider.id` 与已接受的 Realm Profile 相同；
   - 已接受的 Realm Profile 不是已退役状态。

有效性只取决于这些检查，与 Realm Profile 的来源 URL 无关。因此存放 Realm Profile 的 Actr Facility
Provider 不是信任根。

退役 Realm Profile（`status` 为 `retired`）永久有效。接受它之后，Actr Host **MUST** 停止
使用该 Realm。

## 6. 使用 Realm

Actr Host 以不带 credential 的 HTTPS GET 下载 `package.href`，**MUST** 核对
`package.sha256`，并按 [Actor Package](actor-package.zh.md) 验证包签名，包 manifest 声明的 Actor 类型
**MUST** 等于 `package.type`；任一失败即不运行。该类型即该实例的程序：实例的 Service 是本 Realm 内 Actor 类型与之
相同的那个 Actor，恰好一个；零个或多于一个都是错误。

Actr Host 只通过已接受 Realm Profile 指定的 Issuer 和 Actr Facility Provider 访问 Realm。该 Realm
的 Actor credential 以该 Realm 名为 issuer，用 `issuer.keys` 中的 Realm issuer key 验证（第 6.1 节）。
Realm membership 指客户端已在该 Realm 注册其 Realm 客户端 key 且未被撤销；它决定客户端能否
进入该实例的网络，Issuer 只向已注册且未撤销的 Realm 客户端 key 签发 Actor credential（[Actr Realm
Membership](realm-membership.zh.md)）。实例内的应用权限由应用决定，不属于本文范围。

Actr Host **SHOULD** 在与 Realm 建立会话时刷新该域名的 `realm_signing_keys` member，并从
`realms` 位置刷新 Realm Profile，保持连接期间至少每 24 小时刷新一次。Actr Facility Provider
**SHOULD** 以同样间隔为其服务的 Realm 刷新这两者；最新 Realm Profile 已退役或不再列出自己
用于签发的 Realm issuer key 时，停止为该 Realm 签发 Actor credential。

若签署已接受 Realm Profile 的 key 已从该域名的 `keys` 中移除（即吊销，见 Actr Authority
Discovery 第 4.4 节），Actr Host 或 Actr Facility Provider 丢弃该 Realm Profile 及其 Realm Profile
sequence 与 `provider.id` 基线，改用最后一份由仍列出的 key 签名的已接受 Realm Profile；
没有则按从未接受过的情况重新取得。这样域名所有者只需移除 key 并用 `active` key 重签受影响
的 Realm Profile，即可清除用泄露 key 签发的 Realm Profile，包括 sequence 被抬高的。

刷新时 Realm Profile 不可用，Actr Host 继续使用已接受的 Realm Profile。取得的 Realm
Profile 验证失败时，Actr Host 忽略它、保留已接受的并报告失败。从未接受过 Realm Profile 且
无法取得时，Actr Host **MUST** 报错。任何情况下 Actr Host 都 **MUST NOT** 使用已接受
Realm Profile 指定之外的任何 Actr Facility Provider，包括 authority descriptor 中列出的 service。

### 6.1 Realm issuer key

`issuer.keys` 列出 1 至 3 把 Realm issuer key，每把有 `id`、`algorithm`（`Ed25519`）、base64
`public_key` 与 `status`。Actor credential 头部带 32 位无符号整数 `key_id`，`id` **MUST** 是该
整数的十进制形式，不带前导零，例如 `"7"`。`id` 在该 Realm 的全部历史中只对应一个公钥，
**MUST NOT** 复用。

验证 Actor credential 时，验证者取 credential 所属 Realm 的已接受 Realm Profile，在其
`issuer.keys` 中找 `id` 与 `key_id` 相符的 key（`active` 或 `retired` 均可），用它验签并检查
有效期；**MUST NOT** 使用其他 Realm 的 key。找不到时，验证者 **SHOULD** 刷新该 Realm Profile
后再判，同一 Realm 每 60 秒至多刷新一次；仍找不到即拒绝。

Issuer 只用 `active` key 签发，同一时刻只用一把。`retired` key 不再签发，它签发且未过期的
credential 仍然有效。从 `issuer.keys` 移除某把 key 即吊销它：它签发的 credential 立即失效，
Actor 需重新取得 credential。

轮换按「先发布、再使用、后移除」进行，每一步都是一份 sequence 更高的 Realm Profile：

1. **预发布。** 加入新 key（`active`），Issuer 继续用旧 key 签发。
2. **切换。** 列出新 key 的 Realm Profile 被接受至少 24 小时后（第 6 节的刷新间隔），Issuer 改用
   新 key；此后的 Realm Profile 把旧 key 标为 `retired`。
3. **移除。** 旧 key 签发的最后一个 credential 过期再过 5 分钟（容许时钟偏差）后，Realm Profile
   可以不再列出旧 key。

Actr Facility Provider **SHOULD** 每 90 天轮换一次。key 泄露时跳过等待，立即发布移除它的 Realm
Profile。对 Actr Facility Provider 域名下的 Realm，Actr Facility Provider 自行签署这些 Realm Profile；对其他
Realm，Actr Facility Provider 在 `draft_profile` 响应（第 8 节）中给出含新 key 的草案，由 Realm 签名
key 持有方签署后以 `publish_profile` 提交。持有方发布之前，Actr Facility Provider 继续使用旧 key。

## 7. 生命周期

### 7.1 创建

Actr Facility Provider 由 Actr Host、用户或 key 持有方的策略选定。每一步都须经 Realm 签名 key
持有方授权（第 7.3 节）。

1. 向选定的 Actr Facility Provider 发送 `create_realm` 请求（第 8 节）。Actr Facility Provider 创建
   pending Realm，准备其 Realm issuer key 与端点，并返回未签名的首个 Realm Profile。
2. Realm 签名 key 持有方签署该 Realm Profile，以 `publish_profile` 请求提交。Actr Facility
   Provider 验证（第 5 节）后激活 Realm，并在这次响应中一次性返回 owner 邀请（Actr Realm Membership
   第 5 节）。若 `realms` 位置由该 Actr Facility Provider 提供，它此后对外提供该 Realm Profile；否则
   由域名所有者发布到 `realms` 位置。

对 Actr Facility Provider 域名下的 Realm，Realm Profile 由该 Actr Facility Provider 自己签署，不经 Realm 管理请求
（第 7.3 节）。未按时激活的 pending Realm 过期后被清除。

### 7.2 更新

sequence 更高的 Realm Profile 可以更改 `package`（升级程序）、Realm issuer key、Actr Facility
Provider 端点、`display_name` 与 `policy`，**MUST NOT** 更改 `provider.id`。

新 Realm Profile 同样以 `publish_profile` 提交给 Actr Facility Provider（持有方可先用 `draft_profile`
取得含 Actr Facility Provider 当前 issuer 与端点的草案）；`realms` 位置不在 Actr Facility
Provider 时，持有方还要把它发布到该位置。对 Actr Facility Provider 域名下的 Realm，由 Actr Facility Provider 按 owner 的
`update_realm` 请求（Actr Realm Membership 第 4 节）更改 `package` 或 `display_name` 并签署新 Realm Profile。

### 7.3 由 key 持有方授权

创建、发布 Realm Profile 以及重新签发 owner 邀请，都须经 Realm 签名 key 持有方授权。版本 1
不定义创建委派 key。Actr Host 按第 2.1 节的规则区分：

- **Actr Facility Provider 域名下的 Realm。** Actr Facility Provider 就是持有方。版本 1 中这类 Realm 只由 Actr Facility
  Provider 的运营者创建，不经 Realm 管理请求，此后由 owner 以 membership 请求更新或退役；运营者在创建时得到 owner 邀请，经带外渠道交给
  用户（Actr Realm Membership 第 5 节）。由用户以 Actr Facility Provider 账号自助创建属于后续工作（第 10 节）。
- **其他 Realm。** 每个 Realm 管理请求都是由 `active` Realm 签名 key 签名的 Actr 管理信封
  （第 8 节），由 Realm 签名 key 持有方的签名服务签发；持有方可以是域名所有者，也可以是其
  委托代持的 Actr Facility Provider。签名服务如何认证所代签的人，不在本文范围内。

### 7.4 退役

退役只有一条路径：域名所有者发布退役 Realm Profile。Actr Facility Provider 通过刷新得知，写入
Actr Facility Provider tombstone 并停止服务该 Realm。该 Realm 名永不复用。退役 Realm Profile 保持
相同的 `provider.id`，且 sequence 更高。若该 Realm 的 Actr Facility Provider 已停服，域名所有者把
`realms` link 改指自己控制的位置，并在那里发布退役 Realm Profile。属于 Actr Facility Provider 的
域名随其一起消失：其中的 Realm 无法退役，只能在别处重新创建。Actr Facility Provider 域名下的 Realm 由 Actr Facility Provider
按 owner 的 `update_realm` 请求发布退役 Realm Profile。

### 7.5 Realm 签名 key 丢失

某域名的 Realm 签名私钥全部丢失时，已有 Realm 无法再获得新 Realm Profile。域名所有者可以
发布新的 Realm 签名 key（Actr Host 按身份变化处理，见 Actr Authority Discovery 第 4.4
节），并以新的 RealmId 创建新的 Realm。把丢失的 key 继续以 `retired` 状态列出，已有
Realm 就能继续使用当前 Realm Profile。

## 8. Realm 管理请求

Actr Facility Provider 以 HTTPS `POST` 请求的形式，在其自身 authority descriptor
`https://<provider_id>/.well-known/actr` 的 `realm-management` service link（Actr
Authority Discovery 第 4.3 节）接受以下 Realm 管理请求：

| `op` | 用途 |
| --- | --- |
| `create_realm` | 创建 pending Realm，返回其未签名的首个 Realm Profile（其中 `package` 取自请求）。 |
| `publish_profile` | 在 `profile` 中提交已签名的 Realm Profile：首次提交激活 pending Realm，响应一次性携带 owner 邀请（`owner_invite`）；之后提交 sequence 更高的版本即更新（含退役）。 |
| `reissue_owner_invite` | 签发新的 owner 邀请（响应 `owner_invite`），并使该 Realm 此前未用的 owner 邀请失效；用于 owner 丢失全部客户端时的恢复（Actr Realm Membership 第 5 节）。 |
| `draft_profile` | 只用于 Actr Facility Provider 域名以外的 Realm：返回下一份未签名的 Realm Profile 草案。其 sequence 为已接受值加一，`package`、`display_name`、`policy` 取自已接受的 Realm Profile，`issuer` 与 `provider` 为 Actr Facility Provider 的当前值（含轮换中的 Realm issuer key，第 6.1 节）。不改变 Realm。 |

Realm 管理请求只用于 Actr Facility Provider 域名以外的 Realm（第 7.3 节）。每个请求都是 Actr 管理信封：
下述 payload 加上该 Realm 域名一把 `active` Realm 签名 key 的签名。

`create_realm` 请求的 payload 为：

```json
{
  "op": "create_realm",
  "realm": "k7m2q9xh@acme.com",
  "package": { "href": "https://cdn.notes.example/team-notes-1.4.2.actr", "sha256": "<64 hex>", "type": "notes.example/team-notes@1.4.2" },
  "provider_id": "provider.example",
  "requester": "<opaque member id>",
  "nonce": "<random>",
  "issued_at": "2026-07-29T00:00:00Z",
  "expires_at": "2026-07-29T00:05:00Z"
}
```

其他操作使用同一信封与各自的 `op` 和字段。`requester` 可选：key 持有方的签名服务把它设为
所代签者的不透明标识。其他规范 **MAY** 以自己的 `op` 复用 Actr 管理信封，例如 Actr
Authority Discovery 第 4.5 节用它登记 publisher，Actr Realm Membership 用它签 membership 请求。

- payload 按 RFC 8785 JCS 规范化，附 detached Ed25519 签名及其 `key_id`。
- `provider_id` **MUST** 等于接收方的 Actr Facility Provider ID，使请求无法重放到其他 Actr Facility
  Provider。
- `expires_at` 与 `issued_at` 之差 **MUST NOT** 超过五分钟，Actr Facility Provider 拒绝窗口之外的
  请求。
- Actr Facility Provider 记录每个 `nonce` 直到请求过期，并拒绝重复使用。nonce 只防字节级重放。
- `create_realm` 以 Actr Facility Provider 幂等键 `(realm, 请求方)` 保证幂等。请求方为
  `requester` 字段（缺省为空）。同一请求方对自己的 pending 或 active Realm 的重复请求返回该
  Realm 的状态，不返回 owner 邀请；指向其他请求方的 Realm 或已退役 Realm 的请求，一律返回冲突。
- 对已 active 的 Realm，`publish_profile` 只接受 sequence 更高的 Realm Profile，且不再返回
  owner 邀请。首次激活的响应丢失或 owner 邀请过期时，用 `reissue_owner_invite` 重新签发。

接受签名请求前，Actr Facility Provider 按与 Actr Host 相同的规则，通过 Actr Authority Discovery
验证签名用的 Realm 签名 key。由于这次抓取发生在验签之前，Actr Facility Provider **MUST** 按来源
地址和目标域名限流，**SHOULD** 缓存结果（含失败）。Actr Facility Provider **SHOULD** 按 Realm 域名
和按 Realm 执行配额。

## 9. 安全与隐私考虑

- Realm 签名 key 价值很高，强烈建议离线或硬件保存。Actr Facility Provider 只为委托其代持的域名
  （包括它自己的域名）持有 Realm 签名 key。
- Realm Profile 不过期。从未见过较新 Realm Profile 的 Actr Host，可能接受带外交付的旧
  Realm Profile（例如旧 Realm 邀请中的）。从 `realms` 位置刷新可以缩小这一窗口；对见过较新
  Realm Profile 的 Actr Host，Realm Profile sequence 可防止回滚。
- 只使用 Realm Profile 指定的 Actr Facility Provider，可防止攻击者阻断 Realm Profile 获取后把
  Actr Host 导向未经授权的 Actr Facility Provider。
- authority descriptor 不列出 Realm，但 Realm 名在分享时会暴露 Realm 域名与 RealmId。
  Actr Host **SHOULD** 默认生成随机 RealmId，把可读名称放在 `display_name`。
- Realm 名与 ActorId 形似邮件地址。Actr Host **MUST NOT** 把它们渲染为 `mailto:` 链接，应
  显示为实例地址（[Actr URI](uri.zh.md) 第 5.1 节）。
- Actr Host 看不到域名所有者已委托 Actr Facility Provider 代持 Realm 签名 key；对这类 Realm，信任根
  显示为 Realm 域名。这是已接受的限制。
- 泄露的 Realm 签名 key 通过从 `keys` 中移除并重签受影响的 Realm Profile 来吊销（第 6
  节）。尚未见到更新后 `realm_signing_keys` member 的 Actr Host 无法分辨，这是残余风险，由
  刷新限定窗口。
- 移除 key 也会使建立在其所签 Realm Profile 上的退役 Realm Profile 与 `provider.id` 基线
  失效。这是域名所有者信任边界内接受的后果：「Realm 名永不复用」是对所有者的要求，Actr Host
  只能对由仍列出的 key 签名的 Realm Profile 强制执行。Actr Facility Provider tombstone 不受影响。
- Actr Host 在每个 Realm 使用独立的 Realm 客户端 key，Realm 之间无法据 key 关联同一客户端。
  Actr Facility Provider 与 key 持有方的签名服务知道自己为谁创建了哪些 Realm（它们本就为这些 Realm
  签名）；Realm 成员看不到这一点。

## 10. 待完成工作

- 为 Realm 管理请求定义机器可读的 JSON Schema 与签名 test vector；Realm Profile 的
  Schema 与互操作向量已提供于 [`contracts/actr`](../../contracts/actr/README.md#realm-profile)，
  结构校验不替代签名、信任与后继 Profile 检查；
- 定义限定 RealmId 前缀与有效期的创建委派 key；
- Actr Facility Provider 域名下由用户以 Actr Facility Provider 账号（例如 OAuth 2.0 授权码 + PKCE）自助创建与
  管理 Realm，并按账号计配额。

## 11. 参考资料

- [Actr URI](uri.zh.md)
- [Actr Authority Discovery](authority-discovery.zh.md)
- [Actr Realm Membership](realm-membership.zh.md)
- [RFC 3339：Date and Time on the Internet: Timestamps](https://www.rfc-editor.org/rfc/rfc3339.html)
- [RFC 4648：The Base16, Base32, and Base64 Data Encodings](https://www.rfc-editor.org/rfc/rfc4648.html)
- [RFC 8032：Edwards-Curve Digital Signature Algorithm (EdDSA)](https://www.rfc-editor.org/rfc/rfc8032.html)
- [RFC 8785：JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html)
- [RFC 9476：The .alt Special-Use Top-Level Domain](https://www.rfc-editor.org/rfc/rfc9476.html)
