//! The live feed: many snippets, one row each.
//!
//! The row is the whole visual language in miniature — a band block whose
//! colour *and* length say how much attention is owed, then the fixed
//! thirteen-slot capability barcode, then the sentence. Because every
//! capability owns a permanent column, a column of these rows can be read
//! downwards: the same shape means the same thing, every time, and a snippet
//! that suddenly lights the delete slot is visible before a word is read.
//!
//! The header's capability histogram sits in exactly those columns too, so the
//! session's totals line up with the rows underneath them.

use std::collections::VecDeque;

use pyxray_core::model::{Band, Effect, EffectMask, Event, Severity};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::canvas::{self, fit, width};
use crate::panels::{barcode, barcode_width, Icons, Opts};
use crate::theme::Theme;

/// Column geometry, shared by the rows and the header so they line up.
pub const AGE_W: u16 = 6;
pub const BAND_W: u16 = 3;
pub const RISK_W: u16 = 5;
/// Where the barcode starts, in every row and in the header.
pub const BAR_X: u16 = AGE_W + BAND_W + RISK_W;

/// Running totals over the session so far.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub count: u64,
    pub blocked: u64,
    /// Counts per band, indexed by `Band as usize`.
    pub bands: [u64; 4],
    /// Counts per capability, in `Effect::BARCODE` order.
    pub caps: [u64; Effect::COUNT],
    /// Recent (risk, band) pairs, oldest first, for the sparkline: height
    /// from the score, colour from the band the row itself shows.
    pub recent: VecDeque<(u8, Band)>,
    /// Epoch milliseconds of the first and last event seen.
    pub first_ts: u64,
    pub last_ts: u64,
}

impl Stats {
    pub fn push(&mut self, event: &Event) {
        self.count += 1;
        if event.blocked {
            self.blocked += 1;
        }
        self.bands[event.band as usize] += 1;
        let mask = EffectMask(event.mask);
        for (slot, effect) in Effect::BARCODE.iter().enumerate() {
            if mask.contains(*effect) {
                self.caps[slot] += 1;
            }
        }
        self.recent.push_back((event.risk, event.band));
        while self.recent.len() > 240 {
            self.recent.pop_front();
        }
        if self.first_ts == 0 {
            self.first_ts = event.ts;
        }
        self.last_ts = event.ts;
    }

    /// Snippets per minute over the window actually observed. Returns `None`
    /// until there is enough of a window to divide by.
    pub fn rate(&self) -> Option<f32> {
        let span = self.last_ts.saturating_sub(self.first_ts);
        if self.count < 2 || span < 1000 {
            return None;
        }
        Some(self.count as f32 * 60_000.0 / span as f32)
    }

    /// How many wanted a second look.
    pub fn flagged(&self) -> u64 {
        self.bands[Band::Check as usize] + self.bands[Band::Read as usize]
    }
}

fn band_colour(band: Band, t: &Theme) -> ratatui::style::Color {
    t.risk_color(band)
}

/// Age as something a person reads at a glance rather than converts.
fn age(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=1 => "now".into(),
        2..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// One event, one row.
pub fn row(buf: &mut Buffer, a: Rect, event: &Event, now_ms: u64, t: &Theme, o: &Opts) {
    let band = event.band;
    let colour = band_colour(band, t);

    let mut x = a.x;
    let stamp = format!("{:>4} ", age(now_ms, event.ts));
    x += canvas::text(buf, x, a.y, AGE_W, &stamp, t.faint());

    for i in 0..BAND_W {
        let lit = (i as usize) < band.weight();
        canvas::text(
            buf,
            x + i,
            a.y,
            1,
            &if lit { t.gl.block } else { ' ' }.to_string(),
            Style::default().fg(colour),
        );
    }
    x += BAND_W;

    x += canvas::text(
        buf,
        x,
        a.y,
        RISK_W,
        &format!("{:>4} ", event.risk),
        Style::default()
            .fg(colour)
            .add_modifier(if band >= Band::Check {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );

    x += barcode(
        buf,
        x,
        a.y,
        EffectMask(event.mask),
        t,
        o.icons == Icons::Glyph,
    );
    x += canvas::text(buf, x, a.y, 2, "  ", t.faint());

    let name = fit(&event.name, (a.width / 5).max(8), t.gl.ellipsis);
    let blocked = if event.blocked { " blocked" } else { "" };
    // A script that can do something new since its last run says so, in the
    // tail beside its name: `+net`. The detail is in the event.
    let changed = event
        .changed
        .as_ref()
        .filter(|d| !d.added.is_empty())
        .map(|d| {
            d.added
                .iter()
                .map(|e| format!("+{}", e.id()))
                .collect::<Vec<_>>()
                .join(" ")
                + " "
        })
        .unwrap_or_default();
    let tail_w = width(&name) + width(blocked) + width(&changed) + 2;
    let room = a.right().saturating_sub(x).saturating_sub(tail_w);

    let text = t.localise(&event.synopsis);
    canvas::text(
        buf,
        x,
        a.y,
        room,
        &fit(&text, room, t.gl.ellipsis),
        Style::default()
            .fg(if band >= Band::Check {
                t.pal.fg
            } else {
                t.pal.dim
            })
            .add_modifier(if band == Band::Read {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );

    let mut tail: Vec<(&str, Style)> = Vec::new();
    if !changed.is_empty() {
        tail.push((changed.as_str(), Style::default().fg(t.pal.warn)));
    }
    if event.blocked {
        tail.push((
            "blocked ",
            Style::default()
                .fg(t.pal.danger)
                .add_modifier(Modifier::BOLD),
        ));
    }
    tail.push((name.as_str(), t.faint()));
    canvas::segs_right_bounded(buf, x + room, a.right(), a.y, &tail);
}

/// The note under a row that earned one — only ever drawn for the worst
/// finding, and only when the band says it is worth a second line.
pub fn note_row(buf: &mut Buffer, a: Rect, event: &Event, t: &Theme) -> bool {
    let Some(note) = event.notes.first() else {
        return false;
    };
    if note.severity < Severity::Caution {
        return false;
    }
    let mut x = a.x + BAR_X;
    x += canvas::text(buf, x, a.y, 4, &format!("L{} ", note.line), t.faint());
    let head = format!("{} {}", note.verb, note.target.as_deref().unwrap_or(""));
    x += canvas::text(
        buf,
        x,
        a.y,
        (a.width / 3).max(10),
        &fit(head.trim(), (a.width / 3).max(10), t.gl.ellipsis),
        Style::default().fg(t.effect_color(note.effect)),
    );
    if let Some(why) = &note.note {
        let room = a.right().saturating_sub(x + 2);
        canvas::text(
            buf,
            x + 2,
            a.y,
            room,
            &fit(why, room, t.gl.ellipsis),
            Style::default().fg(t.pal.danger),
        );
    }
    true
}

/// Three rows: what the session has seen, a risk sparkline, and the barcode's
/// key sitting over the columns it explains.
pub fn header(buf: &mut Buffer, a: Rect, stats: &Stats, source: &str, t: &Theme) {
    let brand = Style::default()
        .fg(t.pal.accent)
        .add_modifier(Modifier::BOLD);
    let mut x = a.x;
    x += canvas::text(buf, x, a.y, a.width, "pyxray watch", brand);
    x += canvas::text(buf, x, a.y, 3, "   ", t.faint());

    let rate = match stats.rate() {
        Some(r) if r >= 10.0 => format!("{r:.0}/min"),
        Some(r) => format!("{r:.1}/min"),
        None => t.gl.dash.to_string(),
    };
    let sep = t.gl.sep;
    let counts = format!(
        "{} snippet{} {sep} {rate}",
        stats.count,
        if stats.count == 1 { "" } else { "s" }
    );
    x += canvas::text(
        buf,
        x,
        a.y,
        a.width.saturating_sub(x - a.x),
        &counts,
        t.dim(),
    );

    if stats.flagged() > 0 {
        let flagged = format!(" {sep} {} flagged", stats.flagged());
        x += canvas::text(
            buf,
            x,
            a.y,
            a.width.saturating_sub(x - a.x),
            &flagged,
            Style::default().fg(t.pal.warn),
        );
    }
    if stats.blocked > 0 {
        let blocked = format!(" {sep} {} blocked", stats.blocked);
        x += canvas::text(
            buf,
            x,
            a.y,
            a.width.saturating_sub(x - a.x),
            &blocked,
            Style::default()
                .fg(t.pal.danger)
                .add_modifier(Modifier::BOLD),
        );
    }
    // The log's path goes at the right, but only into the space the counts
    // left: drawn over them it reads as one garbled word.
    if canvas::width(source) + 2 <= a.right().saturating_sub(x) {
        canvas::segs_right_bounded(buf, x, a.right(), a.y, &[(source, t.faint())]);
    }

    if a.height < 2 {
        return;
    }

    // Risk over time, most recent at the right.
    canvas::text(buf, a.x, a.y + 1, AGE_W, " risk", t.faint());
    let spark_w = a.width.saturating_sub(BAR_X + barcode_width() + 6);
    let start = stats.recent.len().saturating_sub(spark_w as usize);
    for (i, (risk, band)) in stats.recent.iter().skip(start).enumerate() {
        let ch = t.ramp((*risk as f32 / 100.0).max(0.06));
        canvas::text(
            buf,
            a.x + AGE_W + i as u16,
            a.y + 1,
            1,
            &ch.to_string(),
            Style::default().fg(band_colour(*band, t)),
        );
    }

    if a.height < 3 {
        return;
    }

    // The key, in the columns it explains.
    canvas::text(buf, a.x, a.y + 2, BAR_X, "  seen", t.faint());
    let mut slot = 0usize;
    let mut cx = a.x + BAR_X;
    let peak = stats.caps.iter().copied().max().unwrap_or(0).max(1) as f32;
    for (group, size) in Effect::BARCODE_GROUPS.iter().enumerate() {
        if group > 0 {
            cx += canvas::text(buf, cx, a.y + 2, 1, " ", t.faint());
        }
        for _ in 0..*size {
            let effect = Effect::BARCODE[slot];
            let count = stats.caps[slot];
            slot += 1;
            let (ch, style) = if count == 0 {
                (t.effect_icon(effect), Style::default().fg(t.pal.rule))
            } else {
                (
                    t.ramp(count as f32 / peak),
                    Style::default().fg(t.effect_color(effect)),
                )
            };
            cx += canvas::text(buf, cx, a.y + 2, 1, &ch.to_string(), style);
        }
    }
    let key = format!(
        "  {}",
        ["fs", "world", "eval", "work", "out"].join(t.gl.sep)
    );
    canvas::text(
        buf,
        cx,
        a.y + 2,
        a.right().saturating_sub(cx),
        &key,
        t.faint(),
    );
}
