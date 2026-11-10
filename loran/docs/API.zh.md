# Loran Realm v1 API 边界

状态：本文只描述当前实现，不承诺旧 Issuer Signer/Publisher Registry HTTP API 兼容。一个实例
承载多个 Realm（Realm 名 `<realm_id>@<domain>`）；尚未实现的部分见
[`REALM_V1_ARCHITECTURE.zh.md`](REALM_V1_ARCHITECTURE.zh.md) 第 9 节。历史名称见
[术语表](../../docs/GLOSSARY.zh.md)「历史名称对照」。

## 传输与认证规则

- 生产实例只接受 HTTPS/WSS；端口可以配置，但 endpoint/audience 必须进入凭证作用域。
- 凭证不得放在 URL、query string、普通访问日志或错误文本中。
- 原生 Signaling WebSocket 使用 `x-actr-id` 与
  `Authorization: ActrCredential <base64url-protobuf>` upgrade headers。浏览器不能设置这
  两个 header，因此改用 `actr.v1`、`actr.id.<base64url>` 和
  `actr.credential.<base64url-protobuf>` 子协议项；服务器验证全部身份后只协商返回
  `actr.v1`，避免把凭证暴露为页面可读的已选子协议。
- Publisher 由 Publisher Profile 签名 key 持有方以 `register_publisher` 登记，Publisher
  管理（停用、撤销 key）和配置变更只允许系统管理员调用。其他域名的 Realm 由 Realm 签名
  key 持有方以 Realm 管理请求（`create_realm`、`publish_profile`、`reissue_owner_invite`）创建、
  更新与退役，`POST` 到 Loran authority descriptor 中的 `realm-management` service link（Actr
  Realm Profile 第 8 节，见下文「Realm 管理请求」）；Actr Facility Provider 域名下的 Realm 由系统管理员
  创建，其 owner 以 membership 请求 `update_realm` 更新或退役。
- Realm 成员凭一次性 Realm 邀请加入，每个客户端登记一把 Realm 客户端 key（协议
  `actr/protocols/realm-membership`）。成员管理是 `POST /issuer/membership`（见下文「Realm
  membership」）；Actor credential 的注册、重签与续期带 `x-actr-client-proof`。没有共享的
  Realm secret，也没有 Realm 管理 token。
- Issuer Signer 是内部信任平面；规范入口是同一 HTTP/2 监听器上的 gRPC，
  不是公开 REST 密钥下载服务。

## Realm discovery

Loran 不再提供 `/.well-known/actr/realm.json`：它与 `actr/protocols/authority-discovery`
「`/.well-known/actr` 下不定义子路径」冲突，也无法表达多个 Realm。Realm 的发现走该域名的
authority descriptor（`https://<domain>/.well-known/actr` 中的 `realm_signing_keys` 成员与
`realms` relation，Actr Authority Discovery §4.3–4.4），Realm Profile 位于
`<realms href>/<realm_id>`，由 Realm 签名 key 签名。Loran 仅在 `realms` relation 指向本 Actr Facility
Provider 时提供 Realm Profile。

`[authority] enabled = true` 时 Loran 只为自己的 Actr Facility Provider 域名提供一份
`/.well-known/actr`（支持 `GET`/`HEAD`，带 `Access-Control-Allow-Origin: *` 与
`Cache-Control`）。启用 Issuer 且有 Signer 时，descriptor 额外公布：

- `realms` relation：`https://<provider>/actr/realms/<provider>`（protocol `actr-realm-profile/1`）；
- `realm-management` relation：`https://<provider>/actr/realm-management`（protocol
  `actr-realm-management/1`）；
- `realm_signing_keys`：Signer 中名为 `realm-signing:<provider>` 的 Ed25519 key（启动时缺失即
  生成、不过期），`sequence = 1`，由该 key 对去掉 `signature` 的成员按 JCS 自签名；descriptor 的
  key id 与之相同。

Realm 签名 key 尚未就绪时 descriptor 返回 `503`。`[authority] links` 配置 `links` 成员。

`GET /actr/realms/{domain}/{realm_id}`（`HEAD` 同）原样返回 Realm `<realm_id>@<domain>` 已接受的
Realm Profile（`application/json`，带 `Access-Control-Allow-Origin: *` 与
`Cache-Control: public, max-age=300`）。`active` 与 `retired` Realm 都提供；未知、`pending`
或尚无 Realm Profile 的 Realm 返回 `404`。Actr Facility Provider 域名下的 Realm 由 Loran 用
`realm-signing:<provider>` 签发 Realm Profile：`issuer.url = <origin>/issuer`，`issuer.keys`
为该 Realm 当前列出的 issuer key（`id` 为 Signer 数字 key id 的十进制，即 credential `key_id`；
首把是 Signer 中的 `issuer:<realm>`，轮换见下文「Admin」），`provider.signaling` 的
`control_plane_id = <provider>/v1`、`ticket_audience = actr.signaling`，TURN 条目的
`credential_audience = actr.turn`，`policy.membership = invite-only`；每份都先按 Actr Host
的规则（`RealmProfile::verify` 与 `check_successor`）验证再保存。

其他域名的 Realm 若有已接受的 Realm Profile，Loran 每 `[realm] profile_refresh_secs`（默认
3600，取值限定在 60–86400）刷新一次：经逐跳 SSRF 检查的抓取器（仅 HTTPS、最多 5 次重定向、
拒绝非公网地址并钉住解析结果、64 KiB、结果缓存 5 分钟、含失败）取该域名的 authority
descriptor，按 key pin 接受 `realm_signing_keys`（Actr Authority Discovery §4.4：成员自签名，
首次接受即 pin；此后同 id 不换公钥、`retired` 不复活、已移除不复现、`sequence` 不回退，新 key
只经 `sequence` 更高的有效轮换项引入；签名 key 链接不到已 pin 的 key 即身份变化，拒绝，直到运营者重新确认，
见下文「身份变化的重新确认」），再从其 `realms`
位置取更高 sequence 的 Realm Profile；退役 Realm Profile 触发退役事务。已接受 Realm Profile
的签名 key 被移除时丢弃该基线，改用由仍列出的 key 签名的 Realm Profile，取不到则停用该 Realm。

## Realm 管理请求

`POST /actr/realm-management`（Loran 作为 Actr Facility Provider 时提供）接受 Actr 管理信封：JSON 对象，
含 `op`、`realm`、`provider_id`、`nonce`、`issued_at`、`expires_at`（RFC 3339）、可选
`requester` 与各操作的字段，加上 `signature`（`{"key_id", "algorithm": "Ed25519", "value"}`，
对去掉 `signature` 的请求按 JCS 规范化后的 detached 签名，与 Realm Profile 相同）：

- `create_realm`：带 `package`（`href` 须为 HTTPS，`sha256` 为 64 位小写十六进制，`type` 为包
  manifest 的 Actor 类型 `publisher/program@version`，publisher 须为域名）。建为
  `pending`，在 Signer 中生成该 Realm 的首把 issuer key `issuer:<realm>`，响应
  `{"realm", "status": "pending", "pending_expires_at", "profile"}`，`profile` 是 `sequence = 1`、
  指明本 Actr Facility Provider 的 Issuer、issuer key 与端点的未签名首份 Realm Profile。以
  `(realm, requester)` 幂等（`requester` 缺省为空）：同一请求方重试 `pending` Realm 再次返回
  `profile`，对 `active` Realm 只返回 `{"realm", "status": "active"}`；其他请求方或已退役 Realm
  返回 `409`。
- `publish_profile`：`profile` 为持有方签名的 Realm Profile。须能以该域名当前的
  `realm_signing_keys` 验证、`provider.id` 为本 Actr Facility Provider；`active` Realm Profile 的
  `issuer` 与 `provider` 须与 Loran 生成的完全一致。首次提交激活 `pending` Realm，响应一次性带
  `owner_invite`（`ri_…`）与 `owner_invite_expires_at`；之后只接受通过 `check_successor` 的更高
  sequence（重复提交返回 `409`），不再返回邀请；`status = retired` 的 Realm Profile 触发退役事务。
- `reissue_owner_invite`：为 `active` Realm 签发新的 owner 邀请（`{"realm", "owner_invite",
  "expires_at"}`，有效 7 天），此前未用的 owner 邀请立即失效；用于 owner 丢失全部客户端时的恢复。
- `draft_profile`：`active` 且有已接受 Realm Profile 的 Realm，返回 `{"realm", "profile"}`：
  下一份未签名 Realm Profile（`sequence` 为已接受值加一，`display_name`、`package`、`policy`
  取自已接受的 Realm Profile，`issuer` 与 `provider` 为 Loran 当前值，含轮换中的 issuer key）。
  不改变 Realm；持有方签名后以 `publish_profile` 提交。
- `register_publisher`（Actr Authority Discovery §4.5）：以 `publisher`（规范化 Publisher 域名）
  取代 `realm`，由该域名 Publisher Profile 中 `active` 的 Publisher Profile 签名 key 签名。见下文
  「Publisher Registry」。

校验顺序：请求格式、`provider_id` 等于本 Actr Facility Provider ID、有效期不超过 5 分钟且包含当前时间、
`nonce` 为 1–128 字符（`register_publisher` 之后的步骤见「Publisher Registry」）、Realm 名规范且不在 Actr Facility Provider 域名（及其子域名）下（否则 `403`，
这些 Realm 由运营者创建）；然后经 Realm discovery 中的抓取器取
`https://<domain>/.well-known/actr`，按 key pin 接受其 `realm_signing_keys`，要求签名 key 为其中
`active` key；最后才记录 `nonce`（有效期内重复即 `400`）。请求按来源地址限流，抓取按目标主机
限流并缓存 5 分钟（含失败）。`create_realm` 受 `[realm]` 配额约束：`max_realms`、
`max_realms_per_domain`、`max_pending_per_domain`（超出返回 `429`）；`pending` Realm 在
`pending_ttl_secs` 后清除，名字可再用。

错误以 `{"error": "…"}` 返回：`400` 请求无效，`403` key 或签名不被接受，`404` Realm 不存在，
`409` 冲突（幂等键、回滚、已退役），`429` 配额，`503` 取不到 descriptor 或 Signer 不可用。
响应带 `Cache-Control: no-store`。持有方工具：`actr realm keygen | create-request |
sign-profile | retire | reissue-request | draft-request`；Publisher 用 `actr realm register-publisher`
以 Publisher Profile 签名 key（`keygen` 格式的 key 文件）签署 `register_publisher` 并发往
`https://<provider_id>/.well-known/actr` 所列的 `realm-management` 端点（`--endpoint` 可指定，
`--print` 只打印不发送）。

## Issuer

路由前缀：`/issuer`。

注册、续签与 signing-key 查询使用 protobuf body。`/issuer/register`、`/issuer/reissue` 与
`/issuer/renew` 都必须带请求头 `x-actr-client-proof`：base64url（无填充）的 Actr 管理信封，`op`
为 `register`/`reissue`/`renew`（须与路径一致），`realm` 等于请求体中的 Realm，`body_sha256` 为
请求体字节的 SHA-256，由一把 Realm 客户端 key 签名（Actr Realm Membership 第 6 节）。
`provider_id` 为本 Actr Facility Provider ID；Realm 没有 Realm Profile（开发与 e2e）时也接受请求的 `Host`
（小写 `host[:port]`）。nonce 与 Realm 管理请求共用一张表。Realm 客户端 key 是受信 Actr Host 的
能力，不得下发给 Rill Guest。

证明先于一切 Realm 信息检查：缺失或格式、`op`、`realm`、`body_sha256`、`provider_id`、有效期、
nonce 不对返回 `400`（`invalid_request: …`）；key 未在该 Realm 登记（含 Realm 不存在）或签名不对
返回 `403`（`client_unknown: …`）；key 已撤销为 `403`（`client_revoked: …`），成员已移除为 `403`
（`member_removed: …`）。之后才报告 Realm 不可用的原因。错误以 protobuf `ErrorResponse` 返回，
`message` 以上述稳定错误码开头。其余规则：

- 注册记下 Actor 所属的成员与 Realm 客户端 key；Actor 类型的 publisher 与程序名等于 Realm
  Profile `package.type` 的（不论版本）时只接受 owner 的 key（`403 service_owner_only: …`）；
- 续期须用该 Actor 当前绑定的 key，重签须用同一成员的某把 key，成功后 Actor 改绑到这把 key，否则
  `403 client_unknown: …`；
- 提交事务内重查该 key 未撤销、成员 active；撤销先提交则签发失败；
- key 的 `last_used_at` 至多每分钟更新一次。

签发的 Actor credential claims 固定包含：

- `protocol_version = 1`
- `issuer`：Realm 名，例如 `k7m2q9xh@acme.com`
- `provider`：Actr Facility Provider ID，即 `[authority] uri` 的规范化 DNS 域名，例如
  `provider.example`
- `audience = actr.signaling`
- `realm`（Realm 名，与 `issuer` 相同）、完整 ActorId 与过期时间
- `actor_type`：注册时声明并经验证的 Actor 类型 `publisher/program@version`
- `member`、`client_key`、`role`：成员 id、所用 Realm 客户端 key id 与签发时的角色（每次签发按当时
  记录写入）

Actor credential 由该 Realm 的当前 issuer key（Realm Profile `issuer.keys` 中 `id` 为其十进制的
key）签名，`key_id` 与注册响应的 `signing_key_id` 为该 key 的 Signer key id；
Signaling 与 TURN 按 `(realm, key_id)` 查验证 key，轮换后退役 key 的缓存项在其可移除时刻失效。Issuer 没有全局签名 key；`GET /issuer/health` 报告
数据库与 Signer 是否可用（`database`、`signer_service`）以及已加载的 Realm issuer key 数
（`realm_keys`）。

TURN credential 的 password 为 `base64(HMAC-SHA1(k, username))`，`k` 为该 Realm 的 TURN
secret：`HKDF-SHA256(turn_secret, salt = "actr.turn", info = "<realm>|<provider>")`。TURN 拒绝
不可用（pending、停用、过期或已退役）Realm 与停用的 Actor（已撤销，或绑定的 Realm 客户端 key 已
撤销）的 credential，缓存命中也不例外：变更即时生效，过期最迟 10 秒后生效；它们已建立的 allocation
同时被关闭（按 TURN username 中的 Realm 名与 ActorId）。TURN credential 只随 Actor credential 由
Issuer 签发；`/actr/ice-config` 只返回 ICE URL，不签发 credential。

`/issuer/renew` 与 `/issuer/reissue` 带客户端生成的 16 字节 `attempt_id`（UUID），网络失败时
客户端用完全相同的请求体重试。同一 attempt 的第一个请求占用它，身份状态变化（renewal token
轮换；reissue 消耗持有证明与 publisher nonce）与响应在同一事务提交，响应加密后存于
`issuer_control_attempt`。之后用同一 `attempt_id`：

- 请求体相同：返回首次的响应原文（成功响应保留到 credential 或 renewal token 到期，拒绝保留
  5 分钟）；重放成功响应前仍重查 Actor 撤销与 Realm、包、publisher 状态，已不允许则返回该错误；
- 请求体不同：`409`；
- 超过响应保留期：`410`，客户端应以新 `attempt_id` 重来；
- 另一个请求仍在处理：`425`，带 `Retry-After`（秒）；
- `attempt_id` 不是 16 字节：`400`。

这些以 HTTP 状态返回；`/reissue` 的其他结果仍放在 `RegisterResponse` body 中、HTTP 200，只有提交时
才发现的失败例外：另一个 attempt 先消耗了同一持有证明（`422`）或 publisher nonce（`403`），或存储
出错（`500`）；这时 attempt 被释放，同一 attempt 可以重试。
`/reissue` 与 `/renew` 先校验客户端证明再看 `attempt_id`；重放已存响应前还检查证明的 key 与 Actor
的绑定（续期为同一把 key，重签为同一成员）。一个持有证明只能成功 reissue 一次：此后用它
发起的新 attempt 返回 `422`（持有证明不成立），丢失响应只能靠同一 attempt 重试取回。

`/renew` 先只读校验 renewal token（无效或过期返回 `401`），通过后才报告 Realm 不可用的原因，
未持有有效 token 的调用方探知不到 Realm 是否停用或退役。renewal token 的后继已存在（它已被
`/renew` 轮换或被 `/reissue` 消耗）时，`/renew` 只在后继生成后 5 分钟内接受它，且只返回这个
后继及其到期时间；此后返回 `401`。`/renew` 与 `/reissue` 在提交事务内重查 Actor 撤销、Realm 客户端 key 撤销与成员移除、Realm
状态、包撤销与 publisher 停用，首次检查之后落地的变更同样阻止签发：此时 attempt 被释放，
按该错误返回（`/renew` 以 HTTP 状态，`/reissue` 在 body 中）。

注册请求的 ACL 规则只作用于本 Realm：`source_realm` 可以不填、为 `any_realm`（v1 只匹配本
Realm）或本 Realm 名；写成其他 Realm 名或不合法的 Realm 名时注册返回 `400`。

## Realm membership

`POST /issuer/membership`（protocol `actr-realm-membership/1`，即 Realm Profile 的 `issuer.url` 加
`/membership`）接受 JSON 的 Actr 管理信封（无 `requester`），由 Realm 客户端 key 签名，
`signature.key_id` 为其 `key_id`（公钥 SHA-256 前 16 hex）。`provider_id` 规则与 Issuer 客户端证明
相同。操作见 Actr Realm Membership 第 4 节：

- `join`：`invite`（邀请 secret）、`public_key`、可选 `label`（≤ 64 字符），由新 key 签名；兑换
  邀请并登记该 key，响应 `{realm, member_id, role, key_id}`。同一把 key 重试已兑换的邀请返回首次
  的结果（key 未撤销、成员 active 时）。owner 邀请在 Realm 已有 owner 时把新 key 加给 owner 并撤销
  owner 原有的全部 key；
- `create_invite`：`role`（`editor`/`viewer`，只有 owner）或 `member_id`（为该成员添加客户端：
  owner 可为任何 active 成员，成员只为自己），加 `invite_expires_at`（≤ 30 天）；响应 `{invite_id,
  invite, expires_at}`，`invite` 为 `ri_…` 文本，只返回这一次；
- `revoke_invite {invite_id}`（owner 或签发者所属成员）、`list_members`（owner 看全部，其他成员只看
  自己与自己签发的邀请）、`revoke_client {key_id}`（owner 不能撤销自己最后一把 key）、
  `remove_member {member_id}`、`set_role {member_id, role}`、`transfer_owner {member_id,
  previous_owner_role?}`（目标须 active 且至少有一把未撤销 key）、`leave`（owner 须先转让）；
- `update_realm {package? | display_name? | status: "retired"}`：只有 owner，只用于 Actr Facility Provider
  域名下的 Realm，Loran 签新 Realm Profile（`package.type` 的 publisher/program 不变），响应
  `{realm, sequence}`；其他 Realm `forbidden`。

检查顺序：信封格式、`provider_id`、有效期与 nonce 长度 → 签名 key（`join` 为邀请：查无
`invite_unknown`、属于另一 Realm `invite_realm_mismatch`）→ 记录 nonce → 在一个 `BEGIN IMMEDIATE`
事务中重查 key 与成员、Realm 为 `active`（否则 `realm_unavailable`）→ 角色（`forbidden`）→ 目标
（`not_found`）→ 其余规则。错误以 `{"error": "<code>", "message"}` 返回，状态码与 code 见协议
第 4.3 节；限额 `[realm] max_members_per_realm`（1000）、`max_clients_per_member`（20）、
`max_open_invites_per_realm`（100）超出为 `429 quota`，不消耗邀请。响应带
`Cache-Control: no-store`。

撤销 Realm 客户端 key（`revoke_client`、`remove_member`、`leave`、owner 恢复）在同一事务中撤销该
key 与它签发的未用邀请；绑定到它的 Actor 停用：不再续期、签发 TURN credential，Signaling 与 TURN
立即断开，不能重连。renewal token 保留：同一成员的另一把 key 可凭它 `/reissue` 取回 ActorId 并改绑。
改角色只改记录，在下一份 credential 生效。

成员工具：`actr realm join | invite | members | revoke-invite | revoke-client | remove-member |
set-role | transfer-owner | leave`。`join` 从 stdin（`-`）或参数读邀请，写出 0600 的 Realm 客户端 key
文件；有 Realm Profile 的 Realm 由已验证的 Realm Profile 得到 Issuer 与 Actr Facility Provider，没有的用
`--issuer`（`--provider` 缺省为其 `host[:port]`）。其余命令用 `--key` 指定的 key 文件签名。

## Signaling

`GET /signaling/ws` 升级到 WebSocket。身份只从原生 upgrade headers 或上述浏览器
子协议项读取；连接身份、Actor credential 中的 Actor 与每条 envelope 的 source 必须完全一致。
单帧最大 1 MiB，并执行连接/消息速率限制。连接绑定其身份所属的 Actor 与 Realm：envelope 的
`source` 不是该 Actor、或 relay 的 `target` 不属于该 Realm 时返回 `403`。Actor Registry 与 presence 按
Realm 分区，发现、路由候选、ServiceSpec 查询与上线通知只涉及同 Realm 的 Actor；
`GetSigningKey` 只返回本 Realm 的 issuer key。Realm 不可用（不存在、pending、停用、过期或已
退役）或 Actor 已停用（已撤销，或绑定的 Realm 客户端 key 已撤销）时 upgrade 返回 `403`；Realm 退役
时其在线连接以 close code 1008（`realm retired`）关闭；Realm 客户端 key 撤销时，绑定到它的 Actor 的
连接以 close code 1008（`client revoked`）关闭，其 Actor Registry 记录与 presence 订阅被删除。
按 Realm 分别计的速率限制尚未实现，见架构契约第 9 节。

## Publisher Registry

公开路由前缀：`/registry`。

- `GET /publishers/{name}/verifying-key`
- `POST /packages/nonce`
- `POST /packages/publish`

v1 JSON 统一使用 publisher 术语：

```json
{"publisher":"apps.example.com"}
```

发布 body 的身份字段同样为 `publisher`，成功返回的 package 使用
`publisher_id`、`publisher` 和规范 `type_str`：

```json
{
  "publisher_id": 7,
  "publisher": "apps.example.com",
  "name": "editor",
  "version": "1.2.0",
  "type_str": "apps.example.com/editor@1.2.0"
}
```

其他旧 JSON/query 名称不属于 v1 契约；内部 SQLite 列名和 crate 名不构成 API。Publisher
key id 形如 `actr-pub-<16 hex>`（公钥 SHA-256 的前 16 个十六进制字符）。

Publisher ID 是规范化 DNS 域名。Publisher 以 `register_publisher` 登记（见「Realm 管理请求」）：
Loran 经同一抓取器取 `https://<publisher>/.well-known/actr`，读取其 `publisher` 成员（Publisher
Profile，`docs/product/actrium-v1` 12 §2），要求 `domain` 等于该域名、在
`profile_issued_at`–`profile_expires_at` 之内、由 `profile_signing_keys` 中 `active` 的 key 按 JCS
签名；请求信封须由其中 `active` 的 key 签名；然后记录 `nonce`。首次接受的 Publisher Profile 固定
（pin）其 `profile_signing_keys`；此后按与 `realm_signing_keys` 相同的 §4.4 规则接受（跟随
`rotations` 轮换链，身份变化拒绝，直到运营者重新确认），相同 Publisher Profile 重复登记不变。
Publisher Profile 的 `keys` 至多 8 项，`id` 须按上文规则由公钥算出；须有 1 至 2 把 `active` 包签名
key（`not_after` 为其发布期限，缺省一年；登记时须已过 `not_before`、未到 `not_after`）。两把
`active` key 都可发布与证明持有（轮换重叠期）；原当前 key 只要仍 `active` 就保持为当前 key，否则
另一把成为当前 key。不再 `active` 的 key 归档为 retired，仍可验证旧包；Publisher Profile 不再列出
的 retired key 转为 revoked，其签过的包不再通过验证；用过的 key 不能再成为 `active`。响应 `{"publisher", "status", "sequence"}`；`400` 请求无效，
`403` Publisher Profile 或签名不被接受，`409` Publisher 已停用或并发修改，`429` 已登记的
Publisher 数（不论状态）达到 `[realm] max_publishers`（默认 1000；已登记者更新 Publisher Profile
不受限），`503` 取不到 descriptor。
公开文档中不放 challenge，也没有 `/.well-known/actr/publisher.json`。

Issuer 启动后立即巡检已登记且 active 的 Publisher，此后每轮结束等待 60 秒再扫描。
扫描按 ID 分页（每页 64 条），最多同时复检 8 个域名；单个域名失败不阻塞其余域名。
巡检与注册请求共用同一复检器：成功缓存 24 小时（不能超过 Profile 的签名有效期），
失败至少等待 60 秒再试，仍使用 SSRF 防护抓取、验签与已接受的 key pin。
因此无需等下一次注册请求，已到期的缓存也会更新；正常抓取时，key 移除的同步延迟
最多为剩余成功 TTL、一次扫描间隔与扫描耗时之和。取不到或拒绝新 Profile 时保留
最后已接受的投影并记录告警，注册继续 fail-closed；不会自动停用 Publisher 或确认
新身份。已停用、撤销的 Publisher 不参与扫描，管理员并发删除的登记也不会被巡检重建。
Issuer 关闭时取消扫描及尚未完成的抓取。

管理路由前缀：`/admin/api/publishers`，受 Admin JWT 保护：列表、状态、停用/恢复、删除、key
历史与撤销（含当前 key 的紧急撤销，之后须登记新的 Publisher Profile 才能再发布）、包列表与撤销；
没有由管理员登记 Publisher 的入口。包的长期真实性由 publisher key、signature 与 digest 决定，下载 origin/包 registry
不是信任根。

ActorId 的字符串形式为：

`<serial_hex>.<realm_id>@<domain>`

例如 `2af01.k7m2q9xh@acme.com`。程序类型 `publisher/program@version` 不在 ActorId 中，
由注册与 Actor credential 携带。

可下载 package 的 publisher 必须是规范 DNS 域名；linked/local workload 可以使用
Actr Host 管理的非 DNS publisher 值，但不能借此进入 Publisher Registry。

## Issuer Signer

规范入口：`/signer.v1.Signer/<Method>`（gRPC）。

方法包括 `Sign`、`GetVerifyingKey`、`EnsureNamedKey`、`RetireNamedKey`（退役命名 key，此后不再签名）与 `HealthCheck`。认证使用
nonce-auth request message；`Sign` 的认证 payload 同时绑定 protocol、`key_id` 和待签
消息 SHA-256。不存在下载私钥的 API。生产环境必须配置 KEK，私钥明文落盘会导致配置
校验失败。

## Admin

- `POST /admin/api/auth/login`
- 其余 `/admin/api/**` 需要 `Authorization: Bearer <JWT>`（管理员登录所得），其他 token 为 `401`。

实例运营者可创建多个 Realm，数量受 `[realm] max_realms` 限制（不计已退役的）。
Realm 以 Realm 名寻址，`@` 编码为 `%40`：

- `GET /admin/api/realms`、`POST /admin/api/realms`：列出；创建（body
  `{"realm": "k7m2q9xh@provider.example", "enabled": true, "expires_at": 0, "package": {"href":
  "https://…", "sha256": "<64 hex>", "type": "publisher.example/app@1.0.0"}}`，可选 `display_name`；
  响应一次性返回 `owner_invite` 与 `owner_invite_expires_at`，运营者经带外渠道交给 owner）。Loran
  是 Actr Facility Provider（启用 authority 与带 Signer 的 Issuer）时只创建其 Actr Facility Provider 域名下的
  Realm：先建为 `pending`（`pending_ttl_secs` 后过期），Loran 签发首份 Realm Profile 后转为
  `active`，签发失败则丢弃该 `pending` 行；其他域名的
  Realm 由持有方以 Realm 管理请求创建。Loran 不是 Actr Facility Provider 时创建没有 Realm Profile 的
  `active` Realm，不接受 `package` 与 `display_name`；
- `POST /admin/api/realms/{realm}/owner-invite`：为 `active` Realm 签发新的 owner 邀请（owner
  恢复），响应 `{"success", "realm", "owner_invite", "expires_at"}`，此前未用的 owner 邀请失效。只用于
  Actr Facility Provider 域名下的 Realm 与没有 Realm Profile 的 Realm，其他 Realm 由持有方
  `reissue_owner_invite`（`409`）；Realm 不是 `active` 时 `400`；
- `GET /admin/api/realms/{realm}/members`：只读的成员、客户端（key id、公钥、名称、登记 / 最近使用 /
  撤销时间）与未用邀请，供支持与滥用排查；运营者处置滥用用停用或退役，不直接改成员；
- `GET /admin/api/realms/{realm}/profile`：已接受的 Realm Profile（原样 JSON），没有则 `404`；
- `GET`/`PUT /admin/api/realms/{realm}`：读取；`PUT` 只收 `enabled` 与 `expires_at`，
  `enabled = false` 即停用、`true` 即恢复，Realm 名不可修改；
- `POST /admin/api/realms/{realm}/issuer-key/rotate`：开始轮换该 `active` Realm 的 issuer key
  （Realm Profile §6.1），响应 `{"success", "realm", "key_id"}`。新 key 先在 Signer 中生成并
  列入 Realm Profile（Actr Facility Provider 域名下的 Realm 由 Loran 立即签新 Realm Profile；其他域名
  的 Realm 由持有方经 `draft_profile` 取得并发布），列出它的 Realm Profile 被接受满 24 小时后
  Loran 改用它签发，旧 key 以 `retired` 列出直到最后一个 credential 过期再过 5 分钟，然后移除并
  在 Signer 中退役。并发轮换只有一个能列出新 key，另一个（以及与 Realm 退役竞态的轮换）把刚在
  Signer 中生成的 key 当即退役（此时 Signer 不可用则只记日志，key 留在 Signer 中但不被列出、不签发）。Realm 不是 `active`、已有待切换的 key 或仍列着 3 把 key 时返回 `400`，
  Loran 不是 Actr Facility Provider 时返回 `503`。Actr Facility Provider 域名下的 Realm 每 90 天自动轮换；
- `POST /admin/api/realms/{realm}/retire`：退役。单一事务中删除该 Realm 的连接 ACL、待连接
  注册、邀请、renewal token 与 `/renew`、`/reissue` 的 attempt 记录，保留已签发身份登记、撤销表、
  成员与 Realm 客户端 key；`realm` 行以 `retired` 状态永久保留，
  作为 Actr Facility Provider tombstone，Realm 名不复用。没有硬删除。Actr Facility Provider 域名下的 Realm 由
  Loran 签发退役 Realm Profile 并在同一事务中退役；其他域名的 Realm 若已有 key 持有方签名的
  Realm Profile，运营者不能退役（只能停用），退役只由持有方发布的退役 Realm Profile 触发。
  事务提交后，Signaling 删除该 Realm 的 Actor Registry 记录（内存与缓存）与 presence 订阅并
  关闭其在线连接，Issuer 在 Signer 中退役该 Realm 的全部 issuer key（此后不再签名）；这两步幂等，进程
  重启时对已退役 Realm 重做，Signer 暂不可用时每分钟重试。

gRPC `NodeAdminService` 对应 `CreateRealm`/`GetRealm`/`UpdateRealm`/`ListRealms`/
`RetireRealm`，同样按 Realm 名寻址；`CreateRealm` 新建的 Realm 在响应中一次性带 `owner_invite`。
Loran 是 Actr Facility Provider 时 `CreateRealm` 不能新建 Realm（没有 `package`），只能更新已有 Realm。

Actr Facility Provider 域名下的 Realm 在 v1 只由运营者创建；由用户以 Actr Facility Provider 账号自助创建属于
后续能力。

### 身份变化的重新确认

某 Realm 域名的 `realm_signing_keys`、或已登记 publisher 的 Publisher Profile 签名 key 链接不到
已 pin 的 key（或同 id 换了公钥等被 pin 拒绝）时，Loran 一律拒绝，不自动接受。运营者经以下
admin 接口核对后手动接受（Actr Authority Discovery §4.4 第 6 步）：

- `GET /admin/api/identity/realm-domains/{domain}`、`GET /admin/api/identity/publishers/{publisher}`：
  当场（不经缓存）取该域名的 authority descriptor，返回 `pinned` 与 `fetched` 两组 key（各含
  `sequence`、`digest` 与每把 key 的 `id`、`status`、`fingerprint`，指纹为公钥字节的 SHA-256 hex），
  以及 `outcome`：`accepted`（按 pin 可接受，无需确认）、`identity_change` 或 `rejected`（附
  `reason`）。只对已有 pin 的域名、已登记的 publisher 可用，否则 `404`；取不到或文档本身无效时 `503`。
- `POST` 同一路径，body `{"digest": "<fetched.digest>"}`：重新获取，仅当该文档自身有效、按 pin
  不被接受、且其 `digest` 等于运营者核对过的值时，清空已保存的 key，以它为新基线（publisher 随即按
  `register_publisher` 的规则接受该 Publisher Profile）。按 pin 已可接受、`digest` 不符（核对后文档
  又变了）或 publisher 已停用时 `409`；取不到或文档本身无效时与 GET 相同 `503`；Publisher Profile
  的包签名 key 不合 `register_publisher` 规则时 `400`。响应含 `previous` 与新的
  `pinned`，并写一条审计记录（`identity.confirm`，before/after 为两组 key）。Realm 写入由 superv
  管理时 Realm 域名的确认返回 `403`。

Admin Web 的 Realms 页与 Publisher Registry 页各有对应的核对与「Accept new identity」操作。

## Schema migration

当前 schema 为 v9（`PRAGMA user_version = 9`），各版本的表结构变化见
[架构契约](REALM_V1_ARCHITECTURE.zh.md)第 8 节。运行时只自动初始化空数据库；其他版本（含未版本化的旧库）
一律拒绝启动，不提供迁移工具，需停机后重建数据库，再重新创建 Realm、登记 publisher 与包。

完整信任边界与发布门槛见
[`REALM_V1_ARCHITECTURE.zh.md`](REALM_V1_ARCHITECTURE.zh.md) 和
[`THREAT_MODEL_REALM_V1.zh.md`](THREAT_MODEL_REALM_V1.zh.md)。
