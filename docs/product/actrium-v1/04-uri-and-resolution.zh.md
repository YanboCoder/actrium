# 04. Realm 链接与地址栏

## 1. 目标

一个应用实例就是一个 Realm，它的地址就是 Realm 链接。本章只讲 Actrium 怎样显示、打开、保存和
分享这个地址；链接语法与打开流程以 [Actr URI](../../../actr/protocols/uri.zh.md) 为准，这里不重复。

- 地址栏、历史、收藏和分享里出现的都是实例地址，打开它就回到同一个实例；
- 地址里没有秘密，知道地址不等于有权进入；
- 程序从哪里下载由 Realm Profile 决定，不写在地址里。

包选择无效时以 `REALM_PROFILE_INVALID` 停止打开。见 [Realm Profile §4.2](../../../actr/protocols/realm-profile.zh.md#42-包选择校验)。

## 2. 链接形式

```text
actr://acme.com/k7m2q9xh
actr://acme.com/k7m2q9xh/today?view=week#item-3
```

- `acme.com` 是 Realm 域名，也是用户看到的信任根；`k7m2q9xh` 是 RealmId；
- RealmId 之后的路径、query 和 fragment 原样交给应用，用于页面内定位，不影响打开哪个实例；
- 第一段写成 `<serial>.<realm_id>`（如 `actr://acme.com/2af01.k7m2q9xh`）时指向该实例里的某个 Actor：打开流程相同，只是直接连接这个 Actor 而不按类型查找，它的类型必须与包相同；程序升级替换 Actor 后这类链接失效，所以分享时默认用实例链接；
- Realm 链接的输入与分享只支持 `actr://`。`https://` 地址以 `URI_SCHEME_INVALID` 拒绝，Actrium 不通过 `links` 转换；
- 本机 Realm 的地址形如 `actr://<installation-id>.actr.alt/h3x8p2wq`，只在本安装有效。

## 3. 打开

Actrium 验证已签名的 Realm Profile，从 `packages` 解析 `primary_package` 和 `front_package`，分别验包。核对 Front 的 Service 依赖和 RPC 契约后，创建 Front Actor 并 Attach 到实例的 Service。

产品层面的要求：

- **先确认，后联网**：来自网页、二维码、邮件或其他 App 的链接，先显示 Realm 域名、
  `display_name`，下载验证后再显示程序（Publisher、程序名、版本）；首次信任某个 Realm 域名
  必须确认。Realm 域名等于或属于 Actr Facility Provider ID 时，信任根显示为该 Actr Facility Provider。
- **失败即停。** Profile 不可用、无效或已退役，或任一包验证失败时，停止打开。
- **没有成员身份时**：进入加入流程（例如 Realm 邀请），不静默失败。
- 用户可以选择「在浏览器中打开」外部 https 链接；在同一域名的网页内跳转时，Actrium 不强制接管。
- 链接不能预授任何权限；未知的 query 只交给应用，不影响安装或权限页面。

## 4. 地址栏、历史、收藏与复制链接

- 地址栏显示规范化后的 `actr://` 地址（规范化规则见 Actr URI 第 6 节），并突出 Realm 域名；
- 新建实例成功后，地址栏用 replace（而不是 push）换成新实例的地址：后退不会回到新建确认页，
  刷新会连接新实例，历史里只有最终地址；
- 收藏的是实例，不是程序；
- 「复制链接」：复制规范化的 `actr://` 地址；
- 本机 Realm 禁用「复制链接」，不能伪造域名。

新建实例本身不是链接，而是 Actrium 内的本地操作，见[用户体验](03-user-experience.zh.md)第 5 节。

## 5. 开发来源

开发模式下，新建实例可以使用 localhost、回环地址或显式信任的私有 CA 上的包地址。这类实例：

- 始终显示「开发来源」；
- 不进入已验证的 Program Catalog，不被正式 Marketplace 接受；
- 导出或分享前给出风险提示。

正式地址不支持 HTTP 回退。

## 6. 旧格式

以下格式一律拒绝，只能通过显式迁移工具导入：

- `actr://<publisher>/<program>@<version>[?service=<ActorId>]`（早期产品草案的 Create/Attach URI）；
- 只含数值 `realm_id` 的旧 ActorId；
- `actr://<realm>:<publisher>+<name>@<version>`（早期原型）。

## 7. 错误码

完整清单见 [12 §10](12-reference-spec.zh.md)。与链接相关的有：

| 代码 | 含义 |
|---|---|
| `URI_SCHEME_INVALID` | 不是受支持的 scheme |
| `URI_DOMAIN_INVALID` | Realm 域名不合法（含端口、IP literal 等） |
| `URI_LABEL_INVALID` | RealmId 不合法 |
| `URI_LEGACY_FORM` | 旧格式，需要迁移工具导入 |
| `URI_LOCAL_REALM_FOREIGN` | 本机 Realm 地址不属于本安装 |
| `REALM_KEYS_INVALID` | 域名的 `realm_signing_keys` 缺失、签名或轮换链无效，或 `sequence` 回退 |
| `REALM_PROFILE_UNAVAILABLE` | 取不到有效 Realm Profile |
| `REALM_PROFILE_INVALID` | Realm Profile 签名无效或 Realm 名不符 |
| `REALM_PROFILE_ROLLBACK` | Realm Profile 的 `sequence` 回退 |
| `ACTR_FACILITY_PROVIDER_MISMATCH` | `provider.id` 与已接受的不同 |
| `REALM_RETIRED` | Realm 已退役 |
| `PACKAGE_DIGEST_MISMATCH` | 下载的归档与其包角色所固定的摘要不符 |
| `PACKAGE_SIGNATURE_INVALID` | 包签名验证失败 |
| `SERVICE_NOT_FOUND` | Realm 内匹配固定 Service 包类型的 Actor 为零个或多个 |
| `SERVICE_ACTOR_TYPE_MISMATCH` | 链接所指 Actor 的类型与固定的 Service 包类型不符 |

错误码供日志、测试和开发者工具使用；给用户的文案必须说明可以怎么做。
