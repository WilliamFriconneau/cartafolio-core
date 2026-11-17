# Core audit, 17 November 2026

An adversarial review of the core, done with an AI assistant (not an independent party), the day after the review of 16 November. Each finding has a test written before the fix where a test was possible. Tests are in `crates/server/tests/audit.rs`, `crates/server/tests/admin.rs`, `crates/core/tests/audit.rs` and `crates/core/src/migration_tests.rs`.

## Scope

`crates/core`, `crates/server`, `crates/manager`, the sync half of `crates/client`, and the places of `crates/capture` that write files. Read in full or in part: access, tools, sync, files, export, merge, import, library, brain and schema (core); app, net, http, admin, join, pair, oauth, sync_http, replica, tar (server); peer_sync (client).

## Method

Reading the routes and the tools for who can call what with which key; hostile-input tests at the unit and the socket level; greps for `unsafe`, `unwrap`/`expect`/`panic!` outside tests, string slicing, every `fs::` write and every SQL built with `format!`; `cargo audit` and `cargo deny check` (installed for this); `cargo fmt`, `clippy --all-targets`, the full test suite.

## Findings

| # | Severity | Finding | State | Test |
|---|---|---|---|---|
| 1 | High | A connection limited to tag paths (an app depositing under `sources/x`, a reader of one subject) could read the whole log of the vault and write events with any tag through `/mcp/<name>/sync/*`. | Fixed: sync needs a vault reached whole. | `a_connection_limited_to_paths_cannot_use_the_sync_routes` |
| 2 | High | Any member (not only an administrator) could make the node join another memory (`/admin/api/join-memory`), start `memcp-sync` processes without a cap, and so pull foreign events into spaces of the same name. | Fixed: administrators only; at most 16 programs at once. | `a_member_cannot_make_the_node_join_another_memory` |
| 3 | Medium | Body limit for a request with a valid key was 1 GiB (the comment said 34 MB): 64 requests in flight could ask for 64 GiB of memory. | Fixed: 34 MiB. | `a_request_may_not_make_the_server_buffer_a_gigabyte` |
| 4 | Medium | Key oracle: a wrong key with a big declared body got `413` while a right one was processed, and the refusal was not counted, so the failure guard never fired. | Fixed: a refused oversize body counts as a failed attempt (engine and manager). | `guessing_a_key_by_the_size_of_a_body_is_counted_like_any_wrong_key` |
| 5 | Medium | A peer with write access could store a file with any name (control characters, paths) and any claimed type; both reach the `Content-Type` and `Content-Disposition` headers of the page (header injection), and bytes of a kind that is never kept (executables). Text, tags, author and origin of an event were unbounded. | Fixed: names cleaned, type taken from the bytes, unsupported kinds refused, shapes and sizes bounded (1 MiB text, 64 tags). The header writer also drops any header with a control character. | `a_peer_cannot_store_a_file_under_a_hostile_name_or_type`, `a_peer_cannot_send_a_text_or_a_tag_list_without_limit`, `a_peer_cannot_forge_an_origin_or_a_clock_stamp_with_odd_characters`, `a_header_value_cannot_carry_a_line_break` |
| 6 | Medium | Databases opened with the schema trusted: views and triggers of a hostile database (adopt, merge, restored backup) could call unsafe SQL functions. | Fixed: `PRAGMA trusted_schema = OFF` on every connection. | `a_hostile_schema_cannot_call_unsafe_functions_when_a_database_is_opened` |
| 7 | Medium | The failure guard kept every failure for 15 minutes with no bound (memory and a linear scan per request), and an IPv6 client changed address at every guess. | Fixed: at most 5000 held, an IPv6 address counts as its /64. | `the_failure_guard_holds_an_ipv6_network_and_cannot_grow_without_bound` |
| 8 | Low | `/oauth/token` was not limited or counted. | Fixed (codes have 192 bits, so this is hygiene). | `guessing_oauth_codes_is_counted_and_held_off` |
| 9 | Low | The check phrase of a join drew from about 26 of its 64 words (14 bits instead of 18). | Fixed. | `the_check_phrase_uses_every_word_of_its_list` |
| 10 | Low | An export with a symbolic link passed `verify-export` when the fingerprint matched. | Fixed: links refused. | `an_export_that_holds_a_link_is_not_verified` |
| 11 | Low | `merge` used the `sha256` of the other memory as a file name before checking it; `/dev/zero` as a fingerprint would have been read to the end. | Fixed: must be 64 hexadecimal digits. | covered by the digest check (no fixture of a hostile merge) |
| 12 | Low | Unbounded number of tags per note and of terms in a search (an OR of thousands of terms). | Fixed: 64 and 64. | `tags_are_bounded_in_number_and_a_search_in_terms` |
| 13 | Medium | No read deadline on a connection (finding 2 of the 16 November review). | Open: needs a socket timeout the HTTP library does not expose. Use a proxy with timeouts. | |
| 14 | Medium | Sync events are not signed. `origin` is a claim, checked only for shape. A peer holding a key with write on a space can write anything in it, and set the layout (parent, label) of that space. | Open, by design until FEDERATION.md signing is built. | |
| 15 | Medium (design) | A device the person allowed in advance (`allow-device`) joins at once, once, on its **name** alone, which is not secret. For at most the minutes chosen (default 30, at most 1440). | Fixed: `allow-device` answers a one-time ticket (at most ten minutes) shown to the person; the device gives it with its request; the name alone lets nothing in. | `a_device_the_person_allowed_in_advance_joins_with_the_ticket_only_and_only_once` |
| 16 | Low | `/join` is reachable by anyone who reaches the node and 8 waiting requests lock out real ones for two minutes; the device name is shown as given. | Fixed: at most 3 requests waiting from one address (an IPv6 /64), 64 in all; a request with a ticket is not counted. | `requests_are_limited_per_address_and_in_all` |
| 17 | Low | `oauth_codes` and revoked `oauth_refresh` rows are never pruned. | Open (slow growth, behind a consent). | |
| 18 | Low | The text of a fact has no hard ceiling for a local writer (`max_text` defaults to unlimited); only the 34 MiB body bounds it. | Open. | |
| 19 | Low | Any signed-in person sees `/admin/api/sync/status` (peer names and progress). | Open. | |

## What holds

No `unsafe` and no `unwrap`/`expect`/`panic!` outside tests; slicing sites checked (one in `merge` fixed through finding 11). Every SQL value is a bound parameter, `format!` only inserts constant column lists and table names. FTS queries are built from stems quoted one by one, so a search cannot carry FTS syntax. The key in the address is not logged anywhere (nothing logs requests), is not in the audit, the errors or the metadata; a key at the wrong address gets the same answer for an unknown and a forbidden vault. Join secrets are compared in constant time, polled once, and expire; consents and codes are single use. Redirects of OAuth are an allow list. Library and import walkers do not follow links. The tar writer only writes. Files written by the node are 0600 in 0700 folders (a few are written then restricted, inside a private folder). `cargo audit`: no vulnerability (one allowed, unmaintained, build-time, RUSTSEC-2026-0374). `cargo deny check`: advisories, bans, licences, sources ok.

## Not covered

`explore.rs` and the JavaScript of the admin page line by line; `capture` (daemon, whisper, browser) beyond its file writes; `janitor`; `finances` parsers (spreadsheet and zip bombs); the Home Assistant add-on scripts; `import.rs` heuristics against pathological input (time); `manager` routing beyond the body limit; timing measurements; fuzzing; the `LEASES` map and the `joins` and `allowed_devices` maps were read for growth (bounded) but not load tested.
