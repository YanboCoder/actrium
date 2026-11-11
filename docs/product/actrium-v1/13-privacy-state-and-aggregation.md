# 13. Actrium Privacy Marks and Graph Aggregation

> Security semantics follow [docs/security](../../architecture/security/actrium/README.zh.md). This chapter only describes how Actrium maps that model onto Tabs, Front, Actr Host and the data plane. Where the two conflict, this chapter is changed, not the current security model.

Maturity: `Specified`.

## 1. What this chapter answers

The green / yellow / red on Actrium's address bar corresponds to the `ApplicationAssurance` of a **rootless application graph** on the **current ready slice** — it is not the Program name, not Marketplace review, and not "a whole-graph simultaneous world that once existed."

```text
Tab ──locates──▶ Front Actor ──discovers──▶ Rootless application graph CC(U(D))
                                      │
                                      ▼
                         ActorAssurance (per node, self-signing forbidden)
                                      │
                                      ▼
                    ApplicationStateDigest + ApplicationAssurance
                                      │
                                      ▼
                         Ready? → show green / yellow / red
                         Else   → cannot establish / re-verifying
```

The Front Actor is only the product entry point, not the graph root. The Service is the Guest's business facade, not the security root.

Internal evidence dimensions (identity, artifact, constraint, computation, channel, storage, egress, output) are assessment inputs. The user sees only the three colors; when the slice is not ready, no color counts as the current application assurance.

Channels, primitives and remote attestation are covered in [14](14-privacy-primitives-and-attestation.md). The complete judgment and proofs are in the [formal model](../../architecture/security/actrium/formal/model.zh.md).

## 2. Colors and facts

| What the user sees | Normative meaning | Must not be written as |
| --- | --- | --- |
| Green | `Ready` and every node `CONTROLLED`, channels valid, confirmed no `Retain` | Permanent certification; a whole-graph simultaneous world; software with no vulnerabilities |
| Yellow | Not red, and at least one required fact is `UNKNOWN` | "Somewhat safe"; already leaked |
| Red | A confirmed `UNCONTROLLED` exists (including confirmed `Retain`) | The system witnessed a leak event |
| Cannot establish / re-verifying | The graph or a channel is invalid, or any member's assurance is suspended | A fourth color; can be bypassed with risk acceptance |

Risk acceptance only allows continuing to run; it does not change the color. Input authorization (camera, files) is orthogonal to color. Within the same assessment context, multiple `ActorAssurance` results take the weakest one; you cannot pick a greener verifier.

Data carries the generation at which it was obtained. New configuration applies only to new data; old data follows "the declaration at acquisition time ∩ the current declaration."

## 3. Application graph

The graph is formed by valid `Transfer(ActorId)` edges: the target resolves, both sides share the same Principal, and the declaration and state are currently valid. After dropping invalid edges, direction is ignored and the connected component is taken. Cycles are legal; inbound edges must be discovered.

```text
structureDigest = H("actrium/structure/v1", EncGraph)
EncGraph    = actrium/graph/v1 canonical JSON (sorted ActorIds and directed edges)
```

Excluded from the graph: an installed-but-not-running Program, a same-Realm Actor with no valid dependency, and a cross-Principal Actor. Egress to a cross-Principal or invalid Actor is still scored against the originator and must not be judged `CONTROLLED`.

Non-Actor targets (screen, file, Workspace, HTTPS API) are sinks the Actr Host can enforce. A TLS hostname only proves delivery to that domain, not that the domain does not leak the data further. The target's category is not chosen by the declarant: an endpoint that can present an Actr identity must be assessed as an Actor target and subject to the cross-Principal rule, and must not be recast as an HTTPS sink to bypass scoring; when the category cannot be determined, that egress stays `UNKNOWN`. A user trusting a sink means "accepting that data leaves the assurance scope." A TURN that only relays end-to-end ciphertext does not enter the graph.

The Front's final node conclusion is:

```text
R_A(Front) = join(Actrium Host platform facts, Keel Guest egress facts)
```

Egress that the Actr Host can perform but the Guest cannot is still this Actor's egress. It is not sufficient to assess only the Guest.

## 4. State changes

There is no whole-graph distributed commit. Order: locally commit the new `ActorStateDigest` → revoke old grants → suspend assurance → asynchronously rebuild the slice. `ActorState` includes a monotonic revision number maintained by the state authority: a configuration rollback also gets a new state root, and revoked old Actor Grants, channels and conclusions are not revived by the rollback.

```text
Depends(b, a)           := undirected valid dependency edge
LocalContinue(b | C)    := ACTIVE ∧ current AA still applies ∧ does not depend on C ∧ does not display application green
```

When dependency cannot be determined, stop. An Actor satisfying `LocalContinue` may continue its own work. While any member is suspended, green must not be published or displayed.

New node joining: suspend the related edges first, and only send application data after verification passes. An expansion of capability must not let old data travel through a new egress.

## 5. How a Workload is assessed

### 5.1 WASM

Judging `CONTROLLED` requires at least all of: the artifact matches the verified package; imports are enumerable; no ambient file system / socket / process API; all capabilities go through the Actr Host; the effective declaration is Manifest ∪ Actr Host ∪ environment items that cannot be excluded; each Actor target is assessed separately once it enters the graph; each non-Actor sink is accepted by policy; the evidence has not expired.

What can be measured:

```text
declared ∪ host ∪ env − proven-impossible  = effective declaration
observed ⊆ effective declaration and enforced
```

This does not prove the business logic is free of defects.

### 5.2 Third-party in-process Native

Fixed at `UNKNOWN` (or `UNCONTROLLED` if it is confirmed able to bypass the broker to send data out). Manifest, signature, static scanning and API hooks cannot launder it green.

### 5.3 Native Capsule

Only when it is a separate process with an OS sandbox applied before startup, no raw network / arbitrary file / subprocess access, and only typed broker IPC, can it be assessed on evidence. If any constraint fails, it must fail closed or stay unknown; it must not silently fall back to an ordinary process. This proves the egress closure, not memory safety.

The platform reality is unchanged: v1 WASM can be green; third-party in-process Native cannot be green; desktop Capsule is experimental; iOS third-party dynamic Native is not offered as a product capability.

### 5.4 Actrium Native Front Host

The platform Actr Host must not be blanket-judged unknown, or no Tab could ever be green. The Actr Host's conclusion comes from the official signature, installation integrity, version policy and device attestation; when the installation cannot be verified or the version has been revoked, it is likewise yellow or red. What is assessed is the Front process as a whole.

The Actr Host may provide evidence for the Guest or act as a verifier issuing `ActorAssurance`, but **issuer ≠ subject**: the Actr Host must not self-sign its own final conclusion.

### 5.5 External HTTPS

Not an Actor. Able to send without authorization: `UNCONTROLLED`. Policy requires verifying the counterparty's internal handling but there is no evidence: `UNKNOWN`. A terminal sink the user explicitly authorizes can be `CONTROLLED`, which only means the sink point is controlled, not that the counterparty is internally secure. This assumes the endpoint genuinely cannot be assessed as an Actor: an endpoint that can present an Actr identity must be modeled as an Actor target (see §3) and must not be authorized under the name of an HTTPS sink.

## 6. Declarations are input

The Program Privacy Contract and the Native Capsule profile provide inputs for verification.
The Actr Host checks them against imports, actual capabilities, and actual edges. A potentially
usable undeclared egress is red when confirmed and yellow when it cannot be confirmed.

## 7. Who issues the conclusion

```text
Claim      the current instance has some constraint enabled
Evidence   material used to verify that claim
Assurance  the conclusion a verifier gives, combining state, evidence, rules and user policy
```

The Privacy Agent inside Actrium is responsible for observation, evidence gathering, graph assembly and submitting material to verifiers. When it becomes the security basis across processes, it must be a verifier-signed `ActorAssurance`, precisely bound to `ActorId + ActorStateDigest`, the evidence fingerprint, the policy, the rules and the validity period. A relying party verifies it independently before use.

Before publishing `ApplicationAssurance`: complete discovery, check every current state, no `AA` may be stronger than the weakest item in that context, no suspended member, and re-read the current pointer. Success only means a read-consistent slice.

## 8. Product objects (mapped to the current model)

The `Privacy Mark` / `GraphRoot` / `epoch` from earlier drafts are no longer normative objects. The implementation material maps as follows:

| Old name | Current object |
| --- | --- |
| Actor Privacy Mark | The node conclusion of `ActorAssurance` |
| Application Privacy Mark | The color of `ApplicationAssurance`; display also requires `Ready` |
| privacyGraphId + epoch | `structureDigest` + `ApplicationStateDigest` |
| GraphRoot | None. The Front only locates |
| Actr Host self-signed Mark | Forbidden as the final conclusion of the subject being assessed |

Illustration (field names may be versioned in the schema repository; semantics must not be weaker than below):

```json
{
  "object": "actrium/actor-assurance/v1",
  "issuer": "verifier:…",
  "subject": { "actorId": "…", "actorStateDigest": "…" },
  "qid": "actrium.consumer.default",
  "ruleset": "…",
  "result": "CONTROLLED",
  "findings": [{ "effect": "Transfer(Actor(b))", "verdict": "CONTROLLED" }],
  "notAfter": "…",
  "signature": "…"
}
```

`ApplicationAssurance` commits to `structureDigest`, the Principal, and a Merkle root (leaves bound to `ActorId + ActorStateDigest + H(AA)`). Members prove logarithmic paths.

An edge handshake must still verify, before sending application data, the peer's `gate=ACTIVE`, its current AA, and that the channel binds both sides' current `ActorId + ActorStateDigest`. `minimumTarget` uses the node conclusion, not the old Mark name.

## 9. System protocol

Names may be kept; semantics follow the current objects:

```text
actrium.privacy.v1.GetActorAssurance
actrium.privacy.v1.GetApplicationAssurance
actrium.privacy.v1.SubscribeChanges
actrium.privacy.v1.RequireReady
```

A successful Attach only proves the direct session. The whole graph still requires a `Ready` `ApplicationAssurance`. When a channel is invalid, it is not scored; application data is blocked directly.

When the graph exceeds size limits: it must not be truncated and then shown green; it stays cannot-establish or yellow.

## 10. UI

Address bar: green / yellow / red, or "security state cannot be established / re-verifying / communication blocked."

Details must show: the policy, Actor count, workload kinds, declared egress, the weakest item, the trusted verifier(s) (when there are several, show the weakest item, with no picking), the state root, the validity period, and member proofs. After a configuration change, it must state that the new declaration applies only to new data.

Unknown must not be written as "safe," an HTTPS lock must not be written as "the application does not leak," `healthy` must not be written as green, and store review must not be written as a runtime assurance.

## 11. The graph itself is also privacy

By default, a peer only gets the necessary member proofs and a summary of the conclusion. An ordinary Guest does not get the full ActorId graph. Signaling and telemetry do not upload the business topology. The conclusion object contains no Workspace paths, payloads or tokens.

## 12. Error codes

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

## 13. v1 boundaries

Supported: WASM egress measurement; the Actrium Host as the platform TCB (must not self-sign itself); third-party in-process Native staying unknown or red; a compliant Capsule assessed on evidence; a new edge verified before data is sent; generation intersection; `Ready` display.

Not promised: business logic free of defects; that an opaque HTTPS endpoint does not leak internally; turning third-party Native green without an OS sandbox; a global privacy score; green meaning absolute safety.
