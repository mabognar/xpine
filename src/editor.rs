use std::collections::{HashSet, HashMap};
use std::time::{Instant};
use std::path::Path;
use std::fs::{self, File};
use std::env;
use std::io::{self, BufWriter};
use ropey::Rope;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use crossterm::{terminal, event::{self, KeyCode, KeyModifiers}};

use crate::prompt::PromptExt;
use crate::config::ConfigExt;
use crate::spell::SpellExt;
use crate::ui::UiExt;
use crate::theme::ThemeExt;
use crate::syntax::SyntaxExt;
use crate::search::SearchExt;

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
}

pub enum EditorResult {
    Continue,
    Send,
    Cancel,
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
}

impl Editor {
    pub fn new(filename: Option<String>) -> Self {
        let buffer = if let Some(ref fname) = filename {
            let expanded = Self::expand_tilde(fname);
            if let Ok(file) = File::open(&expanded) {
                Rope::from_reader(io::BufReader::new(file)).unwrap_or_default()
            } else {
                Rope::new()
            }
        } else {
            Rope::new()
        };

        let (theme_set, themes_found, error_occurred) = Self::load_theme_set();
        let initial_status = if themes_found > 0 { String::new() } else if let Some(err) = error_occurred { err } else { String::new() };

        let (mut starting_theme, line_numbers, soft_wrap, sort_newest_first, spellcheck_before_send) = Self::load_settings();
        if !theme_set.themes.contains_key(&starting_theme) {
            starting_theme = String::from("base16-ocean.dark");
        }

        Self {
            buffer,
            cursor_x: 0, cursor_y: 0, desired_cursor_x: 0,
            mark: None, row_offset: 0, col_offset: 0,
            filename, should_quit: false,
            status_message: initial_status,
            status_time: Some(Instant::now()),
            clipboard: String::new(),
            dictionary: None, ignored_words: HashSet::new(), current_suggestions: Vec::new(),
            syntax_set: Self::init_syntax(),
            theme_set,
            is_modified: false, last_search: None,
            menu_state: MenuState::EmailComposer,
            top_margin: 0,
            highlight_match: None, highlight_cache: HashMap::new(),
            current_theme: starting_theme,
            is_justified: false, pre_justify_snapshot: None,
            show_line_numbers: line_numbers, soft_wrap, sort_newest_first, spellcheck_before_send,
            previous_action_was_cut: false,
            menu_page: 1,
        }
    }

    pub fn handle_keypress(&mut self, key: event::KeyEvent) -> io::Result<EditorResult> {
        if key.kind != event::KeyEventKind::Press { return Ok(EditorResult::Continue); }
        self.highlight_match = None;

        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_alt = key.modifiers.contains(KeyModifiers::ALT);

        if self.menu_state == MenuState::EmailComposer {
            if is_ctrl && key.code == KeyCode::Char('x') { return Ok(EditorResult::Send); }
            if is_ctrl && key.code == KeyCode::Char('c') { return Ok(EditorResult::Cancel); }
        }

        if self.menu_state == MenuState::EmailReader {
            match key.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('<') => return Ok(EditorResult::Cancel),
                KeyCode::Char('r') | KeyCode::Char('R') => return Ok(EditorResult::Send),
                KeyCode::Char('f') | KeyCode::Char('F') => return Ok(EditorResult::Send),

                KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::PageDown | KeyCode::Char(' ') => self.page_down()?,
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::PageUp | KeyCode::Char('-') => self.page_up()?,

                KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.row_offset = self.row_offset.saturating_sub(1);
                    self.cursor_y = self.row_offset;
                }

                KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                    let (_, rows) = terminal::size()?;
                    let visible_rows = rows.saturating_sub(4 + self.top_margin) as usize;
                    let max_offset = self.buffer.len_lines().saturating_sub(visible_rows);

                    self.row_offset = (self.row_offset + 1).min(max_offset);
                    self.cursor_y = self.row_offset;
                }

                KeyCode::Home => { self.cursor_y = 0; self.row_offset = 0; self.cursor_x = 0; }
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

        match key.code {
            KeyCode::Char('^') if is_ctrl => self.toggle_mark(),
            KeyCode::Char('6') if is_ctrl => self.toggle_mark(),
            KeyCode::Char('a') if is_alt => self.toggle_mark(),

            // KeyCode::Char('?') => { self.show_help("composer")?; return Ok(EditorResult::Continue); }
            KeyCode::F(1) => { self.show_help("composer")?; return Ok(EditorResult::Continue); }

            KeyCode::Char('x') if is_ctrl => self.exit_editor()?,
            KeyCode::F(2) => self.exit_editor()?,

            KeyCode::Char('o') if is_ctrl => {
                if self.menu_state == MenuState::EmailComposer {
                    self.menu_page = if self.menu_page == 1 { 2 } else { 1 };
                } else {
                    self.save_file()?;
                }
            }

            KeyCode::Char('j') if is_ctrl => {
                self.justify();
                self.is_justified = true;
                keep_justified = true;
            }

            KeyCode::Char('h') | KeyCode::Char('H') if is_ctrl => {
                let _ = self.show_help("composer");
                return Ok(EditorResult::Continue);
            }

            KeyCode::Char('r') if is_ctrl => self.read_file()?,
            KeyCode::F(5) => self.read_file()?,

            KeyCode::Char('w') if is_ctrl => self.where_is()?,
            KeyCode::F(6) => self.where_is()?,

            KeyCode::Char('\\') if is_ctrl => self.replace()?,
            KeyCode::Char('4') if is_ctrl => self.replace()?,

            KeyCode::Char('k') if is_ctrl => { self.cut_line(); current_action_is_cut = true; }
            KeyCode::F(9) => { self.cut_line(); current_action_is_cut = true; }

            KeyCode::Char('u') if is_ctrl => { if was_justified { self.unjustify(); } else { self.paste_line(); } }
            KeyCode::F(10) => { if was_justified { self.unjustify(); } else { self.paste_line(); } }

            KeyCode::Char('j') if is_ctrl => { self.justify(); self.is_justified = true; keep_justified = true; }
            KeyCode::F(4) => { self.justify(); self.is_justified = true; keep_justified = true; }

            KeyCode::Char('t') if is_ctrl => {
                let _ = self.spell_check();
                return Ok(EditorResult::Continue);
            }
            KeyCode::Char('T') if is_ctrl => {
                let _ = self.spell_check();
                return Ok(EditorResult::Continue);
            }
            KeyCode::F(12) => { let _ = self.spell_check()?; }

            KeyCode::Char('c') if is_ctrl => self.cur_pos(),
            KeyCode::F(11) => self.cur_pos(),

            KeyCode::Char('l') if is_ctrl => self.go_to_line()?,

            KeyCode::Char('t') if is_alt => {
                self.cycle_theme();
                self.save_settings();
            },

            KeyCode::Char('l') if is_alt => {
                self.show_line_numbers = !self.show_line_numbers;
                self.save_settings();
                self.set_status(if self.show_line_numbers { "Line numbers enabled".into() } else { "Line numbers disabled".into() });
            }
            KeyCode::Char('s') if is_alt => {
                self.soft_wrap = !self.soft_wrap;
                self.save_settings();
                self.set_status(if self.soft_wrap { "Soft wrap enabled".into() } else { "Soft wrap disabled".into() });
            }

            KeyCode::Char('y') if is_ctrl => self.page_up()?,
            KeyCode::F(7) => self.page_up()?,
            KeyCode::PageUp => self.page_up()?,

            KeyCode::Char('v') if is_ctrl => self.page_down()?,
            KeyCode::F(8) => self.page_down()?,
            KeyCode::PageDown => self.page_down()?,

            KeyCode::Char('b') if is_ctrl => self.move_left(),
            KeyCode::Char('f') if is_ctrl => self.move_right(),
            KeyCode::Char('p') if is_ctrl => self.move_up(),
            KeyCode::Char('n') if is_ctrl => self.move_down(),
            KeyCode::Char('a') if is_ctrl => self.move_to_start_of_line(),
            KeyCode::Char('e') if is_ctrl => self.move_to_end_of_line(),

            KeyCode::Char('d') if is_ctrl => self.delete_char(),
            KeyCode::Delete => self.delete_char(),

            KeyCode::Char('i') if is_ctrl => self.insert_tab(),
            KeyCode::Tab => self.insert_tab(),

            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),

            KeyCode::Char(c) if !is_ctrl && !is_alt => {
                let idx = self.get_cursor_char_idx();
                self.buffer.insert_char(idx, c);
                self.cursor_x += 1;
                self.desired_cursor_x = self.cursor_x;
                self.mark_modified();

                // automatic wrapping at 72
                let wrap_margin = 72;
                if self.cursor_x > wrap_margin && c != ' ' {
                    // look backward for the closest space character in the current line
                    // to break on a word boundary
                    let line_start_idx = self.buffer.line_to_char(self.cursor_y);
                    let current_idx = self.get_cursor_char_idx();

                    let mut space_idx = None;
                    // search backwards from the current position to the start of the line
                    for i in (line_start_idx..current_idx).rev() {
                        if self.buffer.char(i) == ' ' {
                            space_idx = Some(i);
                            break;
                        }
                    }

                    if let Some(idx_to_break) = space_idx {
                        // replace space character with newline character
                        self.buffer.remove(idx_to_break..(idx_to_break + 1));
                        self.buffer.insert_char(idx_to_break, '\n');

                        // recalculate cursor positions after splitting line
                        let new_cursor_idx = current_idx;
                        self.cursor_y = self.buffer.char_to_line(new_cursor_idx);
                        self.cursor_x = new_cursor_idx - self.buffer.line_to_char(self.cursor_y);
                        self.desired_cursor_x = self.cursor_x;
                    }
                }
            }
            KeyCode::Enter => {
                let idx = self.get_cursor_char_idx();
                self.buffer.insert_char(idx, '\n');
                self.cursor_y += 1;
                self.cursor_x = 0;
                self.desired_cursor_x = 0;
                self.mark_modified();
            }
            KeyCode::Backspace => {
                let idx = self.get_cursor_char_idx();
                if idx > 0 {
                    self.buffer.remove((idx - 1)..idx);
                    self.cursor_y = self.buffer.char_to_line(idx - 1);
                    self.cursor_x = (idx - 1) - self.buffer.line_to_char(self.cursor_y);
                    self.desired_cursor_x = self.cursor_x;
                    self.mark_modified();
                }
            }
            _ => { self.clear_status(); }
        }

        if !keep_justified { self.is_justified = false; }
        self.previous_action_was_cut = current_action_is_cut;
        self.scroll()?;
        Ok(EditorResult::Continue)
    }

    pub(crate) fn scroll(&mut self) -> io::Result<()> {
        let (cols, _) = terminal::size()?;
        let (_, rows) = terminal::size().unwrap_or((80, 24));

        let has_status = !self.status_message.trim().is_empty();
        let status_overhead = if has_status { 1 } else { 0 };
        let runtime_overhead = 2 + status_overhead;

        let visible_rows = rows.saturating_sub(runtime_overhead + self.top_margin) as usize;

        let cols_u = cols as usize;
        let max_line_num_len = self.buffer.len_lines().to_string().len();
        let gutter_width = if self.show_line_numbers { max_line_num_len + 1 } else { 0 };
        let available_width = std::cmp::max(1, cols_u.saturating_sub(gutter_width));

        if self.soft_wrap {
            self.col_offset = 0;
            if self.cursor_y < self.row_offset {
                self.row_offset = self.cursor_y;
            } else {
                let mut screen_rows_used = self.get_visual_cursor_x() / available_width;
                let mut required_row_offset = self.cursor_y;
                while required_row_offset > 0 {
                    let prev_line = required_row_offset - 1;
                    let w = self.get_visual_line_width(prev_line);
                    let line_rows = if w == 0 { 1 } else { (w - 1) / available_width + 1 };
                    if screen_rows_used + line_rows >= visible_rows { break; }
                    screen_rows_used += line_rows;
                    required_row_offset -= 1;
                }
                if self.row_offset < required_row_offset { self.row_offset = required_row_offset; }
            }
        } else {
            if self.cursor_y < self.row_offset {
                self.row_offset = self.cursor_y;
            } else if self.cursor_y >= self.row_offset + visible_rows {
                self.row_offset = self.cursor_y.saturating_sub(visible_rows.saturating_sub(1));
            }
            let visual_x = self.get_visual_cursor_x();
            let right_bound = self.col_offset + available_width;
            if visual_x < self.col_offset {
                self.col_offset = visual_x.saturating_sub(available_width / 2);
            } else if visual_x >= right_bound {
                self.col_offset = visual_x.saturating_sub(available_width / 2);
            }
        }
        Ok(())
    }

    pub(crate) fn get_visual_line_width(&self, y: usize) -> usize {
        if y >= self.buffer.len_lines() { return 0; }
        let mut w = 0;
        for ch in self.buffer.line(y).chars() {
            if ch == '\n' || ch == '\r' { continue; }
            if ch == '\t' { w += 4 - (w % 4); } else { w += 1; }
        }
        w
    }

    pub(crate) fn get_visual_cursor_x(&self) -> usize {
        if self.cursor_y >= self.buffer.len_lines() { return 0; }
        let line = self.buffer.line(self.cursor_y);
        let mut visual_x = 0;
        for ch in line.chars().take(self.cursor_x) {
            if ch == '\t' { visual_x += 4 - (visual_x % 4); } else { visual_x += 1; }
        }
        visual_x
    }

    pub(crate) fn get_cursor_char_idx(&self) -> usize { self.buffer.line_to_char(self.cursor_y) + self.cursor_x }

    pub(crate) fn line_len(&self, y: usize) -> usize {
        if y >= self.buffer.len_lines() { return 0; }
        let line = self.buffer.line(y);
        let mut len = line.len_chars();
        if len > 0 && line.char(len - 1) == '\n' { len -= 1; }
        if len > 0 && line.char(len - 1) == '\r' { len -= 1; }
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
            let new_idx = idx - 1;
            self.cursor_y = self.buffer.char_to_line(new_idx);
            self.cursor_x = new_idx - self.buffer.line_to_char(self.cursor_y);
            self.desired_cursor_x = self.cursor_x;
        }
    }

    pub(crate) fn move_right(&mut self) {
        let idx = self.get_cursor_char_idx();
        if idx < self.buffer.len_chars() {
            let new_idx = idx + 1;
            self.cursor_y = self.buffer.char_to_line(new_idx);
            self.cursor_x = new_idx - self.buffer.line_to_char(self.cursor_y);
            self.desired_cursor_x = self.cursor_x;
        }
    }

    pub(crate) fn move_to_start_of_line(&mut self) { self.cursor_x = 0; self.desired_cursor_x = 0; }
    pub(crate) fn move_to_end_of_line(&mut self) { self.cursor_x = self.line_len(self.cursor_y); self.desired_cursor_x = self.cursor_x; }

    pub(crate) fn delete_char(&mut self) {
        let idx = self.get_cursor_char_idx();
        if idx < self.buffer.len_chars() { self.buffer.remove(idx..(idx + 1)); self.mark_modified(); }
    }

    pub(crate) fn insert_tab(&mut self) {
        let idx = self.get_cursor_char_idx();
        self.buffer.insert(idx, "    ");
        self.cursor_x += 4; self.desired_cursor_x = self.cursor_x; self.mark_modified();
    }
    pub(crate) fn page_up(&mut self) -> io::Result<()> {
        let (_, rows) = terminal::size()?;
        let visible_rows = rows.saturating_sub(4 + self.top_margin) as usize;

        // move the cursor up one page
        self.cursor_y = self.cursor_y.saturating_sub(visible_rows);

        // move the viewport up one page
        self.row_offset = self.row_offset.saturating_sub(visible_rows);

        self.cursor_x = self.desired_cursor_x.min(self.line_len(self.cursor_y));
        Ok(())
    }

    pub(crate) fn page_down(&mut self) -> io::Result<()> {
        let (_, rows) = terminal::size()?;
        let visible_rows = rows.saturating_sub(4 + self.top_margin) as usize;

        // move cursor down one page
        let max_y = self.buffer.len_lines().saturating_sub(1);
        self.cursor_y = (self.cursor_y + visible_rows).min(max_y);

        // move viewport down one page
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
                    if !self.is_modified { self.should_quit = true; }
                }
                Some(false) => { self.should_quit = true; }
                None => {}
            }
        } else { self.should_quit = true; }
        Ok(())
    }

    pub(crate) fn toggle_mark(&mut self) {
        if self.mark.is_some() {
            self.mark = None; self.set_status(String::from("Unmark set"));
        } else {
            self.mark = Some(self.get_cursor_char_idx()); self.set_status(String::from("Mark Set"));
        }
    }

    pub(crate) fn cur_pos(&mut self) {
        let line = self.cursor_y + 1; let total_lines = self.buffer.len_lines();
        let col = self.cursor_x + 1; let total_chars = self.buffer.len_chars();
        self.set_status(format!("line {}/{}, col {}, char {}", line, total_lines, col, total_chars));
    }

    pub(crate) fn go_to_line(&mut self) -> io::Result<()> {
        if let Some(input) = self.prompt("Enter line number: ", false)? {
            if let Ok(line) = input.trim().parse::<usize>() {
                self.cursor_y = line.saturating_sub(1).min(self.buffer.len_lines().saturating_sub(1));
                self.cursor_x = 0; self.desired_cursor_x = 0; self.clear_status();
            } else { self.set_status(String::from("Invalid line number")); }
        }
        Ok(())
    }

    pub(crate) fn justify(&mut self) {
        self.pre_justify_snapshot = Some((self.buffer.clone(), self.cursor_x, self.cursor_y));
        let max_y = self.buffer.len_lines().saturating_sub(1);
        if max_y == 0 && self.buffer.len_chars() == 0 { return; }

        let mut start_line = self.cursor_y;
        while start_line > 0 && self.buffer.line(start_line - 1).chars().any(|c| !c.is_whitespace()) { start_line -= 1; }

        let mut end_line = self.cursor_y;
        while end_line < max_y && self.buffer.line(end_line).chars().any(|c| !c.is_whitespace()) { end_line += 1; }

        // if end_line advanced onto an empty line, step back so it stops exactly on the paragraphs last text line
        if end_line > start_line && !self.buffer.line(end_line).chars().any(|c| !c.is_whitespace()) {
            end_line -= 1;
        }

        if start_line == end_line && !self.buffer.line(start_line).chars().any(|c| !c.is_whitespace()) { return; }

        let start_char = self.buffer.line_to_char(start_line);
        let end_char = if end_line + 1 < self.buffer.len_lines() { self.buffer.line_to_char(end_line + 1) } else { self.buffer.len_chars() };
        let text = self.buffer.slice(start_char..end_char).to_string();
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() { return; }

        let mut new_text = String::new();
        let mut current_line_len = 0;

        for word in words {
            let word_len = word.chars().count();
            let space_needed = if current_line_len > 0 { 1 } else { 0 };

            if current_line_len + word_len + space_needed > 72 {
                if current_line_len > 0 {
                    new_text.push('\n');
                    new_text.push_str(word);
                    current_line_len = word_len;
                } else {
                    new_text.push_str(word);
                    current_line_len = word_len;
                }
            } else {
                if current_line_len > 0 { new_text.push(' '); }
                new_text.push_str(word);
                current_line_len += word_len + space_needed;
            }
        }
        new_text.push('\n');

        self.buffer.remove(start_char..end_char);
        self.buffer.insert(start_char, &new_text);

        let safe_pos = (start_char + new_text.chars().count()).min(self.buffer.len_chars());
        self.cursor_y = self.buffer.char_to_line(safe_pos).min(self.buffer.len_lines().saturating_sub(1));
        self.cursor_x = safe_pos - self.buffer.line_to_char(self.cursor_y);
        self.desired_cursor_x = self.cursor_x;

        self.is_justified = true; self.mark_modified(); self.set_status(String::from("Justified --- Ctrl+U to undo"));
    }

    pub(crate) fn unjustify(&mut self) {
        if let Some((snapshot, x, y)) = self.pre_justify_snapshot.take() {
            self.buffer = snapshot; self.cursor_x = x; self.cursor_y = y; self.desired_cursor_x = x;
            self.is_justified = false; self.clear_cache(); self.set_status(String::from("Unjustified")); self.mark_modified();
        }
    }

    pub(crate) fn cut_line(&mut self) {
        if self.buffer.len_chars() == 0 { return; }
        if let Some(mark_idx) = self.mark {
            let cursor_idx = self.get_cursor_char_idx();
            let start_char = mark_idx.min(cursor_idx);
            let end_char = mark_idx.max(cursor_idx);
            if start_char != end_char {
                let cut_text = self.buffer.slice(start_char..end_char).to_string();
                if self.previous_action_was_cut { self.clipboard.push_str(&cut_text); } else { self.clipboard = cut_text; }
                self.buffer.remove(start_char..end_char);
                self.cursor_y = self.buffer.char_to_line(start_char);
                self.cursor_x = start_char - self.buffer.line_to_char(self.cursor_y);
                self.desired_cursor_x = self.cursor_x;
                self.mark = None; self.set_status(String::from("Cut selection")); self.mark_modified();
            }
        } else {
            let start_char = self.buffer.line_to_char(self.cursor_y);
            let end_char = if self.cursor_y + 1 < self.buffer.len_lines() { self.buffer.line_to_char(self.cursor_y + 1) } else { self.buffer.len_chars() };
            let cut_text = self.buffer.slice(start_char..end_char).to_string();
            if self.previous_action_was_cut { self.clipboard.push_str(&cut_text); } else { self.clipboard = cut_text; }
            self.buffer.remove(start_char..end_char);
            self.cursor_x = 0; self.desired_cursor_x = 0;
            if self.cursor_y > self.buffer.len_lines().saturating_sub(1) { self.cursor_y = self.buffer.len_lines().saturating_sub(1); }
            self.set_status(String::from("Cut line")); self.mark_modified();
        }
    }

    pub(crate) fn paste_line(&mut self) {
        if self.clipboard.is_empty() { return; }
        let current_char = self.get_cursor_char_idx();
        self.buffer.insert(current_char, &self.clipboard);
        let new_idx = current_char + self.clipboard.chars().count();
        self.cursor_y = self.buffer.char_to_line(new_idx);
        self.cursor_x = new_idx - self.buffer.line_to_char(self.cursor_y);
        self.desired_cursor_x = self.cursor_x;
        self.set_status(String::from("Pasted text")); self.mark_modified();
    }

    pub(crate) fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") || path.starts_with("~\\") || path == "~" {
            let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).unwrap_or_default();
            if !home.is_empty() { return path.replacen('~', &home, 1); }
        }
        path.to_string()
    }

    pub(crate) fn read_file(&mut self) -> io::Result<()> {
        if let Some(filepath) = self.prompt("File to insert: ", true)? {
            if filepath.is_empty() { self.set_status(String::from("Read cancelled.")); return Ok(()); }
            match fs::read_to_string(&Self::expand_tilde(&filepath)) {
                Ok(contents) => {
                    let idx = self.get_cursor_char_idx();
                    self.buffer.insert(idx, &contents);
                    self.set_status(format!("Read {} lines", contents.lines().count())); self.mark_modified();
                }
                Err(e) => self.set_status(format!("Error reading file: {}", e)),
            }
        }
        Ok(())
    }

    pub(crate) fn save_file(&mut self) -> io::Result<()> {
        let default_name = self.filename.clone().unwrap_or_default();
        let prompt_text = if default_name.is_empty() { String::from("File Name to Write: ") } else { format!("File Name to Write [{}]: ", default_name) };
        if let Some(mut new_name) = self.prompt(&prompt_text, true)? {
            if new_name.is_empty() {
                if !default_name.is_empty() { new_name = default_name; } else { self.set_status(String::from("Save cancelled: No filename provided.")); return Ok(()); }
            }
            let expanded_path = Self::expand_tilde(&new_name);
            let path = Path::new(&expanded_path);
            if path.exists() && Some(&new_name) != self.filename.as_ref() {
                if let Some(false) | None = self.prompt_yn(&format!("File \"{}\" exists, OVERWRITE ?", new_name))? {
                    self.set_status(String::from("Save cancelled")); return Ok(());
                }
            }
            match File::create(&expanded_path) {
                Ok(file) => {
                    if let Err(e) = self.buffer.write_to(BufWriter::new(file)) { self.set_status(format!("Error writing file: {}", e)); }
                    else {
                        self.filename = Some(new_name); self.highlight_cache.clear();
                        self.set_status(format!("Wrote {} lines", self.buffer.len_lines())); self.is_modified = false;
                    }
                }
                Err(e) => self.set_status(format!("Error creating file: {}", e)),
            }
        }
        Ok(())
    }

    pub fn edit_buffer(&mut self, title: &str, initial_content: &str, editor_type: MenuState) -> io::Result<Option<String>> {
        // 1. SAVE STATE: Bookmark where the editor currently is
        let prev_buffer = self.buffer.clone();
        let prev_menu_state = self.menu_state;
        let prev_top_margin = self.top_margin;
        let prev_cursor_x = self.cursor_x;
        let prev_cursor_y = self.cursor_y;
        let prev_col_offset = self.col_offset;
        let prev_row_offset = self.row_offset;

        let prev_status_message = self.status_message.clone();
        let prev_status_time = self.status_time;

        self.buffer = ropey::Rope::from_str(initial_content);
        self.menu_state = editor_type;
        self.top_margin = 1;

        self.cursor_y = self.buffer.len_lines().saturating_sub(1);
        self.cursor_x = self.line_len(self.cursor_y);
        self.desired_cursor_x = self.cursor_x;
        self.row_offset = 0;
        self.col_offset = 0;

        self.highlight_cache.clear();

        let _ = self.scroll();

        self.status_message = String::new();
        self.status_time = None;

        let mut stdout = std::io::stdout();
        let mut result = None;

        loop {
            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let theme = &self.theme_set.themes[&self.current_theme];
            let colors = crate::theme::derive_ui_colors(theme);

            // Draw the Top Title Bar
            crossterm::queue!(
                stdout,
                crossterm::cursor::MoveTo(0, 0),
                crossterm::style::SetBackgroundColor(colors.menu_bg),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::UntilNewLine),
                crossterm::style::SetForegroundColor(colors.accent),
                crossterm::style::Print(format!(" {} ", title)),
                crossterm::style::ResetColor
            )?;

            self.draw_screen()?;

            crossterm::queue!(stdout, crossterm::cursor::SavePosition)?;

            let m_col = (cols as usize / 6).max(1);
            if self.menu_state == MenuState::TeamEditor {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, m_col,
                    &[("^C", " Cancel"), ("^A", " Add Email"), ("^P", " Prev"), ("^Y", " Prev Pg"), ("", ""), ("", "")],
                    colors.menu_bg, colors.accent, colors.fg
                )?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, m_col,
                    &[("^X", " Save"), ("", ""), ("^N", " Next"), ("^V", " Next Pg"), ("", ""), ("", "")],
                    colors.menu_bg, colors.accent, colors.fg
                )?;
            } else {
                Self::draw_menu_line(
                    &mut stdout, rows - 2, cols, m_col,
                    &[("^X", " Save"), ("^C", " Cancel")],
                    colors.menu_bg, colors.accent, colors.fg
                )?;
                Self::draw_menu_line(
                    &mut stdout, rows - 1, cols, m_col,
                    &[("", "")],
                    colors.menu_bg, colors.accent, colors.fg
                )?;
            }

            crossterm::queue!(
                stdout,
                crossterm::cursor::RestorePosition,
                crossterm::cursor::Show
            )?;

            use std::io::Write;
            stdout.flush()?;

            if crossterm::event::poll(std::time::Duration::from_millis(100))? {
                if let crossterm::event::Event::Key(key_event) = crossterm::event::read()? {
                    if key_event.kind == crossterm::event::KeyEventKind::Press {

                        if key_event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                            if key_event.code == crossterm::event::KeyCode::Char('x') {
                                result = Some(self.buffer.to_string());
                                break;
                            }
                            if key_event.code == crossterm::event::KeyCode::Char('c') {
                                break;
                            }
                            if key_event.code == crossterm::event::KeyCode::Char('a') && self.menu_state == MenuState::TeamEditor {
                                let raw_addrs = crate::address::load_address_book();
                                let mut addrs = Vec::new();
                                for a in raw_addrs {
                                    if !a.trim().is_empty() { addrs.push(a); }
                                }

                                if let Ok(Some(selected)) = self.prompt_select_item("Select Address:", &addrs) {
                                    // Append to the end of the text on a new line
                                    let end_idx = self.buffer.len_chars();
                                    let prefix = if end_idx > 0 && self.buffer.char(end_idx - 1) != '\n' { "\n" } else { "" };
                                    let insert_text = format!("{}{}\n", prefix, selected);

                                    self.buffer.insert(end_idx, &insert_text);

                                    // Move cursor to the new bottom
                                    self.cursor_y = self.buffer.len_lines().saturating_sub(1);
                                    self.cursor_x = self.line_len(self.cursor_y);
                                    self.desired_cursor_x = self.cursor_x;
                                    self.mark_modified();
                                    let _ = self.scroll();
                                }
                                continue;
                            }
                        }

                        let _ = self.handle_keypress(key_event)?;
                    }
                }
            }
        }

        self.buffer = prev_buffer;
        self.menu_state = prev_menu_state;
        self.top_margin = prev_top_margin;
        self.cursor_x = prev_cursor_x;
        self.cursor_y = prev_cursor_y;
        self.col_offset = prev_col_offset;
        self.row_offset = prev_row_offset;
        self.desired_cursor_x = prev_cursor_x;

        self.highlight_cache.clear();

        self.status_message = prev_status_message;
        self.status_time = prev_status_time;

        Ok(result)
    }

}
