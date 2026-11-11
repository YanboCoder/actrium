# Actrium for macOS

This directory contains the native macOS browser host, its view service,
shared IPC sources, Xcode projects, and platform-specific tests.

Shared React Native runtime code lives in `../packages/actrium`; Keel lives in
`../packages/keel`; shared Apple packages live in
`../packages/actrium-platform/apple`. The Xcode project and bundle scripts
reference those monorepo paths directly.

From the repository root, run the unsigned debug build with:

```sh
npm run verify:macos
```

Runtime bundles can be generated independently with
`npm run bundle:macos:system`.
