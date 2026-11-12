# 05. Program Package 与 HTTPS 分发

## 1. 三层身份

分发设计必须把三件事分开：

| 层 | 标识 | 证明什么 |
|---|---|---|
| Publisher | 规范化域名 | 当前谁控制发布入口 |
| Program Release | `publisher/program@version` 与固定的归档摘要 | 发布了哪个 Actor 包版本 |
| Service | ActorId + Actor credential | 当前在与哪个运行实例通信 |

HTTPS 不能证明程序无恶意；Marketplace 审核不能证明网络中的 Actor 身份；Actor credential 也不能证明本地加载的 Guest 是正确版本。

## 2. Publisher 身份

### 2.1 域名即 Publisher ID

Publisher 约定使用域名是合理的，因为它直接复用：

- DNS 全局唯一性；
- 域名所有权管理；
- CA/TLS 证书；
- HTTPS；
- 证书透明度；
- CDN、缓存、代理和企业网络设施；
- 组织已有的安全响应能力。

Actrium 不维护另一套全局 Publisher ID 注册中心。

### 2.2 Publisher Profile

Publisher Profile 作为 authority descriptor 的可选成员 `publisher` 发布在该域名唯一的 bootstrap 资源中：

```text
GET https://<publisher>/.well-known/actr
```

`publisher` 成员已在 Actr Authority Discovery §4.5 定义；按 authority-discovery 的规定，`/.well-known/actr` 之下不定义子路径，因此 Publisher 不另设 well-known 文件。同一份 descriptor 还可以承载 `realm_signing_keys`（若该域名下也有 Realm）与 `services`。

bootstrap URL 由 Publisher 身份域名直接导出，必须使用标准 HTTPS 443。

获取 descriptor 时遵循 authority-discovery 的 redirect 规则：最多 5 次、每跳必须 HTTPS，redirect 目标不成为新的 authority，仍须用原 Publisher 域名验证 `authority` 绑定。`publisher` 成员的有效性只取决于其自身签名与固定的 root/key history，不取决于从哪个 origin 取得。

`publisher` 成员字段使用 snake_case，至少包含：

- `schema`：`actr.publisher/v1`；
- `domain`：规范化 Publisher 域名，必须与 descriptor `authority` 的主机名一致；
- 当前和历史 Publisher Profile 签名 key（`profile_signing_keys`）；
- 当前和历史 package signing keys；
- key id、算法、有效期与撤销状态；
- 安全联系地址；
- 可选 Marketplace/透明日志声明。

第一次获取时，信任来自 URL hostname 的 Web PKI，或来自企业预置的 Publisher root pin；Publisher Profile 内携带的 root 不能循环地自证可信。接受首个 Publisher Profile 后，Actrium 固定 root/key history，后续 Publisher Profile 必须：

- 按 RFC 8785 JCS 规范化签名 payload（`publisher` 成员去掉其 `signature` 字段），再验证 detached Ed25519 签名；
- 携带单调递增的 `sequence`、签发时间和到期时间；
- 通过旧 root 签名的轮换链引入新 root；
- 拒绝回滚、过期和未授权换根。

descriptor 中 `publisher` 以外的部分不签名，不参与 Publisher 信任判断。

offline root 是强烈推荐的 Publisher 运维方式，但是否要求硬件/离线保存可由 Marketplace 或企业策略提高；wire protocol 必须支持它。

### 2.3 域名过户

域名控制只决定未来发布入口，不得静默改写历史：

- 已安装版本可凭已接受的包选择和已验证的包继续离线运行。
- 新 key 与历史 key 无可验证轮换链时，视为 Publisher ownership change；
- 已安装版本不得因当前域名返回不同包而变化；
- 新安装或更新必须显示身份变化；
- 高风险策略可以要求 Marketplace/组织管理员再次批准。

## 3. 包从哪来

Publisher 分别签署并分发各包。创建 Realm 时，从 Program Catalog、Marketplace、Publisher 或本地文件选择 Service 和 Front，将精确类型、下载地址和摘要写入 `packages`，由 Realm 签署 Profile。本机 Realm 在设备上保存选择。打开联网实例时使用其[签名 Realm Profile](../../../actr/protocols/realm-profile.zh.md#6-使用-realm)。

## 4. Service 包与 Front 包

### 4.1 包的边界

每个 `actr-package/2` 包声明一个 Actor 类型和一个 Workload。Host 核对各包的 Workload Profile 是否符合所选角色和分发策略。见 [Actor Package](../../../actr/protocols/actor-package.zh.md)。

- **Service 包。** `[workload]` 选择 Service 可执行文件。Realm Profile 解析的主包 `packages[primary_package]` 指向该归档。
  Marketplace Service 使用 `actr.component/1`。
- **Front 包。** `[workload]` 以 `keel.guest/1` 选择 Keel Guest。
  `[workload.keel]` 声明其运行时、能力清单和 Keel 契约。
  Product v1 的 Front 清单保留 `service` 依赖别名，并必须声明 `required = true`。
  `actor` 和 `version` 约束必须与 Realm 选定的 Service 匹配。

每个包的入口使用角色 `workload`。Guest 能力清单使用 `profile`，Keel 和 Proto 契约使用 `contract`，
图标和静态资源使用 `resource`。

### 4.2 推荐目录

```text
team-notes.actr
├── manifest.toml
├── manifest.sig
├── workload/
│   └── service.wasm
└── contracts/
    └── service.proto

team-notes-front.actr
├── manifest.toml
├── manifest.sig
├── workload/
│   ├── guest.js
│   └── capabilities.json
└── contracts/
    ├── notes.json
    └── service.proto
```

每个包的 `manifest.sig` 保存 Publisher 对清单的签名，清单列出本包各载荷摘要。

### 4.3 包内不允许什么

第三方正式包不得包含：

- Front Host 二进制；
- Keel Host 替代实现；
- 自定义 Host Module 实现；
- React Native 原生插件；
- Front 动态库；
- 未在 manifest 中列出的可执行脚本；
- 运行时从网络替换的 Guest 入口。

Actrium 客户端提供 Front Host。Publisher 在 Front 包中提供 Keel Guest Workload 及其声明。

## 5. Service workload 政策

### 5.1 Marketplace 默认等级

v1 Marketplace 第三方 Service workload 必须是：

- WebAssembly Component Model；
- 目标 ABI 在 Actrium 支持列表内；
- 通过 import/export allowlist；
- 带 fuel、wall-clock timeout、memory、table、stream 和 I/O 配额；
- 所有 Workspace、网络和 Actor 能力经 Service Host broker。

### 5.2 Native Service

Actr 已具备 native cdylib/linked workload 能力，但这不等于 Marketplace 应默认允许：

- native workload 与 Actrium 进程具有更高风险；
- iOS 等平台禁止随意下载执行本机代码；
- 动态库存在 ABI、签名和供应链差异。

因此 Native Service 只适用于：

- Actrium 第一方系统程序；
- 企业受管分发；
- 桌面开发模式；
- 用户明确接受的 elevated trust tier。

Native Service 不能改变 Front 仍只允许 Keel Guest 的规则。

Native workload 必须进一步区分：

- `NativeCdylib`：加载进 Actr Host 地址空间；不能证明出口闭合，第三方实例不得为绿；
- `NativeCapsule`：独立受限进程，只有在 OS sandbox、无原始网络/文件、sealed libraries 和 brokered IPC 全部生效时，才可能评为 `CONTROLLED`；
- `NativeTrusted`：Actrium 自带或企业预置 TCB，不属于开放 Marketplace trust tier。

v1 Marketplace 仍只开放 WASM Service。Native Capsule 是桌面优先的后续能力，具体约束见[隐私标记与汇聚协议](13-privacy-state-and-aggregation.md)。

### 5.3 UI-only 程序

v1 不引入「没有 Service 的页面应用」特例。工具链为 UI-only 程序生成一个平台定义的最小 WASM Service Component：

- 它仍有与 Program Release 相同的 ActrType 和独立 ActorId；
- 默认没有 Workspace 和外部网络能力；
- 只实现 session、health 和可选的最小 key-value contract；
- 同样经过包签名、资源限制和 Service 生命周期；
- Publisher 不能用它绕过 Host API 或注入 native code。

因此所有 app Tab 仍统一 Attach 一个 Service，差别只在它是否包含自定义逻辑。

## 6. 包清单字段

Actr 基础清单提供以下表。

- `[actor]` 声明 Publisher、程序名和发布版本。
- `[workload]` 声明各自的 profile 和入口，`[host]` 声明 Host API 版本范围和能力要求。
- `[dependencies.<alias>]` 声明 Actor 依赖的类型、版本约束和契约路径。
- `[signature]` 声明签名算法和 `key-id`。
- `[[files]]` 记录包内文件的角色、大小和 SHA-256，也包含随包分发的依赖锁。
- Service manifest 的 `[workspace]` 声明静态的 `requirement`、`access` 和 `providers`。

对 Product v1 的配套包，`dependencies.service.contract` 以包内相对路径选定 RPC Proto。
两个 Host 在配套包验证时，从已验证的 Front manifest 读取此路径。
Service Host 从 `front_package` 对应的包取得 Front 清单，保存验证后的路径用于 Attach，
再从自身已验证的 Service 归档读取选定的 Proto。
两个清单必须在该路径登记角色为 `contract` 且 SHA-256 相同的文件。包中包含其他契约文件时，
也按此路径选择 RPC 契约。Actr Host 按[第 06 章 §2.2](06-runtime-and-protocol.zh.md#22-front-actor-类型)计算 Attach 指纹。

Service 节点使用现有 Actr `actr.toml` 格式配置 Realm、信令和信任等部署参数。字段见
[节点配置定义](../../../crates/actr-config/src/actr_raw.rs)。Workspace 要求来自签名 Service manifest 的
`[workspace]`。部署或创建流程选择 `provisioning`，Host 按实例保存绑定和实际授权。

两份完整清单见[参考协议](12-reference-spec.zh.md#4-actr-manifest)。

## 7. 能力声明与运行时策略

包声明的是「申请」，不是实际授权：

```text
effective capabilities =
  package requested
  ∩ Actrium Host supported
  ∩ user/admin granted
  ∩ current platform allowed
  ∩ current session scope
```

Actr Host 按产品策略评估以下内容。包的能力要求来自 `[host]`，Guest 的声明来自其签名保护的 Keel 能力清单，
Workspace 要求来自签名 Service manifest，实例设置和实际授权来自部署及授权流程。
Guest 堆、执行时间、render op 上限，以及 Service 内存、fuel、存储和网络配额，
来自平台或部署策略，由 Host 执行。最低 Actrium 客户端版本和受支持的 SDK/工具链版本
属于平台发行策略。包的兼容性通过 `host.api`、`workload.profile`、
`workload.keel.guest-api` 和已验证的 RPC 契约指纹检查。

- Front capability；
- Service capability；
- Keel 组件集合；
- outbound network domain；
- camera/microphone/clipboard/notification/location；
- Workspace access mode；
- Guest heap、单次执行时间、render op；
- Service memory/fuel/storage/network；
- 平台支持的工具链下的 Host API 与 Workload/Guest 契约兼容性。
- 数据分类、预期 Actor 出口、terminal egress、required/optional 和最低节点结论。

新增高风险权限必须发布新版本，并在升级时重新确认。

Privacy Declaration 是静态 allowlist 和安装说明，不是运行时隐私图的权威来源。Actr Host 必须把 WASM imports、实际 capability 与实际 edge 对照声明；Publisher 把依赖标记为 optional 也不能覆盖 Actr Host 根据真实数据流做出的闭包判断。

## 8. 下载与验证流水线

```text
Realm Profile 包选择 / 本机选包
  → 验证 Service 包
  → 验证 Front 包及其 Service 依赖
  → policy + permission review
  → content-addressed cache
  → Service Runtime / verified Guest bytes
```

### 8.1 检查顺序

先验 Realm Profile，再逐包执行以下检查。精确类型和整包 SHA-256 须与对应条目一致。Front 的 `dependencies.service` 须匹配已验证的 Service。

1. 校验 Publisher DNS/IDNA；
2. TLS 和 hostname；
3. 下载大小上限；
4. package SHA-256；
5. ZIP 路径、STORE 方法、文件数和大小上限。
6. `actr-package/2` 格式和 `signature.key-id`。
7. 清单声明的 Publisher 对应的 Publisher Profile。
8. Ed25519 签名。
9. 签名清单中的 Actor 身份和入口，以及 Workload Profile 是否符合 Realm 选定的包角色和第 4.1 节的分发策略。
10. ZIP 文件清单、大小和 SHA-256 与 `[[files]]` 精确匹配。
11. 对 Front 包，检查 Guest 静态依赖和能力清单。
12. 对 WASM Service 包，检查 WASM imports。
13. Host API、Keel API，以及 Front 所需契约与 Service 包的兼容性；
14. 本地/组织安全策略。

使用 Privacy Contract 授权前，按 Service 的签名清单验明契约，再对所选包组合评估。见 [14 §4.2](14-privacy-primitives-and-attestation.zh.md#42-program-privacy-contract)。

任何一步失败都不得把部分内容交给 Keel 或 Service runtime。

### 8.2 Keel 加载边界

Keel `Engine.loadBundle(URL)` 只是加载机制，不是安装信任机制。Actrium 必须：

1. 独立验证 `front_package` 指向的 Front 包。
2. 从 content-addressed cache 读取已验证 Guest bytes；
3. 把本地 bytes/code 交给 Keel；
4. 禁止 Guest 在启动时指定替代 URL。

## 9. 版本不可变

首次接受每个精确 `publisher/program@version` 时固定整包摘要。同一类型出现不同摘要即报 `PACKAGE_EQUIVOCATION`。包内容变化须发布新版本。不同 Realm 可为同一 Service 选用不同的兼容 Front。

按摘要缓存已验证的归档，直到引用它的实例全部释放。重新下载时再次核对摘要。

本地 first-seen pin 只能保护当前安装；Marketplace 可以帮助多个客户端比对已见 digest。要对所有客户端提供全局 non-equivocation，需要后续引入公开 append-only transparency log。v1 不把单机 pin 夸大成全局透明性。

## 10. 签名与 HTTPS 的分工

### HTTPS 提供

- 当前域名控制证明；
- 传输机密性与完整性；
- Publisher Profile 的来源；
- 标准代理/CDN/缓存能力。

### Package 签名与 digest 提供

- CDN、镜像和落盘后的完整性；
- 离线验证；
- key rotation history；
- 精确版本不可变；
- 域名暂时不可用时继续运行；
- 检测同名异包。

两者必须同时使用，不以一方替代另一方。

## 11. Marketplace

Marketplace 可以：

- 搜索、分类和推荐；
- 记录 Publisher 验证；
- 静态/动态审核；
- 风险评级；
- 记录每个精确 Actor 包类型已见的摘要。
- 发布撤销和恶意软件情报；
- 提供缓存或镜像。

Marketplace 不可以：

- 为第三方抢注其域名；
- 静默替换 Publisher package；
- 用市场登录身份替代 Publisher 域名；
- 因「已审核」绕过 Actrium 权限；
- 把下架与 Service/Workspace 自动删除等同。

## 12. 撤销政策

撤销分级：

| 等级 | 行为 |
|---|---|
| Advisory | 显示风险，允许继续 |
| Block new | 阻止新安装/新建，已有实例提示 |
| Quarantine | 默认停止并隔离已有实例，用户/管理员可审计处理 |
| Critical block | 阻止加载 Guest/Service，保留数据恢复入口 |

撤销记录必须包含：

- Program Release 及受影响的包摘要或签名 key。
- 原因；
- 发布时间；
- 发布者或市场签名；
- 建议操作；
- 是否影响仅 Guest、Service 或签名 key。

撤销不自动删除 Workspace。

## 13. 更新与升级

Program Catalog 可以提示新版本，但 Service 不自动原地升级。

v1 升级：

1. 安装新 Program Release；
2. 在同一 Realm 内创建新 Service，获得新 ActorId；
3. 旧 Service 生成迁移快照；
4. 显式交接 Workspace lease；
5. 新 Service 导入并验证；
6. 用户确认切换，随后以 `publish_profile` 发布主包引用指向新 Service 归档的 Realm Profile；
7. 旧 Service 保留回滚窗口后归档；回滚即再发布一份指回旧包、`sequence` 更高的 Realm Profile。

这样避免 ActorId 中版本与实际 workload 不一致，也让失败回滚可解释。

## 14. 本地开发与自建

支持：

- 配套的本地 Service 和 Front `.actr` 文件。
- localhost、回环地址或显式信任的私有 CA 上的两个包地址。
- 本地开发 signing key；
- 热更新 Guest；
- 未发布 Program Catalog entry。

限制：

- 修改归档会产生新的开发包摘要。
- 未验证 key 不继承同域名正式 Publisher 权限；
- 本地包不进入正式 Marketplace；
- 私网、loopback、link-local URL 只在开发模式或企业策略中允许；
- 开发 package 不可用于绕过 Native Service 限制。

## 15. 缓存与离线

缓存按 package digest 内容寻址：

```text
cache/packages/sha256/<digest>/package.actr
cache/extracted/sha256/<digest>/...
```

要求：

- 原始 package 保留，用于重新验证；
- extracted 文件只从已验证 package 产生；
- 原子写入；
- 未完成下载隔离；
- 缓存元数据记录 Publisher key 与验证时间；
- 保存已接受的 Realm Profile 及其固定的归档，供离线重新验证。
- Publisher 离线时，本地实例按离线策略使用固定的 Service 和 Front 包运行。
- Realm 凭据/撤销过期时按离线安全政策处理，不等同于 package 验证失败。
