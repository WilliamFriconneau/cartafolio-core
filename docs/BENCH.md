# The bench: swarm, load, security, crashes

`scripts/bench.sh` starts real nodes from the programs of this workspace and drives them from outside, as a person and
their devices do: `memcp serve` on loopback ports, `memcp-sync` passes, the command line to make connections and people,
the page's API with a console token, MCP over HTTP, raw sockets for the probes. Nothing of the node is replaced by a
mock. It runs on macOS and Linux, in a temporary directory of its own, and prints a summary table. It exits 1 when a
check does not hold.

```sh
scripts/bench.sh                         # everything: about 4 minutes on a laptop
scripts/bench.sh --only security,crash   # some phases
scripts/bench.sh --notes 600 --secs 10   # smaller
scripts/bench.sh --keep                  # keep the directory (stores and logs of every node) to look at after
```

Options: `--only swarm,load,security,crash`, `--notes N` (3000), `--writers N` (8), `--clients N` (32), `--secs N` (60),
`--settle N` (300: how long the swarm may take to converge), `--keep`. The script builds the programs in release
(`memcp`, `memcp-sync`, `pensine-bench`), honours `CARGO_TARGET_DIR`, and writes the table to `target/bench-summary.txt`
as well.

The driver is `crates/bench` (`pensine-bench --bin DIR --work DIR ...`). It kills only the processes it started, by
their PID, and the sync loops the nodes started themselves (it puts a small wrapper in front of `memcp-sync` through
`MEMCP_SYNC_BIN` that notes their PID); before it signals a PID it checks that the process names the bench's
directory. The script removes only the directory it made with `mktemp`.

## What it checks

The oracle of each phase is what the nodes answer (the page's tree of spaces, the tools) and, for the notes, their
stores read read-only.

**1. Swarm and simultaneous writes.** Three nodes of one person: A (with the six usual spaces), B and C (empty). B joins
A and C joins B (a chain); a third of the way in, C joins A as well (a triangle). Each link has a loop of `memcp-sync`
passes, all along. Eight writers, each with its own connection, spread over the three nodes, write 3,000 notes: a quarter
in « À trier », most in the named spaces, some in sub-spaces. During the run B and C make sub-spaces (one of C's under
one of B's), and A renames and moves spaces made elsewhere. Then the bench waits for the three nodes to hold the same
memory and checks:

- every note a writer saw acknowledged is on every node, with its id and text, in its space;
- the three hold the same notes, with no text twice and no id in two spaces;
- the same spaces with the same label and parent, and A's renames and moves everywhere;
- « À trier » is one box on each node, holding every note written to any box;
- every pass ended clean; and how long the convergence took after the last write.

**2. Load.** One node with 2,000 notes; 32 clients (each with its own connection) call `remember`, `recall` and
`living_get` in turn for 60 s. Calls per second, p50 / p95 / p99 latency per tool (from the first try: a client told to
come back, 503 or 429, waits a moment and tries again, and that time counts), how many times it was told to come back,
the node's memory and open files before and after, and whether it still answers.

**3. Security.** Each probe must hold:

- no key, a wrong key, a revoked key: refused (MCP and sync routes);
- a read-only key writes nothing (`remember`, `supersede`, `retire`, `tags_set`, `file_put`, `living_set`);
- a key limited to a tag path (`work/projets/alpha`) neither reads nor writes outside it: `remember`, `recall`,
  `file_put`, `file_get`, `tags_set`, `living_set`, `triage_send`, `supersede`, `retire`, `stats`;
- a peer key of another person (a second person made on the node) can neither list, pull, push, offer nor archive the
  owner's spaces, nor take the owner's « À trier » or a sub-space of theirs as its box;
- a huge declared `Content-Length` (with and without a key) is refused and the node lives on;
- a request line sent without an end (64 MiB) is cut, and the node's memory does not grow with it;
- 64 connections that announce a body and never send it do not stop the node answering an assistant with its key;
- file names with `../`, `..\`, a line break and a header, or a NUL do not leave the node's folder nor add a header to the
  page's answer;
- repeated wrong keys turn the address away (429);
- no token the bench was given or sent appears in any node's log (checked at the end, over every node of the run).

**4. Crashes.** `kill -9` of a node three times while four writers write to it: every acknowledged note is there after the
restart and `memcp doctor` finds the store whole. `kill -9` of a pass in the middle of a first copy of 6,000 notes, then
of the receiving node: the next pass completes and receives only what was missing. `kill -9` of the serving node during a
pass: the next pass completes. A node killed leaves none of its own sync loops behind. After all of it, two passes in a
row: the second moves nothing.

## Reading the table

`ok` and `FAIL` are checks; `-` is a measure. The numbers depend on the machine; the table of a run is what to compare,
on the same machine, before and after a change.

## First run: version 20261117.0.17, and after the fixes

On a Mac laptop (10 cores) and on a Linux server (16 cores), the programs of 20261117.0.17 and those of the version
after it, the same bench.

| | 0.17, Mac | 0.17, Linux | after, Mac | after, Linux |
|---|---|---|---|---|
| Swarm: the triangle could be closed | no (the second peer replaced the first) | no | yes | yes |
| Swarm: 3,000 notes from 8 writers on 3 nodes | (chain only) | (chain only) | 3,000 kept, 0 lost, 0 twice, converged 0.6 s after the last write | the same |
| Load: calls answered per second, 32 clients | 70 | 55 | 450 | 754 |
| Load: `remember` p50 / p99 | 420 / 1,256 ms | 510 / 1,573 ms | 58 / 247 ms | 13 / 68 ms |
| Load: `recall` p50 / p99 | 586 / 1,358 ms | 631 / 1,555 ms | 76 / 282 ms | 54 / 90 ms |
| A peer of another person offering a box | took the owner's À trier and turned a sub-space of the owner into a box | the same | refused | refused |
| A request line without an end (64 MiB) | all kept: +65 MiB | all kept | cut at 8 KiB | cut at 8 KiB |
| 64 keyless bodies that never come | an assistant with its key turned away (503) for as long as they waited | the same | answered in 26 ms | answered in 11 ms |
| A node killed | its 3 sync loops kept running | the same | none left | none left |

What it found, each fixed with a test that failed before:

- Joining a second node whose address starts the same way (two nodes on one machine, `nas.home` and `nas.office`)
  replaced the first peer kept, its key with it.
- A peer of another person could offer « À trier » under the name of a space of the owner: the answer said `created`,
  and a sub-space of the owner became a box.
- The sync loops a node starts (`memcp-sync --every`) outlived the node: killed and started again, a node ran two loops
  with each peer. They now end with it (`MEMCP_SYNC_PARENT`; not on Windows).
- Two passes of one node (one per peer) that made the same space at the same moment: the second failed with « already
  exists ».
- Requests without a key that announced a body and never sent it took every place of their address, the person's
  assistants and peers included, for as long as the connections stayed open. They now have places of their own, a
  connection silent for 30 s is closed (`MEMCP_READ_TIMEOUT_SECS`), and a keyless body has a deadline.
- A body that stopped before the length it announced was taken as a request (a whole JSON could write a note).
- A request line or a header without an end was kept whole, until the memory ran out.
- With SQLite counting its memory under one lock of the process, more clients meant fewer answers (8 clients: 55
  calls a second, 1 client: 82). The workspace builds it without that count.

What the bench measures and leaves as it is: one address may have 16 requests in flight, so 32 clients on one machine
are told to come back (503, `Retry-After: 1`) often; a client of the person behind one address (all of them on the
loopback here) that sends ten wrong keys turns that address away for 15 minutes, its good keys too.
