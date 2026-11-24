use std::io::{self, stdout, Stdout, Write};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{size as term_size, Clear, ClearType},
};

use crate::browser::BrowserExt;
use crate::config::UiColors;
use crate::editor::{Editor, MenuState};
use crate::theme::derive_ui_colors;
use crate::ui::UiExt;

const EMPTY_MENU: [(&str, &str); 6] = [("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")];
const CANCEL_MENU: [(&str, &str); 6] = [
    ("^C", " Cancel"),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
];
const YES_MENU: [(&str, &str); 6] = [
    ("Y", " Yes"),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
];
const NO_MENU: [(&str, &str); 6] = [
    ("N", " No"),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
    ("", ""),
];

pub trait PromptExt {
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>>;
    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>>;
    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>>;
    fn prompt_with_autocomplete(
        &mut self,
        prompt_text: &str,
        suggestions: &[String],
    ) -> io::Result<Option<String>>;
    fn prompt_edit(&mut self, prompt_text: &str, initial_text: &str) -> io::Result<Option<String>>;
    fn prompt_for_folder(
        &mut self,
        prompt_text: &str,
        folders: &[String],
    ) -> io::Result<Option<String>>;
    fn prompt_select_item(
        &mut self,
        prompt_text: &str,
        items: &[String],
    ) -> io::Result<Option<String>>;
    fn prompt_with_placeholder(
        &mut self,
        prompt_text: &str,
        placeholder: &str,
    ) -> io::Result<Option<String>>;
    fn prompt_cancel(&mut self) -> io::Result<bool>;
}

struct InputLine {
    chars: Vec<char>,
    cursor: usize,
}

impl InputLine {
    fn new(initial: &str) -> Self {
        let chars: Vec<char> = initial.chars().collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    fn as_string(&self) -> String {
        self.chars.iter().collect()
    }

    fn into_string(self) -> String {
        self.chars.into_iter().collect()
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            true
        } else {
            false
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }
}

fn hide_cursor(out: &mut Stdout) -> io::Result<()> {
    queue!(out, cursor::Hide, ResetColor)?;
    out.flush()
}

fn draw_prompt_menus(
    out: &mut Stdout,
    rows: u16,
    cols: u16,
    col_divisor: usize,
    top: &[(&str, &str)],
    bottom: &[(&str, &str)],
    colors: &UiColors,
) -> io::Result<()> {
    let col_width = (cols as usize / col_divisor).max(1);
    Editor::draw_menu_line(
        out,
        rows.saturating_sub(2),
        cols,
        col_width,
        top,
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;
    Editor::draw_menu_line(
        out,
        rows.saturating_sub(1),
        cols,
        col_width,
        bottom,
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )
}

fn format_suggestion_hint(typed: &str, suggestion: &str, idx: usize, total: usize) -> String {
    let indicator = if total > 1 {
        format!(" ({}/{})", idx + 1, total)
    } else {
        String::new()
    };

    if suggestion.to_lowercase().starts_with(&typed.to_lowercase()) {
        let remainder: String = suggestion.chars().skip(typed.chars().count()).collect();
        format!("{}{}", remainder, indicator)
    } else {
        format!("  -> {}{}", suggestion, indicator)
    }
}

impl Editor {
    fn ui_colors(&self) -> UiColors {
        derive_ui_colors(&self.theme_set.themes[&self.current_theme])
    }

    fn run_text_prompt(
        &mut self,
        prompt_text: &str,
        allow_browser: bool,
        placeholder: Option<&str>,
    ) -> io::Result<Option<String>> {
        let previous_state = self.menu_state;
        let mut input = InputLine::new("");
        let mut out = stdout();

        if placeholder.is_some() {
            self.menu_state = MenuState::CancelOnly;
        } else if self.menu_state != MenuState::SpellCheck {
            self.menu_state = if allow_browser {
                MenuState::PromptWithBrowser
            } else {
                MenuState::CancelOnly
            };
        }

        let prompt_len = prompt_text.chars().count();

        loop {
            let input_str = input.as_string();
            self.set_status(format!("{}{}", prompt_text, input_str));

            let (cols, rows) = term_size()?;
            let colors = self.ui_colors();
            let prompt_y = rows.saturating_sub(3);

            queue!(
                out,
                cursor::MoveTo(0, prompt_y),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text)
            )?;

            match placeholder {
                Some(ph) if input.is_empty() => {
                    queue!(out, SetForegroundColor(colors.date_color), Print(ph))?;
                }
                _ => {
                    queue!(out, SetForegroundColor(colors.fg), Print(&input_str))?;
                }
            }
            queue!(out, ResetColor)?;

            if self.menu_state == MenuState::SpellCheck && placeholder.is_none() {
                let sug = |i: usize| {
                    self.current_suggestions
                        .get(i)
                        .map(String::as_str)
                        .unwrap_or("")
                };
                draw_prompt_menus(
                    &mut out,
                    rows,
                    cols,
                    5,
                    &[
                        ("1 ", sug(0)),
                        ("2 ", sug(1)),
                        ("3 ", sug(2)),
                        ("4 ", sug(3)),
                        ("5 ", sug(4)),
                    ],
                    &[("^C", " Cancel"), ("I", " Ignore"), ("A", " Add Dict")],
                    &colors,
                )?;
            } else {
                let top_menu = if allow_browser {
                    [
                        ("^T", " To Files"),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                    ]
                } else {
                    EMPTY_MENU
                };
                draw_prompt_menus(&mut out, rows, cols, 5, &top_menu, &CANCEL_MENU, &colors)?;
            }

            let cursor_x = (prompt_len + input.cursor) as u16;
            queue!(out, cursor::MoveTo(cursor_x, prompt_y), cursor::Show)?;
            out.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let is_alt = key.modifiers.contains(KeyModifiers::ALT);

            match key.code {
                KeyCode::Enter => {
                    hide_cursor(&mut out)?;
                    self.clear_status();
                    self.menu_state = previous_state;
                    return Ok(Some(input.into_string()));
                }
                KeyCode::Esc => {
                    hide_cursor(&mut out)?;
                    self.clear_status();
                    self.menu_state = previous_state;
                    return Ok(None);
                }
                KeyCode::Char('c' | 'g') if is_ctrl => {
                    hide_cursor(&mut out)?;
                    self.clear_status();
                    self.menu_state = previous_state;
                    return Ok(None);
                }
                KeyCode::Char('t') if allow_browser && is_ctrl => {
                    hide_cursor(&mut out)?;
                    if let Ok(Some(selected_file)) = self.run_file_browser(false, None) {
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(Some(selected_file));
                    }
                    self.menu_state = MenuState::PromptWithBrowser;
                }
                KeyCode::Left => input.move_left(),
                KeyCode::Char('b') if is_ctrl => input.move_left(),
                KeyCode::Right => input.move_right(),
                KeyCode::Char('f') if is_ctrl => input.move_right(),
                KeyCode::Backspace => {
                    input.backspace();
                }
                KeyCode::Delete => input.delete(),
                KeyCode::Char(c) if !is_ctrl && !is_alt => input.insert(c),
                _ => {}
            }
        }
    }
}

impl PromptExt for Editor {
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>> {
        self.run_text_prompt(prompt_text, allow_browser, None)
    }

    fn prompt_with_placeholder(
        &mut self,
        prompt_text: &str,
        placeholder: &str,
    ) -> io::Result<Option<String>> {
        self.run_text_prompt(prompt_text, false, Some(placeholder))
    }

    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>> {
        let previous_state = self.menu_state;
        self.menu_state = MenuState::YesNoCancel;
        self.set_status(String::from(prompt_text));

        let mut out = stdout();
        let (cols, rows) = term_size()?;
        let colors = self.ui_colors();

        queue!(
            out,
            cursor::MoveTo(0, rows.saturating_sub(3)),
            SetBackgroundColor(colors.menu_bg),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(colors.accent),
            Print(prompt_text),
            ResetColor
        )?;
        draw_prompt_menus(&mut out, rows, cols, 6, &YES_MENU, &NO_MENU, &colors)?;
        out.flush()?;

        let result = loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('y' | 'Y') => break Some(true),
                    KeyCode::Char('n' | 'N') => break Some(false),
                    KeyCode::Esc | KeyCode::Char('c' | 'C') => break None,
                    _ => {}
                }
            }
        };

        self.clear_status();
        self.menu_state = previous_state;
        Ok(result)
    }

    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>> {
        let previous_state = self.menu_state;
        self.menu_state = MenuState::ReplaceAction;
        self.set_status(String::from(prompt_text));
        self.draw_screen()?;

        let result = loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    break None;
                }
                match key.code {
                    KeyCode::Char('y' | 'Y') => break Some('y'),
                    KeyCode::Char('n' | 'N') => break Some('n'),
                    KeyCode::Char('a' | 'A') => break Some('a'),
                    _ => {}
                }
            }
        };

        self.clear_status();
        self.menu_state = previous_state;
        Ok(result)
    }

    fn prompt_with_autocomplete(
        &mut self,
        prompt_text: &str,
        suggestions: &[String],
    ) -> io::Result<Option<String>> {
        let mut out = stdout();
        let mut input = String::new();
        let mut suggestion_idx = 0;
        let colors = self.ui_colors();
        let prompt_len = prompt_text.chars().count();

        loop {
            let (cols, rows) = term_size().unwrap_or((80, 24));
            let matches = if input.is_empty() {
                Vec::new()
            } else {
                find_email_suggestions(&input, suggestions)
            };

            let (hint, matched_suggestion) = if !matches.is_empty() {
                suggestion_idx %= matches.len();
                let current_match = &matches[suggestion_idx];
                let last_part = input.split(',').last().unwrap_or("").trim_start();
                let hint = format_suggestion_hint(
                    last_part,
                    current_match,
                    suggestion_idx,
                    matches.len(),
                );
                (hint, Some(current_match.clone()))
            } else {
                suggestion_idx = 0;
                (String::new(), None)
            };

            let hint_len = hint.chars().count();
            let input_len = input.chars().count();
            let max_input_display = (cols as usize).saturating_sub(prompt_len + hint_len + 1);

            let display_input = if input_len > max_input_display {
                let start_idx = input_len.saturating_sub(max_input_display.saturating_sub(3));
                let window: String = input.chars().skip(start_idx).collect();
                format!("...{}", window)
            } else {
                input.clone()
            };

            let prompt_y = rows.saturating_sub(3);
            let cursor_x = (prompt_len + display_input.chars().count()) as u16;

            queue!(
                out,
                cursor::MoveTo(0, prompt_y),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(&display_input),
                SetForegroundColor(colors.date_color),
                Print(&hint),
                ResetColor,
                cursor::MoveTo(cursor_x, prompt_y),
                cursor::Show
            )?;
            out.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let is_alt = key.modifiers.contains(KeyModifiers::ALT);

            match key.code {
                KeyCode::Enter => {
                    hide_cursor(&mut out)?;
                    return Ok(Some(input));
                }
                KeyCode::Esc => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Char('c' | 'C') if is_ctrl => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Backspace => {
                    input.pop();
                    suggestion_idx = 0;
                }
                KeyCode::Char(c) if !is_ctrl && !is_alt => {
                    input.push(c);
                    suggestion_idx = 0;
                }
                KeyCode::Down if !matches.is_empty() => {
                    suggestion_idx = (suggestion_idx + 1) % matches.len();
                }
                KeyCode::Up if !matches.is_empty() => {
                    suggestion_idx = if suggestion_idx == 0 {
                        matches.len() - 1
                    } else {
                        suggestion_idx - 1
                    };
                }
                KeyCode::Right | KeyCode::Tab => {
                    if let Some(suggestion) = matched_suggestion {
                        let clean_suggestion = suggestion.trim_end_matches(';');
                        if let Some(last_comma_idx) = input.rfind(',') {
                            input.truncate(last_comma_idx + 1);
                            input.push(' ');
                            input.push_str(clean_suggestion);
                        } else {
                            input = clean_suggestion.to_string();
                        }
                        suggestion_idx = 0;
                    }
                }
                _ => {}
            }
        }
    }

    fn prompt_edit(&mut self, prompt_text: &str, initial_text: &str) -> io::Result<Option<String>> {
        let mut input = InputLine::new(initial_text);
        let mut out = stdout();
        let colors = self.ui_colors();
        let prompt_len = prompt_text.chars().count();

        loop {
            let (cols, rows) = term_size()?;
            let prompt_y = rows.saturating_sub(3);

            queue!(
                out,
                cursor::MoveTo(0, prompt_y),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(" "),
                Print(input.as_string()),
                ResetColor
            )?;

            draw_prompt_menus(&mut out, rows, cols, 6, &EMPTY_MENU, &CANCEL_MENU, &colors)?;

            let cursor_x = (prompt_len + 1 + input.cursor) as u16;
            queue!(out, cursor::MoveTo(cursor_x, prompt_y), cursor::Show)?;
            out.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

            match key.code {
                KeyCode::Char('c' | 'C') if is_ctrl => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Enter => {
                    hide_cursor(&mut out)?;
                    return Ok(Some(input.into_string()));
                }
                KeyCode::Esc => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Left => input.move_left(),
                KeyCode::Right => input.move_right(),
                KeyCode::Char('b' | 'B') if is_ctrl => input.move_left(),
                KeyCode::Char('f' | 'F') if is_ctrl => input.move_right(),
                KeyCode::Backspace => {
                    if !input.backspace() && input.is_empty() {
                        hide_cursor(&mut out)?;
                        return Ok(Some(String::new()));
                    }
                }
                KeyCode::Delete => input.delete(),
                KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META) =>
                    {
                        input.insert(c);
                    }
                _ => {}
            }
        }
    }

    fn prompt_for_folder(
        &mut self,
        prompt_text: &str,
        folders: &[String],
    ) -> io::Result<Option<String>> {
        let mut out = stdout();
        let mut input = String::new();
        let colors = self.ui_colors();
        let prompt_len = prompt_text.chars().count();
        let mut suggestion_idx = 0;

        loop {
            let (cols, rows) = term_size()?;
            let suggestions = find_folder_suggestions(&input, folders);
            if !suggestions.is_empty() {
                suggestion_idx %= suggestions.len();
            } else {
                suggestion_idx = 0;
            }

            let current_suggestion = suggestions.get(suggestion_idx);
            let hint = current_suggestion
                .map(|folder| {
                    format_suggestion_hint(&input, folder, suggestion_idx, suggestions.len())
                })
                .unwrap_or_default();

            let prompt_y = rows.saturating_sub(3);
            queue!(
                out,
                cursor::MoveTo(0, prompt_y),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(&input),
                SetForegroundColor(colors.date_color),
                Print(&hint),
                ResetColor
            )?;

            draw_prompt_menus(&mut out, rows, cols, 6, &EMPTY_MENU, &CANCEL_MENU, &colors)?;

            let cursor_x = (prompt_len + input.chars().count()) as u16;
            queue!(out, cursor::MoveTo(cursor_x, prompt_y), cursor::Show)?;
            out.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let is_alt = key.modifiers.contains(KeyModifiers::ALT);

            match key.code {
                KeyCode::Enter => {
                    hide_cursor(&mut out)?;
                    return Ok(Some(input));
                }
                KeyCode::Esc => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Char('c' | 'C') if is_ctrl => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Char(c) if !is_ctrl && !is_alt => {
                    input.push(c);
                    suggestion_idx = 0;
                }
                KeyCode::Backspace => {
                    input.pop();
                    suggestion_idx = 0;
                }
                KeyCode::Up if !suggestions.is_empty() => {
                    suggestion_idx = if suggestion_idx == 0 {
                        suggestions.len() - 1
                    } else {
                        suggestion_idx - 1
                    };
                }
                KeyCode::Down if !suggestions.is_empty() => {
                    suggestion_idx = (suggestion_idx + 1) % suggestions.len();
                }
                KeyCode::Right | KeyCode::Tab => {
                    if let Some(folder) = current_suggestion {
                        input = folder.clone();
                        suggestion_idx = 0;
                    }
                }
                _ => {}
            }
        }
    }

    fn prompt_select_item(
        &mut self,
        prompt_text: &str,
        items: &[String],
    ) -> io::Result<Option<String>> {
        if items.is_empty() {
            return Ok(None);
        }

        let mut out = stdout();
        let colors = self.ui_colors();
        let mut idx = 0;

        loop {
            let (cols, rows) = term_size()?;
            let indicator = if items.len() > 1 {
                format!(" ({}/{})", idx + 1, items.len())
            } else {
                String::new()
            };

            queue!(
                out,
                cursor::MoveTo(0, rows.saturating_sub(3)),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(" "),
                Print(&items[idx]),
                SetForegroundColor(colors.date_color),
                Print(indicator),
                ResetColor
            )?;

            draw_prompt_menus(
                &mut out,
                rows,
                cols,
                5,
                &[
                    ("   ^C", " Cancel"),
                    ("P", " Prev"),
                    ("", ""),
                    ("", ""),
                    ("", ""),
                    ("", ""),
                ],
                &[
                    ("Enter", " Select"),
                    ("N", " Next"),
                    ("", ""),
                    ("", ""),
                    ("", ""),
                    ("", ""),
                ],
                &colors,
            )?;

            queue!(out, cursor::Hide)?;
            out.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Enter => {
                    hide_cursor(&mut out)?;
                    return Ok(Some(items[idx].clone()));
                }
                KeyCode::Esc => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    hide_cursor(&mut out)?;
                    return Ok(None);
                }
                KeyCode::Up | KeyCode::Char('p' | 'P') => {
                    idx = if idx == 0 { items.len() - 1 } else { idx - 1 };
                }
                KeyCode::Down | KeyCode::Char('n' | 'N') => {
                    idx = (idx + 1) % items.len();
                }
                _ => {}
            }
        }
    }

    fn prompt_cancel(&mut self) -> io::Result<bool> {
        let mut out = stdout();
        let (cols, rows) = term_size()?;
        let colors = self.ui_colors();

        queue!(
            out,
            cursor::MoveTo(0, rows.saturating_sub(3)),
            SetBackgroundColor(colors.menu_bg),
            Clear(ClearType::UntilNewLine),
            SetForegroundColor(colors.accent),
            Print("Are you sure you want to cancel? "),
            ResetColor
        )?;
        draw_prompt_menus(&mut out, rows, cols, 6, &YES_MENU, &NO_MENU, &colors)?;
        out.flush()?;

        loop {
            if let Event::Key(pk) = event::read()? {
                if pk.kind != KeyEventKind::Press {
                    continue;
                }
                match pk.code {
                    KeyCode::Char('y' | 'Y') => return Ok(true),
                    KeyCode::Char('n' | 'N') | KeyCode::Esc => return Ok(false),
                    KeyCode::Char('c' | 'C') if pk.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(false);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn find_email_suggestions(input: &str, address_book: &[String]) -> Vec<String> {
    let last_part = input.split(',').last().unwrap_or("").trim_start();
    if last_part.is_empty() {
        return Vec::new();
    }

    let last_part_lower = last_part.to_lowercase();
    let search_term = last_part_lower.trim_start_matches('"');

    let mut matches = Vec::new();
    for addr in address_book {
        if addr.trim().is_empty() {
            continue;
        }

        if let Some((team_name, _)) = addr.split_once(':') {
            let team_name = team_name.trim();
            if team_name.to_lowercase().contains(search_term) {
                matches.push(format!("{} (Team)", team_name));
            }
        } else {
            let clean_lower = addr.to_lowercase();
            if clean_lower.trim_start_matches('"').contains(search_term) {
                matches.push(addr.clone());
            }
        }
    }

    matches
}

pub fn find_folder_suggestions(input: &str, folders: &[String]) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }

    let input_lower = input.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();

    for folder in folders {
        let lower = folder.to_lowercase();
        if lower.starts_with(&input_lower) {
            prefix_matches.push(folder.clone());
        } else if lower.contains(&input_lower) {
            substring_matches.push(folder.clone());
        }
    }

    prefix_matches.extend(substring_matches);
    prefix_matches
}