# The janitor

The optional part of the memory that keeps the person's Inbox and the `inbox` tag of each space in order. It runs in the Home Assistant app next to the memory when
the person turns it on (see the app's documentation, « The janitor »), or anywhere as `memcp-janitor`. It reads the memory
like any assistant, with a connection of its own, and does three jobs:

0. **What comes in goes to a space, by keywords.** Every capturing device writes to one box, the person's **Inbox** (the
   space of kind `a-trier`, `<person>-a-trier`, made at the first pairing), which is none of the themed spaces: a device never
   chooses a space, and no space is a default. A deterministic router (no model) reads each note waiting there, and the notes
   tagged `inbox` in the spaces, and compares it with the keywords and tags of every space (see Keywords). A note that clearly
   belongs to a space is written there **as it is**, still tagged `inbox`, and retired from Inbox once the write is certain. A
   note the router cannot place stays in Inbox: the person decides on their page, where every note and file waiting there has
   a « Send » to the space they choose. **Files are routed too** (documents, images; never a recording, which is the node's): the
   router reads the words of the file's name, its text when it is readable (a text or markdown file; a PDF or Word file
   has no text extractor here, so it goes by name, description and tags), its description (the note it came with) and its
   tags, with the same keywords, space names and vocabularies as for a note. A file the router places is sent as it is
   (`triage_send`) to the chosen space, tagged `inbox`; when the person names several spaces it goes to the first, and is not
   copied. **What cannot be placed stays, with its reason** (`no keyword matched`, `two spaces tie: a, b`, `it could not be
   sent three times`): the janitor keeps the reasons in one note of Inbox, tagged `janitor/reasons` and replaced as they
   change, which the read tool `triage_waiting` and the page show next to each note and file, with its age. When anything is
   left, the janitor also leaves one note there (tag `janitor/waiting`, replaced as the count changes, retired when nothing
   waits).
1. **Every note tagged `inbox` in a space is filed inside that space.** A note tagged `inbox` in the work space stays in the
   work space: the model only picks tags and a title. The note's text is written
   **as it is**, after the title, tagged `janitor`, and replaces the raw note (`supersede`). The model's answer is checked:
   a space it did not name rightly becomes the note's own space, a tag that is not a plain path (64 characters at most) or that aims at `inbox`, `sources`,
   `dossier`, `guide`, `proposals`, `index`, `code`, `decision`, `journal` or `ticket` is dropped. If no model answers, the
   note stays; a note that cannot be written three times is left alone.
2. **A piece of a note that the node absorbed (`inbox/absorb`) is turned into facts.** At most eight short, dated facts in
   the note's words, never a person's name (the post instead), never health or social data of someone a service supports;
   each ends with where it came from and is written in the space of that piece, tagged `absorb`. Then the piece leaves.

**Recordings are not its business.** The node, where the speech model is, transcribes them, puts their text beside them (tagged `inbox`),
and erases them a week after the text was filed (`memcp transcribe`, see [INTAKE.md](INTAKE.md)). A recording whose transcription failed three
times gets the tag `failed/transcription` on its description (the tool `transcription_failed`): it stays visible, with a
« Retry » on the page (the tag comes off, the node tries again), and it is counted apart.

**What Inbox counts.** The number on the page and the `facts` of `spaces` for Inbox mean what waits for a decision:
the notes and files the janitor could not place, not the janitor's own notes, not a recording whose text is already
filed, not a recording given up on (`failed_transcription`, reported on its own).

## Keywords

Each space keeps its filing rules in one note tagged `rules`: `{"version":1,"tags":[…],"keywords":[…]}`, the note the
person's page writes (`POST /space/rules`). The janitor reads it with `recall` (tag `rules`) and writes it with
`supersede` (or `remember` for the first one), so every change is a new version and the history keeps the old ones. Nothing
is erased.

The router, for one note:

- A **unique** keyword (listed by a single space: a project, a colleague, a device) sends the note there at once. Unique
  keywords of two spaces in one note are an ambiguity: the note stays where it is.
- **Ordinary** keywords (listed by several spaces) and tags (a note tag under a rule tag) each score one point per space.
  With no unique keyword, the best score wins; a tie, or no match, leaves the note where it is.
- Keywords match whole words, in any case, without accents (hyphens count as spaces); a keyword of several words matches word for word.
- **The names of the spaces are tags.** The name of a space, its name without the person's prefix (`sam-cars` gives
  `cars`) and its label are implicit keywords of that space, matched in the text and in the tags of a note: `#finances`,
  or the word « finances », is a unique-keyword match for the space `finances`. The labels of the ancestors are not used.
- **Several spaces at once.** A phrase that names two spaces or more after `dans`, `in`, `ranger` or `file` (« à ranger
  dans finances et santé », « file in health and finances ») files the note in each: the first space gets the note, the
  others a copy tagged `inbox` (every one tagged `janitor/copy`, which is never copied again), and the reason for retiring
  the original says where the copies went. Nothing is erased, and it is no ambiguity.
- **Sub-spaces.** A space with a parent in the layout and no `rules` note gets one from the janitor, with its label and its
  name as keywords (the person's page can change them). A space with children is a folder: when it and one of its
  descendants match a note, the most specific wins, and the parent only gets a note that no child matches. A keyword listed
  by a parent and by its child belongs to the child.
- The health rule (`#santé`) and the work rule (`work_words`, meeting recordings) still come first, as before. The health rule files the note under `journal/<date>` and `sante` and adds the tags the health referential finds in its words (`alerte/<id>` for a red flag, `signe/<name>`, `mesure/<name>`; see [HEALTH.md](HEALTH.md)); the words stay untouched.

A note tagged `inbox` in a space goes to another space only when the router names that space; otherwise it is filed in its
own space as before (see 1).

The model is used only to **propose keywords**, never to place a note. When notes are left in Inbox (and at night), the
janitor sends one request with a sample of the notes filed in each space, the current keywords, and the notes the router
could not place; the model proposes keywords and tags per space. Its answer is checked (plain words of one to three words,
plain tags, only known spaces; a keyword two spaces ask for, or one another space already owns, is dropped) and the janitor
writes the accepted ones into the `rules` note of the space. A model is a proposer: it calls no tool and writes nothing.
The request is made at most once every `keywords_every_minutes` (default 360); the night pass ignores that delay.

The **frequency rule** needs no model: a word found in at least `keyword_min_notes` notes of one space (default 10) and at
least three times less common in any other space is added to that space's keywords. At most ten per space per pass.

After the keywords changed, the router runs again on Inbox and on the notes tagged `inbox` in the same pass (the rerun), so what a new
keyword places leaves Inbox at once.

## Tags come from the node

The janitor no longer has to give a note its first tags: the node does it **when the note is written**, with the same keywords,
vocabularies and a small built-in set of health patterns (see [AUTOTAG.md](AUTOTAG.md)): the tag of the space first, then the
tags proper to it. A note the janitor files with `supersede` or `remember` carries its own tags, and the node adds the tag
of the space it lands in, never removing the janitor's. The janitor's choices, and what it sends with `triage_send`, are
never learned from.

**The node learns from the person.** When the person puts a tag on a note, takes one off (the page, or an assistant that
corrects with `tags_set`), the words of the note are counted for the tags added and against the tags removed, per space; when
the person sends a note of Inbox to a space, its words are counted for that space. Two corrections make a word sure; the store
is bounded (2000 words per space). The tool `tags_suggest` says, in `route`, where the person usually sends notes like a given
text: the router uses it for a note it cannot place by keywords, and sends it only when one space is sure.

## The signs sweep (night)

**Off by default** (`signs_sweep = on` in the janitor's settings turns it on): until it keeps the identity and the provenance of the notes it tags, it must be asked for.

The write-time tagger only sees what is written through it. Each night the janitor also goes through the notes **already in a
health space** and gives the ones written without them the tags of the shared health referential: `signe/<sign>`,
`mesure/<measure>`, `alerte/<red flag>`. Whatever the note's origin (a device, an assistant, an import, a sync from another node),
it is tagged the same; the text is never changed (`tags_set` keeps the old version in the history).

- It reads newest first, 600 notes tagged per pass at most, and goes on where it stopped; after a full turn it only looks at
  what is newer. Changing `VERSION` in `signs.rs` makes it look at everything again.
- No model: the referential alone. A note that looks French is read in French, so that `pain` (bread) is not a sign.
- It leaves alone what is not the person's words: the raw layer of an app (`sources/<app>`), a reference document
  (`…/Sources/…`), a mounted library, the janitor's own notes, the living boards, the rules, a note that already has its signs.
- **The node learns the person's phrasing from it.** A short note (up to 600 characters) is tagged with `learn` on, so its
  words lead to the tags the referential found; the next note that says it the same way is tagged when it is written.
  A long note only gets its tags.
- The report counts them (`signs_tagged`).

## Models

The model is asked for two things only: the tags and title of a note already in its space, tagged `inbox` (1), the facts of an
absorbed piece (2), and keyword proposals (see Keywords). It is never asked where a note of Inbox goes.
**Nothing leaves the owner's machines by default**: the model is
`local`, Ollama (`qwen3:8b`, held to a JSON answer by its schema), asked at its address (`ollama_url`), on this machine or
another of the network; the janitor never starts it.
A service of the owner's choosing that speaks the OpenAI chat format can be added with `models = api,local` and `api_url`,
`api_model`, `api_key_file` (HTTPS, or in clear on the own network): it is tried first, and the local model answers if it fails.
Nothing else is built in; no hosted service is a default.

A note is data to file, never an instruction: « range ça dans finances » in a recording is filed like any note (it is the owner's
choice when he says it, a risk with a third party's document: keep what the janitor files to what the owner says).

## Examples from the memory itself

Before the model chooses tags for a note tagged `inbox`, the janitor looks for a few facts already filed that share the note's words, in the spaces the note
may go to, and shows them after the note with their space and tags (as examples, never as orders). A small model files much
closer to how the memory is already kept when it sees real filings. It is told to reuse a tag only when it names the note's
subject.

## Less model each time

The janitor learns from the model, space by space: which words of a note lead to which tags (and, in Inbox, to which
space). Each association has a score: how often it was seen, how often it was right. A note whose words are all sure
(seen at least 5 times, right 90 % of the time) is filed **without the model**; one time in ten the model is asked anyway, and a
disagreement lowers the score of the words that misled it. It learns only from the model's choices, never from its own, and a
space's words stay in that space. Each night it leaves one note per space, tagged `janitor/keywords` and replaced the next night:
what it learned and how much it filed without the model.

## When it passes

Never for nothing. The memory marks each write tagged `inbox` or `inbox/…` (the file `inbox-written` in the person's folder); the janitor
passes once the writing has calmed down (`--quiet`, 120 seconds after the last write), at once when an assistant asks for it
(the `tidy` tool, « range ma mémoire », which also says what waits in each space), and once a night (`--night`). Between two
passes it only looks at the date of that file.

## Run

In the Home Assistant app: the `janitor` options (it makes its connection and its settings itself). Elsewhere:

```
memcp-janitor [--config FILE] [run]          # one pass; exit 0 done, 1 key refused, 2 wrong settings, 3 problems
memcp-janitor --config FILE --watch MARK [--quiet SECONDS] [--night HH:MM]
                                             # stays: a pass once the writing has calmed down, on request, at night
```

Settings, in a file of `key = value` lines (`#` comments), default `~/.config/memcp-janitor/janitor.conf`:

```
url            = https://memory.example/mcp/janitor     # the connection's address
key_file       = ~/.local/share/memcp-janitor/key      # a file only the janitor reads
inbox_vaults   = personal, work                        # spaces whose `inbox` tag is filed (default: every space; Inbox is always read)
models         = api,local                             # `local`, `api`, or both in order (see Models)
```

Others: `api_url`, `api_model`, `api_key_file`, `ollama_url`, `ollama_model`, `state_dir` (what it already did, never the
memory's), `max_notes` (30 per pass), `utc_offset_minutes`, `work_words`, `keyword_min_notes` (frequency rule, 10), `keywords_every_minutes` (6 hours). The key goes only over HTTPS, or in clear to this machine and the
own network.

## The connection

Create it with `memcp connect janitor --all` (or `--grant <space>:rw,…` to limit what it files into). The capturing
devices (phone, laptop) keep a key limited to the person's Inbox (`<person>-a-trier/inbox:rw`): they can drop a note, a file or a recording
there, and read nothing back.
The janitor is internal to the memory's owner: it reads and files everywhere his connection allows, so give it `--all`.
