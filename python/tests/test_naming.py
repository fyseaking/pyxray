"""The Python package is ``pyxray_agent``, published as ``pyxray-agent``.

``pyxray`` on PyPI is an unrelated X-ray physics library, so a stray
``pip install pyxray`` or ``import pyxray`` installs or imports the wrong
thing and fails in confusing ways. This walks the checkout and refuses the
old spellings anywhere they would reach a user or an interpreter.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

OLD = re.compile(
    r"(?<![\w-])(?:import pyxray|from pyxray import|-m pyxray|pip install pyxray|python/pyxray)(?![\w-])"
)
SKIP_DIRS = {".git", "target", "build", "dist", "node_modules", "__pycache__", ".pytest_cache"}
SKIP_SUFFIXES = {".snap", ".png", ".svg", ".html", ".whl", ".so", ".pyd", ".dll", ".lock"}


def _files():
    for path in ROOT.rglob("*"):
        if not path.is_file() or any(part in SKIP_DIRS for part in path.relative_to(ROOT).parts):
            continue
        if path.suffix in SKIP_SUFFIXES or path == Path(__file__).resolve():
            continue
        yield path


def test_no_old_package_name_anywhere():
    offenders = []
    for path in _files():
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for number, line in enumerate(text.splitlines(), 1):
            if OLD.search(line):
                offenders.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()}")
    assert not offenders, "old package name still in use:\n" + "\n".join(offenders)


def test_distribution_and_module_names():
    text = (ROOT / "pyproject.toml").read_text(encoding="utf-8")
    assert re.search(r'^name = "pyxray-agent"$', text, re.M)
    assert 'module-name = "pyxray_agent._pyxray"' in text
    assert (ROOT / "python" / "pyxray_agent" / "__init__.py").is_file()
    assert not (ROOT / "python" / "pyxray").exists()
