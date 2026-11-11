# Actr Actor Package 规范

状态：初始草案。

本文仅提供简体中文版。后续若形成英文规范，应保持两者的协议要求一致。

本文定义 `.actr` Actor Package 的逻辑模型、ZIP 容器、manifest、Workload Profile、
Actr Host 要求、Actor 依赖、完整性验证和装载流程。

`.actr` 发布的是一个 Actor 类型的一次可安装发布，而不是某个网络角色的安装包，也不是
只装有一段可执行文件的通用压缩包。包内 Workload 描述 Actor 的业务计算，Actr Host 声明该
Workload 所需的可信运行环境；运行时结合包、部署配置和新分配的实例身份创建 Actor。

本文中的大写关键词 **MUST**、**MUST NOT**、**SHOULD**、**SHOULD NOT** 和 **MAY**
按 BCP 14 解释。

## 1. 目标与范围

本规范有以下目标：

- 所有可分发 Actor 使用同一种 `.actr` 包；
- 一个包精确发布一个 Actor 类型和一个主 Workload；
- Workload Profile 只表达装载和执行方式，不表达 Actor 的网络角色；
- Actr Host 依赖与远端 Actor 依赖分开声明；
- Actrium 可以直接从 `.actr` 中识别并装载 Keel Guest Workload；
- 一个根 `.actr` 可以通过签名锁文件引用其他 `.actr`，无需额外的应用归档层；
- 包内所有可执行内容、contract 和资源都受到同一条完整性与签名链保护。

本文不定义：

- `.actr` 的仓库、索引、下载和更新协议；
- `actr:` URI 如何解析到 package release；
- authority 如何公布 package repository；
- Actor instance 的 serial number、realm、credential 或部署配置；
- 各 Workload Profile 的完整 ABI；
- Keel capability manifest 的完整字段。

这些内容由 Actr runtime、Actrium 和 Keel 的下级规范分别定义。

## 2. 术语

### 2.1 Actor Package

以 `.actr` 为扩展名、符合本文的签名 ZIP 文件。一个 Actor Package 表示一个 Actor 类型的
一个不可变 release。

### 2.2 Actor 类型

由 `publisher`、`name` 和 `version` 确定的可执行 Actor 定义。Actor 类型
不是运行时实例身份，不包含 realm 或 serial number，也不进入 ActorId；它由 Actor 注册与
Actor credential 携带。

### 2.3 Actor 实例

Actr Host 使用 Actor Package、部署配置和运行时身份实例化出来的节点。不同实例可以来自同一
Package，但必须具有各自的 Actor identity 和状态。

### 2.4 Workload

Actor 内的业务计算。Workload 负责 Actor message、生命周期事件和允许的 Actr Host 调用。
Workload 的具体文件形态与 ABI 由 Workload Profile 定义。

### 2.5 Workload Profile

一个带版本的执行约定，规定 Actr Host 如何解释 Workload entry、如何建立 ABI、如何映射
message 和生命周期，以及允许出现哪些 profile-specific manifest 字段。

### 2.6 Actr Host

创建 Actor 实例、提供 mailbox、transport、storage、clock、UI 或其他受控能力，并装载
Workload 的可信软件。Actr Host 是运行环境，不是远端 Actor。

### 2.7 Actor 依赖

当前 Actor 在运行时需要发现或联系的另一个 Actor 类型。Actor 依赖只表达类型、contract
和版本约束，不表达双方的上下级或网络角色。

### 2.8 根 Package

一次安装操作最先取得的 `.actr`。`root` 只是安装入口，不是特殊 Actor 类型；其依赖仍是
地位对等的 Actor Package。

## 3. 核心模型

一个 `.actr` 的语义结构为：

```text
Actor Package
├── Package format 与 Actor 类型
├── 一个主 Workload
├── Host 要求
├── 导入、导出与 Actor 依赖
├── 可选依赖锁
├── Contract 与资源
└── 文件摘要与发布者签名
```

Actr Host 完成实例化后，运行关系为：

```text
Actor instance
├── Actor identity 与当前 state
├── Host
├── Workload
└── deployment configuration
```

Package **MUST NOT** 声明当前 Actor 是 client、server、frontend、backend 或其他固定网络
角色。某个 Actor 可以主动联系依赖，也可以接收其他 Actor 的消息；这些行为由 contract、
授权和 Workload 决定。

## 4. 一包一个 Actor

一个 `.actr` **MUST** 只声明一个 Actor 类型和一个主 Workload。

需要多个可独立寻址、独立升级或独立授权的 Actor 时，发布者 **MUST** 为它们分别生成
`.actr`，再通过依赖和锁文件组成有限 package graph。实现不得在一个 Package 中用多个
主 Workload 模拟多个 Actor，因为那会混淆：

- Actor identity；
- version 与升级边界；
- lifecycle；
- capability 与授权；
- 部署位置；
- 故障和状态边界。

同一个 Workload Profile 可以允许 Workload 自身包含多个内部 module，但这些 module
共享同一个 Actor identity，并且不能作为独立 Actor 被寻址。

## 5. ZIP 容器

### 5.1 文件扩展名与编码

Actor Package 文件名 **MUST** 使用 `.actr` 扩展名。文件名只用于存储和展示，不能作为
Actor 类型、版本或信任判断的依据。

版本 2 使用 ZIP 容器。所有 entry **MUST** 使用 ZIP `STORE` method，**MUST NOT** 使用
ZIP encryption。文本文件默认使用 UTF-8；具体 payload 可以使用 Workload Profile 规定的
二进制格式。

### 5.2 保留文件

Package 根目录保留：

| 路径 | 要求 | 用途 |
| --- | --- | --- |
| `manifest.toml` | 必需 | Actor、Workload、Actr Host、文件和签名元数据。 |
| `manifest.sig` | 必需 | 对原始 `manifest.toml` bytes 的发布者签名。 |
| `manifest.lock.toml` | 可选 | 精确依赖解析结果。存在时必须由 manifest 绑定。 |

推荐目录为：

```text
package.actr
├── manifest.toml
├── manifest.sig
├── manifest.lock.toml
├── workload/
├── contracts/
└── assets/
```

除三个保留文件外，所有普通文件都 **MUST** 出现在 manifest 的 `files` 列表中。Package
**MUST NOT** 包含未声明的可执行文件、动态入口或运行时下载后替换的 Workload entry。

### 5.3 路径约束

ZIP entry path：

- **MUST** 是 UTF-8 相对路径；
- **MUST** 使用 `/` 分隔；
- **MUST NOT** 以 `/` 开头；
- **MUST NOT** 包含空 segment、`.`、`..`、反斜杠或 NUL；
- **MUST NOT** 重复；
- **MUST NOT** 表示 symlink、hard link、device 或其他特殊文件。

验证器必须在分配大块内存或解包到磁盘前限制 manifest 大小、entry 数量、单 entry 大小和
总 payload 大小。具体默认上限属于实现策略，但实现必须默认拒绝超限 Package。

## 6. `manifest.toml`

### 6.1 顶层结构

版本 2 manifest 至少包含：

```toml
format = "actr-package/2"

[actor]
publisher = "acme.com"
name = "notes"
version = "1.2.0"

[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[host]
profile = "actr.host/1"
api = "^1.0.0"

[signature]
algorithm = "ed25519"
key-id = "release-2026"
```

`format` **MUST** 精确标识 Actor Package format major version。读取器遇到不支持的 major
version 时 **MUST** 拒绝，不能猜测其字段含义。`actr-package/1` 已停用：读取器 **MUST**
拒绝 `format = "actr-package/1"` 或 `[actor]` 缺少 `publisher` 的 manifest，并返回明确指出
需改用 `actr-package/2` 与 `publisher` 字段的错误；旧字段名不作为别名接受（历史字段名见
[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」）。

Actrium Product v1 的 Service 和 Keel Guest 分别使用独立的 `actr-package/2` 包，各自声明一个
Workload。清单和依赖示例见
[Product v1 参考清单](../../docs/product/actrium-v1/12-reference-spec.zh.md#4-actr-manifest)。

### 6.2 `[actor]`

`[actor]` 必须包含：

| 字段 | 含义 |
| --- | --- |
| `publisher` | 发布及命名 Actor 类型的 publisher 标识（规范化 DNS 域名）。 |
| `name` | 该 publisher 下的 Actor 类型名称。 |
| `version` | 此 release 的语义版本。 |

三者共同形成 Actor 类型，唯一的规范文本形式为（`program` 即 `name` 字段）：

```text
publisher/program@version
```

`[actor]` **MUST NOT** 包含 instance serial、realm、authority endpoint、signaling address、
STUN/TURN credential、用户 credential 或安装设备信息。

### 6.3 `[workload]`

`[workload]` 必须包含：

| 字段 | 含义 |
| --- | --- |
| `profile` | 带 major version 的 Workload Profile 标识。 |
| `entry` | Package 内的主入口；仅当 profile 明确定义无文件入口时可以省略。 |

Profile 可以定义额外的 `[workload.<profile-section>]` 字段，但不能改变基础字段含义。
Workload entry 必须由 `files` 列表绑定，并且其 role 必须为 `workload`。

加载器不支持声明的 Workload Profile 时 **MUST** 返回明确的 unsupported-profile 错误，
不能将其当作普通资源运行。

### 6.4 `[host]`

`[host]` 声明装载 Workload 所需的 Actr Host contract，而不是把 Actr Host 实现作为远端 Actor 依赖。
它必须包含：

| 字段 | 含义 |
| --- | --- |
| `profile` | Actr Host 所实现的带版本运行 Profile。 |
| `api` | Package 可接受的 Actr Host API 版本范围。 |

它还可以包含：

```toml
required-capabilities = [
  "actor.call",
  "actor.tell",
  "ui.render",
]
optional-capabilities = [
  "actor.storage",
]
```

Actr Host **MUST** 在执行 Workload 前验证：

1. 支持 `profile`；
2. Actr Host API version 落在 `api` 范围内；
3. 所有 required capability 都可用并获策略允许；
4. Workload Profile 与 Actr Host Profile 的组合有效。

任意 required 条件不满足时，Actr Host **MUST** 拒绝实例化。Optional capability 不可用时，
Actr Host 可以继续实例化，但必须通过 Profile 定义的 feature detection 机制向 Workload 报告。

Actr Host 实现通常由平台内置或在应用构建时链接。版本 2 不定义 Package 内 Actr Host executable，
`[host]` 也 **MUST NOT** 包含指向包内 Actr Host implementation 的 `entry`。这是因为 Actr Host 属于
可信计算边界，普通 Package 不能通过自带 Actr Host 绕过 capability、签名和沙箱策略。

### 6.5 Actor 依赖

Actor 依赖按 alias 声明。示例：

```toml
[dependencies.sync]
actor = "acme.com/notes-sync"
version = "^2.0.0"
contract = "contracts/sync.proto"
required = true

[dependencies.index]
actor = "acme.com/notes-index"
version = "^1.4.0"
required = false
```

Alias 是当前 Workload 内的稳定引用名。`actor` 使用 `publisher/program`，版本约束由
`version` 单独声明。`contract` 可绑定调用方编译时使用的 interface。Profile 或下级规范可以
增加 contract fingerprint。

依赖：

- **MUST NOT** 表达 client/server 关系；
- **MUST NOT** 隐含依赖与当前 Actor 同进程、同设备或同 realm；
- **MUST NOT** 被当作 Actr Host implementation；
- **MAY** 在运行时解析到零个、一个或多个合规 Actor instance，具体由 contract 和发现
  规范决定。

Required dependency 无法解析时，Actr Host 必须按相关 lifecycle 规范使 Actor 保持未就绪或
返回明确错误，不能静默绑定到不满足类型、版本或 contract 的实例。

### 6.6 依赖锁

存在 `manifest.lock.toml` 时，manifest 必须把它列为 role `lock` 的文件并绑定 size 与
SHA-256。锁文件把 dependency alias 固定到精确的：

- Actor type version；
- Package publisher；
- Package digest；
- contract fingerprint；
- 可选 repository locator。

版本范围表达发布者意图；锁文件表达一次可复现安装的解析结果。运行时 **MUST NOT** 用
未锁定的同名 package 静默替代已锁定 digest。

Actor dependency graph 可以包含运行时环，但 package resolver 必须以已访问的精确 package
identity 去重、限制图规模，并拒绝同一 dependency slot 的冲突锁定。解析算法不得假定依赖
关系就是启动顺序。

### 6.7 文件表

每个受保护文件使用一个 `[[files]]` 条目：

```toml
[[files]]
path = "workload/guest.js"
role = "workload"
media-type = "text/javascript"
size = 18432
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

版本 2 定义以下基础 role：

| Role | 含义 |
| --- | --- |
| `workload` | Workload 主入口或 Profile 明确声明的执行模块。 |
| `contract` | Proto、schema、WIT 或其他 interface description。 |
| `resource` | 图标、静态内容和非执行资源。 |
| `lock` | `manifest.lock.toml`。 |
| `profile` | Workload Profile 要求但不直接作为主入口的受控文件。 |

未知 role 必须使用由扩展规范控制的 namespace。验证器必须验证每个条目的 path、size 和
SHA-256，并拒绝缺失、重复、摘要不匹配或未在 manifest 声明的文件。

### 6.8 `[signature]`

版本 2 的 `algorithm` 为 `ed25519`。`key-id` 标识发布者用于此次 release 的公钥，但它
本身不是信任锚。Actr Host 必须从受信 publisher metadata、组织策略或其他可信渠道取得公钥，
不能因为 Package 自称某个 key-id 就信任该 key。publisher metadata 以签名 Publisher Profile
的形式内嵌在 publisher 域名的 authority descriptor 中（`https://<publisher>/.well-known/actr`
的 `publisher` member，见 [Actr Authority Discovery](authority-discovery.zh.md) 第 4.5 节），
不另设独立的 Well-Known 文件。

`manifest.sig` 是对 Package 中原始 `manifest.toml` bytes 的 Ed25519 signature，使用原始
64-byte encoding。

`key-id` 为 `actr-pub-` 加该公钥原始 32 字节 SHA-256 的前 16 个小写十六进制字符。Publisher
Profile `keys` 中每把包签名 key 的 `id` **MUST** 按同一规则由其 `public_key` 算出，因此
`key-id` 与 Publisher Profile 中的 key 一一对应。Actr Host 按 `key-id` 在已接受 Publisher
Profile 的 `keys` 中找 key（`active` 或 `retired` 均可）验证 `manifest.sig`；找不到时
**SHOULD** 刷新 Publisher Profile 后再判，同一 publisher 每 60 秒至多一次。

发布者只用 `active` key 签新包。`retired` key 不再签包，它签过的包仍可验证。从 `keys` 移除
某把 key 即吊销它：它签过的包都不再被接受。`keys` 最多 8 项，其中 `active` 至多 2 项，以便
轮换时重叠：

1. **预发布。** 发布 `sequence` 更高、加入新 key（`active`）的 Publisher Profile，继续用旧 key
   签包。
2. **切换。** 至少 24 小时后改用新 key 签包，并在下一份 Publisher Profile 中把旧 key 标为
   `retired`。
3. **移除。** 只在 key 泄露，或确认它签过的包都不再需要验证时，才从 `keys` 移除。

Publisher Profile 签名 key（`profile_signing_keys`）的轮换另按
[Actr Authority Discovery](authority-discovery.zh.md) 第 4.4、4.5 节进行。

### 6.9 `[workspace]`

可选的顶层 `[workspace]` 表声明此 Workload 对用户工作空间的静态要求，由 manifest 签名覆盖。
Product v1 在 Service 包中声明这些要求。

| 字段 | 值 | 含义 |
| --- | --- | --- |
| `requirement` | `none` / `optional` / `required` | 是否申请 Workspace，以及进入 Ready 前是否必须绑定。 |
| `access` | `read-only` / `read-write` | Workload 申请的访问级别。 |
| `providers` | 非空字符串数组 | Workload 支持的 Workspace Provider 类型，例如 `local-folder`、`cloud-drive`。 |

省略此表等同于 `requirement = "none"`。声明此表时必须提供 `requirement`。
`requirement` 为 `optional` 或 `required` 时，必须提供 `access` 和至少一个 `providers` 条目。
`requirement = "none"` 时省略 `access` 和 `providers`，Workload 仍可使用实例私有存储。

Host 将 Provider 支持范围与平台能力、用户和组织策略取交集，并取得用户授权。
绑定必须提供声明的访问级别，实际授权以 `access` 为上限。`required` 的绑定要求满足后，
Workload 才能进入 Ready。`optional` 允许以未绑定 Workspace 的状态运行。

`provisioning`（`managed` 或 `user-selected`）由部署或实例创建流程选择。
实际 Provider、位置、凭据引用、授权和租约由 Host 按实例保存。
`[workspace]` 仅接受上表定义的三个字段。

## 7. Workload Profile

### 7.1 Profile 的职责

每个 Workload Profile 必须定义：

- entry 文件形态；
- Workload 与 Actr Host 的 ABI；
- Actor message 如何映射到 Workload；
- lifecycle 事件如何映射到 Workload；
- Workload 如何调用 Actr Host capability；
- profile-specific manifest 字段；
- 支持的 target 与资源限制；
- profile-specific 安全要求。

Profile **MUST NOT** 重新定义 Actor type、Package 签名、文件摘要、dependency alias 或 Actr Host
信任规则。

初始保留以下 Profile 标识：

| Profile | 用途 |
| --- | --- |
| `actr.component/1` | WebAssembly Component Model Workload。 |
| `actr.native/1` | 平台原生动态库 Workload。 |
| `actr.linked/1` | 构建时与 Actr Host 链接的 Workload。 |
| `keel.guest/1` | 由兼容 Keel Runtime 装载的 Guest Workload。 |

上述 Profile 的完整 ABI 必须由各自的下级规范定义。仅知道 Profile 名称不足以安全执行。
Profile、Profile Runtime、Execution Engine 与 Actr Host 的分层及首批实现命名见
[`workload-runtimes.zh.md`](../../docs/architecture/workload-runtimes.zh.md)。

### 7.2 `keel.guest/1`

选择 `keel.guest/1` 时，`.actr` 本身就是该 Actor 的唯一分发包。Keel Guest
不是包外附件，也不需要第二层 ZIP。

示例：

```toml
[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = ["actor.call", "actor.tell", "ui.render"]

[workload.keel]
runtime = "js"
guest-api = "keel.guest/1"
capabilities = "workload/capabilities.json"
contract = "contracts/notes.json"
```

Actrium Actr Host 必须：

1. 验证整个 `.actr` 及发布者信任；
2. 为此次实例化创建或取得 Actor identity；
3. 建立 mailbox、transport、storage 和 lifecycle；
4. 在 Keel sandbox 中装载 Guest entry；
5. 按 Keel contract 把 Actor message 映射到 Guest export；
6. 把 Guest 的 Actor 调用映射到受控 Actr Host capability；
7. 对输入、输出和 capability boundary 执行 schema 与权限检查；
8. 在 Actor 停止或失去有效身份时终止相应 Guest execution。

完成上述装载后，该 Keel Workload 属于一个正常 Actor instance，具有 ActorId、mailbox、
lifecycle 和与其他 Actor 对等的通信能力。产品层的 Tab、window、icon 和展示名称不改变
其 Actor 语义。

Keel capability manifest 必须作为 role `profile` 文件受外层 manifest 摘要和签名保护。
它不能自行替代 Package 签名，也不需要重复定义 Package name、version 或 publisher。

`keel.guest/1` 的完整 Guest ABI、capability schema 和 UI lifecycle 属于 Keel 下级规范；
Actrium 的导航和多窗口行为属于产品扩展规范。基础 Actr parser 只负责验证 Package；不支持该 Profile 的 Actr Host
必须报告 unsupported-profile。

## 8. Actr Host 模型

Actr Host 在任何 Actor 实例中都存在；可选的是 Actr Host 的具体提供方式，而不是 Actr Host 本身。

Actr Host implementation 可以：

- 内置于 Actrium；
- 内置于服务器 runtime；
- 与移动或桌面应用链接；
- 由受管平台预先安装。

Package 只声明 Actr Host contract 和 capability 要求，不以普通 Actor dependency 下载 Actr Host。
这样同一 Actor Package 可以由多个兼容实现承载，而无需绑定到具体产品 build。

平台可以基于来源、target、Workload Profile 和 capability 采用更严格策略。例如：

- Actrium 可以允许 `keel.guest/1`，但拒绝第三方 native Workload；
- iOS 可以只接受 linked 或平台允许的 Wasm Profile；
- 企业环境可以要求额外 publisher、审计或签名策略；
- Actr Host 可以拒绝格式有效但 capability 过宽的 Package。

格式有效不代表平台必须执行。

## 9. 安装与装载流程

合规 installer/Actr Host 至少按以下顺序处理：

1. 以受限 ZIP reader 打开 Package；
2. 验证 entry path、数量、method 和 size limit；
3. 读取原始 `manifest.toml` 和 `manifest.sig`；
4. 解析 format major version 与签名 metadata；
5. 根据外部信任信息解析 publisher key；
6. 验证 manifest signature；
7. 验证所有声明文件的 size 与 SHA-256；
8. 拒绝未声明或重复的 payload entry；
9. 验证 Actor type、Workload Profile 和 Actr Host 要求；
10. 解析并验证依赖锁；
11. 获取和验证尚未安装的依赖 `.actr`；
12. 应用平台安装及 capability policy；
13. 创建 Actor identity 与部署配置；
14. 实例化 Actr Host 并装载 Workload；
15. 仅在 required dependency 和 lifecycle 条件满足后进入 Ready。

实现 **MUST NOT** 在签名、摘要、Actr Host 要求和平台策略验证完成前执行 Workload、解析动态
脚本入口或加载 native library。

## 10. 分发与依赖组合

用户打开 Realm 链接时，Actr Host 按该 Realm 的 Realm Profile 中 `package` 给出的地址取得根
`.actr`，核对 SHA-256 后按本规范验证（[Actr Realm Profile](realm-profile.zh.md) 第 6 节）。

用户可以只取得一个根 `.actr`。Installer 根据其 lock file 递归取得所需 Actor Package：

```text
notes.actr                 keel.guest/1
├── dependency: sync  ──> notes-sync.actr
└── dependency: index ──> notes-index.actr
```

这里的箭头表示发布与运行依赖，不表示请求方向。任意 Actor 都可以按自己的 export、import、
capability 和 ACL 与其他 Actor 通信。

Package repository 可以单独传输每个 `.actr`，也可以在缓存或下载层批量获取，但不能因此
改变每个 Package 的独立 identity、digest、签名和升级边界。

## 11. 运行时配置边界

以下内容属于部署或会话状态，**MUST NOT** 固化在可复用 Actor Package 中：

- Actor serial number 和最终 ActorId；
- realm assignment；
- signaling、identity、discovery、STUN 或 TURN endpoint；
- TURN username/password；
- access token、refresh token、private key；
- 用户授权决定；
- 本地数据目录和设备绝对路径；
- 当前 authority 解析结果。

Package 可以声明需要哪些协议、Actr Host API、capability 和 Actor dependency；具体 endpoint 与
credential 由 `actr:` URI、authority discovery、部署配置和授权流程提供。

## 12. 完整性、签名与 Package digest

版本 2 的完整性链为：

```text
payload bytes
  └── SHA-256 + size ──> manifest.toml
                              └── Ed25519 ──> manifest.sig
```

签名保证 manifest 的发布者真实性；manifest 中的摘要绑定每个 payload。ZIP header、entry
顺序和其他未签名 metadata 不能改变 Actor、Workload、Actr Host 或资源语义。

Repository 和 lock file 可以使用整个 `.actr` bytes 的 SHA-256 作为 Package digest。该
digest 用于精确下载和缓存；它不能替代 manifest signature 和 publisher trust validation。

为提高可重现性，packager **SHOULD** 固定 entry 顺序、timestamp、permission bits 和其他
ZIP metadata。后续规范可以定义 canonical archive encoding，但版本 2 的语义验证不依赖
ZIP entry 顺序。

## 13. 版本与扩展

版本演进分为三层：

| 层 | 示例 | 不兼容时的处理 |
| --- | --- | --- |
| Package format | `actr-package/2` | 不支持 major version 时拒绝。 |
| Workload Profile | `keel.guest/1` | 不支持 Profile major version 时拒绝。 |
| Actr Host API | `^1.0.0` | Actr Host version 不在范围内时拒绝。 |

实现可以忽略 manifest 中明确标记为 optional 的未知 metadata，但不得忽略：

- 未知 Package format major；
- 未知 Workload Profile；
- required Actr Host capability；
- required dependency；
- 未知签名或摘要算法；
- 影响执行入口、权限、contract 或验证语义的未知字段。

扩展规范必须使用不会与 Actr core 字段冲突的 namespace，并明确其字段是 required 还是
optional。

## 14. 安全与隐私考虑

- Actr Host 必须把 Package、manifest、lock 和所有 payload 视为不可信输入，直至验证完成。
- ZIP reader 必须防止 path traversal、重复 entry、特殊文件、资源耗尽和解包覆盖。
- Workload entry、contract、capability manifest 和可执行资源必须全部被签名 manifest
  间接绑定。
- 发布者签名只证明来源和未被修改，不自动证明代码安全；平台仍需执行 sandbox、capability
  和 publisher policy。
- Package 不能自带普通第三方 Actr Host 替代实现，否则会破坏 Actr Host/Workload 信任边界。
- Native Workload 具有更高权限，平台应默认采用更严格的来源和安装策略。
- Package 不得包含长期 credential 或用户专属授权；Package 很可能被公开缓存和复制。
- Actr Host 必须对 Guest/Workload 输入输出执行 Profile 规定的 schema、size 和 resource limit。
- 依赖解析必须验证精确 identity、version、digest 和 contract，不能只按显示名称绑定。

## 15. 完整 Actrium 示例

```toml
format = "actr-package/2"

[actor]
publisher = "acme.com"
name = "notes"
version = "1.2.0"

[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[workload.keel]
runtime = "js"
guest-api = "keel.guest/1"
capabilities = "workload/capabilities.json"
contract = "contracts/notes.json"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = [
  "actor.call",
  "actor.tell",
  "ui.render",
]
optional-capabilities = ["actor.storage"]

[dependencies.sync]
actor = "acme.com/notes-sync"
version = "^2.0.0"
contract = "contracts/sync.proto"
required = true

[extensions.actrium]
title = "Notes"
icon = "assets/icon.png"

[signature]
algorithm = "ed25519"
key-id = "release-2026"

[[files]]
path = "workload/guest.js"
role = "workload"
media-type = "text/javascript"
size = 18432
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[[files]]
path = "workload/capabilities.json"
role = "profile"
media-type = "application/json"
size = 746
sha256 = "1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[[files]]
path = "contracts/notes.json"
role = "contract"
media-type = "application/json"
size = 1204
sha256 = "2123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[[files]]
path = "contracts/sync.proto"
role = "contract"
media-type = "text/plain"
size = 912
sha256 = "3123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[[files]]
path = "assets/icon.png"
role = "resource"
media-type = "image/png"
size = 4096
sha256 = "4123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[[files]]
path = "manifest.lock.toml"
role = "lock"
media-type = "application/toml"
size = 680
sha256 = "5123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

安装此 Package 时，Actrium 识别 `keel.guest/1`，验证并解析依赖，再把 Keel Guest 作为
`acme.com/notes@1.2.0` 的 Workload 装载。该 Actor 与 `notes-sync` 等依赖 Actor 通过正常 Actr
contract 通信。

## 16. 合规性

### 16.1 合规 Packager

合规 Packager 必须：

- 生成符合本文的 ZIP STORE archive；
- 只声明一个 Actor 与一个主 Workload；
- 为所有 payload 生成 size 和 SHA-256；
- 生成可验证的 manifest signature；
- 验证依赖锁与 manifest 声明一致；
- 拒绝非法路径、重复 entry 和未声明 payload。

### 16.2 合规通用 Verifier

合规 Verifier 必须验证容器、manifest、签名、文件和基础字段。Verifier 可以在不支持具体
Workload Profile 时完成静态真实性验证，但必须把可执行状态报告为 unsupported，不能报告
为 runnable。

### 16.3 合规 Actr Host

合规 Actr Host 必须在通用验证基础上验证 Workload Profile、Actr Host API、capability、contract、
依赖和平台策略，并按 Profile 建立 Actor identity、mailbox 与 lifecycle。

## 17. 后续规范

本草案需要下列独立文档继续收敛：

- Actor Package TOML schema 与严格字段类型；
- `manifest.lock.toml` v1；
- `actr.component/1` Workload Profile；
- `actr.native/1` Workload Profile；
- `actr.linked/1` Workload Profile；
- `keel.guest/1` Workload Profile；
- `actr.host/1` Actr Host API 与 lifecycle；
- Actrium product metadata extension；
- conformance fixtures、恶意 ZIP 样例和跨实现测试向量。

## 18. 参考资料

- [Actr URI 规范](uri.zh.md)
- [Actr Authority Discovery](authority-discovery.zh.md)
- [Actr Realm Profile](realm-profile.zh.md)
- [RFC 2119：Key words for use in RFCs to Indicate Requirement Levels](https://www.rfc-editor.org/rfc/rfc2119.html)
- [RFC 8174：Ambiguity of Uppercase vs Lowercase in RFC 2119 Key Words](https://www.rfc-editor.org/rfc/rfc8174.html)
- [RFC 8032：Edwards-Curve Digital Signature Algorithm](https://www.rfc-editor.org/rfc/rfc8032.html)
- [FIPS 180-4：Secure Hash Standard](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
