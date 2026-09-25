"""The interceptor's contract: never breaks the caller, never draws into a
pipe, reads argv the way Python does, and finds Python in what an agent runs.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pyxray_agent as pyxray  # noqa: E402
from pyxray_agent import intercept  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PYTHON_DIR = ROOT / "python"


class _Env:
    """Set environment variables for the duration of a block."""

    def __init__(self, **values: str | None) -> None:
        self.values = values
        self.saved: dict[str, str | None] = {}

    def __enter__(self) -> None:
        for k, v in self.values.items():
            self.saved[k] = os.environ.get(k)
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def __exit__(self, *_: object) -> None:
        for k, v in self.saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


class _FakeStderr(io.StringIO):
    def __init__(self, tty: bool) -> None:
        super().__init__()
        self._tty = tty

    def isatty(self) -> bool:
        return self._tty


def test_draw_target_never_writes_into_a_pipe(monkeypatch=None):
    saved = sys.stderr
    try:
        for mode, tty, expect in [
            ("tty", False, None),
            ("tty", True, "stderr"),
            (None, False, None),
            ("never", True, None),
            ("always", False, "stderr"),
        ]:
            sys.stderr = _FakeStderr(tty)
            with _Env(PYXRAY_DRAW=mode, PYXRAY_TTY=None):
                got = intercept.draw_target()
            assert (got is sys.stderr) == (expect == "stderr"), (mode, tty, got)
            assert (got is None) == (expect is None), (mode, tty, got)
    finally:
        sys.stderr = saved
    with tempfile.TemporaryDirectory() as tmp:
        target = os.path.join(tmp, "pane")
        with _Env(PYXRAY_DRAW="tty", PYXRAY_TTY=target):
            stream = intercept.draw_target()
            assert stream is not None
            stream.write("x")
            stream.close()
        assert Path(target).read_text() == "x"
        with _Env(PYXRAY_DRAW="tty", PYXRAY_TTY=os.path.join(tmp, "no", "such", "dir")):
            assert intercept.draw_target() is None


def test_source_for_reads_argv_like_python():
    with tempfile.TemporaryDirectory() as tmp:
        script = Path(tmp) / "s.py"
        script.write_bytes(b"print('s')\n")
        py = sys.executable
        assert intercept._source_for([py, "-X", "faulthandler", str(script)])[:2] == (
            "print('s')\n", "s.py")
        assert intercept._source_for([py, "-W", "ignore", str(script)])[0] == "print('s')\n"
        assert intercept._source_for([py, "-W", "ignore", "-c", "print(1)"])[:2] == (
            "print(1)", "python -c")
        assert intercept._source_for([py, "-uBc", "print(2)"])[0] == "print(2)"
        assert intercept._source_for([py, "-m", "pytest"])[0] == ""
        missing = intercept._source_for([py, str(Path(tmp) / "missing.py")])
        assert missing[0] == ""


def test_payload_classification():
    with tempfile.TemporaryDirectory() as tmp:
        (Path(tmp) / "run.py").write_bytes(b"import os\nos.remove('x')\n")
        found = intercept.python_in_payload({
            "tool_name": "Bash",
            "cwd": tmp,
            "tool_input": {"command": "cd . && python3 run.py"},
        })
        assert found == [("import os\nos.remove('x')\n", "run.py")], found
    # Two pythons in one command: both come back.
    found = intercept.python_in_payload({
        "tool_name": "Bash",
        "tool_input": {"command": "python3 -c 'print(1)'; python3 -c 'print(2)'"},
    })
    assert [f[0] for f in found] == ["print(1)", "print(2)"]
    # Notebook cells: only code cells.
    assert intercept.python_in_payload({
        "tool_name": "NotebookEdit",
        "tool_input": {"cell_type": "markdown", "new_source": "# title"},
    }) == []
    assert intercept.python_in_payload({
        "tool_name": "NotebookEdit",
        "tool_input": {"cell_type": "code", "new_source": "import os"},
    }) == [("import os", "notebook cell")]
    # Unknown tools need real Python, not just an equals sign.
    assert intercept.python_in_payload({
        "tool_name": "whatever", "input": {"code": "x = 1\n"},
    }) == [("x = 1\n", "whatever")]
    assert intercept.python_in_payload({
        "tool_name": "whatever", "input": {"code": "name = value"},
    }) == [("name = value", "whatever")]
    assert intercept.python_in_payload({
        "tool_name": "whatever", "input": {"code": "just some prose, no code here"},
    }) == []


def _hook(payload: dict, **env: str) -> subprocess.CompletedProcess:
    full = {**os.environ, "PYTHONPATH": str(PYTHON_DIR), "PYXRAY_DRAW": "never", **env}
    return subprocess.run(
        [sys.executable, "-m", "pyxray_agent.intercept", "--hook"],
        input=json.dumps(payload), capture_output=True, text=True, env=full,
    )


def test_hook_output_shape_when_gated():
    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "feed.jsonl")
        proc = _hook(
            {"tool_name": "Bash", "session_id": "abcdef123",
             "tool_input": {"command": "python3 -c \"import shutil; shutil.rmtree('/x')\""}},
            PYXRAY_GATE="10", PYXRAY_LOG=log,
        )
        assert proc.returncode == 0, proc.stderr
        out = json.loads(proc.stdout)
        assert out["decision"] == "block"
        assert out["hookSpecificOutput"]["hookEventName"] == "PreToolUse"
        assert out["hookSpecificOutput"]["permissionDecision"] == "ask"
        assert "risk" in out["reason"]
        rows = pyxray.feed(log)
        assert len(rows) == 1 and rows[0]["source"] == "hook:abcdef12"


def test_hook_is_silent_when_not_gated_and_on_garbage():
    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "feed.jsonl")
        proc = _hook({"tool_name": "Bash", "tool_input": {"command": "python3 -c 'print(1)'"}},
                     PYXRAY_GATE="off", PYXRAY_LOG=log)
        assert proc.returncode == 0 and proc.stdout == "", (proc.stdout, proc.stderr)
        assert len(pyxray.feed(log)) == 1
    full = {**os.environ, "PYTHONPATH": str(PYTHON_DIR)}
    proc = subprocess.run([sys.executable, "-m", "pyxray_agent.intercept", "--hook"],
                          input="not json", capture_output=True, text=True, env=full)
    assert proc.returncode == 0 and proc.stdout == ""
    proc = subprocess.run([sys.executable, "-m", "pyxray_agent.intercept", "--hook"],
                          input="[1, 2, 3]", capture_output=True, text=True, env=full)
    assert proc.returncode == 0 and proc.stdout == ""


# A "binary" that exists but is not pyx: every engine call fails loudly.
BROKEN = {"PYXRAY_BIN": sys.executable, "PYXRAY_FORCE_BINARY": "1"}


def test_hook_with_a_broken_engine_still_exits_zero():
    proc = _hook(
        {"tool_name": "Bash", "tool_input": {"command": "python3 -c 'import os'"}},
        PYXRAY_GATE="10", **BROKEN,
    )
    assert proc.returncode == 0, proc.stderr
    assert proc.stdout == ""


def test_wrapper_with_a_broken_engine_still_runs_the_command():
    env = {**os.environ, "PYTHONPATH": str(PYTHON_DIR), "PYXRAY_DRAW": "never", **BROKEN}
    proc = subprocess.run(
        [sys.executable, "-m", "pyxray_agent.intercept", "--", sys.executable, "-c", "print('ran')"],
        capture_output=True, text=True, env=env,
    )
    assert proc.returncode == 0, proc.stderr
    assert "ran" in proc.stdout


def test_wrapper_replays_stdin_scripts():
    env = {**os.environ, "PYTHONPATH": str(PYTHON_DIR), "PYXRAY_DRAW": "never",
           "PYXRAY_GATE": "off"}
    proc = subprocess.run(
        [sys.executable, "-m", "pyxray_agent.intercept", "--", sys.executable, "-"],
        input="print('from stdin')\n", capture_output=True, text=True, env=env,
    )
    assert proc.returncode == 0, proc.stderr
    assert "from stdin" in proc.stdout


def test_look_never_raises():
    with _Env(PYXRAY_DRAW="never", **BROKEN):
        # A fresh interpreter is needed for the engine cache to be empty.
        code = ("from pyxray_agent.intercept import look\n"
                "e = look('import os', 'x')\n"
                "print(e['band'], 'error' in e)\n")
        proc = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                              env={**os.environ, "PYTHONPATH": str(PYTHON_DIR)})
        assert proc.returncode == 0, proc.stderr
        assert proc.stdout.strip() == "inert True"


def test_min_lines_and_off_short_circuit():
    with _Env(PYXRAY_OFF="1"):
        assert intercept.look("import os; os.system(1)")["band"] == "inert"
    with _Env(PYXRAY_OFF=None, PYXRAY_MIN_LINES="5", PYXRAY_DRAW="never"):
        assert intercept.look("import os; os.system(1)")["band"] == "inert"


if __name__ == "__main__":
    failures = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok    {name}")
            except Exception as exc:  # noqa: BLE001
                failures += 1
                print(f"FAIL  {name}: {exc!r}")
    raise SystemExit(1 if failures else 0)
