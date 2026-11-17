# Handover

State on 2026-11-17. Versions are `AAAAMMJJ.0.N`; main is at 20261117.0.6 (a release builds from each tag; the Home Assistant app updates itself).

**Everything below is on main.** The agent branches (sync, janitor keywords, merge, library, tests, window, page) are merged.

- **Nodes and peers:** a node that joins is a peer (`peer-<device>-xxxx`, read and write on every space of the person). Join by request (`POST /join`, three words) or, for a device allowed in advance, `POST /admin/api/join/allow-device`. Sync between peers: `memcp-sync` (client), `/mcp/<connection>/sync/{status,spaces,events}`, protocol number, live status with direction and percent, `GET /admin/api/sync/status` for the page. A replica may hold only the last N days (`memcp-sync --window-days N`; pruning is local).
- **Discovery:** the add-on runs on the host network and advertises `_memcp._tcp`; the page lists the nodes found (`memcp-capture discover`). Another network (VLAN) does not see it: use the tailnet address.
- **MCP tools:** `spaces`, `stats`, `peers`, `triage_claim`/`triage_release`, `rules_get`/`rules_set`, `vocabulary_terms`, `library_search`/`library_raise`, `links`, `living_get`/`living_set`. One address carries the key: `/mcp/<connection>?key=…`; `/mcp/<person>/<connection>` is accepted.
- **Janitor:** deterministic router by keywords (the `rules` note of each space, plus vocabularies imported by the person), frequency rule, the model only proposes keywords, the sort runs again; it claims the notes of À trier with `triage_claim`.
- **Vocabularies:** `memcp vocabulary import|list|remove` (docs/VOCABULARIES.md). The page block is not built.
- **Merge:** `memcp merge --plan|--apply` (docs/MERGE.md). Not on the page or in MCP.
- **Library:** `memcp library add|scan|list`; docx and pdf are indexed by name and dates only.
- **Renamed:** the bridge is the capture program (`memcp-capture`, `memcp-capture-ffi`, `crates/capture*`).

**Open:** the page block for vocabularies; text of docx and pdf in the library; merge on the page; the real test of "Add a node" against a second real node; French completeness (agent on branch `page-polish`); the stale page tests (same agent).

**Pitfalls:** never `git add -A` at the root while agent worktrees exist (`.claude/worktrees/` is ignored). The server runs on the Home Assistant app: `ha store reload` then `ha apps update 94769107_memcp`; a reinstall wipes data (a backup and `memcp adopt` bring it back, but not the connections, the heir or the recovery key: those are in the registry). Never put names of people or hosts in the repository (the audit hook refuses them, and refuses a session line in a commit). The Mac cannot reach the local network of the server: use the tailnet address. The key in the address (`?key=`) can end in logs: the header is safer.
