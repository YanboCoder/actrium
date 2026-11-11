# Actrium v1 Product Requirements

> Status: Target State Draft<br>
> Document version: 0.1<br>
> Date: 2026-07-29<br>
> Scope: the next stage of product and protocol design for Actrium, Actr and Keel<br>
> Principle: define the ideal model first, then adjust the existing implementation; this document does not assume compatibility with the current implementation

## 1. One-sentence definition

Actrium is the "program browser" of the Actrium world: users run native UI described by a Keel Guest inside a resident app Tab; each resident app Tab is hosted by a trusted Front Actor, which accesses the program's state, Workspace and Actor network through that instance's Service.

## 2. Overall conclusion

This revision direction is coherent, unified and technically feasible:

```text
Tab
└── Front Actor
    ├── Actrium's fixed Native Workload
    │   ├── Keel Host
    │   ├── Capability Broker
    │   └── Actr Client
    ├── Keel Guest provided by the Publisher
    └── Service ── Workspace
```

Its main value is not drawing fewer layers, but giving each layer exactly one responsibility:

- Tab is the user's interaction session.
- Front Actor is the front-end security boundary controlled by Actrium.
- Keel Guest is the only front-end code the program developer can customize.
- Each instance has one Service, which holds state; Front only connects to it.
- Workspace is held by the Service; Front never touches paths, keys or the underlying storage.
- An application instance is a Realm, addressed by a Realm link `actr://<Realm domain>/<realm_id>`; where the program is downloaded from is written in the Realm Profile.
- `ActorId` has the form `<serial>.<realm_id>@<domain>`, carries its own Realm name and contains no program type; once an `ActorId` is known it must not be load-balanced.
- `publisher/program@version` exactly identifies an immutable program version.
- A Realm corresponds to one application instance and governs that instance's identity, admission and network entry; a domain is only used to name Realms; signaling and TURN are shared infrastructure delegated by the Realm.

But it depends on three foundational capabilities that must be completed first:

1. **A persistent, resolvable ActorId**: a Service must keep the same `ActorId` across restarts, network loss, credential renewal or migration, and the Realm name must be derivable from the ID. Today's Actr hard rebind can change the ID, so it cannot directly serve as long-term, shareable URL semantics.
2. **Domain-name Publisher**: Actr's publisher semantics and length limits need to change so that a normalized DNS domain becomes the program publisher's identity.
3. **A truly closed Front Host**: Actrium must complete package verification before Keel runs, and must constrain the Guest with capabilities, a component allowlist, resource quotas and a fixed sandbox backend; it must not hand a remote URL directly to Keel to load.

## 3. Frozen core decisions

| No. | Decision |
|---|---|
| D-001 | Every app Tab that is loading, running or foreground-suspended runs exactly one Front Actor; a dormant Tab that has been discarded by the Shell but retained can have no live Actor. |
| D-002 | Every Front Actor, while running, is bound to exactly that instance's Service. It may be temporarily zero during creation/connection. |
| D-003 | "1:1" is the sole binding from the live Front's point of view, not a permanent bijection between a Service and historical Tabs; a Service lives independently and can be reconnected by a new Front. |
| D-004 | v1 defaults to an exclusive interactive session; opening the same instance's link again on this device focuses the existing Tab first. Shared sessions are a later, explicit capability. |
| D-005 | A Service binds zero or one Workspace; a program that needs multiple Actors has them orchestrated by the Service within the same Realm. |
| D-006 | Actrium provides the native Front Host. The Publisher supplies the Keel Guest and Service Workloads with their resources and declarations. |
| D-007 | In v1, third-party Front Guests only support the Keel JS Guest, and it is fixed to a QuickJS backend with a hard timeout and heap cap. |
| D-008 | A signed Program Release record binds separate Service and Front packages. Each `.actr` uses `actr-package/2` and declares one Actor type and one Workload. The Front manifest declares its Service dependency. |
| D-009 | A link only points to an instance and uses the `actr://<Realm domain>/<realm_id>` form; opening a link only connects to that instance and must not silently create a new one on failure. Creating an instance is a local operation inside Actrium, not a link. |
| D-010 | Publisher identity is a normalized domain without a port. The Publisher Profile supplies keys for verifying release records and packages obtained through any distribution channel. |
| D-011 | Marketplace is a source of discovery, review and revocation information, not a Publisher naming authority; the domain and the package signature are the provenance chain. |
| D-012 | A v1 ActorId must contain a verifiable Realm name (`<realm_id>@<domain>`, e.g. `notes@acme.com`); the program type is not in the ActorId. It is a public address and cryptographic identity, not a bearer token: knowing the address does not grant the right to connect. |
| D-013 | A Realm is a logical trust domain corresponding to one application instance, and may delegate multiple signaling/TURN services; a Realm is not the same thing as a physical signaling process — one Actr Facility Provider hosts many Realms; a Realm is bound to an Actr Facility Provider when it is created and can never switch afterward. |
| D-014 | A Service instance is pinned to an exact version and package digest; v1 upgrades create a new Actor through an explicit migration, rather than replacing the workload in place. |
| D-015 | An Actor credential proves the Actor key, identity and claims; it does not prove what code the other side is actually running. How an application authorizes on that basis is up to the application; see [Application development guidance](app-development-guidance.md). |
| D-016 | A Service manifest's `[workspace]` declares requirement, requested access, and supported Providers. The deployment or instance-creation flow determines provisioning, and the Host stores the actual binding and grant per instance. |
| D-017 | Deletion is split into recoverable Trashed and unrecoverable Purged/Deleted; deleting the Workspace is always a separate action. |
| D-018 | Publisher and Realm domain bootstrap always uses HTTPS 443 (i.e. `https://<domain>/.well-known/actr`); the Realm Profile comes from the HTTPS location declared by `realms`, and it pins the Service package and the Actr Facility Provider endpoints (which may carry ports). When the Realm Profile cannot be fetched or fails verification, no Actr Facility Provider other than the Realm Profile's own may be used. |
| D-019 | Application assurance is aggregated over a rootless effective-dependency graph; when the channel is valid and `Ready`, the displayed color takes the weakest of all node conclusions. Front only locates, it is not the graph root. |
| D-020 | `ActorAssurance` is issued by a verifier, `issuer ≠ subject`. The Actr Host may evaluate the Guest and provide evidence, but must not self-sign its own final conclusion. |
| D-021 | The top level has only green / yellow / red; when a facet is not ready, the display shows "cannot be established," not a fourth color. Unknown is not a confirmed leak, but it must never be displayed as green. |
| D-022 | WASM can be measured via imports / effective capabilities / egress closure; a third-party same-process Native cannot be green — only a compliant out-of-process Native Capsule can be raised. |
| D-023 | Conclusions must be linked to the data plane: a member that is suspended and not `LocalContinue` must not process application data; a new configuration only applies to new data. |
| D-024 | The user interface has only three colors; internally it keeps multidimensional facts — identity, provenance, confinement, compute, channel, storage, egress and output — mapped to `CONTROLLED / UNKNOWN / UNCONTROLLED`. |
| D-025 | Every network Actor edge of Actr must be encrypted, authenticated, integrity-protected and replay-resistant; signaling, TURN and ordinary relays must not hold the payload key. |
| D-026 | "End-to-end encryption" means encryption in transit and at rest; plaintext appears only within an execution boundary that policy accepts and can prove — it must not be claimed that data stays ciphertext throughout ordinary computation. |
| D-027 | Confidential Computing protects data in use; WASM/Native confinement constrains workload egress; the two are orthogonal and do not substitute for each other. |
| D-028 | Remote attestation uses the Attester, Verifier, Relying Party division of labor; Evidence is not a trust conclusion — final authorization is decided by local data policy. |
| D-029 | The decryption key for private/secret data may only be sealed to an ephemeral execution-environment key bound to Evidence after the required attestation passes. |
| D-030 | An Actor using a GPU/NPU must produce a combined attestation of CPU, guest, driver, device, firmware, IOMMU/link/memory; when any plaintext-visible link cannot be proven, compute confidentiality is unknown. |
| D-031 | Green is bound to the current ready facet (`ApplicationStateDigest` + `ApplicationAssurance` + `Ready`); it is not the whole Program across all versions, nor a transient transport session. |
| D-032 | The Program Privacy Contract remains a static upper bound on the Actor graph grammar and capabilities; it only constrains the candidate graph and does not directly produce green. |
| D-033 | The initial graph must complete evaluation before releasing protected data or a decryption key; the conclusion is bound to the current `ActorStateDigest`, `structureDigest`, policy and evidence. |
| D-034 | When `ActorStateDigest` changes, old power is locally suspended and revoked; members that do not depend on the changed node may `LocalContinue`, but must not display application green. |
| D-035 | Color can auto-recover when it has not weakened; when it weakens, blocking is maintained and risk acceptance is requested (this does not change the color); the user cannot override an invalid channel or a platform hard floor. |
| D-036 | A trusted Verifier may package an internal Actor subgraph as a Certified Privacy Domain and provide it through an Assured Actor Edge with scope, validity and revocation; a self-signed declaration is invalid. |
| D-037 | Audit logs are an explicit storage/audit sink and must declare fields, data class, target, reading party and retention; LLM/GPU attestation by itself cannot prove that the server keeps no separate log. |

## 4. Document navigation

Recommended reading order:

1. [Product definition](01-product-definition.md): why, for whom, what, and what not.
2. [Domain model](02-domain-model.md): the relationships among Tab, Front, Service, Workspace, Program and Realm.
3. [User experience](03-user-experience.md): from the address bar to the Program Catalog, the Service Inventory and error recovery.
4. [Realm links and the address bar](04-uri-and-resolution.md): how an instance address is shown, opened, saved and shared.
5. [Program package and HTTPS distribution](05-program-package-and-distribution.md): the domain-name Publisher, the `.actr` package, the marketplace and updates.
6. [Runtime and the Front–Service protocol](06-runtime-and-protocol.md): the Native Host, the Keel Guest, capability boundaries and the session protocol.
7. [Realm, signaling, TURN and identity](07-realm-network-and-identity.md): the trust chain, network discovery and port policy.
8. [Workspace, lifecycle and local storage](08-workspace-lifecycle-and-storage.md): state ownership, creation, reconnection, deletion and migration.
9. [Security and privacy](09-security-and-privacy.md): the threat model, permissions, sandboxing and secret management.
10. [Requirements and acceptance criteria](10-requirements-and-acceptance.md): functional requirements, non-functional requirements and end-to-end acceptance.
11. [Delivery plan and feasibility](11-delivery-plan.md): foundational work, phasing, risks and exit criteria.
12. [Reference protocol and samples](12-reference-spec.md): the manifest, Publisher/Realm Profile, handshake messages and terminology.
13. [Actrium privacy marks and graph aggregation](13-privacy-state-and-aggregation.md): grounding the current security model in Tab / Front / Actr Host.
14. [Privacy primitives, remote attestation and trusted compute](14-privacy-primitives-and-attestation.md): end-to-end encryption, the WASM/CC division of labor, RATS, key release and combined CPU/GPU/NPU attestation.

There is also non-normative [application development guidance](app-development-guidance.md): authorization and trust advice for writing a Service; it is not a platform rule.

## 5. Normative language

This document uses the following levels of strength:

- **MUST**: its absence would break the product model, a security boundary, or interoperability.
- **SHOULD**: should be satisfied by default; any deviation must record the reason and the alternative measure.
- **MAY**: a compatible implementation or a future extension.

Example JSON/TOML/protocol fields are used to freeze product semantics; the formal wire schema still needs to be versioned in the Actr/Actrium schema repositories.

### 5.1 Document precedence

When there is a conflict, interpret it in the following order:

1. **Security and privacy semantics**: [docs/security](../../architecture/security/actrium/README.zh.md);
2. product decisions in this README that do not conflict with the item above;
3. the MUST/FR/NFR items in [Requirements and acceptance criteria](10-requirements-and-acceptance.md);
4. product topics on links, distribution, runtime, Realm and Workspace not covered by this chapter;
5. the illustrative fields in the [reference protocol](12-reference-spec.md);
6. product narrative, user flows and the delivery plan.

`01` and `03` mainly help with understanding the product; `04`–`09` and `12`–`14` are draft specifications awaiting P0 freeze; `10` is the acceptance baseline; `11` is a plan, not a wire contract.

## 6. Factual basis

This design uses the following public implementations as a feasibility basis, without being bound by their historical structure:

- [Actrium/actr](https://github.com/Actrium/actr): the Actor model, exact `ActrId` addressing, Realm/connection ACLs, signaling, WebRTC, signed `.actr` packages and WASM workloads.
- [Actrium/keel](https://github.com/Actrium/keel): the Keel Host/Guest, the bridge, the JS sandbox, contracts and native component rendering.
- [Actrium's current architecture overview](../../architecture/actrium/overview.md): the multi-platform Shell, the shared runtime and the Actr SDK boundary.

Key verified facts include:

- Actr already supports exact addressing via `Dest::Peer(ActrId)` and discovery by `ActrType`; the two can be cleanly separated.
- `.actr` already has Ed25519 manifest signatures, SHA-256 file binding and resources; dependency locking is optional depending on the workload/build pipeline.
- Keel's Host/Guest boundary and allowlisted-module mechanism are suitable for carrying Front, but its `Engine.loadBundle(URL)` is not by itself a trusted distribution system.
- Most of Keel's "native" sandboxing is still language-runtime isolation within the same OS process, and cannot be claimed as browser-grade process isolation.

This document consistently uses the canonical names Loran, Publisher Registry, Issuer and publisher; historical names found in code and older documents are collected in the [glossary's "Historical name mapping"](../../GLOSSARY.zh.md).
