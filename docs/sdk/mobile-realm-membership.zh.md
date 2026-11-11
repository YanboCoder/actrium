# 设备 Realm 身份与入会

本实现对应 [v1 网络与身份设计 §7.1](../product/actrium-v1/07-realm-network-and-identity.zh.md#71-信任域名与加入实例-realm)、FR-NET-022、FR-SEC-005、FR-SEC-012，以及 issue #347。

## SDK 调用顺序

1. 从用户已接受的 Realm Profile 得到 Realm、`provider.id` 和 Issuer URL。本文接口不负责域名发现、首次信任或 Profile 验签。开发 TestApp 可以显式配置这三项，并显式允许 HTTP。
2. 用 `RealmIdentityStore.membership` 打开该安装、该 Realm 的身份。新身份必须先持久保存，再调用 `join`；应用重启后打开同一身份。
3. 调用 `join(invite, label)`，由设备自己的新 key 签名。SDK 不保存或输出邀请；成功后 UI 清除输入。响应未知时保留原 key，重试同一个邀请，服务端按 invite + key 原子消费和幂等恢复。
4. 将返回的 `RealmMembership` 传给 `ActrNode.linked` / package 构造函数的 `membership` 参数。Host 的初始注册、重签、续期和重新注册都复用该 signer。配置不得同时出现 `deployment.realm_client_key`。
5. `members` 返回可见成员/客户端；`inviteClient(memberId, expiresAt)` 为已有成员添加设备；`revokeClient(keyId)` 撤销一个客户端；`revokeInvite(inviteId)` 撤销未使用邀请。权限仍由 Issuer 执行。

Swift 使用接受 `String` 配置路径和 `mobileLifecycleObservers` 的完整构造函数；Kotlin 可使用 path 形式的 `linked` / `linkedWithMonitoring`。返回的成员数据是现有协议的 JSON，错误是独立的 UniFFI `RealmMembershipError` / Kotlin `RealmMembershipException`。包运行时和 linked runtime 都能注入身份，旧桌面 key 文件接口继续可用。

## 存储与恢复

| 平台 | 私钥保存 | 安装隔离与恢复 |
| --- | --- | --- |
| Android API 26+ | 为每个安装、Realm 生成 Ed25519 seed；Android Keystore 保存不可导出的 AES-256-GCM wrapping key，密文存入 `noBackupFilesDir`。AAD 绑定安装 UUID 与 Realm。 | AtomicFile 与跨进程文件锁保护创建和读取。重启复用；清数据或卸载后的新安装重新入会。仅恢复密文、丢失 wrapping key、记录损坏或密文丢失均拒绝自动换 key。 |
| iOS / Swift | CryptoKit 软件 Ed25519；seed 保存为 Keychain `AfterFirstUnlockThisDeviceOnly`、`synchronizable=false` 的条目。 | 安装 UUID 和公钥记录保存在排除备份的目录。新安装使用新 UUID，即使系统仍保留旧 Keychain 条目也不复用；公钥记录仍在而 Keychain 丢失时拒绝自动换 key。 |

协议固定 Ed25519，不把 P-256 当作等价替代。这里明确使用软件 Ed25519：Android 由 Keystore 包装，Swift 由本机 Keychain 保存。Keystore 是否由硬件支持取决于设备，不能据此宣称 Ed25519 运算在安全硬件中完成。签名时需要短暂读取 seed，临时缓冲区尽力清零；Host 只保留 public key 与签名回调。

撤销后先停止使用该身份的 Host，再显式 `forgetForRejoin(realm)`，然后用新邀请加入。本地删除不等于服务端撤销；其他客户端可通过 `revokeClient` 清理旧记录。旧 signer 对象不能在重置后偷偷使用新 key。

参考：[Android Keystore](https://developer.android.com/privacy-and-security/keystore)、[Android 备份排除规则](https://developer.android.com/identity/data/autobackup)、[Apple ThisDeviceOnly 存储](https://developer.apple.com/documentation/Security/kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)。

## 错误与重试

- `Rejected` 保留稳定的 HTTP status / code；已使用、过期、撤销、Realm 不匹配分别是 `invite_used`、`invite_expired`、`invite_revoked`、`invite_realm_mismatch`。
- `Uncertain` 表示超时、代理错误、未知错误或异常成功响应。不得因该错误生成新身份；原 key 和原邀请可以重试。
- `KeyUnavailable` 表示平台存储/签名不可用或签名校验不匹配；不会回退到无签名请求。
- SDK 不跟随 membership HTTP 重定向，也不把响应正文、邀请或平台异常详情拼入错误。成功正文上限为 1 MiB。
- 移动时钟可能略快于服务器，客户端把请求起点回退 30 秒，仍保持协议最多 300 秒的有效期。服务器时间窗口、nonce 和签名校验保持严格。
- UniFFI 的设备 Host 构造过程使用现有可取消的 Tokio 执行边界，避免在 Swift 协作执行线程上执行较深的原生注册调用。

## TestApp 与发布检查

Android demo 的 Realm membership 页面和 Swift EchoApp 的同名入口提供加入/重试、恢复/列出、邀请另一设备、撤销与显式重置。管理身份前停止已运行的 Host。开发界面要求输入可信 provider ID；它不是产品的首次域名信任流程。

Kotlin Echo、Swift Echo、DataStream 和 TS workload 的运行时配置不再引用成员私钥。E2E harness 在安装后把一次性邀请通过受限临时文件交给设备，设备自行生成 key；邀请不进入 APK、IPA、资源、启动参数或日志。成功兑换后删除文件。Swift fixture 的未知结果会保留排除备份的临时邀请，以便原身份重试；这是测试夹具，不是应用普通配置。

`scripts/actr/check-mobile-assets.py` 检查资源目录和 APK/IPA 中的 Realm key JSON、邀请及 key-file 配置。Android demo 的 preBuild 和移动 E2E 安装步骤执行此检查。它是结构化凭据检查，不替代通用 secret scanning。

扫描按资源内容进行，不依赖文件扩展名或归档中的目录，覆盖 IPA 的 `.key` / `.txt` 和 APK 的 `res/raw`。已识别的编译代码（ELF、Mach-O、DEX 等）不作为凭据文档扫描；其他单项资源超过 16 MiB、输入缺失或无法读取时检查失败，不静默跳过。回归测试 `python3 scripts/actr/tests/test_check_mobile_assets.py` 由 CI Gate 执行。

## 可重复验证

先按各 SDK 的 `build-android.sh` / `build-xcframework.sh` 构建当前 checkout 的原生库及绑定。使用匹配的库和生成代码，避免 UniFFI checksum 不一致。

```sh
cargo test -p loran-issuer --test integration_test
cargo test -p actr-hyper --lib realm_membership
cargo test -p actr-hyper --lib issuer_client::tests

ANDROID_SERIAL=emulator-5554 bash tests/e2e/actr/mobile-membership/run.sh android
IOS_SIMULATOR_UDID=<booted-simulator-uuid> bash tests/e2e/actr/mobile-membership/run.sh ios
```

移动 harness 使用生产 Issuer、SQLite membership 事务和嵌入式 Signer。只有 signaling 使用测试服务器，并通过 heartbeat warning 触发 Host 正常续期路径；续期本身仍到真实 Issuer。两个隔离安装存储分别加入、注册、自动续期，撤销 A 后，harness 独立检查 A 的 renew 403 和 B 的 renew 200。邀请只写入 mode-0600 的临时文件，结束时删除；诊断只保留公钥 ID 与计数。

普通 `swift test` 未设置 `ACTR_MOBILE_FIXTURE` 时，真实 Issuer 用例明确报告为 skipped，不计作通过；设置 fixture 后仍执行完整用例，文件丢失或损坏会失败。Unix 专用的移动 Issuer fixture 仅在 Unix 平台编译；CI 通过 `python3 scripts/actr/tests/test_mobile_fixture_cfg.py` 在非 Unix 的 `wasm32-wasip2` 目标上编译该 fixture，防止平台隔离回归（需要已安装该 Rust target）。

2026-10-28 验证：Android 14 arm64 模拟器、iOS 26.5 模拟器上的上述用例均通过；每端使用两个独立安装目录。存储测试另外覆盖跨 Realm 隔离、重启恢复、显式重置、旧 handle 失效及恢复时缺少安全存储条目。Rust 集成测试覆盖重签、响应丢失重试、邀请已使用/过期/撤销/Realm 不匹配。模拟器结果不等同于物理设备硬件安全能力认证。
