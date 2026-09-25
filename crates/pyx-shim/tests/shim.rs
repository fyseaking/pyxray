//! The built shim, run as a `python3` in front of the real one.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The real interpreter to put behind the shim: `PYXRAY_TEST_PYTHON`, else
/// the first python3/python on PATH. Tests are skipped when there is none.
fn real_python() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PYXRAY_TEST_PYTHON") {
        return Some(PathBuf::from(p));
    }
    let names: &[&str] = if cfg!(windows) {
        &["python.exe", "python3.exe"]
    } else {
        &["python3", "python"]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            let c = dir.join(n);
            if c.is_file()
                && !c
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("windowsapps")
            {
                return Some(c);
            }
        }
    }
    None
}

struct Setup {
    dir: PathBuf,
    shim: PathBuf,
    feed: PathBuf,
    real: PathBuf,
}

fn setup(name: &str) -> Option<Setup> {
    let real = real_python()?;
    let dir = std::env::temp_dir().join(format!("pyx-shim-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let shim = dir.join("bin").join(if cfg!(windows) {
        "python3.exe"
    } else {
        "python3"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_pyx-shim"), &shim).unwrap();
    Some(Setup {
        feed: dir.join("feed.jsonl"),
        dir,
        shim,
        real,
    })
}

impl Setup {
    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.shim);
        c.env("PYXRAY_PYTHON", &self.real)
            .env("PYXRAY_LOG", &self.feed)
            .env("PYXRAY_POLICY", "off")
            .env_remove("PYXRAY_GATE")
            .env_remove("PYXRAY_OFF")
            .env_remove("PYXRAY_CONFIRM")
            .current_dir(&self.dir);
        c
    }

    fn rows(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.feed)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

macro_rules! need_python {
    ($name:expr) => {
        match setup($name) {
            Some(s) => s,
            None => {
                eprintln!("no python found; skipping");
                return;
            }
        }
    };
}

#[test]
fn exit_codes_pass_through_and_a_row_is_written() {
    let s = need_python!("exit");
    let out = s
        .cmd()
        .args(["-c", "raise SystemExit(7)"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows = s.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["source"], "shim");
    assert!(rows[0]["run"].is_string());
}

#[test]
fn a_script_runs_with_its_arguments_and_its_imports_are_followed() {
    let s = need_python!("script");
    std::fs::write(
        s.dir.join("helper.py"),
        "import os\n\ndef go():\n    return os.getcwd()\n",
    )
    .unwrap();
    std::fs::write(
        s.dir.join("main.py"),
        "import sys, helper\nhelper.go()\nprint('args', sys.argv[1:])\n",
    )
    .unwrap();
    let out = s.cmd().args(["main.py", "a", "b c"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "args ['a', 'b c']"
    );
    let rows = s.rows();
    assert_eq!(rows[0]["name"], "main.py");
    assert!(rows[0]["path"].as_str().unwrap().ends_with("main.py"));
}

#[test]
fn a_script_on_stdin_is_read_once_and_still_runs() {
    let s = need_python!("stdin");
    let mut child = s
        .cmd()
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"import os\nprint('from stdin', os.getpid() > 0)\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "from stdin True"
    );
    assert_eq!(s.rows()[0]["name"], "<stdin>");
}

#[test]
fn over_the_gate_it_refuses_with_status_3() {
    let s = need_python!("gate");
    let out = s
        .cmd()
        .env("PYXRAY_GATE", "10")
        .args(["-c", "import os; os.system('echo should-not-run')"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("should-not-run"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing to run"));
}

#[test]
fn a_policy_deny_refuses() {
    let s = need_python!("policy");
    let policy = s.dir.join("p.toml");
    std::fs::write(
        &policy,
        "[[rule]]\neffect = \"fs_delete\"\ndecision = \"deny\"\nreason = \"no deleting\"\n",
    )
    .unwrap();
    let out = s
        .cmd()
        .env("PYXRAY_POLICY", &policy)
        .args(["-c", "import os; os.remove('nothing-here')"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no deleting"));
}

#[test]
fn pyxray_off_passes_straight_through_and_records_nothing() {
    let s = need_python!("off");
    let out = s
        .cmd()
        .env("PYXRAY_OFF", "1")
        .env("PYXRAY_GATE", "0")
        .args(["-c", "import os; print(os.name)"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(s.rows().is_empty());
}

#[test]
fn with_no_real_python_it_says_so() {
    let Some(s) = setup("none") else { return };
    let out = Command::new(&s.shim)
        .env_remove("PYXRAY_PYTHON")
        .env("PATH", s.dir.join("bin"))
        .args(["-c", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127));
    let _ = Path::new(".");
}
