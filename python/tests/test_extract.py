"""The extraction cases shared with the Rust test-suite.

`tests/fixtures/extract_cases.json` is the single source of truth: the Rust
side runs it against `extract_all` directly, this side runs it through
whichever engine is available, so the extension, the binary fallback and the
core can never quietly disagree about what a command runs.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pyxray_agent as pyxray

FIXTURE = Path(__file__).resolve().parents[2] / "tests" / "fixtures" / "extract_cases.json"


def _cases() -> list[dict]:
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


def test_every_shared_case_extracts_as_expected():
    cases = _cases()
    assert len(cases) >= 40
    for case in cases:
        with tempfile.TemporaryDirectory() as tmp:
            for rel, body in (case.get("files") or {}).items():
                path = Path(tmp) / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(body.encode("utf-8"))
            if case.get("depth") == 0:
                # Depth is not exposed through the Python API; the Rust test
                # covers it.
                continue
            got = [(f["source"], f["label"]) for f in pyxray.extract_all(case["command"], cwd=tmp)]
            want = [(e["source"], e["label"]) for e in case["expect"]]
            assert got == want, (case["name"], case["command"], got, want)


def test_paths_and_segments_come_back():
    with tempfile.TemporaryDirectory() as tmp:
        (Path(tmp) / "s.py").write_bytes(b"print(1)\n")
        found = pyxray.extract_all("cd . && python3 s.py --fast", cwd=tmp)
        assert len(found) == 1
        assert found[0]["path"] and found[0]["path"].endswith("s.py")
        assert found[0]["segment"] == "python3 s.py --fast"
        unread = pyxray.extract_all("python3 s.py", cwd=tmp, read_files=False)
        assert unread[0]["source"] == "" and unread[0]["path"]


def test_classify_argv_reads_like_cpython():
    assert pyxray.classify_argv(["-X", "faulthandler", "s.py"])["script"] == "s.py"
    assert pyxray.classify_argv(["-W", "ignore", "-c", "code"])["code"] == "code"
    assert pyxray.classify_argv(["-uBc", "code"])["code"] == "code"
    assert pyxray.classify_argv(["-m", "pytest"])["module"] == "pytest"
    assert pyxray.classify_argv(["-"])["stdin"] is True
    assert pyxray.classify_argv([])["stdin"] is True


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_"):
            fn()
            print(f"ok    {name}")
