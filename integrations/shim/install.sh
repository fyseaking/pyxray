#!/bin/sh
# Put the pyxray shim on PATH for this shell, or print what to add to your rc.
#
#   . integrations/shim/install.sh            # this shell only
#   integrations/shim/install.sh --print      # show the lines to keep
#   integrations/shim/install.sh --uninstall  # remove the python symlink
#
# The shim catches `python3` and `python`. It does not catch `python3.12`,
# `uv run`, `poetry run` or a virtualenv's own interpreter — those resolve to
# other files — which is why the harness hook (integrations/README.md) is the
# route that sees everything an agent runs.

here=$(cd -- "$(dirname -- "$0")" && pwd)
root=$(cd -- "$here/../.." && pwd)

bin=""
for candidate in "$root/target/release/pyx" "$root/target/debug/pyx"; do
    if [ -x "$candidate" ]; then bin="$candidate"; break; fi
done
[ -n "$bin" ] || bin=$(command -v pyx 2>/dev/null || true)

if [ "$1" = "--uninstall" ]; then
    rm -f "$here/python"
    echo "removed $here/python; drop $here from PATH to finish."
    exit 0
fi

# `python` is the same script under another name; the script reads $0.
if [ ! -e "$here/python" ]; then
    ln -s python3 "$here/python" 2>/dev/null || cp "$here/python3" "$here/python"
fi

if [ "$1" = "--print" ]; then
    cat <<PRINT
# pyxray — see what Python does before it runs
export PATH="$here:\$PATH"
export PYTHONPATH="$root/python\${PYTHONPATH:+:\$PYTHONPATH}"
PRINT
    [ -n "$bin" ] && echo "export PYXRAY_BIN=\"$bin\""
    cat <<'PRINT'
# Optional:
#   export PYXRAY_DRAW=never     # feed only; watch it with: pyx watch
#   export PYXRAY_TTY=/dev/pts/7 # or draw to a dedicated pane
#   export PYXRAY_GATE=off       # the default; a number refuses above it
PRINT
    exit 0
fi

PATH="$here:$PATH"
PYTHONPATH="$root/python${PYTHONPATH:+:$PYTHONPATH}"
export PATH PYTHONPATH
if [ -n "$bin" ]; then
    PYXRAY_BIN="$bin"
    export PYXRAY_BIN
fi

echo "pyxray shim active for this shell (python3 and python)."
if [ -n "$bin" ]; then
    echo "  feed: $("$bin" --list 2>/dev/null | tail -1 | tr -d ' ')"
    echo "  watch it with: $bin watch"
else
    echo "  no pyx binary found yet: cargo build --release, or pip install pyxray-agent"
fi
echo "  make it permanent: $here/install.sh --print >> ~/.bashrc"
