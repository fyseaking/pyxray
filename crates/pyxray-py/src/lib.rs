//! Python bindings.
//!
//! The report crosses the boundary as JSON rather than as a hand-built dict:
//! the schema is already defined once by serde in `pyxray-core`, and keeping
//! one definition beats keeping two in step.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyxray_render::export::{self, Format};
use pyxray_render::layout::{self, Layout};
use pyxray_render::panels::{Icons, Opts};
use pyxray_render::theme::{self, THEMES};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Run a binding body with a panic fence. `xray_guarded` already contains the
/// analyser; this catches anything in rendering or serialisation, so the host
/// interpreter — the agent's own process — gets an exception, never an abort.
fn fenced<T>(what: &str, body: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => {
            let why = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "internal error".to_string());
            Err(PyRuntimeError::new_err(format!("pyxray: {what}: {why}")))
        }
    }
}

fn pick_theme(id: &str) -> PyResult<theme::Theme> {
    theme::theme(id).ok_or_else(|| PyValueError::new_err(format!("unknown theme {id:?}")))
}

fn pick_layout(id: &str) -> PyResult<Layout> {
    layout::layout(id).ok_or_else(|| PyValueError::new_err(format!("unknown layout {id:?}")))
}

fn pick_format(id: &str) -> PyResult<Format> {
    export::format(id).ok_or_else(|| PyValueError::new_err(format!("unknown format {id:?}")))
}

fn pick_icons(id: &str) -> PyResult<Icons> {
    match id {
        "glyph" => Ok(Icons::Glyph),
        "tag" => Ok(Icons::Tag),
        "both" => Ok(Icons::Both),
        other => Err(PyValueError::new_err(format!(
            "unknown icon mode {other:?}"
        ))),
    }
}

/// Where a snippet came from, as the core wants it. Following local imports
/// is on unless asked off or `PYXRAY_FOLLOW=0`.
fn xray_opts(origin: Option<&str>, cwd: Option<&str>, follow: bool) -> pyxray_core::XrayOpts {
    let env_off = std::env::var("PYXRAY_FOLLOW").is_ok_and(|v| v == "0" || v == "off");
    pyxray_core::XrayOpts {
        origin: origin.map(std::path::PathBuf::from),
        cwd: cwd.map(std::path::PathBuf::from),
        follow: if follow && !env_off {
            pyxray_core::FollowOpts::default()
        } else {
            pyxray_core::FollowOpts::off()
        },
    }
}

/// Analyse `source` and return the report as a JSON string. With `origin`
/// (the script's path) or `cwd`, local imports are followed too.
#[pyfunction]
#[pyo3(signature = (source, name = "<stdin>", origin = None, cwd = None, follow = true))]
fn analyze_json(
    source: &str,
    name: &str,
    origin: Option<&str>,
    cwd: Option<&str>,
    follow: bool,
) -> PyResult<String> {
    fenced("analyze", || {
        let report = pyxray_core::xray_guarded_with(source, name, &xray_opts(origin, cwd, follow));
        serde_json::to_string(&report).map_err(|e| PyValueError::new_err(e.to_string()))
    })
}

/// Render `source` to a string in the requested format.
#[pyfunction]
#[pyo3(signature = (
    source,
    name = "<stdin>",
    theme = "blueprint",
    layout = "card",
    format = "ansi",
    width = 100,
    height = None,
    icons = "both",
    depth = 6,
    gutter = true,
    code = false,
))]
#[allow(clippy::too_many_arguments)]
fn render(
    source: &str,
    name: &str,
    theme: &str,
    layout: &str,
    format: &str,
    width: u16,
    height: Option<u16>,
    icons: &str,
    depth: u16,
    gutter: bool,
    code: bool,
) -> PyResult<String> {
    fenced("render", || {
        let report = pyxray_core::xray_guarded(source, name);
        let style = pyxray_render::Style {
            theme: pick_theme(theme)?,
            layout: pick_layout(layout)?,
            opts: Opts {
                max_depth: depth,
                gutter,
                icons: pick_icons(icons)?,
                code,
            },
        };
        Ok(pyxray_render::render_to_string(
            &report,
            &style,
            pick_format(format)?,
            width,
            height,
        ))
    })
}

/// The interceptor's hot path: analyse once, append the feed event, and
/// render only if somebody is actually looking.
///
/// A harness can fire several snippets a second, so this is deliberately one
/// call rather than three — and `draw=False` skips the render entirely, which
/// is the common case when the only consumer is `pyx watch` in another pane.
///
/// Returns `(event_json, rendered)`; `rendered` is empty when `draw` is false.
/// `record=False` analyses without appending (a post-tool hook looking again
/// at a call it already recorded); `origin` is the script's path and `run` an
/// execution id, both carried on the event.
#[pyfunction]
#[pyo3(signature = (
    code,
    name = "<stdin>",
    source = "shim",
    path = None,
    draw = true,
    theme = "blueprint",
    layout = "auto",
    format = "ansi",
    width = 100,
    icons = "both",
    record = true,
    origin = None,
    run = None,
    cwd = None,
    follow = true,
))]
#[allow(clippy::too_many_arguments)]
fn look(
    code: &str,
    name: &str,
    source: &str,
    path: Option<&str>,
    draw: bool,
    theme: &str,
    layout: &str,
    format: &str,
    width: u16,
    icons: &str,
    record: bool,
    origin: Option<&str>,
    run: Option<&str>,
    cwd: Option<&str>,
    follow: bool,
) -> PyResult<(String, String)> {
    fenced("look", || {
        let report = pyxray_core::xray_guarded_with(code, name, &xray_opts(origin, cwd, follow));
        // A feed we cannot write is not worth failing the caller's command
        // over — the interceptor is in front of somebody else's work. But it
        // is worth telling the caller, so `PYXRAY_DEBUG` can surface it.
        let (event, feed_error) = pyxray_core::record(
            &report,
            &pyxray_core::RecordOpts {
                source: source.to_string(),
                run: run.map(str::to_string),
                origin: origin.map(std::path::PathBuf::from),
                feed: path.map(std::path::PathBuf::from),
                write: record,
                cwd: cwd.map(std::path::PathBuf::from),
                policy: true,
            },
        );

        let mut value =
            serde_json::to_value(&event).map_err(|e| PyValueError::new_err(e.to_string()))?;
        if let Some(err) = feed_error {
            value["feed_error"] = serde_json::Value::String(err);
        }
        let json = value.to_string();
        if !draw {
            return Ok((json, String::new()));
        }
        let style = pyxray_render::Style {
            theme: pick_theme(theme)?,
            layout: pick_layout(layout)?,
            opts: Opts {
                icons: pick_icons(icons)?,
                ..Opts::default()
            },
        };
        let rendered =
            pyxray_render::render_to_string(&report, &style, pick_format(format)?, width, None);
        Ok((json, rendered))
    })
}

/// Where the feed log lives, resolved the same way the binary resolves it.
#[pyfunction]
fn feed_path() -> String {
    pyxray_core::feed::default_path().display().to_string()
}

/// Read the whole feed back as a JSON array.
#[pyfunction]
#[pyo3(signature = (path = None))]
fn read_feed(path: Option<&str>) -> PyResult<String> {
    let target = path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(pyxray_core::feed::default_path);
    let events =
        pyxray_core::feed::read_all(&target).map_err(|e| PyValueError::new_err(e.to_string()))?;
    serde_json::to_string(&events).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Pull the first piece of Python out of a shell command, or return the
/// command itself when it is not recognisably a wrapped snippet. Returns
/// `(source, label)`. See `extract_all` for everything.
#[pyfunction]
fn extract(command: &str) -> (String, String) {
    pyxray_core::extract_python(command)
}

/// Every piece of Python a shell command would run, as a JSON array of
/// `{source, label, path, segment}` — heredocs, `-c`, script files (read
/// relative to `cwd`), pipes, `bash -c`, in command order.
#[pyfunction]
#[pyo3(signature = (command, cwd = None, read_files = true))]
fn extract_all(command: &str, cwd: Option<&str>, read_files: bool) -> PyResult<String> {
    fenced("extract", || {
        let opts = pyxray_core::ExtractOpts {
            cwd: cwd.map(std::path::PathBuf::from),
            read_files,
            ..pyxray_core::ExtractOpts::default()
        };
        let found = pyxray_core::extract_all(command, &opts);
        serde_json::to_string(&found).map_err(|e| PyValueError::new_err(e.to_string()))
    })
}

/// Read an interpreter's own argument list the way CPython does. Returns
/// JSON `{code, module, script, stdin}`.
#[pyfunction]
fn classify_argv(args: Vec<String>) -> PyResult<String> {
    fenced("classify_argv", || {
        let inv = pyxray_core::extract::classify_argv(&args);
        serde_json::to_string(&inv).map_err(|e| PyValueError::new_err(e.to_string()))
    })
}

/// The available themes, as `(id, name, blurb)` triples.
#[pyfunction]
fn themes() -> Vec<(String, String, String)> {
    THEMES
        .iter()
        .map(|t| (t.id.to_string(), t.name.to_string(), t.blurb.to_string()))
        .collect()
}

/// The available layouts, as `(id, blurb)` pairs.
#[pyfunction]
fn layouts() -> Vec<(String, String)> {
    Layout::all()
        .into_iter()
        .map(|l| (l.id().to_string(), l.blurb().to_string()))
        .collect()
}

#[pymodule]
fn _pyxray(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(analyze_json, m)?)?;
    m.add_function(wrap_pyfunction!(render, m)?)?;
    m.add_function(wrap_pyfunction!(extract, m)?)?;
    m.add_function(wrap_pyfunction!(extract_all, m)?)?;
    m.add_function(wrap_pyfunction!(classify_argv, m)?)?;
    m.add("SCHEMA", pyxray_core::SCHEMA)?;
    m.add_function(wrap_pyfunction!(look, m)?)?;
    m.add_function(wrap_pyfunction!(feed_path, m)?)?;
    m.add_function(wrap_pyfunction!(read_feed, m)?)?;
    m.add_function(wrap_pyfunction!(themes, m)?)?;
    m.add_function(wrap_pyfunction!(layouts, m)?)?;
    Ok(())
}
