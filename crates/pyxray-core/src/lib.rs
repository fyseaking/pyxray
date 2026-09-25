//! pyxray-core — parse a Python snippet and work out what it will actually do.
//!
//! ```no_run
//! let report = pyxray_core::xray("print('hi')", "<stdin>");
//! println!("{}", report.meta.synopsis);
//! ```
//!
//! The output is a [`Report`]: a structural spine, a list of side effects with
//! severities, the names it binds, the symbols it leans on, and a per-line
//! strip for minimaps. Nothing in here knows how any of it is drawn.

pub mod analyze;
pub mod digest;
pub mod effects;
pub mod extract;
pub mod feed;
pub mod model;
pub mod record;
pub mod source;

pub use extract::{extract_all, extract_python, ExtractOpts, Extracted, Invocation};
pub use model::*;
pub use record::{record, RecordOpts};

use ruff_python_parser::{parse_unchecked, ParseOptions};
use source::Src;

/// Analyse a snippet, and survive it. Runs [`xray`] on its own thread with a
/// generous stack, and if anything inside panics — a parser edge case, an
/// assumption the input broke — returns [`Report::failed`] instead of taking
/// the process down. This is the entry point the CLI and the Python
/// extension use: the analyser sits in front of somebody else's command, and
/// crashing there is worse than saying "could not look".
pub fn xray_guarded(source: &str, name: &str) -> Report {
    const STACK: usize = 64 << 20;
    let src = source.to_string();
    let nm = name.to_string();
    let spawned = std::thread::Builder::new()
        .name("pyxray-analyse".into())
        .stack_size(STACK)
        .spawn(move || xray(&src, &nm));
    let outcome = match spawned {
        Ok(handle) => handle.join(),
        // Could not even start a thread: analyse inline and accept the risk.
        Err(_) => return xray(source, name),
    };
    match outcome {
        Ok(report) => report,
        Err(payload) => {
            let why = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "internal error".to_string());
            Report::failed(source, name, &why)
        }
    }
}

/// Analyse a snippet. Never fails on bad *Python*: a snippet that does not
/// parse still yields a report built from whatever the recovering parser
/// salvaged, with the syntax errors attached as diagnostics — which is the
/// useful behaviour when the input came from a language model rather than a
/// file on disk. Internal panics are not caught here; see [`xray_guarded`].
pub fn xray(source: &str, name: &str) -> Report {
    let src = Src::new(source);
    let parsed = parse_unchecked(source, ParseOptions::from(ruff_python_parser::Mode::Module));

    let mut diagnostics: Vec<Diagnostic> = parsed
        .errors()
        .iter()
        .map(|e| Diagnostic {
            level: DiagLevel::Error,
            message: e.error.to_string(),
            line: src.line_of(e.location.start()),
            col: src.col_of(e.location.start()),
        })
        .collect();
    let parsed_ok = diagnostics.is_empty();

    let body: &[ruff_python_ast::Stmt] = match parsed.syntax() {
        ruff_python_ast::Mod::Module(m) => &m.body,
        ruff_python_ast::Mod::Expression(_) => &[],
    };

    let mut an = analyze::Analyzer::new(&src);
    an.prescan(body, false);
    let children = an.body(body, 0);

    let mut spine = Node {
        id: 0,
        kind: NodeKind::Module,
        label: name.to_string(),
        detail: None,
        line: 1,
        end_line: src.line_count().max(1),
        depth: 0,
        own_effects: EffectMask::default(),
        effects: EffectMask::default(),
        severity: None,
        weight: 1,
        children,
    };
    for c in &spine.children {
        spine.effects.union(c.effects);
        if let Some(cs) = c.severity {
            spine.severity = Some(spine.severity.map_or(cs, |s: Severity| s.max(cs)));
        }
        spine.weight += c.weight;
    }

    let mut found = an.into_parts();
    diagnostics.append(&mut found.diagnostics);
    let analyze::Findings {
        imports,
        effects: mut hits,
        mut symbols,
        mut bindings,
        mut metrics,
        reads,
        max_depth,
        ..
    } = found;

    digest::dedup(&mut hits);
    hits.sort_by_key(|h| (h.line, h.col));

    // Recompute node effect masks from the deduplicated hit list would be
    // exact, but dedup only ever removes duplicates of effects already in the
    // mask, so the masks stay correct as they are.

    for b in &mut bindings {
        b.reads = reads.get(&b.name).copied().unwrap_or(0);
        // The binding's own name appears once on the left-hand side, which is
        // a store, not a read — so any count at all means it is used.
        b.dead = b.reads == 0;
    }

    // The call inventory is for "what does this lean on", so it keeps library
    // calls, builtins and the file's own functions. Methods on values we could
    // not identify are noise unless they turned out to do something.
    symbols.retain(|s| match s.origin {
        SymbolOrigin::Builtin => !analyze::is_noise_symbol(&s.canonical),
        SymbolOrigin::Method => !s.effects.is_empty(),
        SymbolOrigin::Import | SymbolOrigin::Local => true,
    });

    let (total, code, comment, blank) = digest::line_counts(&src);
    metrics.lines_total = total;
    metrics.lines_code = code;
    metrics.lines_comment = comment;
    metrics.lines_blank = blank;
    metrics.max_depth = max_depth as u32;
    metrics.complexity += 1;
    metrics.risk = digest::risk(&hits);

    let texture = digest::texture(&src, &spine, &hits);
    let (synopsis, headline) = if source.trim().is_empty() {
        metrics.lines_code = 0;
        ("nothing to run".to_string(), Vec::new())
    } else {
        digest::synopsis(&hits, &metrics, &spine)
    };

    Report {
        schema: SCHEMA,
        meta: Meta {
            name: name.to_string(),
            bytes: source.len(),
            parsed: parsed_ok,
            synopsis,
            headline,
        },
        spine,
        imports,
        effects: hits,
        symbols,
        bindings,
        metrics,
        texture,
        diagnostics,
        source: source.lines().map(|l| l.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_are_distinguished() {
        let r = xray("f = open('a.txt')\ng = open('b.txt', 'w')\n", "t");
        let verbs: Vec<&str> = r.effects.iter().map(|e| e.verb.as_str()).collect();
        assert!(verbs.contains(&"read"), "{verbs:?}");
        assert!(verbs.contains(&"truncate"), "{verbs:?}");
    }

    #[test]
    fn aliases_resolve() {
        let r = xray("import numpy as np\nx = np.load('a.npy')\n", "t");
        assert!(r.effects.iter().any(|e| e.symbol == "numpy.load"));
    }

    #[test]
    fn method_calls_on_inferred_values_resolve() {
        let r = xray(
            "from pathlib import Path\nPath('x').write_text('hi')\n",
            "t",
        );
        assert!(
            r.effects.iter().any(|e| e.effect == Effect::FsWrite),
            "{:?}",
            r.effects
        );
    }

    #[test]
    fn shell_true_escalates() {
        let r = xray("import subprocess\nsubprocess.run('ls', shell=True)\n", "t");
        let hit = r
            .effects
            .iter()
            .find(|e| e.effect == Effect::Process)
            .unwrap();
        assert_eq!(hit.severity, Severity::Caution);
        assert!(hit.note.as_ref().unwrap().contains("shell=True"));
    }

    #[test]
    fn user_functions_do_not_impersonate_builtins() {
        let r = xray("def open(p):\n    return p\nopen('x')\n", "t");
        assert!(r.effects.is_empty(), "{:?}", r.effects);
    }

    #[test]
    fn broken_source_still_reports() {
        let r = xray("def f(:\n    pass\n", "t");
        assert!(!r.meta.parsed);
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn heredoc_is_extracted() {
        let (code, label) = extract_python("python3 <<'EOF'\nprint(1)\nEOF\n");
        assert_eq!(code, "print(1)\n");
        assert!(label.contains("EOF"));
    }

    #[test]
    fn dash_c_is_extracted() {
        let (code, _) = extract_python("python3 -c 'print(1)'");
        assert_eq!(code, "print(1)");
    }

    #[test]
    fn accumulators_are_spotted() {
        let r = xray("total = 0\nfor n in [1,2]:\n    total += n\n", "t");
        let b = r.bindings.iter().find(|b| b.name == "total").unwrap();
        assert!(b.accumulates);
    }
}
