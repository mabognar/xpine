use std::io::Write;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, event, execute};
use crossterm::style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crate::editor::Editor;
use crate::ui::{UiColors, UiExt};
use crossterm::terminal::size as term_size;

pub fn prompt_cancel(stdout: &mut std::io::Stdout, colors: &UiColors) -> bool {
    let (cols, rows) = term_size().unwrap_or((80, 24));

    execute!(
        stdout,
        cursor::MoveTo(0, rows.saturating_sub(3)),
        SetBackgroundColor(colors.ui_bg),
        Clear(ClearType::UntilNewLine),
        SetForegroundColor(colors.accent),
        Print("Are you sure you want to cancel? "),
        ResetColor
    ).unwrap();

    let col_width = (cols as usize / 6).max(1);
    Editor::draw_menu_line(
        stdout,
        rows.saturating_sub(2),
        cols,
        col_width,
        &[("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
        colors.ui_bg,
        colors.accent,
        colors.fg,
    ).unwrap();
    Editor::draw_menu_line(
        stdout,
        rows.saturating_sub(1),
        cols,
        col_width,
        &[("N", " No"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
        colors.ui_bg,
        colors.accent,
        colors.fg,
    ).unwrap();

    stdout.flush().unwrap();

    loop {
        if let Ok(Event::Key(pk)) = event::read() {
            if pk.kind == KeyEventKind::Press {
                match pk.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => return true,
                    KeyCode::Char('n') | KeyCode::Char('N') => return false,
                    KeyCode::Esc => return false,
                    KeyCode::Char('c') if pk.modifiers.contains(KeyModifiers::CONTROL) => return false,
                    _ => {}
                }
            }
        }
    }
}
