# Actr Realm Profile

Language: **English** | [简体中文](realm-profile.zh.md)

Status: initial draft.

This document defines how an Actr Host or Actr Facility Provider (AFP)
resolves a Realm name to a verified Realm Profile, the `actr.realm/v1` format,
and the Realm management requests an Actr Facility Provider accepts.

## 1. Scope

A Realm is the trust boundary of one application instance. Creating an
application instance creates a new Realm. A Realm is named `<realm_id>@<domain>`,
for example `k7m2q9xh@acme.com`; the syntax is defined in
[Actr URI](uri.md), Section 5.1.

A Realm Profile is a signed, public document that says which package the
instance runs and which Issuer and which Actr Facility Provider serve the Realm. A Realm signing key of the Realm domain signs
it, so the Realm Profile can be stored anywhere: on the domain's own static
hosting, at an Actr Facility Provider, or embedded in a Realm invitation. Where it was
fetched from does not affect its validity.

A Realm is served by one Actr Facility Provider for its whole life. An Actr Facility Provider
is a running deployment, run by some operator, that provides the Realm's
signaling, TURN, and Issuer. Moving an application to another operator means
creating a new Realm there and migrating application data; the old Realm is
retired.

This document does not define the signaling, identity, or ICE protocols an
Actr Facility Provider exposes, the Actor credential format, or application
permissions inside an application instance. Realm membership (members, Realm
invites, and Realm client keys) is defined in
[Actr Realm Membership](realm-membership.md).

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are
to be interpreted as described by BCP 14 when they appear in uppercase.

## 2. Realm domains

Every Realm lives under a Realm domain, which serves only as its naming root:
an existing, globally unique name whose ownership can be verified. Without a
domain there is no Realm, except for local Realms (Section 2.2).

### 2.1 DNS domains

A DNS Realm domain is the domain in `actr://<domain>`. Its owner is the **Realm signing key holder**: its Ed25519
Realm signing keys are the long-term trust root of every Realm under the
domain. Their publication, rotation, and rollback protection are defined by
[Actr Authority Discovery](authority-discovery.md), Section 4.4. A domain
publishes its Realm signing keys once; creating a Realm does not change the
domain's hosting.

The domain owner may hold the keys itself, typically in a signing service, or
delegate their custody to an Actr Facility Provider. A person without a domain of their
own obtains Realms under an Actr Facility Provider's domain; that Actr Facility Provider owns
the domain and holds its keys. Whoever holds the keys is the trust root of the
domain's Realms.

A Realm is **under an Actr Facility Provider's domain** when, compared as normalized
A-labels, its Realm domain equals the Actr Facility Provider ID of its Realm Profile or
ends with `"."` followed by that Actr Facility Provider ID (on a label boundary). Only
in that case do Actr Hosts show the Actr Facility Provider as the trust root; otherwise
they show the Realm domain. A domain owner that delegated custody of its keys
to an Actr Facility Provider is not visible to Actr Hosts (Section 9).

### 2.2 Local Realms

A local Realm uses the pseudo-domain `<installation-id>.actr.alt` under the
RFC 9476 `.alt` suffix. `installation-id` is 128 random bits generated at
installation, encoded as 26 characters of lowercase RFC 4648 base32 without
padding.

Actr Hosts **MUST NOT** resolve `.actr.alt` names through DNS or send them to
any Actr Facility Provider. Local Realms have no Realm Profile, no Actr Facility Provider, and
no Realm management requests; the Actr Host keeps a local record for each one, holding at least the package
the instance runs (download address or local file, plus SHA-256).
A local Realm name is usable only on that installation. The rest of this
document applies to DNS Realm domains only.

## 3. Locating a Realm Profile

For a Realm name `<realm_id>@<domain>`, an Actr Host:

1. obtains and verifies the authority descriptor of `actr://<domain>` and its
   `realm_signing_keys` member, as defined by Actr Authority Discovery;
2. selects the service links with `rel` `realms` and protocol
   `actr-realm-profile/1`;
3. for each link in document order, removes any trailing `/` from `href` and
   sends `GET <href>/<realm_id>` with `Accept: application/json`, stopping at the
   first response that yields a valid Profile.

A `realms` href **MUST NOT** contain a query or fragment. It **MAY** point to
the domain's own hosting or to an Actr Facility Provider, and **MAY** carry an explicit
port. If no link yields a valid Realm Profile (because of `404 Not Found`,
another error, a timeout, or an invalid document), the Realm Profile is
unavailable.

The request is public. Actr Hosts **MUST** use HTTPS and **MUST NOT** send
`Authorization` headers, cookies, or other credentials. Redirects follow the
rules of Actr Authority Discovery: at most five, every hop over HTTPS. A
privileged process, such as an Actr Facility Provider, **MUST** additionally apply
server-side request forgery checks on every hop: reject non-public addresses,
pin each hop to the pre-checked address, and limit the response to 64 KiB.

A Realm Profile **MAY** also be delivered out of band, for example embedded in
a Realm invitation. An Actr Host verifies such a Realm Profile exactly as one
fetched over HTTPS.

## 4. Profile format

A Realm Profile is a UTF-8 JSON object. Member names use snake_case.

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
      { "id": "7", "algorithm": "Ed25519", "public_key": "<base64>", "status": "active" }
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
      }
    ],
    "ice": [
      { "urls": ["stun:stun.provider.example:3478"] },
      { "urls": ["turns:turn.provider.example:5349"], "credential_audience": "turn-pool-1" }
    ]
  },
  "policy": {
    "membership": "invite-only"
  },
  "issued_at": "2026-07-29T00:00:00Z",
  "signature": { "key_id": "rs-2026-1", "algorithm": "Ed25519", "value": "<base64>" }
}
```

| Member | Meaning |
| --- | --- |
| `schema` | **MUST** be `actr.realm/v1`. |
| `sequence` | Realm Profile sequence: a non-negative integer that increases with every Realm Profile published for this Realm. |
| `realm.name` | The Realm name. It **MUST** equal the name being resolved. |
| `realm.display_name` | Human-readable title. It is not an identifier. |
| `realm.status` | `active` or `retired`. |
| `package.href` | HTTPS download address of the package the instance runs; it may be anywhere. |
| `package.sha256` | SHA-256 of all bytes of that package, in lowercase hexadecimal. |
| `package.type` | The Actor type `publisher/program@version` declared by that package's manifest, that is, the type of the instance's Service. |
| `issuer.url` | Registration and verification endpoint of the Issuer. |
| `issuer.keys` | Realm issuer public keys used to verify Actor credentials, 1 to 3 entries (Section 6.1). |
| `provider.id` | Actr Facility Provider ID: the Actr Facility Provider's canonical DNS authority. It never changes during the Realm's life. |
| `provider.signaling` | Signaling endpoints. Entries sharing a `control_plane_id` are replicas or failover entries of one Actr Facility Provider control plane, not a federation. |
| `provider.ice` | STUN and TURN endpoints. TURN entries name the `credential_audience` for short-lived TURN credentials. |
| `policy` | Join policy of the Realm. |
| `issued_at` | RFC 3339 time the Realm Profile was signed. It is informational; Realm Profiles do not expire. |
| `signature` | Detached signature by a Realm signing key. |

When `realm.status` is `active`, the Realm Profile **MUST** include `package`.

Every endpoint **MUST** contain a nonempty host. `package.href` and `issuer.url`
**MUST** use HTTPS; `provider.signaling[].url` **MUST** use WSS. Each
`provider.ice[].urls` list **MUST** be nonempty and contain STUN/TURN addresses
following [RFC 7064](https://www.rfc-editor.org/rfc/rfc7064) or
[RFC 7065](https://www.rfc-editor.org/rfc/rfc7065): `scheme:host[:port]`, with
brackets around IP literals and an optional `?transport=...` only for TURN.
ICE addresses **MUST NOT** contain `//`, userinfo, a path or a fragment.
Consumers **MUST** reject malformed endpoints even when the Profile signature
is valid.

A Realm Profile **MUST NOT** contain secrets, private keys, access tokens,
member lists, or user-specific data. Actr Hosts **MUST** ignore unknown
members.

## 5. Verification

`signature` is a detached Ed25519 signature over the RFC 8785 JCS canonical
form of the Realm Profile with the `signature` member removed. The Realm
signing key holder **MUST** use an `active` Realm signing key.

An Actr Host or Actr Facility Provider accepts a Realm Profile only when all of the
following hold:

1. `schema` is `actr.realm/v1`, and `realm.name` equals the Realm name being
   resolved;
2. `signature.key_id` names a key listed in the accepted `realm_signing_keys`
   member of the Realm's domain (Actr Authority Discovery, Section 4.4), and
   the signature verifies with that key;
3. if a Realm Profile for this Realm was accepted before:
   - its Realm Profile sequence is not lower than the highest accepted one,
     and if equal, the Realm Profile is identical to the accepted one;
   - `provider.id` equals that of the accepted Realm Profile;
   - the accepted Realm Profile is not retired.

Validity depends only on these checks, never on the URL the Realm Profile came
from. An Actr Facility Provider that stores a Realm Profile is therefore not a trust
root.

A retired Realm Profile (`status` `retired`) is permanent. After accepting
it, an Actr Host **MUST** stop using the Realm.

## 6. Using a Realm

An Actr Host downloads `package.href` with an HTTPS GET that carries no
credentials, **MUST** check `package.sha256`, and verifies the package
signature as in [Actor Package](actor-package.zh.md) (Chinese only); the
Actor type declared in the package manifest **MUST** equal `package.type`. If
any check fails, it does not run the package. That type is the instance's
program: the instance's Service is the one
Actor in this Realm with that Actor type; finding none or more than one is an
error.

An Actr Host reaches a Realm only through the Issuer and Actr Facility Provider named
by its accepted Realm Profile. Actor credentials for the Realm name the Realm
as their issuer and are verified with the Realm issuer keys in `issuer.keys` (Section 6.1).
Realm membership means that a client has registered its Realm client key in
the Realm and has not been revoked; it admits the client to the instance's
network, and the Issuer issues Actor credentials only to registered,
unrevoked Realm client keys ([Actr Realm Membership](realm-membership.md)); application
permissions inside the instance are decided by the application, not by this
document.

An Actr Host **SHOULD** refresh the domain's `realm_signing_keys` member and
the Realm Profile from its `realms` location when it establishes a session
with the Realm, and at least every 24 hours while it stays connected. An Actr Facility
Provider **SHOULD** refresh both for the Realms it serves at the same
interval, and stops issuing Actor credentials for a Realm whose latest Realm
Profile is retired or no longer lists the Realm issuer key it signs with.

If the key that signed an accepted Realm Profile has been removed from the
domain's `keys` (a revocation; Actr Authority Discovery, Section 4.4), the
Actr Host or Actr Facility Provider discards that Realm Profile together with its
Realm Profile sequence and `provider.id` baseline, and falls back to the last
accepted Realm Profile signed by a still-listed key; if there is none, it
obtains the Realm Profile again as if none had been accepted. This lets a
domain owner purge Realm Profiles signed with a compromised key, including
ones with an inflated sequence, by removing the key and re-signing the
affected Realm Profiles with an active key.

When a refresh finds the Realm Profile unavailable, the Actr Host keeps using
its accepted Realm Profile. When a fetched Realm Profile fails verification,
the Actr Host ignores it, keeps the accepted one, and reports the failure.
When no Realm Profile has ever been accepted and none can be obtained, the
Actr Host **MUST** report an error. In every case the Actr Host **MUST NOT**
use any Actr Facility Provider other than the one named by the accepted Realm Profile,
including services listed in the authority descriptor.

### 6.1 Realm issuer keys

`issuer.keys` lists 1 to 3 Realm issuer keys, each with an `id`, `algorithm`
(`Ed25519`), a base64 `public_key`, and `status`. An Actor credential header
carries an unsigned 32-bit integer `key_id`; `id` **MUST** be that integer in
decimal without leading zeros, for example `"7"`. An `id` names one public key
for the whole history of the Realm and **MUST NOT** be reused.

To verify an Actor credential, a verifier takes the accepted Realm Profile of
the credential's Realm, finds the key in its `issuer.keys` whose `id` matches
`key_id` (`active` or `retired`), verifies the signature with it, and checks
the validity period; it **MUST NOT** use keys of another Realm. When no key
matches, the verifier **SHOULD** refresh that Realm Profile and try again, at
most once every 60 seconds per Realm; if there is still no match, it rejects
the credential.

The Issuer signs only with an `active` key, and with one key at a time. A
`retired` key no longer signs; unexpired credentials it signed stay valid.
Removing a key from `issuer.keys` revokes it: the credentials it signed stop
being valid at once, and Actors must obtain new ones.

Rotation follows "publish, then use, then remove", each step being a Realm
Profile with a higher sequence:

1. **Pre-publish.** Add the new key (`active`); the Issuer keeps signing with
   the old key.
2. **Switch.** At least 24 hours after the Realm Profile that lists the new key
   is accepted (the refresh interval of Section 6), the Issuer signs with the
   new key; later Realm Profiles mark the old key `retired`.
3. **Remove.** Five minutes (allowing for clock skew) after the last credential
   signed by the old key expires, Realm Profiles may stop listing the old key.

An Actr Facility Provider **SHOULD** rotate every 90 days. A compromised key is removed
at once, without waiting, by publishing a Realm Profile without it. For a Realm
under the Actr Facility Provider's domain, the Actr Facility Provider signs these Realm
Profiles itself; for any other Realm, the Actr Facility Provider returns a draft with
the new key in its `draft_profile` response (Section 8), which the Realm
signing key holder signs and submits with `publish_profile`. Until the holder
publishes it, the Actr Facility Provider keeps using the old key.

## 7. Lifecycle

### 7.1 Creation

The Actr Host, the user, or the key holder's policy chooses the Actr Facility
Provider. Every step is authorized by the Realm signing key holder
(Section 7.3).

1. A `create_realm` request (Section 8) goes to the chosen Actr Facility Provider.
   The Actr Facility Provider creates a pending Realm, prepares its Realm issuer key
   and endpoints, and returns the unsigned first Realm Profile.
2. The Realm signing key holder signs that Realm Profile and submits it with
   a `publish_profile` request. The Actr Facility Provider verifies it (Section 5),
   activates the Realm, and returns an owner invite once, in that response
   (Actr Realm Membership, Section 5). If the Actr Facility Provider serves the `realms`
   location, it serves the Realm Profile from then on; otherwise the domain
   owner publishes it there.

For a Realm under the Actr Facility Provider's domain, the Actr Facility Provider signs the
Realm Profile itself, without Realm management requests (Section 7.3). A
pending Realm that is not activated in time expires and is removed.

### 7.2 Updates

A higher-sequence Realm Profile may change `package` (upgrading the program),
the Realm issuer keys, the Actr Facility Provider's endpoints, `display_name`, and
`policy`. It **MUST NOT** change
`provider.id`.

A new Realm Profile is likewise submitted to the Actr Facility Provider with
`publish_profile` (the holder may first obtain a draft with the Actr Facility
Provider's current issuer and endpoints through `draft_profile`); if the `realms` location is not at the Actr Facility Provider, the
holder also publishes it there. For a Realm under the Actr Facility Provider's domain,
the Actr Facility Provider changes `package` or `display_name` at the owner's
`update_realm` request (Actr Realm Membership, Section 4) and signs the new
Realm Profile.

### 7.3 Authorization by the key holder

Creating a Realm, publishing its Realm Profile, and reissuing its owner invite
require the authorization of the Realm signing key holder. Version 1 defines
no delegated creation keys. The Actr Host tells the cases apart with the rule
of Section 2.1:

- **A Realm under the Actr Facility Provider's domain.** The Actr Facility Provider is the
  holder. In version 1 only the Actr Facility Provider's operator creates these Realms,
  without Realm management requests, and the owner then updates or retires
  them with membership requests; the operator receives the
  owner invite at creation and hands it to the user out of band (Actr Realm
  Membership, Section 5). Self-service creation by users with Actr Facility Provider
  accounts is future work (Section 10).
- **Any other Realm.** Each Realm management request is an Actr management
  envelope signed by an active Realm signing key (Section 8), obtained from
  the signing service of the Realm signing key holder, whether that is the
  domain owner or an Actr Facility Provider it delegated custody to. How that signing
  service authenticates the people it signs for is outside this document.

### 7.4 Retirement

Retirement has one path: the domain owner publishes a retired Realm Profile.
Actr Facility Providers learn it by refreshing, write an Actr Facility Provider tombstone, and
stop serving the Realm. The Realm name is never reused. A retired Realm
Profile keeps the same `provider.id` and has a higher sequence. If the Realm's
Actr Facility Provider has shut down, the domain owner points the `realms` link at a
location it controls and publishes the retired Realm Profile there. A domain
owned by an Actr Facility Provider disappears with it: its Realms cannot be retired
and can only be recreated elsewhere. For a Realm under the Actr Facility Provider's
domain, the Actr Facility Provider publishes the retired Realm Profile at the owner's
`update_realm` request.

### 7.5 Realm signing key loss

If every Realm signing private key of a domain is lost, existing Realms can no
longer receive new Realm Profiles. The domain owner may publish a new Realm
signing key, which Actr Hosts treat as an identity change (Actr Authority
Discovery, Section 4.4), and create new Realms under new RealmIds. Keeping
the lost keys listed as `retired` lets existing Realms keep working with their
current Realm Profiles.

## 8. Realm management requests

An Actr Facility Provider accepts these Realm management requests as HTTPS `POST`
requests to the `realm-management` service link of its own authority
descriptor, `https://<provider_id>/.well-known/actr` (Actr Authority
Discovery, Section 4.3):

| `op` | Purpose |
| --- | --- |
| `create_realm` | Create a pending Realm and return its unsigned first Realm Profile (its `package` taken from the request). |
| `publish_profile` | Submit a signed Realm Profile in `profile`. The first submission activates the pending Realm and the response carries an owner invite (`owner_invite`) once; later submissions with a higher sequence are updates (including retirement). |
| `reissue_owner_invite` | Issue a new owner invite (response `owner_invite`) and invalidate the Realm's earlier unused owner invites; used to recover when the owner has lost all its clients (Actr Realm Membership, Section 5). |
| `draft_profile` | Only for Realms outside the Actr Facility Provider's domain: return the next unsigned Realm Profile draft. Its sequence is the accepted one plus one; `package`, `display_name`, and `policy` come from the accepted Realm Profile, and `issuer` and `provider` are the Actr Facility Provider's current values (including a Realm issuer key being rotated in, Section 6.1). It does not change the Realm. |

Realm management requests are used only for Realms outside the Actr Facility
Provider's domain (Section 7.3). Each request is an Actr management envelope:
the payload below plus a signature by an active Realm signing key of the Realm
domain.

The payload of a `create_realm` request is:

```json
{
  "op": "create_realm",
  "realm": "k7m2q9xh@acme.com",
  "package": { "href": "https://cdn.notes.example/team-notes-1.4.2.actr", "sha256": "<64 hex>", "type": "notes.example/team-notes@1.4.2" },
  "provider_id": "provider.example",
  "requester": "<opaque member id>",
  "nonce": "<random>",
  "issued_at": "2026-07-29T00:00:00Z",
  "expires_at": "2026-07-29T00:05:00Z"
}
```

Other operations use the same envelope with their own `op` and fields.
`requester` is optional: a key holder's signing service sets it to an opaque
identifier of the person it signs for. Other specifications **MAY** reuse the
Actr management envelope with their own `op` values; for example, Actr
Authority Discovery, Section 4.5, uses it to register a publisher, and Actr
Realm Membership uses it for membership requests.

- The payload is canonicalized with RFC 8785 JCS and accompanied by a detached
  Ed25519 signature and its `key_id`.
- `provider_id` **MUST** equal the receiving Actr Facility Provider ID, so a request
  cannot be replayed at another Actr Facility Provider.
- `expires_at` minus `issued_at` **MUST NOT** exceed five minutes. Actr Facility
  Providers reject requests outside that window.
- Actr Facility Providers record each `nonce` until the request expires and reject its
  reuse. The nonce prevents only byte-level replay.
- `create_realm` is idempotent on the Actr Facility Provider idempotency key
  `(realm, requester)`, where the requester is the `requester` field (absent
  means empty). A repeated request from the same requester for its own
  pending or active Realm returns the Realm's state, without an owner invite.
  A request that names a Realm of another requester, or a retired Realm,
  fails with a conflict.
- For an active Realm, `publish_profile` accepts only a higher-sequence Realm
  Profile and no longer returns an owner invite. If the first activation
  response is lost or the owner invite expires, `reissue_owner_invite` issues
  a new one.

Before accepting a signed request, an Actr Facility Provider verifies the Realm signing
key through Actr Authority Discovery under the same rules as Actr Hosts.
Because this fetch happens before the signature is verified, an Actr Facility Provider
**MUST** rate-limit it per source address and per target domain and **SHOULD**
cache the results, including failures. An Actr Facility Provider **SHOULD** enforce
quotas per Realm domain and per Realm.

## 9. Security and privacy considerations

- Realm signing keys are high-value keys. Offline or hardware storage is
  strongly recommended. An Actr Facility Provider holds Realm signing keys only for
  domains whose owners delegated custody to it, including its own domains.
- Realm Profiles do not expire. An Actr Host that has never seen a newer Realm
  Profile may accept an older one delivered out of band, for example in an old
  Realm invitation. Refreshing from the `realms` location bounds this window,
  and the Realm Profile sequence prevents rollback for any Actr Host that has
  seen the newer Realm Profile.
- Using only the Actr Facility Provider named by the Realm Profile prevents an
  attacker who blocks Realm Profile retrieval from redirecting Actr Hosts to
  an unauthorized Actr Facility Provider.
- The authority descriptor does not list Realms, but a Realm name reveals its
  Realm domain and RealmId wherever it is shared. Actr Hosts **SHOULD**
  generate a random RealmId by default and keep the readable title in
  `display_name`.
- Realm names and ActorIds look like email addresses. Actr Hosts **MUST NOT**
  render them as `mailto:` links and should display them as instance
  addresses ([Actr URI](uri.md), Section 5.1).
- Actr Hosts cannot see that a domain owner delegated custody of its Realm
  signing keys to an Actr Facility Provider; for such Realms they show the Realm domain
  as the trust root. This is an accepted limitation.
- A compromised Realm signing key is revoked by removing it from `keys` and
  re-signing the affected Realm Profiles (Section 6). An Actr Host that has
  not yet seen the updated `realm_signing_keys` member cannot tell; this is a
  residual risk bounded by refreshing.
- Removing a key also drops the retired Realm Profiles and `provider.id`
  baselines that rest on Realm Profiles it signed. This is an accepted
  consequence inside the domain owner's trust boundary: "a Realm name is never
  reused" is an obligation on the owner, which Actr Hosts can enforce only for
  Realm Profiles signed by still-listed keys. Actr Facility Provider tombstones are
  unaffected.
- Actr Hosts use a distinct Realm client key in each Realm, so Realms cannot
  correlate a client by its key. An Actr Facility Provider and a key holder's signing
  service know which Realms they created for whom, since they sign for them;
  Realm members do not see this.

## 10. Open work

- Define machine-readable JSON Schemas and signing test vectors for Realm
  management requests. The Realm Profile schema and interoperability vectors
  are available in [`contracts/actr`](../../contracts/actr/README.md#realm-profile);
  schema validation does not replace signature, trust or successor checks.
- Define delegated creation keys with scoped RealmId prefixes and lifetimes.
- Let users create and manage Realms under an Actr Facility Provider's domain
  themselves with Actr Facility Provider accounts (for example OAuth 2.0 authorization
  code with PKCE), with quotas per account.

## 11. References

- [Actr URI](uri.md)
- [Actr Authority Discovery](authority-discovery.md)
- [Actr Realm Membership](realm-membership.md)
- [RFC 3339: Date and Time on the Internet: Timestamps](https://www.rfc-editor.org/rfc/rfc3339.html)
- [RFC 4648: The Base16, Base32, and Base64 Data Encodings](https://www.rfc-editor.org/rfc/rfc4648.html)
- [RFC 8032: Edwards-Curve Digital Signature Algorithm (EdDSA)](https://www.rfc-editor.org/rfc/rfc8032.html)
- [RFC 8785: JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html)
- [RFC 9476: The .alt Special-Use Top-Level Domain](https://www.rfc-editor.org/rfc/rfc9476.html)
