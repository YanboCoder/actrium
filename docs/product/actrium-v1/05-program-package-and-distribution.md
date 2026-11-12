# 05. Program Package and HTTPS Distribution

## 1. Three layers of identity

The distribution design must keep three things separate:

| Layer | Identifier | What it proves |
|---|---|---|
| Publisher | Normalized domain name | Who currently controls the publishing entry point |
| Program Release | `publisher/program@version` and its pinned archive digest | Which version of an Actor package was published |
| Service | ActorId + Actor credential | Which running instance is currently being talked to |

HTTPS cannot prove a program is not malicious; Marketplace review cannot prove an Actor's identity on the network; an Actor credential cannot prove that the Guest loaded locally is the correct version either.

## 2. Publisher identity

### 2.1 The domain is the Publisher ID

It is reasonable for a Publisher to use a domain name as its convention, because it directly reuses:

- DNS global uniqueness;
- domain ownership management;
- CA/TLS certificates;
- HTTPS;
- certificate transparency;
- CDN, cache, proxy and enterprise network facilities;
- an organization's existing security-response capability.

Actrium does not maintain a separate, global Publisher ID registry.

### 2.2 Publisher Profile

The Publisher Profile is published as the optional `publisher` member of the authority descriptor, in the domain's single bootstrap resource:

```text
GET https://<publisher>/.well-known/actr
```

The `publisher` member is defined in Actr Authority Discovery §4.5; per the authority-discovery rules, no sub-paths are defined under `/.well-known/actr`, so Publishers do not get a separate well-known file. The same descriptor may also carry `realm_signing_keys` (if the domain also has Realms) and `services`.

The bootstrap URL is derived directly from the Publisher's identity domain and must use standard HTTPS on port 443.

Fetching the descriptor follows the redirect rules of authority discovery: at most 5 hops, each over HTTPS; a redirect target never becomes a new authority, and the `authority` binding is still validated against the original Publisher domain. The validity of the `publisher` member depends only on its own signature and the pinned root/key history, not on the origin it was fetched from.

The `publisher` member's fields use snake_case and contain at least:

- `schema`: `actr.publisher/v1`;
- `domain`: the normalized Publisher domain, which must match the hostname of the descriptor's `authority`;
- current and historical Publisher Profile signing keys (`profile_signing_keys`);
- current and historical package signing keys;
- key id, algorithm, validity period and revocation status;
- a security contact address;
- optional Marketplace/transparency-log statements.

On first retrieval, trust comes from the Web PKI of the URL hostname, or from an enterprise-provisioned Publisher root pin; a root carried inside the Publisher Profile cannot vouch for itself in a circular way. After accepting the first Publisher Profile, Actrium pins the root/key history, and every later Publisher Profile must:

- be signed over a payload normalized per RFC 8785 JCS (the `publisher` member with its `signature` field removed), then verified as a detached Ed25519 signature;
- carry a monotonically increasing `sequence`, an issue time and an expiry time;
- introduce any new root only through a rotation chain signed by the old root;
- be rejected on rollback, expiry or unauthorized root change.

Parts of the descriptor outside `publisher` are unsigned and play no part in Publisher trust decisions.

An offline root is a strongly recommended Publisher operating practice, but whether hardware/offline storage is required can be raised further by Marketplace or enterprise policy; the wire protocol must support it.

### 2.3 Domain transfer

Control of a domain only determines the future publishing entry point; it must not silently rewrite history:

- an installed release can run offline with its accepted selection and verified packages.
- when a new key has no verifiable rotation chain from the historical key, treat it as a Publisher ownership change;
- an already-installed version must not change just because the current domain now returns a different package;
- a new install or update must display the identity change;
- high-risk policy may require re-approval by the Marketplace or an organization admin.

## 3. Where a package comes from

Publishers distribute independently signed archives. When creating a Realm, choose the Service and Front from the Program Catalog, Marketplace, Publisher or local imports. Record their exact types, download URLs and digests in `packages` and sign the Realm Profile. A local Realm stores the selection on the device. When opening a networked instance, use its [signed Realm Profile](../../../actr/protocols/realm-profile.md#6-using-a-realm).

## 4. Service and Front packages

### 4.1 Package boundaries

Each `actr-package/2` archive declares one Actor type and one Workload. The Host checks each selected package's Workload Profile against its role and distribution policy. See [Actor Package](../../../actr/protocols/actor-package.zh.md).

- **Service package.** `[workload]` selects the Service executable. The Realm Profile's resolved primary `packages[primary_package]`
  points to this archive. A Marketplace Service uses `actr.component/1`.
- **Front package.** `[workload]` selects the Keel Guest under `keel.guest/1`.
  `[workload.keel]` names its runtime, capability manifest, and Keel contract.
  Product v1 reserves the `service` dependency alias in the Front manifest. It must declare
  `required = true`. Its `actor` and `version` constraint must match the Service selected by the Realm.

Each package's entry has role `workload`. The Guest capability manifest has role `profile`,
the Keel and Proto contract files have role `contract`, and icons and static assets have role `resource`.

### 4.2 Recommended layout

```text
team-notes.actr
├── manifest.toml
├── manifest.sig
├── workload/
│   └── service.wasm
└── contracts/
    └── service.proto

team-notes-front.actr
├── manifest.toml
├── manifest.sig
├── workload/
│   ├── guest.js
│   └── capabilities.json
└── contracts/
    ├── notes.json
    └── service.proto
```

Each archive carries its Publisher signature in `manifest.sig`, covering the manifest and its payload hashes.

### 4.3 What a package must not contain

A formal third-party package must not contain:

- a Front Host binary;
- an alternative Keel Host implementation;
- a custom Host Module implementation;
- React Native native plugins;
- a Front dynamic library;
- executable scripts not listed in the manifest;
- a Guest entry point that is swapped in from the network at runtime.

The Actrium client supplies the Front Host. The Publisher supplies the Keel Guest Workload and its declarations in the Front package.

## 5. Service workload policy

### 5.1 Marketplace default tier

In v1, a third-party Service workload on the Marketplace must be:

- WebAssembly Component Model;
- targeting an ABI on Actrium's supported list;
- passing the import/export allowlist;
- subject to fuel, wall-clock timeout, memory, table, stream and I/O quotas;
- brokered by the Service Host for all Workspace, network and Actor capability.

### 5.2 Native Service

Actr already has native cdylib/linked workload capability, but that does not mean the Marketplace should allow it by default:

- a native workload carries higher risk together with the Actrium process;
- platforms such as iOS forbid arbitrarily downloading and executing native code;
- dynamic libraries have ABI, signing and supply-chain differences.

So Native Service applies only to:

- Actrium's own first-party system programs;
- enterprise-managed distribution;
- desktop development mode;
- an elevated trust tier the user has explicitly accepted.

Native Service does not change the rule that Front only ever allows a Keel Guest.

Native workloads must be further distinguished:

- `NativeCdylib`: loaded into the Actr Host's address space; it cannot prove a closed egress boundary, and third-party instances must not be shown green;
- `NativeCapsule`: an independent, restricted process; it can only be assessed as `CONTROLLED` when OS sandboxing, no raw network/file access, sealed libraries and brokered IPC are all in effect;
- `NativeTrusted`: TCB shipped by Actrium itself or preset by an enterprise; not part of the open Marketplace trust tier.

The v1 Marketplace still opens only WASM Services. Native Capsule is a desktop-first follow-on capability; see [Privacy Marks and Aggregation Protocol](13-privacy-state-and-aggregation.md) for the constraints.

### 5.3 UI-only programs

v1 does not introduce a special case for "a page app with no Service." The toolchain generates a platform-defined, minimal WASM Service Component for a UI-only program:

- it still has the same ActrType and an independent ActorId as the Program Release;
- it has no Workspace or outbound network capability by default;
- it implements only session, health and an optional minimal key-value contract;
- it still goes through package signing, resource limits and the Service lifecycle the same way;
- a Publisher cannot use it to bypass the Host API or inject native code.

So every app Tab still uniformly Attaches to a Service; the only difference is whether that Service contains custom logic.

## 6. Package manifest fields

The Actr base manifest provides the following tables.

- `[actor]` for Publisher, program name, and release version.
- `[workload]` for each package's profile and entry, and `[host]` for its Host API and capability requirements.
- `[dependencies.<alias>]` for required Actor types, version constraints, and contract paths.
- `[signature]` for the signature algorithm and `key-id`.
- `[[files]]` for payload roles, sizes, and SHA-256 hashes, including any lock file.
- `[workspace]` in the Service manifest for static `requirement`, `access`, and `providers` declarations.

For a Product v1 pair, `dependencies.service.contract` selects the RPC Proto by package-relative path.
During pair verification, both Hosts read this selector from the verified Front manifest.
The Service Host reads the Front selected by `front_package` and retains
the verified selector for Attach, then reads the selected Proto from its own verified Service archive.
Both manifests must declare the file at that path with role `contract` and the same SHA-256.
The explicit path selects the RPC contract when the archives carry other contract files. Actr Host
derives the Attach fingerprint as described in [06 §2.2](06-runtime-and-protocol.md#22-front-actor-type).

Service nodes use the existing Actr `actr.toml` schema for deployment settings such as Realm,
signaling, and trust. The [node configuration definition](../../../crates/actr-config/src/actr_raw.rs)
describes these fields. Workspace requirements come from the signed Service manifest's `[workspace]`
table. The deployment or creation flow selects `provisioning`, and the Host stores the binding and grant per instance.

See both manifests in the [Reference Specification](12-reference-spec.md#4-actr-manifest).

## 7. Capability declarations and runtime policy

What a package declares is a "request," not an actual grant:

```text
effective capabilities =
  package requested
  ∩ Actrium Host supported
  ∩ user/admin granted
  ∩ current platform allowed
  ∩ current session scope
```

The Actr Host evaluates the following as product policy. Package capability requirements come
from `[host]`, and Guest declarations come from its signed Keel capability manifest.
The signed Service manifest declares Workspace requirements. Deployment and authorization supply
the instance-specific settings and grants.
Guest heap, execution time, render-operation limits, and Service memory, fuel, storage, and network
quotas come from platform or deployment policy and are enforced by the Host.
Minimum Actrium client and supported SDK/toolchain versions are platform release policy.
Package compatibility is checked through `host.api`, `workload.profile`, `workload.keel.guest-api`,
and the verified RPC contract fingerprint.

- Front capability;
- Service capability;
- the set of Keel components;
- outbound network domains;
- camera/microphone/clipboard/notification/location;
- Workspace access mode;
- Guest heap, single-execution time, render ops;
- Service memory/fuel/storage/network;
- Host API and Workload/Guest contract compatibility under the supported platform toolchain;
- data classes, expected Actor egress, terminal egress, required/optional, and the minimum node conclusion.

Adding a new high-risk permission requires a new version, and reconfirmation at upgrade time.

The Privacy Declaration is a static allowlist and installation notice, not the authoritative source of the runtime privacy graph. The Actr Host must check WASM imports and actual capability against the actual edges against the declaration; a Publisher marking a dependency as optional cannot override the closure judgment the Actr Host makes from the real data flow.

## 8. Download and verification pipeline

```text
Realm Profile selection / locally selected packages
  → verify the Service package
  → verify the Front package and its Service dependency
  → policy + permission review
  → content-addressed cache
  → Service Runtime / verified Guest bytes
```

### 8.1 Check order

Verify the Realm Profile, then apply these checks to each selected archive. Its type and whole-archive SHA-256 must match the corresponding entry. Resolve the Front's `dependencies.service` against the verified Service.

1. verify the Publisher's DNS/IDNA;
2. TLS and hostname;
3. download size limit;
4. package SHA-256;
5. ZIP paths, STORE method, file count, and size limits.
6. The `actr-package/2` format and `signature.key-id`.
7. The Publisher Profile of the Publisher declared in the manifest.
8. The Ed25519 signature.
9. The signed Actor identity and entry, and whether the Workload Profile matches the role selected by the Realm and distribution policy under §4.1.
10. The exact ZIP inventory, sizes, and SHA-256 hashes against `[[files]]`.
11. For the Front package, the Guest's static dependencies and capability manifest.
12. For a WASM Service package, its WASM imports.
13. Host API, Keel API and compatibility between the Front's expected Service contract and the Service package;
14. local/organizational security policy.

Before granting capabilities governed by a Privacy Contract, verify it against the signed Service manifest and evaluate it for the selected pair. See [14 §4.2](14-privacy-primitives-and-attestation.md#42-program-privacy-contract).

If any step fails, no part of the content may be handed to the Keel or Service runtime.

### 8.2 Keel loading boundary

Keel's `Engine.loadBundle(URL)` is only a loading mechanism, not an installation trust mechanism. Actrium must:

1. independently verify the Front archive selected by `front_package`.
2. read the already-verified Guest bytes from the content-addressed cache;
3. hand the local bytes/code to Keel;
4. forbid the Guest from specifying an alternate URL at startup.

## 9. Version immutability

On first acceptance, pin the whole-archive digest for each exact `publisher/program@version`. A different digest for that type is `PACKAGE_EQUIVOCATION`. Changed bytes require a new version. Different Realms may choose different compatible Fronts for the same Service.

Cache verified archives by digest for as long as an instance references them. Recheck every download against its pin.

A local first-seen pin only protects the current install; a Marketplace can help multiple clients compare digests they have seen. Providing global non-equivocation across all clients requires introducing a public, append-only transparency log later. v1 does not overstate a single-machine pin into global transparency.

## 10. The division of labor between signing and HTTPS

### What HTTPS provides

- proof of current domain control;
- transport confidentiality and integrity;
- the origin of the Publisher Profile;
- standard proxy/CDN/cache capability.

### What package signing and digests provide

- integrity across CDNs, mirrors and after being written to disk;
- offline verification;
- key-rotation history;
- exact version immutability;
- continued operation while the domain is temporarily unavailable;
- detection of same-name-different-package.

Both must be used together; neither substitutes for the other.

## 11. Marketplace

A Marketplace may:

- search, categorize and recommend;
- record Publisher verification;
- do static/dynamic review;
- give a risk rating;
- record the digest seen for each exact Actor package type.
- publish revocations and malware intelligence;
- provide a cache or mirror.

A Marketplace may not:

- squat a domain on a third party's behalf;
- silently replace a Publisher's package;
- substitute a marketplace login identity for the Publisher domain;
- bypass Actrium's permissions because something is "reviewed";
- equate delisting with automatically deleting the Service/Workspace.

## 12. Revocation policy

Revocation tiers:

| Tier | Behavior |
|---|---|
| Advisory | show the risk, allow continuing |
| Block new | block new installs/creations, prompt on existing instances |
| Quarantine | stop and isolate existing instances by default, user/admin can audit and act |
| Critical block | block loading the Guest/Service, keep a data-recovery entry point |

A revocation record must contain:

- the Program Release and the affected package digest or signing key.
- the reason;
- the publication time;
- the publisher's or marketplace's signature;
- a recommended action;
- whether it affects only the Guest, the Service, or the signing key.

Revocation does not automatically delete the Workspace.

## 13. Update and upgrade

The Program Catalog may prompt about a new version, but a Service does not automatically upgrade in place.

v1 upgrade:

1. install the new Program Release;
2. create a new Service in the same Realm, obtaining a new ActorId;
3. the old Service produces a migration snapshot;
4. explicitly hand off the Workspace lease;
5. the new Service imports and verifies;
6. the user confirms the switch, after which a Realm Profile whose resolved primary selects the new Service archive is published via `publish_profile`;
7. the old Service is archived after a rollback window; rolling back means publishing another Realm Profile, with a higher `sequence`, that points back at the old package.

This avoids a mismatch between the version in an ActorId and the workload actually running, and keeps a failed rollback explainable.

## 14. Local development and self-hosting

Supported:

- a local pair of Service and Front `.actr` files.
- a pair of package addresses on localhost, a loopback address, or an explicitly trusted private CA.
- a local development signing key;
- hot-updating the Guest;
- an unpublished Program Catalog entry.

Restrictions:

- each changed archive gets a new development digest.
- an unverified key does not inherit the formal Publisher permissions of the same domain;
- a local package does not enter the formal Marketplace;
- private-network, loopback and link-local URLs are only allowed in development mode or under enterprise policy;
- a development package must not be used to bypass Native Service restrictions.

## 15. Cache and offline

The cache is content-addressed by package digest:

```text
cache/packages/sha256/<digest>/package.actr
cache/extracted/sha256/<digest>/...
```

Requirements:

- the original package is kept, for re-verification;
- extracted files are only ever produced from an already-verified package;
- atomic writes;
- incomplete downloads are isolated;
- cache metadata records the Publisher key and verification time;
- Keep the accepted Realm Profile with its pinned archives for offline re-verification.
- a local instance can run offline from its pinned Service and Front packages under the offline policy.
- expiry of Realm credentials/revocations is handled under offline security policy, and is not equivalent to package verification failure.
