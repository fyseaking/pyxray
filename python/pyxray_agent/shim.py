"""``pyxray-agent shim install|uninstall|where`` — put pyxray in front of ``python``.

Copies the ``pyx-shim`` executable into a directory as ``python`` and
``python3`` (``python.exe``/``python3.exe`` on Windows). Anything that then
starts Python with that directory first on its ``PATH`` — an agent's
``CreateProcess("python")``, a Makefile, a ``subprocess.run`` deep in a
library — runs through pyxray first: the code is analysed, recorded for
``pyx watch``, checked against the gate and policy, and handed to the real
interpreter.

The directory is not added to your PATH for you. Put it in front for the
agent's process only — the shim is for watching agents, not for every
Python you run — with the snippet this prints.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
from pathlib import Path

EXE = ".exe" if os.name == "nt" else ""
NAMES = ("python", "python3")


def default_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
        return Path(base) / "pyxray" / "shim"
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "pyxray" / "shim"


def find_shim() -> Path | None:
    """The built shim: ``PYXRAY_SHIM``, the one bundled in the wheel, or a
    checkout's ``target/`` build."""
    if explicit := os.environ.get("PYXRAY_SHIM"):
        return Path(explicit) if Path(explicit).is_file() else None
    here = Path(__file__).resolve().parent
    candidates = [here / "bin" / f"pyx-shim{EXE}"]
    root = here.parent.parent
    candidates += [root / "target" / p / f"pyx-shim{EXE}" for p in ("release", "debug")]
    return next((c for c in candidates if c.is_file()), None)


def snippet(directory: Path) -> str:
    d = str(directory)
    return "\n".join([
        "Start the agent with the shim first on its PATH:",
        f'  PowerShell   $env:PATH = "{d};" + $env:PATH',
        f"  cmd          set PATH={d};%PATH%",
        f'  sh / bash    export PATH="{d}:$PATH"',
        "Then PYXRAY_GATE=40 (or a .pyxray.toml policy) to have it stop anything risky;",
        "without one it only records. `pyx watch` shows what went past.",
    ])


def install(directory: Path) -> int:
    shim = find_shim()
    if shim is None:
        print("pyxray: no pyx-shim found — install a release wheel, or `cargo build "
              "--release -p pyx-shim` in a checkout (or set PYXRAY_SHIM)", file=sys.stderr)
        return 1
    directory.mkdir(parents=True, exist_ok=True)
    for name in NAMES:
        target = directory / f"{name}{EXE}"
        shutil.copy2(shim, target)
        if os.name != "nt":
            target.chmod(0o755)
    print(f"pyxray: installed {', '.join(n + EXE for n in NAMES)} in {directory}")
    print(snippet(directory))
    return 0


def uninstall(directory: Path) -> int:
    removed = 0
    for name in NAMES:
        target = directory / f"{name}{EXE}"
        if target.is_file():
            target.unlink()
            removed += 1
    print(f"pyxray: removed {removed} shim(s) from {directory}")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="pyxray-agent shim", description=__doc__.split("\n\n")[0])
    ap.add_argument("action", choices=["install", "uninstall", "where"])
    ap.add_argument("--dir", type=Path, default=None, help=f"default: {default_dir()}")
    a = ap.parse_args(argv)
    directory = a.dir or default_dir()
    if a.action == "install":
        return install(directory)
    if a.action == "uninstall":
        return uninstall(directory)
    shim = find_shim()
    print(f"shim binary: {shim or 'not found'}\ninstall dir: {directory}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
