//! Walks the AST once and produces everything in [`Report`].
//!
//! The walk is deliberately hand-written rather than driven by the visitor
//! trait: the spine needs to know, at every point, which statements are
//! *children* of the current one and which expressions belong to its header,
//! and that distinction is the whole shape of the output.

use std::collections::{HashMap, HashSet};

use ruff_python_ast::{
    ExceptHandler, Expr, ExprAttribute, ExprCall, ExprContext, ExprSubscript, Stmt,
};
use ruff_text_size::Ranged;

use crate::effects::{self, TargetArg};
use crate::model::*;
use crate::source::{brief_expr, clip, param_names, squeeze, stmt_line, Src};

const LABEL: usize = 64;
const TARGET: usize = 44;
const ORIGIN: usize = 40;

/// How deep an expression may nest before the walk stops descending. Python
/// itself refuses to parse much past a couple of hundred levels; this is the
/// analyser's own stack budget, well inside what [`crate::xray_guarded`]
/// provides, and anything deeper is not code a person wrote.
const MAX_EXPR_DEPTH: u32 = 256;
/// The same budget for statement nesting.
const MAX_STMT_DEPTH: u16 = 96;
/// Steps a single `resolve` may take down an attribute/call chain.
const RESOLVE_FUEL: u8 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Class,
    Function,
}

/// One lexical scope: which names it binds. Python's rule is the useful one
/// here — a function sees its own names and the module's, but *not* the class
/// body it sits inside — and that is exactly the difference between "this file
/// defines its own `open`" and "some method somewhere is called `open`".
struct Scope {
    kind: ScopeKind,
    names: HashSet<String>,
}

pub struct Analyzer<'a> {
    src: &'a Src<'a>,
    /// Local name → canonical dotted path it stands for (`np` → `numpy`).
    aliases: HashMap<String, String>,
    /// Local name → what we think it holds, for resolving method calls.
    kinds: HashMap<String, ValueKind>,
    /// Local name → the string it was assigned, so `rmtree(STAGING)` can show
    /// the path it is actually about to delete.
    consts: HashMap<String, String>,
    /// Local name → a short rendering of what it was bound to, used as the
    /// target when a method call has no argument that names one.
    origins: HashMap<String, String>,
    /// Root modules reached by an import, for telling a library call apart
    /// from a method on some local object.
    imported_roots: HashSet<String>,
    /// The lexical scope stack; `scopes[0]` is the module. Used to decide
    /// whether a bare name is the file's own definition or a builtin.
    scopes: Vec<Scope>,
    /// Local name → the effectful callable it was assigned (`f = os.system`),
    /// so the call through the alias is still seen for what it is.
    callable_aliases: HashMap<String, String>,
    /// Modules pulled in with `from m import *`, in order, so a bare
    /// `system(...)` can be tried as `os.system`.
    star_modules: Vec<String>,
    /// Local name → the secret it holds or was derived from. Scoped like
    /// `consts`: a token read inside one function is not the module's.
    taint: HashMap<String, Taint>,
    /// Current expression nesting, against [`MAX_EXPR_DEPTH`].
    expr_depth: u32,
    /// Only warn once per snippet about hitting a depth limit.
    depth_warned: bool,
    imports: Vec<ImportInfo>,
    effects: Vec<EffectHit>,
    symbols: HashMap<String, SymbolUse>,
    bindings: Vec<Binding>,
    binding_at: HashMap<String, usize>,
    reads: HashMap<String, u32>,
    diagnostics: Vec<Diagnostic>,
    metrics: Metrics,
    scope: Vec<String>,
    loop_depth: u32,
    next_id: u32,
    max_depth: u16,
}

impl<'a> Analyzer<'a> {
    pub fn new(src: &'a Src<'a>) -> Self {
        Analyzer {
            src,
            aliases: HashMap::new(),
            kinds: HashMap::new(),
            consts: HashMap::new(),
            origins: HashMap::new(),
            imported_roots: HashSet::new(),
            scopes: vec![Scope {
                kind: ScopeKind::Module,
                names: HashSet::new(),
            }],
            callable_aliases: HashMap::new(),
            star_modules: Vec::new(),
            taint: HashMap::new(),
            expr_depth: 0,
            depth_warned: false,
            imports: Vec::new(),
            effects: Vec::new(),
            symbols: HashMap::new(),
            bindings: Vec::new(),
            binding_at: HashMap::new(),
            reads: HashMap::new(),
            diagnostics: Vec::new(),
            metrics: Metrics::default(),
            scope: vec!["<module>".into()],
            loop_depth: 0,
            next_id: 0,
            max_depth: 0,
        }
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn scope_name(&self) -> String {
        if self.scope.len() == 1 {
            "<module>".into()
        } else {
            self.scope[1..].join(".")
        }
    }

    // ---------------------------------------------------------------- pass 1

    /// Collect imports and top-level names before the main walk, so a symbol
    /// used above its own import (or shadowing a builtin) still resolves.
    pub fn prescan(&mut self, body: &[Stmt], deferred: bool) {
        self.prescan_in(body, deferred, true);
    }

    /// `module_level` is whether names bound here belong to the module scope.
    /// Imports are hoisted from anywhere (an alias is an alias wherever it is
    /// declared); names are not, because a `def eval` inside some class must
    /// not hide the builtin from the rest of the file.
    fn prescan_in(&mut self, body: &[Stmt], deferred: bool, module_level: bool) {
        for stmt in body {
            match stmt {
                Stmt::Import(imp) => {
                    for alias in &imp.names {
                        let module = alias.name.to_string();
                        let local =
                            alias
                                .asname
                                .as_ref()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| {
                                    module.split('.').next().unwrap_or(&module).to_string()
                                });
                        let target = match &alias.asname {
                            Some(_) => module.clone(),
                            // `import os.path` binds `os`, not `os.path`.
                            None => module.split('.').next().unwrap_or(&module).to_string(),
                        };
                        self.imported_roots
                            .insert(target.split('.').next().unwrap_or(&target).to_string());
                        self.aliases.insert(local.clone(), target);
                        self.imports.push(ImportInfo {
                            module: module.clone(),
                            names: Vec::new(),
                            alias: alias.asname.as_ref().map(|a| a.to_string()),
                            line: self.src.line_of(stmt.range().start()),
                            third_party: !effects::is_stdlib(&module),
                            deferred,
                        });
                    }
                }
                Stmt::ImportFrom(imp) => {
                    let module = match &imp.module {
                        Some(m) => format!("{}{m}", ".".repeat(imp.level as usize)),
                        None => ".".repeat(imp.level.max(1) as usize),
                    };
                    let mut names = Vec::new();
                    for alias in &imp.names {
                        let name = alias.name.to_string();
                        let local = alias
                            .asname
                            .as_ref()
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| name.clone());
                        self.imported_roots
                            .insert(module.split('.').next().unwrap_or(&module).to_string());
                        if name == "*" {
                            self.star_modules.push(module.clone());
                        } else {
                            self.aliases
                                .insert(local.clone(), format!("{module}.{name}"));
                        }
                        names.push(match &alias.asname {
                            Some(a) => format!("{name} as {a}"),
                            None => name,
                        });
                    }
                    self.imports.push(ImportInfo {
                        module: module.clone(),
                        names,
                        alias: None,
                        line: self.src.line_of(stmt.range().start()),
                        third_party: imp.level == 0 && !effects::is_stdlib(&module),
                        deferred,
                    });
                }
                Stmt::FunctionDef(f) => {
                    if module_level {
                        self.scopes[0].names.insert(f.name.to_string());
                    }
                    self.prescan_in(&f.body, true, false);
                }
                Stmt::ClassDef(c) => {
                    if module_level {
                        self.scopes[0].names.insert(c.name.to_string());
                    }
                    self.prescan_in(&c.body, true, false);
                }
                Stmt::If(s) => {
                    self.prescan_in(&s.body, deferred, module_level);
                    for clause in &s.elif_else_clauses {
                        self.prescan_in(&clause.body, deferred, module_level);
                    }
                }
                Stmt::Try(s) => {
                    self.prescan_in(&s.body, true, module_level);
                    for handler in &s.handlers {
                        let ExceptHandler::ExceptHandler(h) = handler;
                        self.prescan_in(&h.body, true, module_level);
                    }
                    self.prescan_in(&s.orelse, true, module_level);
                    self.prescan_in(&s.finalbody, true, module_level);
                }
                Stmt::For(s) => self.prescan_in(&s.body, deferred, module_level),
                Stmt::While(s) => self.prescan_in(&s.body, deferred, module_level),
                Stmt::With(s) => self.prescan_in(&s.body, deferred, module_level),
                Stmt::Assign(s) if module_level => {
                    for t in &s.targets {
                        if let Expr::Name(n) = t {
                            self.scopes[0].names.insert(n.id.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The names a function or class body binds directly — one level, no
    /// nested definitions — so a local used above its own assignment still
    /// counts as local. Called when the walk enters the body.
    fn prescan_local(&mut self, body: &[Stmt]) {
        let mut names = Vec::new();
        for stmt in body {
            match stmt {
                Stmt::FunctionDef(f) => names.push(f.name.to_string()),
                Stmt::ClassDef(c) => names.push(c.name.to_string()),
                Stmt::Assign(s) => {
                    for t in &s.targets {
                        if let Expr::Name(n) = t {
                            names.push(n.id.to_string());
                        }
                    }
                }
                Stmt::AnnAssign(s) => {
                    if let Expr::Name(n) = &*s.target {
                        names.push(n.id.to_string());
                    }
                }
                _ => {}
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.names.extend(names);
        }
    }

    fn push_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope {
            kind,
            names: HashSet::new(),
        });
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Whether `id` is bound in a scope the current position can see, by
    /// Python's rules: the innermost scope, every enclosing *function*, and the
    /// module — but not an enclosing class body.
    fn is_bound_locally(&self, id: &str) -> bool {
        let last = self.scopes.len() - 1;
        self.scopes.iter().enumerate().rev().any(|(i, scope)| {
            (i == last || scope.kind != ScopeKind::Class) && scope.names.contains(id)
        })
    }

    /// The value-tracking tables a nested scope may change but must not leak:
    /// a `p = '/tmp/safe'` inside one function is not the `p` at module level.
    fn snapshot(&self) -> ScopeState {
        ScopeState {
            kinds: self.kinds.clone(),
            consts: self.consts.clone(),
            origins: self.origins.clone(),
            callable_aliases: self.callable_aliases.clone(),
            taint: self.taint.clone(),
        }
    }

    fn restore(&mut self, state: ScopeState) {
        self.kinds = state.kinds;
        self.consts = state.consts;
        self.origins = state.origins;
        self.callable_aliases = state.callable_aliases;
        self.taint = state.taint;
    }

    /// One warning per snippet when a depth limit is hit; the rest of the walk
    /// simply does not descend, which is honest ("did not look here") and
    /// cheap.
    fn depth_limit(&mut self, what: &str, limit: u32, range: ruff_text_size::TextRange) {
        if self.depth_warned {
            return;
        }
        self.depth_warned = true;
        self.diagnostics.push(Diagnostic {
            level: DiagLevel::Warning,
            message: format!(
                "{what} nested deeper than {limit} levels; the inner part was not analysed"
            ),
            line: self.src.line_of(range.start()),
            col: self.src.col_of(range.start()),
        });
    }

    // ------------------------------------------------------------ resolution

    /// The canonical namespace of an expression's *value*: what you would have
    /// to write, with no aliases in play, to name the same thing.
    fn resolve(&self, e: &Expr) -> Option<String> {
        self.resolve_fuel(e, RESOLVE_FUEL)
    }

    fn resolve_fuel(&self, e: &Expr, fuel: u8) -> Option<String> {
        let fuel = fuel.checked_sub(1)?;
        match e {
            Expr::Name(n) => {
                let id = n.id.as_str();
                if let Some(path) = self.callable_aliases.get(id) {
                    return Some(path.clone());
                }
                if let Some(kind) = self.kinds.get(id) {
                    if let Some(prefix) = effects::kind_prefix(*kind) {
                        return Some(prefix.to_string());
                    }
                }
                if let Some(path) = self.aliases.get(id) {
                    return Some(path.clone());
                }
                // `from os import *` then `system(...)`: try each star-imported
                // module for a symbol we know something about.
                if !self.star_modules.is_empty() && !self.is_bound_locally(id) {
                    for module in &self.star_modules {
                        let candidate = format!("{module}.{id}");
                        if effects::lookup(&candidate).is_some()
                            || effects::produces(&candidate) != ValueKind::Unknown
                        {
                            return Some(candidate);
                        }
                    }
                }
                Some(id.to_string())
            }
            Expr::Attribute(a) => self
                .resolve_fuel(&a.value, fuel)
                .map(|base| format!("{base}.{}", a.attr)),
            Expr::Call(c) => {
                let base = self.resolve_fuel(&c.func, fuel)?;
                // `__import__("os").system(...)`: the call *is* the module.
                if base == "__import__" || base == "importlib.import_module" {
                    if let Some(module) = c.arguments.args.first().and_then(Self::str_lit) {
                        return Some(module.to_string());
                    }
                }
                match effects::kind_prefix(effects::produces(&base)) {
                    Some(prefix) => Some(prefix.to_string()),
                    None => Some(base),
                }
            }
            // `paths[0].read_text()` and `os.environ["X"]` both want the
            // container's identity, not the element's.
            Expr::Subscript(s) => self.resolve_fuel(&s.value, fuel),
            Expr::Await(a) => self.resolve_fuel(&a.value, fuel),
            // `out / "flexible.json"` is still a path; so is the reverse.
            Expr::BinOp(b) => {
                let left = self.resolve_fuel(&b.left, fuel);
                if left.as_deref() == Some("<path>") {
                    return left;
                }
                let right = self.resolve_fuel(&b.right, fuel);
                if right.as_deref() == Some("<path>") {
                    return right;
                }
                None
            }
            _ => None,
        }
    }

    /// A call to a name the file defines itself is the user's function, not a
    /// builtin of the same name — unless that name is an import or an alias
    /// for something we know, in which case it is exactly what it says.
    fn is_shadowed(&self, e: &Expr) -> bool {
        match e {
            Expr::Name(n) => {
                let id = n.id.as_str();
                self.is_bound_locally(id)
                    && !self.aliases.contains_key(id)
                    && !self.callable_aliases.contains_key(id)
            }
            _ => false,
        }
    }

    /// The effectful callable an assignment's right-hand side names, if any:
    /// `os.system`, `open`, `getattr(os, "remove")`, `Path`.
    fn callable_target(&self, origin: &Expr) -> Option<String> {
        let path = match origin {
            Expr::Name(_) | Expr::Attribute(_) => self.resolve(origin)?,
            Expr::Call(c) => {
                let func = self.resolve(&c.func)?;
                if func != "getattr" || c.arguments.args.len() < 2 {
                    return None;
                }
                let base = self.resolve(&c.arguments.args[0])?;
                let attr = Self::str_lit(&c.arguments.args[1])?;
                format!("{base}.{attr}")
            }
            _ => return None,
        };
        let known = effects::lookup(&path).is_some()
            || effects::produces(&path) != ValueKind::Unknown
            || path == "open"
            || path.starts_with('<');
        known.then_some(path)
    }

    // --------------------------------------------------------------- effects

    #[allow(clippy::too_many_arguments)]
    fn push_hit(
        &mut self,
        effect: Effect,
        severity: Severity,
        verb: &str,
        symbol: &str,
        target: Option<String>,
        range: ruff_text_size::TextRange,
        note: Option<String>,
    ) {
        let snippet = clip(&squeeze(self.src.slice(range)), 72);
        self.effects.push(EffectHit {
            effect,
            severity,
            verb: verb.to_string(),
            symbol: symbol.to_string(),
            target,
            line: self.src.line_of(range.start()),
            col: self.src.col_of(range.start()),
            snippet,
            note,
            resolved: None,
            flow: None,
            file: None,
        });
    }

    /// The literal text of a string expression, if it is one.
    fn str_lit(e: &Expr) -> Option<&str> {
        match e {
            Expr::StringLiteral(s) => Some(s.value.to_str()),
            _ => None,
        }
    }

    /// Render a call argument as the *thing being acted on*: a bare path, a
    /// URL, a joined command line — not a Python expression where we can help
    /// it, because the reader cares about the value, not the syntax.
    fn target_text(&self, e: &Expr) -> String {
        if let Some(s) = Self::str_lit(e) {
            return clip(&squeeze(s), TARGET);
        }
        // `json.load(open("cfg.json"))` is about cfg.json, and naming it so
        // means the read and the parse are counted as one file, not two.
        if let Expr::Call(inner) = e {
            let opens = matches!(
                self.resolve(&inner.func).as_deref(),
                Some("open" | "io.open" | "codecs.open" | "gzip.open" | "bz2.open" | "lzma.open")
            );
            if opens {
                if let Some(file) = inner.arguments.find_argument_value("file", 0) {
                    return self.target_text(file);
                }
            }
        }
        // A name bound to a literal earlier in the file is far more useful
        // shown as its value than as its name.
        if let Expr::Name(n) = e {
            if let Some(value) = self.consts.get(n.id.as_str()) {
                return clip(&squeeze(value), TARGET);
            }
        }
        let elts: Option<&[Expr]> = match e {
            Expr::List(l) => Some(&l.elts),
            Expr::Tuple(t) => Some(&t.elts),
            _ => None,
        };
        if let Some(elts) = elts {
            if !elts.is_empty() && elts.iter().all(|x| Self::str_lit(x).is_some()) {
                let joined = elts
                    .iter()
                    .filter_map(Self::str_lit)
                    .collect::<Vec<_>>()
                    .join(" ");
                return clip(&squeeze(&joined), TARGET);
            }
        }
        brief_expr(self.src, e, TARGET)
    }

    fn extract_target(&self, call: &ExprCall, spec: TargetArg) -> Option<String> {
        let e = match spec {
            TargetArg::None => return None,
            TargetArg::Pos(i) => call.arguments.args.get(i)?,
            TargetArg::Named(name, i) => call.arguments.find_argument_value(name, i)?,
        };
        Some(self.target_text(e))
    }

    /// Scan a call's *own* string arguments for shell fragments and paths that
    /// turn a routine operation into one worth reading closely. Only literals
    /// directly in this call's arguments count: a nested call's arguments are
    /// that call's business, and scanning the whole span would make every
    /// enclosing call inherit them (and cost quadratic time doing it).
    fn hazards(&self, call: &ExprCall) -> Vec<&'static str> {
        fn scan(text: &str, found: &mut Vec<&'static str>) {
            let lower = text.to_ascii_lowercase();
            for (needle, why) in effects::HAZARD_STRINGS {
                if lower.contains(needle) && !found.contains(why) {
                    found.push(*why);
                }
            }
        }
        let mut found = Vec::new();
        let args = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value));
        for arg in args {
            match arg {
                Expr::StringLiteral(s) => scan(s.value.to_str(), &mut found),
                Expr::FString(f) => {
                    for lit in f.value.elements().filter_map(|e| e.as_literal()) {
                        scan(&lit.value, &mut found);
                    }
                }
                Expr::Name(n) => {
                    if let Some(value) = self.consts.get(n.id.as_str()) {
                        scan(value, &mut found);
                    }
                }
                Expr::List(ruff_python_ast::ExprList { elts, .. })
                | Expr::Tuple(ruff_python_ast::ExprTuple { elts, .. }) => {
                    // A command given as a list reads as one command line.
                    let joined = elts
                        .iter()
                        .filter_map(Self::str_lit)
                        .collect::<Vec<_>>()
                        .join(" ");
                    scan(&joined, &mut found);
                }
                _ => {}
            }
        }
        found
    }

    /// Hazard strings only ever escalate an effect that touches the world:
    /// `print("run: pip install foo")` is help text, not a package install.
    fn hazard_applies(effect: Effect) -> bool {
        matches!(
            effect,
            Effect::Process
                | Effect::FsRead
                | Effect::FsWrite
                | Effect::FsDelete
                | Effect::Net
                | Effect::Dynamic
        )
    }

    // ------------------------------------------------------------- secrets

    /// The secret an expression carries, if any. Structural and cheap: it
    /// looks at what the expression is made of, not at what runs.
    fn taint_of(&self, e: &Expr) -> Option<Taint> {
        self.taint_fuel(e, TAINT_FUEL)
    }

    fn taint_fuel(&self, e: &Expr, fuel: u32) -> Option<Taint> {
        let f = fuel.checked_sub(1)?;
        match e {
            Expr::Name(n) => self.taint.get(n.id.as_str()).cloned(),
            Expr::Subscript(sub) => self
                .env_secret(sub)
                .or_else(|| self.taint_fuel(&sub.value, f)),
            Expr::Attribute(a) => self.taint_fuel(&a.value, f),
            Expr::Call(c) => self.call_taint(c, f),
            Expr::FString(fs) => fs
                .value
                .elements()
                .filter_map(|el| el.as_interpolation())
                .find_map(|x| self.taint_fuel(&x.expression, f)),
            Expr::BinOp(b) => self
                .taint_fuel(&b.left, f)
                .or_else(|| self.taint_fuel(&b.right, f)),
            Expr::Dict(d) => d.items.iter().find_map(|it| {
                it.key
                    .as_ref()
                    .and_then(|k| self.taint_fuel(k, f))
                    .or_else(|| self.taint_fuel(&it.value, f))
            }),
            Expr::List(l) => l.elts.iter().find_map(|x| self.taint_fuel(x, f)),
            Expr::Tuple(t) => t.elts.iter().find_map(|x| self.taint_fuel(x, f)),
            Expr::Set(st) => st.elts.iter().find_map(|x| self.taint_fuel(x, f)),
            Expr::If(i) => self
                .taint_fuel(&i.body, f)
                .or_else(|| self.taint_fuel(&i.orelse, f)),
            Expr::Await(a) => self.taint_fuel(&a.value, f),
            Expr::Starred(st) => self.taint_fuel(&st.value, f),
            Expr::Named(n) => self.taint_fuel(&n.value, f),
            _ => None,
        }
    }

    /// `os.environ["GITHUB_TOKEN"]`, as a secret.
    fn env_secret(&self, sub: &ExprSubscript) -> Option<Taint> {
        if self.resolve(&sub.value).as_deref() != Some("os.environ") {
            return None;
        }
        let name = Self::str_lit(&sub.slice)?;
        effects::is_secret_name(name).then(|| Taint {
            secret: SecretKind::Env,
            source: format!("env {name}"),
            line: self.src.line_of(sub.range().start()),
        })
    }

    fn call_taint(&self, call: &ExprCall, f: u32) -> Option<Taint> {
        let canonical = self.resolve(&call.func).unwrap_or_default();
        let line = self.src.line_of(call.range().start());
        // Sources.
        match canonical.as_str() {
            "os.getenv" | "os.environ.get" | "os.environ.pop" | "dotenv.get_key" => {
                let arg = if canonical == "dotenv.get_key" { 1 } else { 0 };
                let name = call.arguments.args.get(arg).and_then(Self::str_lit)?;
                return effects::is_secret_name(name).then(|| Taint {
                    secret: SecretKind::Env,
                    source: format!("env {name}"),
                    line,
                });
            }
            "keyring.get_password" | "keyring.get_credential" => {
                return Some(Taint {
                    secret: SecretKind::Keyring,
                    source: "keyring".into(),
                    line,
                });
            }
            "dotenv.dotenv_values" => {
                let path = call
                    .arguments
                    .args
                    .first()
                    .and_then(|a| self.secret_path_of(a, f))
                    .unwrap_or_else(|| ".env".into());
                return Some(Taint {
                    secret: SecretKind::File,
                    source: format!("file {path}"),
                    line,
                });
            }
            "open" | "io.open" | "builtins.open" | "codecs.open" => {
                if let Some(path) = call
                    .arguments
                    .find_argument_value("file", 0)
                    .and_then(|a| self.secret_path_of(a, f))
                {
                    return Some(Taint {
                        secret: SecretKind::File,
                        source: format!("file {path}"),
                        line,
                    });
                }
            }
            _ => {}
        }
        if TAINT_CLEAN_CALLS.contains(&canonical.as_str()) {
            return None;
        }
        // `Path("~/.ssh/id_rsa").read_text()`: reading a path that names a
        // credential file.
        if let Expr::Attribute(attr) = &*call.func {
            let method = attr.attr.as_str();
            if matches!(
                method,
                "read_text" | "read_bytes" | "open" | "read" | "readlines" | "readline"
            ) {
                if let Some(path) = self.secret_path_of(&attr.value, f) {
                    return Some(Taint {
                        secret: SecretKind::File,
                        source: format!("file {path}"),
                        line,
                    });
                }
            }
            // Any method on a secret — `.encode()`, `.strip()`, `.read()` on a
            // key file's handle — still carries it.
            if let Some(t) = self.taint_fuel(&attr.value, f) {
                return Some(t);
            }
        }
        // Otherwise, what a request, a command or a file read *returns* is new
        // data, not the secret that went in: the response to an authenticated
        // request is not the token. Only transformations carry a secret on.
        if let Some(rule) = effects::lookup(&canonical) {
            if matches!(
                rule.effect,
                Effect::Net
                    | Effect::Process
                    | Effect::FsRead
                    | Effect::FsWrite
                    | Effect::FsDelete
                    | Effect::Dynamic
            ) {
                return None;
            }
        }
        // Anything computed from a secret carries it: base64, json.dumps, a
        // format call, a helper of the snippet's own.
        call.arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value))
            .find_map(|a| self.taint_fuel(a, f))
    }

    /// The path an expression names, when it names a credential file:
    /// a literal, a name bound to one, or one wrapped in `expanduser`,
    /// `os.path.join`, `Path(...)` or `/`.
    fn secret_path_of(&self, e: &Expr, fuel: u32) -> Option<String> {
        let f = fuel.checked_sub(1)?;
        let text = match e {
            Expr::StringLiteral(s) => Some(s.value.to_str().to_string()),
            Expr::Name(n) => self.consts.get(n.id.as_str()).cloned(),
            Expr::FString(fs) => Some(
                fs.value
                    .elements()
                    .filter_map(|el| el.as_literal())
                    .map(|l| l.value.to_string())
                    .collect::<Vec<_>>()
                    .join("*"),
            ),
            Expr::BinOp(b) => {
                let l = self.secret_path_of(&b.left, f);
                if l.is_some() {
                    return l;
                }
                let parts: Vec<String> = [&*b.left, &*b.right]
                    .iter()
                    .filter_map(|x| Self::str_lit(x).map(str::to_string))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("/"))
            }
            Expr::Call(c) => {
                let canonical = self.resolve(&c.func).unwrap_or_default();
                let wraps = matches!(
                    canonical.as_str(),
                    "os.path.expanduser"
                        | "os.path.join"
                        | "os.path.abspath"
                        | "os.path.expandvars"
                        | "pathlib.Path"
                        | "pathlib.PurePath"
                        | "Path"
                ) || canonical.ends_with(".expanduser")
                    || canonical.ends_with(".joinpath");
                if !wraps {
                    return None;
                }
                let mut parts: Vec<String> = Vec::new();
                if let Expr::Attribute(a) = &*c.func {
                    if let Some(base) = self.secret_path_of(&a.value, f) {
                        return Some(base);
                    }
                    if let Some(lit) = Self::str_lit(&a.value) {
                        parts.push(lit.to_string());
                    }
                }
                for a in &c.arguments.args {
                    if let Some(p) = self.secret_path_of(a, f) {
                        return Some(p);
                    }
                    if let Some(lit) = Self::str_lit(a) {
                        parts.push(lit.to_string());
                    } else if let Expr::Name(n) = a {
                        if let Some(v) = self.consts.get(n.id.as_str()) {
                            parts.push(v.clone());
                        }
                    }
                }
                (!parts.is_empty()).then(|| parts.join("/"))
            }
            _ => None,
        }?;
        effects::is_secret_path(&text).then(|| clip(&squeeze(&text), TARGET))
    }

    /// The first secret among a call's own arguments, receiver included.
    fn secret_in_call(&self, call: &ExprCall) -> Option<Taint> {
        let args = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value));
        for a in args {
            if let Some(t) = self.taint_of(a) {
                return Some(t);
            }
        }
        None
    }

    /// If a secret reaches this call and the call sends, runs or writes,
    /// attach the flow to the hit just pushed and raise it accordingly.
    fn mark_flow(&mut self, call: &ExprCall, effect: Effect) {
        let kind = match effect {
            Effect::Net => FlowKind::Exfiltration,
            Effect::Process => FlowKind::SecretToProcess,
            Effect::FsWrite => FlowKind::SecretToDisk,
            _ => return,
        };
        let Some(taint) = self.secret_in_call(call) else {
            return;
        };
        let Some(hit) = self.effects.last_mut() else {
            return;
        };
        let where_to = hit.target.clone().unwrap_or_else(|| "somewhere".into());
        let what = format!("{} (line {})", taint.source, taint.line);
        let (sev, note) = match (kind, taint.secret) {
            (FlowKind::Exfiltration, SecretKind::Env) => (
                Severity::Notable,
                format!("sends the secret from {what} to {where_to}"),
            ),
            (FlowKind::Exfiltration, _) => (
                Severity::Caution,
                format!("sends the contents of {what} to {where_to}"),
            ),
            (FlowKind::SecretToProcess, _) => (
                Severity::Caution,
                format!("passes the secret from {what} to a command"),
            ),
            (FlowKind::SecretToDisk, _) => (
                Severity::Notable,
                format!("writes the secret from {what} to disk"),
            ),
        };
        hit.severity = hit.severity.max(sev);
        hit.note = Some(match hit.note.take() {
            Some(existing) => format!("{note}; {existing}"),
            None => note,
        });
        hit.flow = Some(Flow {
            kind,
            secret: taint.secret,
            source: taint.source,
            source_line: taint.line,
        });
    }

    /// Record the exact target on the hit just pushed, for the effects a
    /// policy can name (paths, hosts, commands, variables) — not for `print`.
    fn set_resolved(&mut self, call: &ExprCall, spec: TargetArg) {
        let wanted = self.effects.last().is_some_and(|h| {
            !matches!(
                h.effect,
                Effect::Stdout | Effect::Random | Effect::Clock | Effect::Compute | Effect::Exit
            )
        });
        if !wanted {
            return;
        }
        let resolved = self.resolved_arg(call, spec);
        if let Some(hit) = self.effects.last_mut() {
            hit.resolved = resolved;
        }
    }

    /// The whole literal an argument holds, unclipped — for policy matching.
    fn resolved_arg(&self, call: &ExprCall, spec: TargetArg) -> Option<String> {
        let e = match spec {
            TargetArg::None => return None,
            TargetArg::Pos(i) => call.arguments.args.get(i)?,
            TargetArg::Named(name, i) => call.arguments.find_argument_value(name, i)?,
        };
        if let Some(s) = Self::str_lit(e) {
            return Some(s.to_string());
        }
        if let Expr::Name(n) = e {
            return self.consts.get(n.id.as_str()).cloned();
        }
        let elts: &[Expr] = match e {
            Expr::List(l) => &l.elts,
            Expr::Tuple(t) => &t.elts,
            _ => return None,
        };
        let parts: Option<Vec<&str>> = elts.iter().map(Self::str_lit).collect();
        parts.filter(|p| !p.is_empty()).map(|p| p.join(" "))
    }

    fn on_call(&mut self, call: &ExprCall) {
        self.metrics.calls += 1;
        if self.is_shadowed(&call.func) {
            return;
        }
        let Some(canonical) = self.resolve(&call.func) else {
            return;
        };
        let display = clip(&squeeze(self.src.slice(call.func.range())), 32);
        let line = self.src.line_of(call.range().start());

        // `open` is the one call whose effect depends on an argument, and it
        // is common enough to be worth special-casing.
        let (rule_path, forced) = if canonical == "open" || canonical == "io.open" {
            let mode = call
                .arguments
                .find_argument_value("mode", 1)
                .and_then(Self::str_lit)
                .unwrap_or("r");
            let (effect, sev, verb) = if mode.contains('w') {
                (Effect::FsWrite, Severity::Notable, "truncate")
            } else if mode.contains('a') {
                (Effect::FsWrite, Severity::Notable, "append")
            } else if mode.contains('x') {
                (Effect::FsWrite, Severity::Notable, "create")
            } else if mode.contains('+') {
                (Effect::FsWrite, Severity::Notable, "update")
            } else {
                (Effect::FsRead, Severity::Info, "read")
            };
            (canonical.clone(), Some((effect, sev, verb)))
        } else {
            (canonical.clone(), None)
        };

        let mut effect_mask = EffectMask::default();
        let hazards = self.hazards(call);

        if let Some((effect, sev, verb)) = forced {
            let target = self.extract_target(call, TargetArg::Named("file", 0));
            let mut sev = sev;
            let mut note = None;
            if !hazards.is_empty() {
                sev = sev.max(Severity::Caution);
                note = Some(hazards.join("; "));
            }
            effect_mask.insert(effect);
            self.push_hit(effect, sev, verb, &rule_path, target, call.range(), note);
            self.set_resolved(call, TargetArg::Named("file", 0));
            self.mark_flow(call, effect);
        } else if let Some(rule) = effects::lookup(&canonical) {
            let target = self
                .extract_target(call, rule.target)
                .or_else(|| self.receiver_origin(&call.func));
            let mut effect = rule.effect;
            let mut sev = rule.sev;
            let mut notes: Vec<String> = rule.note.map(|n| n.to_string()).into_iter().collect();
            if !hazards.is_empty() && Self::hazard_applies(effect) {
                sev = sev.max(Severity::Caution);
                notes.extend(hazards.iter().map(|h| h.to_string()));
            }
            for kw in &call.arguments.keywords {
                let Some(name) = &kw.arg else { continue };
                let value = match &kw.value {
                    Expr::BooleanLiteral(b) if b.value => effects::KwValue::True,
                    Expr::BooleanLiteral(_) => effects::KwValue::False,
                    Expr::NoneLiteral(_) => effects::KwValue::False,
                    Expr::NumberLiteral(n) => match &n.value {
                        ruff_python_ast::Number::Int(i) if i.as_u8() == Some(0) => {
                            effects::KwValue::False
                        }
                        ruff_python_ast::Number::Int(_) => effects::KwValue::True,
                        _ => effects::KwValue::Other,
                    },
                    Expr::Name(_) | Expr::Attribute(_) => match self.resolve(&kw.value) {
                        Some(path) => effects::KwValue::Path(path),
                        None => effects::KwValue::Other,
                    },
                    _ => effects::KwValue::Other,
                };
                if let Some(hazard) = effects::kwarg_hazard(&canonical, name.as_str(), &value) {
                    notes.push(hazard.note.to_string());
                    sev = sev.max(hazard.severity);
                    if let Some(e) = hazard.effect {
                        effect = e;
                    }
                }
            }
            // Reading a variable whose name says "secret" is worth stopping
            // for: it is the one environment read an agent should not be
            // making casually.
            if effect == Effect::Env && (rule.verb == "getenv" || rule.verb == "dotenv") {
                if let Some(name) = target.as_deref() {
                    if effects::is_secret_name(name) {
                        sev = sev.max(Severity::Caution);
                        notes.push("reads a secret from the environment".to_string());
                    }
                }
            }
            effect_mask.insert(effect);
            let note = (!notes.is_empty()).then(|| notes.join("; "));
            self.push_hit(
                effect,
                sev,
                rule.verb,
                &canonical,
                target,
                call.range(),
                note,
            );
            self.set_resolved(call, rule.target);
            self.mark_flow(call, effect);
        } else if let Some(rule) = canonical
            .rsplit_once('.')
            .filter(|(base, _)| !base.starts_with('<'))
            .and_then(|(_, method)| effects::method_lookup(method))
        {
            let target = self
                .extract_target(call, rule.target)
                .or_else(|| self.receiver_origin(&call.func));
            let mut sev = rule.sev;
            let mut notes: Vec<String> = rule.note.map(|n| n.to_string()).into_iter().collect();
            if !hazards.is_empty() && Self::hazard_applies(rule.effect) {
                sev = sev.max(Severity::Caution);
                notes.extend(hazards.iter().map(|h| h.to_string()));
            }
            effect_mask.insert(rule.effect);
            let note = (!notes.is_empty()).then(|| notes.join("; "));
            self.push_hit(
                rule.effect,
                sev,
                rule.verb,
                &canonical,
                target,
                call.range(),
                note,
            );
            self.set_resolved(call, rule.target);
            self.mark_flow(call, rule.effect);
        } else if effects::is_compute_module(&canonical) && canonical.contains('.') {
            effect_mask.insert(Effect::Compute);
            let root = canonical.split('.').next().unwrap_or("").to_string();
            self.push_hit(
                Effect::Compute,
                Severity::Info,
                "compute",
                &canonical,
                Some(root),
                call.range(),
                None,
            );
        }

        // Fold into the symbol inventory whether or not it had an effect: the
        // call list is useful on its own for seeing what a snippet leans on.
        let group = symbol_group(&canonical, self.is_bound_locally(&canonical));
        let origin = self.symbol_origin(&canonical);
        let entry = self
            .symbols
            .entry(canonical.clone())
            .or_insert_with(|| SymbolUse {
                canonical: canonical.clone(),
                display: display.clone(),
                group,
                origin,
                count: 0,
                lines: Vec::new(),
                effects: EffectMask::default(),
            });
        entry.count += 1;
        if !entry.lines.contains(&line) {
            entry.lines.push(line);
        }
        entry.effects.union(effect_mask);
    }

    /// When a rule names no argument as its target, the receiver's own origin
    /// is usually the answer: `fh.write(x)` is about whatever `fh` was opened
    /// on.
    fn receiver_origin(&self, func: &Expr) -> Option<String> {
        let Expr::Attribute(attr) = func else {
            return None;
        };
        match &*attr.value {
            Expr::Name(name) => {
                if let Some(value) = self.consts.get(name.id.as_str()) {
                    return Some(clip(&squeeze(value), TARGET));
                }
                self.origins.get(name.id.as_str()).cloned()
            }
            // `(out / "flexible.json").write_text(...)` names its target
            // inline; show that rather than the bare method.
            other => Some(brief_expr(self.src, other, TARGET)),
        }
    }

    fn symbol_origin(&self, canonical: &str) -> SymbolOrigin {
        if canonical.starts_with('<') {
            return SymbolOrigin::Method;
        }
        match canonical.split_once('.') {
            None => {
                if self.is_bound_locally(canonical) {
                    SymbolOrigin::Local
                } else {
                    SymbolOrigin::Builtin
                }
            }
            Some((root, _)) => {
                if self.imported_roots.contains(root) || effects::is_stdlib(root) {
                    SymbolOrigin::Import
                } else {
                    SymbolOrigin::Method
                }
            }
        }
    }

    fn on_subscript(&mut self, sub: &ExprSubscript) {
        self.on_env_subscript(sub, "getenv", Severity::Notable);
    }

    /// `os.environ[...]` read, written or deleted. Reads of a secret-looking
    /// name escalate; writes are always notable, because an agent setting
    /// `LD_PRELOAD` or `PATH` is changing what everything after it does.
    fn on_env_subscript(&mut self, sub: &ExprSubscript, verb: &str, sev: Severity) -> bool {
        let Some(base) = self.resolve(&sub.value) else {
            return false;
        };
        if base != "os.environ" {
            return false;
        }
        let target = Self::str_lit(&sub.slice).map(|s| s.to_string());
        let mut sev = sev;
        let mut note = None;
        if verb == "getenv" {
            if let Some(name) = target.as_deref() {
                if effects::is_secret_name(name) {
                    sev = Severity::Caution;
                    note = Some("reads a secret from the environment".to_string());
                }
            }
        }
        self.push_hit(
            Effect::Env,
            sev,
            verb,
            "os.environ",
            target,
            sub.range(),
            note,
        );
        true
    }

    fn on_attribute(&mut self, attr: &ExprAttribute) {
        let Some(path) = self.resolve(&attr.value) else {
            return;
        };
        let full = format!("{path}.{}", attr.attr);
        if full == "sys.argv" {
            self.push_hit(
                Effect::Env,
                Severity::Info,
                "argv",
                "sys.argv",
                None,
                attr.range(),
                None,
            );
        }
    }

    // ------------------------------------------------------------ expression

    fn expr(&mut self, e: &Expr) {
        if self.expr_depth >= MAX_EXPR_DEPTH {
            self.depth_limit("an expression", MAX_EXPR_DEPTH, e.range());
            return;
        }
        self.expr_depth += 1;
        self.expr_inner(e);
        self.expr_depth -= 1;
    }

    fn expr_inner(&mut self, e: &Expr) {
        match e {
            Expr::Call(c) => {
                self.on_call(c);
                self.expr(&c.func);
                for a in c.arguments.args.iter() {
                    self.expr(a);
                }
                for k in &c.arguments.keywords {
                    self.expr(&k.value);
                }
            }
            Expr::Subscript(s) => {
                self.on_subscript(s);
                self.expr(&s.value);
                self.expr(&s.slice);
            }
            Expr::Attribute(a) => {
                self.on_attribute(a);
                self.expr(&a.value);
            }
            Expr::Name(n) => {
                if matches!(n.ctx, ExprContext::Load) {
                    *self.reads.entry(n.id.to_string()).or_default() += 1;
                }
            }
            Expr::BoolOp(b) => {
                self.metrics.complexity += b.values.len().saturating_sub(1) as u32;
                for v in &b.values {
                    self.expr(v);
                }
            }
            Expr::Named(n) => {
                if let Expr::Name(name) = &*n.target {
                    let kind = self.value_kind_of(&n.value);
                    self.bind(
                        name.id.as_str(),
                        BindKind::Walrus,
                        kind,
                        &n.value,
                        self.src.line_of(n.range().start()),
                    );
                }
                self.expr(&n.value);
            }
            Expr::BinOp(b) => {
                self.expr(&b.left);
                self.expr(&b.right);
            }
            Expr::UnaryOp(u) => self.expr(&u.operand),
            Expr::Lambda(l) => {
                if let Some(p) = &l.parameters {
                    for d in p.iter_non_variadic_params().filter_map(|p| p.default()) {
                        self.expr(d);
                    }
                }
                self.push_scope(ScopeKind::Function);
                if let Some(p) = &l.parameters {
                    let names = param_names(p);
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.names.extend(
                            names
                                .into_iter()
                                .map(|n| n.trim_start_matches('*').to_string()),
                        );
                    }
                }
                self.expr(&l.body);
                self.pop_scope();
            }
            Expr::If(i) => {
                self.metrics.complexity += 1;
                self.expr(&i.test);
                self.expr(&i.body);
                self.expr(&i.orelse);
            }
            Expr::Dict(d) => {
                for item in &d.items {
                    if let Some(k) = &item.key {
                        self.expr(k);
                    }
                    self.expr(&item.value);
                }
            }
            Expr::Set(s) => self.each(&s.elts),
            Expr::List(l) => self.each(&l.elts),
            Expr::Tuple(t) => self.each(&t.elts),
            Expr::ListComp(c) => {
                self.comprehensions(&c.generators, |a| a.expr(&c.elt));
            }
            Expr::SetComp(c) => {
                self.comprehensions(&c.generators, |a| a.expr(&c.elt));
            }
            Expr::Generator(c) => {
                self.comprehensions(&c.generators, |a| a.expr(&c.elt));
            }
            Expr::DictComp(c) => {
                self.comprehensions(&c.generators, |a| {
                    if let Some(k) = &c.key {
                        a.expr(k);
                    }
                    a.expr(&c.value);
                });
            }
            Expr::Await(a) => self.expr(&a.value),
            Expr::Yield(y) => {
                if let Some(v) = &y.value {
                    self.expr(v);
                }
            }
            Expr::YieldFrom(y) => self.expr(&y.value),
            Expr::Compare(c) => {
                self.expr(&c.left);
                for x in c.comparators.iter() {
                    self.expr(x);
                }
            }
            Expr::Starred(s) => self.expr(&s.value),
            Expr::Slice(s) => {
                for part in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    self.expr(part);
                }
            }
            Expr::FString(f) => {
                for element in f.value.elements().filter_map(|e| e.as_interpolation()) {
                    self.expr(&element.expression);
                }
            }
            Expr::TString(t) => {
                for element in t.value.elements().filter_map(|e| e.as_interpolation()) {
                    self.expr(&element.expression);
                }
            }
            _ => {}
        }
    }

    fn each(&mut self, list: &[Expr]) {
        for e in list {
            self.expr(e);
        }
    }

    /// A comprehension is its own scope: its targets bind there, the element
    /// expression sees them, and nothing leaks out.
    fn comprehensions(
        &mut self,
        gens: &[ruff_python_ast::Comprehension],
        element: impl FnOnce(&mut Self),
    ) {
        self.push_scope(ScopeKind::Function);
        for g in gens {
            self.metrics.complexity += 1 + g.ifs.len() as u32;
            self.expr(&g.iter);
            let line = self.src.line_of(g.range().start());
            self.bind_target(
                &g.target,
                BindKind::Comprehension,
                ValueKind::Unknown,
                &g.iter,
                line,
            );
            for i in &g.ifs {
                self.expr(i);
            }
        }
        element(self);
        self.pop_scope();
    }

    // -------------------------------------------------------------- bindings

    fn value_kind_of(&self, e: &Expr) -> ValueKind {
        match e {
            Expr::StringLiteral(_) | Expr::FString(_) => return ValueKind::Text,
            Expr::NumberLiteral(_) => return ValueKind::Number,
            Expr::BooleanLiteral(_) => return ValueKind::Bool,
            Expr::NoneLiteral(_) => return ValueKind::None,
            Expr::List(_) | Expr::Tuple(_) | Expr::ListComp(_) | Expr::Set(_) => {
                return ValueKind::Seq
            }
            Expr::Dict(_) | Expr::DictComp(_) => return ValueKind::Map,
            Expr::Lambda(_) => return ValueKind::Callable,
            _ => {}
        }
        match self.resolve(e) {
            Some(path) => {
                let direct = effects::produces(&path);
                if direct != ValueKind::Unknown {
                    return direct;
                }
                // `f = open(p)` resolves to `<file>` already via `resolve`.
                match path.as_str() {
                    "<file>" => ValueKind::File,
                    "<path>" => ValueKind::Path,
                    "<session>" => ValueKind::Session,
                    "<response>" => ValueKind::Response,
                    "<frame>" => ValueKind::Frame,
                    "<proc>" => ValueKind::Process,
                    "<socket>" => ValueKind::Socket,
                    "<conn>" => ValueKind::Connection,
                    "<archive>" => ValueKind::Archive,
                    _ => ValueKind::Unknown,
                }
            }
            None => ValueKind::Unknown,
        }
    }

    fn bind(&mut self, name: &str, kind: BindKind, value: ValueKind, origin: &Expr, line: u32) {
        if name == "_" {
            return;
        }
        let origin_text = brief_expr(self.src, origin, ORIGIN);
        match Self::str_lit(origin) {
            Some(literal) if kind == BindKind::Assign => {
                self.consts.insert(name.to_string(), literal.to_string());
            }
            // A rebinding to something else invalidates the constant.
            _ => {
                self.consts.remove(name);
            }
        }
        self.origins.insert(name.to_string(), origin_text.clone());
        // A name holds a secret while it was last bound to something derived
        // from one. `+=` only ever adds to what it held.
        match self.taint_of(origin) {
            Some(t) => {
                self.taint.insert(name.to_string(), t);
            }
            None if kind != BindKind::Augment => {
                self.taint.remove(name);
            }
            None => {}
        }
        if value != ValueKind::Unknown {
            self.kinds.insert(name.to_string(), value);
        }
        // `f = os.system` makes `f(...)` a shell call; anything else assigned
        // to `f` later stops it being one.
        match self.callable_target(origin) {
            Some(path) if kind == BindKind::Assign => {
                self.callable_aliases.insert(name.to_string(), path);
            }
            _ => {
                self.callable_aliases.remove(name);
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.names.insert(name.to_string());
        }
        if let Some(&idx) = self.binding_at.get(name) {
            let existing = &mut self.bindings[idx];
            existing.rebinds += 1;
            if kind == BindKind::Augment && self.loop_depth > 0 {
                existing.accumulates = true;
            }
            if existing.value == ValueKind::Unknown && value != ValueKind::Unknown {
                existing.value = value;
            }
            return;
        }
        self.binding_at
            .insert(name.to_string(), self.bindings.len());
        self.bindings.push(Binding {
            name: name.to_string(),
            kind,
            value,
            origin: origin_text,
            scope: self.scope_name(),
            line,
            depth: self.scope.len().saturating_sub(1) as u16,
            reads: 0,
            rebinds: 0,
            accumulates: kind == BindKind::Augment && self.loop_depth > 0,
            dead: false,
        });
    }

    fn bind_target(
        &mut self,
        target: &Expr,
        kind: BindKind,
        value: ValueKind,
        origin: &Expr,
        line: u32,
    ) {
        match target {
            Expr::Name(n) => self.bind(n.id.as_str(), kind, value, origin, line),
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.bind_target(e, kind, ValueKind::Unknown, origin, line);
                }
            }
            Expr::List(l) => {
                for e in &l.elts {
                    self.bind_target(e, kind, ValueKind::Unknown, origin, line);
                }
            }
            Expr::Starred(s) => self.bind_target(&s.value, kind, ValueKind::Seq, origin, line),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ walk

    pub fn body(&mut self, stmts: &[Stmt], depth: u16) -> Vec<Node> {
        stmts.iter().map(|s| self.stmt(s, depth)).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        id: u32,
        kind: NodeKind,
        label: String,
        detail: Option<String>,
        line: u32,
        end_line: u32,
        depth: u16,
        first_effect: usize,
        own_end: usize,
        children: Vec<Node>,
    ) -> Node {
        let mut own = EffectMask::default();
        let mut own_sev: Option<Severity> = None;
        let own_end = own_end.min(self.effects.len()).max(first_effect);
        for hit in &self.effects[first_effect..own_end] {
            own.insert(hit.effect);
            own_sev = Some(own_sev.map_or(hit.severity, |s: Severity| s.max(hit.severity)));
        }
        let mut all = own;
        let mut sev = own_sev;
        let mut weight = 1;
        for c in &children {
            all.union(c.effects);
            if let Some(cs) = c.severity {
                sev = Some(sev.map_or(cs, |s: Severity| s.max(cs)));
            }
            weight += c.weight;
        }
        self.max_depth = self.max_depth.max(depth);
        Node {
            id,
            kind,
            label,
            detail,
            line,
            end_line,
            depth,
            own_effects: own,
            effects: all,
            severity: sev,
            weight,
            children,
        }
    }

    fn stmt(&mut self, stmt: &Stmt, depth: u16) -> Node {
        self.metrics.statements += 1;
        let id = self.id();
        let line = stmt_line(self.src, stmt);
        let end_line = self.src.line_of(stmt.range().end());
        let e0 = self.effects.len();
        if depth > MAX_STMT_DEPTH {
            self.depth_limit("a block", MAX_STMT_DEPTH as u32, stmt.range());
            let label = clip(&squeeze(self.src.slice(stmt.range())), LABEL);
            return self.finish(
                id,
                NodeKind::Expr,
                label,
                Some("not analysed: too deeply nested".into()),
                line,
                end_line,
                depth,
                e0,
                e0,
                vec![],
            );
        }
        // Effects recorded between `e0` and `own_end` belong to this statement
        // itself; anything after that was raised by a nested statement and is
        // that statement's to report. Arms with children close the window
        // before they recurse; arms without children leave it open, and the
        // `min` in `finish` does the right thing.
        let mut own_end = usize::MAX;

        macro_rules! node {
            ($kind:expr, $label:expr, $detail:expr, $children:expr) => {
                self.finish(
                    id, $kind, $label, $detail, line, end_line, depth, e0, own_end, $children,
                )
            };
        }

        match stmt {
            Stmt::Import(s) => {
                self.metrics.imports += s.names.len() as u32;
                if self.scope.len() > 1 {
                    for a in &s.names {
                        let local = match &a.asname {
                            Some(x) => x.to_string(),
                            None => a.name.split('.').next().unwrap_or(&a.name).to_string(),
                        };
                        let label = format!("import {}", a.name);
                        self.bind_name_only(
                            &local,
                            BindKind::Import,
                            ValueKind::Module,
                            &label,
                            line,
                        );
                    }
                }
                let names: Vec<String> = s
                    .names
                    .iter()
                    .map(|a| match &a.asname {
                        Some(x) => format!("{} as {x}", a.name),
                        None => a.name.to_string(),
                    })
                    .collect();
                node!(
                    NodeKind::Import,
                    format!("import {}", names.join(", ")),
                    None,
                    vec![]
                )
            }
            Stmt::ImportFrom(s) => {
                self.metrics.imports += s.names.len() as u32;
                let module = match &s.module {
                    Some(m) => format!("{}{m}", ".".repeat(s.level as usize)),
                    None => ".".repeat(s.level.max(1) as usize),
                };
                if self.scope.len() > 1 {
                    for a in &s.names {
                        if &*a.name == "*" {
                            continue;
                        }
                        let local = a.asname.as_ref().unwrap_or(&a.name).to_string();
                        let label = format!("from {module} import {}", a.name);
                        self.bind_name_only(
                            &local,
                            BindKind::Import,
                            ValueKind::Unknown,
                            &label,
                            line,
                        );
                    }
                }
                let names: Vec<String> = s
                    .names
                    .iter()
                    .map(|a| match &a.asname {
                        Some(x) => format!("{} as {x}", a.name),
                        None => a.name.to_string(),
                    })
                    .collect();
                let label = clip(&format!("from {module} import {}", names.join(", ")), LABEL);
                node!(NodeKind::Import, label, None, vec![])
            }
            Stmt::FunctionDef(s) => {
                self.metrics.functions += 1;
                if s.is_async {
                    self.metrics.async_defs += 1;
                }
                for d in &s.decorator_list {
                    self.expr(&d.expression);
                }
                for p in s
                    .parameters
                    .iter_non_variadic_params()
                    .filter_map(|p| p.default())
                {
                    self.expr(p);
                }
                let kw = if s.is_async { "async def" } else { "def" };
                let label = clip(
                    &format!("{kw} {}({})", s.name, param_names(&s.parameters).join(", ")),
                    LABEL,
                );
                let mut detail = Vec::new();
                if let Some(ret) = &s.returns {
                    detail.push(format!("→ {}", brief_expr(self.src, ret, 24)));
                }
                for d in &s.decorator_list {
                    detail.push(format!("@{}", brief_expr(self.src, &d.expression, 20)));
                }
                own_end = self.effects.len();
                self.scope.push(s.name.to_string());
                let saved_loop = std::mem::replace(&mut self.loop_depth, 0);
                let state = self.snapshot();
                self.push_scope(ScopeKind::Function);
                for p in s.parameters.iter() {
                    let pname = p.name().to_string();
                    self.bind_name_only(&pname, BindKind::Param, ValueKind::Unknown, &pname, line);
                }
                self.prescan_local(&s.body);
                let children = self.body(&s.body, depth + 1);
                self.pop_scope();
                self.restore(state);
                self.loop_depth = saved_loop;
                self.scope.pop();
                self.bind_name_only(
                    s.name.as_str(),
                    BindKind::Def,
                    ValueKind::Callable,
                    &label,
                    line,
                );
                let detail = (!detail.is_empty()).then(|| detail.join("  "));
                node!(NodeKind::Def, label, detail, children)
            }
            Stmt::ClassDef(s) => {
                self.metrics.classes += 1;
                for d in &s.decorator_list {
                    self.expr(&d.expression);
                }
                let bases = s
                    .arguments
                    .as_ref()
                    .map(|a| {
                        a.args
                            .iter()
                            .map(|e| brief_expr(self.src, e, 20))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let label = if bases.is_empty() {
                    format!("class {}", s.name)
                } else {
                    clip(&format!("class {}({bases})", s.name), LABEL)
                };
                own_end = self.effects.len();
                self.scope.push(s.name.to_string());
                let state = self.snapshot();
                self.push_scope(ScopeKind::Class);
                self.prescan_local(&s.body);
                let children = self.body(&s.body, depth + 1);
                self.pop_scope();
                self.restore(state);
                self.scope.pop();
                self.bind_name_only(
                    s.name.as_str(),
                    BindKind::Class,
                    ValueKind::Class,
                    &label,
                    line,
                );
                node!(NodeKind::Class, label, None, children)
            }
            Stmt::For(s) => {
                self.metrics.loops += 1;
                self.metrics.complexity += 1;
                self.expr(&s.iter);
                let value = self.value_kind_of(&s.iter);
                let element = match value {
                    ValueKind::Seq | ValueKind::Map => ValueKind::Unknown,
                    other => other,
                };
                self.bind_target(&s.target, BindKind::LoopVar, element, &s.iter, line);
                let kw = if s.is_async { "async for" } else { "for" };
                let label = clip(
                    &format!(
                        "{kw} {} in {}",
                        brief_expr(self.src, &s.target, 24),
                        brief_expr(self.src, &s.iter, LABEL / 2)
                    ),
                    LABEL,
                );
                own_end = self.effects.len();
                self.loop_depth += 1;
                let mut children = self.body(&s.body, depth + 1);
                self.loop_depth -= 1;
                if !s.orelse.is_empty() {
                    children.push(self.arm("else", &s.orelse, depth + 1));
                }
                node!(NodeKind::Loop, label, None, children)
            }
            Stmt::While(s) => {
                self.metrics.loops += 1;
                self.metrics.complexity += 1;
                self.expr(&s.test);
                let label = clip(
                    &format!("while {}", brief_expr(self.src, &s.test, LABEL - 6)),
                    LABEL,
                );
                own_end = self.effects.len();
                self.loop_depth += 1;
                let mut children = self.body(&s.body, depth + 1);
                self.loop_depth -= 1;
                if !s.orelse.is_empty() {
                    children.push(self.arm("else", &s.orelse, depth + 1));
                }
                node!(NodeKind::Loop, label, None, children)
            }
            Stmt::If(s) => {
                self.metrics.branches += 1;
                self.metrics.complexity += 1;
                self.expr(&s.test);
                let label = clip(
                    &format!("if {}", brief_expr(self.src, &s.test, LABEL - 3)),
                    LABEL,
                );
                own_end = self.effects.len();
                let mut children = self.body(&s.body, depth + 1);
                for clause in &s.elif_else_clauses {
                    let arm_id = self.id();
                    let arm_line = self.src.line_of(clause.range().start());
                    let arm_end = self.src.line_of(clause.range().end());
                    let a0 = self.effects.len();
                    let arm_label = match &clause.test {
                        Some(test) => {
                            self.metrics.branches += 1;
                            self.metrics.complexity += 1;
                            self.expr(test);
                            clip(
                                &format!("elif {}", brief_expr(self.src, test, LABEL - 5)),
                                LABEL,
                            )
                        }
                        None => "else".to_string(),
                    };
                    let arm_own = self.effects.len();
                    let arm_children = self.body(&clause.body, depth + 2);
                    children.push(self.finish(
                        arm_id,
                        NodeKind::BranchArm,
                        arm_label,
                        None,
                        arm_line,
                        arm_end,
                        depth + 1,
                        a0,
                        arm_own,
                        arm_children,
                    ));
                }
                node!(NodeKind::Branch, label, None, children)
            }
            Stmt::Match(s) => {
                self.metrics.branches += 1;
                self.expr(&s.subject);
                let label = clip(
                    &format!("match {}", brief_expr(self.src, &s.subject, LABEL - 6)),
                    LABEL,
                );
                own_end = self.effects.len();
                let mut children = Vec::new();
                for case in &s.cases {
                    self.metrics.complexity += 1;
                    let arm_id = self.id();
                    let a0 = self.effects.len();
                    let pat = clip(&squeeze(self.src.slice(case.pattern.range())), LABEL - 6);
                    let arm_own = self.effects.len();
                    let arm_children = self.body(&case.body, depth + 2);
                    children.push(self.finish(
                        arm_id,
                        NodeKind::BranchArm,
                        format!("case {pat}"),
                        None,
                        self.src.line_of(case.range().start()),
                        self.src.line_of(case.range().end()),
                        depth + 1,
                        a0,
                        arm_own,
                        arm_children,
                    ));
                }
                node!(NodeKind::Branch, label, None, children)
            }
            Stmt::With(s) => {
                for item in &s.items {
                    self.expr(&item.context_expr);
                    if let Some(vars) = &item.optional_vars {
                        let kind = self.value_kind_of(&item.context_expr);
                        self.bind_target(vars, BindKind::WithVar, kind, &item.context_expr, line);
                    }
                }
                let items: Vec<String> = s
                    .items
                    .iter()
                    .map(|item| {
                        let ctx = brief_expr(self.src, &item.context_expr, 34);
                        match &item.optional_vars {
                            Some(v) => format!("{ctx} as {}", brief_expr(self.src, v, 12)),
                            None => ctx,
                        }
                    })
                    .collect();
                let kw = if s.is_async { "async with" } else { "with" };
                let label = clip(&format!("{kw} {}", items.join(", ")), LABEL);
                own_end = self.effects.len();
                let children = self.body(&s.body, depth + 1);
                node!(NodeKind::With, label, None, children)
            }
            Stmt::Try(s) => {
                own_end = e0;
                let mut children = self.body(&s.body, depth + 1);
                for handler in &s.handlers {
                    let ExceptHandler::ExceptHandler(h) = handler;
                    self.metrics.handlers += 1;
                    self.metrics.complexity += 1;
                    let hid = self.id();
                    let h0 = self.effects.len();
                    if let Some(t) = &h.type_ {
                        self.expr(t);
                    }
                    let mut label = String::from(if s.is_star { "except*" } else { "except" });
                    if let Some(t) = &h.type_ {
                        label.push(' ');
                        label.push_str(&brief_expr(self.src, t, 28));
                    }
                    if let Some(name) = &h.name {
                        label.push_str(&format!(" as {name}"));
                        let hl = self.src.line_of(h.range().start());
                        self.bind_name_only(
                            name.as_str(),
                            BindKind::ExceptVar,
                            ValueKind::Unknown,
                            &label,
                            hl,
                        );
                    }
                    let h_own = self.effects.len();
                    let hchildren = self.body(&h.body, depth + 2);
                    children.push(self.finish(
                        hid,
                        NodeKind::Handler,
                        label,
                        None,
                        self.src.line_of(h.range().start()),
                        self.src.line_of(h.range().end()),
                        depth + 1,
                        h0,
                        h_own,
                        hchildren,
                    ));
                }
                if !s.orelse.is_empty() {
                    children.push(self.arm("else", &s.orelse, depth + 1));
                }
                if !s.finalbody.is_empty() {
                    children.push(self.arm("finally", &s.finalbody, depth + 1));
                }
                node!(NodeKind::Try, "try".into(), None, children)
            }
            Stmt::Assign(s) => {
                self.expr(&s.value);
                let kind = self.value_kind_of(&s.value);
                let tainted = self.taint_of(&s.value);
                for t in &s.targets {
                    self.expr_store(t);
                    self.bind_target(t, BindKind::Assign, kind, &s.value, line);
                    // `headers["Authorization"] = tok` puts the token in
                    // `headers`; so does `req.data = tok`.
                    if let Some(taint) = &tainted {
                        if let Some(base) = store_base(t) {
                            self.taint.insert(base.to_string(), taint.clone());
                        }
                    }
                }
                let lhs = s
                    .targets
                    .iter()
                    .map(|t| brief_expr(self.src, t, 24))
                    .collect::<Vec<_>>()
                    .join(" = ");
                let rhs = brief_expr(self.src, &s.value, LABEL.saturating_sub(lhs.len() + 3));
                node!(
                    NodeKind::Assign,
                    clip(&format!("{lhs} = {rhs}"), LABEL),
                    None,
                    vec![]
                )
            }
            Stmt::AugAssign(s) => {
                self.expr(&s.value);
                self.expr_store(&s.target);
                self.bind_target(
                    &s.target,
                    BindKind::Augment,
                    ValueKind::Unknown,
                    &s.value,
                    line,
                );
                let lhs = brief_expr(self.src, &s.target, 24);
                let rhs = brief_expr(self.src, &s.value, LABEL.saturating_sub(lhs.len() + 4));
                node!(
                    NodeKind::Assign,
                    clip(&format!("{lhs} {}= {rhs}", s.op.as_str()), LABEL),
                    None,
                    vec![]
                )
            }
            Stmt::AnnAssign(s) => {
                if let Some(v) = &s.value {
                    self.expr(v);
                    let kind = self.value_kind_of(v);
                    self.bind_target(&s.target, BindKind::Annotate, kind, v, line);
                }
                let label = clip(&squeeze(self.src.slice(s.range())), LABEL);
                node!(NodeKind::Assign, label, None, vec![])
            }
            Stmt::Return(s) => {
                if let Some(v) = &s.value {
                    self.expr(v);
                }
                let label = match &s.value {
                    Some(v) => clip(
                        &format!("return {}", brief_expr(self.src, v, LABEL - 7)),
                        LABEL,
                    ),
                    None => "return".into(),
                };
                node!(NodeKind::Return, label, None, vec![])
            }
            Stmt::Raise(s) => {
                for e in [&s.exc, &s.cause].into_iter().flatten() {
                    self.expr(e);
                }
                let label = match &s.exc {
                    Some(e) => clip(
                        &format!("raise {}", brief_expr(self.src, e, LABEL - 6)),
                        LABEL,
                    ),
                    None => "raise".into(),
                };
                node!(NodeKind::Raise, label, None, vec![])
            }
            Stmt::Assert(s) => {
                self.expr(&s.test);
                if let Some(m) = &s.msg {
                    self.expr(m);
                }
                self.metrics.complexity += 1;
                let label = clip(
                    &format!("assert {}", brief_expr(self.src, &s.test, LABEL - 7)),
                    LABEL,
                );
                node!(NodeKind::Assert, label, None, vec![])
            }
            Stmt::Delete(s) => {
                for t in &s.targets {
                    match t {
                        Expr::Subscript(sub)
                            if self.on_env_subscript(sub, "unsetenv", Severity::Notable) =>
                        {
                            self.expr(&sub.value);
                            self.expr(&sub.slice);
                        }
                        other => self.expr(other),
                    }
                }
                let names = s
                    .targets
                    .iter()
                    .map(|t| brief_expr(self.src, t, 20))
                    .collect::<Vec<_>>()
                    .join(", ");
                node!(
                    NodeKind::Delete,
                    clip(&format!("del {names}"), LABEL),
                    None,
                    vec![]
                )
            }
            Stmt::Global(s) => {
                let names = s
                    .names
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                node!(NodeKind::Global, format!("global {names}"), None, vec![])
            }
            Stmt::Nonlocal(s) => {
                let names = s
                    .names
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                node!(NodeKind::Global, format!("nonlocal {names}"), None, vec![])
            }
            Stmt::Expr(s) => {
                self.expr(&s.value);
                let (kind, label, detail) = match &*s.value {
                    Expr::StringLiteral(lit) => {
                        let text = squeeze(lit.value.to_str());
                        (
                            NodeKind::Expr,
                            "\"\"\"docstring\"\"\"".to_string(),
                            Some(clip(&text, LABEL)),
                        )
                    }
                    Expr::Call(_) => (
                        NodeKind::Call,
                        clip(&brief_expr(self.src, &s.value, LABEL), LABEL),
                        None,
                    ),
                    _ => (
                        NodeKind::Expr,
                        clip(&brief_expr(self.src, &s.value, LABEL), LABEL),
                        None,
                    ),
                };
                node!(kind, label, detail, vec![])
            }
            Stmt::Pass(_) => node!(NodeKind::Pass, "pass".into(), None, vec![]),
            Stmt::Break(_) => node!(NodeKind::Jump, "break".into(), None, vec![]),
            Stmt::Continue(_) => node!(NodeKind::Jump, "continue".into(), None, vec![]),
            Stmt::TypeAlias(s) => {
                let label = clip(&squeeze(self.src.slice(s.range())), LABEL);
                node!(NodeKind::Assign, label, None, vec![])
            }
            Stmt::IpyEscapeCommand(s) => {
                let label = clip(&squeeze(self.src.slice(s.range())), LABEL);
                node!(NodeKind::Call, label, None, vec![])
            }
        }
    }

    /// Walk a store target for its *nested* loads — `d[k] = v` reads `d` and
    /// `k` even though the statement as a whole is a write.
    fn expr_store(&mut self, target: &Expr) {
        match target {
            Expr::Subscript(s) => {
                self.on_env_subscript(s, "setenv", Severity::Notable);
                self.expr(&s.value);
                self.expr(&s.slice);
            }
            Expr::Attribute(a) => self.expr(&a.value),
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.expr_store(e);
                }
            }
            Expr::List(l) => {
                for e in &l.elts {
                    self.expr_store(e);
                }
            }
            Expr::Starred(s) => self.expr_store(&s.value),
            _ => {}
        }
    }

    fn arm(&mut self, keyword: &str, body: &[Stmt], depth: u16) -> Node {
        let id = self.id();
        let e0 = self.effects.len();
        let line = body.first().map(|s| stmt_line(self.src, s)).unwrap_or(0);
        let end = body
            .last()
            .map(|s| self.src.line_of(s.range().end()))
            .unwrap_or(line);
        let children = self.body(body, depth + 1);
        self.finish(
            id,
            NodeKind::BranchArm,
            keyword.to_string(),
            None,
            line,
            end,
            depth,
            e0,
            e0,
            children,
        )
    }

    /// Record a binding whose origin is a label we already built (a `def` or
    /// `class` header) rather than an expression.
    fn bind_name_only(
        &mut self,
        name: &str,
        kind: BindKind,
        value: ValueKind,
        label: &str,
        line: u32,
    ) {
        if value != ValueKind::Unknown {
            self.kinds.insert(name.to_string(), value);
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.names.insert(name.to_string());
        }
        if self.binding_at.contains_key(name) {
            return;
        }
        self.binding_at
            .insert(name.to_string(), self.bindings.len());
        self.bindings.push(Binding {
            name: name.to_string(),
            kind,
            value,
            origin: clip(label, ORIGIN),
            scope: self.scope_name(),
            line,
            depth: self.scope.len().saturating_sub(1) as u16,
            reads: 0,
            rebinds: 0,
            accumulates: false,
            dead: false,
        });
    }

    pub fn into_parts(self) -> Findings {
        let mut bindings = self.bindings;
        bindings.sort_by_key(|b| (b.line, b.name.clone()));
        let mut symbols: Vec<SymbolUse> = self.symbols.into_values().collect();
        symbols.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(a.group.cmp(&b.group))
                .then(a.canonical.cmp(&b.canonical))
        });
        Findings {
            imports: self.imports,
            effects: self.effects,
            symbols,
            bindings,
            diagnostics: self.diagnostics,
            metrics: self.metrics,
            reads: self.reads,
            max_depth: self.max_depth,
        }
    }
}

/// The tables [`Analyzer::snapshot`] saves around a nested scope.
struct ScopeState {
    kinds: HashMap<String, ValueKind>,
    consts: HashMap<String, String>,
    origins: HashMap<String, String>,
    callable_aliases: HashMap<String, String>,
    taint: HashMap<String, Taint>,
}

/// A secret a value holds or was computed from: where it was read, and what
/// kind of secret it was.
#[derive(Clone, Debug)]
struct Taint {
    secret: SecretKind,
    /// `env GITHUB_TOKEN`, `file ~/.ssh/id_rsa`, `keyring`.
    source: String,
    line: u32,
}

/// How far `taint_of` follows an expression before giving up.
const TAINT_FUEL: u32 = 24;

/// Calls whose result says nothing about their argument's contents. `len(tok)`
/// is a number, not the token.
const TAINT_CLEAN_CALLS: &[&str] = &[
    "len",
    "bool",
    "isinstance",
    "issubclass",
    "type",
    "hash",
    "id",
    "callable",
    "hasattr",
    "print",
    "repr_len",
];

/// Everything the walk collected, handed over in one piece.
pub struct Findings {
    pub imports: Vec<ImportInfo>,
    pub effects: Vec<EffectHit>,
    pub symbols: Vec<SymbolUse>,
    pub bindings: Vec<Binding>,
    pub diagnostics: Vec<Diagnostic>,
    pub metrics: Metrics,
    /// Name → how many times it was read, for spotting dead bindings.
    pub reads: HashMap<String, u32>,
    pub max_depth: u16,
}

fn symbol_group(canonical: &str, is_local: bool) -> String {
    if canonical.starts_with('<') {
        return canonical
            .split('.')
            .next()
            .unwrap_or(canonical)
            .trim_matches(|c| c == '<' || c == '>')
            .to_string();
    }
    match canonical.rsplit_once('.') {
        Some((module, _)) => module.to_string(),
        None => {
            if is_local {
                "<local>".into()
            } else {
                "<builtins>".into()
            }
        }
    }
}

/// Names that would otherwise dominate the call inventory without telling the
/// reader anything.
pub fn is_noise_symbol(canonical: &str) -> bool {
    matches!(
        canonical,
        "len"
            | "str"
            | "int"
            | "float"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "range"
            | "enumerate"
            | "zip"
            | "sorted"
            | "isinstance"
            | "type"
            | "bool"
            | "abs"
            | "min"
            | "max"
            | "sum"
            | "repr"
            | "format"
            | "getattr"
            | "hasattr"
            | "super"
    )
}

/// The plain name a subscript or attribute store ultimately writes into:
/// `headers` for `headers["a"]["b"] = x`, `req` for `req.data = x`.
fn store_base(target: &Expr) -> Option<&str> {
    match target {
        Expr::Subscript(s) => match &*s.value {
            Expr::Name(n) => Some(n.id.as_str()),
            other => store_base(other),
        },
        Expr::Attribute(a) => match &*a.value {
            Expr::Name(n) => Some(n.id.as_str()),
            other => store_base(other),
        },
        _ => None,
    }
}
