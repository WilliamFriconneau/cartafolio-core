# Conformity of the Linux server app

What the documents require of the server side (the `memcp` binary, `memcp-manager`, and the Home Assistant app in `ha-addon/`), one line per requirement, and where it is checked. Scope: UI-SPEC.md, FEDERATION.md, CONCEPTS.md, DISCOVERY.md, TOOLS.md, SECURITY.md. The desktop and phone apps are out of scope, so the rows of UI-SPEC about them are not listed. `crates/server/tests/page/*.test.js` are optional browser tests (not run by `cargo test`).

Status: **done** (code and a test), **partial** (code, or part of the rule), **missing**, **n/a** (a plan the document itself marks as not built).

## UI-SPEC.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| UI-SPEC, top | The words *bridge*, *proxy*, *vault* appear on no screen | done | `crates/server/tests/conformity.rs::no_screen_holds_a_forbidden_word` (text and translations); `tests/page/onboarding.test.js` in a real browser; texts built by the page's scripts are not scanned (partial for those) |
| Principles 1, 3 | Machines first, with whether they are in step; a synchronisation in progress is visible | done | `crates/server/src/admin.html` (Machines), `core/tests/sync.rs::status_reports_direction_progress_and_goes_idle` |
| Principles 2 | After joining, a device shows one memory | partial | `server/src/replica.rs` (the replica is shown as the one memory); no page test |
| Principles 5 | Join a machine first, connect an assistant second | done | order of the screens in `admin.html` |
| Principles 6 | Each machine lists its own connections with last activity | done | `server/tests/admin.rs::each_person_sees_the_events_of_their_own_connections_and_an_administrator_all` |
| Principles 8 | A space with children is a folder; only leaf spaces are opened; labels, never technical names | partial | `admin.html` tree; `server/tests/explore.rs::the_layout_is_read_changed_and_a_space_is_made_with_a_label`; folder rule not tested on the page |
| Node page, sidebar | Machines (this one first), then the space tree with drag and drop | done | `admin.html` (`SYNC`, tree section); `explore.rs::a_tag_dragged_onto_a_sub_space_moves_its_notes_after_a_preview_and_files_there_from_now_on` |
| Node page, machine view | Name, address, version, peers seen, connections, live sync, a button to forget the machine | partial | `admin.html`; `client/tests/peer_sync_ways.rs::a_revoked_peer_is_refused_at_its_next_pass_and_gets_nothing` for the revocation behind the button |
| Node page, sync block | One line per peer: direction, percent, done/total, last activity; refreshed every 2 s; `GET /sync/status` is its only source | done | `admin.html` (`pollSync`, 2000 ms, `api("sync/status")`), `server/src/admin.rs` (`/sync/status`), `server/tests/admin.rs::the_page_reads_the_live_peers_and_the_nodes_found` |
| Node page, join banner | A machine that asks to join shows its name and three words; allow or refuse | done | `server/tests/join.rs::a_device_asks_the_person_compares_a_phrase_and_allows_and_the_key_comes_once`, `a_denied_request_is_told_to_the_device_and_nothing_is_made` |
| Node page, space view | Journal, Dashboard, Notes, Tree tabs; record a voice note, attach a file, write a note; a correction keeps the old text | done | `server/tests/explore.rs` (`a_note_is_listed_searched_corrected_and_removed_with_its_history`, `a_space_shows_the_notes_day_by_day_until_it_has_a_dashboard`, `an_attachment_and_a_voice_note_show_up_as_notes_with_their_file`) |
| Node page, settings | Language, public address, assistants and their rights, instructions, the janitor, export, merge two memories | done | `admin.html` language switch; `admin.rs::a_person_downloads_their_own_memory_and_nobody_elses`, `a_person_writes_their_own_guide_and_the_guides_of_their_spaces`; merge: `core/tests/merge.rs` (the page entry for merge is not tested) |
| Layout | Status dot per machine (green, amber with arrow and percent, grey offline, red refused) | partial | `admin.html` (`in step`, `catching up`, refused); no test of the colours |
| Layout | Footer always shows memory name, machine count, last sync time | missing | not found in `admin.html` |
| What each screen reads | Machines from `GET /sync/status`; space tree from MCP `spaces`; tags from `stats`; waiting notes from `triage_claim` / `triage_release` | done | `server/src/admin.rs`, `core/src/tools.rs`, `core/tests/triage_claim.rs` |
| What each screen reads | `rules_get` / `rules_set` read the `rules` note | done | `core/tests/rules_tools.rs` (the document still says *planned*) |
| What each screen reads | `links` and `library_search`, `library_raise` | done | `core/tests/library.rs` (the document says *planned*; the tools exist) |
| Acceptance 1 | Start a memory, join it from a second machine, write on one, see it on the other | done | `client/tests/peer_sync.rs::two_nodes_end_with_the_same_memory_and_a_second_pass_changes_nothing`, `server/tests/admin.rs::starting_a_memory_makes_the_default_spaces_once_and_names_the_person` |
| Acceptance 2 | During a copy the percentage rises and the direction is right on both machines | partial | `core/tests/sync.rs::status_reports_direction_progress_and_goes_idle` (one side), `client/tests/e2e.rs::a_new_node_catches_up_with_two_thousand_notes_in_pages` |
| Acceptance 3 | A machine switched off shows grey and catches up when it comes back | partial | catch-up: `client/tests/peer_sync_ways.rs::a_peer_that_goes_away_is_resumed_where_it_stopped_when_it_returns`; the grey state is not tested |
| Acceptance 5 | The same functions are reachable on the node page and on each desktop app | n/a | desktop apps are out of scope |

## FEDERATION.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| The idea | Nodes synchronise with each other; a node away for days catches up | done | `client/tests/e2e.rs::three_nodes_writing_everywhere_end_with_the_same_memory`, `peer_sync_ways.rs` |
| The idea | The same program does every step, no "simple" and "server" versions | done | one `memcp` binary (`crates/server`) |
| Facts are events | A fact is never overwritten; a space is an append-only log with stable ids and a hybrid logical clock | done | `core/tests/sync.rs::every_change_is_logged_with_a_stable_id_and_a_rising_clock`, `core/src/sync.rs` (`hlc`) |
| Rules table | A fact added on two nodes: both kept | done | `core/tests/sync.rs::the_same_text_written_on_two_nodes_is_kept_twice` |
| Rules table | The same fact replaced on two nodes: both kept, in dispute | done | `core/tests/sync.rs::the_same_fact_replaced_on_two_nodes_is_kept_both_ways_and_in_dispute`, `peer_sync_ways.rs::the_same_note_changed_on_both_sides_is_kept_both_ways_and_listed_as_a_dispute` |
| Rules table | Retired on one node, replaced on another: in dispute | done | `core/tests/sync.rs::retired_on_one_node_and_replaced_on_another_ends_alike_in_any_order` |
| Rules table | Layout: last writer wins per field by logical clock | done | `core/tests/sync.rs::layout_is_last_writer_wins_per_field` |
| Rules table | Keywords, tags and rules shared as versioned data, newest change wins | partial | rules travel as a note (`rules`); no per-rule id and no separate rule event |
| Rules table | A note's processed mark is a replicated event | partial | `core/src/triage.rs` and `triage_claim` hold a lease on a node; no replicated mark |
| Rules table | Guides: one writer at a time | partial | `server/tests/admin.rs::nobody_writes_the_guide_of_a_space_or_a_person_that_is_not_theirs`; no multi-node rule |
| Rules table | Files by hash; the same name with other content keeps both | done | `core/tests/sync.rs::files_travel_with_their_bytes_and_a_name_clash_keeps_both` |
| Rules table | Files fetched in chunks from any node that has them | missing | bytes travel with the events, not by chunk |
| Shape | Per space and per node, a policy: do not sync, send, receive, both; default *do not sync*; a sensitive space never leaves unless turned on | partial | a peer's rights are those of its connection (`core/tests/pairing.rs::a_peer_reads_and_writes_every_space_of_the_person`); no per-space, per-node direction setting |
| Shape | Identity: a key pair per node, confirmed by a short code on both screens | partial | a peer is a connection with a token; the three words of `/join` confirm the device (`server/tests/join.rs`); no node key pair |
| Shape | A lost node is revoked and the others stop sending it anything | done | `client/tests/peer_sync_ways.rs::a_revoked_peer_is_refused_at_its_next_pass_and_gets_nothing` |
| Shape | Encrypted transport between keys (iroh or another) | n/a | the transport is HTTP over TLS or the tailnet (TAILSCALE.md); the document keeps iroh as a candidate |
| Shape | Catching up keeps one point per peer per space and resumes | done | `peer_sync_ways.rs::a_peer_cut_short_in_the_middle_of_a_large_catch_up_resumes_without_a_gap_or_a_double` |
| Shape | The page lists the nodes with what each holds and when it last synchronised | partial | `GET /sync/status`, `admin.html`; "what each holds" is not shown |
| Processed notes | Claim before work: a lease of minutes; an expired claim is free again | done | `core/tests/triage_claim.rs::a_note_reserved_by_one_sorter_is_not_given_to_another_until_it_is_released`, `a_lease_that_has_ended_gives_the_note_to_the_next_sorter` (unit) |
| Processed notes | Filing keeps the note: `supersede` with the same text | done | `core/tests/inbox.rs`, `janitor/tests/janitor.rs` |
| Rules that improve themselves | Levels *Propose*, *Add*, *Full* per space; unique keywords and sensitive spaces always wait | partial | `guide_propose`, `server/tests/admin.rs::the_owner_approves_what_an_app_proposed_as_it_reads_and_nobody_else_does`; no three-level setting |
| Rules that improve themselves | Every assistant change is an event with the assistant's name and reason, revertible, limited per day | partial | history keeps name and reason; no daily limit |
| Window | A node may hold only a window; write first, prune later; nothing pruned before another node acknowledged | done | `core/tests/window.rs` (8 tests) |
| Window | `GET /sync/status` and `peers` carry `window` | done | `core/src/sync.rs` status, `core/tests/window.rs::the_window_is_off_by_default_and_checked` |
| Window | Write-only connectors | missing | no connector of this kind on the server |
| Window | Offline: the node answers from what it holds | done | `core/tests/window.rs::the_journal_and_a_board_still_work_on_the_window` |
| Security, identity | Ed25519 key pair per node in the system keystore; membership signed by administrator nodes; roles administrator, member, blind, read-only | missing | not built (peers are connections with tokens and read or write rights) |
| Security, joining | A one-use ten-minute code; a short check code both screens show | partial | `core/tests/pairing.rs::a_code_expires_and_cannot_reach_beyond_the_person`, `server/tests/join.rs`; the check phrase is derived from a random draw, not from the two keys |
| Security, revocation | Revoked node refused at its next contact | done | `peer_sync_ways.rs` (see above) |
| Security, events | Every event signed by its node; signature, author and form checked; failures counted against the sender | partial | form checked and refused: `core/tests/sync.rs::bad_events_are_refused_and_change_nothing`; no signature, no count |
| Security, events | Per-peer rate and size limits; quarantine after excess or invalid events | missing | only the guard on failed keys (`server/src/sync_http.rs`, 429) |
| Security, clock | An event too far ahead of the receiver's clock is refused | done | `core/src/sync.rs` (`MAX_DRIFT_MS`), `core/tests/sync.rs::bad_events_are_refused_and_change_nothing` |
| Security, replay | Receiving an event twice changes nothing | done | `core/tests/sync.rs::first_join_is_a_full_copy_and_a_second_catch_up_changes_nothing` |
| Security, rights | A peer's rights are those of its connection; a sensitive space only if turned on | done | `core/tests/pairing.rs`, `core/tests/permissions_matrix.rs` |
| Security, audit | The audit log records who joined, who was revoked, what was refused, and the page shows it | partial | `auth_audit` (`core/src/access.rs`) and the activity list of the page; quarantine is absent |
| Managing | Join, revoke, pause a node; change a role; choose what each node holds | partial | join and revoke done (`server/tests/join.rs`, `peer_sync_ways.rs`); no pause, no roles, no per-node choice |
| Managing | A node gone for long is flagged after a delay; admin rights can move | partial | last activity shown; no delay, and the administrator is a person, not a node |
| Managing | Backups are nodes; restoring is joining a new node | done | `core/tests/backup.rs`, `client/tests/e2e.rs::a_new_node_catches_up_with_two_thousand_notes_in_pages` |
| Managing | Splits keep working and merge when they meet; nothing blocks writing | done | `client/tests/e2e.rs::many_writers_at_once_over_http_lose_nothing`, `peer_sync_ways.rs::writes_on_both_nodes_meet_and_a_pass_with_nothing_new_changes_nothing` |
| Conflicts | A note corrected on two nodes shows *in dispute* with both versions; the person picks; resolving is an event seen on the others | partial | disputes listed: `core/src/sync.rs::disputes`; resolving on the page is not found |
| Conflicts | A retired and a corrected note: in dispute | done | `peer_sync_ways.rs::a_note_retired_on_one_side_and_corrected_on_the_other_is_in_dispute_not_lost` |
| Conflicts | Same file name, other content: both kept, the second named with its node and time | done | `core/src/sync.rs::disputed_name`, `core/tests/sync.rs::files_travel_with_their_bytes_and_a_name_clash_keeps_both` |
| Conflicts | A move that would break the tree: the later one is refused and reported | partial | `core/src/moves.rs`, `core/tests/moves.rs`; not tested across nodes |
| Conflicts | Nothing is overwritten and nothing erased by a merge; a dispute never blocks reading or writing; an assistant never picks | done | `core/tests/merge.rs`, `core/tests/round_trip.rs` |
| Conflicts | A dispute count appears on the page | missing | no count in `admin.html` |
| Tests that decide | Three nodes, one off, every order: the same memory | done | `client/tests/e2e.rs::three_nodes_writing_everywhere_end_with_the_same_memory` |
| Tests that decide | A hostile node (bad signature, size, space, future clock, replay) changes nothing but its quarantine | partial | `core/tests/sync.rs::bad_events_are_refused_and_change_nothing`, `client/tests/e2e.rs::the_doors_stay_shut`; signature and quarantine absent |
| Tests that decide | A join code works once and expires; a wrong check code fails | partial | once and expiry: `core/tests/pairing.rs`; no check-code mismatch test (nothing derives it from keys) |

## CONCEPTS.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| A trace | Never overwritten: a correction replaces it and keeps the old one | done | `core/tests/facts.rs`, `server/tests/explore.rs::a_note_is_listed_searched_corrected_and_removed_with_its_history` |
| A trace | Context of a trace: a place and the day's appointments, kept with it | missing | no calendar read on the server |
| A space | A space is a boundary of rights: an assistant opened on Work cannot read Health | done | `server/tests/server.rs::isolation_between_agents_through_the_protocol`, `core/tests/permissions_matrix.rs` |
| A space | Spaces nest as folders | done | `core/src/layout.rs`, `core/tests/layout.rs` |
| Sorting | A word that belongs to one space files the note there at once; ordinary words score; nothing decisive stays in À trier; deterministic | done | `crates/janitor/src/router.rs` tests (`a_unique_keyword_routes_at_once`), `crates/spaces/tests/router.rs` |
| Sorting | Voice commands are keywords too (idea box, what's next, targets) | partial | `crates/spaces/src/defaults.rs`; not run on the server's path |
| The janitor | Proposes keywords from what stays in À trier and what the person refiled; reruns the router | done | `crates/janitor/tests/janitor.rs`, `janitor/src/learn.rs` |
| Living boards | Four boards per space; cards added and moved one by one; each keeps its history | done | `core/tests/living.rs`, `server/tests/explore.rs::a_stored_dashboard_counts_the_signs_and_lists_the_targets` |
| Living boards | Named lists (`list-shopping`...), open then done, each card with its source | partial | `living_get` pages; named lists are not covered by a test |
| Living boards | Each board remembers its last pass and shows its delta | done | `core/src/tools.rs` (`living-meta/<page>`), `core/tests/living.rs` |
| Living boards | Targets group signs and symptoms, counted day by day on the dashboards | done | `server/tests/explore.rs::a_stored_dashboard_counts_the_signs_and_lists_the_targets` |
| 5S of a memory | À trier empties; what is wrong is retired and kept in history | done | `core/tests/inbox.rs`, `core/tests/triage_audio.rs` |
| Peers and versions | Machines of different versions work together on one protocol; an older one that meets a newer protocol stops and says *update this machine* | done | `client/tests/peer_sync_ways.rs::a_peer_with_a_newer_protocol_is_refused_with_a_clear_message_and_nothing_is_touched`, `a_peer_with_an_older_protocol_or_none_is_still_served_as_the_first_one` |
| Peers and versions | Updates start with the Home Assistant node | n/a | an order of operation for the person, not a rule of the program |
| Local first | A remote service is a choice, never a default | done | `crates/janitor/src/models.rs` (no model unless named), `ha-addon/memcp/run.sh` (`transcribe` off by default) |
| From a note to the calendar | The memory never pushes into a calendar | done | no calendar code on the server |

## DISCOVERY.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| Finding | A node that listens beyond the loopback advertises itself (mDNS `_memcp._tcp`) with name and version | done | `crates/server/src/discovery.rs::advertise`, `server/src/main.rs` (`--advertise --node-name`) |
| Finding | `GET /.well-known/memcp` answers the same, so a find can be checked | done | `crates/server/tests/conformity.rs::discovery_tells_the_service_the_name_and_the_version_and_nothing_else` |
| Finding | In a Home Assistant app, no advertising (the host network would be needed); found by `homeassistant.local`, `memcp.local` or the tailnet | done | `ha-addon/memcp/config.yaml` (no `host_network`), `crates/capture/src/main.rs` (`discover`) |
| Finding | Advertising says nothing of what a node holds and gives no access | done | `conformity.rs` (the answer holds four keys) |
| Joining 2 | The node shows the request on its page with three words; the device shows the same | done | `server/tests/join.rs` (`check.split('-').count() == 3`) |
| Joining 3 | The person presses Allow; nothing is typed | done | `server/tests/join.rs` |
| Joining 4 | The device gets its own connection once; À trier only until the person says otherwise | done | `server/tests/join.rs::a_device_asks_the_person_compares_a_phrase_and_allows_and_the_key_comes_once`, `server/tests/join.rs::a_device_the_person_allowed_in_advance_joins_at_once_and_only_once` |
| Joining | A request lives two minutes | done | `conformity.rs::a_join_request_lives_two_minutes` (`join.rs::TTL`) |
| Joining | A few requests a minute per address and a few waiting at once; a wrong secret learns nothing | done | `server/tests/join.rs::requests_are_limited_per_address_and_in_all`, `only_the_device_that_asked_can_poll_and_only_a_signed_in_person_can_answer` |
| Joining | A QR code on the page carries the address and a one-time code | done | `server/tests/explore.rs::joining_a_network_gives_a_link_and_a_qr_code_to_scan` |
| Not | A node never lets a device in by itself | done | `server/tests/join.rs` (nothing is made until the person answers) |

## TOOLS.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| The tools | `spaces`, `triage_claim`, `triage_release`, `rules_get`, `rules_set`, `peers`, `vocabulary_terms` are built | done | `core/src/tools.rs`, `core/tests/rules_tools.rs`, `core/tests/vocabulary.rs`, `core/tests/triage_claim.rs` |
| The tools | `library_search`, `library_raise`, `links` | done | `core/tests/library.rs` |
| The tools | `tree`, `read`, `files` (with `op`), `dashboard`, `rules`, `finances` as single tools | missing | the separate tools (`stats`, `read_fact`, `history`, `file_*`, `living_get`/`living_set`, `finances_*`) are served instead; no alias list in `initialize` |
| The tools | `recall` takes a path of the tree as `space` | partial | `core/tests/paths.rs`; not checked for the tree path form |
| Rights | A grant is a space or a path and a level read, write, *propose*, *apply*; write includes read | partial | read and write and path grants: `core/tests/paths.rs`, `core/tests/access.rs`; no *propose* or *apply* levels |
| Rights | A grant *with its sub-spaces* covers non-sensitive descendants only | missing | no such option in `core/src/access.rs` |
| Rights | A key may expire; the page shows the last use of each connection | done | `core/src/access.rs` (`expires_at`), `server/tests/admin.rs::each_person_sees_the_events_of_their_own_connections_and_an_administrator_all` |
| Rights | A new connection is made from a template that proposes rights | partial | the wizard of `admin.html` (platforms) proposes rights; templates by kind of assistant are not stored |
| Rights | The address of a connection stays its name | done | `server/tests/server.rs::the_address_of_a_connection_is_its_name_and_lets_in_only_its_own_key` |
| Existing connections | Migrated, not deleted; one that cannot be mapped is listed under "Needs a look" | partial | `core/tests/users.rs`, `core/tests/catalogue.rs` (upgrades keep connections); `admin.html` "Needs a look" lists what deserves one: `admin.rs::only_what_deserves_a_look_is_listed_and_refused_keys_are_for_administrators` |
| Libraries | Mounted read-only; originals never changed; hidden folders and symbolic links left alone | done | `core/tests/library.rs` |
| Libraries | Indexes path, title, dates, the first MiB of `.md` and `.txt`, `[[links]]`; `.docx` and `.pdf` by name only | done | `core/tests/library.rs` |
| Libraries | The janitor never reaches the index; a fact tagged `library` is left alone | done | `crates/janitor/tests/janitor.rs`, `core/src/library.rs` |
| `library_search` | Offered only to a connection that reads a space whole | done | `core/tests/library.rs` |
| `library_raise` | Summary at most 1200 characters; writes a fact with `library/raised`; needs write rights | done | `core/tests/library.rs::links_draws_spaces_tags_library_folders_and_what_was_raised` |
| `links` | Nodes and edges as in the table; limited to spaces read whole | done | `core/tests/library.rs` |

## SECURITY.md

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| What it does | Tokens are shown once; only their digest is stored | done | `conformity.rs::a_token_is_kept_only_as_a_digest` |
| What it does | Failed attempts are rate limited per address | done | `server/tests/server.rs::too_many_refusals_from_one_address_block_even_the_right_token` |
| What it does | The server listens on the loopback unless `--allow-remote` | done | `server/tests/server.rs::refuses_to_listen_on_a_non_local_address_without_consent` |
| What it does | Rights are explicit and never more than the person's; administrators do not read content on the admin page | done | `server/tests/admin.rs::the_page_shows_counters_for_each_space_and_never_the_content`, `a_member_cannot_open_a_space_they_do_not_reach` |
| What it does | What comes back to an agent is flagged as data | done | `server/tests/server.rs::write_search_and_replace_through_the_protocol` (the `notice`) |
| What it does | Files typed by content, stored once by SHA-256, served with `sandbox` and `nosniff` | done | `server/src/admin.rs` (headers), `server/tests/server.rs::a_kept_photo_comes_back_as_an_image`, `core/tests/files.rs` |
| What it does | The server proxies nothing | done | no outbound call on behalf of an assistant in `crates/server` |
| What it does | The admin page is not on the network listener; not through Funnel; failed console tokens counted | done | `server/tests/admin.rs::the_admin_is_not_served_on_the_main_listener_unless_enabled`, `the_admin_is_never_served_through_funnel`, `failed_console_tokens_are_counted_and_then_blocked`, `ingress_headers_are_ignored_on_the_normal_listener` |
| What it does | The person's guide is written only on the admin page; a space's guide by a connection that writes it whole; each write audited | done | `server/tests/admin.rs::a_person_writes_their_own_guide_and_the_guides_of_their_spaces`, `core/tests/guide.rs`, `core/tests/app_guides.rs` |
| What it does | The index is told as data with the section marker broken inside it | done | `core/tests/guide.rs` |
| What it does | A connection reaches only what was chosen; OAuth consent grants nothing unticked; `/mcp/<space>` gets that space only; a new sign-in waits for an administrator | done | `server/tests/oauth.rs` (consent), `server/tests/server.rs::a_key_used_at_the_address_of_a_space_reaches_that_space_and_no_other`, `manager/tests/manager.rs::a_second_login_waits_and_the_administrator_lets_it_in` |
| What it does | `export` has a fingerprint of every file and no key, connection or right; `adopt` checks everything first and undoes itself | done | `core/tests/export.rs::an_export_holds_only_what_the_person_owns_and_no_key`, `an_altered_export_is_caught_and_nothing_is_adopted` |
| Still to get right | The identity listener trusts `Tailscale-User-Login` only from the loopback or `--identity-trust` | done | `manager/tests/manager.rs::claude_on_the_web_signs_in_as_the_person_the_tailnet_says_it_is`, `an_unknown_tailnet_login_is_given_nothing` |
| Still to get right | A key for a local assistant lives in the environment of the program that runs it | done | `server/tests/server.rs::the_stdio_server_speaks_mcp_with_the_rights_of_its_key_and_nothing_else_on_stdout`, `the_stdio_server_stops_when_its_key_is_revoked_and_refuses_to_start_without_one` |
| Still to get right | No key is written in logs, audit or answers | done | `server/tests/server.rs::no_key_is_written_in_the_logs_the_audit_or_the_answers` |
| Still to get right | The memory is not encrypted at rest | n/a | stated as not built; see ENCRYPTION.md |
| Reporting | A vulnerability is reported by GitHub private reporting | done | `SECURITY.md` itself |

## The Home Assistant add-on and the manager

The documents above say little of the add-on directly; what they require of it is covered here.

| Doc and section | Requirement | Status | Test or code |
|---|---|---|---|
| SECURITY, admin page | Served on the Home Assistant ingress | done | `ha-addon/memcp/config.yaml` (`ingress: true`), `server/tests/admin.rs` |
| DISCOVERY, Finding | No host network in the app | done | `ha-addon/memcp/config.yaml` |
| UI-SPEC / CONCEPTS | One memory per person, a new Home Assistant account waits or gets its own | done | `manager/tests/manager.rs::by_default_every_new_home_assistant_account_gets_a_memory_of_its_own`, `a_second_login_waits_and_the_administrator_lets_it_in` |
| SECURITY, leaving | The memory can be backed up and exported | done | `config.yaml` (`backup: cold`), `core/tests/backup.rs` |
| FEDERATION, joining | A device pairs as a peer of its person and never reaches another | done | `manager/tests/manager.rs::a_device_pairs_as_a_peer_of_its_person_and_never_reaches_another` |
| Manager | `GET /health` answers | done | `crates/manager/src/manager.rs` |
| The add-on starts | `run.sh` builds the settings from the options and starts the node and the manager | partial | `ha-addon/memcp/run.sh`; shell, no automated test |

## Gaps left, by importance

Small gaps (under about 50 lines) were closed with `crates/server/tests/conformity.rs`: discovery content, the join lifetime, the token digest, the forbidden words. The footer of the page was left out: it needs the page's data and a browser test, more than a few lines. What remains:

1. **Signed events, node keys and membership** (FEDERATION, *Security of the swarm*): peers are connections with tokens, events carry no signature, there are no roles other than a connection's rights. This is the largest gap and the one that matters for a swarm with machines the person does not fully trust.
2. **Per-peer limits and quarantine** (FEDERATION): a hostile peer is refused event by event but never put aside; only failed keys are rate limited.
3. **Per-space, per-node policy** (send, receive, both, do not sync) and the page controls to choose what each node holds, pause, and change a role.
4. **Resolving a dispute on the page** and the dispute count (FEDERATION, *Conflicts*): disputes are kept and listed in the log, the person's choice is not offered.
5. **Rights model of TOOLS.md**: the *propose* and *apply* levels, the *with sub-spaces* option, templates per kind of assistant, and the merged tools (`tree`, `read`, `files`, `dashboard`, `rules`) with the aliases and the deprecation list in `initialize`.
6. **Assistant levels for rules** (*Propose*, *Add*, *Full*) with a daily limit on changes.
7. **Processed mark as a replicated event** and per-rule ids, so that several janitors on several nodes never file the same note twice.
8. **Footer of the node page** (memory name, machine count, last sync time) and the grey and red machine states under a test.
9. **Files in chunks from any node** and **write-only connectors** (FEDERATION, *Window*).
10. **Context of a trace** (place, appointments of the day) (CONCEPTS).
