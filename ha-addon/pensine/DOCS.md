# Pensine

Pensine stores notes and files for assistants over MCP. Each Home Assistant user
has their own account and grants access to spaces through connections.

## First start

1. Start the add-on and open **Pensine** from the Home Assistant sidebar.
2. Open the add-on log. If it shows a Tailscale approval URL, open it and approve
   the node. A Tailscale auth key can replace this step.
3. Enable Funnel for the node in your tailnet. If the log says Funnel could not
   start, follow the setup link it shows, then restart the add-on.
4. Once the public address appears in the log, set your account's MCP alias on
   the Pensine page. Use `https://<node>.<tailnet>.ts.net/mcp/<alias>` as the
   connector address.

Funnel uses HTTPS on port 443 for MCP and OAuth. Open the Pensine admin page
through Home Assistant ingress.

## Connecting an assistant

For an OAuth connector, generate a one-time code on the Pensine page. Enter it
when the connector asks you to sign in, then choose the spaces and permissions
to grant.

For a client that accepts bearer keys, create a connection on the Pensine page.
Copy the key when it appears and set it as the client's bearer token.

## Options

| Option | Meaning |
|---|---|
| `tailscale` | Starts an independent Tailscale node and Funnel. On by default. |
| `tailscale_hostname` | The node name in your tailnet. Defaults to `pensine`. |
| `tailscale_authkey` | Optional key for joining the tailnet without manual approval. |
| `allow_plain_http` | Allows a non-loopback HTTP listener when Tailscale is off. Off by default. |
| `public_url` | HTTPS origin of your own proxy when Tailscale is off. |

With Tailscale off, set `allow_plain_http: true`, enable port 8791 in the
add-on's Network tab, and put a TLS proxy in front of it. The option defaults
to false and keeps Pensine on loopback. Tokens travel in clear on port 8791.

## Backups

Home Assistant stops the add-on for a cold backup. The backup includes the
accounts, notes, files, and Tailscale node state under `/data`.
