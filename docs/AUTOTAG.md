# Automatic tagging

Every note is tagged **by the node, when it is written**, whatever app or assistant writes it. No model: a small,
deterministic tagger in `memcp-core` (module `autotag`), the same for the node and for the janitor.

## What a write gets

1. **The tag of the space**, first: its name (`health`, `home`, `sam-cars`).
2. **The tags proper to that space**, from:
   - its `rules` note: a keyword found in the text gives the tag of that word (`zendure`); a tag of the rules whose last
     segment is found in the text is added as it is (`home/energie` for « énergie »);
   - its vocabularies: a term found in the text (the plural is the same word) gives `signe/<term>` in a health space, the
     plain word elsewhere;
   - **a health space** (kind `health`, or named `health`, `…-health`, `sante`): the built-in patterns below;
   - what the person taught (see Learning).

Rules of the write:

- The tags the caller gives **stay**. The node only adds; it never removes the caller's tags.
- Nothing is added in Inbox (its notes belong to no space yet), to a key limited to paths, or to a write that carries a plumbing
  tag (`rules`, `index`, `guide`, `dashboard`, `living/…`, `living-meta/…`, `janitor/…`).
- At most 16 tags are added besides the tag of the space; a note holds 64 tags at most.
- It applies to `remember`, `deposit` (each record), the description of `file_put` (its note and the words of the file
  name), the page's « Add to the journal » and `memcp note add`. A correction (`supersede`) is an explicit act: the node
  does not add to it, so a tag the person took off stays off.

## Fields

A **field** is a name and a value, kept as said. It is stored as a tag, so it travels, syncs and has a history like any
tag: `field/<name>/<value>`, where a `/` of the value is one more level of the path and the decimal point is `_`:

| Field | Tag |
|---|---|
| `level` = `2/10` | `field/level/2/10` |
| `tension` = `12/8` | `field/tension/12/8` |
| `poids` = `72.5` | `field/poids/72_5` |
| `poids_unit` = `kg` | `field/poids_unit/kg` |

Every fact the tools return carries `tags` and `fields` (the fields read back from the tags): `recall`, `read_fact`,
`history`, `living_get` (cards and delta), the page's journal.

## Calls and their participants

A write may carry a `call` object ([TOOLS.md](TOOLS.md), `remember`): the node turns it into tags, with the same encoding as any
field: `field/caller/<name>` and `field/callee/<name>` (one per participant), `field/call_app/<app>`, `field/call_direction/<dir>`,
`field/call_started_at/<unix>`, `field/call_duration_s/<n>`, `personnes/<name>` for every named participant (not for a number),
`appel` and `appel/<app>`. A name is a slug (`Alice Martin`: `alice-martin`), a phone number its digits, read back with a leading
`+`. In `fields`, `caller` and `callee` are always lists; any name held twice is a list; the other fields stay texts.

- **No duplicate.** The tagger adds nothing to a field the write already carries (one value per field it did not get from the
  caller), and a tag given is never given twice.
- **Strong hints for the router.** `personnes/` tags are not learned from the words of a note: they are learned **as people**. When
  the person sends a note of Inbox to a space (`triage_send`), each person of the note counts for that space; one note is enough.
  `tags_suggest` with `tags` (the note's tags) answers `route` with `"by": "people"` first when one space holds more than twice the
  next, ahead of the words; the janitor places a note with such a person before it tries its keywords (after the person's own work rule).
- **Corrections.** `tags_set` adds or removes `field/caller/<name>` or `field/callee/<name>` (or sets a whole side with `fields:
  {"callee": ["bob", "+442079460958"]}`); the `personnes/` tag follows (a name that leaves both sides of the call leaves the note).
  Changing people, call fields and `appel` tags is never learned as words.
- `memcp_core::call::people_names(brain, space?)` gives the distinct names of the `personnes/` tags with the number of active notes.

## The health patterns

Read from the health referential ([HEALTH.md](HEALTH.md)): the 85 signs and their everyday words (French; English for 31 of them), the 7 measures, and the 15 red flags (French only). The tables below list the first tagger's signs and measures, which keep their tag names; the referential has more (`signe/loneliness`, `signe/shame`, `mesure/sommeil`...) and a sign that is denied (`pas de fièvre`) is not tagged. A **red flag** gives the tag `alerte/<id>` (`alerte/chest_pain`), first, and the field `red_flag` with the first one found (the 15 before the 3114). The rest of this section describes the same rules.

French and English, folded (case and accents are ignored). A number counts only **next to its word**, with up to five filler
words between (`est`, `de`, `à`, `of`, `is`, `at`, `:` …). Decimals with a comma or a dot (`72,5`, `72.5`). A date
(`2026-11-17`, `17/11/2026`) and a time (`12h30`, `12:30`) are never a value.

### Measures (`mesure/<name>`, field `<name>`, optional `<name>_unit`)

| Measure | Words | Value | Range kept | Unit field |
|---|---|---|---|---|
| `tension` | tension, pression artérielle, blood pressure, bp | `12/8`, `12 sur 8`, `120/80`, `130 over 85`, `13 8` → `s/d` | 5-30 / 3-20 (cmHg) or 60-260 / 30-160 (mmHg), systolic above diastolic | `mmhg`, `cmhg` if said |
| `poids` | poids, pèse, weight, weigh(s/ed) | `72,5` → `72.5` | 20-400 kg, 40-900 lb | `kg`, `lb` if said (kg, kilo(s), lb(s), pounds, livres) |
| `pouls` | pouls, fréquence cardiaque, pulse, heart rate, fc | `64` | 20-250 | `bpm` if said |
| `glycemie` | glycémie, glucose, blood sugar, blood glucose | `1,05` → `1.05` | 0.2-40 (g/l, mmol/l), 20-800 (mg/dl) | `gl`, `mmoll`, `mgdl` if said |
| `temperature` | température, temp, fièvre, fever; also `38,5 de fièvre`, `38.5 °C` | `38.5` | 30-45 °C, 90-110 °F | `c`, `f` if said with `°` |
| `saturation` | saturation, spo2 | `97` | 50-100 | - |

A value out of range is ignored (`poids 5 kg`, `pouls 400`). The first value said of each measure is kept.

### Signs and symptoms (`signe/<name>`), with the level as said

| Tag | French | English |
|---|---|---|
| `signe/douleur` | douleur(s), mal au / aux / à | pain, ache(s) |
| `signe/douleur-thoracique` | douleur thoracique | chest pain |
| `signe/douleur-abdominale` | douleur abdominale, mal au ventre | abdominal pain, stomach ache |
| `signe/douleur-dos` | mal de / au dos | back pain |
| `signe/douleur-gorge` | mal de gorge | sore throat |
| `signe/cephalee` | céphalée(s), migraine(s), mal de tête, mal à la tête | headache(s), migraine |
| `signe/nausee` | nausée(s), mal au cœur | nausea, nauseous |
| `signe/vomissement` | vomissement(s), vomi | vomiting, vomited |
| `signe/vertige` | vertige(s) | dizzy, dizziness, vertigo |
| `signe/toux` | toux | cough, coughing |
| `signe/essoufflement` | essoufflement, essoufflé(e), souffle court, dyspnée | shortness of breath |
| `signe/palpitations` | palpitation(s) | - |
| `signe/diarrhee` | diarrhée | diarrhea, diarrhoea |
| `signe/constipation` | constipation | constipated |
| `signe/insomnie` | insomnie, mal dormi | insomnia |
| `signe/anxiete` | anxiété, angoisse(s), anxieux | anxiety, anxious |
| `signe/frissons` | frisson(s) | chills |
| `signe/demangeaison` | démangeaison(s) | itching, itchy |
| `signe/eruption` | éruption | rash |
| `signe/fievre` | fièvre, fébrile | fever |
| `signe/fatigue` | fatigue(e), épuisé(e), asthénie | tired, exhausted, exhaustion |

A phrase claims its words: « douleur thoracique » is `signe/douleur-thoracique`, not also `signe/douleur`; « mal à la tête »
is `signe/cephalee`, not also `signe/douleur`.

**The level**, said next to the sign, is the field `level`:

- `n/10`: `douleur 2/10`, `douleur à 7 sur 10`, `pain 7 / 10`, `pain level 3 out of 10` give `level` = `2/10`, `7/10`…
  (0 to 10);
- or the word as said: `léger(e)`, `faible`, `modéré(e)`, `fort(e)`, `intense`, `sévère`, `mild`, `moderate`, `strong`, `slight`,
  `severe`, `important`, up to two words after the sign or the word before it (« forte douleur »);
- when two signs of one note each have a level, the fields are `level_<sign>` (`level_douleur`, `level_fatigue`).

## Learning

The node learns from **the person's corrections**, and only from them, in a small file beside its databases
(`autotag-learned.json`, per node, never synced; losing it only makes the node forget).

- **Tags.** The person puts a tag on a note, or takes one off (the page, `tags_set`, a `supersede` that changes the tags): each
  word of the note (four letters or more, at most 30) counts for the tags added (`t`) and against the tags removed (`x`), **per
  space**. Plumbing tags (inbox, sources, janitor…) are never learned.
- **Moves.** The person sends a note or a file of Inbox to a space (the page's *Send*): each word counts for that space
  (`routes`).
- **Scores.** The person is right by definition: a word counts once it was corrected **twice**, and a tag is suggested when two
  words agree (or one word was corrected four times). A tag taken off cancels what was put on; twice taken off, it is
  **denied** for notes like this one (the tag of the space is never denied). A route is given when at least two words point to
  one space and it scores twice the second.
- **Bounded.** 2000 words per space (and for the routes), 8 tags per word; the least seen are forgotten first.
- **Never from a program.** What the janitor does (`supersede`, `triage_send`) is never learned; an engine that proposes tags
  passes `learn: false`.

The learned tags enter the suggestion like any other (the write, `tags_suggest`); the learned routes are answered by
`tags_suggest` (`route`) and used by the janitor for a note of Inbox no keyword places.

## The JSON forms

`tags_suggest` (read), `{ "space"?: string, "text": string }`:

```json
{
  "space": "health",
  "tags": ["health", "signe/douleur", "mesure/tension"],
  "fields": { "level": "2/10", "tension": "12/8" },
  "field_tags": ["field/level/2/10", "field/tension/12/8"],
  "all_tags": ["health", "signe/douleur", "mesure/tension", "field/level/2/10", "field/tension/12/8"],
  "route": [{ "space": "personal", "score": 6 }]
}
```

`tags_set` (write), `{ "id": string, "tags"?: [..], "add"?: [..], "remove"?: [..], "fields"?: { name: value }, "reason"?: string, "learn"?: boolean }`:

```json
{
  "result": "superseded",
  "fact": { "id": "f_ac13", "replaces": "f_ab12", "text": "…", "tags": ["field/level/4/10", "health", "suivi/genou"], "fields": { "level": "4/10" } },
  "added": ["field/level/4/10", "suivi/genou"],
  "removed": ["field/level/2/10", "signe/douleur"],
  "reason": "tags set by engine",
  "learned": true
}
```

The note is replaced by itself with the new tags (a `supersede`): the old version stays in the history with the reason; the
same set again answers `{ "result": "unchanged" }`; a note keeps at least one tag; a note that describes a file is retagged with
the file. A key that does not reach the space gets `unknown fact`.

A fact, anywhere it is returned: `{ …, "tags": ["health", "mesure/tension", "field/tension/12/8"], "fields": { "tension": "12/8" } }`.

An app that wants its own tags passes `tags` as before: they are kept, and the node adds the space's. An engine on the device
(Apple Intelligence through MCP) calls `tags_suggest` to preview, then `tags_set` with `learn: false` to complete a note.
