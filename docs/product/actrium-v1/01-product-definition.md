# 01. Product definition

## 1. Product vision

Actrium turns Actor programs into a product that ordinary users can understand, install, create, open and restore.

Users see a familiar browser:

- an address bar, tabs, history, favorites, a program library and security prompts;
- an address can create a new program instance or return to an existing one;
- a program can run locally or connect to a remote one;
- closing a tab does not accidentally lose working state;
- installing a program does not mean allowing it to run arbitrary native code.

Developers see a constrained small-app platform:

- the front end is written only as a Keel Guest;
- there is no need to implement windows, routing, Actor registration, signaling, permission dialogs or a cross-platform Actr Host;
- the front end always reaches business logic through the instance's Service, via typed capabilities;
- the background Service can use the Actor model to compose more complex services;
- an exact-version package can be downloaded over HTTPS, verified, cached and reproduced.

## 2. Current issues

Without first freezing the model, Actrium can easily end up carrying the semantics of a browser, a plugin system, a remote desktop, a native application container and an Actor debugger all at once, which eventually leads to the following problems:

- the lifecycles of Tab, page, plugin, Actor, Service and Workspace overlap with each other;
- front-end code can go through the Keel Bridge, but might also bypass it and access native modules or the Actor network directly;
- "opening a program" sometimes creates an instance, sometimes discovers a service, and sometimes connects to a fixed service, and the address bar cannot explain which;
- a stateful Service gets treated as a stateless replica that can be load-balanced by type;
- Marketplace, Publisher, Realm and signaling get merged into one centralized source of trust;
- a package downloaded over HTTPS lacks version immutability, a fixed digest and offline verification;
- closing a tab, refreshing, or crashing can wrongly create, replace or clear a Service.

The purpose of this PRD is to remove these ambiguities with a small number of stable invariants.

## 3. Target users

### 3.1 End users

Users who need to run lightweight agents, small programs or Workspace tools, but don't want to understand Actor, Realm, signaling or runtime details.

Core tasks:

- install a program from the marketplace or locally;
- choose a Workspace and create an instance;
- later resume work from history, favorites or the instance library;
- connect to an existing instance from another device;
- clearly know who the program comes from, which Realm (and domain) the instance belongs to, and which permissions have been granted.

### 3.2 Program developers

Develop a program with a native UI, an Actor backend and Workspace state.

Core tasks:

- write a cross-platform front end using only the Keel Guest API;
- describe the Front–Service API with Protobuf/contracts;
- build the Service workload;
- declare components, permissions, resource quotas and the Workspace mode;
- publish an immutable `.actr` package under their own domain;
- debug quickly in local development mode.

### 3.3 Publisher

Publishes programs through a domain name and maintains the Publisher signing key, versions and revocation information. Realms are created per instance, so a Publisher cannot designate a particular Realm in advance.

### 3.4 Realm signing key holder and Realm owner

The owner of a Realm domain is the Realm signing key holder: it publishes the Realm signing key in the authority descriptor, authorizes Realm creation under that domain after authenticating requesters under its own policy, decides which Actr Facility Provider to use, and is responsible for rotating the Realm signing key; it may also delegate custody of the key to an Actr Facility Provider. No domain, no Realm (except the local Realm).

The Realm owner is the creator of the application instance and manages that Realm's membership, Actor admission, connection ACL, Realm invitations, client revocation and audit.

### 3.5 Marketplace operator

Provides program discovery, categorization, review, risk labeling and revocation information, but does not own Publisher naming rights.

## 4. Core user stories

### 4.1 Create an instance

> As a user, I pick a program in the program library and click "Create instance". Actrium should first verify the program and show its permissions, then let me choose a Workspace and a Realm domain, create a new Realm under the chosen domain (a local Realm by default) with a Service inside it, and then show the new instance's address in the address bar.

### 4.2 Connect to an existing instance

> As a user, when I open a link to an instance, Actrium can only connect to that instance; if it is offline or I don't have access, it must report a clear error, not create another instance that "looks the same".

### 4.3 Close and restore

> As a user, closing a Tab only ends the current front-end session. The Service and Workspace continue to exist unless I explicitly stop, archive, move to trash, or permanently delete them in instance management.

### 4.4 Install and verify

> As a user, when I install `publisher/program@version`, I can confirm the Publisher domain, exact version, Service and Front package digests, permissions, and Marketplace risk status. Each installed release stays pinned to those two archives.

### 4.5 Local development

> As a developer, I can load an unpublished program from paired local `.actr` files or a development HTTPS origin. Actrium labels it a development source and uses a development identity.

### 4.6 Realm and network

> As a user, I don't need to manually fill in signaling and TURN addresses; Actrium selects endpoints and short-lived credentials based on a Realm Profile signed by a trusted Realm signing key, while showing me the current instance's Realm, its domain and connection status.

### 4.7 Multi-Actor privacy state

> As a user, when a program is composed of multiple Actors, the green I see must mean that on the current ready cut, the egress of the whole graph is controlled; if any node is unknown it is yellow, if any node is confirmed uncontrolled it is red; when the cut is not ready, green must not be shown.

## 5. Product goals

### G-01: Form one unified application model

Every ordinary application Tab follows the same main chain:

```text
Tab → Front Actor → Service → Workspace
```

UI-only programs also use the minimal Service the platform provides, instead of inventing a second "pure page app" model.

### G-02: Confine Publisher code to an auditable boundary

- A Publisher may not replace the Front Native Host.
- The Front may only carry a Keel Guest.
- By default, the Guest has no arbitrary network, file, Actor discovery, or native-module capability.
- Every capability is the intersection of what is declared, what the user authorizes, and what the Actr Host implements.

### G-03: Make addresses readable, copyable and recoverable

- The Realm link `actr://<Realm domain>/<realm_id>` answers "which instance", i.e. where to verify and address it;
- the Realm Profile pins the Service package, and the matching signed Program Release record pins the Front package.
- the address carries no secrets.

### G-04: Reuse the mature security infrastructure of the Web

Reuse DNS, IDNA, TLS, HTTPS, certificate transparency, caching, CDNs, standard ports and enterprise proxies; supplement HTTPS with package signing and digests to cover what HTTPS lacks for offline caching, mirroring and version immutability.

### G-05: Preserve the Actor model's compositional power

The Front only sees the instance's Service; that doesn't mean the background can only have one Actor. The Service can orchestrate other Actors within the same Realm — workers, media, indexing, sync and so on.

### G-06: Provide end-to-end application assurance

- Conclusions are aggregated along the unrooted effective-dependency graph; the Front only locates it;
- the Actr Host cross-verifies WASM's declarations, imports, effective capabilities and actual egress;
- green is shown only when the state is `Ready` and every node is `CONTROLLED`;
- yellow and red are clearly distinguished; a suspended assurance must never show green;
- a new configuration applies only to new data; a state change propagates to the channel, the Actor Grant and the data plane.

## 6. Non-goals

v1 does not do:

- an arbitrary web-browser compatibility layer or a complete Web API;
- Publisher-supplied custom Front native plugins, dynamic libraries, or Host Module implementations;
- letting the Guest directly access an arbitrary ActorId or discover any Actor by type;
- transparent load balancing across Service instances;
- a multi-party real-time collaboration protocol;
- in-place, unnoticed replacement of a running Service workload;
- automatic cross-Realm (cross-instance) federation or global Actor search;
- domain-level SSO or a member directory;
- open anonymous TURN;
- granting system permissions on Marketplace review labels alone;
- promising that a mobile platform will keep a local background Service online 24×7.

## 7. Product principles

### 7.1 Explicit over guessed

Opening a link only connects; creating is only ever an explicit action in the program library; a failure never silently changes the intent.

### 7.2 An address is not a permission

An address is for locating and identifying; access is decided by Realm membership, application permissions and session authorization.

### 7.3 State belongs to the Service

The Tab, the Front Actor and the Guest can all be rebuilt; the Workspace binding, business state and instance permissions belong to the Service.

### 7.4 An exact version is immutable

Once Actrium accepts `publisher/program@version`, the Service and Front package digests are fixed for that release.

### 7.5 Minimal capability by default

Any capability that is undeclared, unauthorized, or not implemented by the Actr Host is unavailable.

### 7.6 Platform limits are shown explicitly

Facts such as mobile background limits, same-process sandbox strength, and stale offline revocation information must be shown, not hidden behind abstract terms.

### 7.7 Privacy depends on the weakest link, not the average

A controlled node cannot offset another unknown or uncontrolled node; lacking evidence cannot be treated as passing. Green only proves that egress may be controlled on the current ready cut; it does not claim the program is "absolutely free of vulnerabilities", nor does it imply there was ever a single moment when the whole graph existed together.

## 8. v1 scope

### 8.1 Must deliver

- a complete closed loop on macOS/Windows desktop;
- a foreground run-and-restore closed loop on iOS/Android;
- opening and sharing `actr://` Realm links; creating instances from the program library;
- the Program Catalog and the Service Inventory;
- domain-based Publishers, HTTPS download, signature and digest verification;
- a fixed Native Front Host + Keel JS Guest;
- local Service create, stop, start, connect, trash and permanent deletion;
- Workspace binding;
- the Realm signing key, per-instance Realms and their Realm Profiles, identity, signaling and short-lived TURN credentials;
- `ActorAssurance` / `ApplicationAssurance`, the unrooted dependency graph, weakest-link aggregation and `Ready` display;
- permissions, quotas, audit and error pages;
- local development sources.

### 8.2 Can be deferred

- Marketplace transaction capability;
- cross-device Service migration;
- shared attachment;
- multi-Realm federation;
- forcing an independent OS process for the Front;
- the Keel Native WASM Guest;
- a version transparency log and reproducible-build proofs.

## 9. Definition of success

v1's mark of success is not "can show a Keel page" — success means the following closed loop holds all at once:

1. The user installs a signature-verified program.
2. The user creates a Workspace-bound Service.
3. The address bar shows the new instance's address in place.
4. The Front can only call that Service through the declared contract.
5. After closing and restarting Actrium, the same address restores the same Service identity.
6. The same exact version can still be verified when run offline.
7. Service offline, no permission, Realm unreachable, and package incompatibility each produce a distinct, actionable error.
8. A Guest overstepping its bounds, timing out, exceeding memory, or crashing does not corrupt the business state of other Tabs.
9. When any graph member becomes unknown, uncontrolled, or its assurance is suspended, green must stop being shown and the corresponding data flow must be restricted.
