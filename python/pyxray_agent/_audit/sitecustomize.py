"""pyxray's runtime audit: what a run actually did, next to what it was
predicted to do.

This file is only ever loaded because a pyxray wrapper (the ``pyx-shim``
executable or ``python -m pyxray_agent.intercept``) was asked to verify a run
(``PYXRAY_AUDIT=1``) and put this directory first on the child's
``PYTHONPATH``. It installs a PEP 578 audit hook that counts the effects the
interpreter reports — files opened for reading or writing, deletions,
processes started, network connections, dynamic code — and appends one
``observed`` line to the pyxray feed when the process exits, carrying the
same run id as the predicted event.

It is verification, not a sandbox. It observes and never blocks; ``ctypes``,
direct system calls and children that scrub their environment get past it;
and environment reads raise no audit event at all.

Rules it keeps: an exception inside an audit hook would abort the operation
being audited, so every line of the hook is guarded; its own feed write is
not recorded; the interpreter's own imports are not counted as file reads;
and any ``sitecustomize`` of your own still runs afterwards.
"""

import sys


def _install():
    import os

    run = os.environ.get("PYXRAY_RUN")
    log = os.environ.get("PYXRAY_AUDIT_LOG")
    if not run or not log or getattr(sys, "_pyxray_audit", False):
        return
    sys._pyxray_audit = True

    import atexit
    import json
    import threading
    import time

    MAX_ITEMS = 200
    WATCH = frozenset({
        "open",
        "os.remove", "os.rmdir", "shutil.rmtree",
        "os.rename", "shutil.move", "shutil.copyfile", "shutil.copytree", "os.mkdir",
        "os.chmod", "os.chown", "os.truncate", "os.symlink", "os.link",
        "os.system", "subprocess.Popen", "os.exec", "os.posix_spawn", "os.spawn",
        "os.startfile", "pty.spawn", "os.fork",
        "socket.connect", "socket.sendto", "socket.getaddrinfo", "urllib.Request",
        "http.client.connect", "smtplib.connect", "ftplib.connect",
        "exec", "compile", "pickle.find_class", "ctypes.dlopen",
        "os.putenv", "os.unsetenv",
    })
    WRITE_FLAGS = 0
    for flag in ("O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC", "O_APPEND"):
        WRITE_FLAGS |= getattr(os, flag, 0)

    here = os.path.dirname(os.path.abspath(__file__))
    ignored_roots = set()
    for p in (sys.prefix, sys.base_prefix, sys.exec_prefix, getattr(sys, "base_exec_prefix", ""), here):
        if p:
            ignored_roots.add(os.path.normcase(os.path.abspath(p)))
    try:
        import site

        for p in site.getsitepackages() + [site.getusersitepackages()]:
            ignored_roots.add(os.path.normcase(os.path.abspath(p)))
    except Exception:
        pass
    ignored_roots = tuple(sorted(ignored_roots))
    code_suffixes = (".py", ".pyc", ".pyd", ".so", ".dylib", ".dll", ".pth")
    log_path = os.path.normcase(os.path.abspath(log))

    items = {}
    state = {"truncated": False, "flushed": False}
    local = threading.local()

    def clean(target):
        # Paths can hold bytes that are not UTF-8 (surrogate-escaped by
        # fsdecode); the feed is UTF-8 JSON, so they are spelled out instead.
        if target is None:
            return None
        text = str(target)[:200]
        return text.encode("utf-8", "backslashreplace").decode("utf-8")

    def note(effect, verb, target=None):
        key = (effect, verb, clean(target))
        if key in items:
            items[key] += 1
        elif len(items) < MAX_ITEMS:
            items[key] = 1
        else:
            state["truncated"] = True

    def is_ours(path, writing=False):
        # The feed itself, and the interpreter's own files. Reading a .py or
        # a .pyc is what importing looks like; writing one is not, and counts.
        p = os.path.normcase(os.path.abspath(path))
        if p == log_path or p.startswith(ignored_roots):
            return True
        return not writing and p.endswith(code_suffixes)

    def fspath(value):
        if isinstance(value, int):
            return None
        try:
            return os.fsdecode(value)
        except Exception:
            return None

    def record(event, args):
        if event == "open":
            path = fspath(args[0])
            if path is None:
                return
            mode, flags = args[1], args[2] if len(args) > 2 else 0
            writing = bool(mode and any(c in mode for c in "wax+")) or bool((flags or 0) & WRITE_FLAGS)
            if is_ours(path, writing):
                return
            note("fs_write" if writing else "fs_read", "write" if writing else "read", path)
        elif event in ("os.remove", "os.rmdir", "shutil.rmtree"):
            note("fs_delete", event.split(".")[-1], fspath(args[0]))
        elif event in ("os.rename", "shutil.move", "shutil.copyfile", "shutil.copytree", "os.symlink", "os.link"):
            note("fs_write", event.split(".")[-1], fspath(args[1]))
        elif event in ("os.mkdir", "os.chmod", "os.chown", "os.truncate"):
            note("fs_write", event.split(".")[-1], fspath(args[0]))
        elif event == "os.system":
            note("process", "shell", fspath(args[0]))
        elif event == "subprocess.Popen":
            cmd = args[1]
            if isinstance(cmd, (list, tuple)):
                cmd = " ".join(fspath(c) or "?" for c in cmd)
            note("process", "spawn", fspath(cmd) if not isinstance(cmd, str) else cmd)
        elif event in ("os.exec", "os.posix_spawn", "os.spawn", "os.startfile", "pty.spawn"):
            note("process", event.split(".")[-1], fspath(args[0]))
            if event == "os.exec":
                flush()  # the process is about to be replaced; atexit will not run
        elif event == "os.fork":
            note("process", "fork")
        elif event == "socket.connect" or event == "socket.sendto":
            address = args[1] if event == "socket.connect" else args[-1]
            if isinstance(address, tuple) and len(address) >= 2:
                note("net", "connect", f"{address[0]}:{address[1]}")
            elif isinstance(address, (str, bytes)):
                note("net", "connect", fspath(address))
        elif event == "socket.getaddrinfo":
            host = args[0]
            if host not in (None, "", "localhost", b"localhost"):
                note("net", "resolve", fspath(host) if isinstance(host, bytes) else host)
        elif event == "urllib.Request":
            note("net", (args[3] or "GET") if len(args) > 3 else "GET", args[0])
        elif event in ("http.client.connect", "smtplib.connect", "ftplib.connect"):
            note("net", "connect", f"{args[1]}:{args[2]}" if len(args) > 2 else None)
        elif event == "exec":
            code = args[0]
            filename = getattr(code, "co_filename", "") or ""
            # Importing a module execs its code object; that is not eval().
            if filename and not filename.startswith("<") and (is_ours(filename) or os.path.isfile(filename)):
                return
            note("dynamic", "exec", filename or None)
        elif event == "compile":
            filename = fspath(args[1]) if len(args) > 1 else None
            if filename and not filename.startswith("<"):
                return
            note("dynamic", "compile", filename)
        elif event == "pickle.find_class":
            note("dynamic", "unpickle", f"{args[0]}.{args[1]}")
        elif event == "ctypes.dlopen":
            note("dynamic", "dlopen", fspath(args[0]))
        elif event == "os.putenv":
            note("env", "setenv", fspath(args[0]))
        elif event == "os.unsetenv":
            note("env", "unsetenv", fspath(args[0]))

    def hook(event, args):
        if event not in WATCH or getattr(local, "busy", False):
            return
        local.busy = True
        try:
            record(event, args)
        except BaseException:
            pass
        finally:
            local.busy = False

    def flush():
        if state["flushed"]:
            return
        state["flushed"] = True
        local.busy = True
        try:
            line = json.dumps({
                "kind": "observed",
                "schema": 1,
                "ts": int(time.time() * 1000),
                "run": run,
                "pid": os.getpid(),
                "name": os.path.basename(sys.argv[0]) if sys.argv and sys.argv[0] else "<python>",
                "items": [
                    {"effect": e, "verb": v, **({"target": t} if t is not None else {}), "count": n}
                    for (e, v, t), n in items.items()
                ],
                "truncated": state["truncated"],
            }, ensure_ascii=False)
            os.makedirs(os.path.dirname(log) or ".", exist_ok=True)
            fd = os.open(log, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
            try:
                os.write(fd, (line + "\n").encode("utf-8"))
            finally:
                os.close(fd)
        except BaseException:
            pass
        finally:
            local.busy = False

    atexit.register(flush)
    sys.addaudithook(hook)


try:
    _install()
except BaseException:
    pass


def _chain():
    """Run the ``sitecustomize`` this one stood in front of, if there is one."""
    import importlib
    import os

    here = os.path.dirname(os.path.abspath(__file__))
    saved = list(sys.path)
    this = sys.modules.pop("sitecustomize", None)
    try:
        sys.path[:] = [p for p in sys.path if os.path.abspath(p or ".") != here]
        importlib.import_module("sitecustomize")
    except ImportError:
        pass
    except BaseException:
        pass
    finally:
        sys.path[:] = saved
        if this is not None and "sitecustomize" not in sys.modules:
            sys.modules["sitecustomize"] = this


_chain()
