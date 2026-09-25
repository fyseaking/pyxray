"""Locate the analysis engine.

Prefers the compiled extension that ships beside this file. Falls back to the
``pyx`` binary — on ``PATH``, at ``$PYXRAY_BIN``, bundled into the wheel under
``pyxray_agent/bin/``, or in a checkout's ``target/`` — so a clone with a built CLI
but no built extension still works, which is the common case when someone has
just run ``cargo build``.

Set ``PYXRAY_FORCE_BINARY=1`` to skip the extension even when it is present;
the test-suite uses that to exercise the fallback path.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

__all__ = ["Engine", "engine", "EngineError"]

_HERE = Path(__file__).resolve().parent
_ROOT = _HERE.parent.parent
_EXE = "pyx.exe" if os.name == "nt" else "pyx"


class EngineError(RuntimeError):
    """No usable pyxray engine could be found."""


class Engine:
    """Whichever of the two backends is available, behind one interface."""

    kind: str

    def analyze(self, source: str, name: str = "<stdin>") -> dict[str, Any]:
        raise NotImplementedError

    def render(self, source: str, **kw: Any) -> str:
        raise NotImplementedError

    def extract(self, command: str) -> tuple[str, str]:
        raise NotImplementedError

    def extract_all(
        self, command: str, cwd: str | None = None, read_files: bool = True
    ) -> list[dict[str, Any]]:
        """Every piece of Python in a shell command; see ``pyx extract``."""
        raise NotImplementedError

    def classify_argv(self, args: list[str]) -> dict[str, Any]:
        """``{code, module, script, stdin}`` for an interpreter's own argv."""
        raise NotImplementedError

    def themes(self) -> list[tuple[str, str, str]]:
        raise NotImplementedError

    def layouts(self) -> list[tuple[str, str]]:
        raise NotImplementedError

    def look(self, code: str, **kw: Any) -> tuple[dict[str, Any], str]:
        """Analyse once: append the feed event, render only if asked.

        Keywords: ``name``, ``source``, ``path`` (the log), ``draw`` and the
        render options, plus ``record=False`` to analyse without appending,
        ``origin`` (the script's path) and ``run`` (an execution id).
        """
        raise NotImplementedError

    def feed_path(self) -> str:
        raise NotImplementedError

    def read_feed(self, path: str | None = None) -> list[dict[str, Any]]:
        raise NotImplementedError


class _Extension(Engine):
    kind = "extension"

    def __init__(self, mod: Any) -> None:
        self._m = mod

    def analyze(self, source: str, name: str = "<stdin>") -> dict[str, Any]:
        return json.loads(self._m.analyze_json(source, name))

    def render(self, source: str, **kw: Any) -> str:
        return self._m.render(source, **kw)

    def extract(self, command: str) -> tuple[str, str]:
        return tuple(self._m.extract(command))  # type: ignore[return-value]

    def extract_all(
        self, command: str, cwd: str | None = None, read_files: bool = True
    ) -> list[dict[str, Any]]:
        return json.loads(self._m.extract_all(command, cwd, read_files))

    def classify_argv(self, args: list[str]) -> dict[str, Any]:
        return json.loads(self._m.classify_argv(list(args)))

    def themes(self) -> list[tuple[str, str, str]]:
        return self._m.themes()

    def layouts(self) -> list[tuple[str, str]]:
        return self._m.layouts()

    def look(self, code: str, **kw: Any) -> tuple[dict[str, Any], str]:
        event, rendered = self._m.look(code, **kw)
        return json.loads(event), rendered

    def feed_path(self) -> str:
        return self._m.feed_path()

    def read_feed(self, path: str | None = None) -> list[dict[str, Any]]:
        return json.loads(self._m.read_feed(path))


class _Binary(Engine):
    kind = "binary"

    def __init__(self, exe: str) -> None:
        self._exe = exe
        self._listing: dict[str, Any] | None = None

    def _run(self, args: list[str], source: str | None = None) -> str:
        data = source.encode("utf-8") if source is not None else None
        proc = subprocess.run(
            [self._exe, *args],
            input=data,
            capture_output=True,
        )
        if proc.returncode not in (0, 3):
            err = proc.stderr.decode("utf-8", errors="replace").strip()
            raise EngineError(err or f"pyx exited {proc.returncode}")
        return proc.stdout.decode("utf-8", errors="replace")

    def _list(self) -> dict[str, Any]:
        if self._listing is None:
            try:
                self._listing = json.loads(self._run(["--list", "--format", "json"]))
            except (EngineError, json.JSONDecodeError):
                self._listing = {}
        return self._listing

    def analyze(self, source: str, name: str = "<stdin>") -> dict[str, Any]:
        return json.loads(self._run(["--format", "json", "--name", name], source))

    def render(self, source: str, **kw: Any) -> str:
        fmt = str(kw.get("format", "ansi"))
        args = ["--format", fmt, "--name", str(kw.get("name", "<stdin>"))]
        if fmt == "ansi":
            # The binary would otherwise, sensibly, strip colour from a pipe.
            args += ["--color", "always"]
        for flag, key in (
            ("--theme", "theme"),
            ("--layout", "layout"),
            ("--width", "width"),
            ("--height", "height"),
            ("--icons", "icons"),
            ("--depth", "depth"),
        ):
            value = kw.get(key)
            if value is not None:
                args += [flag, str(value)]
        if kw.get("code"):
            args.append("--code")
        if kw.get("gutter") is False:
            args.append("--no-gutter")
        return self._run(args, source)

    def extract(self, command: str) -> tuple[str, str]:
        found = self.extract_all(command)
        for item in found:
            if item.get("source", "").strip():
                return item["source"], item["label"]
        return command, "<stdin>"

    def extract_all(
        self, command: str, cwd: str | None = None, read_files: bool = True
    ) -> list[dict[str, Any]]:
        args = ["extract"]
        if cwd:
            args += ["--cwd", cwd]
        if not read_files:
            args.append("--no-files")
        return json.loads(self._run(args, command))

    def classify_argv(self, args: list[str]) -> dict[str, Any]:
        return json.loads(self._run(["extract", "--argv", "--", *args]))

    def look(self, code: str, **kw: Any) -> tuple[dict[str, Any], str]:
        # One invocation does everything: --emit appends the event,
        # --print-event hands it back as the first line, and the render — if
        # asked for — follows.
        args = ["--print-event", "--source", str(kw.get("source", "shim")),
                "--name", str(kw.get("name", "<stdin>"))]
        if kw.get("record", True):
            args.append("--emit")
            if path := kw.get("path"):
                args += ["--feed", str(path)]
        if origin := kw.get("origin"):
            args += ["--origin", str(origin)]
        if run := kw.get("run"):
            args += ["--run", str(run)]
        if kw.get("draw"):
            args += ["--format", str(kw.get("format", "ansi")),
                     "--color", "always",
                     "--layout", str(kw.get("layout", "auto")),
                     "--theme", str(kw.get("theme", "blueprint")),
                     "--icons", str(kw.get("icons", "both")),
                     "--width", str(kw.get("width", 100))]
        else:
            args.append("--quiet")
        out = self._run(args, code)
        first, _, rest = out.partition("\n")
        try:
            event = json.loads(first)
        except json.JSONDecodeError:
            event = {"risk": 0, "band": "inert", "synopsis": "", "notes": [], "blocked": False}
            rest = out
        return event, rest if kw.get("draw") else ""

    def feed_path(self) -> str:
        return str(self._list().get("feed", ""))

    def read_feed(self, path: str | None = None) -> list[dict[str, Any]]:
        target = path or os.environ.get("PYXRAY_LOG") or self.feed_path()
        rows: list[dict[str, Any]] = []
        try:
            with open(target, encoding="utf-8") as fh:
                for line in fh:
                    try:
                        rows.append(json.loads(line))
                    except json.JSONDecodeError:
                        continue
        except OSError:
            pass
        return rows

    def themes(self) -> list[tuple[str, str, str]]:
        return [(t["id"], t["name"], t["blurb"]) for t in self._list().get("themes", [])]

    def layouts(self) -> list[tuple[str, str]]:
        return [(l["id"], l["blurb"]) for l in self._list().get("layouts", [])]


def _find_binary() -> str | None:
    if override := os.environ.get("PYXRAY_BIN"):
        if Path(override).is_file():
            return override
        if (
            os.name == "nt"
            and override.startswith("/")
            and len(override) > 2
            and override[2] == "/"
            and override[1].isalpha()
        ):
            drive_path = Path(f"{override[1]}:{override[2:]}")
            if drive_path.is_file():
                return str(drive_path)
        # A stale override should not disable the fallback entirely.
        print(f"pyxray: PYXRAY_BIN={override!r} does not exist; looking elsewhere",
              file=sys.stderr)
    found = shutil.which("pyx")
    if found:
        return found
    candidates = [_HERE / "bin" / _EXE]
    for profile in ("release", "debug"):
        candidates.append(_ROOT / "target" / profile / _EXE)
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    return None


_cached: Engine | None = None


def engine() -> Engine:
    """The best available backend, resolved once."""
    global _cached
    if _cached is not None:
        return _cached
    if os.environ.get("PYXRAY_FORCE_BINARY") != "1":
        try:
            from . import _pyxray  # type: ignore[attr-defined]

            _cached = _Extension(_pyxray)
            return _cached
        except ImportError:
            pass
    exe = _find_binary()
    if exe:
        _cached = _Binary(exe)
        return _cached
    raise EngineError(
        "no pyxray engine found — pip install pyxray-agent, or build one with "
        "`cargo build --release` (then `make install-ext`), or set PYXRAY_BIN "
        "to a pyx binary"
    )
