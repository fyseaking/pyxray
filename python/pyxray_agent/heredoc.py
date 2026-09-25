"""Deprecated: the pure-Python extractor has been replaced by the engine's.

There used to be a second implementation of "find the Python in this shell
command" here, kept roughly in step with the Rust one by hand. It recognised
one spelling — a command *starting* with ``python3`` and carrying a heredoc or
``-c`` — and silently missed the rest (``python3 script.py``, ``cd x &&
python3``, ``env python3``, ``bash -c``, pipes). One implementation is easier
to keep honest than two, so this module now forwards to the engine; see
``pyxray.extract`` and ``pyxray.extract_all``.
"""

from __future__ import annotations

from ._native import engine


def extract_python(command: str) -> tuple[str, str]:
    """Return ``(source, label)`` for a command, or the input unchanged."""
    return engine().extract(command)
