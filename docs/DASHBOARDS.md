# Dashboards: a space shows itself, and an assistant may shape it

Status: built on the node page and served as data by the space view contract (`space_view`, `view: "dashboard"`, [CONTRACT.md](CONTRACT.md)); every section now carries an `id`. The declarative format below is what the apps and the assistants share.

## The idea

Each space can have a **dashboard**: a small JSON document that says what to show of the space and how. The dashboard is **data**, not code. It can be written by a person, shipped as a preset, or developed and modified by an assistant, and every platform draws it with its own native controls ([VIEWS.md](VIEWS.md)).

By default everything is **local**: the dashboard is computed on the device or the node from the notes of the space, with no model and no network. An assistant is optional.

## The format (version 1)

```json
{
  "version": 1,
  "title": "Health",
  "sections": [
    { "type": "counts",  "title": "Signs and symptoms", "from": "journal-manifestations", "window_days": 30, "top": 12 },
    { "type": "targets", "title": "Targets", "tag": "cibles" },
    { "type": "series",  "title": "Weight", "from": "measure", "kind": "weight", "window_days": 90 },
    { "type": "daily",   "title": "Notes day by day", "window_days": 30 },
    { "type": "list",    "title": "To review", "tag": "a-revoir", "limit": 8 },
    { "type": "text",    "body": "Short text the person wrote for themselves." }
  ]
}
```

- A **closed catalogue** of section types (`counts`, `targets`, `series`, `daily`, `list`, `table`, `gauge`, `text`), each with a fixed set of parameters. A document with an unknown type or parameter is refused, not guessed at.
- A section reads facts of its own space by **tag path**, by **kind of record**, or by a **named source** that the core knows (the manifestations of a journal, the measures of a health app). It has no script, no link to another site and no access to another space.
- The document is stored with the space, versioned, and travels with the other settings. Every change is in the history with who made it and why, and can be reverted in one step.

## Counting works in every space

The `counts` section is the keyword model, used to count instead of to file, and it is not specific to health:

- `"from": "journal-manifestations"`: the signs and symptoms of the journal days (the health app's format).
- `"from": "keywords", "terms": ["alpha", "beta"]`: how many notes of the window cite each term. Under a work space, a project cited several times this month shows up as a count; in a home space, a room, an appliance or a car.
- `"from": "tags", "tag": "work/projects"`: how many notes sit under each direct child of a tag.

Later the terms can come from the keywords the space already files notes by (see TOOLS.md, `rules`), so that the words that route a note are also the words that are counted.

## Who may change it

The same levels as the rules for filing notes (TOOLS.md):

| Level | An assistant may |
|---|---|
| *Propose* (default) | write a proposed dashboard; the person sees the difference and accepts or refuses |
| *Apply* | apply a change in a space that is not sensitive |
| never on a sensitive space | apply: a change to the dashboard of a health or money space always waits for the person |

An assistant that edits a dashboard needs to see **its definition**, not the notes: reading the notes to suggest better sections is a separate right, off by default on a sensitive space, and never through a remote model unless the person has said so for that space.

## What ships where

- **This repository** ships the format, the checker, the generic renderer (the page of a node draws any dashboard document), the plain default dashboard (the notes day by day) and a simple health space: a space and a folder, with no dashboard of its own.
- **Specific dashboards** are presets, made by the author, and come with the MemCP app for iOS and Mac, which is sold by subscription. The first is the **health dashboard, designed by a nurse**, built around what a person follows (their targets): it counts the signs and symptoms noted in the journal, day by day and over the last weeks, and lists the targets. It interprets nothing, names no diagnosis and advises no treatment. Then, over time: money, development, hobbies, and the house (with Home Assistant first), each with its own dashboard.
- **How a preset reaches a node.** When the app is connected to a node, it stores the dashboard of a space there as an active fact tagged `dashboard` (replaced at each change, so the history keeps every version), and the page of the node draws it with the generic renderer. Without the app a person may write a document by hand, or ask an assistant to, within the levels above.
- **Without any assistant**, a dashboard is local and needs no model. **With one**, if the person chooses, the same dashboard can be reshaped: new sections, other windows, a series for a measure they started to follow.

## Order

1. Built: the sections `daily`, `counts` (from the named source `journal-manifestations`), `targets`, `list` and `text`, read from the stored document and computed on the node page.
2. The format, its validator and its golden files in the shared core; the dashboard of a space stored and versioned.
3. Native renderers from the same document (iOS, macOS, Windows, Linux).
4. Built: the tool `space_view` (`view: "dashboard"`) for clients and assistants; the levels above stay the rule for a write.
