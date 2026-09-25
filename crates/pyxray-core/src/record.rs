//! Turning a report into a feed line: the one place that does it.
//!
//! The binary's `--emit` and the Python extension's `look()` used to each
//! build the event and append it themselves. Both now come here, so whatever
//! an event gains — a run id, the script's path, and later a policy verdict or
//! a diff against the last run — reaches the feed the same way from both.

use std::path::PathBuf;

use crate::feed;
use crate::model::{Event, Report};

/// How to record one report.
#[derive(Clone, Debug)]
pub struct RecordOpts {
    /// Where the snippet came from: `hook`, `shim`, `cli`, `hermes:ab12cd34`…
    pub source: String,
    /// The execution this belongs to, when there is one.
    pub run: Option<String>,
    /// The script's path, when the snippet came from a file.
    pub origin: Option<PathBuf>,
    /// The log to append to; `None` is [`feed::default_path`].
    pub feed: Option<PathBuf>,
    /// Append to the log at all. A post-tool hook re-analyses the call it
    /// already recorded, and must not record it twice.
    pub write: bool,
    /// Where the snippet runs: relative paths in its targets resolve from
    /// here, and so does the search for a repository `.pyxray.toml`.
    pub cwd: Option<PathBuf>,
    /// Evaluate the policy in effect (`PYXRAY_POLICY`, the user file, the
    /// repository's `.pyxray.toml`) and put its verdict on the event.
    pub policy: bool,
}

impl Default for RecordOpts {
    fn default() -> Self {
        RecordOpts {
            source: "cli".into(),
            run: None,
            origin: None,
            feed: None,
            write: true,
            cwd: None,
            policy: true,
        }
    }
}

/// Build the event for `report` and, if asked, append it to the feed.
///
/// Returns the event and, when the append failed, why — as text, because the
/// caller is usually in front of somebody else's command and should report a
/// broken log, not fail on it.
pub fn record(report: &Report, o: &RecordOpts) -> (Event, Option<String>) {
    let mut event = report.event(&o.source);
    event.run = o.run.clone();
    event.path = o.origin.as_ref().map(|p| p.display().to_string());
    event.digest = Some(digest(&report.source.join("\n")));
    if o.policy {
        let dir = o
            .origin
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf)
            .or_else(|| o.cwd.clone());
        let policy = crate::policy::Policy::discover(dir.as_deref());
        event.policy = policy.evaluate(report, o.cwd.as_deref().or(dir.as_deref()));
    }

    if !o.write {
        return (event, None);
    }
    let target = o.feed.clone().unwrap_or_else(feed::default_path);
    if let Some(path) = event.path.clone() {
        if let Some(prev) = feed::previous_for(&target, &path) {
            if prev.digest.is_some() && prev.digest != event.digest {
                event.changed = diff(&prev, &event);
            }
        }
    }
    let error = feed::append(&target, &event)
        .err()
        .map(|e| format!("{}: {e}", target.display()));
    (event, error)
}

/// What `cur` can do that `prev` could not, and the reverse. `None` when the
/// capabilities and named targets are the same — an edit that changes only
/// how, not what, is not news.
pub fn diff(prev: &Event, cur: &Event) -> Option<crate::model::EventDiff> {
    use crate::model::{EffectMask, EventDiff};
    let (p, c) = (EffectMask(prev.mask), EffectMask(cur.mask));
    let added: Vec<_> = c.iter().filter(|e| !p.contains(*e)).collect();
    let removed: Vec<_> = p.iter().filter(|e| !c.contains(*e)).collect();
    let old_targets: std::collections::HashSet<&str> = prev
        .notes
        .iter()
        .filter_map(|n| n.target.as_deref())
        .collect();
    let mut new_targets: Vec<String> = Vec::new();
    for t in cur.notes.iter().filter_map(|n| n.target.as_deref()) {
        if !old_targets.contains(t) && !new_targets.iter().any(|x| x == t) {
            new_targets.push(t.to_string());
        }
    }
    let risk_delta = i16::from(cur.risk) - i16::from(prev.risk);
    if added.is_empty() && removed.is_empty() && new_targets.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let effects: Vec<String> = added
        .iter()
        .map(|e| format!("+{}", e.id()))
        .chain(removed.iter().map(|e| format!("-{}", e.id())))
        .collect();
    if !effects.is_empty() {
        parts.push(effects.join(", "));
    }
    if !new_targets.is_empty() {
        parts.push(format!("new: {}", new_targets.join(", ")));
    }
    if risk_delta != 0 {
        parts.push(format!("risk {risk_delta:+}"));
    }
    Some(EventDiff {
        added,
        removed,
        new_targets,
        risk_delta,
        summary: parts.join("; "),
    })
}

/// A short, stable fingerprint of a source text: 64-bit FNV-1a, as 12 hex
/// digits. Enough to tell two versions of one script apart; not a checksum
/// anyone should trust against an adversary.
pub fn digest(source: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:012x}", h >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_run_path_and_digest() {
        let dir = std::env::temp_dir().join(format!("pyxray-record-{}", std::process::id()));
        let log = dir.join("feed.jsonl");
        let report = crate::xray("import os\nos.system('ls')\n", "job.py");
        let (event, err) = record(
            &report,
            &RecordOpts {
                source: "shim".into(),
                run: Some("r1".into()),
                origin: Some(PathBuf::from("/work/job.py")),
                feed: Some(log.clone()),
                write: true,
                cwd: None,
                policy: false,
            },
        );
        assert!(err.is_none(), "{err:?}");
        assert_eq!(event.source, "shim");
        assert_eq!(event.run.as_deref(), Some("r1"));
        assert_eq!(event.path.as_deref(), Some("/work/job.py"));
        assert_eq!(event.digest.as_deref().map(str::len), Some(12));
        let back = feed::read_all(&log).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].run.as_deref(), Some("r1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_false_leaves_the_log_alone() {
        let dir = std::env::temp_dir().join(format!("pyxray-record-nw-{}", std::process::id()));
        let log = dir.join("feed.jsonl");
        let report = crate::xray("print(1)\n", "t.py");
        let (_, err) = record(
            &report,
            &RecordOpts {
                feed: Some(log.clone()),
                write: false,
                ..RecordOpts::default()
            },
        );
        assert!(err.is_none());
        assert!(!log.exists());
    }

    #[test]
    fn a_changed_script_is_diffed_against_its_last_run() {
        let dir = std::env::temp_dir().join(format!("pyxray-record-diff-{}", std::process::id()));
        let log = dir.join("feed.jsonl");
        let opts = RecordOpts {
            source: "shim".into(),
            origin: Some(PathBuf::from("/w/job.py")),
            feed: Some(log.clone()),
            policy: false,
            ..RecordOpts::default()
        };
        let v1 = crate::xray("import json\njson.load(open('cfg.json'))\n", "job.py");
        let (e1, _) = record(&v1, &opts);
        assert!(
            e1.changed.is_none(),
            "nothing to compare the first run with"
        );
        let (e1b, _) = record(&v1, &opts);
        assert!(e1b.changed.is_none(), "same code, same digest: no diff");
        let v2 = crate::xray(
            "import json, requests\ncfg = json.load(open('cfg.json'))\nrequests.post('https://api.x.example', json=cfg)\n",
            "job.py",
        );
        let (e2, _) = record(&v2, &opts);
        let d = e2.changed.expect("the new version talks to the network");
        assert_eq!(d.added, vec![crate::model::Effect::Net]);
        assert!(
            d.new_targets.iter().any(|t| t.contains("api.x.example")),
            "{d:?}"
        );
        assert!(d.risk_delta > 0);
        assert!(d.summary.starts_with("+net"), "{}", d.summary);
        // Another script's history is not this one's.
        let other = RecordOpts {
            origin: Some(PathBuf::from("/w/other.py")),
            ..opts.clone()
        };
        assert!(record(&v2, &other).0.changed.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn digest_is_stable_and_discriminating() {
        assert_eq!(digest("a"), digest("a"));
        assert_ne!(digest("a"), digest("b"));
        assert_eq!(digest(""), "cbf29ce48422");
    }

    #[test]
    fn events_without_the_new_fields_serialise_as_before() {
        let report = crate::xray("x = 1\n", "t.py");
        let json = serde_json::to_string(&report.event("hook")).unwrap();
        for key in [
            "\"run\"",
            "\"path\"",
            "\"digest\"",
            "\"policy\"",
            "\"changed\"",
        ] {
            assert!(
                !json.contains(key),
                "{key} leaked into an old-shape event: {json}"
            );
        }
    }
}
