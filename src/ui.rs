use crate::app::{App, AppMode};
use crate::editor::{Editor, MenuState};
use crossterm::{cursor, event::{self, Event, KeyCode, KeyModifiers}, queue,
                style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
                terminal::{self, ClearType, size as term_size}};
use syntect::easy::HighlightLines;
use std::io::{self, stdout, Write};
use std::{env, fs};
use std::path::PathBuf;
use crate::config::ConfigExt;
use crate::prompt::PromptExt;
pub(crate) use crate::theme::{derive_ui_colors};

pub trait UiExt {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, cols: u16, col_width: usize,
                      items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()>;
    fn draw_screen(&mut self) -> io::Result<()>;
    fn run_file_browser(&mut self, is_saving: bool) -> io::Result<Option<String>>;
    fn show_help(&mut self) -> io::Result<()>;
    fn set_status(&mut self, message: String);
    fn clear_status(&mut self);
}

impl UiExt for Editor {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, cols: u16, col_width: usize,
                      items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()> {
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

        // let visible_rows = rows.saturating_sub((4 + self.top_margin) as u16) as usize;
        let has_status = !self.status_message.is_empty();
        // If there's a status message, it takes 1 row. If empty, it takes 0 rows.
        let status_overhead = if has_status { 1 } else { 0 };

        // 2 rows for menu keys + dynamic status row overhead.
        // Removing the extra old fixed padding entirely.
        let runtime_overhead = 2 + status_overhead;
        let visible_rows = rows.saturating_sub((runtime_overhead + self.top_margin) as u16) as usize;

        let theme = &self.theme_set.themes[&self.current_theme];
        let is_dark = crate::theme::is_dark_theme(theme);
        let raw_theme_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });

        let default_cross_bg = Color::Rgb { r: raw_theme_bg.r, g: raw_theme_bg.g, b: raw_theme_bg.b };
        let ui_colors = derive_ui_colors(theme);
        let ui_bg = ui_colors.menu_bg;
        let title_fg = if is_dark { Color::Reset } else { Color::Black };
        let menu_key_fg = ui_colors.accent;
        let menu_text_fg = ui_colors.fg;
        let dollar_bg = if is_dark { Color::Rgb { r: 180, g: 180, b: 180 } } else { Color::Rgb { r: 80, g: 80, b: 80 } };
        let dollar_fg = if is_dark { Color::Black } else { Color::White };

        if self.menu_state == MenuState::EmailComposer || self.menu_state == MenuState::EmailReader || self.top_margin > 0 {
            queue!(stdout, cursor::MoveTo(0, self.top_margin), SetBackgroundColor(default_cross_bg), terminal::Clear(ClearType::CurrentLine))?;
        } else {
            queue!(stdout, cursor::MoveTo(0, self.top_margin), SetBackgroundColor(ui_bg))?;
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

//                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin + 1))?;
                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin))?;
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
                // This clears all remaining terminal lines assigned to the editor viewport,
                // obliterating any ghosts left over from the email list index view.
                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin))?;
                if self.show_line_numbers {
                    if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                    queue!(stdout, Print(" ".repeat(gutter_width)))?;
                }
                if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;
            }
            terminal_y += 1; file_y += 1;
        }

        if has_status {
            queue!(stdout, cursor::MoveTo(0, rows - 3))?;
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
        }

        let col_width = ((cols as usize) / 6).max(1);

        match self.menu_state {
            MenuState::EmailComposer => {
                if self.menu_page == 1 {
                    Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("^X", " Send"),   ("^P", " Prev"), ("^Y", " Prev Pg"), ("^K", " Cut"),   ("^J", " Justify"), ("^O", " Other 1/2")], ui_bg, menu_key_fg, menu_text_fg)?;

                    Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("^C", " Cancel"), ("^N", " Next"), ("^V", " Next Pg"), ("^U", " UnCut"), ("^A", " Attach"),   ("^G", " Get Help")], ui_bg, menu_key_fg, menu_text_fg)?;
                } else {
                    Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("^R", " Read File"), ("^T", " To Spell"), ("", ""), ("", ""),  ("",""), ("^O", " Other 2/2")], ui_bg, menu_key_fg, menu_text_fg)?;
                    Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("^W", " Where is"), ("Alt-A", " Mark"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
                }
            }
            MenuState::EmailReader => {
                let menu1 = [("<", " Back"),  ("R", " Reply"),    ("P", " Prev"),    ("Y", " Prev Pg"),  ("A", " Add Addr"), ("S"," Save")];
                let menu2 = [("", ""),  ("F", " Forward"), ("N", " Next"), ("V", " Next Pg"), ("1-9", " Open Att"), ("B"," Browser")];
                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
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
            // MenuState::SpellCheck => {
            //     Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &[("I", " Ignore"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            //     Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &[("A", " Add Word"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, menu_text_fg)?;
            // }
            MenuState::SpellCheck => {
                // Extract up to 4 suggestions safely from the editor state
                let s1 = self.current_suggestions.get(0).cloned().unwrap_or_default();
                let s2 = self.current_suggestions.get(1).cloned().unwrap_or_default();
                let s3 = self.current_suggestions.get(2).cloned().unwrap_or_default();
                let s4 = self.current_suggestions.get(3).cloned().unwrap_or_default();

                let menu1 = vec![
                    ("1", if s1.is_empty() { "" } else { s1.as_str() }),
                    ("3", if s3.is_empty() { "" } else { s3.as_str() }),
                    ("I", "Ignore"),
                    ("^C", "Cancel"),
                ];
                let menu2 = vec![
                    ("2", if s2.is_empty() { "" } else { s2.as_str() }),
                    ("4", if s4.is_empty() { "" } else { s4.as_str() }),
                    ("A", "Add to Dict"),
                    ("", ""), // Blank to balance the menu columns
                ];

                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
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
            virtual_y as u16 + self.top_margin // Removed + 1
        } else {
            self.cursor_y.saturating_sub(self.row_offset) as u16 + self.top_margin // Removed + 1
        };

        if self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::Hide)?;
        } else {
            queue!(stdout, cursor::Show, cursor::MoveTo(final_cursor_x, final_cursor_y))?;
        }

        stdout.flush()?;
        Ok(())
    }

    fn run_file_browser(&mut self, is_saving: bool) -> io::Result<Option<String>> {
        let mut current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut selected_idx = 0;
        let mut scroll_offset = 0;
        let mut error_msg = String::new(); // State variable for file not found errors

        loop {
            let mut stdout = stdout();
            let (cols, rows) = terminal::size()?;

            let theme = &self.theme_set.themes[&self.current_theme];
            let ui_colors = derive_ui_colors(theme);
            let ui_bg = ui_colors.menu_bg;
            let title_fg = ui_colors.fg;
            let menu_key_fg = ui_colors.accent;

            let mut entries = Vec::new();
            entries.push((".".to_string(), true));
            if current_dir.parent().is_some() { entries.push(("..".to_string(), true)); }

            if let Ok(read_dir) = fs::read_dir(&current_dir) {
                let mut dirs = Vec::new();
                let mut files = Vec::new();
                let mut dot_dirs = Vec::new();
                let mut dot_files = Vec::new();

                for entry in read_dir.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let is_dot = name.starts_with('.');

                    if is_dir {
                        if is_dot { dot_dirs.push((name, is_dir)); } else { dirs.push((name, is_dir)); }
                    } else {
                        if is_dot { dot_files.push((name, is_dir)); } else { files.push((name, is_dir)); }
                    }
                }

                dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
                files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
                dot_dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
                dot_files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

                entries.extend(dirs); entries.extend(files); entries.extend(dot_dirs); entries.extend(dot_files);
            }

            if selected_idx >= entries.len() { selected_idx = entries.len().saturating_sub(1); }

            queue!(stdout, SetBackgroundColor(ui_colors.bg), terminal::Clear(ClearType::All))?;

            let title_text = format!("xpine - File Browser: {}", current_dir.display());
            let title_len = title_text.chars().count();

            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(ui_bg), SetForegroundColor(menu_key_fg), Print(&title_text))?;

            if (cols as usize) > title_len {
                let padding = " ".repeat((cols as usize) - title_len);
                queue!(stdout, SetBackgroundColor(ui_bg), Print(padding))?;
            }

            // Draw the error message right below the title if it exists
            if !error_msg.is_empty() {
                queue!(stdout, cursor::MoveTo(0, 1), SetBackgroundColor(ui_bg), SetForegroundColor(Color::Red), Print(format!("   {}", error_msg)))?;
            }

            let visible_rows = (rows.saturating_sub(4)) as usize;
            let visible_rows_safe = visible_rows.max(1);

            if selected_idx < scroll_offset {
                scroll_offset = selected_idx;
            } else if selected_idx >= scroll_offset + visible_rows_safe {
                scroll_offset = selected_idx - visible_rows_safe + 1;
            }

            if scroll_offset + visible_rows_safe > entries.len() {
                scroll_offset = entries.len().saturating_sub(visible_rows_safe);
            }

            let start_idx = scroll_offset;

            for i in 0..visible_rows_safe {
                if start_idx + i < entries.len() {
                    let actual_idx = start_idx + i;
                    let is_selected = actual_idx == selected_idx;

                    // 1. Choose background color
                    let row_bg = if is_selected { ui_colors.selected_bg } else { ui_colors.bg };

                    // 2. IMPORTANT: Move cursor, set background, and Clear the entire line width
                    queue!(stdout,
               cursor::MoveTo(0, i as u16 + 1),
               SetBackgroundColor(row_bg),
               terminal::Clear(ClearType::CurrentLine)).unwrap();

                    // 3. Render your text content
                    let (name, is_dir) = &entries[actual_idx];
                    let prefix = if *is_dir { "[DIR] " } else { "      " };
                    let display_str = format!("{}{}", prefix, name);

                    let fg_color = if is_selected { Color::White } else { if *is_dir { menu_key_fg } else { title_fg } };

                    queue!(stdout,
               cursor::MoveTo(2, i as u16 + 1), // Indent the text
               SetForegroundColor(fg_color),
               Print(display_str)).unwrap();
                }
            }


            let m_col = (cols as usize / 6).max(1);
            Self::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[ ("", ""),        ("P", " Prev"), ("Y", " Prev Pg"), ("Enter", " Select"), ("", ""), ("", ""), ("", "")], ui_bg, menu_key_fg, title_fg)?;
            Self::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"),("N", " Next"), ("V", " Next Pg"), ("", ""), ("", "")], ui_bg, menu_key_fg, title_fg)?;

            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                error_msg.clear(); // Clear the error on the next keystroke
                match key.code {
                    KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => { selected_idx = selected_idx.saturating_sub(1); },
                    KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => { if selected_idx + 1 < entries.len() { selected_idx += 1; } },
                    KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') => { selected_idx = selected_idx.saturating_sub(visible_rows_safe); },
                    KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') => { selected_idx = (selected_idx + visible_rows_safe).min(entries.len().saturating_sub(1)); },
                    KeyCode::Home => { selected_idx = 0; scroll_offset = 0; },
                    KeyCode::End => { selected_idx = entries.len().saturating_sub(1); },
                    KeyCode::Enter | KeyCode::Right => {
                        if !entries.is_empty() {
                            let selected = &entries[selected_idx];
                            if selected.0 == "." {
                                let prompt_str = if is_saving { "Save as: " } else { "Attach file: " };
                                if let Ok(Some(filename)) = self.prompt(prompt_str, false) {
                                    if !filename.trim().is_empty() {
                                        let target_path = current_dir.join(filename.trim());
                                        if !is_saving && !target_path.exists() {
                                            error_msg = format!("File '{}' does not exist in this directory.", filename.trim());
                                        } else {
                                            return Ok(Some(target_path.to_string_lossy().into_owned()));
                                        }
                                    }
                                }
                            } else if selected.0 == ".." {
                                if let Some(parent) = current_dir.parent() {
                                    current_dir = parent.to_path_buf();
                                    selected_idx = 0;
                                    scroll_offset = 0;
                                }
                            } else if selected.1 {
                                current_dir = current_dir.join(&selected.0);
                                selected_idx = 0;
                                scroll_offset = 0;
                            } else {
                                return Ok(Some(current_dir.join(&selected.0).to_string_lossy().into_owned()));
                            }
                        }
                    },
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) { return Ok(None); }
                    },
                    KeyCode::Esc => { return Ok(None); },
                    _ => {}
                }
            }
        }
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
        AppMode::AddressBook { selected_idx, addresses } => {
            let title = "xpine - Address Book";
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(title), Print(" ".repeat((cols as usize).saturating_sub(title.chars().count()))), ResetColor)?;

            let items_per_page = (rows.saturating_sub(4) as usize).max(1);
            let start_idx = if *selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

            for i in 0..items_per_page {
                let actual_idx = start_idx + i;
                if actual_idx < addresses.len() {
                    let is_selected = actual_idx == *selected_idx;
                    let bg_color = if is_selected { colors.selected_bg } else { colors.bg };

                    queue!(stdout, cursor::MoveTo(0, (i + 1) as u16), SetBackgroundColor(bg_color), terminal::Clear(ClearType::CurrentLine))?;

                    let display_str = &addresses[actual_idx];
                    let padding = "  ";

                    queue!(stdout, SetForegroundColor(colors.fg), Print(padding))?;

                    // Check if this is a Team (contains a colon)
                    if let Some((team_name, emails)) = display_str.split_once(':') {
                        queue!(
                                stdout,
                                SetForegroundColor(colors.accent),
                                Print(team_name),
                                SetForegroundColor(colors.fg),
                                Print(":"),
                                Print(emails)
                            )?;
                    } else {
                        // Standard individual email
                        queue!(stdout, SetForegroundColor(colors.fg), Print(display_str))?;
                    }
                }
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(
                stdout,
                rows - 2,
                cols,
                m_col,
                &[("<", " Back"), ("P", " Prev"), ("Y", " Prev Pg"), ("A", " Add"), ("E", " Edit"), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;
            Editor::draw_menu_line(
                stdout,
                rows - 1,
                cols,
                m_col,
                &[("", ""), ("N", " Next"), ("V", " Next Pg"), ("T", " Team"), ("D", " Delete"), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;
        }
        AppMode::EmailAccounts { selected_idx } => {
            let title = "xpine - Email Accounts";
            let title_len = title.chars().count();

            // Draw the top title bar
            queue!(
                stdout,
                cursor::MoveTo(0, 0),
                SetBackgroundColor(colors.menu_bg),
                SetForegroundColor(colors.accent),
                Print(title),
                Print(" ".repeat((cols as usize).saturating_sub(title_len))),
                ResetColor
            )?;

            // Calculate pagination
            let items_per_page = (rows.saturating_sub(4) as usize).max(1);
            let start_idx = if *selected_idx >= items_per_page {
                selected_idx - items_per_page + 1
            } else {
                0
            };

            // Draw the list of email accounts
            for i in 0..items_per_page {
                let actual_idx = start_idx + i;
                if actual_idx < app.accounts.len() {
                    let is_selected = actual_idx == *selected_idx;
                    let bg_color = if is_selected { colors.selected_bg } else { colors.bg };

                    let acc = &app.accounts[actual_idx];
                    // Format: user@email.com (imap.server.com)
                    let display_str = format!("  {} ({})", acc.email, acc.imap_server);

                    queue!(
                stdout,
                cursor::MoveTo(0, (i + 1) as u16),
                SetBackgroundColor(bg_color),
                SetForegroundColor(colors.fg),
                Print(format!("{:<width$}", display_str, width = cols as usize)),
                ResetColor
            )?;
                } else {
                    // Clear remaining empty rows
                    queue!(
                stdout,
                cursor::MoveTo(0, (i + 1) as u16),
                SetBackgroundColor(colors.bg),
                terminal::Clear(ClearType::UntilNewLine)
            )?;
                }
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(
                stdout, rows - 2, cols, m_col,
                // Changed "N New Account" to "A Add Account"
                &[("", ""), ("A", " Add Acct"), ("D", " Del Acct"), ("P", " Prev"), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;
            Editor::draw_menu_line(
                stdout, rows - 1, cols, m_col,
                // Added ("N", " Next") here to match the navigation
                &[("<", " Back"), ("E", " Edit Acct"), ("", ""), ("N", " Next"), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;

            queue!(
                stdout,
                SetBackgroundColor(colors.bg),    // Set background to theme color
                SetForegroundColor(Color::Red),      // Set text to red
                cursor::MoveTo(0, rows - 6),
                Print("  - App Specific Passwords are required for GMail, Outlook, & Yahoo"),
                cursor::MoveTo(0, rows - 5),
                Print("  - Generate online with your email provider (Google for help)"),
                cursor::MoveTo(0, rows - 4),
                Print("  - Enter the App Specific Password WITHOUT spaces"),
                ResetColor                           // Reset all colors
            )?;
        }
        AppMode::FolderList { step, selected_idx, folders } => {
            let header_title = if *step == 0 { "xpine - Select Account".to_string() } else { format!("xpine - Folders ({})", app.active_account.email) };
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

            let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };
            let visible_items = (rows.saturating_sub(5)) as usize;
            let start_idx = if *selected_idx >= visible_items { *selected_idx - visible_items + 1 } else { 0 };


            for i in 0..visible_items {
                let actual_idx = start_idx + i;
                if actual_idx < items_count {
                    let text = if *step == 0 { app.accounts[actual_idx].email.clone() } else { folders[actual_idx].clone() };
                    let y = 1 + i as u16; // Using the updated Y coordinate from our previous fix
                    let x = 2;
                    let is_selected = actual_idx == *selected_idx;
                    let row_bg = if is_selected { colors.selected_bg } else { colors.bg };

                    // 1. Set the background for the whole line
                    queue!(stdout, cursor::MoveTo(0, y), SetBackgroundColor(row_bg), terminal::Clear(ClearType::CurrentLine))?;

                    // 2. Print the text with the correct foreground
                    queue!(stdout, cursor::MoveTo(x, y), SetForegroundColor(colors.fg), Print(&text), ResetColor)?;
                }
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("M", " Main Menu"), ("P", " Prev"), ("Y", " Prev Pg"), (">", " Select"),  ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("<", " Back"),      ("N", " Next"), ("V", " Next Pg"), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
        }
        AppMode::EmailList => {
            let header_title = format!("xpine - {} ({})", app.current_folder, app.active_account.email);

            // Draw the base title
            queue!(
                stdout,
                cursor::MoveTo(0, 0),
                SetBackgroundColor(colors.menu_bg),
                terminal::Clear(ClearType::UntilNewLine),
                cursor::MoveTo(0, 0),
                SetForegroundColor(colors.accent),
                Print(header_title)
            )?;

            // Draw the search query in the red flag color if active
            if let Some(ref query) = app.search_query {
                queue!(
                    stdout,
                    SetForegroundColor(colors.flag_star),
                    Print(format!("   Search Results: {}", query)),
                )?;
            }

            queue!(stdout, ResetColor)?;

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
            if app.menu_page == 1 {
                Editor::draw_menu_line(stdout, rows - 2, cols, r_col, &[("<", " Back"), (">", " View"), ("C", " Compose"), ("R", " Reply"),   ("D", " Delete"), ("O", " Other (1/2)")], colors.menu_bg, colors.accent, colors.fg)?;
                Editor::draw_menu_line(stdout, rows - 1, cols, r_col, &[("Q", " Quit"), ("M", " Menu"), ("*", " Flag"),    ("F", " Forward"), ("X", " Expunge"), ("Tab", " Acct")], colors.menu_bg, colors.accent, colors.fg)?;
            } else {
                Editor::draw_menu_line(stdout, rows - 2, cols, r_col, &[("U", " (Un)Read"), ("P", " Prev"), ("Y", " Prev Pg"), ("Meta+T", " Theme"), ("", ""), ("O", " Other (2/2)")], colors.menu_bg, colors.accent, colors.fg)?;
                Editor::draw_menu_line(stdout, rows - 1, cols, r_col, &[("S", " Search"), ("N", " Next"), ("V", " Next Pg"), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
            }

            if let Some(time) = app.list_status_time {
                if time.elapsed() >= app.list_status_duration {
                    // Timeout handled in main loop logic
                } else if !app.list_status.is_empty() {
                    queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(format!(" {} ", app.list_status)), ResetColor)?;
                }
            }
        }
        AppMode::MainMenu { selected_idx } => {
            let menu_options = [
                ("I", "INBOX", "Go to the default Inbox"),
                ("A", "ADDRESS BOOK", "Update address book"),
                ("F", "FOLDER LIST", "Select folder"),
                ("S", "SETTINGS", "Configure xpine"),
                ("E", "EMAIL ACCOUNTS", "Edit/Add email accounts"),
                ("H", "HELP", "Get help using xpine"),
                ("Q", "QUIT", "Leave the xpine program"),
            ];

            let header_title = format!("xpine - Main Menu ({})", app.active_account.email);
            queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

            for (i, (key, title, desc)) in menu_options.iter().enumerate() {
                let y = (rows / 2).saturating_sub(menu_options.len() as u16) + (i * 2) as u16;
                let x = (cols / 2).saturating_sub(25);
                let row_bg = if i == *selected_idx { colors.selected_bg } else { colors.bg };

                queue!(stdout, cursor::MoveTo(x, y), SetBackgroundColor(row_bg), SetForegroundColor(colors.accent), Print(format!(" {:>2} ", key)), SetForegroundColor(colors.fg), Print(format!("{:<15} - {}", title, desc)), ResetColor)?;
            }

            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("", ""),       ("P", " Prev"), (">", " Select"), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("Q", " Quit"), ("N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;

            if app.accounts.is_empty() {
                let msg = "No email account. Please type 'E' and Add an email account.";
                // Adjust the Y coordinate (rows - 3) based on where your status bar is
                queue!(
                    stdout,
                    cursor::MoveTo(0, rows.saturating_sub(3)),
                    SetForegroundColor(Color::Red),
                    Print(msg),
                    ResetColor
                )?;
            }
        }
        AppMode::Settings { selected_idx } => {
            let header_title = "xpine - Settings";

            queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(colors.menu_bg),
            terminal::Clear(ClearType::CurrentLine),
            SetForegroundColor(colors.accent),
            Print(header_title),
            SetBackgroundColor(colors.bg),
            SetForegroundColor(colors.fg)
        )?;

            let options = [
                ("    Soft Wrap", theme_provider.soft_wrap),
                ("    Show Line Numbers", theme_provider.show_line_numbers),
                ("    Sort Newest First", theme_provider.sort_newest_first),
            ];

            for (i, (title, is_enabled)) in options.iter().enumerate() {
                let y = 1 + i as u16;

                if i == *selected_idx {
                    queue!(stdout, cursor::MoveTo(1, y), SetBackgroundColor(colors.selected_bg))?;
                } else {
                    queue!(stdout, cursor::MoveTo(1, y), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg))?;
                }

                let checkbox = if *is_enabled { " [X]" } else { " [ ]" };

                queue!(stdout, Print(format!("{} {:<20} ", checkbox, title)), ResetColor)?;
            }

            // --- UPDATED CODE: 1 blank line gap, custom column alignments, and theme colorization ---
            let theme_y = 2 + options.len() as u16;

            queue!(
                stdout,
                cursor::MoveTo(2, theme_y),
                SetBackgroundColor(colors.bg),
                SetForegroundColor(colors.accent),
                Print("Meta+T"),
                ResetColor
            )?;

            queue!(
                stdout,
                cursor::MoveTo(10, theme_y),
                SetBackgroundColor(colors.bg),
                SetForegroundColor(colors.fg),
                Print("Theme: "),
                ResetColor
            )?;

            queue!(
                stdout,
                SetBackgroundColor(colors.bg),
                SetForegroundColor(colors.accent),
                Print(format!("{}", theme_provider.current_theme)),
                ResetColor
            )?;

            let m_col = (cols as usize / 6).max(1);

            Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("", ""),          ("P", " Prev"), ("X", " Select"), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
            Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("<", " Back"),    ("N", " Next"), ("Meta+T", " Theme"), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg)?;
        }
        _ => {}
    }

    if !theme_provider.status_message.is_empty() {
        if let Some(time) = theme_provider.status_time {
            if time.elapsed() < std::time::Duration::from_secs(3) {
                queue!(
                    stdout,
                    cursor::MoveTo(0, rows - 3),
                    SetBackgroundColor(colors.selected_bg),
                    terminal::Clear(ClearType::UntilNewLine),
                    SetForegroundColor(colors.accent),
                    Print(format!(" {} ", theme_provider.status_message)),
                    ResetColor
                )?;
            }
        }
    }

    stdout.flush()?;
    Ok(())
}

