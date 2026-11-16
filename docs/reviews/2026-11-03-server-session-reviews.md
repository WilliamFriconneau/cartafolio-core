# Reviews by the server session, 2 and 3 November 2026

Three read-only reviews (the server and core changes, the janitor, the bridge) were run on 2 November, next to the Mac
session's review (`memcp-bridge/docs/reviews/2026-11-02-code-review.md`). Their results were not reported to the owner at
the time; this file is that report, with what was done since.

## Fixed

| sev. | what | fixed in |
|---|---|---|
| critical | a key limited to a path wrote anywhere in the vault (one tag of its own carried the others) | 0.18.0 |
| critical | a key to `finances/inbox` was offered the money tools and read every movement | 0.18.0 |
| high | the welcome message told a path-limited key the index, the guides and the counters outside its paths | 0.18.0 |
| high | `file_erase` accepted any old fact as the transcript of a recording never transcribed | 0.18.0 |
| high | janitor: a note could be "filed" as itself and retired; a dictation cut halfway lost its end; two recordings of one name erased the wrong one | 0.18.0 |
| medium | signing in at the address of an existing connection took it over; at `console-<name>` it gave a console key | 0.18.1 |
| medium | the client took `http://010.0.0.1` as private (the system reads it as 8.0.0.1, public), followed redirects, read streams without a bound | 0.18.0 |
| medium | bridge: a captive portal failed captures for good; two processes corrupted the outbox; `forget` took a path | bridge 2409fc5 |
| medium | the inbox listing hid an old waiting note behind 200 sorted ones | 0.18.0 |
| medium | the server read up to 34 MB of any request before knowing its key, in one of a few workers, with no timeout: a few slow clients stopped everything | 0.18.3 |
| medium | behind a local proxy the client's address was the first `X-Forwarded-For` entry, which the client writes | 0.18.3 |
| medium | opening the admin page counted as a failed guess, in the counter the MCP guard uses | 0.18.3 |
| medium | anyone could fill the 200 OAuth registrations and keep new connectors out for a day; the rate table grew without bound | 0.18.3 |
| medium | the money crate could return a full account number when a statement had no summary line | 0.18.3 |
| low | different refusals told a key whether a vault name existed | 0.18.3 |
| low | no error names the memory's address any more | 0.18.2 |

## Still open

- Bridge: the HTTP listener on this machine has no authentication (the Unix socket, mode 600, is the alternative).
  (`capture_file` no longer reads the bridge's own folder, a file private to its owner, or one that holds a key.)
- Core: migrations are not run in an immediate transaction; multi-step file writes are not atomic; an export fails on a
  leftover temporary file; a few silent discards (`let _ =`) hide storage errors.
- CI: third-party actions pinned by tag, not by commit.
- The rest of the low items of the Mac session's review.
