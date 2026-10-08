# SRV-002 report: MLS for CRDT updates, natively and in WebAssembly

- **Work package:** [SRV-002](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/SRV-002-spike-mls.md), the validation gate of [ADR-0016](https://github.com/BayanDocs/docs/blob/HEAD/adr/0016-e2ee-and-identity.md) (including its amendment of 2026-10-04).
- **Date:** 2026-10-07. **Code:** this folder, a Cargo workspace of its own (see [`../README.md`](../README.md) for why).
- **Verdict:** ADR-0016's design works as planned with OpenMLS 0.9.0, natively and in WebAssembly, and group-per-document holds up to 1,000 members, with costs and six implementation requirements that the ADR should record (§11); one of them is a design limitation (any member can stall a group) that SRV-003 and SRV-103 must resolve. The hybrid post-quantum suite the owner chose for launch already runs in OpenMLS under a provisional code point.

## 1. Summary in plain language

BayanDocs encrypts documents on users' devices with MLS (Messaging Layer Security, RFC 9420). Each document has its own MLS *group*: the set of devices that hold the document's keys. Every change to who is in the group is a *commit*, which moves the group to a new *epoch* with fresh keys; document edits travel as encrypted *application messages*. The server relays all of this without being able to read it.

This spike built that design end to end with the OpenMLS library and checked it on three platforms: native code (as the desktop app and server would run it), and WebAssembly in Node.js and in headless Chromium (as the web app would run it). The same 53 tests pass in all three. They cover:

- creating groups, publishing and fetching *key packages* (the one-time public keys that let someone add a device), adding, removing and re-keying members, joining from a *Welcome* message, and exchanging encrypted updates of 100 B to 100 KB (AC-1);
- a server component, the *validator*, that follows every group's public state from commits without any key, and refuses membership changes and updates that the sender's role does not allow; and clients that check every decrypted update's sender role themselves, so a misbehaving server cannot grant anyone more than their role (AC-2);
- encrypted *snapshots* that let a newcomer read the current document even though MLS hides everything sent before they joined (AC-4);
- moving a document to a new group with another ciphersuite, the planned way to upgrade cryptography;
- saving a device's MLS state and loading it again.

It measured every operation at 2, 10, 100 and 1,000 members, with the classical suite and with the hybrid post-quantum suite (AC-3). Up to 100 members everything is small and fast. At 1,000 members it still works, but some costs grow large, especially with the post-quantum suite: right after many members are added at once, a single member's first commit can reach 1.1 MiB, a device keeps about 9.5 MiB of state per document, and every update costs about 25 ms in the browser because of how OpenMLS stores old epochs. §5 gives the judgment and how to keep these costs down.

mls-rs, the fallback library, runs the same flows natively and in both WebAssembly hosts. Its post-quantum suites, however, need a native C library, so it is not a drop-in replacement for the post-quantum plan (§9).

The spike also found six things the production crate (`bayan-mls` in bayan-core) and the server must handle that ADR-0016 does not mention: joining must be a storage transaction, OpenMLS's stored state can change format between versions, the number of past epochs a device keeps must stay small, OpenMLS has no standard re-initialization yet, its storage format is bulky, and any member can stall a group with a commit that only the other members can check, which the server then follows alone (§5, AC-2, and §10). The docs pull request proposes these, and the design details settled here, as an amendment to ADR-0016 (§11).

## 2. Terms used in this report

| Term | Meaning |
|---|---|
| Group, member, device | One MLS group per document. Members are devices; a user can have several. Roles belong to users. |
| Epoch | A version of the group's keys. Every commit starts a new epoch. |
| Commit | A signed message that changes the group (adds, removals, roster changes) or just renews the committer's keys (a *self-update*, which gives post-compromise security). |
| Key package | A device's signed, one-time public key, published to the server so others can add the device. |
| Welcome | The message that lets added devices join: the group's secrets, encrypted to their key packages. |
| Ratchet tree | The group's public state: a binary tree of the members' public keys. The server keeps a copy. |
| Public and private framing | Commits travel in public framing (signed, readable by the server); updates travel in private framing (encrypted; even the sender is hidden from the server). |
| Ciphersuite | The algorithms a group uses. `0x0001` is the classical suite; `0x004F` is OpenMLS's provisional code point for the hybrid post-quantum suite (ML-KEM-768 + X25519). |
| Bulk-built and healed trees | After one commit adds many members at once, most inner nodes of the tree are blank, and each member's first commit must encrypt to many members (the worst case). Once every member has committed, the tree is *healed* and a commit encrypts to about log₂(n) nodes. |

## 3. How to reproduce

Install the pinned tools once (`scripts/dev-setup.sh`; in BayanDocs cloud sessions the setup script can run it), then from the repository root:

```sh
cargo xtask verify                                 # includes format, Clippy, all native tests, docs and cargo deny for this workspace
cargo xtask mls-spike-wasm                         # all tests in WebAssembly, in Node.js and in headless Chromium
cargo xtask mls-spike-bench native                 # measurements, native (all cores; prefix RAYON_NUM_THREADS=1 for one core)
cargo xtask mls-spike-bench node                   # measurements in Node.js
cargo xtask mls-spike-bench chromium               # measurements in headless Chromium, in a dedicated worker
cargo xtask mls-spike-wasm-size                    # size of the WebAssembly a web client would download
```

The measurements in this report were taken on 2026-10-07 in a BayanDocs cloud session: an Intel Xeon at 2.10 GHz with 4 cores, Linux x86-64, Rust 1.99.0, Node.js 24.21.0, and Chrome for Testing 153.0.8010.12 (the headless shell, driven by chromedriver 153.0.8010.12). In Chromium the measurements run in a dedicated Web Worker, where the web app will run the engine and its MLS client (bayan-web never calls the engine from the page's main thread); on the main thread, a computation that blocks the page for more than 30 seconds makes chromedriver lose contact with the page. Each time is the median of its repetitions: creating key packages once per member; the bulk add, adding one user and removing one user once each (processing them: by two or three members); joining by two newcomers after the bulk add; self-updates five times; updates 50 times (10 times for 100 KB). There was one run per environment, so the times show orders of magnitude and trends, not precise values. The tables were measured before the fixes from this work package's review, which add a strict check of the group context to every commit: it costs about 0.16 ms per check at 1,000 users natively (the validator checks twice per commit, a client once), and a native re-run after the fixes gave identical message sizes and times within the usual run-to-run variation of about ±20%. Sizes do not depend on the platform: every message has exactly the same size in all four runs, and device state differs by less than 1%, because OpenMLS's storage writes random key bytes as decimal numbers of varying length. WebAssembly runs on one thread; natively, OpenMLS spreads parts of a commit over all cores (the appendix also has a native run on one core).

## 4. What was built

| Part | File | What it does |
|---|---|---|
| Devices | `bayan-mls-spike/src/identity.rs` | A device is `user/device`, written into its MLS basic credential. |
| Role roster | `src/roster.rs` | Owner, editor, commenter, viewer; the roster maps each user to a role and lives in a private-use group-context extension (type `0xF0BD`), which every member's capabilities must list. |
| Updates | `src/update.rs` | The plaintext of an application message: its kind (content, comment, snapshot key, group moved) and body. |
| Policy | `src/policy.rs` | The role rules, one copy used by both the server's validator and every client. |
| Client | `src/client.rs` | One device: key packages, groups, commits, joins, sending and receiving, all checked against the policy. |
| Server | `src/directory.rs`, `src/validator.rs` | The key package directory and the validator, built on OpenMLS's `PublicGroup`. |
| Snapshots | `src/snapshot.rs` | Encrypted snapshots for newcomers. |
| Re-creation | `src/recreate.rs` | Moving a document to a new group (ciphersuite migration). |
| Persistence | `src/persist.rs` | Saving and loading a device's state; refuses provisional suites. |
| Simulation | `src/sim.rs` | A whole deployment in one process (server, clients, fan-out), used by the tests. |
| Measurements | `src/bench.rs`, `src/bin/mls-bench.rs`, `tests/bench.rs` | The numbers of §5, natively and in WebAssembly. |
| Web exports | `src/wasm_api.rs` | What a web client would call, for the size measurement. |
| Fallback check | `bayan-mls-rs-check/` | The same flows with mls-rs. |

The transport is simulated: messages are function calls, and the server learns the sending device as an authenticated connection would tell it.

## 5. Results

### AC-1: every flow works natively and in WebAssembly

| Tests | Native | Node.js | Chromium |
|---|---|---|---|
| `bayan-mls-spike` unit tests (identity, roster and group context, policy, update and snapshot formats, hostile bytes, benchmark smoke run) | 19 passed | 19 passed | 19 passed |
| `tests/flows.rs`: key packages; create, add, self-update, remove; updates of 100 B, 1 KB, 10 KB and 100 KB to every member; a user adding and removing their own devices; newcomers joining with the server's tree; late messages; a group of 100 | 7 passed | 7 passed | 7 passed |
| `tests/roles.rs` (AC-2) | 14 passed | 14 passed | 14 passed |
| `tests/snapshots.rs` (AC-4) | 4 passed | 4 passed | 4 passed |
| `tests/recreate.rs`, `tests/persistence.rs`, `tests/provisional_pq.rs` | 8 passed | 8 passed | 8 passed |
| `bayan-mls-rs-check` (§9) | 1 passed | 1 passed | 1 passed |

Natively the tests run in `cargo xtask verify`; in WebAssembly, `cargo xtask mls-spike-wasm` runs exactly the same test functions (written once with `#[wasm_bindgen_test(unsupported = test)]`), and CI runs it in the new `mls-spike` job. The WebAssembly build needs only two settings: OpenMLS's `js` feature and getrandom 0.2's `js` feature, so that randomness comes from the browser's or Node's Web Crypto (`crypto.getRandomValues`).

Design choices the flows confirmed:

- **Commits in public framing, updates in private framing** (OpenMLS's `PURE_PLAINTEXT_WIRE_FORMAT_POLICY` applies to handshake messages only; application messages are always encrypted).
- **Welcomes without the ratchet tree.** The validator tracks every group's public tree anyway, so newcomers fetch it from the server. OpenMLS checks the tree against the hash in the Welcome's signed GroupInfo: a tree changed by one byte, or a tree from another group, is refused (`newcomers_join_with_the_servers_tree_and_reject_a_tampered_one`). This keeps the Welcome for one new member at 1.2 KiB in a 100-member group instead of carrying the 20 KiB tree, and avoids a second copy of the tree in every Welcome.
- **Late updates.** An update sent just before a commit is still readable after it if devices keep one past epoch (`late_message_from_previous_epoch`). The judgment under AC-3 explains why more than one is costly.

### AC-2: roles are enforced by the server and by every client

The roster sits in the group context, so all members agree on it in every epoch: it is covered by the signature on every GroupInfo, bound into the key schedule, and can only change through a commit, which its sender signs. That is what "signed group-context extension" means in this design; no additional signature scheme was needed or built (ADR-0016 §12). The rules (`src/policy.rs`) are:

- Only owners add or remove other users' devices and change roles. Any member may add or remove their own user's devices and renew their own keys.
- Every commit leaves exactly one role for each user with a device in the group, no role for anyone else, and at least one owner. A new user's role is set in the same commit that adds them.
- Only owners may change the group context at all, even with a commit that keeps the roster as it is. After every commit the context must hold exactly two extensions: the roster, and the required-capabilities extension that requires the roster's type and basic credentials (`Roster::from_group_context`). Anything else, such as an *external sender* (a key with which someone outside the group could propose changes, for example adding a member), is refused by the validator and by every client before the commit is applied, and by the validator and by newcomers when a group is registered or joined. The first version of the spike only compared rosters, so any member, even a viewer, could have added an external sender; the review of this work package found it.
- A member's new leaf keeps its identity, and proposal types the design does not use (by reference, pre-shared keys, re-initialization, external joins, custom) are refused.

**The server's validator** (`src/validator.rs`) holds no keys. For each group it builds OpenMLS's `PublicGroup` from the creator's signed GroupInfo, then for every commit checks that it is in public framing and for the current epoch, that the transport-authenticated device is the member that signed it, that OpenMLS accepts it against the public state, and that the policy allows it, before applying it. If the state OpenMLS then holds ever differed from what the policy approved, the validator would quarantine the group and accept nothing more for it, rather than carry on from a state nobody approved (a safety net that no test can trigger). For updates it checks private framing, the epoch (current or previous) and that the authenticated device belongs to a user whose role may send updates. Because the sender inside an update is encrypted, the server can only check the connection, and it cannot tell an edit from a comment: it accepts updates from owners, editors and commenters and refuses viewers.

**Clients** check everything again after decrypting: the update's kind against the sender's role (content needs owner or editor, comments need commenter or above, snapshot keys need editor or above, group moves need owner), every commit against the same policy before merging it, and, when joining, that the group context holds exactly the roster and its requirement, that the roster is consistent and gives them a role, and that whoever added them is an owner or their own other device. A late update, sent in the previous epoch, must have been allowed in both epochs, as the validator also requires: a role granted by the last commit does not cover what was sent before it.

| Test (`tests/roles.rs`) | Shows |
|---|---|
| `validator_rejects_membership_changes_by_non_owners_and_clients_reject_them_too` | Editors, commenters and viewers cannot add or remove members: the validator refuses, and if a misbehaving server relays the commit anyway, every member refuses it and stays in its epoch. |
| `validator_rejects_role_changes_by_non_owners` | Nobody but an owner changes roles, including promoting themselves. |
| `last_owner_cannot_be_removed_or_demoted` | No commit may leave a group without an owner. |
| `validator_rejects_updates_from_viewers_and_non_members` | Viewers cannot send; a non-member's connection cannot submit even a valid member's message; commenters can. |
| `clients_reject_updates_whose_sender_lacks_the_role` | A commenter's edit, a viewer's comment and an editor's "group moved" notice are refused by every client, even when the server relays them. |
| `commit_from_another_devices_connection_is_rejected` | A valid commit submitted over another device's connection is refused. |
| `newcomer_rejects_a_welcome_from_a_non_owner` | A Welcome from an editor's commit, forwarded by a misbehaving server, is refused by the newcomer. |
| `only_owners_change_the_group_context_and_only_to_the_roster_and_its_requirement` | A viewer, a commenter, an editor and the owner each try to keep the roster but add an external sender, and non-owners try to propose the context unchanged: the validator refuses every attempt, every member refuses it when a misbehaving server relays it anyway, and no epoch moves. |
| `a_group_whose_context_holds_more_is_refused_by_the_server_and_by_newcomers` | A group created with an external sender cannot be registered, and a newcomer refuses its Welcome. |
| `a_role_granted_by_a_commit_does_not_cover_updates_sent_before_it` | An edit a commenter wrote before being promoted to editor is refused by every member; the server relays it, because it cannot tell an edit from a comment. |
| `honest_clients_refuse_to_make_changes_their_role_forbids`, `wrong_framing_is_refused`, `hostile_bytes_are_rejected_without_panic` | Clients will not even create forbidden changes; commits must be public and updates private; truncated and bit-flipped messages never panic; the validator refuses every truncation of a commit and the commit with any bit of its signed part flipped, and its epoch does not move. |
| `known_limitation_a_member_can_stall_the_group_with_a_commit_only_members_can_check` | The limitation below. |

**A limitation: any member can stall a group.** The validator checks a commit's signature and everything OpenMLS can verify from the public state, but parts of every commit are keyed with the epoch's secrets, which the server does not have: the *membership tag* and the *confirmation tag* are MACs (message authentication codes), and the path secrets a commit encrypts to the other members can only be checked by decrypting them. So a member can send a commit that the validator accepts and every other member refuses, by corrupting a tag after signing or by encrypting garbage to the others. The validator then moves to the next epoch while the members stay behind, and from then on it refuses every commit they make, because those are for the old epoch, including the owner's removal of the member who caused it. Updates still flow for one more epoch (the validator also accepts the previous epoch's), until a second such commit stops them too. Any role can do this, viewers included, because every member may renew its own keys; `known_limitation_a_member_can_stall_the_group_with_a_commit_only_members_can_check` shows it with a corrupted membership tag. It is denial of service, not a breach: nothing is read or forged. The design has no way out yet: MLS's usual repair, an *external commit* by which a member rejoins at the server's state, is refused by the validator; OpenMLS has no `ReInit`; and an owner's move to a new group (group re-creation, below) requires every current member to come along. So members, not only the server, can split or stall a group. The options, for SRV-003 and SRV-103 to decide (none needs cryptography beyond MLS, ADR-0016 §12):

1. **Two-phase acceptance at the server (recommended).** The server relays a new commit at once but keeps the previous epoch's public state until a device of another user shows, over its own connection, that it has moved to the new epoch (for example with its first message in it). If the other members report that they cannot process the commit, or nobody confirms it within a time limit, the server drops it, returns to the previous epoch and quarantines the committer, whose commits it then refuses until an owner acts. The group recovers by itself, at the cost of one kept copy of the public state per group and a short wait before the next commit. Open details: how to weigh confirmations against refusals (a misbehaving member could also report honest commits as broken), and that two colluding users could still confirm a bad commit.
2. **Resync by external commit.** Allow the one kind of external commit MLS defines for a member that lost its state (RFC 9420 §12.4.3.2), which removes the sender's own old leaf in the same commit, so each stuck member rejoins at the server's epoch. The server only has to check that the commit replaces the sender's own leaf, but every member must rejoin in turn, and the culprit stays in the group until an owner removes it.
3. **An owner's move that may drop users.** Extend group re-creation so that an owner can move the document without some users, which the members check against the owner's announcement. It always works, but it is manual, costs a full re-creation, and leaves the old group's history behind.

Option 1 is the normal defense; option 3 stays as the owner's last resort, and option 2 is better kept for device recovery, where it is needed anyway.

What the server still sees and can do is listed in §11 for SRV-003: who is in each group and with which role (it must, to enforce roles), commit timing, update sizes and timing, and the ability to withhold or reorder messages (denial of service) but not to forge or read them.

### AC-3: measurements at 2, 10, 100 and 1,000 members

Sizes, the same natively and in WebAssembly. "Classical" is `0x0001`; "hybrid" is the provisional post-quantum suite `0x004F`; "–" means not measured (see §12).

| What | Suite | 2 | 10 | 100 | 1,000 |
|---|---|---:|---:|---:|---:|
| Key package | classical | 310 B | 310 B | 311 B | 312 B |
|  | hybrid | 2.6 KiB | 2.6 KiB | 2.6 KiB | 2.6 KiB |
| Bulk add: commit (n − 1 added at once) | classical | 768 B | 3.3 KiB | 31.4 KiB | 312.7 KiB |
|  | hybrid | 5.4 KiB | 29.9 KiB | 269.8 KiB | 2.58 MiB |
| Bulk add: Welcome (one for all newcomers) | classical | 408 B | 1.6 KiB | 15.7 KiB | 157.2 KiB |
|  | hybrid | 1.5 KiB | 11.2 KiB | 121.0 KiB | 1.19 MiB |
| Ratchet tree (fetched by newcomers from the server) | classical | 464 B | 2.3 KiB | 20.6 KiB | 202.7 KiB |
|  | hybrid | 3.9 KiB | 18.5 KiB | 144.4 KiB | 1.34 MiB |
| Self-update commit, bulk-built tree (worst case) | classical | 508 B | 1.2 KiB | 8.5 KiB | 80.6 KiB |
|  | hybrid | 3.9 KiB | 15.5 KiB | 122.0 KiB | 1.13 MiB |
| Self-update commit, healed tree | classical | – | 859 B | 1.2 KiB | – |
|  | hybrid | – | 10.9 KiB | 17.9 KiB | – |
| Add one user (commit with roster change) | classical | 899 B | 1.6 KiB | 9.6 KiB | 89.7 KiB |
|  | hybrid | 7.7 KiB | 18.2 KiB | 125.4 KiB | 1.14 MiB |
| Add one user: Welcome | classical | 418 B | 476 B | 1.2 KiB | 9.1 KiB |
|  | hybrid | 1.5 KiB | 1.5 KiB | 2.2 KiB | 10.1 KiB |
| Remove one user (commit with roster change) | classical | 549 B | 1.3 KiB | 9.3 KiB | 89.4 KiB |
|  | hybrid | 3.9 KiB | 15.6 KiB | 122.8 KiB | 1.14 MiB |
| Update of 100 B, encrypted | classical | 247 B | 247 B | 247 B | 247 B |
|  | hybrid | 247 B | 247 B | 247 B | 247 B |
| Update of 100 KB, encrypted | classical | 97.8 KiB | 97.8 KiB | 97.8 KiB | 97.8 KiB |
|  | hybrid | 97.8 KiB | 97.8 KiB | 97.8 KiB | 97.8 KiB |
| Owner's device state (OpenMLS memory storage) | classical | 12.2 KiB | 27.0 KiB | 165.5 KiB | 1.49 MiB |
|  | hybrid | 45.0 KiB | 153.6 KiB | 1.03 MiB | 9.57 MiB |
| Member's device state | classical | 11.0 KiB | 25.0 KiB | 162.6 KiB | 1.49 MiB |
|  | hybrid | 43.8 KiB | 139.3 KiB | 1.00 MiB | 9.53 MiB |
| Server's public state (ratchet tree) | classical | 481 B | 2.4 KiB | 20.7 KiB | 202.8 KiB |
|  | hybrid | 3.9 KiB | 19.7 KiB | 145.6 KiB | 1.34 MiB |

Times: the most telling rows, at 100 and 1,000 members, natively on 4 cores and in Chromium (WebAssembly). Each cell is classical / hybrid. The appendix has every row for every group size and environment, including Node.js and a native run on one core.

| Median time, classical / hybrid | Native, 100 | Native, 1,000 | Chromium, 100 | Chromium, 1,000 |
|---|---:|---:|---:|---:|
| Add n − 1 members in one commit: create the commit and the Welcome | 22 / 35 ms | 304 / 533 ms | 52 / 83 ms | 703 ms / 1.10 s |
| … the server validates that commit | 10.5 / 15.3 ms | 108 / 160 ms | 29 / 36 ms | 290 / 369 ms |
| A newcomer joins (Welcome and tree) | 5.4 / 9.0 ms | 54 / 84 ms | 15.0 / 21 ms | 141 / 193 ms |
| A member's first self-update after the bulk add (worst case): create | 2.5 / 7.7 ms | 22 / 59 ms | 22 / 44 ms | 241 / 468 ms |
| … another member processes it | 1.1 / 5.7 ms | 6.9 / 42 ms | 2.9 / 12.7 ms | 18.9 / 100 ms |
| … the server validates it | 0.66 / 2.8 ms | 5.0 / 26 ms | 1.8 / 6.7 ms | 14.1 / 66 ms |
| Self-update in a healed tree: create | 1.2 / 2.5 ms | – | 3.3 / 9.9 ms | – |
| … another member processes it | 1.4 / 6.3 ms | – | 2.8 / 14.5 ms | – |
| Share with one more user (add their device and role): create | 3.1 / 7.5 ms | 23 / 52 ms | 22 / 46 ms | 242 / 500 ms |
| Update of 100 B: encrypt | 0.12 / 1.3 ms | 0.90 / 11.6 ms | 0.29 / 2.3 ms | 1.9 / 24 ms |
| … decrypt | 0.13 / 1.3 ms | 0.89 / 11.6 ms | 0.32 / 2.3 ms | 1.9 / 25 ms |
| Update of 100 KB: encrypt | 0.56 / 1.7 ms | 1.3 / 12.2 ms | 2.8 / 4.9 ms | 4.6 / 26 ms |

What keeping past epochs costs at 1,000 members: the time to encrypt a 100 B update (decrypting costs the same) and the owner's device state:

| Environment | Suite | 0 past epochs: time per 100 B update, device state | 1 past epoch (the spike's default) | 2 past epochs |
|---|---|---:|---:|---:|
| Native, 4 cores | classical | 0.12 ms, 1.05 MiB | 0.88 ms, 1.49 MiB | 1.67 ms, 1.93 MiB |
| Native, 4 cores | hybrid | 0.11 ms, 5.12 MiB | 11.8 ms, 9.56 MiB | 24.1 ms, 13.99 MiB |
| Node.js | classical | 0.25 ms, 1.05 MiB | 1.83 ms, 1.49 MiB | 3.58 ms, 1.93 MiB |
| Node.js | hybrid | 0.25 ms, 5.12 MiB | 21.7 ms, 9.56 MiB | 46.2 ms, 13.99 MiB |
| Chromium | classical | 0.28 ms, 1.05 MiB | 1.81 ms, 1.49 MiB | 3.40 ms, 1.93 MiB |
| Chromium | hybrid | 0.31 ms, 5.13 MiB | 23.4 ms, 9.56 MiB | 50.6 ms, 13.99 MiB |

WebAssembly download size: the functions in `src/wasm_api.rs`, which reach the whole client, after wasm-bindgen; the validator, the simulation and the benchmarks are not included.

| Build profile | Classical suites only | With the provisional hybrid suite |
|---|---:|---:|
| `release` (optimized for speed) | 1.86 MiB; 618 KiB with gzip -9 | 2.07 MiB; 661 KiB with gzip -9 |
| `release-size` (optimized for size: `opt-level = "z"`, full link-time optimization) | 1.43 MiB; 468 KiB with gzip -9 | 1.52 MiB; 490 KiB with gzip -9 |

The provisional hybrid suite adds 22 to 43 KiB compressed, and optimizing for size saves about a quarter. wasm-opt (Binaryen) was not applied; it usually shrinks WebAssembly further. For comparison, PERF-06 allows the web app's whole initial download 5 MB compressed, so the MLS client would take about a tenth of it, and the web app could load it only when a shared document is opened.

**Judgment on one group per document.** For the groups documents will usually have, two to a few dozen collaborators, every operation measured at 10 members takes at most 4 ms natively and 10 ms in the browser, and commits stay in the kilobytes, even with the hybrid suite; group-per-document is clearly right there. At 100 members every operation still takes under 50 ms in the browser, except adding 99 members in one commit (52 ms classical, 83 ms hybrid). At 1,000 members the design still works, but three costs need managing, all of them larger with the hybrid suite:

1. **Blank trees.** A group built by one bulk add, which is also what group re-creation produces, has a mostly blank ratchet tree, so each member's first commit encrypts to hundreds of others: up to 80 KiB (classical) or 1.13 MiB (hybrid) per commit, and up to about half a second to create in the browser. Each commit gives the nodes on its sender's path fresh keys, and once every member has committed, a commit encrypts to only about log₂(n) nodes (at 100 members: 1.2 KiB classical, 18 KiB hybrid, 3 to 15 ms in the browser). Adding members one at a time does not avoid this, because only the committer's own path gets fresh keys. Periodic self-updates, which MLS recommends anyway for post-compromise security, heal the tree over time; how to schedule them in large groups, so that hundreds of members do not send large commits at once, is a design question for the Phase-3 implementation.
2. **Per-update cost of past epochs.** To decrypt late updates, OpenMLS keeps every member's leaf for each past epoch it keeps, in the same stored record as the current epoch's message keys, and writes that record again after every update it sends or receives (`MessageSecretsStore`). At 1,000 hybrid members each kept epoch therefore adds about 12 ms per update natively and about 25 ms in the browser (1.5 ms in the browser with the classical suite), and 4.4 MiB of state (0.4 MiB classical). Devices should keep at most one past epoch, which covers the common race of an update sent just before a commit, and large groups could keep none and ask the sender to resend a late update, which CRDT updates tolerate. The cost comes from how OpenMLS 0.9 stores these secrets, not from MLS; an upstream change that writes a past epoch's members only when the epoch changes would remove it.
3. **State per document.** A device keeps 1.5 MiB (classical) or 9.5 MiB (hybrid) per 1,000-member document, mostly because OpenMLS's storage writes keys and trees as JSON text (§8) and keeps the past epoch's members.

So the recommendation is: keep one group per document; design for documents of up to a few hundred members; treat 1,000 as a supported upper bound that needs the mitigations above; and for documents meant for very large read-only audiences, plan in Phase 3 a way to give viewers access without making each of them an MLS member (for example read capabilities through snapshot keys, as link sharing will do).

### AC-4: newcomers read the current document from an encrypted snapshot

MLS gives *forward secrecy*: a newcomer cannot decrypt anything sent before they joined. ADR-0016 Decision 4 therefore asks for encrypted snapshots of the current state. The prototype (`src/snapshot.rs`):

1. A member who may share content (owner or editor) encrypts a snapshot of the document under a fresh random key, with the group's own AEAD (AES-128-GCM in the suites the plan uses) through the OpenMLS crypto provider, and a fresh random nonce. The associated data is `"BayanDocs snapshot v1"`, the group ID and the snapshot's random 16-byte ID.
2. The blob (ID, nonce, ciphertext) goes to the server, which stores it without being able to read it.
3. The member sends the snapshot's key, its ID, the AEAD and the SHA-256 hash of the blob to the group in an MLS application message of kind "snapshot key".
4. A newcomer receives that message after joining, checks that the sender's role allows sharing content, fetches the blob, checks its hash and decrypts it.

| Test (`tests/snapshots.rs`) | Shows |
|---|---|
| `newcomer_reads_the_current_state_from_a_snapshot` | A newcomer, who cannot read earlier updates, reads the current state; the server's blob does not contain the plaintext. |
| `tampered_or_swapped_blobs_are_refused` | A changed byte anywhere, another snapshot's blob, the right blob presented as another document's, or a wrong key: all refused. |
| `only_members_who_may_edit_can_share_snapshots` | A commenter's snapshot key is relayed by the server (it cannot tell) but refused by every client. |
| `removed_members_get_no_later_snapshot_keys` | A removed member cannot decrypt key messages sent after the removal. |

**Security properties**, to be reviewed by SRV-003 and the external audit (SEC-08):

- *Confidentiality from the server:* the key only travels inside MLS application messages; the server stores ciphertext and learns the blob's size and when it was uploaded. The key message is as protected as any update: with the hybrid suite it is post-quantum protected, with the classical suite it is exposed to harvest-now-decrypt-later like everything else.
- *Integrity and origin:* the AEAD tag protects the blob; the hash inside the MLS-authenticated key message pins the exact blob, so the server cannot substitute another, and the key message's sender is authenticated by MLS and checked against the roster. The hash also makes up for AES-GCM not being *key-committing*: a sender cannot craft one blob that decrypts to different documents under different keys.
- *Binding:* the associated data ties a blob to one document's group and one snapshot ID.
- *Single use:* each key encrypts one snapshot, so nonce reuse is impossible in practice.
- *Limits:* a snapshot key gives lasting access to that one snapshot: anyone who held it (every member at the time) can decrypt that blob for as long as they keep it, including after removal. That matches what they could already read; later snapshots use new keys sent only to current members (tested). A compromised device leaks the snapshot keys it holds. The server can withhold a snapshot or an old key message (denial of service, detectable through document version metadata in the sync protocol, SRV-104), but cannot forge or alter one.
- *History (threat T10):* a snapshot carries only what the sharer puts in it; the default should be the current state without edit history, as the threat model requires.

### Group re-creation for ciphersuite migration

`tests/recreate.rs` moves a group with owner, editor (two devices) and viewer from the classical suite to the second classical suite `0x0003` and to the provisional hybrid suite. The owner creates the new group, adds every device from fresh key packages for the new suite with the same roster (one commit, one Welcome), and announces the move in the old group in an update of kind "group moved", which only an owner may send. Each member then checks that the new group has exactly the old group's members and roles and the announced suite before switching; a new group with an extra member, or another suite, is refused (`members_refuse_a_move_that_changes_membership`). Old-group keys cannot read the new group's updates.

MLS has a standard way to do this, a `ReInit` proposal followed by a new group bound to the old one with a resumption pre-shared key (RFC 9420 §11.2), which would also bind the new group to the old one cryptographically. OpenMLS 0.9 does not implement it (`external_proposals.rs`: "ReInit is not yet implemented"), so the spike uses the application-level announcement above. Re-creation produces a bulk-built tree, with the first-commit costs of §5.

## 6. The post-quantum suite (measurement only)

The owner decided on 2026-10-04 that collaboration ships with the hybrid post-quantum suite from its first release. OpenMLS 0.9.0's RustCrypto provider already implements exactly that suite, `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519` (ML-KEM-768 + X25519 "X-Wing", AES-128-GCM, SHA-256, Ed25519), behind its `draft-ietf-mls-pq-ciphersuites` feature, with the provisional code point `0x004F` ("TBD1" in the draft). It uses pure-Rust implementations (RustCrypto's `ml-kem` 0.3.2 and `x-wing` 0.1.1, and libcrux's SHA-3), so it builds and runs in WebAssembly, and every flow works with it (`tests/provisional_pq.rs`, and re-creation into it).

In the spike the suite is behind the `provisional-pq` feature, labelled "PROVISIONAL, test only" wherever it appears, and nothing made with it is ever saved: `persist::export` refuses a device that holds a group with the suite or has ever created a key package for it, whose private keys would otherwise be saved with the rest of the storage (`provisional_suites_are_never_saved`, `provisional_key_packages_are_never_saved`; the review of this work package found that the first version only checked groups).

What it costs, compared with the classical suite (§5 has the numbers):

- **Unchanged:** updates (symmetric encryption), signatures (still Ed25519), the roster.
- **About 8.5 times larger:** key packages (2.6 KiB instead of 312 B), because each holds an ML-KEM public key.
- **About 1.1 KiB more per encryption to a member or subtree:** every HPKE ciphertext carries an ML-KEM ciphertext. A healed 100-member group's self-update grows from 1.2 KiB to 18 KiB; a worst-case commit at 1,000 members from 80 KiB to 1.13 MiB; the Welcome for one newcomer only from 9.1 KiB to 10.1 KiB.
- **Times:** commits take two to three times as long to create, and three to six times as long to process and validate; joining takes about 1.5 times as long.
- **State:** device and server state about 6 to 7 times larger.

None of this blocks the plan: for documents of the usual size the hybrid suite costs tens of kilobytes and a few milliseconds per commit. It makes the 1,000-member mitigations of §5 more important.

## 7. How the plan meets the amendment of 2026-10-04

- Development and tests use `0x0001`; production collaboration uses only the hybrid suite, once IANA assigns its code point and OpenMLS ships it under that code point (the provisional `0x004F` must never reach a release).
- The suite OpenMLS implements is the one the amendment names, so no other library is needed. When the code point is assigned, bayan-mls checks OpenMLS's release against the final RFC's test vectors before adopting it.
- If the code point is still unassigned at the Phase-3 gate, the gate decides between delaying the beta and launching with `0x0001` plus the re-creation migration (§5), as the amendment says. The spike shows that migration works, and what it costs at each group size.

## 8. Storage, persistence and what a library upgrade would require

- **Persistence works:** a device saves its whole MLS state, loads it again and carries on, sending, receiving and committing (`a_restored_client_continues_in_its_groups`). Saved states are parsed with limits and refused when malformed.
- **Size:** OpenMLS's in-memory storage provider, the one the spike uses, writes every key and every structure as JSON text, so a byte of key material takes three to four bytes. A device's state is 7 to 8 times the size of the ratchet tree: 163 KiB per 100-member classical document, 1.5 MiB at 1,000 members, 9.5 MiB with the hybrid suite. OpenMLS ships only this provider and one for SQLite (`openmls_sqlite_storage`, which uses the C SQLite library). The web app will need a provider of its own, for IndexedDB, implementing OpenMLS's `StorageProvider` trait (about 60 methods); bayan-mls should use a compact binary encoding there and encrypt the data at rest (SEC-11).
- **Upgrades can change the stored format, depending on its encoding.** OpenMLS leaves the encoding of stored values to the storage provider. The in-memory provider, which the spike uses, writes JSON, which records enum values by name; compact binary encodings, the kind §10 recommends for production, record them by number. OpenMLS 0.9.0 gave its enums for extension, credential and proposal types fixed storage numbers, and for GREASE values and custom types, which is where an application's own extension types such as the roster's are stored, these differ from the numbers OpenMLS 0.8.1 wrote: in a compact encoding, a custom extension type saved by 0.8.1 would be read by 0.9.0 as a GREASE value. OpenMLS's storage version number (`CURRENT_VERSION` in `openmls_traits`) is 1 in both versions, so nothing signals the change. OpenMLS 0.9 offers two ways across: the compile-time feature `0-8-1-storage-format`, which keeps the old numbers, and the `migration-import` feature, for state exported by a future version's `migration-export`. A throwaway check (two small programs, not committed) confirmed the JSON path: a member's state saved by OpenMLS 0.8.1 loads in 0.9.0, which then decrypts an update sent under 0.8.1, applies a commit made under 0.8.1, sends, commits, and saves and loads the state again. So an OpenMLS upgrade can be a data migration, not just a version bump: bayan-mls must pin OpenMLS exactly (as ADR-0017 already requires), version its own stored data, keep stored fixtures from every released version and test loading them in CI, and upgrade OpenMLS only together with a migration step whenever those fixtures show one is needed. OpenMLS 0.9 also added known-answer tests for its own stored format (`kat_storage_stability`), so upstream now guards against accidental changes too.

## 9. The fallback: mls-rs

`bayan-mls-rs-check` does the same flows with mls-rs 0.56.0 and its RustCrypto provider: a group with a custom, required group-context extension for the roster, adding members, joining with the tree fetched separately, updates of 100 B to 100 KB attributed to the right sender, a self-update, a removal, and a server that follows every commit in public framing through mls-rs's `ExternalClient`, which reports the committer and the proposals it applied. It passes natively, in Node.js and in Chromium. mls-rs also has a policy hook (`MlsRules`) that clients and the external observer can use to refuse proposals, so the role policy could be ported.

Two differences matter. mls-rs's post-quantum suites exist only in its AWS-LC provider, which is C code that does not build for WebAssembly and which ADR-0006 does not allow in clients; and ADR-0016 already notes that its post-quantum code points are private. So mls-rs is a viable fallback for the classical suite, but not for the post-quantum plan unless someone writes it a pure-Rust provider. Its dependency footprint is smaller: 85 crates for the check against 140 for the OpenMLS spike (both counts include the wasm-bindgen test harness).

## 10. What bayan-mls must handle that ADR-0016 does not mention

1. **Joining must be a transaction.** OpenMLS deletes the key package's private keys as soon as it finds them for a Welcome, before it checks the group info and the tree. If anything then fails, the Welcome can never be used again; a server could block a newcomer by sending one bad tree. The spike snapshots the client's storage before joining and restores it on failure (`Client::join`; covered by `newcomers_join_with_the_servers_tree_and_reject_a_tampered_one`, which joins successfully after two failed attempts). Production storage must offer transactions (SQLite and IndexedDB both do).
2. **Stored state can change format between OpenMLS versions without any version signal** (§8): version the stored data, keep fixtures from every release, migrate explicitly.
3. **Keep at most one past epoch.** Each kept epoch costs time on every update and state in large groups (§5).
4. **No standard re-initialization yet.** Re-creation uses the owner's announcement and the members' check (§5) until OpenMLS implements `ReInit` with resumption pre-shared keys.
5. **Storage is bulky.** A compact encoding, whose stored numbers the fixtures of point 2 pin, and an IndexedDB provider for the web (§8).
6. **Members can stall a group** (§5, AC-2): the validator cannot check a commit's MAC tags or encrypted path secrets, so it can accept a commit that every other member refuses, and it then refuses all their later commits. The server and bayan-mls need one of the recovery designs listed there; the spike recommends two-phase acceptance at the server, with an owner's move that may drop users as the last resort.

Smaller findings for the implementation: a client should cache the members and roles of each epoch instead of rebuilding them for every update (the spike does; rebuilding cost 0.4 ms per update at 1,000 members), and judge a late update by the roles of both epochs, as the validator does; adding a user and changing a role are commits with a group-context-extensions proposal, which always carry an update path, so they cost as much as a self-update; and the roster travels in every Welcome and every roster-changing commit (about 10 KiB at 1,000 users).

## 11. Recommendations (AC-5)

**Library.** Keep OpenMLS, pinned exactly (0.9.0 today), with its RustCrypto provider, as ADR-0016 decided. It does everything the design needs, natively and in WebAssembly, includes the planned post-quantum suite in pure Rust, and provides the server's validator (`PublicGroup`). mls-rs stays the fallback for the classical suite. The external audit before collaboration leaves beta (SEC-08) should cover OpenMLS and the young post-quantum crates it uses (`ml-kem` 0.3, `x-wing` 0.1), for which no third-party audit was found.

**Ciphersuite plan.** As in §7: classical `0x0001` for development and tests only; the hybrid suite for every real document once its code point is assigned; migration by re-creation, which the spike shows working; provisional code points never persisted.

**ADR-0016 amendment (proposed in the docs pull request):**

1. Record that SRV-002 passed the validation gate, with a link to this report.
2. Decision 3: Welcomes carry no ratchet tree; newcomers fetch the public tree from the server, which tracks it to validate commits.
3. Decision 4: the snapshot construction of §5, as the design for the external review, which also covers the migration announcement of item 5.
4. Decision 5: roles belong to users and live in a private-use group-context extension that every member must support; only owners change membership of other users, roles or anything else in the group context, a user's devices may add and remove each other, every commit leaves exactly one role per user in the group and at least one owner, and the group context always holds exactly the roster and its required capabilities (no external senders, nothing else). The server enforces these rules on commits and when a group is registered, and accepts updates only from devices whose user may send any (owner, editor, commenter); clients enforce the same rules on commits and Welcomes, the difference between edits and comments, which the server cannot see, and the roles of both epochs for late updates.
5. Decision 2: until OpenMLS implements `ReInit`, migration is the owner's re-creation with an announcement that members verify.
6. Implementation requirements for bayan-mls and the server: the six points of §10, the last of them an open design item for SRV-003 and SRV-103.
7. Scale: group-per-document is designed for documents of up to a few hundred members, with 1,000 as a supported upper bound; very large read-only audiences need a design that does not make every viewer an MLS member (Phase 3).

**For SRV-003 (threat model v1):** the server sees each group's members and their roles (needed to enforce them), commit times, the committer of each commit, update sizes and times, snapshot sizes; it can withhold, delay or reorder messages, block a newcomer's Welcome (mitigated by transactional joins, not prevented), and split a group by delivering different commits to different members (members then stop at different epochs; detectable). Members can stall or split a group too: a commit only the members can check stalls it until a recovery design exists (§5, AC-2), and SRV-003 should choose among the options listed there. A newcomer cannot check how the roster came to be, only that an owner (or their own device) added them; roster transparency is a possible later addition. One open choice: MLS lets a sender attach *authenticated data* to an encrypted update, which the server can read but not change without the recipients noticing. Putting the update's kind there would let the server refuse a commenter's edits too (clients would check that it matches the decrypted kind), at the cost of telling the server which updates are comments. The spike keeps the kind encrypted, following Decision 9's metadata minimization. The spike's identities are plain `user/device` names; production needs the cross-signed device credentials of ADR-0016 Decision 6, and server-side checks that a new device belongs to its user.

**For SRV-103 (delivery and authentication services):** reuse the validator's design: per-group ordering with commits checked against the epoch, the transport-authenticated device, OpenMLS's public state and the shared policy; updates checked for framing, epoch (current or previous) and the sender's role; Welcomes without trees and a tree endpoint; limits on message sizes (16 MiB here) and key packages per device (100 here); and a recovery design for commits the validator accepts but the members refuse (§5, AC-2), which the spike does not build. At 1,000 hybrid members the validator spends about 25 ms of CPU per ordinary commit natively (160 ms for a commit that adds 999 members), and checking an update costs almost nothing (under 0.01 ms).

## 12. Limitations

- One machine, one run per configuration; the numbers show orders of magnitude and trends, not guarantees.
- The transport, accounts and device ownership are simulated; credentials are basic credentials with no cross-signing.
- The validator cannot check a commit's MAC tags or encrypted path secrets, so any member can stall a group (§5, AC-2); the spike documents and tests the limitation but builds no recovery.
- The "healed tree" rows are measured up to 100 members only (making every member of a 1,000-member group join and commit is too slow for this harness); at 1,000 members, a healed self-update encrypts to about log₂(1,000) ≈ 10 subtrees.
- Storage is OpenMLS's in-memory provider; a real IndexedDB or SQLite provider will differ in speed and size.
- The upgrade check covered one two-member classical group stored as JSON; the change for compact encodings (§8) comes from reading both versions' source code, not from a test.
- The post-quantum suite runs on a provisional code point from a draft that may still change.

## Appendix: all timings

Medians, in every environment. In WebAssembly the first measurements of each run (at 2 members) include warm-up, such as the tiered compilation of the WebAssembly code by Node.js or the browser, which is why creating a key package there takes about 9 ms instead of 0.3 ms. "Update: server check" is the validator's check of an encrypted update (framing, epoch and the sender's role).

### Native, 4 cores

| Median time | Suite | 2 | 10 | 100 | 1,000 |
|---|---|---:|---:|---:|---:|
| Create a key package | classical | 0.16 ms | 0.10 ms | 0.10 ms | 0.10 ms |
|  | hybrid | 0.34 ms | 0.27 ms | 0.27 ms | 0.31 ms |
| Bulk add: create commit + Welcome | classical | 0.71 ms | 2.34 ms | 22.1 ms | 304 ms |
|  | hybrid | 0.78 ms | 3.87 ms | 35.3 ms | 533 ms |
| Bulk add: server validates | classical | 0.29 ms | 1.20 ms | 10.5 ms | 108 ms |
|  | hybrid | 0.34 ms | 1.53 ms | 15.3 ms | 160 ms |
| Bulk add: a newcomer joins | classical | 0.36 ms | 0.81 ms | 5.45 ms | 54.0 ms |
|  | hybrid | 0.70 ms | 1.65 ms | 8.99 ms | 84.2 ms |
| Self-update (bulk-built tree): create | classical | 0.22 ms | 0.58 ms | 2.54 ms | 21.6 ms |
|  | hybrid | 0.51 ms | 1.15 ms | 7.71 ms | 59.0 ms |
| Self-update: server validates | classical | 0.12 ms | 0.21 ms | 0.66 ms | 4.99 ms |
|  | hybrid | 0.17 ms | 0.44 ms | 2.77 ms | 25.6 ms |
| Self-update: another member processes | classical | 0.25 ms | 0.49 ms | 1.07 ms | 6.87 ms |
|  | hybrid | 0.72 ms | 1.35 ms | 5.68 ms | 41.7 ms |
| Self-update (healed tree): create | classical | – | 0.62 ms | 1.17 ms | – |
|  | hybrid | – | 1.13 ms | 2.48 ms | – |
| Self-update (healed tree): process | classical | – | 0.47 ms | 1.44 ms | – |
|  | hybrid | – | 1.47 ms | 6.35 ms | – |
| Add one user: create | classical | 0.72 ms | 0.83 ms | 3.08 ms | 22.6 ms |
|  | hybrid | 1.43 ms | 1.56 ms | 7.49 ms | 52.4 ms |
| Add one user: server validates | classical | 0.24 ms | 0.33 ms | 0.92 ms | 5.54 ms |
|  | hybrid | 0.38 ms | 0.65 ms | 2.69 ms | 24.6 ms |
| Add one user: a member processes | classical | 0.58 ms | 0.53 ms | 1.40 ms | 7.95 ms |
|  | hybrid | 1.03 ms | 1.65 ms | 5.26 ms | 40.9 ms |
| Add one user: the newcomer joins | classical | 0.39 ms | 0.78 ms | 5.55 ms | 53.8 ms |
|  | hybrid | 0.78 ms | 1.41 ms | 7.55 ms | 71.3 ms |
| Remove one user: create | classical | 0.31 ms | 0.73 ms | 2.85 ms | 24.0 ms |
|  | hybrid | 0.53 ms | 1.14 ms | 7.46 ms | 54.9 ms |
| Remove one user: a member processes | classical | 0.32 ms | 0.41 ms | 1.19 ms | 7.67 ms |
|  | hybrid | 0.84 ms | 1.36 ms | 5.17 ms | 39.8 ms |
| Update 100 B: encrypt | classical | 0.04 ms | 0.05 ms | 0.12 ms | 0.90 ms |
|  | hybrid | 0.06 ms | 0.16 ms | 1.30 ms | 11.6 ms |
| Update 100 B: decrypt | classical | 0.06 ms | 0.06 ms | 0.13 ms | 0.89 ms |
|  | hybrid | 0.08 ms | 0.17 ms | 1.31 ms | 11.6 ms |
| Update 100 KB: encrypt | classical | 0.66 ms | 0.71 ms | 0.56 ms | 1.32 ms |
|  | hybrid | 0.51 ms | 0.61 ms | 1.73 ms | 12.2 ms |
| Update 100 KB: decrypt | classical | 0.40 ms | 0.44 ms | 0.40 ms | 1.16 ms |
|  | hybrid | 0.35 ms | 0.44 ms | 1.55 ms | 12.1 ms |
| Update: server check | classical | 0.00 ms | 0.00 ms | 0.00 ms | 0.00 ms |
|  | hybrid | 0.00 ms | 0.00 ms | 0.00 ms | 0.00 ms |

### Native, 1 core (`RAYON_NUM_THREADS=1`)

| Median time | Suite | 2 | 10 | 100 | 1,000 |
|---|---|---:|---:|---:|---:|
| Create a key package | classical | 0.19 ms | 0.12 ms | 0.11 ms | 0.11 ms |
|  | hybrid | 0.38 ms | 0.29 ms | 0.27 ms | 0.29 ms |
| Bulk add: create commit + Welcome | classical | 0.61 ms | 2.46 ms | 20.7 ms | 310 ms |
|  | hybrid | 0.86 ms | 3.86 ms | 35.8 ms | 462 ms |
| Bulk add: server validates | classical | 0.29 ms | 1.29 ms | 12.1 ms | 123 ms |
|  | hybrid | 0.35 ms | 1.55 ms | 14.9 ms | 151 ms |
| Bulk add: a newcomer joins | classical | 0.39 ms | 1.09 ms | 6.66 ms | 55.1 ms |
|  | hybrid | 0.73 ms | 1.57 ms | 10.0 ms | 74.7 ms |
| Self-update (bulk-built tree): create | classical | 0.34 ms | 1.07 ms | 7.34 ms | 84.8 ms |
|  | hybrid | 0.50 ms | 2.40 ms | 15.8 ms | 161 ms |
| Self-update: server validates | classical | 0.15 ms | 0.25 ms | 0.71 ms | 8.77 ms |
|  | hybrid | 0.17 ms | 0.52 ms | 2.49 ms | 24.0 ms |
| Self-update: another member processes | classical | 0.39 ms | 0.53 ms | 1.39 ms | 8.64 ms |
|  | hybrid | 0.69 ms | 1.79 ms | 5.53 ms | 40.1 ms |
| Self-update (healed tree): create | classical | – | 0.75 ms | 1.22 ms | – |
|  | hybrid | – | 1.47 ms | 4.05 ms | – |
| Self-update (healed tree): process | classical | – | 0.43 ms | 1.22 ms | – |
|  | hybrid | – | 1.36 ms | 8.14 ms | – |
| Add one user: create | classical | 0.80 ms | 1.49 ms | 7.68 ms | 74.9 ms |
|  | hybrid | 6.81 ms | 2.34 ms | 24.7 ms | 168 ms |
| Add one user: server validates | classical | 0.29 ms | 0.47 ms | 0.94 ms | 6.61 ms |
|  | hybrid | 0.37 ms | 0.61 ms | 2.91 ms | 24.5 ms |
| Add one user: a member processes | classical | 0.54 ms | 0.73 ms | 1.40 ms | 8.38 ms |
|  | hybrid | 2.91 ms | 1.56 ms | 8.77 ms | 41.5 ms |
| Add one user: the newcomer joins | classical | 0.43 ms | 0.91 ms | 5.93 ms | 62.1 ms |
|  | hybrid | 0.86 ms | 1.39 ms | 12.3 ms | 75.9 ms |
| Remove one user: create | classical | 0.29 ms | 1.07 ms | 7.99 ms | 88.3 ms |
|  | hybrid | 0.56 ms | 1.99 ms | 19.9 ms | 196 ms |
| Remove one user: a member processes | classical | 0.28 ms | 0.52 ms | 1.31 ms | 12.3 ms |
|  | hybrid | 0.75 ms | 1.44 ms | 8.53 ms | 40.2 ms |
| Update 100 B: encrypt | classical | 0.05 ms | 0.05 ms | 0.11 ms | 1.00 ms |
|  | hybrid | 0.06 ms | 0.15 ms | 1.22 ms | 11.6 ms |
| Update 100 B: decrypt | classical | 0.06 ms | 0.07 ms | 0.12 ms | 1.00 ms |
|  | hybrid | 0.07 ms | 0.17 ms | 1.25 ms | 11.6 ms |
| Update 100 KB: encrypt | classical | 0.74 ms | 0.72 ms | 0.60 ms | 1.36 ms |
|  | hybrid | 0.53 ms | 0.61 ms | 1.63 ms | 13.8 ms |
| Update 100 KB: decrypt | classical | 0.48 ms | 0.47 ms | 0.42 ms | 1.19 ms |
|  | hybrid | 0.36 ms | 0.44 ms | 1.45 ms | 13.1 ms |
| Update: server check | classical | 0.00 ms | 0.00 ms | 0.00 ms | 0.00 ms |
|  | hybrid | 0.00 ms | 0.00 ms | 0.00 ms | 0.00 ms |

### Node.js 24.21.0 (WebAssembly)

| Median time | Suite | 2 | 10 | 100 | 1,000 |
|---|---|---:|---:|---:|---:|
| Create a key package | classical | 9.17 ms | 0.25 ms | 0.25 ms | 0.23 ms |
|  | hybrid | 2.65 ms | 0.66 ms | 0.62 ms | 0.64 ms |
| Bulk add: create commit + Welcome | classical | 11.6 ms | 5.29 ms | 43.5 ms | 574 ms |
|  | hybrid | 2.92 ms | 8.80 ms | 79.4 ms | 909 ms |
| Bulk add: server validates | classical | 7.75 ms | 2.84 ms | 24.9 ms | 231 ms |
|  | hybrid | 0.82 ms | 3.52 ms | 31.6 ms | 308 ms |
| Bulk add: a newcomer joins | classical | 5.07 ms | 2.12 ms | 12.8 ms | 117 ms |
|  | hybrid | 2.78 ms | 3.75 ms | 18.6 ms | 168 ms |
| Self-update (bulk-built tree): create | classical | 0.72 ms | 2.01 ms | 17.6 ms | 196 ms |
|  | hybrid | 1.43 ms | 4.58 ms | 41.4 ms | 394 ms |
| Self-update: server validates | classical | 0.35 ms | 0.46 ms | 1.56 ms | 11.7 ms |
|  | hybrid | 0.46 ms | 0.98 ms | 6.51 ms | 53.3 ms |
| Self-update: another member processes | classical | 0.81 ms | 0.99 ms | 2.48 ms | 14.8 ms |
|  | hybrid | 1.87 ms | 3.00 ms | 12.7 ms | 83.8 ms |
| Self-update (healed tree): create | classical | – | 1.27 ms | 2.75 ms | – |
|  | hybrid | – | 3.60 ms | 8.71 ms | – |
| Self-update (healed tree): process | classical | – | 0.85 ms | 2.39 ms | – |
|  | hybrid | – | 3.20 ms | 12.8 ms | – |
| Add one user: create | classical | 1.48 ms | 2.89 ms | 19.0 ms | 193 ms |
|  | hybrid | 2.78 ms | 5.81 ms | 44.8 ms | 394 ms |
| Add one user: server validates | classical | 0.65 ms | 0.85 ms | 1.98 ms | 12.5 ms |
|  | hybrid | 0.93 ms | 1.42 ms | 7.42 ms | 57.4 ms |
| Add one user: a member processes | classical | 1.31 ms | 1.31 ms | 2.80 ms | 15.5 ms |
|  | hybrid | 2.45 ms | 3.54 ms | 13.7 ms | 88.4 ms |
| Add one user: the newcomer joins | classical | 1.01 ms | 1.96 ms | 12.2 ms | 116 ms |
|  | hybrid | 2.03 ms | 3.20 ms | 19.0 ms | 154 ms |
| Remove one user: create | classical | 1.31 ms | 2.40 ms | 17.3 ms | 195 ms |
|  | hybrid | 1.50 ms | 4.40 ms | 37.6 ms | 377 ms |
| Remove one user: a member processes | classical | 0.78 ms | 1.04 ms | 2.53 ms | 15.4 ms |
|  | hybrid | 1.88 ms | 3.14 ms | 11.2 ms | 84.3 ms |
| Update 100 B: encrypt | classical | 0.12 ms | 0.12 ms | 0.26 ms | 1.68 ms |
|  | hybrid | 0.17 ms | 0.32 ms | 2.12 ms | 20.4 ms |
| Update 100 B: decrypt | classical | 0.15 ms | 0.15 ms | 0.28 ms | 1.67 ms |
|  | hybrid | 0.21 ms | 0.35 ms | 2.15 ms | 20.4 ms |
| Update 100 KB: encrypt | classical | 2.25 ms | 2.20 ms | 2.36 ms | 4.11 ms |
|  | hybrid | 2.45 ms | 2.42 ms | 4.30 ms | 23.0 ms |
| Update 100 KB: decrypt | classical | 2.04 ms | 2.02 ms | 2.26 ms | 3.88 ms |
|  | hybrid | 2.26 ms | 2.25 ms | 4.07 ms | 22.9 ms |
| Update: server check | classical | 0.00 ms | 0.00 ms | 0.00 ms | 0.00 ms |
|  | hybrid | 0.00 ms | 0.00 ms | 0.00 ms | 0.01 ms |

### Chromium 153 headless, in a dedicated worker (WebAssembly)

| Median time | Suite | 2 | 10 | 100 | 1,000 |
|---|---|---:|---:|---:|---:|
| Create a key package | classical | 8.62 ms | 0.33 ms | 0.28 ms | 0.28 ms |
|  | hybrid | 2.51 ms | 0.73 ms | 0.67 ms | 0.68 ms |
| Bulk add: create commit + Welcome | classical | 13.3 ms | 5.55 ms | 51.6 ms | 703 ms |
|  | hybrid | 2.86 ms | 9.57 ms | 83.2 ms | 1.10 s |
| Bulk add: server validates | classical | 6.24 ms | 3.20 ms | 29.2 ms | 290 ms |
|  | hybrid | 0.83 ms | 3.73 ms | 36.2 ms | 369 ms |
| Bulk add: a newcomer joins | classical | 10.5 ms | 2.71 ms | 15.0 ms | 141 ms |
|  | hybrid | 2.65 ms | 4.13 ms | 20.7 ms | 193 ms |
| Self-update (bulk-built tree): create | classical | 0.80 ms | 2.60 ms | 21.7 ms | 241 ms |
|  | hybrid | 1.38 ms | 5.12 ms | 44.3 ms | 468 ms |
| Self-update: server validates | classical | 0.44 ms | 0.54 ms | 1.84 ms | 14.1 ms |
|  | hybrid | 0.45 ms | 1.12 ms | 6.68 ms | 66.2 ms |
| Self-update: another member processes | classical | 0.93 ms | 1.22 ms | 2.90 ms | 18.9 ms |
|  | hybrid | 1.76 ms | 3.52 ms | 12.7 ms | 99.7 ms |
| Self-update (healed tree): create | classical | – | 1.43 ms | 3.28 ms | – |
|  | hybrid | – | 3.67 ms | 9.93 ms | – |
| Self-update (healed tree): process | classical | – | 0.97 ms | 2.79 ms | – |
|  | hybrid | – | 3.23 ms | 14.5 ms | – |
| Add one user: create | classical | 1.92 ms | 3.90 ms | 21.8 ms | 242 ms |
|  | hybrid | 2.94 ms | 6.29 ms | 45.8 ms | 500 ms |
| Add one user: server validates | classical | 0.80 ms | 0.86 ms | 2.36 ms | 15.5 ms |
|  | hybrid | 0.94 ms | 1.57 ms | 7.00 ms | 62.1 ms |
| Add one user: a member processes | classical | 1.52 ms | 1.55 ms | 3.33 ms | 20.0 ms |
|  | hybrid | 2.50 ms | 3.80 ms | 13.1 ms | 101 ms |
| Add one user: the newcomer joins | classical | 1.23 ms | 3.09 ms | 15.8 ms | 146 ms |
|  | hybrid | 2.06 ms | 3.56 ms | 19.7 ms | 183 ms |
| Remove one user: create | classical | 1.36 ms | 2.41 ms | 22.2 ms | 247 ms |
|  | hybrid | 1.44 ms | 5.25 ms | 42.6 ms | 469 ms |
| Remove one user: a member processes | classical | 0.90 ms | 1.28 ms | 2.94 ms | 20.0 ms |
|  | hybrid | 1.85 ms | 3.31 ms | 12.3 ms | 106 ms |
| Update 100 B: encrypt | classical | 0.15 ms | 0.14 ms | 0.29 ms | 1.94 ms |
|  | hybrid | 0.17 ms | 0.34 ms | 2.29 ms | 24.3 ms |
| Update 100 B: decrypt | classical | 0.18 ms | 0.17 ms | 0.32 ms | 1.93 ms |
|  | hybrid | 0.20 ms | 0.37 ms | 2.33 ms | 24.7 ms |
| Update 100 KB: encrypt | classical | 2.76 ms | 2.76 ms | 2.78 ms | 4.55 ms |
|  | hybrid | 2.62 ms | 2.85 ms | 4.86 ms | 26.1 ms |
| Update 100 KB: decrypt | classical | 2.62 ms | 2.50 ms | 2.57 ms | 4.38 ms |
|  | hybrid | 2.43 ms | 2.62 ms | 4.66 ms | 26.6 ms |
| Update: server check | classical | 0.00 ms | 0.00 ms | 0.00 ms | 0.01 ms |
|  | hybrid | 0.00 ms | 0.00 ms | 0.00 ms | 0.01 ms |
