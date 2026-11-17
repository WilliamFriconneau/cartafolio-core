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
(`memcp`, `memcp-sync`, `memcp-bench`), honours `CARGO_TARGET_DIR`, and writes the table to `target/bench-summary.txt`
as well.

The driver is `crates/bench` (`memcp-bench --bin DIR --work DIR ...`). It kills only the processes it started, by
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
on the same machine, before and after a change. The results of the bench run that came with this file are in the
changelog of its version.
