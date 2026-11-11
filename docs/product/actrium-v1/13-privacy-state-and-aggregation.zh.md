# 13. Actrium 隐私标记与图汇聚

> 安全语义以 [docs/security](../../architecture/security/actrium/README.zh.md) 为准。本章只写 Actrium 如何把该模型落到 Tab、Front、Actr Host 和数据面。两者冲突时改本章，不改现行安全模型。

成熟度：`Specified`。

## 1. 本章回答什么

Actrium 地址栏上的绿 / 黄 / 红，对应一张**无根应用图**在**当前就绪切面**上的 `ApplicationAssurance`，不是 Program 名称、不是 Marketplace 审核，也不是「曾经存在一个全图同时世界」。

```text
Tab ──定位──▶ Front Actor ──发现──▶ 无根应用图 CC(U(D))
                                      │
                                      ▼
                         ActorAssurance（每节点，禁止自签）
                                      │
                                      ▼
                    ApplicationStateDigest + ApplicationAssurance
                                      │
                                      ▼
                         Ready？ → 展示绿 / 黄 / 红
                         否则    → 无法建立 / 正在重新验证
```

Front Actor 只做产品入口，不是图根。Service 是 Guest 的业务 façade，不是安全根。

内部证据维度（身份、制品、约束、计算、通道、存储、出口、输出）是评定输入。用户只看到三色；切面未就绪时不把任何颜色当成当前应用保障。

通道、原语和远程证明见 [14](14-privacy-primitives-and-attestation.md)。完整判断与证明见 [形式模型](../../architecture/security/actrium/formal/model.zh.md)。

## 2. 颜色与事实

| 用户看见 | 规范含义 | 不得写成 |
| --- | --- | --- |
| 绿 | `Ready` 且每个节点 `CONTROLLED`，通道有效，确认无 `Retain` | 永久认证；全图同时世界；软件无漏洞 |
| 黄 | 非红，且至少一项必要事实 `UNKNOWN` | 「稍微安全」；已经泄漏 |
| 红 | 存在已确认 `UNCONTROLLED`（含已确认 `Retain`） | 系统目击了一次泄漏事件 |
| 无法建立 / 正在重新验证 | 图或通道无效，或任一成员保障挂起 | 第四色；可用风险接受绕过 |

风险接受只允许继续跑，不改颜色。输入授权（摄像头、文件）与颜色正交。同一评定上下文多份 `ActorAssurance` 取最弱项，不能挑较绿的验证者。

数据带取得世代。新配置只作用于新数据；旧数据走「取得时声明 ∩ 当前声明」。

## 3. 应用图

图由有效 `Transfer(ActorId)` 形成：目标可解析、双方同一 Principal、声明与状态当前有效。丢掉无效边后忽略方向，取连通分量。循环合法，必须发现入边。

```text
structureDigest = H("actrium/structure/v1", EncGraph)
EncGraph    = actrium/graph/v1 规范 JSON（排序后的 ActorId 与有向边）
```

不进入图：仅安装未执行的 Program、无有效依赖的同 Realm Actor、跨 Principal Actor。跨 Principal 或无效 Actor 出口仍在发起方计分，不得判 `CONTROLLED`。

非 Actor 目标（屏幕、文件、Workspace、HTTPS API）是 Actr Host 可强制的汇。TLS 主机名只证明送到该域名，不证明域内不继续外泄。目标分类不由声明方自选：可出示 Actr 身份的端点必须按 Actor 目标评定并接受跨 Principal 规则，不得改写成 HTTPS 汇绕过评分；类别无法判定时该出口保持 `UNKNOWN`。用户信任一个汇的语义是「认可数据离开保障范围」。只转发端到端密文的 TURN 不入图。

Front 的最终节点结论是：

```text
R_A(Front) = join(Actrium Host 平台事实, Keel Guest 出口事实)
```

Actr Host 能做而 Guest 不能做的出口，仍是这个 Actor 的出口。不能只评 Guest。

## 4. 状态变化

不做全图分布式提交。顺序：本地提交新 `ActorStateDigest` → 废除旧权力 → 保障挂起 → 异步重建切面。`ActorState` 含状态权威维护的单调修订号：配置回滚也得到新状态根，被废除的旧 Actor Grant、通道与结论不会因回滚复活。

```text
Depends(b, a)           := 无向有效依赖边
LocalContinue(b | C)    := ACTIVE ∧ 当前 AA 仍适用 ∧ 不依赖 C ∧ 不展示应用绿
```

无法判断依赖则停。满足 `LocalContinue` 的 Actor 可继续自身工作。任一成员挂起时不得发布或展示绿色。

新节点加入：先挂起相关边，验证通过后再送应用数据。能力扩大不能让旧数据走新出口。

## 5. Workload 怎么评

### 5.1 WASM

判 `CONTROLLED` 至少同时满足：制品与已验证包一致；import 可枚举；无 ambient 文件系统 / socket / 进程 API；能力全部经 Actr Host；有效声明是 Manifest ∪ Actr Host ∪ 环境不可排除项；每个 Actor 目标入图另评；每个非 Actor 汇被策略认可；证据未过期。

可度量的是：

```text
declared ∪ host ∪ env − proven-impossible  = 有效声明
observed ⊆ 有效声明且被强制
```

证不了业务逻辑无缺陷。

### 5.2 同进程第三方 Native

固定 `UNKNOWN`（若已确认可绕过 broker 外发则为 `UNCONTROLLED`）。manifest、签名、静态扫描、API hook 不能把它洗绿。

### 5.3 Native Capsule

独立进程、启动前已施加 OS sandbox、无原始网络 / 任意文件 / 子进程，且只有类型化 broker IPC 时，才可按证据评定。任一约束落空则 fail closed 或保持未知，不得静默退回普通进程。这证明出口闭包，不证明内存安全。

平台现实不变：v1 WASM 可绿；第三方 in-process Native 不能绿；desktop Capsule 实验级；iOS 第三方动态 Native 不做产品能力。

### 5.4 Actrium Native Front Host

不能把平台 Actr Host 一律判未知，否则所有 Tab 永远非绿。Actr Host 结论来自官方签名、安装完整性、版本策略和设备证明；安装无法验证或版本已撤销时，同样是黄或红。被评估的是 Front 进程整体。

Actr Host 可以为 Guest 提供证据或作为验证者签发 `ActorAssurance`，但 **issuer ≠ subject**：Actr Host 不得自签自己的最终结论。

### 5.5 外部 HTTPS

不是 Actor。无授权却能发送：`UNCONTROLLED`。策略要求验证对端内部处理但无证据：`UNKNOWN`。用户明确授权的终端汇可以是 `CONTROLLED`，只表示汇点受控，不表示对端内部安全。前提是该端点确实不可按 Actor 评定：能出示 Actr 身份的端点必须按 Actor 目标建模（见 §3），不得以 HTTPS 汇的名义授权。

## 6. 声明是输入

Program Privacy Contract 与 Native Capsule profile 提供验证输入。Actr Host 对照 imports、实际能力和实际边。
未声明且可能使用的出口在确认时判红，无法确认时判黄。

## 7. 谁签发结论

```text
Claim      当前实例启用了某项约束
Evidence   用来核实该主张的材料
Assurance  验证者结合状态、证据、规则和用户策略给出的结论
```

Actrium 内的 Privacy Agent 负责观测、取证、组图和向验证者提交材料。跨进程成为安全依据时，必须由验证者签名 `ActorAssurance`，精确绑定 `ActorId + ActorStateDigest`、证据指纹、策略、规则和有效期。依赖方使用前自行核验。

发布 `ApplicationAssurance` 前：完整发现、核对每个当前状态、每个 AA 不得强于该上下文最弱项、无挂起成员、重读当前指针。成功只说明读一致切面。

## 8. 产品对象（映射现行模型）

旧稿中的 `Privacy Mark` / `GraphRoot` / `epoch` 不再是规范对象。实现材料对应：

| 旧名 | 现行对象 |
| --- | --- |
| Actor Privacy Mark | `ActorAssurance` 的节点结论 |
| 应用 Privacy Mark | `ApplicationAssurance` 的颜色；展示还须 `Ready` |
| privacyGraphId + epoch | `structureDigest` + `ApplicationStateDigest` |
| GraphRoot | 无。Front 只定位 |
| Actr Host 自签 Mark | 禁止作为被评估者的最终结论 |

示意（字段名可在 schema 仓库版本化，语义不得弱于下表）：

```json
{
  "object": "actrium/actor-assurance/v1",
  "issuer": "verifier:…",
  "subject": { "actorId": "…", "actorStateDigest": "…" },
  "qid": "actrium.consumer.default",
  "ruleset": "…",
  "result": "CONTROLLED",
  "findings": [{ "effect": "Transfer(Actor(b))", "verdict": "CONTROLLED" }],
  "notAfter": "…",
  "signature": "…"
}
```

`ApplicationAssurance` 承诺 `structureDigest`、Principal、Merkle 根（叶子绑定 `ActorId + ActorStateDigest + H(AA)`）。成员证明对数路径。

边握手仍须在发送应用数据前核验对端 `gate=ACTIVE`、当前 AA、通道绑定双方当前 `ActorId + ActorStateDigest`。`minimumTarget` 用节点结论，不用旧 Mark 名。

## 9. 系统协议

名称可保留，语义按现行对象：

```text
actrium.privacy.v1.GetActorAssurance
actrium.privacy.v1.GetApplicationAssurance
actrium.privacy.v1.SubscribeChanges
actrium.privacy.v1.RequireReady
```

Attach 成功只证明直连会话。整图仍须 `Ready` 的 `ApplicationAssurance`。通道无效时不评分，直接阻断应用数据。

图规模超限：不得截断后发绿；保持无法建立或黄。

## 10. UI

地址栏：绿 / 黄 / 红，或「安全状态无法建立 / 正在重新验证 / 通信已阻断」。

详情必须能看到：policy、Actor 数、workload 种类、声明出口、最弱项、所信验证者（多份则展示最弱项，不提供挑选）、状态根、有效期、成员证明。配置变化后须说明新声明只作用于新数据。

不得把未知写成「安全」、把 HTTPS 锁写成「应用不泄露」、把 healthy 写成绿、把商店审核写成运行时保障。

## 11. 图本身也是隐私

peer 默认只拿必要成员证明和结论摘要。普通 Guest 不拿完整 ActorId 图。信令和遥测不上传业务拓扑。结论对象不含 Workspace 路径、payload、token。

## 12. 错误码

```text
ASSURANCE_INVALID
ASSURANCE_EXPIRED
ASSURANCE_REVOKED
ASSURANCE_SUBJECT_MISMATCH
ASSURANCE_SELF_SIGNED
ASSURANCE_CONFLICT_WEAKEST
POLICY_UNSUPPORTED
GRAPH_INCOMPLETE
SNAPSHOT_NOT_CURRENT
GRAPH_LIMIT_EXCEEDED
ACTOR_SUSPENDED
UNDECLARED_EGRESS
GENERATION_VIOLATION
TERMINAL_NOT_AUTHORIZED
NATIVE_NOT_MEASURABLE
CHANNEL_INVALID
```

## 13. v1 边界

支持：WASM 出口度量；Actrium Host 作为平台 TCB（不得自签自己）；第三方 in-process Native 保持未知或红；合规 Capsule 按证据评；动态边先验证后传数据；世代交集；Ready 展示。

不承诺：业务逻辑无缺陷；opaque HTTPS 内部不泄露；无 OS sandbox 时把第三方 Native 变绿；全球隐私评分；绿色等于绝对安全。
