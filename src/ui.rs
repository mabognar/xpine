use crate::app::{App, AppMode};
use crate::editor::{Editor, MenuState};
use crossterm::{cursor, event::{self, Event}, queue,
                style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
                terminal::{self, ClearType, size as term_size}};
use syntect::easy::HighlightLines;
use std::io::{self, stdout, Write};
use crossterm::event::KeyCode;
pub(crate) use crate::theme::{derive_ui_colors};

pub trait UiExt {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, cols: u16, col_width: usize,
                      items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()>;
    fn draw_screen(&mut self) -> io::Result<()>;
    fn show_help(&mut self, context: &str) -> io::Result<()>;
    fn set_status(&mut self, message: String);
    fn clear_status(&mut self);
}

impl UiExt for Editor {
    fn draw_menu_line(writer: &mut io::Stdout, row: u16, _cols: u16, col_width: usize,
                      items: &[(&str, &str)], ui_bg: Color, key_fg: Color, text_fg: Color) -> io::Result<()> {

        // 1. Clear the entire line with the background color first!
        // This cleanly paints the edge without pushing the cursor into the scroll-wrap zone.
        queue!(
            writer,
            cursor::MoveTo(0, row),
            SetBackgroundColor(ui_bg),
            terminal::Clear(ClearType::CurrentLine)
        )?;

        let safe_col_width = col_width.max(1) as u16;
        let mut current_x = 0;

        for (cmd, desc) in items.iter() {
            let cmd_chars = cmd.chars().count() as u16;
            let desc_chars = desc.chars().count() as u16;

            // 2. Jump directly to the starting column for this item
            queue!(
                writer,
                cursor::MoveTo(current_x, row),
                SetForegroundColor(key_fg),
                Print(cmd),
                SetForegroundColor(text_fg)
            )?;

            // 3. Print the description (truncated if it exceeds the column width)
            if cmd_chars + desc_chars <= safe_col_width {
                queue!(writer, Print(desc))?;
            } else {
                let safe_desc = desc.chars().take((safe_col_width.saturating_sub(cmd_chars)) as usize).collect::<String>();
                queue!(writer, Print(safe_desc))?;
            }

            current_x += safe_col_width;
        }

        queue!(writer, SetBackgroundColor(Color::Reset))?;
        Ok(())
    }

    fn draw_screen(&mut self) -> io::Result<()> {
        let mut stdout = stdout();
        let (cols, rows) = terminal::size()?;

        let has_status = !self.status_message.is_empty();
        let status_overhead = if has_status { 1 } else { 0 };

        // 2 rows for menu keys + dynamic status row overhead.
        let runtime_overhead = 2 + status_overhead;
        let visible_rows = rows.saturating_sub(runtime_overhead + self.top_margin) as usize;

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

                                    // FIX: Removed the + 1 to align wrapping with the dynamic margin
                                    queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin))?;
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
                        // FIX: Removed the + 1
                        queue!(stdout, cursor::MoveTo(gutter_width as u16, terminal_y as u16 + self.top_margin), Print('$'))?;
                    }
                    if visual_x > self.col_offset + available_width {
                        if last_bg != Some(dollar_bg) { queue!(stdout, SetBackgroundColor(dollar_bg))?; last_bg = Some(dollar_bg); }
                        if last_fg != Some(dollar_fg) { queue!(stdout, SetForegroundColor(dollar_fg))?; last_fg = Some(dollar_fg); }
                        // FIX: Removed the + 1
                        queue!(stdout, cursor::MoveTo(cols - 1, terminal_y as u16 + self.top_margin), Print('$'))?;
                    }
                }
                if last_bg != Some(default_cross_bg) { queue!(stdout, SetBackgroundColor(default_cross_bg))?; last_bg = Some(default_cross_bg); }
                if last_fg != Some(Color::Reset) { queue!(stdout, SetForegroundColor(Color::Reset))?; last_fg = Some(Color::Reset); }

            } else {
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
                    Self::draw_menu_line(
                        &mut stdout, rows - 2, cols, col_width,
                        &[("^X", " Send"), ("^P", " Prev"), ("^Y", " Prev Pg"), ("^K", " Cut"), ("^J", " Justify"), ("^O", " Other 1/2")],
                        ui_bg, menu_key_fg, menu_text_fg)?;

                    Self::draw_menu_line(
                        &mut stdout, rows - 1, cols, col_width,
                        &[("^C", " Cancel"), ("^N", " Next"), ("^V", " Next Pg"), ("^U", " UnCut"), ("^A", " Attach"), ("^G", " Signature")],
                        ui_bg, menu_key_fg, menu_text_fg)?;
                } else {
                    Self::draw_menu_line(
                        &mut stdout, rows - 2, cols, col_width,
                        &[("^R", " Read File"), ("^T", " To Spell"), ("", ""), ("", ""),  ("",""), ("^O", " Other 2/2")],
                        ui_bg, menu_key_fg, menu_text_fg)?;
                    Self::draw_menu_line(
                        &mut stdout, rows - 1, cols, col_width,
                        &[("^W", " Where is"), ("Alt-A", " Mark"), ("", ""), ("", ""), ("", ""), ("", "")],
                        ui_bg, menu_key_fg, menu_text_fg)?;
                }
            }
            MenuState::EmailReader => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("<", " Back"), ("R", " Reply"), ("P", " Prev"), ("Y", " Prev Pg"), ("A", " Add Addr"), ("B"," Browser")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("", ""), ("F", " Forward"), ("N", " Next"), ("V", " Next Pg"), ("S", " Save"), ("", "") ],
                    ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::YesNoCancel => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::ReplaceAction => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("Y", " Yes"), ("A", " All"), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::PromptWithBrowser => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("^T", " To Files"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::CancelOnly => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::SpellCheck => {
                let s1 = self.current_suggestions.get(0).cloned().unwrap_or_default();
                let s2 = self.current_suggestions.get(1).cloned().unwrap_or_default();
                let s3 = self.current_suggestions.get(2).cloned().unwrap_or_default();
                let s4 = self.current_suggestions.get(3).cloned().unwrap_or_default();
                let s5 = self.current_suggestions.get(4).cloned().unwrap_or_default();

                let menu1 = vec![
                    ("1", if s1.is_empty() { "" } else { s1.as_str() }),
                    ("2", if s2.is_empty() { "" } else { s2.as_str() }),
                    ("3", if s3.is_empty() { "" } else { s3.as_str() }),
                    ("4", if s4.is_empty() { "" } else { s4.as_str() }),
                    ("5", if s5.is_empty() { "" } else { s5.as_str() })
                ];
                let menu2 = vec![
                    ("^C", "Cancel"),
                    ("A", "Add to Dict"),
                    ("I", "Ignore"),
                    ("", ""),
                    ("", "")
                ];

                Self::draw_menu_line(&mut stdout, rows - 2, cols, col_width, &menu1, ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(&mut stdout, rows - 1, cols, col_width, &menu2, ui_bg, menu_key_fg, menu_text_fg)?;
            }
            MenuState::TeamEditor => {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, col_width,
                    &[("^X", " Save"), ("^C", " Cancel"), ("^P", " Prev"), ("^Y", "Prev Pg"), ("^A", " Add Email"), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, col_width,
                    &[("", ""), ("", ""), ("^N", " Next"), ("^V", " Next Pg"), ("", ""), ("", "")],
                    ui_bg, menu_key_fg, menu_text_fg)?;
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
            virtual_y as u16 + self.top_margin
        } else {
            self.cursor_y.saturating_sub(self.row_offset) as u16 + self.top_margin
        };

        if self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::Hide)?;
        } else {
            // Queue the cursor jump, but we won't necessarily flush it immediately!
            queue!(stdout, cursor::Show, cursor::MoveTo(final_cursor_x, final_cursor_y))?;
        }

        if self.menu_state != MenuState::EmailComposer {
            stdout.flush()?;
        }

        Ok(())
    }

    fn show_help(&mut self, context: &str) -> io::Result<()> {
        let theme = &self.theme_set.themes[&self.current_theme];
        let colors = derive_ui_colors(theme);

        // A custom data structure to flawlessly map theme colors to help lines
        enum HelpLine {
            Title(&'static str),
            Text(&'static str),
            Header(&'static str),
            Cmd1(&'static str, &'static str),
            Cmd2(&'static str, &'static str, &'static str, &'static str),
            Cmd3(&'static str, &'static str, &'static str, &'static str, &'static str, &'static str),
            Blank,
        }
        use HelpLine::*;

        let help_content = match context {
            "main_menu" => vec![
                Title("xpine - Main Menu Help"),
                Blank,
                Text("The Main Menu is the starting point for xpine navigation"),
                Blank,
                Text("Note: You can obtain help in much of xpine by typing '?'"),
                Blank,
                Header("NAVIGATION:"),
                Cmd1("P, Up", "Move selection up"),
                Cmd1("N, Down", "Move selection down"),
                Cmd1(">, Right, Enter", "Select the highlighted option"),
                Blank,
                Header("SHORTCUT KEYS:"),
                Cmd1("I", "Jump directly to your Inbox"),
                Cmd1("A", "Manage your Address Book"),
                Cmd1("F", "View and manage your Folders"),
                Cmd1("S", "Adjust Application Settings"),
                Cmd1("E", "Configure Email Accounts"),
                Cmd1("Q", "Quit the xpine application"),
            ],
            "email_list" => vec![
                Title("xpine - Email List Help"),
                Blank,
                Text("This screen displays the emails in your currently selected folder"),
                Blank,
                Header("NAVIGATION"),
                Cmd1("P, Up", "Move up"),
                Cmd1("N, Down", "Move down"),
                Cmd1("Y, -, PgUp", "Page Up"),
                Cmd1("V, Space, PgDn", "Page Down"),
                Cmd1(">, Enter, Right", "Read selected email"),
                Cmd1("<, Left, Esc", "Go Back"),
                Cmd1("Tab", "Cycle through your configured email accounts"),
                Cmd1("1,2,...", "Go to email account"),
                Blank,
                Header("ACTIONS"),
                Cmd1("C", "Compose new email"),
                Cmd1("R", "Reply to email"),
                Cmd1("F", "Forward email"),
                Cmd1("D", "Mark email for deletion"),
                Cmd1("X", "Expunge (completely remove) all emails marked"),
                Cmd1("", "for deletion"),
                Cmd1("O", "Toggle Menu Page"),
                Cmd1("U", "Toggle Read/Unread status"),
                Cmd1("*", "Toggle email as important"),
                Cmd1("S", "Search current folder"),
                Cmd1("Meta+M, Alt+M", "Move email to folder"),
                Cmd1("Meta+T, Alt+T", "Cycle Theme"),
            ],
            "composer" => vec![
                Title("xpine - Email Composer Help"),
                Blank,
                Text("A full-featured text editor for composing your messages."),
                Blank,
                Header("NAVIGATION:"),
                Cmd1("Arrow Keys", "Move cursor"),
                Cmd1("^Y / ^V", "Page Up / Page Down"),
                Cmd1("Alt-A / ^6", "Set a selection mark"),
                Blank,
                Header("ACTIONS (PAGE 1):"),
                Cmd3("^X", "Send Msg", "^C", "Cancel", "^O", "Save/Write Out"),
                Cmd3("^K", "Cut Line", "^U", "Uncut/Paste", "^J", "Justify Paragraph"),
                Cmd2("^A", "Attach File", "^O", "Toggle to Menu Page 2"),
                Blank,
                Header("ACTIONS (PAGE 2):"),
                Cmd1("^R", "Read file into message"),
                Cmd1("^T", "Run Spellchecker"),
                Cmd1("^W", "Search/Where is..."),
            ],
            "address_book" => vec![
                Title("xpine - Address Book Help"),
                Blank,
                Text("Manage your saved contacts and distribution teams (lists)"),
                Blank,
                Header("NAVIGATION"),
                Cmd1("P, Up", "Move up"),
                Cmd1("N, Down", "Move down"),
                Cmd1("Y, -, PgUp", "Page Up"),
                Cmd1("V, Space, PgDn", "Page Down"),
                Cmd1("<, Left, Esc", "Return to the Main Menu"),
                Blank,
                Header("ACTIONS"),
                Cmd1("A", "Add a new email address"),
                Cmd1("T", "Create a new Team (group distribution list)."),
                Cmd1("E", "Edit the currently selected entry"),
                Cmd1("D", "Delete the currently selected entry"),
                Cmd1("I", "Import email list from text file. Format: "),
                Cmd1("", "   1. emails are separated by commas, or"),
                Cmd1("", "   2. emails on separate lines"),
            ],
            "email_accounts" => vec![
                Title("xpine - Email Accounts Help"),
                Blank,
                Text("Configure standard IMAP and Microsoft Graph API accounts."),
                Blank,
                Header("NAVIGATION:"),
                Cmd1("Up/Down or P/N", "Move cursor up and down"),
                Cmd1("<", "Return to the Main Menu"),
                Blank,
                Header("ACTIONS:"),
                Cmd1("A", "Add a new email account"),
                Cmd1("E", "Edit the currently selected account"),
                Cmd1("D", "Delete the currently selected account"),
                Cmd1("M", "Trigger Microsoft OAuth2 login flow"),
            ],
            "folders_list" => vec![
                Title("xpine - Folders List Help"),
                Blank,
                Text("This screen displays the folders in your currently selected account"),
                Blank,
                Header("NAVIGATION"),
                Cmd1("P, Up", "Move up"),
                Cmd1("N, Down", "Move down"),
                Cmd1("Y, -, PgUp", "Page Up"),
                Cmd1("V, Space, PgDn", "Page Down"),
                Cmd1(">, Enter, Right", "Read selected email"),
                Cmd1("<, Left, Esc", "Go Back"),
                Cmd1("Tab", "Cycle through your configured email accounts"),
                Cmd1("1,2,...", "Go to email account"),
                Blank,
                Header("ACTIONS"),
                Cmd1("C", "Compose new email"),
                Cmd1("R", "Reply to email"),
                Cmd1("F", "Forward email"),
                Cmd1("D", "Mark email for deletion"),
                Cmd1("X", "Expunge (completely remove) all emails marked"),
                Cmd1("", "for deletion"),
                Cmd1("O", "Toggle Menu Page"),
                Cmd1("U", "Toggle Read/Unread status"),
                Cmd1("*", "Toggle email as important"),
                Cmd1("S", "Search current folder"),
                Cmd1("Meta+M, Alt+M", "Move email to folder"),
                Cmd1("Meta+T, Alt+T", "Cycle Theme"),
            ],
            _ => vec![
                Title("xpine - Help"),
                Text("No specific help documentation is available for this screen."),
            ],
        };

        let mut stdout = stdout();
        let mut scroll_offset = 0;

        loop {
            let (cols, rows) = terminal::size()?;
            let visible_lines = (rows as usize).saturating_sub(3);
            let max_scroll = help_content.len().saturating_sub(visible_lines);

            scroll_offset = scroll_offset.min(max_scroll);

            queue!(stdout, SetBackgroundColor(colors.bg), terminal::Clear(ClearType::All))?;

            for (i, line) in help_content.iter().skip(scroll_offset).take(visible_lines).enumerate() {
                queue!(stdout, cursor::MoveTo(0, i as u16))?;

                match line {
                    Title(t) => {
                        queue!(stdout,
                            SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent),
                            Print(*t), Print(" ".repeat((cols as usize).saturating_sub(t.chars().count()))),
                            ResetColor, SetBackgroundColor(colors.bg)
                        )?;
                    }
                    Header(h) => {
                        queue!(stdout, SetForegroundColor(colors.date_color), Print(*h))?;
                    }
                    Text(t) => {
                        queue!(stdout, SetForegroundColor(colors.fg), Print(*t))?;
                    }
                    Blank => {}
                    Cmd1(k, d) => {
                        queue!(stdout, Print("  "),
                            SetForegroundColor(colors.accent), Print(format!("{:<15}", k)),
                            SetForegroundColor(colors.fg), Print(format!(" {}", d))
                        )?;
                    }
                    Cmd2(k1, d1, k2, d2) => {
                        queue!(stdout, Print("  "),
                            SetForegroundColor(colors.accent), Print(format!("{:<4}", k1)), SetForegroundColor(colors.fg), Print(format!(" {:<15}", d1)),
                            SetForegroundColor(colors.accent), Print(format!("{:<4}", k2)), SetForegroundColor(colors.fg), Print(format!(" {}", d2))
                        )?;
                    }
                    Cmd3(k1, d1, k2, d2, k3, d3) => {
                        queue!(stdout, Print("  "),
                            SetForegroundColor(colors.accent), Print(format!("{:<4}", k1)), SetForegroundColor(colors.fg), Print(format!(" {:<13}", d1)),
                            SetForegroundColor(colors.accent), Print(format!("{:<4}", k2)), SetForegroundColor(colors.fg), Print(format!(" {:<13}", d2)),
                            SetForegroundColor(colors.accent), Print(format!("{:<4}", k3)), SetForegroundColor(colors.fg), Print(format!(" {}", d3))
                        )?;
                    }
                }
            }

            // Draw the interactive menu at the bottom
            let col_width = ((cols as usize) / 6).max(1);
            Editor::draw_menu_line(
                &mut stdout, rows - 2, cols, col_width,
                &[("<", " Back"), ("P", " Prev"), ("Y", " Prev Pg"), ("", ""), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;
            Editor::draw_menu_line(
                &mut stdout, rows - 1, cols, col_width,
                &[("", ""), ("N", " Next"), ("V", " Next Pg"), ("", ""), ("", ""), ("", "")],
                colors.menu_bg, colors.accent, colors.fg
            )?;

            stdout.flush()?;

            // Wait for user input
            let event = event::read()?;
            if let Event::Key(key) = event {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        // Exit keys
                        KeyCode::Esc | KeyCode::Left | KeyCode::Char('<') => break,

                        // Scroll up one line
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
                            scroll_offset = scroll_offset.saturating_sub(1);
                        }
                        // Scroll down one line
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                            scroll_offset = (scroll_offset + 1).min(max_scroll);
                        }
                        // Scroll up one page
                        KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            scroll_offset = scroll_offset.saturating_sub(visible_lines);
                        }
                        // Scroll down one page
                        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') => {
                            scroll_offset = (scroll_offset + visible_lines).min(max_scroll);
                        }
                        _ => {} // Ignore any other keys to trap the user here
                    }
                }
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

    // Clear the background for the entire screen first
    queue!(stdout, SetBackgroundColor(colors.bg), terminal::Clear(ClearType::All))?;

    // Route to the appropriate rendering helper
    match &app.mode {
        AppMode::AddressBook { selected_idx, addresses } => draw_address_book(stdout, cols, rows, theme_provider, *selected_idx, addresses)?,
        AppMode::EmailAccounts { selected_idx } => draw_email_accounts(stdout, app, cols, rows, theme_provider, *selected_idx)?,
        AppMode::EmailList => draw_email_list(stdout, app, cols, rows, theme_provider)?,
        AppMode::FolderList { step, selected_idx, folders } => draw_folder_list(stdout, app, cols, rows, theme_provider, *step as usize, *selected_idx, folders)?,
        AppMode::MainMenu { selected_idx } => draw_main_menu(stdout, app, cols, rows, theme_provider, *selected_idx)?,
        AppMode::Settings { selected_idx } => draw_settings(stdout, cols, rows, theme_provider, *selected_idx)?,
        AppMode::EmailRead { .. } => {} // Rendered completely in src/read.rs
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

fn draw_address_book(stdout: &mut std::io::Stdout, cols: u16, rows: u16, theme_provider: &Editor, selected_idx: usize, addresses: &[String]) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let title = "xpine - Address Book";
    queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(title), Print(" ".repeat((cols as usize).saturating_sub(title.chars().count()))), ResetColor)?;

    let items_per_page = (rows.saturating_sub(4) as usize).max(1);
    let start_idx = if selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

    for i in 0..items_per_page {
        let actual_idx = start_idx + i;
        if actual_idx < addresses.len() {
            let is_selected = actual_idx == selected_idx;
            let bg_color = if is_selected { colors.selected_bg } else { colors.bg };

            queue!(stdout, cursor::MoveTo(0, (i + 1) as u16), SetBackgroundColor(bg_color), terminal::Clear(ClearType::CurrentLine))?;

            let display_str = &addresses[actual_idx];
            let padding = "  ";

            queue!(stdout, SetForegroundColor(colors.fg), Print(padding))?;

            // Check if this is a Team (contains a colon)
            if let Some((team_name, emails)) = display_str.split_once(':') {
                // Calculate space taken by the padding, team name, and the colon
                let prefix_len = padding.len() + team_name.chars().count() + 1;
                let available_len = (cols as usize).saturating_sub(prefix_len);

                // If the emails run past the margin, truncate and add "..."
                let emails_display = if emails.chars().count() > available_len {
                    let take_len = available_len.saturating_sub(3);
                    format!("{}...", emails.chars().take(take_len).collect::<String>())
                } else {
                    emails.to_string()
                };

                queue!(
                    stdout,
                    SetForegroundColor(colors.accent), Print(team_name),
                    SetForegroundColor(colors.fg), Print(":"), Print(emails_display)
                )?;
            } else {
                queue!(stdout, SetForegroundColor(colors.fg), Print(display_str))?;
            }
        }
    }

    let m_col = (cols as usize / 6).max(1);
    Editor::draw_menu_line(stdout, rows - 2, cols, m_col,
                           &[("<", " Back"), ("P", " Prev"), ("Y", " Prev Pg"), ("A", " Add Email"), ("E", " Edit"), ("I", " Import")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Editor::draw_menu_line(stdout, rows - 1, cols, m_col,
                           &[("", ""), ("N", " Next"), ("V", " Next Pg"), ("T", " Team"), ("D", " Delete"), ("?", " Help")],
                           colors.menu_bg, colors.accent, colors.fg)?;

    queue!(stdout, cursor::Hide)?;

    Ok(())
}

fn draw_email_accounts(stdout: &mut std::io::Stdout, app: &App, cols: u16, rows: u16, theme_provider: &Editor, selected_idx: usize) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let title = "xpine - Email Accounts";
    let title_len = title.chars().count();

    queue!(
        stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg),
        SetForegroundColor(colors.accent), Print(title),
        Print(" ".repeat((cols as usize).saturating_sub(title_len))), ResetColor
    )?;

    let items_per_page = (rows.saturating_sub(4) as usize).max(1);
    let start_idx = if selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

    for i in 0..items_per_page {
        let actual_idx = start_idx + i;
        if actual_idx < app.accounts.len() {
            let is_selected = actual_idx == selected_idx;
            let bg_color = if is_selected { colors.selected_bg } else { colors.bg };
            let acc = &app.accounts[actual_idx];
            let display_str = format!("  {}", acc.email);

            queue!(
                stdout, cursor::MoveTo(0, (i + 1) as u16), SetBackgroundColor(bg_color),
                SetForegroundColor(colors.fg), Print(format!("{:<width$}", display_str, width = cols as usize)), ResetColor
            )?;
        } else {
            queue!(stdout, cursor::MoveTo(0, (i + 1) as u16), SetBackgroundColor(colors.bg), terminal::Clear(ClearType::UntilNewLine))?;
        }
    }

    let m_col = (cols as usize / 6).max(1);
    Editor::draw_menu_line(stdout, rows - 2, cols, m_col,
                           &[(" <", " Back"), ("A", " Add Acct"), ("P", " Prev"), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Editor::draw_menu_line(stdout, rows - 1, cols, m_col,
                           &[("", ""), ("D", " Del Acct"), ("N", " Next"), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;

    queue!(
        stdout, SetBackgroundColor(colors.bg), SetForegroundColor(colors.accent),
        cursor::MoveTo(0, rows - 6), Print("  - App Specific Password is required for Yahoo; generate online"),
        cursor::MoveTo(0, rows - 5), Print("  - Enter the App Specific Password WITHOUT spaces"),
        ResetColor
    )?;
    Ok(())
}

fn draw_email_list(stdout: &mut std::io::Stdout, app: &App, cols: u16, rows: u16, theme_provider: &Editor) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let header_title = format!("xpine - {} ({})", app.current_folder, app.active_account.email);
    queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::UntilNewLine), cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title))?;

    if let Some(ref query) = app.search_query {
        queue!(stdout, SetForegroundColor(colors.flag_star), Print(format!("   Search: {}", query)))?;
    }

    let items_per_page = (rows.saturating_sub(3) as u32).max(1);

    if app.total_messages == 0 || app.page_emails.is_empty() {
        let no_msg = "No messages";
        queue!(stdout,
            cursor::MoveTo(cols.saturating_sub(no_msg.len() as u16), 0), // Removed the + 1
            SetForegroundColor(colors.accent),
            Print(no_msg)
        )?;
    } else {
        let mut end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
        let start_idx = end_idx.saturating_sub(items_per_page.saturating_sub(1)).max(1);

        if start_idx == 1 {
            end_idx = items_per_page.min(app.total_messages);
        }
        
        let msg_x = if theme_provider.sort_newest_first {
            end_idx.saturating_sub(app.selected_index as u32)
        } else {
            let start_idx = end_idx.saturating_sub(app.page_emails.len() as u32).saturating_add(1);
            start_idx + (app.selected_index as u32)
        };

        let x_str = msg_x.to_string();
        let y_str = app.total_messages.to_string();
        let total_len = 8 + x_str.len() + 4 + y_str.len(); // Removed the + 1

        queue!(stdout,
            cursor::MoveTo(cols.saturating_sub(total_len as u16), 0),
            SetForegroundColor(colors.accent), Print("Message "),
            SetForegroundColor(colors.fg), Print(&x_str),
            SetForegroundColor(colors.accent), Print(" of "),
            SetForegroundColor(colors.fg), Print(&y_str) // Removed the trailing Print(" ")
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

        let size_str = if size_kb < 1024.0 {
            format!("{}K", size_kb as u32)
        } else {
            format!("{}M", (size_kb / 1024.0) as u32)
        };

        let size_display = format!("{:>4}", size_str);
        let heat = (size_kb.log2() / 12.3).min(1.0).max(0.0);

        let (base_r, base_g, base_b) = match colors.fg { Color::Rgb { r, g, b } => (r as f32, g as f32, b as f32), _ => (255.0, 255.0, 255.0) };
        let hot_r = if colors.is_dark { 255.0 } else { 220.0 }; let hot_g = if colors.is_dark { 80.0 } else { 0.0 }; let hot_b = if colors.is_dark { 80.0 } else { 0.0 };

        let size_color = Color::Rgb { r: (base_r + (hot_r - base_r) * heat) as u8, g: (base_g + (hot_g - base_g) * heat) as u8, b: (base_b + (hot_b - base_b) * heat) as u8 };

        // 1. Increase from_width from 22 to 23
        let from_width = 23; let from_str = format!("{:<width$}", email.from.chars().take(from_width).collect::<String>(), width = from_width);
        let date_width = 9; let date_str = format!("{:<width$}", email.date, width = date_width);

        let fixed_width = 44;

        let subject_width = (cols as usize).saturating_sub(fixed_width);
        let subj_truncated = email.subject.chars().take(subject_width).collect::<String>();
        let padded_subj = format!("{:<width$}", subj_truncated, width = subject_width);

        let status_color = match status_char { "N" => colors.flag_n, "D" => colors.flag_d, "A" => colors.flag_a, _ => colors.fg };

        queue!(
            stdout, SetBackgroundColor(row_bg),

            // 2. Remove the Print(" ") right after Print(flag_char)
            SetForegroundColor(colors.flag_star), Print(flag_char),
            SetForegroundColor(status_color), Print(status_char), Print(" "),
            SetForegroundColor(colors.date_color), Print(date_str), Print("  "),
            SetForegroundColor(colors.fg), Print(from_str), Print("  "),

            // Print the subject
            Print(padded_subj),

            // Pin the cursor exactly 4 spaces from the right edge
            cursor::MoveTo(cols.saturating_sub(4), row_y),

            // Print the size
            SetForegroundColor(size_color), Print(size_display)
        )?;
    }

    let r_col = (cols as usize / 6).max(1);
    if app.menu_page == 1 {
        Editor::draw_menu_line(stdout, rows - 2, cols, r_col,
                               &[("<", " Back"), (">", " View"), ("C", " Compose"), ("R", " Reply"),   ("D", " Delete"), ("O", " Other (1/2)")],
                               colors.menu_bg, colors.accent, colors.fg)?;
        Editor::draw_menu_line(stdout, rows - 1, cols, r_col,
                               &[("Q", " Quit"), ("M", " Main Menu"), ("S", " Search"), ("F", " Forward"), ("X", " Expunge"), ("Tab", " Acct")],
                               colors.menu_bg, colors.accent, colors.fg)?;
    } else {
        Editor::draw_menu_line(stdout, rows - 2, cols, r_col,
                               &[("*", " Flag"), ("P", " Prev"), ("Y", " Prev Pg"),  ("M+T", " Theme"),   ("", ""), ("O", " Other (2/2)")],
                               colors.menu_bg, colors.accent, colors.fg)?;
        Editor::draw_menu_line(stdout, rows - 1, cols, r_col,
                               &[("U", " (Un)Read"), ("N", " Next"), ("V", " Next Pg"),    ("M+M", " Move To"), ("", ""), ("?", " Help")],
                               colors.menu_bg, colors.accent, colors.fg)?;
    }

    if let Some(time) = app.list_status_time {
        if time.elapsed() >= app.list_status_duration {} else if !app.list_status.is_empty() {
            queue!(stdout, cursor::MoveTo(0, rows - 3),
                SetBackgroundColor(colors.selected_bg),
                terminal::Clear(ClearType::UntilNewLine),
                SetForegroundColor(colors.accent),
                Print(format!("{} ",app.list_status)),
                ResetColor)?;
        }
    }

    queue!(stdout, cursor::Hide)?;

    Ok(())
}

fn draw_folder_list(stdout: &mut std::io::Stdout, app: &App, cols: u16, rows: u16, theme_provider: &Editor, step: usize, selected_idx: usize, folders: &[String]) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let header_title = if step == 0 { "xpine - Select Account".to_string() } else { format!("xpine - Folders ({})", app.active_account.email) };
    queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

    let items_count = if step == 0 { app.accounts.len() } else { folders.len() };
    let visible_items = (rows.saturating_sub(5)) as usize;
    let start_idx = if selected_idx >= visible_items { selected_idx - visible_items + 1 } else { 0 };

    let is_selected_folder_custom = if step != 0 && selected_idx < folders.len() {
        let folder_name = folders[selected_idx].to_lowercase();
        let default_folders = ["inbox", "sent", "trash", "archive", "drafts", "spam", "junk", "deleted", "outbox", "[gmail]", "conversation history"];
        !default_folders.iter().any(|def| folder_name.contains(def))
    } else { false };

    for i in 0..visible_items {
        let actual_idx = start_idx + i;
        if actual_idx < items_count {
            let text = if step == 0 { app.accounts[actual_idx].email.clone() } else { folders[actual_idx].clone() };
            let y = 1 + i as u16;
            let x = 2;
            let is_selected = actual_idx == selected_idx;
            let row_bg = if is_selected { colors.selected_bg } else { colors.bg };

            let default_folders = ["inbox", "sent", "trash", "archive", "drafts", "spam", "junk", "deleted", "outbox", "[gmail]", "conversation history"];
            let is_custom_folder = step != 0 && !default_folders.iter().any(|def| text.to_lowercase().contains(def));
            let fg = if is_custom_folder { colors.accent } else { colors.fg };

            queue!(stdout, cursor::MoveTo(0, y), SetBackgroundColor(row_bg), terminal::Clear(ClearType::CurrentLine))?;
            queue!(stdout, cursor::MoveTo(x, y), SetForegroundColor(fg), Print(&text))?;

            if is_custom_folder {
                let hint_color = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
                queue!(stdout, SetForegroundColor(hint_color), Print("   -> custom folder"))?;
            }
            queue!(stdout, ResetColor)?;
        }
    }

    let m_col = (cols as usize / 6).max(1);
    let rename_opt = if is_selected_folder_custom { ("R", " Rename") } else { ("", "") };
    let del_opt = if is_selected_folder_custom { ("D", " Del Fldr") } else { ("", "") };

    Editor::draw_menu_line(stdout, rows - 2, cols, m_col,
                           &[("M", " Main Menu"), ("P", " Prev"), ("Y", " Prev Pg"), (">", " Select"), rename_opt, ("","")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Editor::draw_menu_line(stdout, rows - 1, cols, m_col,
                           &[("<", " Back"), ("N", " Next"), ("V", " Next Pg"), ("A", " Add Fldr"), del_opt, ("?", " Help")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Ok(())
}

fn draw_main_menu(stdout: &mut std::io::Stdout, app: &App, cols: u16, rows: u16, theme_provider: &Editor, selected_idx: usize) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    // --- NEW: Dynamic Version Strings ---
    let current_version = env!("CARGO_PKG_VERSION");
    let (update_desc, is_update_avail) = match &app.latest_version {
        Some(latest) if latest != current_version => {
            (format!("Update xpine to version {}", latest), true)
        }
        Some(_) => {
            (format!("xpine ({}) is up to date", current_version), false)
        }
        None => {
            ("Checking for updates...".to_string(), false)
        }
    };

    // Add 'U' to the menu array right before 'Q'
    let menu_options = [
        ("I", "INBOX", "Go to the default Inbox"),
        ("A", "ADDRESS BOOK", "Update address book"),
        ("F", "FOLDER LIST", "Select folder"),
        ("S", "SETTINGS", "Configure xpine"),
        ("E", "EMAIL ACCOUNTS", "Add/Delete email accounts"),
        ("H", "HELP", "Get help using xpine"),
        ("U", "UPDATE XPINE", update_desc.as_str()), // ADDED
        ("Q", "QUIT", "Leave the xpine program"),    // Pushed down
    ];

    let header_title = format!("xpine - Main Menu ({})", app.active_account.email);
    queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(header_title), ResetColor)?;

    for (i, (key, title, desc)) in menu_options.iter().enumerate() {
        let y = (rows / 2).saturating_sub(menu_options.len() as u16) + (i * 2) as u16;
        let x = (cols / 2).saturating_sub(25);
        let row_bg = if i == selected_idx { colors.selected_bg } else { colors.bg };

        // Print the Key and Title exactly as before
        queue!(
            stdout,
            cursor::MoveTo(x, y),
            SetBackgroundColor(row_bg),
            SetForegroundColor(colors.accent), Print(format!(" {:>2} ", key)),
            SetForegroundColor(colors.fg), Print(format!("{:<15} - ", title)),
        )?;

        // --- NEW: Custom multi-color rendering for the description ---
        if *key == "U" && !is_update_avail && app.latest_version.is_some() {
            // Split the string into 3 parts to inject `colors.accent` in the middle
            queue!(
                stdout,
                SetForegroundColor(colors.fg), Print("xpine ("),
                SetForegroundColor(colors.accent), Print(current_version), // Hot-key color here!
                SetForegroundColor(colors.fg), Print(") is up to date")
            )?;
        } else {
            // Fallback for all other menu items (and the LightRed update alert)
            let desc_color = if is_update_avail && *key == "U" {
                Color::Red
            } else {
                colors.fg
            };
            queue!(stdout, SetForegroundColor(desc_color), Print(desc))?;
        }

        queue!(stdout, ResetColor)?;
    }

    let m_col = (cols as usize / 6).max(1);
    Editor::draw_menu_line(stdout, rows - 2, cols, m_col,
                           &[(">", " Select"), ("P", " Prev"), ("", ""), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Editor::draw_menu_line(stdout, rows - 1, cols, m_col,
                           &[("Q", " Quit"), ("N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;

    if app.accounts.is_empty() {
        queue!(stdout, cursor::MoveTo(0, rows.saturating_sub(3)), SetForegroundColor(Color::Red), Print("No email account. Please type 'E' and Add an email account."), ResetColor)?;
    }
    Ok(())
}

fn draw_settings(stdout: &mut std::io::Stdout, cols: u16, rows: u16, theme_provider: &Editor, selected_idx: usize) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.menu_bg), terminal::Clear(ClearType::CurrentLine), SetForegroundColor(colors.accent), Print("xpine - Settings"), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg))?;

    let options = [
        ("    Soft Wrap", theme_provider.soft_wrap), ("    Show Line Numbers", theme_provider.show_line_numbers),
        ("    Sort Newest First", theme_provider.sort_newest_first), ("    Spellcheck Before Sending", theme_provider.spellcheck_before_send),
    ];

    for (i, (title, is_enabled)) in options.iter().enumerate() {
        let y = 1 + i as u16;
        if i == selected_idx { queue!(stdout, cursor::MoveTo(1, y), SetBackgroundColor(colors.selected_bg))?; } else { queue!(stdout, cursor::MoveTo(1, y), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg))?; }
        queue!(stdout, Print(format!("{} {:<20} ", if *is_enabled { " [X]" } else { " [ ]" }, title)), ResetColor)?;
    }

    // --- NEW: Signature Option ---
    let sig_idx = options.len();
    let sig_y = 1 + sig_idx as u16;
    if selected_idx == sig_idx {
        queue!(stdout, cursor::MoveTo(1, sig_y), SetBackgroundColor(colors.selected_bg))?;
    } else {
        queue!(stdout, cursor::MoveTo(1, sig_y), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg))?;
    }
    queue!(stdout, Print(" [>]     Edit Email Signature               "), ResetColor)?;
    // -----------------------------

    // Shifted from 2 to 3 to account for the new signature line
    let theme_y = 3 + options.len() as u16;

    queue!(stdout, cursor::MoveTo(2, theme_y), SetBackgroundColor(colors.bg), SetForegroundColor(colors.accent), Print("Meta+T"), ResetColor)?;
    queue!(stdout, cursor::MoveTo(10, theme_y), SetBackgroundColor(colors.bg), SetForegroundColor(colors.fg), Print("Theme: "), ResetColor)?;
    queue!(stdout, SetBackgroundColor(colors.bg), SetForegroundColor(colors.accent), Print(format!("{}", theme_provider.current_theme)), ResetColor)?;

    let m_col = (cols as usize / 6).max(1);
    Editor::draw_menu_line(stdout, rows - 2, cols, m_col,
                           &[("<", " Back"), ("P", " Prev"), ("X", " Select"), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Editor::draw_menu_line(stdout, rows - 1, cols, m_col,
                           &[("", ""), ("N", " Next"), ("Meta+T", " Theme"), ("", ""), ("", ""), ("", "")],
                           colors.menu_bg, colors.accent, colors.fg)?;
    Ok(())
}

