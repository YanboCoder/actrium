# 12. Reference protocols, samples, and terminology

> This document gives a discussable v1 wire shape. Field names and endpoint paths may still be
> adjusted before the P0 schema freezes; object boundaries, identity, and security semantics must
> not change.

## 1. Realm link samples

Syntax: see [Actr URI](../../../actr/protocols/uri.md).

```text
actr://acme.com/k7m2q9xh                         ; open instance k7m2q9xh@acme.com
actr://acme.com/notes/today?filter=assigned      ; an explicit readable RealmId, with a page path and query
actr://acme.com/2af01.k7m2q9xh                   ; an Actor within the instance (ActorId 2af01.k7m2q9xh@acme.com)
actr://<installation-id>.actr.alt/h3x8p2wq       ; a local Realm, not shareable
```

By default Actrium generates the RealmId at random (a short lowercase base32 string, without the
program name); the readable name goes into the Realm Profile's `display_name`. The user can
explicitly switch to a readable RealmId on the create-confirmation page, as with `notes` in the
example above.

An Actor within an instance is identified by an ActorId, e.g. `2af01.k7m2q9xh@acme.com`: dropping
`2af01.` gives the Realm name. Its link form is `actr://acme.com/2af01.k7m2q9xh`; this kind of
link stops working once a program upgrade replaces the Actor, so instance links are the default
for sharing. The program identity `publisher/program@version` is carried by the package manifest
and the Actor credential.

Legacy forms (`actr://notes.example/team-notes@1.4.2?service=…`, or `2af01@42/notes.example:team-notes:1.4.2`
with only a numeric `realm_id`) are always rejected and can only be imported through an explicit
migration tool.

## 2. Publisher Profile

The Publisher Profile is the optional `publisher` member of the authority descriptor (already
defined in Actr Authority Discovery §4.5); its entry point is the Publisher domain's single
bootstrap resource:

```text
https://notes.example/.well-known/actr
```

Sample (the `publisher` member uses snake_case; `domain` must match the `authority` hostname):

```json
{
  "version": 1,
  "authority": "actr://notes.example",
  "services": [
    {
      "rel": "contracts",
      "href": "https://notes.example/actr/contracts"
    }
  ],
  "publisher": {
    "schema": "actr.publisher/v1",
    "sequence": 17,
    "domain": "notes.example",
    "profile_signing_keys": [
      {
        "id": "publisher-root-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "active"
      }
    ],
    "keys": [
      {
        "id": "actr-pub-3f9a1c0d2b7e4a61",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "not_before": "2026-04-01T00:00:00Z",
        "not_after": "2027-04-01T00:00:00Z",
        "status": "active"
      },
      {
        "id": "actr-pub-8c2e5b7d1a4f9e03",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "not_before": "2025-01-01T00:00:00Z",
        "not_after": "2026-06-30T00:00:00Z",
        "status": "retired"
      }
    ],
    "security_contact": "mailto:security@notes.example",
    "profile_issued_at": "2026-07-29T00:00:00Z",
    "profile_expires_at": "2026-08-29T00:00:00Z",
    "signature": {
      "key_id": "publisher-root-1",
      "algorithm": "Ed25519",
      "value": "<base64>"
    }
  }
}
```

The `publisher.signature` object itself does not enter the payload; the rest of the `publisher`
member's JSON is normalized per RFC 8785 JCS and verified as a detached Ed25519 signature.
`sequence` must increase monotonically. The parts of the descriptor outside `publisher` are
unsigned and play no part in Publisher trust decisions. `profile_signing_keys` and the
optional `rotations` follow the format, acceptance, and rotation rules of
`realm_signing_keys` (Actr Authority Discovery §4.4). A package signing key's `id` is computed
from its public key and equals the manifest `signing_key_id`; `keys` has at most 8 entries, at
most 2 of them `active`, and a rotation publishes the new key first and signs packages with it
only 24 hours or more later ([Actor Package](../../../actr/protocols/actor-package.zh.md) §6.8,
Chinese only).

## 3. Package download

Which package an instance runs is written into its Realm Profile's `package` (sample in §6.2):

```json
"package": {
  "href": "https://cdn.notes.example/team-notes-1.4.2.actr",
  "sha256": "<64 hex>",
  "type": "notes.example/team-notes@1.4.2"
}
```

Actrium downloads `href` per [Realm Profile protocol](../../../actr/protocols/realm-profile.md) §6
(an HTTPS GET without credentials), checks `sha256`, and then verifies the manifest signature with
the package signing key from the Publisher Profile (§2); the Actor type the manifest declares is
the instance's program. The download address can be anywhere (a Publisher CDN, Loran package
hosting, object storage) — it is not a trust source.

## 4. `.actr` Manifest

Illustrative:

```toml
publisher = "notes.example"
name = "team-notes"
version = "1.4.2"
signature_algorithm = "ed25519"
signing_key_id = "actr-pub-3f9a1c0d2b7e4a61"

[binary]
path = "bin/service.wasm"
target = "wasm32-wasip2"
kind = "component"
hash = "<sha256>"
size = 1832000

[metadata]
description = "Workspace-bound team notes"
license = "Apache-2.0"

[actrium]
schema_version = 1
min_actrium_version = "1.0.0"
front_host_api = "^1.0"
service_host_api = "^1.0"

[actrium.front]
guest_runtime = "keel-js"
guest_entry = "ui/guest.bundle.js"
keel_api = "^1.0"
contract_manifest = "ui/capability-manifest.json"
contract_digest = "sha256:<digest>"

[actrium.service]
contract = "proto/service.proto"
protocol_fingerprint = "sha256:<digest>"

[actrium.service.workspace]
requirement = "required"
provisioning = "user-selected"
access = "read-write"
providers = ["managed-store", "local-folder", "cloud-drive"]
lease = "exclusive-writer"

[actrium.session]
attachment_policy = "exclusive"
lease_seconds = 45
heartbeat_seconds = 15

[actrium.privacy]
schema_version = 1
data_classes = ["public", "private", "secret"]
default_unknown_action = "block-private-and-secret"

[[actrium.privacy.actor_egress]]
actr_type = "notes.example/index-worker@1.4.2"
required = false
data_classes = ["private"]

[actrium.permissions.front]
capabilities = [
  "service",
  "host:clipboard.write",
  "host:notifications"
]

[actrium.permissions.service]
capabilities = [
  "workspace:read-write",
  "net:https://api.notes.example",
  "actor:call:notes.example:index-worker:1"
]

[actrium.components]
allow = [
  "View",
  "Text",
  "ScrollView",
  "TextInput",
  "Button",
  "FlatList",
  "ImageAsset"
]

[actrium.resources.front]
heap_bytes = 67108864
js_turn_timeout_ms = 100
max_render_nodes = 10000
max_batch_ops = 5000
max_rpc_concurrency = 32

[actrium.resources.service]
memory_bytes = 268435456
max_mailbox = 4096
max_rpc_concurrency = 128
max_workspace_bytes = 10737418240

[[resources]]
path = "ui/guest.bundle.js"
hash = "<sha256>"

[[resources]]
path = "ui/capability-manifest.json"
hash = "<sha256>"

[[resources]]
path = "assets/icon.png"
hash = "<sha256>"

[[proto_files]]
name = "service.proto"
path = "proto/service.proto"
hash = "<sha256>"

[lock_file]
path = "manifest.lock.toml"
hash = "<sha256>"
```

The formal schema still needs to decide how to keep backward compatibility with the Actr base
manifest, and to ensure `[actrium]` fields also fall within the scope the manifest signature
covers.

A Native Capsule workload additionally needs:

```toml
[actrium.native_capsule]
profile = "privacy-closed-v1"
network = "broker-only"
workspace = "broker-only"
child_process = false
ptrace = false
shared_writable_memory = false
dynamic_libraries = "sealed"
ipc = ["actr.capability.v1"]
```

An in-process `NativeCdylib` does not use this profile, and must not be rated `CONTROLLED` on that
basis.

## 5. Keel Capability Manifest

```json
{
  "schemaVersion": 1,
  "keelApi": "1.0",
  "guestEntry": "ui/guest.bundle.js",
  "imports": [
    "react",
    "react/jsx-runtime",
    "keel/guest",
    "host:service",
    "host:clipboard"
  ],
  "hostModules": {
    "service": {
      "rpc": [
        "notes.v1.NoteService.List",
        "notes.v1.NoteService.Get",
        "notes.v1.NoteService.Save"
      ],
      "events": [
        "notes.v1.changed"
      ]
    },
    "clipboard": {
      "rpc": [
        "writeText"
      ]
    }
  },
  "components": [
    "View",
    "Text",
    "TextInput",
    "Button",
    "FlatList",
    "ImageAsset"
  ]
}
```

Actrium must not trust this file alone; the result of static installation analysis must agree with
it.

## 6. Realm signing key and Realm Profile

### 6.1 Realm signing key: authority descriptor extension

A domain's Realm signing key is published in that domain's descriptor:

```text
https://acme.com/.well-known/actr
```

Sample (the `realm_signing_keys` member and the `realms` relation are already defined in Actr
Authority Discovery §4.3–4.4):

```json
{
  "version": 1,
  "authority": "actr://acme.com",
  "services": [
    {
      "rel": "signaling",
      "href": "wss://signal.provider.example/actr",
      "protocol": "actr-signaling/1"
    },
    {
      "rel": "ice-config",
      "href": "https://rtc.provider.example/actr/ice",
      "protocol": "actr-ice-config/1"
    },
    {
      "rel": "realms",
      "href": "https://realms.provider.example/acme.com",
      "protocol": "actr-realm-profile/1"
    }
  ],
  "links": ["/i/"],
  "realm_signing_keys": {
    "sequence": 3,
    "keys": [
      {
        "id": "rs-2026-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "active"
      },
      {
        "id": "rs-2025-1",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "retired"
      }
    ],
    "rotations": [
      {
        "from": "rs-2025-1",
        "to": "rs-2026-1",
        "sequence": 2,
        "signature": "<base64>"
      }
    ],
    "signature": {
      "key_id": "rs-2026-1",
      "algorithm": "Ed25519",
      "value": "<base64>"
    }
  }
}
```

`realm_signing_keys.keys` is the long-term trust root for every Realm under that domain;
`rotations` holds one entry per rotation, each signed by the `from` key over the JCS form of
`{from, to, public_key, sequence}`, where `sequence` is the member `sequence` at which that
rotation was introduced. Removing a key from `keys` revokes every Realm Profile it signed.
`realm_signing_keys.signature` is signed by some `active` key over the JCS-normalized result of
that member with `signature` removed, covering `sequence`. The rest of the descriptor is unsigned
and does not list a Realm inventory. The optional `links` member is retained as descriptor metadata
(Actr Authority Discovery §4.6), but Actrium does not use it to parse or share Realm links; Realm-link
input and sharing use `actr://` only. The signaling, ice-config, and similar entries under `services` serve that
authority itself and have nothing to do with the Realms under that domain; accessing a Realm only
ever uses the Actr Facility Provider named in its own Realm Profile, and no other Actr Facility Provider may be
used when that Realm Profile is unavailable, invalid, or retired. An Actr Facility Provider's own descriptor
(`https://provider.example/.well-known/actr`) separately publishes its management-request endpoint
via a `realm-management` link (protocol `actr-realm-management/1`).

### 6.2 Realm Profile

The entry point is the `realms` relation's `href` plus the `realm_id`:

```text
https://realms.provider.example/acme.com/k7m2q9xh
```

Sample:

```json
{
  "schema": "actr.realm/v1",
  "sequence": 31,
  "realm": {
    "name": "k7m2q9xh@acme.com",
    "display_name": "Acme Notes",
    "status": "active"
  },
  "package": {
    "href": "https://cdn.notes.example/team-notes-1.4.2.actr",
    "sha256": "<64 hex>",
    "type": "notes.example/team-notes@1.4.2"
  },
  "issuer": {
    "url": "https://identity.provider.example/realms/acme.com/k7m2q9xh",
    "keys": [
      {
        "id": "7",
        "algorithm": "Ed25519",
        "public_key": "<base64>",
        "status": "active"
      }
    ]
  },
  "provider": {
    "id": "provider.example",
    "signaling": [
      {
        "url": "wss://signal-a.provider.example/ws",
        "priority": 10,
        "control_plane_id": "provider.example/v1",
        "ticket_audience": "signal-a"
      },
      {
        "url": "wss://signal-b.provider.example:8443/ws",
        "priority": 20,
        "control_plane_id": "provider.example/v1",
        "ticket_audience": "signal-b"
      }
    ],
    "ice": [
      {
        "urls": ["stun:stun.provider.example:3478"]
      },
      {
        "urls": ["turns:turn.provider.example:5349"],
        "credential_audience": "turn-pool-1"
      }
    ]
  },
  "policy": {
    "membership": "invite-only"
  },
  "issued_at": "2026-07-29T00:00:00Z",
  "signature": {
    "key_id": "rs-2026-1",
    "algorithm": "Ed25519",
    "value": "<base64>"
  }
}
```

`signature` is signed by an `active` key from `realm_signing_keys`; an Actor credential's issuer is
exactly `realm.name`, and where the Realm Profile is stored does not affect its validity. An
existing Realm Profile signed by a key that was later retired remains valid until superseded by a
higher `sequence`. A Realm Profile carries no expiry; rollback protection relies solely on
`sequence`. `provider.id` is the Actr Facility Provider's normalized DNS authority and is immutable for the
life of the Realm. Multiple signaling endpoints sharing a `control_plane_id` are replicas or
failover entry points of the same Actr Facility Provider presence/control plane, not v1 federation.
`package` gives the package the instance runs (§3); upgrading the program means publishing a Realm
Profile with a different `package` and a higher `sequence`. Rotating the Realm issuer key or
changing endpoints under the same Actr Facility Provider likewise means publishing a Realm Profile with a
higher `sequence`; retiring publishes a new Realm Profile with `status` set to `retired`, valid
forever after.

## 7. Service Inventory Record

```json
{
  "realm": "notes@acme.com",
  "serviceActorId": "2af01.notes@acme.com",
  "program": {
    "publisher": "notes.example",
    "name": "team-notes",
    "version": "1.4.2",
    "packageDigest": "sha256:..."
  },
  "workspaceId": "ws_01...",
  "runtime": {
    "placement": "local",
    "hostDeviceId": "device_...",
    "lifecycle": "running",
    "desiredState": "running",
    "reachability": "online",
    "health": "healthy"
  },
  "attachment": {
    "state": "leased",
    "policy": "exclusive"
  },
  "createdAt": "2026-07-29T10:00:00Z",
  "lastOpenedAt": "2026-07-29T11:00:00Z"
}
```

Does not include the Actor private key, a Workspace credential, or a signaling/TURN token.

## 8. Permission Grant

```json
{
  "subject": {
    "publisher": "notes.example",
    "program": "team-notes",
    "majorVersion": 1,
    "packageDigest": "sha256:..."
  },
  "capability": "host:clipboard.write",
  "scope": {
    "serviceActorId": "2af01.notes@acme.com",
    "workspaceId": "ws_01..."
  },
  "decision": "allow-while-open",
  "source": "user",
  "grantedAt": "2026-07-29T10:00:00Z",
  "expiresAt": null
}
```

## 9. Front Launch Context

Read-only for the Guest:

```json
{
  "program": {
    "publisher": "notes.example",
    "name": "team-notes",
    "version": "1.4.2"
  },
  "route": "/today",
  "query": {
    "filter": "assigned"
  },
  "session": {
    "connection": "p2p",
    "realmDisplayName": "Acme Notes"
  },
  "workspace": {
    "displayName": "Team Notes",
    "mode": "read-write"
  },
  "theme": "system",
  "locale": "zh-CN"
}
```

Not provided:

- Front/Service credential;
- session id/secret;
- Workspace path;
- an arbitrary Actor-call API;
- Realm client key;
- signaling/TURN endpoint credential.

### 9.1 Privacy state context

The Guest sees only a minimized, user-facing summary:

```json
{
  "privacy": {
    "color": "green",
    "ready": true,
    "policyId": "actrium.consumer.default",
    "workloadKind": "wasm-component",
    "applicationStateDigest": "…",
    "expiresAt": "2026-07-29T12:01:15Z"
  }
}
```

The Guest does not get the Actor graph, raw evidence, an attestation token, or a hidden ActorId.
For the full object see [13](13-privacy-state-and-aggregation.md); for semantics see
[docs/security](../../architecture/security/actrium/README.zh.md).

## 10. System protocol error codes

### URI

```text
URI_SCHEME_INVALID
URI_DOMAIN_INVALID
URI_LABEL_INVALID
URI_LEGACY_FORM
URI_LOCAL_REALM_FOREIGN
```

### Package

```text
PUBLISHER_PROFILE_INVALID
PUBLISHER_PROFILE_EXPIRED
PUBLISHER_PROFILE_ROLLBACK
PUBLISHER_IDENTITY_CHANGED
PACKAGE_DOWNLOAD_FAILED
PACKAGE_DIGEST_MISMATCH
PACKAGE_EQUIVOCATION
PACKAGE_SIGNATURE_INVALID
PACKAGE_KEY_REVOKED
PACKAGE_ARCHIVE_INVALID
PACKAGE_ABI_UNSUPPORTED
PACKAGE_REVOKED
GUEST_POLICY_INVALID
SERVICE_IMPORT_FORBIDDEN
```

### Realm/Identity

```text
REALM_KEYS_INVALID
REALM_DOMAIN_UNTRUSTED
REALM_SIGNING_KEY_CHANGED
REALM_PROFILE_UNAVAILABLE
REALM_PROFILE_INVALID
ACTR_FACILITY_PROVIDER_MISMATCH
REALM_PROFILE_ROLLBACK
REALM_RETIRED
REALM_LOGIN_REQUIRED
REALM_MEMBERSHIP_REVOKED
CLIENT_REVOKED
INVITE_EXPIRED
INVITE_ALREADY_USED
INVITE_REVOKED
INVITE_REALM_MISMATCH
```

### Service/Workspace

```text
SERVICE_CREATE_FAILED
SERVICE_IDENTITY_RECOVERY_FAILED
SERVICE_STOPPED
SERVICE_SUSPENDED
SERVICE_OFFLINE
SERVICE_UNREACHABLE
SERVICE_NOT_FOUND
SERVICE_ACTOR_TYPE_MISMATCH
SERVICE_MIGRATING
SERVICE_TRASHED
SERVICE_DELETED
WORKSPACE_REQUIRED
WORKSPACE_UNAVAILABLE
WORKSPACE_PERMISSION_DENIED
WORKSPACE_WRITE_LEASE_CONFLICT
WORKSPACE_QUOTA_EXCEEDED
```

### Session

```text
ATTACH_PERMISSION_DENIED
ATTACH_IDENTITY_MISMATCH
ATTACH_PROGRAM_MISMATCH
ATTACH_DIGEST_MISMATCH
ATTACH_PROTOCOL_MISMATCH
ATTACH_SESSION_OCCUPIED
ATTACH_REALM_MISMATCH
SESSION_EXPIRED
SESSION_REPLAYED
SESSION_TAKEOVER_DENIED
```

### Guest/Capability

```text
GUEST_LOAD_FAILED
GUEST_RUNTIME_TIMEOUT
GUEST_HEAP_EXCEEDED
GUEST_RENDER_LIMIT_EXCEEDED
CAPABILITY_NOT_DECLARED
CAPABILITY_NOT_GRANTED
CAPABILITY_PLATFORM_DENIED
CAPABILITY_RATE_LIMITED
CONTRACT_INPUT_INVALID
CONTRACT_OUTPUT_INVALID
```

### Assurance

```text
ASSURANCE_INVALID
ASSURANCE_EXPIRED
ASSURANCE_REVOKED
ASSURANCE_SUBJECT_MISMATCH
ASSURANCE_SELF_SIGNED
ASSURANCE_CONFLICT_WEAKEST
POLICY_UNSUPPORTED
GRAPH_INCOMPLETE
SNAPSHOT_NOT_CURRENT
GRAPH_LIMIT_EXCEEDED
ACTOR_SUSPENDED
UNDECLARED_EGRESS
GENERATION_VIOLATION
TERMINAL_NOT_AUTHORIZED
NATIVE_NOT_MEASURABLE
CHANNEL_INVALID
```

## 11. State enumerations

### Program Release

```text
discovered
resolving
downloading
verifying
installed
update-available
identity-changed
revoked
quarantined
incompatible
```

### Service lifecycle

```text
creating
stopped
starting
running
suspended
stopping
migrating
archived
trashed
deleting
deleted
```

### Service desired state

```text
running
stopped
archived
```

### Service reachability

```text
online
offline
unreachable
unknown
```

### Service health

```text
healthy
degraded
failed
unknown
```

### Attachment

```text
idle
leased
occupied
```

### Application color

```text
green
yellow
red
unavailable
```

### Privacy enforcement

```text
allow
block
```

### Shell navigation

```text
parsing
resolving-package
awaiting-confirmation
creating-service
attaching
negotiating
loading-guest
failed
```

### Resident Front

```text
loading
running
suspended
failed
closing
closed
```

## 12. Glossary

The terms used in this document are defined in one place, the [glossary](../../GLOSSARY.zh.md);
they are not repeated here.

## 13. Specification and implementation references

### Actrium

- [Actr repository](https://github.com/Actrium/actr)
- [Keel repository](https://github.com/Actrium/keel)
- [Actr ActorId/ActrType formatting](https://github.com/Actrium/actr/blob/main/core/protocol/src/actr_ext.rs)
- [Actr package manifest](https://github.com/Actrium/actr/blob/main/core/pack/src/manifest.rs)
- [Actr package builder](https://github.com/Actrium/actr/blob/main/cli/src/commands/package_build.rs)
- [Keel Guest–Host interaction](https://github.com/Actrium/keel/blob/main/docs/architecture/guest-host-interaction.md)
- [Keel Guest runtime](https://github.com/Actrium/keel/blob/main/docs/architecture/guest-runtime.md)
- [Keel contract](https://github.com/Actrium/keel/blob/main/src/contract/index.ts)

### Internet standards

- [RFC 3986: URI Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986)
- [RFC 7595: URI Scheme Guidelines](https://www.rfc-editor.org/rfc/rfc7595)
- [RFC 5890: IDNA Definitions](https://www.rfc-editor.org/rfc/rfc5890)
- [RFC 8785: JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)
- [RFC 8489: STUN](https://www.rfc-editor.org/rfc/rfc8489)
- [RFC 8656: TURN](https://www.rfc-editor.org/rfc/rfc8656)
- [RFC 9530: TURN Third-Party Authorization](https://www.rfc-editor.org/rfc/rfc9530)
- [The Update Framework Specification](https://theupdateframework.github.io/specification/latest/)
