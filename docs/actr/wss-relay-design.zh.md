# Loran WSS relay 与对端连接迁移

## 范围与边界

Loran 保留通用标准 UDP TURN/STUN，TCP/TLS TURN 可以暂缺。新增的 WSS relay 复用原 Loran 启动入口与 HTTP/TLS 监听器，通过配置启用。两类 relay 并存：原生 WebRTC 音视频继续通过 ICE 直连或 TURN；RPC 与 DataChunk 可以通过 WSS relay 快速建立可用通道。

应用节点只主动连接 Loran，不再监听对端 WS/WSS 端口、公布自己的 WebSocket 地址或配置对端通信的 TLS 证书。普通客户端访问其他 WebSocket 服务的能力不属于这次移除范围。

## 分层

- `loran-relay-protocol`：平台无关的版本化协议；不包含业务身份、注册流程或认证实现。
- `loran-relay`：有界会话路由、连接生命周期与外部认证/授权接口；只处理 namespace、opaque endpoint、channel 和 bytes。
- Loran 适配层：从配置选择认证模式，将签名业务凭据映射为通用 Principal，公布 relay 地址。
- SDK 适配层：自动发现 relay，读取当前凭据，建立并维护出站连接，将 relay 会话接入现有 RPC/流数据通道。
- `actr-relay-security`：原生与浏览器共用的 Noise 会话和凭证验证；只接收外部 endpoint、证书与受信任签发公钥，不依赖 Actrium 身份类型或 relay 服务实现。

核心接口由外部认证回调返回 namespace、endpoint、可公开的认证元数据与过期时间。核心强制 namespace 隔离；业务授权由外部策略补充。客户端不能通过数据帧改变自己的路由身份。

## v1 协议

底层遵循 [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html)，WebSocket 子协议为 `loran-relay.v1`。这是一套自定义应用 relay 协议，不声称兼容 TURN 或 NetBird relay 协议。

一个节点维护一条 WSS，复用多个会话。控制消息使用带 `type` 的 JSON 文本帧；数据帧为八字节大端 channel 编号加不透明负载。编号由服务端生成，限定在 JavaScript 可精确表示的整数范围。

1. 客户端发送 `hello(endpoint, token)`，服务端验证后返回 `welcome(session)`。凭据不放在 URL 中。
2. 发起端发送 `connect(request, peer)`；服务端向相同 namespace 下的对端发送 `incoming(channel, peer, metadata)`。
3. 对端完成认证检查与接收通道安装后发送 `accept(channel)`。服务端向两端发送 `connected`，之后才允许转发该 channel 的数据。
4. 拒绝、会话关闭、对端掉线与等待超时均显式通知。断线后原 channel 全部失效，重新认证并建立新会话。

重复 endpoint 登录替换旧连接；旧连接的清理操作通过 session 比较，不能移除新连接。数据发送者必须是 channel 的两端之一。离线端点不缓存待投递业务数据。

## 选路与交付

SDK 并发尝试 relay 会话与 WebRTC。首个真正可用的通道开始通信；WSS OPEN 或 ICE connected 单独都不构成业务就绪。直连 WebRTC DataChannel 就绪后优先直连。WebRTC 的直连与 TURN 中继需要分别观测，避免把后者报告成直连。

切换只影响后续发送，保留原路径接收在途响应。失败时依赖现有交付进度判断；已经提交或交付结果不确定的非幂等 RPC 不因换路自动重放。Relay 会话保持可供回退，断线后采用有上限且带抖动的退避重连。

## 安全与资源

认证失败关闭连接；认证有截止时间，过期凭据不能维持无限期会话。限制并发连接、每端点会话、帧大小、出站消息数与字节数、写入时间及未接受会话的等待时间。慢读端点耗尽队列时关闭该端点，使其他节点继续工作。Ping/Pong 与取消令牌负责失活检测和退出。

WSS 为客户端至 relay 的每一跳提供 TLS 保护。SDK 在其内部建立经过双向身份认证的端到端 Noise 会话；RPC、流数据、迁移屏障和确认均经过认证加密。relay 收到密文，不持有会话密钥，不能读取或修改业务消息体。这个边界对应 WebRTC 经 TURN 时由端点 DTLS/SRTP 保护业务内容的分层；TURN 协议和 WSS 本身都不自动为任意应用负载提供端到端加密。

relay 仍可见 namespace、endpoint、连接关系、时序和帧大小，并能丢弃数据或拒绝服务。身份信任根是配置中的 Issuer 及其 HTTPS 连接；relay 返回的公钥不能成为信任根。与 WebRTC 的受信任身份/信令基础设施一样，这个边界不涵盖 Issuer 或通信端点被攻破。

## 端到端认证与加密

原生和浏览器使用同一个 Rust 引擎，经 `snow` 实现标准 `Noise_XX_25519_ChaChaPoly_SHA256`：X25519 密钥协商、ChaCha20-Poly1305 认证加密、SHA-256 握手摘要。每个逻辑 channel 使用新临时密钥，握手摘要绑定排序后的双方完整 endpoint 和服务端 channel 编号。channel 重建不能复用旧密文；会话中每个方向独立维护有序 nonce，并定期重新派生加密密钥。详见 [Noise 规范](https://noiseprotocol.org/noise.html)。

1. The SDK generates a private random relay admission secret and sends its domain-separated SHA-256 commitment, X25519 public key, canonical endpoint and private renewal token to `POST /issuer/relay-identity`, together with a signed `relay_identity` Realm client proof. The Issuer checks current membership, Actor binding, the unexpired renewal token and Actor authorization before signing. The admission secret itself is not sent to the Issuer.
2. The domain-separated `ACTR-RELAY-IDENTITY-V2` certificate binds the endpoint, Actor type, Realm client key, public key, admission commitment and expiry. The SDK sends the certificate and its private admission secret to the relay over the authenticated WebSocket; peers receive only the certificate. The relay verifies the commitment before registering or replacing an endpoint. The signaling bearer, renewal token and Noise private key are never sent to the relay.
3. Noise XX verifies each peer certificate against its endpoint, actual Noise public key and the Realm-scoped Issuer key. Unknown keys are fetched through `GET /issuer/relay-key/{realm}/{key_id}`; keys supplied by the relay are never trusted.
4. 双方完成额外的加密密钥确认后，SDK 才注册业务通道、报告 relay 就绪并允许 RPC/流发送。仅仅打开 WSS、接受 channel 或完成部分握手都不能提前发送业务明文。
5. SDK 在证书临近过期时更新本地密钥和证书，重新建立 relay 会话。每次重连使用新 Noise 临时密钥。握手超时、证书过期、签名错误、篡改、乱序或重放均关闭受影响会话；没有旧明文协议回退。

外层 `loran-relay.v1` 转发格式不变。SDK 在八字节 channel 之后使用带 `ACTRE2E1` 标识的加密记录；每条记录的 Noise 部分至多 65,535 字节，大业务消息拆为多条有序认证加密记录，在接收端完整重组后交给既有 API。业务消息上限保留 8 MiB，收发队列和分片缓存有界。浏览器密钥与续期证明由 Service Worker/WASM 持有，不经过 DOM 消息桥。

Loran 的 `auth = "aid"` 配置名称保留，外部适配器验证专用签名证书与私有接入凭证，并拒绝 signaling AId bearer。创建 channel 时，relay 根据签名证书中的双方 Actor 类型异步查询 Realm ACL；查询失败时拒绝配对。通用 relay 核心与静态认证部署继续只转发字节；独立使用它的其他客户端负责选择自己的端到端协议与身份信任根，不能把通用转发器的存在当成任意负载已加密。

## 验收

- 通用核心不依赖 Actrium 协议或业务包；独立协议/真实 WebSocket 测试覆盖认证、隔离、会话握手、双向数据、断线代际、超时与背压。
- Loran 原入口可配置启用 relay，UDP TURN/STUN 独立部署和已有互通测试继续成立。
- 原生与浏览器 SDK 不要求应用部署 WS server；RPC 请求、响应与流数据可通过 relay。
- 两条路径并发建立、直连升级、回退及在途响应均有验证；音视频路径保持原生 WebRTC/TURN。

## 可靠流跨通道切换

原生与浏览器 SDK 已加入方向独立的迁移屏障。每个目的端的 `StreamReliable` 发送串行化，首次发送直接使用当前最快可用的路径。当更高优先级路径就绪，SDK 先在旧可靠流通道中发送保留的 `DataChunk` 控制记录。对端的顺序接收任务在此前数据已进入接收队列后，使用 `RpcSignal` 返回确认；发送方收到正确对端和随机 nonce 对应的确认后，才允许后续可靠流使用新路径。接收端不把屏障或确认交给应用处理，通用 relay 仍只转发不透明负载。

屏障确认的整体预算为 3 秒；不支持控制记录或超时的对端继续使用健康的原路径，30 秒内不重复探测。RPC 与 `StreamLatencyFirst` 按实时路径优先级发送，不等待可靠流屏障。在等待屏障时取消发送，不会提交该次应用数据；取消和超时会清理待确认状态，且节点的待确认数有上限。

该屏障保证健康路径迁移时的接收队列顺序，不确认应用处理完成，也不提供故障后的自动重传。原物理路径明确失效时保留已有交付不确定语义，不自动重放应用数据。旧路径只是暂时不可发送时，不跳过屏障；返回可证明未提交本次数据的错误，由现有操作预算和恢复逻辑处理。原生回归和真实 Chromium WASM 回归均已通过；真实原生节点的 relay → RTC 直连迁移保持了 32 个可靠流分块的顺序。

## SDK 升级方式

浏览器旧流数据使用长度前缀，原生端使用 protobuf `DataChunk`。新浏览器 SDK 已统一发送 protobuf，并保留接收旧浏览器格式的兼容。旧浏览器 SDK 无法接收新格式，因此本阶段采用受控 SDK 迁移，不宣称任意版本之间的双向流兼容；无需向应用增加格式、relay 地址或传输策略选项。

部署时协调升级 Issuer、Loran relay 和需要互通的原生及浏览器 SDK；V2 relay 身份和接入凭证与旧版不兼容，分阶段升级期间旧客户端无法接入新版 relay。删除应用运行配置中的旧 `[websocket]` 段后重新部署。新版两端的 RPC 与流通过 SDK 自动选择路径。旧浏览器到新版浏览器的旧格式接收兼容只用于迁移接收；新版到旧版的流发送不在兼容范围，旧 SDK 也不能使用新的 relay 快速通道。保留的 protobuf `ws_address` 字段只维持消息结构兼容，不代表恢复旧应用监听通道。

浏览器旧 bytes-only API 没有调用方提供的 sequence，适配为 `DataChunk.sequence = 0`；迁移屏障承担跨路径接收顺序，应用不依赖这个缺省字段排序。

## 独立发布与配置重载

标准 TURN 与原生 WebRTC 使用上游 crate：TURN 相关 crate 固定为同一已发布版本，RTC 相关 crate 固定为包含修复的 Git commit。基于源码路径的 workspace 可继承这些依赖；使用已发布 SDK 的消费者仍需配置 RTC patch。具体 commit、来源覆盖及限制见 [上游依赖说明](upstream-patches.md)。新增 relay/security 包继续进入实际版本更新与发布顺序。

配置重载先校验容量并构造候选 router，候选任务使用独立取消令牌。失败只取消候选任务并恢复配置状态；成功后才切换并取消旧任务。静态、无数据库的 relay 首次开启 admin 等数据库功能需要进程重启，重载明确拒绝并保留原连接，避免在未初始化的存储上启动业务服务。

## 首次验收与加密增补（2026-10-11 / 2026-10-12）

实现、配置、部署说明及回归用例已落盘。规范构建目录的只读问题已解除；根 workspace 所有构建目标检查通过，协议与独立启动器、原生传输、真实 Chromium WASM、Service Worker 与原生节点互通均已执行。通用 UDP TURN 的 coturn ChannelData、Send/Data 和 EVEN-PORT 三组互通通过；原生 TURN 凭据轮换、ICE 重启及 WebRTC 音频 RTP 回环通过。

原生节点测试验证了 relay 先通信、真实 RTC 直连就绪后升级、32 个可靠流分块跨路径有序，以及 relay 断线后直连继续通信。Service Worker 测试在禁止 RTC 转发时验证双向 RPC、64 KiB 数据、两类流和 relay 重连；本机首次对端确认的 relay RPC 为 106 ms，属于功能测试观测值，不是公网延迟承诺。浏览器的迁移屏障、候选路径分类和凭据轮换分别有 WASM 与 JS 回归。

完整证据、复验入口和能力边界见 [验收审计](wss-relay-acceptance.zh.md)。本实现提供所列 UDP 标准能力及自定义 WSS 数据 relay，不代表获得第三方 TURN 合规认证，也不提供跨实例 relay mesh 或故障后无损重放。

浏览器正式 release WASM 和匹配的 wasm-bindgen JS 已重新构建并同步至 CLI 嵌入目录；worker 与 SDK 源码一致。逐文件同步检查及真实 Chromium 发布资产加载、relay 导出调用检查通过，避免仅更新源码而发布旧运行时。

2026-10-12 补齐原生/浏览器共用的端到端 Noise 会话、私有续期证明签发、relay 专用接入凭证及密钥确认就绪门控。加密与 AIS 安全测试、339 项原生/AIS/relay 回归、真实 Chromium Service Worker/原生节点的线上密文检查、正式发布构建与资产加载通过。临时 AIS 签发故障期间，SDK 可复用未过期证书重连，每次 channel 仍生成新临时密钥；证书到期后禁止继续通信或明文降级。验收细节见加密增补记录。
