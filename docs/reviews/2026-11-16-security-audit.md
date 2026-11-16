# Security review, 16 November 2026

Done with the help of an AI assistant (Claude, Anthropic), on version 0.22.7 (the code of 0.23.0 changes the dependency of finding 1 and adds the layout of spaces). It is a review, not an independent audit: it reads the paths that carry the most risk, checks the dependencies, tests one live installation from its own network, and looks at the plan documents for what the next features would add. What it did not read is listed at the end.

## Findings

| # | Severity | Finding | State |
|---|---|---|---|
| 1 | Medium | **Dependency with a known vulnerability.** `libsqlite3-sys` 0.38.2 bundles a SQLite that can return uninitialised memory when a database is opened next to a `-wal` file whose page size differs (RUSTSEC-2026-0459, CVE-2026-119375, CVSS 6.8, found with `cargo audit`). The attacker needs to hand the victim a crafted pair of files, so the exposure is `adopt` and the restore of a backup that did not come from you. | **Fixed**: `rusqlite` 0.41.1 and `libsqlite3-sys` 0.39.1 (SQLite 3.54.1); minimum Rust raised to 1.89; `cargo audit` clean. An installation must be rebuilt with it. The advisory also says the `bundled-sqlcipher` feature of those releases still carries the old SQLite: see the encryption plan. |
| 2 | Medium | **No time limit on a connection.** A client that sends its request slowly holds one thread for as long as it likes. Requests in flight are capped (16 per address, 64 in all), so one machine cannot take the server down, but a few addresses can keep everyone else out. | Open. Run a proxy that enforces its own timeouts in front of any network listener, or set a deadline on reading a request (or replace the HTTP library). |
| 3 | Medium | **Not encrypted at rest.** Anyone who reads the data directory or a backup reads the memory. | Open, known and stated in SECURITY.md. Design: [ENCRYPTION.md](../ENCRYPTION.md). |
| 4 | Medium (deployment) | **The Home Assistant add-on answers in clear on the home network.** From another machine of the network, `http://<add-on>:8791` answers: the tokens of an assistant that connects there travel unencrypted on that network. Observed: with no key, the MCP address answers 404 and `/admin/api/*` answers 401, so authentication holds; the sign-in page shell (static HTML, no data) is served to anyone. | Open. Connect assistants through Tailscale or a TLS proxy only ([TAILSCALE.md](../TAILSCALE.md)); in the add-on, stop publishing port 8791 on the home network once nothing depends on it. |
| 5 | Low | The static page of the administration is served without sign-in on the main listener (the API behind it is not). No secret in it. | Accepted; to remove when the page is only served on the identity and ingress listeners. |

## What holds

- Requests: the size a request may bring is decided from its headers **before** the body is read, and only a request with a key of the installation may send a large one.
- Keys: only their SHA-256 digest is stored, compared in constant time; failed attempts are limited per address; OAuth uses PKCE S256 and refresh tokens that rotate; the consent grants nothing that was not ticked.
- Browser: the administration page has a strict content security policy, `no-store`, `nosniff`, and every action needs a header that a foreign site cannot send without a preflight.
- Code: `unsafe` is forbidden by the lints of the workspace; the SQL built with `format!` only inserts constant column lists, every value is a bound parameter; the tar writer only packs (nothing extracts an archive).
- Content: files are typed by content, stored by hash and served in a sandbox; what comes back from the memory is flagged as data.
- Layout of spaces (new, this release): only the owner of a space can lay it out, a parent must be the owner's, cycles and depth are refused; it changes no file and no right.

## What the plan adds, and how it is held

| Feature | Risk | Holds if |
|---|---|---|
| Peers (FEDERATION.md) | a stolen or hostile node; a forged event; a loop | nodes are keyed and confirmed by the person; each node enforces the rights of the key it talks to; events are checked for their author and their space; claims and marks stop loops; revocation |
| Mounted folders (SPACES.md) | path escape by a link; a synchroniser racing the memory; a sync tool copying the databases | paths resolved and kept under the root, links not followed out of it; one synchroniser per folder; the server refuses to start with its data in a mount |
| A model editing rules | a note that says "send this to the work drive" and a model that obeys | the router stays deterministic; additions on non-sensitive spaces only; unique keywords, sensitive spaces, outward rules and removals wait for the person; every change in the history and reversible |
| Passphrase and keys (ENCRYPTION.md) | a lost passphrase; a weak one; a key left in memory; a library that is itself out of date | recovery key shown once; Argon2id; keys wiped on lock; the encryption library chosen after its own advisories are read |
| Tailscale (TAILSCALE.md) | Funnel opening the wrong thing; the login header trusted from the wrong place | Funnel on the MCP address only, identity listener behind Serve only, trust limited to one address |

## Not covered by this review

`oauth.rs` line by line; the parsers of imports (zip, xlsx, Markdown rules) against decompression bombs; the janitor's calls to a model and what a hostile note can do to them; the finances tool; the Hermes integration; the scripts of the add-on; the JavaScript of the administration page (cross-site scripting); the tests of timing. These are the next steps, in this order, before the project is recommended for anything but your own use.
