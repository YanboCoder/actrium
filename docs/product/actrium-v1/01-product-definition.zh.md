# 01. 产品定义

## 1. 产品愿景

Actrium 要把 Actor 程序变成普通用户可以理解、安装、创建、打开和恢复的产品形态。

用户看到的是熟悉的浏览器：

- 有地址栏、Tab、历史、收藏、程序库和安全提示；
- 一个地址可以创建新的程序实例，也可以回到一个已有实例；
- 程序可以是本地的，也可以连接远端；
- 关闭 Tab 不会误删工作状态；
- 安装程序不等于允许它任意执行本机代码。

开发者看到的是受约束的小程序平台：

- 前台只写 Keel Guest；
- 不需要自己实现窗口、路由、Actor 注册、信令、权限弹窗和跨平台 Actr Host；
- 前台永远通过该实例 Service 的类型化 capability 访问业务；
- 后台 Service 可以使用 Actor 模型组合更复杂的服务；
- 一个精确版本的包可以经 HTTPS 下载、验证、缓存和复现。

## 2. 当前问题

如果不先冻结模型，Actrium 很容易同时承担浏览器、插件系统、远程桌面、原生应用容器和 Actor 调试器等多套语义，最终出现以下问题：

- Tab、页面、插件、Actor、Service 和 Workspace 的生命周期互相覆盖；
- 前台代码既可以走 Keel Bridge，又可能绕过它直接访问原生模块或 Actor 网络；
- 「打开程序」有时创建实例，有时发现服务，有时连接固定服务，地址栏无法解释；
- 状态型 Service 被当成可按类型负载均衡的无状态副本；
- Marketplace、Publisher、Realm 和信令被混成一个中心化信任源；
- 使用 HTTPS 下载了包，却没有版本不可变、digest 固定和离线校验；
- 关闭 Tab、刷新、崩溃恢复可能错误创建、替换或清除 Service。

本 PRD 的目的就是用少量稳定不变量消除这些歧义。

## 3. 目标用户

### 3.1 最终用户

需要运行轻量 agent、小程序或 Workspace 工具，但不希望理解 Actor、Realm、信令和运行时细节的用户。

核心任务：

- 从市场或本地安装一个程序；
- 选择 Workspace 并创建一个实例；
- 稍后从历史、收藏或实例库继续工作；
- 在另一设备上连接已有实例；
- 明确知道程序来自谁、实例属于哪个 Realm（及其域名）、获得了哪些权限。

### 3.2 程序开发者

开发一个有原生 UI、Actor 后端和 Workspace 状态的程序。

核心任务：

- 只用 Keel Guest API 写跨平台前台；
- 用 Protobuf/契约描述 Front–Service API；
- 构建 Service workload；
- 声明组件、权限、资源配额和 Workspace 模式；
- 用自己的域名发布不可变 `.actr` 包；
- 在本地开发模式快速调试。

### 3.3 Publisher

通过域名发布程序，维护 Publisher 签名 key、版本和撤销信息。Realm 按实例创建，Publisher 无法预先指定某个 Realm。

### 3.4 Realm 签名 key 持有方与 Realm 所有者

Realm 域名的所有者即 Realm 签名 key 持有方：在 authority descriptor 中发布 Realm 签名 key，按自己的策略认证请求方后授权在该域名下创建 Realm，决定使用哪个 Actr Facility Provider，并负责 Realm 签名 key 轮换；也可以委托 Actr Facility Provider 代持 key。没有域名就不分配 Realm（本机 Realm 除外）。

Realm 所有者即应用实例的创建者，管理该实例 Realm 的成员身份、Actor 准入、连接 ACL、Realm 邀请、客户端撤销与审计。

### 3.5 Marketplace 运营者

提供程序发现、分类、审核、风险标注和撤销信息，但不拥有 Publisher 的命名权。

## 4. 核心用户故事

### 4.1 新建实例

> 作为用户，我在程序库里选一个程序点「创建实例」，Actrium 应先验证程序、展示权限、Workspace 与 Realm 域名选择，在所选域名下新建一个 Realm 并在其中创建 Service（默认创建本机 Realm），然后在地址栏显示这个新实例的地址。

### 4.2 连接已有实例

> 作为用户，我打开一个实例的链接时，Actrium 只能连接该实例；如果它离线或我无权访问，必须明确报错，不能另建一个「看起来一样」的实例。

### 4.3 关闭与恢复

> 作为用户，我关闭 Tab 只代表结束当前前台会话。Service 与 Workspace 继续存在，除非我在实例管理中显式停止、归档、移入回收站或彻底清除。

### 4.4 安装和验证

> 作为用户，我安装 `publisher/program@version` 时，可以确认 Publisher 域名、精确版本、Service 和 Front 包摘要、权限和市场风险状态。已安装的版本固定到这两个归档。

### 4.5 本地开发

> 作为开发者，我可以从配套的本地 `.actr` 文件或开发 HTTPS Origin 加载未发布程序。Actrium 标记「开发来源」并使用开发身份。

### 4.6 Realm 与网络

> 作为用户，我不需要手填信令和 TURN 地址；Actrium 根据已信任的 Realm 签名 key 签名的 Realm Profile 选择端点和短期凭据，同时让我看见当前实例的 Realm、其域名和连接状态。

### 4.7 多 Actor 隐私状态

> 作为用户，当程序由多个 Actor 组合时，我看到的绿色必须代表当前就绪切面上整图出口都受控；任一节点未知则为黄，已确认未受控则为红；切面未就绪时不能显示绿色。

## 5. 产品目标

### G-01：形成一个统一应用模型

所有普通应用 Tab 都遵守同一条主链：

```text
Tab → Front Actor → Service → Workspace
```

UI-only 程序也使用平台提供的最小 Service，而不是再创造第二套「纯页面应用」模型。

### G-02：把 Publisher 代码限制在可审计边界内

- Publisher 不得替换 Front Native Host。
- Front 只能携带 Keel Guest。
- Guest 默认没有任意网络、文件、Actor discovery 或原生模块能力。
- 所有能力由声明、用户授权和 Actr Host 实现三者取交集。

### G-03：让地址可读、可复制、可恢复

- Realm 链接 `actr://<Realm 域名>/<realm_id>` 回答「哪个实例」，也就是去哪里验证和寻址；
- Realm Profile 固定 Service 包，匹配的签名 Program Release 记录固定 Front 包。
- 地址不携带秘密。

### G-04：复用 Web 的成熟安全基础设施

复用 DNS、IDNA、TLS、HTTPS、证书透明度、缓存、CDN、标准端口和企业代理；同时以包签名和 digest 补足 HTTPS 对离线缓存、镜像和版本不可变性的不足。

### G-05：保持 Actor 模型的组合能力

Front 只看到该实例的 Service，不代表后台只能有一个 Actor。Service 可以在同一 Realm 内管理 worker、媒体、索引、同步等其他 Actor。

### G-06：提供端到端应用保障

- 结论沿无根有效依赖图汇聚；Front 只定位；
- WASM 的声明、imports、有效能力和实际出口由 Actr Host 交叉验证；
- 只有 `Ready` 且全部节点 `CONTROLLED` 才显示绿；
- 黄与红明确区分；挂起不得展示绿；
- 新配置只作用于新数据；状态变化联动通道、Actor Grant 和数据面。

## 6. 非目标

v1 不做：

- 任意网页浏览器兼容层或完整 Web API；
- Publisher 自定义 Front 原生插件、动态库或 Host Module 实现；
- Guest 直接访问任意 ActorId 或按类型发现任意 Actor；
- Service 实例的透明负载均衡；
- 多人实时协作协议；
- 原地无感替换正在运行的 Service workload；
- 跨 Realm（跨实例）自动联邦和全局 Actor 搜索；
- 域名级 SSO 或成员目录；
- 开放匿名 TURN；
- 仅凭 Marketplace 审核标记就授予系统权限；
- 在移动平台承诺 24×7 本地后台 Service 在线。

## 7. 产品原则

### 7.1 明确优于猜测

打开链接只连接；新建只在程序库里显式进行；失败时不改变意图。

### 7.2 地址不等于权限

地址是定位与身份，访问权由 Realm membership、应用权限和会话授权决定。

### 7.3 状态归 Service

Tab、Front Actor、Guest 都可以重建；Workspace 绑定、业务状态与实例权限归 Service。

### 7.4 精确版本不可变

`publisher/program@version` 一旦被 Actrium 接受，该发布版本的 Service 与 Front 包摘要即固定。

### 7.5 默认最小能力

未声明、未授权、Actr Host 未实现的能力一律不可用。

### 7.6 平台限制显式化

移动端后台限制、同进程沙箱强度、离线撤销信息陈旧等事实必须显示，不能用抽象名词掩盖。

### 7.7 隐私取决于最弱项，不取决于平均分

一个受控节点不能抵消另一个未知或未受控节点；缺少证据不能按通过处理。绿色只证明当前就绪切面上可能出口受控，不宣称程序「绝对没有漏洞」，也不表示曾经存在一个全图同时世界。

## 8. v1 范围

### 8.1 必须交付

- macOS/Windows 桌面端完整闭环；
- iOS/Android 前台运行与恢复闭环；
- `actr://` Realm 链接的打开与分享；程序库中新建实例；
- Program Catalog 与 Service Inventory；
- 域名 Publisher、HTTPS 下载、签名和 digest 校验；
- 固定 Native Front Host + Keel JS Guest；
- 本地 Service 创建、停止、启动、连接、回收站和彻底清除；
- Workspace 绑定；
- Realm 签名 key、按实例创建的 Realm 及其 Realm Profile、身份、信令与 TURN 短期凭据；
- `ActorAssurance` / `ApplicationAssurance`、无根依赖图、最弱项汇聚与 `Ready` 展示；
- 权限、配额、审计和错误页面；
- 本地开发来源。

### 8.2 可以延期

- Marketplace 交易能力；
- Service 跨设备迁移；
- Shared attachment；
- 多 Realm 联邦；
- 强制独立 OS 进程 Front；
- Keel Native WASM Guest；
- 版本透明日志和可复现构建证明。

## 9. 成功定义

v1 达成的标志不是「能显示一个 Keel 页面」，而是以下闭环同时成立：

1. 用户安装经签名验证的程序。
2. 用户创建 Workspace-bound Service。
3. 地址栏原位显示新实例的地址。
4. Front 只能经声明契约调用该 Service。
5. 关闭、重启 Actrium 后仍以同一地址、同一 Service 身份恢复。
6. 同一精确版本离线运行时仍可验证。
7. Service 不在线、无权限、Realm 不可达和包不兼容时都有不同且可操作的错误。
8. Guest 越权、超时、内存超限或崩溃不导致其他 Tab 的业务状态损坏。
9. 任一图成员变为未知、未受控或保障挂起时，不得继续展示绿色，并限制相应数据流。
