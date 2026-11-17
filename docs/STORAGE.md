# How the data is stored

The SQL (`crates/core/src/schema.rs`) is the source of truth; this is the map.

- One SQLite file per space, plus `control.sqlite` (connections, grants,
  OAuth). The version is `PRAGMA user_version`; migrations only go forward.
- `facts`: short, dated, sourced facts. Correcting one replaces it and keeps the
  old version (`status`: proposed, active, replaced, retired); `fact_events` is
  append-only (triggers refuse an update or a delete). Search is FTS5.
- `files`: bytes on disk under `files/<space>/<sha256>`, stored once; a file held in
  several spaces is a hard link, so each space keeps its own path.

## What is held twice

Stored once: facts (exact text, per space), near-duplicates (they wait for a
decision), imported notes, bank movements, the same file. Kept
apart on purpose: one fact in two spaces, because a space is the unit of rights.


## Tidying at each write

`remember` answers with a `tidy` block when there is something to look at: facts
that say nearly the same thing, facts past their date and still active, the same
fact in another space the caller can read, a text too long. It is computed, never
applied: the agent or the person decides with `supersede` or `retire`. The memory
calls no model for it, and a space the caller cannot read is never mentioned.

Some tables are no longer used (`devices`, `device_*`, `outbox`, `record`, `embeddings`, `groups`, `group_*`, `pairing_codes`, the `upstream` columns and the per-connection `writes_per_day`): the server no longer reads or writes them. They stay because they may hold data (an old
health vault holds what the HealthApp app pushed before 0.12), and they go at a clean break that exports it first. Nothing is
lost by leaving them.

An app's guide (`app_guides`, one per connection and vault) is kept apart from the facts: no search finds it, it sits in
no layer. What the person approved of it is in `control.sqlite` (`guide_approvals`, by digest of the text).
