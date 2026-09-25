//! Following a snippet's local imports.
//!
//! `python main.py` runs more than `main.py`: every local module it imports
//! runs its top level on import, and whatever functions `main` calls from
//! them run when called. An agent that writes a helper file and then a
//! one-line driver would otherwise show pyxray only the driver. This resolves
//! the snippet's imports the way Python would from the script's directory,
//! analyses the local files it finds, and keeps the effects the snippet
//! actually reaches.
//!
//! What it deliberately does not do: search `sys.path` beyond the script's
//! directory (installed packages are not "the snippet"), follow anything that
//! resolves outside that directory (symlinks and `..` included), execute
//! anything, or chase dynamic imports — those already show up as `Dynamic`.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::effects;
use crate::model::*;

/// Limits on how much following a single analysis may do.
#[derive(Clone, Debug)]
pub struct FollowOpts {
    pub enabled: bool,
    /// Local files analysed, at most.
    pub max_files: usize,
    /// Import hops from the snippet.
    pub max_depth: u8,
    /// A single file larger than this is skipped.
    pub max_bytes: u64,
    /// All followed files together.
    pub max_total: u64,
}

impl Default for FollowOpts {
    fn default() -> Self {
        FollowOpts {
            enabled: true,
            max_files: 24,
            max_depth: 3,
            max_bytes: 256 << 10,
            max_total: 2 << 20,
        }
    }
}

impl FollowOpts {
    pub fn off() -> Self {
        FollowOpts {
            enabled: false,
            ..FollowOpts::default()
        }
    }
}

/// Where a snippet came from, and how far to look beyond it.
#[derive(Clone, Debug, Default)]
pub struct XrayOpts {
    /// The script's own path, when it is a file. Imports resolve from its
    /// directory, as Python's `sys.path[0]` would.
    pub origin: Option<PathBuf>,
    /// The working directory, for a snippet with no file (`-c`, a heredoc,
    /// stdin): Python's `sys.path[0]` is then the current directory.
    pub cwd: Option<PathBuf>,
    pub follow: FollowOpts,
}

/// One module waiting to be analysed.
struct Pending {
    module: String,
    file: PathBuf,
    via_line: u32,
    depth: u8,
    /// What the importer uses from it. `None`: everything (a star import, or
    /// a module imported by a module that is itself only partly reached).
    used: Option<HashSet<String>>,
}

/// Follow `report`'s local imports and fold what they reach into it.
pub fn follow(report: &mut Report, opts: &XrayOpts) {
    if !opts.follow.enabled {
        return;
    }
    let anchor = match (&opts.origin, &opts.cwd) {
        (Some(origin), _) => origin.parent().map(Path::to_path_buf),
        (None, Some(cwd)) => Some(cwd.clone()),
        (None, None) => None,
    };
    let Some(root) = anchor
        .map(|a| {
            if a.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                a
            }
        })
        .and_then(|a| a.canonicalize().ok())
    else {
        return;
    };

    let mut summary = FollowSummary {
        root: root.display().to_string(),
        ..FollowSummary::default()
    };
    let mut visited: HashSet<PathBuf> = HashSet::new();
    if let Some(origin) = opts.origin.as_ref().and_then(|o| o.canonicalize().ok()) {
        visited.insert(origin);
    }
    let mut total: u64 = 0;
    let mut queue: VecDeque<Pending> = VecDeque::new();

    // The snippet itself is the top-level script: relative imports fail there
    // at runtime, so they are noted rather than followed.
    enqueue(
        &report.imports,
        &report.symbols,
        &root,
        None,
        0,
        &mut queue,
        &mut summary,
    );

    let mut linked: Vec<LinkedModule> = Vec::new();
    while let Some(next) = queue.pop_front() {
        let Ok(canonical) = next.file.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(&root) {
            summary.skipped.push((
                next.module.clone(),
                "resolves outside the script's directory".into(),
            ));
            continue;
        }
        if !visited.insert(canonical.clone()) {
            continue;
        }
        if linked.len() >= opts.follow.max_files {
            summary.truncated = true;
            break;
        }
        let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
        if size > opts.follow.max_bytes || total + size > opts.follow.max_total {
            summary.truncated = true;
            summary
                .skipped
                .push((next.module.clone(), format!("too large ({size} bytes)")));
            continue;
        }
        let Ok(bytes) = std::fs::read(&canonical) else {
            continue;
        };
        total += size;
        let text = String::from_utf8_lossy(&bytes);
        let rel = canonical
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| canonical.display().to_string());
        let sub = crate::xray(&text, &rel);

        let (reached, unreached) = reach(&sub, next.used.as_ref(), &rel);
        if next.depth < opts.follow.max_depth {
            let package = canonical.parent().map(Path::to_path_buf);
            enqueue(
                &sub.imports,
                &sub.symbols,
                &root,
                package.as_deref(),
                next.depth + 1,
                &mut queue,
                &mut summary,
            );
        } else if !sub.imports.is_empty() {
            summary.truncated = true;
        }
        linked.push(LinkedModule {
            module: next.module,
            file: rel,
            via_line: next.via_line,
            effects: reached,
            unreached,
        });
    }

    summary.files = linked.len() as u32;
    if linked.is_empty() && summary.skipped.is_empty() {
        return;
    }

    // Fold the reached effects into the verdicts the report gives as a whole.
    let all: Vec<EffectHit> = report
        .effects
        .iter()
        .chain(linked.iter().flat_map(|m| m.effects.iter()))
        .cloned()
        .collect();
    for hit in linked.iter().flat_map(|m| m.effects.iter()) {
        report.spine.effects.insert(hit.effect);
        report.spine.severity = Some(
            report
                .spine
                .severity
                .map_or(hit.severity, |s| s.max(hit.severity)),
        );
    }
    report.metrics.risk = crate::digest::risk(&all);
    report.risk_terms = crate::digest::risk_terms(&all);
    if linked.iter().any(|m| !m.effects.is_empty()) {
        let (synopsis, headline) = crate::digest::synopsis(&all, &report.metrics, &report.spine);
        report.meta.synopsis = synopsis;
        report.meta.headline = headline;
    }
    report.linked = linked;
    report.follow = Some(summary);
}

/// Queue the local modules among `imports`. `package` is the importing
/// module's own directory, for relative imports; `None` for the top-level
/// script, where they cannot work.
fn enqueue(
    imports: &[ImportInfo],
    symbols: &[SymbolUse],
    root: &Path,
    package: Option<&Path>,
    depth: u8,
    queue: &mut VecDeque<Pending>,
    summary: &mut FollowSummary,
) {
    for imp in imports {
        let level = imp.module.chars().take_while(|c| *c == '.').count();
        let dotted = &imp.module[level..];
        let names: Vec<&str> = imp
            .names
            .iter()
            .map(|n| n.split(" as ").next().unwrap_or(n).trim())
            .collect();
        let star = names.contains(&"*");

        let base = if level == 0 {
            let top = dotted.split('.').next().unwrap_or("");
            if top.is_empty() || effects::is_stdlib(top) {
                continue;
            }
            root.to_path_buf()
        } else {
            let Some(pkg) = package else {
                summary.skipped.push((
                    imp.module.clone(),
                    "relative import in the top-level script".into(),
                ));
                continue;
            };
            let mut dir = pkg.to_path_buf();
            for _ in 1..level {
                if !dir.pop() {
                    break;
                }
            }
            dir
        };

        // Everything on the dotted path runs: each package's __init__ and
        // the module at the end.
        let parts: Vec<&str> = dotted.split('.').filter(|p| !p.is_empty()).collect();
        let mut dir = base.clone();
        let mut chain: Vec<(String, PathBuf)> = Vec::new();
        let mut resolved = parts.is_empty();
        for (i, part) in parts.iter().enumerate() {
            let pkg_init = dir.join(part).join("__init__.py");
            let module = dir.join(format!("{part}.py"));
            let so_far = format!("{}{}", ".".repeat(level), parts[..=i].join("."));
            if pkg_init.is_file() {
                chain.push((so_far, pkg_init));
                dir = dir.join(part);
            } else if module.is_file() {
                chain.push((so_far, module));
                resolved = i == parts.len() - 1;
                break;
            } else if dir.join(part).is_dir() {
                dir = dir.join(part); // a namespace package: nothing to run
            } else {
                break;
            }
            if i == parts.len() - 1 {
                resolved = true;
            }
        }
        if !resolved {
            // Not a local module: an installed package, or a typo. Either way
            // not the snippet's own code.
            continue;
        }

        // `import a.b` runs a/__init__ and a/b; `from a import b` may import a
        // submodule `b` too.
        let module_path = format!("{}{}", ".".repeat(level), parts.join("."));
        let used_from_module: HashSet<String> = if star {
            HashSet::new()
        } else {
            used_names(&module_path, &names, symbols)
        };
        let n = chain.len();
        for (i, (module, file)) in chain.into_iter().enumerate() {
            let last = i + 1 == n;
            queue.push_back(Pending {
                module,
                file,
                via_line: imp.line,
                depth,
                used: if last && !star {
                    Some(used_from_module.clone())
                } else if last {
                    None
                } else {
                    Some(HashSet::new())
                },
            });
        }
        if !star {
            for name in &names {
                let sub_init = dir.join(name).join("__init__.py");
                let sub = dir.join(format!("{name}.py"));
                let file = if sub_init.is_file() {
                    sub_init
                } else if sub.is_file() {
                    sub
                } else {
                    continue;
                };
                let sub_path = if module_path.is_empty() || module_path.ends_with('.') {
                    format!("{module_path}{name}")
                } else {
                    format!("{module_path}.{name}")
                };
                let used = used_names(&sub_path, &[], symbols);
                queue.push_back(Pending {
                    module: sub_path,
                    file,
                    via_line: imp.line,
                    depth,
                    used: Some(used),
                });
            }
        }
    }
}

/// The top-level names of `module` the importer uses: every name it imported
/// from it by name, and every `module.name` it calls.
fn used_names(module: &str, imported: &[&str], symbols: &[SymbolUse]) -> HashSet<String> {
    let mut used: HashSet<String> = imported.iter().map(|s| s.to_string()).collect();
    let prefixes = [format!("{module}."), format!(".{module}.")];
    for sym in symbols {
        for prefix in &prefixes {
            if let Some(rest) = sym.canonical.strip_prefix(prefix.as_str()) {
                if let Some(first) = rest.split('.').next() {
                    used.insert(first.to_string());
                }
            }
        }
    }
    used
}

/// Split a linked module's hits into those the importer reaches and a count
/// of the rest. Module-level code always runs on import; a top-level def or
/// class is reached when the importer uses it by name, or when the module's
/// own top level calls it.
fn reach(sub: &Report, used: Option<&HashSet<String>>, rel: &str) -> (Vec<EffectHit>, u32) {
    // Top-level definitions, by line range.
    let defs: Vec<(String, u32, u32)> = sub
        .spine
        .children
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Def | NodeKind::Class))
        .filter_map(|n| {
            let name = sub
                .bindings
                .iter()
                .find(|b| {
                    b.line == n.line
                        && b.scope == "<module>"
                        && matches!(b.kind, BindKind::Def | BindKind::Class)
                })?
                .name
                .clone();
            Some((name, n.line, n.end_line))
        })
        .collect();
    let inside = |line: u32| defs.iter().find(|(_, a, b)| line >= *a && line <= *b);

    // Names the module's own top level calls, read off its top-level
    // statements: a bare `main()` at the bottom runs on import. Branches are
    // left out, and with them `if __name__ == "__main__": main()`, which
    // does not.
    let top_level: Vec<&str> = sub
        .spine
        .children
        .iter()
        .filter(|n| !matches!(n.kind, NodeKind::Def | NodeKind::Class | NodeKind::Branch))
        .map(|n| n.label.as_str())
        .collect();
    let called_at_top: HashSet<&str> = defs
        .iter()
        .map(|(name, _, _)| name.as_str())
        .filter(|name| {
            let call = format!("{name}(");
            top_level.iter().any(|label| {
                label.match_indices(&call).any(|(i, _)| {
                    i == 0
                        || !label[..i]
                            .ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
                })
            })
        })
        .collect();

    let mut reached = Vec::new();
    let mut unreached = 0u32;
    for hit in &sub.effects {
        let keep = match inside(hit.line) {
            None => true,
            Some((name, _, _)) => {
                used.is_none_or(|u| u.contains(name)) || called_at_top.contains(name.as_str())
            }
        };
        if keep {
            let mut hit = hit.clone();
            hit.file = Some(rel.to_string());
            reached.push(hit);
        } else {
            unreached += 1;
        }
    }
    (reached, unreached)
}
