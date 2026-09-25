# Hermes CLI and Hermes Desktop

This directory is a native Hermes plugin: `plugin.yaml` + `__init__.py` for
the agent (CLI, gateway, Desktop's backend), `dashboard/` for its backend
routes and `desktop/plugin.js` for a pane in Hermes Desktop. One folder,
installed once, both surfaces.

Hermes runs Python through two tools and the plugin covers both:

| tool | argument | what arrives |
|---|---|---|
| `execute_code` | `code` | Python, directly |
| `terminal` | `command` | a shell command — `python3 script.py`, a heredoc, `-c`, `uv run`, `cat x.py \| python3`, `bash -c "…"`, `cd dir && python3 …` |

## Install

```sh
hermes plugins install adukhan99/pyxray/integrations/hermes
hermes plugins enable pyxray
```

or, from a checkout, copy (or symlink) `integrations/hermes` to
`~/.hermes/plugins/pyxray` and add `pyxray` to `plugins.enabled` in
`~/.hermes/config.yaml`. On Windows that is `%USERPROFILE%\.hermes\plugins\pyxray`.

The plugin needs the `pyxray_agent` Python package to be importable by
Hermes' Python, found in this order:

1. `import pyxray_agent` (`pip install pyxray-agent` into Hermes' venv, or
   `uv pip install pyxray-agent` from `~/.hermes`);
2. `PYXRAY_ROOT` — a checkout, whose `python/` directory is used directly
   (with the `pyx` binary from `cargo build --release` next to it, or on `PATH`);
3. the checkout this plugin directory sits inside.

`/pyxray` (or `hermes pyxray status`) tells you which one it found, or why it
found none. A missing engine is not an error: the plugin stays loaded and
describes nothing until it has one.

### Desktop

Hermes Desktop copies `desktop/plugin.js` out of the installed package by
itself and lists it under **Capabilities → Plugins**; toggle it on there (both
halves are opt-in by design). The pane docks under the conversation and
polls its own backend, so it needs the agent half enabled too — with it off,
the pane shows an empty list and says so.

Or, without the CLI: `hermes://plugin/install?repo=adukhan99%2Fpyxray&subdir=integrations/hermes&enable=1`.

## What it does

**Before the tool runs** (`pre_tool_call`): every piece of Python in the call
is analysed and appended to the pyxray feed — the same feed `pyx watch`, the
Claude Code plugin and the shim write to — and a card is rendered for the
desktop pane. With a gate set, anything over it goes to Hermes' own
human-approval prompt with the reason attached (`approve` directive; answer
`[a]lways` to let that band through from then on). `mode: block` refuses
instead of asking.

**After the tool returns** (`transform_tool_result`): a short verdict rides
back to the model under the result —

```
---
pyxray — check it · risk 30 · runs `rm -rf build`
  · shell rm -rf build — runs a command through the shell; recursive delete (line 3)
```

— from the `check` band up by default, so the model sees what it just did
and can say so, the way the bundled `security-guidance` plugin does for file
writes. It never blocks: the tool already ran.

Nothing is drawn into Hermes' terminal; inside an agent there is nowhere to
draw. `pyx watch` in a spare pane, the desktop pane, or `/pyxray show`.

## Commands

| | |
|---|---|
| `/pyxray` | engine, feed path, gate, the last few calls |
| `/pyxray last [n]` | the last *n* calls this session, one line each |
| `/pyxray show [n]` | the full card for the *n*-th most recent call |
| `/pyxray gate <0-100\|off>` | set the gate in `config.yaml` |
| `/pyxray feed [n]` | the last *n* rows of the whole feed, whichever harness wrote them |
| `hermes pyxray status\|feed\|cards` | the same from a shell |

## Settings

Under `plugins.entries.pyxray.settings` in `~/.hermes/config.yaml`; every key
is optional.

```yaml
plugins:
  enabled: [pyxray]
  entries:
    pyxray:
      settings:
        gate: 40          # 0-100, or off (default). PYXRAY_GATE in the environment wins.
        mode: approve     # approve (ask the human) or block
        warn_from: check  # routine | check | read | never — when to append the verdict
        tools: [terminal, execute_code]
        cards: true       # render a card per call for the desktop pane
        max_cards: 40
```

`PYXRAY_OFF=1` switches the plugin off for a process, `PYXRAY_LOG` moves the
feed, `HERMES_SAFE_MODE=1` disables all plugins together.

## Files

```
integrations/hermes/
├── plugin.yaml          manifest (hooks, config schema)
├── __init__.py          register(ctx): hooks, /pyxray, hermes pyxray
├── guard.py             the logic, no Hermes import — tested in python/tests/test_hermes_plugin.py
├── dashboard/
│   ├── manifest.json    { "name": "pyxray", "api": "plugin_api.py" }
│   └── plugin_api.py    /api/plugins/pyxray/{status,events,recent,card/<name>,gate}
├── desktop/
│   └── plugin.js        the pane (plain ESM, @hermes/plugin-sdk)
├── hooks.yaml           the older shell-hook route, see below
└── install.sh
```

Cards live in `<hermes home>/plugin-data/pyxray/cards/`, indexed by
`recent.jsonl` beside them; the newest `max_cards` are kept. A card is a render
of the analysis, not of the code — sources stay in memory for `/pyxray show`.

## The shell-hook route

Before the plugin, pyxray reached Hermes through its shell-hook bridge: a
`hooks:` block in `config.yaml` running `integrations/claude-code/hooks/pyxray-hook`
on `pre_tool_call`. `hooks.yaml` and `install.sh` still do that, for a Hermes
without the plugin API or a machine where the plugin cannot import `pyxray`
but a script can. It records and gates; it cannot append the verdict or feed
the desktop pane. Do not run both — each call would be recorded twice.

```sh
integrations/hermes/install.sh            # print the block to paste
integrations/hermes/install.sh --append   # append it, if that is safe
export HERMES_ACCEPT_HOOKS=1              # consent for non-TTY runs
```
