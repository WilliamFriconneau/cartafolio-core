# MemCP

**The external hard drive for your AIs.**

AI assistants forget. Each one keeps its own scraps, in its own cloud, and none of them knows the whole person. MemCP is a drive you host yourself: plug any assistant into it through [MCP](https://modelcontextprotocol.io) and they all read and write the same place. It stays yours: you decide who sees what, nothing is deleted behind your back, and nothing leaves your machines unless you connect it.

Keeping it simple is the point: a drive, folders, a page to look inside. Add-ons come on top. The one closest to a nurse's heart is **health through Apple Health**, and it is the only part of the app with a paid option (3 € a month); everything else is free.

It is also a manager for your notes and files: your Markdown, your photos, your voice notes, organised into one memory you can browse yourself and your assistants can read.

## Who made this, and how

I am a nurse. I am a much better nurse than developer, so this project is **built with AI** (Claude, from Anthropic, in 2026). It is not a weekend of prompts: it is hours and hours of work with the AI, thousands of lines of tests, and four complete refactors since the first day, each one because the last idea was not good enough. I decide what it should do and why, I run it for real on my own home and my own health, and I read what ships. The code has automated tests and runs in daily use. It has **not** been audited by an independent party: read [SECURITY.md](SECURITY.md) before you put anything sensitive in it.

## What it does today

- **Spaces.** What you keep is organised in spaces (health, home, work, anything you create), as folders you can nest, rename and drag around. An assistant opened on one space cannot reach another.
- **A page for you.** Open any space on a dashboard, a day-by-day journal, a tree of tags and its notes. Correct a note and the old text stays in its history; nothing is overwritten.
- **Voice notes and attachments** on that page, the same everywhere: record in the browser, attach photos and PDFs. A recording becomes text through a speech model on one of your computers, or a speech service you choose.
- **Assistants.** Each one gets an address and a key of its own, with rights on the spaces you tick, nothing more.
- **Joining without typing.** A node shows a QR code; a phone or a computer scans it, or finds the node on your network and asks to join, and you answer with one tap after comparing three words.
- **Everything you own travels with you.** One click exports it all, with fingerprints, in a form another installation can take in.

Planned, with a design written for each in [docs/](docs/): folders of Markdown and files mounted as spaces, locking a space with a passphrase or a key, nodes that synchronise with each other like peers (so that sync and backup are one thing), and one configuration screen that is the same on every system. Nothing in this list is built yet.

## Install

### Home Assistant

This repository is an app store for Home Assistant.

1. Open **Settings → Apps** (older versions: **Add-ons**), then the app store.
2. In the menu **⋮**, choose **Repositories** and add `https://github.com/WilliamFriconneau/memcp-core` ([one click with My Home Assistant](https://my.home-assistant.io/redirect/supervisor_add_addon_repository/?repository_url=https%3A%2F%2Fgithub.com%2FWilliamFriconneau%2Fmemcp-core)).
3. Install **MemCP**, start it, and open **Memory** from the sidebar.

The first Home Assistant login becomes the administrator and gets a memory of its own. Port 8791 is not encrypted: read [docs/TAILSCALE.md](docs/TAILSCALE.md) before you connect an assistant from outside. Coming from the former name (HemCP)? See [docs/MIGRATING.md](docs/MIGRATING.md).

### One command (Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/WilliamFriconneau/memcp-core/main/install.sh | sh
```

It asks where the memory should live, checks what it downloads against the published `SHA256SUMS`, can run the server as a service of your own user (nothing exposed: it listens on the loopback), and prints the key that opens the page. Read it first if you like; `--help` lists the options.

### macOS and Windows

The releases carry `memcp`, `memcp-bridge` and `memcp-janitor` for macOS and Windows. Everything builds and its tests pass on both in CI. On Windows, read [docs/SPACES.md](docs/SPACES.md) for what differs.

## Connect an assistant

On the page, press **Connect an assistant**: it gives an address and a key, and the line to paste into your assistant. For an assistant that runs a command on the same computer, `memcp mcp` talks on standard input and output, with nothing exposed on the network. [docs/REFERENCE.md](docs/REFERENCE.md) has every route (Claude Code, Claude on the web, Hermes, any MCP client).

## Join a node

Press **Join a MemCP network** on the page. A phone or a computer scans the QR code, or finds the node by itself on your network or your tailnet (`memcp-bridge discover`) and asks to join; you compare three words on both screens and allow it. Details in [docs/DISCOVERY.md](docs/DISCOVERY.md).

## Safe by default

The server listens on the loopback only. Keys are shown once and only their digest is kept. What comes back from the memory is flagged as data, never as an instruction. A first security review, with what it found and what it did not read, is in [docs/reviews/](docs/reviews/). The memory is **not encrypted at rest yet**.

## Read more

[docs/REFERENCE.md](docs/REFERENCE.md) (the full manual) · [docs/MODEL.md](docs/MODEL.md) · [docs/SPACES.md](docs/SPACES.md) · [docs/FEDERATION.md](docs/FEDERATION.md) · [docs/TAILSCALE.md](docs/TAILSCALE.md) · [docs/ENCRYPTION.md](docs/ENCRYPTION.md) · [CHANGELOG.md](CHANGELOG.md) · [CONTRIBUTING.md](CONTRIBUTING.md)

## License

**AGPL-3.0-or-later**, with a commercial licence available. See `LICENSE`, `NOTICE` and [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md). You may use, study, change and share MemCP, including at work; if you run a modified version for others over a network, the AGPL asks you to offer them its source. To build it into something you do not want to open, you need a commercial licence. Versions 0.23.1 and earlier were released under Apache-2.0 and stay available under it.
