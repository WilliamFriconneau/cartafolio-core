# Finding a node, and joining it without typing

## Finding

A node that listens beyond the loopback **advertises itself** on the local network (mDNS, the service `_memcp._tcp`) with its name and its version, and answers `GET /.well-known/memcp` with the same, so that what a device finds can be checked. The node and the manager turn it on with `--advertise --node-name NAME`.

A Home Assistant app runs in a container, which cannot advertise itself on the network: a device finds it by the names a Home Assistant installation answers to (`homeassistant.local`, `memcp.local`), which `discover` tries, and by the tailnet. Advertising by the app itself would need the host network, and is not done.

A device finds nodes with `memcp intake discover` (the apps use the system's own browser for Bonjour). Without any option it also looks at the machines of your tailnet, when the Tailscale command is there, and asks each whether it is a node.

Advertising says that a node is here, and nothing of what it holds. It gives no access.

## Joining

1. The device asks the node (the page's **Join a MemCP network**, `memcp-sync --join http://HOST:8791 --name "My laptop"`, or an app from its welcome screen).
2. The node shows the request on its page, and in the notifications of its apps, with **three words**. The device shows the same three words.
3. The person compares them and presses **Allow**. Nothing is typed.
4. The device receives a connection of its own, once, and keeps it where only it can read (a file of the node's `peers/` folder, or the Keychain). It reaches the person's Inbox and nothing else until the person says otherwise.

A request lives in memory for two minutes. A node refuses more than a few requests a minute from one address and more than a few waiting at once; a wrong secret learns nothing. Another way in is the QR code on the page of a node, which carries the address and a one-time code.

## What discovery is not

It is not a way around the person: a node never lets a device in by itself. On a network you do not trust, leave `--advertise` off and use the QR code only.
