# Codex CLI

```sh
python -m pyxray_agent.install codex            # show what it would write
python -m pyxray_agent.install codex --apply    # write ~/.codex/hooks.json
```

`--project DIR` writes `DIR/.codex/hooks.json` instead (Codex loads that only
when the project's `.codex/` is trusted). `--uninstall` removes pyxray's
entry and nothing else; a backup is kept either way.

It adds one `PreToolUse` hook on `Bash`, `exec_command` and `shell`:

```json
{ "hooks": { "PreToolUse": [ {
  "matcher": "^(Bash|exec_command|shell)$",
  "hooks": [ { "type": "command",
               "command": "/path/to/python -m pyxray_agent.hook codex pre",
               "timeout": 10 } ] } ] } }
```

Codex sends a shell command as an argv array (`["bash", "-lc", "python3 …"]`);
pyxray quotes it back into one command before reading it, so the Python
inside the `-lc` string is found intact.

**Gating.** Codex's hooks can allow or deny, but not ask. With `PYXRAY_GATE`
set, a call over it is refused with the reason, which Codex shows the model;
raise the gate, or run the command yourself. Unset, nothing is ever refused:
every call is recorded for `pyx watch` and that is all.

Codex's `PostToolUse` is audit-only, so the after-the-call note that Claude
Code, Gemini CLI, Cursor and Hermes get is not installed here.
