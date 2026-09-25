# Integrations

Every integration here ends up calling one launcher:

```sh
integrations/claude-code/hooks/pyxray-hook
```

It reads a `PreToolUse`-shaped JSON payload on stdin, finds the Python inside
it — a tool that takes code directly, or a shell command carrying a script
path, a heredoc, a `-c` one-liner, a `bash -c`, a pipe — appends a row to the
feed for each, and writes a block decision on stdout **only** if
`PYXRAY_GATE` is set. Ungated it prints nothing. It finds pyxray on its own
(`PYXRAY_ROOT`, the checkout it sits in, or `pip install pyxray-agent`), tries
`python3`, `python` and `py -3` until one can import it, and always exits 0 —
a hook that errors blocks the tool call, and an observer that breaks somebody
else's work is worse than no observer. `PYXRAY_DEBUG=1` reports what it
swallowed. (`integrations/pyxray-hook` still exists and forwards to it.)

## Which surface do I want?

| you are | use | why |
|---|---|---|
| typing commands yourself | `shell/pyxray.sh` | a shell function is enough |
| running an agent | that harness's hook, below | the harness knows what it is about to run |
| running an agent with no hook API | `integrations/shim` | a PATH shim catches anything that spawns `python3` or `python` |
| running something exotic | `integrations/shim` | same reason |
| on Windows | that harness's hook | `CreateProcess` does not resolve `.cmd` shims; the hook sees everything |

**A shell function will not catch an agent.** Harnesses spawn non-interactive
shells, which never read your rc file, and many use `/bin/sh` rather than bash.
A PATH shim is a real executable resolved by `execvp`, so it catches the
agent's `sh -c`, a Makefile, and a `subprocess.run` deep inside some library.

## Harnesses

| | how | notes |
|---|---|---|
| [Claude Code / Claude desktop](claude-code/) | plugin (`/plugin marketplace add adukhan99/pyxray`), or a `PreToolUse` hook in `settings.json` | `Bash` commands and `NotebookEdit` code cells; scripts are read relative to the call's `cwd`; a `PostToolUse` note tells the model what a risky call just did |
| [Hermes CLI / Hermes Desktop](hermes/) | native plugin (`hermes plugins install adukhan99/pyxray/integrations/hermes`), with a desktop pane | `terminal` and `execute_code`; verdict appended to the tool result, over-gate calls go to the approval prompt |
| [Codex CLI](codex/) | `python -m pyxray_agent.install codex --apply` (`~/.codex/hooks.json`) | `PreToolUse` on `Bash`/`exec_command`; over the gate is refused (Codex can't ask) |
| [Gemini CLI](gemini/) | `python -m pyxray_agent.install gemini --apply` (`~/.gemini/settings.json`) | `BeforeTool`/`AfterTool` on `run_shell_command`; the model gets a note after risky calls |
| [Cursor](cursor/) | `python -m pyxray_agent.install cursor --apply` (`~/.cursor/hooks.json`) | `beforeShellExecution` asks over the gate; always answers, since Cursor reads silence as "deny" |
| [OpenCode](opencode/) | plugin on `tool.execute.before` | blocks by throwing |
| any MCP client | [`pyxray-mcp`](mcp/) | tools an agent calls *before* running: `analyze`, `extract`, `check`, `explain`, `feed` |
| anything else | [PATH shim](shim/) | no API needed |

## A note on models and harnesses

There is nothing to integrate with a *model*. DeepSeek, Claude, GPT, Qwen and
the rest are things a harness talks to; they have no plugin surface and never
see your filesystem. What runs the Python is the harness around the model, and
that is what pyxray hooks. An agent pointed at DeepSeek is covered by whichever
harness it is — OpenCode, Hermes, something homegrown — or, failing all of
those, by the PATH shim, which does not care what is upstream.

This also means pyxray gets more useful, not less, as models get better.
Frontier models are not going to `rm -rf` your home directory. What they do
constantly is run twenty snippets a minute that each touch a few files, and the
question worth answering is not "is this one malicious" but "what has all of
this been doing". That is why the default is a feed and not a blocker.

## Watching

```sh
pyx watch                       # live column
pyx watch --floor check         # only what wanted a second look
pyx watch --dump                # render once and exit; for scripts, status bars
integrations/tmux/pyxray-pane.sh    # open it in a tmux split
```

## Configuration

All of it is environment variables, so it works the same through every
integration.

| | |
|---|---|
| `PYXRAY_OFF=1` | do nothing at all |
| `PYXRAY_LOG` | feed log path (default: `$XDG_RUNTIME_DIR/pyxray/feed.jsonl`; `%LOCALAPPDATA%\pyxray\feed.jsonl` on Windows; else `~/.cache/pyxray/feed.jsonl`) |
| `PYXRAY_DRAW` | `never` \| `tty` \| `always` (default: `tty`) |
| `PYXRAY_TTY` | draw to this device instead, e.g. `/dev/pts/7` |
| `PYXRAY_LAYOUT` | `line` \| `auto` \| `card` \| `dashboard` \| `stack` \| `flow` (default: `auto`) |
| `PYXRAY_THEME` | `blueprint` \| `neon` \| `paper` \| `carbon` \| `amber` \| `ansi` \| `mono` (default: `blueprint`) |
| `PYXRAY_ICONS` | `glyph` \| `tag` \| `both` (default: `both`) |
| `PYXRAY_WIDTH` | columns |
| `PYXRAY_GATE` | `off` (default), or a risk score to refuse above |
| `PYXRAY_CONFIRM=1` | ask before running (shim only; a hook has no terminal) |
| `PYXRAY_MIN_LINES` | ignore snippets shorter than this |
| `PYXRAY_SOURCE` | label hook events with this origin instead of `hook` |
| `PYXRAY_DEBUG=1` | report on stderr the failures the hook and shim otherwise swallow |
| `PYXRAY_BIN` | the `pyx` binary to use when the extension is not installed |
| `PYXRAY_PYTHON` | the interpreter the hook launcher and the Windows shims should use |
| `PYXRAY_ROOT` | a checkout to import the package from, for a plugin installed elsewhere |

`PYXRAY_GATE=0` is **not** "no gate" — it refuses anything with any effect at
all. `off` is spelled out for exactly that reason.

## Why those defaults

`auto` and a tmpfs feed, together, are the whole posture: **watch, don't
gate.** A dull snippet gets one row and an alarming one blooms into the card,
so the pane stays scannable through a burst without you tuning anything; and
the log lives in `$XDG_RUNTIME_DIR`, which is a tmpfs on any systemd machine,
so a chatty session costs no disk and the record dies with the login — the
right lifetime for something you read live and never audit later. Elsewhere
it falls back to a directory that is still per-user (never a shared `/tmp`
path). Point `PYXRAY_LOG` at a real path if you do want to keep it.

The point is not to stop an agent doing something terrible. It is to let you
see what a few hundred lines of tool calls actually touched, without reading a
few hundred lines.
