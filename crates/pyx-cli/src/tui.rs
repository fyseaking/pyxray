//! The interactive viewer. Its real job is lookdev: every visual decision is
//! on a key, so a look gets chosen by flipping through candidates against real
//! code rather than by reading a description of it.

use std::io::{self, IsTerminal, Stdout, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use pyxray_core::model::Report;
use pyxray_render::export;
use pyxray_render::layout::{self, Layout};
use pyxray_render::panels::Icons;
use pyxray_render::theme::THEMES;

use crate::args::Args;

struct State {
    theme: usize,
    layout: usize,
    icons: Icons,
    depth: u16,
    code: bool,
    gutter: bool,
    scroll: u16,
    status: String,
}

pub fn run(report: &Report, a: &Args) -> io::Result<()> {
    let mut state = State {
        theme: THEMES.iter().position(|t| t.id == a.theme.id).unwrap_or(0),
        layout: Layout::all()
            .iter()
            .position(|l| *l == a.layout)
            .unwrap_or(0),
        icons: a.opts.icons,
        depth: a.opts.max_depth,
        code: a.opts.code,
        gutter: a.opts.gutter,
        scroll: 0,
        status: [
            "t theme", "l layout", "i icons", "d depth", "c code", "s save", "q quit",
        ]
        .join(a.theme.gl.sep),
    };

    if !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "--tui needs a terminal on stdout; use -f text or -o FILE to write elsewhere",
        ));
    }
    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
    let result = event_loop(&mut out, report, &mut state);
    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

fn event_loop(out: &mut Stdout, report: &Report, state: &mut State) -> io::Result<()> {
    loop {
        draw(out, report, state)?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match apply(state, &event::read()?) {
            Action::Quit => return Ok(()),
            Action::Save => state.status = save(report, state),
            Action::Continue => {}
        }
    }
}

/// What the loop should do after an input event. Saving is the one action
/// with a side effect outside the state, so it is handed back rather than
/// done here.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Quit,
    Save,
}

/// Apply one terminal event to the viewer state. Presses only: Windows
/// reports a release for each press too.
fn apply(state: &mut State, input: &Event) -> Action {
    match input {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) => {
            let shift = modifiers.contains(KeyModifiers::SHIFT);
            match code {
                KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    return Action::Quit
                }
                // Terminals disagree on whether shift+t arrives as 'T' with
                // SHIFT, 'T' alone, or 't' with SHIFT; all three step back.
                KeyCode::Char(c @ ('t' | 'T')) => {
                    state.theme = step(state.theme, THEMES.len(), shift || *c == 'T');
                    state.status = format!("theme: {}", THEMES[state.theme].name);
                }
                KeyCode::Char(c @ ('l' | 'L')) => {
                    state.layout = step(state.layout, Layout::all().len(), shift || *c == 'L');
                    state.status = format!("layout: {}", Layout::all()[state.layout].id());
                }
                KeyCode::Char('i') => {
                    state.icons = match state.icons {
                        Icons::Glyph => Icons::Tag,
                        Icons::Tag => Icons::Both,
                        Icons::Both => Icons::Glyph,
                    };
                    state.status = format!("icons: {:?}", state.icons);
                }
                KeyCode::Char('c') => {
                    state.code = !state.code;
                    state.status = format!("source: {}", on_off(state.code));
                }
                KeyCode::Char('g') => {
                    state.gutter = !state.gutter;
                    state.status = format!("line numbers: {}", on_off(state.gutter));
                }
                KeyCode::Char('d') => {
                    state.depth = state.depth.saturating_add(1).min(24);
                    state.status = format!("depth: {}", state.depth);
                }
                KeyCode::Char('D') => {
                    state.depth = state.depth.saturating_sub(1).max(1);
                    state.status = format!("depth: {}", state.depth);
                }
                KeyCode::Char('s') => return Action::Save,
                KeyCode::Down | KeyCode::Char('j') => state.scroll = state.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => state.scroll = state.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    state.scroll = state.scroll.saturating_add(10)
                }
                KeyCode::PageUp => state.scroll = state.scroll.saturating_sub(10),
                KeyCode::Home => state.scroll = 0,
                _ => {}
            }
        }
        Event::Resize(_, _) => state.scroll = 0,
        _ => {}
    }
    Action::Continue
}

fn step(current: usize, len: usize, back: bool) -> usize {
    if back {
        (current + len - 1) % len
    } else {
        (current + 1) % len
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

fn opts(state: &State) -> pyxray_render::Opts {
    pyxray_render::Opts {
        max_depth: state.depth,
        gutter: state.gutter,
        icons: state.icons,
        code: state.code,
    }
}

fn draw(out: &mut Stdout, report: &Report, state: &State) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    out.write_all(frame(report, state, w, h).as_bytes())?;
    out.flush()
}

/// One whole screen, as the bytes to write to a raw-mode terminal: the
/// render windowed to the body, then the status bar on the last row.
fn frame(report: &Report, state: &State, w: u16, h: u16) -> String {
    let theme = THEMES[state.theme];
    let l = Layout::all()[state.layout];
    let body_h = h.saturating_sub(1).max(1);

    // Render tall, then window into it — that keeps scrolling honest even for
    // layouts that would rather be longer than the terminal.
    let buf = layout::render(report, &theme, &opts(state), l, w, None);
    let total = buf.area.height;
    let max_scroll = total.saturating_sub(body_h);
    let top = state.scroll.min(max_scroll);

    let mut screen = String::with_capacity((w as usize) * (h as usize) * 4);
    // Some layouts have a floor on their width; on a terminal narrower than
    // that the render is wider than the screen, so window it sideways too.
    let window = crop(&buf, top, body_h, w);
    screen.push_str(&export::to_ansi_screen(&window, &theme, 1));
    // A render shorter than the body leaves the rows under it holding the
    // previous layout; wipe them rather than let two renders overlap.
    if window.area.height < body_h {
        screen.push_str(&format!("\x1b[{};1H\x1b[J", window.area.height + 1));
    }
    if h > body_h {
        screen.push_str(&status_bar(state, &theme, l, top, body_h, total, w, h));
    }
    screen
}

/// The bottom row. Always exactly `w` cells: one over and the terminal wraps
/// onto a new row, which on the bottom row scrolls the whole render up.
#[allow(clippy::too_many_arguments)]
fn status_bar(
    state: &State,
    theme: &pyxray_render::theme::Theme,
    l: Layout,
    top: u16,
    body_h: u16,
    total: u16,
    w: u16,
    h: u16,
) -> String {
    use pyxray_render::canvas::{fit, width};

    let bar = ratatui::style::Style::default()
        .bg(theme.pal.rule)
        .fg(theme.pal.fg);
    let sep = theme.gl.sep;
    let el = theme.gl.ellipsis;
    // Most important first: where you are, then what is on, then the hint.
    let right = if total > body_h {
        format!(" {}/{} ", top + body_h.min(total), total)
    } else {
        String::new()
    };
    let right = fit(&right, w, el);
    let left = format!(
        " {}{sep}{}{sep}{:?}{sep}depth {} ",
        theme.name,
        l.id(),
        state.icons,
        state.depth
    );
    let left = fit(&left, w.saturating_sub(width(&right)), el);
    let room = w.saturating_sub(width(&left) + width(&right));
    // The hint was joined with the separator of whichever theme was active
    // when the viewer opened; an ASCII theme must not inherit a `·`.
    let hint: String = if el.is_ascii() {
        state
            .status
            .chars()
            .map(|c| match c {
                '\u{00b7}' | '\u{2022}' => '.',
                '\u{2014}' | '\u{2013}' => '-',
                '\u{2026}' => '~',
                '\u{2192}' => '>',
                c if c.is_ascii() => c,
                _ => '?',
            })
            .collect()
    } else {
        state.status.clone()
    };
    let status = if room > 2 {
        format!(" {} ", fit(&hint, room - 2, el))
    } else {
        String::new()
    };
    let pad = room.saturating_sub(width(&status)) as usize;
    format!(
        "\x1b[{h};1H{}{left}\x1b[2m{status}\x1b[22m{}{right}\x1b[0m",
        export::sgr(bar, theme),
        " ".repeat(pad)
    )
}

/// Take `rows` rows starting at `top`, and at most `cols` columns, out of a
/// larger buffer.
fn crop(buf: &ratatui::buffer::Buffer, top: u16, rows: u16, cols: u16) -> ratatui::buffer::Buffer {
    let area = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: buf.area.width.min(cols.max(1)),
        height: rows.min(buf.area.height.saturating_sub(top)).max(1),
    };
    let mut out = ratatui::buffer::Buffer::empty(area);
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(src) = buf.cell((x, y + top)) {
                if let Some(dst) = out.cell_mut((x, y)) {
                    *dst = src.clone();
                }
            }
        }
    }
    out
}

fn save(report: &Report, state: &State) -> String {
    let theme = THEMES[state.theme];
    let l = Layout::all()[state.layout];
    let buf = layout::render(report, &theme, &opts(state), l, 120, None);
    let stem = format!("pyxray-{}-{}", theme.id, l.id());
    let html = export::to_html(&buf, &theme, &report.meta.name);
    let svg = export::to_svg(&buf, &theme, &report.meta.name);
    match (
        std::fs::write(format!("{stem}.html"), html),
        std::fs::write(format!("{stem}.svg"), svg),
    ) {
        (Ok(()), Ok(())) => format!("saved {stem}.html and {stem}.svg"),
        _ => "could not write files here".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;
    use pyxray_render::canvas;

    const SRC: &str = "import os, json\nimport requests\n\ncfg = json.load(open('cfg.json'))\nr = requests.get(cfg['url'])\nos.system('rm -rf build')\nfor i in range(3):\n    print(i)\n";

    fn state() -> State {
        State {
            theme: 0,
            layout: 0,
            icons: Icons::Both,
            depth: 6,
            code: false,
            gutter: true,
            scroll: 0,
            status: ["t theme", "l layout", "s save", "q quit"].join(THEMES[0].gl.sep),
        }
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    /// Split a raw-mode frame into the visible text of each positioned row.
    fn rows(frame: &str) -> Vec<(u16, String)> {
        let mut rows: Vec<(u16, String)> = Vec::new();
        for chunk in frame.split("\x1b[").skip(1) {
            let end = chunk
                .find(|c: char| c.is_ascii_alphabetic())
                .map_or(0, |i| i + 1);
            let (csi, text) = chunk.split_at(end);
            if let Some(pos) = csi.strip_suffix('H') {
                let row = pos
                    .split(';')
                    .next()
                    .and_then(|r| r.parse().ok())
                    .unwrap_or(0);
                rows.push((row, String::new()));
            }
            if let Some(last) = rows.last_mut() {
                last.1.push_str(text);
            }
        }
        rows
    }

    #[test]
    fn every_frame_fits_and_the_status_bar_is_exactly_the_width() {
        let report = pyxray_core::xray(SRC, "sample.py");
        for &w in &[20u16, 40, 80, 200] {
            for &h in &[2u16, 5, 24, 60] {
                for (theme, t) in THEMES.iter().enumerate() {
                    for (layout, l) in Layout::all().iter().enumerate() {
                        let mut s = state();
                        s.theme = theme;
                        s.layout = layout;
                        let f = frame(&report, &s, w, h);
                        let at = format!("{w}x{h} {} {l:?}", t.id);
                        assert!(!f.contains('\n') && !f.contains('\r'), "newline at {at}");
                        for (row, text) in rows(&f) {
                            assert!(row >= 1 && row <= h, "row {row} off screen at {at}");
                            assert!(
                                canvas::width(&text) <= w,
                                "row {row} too wide at {at}: {text:?}"
                            );
                            if row == h {
                                assert_eq!(
                                    canvas::width(&text),
                                    w,
                                    "status bar width at {at}: {text:?}"
                                );
                            }
                        }
                        if t.gl.ellipsis.is_ascii() {
                            let visible: String = rows(&f).into_iter().map(|r| r.1).collect();
                            assert!(visible.is_ascii(), "non-ASCII in {at}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_short_render_clears_below_itself() {
        let report = pyxray_core::xray("x = 1\n", "tiny.py");
        let f = frame(&report, &state(), 80, 60);
        assert!(f.contains("\x1b[J"), "no erase below a short render");
    }

    #[test]
    fn keys() {
        let mut s = state();
        let none = KeyModifiers::NONE;
        assert_eq!(
            apply(&mut s, &press(KeyCode::Char('q'), none)),
            Action::Quit
        );
        assert_eq!(
            apply(&mut s, &press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        assert_eq!(
            apply(&mut s, &press(KeyCode::Char('s'), none)),
            Action::Save
        );

        apply(&mut s, &press(KeyCode::Char('T'), KeyModifiers::SHIFT));
        assert_eq!(s.theme, THEMES.len() - 1, "shift+t steps back and wraps");
        apply(&mut s, &press(KeyCode::Char('t'), none));
        assert_eq!(s.theme, 0);
        apply(&mut s, &press(KeyCode::Char('T'), none));
        assert_eq!(
            s.theme,
            THEMES.len() - 1,
            "'T' without the modifier also steps back"
        );

        for _ in 0..30 {
            apply(&mut s, &press(KeyCode::Char('d'), none));
        }
        assert_eq!(s.depth, 24);
        for _ in 0..30 {
            apply(&mut s, &press(KeyCode::Char('D'), none));
        }
        assert_eq!(s.depth, 1);

        apply(&mut s, &press(KeyCode::Char('k'), none));
        assert_eq!(s.scroll, 0);
        apply(&mut s, &press(KeyCode::PageDown, none));
        apply(&mut s, &press(KeyCode::Char('j'), none));
        assert_eq!(s.scroll, 11);
        apply(&mut s, &Event::Resize(10, 10));
        assert_eq!(s.scroll, 0);

        let release = Event::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: none,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        let code = s.code;
        assert_eq!(apply(&mut s, &release), Action::Continue);
        assert_eq!(s.code, code, "a release toggles nothing");
    }
}
