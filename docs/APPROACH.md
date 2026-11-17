# The approach: human memory first

MemCP is a memory for AI assistants that starts from a person, not from a tool. This page says why it is built the way it is, so that you can judge it, audit it and change it. Where the design is going is in [MODEL.md](MODEL.md); what the code does today is in the [README](../README.md).

## The problem

Assistants forget. Each one keeps its own scraps, in its own cloud, readable by its own vendor, and none of them knows the whole person: the body, the home, the work, the money, the people around. A person who uses three assistants explains themselves three times, and cannot take any of it away.

The usual answer is a memory layer for software projects: a place where an agent stores what it learned about a codebase. A person's life is different in three ways. The data is more sensitive (health, money). Several assistants, with different jobs, should each see only their part. And the person, not the software, must stay in charge.

## Start from needs, not from folders

The memory is organised in **spaces**, and the default spaces follow what a person needs to keep track of: themselves, their health, the people around them, their home and daily life, their money, their work, their passions. This is inspired by the theories of human need, and it is only inspiration:

- Maslow's hierarchy (physiological needs, safety, belonging, esteem, self-actualisation) is the best known, but is described as difficult to put to an experimental test ([Need, Wikipedia](https://en.wikipedia.org/wiki/Need)).
- Doyal and Gough list twelve "intermediate needs" (food and water, housing, a safe work environment, health care, meaningful relations, economic security, education and others), which is closer to something one can organise around.
- Self-determination theory names three psychological needs: autonomy, competence and relatedness ([Wikipedia](https://en.wikipedia.org/wiki/Self-determination_theory)).

None of these is a folder structure. The list of spaces is **my synthesis, not a standard**, and it is data in a catalogue file: an installation can replace it. What the catalogue is really for is the next point.

## A space is the unit of trust

Each space is its own SQLite file. A connection (an assistant, an app) is opened on some spaces and can open no other: its token never reaches the other files. The aim is that a work assistant limited to the work space does not merely promise not to read the health space; it cannot.

This is why rights are not "per tag" or "per fact". Filtering inside one file would mean that every read path (search, neighbours, duplicates, history, files) must remember to filter, and one forgotten path leaks. Separate files make the boundary physical. The price is that a fact lives in one space; it may say it also concerns another space (a reserved tag), which makes it easier to find from there and gives no access.

## Facts, not chat logs

A memory entry is a short, dated, sourced fact that stands on its own. A fact is never edited. Correcting it writes a new one that replaces the old, and the old stays readable in the history. Retiring leaves a tombstone with a reason. Near-duplicates are not written until the assistant has decided. Nothing is deleted behind your back. The one exception is the recording of a voice memo still tagged `inbox`: once a program has written its transcription, `file_erase` erases the audio, and only that, leaving the record and the reason (see `docs/MODEL.md`).

## Instructions and data are not the same thing

Everything an assistant reads back from the memory is flagged as **data**: a sentence hidden in a document must not be obeyed. The one exception is the **guide**, the standing instructions of the person, which only the person writes. An assistant may propose a change to it; the person confirms. A short **index** of each space, written by assistants, is data too.

This split is what lets a memory be shared by assistants that read the open web or your mail without a single poisoned page rewriting your rules.

## Assistants propose, the person decides

What touches health is never decided by an assistant. A condition or an allergy it records stays unconfirmed until the person confirms it; a target or a measure it suggests is acted on by the person's own app only after they accept. Nothing is diagnosed or interpreted: values are compared with the person's own usual range, trends are computed, not judged.

## One place, in your hands

A person's memory lives on machines they own. Each node holds a copy and the nodes keep each other in step, with no central server (see [FEDERATION.md](FEDERATION.md)). There is no shared space between people. Assistants and apps reach a node on the same computer, or through a proxy that handles access, on the local network or a private network. Every node is a copy, so another node is a backup; Home Assistant's cold backup and a scheduled `backup` command add to it, and **leaving is always possible**: a person's memory exports as an archive plus a readable version of the facts.

The memory calls no AI model and no external service. Search is lexical (SQLite FTS5). That is a choice and a limit: recall is only as good as the facts the assistants write.

## A worked example: an app that uploads care data

Take a health app on a phone that records how you feel, what you measure and what you do, and that you want your assistants to be able to read.

1. **The app is an assistant like any other.** On the node page you open an MCP connection for the *health* space. The connection is limited to that space and shows up in the list with a name you can revoke.
2. **The app works without the memory.** Its raw data (an Apple Health export, say) stays on the phone. Connected, it pushes summaries as facts (one per day for the vitals, replaced when the day is updated), tagged under `health/`.
3. **Another assistant reads them.** You open a second MCP, read-only, on the health space, for the chat assistant you talk to about your health. It can recall what the app pushed, with the date and source of each fact.
4. **It proposes, you decide.** The assistant suggests a target. The app shows the suggestion; nothing is written to your health data until you accept it in the app.
5. **A work assistant never sees any of it.** Its connection is opened on the work space only. Asked about your health it can only answer that it has no access, because it has none.
6. **You can leave.** Export the person: the files and a readable version of the facts, with their history.

Steps 2 and 4 describe the direction in [MODEL.md](MODEL.md): today the app side is served by dedicated tools that the redesign removes, in favour of the generic ones.

## What this is not

It is not a medical device: it organises what you and your devices record and compares it with your own usual range, and gives no diagnosis or advice. It is not audited by anyone independent. It is developed with AI coding assistants under the direction of the maintainer, which the README says plainly, so the tests are the specification: read them before trusting a promise.

## How to audit it

- **The promise that matters**: a connection opened on space A never receives anything from space B. Rights are decided in `crates/core/src/access.rs` and applied by every tool in `crates/core/src/tools.rs`; the tests in `crates/core/tests/` and `crates/server/tests/` state each promise.
- **No `unsafe`**: it is forbidden for the whole workspace. No `unwrap` in library code, enforced by the linter.
- **Dependencies**: SQLite is compiled in; the HTTP server inside a node is plain Rust over a small HTTP library; there is no OpenSSL in the tree, so release binaries are static.
- **Secrets**: tokens are stored only as digests; a console token opens the node page and never the memory; the node page is not served on the listener that may face the internet.
- **Run it**: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`.
- Read [SECURITY.md](../SECURITY.md) before exposing it beyond your own network.
