# Loran TURN/STUN 互操作与滥用测试

状态：Realm v1 可复现验收说明。这里区分自动回归、本机真实浏览器验证和发布前外部
矩阵，避免用一个 localhost 成功案例冒充公网生产认证。

## 1. 自动 UDP 协议测试

```bash
cargo test -p loran-turn --test udp_interop -- --nocapture --test-threads=1
cargo test -p loran-stun -- --nocapture --test-threads=1
```

TURN integration test 使用独立的 `turn` client API 连接 Loran server，覆盖：

1. STUN Binding 和 mapped address；
2. 带 Realm/Actr Facility Provider/audience/Actor scope 的时效 TURN credential；
3. TURN Allocate；
4. CreatePermission；
5. client → peer 与 peer → client 双向 UDP relay；
6. relay port 必须落在配置范围内；
7. 4,096 个交替 malformed STUN/ChannelData-looking datagram 后，合法 allocation
   必须在 5 秒内成功。
8. 关闭某 Realm 的 allocation 只关闭该 Realm credential 建立的 allocation
   （`allocations_of_a_closed_realm_are_closed`）。

STUN server test 在 4,096 个畸形 datagram 持续发送时重试合法 RFC 5389 Binding
request，要求在 2 秒总窗口内返回正确的 transaction ID 和 XOR-MAPPED-ADDRESS。
这是有界的 starvation/健壮性回归，不是容量或 DDoS 防护声明。

## 2. Chromium relay-only 验证

启动临时 TURN server 和只监听 loopback 的测试页面：

```bash
cargo run -p loran-turn --example browser_interop
```

打开命令打印的 `ACTR_TURN_BROWSER_URL`。页面会：

1. 创建两个 `RTCPeerConnection`；
2. 将两端 `iceTransportPolicy` 都设为 `relay`；
3. 只配置临时 Loran TURN UDP credential；
4. 完成 offer/answer 和两个 TURN allocation；
5. 通过 DataChannel 发送 `ping` 并接收 `pong:ping`；
6. 从 WebRTC stats 验证选中 candidate pair 的两端都是 `relay`、协议为 UDP。

成功结果必须形如：

```json
{
  "status": "passed",
  "message": "pong:ping",
  "relayCandidates": 2,
  "selectedPair": {
    "state": "succeeded",
    "localCandidateType": "relay",
    "remoteCandidateType": "relay",
    "protocol": "udp"
  }
}
```

fixture 每次运行生成独立 TURN secret，credential 十分钟过期，只监听
`127.0.0.1`，不把测试 secret 写入仓库。

## 3. 2026-07-29 本机证据

- TURN UDP integration：2 passed，0 failed；
- STUN unit/integration：9 passed，0 failed；
- Codex in-app Chromium WebRTC：`pong:ping`，2 个 relay candidate，选中 pair 为
  relay/relay UDP。

这些结果证明本实现与 Chromium WebRTC 栈及 `turn` client 的基本 UDP 协议通路可用，
并证明有界畸形流量不会直接饿死合法请求；它们没有证明公网、复杂 NAT、TCP/TLS TURN、
高并发或持续攻击下的行为。

## 4. Production-ready 前的外部矩阵

至少补齐：

- Chrome/Chromium、Firefox、Safari；
- iOS/Android 原生 WebRTC；
- coturn `turnutils_uclient` 或其他独立 RFC 5766 client/server 对照；
- full-cone、restricted、port-restricted、symmetric NAT 和双 NAT；
- UDP 被屏蔽时的 TURN/TCP 与 TURN/TLS；当前 v1 若尚未实现，应明确列为能力缺口，
  不能以 UDP 结果代替；
- 过期、错误 Realm/Actr Facility Provider/audience、重放和错误 MESSAGE-INTEGRITY；
- allocation/permission/channel bind 洪泛、超大包、随机包、慢速连接和 relay 带宽
  耗尽；
- 持续测试下的 CPU、内存、FD、socket、队列长度、丢包率和 P95/P99。

外部矩阵必须保存环境、客户端版本、网络拓扑、命令、原始日志和失败样本。独立安全
审查者应复核测试是否足以关闭相关 P0/P1，而不能只接受本文件中的自证结论。
