# Swift EchoApp E2E

End-to-end test that verifies an iOS app (EchoApp, linked runtime) can discover and call a remote EchoService through loran signaling + WebRTC — all running locally on CI.

## Architecture

```mermaid
graph LR
    subgraph iOS Simulator
        EA[EchoApp<br/>ActrNode.linked]
        LES[LocalEchoService<br/>swift-local:msg → remote]
    end

    subgraph loran
        WS[WebSocket Signaling]
        Issuer[Issuer Registration]
        REG[Publisher Package Registry]
    end

    subgraph Host Process
        ES[EchoService<br/>Rust cdylib package]
    end

    EA -->|discover| WS
    EA -->|register| Issuer
    ES -->|register| Issuer
    ES -->|publish| REG
    EA -->|WebRTC RPC| ES
    ES -->|echo reply| EA
    WS -->|SDP/ICE relay| EA
    WS -->|SDP/ICE relay| ES
```

## Flow

```mermaid
sequenceDiagram
    participant CI as run.sh
    participant AX as loran
    participant CLI as actr CLI
    participant ES as EchoService host
    participant SIM as iOS Simulator

    CI->>AX: start loran
    CI->>AX: health check
    CI->>AX: admin login
    CI->>AX: create realm → realm + owner invite (actr realm join)
    CI->>AX: register publisher + approve

    CI->>CLI: actr init --template echo --role service
    CLI-->>CI: scaffold echo-service
    CI->>CLI: actr deps install && actr gen
    CI->>CLI: actr build → .actr package

    CI->>CI: prepare temporary EchoApp workspace
    CI->>CI: render actr.toml from template
    CI->>SIM: boot iOS Simulator

    CI->>CLI: actr run (EchoService host)
    ES->>AX: WebSocket signaling connect
    ES->>AX: Issuer register
    CI->>CI: check_service_ready (process alive, signaling health, cache populated)
    CI->>CLI: actr deps install && actr gen -l swift
    CI->>SIM: xcodegen generate
    CI->>SIM: xcodebuild (build EchoApp for Simulator)
    CI->>CI: check_service_ready after build

    CI->>SIM: simctl install EchoApp.app
    CI->>SIM: simctl launch (AUTO_SEND=1)

    SIM->>AX: WebSocket signaling connect
    SIM->>AX: Issuer register
    SIM->>AX: discover EchoService
    SIM->>ES: WebRTC RPC: LocalEcho → EchoService
    ES-->>SIM: echo reply
    SIM-->>CI: print("ACTR_E2E_RESULT:...")
    CI->>CI: assert result contains test message
```

## File Structure

```
tests/e2e/actr/swift-echo-app/
├── run.sh                # CI orchestration script
├── actr.toml.tpl         # Runtime config template (rendered → actr.toml)
├── actr.lock.toml         # Runtime lock placeholder bundled with EchoApp
├── manifest.toml         # Package identity + EchoService dependency
├── project.yml           # XcodeGen project spec + scheme env vars
├── protos/
│   ├── local/local_echo.proto
│   └── remote/echo/echo.proto
└── EchoApp/
    ├── ActrService.swift  # Linked runtime + echo call logic
    ├── ContentView.swift  # UI + ACTR_E2E_RESULT print marker
    ├── EchoApp.swift
    ├── Info.plist
    └── Generated/         # protoc + actr gen outputs
```

## Verification Mechanism

EchoApp runs with `ACTR_ECHOAPP_AUTO_SEND=1` and `ACTR_ECHOAPP_TEST_INPUT=<message>` passed by `run.sh` through `SIMCTL_CHILD_*` launch environment variables. This triggers:

1. `ActrService.startIfNeeded()` — connects to loran, discovers EchoService
2. `ContentView.sendEcho("<message>")` — RPC call through LocalEchoService → EchoService
3. `print("ACTR_E2E_RESULT:\(output)")` — stdout marker captured by `run.sh`

`run.sh` greps the Simulator console log for `ACTR_E2E_RESULT:` and asserts the reply contains the test message.

## Run

```bash
# Local (macOS only)
bash tests/e2e/actr/swift-echo-app/run.sh

# Custom message
bash tests/e2e/actr/swift-echo-app/run.sh "Hello"

# Keep artifacts on failure (diagnostics + sanitized logs)
KEEP_TMP=1 bash tests/e2e/actr/swift-echo-app/run.sh

# Capture diagnostics even on success (uploads to CI artifact)
CAPTURE_DIAGNOSTICS_ON_SUCCESS=1 bash tests/e2e/actr/swift-echo-app/run.sh
```

### Diagnostics

On failure (or when `CAPTURE_DIAGNOSTICS_ON_SUCCESS=1`), `run.sh` captures:

- Process status (loran, EchoService)
- Signaling health endpoint response
- `signaling_cache.db` service registrations and status
- Filtered logs: heartbeat, disconnect, cleanup, ghost, ACL, errors

Sensitive values (admin password, admin token) are redacted before upload.
Diagnostics are written to `.tmp/run-*/diagnostics/` and sanitized copies to `.tmp/run-*/sanitized-logs/`.

## CI

Defined in `.github/workflows/ci-e2e.yml` → `swift-echo-app-e2e` job.

| Trigger | Condition |
|---------|-----------|
| Schedule | Daily UTC 18:00 |
| Manual | Actions → "CI (E2E)" → Run workflow |
| Push | ❌ No (heavy, not a PR gate) |

Runner: `macos-latest`, timeout 240 min.

Diagnostic artifacts are uploaded via `actions/upload-artifact@v4` (always, 7-day retention).
Download from the CI run's Artifacts section: `swift-echo-app-e2e-logs-*`.
