# loran-packages

`loran-packages` is Loran's content-addressed hosting for `.actr` files. It is
a plain static download service: each configured file is served by its
SHA-256 digest, so a Realm Profile's `package.href` may point at it.

The component does not resolve URI targets and is not advertised in the
authority descriptor. It is independent of authority discovery and of the
Publisher Registry:

- `/.well-known/actr` publishes the authority descriptor;
- the Publisher Registry manages Publisher identities and signing keys;
- `loran-packages` serves package bytes.

The default Loran daemon mounts the component at `/actr/packages`. Local files
are listed under `[services.packages]` as absolute `.actr` paths:

```toml
[services.packages]
artifacts = ["/srv/actr/packages/echo-ui-1.0.0.actr"]
```

Each file is served at `/actr/packages/artifact?sha256=<digest>`. The digest is
computed at startup; a file whose bytes change afterwards is no longer served
under the old digest. Consumers still verify the SHA-256 given by the Realm
Profile. Package hosting can equally move to another Loran node, an object
store, or a CDN without changing the Actor.

See the [Actr Realm Profile specification](../../actr/protocols/realm-profile.md).
