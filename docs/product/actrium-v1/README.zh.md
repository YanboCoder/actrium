# Actrium v1 产品需求说明

> 状态：目标设计草案（Target State Draft）<br>
> 文档版本：0.1<br>
> 日期：2026-07-29<br>
> 适用范围：Actrium、Actr、Keel 的下一阶段产品与协议设计<br>
> 原则：先定义理想模型，再调整现有实现；本文不以兼容历史实现为前提

## 1. 一句话定义

Actrium 是 Actrium 世界的「程序浏览器」：用户在 resident app Tab 中运行由 Keel Guest 描述的原生界面，每个 resident app Tab 由一个受信任的 Front Actor 承载，并通过该实例的 Service 访问程序状态、Workspace 与 Actor 网络。

## 2. 总体结论

这套修订方向合理、统一，而且技术上可行：

```text
Tab
└── Front Actor
    ├── Actrium 固定的 Native Workload
    │   ├── Keel Host
    │   ├── Capability Broker
    │   └── Actr Client
    ├── Publisher 提供的 Keel Guest
    └── Service ── Workspace
```

它的主要价值不是少画几层，而是让每一层只承担一种职责：

- Tab 是用户交互会话。
- Front Actor 是受 Actrium 控制的前台安全边界。
- Keel Guest 是程序开发者唯一可定制的前台代码。
- 每个实例有一个 Service，持有状态；Front 只连它。
- Workspace 由 Service 持有，Front 不碰路径、密钥或底层存储。
- 应用实例就是 Realm，地址是 Realm 链接 `actr://<Realm 域名>/<realm_id>`；程序从哪里下载写在 Realm Profile 里。
- `ActorId` 形如 `<serial>.<realm_id>@<domain>`，自带 Realm 名，不含程序类型；已知 `ActorId` 时不得再负载均衡。
- `publisher/program@version` 精确标识不可变的程序版本。
- Realm 对应一个应用实例，管该实例的身份、准入与网络入口；域名只用来给 Realm 命名；信令和 TURN 是 Realm 委派的共享基础设施。

但它依赖三项必须先补齐的基础能力：

1. **持久、可解析的 ActorId**：Service 重启、断网、凭据续期或迁移后必须保留同一 `ActorId`，并能由 ID 得到 Realm 名。当前 Actr 的 hard rebind 可能换 ID，不能直接作为长期、可分享 URL 语义。
2. **域名 Publisher**：Actr 的 publisher 语义与长度限制需要调整，使规范化 DNS 域名成为程序发布者身份。
3. **真正封闭的 Front Host**：Actrium 必须在 Keel 之前完成包校验，并以 capability、组件白名单、资源配额和确定的沙箱后端限制 Guest；不能把远端 URL 直接交给 Keel 加载。

## 3. 冻结的核心决策

| 编号 | 决策 |
|---|---|
| D-001 | 每个处于加载、运行或前台暂停状态的应用 Tab 恰好运行一个 Front Actor；被 Shell 丢弃但仍保留的 dormant Tab 可以没有 live Actor。 |
| D-002 | 每个 Front Actor 在运行态恰好绑定该实例的 Service。创建/连接过程中允许暂时为零。 |
| D-003 | 「1:1」是 live Front 视角的唯一绑定，不是 Service 与历史 Tab 的永久双射；Service 独立存活并可被新 Front 重连。 |
| D-004 | v1 默认采用独占交互会话；同一实例的链接在本机再次打开时优先聚焦已有 Tab。共享会话是后续显式能力。 |
| D-005 | Service 绑定零个或一个 Workspace；需要多 Actor 的程序由 Service 在同一 Realm 内编排。 |
| D-006 | Actrium 提供原生 Front Host。Publisher 提供 Keel Guest 和 Service Workload，以及各自的资源和声明。 |
| D-007 | v1 第三方 Front Guest 只开放 Keel JS Guest，并固定使用具备硬超时和堆上限的 QuickJS 后端。 |
| D-008 | 每个 `actr-package/2` 包声明一个 Actor 类型和一个 Workload，Publisher 在包内签署清单。Front 声明必需的 Service 依赖。 |
| D-009 | 链接只指向实例，且只使用 `actr://<Realm 域名>/<realm_id>` 形式；打开链接只连接该实例，不得失败后偷偷新建。新建实例是 Actrium 内的本地操作，不是链接。 |
| D-010 | Publisher 身份是无端口的规范化域名。Publisher Profile 提供验证包签名的受信公钥。 |
| D-011 | Marketplace 是发现、审核与撤销信息源，不是 Publisher 命名权威；域名与包签名才是来源链。 |
| D-012 | v1 ActorId 必须包含可验证的 Realm 名（`<realm_id>@<domain>`，如 `notes@acme.com`），程序类型不在 ActorId 中；它是公开地址和密码学身份，不是 bearer token，知道地址不等于有权连接。 |
| D-013 | Realm 是逻辑信任域，对应一个应用实例，可委派多个信令/TURN 服务；Realm 与某个物理信令进程不是一回事，一个 Actr Facility Provider 承载多个 Realm；Realm 在创建时绑定一个 Actr Facility Provider，此后不可更换。 |
| D-014 | Service 实例固定到精确版本与包 digest；v1 升级通过显式迁移创建新 Actor，而不是原地替换 workload。 |
| D-015 | Actor credential 证明 Actor key、身份和 claims，不证明对端实际运行的代码。应用如何据此授权属于应用写法，见[应用开发建议](app-development-guidance.zh.md)。 |
| D-016 | Service manifest 的 `[workspace]` 声明必需性、申请的访问级别和支持的 Provider。部署或实例创建流程决定准备方式，Host 按实例保存实际绑定和授权。 |
| D-017 | 删除分为可恢复的 Trashed 与不可恢复的 Purged/Deleted；删除 Workspace 始终是独立动作。 |
| D-018 | Publisher 与 Realm 域名的 bootstrap 固定使用 HTTPS 443（即 `https://<domain>/.well-known/actr`）；Realm Profile 来自 `realms` 声明的 HTTPS 位置（可带端口），其中固定选用的包与 Actr Facility Provider 端点（可带端口）。取不到或验不过 Realm Profile 时不得使用 Realm Profile 以外的任何 Actr Facility Provider。 |
| D-019 | 应用保障按无根有效依赖图汇聚；通道有效且 `Ready` 时，展示颜色取全部节点结论的最弱项。Front 只定位，不是图根。 |
| D-020 | `ActorAssurance` 由验证者签发，`issuer ≠ subject`。Actr Host 可评 Guest、可提供证据，不得自签自己的最终结论。 |
| D-021 | 顶层只分绿 / 黄 / 红；切面未就绪时显示无法建立，不是第四色。未知不是已确认泄漏，但绝不能显示为绿。 |
| D-022 | WASM 可通过 import / 有效能力 / 出口闭包度量；第三方同进程 Native 不得绿，合规 out-of-process Native Capsule 才可能提升。 |
| D-023 | 结论必须联动数据面：挂起与非 `LocalContinue` 的成员不得处理应用数据；新配置只作用于新数据。 |
| D-024 | 用户界面只有三色，内部保留 identity、provenance、confinement、compute、channel、storage、egress 和 output 等多维事实，映射为 `CONTROLLED / UNKNOWN / UNCONTROLLED`。 |
| D-025 | Actr 的每条网络 Actor edge 必须加密、认证、完整性保护和防重放；signaling、TURN 与普通 relay 不得持有 payload key。 |
| D-026 | 「全过程加密」表示传输和静态存储加密，明文只在 policy 认可且可证明的执行边界内出现；不得宣称普通计算期间数据始终是密文。 |
| D-027 | Confidential Computing 保护 data in use，WASM/Native confinement 约束 workload 出口；二者正交，互不替代。 |
| D-028 | 远程证明采用 Attester、Verifier、Relying Party 分工；Evidence 不是信任结论，最终授权由本地 data policy 决定。 |
| D-029 | private/secret 的解密密钥只能在所需 attestation 通过后封装给与 Evidence 绑定的临时执行环境 key。 |
| D-030 | 使用 GPU/NPU 的 Actor 必须进行 CPU、Guest、driver、设备、firmware、IOMMU/link/memory 的组合证明；任一明文环节无法证明时 compute confidentiality 为 unknown。 |
| D-031 | 绿色绑定当前就绪切面（`ApplicationStateDigest` + `ApplicationAssurance` + `Ready`），不是 Program 全部版本，也不是短暂 transport session。 |
| D-032 | Program Privacy Contract 仍是 Actor 图语法和能力的静态上界，只约束候选图，不直接产生绿色。 |
| D-033 | 初始图必须在释放受保护数据或解密密钥前完成评估；结论绑定当前 `ActorStateDigest`、`structureDigest`、策略和证据。 |
| D-034 | `ActorStateDigest` 变化时本地挂起并废除旧权力；不依赖变化节点的成员可 `LocalContinue`，但不得展示应用绿。 |
| D-035 | 颜色未减弱可自动恢复；变弱则保持阻断并请求风险接受（不改颜色）；通道无效或触碰平台硬底线时用户不能覆盖。 |
| D-036 | 受信 Verifier 可以把内部 Actor 子图封装为 Certified Privacy Domain，并通过 Assured Actor Edge 提供有 scope、有效期和撤销的 aggregate；自签声明无效。 |
| D-037 | 审计日志是显式 storage/audit sink，必须声明字段、数据类别、目标、读取主体和 retention；LLM/GPU Attestation 本身不能证明 Server 没有另行留存日志。 |
| D-038 | Realm 签署本实例的选包配置。`primary_package` 和 `front_package` 分别指向 `packages` 中不同的 Service 和 Front 条目。见 [Realm Profile §4](../../../actr/protocols/realm-profile.zh.md#4-realm-profile-格式)。 |

## 4. 文档导航

建议按顺序阅读：

1. [产品定义](01-product-definition.md)：为什么做、为谁做、做什么、不做什么。
2. [领域模型](02-domain-model.md)：Tab、Front、Service、Workspace、Program、Realm 的关系。
3. [用户体验](03-user-experience.md)：从地址栏到程序库、实例库与错误恢复。
4. [Realm 链接与地址栏](04-uri-and-resolution.md)：实例地址怎样显示、打开、保存和分享。
5. [程序包与 HTTPS 分发](05-program-package-and-distribution.md)：域名 Publisher、`.actr` 包、市场与更新。
6. [运行时与 Front–Service 协议](06-runtime-and-protocol.md)：Native Host、Keel Guest、能力边界与会话协议。
7. [Realm、信令、TURN 与身份](07-realm-network-and-identity.md)：信任链、网络发现和端口政策。
8. [Workspace、生命周期与本地存储](08-workspace-lifecycle-and-storage.md)：状态归属、创建、重连、删除和迁移。
9. [安全与隐私](09-security-and-privacy.md)：威胁模型、权限、沙箱和秘密管理。
10. [需求与验收标准](10-requirements-and-acceptance.md)：功能需求、非功能需求和端到端验收。
11. [交付计划与可行性](11-delivery-plan.md)：基础改造、分期、风险与退出条件。
12. [参考协议与样例](12-reference-spec.md)：Manifest、Publisher/Realm Profile、握手消息和术语。
13. [Actrium 隐私标记与图汇聚](13-privacy-state-and-aggregation.md)：把现行安全模型落到 Tab / Front / Actr Host。
14. [隐私原语、远程证明与可信算力](14-privacy-primitives-and-attestation.md)：全过程加密、WASM/CC 分工、RATS、密钥释放与 CPU/GPU/NPU 组合证明。

另有非规范性的[应用开发建议](app-development-guidance.zh.md)：写 Service 时的授权与信任建议，不属于平台规则。

## 5. 规范用语

本文使用以下强度：

- **必须 / MUST**：缺失会破坏产品模型、安全边界或互操作。
- **应该 / SHOULD**：默认应满足，偏离时必须记录理由与替代措施。
- **可以 / MAY**：兼容实现或后续扩展。

示例 JSON/TOML/协议字段用于冻结产品语义；正式 wire schema 仍需在 Actr/Actrium 的 schema 仓库中版本化。

### 5.1 文档效力

出现冲突时按以下顺序解释：

1. **安全与隐私语义**：[docs/security](../../architecture/security/actrium/README.zh.md)；
2. 本 README 中不与上条冲突的产品决策；
3. [需求与验收标准](10-requirements-and-acceptance.md)中的 MUST/FR/NFR；
4. 链接、分发、运行时、Realm、Workspace 及本章未覆盖的产品专题；
5. [参考协议](12-reference-spec.md)中的示意字段；
6. 产品叙事、用户流程和交付计划。

`01` 与 `03` 主要帮助理解产品；`04`—`09`、`12`—`14` 是待 P0 冻结的规范草案；`10` 是验收基线；`11` 是计划而非 wire contract。

## 6. 事实基础

本设计以以下公开实现为可行性依据，但不被其历史结构约束：

- [Actrium/actr](https://github.com/Actrium/actr)：Actor 模型、`ActrId` 精确寻址、Realm/连接 ACL、信令、WebRTC、`.actr` 签名包和 WASM workload。
- [Actrium/keel](https://github.com/Actrium/keel)：Keel Host/Guest、Bridge、JS 沙箱、契约与原生组件渲染。
- [Actrium 当前架构概览](../../architecture/actrium/overview.md)：多平台 Shell、共享运行时与 Actr SDK 边界。

已验证的关键事实包括：

- Actr 已支持 `Dest::Peer(ActrId)` 精确寻址和按 `ActrType` 发现，两者可以清晰分工。
- `.actr` 已具备 Ed25519 manifest 签名、SHA-256 文件绑定和资源；依赖锁按 workload/构建流程可选。
- Keel 的 Host/Guest 边界与白名单模块机制适合承载 Front，但其 `Engine.loadBundle(URL)` 本身不是可信分发系统。
- Keel 多数「原生」沙箱仍是同一 OS 进程内的语言运行时隔离，不能宣称为浏览器级进程隔离。

本文档正文统一使用 Loran、Publisher Registry、Issuer、publisher 等规范名；代码和旧文档中的历史名称集中列在 [术语表「历史名称对照」](../../GLOSSARY.zh.md)。
