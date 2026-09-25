# pyxray

**See what a Python snippet will do before you run it.**

Language models hand you Python constantly — `python3 <<'EOF' … EOF`, a `-c`
one-liner, a file to run. Reading forty lines of someone else's code to work
out whether it touches your disk is exactly the tax that makes people stop
reading and just run it.

`pyxray` parses the snippet, works out what it actually does, and draws that:

```
┌────────────────────────────────────────────────────────────────────────────────────┐
│ pyxray  sketchy.py               21 lines · 16 statements · depth 2 · cyclomatic 2 │
│ reads the environment → makes 2 network calls → DELETES /tmp/stage → evaluates co… │
│ risk ██████████████████  100  read it first                      ·▸× ≈»§ ‡ ···· ¶· │
└────────────────────────────────────────────────────────────────────────────────────┘
┌─ C A P A B I L I T I E S ──────────────────────────────────────────────────────────┐
│  ≈ net 2!   § env 2!   » shell 2!   × delete 1!   ‡ eval 1!   ▸ write 3   ¶ print 1│
└────────────────────────────────────────────────────────────────────────────────────┘
┌─ W H A T   I T   D O E S ──────────────────────────────────────────────────────────┐
│    4 ! § getenv   API_TOKEN                  ! reads a secret from the environment │
│    7 ! × rmtree   /tmp/stage             ! recursively deletes a directory; ignor… │
│   10 ! ≈ GET      https://example.invali…! verify=False — TLS certificates are no… │
│   12 ! ‡ unpickle pickle.loads                    ! unpickling runs arbitrary code │
│   17 ! » run      f"bash {STAGING}/{name…! runs an external command; shell=True —… │
│   20 ! » run      ["rm", "-rf", STAGING] ! runs an external command; recursive de… │
└────────────────────────────────────────────────────────────────────────────────────┘
```

No execution, no sandbox, no imports resolved — it is a static read of the AST,
so it costs about two milliseconds and cannot itself do any of the things it
warns you about.

---

## Install

Linux, macOS and Windows.

```sh
pip install pyxray-agent    # the package, the interceptor, and the pyx binary
cargo install pyx-cli       # just the binary, from source
```

The wheels carry a `pyx` for their platform; `pip install` is the whole
setup. The distribution is `pyxray-agent` and the module `pyxray_agent`:
the PyPI project called `pyxray` is an unrelated X-ray physics library, so
installing that name gets you the wrong thing. If you wired a hook up before the
rename, run its installer again (or point `PYXRAY_ROOT` at a fresh checkout)
so it imports the new name. From a checkout, with a Rust toolchain (1.96+, for the Ruff parser —
`rust-toolchain.toml` pins it) and Python 3.9+:

```sh
make release        # builds target/release/pyx
make install-ext    # builds the extension into your Python (maturin develop)
```

If `$HOME` is small or quota'd — cargo's registry plus a release build runs to
a couple of gigabytes — put them somewhere else and `source env.sh`:

```sh
export PYXRAY_BUILD_ROOT=/scratch/you/pyxray
source env.sh
```

`env.sh` also sources `env.local.sh` if you have one, which is gitignored.

On Windows: the hook (below) is the route that catches an agent; the
`.cmd` and PowerShell shims are for what you type yourself, because
`CreateProcess("python3")` does not resolve a `.cmd`. The feed lives under
`%LOCALAPPDATA%\pyxray\`. Use Windows Terminal (or `chcp 65001`) for the
glyphs, or `-t mono` for pure ASCII.

## Use it

```sh
pyx script.py                       # the dashboard
pyx --layout card script.py         # the glance
cat script.py | pyx                 # from a pipe
pyx --tui script.py                 # interactive; cycle themes and layouts live
pyx --format json script.py | jq    # the whole report as data
pyx --format svg -o x.svg script.py # for a document
pyx --check 40 script.py            # exit 3 above a risk of 40; nothing runs
NO_COLOR=1 pyx script.py            # honoured, as is --color always|never
```

Everything is themeable and every visual axis is a flag:

```sh
pyx --list                          # themes, layouts, formats, feed path
pyx --list --format json            # the same with every palette colour
pyx -t carbon -l flow -i glyph script.py
pyx --contact-sheet sheet.html script.py   # every combination, one page
```

And the piece the hooks are built on is a command of its own:

```sh
echo 'cd build && python3 gen.py --fast' | pyx extract --cwd .
# [{"source": "…the contents of build/gen.py…", "label": "gen.py", …}]
```

### Watching an agent

An agent fires snippets several times a second, and that changes what the tool
has to be. Two things follow.

**The render is not for the model.** Whatever a hook or wrapper writes to
stdout or stderr is, in most harnesses, captured and fed back as tool output —
so drawing a report per snippet costs tokens, confuses the model, and scrolls
past you anyway. So by default pyxray appends one JSON line to a feed log and
draws nothing. You watch the log:

```sh
pyx watch                    # live column, in another pane
pyx watch --floor check      # only what wanted a second look
pyx watch --dump             # render it once and exit; scriptable
```

```
 pyxray watch   214 snippets  ·  38/min  ·  6 flagged            feed.jsonl
  risk ▁▁▂▁▁▁▂▁▁█▂▁▁▁▂▂▁▁▁▁▂▁
   seen        ▆▄▁ ▂▁▃ ▁ █▂▁▁ █▁  fs · world · eval · work · out
 ─────────────────────────────────────────────────────────────────────────
   12s █     1 ··· ··· · ···· ¶·  prints results                  <heredoc>
    9s █    30 ◂▸· ··§ · ∑··· ¶·  reads 2 files → writes 3 files  build.py
    4s ███ 100 ·▸× ≈»§ ‡ ···· ¶·  DELETES /tmp/stage → runs 2 cmd <heredoc>
                L7 rmtree /tmp/stage  recursively deletes a directory
    1s █    10 ··· ≈·· · ···· ··  calls https://pypi.org/…        <heredoc>
```

Every capability owns a permanent column, so the *pattern* is what you
recognise — the same profile always makes the same shape, and a snippet that
lights the delete slot is visible before a word is read. The header's histogram
sits in those same columns, so session totals line up with the rows underneath.

**Nothing is blocked unless you ask.** `PYXRAY_GATE` is unset by default and
pyxray only describes. Note that `PYXRAY_GATE=0` is not "no gate" — it refuses
anything with any effect at all. `off` is spelled out for that reason and is
the default.

The two defaults that matter, and why: interception uses `--layout auto`, so a
dull snippet gets one row and an alarming one blooms into the card and the
pane stays scannable through a burst; and the feed lands in
`$XDG_RUNTIME_DIR`, a tmpfs on any systemd machine, so a chatty session costs
no disk and the record dies with the login. Set `PYXRAY_LOG` if you want to
keep it.

### In front of a command you type

```sh
pyx --exec --confirm script.py      # show the X-ray, then ask
pyx --exec --gate 40 script.py      # refuse outright above a risk of 40
```

### As a PATH shim (works everywhere)

```sh
. integrations/shim/install.sh      # this shell
integrations/shim/install.sh --print >> ~/.bashrc
```

A real executable named `python3` (and `python`), earlier on PATH than the
real one. It is resolved by `execvp` — by any process, including the `sh -c`
a harness spawns, a Makefile, or a `subprocess.run` deep inside a library —
so it catches those. It does not catch `python3.12`, `uv run`, `poetry run`
or a virtualenv's own interpreter, which resolve to other files; the harness
hook, below, sees all of those because it reads the command rather than
standing in for the interpreter.

On Windows — and anywhere you would rather not have a shell script in the
way — the shim is a real executable instead:

```sh
pyxray-agent shim install     # python.exe + python3.exe in %LOCALAPPDATA%\pyxray\shim
```

`pyx-shim` reads its own command line as CPython does, analyses the script
in-process (its local imports included), writes the feed row, applies the
gate and any `.pyxray.toml`, then runs the real interpreter with the same
arguments, streams and exit code; 3 means pyxray refused. `CreateProcess`
finds it where it never finds a `.cmd`. It is installed, not activated: put
its directory first on the *agent's* PATH (the command prints how), so what
you run yourself is left alone.

`shell/pyxray.sh` also defines a `python3` shell *function*, which is fine for
what you type yourself but will not catch an agent: non-interactive shells
never read your rc file, and many harnesses use `/bin/sh` rather than bash.

### As a harness hook

Per-harness setup lives in `integrations/`: Claude Code, Hermes, Codex CLI,
Gemini CLI, Cursor, OpenCode, and a tmux pane recipe. Codex, Gemini and Cursor
install with `python -m pyxray_agent.install codex|gemini|cursor --apply`. Hermes gets a native plugin (`hermes plugins install
adukhan99/pyxray/integrations/hermes`) with a pane in Hermes Desktop and the
verdict appended to each tool result; the others end up calling the same
thing —

```sh
python3 -m pyxray_agent.intercept --hook
```

— which reads a `PreToolUse`-shaped payload on stdin, finds the Python in it,
records it, and asks the harness to stop only if you set a gate. "Finds the
Python" means reading the command the way a shell would: `python3 script.py`
(the file is read, relative to the call's `cwd`), `cd build && python3 …`,
`env`/`sudo`/`nohup`/`VAR=1` in front, `python3.12`, `uv run`, `poetry run`,
a heredoc, `-c`, `bash -c "…"`, `cat gen.py | python3`, and every piece of a
`;`/`&&` chain. The 75 cases in `tests/fixtures/extract_cases.json` are the
contract, and the hook never fails the tool call: a missing engine or a bad
payload exits 0 silently (`PYXRAY_DEBUG=1` says why).

The Claude Code plugin is the same for the terminal and the desktop app:

```
/plugin marketplace add adukhan99/pyxray
/plugin install pyxray@pyxray
```

### From Python

```python
import pyxray_agent as pyxray

report = pyxray.analyze(source)
report["meta"]["synopsis"]      # 'reads 2 files → computes with pandas → writes 3 files'
report["metrics"]["risk"]       # 22
report["effects"][0]            # {'effect': 'fs_read', 'verb': 'read', 'target': 'data/in.csv', …}
report["schema"]                # 1 — the JSON contract is versioned

print(pyxray.render(source, theme="carbon", layout="card", width=100))

pyxray.extract_all("cd build && python3 gen.py")   # every snippet a command runs
```

---

## What it looks for

Effects are grouped into thirteen capabilities, each with a severity:

| | capability | flagged as caution when |
|---|---|---|
| `◂` | reads files | — |
| `▸` | writes files | — |
| `×` | deletes files | always |
| `≈` | network | POST/PUT/DELETE, `verify=False`, downloads to disk |
| `»` | runs commands | always; worse with `shell=True` |
| `§` | environment | reading a secret-looking name (`*_TOKEN`, `*_SECRET`, `*_KEY`, `PASSWORD`…), `getpass`; writes to `os.environ` and `sys.path` are notable |
| `‡` | dynamic code | `eval`, `exec`, `pickle.loads`, `torch.load`, `yaml.load`, `numpy.load(allow_pickle=True)`, `runpy`, `ctypes` |
| `¶` | prints | — |
| `?` | randomness | — |
| `○` | time | — |
| `≡` | concurrency | — |
| `∑` | compute | — |
| `■` | exits | `os._exit` |

Resolution is more than name matching. Import aliases are unwound (`np.load` →
`numpy.load`, `from os import *` then `system(…)` → `os.system`, `f =
os.system` then `f(…)`, `getattr(os, "remove")`, `__import__("os").system`);
values are tracked well enough to resolve methods on them (`fh = open(p)` …
`fh.write(x)` is a write, and it names `p` as the target; a socket, a database
connection, a tar archive or a `requests.Session` keep reporting after they
are made); string constants are folded so `rmtree(STAGE)` shows `/tmp/stage`
rather than `STAGE`, and a constant inside one function is not mistaken for a
module-level one; `Path('out') / 'x.json'` stays a path; a function you defined
called `open` shadows the builtin in its own scope and nowhere else (a method
named `eval` somewhere in a class does not switch the check off for the whole
file); and shell fragments inside a call's own arguments (`rm -rf`, `curl … |
sh`, `sudo`) escalate whatever they are attached to — but only things that
touch the world, so `print("run: pip install foo")` stays inert.

The risk score is driven by severity and by *combinations* — fetching and then
executing scores higher than doing either twice. `pyxray_agent.analyze` and
`pyx --format json` carry the terms it is the sum of, each with its reason.

**Secrets are followed, not just noticed.** A secret-named environment
variable, the keyring, or a credential file (`~/.ssh/*`, `~/.aws/credentials`,
`.env`, `.netrc`, `.pypirc`, `.kube/config`…) read into a name is tracked
through f-strings, dicts, `headers["Authorization"] = tok`, `base64`,
`.encode()` and the snippet's own helpers, until it reaches a call. On a
network call that is an **exfiltration** flow — Caution and +25 for a key
file, Notable and +10 for an env token (usually going to its own API; a
policy can say otherwise); into a command line or onto disk, a flow of its
own. What an authenticated request *returns* is not the token, and
`len(tok)` is a number.

**Local imports are followed.** Given the script's path (a hook reading
`python3 main.py`, the shim, `pyx main.py`), the modules it imports from its
own directory are analysed too — package `__init__`s, `from pkg import mod`,
relative imports inside packages — and what the snippet actually reaches
counts: module-level code, and the functions and classes it uses. A helper
nobody calls is counted but not scored. Nothing outside the script's
directory is read, symlinks included; `--no-follow` or `PYXRAY_FOLLOW=0` turn
it off.

**A script that changes between runs says what changed.** Each event carries
the script's path and a digest; when a new version can do something the last
recorded run of that path could not, the event says `+net`, the new target
and the change in risk, `pyx watch` shows it beside the name, and the harness
note tells the model.

### Policy

One number is a blunt instrument. `.pyxray.toml` says, per effect and per
path, host, command or variable, what may run, what must be asked about and
what must not run:

```toml
version = 1
default = "allow"            # when no rule matches

[[rule]]                     # first match wins
effect = "fs_write"
paths = ["{repo}/**", "/tmp/**"]
decision = "allow"

[[rule]]
effect = "fs_write"
decision = "ask"

[[rule]]
effect = "net"
hosts = ["pypi.org", "*.pythonhosted.org", "api.github.com"]
decision = "allow"

[[rule]]
effect = "net"
decision = "ask"

[[rule]]
flow = "exfiltration"
decision = "deny"
reason = "no secrets leave this machine"
```

It applies to every harness, the shim and `pyx`: Claude Code and Cursor get
"ask" as a prompt, Hermes as its approval dialog; Codex and Gemini, which
cannot ask, refuse with the reason. Two rules keep it honest. A constrained
rule matches only a target the analyser knows exactly — a URL assembled at
runtime never matches an allow-list, it falls through. And the repository's
`.pyxray.toml` (the agent can write that file) can only make your own policy
stricter: yours lives at `~/.config/pyxray/policy.toml`
(`%APPDATA%\pyxray\policy.toml` on Windows) and wins ties unless it says
`trust_repo_policy = true`. `PYXRAY_POLICY=path` uses one file instead,
`PYXRAY_POLICY=off` none; `pyx policy [FILE]` shows which files apply and the
verdict for a snippet. A policy that fails to parse asks rather than allowing
everything or blocking everything.

The **band** is what the compact renders show, and it asks severity first, not
the score:

| band | when | reads as |
|---|---|---|
| inert | nothing notable | one faint cell |
| routine | notable work, any amount of it (or a score of 15+) | one green cell |
| check it | anything at caution severity, or notable work scoring 45+ | two amber cells |
| read it first | caution, and a score at 60 or above | three red cells |

Severity leads deliberately. Volume must not be able to impersonate danger: a
script that writes three files is doing ordinary work and stays green at a
score of 30, while one `shutil.rmtree` is worth stopping for at the same score;
a hundred `print`s add at most 12 to the score and stay inert. Colour,
cell-count and the word beside them all come from the same band, so they
cannot disagree, and the band survives a log file, a screenshot, and a reader
who does not see the hue.

## What it does not do

- It does not run anything, so anything decided at runtime is invisible to it.
  `getattr(os, "rem" + "ove")` will not be caught.
- It follows local imports from the script's own directory, not installed
  packages: a call into a library is judged by the library's name, not its
  code.
- It cannot analyse what it never sees. A hook reads the command an agent
  runs; `python3 -m somemodule` names code that is not in the command, and a
  `curl … | python3` arrives as a pipe from the network. Both are recorded
  with an empty source and a label saying why, so the feed shows a row
  rather than nothing.
- A low score is not a safety claim. It is "nothing in this file's AST looks
  dangerous", which is a much smaller statement.
- Nesting deeper than a couple of hundred levels is skipped, with a warning
  in the report — and an internal failure yields an honest "could not
  analyse" report rather than a crash, since the analyser sits in front of
  somebody else's command.

Treat it as a way to read faster, not as a way to stop reading.

---

## The pieces

```
crates/pyxray-core     parse → analyse → Report (serde; the stable contract)
crates/pyxray-render   Report → themed ratatui buffer → ANSI / text / HTML / SVG
crates/pyx-cli         the `pyx` binary and the interactive viewer
crates/pyxray-py       pyo3 bindings
python/pyxray_agent    the Python package, the interceptor and the hook
```

`pyxray-core` has no idea how anything is drawn, and `pyxray-render` has no
idea what Python is. A new look is a `Theme` value; a new arrangement is a
`Layout` arm; a new output format is a function over a ratatui `Buffer`.
Finding the Python inside a shell command is `pyxray-core::extract`, the one
implementation the hook, the shim and `pyx extract` all use.

The JSON report and every feed line carry `"schema": 1`. A field added with a
default does not bump it; a renamed or repurposed field does, and a reader
skips feed lines newer than it understands. Capability bits are append-only,
because the feed stores the mask as an integer.

Layouts are *planned* before they are drawn — `layout::plan` returns the exact
rectangle each panel will occupy — so measuring for a file and fitting into a
live terminal are the same code path and cannot drift apart.

## Looks

Seven themes, six layouts (four for reading, `line` and `auto` for a stream),
three ways of labelling effects. `make sheet` renders every combination
against your own code so a look can be chosen by comparison:

| theme | |
|---|---|
| `blueprint` | Technical drawing. Navy ground, cyan hairlines, letterspaced headings. |
| `neon` | After dark. Near-black ground, saturated magenta and lime, heavy rules. |
| `paper` | Daylight. Warm paper ground, ink type, rules instead of boxes. |
| `carbon` | Product UI. Neutral greys, one blue accent, rounded frames, tidy. |
| `amber` | Phosphor CRT. One hue, brightness does the work, bracketed panels. |
| `ansi` | Sixteen colours only — emitted as the classic 16-colour codes, never RGB. Survives any terminal, tmux and SSH session. |
| `mono` | No colour and no Unicode at all: pure ASCII, structure carried entirely by glyphs and weight. |

Every role in every colour theme clears 3:1 against its own background, and
body text runs 11:1 to 17.6:1; `make audit` checks it and fails otherwise.

| layout | |
|---|---|
| `line` | One row: band, barcode, score, synopsis. What the feed is made of. |
| `auto` | `line` when dull, `card` when not. The interception default. |
| `card` | A glance: synopsis, risk, capabilities, effect timeline. |
| `dashboard` | Two columns: structure on the left, findings on the right. |
| `stack` | One column, everything, in reading order. |
| `flow` | The outline, given the room to breathe. |

`dashboard` falls back to `stack` below 96 columns rather than cramming.

## Numbers

| | |
|---|---|
| binary | 2.3 MB, no runtime dependencies |
| a 60-line snippet | ~5.4 ms end to end for `pyx`, of which ~3.2 ms is process startup; a hook adds one interpreter start |
| a 3600-line file | ~14 ms |
| tests | 99 Rust (48 detection, 75 shared extraction cases, golden snapshots of every example and every theme × layout), 26 Python on each backend, 9 shim scenarios; CI runs them on Linux, macOS and Windows |

## Licence

MIT.
