"""``python -m pyxray_agent.install HARNESS`` — wire pyxray into a harness's hooks.

    HARNESS   codex | gemini | cursor
    --apply   write the config (a backup is kept); without it, print it
    --project DIR   the repo-level config under DIR instead of your user one
    --uninstall     remove pyxray's entries and nothing else

Works the same on Windows, macOS and Linux. The hook command names this
Python by its absolute path, so it runs with the interpreter that can import
``pyxray_agent`` regardless of what is first on the harness's PATH — or, from
a checkout, the launcher next to the Claude Code plugin, which searches for
one.

The installer only ever touches pyxray's own entries: anything else in the
file — other hooks, other settings — is kept as it was, and a backup of the
original is written beside it.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path
from typing import Any

MARK = "pyxray"


def _checkout_launcher() -> Path | None:
    """The launcher in the checkout this package was imported from, if any."""
    here = Path(__file__).resolve().parent
    root = here.parent.parent
    hooks = root / "integrations" / "claude-code" / "hooks"
    launcher = hooks / ("pyxray-hook.cmd" if os.name == "nt" else "pyxray-hook")
    return launcher if launcher.is_file() else None


def _quote(path: str) -> str:
    return f'"{path}"' if (" " in path or "\\" in path) else path


def hook_command(harness: str, phase: str) -> str:
    """The command a harness should run for one phase."""
    launcher = _checkout_launcher()
    if launcher is not None:
        prefix = _quote(str(launcher)) if os.name == "nt" else f"sh {_quote(str(launcher))}"
        return f"{prefix} {harness} {phase}"
    return f"{_quote(sys.executable)} -m pyxray_agent.hook {harness} {phase}"


# --------------------------------------------------------------------------- per harness


def _codex(project: Path | None) -> tuple[Path, dict[str, Any]]:
    path = (project / ".codex" if project else Path.home() / ".codex") / "hooks.json"
    entry = {
        "matcher": "^(Bash|exec_command|shell)$",
        "hooks": [{"type": "command", "command": hook_command("codex", "pre"), "timeout": 10}],
    }
    return path, {"hooks": {"PreToolUse": [entry]}}


def _gemini(project: Path | None) -> tuple[Path, dict[str, Any]]:
    path = (project / ".gemini" if project else Path.home() / ".gemini") / "settings.json"

    def entry(phase: str) -> dict[str, Any]:
        return {
            "matcher": "run_shell_command",
            "hooks": [
                {
                    "name": f"pyxray-{phase}",
                    "type": "command",
                    "command": hook_command("gemini", phase),
                    "timeout": 10000,
                    "description": "pyxray: what the Python in this command will do",
                }
            ],
        }

    return path, {"hooks": {"BeforeTool": [entry("pre")], "AfterTool": [entry("post")]}}


def _cursor(project: Path | None) -> tuple[Path, dict[str, Any]]:
    path = (project / ".cursor" if project else Path.home() / ".cursor") / "hooks.json"
    return path, {
        "version": 1,
        "hooks": {
            "beforeShellExecution": [{"command": hook_command("cursor", "pre"), "timeout": 10}],
            "postToolUse": [{"command": hook_command("cursor", "post"), "matcher": "Shell", "timeout": 10}],
        },
    }


HARNESSES = {"codex": _codex, "gemini": _gemini, "cursor": _cursor}


# --------------------------------------------------------------------------- merging


def _ours(entry: Any) -> bool:
    """Whether a hooks-list entry is one pyxray wrote."""
    text = json.dumps(entry)
    return "pyxray_agent.hook" in text or "pyxray-hook" in text


def merge(existing: dict[str, Any], ours: dict[str, Any], *, remove: bool = False) -> dict[str, Any]:
    """``existing`` with pyxray's hook entries replaced by ``ours`` (or dropped)."""
    out = dict(existing)
    for key, value in ours.items():
        if key != "hooks":
            out.setdefault(key, value)
            continue
        hooks = dict(out.get("hooks") or {})
        for event, entries in value.items():
            kept = [e for e in (hooks.get(event) or []) if not _ours(e)]
            hooks[event] = kept if remove else kept + entries
            if not hooks[event]:
                hooks.pop(event)
        out["hooks"] = hooks
    return out


def _load(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8").strip()
    if not text:
        return {}
    data = json.loads(text)
    if not isinstance(data, dict):
        raise ValueError(f"{path} is not a JSON object")
    return data


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m pyxray_agent.install", description=__doc__.split("\n\n")[0])
    ap.add_argument("harness", choices=sorted(HARNESSES))
    ap.add_argument("--apply", action="store_true", help="write the config (keeps a backup)")
    ap.add_argument("--project", type=Path, help="the repo-level config under this directory")
    ap.add_argument("--uninstall", action="store_true", help="remove pyxray's entries")
    a = ap.parse_args(argv)

    path, ours = HARNESSES[a.harness](a.project.resolve() if a.project else None)
    try:
        existing = _load(path)
    except (ValueError, json.JSONDecodeError) as exc:
        print(f"pyxray: {path} is not valid JSON ({exc}); fix it or merge by hand:", file=sys.stderr)
        print(json.dumps(ours, indent=2))
        return 1
    merged = merge(existing, ours, remove=a.uninstall)

    if not a.apply:
        print(f"# {'would remove pyxray from' if a.uninstall else 'would write'} {path}")
        print(json.dumps(merged, indent=2))
        print("# run again with --apply to write it", file=sys.stderr)
        return 0

    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        backup = path.with_name(f"{path.name}.bak.{time.strftime('%Y%m%d_%H%M%S')}")
        shutil.copy2(path, backup)
        print(f"pyxray: backup at {backup}")
    path.write_text(json.dumps(merged, indent=2) + "\n", encoding="utf-8")
    print(f"pyxray: {'removed from' if a.uninstall else 'wired into'} {path}")
    if not a.uninstall:
        print("pyxray: set PYXRAY_GATE to have calls over a risk score stopped; `pyx watch` shows the feed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
