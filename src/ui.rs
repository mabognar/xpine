use std::io::{self, stdout, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::{cursor, event::{self, Event, KeyCode, KeyEventKind}, execute, queue, style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor}, terminal::{self, size as term_size, ClearType}};
use crossterm::terminal::Clear;
use syntect::easy::HighlightLines;

use crate::app::{App, AppMode};
use crate::editor::{Editor, MenuState};
pub(crate) use crate::theme::derive_ui_colors;

const DEFAULT_FOLDERS: &[&str] = &[
    "inbox",
    "sent",
    "trash",
    "archive",
    "drafts",
    "spam",
    "junk",
    "deleted",
    "outbox",
    "[gmail]",
    "conversation history",
];

pub trait UiExt {
    fn draw_menu_line(
        writer: &mut io::Stdout,
        row: u16,
        cols: u16,
        col_width: usize,
        items: &[(&str, &str)],
        ui_bg: Color,
        key_fg: Color,
        text_fg: Color,
    ) -> io::Result<()>;
    fn draw_screen(&mut self) -> io::Result<()>;
    fn show_help(&mut self, context: &str) -> io::Result<()>;
    fn set_status(&mut self, message: String);
    fn clear_status(&mut self);
}

struct ColorTracker {
    fg: Option<Color>,
    bg: Option<Color>,
}

impl ColorTracker {
    fn new() -> Self {
        Self { fg: None, bg: None }
    }

    fn set_fg(&mut self, w: &mut io::Stdout, color: Color) -> io::Result<()> {
        if self.fg != Some(color) {
            queue!(w, SetForegroundColor(color))?;
            self.fg = Some(color);
        }
        Ok(())
    }

    fn set_bg(&mut self, w: &mut io::Stdout, color: Color) -> io::Result<()> {
        if self.bg != Some(color) {
            queue!(w, SetBackgroundColor(color))?;
            self.bg = Some(color);
        }
        Ok(())
    }

    fn set(&mut self, w: &mut io::Stdout, fg: Color, bg: Color) -> io::Result<()> {
        self.set_fg(w, fg)?;
        self.set_bg(w, bg)
    }
}

fn syntect_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

fn draw_menu_pair(
    writer: &mut io::Stdout,
    rows: u16,
    cols: u16,
    top: &[(&str, &str)],
    bottom: &[(&str, &str)],
    ui_bg: Color,
    key_fg: Color,
    text_fg: Color,
) -> io::Result<()> {
    let col_width = ((cols as usize) / 6).max(1);
    Editor::draw_menu_line(
        writer,
        rows.saturating_sub(2),
        cols,
        col_width,
        top,
        ui_bg,
        key_fg,
        text_fg,
    )?;
    Editor::draw_menu_line(
        writer,
        rows.saturating_sub(1),
        cols,
        col_width,
        bottom,
        ui_bg,
        key_fg,
        text_fg,
    )
}

fn begin_header_bar(
    stdout: &mut io::Stdout,
    title: &str,
    menu_bg: Color,
    accent: Color,
) -> io::Result<()> {
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        SetBackgroundColor(menu_bg),
        terminal::Clear(ClearType::CurrentLine),
        SetForegroundColor(accent),
        Print(title)
    )
}

fn draw_header_bar(
    stdout: &mut io::Stdout,
    title: &str,
    menu_bg: Color,
    accent: Color,
) -> io::Result<()> {
    begin_header_bar(stdout, title, menu_bg, accent)?;
    queue!(stdout, ResetColor)
}

fn draw_status_bar(
    stdout: &mut io::Stdout,
    rows: u16,
    text: &str,
    menu_bg: Color,
    accent: Color,
) -> io::Result<()> {
    queue!(
        stdout,
        cursor::MoveTo(0, rows.saturating_sub(3)),
        SetBackgroundColor(menu_bg),
        terminal::Clear(ClearType::UntilNewLine),
        SetForegroundColor(accent),
        Print(text),
        ResetColor
    )
}

fn scroll_window_start(selected_idx: usize, visible_items: usize) -> usize {
    if visible_items > 0 && selected_idx >= visible_items {
        selected_idx - visible_items + 1
    } else {
        0
    }
}

fn is_custom_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    !DEFAULT_FOLDERS.iter().any(|def| lower.contains(def))
}

enum HelpLine {
    Title(&'static str),
    Text(&'static str),
    Header(&'static str),
    Cmd1(&'static str, &'static str),
    Cmd2(&'static str, &'static str, &'static str, &'static str),
    Cmd3(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ),
    Blank,
}

fn help_lines_for(context: &str) -> Vec<HelpLine> {
    use HelpLine::*;
    match context {
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
            Cmd1("V, Spc, PgDn", "Page Down"),
            Cmd1("<, Left, Esc", "Return to the Main Menu"),
            Blank,
            Header("ACTIONS"),
            Cmd1("A", "Add a new email address"),
            Cmd1("T", "Create a new Team (group distribution list)"),
            Cmd1("E", "Edit the currently selected entry"),
            Cmd1("D", "Delete the currently selected entry"),
            Cmd1("I", "Import email list from text file. Format: "),
            Cmd1("", "   1. emails are separated by commas, or"),
            Cmd1("", "   2. emails on separate lines"),
            Cmd1("", "Each email must be formatted as:"),
            Cmd1("", "   1. Full Name <useremail@example.com> "),
            Cmd1("", "            ---> email with name - do not remove <>"),
            Cmd1("", "   2. \"Full Name\" <useremail@example.com> "),
            Cmd1("", "            ---> name in quotes - no not remove <>"),
            Cmd1("", "   3. useremail@example.com"),
            Cmd1("", "            ---> email with no name"),
        ],
        "email_accounts" => vec![
            Title("xpine - Email Accounts Help"),
            Blank,
            Text("Configure standard IMAP and Microsoft Graph API accounts."),
            Blank,
            Header("NAVIGATION:"),
            Cmd1("P, Up", "Move cursor up"),
            Cmd1("N, Down", "Move cursor down"),
            Cmd1("<", "Return to the Main Menu"),
            Blank,
            Header("ACTIONS:"),
            Cmd1("A", "Add a new email account"),
            Cmd1("", "You will be presented with 4 account types."),
            Cmd1("", "   1. Microsoft Graph API account (Exchange, Outlook, "),
            Cmd1("", "      Hotmail) with OAuth 2.0 modern authentication"),
            Cmd1("", "   2. Google Gmail account with OAuth 2.0 "),
            Cmd1("", "      modern authentication"),
            Cmd1("", "   3. Google Gmail account with Application "),
            Cmd1("", "      Password (generate online - enter app "),
            Cmd1("", "      password WITHOUT spaces)"),
            Cmd1("", "   4. Basic IMAP, Legacy account using password access"),
            Cmd1("", "Note: Some employers/schools restrict access of 3rd"),
            Cmd1("", "      party email applications with Microsoft accounts - "),
            Cmd1("", "      ask your administrator to allow xpine "),
            Cmd1("", "Note: Yahoo requires application passwords - generate"),
            Cmd1("", "      online and use \"Basic IMAP\" when adding "),
            Cmd1("", "      account"),
            Cmd1("D", "Delete the currently selected account"),
            Cmd1("+, Shift+Up", "Move selected account up in list"),
            Cmd1("-, Shift+Down", "Move selected account down in list"),
            Cmd1("", "Note: Account 1 opens by default on startup. Press"),
            Cmd1("", "      1, 2, ... in the Email List to switch accounts."),
            Cmd1("", ""),
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
        "email_reader" => vec![
            Title("xpine - Email Reader Help"),
            Blank,
            Text("This screen allows you to read an email and manage its attachments."),
            Blank,
            Header("NAVIGATION (PAGE 1)"),
            Cmd1("Y, PgUp", "Page Up"),
            Cmd1("V, Spc, PgDn", "Page Down"),
            Cmd1("^P", "Read previous message"),
            Cmd1("^N", "Read next message"),
            Cmd1("<, Left, Esc", "Go Back to folder"),
            Blank,
            Header("ACTIONS (PAGE 1)"),
            Cmd1("R", "Reply to email"),
            Cmd1("F", "Forward email"),
            Cmd1("D", "Mark email for deletion"),
            Cmd1("A", "Add sender/recipients to Address Book"),
            Cmd1("B", "Open HTML/Text version in Web Browser"),
            Cmd1("O", "Toggle Menu Page"),
            Blank,
            Header("NAVIGATION & ACTIONS (PAGE 2)"),
            Cmd1("P, Up", "Scroll up one line"),
            Cmd1("N, Down", "Scroll down one line"),
            Cmd1("1-9", "Open Attachment # in default app"),
            Cmd1("Alt+1-9", "Save Attachment # to disk"),
            Cmd1("Alt+0", "Save ALL attachments to a folder"),
            Cmd1("S", "Save the email text to disk"),
            Cmd1("^Y", "Copy email text to clipboard"),
        ],
        _ => vec![
            Title("xpine - Help"),
            Text("No specific help documentation is available for this screen."),
        ],
    }
}

impl UiExt for Editor {
    fn draw_menu_line(
        writer: &mut io::Stdout,
        row: u16,
        _cols: u16,
        col_width: usize,
        items: &[(&str, &str)],
        ui_bg: Color,
        key_fg: Color,
        text_fg: Color,
    ) -> io::Result<()> {
        queue!(
            writer,
            cursor::MoveTo(0, row),
            SetBackgroundColor(ui_bg),
            terminal::Clear(ClearType::CurrentLine)
        )?;

        let safe_col_width = col_width.max(1) as u16;
        let mut current_x = 0;

        for (cmd, desc) in items {
            let cmd_chars = cmd.chars().count() as u16;
            let desc_chars = desc.chars().count() as u16;

            queue!(
                writer,
                cursor::MoveTo(current_x, row),
                SetForegroundColor(key_fg),
                Print(cmd),
                SetForegroundColor(text_fg)
            )?;

            if cmd_chars + desc_chars <= safe_col_width {
                queue!(writer, Print(desc))?;
            } else {
                let take = safe_col_width.saturating_sub(cmd_chars) as usize;
                let safe_desc: String = desc.chars().take(take).collect();
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
        queue!(stdout, cursor::Hide)?;

        let has_status = !self.status_message.is_empty();
        let visible_rows = rows.saturating_sub(2 + u16::from(has_status) + self.top_margin) as usize;

        let theme = &self.theme_set.themes[&self.current_theme];
        let ui_colors = derive_ui_colors(theme);
        let is_dark = ui_colors.is_dark;
        let default_cross_bg = syntect_color(
            theme
                .settings
                .background
                .unwrap_or(syntect::highlighting::Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
        );
        let ui_bg = ui_colors.menu_bg;
        let title_fg = if is_dark { Color::Reset } else { Color::Black };
        let menu_key_fg = ui_colors.accent;
        let menu_text_fg = ui_colors.fg;
        let dollar_bg = if is_dark {
            Color::Rgb { r: 180, g: 180, b: 180 }
        } else {
            Color::Rgb { r: 80, g: 80, b: 80 }
        };
        let dollar_fg = if is_dark { Color::Black } else { Color::White };

        if matches!(
            self.menu_state,
            MenuState::EmailComposer | MenuState::EmailReader
        ) || self.top_margin > 0
        {
            queue!(
                stdout,
                cursor::MoveTo(0, self.top_margin),
                SetBackgroundColor(default_cross_bg),
                terminal::Clear(ClearType::CurrentLine)
            )?;
        } else {
            queue!(
                stdout,
                cursor::MoveTo(0, self.top_margin),
                SetBackgroundColor(ui_bg)
            )?;
        }

        let syntax = self
            .filename
            .as_deref()
            .and_then(|name| Path::new(name).extension().and_then(|s| s.to_str()))
            .and_then(|ext| self.syntax_set.find_syntax_by_extension(ext))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let max_line_num_len = self.buffer.len_lines().to_string().len();
        let gutter_width = if self.show_line_numbers {
            max_line_num_len + 1
        } else {
            0
        };
        let available_width = (cols as usize).saturating_sub(gutter_width).max(1);
        let cursor_absolute = self.get_cursor_char_idx();
        let mark_range = self
            .mark
            .map(|m| (m.min(cursor_absolute), m.max(cursor_absolute)));

        let mut pen = ColorTracker::new();
        let mut fallback_highlighter = None;
        let mut terminal_y = 0;
        let mut file_y = self.row_offset;

        while terminal_y < visible_rows {
            if file_y < self.buffer.len_lines() {
                let line_start_char = self.buffer.line_to_char(file_y);
                let ranges = &*self.highlight_cache.entry(file_y).or_insert_with(|| {
                    let highlighter = fallback_highlighter
                        .get_or_insert_with(|| HighlightLines::new(syntax, theme));
                    let line_str = self.buffer.line(file_y).to_string();
                    highlighter
                        .highlight_line(&line_str, &self.syntax_set)
                        .unwrap()
                        .into_iter()
                        .map(|(s, t)| (s, t.to_string()))
                        .collect()
                });

                let mut visual_x = 0;
                let mut line_char_idx = 0;
                let mut printed_on_current_line = 0;
                let search_range = self
                    .highlight_match
                    .and_then(|(h_y, start, end)| (h_y == file_y).then_some((start, end)));

                queue!(stdout, cursor::MoveTo(0, terminal_y as u16 + self.top_margin))?;
                if self.show_line_numbers {
                    pen.set(&mut stdout, menu_key_fg, default_cross_bg)?;
                    queue!(
                        stdout,
                        Print(format!("{:>width$} ", file_y + 1, width = max_line_num_len))
                    )?;
                }

                'char_loop: for (style, text) in ranges {
                    let cross_fg = syntect_color(style.foreground);
                    let cross_bg = syntect_color(style.background);
                    pen.set(&mut stdout, cross_fg, cross_bg)?;

                    for ch in text.chars() {
                        if ch == '\n' || ch == '\r' {
                            line_char_idx += 1;
                            continue;
                        }

                        let is_highlighted = if let Some((h_start, h_end)) = search_range {
                            line_char_idx >= h_start && line_char_idx < h_end
                        } else if let Some((m_start, m_end)) = mark_range {
                            let abs_idx = line_start_char + line_char_idx;
                            abs_idx >= m_start && abs_idx < m_end
                        } else {
                            false
                        };

                        let (display_ch, repeat_count) = if ch == '\t' {
                            (' ', 4 - (visual_x % 4))
                        } else {
                            (ch, 1)
                        };

                        for _ in 0..repeat_count {
                            if self.soft_wrap {
                                if printed_on_current_line >= available_width {
                                    pen.set_bg(&mut stdout, default_cross_bg)?;
                                    queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;
                                    terminal_y += 1;
                                    if terminal_y >= visible_rows {
                                        break 'char_loop;
                                    }

                                    queue!(
                                        stdout,
                                        cursor::MoveTo(0, terminal_y as u16 + self.top_margin)
                                    )?;
                                    if self.show_line_numbers {
                                        queue!(stdout, Print(" ".repeat(gutter_width)))?;
                                    }
                                    pen.set(&mut stdout, cross_fg, cross_bg)?;
                                    printed_on_current_line = 0;
                                }

                                if is_highlighted {
                                    pen.set(&mut stdout, Color::White, Color::Red)?;
                                }
                                queue!(stdout, Print(display_ch))?;
                                if is_highlighted {
                                    pen.set(&mut stdout, cross_fg, cross_bg)?;
                                }
                                printed_on_current_line += 1;
                            } else if visual_x >= self.col_offset
                                && printed_on_current_line < available_width
                            {
                                if is_highlighted {
                                    pen.set(&mut stdout, Color::White, Color::Red)?;
                                }
                                queue!(stdout, Print(display_ch))?;
                                if is_highlighted {
                                    pen.set(&mut stdout, cross_fg, cross_bg)?;
                                }
                                printed_on_current_line += 1;
                            }
                            visual_x += 1;
                        }
                        line_char_idx += 1;
                    }
                }

                pen.set_bg(&mut stdout, default_cross_bg)?;
                queue!(stdout, terminal::Clear(ClearType::UntilNewLine))?;

                if !self.soft_wrap {
                    let row_y = terminal_y as u16 + self.top_margin;
                    if self.col_offset > 0 {
                        pen.set(&mut stdout, dollar_fg, dollar_bg)?;
                        queue!(stdout, cursor::MoveTo(gutter_width as u16, row_y), Print('$'))?;
                    }
                    if visual_x > self.col_offset + available_width {
                        pen.set(&mut stdout, dollar_fg, dollar_bg)?;
                        queue!(stdout, cursor::MoveTo(cols.saturating_sub(1), row_y), Print('$'))?;
                    }
                }
                pen.set(&mut stdout, Color::Reset, default_cross_bg)?;
            } else {
                pen.set_bg(&mut stdout, default_cross_bg)?;
                queue!(
                    stdout,
                    cursor::MoveTo(0, terminal_y as u16 + self.top_margin),
                    terminal::Clear(ClearType::CurrentLine)
                )?;
            }
            terminal_y += 1;
            file_y += 1;
        }

        if has_status {
            queue!(
                stdout,
                cursor::MoveTo(0, rows.saturating_sub(3)),
                SetBackgroundColor(ui_bg),
                SetForegroundColor(menu_key_fg)
            )?;
            let mut printed_len = 0;

            if self.menu_state == MenuState::SpellCheck {
                if !self.current_suggestions.is_empty() {
                    for (i, sug) in self.current_suggestions.iter().enumerate() {
                        let num_str = (i + 1).to_string();
                        queue!(
                            stdout,
                            SetForegroundColor(menu_key_fg),
                            Print(&num_str),
                            SetForegroundColor(title_fg),
                            Print(format!(" {}   ", sug))
                        )?;
                        printed_len += num_str.len() + 1 + sug.chars().count() + 3;
                    }
                } else {
                    queue!(stdout, Print("No suggestions   "))?;
                    printed_len += "No suggestions   ".len();
                }
            }

            queue!(
                stdout,
                Print(&self.status_message),
                Print(
                    " ".repeat(
                        (cols as usize).saturating_sub(printed_len + self.status_message.chars().count())
                    )
                ),
                SetBackgroundColor(Color::Reset),
                SetForegroundColor(Color::Reset)
            )?;
        }

        if self.menu_state == MenuState::SpellCheck {
            let sug = |i: usize| {
                self.current_suggestions
                    .get(i)
                    .map(String::as_str)
                    .unwrap_or("")
            };
            let top = [
                ("1", sug(0)),
                ("2", sug(1)),
                ("3", sug(2)),
                ("4", sug(3)),
                ("5", sug(4)),
            ];
            let bottom = [
                ("^C", "Cancel"),
                ("A", "Add to Dict"),
                ("I", "Ignore"),
                ("", ""),
                ("", ""),
            ];
            draw_menu_pair(
                &mut stdout,
                rows,
                cols,
                &top,
                &bottom,
                ui_bg,
                menu_key_fg,
                menu_text_fg,
            )?;
        } else {
            let (top, bottom): ([(&str, &str); 6], [(&str, &str); 6]) = match self.menu_state {
                MenuState::EmailComposer if self.menu_page == 1 => (
                    [
                        ("^X", " Send"),
                        ("^P", " Prev"),
                        ("^Y", " Prev Pg"),
                        ("^K", " Cut"),
                        ("^J", " Justify"),
                        ("^O", " Other 1/2"),
                    ],
                    [
                        ("^C", " Cancel"),
                        ("^N", " Next"),
                        ("^V", " Next Pg"),
                        ("^U", " UnCut"),
                        ("^A", " Attach"),
                        ("^G", " Signature"),
                    ],
                ),
                MenuState::EmailComposer => (
                    [
                        ("^R", " Read File"),
                        (" ^T", " To Spell"),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                        ("^O", " Other 2/2"),
                    ],
                    [
                        ("^W", " Where is"),
                        ("M+A", " Mark"),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                    ],
                ),
                MenuState::EmailReader if self.menu_page == 1 => (
                    [
                        ("<", " Back"),
                        ("R", " Reply"),
                        ("  Y", " Prv Pg"),
                        ("^P", " Prv Msg"),
                        ("D", " Delete"),
                        ("O", " Other (1/2)"),
                    ],
                    [
                        ("B", " Browser"),
                        ("F", " Forward"),
                        ("Spc", " Nxt Pg"),
                        ("^N", " Nxt Msg"),
                        ("A", " Add Addr"),
                        ("H", " Help"),
                    ],
                ),
                MenuState::EmailReader => (
                    [
                        ("P", " Scroll Up"),
                        ("^R", " ReplyAll"),
                        ("E", " ExpandHdr"),
                        ("", ""),
                        ("", ""),
                        ("O", " Other (2/2)"),
                    ],
                    [
                        ("N", " Scroll Dn"),
                        (" S", " SaveMsg"),
                        ("", ""),
                        ("", ""),
                        ("", ""),
                        ("H", " Help"),
                    ],
                ),
                MenuState::YesNoCancel => (
                    [("Y", " Yes"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    [("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")],
                ),
                MenuState::ReplaceAction => (
                    [("Y", " Yes"), ("A", " All"), ("", ""), ("", ""), ("", ""), ("", "")],
                    [("N", " No"), ("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", "")],
                ),
                MenuState::PromptWithBrowser => (
                    [("^T", " To Files"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    [("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                ),
                MenuState::CancelOnly => (
                    [("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    [("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                ),
                MenuState::TeamEditor => (
                    [
                        ("^X", " Save"),
                        ("^C", " Cancel"),
                        ("^P", " Prev"),
                        ("^Y", " Prev Pg"),
                        ("^A", " Add Email"),
                        ("", ""),
                    ],
                    [
                        ("", ""),
                        ("", ""),
                        ("^N", " Next"),
                        ("^V", " Next Pg"),
                        ("", ""),
                        ("", ""),
                    ],
                ),
                MenuState::SignatureEditor => (
                    [("^X", " Save"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                    [("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")],
                ),
                MenuState::SpellCheck => unreachable!(),
            };
            draw_menu_pair(
                &mut stdout,
                rows,
                cols,
                &top,
                &bottom,
                ui_bg,
                menu_key_fg,
                menu_text_fg,
            )?;
        }

        let visual_x = self.get_visual_cursor_x();
        let display_x = visual_x.saturating_sub(self.col_offset);
        let final_cursor_x = if self.soft_wrap {
            gutter_width as u16 + (display_x % available_width) as u16
        } else {
            gutter_width as u16 + display_x as u16
        };

        let final_cursor_y = if self.soft_wrap {
            let mut virtual_y = 0;
            for i in self.row_offset..self.cursor_y {
                let w = self.get_visual_line_width(i);
                virtual_y += w.saturating_sub(1) / available_width + 1;
            }
            virtual_y += visual_x / available_width;
            virtual_y as u16 + self.top_margin
        } else {
            self.cursor_y.saturating_sub(self.row_offset) as u16 + self.top_margin
        };

        if self.menu_state == MenuState::EmailReader {
            queue!(stdout, cursor::Hide)?;
        } else {
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
        let help_content = help_lines_for(context);

        let mut stdout = stdout();
        let mut scroll_offset = 0;

        loop {
            let (cols, rows) = terminal::size()?;
            let visible_lines = (rows as usize).saturating_sub(3);
            let max_scroll = help_content.len().saturating_sub(visible_lines);
            scroll_offset = scroll_offset.min(max_scroll);

            queue!(stdout, SetBackgroundColor(colors.bg), terminal::Clear(ClearType::All))?;

            for (i, line) in help_content
                .iter()
                .skip(scroll_offset)
                .take(visible_lines)
                .enumerate()
            {
                queue!(stdout, cursor::MoveTo(0, i as u16))?;

                match line {
                    HelpLine::Title(t) => {
                        let display_title = t.replace(
                            "xpine - ",
                            &format!("xpine ({}) - ", env!("CARGO_PKG_VERSION")),
                        );
                        queue!(
                            stdout,
                            SetBackgroundColor(colors.menu_bg),
                            terminal::Clear(ClearType::CurrentLine),
                            SetForegroundColor(colors.accent),
                            Print(&display_title),
                            ResetColor,
                            SetBackgroundColor(colors.bg)
                        )?;
                    }
                    HelpLine::Header(h) => {
                        queue!(stdout, SetForegroundColor(colors.date_color), Print(*h))?;
                    }
                    HelpLine::Text(t) => {
                        queue!(stdout, SetForegroundColor(colors.fg), Print(*t))?;
                    }
                    HelpLine::Blank => {}
                    HelpLine::Cmd1(k, d) => {
                        queue!(
                            stdout,
                            Print("  "),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<15}", k)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {}", d))
                        )?;
                    }
                    HelpLine::Cmd2(k1, d1, k2, d2) => {
                        queue!(
                            stdout,
                            Print("  "),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<4}", k1)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {:<15}", d1)),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<4}", k2)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {}", d2))
                        )?;
                    }
                    HelpLine::Cmd3(k1, d1, k2, d2, k3, d3) => {
                        queue!(
                            stdout,
                            Print("  "),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<4}", k1)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {:<13}", d1)),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<4}", k2)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {:<13}", d2)),
                            SetForegroundColor(colors.accent),
                            Print(format!("{:<4}", k3)),
                            SetForegroundColor(colors.fg),
                            Print(format!(" {}", d3))
                        )?;
                    }
                }
            }

            draw_menu_pair(
                &mut stdout,
                rows,
                cols,
                &[("<", " Back"), ("P", " Prev"), ("Y", " Prev Pg"), ("", ""), ("", ""), ("", "")],
                &[("", ""), ("N", " Next"), ("V", " Next Pg"), ("", ""), ("", ""), ("", "")],
                colors.menu_bg,
                colors.accent,
                colors.fg,
            )?;

            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc | KeyCode::Left | KeyCode::Char('<') => break,
                        KeyCode::Up | KeyCode::Char('p' | 'P') => {
                            scroll_offset = scroll_offset.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('n' | 'N') => {
                            scroll_offset = (scroll_offset + 1).min(max_scroll);
                        }
                        KeyCode::PageUp | KeyCode::Char('y' | 'Y') => {
                            scroll_offset = scroll_offset.saturating_sub(visible_lines);
                        }
                        KeyCode::PageDown | KeyCode::Char('v' | 'V') => {
                            scroll_offset = (scroll_offset + visible_lines).min(max_scroll);
                        }
                        _ => {}
                    }
                }
            }
        }
        execute!(stdout, SetBackgroundColor(colors.bg), Clear(ClearType::All))?;
        // execute!(stdout, Clear(ClearType::All))?;
        Ok(())
    }

    fn set_status(&mut self, message: String) {
        self.status_message = message;
        self.status_time = Some(Instant::now());
    }

    fn clear_status(&mut self) {
        self.status_message.clear();
        self.status_time = None;
    }
}

pub fn draw_app(
    stdout: &mut io::Stdout,
    app: &App,
    theme_provider: &Editor,
) -> io::Result<()> {
    let (cols, rows) = term_size().unwrap_or((80, 24));
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    // REMOVE Clear(ClearType::All) from here:
    queue!(
        stdout,
        cursor::Hide,
        SetBackgroundColor(colors.bg)
    )?;

    match &app.mode {
        AppMode::AddressBook {
            selected_idx,
            addresses,
        } => draw_address_book(stdout, cols, rows, theme_provider, *selected_idx, addresses)?,
        AppMode::EmailAccounts { selected_idx } => {
            draw_email_accounts(stdout, app, cols, rows, theme_provider, *selected_idx)?;
        }
        AppMode::EmailList => draw_email_list(stdout, app, cols, rows, theme_provider)?,
        AppMode::FolderList {
            step,
            selected_idx,
            folders,
        } => draw_folder_list(
            stdout,
            app,
            cols,
            rows,
            theme_provider,
            *step as usize,
            *selected_idx,
            folders,
        )?,
        AppMode::MainMenu { selected_idx } => {
            draw_main_menu(stdout, app, cols, rows, theme_provider, *selected_idx)?;
        }
        AppMode::Settings { selected_idx } => {
            draw_settings(stdout, cols, rows, theme_provider, *selected_idx)?;
        }
        AppMode::EmailRead { .. } => {} // Rendered completely in src/read.rs
    }

    // if !theme_provider.status_message.is_empty()
    //     && theme_provider
    //     .status_time
    //     .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
    // {
    //     draw_status_bar(
    //         stdout,
    //         rows,
    //         &format!(" {} ", theme_provider.status_message),
    //         colors.menu_bg,
    //         colors.accent,
    //     )?;
    // }

    if !theme_provider.status_message.is_empty()
        && theme_provider
        .status_time
        .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
    {
        draw_status_bar(
            stdout,
            rows,
            &format!(" {} ", theme_provider.status_message),
            colors.menu_bg,
            colors.accent,
        )?;
    } else if !matches!(app.mode, AppMode::EmailList)
        && !(matches!(app.mode, AppMode::MainMenu { .. }) && app.accounts.is_empty())
    {
        queue!(
            stdout,
            cursor::MoveTo(0, rows.saturating_sub(3)),
            SetBackgroundColor(colors.bg),
            terminal::Clear(ClearType::CurrentLine),
            ResetColor
        )?;
    }

    stdout.flush()?;
    Ok(())
}

fn draw_address_book(
    stdout: &mut io::Stdout,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
    selected_idx: usize,
    addresses: &[String],
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    draw_header_bar(
        stdout,
        &format!("xpine ({}) - Address Book", env!("CARGO_PKG_VERSION")),
        colors.menu_bg,
        colors.accent,
    )?;

    let items_per_page = (rows.saturating_sub(4) as usize).max(1);
    let start_idx = scroll_window_start(selected_idx, items_per_page);

    for i in 0..items_per_page {
        let actual_idx = start_idx + i;
        if actual_idx >= addresses.len() {
            break;
        }

        let bg_color = if actual_idx == selected_idx {
            colors.selected_bg
        } else {
            colors.bg
        };
        let display_str = &addresses[actual_idx];
        let padding = "  ";

        queue!(
            stdout,
            cursor::MoveTo(0, (i + 1) as u16),
            SetBackgroundColor(bg_color),
            terminal::Clear(ClearType::CurrentLine),
            SetForegroundColor(colors.fg),
            Print(padding)
        )?;

        if let Some((team_name, emails)) = display_str.split_once(':') {
            let member_count = crate::address::split_unquoted_commas(emails).len();
            let team_name_with_count = format!("{} ({})", team_name, member_count);
            let prefix_len = padding.len() + team_name_with_count.chars().count() + 1;
            let available_len = (cols as usize).saturating_sub(prefix_len);

            let emails_display = if emails.chars().count() > available_len {
                let take_len = available_len.saturating_sub(3);
                format!("{}...", emails.chars().take(take_len).collect::<String>())
            } else {
                emails.to_string()
            };

            queue!(
                stdout,
                SetForegroundColor(colors.accent),
                Print(team_name_with_count),
                SetForegroundColor(colors.fg),
                Print(":"),
                Print(emails_display)
            )?;
        } else if let Some(start) = display_str.find('<').filter(|_| display_str.contains('>')) {
            queue!(
                stdout,
                SetForegroundColor(colors.date_color),
                Print(&display_str[..start]),
                SetForegroundColor(colors.fg),
                Print(&display_str[start..])
            )?;
        } else {
            queue!(stdout, SetForegroundColor(colors.fg), Print(display_str))?;
        }
    }

    draw_menu_pair(
        stdout,
        rows,
        cols,
        &[
            ("<", " Back"),
            ("P", " Prev"),
            ("  Y", " Prev Pg"),
            ("A", " Add Email"),
            ("E", " Edit"),
            ("I", " Import"),
        ],
        &[
            ("M", " Main Menu"),
            ("N", " Next"),
            ("Spc", " Next Pg"),
            ("T", " Add Team"),
            ("D", " Delete"),
            ("H", " Help"),
        ],
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;

    queue!(stdout, cursor::Hide)?;
    Ok(())
}

fn draw_email_accounts(
    stdout: &mut io::Stdout,
    app: &App,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
    selected_idx: usize,
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    draw_header_bar(
        stdout,
        &format!("xpine ({}) - Email Accounts", env!("CARGO_PKG_VERSION")),
        colors.menu_bg,
        colors.accent,
    )?;

    let items_per_page = (rows.saturating_sub(4) as usize).max(1);
    let start_idx = scroll_window_start(selected_idx, items_per_page);

    for i in 0..items_per_page {
        let actual_idx = start_idx + i;
        let row_y = (i + 1) as u16;
        // if let Some(acc) = app.accounts.get(actual_idx) {
        //     let bg_color = if actual_idx == selected_idx {
        //         colors.selected_bg
        //     } else {
        //         colors.bg
        //     };
        //     queue!(
        //         stdout,
        //         cursor::MoveTo(0, row_y),
        //         SetBackgroundColor(bg_color),
        //         terminal::Clear(ClearType::CurrentLine),
        //         SetForegroundColor(colors.fg),
        //         Print(format!("  {}", acc.email)),
        //         ResetColor
        //     )?;
        // }
        if let Some(acc) = app.accounts.get(actual_idx) {
            let bg_color = if actual_idx == selected_idx {
                colors.selected_bg
            } else {
                colors.bg
            };
            queue!(
                stdout,
                cursor::MoveTo(0, row_y),
                SetBackgroundColor(bg_color),
                terminal::Clear(ClearType::CurrentLine),
                SetForegroundColor(colors.accent),
                Print(format!("  {}. ", actual_idx + 1)),
                SetForegroundColor(colors.fg),
                Print(&acc.email),
                ResetColor
            )?;
        } else {
            queue!(
                stdout,
                cursor::MoveTo(0, row_y),
                SetBackgroundColor(colors.bg),
                terminal::Clear(ClearType::UntilNewLine)
            )?;
        }
    }

    // draw_menu_pair(
    //     stdout,
    //     rows,
    //     cols,
    //     &[("<", " Back"), ("A", " Add Acct"), ("P", " Prev"), ("", ""), ("", ""), ("", "")],
    //     &[("M", " Main Menu"), ("D", " Del Acct"), ("N", " Next"), ("", ""), ("", ""), ("", "")],
    //     colors.menu_bg,
    //     colors.accent,
    //     colors.fg,
    // )?;
    draw_menu_pair(
        stdout,
        rows,
        cols,
        &[
            ("<", " Back"),
            ("A", " Add Acct"),
            ("P", " Prev"),
            ("+", " Move Up"),
            ("", ""),
            ("H", " Help"),
        ],
        &[
            ("M", " Main Menu"),
            ("D", " Del Acct"),
            ("N", " Next"),
            ("-", " Move Dn"),
            ("", ""),
            ("", ""),
        ],
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;

    queue!(
        stdout,
        SetBackgroundColor(colors.bg),
        SetForegroundColor(colors.accent),
        cursor::MoveTo(0, rows.saturating_sub(7)),
        Print("  - App Specific Password is required for Google & Yahoo"),
        cursor::MoveTo(0, rows.saturating_sub(6)),
        Print("  - Go online to generate App Specific Password"),
        cursor::MoveTo(0, rows.saturating_sub(5)),
        Print("  - Enter the App Specific Password WITHOUT spaces"),
        ResetColor
    )?;
    Ok(())
}

fn draw_email_list(
    stdout: &mut io::Stdout,
    app: &App,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    begin_header_bar(
        stdout,
        &format!(
            "xpine ({}) - {} ({})",
            env!("CARGO_PKG_VERSION"),
            app.current_folder,
            app.active_account.email
        ),
        colors.menu_bg,
        colors.accent,
    )?;

    if let Some(ref query) = app.search_query {
        queue!(
            stdout,
            SetForegroundColor(colors.flag_star),
            Print(format!("   Search: {}", query))
        )?;
    }

    let items_per_page = (rows.saturating_sub(3) as u32).max(1);

    if app.total_messages == 0 || app.page_emails.is_empty() {
        let no_msg = "No messages";
        queue!(
            stdout,
            cursor::MoveTo(cols.saturating_sub(no_msg.len() as u16), 0),
            SetForegroundColor(colors.accent),
            Print(no_msg)
        )?;
    } else {
        let mut end_idx = app
            .total_messages
            .saturating_sub(app.current_page * items_per_page);
        let start_idx = end_idx
            .saturating_sub(items_per_page.saturating_sub(1))
            .max(1);
        if start_idx == 1 {
            end_idx = items_per_page.min(app.total_messages);
        }

        let msg_x = if theme_provider.sort_newest_first {
            end_idx.saturating_sub(app.selected_index as u32)
        } else {
            let page_start = end_idx
                .saturating_sub(app.page_emails.len() as u32)
                .saturating_add(1);
            page_start + app.selected_index as u32
        };

        let x_str = msg_x.to_string();
        let y_str = app.total_messages.to_string();
        let total_len = 8 + x_str.len() + 4 + y_str.len();

        queue!(
            stdout,
            cursor::MoveTo(cols.saturating_sub(total_len as u16), 0),
            SetForegroundColor(colors.accent),
            Print("Message "),
            SetForegroundColor(colors.fg),
            Print(&x_str),
            SetForegroundColor(colors.accent),
            Print(" of "),
            SetForegroundColor(colors.fg),
            Print(&y_str)
        )?;
    }

    queue!(stdout, ResetColor)?;

    let (base_r, base_g, base_b) = match colors.fg {
        Color::Rgb { r, g, b } => (r as f32, g as f32, b as f32),
        _ => (255.0, 255.0, 255.0),
    };
    let (hot_r, hot_g, hot_b) = if colors.is_dark {
        (255.0, 80.0, 80.0)
    } else {
        (220.0, 0.0, 0.0)
    };

    let visible_capacity = rows.saturating_sub(3) as usize;
    let subject_width = (cols as usize).saturating_sub(44);

    for (i, email) in app.page_emails.iter().take(visible_capacity).enumerate() {
        let row_y = (i + 1) as u16;
        let row_bg = if i == app.selected_index {
            colors.selected_bg
        } else {
            colors.bg
        };

        let flag_char = if email.is_flagged { "*" } else { " " };
        let (status_char, status_color) = if email.is_deleted {
            ("D", colors.flag_d)
        } else if !email.is_read {
            ("N", colors.flag_n)
        } else if email.is_answered {
            ("A", colors.flag_a)
        } else {
            (" ", colors.fg)
        };
        let attach_char = if email.has_attachments { "&" } else { " " };

        let size_kb = (email.size / 1024).max(1) as f32;
        let size_str = if size_kb < 1024.0 {
            format!("{}K", size_kb as u32)
        } else {
            format!("{}M", (size_kb / 1024.0) as u32)
        };
        let heat = (size_kb.log2() / 12.3).clamp(0.0, 1.0);
        let size_color = Color::Rgb {
            r: (base_r + (hot_r - base_r) * heat) as u8,
            g: (base_g + (hot_g - base_g) * heat) as u8,
            b: (base_b + (hot_b - base_b) * heat) as u8,
        };

        let from_truncated: String = email.from.chars().take(23).collect();
        let subj_truncated: String = email.subject.chars().take(subject_width).collect();

        queue!(
            stdout,
            cursor::MoveTo(0, row_y),
            SetBackgroundColor(row_bg),
            terminal::Clear(ClearType::UntilNewLine),
            SetForegroundColor(colors.flag_star),
            Print(flag_char),
            SetForegroundColor(status_color),
            Print(status_char),
            SetForegroundColor(Color::Cyan),
            Print(attach_char),
            SetForegroundColor(colors.date_color),
            Print(format!("{:>9}  ", email.date)),
            SetForegroundColor(colors.fg),
            Print(format!("{:<23}  {:<width$}", from_truncated, subj_truncated, width = subject_width)),
            cursor::MoveTo(cols.saturating_sub(4), row_y),
            SetForegroundColor(size_color),
            Print(format!("{:>4}", size_str))
        )?;
    }

    let rendered = app.page_emails.len().min(visible_capacity);
    for i in rendered..visible_capacity {
        queue!(
            stdout,
            cursor::MoveTo(0, (i + 1) as u16),
            SetBackgroundColor(colors.bg),
            terminal::Clear(ClearType::CurrentLine)
        )?;
    }

    let (top, bottom) = if app.menu_page == 1 {
        (
            [
                ("<", " Back"),
                (">", " View"),
                ("C", " Compose"),
                ("R", " Reply"),
                ("D", " Delete"),
                ("O", " Other (1/2)"),
            ],
            [
                ("M", " Main Menu"),
                ("*", " Flag"),
                ("S", " Search"),
                ("F", " Forward"),
                ("X", " Expunge"),
                ("Tab", " Acct"),
            ],
        )
    } else {
        (
            [
                ("", ""),
                ("^R", " ReplyAll"),
                ("P", " Prev"),
                ("Y", " Prev Pg"),
                ("M+T", " Theme"),
                ("O", " Other (2/2)"),
            ],
            [
                (" Q", " Quit"),
                (" U", " Un-read"),
                ("N", " Next"),
                ("V", " Next Pg"),
                ("M+M", " Move To"),
                ("H", " Help"),
            ],
        )
    };
    draw_menu_pair(
        stdout,
        rows,
        cols,
        &top,
        &bottom,
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;

    if !app.list_status.is_empty()
        && app
        .list_status_time
        .is_some_and(|t| t.elapsed() < app.list_status_duration)
    {
        draw_status_bar(
            stdout,
            rows,
            &format!("{} ", app.list_status),
            colors.menu_bg,
            colors.accent,
        )?;
    }

    queue!(stdout, cursor::Hide)?;
    Ok(())
}

fn draw_folder_list(
    stdout: &mut io::Stdout,
    app: &App,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
    step: usize,
    selected_idx: usize,
    folders: &[String],
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let header_title = if step == 0 {
        format!("xpine ({}) - Select Account", env!("CARGO_PKG_VERSION"))
    } else {
        format!(
            "xpine ({}) - Folders ({})",
            env!("CARGO_PKG_VERSION"),
            app.active_account.email
        )
    };
    draw_header_bar(stdout, &header_title, colors.menu_bg, colors.accent)?;

    let items_count = if step == 0 {
        app.accounts.len()
    } else {
        folders.len()
    };
    let visible_items = rows.saturating_sub(5) as usize;
    let start_idx = scroll_window_start(selected_idx, visible_items);

    let is_selected_folder_custom = step != 0
        && folders
        .get(selected_idx)
        .is_some_and(|name| is_custom_folder(name));
    let hint_color = if colors.is_dark {
        Color::DarkGrey
    } else {
        Color::Grey
    };

    for i in 0..visible_items {
        let actual_idx = start_idx + i;
        if actual_idx >= items_count {
            break;
        }

        let text = if step == 0 {
            app.accounts[actual_idx].email.as_str()
        } else {
            folders[actual_idx].as_str()
        };
        let y = 1 + i as u16;
        let row_bg = if actual_idx == selected_idx {
            colors.selected_bg
        } else {
            colors.bg
        };
        let custom = step != 0 && is_custom_folder(text);
        let fg = if custom { colors.accent } else { colors.fg };

        queue!(
            stdout,
            cursor::MoveTo(0, y),
            SetBackgroundColor(row_bg),
            terminal::Clear(ClearType::CurrentLine),
            cursor::MoveTo(2, y),
            SetForegroundColor(fg),
            Print(text)
        )?;

        if custom {
            queue!(stdout, SetForegroundColor(hint_color), Print("   -> custom folder"))?;
        }
        queue!(stdout, ResetColor)?;
    }

    let rename_opt = if is_selected_folder_custom { ("R", " Rename") } else { ("", "") };
    let del_opt = if is_selected_folder_custom { ("D", " Del Fldr") } else { ("", "") };
    let add_fldr_opt = if step == 0 { ("", "") } else { ("A", " Add Fldr") };

    draw_menu_pair(
        stdout,
        rows,
        cols,
        &[("<", " Back"), (">", " Select"), ("P", " Prev"), add_fldr_opt, rename_opt, ("", "")],
        &[("M", " Main Menu"), ("", ""), ("N", " Next"), ("", ""), del_opt, ("H", " Help")],
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )
}

fn draw_main_menu(
    stdout: &mut io::Stdout,
    app: &App,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
    selected_idx: usize,
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    let current_version = env!("CARGO_PKG_VERSION");
    let (update_desc, is_update_avail) = match &app.latest_version {
        Some(latest) if latest != current_version => {
            (format!("Update xpine to version {}", latest), true)
        }
        Some(_) => (format!("xpine ({}) is up to date", current_version), false),
        None => ("Checking for updates...".to_string(), false),
    };

    let menu_options = [
        ("I", "INBOX", "Go to the default Inbox"),
        ("A", "ADDRESS BOOK", "Update address book"),
        ("F", "FOLDER LIST", "Select folder"),
        ("S", "SETTINGS", "Configure xpine"),
        ("E", "EMAIL ACCOUNTS", "Add/Delete email accounts"),
        ("H", "HELP", "Get help using xpine"),
        ("U", "UPDATE XPINE", update_desc.as_str()),
        ("Q", "QUIT", "Leave the xpine program"),
    ];

    draw_header_bar(
        stdout,
        &format!("xpine ({}) - Main Menu ({})", current_version, app.active_account.email),
        colors.menu_bg,
        colors.accent,
    )?;

    let x = (cols / 2).saturating_sub(25);
    let base_y = (rows / 2).saturating_sub(menu_options.len() as u16);

    for (i, (key, title, desc)) in menu_options.iter().enumerate() {
        let y = base_y + (i * 2) as u16;
        let row_bg = if i == selected_idx {
            colors.selected_bg
        } else {
            colors.bg
        };

        queue!(
            stdout,
            cursor::MoveTo(x, y),
            SetBackgroundColor(row_bg),
            SetForegroundColor(colors.accent),
            Print(format!(" {:>2} ", key)),
            SetForegroundColor(colors.fg),
            Print(format!("{:<15} - ", title))
        )?;

        if *key == "U" && !is_update_avail && app.latest_version.is_some() {
            queue!(
                stdout,
                SetForegroundColor(colors.fg),
                Print("xpine ("),
                SetForegroundColor(colors.accent),
                Print(current_version),
                SetForegroundColor(colors.fg),
                Print(") is up to date")
            )?;
        } else {
            let desc_color = if is_update_avail && *key == "U" {
                Color::Red
            } else {
                colors.fg
            };
            queue!(stdout, SetForegroundColor(desc_color), Print(desc))?;
        }

        queue!(stdout, ResetColor)?;
    }

    draw_menu_pair(
        stdout,
        rows,
        cols,
        &[(">", " Select"), ("P", " Prev"), ("", ""), ("", ""), ("", ""), ("", "")],
        &[("Q", " Quit"), ("N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")],
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;

    if app.accounts.is_empty() {
        queue!(
            stdout,
            cursor::MoveTo(0, rows.saturating_sub(3)),
            SetForegroundColor(Color::Red),
            Print("No email account. Please type 'E' and Add an email account."),
            ResetColor
        )?;
    }
    Ok(())
}

fn draw_settings(
    stdout: &mut io::Stdout,
    cols: u16,
    rows: u16,
    theme_provider: &Editor,
    selected_idx: usize,
) -> io::Result<()> {
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
    let colors = derive_ui_colors(theme);

    draw_header_bar(
        stdout,
        &format!("xpine ({}) - Settings", env!("CARGO_PKG_VERSION")),
        colors.menu_bg,
        colors.accent,
    )?;

    let options = [
        ("    Sort Newest First", theme_provider.sort_newest_first),
        ("    Spellcheck Before Sending", theme_provider.spellcheck_before_send),
        ("    Show Line Numbers", theme_provider.show_line_numbers),
        (
            "    Use day/month/year (instead of month/day/year)",
            theme_provider.use_dmy_dates,
        ),
    ];

    for (i, (label, is_enabled)) in options.iter().enumerate() {
        let row_bg = if i == selected_idx {
            colors.selected_bg
        } else {
            colors.bg
        };
        let checkbox = if *is_enabled { " [X]" } else { " [ ]" };
        queue!(
            stdout,
            cursor::MoveTo(1, 1 + i as u16),
            SetBackgroundColor(row_bg),
            SetForegroundColor(colors.fg),
            Print(format!("{} {:<20} ", checkbox, label)),
            ResetColor
        )?;
    }

    let sig_idx = options.len();
    let sig_bg = if selected_idx == sig_idx {
        colors.selected_bg
    } else {
        colors.bg
    };
    queue!(
        stdout,
        cursor::MoveTo(1, 1 + sig_idx as u16),
        SetBackgroundColor(sig_bg),
        SetForegroundColor(colors.fg),
        Print(" [>]     Edit Email Signature               "),
        ResetColor
    )?;

    let theme_y = 3 + options.len() as u16;
    queue!(
        stdout,
        cursor::MoveTo(2, theme_y),
        SetBackgroundColor(colors.bg),
        SetForegroundColor(colors.accent),
        Print("Meta+T"),
        cursor::MoveTo(10, theme_y),
        SetForegroundColor(colors.fg),
        Print("Theme: "),
        SetForegroundColor(colors.accent),
        Print(&theme_provider.current_theme),
        ResetColor
    )?;

    draw_menu_pair(
        stdout,
        rows,
        cols,
        &[("<", " Back"), ("P", " Prev"), ("     X", " Select"), ("", ""), ("", ""), ("", "")],
        &[("M", " Main Menu"), ("N", " Next"), ("Meta+T", " Theme"), ("", ""), ("", ""), ("", "")],
        colors.menu_bg,
        colors.accent,
        colors.fg,
    )?;
    queue!(stdout, cursor::Hide)?;
    Ok(())
}