# Loran crate 导航

Loran 位于 Actr workspace 内，crate 版本与 workspace 同步。本文只说明稳定职责，
不复制易失真的函数签名或行号。Cargo package 名仍沿用 `loran-` 前缀（见[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」）。

| 目录 | package | 职责 |
|---|---|---|
| `crates/loran-server` | `loran`（binary） | 读取/验证配置，组合 listener、router、ICE 服务、管理面与关闭流程 |
| `crates/loran-platform` | `loran-platform` | 配置、SQLite schema、Realm/连接 ACL、Actor credential validation、监控与公共基础设施 |
| `crates/loran-issuer` | `loran-issuer` | Issuer：Actor serial 分配、注册证明、Actor/TURN credential、renewal |
| `crates/loran-signer` | `loran-signer` | Issuer Signer：Realm issuer key 生命周期、KEK 加密存储、签名和验签 key 查询 |
| `crates/loran-signaling` | `loran-signaling` | 认证 WebSocket、presence、Actor Registry 与发现、signaling envelope 转发 |
| `crates/loran-stun` | `loran-stun` | RFC 5389 Binding 与 NAT 地址发现 |
| `crates/loran-turn` | `loran-turn` | scoped long-term credential、allocation、permission、relay 与配额 |
| `crates/loran-publisher-registry` | `loran-publisher-registry` | Publisher Registry：publisher 身份、Publisher 签名公钥与密钥撤销 |
| `crates/loran-packages` | `loran-packages` | `.actr` 包托管（Realm Profile 的 `package.href` 可指向它）：按 SHA-256 内容寻址的静态下载，不解析 Target |
| `crates/loran-proto` | `loran-proto` | 管理/控制面的 protobuf 契约 |
| `crates/loran-control` | `loran-admin` | 管理控制服务实现 |
| `crates/loran-sdk` | `loran-sdk` | Loran 组件的内部聚合导出 |

## 依赖方向

```text
loran-server
 ├─ loran-platform
 ├─ loran-issuer ── Issuer Signer client
 ├─ loran-signaling ── Issuer/Issuer Signer clients
 ├─ loran-stun
 ├─ loran-turn
 ├─ loran-publisher-registry
 └─ loran-control/loran-proto
```

`loran-server` 是 composition root。业务 crate 不应反向依赖 daemon，也不应通过全局默认值
偷偷构造 Realm、publisher 或 Actor credential scope。

## 关键不变量

### `loran-platform`

- SQLite `user_version` 必须等于 schema v10；
- 一个实例承载多个 Realm，`realm.name`（完整 Realm 名）唯一且不可变，退役行即 Actr Facility
  Provider tombstone、名字不复用；所有按 Realm 分区的状态以完整 Realm 名 `realm` 为键；
- Actor credential validation 检查 protocol version、issuer（即 Realm 名）、Actr Facility Provider、
  audience、Realm、Actor 和过期时间，且 Realm 必须为 `active`；
- 配置未知字段和生产不安全值 fail closed。

### `loran-issuer`（Issuer）

- ActorId 是实例身份，不含程序类型，类型由注册与 Actor credential 携带；
- Actor serial 由显式 worker ID 的 Snowflake 候选生成，并在 SQLite 已签发身份登记中按 Realm
  分区永久预留，只为 `active` Realm 签发；
- package workload 必须有 publisher/package proof，linked workload 必须有授权证明；
- renewal 需要 possession proof，不能仅凭 ActorId 续签；
- Realm registration capability 不得下发给 Guest。

### `loran-signer`（Issuer Signer）

- 私钥没有下载 API；
- `Sign` 认证 payload 绑定 key ID 和待签消息 digest；
- 生产 SQLite 私钥由独立 KEK 加密；
- key 都是具名 key，不按时间过期；退役即销毁私钥，此后不再签名。

### `loran-signaling`

- upgrade 身份、Actor credential 中的 Actor 和 envelope source 必须一致；
- Actor Registry 按 Realm 分区（SQLite 表 `actor_registry`，启动时由旧表 `service_registry` 原地改名）；
- 原生客户端用 headers，浏览器用限定 WebSocket subprotocol transport；
- 连接 ACL 默认拒绝；
- 帧大小、连接数、消息速率和内存状态有界。

### `loran-turn` / `loran-stun`

- STUN Binding 不要求身份，只返回观测到的 mapped address；
- TURN allocation 要求 scoped、短期 TURN credential；
- auth cache、allocation、permission 与 relay port 使用必须有界；
- 畸形 datagram 不得让 listener 失去服务能力。

### `loran-publisher-registry`（Publisher Registry）

- 公开身份只叫 publisher，ID 是规范化 DNS 域名；
- publisher 经 `register_publisher`（Actr 管理信封，由 Publisher Profile 中 `active` 的签名
  key 签名）登记；获取 authority descriptor 拒绝重定向超限、私网目标、超限 body 与 DNS
  rebinding；key rotation 通过再次发送 `register_publisher` 完成，见 REALM_V1_ARCHITECTURE
  第 7 节；
- 包身份直接验证 manifest 签名与 Publisher 公钥，不登记包发布记录；
- package origin 不成为信任根。

## 公开与内部命名

v1 公开 TOML（如 `[services.issuer]`、
`[services.signer]`）、HTTP JSON、descriptor 和文档统一使用 `publisher`、`issuer`、
`signer`。SQLite 表名与 Cargo package 名属于内部实现，不是协议标识，不允许
出现在新的外部契约中。

## 测试入口

```bash
cargo test -p loran-platform
cargo test -p loran-issuer
cargo test -p loran-signer
cargo test -p loran-signaling
cargo test -p loran-stun
cargo test -p loran-turn -- --test-threads=1
cargo test -p loran-publisher-registry
cargo test -p loran-server --test loran_fullstack
```

完整 workspace 仍须执行 `cargo test --workspace -- --test-threads=1`，crate 局部测试不能
替代跨协议/客户端兼容验证。
