# Living boards

The specification of the four boards every space has: what's next, ideas, targets, where things stand. The concepts and
cases are in [CONCEPTS.md](CONCEPTS.md#living-boards-whats-next-ideas-targets-where-things-stand).

## Shape

A card is a one-line note tagged `living/<board>/<column>`; a target's card also carries `sign/<name>` for each sign or
symptom it groups. A board's last pass is a note `living-meta/<board>` (`{"covers": <seconds>, "at": <time>, "by":
<connection>}`). Cards and passes are notes: they synchronise between machines and keep their history.

| Board | Columns |
|---|---|
| `next` | now, next, later, done |
| `ideas` | new, kept, done |
| `targets` | open, reached |
| `index` | current |

## Tools

- `living_get(space, board)`: the cards by column, the last pass, and the delta (notes filed since the pass, oldest first,
  with `behind` their count).
- `living_set(space, board, add, move, drop, covers)`: cards added (`{text, column, signs}`), moved (`{id, column}`),
  dropped (ids, kept in history); `covers` moves forward only; the pass is recorded.

## Rules for an assistant

1. Work from the delta only, oldest first.
2. Every card ends with the id of the note it comes from. Nothing without a source.
3. Never drop or reword a card the person wrote; move it, or add one beside it.
4. One idea per card, short, the person's words.
5. A target names the signs and symptoms it groups; a sign seen again goes to its target, not to a new one.
6. Set `covers` to the last note actually read.

## In the apps

The boards are seen and changed in the apps and on the node's page: tick a card, drag it to another column, edit a
target and its signs. Each board shows its last pass and a badge for its delta.
