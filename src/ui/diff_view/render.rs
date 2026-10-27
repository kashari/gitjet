//! Turning diff rows into styled terminal cells: unified and side-by-side painting, word-level
//! and syntax colors, and the per-diff caches that keep this cheap enough to run every frame.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use super::highlight::{FileHighlighter, Side, StyledRange};
use super::model::{self, Row};
use crate::ctx::Ctx;
use crate::git::diff::{DiffLine, FileDiff, LineKind};

/// Lines of syntax highlighting computed per frame and diff (the rest follows on later frames).
pub const HIGHLIGHT_BUDGET: usize = 800;
/// Display width of a tab character.
pub const TAB_WIDTH: usize = 4;

/// Rendering toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub word_diff: bool,
    pub syntax: bool,
}

impl Flags {
    pub fn from_config(c: &crate::config::DiffConfig) -> Self {
        Self {
            word_diff: c.word_diff,
            syntax: c.syntax_highlight,
        }
    }
}

/// What part of the rows to show.
#[derive(Debug, Clone, Copy, Default)]
pub struct Viewport {
    /// First visible row
    pub scroll: usize,
    /// Columns scrolled horizontally (content only; gutters stay put)
    pub hscroll: u16,
    /// Row with the caret, if any
    pub cursor: Option<usize>,
}

/// (file, hunk, line) of a diff line.
type LineId = (usize, usize, usize);

/// Lazily computed per-diff data: change pairs, word ranges, syntax colors, gutter widths.
/// Valid only for the exact `diffs` it was filled from; call [`clear`](Self::clear) when they change.
#[derive(Default)]
pub struct DiffCache {
    pairs: HashMap<(usize, usize), Vec<Option<usize>>>,
    words: HashMap<LineId, Option<Vec<Range<usize>>>>,
    highlighters: HashMap<usize, FileHighlighter>,
    digits: HashMap<usize, usize>,
    budget: usize,
}

impl DiffCache {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Reset the per-frame highlighting budget.
    pub fn begin_frame(&mut self) {
        self.budget = HIGHLIGHT_BUDGET;
    }

    /// Changed byte ranges of line `l` relative to its paired counterpart, if it has one.
    pub fn words(
        &mut self,
        diffs: &[FileDiff],
        f: usize,
        h: usize,
        l: usize,
    ) -> Option<&[Range<usize>]> {
        let hunk = diffs.get(f)?.hunks.get(h)?;
        if !self.words.contains_key(&(f, h, l)) {
            let pairs = self
                .pairs
                .entry((f, h))
                .or_insert_with(|| model::pair_changes(&hunk.lines));
            match pairs.get(l).copied().flatten() {
                None => {
                    self.words.insert((f, h, l), None);
                }
                Some(p) => {
                    let (del, add) = if hunk.lines[l].kind == LineKind::Removed {
                        (l, p)
                    } else {
                        (p, l)
                    };
                    let (ro, rn) = match model::word_ranges(
                        &hunk.lines[del].content,
                        &hunk.lines[add].content,
                    ) {
                        Some((a, b)) => (Some(a), Some(b)),
                        None => (None, None),
                    };
                    self.words.insert((f, h, del), ro);
                    self.words.insert((f, h, add), rn);
                }
            }
        }
        self.words.get(&(f, h, l))?.as_deref()
    }

    fn syntax(
        &mut self,
        diffs: &[FileDiff],
        (f, h, l): LineId,
        side: Side,
        theme: &str,
    ) -> Option<&[StyledRange]> {
        let d = diffs.get(f)?;
        let hunk = d.hunks.get(h)?;
        let hl = self
            .highlighters
            .entry(f)
            .or_insert_with(|| FileHighlighter::new(d.path(), theme));
        hl.line(hunk, h, l, side, &mut self.budget)
    }

    /// Digits needed for the largest line number of file `f` (at least 3).
    fn digits(&mut self, diffs: &[FileDiff], f: usize) -> usize {
        *self.digits.entry(f).or_insert_with(|| {
            let max = diffs
                .get(f)
                .map(|d| {
                    d.hunks
                        .iter()
                        .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            max.to_string().len().max(3)
        })
    }
}

/// Spans for `text` with `base` style, syntax colors and highlighted word ranges composed:
/// syntax decides the foreground, word ranges get `word_bg`. Tabs are expanded and control
/// characters made visible so the terminal grid stays intact.
pub fn content_spans(
    text: &str,
    base: Style,
    syntax: Option<&[StyledRange]>,
    words: Option<&[Range<usize>]>,
    word_bg: Color,
) -> Vec<Span<'static>> {
    let syntax = syntax.unwrap_or(&[]);
    let words = words.unwrap_or(&[]);
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for r in syntax.iter().map(|s| &s.range).chain(words.iter()) {
        cuts.push(r.start.min(text.len()));
        cuts.push(r.end.min(text.len()));
    }
    cuts.retain(|&c| text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans = Vec::with_capacity(cuts.len());
    let mut col = 0usize;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let mut style = base;
        if let Some(s) = syntax
            .iter()
            .find(|s| s.range.start <= a && a < s.range.end)
        {
            style = style.patch(s.style);
        }
        if words.iter().any(|r| r.start <= a && a < r.end) {
            style = style.bg(word_bg);
        }
        spans.push(Span::styled(sanitize(&text[a..b], &mut col), style));
    }
    spans
}

/// Expand tabs relative to the running display column `col`, replace control characters.
pub fn sanitize(s: &str, col: &mut usize) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\t' {
            let n = TAB_WIDTH - (*col % TAB_WIDTH);
            out.extend(std::iter::repeat(' ').take(n));
            *col += n;
        } else if c.is_control() {
            out.push('\u{FFFD}');
            *col += 1;
        } else {
            out.push(c);
            *col += c.width().unwrap_or(0);
        }
    }
    out
}

/// Line background, word-highlight background and default foreground for a line kind.
fn kind_colors(kind: LineKind, ctx: &Ctx) -> (Option<Color>, Color, Style) {
    let t = &ctx.theme;
    match kind {
        LineKind::Added => (
            Some(t.added_bg),
            t.added_word_bg,
            Style::default().fg(t.added),
        ),
        LineKind::Removed => (
            Some(t.removed_bg),
            t.removed_word_bg,
            Style::default().fg(t.removed),
        ),
        LineKind::Context => (None, t.selection_bg, t.text()),
        LineKind::NoNewline => (
            None,
            t.selection_bg,
            t.dimmed().add_modifier(Modifier::ITALIC),
        ),
    }
}

/// Paints diff rows. Holds what every row needs so the draw functions stay small.
pub struct Painter<'a> {
    pub diffs: &'a [FileDiff],
    pub cache: &'a mut DiffCache,
    pub ctx: &'a Ctx,
    pub flags: Flags,
}

impl Painter<'_> {
    /// Styled content of one diff line (without sign or line numbers) and its row background.
    fn line_content(
        &mut self,
        f: usize,
        h: usize,
        l: usize,
    ) -> (Vec<Span<'static>>, Option<Color>) {
        let Some(dl) = self
            .diffs
            .get(f)
            .and_then(|d| d.hunks.get(h))
            .and_then(|hk| hk.lines.get(l))
        else {
            return (Vec::new(), None);
        };
        let dl: &DiffLine = dl;
        let (bg, word_bg, base) = kind_colors(dl.kind, self.ctx);
        if dl.kind == LineKind::NoNewline {
            return (vec![Span::styled(dl.content.clone(), base)], None);
        }
        let words = if self.flags.word_diff && dl.kind != LineKind::Context {
            self.cache.words(self.diffs, f, h, l).map(<[_]>::to_vec)
        } else {
            None
        };
        let theme = self.ctx.theme.syntax_theme;
        let syntax = if self.flags.syntax {
            self.cache
                .syntax(self.diffs, (f, h, l), Side::of(dl.kind), theme)
        } else {
            None
        };
        (
            content_spans(&dl.content, base, syntax, words.as_deref(), word_bg),
            bg,
        )
    }

    fn file_title(&self, f: usize) -> Line<'static> {
        let t = &self.ctx.theme;
        let Some(d) = self.diffs.get(f) else {
            return Line::default();
        };
        let mut spans = Vec::new();
        if let (Some(o), Some(n)) = (&d.old_path, &d.new_path) {
            if o != n {
                spans.push(Span::styled(format!("{o} → "), t.dimmed()));
                spans.push(Span::styled(n.clone(), t.title()));
            }
        }
        if spans.is_empty() {
            spans.push(Span::styled(d.path().to_string(), t.title()));
        }
        spans.push(Span::styled(
            format!("  {}", model::kind_label(d.kind)),
            t.dimmed(),
        ));
        spans.extend(counts(d, self.ctx));
        Line::from(spans)
    }

    /// A row spanning the full width that is neither a line nor a split cell.
    fn full_row(
        &mut self,
        frame: &mut Frame,
        rect: Rect,
        row: &Row,
        gutter: u16,
        cursor: bool,
    ) -> bool {
        let t = &self.ctx.theme;
        let line = match row {
            Row::File(fi) => self.file_title(*fi),
            Row::Meta(_, text) => Line::from(vec![
                Span::raw(" ".repeat(gutter as usize)),
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(t.warning)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]),
            Row::Hunk(fi, hi) => {
                let header = self
                    .diffs
                    .get(*fi)
                    .and_then(|d| d.hunks.get(*hi))
                    .map(|h| h.header.clone())
                    .unwrap_or_default();
                Line::from(vec![
                    Span::raw(" ".repeat(gutter as usize)),
                    Span::styled(header, Style::default().fg(t.hunk)),
                ])
            }
            _ => return false,
        };
        frame.render_widget(Paragraph::new(line), rect);
        if cursor {
            let w = gutter.max(1).min(rect.width);
            frame
                .buffer_mut()
                .set_style(Rect { width: w, ..rect }, t.selected());
        }
        true
    }

    /// Paint `rows` as a unified diff.
    pub fn unified(&mut self, frame: &mut Frame, area: Rect, rows: &[Row], vp: Viewport) {
        let ctx = self.ctx;
        let t = &ctx.theme;
        for (y, ri) in (vp.scroll..rows.len())
            .take(area.height as usize)
            .enumerate()
        {
            let rect = Rect {
                y: area.y + y as u16,
                height: 1,
                ..area
            };
            let row = &rows[ri];
            let digits = self.cache.digits(self.diffs, row.file());
            let gw = (2 * digits + 2) as u16;
            let cursor = vp.cursor == Some(ri);
            if self.full_row(frame, rect, row, gw + 1, cursor) {
                continue;
            }
            let Row::Line { file, hunk, line } = *row else {
                continue;
            };
            let Some(dl) = self.diffs[file]
                .hunks
                .get(hunk)
                .and_then(|h| h.lines.get(line))
            else {
                continue;
            };
            let num = |n: Option<u32>| {
                n.map(|n| format!("{n:>digits$}"))
                    .unwrap_or_else(|| " ".repeat(digits))
            };
            let gutter_style = if cursor { t.selected() } else { t.dimmed() };
            let [g, rest] = split_h(rect, gw);
            frame.render_widget(
                Paragraph::new(format!("{} {} ", num(dl.old_no), num(dl.new_no)))
                    .style(gutter_style),
                g,
            );
            let (sign, sign_style) = match dl.kind {
                LineKind::Added => ("+", Style::default().fg(t.added)),
                LineKind::Removed => ("-", Style::default().fg(t.removed)),
                _ => (" ", t.dimmed()),
            };
            let [s, content] = split_h(rest, 1);
            let (spans, bg) = self.line_content(file, hunk, line);
            paint(frame, s, vec![Span::styled(sign, sign_style)], bg, 0);
            paint(frame, content, spans, bg, vp.hscroll);
        }
    }

    /// Paint `rows` (from [`model::split_rows`]) side by side.
    pub fn split(&mut self, frame: &mut Frame, area: Rect, rows: &[Row], vp: Viewport) {
        let ctx = self.ctx;
        let t = &ctx.theme;
        let lw = area.width.saturating_sub(1) / 2;
        for (y, ri) in (vp.scroll..rows.len())
            .take(area.height as usize)
            .enumerate()
        {
            let rect = Rect {
                y: area.y + y as u16,
                height: 1,
                ..area
            };
            let row = &rows[ri];
            let digits = self.cache.digits(self.diffs, row.file());
            let gw = (digits + 1) as u16;
            let cursor = vp.cursor == Some(ri);
            if self.full_row(frame, rect, row, gw, cursor) {
                continue;
            }
            let Row::Split {
                file,
                hunk,
                old,
                new,
            } = *row
            else {
                continue;
            };
            let [left, sep, right] = split_h3(rect, lw, 1);
            frame.render_widget(Paragraph::new("│").style(t.border(false)), sep);
            for (cell, idx, side) in [(left, old, Side::Old), (right, new, Side::New)] {
                self.split_cell(
                    frame,
                    cell,
                    (file, hunk, idx),
                    side,
                    digits,
                    cursor,
                    vp.hscroll,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn split_cell(
        &mut self,
        frame: &mut Frame,
        cell: Rect,
        (file, hunk, idx): (usize, usize, Option<usize>),
        side: Side,
        digits: usize,
        cursor: bool,
        hscroll: u16,
    ) {
        let t = &self.ctx.theme;
        let gutter_style = if cursor { t.selected() } else { t.dimmed() };
        let [g, content] = split_h(cell, (digits + 1) as u16);
        let dl = idx.and_then(|i| self.diffs[file].hunks.get(hunk)?.lines.get(i));
        let no = dl.and_then(|l| {
            if side == Side::Old {
                l.old_no
            } else {
                l.new_no
            }
        });
        let num = no
            .map(|n| format!("{n:>digits$} "))
            .unwrap_or_else(|| " ".repeat(digits + 1));
        frame.render_widget(Paragraph::new(num).style(gutter_style), g);
        match (dl, idx) {
            (Some(_), Some(i)) => {
                let (spans, bg) = self.line_content(file, hunk, i);
                paint(frame, content, spans, bg, hscroll);
            }
            _ => {
                let filler = "╱".repeat(content.width as usize);
                frame.render_widget(
                    Paragraph::new(filler).style(Style::default().fg(t.border)),
                    content,
                );
            }
        }
    }
}

/// `+a -r` counts in diff colors.
pub fn counts(d: &FileDiff, ctx: &Ctx) -> Vec<Span<'static>> {
    let t = &ctx.theme;
    vec![
        Span::styled(
            format!("  +{}", d.added_count()),
            Style::default().fg(t.added),
        ),
        Span::styled(
            format!(" -{}", d.removed_count()),
            Style::default().fg(t.removed),
        ),
    ]
}

fn paint(
    frame: &mut Frame,
    rect: Rect,
    spans: Vec<Span<'static>>,
    bg: Option<Color>,
    hscroll: u16,
) {
    if rect.width == 0 {
        return;
    }
    if let Some(bg) = bg {
        frame.buffer_mut().set_style(rect, Style::default().bg(bg));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).scroll((0, hscroll)), rect);
}

fn split_h(r: Rect, first: u16) -> [Rect; 2] {
    let w = first.min(r.width);
    [
        Rect { width: w, ..r },
        Rect {
            x: r.x + w,
            width: r.width - w,
            ..r
        },
    ]
}

fn split_h3(r: Rect, first: u16, second: u16) -> [Rect; 3] {
    let [a, rest] = split_h(r, first);
    let [b, c] = split_h(rest, second);
    [a, b, c]
}

/// Content hash of a diff list, so callers handing in fresh copies every frame still hit the cache.
pub fn fingerprint(diffs: &[FileDiff], theme: &str) -> u64 {
    let mut h = DefaultHasher::new();
    theme.hash(&mut h);
    diffs.len().hash(&mut h);
    for d in diffs {
        d.old_path.hash(&mut h);
        d.new_path.hash(&mut h);
        model::kind_letter(d.kind).hash(&mut h);
        d.binary.hash(&mut h);
        d.old_mode.hash(&mut h);
        d.new_mode.hash(&mut h);
        d.header.hash(&mut h);
        for hk in &d.hunks {
            hk.header.hash(&mut h);
            for l in &hk.lines {
                (l.kind as u8).hash(&mut h);
                l.content.hash(&mut h);
            }
        }
    }
    h.finish()
}

struct InlineEntry {
    key: u64,
    rows: Vec<Row>,
    cache: DiffCache,
}

thread_local! {
    static INLINE: RefCell<Vec<InlineEntry>> = const { RefCell::new(Vec::new()) };
}

/// How many distinct inline diffs keep their caches (several tabs may show previews).
const INLINE_SLOTS: usize = 4;

/// Unified rendering for inline previews, cached by content so it is cheap every frame.
pub fn draw_inline(frame: &mut Frame, area: Rect, diffs: &[FileDiff], scroll: usize, ctx: &Ctx) {
    let key = fingerprint(diffs, ctx.theme.syntax_theme);
    INLINE.with(|slots| {
        let mut slots = slots.borrow_mut();
        let pos = match slots.iter().position(|e| e.key == key) {
            Some(p) => p,
            None => {
                if slots.len() >= INLINE_SLOTS {
                    slots.remove(0);
                }
                slots.push(InlineEntry {
                    key,
                    rows: model::unified_rows(diffs, 0..diffs.len(), true),
                    cache: DiffCache::default(),
                });
                slots.len() - 1
            }
        };
        // Most recently used last.
        let mut entry = slots.remove(pos);
        entry.cache.begin_frame();
        let mut p = Painter {
            diffs,
            cache: &mut entry.cache,
            ctx,
            flags: Flags::from_config(&ctx.config.diff),
        };
        p.unified(
            frame,
            area,
            &entry.rows,
            Viewport {
                scroll,
                ..Viewport::default()
            },
        );
        slots.push(entry);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::diff::parse_unified;
    use crate::git::testutil::temp_repo;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn composes_syntax_and_words() {
        let syntax = vec![
            StyledRange {
                style: Style::default().fg(Color::Red),
                range: 0..3,
            },
            StyledRange {
                style: Style::default().fg(Color::Blue),
                range: 3..9,
            },
        ];
        let words: Vec<Range<usize>> = std::iter::once(2..5).collect();
        let s = content_spans(
            "let x = 1",
            Style::default().fg(Color::Green),
            Some(&syntax),
            Some(&words),
            Color::Yellow,
        );
        assert_eq!(text_of(&s), "let x = 1");
        let parts: Vec<(&str, Option<Color>, Option<Color>)> = s
            .iter()
            .map(|s| (s.content.as_ref(), s.style.fg, s.style.bg))
            .collect();
        assert_eq!(
            parts,
            vec![
                ("le", Some(Color::Red), None),
                ("t", Some(Color::Red), Some(Color::Yellow)),
                (" x", Some(Color::Blue), Some(Color::Yellow)),
                (" = 1", Some(Color::Blue), None),
            ]
        );
    }

    #[test]
    fn plain_content_keeps_base_style() {
        let s = content_spans(
            "abc",
            Style::default().fg(Color::Green),
            None,
            None,
            Color::Yellow,
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].style.fg, Some(Color::Green));
        assert!(content_spans("", Style::default(), None, None, Color::Yellow).is_empty());
    }

    #[test]
    fn expands_tabs_and_control_chars() {
        let mut col = 0;
        assert_eq!(sanitize("\tx", &mut col), "    x");
        assert_eq!(col, 5);
        let mut col = 2;
        assert_eq!(sanitize("\t", &mut col), "  ");
        let mut col = 0;
        assert_eq!(sanitize("a\u{1b}b", &mut col), "a\u{FFFD}b");
        // Tabs continue from the column reached by earlier spans.
        let s = content_spans(
            "ab\tc",
            Style::default(),
            None,
            Some(std::slice::from_ref(&(0..1))),
            Color::Yellow,
        );
        assert_eq!(text_of(&s), "ab  c");
    }

    #[test]
    fn ignores_ranges_off_char_boundaries() {
        let s = content_spans(
            "é",
            Style::default(),
            None,
            Some(std::slice::from_ref(&(1..2))),
            Color::Yellow,
        );
        assert_eq!(text_of(&s), "é");
    }

    #[test]
    fn fingerprint_tracks_content() {
        let a = parse_unified("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n");
        let b = parse_unified("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+c\n");
        assert_eq!(fingerprint(&a, "t"), fingerprint(&a.clone(), "t"));
        assert_ne!(fingerprint(&a, "t"), fingerprint(&b, "t"));
        assert_ne!(fingerprint(&a, "t"), fingerprint(&a, "u"));
    }

    fn render(diffs: &[FileDiff], split: bool) -> Vec<String> {
        let (_d, repo) = temp_repo();
        let ctx = Ctx::new(repo, Config::default());
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        let mut cache = DiffCache::default();
        term.draw(|f| {
            cache.begin_frame();
            let mut p = Painter {
                diffs,
                cache: &mut cache,
                ctx: &ctx,
                flags: Flags {
                    word_diff: true,
                    syntax: true,
                },
            };
            let area = f.area();
            if split {
                let rows = model::split_rows(diffs, 0);
                p.split(f, area, &rows, Viewport::default());
            } else {
                let rows = model::unified_rows(diffs, 0..diffs.len(), true);
                p.unified(f, area, &rows, Viewport::default());
            }
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    const SAMPLE: &str = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn a() {
-\tlet x = 1;
+\tlet y = 1;
 }
";

    #[test]
    fn renders_unified_rows() {
        let d = parse_unified(SAMPLE);
        let lines = render(&d, false);
        assert!(lines[0].starts_with("a.rs  modified  +1 -1"));
        assert!(lines[1].contains("@@ -1,3 +1,3 @@"));
        assert!(lines[2].contains("  1   1  fn a() {"));
        assert!(
            lines[3].contains("  2     -    let x = 1;"),
            "{:?}",
            lines[3]
        );
        assert!(
            lines[4].contains("      2 +    let y = 1;"),
            "{:?}",
            lines[4]
        );
    }

    #[test]
    fn renders_side_by_side_rows() {
        let d = parse_unified(SAMPLE);
        let lines = render(&d, true);
        assert!(lines[0].contains("@@ -1,3 +1,3 @@"));
        assert!(lines[2].contains("  2     let x = 1;"), "{:?}", lines[2]);
        assert!(lines[2].contains('│'));
        let right = lines[2].split('│').nth(1).unwrap();
        assert!(right.contains("  2     let y = 1;"), "{right:?}");
    }

    #[test]
    fn inline_cache_handles_repeated_frames() {
        let (_d, repo) = temp_repo();
        let ctx = Ctx::new(repo, Config::default());
        let d = parse_unified(SAMPLE);
        let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
        for scroll in [0, 1, 2, 100] {
            term.draw(|f| draw_inline(f, f.area(), &d, scroll, &ctx))
                .unwrap();
        }
        INLINE.with(|s| assert_eq!(s.borrow().len(), 1));
    }
}
