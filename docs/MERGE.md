# Merging two memories

Status: planned (design). Nothing here is built yet, except `adopt`, which it extends.

## What it is for

Two memories exist: yours on the node that is always on, one on your laptop, one inherited from an older install.
You want one. The way a person merges two folders: for each space on one side, say which space on the
other side it joins, whether it is renamed, and what happens when the same note is on both sides.

## What exists today

`memcp adopt EXPORT --user NAME` takes the spaces of an export into a person. A space that already has
the name is refused. That is the only case it knows: every space is new.

## The merge, as the person sees it

1. **Choose the other memory**: an export folder, or a node you are connected to.
2. **A correspondence table**, one line per space of the other memory, with a default filled in:
   - same name and same kind: *merge into*;
   - same kind, other name: *merge into* the space of that kind, offered, never silent;
   - no match: *copy as a new space* (the name is kept, or changed in the line);
   - or *leave out*.
   Each line can be renamed. A space can be merged into a space of another name (a rename on the way).
3. **A preview**, nothing written: for each line, how many notes are new, how many are already there
   (same fingerprint), how many differ, how many files.
4. **What to do with conflicts**, per line or for all: *keep both* (default), *keep mine*, *take theirs*,
   *replace* (their version replaces mine, mine goes to history, never erased).
5. **A backup is taken first** and its place is shown. The merge is one operation: it finishes or it
   changes nothing.

## Rules (they follow the design of the rest)

- Nothing is deleted: a replaced fact is superseded; its old text stays in its history.
- A note is the same note when its fingerprint (text, tags, date) matches: it is not copied twice.
  Merging twice is harmless.
- Tags and the rules note (`rules`) of two spaces are merged as sets; the dashboard note of the target
  wins unless *replace* is chosen. A note of the target that gains tags is replaced by a copy of itself with
  every tag, as a note filed elsewhere is.
- Everything a merge writes is in the space's log: the other nodes of the person get the merged space as it
  is on this one.
- Files keep their fingerprints; a file already present is not copied again.
- Instructions (guides) are not merged: the target's stay, the other's are offered as a proposal.
- Connections never travel; they are made again where the memory now lives.
- The layout (folders) of the other memory is offered as a suggestion; the tree of the target wins.

## Steps

1. `merge --plan EXPORT` writes the table and the preview (JSON, and a page); no write.
2. The page lets the person edit the table; it produces a plan file.
3. `merge --apply PLAN` takes the backup, then applies, then verifies the counts and `doctor`.
4. The same engine is behind the page, the CLI and an MCP tool, so the apps do the same.

## Order of work

1. Engine in `pensine-core` (`merge.rs`): plan, preview, apply, with tests (same, renamed, conflicting,
   twice, interrupted).
2. CLI `merge`.
3. The page.
4. Merge between two live nodes (needs the swarm design in `FEDERATION.md`).
