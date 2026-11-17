# Security

memcp holds what you would not post anywhere: health, money, the people around you. Read this before you expose it.

## What it does to protect that

- **Tokens** are shown once; only their SHA-256 digest is stored. Failed attempts are rate limited per address.
- **The server listens on the loopback** unless you pass `--allow-remote`. Without TLS, tokens travel in clear on any other interface: put a TLS proxy in front (Tailscale `serve`, Caddy…).
- **Rights are explicit.** A connection gets named spaces with read or read-write, and never more than the person it belongs to. Administrators manage access; they do not read memory content on the admin page.
- **What comes back to an agent is data, flagged as such** (memory facts), so an instruction hidden in it is not a command.
- **Files are typed by their content**, not their name, stored once by SHA-256, and served with `sandbox` and `nosniff`.
- **The server proxies nothing.** It calls no other server on behalf of an assistant.
- **The admin page is not on the listener that faces the network.** It is served on the Home Assistant ingress, on the tailnet identity listener, and on the main listener only when that one is on the loopback or `--console-admin` is given; never to a request that came through Tailscale Funnel. Failed console tokens are counted per address with the same guard as failed MCP keys.
- **Standing instructions of a space can be kept by an assistant that writes it whole (`guide_set`); the person's own guide cannot.** The guide of a person is written only on the admin page; a key that opens MCP is refused on the admin route. A space's guide can be replaced by a connection that writes that space whole (never one limited to paths): every text stays in the history, each write is audited (`guide_set_by_assistant`), and the person restores any earlier one. The trade-off is chosen by the owner: an assistant that is misled by what it read could write a lasting rule into such a space. Everything else an assistant reads back is flagged as data, and the index of a space is told as data, with the section marker broken inside it.
- **A connection reaches only what was chosen.** The OAuth consent lists the spaces a person reaches with nothing ticked and grants nothing otherwise; a connector added with the address of a space (`/mcp/<space>`) gets that space only, whatever the form carries; a key used at the address of a space is seen through that space alone. A new sign-in or an unknown Home Assistant login waits for an administrator instead of joining the household.
- **Leaving is always possible.** `export` writes a person's memory with a fingerprint of every file and no key, connection or right; `adopt` checks everything before it changes anything and undoes itself on failure.

## What you must still get right

- **The identity listener** (the sign-in page for Claude on the web, `--identity-listen`) believes the `Tailscale-User-Login` header of requests from the loopback, or from the one address given with `--identity-trust`. Never put it behind Funnel. On a host with local accounts you do not trust, anyone local could claim a tailnet identity.
- **An assistant's access is a decision.** Granting a space to an agent lets whatever model or service runs it read that space. Grant one space at a time, read-only first.
- **A key opened for a local assistant** (`memcp mcp`) lives in the environment of the program that runs it. Anything that can read that environment holds the key; revoke it when in doubt.
- **The memory is not encrypted at rest** (a design for locking it with a passphrase or a key is in [docs/ENCRYPTION.md](docs/ENCRYPTION.md)). Anyone who can read the data directory can read the memory, and so can anyone who can read a backup of it. Home Assistant can encrypt its own backups; turn that on. Encryption of the files themselves is designed (docs/MODEL.md) and not built.
- **The memory is not a medical device**: it gives no diagnosis.

## Status

The code has automated tests and runs in daily use, and it was largely written with AI assistants (see the README). It has **not** been audited by an independent party; a first review, made with an AI assistant, is in [docs/reviews/2026-11-16-security-audit.md](docs/reviews/2026-11-16-security-audit.md), with what it found and what it did not read. Dependencies are checked with `cargo audit` in CI.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting (the Security tab of the repository, « Report a vulnerability »). Say what you found, how to reproduce it and what it exposes. Expect an answer within a week.
