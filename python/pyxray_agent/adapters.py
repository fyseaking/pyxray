"""Harness dialects: each one's hook payload in, its answer out.

Every harness here has a hook that runs a command with a JSON payload on
stdin before a tool call (and most, after one), and reads JSON back from
stdout. They differ only in field names and in how "ask the human", "refuse"
and "tell the model this" are spelled. An adapter is those spellings and
nothing else; what to say is decided once, in :mod:`pyxray_agent.verdict`.

    normalize(raw)  →  the Claude-shaped {tool_name, tool_input, cwd, session_id}
    pre(verdict)    →  the harness's pre-tool answer, or None for "say nothing"
    post(text)      →  the harness's after-tool answer, or None
    FALLBACK        →  what to print when anything at all goes wrong

``FALLBACK`` exists for Cursor, which treats a permission hook that prints
nothing (or invalid JSON) as a refusal. For every other harness silence
means "carry on", which is also what a broken observer should mean.
"""

from __future__ import annotations

from typing import Any

from .verdict import Verdict

__all__ = ["ADAPTERS", "Adapter", "get"]


class Adapter:
    """The Claude Code dialect, which Codex and Hermes' shell hooks also speak."""

    name = "claude"
    FALLBACK: dict[str, Any] | None = None

    def normalize(self, raw: dict[str, Any]) -> dict[str, Any] | None:
        if not isinstance(raw, dict):
            return None
        payload = dict(raw)
        args = payload.get("tool_input")
        if isinstance(args, dict) and not payload.get("cwd"):
            for key in ("cwd", "workdir", "working_directory", "directory"):
                if isinstance(args.get(key), str) and args[key]:
                    payload["cwd"] = args[key]
                    break
        return payload

    def pre(self, verdict: Verdict) -> dict[str, Any] | None:
        if verdict.decision == "allow":
            return None
        reason = verdict.message
        return {
            # Read by Hermes' shell-hook bridge and by the OpenCode plugin,
            # which both predate hookSpecificOutput.
            "decision": "block",
            "reason": reason,
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": verdict.decision,
                "permissionDecisionReason": reason,
            },
        }

    def post(self, text: str | None) -> dict[str, Any] | None:
        if not text:
            return None
        return {
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": text,
            }
        }


class Codex(Adapter):
    """Codex CLI. Its hooks are Claude-shaped; the shell tool is ``Bash`` or
    ``exec_command``, and a command can arrive as an argv array, which
    :func:`pyxray_agent.intercept.snippets_in_payload` quotes back together.

    Codex's PreToolUse honours ``allow`` and ``deny`` but has no "ask", and its
    PostToolUse is audit-only. So an over-gate call is refused with the
    reason, and the post phase is not installed for Codex (the adapter still
    answers in the Claude shape, which Codex ignores harmlessly).
    """

    name = "codex"

    def pre(self, verdict: Verdict) -> dict[str, Any] | None:
        if verdict.decision == "allow":
            return None
        reason = verdict.message
        if verdict.decision == "ask":
            reason += (
                "\nCodex hooks cannot ask, so pyxray refused instead. Raise PYXRAY_GATE, "
                "or have the user run it themselves."
            )
        return {
            "decision": "block",
            "reason": reason,
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            },
        }


class Gemini(Adapter):
    """Gemini CLI: ``BeforeTool`` / ``AfterTool`` on ``run_shell_command``.

    Gemini has no "ask": a hook allows or denies. An over-gate call is denied
    with the reason, which reaches the model as the tool's error — it can
    explain, and the person can raise the gate or run the command themselves.
    A deny is always spelled in JSON, never as exit code 2: the exit code of
    an observer stays 0 whatever happens.
    """

    name = "gemini"

    def normalize(self, raw: dict[str, Any]) -> dict[str, Any] | None:
        payload = super().normalize(raw)
        if payload is None:
            return None
        # run_shell_command's `directory` is relative to the project root.
        args = payload.get("tool_input")
        if isinstance(args, dict) and isinstance(args.get("directory"), str):
            import os

            base = raw.get("cwd") or ""
            payload["cwd"] = os.path.join(base, args["directory"]) if base else args["directory"]
        return payload

    def pre(self, verdict: Verdict) -> dict[str, Any] | None:
        if verdict.decision == "allow":
            return None
        reason = verdict.message
        if verdict.decision == "ask":
            reason += (
                "\nGemini CLI cannot ask, so pyxray refused instead. Raise PYXRAY_GATE, "
                "or have the user run it themselves."
            )
        return {"decision": "deny", "reason": reason}

    def post(self, text: str | None) -> dict[str, Any] | None:
        if not text:
            return None
        return {"hookSpecificOutput": {"hookEventName": "AfterTool", "additionalContext": text}}


class Cursor(Adapter):
    """Cursor: ``beforeShellExecution`` ({command, cwd}) or ``preToolUse``
    (``Shell``) before, ``postToolUse`` after.

    A Cursor permission hook must always answer: empty or invalid output is
    taken as a refusal. So the fallback is an explicit allow, printed by
    :mod:`pyxray_agent.hook` whenever nothing else was.
    """

    name = "cursor"
    FALLBACK = {"permission": "allow"}

    def normalize(self, raw: dict[str, Any]) -> dict[str, Any] | None:
        if not isinstance(raw, dict):
            return None
        if raw.get("tool_name"):
            payload = super().normalize(raw)
            if payload is not None and str(payload["tool_name"]).lower() == "shell":
                payload["tool_name"] = "shell"
            return payload
        command = raw.get("command")
        if not isinstance(command, str):
            return None
        return {
            "tool_name": "shell",
            "tool_input": {"command": command},
            "cwd": raw.get("cwd"),
            "session_id": raw.get("conversation_id") or raw.get("session_id"),
        }

    def pre(self, verdict: Verdict) -> dict[str, Any] | None:
        if verdict.decision == "allow":
            return dict(self.FALLBACK or {})
        return {
            "permission": verdict.decision,
            "user_message": verdict.reason,
            "agent_message": verdict.message,
        }

    def post(self, text: str | None) -> dict[str, Any] | None:
        if not text:
            return None
        return {"additional_context": text}


ADAPTERS: dict[str, Adapter] = {
    "claude": Adapter(),
    "codex": Codex(),
    "gemini": Gemini(),
    "cursor": Cursor(),
}


def get(name: str) -> Adapter:
    try:
        return ADAPTERS[name.strip().lower()]
    except KeyError:
        raise ValueError(f"unknown harness {name!r}; one of {', '.join(ADAPTERS)}") from None
