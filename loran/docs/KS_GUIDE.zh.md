# Loran Issuer Signer v1 指南

状态：首版安全契约。Issuer Signer 是 Issuer 的签名后端，代码 `crates/loran-signer`，配置
`[services.signer]`，gRPC 包 `signer.v1`（历史名称见[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」）。

## 职责

Issuer Signer 是 Issuer 的最小私钥边界：

1. 生成 Ed25519 signing key：每个 Realm 的 Realm issuer key（首把 `issuer:<realm>`，轮换产生 `issuer:<realm>/<毫秒时间戳>`）、每个
   Actr Facility Provider 域名一把 Realm 签名 key（`realm-signing:<domain>`），全部 Realm 共用同一
   Issuer Signer；
2. 在服务端保存私钥，只返回 verifying key；
3. 对 Issuer 提交的精确消息签名；
4. 退役 key：退役即销毁私钥，此后不再签名。key 都是具名 key，不按时间过期。

Issuer Signer 不负责 Actor 路由、Realm 管理、Publisher 下载，也不提供「获取私钥」接口。

## 接口

Loran all-in-one profile 将 Issuer Signer gRPC 挂载在主 HTTP/2 监听器：

```text
/signer.v1.Signer/Sign
/signer.v1.Signer/GetVerifyingKey
/signer.v1.Signer/EnsureNamedKey
/signer.v1.Signer/RetireNamedKey
/signer.v1.Signer/HealthCheck
```

请求凭证放在 protobuf message 中，不进入 URL 或日志。所有有副作用请求使用 nonce-auth
防重放。`Sign` 的规范认证 payload 为：

```text
loran-signer-v1:sign:<key_id>:sha256:<message_digest>
```

因此认证过的请求不能被替换成另一把 key 或另一段消息。HTTP JSON handler 仅供 crate
级测试/嵌入使用，语义与 gRPC 相同。

## 存储

首版生产配置推荐 SQLite，不依赖 PostgreSQL、NATS 或 KMS。SQLite 保存 key metadata
和经 KEK 加密的私钥；nonce 使用独立 SQLite 表防重放。

生产环境必须配置以下一种 KEK source：

```toml
[services.signer]
kek_env = "LORAN_SIGNER_KEK"
```

也可以使用 `kek_file`；文件应只有 Loran 运行用户可读。`kek` 直接写配置只适合受控
迁移或测试。缺少 KEK 的 `env = "prod"` 配置会 fail closed。首版不强制外部 KMS，
但 Issuer Signer 可以拆成独立进程和安全边界。

## 生命周期

- `EnsureNamedKey` 按名字取 key，缺失时生成；key 不按时间过期；
- `RetireNamedKey` 清除其私钥、保留名字与 verifying key，之后 `Sign`
  返回 `FAILED_PRECONDITION`。退役幂等；名字尚无 key 时直接建为已退役，名字不会再得到可用
  key。Realm issuer key 轮换后移除旧 key、以及 Realm 退役时，Issuer 以此退役相应的 issuer key；
- revocation 与轮换必须进入审计记录。

## 运维要求

- `loran_shared_key`、KEK、TURN secret 和 renewal secret 分离；
- secrets 不使用示例值，不写普通日志，不放命令行或 URL；
- 生产使用 HTTPS/mTLS 或等价的受保护内部链路；
- 备份 SQLite 时同时保护 KEK；只有数据库或只有 KEK 都不足以恢复签名能力；
- 发布前执行威胁模型中的独立安全审查、TURN/STUN 滥用与互操作测试。

更多内容见
[`THREAT_MODEL_REALM_V1.zh.md`](THREAT_MODEL_REALM_V1.zh.md)。
