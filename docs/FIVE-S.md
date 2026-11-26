# 5S of the code

The method of [CONCEPTS.md](CONCEPTS.md) applied to this repository. Rerun it before a release.

| S | In the code | How to check |
|---|---|---|
| Sort (*seiri*) | remove what nothing uses: functions, constants, types, dependencies, features, scripts, documents | `scripts/dead.sh` |
| Set in order (*seiton*) | one place for each thing: one cleaner for a keyword or a tag list (`pensine_core::facts::clean_keyword`, `clean_rules_list`), one folding function (`pensine_core::text::fold`), each document linked from [README.md](README.md) | `scripts/dead.sh` (last two sections), review |
| Shine (*seiso*) | no warning, formatted, no copy of a helper | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Standardise (*seiketsu*) | the same words everywhere: *space* (never *vault*), *node* or *the app* (never *bridge*, never *capture program*), *peer* or *node* (never *proxy*); the same checks for everyone | `scripts/check.sh`, the forbidden-word test of the page (`crates/server/tests/page/onboarding.test.js`) |
| Sustain (*shitsuke*) | the pass is cheap and written down, so it gets repeated | `scripts/check.sh` runs `scripts/dead.sh` when asked, CI runs the rest |

## The tools

`scripts/dead.sh` runs, and only reads:

1. the compiler with `-W dead_code -W unused` (through clippy);
2. `cargo machete` and `cargo shear` (unused or misplaced dependencies), when installed: `cargo install cargo-machete cargo-shear`;
3. a search for public functions, constants and types that are named once in the whole workspace;
4. documents of `docs/` that [README.md](README.md) does not link;
5. the forbidden words in code and docs.

`cargo udeps` needs a nightly toolchain and was not run; `cargo +nightly udeps --workspace --all-targets` is the extra check if one is installed.
Run it all with `scripts/check.sh dead`, or `MEMCP_DEAD=1 scripts/check.sh` to add it to the whole set.

## Kept on purpose

These names look like leftovers and are not. They are on disk, on the wire or in the Swift app's API, and changing them would break an existing installation:

- `vaults` where an older program wrote it and the node still reads it (the answer of `POST /pair`, the manifest of an export made before the rename and its `vaults/` folder, `vault_names` of the people list); what the node writes is `spaces` ([CHANGELOG-API.md](CHANGELOG-API.md)). `Pairing.vaults` of `pensine-capture-ffi` stays next to `spaces`, deprecated. `inbox_vaults` (janitor setting); `memcp vault add` (alias of `memcp space new`); the `--vault` flag (refused with a message).
- `bridge` where an installation made before the rename is migrated or still read: the `bridge_pairings` table in the migrations that made it and renamed it (`CONTROL_V13`, `CONTROL_V15`, `CONTROL_V22`) and in their test; in the installers, the old `bridge/` folder, `bridge.conf`, `bridge.key`, the `desktop-bridge` connection, the task and plist labels, `--bridge-port` and `MEMCP_BRIDGE_PORT` (read, said to be unused, ignored for one release); `memcp prepare transcribe`, which removes the old `bridge.conf`; and the `bridge-` prefix of old connection names, which the page does not show (the address of a connection is its name, so the name is not changed).
- `"app": "memcp-capture"` in the answer the node gives the browser extension of the apps: the extension looks for that value.
- `crates/capture-ffi`, the crate `pensine-capture-ffi` and the library `pensine_capture_ffi`: the apps that already link them keep the names (a new app crate has its own). Every public item that is not marked deprecated is the Swift app's.
- Features `local-whisper` and `metal` of `pensine-server` and `sync` of `pensine-client` are enabled by the Mac app build, the release workflow or `ha-addon/build.sh`, not by the default build.

## Last pass

17 November 2026. Removed: the Hermes plugin (an assistant connects through the MCP, one way in for all), the handover notes that had no place in a public repository. Linked from the index: CRUD, CONFORMITY, this document. README: what is built and what is planned, said as it is. Renamed: the capture program is gone (the node does its work, [INTAKE.md](INTAKE.md)); *bridge* and *capture program* left the code, the page and the documents, and the names an older installation still holds are migrated (the names kept on purpose above stay).
