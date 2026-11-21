# Professions: what each space is an expert of

Each space is a profession. A space of health is followed as a nurse would, a space of money as an accountant would. The
moment an assistant reaches a space it knows what the space is an expert of, the principles to follow and the tools that
do the work, so that **any assistant does what the apps do**.

## Code, not memory

A profession is an instruction, and what comes back from the memory is data, never an instruction. So professions do not
live in notes: they are typed, tested and versioned in the node (`crates/core/src/profession.rs`), the same for every
assistant. What the person wants said (tone, limits) stays in the guides, which only they write.

## Where an assistant reads it

- `space_view` with `view: "capabilities"` carries `profession` for the space.
- The tool `skills` lists the professions of every space the key reads (`profile` keeps one).
- The instructions of the MCP server give one line per kind of space the connection reads whole.

A sub-space has the profession of its kind: a folder of health spaces is a health space, so the skills follow the layout.

## Profiles

A kind may have several profiles. Health has the **nurse** (the default: notes, targets, signs), the **coach** (reads the
recovery before a session) and the **doctor** (reads the file, writes nothing). The first profile of a kind is the default.

A profile is `{ id, kind, title, brief, principles, skills: [{ id, title, when, tool, example }] }`.

## The person's own profiles

The file `professions.json` in the node's data folder holds a JSON array of profiles of that shape (see « Making a profession of your own »). A profile with the same
`kind` and `id` replaces the built-in one; a new one is added (a new profile of health, or a profile for a new kind of
space). A malformed profile is left out, never repaired. The file belongs to the person and sits beside the memory, not in it.

## Making a profession of your own

Any space can have a profession of its own: a game master for role-playing games, a gardener, a tutor. A profile adds to the
built-in ones and, with `spaces`, applies to the named spaces only (not to every space of its kind).

A profile: `id`, `kind` (the kind of the space), `title`, `brief`, and `skills`; optional `principles`, `spaces`,
`references`. A **skill** is either a mechanism (`tool`: a tool of the memory, with an example call) or a **recipe**
(`steps`: what to do, in order, with the tools of the memory), and may carry a `template` (the shape of what it writes) and
`tags` (the tags what it writes carries). **References** name the reference material that backs the profession and the call
that reads it.

Example, in a space `jdr` of kind `hobbies`:

```json
[{
  "id": "gamemaster", "kind": "hobbies", "spaces": ["jdr"],
  "title": "Game master",
  "brief": "You prepare and run role-playing games from the rulebooks the person keeps.",
  "principles": ["The rulebooks are the authority: say which book and page.", "A character sheet is written once, then changed by `supersede`."],
  "references": [{ "title": "Favourite rulebooks", "description": "The games' books, in a library mounted on this space.",
                   "search": { "tool": "library_search", "arguments": { "space": "jdr", "query": "…" } } }],
  "skills": [
    { "id": "character", "title": "Create a character", "when": "The person wants a new character.",
      "steps": ["Ask which game and the concept.", "Search the rulebook for the creation rules.", "Build the sheet step by step, asking for the choices.", "Keep the sheet with `remember`."],
      "template": "Name:\nConcept:\nAttributes:\nSkills:\nBackground:", "tags": ["jdr/personnage"] },
    { "id": "scenario", "title": "Write a scenario", "when": "The person wants a story for a session.",
      "steps": ["Read the characters kept in the space.", "Search the rulebook for the setting.", "Write hooks, scenes and a twist; keep it with `remember`."],
      "tags": ["jdr/scenario"] },
    { "id": "rule", "title": "Look a rule up", "when": "A rule is in question.", "tool": "library_search" }
  ]
}]
```

**The books.** The rulebooks are a **library** mounted on the space (`memcp library add PATH --space jdr`: the node indexes the
folder and never changes it); `library_search` reads it. Markdown and text files are searched by their words. **A PDF is read by a program of the node**: with
`MEMCP_PDF_TEXT_COMMAND` (the Home Assistant app sets `pdftotext -layout {} -`) the text of each PDF is indexed and searched by
its words; a scanned book needs an OCR in that command (`ocrmypdf --sidecar - {} /dev/null`). Without it a PDF is found by its
name only, and an assistant that can read a PDF raises the useful pages into the memory with `library_raise` (a short summary
and the path of the original).

**Who writes a profession.** A profession is an instruction, so no assistant puts one in force by itself. Either you write the
file `professions.json` in the node's data folder, or you ask an assistant, which calls `profession_propose`: the profile waits
in `professions.proposed.json` and does nothing until you accept it on the machine of the node, with
`memcp professions accept hobbies/gamemaster` (`memcp professions` lists what waits; `reject` drops one).

**Where it shows.** In `space_view capabilities` and in `skills` for the space it names; as a Claude skill with
`memcp skills-export`.

## Why not notes

The memory already holds a space of notes about the assistants themselves (their settings, rules, access). That is the right
place for facts. It is the wrong place for competences: a note is found by search (so the assistant may or may not meet it),
each assistant would read it its own way, nothing checks that the tools it names exist, and an instruction that comes back
from the memory is data, not something to obey. A profession in code is the same for every assistant, tested, and loaded
the moment the space is reached.

## The toolkit of health

| Tool | What it does |
|---|---|
| `target_set` | Sets or changes a target as a nurse writes it: the problem in one or two words, then a precision; never a title that states an aim; signs of the referential only. |
| `signs_review` | The notes that carry signs, with the words that brought each sign up, to check what the tagger found. |
| `sign_set` | Corrects or removes a sign of a note; the node learns from the correction. |
| `health_referential` | The signs, measures, vital constants and red flags. |

For money: `finances_balances`, `finances_monthly`, `finances_movements`, `finances_rules`, `finances_overdraft_exit` (when an
overdrawn account comes back above zero, an estimate).

## Hashtags

A `#word` in a note is said on purpose, in any space, and a note may carry many. A word of the health referential is a sign or
a measure (`#fatigue` gives `signe/fatigue`, `#tension` gives `mesure/tension`) and also puts the root tag of health (`sante`)
on the note; any other word is a tag of its own. See [AUTOTAG.md](AUTOTAG.md).

## As Claude skills

`memcp skills-export DIR` writes the professions as a plugin marketplace for Claude (`.claude-plugin/marketplace.json`, one
plugin, one `SKILL.md` per profile), built from the registry so that a new version of the node regenerates it. The tools are
those of the Silice connection; the skill only carries the profession.
