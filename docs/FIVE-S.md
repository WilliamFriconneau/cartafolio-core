# 5S of the code

The method of [CONCEPTS.md](CONCEPTS.md) applied to this repository. Rerun it before a release.

| S | In the code | How to check |
|---|---|---|
| Sort (*seiri*) | remove what nothing uses: functions, constants, types, dependencies, features, scripts, documents | `scripts/dead.sh` |
| Set in order (*seiton*) | one place for each thing: one cleaner for a keyword or a tag list (`memcp_core::facts::clean_keyword`, `clean_rules_list`), one folding function (`memcp_core::text::fold`), each document linked from [README.md](README.md) | `scripts/dead.sh` (last two sections), review |
| Shine (*seiso*) | no warning, formatted, no copy of a helper | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` |
| Standardise (*seiketsu*) | the same words everywhere: *space* (never *vault*), *capture program* (never *bridge*), *peer* or *node* (never *proxy*); the same checks for everyone | `scripts/check.sh`, the forbidden-word test of the page (`crates/server/tests/page/onboarding.test.js`) |
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

- `vaults` in the pairing answer (`POST /pair`, `memcp_client::Pairing`, `memcp-capture-ffi::Pairing`), in the export manifest and its `vaults/` folder, in the `stats` tool answer and in the administrator page's data; `inbox_vaults` (janitor setting); `vault` (outbox entries and the old settings key of the capture program, accepted and ignored); `memcp vault add` (alias of `memcp space new`); the `--vault` flag (refused with a message).
- `bridge` in `bridge.conf`, `bridge.sock`, `bridge.key`, the `bridge/` folder of the installers, the `bridge_pairings` table, the `MEMCP_BRIDGE_BIN` and `MEMCP_BRIDGE_PORT` variables, `--bridge-port`, the `desktop-bridge` connection and the `bridge-` prefix of connection names, the task and plist labels, and `/mcp/bridge` in tests. A later change may rename them together with a migration.
- Features `local-whisper` and `metal` of `memcp-capture` and `sync` of `memcp-client` are enabled by the Mac app build, the release workflow or `ha-addon/build.sh`, not by the default build.
- `crates/capture-ffi`: every public item is the Swift app's.

## Last pass

17 November 2026. Removed: the Hermes plugin (an assistant connects through the MCP, one way in for all), the handover notes that had no place in a public repository. Linked from the index: CRUD, CONFORMITY, this document. README: what is built and what is planned, said as it is. Renamed: *bridge* and *vault* left in the capture program's comments, messages, help and tests became *capture program* and *space* (the names kept on purpose above stay).
