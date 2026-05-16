use std::io;
use crate::editor::{Editor, MenuState}; // Ensure MenuState is imported
use crate::ui::UiExt;
use crate::syntax::SyntaxExt;

pub trait SearchExt {
    fn where_is(&mut self) -> io::Result<()>;
    fn replace(&mut self) -> io::Result<()>;
}

impl SearchExt for Editor {
    fn where_is(&mut self) -> io::Result<()> {
        let previous_state = self.menu_state;
        let prompt_text = if let Some(ref last) = self.last_search { format!("Search [{}]: ", last) } else { String::from("Search: ") };
        if let Some(mut query) = self.prompt(&prompt_text, false)? {
            if query.is_empty() {
                if let Some(ref last) = self.last_search {
                    query = last.clone();
                } else {
                    self.set_status(String::from("Cancelled"));
                    self.menu_state = previous_state;
                    return Ok(());
                }
            } else { self.last_search = Some(query.clone()); }

            let text = self.buffer.to_string();
            let mut start_char = self.get_cursor_char_idx();
            if text[start_char..].starts_with(&query) { start_char += 1; }

            if let Some(pos) = text[start_char..].find(&query) {
                let absolute_pos = start_char + pos;
                self.cursor_y = self.buffer.char_to_line(absolute_pos);
                self.cursor_x = absolute_pos - self.buffer.line_to_char(self.cursor_y);
                self.desired_cursor_x = self.cursor_x;
                self.highlight_match = Some((self.cursor_y, self.cursor_x, self.cursor_x + query.chars().count()));
                self.clear_status();
            } else {
                if let Some(pos) = text.find(&query) {
                    self.cursor_y = self.buffer.char_to_line(pos);
                    self.cursor_x = pos - self.buffer.line_to_char(self.cursor_y);
                    self.desired_cursor_x = self.cursor_x;
                    self.highlight_match = Some((self.cursor_y, self.cursor_x, self.cursor_x + query.chars().count()));
                    self.set_status(String::from("Search wrapped to top"));
                } else {
                    self.set_status(format!("\"{}\" not found", query));
                }
            }
        }
        self.menu_state = previous_state;
        Ok(())
    }

    fn replace(&mut self) -> io::Result<()> {
        let is_composer = self.menu_state == MenuState::EmailComposer;
        let prompt_text = if let Some(ref last) = self.last_search { format!("Search (to replace) [{}]: ", last) } else { String::from("Search (to replace): ") };
        if let Some(mut query) = self.prompt(&prompt_text, false)? {
            if query.is_empty() {
                if let Some(ref last) = self.last_search {
                    query = last.clone();
                } else {
                    self.set_status(String::from("Cancelled"));
                    if is_composer { self.menu_state = MenuState::EmailComposer; }
                    return Ok(());
                }
            } else { self.last_search = Some(query.clone()); }

            if let Some(replacement) = self.prompt("Replace with: ", false)? {
                let mut current_idx = self.get_cursor_char_idx();
                let mut changes_made = 0;
                let mut replace_all = false;
                let mut wrapped = false;

                loop {
                    let text = self.buffer.to_string();
                    if let Some(pos) = text[current_idx..].find(&query) {
                        let start_idx = current_idx + pos;
                        let end_idx = start_idx + query.chars().count();
                        self.cursor_y = self.buffer.char_to_line(start_idx);
                        self.cursor_x = start_idx - self.buffer.line_to_char(self.cursor_y);
                        self.desired_cursor_x = self.cursor_x;
                        self.scroll()?;

                        if replace_all {
                            self.buffer.remove(start_idx..end_idx);
                            self.buffer.insert(start_idx, &replacement);
                            current_idx = start_idx + replacement.chars().count();
                            changes_made += 1; self.mark_modified(); continue;
                        }

                        self.highlight_match = Some((self.cursor_y, self.cursor_x, self.cursor_x + query.chars().count()));
                        let prompt_result = self.prompt_replace("Replace this instance?");
                        self.highlight_match = None;

                        if let Some(action) = prompt_result? {
                            match action {
                                'y' => {
                                    self.buffer.remove(start_idx..end_idx); self.buffer.insert(start_idx, &replacement);
                                    current_idx = start_idx + replacement.chars().count(); changes_made += 1; self.mark_modified();
                                }
                                'n' => { current_idx = end_idx; }
                                'a' => {
                                    replace_all = true; self.buffer.remove(start_idx..end_idx); self.buffer.insert(start_idx, &replacement);
                                    current_idx = start_idx + replacement.chars().count(); changes_made += 1; self.mark_modified();
                                }
                                _ => unreachable!()
                            }
                        } else {
                            self.set_status(String::from("Cancelled"));
                            if is_composer { self.menu_state = MenuState::EmailComposer; }
                            return Ok(());
                        }
                    } else {
                        if current_idx > 0 && !wrapped { current_idx = 0; wrapped = true; } else { break; }
                    }
                }
                if changes_made > 0 { self.set_status(format!("Replaced {} occurrences", changes_made)); } else { self.set_status(String::from("No matches found")); }
            }
        }
        if is_composer { self.menu_state = MenuState::EmailComposer; }
        Ok(())
    }
}
