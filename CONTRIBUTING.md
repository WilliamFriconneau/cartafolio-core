# Contributing

Thank you for looking. This is a small project with a clear purpose: a memory for AI around the human, hosted by that human.

## Build and test

```sh
scripts/check.sh          # format, lints, tests, dependency checks (the same steps as the CI)
scripts/check.sh quick    # format and lints only
```

These must pass. The add-on binaries are built with `ha-addon/build.sh` (needs `cross` and Docker).

## Conventions

- **English everywhere** in the repository: identifiers, comments, messages, documentation.
- **No personal data**, no real address, name or account in code, tests, fixtures or examples. Tests use fictitious data (see `crates/finances/tests/fixtures`).
- **No secret in the code.** Secrets come from options, files or the environment.
- **A rule that can be written as a test is a test**, in the crate it belongs to.
- **Rights stay explicit and simple.** A change that widens what an agent can read or do needs a clear reason in its description.
- **Memory only.** A node stores and finds what it is given. It never fetches, proxies or sends anything for an assistant, and agents never decide health facts on their own.

## Licence of your contribution

The project is released under the AGPL-3.0-or-later and is also offered under commercial terms (see [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md)). For both to stay possible, a contribution is accepted only on these terms: **by sending it you confirm that you wrote it or have the right to send it, and you grant the author a perpetual, worldwide, royalty-free licence to use, change, sublicense and relicense it, under the AGPL or under any other terms, including commercial ones.** You keep the copyright on your contribution. Say so in the pull request ("I agree to the contribution terms of CONTRIBUTING.md"). This text is a plain-language agreement drafted for the project; it has not been reviewed by a lawyer.

## AI assistance

AI helped build this project and you may use it too. If an assistant did a meaningful part of your change, say so in the pull request and add a trailer to the commit, for instance `Assisted-by: <assistant>:<model>`. You remain responsible for what you submit: read it, run it, and be able to explain it. The maintainer's own commits made with Claude Code carry `Co-Authored-By` trailers.

## Pull requests

Keep them small and focused, with the tests that prove them. Describe what changes for a user and what you did not verify.
