# Cursor

```sh
python -m pyxray_agent.install cursor            # show what it would write
python -m pyxray_agent.install cursor --apply    # merge into ~/.cursor/hooks.json
```

`--project DIR` uses `DIR/.cursor/hooks.json`; `--uninstall` removes
pyxray's entries only (a backup is kept).

| hook | what it does |
|---|---|
| `beforeShellExecution` | records every snippet in the command; with `PYXRAY_GATE` set, anything over it comes back as `"permission": "ask"` with the reason for you and a fuller one for the agent |
| `postToolUse` (`Shell`) | adds a short note for the model when what just ran was `check it` or worse |

Cursor treats a permission hook that prints nothing — or anything that is
not valid JSON — as a refusal. pyxray therefore always answers: an explicit
`{"permission": "allow"}` when it has nothing to say, when the payload is
unreadable, when the engine is missing, and (from the launcher in a checkout)
even when no Python could be started at all.

On Windows the installer writes the absolute path of the Python it ran
under; from a checkout it points at `integrations/claude-code/hooks/pyxray-hook.cmd`,
which searches for one.
