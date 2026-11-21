# Over Tailscale

[Tailscale](https://tailscale.com) gives every machine of a person a private address and a name on a network of their own (a *tailnet*), with encrypted connections between them. Silice is built to sit behind it: a node reaches its peers, an assistant reaches a node, and the person reaches the page of a node, without opening a port to the internet.

What follows was checked against Tailscale's documentation of `tailscale serve` (page last validated 26 January 2026, for client 1.52 and later; read the page again for your version) and against what the code does. Where a statement rests on the code only, it says so.

## What goes through the tailnet

| Traffic | How | Exposed to |
|---|---|---|
| An assistant on one of your machines → a node | MCP over HTTPS, address `https://<machine>.<tailnet>.ts.net/mcp/<connection>` | your tailnet only |
| A node ↔ its peers (synchronisation, backup) | one encrypted connection per pair, over the tailnet (FEDERATION.md) | your tailnet only |
| You → the page of a node | the identity listener behind `tailscale serve` | your tailnet only |
| Claude on the web or the phone → a node (OAuth) | the MCP address must be public: `tailscale funnel` on that address only | the internet, on the MCP address only |

## Serving a node to the tailnet

The node listens on the loopback (its default, `127.0.0.1:8791`), and Tailscale puts HTTPS in front of it:

```sh
memcp --data ~/memory serve                       # 127.0.0.1:8791
tailscale serve --bg --https=443 http://127.0.0.1:8791
tailscale serve status                            # what is served, and where
```

- `--bg` keeps it running in the background and after a reboot. `tailscale serve reset` removes everything served; to stop one thing, repeat its command with `off` and **all the original flags**.
- The certificate is provisioned by Tailscale: the tokens travel encrypted, which they do not on a plain `http://` address. This is why a node never listens on another interface without `--allow-remote`, and why the Home Assistant add-on's open port (8791) is meant to be reached through Tailscale or a TLS proxy only.
- Serve only supports `http://127.0.0.1` targets for a proxy: the node must be on the loopback of the machine that runs `tailscale serve`.

## The node page and who you are

The node page is not on the listener that faces the network. Behind `tailscale serve`, Silice can know who is asking from the login header that Serve adds (`Tailscale-User-Login`; this is what the code reads, and Tailscale's page for Serve does not list the headers, so check their documentation for your version):

```sh
memcp serve --identity-listen 127.0.0.1:8792 --login-url https://<machine>.<tailnet>.ts.net:8792 \
  --tailnet-user you@example.org=you
tailscale serve --bg --https=8792 http://127.0.0.1:8792
```

- Put the identity listener behind `serve`, **never behind `funnel`**: Funnel opens it to the whole internet, and a header that anyone can send is no identity.
- The listener believes the login header only from the loopback or from the one address given with `--identity-trust`. On a machine with other local accounts, those accounts could claim an identity: run it on a machine whose local accounts you trust.
- `--tailnet-user LOGIN=PERSON` says which person a login is. A login that is not listed is no one: the page answers that it needs a sign-in.

## Funnel, for Claude on the web only

Claude's web and phone apps connect from Anthropic's servers, so the MCP address has to be public. Use Funnel on that address and nowhere else:

```sh
tailscale funnel --bg --https=443 http://127.0.0.1:8791
```

(`funnel` takes the same form as `serve`; I read the page of `serve` only, so check Tailscale's Funnel page for the flags of your version, and for the ports and the rules it asks for.)

The consent page stays on the identity listener, private. Each approval makes an ordinary connection named `oauth-<person>-<client>`, with the rights you ticked and nothing else, which you can revoke on the page. A public address attracts probing: keep the keys limited to one space at a time and read-only first, and keep the node page off the public address (it is, by default).

## Between nodes

- Nodes find each other by tailnet name, so a laptop that changes network is still found. A node behind a router connects out; nothing is opened.
- Restrict who may talk to whom with Tailscale's access rules: a tag for the nodes of one person (for example `tag:memcp`), and a rule that lets that tag reach only the port of the node. This is Tailscale's own feature; Silice does not need it to be safe (every connection needs a key), but it keeps strangers on a shared tailnet away from the port.
- A node that is **blind** (it holds encrypted spaces without the key, see ENCRYPTION.md) can be run on a machine you do not fully trust, or in a friend's tailnet, and still learns nothing but sizes and times.

## Using another network

Nothing in Silice needs Tailscale. A TLS proxy (Caddy, nginx), a WireGuard network of your own, or a plain local network behind a firewall work the same way: the node listens on the loopback, something in front of it provides encryption, and `--public-url` says the address the outside sees. Without encryption in front, tokens travel in clear and the node refuses to listen beyond the loopback unless told to.
