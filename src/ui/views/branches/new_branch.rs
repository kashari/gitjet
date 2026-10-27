//! "New branch" dialog: name field plus "Checkout branch" / "Overwrite existing branch" checkboxes.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::branch::{self, Branch};
use crate::ui::popups::ConfirmPopup;
use crate::ui::widgets::{centered, panel, TextInput};
use crate::ui::{KeyHelp, Popup};

use super::ops::done;

/// Focusable parts of the dialog, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Checkout,
    Overwrite,
}

impl Field {
    fn next(self) -> Field {
        match self {
            Field::Name => Field::Checkout,
            Field::Checkout => Field::Overwrite,
            Field::Overwrite => Field::Name,
        }
    }

    fn prev(self) -> Field {
        match self {
            Field::Name => Field::Overwrite,
            Field::Checkout => Field::Name,
            Field::Overwrite => Field::Checkout,
        }
    }
}

pub struct NewBranchPopup {
    /// Start point (empty = HEAD)
    start: String,
    input: TextInput,
    checkout: bool,
    overwrite: bool,
    focus: Field,
    error: Option<String>,
}

impl NewBranchPopup {
    /// Start at `start` (None = HEAD). For remote branches the name is prefilled with the short name.
    pub fn new(start: Option<&Branch>) -> Self {
        let initial = start
            .filter(|b| b.is_remote())
            .map(|b| b.short_name().to_string())
            .unwrap_or_default();
        Self {
            start: start.map(|b| b.name.clone()).unwrap_or_default(),
            input: TextInput::new(&initial),
            checkout: true,
            overwrite: false,
            focus: Field::Name,
            error: None,
        }
    }

    /// Validate, then create the branch; overwriting a branch that would lose commits asks first.
    fn create(&mut self, ctx: &Ctx) -> Action {
        let name = self.input.value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a branch name".into());
            return Action::None;
        }
        let req = CreateRequest {
            name,
            start: self.start.clone(),
            checkout: self.checkout,
            overwrite: self.overwrite,
        };
        if req.overwrite {
            if let Some((lost, tip)) =
                branch::commits_lost_by_reset(&ctx.repo, &req.name, &req.start)
            {
                if lost > 0 {
                    return overwrite_confirm(req, lost, &tip);
                }
            }
        }
        match req.run(ctx) {
            Ok(a) => a,
            Err(msg) => {
                self.error = Some(msg);
                Action::None
            }
        }
    }

    fn toggle(&mut self) {
        match self.focus {
            Field::Checkout => self.checkout = !self.checkout,
            Field::Overwrite => self.overwrite = !self.overwrite,
            Field::Name => {}
        }
    }
}

/// A validated "create branch" request.
#[derive(Debug, Clone)]
struct CreateRequest {
    name: String,
    /// Start point (empty = HEAD)
    start: String,
    checkout: bool,
    overwrite: bool,
}

impl CreateRequest {
    /// Create the branch; on success close the dialog with a toast, else the inline error text.
    fn run(&self, ctx: &Ctx) -> Result<Action, String> {
        let name = &self.name;
        match branch::create_branch_with(
            &ctx.repo,
            name,
            &self.start,
            self.checkout,
            self.overwrite,
        ) {
            Ok(()) => {
                let from = if self.start.is_empty() {
                    String::new()
                } else {
                    format!(" from {}", self.start)
                };
                let what = match (self.checkout, self.overwrite) {
                    (true, true) => "Reset and checked out",
                    (false, true) => "Reset",
                    (true, false) => "Created and checked out",
                    (false, false) => "Created",
                };
                Ok(Action::ClosePopup.then(done(format!("{what} branch {name}{from}"))))
            }
            Err(e) => {
                let msg = format!("{e:#}");
                Err(if msg.contains("already exists") {
                    format!("{msg} — tick \"Overwrite existing branch\" to reset it")
                } else {
                    msg
                })
            }
        }
    }
}

/// Danger confirm before resetting an existing branch whose `lost` commits would become unreachable.
/// Confirming closes the confirm and, on success, the dialog underneath; errors keep the dialog open.
fn overwrite_confirm(req: CreateRequest, lost: u32, tip: &str) -> Action {
    let short: String = tip.chars().take(8).collect();
    let start = if req.start.is_empty() {
        "HEAD"
    } else {
        req.start.as_str()
    };
    let msg = format!(
        "Branch '{}' already exists and has {} that {} not in {start}.\n\
         Overwriting resets it to {start}; those commits will only be reachable via the reflog\n\
         (old tip {short}, restore with: git branch -f {} {short}).\n\nOverwrite it?",
        req.name,
        branch::commits(lost),
        if lost == 1 { "is" } else { "are" },
        req.name,
    );
    Action::popup(
        ConfirmPopup::new("Overwrite existing branch", msg, move |ctx| {
            req.run(ctx).unwrap_or_else(Action::Error)
        })
        .danger(),
    )
}

fn checkbox(on: bool, label: &str, focused: bool, ctx: &Ctx, danger: bool) -> Line<'static> {
    let t = &ctx.theme;
    let mark = if on { "[x] " } else { "[ ] " };
    let mut style = if danger && on {
        Style::default().fg(t.warning)
    } else {
        t.text()
    };
    if focused {
        style = style.patch(t.selected());
    }
    Line::from(vec![Span::styled(format!("{mark}{label}"), style)])
}

impl Popup for NewBranchPopup {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let r = centered(area, 72, 11);
        f.render_widget(Clear, r);
        let from = if self.start.is_empty() {
            "HEAD"
        } else {
            self.start.as_str()
        };
        let block = panel(format!(" New branch from {from} "), true, t);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [label, field, _, cb1, cb2, err, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        f.render_widget(Span::styled("Branch name:", t.dimmed()), label);
        let field_style = if self.focus == Field::Name {
            t.text()
        } else {
            t.dimmed()
        };
        self.input
            .render(f, field, field_style, self.focus == Field::Name);
        f.render_widget(
            checkbox(
                self.checkout,
                "Checkout branch",
                self.focus == Field::Checkout,
                ctx,
                false,
            ),
            cb1,
        );
        f.render_widget(
            checkbox(
                self.overwrite,
                "Overwrite existing branch",
                self.focus == Field::Overwrite,
                ctx,
                true,
            ),
            cb2,
        );
        if let Some(e) = &self.error {
            f.render_widget(
                Paragraph::new(e.as_str())
                    .style(Style::default().fg(t.error))
                    .wrap(Wrap { trim: true }),
                err,
            );
        }
        f.render_widget(
            Line::from(vec![
                Span::styled("Enter", t.key()),
                Span::styled(" create   ", t.dimmed()),
                Span::styled("Tab", t.key()),
                Span::styled(" next field   ", t.dimmed()),
                Span::styled("Space", t.key()),
                Span::styled(" toggle   ", t.dimmed()),
                Span::styled("Esc", t.key()),
                Span::styled(" cancel", t.dimmed()),
            ]),
            keys,
        );
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => return Action::ClosePopup,
            KeyCode::Enter => return self.create(ctx),
            KeyCode::Tab | KeyCode::Down => self.focus = self.focus.next(),
            KeyCode::BackTab | KeyCode::Up => self.focus = self.focus.prev(),
            KeyCode::Char(' ') if self.focus != Field::Name => self.toggle(),
            _ if self.focus == Field::Name && self.input.handle_key(&key) => self.error = None,
            _ => {}
        }
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            ("Enter", "create"),
            ("Tab", "next field"),
            ("Space", "toggle"),
            ("Esc", "cancel"),
        ]
    }
}
