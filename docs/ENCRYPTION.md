# Passphrase and keys: locking spaces and folders

Status: plan. Today **the memory is not encrypted at rest** (see SECURITY.md). This document says how a person locks it, with a passphrase or a key, on every node and on every folder a node manages.

## What it protects, and what it does not

Protects: a stolen laptop or disk; a copy of the data directory or of a backup; a drive provider or a sync service that stores the folders; a backup-only node run by someone else.

Does not protect: malware on a node while it is unlocked; an assistant that was granted a space (it reads it by design); the size and the timing of what is stored. A lost passphrase **with no recovery key means the data is gone**: this is said when the lock is set, and the recovery key is shown once.

## Keys

- Each space has a random 256-bit **space key**. Nothing is encrypted directly with a passphrase.
- The space key is stored **sealed** by one or more **unlockers**, as the key slots of LUKS do:
  - a **passphrase**, stretched with Argon2id (memory-hard; the cost is set so that unlocking takes about a second on the slowest node);
  - a **recovery key**: 256 random bits written as words, shown once, to be printed or put in a password manager;
  - a **key file** (a USB stick, a file kept elsewhere);
  - the **system keystore** of the node: macOS and iOS Keychain (with Face ID or the Secure Enclave when the system offers it), Windows credential store (DPAPI), Linux secret service or a TPM: unlocks without a prompt on that machine.
- Changing a passphrase seals the same space key again: nothing is re-encrypted. Removing an unlocker removes one slot.
- A node's own identity key (see FEDERATION.md) is kept in the system keystore, never in the data directory.

## Lock states

A locked space refuses every read and write with the answer "locked", and says nothing else about its content, not even a count. Unlocking puts the space key in memory only; it is wiped when the node locks again (on a timer chosen by the person, when the app goes to the background, or on request). A node started with no keystore unlocker starts locked and asks for the passphrase on its page, its app or its command line.

**Defaults.** A new sensitive space (health, money) is locked at rest with the system keystore: nothing to type on that machine, and a copy of the disk is useless elsewhere. A passphrase is an option the person turns on, per space. Spaces that are not sensitive are not locked unless asked.

## The stores

The databases are encrypted as a whole, so that nothing about a space (text, tags, titles, the search index) is readable on the disk. The candidate is SQLCipher through the SQLite binding, which encrypts the file in AES-256 and keeps full text search; to verify before adopting: that it builds on every target of `deny.toml` (including Windows and iOS), its licence next to ours, and the cost on a large space. The control database (who may reach what) holds no content and holds only digests of keys; it is locked with the node's identity key.

## Folders managed by a node

A folder mounted in a space (SPACES.md) has a mode that says how it is kept on the disk:

| Mode | On the disk | Read by |
|---|---|---|
| **Plain** | Markdown and files as they are | any program: Obsidian, an editor, an assistant that reads the folder |
| **Locked, file by file** | each file encrypted, in the open [age](https://age-encryption.org) format (`name.md.age`), with the space key | the node (unlocked), or anyone with the key and a standard `age` tool |
| **Locked, one container** | all the Markdown and files of the mount in one encrypted database file (the node's own encrypted store, kept in the folder) | the node only, once unlocked |

- *Plain* is the right choice for a folder you work in with other programs, and it is the default for a folder that is not sensitive. Its content is as safe as the disk it is on.
- *Locked* is for a folder you put on a drive you do not trust, or that holds a sensitive space. A synchronised drive then sees only ciphertext. The price: other programs cannot read it; an assistant reads it through the node.
- A locked mount hides names too, if the person asks: files are stored under random names and the list of names is kept, encrypted, in the node. Visible names leak titles and structure; hidden names make the folder unusable by hand. The choice is the person's, per mount.
- *File by file* suits a synchronised drive: a small change re-uploads one small file, and conflicts are per file. *One container* hides the names, the number and the sizes of the files, but a synchroniser re-uploads the whole file at every change and a conflict loses a copy. Use it for a folder that rarely changes, or for a copy at rest; use *file by file* for a living folder.
- The open format is deliberate: a person is never trapped; the files can be opened without MemCP, and the tool is a standard one.

## Peers and blind nodes

- Events, files and layout travel and are stored encrypted with the space key. A node that does not hold the space key can store and forward them but cannot read them: a **blind node**. A backup at a friend's, in a cloud bucket or on a rented machine learns nothing but sizes and times. It cannot search; restoring means unlocking on a node that holds the key.
- A new node gets a space key only from the person: the key is sealed to the new node's public key after the person confirms that node (the code or QR code of FEDERATION.md). A node revoked from the swarm stops receiving; the space key is **rotated** for new events (old events stay readable with the old key, which the nodes that had it still hold: the page says so).
- The relay of a transport, if any, sees encrypted traffic between keys, and who talks to whom.

## Order

1. Encrypted store with the keystore unlocker, then the passphrase and the recovery key, then lock and auto-lock. Tests: a stolen copy of the data directory yields no readable fact, tag or title; a wrong passphrase leaks nothing in the error; locking wipes the key.
2. Locked mounts in the `age` format, names visible, then hidden names.
3. Space keys inside the swarm, blind nodes, rotation.

## A work assistant on the same computer

An assistant that must see only part of the memory (a work assistant, on a computer that also holds the personal memory) connects to the **local node** on the loopback, with a key limited to the space it needs. The node holds the whole memory, encrypted; the assistant's key reaches one space, and a locked space that is not unlocked is simply not there. This is the same local connection as in the README (`memcp mcp`, no port), and it works offline.
