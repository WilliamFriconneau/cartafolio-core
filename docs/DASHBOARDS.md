# Dashboards: a space shows itself, and an assistant may shape it

Status: design. The first part is built on the server's page (the notes per day, and for a health space the count of signs and symptoms and the targets); the declarative format below is what the apps and the assistants will share.

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

## Who may change it

The same levels as the rules for filing notes (TOOLS.md):

| Level | An assistant may |
|---|---|
| *Propose* (default) | write a proposed dashboard; the person sees the difference and accepts or refuses |
| *Apply* | apply a change in a space that is not sensitive |
| never on a sensitive space | apply: a change to the dashboard of a health or money space always waits for the person |

An assistant that edits a dashboard needs to see **its definition**, not the notes: reading the notes to suggest better sections is a separate right, off by default on a sensitive space, and never through a remote model unless the person has said so for that space.

## The health dashboard

The health space ships with a dashboard **designed by a nurse**, built around what a person follows (their targets), with the sections above. Without any assistant, it does one plain thing well: **it counts the signs and symptoms noted in the journal**, day by day and over the last weeks, and lists the targets. It interprets nothing, names no diagnosis and advises no treatment.

With an assistant, if the person chooses to connect one, the same dashboard can be reshaped, within the levels above: new sections, other windows, a series for a measure they started to follow.

This is what the iPhone app is sold with, and the page of a server shows the same thing.

## Order

1. The two sections built today on the page (`daily`, and for a health space `counts` and `targets`), computed on the server.
2. The format, its validator and its golden files in the shared core; the dashboard of a space stored and versioned.
3. Native renderers from the same document (iOS, macOS, Windows, Linux).
4. The tool `dashboard` for assistants, with its levels.
