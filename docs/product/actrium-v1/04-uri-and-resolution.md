# 04. Realm links and the address bar

## 1. Purpose

An application instance is a Realm, and its address is the Realm link. This chapter only covers
how Actrium displays, opens, saves, and shares that address; the link syntax and opening procedure
are defined in [Actr URI](../../../actr/protocols/uri.md) and are not repeated here.

- What appears in the address bar, history, favorites, and shares is always the instance address;
  opening it returns to the same instance;
- The address carries no secret; knowing it does not grant the right to enter;
- Where the program is downloaded from is decided by the Realm Profile, not written into the
  address.

An invalid package selection stops opening with `REALM_PROFILE_INVALID`. See [Realm Profile §4.2](../../../actr/protocols/realm-profile.md#42-package-selection-validation).

## 2. Link forms

```text
actr://acme.com/k7m2q9xh
actr://acme.com/k7m2q9xh/today?view=week#item-3
```

- `acme.com` is the Realm domain, and also the trust root the user sees; `k7m2q9xh` is the
  RealmId;
- The path, query, and fragment after the RealmId are handed to the app unchanged, for
  positioning within the page; they do not affect which instance is opened;
- When the first segment is written as `<serial>.<realm_id>` (e.g.
  `actr://acme.com/2af01.k7m2q9xh`), it points at a specific Actor in that instance: the opening
  procedure is the same, except it connects to that Actor directly instead of looking it up by
  type, and that Actor's type must match the package. This kind of link stops working once a
  program upgrade replaces the Actor, so instance links are the default for sharing;
- Only `actr://` is supported for Realm-link input and sharing. An `https://` address is rejected
  with `URI_SCHEME_INVALID`; Actrium does not convert it via `links`;
- A local Realm's address looks like `actr://<installation-id>.actr.alt/h3x8p2wq` and is valid
  only on this installation.

## 3. Opening

Actrium verifies the signed Realm Profile, resolves `primary_package` and `front_package` in `packages`,
and verifies both archives. It checks the Front's Service dependency and RPC contract, creates the Front Actor and Attaches to the instance's Service.

Product-level requirements:

- **Confirm first, then go online**: for a link from a web page, a QR code, an email, or another
  app, show the Realm domain and `display_name` first, and only show the program (Publisher,
  program name, version) after download and verification; first-time trust of a Realm domain must
  be confirmed. When the Realm domain equals or belongs to the Actr Facility Provider ID, the trust root
  is displayed as that Actr Facility Provider.
- **Fail closed.** Stop opening if the Profile is unavailable, invalid or retired, or either package fails verification.
- **No membership yet**: enter the join flow (e.g., a Realm invitation) instead of failing
  silently.
- The user may choose "open in browser" for an external https link; when navigating within a page
  on the same domain, Actrium does not force a takeover.
- A link cannot pre-grant any permission; unknown query parameters are handed to the app only and
  do not affect the install or permission screens.

## 4. Address bar, history, favorites, and copy link

- The address bar shows the normalized `actr://` address (normalization rules in Actr URI §6),
  with the Realm domain highlighted;
- After a new instance is created successfully, the address bar switches to the new instance's
  address with a replace (not a push): back does not return to the create-confirmation page,
  refresh connects to the new instance, and history holds only the final address;
- What gets favorited is the instance, not the program;
- "Copy link": copies the normalized `actr://` address;
- A local Realm disables "copy link"; it must not forge a domain.

Creating a new instance is not itself a link; it is a local operation inside Actrium — see
[User experience](03-user-experience.md) §5.

## 5. Development sources

In development mode, creating a new instance may use a package address on localhost, a loopback
address, or an explicitly trusted private CA. Such instances:

- always show "development source";
- do not enter the verified Program Catalog and are not accepted by the official Marketplace;
- show a risk notice before export or sharing.

Official addresses do not support HTTP fallback.

## 6. Legacy forms

The following forms are always rejected and can only be imported through an explicit migration
tool:

- `actr://<publisher>/<program>@<version>[?service=<ActorId>]` (the Create/Attach URI of the
  earlier product draft);
- a legacy ActorId containing only a numeric `realm_id`;
- `actr://<realm>:<publisher>+<name>@<version>` (an early prototype).

## 7. Error codes

See [12 §10](12-reference-spec.md) for the full list. Those related to links:

| Code | Meaning |
|---|---|
| `URI_SCHEME_INVALID` | Not a supported scheme |
| `URI_DOMAIN_INVALID` | Realm domain is invalid (contains a port, an IP literal, etc.) |
| `URI_LABEL_INVALID` | RealmId is invalid |
| `URI_LEGACY_FORM` | Legacy form; needs the migration tool to import |
| `URI_LOCAL_REALM_FOREIGN` | Local Realm address does not belong to this installation |
| `REALM_KEYS_INVALID` | The domain's `realm_signing_keys` is missing, its signature or rotation chain is invalid, or `sequence` rolls back |
| `REALM_PROFILE_UNAVAILABLE` | Could not fetch a valid Realm Profile |
| `REALM_PROFILE_INVALID` | Realm Profile signature is invalid or the Realm name does not match |
| `REALM_PROFILE_ROLLBACK` | The Realm Profile's `sequence` rolls back |
| `ACTR_FACILITY_PROVIDER_MISMATCH` | `provider.id` differs from the one already accepted |
| `REALM_RETIRED` | The Realm is retired |
| `PACKAGE_DIGEST_MISMATCH` | A downloaded archive differs from the digest pinned for its package role |
| `PACKAGE_SIGNATURE_INVALID` | Package signature verification failed |
| `SERVICE_NOT_FOUND` | The Realm has zero or multiple Actors whose type matches the pinned Service package |
| `SERVICE_ACTOR_TYPE_MISMATCH` | The linked Actor's type differs from the type in the pinned Service package |

Error codes are for logs, tests, and developer tools; user-facing copy must explain what the user
can do.
