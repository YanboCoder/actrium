# Actr Authority Discovery

Language: **English** | [简体中文](authority-discovery.zh.md)

Status: initial draft; not yet registered with IANA.

This document defines how an Actr client discovers the public services for an
Actr authority. It is intended to become the specification referenced by the
IANA `actr` Well-Known URI registration.

## 1. Scope

Each domain publishes one public authority descriptor at
`https://<domain>/.well-known/actr`. It states which Actr services the domain
offers and may carry Realm signing keys, a Publisher Profile, and Realm link
prefixes. It does not define runtime Actor discovery, signaling,
authentication, or ICE configuration protocols; `actr:` URIs are defined by the
[Actr URI specification](uri.md).

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are
to be interpreted as described by BCP 14 when they appear in uppercase.

## 2. Discovery URI

For the domain `example.com`, the discovery resource is:

```text
https://example.com/.well-known/actr
```

It uses the default HTTPS port and applies only to that domain, not
automatically to parent domains or subdomains.

Version 1 defines no additional path components below `/.well-known/actr`,
and no specification may assign meaning to such subpaths. Information that is
specific to one Realm or to one publisher is not discovered through subpaths.
Instead, the descriptor MAY carry the signed members defined in Section 4.4
and Section 4.5, and the `realms` relation (Section 4.3) locates the Realm
Profiles defined by [Actr Realm Profile](realm-profile.md). One domain can
therefore name many Realms. A deployment that requires
independently controlled discovery documents should use distinct Actr
authorities, normally distinct hostnames.

## 3. Request

Clients **MUST** retrieve the resource with HTTPS and validate the server
certificate according to normal HTTPS rules.

Servers **MUST** support:

```http
GET /.well-known/actr HTTP/1.1
Host: example.com
Accept: application/json
```

The request is public bootstrap discovery. Clients **MUST NOT** send an
`Authorization` header, cookies, or other application credentials. Servers
**SHOULD** support `HEAD` with the same response headers as `GET` and no
response body. No semantics are defined for other methods.

## 4. Successful response

A successful response **MUST** use status `200 OK` and media type
`application/json`. The response body **MUST** be a UTF-8 JSON object conforming
to
[`authority-descriptor.schema.json`](../../contracts/actr/authority-descriptor.schema.json).

Example:

```json
{
  "version": 1,
  "authority": "actr://example.com",
  "services": [
    {
      "rel": "signaling",
      "href": "wss://signal.example.com/actr",
      "protocol": "actr-signaling/1"
    },
    {
      "rel": "identity",
      "href": "https://identity.example.com/actr"
    },
    {
      "rel": "discovery",
      "href": "https://directory.example.com/actr"
    },
    {
      "rel": "ice-config",
      "href": "https://rtc.example.com/actr/ice",
      "protocol": "actr-ice-config/1"
    },
    {
      "rel": "contracts",
      "href": "https://contracts.example.com/actr"
    }
  ],
  "links": ["/i/"]
}
```

JSON member names **MUST** be unique. Clients **MUST** reject a document with
duplicate member names, a missing required member, or an unsupported
`version`. Clients **MUST** ignore unknown members so compatible additions do
not require a new discovery URI.

### 4.1 `version`

`version` identifies the authority descriptor format. This draft defines
integer version `1`. Descriptor format evolution belongs in this member, not
in a new Well-Known URI suffix such as `actr-v2`.

### 4.2 `authority`

`authority` is the canonical `actr:` URI of the domain described by the
document, that is `actr://<domain>`, with no path, query, or fragment.

After applying the normalization rules from the Actr URI specification, a
client **MUST** verify that this value matches the authority used to construct
the discovery request. A redirect does not change this binding.

### 4.3 `services`

`services` is a non-empty array of service links. More than one entry can use
the same relation when an authority advertises multiple endpoints.

Each service link contains:

- `rel`: the role of the service.
- `href`: an absolute URI for the service endpoint.
- `protocol`: an optional Actr protocol name and major version supported by
  the endpoint.

Version 1 defines these core relations:

| Relation | Purpose |
| --- | --- |
| `signaling` | Establish Actor connectivity and exchange session negotiation messages. |
| `identity` | Resolve identities, keys, and authentication metadata. |
| `discovery` | After an Actor starts, discover available Actor instances by dependency, contract, and capability. |
| `ice-config` | Obtain ICE server configuration and, after authorization, short-lived TURN credentials as defined by [Actr ICE Configuration](ice-configuration.md). |
| `contracts` | Obtain public Actor contract and interface descriptions. |
| `realms` | Retrieve signed Realm Profiles for Realms named under this domain from where they are stored, as defined by [Actr Realm Profile](realm-profile.md). The protocol value is `actr-realm-profile/1`. |
| `realm-management` | Accept Realm management requests (`POST`) for Realms this authority serves as an Actr Facility Provider, as defined by [Actr Realm Profile](realm-profile.md), Section 8. The protocol value is `actr-realm-management/1`. |

Extension relation values **MUST** be absolute URIs controlled by the extension
author. This avoids collisions with short core relation names.

The descriptor **MUST NOT** contain access tokens, passwords, long-lived TURN
credentials, private keys, or user-specific data. In particular,
`ice-config` identifies an endpoint; it does not embed private TURN
credentials in this public document.

The `signaling` and `ice-config` links of a descriptor serve the authority
itself; they have nothing to do with the Realms named under the domain. A client **MUST** reach a Realm only through the Actr Facility Provider named
by that Realm's accepted Realm Profile and **MUST NOT** use any other Actr Facility
Provider, including these links, when the Realm Profile is unavailable,
invalid, or retired.

### 4.4 `realm_signing_keys`

`realm_signing_keys` is an optional member. When present, the domain is the
domain of Realm names `<realm_id>@<domain>`.

The member publishes the Realm signing keys, which are the long-term trust
root of every Realm named under the domain:

```json
"realm_signing_keys": {
  "sequence": 3,
  "keys": [
    { "id": "rs-2026-1", "algorithm": "Ed25519", "public_key": "<base64>", "status": "active" },
    { "id": "rs-2025-1", "algorithm": "Ed25519", "public_key": "<base64>", "status": "retired" }
  ],
  "rotations": [
    { "from": "rs-2025-1", "to": "rs-2026-1", "sequence": 2, "signature": "<base64>" }
  ],
  "signature": { "key_id": "rs-2026-1", "algorithm": "Ed25519", "value": "<base64>" }
}
```

- `sequence` is a non-negative integer no greater than 2<sup>53</sup> − 1 (the
  largest integer JCS represents exactly; the same holds for rotation entry
  `sequence`s) that increases with every change to the member.
- `keys` lists the Realm signing keys, at most 8. Each key has an `id`,
  `algorithm` (version 1 defines only `Ed25519`), a base64 `public_key`, and
  `status`. An `id` names one public key for the whole history of the domain
  and **MUST NOT** be reused. An `active` key signs; a `retired` key no longer
  signs, but signatures it made earlier, such as on existing Realm Profiles,
  stay valid, and it never becomes `active` again. Because signatures carry no
  trusted time, `retired` gives no security guarantee; revoking a key requires
  removing it. Removing a key from `keys` revokes it: nothing it signed is
  accepted any longer, so a domain owner removes a key that may be compromised
  and re-signs the affected Profiles. At least one key **MUST** be `active`.
- `rotations` lists the Realm signing key rotation chain, at most 8 entries. In
  each entry, the key named by `from` signs the JCS form of
  `{"from": ..., "to": ..., "public_key": ..., "sequence": ...}`, where
  `public_key` is that of the `to` key in `keys` and `sequence` is the member
  `sequence` that introduced it, and `signature` carries that detached
  signature.
- `signature` is a detached Ed25519 signature by an `active` key over the
  RFC 8785 JCS canonical form of the member with `signature` removed. It
  covers `sequence`.

The first acceptance of a domain's Realm signing keys relies on HTTPS (Web PKI)
or on a key pinned by enterprise policy. After that, a client or Actr Facility Provider
keeps, for the domain, every key it has ever accepted (`id`, public key, and
last state: `active`, `retired`, or removed) and the accepted member
`sequence`. It processes a newly fetched member in this order:

1. `signature` **MUST** verify with the `active` key of the same `id` in the
   member's `keys`; otherwise the member is rejected.
2. A kept `id` that appears in `keys` **MUST** have the kept public key; a key
   kept as `retired` must not appear as `active`; a removed key must not appear
   again. Otherwise the member is rejected.
3. For each rotation entry whose `from` and `to` are both in `keys`, its
   signature is verified with the `from` public key; a failure rejects the
   whole member. Entries whose `from` or `to` is not in `keys` are ignored and
   do not invalidate the member.
4. The trusted keys are the kept, not removed keys still listed in `keys`, plus
   the not yet kept keys that rotation entries introduce from a trusted key. A
   rotation entry that introduces a new key **MUST** carry a `sequence` higher
   than the accepted member `sequence`, and its `from` must not be kept as
   `retired`.
5. If the signing key is trusted and not kept as `retired`: a `sequence` lower
   than the accepted value is rejected, and an equal one requires content
   identical to the accepted member; otherwise the member is accepted, new keys
   are kept, every key's latest state is recorded, and keys no longer listed
   are recorded as removed.
6. If the signing key is not trusted, the member is an identity change: no
   `sequence` comparison is made, the user or operator must confirm trust
   again, and after confirmation the kept keys are cleared and the new member
   becomes the baseline.

A rejected member is ignored and the accepted state is unchanged.
[`realm-signing-keys-rotation.json`](../../contracts/actr/test-vectors/realm-signing-keys-rotation.json)
gives keys generated from fixed seeds, JCS strings, signatures, and the
expected result of each case.

#### 4.4.1 Rotation

A domain owner replaces an old key `old` with a new key `new` in three steps:

1. **Pre-publish.** Publish a member with `sequence` plus one: add `new`
   (`active`) to `keys`, add an `old` → `new` entry signed by `old` with this
   `sequence` to `rotations`, and still sign the member with `old`. Clients
   that pinned `old` accept `new` through it.
2. **Switch.** At least 24 hours later (the client refresh interval, see
   [Actr Realm Profile](realm-profile.md), Section 6), publish a member signed
   by `new` with `old` set to `retired`. From then on Realm Profiles and
   management envelopes are signed only with `new`, and existing Realm
   Profiles are re-signed with it.
3. **Remove.** Keep `old` listed as `retired` for at least 365 days so that
   clients that were offline for a long time can still follow the chain; then
   publish a member without `old` and without the entries whose `from` is
   `old`. A client that refreshes only after that treats the change as an
   identity change.

A compromised key is removed at once, without waiting. A domain **SHOULD**
also list a spare `active` key kept offline: after the working key is
compromised, the spare key signs a new member that introduces a new key, and
clients see no identity change.

The member **MUST NOT** list the Realms named under the domain, so that Realm names
cannot be enumerated from the descriptor.

### 4.5 `publisher`

`publisher` is an optional member that carries the authority's signed
Publisher Profile inline. When present, the domain is the Publisher ID. It replaces standalone publisher bootstrap files;
specifications **MUST NOT** define other Well-Known locations such as
`/.well-known/actr/publisher.json` or `/.well-known/actrium-publisher.json`
for this purpose.

The Publisher Profile uses snake_case member names and
`"schema": "actr.publisher/v1"`. Its full format, key roles, and rotation
rules are left to the Actor Package specification (Section 10). At the
descriptor level:

- `domain` **MUST** equal the host of `authority`;
- the Publisher Profile is signed by an active Publisher Profile signing key
  and verified by its own signature, independently of the HTTPS channel;
- HTTPS (or an enterprise pin) provides only the first-contact binding between
  the domain and the publisher's keys;
- after the first Publisher Profile is accepted, its `profile_signing_keys`
  and optional `rotations` follow Section 4.4: the same format, limits,
  acceptance rules, and rotation steps, with the Publisher Profile `sequence`
  as the member `sequence`, independently of the descriptor's HTTP caching;
  package signing keys follow [Actor Package](actor-package.zh.md) (Chinese
  only), Section 6.8.

An Actr Facility Provider that registers publishers does so through a
`register_publisher` request in the Actr management envelope of
[Actr Realm Profile](realm-profile.md), Section 8, with a `publisher` member
(the normalized Publisher ID) in place of `realm`, signed by an active
Publisher Profile signing key, not through a challenge placed in the public
descriptor.

### 4.6 `links`

`links` is an optional member: an array of path prefixes, each starting and
ending with `/` and containing at least one segment (not just `/`), for
example `["/i/"]`. It declares which https paths under this
domain are Realm links: `https://<domain><prefix><realm_id>…` corresponds to
`actr://<domain>/<realm_id>…` ([Actr URI specification](uri.md), Section 3).

- The domain chooses its prefixes; this specification defines no default.
- An Actr Host **MUST** convert https addresses only as declared here and
  **MUST NOT** infer from how a path looks.
- When several prefixes match, the longest one wins.

### 4.7 Signed and unsigned parts

The descriptor as a whole remains an unsigned public document protected by
HTTPS. `realm_signing_keys` and `publisher` are verified only by their own
signatures, and a valid signature on one of them does not authenticate any
other member. The document size limit in Section 6 applies to the whole
descriptor, including signed members.

## 5. Errors, redirects, caching, and CORS

An origin that does not support Actr discovery **SHOULD** return `404 Not
Found`. Other failures use their normal HTTP status codes. Error response
bodies are not authority descriptors.

Servers **SHOULD** serve the discovery resource directly. A client **MAY**
follow at most five redirects, but every redirect target **MUST** use HTTPS. A
client **MUST NOT** treat the redirect target as a new Actr authority and
**MUST** still validate the returned `authority` against the original Actr
authority.

Servers **SHOULD** send explicit `Cache-Control` freshness information. Clients
and intermediaries **MUST** apply normal HTTP caching semantics. This draft
does not define an implicit freshness lifetime when the server omits one.

Because the descriptor is public and contains no credentials, deployments
that support browser-based Actr clients **SHOULD** send:

```http
Access-Control-Allow-Origin: *
```

Credentialed cross-origin discovery is not defined by this specification.

## 6. Security and privacy considerations

- HTTPS is mandatory. Clients **MUST NOT** downgrade discovery or redirected
  requests to plaintext HTTP.
- Control of the discovery document represents control of the entire Actr
  authority. Write access to it must be restricted accordingly.
- Clients must validate the authority binding before using any advertised
  service.
- Clients must validate each service URI and allow only schemes supported for
  that relation. Implementations should also guard against server-side request
  forgery when a service URI is consumed by a privileged process.
- The descriptor is public, predictable, and cacheable. It must not contain
  secrets, user identifiers, or authorization state.
- Implementations should limit response size and JSON parsing resources. A
  default maximum document size of 64 KiB is recommended.

## 7. Versioning and compatibility

Compatible revisions may add optional members, new URI-valued extension
relations, or clarification without changing `version`. The `realms` relation
and the `realm_signing_keys`, `publisher`, and `links` members are such additions: clients
that do not implement them ignore them. A revision that
changes the meaning of an existing member or makes a previously valid
descriptor invalid requires a new descriptor version.

The Well-Known URI remains `/.well-known/actr` across descriptor versions.

## 8. IANA considerations

The intended provisional registration is:

```text
URI suffix: actr
Change controller: Actrium Project
Specification document: this document
Related information: none
```

The permitted URI scheme for the Well-Known URI is `https`.

This document uses the already registered `application/json` media type and
does not request a new media type. The JSON Schema is part of the Actr
specification and does not require a separate IANA registration.

## 9. References

- [RFC 3986: Uniform Resource Identifier: Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986.html)
- [RFC 8259: The JavaScript Object Notation Data Interchange Format](https://www.rfc-editor.org/rfc/rfc8259.html)
- [RFC 8615: Well-Known Uniform Resource Identifiers](https://www.rfc-editor.org/rfc/rfc8615.html)
- [RFC 9110: HTTP Semantics](https://www.rfc-editor.org/rfc/rfc9110.html)
- [RFC 9111: HTTP Caching](https://www.rfc-editor.org/rfc/rfc9111.html)

## 10. Open work

This initial draft deliberately leaves the following work open:

- stabilize the Actr URI specification and its authority mapping;
- define the individual service protocols and their allowed URI schemes;
- decide whether service ordering, priority, or failover metadata is needed;
- specify the `actr.publisher/v1` Publisher Profile format in the Actor
  Package specification;
- define delegated Realm-creation keys, which version 1 does not provide;
- add interoperability fixtures and negative test cases;
- establish the durable public specification URL and project contact used by
  the IANA registration.
