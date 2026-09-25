"""pyxray — see what a Python snippet will do before you run it.

    >>> import pyxray_agent as pyxray
    >>> report = pyxray.analyze("import os; os.remove('x')")
    >>> report["meta"]["synopsis"]
    'DELETES x'
    >>> print(pyxray.render(open("script.py").read(), theme="carbon"))

The heavy lifting is Rust; this package is the ergonomic surface and the
interception glue. See :mod:`pyxray_agent.intercept` for the part that sits in front
of a command a model just handed you.
"""

from __future__ import annotations

from typing import Any

from ._native import Engine, EngineError, engine

__all__ = [
    "analyze",
    "render",
    "extract",
    "extract_all",
    "classify_argv",
    "look",
    "feed",
    "feed_path",
    "themes",
    "layouts",
    "backend",
    "Engine",
    "EngineError",
]

__version__ = "0.1.0"


def analyze(source: str, name: str = "<stdin>") -> dict[str, Any]:
    """Analyse a snippet and return the full report as a dict.

    Never raises on bad syntax: an unparseable snippet still yields a report,
    with the errors under ``diagnostics`` and ``meta.parsed`` set to False.
    """
    return engine().analyze(source, name)


def render(
    source: str,
    *,
    name: str = "<stdin>",
    theme: str = "blueprint",
    layout: str = "card",
    format: str = "ansi",
    width: int = 100,
    height: int | None = None,
    icons: str = "both",
    depth: int = 6,
    gutter: bool = True,
    code: bool = False,
) -> str:
    """Render a snippet to ANSI, plain text, HTML or SVG."""
    return engine().render(
        source,
        name=name,
        theme=theme,
        layout=layout,
        format=format,
        width=width,
        height=height,
        icons=icons,
        depth=depth,
        gutter=gutter,
        code=code,
    )


def extract(command: str) -> tuple[str, str]:
    """The first piece of Python in a shell command, as ``(source, label)``.

    Returns the command itself, labelled ``<stdin>``, when it does not look
    like a wrapped snippet — on the assumption that it was Python already.
    """
    return engine().extract(command)


def extract_all(
    command: str, cwd: str | None = None, read_files: bool = True
) -> list[dict[str, Any]]:
    """Every piece of Python a shell command would run, in command order.

    Each item is ``{"source", "label", "path", "segment"}``. Script paths are
    read relative to ``cwd`` (the harness usually knows it); ``read_files=False``
    reports them without reading. Heredocs, ``-c``, ``bash -c``, ``cat f | py``,
    ``cd x && python3 …`` and the usual wrappers are all understood.
    """
    return engine().extract_all(command, cwd=cwd, read_files=read_files)


def classify_argv(args: list[str]) -> dict[str, Any]:
    """Read an interpreter's own argv: ``{"code", "module", "script", "stdin"}``."""
    return engine().classify_argv(list(args))


def look(
    source: str,
    *,
    name: str = "<stdin>",
    origin: str = "api",
    path: str | None = None,
    draw: bool = False,
    **kw: Any,
) -> tuple[dict[str, Any], str]:
    """Analyse once, append a feed event, and render only if ``draw``.

    The interceptor's hot path, exposed because it is also the cheapest way to
    ask "what is this and should I care" from your own code.
    """
    return engine().look(source, name=name, source=origin, path=path, draw=draw, **kw)


def feed(path: str | None = None) -> list[dict[str, Any]]:
    """Every event in the feed log, oldest first."""
    return engine().read_feed(path)


def feed_path() -> str:
    """Where the feed log lives."""
    return engine().feed_path()


def themes() -> list[tuple[str, str, str]]:
    """Available themes as ``(id, name, blurb)``."""
    return engine().themes()


def layouts() -> list[tuple[str, str]]:
    """Available layouts as ``(id, blurb)``."""
    return engine().layouts()


def backend() -> str:
    """Which engine is in use: ``"extension"`` or ``"binary"``."""
    return engine().kind
