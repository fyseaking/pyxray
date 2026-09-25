"""The Hermes plugin's contract, exercised with a fake ``ctx``.

Hermes itself is not a dependency: ``integrations/hermes`` is loaded as a
package straight from the checkout and driven the way Hermes drives it —
``register(ctx)``, then hook calls with keyword arguments.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[2]
PLUGIN_DIR = ROOT / "integrations" / "hermes"

sys.path.insert(0, str(ROOT / "python"))

import pyxray_agent as pyxray  # noqa: E402


def _load_plugin() -> Any:
    name = "pyxray_hermes_plugin_under_test"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(
        name, PLUGIN_DIR / "__init__.py", submodule_search_locations=[str(PLUGIN_DIR)]
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


class FakeCtx:
    """Just enough of Hermes' PluginContext to register against."""

    def __init__(self, settings: dict[str, Any] | None = None) -> None:
        self.hooks: dict[str, list[Any]] = {}
        self.commands: dict[str, Any] = {}
        self.cli: dict[str, Any] = {}
        self.settings = dict(settings or {})

    def register_hook(self, name: str, fn: Any) -> None:
        self.hooks.setdefault(name, []).append(fn)

    def register_command(self, name: str, handler: Any, description: str = "", args_hint: str = "") -> None:
        self.commands[name] = handler

    def register_cli_command(self, name: str, help: str, setup_fn: Any, handler_fn: Any = None, **_: Any) -> None:
        self.cli[name] = (setup_fn, handler_fn)

    def get_config(self, key: str, default: Any = None) -> Any:
        return self.settings.get(key, default)

    def set_config(self, key: str, value: Any) -> None:
        self.settings[key] = value


@pytest.fixture
def plugin(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Any:
    monkeypatch.setenv("PYXRAY_LOG", str(tmp_path / "feed.jsonl"))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / "hermes"))
    monkeypatch.delenv("PYXRAY_GATE", raising=False)
    monkeypatch.delenv("PYXRAY_DRAW", raising=False)
    monkeypatch.delenv("PYXRAY_OFF", raising=False)
    mod = _load_plugin()
    mod._recent.clear()
    mod._warned_missing = False
    return mod


def _register(plugin: Any, **settings: Any) -> FakeCtx:
    ctx = FakeCtx(settings)
    plugin.register(ctx)
    return ctx


def _pre(ctx: FakeCtx, tool: str, args: dict[str, Any], **kw: Any) -> Any:
    (fn,) = ctx.hooks["pre_tool_call"]
    return fn(tool_name=tool, args=args, task_id="t1", session_id="abcdef0123", **kw)


def _post(ctx: FakeCtx, tool: str, args: dict[str, Any], result: str, **kw: Any) -> Any:
    (fn,) = ctx.hooks["transform_tool_result"]
    return fn(tool_name=tool, args=args, result=result, task_id="t1", **kw)


# --------------------------------------------------------------------------- registration


def test_register_wires_hooks_commands_and_cli(plugin: Any) -> None:
    ctx = _register(plugin)
    assert set(ctx.hooks) == {"pre_tool_call", "transform_tool_result"}
    assert "pyxray" in ctx.commands
    assert "pyxray" in ctx.cli
    # Inside an agent nothing should be drawn to Hermes' own stderr.
    assert os.environ.get("PYXRAY_DRAW") == "never"


def test_manifest_matches_registration() -> None:
    text = (PLUGIN_DIR / "plugin.yaml").read_text(encoding="utf-8")
    assert "name: pyxray" in text
    for hook in ("pre_tool_call", "transform_tool_result"):
        assert hook in text
    manifest = json.loads((PLUGIN_DIR / "dashboard" / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["name"] == "pyxray"
    assert manifest["api"] == "plugin_api.py"
    assert (PLUGIN_DIR / "dashboard" / manifest["api"]).is_file()
    assert (PLUGIN_DIR / "desktop" / "plugin.js").is_file()


# --------------------------------------------------------------------------- pre_tool_call


def test_terminal_call_writes_a_feed_row_and_passes_ungated(plugin: Any, tmp_path: Path) -> None:
    ctx = _register(plugin)
    verdict = _pre(ctx, "terminal", {"command": 'python3 -c "import os; os.system(\'ls\')"'})
    assert verdict is None
    rows = pyxray.feed()
    assert len(rows) == 1
    assert rows[0]["source"].startswith("hermes:abcdef01")
    assert rows[0]["band"] == "check"


def test_execute_code_is_read_directly(plugin: Any) -> None:
    ctx = _register(plugin)
    _pre(ctx, "execute_code", {"code": "import shutil\nshutil.rmtree('/tmp/x')\n"})
    rows = pyxray.feed()
    assert rows and rows[0]["band"] in ("check", "read")
    assert any(n["effect"] == "fs_delete" for n in rows[0]["notes"])


def test_over_the_gate_escalates_to_approval(plugin: Any) -> None:
    ctx = _register(plugin, gate=10)
    verdict = _pre(ctx, "terminal", {"command": "python3 -c 'import os; os.system(\"rm -rf /\")'"})
    assert verdict is not None
    assert verdict["action"] == "approve"
    assert "over the gate of 10" in verdict["message"]
    assert verdict["rule_key"].startswith("pyxray:")


def test_block_mode_refuses(plugin: Any) -> None:
    ctx = _register(plugin, gate=10, mode="block")
    verdict = _pre(ctx, "execute_code", {"code": "import os; os.system('x')"})
    assert verdict["action"] == "block"
    assert "raise or unset" in verdict["message"]


def test_env_gate_beats_config_gate(plugin: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PYXRAY_GATE", "off")
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "execute_code", {"code": "import os; os.system('x')"}) is None


def test_inert_code_never_escalates(plugin: Any) -> None:
    # gate=0 really does mean "anything with an effect", and print is one.
    ctx = _register(plugin, gate=5)
    assert _pre(ctx, "execute_code", {"code": "x = 1 + 1\nprint(x)\n"}) is None


def test_other_tools_are_ignored(plugin: Any) -> None:
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "write_file", {"path": "a.py", "content": "import os; os.system('x')"}) is None
    assert pyxray.feed() == []


def test_shell_without_python_is_ignored(plugin: Any) -> None:
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "terminal", {"command": "ls -la && git status"}) is None
    assert pyxray.feed() == []


def test_script_path_is_read_relative_to_cwd(plugin: Any, tmp_path: Path) -> None:
    (tmp_path / "job.py").write_bytes(b"import subprocess\nsubprocess.run(['curl', 'x'])\n")
    ctx = _register(plugin)
    _pre(ctx, "terminal", {"command": "python3 job.py", "cwd": str(tmp_path)})
    rows = pyxray.feed()
    assert rows and rows[0]["name"].endswith("job.py")


# --------------------------------------------------------------------------- transform_tool_result


def test_warning_rides_back_on_the_result(plugin: Any) -> None:
    ctx = _register(plugin)
    args = {"command": "python3 -c 'import os; os.system(\"ls\")'"}
    _pre(ctx, "terminal", args, tool_call_id="c1")
    out = _post(ctx, "terminal", args, "ok\n", tool_call_id="c1")
    assert out is not None and out.startswith("ok\n")
    assert "pyxray — check it" in out
    assert "shell ls" in out


def test_no_warning_below_the_band_floor(plugin: Any) -> None:
    ctx = _register(plugin)
    args = {"code": "print('hi')"}
    _pre(ctx, "execute_code", args)
    assert _post(ctx, "execute_code", args, "hi\n") is None


def test_warning_floor_is_configurable(plugin: Any) -> None:
    ctx = _register(plugin, warn_from="never")
    args = {"code": "import os; os.system('x')"}
    _pre(ctx, "execute_code", args)
    assert _post(ctx, "execute_code", args, "") is None


def test_result_without_a_matching_call_is_untouched(plugin: Any) -> None:
    ctx = _register(plugin)
    assert _post(ctx, "terminal", {"command": "python3 -c 'import os'"}, "x") is None


def test_findings_are_consumed_once(plugin: Any) -> None:
    ctx = _register(plugin)
    args = {"code": "import os; os.system('x')"}
    _pre(ctx, "execute_code", args)
    assert _post(ctx, "execute_code", args, "") is not None
    assert _post(ctx, "execute_code", args, "") is None


# --------------------------------------------------------------------------- cards


def test_cards_are_written_and_trimmed(plugin: Any, tmp_path: Path) -> None:
    ctx = _register(plugin, max_cards=2)
    for i in range(4):
        _pre(ctx, "execute_code", {"code": f"import os; os.system('cmd{i}')"})
    root = plugin.guard.data_dir()
    assert root == tmp_path / "hermes" / "plugin-data" / "pyxray"
    index = plugin.guard.read_recent()
    assert len(index) == 2
    assert index[0]["event"]["synopsis"].endswith("`cmd3`")
    cards = sorted((root / "cards").iterdir())
    assert len(cards) == 2
    html = cards[0].read_text(encoding="utf-8")
    assert "<" in html and "pyxray" in html.lower() or "<svg" in html or "<html" in html


def test_cards_can_be_switched_off(plugin: Any) -> None:
    ctx = _register(plugin, cards=False)
    _pre(ctx, "execute_code", {"code": "import os; os.system('x')"})
    assert plugin.guard.read_recent() == []


# --------------------------------------------------------------------------- slash commands


def test_slash_status_and_last(plugin: Any) -> None:
    ctx = _register(plugin, gate=50)
    cmd = ctx.commands["pyxray"]
    assert "nothing seen yet" in cmd("last")
    _pre(ctx, "execute_code", {"code": "import os; os.system('x')"})
    status = cmd("")
    assert "engine" in status and "gate     50 → approve" in status
    last = cmd("last 5")
    assert "check it" in last and "runs `x`" in last
    shown = cmd("show")
    assert "x" in shown and len(shown.splitlines()) > 3


def test_slash_gate_writes_config(plugin: Any) -> None:
    ctx = _register(plugin)
    cmd = ctx.commands["pyxray"]
    assert "off" in cmd("gate")
    assert "35" in cmd("gate 35")
    assert ctx.settings["gate"] == 35
    assert "off" in cmd("gate off")
    assert ctx.settings["gate"] == "off"


def test_slash_unknown_subcommand(plugin: Any) -> None:
    ctx = _register(plugin)
    assert "unknown subcommand" in ctx.commands["pyxray"]("frobnicate")


# --------------------------------------------------------------------------- resilience


def test_missing_engine_never_breaks_the_call(plugin: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    guard = plugin.guard

    def boom(*_: Any, **__: Any) -> Any:
        raise guard.EngineMissing("no engine for the test")

    monkeypatch.setattr(guard, "pyxray_module", boom)
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "execute_code", {"code": "import os; os.system('x')"}) is None
    assert _post(ctx, "execute_code", {"code": "import os"}, "r") is None
    assert "missing" in ctx.commands["pyxray"]("status")


def test_hook_survives_a_broken_payload(plugin: Any) -> None:
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "terminal", {"command": None}) is None
    assert _pre(ctx, "terminal", "not a dict") is None  # type: ignore[arg-type]
    assert _post(ctx, "terminal", {"command": "x"}, {"not": "a string"}) is None  # type: ignore[arg-type]


def test_pyxray_off_switch(plugin: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PYXRAY_OFF", "1")
    ctx = _register(plugin, gate=0)
    assert _pre(ctx, "execute_code", {"code": "import os; os.system('x')"}) is None
    assert pyxray.feed() == []


# --------------------------------------------------------------------------- dashboard half


def test_dashboard_routes_read_what_the_agent_wrote(plugin: Any) -> None:
    pytest.importorskip("fastapi")
    import asyncio

    ctx = _register(plugin, gate=20)
    _pre(ctx, "execute_code", {"code": "import os; os.system('x')"})

    spec = importlib.util.spec_from_file_location("pyxray_dashboard_under_test", PLUGIN_DIR / "dashboard" / "plugin_api.py")
    assert spec and spec.loader
    api = importlib.util.module_from_spec(spec)
    sys.modules["pyxray_dashboard_under_test"] = api
    spec.loader.exec_module(api)
    # The dashboard loads guard.py by path; point that copy at the same data dir.
    status = asyncio.run(api.status())
    assert status["engine"]["ok"] is True
    assert status["tools"] == ["terminal", "execute_code"]
    recent = asyncio.run(api.recent(limit=5))["rows"]
    assert len(recent) == 1 and recent[0]["event"]["band"] == "check"
    card = asyncio.run(api.card(recent[0]["card"]))
    assert "html" in card and card["html"]
    events = asyncio.run(api.events(limit=10, source="hermes"))["rows"]
    assert len(events) == 1
    with pytest.raises(Exception):
        asyncio.run(api.card("../etc/passwd"))
