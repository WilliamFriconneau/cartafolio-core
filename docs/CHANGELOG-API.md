# API changes, for the apps

Every new route, tool or JSON form, with the version that brings it. The apps read this file.

## 20261117.0.14

- **MCP `tags_suggest`** (read) `{space, text}` -> `{space, tags, fields, field_tags, all_tags, route: [{space, score}]}`: what the tagger would give for a text, without writing.
- **MCP `tags_set`** (write) `{id, tags? | add?, remove?, fields?, reason?, learn?}` -> `{result: "superseded", fact, added, removed, reason, learned}` or `{result: "unchanged"}`: corrects or completes the tags of a note (a new version; the old one stays in the history; a note that describes a file is refused; a note keeps at least one tag). Used by a device engine (Apple Intelligence through MCP) to correct or complete the tags after the note.
- **Automatic tags on write** (`remember`, each `deposit` record, the description of `file_put`): the node adds the tag of the space first, then the tags proper to it (keywords, vocabulary terms; in a health space the built-in signs and measures). The caller's tags are kept; none is removed. Not applied in À trier, for a key limited to paths, or for plumbing tags (`rules`, `index`, `living/…`, `janitor/…`), and not on `supersede`.
- **Fields**: a level or a measure said in the text is a field, stored as a tag `field/<name>/<value>` (`/` in the value is a path level, the decimal point is `_`: `field/level/2/10`, `field/tension/12/8`, `field/poids/72_5`). Every fact returned by the tools now has `"fields": {name: value}` read back from those tags (`recall`, `read_fact`, `living_get` cards and delta, the journal rows).
- **Learning**: a change of tags (page, `tags_set`, `supersede` with different tags) or a send from À trier teaches the node; two corrections make a word sure. `learn: false` on `tags_set` keeps a correction from teaching.
- **MCP `living_get`**: `first_pass: true` while no pass was ever recorded (then `behind` counts every note of the space). `living_set`: a pass that does not say `covers` has read up to the moment it writes.
