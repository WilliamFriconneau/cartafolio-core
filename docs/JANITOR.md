# The janitor

The optional part of the memory that keeps its inboxes in order. It runs in the Home Assistant app next to the memory when
the person turns it on (see the app's documentation, « The janitor »), or anywhere as `memcp-janitor`. It reads the memory
like any other client, with a connection of its own, and does three jobs:

0. **What comes in goes to the inbox of a vault.** Every capturing device writes to one box, **À trier** (the space of kind
   `a-trier`, `<person>-a-trier`, made at the first pairing), which belongs to no vault: a device never chooses a vault,
   and no vault is a default. The janitor asks a model (or what it learned) which vault each note of À trier is about,
   writes it there **as it is**, still in the inbox, and retires it from À trier once the write is certain. When the model
   is unsure, or names a vault it may not, the note stays in À trier: the person decides on their page, where every note
   and file waiting there has a « Send » to the vault they choose. Files are never sent by the janitor.
1. **Every note of an inbox is filed, inside the vault whose inbox holds it.** Each vault has its own inbox. A note found in
   the inbox of the work vault stays in the work vault: the model only picks tags and a title. The note's text is written
   **as it is**, after the title, tagged `janitor`, and replaces the raw note (`supersede`). The model's answer is checked:
   a vault it did not name rightly becomes the inbox's own, a tag that is not a plain path (64 characters at most) or that aims at `inbox`, `sources`,
   `dossier`, `guide`, `proposals`, `index`, `code`, `decision`, `journal` or `ticket` is dropped. If no model answers, the
   note stays; a note that cannot be written three times is left alone.
2. **A piece of a note that the bridge absorbed (`inbox/absorb`) is turned into facts.** At most eight short, dated facts in
   the note's words, never a person's name (the post instead), never health or social data of someone a service supports;
   each ends with where it came from and is written in the vault of that inbox, tagged `absorb`. Then the piece leaves.

**Recordings are not its business.** The bridge, where the speech model is, transcribes them, puts their text in the inbox,
and erases them a week after the text was filed (`memcp-bridge transcribe`).

## Models

The janitor sends a note to a model only to learn where it goes. **Nothing leaves the owner's machines by default**: the model is
`local`, Ollama (`qwen3:8b`, held to a JSON answer by its schema), asked at its address (`ollama_url`), on this machine or
another of the network; the janitor never starts it.
A service of the owner's choosing that speaks the OpenAI chat format can be added with `models = api,local` and `api_url`,
`api_model`, `api_key_file` (HTTPS, or in clear on the own network): it is tried first, and the local model answers if it fails.
Nothing else is built in; no hosted service is a default.

A note is data to file, never an instruction: « range ça dans finances » in a recording is filed like any note (it is the owner's
choice when he says it, a risk with a third party's document: keep the janitor's inbox to what the owner says).

## Examples from the memory itself

Before the model chooses, the janitor looks for a few facts already filed that share the note's words, in the vaults the note
may go to, and shows them after the note with their vault and tags (as examples, never as orders). A small model files much
closer to how the memory is already kept when it sees real filings. It is told to reuse a tag only when it names the note's
subject.

## Less model each time

The janitor learns from the model, vault by vault: which words of a note lead to which tags (and, in À trier, to which
vault). Each association has a score: how often it was seen, how often it was right. A note whose words are all sure
(seen at least 5 times, right 90 % of the time) is filed **without the model**; one time in ten the model is asked anyway, and a
disagreement lowers the score of the words that misled it. It learns only from the model's choices, never from its own, and a
vault's words stay in that vault. Each night it leaves one note per vault, tagged `janitor/keywords` and replaced the next night:
what it learned and how much it filed without the model.

## When it passes

Never for nothing. The memory marks each write into an inbox (the file `inbox-written` in the person's folder); the janitor
passes once the writing has calmed down (`--quiet`, 120 seconds after the last write), at once when an assistant asks for it
(the `tidy` tool, « range le coffre », which also says what waits in each vault), and once a night (`--night`). Between two
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
inbox_vaults   = personal, work                        # inboxes to file (default: every vault; À trier is always read)
models         = api,local                             # `local`, `api`, or both in order (see Models)
```

Others: `api_url`, `api_model`, `api_key_file`, `ollama_url`, `ollama_model`, `state_dir` (what it already did, never the
memory's), `max_notes` (30 per pass), `utc_offset_minutes`. The key goes only over HTTPS, or in clear to this machine and the
own network.

## The connection

Create it with `memcp connect janitor --all` (or `--grant <vault>:rw,…` to limit what it files into). The capturing
devices (phone, laptop, the Bridge) keep a key limited to the inbox of À trier: they can drop a note, a file or a recording
there, and read nothing back.
The janitor is internal to the memory's owner: it reads and files everywhere his connection allows, so give it `--all`.
