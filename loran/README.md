# Loran

Loran is Actrium's default and reference shore-side service suite. It provides
the service composition around Actr peers: signaling, STUN/TURN, signing,
discovery, package distribution, control, administration, and observability.

The public bootstrap roles remain separate even when one Loran process serves
them together:

- `/.well-known/actr` publishes the authority descriptor;
- `loran-packages` hosts content-addressed `.actr` files that a Realm Profile's
  `package.href` may point to;
- the Publisher Registry manages publisher identity and signing keys;
- signaling, the Issuer, and ICE remain independent runtime services.

Package hosting is configured under `[services.packages]`, not inside an
Actor package. It can later be moved to another Loran node, an object store,
or a CDN without changing the Actor.

`config/` and `deploy/` define how the suite is assembled and operated. The
main daemon is under `../crates/loran-server/`; reusable services live in the
other `../crates/loran-*` directories. The server package is `loran-server`;
its executable is `loran`. Reusable packages use the `loran-` prefix, and the
deployment helper package is `loran-deploy`.

See [the naming migration guide](../docs/migration/loran-naming.md) before
upgrading an existing deployment.
