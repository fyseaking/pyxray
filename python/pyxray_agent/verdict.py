"""One verdict for every harness.

Each harness asks the same two questions in its own dialect — before a tool
runs, "should this go ahead?"; after, "is there anything the model should
know?" — and this module answers them once. The adapters in
:mod:`pyxray_agent.adapters` only translate: a :class:`Verdict` in, a
harness's JSON out.

    payload ──scan_payload()──▶ Findings ──judge()──────▶ Verdict  (pre)
                                         └─post_context()▶ text     (post)

Nothing here raises. A missing engine, a payload of the wrong shape or a bug
in the analyser gives empty findings and an ``allow``: pyxray sits in front
of somebody else's work and must never be the reason it fails.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Any, Literal

__all__ = [
    "BANDS",
    "BAND_WORDS",
    "Findings",
    "Settings",
    "Snippet",
    "Verdict",
    "context_text",
    "judge",
    "note_lines",
    "parse_gate",
    "post_context",
    "scan_payload",
    "wants_warning",
]

# Band order, least to most interesting. Mirrors pyxray-core's `Band`.
BANDS = ("inert", "routine", "check", "read")
BAND_WORDS = {"inert": "inert", "routine": "routine", "check": "check it", "read": "read it first"}

Decision = Literal["allow", "ask", "deny"]
_STRICTNESS = {"allow": 0, "ask": 1, "deny": 2}


def parse_gate(raw: Any) -> int | None:
    """``None``/``off``/``none``/``-1``/empty → no gate; otherwise clamp to 0..100.

    Accepts what a config file or an environment variable might hold: ints,
    floats, numeric strings. Anything unreadable is no gate, not an error —
    a typo in a setting should not stop every tool call.
    """
    if raw is None or isinstance(raw, bool):
        return None
    if isinstance(raw, (int, float)):
        value = int(raw)
        return None if value < 0 else max(0, min(100, value))
    text = str(raw).strip().lower()
    if text in ("", "off", "none", "-1", "null"):
        return None
    try:
        return max(0, min(100, int(text)))
    except ValueError:
        return None


@dataclass(frozen=True)
class Settings:
    """What a harness-neutral verdict depends on.

    ``gate``: risk above this asks (``None``: describe only). ``hard``: ask
    becomes deny, for places with nobody to ask. ``warn_from``: the lowest
    band whose findings are worth telling the model about after the fact.
    """

    gate: int | None = None
    hard: bool = False
    warn_from: str = "check"

    @classmethod
    def from_env(cls) -> "Settings":
        warn = (os.environ.get("PYXRAY_WARN") or "check").strip().lower()
        if warn not in BANDS and warn != "never":
            warn = "check"
        hard = (os.environ.get("PYXRAY_GATE_MODE") or "").strip().lower() in ("deny", "block", "hard")
        return cls(gate=parse_gate(os.environ.get("PYXRAY_GATE")), hard=hard, warn_from=warn)


@dataclass
class Snippet:
    source: str
    label: str
    event: dict[str, Any]
    path: str | None = None

    @property
    def band(self) -> str:
        return str(self.event.get("band") or "inert")

    @property
    def risk(self) -> int:
        try:
            return int(self.event.get("risk") or 0)
        except (TypeError, ValueError):
            return 0


@dataclass
class Findings:
    tool: str
    snippets: list[Snippet] = field(default_factory=list)
    error: str | None = None

    @property
    def worst(self) -> Snippet | None:
        return max(
            self.snippets,
            key=lambda s: (s.risk, BANDS.index(s.band) if s.band in BANDS else 0),
            default=None,
        )

    @property
    def empty(self) -> bool:
        return not self.snippets


@dataclass(frozen=True)
class Verdict:
    decision: Decision
    reason: str = ""
    detail: tuple[str, ...] = ()
    #: What decided it: ``gate``, or a policy rule's name. ``None`` for allow.
    rule: str | None = None

    @property
    def message(self) -> str:
        """The reason and its detail lines, as one block of text."""
        return "\n".join([self.reason, *self.detail]) if self.detail else self.reason


ALLOW = Verdict("allow")


def scan_payload(
    payload: dict[str, Any],
    *,
    source: str = "hook",
    record: bool = True,
) -> Findings:
    """Find and analyse every piece of Python a tool call would run.

    ``payload`` is the Claude-shaped ``{tool_name, tool_input, cwd,
    session_id}`` the adapters normalise every harness to. With ``record``,
    each snippet becomes one feed row; a post-tool look passes
    ``record=False`` so a call is never logged twice.
    """
    tool = str(payload.get("tool_name") or payload.get("tool") or "")
    found = Findings(tool=tool)
    if os.environ.get("PYXRAY_OFF") == "1" or not isinstance(payload, dict):
        return found
    try:
        from . import intercept

        items = intercept.snippets_in_payload(payload)
    except Exception as exc:  # noqa: BLE001 — the observer must not break the observed
        found.error = f"could not read the payload: {exc}"
        return found
    session = str(payload.get("session_id") or "")[:8]
    origin = f"{source}{':' + session if session else ''}"
    for item in items:
        try:
            event = intercept.look(
                item["source"],
                item["label"],
                source=origin,
                record=record,
                origin=item.get("path"),
            )
        except Exception as exc:  # noqa: BLE001 — look() swallows already; belt and braces
            event = {"risk": 0, "band": "inert", "synopsis": "", "notes": [], "error": str(exc)}
        if event.get("error") and not found.error:
            found.error = str(event["error"])
        found.snippets.append(
            Snippet(source=item["source"], label=item["label"], event=event, path=item.get("path"))
        )
    return found


def judge(findings: Findings, settings: Settings) -> Verdict:
    """Should the call go ahead? The strictest of every reason to stop.

    Today the reasons are the risk gate and, when an event carries one, a
    policy verdict (``event["policy"]``, written by the core's policy
    engine). A missing engine is never a reason: the verdict is ``allow``.
    """
    best = ALLOW
    for snippet in findings.snippets:
        for candidate in (_gate_verdict(snippet, settings), _policy_verdict(snippet)):
            if candidate is None:
                continue
            if settings.hard and candidate.decision == "ask":
                candidate = Verdict("deny", candidate.reason, candidate.detail, candidate.rule)
            if _STRICTNESS[candidate.decision] > _STRICTNESS[best.decision]:
                best = candidate
    return best


def _gate_verdict(snippet: Snippet, settings: Settings) -> Verdict | None:
    if settings.gate is None or snippet.risk <= settings.gate:
        return None
    reason = (
        f"pyxray: risk {snippet.risk} is over the gate of {settings.gate} "
        f"({BAND_WORDS.get(snippet.band, snippet.band)}) — "
        f"{snippet.event.get('synopsis') or 'see notes'}"
    )
    return Verdict("ask", reason, tuple(note_lines(snippet.event)), "gate")


def _policy_verdict(snippet: Snippet) -> Verdict | None:
    policy = snippet.event.get("policy")
    if not isinstance(policy, dict):
        return None
    decision = str(policy.get("decision") or "allow")
    if decision not in ("ask", "deny"):
        return None
    rule = str(policy.get("rule") or "policy")
    reason = f"pyxray policy: {policy.get('reason') or decision} ({rule})"
    return Verdict(decision, reason, tuple(note_lines(snippet.event)), rule)  # type: ignore[arg-type]


def note_lines(event: dict[str, Any], limit: int = 4) -> list[str]:
    """``· verb target — note (line N)`` for the first few notes of an event."""
    lines: list[str] = []
    for note in (event.get("notes") or [])[:limit]:
        if not isinstance(note, dict):
            continue
        text = str(note.get("verb") or note.get("effect") or "").strip()
        if target := note.get("target"):
            text += f" {target}"
        if extra := note.get("note"):
            text += f" — {extra}"
        if file := note.get("file"):
            text += f" (in {file}"
            text += f", line {note['line']})" if note.get("line") else ")"
        elif line := note.get("line"):
            text += f" (line {line})"
        lines.append(f"  · {text}")
    return lines


def wants_warning(findings: Findings, settings: Settings) -> bool:
    """Is the worst snippet at or above the band worth telling the model about?"""
    worst = findings.worst
    if worst is None or settings.warn_from == "never":
        return False
    floor = BANDS.index(settings.warn_from) if settings.warn_from in BANDS else BANDS.index("check")
    band = BANDS.index(worst.band) if worst.band in BANDS else 0
    return band >= floor


def context_text(findings: Findings, settings: Settings) -> str:
    """The short Markdown block that rides back to the model after a call.

    One verdict line, the named effects, and one sentence telling the model
    what to do with it. The full picture is in the feed; this is the part
    worth spending the model's context on.
    """
    worst = findings.worst
    if worst is None:
        return ""
    header = f"pyxray — {BAND_WORDS.get(worst.band, worst.band)} · risk {worst.risk}"
    if synopsis := worst.event.get("synopsis"):
        header += f" · {synopsis}"
    lines = ["", "---", header]
    if len(findings.snippets) > 1:
        lines.append(f"({len(findings.snippets)} snippets in this call; worst shown: {worst.label})")
    lines += note_lines(worst.event)
    changed = worst.event.get("changed")
    if isinstance(changed, dict) and changed.get("summary"):
        lines.append(f"New since the last run of this script: {changed['summary']}")
    if worst.band == "read":
        lines.append(
            "This already ran. If any of those effects was not what the user asked for, "
            "say so now and do not repeat it; otherwise carry on."
        )
    elif settings.gate is not None:
        lines.append(f"Under the gate of {settings.gate}; recorded for the operator.")
    return "\n".join(lines)


def post_context(findings: Findings, settings: Settings) -> str | None:
    """The after-the-call text, or ``None`` when nothing is worth saying."""
    if findings.empty or not wants_warning(findings, settings):
        return None
    text = context_text(findings, settings)
    return text.strip("\n") or None
