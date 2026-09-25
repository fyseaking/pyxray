"""``pyxray-mcp`` — pyxray as an MCP server, so any agent can ask first.

The hooks watch what an agent runs whether it asks or not. This is the other
half: a tool the agent — or you, from a chat — can call *before* deciding to
run something. It speaks MCP over stdio, newline-delimited JSON-RPC 2.0, with
no dependencies (the MCP SDK needs Python 3.10; pyxray supports 3.9).

Tools, all read-only and none of them writing the feed:

    analyze        what a snippet or a script file will do
    extract        every piece of Python in a shell command, analysed
    check          the verdict a hook would give: allow, ask or deny, and why
    explain        how a risk score adds up
    feed           the most recent rows of the pyxray feed

Register it with, for example, ``claude mcp add pyxray -- pyxray-mcp``; the
integrations README has the Codex, Gemini CLI and Cursor spellings.
"""

from __future__ import annotations

import json
import os
import sys
from typing import Any, Callable

PROTOCOL = "2025-06-18"
MAX_FILE = 1 << 20

TOOLS: list[dict[str, Any]] = [
    {
        "name": "analyze",
        "title": "What will this Python do?",
        "description": (
            "Statically analyse a Python snippet or script file before running it: its "
            "side effects (files read, written, deleted; network; commands; environment; "
            "eval/pickle), where secrets flow, a 0-100 risk score and a one-line synopsis. "
            "Nothing is executed. Give `code`, or `path` to a .py file; with `path` (or "
            "`cwd`) the local modules it imports are analysed too."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "Python source"},
                "path": {"type": "string", "description": "a .py file to read instead"},
                "cwd": {"type": "string", "description": "where the code would run, for imports and relative paths"},
                "follow": {"type": "boolean", "description": "analyse local imports (default true)"},
            },
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "extract",
        "title": "What Python is in this shell command?",
        "description": (
            "Find every piece of Python a shell command would run — heredocs, python -c, "
            "script files, pipes into python, bash -c, uv/poetry run — and analyse each."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "cwd": {"type": "string", "description": "where the command would run"},
            },
            "required": ["command"],
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "check",
        "title": "Would pyxray let this run?",
        "description": (
            "The verdict pyxray's hooks would give a shell command or Python snippet: "
            "allow, ask or deny, under the risk gate (PYXRAY_GATE) and the policy in "
            "effect (.pyxray.toml), with the reason."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "a shell command"},
                "code": {"type": "string", "description": "or Python source"},
                "cwd": {"type": "string"},
            },
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "explain",
        "title": "Why this risk score?",
        "description": "The terms a snippet's risk score is the sum of, each with its reason, and the band it lands in.",
        "inputSchema": {
            "type": "object",
            "properties": {"code": {"type": "string"}, "cwd": {"type": "string"}},
            "required": ["code"],
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "feed",
        "title": "What has run lately?",
        "description": "The most recent rows of the pyxray feed — every snippet any pyxray hook, shim or plugin has seen — newest first.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "n": {"type": "integer", "minimum": 1, "maximum": 200},
                "floor": {"type": "string", "enum": ["inert", "routine", "check", "read"]},
            },
        },
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
]

BANDS = ("inert", "routine", "check", "read")


class ToolError(Exception):
    """A problem with the call's arguments, reported to the caller as such."""


# --------------------------------------------------------------------------- tools


def _engine() -> Any:
    from ._native import engine

    return engine()


def _read_path(path: str) -> str:
    if not os.path.isfile(path):
        raise ToolError(f"not a file: {path}")
    if os.path.getsize(path) > MAX_FILE:
        raise ToolError(f"{path} is larger than {MAX_FILE} bytes")
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def _summary(report: dict[str, Any]) -> dict[str, Any]:
    """The parts of a report worth an agent's context, in one small object."""
    meta, metrics = report.get("meta", {}), report.get("metrics", {})
    effects = list(report.get("effects") or [])
    for linked in report.get("linked") or []:
        effects.extend(linked.get("effects") or [])
    notable = [
        {
            k: v
            for k, v in {
                "effect": h.get("effect"),
                "severity": h.get("severity"),
                "verb": h.get("verb"),
                "target": h.get("target"),
                "line": h.get("line"),
                "file": h.get("file"),
                "note": h.get("note"),
                "flow": (h.get("flow") or {}).get("kind"),
            }.items()
            if v is not None
        }
        for h in effects
        if h.get("severity") in ("notable", "caution")
    ]
    return {
        "name": meta.get("name"),
        "synopsis": meta.get("synopsis"),
        "risk": metrics.get("risk"),
        "parsed": meta.get("parsed"),
        "effects": notable[:20],
        "more": max(0, len(notable) - 20),
        "imports_followed": [m.get("file") for m in report.get("linked") or []],
        "diagnostics": [d.get("message") for d in report.get("diagnostics") or []][:5],
    }


def _text(summary: dict[str, Any]) -> str:
    lines = [f"{summary.get('name')}: risk {summary.get('risk')} — {summary.get('synopsis')}"]
    for e in summary.get("effects", []):
        where = f"{e['file']}:{e.get('line')}" if e.get("file") else f"line {e.get('line')}"
        bit = f"  {e.get('severity')}: {e.get('verb')} {e.get('target') or ''}".rstrip()
        bit += f" ({where})"
        if e.get("note"):
            bit += f" — {e['note']}"
        lines.append(bit)
    if summary.get("more"):
        lines.append(f"  … and {summary['more']} more")
    if summary.get("imports_followed"):
        lines.append("  followed: " + ", ".join(summary["imports_followed"]))
    return "\n".join(lines)


def tool_analyze(args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    code, path, cwd = args.get("code"), args.get("path"), args.get("cwd")
    if not code and not path:
        raise ToolError("give `code` or `path`")
    if path:
        code = _read_path(path)
    name = os.path.basename(path) if path else "<snippet>"
    report = _engine().analyze(
        code,
        name,
        origin=os.path.abspath(path) if path else None,
        cwd=cwd,
        follow=bool(args.get("follow", True)),
    )
    summary = _summary(report)
    return _text(summary), summary


def tool_extract(args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    command = args.get("command")
    if not isinstance(command, str) or not command.strip():
        raise ToolError("give `command`")
    cwd = args.get("cwd")
    found = _engine().extract_all(command, cwd=cwd)
    items = []
    for f in found:
        entry: dict[str, Any] = {"label": f.get("label"), "path": f.get("path")}
        if f.get("source", "").strip():
            report = _engine().analyze(f["source"], f.get("label") or "<snippet>", origin=f.get("path"), cwd=cwd)
            entry.update(_summary(report))
        items.append(entry)
    if not items:
        return "no Python in that command", {"snippets": []}
    text = "\n".join(_text(i) if "risk" in i else f"{i['label']}: not readable" for i in items)
    return text, {"snippets": items}


def tool_check(args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    from . import verdict

    command, code, cwd = args.get("command"), args.get("code"), args.get("cwd")
    if command:
        payload = {"tool_name": "bash", "tool_input": {"command": command}, "cwd": cwd}
    elif code:
        payload = {"tool_name": "execute_code", "tool_input": {"code": code}, "cwd": cwd}
    else:
        raise ToolError("give `command` or `code`")
    findings = verdict.scan_payload(payload, source="mcp", record=False)
    v = verdict.judge(findings, verdict.Settings.from_env())
    worst = findings.worst
    result = {
        "decision": v.decision,
        "reason": v.message or ("nothing to stop" if findings.snippets else "no Python in it"),
        "rule": v.rule,
        "risk": worst.risk if worst else 0,
        "synopsis": (worst.event.get("synopsis") if worst else None),
    }
    return f"{v.decision}: {result['reason']}", result


def tool_explain(args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    code = args.get("code")
    if not isinstance(code, str):
        raise ToolError("give `code`")
    report = _engine().analyze(code, "<snippet>", cwd=args.get("cwd"))
    terms = report.get("risk_terms") or []
    risk = (report.get("metrics") or {}).get("risk", 0)
    from . import verdict

    # The band comes from the core (severity first, then the score), via the
    # same event a hook would record — without recording it.
    event, _ = _engine().look(code, name="<snippet>", source="mcp", record=False, draw=False,
                              cwd=args.get("cwd"))
    band = str(event.get("band") or "inert")
    lines = [f"risk {risk} ({verdict.BAND_WORDS.get(band, band)})"]
    lines += [f"  +{t['points']:<3} {t['why']}" for t in terms]
    if sum(t["points"] for t in terms) > 100:
        lines.append("  (capped at 100)")
    return "\n".join(lines), {"risk": risk, "band": band, "terms": terms}


def tool_feed(args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    n = int(args.get("n") or 20)
    n = max(1, min(200, n))
    floor = args.get("floor") or "inert"
    if floor not in BANDS:
        raise ToolError(f"floor must be one of {', '.join(BANDS)}")
    rows = [r for r in _engine().read_feed() if BANDS.index(r.get("band", "inert")) >= BANDS.index(floor)]
    rows = rows[-n:][::-1]
    from . import verdict

    lines = [
        f"{verdict.BAND_WORDS.get(r.get('band'), r.get('band')):<14}{r.get('risk', 0):>3}  "
        f"{r.get('synopsis') or '—'}  ({r.get('source')}, {r.get('name')})"
        for r in rows
    ]
    return ("\n".join(lines) or "the feed is empty"), {"rows": rows}


HANDLERS: dict[str, Callable[[dict[str, Any]], tuple[str, dict[str, Any]]]] = {
    "analyze": tool_analyze,
    "extract": tool_extract,
    "check": tool_check,
    "explain": tool_explain,
    "feed": tool_feed,
}


# --------------------------------------------------------------------------- protocol


def _version() -> str:
    try:
        from . import __version__

        return __version__
    except Exception:  # noqa: BLE001
        return "0"


def handle(message: Any) -> dict[str, Any] | None:
    """One JSON-RPC message in, its response out (``None`` for notifications)."""
    if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
        return _error(None, -32600, "invalid request")
    mid = message.get("id")
    method = message.get("method")
    params = message.get("params") or {}
    if mid is None:
        return None  # a notification: initialized, cancelled, …
    try:
        if method == "initialize":
            requested = params.get("protocolVersion") if isinstance(params, dict) else None
            return _ok(mid, {
                "protocolVersion": requested or PROTOCOL,
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "pyxray", "title": "pyxray", "version": _version()},
                "instructions": (
                    "Call `check` or `analyze` before running Python or a shell command "
                    "that runs Python; both are static and never execute anything."
                ),
            })
        if method == "ping":
            return _ok(mid, {})
        if method == "tools/list":
            return _ok(mid, {"tools": TOOLS})
        if method == "tools/call":
            name = params.get("name")
            handler = HANDLERS.get(name)
            if handler is None:
                return _error(mid, -32602, f"unknown tool {name!r}")
            args = params.get("arguments") or {}
            if not isinstance(args, dict):
                return _error(mid, -32602, "arguments must be an object")
            try:
                text, structured = handler(args)
            except ToolError as exc:
                return _ok(mid, {"content": [{"type": "text", "text": str(exc)}], "isError": True})
            except Exception as exc:  # noqa: BLE001 — a missing engine, a bad file
                return _ok(mid, {"content": [{"type": "text", "text": f"pyxray: {exc}"}], "isError": True})
            return _ok(mid, {"content": [{"type": "text", "text": text}], "structuredContent": structured})
        return _error(mid, -32601, f"method not found: {method}")
    except Exception as exc:  # noqa: BLE001
        return _error(mid, -32603, f"internal error: {exc}")


def _ok(mid: Any, result: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": mid, "result": result}


def _error(mid: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": mid, "error": {"code": code, "message": message}}


def main() -> int:
    # Nothing but protocol may reach stdout; renders and debug go to stderr.
    os.environ.setdefault("PYXRAY_DRAW", "never")
    out = sys.stdout
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            response: dict[str, Any] | None = _error(None, -32700, "parse error")
        else:
            if isinstance(message, list):  # a batch
                responses = [r for r in (handle(m) for m in message) if r is not None]
                response = responses or None  # type: ignore[assignment]
            else:
                response = handle(message)
        if response is not None:
            out.write(json.dumps(response, ensure_ascii=False) + "\n")
            out.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
