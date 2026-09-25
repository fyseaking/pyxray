//! The event feed: one JSON object per line, appended by whatever is watching
//! Python go past, tailed by `pyx watch`.
//!
//! A harness can fire several snippets a second, and drawing a report per
//! snippet into the terminal the agent is already talking through is both
//! unreadable and, in most harnesses, fed straight back to the model as tool
//! output. So the render goes somewhere else: a file here, a second pane
//! there. This module is the contract between the two ends.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::model::Event;

/// Keep the log bounded. A busy session appends a few hundred bytes per
/// snippet; at this cap that is tens of thousands of them, and the oldest
/// half is dropped rather than growing without limit on a machine whose home
/// quota may be nearly full.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Where the feed lives when nobody said otherwise.
///
/// Prefers the per-user runtime directory: it is a tmpfs on every systemd
/// machine, so a chatty session costs no disk quota and the log dies with the
/// login, which is the right lifetime for it. Elsewhere it falls back to a
/// directory that is still *per user* — `%LOCALAPPDATA%` on Windows, the
/// per-user `$TMPDIR` on macOS, then the cache directory — rather than a
/// shared, guessable path in `/tmp`.
pub fn default_path() -> PathBuf {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    if let Some(explicit) = var("PYXRAY_LOG") {
        return PathBuf::from(explicit);
    }
    if let Some(runtime) = var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("pyxray").join("feed.jsonl");
    }
    if cfg!(windows) {
        if let Some(local) = var("LOCALAPPDATA") {
            return PathBuf::from(local).join("pyxray").join("feed.jsonl");
        }
    }
    if cfg!(target_os = "macos") {
        // macOS gives every user their own $TMPDIR under /var/folders.
        if let Some(tmp) = var("TMPDIR") {
            return PathBuf::from(tmp).join("pyxray").join("feed.jsonl");
        }
    }
    if let Some(cache) = var("XDG_CACHE_HOME") {
        return PathBuf::from(cache).join("pyxray").join("feed.jsonl");
    }
    if let Some(home) = var("HOME").or_else(|| var("USERPROFILE")) {
        return PathBuf::from(home)
            .join(".cache")
            .join("pyxray")
            .join("feed.jsonl");
    }
    let who = var("USER")
        .or_else(|| var("USERNAME"))
        .unwrap_or_else(|| "user".into());
    std::env::temp_dir()
        .join(format!("pyxray-{who}"))
        .join("feed.jsonl")
}

/// Append one event. Creates the parent directory on first use, and trims the
/// file from the front once it passes [`MAX_BYTES`].
pub fn append(path: &Path, event: &Event) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(event).map_err(std::io::Error::other)?;
    line.push('\n');

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;

    if file.metadata()?.len() > MAX_BYTES {
        trim(path)?;
    }
    Ok(())
}

/// Drop the oldest half of the log, keeping whole lines.
fn trim(path: &Path) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let cut = text.len() / 2;
    let keep = match text[cut..].find('\n') {
        Some(offset) => &text[cut + offset + 1..],
        None => "",
    };
    std::fs::write(path, keep)
}

/// Read every event currently in the log, ignoring lines that do not parse —
/// a half-written line from a concurrent append is normal, not an error.
pub fn read_all(path: &Path) -> std::io::Result<Vec<Event>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Some(event) = parse_line(&line) {
            out.push(event);
        }
    }
    Ok(out)
}

/// Every line of either kind currently in the log.
pub fn read_all_lines(path: &Path) -> std::io::Result<Vec<FeedLine>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| parse_any(&l))
        .collect())
}

/// One feed line → one event, or nothing: a half-written line from a
/// concurrent append is normal, and so is a line written by a newer build
/// whose schema this one does not understand.
pub fn parse_line(line: &str) -> Option<Event> {
    let event = serde_json::from_str::<Event>(line).ok()?;
    (event.schema <= crate::model::SCHEMA).then_some(event)
}

/// Either kind of feed line.
#[derive(Clone, Debug)]
pub enum FeedLine {
    Event(Box<Event>),
    Observed(crate::model::Observation),
}

/// One feed line of either kind — an event, or an observation from the
/// runtime audit hook — or nothing.
pub fn parse_any(line: &str) -> Option<FeedLine> {
    if line.contains("\"kind\":\"observed\"") || line.contains("\"kind\": \"observed\"") {
        let obs = serde_json::from_str::<crate::model::Observation>(line).ok()?;
        return (obs.kind == "observed" && obs.schema <= crate::model::SCHEMA)
            .then_some(FeedLine::Observed(obs));
    }
    parse_line(line).map(|e| FeedLine::Event(Box::new(e)))
}

/// Append one observation, the way [`append`] appends an event.
pub fn append_observation(path: &Path, obs: &crate::model::Observation) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(obs).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

/// A cursor over a growing log, for tailing it.
pub struct Tail {
    path: PathBuf,
    offset: u64,
    /// Carried over when a read lands mid-line.
    partial: String,
    /// The first bytes of the file at the last poll. Length alone cannot spot
    /// a log that was trimmed and refilled to the same size, which is exactly
    /// what a rotation looks like from here.
    signature: Vec<u8>,
}

impl Tail {
    /// Start at the end of whatever is already there.
    pub fn at_end(path: &Path) -> Tail {
        let offset = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Tail {
            path: path.to_path_buf(),
            offset,
            partial: String::new(),
            signature: head_bytes(path),
        }
    }

    pub fn at_start(path: &Path) -> Tail {
        Tail {
            path: path.to_path_buf(),
            offset: 0,
            partial: String::new(),
            signature: head_bytes(path),
        }
    }

    /// Every whole event appended since the last call. A file that shrank was
    /// trimmed or replaced, so the cursor restarts from the beginning.
    pub fn poll(&mut self) -> Vec<Event> {
        self.poll_lines()
            .into_iter()
            .filter_map(|l| match l {
                FeedLine::Event(e) => Some(*e),
                FeedLine::Observed(_) => None,
            })
            .collect()
    }

    /// Every whole line of either kind appended since the last call.
    pub fn poll_lines(&mut self) -> Vec<FeedLine> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let signature = head_bytes(&self.path);
        if len < self.offset || signature != self.signature {
            self.offset = 0;
            self.partial.clear();
        }
        self.signature = signature;
        if len == self.offset {
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = String::new();
        let read = {
            use std::io::Read;
            let mut bytes = Vec::new();
            match file.take(len - self.offset).read_to_end(&mut bytes) {
                Ok(n) => {
                    buf = String::from_utf8_lossy(&bytes).into_owned();
                    n as u64
                }
                Err(_) => 0,
            }
        };
        self.offset += read;

        let mut text = std::mem::take(&mut self.partial);
        text.push_str(&buf);
        let mut out = Vec::new();
        let ends_clean = text.ends_with('\n');
        let mut pieces: Vec<&str> = text.split('\n').collect();
        if !ends_clean {
            self.partial = pieces.pop().unwrap_or("").to_string();
        }
        for piece in pieces {
            if piece.is_empty() {
                continue;
            }
            if let Some(line) = parse_any(piece) {
                out.push(line);
            }
        }
        out
    }
}

/// The first bytes of a file, used as a cheap identity check between polls.
fn head_bytes(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(64).read_to_end(&mut buf);
    buf
}

/// The most recent event in the log for the script at `path`, looking back
/// through at most the last 512 KiB — a diff against a run from last week is
/// not what anybody watching an agent wants.
pub fn previous_for(log: &Path, path: &str) -> Option<Event> {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 512 << 10;
    let mut file = std::fs::File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .rev()
        .filter_map(parse_line)
        .find(|e| e.path.as_deref() == Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xray;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pyxray-test-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn appends_and_reads_back() {
        let path = tmp("roundtrip");
        let _ = std::fs::remove_file(&path);
        for source in ["print(1)", "import os\nos.remove('x')"] {
            append(&path, &xray(source, "t").event("test")).unwrap();
        }
        let events = read_all(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[1].risk > events[0].risk);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn event_without_schema_still_parses_and_newer_schema_is_skipped() {
        // A line exactly as builds before versioning wrote it.
        let old = r#"{"ts":1,"name":"x","source":"shim","lines":1,"risk":0,"band":"inert","mask":0,"worst":null,"synopsis":"prints results","notes":[],"blocked":false}"#;
        let event = parse_line(old).expect("pre-versioning line parses");
        assert_eq!(event.schema, 0);
        let newer = old.replacen(
            "{",
            &format!("{{\"schema\":{},", crate::model::SCHEMA + 1),
            1,
        );
        assert!(parse_line(&newer).is_none(), "{newer}");
        assert!(parse_line("{\"ts\":").is_none());
    }

    #[test]
    fn the_default_path_is_never_a_shared_tmp_dir() {
        // Can't clear the environment safely in-process; check the shape of
        // the answer instead: whatever it is, it is not the old shared path.
        let p = default_path();
        let text = p.display().to_string();
        assert!(!text.contains("pyxray-user"), "{text}");
        assert!(text.ends_with("feed.jsonl"), "{text}");
    }

    #[test]
    fn tail_sees_only_new_events() {
        let path = tmp("tail");
        let _ = std::fs::remove_file(&path);
        append(&path, &xray("print(1)", "a").event("test")).unwrap();
        let mut tail = Tail::at_end(&path);
        assert!(tail.poll().is_empty());
        append(&path, &xray("print(2)", "b").event("test")).unwrap();
        let seen = tail.poll();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].name, "b");
        assert!(tail.poll().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_truncated_log_restarts_the_cursor() {
        let path = tmp("trunc");
        let _ = std::fs::remove_file(&path);
        append(&path, &xray("print(1)", "a").event("test")).unwrap();
        let mut tail = Tail::at_end(&path);
        std::fs::write(&path, "").unwrap();
        append(&path, &xray("print(2)", "b").event("test")).unwrap();
        assert_eq!(tail.poll().len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn observations_are_a_second_kind_of_line_old_readers_skip() {
        let obs = r#"{"kind":"observed","schema":1,"ts":5,"run":"r1","pid":42,"name":"job.py","items":[{"effect":"net","verb":"connect","target":"1.2.3.4:443","count":2}],"truncated":false}"#;
        assert!(parse_line(obs).is_none(), "an event reader must skip it");
        match parse_any(obs) {
            Some(FeedLine::Observed(o)) => {
                assert_eq!(o.run, "r1");
                assert_eq!(o.items[0].effect, crate::model::Effect::Net);
            }
            other => panic!("{other:?}"),
        }
        let event = serde_json::to_string(&crate::xray("x = 1\n", "t").event("t")).unwrap();
        assert!(matches!(parse_any(&event), Some(FeedLine::Event(_))));

        let dir = std::env::temp_dir().join(format!("pyxray-feed-obs-{}", std::process::id()));
        let log = dir.join("feed.jsonl");
        let _ = std::fs::remove_dir_all(&dir);
        let mut tail = Tail::at_start(&log);
        append(&log, &crate::xray("x = 1\n", "t").event("t")).unwrap();
        let o: crate::model::Observation = serde_json::from_str(obs).unwrap();
        append_observation(&log, &o).unwrap();
        let lines = tail.poll_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            read_all(&log).unwrap().len(),
            1,
            "read_all stays events-only"
        );
        assert_eq!(read_all_lines(&log).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
