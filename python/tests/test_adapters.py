"""Every harness, both phases: the verdict is shared, only the dialect differs.

Each case is a payload in the shape the harness really sends, run through
``pyxray_agent.hook`` the way the harness runs it — as a subprocess, JSON on
stdin — plus direct calls into ``answer()`` for the finer points.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from pyxray_agent import adapters, hook, verdict  # noqa: E402
from pyxray_agent import install  # noqa: E402

RISKY = "python3 -c \"import os; os.system('rm -rf build')\""
CALM = "python3 -c \"print(1)\""

# The payload each harness sends before a shell tool runs.
PRE = {
    "claude": lambda cmd: {"tool_name": "Bash", "session_id": "s-claude-1", "cwd": "/tmp",
                           "tool_input": {"command": cmd}},
    "codex": lambda cmd: {"tool_name": "exec_command", "session_id": "s-codex-1", "cwd": "/tmp",
                          "tool_input": {"command": ["bash", "-lc", cmd], "workdir": "/tmp"}},
    "gemini": lambda cmd: {"hook_event_name": "BeforeTool", "tool_name": "run_shell_command",
                           "session_id": "s-gemini-1", "cwd": "/tmp",
                           "tool_input": {"command": cmd, "description": "x"}},
    "cursor": lambda cmd: {"command": cmd, "cwd": "/tmp", "conversation_id": "s-cursor-1"},
}
# And after it ran.
POST = {
    "claude": PRE["claude"],
    "codex": PRE["codex"],
    "gemini": lambda cmd: {**PRE["gemini"](cmd), "hook_event_name": "AfterTool",
                           "tool_response": {"llmContent": "ok"}},
    "cursor": lambda cmd: {"tool_name": "Shell", "tool_input": {"command": cmd}, "cwd": "/tmp",
                           "tool_output": "{}"},
}


@pytest.fixture
def env(tmp_path, monkeypatch):
    log = tmp_path / "feed.jsonl"
    monkeypatch.setenv("PYXRAY_LOG", str(log))
    monkeypatch.setenv("PYXRAY_DRAW", "never")
    for var in ("PYXRAY_GATE", "PYXRAY_GATE_MODE", "PYXRAY_WARN", "PYXRAY_OFF", "PYXRAY_SOURCE"):
        monkeypatch.delenv(var, raising=False)
    return log


def run_hook(harness: str, phase: str, payload, **extra: str) -> subprocess.CompletedProcess:
    stdin = payload if isinstance(payload, str) else json.dumps(payload)
    full = {**os.environ, "PYTHONPATH": str(ROOT / "python"), **extra}
    return subprocess.run(
        [sys.executable, "-m", "pyxray_agent.hook", harness, phase],
        input=stdin, capture_output=True, text=True, env=full, timeout=60,
    )


def rows(log: Path) -> list[dict]:
    if not log.exists():
        return []
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines() if line.strip()]


# --------------------------------------------------------------------------- pre, ungated


@pytest.mark.parametrize("harness", sorted(PRE))
def test_ungated_pre_records_and_lets_it_run(harness, env):
    proc = run_hook(harness, "pre", PRE[harness](RISKY))
    assert proc.returncode == 0, proc.stderr
    if harness == "cursor":
        assert json.loads(proc.stdout) == {"permission": "allow"}
    else:
        assert proc.stdout == ""
    found = rows(env)
    assert len(found) == 1, found
    assert found[0]["band"] == "check"
    assert found[0]["source"].startswith(harness)


# --------------------------------------------------------------------------- pre, over the gate


def test_claude_asks(env):
    out = json.loads(run_hook("claude", "pre", PRE["claude"](RISKY), PYXRAY_GATE="10").stdout)
    spec = out["hookSpecificOutput"]
    assert spec["hookEventName"] == "PreToolUse"
    assert spec["permissionDecision"] == "ask"
    assert "over the gate of 10" in spec["permissionDecisionReason"]
    assert out["decision"] == "block"  # for Hermes' shell bridge and OpenCode


def test_claude_hard_mode_denies(env):
    out = json.loads(run_hook("claude", "pre", PRE["claude"](RISKY),
                              PYXRAY_GATE="10", PYXRAY_GATE_MODE="deny").stdout)
    assert out["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_codex_denies_because_it_cannot_ask(env):
    out = json.loads(run_hook("codex", "pre", PRE["codex"](RISKY), PYXRAY_GATE="10").stdout)
    assert out["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "cannot ask" in out["reason"]


def test_codex_argv_arrays_keep_their_quoting(env):
    """["bash", "-lc", "python3 -c '…'"] joined with spaces would split the
    -lc string apart; it has to be quoted back into one command."""
    run_hook("codex", "pre", PRE["codex"](RISKY))
    found = rows(env)
    assert found and "rm -rf build" in found[0]["synopsis"]


def test_gemini_denies_in_json_not_by_exit_code(env):
    proc = run_hook("gemini", "pre", PRE["gemini"](RISKY), PYXRAY_GATE="10")
    assert proc.returncode == 0
    out = json.loads(proc.stdout)
    assert out["decision"] == "deny"
    assert "over the gate" in out["reason"]


def test_cursor_asks_with_both_messages(env):
    out = json.loads(run_hook("cursor", "pre", PRE["cursor"](RISKY), PYXRAY_GATE="10").stdout)
    assert out["permission"] == "ask"
    assert "over the gate" in out["user_message"]
    assert "rm -rf build" in out["agent_message"]


def test_cursor_pre_tool_use_shape_is_understood(env):
    payload = {"tool_name": "Shell", "tool_input": {"command": RISKY}, "cwd": "/tmp"}
    out = json.loads(run_hook("cursor", "pre", payload, PYXRAY_GATE="10").stdout)
    assert out["permission"] == "ask"


@pytest.mark.parametrize("harness", sorted(PRE))
def test_calm_code_is_never_stopped(harness, env):
    proc = run_hook(harness, "pre", PRE[harness](CALM), PYXRAY_GATE="10")
    assert proc.returncode == 0
    if harness == "cursor":
        assert json.loads(proc.stdout) == {"permission": "allow"}
    else:
        assert proc.stdout == ""


# --------------------------------------------------------------------------- post


@pytest.mark.parametrize("harness", sorted(POST))
def test_post_tells_the_model_and_records_nothing(harness, env):
    proc = run_hook(harness, "post", POST[harness](RISKY))
    assert proc.returncode == 0, proc.stderr
    out = json.loads(proc.stdout)
    text = json.dumps(out)
    assert "pyxray" in text and "rm -rf build" in text
    if harness == "cursor":
        assert "additional_context" in out
    else:
        assert "additionalContext" in out["hookSpecificOutput"]
    assert rows(env) == [], "the post phase must not log the call a second time"


@pytest.mark.parametrize("harness", sorted(POST))
def test_post_is_silent_below_the_warning_band(harness, env):
    proc = run_hook(harness, "post", POST[harness](CALM))
    assert proc.returncode == 0
    assert proc.stdout == ""


def test_post_warning_floor_is_configurable(env):
    proc = run_hook("claude", "post", POST["claude"](RISKY), PYXRAY_WARN="never")
    assert proc.stdout == ""


# --------------------------------------------------------------------------- never break the call


@pytest.mark.parametrize("harness", sorted(PRE))
@pytest.mark.parametrize("junk", ["", "not json", "[1, 2]", '{"tool_name": 7}', "null"])
def test_garbage_in_is_exit_zero(harness, junk, env):
    proc = run_hook(harness, "pre", junk)
    assert proc.returncode == 0
    if harness == "cursor":
        assert json.loads(proc.stdout) == {"permission": "allow"}
    else:
        assert proc.stdout == ""


@pytest.mark.parametrize("harness", sorted(PRE))
def test_a_broken_engine_is_exit_zero(harness, env, monkeypatch, capsys):
    """No engine at all — nothing on PATH, no extension, no checkout build —
    must look exactly like "nothing to say", never like a refusal."""
    import io

    from pyxray_agent import _native, intercept

    def broken():
        raise _native.EngineError("no engine for this test")

    monkeypatch.setattr(_native, "engine", broken)
    monkeypatch.setattr(intercept, "engine", broken)
    monkeypatch.setenv("PYXRAY_GATE", "0")
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(PRE[harness](RISKY))))
    assert hook.main([harness, "pre"]) == 0
    out = capsys.readouterr().out
    if harness == "cursor":
        assert json.loads(out) == {"permission": "allow"}
    else:
        assert out == ""


def test_unknown_harness_or_phase_is_exit_zero(env):
    assert run_hook("nope", "pre", PRE["claude"](RISKY)).returncode == 0
    assert run_hook("claude", "sideways", PRE["claude"](RISKY)).returncode == 0


def test_pyxray_off(env):
    proc = run_hook("claude", "pre", PRE["claude"](RISKY), PYXRAY_OFF="1", PYXRAY_GATE="0")
    assert proc.stdout == ""
    assert rows(env) == []


def test_intercept_hook_is_the_claude_pre_hook(env):
    """Hooks installed before the adapters call `intercept --hook`."""
    full = {**os.environ, "PYTHONPATH": str(ROOT / "python"), "PYXRAY_GATE": "10"}
    proc = subprocess.run([sys.executable, "-m", "pyxray_agent.intercept", "--hook"],
                          input=json.dumps(PRE["claude"](RISKY)), capture_output=True,
                          text=True, env=full, timeout=60)
    out = json.loads(proc.stdout)
    assert out["hookSpecificOutput"]["permissionDecision"] == "ask"
    assert rows(env)[0]["source"].startswith("hook")


# --------------------------------------------------------------------------- the launcher


@pytest.mark.skipif(os.name == "nt", reason="the sh launcher")
def test_the_launcher_still_answers_cursor_with_nothing_on_path(tmp_path):
    launcher = ROOT / "integrations" / "claude-code" / "hooks" / "pyxray-hook"
    proc = subprocess.run(["/bin/sh", str(launcher), "cursor", "pre"], input='{"command": "ls"}',
                          capture_output=True, text=True, env={"PATH": str(tmp_path)}, timeout=30)
    assert proc.returncode == 0
    assert json.loads(proc.stdout) == {"permission": "allow"}


# --------------------------------------------------------------------------- the installer


def test_installer_merges_and_removes_only_its_own_entries(tmp_path):
    cfg = tmp_path / ".cursor" / "hooks.json"
    cfg.parent.mkdir()
    theirs = {"command": "./audit.sh"}
    cfg.write_text(json.dumps({"version": 1, "hooks": {"beforeShellExecution": [theirs]}}))

    assert install.main(["cursor", "--project", str(tmp_path), "--apply"]) == 0
    assert install.main(["cursor", "--project", str(tmp_path), "--apply"]) == 0  # idempotent
    data = json.loads(cfg.read_text())
    before = data["hooks"]["beforeShellExecution"]
    assert before[0] == theirs and len(before) == 2
    assert "cursor pre" in before[1]["command"]
    assert "cursor post" in data["hooks"]["postToolUse"][0]["command"]
    assert list(tmp_path.glob(".cursor/hooks.json.bak.*")), "no backup kept"

    assert install.main(["cursor", "--project", str(tmp_path), "--apply", "--uninstall"]) == 0
    data = json.loads(cfg.read_text())
    assert data["hooks"] == {"beforeShellExecution": [theirs]}


@pytest.mark.parametrize("harness", ["codex", "gemini"])
def test_installer_writes_each_harness(harness, tmp_path):
    assert install.main([harness, "--project", str(tmp_path), "--apply"]) == 0
    path, _ = install.HARNESSES[harness](tmp_path)
    data = json.loads(path.read_text())
    text = json.dumps(data)
    assert f"{harness} pre" in text
    if harness == "gemini":
        assert data["hooks"]["BeforeTool"][0]["matcher"] == "run_shell_command"
        assert f"{harness} post" in text
    else:
        assert "PostToolUse" not in data["hooks"], "Codex post hooks are audit-only"


def test_installer_refuses_to_touch_invalid_json(tmp_path, capsys):
    cfg = tmp_path / ".gemini" / "settings.json"
    cfg.parent.mkdir()
    cfg.write_text("{ not json")
    assert install.main(["gemini", "--project", str(tmp_path), "--apply"]) == 1
    assert cfg.read_text() == "{ not json"


# --------------------------------------------------------------------------- verdict, directly


def _findings(*events: dict) -> verdict.Findings:
    f = verdict.Findings(tool="t")
    for e in events:
        f.snippets.append(verdict.Snippet(source="", label="x", event=e))
    return f


def test_judge_gate_boundary():
    at = _findings({"risk": 40, "band": "check", "synopsis": "s", "notes": []})
    assert verdict.judge(at, verdict.Settings(gate=40)).decision == "allow"
    assert verdict.judge(at, verdict.Settings(gate=39)).decision == "ask"
    assert verdict.judge(at, verdict.Settings(gate=39, hard=True)).decision == "deny"
    assert verdict.judge(at, verdict.Settings()).decision == "allow"


def test_judge_takes_the_strictest_of_gate_and_policy():
    event = {"risk": 10, "band": "routine", "notes": [],
             "policy": {"decision": "deny", "rule": ".pyxray.toml#2 net deny", "reason": "net to x"}}
    v = verdict.judge(_findings(event), verdict.Settings(gate=50))
    assert v.decision == "deny" and v.rule == ".pyxray.toml#2 net deny"
    event["policy"]["decision"] = "ask"
    assert verdict.judge(_findings(event), verdict.Settings(gate=5)).decision == "ask"


def test_judge_worst_of_several_snippets():
    f = _findings({"risk": 5, "band": "routine"}, {"risk": 70, "band": "read", "synopsis": "deletes"})
    v = verdict.judge(f, verdict.Settings(gate=50))
    assert v.decision == "ask" and "deletes" in v.reason


def test_answer_for_an_unreadable_payload_is_the_fallback():
    assert hook.answer("cursor", "pre", None) == {"permission": "allow"}
    assert hook.answer("claude", "pre", None) is None
    assert hook.answer("cursor", "post", None) is None


def test_every_adapter_is_registered():
    assert set(adapters.ADAPTERS) == {"claude", "codex", "gemini", "cursor"}
    with pytest.raises(ValueError):
        adapters.get("vim")


# --------------------------------------------------------------------------- following imports


def test_the_hook_follows_a_scripts_local_imports(env, tmp_path):
    (tmp_path / "helper.py").write_bytes(b"import os\n\ndef run(c):\n    os.system(c)\n")
    (tmp_path / "main.py").write_bytes(b"import helper\nhelper.run('make clean')\n")
    payload = {"tool_name": "Bash", "cwd": str(tmp_path), "tool_input": {"command": "python3 main.py"}}
    out = json.loads(run_hook("claude", "pre", payload, PYXRAY_GATE="10").stdout)
    assert out["hookSpecificOutput"]["permissionDecision"] == "ask"
    assert "helper.py" in out["reason"]
    row = rows(env)[0]
    assert row["risk"] >= 30
    assert any(n.get("file") == "helper.py" for n in row["notes"])


def test_following_can_be_switched_off(env, tmp_path):
    (tmp_path / "helper.py").write_bytes(b"import os\nos.system('x')\n")
    (tmp_path / "main.py").write_bytes(b"import helper\n")
    payload = {"tool_name": "Bash", "cwd": str(tmp_path), "tool_input": {"command": "python3 main.py"}}
    run_hook("claude", "pre", payload, PYXRAY_FOLLOW="0")
    assert rows(env)[0]["risk"] < 30
