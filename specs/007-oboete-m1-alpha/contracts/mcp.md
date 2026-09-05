# MCP Contract: `oboete mcp` (M1, legacy era)

Transport: stdio, newline-delimited JSON-RPC 2.0; logging to stderr only. M1 implements the
legacy-era lifecycle only. `server/discover` and every other unknown method return `-32601` so
clients that implement the 2026-07-28 revision fall back to `initialize`. Whether each agent's client
completes the flows below is a verification-gate probe (research R13) that compares raw frames; a
client that cannot blocks that agent's tool surface pending an owner amendment.

Repository boundary: the server derives the repository identity from its own working directory with
the same function as capture; any `repo` argument is rejected with `-32602`. Sensitivity boundary:
the shared query function with `destination = injection`.

## Wire examples

Initialize (the server echoes the client's `protocolVersion` when it is one it supports, else the
latest legacy version it implements):

```json
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.258"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"oboete","version":"<package.json version>"}}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
```

Tools list:

```json
← {"jsonrpc":"2.0","id":2,"result":{"tools":[
  {"name":"search","description":"Search memories of the current repository","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50,"default":10}},"required":["query"]}},
  {"name":"timeline","description":"Sessions and turns of the current repository","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50,"default":10}}}},
  {"name":"get","description":"One memory by id within the current repository","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}
]}}
```

Tool call (success): `content` always carries a text rendering; `structuredContent` carries the
object. Not found and out-of-boundary ids are tool results with `isError: true`, not protocol
errors; protocol errors (`-32602`) are reserved for invalid arguments.

```json
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search","arguments":{"query":"sqlite busy timeout","limit":5}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"2 memories\n- [decision] ...\n- [discovery] ..."}],"structuredContent":{"memories":[{"id":"m_...","type":"decision","title":"...","body":"...","sensitivity":"eligible","created_at":0,"citations":[],"score":0.83,"stale":false}],"degraded":null}}}
← {"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"not found"}],"isError":true}}
```

`ping` returns `{}`. Unknown tool names return `-32602`.

## Registration (verified by the R13 probe and the E2E run)

- Claude Code: `claude mcp add oboete --scope user -- "<node>" "<bundle>" mcp`, and
  `claude mcp remove oboete --scope user` to take it back. Both default to local scope, which
  reaches only the directory setup ran in, and a scope-less removal refuses outright once an
  entry exists in more than one scope. Setup removes before it adds, because `add` refuses a
  name that is already registered.
- Codex: `[mcp_servers.oboete] command = "<node>" args = ["<bundle>", "mcp"]` inside the managed
  block of `~/.codex/config.toml`
- Grok Build: its MCP server configuration as confirmed by the probe
- Pi: not registered; the Pi extension registers `search`, `timeline`, `get` tools that call the
  CLI as child processes
