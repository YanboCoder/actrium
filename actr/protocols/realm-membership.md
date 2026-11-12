# Actr Realm Membership

Language: **English** | [简体中文](realm-membership.zh.md)

Status: initial draft.

This document defines how members of a Realm join with a one-time Realm
invite, how each client registers a Realm client key, how members are
managed, and how an Actr Host proves membership when it requests an Actor
credential.

## 1. Scope and model

Realms and Realm Profiles are defined in [Actr Realm Profile](realm-profile.md).
This document applies only to Realms under DNS domains; a local Realm has no
members and no invites.

- **Member**: a party the Actr Facility Provider records for a Realm, with a
  `member_id` and one role.
- **Role**: `owner`, `editor` or `viewer`. Each Realm has exactly one owner,
  which holds both the right to manage the Realm's members and the owner role
  inside the application. What `editor` and `viewer` mean is up to the
  application; the Actr Facility Provider only records the role and writes it into
  Actor credentials, it does not enforce application permissions.
- **Realm client key**: an Ed25519 key that one client of a member generates
  for this Realm. A member may have several (several devices or installs),
  each revocable on its own; it **MUST NOT** be reused across Realms.
- **Realm membership**: the member is `active` and the Realm client key used
  for a request is registered and not revoked. There is no separate
  membership credential.
- **Realm invite**: a one-time secret that stops working once redeemed. It
  either carries a role (inviting a new member) or names an existing member
  (adding a client for them).

Version 1 defines no Actr Facility Provider accounts, no multiple owners and no
domain-wide SSO.

The key words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT** and **MAY**
are to be interpreted as described in BCP 14.

## 2. Identifiers

- `key_id` of a Realm client key: the first 16 lowercase hex characters of
  the SHA-256 of the Ed25519 public key (32 bytes).
- Invite `secret`: 32 random bytes, written as base64url without padding. The
  invite's `invite_id`: the first 16 lowercase hex characters of the SHA-256
  of the secret bytes. The Actr Facility Provider stores only the SHA-256 of the
  secret.
- `member_id`: 16 lowercase hex characters assigned by the Actr Facility Provider,
  never reused within the Realm.

## 3. Realm invite

An invite is a JSON object:

```json
{
  "schema": "actr.realm-invite/v1",
  "realm": "k7m2q9xh@acme.com",
  "secret": "<base64url, 43 characters>",
  "profile": { "...": "optional embedded Realm Profile" }
}
```

Its text form is `ri_` followed by the base64url (no padding) encoding of the
JSON's UTF-8 bytes, suitable for a message, an email, a QR code or a file.

- Invites travel only out of band. An invite **MUST NOT** appear in an
  `actr://` URI, in the https form of a Realm link, or in a query or
  fragment.
- An embedded `profile` is verified per Realm Profile section 5, exactly like
  one fetched over HTTPS, and its `realm.name` **MUST** equal `realm`; the Actr
  Host still **SHOULD** refresh it from the `realms` location.
- Whoever holds an invite can redeem it, so the recipient **SHOULD** redeem it
  promptly; the issuer can revoke it before then.

## 4. Membership requests

An Actr Host sends membership requests as HTTPS `POST <issuer.url>/membership`
(protocol `actr-realm-membership/1`), with `issuer.url` taken from the Realm's
accepted Realm Profile. The body is the Actr management envelope of
[Actr Realm Profile](realm-profile.md) section 8: `op`, `realm`,
`provider_id`, `nonce`, `issued_at`, `expires_at` and the operation's fields,
signed with a Realm client key whose `key_id` is `signature.key_id`. The
envelope's canonicalization, validity window (at most five minutes),
`provider_id` binding and nonce rules are unchanged; membership requests carry
no `requester`.

| `op` | Fields | Who may send it | Effect |
| --- | --- | --- | --- |
| `join` | `invite` (the secret), `public_key`, optional `label` | Whoever holds the invite, signing with the new key | Redeems the invite and registers the key; responds `{realm, member_id, role, key_id}` |
| `create_invite` | `role` (`editor`/`viewer`) or `member_id`, plus `invite_expires_at` | New-member invite: the owner; client invite: the owner, or a member for itself | Responds `{invite_id, invite, expires_at}`, where `invite` is the text form, returned only this once |
| `revoke_invite` | `invite_id` | The owner, or whoever issued the invite | The unused invite stops working |
| `list_members` | none | Any member | The owner gets all members, clients and unused invites; other members get only their own record, clients and the invites they issued |
| `revoke_client` | `key_id` | A member for its own clients; the owner for any client | The key is revoked (section 6), together with the unused invites it issued |
| `remove_member` | `member_id` | The owner (not for itself) | The member is removed and all its Realm client keys revoked |
| `set_role` | `member_id`, `role` (`editor`/`viewer`) | The owner (not for itself) | Changes only the role |
| `transfer_owner` | `member_id`, optional `previous_owner_role` (default `editor`) | The owner | Atomically hands ownership to another active member that has at least one unrevoked client key; the previous owner takes the given role |
| `leave` | none | A member other than the owner | Removes itself, with the effect of `remove_member` |
| `update_realm` | optional complete selection (`primary_package`, `front_package` and `packages`), `display_name`, or `status: "retired"` | Owner of a Realm under the AFP domain | The AFP validates and signs a higher-sequence Profile and returns `{realm, sequence}` (Realm Profile Sections 7.2 and 7.4). |

After a selection update, the caller verifies the signed Profile and compares both selectors and the complete map with its request. See [Realm Profile Section 7.2](realm-profile.md#72-updates).

In a `join` request, `signature.key_id` **MUST** equal the `key_id` computed
from `public_key`; the signature is the proof of possession. For every other
request the signing key **MUST** be a registered, unrevoked Realm client key of
an `active` member of the Realm.

The Actr Facility Provider checks the role before looking up the target: a request
the sender may not make always gets `forbidden`, so `not_found` never reveals
whether a target exists. Other Realms are still updated and retired by the
Realm signing key holder through Realm management requests; `update_realm` for
them gets `forbidden`.

`label` is a client name of at most 64 characters that lets members tell their
clients apart. `invite_expires_at` is an RFC 3339 time no more than 30 days
after issuance.

### 4.1 Redemption

The Actr Facility Provider redeems an invite in a single transaction: it marks the
invite used, creates the new member or finds the target member, and registers
the key. Of two concurrent redemptions at most one succeeds; when a quota would be
exceeded (`quota`) the invite is not consumed. Redeeming the same invite again
with the same key (a retry after a lost response) returns the first result as
long as the key is still unrevoked and the member still `active`; otherwise
the answer is `invite_used`.

- An invite with a `role` creates a new member. Invites with the `owner` role
  (owner invites) are issued only by the Realm signing key holder or the Actr Facility
  Provider operator (section 5): if the Realm has no owner yet, redeeming one
  creates the owner; if it has one, the new key becomes a client of the owner
  member and all of the owner's previous Realm client keys are revoked.
- An invite with a `member_id` registers a new client for that member, which
  **MUST** still be `active`.

### 4.2 Owner protection

- The owner **MUST NOT** revoke its own last unrevoked Realm client key,
  remove itself or `leave`; it must `transfer_owner` first.
- The `owner` role arises only from an owner invite or `transfer_owner`;
  `create_invite` and `set_role` do not accept `owner`.

### 4.3 Errors

The Actr Facility Provider returns errors as JSON `{"error": "<code>", "message":
"<text>"}`; `code` is stable so that Actr Hosts can branch on it:

| HTTP | `code` | Meaning |
| --- | --- | --- |
| 400 | `invalid_request` | Malformed, wrong `provider_id`, validity window, reused nonce, or `key_id` not matching `public_key` |
| 403 | `client_unknown` | The signing key is not registered in the Realm, the signature is invalid, or the Realm does not exist |
| 403 | `client_revoked` | The signing key has been revoked |
| 403 | `member_removed` | The member has been removed or has left |
| 403 | `forbidden` | The role may not perform this operation |
| 403 | `realm_unavailable` | The Realm is not `active` |
| 404 | `invite_unknown` | No invite matches the secret |
| 404 | `not_found` | The target member, client or invite does not exist |
| 409 | `invite_realm_mismatch` | The invite belongs to another Realm |
| 409 | `owner_last_client`, `owner_must_transfer`, `conflict` | Owner protection, key already registered, or target in the wrong state |
| 410 | `invite_expired`, `invite_used`, `invite_revoked` | The invite has expired, has been used, or has been revoked |
| 429 | `quota` | Too many members, clients or unused invites |

`realm_unavailable` is returned only after the requester has proven that it
holds a registered key or a valid invite; otherwise the answer is
`client_unknown` or `invite_unknown`, revealing nothing about the Realm's
state. Expired, used, revoked and Realm-mismatched invites **MUST** be distinct
errors.

## 5. Issuing owner invites

An owner invite is an invite with `role = owner`, valid for 7 days. Issuing a
new owner invite invalidates the Realm's earlier unused owner invites.

- **Other Realms.** When the Realm signing key holder first activates a Realm
  with `publish_profile`, the response carries an `owner_invite` (text form)
  once; afterwards the holder issues new ones with `reissue_owner_invite`
  (Realm Profile section 8) to recover when the owner has lost all its
  clients.
- **Realms under the Actr Facility Provider's domain.** In version 1 only the Actr Facility
  Provider's operator creates them and receives the owner invite at creation,
  handing it to the user out of band; the operator also issues recovery
  invites. How it does so is outside this protocol. The owner then updates or
  retires the Realm with `update_realm`.

## 6. Actor credentials and membership

When an Actr Host registers, reissues or renews an Actor credential, or enrolls
a relay identity, it
**MUST** prove possession of a Realm client key of the Actor's Realm. The proof
goes in an HTTP request header:

```text
x-actr-client-proof: <base64url (no padding) of UTF-8 JSON>
```

The JSON is an Actr management envelope whose `op` is `register`, `reissue`,
`renew` or `relay_identity` (matching the registration, reissue, renewal and
relay identity endpoints), with an extra `body_sha256`: the SHA-256 of the HTTP
request body bytes, in lowercase hex. `realm` **MUST** equal the Realm in the request body. The Issuer checks
`body_sha256`, the signature, the validity window, `provider_id` and the
nonce. An idempotent retry of the same body carries a freshly signed proof each
time.

The Issuer issues only when all of the following hold:

1. the signing key is registered in the Realm, not revoked, and its member is
   `active`;
2. for renewal, the signing key is the one the Actor is currently bound to;
   for reissue, the signing key belongs to the Actor's member, and a
   successful reissue rebinds the Actor to that key (so a member holding the
   Actor private key can move its Actor to another of its clients). Relay
   identity enrollment also requires the currently bound key and an unexpired
   renewal token;
3. at registration, resolve `packages[primary_package]` from the verified Profile.
   The Issuer **MUST** reject an invalid primary reference. If the Actor type's publisher and
   program name equal those of this primary's `type` (in any version, that is, the
   instance's Service and its upgrades), the signing key belongs to the owner.
   Renewal and reissue do not check the role again, so an ownership transfer
   does not affect a running Service.

Package selection does not change membership or Actor-admission rules.

A connection ACL carried by a registration applies only to the registering
Actor's own type; since only the owner can register the Service's program,
other members cannot rewrite the Service's ACL.

Actor credential claims include the member id, the Realm client key id and
the role at issuance. A Service **MAY** use them to decide a Front's
application permissions; it **MUST** treat the Front credential's expiry as the
limit of that decision and read the role again when the Front presents a
renewed credential.

When a Realm client key is revoked (including by removing a member, `leave`
and owner recovery), the Actr Facility Provider **MUST** immediately:

- refuse new requests signed with the key, including Actor credential
  issuance, and revoke the unused invites it issued;
- disable every Actor currently bound to the key: refuse their renewal, close
  their signaling connections and TURN allocations, and refuse their
  reconnection and TURN credentials.

The ActorIds are not voided: the same `active` member can reissue with another
of its unrevoked keys (item 2 above), which brings the Actor back. A removed
member, or one that has left, has no usable key, so its Actors stay disabled
for good. Actor credentials already issued stop working outside the Actr Facility
Provider (for example, where a peer Actor verifies them) when they expire. Changing a role
revokes no key; it takes effect in the member's next Actor credential.

TURN credentials bind the issuing Realm client key as well as the ActorId.
Their HMAC-covered username contains `:client_key=<key_id>:actor=<actor_id>`.
The Provider checks that binding against the Actor's currently authorized key
before accepting cached or uncached authentication, and closes allocations
whose binding is no longer current. Reissuing an Actor under another key must
never make the old device's TURN credentials usable again. Legacy credentials
without a key binding are refused; clients obtain bound credentials by renewal.

A Realm client key is a capability of the Actr Host and **MUST NOT** be given
to Guest workloads.

### 6.1 Package manifest registration

Package-mode `RegisterRequest` carries the original `manifest.toml` bytes in
`manifest_raw` and the original 64-byte `manifest.sig` in `publisher_signature`.
The Host **MUST NOT** reserialize the manifest, inject legacy fields, or replace
the Publisher signature. Its Realm client proof signs the exact Protobuf body.
The same registration material is retained for `/issuer/reissue`.

| Manifest | Identity and signing key | Request `target` |
| --- | --- | --- |
| `format = "actr-package/2"` | `actor.publisher/name/version`, `signature.key-id`, Ed25519 | **MUST** be absent; an empty string is also rejected |
| Existing unversioned binary manifest | Top-level `publisher/name/version/signing_key_id` | Required, nonempty, and equal to signed `binary.target` |

A format declaration or versioned `actor/workload/host/signature` table selects
the versioned parser exclusively. Unsupported versions, `actr-package/1`, mixed
schemas and invalid v2 documents **MUST NOT** fall back to the unversioned or
linked path. The manifest is bounded to 1 MiB. The requested Actor type must
exactly equal the signed type, and the key id must identify the Ed25519 key that
verifies the original bytes. Format/target-shape failures are rejected as invalid
requests (Protobuf error code 400); identity/signature failures are rejected as
unverified provenance (403).

For v2, execution requirements come from the signed workload/Host profiles.
The Host checks payload hashes, entry points, ABI and capabilities. Issuer
validates the manifest schema, signature, Publisher provenance and Realm
authorization; it does not receive the ZIP or attest to remotely executed code.
Supplying a public key does not grant trusted Publisher provenance. The Host
needs its own Realm client key and public package material, not the Publisher
private key or a package-publication record.

Renewal keeps the existing member/client/Realm/Publisher checks. Reissue verifies
the original package-mode request and the Actor possession/member/type binding,
and preserves ActorId. A v2 registration records a null legacy target; a successful
v2 reissue clears any previous legacy target atomically with its response.

## 7. Security and privacy considerations

- Each Realm uses its own Realm client keys, so Realms cannot correlate one
  client through its key; `label` is chosen by the client, and Actr Hosts
  **SHOULD** default to a name without personal information.
- An invite is a bearer secret: a leaked unused invite can be redeemed by
  someone else. Issuers **SHOULD** choose short validity and revoke on
  suspected leaks; the owner sees every member and its clients in
  `list_members` and can remove anyone who got in by mistake. Tools **SHOULD
  NOT** expose an invite where other local users can see it, such as a
  command-line argument.
- A Realm client key's private key stays on its client. Actr Hosts **SHOULD**
  refuse to load a key file that other local users can read.
- The holder of the Realm signing key and the operator can recover ownership
  through owner invites, so they sit inside the Realm's trust boundary, which
  is consistent with their existing power to sign Realm Profiles or suspend the
  Realm (the Actr Facility Provider also holds the Realm issuer key).
- Owner recovery revokes all of the owner's previous client keys and the unused
  invites they issued, so a stolen owner key cannot keep fighting the
  recovering party. While it was stolen, the other party may have invited
  members or transferred ownership; after recovery the owner **SHOULD** review
  `list_members` and remove them.
- An Actor credential's lifetime bounds how long a revocation takes to reach
  places outside the Actr Facility Provider; Actr Facility Providers **SHOULD** keep it short.

## 8. Test vectors and schema

- `contracts/actr/test-vectors/realm-membership.json`: `key_id`, `invite_id`,
  invite text encoding, `join` / `create_invite` / `transfer_owner` envelopes
  and signatures, `x-actr-client-proof` (with `body_sha256`), and negative
  cases;
- `contracts/actr/realm-membership.schema.json`: JSON Schema for invites,
  requests, responses and the client proof.

## 9. References

- [Actr Realm Profile](realm-profile.md)
- [Actr URI](uri.md)
- [RFC 3339: Date and Time on the Internet: Timestamps](https://www.rfc-editor.org/rfc/rfc3339.html)
- [RFC 4648: The Base16, Base32, and Base64 Data Encodings](https://www.rfc-editor.org/rfc/rfc4648.html)
- [RFC 8032: Edwards-Curve Digital Signature Algorithm (EdDSA)](https://www.rfc-editor.org/rfc/rfc8032.html)
- [RFC 8785: JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785.html)
