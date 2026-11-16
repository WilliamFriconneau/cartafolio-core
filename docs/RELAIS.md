# Handover

State at 2026-11-16, about 23:55. Versions are `AAAAMMJJ.0.N`; main is at 20261116.0.5 (release builds from the tag).

**On main, finished:** peer join (`POST /join` approved gives `peer-<device>-xxxx`, read and write on every space of the person); `/mcp/<person>/<connection>`; MCP tools `spaces`, `triage_claim`/`triage_release`, `rules_get`/`rules_set`, `peers`; `memcp init --defaults`; the À trier warning in the instructions of a connecting assistant; docs/UI-SPEC.md (screens per app), docs/MERGE.md (design).

**Branches, each pushed to origin, none merged into main:**
- `merge-memories` (finished): `memcp merge --plan/--apply` (crates/core/src/merge.rs, 16 tests). Missing: the page, an MCP tool, merge between live nodes, the other memory's layout.
- `library-links` (finished): `memcp library add/scan/list`, MCP `library_search`, `library_raise`, `links`, migration SPACE_V9. Missing: text of docx/pdf (names and dates only), a rescan tool or timer. Its tests that touch pairing may need the main version of `crates/core/tests/pairing.rs`.
- `sync-peers` (agent still working, no commit yet at this time): event log, catch-up, `/sync/events`, `/sync/status` (per peer: direction, spaces done/total/percent), a Peers block on the page. Read the branch before touching it.
- `janitor-keywords` (agent still working, no commit yet): keywords per space in the `rules` note, deterministic router, the model only proposes keywords, then the sort runs again.

**Pitfalls:** do not `git add -A` at the root while agent worktrees exist (`.claude/worktrees/` is ignored now). The server runs on the Home Assistant app, updated with `ha store reload` then `ha apps update 94769107_memcp`; a reinstall wipes data (a backup and `memcp adopt` bring it back). Connections never travel with an export. The 100 audio recordings in À trier are not transcribed or erased by the voice job (cause not found; the job's state is in the voice job's `voice.json`). Never put names of people or hosts in the repository (the audit hook refuses them). The Mac cannot reach the local network of the server: use the tailnet address.

**Still wanted by William:** one long URL to connect an assistant (no two round trips between windows); no word "bridge" in any text (the crates and binaries still carry the name); À trier empty by keywords, not by a model per note; peers visible live with direction and percent.
