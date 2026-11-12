//! `gitjet __screenshot <theme> <tab 1-9> <out.svg> [dir] [keys]`: draw the real UI off screen
//! and save it as an SVG picture. Hidden; used to make the theme gallery in the README.
//!
//! The picture does not depend on the fonts of whoever looks at it more than it has to: cell
//! backgrounds are rectangles, lines of boxes and of the commit graph are drawn as lines, and
//! every character is placed at its own cell.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

pub const ARG: &str = "__screenshot";

const COLS: u16 = 132;
const ROWS: u16 = 26;
const CW: f32 = 8.4;
const CH: f32 = 18.0;
const PAD: f32 = 14.0;

pub fn run(args: &[String]) -> Result<()> {
    let [theme, tab, out, rest @ ..] = args else {
        bail!("usage: gitjet {ARG} <theme> <tab 1-9> <out.svg> [dir] [keys]");
    };
    let dir = rest.first().map_or(".", String::as_str);
    let keys = rest.get(1).map_or("", String::as_str);
    let repo = crate::git::Repo::discover(Path::new(dir))?;
    let config = crate::config::Config {
        theme: theme.clone(),
        ..crate::config::Config::default()
    };
    let ctx = crate::ctx::Ctx::new(repo, config);
    let dark = !matches!(theme.as_str(), "light");
    let (bg, fg) = (
        ctx.theme.bg.unwrap_or(if dark {
            Color::Rgb(0x16, 0x18, 0x1d)
        } else {
            Color::Rgb(0xff, 0xff, 0xff)
        }),
        ctx.theme.fg,
    );
    let tab: usize = tab.parse().context("tab must be a number from 1 to 9")?;
    let buffer = crate::app::render_off_screen(ctx, tab.saturating_sub(1), COLS, ROWS, keys);
    std::fs::write(out, svg(&buffer, bg, fg, theme)).with_context(|| format!("writing {out}"))
}

fn hex(c: Color, default: Color) -> String {
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => match default {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (0x80, 0x80, 0x80),
        },
        Color::Black => (0, 0, 0),
        Color::Red => (0xcd, 0x31, 0x31),
        Color::Green => (0x0d, 0xbc, 0x79),
        Color::Yellow => (0xe5, 0xe5, 0x10),
        Color::Blue => (0x24, 0x72, 0xc8),
        Color::Magenta => (0xbc, 0x3f, 0xbc),
        Color::Cyan => (0x11, 0xa8, 0xcd),
        Color::Gray => (0xcc, 0xcc, 0xcc),
        Color::DarkGray => (0x66, 0x66, 0x66),
        Color::White => (0xff, 0xff, 0xff),
        _ => (0x80, 0x80, 0x80),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Which half-lines from the cell center a box-drawing character has: (left, right, up, down).
/// `None` for characters that are drawn as text.
fn box_arms(ch: &str) -> Option<(bool, bool, bool, bool)> {
    Some(match ch {
        "─" | "━" => (true, true, false, false),
        "│" | "┃" => (false, false, true, true),
        "╭" | "┌" => (false, true, false, true),
        "╮" | "┐" => (true, false, false, true),
        "╰" | "└" => (false, true, true, false),
        "╯" | "┘" => (true, false, true, false),
        "├" => (false, true, true, true),
        "┤" => (true, false, true, true),
        "┬" => (true, true, false, true),
        "┴" => (true, true, true, false),
        "┼" => (true, true, true, true),
        _ => return None,
    })
}

fn svg(buf: &Buffer, bg: Color, fg: Color, title: &str) -> String {
    let area = buf.area;
    let (w, h) = (
        area.width as f32 * CW + 2.0 * PAD,
        area.height as f32 * CH + 2.0 * PAD,
    );
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" font-family="'JetBrains Mono','Cascadia Mono','DejaVu Sans Mono',Menlo,Consolas,'Liberation Mono',monospace" font-size="14"><title>gitjet, {title} theme</title><rect width="100%" height="100%" rx="10" fill="{}"/>"#,
        hex(bg, bg)
    );
    let mut text = String::new();
    let mut lines: Vec<(String, String)> = Vec::new();
    for y in 0..area.height {
        let top = PAD + y as f32 * CH;
        // Backgrounds: one rectangle per run of cells with the same color.
        let mut x = 0;
        while x < area.width {
            let cell = &buf[(x, y)];
            let rev = cell.modifier.contains(Modifier::REVERSED);
            let cell_bg = if rev {
                hex(cell.fg, fg)
            } else {
                hex(cell.bg, bg)
            };
            let mut end = x + 1;
            while end < area.width {
                let c = &buf[(end, y)];
                let r = c.modifier.contains(Modifier::REVERSED);
                if (if r { hex(c.fg, fg) } else { hex(c.bg, bg) }) != cell_bg {
                    break;
                }
                end += 1;
            }
            if cell_bg != hex(bg, bg) {
                let _ = write!(
                    s,
                    r#"<rect x="{:.1}" y="{top:.1}" width="{:.1}" height="{CH}" fill="{cell_bg}"/>"#,
                    PAD + x as f32 * CW,
                    (end - x) as f32 * CW + 0.3
                );
            }
            x = end;
        }
        // Text: one element per run of characters in the same style, each character at its cell.
        let mut run: Option<(String, String, String, String)> = None; // (color, attrs, xs, chars)
        let flush = |run: &mut Option<(String, String, String, String)>, text: &mut String| {
            if let Some((color, attrs, xs, chars)) = run.take() {
                let _ = write!(
                    text,
                    r#"<text x="{}" y="{:.1}" fill="{color}"{attrs}>{chars}</text>"#,
                    xs.trim_end(),
                    top + CH * 0.72
                );
            }
        };
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            let ch = cell.symbol();
            if ch.trim().is_empty() {
                flush(&mut run, &mut text);
                continue;
            }
            let rev = cell.modifier.contains(Modifier::REVERSED);
            let color = if rev {
                hex(cell.bg, bg)
            } else {
                hex(cell.fg, fg)
            };
            let left = PAD + x as f32 * CW;
            let (cx, cy) = (left + CW / 2.0, top + CH / 2.0);
            if let Some((l, r, u, d)) = box_arms(ch) {
                flush(&mut run, &mut text);
                let mut path = String::new();
                if l || r {
                    let _ = write!(
                        path,
                        "M{:.1} {cy:.1}H{:.1}",
                        if l { left } else { cx },
                        if r { left + CW } else { cx }
                    );
                }
                if u || d {
                    let _ = write!(
                        path,
                        "M{cx:.1} {:.1}V{:.1}",
                        if u { top } else { cy },
                        if d { top + CH } else { cy }
                    );
                }
                // Neighbouring pieces of the same color continue one path.
                match lines.last_mut() {
                    Some((c, d)) if *c == color => d.push_str(&path),
                    _ => lines.push((color, path)),
                }
                continue;
            }
            if ch == "●" || ch == "○" {
                flush(&mut run, &mut text);
                let fill = if ch == "●" { color.as_str() } else { "none" };
                let _ = write!(
                    s,
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="3.6" fill="{fill}" stroke="{color}"/>"#
                );
                continue;
            }
            let escaped = match ch {
                "<" => "&lt;",
                ">" => "&gt;",
                "&" => "&amp;",
                other => other,
            };
            let mut attrs = String::new();
            if cell.modifier.contains(Modifier::BOLD) {
                attrs.push_str(r#" font-weight="bold""#);
            }
            if cell.modifier.contains(Modifier::UNDERLINED) {
                attrs.push_str(r#" text-decoration="underline""#);
            }
            if run
                .as_ref()
                .is_some_and(|(c, a, _, _)| *c != color || *a != attrs)
            {
                flush(&mut run, &mut text);
            }
            let (_, _, xs, chars) =
                run.get_or_insert_with(|| (color, attrs, String::new(), String::new()));
            let _ = write!(xs, "{cx:.1} ");
            chars.push_str(escaped);
        }
        flush(&mut run, &mut text);
    }
    for (color, d) in &lines {
        let _ = write!(s, r#"<path d="{d}" stroke="{color}" fill="none"/>"#);
    }
    let _ = write!(s, r#"<g text-anchor="middle">{text}</g></svg>"#);
    s
}
