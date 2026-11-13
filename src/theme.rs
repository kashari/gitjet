//! Colors and common styles. Always take colors from here so light/dark both work.

use ratatui::style::{Color, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    /// Background painted behind everything; `None` keeps the terminal's own background
    pub bg: Option<Color>,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    pub border_focus: Color,
    pub selection_bg: Color,
    pub added: Color,
    pub removed: Color,
    pub modified: Color,
    pub renamed: Color,
    pub untracked: Color,
    pub ignored: Color,
    pub conflict: Color,
    pub added_bg: Color,
    pub removed_bg: Color,
    pub added_word_bg: Color,
    pub removed_word_bg: Color,
    pub hunk: Color,
    pub hash: Color,
    pub author: Color,
    pub date: Color,
    pub local_branch: Color,
    pub remote_branch: Color,
    pub tag: Color,
    pub head: Color,
    pub warning: Color,
    pub error: Color,
    pub success: Color,
    /// Lane colors for the commit graph
    pub graph: [Color; 8],
    /// syntect theme name used for syntax highlighting
    pub syntax_theme: &'static str,
}

/// The colors a theme is built from.
struct Palette {
    bg: u32,
    fg: u32,
    dim: u32,
    border: u32,
    selection: u32,
    blue: u32,
    green: u32,
    yellow: u32,
    purple: u32,
    cyan: u32,
    red: u32,
    orange: u32,
    syntax: &'static str,
}

fn rgb(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// `amount` (0..=1) of `over` mixed into `base`.
fn mix(base: u32, over: u32, amount: f32) -> Color {
    let ch = |shift: u32| {
        let (b, o) = (
            ((base >> shift) & 0xff) as f32,
            ((over >> shift) & 0xff) as f32,
        );
        (b + (o - b) * amount).round() as u8
    };
    Color::Rgb(ch(16), ch(8), ch(0))
}

/// Built-in themes: (name, palette). `dark` comes first and keeps the terminal's own
/// background; all others paint theirs.
const PALETTES: &[(&str, Palette)] = &[
    (
        "dracula",
        Palette {
            bg: 0x282a36,
            fg: 0xf8f8f2,
            dim: 0x6272a4,
            border: 0x44475a,
            selection: 0x44475a,
            blue: 0x8be9fd,
            green: 0x50fa7b,
            yellow: 0xf1fa8c,
            purple: 0xbd93f9,
            cyan: 0x8be9fd,
            red: 0xff5555,
            orange: 0xffb86c,
            syntax: "base16-eighties.dark",
        },
    ),
    (
        "nord",
        Palette {
            bg: 0x2e3440,
            fg: 0xd8dee9,
            dim: 0x7b88a1,
            border: 0x434c5e,
            selection: 0x434c5e,
            blue: 0x81a1c1,
            green: 0xa3be8c,
            yellow: 0xebcb8b,
            purple: 0xb48ead,
            cyan: 0x88c0d0,
            red: 0xbf616a,
            orange: 0xd08770,
            syntax: "base16-ocean.dark",
        },
    ),
    (
        "gruvbox-dark",
        Palette {
            bg: 0x282828,
            fg: 0xebdbb2,
            dim: 0x928374,
            border: 0x504945,
            selection: 0x504945,
            blue: 0x83a598,
            green: 0xb8bb26,
            yellow: 0xfabd2f,
            purple: 0xd3869b,
            cyan: 0x8ec07c,
            red: 0xfb4934,
            orange: 0xfe8019,
            syntax: "base16-mocha.dark",
        },
    ),
    (
        "gruvbox-light",
        Palette {
            bg: 0xfbf1c7,
            fg: 0x3c3836,
            dim: 0x7c6f64,
            border: 0xd5c4a1,
            selection: 0xebdbb2,
            blue: 0x076678,
            green: 0x79740e,
            yellow: 0xb57614,
            purple: 0x8f3f71,
            cyan: 0x427b58,
            red: 0x9d0006,
            orange: 0xaf3a03,
            syntax: "Solarized (light)",
        },
    ),
    (
        "solarized-dark",
        Palette {
            bg: 0x002b36,
            fg: 0x93a1a1,
            dim: 0x657b83,
            border: 0x0a4a59,
            selection: 0x073642,
            blue: 0x268bd2,
            green: 0x859900,
            yellow: 0xb58900,
            purple: 0x6c71c4,
            cyan: 0x2aa198,
            red: 0xdc322f,
            orange: 0xcb4b16,
            syntax: "Solarized (dark)",
        },
    ),
    (
        "solarized-light",
        Palette {
            bg: 0xfdf6e3,
            fg: 0x586e75,
            dim: 0x93a1a1,
            border: 0xd9d2bf,
            selection: 0xeee8d5,
            blue: 0x268bd2,
            green: 0x859900,
            yellow: 0xb58900,
            purple: 0x6c71c4,
            cyan: 0x2aa198,
            red: 0xdc322f,
            orange: 0xcb4b16,
            syntax: "Solarized (light)",
        },
    ),
    (
        "tokyo-night",
        Palette {
            bg: 0x1a1b26,
            fg: 0xc0caf5,
            dim: 0x565f89,
            border: 0x3b4261,
            selection: 0x283457,
            blue: 0x7aa2f7,
            green: 0x9ece6a,
            yellow: 0xe0af68,
            purple: 0xbb9af7,
            cyan: 0x7dcfff,
            red: 0xf7768e,
            orange: 0xff9e64,
            syntax: "base16-ocean.dark",
        },
    ),
    (
        "catppuccin-mocha",
        Palette {
            bg: 0x1e1e2e,
            fg: 0xcdd6f4,
            dim: 0x7f849c,
            border: 0x45475a,
            selection: 0x45475a,
            blue: 0x89b4fa,
            green: 0xa6e3a1,
            yellow: 0xf9e2af,
            purple: 0xcba6f7,
            cyan: 0x94e2d5,
            red: 0xf38ba8,
            orange: 0xfab387,
            syntax: "base16-mocha.dark",
        },
    ),
    (
        "catppuccin-latte",
        Palette {
            bg: 0xeff1f5,
            fg: 0x4c4f69,
            dim: 0x8c8fa1,
            border: 0xccd0da,
            selection: 0xccd0da,
            blue: 0x1e66f5,
            green: 0x40a02b,
            yellow: 0xdf8e1d,
            purple: 0x8839ef,
            cyan: 0x179299,
            red: 0xd20f39,
            orange: 0xfe640b,
            syntax: "InspiredGitHub",
        },
    ),
    (
        "one-dark",
        Palette {
            bg: 0x282c34,
            fg: 0xabb2bf,
            dim: 0x5c6370,
            border: 0x3e4451,
            selection: 0x3e4451,
            blue: 0x61afef,
            green: 0x98c379,
            yellow: 0xe5c07b,
            purple: 0xc678dd,
            cyan: 0x56b6c2,
            red: 0xe06c75,
            orange: 0xd19a66,
            syntax: "base16-ocean.dark",
        },
    ),
    // One Dark Pro (Binaryify): the same colors as Atom's One Dark above.
    (
        "one-dark-pro",
        Palette {
            bg: 0x282c34,
            fg: 0xabb2bf,
            dim: 0x7f848e,
            border: 0x3e4452,
            // Its translucent selection (#67769660) over the background.
            selection: 0x404859,
            blue: 0x61afef,
            green: 0x98c379,
            yellow: 0xe5c07b,
            purple: 0xc678dd,
            cyan: 0x56b6c2,
            red: 0xe06c75,
            orange: 0xd19a66,
            syntax: "base16-ocean.dark",
        },
    ),
    // Panda Syntax (tinkertrain); the selection is its translucent orange over the background.
    (
        "panda",
        Palette {
            bg: 0x292a2b,
            fg: 0xe6e6e6,
            dim: 0x676b79,
            border: 0x3f4142,
            selection: 0x5e4d3b,
            blue: 0x45a9f9,
            green: 0x19f9d8,
            yellow: 0xffb86c,
            purple: 0xb084eb,
            cyan: 0x6fc1ff,
            red: 0xff2c6d,
            orange: 0xff75b5,
            syntax: "base16-eighties.dark",
        },
    ),
    // Bluloco Light (uloco).
    (
        "bluloco-light",
        Palette {
            bg: 0xf9f9f9,
            fg: 0x383a42,
            dim: 0xa0a1a7,
            border: 0xd5d7d8,
            selection: 0xd2ecff,
            blue: 0x275fe4,
            green: 0x23974a,
            yellow: 0xc5a332,
            purple: 0x823ff1,
            cyan: 0x0098dd,
            red: 0xd52753,
            orange: 0xdf631c,
            syntax: "InspiredGitHub",
        },
    ),
    // "Theme" by tal7aouy and its "Theme Darker" variant: One Dark with stronger colors.
    (
        "tal7aouy",
        Palette {
            bg: 0x282c34,
            fg: 0xabb2bf,
            dim: 0x7f848e,
            border: 0x3e4452,
            selection: 0x404859,
            blue: 0x61afef,
            green: 0x89ca78,
            yellow: 0xe5c07b,
            purple: 0xd55fde,
            cyan: 0x42b3c2,
            red: 0xef596f,
            orange: 0xd19a66,
            syntax: "base16-ocean.dark",
        },
    ),
    (
        "tal7aouy-darker",
        Palette {
            bg: 0x23272e,
            fg: 0xabb2bf,
            dim: 0x7f848e,
            border: 0x3e4452,
            selection: 0x3d4555,
            blue: 0x61afef,
            green: 0x89ca78,
            yellow: 0xe5c07b,
            purple: 0xd55fde,
            cyan: 0x42b3c2,
            red: 0xef596f,
            orange: 0xd19a66,
            syntax: "base16-ocean.dark",
        },
    ),
    // Apple Dark (Mac3): near-black background, white text. Comments and the selection are a
    // step brighter than in the VS Code theme, for contrast in lists.
    (
        "apple-dark",
        Palette {
            bg: 0x1c1c1e,
            fg: 0xffffff,
            dim: 0x9a9a9e,
            border: 0x3f3f41,
            selection: 0x173b52,
            blue: 0x49ace9,
            green: 0x49e9a6,
            yellow: 0xe4b781,
            purple: 0xdf769b,
            cyan: 0x29b8db,
            red: 0xf14c4c,
            orange: 0xe66533,
            syntax: "base16-eighties.dark",
        },
    ),
    // GitHub Light Theme (Hyzeta / yunlingz): white, black text, GitHub's syntax colors.
    (
        "github-light",
        Palette {
            bg: 0xffffff,
            fg: 0x000000,
            dim: 0x6a737d,
            border: 0xd1d5da,
            selection: 0xeeeeee,
            blue: 0x005cc5,
            green: 0x22863a,
            yellow: 0xe36209,
            purple: 0x6f42c1,
            cyan: 0x032f62,
            red: 0xd73a49,
            orange: 0xe36209,
            syntax: "InspiredGitHub",
        },
    ),
    (
        "monokai",
        Palette {
            bg: 0x272822,
            fg: 0xf8f8f2,
            dim: 0x75715e,
            border: 0x49483e,
            selection: 0x49483e,
            blue: 0x66d9ef,
            green: 0xa6e22e,
            yellow: 0xe6db74,
            purple: 0xae81ff,
            cyan: 0x66d9ef,
            red: 0xf92672,
            orange: 0xfd971f,
            syntax: "base16-eighties.dark",
        },
    ),
    (
        "github-dark",
        Palette {
            bg: 0x0d1117,
            fg: 0xc9d1d9,
            dim: 0x8b949e,
            border: 0x30363d,
            selection: 0x1f3a5f,
            blue: 0x58a6ff,
            green: 0x3fb950,
            yellow: 0xd29922,
            purple: 0xbc8cff,
            cyan: 0x39c5cf,
            red: 0xf85149,
            orange: 0xf0883e,
            syntax: "base16-ocean.dark",
        },
    ),
    (
        "rose-pine",
        Palette {
            bg: 0x191724,
            fg: 0xe0def4,
            dim: 0x6e6a86,
            border: 0x403d52,
            selection: 0x403d52,
            blue: 0x9ccfd8,
            green: 0x31748f,
            yellow: 0xf6c177,
            purple: 0xc4a7e7,
            cyan: 0x9ccfd8,
            red: 0xeb6f92,
            orange: 0xebbcba,
            syntax: "base16-mocha.dark",
        },
    ),
    (
        "high-contrast",
        Palette {
            bg: 0x000000,
            fg: 0xffffff,
            dim: 0xb0b0b0,
            border: 0x808080,
            selection: 0x003f8f,
            blue: 0x5cb3ff,
            green: 0x3dff7a,
            yellow: 0xffe14d,
            purple: 0xe29bff,
            cyan: 0x4de8ff,
            red: 0xff5c5c,
            orange: 0xffa64d,
            syntax: "base16-ocean.dark",
        },
    ),
];

impl Theme {
    /// Names of all built-in themes, in the order the switcher lists them.
    pub fn names() -> Vec<&'static str> {
        ["dark", "light"]
            .into_iter()
            .chain(PALETTES.iter().map(|(n, _)| *n))
            .collect()
    }

    pub fn from_name(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "light" => Self::light(),
            _ => PALETTES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, p)| Self::from_palette(p))
                .unwrap_or_else(Self::dark),
        }
    }

    /// The theme named in the config, with the `[colors]` overrides applied. The second value
    /// lists what was wrong (unknown theme, unknown color name, bad value).
    pub fn from_config(config: &crate::config::Config) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        if !Self::names().contains(&config.theme.to_ascii_lowercase().as_str()) {
            problems.push(format!(
                "unknown theme \"{}\" (available: {})",
                config.theme,
                Self::names().join(", ")
            ));
        }
        let mut theme = Self::from_name(&config.theme);
        for (key, value) in &config.colors {
            // `bg = "terminal"`: do not paint a background, keep the terminal's own.
            if key == "bg" && matches!(value.trim(), "terminal" | "none") {
                theme.bg = None;
                continue;
            }
            let Some(color) = parse_color(value) else {
                problems.push(format!(
                    "[colors] {key} = \"{value}\": expected \"#rrggbb\""
                ));
                continue;
            };
            if !theme.set(key, color) {
                problems.push(format!("[colors] {key}: no such color"));
            }
        }
        (theme, problems)
    }

    /// Set one color by its field name; `false` when there is no such field.
    fn set(&mut self, key: &str, c: Color) -> bool {
        let slot = match key {
            "bg" => {
                self.bg = Some(c);
                return true;
            }
            "fg" => &mut self.fg,
            "dim" => &mut self.dim,
            "accent" => &mut self.accent,
            "border" => &mut self.border,
            "border_focus" => &mut self.border_focus,
            "selection_bg" => &mut self.selection_bg,
            "added" => &mut self.added,
            "removed" => &mut self.removed,
            "modified" => &mut self.modified,
            "renamed" => &mut self.renamed,
            "untracked" => &mut self.untracked,
            "ignored" => &mut self.ignored,
            "conflict" => &mut self.conflict,
            "added_bg" => &mut self.added_bg,
            "removed_bg" => &mut self.removed_bg,
            "added_word_bg" => &mut self.added_word_bg,
            "removed_word_bg" => &mut self.removed_word_bg,
            "hunk" => &mut self.hunk,
            "hash" => &mut self.hash,
            "author" => &mut self.author,
            "date" => &mut self.date,
            "local_branch" => &mut self.local_branch,
            "remote_branch" => &mut self.remote_branch,
            "tag" => &mut self.tag,
            "head" => &mut self.head,
            "warning" => &mut self.warning,
            "error" => &mut self.error,
            "success" => &mut self.success,
            _ => return false,
        };
        *slot = c;
        true
    }

    fn from_palette(p: &Palette) -> Self {
        Self {
            bg: Some(rgb(p.bg)),
            fg: rgb(p.fg),
            dim: rgb(p.dim),
            accent: rgb(p.blue),
            border: rgb(p.border),
            border_focus: rgb(p.blue),
            selection_bg: rgb(p.selection),
            added: rgb(p.green),
            removed: rgb(p.red),
            modified: rgb(p.blue),
            renamed: rgb(p.purple),
            untracked: rgb(p.orange),
            ignored: rgb(p.dim),
            conflict: rgb(p.red),
            added_bg: mix(p.bg, p.green, 0.16),
            removed_bg: mix(p.bg, p.red, 0.16),
            added_word_bg: mix(p.bg, p.green, 0.36),
            removed_word_bg: mix(p.bg, p.red, 0.36),
            hunk: rgb(p.cyan),
            hash: rgb(p.yellow),
            author: rgb(p.green),
            date: rgb(p.dim),
            local_branch: rgb(p.green),
            remote_branch: rgb(p.purple),
            tag: rgb(p.yellow),
            head: rgb(p.blue),
            warning: rgb(p.yellow),
            error: rgb(p.red),
            success: rgb(p.green),
            graph: [
                rgb(p.blue),
                rgb(p.green),
                rgb(p.yellow),
                rgb(p.purple),
                rgb(p.cyan),
                rgb(p.red),
                rgb(p.orange),
                rgb(p.dim),
            ],
            syntax_theme: p.syntax,
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: None,
            fg: Color::Rgb(0xd4, 0xd7, 0xdd),
            dim: Color::Rgb(0x7f, 0x84, 0x8e),
            accent: Color::Rgb(0x6c, 0xa9, 0xf2),
            border: Color::Rgb(0x3a, 0x3e, 0x46),
            border_focus: Color::Rgb(0x6c, 0xa9, 0xf2),
            selection_bg: Color::Rgb(0x26, 0x36, 0x55),
            added: Color::Rgb(0x73, 0xc9, 0x91),
            removed: Color::Rgb(0xf0, 0x71, 0x78),
            modified: Color::Rgb(0x6c, 0xa9, 0xf2),
            renamed: Color::Rgb(0xc6, 0x95, 0xe8),
            untracked: Color::Rgb(0xd0, 0x8a, 0x5c),
            ignored: Color::Rgb(0x6b, 0x70, 0x78),
            conflict: Color::Rgb(0xff, 0x6b, 0x6b),
            added_bg: Color::Rgb(0x1f, 0x35, 0x28),
            removed_bg: Color::Rgb(0x3d, 0x22, 0x25),
            added_word_bg: Color::Rgb(0x2b, 0x5c, 0x3c),
            removed_word_bg: Color::Rgb(0x75, 0x2f, 0x35),
            hunk: Color::Rgb(0x56, 0xb6, 0xc2),
            hash: Color::Rgb(0xe5, 0xc0, 0x7b),
            author: Color::Rgb(0x98, 0xc3, 0x79),
            date: Color::Rgb(0x7f, 0x84, 0x8e),
            local_branch: Color::Rgb(0x73, 0xc9, 0x91),
            remote_branch: Color::Rgb(0xc6, 0x95, 0xe8),
            tag: Color::Rgb(0xe5, 0xc0, 0x7b),
            head: Color::Rgb(0x6c, 0xa9, 0xf2),
            warning: Color::Rgb(0xe5, 0xc0, 0x7b),
            error: Color::Rgb(0xff, 0x6b, 0x6b),
            success: Color::Rgb(0x73, 0xc9, 0x91),
            graph: [
                Color::Rgb(0x6c, 0xa9, 0xf2),
                Color::Rgb(0x73, 0xc9, 0x91),
                Color::Rgb(0xe5, 0xc0, 0x7b),
                Color::Rgb(0xc6, 0x95, 0xe8),
                Color::Rgb(0x56, 0xb6, 0xc2),
                Color::Rgb(0xf0, 0x71, 0x78),
                Color::Rgb(0xd0, 0x8a, 0x5c),
                Color::Rgb(0xa0, 0xa8, 0xb7),
            ],
            syntax_theme: "base16-ocean.dark",
        }
    }

    pub fn light() -> Self {
        Self {
            // Painted: dark text on a terminal with a dark background would be unreadable.
            bg: Some(Color::Rgb(0xff, 0xff, 0xff)),
            fg: Color::Rgb(0x24, 0x29, 0x2f),
            dim: Color::Rgb(0x6e, 0x77, 0x81),
            accent: Color::Rgb(0x09, 0x69, 0xda),
            border: Color::Rgb(0xd8, 0xdc, 0xe0),
            border_focus: Color::Rgb(0x09, 0x69, 0xda),
            selection_bg: Color::Rgb(0xcc, 0xe0, 0xff),
            added: Color::Rgb(0x1a, 0x7f, 0x37),
            removed: Color::Rgb(0xcf, 0x22, 0x2e),
            modified: Color::Rgb(0x09, 0x69, 0xda),
            renamed: Color::Rgb(0x82, 0x50, 0xdf),
            untracked: Color::Rgb(0xbc, 0x4c, 0x00),
            ignored: Color::Rgb(0x8c, 0x95, 0x9f),
            conflict: Color::Rgb(0xcf, 0x22, 0x2e),
            added_bg: Color::Rgb(0xe6, 0xff, 0xec),
            removed_bg: Color::Rgb(0xff, 0xeb, 0xe9),
            added_word_bg: Color::Rgb(0xab, 0xf2, 0xbc),
            removed_word_bg: Color::Rgb(0xff, 0xc1, 0xc0),
            hunk: Color::Rgb(0x05, 0x50, 0xae),
            hash: Color::Rgb(0x95, 0x38, 0x00),
            author: Color::Rgb(0x1a, 0x7f, 0x37),
            date: Color::Rgb(0x6e, 0x77, 0x81),
            local_branch: Color::Rgb(0x1a, 0x7f, 0x37),
            remote_branch: Color::Rgb(0x82, 0x50, 0xdf),
            tag: Color::Rgb(0x95, 0x38, 0x00),
            head: Color::Rgb(0x09, 0x69, 0xda),
            warning: Color::Rgb(0x9a, 0x67, 0x00),
            error: Color::Rgb(0xcf, 0x22, 0x2e),
            success: Color::Rgb(0x1a, 0x7f, 0x37),
            graph: [
                Color::Rgb(0x09, 0x69, 0xda),
                Color::Rgb(0x1a, 0x7f, 0x37),
                Color::Rgb(0x9a, 0x67, 0x00),
                Color::Rgb(0x82, 0x50, 0xdf),
                Color::Rgb(0x1b, 0x7c, 0x83),
                Color::Rgb(0xcf, 0x22, 0x2e),
                Color::Rgb(0xbc, 0x4c, 0x00),
                Color::Rgb(0x57, 0x60, 0x6a),
            ],
            syntax_theme: "InspiredGitHub",
        }
    }

    pub fn text(&self) -> Style {
        Style::default().fg(self.fg)
    }

    pub fn dimmed(&self) -> Style {
        Style::default().fg(self.dim)
    }

    /// Selection is shown by its background alone; text keeps its regular weight.
    pub fn selected(&self) -> Style {
        Style::default().bg(self.selection_bg)
    }

    pub fn title(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn border(&self, focused: bool) -> Style {
        Style::default().fg(if focused {
            self.border_focus
        } else {
            self.border
        })
    }

    pub fn key(&self) -> Style {
        Style::default().fg(self.accent)
    }
}

/// `#rrggbb` (the `#` is optional).
fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn at_least_ten_themes_all_resolve_by_name() {
        let names = Theme::names();
        assert!(names.len() >= 10, "{names:?}");
        for n in &names {
            let c = Config {
                theme: n.to_string(),
                ..Config::default()
            };
            let (t, problems) = Theme::from_config(&c);
            assert!(problems.is_empty(), "{n}: {problems:?}");
            assert_eq!(t.bg.is_none(), *n == "dark", "{n}");
            assert!(
                crate::ui::diff_view::highlight::theme(t.syntax_theme)
                    .is_some_and(|_| crate::ui::diff_view::highlight::has_theme(t.syntax_theme)),
                "{n}: syntax theme {}",
                t.syntax_theme
            );
        }
        assert_eq!(Theme::from_name("Nord").bg, Some(rgb(0x2e3440)));
    }

    #[test]
    fn colors_from_the_config_override_the_theme_and_mistakes_are_reported() {
        let mut c = Config {
            theme: "nord".into(),
            ..Config::default()
        };
        c.colors.insert("added".into(), "#00ff00".into());
        c.colors.insert("bg".into(), "101010".into());
        c.colors.insert("nonsense".into(), "#000000".into());
        c.colors.insert("removed".into(), "red".into());
        let (t, problems) = Theme::from_config(&c);
        assert_eq!(t.added, Color::Rgb(0, 255, 0));
        assert_eq!(t.bg, Some(Color::Rgb(16, 16, 16)));
        assert_eq!(problems.len(), 2, "{problems:?}");
        // The terminal's own background instead of the theme's.
        c.colors.insert("bg".into(), "terminal".into());
        assert_eq!(Theme::from_config(&c).0.bg, None);
        c.theme = "no-such".into();
        assert!(Theme::from_config(&c).1[0].contains("unknown theme"));
    }
}
