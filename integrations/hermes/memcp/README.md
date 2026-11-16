# MemCP memory for Hermes Agent

A [Hermes Agent](https://github.com/NousResearch/hermes-agent) memory provider that makes a
MemCP connection the agent's memory.

- **System prompt**: the instructions MemCP gives the connection (the person's guide, the
  guides and indexes of the vaults it reaches).
- **Before each turn**: a `recall` on the user's words; the facts found are given to the model
  as context, marked as data.
- **Tools**: the connection's own tools, prefixed `memcp_` (`memcp_recall`, `memcp_remember`,
  `memcp_supersede`, ...). What a connection may read or write is decided by MemCP, not here.
- **Nothing written behind the agent's back**: no transcript is sent; a fact is written only
  when the model calls a tool.

Standard library only. MemCP answers plain JSON-RPC over HTTP.

## Install

One copy per profile (a Hermes profile is a `HERMES_HOME`):

```sh
cp -r integrations/hermes/memcp "$HERMES_HOME/plugins/memcp"
```

In the profile's `.env`:

```sh
MEMCP_URL=https://memory.example.net/mcp/<connection>
MEMCP_TOKEN=<the connection's key>
```

Then make it the profile's memory and turn the built-in one off:

```sh
hermes config set memory.provider memcp
hermes config set memory.memory_enabled false
hermes config set memory.user_profile_enabled false
```

Create the connection with `memcp connect <name> --grant <vault>:rw,...`: one connection per
agent, with only the vaults it needs. If the same connection is also listed under
`mcp_servers`, remove it there: the tools would appear twice.
