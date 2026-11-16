# Contributing

Thank you for looking. This is a small project with a clear purpose: a memory for AI around the human, hosted by that human.

## Build and test

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo deny check        # advisories, licences, bans, sources (deny.toml)
```

These must pass. The add-on binaries are built with `ha-addon/build.sh` (needs `cross` and Docker).

## Conventions

- **English everywhere** in the repository: identifiers, comments, messages, documentation.
- **No personal data**, no real address, name or account in code, tests, fixtures or examples. Tests use fictitious data (see `crates/finances/tests/fixtures`).
- **No secret in the code.** Secrets come from options, files or the environment.
- **A rule that can be written as a test is a test**, in the crate it belongs to.
- **Rights stay explicit and simple.** A change that widens what an agent can read or do needs a clear reason in its description.
- **Memory only.** The server stores and finds what it is given. It never fetches, proxies or sends anything for an assistant, and agents never decide health facts on their own.

## AI assistance

AI helped build this project and you may use it too. If an assistant did a meaningful part of your change, say so in the pull request and add a trailer to the commit, for instance `Assisted-by: <assistant>:<model>`. You remain responsible for what you submit: read it, run it, and be able to explain it. The maintainer's own commits made with Claude Code carry `Co-Authored-By` trailers.

## Pull requests

Keep them small and focused, with the tests that prove them. Describe what changes for a user and what you did not verify.
