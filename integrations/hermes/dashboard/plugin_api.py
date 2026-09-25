"""pyxray — backend routes for the Hermes desktop pane.

Mounted by the Hermes web server at ``/api/plugins/pyxray/``. The desktop
half (``../desktop/plugin.js``) reaches them through the SDK's scoped
``ctx.rest`` door, so nothing here is reachable from a page.

Reads two things, both written elsewhere:

- the pyxray feed (``feed.jsonl``) — every snippet any pyxray integration
  has seen, Hermes' included;
- ``<hermes home>/plugin-data/pyxray/recent.jsonl`` + ``cards/`` — the
  per-call HTML cards the agent half rendered.

Only reads. The one write, ``POST /gate``, goes through Hermes' own config
API when the agent half is importable, and refuses otherwise.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path
from typing import Any

from fastapi import APIRouter, HTTPException, Query

router = APIRouter()

_HERE = Path(__file__).resolve().parent
_PLUGIN = _HERE.parent


def _guard() -> Any:
    """Load ``../guard.py`` without needing the plugin package importable here."""
    name = "hermes_dashboard_plugin_pyxray_guard"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, _PLUGIN / "guard.py")
    if spec is None or spec.loader is None:
        raise HTTPException(500, "pyxray guard module missing")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def _feed_rows(limit: int) -> list[dict[str, Any]]:
    guard = _guard()
    try:
        return list(guard.pyxray_module().feed())[-limit:][::-1]
    except Exception:  # noqa: BLE001 — engine missing here is fine, the file may still exist
        path = os.environ.get("PYXRAY_LOG") or guard.feed_path_fallback()
    rows: list[dict[str, Any]] = []
    try:
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if isinstance(row, dict):
                    rows.append(row)
    except OSError:
        return []
    return rows[-limit:][::-1]


@router.get("/status")
async def status() -> dict[str, Any]:
    guard = _guard()
    info = guard.engine_status()
    settings = guard.Settings.load()
    return {
        "engine": info,
        "gate": settings.gate,
        "mode": settings.mode,
        "warn_from": settings.warn_from,
        "tools": list(settings.tools),
        "cards": settings.cards,
        "data_dir": str(guard.data_dir()),
        "env_gate": os.environ.get("PYXRAY_GATE"),
    }


@router.get("/events")
async def events(limit: int = Query(60, ge=1, le=500), source: str = Query("all")) -> dict[str, Any]:
    """Feed rows newest first. ``source=hermes`` keeps only this harness' rows."""
    rows = _feed_rows(limit * 4 if source != "all" else limit)
    if source != "all":
        rows = [r for r in rows if str(r.get("source", "")).startswith(source)][:limit]
    return {"rows": rows}


@router.get("/observed")
async def observed(limit: int = Query(500, ge=1, le=5000)) -> dict[str, Any]:
    """What audited runs actually did (`PYXRAY_AUDIT=1` through the shim), by
    run id, for joining to the events that predicted them."""
    guard = _guard()
    try:
        path = guard.pyxray_module().feed_path()
    except Exception:  # noqa: BLE001
        path = os.environ.get("PYXRAY_LOG") or guard.feed_path_fallback()
    runs: dict[str, list[dict[str, Any]]] = {}
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.readlines()[-limit * 4:]
    except OSError:
        return {"runs": {}}
    for line in lines:
        if '"observed"' not in line:
            continue
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and row.get("kind") == "observed" and row.get("run"):
            runs.setdefault(str(row["run"]), []).extend(row.get("items") or [])
    return {"runs": dict(list(runs.items())[-limit:])}


@router.get("/recent")
async def recent(limit: int = Query(40, ge=1, le=200)) -> dict[str, Any]:
    """The card index: one entry per Hermes tool call that had Python in it."""
    return {"rows": _guard().read_recent(limit)}


@router.get("/card/{name}")
async def card(name: str) -> dict[str, Any]:
    """One rendered card, as ``{"html": …}`` (``ctx.rest`` expects JSON)."""
    if "/" in name or "\\" in name or ".." in name or not name.endswith(".html"):
        raise HTTPException(400, "bad card name")
    path = _guard().data_dir() / "cards" / name
    try:
        return {"name": name, "html": path.read_text(encoding="utf-8")}
    except OSError as exc:
        raise HTTPException(404, "no such card") from exc


@router.post("/gate")
async def set_gate(body: dict) -> dict[str, Any]:
    """Change the gate in config.yaml (``plugins.entries.pyxray.settings.gate``)."""
    guard = _guard()
    value = guard.parse_gate(body.get("gate"))
    try:
        guard.save_gate(value)
    except ImportError as exc:
        raise HTTPException(501, "config API unavailable in this process") from exc
    except Exception as exc:  # noqa: BLE001 — managed install, locked file, …
        raise HTTPException(403, f"could not save: {exc}") from exc
    return {"gate": value, "env_gate": os.environ.get("PYXRAY_GATE")}
