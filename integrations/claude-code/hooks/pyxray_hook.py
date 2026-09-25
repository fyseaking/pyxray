"""The hook, in Python: find pyxray, run the interceptor, never fail.

Exit 75 (EX_TEMPFAIL) tells the launcher "this interpreter cannot see pyxray,
try another"; every other outcome is exit 0. Stdin is the harness payload.
"""

import os
import sys


def _add_checkout(root):
    package = os.path.join(root, "python", "pyxray_agent", "__init__.py")
    if not os.path.isfile(package):
        return False
    sys.path.insert(0, os.path.join(root, "python"))
    if not os.environ.get("PYXRAY_BIN"):
        exe = "pyx.exe" if os.name == "nt" else "pyx"
        for profile in ("release", "debug"):
            candidate = os.path.join(root, "target", profile, exe)
            if os.path.isfile(candidate):
                os.environ["PYXRAY_BIN"] = candidate
                break
    return True


def main():
    explicit = os.environ.get("PYXRAY_ROOT")
    if explicit:
        _add_checkout(explicit)
    else:
        here = os.path.dirname(os.path.abspath(__file__))
        # hooks/ -> claude-code/ -> integrations/ -> the checkout
        _add_checkout(os.path.normpath(os.path.join(here, "..", "..", "..")))
    try:
        from pyxray_agent.intercept import main as intercept
    except ImportError:
        return 75
    try:
        return intercept(["--hook"])
    except BaseException:  # noqa: BLE001 — never fail the tool call
        return 0


if __name__ == "__main__":
    sys.exit(main())
