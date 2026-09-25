//! Argument parsing. Hand-rolled: the surface is small, the help text is the
//! part worth spending effort on, and it keeps the binary's dependency list
//! down to what actually draws pixels.

use pyxray_core::model::Band;
use pyxray_render::export::{self, Format};
use pyxray_render::layout::{self, Layout};
use pyxray_render::panels::{Icons, Opts};
use pyxray_render::theme::{self, Theme};

pub const HELP: &str = "\
pyx — see what a Python snippet will do before you run it

USAGE
    pyx [OPTIONS] [FILE]
    cat script.py | pyx
    pyx --tui script.py
    pyx watch                 live feed of everything analysed elsewhere
    pyx extract               read a shell command on stdin, print the Python in it

INPUT
    FILE                Python file to read. Omit (or use -) to read stdin.
    --name NAME         what to call stdin input in the report (default: <stdin>)
    --stdin-is-command  treat stdin as a shell command and pull the Python out
                        of it (python3 script.py, python3 <<'EOF' … EOF,
                        python -c '…', cd x && python3 …, bash -c '…')

LOOK
    -t, --theme ID      blueprint | neon | paper | carbon | amber | ansi | mono
    -l, --layout ID     line | auto | card | dashboard | stack | flow
                        auto is one row when it is dull, the card when it is
                        not — the right default when a harness is firing
                        several a second
    -i, --icons MODE    glyph | tag | both            (default: both)
        --depth N       fold the outline below depth N (default: 6)
        --code          show the source alongside the analysis
        --no-gutter     drop line numbers from the outline

OUTPUT
    -f, --format FMT    ansi | text | html | svg | json   (default: ansi)
        --json          the whole report as JSON; same as -f json
        --color WHEN    auto | always | never          (default: auto)
                        auto colours a terminal and not a pipe or file, and
                        honours NO_COLOR and CLICOLOR_FORCE
        --fragment      with --format html, emit the bare <pre> for embedding
    -w, --width N       columns (default: terminal width, else 100)
        --height N      rows; only meaningful for a fixed-size render
    -o, --out PATH      write to a file instead of stdout
        --tui           interactive viewer; cycle themes and layouts live
        --contact-sheet PATH
                        render every theme and layout to one HTML page
        --check N       exit 3 when the risk score is above N, after drawing.
                        For scripts and CI; nothing is run.

FEED
        --emit          append this analysis to the feed log as one JSON line
        --feed PATH     which log (default: $PYXRAY_LOG, else the per-user
                        runtime directory; `pyx --list` prints it)
        --source NAME   label the event with where it came from
        --print-event   print the event JSON as the first line of output
                        (with or without --emit; only --emit writes the log)
        --run ID        an execution id to carry on the event
        --origin PATH   the script's path, when its source comes on stdin
        --cwd DIR       where a snippet with no file would run (for imports)
        --no-follow     do not analyse the local modules it imports
                        (also PYXRAY_FOLLOW=0)
        --quiet         emit only; draw nothing

    pyx watch [PATH]    follow the log. Keys: q quit, p pause, f filter,
                        n notes, t theme, c clear.
        --dump          render the log once to stdout and exit
        --replay        start from the whole log, not just what arrives next
        --floor BAND    hide below inert | routine | check | read

POLICY
    pyx policy [FILE]   which policy files apply (PYXRAY_POLICY, your user
                        file, the repository's .pyxray.toml), whether they
                        parse, and — given FILE or a snippet on stdin — the
                        verdict for it
        --cwd DIR       look for the repository policy from DIR

EXTRACT
    pyx extract         read a shell command on stdin; print a JSON array of
                        {source, label, path, segment} for every Python in it
        --cwd DIR       resolve script paths against DIR
        --no-files      report script paths without reading them
        --argv -- ARGS  instead: classify an interpreter's own arguments
                        ({code, module, script, stdin})

RUN
        --exec          run the snippet with python after showing the X-ray
        --confirm       with --exec, ask before running
        --gate N        with --exec, refuse to run above this risk score.
                        Off unless you set it: pyxray describes by default and
                        blocks only on request. Note that --gate 0 blocks
                        everything with any effect at all; --gate off is the
                        default and is accepted explicitly.

OTHER
        --list          list themes, layouts, formats and the feed path
                        (--format json for a machine-readable version)
    -h, --help          this text
    -V, --version       version

EXIT CODES
    0 fine · 2 usage or I/O error · 3 refused by --gate, --confirm or --check
    · with --exec, the snippet's own exit code
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Xray,
    Watch,
    Extract,
    Policy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
pub struct Args {
    pub mode: Mode,
    pub file: Option<String>,
    pub name: Option<String>,
    pub stdin_is_command: bool,
    pub theme: Theme,
    pub layout: Layout,
    pub format: Format,
    pub json: bool,
    pub color: ColorMode,
    pub fragment: bool,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub out: Option<String>,
    pub tui: bool,
    pub contact_sheet: Option<String>,
    pub list: bool,
    pub check: Option<u8>,
    pub opts: Opts,
    pub exec: bool,
    pub confirm: bool,
    pub gate: Option<u8>,
    pub emit: bool,
    pub feed: Option<String>,
    pub source: String,
    pub print_event: bool,
    /// An execution id to carry on the event (`--run`).
    pub run: Option<String>,
    /// The script's path, when the source arrives on stdin (`--origin`).
    pub origin: Option<String>,
    /// Follow the snippet's local imports (off with `--no-follow`).
    pub follow: bool,
    pub quiet: bool,
    pub replay: bool,
    pub dump: bool,
    pub floor: Band,
    pub cwd: Option<String>,
    pub read_files: bool,
    /// `pyx extract --argv -- …`: the interpreter arguments to classify.
    pub argv: Option<Vec<String>>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            mode: Mode::Xray,
            file: None,
            name: None,
            stdin_is_command: false,
            theme: theme::default_theme(),
            layout: Layout::Dashboard,
            format: Format::Ansi,
            json: false,
            color: ColorMode::Auto,
            fragment: false,
            width: None,
            height: None,
            out: None,
            tui: false,
            contact_sheet: None,
            list: false,
            check: None,
            opts: Opts::default(),
            exec: false,
            confirm: false,
            gate: None,
            emit: false,
            feed: None,
            source: "cli".to_string(),
            print_event: false,
            run: None,
            origin: None,
            follow: true,
            quiet: false,
            replay: false,
            dump: false,
            floor: Band::Inert,
            cwd: None,
            read_files: true,
            argv: None,
        }
    }
}

pub enum Parsed {
    Run(Box<Args>),
    Help,
    Version,
}

/// Which modes a flag makes sense in. Anything else is a mistake worth
/// saying out loud rather than a flag to ignore quietly.
fn allowed(flag: &str) -> &'static [Mode] {
    use Mode::*;
    match flag {
        "--dump" | "--replay" | "--floor" => &[Watch],
        "--no-files" | "--argv" => &[Extract],
        "--cwd" => &[Extract, Xray, Policy],
        "--exec" | "--confirm" | "--gate" | "--emit" | "--feed" | "--source" | "--print-event"
        | "--run" | "--origin" | "--no-follow" | "--quiet" | "--tui" | "--contact-sheet"
        | "--check" | "--json" | "--stdin-is-command" | "--name" | "--code" | "--no-gutter"
        | "--depth" | "-l" | "--layout" => &[Xray],
        _ => &[Xray, Watch, Extract, Policy],
    }
}

pub fn parse<I: Iterator<Item = String>>(mut it: I) -> Result<Parsed, String> {
    let mut a = Args::default();
    let mut explicit_layout = false;
    let mut first = true;
    let mut seen: Vec<&'static str> = Vec::new();
    while let Some(arg) = it.next() {
        if first {
            first = false;
            match arg.as_str() {
                "watch" => {
                    a.mode = Mode::Watch;
                    continue;
                }
                "extract" => {
                    a.mode = Mode::Extract;
                    continue;
                }
                "policy" => {
                    a.mode = Mode::Policy;
                    continue;
                }
                _ => {}
            }
        }
        let mut value = |name: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{name} needs a value"))
        };
        // Remember the canonical spelling of every flag, for the mode check.
        let flag: &'static str = match arg.as_str() {
            "-t" => "--theme",
            "-l" => "--layout",
            "-i" => "--icons",
            "-f" => "--format",
            "-w" => "--width",
            "-o" => "--out",
            s if s.starts_with("--emit=") => "--emit",
            s if s.starts_with("--") => {
                // Leak-free: the set of flags is fixed, so map to the static.
                canonical(s).unwrap_or("--unknown")
            }
            _ => "",
        };
        if !flag.is_empty() && flag != "--unknown" {
            seen.push(flag);
        }
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--" => {
                if a.argv.is_some() {
                    a.argv = Some(it.collect());
                    break;
                }
                // Everything after `--` is the file, even one called `-t`.
                if let Some(f) = it.next() {
                    set_file(&mut a, f)?;
                }
                if it.next().is_some() {
                    return Err("only one FILE can be analysed at a time".into());
                }
                break;
            }
            "--list" => a.list = true,
            "--tui" => a.tui = true,
            "--code" => a.opts.code = true,
            "--no-gutter" => a.opts.gutter = false,
            "--stdin-is-command" => a.stdin_is_command = true,
            "--quiet" => a.quiet = true,
            "--replay" => a.replay = true,
            "--dump" => a.dump = true,
            "--emit" => a.emit = true,
            _ if arg.starts_with("--emit=") => {
                a.emit = true;
                a.feed = Some(arg["--emit=".len()..].to_string());
            }
            "--feed" => {
                a.emit = true;
                a.feed = Some(value("--feed")?);
            }
            "--print-event" => a.print_event = true,
            "--run" => a.run = Some(value("--run")?),
            "--origin" => a.origin = Some(value("--origin")?),
            "--no-follow" => a.follow = false,
            "--source" => a.source = value("--source")?,
            "--name" => a.name = Some(value("--name")?),
            "--cwd" => a.cwd = Some(value("--cwd")?),
            "--no-files" => a.read_files = false,
            "--argv" => a.argv = Some(Vec::new()),
            "--floor" => {
                a.floor = match value("--floor")?.as_str() {
                    "inert" => Band::Inert,
                    "routine" => Band::Routine,
                    "check" => Band::Check,
                    "read" => Band::Read,
                    other => return Err(format!("unknown band {other:?}")),
                }
            }
            "--fragment" => a.fragment = true,
            "--exec" => a.exec = true,
            "--confirm" => a.confirm = true,
            "--json" => a.json = true,
            "--color" | "--colour" => {
                a.color = match value("--color")?.as_str() {
                    "auto" => ColorMode::Auto,
                    "always" | "yes" | "force" => ColorMode::Always,
                    "never" | "no" | "none" => ColorMode::Never,
                    other => return Err(format!("--color wants auto|always|never, got {other:?}")),
                }
            }
            "-t" | "--theme" => {
                let v = value("--theme")?;
                a.theme =
                    theme::theme(&v).ok_or_else(|| format!("unknown theme {v:?}; try --list"))?;
            }
            "-l" | "--layout" => {
                let v = value("--layout")?;
                a.layout = layout::layout(&v)
                    .ok_or_else(|| format!("unknown layout {v:?}; try --list"))?;
                explicit_layout = true;
            }
            "-i" | "--icons" => {
                a.opts.icons = match value("--icons")?.as_str() {
                    "glyph" => Icons::Glyph,
                    "tag" => Icons::Tag,
                    "both" => Icons::Both,
                    other => return Err(format!("unknown icon mode {other:?}")),
                };
            }
            "-f" | "--format" => {
                let v = value("--format")?;
                if v == "json" {
                    a.json = true;
                } else {
                    a.format = export::format(&v).ok_or_else(|| format!("unknown format {v:?}"))?;
                }
            }
            "-w" | "--width" => {
                a.width = Some(parse_num(&value("--width")?, "--width")?);
            }
            "--height" => {
                a.height = Some(parse_num(&value("--height")?, "--height")?);
            }
            "--depth" => {
                a.opts.max_depth = parse_num(&value("--depth")?, "--depth")?;
            }
            "--check" | "--fail-over" => {
                a.check = Some(parse_num::<u8>(&value("--check")?, "--check")?);
            }
            "--gate" => {
                // `off` is spelled out because `0` reads like "no gate" and
                // means the exact opposite: block anything with any effect.
                let raw = value("--gate")?;
                a.gate = match raw.as_str() {
                    "off" | "none" | "" => None,
                    other => Some(parse_num::<u8>(other, "--gate")?),
                };
            }
            "-o" | "--out" => a.out = Some(value("--out")?),
            "--contact-sheet" => a.contact_sheet = Some(value("--contact-sheet")?),
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other:?}; try --help"));
            }
            other => set_file(&mut a, other.to_string())?,
        }
    }

    for flag in &seen {
        let ok = allowed(flag);
        if !ok.contains(&a.mode) {
            let where_ = match ok {
                [Mode::Watch] => "pyx watch",
                [Mode::Extract] => "pyx extract",
                [Mode::Policy] => "pyx policy",
                _ => "plain pyx",
            };
            return Err(format!("{flag} only makes sense with {where_}"));
        }
    }
    if a.mode == Mode::Xray {
        if a.tui && (a.json || a.out.is_some() || a.contact_sheet.is_some()) {
            return Err("--tui cannot be combined with --json, --out or --contact-sheet".into());
        }
        if a.fragment && a.format != Format::Html {
            return Err("--fragment needs --format html".into());
        }
        if a.confirm && !a.exec {
            return Err("--confirm needs --exec".into());
        }
        if a.gate.is_some() && !a.exec {
            return Err("--gate needs --exec; to fail without running, use --check".into());
        }
    }
    if a.mode == Mode::Extract && a.file.is_some() {
        return Err("pyx extract reads the command from stdin; it takes no FILE".into());
    }
    // Defaults follow the situation. Somebody who ran `pyx file.py` is reading
    // a report; somebody about to execute wants a glance that gets out of the
    // way unless it shouldn't.
    if !explicit_layout && a.exec {
        a.layout = Layout::Auto;
    }
    Ok(Parsed::Run(Box::new(a)))
}

fn set_file(a: &mut Args, f: String) -> Result<(), String> {
    if a.file.is_some() {
        return Err(format!(
            "only one FILE can be analysed at a time (got {:?} and {f:?})",
            a.file.as_deref().unwrap_or("")
        ));
    }
    a.file = Some(f);
    Ok(())
}

fn canonical(flag: &str) -> Option<&'static str> {
    const ALL: &[&str] = &[
        "--help",
        "--version",
        "--list",
        "--tui",
        "--code",
        "--no-gutter",
        "--stdin-is-command",
        "--quiet",
        "--replay",
        "--dump",
        "--emit",
        "--feed",
        "--print-event",
        "--run",
        "--origin",
        "--no-follow",
        "--source",
        "--name",
        "--cwd",
        "--no-files",
        "--argv",
        "--floor",
        "--fragment",
        "--exec",
        "--confirm",
        "--json",
        "--color",
        "--colour",
        "--theme",
        "--layout",
        "--icons",
        "--format",
        "--width",
        "--height",
        "--depth",
        "--check",
        "--fail-over",
        "--gate",
        "--out",
        "--contact-sheet",
    ];
    ALL.iter().copied().find(|f| *f == flag)
}

fn parse_num<T: std::str::FromStr>(s: &str, name: &str) -> Result<T, String> {
    s.parse::<T>()
        .map_err(|_| format!("{name} needs a number, got {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Args, String> {
        match parse(args.iter().map(|s| s.to_string()))? {
            Parsed::Run(a) => Ok(*a),
            _ => Err("not a run".into()),
        }
    }

    #[test]
    fn a_second_positional_is_an_error_not_a_silent_overwrite() {
        let err = run(&["--emit", "/tmp/feed.jsonl", "script.py"]).unwrap_err();
        assert!(err.contains("only one FILE"), "{err}");
        let a = run(&["--emit=/tmp/feed.jsonl", "script.py"]).unwrap();
        assert!(a.emit);
        assert_eq!(a.feed.as_deref(), Some("/tmp/feed.jsonl"));
        assert_eq!(a.file.as_deref(), Some("script.py"));
        let a = run(&["--feed", "/tmp/f", "s.py"]).unwrap();
        assert_eq!(a.feed.as_deref(), Some("/tmp/f"));
    }

    #[test]
    fn a_file_named_like_a_flag_is_reachable_after_a_double_dash() {
        let a = run(&["--", "-t"]).unwrap();
        assert_eq!(a.file.as_deref(), Some("-t"));
    }

    #[test]
    fn watch_only_flags_are_refused_elsewhere_and_vice_versa() {
        assert!(run(&["--dump", "x.py"]).unwrap_err().contains("pyx watch"));
        assert!(run(&["watch", "--exec"]).unwrap_err().contains("plain pyx"));
        assert!(run(&["--no-files", "s.py"])
            .unwrap_err()
            .contains("pyx extract"));
        // --cwd is shared: where extract reads scripts, where analysis
        // follows imports from. Watch has no use for it.
        assert!(run(&["--cwd", "/x", "s.py"]).is_ok());
        assert!(run(&["watch", "--cwd", "/x"]).is_err());
        assert!(run(&["watch", "--dump", "--floor", "check"]).is_ok());
    }

    #[test]
    fn nonsense_combinations_are_errors() {
        assert!(run(&["--tui", "--json", "x.py"]).is_err());
        assert!(run(&["--fragment", "x.py"]).is_err());
        assert!(run(&["--gate", "40", "x.py"])
            .unwrap_err()
            .contains("--check"));
        assert!(run(&["--confirm", "x.py"]).is_err());
        assert!(
            run(&["--print-event", "x.py"]).is_ok(),
            "print without writing"
        );
        assert!(
            run(&["watch", "--run", "r1"]).is_err(),
            "--run is for analysis"
        );
        assert!(run(&["--exec", "--gate", "40", "x.py"]).is_ok());
    }

    #[test]
    fn run_and_origin_parse() {
        let a = run(&["--print-event", "--run", "r1", "--origin", "/w/job.py", "-"]).unwrap();
        assert_eq!(a.run.as_deref(), Some("r1"));
        assert_eq!(a.origin.as_deref(), Some("/w/job.py"));
        assert!(!a.emit);
    }

    #[test]
    fn the_new_flags_parse() {
        let a = run(&[
            "--color", "never", "--check", "40", "--name", "snippet", "-",
        ])
        .unwrap();
        assert_eq!(a.color, ColorMode::Never);
        assert_eq!(a.check, Some(40));
        assert_eq!(a.name.as_deref(), Some("snippet"));
        assert!(run(&["--color", "sometimes"]).is_err());
        let a = run(&["--json", "x.py"]).unwrap();
        assert!(a.json);
        let a = run(&["--list", "--format", "json"]).unwrap();
        assert!(a.list && a.json);
    }

    #[test]
    fn extract_mode_takes_its_own_flags() {
        let a = run(&["extract", "--cwd", "/w", "--no-files"]).unwrap();
        assert_eq!(a.mode, Mode::Extract);
        assert_eq!(a.cwd.as_deref(), Some("/w"));
        assert!(!a.read_files);
        let a = run(&["extract", "--argv", "--", "-X", "dev", "s.py"]).unwrap();
        assert_eq!(
            a.argv.as_deref(),
            Some(&["-X".to_string(), "dev".into(), "s.py".into()][..])
        );
        assert!(run(&["extract", "file.py"]).is_err());
    }

    #[test]
    fn exec_defaults_to_the_auto_layout() {
        assert_eq!(run(&["--exec", "x.py"]).unwrap().layout, Layout::Auto);
        assert_eq!(
            run(&["--exec", "-l", "card", "x.py"]).unwrap().layout,
            Layout::Card
        );
    }
}
