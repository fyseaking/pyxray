"""The pyxray Hermes plugin, minus Hermes.

Everything in here is plain Python with no Hermes import, so it can be
tested with a fake ``ctx`` and reused by the dashboard half. ``__init__.py``
is the thin layer that wires these functions into ``register(ctx)``.

Flow for one tool call:

    pre_tool_call  ── scan() ──▶ Findings ──▶ decide()  ─▶ approve / block / None
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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable

PLUGIN_ID = "pyxray"
PLUGIN_DIR = Path(__file__).resolve().parent

# Band order, least to most interesting. Mirrors pyxray-core's `Band`.
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


@dataclass
class Snippet:
    source: str
    label: str
    event: dict[str, Any]

    @property
    def band(self) -> str:
        return str(self.event.get("band") or "inert")

    @property
    def risk(self) -> int:
        try:
            return int(self.event.get("risk") or 0)
        except (TypeError, ValueError):
            return 0


@dataclass
class Findings:
    tool: str
    snippets: list[Snippet] = field(default_factory=list)
    error: str | None = None

    @property
    def worst(self) -> Snippet | None:
        return max(self.snippets, key=lambda s: (s.risk, BANDS.index(s.band) if s.band in BANDS else 0), default=None)

    @property
    def empty(self) -> bool:
        return not self.snippets


def _payload_for(tool_name: str, args: dict[str, Any]) -> dict[str, Any]:
    """Shape Hermes' ``(tool_name, args)`` like the hook payload ``python_in_payload`` reads."""
    payload: dict[str, Any] = {"tool_name": tool_name, "tool_input": args}
    for key in ("cwd", "workdir", "working_directory"):
        if isinstance(args.get(key), str) and args[key]:
            payload["cwd"] = args[key]
            break
    return payload


def scan(tool_name: str, args: Any, settings: Settings, *, session: str = "") -> Findings:
    """Find and analyse every piece of Python a tool call would run.

    Writes one feed row per snippet (through ``pyxray_agent.intercept.look``) and
    returns the events. Never raises: a missing engine or a broken payload
    gives an empty ``Findings`` with ``error`` set.
    """
    found = Findings(tool=tool_name)
    if tool_name not in settings.tools or not isinstance(args, dict):
        return found
    if os.environ.get("PYXRAY_OFF") == "1":
        return found
    try:
        pyxray_module()
        from pyxray_agent import intercept  # type: ignore[import-not-found]
    except EngineMissing as exc:
        found.error = str(exc)
        return found
    try:
        items = intercept.python_in_payload(_payload_for(tool_name, args))
    except Exception as exc:  # noqa: BLE001
        found.error = f"could not read the payload: {exc}"
        return found
    origin = f"hermes{':' + session[:8] if session else ''}"
    for source, label in items:
        try:
            event = intercept.look(source, label, source=origin)
        except Exception as exc:  # noqa: BLE001 — look() already swallows; belt and braces
            event = {"risk": 0, "band": "inert", "synopsis": "", "notes": [], "error": str(exc)}
        found.snippets.append(Snippet(source=source, label=label, event=event))
    return found


# --------------------------------------------------------------------------- deciding


def decide(findings: Findings, settings: Settings) -> dict[str, Any] | None:
    """The ``pre_tool_call`` directive, if any.

    Over the gate → ``approve`` (Hermes' human-approval prompt, the analogue of
    Claude Code's ``permissionDecision: ask``) or ``block`` when configured.
    ``rule_key`` lets the ``[a]lways`` answer at the prompt allow that band
    from then on rather than every call individually.
    """
    worst = findings.worst
    if worst is None or settings.gate is None or worst.risk <= settings.gate:
        return None
    reason = (
        f"pyxray: risk {worst.risk} is over the gate of {settings.gate} "
        f"({BAND_WORDS.get(worst.band, worst.band)}) — {worst.event.get('synopsis') or 'see notes'}"
    )
    notes = _note_lines(worst.event)
    if notes:
        reason += "\n" + "\n".join(notes)
    if settings.mode == "block":
        return {"action": "block", "message": reason + "\n\nTo run it anyway, raise or unset the pyxray gate."}
    return {"action": "approve", "message": reason, "rule_key": f"pyxray:{worst.band}"}


def _note_lines(event: dict[str, Any], limit: int = 4) -> list[str]:
    lines: list[str] = []
    for note in (event.get("notes") or [])[:limit]:
        if not isinstance(note, dict):
            continue
        verb = str(note.get("verb") or note.get("effect") or "").strip()
        target = note.get("target")
        line = note.get("line")
        text = verb
        if target:
            text += f" {target}"
        if extra := note.get("note"):
            text += f" — {extra}"
        if line:
            text += f" (line {line})"
        lines.append(f"  · {text}")
    return lines


def wants_warning(findings: Findings, settings: Settings) -> bool:
    worst = findings.worst
    if worst is None or settings.warn_from == "never":
        return False
    floor = BANDS.index(settings.warn_from) if settings.warn_from in BANDS else BANDS.index("check")
    band = BANDS.index(worst.band) if worst.band in BANDS else 0
    return band >= floor


def format_block(findings: Findings, settings: Settings) -> str:
    """The Markdown that rides back to the model under the tool result.

    Deliberately short: one verdict line, the named effects, and a sentence
    telling the model what to do with it. The full picture is in the feed.
    """
    worst = findings.worst
    if worst is None:
        return ""
    header = (
        f"pyxray — {BAND_WORDS.get(worst.band, worst.band)} · risk {worst.risk}"
        + (f" · {worst.event.get('synopsis')}" if worst.event.get("synopsis") else "")
    )
    lines = ["", "---", header]
    if len(findings.snippets) > 1:
        lines.append(f"({len(findings.snippets)} snippets in this call; worst shown: {worst.label})")
    lines += _note_lines(worst.event)
    if worst.band == "read":
        lines.append(
            "This already ran. If any of those effects was not what the user asked for, "
            "say so now and do not repeat it; otherwise carry on."
        )
    elif settings.gate is not None:
        lines.append(f"Under the gate of {settings.gate}; recorded for the operator.")
    return "\n".join(lines)


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
        self._by_key: OrderedDict[str, Findings] = OrderedDict()
        self._order: list[Findings] = []
        self._keep = keep
        self._lock = threading.Lock()

    def remember(self, tool_name: str, args: Any, findings: Findings, tool_call_id: str = "") -> None:
        with self._lock:
            self._by_key[_key(tool_name, args, tool_call_id)] = findings
            while len(self._by_key) > self._keep:
                self._by_key.popitem(last=False)
            if not findings.empty:
                self._order.append(findings)
                del self._order[: -self._keep]

    def take(self, tool_name: str, args: Any, tool_call_id: str = "") -> Findings | None:
        with self._lock:
            found = self._by_key.pop(_key(tool_name, args, tool_call_id), None)
            if found is None and not tool_call_id:
                return None
            if found is None:
                # The id was not known at pre_tool_call time; fall back to the args.
                found = self._by_key.pop(_key(tool_name, args), None)
            return found

    def last(self, n: int = 1) -> list[Findings]:
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


def write_card(findings: Findings, settings: Settings, *, now_ms: int | None = None) -> Path | None:
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
