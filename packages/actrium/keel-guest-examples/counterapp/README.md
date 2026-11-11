# Counter Keel Guest

Counter is a standalone `keel.guest/1` example for Actrium's Actor Host. The
example builds one `actr-package/2` Guest archive. See the
[Product v1 reference manifests](../../../../docs/product/actrium-v1/12-reference-spec.md#4-actr-manifest)
for a release with both a Front package and a Service package.

From this directory:

```bash
bun install
bun run build
bun run build:package
bun run verify:package
```

The signed package is written to `dist/actrium-counter-1.0.0.actr`. The checked
development key is shared with the Echo example and must not be used for a
production release.

Package shape:

```text
actrium-counter-1.0.0.actr
├── manifest.toml
├── manifest.sig
└── workload/
    ├── guest.js
    └── *.png
```

The normative package model is documented in
[`actr/protocols/actor-package.zh.md`](../../../../actr/protocols/actor-package.zh.md).
