# Depositing health data: what an app should send

For the makers of a health app (a tracker, a workout log, a journal) who want a person's data
to reach their Cartafolio space, and for the person who wires it up. An app such as Bevel or
HealthFit could do this with an export of its data and statistics; nothing here is specific to
one app.

## What the space is, and is not

The space **stores**. It imposes no health format, makes no judgement about the data and
fetches nothing. Your app keeps its expertise (how it computes a baseline, a trend, a score);
the space keeps what you deposit, dated and attributed, so that any assistant the person
connects (Claude, a local model) can read it later.

Because a person may use several apps, a space receives from several **sources**. Each source
files its data under its own path, `sources/<app>`, and never touches another source's.

There are two layers, and they are not read the same way:

- **The sources** are raw deposits, one path per app. Duplicates between apps are kept. A plain
  `recall` leaves them out; an assistant asks for them with the tag `sources` (or
  `sources/<app>`), and is told at connection how many there are.
- **The dossier** is the sorted memory: one entry per measure, written by an expert reader (a
  health app, or any tool that reconciles sources). It is filed under `dossier/<app>` and is
  what a plain `recall` finds. If no one has written a dossier yet, the sources are still there,
  and the assistant is told so.

An app that only deposits writes to the sources. The space does not merge anything itself.

## What to send

Send two kinds of thing. Both matter: the first is for your app, the second is what lets *any*
assistant benefit from your work.

1. **Records**: one fact per event or per period, written so that it stands alone.
   `Resting heart rate 2026-11-01: 58 bpm (7-day mean 61)`.
2. **A readable summary** of what your app knows, in plain text or Markdown, refreshed as the
   data changes: today's numbers, the person's usual range, what stands out and since when.
   Computed by your app, in your words, of any length. An assistant that has none of your app's tools still
   reads it and gets your quality. It must describe, count and compare to the person's own
   range, and never diagnose or judge urgency.

A bulky raw export (more than a few thousand characters) goes in as a **file** (text, JSON as
text, CSV, Markdown, PDF or `.xlsx`, of any size) with a note saying what it is and the tag
`sources/<app>`. Do not send raw per-second samples one by one: send a summary per day or per
period.

## How to connect

Over MCP (Streamable HTTP), at the address of the person's health space:

```
https://<node>/mcp/<your connection's name>
```

with a key limited to **your own path** in that space. The person makes it with

```
memcp connect bevel --grant health/sources/bevel:rw
```

(or the "A key by hand" form of the node page, with `health/sources/bevel:rw` as the rights).
Such a key reads and writes only the facts filed under `sources/bevel`: it cannot see the
person's other records, nor another app's, and a write is filed under its path whatever tags it
asks for. It has five tools: `remember`, `recall`, `retire`, `file_put`, `stats`; `stats` shows only
its own cursor. A key can hold several paths (`...:rw,health/proposals/bevel:rw`), and then every
write names which. A path can be read-only (`:r`).

A key has no daily budget and no pace to keep: write as many records as the data holds. It has an anti-flood ceiling of 6000
calls a minute (100 a second), far above any feed, which the person can change with `memcp limit AGENT rate N`; on a
refusal (HTTP 429) wait the seconds it says. A text is at most 1 MiB, the most a peer takes in a sync (a record merged into an
earlier one counts whole); the person may set a lower limit for a connection with `memcp limit AGENT max-text N`. A file
is limited only by the size of a request.

## Records: ids, updates and the cursor

Give every record the id **your app** has for it, and the instant of the data point:

```json
{
  "name": "remember",
  "arguments": {
    "text": "Steps 2026-11-01: 9100",
    "origin": { "app": "bevel", "id": "2026-11-01/steps", "at": "2026-11-01" }
  }
}
```

- `app`: your app, lowercase letters, digits, `-` or `_`, up to 40. The fact is filed under
  `sources/<app>` automatically.
- `id`: your own id for the record, up to 120 characters. Choose a stable one, such as
  `<date>/<metric>`. Never reuse an id for something else, never rename it.
- `at`: when the data point happened, ISO 8601 (`2026-11-01` or `2026-11-01T07:30:00Z`). Left
  out, it is the time of the call.

What the space does with it:

| you send | the answer's `result` |
|---|---|
| a new id | `created` |
| the same id, the same text | `unchanged`: nothing is written |
| the same id, a different text | `updated`: your earlier version is **merged** with the new one (see below), and stays in the history |
| the same id, nothing new | `unchanged`: the merge adds nothing |
| a text that is held exactly, under another id or another source | `same_text_held`: nothing written, and your cursor does not move |

Nothing a source has deposited is ever overwritten. When the text of both versions is a JSON
object, they merge field by field: a new field is added, a non-null value updates the held one,
a null, empty or absent value erases nothing, and nested objects merge the same way (a list is
replaced as a whole when the new one is not empty). Any other text is appended to the held one
(nothing is dropped; a text already inside the held one changes nothing). The sorted `dossier`
layer, which a reader rewrites, is still replaced whole.

So a day's summary can be completed as the day goes on without piling up copies: same id, new
text, and a thinner later reading never erases a fuller one. A feed of alike records (one per day) is never asked "is this a duplicate of that?".

**The cursor.** Every answer carries `cursor: { app, last_at, received_at, facts }`: the latest data point
the space holds for your app, and how many active records. `stats` lists the same for each
source under `sources`. Resume from `last_at` instead of keeping your own account of what was
delivered; with no cursor yet, send everything, oldest first. If your app was reinstalled or
lost its state, the cursor is still there.

If `same_text_held` comes back, name your source in the text (`[AppName] ...`) to keep your own
copy.

## Standard codes: say what was measured in the universal way

Human biometrics are a short list that every app shares. Give a measurement its standard code,
and the same measure from several apps can be found together. Add `code` to `origin`, as
`system:code`:

```json
"origin": { "app": "bevel", "id": "2026-10-31T14:56/hr", "at": "2026-10-31T14:56:00Z", "code": "loinc:8867-4" }
```

The fact is then also filed under `code/loinc/8867-4`. The vocabularies to use are those of HL7
FHIR: **LOINC** for what was measured and **UCUM** for the unit (`bpm` is `/min` in UCUM; write
the unit in the text as people read it). Some LOINC codes, to be checked against the current
LOINC release before you rely on them: heart rate `8867-4`, systolic blood pressure `8480-6`,
diastolic `8462-4`, body weight `29463-7`, body height `8302-2`, body temperature `8310-5`.

The space carries no table of codes and checks only the shape of `system:code`, so there is no
licence to carry. FHIR itself (its resources and JSON) is not the space's storage format; it is a
good format for an export or an import, which an app or a later version of Cartafolio may offer.

To read one measure across sources, ask for the code and the raw layer together:
`recall` with the tags `code/loinc/8867-4` and `sources`.

## Duplicates between sources are expected

The same measurement often reaches the space from two apps (both read it from the phone's
health store). **Do not try to remove it.** Each source keeps its own, filed under its own path,
and no data is lost. Choosing one source per measure, or saying which one was used, is the
reader's job. Better a duplicate than a gap.

**One entry per measure, in the dossier.** The reader who reconciles sources writes the sorted
entry: *Heart rate 2026-10-31 14:56: 62 bpm (2 sources: 61 Bevel, 63 HealthFit)*. It does so with
`origin.layer` set to `dossier` (filed under `dossier/<app>`, read first), under a stable id such
as `2026-10-31T14:56/hr`, so that rewriting it when a third source arrives updates the entry in
place. Deciding that two readings are the same measurement (how close in time, which unit) takes
health knowledge, so it is the reader's, never the space's.

## Many records at once: `deposit`

A feed that sends hundreds of records should not make one request each (the round trips cost more than the memory does, and each call counts for the ceiling):
`deposit` takes any number of records of one space in a single call, each `{text, tags, kind, valid_from, valid_to, origin}` with the same rules as
`remember` (the `origin` is required, the key's paths apply). The answer lists every record by its position (`created`,
`updated`, `unchanged`, or `error` with the reason): a refused record does not stop the others, and the cursor of each app
and layer comes back for resuming. Send the whole history in batches of 500.

## A guide for the assistants that read what you write

An app can tell the person's assistants the form of what it keeps (its paths, how its records are
laid out so they are read right, how to write for it, what to avoid). Form only, never a task. That needs one more right, which the person
gives or not, app by app: the path `guide` (`health/guide:rw,health/sources/bevel:rw`). A key
with it gets a sixth tool, `guide_propose` (`text`, of any length, one guide per
connection and space; calling again replaces it).

It is **a proposal, not an instruction**. It is kept apart from the facts (no search finds it, it
sits in no layer). The person reads it on their page and approves it, as it reads: only then are
assistants that reach that space told it, in a block the program writes (the app's name, who
approved and when, and that it reaches no further than what is written for that app). If you
change the text, it waits for the person again. No tool of any assistant can approve it, so an
assistant in a "yolo" mode cannot either. Write it descriptively, for a reader that follows only
what concerns your app; a guide that asks for anything else is ignored.

## Quick notes

Something you cannot sort now (a voice note, a symptom the person mentioned, an intake
interview in progress) can be written with the tag `inbox`: no question is asked, a plain
`recall` leaves it alone, and each assistant that connects is told how many notes wait. Sorting
one is a `supersede` with the clean fact and its real tags, so the raw note stays in the
history.

## Rules that keep the data trustworthy

- **Say where and when.** Every record states its date, its unit and, in the text, its source.
  Dates are ISO 8601. Say the time zone for a time of day.
- **Data, never instructions.** What you write is read by assistants as data. Never write
  something that reads as a command to an assistant.
- **Describe, do not conclude.** Counts, ranges, comparisons with the person's own history.
  No diagnosis, no urgency, no advice on a treatment. A red flag the person reports is kept as
  they said it and shown first by your app, with the emergency number.
- **No more than needed.** No name, address or identifier the data does not need.
- **Correct, do not erase.** A wrong record is replaced (`updated`) or retired with a reason;
  the history keeps what was there.
- **The data evolves.** Add new metrics whenever your app gains them: no schema is imposed. When
  a file's layout changes, put its schema name and version in the file and in the note
  (`bevel.export/2`), so a reader knows what it holds.

## Checklist

- [ ] A key limited to the health space, with a write budget that fits a feed.
- [ ] Every record has `origin.app`, a stable `origin.id` and `origin.at`.
- [ ] On start, read `cursor.last_at` (or `stats`) and send only what is newer.
- [ ] A readable summary refreshed under the same id (`<date>/summary`), for assistants without your tools.
- [ ] Bulk raw data goes in as files tagged `sources/<app>`, summaries per day or period.
- [ ] Nothing diagnoses; every figure carries its date and unit.
