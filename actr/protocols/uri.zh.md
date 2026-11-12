# Actr URI Scheme

语言：[English](uri.md) | **简体中文**

状态：初始草案；尚未向 IANA 注册。

本文定义 `actr:` URI 的语法、规范化方式和打开流程，计划作为 IANA `actr` URI scheme
登记项所引用的规范。

## 1. 目的

`actr:` URI 指向一个应用实例（Realm），或实例里的某个 Actor。第一段就是 Realm 名或
ActorId 在 `@` 左边的部分，后面可以跟页面路径：

```text
actr://acme.com/k7m2q9xh                    ; 实例 k7m2q9xh@acme.com
actr://acme.com/k7m2q9xh/today?view=week    ; 同一实例，带页面路径
actr://acme.com/2af01.k7m2q9xh              ; Actor 2af01.k7m2q9xh@acme.com
```

打开实例时，Actr Host 取得该 Realm 的 Realm Profile，按其中的包启动应用并连接该实例的
Service（第 4 节）。程序从哪里下载写在 Realm Profile 里，不写在 URI 里。

`actr:` URI 不是 signaling、WebSocket、WebRTC、STUN 或 TURN 的传输地址。

本文中的大写关键词 **MUST**、**MUST NOT**、**SHOULD**、**SHOULD NOT** 和 **MAY**
按 BCP 14 解释。

## 2. 语法

```abnf
actr-uri    = "actr://" domain [ "/" target [ path ] ] [ "?" query ] [ "#" fragment ]
target      = realm-id / serial "." realm-id
path        = *( "/" segment-nz )
```

`domain`、`realm-id` 与 `serial` 见第 5 节；`segment-nz`、`query`、`fragment` 引自 RFC 3986。

- `domain` 是 Realm 域名。它不含端口、userinfo，也不是 IP literal。
- 第一个 path segment 不含 `.` 时是 RealmId，`<realm_id>@<domain>` 即 Realm 名；含 `.` 时是
  `<serial>.<realm_id>`，加上 `@<domain>` 即 ActorId。RealmId 不含 `.`，因此两者不会混淆。
- 其余 path、query 和 fragment 原样交给应用，用于页面内定位。本协议不赋予它们任何含义。
- 只有 `domain`、没有 RealmId 的 URI 只表示该域名本身，不打开任何应用。
- 不允许空 segment 和 dot segment `.`、`..`。

URI **MUST NOT** 携带 credential、access token、私钥或其他秘密。

## 3. https 形式

Realm 链接也可以写成 https 地址，便于在任何聊天工具和网页中点击；设备上没有 Actr Host
时，同一地址打开的是普通网页。

Realm 域名在 authority descriptor（[Actr Authority Discovery](authority-discovery.zh.md)
第 4.6 节）的 `links` 中列出若干路径前缀。https 地址的 path 以其中某个前缀开头时，去掉该
前缀即得到对应的 `actr:` URI：

```text
links: ["/i/"]

https://acme.com/i/k7m2q9xh/today?view=week  ⇔  actr://acme.com/k7m2q9xh/today?view=week
```

- Actr Host **MUST** 先取得该域名的 authority descriptor，只按其中 `links` 的声明转换；
  **MUST NOT** 凭路径外观猜测。未声明的 https 地址就是普通网页。
- 去掉前缀后必须是合规的 `actr:` URI（第 2、5 节），否则该地址也只是普通网页。
- 声明了 `links` 的域名 **SHOULD** 在这些路径上提供网页，其中包含跳转到对应 `actr:` URI 的
  「在 Actrium 中打开」链接。
- 本机 Realm 没有 https 形式。

## 4. 打开流程

Actr Host 打开 `actr://<domain>/<realm_id>[…]`（实例）时：

1. 解析并规范化 URI（第 6 节）。
2. 本机 Realm（`domain` 以 `.actr.alt` 结尾）只使用本机记录，其中含同样的包信息，跳过
   Realm Profile；否则按
   [Actr Realm Profile](realm-profile.zh.md) 第 3、5 节取得并验证 `<realm_id>@<domain>` 的
   Realm Profile。
3. 在 `packages` 中解析 `primary_package` 和 `front_package`。任一引用无效即停止打开。
4. 按 [Realm Profile §6](realm-profile.zh.md#6-使用-realm) 验证两个包及 Front 的 Service 依赖和 RPC 契约，再将余下的 path、query 和 fragment 交给选定的 Front。
5. 经 Realm Profile 指定的 Actr Facility Provider 连接该实例的 Service：本 Realm 内 Actor 类型与 Service 包
   相同的 Actor 恰好一个；找到零个或多于一个都是错误，Actr Host 不做选择。连接需要 Realm
   membership；没有时进入加入流程（例如 Realm 邀请）。

打开 `actr://<domain>/<serial>.<realm_id>[…]`（某个 Actor）时流程相同，只是第 5 步直接连接该
ActorId，不按类型查找；它的 Actor 类型必须与 Service 包相同，否则以 `SERVICE_ACTOR_TYPE_MISMATCH`
失败。Actor 被替换（例如程序升级）后，这类链接失效；指向实例的链接不受影响。

持有 URI 不授予任何访问权。

## 5. Realm 名与 ActorId

### 5.1 Realm 名

```abnf
realm-name      = realm-id "@" domain
domain          = dns-name / installation-id ".actr.alt"
realm-id        = let-dig [ *61( let-dig / "-" ) let-dig ]   ; 字符规则同 DNS label
let-dig         = %x61-7A / DIGIT              ; 小写 a-z 与 0-9
installation-id = 26( %x61-7A / %x32-37 )      ; 小写 base32，a-z 与 2-7
```

- `dns-name` 使用小写 IDNA A-label，不含端口。
- `<installation-id>.actr.alt` 是某个 Actr Host 安装的伪域名，位于 RFC 9476 `.alt` 之下，
  永不经 DNS 解析，只在该安装上可用。`installation-id` 是安装时生成的 128 位随机数。
- RealmId（`realm-id`）为 1 到 63 个小写字母、数字或 `-`，不以 `-` 开头或结尾；在所属域名内
  唯一，创建后不可修改，退役后永不复用。Actr Host 默认随机生成，可读 RealmId 需显式选择。

示例：`k7m2q9xh@acme.com`、`notes@acme.com`、`h3x8p2wq@<installation-id>.actr.alt`。

Realm 名形似电子邮件地址，但不是。Actr Host **MUST NOT** 把它渲染为 `mailto:` 链接。

### 5.2 ActorId

```abnf
actor-id        = serial "." realm-name
serial          = "0" / non-zero-hex *12hex-digit / %x31-33 13hex-digit
hex-digit       = DIGIT / %x61-66            ; 小写十六进制
non-zero-hex    = %x31-39 / %x61-66          ; 1-9 或 a-f
```

例如 `2af01.k7m2q9xh@acme.com`：`serial` 由该 Realm 的 Issuer 分配，去掉
`<serial>.` 即得 Realm 名。其规范文本形式是 `0`，或不带前导零的小写十六进制，
无符号数值 **MUST** 不超过 `2^54 - 1`（`3fffffffffffff`）。因此，14 位 serial
必须以 `1`、`2` 或 `3` 开头，超过 14 位的 serial 无效。实现 **MUST** 拒绝大写
十六进制、前导零和超出范围的 serial，**MUST NOT** 将其改写为有效的 serial。

ActorId 不含程序类型，程序类型由 Actor credential 携带。ActorId 本身不是 URI，其 URI
形式见第 2 节。

Actor URI/ActorId serial 边界的共享测试向量见
[`actor-uri-serial.json`](../../contracts/actr/test-vectors/actor-uri-serial.json)。
serial 精确值（包括十进制预期值）均使用字符串，避免 JSON 消费方在超出 JavaScript
安全整数范围时丢失精度。

## 6. 规范化与比较

1. scheme 小写为 `actr`；
2. `domain` 转为小写 IDNA A-label；
3. percent-encoding 的十六进制数字大写，表示 unreserved 字符的编码解码；
4. Actor 的 `serial` MUST 使用第 5.2 节的规范文本形式，不得删除前导零、改变大小写或截断；
5. 拒绝空 segment 与 dot segment；
6. 只有 `domain` 时省略尾部 `/`。

规范化后逐字节相同的两个 URI 等价。RealmId 之后的 path、query 与 fragment 区分大小写。
解析与规范化的共享测试向量见
[`contracts/actr/test-vectors/uri.json`](../../contracts/actr/test-vectors/uri.json)。

## 7. 旧格式

以下格式不符合本规范。实现 **MAY** 只在显式迁移工具中识别它们，**MUST NOT** 当作合规 URI
静默接受：

- `actr://<realm>:<publisher>+<name>@<version>`（早期原型）；
- `actr://<publisher>/<program>@<version>[?service=<ActorId>]`（早期产品草案）；
- 以不透明 target 经 `packages` 服务解析入口包的形式（已退役）。

## 8. 安全与隐私考虑

- URI 会出现在日志、历史记录、剪贴板和消息中，因此不得包含秘密。
- Realm 链接会暴露 Realm 域名与 RealmId。Actr Host **SHOULD** 默认随机生成 RealmId，
  把可读名称放在 Realm Profile 的 `display_name` 中。
- https 形式只按 `links` 声明转换，防止任意网站路径被当成 Realm 链接。
- 显示时应突出 Realm 域名（它是信任根），并提防国际化域名的同形异义。

## 9. IANA 考虑

计划申请的 provisional 登记为：

```text
Scheme name: actr
Status: Provisional
Applications/protocols that use this scheme name: Actr
Contact: Actr Protocol Maintainers
Change controller: Actrium Project
References: this document
```

## 10. 参考资料

- [RFC 3986：Uniform Resource Identifier: Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986.html)
- [RFC 7595：Guidelines and Registration Procedures for URI Schemes](https://www.rfc-editor.org/rfc/rfc7595.html)
- [RFC 9476：The .alt Special-Use Top-Level Domain](https://www.rfc-editor.org/rfc/rfc9476.html)

## 11. 待完成工作

- 增加 https 转换的测试向量；
- 确立 IANA 登记使用的长期公开规范地址和项目联系人。
