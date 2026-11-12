# V2 Runtime and Issuer joint acceptance

This matrix integrates PR #426 (Issuer) and PR #427 (native Host/CLI). It
complements each PR's unit tests with a real Component load, real Issuer
registration, signaling, and a cross-Actor Echo RPC.

## Reproduction prerequisites

- Use one active Rust toolchain with the `wasm32-wasip2` target installed.
  CI uses Rust 1.99.0. A target installed through rustup is not automatically
  available to an unrelated Homebrew/system `rustc`.
- Use `wasm-component-ld 0.5.26`, `wasm-tools 1.253.0`, Python 3.11+, and the
  existing package-runtime-echo prerequisites listed in README.md.
- Prepare the CLI Web runtime assets using
  `bash packages/actr-web/scripts/sync-cli-assets.sh --build`, or use an existing
  compatible generated-asset cache and verify the source assets are synchronized.
- Select unused local HTTP/ICE ports. Each run creates its own Realm, SQLite
  state, Publisher key, device keys, and temporary Host directories.

The server is the real Loran composition with its existing test-only public
document transport. That transport supplies disposable signed Publisher/Realm
documents; it does not replace Issuer authentication, Signer, SQLite, signaling,
the Component engine, or RPC. The fixture override does not exist in production
binaries.

## Automated matrix

| Area | Assertions | Entry point |
| --- | --- | --- |
| Shared manifest | The Issuer fixture meets the supported native Host profile; v2 has no legacy target; unsupported profile/API/required capabilities are rejected | `issuer_fixture_matches_the_supported_native_profile` |
| Host recovery context | The real Host request builder sends original manifest/signature/public key in PACKAGE mode and retains exactly that request for credential recovery | `v2_registration_preserves_original_material_for_credential_recovery` |
| Package integrity | Bounded payload verification, identity/key binding, role/path/file-table checks, and original-byte signatures | actr-pack unit tests |
| RPC contracts and locks | Self-contained Proto fingerprints, signed lock binding, required aliases, and explicit export of a contract shared with an imported dependency | actr-service-compat and package_runtime unit tests |
| Real v2 runtime | Build the current Workload ABI, sign/verify its v2 archive, load it with production `actr run`, register with real Issuer, and exchange an Echo RPC with another Actor | `joint-v2-issuer.sh` |
| Realm lifecycle | Create/join a signed Provider Realm, reject malformed trust/invitation inputs, restart the service, repeat RPC, and reject a removed member | Provider/HTTPS mode of the runtime runner |
| Real Component identity lifecycle | Reuse the exact Component bytes that passed runtime/RPC acceptance in the real SDK HTTP → Issuer → SQLite/Signer tests; verify renew/reissue, stable ActorId, retry idempotency, and same-member device recovery | `ACTR_V2_COMPONENT_FIXTURE` plus `v2_package::` tests, invoked by the joint runner |
| Identity refusals | Wrong format/target/type/key/signature, revoked/unknown devices, non-Owner main-Service registration, foreign-member/invalid possession proofs, and Publisher impersonation | Issuer v2 integration matrix |
| Legacy behavior | Existing unversioned cdylib registration/RPC and Publisher key retirement/revocation | Existing runtime and rotation runner modes |

The Component-backed Issuer lifecycle cases use independent fixture Actor
identities. They verify the real HTTP identity protocol for the executed
Component's package bytes; they do not claim to force a running CLI process's
background credential scheduler through every recovery state.

## Unit and identity suites

Use the same process isolation as CI. A bare multi-test libtest process can
share a tracing subscriber or exhaust short timing budgets across unrelated
Hyper tests; Nextest gives each test its own process.

```sh
cargo nextest run --locked --offline \
  -p actr-pack -p actr-protocol -p actr-service-compat \
  -p loran-issuer -p actr-hyper -p actr-cli \
  --features actr-cli/wasm-engine,actr-hyper/test-utils \
  --lib --test integration_test --no-fail-fast --test-threads 4
```

Omit `--offline` only when the pinned registry/Git dependencies have not yet
been downloaded. Default-profile debug symbols and incremental compilation can
be disabled through Cargo environment variables when disk space is limited.

## Real v2 Component, RPC, and Issuer lifecycle

```sh
HTTP_PORT=18081 ICE_PORT=13478 REALM_MODE=provider REALM_HTTPS=1 \
  bash tests/e2e/actr/package-runtime-echo/joint-v2-issuer.sh
```

The joint runner preserves the successful runtime's Component until the Issuer
lifecycle matrix completes, then cleans up its temporary credentials/artifacts
on success. `KEEP_TMP=1` retains them for local diagnosis. Fixture directories
contain temporary private material; publish sanitized summaries rather than
attaching those directories to a PR.

The runner seeds the disposable Component workspace from the merged root
Cargo.lock, resolves its standalone root, and builds with `--locked`.

## Legacy regressions

```sh
HTTP_PORT=18082 ICE_PORT=13479 REALM_MODE=provider REALM_HTTPS=1 \
  bash tests/e2e/actr/package-runtime-echo/run.sh

HTTP_PORT=18083 ICE_PORT=13480 MODE=rotation \
  bash tests/e2e/actr/package-runtime-echo/run.sh
```

## Submission checks

```sh
cargo fmt
cargo clippy --fix --lib --bins --all-targets --allow-dirty
bash -n tests/e2e/actr/package-runtime-echo/run.sh
bash -n tests/e2e/actr/package-runtime-echo/joint-v2-issuer.sh
python3 tests/e2e/actr/package-runtime-echo/test_runtime_isolation.py
python3 tests/e2e/actr/package-runtime-echo/test_https_documents.py
bash scripts/actrium/check-retired-protocol-names.sh
git diff --check
```

Scan each staged/committed diff and its included files for secrets before
committing or pushing. Record the actual merged heads, toolchain, executed
commands, pass/skip counts, and E2E results in the integration PR.

## Execution record: 2026-11-12

Inputs:

- Review baseline: `cba08b46812d02311b197c2323cc6c53dbbd7bab`.
- Issuer PR #426: `a1690baff879a70cc4dc0932c360d11c244a3845`, subsequently merged
  into main as `90786d795f41b6f7964a5c3948057abd624ad689` (the integration base).
- Runtime PR #427: `db0b40ea66d4878d3ffb4f870ac7eea68c24b6a8`, including the
  author's signed RPC export-selection fix and its shared-contract regression.
- Integration branch: `codex/integrate-v2-runtime-issuer`.

After #426 entered main and #427 was rebased, the integration history was
updated onto that main. Exact tracked-tree equality with the tested union of
the original heads was verified before updating this execution record; no
runtime source content changed during that history update.

The local runner used macOS arm64, Native Rust 1.96.0, Component Rust 1.99.0,
Python 3.12.11, wasm-component-ld 0.5.26, and wasm-tools 1.253.0. The separate
Component compiler was invoked through rustup so its LLVM runtime libraries
were available. Cargo debug symbols/incremental output were disabled to limit
disk usage. The CLI's cached generated Web assets satisfied native build
prerequisites; browser runtime execution is outside this matrix.

| Executed check | Result |
| --- | --- |
| Nextest: six changed-domain libraries plus Issuer integration_test | 2111 passed; 4 existing ignored tests skipped; no retries needed |
| Shared-contract export regression from the author update | Passed within that Nextest run |
| Shared Host/Issuer fixture and original recovery-context tests | Passed within that Nextest run |
| Real v2 Component, Provider/HTTPS Realm, registration and Echo RPC | Passed; Echo succeeded before and after service restart |
| Component-backed real Issuer lifecycle matrix | 5 passed; no skips; includes stable ActorId, retry replay, device recovery, refusals and legacy-target clearing |
| Legacy cdylib, Provider/HTTPS Realm and restart/RPC | Passed; removed-member and retired-Realm refusals also passed |
| Legacy Publisher key retirement/revocation | Passed; retired key registered successfully, revoked key was refused |
| Runtime isolation Python tests | 2 passed |
| HTTPS document fixture Python tests | 7 passed |
| cargo fmt and final formatting check | Passed |
| Full-workspace cargo clippy --fix --lib --bins --all-targets --allow-dirty | Passed; no source fixes required |
| Shell syntax, retired protocol identifiers and CLI source-asset synchronization | Passed |
| Locked/offline standalone manifest metadata checks (no-deps) | Passed for examples, TypeScript, E2E and WinUI workload workspaces |

The initial shared-process `cargo test` run exposed four unrelated Hyper test
interactions involving global tracing and short timing budgets. All four passed
in the CI-standard process-isolated Nextest run, together with the rest of the
selected suites. No production behavior was changed to suppress those failures.
