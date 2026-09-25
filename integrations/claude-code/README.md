# Claude Code and the Claude desktop app

Claude runs Python through the `Bash` tool — as a heredoc, a `-c` one-liner,
or `python3 script.py` — and edits notebooks through `NotebookEdit`. The hook
matches both, pulls the Python back out (reading a script relative to the
tool call's `cwd`), and appends one row to the feed.

This plugin is the same for the terminal (Claude Code) and the desktop app:
both load plugins from the same manifest and run the same `PreToolUse` hook.

## As a plugin (recommended)

```
/plugin marketplace add adukhan99/pyxray
/plugin install pyxray@pyxray
```

or, from a checkout, `/plugin install ./integrations/claude-code`.

The plugin's `hooks/hooks.json` runs `hooks/pyxray-hook` through
`${CLAUDE_PLUGIN_ROOT}`, so it works wherever the plugin is copied to. The
hook needs the `pyxray_agent` Python package to be importable by *some*
Python on your machine, found in this order:

1. `PYXRAY_ROOT` — a checkout; its `python/` directory is used directly.
2. The checkout this plugin directory sits inside (`/plugin install ./…`).
3. Wherever `pip install pyxray-agent` put it.

and it tries `PYXRAY_PYTHON`, then `python3`, `python`, and the Windows
`py -3` launcher, until one of them can import it. Set either variable in the
`env` block of `settings.json` if the defaults pick the wrong interpreter:

```json
{ "env": { "PYXRAY_PYTHON": "/opt/homebrew/bin/python3.12" } }
```

## Without the plugin

`integrations/claude-code/install.sh` (or `install.ps1` on Windows) writes
the equivalent into `.claude/settings.json` (project) or, with `--global` /
`-Global`, `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash",
        "hooks": [{ "type": "command",
                    "command": "sh \"/path/to/pyxray/integrations/claude-code/hooks/pyxray-hook\"",
                    "timeout": 5 }] },
      { "matcher": "NotebookEdit",
        "hooks": [{ "type": "command",
                    "command": "sh \"/path/to/pyxray/integrations/claude-code/hooks/pyxray-hook\"",
                    "timeout": 5 }] }
    ]
  }
}
```

## Then

```sh
pyx watch
```

in a second terminal, and every snippet Claude runs lands there as one row.

After a `Bash` call, a `PostToolUse` hook adds a short note to the result
when what just ran was `check it` or worse — the same note Hermes, Gemini CLI
and Cursor get — so the model can say so if it was not what you asked for.
`PYXRAY_WARN=read` raises that floor; `never` turns it off.

The pre-tool hook prints nothing unless you set `PYXRAY_GATE`. When you do, anything
above it comes back as `permissionDecision: "ask"` with the reason attached,
so Claude prompts you rather than silently refusing — the model still sees
why, and you still decide.

## Windows

Claude Code on Windows runs hook commands through the Git Bash it also uses
for the Bash tool, so the same `sh` launcher applies; `install.ps1` writes the
path with forward slashes and sets `PYXRAY_PYTHON`. The feed lives under
`%LOCALAPPDATA%\pyxray\`. Use Windows Terminal (or `chcp 65001`) for the
Unicode glyphs, or `-t mono` for pure ASCII.

## Notes

- The hook receives `{"tool_name": "Bash", "cwd": …, "tool_input": {"command": …}}`
  on stdin. It ignores anything with no Python in it and exits 0.
- It never fails the tool call: a missing engine, a broken Python, a bad
  payload all exit 0 silently. Set `PYXRAY_DEBUG=1` to see why something was
  skipped, or run it by hand:
  `echo '{"tool_name":"Bash","tool_input":{"command":"python3 -c \"import os\""}}' | sh integrations/claude-code/hooks/pyxray-hook`
- `PYXRAY_DRAW=never` is the effective default under a hook (there is no
  terminal to draw to), so the render never reaches the transcript. Use
  `pyx watch`, or point `PYXRAY_TTY` at a spare terminal.
