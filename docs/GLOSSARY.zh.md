# Actrium 术语表

本表是 Actrium、Actr 协议、Loran 与安全模型文档唯一的术语表；其他文档不再单独维护术语表，
需要时链接到本表。历史名称只出现在本表末尾「历史名称对照」。

**写作规则**：正文中凡不是专有名词的普通词（provider、label、token、domain、key、
credential、request、registry、invitation 等）必须带限定前缀，前缀为其所属专名（Realm、
Actor、Publisher、Workspace、Issuer、Actr）；有歧义处一律写全称。schema 字段名、JSON
成员、ABNF 规则名、配置键、`op` 与状态枚举值保持简短，正文引用时用反引号，首次出现带
全称（如「RealmId（`realm_id`）」）。由内容计算得出的标识叫 Digest（如 ActorStateDigest、Package digest），分配得出的才叫 Id（如 ActorId）。状态词、sequence、rotation 附着宿主（active Realm、
active key、Realm Profile sequence）。其他 credential 一律带主语（Actor credential、TURN
credential、Workspace Provider credential）。正文用规范名，代码类型名（`ActrId`、`ActrType`）
只在指代代码时使用。

## 1. 产品与组件

| 术语 | 定义 |
| --- | --- |
| Actrium | 整个产品与仓库：本地优先的应用运行平台。 |
| Actr | Actor 通信、身份、发现与 workload 运行时框架，以及 `actr://` URI、authority discovery 等协议（`actr/protocols/`）。 |
| Actr Host / Guest | Actr Host 是创建 Actor 实例、提供 mailbox、transport、storage、clock、UI 等受控能力并装载 Workload 的可信宿主软件（实现 Actr 运行时），与在其中运行的 Guest（workload guest）相对。Actr Host 是运行环境，不是远端 Actor。 |
| Actrium | 用户端产品：程序浏览器与宿主，提供 Actr Host 能力及更多功能（运行应用实例、管理 Realm 客户端 key、新建实例、打开 Realm 链接等）。不是 Actr Host 的同义词。 |
| Actrium Shell | Actrium 中 Tab 之外的产品外壳：地址栏、Tab strip、权限弹窗等。 |
| Program Catalog | Actrium 中回答「我可以运行什么」的目录：已安装程序、Marketplace 收藏、本地导入等，每个程序带版本、digest、来源与更新状态。 |
| Service Inventory | Actrium 中回答「我已经创建了什么」的清单：以 Realm 名为键（升级时不变），记录当前 Service 的 ActorId、程序版本、Workspace、运行位置以及生命周期、可达性、健康度、会话占用四组状态。 |
| Keel | 沙箱化动态原生 UI 的 Host/Guest 运行时。 |
| Actrium Component Registry | Actrium 内可供 Keel Guest 使用的组件注册表；与 Actor Registry、Publisher Registry 无关。 |
| Loran | 默认/参考服务端套件，即 Actr Facility Provider 运行的软件：Signaling、STUN/TURN、Issuer、Issuer Signer、Publisher Registry、包分发与管理。代码在 `crates/loran-*`。 |
| Issuer | 为 Realm 签发 Actor 身份的服务（一个 Issuer 服务承载多个 Realm；代码 `crates/loran-issuer`，路由 `/issuer`，配置 `[services.issuer]`）：验证注册证明，分配 Actor serial，经 Issuer Signer 签发短期 Actor credential。不是用户目录，也不拥有 Workspace。 |
| └ Issuer Signer | Issuer 的签名后端（代码 `crates/loran-signer`，配置 `[services.signer]`，其他服务经 `dependencies.signer` 访问，gRPC 包 `signer.v1`）：持有各 Realm 的 Realm issuer key 私钥（每个 Realm 一把），为 Issuer 签名，并向 Signaling 提供公钥用于验证 Actor credential。没有下载私钥的 API。可与 Issuer 同进程；生产环境建议独立部署在安全边界或 HSM 内，使主进程失陷不泄露私钥。存储加密 KEK 经 `LORAN_SIGNER_KEK` 提供。 |
| Publisher Registry | Loran 中管理 publisher 身份、Publisher 签名公钥与密钥撤销的服务（代码 `crates/loran-publisher-registry`，公开路由 `/registry/*`，管理 API `/admin/api/publishers`），也是 `register_publisher` 的接收方。Publisher key id 形如 `actr-pub-<16 hex>`。 |
| Actor Registry | Loran 按 Realm 记录在线 Actor 及其 Actor 类型、端点的注册表，供发现与连接 ACL 使用（SQLite 表 `actor_registry`）。 |
| Marketplace | 程序发现、审核与撤销信息服务；不是 Publisher 命名权威，来源链只看域名与包签名。 |

## 2. Publisher、程序与包

| 术语 | 定义 |
| --- | --- |
| Publisher | 程序发布者，以规范化 DNS 域名（Publisher 域名）为 ID。Publisher 域名给「程序」命名，Realm 域名给「实例」命名，两种角色不同。 |
| Publisher Profile（`actr.publisher/v1`） | authority descriptor 中带签名的可选成员 `publisher`，由 active 的 Publisher Profile 签名 key 签署。其包签名公钥用于验证该 Publisher 的包与 Program Release 记录。 |
| Program | Publisher 发布的程序。 |
| Program Release | `publisher/program@version` 对应的不可变发布。Actrium Product v1 使用 Publisher 签名的发布记录固定 Service 与 Front 两个包的摘要。 |
| Actor 类型（程序身份） | 唯一文本形式 `publisher/program@version`，表示不可变的程序版本，不表示某个 Actor 实例；由注册请求声明、写入 Actor credential 与 Actor Registry，供连接 ACL 与发现使用，不在 ActorId 中。linked/local workload 可使用 Actr Host 管理的非 DNS publisher 值，不能借此进入 Publisher Registry。代码类型名为 `ActrType`。 |
| Actor Package（`.actr`） | Actr 签名 Workload 包，格式为 `actr-package/2`。清单 `[actor]` 以 `publisher`、`name`、`version` 标识 Actor 类型。Product v1 的 Service Workload 和 Keel Guest Workload 分别装在各自的包中。 |
| Package digest | 一个完整 `.actr` 归档的 SHA-256。 |
| Workload | Actor 内的业务计算：处理 Actor message、生命周期事件和允许的 Actr Host 调用。 |
| Workload Profile | 带版本的执行约定（如 `keel.guest/1`），规定 Actr Host 如何解释 Workload entry、建立 ABI、映射 message 与生命周期，以及允许哪些 profile 专有 manifest 字段。 |
| linked workload / Native workload | 由 Actr Host 平台原生提供并与宿主一起构建和签名的 Workload。Actrium 提供原生 Front Host，Publisher 的 Front 包提供由该 Host 装载的 Keel Guest Workload。 |
| Manifest | Publisher 对包的静态描述；在安全模型中只是有效能力声明的输入。 |

## 3. Actor、界面与运行时

| 术语 | 定义 |
| --- | --- |
| Actor | 运行中的程序实体，由 ActorId 标识；安全模型中唯一的规范节点和宏观通信、执行边界。 |
| Service | 实例的 Service：每个 Realm 恰好一个，随 Realm 创建；有持久 ActorId，固定运行 Realm Profile 所指的 Service 包，持有实例状态与 Workspace，生命周期独立于 Tab，可本地或远端运行。Front 只连它；它是本 Realm 内唯一与包同类型的 Actor，Front 按类型找到它。同一 Realm 内的其他 Actor 由它编排，不另设类别名。 |
| Front Actor | 每个 resident app Tab 的短生命周期 Actor，类型和 Keel Guest Workload 来自已验证的 Front 包，由 Actrium 的原生 Front Host 装载。 |
| Front Host | Front Actor 的受信原生宿主部分：随 Actrium 发布，Publisher 不可替换或扩展；负责 Actor 注册、身份、连接、会话、Keel Engine 与 Host capability，Keel Guest 运行其上。 |
| Keel Guest | Publisher 唯一可定制的前台代码，运行于 Keel。 |
| Tab | Actrium Shell 中的用户交互容器；Tab record 可在 Front Actor 回收后继续存在（dormant Tab）。 |
| Attach | Front 连接本实例 Service 的握手。双方按已验证的发布记录核对 Actor 类型与契约，Front 取得 Attachment lease。 |
| Attachment lease | Front 使用 Service 的短期会话租约（v1 默认 `exclusive`）。 |
| capability | Actr Host 在本机明确授予 Guest/Service 的最小受控能力或句柄（如 `host:*` capability、Workspace capability、session capability），由 Actr Host 执行，不跨边界签名；与安全模型的 Actor Grant 不同。 |
| Workspace | Service 的持久数据边界：由 Actr Host 托管、经 Workspace Provider 存储的 capability，不是简单文件路径。一个 Service 至多绑定一个 Workspace；同一 Workspace 可按写入并发策略（默认 `exclusive-writer`，另有 `shared-readers` 等）被多个 Service 使用。Workspace 不自动是 Actor，同一 Principal 也不自动获得授权。删除 Service 默认不删除 Workspace。Workspace 不属于 Loran 的 Realm 基础设施；其存储方 Workspace Provider 与 Actr Facility Provider 不是同一概念。 |
| Native Capsule | 在独立 OS sandbox 中运行、只开放 Actr Host brokered IPC 的 Native workload。 |

## 4. Realm 与身份

| 术语 | 定义 |
| --- | --- |
| Realm | 一个应用实例的信任与治理域：Actor 身份、Realm membership、Realm 客户端准入、连接 ACL 与 Actr Facility Provider 委派都以它为边界。创建一个应用实例即创建一个新 Realm（跟实例走，不跟应用走）。Realm 不是应用（程序），不是 Loran 实例，不是一台信令服务器，也不是消息明文端点或应用保障权威；一个 Loran 实例承载多个 Realm。 |
| Realm 域名 | Realm 名中的域名部分：普通 DNS 域名，不含端口。域名只是天然唯一的命名空间根，不分类；没有域名就不分配 Realm。一个 Realm 域名下可以有多个 Realm。唯一例外是本机 Realm。 |
| Realm 名 | Realm 的完整名字 `<realm_id>@<domain>`，如 `k7m2q9xh@acme.com`。形似邮件地址但不是，不得渲染为 mailto。 |
| RealmId（`realm_id`） | Realm 名中 `@` 之前的部分，在所属域名内唯一，是分配得出的标识（字符规则同 DNS label：1–63 个小写字母、数字或 `-`）。默认随机生成，创建时可显式选择可读的 RealmId；创建后不可修改，退役后永不复用。完整身份始终是 Realm 名。 |
| display_name | 给人看的实例名称，写在 Realm Profile 中，不进入 ActorId。 |
| 本机 Realm | 域名为本机伪域名 `<installation-id>.actr.alt`（RFC 9476，永不经 DNS 解析）的 Realm：只有本地记录，没有 Realm Profile，不经过 Actr Facility Provider，其他设备无法解析。 |
| installation-id | Actrium 安装时随机生成的 128 位标识（base32 小写 26 字符），只用于本机伪域名。 |
| Actr Facility Provider 域名下的 Realm | 在规范化 A-label 上比较，Realm 域名等于其 Realm Profile 的 Actr Facility Provider ID，或以 `"."` 加该 ID 结尾（按 label 边界）的 Realm。只有它：显示 Actr Facility Provider 为信任根；v1 只由 Actr Facility Provider 运营者创建，Actr Facility Provider 自签 Realm Profile，不经 Realm 管理请求（用户以 Actr Facility Provider 账号自助创建属后续能力）。其他 Realm 一律显示 Realm 域名为信任根，走 Actr 管理信封。 |
| ActorId | Actor 实例的持久身份，与 Actor 长期 key 绑定，类似进程 ID：`<serial>.<realm_id>@<domain>`，如 `2af01.k7m2q9xh@acme.com`。单个 `@`，左边按 DNS 顺序从具体到一般，去掉 `<serial>.` 即 Realm 名；URI 中 `@` 写作 `%40`。不含程序类型，不是 Workspace、用户或可变服务别名。代码类型名为 `ActrId`（字段 `realm` 为 Realm 名、`serial_number` 为 Actor serial）。 |
| Actor serial | ActorId 中由该 Realm 的 Issuer 分配的序列号，文本形式为小写十六进制（当前实现为 54-bit Snowflake）。 |

## 5. 信任与 key

| 术语 | 定义 |
| --- | --- |
| authority descriptor | 每个域名在 `https://<domain>/.well-known/actr` 发布的一份公开 JSON：列出该域名的 Actr 服务，并可携带 `realm_signing_keys`、`publisher`、`links`。整体由 HTTPS 保护、未签名，其中 `realm_signing_keys` 与 `publisher` 各自签名。 |
| `links` | authority descriptor 中的路径前缀数组（如 `["/i/"]`），声明本域名下哪些 https 路径是 Realm 链接；前缀由域名自选，没有默认值，Actr Host 只按声明转换。 |
| `realm_signing_keys` | authority descriptor 中的签名成员（Actr Authority Discovery §4.4）：公布 Realm 签名 key、Realm 签名 key 轮换链与 sequence。一个 Realm 域名只需发布一次。 |
| Realm 签名 key | Realm 域名所有者在 `realm_signing_keys` 中公布的 Ed25519 key，是该域名下所有 Realm 的长期信任根，签署 Realm Profile 与 Actr 管理信封；可有多把，只有 active key 签名，至少一把为 active。从 `keys` 中移除某 key 即吊销它签过的一切。 |
| Realm 签名 key 持有方 | 持有某 Realm 域名 Realm 签名 key 的一方，即域名所有者；可委托某个 Actr Facility Provider 代持（受托代持对客户端不可见，属已接受的限制）。没有自己域名的个人由 Actr Facility Provider 在其域名下分配 Realm，此时该 Actr Facility Provider 即持有方与信任根。Loran 默认不持有第三方域名的 Realm 签名 key。信任根显示与授权路径见「Actr Facility Provider 域名下的 Realm」。 |
| Realm 签名 key 轮换链（`rotations`） | 旧 key 签名 `{from, to, public_key, sequence}` 认可新 key 的记录；没有轮换链的 key 变化视为身份变化，需重新确认信任。 |
| `realms` relation | authority descriptor 中指向 Realm Profile 存放位置的链接：`GET <href>/<realm_id>`。 |
| `realm-management` relation | Actr Facility Provider 自身 authority descriptor 中的 Realm 管理请求端点（`POST`，protocol `actr-realm-management/1`）。 |
| Realm Profile（`actr.realm/v1`） | 每个 Realm 一份的公开文档，由 Realm 签名 key 签名：`realm.name`、状态、该实例运行的包（`package`：下载地址、SHA-256 与 Actor 类型）、Realm issuer key、Actr Facility Provider 与端点。放在哪都行（`realms` 位置、Actr Facility Provider、Realm 邀请内嵌），存放方不成为信任根，有效性只看签名；不过期，回滚保护只靠 Realm Profile sequence；`provider.id` 在 Realm 生命周期内不可变。签名 key 被移除后，回退到仍在 `keys` 中的 key 签署的最近一份。 |
| 退役 Realm Profile | `status: retired` 的 Realm Profile，由 Realm 签名 key 签署，永久有效，Realm 名不再复用。退役只有这一条路径。 |
| Realm issuer key | Issuer 为每个 Realm 持有的签名 key：私钥在 Actr Facility Provider 的 Issuer Signer 中，公钥列在 Realm Profile。 |
| Actor credential | Issuer 签发给 Actor 的短期凭证：issuer 字段即 Realm 名，绑定 ActorId、Actor key、Actor 类型、Realm、有效期、principal/client binding（成员 id、Realm 客户端 key id）与签发时的角色。它证明 Actor key、身份与 claims，不单独证明对端实际运行的代码。 |
| Web PKI / pin | 首次信任 Realm 签名 key 靠 HTTPS 证书体系，或企业预置的 key（pin）。 |

## 6. Actr Facility Provider 与 Realm 管理

| 术语 | 定义 |
| --- | --- |
| Actr Facility Provider（AFP） | 一套正在运营的共享基础设施部署（例如某家运营的 Loran），为多个 Realm 提供 Signaling、STUN/TURN 与 Issuer。不是软件供应方。与 Workspace Provider 不是同一概念。 |
| Actr Facility Provider ID | Actr Facility Provider 自己的 DNS authority（如 `provider.example`）。Realm 与创建时的 Actr Facility Provider 永久绑定，Realm Profile 中 `provider.id` 不可更改；访问 Realm 只使用其 Realm Profile 指定的 Actr Facility Provider，不回退到其他 Actr Facility Provider。 |
| Realm 管理请求 | 发往 `realm-management` 端点的请求：`create_realm`、`publish_profile`、`reissue_owner_invite`、`draft_profile`（字段 `realm` 为完整 Realm 名），只用于 Actr Facility Provider 域名以外的 Realm，是 Realm 签名 key 持有方签名服务签发的 Actr 管理信封（含受托代持的 Actr Facility Provider）。 |
| Actr 管理信封 | Realm 管理请求、`register_publisher` 与 membership 请求共用的签名请求格式（Actr management envelope）：JCS + detached Ed25519，绑定 `provider_id`、≤5 分钟有效、nonce 防重放；签名 key 分别是 active Realm 签名 key、active Publisher Profile 签名 key 与成员的 Realm 客户端 key。 |
| pending / active Realm | 创建过程的状态：`create_realm` 后为 pending Realm；持有方以 `publish_profile` 提交签名 Realm Profile 后为 active Realm（Actr Facility Provider 域名下的 Realm 由 Actr Facility Provider 自签，无需此步）。只有 active Realm 签发 Actor credential、接受 Signaling 连接和分配 TURN；过期的 pending Realm 由 Actr Facility Provider 自动清理。 |
| owner 邀请 | 角色为 owner 的 Realm 邀请，把一个 Realm 的管理权交给其 owner：持有方首次 `publish_profile` 激活时、Actr Facility Provider 运营者创建时一次性返回；owner 丢失全部客户端时由持有方（`reissue_owner_invite`）或运营者重新签发，兑换后成为 owner 的新客户端、owner 原有客户端 key 全部撤销。新签发的使此前未用的失效，有效期 7 天。 |
| 实例运营者 | Loran 实例唯一的系统管理根，管理 Actr Facility Provider 配置与全部 Realm 生命周期。 |
| Actr Facility Provider tombstone | Actr Facility Provider 本地以 Realm 名为键的已退役/已用记录，防止 RealmId 复用；与退役 Realm Profile（公开、签名）不同。 |
| 本机实例 tombstone | Actrium 清除实例后的本地记录。 |

## 7. 成员、客户端与权限

| 术语 | 定义 |
| --- | --- |
| Realm membership | 成员为 active，且其客户端已在该 Realm 登记 Realm 客户端 key、未被撤销；由此可申请 Actor credential、连接 Signaling、进行 Attach。没有单独的 membership credential：Issuer 只对这样的 key 签名的请求签发 Actor credential，撤销客户端即撤销其 key 登记。协议见 Actr Realm Membership。 |
| Realm 客户端 key | 某成员的某个客户端安装在某个 Realm 中使用的 key，按 Realm 独立生成，可单独吊销；不跨 Realm 复用，防止跨实例关联同一客户端。对应 Realm 客户端注册。 |
| 成员（`member_id`） | Actr Facility Provider 为某个联网 Realm 记录的一方：`member_id`（16 位十六进制）、角色与状态（active/removed）。凭 Realm 邀请加入，可有多个客户端；移除成员或离开 Realm 即撤销其全部 Realm 客户端 key。本机 Realm 没有成员。 |
| 角色（owner / editor / viewer） | 成员在实例内的应用权限角色，由 Actr Facility Provider 随成员记录保存、写入 Actor credential，由 Service 执行；改角色不撤销 key，在下一份 credential 生效。每个 Realm 恰好一个 owner：它同时持有成员管理权（邀请、撤销客户端、移除成员、改角色）与应用 owner 角色，可原子转让，v1 不支持多 owner。 |
| membership 请求 | 成员用 Realm 客户端 key 签名、发往 `<issuer.url>/membership` 的 Actr 管理信封：`join`、`create_invite`、`revoke_invite`、`list_members`、`revoke_client`、`remove_member`、`set_role`、`transfer_owner`、`leave`，以及只用于 Actr Facility Provider 域名下 Realm 的 `update_realm`（owner 更新程序、名称或退役）。 |
| renewal token | Actor credential 续期用的凭证，绑定 Realm 客户端 key。 |
| Principal / PrincipalId | 使用某 Actor 实例的一方及其外部不透明标识；不是 Publisher、Actr Facility Provider 或运营方。一个 Actor 实例完整生命周期只绑定一个 PrincipalId。Actor credential 中的 principal/client binding 即成员 id 与 Realm 客户端 key id。 |
| 应用权限 | 实例内的权限（读写、管理、分享），由应用与会话授权决定；与 Realm membership 分开，取得 membership 不等于获得任何应用权限。成员角色是其最小部分，由 Service 从 Front 的 Actor credential 读取。 |
| 连接 ACL | Actr 运行时与 Signaling 按 Actor 类型与 Realm 决定谁可发现、连接谁的规则（`AclRule`）。 |

## 8. 流程与 URI

| 术语 | 定义 |
| --- | --- |
| Realm 链接（`actr://` URI） | 应用实例或其中某个 Actor 的地址：`actr://<Realm 域名>/<realm_id>[/<页面路径>]` 指向实例，`actr://<Realm 域名>/<serial>.<realm_id>[…]` 指向该 ActorId 的 Actor（第一段即 Realm 名或 ActorId 在 `@` 左边的部分）。页面路径、query、fragment 原样交给应用。https 形式是 `https://<Realm 域名><links 前缀>…`，设备上没有 Actr Host 时打开普通网页；本机 Realm 没有 https 形式。URI 不携带任何秘密。 |
| 新建实例（Create） | Actrium 内的本地操作：从 Program Catalog、Marketplace、本地文件或包地址选包，选 Realm 域名 → 持有方授权 → pending Realm → 签名 Realm Profile（含 `package`）并以 `publish_profile` 提交（Actr Facility Provider 域名下的 Realm 无需此步）→ active Realm → 创建 Service。成功后地址栏显示该实例的 Realm 链接。 |
| Create 幂等键（`idempotency_key`） | 新建实例的本地持久化标识：重试时继续同一个 pending Realm 与同一 RealmId。Actr Facility Provider 侧幂等键为 `(realm, 请求方)`，请求方是 Actr 管理信封中的 `requester`。 |
| Realm 邀请 | 加入某个实例 Realm 的一次性 secret：带角色（邀请新成员）或指向已有成员（为其添加客户端），可过期、签发者可在兑换前撤销；兑换时由新 Realm 客户端 key 签名并原子消耗。文本形式 `ri_…`（携带 Realm 名，可内嵌 Realm Profile），只经带外渠道交付，不进入 URI。过期、已使用、已撤销与 Realm 不匹配是不同错误。 |

## 9. 网络

| 术语 | 定义 |
| --- | --- |
| Signaling（信令） | 帮助已认证 Actor 做 presence、发现和交换 WebRTC 建连信息的控制面（Loran 文档中也称 Rendezvous）。不签发身份，只转发结论摘要，不是明文端点，也不是通信内容的最终信任根。 |
| signaling ticket | Issuer 签发的一次性、极短期 Signaling 入场凭证，只在内存中；`actr://` URI 永不携带。 |
| STUN | Session Traversal Utilities for NAT：公开 Binding 服务，让客户端得知服务端观测到的网络地址（NAT 映射发现）。 |
| TURN | Traversal Using Relays around NAT：P2P 不可达时的受控密文中继（Loran 文档中称 Relay 平面）；allocation 需要绑定 Realm/Actr Facility Provider/audience/Actor/expiry 的短期 TURN credential。配置中的 `[turn].realm` 是 TURN 认证域字符串（TURN REALM），与 Actr Realm 无关。 |

## 10. 安全与隐私模型

安全语义以 `docs/architecture/security/actrium/` 为准；本节只给定义。

| 术语 | 定义 |
| --- | --- |
| Actor 主体 | 用户在品牌 / 软件层识别的对象。 |
| Actor 实例（安全模型） | `(节点应用 + 实例配置) + 宿主环境`；节点应用包括 Workload、Host 软件、Manifest 和装载闭包。 |
| Host 软件 | 与 Workload 一起构成节点应用、提供 API 与执行点的软件。 |
| 宿主环境 | 软件包之外的 OS、硬件、TEE、管理员边界等。 |
| ActorState / ActorStateDigest | ActorState 是 Actor 实例事实的不可变原子记录（`actor_id`、`principal_id`、单调修订号，以及软件包、实例配置、有效能力声明、宿主环境四个子指纹）；ActorStateDigest 是对它计算的内容摘要，充当该状态的唯一标识，与 ActorId 成对使用。修订号进入摘要，内容回滚也得到新摘要。证据、策略、评定结果与在线状态不进入 ActorState。 |
| 有效能力声明 | Actor 当前实际可能造成的全部规范效果的闭合上限，是安全计算的唯一能力清单；Manifest 只是输入。 |
| Transfer / Retain | V1 仅有的两种规范效果：`Transfer(target)` 指应用信息可能被边界外目标取得；`Retain` 指临时使用边界结束后仍能恢复应用信息。 |
| Actor Grant | 权力来源直接签发给目标 `ActorId + ActorStateDigest` 的自包含签名授权（V1 不允许转授权），决定某个动作现在能不能做；不裁剪有效能力声明，也不是安全结论。`ActorStateDigest` 一变即不再匹配，须对新状态重新签发。 |
| 数据输入授权 | 决定麦克风、摄像头、文件等数据是否进入应用；与颜色正交。 |
| 风险接受 | 用户了解当前黄 / 红事实后允许继续运行；只影响运行许可，不改事实和颜色。 |
| Claim / Evidence | 待验证陈述，以及用来核实它的材料。 |
| Verifier（验证者） | 按规则、证据和用户策略核实状态的一方。 |
| TrustPolicy | 版本化的信任与行为模式规则。 |
| 评定上下文 | `ActorStateDigest` + 策略版本 + 规则集；同一上下文内多份结论取最弱项。 |
| ActorAssurance | 验证者针对准确 `ActorStateDigest`，在特定证据、策略、规则和有效期下给出的节点结论；禁止自签。 |
| Actr Host 证据键 | Actr Host 为 workload 出口闭包提供证据时使用的独立、可验证 key；本机 Realm 只绑定当前安装，联网 Realm 可绑定 Realm 客户端 key。Actor 长期 key 不能冒充它。 |
| 应用图（ApplicationStructure） | 由有效 `Transfer(ActorId)` 依赖形成的无根最大连通组；在线与否、是否通信不改变成员。 |
| EncGraph / structureDigest | EncGraph 是应用图的规范字节编码，对带标签节点 / 边集合单射；structureDigest 是其摘要 `H("actrium/structure/v1", EncGraph(G))`，成员或边变化即改变。 |
| Depends / LocalContinue | 无向有效依赖；不依赖变化节点时允许局部继续，但不得展示应用绿色。 |
| ApplicationStateDigest / ApplicationAssurance | ApplicationStateDigest 是应用当前状态的承诺摘要，覆盖 structureDigest、Principal 以及每个 `ActorId + ActorStateDigest + H(ActorAssurance)`；ApplicationAssurance 是它在当前就绪切面上的汇聚结论。 |
| Ready | 已发布快照仍对应当前指针，且全部图成员 `ACTIVE`；展示颜色的前提。 |
| 状态权威 | 某修订期内唯一可认证的当前状态发布者。 |
| 保障挂起 | `ASSURANCE_SUSPENDED`：隔离应用数据，但不等于停机。 |
| 有效 Actr 通道 | 端到端加密、完整性、双向绑定双方 `ActorId + ActorStateDigest` 的通道。 |
| TCB | 失陷会破坏相应保障的组件集合。 |
| CONTROLLED / UNKNOWN / UNCONTROLLED | 出口事实（不是产品第四色）：出口受不可绕过控制且命中可信行为模式 / 缺少足够依据 / 已确认存在越界泄漏能力或 `Retain`。`UNCONTROLLED` 表示能力存在，不等于已观测到泄漏。 |
| 绿 / 黄 / 红 | Actrium 顶层隐私标记；绿只保证当前就绪切面。 |
| 数据世代 | 应用数据取得时的 `ActorStateDigest`；旧数据适用「取得时声明 ∩ 当前声明」。 |

几组不要混用的词：

- **Actor Grant** 是权力来源签名、绑定 `ActorId + ActorStateDigest` 的授权；**capability** 是 Actr Host 在本机发放、由 Host 执行的受控能力。
- **TCB** 是「必须信赖它」；**CONTROLLED** 是「当前这项出口有充分证据」。
- **Security** 是广义安全属性；**Privacy** 是谁能接触、保留和输出数据；**Assurance** 是凭什么相信某属性在给定作用域成立。绿黄红优先表达隐私保障。
- **加密** 保护途中或静态介质；端点拿到明文后能做什么，由能力和执行点决定。
- **Attestation** 提供环境 / 测量证据；**Authorization** 才决定是否放行。
- **Native / Wasm** 是执行技术，不是颜色。

安全模型已废弃、不再作为规范用语的旧词：GraphRoot 作为图安全根、逐消息权限、动态 Capability 交集、Waiver 改色、四值 Claim Verdict 作为顶层颜色。

## 历史名称对照

正文统一使用左列规范名；右列历史名只出现在本表、数据库迁移代码与旧文档中。
服务端套件的当前包名、二进制、配置、环境变量与指标均统一使用 Loran 命名。

| 规范名 | 历史名 | 说明 |
| --- | --- | --- |
| Actrium（用户端产品） | Actro | 用户端产品改用项目名；已安装应用的 bundle id、Keychain 服务名等为兼容升级保留旧值 |
| Issuer | Authority、AIS（Actor Identity Service） | 目录 `crates/loran-ais` → `crates/loran-issuer`；当前包 `loran-issuer`；路由 `/ais` → `/issuer`；配置 `[services.ais]` → `[services.issuer]`；Actr.toml `ais_endpoint` → `issuer_endpoint`；表 `ais_renewal_token`、`ais_reissue_response_cache` → `issuer_renewal_token`、`issuer_reissue_response_cache`（schema v2 离线迁移）；`ais_manufacturer_auth_nonce`（曾改名 `issuer_publisher_auth_nonce`）随 Publisher 私钥注册证明一并删除；管理路由 `/admin/api/services/ais/keys` 与 key 存储 `ais_keys.db` 随全局轮换 key 删除（改用每个 Realm 的 `issuer:<realm>` key） |
| Issuer Signer | KS、Key Server | gRPC 包 `ks.v1` → `signer.v1`；配置 `[services.ks]`、`dependencies.ks` → `[services.signer]`、`dependencies.signer`；当前环境变量 `LORAN_SIGNER_KEK`；示例 `ks.db` → `signer.db`；Postgres 默认库 `loran_signer`；指标 static `KS_*` → `SIGNER_*`（导出的指标名前缀为 `loran_`） |
| publisher | manufacturer | 包清单 `actr-package/1` 与 proto `ActrType` 曾用此字段名；`actr-package/2` 起统一为 `publisher`（proto 字段编号不变），旧键被拒绝并提示改名；Actor 类型字符串旧形式 `publisher:name:version`（曾写作 `manufacturer:name:version`）→ `publisher/program@version`，旧形式被拒绝，已存数据由 schema v2 离线工具改写；CLI 用户配置 `[mfr] manufacturer` → `[publisher] name` |
| Publisher Registry | MFR（源自 manufacturer） | 目录 `crates/loran-mfr` → `crates/loran-publisher-registry`；当前包 `loran-publisher-registry`；路由保持 `/registry/*` 与 `/admin/api/publishers`；历史配置 `[services.mfr]` 与 `[services.publisher_registry]` 已移除；表 `mfr*` → `publisher*`、列 `mfr_id`/`manufacturer` → `publisher_id`/`publisher`（schema v2 离线迁移）；key id 格式 `actr-pub-<16 hex>` 不变 |
| Actor Registry | service registry | 类型 `ServiceRegistry` → `ActorRegistry`；SQLite 表 `service_registry` → `actor_registry`、列 `actor_manufacturer` → `actor_publisher`（启动时原地改名） |
| Actr Facility Provider ID（Actr Facility Provider 的 DNS authority） | 固定 Provider ID 常量 | 2026-09 C2.1 前的实现；现取 Loran `[authority] uri` 的域名 |
| Realm Profile | Realm descriptor（`/.well-known/actr/realm.json`） | 2026-09 C2.2 前 Loran 在该路径发布的单 Realm 非秘密元数据；与 authority discovery「`/.well-known/actr` 下不定义子路径」冲突，已移除，改由 Realm 签名 key 签名的 Realm Profile（`GET /actr/realms/{domain}/{realm_id}`）取代 |
| Realm 名 | 数值 `realm_id`、`actr:realm:<realm_id>:authority` | 2026-09 C3 前的 Realm 标识与 credential issuer；与 RealmId（`@` 左边那段）无关 |
| Realm 签名 key、`realm_signing_keys` | 命名空间 key、`realm_namespace` | 本契约早期草稿用语 |
| RealmId（`realm_id`） | Realm label（`label`） | 改名以与 ActorId 等 *Id 风格统一 |
| RealmId、Realm 名 | `local_name`、Realm 引用、`dns:`/`local:` 前缀 | 本契约早期草稿用语 |
| Realm 签名 key 持有方 | 组织域名、托管域名、provider 自有命名空间、组织签名服务 | 本契约早期草稿用语（域名不再分类）；「组织签名服务」仅作为域名分类的说法退役，持有方运行签名服务签发 Actr 管理信封的机制仍在 |
| Actr 管理信封 | 签名信封 | 本契约早期草稿用语 |
| Realm 客户端 key | 设备 key、device private key | 本契约早期草稿用语 |
| owner、owner 邀请 | Realm 管理员、Realm 管理 token（`rat_…`）、`reissue_admin_token` | 2026-09 membership 设计取代：管理权由 owner 用自己的 Realm 客户端 key 签名行使，丢失时以 owner 邀请恢复 |
| Realm 客户端 key 持有证明（`x-actr-client-proof`） | Realm registration secret（Realm secret，共享注册 secret 请求头） | 2026-09 membership 设计删除的 Realm 共享注册 secret |
| （后续能力） | Actr Facility Provider 账号（OAuth 2.0 + PKCE，client ID `actr-host`） | v1 不做，Actr Facility Provider 域名下的 Realm 只由运营者创建 |
| renewal token | Realm refresh material | 产品文档早期用语 |
| Actor credential | Realm credential | 产品文档早期用语；当前代码类型名为 proto `AIdCredential` 与 `crates/loran-issuer` 的 `AIdIssuer`、`AIdSerialNumberIssuer`，Actor 类型在 claims 中的字段为 `IdentityClaims.actor_type` |
| Actor credential（含 Realm membership 证明） | Realm membership credential | 本契约早期草稿用语：并入 Actor credential，membership 即已注册未撤销的 Realm 客户端 key |
| Actr Facility Provider | provider | 本契约早期草稿用语（泛词） |
| `publish_profile` | `activate_realm` | 早期草稿只管首次激活；合并为发布 Realm Profile（首次即激活，之后即更新或退役） |
| ActorStateDigest、ApplicationStateDigest、structureDigest | ActorStateId、ApplicationStateId、structureId | 安全模型早期用语；按「摘要类标识叫 Digest」统一，代码与字段同步改为 `*_digest` |
| Service | Primary Service、Service Actor | 产品文档早期用语；Realm 即实例后不再区分主次 |
| Realm 内的其他 Actor | Auxiliary Actor | 产品文档早期用语 |
| Realm 链接 | Create URI、Attach URI、`actr://<publisher>/<program>@<version>`、`service`/`domain`/`realm` 链接参数 | 产品文档早期用语；新建实例改为 Actrium 本地操作 |
| （已删除） | Package Resolution、`packages` relation、`program_resolver`、`actr-packages/1` | 程序从哪来改由 Realm Profile 的 `package` 给出 |
| Actor Grant | Capability（安全模型中的签名授权） | 安全模型早期用语；形式模型代码类 `Capability` → `ActorGrant` |
