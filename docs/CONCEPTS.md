# Concepts, with cases

What MemCP is made of, each concept with why it is there and an example. The examples use made-up people and places.

## A trace

**What.** Anything kept: a dictated sentence, a typed note, a photo, a PDF, a recording and its transcript, a meeting, a
measure an app sends (sleep, heart rate, a training load), a calendar event. A trace has a date, an author (a person, a
device, an app, an assistant), a space, tags, and a history. It is never overwritten: a correction replaces it and keeps
the old one.

**Why many forms.** Real life does not arrive in one format. A nurse's handover, a boiler's error code and a heart-rate
series are all traces; what they share is *when*, *who* and *where it belongs*, not their shape.

**Context of a trace.** A trace can carry where it was taken (a place, when the person allows it) and the context of
the day (its appointments, read from the calendar at the moment of the note, kept with it). « Le rendez-vous s'est mal
passé » then knows which appointment it was.

**Cases.**
- In the car: « idée pour le projet Atlas : un export PDF des devis » → a voice trace, transcribed, filed in Work, and a
  card in Work's idea box.
- After a meeting: the recording, its transcript, its summary, three actions → four traces tied together, the actions as
  cards in *What's next*.
- An app sends `[HealthApp] charge 2025-11-16: {"load": 45.8, …}` → a structured trace, shown as its values, counted by the
  dashboards, never read as an instruction.

## A space

**What.** Where traces live: health, home, work, hobbies…, nested as folders (Work › Clients › Atlas). A space is also a
boundary of rights: an assistant opened on Work cannot read Health.

**Why.** A person's life has rooms; mixing them leaks (health into work) and drowns (everything in one list).

**Case.** A teacher keeps *École* (with sub-spaces per class), *Maison*, *Santé*. A substitute assistant gets *École* only.

## Sorting: the router and its keywords

**What.** Each space has tags and keywords (its `rules`). A note naming a word that belongs to one space goes there at
once; ordinary words score; nothing decisive leaves it in *À trier*. Deterministic: the same note always goes to the same
place, on every machine.

**Why not a model per note.** A model at the moment a note arrives is slow, costly, and changes its mind; a list of words
can be read, corrected and shared.

**Case.** « Rappeler Atlas pour le devis » → *Atlas* is a word of Work › Clients › Atlas → filed there. « Acheter du
pain » → nothing decisive → *À trier*, until a keyword (*courses*) is added to Home.

**Voice commands are keywords too.** « J'ai une idée pour le projet Atlas » → *idée* sends it to the idea box, *Atlas* to
the space. « Il faut… », « à faire » → *What's next*. « Problème », « risque » → *Targets*.

## The janitor

**What.** The part that keeps the keywords: it looks at what stays in *À trier* and what the person refiled by hand,
proposes keywords (a model on the machine may help), then runs the router again.

**Case.** Twelve notes about *pain*, *lait*, *courses* stayed unsorted; the person filed four in Home → the janitor
proposes *courses* and *pain* as Home's words → the eight others follow.

## Living boards (*tableaux vivants*): what's next, ideas, targets, where things stand, and lists

**What.** Four boards per space, made with it. Each card is a one-line note in a column:

| Board | Columns | Holds |
|---|---|---|
| What's next | now, next, later, done | actions, a verb first |
| Ideas | new, kept, done | ideas not yet actions |
| Targets | open, reached | **long-term problems and risks, each grouping the signs and symptoms that show it** |
| Where things stand | current | a few lines on the state of the space |

Besides the four boards, a space keeps **named lists** (`list-shopping`, `list-books`…, open then done). A list is fed
from notes of any space and from what an assistant was told: « il manque des piles dans le détecteur de fumée » (home),
« acheter du paracétamol » (health), « on n'a plus de café » said to an assistant → three cards on the home shopping list,
each with its source. « Je dois penser à acheter quoi ? » is answered by reading that list.

Each board remembers its **last pass** (when, by whom, how far it read) and shows its **delta**: the notes filed since.
An assistant updates a board from its delta only, card by card, each card citing its note. The person ticks, moves or
changes cards in the app.

**Why.** A person and an assistant both work best from a short, current, sourced board plus what changed since, rather
than from thousands of notes. Cards one by one mean two writers never overwrite each other; every card keeps its history.

**Targets, the heart of it.** A target is what a nurse calls a *cible*: a problem or a risk that lasts, with the signs and
symptoms that show it, followed over time. In Health: *lombalgie chronique* grouping *douleur lombaire*, *raideur
matinale*, counted day by day. Elsewhere the same idea holds: in Home, *the boiler keeps failing* grouping *error F28*,
*no hot water*; in the car space, *the battery is weak* grouping *slow start*, *warning light*. The dashboards count the
signs of each target, so a trend shows before it becomes an emergency.

## The 5S of a memory

The lean method of a workshop, applied to what one keeps:

| S | In a workshop | In the memory |
|---|---|---|
| Sort (*seiri*) | throw away what is useless | À trier empties; what is wrong is retired (kept in history, out of the way) |
| Set in order (*seiton*) | a place for everything | spaces and their keywords: each trace has one place |
| Shine (*seiso*) | clean | the janitor merges near-duplicates, the boards follow their delta |
| Standardise (*seiketsu*) | the same way everywhere | four boards in every space, the same rules format, the same screens on every machine |
| Sustain (*shitsuke*) | keep it up | every board shows its last pass and its delta; a review now and then archives what is done |

## Peers and versions

**What.** Every machine (Home Assistant, a Mac, a PC, a phone) runs the same program and holds a copy of the memory.
They exchange events (a trace added, replaced, retired) and resume where they stopped. Machines of different versions
work together as long as they speak the same exchange protocol; an older one that meets a newer protocol stops and says
*update this machine*, never guesses. Updates start with the Home Assistant node, then the computers, then the phones.

**Case.** The Mac is closed for a week; the iPhone and the server kept writing. The Mac opens, asks each space for the
events after its cursor, applies them, sends its own: the same memory everywhere, nothing written twice.

## Local first

Each machine sorts, transcribes and summarises with its own tools (Apple Intelligence on Apple devices, a local model on a
PC). A remote service is a choice, never a default. See [LOCAL-TOOLS.md](LOCAL-TOOLS.md).

## From a note to the calendar

A note that says something dated (« dentiste jeudi à 14 h », « Zelda sort le 4 novembre ») is not pushed into a calendar
by the memory. An assistant connected to the person's calendar proposes the event; on an iPhone, the app can prepare it
with Apple Intelligence as a calendar file (.ics) the person adds with one tap. The person decides.

## Examples by space

| Space | Said or written | What happens |
|---|---|---|
| Home | « Je dois aller à la déchetterie » | a card in *what's next* |
| Home | « Il manque des piles dans le détecteur de fumée » | a card on the shopping list, one in *what's next* |
| Home | « La chaudière affiche F28 » (third time) | a sign of the target *the boiler keeps failing* |
| Work | « J'ai une idée pour le projet Atlas » | a card in Work › Atlas idea box |
| Work | the recording of a meeting | its transcript, a summary, the actions as cards |
| Hobbies | « Zelda sort le 4 novembre » | a dated note; an assistant proposes the calendar event |
| Health | « Douleur lombaire 4 sur 10 ce matin » | a sign of the target *chronic low back pain*, counted on the dashboard |
| Health | « J'ai dormi 2 h » (no watch that night) | a measure added by hand where Apple Health has none |
