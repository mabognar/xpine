use crate::app::{App, AppMode};
use crate::editor::{Editor, MenuState};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType, size as term_size},
};
use syntect::easy::HighlightLines;
use std::io::{self, stdout, Write};
use std::env;
use crate::config::ConfigExt;

#[derive(Clone, Copy)]
pub struct UiColors {
    pub bg: Color,
    pub fg: Color,
    pub ui_bg: Color,
    pub selected_bg: Color,
    pub accent: Color,
    pub date_color: Color,
    pub flag_n: Color,
    pub flag_d: Color,
    pub flag_a: Color,
    pub flag_star: Color,
    pub is_dark: bool,
}

pub fn derive_ui_colors(theme: &syntect::highlighting::Theme) -> UiColors {
    let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
    let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });

    let bg = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
    let fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
    let is_dark = (raw_bg.r as u32 + raw_bg.g as u32 + raw_bg.b as u32) < 384;

    let ui_bg = if is_dark {
        Color::Rgb { r: raw_bg.r.saturating_add(20), g: raw_bg.g.saturating_add(20), b: raw_bg.b.saturating_add(20) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(20), g: raw_bg.g.saturating_sub(20), b: raw_bg.b.saturating_sub(20) }
    };

    let selected_bg = if raw_bg.r < 128 {
        Color::Rgb { r: raw_bg.r.saturating_add(40), g: raw_bg.g.saturating_add(40), b: raw_bg.b.saturating_add(40) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(40), g: raw_bg.g.saturating_sub(40), b: raw_bg.b.saturating_sub(40) }
    };

    let get_theme_color = |keys: &[&str]| -> Option<Color> {
        for item in &theme.scopes {
            let scope_str = format!("{:?}", item.scope).to_lowercase();
            for key in keys {
                if scope_str.contains(key) {
                    if let Some(c) = item.style.foreground {
                        return Some(Color::Rgb { r: c.r, g: c.g, b: c.b });
                    }
                }
            }
        }
        None
    };

    let flag_a = Color::Green;
    let flag_d = Color::Magenta;
    let flag_n = Color::Yellow;
    let flag_star = Color::Red;

    let accent = get_theme_color(&["entity.name.function", "variable"])
        .unwrap_or(if is_dark { Color::Rgb { r: 100, g: 200, b: 255 } } else { Color::Rgb { r: 20, g: 100, b: 180 } });

    let date_color = get_theme_color(&["comment", "punctuation.definition.comment"])
        .unwrap_or(if is_dark { Color::Rgb { r: 120, g: 120, b: 120 } } else { Color::Rgb { r: 140, g: 140, b: 140 } });

    UiColors { bg, fg, ui_bg, selected_bg, accent, date_color, flag_n, flag_d, flag_a, flag_star, is_dark }
}

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

        if self.menu_state == MenuState::EmailComposer || self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::MoveTo(0, self.top_margin), SetBackgroundColor(default_cross_bg), terminal::Clear(ClearType::CurrentLine))?;
        } else {
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
                    let line_str = self.buffer.line(file_y).to_string();
                    let ranges = fallback_highlighter.as_mut().unwrap().highlight_line(&line_str, &self.syntax_set).unwrap();
                    self.highlight_cache.insert(file_y, ranges.into_iter().map(|(s, t)| (s, t.to_string())).collect());
                }

                let ranges = self.highlight_cache.get(&file_y).unwrap();
                let mut visual_x = 0;
                let mut line_char_idx = 0;
                let line_has_search_highlight = self.highlight_match.map_or(false, |(h_y, _, _)| h_y == file_y);

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

        queue!(stdout, cursor::MoveTo(0, rows - 3))?;
        if !self.status_message.is_empty() {
            queue!(stdout, SetBackgroundColor(ui_bg), SetForegroundColor(menu_key_fg))?;
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

        let col_width = ((cols as usize) / 6).max(1);

        match self.menu_state {
            MenuState::EmailComposer => {
                let menu1 = [("^X", " Send"), ("^O", " Write Out"), ("^R", " Read File"), ("^Y", " Prev Pg"), ("^K", " Cut Txt"), ("^C", " Cancel")];
                let menu2 = [("^J", " Justify"), ("^W", " Where Is"), ("^V", " Next Pg"), ("^U", if self.is_justified { " Unjustify" } else { " UnCut" }), ("^T", " To Spell"), ("", "")];
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::EmailReader => {
                let menu1 = [("<", " Back"),  ("R", " Reply"),    ("P", " Prev"),    ("^Y", " Prev Pg"),    ("Home", " Top"), ("A", " Add Address")];
                let menu2 = [("", ""),  ("F", " Forward"), ("N", " Next"), ("^V", " Next Pg"), ("End", " Bottom"), ("1-9", " Open Att")];
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

pub fn draw_app(stdout: &mut std::io::Stdout, app: &App, theme_provider: &Editor) -> io::Result<()> {
    let (cols, rows) = term_size().unwrap_or((80, 24));
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    queue!(stdout, SetBackgroundColor(colors.bg), terminal::Clear(ClearType::All))?;

    match &app.mode {
        AppMode::MainMenu { selected_idx } => {
            let menu_options = [
                ("I", "INBOX", "Go to the default Inbox"),
                ("A", "ADDRESS BOOK", "Update your address book"),
                ("F", "FOLDER LIST", "Select a different folder"),
                ("S", "SETTINGS", "Configure xpine Options"),
                ("H", "HELP", "Get help using xpine"),
                ("Q", "QUIT", "Leave the xpine program"),
            ];

            let header_title = format!("xpine - Main Menu ({})", app.active_account.email);
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

            for (i, (key, title, desc)) in menu_options.iter().enumerate() {
                let y = (rows / 2).saturating_sub(menu_options.len() as u16) + (i * 2) as u16;
                let x = (cols / 2).saturating_sub(25);
                let row_bg = if i == *selected_idx { colors.selected_bg } else { colors.bg };

                queue!(stdout, cursor::MoveTo(x, y), SetBackgroundColor(row_bg), SetForegroundColor(colors.accent), Print(format!(" {:>2} ", key)), SetForegroundColor(colors.fg), Print(format!("{:<15} - {}", title, desc)), ResetColor)?;
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("", ""),       ("P", " Prev"), (">", " Select"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("Q", " Quit"), ("N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        }
        AppMode::AddressBook { selected_idx, addresses } => {
            let title = " --- Address Book --- ";
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), SetForegroundColor(colors.accent), Print(title), Print(" ".repeat((cols as usize).saturating_sub(title.chars().count()))), ResetColor)?;

            let items_per_page = (rows.saturating_sub(3) as usize).max(1);
            let start_idx = if *selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

            for i in 0..items_per_page {
                let actual_idx = start_idx + i;
                queue!(stdout, cursor::MoveTo(0, (i + 1) as u16), SetBackgroundColor(colors.bg), terminal::Clear(ClearType::UntilNewLine))?;

                if actual_idx < addresses.len() {
                    let display_str = format!("  {}", addresses[actual_idx]);
                    if actual_idx == *selected_idx {
                        queue!(stdout, SetBackgroundColor(colors.selected_bg), SetForegroundColor(colors.fg), Print(display_str), ResetColor)?;
                    } else {
                        queue!(stdout, SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg), Print(display_str), ResetColor)?;
                    }
                }
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("<", " Back"), ("D", " Delete"), ("P", " Prev"), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("", ""), ("E", " Edit"), ("N", " Next"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        }
        // AppMode::Settings { selected_idx } => {
        //     let header_title = format!("xpine - Settings ({})", app.active_account.email);
        //     queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;
        //
        //     let menu_options = [
        //         ("W", "SOFT WRAP", theme_provider.soft_wrap),
        //         ("L", "LINE NUMBERS", theme_provider.show_line_numbers),
        //         ("O", "NEWEST EMAIL FIRST", theme_provider.sort_newest_first)
        //     ];
        //
        //     for (i, (key, title, state)) in menu_options.iter().enumerate() {
        //         let y = (rows / 2).saturating_sub(menu_options.len() as u16) + (i * 2) as u16;
        //         let x = (cols / 2).saturating_sub(20);
        //         let row_bg = if i == *selected_idx { colors.selected_bg } else { colors.bg };
        //         let state_str = if *state { "ON " } else { "OFF" };
        //
        //         queue!(stdout, cursor::MoveTo(x, y), SetBackgroundColor(row_bg), SetForegroundColor(colors.accent), Print(format!(" {:>2} ", key)), SetForegroundColor(colors.fg), Print(format!("{:<15} : {}", title, state_str)), ResetColor)?;
        //     }
        //
        //     let m_col = (cols as usize / 6).max(1);
        //     Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("Up/Dn/P/N", " Nav"), ("Right/Ent", " Toggle"), ("</Left", " Back"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        //     Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("W", " Soft Wrap"), ("L", " Line Nums"), ("O", " Sort Order"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        // }
        AppMode::Settings { selected_idx } => {
            let header_title = "xpine - Settings";
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::CurrentLine), SetForegroundColor(colors.accent), Print("   "), Print(header_title), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg))?;

            // Add the third option "Sort Newest First"
            let options = [
                ("Soft Wrap", theme_provider.soft_wrap),
                ("Show Line Numbers", theme_provider.show_line_numbers),
                ("Sort Newest First", theme_provider.sort_newest_first), // Added this line
            ];

            for (i, (title, is_enabled)) in options.iter().enumerate() {
                let y = 2 + i as u16;
                if i == *selected_idx {
                    queue!(stdout, cursor::MoveTo(2, y), SetBackgroundColor(colors.selected_bg))?;
                } else {
                    queue!(stdout, cursor::MoveTo(2, y))?;
                }
                let checkbox = if *is_enabled { "[X]" } else { "[ ]" };
                queue!(stdout, Print(format!(" {} {:<20} ", checkbox, title)), ResetColor)?;
            }

            let m_col = (cols as usize / 6).max(1);

            // Add the "O" (Order) hotkey to the menu bar description
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("", ""),       ("P", " Prev"), ("X", " Select"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("<", " Back"), ("N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        }
        AppMode::FolderList { step, selected_idx, folders } => {
            let header_title = if *step == 0 { "xpine - Select Account".to_string() } else { format!("xpine - Folders ({})", app.active_account.email) };
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

            let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };
            let visible_items = (rows.saturating_sub(6)) as usize;
            let start_idx = if *selected_idx >= visible_items { *selected_idx - visible_items + 1 } else { 0 };

            for i in 0..visible_items {
                let actual_idx = start_idx + i;
                if actual_idx < items_count {
                    let text = if *step == 0 { app.accounts[actual_idx].email.clone() } else { folders[actual_idx].clone() };
                    let y = 3 + i as u16;
                    let x = (cols / 2).saturating_sub(20);
                    let row_bg = if actual_idx == *selected_idx { colors.selected_bg } else { colors.bg };

                    queue!(stdout, cursor::MoveTo(x, y), SetBackgroundColor(row_bg), SetForegroundColor(colors.fg), Print(format!("{:<40}", text)), ResetColor)?;
                }
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("Up/Dn", " Nav"), ("Enter", " Select"), ("Esc", " Back"), ("M", " Main Menu"), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg)?;
        }
        AppMode::List => {
            let header_title = format!("xpine - {} ({})", app.current_folder, app.active_account.email);
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine), cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

            let list_start_y = 1;
            let visible_capacity = rows.saturating_sub(3) as usize;

            for (i, email) in app.page_emails.iter().enumerate() {
                if i >= visible_capacity { break; }

                let row_y = (i + list_start_y) as u16;
                let row_bg = if i == app.selected_index { colors.selected_bg } else { colors.bg };

                queue!(stdout, cursor::MoveTo(0, row_y), SetBackgroundColor(row_bg), terminal::Clear(ClearType::UntilNewLine))?;

                let flag_char = if email.is_flagged { "*" } else { " " };
                let status_char = if email.is_deleted { "D" } else if !email.is_read { "N" } else if email.is_answered { "A" } else { " " };

                let size_kb = (email.size / 1024).max(1) as f32;
                let size_str = if size_kb < 1024.0 { format!("({}K)", size_kb as u32) } else { format!("({}M)", (size_kb / 1024.0) as u32) };
                let size_display = format!("{:>6}", size_str);
                let heat = (size_kb.log2() / 12.3).min(1.0).max(0.0);

                let (base_r, base_g, base_b) = match colors.fg { Color::Rgb { r, g, b } => (r as f32, g as f32, b as f32), _ => (255.0, 255.0, 255.0) };
                let hot_r = if colors.is_dark { 255.0 } else { 220.0 }; let hot_g = if colors.is_dark { 80.0 } else { 0.0 }; let hot_b = if colors.is_dark { 80.0 } else { 0.0 };

                let size_color = Color::Rgb { r: (base_r + (hot_r - base_r) * heat) as u8, g: (base_g + (hot_g - base_g) * heat) as u8, b: (base_b + (hot_b - base_b) * heat) as u8 };
                let from_width = 22; let from_str = format!("{:<width$}", email.from.chars().take(from_width).collect::<String>(), width = from_width);
                let date_width = 9; let date_str = format!("{:<width$}", email.date, width = date_width);
                let fixed_width = 47; let subject_width = (cols as usize).saturating_sub(fixed_width);
                let subj_truncated = email.subject.chars().take(subject_width).collect::<String>();
                let padded_subj = format!("{:<width$}", subj_truncated, width = subject_width);

                let status_color = match status_char { "N" => colors.flag_n, "D" => colors.flag_d, "A" => colors.flag_a, _ => colors.fg };

                queue!(
                    stdout, SetBackgroundColor(row_bg),
                    SetForegroundColor(colors.flag_star), Print(flag_char), Print(" "),
                    SetForegroundColor(status_color), Print(status_char), Print(" "),
                    SetForegroundColor(colors.date_color), Print(date_str), Print("  "),
                    SetForegroundColor(colors.fg), Print(from_str), Print("  "),
                    Print(padded_subj), Print("  "),
                    SetForegroundColor(size_color), Print(size_display)
                )?;
            }

            let r_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, r_col, &[(">", " View"), ("M", " Menu"), ("C", " Compose"), ("R", " Reply"),   ("D", " Delete"), ("*", " Flag")], colors.ui_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, r_col, &[("Q", " Quit"), ("<", " Back"), ("Tab", " Acct"),  ("F", " Forward"), ("X", " Expunge"), ("U", " Toggle Read"), ], colors.ui_bg, colors.accent, colors.fg)?;

            if let Some(time) = app.list_status_time {
                if time.elapsed() >= app.list_status_duration {
                    // Timeout handled in main loop logic
                } else if !app.list_status.is_empty() {
                    queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(format!(" {} ", app.list_status)), ResetColor)?;
                }
            }
        }
        _ => {}
    }
    stdout.flush()?;
    Ok(())
}