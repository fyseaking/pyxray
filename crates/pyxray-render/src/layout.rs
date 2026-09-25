//! Layouts. A layout is planned before it is drawn — `plan` returns the exact
//! rectangle every panel will occupy — so measuring for a static export and
//! fitting into a live terminal are the same code path and can never drift.

use pyxray_core::model::Report;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::canvas::{self, frame_overhead};
use crate::panels::{Opts, Panel};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One row. For a feed of them, where the reader is watching a rate, not
    /// reading a report.
    Line,
    /// One row when nothing interesting happened, the full card when something
    /// did — so visual weight tracks how much attention is owed.
    Auto,
    /// A glance: what it does, how risky, and the effect timeline. Sized for a
    /// terminal notification, not a session.
    Card,
    /// Two columns: the structure on the left, everything else on the right.
    Dashboard,
    /// One column, everything, in reading order. Narrow-terminal safe.
    Stack,
    /// The structural outline given almost all the room.
    Flow,
}

impl Layout {
    pub fn id(self) -> &'static str {
        match self {
            Layout::Line => "line",
            Layout::Auto => "auto",
            Layout::Card => "card",
            Layout::Dashboard => "dashboard",
            Layout::Stack => "stack",
            Layout::Flow => "flow",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Layout::Line => {
                "One row: band, capability barcode, score, synopsis. Built for a stream."
            }
            Layout::Auto => "A row when it is dull, the card when it is not.",
            Layout::Card => "A glance: synopsis, risk, capabilities, effect timeline.",
            Layout::Dashboard => "Two columns: structure on the left, findings on the right.",
            Layout::Stack => "One column, everything, in reading order.",
            Layout::Flow => "The outline, given the room to breathe.",
        }
    }

    pub fn all() -> [Layout; 6] {
        [
            Layout::Line,
            Layout::Auto,
            Layout::Card,
            Layout::Dashboard,
            Layout::Stack,
            Layout::Flow,
        ]
    }

    /// The layouts worth putting side by side in a contact sheet: `auto` is a
    /// rule for choosing between two of the others, not a look of its own.
    pub fn gallery() -> [Layout; 5] {
        [
            Layout::Line,
            Layout::Card,
            Layout::Dashboard,
            Layout::Stack,
            Layout::Flow,
        ]
    }
}

pub fn layout(id: &str) -> Option<Layout> {
    Layout::all().into_iter().find(|l| l.id() == id)
}

#[derive(Clone, Debug)]
pub struct Plan {
    /// Panel, its outer rectangle (including frame), and its title.
    pub items: Vec<(Panel, Rect)>,
    pub width: u16,
    pub height: u16,
}

/// Vertical panel stacker for one column.
struct Column<'a> {
    x: u16,
    width: u16,
    y: u16,
    /// Row after which nothing more fits, when bounded.
    limit: Option<u16>,
    theme: &'a Theme,
    items: Vec<(Panel, Rect)>,
}

impl<'a> Column<'a> {
    fn new(x: u16, y: u16, width: u16, limit: Option<u16>, theme: &'a Theme) -> Self {
        Column {
            x,
            width,
            y,
            limit,
            theme,
            items: Vec::new(),
        }
    }

    fn room(&self) -> u16 {
        match self.limit {
            Some(bottom) => bottom.saturating_sub(self.y),
            None => u16::MAX,
        }
    }

    /// Place a panel. `cap` bounds the content rows regardless of how much the
    /// panel would like; `min` refuses to place it at all below that.
    fn push(&mut self, panel: Panel, r: &Report, o: &Opts, cap: Option<u16>, min: u16) {
        let (chrome_rows, chrome_cols) = if panel.framed() {
            frame_overhead(self.theme, !panel.title().is_empty())
        } else {
            (0, 0)
        };
        let inner_w = self.width.saturating_sub(chrome_cols);
        if inner_w < 4 {
            return;
        }
        let mut content = panel.content_height(r, inner_w, o);
        if let Some(cap) = cap {
            content = content.min(cap);
        }
        if content == 0 {
            return;
        }
        let want = content + chrome_rows;
        let room = self.room();
        if room < min + chrome_rows {
            return;
        }
        let height = want.min(room);
        self.items.push((
            panel,
            Rect {
                x: self.x,
                y: self.y,
                width: self.width,
                height,
            },
        ));
        self.y += height + self.theme.breathe;
    }

    fn end(&self) -> u16 {
        self.y.saturating_sub(self.theme.breathe)
    }
}

/// Work out where every panel goes. `height` bounds the result when the target
/// is a live terminal; leave it `None` to let the plan grow to fit.
pub fn plan(
    layout: Layout,
    r: &Report,
    t: &Theme,
    o: &Opts,
    width: u16,
    height: Option<u16>,
) -> Plan {
    let width = width.max(24);

    // `auto` is a decision, not an arrangement: below the "check it" band a
    // snippet gets one row, and at or above it the full card. That is the
    // whole answer to a harness firing several of these a second — the dull
    // ones stay out of the way and the alarming one blooms.
    if layout == Layout::Auto {
        let resolved = if r.band() >= pyxray_core::model::Band::Check {
            Layout::Card
        } else {
            Layout::Line
        };
        return plan(resolved, r, t, o, width, height);
    }

    // A single row has no frame, no padding and no header: it is one line in
    // somebody else's column and must not decorate itself.
    if layout == Layout::Line {
        return Plan {
            items: vec![(
                Panel::Line,
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: 1,
                },
            )],
            width,
            height: height.unwrap_or(1).max(1),
        };
    }

    let pad = if t.frame == crate::theme::Frame::None || t.frame == crate::theme::Frame::Underline {
        1
    } else {
        0
    };
    let inner_w = width.saturating_sub(pad * 2);
    let bottom = height.map(|h| h.saturating_sub(pad));
    let x = pad;
    let top = pad;

    let mut items: Vec<(Panel, Rect)> = Vec::new();
    let mut y = top;

    // The header and the minimap always span the full width: they are the
    // parts a reader takes in without moving their eyes.
    {
        let mut head = Column::new(x, y, inner_w, bottom, t);
        head.push(Panel::Header, r, o, None, 3);
        if !r.diagnostics.is_empty() {
            head.push(Panel::Diagnostics, r, o, Some(4), 1);
        }
        if layout != Layout::Card {
            head.push(Panel::Minimap, r, o, None, 2);
        }
        y = head.end() + t.breathe;
        items.extend(head.items);
    }

    match layout {
        Layout::Line | Layout::Auto => unreachable!("handled above"),
        Layout::Card => {
            let mut col = Column::new(x, y, inner_w, bottom, t);
            col.push(Panel::Caps, r, o, Some(3), 1);
            col.push(Panel::Minimap, r, o, Some(2), 2);
            col.push(Panel::Timeline, r, o, Some(14), 1);
            y = col.end();
            items.extend(col.items);
        }
        Layout::Flow => {
            let mut col = Column::new(x, y, inner_w, bottom, t);
            col.push(Panel::Caps, r, o, Some(2), 1);
            let reserve = if r.all_effects().next().is_none() {
                0
            } else {
                10
            };
            let flow_cap = bottom.map(|b| b.saturating_sub(col.y + reserve));
            col.push(Panel::Flow, r, o, flow_cap, 3);
            col.push(Panel::Timeline, r, o, Some(10), 1);
            y = col.end();
            items.extend(col.items);
        }
        Layout::Stack => {
            let mut col = Column::new(x, y, inner_w, bottom, t);
            col.push(Panel::Caps, r, o, Some(3), 1);
            col.push(Panel::Timeline, r, o, Some(16), 1);
            col.push(Panel::Flow, r, o, Some(40), 3);
            col.push(Panel::Imports, r, o, Some(10), 1);
            col.push(Panel::Bindings, r, o, Some(14), 1);
            col.push(Panel::Symbols, r, o, Some(12), 1);
            col.push(Panel::Metrics, r, o, None, 2);
            if o.code {
                col.push(Panel::Code, r, o, Some(60), 3);
            }
            y = col.end();
            items.extend(col.items);
        }
        Layout::Dashboard => {
            // Below about 96 columns two columns stop earning their keep.
            if inner_w < 96 {
                return plan(Layout::Stack, r, t, o, width, height);
            }
            let gap = 2;
            let left_w = (inner_w as f32 * 0.56) as u16;
            let right_w = inner_w - left_w - gap;

            let mut left = Column::new(x, y, left_w, bottom, t);
            left.push(Panel::Flow, r, o, Some(60), 3);
            if o.code {
                left.push(Panel::Code, r, o, Some(40), 3);
            }

            let mut right = Column::new(x + left_w + gap, y, right_w, bottom, t);
            right.push(Panel::Caps, r, o, Some(4), 1);
            right.push(Panel::Timeline, r, o, Some(18), 1);
            right.push(Panel::Imports, r, o, Some(9), 1);
            right.push(Panel::Bindings, r, o, Some(12), 1);
            right.push(Panel::Symbols, r, o, Some(10), 1);
            right.push(Panel::Metrics, r, o, None, 2);

            y = left.end().max(right.end());
            items.extend(left.items);
            items.extend(right.items);
        }
    }

    let total = match height {
        Some(h) => h,
        None => y + pad,
    };
    Plan {
        items,
        width,
        height: total.max(1),
    }
}

/// Draw a plan. The buffer must already be the plan's size.
pub fn draw(buf: &mut Buffer, plan: &Plan, r: &Report, t: &Theme, o: &Opts) {
    canvas::fill(buf, buf.area, t.base());
    for (panel, area) in &plan.items {
        if area.height == 0 {
            continue;
        }
        let inner = if panel.framed() {
            canvas::frame(buf, *area, t, panel.title())
        } else {
            *area
        };
        panel.render(buf, inner, r, t, o);
    }
}

/// Plan and draw in one step, returning the buffer.
pub fn render(
    r: &Report,
    t: &Theme,
    o: &Opts,
    l: Layout,
    width: u16,
    height: Option<u16>,
) -> Buffer {
    let plan = plan(l, r, t, o, width, height);
    let area = Rect {
        x: 0,
        y: 0,
        width: plan.width,
        height: plan.height,
    };
    let mut buf = Buffer::empty(area);
    draw(&mut buf, &plan, r, t, o);
    if t.gl.ellipsis.is_ascii() {
        asciify(&mut buf);
    }
    buf
}

/// For a theme whose glyph set is ASCII, the analyser's own punctuation —
/// the `…` a clipped label ends in, the `→` in a return annotation, the
/// `—` between clauses — still arrives from `pyxray-core`, which has no idea
/// what is being drawn. Swap the handful of single-cell characters it uses
/// for their ASCII stand-ins. The user's own text is left alone: a CJK
/// identifier is theirs to display, and replacing a wide cell with a narrow
/// one would break the grid anyway.
///
/// Public so that anything composing its own screen from the same parts —
/// `pyx watch`, the viewer's status bar — can hold the same promise.
pub fn asciify(buf: &mut Buffer) {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            let sub = match cell.symbol() {
                "\u{2026}" => "~",
                "\u{2192}" => ">",
                "\u{2190}" => "<",
                "\u{2014}" | "\u{2013}" => "-",
                "\u{00b7}" | "\u{2022}" => ".",
                "\u{2588}" => "#",
                "\u{2591}" => ".",
                _ => continue,
            };
            cell.set_symbol(sub);
        }
    }
}
