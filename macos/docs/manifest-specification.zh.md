# Actrium Actor Package Manifest

本文说明 macOS 上的独立 Keel Guest 包。该包使用通用 `actr-package/2` 格式，主 Workload 为
`keel.guest/1`。Product v1 发布版本配对分发 Front 包和 Service 包，见
[Product v1 参考清单](../../docs/product/actrium-v1/12-reference-spec.zh.md#4-actr-manifest)。

每个包的根目录包含 `manifest.toml`、Ed25519 原始签名 `manifest.sig`，以及所有在
`[[files]]` 中声明的 payload。最小 Actrium Guest Manifest 如下：

```toml
format = "actr-package/2"

[actor]
publisher = "acme"
name = "notes"
version = "1.0.0"

[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[workload.keel]
runtime = "js"
guest-api = "keel.guest/1"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = ["ui.render"]
optional-capabilities = []

[extensions.actrium]
title = "Notes"
description = "A Keel Guest notes application"
left-panel-default-visible = false
right-panel-default-visible = false

[signature]
algorithm = "ed25519"
key-id = "placeholder"

[[files]]
path = "workload/guest.js"
role = "workload"
media-type = "text/javascript"
size = 0
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
```

打包器会在签名前将 `size`、`sha256` 和 `signature.key-id` 替换为规范值。
Actrium 专属的展示元数据放在 `[extensions.actrium]` 下；它不会改变 Actor 身份、
Workload 语义、Host 要求或包的信任关系。

字段、路径、签名和装载规则以
[`actr/protocols/actor-package.zh.md`](../../actr/protocols/actor-package.zh.md)
为准。可分别通过 `cargo run -p actr-cli -- pkg pack --help` 和
`cargo run -p actr-cli -- pkg verify --help` 查看打包与验证命令。
