# Page tests (optional)

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
