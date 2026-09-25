//! Turning raw findings into the things a person actually reads first: the
//! one-line synopsis, the risk number, and the per-line minimap.

use std::collections::BTreeSet;

use crate::model::*;
use crate::source::{clip, Src};

/// Order effects the way a reader narrates a program: where the data comes
/// from, what happens to it, where it ends up.
pub fn stage(effect: Effect) -> u8 {
    match effect {
        Effect::FsRead | Effect::Net | Effect::Env => 0,
        Effect::Compute | Effect::Random | Effect::Concurrency | Effect::Clock => 1,
        Effect::FsWrite | Effect::FsDelete | Effect::Process | Effect::Dynamic => 2,
        Effect::Stdout | Effect::Exit => 3,
    }
}

/// How many distinct things an effect touched, and the one worth naming — the
/// most frequently mentioned, so `numpy` does not out-rank `pandas` merely by
/// sorting earlier.
fn count_targets(hits: &[EffectHit], effect: Effect) -> (usize, Option<String>) {
    let mut tally: Vec<(&str, u32)> = Vec::new();
    let mut n = 0;
    for h in hits.iter().filter(|h| h.effect == effect) {
        n += 1;
        if let Some(t) = &h.target {
            match tally.iter_mut().find(|(name, _)| *name == t.as_str()) {
                Some(slot) => slot.1 += 1,
                None => tally.push((t.as_str(), 1)),
            }
        }
    }
    // On a tie, the one that appeared first wins: `max_by_key` would hand back
    // the last, which is the wrong end of the pipeline to name.
    let best = tally
        .iter()
        .fold(None::<(&str, u32)>, |best, (name, count)| match best {
            Some((_, b)) if *count <= b => best,
            _ => Some((name, *count)),
        })
        .map(|(name, _)| clip(name, 28));
    (if tally.is_empty() { n } else { tally.len() }, best)
}

fn phrase(effect: Effect, hits: &[EffectHit]) -> Option<String> {
    let (n, first) = count_targets(hits, effect);
    if n == 0 {
        return None;
    }
    let thing = |singular: &str, plural: &str| {
        if n == 1 {
            singular.to_string()
        } else {
            format!("{n} {plural}")
        }
    };
    Some(match effect {
        Effect::FsRead => match &first {
            Some(t) if n == 1 => format!("reads {t}"),
            _ => format!("reads {}", thing("a file", "files")),
        },
        Effect::FsWrite => match &first {
            Some(t) if n == 1 => format!("writes {t}"),
            _ => format!("writes {}", thing("a file", "files")),
        },
        Effect::FsDelete => match &first {
            Some(t) if n == 1 => format!("DELETES {t}"),
            _ => format!("DELETES {}", thing("a path", "paths")),
        },
        Effect::Net => match &first {
            Some(t) if n == 1 => format!("calls {t}"),
            _ => format!("makes {}", thing("a network call", "network calls")),
        },
        Effect::Process => match &first {
            Some(t) if n == 1 => format!("runs `{t}`"),
            _ => format!("runs {}", thing("a command", "commands")),
        },
        Effect::Env => "reads the environment".into(),
        Effect::Dynamic => format!(
            "evaluates {}",
            thing("code at runtime", "runtime code paths")
        ),
        Effect::Stdout => "prints results".into(),
        Effect::Random => "uses randomness".into(),
        Effect::Clock => "reads the clock".into(),
        Effect::Concurrency => "runs work in parallel".into(),
        Effect::Compute => match &first {
            Some(t) => format!("computes with {t}"),
            None => "computes".into(),
        },
        Effect::Exit => "exits the process".into(),
    })
}

/// A one-line answer to "what does this do?", assembled from the effects in
/// pipeline order. Falls back to structure when there are no effects at all.
pub fn synopsis(
    report_effects: &[EffectHit],
    metrics: &Metrics,
    spine: &Node,
) -> (String, Vec<String>) {
    let mut kinds: Vec<Effect> = Vec::new();
    for hit in report_effects {
        if !kinds.contains(&hit.effect) {
            kinds.push(hit.effect);
        }
    }
    // Pipeline order by default, so the sentence reads the way the program
    // runs. Only a Caution jumps the queue — if something here deletes a
    // directory, that is the first thing anyone needs to know.
    kinds.sort_by_key(|e| {
        let worst = report_effects
            .iter()
            .filter(|h| h.effect == *e)
            .map(|h| h.severity)
            .max()
            .unwrap_or(Severity::Info);
        let alarming = u8::from(worst != Severity::Caution);
        (alarming, stage(*e))
    });

    // Background noise — reading the clock, seeding an RNG, printing, poking
    // at the environment — earns a place in the sentence only when there is
    // nothing more substantial to say.
    let quiet = |e: Effect| {
        matches!(
            e,
            Effect::Clock | Effect::Random | Effect::Stdout | Effect::Env
        )
    };
    let worst = |e: Effect| {
        report_effects
            .iter()
            .filter(|h| h.effect == e)
            .map(|h| h.severity)
            .max()
            .unwrap_or(Severity::Info)
    };
    let mut parts: Vec<String> = Vec::new();
    let mut asides: Vec<String> = Vec::new();
    for effect in &kinds {
        let is_quiet = quiet(*effect) && worst(*effect) != Severity::Caution;
        let Some(text) = phrase(*effect, report_effects) else {
            continue;
        };
        if is_quiet {
            asides.push(text);
        } else {
            parts.push(text);
        }
    }
    // Asides go on the end, and only while the sentence is still short: they
    // are true but they are not why anyone is reading this line.
    while parts.len() < 3 && !asides.is_empty() {
        parts.push(asides.remove(0));
    }

    if parts.is_empty() {
        let mut bits = Vec::new();
        if metrics.functions > 0 {
            bits.push(format!(
                "defines {} function{}",
                metrics.functions,
                if metrics.functions == 1 { "" } else { "s" }
            ));
        }
        if metrics.classes > 0 {
            bits.push(format!(
                "{} class{}",
                metrics.classes,
                if metrics.classes == 1 { "" } else { "es" }
            ));
        }
        if bits.is_empty() {
            bits.push(format!(
                "{} statement{}",
                spine.weight.saturating_sub(1),
                if spine.weight == 2 { "" } else { "s" }
            ));
        }
        let text = format!("no side effects — {}", bits.join(", "));
        return (text, bits);
    }

    let shown: Vec<String> = parts.iter().take(4).cloned().collect();
    let mut text = shown.join(" → ");
    if parts.len() > shown.len() {
        text.push_str(&format!(" (+{} more)", parts.len() - shown.len()));
    }
    (text, parts)
}

/// The most that Info-level hits can add, together. Below the "routine"
/// floor (15), so no amount of printing can climb out of "inert" on its own.
const INFO_CAP: u32 = 12;

/// 0..100. Driven by severity, not volume: one `shutil.rmtree` should read
/// louder than fifty `print`s.
///
/// The weights are pinned to the band boundaries rather than chosen for their
/// own sake, because the band is what the compact renders show. One Caution
/// lands exactly on "check it" (30) and one Notable exactly on "routine" (10),
/// so a lone `rmtree` can never come out green and a lone `requests.get` can
/// never come out inert. Anything else and the score and the colour would be
/// telling a reader two different stories.
pub fn risk(hits: &[EffectHit]) -> u8 {
    risk_breakdown(hits)
        .iter()
        .map(|(_, points)| points)
        .sum::<u32>()
        .min(100) as u8
}

/// The terms [`risk`] adds up, each with a reason a person can read. This is
/// the score, not a description of it: `risk` is literally their sum, capped
/// at 100, so the two can never disagree.
pub fn risk_breakdown(hits: &[EffectHit]) -> Vec<(String, u32)> {
    let mut terms: Vec<(String, u32)> = Vec::new();
    let mut infos = 0u32;
    let mut caution_kinds: BTreeSet<Effect> = BTreeSet::new();
    for hit in hits {
        let what = match &hit.target {
            Some(t) => format!("{} {t} (line {})", hit.verb, hit.line),
            None => format!("{} (line {})", hit.verb, hit.line),
        };
        match hit.severity {
            Severity::Caution => {
                terms.push((format!("caution: {what}"), 30));
                caution_kinds.insert(hit.effect);
            }
            Severity::Notable => terms.push((format!("notable: {what}"), 10)),
            Severity::Info => infos += 1,
        }
    }
    // Volume of ordinary work is capped: a hundred prints is still a script
    // that prints, and must never *look* like one that deletes.
    if infos > 0 {
        terms.push((format!("{infos} ordinary effect(s)"), infos.min(INFO_CAP)));
    }
    // Distinct dangerous capabilities compound: fetching *and* executing is
    // worse than doing either twice.
    if caution_kinds.len() > 1 {
        terms.push((
            format!("{} different kinds of caution", caution_kinds.len()),
            (caution_kinds.len() as u32 - 1) * 12,
        ));
    }
    let combo = |a: Effect, b: Effect| {
        hits.iter().any(|h| h.effect == a) && hits.iter().any(|h| h.effect == b)
    };
    if combo(Effect::Net, Effect::Process) || combo(Effect::Net, Effect::Dynamic) {
        terms.push(("network and execution together".into(), 15));
    }
    if combo(Effect::Net, Effect::FsWrite) {
        terms.push(("network and writes together".into(), 8));
    }
    // A secret actually reaching the network is worth more than a secret and
    // a network call merely being in the same snippet — and a key file leaving
    // the machine more than a token going to what is probably its own API.
    let flows: Vec<&Flow> = hits
        .iter()
        .filter_map(|h| h.flow.as_ref())
        .filter(|f| f.kind == FlowKind::Exfiltration)
        .collect();
    if let Some(f) = flows.iter().find(|f| f.secret != SecretKind::Env) {
        terms.push((format!("{} is sent over the network", f.source), 25));
    } else if let Some(f) = flows.first() {
        terms.push((format!("{} is sent over the network", f.source), 10));
    } else if combo(Effect::Env, Effect::Net) {
        terms.push(("environment and network together".into(), 10));
    }
    terms
}

/// Per-line strip data: indentation profile plus whatever the analysis found
/// on that line, so the minimap can double as an effect heatmap.
pub fn texture(src: &Src, spine: &Node, hits: &[EffectHit]) -> Vec<LineCell> {
    let n = src.line_count().max(1) as usize;
    let mut cells: Vec<LineCell> = Vec::with_capacity(n);
    for i in 0..n {
        let text = src.line_text(i as u32 + 1);
        let trimmed = text.trim_start();
        let indent = text.len().saturating_sub(trimmed.len());
        // Tabs count as four columns, matching how the code will be read.
        let indent = text
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum::<usize>()
            .max(if trimmed.is_empty() {
                0
            } else {
                indent.min(255)
            });
        cells.push(LineCell {
            indent: indent.min(255) as u8,
            width: trimmed.trim_end().chars().count().min(u16::MAX as usize) as u16,
            blank: trimmed.is_empty(),
            comment: trimmed.starts_with('#'),
            kind: None,
            effects: EffectMask::default(),
            severity: None,
        });
    }

    spine.walk(&mut |node| {
        if node.kind == NodeKind::Module {
            return;
        }
        if let Some(cell) = cells.get_mut(node.line.saturating_sub(1) as usize) {
            if cell.kind.is_none() {
                cell.kind = Some(node.kind);
            }
        }
    });

    for hit in hits {
        if let Some(cell) = cells.get_mut(hit.line.saturating_sub(1) as usize) {
            cell.effects.insert(hit.effect);
            cell.severity = Some(cell.severity.map_or(hit.severity, |s| s.max(hit.severity)));
        }
    }
    cells
}

pub fn line_counts(src: &Src) -> (u32, u32, u32, u32) {
    let mut blank = 0;
    let mut comment = 0;
    let mut code = 0;
    let total = src.line_count();
    for i in 1..=total {
        let t = src.line_text(i).trim();
        if t.is_empty() {
            blank += 1;
        } else if t.starts_with('#') {
            comment += 1;
        } else {
            code += 1;
        }
    }
    (total, code, comment, blank)
}

/// Drop effect hits that name the same operation at the same place twice —
/// an attribute and the call that wraps it can both fire.
pub fn dedup(hits: &mut Vec<EffectHit>) {
    let mut seen: BTreeSet<(u32, u32, String)> = BTreeSet::new();
    hits.retain(|h| seen.insert((h.line, h.col, h.symbol.clone())));
}
