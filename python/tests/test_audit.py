"""The runtime audit hook: what a run actually did, next to what was predicted."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AUDIT = ROOT / "python" / "pyxray_agent" / "_audit"

CHILD = r"""
import os, subprocess, socket, sys, pickle, json
open("made.txt", "w").write("x")
open("made.txt").read()
os.rename("made.txt", "moved.txt")
os.remove("moved.txt")
subprocess.run([sys.executable, "-c", "pass"])
try:
    socket.getaddrinfo("example.invalid", 443)
except OSError:
    pass
eval("1 + 1")
pickle.loads(pickle.dumps(json.JSONDecoder))
import email.utils  # an import: must not count as a file read or an exec
"""


def run_child(tmp: Path, code: str, *, extra_path: list[str] | None = None, run: str = "r-test",
              **env: str) -> tuple[subprocess.CompletedProcess, list[dict]]:
    log = tmp / "feed.jsonl"
    paths = [str(AUDIT), *(extra_path or [])]
    full = {
        **os.environ,
        "PYTHONPATH": os.pathsep.join(paths),
        "PYXRAY_RUN": run,
        "PYXRAY_AUDIT_LOG": str(log),
        **env,
    }
    proc = subprocess.run([sys.executable, "-c", code], cwd=tmp, env=full,
                          capture_output=True, text=True, timeout=60)
    rows = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    return proc, rows


def items(row: dict) -> set[tuple[str, str]]:
    return {(i["effect"], i["verb"]) for i in row["items"]}


def test_one_observation_with_what_the_run_did(tmp_path):
    proc, rows = run_child(tmp_path, CHILD)
    assert proc.returncode == 0, proc.stderr
    mine = [r for r in rows if r["pid"] != 0]
    parent = [r for r in mine if r["run"] == "r-test"]
    assert parent, rows
    seen = set().union(*(items(r) for r in parent))
    for expected in [
        ("fs_write", "write"), ("fs_read", "read"), ("fs_write", "rename"),
        ("fs_delete", "remove"), ("process", "spawn"), ("net", "resolve"),
        ("dynamic", "exec"), ("dynamic", "unpickle"),
    ]:
        assert expected in seen, (expected, seen)
    targets = {i.get("target") for r in parent for i in r["items"]}
    assert "made.txt" in targets
    assert "example.invalid" in targets
    assert not any(str(t).endswith(".py") for t in targets if t), "imports are not reads"
    assert all(r["kind"] == "observed" for r in rows)


def test_nested_pythons_share_the_run(tmp_path):
    _, rows = run_child(tmp_path, "import subprocess, sys; subprocess.run([sys.executable, '-c', 'open(\"x\", \"w\")'])")
    runs = {r["run"] for r in rows}
    pids = {r["pid"] for r in rows}
    assert runs == {"r-test"} and len(pids) == 2, rows


def test_a_users_own_sitecustomize_still_runs(tmp_path):
    theirs = tmp_path / "theirs"
    theirs.mkdir()
    (theirs / "sitecustomize.py").write_text("import builtins\nbuiltins.THEIRS_RAN = True\n")
    proc, rows = run_child(tmp_path, "import builtins; print(getattr(builtins, 'THEIRS_RAN', False))",
                           extra_path=[str(theirs)])
    assert proc.stdout.strip() == "True", proc.stderr
    assert rows, "and ours ran too"


def test_the_hook_never_breaks_the_program(tmp_path):
    code = (
        "import os\n"
        "fd = os.open('f', os.O_WRONLY | os.O_CREAT)\n"
        "os.write(fd, b'x'); os.close(fd)\n"
        "open(3 if False else 'f', 'rb').read()\n"
        "try:\n    open(b'\\xff\\xfe-undecodable', 'w')\nexcept (OSError, ValueError):\n    pass\n"
        "print('fine')\n"
    )
    proc, rows = run_child(tmp_path, code)
    assert proc.returncode == 0 and proc.stdout.strip() == "fine", proc.stderr
    assert rows


def test_without_a_run_id_it_stays_out_of_the_way(tmp_path):
    log = tmp_path / "feed.jsonl"
    full = {**os.environ, "PYTHONPATH": str(AUDIT), "PYXRAY_AUDIT_LOG": str(log)}
    full.pop("PYXRAY_RUN", None)
    proc = subprocess.run([sys.executable, "-c", "open('x', 'w')"], cwd=tmp_path, env=full,
                          capture_output=True, text=True, timeout=60)
    assert proc.returncode == 0
    assert not log.exists()


def test_the_wrapper_audits_when_asked(tmp_path):
    """PYXRAY_AUDIT=1 through the Python wrapper: a predicted event and an
    observation, joined by the run id."""
    log = tmp_path / "feed.jsonl"
    script = tmp_path / "job.py"
    script.write_bytes(b"import os\nopen('out.txt', 'w').write('x')\nos.remove('out.txt')\n")
    full = {**os.environ, "PYTHONPATH": str(ROOT / "python"), "PYXRAY_LOG": str(log),
            "PYXRAY_AUDIT": "1", "PYXRAY_POLICY": "off", "PYXRAY_DRAW": "never"}
    full.pop("PYXRAY_RUN", None)
    proc = subprocess.run([sys.executable, "-m", "pyxray_agent.intercept", "--", sys.executable, str(script)],
                          cwd=tmp_path, env=full, capture_output=True, text=True, timeout=60)
    assert proc.returncode == 0, proc.stderr
    lines = [json.loads(line) for line in log.read_text().splitlines()]
    events = [x for x in lines if x.get("kind") != "observed"]
    observed = [x for x in lines if x.get("kind") == "observed"]
    assert len(events) == 1 and observed, lines
    assert events[0]["run"] == observed[0]["run"]
    assert ("fs_delete", "remove") in items(observed[0])


def test_overhead_is_small(tmp_path):
    """Informational, not a gate: an open-heavy loop with and without."""
    code = "import os\nfor i in range(3000):\n    open(__file__ if False else os.devnull).close()\n"
    def timed(**env: str) -> float:
        t = time.perf_counter()
        subprocess.run([sys.executable, "-c", code], env={**os.environ, **env}, cwd=tmp_path,
                       capture_output=True, timeout=120)
        return time.perf_counter() - t
    base = min(timed() for _ in range(3))
    audited = min(timed(PYTHONPATH=str(AUDIT), PYXRAY_RUN="r", PYXRAY_AUDIT_LOG=str(tmp_path / "f.jsonl"))
                  for _ in range(3))
    print(f"\naudit overhead: {base * 1000:.0f} ms -> {audited * 1000:.0f} ms")
    assert audited < base * 3 + 0.5
