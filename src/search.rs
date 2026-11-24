use std::io;

use crate::editor::Editor;
use crate::prompt::PromptExt;
use crate::syntax::SyntaxExt;
use crate::ui::UiExt;

pub trait SearchExt {
    fn where_is(&mut self) -> io::Result<()>;
    fn replace(&mut self) -> io::Result<()>;
}

fn prompt_search_query(editor: &mut Editor, label: &str) -> io::Result<Option<String>> {
    let prompt_text = match &editor.last_search {
        Some(last) => format!("{} [{}]: ", label, last),
        None => format!("{}: ", label),
    };

    let Some(query) = editor.prompt(&prompt_text, false)? else {
        return Ok(None);
    };

    if query.is_empty() {
        if let Some(last) = editor.last_search.clone() {
            Ok(Some(last))
        } else {
            editor.set_status(String::from("Cancelled"));
            Ok(None)
        }
    } else {
        editor.last_search = Some(query.clone());
        Ok(Some(query))
    }
}

fn move_cursor_to_char(editor: &mut Editor, char_idx: usize) {
    editor.cursor_y = editor.buffer.char_to_line(char_idx);
    editor.cursor_x = char_idx - editor.buffer.line_to_char(editor.cursor_y);
    editor.desired_cursor_x = editor.cursor_x;
}

impl SearchExt for Editor {
    fn where_is(&mut self) -> io::Result<()> {
        let previous_state = self.menu_state;
        let result = (|| -> io::Result<()> {
            let Some(query) = prompt_search_query(self, "Search")? else {
                return Ok(());
            };

            let text = self.buffer.to_string();
            let mut start_byte = self.buffer.char_to_byte(self.get_cursor_char_idx());
            if text[start_byte..].starts_with(&query) {
                start_byte += text[start_byte..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8);
            }

            let found = if let Some(offset) = text[start_byte..].find(&query) {
                Some((self.buffer.byte_to_char(start_byte + offset), false))
            } else {
                text.find(&query)
                    .map(|byte_pos| (self.buffer.byte_to_char(byte_pos), true))
            };

            if let Some((match_char, wrapped)) = found {
                move_cursor_to_char(self, match_char);
                self.highlight_match = Some((
                    self.cursor_y,
                    self.cursor_x,
                    self.cursor_x + query.chars().count(),
                ));
                if wrapped {
                    self.set_status(String::from("Search wrapped to top"));
                } else {
                    self.clear_status();
                }
            } else {
                self.set_status(format!("\"{}\" not found", query));
            }

            Ok(())
        })();

        self.menu_state = previous_state;
        result
    }

    fn replace(&mut self) -> io::Result<()> {
        let previous_state = self.menu_state;
        let result = (|| -> io::Result<()> {
            let Some(query) = prompt_search_query(self, "Search (to replace)")? else {
                return Ok(());
            };
            let Some(replacement) = self.prompt("Replace with: ", false)? else {
                return Ok(());
            };

            let query_len = query.chars().count();
            let replacement_len = replacement.chars().count();
            let mut current_char = self.get_cursor_char_idx();
            let mut changes_made = 0;
            let mut replace_all = false;
            let mut wrapped = false;

            loop {
                let text = self.buffer.to_string();
                let start_byte = self.buffer.char_to_byte(current_char);

                let Some(offset) = text[start_byte..].find(&query) else {
                    if current_char > 0 && !wrapped {
                        current_char = 0;
                        wrapped = true;
                        continue;
                    }
                    break;
                };

                let start_char = self.buffer.byte_to_char(start_byte + offset);
                let end_char = start_char + query_len;
                move_cursor_to_char(self, start_char);
                self.scroll()?;

                if !replace_all {
                    self.highlight_match = Some((self.cursor_y, self.cursor_x, self.cursor_x + query_len));
                    let action = self.prompt_replace("Replace this instance?")?;
                    self.highlight_match = None;

                    match action {
                        Some('y') => {}
                        Some('a') => replace_all = true,
                        Some('n') => {
                            current_char = end_char;
                            continue;
                        }
                        None => {
                            self.set_status(String::from("Cancelled"));
                            return Ok(());
                        }
                        _ => unreachable!(),
                    }
                }

                self.buffer.remove(start_char..end_char);
                self.buffer.insert(start_char, &replacement);
                current_char = start_char + replacement_len;
                changes_made += 1;
                self.mark_modified();
            }

            if changes_made > 0 {
                self.set_status(format!("Replaced {} occurrences", changes_made));
            } else {
                self.set_status(String::from("No matches found"));
            }

            Ok(())
        })();

        self.menu_state = previous_state;
        result
    }
}