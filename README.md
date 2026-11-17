# MemCP

**The external hard drive for your AIs.**

Apple sold *personal context* as the heart of its new Siri; in Europe we do not have it. We have intelligence anyway: MemCP builds your personal context and your memory on your own machines, as a network of nodes, and lets the model of your choice use it (Claude, a local model, any MCP client).

- In the car, a new idea: you say it, it is filed in the right space and lands in that space's idea box.
- After a meeting you no longer know what you have to do: the actions are already on the space's board, each with the note it came from.
- « What do I need to buy? »: the shopping list of your home space answers, fed by every note that mentioned a purchase, wherever it was said.
- The boiler shows the same error for the third time: it is a target of the home space, with its signs counted day after day.

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

## Principles

These hold for every part of MemCP: the server, the apps, the tools. The screens of each app are specified in [docs/UI-SPEC.md](docs/UI-SPEC.md).

- **One program, one memory, many machines.** A Home Assistant install, a Mac, a Windows or Linux PC each run the same complete node: the memory, its page, its janitor, its voice. Nothing is migrated from a "simple" version to a "server" one. A device that joins a memory is a **peer** of it: it reads and writes the person's spaces and holds a copy, kept in step with the others. Nothing only drops notes into another machine.
- **Everything runs locally first.** Each node sorts, transcribes and summarises with the tools of its own machine: Apple Intelligence on a Mac, an iPhone or an iPad, a local model (Ollama) or a speech model on a PC. A remote model is a choice of the installation, never a default.
- **The janitor keeps the keywords; the router files.** Sorting is deterministic: each space has its tags and keywords, kept in the space itself (its `rules` note) and shared by every node. A word that belongs to one space sends a note there at once; ordinary words score; a tie or nothing leaves the note in « À trier ». The janitor improves those lists, proposing keywords from what stays unsorted or is refiled by hand, then runs the sorting again. A model never decides at the moment a note arrives.
- **Lists are shared through the space.** The tags of notes and traces, the classifications and the keywords that recognise **signs** are data of the space, the same on every node. Signs are not only health: a failure in the home space (a leak, a boiler down) or a warning light in the car space is read and counted like a symptom in the health space, day by day, for the dashboards.
- **Every space keeps living boards.** *What's next*, *Ideas*, *Targets* (long-term problems and risks, each grouping the signs and symptoms that show it) and *Where things stand*: cards in columns, kept from the notes by an assistant or the local model, each board with its last pass and the notes that came since. Seen and changed in the apps. Concepts and cases in [docs/CONCEPTS.md](docs/CONCEPTS.md), the specification in [docs/LIVING-PAGES.md](docs/LIVING-PAGES.md).
- **Join before you connect.** On every app the first screen offers two things: *Start a memory* or *Join a memory*. Machines come first; connecting an assistant comes after.
- **A desktop app records meetings.** On a Mac or a PC the app can record a call or a meeting and transcribe it on the machine. Recording people needs their knowledge and their agreement: tell everyone at the start and record only if they accept. The law of your country applies.
- **A phone holds a window.** An iPhone or an iPad keeps the last thirty days and the facts marked important, syncs in the background, and shows the memory as folders in the Files app, where documents can be added. The whole memory stays on the computers and the server.
- **Old folders are a library.** Years of a drive or of daily notes are mounted read-only and indexed, never filed or rewritten; what is searched, opened or cited is raised into the memory. Originals keep their format.
- **Health is the one paid part, set up by a nurse.** The free app does everything above. *MemCP Health* adds a health space already organised by a registered nurse: Apple Health synchronised, notes tagged by themselves, targets to follow, conditions followed over time, and the dashboards a nurse would read. The help screen, the red-flag checks and the export for a health professional (a PDF laid out as nurses hand over, SBAR) stay free.

Built: nodes that synchronise as peers (a phone holds a window of thirty days), living boards in every space, a deterministic router whose keywords the janitor improves, the map of every peer and how it is reached. Planned, with a design written for each in [docs/](docs/): locking a space with a passphrase or a key ([ENCRYPTION.md](docs/ENCRYPTION.md)), and signed sync events ([FEDERATION.md](docs/FEDERATION.md)).

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

It asks where the memory should live, checks what it downloads against the published `SHA256SUMS`, runs `memcp setup` (a private data folder, your first spaces, `memcp.toml`), can run the server as a service of your own user (nothing exposed: it listens on the loopback), and prints the address of the page and one address with the key for an assistant. Read it first if you like; `--help` lists the options.

As a system service, with a Debian or Arch package and hardened systemd units, a nightly backup and the firewall and TLS recipes: [docs/LINUX.md](docs/LINUX.md).

### macOS and Windows

The releases carry `memcp`, `memcp-capture` and `memcp-janitor` for macOS and Windows. Everything builds and its tests pass on both in CI. On Windows, read [docs/SPACES.md](docs/SPACES.md) for what differs.

## Connect an assistant

On the page, press **Connect an assistant**: it gives an address and a key, and the line to paste into your assistant. For an assistant that runs a command on the same computer, `memcp mcp` talks on standard input and output, with nothing exposed on the network. [docs/REFERENCE.md](docs/REFERENCE.md) has every route (Claude Code, Claude on the web, Hermes, any MCP client).

## Join a node

Press **Join a MemCP network** on the page. A phone or a computer scans the QR code, or finds the node by itself on your network or your tailnet (`memcp-capture discover`) and asks to join; you compare three words on both screens and allow it. Details in [docs/DISCOVERY.md](docs/DISCOVERY.md).

## Safe by default

The server listens on the loopback only. Keys are shown once and only their digest is kept. What comes back from the memory is flagged as data, never as an instruction. A first security review, with what it found and what it did not read, is in [docs/reviews/](docs/reviews/). The memory is **not encrypted at rest yet**.

## Read more

[docs/REFERENCE.md](docs/REFERENCE.md) (the full manual) · [docs/LINUX.md](docs/LINUX.md) · [docs/MODEL.md](docs/MODEL.md) · [docs/SPACES.md](docs/SPACES.md) · [docs/FEDERATION.md](docs/FEDERATION.md) · [docs/TAILSCALE.md](docs/TAILSCALE.md) · [docs/ENCRYPTION.md](docs/ENCRYPTION.md) · [CHANGELOG.md](CHANGELOG.md) · [CONTRIBUTING.md](CONTRIBUTING.md)

## License

**AGPL-3.0-or-later**, with a commercial licence available. See `LICENSE`, `NOTICE` and [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md). You may use, study, change and share MemCP, including at work; if you run a modified version for others over a network, the AGPL asks you to offer them its source. To build it into something you do not want to open, you need a commercial licence. Versions 0.23.1 and earlier were released under Apache-2.0 and stay available under it.
