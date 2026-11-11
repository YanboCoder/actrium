# React Native Bundle 构建指南

Actrium 项目包含独立的 React Native bundle。现行 Helper 是单一 `ActriumRenderer.app`，按 fingerprint 隔离进程，不再复制 `RCTViewService*.xpc`。详见 [Renderer-Process-Pool.md](Renderer-Process-Pool.md)。

| Bundle | 入口文件 | 输出文件 | 用途 |
|--------|----------|----------|------|
| System Apps | `index-system.tsx` | `system.jsbundle` | 系统 Renderer（`system.*`） |

## 快速使用

```bash
# 构建 system bundle（Release 模式）
./scripts/bundle-actrium-runtime.sh

# 仅构建 System Apps bundle
./scripts/bundle-actrium-runtime.sh --target system

# 构建 Development bundle（包含 source map）
./scripts/bundle-actrium-runtime.sh --dev

# 查看帮助
./scripts/bundle-actrium-runtime.sh --help
```

## Bundle 说明

### System Apps (`system.jsbundle`)

**用途**：由 system Renderer 加载，渲染系统内置页面。

**包含的应用**：
- `StartPageApp` - 新标签页（`about:blank`）
- `HistoryApp` - 历史记录（`about:history`）
- `BookmarksApp` - 书签管理（`about:bookmarks`）
- `DebugApp` - 调试页面（`about:debug`）

**入口文件**：`packages/actrium/index-system.tsx`

## 源码结构

```
packages/actrium/
├── index-system.tsx      # System Apps 入口
└── src/
    └── runtime-host/     # Actr Guest 容器源码
        └── StandaloneGuestApp.tsx
```

## 开发流程

1. **修改源码**：编辑 `packages/actrium/src/` 下的文件

2. **构建 bundle**：
   ```bash
   ./scripts/bundle-actrium-runtime.sh
   ```

3. **构建 Xcode 项目**：
   ```bash
   xcodebuild -workspace Actrium.xcworkspace -scheme Actrium build
   ```

4. **运行应用**：从 Xcode 运行或打开 `.app` 文件

## 注意事项

- Bundle 文件必须添加到 Xcode 项目的 Resources 中
- 修改源码后需要重新运行 bundle 脚本
- Debug 构建时，8081 端口的 "Connection refused" 错误是正常的（来自 Hermes Inspector）
- Release bundle 使用 `--dev false` 和 `--minify true` 优化

## 文件位置

- **Bundle 输出**：`Actrium/Resources/`
- **Xcode 项目**：`Actrium.xcodeproj`
- **构建脚本**：`scripts/bundle-actrium-runtime.sh`
