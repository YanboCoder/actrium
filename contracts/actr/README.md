# Actr contracts

Actor, transport, runtime, and protocol contracts that cross an Actr boundary
belong here.

## Actor URI serials

[`test-vectors/actor-uri-serial.json`](test-vectors/actor-uri-serial.json)
contains shared valid and invalid Actor URI/ActorId serial cases from
[`uri.md`](../../actr/protocols/uri.md), section 5.2. Valid cases include exact
decimal `serial_number` strings; do not parse them through floating-point JSON
numbers. Negative cases specify the stable URI error code.

Run the fixture-backed parser and conversion assertions with:

```sh
cargo test -p actr-protocol --lib uri::tests::actor_serial_contract_is_consistent --locked
```

## Realm Profile

[`realm-profile.schema.json`](realm-profile.schema.json) describes the public
`actr.realm/v1` document from
[`realm-profile.md`](../../actr/protocols/realm-profile.md), sections 4-6.
Use a JSON Schema Draft 2020-12 validator with format assertions enabled.
Active Profiles require a package; retired Profiles may omit it or use `null`.
Unknown members are allowed, including inside known objects. Preserve them
when constructing the signed payload.

The schema checks structure, not trust. Consumers must also verify the detached
Ed25519 signature over the RFC 8785 JCS document without `signature`, using the
domain's accepted signing keys; bind the requested Realm name; and enforce the
accepted sequence, immutable Provider identity and retirement rules. IDNA and
Actor-type semantics, unique UInt32 issuer key IDs, decoded key/signature validity
and historical key-ID reuse are not fully expressible in this schema. A valid
schema result must never bypass the protocol verifier. HTTPS/WSS and ICE URI
constraints describe the public network contract, not insecure test transports.
Every endpoint must have a nonempty host. ICE addresses follow
[RFC 7064](https://www.rfc-editor.org/rfc/rfc7064) and
[RFC 7065](https://www.rfc-editor.org/rfc/rfc7065), including bracketed IPv6
hosts and TURN transport queries. They do not use `//`, userinfo, paths or
fragments. Their scheme-specific schema deliberately does not use generic
`format: uri`, which would reject bracketed IPv6 outside a URI authority.
The protocol verifier checks these endpoint constraints even when callers
do not run the JSON Schema validator.

[`test-vectors/realm-profile.json`](test-vectors/realm-profile.json) is a shared
fixture for implementations in any language:

- `key_sets` contains separately acquired, self-signed Realm signing-key lists.
  Signing-key rotation/pinning transitions have their own existing vector file.
- `profiles` contains complete documents and their exact unsigned `jcs` strings
  (UTF-8 signature input). Signatures use public RFC 8032 section 7.1 test keys,
  independently generated with Python's `cryptography` implementation. No private
  key material is included. Unicode extension keys exercise UTF-16 JCS ordering.
- `verification_cases` selects a Profile, accepted key set and optional requested
  Realm, with independent `schema_valid` and protocol `result` expectations.
  Schema-valid tampering, unlisted/removed signing keys and issuer-ID constraints
  deliberately demonstrate why structural acceptance is insufficient.
- `successor_cases` compares two verified Profiles; `issuer_key_lookups` checks
  active and retired credential keys, including `0` and `4294967295`.
- `schema_cases` starts from a named Profile and applies JSON Pointer object/array
  replacements in `set`, then object-member removals in `remove`. These unsigned
  mutations only test schema expectations (`valid`), not signature acceptance.
- `endpoint_cases` applies `set` replacements to the active Profile and checks
  the same `valid` expectation in the schema and protocol verifier. The Rust
  suite re-signs each document with a fresh, separately accepted test key, so
  malformed endpoints are rejected even with a valid signature. Negative cases
  also name the expected `error_field`. Positive cases cover DNS, IPv4, IPv6,
  optional ports and TURN transport queries.

Run the schema and interoperability tests with:

```sh
cargo test -p actr-protocol --test realm_profile --locked
```

These tests are also included in the workspace Rust CI suite. The JSON Schema
validator is a development-only dependency, with external reference fetching
disabled. Realm management request schemas/vectors remain separate work.

## URI

[`test-vectors/uri.json`](test-vectors/uri.json) covers `actr://` parsing and
normalization from [`uri.md`](../../actr/protocols/uri.md). Valid cases specify
the normalized URI and public parsed fields, normalization idempotence, Actor
URI to `ActrId` conversion, and local-installation checks. Invalid cases specify
stable `ActrUriError::code()` values. Coverage includes IDNA, percent encoding,
application components, legacy forms, and canonical Actor serial boundaries.

Run the interoperability tests with:

```sh
cargo test -p actr-protocol --test uri --locked
```
