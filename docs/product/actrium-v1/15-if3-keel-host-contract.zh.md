# 15-IF3. Shell → Keel 能力面约定（原型版）

> 状态：骨架稿（C 主笔回填 §2 schema；评审确认后即为 [IF-3](15-macos-prototype-plan.zh.md) 冻结载体）<br>
> 日期：2026-10-28<br>
> 定位：只填缺口，不重复既有定义——语义原则见 [06 §4/§5](06-runtime-and-protocol.zh.md)，manifest 格式见 [12 §5](12-reference-spec.zh.md)，类型机制与工具链见 `packages/keel`
> 分工：C 主笔（§1/§2）· B 补审（§3）· A 确认 FFI 对接（§3.3）· 组外确认 broker 侧（§3.4）

## 1. 原型开放清单

原型期 Shell 提供给 Keel Guest 的能力面收敛为最小集：

### 1.1 Host modules

| module id | 方法面 | 状态 | 说明 |
|---|---|---|---|
| `host:service` | 按 §2 schema（PrimaryService contract 生成） | **新建** | Guest 的唯一 Actor 入口；Host 内部固定 target 为当前 Service ActorId，Guest 不可传 target（06 §5.2） |
| `host:assets` | `get(id) -> bytes|uri` | 复用现有资产通路 | 仅接受包内 asset id（06 §4 Image asset-only）；demo 需要图片时启用，否则可裁 |

原型期**不开放**：`host:clipboard` / `host:notifications` / `host:camera` / `host:microphone` / `host:location` / `host:external-link` / `host:file-import` / `host:file-export` / `host:share` / `host:net`（06 §5.3 清单中其余能力；未开放即不可用，Guest 引用将在 build 静态分析报错）。

### 1.2 Components 白名单

沿用 12 §5 示例最小集：

```json
["View", "Text", "TextInput", "Button", "FlatList", "ImageAsset"]
```

规则（06 §4）：props/event 走 schema 校验；文本/数组/层级/render-op 受配额；`WebView` 不进 Registry；禁用通用字符串 RemoteRef，仅类型化方法。

## 2. `host:service` 方法级 schema（C 回填）

由 IF-3 的 PrimaryService proto 经代码生成产出，路由格式沿用 12 §5（`<proto>.<Service>.<Method>`）。

```text
【占位：C 按 PrimaryService.proto 回填】

module: host:service
rpc:
  - route: "actrium.primary.v1.<Service>.<Method>"
    input:  <schema>
    output: <schema>
  - ...
events:
  - "actrium.primary.v1.<changed>"
binary 字段标记：<无 / 标注>
```

约束：

- 每个路由必须有 input/output schema（KeelContractShape 的 schema 机制，`packages/keel/src/contract/index.ts`）；无 schema 的方法不进 manifest，build 静态分析拒绝
- guest 侧使用形态（06 §5.2）：

```typescript
const result = await service.call("actrium.primary.v1.X.Get", input);  // request/response
service.tell("actrium.primary.v1.X.Do", input);                        // fire-and-forget
service.on("actrium.primary.v1.changed", handler);                     // named event
```

- capability manifest（12 §5 格式）由 keel CLI build 外化生成（`keel-capabilities.json`），安装时与静态分析结果比对，不一致即拒

## 3. Host 侧实现约定（macOS renderer）

### 3.1 注册

Host module 实现沿用现有 renderer 惯例（`RuntimeTabView` 的 `hostComponents` / askit contracts 注册路径），本契约将其落为明示要求：

- 每个 `host:<id>` 实现为 `HostModulesImplementation<TContract>`（`packages/keel/src/contract/index.ts`）；
- guest bundle 中的 `import 'host:xxx'` 由 keel CLI 外化，运行期经 Engine dispatch 进入 Host handler（`HostModuleDispatchTable`，按 input/output/event 三相边界校验）。

### 3.2 `host:service` dispatch 通道

```text
guest: service.call(route, input)
  → Keel Engine dispatch（input schema 校验，phase=input）
  → renderer 内 host:service handler
  → capability 闸门（§3.5）
  → ActrBindings FFI：service_call(session, route, inputBytes)   ← A（IF-1）
  → Actr RPC（Dest::Peer(serviceActorId)，Host 固定 target，Guest 不可见）
  → outputBytes 回传（phase=output schema 校验）
  → Promise resolve
```

- `tell`：同链路，不等待 output，FFI 侧不建 reply 等待；
- `on`：named event 经 FFI 订阅回调 → dispatch（phase=event）→ guest handler；
- session/lease 元数据（session id、digests、deadline）由 Host 层附加，Guest 不可覆盖（06 §10）。

### 3.3 错误与超时映射

| 层 | 错误 | Guest 侧所见 |
|---|---|---|
| schema 校验失败（input/output/event） | dispatch 边界拒绝 | `Error`（含 phase 与字段路径），不触达 FFI |
| FFI / Actr RPC | `ActrError`（12 §10 系统错误码） | `Error`：`name = "ActrError"`、`code = <稳定错误码>`、`message = 用户可读文案` |
| 超时 | RPC deadline 到期 | 同上，`code = TIMED_OUT` |
| Service 离线 | `SERVICE_NOT_FOUND` / offline | 同上；不自动重试（重试策略属 Guest/业务层） |

约定：**错误码用 12 §10 的稳定码，不透传 Rust debug 字符串**；deadline 由 Host 层按 contract 声明注入。

### 3.4 broker 侧对齐（组外确认）

- 原型期 broker（组外交付）只需支持 `service_call` / `service_tell` / `service_subscribe` 三个 FFI 后端语义；
- mock/真实 Service 均按 IF-3 contract 应答；契约外路由返回 `UnknownRoute` 类稳定错误码。

### 3.5 原型期闸门裁剪（06 §5.3 的 7 道闸）

| 闸 | 原型期 |
|---|---|
| ① contract schema | ✅ 保留（dispatch 边界强制） |
| ② manifest requested | ✅ 保留（keel CLI 生成 + 安装比对） |
| ③ permission grant | ✅ 保留（创建实例确认页授予，B） |
| ④ session scope | ⏩ 后置（原型单会话） |
| ⑤ platform policy | ⏩ 后置 |
| ⑥ rate/resource limit | ⏩ 后置（配额体系属 P2-A） |
| ⑦ audit | ⏩ 后置 |

## 4. 与既有文档/代码的引用关系

| 内容 | 出处 | 本契约动作 |
|---|---|---|
| 能力语义、7 道闸、Component Registry 规则 | 06 §4 / §5 | 引用，不重复 |
| capability manifest JSON 格式 | 12 §5 | 引用；§2 的 schema 是其 `hostModules.service.rpc` 的方法级展开 |
| `KeelContractShape` / Dispatch / 静态分析 | `packages/keel/src/contract`、`src/cli` | 引用；Host 实现按其类型签名 |
| FFI 表面 | [15 §4 IF-1](15-macos-prototype-plan.zh.md) | §3.2/§3.4 消费 |
| PrimaryService proto | IF-3（C） | §2 由其生成 |

## 5. 冻结条件

- §2 schema 回填完毕（C，proto 定稿后）；
- §3.2 通道与 §3.3 错误映射经 B/A 评审确认；
- §3.4 经组外确认；
- 三方确认后，本文档即 [15 号计划](15-macos-prototype-plan.zh.md) IF-3 的冻结载体，冻结时点按该计划 D3 末执行。
