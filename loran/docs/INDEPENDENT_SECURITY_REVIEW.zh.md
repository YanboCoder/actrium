# Loran Realm v1 独立安全评审包

状态：交给独立评审者的范围与验收模板。项目作者不得把自审签字当作独立结论。

## 评审目标

判断单节点、SQLite、单一 Actr Facility Provider profile 是否存在可导致以下结果的缺陷。评审对象
须写明所评审的 commit 属于哪种 Realm 模型：当前实现是一个实例一个 Realm；目标
契约是一个实例承载多个 Realm（Realm 名 `<realm_id>@<domain>`，以 Realm 签名 key 签名的
Realm Profile 激活），见 `REALM_V1_ARCHITECTURE.zh.md` 第 9 节。缺陷类别：

- 未授权签发或冒充 Actor；
- 跨 Realm/Actr Facility Provider/audience 重放，或同一实例内 Realm 之间的状态、缓存、发现与管理
  权限串通；
- 在他人域名下创建或激活 Realm、接受未经 Realm 签名 key 签名、已回滚或改换 `provider.id` 的
  Realm Profile，或在 Realm Profile 不可用时回退到未委派的 Actr Facility Provider；
- 绕过连接 ACL 发现/连接 Actor；
- 获取 Issuer Signer 私钥、Realm/TURN/renewal/Admin secret；
- 发布或运行未获 Publisher 授权的 package；
- SSRF、DNS rebinding 或任意内网读取；
- TURN 未授权中继、资源耗尽或流量放大；
- SQLite/迁移破坏身份唯一性、revocation 或 Realm 隔离；
- credential/secret 经 URL、日志、指标或错误消息泄露。

评审基线必须固定到具体 commit，并记录编译器、依赖锁文件、部署配置和测试环境。

## 必读材料

1. `REALM_V1_ARCHITECTURE.zh.md`
2. `THREAT_MODEL_REALM_V1.zh.md`
3. `API.md`
4. `CONFIGURATION.md`
5. `TURN_STUN_INTEROP_TESTING.zh.md`
6. `REALM_V1_ACCEPTANCE_EVIDENCE.zh.md`
7. `DEPENDENCY_RISK_REGISTER.zh.md`

## 高风险代码

| 边界 | 位置 | 重点 |
|---|---|---|
| 配置/启动 | `crates/loran-platform/src/config`、`crates/loran-server/src/main.rs` | fail closed、secret 来源、TLS、降权 |
| SQLite | `crates/loran-platform/src/storage/db.rs`、迁移脚本 | schema gate、Realm 约束（当前：singleton；目标：Realm 名唯一与 Actr Facility Provider tombstone）、事务/备份 |
| Issuer | `crates/loran-issuer/src/issuer.rs`、`renewal.rs` | 注册模式、proof、nonce、scope、ID collision |
| Issuer Signer | `crates/loran-signer/src/grpc_handlers.rs`、`storage` | auth payload、expiry、KEK、私钥不外泄 |
| Signaling | `crates/loran-signaling/src/axum_router.rs`、`server.rs` | WS transport、source binding、连接 ACL、限流 |
| TURN/STUN | `crates/loran-turn/src/authenticator.rs`、`crates/loran-stun/src/lib.rs` | credential、cache、allocation、畸形流量 |
| Publisher Registry | `crates/loran-publisher-registry/src/`（publisher 发现、`manager.rs`） | SSRF、descriptor、rotation、nonce、签名 |
| Admin | `crates/loran-server/src/service/http/admin_api.rs` | Argon2id/JWT、Realm 唯一性、owner 邀请签发与只读成员列表、审计 |

评审还应确认 SDK/authoring 源码中的历史发布者字段名（见[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」）没有
意外重新进入 Loran v1 的 HTTP/JSON、包清单和 ActorId 边界；源码命名迁移本身不作为安全通过的替代条件。

## 最小动态测试

- 对所有管理/Issuer Signer/Issuer/Signaling 请求做缺失、错误、过期、重放、跨 scope credential
  测试；
- 并发创建 Realm、并发消费 nonce、并发 renewal 和时钟回拨；
- 目标契约：两个 Realm 使用相同 Actor 类型与相同 serial 候选时，credential、presence、
  TURN、连接 ACL 与管理接口互不相通；伪造或回滚 `realm_signing_keys`/Realm Profile；authority
  descriptor 获取在逐跳重定向下的 SSRF/DNS rebinding；Realm 管理请求重放到其他 Actr Facility Provider
  或在有效期内重复提交；
- Publisher DNS 在预检查与连接间切换到 loopback/private/link-local；
- redirect、超限 descriptor、慢响应、Unicode/大小写/尾点/端口域名；
- TURN allocation/permission/channel bind 洪泛、credential cache 填满、端口耗尽；
- WebSocket 超限 frame、连接洪泛、伪造 envelope source、浏览器 subprotocol 注入；
- SQLite 备份/恢复、截断、磁盘满、旧 schema、多 Realm 输入；服务注册后立即重启应
  恢复原身份/元数据但保持离线，只有原 Actor 认证重连后才恢复发现/路由和 ACL；
  注销成功后立即重启不得恢复 ServiceSpec 或服务记录；
- key rotation/revocation 后验证新签名、历史签名、泄露 key 和未过期 credential；
- 检查 access/security/audit/operations 日志及 `/metrics` 无 credential/secret。

## 结论格式

每个发现至少包含：

- ID、严重度（P0/P1/P2/P3）；
- 可利用前提与影响；
- 最小复现；
- 受影响 commit/文件/协议；
- 修复建议；
- 修复后复测证据。

发布阻断规则：

- P0/P1 未关闭：禁止 production-ready；
- 测试范围缺失、环境不明或无法复现：结论不能记为通过；
- 仅自动扫描无人工 trust-boundary review：不算独立安全评审；
- 评审者与主要实现者相同：不算独立。

最终报告应作为 PR/发布工件链接，并明确列出仍接受的 P2/P3 和残余风险。

## 独立评审记录（2026-07-30）

评审者：非本 PR 实现者的独立评审线（对抗式源码审查 + 真实实例动态验证）。

### 固定基线

- 评审固定 commit：`221828cd`（= PR head `61a23bd9` 合入 `main` v0.5.9 后的同步基线）。
- 编译器：rustc/cargo 1.96.0；依赖：`turn 0.11.0`、`stun 0.8.0`、`jsonwebtoken 10.4.0`（`aws_lc_rs`）。
- 环境：Linux x86_64 单机实例（`env=dev`），TURN 互操作跨 s67(macOS)/s68(Linux)/s17(Linux) 局域网。

### 裁决：DO NOT MERGE（未达 production-ready）

存在已确认的 P0/P1 阻断项，命中本评审包「发布阻断规则」第 1 条，全部需在再次评审前关闭。

具体 finding（含可利用漏洞的复现步骤、受影响范围与修复方案）按协同披露原则不在本公开文档展开，已与维护者私下同步、单独跟踪与修复。本文件只记录基线、裁决与不涉及利用路线的正向结论。

### 动态验证纠正

- 一项静态推断的资源耗尽风险（未认证 Allocate → nonce 表无界增长）在真实实例上**未复现**：对 PR TURN 与示例 fixture 共约 64 万次未认证 Allocate（含数千不同源端口），进程 RSS 零增长。这体现回真实数据验证的必要性。

### 已确认的正向控制（做对的部分，勿回退）

- schema 版本 gate（当前 v2）对未版本化/错误版本库 fail-closed（`unsupported ... run the standalone migration tool`）。
- JWT 算法锁定 HS256、无 `alg:none`/RS-HS 混淆；realm 单例唯一索引（该控制针对当时的单 Realm 契约；已由按 Realm
分区与 Realm 名唯一约束取代，见架构契约第 9 节）；signaling 侧 credential source/连接绑定；移除全局 `CorsLayer::permissive`；TURN 出厂 placeholder secret 已移除并 fail-closed。
- 浏览器 TURN relay-only 互操作：Chrome 148、Firefox 152 均 PASS（relay/relay UDP）；Safari 26.5 经自动化未在窗口内完成握手，记为待人工复核；独立 RFC5766 客户端 / iOS·Android 原生 / NAT 矩阵 / TURN-over-TCP·TLS 仍为发布前外部矩阵缺口。
