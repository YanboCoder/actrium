# Counter Keel Guest

Counter 是用于验证 Actrium Actor Host 的独立 `keel.guest/1` 示例，构建产物是一个
`actr-package/2` Guest 包。包含 Front 包和 Service 包的发布版本见
[Product v1 参考清单](../../../../docs/product/actrium-v1/12-reference-spec.zh.md#4-actr-manifest)。

在当前目录执行：

```bash
bun install
bun run build
bun run build:package
bun run verify:package
```

签名包输出到 `dist/actrium-counter-1.0.0.actr`。这里与 Echo 示例共用仓库内的开发
密钥；该密钥不能用于正式发布。

包结构如下：

```text
actrium-counter-1.0.0.actr
├── manifest.toml
├── manifest.sig
└── workload/
    ├── guest.js
    └── *.png
```

规范模型见
[`actr/protocols/actor-package.zh.md`](../../../../actr/protocols/actor-package.zh.md)。
