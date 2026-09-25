//! `pyx watch` — a live column of everything Python that went past.
//!
//! This is the answer to a harness firing several snippets a second. Drawing a
//! report per snippet into the terminal the agent is talking through is
//! unreadable and, in most harnesses, feeds straight back to the model as
//! tool output. So the renders come here instead: a second pane, its own
//! window, a tmux split — somewhere a person is watching and the model is not.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Stdout, Write};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use pyxray_core::feed::{self, Tail};
use pyxray_core::model::{Band, Event};
use pyxray_render::export;
use pyxray_render::feed as feedview;
use pyxray_render::panels::Opts;
use pyxray_render::theme::{Theme, THEMES};

/// How many events to keep on screen. Older ones fall off the top; the log on
/// disk keeps them.
const BACKLOG: usize = 2000;

pub struct Options {
    pub path: PathBuf,
    pub theme: Theme,
    pub opts: Opts,
    /// Start from the whole log rather than only what arrives from now on.
    pub replay: bool,
    /// Hide anything below this band.
    pub floor: Band,
    /// Render what is in the log once and exit, instead of following it.
    pub dump: bool,
    /// Output format for `--dump`.
    pub format: export::Format,
    /// Emit the bare `<pre>` rather than a whole document, for embedding.
    pub fragment: bool,
    /// Columns and rows to render at when dumping; `None` asks the terminal.
    pub size: (Option<u16>, Option<u16>),
}

struct State {
    events: VecDeque<Event>,
    stats: feedview::Stats,
    theme: usize,
    floor: Band,
    paused: bool,
    /// Rows scrolled up from the live edge; zero means following.
    scroll: usize,
    notes: bool,
    status: String,
}

pub fn run(o: Options) -> io::Result<()> {
    let mut state = State {
        events: VecDeque::new(),
        stats: feedview::Stats::default(),
        theme: THEMES.iter().position(|t| t.id == o.theme.id).unwrap_or(0),
        floor: o.floor,
        paused: false,
        scroll: 0,
        notes: true,
        status: String::new(),
    };

    state.status = [
        "q quit", "p pause", "f filter", "n notes", "t theme", "c clear",
    ]
    .join(o.theme.gl.sep);

    if o.dump {
        for event in feed::read_all(&o.path).unwrap_or_default() {
            state.stats.push(&event);
            state.events.push_back(event);
        }
        while state.events.len() > BACKLOG {
            state.events.pop_front();
        }
        let (tw, th) = terminal::size().unwrap_or((100, 40));
        let w = o.size.0.unwrap_or(tw).max(40);
        let h = o.size.1.unwrap_or_else(|| {
            // Tall enough for everything, so a dump is never silently cut.
            (state.events.len() as u16).saturating_add(6).max(th)
        });
        let buf = compose(&state, &o, w, h, now_ms());
        let theme = THEMES[state.theme];
        let text = match o.format {
            // A dump with no format asked for follows the terminal: escapes
            // when someone is looking, plain text when it is going into a file.
            export::Format::Ansi if !std::io::stdout().is_terminal() => export::to_text(&buf),
            export::Format::Html if o.fragment => export::to_html_fragment(&buf, &theme),
            other => export::emit(&buf, &theme, other, "pyxray watch"),
        };
        print!("{text}");
        return Ok(());
    }

    let mut tail = if o.replay {
        Tail::at_start(&o.path)
    } else {
        // Seed the totals from the log so the header is not a lie, then follow
        // only what arrives next.
        for event in feed::read_all(&o.path).unwrap_or_default() {
            state.stats.push(&event);
        }
        Tail::at_end(&o.path)
    };

    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
    let result = pump(&mut out, &mut tail, &mut state, &o);
    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

fn pump(out: &mut Stdout, tail: &mut Tail, state: &mut State, o: &Options) -> io::Result<()> {
    loop {
        for event in tail.poll() {
            ingest(state, event);
        }
        draw(out, state, o)?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Action::Quit = apply(state, &event::read()?) {
            return Ok(());
        }
    }
}

/// What the loop should do after an input event.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Quit,
}

/// Take one event from the log: always into the totals, onto the screen
/// unless paused, and never more than [`BACKLOG`] of them.
fn ingest(state: &mut State, event: Event) {
    state.stats.push(&event);
    if !state.paused {
        state.events.push_back(event);
        while state.events.len() > BACKLOG {
            state.events.pop_front();
        }
    }
}

/// Apply one terminal event to the state. Pure apart from the state it is
/// given, so every key can be tested without a terminal.
///
/// Only key *presses* count. Windows reports a release for every press, and
/// acting on both would make every toggle a no-op and every scroll a double.
fn apply(state: &mut State, input: &TermEvent) -> Action {
    match input {
        TermEvent::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) => match code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Action::Quit,
            KeyCode::Char('p') => {
                state.paused = !state.paused;
                state.status = if state.paused {
                    "paused \u{00b7} p to resume".into()
                } else {
                    "following".into()
                };
            }
            KeyCode::Char('f') => {
                state.floor = match state.floor {
                    Band::Inert => Band::Routine,
                    Band::Routine => Band::Check,
                    Band::Check => Band::Read,
                    Band::Read => Band::Inert,
                };
                state.status = format!("showing {} and above", state.floor.word());
            }
            KeyCode::Char('n') => {
                state.notes = !state.notes;
                state.status = format!("notes {}", if state.notes { "on" } else { "off" });
            }
            KeyCode::Char('t') => {
                state.theme = (state.theme + 1) % THEMES.len();
                state.status = format!("theme: {}", THEMES[state.theme].name);
            }
            KeyCode::Char('c') => {
                state.events.clear();
                state.scroll = 0;
                state.status = "cleared the view; the log is untouched".into();
            }
            KeyCode::Up | KeyCode::Char('k') => state.scroll += 1,
            KeyCode::Down | KeyCode::Char('j') => state.scroll = state.scroll.saturating_sub(1),
            KeyCode::PageUp => state.scroll += 10,
            KeyCode::PageDown => state.scroll = state.scroll.saturating_sub(10),
            KeyCode::End => state.scroll = 0,
            _ => {}
        },
        TermEvent::Resize(_, _) => state.scroll = 0,
        _ => {}
    }
    Action::Continue
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn draw(out: &mut Stdout, state: &mut State, o: &Options) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    out.write_all(frame(state, o, w, h, now_ms()).as_bytes())?;
    out.flush()
}

/// One whole screen, as the bytes to write to a raw-mode terminal.
fn frame(state: &mut State, o: &Options, w: u16, h: u16, now: u64) -> String {
    // Scrolling is clamped here rather than at the keypress: only now is the
    // terminal height known, and without this `k` past the oldest row walks the
    // window off the top and leaves a blank body under an honest "N shown".
    let ceiling = rows(state).len().saturating_sub(body_height(h) as usize);
    state.scroll = state.scroll.min(ceiling);
    let buf = compose(state, o, w, h, now);
    export::to_ansi_screen(&buf, &THEMES[state.theme], 1)
}

/// Rows of the body, newest last: one per event that clears the filter, plus a
/// second for the ones carrying a note worth reading. `true` marks a note row.
fn rows(state: &State) -> Vec<(&Event, bool)> {
    let mut rows: Vec<(&Event, bool)> = Vec::new();
    for event in state.events.iter().filter(|e| e.band >= state.floor) {
        rows.push((event, false));
        if state.notes
            && event.band >= Band::Check
            && event
                .notes
                .first()
                .is_some_and(|n| n.severity >= pyxray_core::model::Severity::Caution)
        {
            rows.push((event, true));
        }
    }
    rows
}

/// Rows the body has to draw into, given the whole terminal height. The header
/// and its rule take the top, the status line the bottom.
fn body_height(h: u16) -> u16 {
    h.saturating_sub(3u16.min(h) + 2)
}

/// The screen as a buffer. `now` is the clock the age column is measured
/// against, passed in so a render is a pure function of its inputs.
fn compose(state: &State, o: &Options, w: u16, h: u16, now: u64) -> ratatui::buffer::Buffer {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    let theme = THEMES[state.theme];
    let area = Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    };
    let mut buf = Buffer::empty(area);
    pyxray_render::canvas::fill(&mut buf, area, theme.base());

    let head_h = 3u16.min(h);
    feedview::header(
        &mut buf,
        Rect {
            x: 1,
            y: 0,
            width: w.saturating_sub(2),
            height: head_h,
        },
        &state.stats,
        &o.path.display().to_string(),
        &theme,
    );
    pyxray_render::canvas::hline(&mut buf, 0, head_h, w, theme.gl.h, theme.rule_style());

    let body_top = head_h + 1;
    let body_h = body_height(h);

    // Build the rows newest-last, honouring the filter, then window them.
    let rows = rows(state);
    let total = rows.len();
    let window = body_h as usize;
    let end = total.saturating_sub(state.scroll);
    let start = end.saturating_sub(window);

    for (i, (event, is_note)) in rows[start..end].iter().enumerate() {
        let y = body_top + i as u16;
        let line = Rect {
            x: 1,
            y,
            width: w.saturating_sub(2),
            height: 1,
        };
        if *is_note {
            feedview::note_row(&mut buf, line, event, &theme);
        } else {
            feedview::row(&mut buf, line, event, now, &theme, &o.opts);
        }
    }

    if total == 0 {
        let hint = if state.stats.count == 0 {
            "nothing yet \u{2014} run some Python through a shim or a hook and it will appear here"
        } else {
            "nothing above the current filter \u{2014} press f to widen it"
        };
        pyxray_render::canvas::text(
            &mut buf,
            2,
            body_top + 1,
            w.saturating_sub(4),
            hint,
            theme.faint(),
        );
    }

    // Status line.
    let y = h.saturating_sub(1);
    let mode = if state.paused {
        "paused"
    } else if state.scroll > 0 {
        "scrolled"
    } else {
        "live"
    };
    let sep = theme.gl.sep;
    let left = format!(
        " {mode}{sep}{} shown{sep}{} and above ",
        total,
        state.floor.word()
    );
    let bar = theme.base().bg(theme.pal.rule).fg(theme.pal.fg);
    pyxray_render::canvas::fill(
        &mut buf,
        Rect {
            x: 0,
            y,
            width: w,
            height: 1,
        },
        bar,
    );
    let used = pyxray_render::canvas::text(&mut buf, 0, y, w, &left, bar);
    pyxray_render::canvas::text(
        &mut buf,
        used,
        y,
        w.saturating_sub(used),
        &format!(" {}", state.status),
        bar.fg(theme.pal.dim),
    );

    // The hints and status above, and every synopsis in the rows, carry the
    // analyser's own punctuation; an ASCII theme has to stay ASCII anyway.
    if theme.gl.ellipsis.is_ascii() {
        pyxray_render::layout::asciify(&mut buf);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;
    use pyxray_render::canvas;

    const NOW: u64 = 1_800_000_000_000;

    fn event(src: &str, name: &str, ago_ms: u64) -> Event {
        let mut e = pyxray_core::xray(src, name).event("test");
        e.ts = NOW - ago_ms;
        e
    }

    /// Three events across the bands: one inert, one routine read, one
    /// shell-out whose note earns it a second row.
    fn sample() -> Vec<Event> {
        vec![
            event("x = 1 + 1\n", "inert.py", 90_000),
            event(
                "import json\njson.load(open('cfg.json'))\n",
                "reads.py",
                30_000,
            ),
            event("import os\nos.system('rm -rf build')\n", "shell.py", 5_000),
        ]
    }

    fn state(events: Vec<Event>) -> State {
        let mut s = State {
            events: VecDeque::new(),
            stats: feedview::Stats::default(),
            theme: THEMES.iter().position(|t| t.id == "mono").unwrap_or(0),
            floor: Band::Inert,
            paused: false,
            scroll: 0,
            notes: true,
            status: "q quit".into(),
        };
        for e in events {
            ingest(&mut s, e);
        }
        s
    }

    fn options() -> Options {
        Options {
            path: PathBuf::from("feed.jsonl"),
            theme: THEMES[0],
            opts: Opts::default(),
            replay: false,
            floor: Band::Inert,
            dump: false,
            format: export::Format::Text,
            fragment: false,
            size: (None, None),
        }
    }

    fn text(s: &State, w: u16, h: u16) -> String {
        export::to_text(&compose(s, &options(), w, h, NOW))
    }

    fn press(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn empty_feed() {
        insta::assert_snapshot!(text(&state(vec![]), 80, 8));
    }

    #[test]
    fn three_events_with_a_note_row() {
        insta::assert_snapshot!(text(&state(sample()), 80, 10));
    }

    #[test]
    fn filtered_to_check_and_above() {
        let mut s = state(sample());
        s.floor = Band::Check;
        insta::assert_snapshot!(text(&s, 80, 8));
    }

    #[test]
    fn paused_and_narrow() {
        let mut s = state(sample());
        s.paused = true;
        insta::assert_snapshot!(text(&s, 40, 8));
    }

    #[test]
    fn scrolled_back() {
        let mut s = state(sample());
        s.scroll = 1;
        insta::assert_snapshot!(text(&s, 80, 7));
    }

    /// The regressions fixed in 73f14f6, as invariants: a raw-mode frame has
    /// no newlines, positions every row itself, and never writes past the
    /// right edge — any of which scrolls or smears the screen.
    #[test]
    fn frames_position_every_row_and_fit_the_width() {
        for &w in &[20u16, 40, 80, 200] {
            for &h in &[1u16, 3, 5, 40] {
                for theme in 0..THEMES.len() {
                    let mut s = state(sample());
                    s.theme = theme;
                    let f = frame(&mut s, &options(), w, h, NOW);
                    assert!(!f.contains('\n') && !f.contains('\r'), "newline at {w}x{h}");
                    let positioned = (1..=h)
                        .filter(|y| f.contains(&format!("\x1b[{y};1H")))
                        .count();
                    assert_eq!(positioned, h as usize, "rows positioned at {w}x{h}");
                    for row in visible_rows(&f) {
                        assert!(canvas::width(&row) <= w, "row wider than {w}: {row:?}");
                    }
                }
            }
        }
    }

    /// The text of each screen row, with every escape sequence removed.
    fn visible_rows(frame: &str) -> Vec<String> {
        let mut rows = Vec::new();
        for chunk in frame.split("\x1b[").skip(1) {
            // Each chunk starts with the rest of one CSI sequence; the final
            // byte is the first ASCII letter.
            let end = chunk
                .find(|c: char| c.is_ascii_alphabetic())
                .map_or(0, |i| i + 1);
            let (csi, text) = chunk.split_at(end);
            if csi.ends_with('H') {
                rows.push(String::new());
            }
            if let Some(last) = rows.last_mut() {
                last.push_str(text);
            }
        }
        rows
    }

    /// `mono` promises pure ASCII; the hints, the status line and the
    /// analyser's synopses all bring their own punctuation.
    #[test]
    fn ascii_themes_stay_ascii() {
        for (i, theme) in THEMES.iter().enumerate() {
            if !theme.gl.ellipsis.is_ascii() {
                continue;
            }
            for events in [vec![], sample()] {
                let mut s = state(events);
                s.theme = i;
                s.paused = true;
                apply(&mut s, &press(KeyCode::Char('p')));
                apply(&mut s, &press(KeyCode::Char('p')));
                let out = text(&s, 100, 10);
                assert!(out.is_ascii(), "{}: {out}", theme.id);
            }
        }
    }

    #[test]
    fn a_script_that_gained_a_capability_says_so() {
        let mut e = event(
            "import requests\nrequests.post('https://x.example')\n",
            "job.py",
            1_000,
        );
        e.changed = Some(pyxray_core::model::EventDiff {
            added: vec![pyxray_core::model::Effect::Net],
            removed: vec![],
            new_targets: vec!["https://x.example".into()],
            risk_delta: 10,
            summary: "+net".into(),
        });
        let out = text(&state(vec![e]), 100, 8);
        assert!(out.contains("+net") && out.contains("job.py"), "{out}");
    }

    #[test]
    fn scroll_is_clamped_to_the_oldest_row() {
        let mut s = state(sample());
        s.scroll = 999;
        let h = 6;
        frame(&mut s, &options(), 80, h, NOW);
        assert_eq!(s.scroll, rows(&s).len() - body_height(h) as usize);
    }

    #[test]
    fn keys() {
        let mut s = state(sample());
        assert_eq!(apply(&mut s, &press(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(apply(&mut s, &press(KeyCode::Esc)), Action::Quit);
        let ctrl_c = TermEvent::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        assert_eq!(apply(&mut s, &ctrl_c), Action::Quit);

        let mut seen = vec![s.floor];
        for _ in 0..4 {
            apply(&mut s, &press(KeyCode::Char('f')));
            seen.push(s.floor);
        }
        assert_eq!(
            seen,
            [
                Band::Inert,
                Band::Routine,
                Band::Check,
                Band::Read,
                Band::Inert
            ]
        );

        apply(&mut s, &press(KeyCode::Char('j')));
        assert_eq!(s.scroll, 0, "j at the live edge stays there");
        apply(&mut s, &press(KeyCode::Char('k')));
        apply(&mut s, &press(KeyCode::PageUp));
        assert_eq!(s.scroll, 11);
        apply(&mut s, &press(KeyCode::End));
        assert_eq!(s.scroll, 0);

        apply(&mut s, &press(KeyCode::Char('p')));
        assert!(s.paused);
        ingest(&mut s, event("print(1)\n", "late.py", 0));
        assert_eq!(s.events.len(), 3, "paused: counted, not shown");
        assert_eq!(s.stats.count, 4);

        let count = s.stats.count;
        apply(&mut s, &press(KeyCode::Char('c')));
        assert!(s.events.is_empty());
        assert_eq!(s.stats.count, count, "clearing the view keeps the totals");
    }

    #[test]
    fn a_key_release_does_nothing() {
        let mut s = state(sample());
        let release = TermEvent::Key(KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(apply(&mut s, &release), Action::Continue);
        assert!(!s.paused);
        let quit_release = TermEvent::Key(KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(apply(&mut s, &quit_release), Action::Continue);
    }
}
