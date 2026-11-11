# Package Runtime Echo E2E

This scenario validates the local package runtime flow end-to-end:

1. Start the in-tree `loran` test fixture from the scenario config.
2. Use the Admin API to create a dedicated realm and capture its owner invite;
   the server host joins as the owner (`actr realm join`), invites the client
   host as an editor (`actr realm invite --role editor`), and the client host
   joins with its own Realm client key. Each host signs its Issuer requests
   with its key (`deployment.realm_client_key`).
3. Seed the service publisher's public package key into the disposable SQLite database. The test-only server supplies a signed Publisher Profile for `e2e.actrium.example` through the Issuer's document-fetch interface, then runs normal signature, pin and Registry acceptance checks. Production binaries have no fixture override and always fetch public HTTPS documents. The client package remains a development identity, verified by its configured public key.
4. Generate a temporary Rust echo service with `actr init/install/gen`.
5. Build and verify the signed server package.
6. Build a local client guest package and run the client host.
7. Assert the client receives the echoed reply.
8. Remove the client's member (`actr realm remove-member`) and assert the same
   client host is refused a credential (`client_revoked`).

Formal Realm scenarios also repeat the Echo after a Service Host restart, then
retire the Realm and require the resolver to refuse it.

## Requirements

- `cargo`
- `curl`
- `jq`
- `sqlite3`
- `python3` 3.11 or later
- `perl`, `rustc`, `lsof`, `timeout`
- `openssl` for the formal matrix's disposable TLS certificates
- Linux: Landlock ABI 2+ enabled in the kernel, or `bubblewrap` with permission
  to create mount/PID namespaces.
- macOS: the system `sandbox-exec` command.

`loran` does not need to be preinstalled. The scenario builds its test binary with `cargo test --no-run --bin loran` and starts only the ignored `package_runtime_fixture` entry point. This avoids depending on public DNS for the virtual publisher domain while exercising the complete server and package runtime. An explicit `LORAN_BIN` must name that test binary.

## Run

```bash
bash tests/e2e/actr/package-runtime-echo/run.sh
bash tests/e2e/actr/package-runtime-echo/run.sh "Hello"

# Verify Publisher signing-key retirement and revocation.
MODE=rotation bash tests/e2e/actr/package-runtime-echo/run.sh
```

## Signed Realm creation matrix (PR #305)

```bash
# Both formal creation paths, each against a fresh single-node Loran.
bash tests/e2e/actr/package-runtime-echo/realm-matrix.sh

# Or run one path while iterating.
REALM_HTTPS=1 REALM_MODE=provider bash tests/e2e/actr/package-runtime-echo/run.sh
REALM_HTTPS=1 REALM_MODE=external bash tests/e2e/actr/package-runtime-echo/run.sh
```

The matrix defaults to `MODE=runtime` and enables the Provider's authority. It builds
the real package before creating the Realm, so the Profile pins its actual
SHA-256 and Actor type. `REALM_MODE=provider` creates `echo-<run>@localhost`
through the Admin API and verifies the Provider-signed Profile.
`REALM_MODE=external` publishes a separate domain's self-signed Realm keys, uses
the real CLI `create-request` / `sign-profile` commands, and checks the
pending-to-active transition through `/actr/realm-management`.

The matrix defaults to `REALM_HTTPS=1`. A disposable private CA and TLS server
serve the Realm/Publisher documents and actual `.actr` bytes. A restricted
loopback CONNECT tunnel preserves bootstrap URLs on HTTPS port 443 without
requiring root privileges or changing system DNS/trust. Both Rust test clients
and the test server explicitly trust that CA, with certificate/hostname checks
enabled. Each client resolves and verifies the signed Realm and Publisher
Profiles, downloads the package, checks its digest/signature/type, and only then
redeems its invite. The Service Host executes the downloaded package. Negative
cases require untrusted CA, wrong authority, bad digest and invalid Publisher
signature to fail before key creation or package-cache publication. The external
domain also signs Profiles selecting a type-mismatched package or an invalid
package signature with a matching digest; both must be refused.

`test_https_documents.py` checks the real TLS fixture, including wrong-hostname
rejection and the tunnel's destination allowlist. CI runs it before the matrix.
`REALM_STAGE=profile bash tests/e2e/actr/package-runtime-echo/realm-matrix.sh`
isolates discovery, download/verification, joining and retirement for diagnosis
on machines without usable RTC networking. Its result explicitly excludes Echo,
restart and live Actor revocation; CI retains the default full runtime matrix.
`REALM_HTTPS=0` retains the map-only diagnostic mode. Publisher rotation retains
its separate dynamic signed-document fixture (`MODE=rotation` defaults to
`REALM_HTTPS=0`), because it intentionally edits Registry key states directly.

Every formal Realm scenario, including the map-only diagnostic mode, runs its
Issuer and signaling endpoints over HTTPS/WSS to satisfy the signed Realm
Profile contract. Each run generates a separate disposable server certificate
and CA under `loran-tls/`. `SSL_CERT_FILE` adds that CA to a copy of the system
roots for the CLI and isolated Host processes; `CURL_CA_BUNDLE` configures curl.
Certificate and hostname verification remain enabled, and system trust is not
modified. Legacy scenarios without a Realm Profile retain HTTP/WS.

Both paths resolve and verify the Profile before redeeming owner/editor invites,
use independent Realm client keys, run the real Issuer, signaling and Host Echo
round-trip, repeat the round-trip after restarting the Service Host, remove the
editor and require `client_revoked`, then retire the Realm and require the
client resolver to refuse its retired Profile. Negative cases cover tampered
Profiles, single-use invites, and (external domain) invalid management signatures
and request replay. The test orchestrator translates the verified Profile into
the existing Host runtime configuration; automatic Host resolution is not added.

Before and after the lifecycle, the scenario requires the obsolete package-publication
tables to be absent. Its public Publisher Profile/key cache is provisioned separately.
The Issuer must verify the package as the trusted Publisher, rather than a
development identity.

## Publisher signing-key boundary (#345)

Package signing happens in the builder. Its
private keychains stay in a dedicated `publisher-private` directory. Both the
Service Host and the client Host run through `lib/runtime-isolation.py`, which
removes builder-specific environment variables and denies access to that
directory. Linux uses an inherited Landlock filesystem policy; kernels without
Landlock may use bubblewrap mount/PID namespaces. macOS uses a Seatbelt
file-access rule. The launcher fails if neither supported boundary is available.
There is no fallback to HOME-only isolation.

`test_runtime_isolation.py` first verifies the actual OS boundary: a disposable
signing key and its symlink cannot be read, public material and the client's own
Realm key remain usable, and builder-only environment variables are absent.
Writing the signing file or moving/linking it into an allowed directory must
also fail. Landlock allows filesystem siblings while leaving the signing
directory's ancestor chain without content-read or mutation permissions.
Before each Service startup the same launcher also probes the actual Publisher
keychain and Realm key. The signing keys remain on disk during execution, so a
missing-file cleanup cannot make this check pass accidentally. The existing
Issuer integration regression `test_trusted_package_registers_without_publisher_private_key`
also covers registration, renewal and reissue after dropping the Publisher key,
while checking that no package-publication tables exist.

Builder CLI commands also use a disposable HOME so `pkg keygen` cannot rewrite
the developer's global `~/.actr/config.toml`. Cargo/Rustup caches are explicitly
retained for subprocess builds; runtime HOME and key isolation stay separate.

This boundary protects the test's Publisher signing directory; it is not a
general-purpose sandbox for untrusted native workloads. The server fixture's
legacy/rotation Publisher Profile root stays inside the server process. In HTTPS
mode the builder's Profile root stays in the protected `publisher-private`
directory and the TLS origin serves only its public signed descriptor.
Production package signing continues to require its legitimate signing keys. Removed runtime proof fields
remain reserved in the protocol.

CI runs the default runtime, both formal Realm paths, and
the retired-key/revoked-key scenario. The latter is Publisher package-key
revocation coverage, separate from Realm Issuer-key rotation tests.
The lightweight `Runtime Publisher Key Isolation` workflow can also validate
Linux and macOS boundaries independently, without building Rust fixtures.

## Test transport and remaining coverage

**Transport boundary:** in HTTPS mode the map is the TLS origin's published
document store, not a replacement client fetcher. The server test binary and
explicit CLI lib-test entry point `realm_runtime_join_fixture` use real reqwest
HTTPS requests with the isolated test CA/tunnel. The entry point runs the
production Profile resolver and join implementation without `--issuer`, plus
package verification before joining. Membership and the Loran runtime control
plane use the test server's HTTPS/WSS endpoints with the run's disposable CA.
Signatures, domain binding, membership handlers and client proofs are not
stubbed. Production executables have no test CA, tunnel or document-map override.

The matrix enables real STUN/TURN and uses relay-only RTC, so the Echo also
requires usable Realm-scoped TURN credentials. Its test server permits private
relay peers only when both the existing package-runtime fixture marker and the
Realm document-map marker are set; production keeps its private-peer filter.
The advertised ICE address defaults to the IPv4 address selected by the OS's
default route (determined without sending a packet), with a loopback fallback
when no route exists. Override `REALM_ICE_IP` to select another reachable IPv4
address of the test machine. This changes the advertised test deployment
address, not identity checks or the RTC implementation.

This is protocol/runtime E2E with HTTPS package download, not public DNS/WebPKI
or Actrium UI E2E. The private-CA transport does not exercise production SSRF
DNS filtering. Host automatic Profile/package integration remains #324/#336;
the test orchestrator still translates a verified Profile into Host config.
It does not claim coverage for `.actr.alt` local Realms, Provider subdomains,
multi-node infrastructure, periodic external Profile refresh, or persistent
client trust-pin/rollback handling. Those need separate implementation or
deployment tests. Existing multi-Realm isolation and key-lifecycle suites remain
complementary coverage; the matrix does not replace them.

`KEEP_TMP=1` retains local logs and fixtures for diagnosis. The retained directory
contains generated private keys and invites: do not upload the whole directory.

Useful environment variables:

- `LORAN_BIN` supplies an already-built `loran` test binary containing `package_runtime_fixture`.
- `HTTP_PORT` and `ICE_PORT` override the default ports 8081 and 3478.
- `ACTR_TARGET_DIR`, `LORAN_TARGET_DIR`, `WORKSPACE_TARGET_DIR` and `TEMP_SERVICE_TARGET_DIR` allow build-cache reuse.
- `KEEP_TMP=1` keeps the `.tmp/run-*` directory after the run.
- `CLIENT_TIMEOUT_SECONDS` changes the client wait timeout.
- `RUST_LOG` forwards the log level to the host processes.
