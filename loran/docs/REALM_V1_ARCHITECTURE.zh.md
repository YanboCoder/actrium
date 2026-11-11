# Loran Realm v1 架构契约

状态：v1 架构契约，与当前代码一致。一个 Loran 实例作为共享 Actr Facility Provider 承载多个
Realm；首版不承诺 HA 或跨 Realm federation。尚未落地的部分集中列在第 9 节。
历史名称（旧产品名、旧服务名）集中见[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」。

## 1. Realm 模型

### 1.1 Realm 是实例概念

Realm 对应一次应用实例部署，而不是一个应用（程序），也不是组织级成员域。同一个程序
`notes.example/team-notes` 被 A 组织部署一次、被 B 组织部署一次，就是两个 Realm。
在 Actrium 中，新建一个实例就同时创建一个新 Realm；该实例的 Service、Realm 内的其他
Actor 以及 Attach 到它的 Front 都在这个 Realm 内。Realm membership 与 Realm 客户端准入、ActorId、Actor credential 签发与续期、连接 ACL、撤销与审计
都属于该 Realm。

由此 Realm 的数量按部署次数增长，远多于应用数量，也远多于基础设施节点数量。Realm
必须是轻量的逻辑记录：创建一个 Realm 不启动新进程、不占用新端口、不需要新数据库。

### 1.2 Realm 名

Realm 以 Realm 域名命名；没有域名就不分配 Realm（本机 Realm 除外）。域名在现实中已经具备全局唯一性，且归属可验证；Realm 只使用这两点，
不要求域名指向 Loran，也不要求域名承载任何基础设施。

```text
Realm 名 = <realm_id>@<domain>        例：k7m2q9xh@acme.com、crm@acme.com
```

- `domain` 是规范化 DNS 域名：小写 IDNA A-label，无 scheme、端口、userinfo 或 path，
  与 Publisher ID 的规范化规则相同。它与不含端口的 Actr authority 同名：
  `k7m2q9xh@acme.com` 的域名就是 authority `actr://acme.com`。
- 本机 Realm 使用伪域名 `<installation-id>.actr.alt`（RFC 9476 `.alt`，永不经 DNS
  解析）。`installation-id` 是安装时随机生成的 128 位值（base32 小写 26 字符）。本机
  Realm 由 Actrium 内嵌实现，只有本地记录，没有 Actr Facility Provider、Realm Profile 与 Realm 管理请求；Loran 只
  承载 DNS 域名下的 Realm。
- RealmId（`realm_id`）使用 DNS label 的字符规则：`[a-z0-9-]`，1..63 字符，不以 `-` 开头或结尾；在域名内
  唯一，创建后不可修改；默认由客户端随机生成（base32 小写短串，不含程序名），可读名称
  放在 Realm Profile 的 `display_name`。
- 已退役 Realm 的名字永不复用：域名所有者不得复用，Actr Facility Provider 在本地以 Actr Facility
  Provider tombstone 强制，避免旧 Actor credential、ActorId 或缓存被新 Realm 继承。

一个 Realm 域名下可以有多个 Realm。

### 1.3 Realm 与 Publisher 的区别

两者都使用域名，但角色不同：

| | Publisher | Realm |
| --- | --- | --- |
| 命名对象 | 程序（谁发布了这个包） | 部署（谁运营这个实例） |
| 标识 | `publisher` 域名 | Realm 名 `<realm_id>@<domain>` |
| 长期信任根 | Publisher Profile 签名 key | Realm 签名 key |

同一个包可以运行在 `notes@acme.com`，也可以运行在 `notes@foo.org`。

### 1.4 Realm 与 Actr Facility Provider 的区别

Realm 是逻辑信任域；Actr Facility Provider 是运营中的共享基础设施，Loran 是其参考实现。一个 Loran
实例承载来自多个 Realm 域名的多个 Realm，共用同一进程、同一 SQLite、同一 signaling 服务、同一
STUN/TURN 端口池和同一 Issuer Signer（每个 Realm 一把 Realm issuer key）。Realm 在创建时
绑定一个 Actr Facility Provider，此后不可更换：Realm Profile 的 `provider.id` 在 Realm 生命周期内
不变。换运营方即在新 Actr Facility Provider 上新建实例并做应用层数据迁移，旧实例退役。

首版部署形态是单节点、SQLite、单一 Actr Facility Provider；Issuer、Rendezvous、Relay、Publisher
Registry 和 Management 是逻辑平面，可以同进程部署。Issuer Signer 支持同进程开发模式，也支持通过现有
TLS/mTLS gRPC 客户端配置放入独立安全边界。

只需要独占部署的场景（如企业自建）可配置 `max_realms = 1`；默认不限制。

Workspace 不属于 Realm 基础设施的数据模型。一个 Service 至多绑定一个
Workspace，同一 Workspace 可按写入并发策略被多个 Service 使用；删除 Actor 不删除 Workspace；Workspace 密钥属于用户。这些规则由 Actrium
实现，Loran 不取得目录所有权。

## 2. 信任根

1. Realm 签名 key（Ed25519）是一个 Realm 域名下所有 Realm 的长期信任根，由 Realm 签名
   key 持有方（域名所有者，或受其委托代持的 Actr Facility Provider）持有，
   声明在该域名 authority descriptor 的 `realm_signing_keys` 成员中，支持多把与轮换链。
   首次信任来自对 `domain` 的 HTTPS（Web PKI）访问，或企业预置的 key pin。
2. Realm Profile 由 Realm 签名 key 签名，把 Realm issuer key 与 Actr Facility Provider 端点委派给
   某个 Actr Facility Provider。Realm Profile 的有效性只取决于签名，不取决于从哪里取到。
3. Realm issuer key（Ed25519）签发 Actor credential，每个 Realm 同一时刻只用一把签发（轮换时
   Realm Profile 最多列 3 把，见 5.3），私钥留在 Actr Facility Provider 的 Issuer Signer；key 退役即销毁私钥，此后不再签名。
4. Publisher ID 是规范化 DNS 域名。Publisher Profile 是 authority descriptor
   `https://<publisher>/.well-known/actr` 中带签名的可选成员 `publisher`（Actr Authority
   Discovery §4.5）。其中的 Ed25519 key 才是包的长期真实性根；包 registry/origin 只是
   HTTPS 分发位置，包最终按 digest 和 publisher signature 验证。

信任链：

```text
Web PKI 或企业 pin → authority descriptor → realm_signing_keys（Realm 签名 key）
  → Realm Profile → Realm issuer key → Actor credential（Actor 长期 key 另行绑定）
```

Actr Facility Provider 只持有 Realm issuer key 私钥；只有受托成为 Realm 签名 key 持有方时（见 3.6）
才持有 Realm 签名 key。

管理权限分三类：

- 实例运营者：Loran 实例唯一的系统管理根，管理 Actr Facility Provider 配置与全部 Realm 生命周期，
  创建 Actr Facility Provider 域名下的 Realm 并签发其 owner 邀请（见 3.5）。
  Admin UI 配置只接受 Argon2id PHC hash，不接受明文密码；管理会话使用进程内 CSPRNG
  生成的 JWT key。
- Realm 签名 key 持有方：通过 Realm 管理请求创建 Realm、发布 Realm Profile（首次即激活，
  之后即更新或退役）与重签 owner 邀请，无需在 Loran 另开账号（Actr Facility Provider 域名下的
  Realm 由运营者代建，见 3.6）。
- owner：每个 Realm 恰好一个成员（见 4.1），由 owner 邀请产生、可转让；用自己的 Realm 客户端
  key 签 membership 请求，管理本 Realm 的成员（邀请、客户端撤销、移除、改角色、转让），同时是
  应用 owner。只作用于本 Realm，跨 Realm 请求一律拒绝，且不得通过错误信息泄露其他 Realm 是否存在。

Realm 客户端 key 是 Actr Host 能力，不是 Guest workload credential。Issuer 的注册入口同时要求
Realm 客户端 key 的持有证明与 package/linked 身份证明；Actrium/Rill 等 Actr Host 持有该 key，
但不得把它暴露给用户编写的 Rill Guest。注册请求携带的连接 ACL 因此是 Actr Host 管理行为，只作用于
所注册 Actor 自己的类型；该实例程序的 Actor 只有 owner 的客户端能注册（见 4.1），其他成员因此改不了
Service 的 ACL；无 ACL 时默认拒绝发现/中继。

## 3. Realm 创建与激活

### 3.1 流程

创建 Realm 不需要按 Realm 或按域名做 challenge 验证；授权来自 Realm 签名 key 持有方：

1. 调用方（Realm 签名 key 持有方的签名工具、实例运营者或 supervisor）以 3.2 的 Realm 管理
   请求提交 `create_realm`，其中携带该实例要运行的包 `package`（下载地址、SHA-256 与 Actor 类型）；
2. Loran 获取该域名的 authority descriptor（见 5.1）并验证 `realm_signing_keys`，再用
   其中的 key 验证请求签名；通过后创建 `pending` Realm，在 Issuer Signer 中生成该 Realm
   的 Realm issuer key，在响应中返回待签 Realm Profile payload（`package`、Realm issuer
   key、端点、`provider.id`）；
3. 调用方用 Realm 签名 key 签名 Realm Profile，**总是**以 `publish_profile` 请求（同一格式，
   携带签名 Realm Profile）提交给 Loran；
4. Loran 验证 Realm Profile 签名，并确认其中的 Realm issuer key 与 Actr Facility Provider 端点正是
   自己，Realm 转为 `active`。`publish_profile` 的响应即激活响应，一次性返回该 Realm 的
   owner 邀请（见 3.5）。若 Realm Profile 存放位置（`realms`）由本 Actr Facility Provider
   提供，Loran 同时对外提供该 Realm Profile；否则由调用方发布到自己声明的位置。

Actr Facility Provider 域名下的 Realm（3.6）由运营者经 admin API 创建，Loran 自己签 Realm Profile，
并激活，owner 邀请在创建响应中返回给运营者。

幂等与冲突：创建以 `(realm, 请求方)` 为幂等键，`realm` 为完整 Realm 名。持有方自己签名时，
请求方是 Actr 管理信封中可选的 `requester` 字段（持有方签名服务填写的不透明成员标识，缺省
为空）。同一请求方对自己的 pending 或 active Realm 重试时返回其状态，不返回 owner 邀请；
不同请求方命中同名 Realm，或任何请求命中 Actr Facility Provider tombstone，一律返回冲突。对已 active
的 Realm，`publish_profile` 只接受 sequence 更高的 Realm Profile，不再返回 owner 邀请；激活响应
丢失或 owner 邀请过期时用 `reissue_owner_invite` 重新签发。

只有 `active` Realm 才签发 Actor credential、接受 signaling 连接和分配 TURN。

`pending` Realm 有 TTL，过期清理。因为创建请求必须经 Realm 签名 key 持有方授权，他人无法
在本 Actr Facility Provider 上抢注某个 Realm 域名下的 RealmId。

谁来授权：

- 创建必须经该 Realm 域名的 Realm 签名 key 持有方授权。持有方自己签名时，按自己的策略
  认证请求方，再以 Actr 管理信封代签创建请求与 Realm Profile；Loran 不替持有方判断谁有权
  创建；
- 域名所有者可以委托某个 Actr Facility Provider 代持 Realm 签名 key（例如自建 Loran 时由自己的
  Actr Facility Provider 持有）；没有自己域名的个人由 Actr Facility Provider 在其域名下分配 Realm，此时
  该 Actr Facility Provider 就是持有方（见 3.6）。本机 Realm 不经过 Loran。
- 授权路径与信任根显示按同一可判定规则：在规范化 A-label 上比较，Realm 域名等于
  Actr Facility Provider ID 或以 `"." + Actr Facility Provider ID` 结尾（按 label 边界）时，为「Actr Facility
  Provider 域名下的 Realm」，由运营者创建（见 3.6），客户端显示信任根为该 Actr Facility
  Provider；其他情形一律走 Actr 管理信封，由持有方（域名所有者自己或受托代持的 Actr Facility
  Provider）的签名服务签发，客户端显示信任根为 Realm 域名。受托代持对客户端不可见（见
  THREAT_MODEL 残余风险）。

Loran 获取 authority descriptor 与 Realm Profile 时遵循 authority-discovery 的重定向规则：
最多跟随 5 次，每一跳必须是 HTTPS，并始终用原 authority 验证返回的 `authority` 绑定。
不另设比客户端更严的「一律不跟随」规则，避免同一 descriptor 对客户端有效而 Actr Facility
Provider 拒绝。作为服务端，Loran 在每一跳额外执行 SSRF 检查：拒绝内网/loopback/link-local
DNS 结果，把该跳请求钉住到预检查的公网地址（逐跳 DNS pinning），响应限长 64 KiB，
避免 SSRF 和 DNS rebinding。由于验签前就要外联，Loran 按来源与目标域名限流，并缓存
descriptor 获取结果（含失败）。

### 3.2 Realm 管理请求

v1 的 Realm 管理请求只有 `create_realm`、`publish_profile`、`reissue_owner_invite` 与 `draft_profile`，共用
Actr 管理信封（Actr management envelope），只有 `op` 与操作字段不同（Publisher 注册复用
同一格式，见第 7 节）。请求以 HTTPS `POST` 发往 Loran 自身 authority descriptor
（`https://<provider_id>/.well-known/actr`）中的 `realm-management` service link（protocol
`actr-realm-management/1`），只用于 Actr Facility Provider 域名以外的 Realm：

```json
{
  "op": "create_realm",
  "realm": "k7m2q9xh@acme.com",
  "package": { "href": "https://cdn.notes.example/team-notes-1.4.2.actr", "sha256": "<64 hex>",
               "type": "notes.example/team-notes@1.4.2" },
  "requester": "<可选，持有方的成员标识>",
  "provider_id": "provider.example",
  "nonce": "<random>",
  "issued_at": "<RFC 3339>",
  "expires_at": "<RFC 3339>"
}
```

- payload 按 RFC 8785 JCS 规范化，附 Realm 签名 key 的 detached Ed25519 签名与 `key_id`；
- `provider_id` 必须等于接收方的 Actr Facility Provider ID，即 Actr Facility Provider 的规范化 DNS
  authority，防止请求被重放到其他 Actr Facility Provider；
- `expires_at - issued_at` 不超过 5 分钟，Loran 拒绝已过期或尚未生效的请求；
- Loran 在有效期内记录 `nonce`，拒绝重复使用；nonce 只防字节级重放，重试靠 3.1 的
  幂等键；
- 签名 key 必须是 `realm_signing_keys` 中的 `active` key。

### 3.3 持续校验

Loran 以同一节奏周期性重新获取 authority descriptor（含 `realm_signing_keys`）与
Realm Profile。若已接受 Realm Profile 的签名 key 已从 `keys` 中移除（吊销），丢弃该 Profile 及其
`sequence` 基线，改用最后一份由仍列出的 key 签名的 Realm Profile；没有则按从未接受处理并
重新获取。出现以下任一情况时，本地挂起该 Realm，不再签发新 Actor credential：

- Realm Profile 的 `provider.id` 不是本 Actr Facility Provider，或未把本 Actr Facility Provider 当前签发用的
  Realm issuer key 列为 `active`；
- Realm Profile `status` 为 `retired`；
- 找不到由 `realm_signing_keys` 中仍列出的 key 签名的有效 Realm Profile；
- Realm Profile 或 `realm_signing_keys` 的 `sequence` 低于已接受的值（回滚）。

### 3.4 退役

退役只有一条路径：Realm 签名 key 持有方发布 `status = retired` 的新 Realm Profile（以
`publish_profile` 提交，并发布到 `realms` 位置；即退役
Realm Profile，永久有效；保持相同 `provider.id`，`sequence` 更高）。Loran 通过 3.3 的周期校验得知后，在单一事务中撤销
Actor credential、renewal token 与续期/重签 attempt 记录、未用的 Realm 邀请、presence、Actor Registry 记录与连接 ACL，退役 Realm issuer
key，并写入以 Realm 名为键的 Actr Facility Provider tombstone。若绑定的 Actr Facility Provider 已停服，
持有方把 `realms` href 改指自控位置，并在那里发布退役 Realm Profile。若持有方本身就是已
停服的 Actr Facility Provider，其域名下的 Realm 随之失效、无法退役，只能在别处新建。

### 3.5 owner 邀请的交付

Realm 的管理权以一次性 owner 邀请（Actr Realm Membership 第 5 节）交付，不设 Realm 管理 token：
持有方在激活响应中收到它（由其签名工具交给实例创建者，Actrium 新建流程中当场兑换）；Actr Facility
Provider 域名下的 Realm 与 Loran 不是 Actr Facility Provider 时创建的 Realm，由运营者在创建响应中收到，
经带外渠道交给用户。Loran 只保存邀请 secret 的哈希；owner 邀请 7 天后过期。

owner 丢失全部客户端时重新签发：持有方以 Actr 管理信封发出 `reissue_owner_invite`，或运营者调用
admin API；新的 owner 邀请使此前未用的失效，兑换后成为 owner 的新客户端，owner 原有 Realm 客户端
key 全部撤销（见 4.1）。

### 3.6 Actr Facility Provider 持有 Realm 签名 key 的情形

没有自己域名的个人可由 Actr Facility Provider 在其域名下分配 Realm，例如
`k7m2q9xh@realms.provider.example`。此时 Actr Facility Provider 是该域名的所有者和 Realm 签名 key
持有方，Loran 直接服务该域名的 authority descriptor 与 Realm Profile，信任根即该 Actr Facility
Provider；按 3.1 的可判定规则，客户端显示这一点。域名所有者也可以委托某个 Actr Facility Provider
代持其 Realm 签名 key；此时 Loran 以持有方签名服务的身份签发 Actr 管理信封，客户端仍显示
信任根为 Realm 域名（受托代持不可见）。未经委托，Loran 不代持第三方域名的 Realm 签名 key。

v1 中 Actr Facility Provider 域名下的 Realm 只由运营者经 admin API 创建（产品决定 Q1）；用户以运营者
交付的 owner 邀请加入。由个人以 Actr Facility Provider 账号（OAuth 2.0 + PKCE）自助创建、按账号计配额
属于后续能力（第 10 节）。Actr Facility Provider 知道自己为谁创建了哪些实例；实例成员看不到这一点，
Realm 客户端 key 仍按 Realm 独立。

Realm 域名等于 Actr Facility Provider ID 时（如 `k7m2q9xh@provider.example`），Actr Facility Provider 的
`/.well-known/actr` 同时承载它自己的 `services`（signaling、identity、ice-config、
realm-management 等）与该域名的 `realm_signing_keys`、`realms`，是同一份 descriptor；Realm
域名是其子域名时（如 `realms.provider.example`），两者各有一份 descriptor。

### 3.7 配额

按 Realm 域名与按 Realm 设配额：Realm 域名可创建的 Realm 数、每个 Realm 的成员数、每个成员的
客户端数与未用邀请数、连接数、注册速率、signaling 消息速率、TURN allocation 数与带宽。`register_publisher` 可登记的 Publisher
总数另受 `[realm] max_publishers` 限制（默认 1000，超出 `429`）。

## 4. 身份模型

ActorId 是 Actor 实例身份，不是用户、Workspace、程序或逻辑服务名，只含两部分：

```text
<serial_hex>.<realm_id>@<domain>        例：2af01.k7m2q9xh@acme.com
```

- `serial_hex`：该 Realm 的 Issuer 分配的 54-bit Snowflake serial，小写十六进制；
- `realm-name`：完整 Realm 名。

程序身份（Actor 类型）`publisher/program@version` 不在 ActorId 中。它由注册请求声明、
经 package/linked 身份证明验证后记入 Actor Registry，并写入 Actor credential；连接 ACL、
发现与版本校验都使用注册与 Actor credential 中的类型，不读取 ActorId。legacy 形式
`<serial_hex>@<realm_id>/<type>` 导入时必须显式补足 Realm 名并重写，不做推断。

Loran 不从 RealmId 推断任何语义，也不在对外接口中泄露同一域名下其他 Realm 是否存在。

每个客户端安装在每个 Realm 使用独立的 Realm 客户端 key，不跨 Realm 复用；Realm 客户端
注册按 Realm 分别进行，Loran 无法据 Realm 客户端 key 关联同一客户端加入的不同 Realm。
没有单独的 membership credential：客户端凭 Realm 邀请完成 Realm 客户端注册后，Loran 记录其
Realm 客户端 key，这就是 Realm membership（见 4.1）。

数据库直接以完整 Realm 名为键：它唯一、不可变、退役后不复用，不需要另一个数字。

Worker ID 为显式配置的 0..31 值，不再由 PID/hostname 的 5-bit hash 猜测。单节点
默认 0；未来并发 Issuer 必须由部署租约分配不同 worker ID。SQLite 的
`issued_actor_identity` 永久登记本实例已签发的 `(Realm, serial)`；进程重启或时钟
回拨导致 Snowflake 候选碰撞时只会继续分配，绝不覆盖原身份。跨 package version
升级视为新 Actor：持有方发布 `package` 已更新、sequence 更高的 Realm Profile，新的
Service 仍在同一 Realm 内，Realm 链接不变。

Service 可用 renewal possession proof 在重启后保持 ActorId；Tab Actor 的新建策略属于
Actrium。Tab 与 Service 不由基础设施强制 1:1，同一 Service 允许多个 Tab/窗口连接和重连。

实例的 Service 就是本 Realm 内 Actor 类型与 Realm Profile 所指包相同的 Actor。Front 通过
Signaling 的发现在本 Realm 内按该类型找到它；Actor Registry 不另设入口角色。

### 4.1 成员与 Realm 客户端 key

协议以 `actr/protocols/realm-membership` 为准，Loran 的实现约定：

- 记录：成员（`member_id`、角色 owner/editor/viewer、active/removed）、Realm 客户端 key（`key_id` =
  公钥 SHA-256 前 16 hex、公钥、所属成员、客户端名称、登记 / 最近使用 / 撤销时间）、邀请（`invite_id`、
  secret 的 SHA-256、角色或目标成员、签发者、过期 / 使用 / 撤销时间）。每个 Realm 至多一个 active
  owner，由部分唯一索引保证。
- membership 请求：`POST /issuer/membership`（即 Realm Profile 的 `issuer.url` 加 `/membership`），
  Actr 管理信封，签名 key 为成员的 Realm 客户端 key，与 Realm 管理请求共用 nonce 记录；兑换、撤销、
  移除、转让各在一个 `BEGIN IMMEDIATE` 事务中完成。信封的 `provider_id` 为本 Actr Facility Provider ID；
  没有 Realm Profile 的 Realm（Loran 不是 Actr Facility Provider 时创建，开发与 e2e）也接受 Issuer URL 的
  authority（Loran 以请求的 `Host` 比较）。
- 申请 Actor credential：`/issuer/register`、`/issuer/reissue`、`/issuer/renew` 都须带
  `x-actr-client-proof`（`op` 为对应入口、含请求体 SHA-256 的 Actr 管理信封）。Issuer 只对已登记、
  未撤销、属于 active 成员的 key 签发；续期须用该 Actor 当前绑定的 key，重签可用同一成员的任一未撤销
  key 并把 Actor 改绑过去；注册时，Actor 类型的 publisher 与程序名等于 Realm Profile `package.type` 的
  （即 Service 及其升级版本）只接受 owner 的客户端；提交事务内重查 key 与成员状态，续期另查 Actor 仍绑定
  这把 key（同成员的重签可能已把它改绑）。
  `issued_actor_identity` 记录每个 Actor 的成员与当前绑定的 key。
- 撤销 key（客户端撤销、移除成员、离开、owner 恢复）：同一事务内撤销 key 及它签发的未用邀请；
  绑定到它的 Actor 停用而不作废：不写 `revoked_actor`，renewal token 也保留，因为同一成员的另一把
  key 正是凭它 `/reissue` 取回 ActorId 并改绑（续期要求绑定 key，已撤销的 key 无法续期）。提交后经
  进程内通知，Signaling 以 close code 1008（`client revoked`）关闭这些 Actor 的连接并删除其 Actor
  Registry 记录与 presence 订阅，TURN 关闭其 allocation；Signaling upgrade、Issuer 签发（TURN
  credential 只随 Actor credential 签发）与 TURN 认证都拒绝「在 `revoked_actor` 中，或绑定 key 已撤销」
  的 Actor；Signaling upgrade 另拒绝 claims 的 `client_key` 不是该 Actor 当前绑定 key 的 credential
  （重签已把 Actor 改绑，旧 key 此后被撤销也不会连带停用它）。连接与 allocation 都在内存中，重启后
  必须重建并经过上述检查，因此无需重做。已签发 credential 在 Loran 之外到期失效（`token_ttl_secs`，
  默认 3600）；改绑前旧 key 取得的 TURN credential 与已建立的连接同样以此为界。
- 改角色只改记录；角色在下一份 credential 中生效。
- `update_realm` 只用于 Actr Facility Provider 域名下的 Realm：owner 更新 `package`、`display_name` 或退役，
  Loran 走与运营者 `UpdateRealm`、`…/retire` 相同的自签路径。其他 Realm 一律 `forbidden`。

## 5. Realm 签名 key 与 Realm Profile

### 5.1 authority descriptor 中的 Realm 信息

Realm 域名不另设发现文件，而是复用该域名的 Actr authority descriptor：

```text
GET https://<domain>/.well-known/actr
```

获取与校验规则以 `actr/protocols/authority-discovery` 为准。按该协议，`/.well-known/actr`
下不定义任何子路径，因此 Realm 相关信息全部以 descriptor 成员与 service relation 表达
（Actr Authority Discovery §4.3–4.4）：

- 可选成员 `realm_signing_keys`：Realm 签名 key（当前与历史，含 `key_id`）、轮换签名链、
  单调递增的 `sequence` 与 `signature`。`signature` 由某个 `active` key 对该成员（去掉
  `signature` 字段后按 RFC 8785 JCS 规范化）签名，覆盖 `sequence`，为已固定 key 的客户端
  和 Loran 提供回滚保护；authority descriptor 其余部分仍是无签名的公开文档；
- core relation `realms`（protocol `actr-realm-profile/1`），`href` 指向 Realm Profile
  服务；
- descriptor 中的 `signaling`、`identity`、`ice-config` 等 service 服务于该 authority
  自身，与该域名下的 Realm 无关。访问某个 Realm 只使用其 Realm Profile 指定的 Actr Facility
  Provider：取不到、验不过或已退役的 Realm Profile 时，客户端不得使用 Realm Profile 以外的
  任何 Actr Facility Provider。创建时选哪个 Actr Facility Provider 由 Actrium、用户或持有方策略决定。

协议对象字段一律使用 snake_case（如 `realm_signing_keys`、`display_name`、`public_key`、
`key_id`、`credential_audience`）。

Realm 签名 key 轮换时，新 key 由旧 key 签名，形成 Realm 签名 key 轮换链（轮换项 payload 为
`{from, to, public_key, sequence}`）；接受规则是 Actr Authority Discovery §4.4 的六步算法：
Loran 为每个域名保存曾接受的每把 key（公钥与最后状态 `active`/`retired`/已移除）及已接受的
`sequence`（`realm_domain.pins_json`），同 id 不得换公钥、`retired` 不得复活、已移除不得复现，
新 key 只经 `sequence` 更高的轮换项引入。没有有效轮换链的 key 变化按身份变化处理：签名 key
不在已固定的链中时不做 `sequence` 比较，直接要求重新确认信任，确认后以新成员的
`sequence` 为基线。重新确认只由运营者经 admin 接口手动完成（核对 pin 与当前 key 的指纹，见
API「身份变化的重新确认」），Loran 从不自动接受。authority descriptor 不列出 Realm 清单，避免枚举。

### 5.2 Realm Profile

每个 Realm 一份，位置为：

```text
GET <realms href>/<realm_id>
```

`realms` 的 `href` 可以指向 Realm 域名自己的静态存放位置（例如
`https://acme.com/actr/realms`），也可以指向 Actr Facility Provider（例如
`https://realms.provider.example/acme.com`，可带显式端口）；Realm Profile 也可以随 Realm
邀请内嵌。Realm Profile 由 Realm 签名 key 签名，因此存放它的一方不成为信任根。`href` 不得
含 query/fragment，拼接前去掉尾部 `/`；多条 `realms` link 按出现顺序尝试，任一返回有效
Realm Profile 即止；所有 link 均未产出有效 Realm Profile（404、其他错误、超时或无效文档）
即视为不可用。

schema 是 `actr.realm/v1`，只包含非秘密信息，至少声明：

- `realm.name`（完整 Realm 名）、`display_name`；
- `status`（`active` | `retired`）；
- 单调递增的 `sequence` 与 `issued_at`（Realm Profile 不设过期时间）；
- issuer：1 至 3 把 Realm issuer key（`id` 为 Actor credential 头部 `key_id` 的十进制串，即
  Signer 的数字 key id）、注册和验签端点；
- provider：Actr Facility Provider ID、signaling/STUN/TURN 端点、`credential_audience`；
- 加入策略。

Realm Profile payload 用 RFC 8785 JCS 规范化，附 `active` Realm 签名 key 的 detached Ed25519
签名；由后来退役的 key 签发的既有 Realm Profile 在被更高 `sequence` 取代前仍有效，但签名 key
被从 `keys` 移除时立即失效（见 3.3）。Realm Profile 不过期，回滚保护只靠每个 Realm 单调递增的
Realm Profile sequence（相同 `sequence` 内容必须一致）；`provider.id` 与已接受 Realm
Profile 不同的新 Realm Profile 一律拒绝。退役 Realm Profile 永久有效。

### 5.3 Actor credential 与 TURN 约束

Actr Facility Provider ID 是 Actr Facility Provider 的规范化 DNS authority（例如 `provider.example`），Loran 取
`[authority] uri` 的域名。Actor credential 必须同时满足：

- `protocol_version = 1`
- `issuer = <realm-name>`，例如 `k7m2q9xh@acme.com`
- `provider = <Actr Facility Provider ID>`
- `audience = actr.signaling`
- 携带 Actor 类型 `publisher/program@version`（见第 4 节）
- 携带成员 id、Realm 客户端 key id 与签发时的角色（见 4.1）

每个 Realm 的 issuer key 按 Realm Profile §6.1 轮换（「先发布、再使用、后移除」）：新 key 先以
`active` 列入 Realm Profile，列出它的 Realm Profile 被接受满 24 小时后才开始签发（其间新接受的
Realm Profile 不再列出它则重新计时），旧 key 以 `retired` 继续列出，直到它签发的最后一个
credential 过期再过 5 分钟才移除（此时在 Signer 中退役）。Actr Facility Provider 域名下的 Realm 每 90 天自动轮换，Loran 自行签新 Realm Profile；其他
域名的 Realm 由运营者经 admin API 发起，持有方用 `draft_profile` 取得含新 key 的草案、签名后
`publish_profile`，发布之前 Loran 继续用旧 key。签名只用当前 key；验证者（signaling、TURN）
按 `(realm, key_id)` 查 key，退役 key 的缓存项到移除时刻失效。

TURN 使用单一端口池。TURN credential 的 HMAC username 同时绑定 Realm 名、Actr Facility Provider、
`actr.turn` audience 和 ActorId；TURN secret 按 Realm/Actr Facility Provider 隔离，由 Actr Facility
Provider 主密钥经 HKDF 以 Realm 名和 Actr Facility Provider ID 派生。第三方 Actr Facility Provider 在 v1
仅有 Realm Profile 数据契约，没有 federation、跨 Actr Facility Provider 委托或信任协商协议。

## 6. 多 Realm 隔离

同一进程承载多个 Realm 时，隔离不能依赖进程边界，必须由以下机制显式保证：

| 风险 | 控制 |
| --- | --- |
| Actor credential 跨 Realm 重放 | claims 绑定 issuer（即 Realm 名）/Actr Facility Provider/audience；signaling 以连接绑定的 Realm 校验每条消息 |
| 缓存串 Realm | signaling presence、Actor Registry、TURN authenticator cache、renewal token、连接 ACL 的 key 均包含 Realm；禁止仅以 Actor serial 或类型做 key |
| Discovery 越界 | Discovery 与 RouteCandidates 只返回同 Realm 的 Actor；v1 不做跨 Realm 发现 |
| 管理越权 | Realm 管理请求只作用于签名 key 所属 Realm 域名；membership 请求的签名 key 只在其所属 Realm 内查找，角色逐请求检查；列表接口只返回作用域内的 Realm |
| 成员越权 | 该实例程序（`package.type` 的 publisher 与程序名）的 Actor 只接受 owner 客户端注册；注册携带的 ACL 只作用于所注册 Actor 自己的类型；角色检查先于目标查找；owner 身份只经 owner 邀请或转让产生 |
| 未激活或已挂起的 Realm 被使用 | 只有 `active` Realm 签发 Actor credential、接受连接与分配 TURN |
| 删除后残留 | 退役在单一事务中撤销 Actor credential、renewal token 与续期/重签 attempt 记录、未用的 Realm 邀请、presence、Actor Registry 记录与连接 ACL，退役 Realm issuer key，并写入以 Realm 名为键的 Actr Facility Provider tombstone |
| 资源挤占 | 按 Realm 域名与按 Realm 配额 |
| 进程失陷影响全部 Realm | 首版接受 all-in-one 部署的此项风险；生产建议把 Issuer Signer 放入独立安全边界，使主进程失陷不泄露 Realm issuer key 私钥 |

跨 Realm 混淆必须有专门的回归测试：两个 Realm 使用相同 Actor 类型、相同 Actor serial
候选时，Actor credential、presence、TURN、连接 ACL 与管理接口均不得互通。

v1 的连接 ACL 只作用于同一 Realm 内的 Actor 类型。ACL 数据模型保留 `source_realm`
维度供后续跨 Realm 规则使用；v1 管理接口拒绝非空的 `source_realm`。实例内的应用权限
（读写、管理、分享）由应用执行；Loran 只保存成员角色并写入 Actor credential。

## 7. Publisher 发现

Publisher Profile 是 `https://<publisher>/.well-known/actr` 中带签名的可选成员
`publisher`，schema 为 `actr.publisher/v1`，字段使用 snake_case，内容以产品规范
（`docs/product/actrium-v1` 05 §2.2、12 §2）为准：`domain` 必须等于 descriptor
`authority` 的主机名，Publisher Profile 签名 key 与 package signing key 分离，带轮换链
（`rotations`）、
`sequence` 与有效期。成员的有效性只取决于自身签名与已固定的
key history。

Loran 获取 descriptor 的重定向与 SSRF 规则与 3.1 相同。`register_publisher` 把一个
publisher 登记到本 Actr Facility Provider 的 Publisher Registry（用于验证该
Publisher 的包签名，包文件托管独立配置），与 Realm 管理请求共用同一端点 `POST /actr/realm-management`。它复用
3.2 的 Actr 管理信封：以 `publisher` 字段取代 `realm`，由 Publisher Profile 中 `active` 的
Publisher Profile 签名 key 签名，同样绑定 `provider_id`、5 分钟有效期与 nonce；公开文档中
不放 challenge。首个 Publisher Profile 固定其 `profile_signing_keys`；之后按与 `realm_signing_keys`
相同的 §4.4 算法接受（`publisher.profile_pins`），身份变化拒绝，直到运营者重新确认。包签名 key 的 `id` 须为
`actr-pub-<SHA-256 前 16 hex>`；Publisher Profile 列 1 至 2 把 `active` 包签名 key，两把都可
验证包签名（轮换重叠期），原当前 key 只要仍 `active` 就保持当前；不再 `active` 的 key
归档为 retired，只用于历史包验签；Publisher Profile 不再列出的 key 即吊销（revoked），泄露时
也可由实例运营者显式 revoke。Loran 不周期刷新 Publisher Profile，key 轮换由 publisher 再次
发送 `register_publisher` 完成（见第 9 节）。

程序身份写作 `publisher/program@version`。实例运行哪个包由 Realm Profile 的 `package`
（下载地址、SHA-256 与 Actor 类型）给出，包可以放在任何 HTTPS 位置；消费端必须核对 SHA-256，并验证
manifest、publisher key 签名和 key revocation 状态。

包托管：`loran-packages` 只是按内容寻址托管 `.actr` 文件的静态下载服务，
`package.href` 可以指向它，也可以指向 Publisher 自己的 CDN。它不把 Target 解析为包：
Package Resolution、`packages` relation 与 `program_resolver` 已从协议和代码中删除。

## 8. 数据与升级

当前 schema v10 使用 SQLite `PRAGMA user_version = 10`。Loran 遇到旧版或未知 schema
直接拒绝启动，不在启动路径中执行「尝试 ALTER 后忽略错误」。v2 → v3（新增
`issuer_control_attempt`）、v3 → v4（以 Realm 名取代数值 Realm id）与 v4 → v5（去掉单
Realm 约束，`realm.status` 改为 `pending | active | retired`，新增 `realm_domain`、
`realm_profile`、`realm_mgmt_nonce`）与 v5 → v6（删除 `publisher_challenge`，
`publisher.descriptor_json` 改为 `profile_json`）与 v6 → v7（删除
`issuer_reissue_response_cache` 与 `issuer_renewal_token.delivered_at`）与 v7 → v8（`realm_domain`
改存 key pins，新增 `realm_issuer_key`，`publisher` 新增第二把 `active` 包签名 key 与
`profile_pins`，删除未用的 `realm.issuer_key_id`）与 v8 → v9（新增 `realm_member`、
`realm_client_key`、`realm_invite`，`issued_actor_identity` 新增 `member_id`、`client_key_id`，`realm`
删除 Realm 注册 secret 与 Realm 管理 token 列）与 v9 → v10（移除包发布记录表
`publisher_package` 与发布 nonce 表 `publisher_publish_nonce`）都没有离线迁移，需停机后重建数据库。
已删除的全局 Issuer key 曾存于数据目录下的 `issuer_keys.db`（更早为 `ais_keys.db`）；Issuer 启动时若发现该文件（及其
`-wal`、`-shm`）即删除并记日志。Issuer Signer 的 `signer_keys.db` 打开时删除没有名字的 key
（已删除的 `GenerateSigningKey` 留下的），其余 key 保留。

v4 已以 Realm 名为键：`realm` 表以列 `name`（完整 Realm 名）为主键，`actoracl`、
`pending_registration`、`issued_actor_identity`、`revoked_actor` 以 `realm` 列引用它，后三者按
`(realm, serial_number)` 建主键；Signaling 的 `actor_registry` 同样按 Realm 名与 serial 建键。

表结构：

- `realm_domain`：`domain`、已接受的 `realm_signing_keys` 成员原文、key pins（每把曾接受的
  key 的公钥与最后状态、已接受的 `sequence` 与内容 digest）、固定与最近校验时间；
- `realm`：以完整 Realm 名（列 `name`）为主键、状态（`pending` | `active` | `retired`，
  实例运营者的本地挂起用独立的 `enabled` 开关）、pending 到期时间、请求方（`requester`）；Actr Facility
  Provider tombstone 与活跃 Realm 共用该主键约束；
- `realm_member`、`realm_client_key`、`realm_invite`：成员、Realm 客户端 key 与邀请（见 4.1）；
  退役删除邀请，成员与 key 随 tombstone 保留；
- `issued_actor_identity`：已签发 ActorId 的永久登记，含 Actor 类型、包 target、成员与当前绑定的
  Realm 客户端 key；
- `realm_profile`：每个 Realm 已接受的最新 Realm Profile 原文及其 digest 与 `sequence`；
- `realm_issuer_key`：每个 Realm 的 issuer key（Signer key id 与名字、公钥、`next` | `current` |
  `retired`、被已接受 Realm Profile 连续列出的起始时间、成为当前的时间、可移除时间）；
- Realm 管理请求、membership 请求与客户端证明的 `nonce` 记录，保留到对应请求过期；
- 其余按 Realm 分区的表（连接 ACL、已签发身份登记、Actor Registry、renewal state）以
  `realm` 为键。

Actor Registry 把持久元数据与在线状态分开：注册在 SQLite 写入完成后才成功；
Loran 重启恢复的记录可继续提供 ServiceSpec，但在原 Actor 认证重连前不能进入
Discovery 或 RouteCandidates。显式注销则先原子删除该 Actor 的缓存服务，再返回
成功，因而紧接着重启也不能复活已注销记录。

Actor Registry 的持久元数据在 Signaling 自己的缓存库 `signaling_cache.db` 中，不在上述 schema
版本内；遇到旧表结构直接丢弃，由 Actor 重新注册。

## 9. 与当前实现的差异

当前代码已承载多个 Realm，以下差异仍需跟进：

- 已实现：authority descriptor 获取（逐跳 SSRF 检查、最多 5 次重定向、64 KiB、缓存含失败、按
  主机限流）、`realm_signing_keys` 成员自签名验证与按 key pin 的接受（Actr Authority Discovery
  §4.4 六步算法，跟随轮换链，身份变化拒绝；以 `contracts/actr/test-vectors/realm-signing-keys-rotation.json`
  逐条测试）、Actr Facility Provider 域名下 Realm 的 Realm Profile 签发、`realm_profile`
  存储与 `GET /actr/realms/{domain}/{realm_id}`、其他域名 Realm 的周期性刷新（含 key 移除后丢弃
  基线、取不到则停用）、退役 Realm Profile 触发的退役事务、身份变化的重新确认（运营者经
  admin 接口核对 pin 与当前 key 的指纹后手动接受，带审计记录；不自动接受）。尚未实现：Loran
  自己的 Realm 签名 key（`realm-signing:<domain>`）的轮换；
- 已实现其他域名的 Realm 管理请求 `create_realm`、`publish_profile`、`reissue_owner_invite`、
  `draft_profile`（`provider_id` 绑定、5 分钟有效期、nonce 防重放、`(realm, 请求方)` 幂等、待签 Realm Profile）、
  `[realm]` 按域名配额、`pending` TTL 清理；
- 已实现 membership（4.1、3.5，`actr/protocols/realm-membership`，schema v9）：一次性邀请与 owner
  邀请、每客户端 Realm 客户端 key、`POST /issuer/membership` 的全部操作、`/issuer/register`、
  `/reissue`、`/renew` 的 `x-actr-client-proof`（含 `body_sha256`），credential 带成员、key 与角色，
  Service 程序只由 owner 注册；撤销 key 即时停用其 Actor（Signaling 断开并拒绝重连、TURN 关闭
  allocation 并拒绝认证）；运营者 `…/owner-invite` 与只读 `…/members`。共享 Realm 注册 secret、Realm
  管理 token 及其 admin 路由与认证已删除。Actr Host 以 `deployment.realm_client_key`（`actr realm join`
  写出的 key 文件，含 `realm`、`provider_id`、`issuer`、`key_id` 与密钥，权限 0600；Unix 上组或其他用户
  可访问的 key 文件被拒绝加载，提示 `chmod 600`，Android、iOS 除外）为每个
  `/register`、`/reissue`、`/renew` 请求新签证明；`actr realm` 提供 `join`、`invite`、`members`、
  `revoke-invite`、`revoke-client`、`remove-member`、`set-role`、`transfer-owner`、`leave`（请求有连接与总
  超时、不跟随重定向）。`join` 不给 `--issuer` 时取 `https://<domain>/.well-known/actr` 的
  `realm_signing_keys`，从 `realms` 位置取 Realm Profile 并验证（公开文档只走 HTTPS、上限 64 KiB），用其 `issuer.url` 与 `provider.id`；邀请内嵌的
  Realm Profile 只在同样验证通过时使用，两份都有效时取 sequence 高者（须为合法后继）；`--issuer` 留给没有
  Realm Profile 的 Realm。脚本以 stdin（`-`）传邀请。Admin Web 有每个 Realm 的只读成员列表。
  e2e `package-runtime-echo` 走 owner 邀请 → 邀请 editor → 移除后拒签的全流程。尚未实现：Web（sw-host）
  客户端的 Realm 客户端 key；Actr Host 下载 Realm 包后核对 `package.type`；Service 侧即时得知成员变化
  （现以 credential 到期为界）；
- 已实现运行时分区：Actor credential 由该 Realm 的当前 issuer key 签发（首把为 Signer 中的
  `issuer:<realm>`，随首份 Realm Profile 草稿或运营者创建 `active` Realm 时生成，缺失时签发前
  补建），验证 key 按 `(realm, key_id)` 缓存；signaling 连接绑定其身份的 Realm，source/target 不在该 Realm 的
  envelope 返回 `403`；Actor Registry 与 presence 按 Realm 分区；TURN secret 由主密钥按 Realm
  HKDF 派生；注册拒绝指向其他 Realm 的 ACL `source_realm`；跨 Realm 混淆回归测试
  `realms_with_the_same_actor_type_and_serial_are_isolated`。Issuer 原有的全局轮换 key 及其
  `/issuer/current-key`、`/issuer/signing-pubkey`、admin 与 gRPC 轮换入口已删除。退役事务提交
  后，Signaling 删除该 Realm 的 Actor Registry 记录与 presence 订阅并关闭其在线连接，Issuer 在
  Signer 中退役该 Realm 的全部 issuer key（私钥清除，此后不再签名）；两步幂等、重启时重做，并发的连接与
  注册要么被清理、要么被拒绝后自行关闭；
- 已实现 Realm issuer key 轮换（Realm Profile §6.1，见 5.3）：预发布 24 小时、切换、旧 key
  在最后 credential 过期 +5 分钟后移除并在 Signer 中退役；Actr Facility Provider 域名下的 Realm 每 90 天
  自动轮换；运营者入口 `POST /admin/api/realms/{realm}/issuer-key/rotate`，持有方工具
  `actr realm draft-request`；并发轮换或与 Realm 退役竞态时未能列出的新 key 当即在 Signer 中
  退役。Issuer Signer 的 key 均为具名 key、不按时间过期（旧的过期 key 机制及
  `tolerance_seconds`、`key_ttl_seconds` 已删除）。尚未实现：泄露时的立即移除入口（目前只能退役 Realm）、验证者遇到
  未知 `key_id` 时按 Realm 限流刷新（Loran 内的验证者与 Issuer 同进程共享 key 缓存，不需要）；
- 已实现 `register_publisher`：读取 authority descriptor 的 `publisher` 成员（第 7 节），
  `/.well-known/actr/publisher.json` 与 challenge 已移除；Publisher Profile 签名 key 按 key pin
  跟随轮换链，身份变化由运营者经 admin 接口重新确认；包签名 key id 由公钥推导，轮换重叠期两把
  `active` key 均可验证包签名。尚未实现：Publisher Profile 的周期刷新（过期或 key 移除后不自动
  停用该 publisher）；已登记 Publisher 总数受 `[realm] max_publishers` 限制（默认 1000）；
- 已实现 `/renew`、`/reissue` 加固：`/renew` 先只读校验 renewal token 再报告 Realm 状态；后继已
  生成的 token 只在 5 分钟内可 `/renew`，且只换回该后继；两者的提交事务内重查 Actor 撤销、Realm
  状态与 publisher 停用；Realm 退役同时删除其 attempt 记录；
- 按 Realm 的连接、注册、消息与 TURN 配额（TURN 认证已拒绝不可用 Realm 的 credential，Realm
  停用、过期或退役时关闭其已建立的 allocation）；
- 控制面与本契约的剩余差异：Loran 不是 Actr Facility Provider（未启用 authority 或 Issuer 无 Signer）时，
  运营者仍可创建任意域名下没有 Realm Profile 的 `active` Realm，并可经 `RetireRealm` 与
  admin `…/retire` 直接退役。

## 10. 明确不做

- 多节点 HA、自动 worker lease 或跨节点 SQLite；
- Realm federation、跨 Realm 发现与调用、跨 Realm ActorId 迁移；
- 第三方 Actr Facility Provider 之间的 signaling/TURN 委托协议；
- 未经域名所有者委托代持其 Realm 签名 key；
- 由 Realm 签名 key 签发的受限创建委派 key（限定 RealmId 前缀、配额与有效期）；
- Realm 域名级 SSO 或成员目录；
- Actr Facility Provider 账号（OAuth 2.0 + PKCE）、Actr Facility Provider 域名下的自助创建与按账号配额（后续）；
- 一个 Realm 多个 owner 或管理员委派（后续）；
- 更换 Actr Facility Provider（Realm 与 Actr Facility Provider 永久绑定，这是模型而非首版限制）；
- Workspace 生命周期或 Workspace 密钥托管；
- 旧 Actor credential、ActorId、publisher 身份或协议的双轨兼容。

这些能力若进入后续版本，应先扩展 authority descriptor/Realm Profile/protocol version，不得
削弱 v1 的 Realm 隔离不变量或默认拒绝策略。
