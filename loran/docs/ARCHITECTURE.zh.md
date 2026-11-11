# Loran Realm v1 架构

本页是代码导航；规范性设计以
[`REALM_V1_ARCHITECTURE.zh.md`](REALM_V1_ARCHITECTURE.zh.md)为准。

## 边界

一个 Loran 实例是共享 Actr Facility Provider（AFP），承载来自多个 Realm 域名的多个 Realm；Realm 对应
一次应用实例部署，以 Realm 名 `<realm_id>@<domain>`（如 `k7m2q9xh@acme.com`）标识，以
该 Realm 域名的 Realm 签名 key 为长期信任根。v1 是单节点、SQLite、单一 Actr Facility Provider，
不提供 HA 或跨 Realm federation。管理权限分实例运营者、Realm 签名 key 持有方发起的 Realm
管理请求、成员用 Realm 客户端 key 签名的 membership 请求（按角色，owner 管理成员）三类；尚未
落地的部分见
[`REALM_V1_ARCHITECTURE.zh.md`](REALM_V1_ARCHITECTURE.zh.md) 第 9 节。
历史名称见[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」。

```text
受信 Actr Host（Actrium/Rill）
        │ Realm registration capability
        ▼
┌──────────────────── 单个 Loran 进程 ─────────────────────┐
│ Issuer ──► Issuer Signer                                 │
│       │ Actor credential / TURN credential              │
│       ├────────► Rendezvous: Signaling                   │
│       └────────► Relay: STUN / TURN                      │
│ Publisher identity: Publisher Registry                         │
│ Management: Admin UI/API + Realm 管理请求 + Realm 生命周期 │
└────────────────────────┬─────────────────────────────────┘
                         ▼
                    SQLite schema v10
```

这些是逻辑平面，不要求永远同进程。v1 默认 all-in-one 是为了减少部署依赖；Issuer
Signer 可在需要时通过 TLS/mTLS gRPC（`signer.v1`）独立隔离。平面拆分不能绕过 issuer、
Actr Facility Provider、audience 与 endpoint scope。

## 身份与信任

- 每个 Realm 的 Realm issuer key（Ed25519）签发短期 Actor credential；Realm issuer key 由
  Realm 签名 key 签名的 Realm Profile 委派，Loran 未经委托不持有第三方域名的 Realm 签名 key。
- ActorId 表示实例身份，形式为 `<serial_hex>.<realm_id>@<domain>`，不含程序类型；
  程序类型由注册与 Actor credential 携带。Actor credential 的 issuer 即 Realm 名。
- Service 通过 renewal possession proof 在重启后保留 ID。
- 程序身份规范写作 `publisher/program@version`。
- Publisher 是规范化 DNS 域名，其 Publisher Profile 是 authority descriptor
  `https://<publisher>/.well-known/actr` 中带签名的可选成员 `publisher`。
- HTTPS 证明域名控制与安全传输；长期包真实性由 publisher key、signature、
  digest 和 revocation 状态决定。
- Signaling 只做已认证 Actor 的 rendezvous，不签发身份，也不是信任根。
- TURN credential 绑定 Realm、Actr Facility Provider、`actr.turn` audience、Actor 与过期时间；
  TURN secret 由主密钥按 Realm 名与 Actr Facility Provider ID 经 HKDF 派生。

注册 Actor 的能力是成员在每个客户端登记的 Realm 客户端 key，只属于可信 Actr Host（见架构契约
4.1）。用户编写的 Rill Guest 不得接触 Realm 客户端 key、Issuer Signer 私钥或管理 credential。

## 数据所有权

SQLite 持久化：

- Realm：多个 Realm 及其 Realm 名、Realm Profile 与状态；成员、Realm 客户端 key 与邀请（邀请只存
  secret 的哈希）；
- Issuer Signer key material（生产时由 KEK 加密，`LORAN_SIGNER_KEK`）；
- 已签发 Actor 身份登记与 renewal 状态；
- 连接 ACL、Publisher Profile/key/package metadata；
- 配置 override 与审计需要的 nonce。

Workspace 不属于 Loran。一个 Service 至多绑定一个 Workspace，同一 Workspace 可按
写入并发策略被多个 Service 使用；关闭或删除 Actor 不删除目录；Workspace 加密密钥属于用户。这些规则由 Actrium 实现。

## 请求流

### Realm 创建

1. 调用方提交经 Realm 签名 key 持有方授权的 `create_realm` 请求（Actr 管理信封）。
2. Loran 按 authority-discovery 获取 `https://<domain>/.well-known/actr` 中的
   `realm_signing_keys` 并验签，创建 `pending` Realm，在 Issuer Signer 生成 Realm
   issuer key，返回待签 Realm Profile payload。
3. 调用方签名 Realm Profile，以 `publish_profile` 提交；Loran 验证 Realm Profile 指向自己
   后转为 `active`，在响应中一次性返回 owner 邀请，此后才签发 Actor credential。
   Actr Facility Provider 域名下的 Realm 由 Loran 自签、首份 Realm Profile 在 `create_realm` 响应中
   直接返回，无需 `publish_profile` 这一步；v1 只由实例运营者经 Admin API 创建（Actr Facility Provider
   账号自助创建属后续能力），owner 邀请同样在创建响应中一次性返回。
   非 Actr Facility Provider 部署（未启用 authority 或 Issuer 无 Signer）仍允许运营者直接创建任意
   域名下没有 Realm Profile 的 `active` Realm。
4. 数据库以 Realm 名（含域名）为主键，`realm_name_immutable`/`realm_retired_is_final`
   触发器保证名字不可变、退役行不复活；`[realm]` 配置按域名与 pending 数量设配额。

### Actor 注册

1. 可信 Actr Host 提交 Realm capability 与 package/linked workload 身份证明。
2. Issuer 验证 Realm（须为 `active`）、publisher/package、nonce、连接 ACL 和
   注册模式。
3. Issuer 从持久的已签发身份登记预留 Snowflake serial。
4. Issuer Signer 签发 Actor credential（携带 Actor 类型）；Issuer 同时生成 scoped
   TURN credential。
5. Guest 只获得运行所需的最小能力。

### Signaling

1. 客户端在 WebSocket upgrade headers 或受约束 subprotocol transport 中传身份。
2. 服务验证 Actor credential 的签名、期限、issuer/Actr Facility Provider/audience 与连接 Actor。
3. envelope source 必须与连接身份一致，relay 的 target 须属于连接绑定的 Realm。
4. 连接 ACL 默认拒绝；通过后才能发现或转发信令；发现只返回同 Realm 的 Actor。

### Publisher 与包

1. Publisher 在 authority descriptor `/.well-known/actr` 的 `publisher` 成员发布签名 Publisher
   Profile，并以 Publisher Profile 签名 key 签名的 `register_publisher` 请求（Actr 管理信封）
   在 Actr Facility Provider 登记；公开文档中不放 challenge。首次接受的 Publisher Profile 固定其签名
   key，之后只接受由已固定 key 签名、`sequence` 更高的 Publisher Profile（包签名 key 轮换即由此完成）。
2. Loran 取 descriptor 的方式与 Realm 相同：按 authority-discovery 跟随重定向（最多 5 次、每跳
   HTTPS、始终用原域名验证 `authority` 绑定），每一跳执行 SSRF 检查与 DNS pinning，限长 64 KiB。
3. 包身份直接验证 manifest 签名与 Publisher 公钥，不需要发布备案。
4. 消费端从允许的 HTTPS origin 下载后验证 digest 和签名；origin 不是信任根。

## 网络

HTTP 服务共享一个 HTTP/2/HTTPS listener；STUN/TURN 使用 ICE UDP listener 与独立
relay port range。端口可配置，descriptor 发布实际 endpoint。公网部署必须在反向
代理、证书、监听地址和 advertised endpoint 之间保持一致。

## 失败策略

- 配置未知字段、占位 secret、明文 admin 密码、生产无 TLS/KEK：拒绝启动。
- SQLite schema 不是 v10：拒绝启动，需停机后重建数据库。
- 请求所属 Realm 不存在或不是 `active`：拒绝，不猜测默认 Realm；Realm 签名 key 生成完成前
  `GET /.well-known/actr` 返回 503。
- Actor credential scope、连接 ACL 或 envelope identity 不一致：拒绝。
- Publisher DNS 指向非公网或发生重绑定：拒绝。
- Actor serial 候选碰撞：继续分配，不覆盖旧身份。

## 代码入口

| 领域 | 位置 |
|---|---|
| 进程与服务组合 | `crates/loran-server/src/service/` |
| 配置与 schema gate | `crates/loran-platform/src/config/`、`storage/db.rs` |
| Realm/连接 ACL | `crates/loran-platform/src/realm/` |
| Issuer/renewal | `crates/loran-issuer/` |
| Issuer Signer | `crates/loran-signer/` |
| Signaling / Actor Registry | `crates/loran-signaling/` |
| STUN/TURN | `crates/loran-stun/`、`crates/loran-turn/` |
| Publisher Registry | `crates/loran-publisher-registry/` |
| 管理契约 | `crates/loran-proto/`、`crates/loran-control/` |

## 源码命名

历史名清理分支 `refactor/historical-name-cleanup-hardening` 已把代码中的历史名统一为
publisher、Publisher Registry、Issuer、Issuer Signer、Actor Registry（proto 字段编号不变，
包清单升为 `actr-package/2` 且不接受旧键，SQLite 表原地迁移），不使用 serde alias 或双轨
解析。仍沿用旧名的只有 `loran*` 包名/二进制名，旧→新对照见[术语表](../../docs/GLOSSARY.zh.md)
「历史名称对照」。

安全假设、P0/P1 和发布门槛见
[`THREAT_MODEL_REALM_V1.zh.md`](THREAT_MODEL_REALM_V1.zh.md)。
