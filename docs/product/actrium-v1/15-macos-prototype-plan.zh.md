# 15. macOS 原型三人分工计划

> 状态：执行计划（已采纳 FFI 层边界切分）<br>
> 日期：2026-10-28<br>
> 目标：跑通 [11 §14 最小纵切](11-delivery-plan.zh.md) 的 macOS 原型<br>
> 人力：3 人（A=FFI/绑定层、B=Apple 壳、C=工具链+RN）+ 底层运行时由既有维护者（组外）交付

## 1. 原型目标

最小可演示纵切（对应 11 §14，按当前代码基础裁剪）：

```
地址栏输入包地址 → 确认 → 下载 + SHA-256 → 验签 → digest 缓存
→ local Realm 创建 Service（持久 ActorId）→ 渲染包内 Keel Guest
→ 地址栏 replace 为实例地址 → 重启 Actrium 后同 ActorId 恢复
→ Guest 经 PrimaryService RPC 调用 Service
```

裁剪约定：

- 持久 ActorId 采用 local Realm 最小版（不做 reclaim / 跨 Realm / 凭据轮换，留 P1-A 完全体）；
- Service workload 允许先以 `native-cdylib` 代替 WASM（05 §5.2 桌面开发模式允许），WASM 后置；
- 总工期 6 个工作日，验收按 §5 的 M1'–M4' 降级定义执行（模拟 ActorId 骨架为主线），降级项见 6 天外后置清单；

## 2. 职责边界

**核心切分原则**：三人组只负责 FFI/绑定层、Apple 壳与工具链；底层运行时语义（身份、Realm、Service 生命周期）由既有维护者在 `actr-hyper` 等库交付。这样避开 actr-hyper（135K 行，multi-Realm 重构 #346 进行中）的并行冲突区。

**FFI 两条链（现状分层，原型沿用，不合并）**：

```
包解析链：actr-pack/actr-protocol → actrium-core (UniFFI) → ActriumCore.framework
          （macOS 现用；actrium-core 刻意不依赖 actr-hyper，保持不含运行时）
运行时链：actr-hyper → actr-ffi (UniFFI) → ActrBindings (packages/actr-swift)
          （iOS 现用；macOS 原型首次接入）
```

### A — FFI / 绑定层（actrium-core、actr-runtime、actr-ffi、actr-swift）

| 做 | 不做（转组外） |
|---|---|
| actrium-core：验签 FFI 暴露 + SQL/契约下发维护（包解析链） | actr-hyper：持久 ActorId、local Realm 运行时、Service 生命周期实现、PrimaryService broker 实现 |
| actr-ffi：uniFFI 定义 + `service_create`/`attach`/`recover` 运行时 FFI（运行时链） | actr-protocol：actr:// 语法、Realm Profile schema 定义 |
| actr-swift：Swift 绑定同步 | actr-config：local Realm bootstrap 实现 |
| actr-runtime：wasm32 分发薄层维护 | actr-platform-*：key 存储/持久化原语 |
| 向底层库提出 FFI/接口需求、输出《底层能力需求单》并锁定接口时间表 | actr-pack：manifest schema 扩展决定权 |

### B — Apple 壳（端到端集成，唯一全程碰 UI 的人）

macOS 原型的全部壳侧工作：地址栏/下载/验签消费/缓存/创建实例 UI/URI 改写/重启恢复/错误页。

### C — 工具链 + Guest 样例 + RN 侧

demo Guest、actr CLI 打包链、PrimaryService contract 定义、e2e 脚本、RN 双文件改造；联调期第一机动。

### 组外交付（底层运行时，对应"其他人员"）

| 交付物 | 自然归属（按现有维护格局） | 说明 |
|---|---|---|
| 持久 ActorId（local Realm 最小版）+ Service 生命周期 + PrimaryService broker | actr-hyper 维护者（zhanghongjun/alinejun；multi-Realm #346 为前置） | 原型 M2/M3 的关键路径 |
| actr:// URI 语法 + Realm Profile schema | actr-protocol 维护者（kookyleo/zhanghongjun） | IF-2 的 schema 定义权 |
| local Realm bootstrap 配置 | actr-config 维护者（kookyleo） | 体量小（4.3K 行） |

## 3. TODO 清单

### A（FFI/绑定层）

- [ ] 输出《底层能力需求单》：持久 ID / local Realm / `service_recover` 的接口语义 + 需求时间表（交组外，作为其工作锚点）
- [ ] IF-1 提案：FFI 表面签名（`verify_package_signature` / `service_create` / `attach` / `recover`），提交 actr-hyper 维护者评审
- [ ] actrium-core 暴露验签 FFI（消费 actr-pack 既有能力，M1 可用；留在包解析链）
- [ ] actr-ffi 暴露 `service_create` / `attach` / `recover`（运行时链，沿用 iOS 已验证的 ActrBindings 路径；依赖组外交付，接口先冻结）
- [ ] actr-swift 同步两条链的新绑定（包解析链 → ActriumCore.framework；运行时链 → ActrBindings）
- [ ] actr-ffi / actr-runtime 常规维护（按需）

### B（Apple 壳）

- [ ] 地址栏 `actr://`/URL 解析分支 + HTTPS 下载 + 大小限制 + 「先确认后联网」简版确认页
- [ ] SHA-256 校验接入下载链
- [ ] 【存量修复】解压临时目录改 digest 寻址缓存（`cache/packages/sha256/<digest>/`，05 §15），消除每次重解压
- [ ] 接入 A 的验签 FFI（补上当前只验 SHA-256、不验 ed25519 的缺口）
- [ ] macOS 首次接入 ActrBindings 框架（运行时链，工程配置参照 iOS ActrBridge 的接入方式；与 ActriumCore 并存两条链）
- [ ] 渲染链改造：verified bytes → renderer 进程 Keel Engine，guest 从缓存 bytes 加载（禁止 URL 直载）
- [ ] 创建实例确认页（权限/Workspace 简版）→ 调 `service_create` → 地址栏 replace 为实例地址
- [ ] 【存量修复】Tab 会话持久化 ActorId（修掉会话恢复不含 `.actr` 的缺口）→ 启动时 `service_recover` 重连
- [ ] 错误页 3 个：下载失败 / 验签失败 / Service 离线（对齐 04 §7 错误码子集）
- [ ] Guest↔Service RPC 联调

### C（工具链 + Guest + RN）

- [ ] demo 程序：最小 Keel Guest（`guest.bundle.js` + `capability-manifest.json`，三栏中可渲染列表/按钮）
- [ ] 打包链：actr CLI 产出符合 IF-2 schema 的签名 `.actr` + 配套 service workload（初版 native-cdylib）
- [ ] IF-3：PrimaryService proto/contract 文件与代码生成
- [ ] e2e 脚本：基于 `tests/e2e/actr` 写「下载 → 创建 → 重启 → 恢复」自动验收
- [ ] RN 侧 manifest/actr 双文件改造（iOS 复用纵切的前提，不丢）
- [ ] 联调期第一机动（最熟悉包格式全链路）

### 组外（底层运行时）

- [ ] 评审 IF-1 FFI 表面与 IF-2 schema 提案
- [ ] 持久 ActorId：local Realm 内长期 key、ID 持久化、进程重启 rebind 同 ID
- [ ] Service 生命周期：create / stop / start / recover
- [ ] PrimaryService broker 最简实现（session/health + 一个业务方法，按 IF-3）
- [ ] local Realm bootstrap 配置（actr-config）

## 4. 接口冻结点

| 编号 | 接口 | Owner | 评审/确认方 | 冻结时点 |
|---|---|---|---|---|
| IF-1 | FFI 表面：`verify_package_signature`（包解析链，actrium-core）+ `service_create` / `attach` / `recover`（运行时链，actr-ffi） | **A 提案** | actr-hyper 维护者确认可行性 | D1 末 |
| IF-2 | manifest 扩展 schema：`actr-package/1` 内 `[actrium]` 段字段 | **组外定义权**（actr-pack/actr-protocol 一侧），A/C 共同提案 | — | D1 末 |
| IF-3 | PrimaryService contract：proto + capability manifest + `host:service` HostModule 桥接约定，**载体文档见 [15-IF3](15-if3-keel-host-contract.zh.md)** | C 定义 | A 确认可暴露、组外确认可实现 | D3 末 |

IF 之外三人各自迭代互不阻塞；A↔组外通过《底层能力需求单》与 IF-1 评审对齐。

## 5. 时间表（总工期 6 个工作日）

按 D1–D6 六个工作日编排。**工期压缩的前提是接受降级定义**：组外的持久 ActorId（XL 级）不可能在 6 天内完整交付，6 天版以**模拟 ActorId 骨架为主线**（本地持久化的占位身份，接口与真 ID 完全一致），组外若 D3 末能交付 local Realm 最小 ID 则当天切换真实实现。

**6 天版验收（M1'–M4'，带撇号即降级定义）**：

- M1' = M1 全量：地址栏输入包地址 → 下载 + 验签 → Guest 渲染
- M2' = 创建实例流程走通，拿到 ActorId（**真或模拟**），地址栏 replace 为实例地址
- M3' = 重启 Actrium 后同 ActorId 恢复（真或模拟）
- M4' = RPC **契约冻结 + 单链桩演示**（guest `call` → 桥 → 按 IF-3 契约返回固定应答），非完整 broker

**6 天外后置清单**（原型完成即启动）：真持久 ActorId 替换模拟、PrimaryService broker 真实现、渲染链 bytes 加载彻底改造（6 天版沿用现有注入路径 + 缓存目录）、e2e 全绿、RN 双文件改造、剩余错误页。

| 日 | A | B | C | 组外 | 出口 |
|---|---|---|---|---|---|
| D1 | 《底层能力需求单》发出；IF-1/IF-2 提案送评审；验签 FFI 开工（actrium-core） | 地址栏解析分支 + 下载 + SHA-256 | demo Guest 开发；IF-2 schema 对齐 | 评审 IF-1/IF-2 | IF-1/IF-2 冻结 |
| D2 | 验签 FFI 完成；`service_create`/`attach`/`recover` 接口骨架（模拟实现先行） | 接验签 FFI；digest 缓存改造 | 打包链产出首个可用签名 `.actr` | 持久 ID 突击（沿用 multi-Realm #346 方向） | **M1' 退出**：下载验签 → Guest 渲染 |
| D3 | FFI 骨架联调；actr-swift 同步 | 渲染链最小改造（缓存目录 + bytes 优先、现有注入路径兜底） | IF-3 contract 定义并冻结；PrimaryService 代码生成 | **交付判定点（D3 末）**：local Realm 最小 ID 可交付 → 真路径；否则全链走模拟 | IF-3 冻结；真/模拟路径定案 |
| D4 | `service_create` 接通（真或模拟桩） | 创建实例确认页 + URI 改写（对接 D3 定案路径） | e2e 脚本（按定案路径） | 真路径时：Service 生命周期交付 | **M2' 退出**：创建实例 → 拿到 ID → 地址栏变实例地址 |
| D5 | `service_recover` 接通 | 重启恢复（会话持久化 ActorId）+ 错误页 3 个 | e2e 跑通；RPC 单链演示准备 | broker 桩（按 IF-3 返回固定应答） | **M3' 退出**：重启后同 ID 恢复 |
| D6 | FFI 面整理、需求单回填 | Guest↔Service RPC 联调（契约级） | RPC 单链演示 + 演示脚本 | broker 桩联调 | **M4' 退出**：RPC 契约冻结演示，骨架原型完成 |

**压缩期裁剪说明**：6 天版中渲染链只做最小改造（bytes 优先 + 兜底），「禁止 URL 直载」的彻底改造后置；模拟 ActorId 的占位实现由 A 在 actr-ffi 骨架中提供，切真实现时 B 的壳层代码零改动（仅换 FFI 后端）。

## 6. 风险与兜底

| 风险 | 兜底 |
|---|---|
| 6 天工期压缩（原 6 周内容量） | 验收降级为 M1'–M4'（模拟 ActorId 骨架 + 契约级 RPC）；后置清单随原型完成立即启动，降级项不丢失 |
| 组外持久 ActorId 超期（唯一关键路径） | D3 末交付判定点；未交付即全链走模拟 ID；需求单保证组外工作有锚点 |
| actr-hyper 并行冲突 | A 不直接改 actr-hyper，全部经 FFI 面；IF-1 由组外评审后再实现 |
| FFI 链路选型反复 | 已定：运行时能力走 actr-ffi → ActrBindings（与 iOS 同链），actrium-core 保持不含运行时；如需合并须 IF-1 评审重新决策 |
| WASM Service 复杂 | Service 先走 `native-cdylib`（05 §5.2 桌面开发模式合规），WASM 后置 |
| QuickJS 硬限制验证 | 原型用现有 Keel JS 后端，硬限制后置 hardening |
| 三人中任一阻塞 | C 为第一机动；IF 冻结点保证阻塞不跨人传染 |

## 7. 与既有计划的衔接

- 本计划是 [11 交付计划](11-delivery-plan.zh.md) 中 P1-A（持久身份）与 P2-B（Service Manager 最简子集）在 macOS 上的**垂直切片预演**；A 的 FFI 面与 IF-2 schema 直接回流 P1/P2 主线；
- 底层运行时工作与 upstream 进行中的 multi-Realm 重构（#346）、Realm 授权修复是同一条线，无需维护者切换上下文；
- 不做的东西（Publisher 域名信任链、Marketplace、远端 Attach、多 Realm、三色保障）见 [11 §14](11-delivery-plan.zh.md) 与 [01 §8.2](01-product-definition.zh.md) 可延期清单；
- 四端复用路径：RN 双文件改造与 e2e 脚本（C）保住移动端复用前提；FFI 面设计保持平台中立（A），后续 Android 经 actr-kotlin、Windows 经 actr-cabi 接入。
