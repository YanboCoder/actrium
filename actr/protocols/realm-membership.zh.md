# Actr Realm Membership

语言：[English](realm-membership.md) | **简体中文**

状态：初始草案。

本文定义一个 Realm 的成员如何凭一次性 Realm 邀请加入、如何为每个客户端登记 Realm 客户端 key、
如何管理成员，以及 Actr Host 申请 Actor credential 时如何证明 membership。

## 1. 范围与模型

Realm 与 Realm Profile 见 [Actr Realm Profile](realm-profile.zh.md)。本文只适用于 DNS 域名下的
Realm；本机 Realm 没有成员与邀请。

- **成员**：Actr Facility Provider 为某个 Realm 记录的一方，有 `member_id` 与一个角色。
- **角色**：`owner`、`editor`、`viewer`。每个 Realm 恰好一个 owner：它同时持有该 Realm 的成员管理权与
  应用内的 owner 角色。`editor`、`viewer` 的含义由应用决定；Actr Facility Provider 只记录并写入 Actor
  credential，不执行应用权限。
- **Realm 客户端 key**：成员的某个客户端为该 Realm 生成的 Ed25519 key。一个成员可有多把（多台设备、
  多个安装），每把可单独撤销；**MUST NOT** 跨 Realm 复用。
- **Realm membership**：成员为 `active`，且请求所用的 Realm 客户端 key 已登记、未撤销。没有单独的
  membership credential。
- **Realm 邀请**：一次性 secret，兑换一次后失效。它或者带一个角色（邀请新成员），或者指向一个已有
  成员（为其添加客户端）。

v1 不定义 Actr Facility Provider 账号、多个 owner 或域名级 SSO。

本文中的大写关键词 **MUST**、**MUST NOT**、**SHOULD**、**SHOULD NOT** 和 **MAY** 按 BCP 14 解释。

## 2. 标识

- Realm 客户端 key 的 `key_id`：Ed25519 公钥（32 字节）SHA-256 的前 16 个小写十六进制字符。
- 邀请 `secret`：32 字节随机数，以 base64url（无填充）表示。邀请的 `invite_id`：secret 字节的 SHA-256 的前
  16 个小写十六进制字符。Actr Facility Provider 只保存 secret 的 SHA-256。
- `member_id`：Actr Facility Provider 分配的 16 个小写十六进制字符，在该 Realm 内不复用。

## 3. Realm 邀请

邀请是 JSON 对象：

```json
{
  "schema": "actr.realm-invite/v1",
  "realm": "k7m2q9xh@acme.com",
  "secret": "<base64url，43 字符>",
  "profile": { "...": "可选，内嵌的 Realm Profile" }
}
```

文本形式为 `ri_` 后接该 JSON 的 UTF-8 字节的 base64url（无填充）编码，可放进消息、邮件、二维码或文件。

- 邀请只经带外渠道交付。它 **MUST NOT** 出现在 `actr://` URI、Realm 链接的 https 形式、query 或
  fragment 中。
- 内嵌的 `profile` 按 Realm Profile 第 5 节验证，与经 HTTPS 取得的相同，其 `realm.name` **MUST** 等于
  `realm`；Actr Host 仍 **SHOULD** 从 `realms` 位置刷新。
- 持有邀请即可兑换，接收方 **SHOULD** 尽快兑换；签发方可在兑换前撤销。

## 4. Membership 请求

Actr Host 以 HTTPS `POST <issuer.url>/membership` 发送 membership 请求（protocol
`actr-realm-membership/1`），`issuer.url` 取自该 Realm 已接受的 Realm Profile。请求体是
[Actr Realm Profile](realm-profile.zh.md) 第 8 节的 Actr 管理信封：`op`、`realm`、`provider_id`、`nonce`、
`issued_at`、`expires_at` 与各操作的字段，签名 key 是 Realm 客户端 key，`signature.key_id` 为其 `key_id`。
信封的规范化、有效期（不超过五分钟）、`provider_id` 绑定与 nonce 规则不变；membership 请求不带 `requester`。

| `op` | 字段 | 谁可以发 | 效果 |
| --- | --- | --- | --- |
| `join` | `invite`（secret）、`public_key`、可选 `label` | 持有邀请者，用新 key 签名 | 兑换邀请，登记该 key；响应 `{realm, member_id, role, key_id}` |
| `create_invite` | `role`（`editor`/`viewer`）或 `member_id`，加 `invite_expires_at` | 新成员邀请：owner；客户端邀请：owner，或成员为自己 | 响应 `{invite_id, invite, expires_at}`，`invite` 为文本形式，只返回这一次 |
| `revoke_invite` | `invite_id` | owner，或该邀请的签发者 | 未用的邀请失效 |
| `list_members` | 无 | 任何成员 | owner 得到全部成员、客户端与未用邀请；其他成员只得到自己的记录、客户端与自己签发的邀请 |
| `revoke_client` | `key_id` | 成员撤销自己的客户端；owner 撤销任何客户端 | 该 key 撤销（第 6 节），它签发的未用邀请一并撤销 |
| `remove_member` | `member_id` | owner（不能是自己） | 成员移除，其全部客户端 key 撤销 |
| `set_role` | `member_id`、`role`（`editor`/`viewer`） | owner（不能是自己） | 只改角色 |
| `transfer_owner` | `member_id`、可选 `previous_owner_role`（缺省 `editor`） | owner | 原子地把 owner 交给另一 active 且至少有一把未撤销客户端 key 的成员，原 owner 改为指定角色 |
| `leave` | 无 | 非 owner 成员 | 移除自己，效果同 `remove_member` |
| `update_realm` | 可选 `package`、`display_name`，或 `status: "retired"` | owner；只用于 Actr Facility Provider 域名下的 Realm | Actr Facility Provider 签署并发布 sequence 更高的 Realm Profile（Realm Profile 第 7.2、7.4 节）；响应 `{realm, sequence}` |

`join` 请求中 `signature.key_id` **MUST** 等于由 `public_key` 算出的 `key_id`，签名即持有证明。其他请求的签名
key **MUST** 是该 Realm 已登记、未撤销、属于 `active` 成员的 Realm 客户端 key。

Actr Facility Provider 先检查角色，再查找目标：无权执行的请求一律 `forbidden`，不以 `not_found` 透露目标是否存在。
其他 Realm 的更新与退役仍由 Realm 签名 key 持有方经 Realm 管理请求完成，对它们发 `update_realm` 得到 `forbidden`。

`label` 是客户端名称，至多 64 个字符，供成员辨认自己的客户端。`invite_expires_at` 为 RFC 3339 时间，不超过
签发时刻后 30 天。

### 4.1 兑换规则

Actr Facility Provider 在单个事务中兑换邀请：记录邀请已用，按邀请建立新成员或定位目标成员，登记 key。两个并发兑换
最多一个成功；超过限额（`quota`）时邀请不被消耗。同一把 key 重复兑换同一邀请（响应丢失后的重试），只要
该 key 仍未撤销、成员仍为 `active`，就返回首次的结果，否则返回 `invite_used`。

- 带 `role` 的邀请建立新成员。`owner` 角色的邀请（owner 邀请）只由 Realm 签名 key 持有方或 Actr Facility Provider
  运营者签发（第 5 节）：Realm 尚无 owner 时建立 owner；已有 owner 时，新 key 成为该 owner 成员的客户端，
  owner 原有的全部客户端 key 撤销。
- 带 `member_id` 的邀请为该成员登记新客户端，目标成员 **MUST** 仍为 `active`。

### 4.2 owner 保护

- owner **MUST NOT** 撤销自己最后一把未撤销的客户端 key，也不能移除自己或 `leave`；须先 `transfer_owner`。
- `owner` 角色只经 owner 邀请或 `transfer_owner` 产生；`create_invite` 与 `set_role` 不接受 `owner`。

### 4.3 错误

Actr Facility Provider 以 JSON `{"error": "<code>", "message": "<text>"}` 返回错误，`code` 稳定、可供 Actr Host 分支：

| HTTP | `code` | 含义 |
| --- | --- | --- |
| 400 | `invalid_request` | 格式、`provider_id`、有效期、nonce 重复，或 `key_id` 与 `public_key` 不符 |
| 403 | `client_unknown` | 签名 key 未在该 Realm 登记、签名无效，或 Realm 不存在 |
| 403 | `client_revoked` | 签名 key 已撤销 |
| 403 | `member_removed` | 成员已被移除或已离开 |
| 403 | `forbidden` | 该角色不能执行此操作 |
| 403 | `realm_unavailable` | Realm 不是 `active` |
| 404 | `invite_unknown` | 没有与该 secret 对应的邀请 |
| 404 | `not_found` | 目标成员、客户端或邀请不存在 |
| 409 | `invite_realm_mismatch` | 邀请属于另一个 Realm |
| 409 | `owner_last_client`、`owner_must_transfer`、`conflict` | 违反 owner 保护、key 已登记或目标状态不符 |
| 410 | `invite_expired`、`invite_used`、`invite_revoked` | 邀请已过期、已被使用、已被撤销 |
| 429 | `quota` | 超过成员、客户端或未用邀请数量限制 |

`realm_unavailable` 只在请求方已证明持有登记的 key 或有效邀请之后返回；否则一律 `client_unknown` 或
`invite_unknown`，不透露 Realm 状态。邀请过期、已用、已撤销与 Realm 不匹配 **MUST** 是不同错误。

## 5. owner 邀请的签发

owner 邀请是 `role = owner` 的邀请，有效期 7 天。签发新的 owner 邀请时，该 Realm 此前未用的 owner 邀请失效。

- **其他域名的 Realm。** Realm 签名 key 持有方首次以 `publish_profile` 激活 Realm 时，响应一次性携带
  `owner_invite`（文本形式）；此后以 `reissue_owner_invite`（Realm Profile 第 8 节）签发新的，用于 owner 丢失
  全部客户端时的恢复。
- **Actr Facility Provider 域名下的 Realm。** v1 只由 Actr Facility Provider 运营者创建，创建时得到 owner 邀请，经带外渠道交给
  用户；恢复同样由运营者签发。签发接口不属于本协议。此后 owner 以 `update_realm` 更新或退役该 Realm。

## 6. Actor credential 与 membership

Actr Host 向 Issuer 申请、重签或续期 Actor credential 时，**MUST** 用该 Actor 所在 Realm 的 Realm 客户端 key
证明持有。证明放在 HTTP 请求头：

```text
x-actr-client-proof: <base64url（无填充）的 UTF-8 JSON>
```

JSON 是 Actr 管理信封，`op` 为 `register`、`reissue` 或 `renew`（对应注册、重签与续期入口），另加
`body_sha256`：HTTP 请求体字节的 SHA-256，小写十六进制。`realm` **MUST** 等于请求体中的 Realm。Issuer 核对
`body_sha256`、签名、有效期、`provider_id` 与 nonce。同一请求体的幂等重试每次都带新签的证明。

Issuer 只在以下条件都满足时签发：

1. 签名 key 已在该 Realm 登记、未撤销，所属成员为 `active`；
2. 续期时，签名 key 是该 Actor 当前绑定的 key；重签时，签名 key 属于该 Actor 所属的成员，重签成功后 Actor
   改绑到这把 key（成员可凭 Actor 私钥把自己的 Actor 移到自己的另一个客户端）；
3. 注册时，若 Actor 类型的 publisher 与程序名等于 Realm Profile `package.type` 的（不论版本，即该实例的
   Service 及其升级版本），签名 key 属于 owner。续期与重签不再检查角色，owner 转让不影响运行中的 Service。

注册携带的连接 ACL 只作用于所注册 Actor 自己的类型；Service 的程序只有 owner 能注册，其他成员因此不能改写
Service 的 ACL。

Actor credential 的 claims 包含成员 id、Realm 客户端 key id 与签发时的角色。Service **MAY** 据此判断 Front 的
应用权限；它 **MUST** 以 Front credential 的到期时间为该判断的有效期，Front 续期后出示新 credential 时再读取
角色。

撤销 Realm 客户端 key（包括移除成员、`leave` 与 owner 恢复）时，Actr Facility Provider **MUST** 立即：

- 拒绝该 key 的新请求，包括签发 Actor credential；撤销它签发的未用邀请；
- 停用当前绑定到该 key 的全部 Actor：拒绝它们续期，关闭它们的 signaling 连接与 TURN allocation，拒绝它们
  重连、取得 TURN credential。

ActorId 不因此作废：同一 `active` 成员可用自己另一把未撤销的 key 重签（上文第 2 条），Actor 随之恢复。被移除
或离开的成员没有可用的 key，其 Actor 永久停用。已签发的 Actor credential 在 Actr Facility Provider 之外（例如对端
Actor 的验证）到期失效。改角色不撤销任何 key，
在该成员下一份 Actor credential 生效。

Realm 客户端 key 是 Actr Host 的能力，**MUST NOT** 交给 Guest workload。

## 7. 安全与隐私考虑

- 每个 Realm 使用独立的 Realm 客户端 key，不同 Realm 无法据 key 关联同一客户端；`label` 由客户端自定，
  Actr Host **SHOULD** 默认使用不含个人信息的名称。
- 邀请是 bearer secret：泄露的未用邀请可被他人兑换。签发方 **SHOULD** 选短有效期，发现泄露时撤销；
  owner 在 `list_members` 中能看到每个成员及其客户端，可移除误入者。工具 **SHOULD NOT** 让邀请出现在
  其他本机用户可见之处，例如命令行参数。
- Realm 客户端 key 的私钥只在该客户端保存。Actr Host **SHOULD** 拒绝加载本机其他用户可读的 key 文件。
- owner 邀请的持有方与运营者能恢复 owner，因此它们处于 Realm 的信任边界内，这与它们本就能签发 Realm Profile、
  停用 Realm（Actr Facility Provider 还持有 Realm issuer key）一致。
- owner 恢复撤销 owner 原有的全部客户端 key 与它们签发的未用邀请，被盗的 owner key 因此无法与恢复者反复争夺。
  被盗期间对方可能已邀请成员或转让 owner；恢复后 owner **SHOULD** 在 `list_members` 中核对并移除。
- Actor credential 有效期是撤销在 Actr Facility Provider 之外生效的上限；Actr Facility Provider **SHOULD** 保持较短的
  credential 有效期。

## 8. 测试向量与 Schema

- `contracts/actr/test-vectors/realm-membership.json`：`key_id`、`invite_id`、邀请文本编解码、`join` /
  `create_invite` / `transfer_owner` 信封与签名、`x-actr-client-proof`（含 `body_sha256`）及负例；
- `contracts/actr/realm-membership.schema.json`：邀请、请求、响应与客户端证明的 JSON Schema。

## 9. 参考资料

- [Actr Realm Profile](realm-profile.zh.md)
- [Actr URI](uri.zh.md)
- [RFC 3339：Date and Time on the Internet: Timestamps](https://www.rfc-editor.org/rfc/rfc3339.html)
- [RFC 4648：The Base16, Base32, and Base64 Data Encodings](https://www.rfc-editor.org/rfc/rfc4648.html)
- [RFC 8032：Edwards-Curve Digital Signature Algorithm (EdDSA)](https://www.rfc-editor.org/rfc/rfc8032.html)
- [RFC 8785：JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html)
