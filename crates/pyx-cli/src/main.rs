//! pyx — see what a Python snippet will do before you run it.

mod args;
mod sheet;
mod tui;
mod watch;

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use pyxray_core::feed;
use pyxray_core::{extract, xray_guarded_with, FollowOpts, Report, XrayOpts};
use pyxray_render::export::Format;
use pyxray_render::layout;
use pyxray_render::theme::THEMES;

use args::{ColorMode, Mode};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("pyx: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let a = match args::parse(std::env::args().skip(1))? {
        args::Parsed::Help => {
            print!("{}", args::HELP);
            return Ok(ExitCode::SUCCESS);
        }
        args::Parsed::Version => {
            println!("pyx {VERSION}");
            return Ok(ExitCode::SUCCESS);
        }
        args::Parsed::Run(a) => *a,
    };

    if a.list {
        return list(a.json, a.out.as_deref()).map(|_| ExitCode::SUCCESS);
    }

    match a.mode {
        Mode::Watch => {
            let path = a
                .file
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(feed::default_path);
            watch::run(watch::Options {
                path,
                theme: a.theme,
                opts: a.opts,
                replay: a.replay,
                floor: a.floor,
                dump: a.dump,
                format: a.format,
                fragment: a.fragment,
                size: (a.width, a.height),
            })
            .map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
        Mode::Extract => return run_extract(&a),
        Mode::Policy => return run_policy(&a),
        Mode::Xray => {}
    }

    let (source, name) = read_input(&a)?;
    let origin = a
        .origin
        .clone()
        .or_else(|| a.file.clone().filter(|p| p != "-"))
        .map(std::path::PathBuf::from);
    let follow_off =
        !a.follow || std::env::var("PYXRAY_FOLLOW").is_ok_and(|v| v == "0" || v == "off");
    let report = xray_guarded_with(
        &source,
        &name,
        &XrayOpts {
            origin,
            cwd: a.cwd.clone().map(std::path::PathBuf::from),
            follow: if follow_off {
                FollowOpts::off()
            } else {
                FollowOpts::default()
            },
        },
    );

    if a.json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        write_out(a.out.as_deref(), &text)?;
        return Ok(check(&report, &a));
    }

    if let Some(path) = &a.contact_sheet {
        let width = a.width.unwrap_or(112);
        let html = sheet::build(&report, width, &a.opts);
        write_out(Some(path), &html)?;
        eprintln!("pyx: wrote {path} ({} bytes)", html.len());
        return Ok(check(&report, &a));
    }

    if a.tui {
        tui::run(&report, &a).map_err(|e| e.to_string())?;
        return Ok(check(&report, &a));
    }

    // The feed comes first: it is the one output that must survive even when
    // nothing is drawn, because it is what `pyx watch` is reading.
    let mut head = String::new();
    if a.emit || a.print_event {
        let (event, feed_error) = pyxray_core::record(
            &report,
            &pyxray_core::RecordOpts {
                source: a.source.clone(),
                run: a.run.clone(),
                origin: a.origin.clone().map(std::path::PathBuf::from).or_else(|| {
                    // A file on the command line is its own origin.
                    a.file
                        .clone()
                        .filter(|p| p != "-")
                        .map(std::path::PathBuf::from)
                }),
                feed: a.feed.as_ref().map(std::path::PathBuf::from),
                // `--print-event` alone analyses and reports; only `--emit`
                // writes the log.
                write: a.emit,
                cwd: a.cwd.clone().map(std::path::PathBuf::from),
                policy: true,
            },
        );
        let mut value = serde_json::to_value(&event).map_err(|e| e.to_string())?;
        if let Some(err) = feed_error {
            eprintln!("pyx: could not write {err}");
            value["feed_error"] = serde_json::Value::String(err);
        }
        if a.print_event {
            head = format!("{value}\n");
        }
    }
    if a.quiet {
        write_out(a.out.as_deref(), &head)?;
        if a.exec {
            return execute(&report, &source, &a);
        }
        return Ok(check(&report, &a));
    }

    let width = a.width.unwrap_or_else(|| term_width().unwrap_or(100));
    let style = pyxray_render::Style {
        theme: a.theme,
        layout: a.layout,
        opts: a.opts,
    };
    let format = if a.format == Format::Ansi && !use_color(&a) {
        Format::Text
    } else {
        a.format
    };
    let text = if a.fragment && format == Format::Html {
        let buf = pyxray_render::layout::render(
            &report,
            &style.theme,
            &style.opts,
            style.layout,
            width,
            a.height,
        );
        pyxray_render::export::to_html_fragment(&buf, &style.theme)
    } else {
        pyxray_render::render_to_string(&report, &style, format, width, a.height)
    };
    write_out(a.out.as_deref(), &format!("{head}{text}"))?;

    if a.exec {
        return execute(&report, &source, &a);
    }
    Ok(check(&report, &a))
}

/// `--check N`: the exit code that lets a script or CI step refuse a snippet
/// without running it.
fn check(report: &Report, a: &args::Args) -> ExitCode {
    match a.check {
        Some(limit) if report.metrics.risk > limit => {
            eprintln!(
                "pyx: risk {} is above the --check limit of {limit}",
                report.metrics.risk
            );
            ExitCode::from(3)
        }
        _ => ExitCode::SUCCESS,
    }
}

/// Whether ANSI colour should go to the output. `--color` wins; then the
/// two conventions everybody's terminal already honours; then "is this a
/// terminal at all" — and on Windows, a console that has been switched into
/// VT mode, which crossterm checks (and enables) for us.
fn use_color(a: &args::Args) -> bool {
    match a.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            let piped = a.out.is_some() || !std::io::stdout().is_terminal();
            let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
            let force =
                std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0");
            if force {
                return true;
            }
            if no_color || piped {
                return false;
            }
            if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
                return false;
            }
            #[cfg(windows)]
            {
                if !crossterm::ansi_support::supports_ansi() {
                    return false;
                }
            }
            true
        }
    }
}

fn run_extract(a: &args::Args) -> Result<ExitCode, String> {
    if let Some(argv) = &a.argv {
        let inv = extract::classify_argv(argv);
        let text = serde_json::to_string(&inv).map_err(|e| e.to_string())?;
        return write_out(a.out.as_deref(), &format!("{text}\n")).map(|_| ExitCode::SUCCESS);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("stdin: {e}"))?;
    let opts = extract::ExtractOpts {
        cwd: a.cwd.as_ref().map(std::path::PathBuf::from),
        read_files: a.read_files,
        ..extract::ExtractOpts::default()
    };
    let found = extract::extract_all(&buf, &opts);
    let text = serde_json::to_string(&found).map_err(|e| e.to_string())?;
    write_out(a.out.as_deref(), &format!("{text}\n")).map(|_| ExitCode::SUCCESS)
}

fn run_policy(a: &args::Args) -> Result<ExitCode, String> {
    use pyxray_core::policy::{user_policy_path, Policy};
    let file = a.file.clone().filter(|p| p != "-");
    let dir = a
        .cwd
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            file.as_deref()
                .and_then(|f| std::path::Path::new(f).parent().map(|p| p.to_path_buf()))
                .filter(|p| !p.as_os_str().is_empty())
        })
        .or_else(|| std::env::current_dir().ok());
    let policy = Policy::discover(dir.as_deref());
    let mut out = String::new();
    match std::env::var("PYXRAY_POLICY") {
        Ok(v) if !v.trim().is_empty() => out.push_str(&format!("PYXRAY_POLICY={v}\n")),
        _ => {}
    }
    if policy.is_empty() {
        out.push_str("no policy applies");
        if let Some(user) = user_policy_path() {
            out.push_str(&format!(" (your user file would be {})", user.display()));
        }
        out.push('\n');
    }
    for (path, state) in policy.files() {
        match state {
            Ok(n) => out.push_str(&format!("{path}  {n} rule(s)\n")),
            Err(why) => out.push_str(&format!("{path}  ERROR {why}\n")),
        }
    }
    let snippet = match &file {
        Some(path) => Some((
            std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
            short_name(path),
        )),
        None if !std::io::stdin().is_terminal() => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("stdin: {e}"))?;
            (!buf.trim().is_empty()).then(|| (buf, "<stdin>".to_string()))
        }
        None => None,
    };
    if let Some((source, name)) = snippet {
        let report = xray_guarded_with(
            &source,
            &name,
            &XrayOpts {
                origin: file.map(std::path::PathBuf::from),
                cwd: dir.clone(),
                follow: FollowOpts::default(),
            },
        );
        match policy.evaluate(&report, dir.as_deref()) {
            Some(v) => out.push_str(&format!(
                "{name}: {} ({}) — {}\n",
                v.decision.word(),
                v.rule,
                v.reason
            )),
            None => out.push_str(&format!("{name}: no policy, nothing to decide\n")),
        }
    }
    write_out(a.out.as_deref(), &out).map(|_| ExitCode::SUCCESS)
}

fn read_input(a: &args::Args) -> Result<(String, String), String> {
    match &a.file {
        Some(path) if path != "-" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            Ok((text, a.name.clone().unwrap_or_else(|| short_name(path))))
        }
        _ => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("stdin: {e}"))?;
            if a.stdin_is_command {
                let (code, label) = extract::extract_python(&buf);
                Ok((code, a.name.clone().unwrap_or(label)))
            } else {
                Ok((buf, a.name.clone().unwrap_or_else(|| "<stdin>".to_string())))
            }
        }
    }
}

fn short_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn write_out(path: Option<&str>, text: &str) -> Result<(), String> {
    match path {
        Some(p) => std::fs::write(p, text).map_err(|e| format!("{p}: {e}")),
        None => {
            let mut out = std::io::stdout().lock();
            out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())
        }
    }
}

fn term_width() -> Option<u16> {
    crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w.clamp(40, 400))
}

fn list(json: bool, out: Option<&str>) -> Result<(), String> {
    if !json {
        let mut text = String::from("themes:\n");
        for t in THEMES {
            text.push_str(&format!("  {:<10} {}\n", t.id, t.blurb));
        }
        text.push_str("\nlayouts:\n");
        for l in layout::Layout::all() {
            text.push_str(&format!("  {:<10} {}\n", l.id(), l.blurb()));
        }
        text.push_str("\nformats:\n  ansi  text  html  svg  json\n");
        text.push_str(&format!("\nfeed:\n  {}\n", feed::default_path().display()));
        return write_out(out, &text);
    }
    use pyxray_render::export::{hex, to_rgb};
    let themes: Vec<serde_json::Value> = THEMES
        .iter()
        .map(|t| {
            let c = |col, bg| hex(to_rgb(col, t, bg));
            let effects: serde_json::Map<String, serde_json::Value> = pyxray_core::Effect::ALL
                .iter()
                .map(|e| (e.id().to_string(), c(t.effect_color(*e), false).into()))
                .collect();
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "blurb": t.blurb,
                "truecolor": t.truecolor,
                "light": t.light,
                "ascii": t.gl.ellipsis.is_ascii(),
                "palette": {
                    "bg": c(t.pal.bg, true),
                    "surface": c(t.pal.surface, true),
                    "fg": c(t.pal.fg, false),
                    "dim": c(t.pal.dim, false),
                    "faint": c(t.pal.faint, false),
                    "rule": c(t.pal.rule, false),
                    "accent": c(t.pal.accent, false),
                    "accent_alt": c(t.pal.accent_alt, false),
                    "ok": c(t.pal.ok, false),
                    "warn": c(t.pal.warn, false),
                    "danger": c(t.pal.danger, false),
                    "effects": effects,
                },
            })
        })
        .collect();
    let layouts: Vec<serde_json::Value> = layout::Layout::all()
        .into_iter()
        .map(|l| serde_json::json!({ "id": l.id(), "blurb": l.blurb(), "gallery": layout::Layout::gallery().contains(&l) }))
        .collect();
    let value = serde_json::json!({
        "version": VERSION,
        "schema": pyxray_core::SCHEMA,
        "themes": themes,
        "layouts": layouts,
        "formats": ["ansi", "text", "html", "svg", "json"],
        "feed": feed::default_path().display().to_string(),
    });
    write_out(
        out,
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        ),
    )
}

/// The interpreter `--exec` hands the snippet to: `$PYXRAY_PYTHON`, else
/// the first of the usual names that starts.
fn interpreter() -> Vec<String> {
    if let Ok(explicit) = std::env::var("PYXRAY_PYTHON") {
        if !explicit.is_empty() {
            return vec![explicit];
        }
    }
    let candidates: &[&[&str]] = &[&["python3"], &["python"], &["py", "-3"]];
    for cand in candidates {
        let ok = std::process::Command::new(cand[0])
            .args(&cand[1..])
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return cand.iter().map(|s| s.to_string()).collect();
        }
    }
    vec!["python3".to_string()]
}

/// Run the snippet, subject to whatever gate the caller asked for. The X-ray
/// has already been printed by this point, which is the whole idea: the
/// decision is made with the analysis on screen.
fn execute(report: &Report, source: &str, a: &args::Args) -> Result<ExitCode, String> {
    if let Some(limit) = a.gate {
        if report.metrics.risk > limit {
            eprintln!(
                "pyx: refusing to run — risk {} exceeds the --gate limit of {limit}",
                report.metrics.risk
            );
            return Ok(ExitCode::from(3));
        }
    }
    if a.confirm {
        if !std::io::stdin().is_terminal() {
            eprintln!("pyx: --confirm needs a terminal; not running");
            return Ok(ExitCode::from(3));
        }
        eprint!("pyx: run this? [y/N] ");
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            eprintln!("pyx: not running");
            return Ok(ExitCode::from(3));
        }
    }

    use std::process::{Command, Stdio};
    let py = interpreter();
    let mut cmd = Command::new(&py[0]);
    cmd.args(&py[1..]);
    let status = match &a.file {
        // A real file runs as itself, so `__file__`, `sys.argv[0]` and the
        // snippet's own stdin all behave as they would without pyx.
        Some(path) if path != "-" && !a.stdin_is_command => cmd
            .arg(path)
            .status()
            .map_err(|e| format!("{}: {e}", py.join(" ")))?,
        _ => {
            let mut child = cmd
                .arg("-")
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| format!("{}: {e}", py.join(" ")))?;
            child
                .stdin
                .as_mut()
                .ok_or("python: no stdin")?
                .write_all(source.as_bytes())
                .map_err(|e| e.to_string())?;
            child.wait().map_err(|e| e.to_string())?
        }
    };
    Ok(ExitCode::from(
        status.code().unwrap_or(1).clamp(0, 255) as u8
    ))
}
