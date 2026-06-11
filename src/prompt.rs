use crate::editor::{Editor, MenuState};
use crate::theme::{derive_ui_colors};
use crate::ui::UiExt; 
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
    fn prompt_for_folder(&mut self, prompt_text: &str, folders: &[String]) -> io::Result<Option<String>>;
}

impl PromptExt for Editor {
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>> {
        let previous_state = self.menu_state;
        let mut input = String::new();
        let mut cursor_idx = 0;

        if self.menu_state != MenuState::SpellCheck {
            self.menu_state = if allow_browser { MenuState::PromptWithBrowser } else { MenuState::CancelOnly };
        }

        loop {
            self.set_status(format!("{}{}", prompt_text, input));

            let mut stdout_handle = stdout();
            let (cols, rows) = term_size()?;
            let theme = &self.theme_set.themes[&self.current_theme];
            let ui_colors = derive_ui_colors(theme);

            queue!(
                stdout_handle,
                cursor::MoveTo(0, rows.saturating_sub(3)),
                SetBackgroundColor(ui_colors.menu_bg),
                Clear(ClearType::CurrentLine),
                SetForegroundColor(ui_colors.fg),
                Print(prompt_text),
                Print(&input),
                SetBackgroundColor(Color::Reset),
                SetForegroundColor(Color::Reset)
            )?;

            let col_width = (cols as usize / 5).max(1);

            if self.menu_state == MenuState::SpellCheck {
                let s1 = self.current_suggestions.get(0).cloned().unwrap_or_default();
                let s2 = self.current_suggestions.get(1).cloned().unwrap_or_default();
                let s3 = self.current_suggestions.get(2).cloned().unwrap_or_default();
                let s4 = self.current_suggestions.get(3).cloned().unwrap_or_default();
                let s5 = self.current_suggestions.get(4).cloned().unwrap_or_default();

                Self::draw_menu_line(
                    &mut stdout_handle, rows.saturating_sub(2), cols, col_width,
                    &[("1 ", if s1.is_empty() { "" } else { s1.as_str() }),
                        ("2 ", if s2.is_empty() { "" } else { s2.as_str() }),
                        ("3 ", if s3.is_empty() { "" } else { s3.as_str() }),
                        ("4 ", if s4.is_empty() { "" } else { s4.as_str() }),
                        ("5 ", if s5.is_empty() { "" } else { s5.as_str() })],
                    ui_colors.menu_bg, ui_colors.accent, ui_colors.fg,
                )?;
                Self::draw_menu_line(
                    &mut stdout_handle, rows.saturating_sub(1), cols, col_width,
                    &[("^C", " Cancel"), ("I", " Ignore"), ("A", " Add Dict")],
                    ui_colors.menu_bg, ui_colors.accent, ui_colors.fg,
                )?;
            } else {
                if allow_browser {
                    Self::draw_menu_line(
                        &mut stdout_handle, rows.saturating_sub(2), cols, col_width,
                        &[("^T", " To Files"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                        ui_colors.menu_bg, ui_colors.accent, ui_colors.fg,
                    )?;
                } else {
                    Self::draw_menu_line(
                        &mut stdout_handle, rows.saturating_sub(2), cols, col_width,
                        &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                        ui_colors.menu_bg, ui_colors.accent, ui_colors.fg,
                    )?;
                }
                Self::draw_menu_line(
                    &mut stdout_handle, rows.saturating_sub(1), cols, col_width,
                    &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_colors.menu_bg, ui_colors.accent, ui_colors.fg,
                )?;
            }

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
                if key.kind != KeyEventKind::Press { continue; }

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
                        if let Ok(Some(selected_file)) = self.run_file_browser(false, None) {
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
            SetBackgroundColor(ui_colors.menu_bg),
            Clear(ClearType::CurrentLine),
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
            &[("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
            ui_colors.menu_bg,
            ui_colors.accent,
            ui_colors.fg,
        )?;
        Self::draw_menu_line(
            &mut stdout_handle,
            rows.saturating_sub(1),
            cols,
            col_width,
            &[("N", " No"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
            ui_colors.menu_bg,
            ui_colors.accent,
            ui_colors.fg,
        )?;

        stdout_handle.flush()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }
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

    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>> {
        let previous_state = self.menu_state; // Save the state
        self.menu_state = MenuState::ReplaceAction;
        self.set_status(String::from(prompt_text));
        self.draw_screen()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }
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

    fn prompt_with_autocomplete(&mut self, prompt_text: &str, suggestions: &[String]) -> io::Result<Option<String>> {
        let mut stdout = stdout();
        let mut input = String::new();
        let (_, rows) = term_size()?;

        execute!(stdout, cursor::Show)?;
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
            cursor::MoveTo(0, rows - 3),
            SetBackgroundColor(colors.menu_bg),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(colors.fg),
            Print(prompt_text),
            Print(&input),
            SetForegroundColor(colors.date_color),
            Print(&hint),
            ResetColor
        )?;

            // 3. Move the cursor to the end of the USER INPUT (not the end of the hint)
            let cursor_x = prompt_text.len() as u16 + input.len() as u16;
            queue!(stdout, cursor::MoveTo(cursor_x, rows - 3))?;

            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Enter => {
                            execute!(stdout, cursor::Hide)?; // Hide cursor before returning
                            return Ok(Some(input));
                        },
                        KeyCode::Esc => {
                            execute!(stdout, cursor::Hide)?; // Hide cursor before returning
                            return Ok(None);
                        },
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

    fn prompt_edit(&mut self, prompt_text: &str, initial_text: &str) -> io::Result<Option<String>> {
        // Use a Vec<char> for safe, character-accurate insertions and deletions
        let mut input: Vec<char> = initial_text.chars().collect();
        let mut cursor_idx = input.len(); // Start cursor at the end of the text

        let (cols, rows) = terminal::size()?;

        let theme = self.theme_set.themes.get(&self.current_theme).expect("Theme not found");
        let colors = derive_ui_colors(theme);

        loop {
            let prompt_y = rows.saturating_sub(3);

            // Reconstruct the string for display
            let input_str: String = input.iter().collect();

            let display_str = format!("{} {}", prompt_text, input_str);
            let pad_len = (cols as usize).saturating_sub(display_str.chars().count());
            let padded_str = format!("{}{}", display_str, " ".repeat(pad_len));

            queue!(
                stdout(),
                cursor::MoveTo(0, prompt_y),
                SetBackgroundColor(colors.menu_bg),
                SetForegroundColor(colors.fg),
                Print(padded_str),
                ResetColor
            )?;

            let (_, rows) = terminal::size().unwrap_or((80, 24));

            // We queue the commands to temporarily overwrite the menu
            queue!(
                stdout(),
                cursor::SavePosition, // 1. Save where the typing cursor currently is!

                SetBackgroundColor(colors.menu_bg),
                SetForegroundColor(colors.fg),

                // 2. Move to the menu area (Assuming a standard 2-line menu at the bottom)
                cursor::MoveTo(0, rows - 2),
                Clear(ClearType::CurrentLine),
                cursor::MoveTo(0, rows - 1),
                Clear(ClearType::CurrentLine),

                // 3. Draw the contextual menu
                cursor::MoveTo(0, rows - 1),
                // You can wrap these in your app's theme colors if you pass them into the function!
                Print("^C"),
                Print(" Cancel"),

                // 4. Put the cursor back exactly where it was so the user can type
                cursor::RestorePosition,
                ResetColor,
            )?;

            // Flush to make sure it draws to the screen before the blocking loop starts
            stdout().flush()?;

            // Calculate cursor position based on the INTERNAL index, not the total length
            let prompt_len = prompt_text.chars().count();
            let cursor_x = (prompt_len + 1 + cursor_idx) as u16;

            queue!(
                stdout(),
                cursor::MoveTo(cursor_x, prompt_y),
                cursor::Show
            )?;

            stdout().flush()?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('c') | KeyCode::Char('C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    KeyCode::Enter => {
                        queue!(stdout(), cursor::Hide, ResetColor)?;
                        stdout().flush()?;
                        return Ok(Some(input.into_iter().collect())); // Convert back to String
                    }
                    KeyCode::Esc => {
                        queue!(stdout(), cursor::Hide, ResetColor)?;
                        stdout().flush()?;
                        return Ok(None);
                    }
                    // --- Navigation ---
                    KeyCode::Left => {
                        if cursor_idx > 0 { cursor_idx -= 1; }
                    }
                    KeyCode::Right => {
                        if cursor_idx < input.len() { cursor_idx += 1; }
                    }
                    // Ctrl+B (Backward) and Ctrl+F (Forward)
                    KeyCode::Char('b') | KeyCode::Char('B') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if cursor_idx > 0 { cursor_idx -= 1; }
                    }
                    KeyCode::Char('f') | KeyCode::Char('F') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if cursor_idx < input.len() { cursor_idx += 1; }
                    }
                    KeyCode::Backspace => {
                        if cursor_idx > 0 {
                            cursor_idx -= 1;
                            input.remove(cursor_idx);
                        } else if input.is_empty() {
                            queue!(stdout(), cursor::Hide, ResetColor)?;
                            stdout().flush()?;
                            return Ok(Some(String::new()));
                        }
                    }
                    KeyCode::Delete => {
                        if cursor_idx < input.len() {
                            input.remove(cursor_idx); // Removes character at current cursor
                        }
                    }
                    // --- Insertion ---
                    KeyCode::Char(c) => {
                        // Prevent Ctrl modifiers from accidentally typing text
                        if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META) {
                            input.insert(cursor_idx, c);
                            cursor_idx += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn prompt_for_folder(&mut self, prompt_text: &str, folders: &[String]) -> io::Result<Option<String>> {
        let mut stdout = stdout();
        let mut input = String::new();
        let (cols, rows) = term_size()?;

        let theme = &self.theme_set.themes[&self.current_theme];
        let colors = derive_ui_colors(theme);

        // Calculate standard column width for your menu items
        let col_width = (cols as usize / 6).max(1);

        let mut suggestion_idx = 0;

        loop {
            let suggestions = find_folder_suggestions(&input, folders);
            if !suggestions.is_empty() {
                suggestion_idx %= suggestions.len();
            } else {
                suggestion_idx = 0;
            }

            let current_suggestion = suggestions.get(suggestion_idx);

            // Format the hint appropriately
            let hint = if let Some(folder) = current_suggestion {
                let match_indicator = if suggestions.len() > 1 {
                    format!(" ({}/{})", suggestion_idx + 1, suggestions.len())
                } else {
                    String::new()
                };

                if folder.to_lowercase().starts_with(&input.to_lowercase()) {
                    // Prefix match: inline remainder + indicator
                    format!("{}{}", &folder[input.len()..], match_indicator)
                } else {
                    // Substring match: show full path + indicator
                    format!("  -> {}{}", folder, match_indicator)
                }
            } else {
                String::new()
            };

            // 1. Draw the prompt and input text (moved up to row - 3)
            queue!(
                stdout,
                cursor::MoveTo(0, rows.saturating_sub(3)),
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

            // 2. Draw the upper menu line (blank)
            Self::draw_menu_line(
                &mut stdout, rows.saturating_sub(2), cols, col_width,
                &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg,
            )?;

            // 3. Draw the lower menu line with just the Cancel command
            Self::draw_menu_line(
                &mut stdout, rows.saturating_sub(1), cols, col_width,
                &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg,
            )?;

            // 4. Place the cursor securely at the end of the user's typed input
            let prompt_len = prompt_text.chars().count();
            let input_len = input.chars().count();
            let cursor_x = (prompt_len + input_len) as u16;

            queue!(
                stdout,
                cursor::MoveTo(cursor_x, rows.saturating_sub(3)),
                cursor::Show
            )?;

            stdout.flush()?;

            // 5. Handle Keyboard Events
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Enter => {
                            queue!(stdout, cursor::Hide, ResetColor)?;
                            stdout.flush()?;
                            return Ok(Some(input));
                        }
                        KeyCode::Esc => {
                            queue!(stdout, cursor::Hide, ResetColor)?;
                            stdout.flush()?;
                            return Ok(None);
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            queue!(stdout, cursor::Hide, ResetColor)?;
                            stdout.flush()?;
                            // suggestion_idx = 0;
                            return Ok(None);
                        }
                        // KeyCode::Backspace => {
                        //     input.pop();
                        //     suggestion_idx = 0;
                        // },
                        // KeyCode::Char(c) => {
                        //     if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META) {
                        //         input.push(c);
                        //     }
                        // },
                        KeyCode::Char(c) => {
                            input.push(c);
                            suggestion_idx = 0; // Reset back to first match on typing
                        }
                        KeyCode::Backspace => {
                            input.pop();
                            suggestion_idx = 0; // Reset back to first match on deleting
                        }
                        KeyCode::Up => {
                            if !suggestions.is_empty() {
                                suggestion_idx = if suggestion_idx == 0 { suggestions.len() - 1 } else { suggestion_idx - 1 };
                            }
                        }
                        KeyCode::Down => {
                            if !suggestions.is_empty() {
                                suggestion_idx = (suggestion_idx + 1) % suggestions.len();
                            }
                        }
                        KeyCode::Right | KeyCode::Tab => {
                            if let Some(folder) = current_suggestion {
                                input = folder.clone();
                                suggestion_idx = 0; // Reset index after completing
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub fn prompt_cancel(stdout: &mut io::Stdout, colors: &UiColors) -> bool {
    let (cols, rows) = term_size().unwrap_or((80, 24));

    execute!(
        stdout,
        cursor::MoveTo(0, rows.saturating_sub(3)),
        SetBackgroundColor(colors.menu_bg),
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
        colors.menu_bg,
        colors.accent,
        colors.fg,
    ).unwrap();
    Editor::draw_menu_line(
        stdout,
        rows.saturating_sub(1),
        cols,
        col_width,
        &[("N", " No"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
        colors.menu_bg,
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

pub fn find_email_suggestions(input: &str, address_book: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    if input.is_empty() { return matches; }

    let last_part = input.split(',').last().unwrap_or("").trim_start();
    if last_part.is_empty() { return matches; }

    let last_part_lower = last_part.to_lowercase();

    for addr in address_book {
        if addr.trim().is_empty() { continue; } // Skip empty spacer lines

        // Extract the searchable portion: just the team name if it's a team,
        // or the full email address if it's an individual.
        let searchable_part = if let Some((team_name, _)) = addr.split_once(':') {
            team_name.trim()
        } else {
            addr.as_str()
        };

        // Match against the isolated searchable part
        if searchable_part.to_lowercase().starts_with(&last_part_lower) {
            matches.push(addr.clone()); // Still return the full string for insertion
        }
    }

    matches
}

pub fn find_folder_suggestions(input: &str, folders: &[String]) -> Vec<String> {
    let mut matches = Vec::new();
    if input.is_empty() { return matches; }

    let input_lower = input.to_lowercase();

    // 1. Try exact prefix matches first
    for folder in folders {
        if folder.to_lowercase().starts_with(&input_lower) {
            matches.push(folder.clone());
        }
    }

    // 2. Fallback to substring matches (exclude ones we already added)
    for folder in folders {
        if !folder.to_lowercase().starts_with(&input_lower) && folder.to_lowercase().contains(&input_lower) {
            matches.push(folder.clone());
        }
    }

    matches
}

