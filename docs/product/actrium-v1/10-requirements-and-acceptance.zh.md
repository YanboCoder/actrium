# 10. 需求与验收标准

## 1. 功能需求

### 1.1 Shell 与 Tab

| ID | 需求 |
|---|---|
| FR-SH-001 | 每个 resident app Tab 在 loading/running/suspended 期间必须恰有一个 live Front Actor；Shell 页面与 dormant Tab 可以没有 Front。 |
| FR-SH-002 | Front Actor 完成启动后必须绑定且只绑定一个 Service。 |
| FR-SH-003 | 刷新必须重建 Front 并连接同一 Service，不得新建 Service。 |
| FR-SH-004 | 关闭 Tab 必须撤销 session 并销毁 Front，不得停止或删除 Service。 |
| FR-SH-005 | 后台 Tab 必须 pause Keel Engine；恢复必须 resume 或重建后 Attach。 |
| FR-SH-006 | 本机重复打开同一 exclusive Service 默认聚焦已有 Tab。 |
| FR-SH-007 | 地址栏必须分别展示 Publisher、实例、Realm 和连接安全状态。 |
| FR-SH-008 | Guest 导航必须经 `host:navigate`，不能直接改 Shell 历史、跨实例继承 capability 或把 secret 写入 URI。 |

### 1.2 URI

| ID | 需求 |
|---|---|
| FR-URI-001 | 必须支持 Realm 链接 `actr://<Realm 域名>/<realm_id>[/<页面路径>][?…][#…]`；RealmId 之后的部分原样交给应用。 |
| FR-URI-002 | 打开 Realm 链接必须验证 Realm Profile、其中指定的 Service 包、匹配的 Program Release 记录及 Front 包。随后在该 Realm 内按 Service 的 Actor 类型找到实例并 Attach。 |
| FR-URI-003 | Attach 失败不得 LB、降级版本、换 Realm 或创建替代实例。 |
| FR-URI-004 | Service 的 Actor 类型必须与 Realm Profile 所指包声明的 Actor 类型一致。 |
| FR-URI-005 | Realm 链接不得包含 credential、session secret、Workspace secret 或 bearer invite。 |
| FR-URI-006 | Realm 链接不得含端口；开发来源只通过包地址（localhost、回环地址或私有 CA）区分。 |
| FR-URI-007 | Realm 链接的输入与分享只能使用 `actr://`；`https://` 输入必须以 `URI_SCHEME_INVALID` 拒绝，不得通过 `links` 转换。 |
| FR-URI-008 | Parser 必须实施 IDNA、非法编码和控制字符检查。 |
| FR-URI-009 | 新建实例是 Actrium 本地操作，不由链接触发；成功后地址栏显示该实例的 Realm 链接。 |

### 1.3 Program Catalog 与分发

| ID | 需求 |
|---|---|
| FR-PKG-001 | Program Catalog 与 Service Inventory 必须分开。 |
| FR-PKG-002 | 发布版本必须提供 Publisher 签名记录，绑定两个包的类型和摘要。通过 Realm 打开时，记录中的 Service 包必须匹配 Realm Profile 的 `package`。 |
| FR-PKG-003 | 每个 Program Release 必须固定 Service 和 Front 两个包的摘要。 |
| FR-PKG-004 | 每份 `.actr` 清单必须声明一个 Actor 类型和一个 Workload，并校验 Publisher key、清单签名和所有文件哈希。 |
| FR-PKG-005 | 必须拒绝 zip traversal、超限文件数、解压总量和压缩炸弹。 |
| FR-PKG-006 | 第三方 package 不得包含 Front Host/native plugin。 |
| FR-PKG-007 | Guest 必须从已验证本地 bytes 加载，不得由 Keel 直接信任远端 URL。 |
| FR-PKG-008 | 同一 Program Release 的 Service、Front 或 Privacy Contract 绑定发生变化时必须拒绝。 |
| FR-PKG-009 | Service 实例仍在引用发布版本时，必须保留 Service、Front 包及绑定的 Privacy Contract 缓存。 |
| FR-PKG-010 | Marketplace metadata 不得覆盖 Publisher package identity。 |
| FR-PKG-011 | Product v1 的本地导入必须支持配套的 Service 与 Front `.actr` 文件。开发来源必须明确标记。 |
| FR-PKG-012 | 必须支持 Advisory/Block new/Quarantine/Critical block 撤销等级。 |
| FR-PKG-013 | Publisher bootstrap 必须固定 HTTPS 443；Publisher Profile 必须有规范化签名、序号/时效和回滚保护。 |
| FR-PKG-014 | 官方发布工具必须构建、验证并签名两个包，发布签名记录，再回读核对两个包的 SHA-256 与签名。 |

### 1.4 Front/Keel

| ID | 需求 |
|---|---|
| FR-FRONT-001 | Front Host 必须随 Actrium 发布且 Publisher 不可替换。 |
| FR-FRONT-002 | v1 正式 Guest 只支持 Keel JS Guest。 |
| FR-FRONT-003 | 必须固定 QuickJS 并启用硬 timeout 与 heap cap。 |
| FR-FRONT-004 | Guest 模块、capability、组件和 RemoteRef 必须白名单。 |
| FR-FRONT-005 | Guest 不得获得默认 fetch/socket/filesystem/native module。 |
| FR-FRONT-006 | Guest 只获得固定 target 的 Service capability。 |
| FR-FRONT-007 | 所有 Guest RPC/input/output/event 必须 schema validate。 |
| FR-FRONT-008 | Image 和资源组件不得形成未声明网络出口。 |
| FR-FRONT-009 | Engine close/crash 必须清理 timer、callback、listener 和 capability。 |
| FR-FRONT-010 | UI 必须把 v1 描述为 same-process sandbox，不得声称 OS process isolation。 |

### 1.5 Service 与 Workspace

| ID | 需求 |
|---|---|
| FR-SVC-001 | 本地新建必须创建持久 Service ActorId。 |
| FR-SVC-002 | Service 重启、续期、signaling 切换后必须保留 ActorId。 |
| FR-SVC-003 | Service 实例必须固定 Program Release 和它所运行的 Service 包摘要。 |
| FR-SVC-004 | Service 必须有独立于 ActrType 的 per-instance store。 |
| FR-SVC-005 | Workspace 必须由 Service Host broker 持有，Front 不得获得路径/credential。 |
| FR-SVC-006 | v1 默认 Workspace 写入模式为 exclusive writer。 |
| FR-SVC-007 | 创建事务必须幂等且崩溃可恢复。 |
| FR-SVC-008 | Stop、Archive、Delete Service 与 Delete Workspace 必须是不同操作。 |
| FR-SVC-009 | 第三方 Service 默认必须使用受限 WASM Component workload。 |
| FR-SVC-010 | Service 可以编排 Realm 内的其他 Actor，但 Front 只绑定 Service。 |
| FR-SVC-011 | v1 版本升级必须走新 Actor + 显式迁移。 |
| FR-SVC-012 | Service 彻底清除后 ActorId 不得复用。 |
| FR-SVC-013 | 静态 Workspace 要求必须通过 Service manifest 的 `[workspace]` 表声明，字段为 `requirement`、`access` 和 `providers`。省略此表等同于 `requirement = "none"`。准备方式和实际绑定属于实例设置。必需的绑定满足声明的访问要求后，Service 才能进入 Ready。 |
| FR-SVC-014 | 移入回收站必须保留身份与实例数据并可恢复；彻底清除必须撤销身份且不可恢复。 |
| FR-SVC-015 | 生命周期、可达性、健康度、placement 与 attachment 状态必须分字段记录。 |
| FR-SVC-016 | v1 新建实例只在当前设备的受支持本地 Actr Host 创建；远端管理必须另有宿主授予的 management capability。 |

### 1.6 Realm、身份与网络

| ID | 需求 |
|---|---|
| FR-NET-001 | Actrium 必须内置 signaling client 和本机 Realm 支持，不写死唯一全球服务器。 |
| FR-NET-002 | 域名的 Realm 签名 key（authority descriptor 的 `realm_signing_keys` 成员，已在 Actr Authority Discovery §4.4 定义）必须通过 HTTPS 获取；Realm Profile 必须由已信任的 Realm 签名 key 签名，其有效性不取决于获取位置。 |
| FR-NET-003 | 一个 Realm 必须可委派多个 signaling/TURN endpoint。 |
| FR-NET-004 | v1 Front 与 Service 必须处于同一 Realm，即同一应用实例的 Realm；类型发现只在本 Realm 内进行。 |
| FR-NET-005 | Actor credential 必须绑定 ActorId、Actor key、Actor 类型、Realm 和有效期。 |
| FR-NET-006 | Service identity 必须支持凭长期 key reclaim 同一 ActorId。 |
| FR-NET-007 | signaling credential 不得放入长期 URL query。 |
| FR-NET-008 | TURN 必须使用短期、限 audience、可轮换 credential。 |
| FR-NET-009 | Actr Realm 与 TURN auth realm 必须使用不同类型/命名。 |
| FR-NET-010 | endpoint 可以使用非标准端口，但必须来自可信 Realm Profile 或 Publisher Profile。 |
| FR-NET-011 | Service offline 必须是可区分状态，不能退化为同类型发现。 |
| FR-NET-012 | v1 的多个 signaling endpoint 只能作为同一 Realm 控制面的 failover，不支持隐式跨控制面 federation。 |
| FR-NET-013 | 本机 Realm 的 ActorId 必须使用本机伪域名 `<installation-id>.actr.alt`，禁止复制为跨设备链接。 |
| FR-NET-015 | Realm 域名 bootstrap（`https://<domain>/.well-known/actr`）必须固定 HTTPS 443，并以 Realm 签名 key 轮换链与 Realm Profile 序号防回滚（Realm Profile 不设过期时间）。 |
| FR-NET-016 | Actrium 必须提供域名信任、经 Realm 邀请加入实例 Realm、添加客户端、重新认证、客户端撤销、成员管理（仅 owner）、离开、owner 恢复和 Realm 签名 key 变化处置流程。 |
| FR-NET-017 | 新建实例必须在所选域名下新建一个 Realm；RealmId 在域名内唯一、创建后不可修改、退役后不复用。 |
| FR-NET-018 | Service 升级为新 Program Release 时，Realm 必须发布 `package` 指向新包的 Realm Profile，新 Service 必须创建在同一 Realm；迁移到另一个 Realm 必须产生新 ActorId。 |
| FR-NET-019 | 取不到、验不过或已退役的 Realm Profile 时，Actrium 不得使用 Realm Profile 以外的任何 Actr Facility Provider（包括 authority descriptor `services` 中的服务）与该 Realm 通信。 |
| FR-NET-020 | 新建实例所选的 Realm 域名必须已受信，或在新建确认页完成首次信任；没有域名不分配 Realm（本机 Realm 除外）；创建须经该 Realm 域名的 Realm 签名 key 持有方授权：其他 Realm 域名由持有方的签名服务签署 Actr 管理信封，确认页显示信任根为该 Realm 域名；Realm 域名等于或属于 Actr Facility Provider ID 时，v1 只由该 Actr Facility Provider 的运营者创建，用户以其交付的 owner 邀请加入，确认页显示其为信任根。 |
| FR-NET-021 | 每个联网 Realm 必须恰好一个 owner（创建者或经转让得到者）；owner 转让必须原子完成，原 owner 默认改为 editor；owner 离开前必须先转让，且不能撤销自己最后一个客户端；多 owner 不属于 v1。 |
| FR-NET-022 | 每个客户端必须在每个 Realm 登记独立的 Realm 客户端 key；注册、重签与续期 Actor credential 都必须以该 key 签名证明持有，Issuer 只对已登记、未撤销、属于 active 成员的 key 签发；不得使用 Realm 共享注册 secret。 |
| FR-NET-023 | 撤销 Realm 客户端 key 必须立即拒绝其新 Actor credential、撤销它签发的未用邀请、停用绑定到它的 Actor 并关闭它们的 signaling 连接与 TURN allocation；移除成员与离开 Realm 必须撤销该成员全部客户端 key；改角色只改应用权限，在该成员下一份 Actor credential 生效。 |
| FR-NET-024 | 创建 Realm 与 owner 恢复必须以一次性 owner 邀请交付管理权，不设 Realm 管理 token；owner 邀请由 Realm 签名 key 持有方（激活响应、`reissue_owner_invite`）或 Actr Facility Provider 运营者签发，新签发的使此前未用的失效。Actr Facility Provider 账号自助创建属于后续能力。 |

### 1.7 Permission、安全与隐私

| ID | 需求 |
|---|---|
| FR-SEC-001 | effective capability 必须为声明、Actr Host 支持、用户/管理员授权、平台策略的交集。 |
| FR-SEC-002 | 权限 grant 必须绑定 Publisher/Program/版本或 digest 与 scope。 |
| FR-SEC-003 | 新增或扩大高风险权限必须重新确认。 |
| FR-SEC-004 | ActorId 必须被视为公开地址而非访问 token。 |
| FR-SEC-005 | Actor key、Realm 客户端 key、Workspace Provider credential 必须进入 OS 安全存储。 |
| FR-SEC-006 | 日志和 crash report 不得记录 secret、业务 payload 或完整 Workspace 路径。 |
| FR-SEC-007 | package/Realm/网络 URL 必须实施 SSRF 与 redirect policy。 |
| FR-SEC-008 | 必须支持 signed emergency blocklist、过期和审计。 |
| FR-SEC-009 | 回收站、隔离或彻底清除 Service 均不得自动删除 Workspace。 |
| FR-SEC-010 | 遥测必须默认关闭；用户或管理员启用后仍必须最小化敏感标识。 |
| FR-SEC-011 | 撤销 grant 必须立即失效对应 capability handle，并终止或降权活跃会话。 |
| FR-SEC-012 | Realm 邀请必须一次性、可过期、可撤销，兑换时由新 Realm 客户端 key 签名并原子消耗，且不得作为 bearer secret 进入实例 URI；过期、已使用、已撤销与 Realm 不匹配必须是不同错误。 |
| FR-SEC-013 | 应用权限至少支持 owner/editor/viewer 角色，并把 Realm membership 与实例授权分开：membership 只决定能否进入该实例的网络（取得 Actor credential、连接 signaling、Attach 握手的前提），实例内角色与权限仍由应用权限与会话授权决定；成员角色由 Actr Facility Provider 保存并写入 Actor credential，由 Service 执行，会话不超过 Front credential 的有效期。 |
| FR-SEC-014 | 应用颜色必须与 lifecycle、reachability、health 和 enforcement 分字段表达。 |
| FR-SEC-015 | 应用图必须是无根有效依赖连通分量，并包含全部有效 `Transfer(ActorId)` 成员。 |
| FR-SEC-016 | 通道有效且 `Ready` 时，展示颜色必须取全部节点结论的最弱项。 |
| FR-SEC-017 | 顶层只分绿 / 黄 / 红；切面未就绪不得展示颜色；未知不得按绿处理。 |
| FR-SEC-018 | 被评估者不得自签最终 `ActorAssurance`；有效声明由 Actr Host 闭合，结论由验证者签发。 |
| FR-SEC-019 | `ActorAssurance` 必须签名，并绑定 `ActorId + ActorStateDigest`、证据指纹、策略、规则和有效期。 |
| FR-SEC-020 | 新 Actor 边必须在对端 `ACTIVE` 且当前 AA 可接受后，才能收到应用数据。 |
| FR-SEC-021 | package、Actr Host 证据键、credential、Actor Grant、策略或结论撤销必须向最弱项传播并同步执行点。 |
| FR-SEC-022 | 无效依赖结束后，该 Actor 才不再进入下一份 `structureDigest`。 |
| FR-SEC-023 | 第三方 in-process NativeCdylib 不得为 `CONTROLLED`；不得靠声明、签名或静态扫描提升。 |
| FR-SEC-024 | Attach 必须协商策略、nonce、当前 `ActorStateDigest` 与 `ApplicationStateDigest`，不得依赖 GraphRoot/epoch。 |
| FR-SEC-025 | 完整 Actor 图必须最小披露，不得默认发送给 signaling、普通 peer 或遥测。 |
| FR-SEC-026 | 能接触受保护数据的 opaque HTTPS 必须作为非 Actor 汇明确授权；TLS 本身不得证明其内部隐私。 |
| FR-SEC-027 | 每条受保护 Actor 调用必须按当前状态与数据世代检查权限。 |
| FR-SEC-028 | WASM 只有在声明、imports、有效能力和可能/观测出口形成可强制闭包时才能为 `CONTROLLED`。 |
| FR-SEC-029 | NativeCapsule 只有在第三方代码执行前完成独立进程 OS sandbox、关闭原始出口并只开放 brokered IPC 时，才可以评为 `CONTROLLED`。 |
| FR-SEC-030 | 三色内部必须保留 identity、artifact provenance、workload confinement、compute confidentiality、channel security、storage protection、egress closure 和 output control 等多维事实。 |
| FR-SEC-031 | 每条网络 Actor edge 必须具有双向认证、机密性、完整性、前向保密和防重放；signaling、TURN 与普通 relay 不得持有 payload key。 |
| FR-SEC-032 | Actor Secure Channel 握手必须绑定两端 ActorId、当前 `ActorStateDigest`、runtime、credential、临时 key、目标 Service、policy、attestation result、nonce 和角色，并双向认证联合 transcript。 |
| FR-SEC-033 | transport TLS/DTLS/QUIC 成功不能单独生成 channel security pass；必须完成 Actor 身份、运行实例和 session key 绑定。 |
| FR-SEC-034 | Workspace、snapshot、backup、cache、index 和临时文件必须按 data class 实施 envelope encryption；ActorId 不得充当密钥。 |
| FR-SEC-035 | Data Policy 必须按数据类别和操作声明 required assurance dimensions；非 required 维度不得使公共数据无条件 unknown。 |
| FR-SEC-036 | 远程证明必须区分 Attester、Verifier 与 Relying Party；Evidence/self-assertion 不得直接成为授权决定。 |
| FR-SEC-037 | Attestation 必须绑定 fresh nonce、Actor/runtime、workload/package digest、policy、audience 和当前 ephemeral channel/decryption key。 |
| FR-SEC-038 | policy 要求 confidential compute 时，private/secret DEK 只能在 appraisal 通过后封装给 Attestation 覆盖的 ephemeral key。 |
| FR-SEC-039 | Confidential Computing 只能满足对应 compute confidentiality claims，不得替代 WASM/Native confinement、egress closure 或 output control。 |
| FR-SEC-040 | 使用 GPU/NPU 的 confidential workload 必须组合评估 CPU TEE、guest、runtime、driver、IOMMU/device assignment、device firmware、CC mode、link 和 device memory。 |
| FR-SEC-041 | 任一可见明文的 accelerator 环节缺失有效 Evidence 时，compute confidentiality 必须为 indeterminate/unknown。 |
| FR-SEC-042 | 厂商原生 Evidence 必须经签名、版本化、可撤销的 Verifier adapter 映射为 canonical claims；厂商型号或营销声明不得直接映射为 pass。 |
| FR-SEC-043 | Evidence、Attestation Result、Reference Value、Endorsement 或 TCB 状态到期/撤销时必须停止新的敏感数据与 key release，并触发 re-attestation/rekey。 |
| FR-SEC-044 | 高敏输出跨越 data class 时必须经过显式 declassification/output policy；合法 Actor response 不得成为绕过 egress closure 的隐蔽出口。 |
| FR-SEC-045 | 绿色必须绑定当前 `ApplicationStateDigest`（`structureDigest` + Principal + Merkle 根）且 `Ready`；不得给 Publisher 或 Program 笼统绿标。 |
| FR-SEC-046 | Program Privacy Contract 必须静态声明允许的 Actor roles/types/profiles、cardinality、edges、data classes、capabilities、terminals、audit sinks 和 assurance requirements。Host 将其用于授权前，必须按签名 Program Release 记录验证契约原始字节的 SHA-256，并核对发布版本和包绑定。 |
| FR-SEC-047 | Actr Host 可发放的有效 capability 必须是 Contract、用户/组织 policy、Host policy 和 instance grant 的交集；未声明能力不得获得 handle。 |
| FR-SEC-048 | 实际 Actor/runtime 图必须是 Contract graph grammar 的实例；所有 mutation 必须先形成 candidate graph，不得直接改变已获批数据图。 |
| FR-SEC-049 | 初始图必须在释放受保护数据或 DEK 前完成身份、证据、策略和 `ApplicationAssurance` 评估。 |
| FR-SEC-050 | 无法满足基础 profile 的 optional 功能必须拆分为独立 feature profile/capability；禁用后不得污染已认证基础闭包。 |
| FR-SEC-051 | 受信 Verifier 可以为内部闭合子图签发有 scope、policy、evidence root、有效期和撤销信息的 Certified Privacy Domain aggregate；上游可以按 policy 将其作为单一节点。 |
| FR-SEC-052 | Actor/Service 的 self-signed privacy declaration 不得形成 Assured Actor Edge；Verifier trust、certificate scope、endpoint、session 和 data class 必须全部匹配。 |
| FR-SEC-053 | 所有审计日志必须是 Contract 中的显式 audit sink，并声明字段、payload、data class、retention、operator visibility、加密和导出规则。 |
| FR-SEC-054 | `ActorStateDigest` 变化必须本地挂起并废除旧权力；不依赖者可 `LocalContinue`，但不得展示应用绿；控制面不得夹带业务数据。 |
| FR-SEC-055 | 颜色未减弱且策略允许时可以自动恢复；仍须 `Ready` 才展示绿。 |
| FR-SEC-056 | 颜色变弱时必须保持阻断并显示原因，只有风险接受后才能继续相关工作；风险接受不改颜色。 |
| FR-SEC-057 | 风险接受不能把缺失证明提升为绿；平台硬失败和无效通道不得由用户覆盖。 |
| FR-SEC-058 | 图结构不变但证据到期、验证者 / TCB / 凭据撤销、策略或状态指纹变化时，同样必须重评。 |
| FR-SEC-059 | GPU/NPU Attestation 不得被解释为 LLM Server 无日志；只有被度量的日志配置、封闭 egress/audit capabilities 或可信 Certified Privacy Domain retention claim 才能评估日志口子。 |
| FR-SEC-060 | `ActorState.revision` 必须由状态权威单调递增并进入 `ActorStateDigest`；内容回滚不得复现旧状态根，旧 Actor Grant / 结论不得因回滚恢复有效。 |
| FR-SEC-061 | 可出示 Actr 身份的端点必须按 Actor 目标评定，不得声明为 HTTPS/域名汇绕过跨 Principal 评分；目标类别不可判定时该出口保持 `UNKNOWN`；信任汇的授权界面必须呈现为「数据离开保障范围」。 |
| FR-SEC-062 | 执行方与评定方用于期限、证据与凭证有效期核对的本地时钟均不可回拨。 |
| FR-SEC-063 | `EncGraph` 标识符必须落在受限 ASCII 字符集；`ApplicationStateDigest` 叶子必须按 `ActorId` 排序且每个 `ActorId` 恰好一个叶子。 |

## 2. 非功能需求

### 2.1 性能

以下预算不包括首次 package 下载和用户停留时间：

| ID | 指标 | v1 目标 |
|---|---|---|
| NFR-PERF-001 | 已缓存本地 Service Attach 到可交互 | p50 ≤ 800 ms，p95 ≤ 2 s |
| NFR-PERF-002 | 已缓存远端 Service Attach | 网络可用时 p50 ≤ 2 s，p95 ≤ 5 s |
| NFR-PERF-003 | Front Guest cold load | p95 ≤ 1.5 s |
| NFR-PERF-004 | 输入到原生 UI 更新 | p95 ≤ 100 ms，不含远端 RPC |
| NFR-PERF-005 | 后台 Tab pause | ≤ 250 ms |
| NFR-PERF-006 | Service 本机重启恢复 | p95 ≤ 3 s |

### 2.2 资源

默认可配置预算：

| ID | 项目 | 默认 |
|---|---|---|
| NFR-RES-001 | Keel Guest heap | desktop 64 MiB；mobile 48 MiB |
| NFR-RES-002 | Guest 单次 JS turn | 100 ms；启动阶段可单独给 1 s 总预算 |
| NFR-RES-003 | Guest render nodes | 10,000 |
| NFR-RES-004 | 单 batch render ops | 5,000 |
| NFR-RES-005 | Front RPC 并发 | 32 |
| NFR-RES-006 | 默认 package 下载大小 | 200 MiB hard cap，可由企业策略降低 |
| NFR-RES-007 | archive 文件数 | 10,000 |
| NFR-RES-008 | archive 解压总量 | 500 MiB |
| NFR-RES-009 | Guest 调度 | 每个 Guest 使用 Actr Host 可中止的专用执行线程/worker，不占用 Shell/UI 主线程 |

任何提高都必须受平台上限和权限政策约束。

### 2.3 可靠性

| ID | 需求 |
|---|---|
| NFR-REL-001 | 创建事务在任意阶段崩溃恢复后不得产生两个 Service。 |
| NFR-REL-002 | Service 重启 10,000 次身份恢复测试中 ActorId 变化为 0。 |
| NFR-REL-003 | Attach 失败路径中替代实例创建次数为 0。 |
| NFR-REL-004 | package cache 写入必须原子，断电后旧版本仍可验证。 |
| NFR-REL-005 | Front crash 不得损坏 Service store 或 Workspace。 |
| NFR-REL-006 | 同一 Actr Facility Provider 内 signaling endpoint 故障切换不得改变 ActorId。 |
| NFR-REL-007 | lease 网络抖动测试不得产生两个同时有效的 exclusive attachment。 |
| NFR-REL-008 | beta 的 crash-free app session 必须达到 99.5%，stable 必须达到 99.9%。 |

### 2.4 安全

| ID | 需求 |
|---|---|
| NFR-SEC-001 | 所有 parser 必须纳入持续 fuzz。 |
| NFR-SEC-002 | 第三方 package/Guest/Service 不得绕过 capability allowlist。 |
| NFR-SEC-003 | secret scanner 测试不得在 URI、日志、crash fixture 中发现真实 credential。 |
| NFR-SEC-004 | 高危安全边界在公开 beta 前完成独立渗透测试。 |
| NFR-SEC-005 | 安全更新和 emergency blocklist 验证失败时 fail closed。 |
| NFR-SEC-006 | 发布构建必须生成 SBOM 和签名 provenance。 |
| NFR-SEC-007 | 节点结论变弱后，相关受保护效果必须在下一次执行前同步失效，Shell 更新 p95 ≤ 250 ms。 |
| NFR-SEC-008 | 1,024 Actor、4,096 edge 以内的增量最弱项汇聚在 reference desktop 上 p95 ≤ 100 ms；超限必须 unknown。 |
| NFR-SEC-009 | 图环、epoch 竞争、Mark 过期、依赖撤销、未声明出口和 Actr Host 隐瞒模拟必须纳入 property/fault-injection tests。 |
| NFR-SEC-010 | Actor channel、Attestation Result 和 Evidence Bundle 必须具有跨实现测试向量、重放/错绑/降级负向测试。 |
| NFR-SEC-011 | 每个 production Verifier profile 必须记录 trust root、覆盖的攻击面、未覆盖项、最低 TCB、freshness、撤销源和 conformance tests。 |
| NFR-SEC-012 | Verifier/Key Broker 暂时不可用时不得无限期复用过期 Result；grace window 必须由 data policy 明确限制。 |
| NFR-SEC-013 | graph mutation 请求、旧 epoch lease 冻结与 candidate edge 创建必须线性化；并发测试中不得出现 mutation 后、冻结前的 payload 窗口。 |
| NFR-SEC-014 | downgrade partial order、不可比较 vector、自动恢复、用户拒绝和 hard-floor 不可覆盖必须纳入 property/fault-injection tests。 |

### 2.5 可访问性与国际化

| ID | 需求 |
|---|---|
| NFR-A11Y-001 | Shell 和系统页面达到 WCAG 2.2 AA 对应平台要求。 |
| NFR-A11Y-002 | 所有安全状态有文字/图标，不只靠颜色。 |
| NFR-A11Y-003 | 支持键盘、屏幕阅读器、动态字体和高对比度。 |
| NFR-I18N-001 | 稳定错误码与本地化文案分离。 |
| NFR-I18N-002 | IDNA 域名可同时检查 Unicode 与 A-label。 |

### 2.6 可维护性

| ID | 需求 |
|---|---|
| NFR-MNT-001 | URI、manifest、Realm Profile 和 session protocol 必须版本化。 |
| NFR-MNT-002 | 各平台共享同一规范化与验证测试向量。 |
| NFR-MNT-003 | Host capability 和 Safe Component schema 必须由单一源生成类型/测试。 |
| NFR-MNT-004 | 所有状态机必须有可恢复 transition 和稳定 error code。 |

## 3. 端到端验收场景

### AC-001：首次创建

**Given** 用户在 Program Catalog 选中未安装的 `notes.example/team-notes@1.4.2`<br>
**When** 新建实例并确认权限/Workspace<br>
**Then** Actrium 分别验证 Service 包和 Front 包，只创建一个 Realm 与一个 Service，获得持久 ActorId，地址栏显示该实例的 Realm 链接，Attach 并显示已验证的 Guest。

### AC-002：刷新不重复创建

**Given** 创建刚完成<br>
**When** 用户连续刷新、崩溃恢复并后退/前进<br>
**Then** Service Inventory 中仍只有原 ActorId。

### AC-003：精确实例 offline

**Given** Realm 链接所指实例的 Service offline<br>
**When** 打开该链接<br>
**Then** 本地 Inventory 已知 stopped 时显示启动入口；远端没有宿主状态证明时显示 offline/unreachable；两者均不得创建同类型实例或调用类型发现。

### AC-004：Service 类型不匹配

**Given** 对端 Service 的 credential 中的 Actor 类型与 Realm Profile 固定的 Service 包类型不同<br>
**When** Attach 握手<br>
**Then** 在任何 Guest 执行前以 `SERVICE_ACTOR_TYPE_MISMATCH` 失败，不创建替代实例。

### AC-005：同版本异包

**Given** 本地已固定一个 Program Release 的 Service 和 Front 包摘要<br>
**When** 同一 Program Release 的任一包出现不同摘要<br>
**Then** 以 `PACKAGE_EQUIVOCATION` 阻止安装和运行变更的归档，保留原有摘要供审计。

### AC-006：Front Guest 越权

**Given** Manifest 未声明 network<br>
**When** Guest 尝试 require 网络模块、使用 Image headers 或调用 `host:net`<br>
**Then** 所有路径均被拒绝，并产生不含秘密的审计事件。

### AC-007：Guest 卡死

**Given** Guest 执行无限循环<br>
**When** 超过 turn timeout<br>
**Then** 仅该 Keel Engine 被终止，Service 和其他 Tab 保持可用。

### AC-008：Tab 关闭

**Given** 一个运行中的实例<br>
**When** 关闭 Tab<br>
**Then** Front/session 销毁，Service/Workspace/ActorId 保留并可从 Inventory 重开。

### AC-009：Actrium 重启

**Given** 本地 Service 已创建<br>
**When** 完全退出并重启 Actrium<br>
**Then** Service 用同一 ActorId恢复；收藏和历史中的 Realm 链接仍可连接。

### AC-010：独占会话

**Given** Service 已被一个 Front 持有 exclusive lease<br>
**When** 第二设备 Attach<br>
**Then** Service 返回 occupied；未授权情况下不得建立第二个 active attachment。

### AC-011：signaling endpoint 故障切换

**Given** signaling A 故障、Realm Profile 声明 signaling B<br>
**When** Actor 重连<br>
**Then** 使用 B 且 ActorId 不变；未声明 endpoint 不被尝试。

### AC-012：TURN

**Given** P2P 不可达<br>
**When** 使用 TURN<br>
**Then** 使用该 Realm 的 Issuer 签发的短期 audience TURN credential，UI 显示 relayed，日志不含密码。

### AC-013：Workspace 冲突

**Given** Workspace 已有 exclusive writer Service<br>
**When** 创建第二个写 Service<br>
**Then** 创建前明确拒绝或要求迁移/只读，不发生静默多写。

### AC-014：回收站与彻底清除

**Given** Service 绑定外部 Workspace<br>
**When** 用户先移入回收站再恢复<br>
**Then** Service 用同一 ActorId 恢复，Workspace 内容不变；随后用户彻底清除但未选择删除 Workspace 时，Actor credential/key/实例 store 被删除且不可恢复，Workspace 仍保留。

### AC-015：升级失败回滚

**Given** v1 Service 和 Workspace<br>
**When** v2 迁移验证失败<br>
**Then** v1 ActorId、Workspace lease 和旧 Tab 可恢复，v2 不接管。

### AC-016：域名过户/新 key

**Given** 已安装版本由旧 key 签名<br>
**When** Publisher Profile 无轮换链地出现新 key<br>
**Then** 旧固定 digest可离线运行，新安装/更新显示身份变化并要求确认。

### AC-017：移动平台 suspend

**Given** 本地 Service 在移动端后台被系统暂停<br>
**When** 远端或本机 Front连接<br>
**Then** 显示 suspended/host offline，不报告不存在，也不新建替代实例。

### AC-018：安全撤销

**Given** digest 被标记 Critical block<br>
**When** 用户打开已有实例<br>
**Then** Guest/Service 不加载，Workspace 导出和安全说明仍可用。

### AC-019：Local Realm 链接

**Given** Realm 链接使用另一安装的本机伪域名 `<installation-id>.actr.alt`<br>
**When** 当前设备打开或尝试分享<br>
**Then** 以 `URI_LOCAL_REALM_FOREIGN` 拒绝连接，并禁用分享；不得把它解释为某个 DNS 域名下的 Realm。

### AC-021：Realm Profile 回滚

**Given** 本地已固定 Realm 签名 key 轮换链，并接受 sequence 31 的 Realm Profile<br>
**When** endpoint 返回仍有效但 sequence 30 的 Realm Profile，或由未经旧 Realm 签名 key 授权的新 key 签名的 Realm Profile<br>
**Then** 以稳定安全错误拒绝，不切换 Issuer/signaling endpoint。

### AC-022：邀请不是 URL 权限

**Given** owner 给成员签发一次性 editor 邀请<br>
**When** 成员接受后再次使用邀请，或第三方只获得相同的 Realm 链接<br>
**Then** 邀请重放失败；第三方仍无应用权限；Realm 链接中不存在邀请 secret。

### AC-023：远端管理边界

**Given** Inventory 中的远端 Service 只授予 Attach、未授予 host management<br>
**When** 用户点击启动、停止或清除<br>
**Then** Actrium 显示需要远端宿主授权，不伪造成功、不在本地创建替代 Service。

### AC-024：必要的其他 Actor 失陷

**Given** Front 与 Service 当前均为绿，但 Realm 内一个持有应用数据的必要 Actor 的包被 Critical 撤销<br>
**When** 撤销事件到达任一 Actr Host<br>
**Then** 该节点为红，应用最弱项为红，相关 Actor Grant 被撤销；不得以「多数 Actor 安全」继续显示绿色。

### AC-025：证据未知不是安全

**Given** 其他 Actor 均为 `CONTROLLED`，但一份远端 `ActorAssurance` 已过期<br>
**When** 应用评定<br>
**Then** 整体为黄或无法建立，不得显示绿色；应用数据默认不再流入该 Actor。

### AC-026：可选功能安全隔离

**Given** 可选索引 Actor 变为 `UNKNOWN`<br>
**When** Actr Host 拒绝其边、撤销 capability、确认不再持有受保护数据并换 `structureDigest`<br>
**Then** 索引功能禁用；隔离完成前不得展示绿；移出图并重新 `Ready` 后剩余图可以再绿。

### AC-027：动态依赖先验证

**Given** Service 运行中首次请求一个新 Actor<br>
**When** 对端尚无当前 AA 或仍挂起<br>
**Then** 不得发送应用数据；不得把应用显示为绿。

### AC-028：恶意 Service 隐瞒依赖

**Given** Service 不在自报材料中声明某个下游 Actor<br>
**When** 它通过受控能力发起 `Transfer(Actor)`<br>
**Then** 该边仍进入无根图并重评；自报不能覆盖实际有效依赖。

### AC-029：安全图成环或超限

**Given** Realm 内的 Actor 形成调用环，或图超过策略上限<br>
**When** 汇聚<br>
**Then** 环按无向连通分量去重并稳定求值；超限返回 `GRAPH_LIMIT_EXCEEDED`，不能截断后发绿。

### AC-030：健康与安全正交

**Given** Actor 功能正常但它是第三方 in-process Native，同时另一个 WASM Actor 崩溃但出口闭包仍有效<br>
**When** UI 展示状态<br>
**Then** 前者显示 healthy + 黄，后者显示 failed + 绿（若切面仍 Ready）；不得合并成一个模糊状态。

### AC-031：外部 HTTPS 不是自动安全

**Given** 已绿的应用把 Workspace 内容发送到允许域名的 HTTPS API，但该 API 没有满足严格策略的证明<br>
**When** 评定该非 Actor 汇<br>
**Then** 它只能作为策略明确授权的终端汇；若策略要求验证其后续处理，该项为 `UNKNOWN`。TLS 主机名验证不能把应用提升为绿。

### AC-032：Native Capsule 约束

**Given** 同一 native workload 分别以 NativeCdylib 和 NativeCapsule 运行<br>
**When** Capsule 在执行代码前已进入独立 OS sandbox，禁止 socket/任意文件/子进程并只开放 Host typed IPC<br>
**Then** NativeCdylib 不得为 `CONTROLLED`；Capsule 可以按实际 sandbox 证据评定。任一平台约束未生效时 Capsule 必须 fail closed 或保持未知。

### AC-033：TURN 与信令不能解密

**Given** Actor A 与 Actor B 只能经恶意 signaling 和 TURN relay 通信<br>
**When** relay 记录、重放或替换协商消息<br>
**Then** relay 只能观察被最小化的连接元数据，不能得到 payload key 或明文；身份/handshake transcript 不匹配时 Actor edge 建立失败。

### AC-034：CC 不替代 Workload 约束

**Given** Service 正确运行在通过 Attestation 的 confidential VM 中，但 workload 具有未声明网络出口<br>
**When** 计算 Assurance Vector<br>
**Then** compute confidentiality 可以 pass，但 egress 为 `UNCONTROLLED`，应用为红；不得因硬件 TEE 把恶意 workload 标绿。

### AC-035：证明与当前会话错绑

**Given** 攻击者持有另一台合规 TEE 的新鲜合法 Attestation Result<br>
**When** 它将该 Result 用于当前 Actor/runtime、policy、nonce 或 ephemeral key 不同的连接<br>
**Then** Relying Party 以 subject/channel binding mismatch 拒绝，不发送 private/secret payload。

### AC-036：证明通过后才释放密钥

**Given** secret Workspace 将在远端 confidential Actor 中处理<br>
**When** workload/guest image/TCB appraisal 尚未通过或 ephemeral key 未进入 Evidence<br>
**Then** Key Broker 不释放 DEK；通过后只把 DEK 封装给被证明的 key，普通 Actr Host 不获得明文 DEK。

### AC-037：CPU 与 NPU 组合证明

**Given** confidential VM 通过 CPU TEE 证明，但 Agent 把明文 tensor 发送到无法证明 firmware、device memory 或安全链路的 GPU/NPU<br>
**When** secret policy 要求 compute confidentiality<br>
**Then** compute confidentiality 为 unknown，edge 不接收 secret；只有完整 composite attestation 满足 policy 后才恢复。

### AC-038：Policy 按数据类别要求证据

**Given** 同一 WASM Actor 没有 CC Evidence，但 identity、provenance、channel 和 egress closure 均 pass<br>
**When** 它先处理 public 数据，再请求处理要求 CC 的 secret 数据<br>
**Then** public 操作可以为绿；secret 操作为黄或红，不能用 Actor 的单一静态等级覆盖两种决策。

### AC-039：证明过期与撤销

**Given** 正在运行的 confidential Actor 的 Attestation Result 到期，或 vendor TCB advisory 撤销其参考值<br>
**When** 下一次 private/secret 操作或撤销事件到达<br>
**Then** 停止新 payload 和 key release，触发 re-attestation/rekey；不得无限期沿用旧 Result。

### AC-040：合法响应不能偷带 Secret

**Given** workload 只有向 Front 返回类型化结果的合法 Actor edge，却尝试把 secret 编码进 public 字段<br>
**When** output schema/data class 跨级且没有 declassification capability<br>
**Then** Actr Host 拒绝输出并将 output control 标为 fail；通道合法不能覆盖数据分类策略。

### AC-041：应用实例初始绿标

**Given** 新建 Application Instance 已确定 Service ActorId、Program/Workspace digest 和初始 candidate graph<br>
**When** 所有 Actor/Edge/Storage/Compute Evidence 满足当前 policy<br>
**Then** Actr Host 在 `Ready` 的 `ApplicationAssurance` 为绿之后，才展示绿色并发放受保护数据的 lease。结论绑定 `structureDigest` 与当前状态根，不是 GraphRoot。

### AC-042：声明就是能力边界

**Given** Program Privacy Contract 未声明某 Actor role、网络 origin 或 audit sink<br>
**When** workload 在运行时尝试建立对应 edge 或申请 capability<br>
**Then** Actr Host 在创建 handle/发送数据前拒绝；不得先建立连接再把应用标为黄/红。

### AC-043：合规扩容自动恢复

**Given** Contract 允许 `ai-inference` role 的 cardinality 为 `0..n`，并要求每个实例满足 `confidential-ai-v1`<br>
**When** Scheduler 请求加入三个不同 ActorId、且 assurance 不弱于 baseline 的推理 Actor<br>
**Then** 变化节点挂起并重评；颜色未减弱则自动恢复且不询问用户。创建未声明 role 必须失败。不依赖者可 `LocalContinue`，但不得展示应用绿直至 `Ready`。

### AC-044：可信 Actor Edge 封装内部子图

**Given** 外部推理服务不披露内部 Actor 图，但提供受本地 policy 信任的 Verifier 所签 Certified Privacy Domain aggregate<br>
**When** aggregate 的 endpoint、profile、data class、evidence root、有效期和当前 session 全部匹配<br>
**Then** 上游可以把该 domain 作为一个节点汇聚，结论取最弱项；若只是自签或 scope 不匹配，则为 `UNKNOWN` 或 `UNCONTROLLED`。

### AC-045：服务器审计日志可识别

**Given** LLM Server 的 NVIDIA/Ascend 类 accelerator Attestation 通过，但其上层服务可能保留 prompt/response 审计日志<br>
**When** audit sink 已声明并由 measured config + sealed egress 或可信 Certified Privacy Domain 证明只记录允许字段、retention 和读取主体<br>
**Then** 日志口子进入图并可评估；只有 accelerator Evidence、无法证明内部 retention，或存在额外未声明明文日志时必须为 `UNKNOWN` 或 `UNCONTROLLED`。

### AC-046：图不变但证明撤销

**Given** 实例图结构未变且已有绿标，但随后 vendor 撤销当前 TCB reference value<br>
**When** 撤销到达 Actr Host / 验证者<br>
**Then** 在下一次受保护操作前废除相关权力并重评；不能因结构未变而沿用旧结论。

### AC-047：结构变化且安全降级

**Given** 绿标实例请求把原有 confidential LLM Actor 替换为未提供 compute Attestation 的同接口 Actor<br>
**When** Actr Host 建立 candidate graph<br>
**Then** 变化节点挂起；compute confidentiality 从 pass 降为 indeterminate，UI 显示替换 Actor 和受影响数据，只有风险接受后才可以在黄下继续。颜色仍是黄。

### AC-048：结构变化但安全不变或增强

**Given** 绿色切面把一个 `CONTROLLED` Actor 替换为 identity、confinement、compute、storage、egress 均不弱于原切面的 Actor<br>
**When** 新状态的证据评估通过并 `Ready`<br>
**Then** 自动恢复且不弹用户授权；变更写入本地审计。旧数据仍受取得世代约束。

### AC-049：用户不能覆盖硬失败

**Given** candidate Actor 的 workload digest 与证明不一致，或出现未声明 terminal egress<br>
**When** 用户尝试接受风险<br>
**Then** Actr Host 仍阻断，不提供把该项洗绿的按钮；用户同意不能把硬失败或未知提升为绿。

### AC-050：冻结不阻塞证明控制面

**Given** graph mutation 已冻结应用数据面<br>
**When** Actr Host 获取 Attestation、Reference Values 并向 Shell 展示降级 diff<br>
**Then** 最小控制面可以通信，但挂起节点及其依赖方不得处理应用数据；旧状态的 payload 不能通过。

### AC-051：新建实例即新建 Realm

**Given** 用户已信任域名 `team.example`<br>
**When** 选择 `notes.example/team-notes@1.4.2`，以 `team.example` 为 Realm 域名新建实例并确认<br>
**Then** 在 `team.example` 下新建一个 Realm（如 `k7m2q9xh@team.example`），其 Realm Profile 的 `package` 指向所选包，地址栏显示 `actr://team.example/k7m2q9xh`；再次新建得到另一个 Realm，而不是复用前者。

### AC-052：Realm 与 Actr Facility Provider 绑定

**Given** 已接受某 Realm 的 Realm Profile，随后收到一份 Realm 签名 key 签名有效、`sequence` 更高但 `provider.id` 不同的 Realm Profile<br>
**When** Service 与 Front 重连<br>
**Then** 拒绝该 Realm Profile 并报 `ACTR_FACILITY_PROVIDER_MISMATCH`，不连接新 Actr Facility Provider；同一 Actr Facility Provider 内轮换 Realm issuer key 的更高 `sequence` Realm Profile 正常生效，Realm 名与 ActorId 不变。

### AC-053：已退役 Realm

**Given** 域名所有者发布了 `status = retired` 的 Realm Profile<br>
**When** 打开该 Realm 的 Realm 链接<br>
**Then** 以稳定错误拒绝连接；同一 RealmId 不能被重新用于新 Realm。

### AC-054：Realm Profile 不可用不回退

**Given** 实例 Realm 的 Realm Profile 无法获取或签名验证失败，而该域名的 authority descriptor 在 `services` 中声明了 signaling 等服务<br>
**When** 用户打开该实例的 Realm 链接<br>
**Then** 显示 Realm 不可达或安全错误，不连接这些服务或任何其他 Actr Facility Provider，也不创建替代实例。

### AC-055：新建事务失败与重试

**Given** 用户在某个 Realm 域名下确认新建实例<br>
**When** Realm 签名 key 持有方不可达，或 Actr Facility Provider 激活 Realm 失败，随后用户以同一 `idempotency_key` 重试<br>
**Then** 失败时不产生 Service；重试继续同一个 pending Realm（同一 RealmId，Actr Facility Provider 以 `(realm, 请求方)` 识别），不重复创建 Realm；过期的 pending Realm 被自动清理。

### AC-056：邀请错误分明

**Given** owner 为 `k7m2q9xh@acme.com` 签发了一次性 viewer 邀请<br>
**When** 成员兑换后再次兑换；另一邀请已过期；第三个邀请已被 owner 撤销；第四个属于另一个 Realm 却被用于本 Realm<br>
**Then** 分别以 `invite_used`、`invite_expired`、`invite_revoked`、`invite_realm_mismatch` 拒绝；同一客户端重试首次兑换得到相同结果；两个客户端并发兑换同一邀请只有一个成功。

### AC-057：移除成员即撤销全部客户端

**Given** editor 有两个客户端，各自运行一个在线 Actor<br>
**When** owner 移除该成员<br>
**Then** 两个客户端的 Realm 客户端 key 均被撤销：注册、续期与重签立即以 `client_revoked` 或 `member_removed` 拒绝，其 Actor 的 signaling 连接与 TURN allocation 被关闭且不能重连；已建立的 Attach 会话至迟在其 credential 到期时由 Service 结束；另一成员被改为 viewer 时不撤销任何 key，其下一份 credential 带 `viewer`。

### AC-058：owner 转让与恢复

**Given** owner 只有一个客户端，Realm 另有 editor B<br>
**When** owner 试图撤销自己的客户端或离开；随后把 owner 转让给 B；之后 B 丢失全部客户端，Realm 签名 key 持有方签发新的 owner 邀请并被兑换<br>
**Then** 撤销与离开以 `owner_last_client`、`owner_must_transfer` 拒绝；转让后 B 为唯一 owner、原 owner 为 editor；兑换恢复邀请的客户端成为 B 的新客户端，B 原有客户端 key 全部撤销，此前未用的 owner 邀请失效。

## 4. 兼容性验收矩阵

每个发布候选至少覆盖：

| 维度 | 组合 |
|---|---|
| 平台 | macOS、Windows、iOS、Android |
| Service | local、remote |
| 网络 | loopback、direct WebRTC、TURN、offline、Actr Facility Provider failover |
| Package | cached、first download、revoked、digest conflict、dev |
| Workspace | requirement: none/optional/required；provisioning: managed/user-selected；access: read-only/read-write；local/remote/locked Workspace Provider |
| Lifecycle | create、refresh、close、crash、restart、suspend、trash、restore、purge、migrate |
| Privacy | Ready 切面、世代交集、LocalContinue、Program Privacy Contract、WASM 声明/未声明出口、NativeCdylib、NativeCapsule、过期 AA、红色的其他节点、隐瞒依赖、Assured Actor Edge、LLM 审计汇、opaque HTTPS、图环/上限、通道绑定、CC、密钥释放、CPU+GPU/NPU 组合证明、输出控制、撤销传播 |

### 4.1 平台能力承诺

| 能力 | macOS | Windows | iOS | Android |
|---|---|---|---|---|
| 本地 Create/Attach | 完整 | 完整 | 前台可用 | 前台可用 |
| 本地长期在线 Actr Host | 支持，受用户后台策略控制 | 支持，受用户后台策略控制 | 不承诺 | 仅 foreground service 条件承诺 |
| 远端 Attach | 支持 | 支持 | 支持 | 支持 |
| 后台 Guest | pause/可回收 | pause/可回收 | 系统决定、可回收 | 系统决定、可回收 |
| local-folder Workspace | 支持 | 支持 | security-scoped provider | SAF/provider |
| 远端 Actr Host 管理 | 有 management capability 时 | 有 management capability 时 | 有 management capability 时 | 有 management capability 时 |

Linux/通用服务器宿主、云端自动调度、浏览器 Web 版和跨 Realm federation 不属于 Actrium v1 客户端承诺；它们可以作为独立 Actr Host 后续定义。

## 5. 产品指标

### 5.1 核心漏斗

```text
链接/程序打开
→ package verified
→ user approved
→ Service created or attached
→ Guest interactive
→ reopened successfully
```

关注：

- 首次创建完成率；
- Attach 成功率；
- 重启恢复成功率；
- package 验证失败原因；
- Realm/网络失败分布；
- Guest/Service crash-free session；
- 权限拒绝率；
- Workspace 冲突率；
- 误创建重复 Service 数（目标 0）。

### 5.2 隐私

指标默认不采集：

- 完整 ActorId；
- Workspace 名/路径/内容；
- Guest route/query；
- RPC payload；
- 用户输入。

遥测默认关闭。发布指标优先由本地聚合、自愿诊断和不含业务标识的质量实验得到。

### 5.3 性能测量口径

- 使用每个平台公布的 reference hardware、固定 package fixture 和可复现网络 profile；
- 每项至少 30 次冷/热运行，报告 p50、p95、样本数和失败率；
- 「可交互」定义为 Guest 首屏 commit 完成、输入 handler 已注册且 Service contract 可调用；
- 远端指标分别报告 direct 与 TURN，不把 DNS、登录或用户授权时间混入 Attach；
- 发布报告保留原始匿名测量和构建 digest，以便回归比较。

## 6. 发布门槛

### Developer Preview

- URI/创建/Attach 本地闭环；
- fixed QuickJS；
- signed package；
- Service same-ID restart；
- 基本 Workspace broker。

### Public Beta

- 全平台；
- 联网 Realm、P2P/TURN；
- 权限和撤销；
- 故障注入、fuzz、渗透测试；
- Program/Service 管理 UI；
- 迁移与删除保护。

### v1 Stable

- 所有 MUST 需求通过；
- 全部 AC 场景自动化或可重复验证；
- 安全审计高危项清零；
- crash-free 和恢复指标达到发布阈值；
- 协议版本与兼容承诺发布；
- 运维、撤销、key compromise 和数据恢复 runbook 完成。

## 7. 关键目标追踪

| 产品目标 | 主要需求 | 验收 | 交付阶段 |
|---|---|---|---|
| 新建与打开无歧义 | FR-URI-001—009、FR-SVC-007 | AC-001—004 | P0、P2-B、P3-A |
| Tab/Front/Service 统一模型 | FR-SH-001—006、FR-FRONT-001 | AC-002、AC-008—010 | P2-A、P3-A |
| 精确、持久 Actor 身份 | FR-SVC-001—003、FR-NET-005—006 | AC-009、AC-011、AC-019 | P1-A |
| Publisher 与包供应链 | FR-PKG-002—014 | AC-005、AC-016、AC-021 | P1-B |
| Realm/信令有权但受限 | FR-NET-001—024 | AC-011、AC-012、AC-021—023、AC-051—058 | P1-A、P3-B |
| Workspace 不被 UI/删除误伤 | FR-SVC-005—016、FR-SEC-009 | AC-013—015、AC-023 | P2-B |
| Guest 可运行但不越界 | FR-FRONT-002—010、NFR-SEC-001—004 | AC-006、AC-007 | P2-A、P4 |
| 应用实例隐私快照与动态图重评 | FR-SEC-014—059、NFR-SEC-007—014 | AC-024—050 | P0、P1-A、P2、P3-A、P4 |
