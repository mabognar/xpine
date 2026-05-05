use crate::editor::{Editor, MenuState};
use crate::config::ConfigExt;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    queue,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
};
use syntect::easy::HighlightLines;
use syntect::highlighting::Style;
use std::io::{self, stdout, Write};
use std::env;

pub trait UiExt {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, cols: u16, col_width: usize, items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()>;
    fn draw_screen(&mut self) -> io::Result<()>;
    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>>;
    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>>;
    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>>;
    fn run_file_browser(&mut self) -> io::Result<Option<String>>;
    fn show_help(&mut self) -> io::Result<()>;
    fn set_status(&mut self, message: String);
    fn clear_status(&mut self);
}

impl UiExt for Editor {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, cols: u16, col_width: usize, items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()> {
        queue!(writer, cursor::MoveTo(0, row), SetBackgroundColor(ui_bg))?;
        let mut printed = 0;
        for (cmd, desc) in items.iter() {
            let cmd_chars = cmd.chars().count();
            let desc_chars = desc.chars().count();
            let total_chars = cmd_chars + desc_chars;

            // Prevent division/layout panic if width is super tiny
            let safe_col_width = col_width.max(1);

            if total_chars <= safe_col_width {
                queue!(writer, SetForegroundColor(key_fg), Print(cmd), SetForegroundColor(text_fg), Print(format!("{}{}", desc, " ".repeat(safe_col_width - total_chars))))?;
            } else {
                queue!(writer, SetForegroundColor(key_fg), Print(cmd), SetForegroundColor(text_fg), Print(desc.chars().take(safe_col_width.saturating_sub(cmd_chars)).collect::<String>()))?;
            }
            printed += safe_col_width;
        }
        queue!(writer, Print(" ".repeat((cols as usize).saturating_sub(printed))), SetBackgroundColor(Color::Reset))?;
        Ok(())
    }

    fn draw_screen(&mut self) -> io::Result<()> {
        let mut stdout = stdout();
        let (cols, rows) = terminal::size()?;

        // Calculate visible rows factoring in the reserved top margin
        let visible_rows = rows.saturating_sub((4 + self.top_margin) as u16) as usize;

        let theme = &self.theme_set.themes[&self.current_theme];
        let is_dark = Self::is_dark_theme(theme);
        let raw_theme_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });

        let default_cross_bg = Color::Rgb { r: raw_theme_bg.r, g: raw_theme_bg.g, b: raw_theme_bg.b };
        let ui_bg = Self::derive_ui_color(raw_theme_bg, is_dark);
        let title_fg = if is_dark { Color::Reset } else { Color::Black };
        let menu_key_fg = if is_dark { Color::Rgb { r: 0, g: 150, b: 200 } } else { Color::Rgb { r: 0, g: 100, b: 200 } };
        let menu_text_fg = if is_dark { Color::Reset } else { Color::Black };
        let dollar_bg = if is_dark { Color::Rgb { r: 180, g: 180, b: 180 } } else { Color::Rgb { r: 80, g: 80, b: 80 } };
        let dollar_fg = if is_dark { Color::Black } else { Color::White };

        // --- TITLE BAR LOGIC ---
        // Hide the top title bar natively if acting as an email composer or reader
        if self.menu_state == MenuState::EmailComposer || self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::MoveTo(0, self.top_margin), SetBackgroundColor(default_cross_bg), terminal::Clear(ClearType::CurrentLine))?;
        } else {
            // Standard title bar rendering
            queue!(stdout, cursor::MoveTo(0, self.top_margin), SetBackgroundColor(ui_bg))?;

            let title = "   xnano";
            let file_display_string = match self.filename.as_deref() {
                Some(name) => {
                    let path = std::path::Path::new(name);
                    if path.is_absolute() { name.to_string() } else if let Ok(cwd) = env::current_dir() { cwd.join(path).to_string_lossy().into_owned() } else { name.to_string() }
                }
                None => String::from("New Buffer"),
            };
            let file_section = format!("     {}", file_display_string);
            let right_indicator_len = if self.is_modified { "[ Modified ]   ".len() } else { 0 };
            let max_allowable_len = (cols as usize).saturating_sub(right_indicator_len);
            let full_len = title.chars().count() + file_section.chars().count();

            let mut final_file_section = file_section.clone();
            if full_len > max_allowable_len {
                let allowed_file_len = max_allowable_len.saturating_sub(title.chars().count());
                if allowed_file_len > 3 {
                    final_file_section = file_section.chars().take(allowed_file_len.saturating_sub(3)).collect();
                    final_file_section.push_str("...");
                } else { final_file_section = String::new(); }
            }

            let printed_left_len = title.chars().count() + final_file_section.chars().count();

            if self.is_modified {
                let right = "[ Modified ]   ";
                queue!(stdout, SetForegroundColor(menu_key_fg), Print(title), SetForegroundColor(title_fg), Print(&final_file_section), Print(" ".repeat((cols as usize).saturating_sub(printed_left_len + right.len()))), SetForegroundColor(title_fg), Print(right), SetForegroundColor(Color::Reset), SetBackgroundColor(Color::Reset))?;
            } else {
                queue!(stdout, SetForegroundColor(menu_key_fg), Print(title), SetForegroundColor(title_fg), Print(&final_file_section), Print(" ".repeat((cols as usize).saturating_sub(printed_left_len))), SetForegroundColor(Color::Reset), SetBackgroundColor(Color::Reset))?;
            }
        }

        // --- SYNTAX HIGHLIGHTING & LINE RENDERING ---
        let syntax = if let Some(ref name) = self.filename {
            let path = std::path::Path::new(name);
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) { self.syntax_set.find_syntax_by_extension(ext).unwrap_or_else(|| self.syntax_set.find_syntax_plain_text()) } else { self.syntax_set.find_syntax_plain_text() }
        } else { self.syntax_set.find_syntax_plain_text() };

        let max_line_num_len = self.buffer.len_lines().to_string().len();
        let gutter_width = if self.show_line_numbers { max_line_num_len + 1 } else { 0 };
        let available_width = std::cmp::max(1, (cols as usize).saturating_sub(gutter_width));
        let cursor_absolute = self.get_cursor_char_idx();
        let mark_range = self.mark.map(|m| { if m < cursor_absolute { (m, cursor_absolute) } else { (cursor_absolute, m) } });

        let mut last_fg: Option<Color> = None;
        let mut last_bg: Option<Color> = None;
        let mut fallback_highlighter = None;

        let mut terminal_y = 0;
        let mut file_y = self.row_offset;

        while terminal_y < visible_rows {
            if file_y < self.buffer.len_lines() {
                if !self.highlight_cache.contains_key(&file_y) {
                    if fallback_highlighter.is_none() { fallback_highlighter = Some(HighlightLines::new(syntax, theme)); }

                    // Fixed Lifetime Issue: Bind the string to a variable before passing it to the highlighter
                    let line_str = self.buffer.line(file_y).to_string();
                    let ranges = fallback_highlighter.as_mut().unwrap().highlight_line(&line_str, &self.syntax_set).unwrap();

                    self.highlight_cache.insert(file_y, ranges.into_iter().map(|(s, t)| (s, t.to_string())).collect());
                }

                let ranges = self.highlight_cache.get(&file_y).unwrap();
                let mut visual_x = 0;
                let mut line_char_idx = 0;
                let line_has_search_highlight = self.highlight_match.map_or(false, |(h_y, _, _)| h_y == file_y);

                // Add top_margin to all visual line positioning
                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin + 1))?;
                if self.show_line_numbers {
                    if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                    if last_fg != Some(menu_key_fg) { queue!(stdout, SetForegroundColor(menu_key_fg))?; last_fg = Some(menu_key_fg); }
                    queue!(stdout, Print(format!("{:>width$} ", file_y + 1, width = max_line_num_len)))?;
                }

                let mut printed_on_current_line = 0;

                'char_loop: for (style, text) in ranges {
                    let syn_color = style.foreground; let cross_color = Color::Rgb { r: syn_color.r, g: syn_color.g, b: syn_color.b };
                    let syn_bg = style.background; let cross_bg = Color::Rgb { r: syn_bg.r, g: syn_bg.g, b: syn_bg.b };

                    if last_fg != Some(cross_color) { queue!(stdout, SetForegroundColor(cross_color))?; last_fg = Some(cross_color); }
                    if last_bg != Some(cross_bg) { queue!(stdout, SetBackgroundColor(cross_bg))?; last_bg = Some(cross_bg); }

                    for ch in text.chars() {
                        if ch == '\n' || ch == '\r' { line_char_idx += 1; continue; }
                        let absolute_char_idx = self.buffer.line_to_char(file_y) + line_char_idx;

                        let is_highlighted = if line_has_search_highlight {
                            if let Some((_, h_start, h_end)) = self.highlight_match { line_char_idx >= h_start && line_char_idx < h_end } else { false }
                        } else if let Some((m_start, m_end)) = mark_range { absolute_char_idx >= m_start && absolute_char_idx < m_end } else { false };

                        let display_chars = if ch == '\t' { vec![' '; 4 - (visual_x % 4)] } else { vec![ch] };

                        for display_ch in display_chars {
                            if self.soft_wrap {
                                if printed_on_current_line >= available_width {
                                    if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                                    queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;
                                    terminal_y += 1;
                                    if terminal_y >= visible_rows { break 'char_loop; }

                                    // Offset Top margin on wrapped lines
                                    queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin + 1))?;
                                    if self.show_line_numbers { queue!(stdout, Print(" ".repeat(gutter_width)))?; }
                                    if last_fg != Some(cross_color) { queue!(stdout, SetForegroundColor(cross_color))?; last_fg = Some(cross_color); }
                                    if last_bg != Some(cross_bg) { queue!(stdout, SetBackgroundColor(cross_bg))?; last_bg = Some(cross_bg); }
                                    printed_on_current_line = 0;
                                }

                                if is_highlighted {
                                    if last_bg != Some(Color::Red) { queue!(stdout, SetBackgroundColor(Color::Red))?; last_bg = Some(Color::Red); }
                                    if last_fg != Some(Color::White) { queue!(stdout, SetForegroundColor(Color::White))?; last_fg = Some(Color::White); }
                                }
                                queue!(stdout, Print(display_ch))?;
                                if is_highlighted {
                                    if last_bg != Some(cross_bg) { queue!(stdout, SetBackgroundColor(cross_bg))?; last_bg = Some(cross_bg); }
                                    if last_fg != Some(cross_color) { queue!(stdout, SetForegroundColor(cross_color))?; last_fg = Some(cross_color); }
                                }
                                printed_on_current_line += 1; visual_x += 1;
                            } else {
                                if visual_x >= self.col_offset && printed_on_current_line < available_width {
                                    if is_highlighted {
                                        if last_bg != Some(Color::Red) { queue!(stdout, SetBackgroundColor(Color::Red))?; last_bg = Some(Color::Red); }
                                        if last_fg != Some(Color::White) { queue!(stdout, SetForegroundColor(Color::White))?; last_fg = Some(Color::White); }
                                    }
                                    queue!(stdout, Print(display_ch))?;
                                    if is_highlighted {
                                        if last_bg != Some(cross_bg) { queue!(stdout, SetBackgroundColor(cross_bg))?; last_bg = Some(cross_bg); }
                                        if last_fg != Some(cross_color) { queue!(stdout, SetForegroundColor(cross_color))?; last_fg = Some(cross_color); }
                                    }
                                    printed_on_current_line += 1;
                                }
                                visual_x += 1;
                            }
                        }
                        line_char_idx += 1;
                    }
                }

                if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;

                if !self.soft_wrap {
                    if self.col_offset > 0 {
                        if last_bg != Some(dollar_bg) { queue!(stdout, SetBackgroundColor(dollar_bg))?; last_bg = Some(dollar_bg); }
                        if last_fg != Some(dollar_fg) { queue!(stdout, SetForegroundColor(dollar_fg))?; last_fg = Some(dollar_fg); }
                        queue!(stdout, cursor::MoveTo(gutter_width as u16, terminal_y as u16 + self.top_margin + 1), Print('$'))?;
                    }
                    if visual_x > self.col_offset + available_width {
                        if last_bg != Some(dollar_bg) { queue!(stdout, SetBackgroundColor(dollar_bg))?; last_bg = Some(dollar_bg); }
                        if last_fg != Some(dollar_fg) { queue!(stdout, SetForegroundColor(dollar_fg))?; last_fg = Some(dollar_fg); }
                        queue!(stdout, cursor::MoveTo(cols - 1, terminal_y as u16 + self.top_margin + 1), Print('$'))?;
                    }
                }
                if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                if last_fg != Some(Color::Reset) { queue!(stdout, SetForegroundColor(Color::Reset))?; last_fg = Some(Color::Reset); }

            } else {
                // Top margin for empty EOF lines
                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin + 1))?;
                if self.show_line_numbers {
                    if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                    queue!(stdout, Print(" ".repeat(gutter_width)))?;
                }
                if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;
            }
            terminal_y += 1; file_y += 1;
        }

        // --- STATUS BAR ---
        queue!(stdout, cursor::MoveTo(0, rows - 3))?;
        if !self.status_message.is_empty() {
            queue!(stdout, SetBackgroundColor(ui_bg), SetForegroundColor(title_fg))?;
            let mut printed_len = 0;

            if self.menu_state == MenuState::SpellCheck {
                if !self.current_suggestions.is_empty() {
                    for (i, sug) in self.current_suggestions.iter().enumerate() {
                        let num_str = format!("{}", i + 1);
                        queue!(stdout, SetForegroundColor(menu_key_fg), Print(&num_str), SetForegroundColor(title_fg), Print(format!(" {}   ", sug)))?;
                        printed_len += num_str.len() + 1 + sug.len() + 3;
                    }
                } else {
                    queue!(stdout, Print("No suggestions   "))?; printed_len += "No suggestions   ".len();
                }
            }

            queue!(stdout, Print(&self.status_message))?; printed_len += self.status_message.len();
            queue!(stdout, Print(" ".repeat((cols as usize).saturating_sub(printed_len))), SetBackgroundColor(Color::Reset), SetForegroundColor(Color::Reset))?;
        } else {
            queue!(stdout, SetBackgroundColor(default_cross_bg), terminal::Clear(ClearType::CurrentLine))?;
        }

        // --- DYNAMIC MENUS (Standardized 12-Item Grid) ---
        let col_width = ((cols as usize) / 6).max(1);

        match self.menu_state {
            MenuState::EmailComposer => {
                let menu1 = [("^X", " Send"), ("^O", " Write Out"), ("^R", " Read File"), ("^Y", " Prev Pg"), ("^K", " Cut Txt"), ("^C", " Cancel")];
                let menu2 = [("^J", " Justify"), ("^W", " Where Is"), ("^V", " Next Pg"), ("^U", if self.is_justified { " Unjustify" } else { " UnCut" }), ("^T", " To Spell"), ("", "")];
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::EmailReader => {
                let menu1 = [("<", " Back"), ("R", " Reply"), ("F", " Forward"), ("^Y", " Prev Pg"), ("^V", " Next Pg"), ("", "")];
                let menu2 = [("P", " Prev"), ("N", " Next"), ("Home", " Top"), ("End", " Bottom"), ("", ""), ("", "")];
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::Default => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("^G", " Get Help"), ("^O", " Write Out"), ("^R", " Read File"), ("^Y", " Prev Pg"), ("^K", " Cut Txt"), ("^C", " Cur Pos")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("^X", " Exit"), ("^J", " Justify"), ("^W", " Where Is"), ("^V", " Next Pg"), ("^U", if self.is_justified { " Unjustify" } else { " UnCut" }), ("^T", " To Spell")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::YesNoCancel => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::ReplaceAction => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("Y", " Yes"), ("A", " All"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::PromptWithBrowser => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("^T", " To Files"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::CancelOnly => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::SpellCheck => {
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("I", " Ignore"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("A", " Add Word"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            }
        }

        let visual_x = self.get_visual_cursor_x();
        let display_x = visual_x.saturating_sub(self.col_offset);
        let final_cursor_x = if self.soft_wrap { gutter_width as u16 + display_x as u16 % available_width as u16 } else { gutter_width as u16 + display_x as u16 };

        // Final cursor placement calculation factoring in top_margin
        let final_cursor_y = if self.soft_wrap {
            let mut virtual_y = 0;
            for i in self.row_offset..self.cursor_y {
                let w = self.get_visual_line_width(i);
                virtual_y += if w == 0 { 1 } else { (w - 1) / available_width + 1 };
            }
            virtual_y += visual_x / available_width;
            virtual_y as u16 + self.top_margin + 1
        } else {
            self.cursor_y.saturating_sub(self.row_offset) as u16 + self.top_margin + 1
        };

        // Hide cursor completely in EmailReader mode
        if self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::Hide)?;
        } else {
            queue!(stdout, cursor::Show, cursor::MoveTo(final_cursor_x, final_cursor_y))?;
        }

        stdout.flush()?;
        Ok(())
    }

    fn prompt(&mut self, prompt_text: &str, allow_browser: bool) -> io::Result<Option<String>> {
        let mut input = String::new();
        self.menu_state = if allow_browser { MenuState::PromptWithBrowser } else { MenuState::CancelOnly };
        loop {
            self.set_status(format!("{}{}", prompt_text, input));
            self.draw_screen()?;
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.clear_status();
                    self.menu_state = MenuState::Default;
                    return Ok(None);
                } else if allow_browser && key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if let Ok(Some(selected_file)) = self.run_file_browser() {
                        self.menu_state = MenuState::Default;
                        self.clear_status();
                        return Ok(Some(selected_file));
                    } else {
                        self.menu_state = MenuState::PromptWithBrowser;
                    }
                } else {
                    // Properly nested match block to fix E0369
                    match key.code {
                        KeyCode::Enter => {
                            self.clear_status();
                            self.menu_state = MenuState::Default;
                            return Ok(Some(input));
                        }
                        KeyCode::Backspace => { input.pop(); }
                        KeyCode::Char(c) => { input.push(c); }
                        _ => {}
                    }
                }
            }
        }
    }

    fn prompt_yn(&mut self, prompt_text: &str) -> io::Result<Option<bool>> {
        self.menu_state = MenuState::YesNoCancel;
        self.set_status(String::from(prompt_text));
        self.draw_screen()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.clear_status();
                    self.menu_state = MenuState::Default;
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => { self.clear_status(); self.menu_state = MenuState::Default; return Ok(Some(true)); }
                    KeyCode::Char('n') | KeyCode::Char('N') => { self.clear_status(); self.menu_state = MenuState::Default; return Ok(Some(false)); }
                    _ => {}
                }
            }
        }
    }

    fn prompt_replace(&mut self, prompt_text: &str) -> io::Result<Option<char>> {
        self.menu_state = MenuState::ReplaceAction;
        self.set_status(String::from(prompt_text));
        self.draw_screen()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press { continue; }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.clear_status();
                    self.menu_state = MenuState::Default;
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => { self.clear_status(); self.menu_state = MenuState::Default; return Ok(Some('y')); }
                    KeyCode::Char('n') | KeyCode::Char('N') => { self.clear_status(); self.menu_state = MenuState::Default; return Ok(Some('n')); }
                    KeyCode::Char('a') | KeyCode::Char('A') => { self.clear_status(); self.menu_state = MenuState::Default; return Ok(Some('a')); }
                    _ => {}
                }
            }
        }
    }

    fn run_file_browser(&mut self) -> io::Result<Option<String>> {
        // File browser for xnano is currently disabled in favor of the email app's attachment browser
        Ok(None)
    }

    fn show_help(&mut self) -> io::Result<()> {
        let help_text = vec![
            "xnano help", "----------", "^G (F1) Get Help", "^X (F2) Exit", "^O (F3) Write Out",
            "^J (F4) Justify", "^R (F5) Read File", "^W (F6) Search", "^Y (F7) Prev Pg", "^V (F8) Next Pg",
            "Press any key to return..."
        ];
        let mut stdout = stdout();
        let (_cols, rows) = terminal::size()?;

        queue!(stdout, terminal::Clear(ClearType::All))?;
        for (i, line) in help_text.iter().enumerate() {
            if i >= (rows as usize).saturating_sub(1) { break; }
            queue!(stdout, cursor::MoveTo(0, i as u16), Print(*line))?;
        }
        stdout.flush()?;

        loop {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press { break; }
            }
        }
        Ok(())
    }

    fn set_status(&mut self, message: String) {
        self.status_message = message;
        self.status_time = Some(std::time::Instant::now());
    }

    fn clear_status(&mut self) {
        self.status_message.clear();
        self.status_time = None;
    }
}