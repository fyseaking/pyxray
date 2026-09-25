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
}

impl Default for RecordOpts {
    fn default() -> Self {
        RecordOpts {
            source: "cli".into(),
            run: None,
            origin: None,
            feed: None,
            write: true,
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

    if !o.write {
        return (event, None);
    }
    let target = o.feed.clone().unwrap_or_else(feed::default_path);
    let error = feed::append(&target, &event)
        .err()
        .map(|e| format!("{}: {e}", target.display()));
    (event, error)
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
    fn digest_is_stable_and_discriminating() {
        assert_eq!(digest("a"), digest("a"));
        assert_ne!(digest("a"), digest("b"));
        assert_eq!(digest(""), "cbf29ce48422");
    }

    #[test]
    fn events_without_the_new_fields_serialise_as_before() {
        let report = crate::xray("x = 1\n", "t.py");
        let json = serde_json::to_string(&report.event("hook")).unwrap();
        for key in ["\"run\"", "\"path\"", "\"digest\""] {
            assert!(
                !json.contains(key),
                "{key} leaked into an old-shape event: {json}"
            );
        }
    }
}
