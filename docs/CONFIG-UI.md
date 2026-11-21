# One configuration screen, everywhere

Status: design. The same screens, the same options, in the same order and with the same words, on the Home Assistant add-on, the Windows app, the Mac, Linux and iOS.

## How it is the same

The interfaces are **native on each platform**, and they are the same because they are all built from **one description**, not because they share code.

- **One description of the screens, in the core.** The core holds the screens as data: sections, fields, their order, their types (text, list, switch, choice, path), their limits and validation, the help text, the languages, and on which platforms each option can work. It also holds the settings file those screens read and write (spaces, keywords, connectors, rules, peers), versioned. An assistant, a script or a restore can change that file; every screen shows it.
- **One native renderer per platform**, each drawing that description with the system's own controls:
  - iOS and macOS: SwiftUI;
  - Windows: a native toolkit (to choose when it is built; the criterion is that it looks and behaves like a Windows application, with the system's text size, dark mode and screen reader);
  - Linux: a native toolkit to choose the same way;
  - the Home Assistant add-on: its page in the sidebar, which is web because that is what the platform is.
- **Same options everywhere.** An option that exists on one platform exists on all, in the same place and with the same words. If a system cannot offer it (Apple Health off iOS), it is shown switched off with the reason, not hidden.
- **A test that keeps them the same.** From the description the core produces a plain list of what a screen must contain (sections, fields, order, texts). Each renderer is tested against that list, so a screen that drops a field or reorders two fails the build.
- **Words.** One set of texts per language (French, English, Spanish), taken from one file by every renderer.
- **Capture stays native too** (the microphone, the notes list, the watch, sharing a note into the app); it is not part of the description.

The price is that each platform needs its own renderer, written and kept up to date. It is accepted because a native interface is the requirement; the description and the test are what stop the renderers from drifting apart.

## The screens

1. **Spaces.** The tree of spaces; drag to move, rename in place, add, merge, retire. Each line shows the number of notes waiting, the connector, a lock if it is locked, a shield if it is sensitive.
2. **A space.** Name and place in the tree; **keywords**; sensitive or not and, if sensitive, the spaces it **may share notes with**; its **folders** (several, each with the path on this machine, a mode: source or export, and plain or locked); its connectors (Pensine node, folder, drive, another MCP memory); **assistants** and the level each has on its keywords (propose, add, full); a switch to send Apple Health data with it (iOS only, off by default); lock and passphrase.
3. **Intelligence.** The janitor that files what the keywords did not settle: off, on the device, or an API of your own (address and key); never used for a sensitive space unless the person says so for that space.
4. **Nodes.** This node and its peers: name, last synchronisation, what each holds, which spaces are synchronised with which node; add a node by its code, revoke one.
5. **Backup.** Where snapshots go, how often, the last one, a restore.
6. **Locks and keys.** Passphrase, recovery key, key file, keystore, auto-lock.
7. **Network.** How this node is reached (Tailscale, a TLS address), the keys of assistants, the identity of the page.
8. **Activity.** What changed, by whom, with the reason; revert a change of rules.

## Keywords shared on purpose

Two spaces may have the same keyword, and it is **allowed**: a note that says it feeds both. It is never an error. It is only **shown**:

- in the keyword list of a space, a keyword that another space also has carries a mark and the names of those spaces ("also in Pro, Dev");
- in the preview of a note (on the phone and on the watch), the spaces fed are listed and the ones that a shared keyword adds are marked as such, so that the person sees why;
- a unique keyword (one that decides alone) that is also in another space is shown as unique **and** shared; a keyword that a sensitive space has and another space also has is shown with the sharing rule of that sensitive space (it feeds both only if the sensitive space allows that space).

Nothing here changes the filing rules; it changes what the person can see of them.
