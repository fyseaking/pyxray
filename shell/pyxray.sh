# pyxray shell integration — source this from ~/.bashrc or ~/.zshrc.
#
#   source /path/to/pyxray/shell/pyxray.sh
#
# This wraps python3 for *interactive shells you type into*. It does NOT catch
# an agent's commands: harnesses spawn non-interactive `sh -c`, which never
# reads your rc file, and many use /bin/sh rather than bash at all. For those,
# use the PATH shim (integrations/shim) or the harness's own hook — see
# integrations/README.md.
#
#   export PYXRAY_DRAW=never     draw nothing; follow it with `pyx watch`
#   export PYXRAY_TTY=/dev/pts/7 draw to a dedicated pane instead
#   export PYXRAY_LAYOUT=auto    one row when dull, the card when not
#   export PYXRAY_GATE=off       the default; a number refuses above it
#   export PYXRAY_OFF=1          turn it off for this shell

_PYXRAY_ROOT="${_PYXRAY_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-${(%):-%x}}")/.." && pwd)}"
export PYTHONPATH="${_PYXRAY_ROOT}/python${PYTHONPATH:+:$PYTHONPATH}"
export PYXRAY_BIN="${PYXRAY_BIN:-${_PYXRAY_ROOT}/target/release/pyx}"
[ -x "$PYXRAY_BIN" ] || export PYXRAY_BIN="${_PYXRAY_ROOT}/target/debug/pyx"

# The real interpreter, resolved once so the wrapper cannot recurse into itself.
_PYXRAY_PYTHON="${_PYXRAY_PYTHON:-$(command -v python3)}"

python3() {
  if [ "${PYXRAY_OFF:-0}" = "1" ] || [ $# -eq 0 ]; then
    command "$_PYXRAY_PYTHON" "$@"
  else
    command "$_PYXRAY_PYTHON" -m pyxray_agent.intercept -- "$_PYXRAY_PYTHON" "$@"
  fi
}

pyx() { command "$PYXRAY_BIN" "$@"; }

# Open the live feed in this terminal.
pyxwatch() { command "$PYXRAY_BIN" watch "$@"; }

# Pipe a whole shell command in and see the Python inside it:
#   pyxcmd "python3 <<'EOF' ... EOF"
pyxcmd() { printf '%s' "$*" | command "$PYXRAY_BIN" --stdin-is-command; }
