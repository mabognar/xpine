use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::config::ConfigExt;
use crate::editor::{Editor, MenuState};
use crate::prompt::PromptExt;
use crate::syntax::SyntaxExt;
use crate::ui::UiExt;

pub trait SpellExt {
    fn load_dictionary() -> HashSet<String>;
    fn find_next_misspelled(&self, start_idx: usize) -> Option<(String, usize, usize)>;
    fn spell_check(&mut self) -> io::Result<bool>;
    fn get_suggestions(word: &str, dict: &HashSet<String>) -> Vec<String>;
}

fn custom_dict_path() -> Option<PathBuf> {
    Editor::get_base_dir().map(|dir| dir.join("custom_dict.txt"))
}

fn load_words_from_file(path: &Path, dict: &mut HashSet<String>) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        dict.insert(line.trim().to_lowercase());
    }
    true
}

fn apply_casing(template: &str, suggestions: &mut [String]) {
    if template.chars().all(char::is_uppercase) {
        for sugg in suggestions {
            *sugg = sugg.to_uppercase();
        }
    } else if template.chars().next().is_some_and(char::is_uppercase) {
        for sugg in suggestions {
            let mut chars = sugg.chars();
            if let Some(first_char) = chars.next() {
                *sugg = first_char.to_uppercase().collect::<String>() + chars.as_str();
            }
        }
    }
}

impl SpellExt for Editor {
    fn load_dictionary() -> HashSet<String> {
        let mut dict = HashSet::new();

        for path in ["/usr/share/dict/words", "/usr/dict/words"] {
            if load_words_from_file(Path::new(path), &mut dict) {
                break;
            }
        }

        if let Some(custom_path) = custom_dict_path() {
            load_words_from_file(&custom_path, &mut dict);
        }

        dict
    }

    fn find_next_misspelled(&self, start_idx: usize) -> Option<(String, usize, usize)> {
        let dict = self.dictionary.as_ref()?;
        if start_idx >= self.buffer.len_chars() {
            return None;
        }

        let start_line = self.buffer.char_to_line(start_idx);

        for line_idx in start_line..self.buffer.len_lines() {
            let line = self.buffer.line(line_idx);
            let line_str = line.to_string();
            let trimmed = line_str.trim_end();

            // Skip quoted replies and stop at forwarded messages
            if line_str.starts_with('>')
                || (trimmed.starts_with("On ") && trimmed.ends_with(" wrote:"))
            {
                continue;
            }
            if line_str.contains("--- Forwarded message ---") {
                return None;
            }

            let line_start_char = self.buffer.line_to_char(line_idx);
            let char_offset = if line_idx == start_line {
                start_idx.saturating_sub(line_start_char)
            } else {
                0
            };

            let mut word_start = 0;
            let mut word = String::new();

            for (i, c) in line.chars().enumerate().skip(char_offset) {
                let actual_idx = line_start_char + i;
                if c.is_alphabetic() {
                    if word.is_empty() {
                        word_start = actual_idx;
                    }
                    word.push(c);
                } else if !word.is_empty() {
                    if !dict.contains(&word.to_lowercase()) {
                        return Some((word, word_start, actual_idx));
                    }
                    word.clear();
                }
            }

            if !word.is_empty() && !dict.contains(&word.to_lowercase()) {
                return Some((word, word_start, line_start_char + line.len_chars()));
            }
        }

        None
    }

    fn spell_check(&mut self) -> io::Result<bool> {
        let previous_state = self.menu_state;
        self.dictionary.get_or_insert_with(Self::load_dictionary);

        let mut current_idx = 0;
        let mut corrections = 0;

        while let Some((word, start, end)) = self.find_next_misspelled(current_idx) {
            let lower_word = word.to_lowercase();
            if self.ignored_words.contains(&lower_word) {
                current_idx = end;
                continue;
            }

            self.cursor_y = self.buffer.char_to_line(start);
            self.cursor_x = start - self.buffer.line_to_char(self.cursor_y);
            self.desired_cursor_x = self.cursor_x;
            self.scroll()?;

            let word_len = word.chars().count();
            self.highlight_match = Some((self.cursor_y, self.cursor_x, self.cursor_x + word_len));

            let dict = self.dictionary.as_ref().unwrap();
            let mut suggestions = Self::get_suggestions(&lower_word, dict);
            apply_casing(&word, &mut suggestions);
            suggestions.truncate(5);

            self.current_suggestions = suggestions;
            self.menu_state = MenuState::SpellCheck;
            self.draw_screen()?;

            let choice_result = self.prompt("Replace with: ", false)?;
            let suggestions = std::mem::take(&mut self.current_suggestions);

            let Some(choice) = choice_result else {
                self.highlight_match = None;
                self.set_status(String::from("Spell check cancelled"));
                self.menu_state = previous_state;
                return Ok(false);
            };

            let choice_clean = choice.trim().to_lowercase();
            let replacement: Option<&str> = match choice_clean.as_str() {
                "i" => {
                    self.ignored_words.insert(lower_word);
                    None
                }
                "a" => {
                    if let Some(dict) = self.dictionary.as_mut() {
                        dict.insert(lower_word.clone());
                    }
                    if let Some(custom_path) = custom_dict_path() {
                        if let Ok(mut file) = OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(custom_path)
                        {
                            let _ = writeln!(file, "{}", lower_word);
                        }
                    }
                    None
                }
                _ => {
                    if let Ok(num) = choice_clean.parse::<usize>() {
                        num.checked_sub(1)
                            .and_then(|i| suggestions.get(i))
                            .map(String::as_str)
                    } else if !choice.is_empty() {
                        Some(choice.as_str())
                    } else {
                        None
                    }
                }
            };

            if let Some(rep) = replacement {
                self.buffer.remove(start..end);
                self.buffer.insert(start, rep);
                current_idx = start + rep.chars().count();
                corrections += 1;
                self.mark_modified();
            } else {
                current_idx = end;
            }

            self.highlight_match = None;
        }

        self.set_status(format!(
            "Spell check complete. {} corrections made.",
            corrections
        ));
        self.menu_state = previous_state;
        self.draw_screen()?;

        Ok(true)
    }

    fn get_suggestions(word: &str, dict: &HashSet<String>) -> Vec<String> {
        let mut scored: Vec<(&String, usize)> = dict
            .iter()
            .filter(|w| w.len().abs_diff(word.len()) <= 2)
            .filter_map(|w| {
                let dist = edit_distance(word, w);
                (dist <= 3).then_some((w, dist))
            })
            .collect();

        scored.sort_by_key(|&(_, dist)| dist);
        scored.into_iter().take(6).map(|(w, _)| w.clone()).collect()
    }
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, &a_char) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &b_char) in b.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}