# Views: a space reads itself

Status: the read model is served by the space view contract `memcp.view/1` (`space_view`, [CONTRACT.md](CONTRACT.md)); the node page and the native renderers migrate onto it. The journal of the health app becomes the **health view** of every interface.

A **view** is how a space is read by a person (not by an assistant: assistants have tools). Each kind of space has one, chosen automatically, and every interface offers it: iOS, macOS, Windows, Linux, and the web page of a node.

## One view, every interface

- A view is two things: a **read model** and a **renderer** per platform.
  - The read model is computed by the shared Rust core from the facts of the space. It is plain data (days, series, targets, counts), the same on every node.
  - Each platform draws that read model with its own native controls (SwiftUI on Apple systems, a native toolkit on Windows and Linux), and a node draws it as a web page. Same rule as the settings screens ([CONFIG-UI.md](CONFIG-UI.md)): a description in the core, one native renderer per platform, and a test that fails when a renderer drops a section.
- The kind of a space says which view opens when the person touches the space. A space may override it, and any space can fall back to the **generic view**: the tree of tags, the latest facts, search.
- A person explores their memory the way an assistant does through its tools, and on the same facts: the web page opens each space on its dashboard, its tree of tags and its notes, and the person may correct or remove a note there. A correction is a `supersede` and a removal a `retire` with its reason, so the history keeps every earlier text: nothing is overwritten. Capture stays native (the phone, the watch, the desktop app).
- A locked space shows "locked" and nothing else; a view of a sensitive space is shown only to its owner, signed in (Home Assistant sign-in, tailnet identity, or a console key), never on the public MCP address.

## The health view

It is the journal of the health app, with the same sections everywhere:

- **Targets**: what the person follows (a problem, a risk, an objective), each with its notes, its actions and when they are reviewed; a target is added by the person, or proposed by an assistant and accepted by the person.
- **Check-in and notes of the day**: what was noted, with the manifestations (an intensity from 0 to 10, an energy from −5 to +5, a mood), the time, and the way it was entered (typed, dictated).
- **Measures**: weight, blood pressure, heart rate, glucose, temperature, oxygen, breathing, waist, water, caffeine, naps, workouts; series over time.
- **Events followed from their day**: a vaccine, a treatment, an injury, an illness, with the days to follow.
- **Questionnaires** the person answered (with the safety rule that an answer about self-harm shows the emergency numbers first), and **hypotheses** counted in the person's own days.
- **Safety, always**: the distress screen and the red-flag checks are part of the view and are never behind anything. The view names no diagnosis and advises no treatment.

Where its data comes from: the app deposits what it records under its own path of the health space (`sources/<app>` for the raw records, `dossier/<app>` for the sorted day by day entries with their targets), and the view is computed from those facts, so the page of a node, a Mac and a phone show the same journal. The paths keep the id of the app that wrote them; an installation that already holds deposits keeps its paths.

## Other kinds

| Kind | View |
|---|---|
| health | the journal above |
| money | balances, movements and the monthly statement, from the bank statements; caps and merchants to watch |
| home | the house: latest notes by tag, decisions, open to-dos |
| work, hobbies, people, me | the generic view, with the to-do and decision tags in front |
| any custom space | the generic view |

## The contract

The three things a space exposes to a person and to a client are its **dashboard**, its **targets** and its
**timestamped register**. They are served as data by `space_view` (`memcp.view/1`): see [CONTRACT.md](CONTRACT.md)
for the fields, the filters, the pages, the stable ids and the provenance. The node page reads them through
`GET /admin/api/space/view`; the native renderers read the same shape from the core.

## On the node page

The page of a node gets one entry per space the person owns, opening that space's view. The node computes the read model with the same core as the apps, so no logic is written twice. A view is served only through the sign-in of the page (never through an assistant's key), and the page says when it is built from a space that is locked or not synchronised yet.

## Order

1. The read model of the health journal in the shared core (from the day entries, targets and measures that the app deposits), with golden files, used by the iOS app first.
2. The web renderer of the node page for the health view, then the generic view for every space.
3. The money view.
4. Native renderers on Mac, Windows and Linux, from the same read model.
