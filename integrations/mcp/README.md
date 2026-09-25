# pyxray as an MCP server

The hooks watch everything an agent runs, whether it asks or not. The MCP
server is the other half: tools an agent (or you, from a chat) can call
*before* deciding to run something. It is static like everything else in
pyxray — nothing is executed — and nothing it does writes the feed.

| tool | what it answers |
|---|---|
| `analyze` | what a snippet or a `.py` file will do: effects, secret flows, risk, synopsis; with a path, its local imports too |
| `extract` | every piece of Python a shell command would run, each analysed |
| `check` | the verdict a hook would give — allow, ask or deny — under `PYXRAY_GATE` and the `.pyxray.toml` in effect |
| `explain` | the terms a risk score is the sum of |
| `feed` | the latest rows of the pyxray feed, from every harness |

`pip install pyxray-agent` puts `pyxray-mcp` on your PATH. It speaks MCP over
stdio; register it wherever your client keeps its servers:

**Claude Code**

```sh
claude mcp add pyxray -- pyxray-mcp
```

**Claude desktop** — `claude_desktop_config.json`:

```json
{ "mcpServers": { "pyxray": { "command": "pyxray-mcp" } } }
```

**Codex CLI** — `~/.codex/config.toml`:

```toml
[mcp_servers.pyxray]
command = "pyxray-mcp"
```

**Gemini CLI** — `~/.gemini/settings.json`:

```json
{ "mcpServers": { "pyxray": { "command": "pyxray-mcp" } } }
```

**Cursor** — `~/.cursor/mcp.json`:

```json
{ "mcpServers": { "pyxray": { "command": "pyxray-mcp" } } }
```

If `pyxray-mcp` is not on the PATH your client uses — common on Windows, and
under GUI apps on macOS — give the interpreter instead: `"command":
"C:\\Python312\\python.exe", "args": ["-m", "pyxray_agent.mcp"]`.

From a checkout without installing: `PYTHONPATH=python python3 -m
pyxray_agent.mcp`, with a `pyx` built by `cargo build --release`.
