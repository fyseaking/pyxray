"""``python -m pyxray_agent.hook HARNESS PHASE`` — the one hook, every harness.

    HARNESS  claude | codex | gemini | cursor
    PHASE    pre  (before the tool runs: record, and ask or refuse if gated)
             post (after it ran: tell the model what it just did, if worth it)

Reads the harness's payload on stdin, writes the harness's answer on stdout,
exits 0. Always 0: an exit code is how several harnesses spell "block", and
an observer that breaks must not block. Cursor, which reads silence as a
refusal, gets an explicit allow whenever nothing else was printed.

Settings come from the environment, the same for every harness:
``PYXRAY_GATE`` (risk ceiling, or ``off``), ``PYXRAY_GATE_MODE=deny`` (refuse
instead of asking), ``PYXRAY_WARN`` (the lowest band worth a post-tool note:
``routine``, ``check``, ``read`` or ``never``), ``PYXRAY_OFF=1``.
"""

from __future__ import annotations

import json
import os
import sys
from typing import Any

PHASES = ("pre", "post")


def _debug(msg: str) -> None:
    if os.environ.get("PYXRAY_DEBUG") == "1":
        print(f"pyxray: {msg}", file=sys.stderr)


def answer(harness: str, phase: str, raw: Any) -> dict[str, Any] | None:
    """The harness's answer to one payload, or ``None`` to say nothing.

    Pure apart from the feed write (pre only), so the tests drive it
    directly with fixture payloads.
    """
    from . import adapters, verdict

    adapter = adapters.get(harness)
    payload = adapter.normalize(raw) if isinstance(raw, dict) else None
    if payload is None:
        return adapter.FALLBACK if phase == "pre" else None
    settings = verdict.Settings.from_env()
    source = str(payload.get("hook_source") or os.environ.get("PYXRAY_SOURCE") or harness)
    findings = verdict.scan_payload(payload, source=source, record=(phase == "pre"))
    if findings.error:
        _debug(findings.error)
    if phase == "pre":
        return adapter.pre(verdict.judge(findings, settings))
    return adapter.post(verdict.post_context(findings, settings))


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    harness = args[0] if args else "claude"
    phase = args[1] if len(args) > 1 else "pre"
    printed = False
    fallback: dict[str, Any] | None = None
    try:
        from . import adapters

        adapter = adapters.get(harness)
        fallback = adapter.FALLBACK if phase == "pre" else None
        if phase not in PHASES:
            _debug(f"unknown phase {phase!r}; want pre or post")
            return 0
        try:
            raw = json.load(sys.stdin)
        except (json.JSONDecodeError, ValueError, OSError):
            return 0
        out = answer(harness, phase, raw)
        if out is not None:
            json.dump(out, sys.stdout)
            sys.stdout.flush()
            printed = True
    except BaseException as exc:  # noqa: BLE001 — never fail the tool call
        _debug(f"hook failed: {exc!r}")
    finally:
        if not printed and fallback is not None:
            try:
                json.dump(fallback, sys.stdout)
                sys.stdout.flush()
            except Exception:  # noqa: BLE001
                pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
