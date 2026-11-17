# The memory in a voice assistant (plan)

What Apple announced as *personal context* for Siri, a voice assistant at home can do with MemCP: « what do I need to buy? », « I have an idea for project X at work », « how did I sleep this week? », answered from the person's own memory, with the model of their choice. This page is the plan; nothing here is built unless it says so.

## Home Assistant Assist

Home Assistant is an MCP client since version 2025.2: its *Model Context Protocol* integration gives the tools of an MCP server to its conversation agents, the way the Assist API gives them the house ([integration page](https://www.home-assistant.io/integrations/mcp), official documentation; the page describes the SSE transport and OAuth). So Assist needs no plugin to reach a memory:

1. On the node page, make a connection for Assist (« Connect an assistant »), with the spaces it may read and write. Health stays out unless the person ticks it.
2. In Home Assistant, add the *Model Context Protocol* integration with that connection's address and key.
3. Choose a conversation agent that uses tools (a local model through Ollama, or a cloud model) and give it the MemCP tools beside the Assist API.

Then a voice pipeline (wake word, speech to text, the agent, text to speech) answers from the memory: `living_get` for the boards and lists (« what do I need to buy » is the open cards of the home space's shopping list), `recall` for a fact, `remember` with the tag `inbox` for a new note, which the router files.

**To check before calling it built:** which MCP transport the integration speaks today (the English page says SSE; MemCP serves the streamable HTTP transport: a node may need an SSE endpoint), whether the key can travel as a header from that integration, and how long the agent's prompt grows with the tool list. Each point gets a test on a real Home Assistant before the page says « works with Assist ».

## The other assistants

| Assistant | How it reaches the memory | State |
|---|---|---|
| Claude (web, apps) | OAuth connection through Tailscale Funnel ([REFERENCE.md](REFERENCE.md)) | built |
| Claude Code, OpenCode, Hermes, any MCP client | the connection's address and key, or `memcp mcp` on the same computer | built |
| Siri and Apple Intelligence | the iPhone indexes the notes of its copy as App Entities (Spotlight), and offers shortcuts (« what do I need to buy ») | built in the app; how far Siri uses the index is Apple's to say |
| Home Assistant Assist | the MCP integration above | planned |

One way in for all: the MCP. A plugin for one assistant is not written; when an assistant cannot speak MCP, the answer is an adapter outside this repository.

## On the node page: « Integrations »

A block lists each assistant connected, the way it connects (cloud or local, through which node: the peer directory says it), the spaces it reaches, and its last use; and for each kind above, the steps to connect one. Internal connections (janitor, voice, sorter) are not listed.
