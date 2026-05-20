use crate::editor::{Editor, MenuState};
use crate::ui::{derive_ui_colors, UiExt}; // UiExt is needed if prompt uses draw_menu_line
use crossterm::{cursor, event::{self, Event, KeyCode, KeyModifiers, KeyEventKind},
                execute, queue, style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
                terminal::{self, ClearType, size as term_size}};
use std::io::{self, stdout, Write};
use crossterm::terminal::Clear;
use crate::config::UiColors;

pub trait PromptExt {
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>>;
    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>>;
    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>>;
    fn prompt_with_autocomplete(&mut self, prompt_text: &str, suggestions: &[String]) -> io::Result<Option<String>>;
    fn prompt_edit(&mut self, prompt_text: &str, initial_text: &str) -> io::Result<Option<String>>;
}

impl PromptExt for Editor {
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>> {
        let previous_state = self.menu_state;
        let mut input = String::new();
        let mut cursor_idx = 0;
        self.menu_state = if allow_browser { MenuState::PromptWithBrowser } else { MenuState::CancelOnly };

        loop {
            self.set_status(format!("{}{}", prompt_text, input));

            let mut stdout_handle = stdout();
            let (cols, rows) = term_size()?;
            let theme = &self.theme_set.themes[&self.current_theme];
            let ui_colors = derive_ui_colors(theme);

            // Draw the prompt text right at column 0 without leading blank padding
            queue!(
                stdout_handle,
                cursor::MoveTo(0, rows.saturating_sub(3)),
                SetBackgroundColor(ui_colors.ui_bg),
                terminal::Clear(ClearType::CurrentLine),
                SetForegroundColor(ui_colors.fg),
                Print(prompt_text),
                Print(&input),
                SetBackgroundColor(Color::Reset),
                SetForegroundColor(Color::Reset)
            )?;

            let col_width = (cols as usize / 6).max(1);
            if allow_browser {
                Self::draw_menu_line(
                    &mut stdout_handle,
                    rows.saturating_sub(2),
                    cols,
                    col_width,
                    &[("^T", " To Files"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_colors.ui_bg,
                    ui_colors.accent,
                    ui_colors.fg,
                )?;
            } else {
                Self::draw_menu_line(
                    &mut stdout_handle,
                    rows.saturating_sub(2),
                    cols,
                    col_width,
                    &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_colors.ui_bg,
                    ui_colors.accent,
                    ui_colors.fg,
                )?;
            }
            Self::draw_menu_line(
                &mut stdout_handle,
                rows.saturating_sub(1),
                cols,
                col_width,
                &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                ui_colors.ui_bg,
                ui_colors.accent,
                ui_colors.fg,
            )?;

            // Calculate cursor X position relative to the absolute margin start
            let prompt_len = prompt_text.chars().count();
            let input_chars_before_cursor = input.chars().take(cursor_idx).count();
            let cursor_x = prompt_len as u16 + input_chars_before_cursor as u16;

            queue!(
                stdout_handle,
                cursor::MoveTo(cursor_x, rows.saturating_sub(3)),
                cursor::Show
            )?;
            stdout_handle.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }

                let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let is_alt = key.modifiers.contains(KeyModifiers::ALT);

                match key.code {
                    KeyCode::Enter => {
                        queue!(stdout_handle, cursor::Hide)?;
                        stdout_handle.flush()?;
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(Some(input));
                    }
                    KeyCode::Esc => {
                        queue!(stdout_handle, cursor::Hide)?;
                        stdout_handle.flush()?;
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(None);
                    }
                    KeyCode::Char('c') | KeyCode::Char('g') if is_ctrl => {
                        queue!(stdout_handle, cursor::Hide)?;
                        stdout_handle.flush()?;
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(None);
                    }
                    KeyCode::Char('t') if allow_browser && is_ctrl => {
                        queue!(stdout_handle, cursor::Hide)?;
                        stdout_handle.flush()?;
                        if let Ok(Some(selected_file)) = self.run_file_browser(false) {
                            self.clear_status();
                            self.menu_state = previous_state;
                            return Ok(Some(selected_file));
                        }
                        self.menu_state = if allow_browser { MenuState::PromptWithBrowser } else { MenuState::CancelOnly };
                    }
                    KeyCode::Left => {
                        if cursor_idx > 0 { cursor_idx -= 1; }
                    }
                    KeyCode::Char('b') if is_ctrl => {
                        if cursor_idx > 0 { cursor_idx -= 1; }
                    }
                    KeyCode::Right => {
                        if cursor_idx < input.chars().count() { cursor_idx += 1; }
                    }
                    KeyCode::Char('f') if is_ctrl => {
                        if cursor_idx < input.chars().count() { cursor_idx += 1; }
                    }
                    KeyCode::Backspace => {
                        if cursor_idx > 0 {
                            let mut chars: Vec<char> = input.chars().collect();
                            chars.remove(cursor_idx - 1);
                            input = chars.into_iter().collect();
                            cursor_idx -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if cursor_idx < input.chars().count() {
                            let mut chars: Vec<char> = input.chars().collect();
                            chars.remove(cursor_idx);
                            input = chars.into_iter().collect();
                        }
                    }
                    KeyCode::Char(c) => {
                        if !is_ctrl && !is_alt {
                            let mut chars: Vec<char> = input.chars().collect();
                            chars.insert(cursor_idx, c);
                            input = chars.into_iter().collect();
                            cursor_idx += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn prompt_edit(&mut self, prompt_text: &str, initial_text: &str) -> io::Result<Option<String>> {
        let mut stdout = stdout();
        // Use a char vector for easy and safe cursor manipulation
        let mut input: Vec<char> = initial_text.chars().collect();
        let mut cursor_pos = input.len(); // Place cursor at the end
        let (cols, rows) = term_size()?;

        let theme = &self.theme_set.themes[&self.current_theme];
        let colors = derive_ui_colors(theme);

        loop {
            let input_str: String = input.iter().collect();
            queue!(
                stdout,
                cursor::MoveTo(0, rows - 3),
                SetBackgroundColor(colors.ui_bg),
                terminal::Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(&input_str)
            )?;

            // Move cursor directly to the edit position
            let prompt_len = prompt_text.chars().count();
            queue!(
                stdout,
                cursor::MoveTo((prompt_len + cursor_pos) as u16, rows - 3)
            )?;

            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Enter => return Ok(Some(input.into_iter().collect())),
                        KeyCode::Esc => return Ok(None),
                        KeyCode::Char('c') | KeyCode::Char('C') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),

                        // Navigation
                        KeyCode::Left => {
                            if cursor_pos > 0 { cursor_pos -= 1; }
                        }
                        KeyCode::Right => {
                            if cursor_pos < input.len() { cursor_pos += 1; }
                        }
                        KeyCode::Char('b') | KeyCode::Char('B') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if cursor_pos > 0 { cursor_pos -= 1; }
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if cursor_pos < input.len() { cursor_pos += 1; }
                        }

                        // Editing
                        KeyCode::Backspace => {
                            if input.is_empty() {
                                // If empty, Backspace instantly submits the empty string
                                // to clear the search and return to the main list
                                return Ok(Some(String::new()));
                            } else {
                                if cursor_pos > 0 {
                                    cursor_pos -= 1;
                                    input.remove(cursor_pos);
                                }
                            }
                        }
                        KeyCode::Char('<') => {
                            if input.is_empty() {
                                // If empty, `<` instantly returns to the main list
                                return Ok(Some(String::new()));
                            } else {
                                // [Your existing character insertion logic]
                                input.push('<');
                            }
                        }
                        // Standard Typing
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                            input.insert(cursor_pos, c);
                            cursor_pos += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn prompt_with_autocomplete(&mut self, prompt_text: &str, suggestions: &[String]) -> io::Result<Option<String>> {
        let mut stdout = stdout();
        let mut input = String::new();
        let (cols, rows) = term_size()?;

        let theme = &self.theme_set.themes[&self.current_theme];
        let colors = derive_ui_colors(theme);

        loop {
            // Calculate the current autocomplete hint
            let mut hint = String::new();
            if !input.is_empty() {
                let last_part = input.split(',').last().unwrap_or("").trim_start();
                if !last_part.is_empty() {
                    for addr in suggestions {
                        if addr.to_lowercase().starts_with(&last_part.to_lowercase()) {
                            hint = addr[last_part.len()..].to_string();
                            break;
                        }
                    }
                }
            }

            queue!(
                stdout,
                cursor::MoveTo(0, rows - 1),
                SetBackgroundColor(colors.ui_bg),
                terminal::Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(prompt_text),
                SetForegroundColor(colors.fg),
                Print(&input),
                SetForegroundColor(colors.date_color), // using date_color as a subtle grey for the hint
                Print(&hint),
                ResetColor
            )?;
            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Enter => return Ok(Some(input)),
                        KeyCode::Esc => return Ok(None),
                        KeyCode::Backspace => { input.pop(); },
                        KeyCode::Char(c) => input.push(c),
                        KeyCode::Right | KeyCode::Tab => {
                            if !hint.is_empty() {
                                input.push_str(&hint);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>> {
        let previous_state = self.menu_state; // Save the state
        self.menu_state = MenuState::ReplaceAction;
        self.set_status(String::from(prompt_text));
        self.draw_screen()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.clear_status();
                    self.menu_state = previous_state; // Restore state on cancel
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => { self.clear_status(); self.menu_state = previous_state; return Ok(Some('y')); }
                    KeyCode::Char('n') | KeyCode::Char('N') => { self.clear_status(); self.menu_state = previous_state; return Ok(Some('n')); }
                    KeyCode::Char('a') | KeyCode::Char('A') => { self.clear_status(); self.menu_state = previous_state; return Ok(Some('a')); }
                    _ => {}
                }
            }
        }
    }

    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>> {
        let previous_state = self.menu_state;
        self.menu_state = MenuState::YesNoCancel;
        self.set_status(String::from(prompt_text));

        let mut stdout_handle = stdout();
        let (cols, rows) = term_size()?;
        let theme = &self.theme_set.themes[&self.current_theme];
        let ui_colors = derive_ui_colors(theme);

        // Draw the prompt text right at column 0 without leading blank padding
        queue!(
            stdout_handle,
            cursor::MoveTo(0, rows.saturating_sub(3)),
            SetBackgroundColor(ui_colors.ui_bg),
            terminal::Clear(ClearType::CurrentLine),
            SetForegroundColor(ui_colors.fg),
            Print(prompt_text),
            SetBackgroundColor(Color::Reset),
            SetForegroundColor(Color::Reset)
        )?;

        // Draw the menu items on the bottom two lines
        let col_width = (cols as usize / 6).max(1);
        Self::draw_menu_line(
            &mut stdout_handle,
            rows.saturating_sub(2),
            cols,
            col_width,
            &[("^C", " Cancel"), ("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", "")],
            ui_colors.ui_bg,
            ui_colors.accent,
            ui_colors.fg,
        )?;
        Self::draw_menu_line(
            &mut stdout_handle,
            rows.saturating_sub(1),
            cols,
            col_width,
            &[("", ""), ("N", " No"), ("", ""), ("", ""), ("", ""), ("", "")],
            ui_colors.ui_bg,
            ui_colors.accent,
            ui_colors.fg,
        )?;

        stdout_handle.flush()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(Some(true));
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(Some(false));
                    }
                    KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
                        self.clear_status();
                        self.menu_state = previous_state;
                        return Ok(None);
                    }
                    _ => {}
                }
            }
        }
    }
}

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

