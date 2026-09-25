//! The panels. Each one knows how tall it wants to be and how to draw itself;
//! layouts do nothing but place them.

use pyxray_core::model::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::canvas::{self, fit, width};

fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

use crate::theme::{effect_tag, Theme};

/// How effects are labelled. A useful A/B axis on its own: glyphs are dense
/// and quiet, words are unmissable and wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icons {
    Glyph,
    Tag,
    Both,
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Deepest spine level to draw; deeper nodes are folded into a count.
    pub max_depth: u16,
    /// Show source line numbers in the flow gutter.
    pub gutter: bool,
    pub icons: Icons,
    /// Draw the source alongside the analysis.
    pub code: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            max_depth: 6,
            gutter: true,
            icons: Icons::Both,
            code: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// The whole report on one row: band block, capability barcode, score,
    /// synopsis. Built for a stream of them, not for one.
    Line,
    /// The barcode's key, printed once at the top of a feed.
    Legend,
    Header,
    Flow,
    Timeline,
    Caps,
    Imports,
    Bindings,
    Symbols,
    Metrics,
    Minimap,
    Diagnostics,
    Code,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Panel::Line => "",
            Panel::Legend => "",
            Panel::Header => "",
            Panel::Flow => "flow",
            Panel::Timeline => "what it does",
            Panel::Caps => "capabilities",
            Panel::Imports => "imports",
            Panel::Bindings => "values",
            Panel::Symbols => "calls",
            Panel::Metrics => "shape",
            Panel::Minimap => "texture",
            Panel::Diagnostics => "syntax",
            Panel::Code => "source",
        }
    }

    /// Whether this panel is drawn inside a frame. The one-liners are not:
    /// they are single rows in somebody else's column, and a box around each
    /// would triple the height of a feed.
    pub fn framed(self) -> bool {
        !matches!(self, Panel::Line | Panel::Legend)
    }

    /// Rows of content this panel would like, excluding any frame.
    pub fn content_height(self, r: &Report, w: u16, o: &Opts) -> u16 {
        match self {
            Panel::Line => 1,
            Panel::Legend => 1,
            Panel::Header => 3,
            Panel::Flow => flatten(r, o, &crate::theme::UNICODE).len() as u16,
            Panel::Timeline => r.timeline(usize::MAX).len().max(1) as u16,
            Panel::Caps => {
                let n = r.effect_summary().len() as u16;
                let per = (w / 12).max(1);
                n.div_ceil(per).max(1)
            }
            Panel::Imports => r.imports.len().max(1) as u16,
            Panel::Bindings => r.bindings.len().max(1) as u16,
            Panel::Symbols => r.symbols.len().max(1) as u16,
            Panel::Metrics => 2,
            Panel::Minimap => 3,
            Panel::Diagnostics => r.diagnostics.len() as u16,
            Panel::Code => r.source.len() as u16,
        }
    }

    pub fn render(self, buf: &mut Buffer, area: Rect, r: &Report, t: &Theme, o: &Opts) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        match self {
            Panel::Line => line(buf, area, r, t, o),
            Panel::Legend => legend(buf, area, t),
            Panel::Header => header(buf, area, r, t),
            Panel::Flow => flow(buf, area, r, t, o),
            Panel::Timeline => timeline(buf, area, r, t, o),
            Panel::Caps => caps(buf, area, r, t, o),
            Panel::Imports => imports(buf, area, r, t),
            Panel::Bindings => bindings(buf, area, r, t),
            Panel::Symbols => symbols(buf, area, r, t),
            Panel::Metrics => metrics(buf, area, r, t),
            Panel::Minimap => minimap(buf, area, r, t),
            Panel::Diagnostics => diagnostics(buf, area, r, t),
            Panel::Code => code(buf, area, r, t),
        }
    }
}

// --------------------------------------------------------------- one-liner

/// Width of the capability barcode, including the gaps between groups.
pub fn barcode_width() -> u16 {
    (Effect::BARCODE.len() + Effect::BARCODE_GROUPS.len() - 1) as u16
}

/// The fixed thirteen-slot capability strip.
///
/// Every capability owns a permanent column, so the *pattern* of lit cells is
/// what a reader recognises — the same profile always makes the same shape,
/// and two snippets can be told apart without reading a word. Unlit slots stay
/// drawn, faintly: a barcode with holes in it only reads as a barcode if the
/// holes are visible.
pub fn barcode(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    mask: EffectMask,
    t: &Theme,
    lit_only: bool,
) -> u16 {
    let mut cx = x;
    let mut slot = 0usize;
    for (group, size) in Effect::BARCODE_GROUPS.iter().enumerate() {
        if group > 0 {
            cx += canvas::text(buf, cx, y, 1, " ", t.faint());
        }
        for _ in 0..*size {
            let effect = Effect::BARCODE[slot];
            slot += 1;
            let on = mask.contains(effect);
            let (ch, style) = if on {
                (
                    t.effect_icon(effect),
                    Style::default()
                        .fg(t.effect_color(effect))
                        .add_modifier(Modifier::BOLD),
                )
            } else if lit_only {
                (' ', t.faint())
            } else {
                (t.gl.dot, Style::default().fg(t.pal.rule))
            };
            cx += canvas::text(buf, cx, y, 1, &ch.to_string(), style);
        }
    }
    cx - x
}

/// Three cells in the band's colour, filled by the band's weight. Colour and
/// length say the same thing twice, so the strip survives a log file, a
/// screenshot, and a reader who does not see the hue.
pub fn band_block(buf: &mut Buffer, x: u16, y: u16, band: Band, t: &Theme) -> u16 {
    let colour = t.risk_color(band);
    let filled = band.weight();
    for i in 0..3usize {
        let (ch, style) = if i < filled {
            (t.gl.block, Style::default().fg(colour))
        } else {
            (' ', t.faint())
        };
        canvas::text(buf, x + i as u16, y, 1, &ch.to_string(), style);
    }
    3
}

fn line(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme, o: &Opts) {
    let band = r.band();
    let mut x = a.x;
    x += band_block(buf, x, a.y, band, t);
    x += canvas::text(
        buf,
        x,
        a.y,
        5,
        &format!("{:>4} ", r.metrics.risk),
        Style::default()
            .fg(t.risk_color(band))
            .add_modifier(if band >= Band::Check {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );
    x += barcode(buf, x, a.y, r.mask(), t, o.icons == Icons::Glyph);
    x += canvas::text(buf, x, a.y, 2, "  ", t.faint());

    // The name is pinned to the right so a column of these lines stays
    // scannable; the synopsis takes whatever is left.
    let name = fit(&r.meta.name, (a.width / 4).max(8), t.gl.ellipsis);
    let name_w = width(&name) + 2;
    let room = a.right().saturating_sub(x).saturating_sub(name_w);
    let synopsis = t.localise(&r.meta.synopsis);
    canvas::text(
        buf,
        x,
        a.y,
        room,
        &fit(&synopsis, room, t.gl.ellipsis),
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
    canvas::segs_right_bounded(buf, x + room, a.right(), a.y, &[(name.as_str(), t.faint())]);
}

fn legend(buf: &mut Buffer, a: Rect, t: &Theme) {
    let mut x = a.x;
    x += canvas::text(buf, x, a.y, 8, "risk", t.faint());
    x += canvas::text(buf, x, a.y, 4, "    ", t.faint());
    let mut slot = 0usize;
    for (group, size) in Effect::BARCODE_GROUPS.iter().enumerate() {
        if group > 0 {
            x += canvas::text(buf, x, a.y, 1, " ", t.faint());
        }
        for _ in 0..*size {
            let effect = Effect::BARCODE[slot];
            slot += 1;
            x += canvas::text(
                buf,
                x,
                a.y,
                1,
                &t.effect_icon(effect).to_string(),
                Style::default().fg(t.effect_color(effect)),
            );
        }
    }
    x += canvas::text(buf, x, a.y, 3, "   ", t.faint());
    let names = ["fs", "world", "eval", "work", "out"].join(t.gl.sep);
    canvas::text(buf, x, a.y, a.right().saturating_sub(x), &names, t.faint());
}

// ------------------------------------------------------------------- header

fn header(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    let m = &r.metrics;
    let brand = Style::default()
        .fg(t.pal.accent)
        .add_modifier(Modifier::BOLD);

    // The metadata degrades in three steps rather than overflowing: the full
    // sentence, an abbreviated form, then nothing at all. At 32 columns there
    // simply is no room, and eating the frame to say so helps no one.
    let sep = t.gl.sep;
    let full = format!(
        "{} line{}{sep}{} statement{}{sep}depth {}{sep}cyclomatic {}",
        m.lines_total,
        plural(m.lines_total),
        m.statements,
        plural(m.statements),
        m.max_depth,
        m.complexity
    );
    let short = format!(
        "{}L{sep}{}S{sep}d{}{sep}cx{}",
        m.lines_total, m.statements, m.max_depth, m.complexity
    );
    let brand_w = width("pyxray") + 2;
    let meta = if brand_w + 8 + width(&full) <= a.width {
        Some(full)
    } else if brand_w + 6 + width(&short) <= a.width {
        Some(short)
    } else {
        None
    };
    let meta_w = meta.as_ref().map(|s| width(s) + 2).unwrap_or(0);

    let mut x = a.x;
    x += canvas::text(buf, x, a.y, a.width, "pyxray", brand);
    x += canvas::text(buf, x, a.y, a.width, "  ", t.faint());
    let name_room = a.right().saturating_sub(x).saturating_sub(meta_w);
    canvas::text(
        buf,
        x,
        a.y,
        name_room,
        &fit(&r.meta.name, name_room, t.gl.ellipsis),
        t.strong(),
    );
    if let Some(meta) = &meta {
        canvas::segs_right_bounded(
            buf,
            x + name_room,
            a.right(),
            a.y,
            &[(meta.as_str(), t.faint())],
        );
    }

    // Synopsis — the line most readers will stop at. No leading marker: the
    // arrows inside it are already doing the separating, and a second kind of
    // arrow in front of them only muddies that.
    // The synopsis is built without knowing the theme, so swap in whichever
    // arrow this one uses before measuring it.
    let synopsis = t.localise(&r.meta.synopsis);
    let synopsis = fit(&synopsis, a.width, t.gl.ellipsis);
    canvas::text(
        buf,
        a.x,
        a.y + 1,
        a.width,
        &synopsis,
        Style::default().fg(t.pal.fg).add_modifier(Modifier::BOLD),
    );

    // Risk meter. Coloured and worded by the band, which asks severity
    // first — the same answer the band block beside it gives.
    let risk = m.risk;
    let band = r.band();
    let colour = t.risk_color(band);
    let word = t.risk_word(band);
    let label = "risk ";
    let mut x = a.x;
    x += canvas::text(buf, x, a.y + 2, a.width, label, t.faint());
    let bar_w = 18.min(a.width.saturating_sub(width(label) + 24));
    canvas::meter(
        buf,
        x,
        a.y + 2,
        bar_w,
        risk as f32 / 100.0,
        t,
        Style::default().fg(colour),
        t.faint(),
    );
    x += bar_w;
    let tail = format!("  {risk:>3}  {word}");
    x += canvas::text(
        buf,
        x,
        a.y + 2,
        a.width.saturating_sub(x - a.x),
        &tail,
        Style::default().fg(colour),
    );

    // The same barcode the one-line renders use, in the same reading order, so
    // a card that blooms out of a feed asks the eye to relearn nothing.
    let bar_w = barcode_width();
    if a.right().saturating_sub(x) > bar_w + 2 {
        barcode(buf, a.right() - bar_w, a.y + 2, r.mask(), t, false);
    }
}

// --------------------------------------------------------------------- flow

pub struct Row<'a> {
    pub node: &'a Node,
    pub prefix: String,
    /// Children hidden by the depth limit.
    pub folded: u32,
}

/// Depth-first flatten with box-drawing prefixes built from the ancestry.
pub fn flatten<'a>(r: &'a Report, o: &Opts, gl: &crate::theme::Glyphs) -> Vec<Row<'a>> {
    fn walk<'a>(
        node: &'a Node,
        stack: &mut Vec<bool>,
        out: &mut Vec<Row<'a>>,
        o: &Opts,
        gl: &crate::theme::Glyphs,
    ) {
        let mut prefix = String::new();
        if let Some((_, ancestors)) = stack.split_last() {
            for cont in ancestors {
                prefix.push_str(if *cont { gl.pipe } else { gl.gap });
            }
        }
        if let Some(last) = stack.last() {
            prefix.push_str(if *last { gl.branch } else { gl.branch_last });
        }

        let folded = if node.depth >= o.max_depth && !node.children.is_empty() {
            node.weight - 1
        } else {
            0
        };
        out.push(Row {
            node,
            prefix,
            folded,
        });
        if folded > 0 {
            return;
        }
        let n = node.children.len();
        for (i, child) in node.children.iter().enumerate() {
            stack.push(i + 1 < n);
            walk(child, stack, out, o, gl);
            stack.pop();
        }
    }

    let mut out = Vec::new();
    let mut stack = Vec::new();
    for (i, child) in r.spine.children.iter().enumerate() {
        stack.push(i + 1 < r.spine.children.len());
        walk(child, &mut stack, &mut out, o, gl);
        stack.pop();
    }
    out
}

fn effect_marks(t: &Theme, mask: EffectMask, o: &Opts) -> Vec<(String, Style)> {
    mask.iter()
        .map(|e| {
            let style = Style::default().fg(t.effect_color(e));
            let s = match o.icons {
                Icons::Glyph => format!("{}", t.effect_icon(e)),
                Icons::Tag | Icons::Both => effect_tag(e).to_string(),
            };
            (s, style)
        })
        .collect()
}

fn flow(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme, o: &Opts) {
    let rows = flatten(r, o, &t.gl);
    let gutter = if o.gutter { 5u16 } else { 0 };
    for (i, row) in rows.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let node = row.node;

        if o.gutter {
            let num = format!("{:>4} ", node.line);
            canvas::text(buf, a.x, y, gutter, &num, t.faint());
        }

        // A caution anywhere below this node earns a marker in the margin.
        let mut x = a.x + gutter;
        if let Some(sev) = node.severity {
            if sev == Severity::Caution {
                x += canvas::text(
                    buf,
                    x,
                    y,
                    2,
                    &format!("{} ", t.severity_mark(sev)),
                    Style::default()
                        .fg(t.pal.danger)
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                x += canvas::text(buf, x, y, 2, "  ", t.faint());
            }
        } else {
            x += canvas::text(buf, x, y, 2, "  ", t.faint());
        }

        x += canvas::text(buf, x, y, a.width, &row.prefix, t.rule_style());

        // Reserve the right-hand strip for the effect marks before laying the
        // label out, so a long label never collides with them.
        let marks = effect_marks(t, node.own_effects, o);
        let marks_w: u16 = marks.iter().map(|(s, _)| width(s) + 1).sum::<u16>();
        let avail = a.right().saturating_sub(x).saturating_sub(marks_w + 1);

        let label_style = Style::default().fg(t.node_color(node.kind));
        let label_style = if node.kind.is_block() {
            label_style.add_modifier(Modifier::BOLD)
        } else {
            label_style
        };
        let label = fit(&node.label, avail, t.gl.ellipsis);
        let mut lx = x + canvas::text(buf, x, y, avail, &label, label_style);

        if let Some(detail) = &node.detail {
            let room = (x + avail).saturating_sub(lx);
            if room > 6 {
                let d = format!("  {}", fit(detail, room.saturating_sub(2), t.gl.ellipsis));
                lx += canvas::text(buf, lx, y, room, &d, t.faint());
            }
        }
        if row.folded > 0 {
            let more = format!("  +{} hidden", row.folded);
            canvas::text(buf, lx, y, (x + avail).saturating_sub(lx), &more, t.faint());
        }

        if !marks.is_empty() {
            let parts: Vec<(&str, Style)> = marks
                .iter()
                .flat_map(|(s, st)| [(" ", *st), (s.as_str(), *st)])
                .collect();
            canvas::segs_right(buf, a.right(), y, &parts);
        }
    }

    if rows.len() > a.height as usize && a.height > 0 {
        let more = format!("{} +{} more", t.gl.ellipsis, rows.len() - a.height as usize);
        canvas::text(buf, a.x, a.bottom() - 1, a.width, &more, t.faint());
    }
}

// ----------------------------------------------------------------- timeline

fn timeline(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme, o: &Opts) {
    if r.all_effects().next().is_none() {
        canvas::text(buf, a.x, a.y, a.width, "no side effects", t.faint());
        return;
    }
    let rows = r.timeline(a.height as usize);
    let verb_w = rows
        .iter()
        .map(|(h, _)| width(&h.verb))
        .max()
        .unwrap_or(6)
        .clamp(4, 10);

    for (i, (hit, count)) in rows.iter().enumerate() {
        let y = a.y + i as u16;
        let colour = t.effect_color(hit.effect);
        let sev_style = Style::default().fg(t.severity_color(hit.severity));

        let mut x = a.x;
        x += canvas::text(buf, x, y, 6, &format!("{:>4} ", hit.line), t.faint());
        x += canvas::text(
            buf,
            x,
            y,
            2,
            &format!("{} ", t.severity_mark(hit.severity)),
            sev_style.add_modifier(if hit.severity == Severity::Caution {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        );
        let icon = match o.icons {
            Icons::Tag => String::new(),
            _ => format!("{} ", t.effect_icon(hit.effect)),
        };
        x += canvas::text(buf, x, y, 3, &icon, Style::default().fg(colour));
        let verb = format!("{:<w$} ", hit.verb, w = verb_w as usize);
        x += canvas::text(buf, x, y, verb_w + 1, &verb, Style::default().fg(colour));

        let note_w = hit
            .note
            .as_ref()
            .map(|n| (width(n) + 3).min(a.width / 2))
            .unwrap_or(0);
        let room = a.right().saturating_sub(x).saturating_sub(note_w);
        let mut target = hit.target.clone().unwrap_or_else(|| hit.symbol.clone());
        // A hit in an imported module says where: its line number above is
        // that file's, not the snippet's.
        if let Some(file) = &hit.file {
            target = format!("{target}  {} {file}", t.gl.arrow);
        }
        if *count > 1 {
            target.push_str(&format!("  x{count}"));
        }
        canvas::text(
            buf,
            x,
            y,
            room,
            &fit(&target, room, t.gl.ellipsis),
            t.base().fg(t.pal.fg),
        );

        if let Some(note) = &hit.note {
            let text = format!("{} {}", t.severity_mark(Severity::Caution), note);
            canvas::segs_right(
                buf,
                a.right(),
                y,
                &[(fit(&text, note_w, t.gl.ellipsis).as_str(), sev_style)],
            );
        }
    }
}

// --------------------------------------------------------------- capabilities

fn caps(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme, o: &Opts) {
    let summary = r.effect_summary();
    if summary.is_empty() {
        canvas::text(buf, a.x, a.y, a.width, "none", t.faint());
        return;
    }
    let mut x = a.x;
    let mut y = a.y;
    for (effect, sev, count) in summary {
        let colour = t.effect_color(effect);
        let mark = if sev == Severity::Caution {
            t.gl.bang.to_string()
        } else {
            String::new()
        };
        let label = match o.icons {
            Icons::Glyph => format!("{} {count}{mark}", t.effect_icon(effect)),
            Icons::Tag => format!("{} {count}{mark}", effect_tag(effect)),
            Icons::Both => format!(
                "{} {} {count}{mark}",
                t.effect_icon(effect),
                effect_tag(effect)
            ),
        };
        let w = width(&label) + 3;
        if x + w > a.right() {
            x = a.x;
            y += 1;
            if y >= a.bottom() {
                return;
            }
        }
        let style = if sev == Severity::Caution {
            Style::default()
                .fg(t.pal.bg)
                .bg(colour)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colour)
        };
        canvas::chip(buf, x, y, w, &label, style);
        x += w;
    }
}

// ------------------------------------------------------------------ imports

fn imports(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    if r.imports.is_empty() {
        canvas::text(buf, a.x, a.y, a.width, "none", t.faint());
        return;
    }
    let name_w = r
        .imports
        .iter()
        .map(|i| width(&i.module))
        .max()
        .unwrap_or(8)
        .clamp(6, a.width / 2);

    for (i, imp) in r.imports.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let marker = if imp.third_party { t.gl.bullet } else { ' ' };
        let mstyle = Style::default().fg(if imp.third_party {
            t.pal.accent_alt
        } else {
            t.pal.faint
        });
        let mut x = a.x;
        x += canvas::text(buf, x, y, 2, &format!("{marker} "), mstyle);
        let module = fit(&imp.module, name_w, t.gl.ellipsis);
        x += canvas::text(
            buf,
            x,
            y,
            name_w + 1,
            &format!("{module:<w$} ", w = name_w as usize),
            t.base().fg(t.pal.fg),
        );
        let mut tail = String::new();
        if let Some(alias) = &imp.alias {
            tail.push_str(&format!("as {alias}  "));
        }
        if !imp.names.is_empty() {
            tail.push_str(&imp.names.join(", "));
        }
        if imp.deferred {
            tail.push_str("  (deferred)");
        }
        canvas::text(
            buf,
            x,
            y,
            a.right().saturating_sub(x),
            &fit(&tail, a.right().saturating_sub(x), t.gl.ellipsis),
            t.dim(),
        );
    }
}

// ----------------------------------------------------------------- bindings

fn bindings(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    if r.bindings.is_empty() {
        canvas::text(buf, a.x, a.y, a.width, "none", t.faint());
        return;
    }
    let name_w = r
        .bindings
        .iter()
        .map(|b| width(&b.name))
        .max()
        .unwrap_or(6)
        .clamp(4, 16);

    for (i, b) in r.bindings.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let mut x = a.x;
        x += canvas::text(buf, x, y, 6, &format!("{:>4} ", b.line), t.faint());
        x += canvas::text(
            buf,
            x,
            y,
            name_w + 1,
            &format!(
                "{:<w$} ",
                fit(&b.name, name_w, t.gl.ellipsis),
                w = name_w as usize
            ),
            t.base().fg(t.pal.fg).add_modifier(Modifier::BOLD),
        );
        let kind = b.value.id();
        x += canvas::text(
            buf,
            x,
            y,
            9,
            &format!("{kind:<8} "),
            Style::default().fg(t.value_color(b.value)),
        );
        let flags = {
            let mut f = String::new();
            if b.accumulates {
                f.push_str(" accum");
            }
            if b.rebinds > 0 {
                f.push_str(&format!(" x{}", b.rebinds + 1));
            }
            if b.dead {
                f.push_str(" unused");
            }
            f
        };
        let flag_w = width(&flags) + 1;
        let room = a.right().saturating_sub(x).saturating_sub(flag_w);
        canvas::text(
            buf,
            x,
            y,
            room,
            &fit(&b.origin, room, t.gl.ellipsis),
            t.dim(),
        );
        if !flags.is_empty() {
            let style = if b.dead {
                t.faint()
            } else {
                Style::default().fg(t.pal.accent_alt)
            };
            canvas::segs_right(buf, a.right(), y, &[(flags.as_str(), style)]);
        }
    }
}

// ------------------------------------------------------------------ symbols

fn symbols(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    if r.symbols.is_empty() {
        canvas::text(buf, a.x, a.y, a.width, "none", t.faint());
        return;
    }
    for (i, s) in r.symbols.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let colour = s
            .effects
            .iter()
            .next()
            .map(|e| t.effect_color(e))
            .unwrap_or(t.pal.fg);
        let count = format!("x{}", s.count);
        let mut x = a.x;
        let room = a
            .right()
            .saturating_sub(a.x)
            .saturating_sub(width(&count) + 2);
        x += canvas::text(
            buf,
            x,
            y,
            room,
            &fit(&s.canonical, room, t.gl.ellipsis),
            Style::default().fg(colour),
        );
        let _ = x;
        if s.count > 1 {
            canvas::segs_right(buf, a.right(), y, &[(count.as_str(), t.faint())]);
        }
    }
}

// ------------------------------------------------------------------ metrics

fn metrics(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    let m = &r.metrics;
    let tiles: [(String, &str); 6] = [
        (m.lines_total.to_string(), "lines"),
        (m.statements.to_string(), "stmts"),
        (m.max_depth.to_string(), "depth"),
        (m.complexity.to_string(), "cyclo"),
        (m.functions.to_string(), "defs"),
        (m.calls.to_string(), "calls"),
    ];
    let cell = (a.width / tiles.len() as u16).max(6);
    for (i, (value, label)) in tiles.iter().enumerate() {
        let x = a.x + i as u16 * cell;
        if x >= a.right() {
            break;
        }
        canvas::text(
            buf,
            x,
            a.y,
            cell,
            value,
            Style::default().fg(t.pal.fg).add_modifier(Modifier::BOLD),
        );
        if a.height > 1 {
            canvas::text(buf, x, a.y + 1, cell, label, t.faint());
        }
    }
}

// ------------------------------------------------------------------ minimap

fn minimap(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    let cells = &r.texture;
    if cells.is_empty() || a.width == 0 {
        return;
    }
    let cols = a.width as usize;
    let n = cells.len();
    let max_indent = cells.iter().map(|c| c.indent).max().unwrap_or(1).max(1) as f32;

    // Proportional mapping in both directions: the strip always spans the full
    // width, whether the file is longer than the panel or much shorter.
    for col in 0..cols {
        let lo = col * n / cols;
        let hi = (((col + 1) * n).div_ceil(cols)).clamp(lo + 1, n);
        if lo >= n {
            break;
        }
        let slice = &cells[lo..hi];
        if slice.is_empty() {
            continue;
        }
        let indent = slice.iter().map(|c| c.indent as f32).fold(0.0, f32::max);
        let live = slice.iter().filter(|c| !c.blank).count();
        let effect = slice.iter().flat_map(|c| c.effects.iter()).next();
        let sev = slice.iter().filter_map(|c| c.severity).max();

        let x = a.x + col as u16;
        // Row 0: indentation profile — the silhouette of the code.
        let frac = if live == 0 {
            0.0
        } else {
            0.25 + 0.75 * (indent / max_indent)
        };
        let colour = match effect {
            Some(e) => t.effect_color(e),
            None if live == 0 => t.pal.rule,
            None => t.pal.dim,
        };
        canvas::text(
            buf,
            x,
            a.y,
            1,
            &t.ramp(frac).to_string(),
            Style::default().fg(colour),
        );

        // Row 1: an effect tick, so the heatmap survives at one row too.
        if a.height > 1 {
            let (ch, style) = match (effect, sev) {
                (Some(_), Some(Severity::Caution)) => (
                    t.gl.severity[2],
                    Style::default()
                        .fg(t.pal.danger)
                        .add_modifier(Modifier::BOLD),
                ),
                (Some(e), _) => (t.effect_icon(e), Style::default().fg(t.effect_color(e))),
                (None, _) => (' ', t.faint()),
            };
            canvas::text(buf, x, a.y + 1, 1, &ch.to_string(), style);
        }
    }

    if a.height > 2 {
        let scale = format!(
            "L1{}L{}",
            " ".repeat((a.width as usize).saturating_sub(6)),
            r.metrics.lines_total
        );
        canvas::text(buf, a.x, a.y + 2, a.width, &scale, t.faint());
    }
}

// -------------------------------------------------------------- diagnostics

fn diagnostics(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    for (i, d) in r.diagnostics.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let style = Style::default().fg(match d.level {
            DiagLevel::Error => t.pal.danger,
            DiagLevel::Warning => t.pal.warn,
            DiagLevel::Hint => t.pal.dim,
        });
        let s = format!("L{}:{}  {}", d.line, d.col, d.message);
        canvas::text(
            buf,
            a.x,
            y,
            a.width,
            &fit(&s, a.width, t.gl.ellipsis),
            style,
        );
    }
}

// --------------------------------------------------------------------- code

fn code(buf: &mut Buffer, a: Rect, r: &Report, t: &Theme) {
    for (i, line) in r.source.iter().take(a.height as usize).enumerate() {
        let y = a.y + i as u16;
        let n = i as u32 + 1;
        let cell = r.texture.get(i);
        let mut x = a.x;
        x += canvas::text(buf, x, y, 6, &format!("{n:>4} ",), t.faint());
        let mark = cell
            .and_then(|c| c.effects.iter().next())
            .map(|e| (t.effect_icon(e), Style::default().fg(t.effect_color(e))))
            .unwrap_or((' ', t.faint()));
        x += canvas::text(buf, x, y, 2, &format!("{} ", mark.0), mark.1);
        let style = if cell.map(|c| c.comment).unwrap_or(false) {
            t.faint()
        } else {
            t.base().fg(t.pal.fg)
        };
        let room = a.right().saturating_sub(x);
        canvas::text(buf, x, y, room, &fit(line, room, t.gl.ellipsis), style);
    }
}
