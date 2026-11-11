# 10. Requirements and acceptance criteria

## 1. Functional requirements

### 1.1 Shell and Tab

| ID | Requirement |
|---|---|
| FR-SH-001 | Every resident app Tab MUST have exactly one live Front Actor while loading/running/suspended; Shell pages and dormant Tabs may have no Front. |
| FR-SH-002 | A Front Actor MUST bind to exactly one Service once it finishes starting up. |
| FR-SH-003 | A refresh MUST rebuild the Front and reconnect to the same Service; it MUST NOT create a new Service. |
| FR-SH-004 | Closing a Tab MUST revoke the session and destroy the Front; it MUST NOT stop or delete the Service. |
| FR-SH-005 | A backgrounded Tab MUST pause the Keel Engine; resuming MUST either resume it or rebuild it and Attach. |
| FR-SH-006 | Reopening the same exclusive Service on this device by default focuses the existing Tab. |
| FR-SH-007 | The address bar MUST separately show the Publisher, the instance, the Realm and the connection's security state. |
| FR-SH-008 | Guest navigation MUST go through `host:navigate`; it must not directly change Shell history, inherit capabilities across instances, or write a secret into the URI. |

### 1.2 URI

| ID | Requirement |
|---|---|
| FR-URI-001 | Realm links of the form `actr://<Realm domain>/<realm_id>[/<page path>][?…][#…]` MUST be supported; everything after the RealmId is passed to the application as-is. |
| FR-URI-002 | Opening a Realm link MUST verify the Realm Profile, its Service package, the matching Program Release record, and the Front package. Actrium then locates the Service by its Actor type within that Realm and Attaches. |
| FR-URI-003 | An Attach failure MUST NOT trigger load balancing, downgrading to another version, switching Realm, or creating a substitute instance. |
| FR-URI-004 | The Service's Actor type MUST match the Actor type declared by the package the Realm Profile points to. |
| FR-URI-005 | A Realm link MUST NOT contain a credential, session secret, Workspace secret or bearer invite. |
| FR-URI-006 | A Realm link MUST NOT contain a port; development origins are distinguished only through the package address (localhost, loopback addresses or a private CA). |
| FR-URI-007 | Realm-link input and sharing MUST use `actr://` only; an `https://` input MUST be rejected with `URI_SCHEME_INVALID`, not converted via `links`. |
| FR-URI-008 | The parser MUST implement IDNA, illegal-encoding and control-character checks. |
| FR-URI-009 | Creating an instance is a local Actrium operation and is not triggered by a link; on success the address bar shows that instance's Realm link. |

### 1.3 Program Catalog and distribution

| ID | Requirement |
|---|---|
| FR-PKG-001 | The Program Catalog and the Service Inventory MUST be kept separate. |
| FR-PKG-002 | A published release MUST supply a Publisher-signed record that binds both package types and digests. On Realm entry, the record's Service package MUST match the Realm Profile's `package`. |
| FR-PKG-003 | Every Program Release MUST pin fixed Service and Front package digests. |
| FR-PKG-004 | Each `.actr` manifest MUST declare one Actor type and one Workload. Verify its Publisher key, manifest signature, and every file's hash. |
| FR-PKG-005 | Zip traversal, an excessive file count, an excessive total extracted size and zip bombs MUST be rejected. |
| FR-PKG-006 | A third-party package MUST NOT contain a Front Host/native plugin. |
| FR-PKG-007 | The Guest MUST load from verified local bytes; Keel MUST NOT trust a remote URL directly. |
| FR-PKG-008 | A different Service, Front, or Privacy Contract binding for the same Program Release MUST hard fail. |
| FR-PKG-009 | Service and Front packages and any bound Privacy Contract MUST remain cached while a Service instance references the release. |
| FR-PKG-010 | Marketplace metadata MUST NOT override the Publisher's package identity. |
| FR-PKG-011 | Product v1 local import MUST accept a matching Service and Front `.actr` pair. Development sources MUST be clearly labeled. |
| FR-PKG-012 | Advisory/Block-new/Quarantine/Critical-block revocation levels MUST be supported. |
| FR-PKG-013 | Publisher bootstrap MUST be pinned to HTTPS 443; the Publisher Profile MUST have a normalized signature, a sequence number/freshness, and rollback protection. |
| FR-PKG-014 | The official publishing tool MUST build, validate, and sign both packages and publish their signed release record. It MUST read both packages back to verify their SHA-256 digests and signatures. |

### 1.4 Front/Keel

| ID | Requirement |
|---|---|
| FR-FRONT-001 | Front Host MUST ship with Actrium and MUST NOT be replaceable by a Publisher. |
| FR-FRONT-002 | v1's official Guest supports only the Keel JS Guest. |
| FR-FRONT-003 | QuickJS MUST be pinned, with a hard timeout and a heap cap enabled. |
| FR-FRONT-004 | Guest modules, capabilities, components and RemoteRefs MUST be allowlisted. |
| FR-FRONT-005 | The Guest MUST NOT get default fetch/socket/filesystem/native modules. |
| FR-FRONT-006 | The Guest only gets a Service capability with a fixed target. |
| FR-FRONT-007 | All Guest RPC/input/output/event data MUST be schema-validated. |
| FR-FRONT-008 | Image and resource components MUST NOT form an undeclared network egress. |
| FR-FRONT-009 | Engine close/crash MUST clean up timers, callbacks, listeners and capabilities. |
| FR-FRONT-010 | The UI MUST describe v1 as a same-process sandbox and MUST NOT claim OS process isolation. |

### 1.5 Service and Workspace

| ID | Requirement |
|---|---|
| FR-SVC-001 | A local Create MUST create a persistent Service ActorId. |
| FR-SVC-002 | The Service MUST keep its ActorId across restarts, credential renewal and signaling switchover. |
| FR-SVC-003 | Each Service instance MUST pin its Program Release and the digest of the Service package it runs. |
| FR-SVC-004 | The Service MUST have a per-instance store independent of the ActrType. |
| FR-SVC-005 | The Workspace MUST be held by a broker in the Service Host; the Front MUST NOT obtain its path/credential. |
| FR-SVC-006 | v1's default Workspace write mode is exclusive writer. |
| FR-SVC-007 | The creation transaction MUST be idempotent and crash-recoverable. |
| FR-SVC-008 | Stop, Archive, Delete Service and Delete Workspace MUST be distinct operations. |
| FR-SVC-009 | A third-party Service MUST default to a confined WASM Component workload. |
| FR-SVC-010 | A Service may orchestrate other Actors within the Realm, but the Front binds only to the Service. |
| FR-SVC-011 | A v1 version upgrade MUST go through a new Actor plus an explicit migration. |
| FR-SVC-012 | Once a Service is purged, its ActorId MUST NOT be reused. |
| FR-SVC-013 | Static Workspace requirements MUST use the Service manifest's `[workspace]` table with `requirement`, `access`, and `providers`. Omitting the table means `requirement = "none"`. Provisioning and actual bindings are instance settings. A required binding MUST satisfy the declared access before the Service enters Ready. |
| FR-SVC-014 | Moving to trash MUST preserve identity and instance data and remain recoverable; purging MUST revoke identity and be unrecoverable. |
| FR-SVC-015 | Lifecycle, reachability, health, placement and attachment state MUST be recorded as separate fields. |
| FR-SVC-016 | v1 only creates a new instance on the current device's supported local Actr Host; remote management MUST require a separate management capability granted by that host. |

### 1.6 Realm, identity and network

| ID | Requirement |
|---|---|
| FR-NET-001 | Actrium MUST embed a signaling client and support the local Realm, and MUST NOT hard-code a single global server. |
| FR-NET-002 | A domain's Realm signing key (the `realm_signing_keys` member of its authority descriptor, already defined in Actr Authority Discovery §4.4) MUST be fetched over HTTPS; the Realm Profile MUST be signed by a trusted Realm signing key, and its validity does not depend on where it was fetched from. |
| FR-NET-003 | A Realm MUST be able to delegate multiple signaling/TURN endpoints. |
| FR-NET-004 | In v1 the Front and the Service MUST be in the same Realm, i.e. the same application instance's Realm; type discovery only happens within that Realm. |
| FR-NET-005 | An Actor credential MUST be bound to the ActorId, the Actor key, the Actor type, the Realm and its validity period. |
| FR-NET-006 | Service identity MUST support reclaiming the same ActorId with the long-term key. |
| FR-NET-007 | A signaling credential MUST NOT be placed in a long-lived URL query. |
| FR-NET-008 | TURN MUST use a short-lived, audience-restricted, rotatable credential. |
| FR-NET-009 | The Actr Realm and the TURN auth realm MUST use different types/naming. |
| FR-NET-010 | An endpoint MAY use a non-standard port, but it MUST come from a trusted Realm Profile or Publisher Profile. |
| FR-NET-011 | Service offline MUST be a distinguishable state; it must not degrade into the same-type discovery path. |
| FR-NET-012 | In v1, multiple signaling endpoints may only serve as failover within the same Realm's control plane; implicit cross-control-plane federation is not supported. |
| FR-NET-013 | A local Realm's ActorId MUST use the local pseudo-domain `<installation-id>.actr.alt`, which is forbidden to be copied as a cross-device link. |
| FR-NET-015 | A Realm domain's bootstrap (`https://<domain>/.well-known/actr`) MUST be pinned to HTTPS 443, and rollback MUST be prevented by the Realm signing key rotation chain plus the Realm Profile's sequence number (a Realm Profile does not expire). |
| FR-NET-016 | Actrium MUST provide flows for trusting a domain, joining an instance's Realm via a Realm invitation, adding a client, re-authentication, client revocation, member management (owner only), leaving, owner recovery, and handling Realm signing key changes. |
| FR-NET-017 | Creating an instance MUST create a new Realm under the chosen domain; the RealmId is unique within the domain, immutable after creation, and never reused after retirement. |
| FR-NET-018 | When a Service is upgraded to a new Program Release, the Realm MUST publish a Realm Profile whose `package` points to the new package, and the new Service MUST be created in the same Realm; migrating to a different Realm MUST produce a new ActorId. |
| FR-NET-019 | When the Realm Profile cannot be fetched, fails verification, or is retired, Actrium MUST NOT use any Actr Facility Provider other than the one specified in the Realm Profile to communicate with that Realm (including services listed in the authority descriptor's `services`). |
| FR-NET-020 | The Realm domain chosen for creating an instance MUST already be trusted, or first trust MUST be established on the creation confirmation page; no domain means no Realm is assigned (except the local Realm); creation MUST be authorized by the holder of that Realm domain's Realm signing key: for any other Realm domain, the holder's signing service signs an Actr management envelope and the confirmation page shows that Realm domain as the trust root; when the Realm domain equals or belongs to the Actr Facility Provider ID, in v1 only that Actr Facility Provider's operator creates the Realm, the user joins with the owner invite the operator hands over, and the confirmation page shows the Actr Facility Provider as the trust root. |
| FR-NET-021 | Each networked Realm MUST have exactly one owner (its creator or whoever received ownership by transfer); an ownership transfer MUST be atomic, with the previous owner becoming an editor by default; the owner MUST transfer before leaving and MUST NOT revoke its own last client; multiple owners are not part of v1. |
| FR-NET-022 | Each client MUST register a separate Realm client key in each Realm; registering, reissuing and renewing an Actor credential MUST all prove possession with a signature by that key, and the Issuer issues only for registered, unrevoked keys of active members; a shared Realm registration secret MUST NOT be used. |
| FR-NET-023 | Revoking a Realm client key MUST immediately refuse its new Actor credentials, revoke the unused invites it issued, disable the Actors bound to it, and close their signaling connections and TURN allocations; removing a member and leaving a Realm MUST revoke all of that member's client keys; changing a role changes only application permissions and takes effect in the member's next Actor credential. |
| FR-NET-024 | Creating a Realm and recovering ownership MUST deliver the management right as a one-time owner invite, with no Realm admin token; owner invites are issued by the Realm signing key holder (activation response, `reissue_owner_invite`) or the Actr Facility Provider's operator, and a new one invalidates earlier unused ones. Self-service creation with Actr Facility Provider accounts is a later capability. |

### 1.7 Permissions, security and privacy

| ID | Requirement |
|---|---|
| FR-SEC-001 | The effective capability MUST be the intersection of the declaration, what the Actr Host supports, user/administrator authorization, and platform policy. |
| FR-SEC-002 | A permission grant MUST be bound to a Publisher/Program/version or digest and a scope. |
| FR-SEC-003 | Adding or expanding a high-risk permission MUST require reconfirmation. |
| FR-SEC-004 | An ActorId MUST be treated as a public address, not an access token. |
| FR-SEC-005 | The Actor key, the Realm client key and Workspace Provider credentials MUST go into OS secure storage. |
| FR-SEC-006 | Logs and crash reports MUST NOT record secrets, business payloads or a full Workspace path. |
| FR-SEC-007 | Package/Realm/network URLs MUST implement an SSRF and redirect policy. |
| FR-SEC-008 | A signed emergency blocklist, expiry and audit MUST be supported. |
| FR-SEC-009 | Trashing, quarantining or purging a Service MUST NOT automatically delete the Workspace. |
| FR-SEC-010 | Telemetry MUST default to off; once a user or administrator enables it, sensitive identifiers MUST still be minimized. |
| FR-SEC-011 | Revoking a grant MUST immediately invalidate the corresponding capability handle, and terminate or downgrade active sessions. |
| FR-SEC-012 | A Realm invitation MUST be one-time, expirable and revocable, redeemed with a signature by a new Realm client key and consumed atomically, and MUST NOT enter an instance's URI as a bearer secret; expired, used, revoked and Realm-mismatched invitations MUST be distinct errors. |
| FR-SEC-013 | Application permissions MUST support at least owner/editor/viewer roles, and MUST keep Realm membership separate from instance authorization: membership only decides whether one can enter that instance's network (a prerequisite for obtaining an Actor credential, connecting to signaling and doing the Attach handshake); roles and permissions inside the instance are still decided by application permissions and session authorization; the member role is kept by the Actr Facility Provider, written into the Actor credential and enforced by the Service, and a session never outlives the Front credential. |
| FR-SEC-014 | The application color MUST be expressed as separate fields for lifecycle, reachability, health and enforcement. |
| FR-SEC-015 | The application graph MUST be a rootless connected component of effective dependencies and MUST include every effective `Transfer(ActorId)` member. |
| FR-SEC-016 | When the channel is valid and `Ready`, the displayed color MUST take the weakest of all node conclusions. |
| FR-SEC-017 | The top level distinguishes only green / yellow / red; no color MUST be displayed while a facet is not ready; unknown MUST NOT be treated as green. |
| FR-SEC-018 | The party being assessed MUST NOT self-sign the final `ActorAssurance`; the effective declaration is closed by the Actr Host, and the conclusion is issued by a verifier. |
| FR-SEC-019 | `ActorAssurance` MUST be signed and bound to the `ActorId + ActorStateDigest`, the evidence fingerprint, the policy, the rules and its validity period. |
| FR-SEC-020 | A new Actor edge may only receive application data after the peer is `ACTIVE` and its current AA is acceptable. |
| FR-SEC-021 | Revocation of a package, an Actr Host evidence key, a credential, an Actor Grant, a policy, or a conclusion MUST propagate to the weakest item and be synchronized to enforcement points. |
| FR-SEC-022 | Only after an ineffective dependency ends does that Actor stop entering the next `structureDigest`. |
| FR-SEC-023 | A third-party in-process NativeCdylib MUST NOT be `CONTROLLED`; it MUST NOT be upgraded through a declaration, a signature, or static scanning alone. |
| FR-SEC-024 | Attach MUST negotiate the policy, a nonce, the current `ActorStateDigest` and `ApplicationStateDigest`; it MUST NOT rely on GraphRoot/epoch. |
| FR-SEC-025 | The full Actor graph MUST be minimally disclosed and MUST NOT be sent by default to signaling, an ordinary peer, or telemetry. |
| FR-SEC-026 | An opaque HTTPS endpoint that can reach protected data MUST be explicitly authorized as a non-Actor sink; TLS by itself does not prove its internal privacy. |
| FR-SEC-027 | Every protected Actor call MUST be checked against the current state and data generation for permission. |
| FR-SEC-028 | WASM can only be `CONTROLLED` when its declaration, imports, effective capabilities and possible/observed egress form an enforceable closure. |
| FR-SEC-029 | A NativeCapsule can only be rated `CONTROLLED` once it has entered an independent-process OS sandbox before third-party code executes, closed off its original egress, and opened only brokered IPC. |
| FR-SEC-030 | The three colors MUST internally retain multidimensional facts, including identity, artifact provenance, workload confinement, compute confidentiality, channel security, storage protection, egress closure and output control. |
| FR-SEC-031 | Every network Actor edge MUST have mutual authentication, confidentiality, integrity, forward secrecy and replay protection; signaling, TURN and ordinary relays MUST NOT hold the payload key. |
| FR-SEC-032 | An Actor Secure Channel handshake MUST bind both ends' ActorId, current `ActorStateDigest`, runtime, credential, ephemeral key, target Service, policy, attestation result, nonce and role, and mutually authenticate the joint transcript. |
| FR-SEC-033 | Transport TLS/DTLS/QUIC succeeding alone cannot produce a channel security pass; Actor identity, the running instance and the session key MUST be bound. |
| FR-SEC-034 | Workspace, snapshot, backup, cache, index and temporary files MUST implement envelope encryption keyed by data class; the ActorId MUST NOT act as the key. |
| FR-SEC-035 | A Data Policy MUST declare required assurance dimensions by data category and operation; a non-required dimension MUST NOT make public data unconditionally unknown. |
| FR-SEC-036 | Remote attestation MUST distinguish the Attester, the Verifier and the Relying Party; Evidence/self-assertion MUST NOT directly become an authorization decision. |
| FR-SEC-037 | Attestation MUST be bound to a fresh nonce, the Actor/runtime, the workload/package digest, the policy, the audience and the current ephemeral channel/decryption key. |
| FR-SEC-038 | When policy requires confidential compute, a private/secret DEK may only be sealed to the ephemeral key covered by attestation after appraisal passes. |
| FR-SEC-039 | Confidential Computing can only satisfy the corresponding compute confidentiality claims; it MUST NOT substitute for WASM/Native confinement, egress closure or output control. |
| FR-SEC-040 | A confidential workload using a GPU/NPU MUST be jointly assessed on the CPU TEE, the guest, the runtime, the driver, IOMMU/device assignment, device firmware, the CC mode, the link, and device memory. |
| FR-SEC-041 | When any accelerator link that can see plaintext lacks valid evidence, compute confidentiality MUST be indeterminate/unknown. |
| FR-SEC-042 | A vendor's native evidence MUST be mapped to canonical claims through a signed, versioned, revocable verifier adapter; a vendor's model name or marketing claim MUST NOT be mapped directly to a pass. |
| FR-SEC-043 | When Evidence, an Attestation Result, a Reference Value, an Endorsement, or a TCB status expires or is revoked, new sensitive-data and key release MUST stop, and re-attestation/rekeying MUST be triggered. |
| FR-SEC-044 | Highly sensitive output crossing a data class MUST go through an explicit declassification/output policy; a legitimate Actor response MUST NOT become a covert channel that bypasses egress closure. |
| FR-SEC-045 | Green MUST be bound to the current `ApplicationStateDigest` (`structureDigest` + Principal + Merkle root) and `Ready`; a blanket green label MUST NOT be given to a Publisher or a Program. |
| FR-SEC-046 | The Program Privacy Contract MUST statically declare the allowed Actor roles/types/profiles, cardinality, edges, data classes, capabilities, terminals, audit sinks and assurance requirements. The Host MUST verify its raw-byte SHA-256 against the signed Program Release record and check its release and package bindings before using it for authorization. |
| FR-SEC-047 | The effective capabilities the Actr Host may grant MUST be the intersection of the Contract, user/organization policy, host policy and the instance grant; an undeclared capability MUST NOT obtain a handle. |
| FR-SEC-048 | The actual Actor/runtime graph MUST be an instance of the Contract's graph grammar; every mutation MUST first form a candidate graph and MUST NOT directly change an already-approved data graph. |
| FR-SEC-049 | The initial graph MUST complete identity, evidence, policy and `ApplicationAssurance` evaluation before releasing protected data or a DEK. |
| FR-SEC-050 | An optional feature that cannot satisfy the base profile MUST be split into its own feature profile/capability; disabling it MUST NOT contaminate the certified base closure. |
| FR-SEC-051 | A trusted Verifier may issue a Certified Privacy Domain aggregate for an internally closed subgraph, with scope, policy, an evidence root, a validity period and revocation information; upstream MAY treat it as a single node per policy. |
| FR-SEC-052 | An Actor/Service's self-signed privacy declaration MUST NOT form an Assured Actor Edge; the Verifier's trust, the certificate's scope, the endpoint, the session and the data class MUST all match. |
| FR-SEC-053 | Every audit log MUST be an explicit audit sink in the Contract, and MUST declare its fields, payload, data class, retention, operator visibility, encryption and export rules. |
| FR-SEC-054 | A change in `ActorStateDigest` MUST locally suspend and revoke old power; a non-dependent member MAY `LocalContinue`, but MUST NOT display application green; the control plane MUST NOT carry business data. |
| FR-SEC-055 | When the color has not weakened and policy allows, it MAY auto-recover; it still needs to be `Ready` to display green. |
| FR-SEC-056 | When the color weakens, blocking MUST be maintained and the reason shown; work may continue only after risk acceptance; risk acceptance does not change the color. |
| FR-SEC-057 | Risk acceptance MUST NOT raise a missing proof to green; a platform hard failure and an invalid channel MUST NOT be overridden by the user. |
| FR-SEC-058 | Re-evaluation is also required when the graph structure is unchanged but evidence expires, a verifier/TCB/credential is revoked, or the policy or state fingerprint changes. |
| FR-SEC-059 | GPU/NPU attestation MUST NOT be interpreted as an LLM server having no logs; only a measured logging configuration, closed egress/audit capabilities, or a trusted Certified Privacy Domain retention claim can assess the logging gap. |
| FR-SEC-060 | `ActorState.revision` MUST be monotonically incremented by the state authority and enter the `ActorStateDigest`; content rollback MUST NOT reproduce an old state root, and an old Actor Grant/conclusion MUST NOT become valid again through rollback. |
| FR-SEC-061 | An endpoint that can present Actr identity MUST be assessed against the Actor target; it MUST NOT be declared exempt from cross-Principal scoring as an HTTPS/domain-name sink; when the target category cannot be determined, that egress stays `UNKNOWN`; the authorization interface for a trust sink MUST present as "data leaves the assurance boundary." |
| FR-SEC-062 | The local clocks used by both the enforcer and the assessor to check deadlines, evidence and credential validity MUST NOT be set back. |
| FR-SEC-063 | `EncGraph` identifiers MUST fall within a restricted ASCII character set; `ApplicationStateDigest` leaves MUST be sorted by `ActorId`, with exactly one leaf per `ActorId`. |

## 2. Non-functional requirements

### 2.1 Performance

The following budgets exclude the first package download and user dwell time:

| ID | Metric | v1 target |
|---|---|---|
| NFR-PERF-001 | Cached local Service Attach to interactive | p50 ≤ 800 ms, p95 ≤ 2 s |
| NFR-PERF-002 | Cached remote Service Attach | p50 ≤ 2 s, p95 ≤ 5 s when the network is available |
| NFR-PERF-003 | Front Guest cold load | p95 ≤ 1.5 s |
| NFR-PERF-004 | Input to native UI update | p95 ≤ 100 ms, excluding remote RPC |
| NFR-PERF-005 | Backgrounded Tab pause | ≤ 250 ms |
| NFR-PERF-006 | Service local restart recovery | p95 ≤ 3 s |

### 2.2 Resources

Default, configurable budgets:

| ID | Item | Default |
|---|---|---|
| NFR-RES-001 | Keel Guest heap | desktop 64 MiB; mobile 48 MiB |
| NFR-RES-002 | A single Guest JS turn | 100 ms; the startup phase may be given a separate 1 s total budget |
| NFR-RES-003 | Guest render nodes | 10,000 |
| NFR-RES-004 | Render ops per batch | 5,000 |
| NFR-RES-005 | Front RPC concurrency | 32 |
| NFR-RES-006 | Default package download size | 200 MiB hard cap, lowerable by enterprise policy |
| NFR-RES-007 | Archive file count | 10,000 |
| NFR-RES-008 | Archive total extracted size | 500 MiB |
| NFR-RES-009 | Guest scheduling | each Guest uses an Actr Host-abortable dedicated execution thread/worker and does not occupy the Shell/UI main thread |

Any increase MUST still be bounded by the platform's cap and permission policy.

### 2.3 Reliability

| ID | Requirement |
|---|---|
| NFR-REL-001 | A crash at any stage of the creation transaction, followed by recovery, MUST NOT produce two Services. |
| NFR-REL-002 | Across 10,000 Service restart identity-recovery tests, the ActorId change count is 0. |
| NFR-REL-003 | The count of substitute instances created on an Attach failure path is 0. |
| NFR-REL-004 | A package cache write MUST be atomic; the old version must still verify after a power loss. |
| NFR-REL-005 | A Front crash MUST NOT corrupt the Service store or the Workspace. |
| NFR-REL-006 | A signaling endpoint failover within the same Actr Facility Provider MUST NOT change the ActorId. |
| NFR-REL-007 | Lease network-jitter tests MUST NOT produce two simultaneously valid exclusive attachments. |
| NFR-REL-008 | Beta's crash-free app session rate MUST reach 99.5%, and stable's MUST reach 99.9%. |

### 2.4 Security

| ID | Requirement |
|---|---|
| NFR-SEC-001 | All parsers MUST be under continuous fuzzing. |
| NFR-SEC-002 | A third-party package/Guest/Service MUST NOT bypass the capability allowlist. |
| NFR-SEC-003 | A secret-scanner test MUST NOT find a real credential in the URI, logs, or crash fixtures. |
| NFR-SEC-004 | High-risk security boundaries complete an independent penetration test before public beta. |
| NFR-SEC-005 | Security updates and the emergency blocklist fail closed when verification fails. |
| NFR-SEC-006 | A release build MUST generate an SBOM and signed provenance. |
| NFR-SEC-007 | After a node's conclusion weakens, the associated protected effects MUST be invalidated in sync before the next execution, with Shell update p95 ≤ 250 ms. |
| NFR-SEC-008 | Incremental weakest-item aggregation within 1,024 Actors and 4,096 edges completes at p95 ≤ 100 ms on a reference desktop; beyond that it MUST be unknown. |
| NFR-SEC-009 | Graph cycles, epoch races, Mark expiry, dependency revocation, undeclared egress and Actr Host concealment simulations MUST be covered by property/fault-injection tests. |
| NFR-SEC-010 | The Actor channel, the Attestation Result and the Evidence Bundle MUST have cross-implementation test vectors and replay/mis-binding/downgrade negative tests. |
| NFR-SEC-011 | Every production Verifier profile MUST record its trust root, the attack surface it covers, what it does not cover, the minimum TCB, freshness, the revocation source and conformance tests. |
| NFR-SEC-012 | When the Verifier/Key Broker is temporarily unavailable, an expired Result MUST NOT be reused indefinitely; the grace window MUST be explicitly bounded by data policy. |
| NFR-SEC-013 | A graph mutation request, freezing the old epoch's lease, and creating a candidate edge MUST be linearized; concurrency tests MUST NOT show a payload window after the mutation but before the freeze. |
| NFR-SEC-014 | The downgrade partial order, incomparable vectors, auto-recovery, user rejection and the non-overridable hard floor MUST be covered by property/fault-injection tests. |

### 2.5 Accessibility and internationalization

| ID | Requirement |
|---|---|
| NFR-A11Y-001 | Shell and system pages meet WCAG 2.2 AA per platform requirements. |
| NFR-A11Y-002 | Every security state has text/an icon, not color alone. |
| NFR-A11Y-003 | Keyboard, screen reader, dynamic font and high-contrast support. |
| NFR-I18N-001 | Stable error codes are separated from localized copy. |
| NFR-I18N-002 | An IDNA domain can be checked in both Unicode and A-label form. |

### 2.6 Maintainability

| ID | Requirement |
|---|---|
| NFR-MNT-001 | The URI, manifest, Realm Profile and session protocol MUST be versioned. |
| NFR-MNT-002 | All platforms share the same normalization and validation test vectors. |
| NFR-MNT-003 | Host capability and Safe Component schemas MUST generate types/tests from a single source. |
| NFR-MNT-004 | Every state machine MUST have recoverable transitions and stable error codes. |

## 3. End-to-end acceptance scenarios

### AC-001: First creation

**Given** a user selects the uninstalled `notes.example/team-notes@1.4.2` in the Program Catalog<br>
**When** they create an instance and confirm permissions/Workspace<br>
**Then** Actrium verifies the separate Service and Front packages, creates exactly one Realm and one Service, obtains a persistent ActorId, shows that instance's Realm link in the address bar, Attaches, and displays the verified Guest.

### AC-002: Refresh does not duplicate creation

**Given** creation has just completed<br>
**When** the user repeatedly refreshes, recovers from a crash, and goes back/forward<br>
**Then** the Service Inventory still shows only the original ActorId.

### AC-003: A specific instance is offline

**Given** the Service that a Realm link points to is offline<br>
**When** the link is opened<br>
**Then** if the local Inventory knows it is stopped, a start entry point is shown; if there is no remote host-state proof, offline/unreachable is shown; neither path creates a same-type instance or invokes type discovery.

### AC-004: Service type mismatch

**Given** the Actor type in the peer Service's credential differs from the type declared by the Service package pinned by the Realm Profile<br>
**When** the Attach handshake runs<br>
**Then** it fails with `SERVICE_ACTOR_TYPE_MISMATCH` before any Guest executes, and no substitute instance is created.

### AC-005: Same version, different package

**Given** the Service and Front package digests for a release are already pinned locally<br>
**When** either package has a different digest for the same Program Release<br>
**Then** `PACKAGE_EQUIVOCATION` blocks installing and running the changed archive. The original digests are retained for audit.

### AC-006: Front Guest overreach

**Given** the manifest does not declare network access<br>
**When** the Guest tries to require a network module, use Image headers, or call `host:net`<br>
**Then** every path is rejected, and a secret-free audit event is produced.

### AC-007: Guest hang

**Given** the Guest runs an infinite loop<br>
**When** the turn timeout is exceeded<br>
**Then** only that Keel Engine is terminated; the Service and other Tabs stay available.

### AC-008: Tab close

**Given** a running instance<br>
**When** the Tab is closed<br>
**Then** the Front/session is destroyed; the Service/Workspace/ActorId are preserved and can be reopened from the Inventory.

### AC-009: Actrium restart

**Given** a local Service has been created<br>
**When** Actrium fully quits and restarts<br>
**Then** the Service recovers with the same ActorId; Realm links in favorites and history still connect.

### AC-010: Exclusive session

**Given** a Service is already held under an exclusive lease by one Front<br>
**When** a second device Attaches<br>
**Then** the Service returns occupied; a second active attachment MUST NOT be established without authorization.

### AC-011: Signaling endpoint failover

**Given** signaling A fails and the Realm Profile declares signaling B<br>
**When** the Actor reconnects<br>
**Then** B is used and the ActorId is unchanged; an undeclared endpoint is not tried.

### AC-012: TURN

**Given** P2P is unreachable<br>
**When** TURN is used<br>
**Then** a short-lived, audience-restricted TURN credential issued by that Realm's Issuer is used, the UI shows relayed, and logs contain no passwords.

### AC-013: Workspace conflict

**Given** a Workspace already has an exclusive-writer Service<br>
**When** a second writing Service is created<br>
**Then** it is either explicitly rejected before creation or required to migrate/go read-only; no silent multi-writer state occurs.

### AC-014: Trash and purge

**Given** a Service is bound to an external Workspace<br>
**When** the user first moves it to trash and then restores it<br>
**Then** the Service is restored with the same ActorId and the Workspace content is unchanged; when the user later purges it without choosing to delete the Workspace, the Actor credential/key/instance store are deleted and unrecoverable, while the Workspace is still retained.

### AC-015: Upgrade failure rollback

**Given** a v1 Service and Workspace<br>
**When** v2 migration validation fails<br>
**Then** the v1 ActorId, Workspace lease and old Tab are recoverable, and v2 does not take over.

### AC-016: Domain transfer/new key

**Given** the installed version is signed by the old key<br>
**When** the Publisher Profile shows a new key with no rotation chain<br>
**Then** the old pinned digest can still run offline, and a new install/update shows an identity change and requires confirmation.

### AC-017: Mobile-platform suspend

**Given** a local Service is suspended by the system in the mobile background<br>
**When** a remote or local Front connects<br>
**Then** suspended/host-offline is shown; it is not reported as nonexistent, and no substitute instance is created.

### AC-018: Security revocation

**Given** a digest is marked Critical block<br>
**When** the user opens an existing instance<br>
**Then** the Guest/Service does not load, while Workspace export and the security explanation remain available.

### AC-019: Local Realm link

**Given** a Realm link uses another installation's local pseudo-domain `<installation-id>.actr.alt`<br>
**When** the current device opens it or tries to share it<br>
**Then** the connection is rejected with `URI_LOCAL_REALM_FOREIGN` and sharing is disabled; it must not be interpreted as a Realm under some DNS domain.

### AC-021: Realm Profile rollback

**Given** the Realm signing key rotation chain is pinned locally, and a Realm Profile with sequence 31 has been accepted<br>
**When** the endpoint returns a still-valid Realm Profile with sequence 30, or a Realm Profile signed by a new key not authorized by the old Realm signing key<br>
**Then** it is rejected with a stable security error, and the Issuer/signaling endpoints are not switched.

### AC-022: An invitation is not a URL permission

**Given** an owner issues a one-time editor invitation to a member<br>
**When** the member reuses the invitation after accepting it, or a third party obtains only the same Realm link<br>
**Then** replaying the invitation fails; the third party still has no application permission; the Realm link contains no invitation secret.

### AC-023: Remote management boundary

**Given** a remote Service in the Inventory has only been granted Attach, not host management<br>
**When** the user clicks start, stop or purge<br>
**Then** Actrium shows that remote host authorization is required; it does not fake success or create a substitute Service locally.

### AC-024: A necessary other Actor is compromised

**Given** the Front and Service are both currently green, but a necessary Actor in the Realm that holds application data has its package Critical-revoked<br>
**When** the revocation event reaches any Actr Host<br>
**Then** that node turns red, the application's weakest item turns red, and the related Actor Grant is revoked; green MUST NOT continue to be shown on the grounds that "most Actors are secure."

### AC-025: Unknown evidence is not safe

**Given** every other Actor is `CONTROLLED`, but one remote `ActorAssurance` has expired<br>
**When** the application is evaluated<br>
**Then** the overall result is yellow or cannot be established, never green; application data by default no longer flows to that Actor.

### AC-026: Optional-feature safety isolation

**Given** an optional index Actor becomes `UNKNOWN`<br>
**When** the Actr Host rejects its edge, revokes its capability, confirms it no longer holds protected data, and rolls the `structureDigest`<br>
**Then** the index feature is disabled; green MUST NOT be shown before isolation completes; once it is removed from the graph and the remaining graph is `Ready` again, the remainder can be green again.

### AC-027: Verify dynamic dependencies first

**Given** a running Service requests a new Actor for the first time<br>
**When** the peer has no current AA yet, or one is still suspended<br>
**Then** application data must not be sent, and the application must not be shown as green.

### AC-028: A malicious Service hides a dependency

**Given** a Service does not declare a downstream Actor in its self-reported material<br>
**When** it initiates `Transfer(Actor)` through a controlled capability<br>
**Then** that edge still enters the rootless graph and is re-evaluated; self-reporting cannot override an actual effective dependency.

### AC-029: The security graph has a cycle or exceeds the limit

**Given** Actors in the Realm form a call cycle, or the graph exceeds the policy limit<br>
**When** aggregation runs<br>
**Then** a cycle is deduplicated as an undirected connected component and evaluated stably; exceeding the limit returns `GRAPH_LIMIT_EXCEEDED` and must not be truncated and shown green.

### AC-030: Health and security are orthogonal

**Given** an Actor is functioning normally but is a third-party in-process Native, while another WASM Actor has crashed but its egress closure is still valid<br>
**When** the UI shows status<br>
**Then** the former shows healthy + yellow, the latter shows failed + green (if that facet is still `Ready`); they must not be merged into one vague state.

### AC-031: External HTTPS is not automatically safe

**Given** a green application sends Workspace content to an HTTPS API on an allowed domain, but that API has no proof satisfying strict policy<br>
**When** that non-Actor sink is evaluated<br>
**Then** it can only serve as a terminal sink explicitly authorized by policy; if policy requires verifying its further processing, that item is `UNKNOWN`. TLS hostname verification cannot promote the application to green.

### AC-032: Native Capsule constraints

**Given** the same native workload is run separately as a NativeCdylib and a NativeCapsule<br>
**When** the Capsule has entered an independent OS sandbox before executing code, forbidding sockets/arbitrary files/subprocesses and opening only typed Host IPC<br>
**Then** the NativeCdylib MUST NOT be `CONTROLLED`; the Capsule may be rated per the actual sandbox evidence. Whenever any platform constraint has not taken effect, the Capsule MUST fail closed or stay unknown.

### AC-033: TURN and signaling cannot decrypt

**Given** Actor A and Actor B can only communicate through a malicious signaling server and a TURN relay<br>
**When** the relay records, replays or substitutes negotiation messages<br>
**Then** the relay can only observe minimized connection metadata, and cannot obtain the payload key or plaintext; when the identity/handshake transcript does not match, the Actor edge fails to establish.

### AC-034: CC does not substitute for workload constraints

**Given** a Service correctly runs inside a confidential VM that passes attestation, but its workload has an undeclared network egress<br>
**When** the Assurance Vector is computed<br>
**Then** compute confidentiality can pass, but egress is `UNCONTROLLED` and the application is red; the hardware TEE must not label a malicious workload green.

### AC-035: Attestation mis-bound to the current session

**Given** an attacker holds a fresh, legitimate Attestation Result from another compliant TEE<br>
**When** they use that Result for a connection with a different Actor/runtime, policy, nonce or ephemeral key<br>
**Then** the Relying Party rejects it for subject/channel binding mismatch, and does not send private/secret payload.

### AC-036: Key released only after attestation passes

**Given** a secret Workspace will be processed in a remote confidential Actor<br>
**When** workload/guest-image/TCB appraisal has not yet passed, or the ephemeral key has not entered Evidence<br>
**Then** the Key Broker does not release the DEK; once it passes, the DEK is only sealed to the attested key, and an ordinary Actr Host does not get the plaintext DEK.

### AC-037: Combined CPU and NPU attestation

**Given** a confidential VM passes CPU TEE attestation, but the agent sends plaintext tensors to a GPU/NPU whose firmware, device memory or secure link cannot be attested<br>
**When** the secret policy requires compute confidentiality<br>
**Then** compute confidentiality is unknown, and the edge does not receive the secret; it is only restored once a full composite attestation satisfies policy.

### AC-038: Policy requires evidence by data category

**Given** the same WASM Actor has no CC Evidence, but identity, provenance, channel and egress closure all pass<br>
**When** it first processes public data and then requests processing of secret data that requires CC<br>
**Then** the public operation can be green; the secret operation is yellow or red; a single static rating for the Actor cannot cover both decisions.

### AC-039: Attestation expiry and revocation

**Given** a running confidential Actor's Attestation Result expires, or a vendor's TCB advisory revokes its reference value<br>
**When** the next private/secret operation runs, or the revocation event arrives<br>
**Then** new payload and key release stop, and re-attestation/rekeying is triggered; the old Result must not be relied on indefinitely.

### AC-040: A legitimate response cannot smuggle a secret

**Given** the workload only has a legitimate Actor edge returning a typed result to the Front, but tries to encode a secret into a public field<br>
**When** the output schema/data class crosses a boundary without a declassification capability<br>
**Then** the Actr Host rejects the output and marks output control as fail; a legitimate channel does not override the data-classification policy.

### AC-041: An application instance's initial green label

**Given** a new application instance has determined its Service ActorId, Program/Workspace digest and initial candidate graph<br>
**When** all Actor/Edge/Storage/Compute Evidence satisfies the current policy<br>
**Then** only after the Actr Host's `ApplicationAssurance` for `Ready` is green does it display green and release a lease on protected data. The conclusion is bound to the `structureDigest` and the current state root, not a GraphRoot.

### AC-042: A declaration is the boundary of capability

**Given** the Program Privacy Contract does not declare a given Actor role, network origin or audit sink<br>
**When** the workload tries at runtime to establish the corresponding edge or request the capability<br>
**Then** the Actr Host rejects it before creating a handle/sending data; a connection must not be established first and the application then marked yellow/red.

### AC-043: Auto-recovery under compliant scale-out

**Given** the Contract allows cardinality `0..n` for the `ai-inference` role and requires every instance to satisfy `confidential-ai-v1`<br>
**When** the scheduler requests joining three inference Actors with distinct ActorIds whose assurance is no weaker than baseline<br>
**Then** the changed nodes are suspended and re-evaluated; if the color has not weakened, it auto-recovers without asking the user. Creating an undeclared role MUST fail. A non-dependent member may `LocalContinue`, but must not display application green until `Ready`.

### AC-044: A trusted Actor Edge encapsulates an internal subgraph

**Given** an external inference service does not disclose its internal Actor graph, but provides a Certified Privacy Domain aggregate signed by a Verifier trusted by local policy<br>
**When** the aggregate's endpoint, profile, data class, evidence root, validity period and current session all match<br>
**Then** upstream may aggregate that domain as a single node and take the weakest of its conclusions; if it is only self-signed or the scope does not match, it is `UNKNOWN` or `UNCONTROLLED`.

### AC-045: A server's audit log is recognizable

**Given** an LLM server's NVIDIA/Ascend-class accelerator attestation passes, but its upper-level service may retain a prompt/response audit log<br>
**When** the audit sink is declared and proven, by a measured config plus sealed egress or a trusted Certified Privacy Domain, to record only permitted fields, retention and readers<br>
**Then** the logging gap enters the graph and can be evaluated; whenever only accelerator Evidence exists, internal retention cannot be proven, or an additional undeclared plaintext log exists, it MUST be `UNKNOWN` or `UNCONTROLLED`.

### AC-046: Graph unchanged but attestation revoked

**Given** an instance's graph structure is unchanged and already green, but a vendor then revokes the current TCB reference value<br>
**When** the revocation reaches the Actr Host / verifier<br>
**Then** the related power is revoked and re-evaluated before the next protected operation; the old conclusion must not be relied on just because the structure is unchanged.

### AC-047: Structural change with a security downgrade

**Given** a green instance requests replacing an existing confidential LLM Actor with a same-interface Actor that provides no compute attestation<br>
**When** the Actr Host builds the candidate graph<br>
**Then** the changed node is suspended; compute confidentiality drops from pass to indeterminate; the UI shows the replacement Actor and the affected data; work can continue under yellow only after risk acceptance. The color stays yellow.

### AC-048: Structural change with security unchanged or improved

**Given** a green facet replaces a `CONTROLLED` Actor with one whose identity, confinement, compute, storage and egress are no weaker than the original<br>
**When** the new state's evidence evaluation passes and it is `Ready`<br>
**Then** it auto-recovers without a user prompt; the change is written to the local audit log. Old data is still constrained by the generation it was obtained under.

### AC-049: A user cannot override a hard failure

**Given** a candidate Actor's workload digest is inconsistent with its attestation, or it has an undeclared terminal egress<br>
**When** the user tries to accept the risk<br>
**Then** the Actr Host still blocks it and provides no button to wash that item green; user consent cannot raise a hard failure or an unknown to green.

### AC-050: Freezing does not block the attestation control plane

**Given** a graph mutation has frozen the application data plane<br>
**When** the Actr Host fetches attestation and reference values and shows the Shell a degraded diff<br>
**Then** the minimal control plane can still communicate, but the suspended node and its dependents must not process application data; the old state's payload cannot pass through.

### AC-051: Creating an instance creates a new Realm

**Given** the user has already trusted the domain `team.example`<br>
**When** they select `notes.example/team-notes@1.4.2`, create an instance with `team.example` as the Realm domain, and confirm<br>
**Then** a new Realm is created under `team.example` (e.g. `k7m2q9xh@team.example`) whose Realm Profile `package` points to the selected package, and the address bar shows `actr://team.example/k7m2q9xh`; creating again yields another Realm rather than reusing the first.

### AC-052: A Realm is bound to its Actr Facility Provider

**Given** a Realm's Realm Profile has already been accepted, and a later Realm Profile arrives with a valid Realm signing key signature and a higher `sequence` but a different `provider.id`<br>
**When** the Service and Front reconnect<br>
**Then** that Realm Profile is rejected with `ACTR_FACILITY_PROVIDER_MISMATCH`, and the new Actr Facility Provider is not contacted; a higher-`sequence` Realm Profile that rotates the Realm issuer key within the same Actr Facility Provider takes effect normally, with the Realm name and ActorId unchanged.

### AC-053: A retired Realm

**Given** the domain owner has published a Realm Profile with `status = retired`<br>
**When** that Realm's Realm link is opened<br>
**Then** the connection is rejected with a stable error, and the same RealmId cannot be reused for a new Realm.

### AC-054: No fallback when the Realm Profile is unavailable

**Given** the instance Realm's Realm Profile cannot be fetched or fails signature verification, while that domain's authority descriptor declares services such as signaling in `services`<br>
**When** the user opens that instance's Realm link<br>
**Then** a Realm-unreachable or security error is shown; neither those services nor any other Actr Facility Provider is contacted, and no substitute instance is created.

### AC-055: Create transaction failure and retry

**Given** the user confirms creating an instance under a Realm domain<br>
**When** the Realm signing key holder is unreachable, or the Actr Facility Provider fails to activate the Realm, and the user then retries with the same `idempotency_key`<br>
**Then** no Service is produced on failure; the retry continues the same pending Realm (same RealmId; the Actr Facility Provider identifies it by `(realm, requester)`) instead of creating a new Realm; an expired pending Realm is cleaned up automatically.

### AC-056: Distinct invitation errors

**Given** the owner issued a one-time viewer invitation for `k7m2q9xh@acme.com`<br>
**When** a member redeems it and then redeems it again; another invitation has expired; a third was revoked by the owner; a fourth belongs to another Realm but is used for this one<br>
**Then** they are refused with `invite_used`, `invite_expired`, `invite_revoked` and `invite_realm_mismatch` respectively; the same client retrying its first redemption gets the same result; of two clients redeeming the same invitation concurrently only one succeeds.

### AC-057: Removing a member revokes all its clients

**Given** an editor has two clients, each running an online Actor<br>
**When** the owner removes that member<br>
**Then** both clients' Realm client keys are revoked: registration, renewal and reissue are refused at once with `client_revoked` or `member_removed`, their Actors' signaling connections and TURN allocations are closed and cannot reconnect, and the Service ends established Attach sessions at the latest when their credentials expire; changing another member to viewer revokes no key, and that member's next credential carries `viewer`.

### AC-058: Ownership transfer and recovery

**Given** the owner has a single client, and the Realm also has editor B<br>
**When** the owner tries to revoke its client or leave; then transfers ownership to B; later B loses all its clients and the Realm signing key holder issues a new owner invite that gets redeemed<br>
**Then** the revocation and leaving are refused with `owner_last_client` and `owner_must_transfer`; after the transfer B is the only owner and the previous owner is an editor; the client redeeming the recovery invite becomes a new client of B, all of B's previous client keys are revoked, and earlier unused owner invites stop working.

## 4. Compatibility acceptance matrix

Every release candidate covers at least:

| Dimension | Combinations |
|---|---|
| Platform | macOS, Windows, iOS, Android |
| Service | local, remote |
| Network | loopback, direct WebRTC, TURN, offline, Actr Facility Provider failover |
| Package | cached, first download, revoked, digest conflict, dev |
| Workspace | requirement: none/optional/required; provisioning: managed/user-selected; access: read-only/read-write; local/remote/locked Workspace Provider |
| Lifecycle | create, refresh, close, crash, restart, suspend, trash, restore, purge, migrate |
| Privacy | the Ready facet, generation intersection, LocalContinue, Program Privacy Contract, WASM declared/undeclared egress, NativeCdylib, NativeCapsule, expired AA, another node being red, a hidden dependency, Assured Actor Edge, LLM audit sink, opaque HTTPS, graph cycle/limit, channel binding, CC, key release, combined CPU+GPU/NPU attestation, output control, revocation propagation |

### 4.1 Platform capability commitments

| Capability | macOS | Windows | iOS | Android |
|---|---|---|---|---|
| Local Create/Attach | Full | Full | Foreground only | Foreground only |
| Local long-running Actr Host | Supported, subject to user background policy | Supported, subject to user background policy | Not committed | Committed only under a foreground service |
| Remote Attach | Supported | Supported | Supported | Supported |
| Background Guest | pause/reclaimable | pause/reclaimable | decided by the system, reclaimable | decided by the system, reclaimable |
| local-folder Workspace | Supported | Supported | security-scoped provider | SAF/provider |
| Remote Actr Host management | when a management capability is held | when a management capability is held | when a management capability is held | when a management capability is held |

A Linux/general server host, cloud auto-scheduling, a browser web version, and cross-Realm federation are not part of the Actrium v1 client's commitments; they may be defined later as an independent Actr Host.

## 5. Product metrics

### 5.1 Core funnel

```text
Link/program opened
→ package verified
→ user approved
→ Service created or attached
→ Guest interactive
→ reopened successfully
```

Tracked:

- first-creation completion rate;
- Attach success rate;
- restart-recovery success rate;
- package verification failure reasons;
- Realm/network failure distribution;
- Guest/Service crash-free session rate;
- permission denial rate;
- Workspace conflict rate;
- number of erroneously duplicated Services created (target 0).

### 5.2 Privacy

Not collected by metrics by default:

- the full ActorId;
- the Workspace name/path/content;
- the Guest route/query;
- RPC payload;
- user input.

Telemetry defaults to off. Release metrics are obtained primarily through local aggregation, voluntary diagnostics, and quality experiments that carry no business identifiers.

### 5.3 Performance measurement methodology

- use each platform's published reference hardware, a fixed package fixture, and a reproducible network profile;
- at least 30 cold/warm runs per item, reporting p50, p95, sample count and failure rate;
- "interactive" is defined as the Guest's first-screen commit completing, the input handler being registered, and the Service contract being callable;
- remote metrics report direct and TURN separately, without mixing DNS, login or user-authorization time into Attach;
- release reports retain the raw anonymized measurements and build digests, to allow regression comparison.

## 6. Release gates

### Developer Preview

- URI/Create/Attach closed loop, locally;
- fixed QuickJS;
- signed package;
- Service same-ID restart;
- basic Workspace broker.

### Public Beta

- all platforms;
- networked Realm, P2P/TURN;
- permissions and revocation;
- fault injection, fuzzing, penetration testing;
- Program/Service management UI;
- migration and delete protection.

### v1 Stable

- every MUST requirement passes;
- every AC scenario is automated or reproducibly verifiable;
- zero outstanding high-risk security audit items;
- crash-free and recovery metrics meet the release threshold;
- the protocol version and compatibility commitments are published;
- operations, revocation, key-compromise and data-recovery runbooks are complete.

## 7. Key goal tracking

| Product goal | Main requirements | Acceptance | Delivery phase |
|---|---|---|---|
| Unambiguous create and open | FR-URI-001—009, FR-SVC-007 | AC-001—004 | P0, P2-B, P3-A |
| Unified Tab/Front/Service model | FR-SH-001—006, FR-FRONT-001 | AC-002, AC-008—010 | P2-A, P3-A |
| Precise, persistent Actor identity | FR-SVC-001—003, FR-NET-005—006 | AC-009, AC-011, AC-019 | P1-A |
| Publisher and package supply chain | FR-PKG-002—014 | AC-005, AC-016, AC-021 | P1-B |
| Realm/signaling authoritative but bounded | FR-NET-001—024 | AC-011, AC-012, AC-021—023, AC-051—058 | P1-A, P3-B |
| Workspace not harmed by the UI/deletion | FR-SVC-005—016, FR-SEC-009 | AC-013—015, AC-023 | P2-B |
| Guest can run but not overreach | FR-FRONT-002—010, NFR-SEC-001—004 | AC-006, AC-007 | P2-A, P4 |
| Application-instance privacy snapshot and dynamic graph re-evaluation | FR-SEC-014—059, NFR-SEC-007—014 | AC-024—050 | P0, P1-A, P2, P3-A, P4 |
