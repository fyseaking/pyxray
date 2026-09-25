# Gemini CLI

```sh
python -m pyxray_agent.install gemini            # show what it would write
python -m pyxray_agent.install gemini --apply    # merge into ~/.gemini/settings.json
```

`--project DIR` uses `DIR/.gemini/settings.json`; `--uninstall` removes
pyxray's two hooks and leaves every other setting alone (a backup is kept).

Two hooks on `run_shell_command`:

| event | what it does |
|---|---|
| `BeforeTool` | records every snippet in the command for `pyx watch`; with `PYXRAY_GATE` set, refuses anything over it |
| `AfterTool` | adds a short note to the result for the model when what just ran was `check it` or worse (`PYXRAY_WARN` moves that floor, `never` turns it off) |

Gemini's hooks can allow or deny but not ask, so an over-gate call is denied
with the reason, spelled in JSON. pyxray never exits 2 (Gemini's other way of
saying deny): whatever goes wrong inside it, the hook exits 0 and the
command runs.

Gemini fingerprints hooks and warns when one changes; after reinstalling from
a different checkout or interpreter, expect that warning once. `/hooks` in a
session lists them and can switch pyxray's off.
