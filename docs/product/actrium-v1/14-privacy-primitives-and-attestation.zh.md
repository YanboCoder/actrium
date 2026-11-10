# 14. 隐私原语、远程证明与可信算力

> 安全语义以 [docs/security](../../architecture/security/actrium/README.zh.md) 为准。本章只写原语、证明和算力如何进入现行评定，不另定义颜色或图根。

## 1. 目标

应用绿色不能来自单一技术或单一厂商，而必须来自一组可组合、可验证、可撤销的隐私原语：

```text
声明（想允许什么）
  ↓
强制执行（实际上只能做什么）
  ↓
Evidence / Attestation（远端如何知道约束已生效）
  ↓
Policy Appraisal（当前数据是否可以进入）
  ↓
Privacy Graph Aggregation（整条 Actor 数据路径是否成立）
```

本章回答：

- 「Actor 网络全过程加密」具体保证什么；
- WASM、Native Capsule 与 Confidential Computing 分别解决什么问题；
- Agent 使用 CPU、GPU、NPU 或边缘算力时如何证明运行环境；
- 不同厂商证明如何映射为统一 Actrium claims；
- 密钥何时可以释放；
- 多项证据如何进入 `ActorAssurance` 的事实项（`CONTROLLED / UNKNOWN / UNCONTROLLED`）。

## 2. 准确的产品承诺

### 2.1 不使用「数据始终是密文」

普通计算必须在某个执行边界内使用明文。除非使用 FHE、MPC 等专用密码学计算，Actrium 不应声称数据在每个时刻都是密文。

推荐表述与 [总览](../../architecture/security/actrium/01-overview.zh.md) 一致：

> 数据在受保护链路和受保护存储上以密文存在；明文只应出现在身份已知、边界满足要求的处理环境中。绿色只保证当前就绪切面，新配置只约束新数据。

「全生命周期保护」分为：

| 数据状态 | 主要机制 |
|---|---|
| in transit | Actor Secure Channel、AEAD、身份绑定、前向保密、防重放 |
| at rest | Workspace envelope encryption、设备 Keystore/KMS、备份密钥策略 |
| in use | WASM/Native confinement；高敏数据按 policy 要求 TEE/CC |
| on output | 类型化结果、数据标签、declassification、速率/聚合/审计 |

### 2.2 「网络全过程加密」的边界

Actr 必须保证每一条网络 Actor edge：

- 双向认证；
- 机密性和完整性；
- 防重放；
- 会话密钥与两端 Actor/runtime 身份绑定；
- 使用临时密钥并支持重协商；
- signaling、TURN 和普通 relay 不持有 payload key；
- 不能把传输降级为未认证明文。

但必须区分：

```text
Actor A ── encrypted edge ── Actor B ── encrypted edge ── Actor C
```

如果 B 要处理数据，B 就是明文执行端，也是隐私闭包中的必要 Actor。A→C 不能因为两段链路都加密，就跳过对 B 的评估。

只有不处理 payload 的 signaling/TURN/relay 才不是数据执行端。它们仍可能看见 IP、时序、包长和连接关系，因此「payload 加密」不等于「元数据匿名」。

## 3. 隐私原语矩阵

| 原语 | 主要对手 | 能证明什么 | 不能证明什么 |
|---|---|---|---|
| Actor Secure Channel | 网络窃听、篡改、重放、relay | payload 只被经认证端点解密 | 端点收到明文后不会泄露 |
| `.actr` 签名与 provenance | CDN、镜像篡改、供应链替换 | 运行 artifact 的来源和 digest | artifact 本身没有恶意逻辑 |
| WASM capability sandbox | 恶意/错误 workload | imports、内存边界和可能出口可枚举、可强制 | Actr Host/内核不会偷看；允许出口的接收者不会滥用 |
| Native Capsule | 第三方 native workload | native 进程无法绕过 broker 使用原始资源 | 内核零漏洞；与 WASM 同等可移植、可验证 |
| TEE / Confidential Computing | 云管理员、宿主机、Hypervisor 等基础设施对手 | 指定执行边界中的 data in use 受到硬件隔离 | workload 本身不会主动外泄 |
| Remote Attestation | 伪造运行环境、错误版本 | 某 Attester 对环境状态给出了可验 Evidence | 该状态自动满足业务 policy |
| KMS / Key Broker | 未经认可环境提前拿到明文 | 只有通过 appraisal 的实例得到数据密钥 | 获得密钥后的 workload 行为安全 |
| Workspace encryption | 磁盘、快照、备份泄露 | 静态数据没有密钥无法读取 | 解密后的运行期数据不泄露 |
| 数据标签与 declassification | 合法通道上的过度输出 | 哪些数据允许以何种粒度离开 | 通用算法自动符合业务最小化 |
| DP / MPC / FHE | 特定统计或多方计算对手 | 在限定模型下减少或避免明文暴露 | 通用、低成本替代所有普通计算 |

结论：

```text
WASM ≠ CC
CC ≠ Attestation
Attestation ≠ Authorization
Encryption ≠ Endpoint Privacy
```

最佳的高隐私组合通常是：

```text
signed WASM workload
inside attested confidential execution environment
+ Actor end-to-end channel
+ attestation-gated key release
+ Host-enforced capability/egress closure
```

## 4. 内部 Assurance Vector

对用户仍只显示绿 / 黄 / 红（或无法建立），但内部不能只存一个布尔值。

每个 Actor runtime 至少产生：

```text
ActorAssuranceVector {
  identity
  artifact_provenance
  workload_confinement
  compute_confidentiality
  channel_security
  storage_protection
  egress_closure
  output_control
}
```

每个维度的评估结果：

```text
pass | indeterminate | fail | not-required
```

映射：

| 维度结果 | 出口事实 |
| --- | --- |
| 所有 policy-required 维度均 pass | `CONTROLLED` |
| 至少一个 required 维度 indeterminate | `UNKNOWN` |
| 至少一个 required 维度 fail | `UNCONTROLLED` |
| 非当前数据/操作所需 | 不参与此次汇聚 |

展示：全部 `CONTROLLED` 且 `Ready` 且通道有效 → 绿；无红而有未知 → 黄；任一 `UNCONTROLLED` → 红；切面未就绪 → 不展示颜色。

不是所有 Actor、所有数据都强制要求 CC。例如 public 数据可以允许在普通 WASM Actr Host 上运行；secret 数据的组织策略可以要求 `compute_confidentiality=pass`。

### 4.1 绿标主体：Application Instance Snapshot

结论绑定当前就绪切面，而不是笼统 Program，也不是短暂 transport session。Front / Service 只用于定位图，不进入 `structureDigest` 当根。

```text
ApplicationStateDigest =
  H(actrium/application-state/v1,
    structureDigest,          # EncGraph
    principal,
    MerkleRoot(ActorId + ActorStateDigest + H(ActorAssurance)))
```

`ActorStateDigest` 变化、证据或策略变化，都会使旧 `ApplicationAssurance` 失效。Tab 重连、direct/TURN 切换（中继始终无明文）不改图。

Program release 可以提供经过审核的静态 Contract 和 evidence，帮助形成结论，但 Catalog 只能显示「支持哪些可验证 profile」，不能代替当前切面。

### 4.2 Program Privacy Contract

应用实例结构可以动态变化，但其结构类型和能力上界仍必须静态：

```text
static graph grammar
→ dynamic actor instances
```

Contract 至少冻结：

- 允许出现的 Actor role/type/release 或 assurance profile；
- required/optional、cardinality 和可建立的 edge；
- 每条 edge 可携带的数据类别和操作；
- Actor/network/Workspace/storage/compute/audit capabilities；
- terminal egress、purpose、retention 和 consent；
- 所需 sandbox、CC、Verifier 与可信 Privacy Domain profile；
- output/declassification 规则；
- 未知或失效证据的动作。

示意：

```toml
[actrium.privacy_contract]
schema_version = 1
contract_id = "notes.example/team-notes@1.4.2#privacy"
package_digest = "sha256:..."

[[actrium.privacy_contract.actor_role]]
role = "service"
actr_type = "notes.example/team-notes-service@1.4.2"
cardinality = "1"
required = true

[[actrium.privacy_contract.actor_role]]
role = "ai-inference"
cardinality = "0..n"
required_assurance_profile = "confidential-ai-v1"

[[actrium.privacy_contract.edge]]
source = "service"
target = "ai-inference"
data_classes = ["private"]
capabilities = ["actor.call:infer.v1"]
```

声明不是说明文字，而是 Actr Host 能发放能力的上界：

```text
Effective Capability =
  Declared Contract
  ∩ User/Organization Policy
  ∩ Host Policy
  ∩ Instance Grant
```

未声明 Actor role、edge、audit sink、terminal、文件、网络或计算能力不得获得 handle。运行时不能用「先连接、再报告」扩张 Contract。

Contract 的作用是让所有图变化在 capability 创建前可拦截。它是边界，不是应用绿色本身。

### 4.3 状态变化

不做全图分布式提交。变化 Actor 本地原子切换 `ActorStateDigest`，同步废除旧密钥、旧通道和旧 Actor Grant，进入保障挂起。事件可以丢失。`ActorState` 含单调修订号：配置回滚也得到新状态根，废除是持久的，旧权力不因回滚复活。

```text
Change → SUSPENDED → 取证 / 评定 → Resume（须有当前 AA）→ 尝试 Publish
```

挂起期间不得处理应用数据，也不得向其发送应用数据。不依赖变化节点的成员可按 `LocalContinue` 继续自身工作，但顶层不得展示应用绿色。旧数据不得改写世代以使用新出口。

控制面（取证、attestation、UI、同意）可以继续；不得夹带业务数据。

### 4.4 降级比较与恢复

结构或状态变化不必然打扰用户。相对旧结论：

```text
颜色未减弱且策略允许
  → 自动恢复相关工作（仍须 Ready 才展示绿）

颜色变黄或变红
  → 保持阻断
  → 显示精确 diff
  → 请求风险接受（不改颜色）

通道无效或触碰平台硬底线
  → 阻断
  → 用户不能覆盖
```

「更弱」不能只看颜色。任一 required 维度从 `pass→indeterminate`、profile 降级、所信验证者改变、terminal / audit / retention 扩张，均为降级。一个维度增强不能抵消另一个变弱。

风险接受绑定 Principal、对象、原因、范围、当时的状态根和策略版本。同意只能让实例在黄 / 红下继续跑，不能把缺失证明变成绿。

### 4.5 哪些事件算结构变化

会改变结构身份或当前状态（因而旧结论失效）：

- Actor/Privacy Domain 加入、替换、迁移或移除；
- Actor workload/config/Host/compute/accelerator assurance 变化；
- 新增或扩大 Actor edge、data class、purpose、terminal；
- audit/log/telemetry sink、字段、读取主体或 retention 变化；
- Workspace 或 Workspace Provider 变化；
- capability 或 declassification 扩张。

不改变图结构（仍可能因证据过期而重评）：

- 相同 Actor/runtime/evidence 下的 transport reconnect；
- direct/relay/TURN 路径切换且 relay 始终无 payload key；
- 不影响隐私 claims 的健康、延迟或扩缩容指标变化。

图不变也不代表结论永远不变。证据到期、验证者 / TCB / 凭据撤销、策略更新会使旧 `ActorAssurance` 失效，必须重评。展示仍须 `Ready`。

### 4.6 UI

| 位置 | 展示 |
|---|---|
| Catalog/安装页 | Program 支持的 privacy profiles、Contract 和静态审核信息，不显示实例绿标 |
| App Library | 每个应用最近一份 `ApplicationAssurance`、状态根和更新时间（须标明是否仍 Ready） |
| 活跃 Tab 地址栏 | 当前就绪切面的绿 / 黄 / 红；重评或挂起时显示「正在重新验证 / 通信已阻断」 |
| 降级确认页 | 新增/替换 Actor、audit/terminal 变化、最弱维度、影响的数据和功能 |
| 详情面板 | 当前 Actor/Edge、Assured Domain、Evidence、授权历史与到期时间 |

## 5. Data Policy 决定所需证据

示意：

```toml
[[actrium.privacy.data_policy]]
class = "public"
required = [
  "identity",
  "artifact_provenance",
  "channel_security",
  "egress_closure"
]

[[actrium.privacy.data_policy]]
class = "private"
required = [
  "identity",
  "artifact_provenance",
  "workload_confinement",
  "channel_security",
  "storage_protection",
  "egress_closure"
]

[[actrium.privacy.data_policy]]
class = "secret"
required = [
  "identity",
  "artifact_provenance",
  "workload_confinement",
  "compute_confidentiality",
  "channel_security",
  "storage_protection",
  "egress_closure",
  "output_control"
]
```

Publisher 可以声明最低需求，用户/组织可以收紧，不能由 Publisher 放宽平台或组织底线。

## 6. Actor Secure Channel

### 6.1 两层保护

Actr 网络应同时提供：

1. transport protection：例如 TLS 1.3、QUIC 或 WebRTC DTLS；
2. Actor Secure Envelope：绑定 Actor 语义的端到端 AEAD。

第二层的作用是：

- 即使 signaling、TURN、代理或某段 transport 被替换，relay 仍不能获得 payload；
- 把密码学端点绑定到 `ActorId + runtimeInstanceId`；
- 把 privacy policy、attestation result 和具体 session 绑定在一起；
- 防止仅凭一张合法 Actor credential 把流量引到另一 workload。

### 6.2 Handshake 绑定

握手 transcript 至少覆盖：

```text
protocolVersion
sourceActorId
targetActorId
sourceActorStateDigest
targetActorStateDigest
sourceRuntimeInstanceId
targetRuntimeInstanceId
sourceCredentialDigest
targetCredentialDigest
sourceEphemeralKey
targetEphemeralKey
requestedServiceActorId
policyDigest
attestationResultDigest
channelNonce
roles
```

双方当前 `ActorStateDigest` 必须进入联合 transcript 并被双向认证：只对本方字段做 MAC 不足以抵抗旧状态问候重放（见 [安全协议](../../architecture/security/actrium/03-protocol.zh.md) §3 与形式模型 F-CHAN-01）。任一方 `ActorStateDigest` 变化后，旧通道不得承载应用数据，须针对新状态重新握手。

Actor Actr Host 必须证明持有 Actor key 和临时会话 key。若目标运行在 TEE 中，临时解密公钥必须由 TEE 内生成，或被 TEE 安全持有，并进入 attestation Evidence/Result。

### 6.3 数据面

每个 encrypted record：

- 使用 AEAD；
- 带 session id、direction、epoch、sequence；
- 将 Actor edge id、数据类别、消息类型作为 authenticated additional data；
- 拒绝 nonce/sequence 重用；
- 检测重复、乱序和 epoch 回退；
- 定期或按数据量 rekey；
- private/secret 的状态改变请求不得通过不具备等价防重放语义的 0-RTT 发送。

### 6.4 Channel Evidence

```json
{
  "schemaVersion": 1,
  "edgeId": "edge_01...",
  "sourceActorId": "<ActorId>",
  "targetActorId": "<ActorId>",
  "sourceRuntimeInstanceId": "runtime_01...",
  "targetRuntimeInstanceId": "runtime_02...",
  "suite": "actr-channel-v1",
  "payloadProtection": "actor-aead-envelope",
  "forwardSecrecy": true,
  "antiReplay": true,
  "relayCanDecrypt": false,
  "transcriptDigest": "sha256:...",
  "peerAttestationResultDigest": "sha256:...",
  "sessionKeyThumbprint": "sha256:...",
  "issuedAt": "2026-07-29T12:00:00Z",
  "expiresAt": "2026-07-29T12:10:00Z"
}
```

「TLS 已连接」本身不足以生成 Actor channel `pass`；还必须完成 Actor 身份与 runtime/session 绑定。

## 7. Confidential Computing

### 7.1 它解决什么

CC 的目标是保护 data in use，使代码和数据在硬件支持、可证明的 Trusted Execution Environment 中运行。

其主要价值是把以下主体从隐私信任边界中移出或削弱其权限：

- 云平台管理员；
- Actr Host OS / Hypervisor；
- 同机其他 VM、容器或进程；
- 部分物理与设备攻击者。

具体能排除哪些对手，取决于硬件和 profile，不能统一写成 `cc=true`。

### 7.2 它不解决什么

CC 内运行的恶意 workload 仍然可以把数据发送给已开放的网络目标。因此：

```text
compute_confidentiality = pass
```

不能替代：

```text
workload_confinement = pass
egress_closure = pass
```

同理，WASM 出口闭合不能证明云管理员无法读取普通 Actr Host 内存。

### 7.3 Canonical Compute Claims

不同 TEE/CC backend 必须映射为统一 claims，而不是暴露一个厂商布尔值：

```json
{
  "profile": "actrium.compute-assurance.v1",
  "environmentType": "confidential-vm",
  "platformVendor": "vendor-id",
  "platformModel": "model-id",
  "teeType": "vendor-tee-profile",
  "isolationScope": "vm",
  "confidentialityScope": ["cpu-memory", "guest-state"],
  "integrityScope": ["firmware", "boot-chain", "guest-image"],
  "debugDisabled": true,
  "migrationProtection": "measured-and-policy-bound",
  "hostAdminExcluded": true,
  "workloadDigest": "sha256:...",
  "runtimeDigest": "sha256:...",
  "guestImageDigest": "sha256:...",
  "ephemeralKeyThumbprint": "sha256:...",
  "accelerators": [],
  "freshness": {
    "nonce": "...",
    "issuedAt": "2026-07-29T12:00:00Z",
    "expiresAt": "2026-07-29T12:05:00Z"
  }
}
```

不得把 vendor/product name 本身解释为 `pass`。只有对应 Verifier profile、Endorsements、Reference Values、TCB 状态和 freshness 均满足 policy 时才通过。

## 8. RATS 角色模型

Actrium 采用标准远程证明角色：

| 角色 | Actrium 中的实体 | 职责 |
|---|---|---|
| Attester | Actr Host TEE、设备、GPU/NPU、可信 launcher | 产生关于 Target Environment 的 Evidence |
| Verifier | Realm/组织/本地受信 Verifier 与 vendor adapter | 验证签名、证书链、测量值、参考值、撤销与 freshness |
| Relying Party | 发数据的 Actor Actr Host、Privacy Agent、Key Broker | 根据 Attestation Result 和本地 policy 做授权决定 |

需要严格区分：

```text
Evidence
  + Endorsements
  + Reference Values
  + Appraisal Policy for Evidence
  → Attestation Result

Attestation Result
  + Data Policy
  → allow | unknown | block
```

Attester 不能给自己签发最终 `ActorAssurance`（`issuer ≠ subject`）。Verifier 只给出证明评估；最终数据释放仍由依赖方按当前策略决定。同一评定上下文取最弱项。

证据编码优先兼容 EAT/CWT/COSE；厂商原生 quote/report/document 可以封装，但原始签名材料和验证结果必须可追溯。

### 8.1 Assured Actor Edge 与 Certified Privacy Domain

跨组织或大型云服务不一定向调用方披露完整内部 Actor 图。Actrium 允许可信 Verifier 把一个已经闭合的内部子图封装成 `CertifiedPrivacyDomain`：

```text
Actor A
  └── AssuredActorEdge
      └── Certified Privacy Domain B
          └── internal Actor graph（不向 A 展开）
```

边界证明至少包含：

- domain/endpoint identity；
- 接受的数据类别、purpose 和操作；
- 可观察出口、retention、audit 和 deletion contract；
- 内部 privacy graph/evidence Merkle root；
- assurance profile 和未覆盖攻击面；
- Verifier/issuer、trust chain、policy digest；
- workload/service release 范围；
- freshness、revocation 和 incident obligations。

上游 policy 信任该 Verifier 和 profile 时，可以把整个子图作为一个已证明节点参与 meet，不再递归扩散内部 unknown。这不是消灭未知，而是把内部验证责任封装到一个明确、可撤销的信任主体。

Service 自签「我安全」不能形成 Assured Edge。只有用户/组织/Realm policy 已信任的 Verifier、证书 scope 与当前 endpoint/session 均匹配时才可以。

## 9. Attestation 与会话/密钥绑定

### 9.1 必须防止「证明是真的，连接是假的」

合法 Evidence 如果没有绑定当前 session，攻击者可能把另一个正常 TEE 的证明转发过来，再把数据导向不受保护环境。

因此 attestation challenge 至少绑定：

```text
fresh nonce
+ ActorId
+ current ActorStateDigest
+ runtimeInstanceId
+ workload/package digest
+ policy digest
+ ephemeral channel/decryption public key
+ intended use / audience
```

### 9.2 Attestation-gated Key Release

private/secret 数据推荐采用：

```text
1. Scheduler 选择候选 Compute Actor
2. Relying Party 发送 nonce 和 policy digest
3. TEE 内生成 ephemeral key
4. Attester 生成绑定 workload、key、nonce 的 Evidence
5. Verifier 生成短期 Attestation Result
6. Relying Party 检查 Actor/edge/data policy
7. Key Broker 把 Workspace/session DEK 封装给 attested ephemeral key
8. TEE 内解封并开始处理
9. Result 到期、TCB 撤销或 graph 变化时 re-attest/rekey
```

没有通过 appraisal 时，敏感数据密钥不得先释放给普通 Actr Host，再由 Actr Host 转发进 TEE。

## 10. GPU、NPU 与异构算力

### 10.1 Composite Attestation

Agent 使用 GPU/NPU 时，只有 CPU TEE 通过证明还不够。隐私边界可能包含：

```text
CPU TEE / Confidential VM
├── Guest kernel
├── runtime / container
├── driver
├── IOMMU / device assignment
├── CPU↔accelerator link
└── GPU/NPU
    ├── device firmware
    ├── CC mode
    └── device memory
```

任何一环能看到明文但无法证明，`compute_confidentiality` 就是 `indeterminate`。

### 10.2 Accelerator Claims

```json
{
  "deviceClass": "npu",
  "vendor": "vendor-id",
  "model": "model-id",
  "deviceIdentityDigest": "sha256:...",
  "firmwareMeasurement": "sha256:...",
  "driverMeasurement": "sha256:...",
  "confidentialMode": true,
  "deviceMemoryProtected": true,
  "hostLinkProtected": true,
  "iommuBoundToTee": true,
  "debugDisabled": true,
  "attestationEvidenceDigest": "sha256:...",
  "endorsementChainDigest": "sha256:..."
}
```

如果 accelerator 没有设备级证明，或 CPU→设备传输会在普通 Actr Host/驱动中暴露明文：

```text
compute_confidentiality = indeterminate
reason = ACCELERATOR_CONFIDENTIALITY_NOT_PROVEN
```

### 10.3 厂商兼容

Actrium 不把 Intel、AMD、Arm、NVIDIA、华为 Ascend 或某云服务写成固定信任结论，而采用：

```text
vendor evidence
  → versioned verifier adapter
  → canonical compute claims
  → organization/Realm appraisal policy
```

每个 adapter 必须声明：

- 支持的硬件、固件、驱动和部署拓扑；
- vendor trust roots；
- Evidence 格式与签名算法；
- Endorsements 和 Reference Values 获取方式；
- TCB advisory/撤销源；
- 能保护和不能保护的攻击面；
- freshness 和 nonce 语义；
- 测试向量与负向测试；
- adapter 自身签名、版本和撤销。

厂商宣传材料只能用于发现 capability，不能直接成为 `CONTROLLED` 的依据。只有公开或经组织审核的验证接口、参考值、证书链和可重复负向测试全部具备后，才能进入 production profile。

## 11. Workspace 与静态数据

### 11.1 Envelope Encryption

每个 Workspace/Service 至少使用独立 Data Encryption Key：

```text
Workspace data ── DEK
DEK ── KEK in local Keystore / organization KMS / Realm-approved KMS
```

要求：

- ActorId 不是解密密钥；
- DB 只保存 ciphertext、key id 和必要 metadata；
- backup/snapshot 保持加密；
- Service 删除、设备撤销或 policy 变化可撤销 KEK/DEK 访问；
- key rotation 不要求重新发布 ActorId；
- 日志、索引、缓存和临时文件遵循同一 data class。

### 11.2 CC 场景

如果 policy 要求 confidential compute：

- KMS policy 必须检查 Attestation Result；
- DEK 只封装给被证明的 ephemeral key；
- 解密只发生在证明覆盖的执行边界；
- snapshot、migration 和 accelerator offload 必须在证明覆盖范围内；
- 证明到期不要求立即丢失内存中的运行状态，但在 grace window 后必须停止接收新 secret 并 re-attest/rekey。

### 11.3 审计日志

服务器保留审计日志本身必须是静态 Contract 中的显式 storage/audit sink：

```toml
[[actrium.privacy_contract.audit_sink]]
kind = "security-audit"
fields = ["actor-pseudonym", "operation", "decision", "timestamp"]
payload = false
data_classes = ["metadata"]
retention = "30d"
operator_visibility = "authorized-security-admin"
encrypted = true
```

Actr Host 必须把日志 capability、字段 schema、目标、加密、保留期、读取主体和导出行为纳入隐私图。能记录 payload、prompt、Workspace 内容、模型输入输出或稳定用户标识时，必须按其真实 data class 重新评估，不能统称「审计」绕过策略。

Attestation 可以证明正在运行包含某日志配置的 workload/环境，Host broker 可以证明只发放了声明的 audit capability；它不能仅凭「服务器有审计日志」证明服务器没有其他隐蔽记录。因此：

- WASM/Native confinement 证明没有绕过 broker 的其他日志出口；
- CC 证明 Actr Host/云管理员不能在边界外读取内存并偷偷记录；
- audit sink 自身的存储、管理员和导出方继续作为闭包节点或 Certified Privacy Domain；
- 未声明日志、Actr Host 级明文采集或无法证明的 retention 使相关事实为 `UNKNOWN` 或 `UNCONTROLLED`。

对 LLM Server 和 GPU/NPU 尤其要区分：

- accelerator Attestation 通常证明设备身份、固件、CC mode 和设备状态，不自动证明上层 LLM Server 没有把 prompt/response 写入日志；
- 若日志通过受控文件、对象存储、网络或 Actor capability 离开被证明边界，Actr Host 必须观察到 audit edge；
- 若日志逻辑位于被度量 workload 内，workload/config digest 可以证明「运行的是哪套日志代码与配置」，但仍要结合封闭 egress 和可信 Verifier 才能证明没有其他口子；
- 若是 opaque 第三方 LLM API，外部调用方不能仅凭 GPU 证明技术性检测其内部留存；只能依赖 Certified Privacy Domain 对 retention/audit 的有 scope 证明。没有该证明该项为 `UNKNOWN`。

所以「服务器留了审计日志」不是天然对外可感知。只有日志出口被 capability 化，或者整个服务边界对日志/retention 提供了可验证 aggregate，它才进入 Actrium 可证明的隐私图。

## 12. 输出与二次泄露

「没有原始 socket」不能防止 workload 把 secret 塞入一个合法 Actor 响应。

因此高敏数据还需要：

- 输入、内部值和输出 schema 带 data class；
- 跨类别输出需要显式 declassification capability；
- Actr Host 校验结构、大小、目标和用途；
- 聚合查询可以要求 k-anonymity、差分隐私预算或最小样本数；
- model inference 明确 prompt、embedding、cache、tool result 和 telemetry 的分类；
- 对高带宽 covert channel 做配额、速率和调用形状限制；
- 不能完全关闭的 timing/size/side-channel 在 assurance 中显式披露。

这仍不能自动证明任意业务算法的数据最小化；高风险程序需要专项 policy、测试或形式化模型。

## 13. Evidence Bundle

`ActorAssurance` 引用、而不是内嵌全部原始证明：

```json
{
  "schemaVersion": 1,
  "bundleId": "aeb_01...",
  "subject": {
    "actorId": "<ActorId>",
    "runtimeInstanceId": "runtime_01...",
    "packageDigest": "sha256:...",
    "workloadDigest": "sha256:..."
  },
  "policyDigest": "sha256:...",
  "assurance": {
    "identity": "pass",
    "artifactProvenance": "pass",
    "workloadConfinement": "pass",
    "computeConfidentiality": "pass",
    "channelSecurity": "pass",
    "storageProtection": "pass",
    "egressClosure": "pass",
    "outputControl": "not-required"
  },
  "evidenceRefs": [
    {
      "kind": "keel-wasm-closure",
      "digest": "sha256:..."
    },
    {
      "kind": "rats-attestation-result",
      "profile": "vendor-profile@1",
      "digest": "sha256:..."
    },
    {
      "kind": "actor-channel-evidence",
      "digest": "sha256:..."
    }
  ],
  "binding": {
    "actorStateDigest": "…",
    "structureDigest": "…"
  },
  "issuedAt": "2026-07-29T12:00:00Z",
  "expiresAt": "2026-07-29T12:05:00Z",
  "signature": {
    "algorithm": "Ed25519",
    "keyId": "privacy-key-3",
    "value": "<base64>"
  }
}
```

原始 hardware Evidence 可能包含设备序列号等敏感数据。普通 peer 默认只获得：

- Attestation Result；
- canonical claims 的必要子集；
- Evidence digest；
- Verifier identity；
- profile/version；
- freshness；
- 撤销检查状态。

## 14. 汇聚算法

对携带数据类别 `d` 的 edge `e = A → B`：

```text
Required(d, operation, organizationPolicy)

EdgePrivacy(e, d) = meet(
  RequiredClaims(SourceActor A),
  ChannelEvidence(e),
  RequiredClaims(TargetActor B),
  ComputeEvidence(B, d),
  StorageEvidence(B, d),
  EgressAndOutputPolicy(B, d)
)
```

应用整体是无根图上各节点 \(R_A\) 的最弱项，且展示须 `Ready`。

规则：

- 只对当前数据和操作 required 的维度汇聚；
- 缺失 / 过期 / 不支持的 Evidence 是 `UNKNOWN`；
- 签名失败、measurement 不匹配、撤销或策略明确拒绝是 `UNCONTROLLED`；
- 强项不能抵消任一 required fail / indeterminate；
- 图、workload、设备或会话材料变化会换 `ActorStateDigest` / `ApplicationStateDigest`；
- 绑旧状态的 Attestation Result 不能授权新边。

## 15. 控制面组件

```text
Actrium / Actr Privacy Control Plane
├── Policy Registry
│   ├── data class policy
│   ├── organization overrides
│   └── accepted assurance profiles
├── Evidence Collectors
│   ├── Host/WASM/Native
│   ├── Channel
│   ├── Workspace
│   └── CPU/GPU/NPU attesters
├── Verifier Gateway
│   ├── RATS/EAT
│   └── versioned vendor adapters
├── Reference Value & Endorsement Store
├── Revocation/TCB Advisory Feed
├── Privacy Graph Evaluator
├── Key Broker / KMS Adapter
└── Audit & Transparency Log
```

Realm 签名 key 持有方（Realm 域名所有者）或其委派的 Actr Facility Provider 可以运营或委派 Verifier、Reference Value、Revocation 和 Key Broker 服务，但它不是唯一全球信任根；单个实例 Realm 只在自己的 policy 中选择信任哪些 Verifier。组织和本地 Actrium 可以选择自己的 Verifier 与 appraisal policy。

## 16. 失败与撤销

以下事件必须触发重评：

- workload、package、guest image、driver、firmware digest 变化；
- Actor runtime 重建或迁移；
- accelerator 加入/移除；
- channel re-handshake 或 endpoint key 变化；
- Attestation Result/Endorsement 到期；
- vendor TCB advisory 或 key revocation；
- organization policy 变化；
- Workspace key rotation；
- `ActorStateDigest` 或 `structureDigest` 变化。

动作：

```text
indeterminate → 停止发送新的 private/secret，进入 re-attestation
fail          → 关闭 edge、撤销 capability、停止 key release
recovered     → 新 nonce、新 Result、绑新 `ActorStateDigest` 的通道与密钥
```

不能因 Verifier 暂时不可用而继续沿用无限期缓存证明。核对 `expiresAt`、Attestation Result 与 Actor Grant 有效期所用的本地时钟不可回拨——执行方与评定方同受此约束，可被回拨的时钟会让过期材料重新被接受。

## 17. 稳定错误码

```text
CHANNEL_IDENTITY_BINDING_MISMATCH
CHANNEL_ATTESTATION_BINDING_MISMATCH
CHANNEL_REPLAY_DETECTED
CHANNEL_DOWNGRADE_REJECTED

ATTESTATION_EVIDENCE_INVALID
ATTESTATION_SUBJECT_MISMATCH
ATTESTATION_NONCE_MISMATCH
ATTESTATION_POLICY_MISMATCH
ATTESTATION_RESULT_EXPIRED
ATTESTATION_TCB_REVOKED
ATTESTATION_PROFILE_UNSUPPORTED
ATTESTATION_VERIFIER_UNTRUSTED

COMPUTE_CONFIDENTIALITY_NOT_PROVEN
ACCELERATOR_ATTESTATION_INCOMPLETE
ACCELERATOR_CONFIDENTIALITY_NOT_PROVEN

KEY_RELEASE_POLICY_NOT_SATISFIED
KEY_RELEASE_TARGET_KEY_MISMATCH

OUTPUT_DECLASSIFICATION_REQUIRED
OUTPUT_POLICY_NOT_SATISFIED

PRIVACY_CONTRACT_INVALID
PRIVACY_CONTRACT_DIGEST_MISMATCH
PRIVACY_CERTIFICATE_EXPIRED
PRIVACY_CERTIFICATE_REVOKED
PRIVACY_RUNTIME_GRAPH_OUTSIDE_CONTRACT
PRIVACY_CAPABILITY_NOT_DECLARED
PRIVACY_GRAPH_MUTATION_PENDING
PRIVACY_DOWNGRADE_CONSENT_REQUIRED
PRIVACY_DOWNGRADE_GRANT_INVALID
PRIVACY_HARD_FLOOR_NOT_SATISFIED
PRIVACY_DOMAIN_CERTIFICATE_INVALID
PRIVACY_DOMAIN_SCOPE_MISMATCH
PRIVACY_AUDIT_SINK_NOT_DECLARED
```

## 18. 交付顺序

### P0：统一语义

- Actor Secure Channel requirements；
- Assurance Vector；
- RATS 角色和 Evidence Bundle；
- data class→required claims policy；
- evidence freshness、binding、revocation。

### P1：本地与普通云

- WASM egress closure；
- Workspace envelope encryption；
- package provenance；
- 非自签的 `ActorAssurance`（Actr Host 可评 Guest，不得自签 Actr Host）；
- 不要求 CC 的 public/private policy。

### P2：Confidential CPU

- 第一种 production TEE/CVM profile；
- Verifier adapter；
- attestation-bound session key；
- KMS/Key Broker 条件释放；
- migration、snapshot、revocation tests。

### P3：Confidential Accelerator

- CPU+GPU/NPU composite attestation；
- driver/device/firmware/reference values；
- secure device assignment/link/memory claims；
- 不同厂商 adapter conformance suite。

### P4：高级输出隐私

- declassification；
- DP budget；
- MPC/FHE 特定 workload profile；
- side-channel assurance disclosure。

## 19. 参考

- [RFC 9334: RATS Architecture](https://www.rfc-editor.org/rfc/rfc9334.html)
- [RFC 9711: Entity Attestation Token](https://www.rfc-editor.org/rfc/rfc9711.html)
- [RFC 8446: TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446.html)
- [RFC 9180: HPKE](https://www.rfc-editor.org/rfc/rfc9180.html)
- [Confidential Computing Consortium](https://confidentialcomputing.io/)
- [NVIDIA Confidential Containers：CPU + GPU composite attestation](https://docs.nvidia.com/datacenter/cloud-native/confidential-containers/latest/attestation.html)
- [华为云 QingTian Enclave 密码学证明](https://support.huaweicloud.com/intl/zh-cn/twp-ecs/ecs_twp_0010.html)
- [昇腾社区：基于昇腾 TEE 与远程证明的 AI 可信计算](https://www.hiascend.com/activities/dynamic-news/733)
