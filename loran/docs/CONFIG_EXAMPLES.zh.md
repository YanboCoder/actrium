# Loran Realm v1 配置示例

v1 只推荐一种生产拓扑：单节点、SQLite、单一 Actr Facility Provider、all-in-one。这个单节点作为
共享 Actr Facility Provider 承载多个 Realm，无需为每个 Realm 另起实例（见
[架构契约](REALM_V1_ARCHITECTURE.zh.md)）。本文不提供集群、跨区域或 federation 示例，
因为实现不承诺这些能力。

## 安全单节点生产版

从仓库模板开始：

```bash
install -m 600 config.example.toml /etc/loran/config.toml
openssl rand -hex 32
openssl rand -base64 48
```

核心配置形态：

```toml
name = "loran-prod-01"
env = "prod"
enable = 31
sqlite_path = "/var/lib/loran"
location_tag = "single-node"
loran_shared_key = "<独立随机值>"

[authority]
enabled = true
uri = "actr://realm.example.com"

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

[turn]
realm = "realm.example.com"
relay_port_range = "49152-65535"
turn_secret = "<独立随机值>"

[services.signer]
kek_env = "LORAN_SIGNER_KEK"

[services.signer.storage]
backend = "sqlite"

[services.signer.storage.sqlite]

[services.issuer]

[services.issuer.server]
worker_id = 0
renewal_token_secret = "<独立 Base64 随机值>"

[services.signaling]

[services.signaling.server]
ws_path = "/signaling"

[control.admin_ui]
enabled = true
password_hash = "<Argon2id PHC hash>"
session_expiry_secs = 86400

[control.grpc_api]
enabled = false
```

这段只展示关键字段，完整 recording、monitoring、限流和注释见
[`config.example.toml`](../config.example.toml)。各 secret 必须分别生成，不能把
示例字符串直接用于部署。

启动前：

```bash
LORAN_SIGNER_KEK="<64-hex-or-44-base64>" \
  loran test /etc/loran/config.toml
```

验证通过后再由 service manager/systemd 注入同一环境变量并启动。首次启动后：

- 在 authority descriptor 中的 Realm 签名 key 生成完成前，`GET /.well-known/actr` 返回
  503，这是正确行为；
- 之后 Realm 由经 Realm 签名 key 持有方授权的创建请求建立，`publish_profile` 验证通过后
  激活；Actr Facility Provider 域名下的 Realm 目前由管理员在 Admin UI 中经 Admin API 代办创建。
  实例本身不需要预先 provision 任何 Realm。

## 本地开发

本地开发仍使用同一逻辑模型，只放宽 TLS：

```toml
name = "loran-dev"
env = "dev"
enable = 31
sqlite_path = "database"
location_tag = "local"
loran_shared_key = "0123456789abcdef0123456789abcdef"

[authority]
enabled = true
uri = "actr://localhost"

[bind.http]
domain_name = "localhost"
advertised_ip = "127.0.0.1"
ip = "127.0.0.1"
port = 8081

[bind.ice]
advertised_ip = "127.0.0.1"
advertised_port = 3478
ip = "127.0.0.1"
port = 3478

[turn]
realm = "loran.local"
relay_port_range = "49152-65535"
turn_secret = "0123456789abcdef0123456789abcdef"

[services.signer]

[services.signer.storage]
backend = "sqlite"

[services.signer.storage.sqlite]

[services.issuer]

[services.issuer.server]
worker_id = 0
renewal_token_secret = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="

[services.signaling]

[services.signaling.server]
ws_path = "/signaling"

[control.admin_ui]
enabled = true
password_hash = "<generate-argon2id-phc>"

[control.grpc_api]
enabled = false
```

上面的固定值只适合隔离的本地测试，绝不能提升到公网。

## 可选：独立 Issuer Signer 边界

若风险评估要求私钥操作与公网 listener 分进程，可让 Issuer/Signaling 指向独立 Issuer
Signer：

```toml
[services.issuer.dependencies.signer]
endpoint = "https://signer.internal.example:8443"
timeout_seconds = 30
enable_tls = true
tls_domain = "signer.internal.example"
ca_cert = "/etc/loran/pki/ca.pem"
client_cert = "/etc/loran/pki/issuer.pem"
client_key = "/etc/loran/pki/issuer-key.pem"
```

这只是逻辑平面的物理隔离，不改变实例承载的 Realm 数量，也不提供 HA。首版若无该隔离需求，
all-in-one 更简单，也更容易正确运维。
