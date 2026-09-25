"""Sit in front of a Python snippet and record what it will do.

Built for the case that actually happens: an agent firing snippets several
times a second. Two consequences shape everything here.

First, **the render is not for the model.** In most harnesses whatever a hook
or a wrapper writes to stdout or stderr is captured and fed back as tool
output, so drawing a report per snippet would cost tokens, confuse the model,
and scroll past the human anyway. So the default is to append a one-line event
to the feed log and draw nothing. Point ``pyx watch`` at that log in another
pane and you have a live column instead of a wall of text.

Second, **nothing is blocked unless you ask.** ``PYXRAY_GATE`` is unset by
default and pyxray only describes. Note that ``PYXRAY_GATE=0`` is not "no
gate" — it refuses anything with any effect at all, which is almost never what
someone means. ``off`` is spelled out for that reason.

And a rule above both: **the observer never breaks the thing it observes.**
A hook exits 0 whatever happens inside it, and the wrapper runs the user's
command whether or not the analysis worked. Set ``PYXRAY_DEBUG=1`` to hear
about the failures it would otherwise swallow.

Entry points
------------
``python -m pyxray_agent.intercept -- python3 script.py``
    Record, draw if watched, then run. This is what the PATH shim calls.

``python -m pyxray_agent.intercept --hook``
    A ``PreToolUse``-shaped hook. Reads a JSON payload on stdin, finds the
    Python in it (a tool that takes code, or a shell command with Python in
    it — a script path, a heredoc, ``-c``, ``bash -c``, a pipe), records it,
    and optionally asks the harness to stop.

Environment
    PYXRAY_OFF=1        do nothing at all
    PYXRAY_LOG          feed log path (default: the per-user runtime directory)
    PYXRAY_DRAW         never | tty | always   (default: tty)
    PYXRAY_TTY          draw to this device instead, e.g. /dev/pts/7
    PYXRAY_LAYOUT       line | auto | card | dashboard | stack | flow
    PYXRAY_THEME        blueprint | neon | paper | carbon | amber | ansi | mono
    PYXRAY_ICONS        glyph | tag | both
    PYXRAY_WIDTH        columns
    PYXRAY_GATE         off (default) or a risk score to refuse above
    PYXRAY_CONFIRM=1    ask before running (wrapper only)
    PYXRAY_MIN_LINES    ignore snippets shorter than this
    PYXRAY_SOURCE       label hook events with this origin
    PYXRAY_DEBUG=1      report swallowed errors on stderr
    PYXRAY_BIN          the pyx binary to use when the extension is absent
    PYXRAY_PYTHON       the interpreter the hook launcher should use
"""

from __future__ import annotations

import os
import re
import shlex
import shutil
import subprocess
import sys
from typing import Any

from ._native import engine

__all__ = ["main", "look", "should_run", "gate", "draw_target", "python_in_payload"]

#: Risk scores at or above this get the full card under ``layout=auto``.
CARD_FROM = 30

#: The event `look` returns when it has nothing to say.
INERT: dict[str, Any] = {
    "risk": 0,
    "band": "inert",
    "synopsis": "",
    "notes": [],
    "blocked": False,
    "mask": 0,
}


def _debug(msg: str) -> None:
    if os.environ.get("PYXRAY_DEBUG") == "1":
        print(f"pyxray: {msg}", file=sys.stderr)


def _env_int(name: str, default: int | None = None) -> int | None:
    raw = os.environ.get(name)
    if raw is None or not raw.strip():
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _width() -> int:
    if explicit := _env_int("PYXRAY_WIDTH"):
        return explicit
    return min(shutil.get_terminal_size(fallback=(100, 24)).columns, 180)


def gate() -> int | None:
    """The configured risk ceiling, or None when pyxray is only describing.

    ``off``, ``none`` and an empty value all mean no gate. A bare number is a
    ceiling — including ``0``, which really does refuse everything with an
    effect; that is a legitimate thing to want, just rarely the thing someone
    means when they type it.
    """
    raw = (os.environ.get("PYXRAY_GATE") or "").strip().lower()
    if raw in ("", "off", "none", "-1"):
        return None
    try:
        return max(0, min(100, int(raw)))
    except ValueError:
        print(f"pyxray: ignoring PYXRAY_GATE={raw!r} (want a number, or 'off')",
              file=sys.stderr)
        return None


def draw_target() -> Any:
    """Where the render should go, or None when nobody is watching.

    Defaults to "only if stderr is a real terminal", which is what keeps the
    picture out of a harness's captured tool output.
    """
    mode = (os.environ.get("PYXRAY_DRAW") or "tty").strip().lower()
    if mode == "never":
        return None
    if device := os.environ.get("PYXRAY_TTY"):
        try:
            return open(device, "w", encoding="utf-8")  # noqa: SIM115 — caller closes
        except OSError:
            return None
    if mode == "always":
        return sys.stderr
    try:
        return sys.stderr if sys.stderr.isatty() else None
    except (AttributeError, ValueError):
        return None


def look(
    code: str,
    name: str = "<stdin>",
    source: str = "shim",
    *,
    record: bool = True,
    origin: str | None = None,
    run: str | None = None,
) -> dict[str, Any]:
    """Record a snippet, and draw it if there is somewhere to draw.

    Returns the feed event: ``risk``, ``band``, ``synopsis``, ``notes`` and the
    capability ``mask``. Cheap enough to call on every snippet — the analysis
    is a couple of milliseconds and the render is skipped when unwatched.
    Never raises: if the engine is missing or fails, the event is inert and
    carries an ``error`` key.
    """
    if os.environ.get("PYXRAY_OFF") == "1":
        return dict(INERT)

    minimum = _env_int("PYXRAY_MIN_LINES", 0) or 0
    if minimum and len(code.splitlines()) < minimum:
        return dict(INERT)

    # A second look at a call already recorded draws nothing either: whoever
    # was watching saw it the first time.
    stream = draw_target() if record else None
    kwargs = dict(
        name=name,
        source=source,
        record=record,
        path=os.environ.get("PYXRAY_LOG") or None,
        draw=stream is not None,
        theme=os.environ.get("PYXRAY_THEME", "blueprint"),
        layout=os.environ.get("PYXRAY_LAYOUT", "auto"),
        icons=os.environ.get("PYXRAY_ICONS", "both"),
        width=_width(),
        format="ansi",
    )
    if origin:
        kwargs["origin"] = origin
    if run:
        kwargs["run"] = run
    try:
        event, rendered = engine().look(code, **kwargs)
    except Exception as exc:  # noqa: BLE001 — the observer must not break the observed
        _debug(f"could not analyse {name}: {exc}")
        event = dict(INERT)
        event["error"] = str(exc)
        rendered = ""
    if event.get("feed_error"):
        _debug(f"feed not written: {event['feed_error']}")
    if stream is not None and rendered:
        try:
            stream.write(rendered)
            stream.flush()
        except OSError:
            pass
        finally:
            if stream not in (sys.stderr, sys.stdout):
                stream.close()
    return event


def _tty_prompt(prompt: str) -> str | None:
    """Ask on the controlling terminal, even when stdin has been consumed."""
    device = "CON" if os.name == "nt" else "/dev/tty"
    try:
        with open(device, "r+", encoding="utf-8", errors="replace") as tty:
            tty.write(prompt)
            tty.flush()
            return tty.readline()
    except (OSError, ValueError):
        return None


def should_run(event: dict[str, Any]) -> bool:
    """Apply the gate, then the confirmation prompt. Both are opt-in."""
    risk = int(event.get("risk", 0))
    ceiling = gate()
    if ceiling is not None and risk > ceiling:
        worst = event.get("synopsis") or "no summary"
        print(f"pyxray: refusing to run — risk {risk} is over the gate of "
              f"{ceiling} ({worst})", file=sys.stderr)
        return False
    if os.environ.get("PYXRAY_CONFIRM") == "1":
        answer = _tty_prompt(f"pyxray: risk {risk}. Run it? [y/N] ")
        if answer is None:
            print("pyxray: PYXRAY_CONFIRM needs a terminal; not running", file=sys.stderr)
            return False
        if answer.strip().lower() not in ("y", "yes"):
            return False
    return True


def _source_for(argv: list[str]) -> tuple[str, str, bool]:
    """What a ``python …`` command line is actually going to run.

    Returns ``(source, label, from_stdin)``. The interpreter's own options are
    read the way CPython reads them — ``-X faulthandler script.py`` runs
    ``script.py``, ``-uBc code`` runs ``code`` — by the same Rust code the
    hook uses, so the two paths cannot disagree.
    """
    args = argv[1:]
    try:
        inv = engine().classify_argv(args)
    except Exception as exc:  # noqa: BLE001
        _debug(f"could not classify argv: {exc}")
        return "", "<unknown>", False
    if inv.get("code") is not None:
        return inv["code"], "python -c", False
    if inv.get("module") is not None:
        return "", f"python -m {inv['module']}", False
    if inv.get("script"):
        script = inv["script"]
        try:
            with open(script, encoding="utf-8", errors="replace") as fh:
                return fh.read(), os.path.basename(script), False
        except OSError:
            return "", script, False
    if inv.get("stdin"):
        try:
            if sys.stdin is None or sys.stdin.isatty():
                return "", "<repl>", False
        except (AttributeError, ValueError):
            return "", "<repl>", False
        return sys.stdin.read(), "<stdin>", True
    return "", "<unknown>", False


def _hand_over(argv: list[str]) -> int:
    """Run the real command in our place."""
    if os.name != "nt":
        try:
            os.execv(argv[0], argv)
        except OSError as exc:
            _debug(f"execv failed ({exc}); falling back to subprocess")
    return subprocess.call(argv)


def _run_wrapped(argv: list[str]) -> int:
    if os.environ.get("PYXRAY_OFF") == "1":
        return _hand_over(argv)
    source, name, from_stdin = _source_for(argv)
    if not source:
        return _hand_over(argv)
    event = look(source, name, source="shim")
    if not should_run(event):
        return 3
    if from_stdin:
        # The script came from stdin, which we have already consumed, so hand
        # it back rather than re-reading it.
        proc = subprocess.Popen(argv, stdin=subprocess.PIPE, text=True)
        proc.communicate(source)
        return proc.returncode
    return _hand_over(argv)


#: Tool names, across harnesses, whose arguments carry Python directly.
#: Compared case-insensitively — Claude Code says "Bash", Hermes says
#: "terminal", and a matcher that cares about the capital B silently ignores
#: every payload, which is the worst possible failure for an observer.
CODE_TOOLS = {"execute_code", "python", "run_python", "ipython", "code_interpreter",
              "python_tool", "jupyter", "notebookedit", "python_repl", "execute_python"}
#: Tool names whose arguments carry a shell command that might contain Python.
SHELL_TOOLS = {"exec_command", "bash", "terminal", "shell", "run_command", "execute_command", "sh",
               "bashtool", "run_terminal_cmd", "execute_bash", "shell_command",
               "run_shell_command", "exec", "execute"}
#: Tools that carry text that is not going to be run: file edits, searches.
#: Analysing a file an agent is *writing* would be a different product.
SKIP_TOOLS = {"write", "edit", "multiedit", "read", "glob", "grep", "ls", "webfetch",
              "websearch", "todowrite", "task", "agent", "askuserquestion", "str_replace",
              "create_file", "view", "search", "list_dir", "read_file", "write_file",
              "edit_file", "apply_patch"}
#: Argument keys to look in, in order of preference.
CODE_KEYS = ("code", "source", "python", "script")
SHELL_KEYS = ("command", "cmd", "commandLine", "args")

_PY_MARKERS = re.compile(
    r"^\s*(?:import\s+[\w.]+|from\s+[\w.]+\s+import\b|(?:async\s+)?def\s+\w+\s*\(|class\s+\w+"
    r"|(?:if|for|while|with|try|elif|except|else|finally)\b[^\n]*:\s*(?:#.*)?$"
    r"|[\w.\[\]]+\s*(?:[-+*/%|&^]|//|\*\*|<<|>>)?=(?!=)[^\n]*$|print\s*\(|return\b|raise\b"
    r"|@\w+|lambda\b|yield\b|await\b)",
    re.MULTILINE,
)


def _looks_like_python(text: str) -> bool:
    """Cheap guard for tools we could not identify.

    A tool we do not recognise might be handing us a shell command, a diff or a
    paragraph of prose in a field called `script`, and analysing that as Python
    would produce nonsense in the feed. This wants a real Python statement
    somewhere in the text — a bare `=` or a newline is not evidence of
    anything.
    """
    stripped = text.strip()
    if not stripped:
        return False
    return _PY_MARKERS.search(stripped) is not None


def python_in_payload(payload: dict[str, Any]) -> list[tuple[str, str]]:
    """Every piece of Python inside a tool-call payload, as ``(source, label)``.

    See :func:`snippets_in_payload`, which also says where each piece came
    from; this is the older shape, kept for callers that only want the code.
    """
    return [(s["source"], s["label"]) for s in snippets_in_payload(payload)]


def snippets_in_payload(payload: dict[str, Any]) -> list[dict[str, Any]]:
    """Every piece of Python inside a tool-call payload, whatever its shape.

    Handles both kinds of tool a harness might offer: one that takes Python
    directly, and one that takes a shell command with Python wrapped inside it
    — a script path (read relative to the payload's ``cwd``), a heredoc, a
    ``-c`` one-liner, a pipe, a ``bash -c``. Returns ``{source, label, path}``
    dicts in command order, ``path`` being the script file when there was
    one; an empty list means "nothing to look at".
    """
    tool = (payload.get("tool_name") or payload.get("tool") or "").strip()
    args = payload.get("tool_input") or payload.get("args") or payload.get("input") or {}
    if not isinstance(args, dict):
        return []

    key = tool.lower()
    if key in SKIP_TOOLS:
        return []
    takes_code = key in CODE_TOOLS
    takes_shell = key in SHELL_TOOLS
    # A tool we have never heard of gets both treatments rather than none:
    # being wrong about the shape costs a wasted parse, being silent costs the
    # whole point of the hook. The guard is `_looks_like_python`.
    unknown = not (takes_code or takes_shell)

    if key == "notebookedit":
        # Only code cells are code. Claude Code's field is `new_source`.
        if str(args.get("cell_type") or "code").lower() != "code":
            return []
        value = args.get("new_source") or args.get("source")
        if isinstance(value, str) and value.strip():
            return [{"source": value, "label": "notebook cell", "path": None}]
        return []

    if takes_code or unknown:
        for field in CODE_KEYS:
            value = args.get(field)
            if isinstance(value, str) and value.strip():
                if takes_code or _looks_like_python(value):
                    return [{"source": value, "label": tool or "code", "path": None}]

    if takes_shell or unknown:
        cwd = payload.get("cwd")
        for field in SHELL_KEYS:
            value = args.get(field)
            if isinstance(value, list):
                # An argv array (Codex sends ["bash", "-lc", "python3 …"]):
                # quote it back into one command, or the words inside the
                # -lc string fall apart.
                value = shlex.join(str(v) for v in value)
            if not (isinstance(value, str) and value.strip()):
                continue
            try:
                found = engine().extract_all(value, cwd=str(cwd) if cwd else None)
            except Exception as exc:  # noqa: BLE001
                _debug(f"could not extract from {field!r}: {exc}")
                found = []
            items = [
                {"source": f["source"], "label": f["label"], "path": f.get("path")}
                for f in found
                if f.get("source", "").strip()
            ]
            if items:
                return items
    return []


def _run_hook() -> int:
    """``--hook``: the Claude Code dialect of :mod:`pyxray_agent.hook`.

    Kept so hooks installed before the harness adapters existed keep working;
    it is exactly ``python -m pyxray_agent.hook claude pre``.
    """
    from .hook import main as hook_main

    # Rows from this entry point have always been labelled `hook`; the feed's
    # readers and people's filters expect it.
    os.environ.setdefault("PYXRAY_SOURCE", "hook")
    return hook_main(["claude", "pre"])


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if args and args[0] == "--hook":
        try:
            return _run_hook()
        except BaseException as exc:  # noqa: BLE001 — never fail the caller
            _debug(f"hook failed: {exc!r}")
            return 0
    if args and args[0] == "--":
        args = args[1:]
    if not args:
        print(__doc__, file=sys.stderr)
        return 2
    return _run_wrapped(args)


if __name__ == "__main__":
    raise SystemExit(main())
