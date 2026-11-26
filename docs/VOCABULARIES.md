# Vocabularies (a database of terms for the keywords)

Status: built (table, `memcp vocabulary import|list|remove`, MCP `vocabulary_terms`, the janitor reads it after the keywords). The page block and the format adapters are not built.

## What it is for

A space keeps its own keywords in its `rules` note (a hundred at most, chosen by the person or proposed by
the janitor). A health space can profit from a large, standard list of terms (signs, symptoms, diagnoses) so
that a note that says « céphalée » or « dyspnée » is filed with no model, and so that the tags are the
same words everywhere. That list is a **vocabulary**: too large for the `rules` note, so it lives beside it.

## Shape

- A vocabulary is a file the person chooses (one term per line, or `term<TAB>group<TAB>code`), imported with
  `memcp vocabulary import --space health FILE --name NAME --source "where it comes from"`.
- It is stored in the space database as its own table (term, group, code, vocabulary name), never as notes,
  so it adds nothing to the counts and nothing to the journal. A vocabulary has a name, a source, a date and a
  licence line shown on the page; removing it removes its terms and nothing else.
- The router reads the `rules` note first (the person's words win), then the vocabularies: a term that
  belongs to the vocabulary of one space only is a unique word; a term shared by several is an ordinary
  word and only scores.
- Matching is on whole words, lower case, without accents, with the plural and the common forms of the
  language of the space.
- The janitor never rewrites a vocabulary. It may propose to copy a frequent term into the `rules` note.

## Tags at write time

The node also reads the vocabularies when a note is written (see [AUTOTAG.md](AUTOTAG.md)): a term found in the text becomes
a tag, `signe/<term>` in a health space, the plain word elsewhere. The plural is the same word.

## Where the terms can come from

The person brings the file; the app embeds none, so no licence travels with it. Candidates to check
before use, with their licence read on the owner's site (this list is from memory, not verified):
SNOMED CT (a licence that depends on the country and the use), ICD-10 / CIM-10, MeSH and its French
translation, the Human Phenotype Ontology. Each has a different file format: the import takes the plain
list and a small adapter per format can come later.

## Order of work

1. Table and `vocabulary import/list/remove` in `pensine-core`, tests (accents, plural, unique and shared terms).
2. The router reads vocabularies after the `rules` note.
3. The page: a Vocabularies block in the space, with name, source, number of terms, remove.
4. Adapters for the common formats, when a real file is at hand.
