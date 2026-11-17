# Page tests

CI runs every `*.test.js` here (the `page` job of `.github/workflows/ci.yml`), with the program built in
`target/debug` (`MEMCP_BIN` says otherwise) and the Chrome of the runner (`CHROMIUM`). They are not part of `cargo test`.
The page has four sections (`#note`, `#journal`, the empty hash for Spaces, `#settings`); a test that reaches a part of
Settings goes to `#settings` first.

```sh
cargo build -p memcp-server
cd crates/server/tests/page && npm install jsdom playwright-core --no-save
for t in *.test.js; do CHROMIUM=/usr/bin/chromium node "$t" || echo "FAILED $t"; done
```

## The golden paths, against a real node

`e2e.test.js` inits a node in a temporary folder (two spaces, a console token), serves it on a free port and loads its
page in jsdom: the page talks to the real API, only the browser is simulated. In French, it signs in and checks the four
sections and their labels; Spaces shows the tiles and a space opens on its Boards tab; Trace or drop writes a note that
appears in its list; the Journal lists the notes of every space, each with its space; Settings shows the machines and
« Connect an assistant », hides the tiles and shows no empty section. On every view: no « null », « undefined », and none
of the words the interface never uses (bridge, capture app, vault), and nothing logged as an error. jsdom only:

```sh
cd crates/server/tests/page && npm install jsdom --no-save && node e2e.test.js
```

## The logic of the page, against a fake server

`wizard.test.js` loads the admin page in a fake browser (jsdom) against a fake server and
walks through the "Connect an assistant" steps, the triage view and the instructions editors. It is not part of `cargo test`; it needs
Node.js:

```sh
cd crates/server/tests/page && npm install jsdom --no-save && node wizard.test.js
```

It is a check of the page's logic, not of a real browser: the clipboard and the real
`<dialog>` are not exercised.

## In a real browser

`browser.test.js` drives the real page, served by the real program, in a real headless Chromium
(through `playwright-core`): signing in with a console key, the vault list, the "connect an
assistant" steps and the wait for the first request, copying to the clipboard, the instructions
editor, the download of the export, the activity list, and that nothing breaks the page's content
security policy or logs an error.

```sh
cargo build -p memcp-server
cd crates/server/tests/page && npm install jsdom playwright-core --no-save
CHROMIUM=/usr/bin/chromium node browser.test.js     # SCREENSHOTS=some/dir keeps pictures of the page
```

It needs a Chromium on the machine (`CHROMIUM` says where) and the program built in `target/debug`
(`MEMCP_BIN` says otherwise). Not part of `cargo test`.

## The start screen, machines, keywords and targets, languages and themes

`onboarding.test.js` drives the real program in a real Chromium: a node with no memory offers exactly two choices and a
guided path of four steps with its progress (start: name, an assistant with the single address that carries the key;
join: nodes found on the network or an address, then three words), a node with spaces shows its machines first (live
peers), folders, labels, the keywords and targets of a space, French and English, light and dark, and that nothing
scrolls sideways at 400 px. It stands in for `memcp-capture discover` and `memcp-sync --join` with small scripts, so it
needs no network. `SCREENSHOTS=dir` keeps pictures at 1280 and 400 px.

```sh
cargo build -p memcp-server
cd crates/server/tests/page && npm install jsdom playwright-core --no-save
CHROMIUM=/usr/bin/chromium node onboarding.test.js
```

## French completeness

`french.test.js` opens the page in French against the real program and walks its main views (home with every fold open,
triage, administration, a space and each of its tabs), every wizard platform before and after the connection is made, the
instruction editors, the add-a-node, confirmation and secret windows, an error shown by the page, and the start screen with
its join step. It fails when an element still holds one of the English sentences of the short `ENGLISH` list at the top of the
file, when a visible text still reads as English (stop words) after the page's own translator has run, or when the translator
knows a text that was left in English. A new English string in `admin.html` goes into `FR` (exact text) or `FRX` (text with a
number or a name in it); if it is a sentence worth guarding, add its first words to `ENGLISH`. The long assistant prompt is
written once in each language (`ORGANISE_PROMPT` and `ORGANISE_PROMPT_FR`), not translated line by line.

```sh
cargo build -p memcp-server
cd crates/server/tests/page && npm install jsdom playwright-core --no-save
CHROMIUM=/usr/bin/chromium node french.test.js
```

## Create, read, update, delete

`crud.test.js` drives the real program in a real Chromium through the buttons added for docs/CRUD.md: create a space with
keywords, delete it with a choice (preview first, then hold to confirm), the archive list and restore, delete into another
space, a new key, revoke and remove a connection, the janitor kept in *Internal tools*, pause, revoke and forget a machine,
bring a removed note back, remove a vocabulary, and that no browser dialog is ever used.

```sh
cargo build -p memcp-server
cd crates/server/tests/page && npm install jsdom playwright-core --no-save
CHROMIUM=/usr/bin/chromium node crud.test.js
```
