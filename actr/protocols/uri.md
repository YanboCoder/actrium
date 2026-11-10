# Actr URI Scheme

Language: **English** | [简体中文](uri.zh.md)

Status: initial draft; not yet registered with IANA.

This document defines the syntax, normalization, and opening procedure of the
`actr:` URI scheme. It is intended to become the specification referenced by
the IANA `actr` URI scheme registration.

## 1. Purpose

An `actr:` URI points to an application instance (a Realm) or to one Actor in
it. The first segment is the part of the Realm name or ActorId left of the
`@`, optionally followed by an in-app path:

```text
actr://acme.com/k7m2q9xh                    ; instance k7m2q9xh@acme.com
actr://acme.com/k7m2q9xh/today?view=week    ; same instance, with a path
actr://acme.com/2af01.k7m2q9xh              ; Actor 2af01.k7m2q9xh@acme.com
```

Opening an instance means: fetch the Realm's Realm Profile, start the
application from the package it names, then connect to the instance's Service
(Section 4). Where the program is downloaded from is stated in the Realm
Profile, not in the URI.

An `actr:` URI is not a transport address for signaling, WebSocket, WebRTC,
STUN, or TURN.

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are
to be interpreted as described by BCP 14 when they appear in uppercase.

## 2. Syntax

```abnf
actr-uri    = "actr://" domain [ "/" target [ path ] ] [ "?" query ] [ "#" fragment ]
target      = realm-id / serial "." realm-id
path        = *( "/" segment-nz )
```

`domain`, `realm-id`, and `serial` are defined in Section 5; `segment-nz`, `query`, and
`fragment` are from RFC 3986.

- `domain` is the Realm domain. It has no port or userinfo and is not an IP
  literal.
- If the first path segment contains no `.`, it is a RealmId and
  `<realm_id>@<domain>` is the Realm name; if it contains a `.`, it is
  `<serial>.<realm_id>` and adding `@<domain>` gives an ActorId. RealmIds never
  contain `.`, so the two cannot be confused.
- The remaining path, the query, and the fragment are passed to the
  application unchanged, for in-app navigation. This protocol gives them no
  meaning.
- A URI with only a `domain` and no RealmId denotes the domain itself and opens
  no application.
- Empty segments and the dot segments `.` and `..` are not allowed.

A URI **MUST NOT** carry credentials, access tokens, private keys, or other
secrets.

## 3. https form

A Realm link can also be written as an https address, so it can be clicked in
any chat tool or web page; on a device without an Actr Host, the same address
opens an ordinary web page.

The Realm domain lists path prefixes in `links` in its authority descriptor
([Actr Authority Discovery](authority-discovery.md), Section 4.6). When the
path of an https address starts with one of these prefixes, removing the
prefix gives the corresponding `actr:` URI:

```text
links: ["/i/"]

https://acme.com/i/k7m2q9xh/today?view=week  ⇔  actr://acme.com/k7m2q9xh/today?view=week
```

- An Actr Host **MUST** first fetch the domain's authority descriptor and
  convert only as its `links` declares; it **MUST NOT** guess from how a path
  looks. An https address that is not declared is an ordinary web page.
- After removing the prefix, the result must be a valid `actr:` URI
  (Sections 2 and 5); otherwise the address is also just an ordinary web page.
- A domain that declares `links` **SHOULD** serve web pages at those paths
  that include an "Open in Actrium" link to the corresponding `actr:` URI.
- Local Realms have no https form.

## 4. Opening procedure

When an Actr Host opens `actr://<domain>/<realm_id>[…]` (an instance), it:

1. Parses and normalizes the URI (Section 6).
2. For a local Realm (`domain` ending in `.actr.alt`), uses only local
   records, which hold the same package information, and skips the Realm
   Profile; otherwise obtains and validates the Realm Profile of
   `<realm_id>@<domain>` as in [Actr Realm Profile](realm-profile.md),
   Sections 3 and 5.
3. Gets the package named by `package`, checks `sha256`, and verifies the
   package signature as in [Actor Package](actor-package.zh.md) (Chinese
   only); the Actor type declared in the package
   is the instance's program.
4. Starts the foreground part of the package and passes it the rest of the
   path, the query, and the fragment.
5. Connects to the instance's Service through the Actr Facility Provider named in the
   Realm Profile: there is exactly one Actor in this Realm whose Actor type
   equals the package's; finding none or more than one is an error, and the
   Actr Host does not choose. Connecting requires Realm membership; without
   it, the Actr Host starts a join flow (for example a Realm invitation).

Opening `actr://<domain>/<serial>.<realm_id>[…]` (one Actor) follows the same
procedure, except that step 5 connects to that ActorId directly instead of
looking up by type; its Actor type must equal the package's, otherwise the
open fails with `SERVICE_ACTOR_TYPE_MISMATCH`. Such a link stops working once
the Actor is replaced (for example by a program upgrade); links to the
instance are not affected.

Holding a URI grants no access.

## 5. Realm names and ActorId

### 5.1 Realm names

```abnf
realm-name      = realm-id "@" domain
domain          = dns-name / installation-id ".actr.alt"
realm-id        = let-dig [ *61( let-dig / "-" ) let-dig ]   ; same characters as a DNS label
let-dig         = %x61-7A / DIGIT              ; lowercase a-z and 0-9
installation-id = 26( %x61-7A / %x32-37 )      ; lowercase base32, a-z and 2-7
```

- `dns-name` uses lowercase IDNA A-labels, with no port.
- `<installation-id>.actr.alt` is the pseudo-domain of one Actr Host
  installation, under the RFC 9476 `.alt` suffix. It is never resolved
  through DNS and is usable only on that installation. `installation-id` is
  128 random bits generated at installation.
- A RealmId (`realm-id`) is 1 to 63 lowercase letters, digits, or `-`, not starting or ending
  with `-`. It is unique within its domain, immutable after creation, and
  never reused after the Realm is retired. Actr Hosts generate a random RealmId
  by default; a readable RealmId is an explicit choice.

Examples: `k7m2q9xh@acme.com`, `notes@acme.com`,
`h3x8p2wq@<installation-id>.actr.alt`.

A Realm name looks like an email address but is not one. An Actr Host
**MUST NOT** render it as a `mailto:` link.

### 5.2 ActorId

```abnf
actor-id        = serial "." realm-name
serial          = "0" / non-zero-hex *12hex-digit / %x31-33 13hex-digit
hex-digit       = DIGIT / %x61-66            ; lowercase hexadecimal
non-zero-hex    = %x31-39 / %x61-66          ; 1-9 or a-f
```

For example `2af01.k7m2q9xh@acme.com`: `serial` is assigned by the Realm's
Issuer, and removing `<serial>.` gives the Realm name. Its canonical text form
is `0` or lowercase hexadecimal without leading zeros, and its unsigned value
**MUST** be at most `2^54 - 1` (`3fffffffffffff`). Consequently, a 14-digit
serial starts with `1`, `2`, or `3`, and longer serials are invalid.
Implementations **MUST** reject uppercase hexadecimal, leading zeros, and
out-of-range serials; they **MUST NOT** rewrite them into valid serials.

An ActorId contains no program type; the program type is carried by the Actor
credential. An ActorId itself is not a URI; its URI form is given in Section 2.

Shared Actor URI/ActorId serial boundary vectors are in
[`actor-uri-serial.json`](../../contracts/actr/test-vectors/actor-uri-serial.json).
Exact serial values are strings, including the decimal expectations, so JSON
consumers do not lose precision above JavaScript's safe integer range.

## 6. Normalization and comparison

1. Lowercase the scheme to `actr`;
2. convert `domain` to lowercase IDNA A-labels;
3. use uppercase hexadecimal digits in percent-encoding, and decode
   percent-encoded unreserved characters;
4. require an Actor `serial` to use the canonical text form in Section 5.2,
   without stripping leading zeros, changing case, or truncating it;
5. reject empty segments and dot segments;
6. omit a trailing `/` when there is only a `domain`.

Two URIs are equivalent when their normalized forms are byte-for-byte equal.
The path after the RealmId, the query, and the fragment are case-sensitive.
Shared parsing and normalization vectors are in
[`contracts/actr/test-vectors/uri.json`](../../contracts/actr/test-vectors/uri.json).

## 7. Legacy forms

The following forms do not conform to this specification. An implementation
**MAY** recognize them only in an explicit migration tool and **MUST NOT**
silently accept them as conforming URIs:

- `actr://<realm>:<publisher>+<name>@<version>` (early prototype);
- `actr://<publisher>/<program>@<version>[?service=<ActorId>]` (early product
  draft);
- an opaque target resolved to an entry package through a `packages` service
  (retired).

## 8. Security and privacy considerations

- URIs appear in logs, history, clipboards, and messages, so they must not
  contain secrets.
- A Realm link reveals the Realm domain and RealmId. An Actr Host
  **SHOULD** generate a random RealmId by default and put the human-readable
  name in the Realm Profile's `display_name`.
- The https form is converted only as `links` declares, so an arbitrary web
  path cannot be taken for a Realm link.
- Displays should emphasize the Realm domain (it is the trust root) and guard
  against homographs in internationalized domain names.

## 9. IANA considerations

The planned provisional registration is:

```text
Scheme name: actr
Status: Provisional
Applications/protocols that use this scheme name: Actr
Contact: Actr Protocol Maintainers
Change controller: Actrium Project
References: this document
```

## 10. References

- [RFC 3986: Uniform Resource Identifier: Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986.html)
- [RFC 7595: Guidelines and Registration Procedures for URI Schemes](https://www.rfc-editor.org/rfc/rfc7595.html)
- [RFC 9476: The .alt Special-Use Top-Level Domain](https://www.rfc-editor.org/rfc/rfc9476.html)

## 11. Open work

- Add test vectors for https conversion;
- establish the long-term public specification URL and project contact used
  for IANA registration.
