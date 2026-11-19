# The space view contract (`memcp.view/1`)

Status: built in the core, served by the node, and used by the node's web cockpit on Windows, Linux and Home
Assistant. The Apple apps and the assistants read the same contract over MCP.

A space offers four main views, and its **Targets are its home page**:

| View | What it gives |
|---|---|
| `targets` | the space's targets, with a deadline, a target amount, a realized amount and the progress; in Home and Hobbies, the checkable tasks |
| `chronology` | one filterable chronology of notes and structured entries (signs, measures, movements), newest first, with their time and provenance |
| `stats` | totals by type, days, tags and sources, with targets and tasks |
| `files` | the inventory of files still kept, with the note that describes them |

`dashboard` and `register` stay callable for older clients but are **not advertised**: the web has no Dashboard
or Register page in duplicate.

- **Name and version**: `memcp.view/1` (`memcp_core::view::CONTRACT`, `CONTRACT_VERSION`). Additive fields keep the
  version; a removed or renamed field is a new contract.
- **Discovery**: MCP `space_view {space, view: "capabilities"}`. The node also advertises `viewContract` in its MCP
  `serverInfo` and `view_contract` in `GET /.well-known/memcp`.
- **Node page**: `GET /admin/api/space/view?space=NAME&view=…` answers the same shape for the signed-in person.
- **Rights**: the contract reads only a space the caller reads **whole**. A key limited to paths does not see
  `space_view` (`unknown tool`). A space the caller does not read answers `denied`, in the same words as a space
  that does not exist. The check is in the core, before any view is computed.

## The answer

Every view carries:

| Field | Meaning |
|---|---|
| `contract`, `contract_version` | `memcp.view/1`, `1` |
| `space`, `kind`, `label` | the space, its kind and its label |
| `view` | `capabilities`, `targets`, `chronology`, `stats`, `files`, `tasks` (legacy: `dashboard`, `register`) |
| `home` | `targets` in capabilities: what a client opens first |
| `generated_at`, `notice` | when it was computed, and the memory-data notice |
| `views`, `item_types`, `schemas`, `tools` | capabilities only: what exists, typed shapes, and the tools the caller may actually call |
| `items`, `page` | targets, chronology, files: the data and its cursor |
| `stats` | stats: the indicators |
| `provenance` | where the data comes from |
| `extensions` | a money space: the finance tools and the `movement` view |

An **item** has a stable `id` (the fact id for notes and cards, the file id for the inventory, `mv_…` for a
movement), a `type`, `at`, `created_at` when it has one, `text`/`extract`, `tags`, `fields`, `source`, `author`
and `provenance`. A health sign adds `sign`, `level` and its referential `label`; a measure adds `measure`,
`value`, `unit` and `label`; a target adds `column`, `signs` and `metrics`; a task adds `done`; a movement adds
`amount`, `account` and `date`.

`metrics` is `{due, target_amount, realized_amount, progress_percent, currency}`, each nullable. It is read from
the target's fields (`due`, `target_amount`, `realized_amount`, `currency`, with French aliases). A financial goal
sets them with `living_set`:

```json
{ "name": "living_set", "arguments": {
    "space": "finances", "page": "targets",
    "add": [{ "text": "Holiday fund", "column": "open",
              "fields": { "due": "2027-07-01", "target_amount": 2000, "realized_amount": 450, "currency": "EUR" } }] } }
```

## The chronology

`space_view {space, view: "chronology", from?, to?, type?, tag?, q?, limit?, cursor?}`

- `from`, `to`: `YYYY-MM-DD` (the whole day for `to`) or ISO 8601. Defaults: the beginning of time and now.
- `type`: `all` (default), `note`, `file`, `sign`, `measure`, `movement`.
- `tag`: a tag path; `q`: words in the entry.
- `limit`: 1 to 200, default 50; `cursor`: opaque, take `page.next_cursor`.
- Pages are newest first. A money space merges the ledger's movements into the same chronology; a node without
  the ledger leaves them out, never as an error.

The web cockpit has no separate Journal and Register: the **Journal** view is this chronology, with filters for
type, words, tag and date.

## The other views

- `stats {from?, to?}`: `total`, `by_type`, `by_day`, `by_tag`, `sources`, `targets` (`open`/`reached`), `tasks`
  (`open`/`done`), `first_at`, `last_at`.
- `files {from?, to?, q?, mime?, limit?, cursor?}`: active files, newest first, with `name`, `mime`, `size`, the
  note that describes it, its provenance and a cursor. `file_get` gives the bytes when the rights allow it.
- `tasks {space}`: the cards of the living board `next`; `done` is true for the column `done`. Check one with
  `living_set`:

```json
{ "name": "living_set", "arguments": {
    "space": "home", "page": "next", "move": [{ "id": "f_…", "column": "done" }] } }
```

## Reading examples, for an LLM

1. *« What can I read in the health space? »*
   ```json
   { "name": "space_view", "arguments": { "space": "health", "view": "capabilities" } }
   ```
   Read `home` (targets), `views`, `schemas` and `tools`; do not invent a view.

2. *« What did we note last week? »*
   ```json
   { "name": "space_view", "arguments": {
       "space": "work", "view": "chronology",
       "from": "2026-11-10", "to": "2026-11-16", "limit": 50 } }
   ```
   Pass `page.next_cursor` back as `cursor` until `has_more` is false. Cite each item's `id` and `provenance`.

3. *« What is my migraine target following, and what did I note for it? »*
   ```json
   { "name": "space_view", "arguments": { "space": "health", "view": "targets" } }
   ```
   ```json
   { "name": "space_view", "arguments": {
       "space": "health", "view": "chronology", "type": "sign", "tag": "signe/cephalee" } }
   ```

4. *« What are my last bank movements? »*
   ```json
   { "name": "space_view", "arguments": { "space": "finances", "view": "capabilities" } }
   ```
   ```json
   { "name": "space_view", "arguments": {
       "space": "finances", "view": "chronology", "type": "movement",
       "from": "2026-09-01", "to": "2026-09-30", "limit": 20 } }
   ```

5. *« What can I tick at home? »*
   ```json
   { "name": "space_view", "arguments": { "space": "home", "view": "tasks" } }
   ```

## Typed schemas and tools

`capabilities.schemas` gives the envelope, the pagination, the provenance, the item types (`note`, `file`,
`target`, `task`, `sign`, `measure`, `movement`) and `target_metrics`, plus the action list:

- `write_note` → `remember`; `add_file` → `file_put`; `add_target` and `move_target` → `living_set`;
  `check_task` → `living_set`.

`capabilities.tools` lists the tool names this connection may actually call, in the order the node offers them.
A read-only key sees no write tool, and a key limited to paths does not see the whole-space contract at all.

## Fictitious fixtures

Three examples live in `crates/core/tests/fixtures/view/` and are checked by a test:

- `generic.json`: a work space, a note and a file, a cursor.
- `health.json`: a health space, a target, a sign (`signe/cephalee`, level `6/10`) and a measure
  (`mesure/tension`, `12/8`), each with its referential label.
- `finances.json`: a money space, the `finances` extension and two `movement` items.

Names, accounts and amounts are fictitious.

## Compatibility

- The existing tools keep their names, arguments and answers: `living_get`, `living_set`, `recall`, `read_fact`,
  `finances_*`, the page's `/space`, `/facts`, `/living` routes. `living_set` gains the optional `fields` of a card.
- The page's `/space?space=…` still carries `dashboard`; a section has an `id` (`section:<n>`), which an older
  renderer ignores.
- The node's peer protocol does not change. `space_view` is an MCP tool, not a sync event.
- An older node answers `unknown tool` to `space_view`; a client falls back to the old tools until it updates.

## What remains

The Apple apps (iOS, macOS) and the native Windows/Linux clients still have to read the contract for their own
renderers; the node's web cockpit already does. The visual work is done first on the web; the same four views and
the + button are the reference for the native surfaces.
