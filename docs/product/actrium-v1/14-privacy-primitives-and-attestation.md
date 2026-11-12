# 14. Privacy primitives, remote attestation, and trusted compute

> Security semantics follow [docs/security](../../architecture/security/actrium/README.zh.md). This chapter only describes how primitives, attestation, and compute enter the existing appraisal; it does not separately define colors or the graph root.

## 1. Objective

Application green cannot come from a single technology or a single vendor; it must come from a set of composable, verifiable, and revocable privacy primitives:

```text
Declaration (what is allowed to happen)
  ↓
Enforcement (what can actually happen)
  ↓
Evidence / Attestation (how the remote side knows the constraint took effect)
  ↓
Policy Appraisal (whether the current data may enter)
  ↓
Privacy Graph Aggregation (whether the whole Actor data path holds)
```

This chapter answers:

- What "end-to-end encryption across the Actor network" actually guarantees;
- What problems WASM, Native Capsule, and Confidential Computing each solve;
- How an Agent proves its runtime environment when using CPU, GPU, NPU, or edge compute;
- How different vendors' attestations map to unified Actrium claims;
- When keys may be released;
- How multiple pieces of evidence enter the fact fields of `ActorAssurance` (`CONTROLLED / UNKNOWN / UNCONTROLLED`).

## 2. Accurate product claims

### 2.1 Do not claim "data is always ciphertext"

Ordinary computation must use plaintext within some execution boundary. Unless using specialized cryptographic computation such as FHE or MPC, Actrium should not claim that data is ciphertext at every moment.

The recommended wording is consistent with the [overview](../../architecture/security/actrium/01-overview.zh.md):

> Data exists as ciphertext on protected links and protected storage; plaintext should appear only in processing environments where identity is known and the boundary meets requirements. Green only guarantees the current ready facet; a new configuration only binds new data.

"Protection across the full lifecycle" breaks down as:

| Data state | Primary mechanism |
|---|---|
| in transit | Actor Secure Channel, AEAD, identity binding, forward secrecy, replay protection |
| at rest | Workspace envelope encryption, device Keystore/KMS, backup key policy |
| in use | WASM/Native confinement; policy may require TEE/CC for highly sensitive data |
| on output | typed results, data labels, declassification, rate limiting/aggregation/audit |

### 2.2 The boundary of "end-to-end network encryption"

Actr must guarantee, for every network Actor edge:

- mutual authentication;
- confidentiality and integrity;
- replay protection;
- the session key bound to both endpoints' Actor/runtime identity;
- use of ephemeral keys with support for rekeying;
- signaling, TURN, and ordinary relays do not hold the payload key;
- the transport cannot be downgraded to unauthenticated plaintext.

But a distinction must be made:

```text
Actor A ── encrypted edge ── Actor B ── encrypted edge ── Actor C
```

If B has to process the data, B is a plaintext execution endpoint and a necessary Actor in the privacy closure. A→C cannot skip the evaluation of B just because both links are encrypted.

Only a signaling/TURN/relay that does not process the payload is not a data execution endpoint. It can still see IP addresses, timing, packet length, and connection relationships, so "payload encryption" does not equal "metadata anonymity".

## 3. Privacy primitive matrix

| Primitive | Primary adversary | What it can prove | What it cannot prove |
|---|---|---|---|
| Actor Secure Channel | network eavesdropping, tampering, replay, relay | the payload is decrypted only by an authenticated endpoint | the endpoint won't leak the plaintext once it has it |
| `.actr` signature and provenance | CDN, mirror tampering, supply-chain substitution | the source and digest of the running artifact | the artifact itself has no malicious logic |
| WASM capability sandbox | malicious/buggy workload | imports, memory boundaries, and possible egress are enumerable and enforceable | the Actr Host/kernel won't peek; a permitted egress recipient won't misuse it |
| Native Capsule | third-party native workload | the native process cannot bypass the broker to use raw resources | zero kernel vulnerabilities; portability and verifiability equal to WASM |
| TEE / Confidential Computing | infrastructure adversaries such as cloud administrators, the host machine, the hypervisor | data in use within the designated execution boundary is hardware-isolated | the workload itself won't actively exfiltrate |
| Remote Attestation | forged runtime environment, wrong version | an Attester gave verifiable Evidence about the environment state | that state automatically satisfies business policy |
| KMS / Key Broker | an unapproved environment obtaining plaintext early | only an instance that passed appraisal gets the data key | the workload's behavior after obtaining the key is safe |
| Workspace encryption | disk, snapshot, backup leakage | data at rest cannot be read without the key | decrypted runtime data won't leak |
| Data labels and declassification | excessive output over a legitimate channel | which data may leave, and at what granularity | that a generic algorithm automatically satisfies business minimization |
| DP / MPC / FHE | adversaries in specific statistical or multi-party computation settings | reducing or avoiding plaintext exposure under a bounded model | being a generic, low-cost replacement for all ordinary computation |

Conclusions:

```text
WASM ≠ CC
CC ≠ Attestation
Attestation ≠ Authorization
Encryption ≠ Endpoint Privacy
```

The best high-privacy combination is usually:

```text
signed WASM workload
inside attested confidential execution environment
+ Actor end-to-end channel
+ attestation-gated key release
+ Host-enforced capability/egress closure
```

## 4. Internal Assurance Vector

Users still see only green / yellow / red (or "cannot be established"), but internally a single boolean is not enough.

Every Actor runtime produces at least:

```text
ActorAssuranceVector {
  identity
  artifact_provenance
  workload_confinement
  compute_confidentiality
  channel_security
  storage_protection
  egress_closure
  output_control
}
```

The assessment result for each dimension:

```text
pass | indeterminate | fail | not-required
```

Mapping:

| Dimension result | Egress fact |
| --- | --- |
| all policy-required dimensions pass | `CONTROLLED` |
| at least one required dimension is indeterminate | `UNKNOWN` |
| at least one required dimension fails | `UNCONTROLLED` |
| not required for the current data/operation | does not participate in this aggregation |

Display: all `CONTROLLED`, `Ready`, and the channel valid → green; no red but some unknown → yellow; any `UNCONTROLLED` → red; facet not ready → no color shown.

Not every Actor or every piece of data mandates CC. For example, `public` data may be allowed to run on an ordinary WASM Actr Host; an organization's policy for `secret` data may require `compute_confidentiality=pass`.

### 4.1 The subject of the green mark: Application Instance Snapshot

The conclusion is bound to the current ready facet, not to a Program in general, nor to a transient transport session. Front / Service is used only to locate the graph and does not enter `structureDigest` as a root.

```text
ApplicationStateDigest =
  H(actrium/application-state/v1,
    structureDigest,          # EncGraph
    principal,
    MerkleRoot(ActorId + ActorStateDigest + H(ActorAssurance)))
```

A change in `ActorStateDigest`, evidence, or policy invalidates the old `ApplicationAssurance`. A Tab reconnect or a direct/TURN switch (the relay never has plaintext) does not change the graph.

A Program release may provide an audited static Contract and evidence to help form the conclusion, but the Catalog can only show "which verifiable profiles are supported"; it cannot stand in for the current facet.

### 4.2 Program Privacy Contract

An application instance's structure may change dynamically, but its structural type and capability upper bound must remain static:

```text
static graph grammar
→ dynamic actor instances
```

The Contract freezes at least:

- the Actor role/type/release or assurance profile allowed to appear;
- required/optional, cardinality, and the edges that may be established;
- the data classes and operations each edge may carry;
- Actor/network/Workspace/storage/compute/audit capabilities;
- terminal egress, purpose, retention, and consent;
- the required sandbox, CC, Verifier, and trusted Privacy Domain profile;
- output/declassification rules;
- the action to take on unknown or invalid evidence.

A Service with a Program Privacy Contract includes `contracts/privacy.toml` as a `contract` in its signed manifest. Before granting capabilities, verify the file's size and SHA-256, match `release_id` to the selected Service type, and check the selected Actors against the contract. Bind the authorization decision to the contract digest and both verified archive digests. Re-evaluate it when either selection changes.

```toml
[privacy_contract]
schema_version = 1
contract_id = "notes.example/team-notes@1.4.2#privacy"
release_id = "notes.example/team-notes@1.4.2"

[[privacy_contract.actor_role]]
role = "service"
actr_type = "notes.example/team-notes@1.4.2"
cardinality = "1"
required = true

[[privacy_contract.actor_role]]
role = "ai-inference"
cardinality = "0..n"
required_assurance_profile = "confidential-ai-v1"

[[privacy_contract.edge]]
source = "service"
target = "ai-inference"
data_classes = ["private"]
capabilities = ["actor.call:infer.v1"]
```

The declaration is not descriptive text; it is the upper bound on the capability the Actr Host may grant:

```text
Effective Capability =
  Declared Contract
  ∩ User/Organization Policy
  ∩ Host Policy
  ∩ Instance Grant
```

An undeclared Actor role, edge, audit sink, terminal, file, network, or compute capability must not obtain a handle. The runtime must not expand the Contract by "connect first, report later".

The Contract's role is to make every graph change interceptable before the capability is created. It is a boundary, not the application's green mark itself.

### 4.3 State changes

There is no full-graph distributed commit. The changed Actor atomically switches `ActorStateDigest` locally, revoking the old key, old channel, and old Actor Grant in the same step, and enters assurance suspension. Events may be lost. `ActorState` carries a monotonic revision number: a configuration rollback also gets a new state root, revocation is durable, and old authority is not revived by a rollback.

```text
Change → SUSPENDED → forensics / appraisal → Resume (requires current AA) → attempt Publish
```

While suspended, application data must not be processed, and must not be sent to it. Members that don't depend on the changed node may continue their own work under `LocalContinue`, but the top level must not show application green. Old data must not have its generation rewritten to use a new egress.

The control plane (forensics, attestation, UI, consent) may continue; it must not smuggle in business data.

### 4.4 Downgrade comparison and recovery

A structural or state change does not necessarily disturb the user. Relative to the old conclusion:

```text
Color did not weaken and policy allows it
  → automatically resume related work (still requires Ready to show green)

Color turns yellow or red
  → keep blocking
  → show the exact diff
  → request risk acceptance (does not change the color)

Channel invalid or hits a platform hard floor
  → block
  → user cannot override
```

"Weaker" cannot be judged by color alone. Any required dimension going from `pass` to `indeterminate`, a profile downgrade, a change of trusted Verifier, or an expansion of terminal / audit / retention all count as a downgrade. Strengthening one dimension cannot offset another one weakening.

Risk acceptance is bound to the Principal, the object, the reason, the scope, the state root at the time, and the policy version. Consent can only let the instance keep running under yellow / red; it cannot turn missing proof into green.

### 4.5 Which events count as a structural change

Events that change structural identity or the current state (and thus invalidate the old conclusion):

- an Actor/Privacy Domain joining, being replaced, migrating, or being removed;
- a change in Actor workload/config/Host/compute/accelerator assurance;
- adding or expanding an Actor edge, data class, purpose, or terminal;
- a change to an audit/log/telemetry sink, its fields, the reading subject, or retention;
- a change of Workspace or Workspace Provider;
- an expansion of capability or declassification.

Events that do not change the graph structure (but may still trigger re-evaluation because evidence expired):

- a transport reconnect under the same Actor/runtime/evidence;
- a direct/relay/TURN path switch, provided the relay never holds the payload key;
- health, latency, or scaling metric changes that don't affect privacy claims.

An unchanged graph doesn't mean the conclusion never changes. Evidence expiry, Verifier / TCB / credential revocation, or a policy update invalidates the old `ActorAssurance` and requires re-evaluation. Display still requires `Ready`.

### 4.6 UI

| Location | Display |
|---|---|
| Catalog/install page | the privacy profiles a Program supports, the Contract, and static audit information; no instance green mark shown |
| App Library | each app's most recent `ApplicationAssurance`, state root, and update time (must indicate whether it is still Ready) |
| Active Tab address bar | green / yellow / red for the current ready facet; shows "re-verifying / communication blocked" while re-evaluating or suspended |
| Downgrade confirmation page | added/replaced Actor, audit/terminal changes, the weakest dimension, and the affected data and functionality |
| Details panel | current Actor/Edge, Assured Domain, Evidence, authorization history, and expiry time |

## 5. Data Policy determines the required evidence

Illustration:

```toml
[[data_policy]]
class = "public"
required = [
  "identity",
  "artifact_provenance",
  "channel_security",
  "egress_closure"
]

[[data_policy]]
class = "private"
required = [
  "identity",
  "artifact_provenance",
  "workload_confinement",
  "channel_security",
  "storage_protection",
  "egress_closure"
]

[[data_policy]]
class = "secret"
required = [
  "identity",
  "artifact_provenance",
  "workload_confinement",
  "compute_confidentiality",
  "channel_security",
  "storage_protection",
  "egress_closure",
  "output_control"
]
```

A Publisher may declare a minimum requirement; the user/organization may tighten it. A Publisher cannot relax the platform's or organization's floor.

## 6. Actor Secure Channel

### 6.1 Two layers of protection

The Actr network should provide, together:

1. transport protection: e.g., TLS 1.3, QUIC, or WebRTC DTLS;
2. Actor Secure Envelope: an end-to-end AEAD bound to Actor semantics.

The second layer's purpose is to:

- ensure that even if signaling, TURN, a proxy, or some transport segment is substituted, the relay still cannot obtain the payload;
- bind the cryptographic endpoint to `ActorId + runtimeInstanceId`;
- bind the privacy policy, attestation result, and the specific session together;
- prevent a single legitimate Actor credential alone from redirecting traffic to a different workload.

### 6.2 Handshake binding

The handshake transcript covers at least:

```text
protocolVersion
sourceActorId
targetActorId
sourceActorStateDigest
targetActorStateDigest
sourceRuntimeInstanceId
targetRuntimeInstanceId
sourceCredentialDigest
targetCredentialDigest
sourceEphemeralKey
targetEphemeralKey
requestedServiceActorId
policyDigest
attestationResultDigest
channelNonce
roles
```

Both parties' current `ActorStateDigest` must enter the joint transcript and be mutually authenticated: MACing only one's own fields is not enough to resist a replay of an old-state greeting (see [security protocol](../../architecture/security/actrium/03-protocol.zh.md) §3 and formal model F-CHAN-01). Once either party's `ActorStateDigest` changes, the old channel must not carry application data and must re-handshake for the new state.

The Actor's Actr Host must prove possession of the Actor key and the ephemeral session key. If the target runs inside a TEE, the ephemeral decryption public key must be generated inside the TEE, or be securely held by the TEE, and enter the attestation Evidence/Result.

### 6.3 Data plane

Every encrypted record:

- uses AEAD;
- carries a session id, direction, epoch, and sequence;
- treats the Actor edge id, data class, and message type as authenticated additional data;
- rejects nonce/sequence reuse;
- detects duplication, reordering, and epoch rollback;
- rekeys periodically or by data volume;
- a private/secret state-change request must not be sent over 0-RTT that lacks equivalent replay-protection semantics.

### 6.4 Channel Evidence

```json
{
  "schemaVersion": 1,
  "edgeId": "edge_01...",
  "sourceActorId": "<ActorId>",
  "targetActorId": "<ActorId>",
  "sourceRuntimeInstanceId": "runtime_01...",
  "targetRuntimeInstanceId": "runtime_02...",
  "suite": "actr-channel-v1",
  "payloadProtection": "actor-aead-envelope",
  "forwardSecrecy": true,
  "antiReplay": true,
  "relayCanDecrypt": false,
  "transcriptDigest": "sha256:...",
  "peerAttestationResultDigest": "sha256:...",
  "sessionKeyThumbprint": "sha256:...",
  "issuedAt": "2026-07-29T12:00:00Z",
  "expiresAt": "2026-07-29T12:10:00Z"
}
```

"TLS is connected" by itself is not enough to produce an Actor channel `pass`; Actor identity and runtime/session binding must also be completed.

## 7. Confidential Computing

### 7.1 What it solves

CC's goal is to protect data in use, so that code and data run inside a hardware-backed, provable Trusted Execution Environment.

Its main value is to remove the following parties from the privacy trust boundary, or weaken their privileges:

- the cloud platform administrator;
- the Actr Host OS / hypervisor;
- other VMs, containers, or processes on the same machine;
- some physical and device-level attackers.

Exactly which adversaries can be excluded depends on the hardware and the profile; it cannot be uniformly written as `cc=true`.

### 7.2 What it does not solve

A malicious workload running inside CC can still send data to an already-open network target. Therefore:

```text
compute_confidentiality = pass
```

cannot substitute for:

```text
workload_confinement = pass
egress_closure = pass
```

Likewise, WASM egress closure cannot prove that the cloud administrator cannot read an ordinary Actr Host's memory.

### 7.3 Canonical Compute Claims

Different TEE/CC backends must map to unified claims rather than exposing a single vendor boolean:

```json
{
  "profile": "actrium.compute-assurance.v1",
  "environmentType": "confidential-vm",
  "platformVendor": "vendor-id",
  "platformModel": "model-id",
  "teeType": "vendor-tee-profile",
  "isolationScope": "vm",
  "confidentialityScope": ["cpu-memory", "guest-state"],
  "integrityScope": ["firmware", "boot-chain", "guest-image"],
  "debugDisabled": true,
  "migrationProtection": "measured-and-policy-bound",
  "hostAdminExcluded": true,
  "workloadDigest": "sha256:...",
  "runtimeDigest": "sha256:...",
  "guestImageDigest": "sha256:...",
  "ephemeralKeyThumbprint": "sha256:...",
  "accelerators": [],
  "freshness": {
    "nonce": "...",
    "issuedAt": "2026-07-29T12:00:00Z",
    "expiresAt": "2026-07-29T12:05:00Z"
  }
}
```

The vendor/product name itself must not be interpreted as `pass`. It passes only when the corresponding Verifier profile, Endorsements, Reference Values, TCB status, and freshness all satisfy policy.

## 8. RATS role model

Actrium adopts the standard remote attestation roles:

| Role | Entity in Actrium | Responsibility |
|---|---|---|
| Attester | Actr Host TEE, device, GPU/NPU, trusted launcher | produces Evidence about the Target Environment |
| Verifier | a Realm's/organization's/local trusted Verifier and vendor adapter | verifies signatures, certificate chains, measurements, reference values, revocation, and freshness |
| Relying Party | the data-sending Actor's Actr Host, Privacy Agent, Key Broker | makes the authorization decision based on the Attestation Result and local policy |

A strict distinction is required:

```text
Evidence
  + Endorsements
  + Reference Values
  + Appraisal Policy for Evidence
  → Attestation Result

Attestation Result
  + Data Policy
  → allow | unknown | block
```

An Attester cannot issue the final `ActorAssurance` to itself (`issuer ≠ subject`). A Verifier only gives an attestation assessment; the final data release decision is still made by the relying party under current policy. The same appraisal context takes the weakest item.

Evidence encoding should preferentially be compatible with EAT/CWT/COSE; a vendor's native quote/report/document may be wrapped, but the original signature material and verification result must remain traceable.

### 8.1 Assured Actor Edge and Certified Privacy Domain

A cross-organization or large cloud service does not necessarily disclose its full internal Actor graph to the caller. Actrium allows a trusted Verifier to encapsulate an already-closed internal subgraph as a `CertifiedPrivacyDomain`:

```text
Actor A
  └── AssuredActorEdge
      └── Certified Privacy Domain B
          └── internal Actor graph (not expanded to A)
```

The boundary certificate contains at least:

- domain/endpoint identity;
- the accepted data classes, purpose, and operations;
- observable egress, retention, audit, and deletion contract;
- the internal privacy graph/evidence Merkle root;
- the assurance profile and the attack surface not covered;
- Verifier/issuer, trust chain, policy digest;
- the workload/service release scope;
- freshness, revocation, and incident obligations.

When upstream policy trusts that Verifier and profile, the entire subgraph may participate in the `meet` as a single proven node, without recursively propagating the internal unknown. This does not eliminate the unknown; it encapsulates the internal verification responsibility into a well-defined, revocable trust subject.

A Service self-attesting "I am safe" cannot form an Assured Edge. It can only do so when the Verifier is already trusted by user/organization/Realm policy, and the certificate scope matches the current endpoint/session.

## 9. Attestation and session/key binding

### 9.1 Must prevent "the attestation is real, but the connection is fake"

If legitimate Evidence is not bound to the current session, an attacker could forward the attestation of a different, healthy TEE and then route the data toward an unprotected environment.

So the attestation challenge binds at least:

```text
fresh nonce
+ ActorId
+ current ActorStateDigest
+ runtimeInstanceId
+ workload/package digest
+ policy digest
+ ephemeral channel/decryption public key
+ intended use / audience
```

### 9.2 Attestation-gated Key Release

For private/secret data, the recommended flow is:

```text
1. Scheduler selects a candidate Compute Actor
2. Relying Party sends the nonce and policy digest
3. an ephemeral key is generated inside the TEE
4. Attester generates Evidence binding the workload, key, and nonce
5. Verifier generates a short-lived Attestation Result
6. Relying Party checks Actor/edge/data policy
7. Key Broker wraps the Workspace/session DEK for the attested ephemeral key
8. unwrapping happens inside the TEE and processing begins
9. re-attest/rekey when the Result expires, the TCB is revoked, or the graph changes
```

When appraisal has not passed, the sensitive data key must not be released to an ordinary Actr Host first and then forwarded into the TEE by that Actr Host.

## 10. GPU, NPU, and heterogeneous compute

### 10.1 Composite Attestation

When an Agent uses GPU/NPU, having only the CPU TEE pass attestation is not enough. The privacy boundary may include:

```text
CPU TEE / Confidential VM
├── Guest kernel
├── runtime / container
├── driver
├── IOMMU / device assignment
├── CPU↔accelerator link
└── GPU/NPU
    ├── device firmware
    ├── CC mode
    └── device memory
```

If any link can see plaintext but cannot prove it, `compute_confidentiality` is `indeterminate`.

### 10.2 Accelerator Claims

```json
{
  "deviceClass": "npu",
  "vendor": "vendor-id",
  "model": "model-id",
  "deviceIdentityDigest": "sha256:...",
  "firmwareMeasurement": "sha256:...",
  "driverMeasurement": "sha256:...",
  "confidentialMode": true,
  "deviceMemoryProtected": true,
  "hostLinkProtected": true,
  "iommuBoundToTee": true,
  "debugDisabled": true,
  "attestationEvidenceDigest": "sha256:...",
  "endorsementChainDigest": "sha256:..."
}
```

If the accelerator has no device-level attestation, or the CPU→device transfer exposes plaintext in an ordinary Actr Host/driver:

```text
compute_confidentiality = indeterminate
reason = ACCELERATOR_CONFIDENTIALITY_NOT_PROVEN
```

### 10.3 Vendor compatibility

Actrium does not hard-code Intel, AMD, Arm, NVIDIA, Huawei Ascend, or a given cloud service as a fixed trust conclusion; instead it uses:

```text
vendor evidence
  → versioned verifier adapter
  → canonical compute claims
  → organization/Realm appraisal policy
```

Every adapter must declare:

- the supported hardware, firmware, driver, and deployment topology;
- vendor trust roots;
- the Evidence format and signature algorithm;
- how Endorsements and Reference Values are obtained;
- the TCB advisory/revocation source;
- the attack surface it can and cannot protect;
- freshness and nonce semantics;
- test vectors and negative tests;
- the adapter's own signature, version, and revocation.

Vendor marketing material can only be used for capability discovery; it cannot directly be the basis for `CONTROLLED`. A production profile is possible only once public or organization-audited verification interfaces, reference values, certificate chains, and reproducible negative tests are all in place.

## 11. Workspace and data at rest

### 11.1 Envelope Encryption

Every Workspace/Service uses at least an independent Data Encryption Key:

```text
Workspace data ── DEK
DEK ── KEK in local Keystore / organization KMS / Realm-approved KMS
```

Requirements:

- the ActorId is not the decryption key;
- the DB stores only ciphertext, key id, and necessary metadata;
- backups/snapshots stay encrypted;
- Service deletion, device revocation, or a policy change can revoke KEK/DEK access;
- key rotation does not require reissuing the ActorId;
- logs, indexes, caches, and temporary files follow the same data class.

### 11.2 CC scenarios

If policy requires confidential compute:

- KMS policy must check the Attestation Result;
- the DEK is wrapped only for an attested ephemeral key;
- decryption happens only within the execution boundary covered by the attestation;
- snapshot, migration, and accelerator offload must stay within the scope covered by the attestation;
- attestation expiry does not require immediately discarding in-memory runtime state, but after the grace window it must stop accepting new secrets and re-attest/rekey.

### 11.3 Audit logs

A server retaining audit logs must itself be an explicit storage/audit sink in the static Contract:

```toml
[[privacy_contract.audit_sink]]
kind = "security-audit"
fields = ["actor-pseudonym", "operation", "decision", "timestamp"]
payload = false
data_classes = ["metadata"]
retention = "30d"
operator_visibility = "authorized-security-admin"
encrypted = true
```

The Actr Host must bring the log capability, field schema, destination, encryption, retention period, reading subject, and export behavior into the privacy graph. When it can record payload, prompt, Workspace content, model input/output, or a stable user identifier, it must be re-evaluated under its true data class; it cannot be lumped together as "audit" to bypass policy.

Attestation can prove that a workload/environment running a given log configuration is currently running, and the Host broker can prove that only the declared audit capability was granted; it cannot prove, merely from "the server has an audit log," that the server has no other hidden recording. Therefore:

- WASM/Native confinement proves there is no other log egress that bypasses the broker;
- CC proves the Actr Host/cloud administrator cannot read memory outside the boundary and secretly record it;
- the audit sink's own storage, administrator, and exporter continue to act as a closure node or a Certified Privacy Domain;
- an undeclared log, Actr-Host-level plaintext collection, or unprovable retention makes the relevant fact `UNKNOWN` or `UNCONTROLLED`.

For an LLM Server and GPU/NPU in particular, distinguish:

- accelerator Attestation typically proves device identity, firmware, CC mode, and device state; it does not automatically prove that the LLM Server above it isn't writing prompts/responses to a log;
- if the log leaves the attested boundary through a controlled file, object storage, network, or Actor capability, the Actr Host must observe the audit edge;
- if the log logic sits inside the measured workload, the workload/config digest can prove "which logging code and configuration is running", but proving there is no other outlet still requires combining it with closed egress and a trusted Verifier;
- if it is an opaque third-party LLM API, an external caller cannot technically detect its internal retention just from GPU attestation; it can only rely on a Certified Privacy Domain's scoped proof of retention/audit. Without that proof, this item is `UNKNOWN`.

So "the server keeps an audit log" is not naturally observable from outside. It enters Actrium's provable privacy graph only once the log egress is turned into a capability, or the whole service boundary provides a verifiable aggregate for log/retention.

## 12. Output and secondary leakage

"No raw socket" does not stop a workload from stuffing a secret into a legitimate Actor response.

So highly sensitive data also needs:

- input, internal values, and output schemas carrying a data class;
- cross-category output requiring an explicit declassification capability;
- the Actr Host validating structure, size, destination, and purpose;
- aggregate queries that may require k-anonymity, a differential-privacy budget, or a minimum sample count;
- model inference explicitly classifying prompt, embedding, cache, tool result, and telemetry;
- quota, rate, and call-shape limits on high-bandwidth covert channels;
- timing/size/side-channel leakage that cannot be fully closed being explicitly disclosed in the assurance.

This still does not automatically prove data minimization for an arbitrary business algorithm; high-risk programs need dedicated policy, testing, or formal models.

## 13. Evidence Bundle

`ActorAssurance` references, rather than inlines, all the raw proofs:

```json
{
  "schemaVersion": 1,
  "bundleId": "aeb_01...",
  "subject": {
    "actorId": "<ActorId>",
    "runtimeInstanceId": "runtime_01...",
    "packageDigest": "sha256:...",
    "workloadDigest": "sha256:..."
  },
  "policyDigest": "sha256:...",
  "assurance": {
    "identity": "pass",
    "artifactProvenance": "pass",
    "workloadConfinement": "pass",
    "computeConfidentiality": "pass",
    "channelSecurity": "pass",
    "storageProtection": "pass",
    "egressClosure": "pass",
    "outputControl": "not-required"
  },
  "evidenceRefs": [
    {
      "kind": "keel-wasm-closure",
      "digest": "sha256:..."
    },
    {
      "kind": "rats-attestation-result",
      "profile": "vendor-profile@1",
      "digest": "sha256:..."
    },
    {
      "kind": "actor-channel-evidence",
      "digest": "sha256:..."
    }
  ],
  "binding": {
    "actorStateDigest": "…",
    "structureDigest": "…"
  },
  "issuedAt": "2026-07-29T12:00:00Z",
  "expiresAt": "2026-07-29T12:05:00Z",
  "signature": {
    "algorithm": "Ed25519",
    "keyId": "privacy-key-3",
    "value": "<base64>"
  }
}
```

Raw hardware Evidence may contain sensitive data such as a device serial number. An ordinary peer gets, by default, only:

- the Attestation Result;
- the necessary subset of canonical claims;
- the Evidence digest;
- the Verifier identity;
- the profile/version;
- freshness;
- the revocation check status.

## 14. Aggregation algorithm

For an edge `e = A → B` carrying data class `d`:

```text
Required(d, operation, organizationPolicy)

EdgePrivacy(e, d) = meet(
  RequiredClaims(SourceActor A),
  ChannelEvidence(e),
  RequiredClaims(TargetActor B),
  ComputeEvidence(B, d),
  StorageEvidence(B, d),
  EgressAndOutputPolicy(B, d)
)
```

The application as a whole is the weakest of the per-node \(R_A\) values on the rootless graph, and display still requires `Ready`.

Rules:

- only aggregate the dimensions required for the current data and operation;
- missing / expired / unsupported Evidence is `UNKNOWN`;
- a signature failure, a measurement mismatch, revocation, or an explicit policy rejection is `UNCONTROLLED`;
- a strong item cannot offset any required fail / indeterminate;
- a change to the graph, workload, device, or session material changes `ActorStateDigest` / `ApplicationStateDigest`;
- an Attestation Result bound to an old state cannot authorize a new edge.

## 15. Control-plane components

```text
Actrium / Actr Privacy Control Plane
├── Policy Registry
│   ├── data class policy
│   ├── organization overrides
│   └── accepted assurance profiles
├── Evidence Collectors
│   ├── Host/WASM/Native
│   ├── Channel
│   ├── Workspace
│   └── CPU/GPU/NPU attesters
├── Verifier Gateway
│   ├── RATS/EAT
│   └── versioned vendor adapters
├── Reference Value & Endorsement Store
├── Revocation/TCB Advisory Feed
├── Privacy Graph Evaluator
├── Key Broker / KMS Adapter
└── Audit & Transparency Log
```

The Realm signing key holder (the Realm domain owner), or an Actr Facility Provider it delegates to, may operate or delegate the Verifier, Reference Value, Revocation, and Key Broker services, but it is not the sole global root of trust; an individual instance Realm chooses which Verifiers to trust only within its own policy. Organizations and local Actrium installations may choose their own Verifier and appraisal policy.

## 16. Failure and revocation

The following events must trigger re-evaluation:

- a change to the workload, package, guest image, driver, or firmware digest;
- the Actor runtime being rebuilt or migrated;
- an accelerator being added/removed;
- a channel re-handshake or an endpoint key change;
- Attestation Result/Endorsement expiry;
- a vendor TCB advisory or key revocation;
- an organization policy change;
- Workspace key rotation;
- a change to `ActorStateDigest` or `structureDigest`.

Actions:

```text
indeterminate → stop sending new private/secret data, enter re-attestation
fail          → close the edge, revoke the capability, stop key release
recovered     → new nonce, new Result, channel and keys bound to the new `ActorStateDigest`
```

A temporarily unavailable Verifier is not grounds to keep relying on an indefinitely cached attestation. The local clock used to check `expiresAt`, the Attestation Result, and the Actor Grant's validity period must not be set back — both the enforcing party and the appraising party are bound by this; a clock that can be set back would let expired material be accepted again.

## 17. Stable error codes

```text
CHANNEL_IDENTITY_BINDING_MISMATCH
CHANNEL_ATTESTATION_BINDING_MISMATCH
CHANNEL_REPLAY_DETECTED
CHANNEL_DOWNGRADE_REJECTED

ATTESTATION_EVIDENCE_INVALID
ATTESTATION_SUBJECT_MISMATCH
ATTESTATION_NONCE_MISMATCH
ATTESTATION_POLICY_MISMATCH
ATTESTATION_RESULT_EXPIRED
ATTESTATION_TCB_REVOKED
ATTESTATION_PROFILE_UNSUPPORTED
ATTESTATION_VERIFIER_UNTRUSTED

COMPUTE_CONFIDENTIALITY_NOT_PROVEN
ACCELERATOR_ATTESTATION_INCOMPLETE
ACCELERATOR_CONFIDENTIALITY_NOT_PROVEN

KEY_RELEASE_POLICY_NOT_SATISFIED
KEY_RELEASE_TARGET_KEY_MISMATCH

OUTPUT_DECLASSIFICATION_REQUIRED
OUTPUT_POLICY_NOT_SATISFIED

PRIVACY_CONTRACT_INVALID
PRIVACY_CONTRACT_DIGEST_MISMATCH
PRIVACY_CERTIFICATE_EXPIRED
PRIVACY_CERTIFICATE_REVOKED
PRIVACY_RUNTIME_GRAPH_OUTSIDE_CONTRACT
PRIVACY_CAPABILITY_NOT_DECLARED
PRIVACY_GRAPH_MUTATION_PENDING
PRIVACY_DOWNGRADE_CONSENT_REQUIRED
PRIVACY_DOWNGRADE_GRANT_INVALID
PRIVACY_HARD_FLOOR_NOT_SATISFIED
PRIVACY_DOMAIN_CERTIFICATE_INVALID
PRIVACY_DOMAIN_SCOPE_MISMATCH
PRIVACY_AUDIT_SINK_NOT_DECLARED
```

## 18. Delivery order

### P0: Unified semantics

- Actor Secure Channel requirements;
- Assurance Vector;
- RATS roles and Evidence Bundle;
- data class→required claims policy;
- evidence freshness, binding, revocation.

### P1: Local and ordinary cloud

- WASM egress closure;
- Workspace envelope encryption;
- package provenance;
- a non-self-signed `ActorAssurance` (the Actr Host may assess the Guest, but must not self-sign the Actr Host);
- a public/private policy that does not require CC.

### P2: Confidential CPU

- the first production TEE/CVM profile;
- Verifier adapter;
- attestation-bound session key;
- conditional KMS/Key Broker release;
- migration, snapshot, revocation tests.

### P3: Confidential Accelerator

- CPU+GPU/NPU composite attestation;
- driver/device/firmware/reference values;
- secure device assignment/link/memory claims;
- a conformance suite for adapters across different vendors.

### P4: Advanced output privacy

- declassification;
- DP budget;
- MPC/FHE-specific workload profiles;
- side-channel assurance disclosure.

## 19. References

- [RFC 9334: RATS Architecture](https://www.rfc-editor.org/rfc/rfc9334.html)
- [RFC 9711: Entity Attestation Token](https://www.rfc-editor.org/rfc/rfc9711.html)
- [RFC 8446: TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446.html)
- [RFC 9180: HPKE](https://www.rfc-editor.org/rfc/rfc9180.html)
- [Confidential Computing Consortium](https://confidentialcomputing.io/)
- [NVIDIA Confidential Containers: CPU + GPU composite attestation](https://docs.nvidia.com/datacenter/cloud-native/confidential-containers/latest/attestation.html)
- [Huawei Cloud QingTian Enclave cryptographic attestation](https://support.huaweicloud.com/intl/zh-cn/twp-ecs/ecs_twp_0010.html)
- [Ascend Community: trusted AI computing based on Ascend TEE and remote attestation](https://www.hiascend.com/activities/dynamic-news/733)
