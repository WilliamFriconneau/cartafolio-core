# Page tests (optional)

`wizard.test.js` loads the admin page in a fake browser (jsdom) against a fake server and
walks through the "Connect an assistant" steps. It is not part of `cargo test`; it needs
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
