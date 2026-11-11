# Loran Realm v1 配置

本文描述当前实现的单节点、SQLite profile：一个实例承载多个 Realm。尚未实现的部分见
[架构契约](REALM_V1_ARCHITECTURE.zh.md)第 9 节。字段的可运行模板以
[`loran.example.toml`](../config/loran.example.toml)为准；配置结构使用
`deny_unknown_fields`，旧字段或拼错字段会导致启动失败。

## 验证与启动

```bash
loran test /etc/loran/config.toml
loran --config /etc/loran/config.toml
```

示例中的 secret 均为故意不可用的 placeholder。生产配置若仍含
`default`、`change`、`example`、`replace` 或 `<generate` 等标记会被拒绝。

## 实例

```toml
name = "loran-realm-01"
env = "prod"
enable = 31
sqlite_path = "/var/lib/loran"
location_tag = "single-node"
loran_shared_key = "<generate-with-openssl-rand-hex-32>"
```

`enable` 是业务服务位掩码：

| 位 | 值 | 服务 |
|---|---:|---|
| 0 | 1 | Signaling |
| 1 | 2 | STUN |
| 2 | 4 | TURN |
| 3 | 8 | Issuer（配置键 `issuer`） |
| 4 | 16 | Issuer Signer（配置键 `signer`） |

`enable = 31` 是 v1 推荐 all-in-one profile。Publisher Registry 与管理面挂在
共享 HTTP listener 上，不由该位掩码关闭。

`env = "prod"` 会启用严格校验：HTTPS 证书、非占位 secret、Issuer Signer KEK 和
Argon2id 管理密码 hash 缺一不可。`location_tag` 只用于观测；v1 不用它做
多区域路由或 HA。

## 网络

```toml
[bind.http]
domain_name = "realm.example.com"
advertised_ip = "203.0.113.10"
ip = "::"
port = 443
cert = "/etc/loran/tls/fullchain.pem"
key = "/etc/loran/tls/privkey.pem"

[bind.ice]
ip = "0.0.0.0"
port = 3478
advertised_ip = "203.0.113.10"
advertised_port = 3478
```

HTTP 与 ICE 端口均可配置，并会精确出现在待签 Realm Profile 的 Actr Facility Provider 端点中。
防火墙必须放行
HTTP/TCP、ICE/UDP 以及 TURN relay port range。`advertised_ip` 必须是客户端可达
地址，不能用监听通配地址代替。

## TURN

```toml
[turn]
realm = "realm.example.com"
relay_port_range = "49152-65535"
turn_secret = "<generate-with-openssl-rand-base64-48>"
```

`[turn].realm` 是 TURN 长期凭据算法中的认证域字符串（TURN REALM），与 Actr Realm
不是一回事，不随 Actr Realm 数量变化。

`turn_secret` 是 Actr Facility Provider 主密钥，供 Issuer 签发和 TURN 验证时效凭证，必须按 Actr Facility
Provider 独立、稳定保存。每个 Actr Realm 的 TURN secret 由它按 Realm 名与 Actr Facility Provider ID
经 HKDF-SHA256 派生（salt `actr.turn`，info `<realm>|<provider>`），不在配置中逐个声明；外部
coturn 无法直接使用主密钥验证。直接替换会使尚未过期的 TURN credential 失效，因此轮换必须作为显式
维护操作。relay range 不得包含特权端口，起始值不得大于结束值。

## Issuer Signer

```toml
[services.signer]
kek_env = "LORAN_SIGNER_KEK"

[services.signer.storage]
backend = "sqlite"

[services.signer.storage.sqlite]
```

首版推荐 SQLite。生产环境必须从 `kek_file`、`kek_env` 或 `kek` 提供 KEK；
推荐前两者。KEK 与 TURN secret、renewal secret 不得复用。
Signer 只保存具名 key，key 不按时间过期，由 Realm 退役或 issuer key 轮换退役。旧版的
`[services.signer] tolerance_seconds` 与 `[services.signer.storage] key_ttl_seconds` 已删除；
配置段拒绝未知字段，仍写这两项的配置会启动失败，删掉即可。
Issuer Signer 可以留在 all-in-one 进程；需要更强隔离时可使用现有 TLS/mTLS gRPC
客户端配置部署为独立安全边界。

## Issuer

```toml
[services.issuer]

[services.issuer.server]
worker_id = 0
token_ttl_secs = 3600
renewal_token_ttl_secs = 86400
renewal_rotation_window_secs = 21600
renewal_token_secret = "<generate-with-openssl-rand-base64-48>"
```

单节点使用固定 `worker_id`，范围是 `0..31`。未来若出现并发 Issuer，必须由部署
系统为每个活动节点分配不同 worker ID；v1 不实现自动租约。renewal secret 的
Base64 解码结果至少 32 字节，并须跨重启稳定，否则 Service 无法保持实例身份。

## Signaling

```toml
[services.signaling]

[services.signaling.server]
ws_path = "/signaling"
```

连接与消息限流均有安全默认值，可在
`services.signaling.server.rate_limit` 下显式调整。生产修改前必须用目标连接数、
消息大小和重连模式压测。Signaling 自动使用本地 Issuer/Issuer Signer；外置依赖应配置
在 `dependencies.issuer` 与 `dependencies.signer`，并使用 TLS/mTLS。

## Publisher Registry

Publisher Registry 不需要单独的服务配置。
Publisher ID 是域名，通过 HTTPS descriptor 验证域名控制权并以 `register_publisher` 登记，
见架构契约第 7 节。

## 管理面

```toml
[control.admin_ui]
enabled = true
password_hash = "<argon2id-phc>"
session_expiry_secs = 86400

[control.grpc_api]
enabled = false
```

Admin UI 密码只接受 Argon2id PHC hash，明文密码字段不存在。v1 推荐只启用 Admin UI：
系统管理员经它创建、停用/恢复、退役 Realm 并代办 Actr Facility Provider 域名下的 Realm；其他域名
的 Realm 由经 Realm 签名 key 持有方授权的 Realm 管理请求创建。`grpc_api` 是 supervisor
集成接口；启用后
需要独立的 hex shared secret，并会改变 Realm 写入归属，不属于首版默认部署。

## Recording 与监控

`recording` 支持 stderr、文件与 OTLP sink，并将 observability、audit、security、
operations 分成独立 channel。secret、Actor credential、Authorization、WebSocket 身份
材料不得写入普通日志。

`/health` 与 `/metrics` 可用 CIDR allowlist 或 htpasswd 保护。公网生产环境不要在
两者均未配置时暴露管理监控端点。

## Realm provision 与数据

Realm 不在 TOML 中声明。一个实例承载多个 Realm，默认不限制数量；需要独占部署时可配置
`[realm] max_realms = 1`。空 schema 启动后没有任何 Realm；Realm 经 Realm 签名 key 持有方
授权的创建请求（`create_realm` + `publish_profile`）与 Realm Profile 验证后激活，Actr Facility
Provider 域名下的 Realm 由系统管理员经 Admin API 代办创建；收到退役 Realm Profile 或经
`…/retire` 时退役，写入 Actr Facility Provider tombstone，运行时不提供硬删除。按 Realm 的连接、
消息与 TURN 配额尚未实现，见架构契约第 9 节。

`[realm] profile_refresh_secs`（默认 3600，限定在 60–86400）是 Loran 刷新其他域名
`realm_signing_keys` 与 Realm Profile 的间隔（API.zh.md「Realm discovery」），每轮同时清除过期的
`pending` Realm。Realm 管理请求（API.zh.md「Realm 管理请求」）的 `create_realm` 另受：

- `max_realms_per_domain`：每个 Realm 域名的 `pending` 与 `active` Realm 数，默认不限；
- `max_pending_per_domain`：每个 Realm 域名的 `pending` Realm 数，默认 10；
- `pending_ttl_secs`：`pending` Realm 等待首份 Realm Profile 的时间，默认 3600，限定在
  60–86400；过期即删除，名字可再用（从未提供服务，不是 tombstone）。运营者创建的 `pending`
  Realm 同样受此限制。

`register_publisher` 受 `[realm] max_publishers` 约束：已登记的 Publisher 总数（不论状态），默认
1000，达到后新登记返回 `429`，已登记的 Publisher 仍可更新其 Publisher Profile。

Realm membership（API.zh.md「Realm membership」）的限额，超出返回 `429 quota`，邀请不被消耗：

- `max_members_per_realm`：每个 Realm 的 active 成员数，默认 1000；
- `max_clients_per_member`：每个成员未撤销的 Realm 客户端 key 数，默认 20；
- `max_open_invites_per_realm`：每个 Realm 未用、未撤销、未过期的邀请数，默认 100（owner 邀请不计）。

邀请有效期不超过 30 天，owner 邀请固定 7 天。

Actr Facility Provider ID 取 `[authority] uri`（`actr://<domain>`，不带端口）的规范化 DNS 域名。
启用 Issuer、Signaling 或 TURN 时 `uri` 必填，缺失或非法即配置校验失败；它写入 Actor
credential 与 TURN username 的 `provider`，并由 Signaling、TURN 校验。`[authority] enabled`
控制是否提供 `/.well-known/actr`（此时域名须与 `bind.http` 一致），`[authority] links`
是 descriptor 的 Realm 链接前缀（如 `["/i/"]`），默认不输出。

SQLite schema 不是当前的 v10 时拒绝启动。不提供迁移工具：旧库需停机后重建（见「Realm
provision 与数据」）。

详见[架构契约](REALM_V1_ARCHITECTURE.zh.md)和
[威胁模型](THREAT_MODEL_REALM_V1.zh.md)。
