# Changelog

## 20261120.12.0

- **Every new connection of an assistant that signs in starts with an invitation code, on the tailnet too.** The consent page (`/oauth/authorize`) is the same on the public address and on the identity listener: it asks for a one-time code (twelve letters, `XXXX-XXXX-XXXX`) and shows the consent page of the person the code was made for; there is no more shortcut for a person Tailscale recognises. The person makes the code on their own page (`POST /admin/api/invite`, and the « Connect an assistant » steps of Claude, ChatGPT and Le Chat now show it), or an administrator makes it for them. Keys for assistants that take a key, and the pairing codes of devices, are unchanged.
- **A code is one transaction.** The nonce the consent page gives is bound to the OAuth request it was opened for (client, return address, PKCE challenge, address asked), is shown again unchanged when nothing was ticked, and is spent once. A request the server would refuse (another site, a PKCE method it does not take) does not spend the code. At most five codes wait per person. The administrator's route needs `X-Requested-With: memcp` and a bearer token (never the address). The answer that carries a code is `no-store`.
- **The page tests pass again.** The capture form of « Trace or drop » is back, a click in the Journal opens its note, and the people tiles and the timeline are translated.

## 20261120.11.3

- **The administrator's page makes the invitation code.** Each person's tile has a « claude.ai code » button: it shows the one-time code, the address to paste in Claude's connector settings (`<public address>/mcp/<person>-claude`) and the two steps. The answer of `POST /admin/api/users/invite` carries that address as `connector_url`.

## 20261120.11.2

- **The discovery points Claude to the public consent page.** `authorization_endpoint` is now `<public address>/oauth/authorize` (it was the private page, which a person who is not on the tailnet cannot reach). That page asks for an invitation code, and shows a link that takes a person who is on the tailnet to the private page with the same request, where Tailscale says who they are and no code is needed.

## 20261120.11.1

- **The invitation endpoint follows the rest of the control plane.** `POST /admin/api/users/invite` takes the console token of an administrator of the installation (a member's token is refused), and is never served to a request that came through Funnel.

## 20261120.11.0

- **A person who is not on the tailnet can approve a connector (Claude on the web) with an invitation code.** An administrator makes the code with `POST /admin/api/users/invite {"name": "<person>"}` (a registry console token, or their page): twelve letters shown once as `XXXX-XXXX-XXXX`, good for one hour and one use, kept as a digest in memory only (no database change; a restart forgets them; twenty at most wait at once). On the public address, `GET /oauth/authorize` now shows a form for the code instead of a refusal; the code opens the consent page of that person alone, and the approval carries a nonce tied to them, with the same origin checks as on the tailnet. Wrong codes are counted per address like wrong keys (ten in fifteen minutes hold the address off); a redirect address that is not registered does not spend a code. The tailnet way is unchanged.

## 20261120.10.0

- **`memcp space rename NAME NEW [--yes]`**: a space can be renamed, which it could not (only its label and place changed). The database, the folder of its files and every row of the control database that names it follow (owner, members, grants and path limits of the connections, folders, sync state and policy, approvals, sub-spaces), so a key keeps its access. It counts first and renames with `--yes`, while the node is quiet and after a backup. A box of the old kind (`a-trier`) comes out as `inbox`, labelled « Inbox ».
- **The Inbox can be given another label** (`space set`); its kind, place and rules stay its own.

## 20261120.9.0

- **The Inbox has an English name.** Its kind is `inbox` (it was `a-trier`, French, in the code and the apps), and a box made now is named `<person>-inbox`. The old kind and the old name `<person>-a-trier` are read as the Inbox everywhere (the memory, the janitor, the sync between nodes, the page), so a memory made before keeps working as it is until its box is renamed. A custom kind cannot be called `inbox`.

## 20261120.8.0

- **The `stats` tool keeps the tag vocabulary of a space while it holds the same notes.** The vocabulary (every tag of every note) was counted again at each call, 100 ms or more on a large space. It is kept, with a signature of the space (notes, tags, last write) that any write, retag, retirement or purge changes.

## 20261120.7.0

- **A denial told by a verb or an absence is not a red flag.** « il niait les idées suicidaires », « elle nie toute idée suicidaire », « absence d'idées suicidaires », « pas d'idée suicidaire » no longer raise `alerte/suicidal_ideation`; up to two article words (`les`, `toute`, `idée`…) may stand between the negation and the sign. In doubt it still fires.
- **`memcp purge --allow TAG`** lifts the protection of the named tags for one purge (a referential filed under `journal` by mistake, say).

## 20261120.6.1

- **A sign taken off with a whole list of tags leaves the mark too.** `tags_set` with `tags` (the list that replaces all of them) marks a `signe/<sign>` it leaves out, as `remove` does: the tagger and the night sweep keep that sign off the note. An engine's pass (`learn: false`) still leaves no mark.

## 20261120.6.0

- **A sign the person takes off a note stays off.** Correcting a note with `sign_set` (or `tags_set`, by default) leaves the mark `signe-refuse/<sign>` beside the note's tags: the node's tagger and the janitor's night sweep no longer put that sign back on that note, whatever the person writes there afterwards. Adding the sign again with `sign_set` takes the mark off. A pass that says `learn: false` (the sweep, an engine) leaves no mark and clears none.
- `signs_review` shows those notes too, with `refused`: the signs the person took off, so that one can come back with `sign_set`.
- **The language of a note is read before its signs** (`memcp_health::lang_of`, used by the node's tagger and by the janitor): French alone when the text looks French, every language at once otherwise. A note once taken for French because of `de`, `la` or `un` (words three languages share) is now read with its English signs; `Müdigkeit` is no longer French.

## 20261120.5.0

- **`space_view` takes `text_limit`** (register and chronology): each item's `text` is cut to that many characters and `text_truncated` says so. A page of 50 notes was 353 KB; a list that only draws titles can ask for 160.
- **The stats of a space are twice as fast** (`stats` view of a 11 000 note health space: 188 ms to 94 ms): the counts by sign, measure, board and source read the index of tags by range instead of matching every tag with `LIKE`.
- **The sign « envie d'acheter » no longer matches `besoin d'acheter`** (an ordinary shopping need).

## 20261120.4.0

- **A PDF in a library is read.** When `MEMCP_PDF_TEXT_COMMAND` names a program (a program and its arguments, `{}` where the file goes: `pdftotext -layout {} -`, or an OCR such as `ocrmypdf --sidecar - {} /dev/null` for a scanned book), the text of each PDF is indexed like a text file and found by `library_search`. It runs with no shell, for 120 seconds at most per file. The Home Assistant app ships `pdftotext` and sets it.

## 20261120.3.0

- **The keys and numbers of a JSON dump are no longer tagged.** A note that is a JSON object after a short label (`[Kalios] charge 2025-12-10: {...}`, a day dump) is tagged from the sentences inside it only; the keys, numbers and booleans made tags such as `acwr`, `true` and `warming`. The words of a day dump (a dictated line) are still read.
- **`memcp tags remove --space S --tags A,B [--only TAG] [--yes]`** takes tags off the notes that carry them (each note keeps its date, source and origin); it counts first.
- **Names and units for the numbers a health app sends that the referential has no measure for** (Apple Health constants: `hrv_sdnn_ms`, `steps`…): an item of type `measure` gets their label and unit.
- **The chronology of a money space no longer stops early** when a day holds more movements than a page.
- The janitor's signs sweep (still off unless asked for) dates a turn by its first page, reads at most 100 pages a pass, and never teaches the node.

## 20261120.2.0

- **A note with several measures shows all of them.** In the `chronology` of a health space an item of type `measure` carries `measures` (`[{measure, value, unit, label}]`) with every `mesure/*` tag of the note; the first stays in `measure`, `value`, `unit` and `label`.
- **A measure is named by its tag as well as by its id** (`pouls` and `heart_rate`), and a measure the referential does not know is named by its key, readable.
- **The referential reads `sommeil`** (`sommeil 7,2 h`, `sommeil 7 h 11`) and `sleep`, besides `dormi` and `slept`; the `%` of the saturation is kept as its unit.

## 20261120.1.1

- **A retag keeps the note.** `tags_set` and `sign_set` replaced a note by a copy with a new date and an agent's source: it lost the date, the author, the path and reference it was imported with, and its origin. The new version now keeps them all.
- `memcp repair provenance --space S --reason PREFIX` puts back what an older retag lost, from the version each note replaced; it counts first and writes with `--yes`.

## 20261120.1.0

- **The janitor's signs sweep is off unless asked for** (`signs_sweep = on`). It rewrote notes with `tags_set`, which gave them a new id and the date of the pass and dropped the path and reference they were imported with; a review found it. It must keep the identity and the provenance of a note before it runs again.
- **`memcp purge`**: removes notes for good, by id, on the machine of the node (it is no tool): every old version, the tags, the history, the search index, the origin and the copies kept in the log for the other machines, with the ids remembered so that a machine that still has them does not bring them back. Journals, boards, rules, indexes and `dossier/kalios` are protected; a note with a stored file stays. It counts first and removes with `--yes`; `--vacuum` compacts the file. See [CLI.md](docs/CLI.md).
- `supersede` learns only from the tags the person gave, not from those the node added.
- The tree of tags in the page no longer follows inherited properties (a tag named `__proto__`).

## 20261119.2.1

- **A fresh memory gives its usual spaces their kind.** `memcp init --defaults` and `setup` made `health`, `home`, `finances`, `hobbies`, `work` and `personal` as custom spaces, so `target_set` answered « health is not a health space » until `space kind` was run. They are now made as health, home, money, hobbies, work and me spaces, and the toolkit of their profession works at once. A memory that already exists is not touched.

## 20261119.2.0

- **Each space is a profession, with its toolkit.** `space_view capabilities` carries `profession`, the tool `skills` lists the professions of the spaces a key reads, and the instructions of the MCP server say in a line what each kind of space is an expert of. A kind may have several profiles (health: nurse, coach, doctor); the person adds or replaces profiles in `professions.json` beside the memory. See [PROFESSIONS.md](docs/PROFESSIONS.md).
- **Health tools.** `target_set` sets or changes a target as a nurse writes it (the problem in one or two words, then a precision; a title that states an aim is refused; signs of the referential only), `signs_review` shows the signs put on notes with the words that brought them up, and `sign_set` corrects or removes a sign (the node learns from it). `living_set` gains `update`: a card changes in place and keeps its id and its history.
- **Money.** `finances_overdraft_exit` estimates when an overdrawn account comes back above zero, from the average net of the last complete months.
- **Hashtags.** A `#word` is tagged first and never squeezed out: in any space a word of the health referential is a sign or a measure and puts the root tag `sante` on the note; any other word is a tag of its own; a note may carry many.
- **Professions of your own.** A profile may apply to named spaces, carry reference material and skills that are recipes (`steps`, `template`, `tags`), so a game master for role-playing games, say, is a profile and not code. An assistant proposes one with `profession_propose`; it does nothing until the person accepts it (`memcp professions accept KIND/ID`).
- **The professions as Claude skills.** `memcp skills-export DIR` writes a plugin marketplace (one `SKILL.md` per profile) built from the registry.

## 20261119.1.0

- **The janitor tags the signs and symptoms of the notes already in the memory, wherever they came from.** Each night it goes through the notes of the health spaces and adds the `signe/*`, `mesure/*` and `alerte/*` tags of the shared referential to those written without them (imports, syncs, assistants limited to paths), 600 per pass, never touching the text, the raw layer of an app or a reference document. A short note also teaches the node its phrasing. A note that looks French is read in French, so `pain` (bread) is no longer a sign. See [JANITOR.md](docs/JANITOR.md).
- **The tagger also runs when an assistant corrects a note.** `supersede` with a new text gives the new text its signs, measures and alerts, as `remember` does; the tags the note had stay.
- **A sign for the urge to buy.** `purchase_urge` ("Envie d'acheter", "Urge to buy") joins the health referential (manifestations, version 2), so a target such as compulsive buying has a canonical sign to carry.

## 20261117.0.37

- **The Targets card is localized.** Column names, move buttons, amounts, percentages and deadlines follow the page language (`25,7 %`, `15 déc. 2026` in French); the title no longer stays French in English. A card with only a deadline no longer shows an empty amounts row.
- `metrics.currency` of a target is upper case (ISO 4217), whatever the case it was written in.

## 20261117.0.36

- **The space page keeps every view.** Boards, Notes, Keywords, Tree and Graph are back beside Targets, Journal, Stats and Inventory; the Tree and Targets pages no longer print a stray `null`.
- **Finance stats carry the bank movements.** `stats` in a money space adds `finances.months`: income, outflow and net of the last six months.
- **A merged chronology says when a source has more.** `page.has_more` stays true when either the notes or the movements were cut at their own page size.
- Targets keep their stable card id across a move, now covered by a test.

## 20261117.0.35

- **The cockpit opens on Targets, with one chronology, Stats and an Inventory.** `space_view` (`memcp.view/1`) now serves a space's home (`targets`, with figures and progress), a merged journal/register chronology, `stats`, the file inventory and, in Home and Hobbies, checkable tasks. Financial goals carry a deadline, a target amount and a realized amount; a card's figures are set with `living_set` and survive a move. The node page (Windows, Linux, Home Assistant) has a persistent space bar, the four views and one + button for a note, a photo or a file; no Dashboard or Register page is duplicated. The same typed contract and the tools actually available are announced to the Apple apps, the desktop clients and the assistants over MCP.

## 20261117.0.34

- Health targets accept only signs and symptoms from the shared referential.
- Blood pressure detection reads dotted French abbreviations and spoken diastolic values.

## 20261117.0.33

- **Deleting, archiving, restoring, making, merging and adopting spaces survive a crash.** Cut off at any instant, each one ends done or not done when the node starts again: a deleted space never comes back from another machine, an archive always restores, a merge finished after a crash doubles nothing, and no half-made space blocks its name.

## 20261117.0.32

- **A connection made on a node served on another port works.** Without `--public-url`, the server and the manager hand out the address they listen on (a wildcard host becomes 127.0.0.1), not port 8791: the Mac app, which serves on 8799, no longer throws its connection away.

## 20261117.0.31

- **The memories of a Home Assistant installation are repaired at start too.** The manager, `memcp-sync` and the apps' own node now run the start-up repair pass on every memory they open, as the server did: a crash there is finished or undone before anything is served.

## 20261117.0.30

- **A crash loses nothing and doubles nothing.** A file put, retired or erased when the machine stops is either fully done or not done; an erased recording's audio goes from every machine even if the node was killed mid-way; big files left out for an older machine are sent once it updates, even after a crash; leftovers are cleaned at start.

## 20261117.0.29

- **Moving a big tag no longer stalls or locks.** `notes_move` goes by batches of 1000 notes (call again until nothing remains), a cut-off or doubled call never fails or duplicates a note, a crash mid-move is finished at the next start, and refusals say why; `supersede` keeps the kind of a note and can add tags.

## 20261117.0.28

- **Space reads remind assistants to write a journal handoff.** MemCP repeats the reminder in the tool response, even when the client did not show the initial guide. One entry per topic brings together the important reasoning, any advice given and the agreed next step.

## 20261117.0.27

- **ChatGPT and Le Chat connect.** Adding MemCP in ChatGPT (and in Le Chat) was refused: the node accepted only Claude's return address. It now accepts theirs, and names itself in every sign-in answer, as ChatGPT requires.
- **Big files reach every machine.** A recording or a document of up to 2 GB (24 MB before) goes between your machines in pieces of 4 MB, each checked; a transfer cut halfway goes on where it stopped. A machine not yet updated keeps getting files up to 24 MB, and the bigger ones once it is. Big files come in from a watched folder.
- **Choose which spaces go to each machine, and which way.** On a machine, « spaces and ways » sets each space both ways, send only, receive only or no sync, and the choice for spaces made later; health and money are marked. Nothing changes until you choose: every space still goes both ways (`memcp node policy`).

## 20261117.0.26

- **Disputes are shown and resolved.** A note changed differently on two machines is counted and listed in Machines, with every version; you pick the one to keep, the others are retired (never erased) and every machine follows. An assistant sees the note flagged in dispute and resolves it only on your word (`disputes`, `dispute_resolve`).

## 20261117.0.25

- **The Inbox warning counts what waits.** An assistant that connects is told how many notes and files wait for a decision in the Inbox, counted as `triage_waiting` counts them, not every fact the Inbox space holds (keywords, a reader's dossier, recordings already transcribed).

## 20261117.0.24

- **Connecting an assistant is three short steps.** ChatGPT and Le Chat (Mistral) have their own steps, with the menus they use; Claude is one choice for the web, the app and the phone. Each step says only what to do, and the text that sets your memory up is folded away until you want it.

## 20261117.0.23

- **The health referential is in the node.** The tagger reads one referential of signs, measures, vital constants and red flags (`health_referential`, docs/HEALTH.md) instead of its own patterns. Every tag it gave before is still given.
- **Transcription knows what to expect.** The node gives the speech model the names, keywords and terms of your notes (`transcription_hints`, setting `transcribe_hints`), so they come out right.
- **Call recordings carry their participants.** A recording says who called whom and with which app; its transcription inherits it, as fields and tags.

## 20261117.0.22

- **What a peer could not keep is never taken as kept.** When a node refuses an event, or leaves it out of its window, the sender now knows it and keeps that event, so nothing is lost when the phone trims its 30 days. An event that waits for one that never comes no longer stops a space for ever. A space of another person with the same name is never mixed with yours, a merge reaches the other nodes as it is, and a file is at most 24 MB.
- **A machine is what made it, not its name.** Only a key made by a pairing code, a join you allowed or the node's own command line can sync, invite or answer a device; an assistant's key is refused there whatever it is called, and the name `peer-…` is kept for machines. A device joins with a single-use ticket that lasts at most ten minutes, too many waiting requests from one address are turned away, an OAuth consent never takes over another connection, and a space's folder can be set only from the machine itself and never in a folder of the system or of keys.
- **Your devices keep syncing after the update.** A machine already in step stays a machine: the node recognises it by what made it or, when that proof is gone, by its past syncs as a machine with full rights. A connection the node cannot recognise this way has to join again.

## 20261117.0.21

- **A device joined to the swarm shares its person's Inbox and spaces**, whatever the person is called on each node: the sync works for the person who owns the spaces of this node, not for a fixed internal user. What a pass could not do is kept and shown (`last_pass` in the sync status).

## 20261117.0.19

- Adding a node from any node, like Syncthing: each node has an id and a link (QR); a request shows the same three words on both sides and on every peer of the person; an introducer ticket needs no words; the page lists this node, the nodes and the requests, the older codes are under Advanced (routes and forms in docs/CHANGELOG-API.md).

## 20261117.0.18

- `notes_move`: move the notes of a tag to another space by MCP; `space_create` and `space_move` documented.
- The keyword learning ignores more empty words (French, English, Spanish).

## 20261117.0.20

- **The command line speaks to a running node:** `memcp --node URL --key-file F <command>` for every command (docs/CLI.md; exit codes; the table of local and remote forms is tested).
- **A folder for each space, on each node:** `folder_set`, `folder_get`, `memcp folder set|get|sync`, a block on the page; the node keeps the notes and files of the space there as Markdown, both ways; never shared with the other nodes; cloud folders are flagged (docs/FOLDERS.md). `library add` is an alias.
- **« À trier » is now called Inbox**, in English, French and Spanish: on the page, in the tool descriptions and the instructions given to assistants, in the janitor's notes, the command line and the docs. The box is stored as before (kind `a-trier`, space `<person>-a-trier`), so a box made by an earlier version is the same box: nothing is renamed or migrated, and the swarm still pairs the boxes of a person by kind, whatever their names. A spaces file whose default space is still labelled « À trier » is read as « Inbox ».
- **Every node is equal, and the words say so:** the page, the consent page, the command line, `doctor` and the docs no longer call a node « the server », its page « administration » or « the admin page », or a device a « client » of another. The people section of the page is « People of this node ». The `/admin` path, `--console-admin` and `--admins` keep their names.
- No « vault » left in what a person reads: the consent page, `doctor`, the command line and the janitor's notes say space. `memcp space make KIND` is the new name of `vault add KIND` and `import --folder DIR` of `import --vault DIR`; the old forms still work.
- **A bench of real nodes** (`scripts/bench.sh`, docs/BENCH.md): three nodes of one person writing at once, load, security probes and `kill -9`, on macOS and Linux, with a summary table. What it found is fixed below.
- **Swarm:** a node that joins a second node on the same host (or one whose name starts the same way) keeps both peers instead of replacing the first; two passes of one node that make the same space at the same moment both end clean; a box offered by a peer of another person can no longer turn a space of this person into a box; the sync loops a node starts end when the node is killed instead of piling up at the next start.
- **The node keeps answering:** requests without a key that keep their body waiting have places of their own and no longer turn away assistants and peers; a connection silent for 30 s is closed (`MEMCP_READ_TIMEOUT_SECS`) and a keyless body has a deadline; a body that stops before its announced length is refused (400) instead of being acted on; a request line or a header is at most 8 KiB, 100 headers at most (one connection could fill the memory).
- **Load:** SQLite is built without its process-wide memory count (`.cargo/config.toml`): with 32 clients a node answers about six times as many calls as before, and more clients no longer means fewer answers.

## 20261117.0.17

- **The swarm:** every node of a person is equal. A space or sub-space made on any node reaches the others with its label and parent; a rename, a move or an archive follows; the person's À trier is one box across the nodes, so a dictation captured on a device reaches every node, and a note filed from it by two janitors is one note. Peers that joined earlier reach the spaces made since.

## 20261117.0.16

- **A note sent out of À trier is tagged like one written in its space:** the tag of the space, then what the text says there (signs, levels and measures in a health space, the space's own keywords elsewhere). Notes dictated on a device, which wait in À trier first, now get their tags when the janitor or the person files them.

## 20261117.0.15

- **The node keeps answering:** a request declaring a huge body no longer aborts it, clients that declare a body and send nothing no longer hold its workers, and a burst of connections kept open no longer leaves later ones unread (the HTTP server tiny_http is kept in `vendor/` with these fixes).
- **Rights and data kept through an archive:** a connection limited to paths comes back limited after a restore, and the last writes still in the write-ahead log are archived with the space.
- **Files:** a file whose description reads like a note already there gets a note of its own instead of taking that one over.
- **Facts are at most 1 MiB**, the most a peer takes in a sync; a sync pass reports the events it could not read.
- **Janitor:** no panic on a long recording name with accents; the hour of waking is read from any app's day note.
- Tools, errors and the instructions given to assistants say *space*, not *vault*.

## 20261117.0.14

- Automatic tags in the core: every write gets the tag of its space and the tags proper to it (signs, levels and measures in a health space); `tags_suggest`, `tags_set`; the node learns from corrections (docs/AUTOTAG.md, docs/CHANGELOG-API.md).
- A board written by an app is no longer « behind » by every note of the space.
- No app name in the examples of the core and the docs.

## 20261117.0.13

- **Every node answers a device asking to join:** a peer of the person lists the requests waiting (`GET /mcp/<connection>/join/pending`) and allows or refuses them (`POST /mcp/<connection>/join/approve|deny` {id}) with its own key, as the page does. The apps show the request where the person is.
- **Journal:** every note of every space, À trier included.
- **Page:** golden paths tested against a real node in CI; badges, empty parts of Settings and French fixed.

## 20261117.0.12

- **Sync:** after a copy larger than a page, a note written on this node leaves on the next pass. A page holding only the peer's own events no longer ended the push, so a new note waited dozens of passes (found by an end-to-end test against a real memory; a test now copies 1 200 notes then writes one).

## 20261117.0.11

- **The page in four sections**, the same as every app (docs/UI-SPEC.md, rewritten): *Trace or drop* (write, record or attach, into À trier, which every memory now has from the start), *Journal*, *Spaces* (health is a space among them), *Settings* (machines, assistants, tools, safety, activity, files, advanced).
- **Machines:** a node renamed never comes back as another machine under its old name; the states of a machine are translated; no « null » button.
- **One kind of device:** no capture apps on the page; a code pairs a peer of every space.

## 20261117.0.10

- **Device invitation on a multi-person installation:** the invite of a peer reached at `/mcp/<person>/<connection>/join/invite` is passed to that person's memory, and `POST /join` with a ticket (and its polls) goes to the memory that made the ticket. Before, a ticket was looked for in the first administrator's memory only and was « unknown » for everyone else.

## 20261117.0.9

- **Peer directory, as the apps read it:** the node's internal connections are not listed among the assistants; a connection used through Tailscale Funnel is in the cloud; each memory of a multi-person installation goes by the installation's name and says how it can be reached.
- **Voice:** a transcribed recording is erased as soon as its text exists.
- **Device invitation:** a peer invites another device of the same person with a one-time ticket (`POST /mcp/<connection>/join/invite`).
- Hermes connects through the MCP like every assistant; its plugin is removed. Plan for a voice assistant at home (docs/ASSIST.md). 5S pass: words standardised in the capture program, internal notes removed.

## 20261117.0.8

- **Audit of the core** (docs/reviews/2026-11-17-core-audit.md): a connection limited to tag paths can no longer read or write the log of a space through the sync routes; joining another memory from the page is for administrators; a wrong key with a large declared body counts as a failed attempt; hostile file names and types from a peer are refused; databases opened with trusted_schema off; the failure guard is bounded; OAuth is rate limited; the join check phrase uses all its words.
- **Peer directory:** every node knows every peer of the memory (name, last activity, version, how it can be reached, which node told it); `peers` and `GET /sync/status` return the union, with `behind` for older versions; assistant connections carry a kind (local or cloud).
- **Page:** sub-spaces inside their folder, a global journal, a graph of spaces, tags and keywords, assistants listed apart from the machines, French complete.
- **Janitor:** the names of spaces are tags; "to file in finances and health" copies to both; sub-spaces get their keywords and the most specific wins.
- **API:** every public item of the core is documented (docs/CORE-API.md), docs/TOOLS.md is exact for all tools and checked by a test, an error vocabulary with HTTP statuses.
- Conformity table (docs/CONFORMITY.md) and the 5S notes (docs/FIVE-S.md).

## 20261117.0.7

- The add-on is no longer on the host network: its ingress port clashed with another process on the host and the node did not start. Advertising on the network is left for later.

## 20261117.0.6

- The bridge is now the capture program: crates `memcp-capture` and `memcp-capture-ffi`, binary `memcp-capture` (the commands are the same).
- A replica can hold only the last N days (`memcp-sync --window-days N`; pruning is local and never touches what no peer has acknowledged).
- A device the person allowed in advance joins at once, once (`/join/allow-device`).
- The page: machines first with live peers, folders, French and English, light and dark, a first screen with two choices, keywords and targets of a space.

## 20261117.0.5

- The add-on uses the host network and advertises the node (mDNS), so the apps on the same network find it without an address; option `node_name`. memcp-sync ships in the image.
- More tests: permissions of every tool, migrations from every older schema, concurrent writers, peers in the ways that go wrong, À trier leases.
- `rules_set` with rules a space already has is no longer an error.

## 20261117.0.4

- Vocabularies: a list of terms (signs, symptoms, project words) imported into a space (`memcp vocabulary import|list|remove`, MCP `vocabulary_terms`); the janitor reads them after the keywords of the space.
- The janitor claims the notes of À trier (`triage_claim`) and gives them back at the end of its pass, so that a sorter on a device does not take the same notes.
- A busy node answers 503 with Retry-After.

## 20261117.0.3

- One address is enough to connect an assistant: the key can ride in the address (`/mcp/<connection>?key=…`), and the page shows that address when a connection is made.
- CI no longer builds the Metal-only speech feature on Linux and Windows.

## 20261117.0.2

- Voice commands by keywords: a note that starts « j'ai une idée », « il faut », « je dois », « problème », « il manque », « acheter »… also becomes a card of the space's ideas, what's next or targets, or of the home shopping list, with the note as its source.
- A board's first pass reads the newest notes, so a memory that starts with a large import is not read from its archive.

## 20261117.0.1

- The janitor files by keywords, without a model: a word that belongs to one space sends a note there at once, ordinary words score, a tie or nothing leaves it in À trier; each space's keywords live in its `rules` note, versioned. A model may only propose keywords.

## 20261116.0.9

- Reliability: writes from many devices at the same time are no longer lost (« database is locked »); every write waits its turn.
- An end-to-end test against real nodes: simultaneous writers, three nodes converging, refused access, a 2000-note catch-up.

## 20261116.0.8

- The page shows the living boards of a space (Boards tab): cards by column, moved in one click, added in a field; a target shows the signs it groups; each board says its last pass and how many notes came since.
- Named lists in every space (`list-shopping`…), fed from notes of any space.
- An app's structured trace is read as values in the journal and the notes; board notes stay out of the journal and the counts.

## 20261116.0.7

- Living boards in every space: *what's next* (now, next, later, done), *ideas* (new, kept, done), *targets* (long-term problems and risks, each grouping its signs and symptoms; open, reached) and *where things stand*. Cards are one-line notes moved between columns, each keeping its history; each board records its last pass and shows the notes filed since. MCP tools `living_get` and `living_set`.
- Machines of different versions: the exchange between peers has a protocol number; an older machine that meets a newer protocol stops and says so.
- Docs: the concepts with cases (CONCEPTS.md), the specification of the boards (LIVING-PAGES.md).

## 20261116.0.6

- Machines in step: a node serves `/sync/status`, `/sync/spaces` and `/sync/events` at the address of a peer's connection, and `memcp-sync` keeps a node in step with a peer (pull, then push, cursors that resume). Tested end to end with two real nodes.
- `memcp merge --plan/--apply`: merging two memories without loss.
- Libraries: a folder mounted read-only, indexed, searched (`library_search`), raised into the memory (`library_raise`); `links` gives the graph.
- On the page, the notes of a space read as a thread, a day at a time: the person's notes on one side, what assistants and apps wrote on the other, an app's structured trace shown as its values.
- Docs: the principles in the README, local tools on every machine (LOCAL-TOOLS.md), Claude Code on the same computer.

## 20261116.0.5

- MCP tools `rules_get` / `rules_set` (filing rules of a space, versioned) and `peers` (the machines holding the memory).
- Interface specifications for every app: docs/UI-SPEC.md.

## 20261116.0.3

- `triage_claim` and `triage_release`: a sorter outside the memory (an assistant on a device) reserves notes of À trier for a few minutes, so that two sorters never take the same.
- The pairing text no longer says everything lands in À trier.
- `/mcp/<person>/<connection>` is accepted beside `/mcp/<connection>`: the address says whose connection it is.
- On the page, joining a network comes first and connecting an assistant second.

## 20261116.0.2

- `memcp init --defaults`: the usual spaces (personal, health, home, finances, hobbies, work) in one go.
- An assistant that connects is told when notes wait in À trier, and asked to tell the person and offer to sort them.
- The janitor's own connection no longer shows among the connected assistants; `spaces` counts 0 for an empty folder.
- Planned in docs/MERGE.md: merging two memories.

## 20261116.0.1

Versions are now the day of the release, then a counter: `AAAAMMJJ.0.N`, for every app.

- The `spaces` MCP tool: the tree of the spaces a connection can read, with label, parent, kind, rights and counts.
- The add-on has an icon and a logo.

## 0.25.0

- **The spaces as folders** on the page: a tree you open and close, with drag and drop to nest or reorder, a `+` to make a space inside another, rename in place. Opening one shows its **tags** and its **keywords**; drag a tag onto another space to send the notes it carries there (previewed first, never erased: each note is retired in the source with the reason "moved to …"). The tags that file notes in a space are kept in its settings, with a history.
- **Joining by request**: a device that finds a node asks to join, the person compares **three words** on both screens and allows it; the key comes once. The join window on the page says **Connected** with the name as soon as a device joins, and waiting requests show up as a banner and a browser notification.
- **Discovery**: a node advertises itself on the local network (`--advertise`, mDNS `_memcp._tcp`) and answers `/.well-known/memcp`; `memcp-bridge discover` finds nodes on the network and on the tailnet, and a Home Assistant installation by its own names; `memcp-bridge join` asks one to let this computer in.
- Moving notes between spaces by tag in the core (`move_by_tag`).
- A shorter, plainer README; the full manual is in `docs/REFERENCE.md`.

## 0.24.3

- The Home Assistant app takes in the data left by another installation (a former HemCP app) in its own configuration folder, once, keeping every key and address (see DOCS.md, *Moving from another app*).

## 0.24.2

- **Windows and macOS are first-class**: the whole test suite runs and passes on Linux, macOS and Windows in CI, and the release carries the programs of the three systems.
- **Fix (Windows):** an export listed its files with backslashes while its manifest and its archive use forward slashes, so a fresh export failed its own check and the page's "export my data" was refused; resolved paths lost the `\\?\` prefix that SQLite's `VACUUM INTO` takes badly.
- A flaky bridge test waits for its port to close.

## 0.24.1

- **Fix:** `memcp-bridge transcribe` refused to start with a speech service and no local model (`transcribe needs whisper_model`). It now needs a local model or `whisper_api_url`, so the Home Assistant app's voice pass works. Checked end to end: a voice note recorded on the page, read through a speech service, becomes a dictation note in the inbox.
- `memcp-bridge connect ADDRESS` carries an assistant's standard input and output to the bridge's HTTP gateway on this machine (the way on Windows).
- The test suites run on macOS and Windows in CI; the Unix-only tests are gated.

## 0.24.0

First release under the AGPL-3.0-or-later (with a commercial licence available); earlier versions stay Apache-2.0.

- **Voice notes and attachments on the page of a node**, the same functions as on the computers: **Record** keeps a voice note from the browser (WebM, or m4a on Safari), **Attach files** keeps photos, PDF, text and recordings, each as a note of the space with its file, which can be opened or played.
- **Voice notes read by a speech service**: the bridge can send a recording to a service of the OpenAI kind (`whisper_api_url`, `whisper_api_model`, `whisper_api_key`) instead of a local speech model, and the Home Assistant app can run that pass (`transcribe` options), so that a voice note recorded on the page becomes text there too.
- The desktop bridge (`memcp-bridge`, `memcp-bridge-ffi`) joins this repository, and the add-on image carries it.
- The Home Assistant app takes the page's new explorer: dashboards, tree, notes and their history, and the layout of spaces.
- `counts` sections can count the terms cited in the notes and the notes under each child of a tag, in any space.
- Recordings made by a browser (WebM with Opus, fragmented MP4) are accepted as audio.


- The desktop bridge (`memcp-bridge`, and its Swift interface `memcp-bridge-ffi`) joins this repository: one repository for the server and the desktop bridges.
- `counts` sections can count the terms cited in the notes and the notes under each child of a tag, in any space.

## 0.23.1

- The page opens each space: a dashboard (notes day by day; for a health space, the signs and symptoms counted from the journal and the targets), the tree of tags, and the notes, which can be searched, added, corrected and removed with their history kept.
- The layout of spaces on the page: rename, nest, reorder, add a space of one's own.
- The page says *space* where it said *vault*.
- The add-on image keeps `curl` (the options of the app are read with it).

## 0.23.0

First public release, under the name MemCP (the project was developed privately as HemCP up to 0.22.7).

- Spaces can be laid out: a label, a parent and a position, nesting up to eight deep, free creation and renaming, without touching a file or a right.
- Dependency fix: SQLite updated through `rusqlite` 0.41 (RUSTSEC-2026-0459). The minimum Rust version is now 1.89.
- Designs for mounted folders, locking with a passphrase or a key, a swarm of peer nodes, and one configuration screen on every platform (see `docs/`).
- A first security review (`docs/reviews/`).

An installation from before the rename keeps working: the data is read as it is, and the Home Assistant add-on keeps its slug when it is updated in place.
