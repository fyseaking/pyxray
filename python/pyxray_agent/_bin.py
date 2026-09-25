"""The ``pyx`` console script: hand over to the native binary.

Wheels built by the release workflow carry ``pyxray_agent/bin/pyx`` (or ``pyx.exe``)
for their platform, so ``pip install pyxray-agent`` gives you the CLI as well as
the package. A wheel built locally with ``maturin develop`` has no binary in
it; this then looks for one on ``PATH`` or in a checkout, and says how to
get one if there is none.
"""

from __future__ import annotations

import os
import subprocess
import sys

from ._native import _find_binary


def main() -> int:
    exe = _find_binary()
    if not exe:
        print(
            "pyx: no native binary found. This wheel was built without one; "
            "install a release wheel from PyPI, `cargo install pyx-cli`, or set "
            "PYXRAY_BIN to a built pyx.",
            file=sys.stderr,
        )
        return 2
    argv = [exe, *sys.argv[1:]]
    if os.name != "nt":
        try:
            os.execv(exe, argv)
        except OSError:
            pass
    return subprocess.call(argv)


if __name__ == "__main__":
    raise SystemExit(main())
