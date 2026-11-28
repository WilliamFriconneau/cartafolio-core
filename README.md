# Pensine

Pensine is a Rust memory service for assistants, designed to run as a Home
Assistant add-on. It will store notes with history and files in spaces,
support full-text search, and expose the tools over MCP with per-space access.

This workspace contains the initial crate skeletons. The `pensine` binary
currently exits immediately.

## Build

Use stable Rust with Cargo, rustfmt, and Clippy.

```sh
cargo build --workspace
```

## Run

```sh
cargo run -p pensine
```

## Check

```sh
scripts/check.sh
```

Pensine is licensed under AGPL-3.0-or-later. See [LICENSE](LICENSE).
