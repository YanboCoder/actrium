# Actrium macOS architecture

Actrium is an Actor Host built with Swift, React Native, Rust, and Keel. The
native shell owns windows, tabs, package storage, process isolation, and the
Rust bridge. The React Native runtime owns system applications and the Keel
Host UI. A third-party UI workload runs only as a `keel.guest/1` workload from
a signed `.actr` package.

```text
Actrium native shell
├── Rust core and Actor runtime bridge
└── React Native system bundle (system apps + Keel Host UI)
    └── signed .actr package
        ├── manifest.toml
        ├── manifest.sig
        └── workload/guest.js
```

The package loading boundary is strict:

1. download or open the `.actr` archive;
2. verify the pinned archive digest when package resolution supplies one;
3. read `manifest.toml` and require `actr-package/2` plus `keel.guest/1`;
4. verify every declared payload digest and the publisher signature;
5. load only the workload entry declared by the manifest;
6. grant only Host capabilities permitted by package and deployment policy.

There is no raw JavaScript distribution path and no second application archive
layer around `.actr`.

Important paths:

- `packages/actrium/index-system.tsx`: React Native system bundle entry;
- `packages/actrium/src/runtime-host/`: Keel Host and application UI;
- `packages/keel/`: sandbox engine, reconciler, and Guest build tooling;
- `packages/actrium-app-sdk/`: Host/Guest bridge APIs;
- `crates/actrium-core/`: shared Rust/native policy and bindings;
- `macos/Actrium/Models/ActrPackageManifestReader.swift`: macOS package reader;
- `packages/actrium/keel-guest-examples/counterapp/`: reusable Guest example;
- `examples/actrium/echo/`: complete Loran + Actor + Actrium Echo demo.

See [Actor Package manifest](docs/manifest-specification.md),
[Keel Guest development](docs/keel-askit-guest-guide.md), and the normative
[Actor Package specification](../actr/protocols/actor-package.zh.md).
