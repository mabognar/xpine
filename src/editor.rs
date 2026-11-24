use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::Path;
use std::time::Instant;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use ropey::Rope;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::config::ConfigExt;
use crate::prompt::PromptExt;
use crate::search::SearchExt;
use crate::spell::SpellExt;
use crate::syntax::SyntaxExt;
use crate::theme::ThemeExt;
use crate::ui::UiExt;

const WRAP_MARGIN: usize = 72;

#[derive(PartialEq, Clone, Copy)]
pub enum MenuState {
    YesNoCancel,
    ReplaceAction,
    CancelOnly,
    PromptWithBrowser,
    SpellCheck,
    EmailComposer,
    EmailReader,
    TeamEditor,
    SignatureEditor,
}

pub enum EditorResult {
    Continue,
    Send,
    Cancel,
}

struct EditorSnapshot {
    buffer: Rope,
    menu_state: MenuState,
    top_margin: u16,
    cursor_x: usize,
    cursor_y: usize,
    desired_cursor_x: usize,
    col_offset: usize,
    row_offset: usize,
    status_message: String,
    status_time: Option<Instant>,
}

pub struct Editor {
    pub(crate) buffer: Rope,
    pub(crate) cursor_x: usize,
    pub(crate) cursor_y: usize,
    pub(crate) desired_cursor_x: usize,
    pub(crate) mark: Option<usize>,
    pub(crate) row_offset: usize,
    pub(crate) col_offset: usize,
    pub(crate) filename: Option<String>,
    pub(crate) should_quit: bool,
    pub(crate) status_message: String,
    pub(crate) clipboard: String,
    pub(crate) dictionary: Option<HashSet<String>>,
    pub(crate) ignored_words: HashSet<String>,
    pub(crate) current_suggestions: Vec<String>,
    pub(crate) syntax_set: SyntaxSet,
    pub(crate) theme_set: ThemeSet,
    pub(crate) is_modified: bool,
    pub(crate) last_search: Option<String>,
    pub menu_state: MenuState,
    pub top_margin: u16,
    pub(crate) status_time: Option<Instant>,
    pub(crate) highlight_match: Option<(usize, usize, usize)>,
    pub(crate) highlight_cache: HashMap<usize, Vec<(Style, String)>>,
    pub(crate) current_theme: String,
    pub(crate) is_justified: bool,
    pub(crate) pre_justify_snapshot: Option<(Rope, usize, usize)>,
    pub(crate) show_line_numbers: bool,
    pub(crate) soft_wrap: bool,
    pub(crate) sort_newest_first: bool,
    pub(crate) previous_action_was_cut: bool,
    pub menu_page: u8,
    pub(crate) spellcheck_before_send: bool,
    pub(crate) use_dmy_dates: bool,
}

impl Editor {
    pub fn new(filename: Option<String>) -> Self {
        let buffer = filename
            .as_deref()
            .and_then(|fname| File::open(Self::expand_tilde(fname)).ok())
            .and_then(|file| Rope::from_reader(io::BufReader::new(file)).ok())
            .unwrap_or_default();

        let (theme_set, themes_found, error_occurred) = Self::load_theme_set();
        let initial_status = if themes_found > 0 {
            String::new()
        } else {
            error_occurred.unwrap_or_default()
        };

        let (
            mut starting_theme,
            show_line_numbers,
            soft_wrap,
            sort_newest_first,
            spellcheck_before_send,
            use_dmy_dates,
        ) = Self::load_settings();
        if !theme_set.themes.contains_key(&starting_theme) {
            starting_theme = String::from("base16-ocean.dark");
        }

        Self {
            buffer,
            cursor_x: 0,
            cursor_y: 0,
            desired_cursor_x: 0,
            mark: None,
            row_offset: 0,
            col_offset: 0,
            filename,
            should_quit: false,
            status_message: initial_status,
            status_time: Some(Instant::now()),
            clipboard: String::new(),
            dictionary: None,
            ignored_words: HashSet::new(),
            current_suggestions: Vec::new(),
            syntax_set: Self::init_syntax(),
            theme_set,
            is_modified: false,
            last_search: None,
            menu_state: MenuState::EmailComposer,
            top_margin: 0,
            highlight_match: None,
            highlight_cache: HashMap::new(),
            current_theme: starting_theme,
            is_justified: false,
            pre_justify_snapshot: None,
            show_line_numbers,
            soft_wrap,
            sort_newest_first,
            spellcheck_before_send,
            use_dmy_dates,
            previous_action_was_cut: false,
            menu_page: 1,
        }
    }

    pub fn handle_keypress(&mut self, key: event::KeyEvent) -> io::Result<EditorResult> {
        if key.kind != KeyEventKind::Press {
            return Ok(EditorResult::Continue);
        }
        self.highlight_match = None;

        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_alt = key.modifiers.contains(KeyModifiers::ALT);

        if self.menu_state == MenuState::EmailComposer && is_ctrl {
            match key.code {
                KeyCode::Char('x') => return Ok(EditorResult::Send),
                KeyCode::Char('c') => return Ok(EditorResult::Cancel),
                _ => {}
            }
        }

        if self.menu_state == MenuState::EmailReader {
            match key.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('<') => return Ok(EditorResult::Cancel),
                KeyCode::Char('r' | 'R' | 'f' | 'F') => return Ok(EditorResult::Send),
                KeyCode::Char('v' | 'V' | ' ') | KeyCode::PageDown => self.page_down()?,
                KeyCode::Char('y' | 'Y' | '-') | KeyCode::PageUp => self.page_up()?,
                KeyCode::Up | KeyCode::Char('p' | 'P') => {
                    self.row_offset = self.row_offset.saturating_sub(1);
                    self.cursor_y = self.row_offset;
                }
                KeyCode::Down | KeyCode::Char('n' | 'N') => {
                    let visible_rows = self.visible_page_rows()?;
                    let max_offset = self.buffer.len_lines().saturating_sub(visible_rows);
                    self.row_offset = (self.row_offset + 1).min(max_offset);
                    self.cursor_y = self.row_offset;
                }
                KeyCode::Home => {
                    self.cursor_y = 0;
                    self.row_offset = 0;
                    self.cursor_x = 0;
                }
                KeyCode::End => {
                    self.cursor_y = self.buffer.len_lines().saturating_sub(1);
                    self.cursor_x = 0;
                }
                _ => {}
            }
            self.scroll()?;
            return Ok(EditorResult::Continue);
        }

        let was_justified = self.is_justified;
        let mut keep_justified = false;
        let mut current_action_is_cut = false;

        match (key.code, is_ctrl, is_alt) {
            (KeyCode::Char('^' | '6'), true, _) | (KeyCode::Char('a'), _, true) => {
                self.toggle_mark();
            }

            (KeyCode::F(1), _, _) => {
                self.show_help("composer")?;
                return Ok(EditorResult::Continue);
            }
            (KeyCode::Char('h' | 'H'), true, _) => {
                let _ = self.show_help("composer");
                return Ok(EditorResult::Continue);
            }

            (KeyCode::Char('x'), true, _) | (KeyCode::F(2), _, _) => self.exit_editor()?,

            (KeyCode::Char('o'), true, _) => {
                if self.menu_state == MenuState::EmailComposer {
                    self.menu_page = if self.menu_page == 1 { 2 } else { 1 };
                } else if !matches!(
                    self.menu_state,
                    MenuState::SignatureEditor | MenuState::TeamEditor
                ) {
                    self.save_file()?;
                }
            }

            (KeyCode::Char('j'), true, _) | (KeyCode::F(4), _, _) => {
                self.justify();
                keep_justified = true;
            }

            (KeyCode::Char('r'), true, _) | (KeyCode::F(5), _, _) => self.read_file()?,
            (KeyCode::Char('w'), true, _) | (KeyCode::F(6), _, _) => self.where_is()?,
            (KeyCode::Char('\\' | '4'), true, _) => self.replace()?,

            (KeyCode::Char('k'), true, _) | (KeyCode::F(9), _, _) => {
                self.cut_line();
                current_action_is_cut = true;
            }
            (KeyCode::Char('u'), true, _) | (KeyCode::F(10), _, _) => {
                if was_justified {
                    self.unjustify();
                } else {
                    self.paste_line();
                }
            }

            (KeyCode::Char('t' | 'T'), true, _) => {
                let _ = self.spell_check();
                return Ok(EditorResult::Continue);
            }
            (KeyCode::F(12), _, _) => {
                let _ = self.spell_check()?;
            }

            (KeyCode::Char('c'), true, _) | (KeyCode::F(11), _, _) => self.cur_pos(),
            (KeyCode::Char('l'), true, _) => self.go_to_line()?,

            (KeyCode::Char('t'), _, true) => {
                self.cycle_theme();
                self.save_settings();
            }
            (KeyCode::Char('l'), _, true) => {
                self.show_line_numbers = !self.show_line_numbers;
                self.save_settings();
                self.set_status(if self.show_line_numbers {
                    "Line numbers enabled".into()
                } else {
                    "Line numbers disabled".into()
                });
            }
            (KeyCode::Char('s'), _, true) => {
                self.soft_wrap = !self.soft_wrap;
                self.save_settings();
                self.set_status(if self.soft_wrap {
                    "Soft wrap enabled".into()
                } else {
                    "Soft wrap disabled".into()
                });
            }

            (KeyCode::Char('y'), true, _) | (KeyCode::F(7) | KeyCode::PageUp, _, _) => {
                self.page_up()?;
            }
            (KeyCode::Char('v'), true, _) | (KeyCode::F(8) | KeyCode::PageDown, _, _) => {
                self.page_down()?;
            }

            (KeyCode::Char('b'), true, _) | (KeyCode::Left, _, _) => self.move_left(),
            (KeyCode::Char('f'), true, _) | (KeyCode::Right, _, _) => self.move_right(),
            (KeyCode::Char('p'), true, _) | (KeyCode::Up, _, _) => self.move_up(),
            (KeyCode::Char('n'), true, _) | (KeyCode::Down, _, _) => self.move_down(),
            (KeyCode::Char('a'), true, _) => self.move_to_start_of_line(),
            (KeyCode::Char('e'), true, _) => self.move_to_end_of_line(),

            (KeyCode::Char('d'), true, _) | (KeyCode::Delete, _, _) => self.delete_char(),
            (KeyCode::Char('i'), true, _) | (KeyCode::Tab, _, _) => self.insert_tab(),

            (KeyCode::Char(c), false, false) => self.insert_char_with_wrap(c),
            (KeyCode::Enter, _, _) => {
                let idx = self.get_cursor_char_idx();
                self.buffer.insert_char(idx, '\n');
                self.cursor_y += 1;
                self.cursor_x = 0;
                self.desired_cursor_x = 0;
                self.mark_modified();
            }
            (KeyCode::Backspace, _, _) => {
                let idx = self.get_cursor_char_idx();
                if idx > 0 {
                    self.buffer.remove((idx - 1)..idx);
                    self.set_cursor_from_char_idx(idx - 1);
                    self.mark_modified();
                }
            }
            _ => self.clear_status(),
        }

        if !keep_justified {
            self.is_justified = false;
        }
        self.previous_action_was_cut = current_action_is_cut;
        self.scroll()?;
        Ok(EditorResult::Continue)
    }

    fn insert_char_with_wrap(&mut self, c: char) {
        let idx = self.get_cursor_char_idx();
        self.buffer.insert_char(idx, c);
        self.cursor_x += 1;
        self.desired_cursor_x = self.cursor_x;
        self.mark_modified();

        if self.cursor_x > WRAP_MARGIN && c != ' ' {
            let line_start_idx = self.buffer.line_to_char(self.cursor_y);
            let current_idx = self.get_cursor_char_idx();

            if let Some(idx_to_break) = (line_start_idx..current_idx)
                .rev()
                .find(|&i| self.buffer.char(i) == ' ')
            {
                self.buffer.remove(idx_to_break..(idx_to_break + 1));
                self.buffer.insert_char(idx_to_break, '\n');
                self.set_cursor_from_char_idx(current_idx);
            }
        }
    }

    fn set_cursor_from_char_idx(&mut self, idx: usize) {
        let clamped = idx.min(self.buffer.len_chars());
        self.cursor_y = self
            .buffer
            .char_to_line(clamped)
            .min(self.buffer.len_lines().saturating_sub(1));
        self.cursor_x = clamped - self.buffer.line_to_char(self.cursor_y);
        self.desired_cursor_x = self.cursor_x;
    }

    fn visible_page_rows(&self) -> io::Result<usize> {
        let (_, rows) = terminal::size()?;
        Ok(rows.saturating_sub(4 + self.top_margin) as usize)
    }

    fn visual_width(chars: impl Iterator<Item = char>) -> usize {
        chars.fold(0, |w, ch| match ch {
            '\n' | '\r' => w,
            '\t' => w + (4 - (w % 4)),
            _ => w + 1,
        })
    }

    fn line_has_text(&self, y: usize) -> bool {
        self.buffer.line(y).chars().any(|c| !c.is_whitespace())
    }

    pub(crate) fn scroll(&mut self) -> io::Result<()> {
        let (cols, rows) = terminal::size()?;

        let status_overhead = u16::from(!self.status_message.trim().is_empty());
        let visible_rows = rows.saturating_sub(2 + status_overhead + self.top_margin) as usize;

        let gutter_width = if self.show_line_numbers {
            self.buffer.len_lines().to_string().len() + 1
        } else {
            0
        };
        let available_width = (cols as usize).saturating_sub(gutter_width).max(1);

        if self.soft_wrap {
            self.col_offset = 0;
            if self.cursor_y < self.row_offset {
                self.row_offset = self.cursor_y;
            } else {
                let mut screen_rows_used = self.get_visual_cursor_x() / available_width;
                let mut required_row_offset = self.cursor_y;
                while required_row_offset > 0 {
                    let w = self.get_visual_line_width(required_row_offset - 1);
                    let line_rows = w.saturating_sub(1) / available_width + 1;
                    if screen_rows_used + line_rows >= visible_rows {
                        break;
                    }
                    screen_rows_used += line_rows;
                    required_row_offset -= 1;
                }
                self.row_offset = self.row_offset.max(required_row_offset);
            }
        } else {
            if self.cursor_y < self.row_offset {
                self.row_offset = self.cursor_y;
            } else if self.cursor_y >= self.row_offset + visible_rows {
                self.row_offset = self.cursor_y.saturating_sub(visible_rows.saturating_sub(1));
            }
            let visual_x = self.get_visual_cursor_x();
            if visual_x < self.col_offset || visual_x >= self.col_offset + available_width {
                self.col_offset = visual_x.saturating_sub(available_width / 2);
            }
        }
        Ok(())
    }

    pub(crate) fn get_visual_line_width(&self, y: usize) -> usize {
        if y >= self.buffer.len_lines() {
            return 0;
        }
        Self::visual_width(self.buffer.line(y).chars())
    }

    pub(crate) fn get_visual_cursor_x(&self) -> usize {
        if self.cursor_y >= self.buffer.len_lines() {
            return 0;
        }
        Self::visual_width(self.buffer.line(self.cursor_y).chars().take(self.cursor_x))
    }

    pub(crate) fn get_cursor_char_idx(&self) -> usize {
        self.buffer.line_to_char(self.cursor_y) + self.cursor_x
    }

    pub(crate) fn line_len(&self, y: usize) -> usize {
        if y >= self.buffer.len_lines() {
            return 0;
        }
        let line = self.buffer.line(y);
        let mut len = line.len_chars();
        if len > 0 && line.char(len - 1) == '\n' {
            len -= 1;
        }
        if len > 0 && line.char(len - 1) == '\r' {
            len -= 1;
        }
        len
    }

    pub(crate) fn move_up(&mut self) {
        if self.cursor_y > 0 {
            self.cursor_y -= 1;
            self.cursor_x = self.desired_cursor_x.min(self.line_len(self.cursor_y));
        }
    }

    pub(crate) fn move_down(&mut self) {
        if self.cursor_y < self.buffer.len_lines().saturating_sub(1) {
            self.cursor_y += 1;
            self.cursor_x = self.desired_cursor_x.min(self.line_len(self.cursor_y));
        }
    }

    pub(crate) fn move_left(&mut self) {
        let idx = self.get_cursor_char_idx();
        if idx > 0 {
            self.set_cursor_from_char_idx(idx - 1);
        }
    }

    pub(crate) fn move_right(&mut self) {
        let idx = self.get_cursor_char_idx();
        if idx < self.buffer.len_chars() {
            self.set_cursor_from_char_idx(idx + 1);
        }
    }

    pub(crate) fn move_to_start_of_line(&mut self) {
        self.cursor_x = 0;
        self.desired_cursor_x = 0;
    }

    pub(crate) fn move_to_end_of_line(&mut self) {
        self.cursor_x = self.line_len(self.cursor_y);
        self.desired_cursor_x = self.cursor_x;
    }

    pub(crate) fn delete_char(&mut self) {
        let idx = self.get_cursor_char_idx();
        if idx < self.buffer.len_chars() {
            self.buffer.remove(idx..(idx + 1));
            self.mark_modified();
        }
    }

    pub(crate) fn insert_tab(&mut self) {
        let idx = self.get_cursor_char_idx();
        self.buffer.insert(idx, "    ");
        self.cursor_x += 4;
        self.desired_cursor_x = self.cursor_x;
        self.mark_modified();
    }

    pub(crate) fn page_up(&mut self) -> io::Result<()> {
        let visible_rows = self.visible_page_rows()?;
        self.cursor_y = self.cursor_y.saturating_sub(visible_rows);
        self.row_offset = self.row_offset.saturating_sub(visible_rows);
        self.cursor_x = self.desired_cursor_x.min(self.line_len(self.cursor_y));
        Ok(())
    }

    pub(crate) fn page_down(&mut self) -> io::Result<()> {
        let visible_rows = self.visible_page_rows()?;
        let max_y = self.buffer.len_lines().saturating_sub(1);
        self.cursor_y = (self.cursor_y + visible_rows).min(max_y);

        let max_offset = self.buffer.len_lines().saturating_sub(visible_rows);
        self.row_offset = (self.row_offset + visible_rows).min(max_offset);

        self.cursor_x = self.desired_cursor_x.min(self.line_len(self.cursor_y));
        Ok(())
    }

    pub(crate) fn exit_editor(&mut self) -> io::Result<()> {
        if self.is_modified {
            match self.prompt_yn("Save modified buffer (ANSWERING \"No\" WILL DESTROY CHANGES) ?")? {
                Some(true) => {
                    self.save_file()?;
                    if !self.is_modified {
                        self.should_quit = true;
                    }
                }
                Some(false) => self.should_quit = true,
                None => {}
            }
        } else {
            self.should_quit = true;
        }
        Ok(())
    }

    pub(crate) fn toggle_mark(&mut self) {
        if self.mark.is_some() {
            self.mark = None;
            self.set_status(String::from("Unmark set"));
        } else {
            self.mark = Some(self.get_cursor_char_idx());
            self.set_status(String::from("Mark Set"));
        }
    }

    pub(crate) fn cur_pos(&mut self) {
        let line = self.cursor_y + 1;
        let total_lines = self.buffer.len_lines();
        let col = self.cursor_x + 1;
        let total_chars = self.buffer.len_chars();
        self.set_status(format!(
            "line {}/{}, col {}, char {}",
            line, total_lines, col, total_chars
        ));
    }

    pub(crate) fn go_to_line(&mut self) -> io::Result<()> {
        if let Some(input) = self.prompt("Enter line number: ", false)? {
            if let Ok(line) = input.trim().parse::<usize>() {
                self.cursor_y = line
                    .saturating_sub(1)
                    .min(self.buffer.len_lines().saturating_sub(1));
                self.cursor_x = 0;
                self.desired_cursor_x = 0;
                self.clear_status();
            } else {
                self.set_status(String::from("Invalid line number"));
            }
        }
        Ok(())
    }

    pub(crate) fn justify(&mut self) {
        let max_y = self.buffer.len_lines().saturating_sub(1);
        if max_y == 0 && self.buffer.len_chars() == 0 {
            return;
        }

        let mut start_line = self.cursor_y;
        while start_line > 0 && self.line_has_text(start_line - 1) {
            start_line -= 1;
        }

        let mut end_line = self.cursor_y;
        while end_line < max_y && self.line_has_text(end_line) {
            end_line += 1;
        }

        // If end_line advanced onto an empty line, step back onto the paragraph's last text line
        if end_line > start_line && !self.line_has_text(end_line) {
            end_line -= 1;
        }

        if start_line == end_line && !self.line_has_text(start_line) {
            return;
        }

        let start_char = self.buffer.line_to_char(start_line);
        let end_char = if end_line + 1 < self.buffer.len_lines() {
            self.buffer.line_to_char(end_line + 1)
        } else {
            self.buffer.len_chars()
        };
        let text = self.buffer.slice(start_char..end_char).to_string();
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return;
        }

        self.pre_justify_snapshot = Some((self.buffer.clone(), self.cursor_x, self.cursor_y));

        let mut new_text = String::new();
        let mut current_line_len = 0;

        for word in words {
            let word_len = word.chars().count();
            let space_needed = usize::from(current_line_len > 0);

            if current_line_len + word_len + space_needed > WRAP_MARGIN {
                if current_line_len > 0 {
                    new_text.push('\n');
                }
                new_text.push_str(word);
                current_line_len = word_len;
            } else {
                if current_line_len > 0 {
                    new_text.push(' ');
                }
                new_text.push_str(word);
                current_line_len += word_len + space_needed;
            }
        }
        new_text.push('\n');

        self.buffer.remove(start_char..end_char);
        self.buffer.insert(start_char, &new_text);

        self.set_cursor_from_char_idx(start_char + new_text.chars().count());
        self.is_justified = true;
        self.mark_modified();
        self.set_status(String::from("Justified --- Ctrl+U to undo"));
    }

    pub(crate) fn unjustify(&mut self) {
        if let Some((snapshot, x, y)) = self.pre_justify_snapshot.take() {
            self.buffer = snapshot;
            self.cursor_x = x;
            self.cursor_y = y;
            self.desired_cursor_x = x;
            self.is_justified = false;
            self.clear_cache();
            self.set_status(String::from("Unjustified"));
            self.mark_modified();
        }
    }

    pub(crate) fn cut_line(&mut self) {
        if self.buffer.len_chars() == 0 {
            return;
        }

        let (start_char, end_char, status, was_selection) = if let Some(mark_idx) = self.mark {
            let cursor_idx = self.get_cursor_char_idx();
            let (start, end) = (mark_idx.min(cursor_idx), mark_idx.max(cursor_idx));
            if start == end {
                return;
            }
            (start, end, "Cut selection", true)
        } else {
            let start = self.buffer.line_to_char(self.cursor_y);
            let end = if self.cursor_y + 1 < self.buffer.len_lines() {
                self.buffer.line_to_char(self.cursor_y + 1)
            } else {
                self.buffer.len_chars()
            };
            (start, end, "Cut line", false)
        };

        let cut_text = self.buffer.slice(start_char..end_char).to_string();
        if self.previous_action_was_cut {
            self.clipboard.push_str(&cut_text);
        } else {
            self.clipboard = cut_text;
        }
        self.buffer.remove(start_char..end_char);

        if was_selection {
            self.set_cursor_from_char_idx(start_char);
            self.mark = None;
        } else {
            self.cursor_x = 0;
            self.desired_cursor_x = 0;
            self.cursor_y = self.cursor_y.min(self.buffer.len_lines().saturating_sub(1));
        }

        self.set_status(status.into());
        self.mark_modified();
    }

    pub(crate) fn paste_line(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        let current_char = self.get_cursor_char_idx();
        self.buffer.insert(current_char, &self.clipboard);
        self.set_cursor_from_char_idx(current_char + self.clipboard.chars().count());
        self.set_status(String::from("Pasted text"));
        self.mark_modified();
    }

    pub(crate) fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") || path.starts_with("~\\") || path == "~" {
            let home = env::var("HOME")
                .or_else(|_| env::var("USERPROFILE"))
                .unwrap_or_default();
            if !home.is_empty() {
                return path.replacen('~', &home, 1);
            }
        }
        path.to_string()
    }

    pub(crate) fn read_file(&mut self) -> io::Result<()> {
        if let Some(filepath) = self.prompt("File to insert: ", true)? {
            if filepath.is_empty() {
                self.set_status(String::from("Read cancelled."));
                return Ok(());
            }
            match fs::read_to_string(&Self::expand_tilde(&filepath)) {
                Ok(contents) => {
                    let idx = self.get_cursor_char_idx();
                    self.buffer.insert(idx, &contents);
                    self.set_status(format!("Read {} lines", contents.lines().count()));
                    self.mark_modified();
                }
                Err(e) => self.set_status(format!("Error reading file: {}", e)),
            }
        }
        Ok(())
    }

    pub(crate) fn save_file(&mut self) -> io::Result<()> {
        let default_name = self.filename.clone().unwrap_or_default();
        let prompt_text = if default_name.is_empty() {
            String::from("File Name to Write: ")
        } else {
            format!("File Name to Write [{}]: ", default_name)
        };
        if let Some(mut new_name) = self.prompt(&prompt_text, true)? {
            if new_name.is_empty() {
                if !default_name.is_empty() {
                    new_name = default_name;
                } else {
                    self.set_status(String::from("Save cancelled: No filename provided."));
                    return Ok(());
                }
            }
            let expanded_path = Self::expand_tilde(&new_name);
            let path = Path::new(&expanded_path);
            if path.exists() && Some(&new_name) != self.filename.as_ref() {
                if let Some(false) | None =
                    self.prompt_yn(&format!("File \"{}\" exists, OVERWRITE ?", new_name))?
                {
                    self.set_status(String::from("Save cancelled"));
                    return Ok(());
                }
            }
            match File::create(&expanded_path) {
                Ok(file) => {
                    if let Err(e) = self.buffer.write_to(BufWriter::new(file)) {
                        self.set_status(format!("Error writing file: {}", e));
                    } else {
                        self.filename = Some(new_name);
                        self.highlight_cache.clear();
                        self.set_status(format!("Wrote {} lines", self.buffer.len_lines()));
                        self.is_modified = false;
                    }
                }
                Err(e) => self.set_status(format!("Error creating file: {}", e)),
            }
        }
        Ok(())
    }

    pub fn edit_buffer(
        &mut self,
        title: &str,
        initial_content: &str,
        editor_type: MenuState,
    ) -> io::Result<Option<String>> {
        let saved = EditorSnapshot {
            buffer: std::mem::replace(&mut self.buffer, Rope::from_str(initial_content)),
            menu_state: std::mem::replace(&mut self.menu_state, editor_type),
            top_margin: std::mem::replace(&mut self.top_margin, 1),
            cursor_x: self.cursor_x,
            cursor_y: self.cursor_y,
            desired_cursor_x: self.desired_cursor_x,
            col_offset: std::mem::replace(&mut self.col_offset, 0),
            row_offset: std::mem::replace(&mut self.row_offset, 0),
            status_message: std::mem::take(&mut self.status_message),
            status_time: self.status_time.take(),
        };

        self.cursor_y = self.buffer.len_lines().saturating_sub(1);
        self.move_to_end_of_line();
        self.highlight_cache.clear();
        let _ = self.scroll();

        let mut stdout = io::stdout();
        let mut result = None;
        let mut needs_redraw = true;

        loop {
            if needs_redraw {
                let theme = &self.theme_set.themes[&self.current_theme];
                let colors = crate::theme::derive_ui_colors(theme);

                queue!(
                    stdout,
                    cursor::Hide,
                    cursor::MoveTo(0, 0),
                    SetBackgroundColor(colors.menu_bg),
                    Clear(ClearType::UntilNewLine),
                    SetForegroundColor(colors.accent),
                    Print(format!(" {} ", title)),
                    ResetColor
                )?;

                self.draw_screen()?;
                needs_redraw = false;
            }

            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                        match key_event.code {
                            KeyCode::Char('x') => {
                                result = Some(self.buffer.to_string());
                                break;
                            }
                            KeyCode::Char('c') => break,
                            KeyCode::Char('a') if self.menu_state == MenuState::TeamEditor => {
                                let addrs: Vec<String> = crate::address::load_address_book()
                                    .into_iter()
                                    .filter(|a| !a.trim().is_empty())
                                    .collect();

                                if let Ok(Some(selected)) =
                                    self.prompt_select_item("Select Address:", &addrs)
                                {
                                    let end_idx = self.buffer.len_chars();
                                    let prefix = if end_idx > 0
                                        && self.buffer.char(end_idx - 1) != '\n'
                                    {
                                        "\n"
                                    } else {
                                        ""
                                    };
                                    self.buffer
                                        .insert(end_idx, &format!("{}{}\n", prefix, selected));
                                    self.cursor_y = self.buffer.len_lines().saturating_sub(1);
                                    self.move_to_end_of_line();
                                    self.mark_modified();
                                    let _ = self.scroll();
                                }
                                needs_redraw = true;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    let _ = self.handle_keypress(key_event)?;
                    needs_redraw = true;
                }
                Event::Resize(_, _) => {
                    let _ = self.scroll();
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        self.buffer = saved.buffer;
        self.menu_state = saved.menu_state;
        self.top_margin = saved.top_margin;
        self.cursor_x = saved.cursor_x;
        self.cursor_y = saved.cursor_y;
        self.desired_cursor_x = saved.desired_cursor_x;
        self.col_offset = saved.col_offset;
        self.row_offset = saved.row_offset;
        self.status_message = saved.status_message;
        self.status_time = saved.status_time;
        self.highlight_cache.clear();

        Ok(result)
    }
}