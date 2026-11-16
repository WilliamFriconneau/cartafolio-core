# Living pages: what's next, ideas, targets, where things stand

Status: specification, the default for every space. The notes are the record; the living pages are what a person (and an
assistant) reads first. They are kept up to date from the notes, never instead of them.

## The four pages of every space

Made with the space, in the person's language, never empty of meaning (a short line says what goes there):

| Page | Tag | What it holds | Who writes |
|---|---|---|---|
| **What's next** | `living/next` | Short-term actions: one line each, starting with a verb, a date when there is one. Done items are ticked with the day, and leave for an *Archive* section after thirty days. | an assistant from the notes; the person at any time |
| **Ideas** | `living/ideas` | Ideas that are not yet actions, as they came. Reviewed now and then: an idea becomes an action, a target, or stays. | the same |
| **Targets** | `living/targets` | Long-term problems and goals: what, why, where it stands, the notes that show it. In the health space these are the targets of the nurse-made health journal (MemCP Health); elsewhere, a boiler that keeps failing, a project, a habit. | the same |
| **Where things stand** | `index` (exists) | A few lines on the state of the space: what it holds, what moved lately. What an assistant is given first. | the same |

The work and home spaces, and every space the person makes, have them. A page is a note with its tag, like `rules` and
`dashboard`: protected, left out of the plain list of notes, versioned (each change supersedes the last; the history keeps
every version).

## Always fresh: the date and the delta

Each page carries, in a front matter, when it was last changed and **the last note it took into account**:

```markdown
---
page: next
updated: 2026-11-16T22:40
covers: 2026-11-16T22:31 f_54upozy7czvf
---
## To do
- [ ] Call the supplier about the quote, before Friday (f_8k2…, 16/11)
- [x] Book the boiler service, 14/11 (f_3j1…)

## Archive
```

The **delta** of a page is the set of notes filed into its space after `covers` (living pages, rules and dashboards left
out). It is computed by the node, never guessed:

- `living_get(space, page)`: the page, its dates, and its delta (how many notes, and the notes themselves, newest last, up
  to a limit).
- `living_set(space, page, text, covers)`: a new version. Refused when `covers` is older than the page's current one (a
  stale writer), or when the person changed the page after the version the writer read: the writer reads again and merges.
- An assistant that connects is told which pages are behind and by how much (« work · what's next: 7 notes since 16/11 »),
  the way it is told that notes wait in À trier.
- The page of the node and the apps show each page with its date and a badge for its delta.

## How an assistant keeps them (rules given with the tools)

1. Work from the delta only, oldest first; never from memory of an earlier conversation.
2. Every line cites the note it comes from (id and date). Nothing without a source.
3. Never delete or reword a line the person wrote; tick it, or propose beside it.
4. One idea per line; short; the person's words.
5. A note can feed several pages (an action and a target); a note that feeds none is left alone.
6. Set `covers` to the last note actually read, so the next pass starts where this one stopped.
7. Keep each page short (about a hundred lines): archive done items after thirty days, keep targets to what is alive.

## Who updates, when

After each sorting pass the janitor records which pages have a delta. An assistant connected to the space updates them
(Claude Code, Hermes, any MCP client), or the local model of the machine for the simple pages (what's next, ideas) when the
person has allowed it; targets are left to a stronger assistant or the person. Nothing is updated without a delta.

## Why this shape

A model works best from a short, current, sourced page plus what changed since, rather than from thousands of notes: it
costs less context, it is checked line by line against its sources, and two assistants (or a person and an assistant) can
work on the same page without overwriting each other because every write names what it read.
