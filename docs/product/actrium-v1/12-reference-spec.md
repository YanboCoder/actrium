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
from its public key and equals the manifest `signature.key-id`; `keys` has at most 8 entries, at
most 2 of them `active`, and a rotation publishes the new key first and signs packages with it
only 24 hours or more later ([Actor Package](../../../actr/protocols/actor-package.zh.md) §6.8,
Chinese only).

## 3. Package download

Resolve `primary_package` and `front_package` in the verified Realm Profile. For each selected archive, fetch `href` over HTTPS without credentials, verify its SHA-256, and verify `manifest.sig` with the Publisher key for its exact Actor type. Check the Front's required Service dependency and RPC contract before loading. See [§6.2](#62-realm-profile) for the complete example.

## 4. `.actr` Manifest

Team Notes uses a separate signed `actr-package/2` archive for each Workload. File sizes, digests,
and the key ID are illustrative.

### 4.1 Service package

The WASM Component is this package's Workload. Its only Proto file is the Service RPC contract.

```toml
format = "actr-package/2"

[actor]
publisher = "notes.example"
name = "team-notes"
version = "1.4.2"

[workload]
profile = "actr.component/1"
entry = "workload/service.wasm"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = []
optional-capabilities = []

[workspace]
requirement = "required"
access = "read-write"
providers = ["managed-store", "local-folder", "cloud-drive"]

[signature]
algorithm = "ed25519"
key-id = "actr-pub-3f9a1c0d2b7e4a61"

[[files]]
path = "workload/service.wasm"
role = "workload"
media-type = "application/wasm"
size = 1832000
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/service.proto"
role = "contract"
media-type = "text/plain"
size = 1204
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"

[[files]]
path = "contracts/privacy.toml"
role = "contract"
media-type = "application/toml"
size = 530
sha256 = "50037be0a33ed2265fe5a79b3f6a0b36762d81de6bff066ea33c6a9d21f0f171"
```

### 4.2 Front package

The Keel Guest is this package's Workload. `workload.keel.contract` selects its Keel contract.
`dependencies.service.contract` selects the Proto used to generate the Guest's Service RPC codecs.

```toml
format = "actr-package/2"

[actor]
publisher = "notes.example"
name = "team-notes-front"
version = "1.4.2"

[workload]
profile = "keel.guest/1"
entry = "workload/guest.js"

[workload.keel]
runtime = "js"
guest-api = "keel.guest/1"
capabilities = "workload/capabilities.json"
contract = "contracts/notes.json"

[host]
profile = "actr.host/1"
api = "^1.0.0"
required-capabilities = ["actor.call", "ui.render"]
optional-capabilities = []

[dependencies.service]
actor = "notes.example/team-notes"
version = "=1.4.2"
contract = "contracts/service.proto"
required = true

[signature]
algorithm = "ed25519"
key-id = "actr-pub-3f9a1c0d2b7e4a61"

[[files]]
path = "workload/guest.js"
role = "workload"
media-type = "text/javascript"
size = 18432
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "workload/capabilities.json"
role = "profile"
media-type = "application/json"
size = 746
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/notes.json"
role = "contract"
media-type = "application/json"
size = 512
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[files]]
path = "contracts/service.proto"
role = "contract"
media-type = "text/plain"
size = 1204
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
```

The two examples use the same `contracts/service.proto` bytes. Each archive has its own `manifest.sig`.
Deployment settings such as Realm, signaling, and trust are supplied through `actr.toml`.

## 5. Keel Capability Manifest

```json
{
  "schemaVersion": 1,
  "keelApi": "1.0",
  "guestEntry": "workload/guest.js",
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

In the [complete signed Echo example](../../../actr/protocols/examples/realm-profile.json), `primary_package: "echo"` selects the Service and `front_package: "ui"` selects its Front. The Realm signs the complete Profile and increases `sequence` whenever its selection changes. See [Realm Profile §4](../../../actr/protocols/realm-profile.md#4-profile-format) for the test public key and field definitions.

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
