//! `pyx-shim` — a `python.exe` that looks before it leaps.
//!
//! Copied (or linked) as `python`, `python3` or `python3.12` into a directory
//! that comes first on an agent's PATH, it is what the agent's
//! `CreateProcess("python", …)` or `execvp("python3", …)` finds. It reads its
//! own command line the way CPython does, analyses the code it is about to run
//! — in-process, no Python needed for that — writes the feed row, applies
//! the gate and the policy, and then runs the real interpreter with the same
//! arguments, standard streams and exit code.
//!
//! On Windows this is the only way to put pyxray in front of an agent that
//! is not driven through hooks: `CreateProcess` never resolves the `.cmd`
//! shims, and the POSIX shim is a shell script. On Unix it does the same job
//! as that script in one process instead of two interpreter starts.
//!
//! Environment: `PYXRAY_OFF=1` passes straight through; `PYXRAY_PYTHON` names
//! the real interpreter; `PYXRAY_GATE`, `PYXRAY_CONFIRM=1`, `PYXRAY_POLICY`,
//! `PYXRAY_FOLLOW`, `PYXRAY_LOG` mean what they mean everywhere else.
//! Exit status 3 means pyxray refused to run it.

use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use pyxray_core::{extract, FollowOpts, RecordOpts, XrayOpts};

/// What pyxray exits with when it refused to run something, the same as the
/// Python wrapper and `pyx --exec`.
const REFUSED: u8 = 3;
/// Scripts larger than this are run but not read.
const MAX_SCRIPT: u64 = 1 << 20;

fn main() -> ExitCode {
    let own = std::env::current_exe().ok();
    let name = own
        .as_deref()
        .and_then(interpreter_name)
        .unwrap_or_else(|| "python".to_string());
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    let real = match find_real(
        &name,
        std::env::var_os("PYXRAY_PYTHON"),
        std::env::var_os("PATH"),
        own.as_deref().and_then(Path::parent),
    ) {
        Some(r) => r,
        None => {
            eprintln!("pyxray: no real {name} found on PATH (set PYXRAY_PYTHON)");
            return ExitCode::from(127);
        }
    };

    if std::env::var("PYXRAY_OFF").as_deref() == Ok("1") {
        return hand_over(&real, &args, None);
    }

    let text_args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let inv = extract::classify_argv(&text_args);

    // What will run, and where it came from.
    let mut stdin_buffer: Option<Vec<u8>> = None;
    let (source, label, origin) = if let Some(code) = inv.code.clone() {
        (code, "python -c".to_string(), None)
    } else if let Some(module) = &inv.module {
        // Not in the command, and not ours to go looking for.
        (String::new(), format!("python -m {module}"), None)
    } else if let Some(script) = &inv.script {
        let path = PathBuf::from(script);
        let text = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= MAX_SCRIPT)
            .and_then(|_| std::fs::read(&path).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| script.clone());
        let origin = std::fs::canonicalize(&path).ok().or(Some(path));
        (text, label, origin)
    } else if inv.stdin && !std::io::stdin().is_terminal() {
        let mut buf = Vec::new();
        if std::io::stdin().read_to_end(&mut buf).is_err() {
            buf.clear();
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        stdin_buffer = Some(buf);
        (text, "<stdin>".to_string(), None)
    } else {
        // A REPL, or nothing we can read: nothing to look at.
        return hand_over(&real, &args, None);
    };

    if source.trim().is_empty() {
        return hand_over(&real, &args, stdin_buffer);
    }

    let cwd = std::env::current_dir().ok();
    let follow_off = std::env::var("PYXRAY_FOLLOW").is_ok_and(|v| v == "0" || v == "off");
    let report = pyxray_core::xray_guarded_with(
        &source,
        &label,
        &XrayOpts {
            origin: origin.clone(),
            cwd: cwd.clone(),
            follow: if follow_off {
                FollowOpts::off()
            } else {
                FollowOpts::default()
            },
        },
    );
    let run = run_id();
    let (event, feed_error) = pyxray_core::record(
        &report,
        &RecordOpts {
            source: "shim".into(),
            run: Some(run.clone()),
            origin,
            feed: None,
            write: true,
            cwd,
            policy: true,
        },
    );
    if let (Some(err), Ok("1")) = (feed_error, std::env::var("PYXRAY_DEBUG").as_deref()) {
        eprintln!("pyxray: feed not written: {err}");
    }

    match decide(
        &event,
        gate(),
        std::env::var("PYXRAY_CONFIRM").as_deref() == Ok("1"),
    ) {
        Verdict::Run => {}
        Verdict::Refuse(reason) => {
            eprintln!("pyxray: refusing to run — {reason}");
            return ExitCode::from(REFUSED);
        }
        Verdict::Confirm(reason) => {
            if !confirm(&format!(
                "pyxray: risk {}{}. Run it? [y/N] ",
                event.risk,
                reason.map(|r| format!(" ({r})")).unwrap_or_default()
            )) {
                return ExitCode::from(REFUSED);
            }
        }
    }

    // Children started by the script (and nested pythons through this shim)
    // share the run id, so their rows belong to this one.
    std::env::set_var("PYXRAY_RUN", &run);
    if std::env::var("PYXRAY_AUDIT").as_deref() == Ok("1") {
        arm_audit();
    }
    hand_over(&real, &args, stdin_buffer)
}

/// The runtime audit hook, carried inside the shim so it needs no Python
/// package: written once per version to a temporary directory that goes first
/// on the child's `PYTHONPATH`, where CPython imports it as `sitecustomize`.
const AUDIT_HOOK: &str = include_str!("../../../python/pyxray_agent/_audit/sitecustomize.py");

/// Have the interpreter about to start report what it actually does.
fn arm_audit() {
    let dir = std::env::temp_dir().join(format!(
        "pyxray-audit-{}",
        pyxray_core::record::digest(AUDIT_HOOK)
    ));
    let file = dir.join("sitecustomize.py");
    if !file.is_file() {
        let staged = dir.join(format!("sitecustomize.{}.tmp", std::process::id()));
        let ok = std::fs::create_dir_all(&dir).is_ok()
            && std::fs::write(&staged, AUDIT_HOOK).is_ok()
            && std::fs::rename(&staged, &file).is_ok();
        if !ok && !file.is_file() {
            return; // nowhere to put it: run unaudited rather than not at all
        }
    }
    std::env::set_var("PYXRAY_AUDIT_LOG", pyxray_core::feed::default_path());
    let mut paths = vec![dir];
    if let Some(existing) = std::env::var_os("PYTHONPATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PYTHONPATH", joined);
    }
}

/// The interpreter to run for real: a program plus any arguments that select
/// it (`py -3`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Real {
    program: PathBuf,
    prefix: Vec<String>,
}

/// `python3.12.exe` → `python3.12`; anything not python-shaped is `python`.
fn interpreter_name(exe: &Path) -> Option<String> {
    // Not `file_stem`: that reads `python3.12` as `python3` plus an extension.
    let file = exe.file_name()?.to_string_lossy().to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file).to_string();
    Some(if stem.starts_with("python") {
        stem
    } else {
        "python".to_string()
    })
}

/// Find the interpreter this shim stands in for.
///
/// `PYXRAY_PYTHON` wins. Otherwise the first `name` on PATH that is not in
/// this shim's own directory; Microsoft Store stubs (`…\WindowsApps\`) only
/// if nothing else exists, since they open the Store instead of running
/// Python; `python3` falls back to `python`; and on Windows, last, the `py`
/// launcher with `-3`.
fn find_real(
    name: &str,
    explicit: Option<OsString>,
    path_var: Option<OsString>,
    own_dir: Option<&Path>,
) -> Option<Real> {
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        return Some(Real {
            program: PathBuf::from(p),
            prefix: Vec::new(),
        });
    }
    let own = own_dir.and_then(|d| std::fs::canonicalize(d).ok());
    let dirs: Vec<PathBuf> = path_var
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let exe = |n: &str| {
        if cfg!(windows) {
            format!("{n}.exe")
        } else {
            n.to_string()
        }
    };
    let mut names = vec![name.to_string()];
    if name != "python" {
        names.push(if name.starts_with("python3") {
            "python".to_string()
        } else {
            "python3".to_string()
        });
    }
    let mut store_stub: Option<PathBuf> = None;
    for n in &names {
        for dir in &dirs {
            let candidate = dir.join(exe(n));
            if !candidate.is_file() {
                continue;
            }
            let canonical_dir = std::fs::canonicalize(dir).ok();
            if canonical_dir.is_some() && canonical_dir == own {
                continue;
            }
            if candidate
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("windowsapps")
            {
                store_stub.get_or_insert(candidate);
                continue;
            }
            return Some(Real {
                program: candidate,
                prefix: Vec::new(),
            });
        }
    }
    if cfg!(windows) {
        for dir in &dirs {
            let py = dir.join("py.exe");
            if py.is_file() {
                return Some(Real {
                    program: py,
                    prefix: vec!["-3".into()],
                });
            }
        }
    }
    store_stub.map(|program| Real {
        program,
        prefix: Vec::new(),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Run,
    Refuse(String),
    /// Ask at the terminal. `Some(reason)` when a gate or policy asked for it,
    /// `None` when it is only `PYXRAY_CONFIRM` asking.
    Confirm(Option<String>),
}

/// The same decision the Python wrapper and every harness hook make: the
/// strictest of the risk gate and the policy. `ask` with nobody to ask is
/// a refusal.
fn decide(event: &pyxray_core::Event, gate: Option<u8>, confirm: bool) -> Verdict {
    use pyxray_core::Decision;
    let mut decision = Decision::Allow;
    let mut reason = String::new();
    if let Some(g) = gate {
        if event.risk > g {
            decision = Decision::Ask;
            reason = format!(
                "risk {} is over the gate of {g} ({})",
                event.risk,
                if event.synopsis.is_empty() {
                    "no summary"
                } else {
                    &event.synopsis
                }
            );
        }
    }
    if let Some(p) = &event.policy {
        if p.decision > decision {
            decision = p.decision;
            reason = format!("{} ({})", p.reason, p.rule);
        }
    }
    match (decision, confirm) {
        (Decision::Deny, _) => Verdict::Refuse(reason),
        (Decision::Ask, false) => Verdict::Refuse(reason),
        (Decision::Ask, true) => Verdict::Confirm(Some(reason)),
        (Decision::Allow, true) => Verdict::Confirm(None),
        (Decision::Allow, false) => Verdict::Run,
    }
}

/// `PYXRAY_GATE`, read the way every other pyxray entry point reads it.
fn gate() -> Option<u8> {
    let raw = std::env::var("PYXRAY_GATE").ok()?;
    let raw = raw.trim().to_ascii_lowercase();
    if matches!(raw.as_str(), "" | "off" | "none" | "-1") {
        return None;
    }
    raw.parse::<i64>().ok().map(|v| v.clamp(0, 100) as u8)
}

/// Ask on the console itself, not on stdin — stdin may be the script.
fn confirm(prompt: &str) -> bool {
    let device = if cfg!(windows) { "CONIN$" } else { "/dev/tty" };
    let out_device = if cfg!(windows) { "CONOUT$" } else { "/dev/tty" };
    let (Ok(mut input), Ok(mut output)) = (
        std::fs::File::open(device),
        std::fs::OpenOptions::new().write(true).open(out_device),
    ) else {
        eprintln!("pyxray: PYXRAY_CONFIRM needs a terminal; not running");
        return false;
    };
    let _ = output.write_all(prompt.as_bytes());
    let _ = output.flush();
    let mut answer = [0u8; 16];
    let n = input.read(&mut answer).unwrap_or(0);
    let text = String::from_utf8_lossy(&answer[..n])
        .trim()
        .to_ascii_lowercase();
    text == "y" || text == "yes"
}

/// Run the real interpreter with our arguments and our standard streams (or
/// the stdin we already read), and exit with its code.
fn hand_over(real: &Real, args: &[OsString], stdin: Option<Vec<u8>>) -> ExitCode {
    #[cfg(unix)]
    if stdin.is_none() {
        use std::os::unix::process::CommandExt;
        // Become the interpreter: one process, signals and job control intact.
        let err = Command::new(&real.program)
            .args(&real.prefix)
            .args(args)
            .exec();
        eprintln!("pyxray: could not run {}: {err}", real.program.display());
        return ExitCode::from(127);
    }

    // Leave Ctrl-C to the child: it is the one that should decide what an
    // interrupt means, and the shim must outlive it to pass on its status.
    ignore_interrupts();
    let mut cmd = Command::new(&real.program);
    cmd.args(&real.prefix).args(args);
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pyxray: could not run {}: {e}", real.program.display());
            return ExitCode::from(127);
        }
    };
    if let (Some(buf), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(&buf);
    }
    match child.wait() {
        Ok(status) => ExitCode::from(exit_byte(status)),
        Err(_) => ExitCode::from(1),
    }
}

fn exit_byte(status: std::process::ExitStatus) -> u8 {
    match status.code() {
        Some(c) => (c & 0xff) as u8,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    return (128 + sig).clamp(0, 255) as u8;
                }
            }
            1
        }
    }
}

#[cfg(windows)]
fn ignore_interrupts() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    // SAFETY: passing no handler with add=TRUE only sets this process to
    // ignore Ctrl-C; the child gets it from the console as usual.
    unsafe {
        SetConsoleCtrlHandler(None, 1);
    }
}

#[cfg(not(windows))]
fn ignore_interrupts() {}

/// `pid-millis`, in hex: unique enough to join a run's lines in one feed.
fn run_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    std::env::var("PYXRAY_RUN").unwrap_or_else(|_| format!("{:x}-{:x}", std::process::id(), ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pyx-shim-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    fn exe(n: &str) -> String {
        if cfg!(windows) {
            format!("{n}.exe")
        } else {
            n.to_string()
        }
    }

    #[test]
    fn skips_its_own_directory() {
        let root = dir("own");
        let own = root.join("shim");
        let real = root.join("real");
        touch(&own.join(exe("python3")));
        touch(&real.join(exe("python3")));
        let path = std::env::join_paths([&own, &real]).unwrap();
        let found = find_real("python3", None, Some(path), Some(&own)).unwrap();
        assert_eq!(found.program, real.join(exe("python3")));
    }

    #[test]
    fn store_stubs_come_last() {
        let root = dir("store");
        let store = root.join("WindowsApps");
        let real = root.join("Python312");
        touch(&store.join(exe("python")));
        touch(&real.join(exe("python")));
        let path = std::env::join_paths([&store, &real]).unwrap();
        assert_eq!(
            find_real("python", None, Some(path), None).unwrap().program,
            real.join(exe("python"))
        );
        let only_stub = std::env::join_paths([&store]).unwrap();
        assert_eq!(
            find_real("python", None, Some(only_stub), None)
                .unwrap()
                .program,
            store.join(exe("python"))
        );
    }

    #[test]
    fn python3_falls_back_to_python_and_explicit_wins() {
        let root = dir("fallback");
        touch(&root.join(exe("python")));
        let path = std::env::join_paths([&root]).unwrap();
        assert_eq!(
            find_real("python3", None, Some(path.clone()), None)
                .unwrap()
                .program,
            root.join(exe("python"))
        );
        let explicit = find_real(
            "python3",
            Some("/opt/py/bin/python3".into()),
            Some(path),
            None,
        );
        assert_eq!(
            explicit.unwrap().program,
            PathBuf::from("/opt/py/bin/python3")
        );
        assert_eq!(
            find_real("python3", None, Some(OsString::new()), None),
            None
        );
    }

    #[test]
    fn names() {
        assert_eq!(
            interpreter_name(Path::new("/x/python3.12")).as_deref(),
            Some("python3.12")
        );
        assert_eq!(
            interpreter_name(Path::new("C:/x/Python.EXE")).as_deref(),
            Some("python")
        );
        assert_eq!(
            interpreter_name(Path::new("/x/pyx-shim")).as_deref(),
            Some("python")
        );
    }

    fn event(src: &str) -> pyxray_core::Event {
        pyxray_core::xray(src, "t").event("shim")
    }

    #[test]
    fn decisions() {
        let risky = event("import os\nos.system('ls')\n");
        assert_eq!(decide(&risky, None, false), Verdict::Run);
        assert!(
            matches!(decide(&risky, Some(10), false), Verdict::Refuse(r) if r.contains("gate of 10"))
        );
        assert!(matches!(
            decide(&risky, Some(10), true),
            Verdict::Confirm(Some(_))
        ));
        assert_eq!(decide(&risky, Some(90), true), Verdict::Confirm(None));
        let mut denied = event("print(1)\n");
        denied.policy = Some(pyxray_core::PolicyVerdict {
            decision: pyxray_core::Decision::Deny,
            rule: "p#1 any deny".into(),
            reason: "nope".into(),
            line: None,
            file: None,
        });
        assert!(matches!(decide(&denied, None, true), Verdict::Refuse(r) if r.contains("nope")));
    }
}
