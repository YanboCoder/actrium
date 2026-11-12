# 15-B. B（Apple 壳）执行清单

> 状态：执行清单（待 [15 号计划](15-macos-prototype-plan.zh.md) 评审后执行）<br>
> 日期：2026-10-28<br>
> 适用：[15 号计划](15-macos-prototype-plan.zh.md) 中 B 角色的全部工作，按执行顺序组织

## 前置依赖（B 开工前需就位）

| 依赖 | 提供方 | 就绪时点 |
|---|---|---|
| IF-1 冻结（FFI 表面签名） | A 提案 / 组外评审 | D1 末 |
| 验签 FFI（ActriumCore，包解析链） | A | D2 |
| `service_create` / `attach` / `recover` FFI **骨架+模拟桩**（ActrBindings，运行时链） | A | D2 |
| 首个可用签名 `.actr`（含 demo Guest） | C | D2 |
| 真/模拟路径判定 | 组外交付判定点 | D3 末（不影响 B 开工，模拟桩先行） |

## 任务清单（按执行顺序）

### B1. 地址栏解析分支 + 下载链（M，D1）

- `actr://` / 包 URL 的解析分支加入 `BrowserViewModel.navigateCurrentTab`（现仅识别 `actro://` 与本地路径，`macos/Actrium/Views/MainContentView/BrowserViewModel.swift:675`）
- HTTPS 下载 + 大小上限；「先确认后联网」简版确认页（展示域名与目标，用户确认后才开始下载）
- 下载后 SHA-256 校验
- **验收**：输入包 URL 能下载到本地并校验通过

### B2. digest 寻址缓存改造【存量修复】（M，D2）

- 解压目标从 `$(TMPDIR)/actr-<UUID>`（每次重解压，`unzipActrPackage`，BrowserViewModel.swift:1618）改为 `cache/packages/sha256/<digest>/`（05 §15 结构）
- 6 天版：缓存目录 + 版本去重判断；清理 / lipo 策略后置
- **验收**：同包二次打开不再重新下载 / 解压

### B3. 接入验签 FFI（S，D2）

- `AppleActrPackageManifestReader` 链路补 ed25519 验签（当前只验 SHA-256、不验签名的缺口，`packages/actrium-platform/apple/ActriumCore/Sources/ActriumCore/AppleActrPackageManifestReader.swift:24`）
- **验收**：篡改包被拒，正常包通过

### B4. 渲染链改造（6 天版 M / 完整版 L，D3）

- 6 天版：guest 从缓存目录加载（bytes 优先），现有注入路径兜底
- 完整版：彻底 bytes 加载，禁止 URL 直载（对齐 05 §8.2 Keel 加载边界）
- 涉及 renderer config 下发链（`UnifiedReactNativeLayer.sendTabConfigIfReady`）
- **验收**：M1'（M1）退出——地址栏输入包地址 → 下载验签 → Guest 渲染

### B5. macOS 首次接入 ActrBindings（M，随 D4 联调落地）

- 工程接入 `packages/actr-swift`（与 ActriumCore 并存两条 FFI 链；工程配置参照 iOS ActrBridge 的接入方式）
- 注意：现成 `dist/ActrFFI.xcframework` 缺 x86_64 切片，需重跑 `build-xcframework.sh`
- **验收**：macOS 工程内 `import Actr` 可调用 `service_create` 骨架（模拟桩）

### B6. 创建实例流程 + URI 改写（M，D4）

- 创建实例确认页（权限 / Workspace 简版声明展示）
- 调 `service_create`（D3 判定的真 / 模拟路径）→ 拿到 ActorId
- 地址栏 **replace**（非 push）为实例地址；失败时保持原 URI，不静默降级
- **验收**：M2'（M2）退出——创建实例 → 拿到 ID → 地址栏变实例地址

### B7. 重启恢复【存量修复】（M，D5）

- `BrowserDataRepository.saveSession`（:364）去除对非 http/https 的过滤，会话持久化 ActorId
- 启动时经 `service_recover` 重连
- **验收**：M3'（M3）退出——重启 Actrium 后从实例地址恢复，同一 ActorId

### B8. 错误页（6 天版 1 个 / 完整版 3 个，D5）

- 对齐 04 §7 错误码子集：`PACKAGE_DIGEST_MISMATCH` / `PACKAGE_SIGNATURE_INVALID` / `SERVICE_NOT_FOUND`
- 6 天版只做验签失败页（超载减法第一刀）
- **验收**：失败路径有明确可操作的文案，不出现统一「无法连接」

### B9. Guest↔Service RPC 联调（M，D6）

- 消费 C 的 IF-3 contract；6 天版为契约级单链演示（guest `call` → 桥 → 按 IF-3 返回固定应答），完整版为真 broker 全链
- **验收**：M4'（M4）退出

## 负载与风险

- **总负载**：完整版 ≈ 15~18 天；6 天版经减法（B2 降级、B4 兜底、B8 砍到 1 页、B5 因模拟桩降为 M）≈ 6~7 天，缺口由 C 机动支援（建议支援项：B7 恢复链路测试）
- **最大单项**：B4 渲染链彻底改造（完整版 L）——它不阻塞 M2/M3 退出，宽松版可放 W3–W4 与恢复链并行
- **B 的红线**：URI replace 语义（后退不回确认页、失败不改 URI）与会话恢复的 ActorId 一致性——这两处是 v1 模型的行为锚点，宁可延后不可做歪

## 排布对照

| 排布 | B 的节奏 | 验收 |
|---|---|---|
| 6 天版（[15 §5](15-macos-prototype-plan.zh.md)） | D1 下载链 → D2 验签+缓存 → D3 渲染链最小改造 → D4 创建实例 → D5 恢复+错误页 → D6 RPC 联调 | M1'–M4'（降级） |
| 宽松版（4 周） | W1 下载/缓存 → W2 首接+创建实例 → W3 恢复 + 渲染链彻底改造开工 → W4 RPC 全链 + 收尾 | M1–M4（全量） |
