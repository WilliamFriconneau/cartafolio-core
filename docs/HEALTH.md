# The health referential

What the node knows about health, as data and as code: the signs and symptoms a person says, the measures and vital constants with their units and plausible ranges, and the red flags. It is the crate `memcp-health` (`crates/health`), a library with no binary, no model, no I/O and no clock. The node's tagger ([AUTOTAG.md](AUTOTAG.md)), the janitor's health rule and the read tool `health_referential` ([TOOLS.md](TOOLS.md)) all read the same data, so an app, an assistant and the node agree.

A result is a **suggestion for the person to confirm**, never a diagnosis. Nothing is ticked by itself.

## The files

Embedded in the program (`crates/health/data/`), each with a `schema`, a `version` (an integer, raised when its content changes), a `license` and a `provenance` line. The tool `health_referential` answers the versions.

| File | Content |
|---|---|
| `manifestations.json` | the structure of the 85 signs: id, category, dimension, scale, `also`, `located`, the tag name, and the pairs of opposites |
| `manifestations.fr.json`, `manifestations.en.json` | the label, the everyday keywords, the negators and the words of a level, by language |
| `measures.json` | the 7 measures: keywords, units, plausible ranges |
| `vitals.json` | the 9 vital constants: unit, plausible range, usual limit, labels in `fr`, `en`, `es` |
| `red_flags.fr.json` | the 15 red flags: patterns, exceptions, message, number to call first, characterization questions, cited sources |

### Provenance and licence

The memory's code is AGPL-3.0-or-later and so is every file here.

- **Signs (83), their French labels and keywords, the pairs, the red flags and the usual limits of the vitals** come from the health referential of a private project of the same owner. That project is licensed AGPL-3.0 (its `LICENSE`), and its referential files carry **no licence line of their own** and no per-keyword origin: the origin of each keyword is **unknown** and says so in the `provenance` fields. Left out on purpose: the explanatory texts, the definitions and the healthkit identifiers, which are the app's business. The red flags cite public sources (ameli, NHS, 3114); the list is in the file (`sources`), and a source with `level: "missing"` is a gap stated, not invented.
- **Written for this project**, marked in the data: two signs (`itching`, `rash`, with `origin: "added"`), the `keywords_added` lists (forms the tagger already knew: `nausées`, `vomis`, `asthénie`...), the whole English vocabulary, the measures file (keywords, units and ranges follow what the first tagger and the reference note reader already accepted; the ranges are physiological bounds, not clinical limits: source **unknown**), the labels in `fr`, `en`, `es` and the plausible ranges of the vitals.

## Languages

| | French | English | Spanish |
|---|---|---|---|
| Signs | all 85 | 31 signs (the ones the tagger knew, plus a few common ones) | **none** |
| Measures | all 7 | all 7 (keywords and units) | **none** (labels only) |
| Red flags | all 15 | **none** | **none** |
| Vital constants | labels | labels | labels |

The source referential has French everywhere; the only text it has in English and Spanish is its explanatory texts for the vitals, which are not copied. English signs and measures were written for this project. **There is no English or Spanish red flag**: a text in those languages is never reported as an alert, and an app must not take silence for safety. `Lang::Es` is accepted and finds nothing. `Lang::Auto` reads French and English together (the labels follow the language of the words that brought a sign up).

## Detection

```rust
let d = memcp_health::detect("Mal de tête 6/10, tension 12/8", Lang::Fr);
```

```json
{
  "signs": [{ "id": "headache", "label": "Mal de tête", "level": "6/10", "scale": "pain" }],
  "measures": [{ "id": "blood_pressure", "value": 120.0, "second": 80.0, "unit": "mmHg" }],
  "red_flags": []
}
```

```json
{
  "signs": [{ "id": "chest_pain", "label": "Douleur à la poitrine", "scale": "pain" }],
  "measures": [],
  "red_flags": [{ "id": "chest_pain", "message": "Une douleur dans la poitrine peut être urgente…", "first": "15" }]
}
```

- **Signs.** A sign is found by its label or one of its keywords, as a run of whole words (case, accents and apostrophes ignored; the last word may be a plural). A match right after a negation (`pas de`, `aucune`, `sans`, `jamais de`; English `no`, `not`, `without`) does not count, but a bare `pas` does not deny (`pas mal de fatigue` says there is some). A shorter keyword inside a longer one of another sign is dropped (`mal au ventre` is the belly's, not also `pain`). Each sign is given once, in order of appearance; a more general sign it also is (`also`: mental fatigue is fatigue too) follows it.
- **Level.** On a named scale only (`pain`, `intensity`, `wellbeing`; the mood, `-5` to `+5`, is left to its bar, and a sign noted by presence has none): `n/10`, `n sur 10`, `n out of 10`, `n / 10`, from 0 to 10. A number without its scale is no level (`depuis 2 jours`). Phrase by phrase (cut at `. ; ! ? , ` and line breaks): as many levels as signs pair in order; a level alone goes to the one sign of the phrase before (`anxieux, 2 sur 10`); a level that a later sign closes goes to the sign before it (`douleur 4 sur 10 et fatigue`); a level that ends a phrase of two signs is ambiguous and left out (`triste et anxieux à 4/10`). Without a number, the word as said, up to two words after the sign or the word before it (`forte douleur`, `mild headache`).
- **Measures.** A number counts only next to its word (`tension 12/8`, `je pèse 72,5 kg`, `38,5 de fièvre`; up to five filler words such as `est`, `de`, `à`, `of`, `:`), or with a unit that says what it is (`64 bpm`, `72 kg`, `38,5 °C`), and only inside its plausible range. A comma is a decimal point. **A date (`2026-11-17`, `17/11/2026`) and a clock time (`12h30`, `12:30`) are never a value.** The one exception is a duration after its own keyword: `dormi 6h30` is 6.5 hours, `dormi à 12h30` is nothing. The first value said of each measure is kept. `value` is in `unit`; a pressure said in centimetres of mercury (`12/8`) is given in mmHg (`120/80`). Glucose with no unit: up to 40 is g/L or mmol/L, from 40 to 600 is mg/dL.
- **Red flags.** Matched clause by clause on folded text. Only a real negation denies (a negator right before the sign, one indefinite article allowed between; `plus (de)` after a `ne`); doubt fires; `de plus en plus` never denies. Harmless idioms (`mourir de rire`, `en finir avec ce dossier`) are silenced only when they cover the match entirely. Rules that send to the 15 come before the 3114 rule, so the first hit gives the number to put first. Messages never name a disease. The characterization questions are shown only after the help screen and change nothing.

| Measure | Id | Tag in the tagger | Unit of `value` | Range kept |
|---|---|---|---|---|
| Blood pressure | `blood_pressure` | `tension` | mmHg | 60-260 / 30-160 mmHg or 5-30 / 3-20 cmHg, systolic above diastolic |
| Weight | `weight` | `poids` | kg | 20-400 kg, 40-900 lb |
| Heart rate | `heart_rate` | `pouls` | bpm | 20-250 |
| Glucose | `glucose` | `glycemie` | g/L | 0.2-40 (g/L, mmol/L), 20-800 mg/dL |
| Temperature | `temperature` | `temperature` | °C | 30-45 °C, 90-110 °F |
| Oxygen saturation | `saturation` | `saturation` | % | 50-100 (the vital `oxygen_saturation`) |
| Sleep | `sleep` | `sommeil` | h | 0-16 |

## Tags in the tagger

`signe/<tag>`, `mesure/<tag>`, `alerte/<id>` (see [AUTOTAG.md](AUTOTAG.md)). The tag of a sign is its id, except for the 19 signs of the source that the first tagger already named, which keep that name so that the notes already tagged stay consistent: `pain`=`douleur`, `chest_pain`=`douleur-thoracique`, `abdominal_cramps`=`douleur-abdominale`, `lower_back_pain`=`douleur-dos`, `sore_throat`=`douleur-gorge`, `headache`=`cephalee`, `nausea`=`nausee`, `vomiting`=`vomissement`, `dizziness`=`vertige`, `coughing`=`toux`, `shortness_of_breath`=`essoufflement`, `palpitations`, `diarrhea`=`diarrhee`, `constipation`, `sleep_changes`=`insomnie`, `anxiety`=`anxiete`, `chills`=`frissons`, `fever`=`fievre`, `fatigue` and the two added signs, `itching`=`demangeaison` and `rash`=`eruption`. The `tag` field of `manifestations.json` says it.

## Differences with the reference, on purpose

- No `sucre` keyword for glucose (it read `2 sucres dans mon café` as 2 g/L).
- The last word of a keyword of five letters or more also matches its plural.
- A `dormi` preceded by `pas`, `jamais`, `plus` is no duration (`j'ai pas dormi 3 heures`).
- The reference's reminders (`rappelle-moi de...`) are not health and are not here.
- A level in the same phrase as two signs follows the rule above (the reference left every such case out).

## Tests

`crates/health/tests`: the cases of the reference implementation, rewritten with generic sentences and expected to give the same results (recall of everyday French on 100+ sentences, denial, idioms that stay quiet and real cases that are never hidden, levels, measures, red flags and their numbers), plus properties: no text makes the detection panic, dates and clock times never become values, a comma is a decimal point, every range has its edges. The node's own tests are in `crates/core` (`autotag`) and the janitor's in `crates/janitor` (`health_rule`).

## Changing the referential

Edit the JSON, raise the `version` of the file, and add a test with the sentence that was missed. A keyword added for this project goes in `keywords_added`, so the origin stays readable. Never make a red-flag message name a disease.
