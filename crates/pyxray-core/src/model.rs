//! The X-ray report: everything `pyxray` knows about a snippet, with no
//! opinion at all about how it should look. `pyxray-render` turns this into
//! pixels; the JSON form is the stable contract for anything else.

use serde::{Deserialize, Serialize};

/// Version of the JSON contract carried by [`Report`] and [`Event`].
///
/// Rules for bumping it: adding a field with `#[serde(default)]` does not
/// need a bump; renaming or removing a field, or changing what an existing
/// value means, does. [`Effect`] variants may only ever be *appended* — the
/// feed stores [`EffectMask`] as a bare integer whose bit positions are the
/// declaration order, and a watcher reads events written by older builds.
/// A reader skips any event whose `schema` is newer than its own.
pub const SCHEMA: u32 = 1;

/// A capability the snippet reaches for. Deliberately a short, memorable set —
/// every variant gets its own colour and glyph downstream, and a list longer
/// than a dozen or so stops being glanceable.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    FsRead,
    FsWrite,
    FsDelete,
    Net,
    Process,
    Env,
    Dynamic,
    Stdout,
    Random,
    Clock,
    Concurrency,
    Compute,
    Exit,
}

impl Effect {
    /// How many capabilities there are. Arrays keyed by capability — palettes,
    /// glyph sets, the barcode — are sized by this so a new variant fails to
    /// compile until every table has a slot for it.
    pub const COUNT: usize = 13;

    /// Reading order for the capability barcode: the fixed 13-slot strip a
    /// reader learns as a *shape*. Grouped so related capabilities sit
    /// together — filesystem, then the world outside the process, then
    /// runtime code, then work, then output — because the eye learns
    /// clusters faster than it learns thirteen separate positions.
    pub const BARCODE: [Effect; Effect::COUNT] = [
        Effect::FsRead,
        Effect::FsWrite,
        Effect::FsDelete,
        Effect::Net,
        Effect::Process,
        Effect::Env,
        Effect::Dynamic,
        Effect::Compute,
        Effect::Random,
        Effect::Clock,
        Effect::Concurrency,
        Effect::Stdout,
        Effect::Exit,
    ];

    /// Sizes of the barcode's groups, in `BARCODE` order. Rendered with a
    /// gap between each, like the guard bars on a real barcode.
    pub const BARCODE_GROUPS: [usize; 5] = [3, 3, 1, 4, 2];

    /// Every capability, in declaration order. This is the bit order of
    /// [`EffectMask`] and the order `from_id` searches; append only.
    pub const ALL: [Effect; Effect::COUNT] = [
        Effect::FsRead,
        Effect::FsWrite,
        Effect::FsDelete,
        Effect::Net,
        Effect::Process,
        Effect::Env,
        Effect::Dynamic,
        Effect::Stdout,
        Effect::Random,
        Effect::Clock,
        Effect::Concurrency,
        Effect::Compute,
        Effect::Exit,
    ];

    /// Stable short id — the same spelling serde writes into JSON — used in
    /// CLI filters and theme lookup tables.
    pub fn id(self) -> &'static str {
        match self {
            Effect::FsRead => "fs_read",
            Effect::FsWrite => "fs_write",
            Effect::FsDelete => "fs_delete",
            Effect::Net => "net",
            Effect::Process => "process",
            Effect::Env => "env",
            Effect::Dynamic => "dynamic",
            Effect::Stdout => "stdout",
            Effect::Random => "random",
            Effect::Clock => "clock",
            Effect::Concurrency => "concurrency",
            Effect::Compute => "compute",
            Effect::Exit => "exit",
        }
    }

    /// Human label for panel headings.
    pub fn label(self) -> &'static str {
        match self {
            Effect::FsRead => "reads files",
            Effect::FsWrite => "writes files",
            Effect::FsDelete => "deletes files",
            Effect::Net => "network",
            Effect::Process => "runs commands",
            Effect::Env => "environment",
            Effect::Dynamic => "dynamic code",
            Effect::Stdout => "prints",
            Effect::Random => "randomness",
            Effect::Clock => "time",
            Effect::Concurrency => "concurrency",
            Effect::Compute => "compute",
            Effect::Exit => "exits",
        }
    }

    pub fn bit(self) -> u16 {
        1u16 << (self as u16)
    }

    /// Parse an id. Accepts the JSON spelling (`fs_read`) and the dotted
    /// spelling earlier builds used (`fs.read`).
    pub fn from_id(s: &str) -> Option<Effect> {
        let s = s.replace('.', "_");
        Effect::ALL.into_iter().find(|e| e.id() == s)
    }
}

/// Union of effects over a subtree. Cheap enough to hang off every spine node.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectMask(pub u16);

impl EffectMask {
    pub fn insert(&mut self, e: Effect) {
        self.0 |= e.bit();
    }
    pub fn union(&mut self, other: EffectMask) {
        self.0 |= other.0;
    }
    pub fn contains(self, e: Effect) -> bool {
        self.0 & e.bit() != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn iter(self) -> impl Iterator<Item = Effect> {
        Effect::ALL.into_iter().filter(move |e| self.contains(*e))
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Ordinary and expected — printing, importing, arithmetic.
    Info,
    /// Worth a glance before you run it — writes, network, environment reads.
    Notable,
    /// Stop and read the code — deletion, shells, eval, pickle.
    Caution,
}

/// One concrete place the snippet exercises a capability.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EffectHit {
    pub effect: Effect,
    pub severity: Severity,
    /// Imperative verb for the timeline: "read", "write", "spawn".
    pub verb: String,
    /// Resolved dotted callee, e.g. `subprocess.run`, `<file>.write`.
    pub symbol: String,
    /// The thing acted upon, when we can name it: a path, URL, command.
    pub target: Option<String>,
    pub line: u32,
    pub col: u32,
    /// Normalised source of the call itself.
    pub snippet: String,
    /// Why this was flagged, when the reason is not obvious from the symbol.
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Module,
    Import,
    Def,
    Class,
    Loop,
    Branch,
    BranchArm,
    Try,
    Handler,
    With,
    Assign,
    Call,
    Return,
    Raise,
    Assert,
    Jump,
    Expr,
    Pass,
    Global,
    Delete,
}

impl NodeKind {
    pub fn id(self) -> &'static str {
        match self {
            NodeKind::Module => "module",
            NodeKind::Import => "import",
            NodeKind::Def => "def",
            NodeKind::Class => "class",
            NodeKind::Loop => "loop",
            NodeKind::Branch => "branch",
            NodeKind::BranchArm => "arm",
            NodeKind::Try => "try",
            NodeKind::Handler => "handler",
            NodeKind::With => "with",
            NodeKind::Assign => "assign",
            NodeKind::Call => "call",
            NodeKind::Return => "return",
            NodeKind::Raise => "raise",
            NodeKind::Assert => "assert",
            NodeKind::Jump => "jump",
            NodeKind::Expr => "expr",
            NodeKind::Pass => "pass",
            NodeKind::Global => "global",
            NodeKind::Delete => "delete",
        }
    }

    /// Does this kind introduce a nesting level a reader cares about?
    pub fn is_block(self) -> bool {
        matches!(
            self,
            NodeKind::Def
                | NodeKind::Class
                | NodeKind::Loop
                | NodeKind::Branch
                | NodeKind::BranchArm
                | NodeKind::Try
                | NodeKind::Handler
                | NodeKind::With
                | NodeKind::Module
        )
    }
}

/// One entry in the structural outline. Labels are already human-readable —
/// the renderer never needs to know Python.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub id: u32,
    pub kind: NodeKind,
    /// e.g. `for path in paths`, `def load(path, *, strict=False)`.
    pub label: String,
    /// Secondary text: a return type, a decorator list, an effect target.
    pub detail: Option<String>,
    pub line: u32,
    pub end_line: u32,
    pub depth: u16,
    /// Effects raised directly by this statement.
    pub own_effects: EffectMask,
    /// Effects raised anywhere in this subtree.
    pub effects: EffectMask,
    /// Highest severity anywhere in this subtree.
    pub severity: Option<Severity>,
    /// Statements in this subtree, including this one.
    pub weight: u32,
    pub children: Vec<Node>,
}

impl Node {
    pub fn walk(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportInfo {
    /// Canonical module path, e.g. `numpy`, `os.path`, `.relative`.
    pub module: String,
    /// Names pulled out of it by `from x import a, b`; empty for plain imports.
    pub names: Vec<String>,
    /// Local alias the module is bound to, when renamed.
    pub alias: Option<String>,
    pub line: u32,
    /// Not in the standard library — i.e. something that has to be installed.
    pub third_party: bool,
    /// Inside a function/try, rather than at module top level.
    pub deferred: bool,
}

/// How a name came to exist, and what it appears to hold.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindKind {
    Assign,
    Augment,
    Annotate,
    LoopVar,
    WithVar,
    Param,
    Walrus,
    Comprehension,
    ExceptVar,
    Import,
    Def,
    Class,
}

/// A coarse guess at what a name holds, used to resolve method calls
/// (`f.write(...)` after `f = open(...)`) and to colour the bindings panel.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Unknown,
    File,
    Path,
    Response,
    Session,
    Process,
    Socket,
    Connection,
    Frame,
    Array,
    Tensor,
    Text,
    Number,
    Bool,
    Seq,
    Map,
    Callable,
    Module,
    Class,
    None,
    /// A zip or tar handle; extracting it writes wherever its members say.
    Archive,
}

impl ValueKind {
    pub fn id(self) -> &'static str {
        match self {
            ValueKind::Unknown => "?",
            ValueKind::File => "file",
            ValueKind::Path => "path",
            ValueKind::Response => "response",
            ValueKind::Session => "session",
            ValueKind::Process => "proc",
            ValueKind::Socket => "socket",
            ValueKind::Connection => "conn",
            ValueKind::Frame => "frame",
            ValueKind::Array => "array",
            ValueKind::Tensor => "tensor",
            ValueKind::Text => "str",
            ValueKind::Number => "num",
            ValueKind::Bool => "bool",
            ValueKind::Seq => "seq",
            ValueKind::Map => "map",
            ValueKind::Callable => "fn",
            ValueKind::Module => "mod",
            ValueKind::Class => "class",
            ValueKind::None => "none",
            ValueKind::Archive => "archive",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub kind: BindKind,
    pub value: ValueKind,
    /// Normalised source of what it was bound to.
    pub origin: String,
    /// Enclosing scope path, e.g. `<module>` or `Loader.read`.
    pub scope: String,
    pub line: u32,
    pub depth: u16,
    /// Number of later reads of this name.
    pub reads: u32,
    /// Rebound or augmented after first binding.
    pub rebinds: u32,
    /// Augmented inside a loop — i.e. it accumulates.
    pub accumulates: bool,
    /// Bound but never read afterwards.
    pub dead: bool,
}

/// Where a called symbol comes from, which decides whether it is worth listing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolOrigin {
    /// Reached through an import in this file.
    Import,
    /// A Python builtin.
    Builtin,
    /// Defined in this file.
    Local,
    /// A method on a value whose type we could not determine.
    Method,
}

/// An externally-defined thing the snippet calls, folded across call sites.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SymbolUse {
    /// Resolved dotted path, e.g. `numpy.array`.
    pub canonical: String,
    /// As written, e.g. `np.array`.
    pub display: String,
    /// Owning module for grouping; `<builtins>` or `<local>` when there is none.
    pub group: String,
    pub origin: SymbolOrigin,
    pub count: u32,
    pub lines: Vec<u32>,
    pub effects: EffectMask,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub lines_total: u32,
    pub lines_code: u32,
    pub lines_comment: u32,
    pub lines_blank: u32,
    pub statements: u32,
    pub max_depth: u32,
    /// Cyclomatic-ish: 1 + every branch, loop, handler and boolean operator.
    pub complexity: u32,
    pub functions: u32,
    pub classes: u32,
    pub async_defs: u32,
    pub calls: u32,
    pub imports: u32,
    pub loops: u32,
    pub branches: u32,
    pub handlers: u32,
    /// 0..100, from the severity and spread of the effects found.
    pub risk: u8,
}

/// What one source line looks like, for the minimap strip.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct LineCell {
    pub indent: u8,
    pub width: u16,
    pub blank: bool,
    pub comment: bool,
    pub kind: Option<NodeKind>,
    pub effects: EffectMask,
    pub severity: Option<Severity>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagLevel {
    Error,
    Warning,
    Hint,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub message: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Meta {
    /// Display name for the snippet: a filename, or `<stdin>`.
    pub name: String,
    pub bytes: usize,
    /// True when the source parsed cleanly; a failed parse still yields a
    /// partial report from whatever the recovering parser produced.
    pub parsed: bool,
    /// One-line plain-English answer to "what does this do?".
    pub synopsis: String,
    /// Ranked verbs behind the synopsis, for renderers that want the pieces.
    pub headline: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Report {
    /// See [`SCHEMA`]. Absent (0) in output from builds before versioning.
    #[serde(default)]
    pub schema: u32,
    pub meta: Meta,
    pub spine: Node,
    pub imports: Vec<ImportInfo>,
    pub effects: Vec<EffectHit>,
    pub symbols: Vec<SymbolUse>,
    pub bindings: Vec<Binding>,
    pub metrics: Metrics,
    pub texture: Vec<LineCell>,
    pub diagnostics: Vec<Diagnostic>,
    /// The source, kept so renderers can show code beside the analysis.
    pub source: Vec<String>,
}

impl Report {
    /// The report for a snippet the analyser could not finish — an internal
    /// panic caught by [`crate::xray_guarded`]. Empty, honest, and marked with
    /// one error diagnostic so a reader can tell "nothing found" from "could
    /// not look".
    pub fn failed(source: &str, name: &str, why: &str) -> Report {
        let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
        let n = lines.len() as u32;
        Report {
            schema: SCHEMA,
            meta: Meta {
                name: name.to_string(),
                bytes: source.len(),
                parsed: false,
                synopsis: "analysis failed — read it yourself".to_string(),
                headline: Vec::new(),
            },
            spine: Node {
                id: 0,
                kind: NodeKind::Module,
                label: name.to_string(),
                detail: None,
                line: 1,
                end_line: n.max(1),
                depth: 0,
                own_effects: EffectMask::default(),
                effects: EffectMask::default(),
                severity: None,
                weight: 1,
                children: Vec::new(),
            },
            imports: Vec::new(),
            effects: Vec::new(),
            symbols: Vec::new(),
            bindings: Vec::new(),
            metrics: Metrics {
                lines_total: n,
                ..Metrics::default()
            },
            texture: Vec::new(),
            diagnostics: vec![Diagnostic {
                level: DiagLevel::Error,
                message: format!("pyxray could not analyse this snippet: {why}"),
                line: 1,
                col: 1,
            }],
            source: lines,
        }
    }
}

/// Which of the four risk bands a score falls in. The bands, not the number,
/// are what anyone acts on — and at one render per second the band is all
/// there is time to read.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Inert,
    Routine,
    Check,
    Read,
}

impl Band {
    /// Band from the score alone. Only for places that have nothing else —
    /// the sparkline, mostly. Prefer [`Band::assess`].
    #[doc(hidden)]
    pub fn of(risk: u8) -> Band {
        match risk {
            0..=9 => Band::Inert,
            10..=29 => Band::Routine,
            30..=59 => Band::Check,
            _ => Band::Read,
        }
    }

    /// Band from the worst thing found, refined by the score.
    ///
    /// Severity leads because volume must not be able to impersonate danger:
    /// a script that writes three files is doing ordinary work and should stay
    /// green, while one `shutil.rmtree` is worth stopping for even though it
    /// is a single call. Scoring alone cannot express that — three notables
    /// and one caution land on the same number — so the two are kept separate
    /// and the band asks the severity first.
    pub fn assess(risk: u8, worst: Option<Severity>) -> Band {
        match worst {
            Some(Severity::Caution) => {
                if risk >= 60 {
                    Band::Read
                } else {
                    Band::Check
                }
            }
            Some(Severity::Notable) => {
                if risk >= 45 {
                    Band::Check
                } else {
                    Band::Routine
                }
            }
            _ => {
                if risk >= 15 {
                    Band::Routine
                } else {
                    Band::Inert
                }
            }
        }
    }

    /// How many cells of the three-cell margin block to fill. Length carries
    /// the same information as the colour, so the strip still reads when
    /// colour does not survive — a log file, a colourblind reader, a
    /// screenshot in a chat.
    pub fn weight(self) -> usize {
        match self {
            Band::Inert => 1,
            Band::Routine => 1,
            Band::Check => 2,
            Band::Read => 3,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Band::Inert => "inert",
            Band::Routine => "routine",
            Band::Check => "check it",
            Band::Read => "read it first",
        }
    }
}

/// One line in the feed: everything a watcher needs about a single snippet,
/// small enough to append to a log a few times a second.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    /// See [`SCHEMA`]. Absent (0) in feed lines written before versioning.
    #[serde(default)]
    pub schema: u32,
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    pub name: String,
    /// Where it came from: `hook`, `shim`, `cli`, or a harness name.
    pub source: String,
    pub lines: u32,
    pub risk: u8,
    pub band: Band,
    /// The capability mask, for rendering the barcode without re-analysing.
    pub mask: u16,
    pub worst: Option<Severity>,
    pub synopsis: String,
    /// The few effects worth naming, most severe first.
    pub notes: Vec<EventNote>,
    /// Whether anything downstream refused to run it.
    pub blocked: bool,
    /// One execution's id, shared by every line that belongs to it — the
    /// predicted event and, later, what was observed. Absent when the caller
    /// had none (a hook sees a tool call, not a process).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The script's path, when the snippet came from a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A short hash of the source, so two runs of one path can be told apart
    /// from two runs of the same code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EventNote {
    pub effect: Effect,
    pub severity: Severity,
    pub verb: String,
    pub target: Option<String>,
    pub line: u32,
    pub note: Option<String>,
}

impl Report {
    /// Condense to a feed event.
    pub fn event(&self, source: &str) -> Event {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut notes: Vec<EventNote> = self
            .effects
            .iter()
            .filter(|h| h.severity > Severity::Info)
            .map(|h| EventNote {
                effect: h.effect,
                severity: h.severity,
                verb: h.verb.clone(),
                target: h.target.clone(),
                line: h.line,
                note: h.note.clone(),
            })
            .collect();
        notes.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.line.cmp(&b.line)));
        notes.truncate(4);
        Event {
            schema: SCHEMA,
            ts,
            name: self.meta.name.clone(),
            source: source.to_string(),
            lines: self.metrics.lines_total,
            risk: self.metrics.risk,
            band: self.band(),
            mask: self.spine.effects.0,
            worst: self.worst(),
            synopsis: self.meta.synopsis.clone(),
            notes,
            blocked: false,
            run: None,
            path: None,
            digest: None,
        }
    }

    pub fn band(&self) -> Band {
        Band::assess(self.metrics.risk, self.worst())
    }

    /// Effects folded by kind, most severe first then most frequent, for the
    /// capability panel.
    pub fn effect_summary(&self) -> Vec<(Effect, Severity, u32)> {
        let mut out: Vec<(Effect, Severity, u32)> = Vec::new();
        for hit in &self.effects {
            match out.iter_mut().find(|(e, _, _)| *e == hit.effect) {
                Some(slot) => {
                    slot.1 = slot.1.max(hit.severity);
                    slot.2 += 1;
                }
                None => out.push((hit.effect, hit.severity, 1)),
            }
        }
        // Cautions lead; everything else falls into the order the program
        // runs in, so the row reads as a pipeline.
        out.sort_by(|a, b| {
            let rank = |s: Severity| u8::from(s != Severity::Caution);
            rank(a.1)
                .cmp(&rank(b.1))
                .then(crate::digest::stage(a.0).cmp(&crate::digest::stage(b.0)))
                .then(b.2.cmp(&a.2))
                .then(a.0.cmp(&b.0))
        });
        out
    }

    /// The effect hits worth showing when there is only room for `limit` rows:
    /// identical operations folded together, the most severe kept, and the
    /// survivors put back into source order.
    pub fn timeline(&self, limit: usize) -> Vec<(&EffectHit, u32)> {
        let mut folded: Vec<(&EffectHit, u32)> = Vec::new();
        for hit in &self.effects {
            match folded.iter_mut().find(|(h, _)| {
                h.effect == hit.effect && h.verb == hit.verb && h.target == hit.target
            }) {
                Some(slot) => slot.1 += 1,
                None => folded.push((hit, 1)),
            }
        }
        if folded.len() > limit {
            folded.sort_by(|a, b| {
                b.0.severity
                    .cmp(&a.0.severity)
                    .then(b.1.cmp(&a.1))
                    .then(a.0.line.cmp(&b.0.line))
            });
            folded.truncate(limit);
        }
        folded.sort_by_key(|(h, _)| (h.line, h.col));
        folded
    }

    pub fn mask(&self) -> EffectMask {
        self.spine.effects
    }

    pub fn worst(&self) -> Option<Severity> {
        self.effects.iter().map(|h| h.severity).max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_bits_are_declaration_order() {
        // The feed persists masks as integers; this pins the bit layout so a
        // reordered enum fails here rather than silently relabelling old rows.
        for (i, e) in Effect::ALL.iter().enumerate() {
            assert_eq!(*e as u16, i as u16, "{e:?}");
            assert_eq!(e.bit(), 1 << i);
        }
        assert_eq!(Effect::ALL.len(), Effect::COUNT);
    }

    #[test]
    fn barcode_is_a_permutation_of_all() {
        let mut sorted = Effect::BARCODE.to_vec();
        sorted.sort();
        let mut all = Effect::ALL.to_vec();
        all.sort();
        assert_eq!(sorted, all);
        assert_eq!(Effect::BARCODE_GROUPS.iter().sum::<usize>(), Effect::COUNT);
    }

    #[test]
    fn ids_round_trip_in_both_spellings() {
        for e in Effect::ALL {
            assert_eq!(Effect::from_id(e.id()), Some(e));
            assert_eq!(
                serde_json::to_string(&e).unwrap(),
                format!("\"{}\"", e.id())
            );
        }
        assert_eq!(Effect::from_id("fs.read"), Some(Effect::FsRead));
        assert_eq!(Effect::from_id("nope"), None);
    }

    #[test]
    fn report_json_carries_the_schema() {
        let report = crate::xray("print(1)", "t");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema"], SCHEMA);
        let event = serde_json::to_value(report.event("test")).unwrap();
        assert_eq!(event["schema"], SCHEMA);
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.schema, SCHEMA);
    }

    #[test]
    fn a_failed_report_says_so() {
        let r = Report::failed("a = 1\nb = 2\n", "x.py", "boom");
        assert_eq!(r.metrics.lines_total, 2);
        assert!(!r.meta.parsed);
        assert_eq!(r.diagnostics.len(), 1);
        assert!(r.diagnostics[0].message.contains("boom"));
        assert!(r.effects.is_empty());
    }
}
