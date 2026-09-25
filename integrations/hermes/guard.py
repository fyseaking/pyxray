"""The pyxray Hermes plugin, minus Hermes.

Everything in here is plain Python with no Hermes import, so it can be
tested with a fake ``ctx`` and reused by the dashboard half. ``__init__.py``
is the thin layer that wires these functions into ``register(ctx)``.

Flow for one tool call:

    pre_tool_call  ── scan() ──▶ Any ──▶ decide()  ─▶ approve / block / None
                                    │
                                    └── remember() ─▶ transform_tool_result appends
                                                       format_block() to the result

The feed row is written by ``pyxray_agent.intercept.look`` (the same call the
Claude Code hook and the shim make), so ``pyx watch`` and the desktop pane
see Hermes' snippets alongside everyone else's.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

PLUGIN_ID = "pyxray"
PLUGIN_DIR = Path(__file__).resolve().parent

# Band order, least to most interesting. The same as `pyxray_agent.verdict`,
# repeated here (with `parse_gate`) because settings are read — for the
# dashboard and for `/pyxray status` — even when there is no engine to import.
BANDS = ("inert", "routine", "check", "read")
BAND_WORDS = {"inert": "inert", "routine": "routine", "check": "check it", "read": "read it first"}

DEFAULT_TOOLS = ("terminal", "execute_code")
DEFAULTS: dict[str, Any] = {
    "gate": None,
    "mode": "approve",
    "warn_from": "check",
    "tools": list(DEFAULT_TOOLS),
    "cards": True,
    "max_cards": 40,
}

_TRUTHY = {"1", "true", "yes", "on"}


# --------------------------------------------------------------------------- engine


class EngineMissing(RuntimeError):
    """No pyxray_agent package (and no engine behind it) could be found."""


_pyxray: Any = None
_pyxray_error: str | None = None
_pyxray_lock = threading.Lock()


def _checkout_roots() -> Iterable[Path]:
    """Places a pyxray checkout might be, relative to this plugin."""
    if root := os.environ.get("PYXRAY_ROOT"):
        yield Path(root)
    # integrations/hermes/ inside a checkout.
    yield PLUGIN_DIR.parent.parent
    # A checkout the user cloned next to Hermes' plugin dir, or into it.
    yield PLUGIN_DIR / "pyxray"
    yield PLUGIN_DIR.parent / "pyxray"


def pyxray_module() -> Any:
    """Import ``pyxray`` once, looking in a checkout if it is not installed.

    Raises :class:`EngineMissing` with a human-readable reason when nothing
    usable exists. Callers treat that as "describe nothing", never as an
    error that reaches the tool call.
    """
    global _pyxray, _pyxray_error
    if _pyxray is not None:
        return _pyxray
    if _pyxray_error is not None:
        raise EngineMissing(_pyxray_error)
    with _pyxray_lock:
        if _pyxray is not None:
            return _pyxray
        try:
            import pyxray_agent as pyxray  # type: ignore[import-not-found]
        except ImportError:
            pyxray = None
            for root in _checkout_roots():
                pkg = root / "python" / "pyxray_agent" / "__init__.py"
                if pkg.is_file():
                    sys.path.insert(0, str(pkg.parent.parent))
                    try:
                        import pyxray_agent as pyxray  # type: ignore[import-not-found]  # noqa: F811
                    except ImportError:
                        sys.path.pop(0)
                        continue
                    break
        if pyxray is None:
            _pyxray_error = (
                "the pyxray_agent package is not importable from Hermes' Python — "
                "`pip install pyxray-agent` into it, or set PYXRAY_ROOT to a checkout"
            )
            raise EngineMissing(_pyxray_error)
        try:
            # Resolve the engine now so a missing binary surfaces in /pyxray
            # status rather than on the first tool call.
            pyxray.backend()
        except Exception as exc:  # noqa: BLE001 — EngineError, or anything a broken build raises
            _pyxray_error = f"pyxray imported but has no engine: {exc}"
            raise EngineMissing(_pyxray_error) from exc
        _pyxray = pyxray
        return pyxray


def engine_status() -> dict[str, Any]:
    """What ``/pyxray`` and the desktop pane report about the engine."""
    try:
        mod = pyxray_module()
    except EngineMissing as exc:
        return {"ok": False, "error": str(exc), "feed": feed_path_fallback()}
    info: dict[str, Any] = {"ok": True, "backend": mod.backend(), "version": getattr(mod, "__version__", "?")}
    try:
        info["feed"] = mod.feed_path()
    except Exception:  # noqa: BLE001
        info["feed"] = feed_path_fallback()
    return info


def feed_path_fallback() -> str:
    """`pyxray_core::feed::default_path`, re-derived for when the engine is absent."""
    env = os.environ.get
    if p := env("PYXRAY_LOG"):
        return p
    if p := env("XDG_RUNTIME_DIR"):
        return str(Path(p) / "pyxray" / "feed.jsonl")
    if os.name == "nt" and (p := env("LOCALAPPDATA")):
        return str(Path(p) / "pyxray" / "feed.jsonl")
    if sys.platform == "darwin" and (p := env("TMPDIR")):
        return str(Path(p) / "pyxray" / "feed.jsonl")
    if p := env("XDG_CACHE_HOME"):
        return str(Path(p) / "pyxray" / "feed.jsonl")
    home = env("HOME") or env("USERPROFILE")
    if home:
        return str(Path(home) / ".cache" / "pyxray" / "feed.jsonl")
    who = env("USER") or env("USERNAME") or "user"
    import tempfile

    return str(Path(tempfile.gettempdir()) / f"pyxray-{who}" / "feed.jsonl")


# --------------------------------------------------------------------------- settings


@dataclass
class Settings:
    gate: int | None = None
    mode: str = "approve"
    warn_from: str = "check"
    tools: tuple[str, ...] = DEFAULT_TOOLS
    cards: bool = True
    max_cards: int = 40

    @classmethod
    def load(cls, get: Callable[[str, Any], Any] | None = None) -> "Settings":
        """Merge defaults, ``plugins.entries.pyxray.settings`` and the environment.

        ``PYXRAY_GATE`` wins over the config value because that is how every
        other pyxray integration is driven, and a shell variable is the
        quickest thing to flip while watching a session.
        """
        get = get or config_getter()
        env = os.environ.get

        gate = parse_gate(env("PYXRAY_GATE")) if env("PYXRAY_GATE") is not None else parse_gate(get("gate", None))
        mode = str(env("PYXRAY_HERMES_MODE") or get("mode", "approve") or "approve").lower()
        if mode not in ("approve", "block"):
            mode = "approve"
        warn = str(env("PYXRAY_HERMES_WARN") or get("warn_from", "check") or "check").lower()
        if warn not in BANDS and warn != "never":
            warn = "check"
        raw_tools = get("tools", None)
        if isinstance(raw_tools, str):
            raw_tools = [t.strip() for t in raw_tools.split(",")]
        tools = tuple(str(t) for t in raw_tools if str(t).strip()) if isinstance(raw_tools, (list, tuple)) else DEFAULT_TOOLS
        cards = get("cards", True)
        cards = str(cards).lower() in _TRUTHY if not isinstance(cards, bool) else cards
        try:
            max_cards = max(1, int(get("max_cards", 40) or 40))
        except (TypeError, ValueError):
            max_cards = 40
        return cls(gate=gate, mode=mode, warn_from=warn, tools=tools or DEFAULT_TOOLS, cards=cards, max_cards=max_cards)

    def core(self) -> Any:
        """The harness-neutral half, as :class:`pyxray_agent.verdict.Settings`."""
        return _verdict().Settings(gate=self.gate, hard=self.mode == "block", warn_from=self.warn_from)


def config_getter() -> Callable[[str, Any], Any]:
    """A ``get(key, default)`` over ``plugins.entries.pyxray.settings`` for
    processes that have no ``ctx`` (the dashboard, tests). Reads Hermes'
    config when Hermes is importable, otherwise sees no settings at all."""
    settings: dict[str, Any] = {}
    try:
        from hermes_cli.config import load_config_readonly  # type: ignore[import-not-found]

        cfg = load_config_readonly() or {}
        entry = ((cfg.get("plugins") or {}).get("entries") or {}).get(PLUGIN_ID) or {}
        found = entry.get("settings")
        if isinstance(found, dict):
            settings = found
    except Exception:  # noqa: BLE001
        settings = {}

    def get(key: str, default: Any = None) -> Any:
        return settings.get(key, default)

    return get


def save_gate(value: int | None, ctx: Any = None) -> None:
    """Persist ``plugins.entries.pyxray.settings.gate`` in config.yaml.

    Through ``ctx.set_config`` when the running Hermes has it, else through
    the config module directly (a partial, merged save — nothing else in the
    file is touched). Raises on a managed install or an unwritable file.
    """
    stored: Any = "off" if value is None else value
    if ctx is not None and hasattr(ctx, "set_config"):
        ctx.set_config("gate", stored)
        return
    from hermes_cli.config import save_config  # type: ignore[import-not-found]

    path = ("plugins", "entries", PLUGIN_ID, "settings", "gate")
    partial = {"plugins": {"entries": {PLUGIN_ID: {"settings": {"gate": stored}}}}}
    save_config(partial, preserve_keys={path}, merge_existing=True)


def parse_gate(raw: Any) -> int | None:
    """``None``/``off``/``none``/``-1``/empty → no gate; otherwise clamp to 0..100."""
    if raw is None or isinstance(raw, bool):
        return None
    if isinstance(raw, (int, float)):
        value = int(raw)
        return None if value < 0 else max(0, min(100, value))
    text = str(raw).strip().lower()
    if text in ("", "off", "none", "-1", "null"):
        return None
    try:
        return max(0, min(100, int(text)))
    except ValueError:
        return None


# --------------------------------------------------------------------------- scanning
#
# What counts as Python, how it is analysed and what the verdict is are the
# same for every harness, and live in `pyxray_agent.verdict`. This file only
# knows Hermes: which tools, where the settings are, and how Hermes spells
# "ask the human" (`approve`) and "refuse" (`block`).


def _verdict() -> Any:
    """``pyxray_agent.verdict``, from wherever :func:`pyxray_module` found the package."""
    pyxray_module()
    from pyxray_agent import verdict  # type: ignore[import-not-found]

    return verdict


class _NoFindings:
    """What :func:`scan` returns when there is no engine to scan with."""

    def __init__(self, tool: str, error: str | None = None) -> None:
        self.tool = tool
        self.snippets: list[Any] = []
        self.error = error
        self.worst = None
        self.empty = True


def _payload_for(tool_name: str, args: dict[str, Any], session: str = "") -> dict[str, Any]:
    """Shape Hermes' ``(tool_name, args)`` like the hook payload the verdict module reads."""
    payload: dict[str, Any] = {"tool_name": tool_name, "tool_input": args, "session_id": session}
    for key in ("cwd", "workdir", "working_directory"):
        if isinstance(args.get(key), str) and args[key]:
            payload["cwd"] = args[key]
            break
    return payload


def scan(tool_name: str, args: Any, settings: Settings, *, session: str = "") -> Any:
    """Find and analyse every piece of Python a tool call would run.

    Writes one feed row per snippet and returns ``verdict.Any``. Never
    raises: a missing engine or a broken payload gives empty findings with
    ``error`` set.
    """
    if tool_name not in settings.tools or not isinstance(args, dict):
        return _NoFindings(tool_name)
    if os.environ.get("PYXRAY_OFF") == "1":
        return _NoFindings(tool_name)
    try:
        verdict = _verdict()
    except EngineMissing as exc:
        return _NoFindings(tool_name, str(exc))
    return verdict.scan_payload(_payload_for(tool_name, args, session), source="hermes")


# --------------------------------------------------------------------------- deciding


def decide(findings: Any, settings: Settings) -> dict[str, Any] | None:
    """The ``pre_tool_call`` directive, if any.

    The shared verdict's ``ask`` is Hermes' ``approve`` (the human-approval
    prompt); ``deny`` is ``block``. ``rule_key`` lets the ``[a]lways`` answer
    at the prompt allow that band from then on rather than every call.
    """
    if findings.empty:
        return None
    v = _verdict().judge(findings, settings.core())
    if v.decision == "allow":
        return None
    if v.decision == "deny":
        return {"action": "block", "message": v.message + "\n\nTo run it anyway, raise or unset the pyxray gate."}
    band = findings.worst.band if findings.worst is not None else "check"
    return {"action": "approve", "message": v.message, "rule_key": f"pyxray:{band}"}


def wants_warning(findings: Any, settings: Settings) -> bool:
    if findings.empty:
        return False
    return bool(_verdict().wants_warning(findings, settings.core()))


def format_block(findings: Any, settings: Settings) -> str:
    """The Markdown that rides back to the model under the tool result."""
    if findings.empty:
        return ""
    return str(_verdict().context_text(findings, settings.core()))


# --------------------------------------------------------------------------- remembering


def _key(tool_name: str, args: Any, tool_call_id: str = "") -> str:
    if tool_call_id:
        return f"id:{tool_call_id}"
    try:
        blob = json.dumps(args, sort_keys=True, default=str)
    except (TypeError, ValueError):
        blob = repr(args)
    return f"{tool_name}:{hashlib.sha1(blob.encode('utf-8', 'replace')).hexdigest()}"


class Recent:
    """What the last few calls looked like — for the transform hook and ``/pyxray``.

    Sources stay in memory only; the on-disk card is a render of the analysis,
    not the code.
    """

    def __init__(self, keep: int = 64) -> None:
        self._by_key: OrderedDict[str, Any] = OrderedDict()
        self._order: list[Any] = []
        self._keep = keep
        self._lock = threading.Lock()

    def remember(self, tool_name: str, args: Any, findings: Any, tool_call_id: str = "") -> None:
        with self._lock:
            self._by_key[_key(tool_name, args, tool_call_id)] = findings
            while len(self._by_key) > self._keep:
                self._by_key.popitem(last=False)
            if not findings.empty:
                self._order.append(findings)
                del self._order[: -self._keep]

    def take(self, tool_name: str, args: Any, tool_call_id: str = "") -> Any | None:
        with self._lock:
            found = self._by_key.pop(_key(tool_name, args, tool_call_id), None)
            if found is None and not tool_call_id:
                return None
            if found is None:
                # The id was not known at pre_tool_call time; fall back to the args.
                found = self._by_key.pop(_key(tool_name, args), None)
            return found

    def last(self, n: int = 1) -> list[Any]:
        with self._lock:
            return list(self._order[-n:])[::-1]

    def clear(self) -> None:
        with self._lock:
            self._by_key.clear()
            self._order.clear()


# --------------------------------------------------------------------------- cards


def data_dir() -> Path:
    """``<hermes home>/plugin-data/pyxray``, with or without Hermes on the path."""
    try:
        from plugins.plugin_storage import plugin_data_dir  # type: ignore[import-not-found]

        return Path(plugin_data_dir(PLUGIN_ID))
    except Exception:  # noqa: BLE001 — not inside Hermes, or an older Hermes
        home = os.environ.get("HERMES_HOME") or str(Path.home() / ".hermes")
        path = Path(home) / "plugin-data" / PLUGIN_ID
        path.mkdir(parents=True, exist_ok=True)
        return path


def write_card(findings: Any, settings: Settings, *, now_ms: int | None = None) -> Path | None:
    """Render the worst snippet of a call to HTML for the desktop pane.

    ``recent.jsonl`` next to the cards is the index the dashboard reads:
    one line per call with the event and the card's file name.
    """
    if not settings.cards:
        return None
    worst = findings.worst
    if worst is None:
        return None
    try:
        mod = pyxray_module()
    except EngineMissing:
        return None
    root = data_dir()
    cards = root / "cards"
    cards.mkdir(parents=True, exist_ok=True)
    ts = now_ms if now_ms is not None else int(time.time() * 1000)
    name = f"{ts}-{hashlib.sha1(worst.source.encode('utf-8', 'replace')).hexdigest()[:8]}.html"
    try:
        html = mod.render(worst.source, name=worst.label, format="html", layout="card", width=96)
    except Exception:  # noqa: BLE001
        return None
    try:
        (cards / name).write_text(html, encoding="utf-8")
        entry = {
            "ts": ts,
            "tool": findings.tool,
            "label": worst.label,
            "card": name,
            "count": len(findings.snippets),
            "event": worst.event,
        }
        with (root / "recent.jsonl").open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, ensure_ascii=False) + "\n")
        _trim_cards(root, settings.max_cards)
    except OSError:
        return None
    return cards / name


def _trim_cards(root: Path, keep: int) -> None:
    index = root / "recent.jsonl"
    try:
        lines = index.read_text(encoding="utf-8").splitlines()
    except OSError:
        return
    if len(lines) <= keep:
        return
    dropped, kept = lines[:-keep], lines[-keep:]
    index.write_text("\n".join(kept) + "\n", encoding="utf-8")
    for line in dropped:
        try:
            card = json.loads(line).get("card")
        except (ValueError, AttributeError):
            continue
        if card:
            try:
                (root / "cards" / str(card)).unlink()
            except OSError:
                pass


def read_recent(limit: int = 50, root: Path | None = None) -> list[dict[str, Any]]:
    """The card index, newest first. Used by the dashboard and ``/pyxray last``."""
    root = root or data_dir()
    rows: list[dict[str, Any]] = []
    try:
        for line in (root / "recent.jsonl").read_text(encoding="utf-8").splitlines():
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if isinstance(row, dict):
                rows.append(row)
    except OSError:
        return []
    return rows[::-1][:limit]


# --------------------------------------------------------------------------- text helpers


def one_line(event: dict[str, Any], *, when: int | None = None, label: str = "") -> str:
    """``12:04:31  check it  30  runs `ls`  (terminal)`` — the feed row as a line."""
    band = str(event.get("band") or "inert")
    word = BAND_WORDS.get(band, band)
    stamp = ""
    if when:
        stamp = time.strftime("%H:%M:%S", time.localtime(when / 1000)) + "  "
    risk = event.get("risk", 0)
    synopsis = str(event.get("synopsis") or "").strip() or "—"
    tail = f"  ({label})" if label else ""
    return f"{stamp}{word:<14}{risk:>3}  {synopsis}{tail}"


def status_text(settings: Settings, recent: Recent | None = None) -> str:
    info = engine_status()
    lines = ["pyxray"]
    if info.get("ok"):
        lines.append(f"  engine   {info.get('backend')} (pyxray {info.get('version')})")
    else:
        lines.append(f"  engine   missing — {info.get('error')}")
    lines.append(f"  feed     {info.get('feed')}")
    gate = "off (describe only)" if settings.gate is None else f"{settings.gate} → {settings.mode}"
    lines.append(f"  gate     {gate}")
    lines.append(f"  warn     from '{settings.warn_from}' band")
    lines.append(f"  tools    {', '.join(settings.tools)}")
    lines.append(f"  cards    {'on' if settings.cards else 'off'} → {data_dir()}")
    if recent is not None:
        latest = recent.last(5)
        if latest:
            lines.append("  recent")
            for found in latest:
                worst = found.worst
                if worst:
                    lines.append("    " + one_line(worst.event, when=worst.event.get("ts"), label=found.tool))
    lines.append("  /pyxray last [n] · /pyxray show [n] · /pyxray gate <0-100|off>")
    return "\n".join(lines)
