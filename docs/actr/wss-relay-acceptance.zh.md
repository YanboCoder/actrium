> 当前 TURN 使用固定的上游发布版本，WebRTC 使用固定的上游 Git commit，详见[依赖说明](upstream-patches.md)和 [TURN 上游清单](turn-upstream-inventory.zh.md)。下文 vendor、0.21.0 和旧验证数字是当时阶段记录，不代表迁移后的交付方式或验证结果。

> Realm integration update (English): Validation records below describe the original
> pre-merge baseline. The current implementation also requires a signed Realm
> client proof for relay enrollment and checks Actor revocation and bindings.

# Loran TURN/STUN 与 WSS relay 验收记录

初次验收日期：2026-10-11；端到端加密增补：2026-10-12。规范构建目录的只读问题已解除；以下记录使用工作区实现进行编译、协议及真实节点验证。目标包括通用 UDP TURN/STUN、同入口独立部署、新增 WSS relay、原生与浏览器 SDK 迁移，以及与 WebRTC 经 TURN 类似的端点加密边界。

## 要求与证据

| 要求 | 实现与已执行验证 |
| --- | --- |
| 保留通用标准 UDP TURN/STUN | UDP 线协议覆盖认证、401/438、MD5/SHA-256、完整性、未知属性、重传、分配/权限/通道、IPv6 及 EVEN-PORT；当前服务端/协议组 235 项通过。coturn 独立客户端的 ChannelData、Send/Data、EVEN-PORT 三组通过 |
| 原 Loran 入口独立部署 | 原二进制仍为 `loran`。`ice_only` 验证 enable=2/4/6、不启动 HTTP/数据库、UDP Binding/Allocate、SIGHUP 重载及 SIGTERM 退出 |
| 新增可独立部署的 WSS relay | enable=32 单独部署、enable=38 与 UDP ICE 组合部署。`relay_only` 使用真实进程与 WebSocket 验证静态认证、转发、退出和不创建数据库，并用受信任测试证书验证生产 TLS/WSS 监听 |
| 通用核心无 Actrium 业务耦合 | `loran-relay-protocol` 和 `loran-relay` 只处理外部 Principal、namespace、endpoint、channel 与 bytes；10 项通用测试覆盖认证拒绝、隔离、接受握手、双向数据、连接替换、过期、超时及背压。静态/AId 认证位于外部 Loran 适配层 |
| 应用无需对端 WS server | 删除原生监听器及启动路径，不公布 `ws_address`；旧 `[websocket]` 配置报告迁移提示。SDK 从既有 signaling bootstrap 自动发现 relay，不增加应用 URL、监听端口或证书配置 |
| 先使用最快的端到端可用通道 | relay 必须收到对端接受、完成双向 Noise 身份认证及密钥确认、安装接收通道才就绪。原生真实节点在暂停 RTC 时完成双向 typed RPC、64 KiB 两类流和主动断线后的恢复。真实 Chromium Service Worker 与原生节点完成同类互通；加密前初次验收的本机首次 relay RPC 为 106 ms，该值不代表加密版本或公网性能 |
| 真实直连优先，并保留恢复路径 | 原生真实节点先走 relay，随后双方选中 ICE pair 均成为 Direct；服务端帧计数证明后续双向 RPC 不经过 relay，关闭 relay 后直连仍可通信。SDK 单元回归覆盖路径失败、已有回复通道、交付不确定时禁止自动重放；DOM 回归区分任一候选为 relay 的 TURN 路径 |
| 可靠流有序迁移 | 原生真实节点的 32 个可靠分块跨 relay → RTC 迁移后保持顺序。原生及真实 Chromium WASM 回归覆盖正确对端 ACK、取消、超时、RPC 并行、暂时暂停与旧端不支持；健康旧路径在屏障确认前保持使用 |
| 原生和浏览器应用 API 保留 | 两端沿用 RPC/DataChunk API。真实 Service Worker 调用原生 workload，原生嵌套调用浏览器 workload；两类 64 KiB 流经实际 WASM RuntimeContext 与 SDK 适配器互通，relay 重连后继续通信 |
| 原生 WebRTC 音视频和 TURN 兜底保留 | 当前原生回归验证 TURN 凭据轮换、ICE 重启、分配释放及媒体注册分发；另显式执行真实 WebRTC Opus 音频 RTP 回环。媒体仍只走 RTC，数据 relay 不接受 MediaRtp |
| 发布与部署资料 | 提供 `ice.example.toml`、`relay.example.toml`、原入口部署说明；浏览器正式 release 构建通过并同步 CLI 嵌入资产，逐文件一致性检查通过，真实 Chromium 加载发布 WASM/JS 并成功调用 relay 导出接口 |

## 验证记录

各组存在重叠，数量不应相加为不重复的总用例数。

- 根 workspace：`cargo check --offline --workspace --all-targets` 通过。
- 通用 relay/protocol：10 项通过。
- 服务端、配置、Actrium 协议与 UDP TURN/STUN：235 项通过。
- 原生传输、节点、TURN 轮换与媒体分发：249 项通过。之后删除了一个仅验证旧应用 WS 监听器退避的用例；相关清理另做定向复验。
- 显式真实原生路径/媒体组：3 项通过，含 relay-only、直连升级及 Opus RTP 回环。
- 最终清理后的定向复验：30 项通过，含生产 AId 认证适配器对身份、realm、签名、有效期和非规范端点的校验，以及计时器审计、可靠流迁移和两个真实 relay 节点场景。
- 真实 Chromium WASM：68 项通过，含两个原生 DataChunk 编码向量及有序迁移屏障。
- 真实 Chromium Service Worker ↔ 原生节点：1 项通过，含双向 RPC、两类流、64 KiB 负载与 relay 重连。
- JS：20 项通过，含 relay/适配/CLI 模板 13 项、DOM 轮换与路径分类 5 项、逻辑对端事件 2 项。
- DOM 与 SDK 严格 TypeScript 检查、worker/测试脚本语法及差异空白检查通过。
- 原生/服务端/核心所有构建目标以及浏览器 wasm32 所有构建目标 clippy 通过；浏览器保留两条既有格式借用建议。根 workspace 和浏览器 workspace 的 Rust 格式检查通过。
- 浏览器正式 release 构建通过；CLI 的 WASM/JS/worker 与发布源逐文件一致。真实 Chromium 加载 CLI 发布资产，验证五个 relay 导出存在并实际调用其认证信息入口。
- 同步资产后 `cargo check --offline -p actr-cli --all-targets` 通过；保留一条既有网络事件辅助函数 dead-code 提示。
- coturn：ChannelData 6/6、Send/Data 6/6、EVEN-PORT 12/12 消息收到。客户端建连早期的发送丢弃计数有单独日志，未将其描述成零丢包测试。

完整命令、编译与测试日志位于 `~/.codex/artifacts/actrium/wss-relay-20261011/`；`progress.md` 保留阶段检查点，`result.md` 为最终结果。早期只读错误日志属于已解决的环境问题，不能用来判断当前实现状态。

## 2026-10-12 端到端加密增补

- 原生和浏览器共用 `actr-relay-security`，采用 Snow 的 `Noise_XX_25519_ChaChaPoly_SHA256`。6 项密码会话测试通过，覆盖双向认证与确认、最大 8 MiB 消息分片、篡改/重放/明文拒绝、通道和身份替换、签名用途域、有效期、双向重新派生密钥与重连后的新临时密钥。
- 正式 AIS 接口集成测试通过，使用真实临时数据库和内嵌 Signer，验证完整身份与未过期私有续期证明、其他身份/随机证明/AId bearer 拒绝、证书有效期上限、删除或过期的续期证明拒绝、realm 禁用拒绝及可信公钥查询。
- Loran 正式认证适配器回归通过，验证只接受 relay 专用签名证书，拒绝 signaling AId bearer、其他用途域、错误身份、非规范 endpoint、超期和伪造签名；证书也不能用于 signaling 认证。
- 两个真实原生节点场景重新通过：加密 relay-only 双向 RPC/两类流/重连，以及加密 relay → RTC 直连的可靠流有序升级、relay 丢失后继续直连。
- 真实 Chromium Service Worker 与原生节点加密互通通过：双向 RPC、64 KiB 多记录分片、两类接收流、浏览器公开流 API 与重连。该历史测试检查了加密记录和已知业务明文；当前 hello 还包含与证书绑定的私有 relay 接入凭证。
- 浏览器 WASM 68 项回归与 JS 23 项回归通过；JS 新增密钥确认之前不报告业务就绪、加密拒绝关闭 channel、证书刷新重连和旧代际发送拒绝。

- 原生/AIS/通用 relay/密码会话合并回归 339 项通过。最后补充的有效证书缓存经真实节点复验：在暂停 AIS 证书签发并断开 relay 后，两端仍使用未过期证书完成新 Noise 会话与双向 RPC，保持原有恢复能力。
- 原生及服务端所有构建目标 clippy 通过，无新增 Rust lint；浏览器保留两条未改动文件的既有格式借用建议。正式 release WASM/JS 已构建并同步 CLI，真实 Chromium 验证八个 relay/加密导出及未注册客户端拒绝；同步检查通过。worker 侧可信 AIS I/O 与发布绑定格式保持兼容，重新执行实际加密互通通过。

增补日志和阶段检查点位于 `~/.codex/artifacts/actrium/wss-relay-e2ee-20261012/`。

## 独立 Astra 系统审查与修复

按要求使用 `gpt-6-astra`、独立上下文审查本次所有 TURN/STUN、WSS relay、加密、SDK、部署与发布配套改动。初审报告保留原始发现，修复后在同一独立上下文复核；报告分别为上述增补日志目录中的 `astra-review.md` 与 `astra-review-followup.md`。

初审发现 3 项 P1、2 项 P2；修复复核另发现 1 项 P2，现均已关闭：

| 发现 | 修复与验证 |
| --- | --- |
| 新包缺少可发布依赖版本及发布清单 | 所有新增依赖补齐版本，新 relay/security 包及协议 fork 纳入发布顺序、版本更新、锁文件和暂存清单。真实版本更新在隔离目录执行，发布依赖顺序检查通过 |
| 根 workspace 的 patch 无法传给独立 NAPI 或外部 SDK 消费者 | 改为显式有版本的 `actr-hyper → actr-webrtc → actr-rtc → actr-rtc-turn` 依赖链；标准 TURN 使用 `loran-turn-protocol`。RTC 桥 137 个库源码文件与维护的上游版本逐字节一致，只调整包与依赖清单。独立 NAPI 的 locked all-targets 检查通过 |
| 无数据库 relay 通过重载首次开启数据库功能导致 panic | 在候选任务启动前检查存储初始化状态，明确报告需要进程重启。真实 SIGHUP 验证进程和旧连接存活、新连接继续可用，也不创建数据库 |
| 候选 router 构建失败提前取消旧 relay | 候选使用独立取消令牌与 DropGuard，成功后才替换并取消旧令牌；失败恢复配置、secret 和 override store，collector 更新推迟至全部可失败步骤之后。运行时容量在配置层验证，另有绕过验证直接制造 builder 失败的回归 |
| 浏览器 RTC 临时断连恢复时遗漏逻辑路径恢复 | 保留 RPC channel-open 与暂时断连状态；重新 connected 只在 RPC 通道仍打开时恢复 rtc 路径。覆盖无再次 onopen 的恢复、relay 共存与失效、终止状态及通道关闭 |
| 打包工具在普通 CI 上从错误 target 目录取包 | 对同一临时 workspace、同一 Cargo 环境读取 metadata 的实际 target_directory。两项生产入口回归验证默认临时目录和配置目录均不会误取仓库旧包，同时保留浏览器 ABI 的继承元数据；纳入 release validation/CI |

修复后的定向验证：服务端/配置/UDP/加密会话/独立启动器 150 项、原生 relay/迁移/TURN 轮换 17 项、JS 25 项通过；严格 TypeScript、独立 NAPI all-targets、发布顺序和真实版本更新、release modes、asset validation、打包目录两项回归均通过。独立审查者还亲自复验了两种 SIGHUP 原始复现场景、4 项生产 TS 路径测试、发布依赖及归档闭包；不能把父侧验证记录算成其亲自执行的测试。

最终根 workspace all-targets check、原生/服务端/relay all-targets clippy、根与浏览器格式检查、CLI 运行时资产同步检查通过。已实际重新生成 35 个 `.crate`，规范化依赖不再含本地 path/patch，归档里的 RTC 桥 137 个 Rust 源文件与本地一致，浏览器 ABI authors 保留原值。独立 Astra 再次打开最新归档复核补丁链及元数据后，确认本次审查和修复复核完成，没有未关闭发现。对应日志为 `fixed-workspace-check.log`、`fixed-clippy.log`、`fixed-package.log`、`fixed-package-inspection.log` 与 `astra-followup-final-archives.log`；归档及 SHA-256 摘要保存在 `packaged/`。

这次审查没有发现已证实的端到端加密降级、身份伪造或 signaling bearer 泄漏路径；不将代码审查和互通测试描述为形式化密码证明或 RFC 合规认证。打包验证使用 `--no-verify`，只验证可生成归档与发布依赖闭包，编译由前述独立检查覆盖；本地验收没有上传 registry 或部署，变更通过 PR 交付。

## 复验入口

提交 PR 时移植至主分支，保留其 WebRTC 0.21.0 稳定版升级。三个客户端维护包的来源与精确协议依赖同步为 0.21.0；复制稳定版的 SCTP/TURN 修复前逐文件确认它们与本地补丁没有重叠。RTC 桥仍与稳定版上游库源码逐字节一致，异步 WebRTC 库源码在 rc.2 与稳定版间没有变化。主分支新增的通用 WebSocket sink 测试支持予以保留，并同步清理新测试夹具里已删除的旧 WS 地址缓存参数。本次移植的检查和回归单独记录于 `~/.codex/artifacts/actrium/turn-wss-prs-20261012/`，不把先前分支的通过记录冒充移植后验证。

移植后重新执行并通过：根 workspace 与独立 NAPI 的 locked all-targets check，原生/服务端/relay all-targets clippy，根与浏览器格式检查；服务端/配置/UDP/加密会话/独立启动器 159 项，原生 relay/路径迁移/TURN 轮换 27 项，真实 Opus RTP 回环 1 项，真实 Chromium WASM 68 项，Chromium Service Worker 与重新构建的原生节点加密互通 1 项，JS 25 项及严格 TypeScript 检查。正式 release WASM/JS、CLI 资产同步和真实 Chromium 发布资产加载检查通过。发布依赖顺序、真实版本更新、release modes、asset validation、打包目录两项回归通过，重新生成并检查 35 个归档，确认版本化依赖链、ABI authors 和全部 137 个稳定版 RTC 库源码。独立 Astra 另行复核 main 移植差异，没有未关闭发现；报告为 `main-port-review.md`。上述测试组存在重叠，不能相加为独立用例总数。

以下命令使用 Cargo 标准配置和环境变量，同机串行构建。需要自定义构建目录时设置 `CARGO_TARGET_DIR`，按本机安装情况提供 `PROTOC`。

```bash
cargo nextest run --offline -p loran-relay-protocol -p loran-relay

cargo nextest run --offline -p actr-protocol -p actr-config -p loran-platform -p loran-stun -p loran-turn -p loran-server --lib --test relay_only --test ice_only --test udp_protocol

cargo nextest run --offline -p actr-hyper --test node_start_smoke --test media_loopback -E 'test(relay_) or test(test_media_rtp_audio_loopback)' --run-ignored all --retries 0
```

浏览器为独立 workspace，先构建 WASM 测试，再用真实 Chromium 执行：

```bash
cd packages/actr-web
cargo test --offline -p actr-sw-host --lib --test relay_interop --target wasm32-unknown-unknown --no-run
```

`tests/e2e/relay-runtime.mjs` 接受生成的测试 `.wasm` 路径；提供与 lockfile 匹配的 `WASM_BINDGEN_TEST_RUNNER` 和浏览器 `TEST_BROWSER_PATH`。`--interop` 还需要 `ACTR_RELAY_FIXTURE` 指向根 workspace 构建的 `error-fixture`；可用 `PLAYWRIGHT_MODULE` 指定已安装的 Playwright。测试自动启动并关闭本地 fixture、HTTP 转发器和浏览器，不连接外部业务服务。

发布资产使用 `bash packages/actr-web/scripts/build-wasm.sh` 构建，再运行 `bash packages/actr-web/scripts/sync-cli-assets.sh --check` 验证。构建使用正式 release profile、规范 target 目录及匹配版本的 wasm-bindgen CLI；不临时调整 RUSTFLAGS 或 profile。

## 能力边界与迁移

- UDP TURN/STUN 的能力与未实现的可选扩展见[上游依赖说明](upstream-patches.md)。TCP/TLS/DTLS TURN 不在本阶段范围，也不宣称第三方合规认证。单实例只提供所配置的一种 relay 地址族；coturn 的随机双地址族 Refresh 扩展不作为跨族分配能力通过记录。
- WSS 提供逐跳 TLS，原生与浏览器 SDK 另提供双向身份认证的 Noise 端到端加密。relay 可见路由元数据、时序与大小，无法读取业务消息体或伪造通过认证的消息。私钥、续期证明和 signaling AId bearer 不交给 relay；配置中的 AIS/HTTPS 是身份信任根。通用 relay 服务只转发 bytes，其他独立客户端需自行提供端到端协议。
- relay 会话在进程内配对；多实例需要端点亲和或分开 namespace，不提供 mesh。
- 已有配置重载失败时保留运行中的配置与 relay 连接；静态 relay 首次开启数据库功能需要重启进程，不能通过 SIGHUP 初始化此前未启用的存储。
- 可靠流屏障保证健康路径迁移时的接收队列顺序，不确认应用处理完成，也不把故障后交付不确定的数据自动重放。
- 先升级 AIS 与 Loran 并启用服务端 relay，再统一升级需要互通的 SDK，删除旧应用 `[websocket]` 配置。新增签发接口和专用接入凭证必须同步部署；SDK 不向旧明文 relay 降级。新浏览器发送 protobuf DataChunk，接收兼容旧前缀；旧浏览器不能接收新格式，不承诺任意版本双向流兼容。
- 真实音频 RTP 回环和视频分发回归已执行；未将其扩大为全部音视频编解码器或浏览器设备组合的认证。
