# 03. User Experience

## 1. Information architecture

Actrium's top-level product surface has five parts:

```text
Actrium Shell
├── Tab and address bar
├── Program Catalog (program library)
├── Service Inventory (instance inventory)
├── Realm & Identity (identity and networking)
└── Security Center (permissions, risk, revocation and audit)
```

### 1.1 Program Catalog

Answers "What can I run?":

- Installed programs;
- Marketplace favorites or recommendations;
- Direct Publisher sources;
- Local imports and development programs;
- Each program's known version, digest, source and update status.

### 1.2 Service Inventory

Answers "What have I already created?":

- Instance address (Realm link) and Realm name;
- Program and exact version;
- Workspace display name;
- Local/Remote run location;
- Four independent status groups: lifecycle, reachability, health and session occupancy;
- Last-opened time and current Tab;
- Actions: start, stop, open, export link, migrate, archive, move to trash and purge.

Program and Service must be kept separate in the UI and the data model. Installing a program does not mean an instance has been created; deleting a program's cache does not mean deleting the Workspace.

## 2. Address bar

The address bar shows the instance address (Realm link, see [04](04-uri-and-resolution.md)) and answers three questions:

| Information | Source | User question |
|---|---|---|
| Realm name | The address itself (`actr://<Realm domain>/<realm_id>`) | Which instance is this? Who is the trust root? |
| Program (Publisher, program name, version) | The manifest of the package the Realm Profile points to | Which Publisher published which version this instance runs? |
| Page path | The part of the address after the RealmId | Which page of the app am I on? |

### 2.1 Security indicators

The security control on the left of the address bar must show:

- Current application assurance: green / yellow / red, or re-validating;
- Publisher domain and verified/development/revoked status;
- Short fingerprint of the current package digest;
- Realm name and its Realm domain; when the Realm domain equals or belongs to the Actr Facility Provider ID, state that the trust root is that Actr Facility Provider, otherwise the trust root is the Realm domain;
- Service connection status: local, P2P, TURN relayed, offline;
- Whether the current permissions include a high-risk grant.

A single, ambiguous "lock" icon must not conflate TLS, package security, Actor identity and permissions.

Green can only be shown when the current facet is `Ready` and the application conclusion is `CONTROLLED`. Yellow does not mean a known leak, but it likewise cannot show green. While suspended or when a facet has expired, show "re-validating / communication blocked". Clicking must reveal the current policy, Actor count, workload kinds, declared exports, the weakest item, the trusted verifier and the conclusion's validity period.

### 2.2 Visible difference between creating and opening

- Creating an instance happens on the Shell's new-instance page; it shows "a new instance will be created" and the domain the new Realm will be in; the new-instance page corresponds to no link and cannot be bookmarked or shared.
- Opening a link shows the instance name (`display_name`) and Realm name, with the hint "opening an existing instance".
- Once creation completes, the address bar immediately switches to the new instance's canonical address.
- If opening fails, the address bar keeps the original address.

## 3. First launch

Actrium completes the following setup on first launch:

1. Initializes the Actrium key store in OS secure storage (used to generate each Realm's client key);
2. Randomly generates an `installation-id`; the local Realm uses the pseudo-domain `<installation-id>.actr.alt`;
3. Sets the default location for new instances to "this machine", i.e. new instances use the local Realm;
4. Lets the user later trust a Realm domain, or join another instance's Realm via a Realm invitation;
5. Shows the difference between Program Catalog and Service Inventory;
6. Turns off anonymous telemetry and development mode by default.

First launch creates no Realm; each local instance gets its own local Realm when it is created. The local Realm lets the local Front and Service run offline, but its address can only be resolved within that Actrium installation. Cross-device connection requires first migrating the instance to a new Realm under a Realm domain, which yields a new address.

## 4. Installing programs

### 4.1 From Marketplace

1. The user opens the program's detail page;
2. Actrium shows the Publisher, version, digest, permissions, Workspace mode and marketplace review status;
3. Actrium downloads the `.actr` from the package address given by the Marketplace or Publisher;
4. Verifies HTTPS, the Publisher key, the package signature, the digest, compatibility and archive limits;
5. Records the Program Release;
6. Only caches the program; it does not automatically create a Service unless the user clicks "Create instance".

The download address is not the trust source; Actrium relies on the package signature (verified against the Publisher Profile) and the digest.

### 4.2 When opening a link

If the package is not yet installed, handle it per [04 §3](04-uri-and-resolution.md): confirm first, then download and verify; canceling produces no Service or Front Actor.

### 4.3 Local import

For a local `.actr` package:

- It must still pass structure, hash and signature checks;
- If the signing key does not belong to a verified domain, mark it "local development identity";
- It must not occupy the trusted record of a verified Publisher with the same name;
- It is not automatically synced to other devices;
- The development marker is always shown while it runs.

## 5. Creating an instance

### 5.1 Main flow

```mermaid
sequenceDiagram
    actor U as User
    participant A as Actrium Shell
    participant P as Package verification
    participant M as Service Manager
    participant S as Service
    participant F as Front Actor

    U->>A: Click "Create instance" in Program Catalog (or import a package, or enter a package address)
    A->>P: Download and verify the package
    P-->>A: package digest + manifest
    A->>U: Confirm permissions, Realm domain and RealmId, and Workspace
    U-->>A: Confirm creation
    A->>M: Create(idempotency_key, package, workspace, domain, realm_id, display_name)
    M->>M: Obtain authorization from the Realm signing key holder
    M->>M: Actr Facility Provider creates a pending Realm
    M->>M: Sign the Realm Profile (including package) and submit it for activation; Realm becomes active
    M->>M: Redeem the owner invite and become the Realm's owner (obtain Realm membership)
    M->>S: Persist identity within the new Realm and start the Service
    S-->>M: stable ActorId
    M-->>A: Service record + ActorId
    A->>A: Replace the address bar with the new instance's Realm link
    A->>F: Start the fixed Native Front workload
    F->>S: Attach handshake
    S-->>F: session lease + contract
    F->>F: Load the verified Keel Guest
    A-->>U: Show the program
```

### 5.2 Realm domain and RealmId

Every new instance creates a new Realm. The confirmation page shows:

- Realm domain: creates a local Realm by default; any trusted Realm domain can be chosen. Without a domain, no Realm is allocated (except the local Realm); a user without their own domain uses a Realm that an Actr Facility Provider allocates under its own domain;
- `realm_id`: by default Actrium generates a fully random short string (e.g. `k7m2q9xh`, lowercase base32) containing no program name or user input; a readable name goes into `display_name`, shown to the user, appearing only locally and in the Realm Profile, never in the ActorId; the user may also explicitly choose a readable `realm_id` (e.g. `notes`), and the confirmation page warns that a readable name will be exposed in shared links;
- Creation must be authorized by the Realm signing key holder of that Realm domain: the holder's signing service authenticates the current user under its own policy and signs the Actr management envelope and the Realm Profile; after activation it hands the one-time owner invite back to Actrium, which redeems it at once, so the creator is the Realm's only owner; the confirmation page shows that Realm domain as the trust root (delegated custody is invisible to the user);
- When the Realm domain equals or belongs to the Actr Facility Provider ID, v1 offers no self-service creation: the user contacts that Actr Facility Provider's operator, who creates the Realm and hands the owner invite to the user out of band; the user chooses "Join with an invitation" on the identity page, and after redeeming it Actrium creates the Service and Front in that Realm; the confirmation page states that the trust root is that Actr Facility Provider. Self-service creation with an Actr Facility Provider account is a later capability;
- If authorization is refused or the holder is unreachable, a clear error is shown, with no fallback to another Realm domain.

If the Realm domain is not yet trusted, first-time trust must be completed on the confirmation page.

The RealmId cannot be changed after creation; a RealmId already used within the same Realm domain cannot be reused.

### 5.3 Workspace selection

The manifest must declare three dimensions separately; they cannot be squeezed into one enum:

- requirement: `none`, `optional` or `required`;
- provisioning: `managed` or `user-selected`;
- access: `read-only` or `read-write`.

It may also restrict the allowed Workspace Providers, e.g. `managed-store`, `local-folder`, `cloud-drive` or `remote-repository`. `managed` is a configuration method, not "whether a Workspace is needed"; `read-only` is an access mode, not a lifecycle mode.

The selector shows:

- Workspace Provider;
- Path or logical location;
- Read/write mode;
- Whether another Service already occupies it;
- Backup and sync capabilities;
- How the Workspace is handled when the Service is deleted.

### 5.4 Create idempotency

Every creation must carry a locally persisted `idempotency_key`:

- UI redraws, network retries, back-then-forward, and package download retries must not create duplicates;
- If creation succeeds but the Front fails to start, the Service still enters the Inventory, and the page offers "retry connection";
- Clicking "Create instance" again means a new creation, requiring reconfirmation;
- Bulk creation triggered from an external source in a short time must be subject to quota and rate limits.

Creation proceeds in the order "holder authorization → pending Realm → sign the Realm Profile and submit it for activation → Realm active → create Service":

- If any step before the Realm becomes active fails, no Service is created and no half-finished instance appears in the Inventory; the page states which step failed (authorization refused, Actr Facility Provider unreachable, Realm Profile publication failed) and offers a retry;
- The default random RealmId is generated once when creation begins and persisted with it; a retry never regenerates it;
- A retry reuses the same `idempotency_key`, continuing the same pending Realm and the same RealmId, without creating a duplicate; if the Realm was already activated but the response was lost, the retry only returns its state, and the owner invite is reissued with `reissue_owner_invite`;
- If the user abandons it, the Actr Facility Provider automatically cleans up the pending Realm on expiry.

## 6. Connecting to an existing instance

### 6.1 Main flow

1. Resolve the Realm link to get the Realm name;
2. Fetch that domain's Realm signing key and Realm Profile and verify the signature;
3. Per the Realm Profile's `package`, ensure the package is installed and the digest matches;
4. Via the Actr Facility Provider specified by the Realm Profile, find within that Realm the Service whose Actor type matches the package;
5. Complete mutual identity and challenge verification;
6. Negotiate the Front–Service contract;
7. Obtain the exclusive lease;
8. Load the Keel Guest.

### 6.2 Reopening locally

If an active Tab already has this instance open:

- Focus the existing Tab by default;
- When the user explicitly chooses "open in new Tab", v1 warns that the instance is an exclusive session;
- No second Service is created;
- It is not replaced by a Service of the same type.

### 6.3 Occupied remotely

The page shows:

- The public name of the device currently occupying the session;
- The lease's expected expiry;
- "Wait", "Request takeover", "Cancel" actions;
- Takeover must be decided by Service policy and the current user's permissions.

## 7. Tab behavior

There are two kinds of Tab in the Actrium UI:

- **Shell Navigation Tab**: installation, creation confirmation, error detail, Program and Service management pages; it does not represent a running application, so it can have no Front Actor.
- **Resident App Tab**: an application page that is loading, running or suspended; at any moment there must be exactly one live Front Actor, and once the Front has completed Attach, exactly one Service.

A page evicted under memory pressure enters the dormant state, in which it may temporarily have no Front; reactivating it must rebuild the Front and Attach to the original Service. The product copy "one Actor per Tab" refers specifically to resident app Tabs, not to every Shell page or dormant record on disk.

### 7.1 Creating

- Creating an instance: opens the Shell's new-instance page and follows the flow in section 5.
- Opening an instance link: connects to that instance.
- Clicking "Open" from Service Inventory: uses that instance's address.

### 7.2 Refresh

Refresh only rebuilds the Front/Keel Guest and reconnects to the same Service:

- Does not create a Service;
- Does not rebind the Workspace;
- Does not upgrade the version;
- The Guest's temporary UI state may be lost; business state must not be lost.

Refreshing the new-instance page continues the same `idempotency_key` or returns to the confirmation page.

### 7.3 Closing

Closing a Tab:

1. Revokes the Front session capability;
2. Sends `Detach`;
3. Destroys the Keel Engine;
4. Destroys the Front Actor;
5. Keeps the Service, Workspace and instance record.

Closing is not stopping, still less deleting.

### 7.4 Background and recovery

- A backgrounded Tab calls `Engine.pause()`, freezing the Guest's timers and event delivery;
- Whether the Service keeps running is decided by instance policy and platform capability;
- On resume, calls `resume()` and sends lifecycle events;
- Under memory pressure the Front may be destroyed and rebuilt and re-Attached when brought back to the foreground;
- The Guest must not assume the JS heap persists forever.

### 7.5 Crash

The Front crash page offers:

- Reload the Front;
- View the error summary;
- Temporarily disable newly granted permissions;
- Open instance management;
- Report the program.

A Guest crash must not automatically restart the Service. A Service crash shows a separate state and allows restarting the same ActorId per policy.

## 8. History, favorites and sharing

### 8.1 Favorites

Everything favorited in the address bar is an instance; opening it returns to the same instance. To favorite a program for frequent use, favorite the program in Program Catalog and create instances from there; the two pieces of copy must not be conflated.

### 8.2 History

- Only the instance's canonical address is recorded after a successful creation;
- The creation confirmation page may be recorded as a non-persistent navigation event, not added to synced history;
- The instance address is not a secret, but history-sync settings should let the user exclude instance addresses;
- The URL must never record a credential, Workspace path or Realm invitation.

### 8.3 Sharing

Sharing an instance shares only its canonical `actr://` address; it does not automatically grant access.

- The recipient still needs to join that instance's Realm;
- Application permissions must allow it;
- The invitation is a one-time Realm invitation issued by the owner (see Section 15);
- The bearer token never enters the URI's query or fragment;
- The instance address exposes the Realm domain and RealmId; warn about this before sharing a private instance, with an extra warning if the instance uses a readable RealmId.

Local Realm instances cannot be shared; "copy link" is disabled. To share, the instance must first be migrated to a new Realm under a Realm domain, yielding a new address.

## 9. Program Catalog experience

Each program shows:

- Publisher domain;
- Program name and description;
- Installed version;
- package digest;
- Marketplace source and review marker;
- Permission summary;
- Workspace mode;
- Number of instances created;
- Update, revocation and compatibility status;
- "Create instance", "View instances", "Remove cache".

If a Service still uses that digest, "Remove cache" should either be refused or retain the copy needed to keep it running.

Program Catalog can only show Release assurance (signature, digest, revocation, review); it must not claim in advance that some future running instance or the whole application graph will be green.

## 10. Service Inventory experience

Each instance shows:

- User-given name;
- Realm name and a "copy link" entry;
- Program Release;
- Workspace;
- Local/Remote;
- Running status;
- Current session;
- Permissions;
- Most recent backup/snapshot;
- Created and last-opened time.

Service Inventory may show the last observed application conclusion and its time, but it must be marked as a historical snapshot. The address bar shows a live color only once the current facet is `Ready`.

Action semantics:

| Action | Service | Workspace |
|---|---|---|
| Open | Attach | Unchanged |
| Stop | Offline but identity kept | Unchanged |
| Start | Restore with the same ActorId | Unchanged |
| Archive | Stop and hide from the default list | Unchanged |
| Move to trash | Stop and hide, keep identity and instance state | Unchanged |
| Restore | Return to stopped with the same ActorId | Unchanged |
| Purge instance | Revoke identity, delete key/store, keep a local instance tombstone | Kept by default |
| Delete Workspace | Does not implicitly delete the Service | High-risk reconfirmation |
| Upgrade | Create a new Service and migrate | Explicit handoff |

## 11. Error experience

Error pages must distinguish:

| Error | User meaning | Primary action |
|---|---|---|
| Package unavailable | The package the Realm Profile points to failed to download, or its digest/signature does not match | Retry; contact the instance admin |
| Publisher untrusted | The Publisher domain or key is not trusted | View certificate/development mode |
| Digest conflict | Different content appears for the same version | Block running, view security report |
| Package revoked | The version has been revoked | View reason, run isolated or delete |
| Domain untrusted | The instance's Realm domain is not yet trusted | View the Realm domain and key fingerprint, confirm trust |
| Domain authorization failed | The Realm signing key holder refused authorization or was unreachable, so the instance cannot be created under that Realm domain | Contact that domain's admin, or use the local Realm or another Realm domain |
| Realm not joined | The Realm domain is trusted, but the instance's Realm has not been joined | Join with a Realm invitation, or request one |
| Invite expired / used / revoked | The Realm invitation has expired, has been used or has been revoked (each shown separately) | Ask the owner for a new invitation |
| Invite realm mismatch | The invitation belongs to another instance's Realm | Check which instance to join and use its invitation |
| Client revoked / Removed from Realm | This client has been revoked, or this member has been removed from the instance's Realm | Ask the owner for a new invitation; local data is kept |
| Realm retired | This instance's Realm has been retired | View archive information; do not create a replacement instance |
| Service offline | The exact instance is not online | Retry, view the instance; offer start only when locally known to be stopped |
| Permission denied | Found, but not authorized to Attach | Request authorization |
| Session occupied | The exclusive lease is occupied | Wait or request takeover |
| Identity mismatch | Actor identity or type does not match | Hard block |
| Contract mismatch | Front/Service protocol is incompatible | Install the correct version or migrate |
| Workspace unavailable | Workspace Provider/credential/lock error | Fix the Workspace, do not create a new Service |
| Guest crashed | Front-end code failed | Reload the Front |
| Service crashed | Background instance failed | Restart the same ActorId, view logs |
| Assurance yellow | A necessary fact could not be confirmed | View workload type/declaration, restrict protected data |
| Assurance red | Confirmed uncontrolled export or Retain | Block the data path, view the weakest item |
| Graph incomplete | Cannot prove the rootless graph was fully discovered | Disable the protected capability, or migrate to a supported Actr Host |

The error page must not hide security differences behind a generic "could not connect".

## 12. Developer experience

Developer mode provides:

- Import a local `.actr`;
- Use a package address on localhost or a private CA (marked as a development source);
- View the manifest, digest, contract and capabilities;
- Hot-reload the Guest, but each change uses a new development digest;
- Start a local Realm and Service;
- Inspect Front–Service RPC;
- View Keel render ops, CPU, heap, timers and capability calls;
- Simulate permission denial, offline, TURN, Service crash and Front rebuild.

Developer mode does not change the trust rules for production addresses and production Publishers.

## 13. Accessibility and internationalization

- No security state may rely on color alone;
- ActorId, digest and Realm domain support copying and a short screen-reader-readable description;
- IDNA domain names provide both Unicode display and ASCII A-label inspection;
- Permission copy describes user actions, not underlying API names;
- Keyboard navigation, dynamic font size, high contrast and Reduce Motion are MUST;
- Error codes are stable; display text is localizable.

## 14. Realm, device and identity experience

A Realm corresponds to a single instance, so the identity page is organized in two layers: trusted Realm domains, and joined Realms (instances). It supports at least:

- Trusting a Realm domain: on first encountering a Realm domain, fetch that domain's Realm signing key over HTTPS from its authority descriptor (`https://<domain>/.well-known/actr`), show the domain and key fingerprint, and pin it after confirmation by the user or organization policy;
- Joining a Realm: paste or scan a one-time Realm invitation to join an instance's Realm, completing client registration and obtaining Realm membership; Actrium performs each Realm's client registration automatically, without requiring the user to configure it per Realm; each Realm uses a separate Realm client key, never reused across Realms, so different instances cannot correlate the same client through it;
- Re-authentication: when a credential expires or the admin requires it, pause new Attach attempts, keep local data and guide the user to recover;
- Adding a client: on a joined client, issue an "add a client" invitation for yourself and redeem it on the new client;
- Client revocation: list clients, last activity time and hosted Services; a member can revoke its own clients and the owner can revoke any member's clients; after revocation, that client's Realm client key registration becomes invalid, it can no longer obtain Actor credentials, and its Actors are disconnected from signaling and TURN at once; revoking the client that hosts the Service takes the instance offline, so warn first;
- Member management (owner only): list members and roles, issue and revoke invitations, change roles, remove members (which also revokes all their clients), and transfer ownership to another member;
- Leaving a Realm: means losing access to that instance, and all of this member's clients are revoked with it; first explain the affected Services, Workspaces and sharing relationships, and never silently migrate to the local Realm; the owner must transfer ownership first;
- Realm signing key change: show the signed rotation chain; without a valid rotation chain, treat it as an identity change and reconfirm trust;
- Recovery: after a device is lost, only encrypted backups or data on a remote host can be recovered; there is no promise to recover an Actor private key that was never backed up; a member revokes the lost client from another client, or requests an "add a client" invitation from the owner; when the owner has lost all its clients, it requests a new owner invite from the Realm signing key holder or the Actr Facility Provider's operator, and redeeming it revokes all previous clients.

Trusting a Realm domain does not join any Realm within it; joining a Realm only establishes a Realm client identity — actual Attach is still governed by application permissions. Realm-domain-level SSO/directory is a later capability; v1 does not include it.

## 15. Sharing, invitations and application permissions

The minimum application-permission roles for a Service are:

| Role | Capability |
|---|---|
| owner | Manage application permissions, migration, trash and purge |
| editor | Attach and modify business state per the contract |
| viewer | Read-only Attach; availability depends on the program's contract |

Each instance has exactly one owner, its creator; the owner can transfer ownership to another member (the previous owner becomes an editor by default), and v1 does not support several owners. The Actr Facility Provider keeps each member's role with the member record and writes it into the Actor credential; the Service reads it from the Front's credential and enforces it.

Sharing flow:

1. The owner picks a role (editor or viewer) and an expiry;
2. That instance Realm's Issuer generates a one-time Realm invitation (text `ri_…`) carrying the Realm name, optionally embedding the Realm Profile;
3. The invitation travels over an independent secure channel, never written into the `actr://` URI;
4. The recipient trusts the domain (if not yet trusted) and redeems the invitation to join the Realm as a member with that role;
5. The invitation is immediately invalidated; the owner can revoke an unused invitation before it is redeemed;
6. The owner can change a member's role or remove the member: removal immediately stops that member from obtaining Actor credentials and connecting to signaling and TURN, and the Service ends already established sessions at the latest when their credential expires; a role change takes effect in the member's next credential.

Merely copying the instance address does not change application permissions. Expired, already-used, revoked and Realm-mismatched invitations must produce distinct errors.

## 16. When permissions are granted

- The install page shows only the capabilities the package requests; it grants no running permission;
- At creation time, Service, Workspace and long-running background capabilities are granted and written as a persistent grant;
- The Front's interactive capabilities — camera, microphone, clipboard, etc. — are requested just-in-time on first use, with an optional "this session only";
- Admin policy can pre-deny or constrain scope, but must not disguise this as user consent in the UI;
- After a persistent grant is revoked, the Actr Host must immediately invalidate the related capability handles and end any calls still using them;
- A remote Service's host-level permissions are managed by the remote host; the local Actrium can only show the result of a signed declaration, and cannot modify another host's OS permissions on the user's behalf.

## 17. Remote Service origin and management boundary

In v1, creating an instance only happens on a local Service Host that the current Actrium supports. A remote Service enters the Inventory through:

- Opening an existing instance's link;
- Accepting a Service invitation;
- Syncing an instance address from the same user's encrypted Inventory;
- Explicitly importing signed Service metadata.

Actrium can remotely start, stop, migrate or purge a Service only when the remote host has separately granted it a management capability; otherwise it can only Attach, request authorization and view verifiable state. On iOS/Android, "long-running" usually means connecting to a desktop, server or other remote host — it does not mean the mobile client can arbitrarily create cloud-side processes. General-purpose server hosts and cloud scheduling are outside the scope of the Actrium v1 client.

## 18. Guest navigation semantics

The Guest cannot rewrite the Shell's address bar directly; it can only call `host:navigate`:

- Route/query changes within the same instance use `push` or `replace`;
- Navigating to another instance's link opens that instance; it cannot carry the current capability along;
- The Guest cannot create a new instance via a link;
- `https://` external links are handed to the system browser or a controlled WebView, showing the target domain;
- Route and query are subject to length, character and sensitive-field policy;
- Back/forward replays the canonical address, not application-permission tokens or RPC.

## 19. Publisher development and publishing path

The official toolchain should provide a repeatable flow:

```text
actrium init
→ write the Keel Guest, Service Component and contract
→ build
→ validate (schema, capability, hash, sandbox compatibility)
→ sign
→ generate/update the Publisher Profile
→ upload the package
→ read it back from the package address and check the digest and signature
→ optionally submit for Marketplace review
```

Publisher key creation, rotation, revocation and offline root backup must have clear commands and recovery instructions. Development hot-reload serves development sources only; it cannot overwrite an already-published exact version.
