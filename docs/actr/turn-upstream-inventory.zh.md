# PR #325 TURN 上游来源与差异清单

源码核验日期：2026-10-12；上游合并状态核验日期：2026-11-02；依赖发布状态核验日期：2026-11-09（UTC+8）。本清单覆盖 #325 中 TURN/STUN 的标准化改动；WSS 业务 relay 的协议与产品功能另见 [验收记录](wss-relay-acceptance.zh.md)。本文件为中文文档。

收敛规则：上游已有则采用对应版本或提交；上游缺少的通用实现必须有已提交的 issue/PR；项目配置和接线不能包含独有的 RFC 报文实现。

## 源码来源

| 上游 crate | 当前使用的来源 | 包含的改动 |
| --- | --- | --- |
| `webrtc` | 官方上游 `8ea6e2a69b0c4c97f38b0e81e5a77e1c066f3757` | 包含 [#928](https://github.com/webrtc-rs/webrtc/pull/928)、已合并的 [#944](https://github.com/webrtc-rs/webrtc/pull/944) 和 TURN 发送修复 [#946](https://github.com/webrtc-rs/webrtc/pull/946) |
| `rtc`、`rtc-turn`、`rtc-rtp` | 同一 WebRTC 提交中的 RTC 子模块 `cd3f08a243ced3714c965070b4fa7105dfe89c0e` | 包含 [rtc #274](https://github.com/webrtc-rs/rtc/pull/274) 及后续 RTC 修复 |
| `turn`、`stun`、`webrtc-util` | crates.io `0.17.3` | [#936](https://github.com/webrtc-rs/webrtc/pull/936) 及其前置修复 |

#936 的合并提交包含 #927/#930/#932/#934 的前置修复，以及剩余通用实现和回归；这些 PR 现均已合并，同一修复不能重复计数为新增贡献。Cargo 覆盖配置与外部消费者的要求见[上游依赖说明](upstream-patches.md)。

客户端依赖固定为官方上游提交，服务端依赖固定为已发布版本；依赖图核验需要同时检查 `rtc-turn` 的实际来源及 RTC 子模块提交。

客户端 Refresh(0)、原 socket 释放、close 完成等待、自 driver 回调避免自等待，直接采用 0.21.1（上游 #895/#911/#912），删除原本地重复实现；等待上限采用上游 100ms。Allocate 重传直接采用已合入 legacy 的 [#896](https://github.com/webrtc-rs/webrtc/pull/896) 缓存结构和处理，不保留本地另一套 raw 缓存。

## 已合并的上游贡献

| 问题 | Issue | PR | 合并目标与日期（UTC+8） |
| --- | --- | --- | --- |
| RTC channel 范围、回绕和耗尽 | [rtc #273](https://github.com/webrtc-rs/rtc/issues/273) | [rtc #274](https://github.com/webrtc-rs/rtc/pull/274) | `master`，2026-10-26 |
| 凭据轮换保留已有 allocation 的原身份 | [webrtc #925](https://github.com/webrtc-rs/webrtc/issues/925) | [webrtc #928](https://github.com/webrtc-rs/webrtc/pull/928) | `master`，2026-10-25 |
| requested relay port 越出配置范围 | [webrtc #926](https://github.com/webrtc-rs/webrtc/issues/926) | [webrtc #927](https://github.com/webrtc-rs/webrtc/pull/927) | `v0.17.x`，2026-10-25 |
| legacy channel 范围、回绕和耗尽 | [webrtc #929](https://github.com/webrtc-rs/webrtc/issues/929) | [webrtc #930](https://github.com/webrtc-rs/webrtc/pull/930) | `v0.17.x`，2026-10-25 |
| 跨 listener 的实际 EVEN-PORT socket 保留 | [webrtc #931](https://github.com/webrtc-rs/webrtc/issues/931) | [webrtc #932](https://github.com/webrtc-rs/webrtc/pull/932) | `v0.17.x`，2026-10-25 |
| 认证错误和完整性/nonce 检查顺序 | [webrtc #933](https://github.com/webrtc-rs/webrtc/issues/933) | [webrtc #934](https://github.com/webrtc-rs/webrtc/pull/934) | `v0.17.x`，2026-10-25 |
| 剩余通用认证、报文、allocation 和 peer 校验 | [webrtc #935](https://github.com/webrtc-rs/webrtc/issues/935) | [webrtc #936](https://github.com/webrtc-rs/webrtc/pull/936) | `v0.17.x`，2026-10-26 |

以上七个 PR 均已合并；合并状态与 crates.io 版本发布状态是两回事。服务端修复已包含在 0.17.3 中，客户端修复仍需使用上述固定 Git 提交。

## 逐项映射

ID 延续最初 38 项审计。表内「#」均指 webrtc-rs/webrtc，标注 rtc 的除外。规范以 [RFC 8489](https://www.rfc-editor.org/rfc/rfc8489.html) 与 [RFC 8656](https://www.rfc-editor.org/rfc/rfc8656.html) 为准。

| ID | 内容 | RFC / 边界 | 最终出处或处置 |
| --- | --- | --- | --- |
| S01 | 401 的 REALM/NONCE 挑战 | 8489 §9.2.4 | #934；算法列表扩展 #936 |
| S02 | 用户、realm、完整性错误 | 8489 §9.2.4 | #934 与 #936 |
| S03 | 先验完整性再验 stale nonce | 8489 §9.2.4 | #934 |
| S04 | 401/438 挑战字段 | 8489 §9.2.4 | #934 与 #936；438 完整性属性为 MAY |
| S05 | nonce cookie/security feature | 8489 §9.2/§18.1 | #936；保留正文 wire 编码并记录勘误歧义，见下文 |
| S06 | PASSWORD-ALGORITHMS 协商 | 8489 §9.2.4、§14.11–12 | #936 |
| S07 | MD5/SHA256 key、OpaqueString | 8489 §9.2.2、§18.3 | #936；本地 wrapper 委托库 helper |
| S08 | SHA-1/SHA-256 HMAC 与双完整性 | 8489 §9.2.4、§14.5–6 | #936 |
| S09 | SHA-256 只接受完整 32 字节 | 8489 §14.6；TURN 未许可截短 | #936，含截短/超长拒绝及正例 |
| S10 | 已认证成功/错误响应签名 | 8489 §9.2.4 | #936 |
| S11 | 完整性后语义属性不影响处理 | 8489 §9.2.4 | #936，共享 datagram parser |
| S12 | 长度、FINGERPRINT、未知方法/类 | 8489 §5、§6.3、§14.7 | #936，含独立 Binding helper |
| S13 | 未知必选 420、可选忽略 | 8489 §6.3.1 | #936；DONT-FRAGMENT 作为未支持的必选属性 |
| S14 | allocation 原用户名/441 | 8656 §5 | #936 |
| S15 | 缺失 allocation/437 | 8656 §5 | #936 |
| S16 | Allocate 重传重放 | 8489 §6.3.1、8656 §7.2 | 已合入 #896，直接采用 |
| S17 | allocation 保留原 key | 8656 §6 | #936；凭据过期挡新建，显式删除用于撤销 |
| S18 | 按用户名删除和等待完成 | 管理 API，并非 RFC wire MUST | 上游已有 API；完成通知时序修复 #936 |
| S19 | LIFETIME 长度、默认/上限、零释放 | 8656 §7.2、§8.2 | #936，600 秒默认与 3600 秒服务上限 |
| S20 | 地址族与 440/443 | 8656 §7.2、§8.2、§10.2、§12.2 | #936 |
| S21 | ADDITIONAL-ADDRESS-FAMILY 部分成功 | 8656 §7.2、§18.11–12 | #936；单 relay socket，440/508，不宣称真实双 allocation |
| S22 | 多 peer 全部验证后安装 permission | 8656 §10.2 | #936，含混合非法 peer 无副作用回归 |
| S23 | channel 参数、唯一绑定与范围 | 8656 §12、§12.2 | #930 范围；#936 服务端验证 |
| S24 | RTC channel 范围与耗尽 | 8656 §12 | rtc #274 |
| S25 | ChannelData 范围 | 8656 §12.6 | rtc #274、#930；采用新范围，不增本地兼容开关 |
| S26 | Teredo/6to4 端点禁止 | 8656 §21.4 | #936 |
| S27 | EVEN-PORT 的 R=0/R=1、实际 odd socket | 8656 §7.2、§18.7 | #932 |
| S28 | token 30 秒、跨 listener、单次消费 | 8656 §7.2 | #932 |
| S29 | inclusive range/requested port | 8656 §7.2 推荐范围；配置边界 | #927；65535 原表达式已正确，删重复改写 |
| S30 | IPv6 bind socket 构造 | 地址解析兼容性 | 原上游 resolve_addr 已支持，恢复上游实现 |
| S31 | 大 UDP 接收缓冲 | 8656 §11、§14 | #936；8192 字节双向回归，不外推所有极限报文 |
| S32 | legacy channel 回绕不覆盖活跃绑定 | 8656 §12 | #930，移除本地重复 create 逻辑 |
| A01 | 轮换凭据保留旧 allocation 身份 | 8656 §5 | #928 |
| A02 | 原五元组 Refresh(0) 释放 | 8656 §8.1–2 | 0.21.1 的既有上游实现 |
| A03 | close driver 完成和超时 | 生命周期工程约束 | 0.21.1，#911/#912，无私有两秒版本 |
| I01 | AuthHandler 扩展、EVEN-PORT API | 通用库 API | #936、#932；旧 handler 默认 MD5 |
| I02 | 包名、版本、manifest、发布链 | 项目交付接线 | 本项目保留；库源码按上述来源逐字节采用 |
| I03 | relay bind hostname 回退 | 既有 API 兼容性 | 恢复上游 resolve_addr；#927 原样保留，非新 RFC 修补 |

## 行业对照与争议

固定源码对照：[coturn](https://github.com/coturn/coturn/blob/fc8eb910fbb7b49bf35e8361b05762f2b1f7a0fe/src/server/ns_turn_server.c) 的原 allocation 身份/session key、错误码、lifetime、family 与 permission；[Pion](https://github.com/pion/turn/tree/b1bcca91802dffac8070b12ce0bee5bf244ffef1/internal/server) 的 allocation 查找和多 peer 处理；[rustun](https://github.com/sancane/rustun/blob/9ff67149b172b2cb7acc5845ca9d7d294a5eba19/stun-rs/src/attributes/stun/nonce_cookie.rs) 的 nonce 特性编码；[libwebrtc](https://github.com/webrtc-mirror/webrtc/blob/cc7d1174bd1b935db280e606f500b96bc44ba2a4/p2p/base/turn_port.cc) 的原 TURN 端口生命周期与 Refresh(0)。旧实现的 channel 范围、认证错误等分歧不凌驾于 RFC。此处是源码交叉验证，不冒充完整的跨服务端互操作实测。

RFC 8489 §9.2 指定 feature 0 的线上编码为 `80 00 00`（`gAAA`），rustun 相同；verified erratum 6290 改了 §18.1 的注册位编号，却未修改 §9.2。#936 及本地一致保留正文编码并记录争议，未盲改 `AAAB` 或同时置两个特性位。reported OpaqueString erratum 8746 也不作为已经验证的新规范。

## 本项目保留内容

- `loran-turn`：配置、端口范围解析、服务生命周期、统计，以及 REST HMAC 凭据签发、`timestamp[:opaque_suffix]` 约定、LRU 和新建准入过期策略。标准 key 派生、OpaqueString、REALM、静态 handler 均委托 #936。
- `loran-stun`：socket 循环、关闭信号、observer 和错误类型。Binding 报文处理与接收缓冲常量来自 #936，不保留独立解析器。
- `loran-platform` / `loran-server`：静态用户或共享密钥模式选择、advertised IP、端口范围、URL 与服务接线。URL 使用标准 `SocketAddr` 格式化，不自行实现 TURN URI parser。
- 原生 SDK 把配置交给 WebRTC adapter；浏览器 coordinator 调用浏览器 `RTCPeerConnection.setConfiguration`，维护本项目的消息桥接和路径观察，不实现 TURN wire 认证或 allocation 状态机。
- 上游 TURN、STUN 和 `webrtc-util` 使用同一 crates.io 0.17.3 版本，RTC 使用固定的上游 WebRTC 快照。Git/path 消费者可直接继承这些依赖；registry 消费者在上游发布包含客户端修复的版本前仍需配置 RTC 工作区 patch。

## 验证证据

以下验证记录对应迁移到 Git patch 之前的 vendor 快照，不代表迁移后的构建或运行结果。

上游 [#936 的覆盖说明](https://github.com/kookyleo/webrtc/blob/30102430994e7f797769aa44a90121dcb140469d/turn/doc/udp-conformance.md) 将行为对应到测试名。补齐前在四个既有服务端 PR 上，仅加入可编译测试所需的认证 API/helpers，17 项 UDP 测试为 14 失败、3 通过；补齐后为 19/19（新增独立 Binding 与 custom-family 边界）。库 72 项、认证错误 4 项、EVEN-PORT 3 项通过，strict clippy 通过。

本地 STUN 9 项、TURN 库 18 项、UDP 17 项及 vendored TURN 库 72 项通过。按 `test_client_with_stun` 前缀过滤三项测试（包含公共 STUN 及本地超时用例）；一项上游既有测试保留 ignored。客户端 RTC TURN 46 项（过滤 3 项 STUN 用例）、异步 WebRTC 49 项、本地真实 TURN 生命周期集成 5 项及浏览器配置/路径测试 5 项通过。根格式检查、TURN/STUN 全 target strict clippy、独立 NAPI/browser locked 依赖解析与发布依赖顺序/版本更新验证通过。生成全部 35 个 `.crate` 归档，核对四个维护包的完整源码摘要、双许可证、规范化 registry 依赖链和 0.21.1 协议版本，全部一致。归档使用 `--no-verify`，编译由上述独立检查和测试覆盖，未发布任何 crate。测试组存在重叠，不相加为独立用例总数。

本清单约束 #325 已涉及的 UDP 标准化改动，不宣称整套 RFC 认证，也不扩展为实现 TCP/TLS/DTLS、OAuth、USERHASH、DONT-FRAGMENT socket control 或 RFC 5780。
