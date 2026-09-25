"""``python -m pyxray_agent`` — a small CLI for when the ``pyx`` binary is not on PATH.

It covers rendering and JSON output. The feed, the watcher, the TUI, the
gate and the contact sheet live in ``pyx`` (``pip install pyxray-agent`` ships it as
a console script; a source checkout builds it with ``cargo build``)."""

from __future__ import annotations

import argparse
import sys

from . import analyze, backend, layouts, render, themes
from . import extract as extract_python


def main(argv: list[str] | None = None) -> int:
    args_in = list(sys.argv[1:] if argv is None else argv)
    if args_in[:1] == ["shim"]:
        from .shim import main as shim_main

        return shim_main(args_in[1:])
    parser = argparse.ArgumentParser(
        prog="python -m pyxray_agent",
        description="See what a Python snippet will do before you run it.",
    )
    parser.add_argument("file", nargs="?", help="Python file; omit to read stdin")
    parser.add_argument("-t", "--theme", default="blueprint")
    parser.add_argument("-l", "--layout", default="dashboard")
    parser.add_argument("-f", "--format", default="ansi",
                        choices=["ansi", "text", "html", "svg", "json"])
    parser.add_argument("-w", "--width", type=int, default=100)
    parser.add_argument("-i", "--icons", default="both", choices=["glyph", "tag", "both"])
    parser.add_argument("--depth", type=int, default=6)
    parser.add_argument("--code", action="store_true", help="show the source too")
    parser.add_argument("--command", action="store_true",
                        help="treat the input as a shell command and find the Python in it")
    parser.add_argument("--list", action="store_true", help="list themes and layouts")
    parser.epilog = "pyxray-agent shim install|uninstall|where — the python.exe shim; see --help there"
    args = parser.parse_args(args_in)

    if args.list:
        print(f"backend: {backend()}\n")
        print("themes:")
        for ident, _, blurb in themes():
            print(f"  {ident:<10} {blurb}")
        print("\nlayouts:")
        for ident, blurb in layouts():
            print(f"  {ident:<10} {blurb}")
        return 0

    if args.file:
        with open(args.file, encoding="utf-8") as fh:
            source, name = fh.read(), args.file
    else:
        source, name = sys.stdin.read(), "<stdin>"
    if args.command:
        source, name = extract_python(source)

    if args.format == "json":
        import json

        json.dump(analyze(source, name), sys.stdout, indent=2)
        print()
        return 0

    sys.stdout.write(
        render(
            source,
            name=name,
            theme=args.theme,
            layout=args.layout,
            format=args.format,
            width=args.width,
            icons=args.icons,
            depth=args.depth,
            code=args.code,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
