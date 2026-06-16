use std::{env, fs};
use std::path::PathBuf;
use std::io::{self, stdout, Write};
use crossterm::{
    cursor, event::{self, Event, KeyCode, KeyModifiers}, queue,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType}
};

use crate::editor::Editor;
use crate::ui::UiExt; // Needed for Self::draw_menu_line
use crate::prompt::PromptExt; // Needed for self.prompt and self.prompt_edit
use crate::theme::derive_ui_colors;

pub trait BrowserExt {
    fn run_file_browser(&mut self, is_saving: bool, default_filename: Option<&str>) -> io::Result<Option<String>>;
}

impl BrowserExt for Editor {
    fn run_file_browser(&mut self, is_saving: bool, default_filename: Option<&str>) -> io::Result<Option<String>> {
        // ... PASTE your entire existing run_file_browser body here ...
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

            let title_text = if default_filename == Some("<DIR_ONLY>") {
                format!("xpine - Select Directory: {}", current_dir.display())
            } else {
                format!("xpine - File Browser: {}", current_dir.display())
            };
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
            Self::draw_menu_line(
                &mut stdout, rows - 2, cols, m_col,
                &[ ("", ""), ("P", " Prev"), ("Y", " Prev Pg"), ("Enter", " Select"), ("", ""), ("", ""), ("", "")],
                ui_bg, menu_key_fg, title_fg)?;
            Self::draw_menu_line(
                &mut stdout, rows - 1, cols, m_col,
                &[("^C", " Cancel"),("N", " Next"), ("V", " Next Pg"), ("", ""), ("", "")],
                ui_bg, menu_key_fg, title_fg)?;

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

                                if default_filename == Some("<DIR_ONLY>") {
                                    return Ok(Some(current_dir.to_string_lossy().into_owned()));
                                }

                                let prompt_str = if is_saving { "Save as: " } else { "Attach file: " };

                                let prompt_result = match default_filename {
                                    Some(name) => self.prompt_edit(prompt_str, name),
                                    None => self.prompt(prompt_str, false),
                                };

                                if let Ok(Some(filename)) = prompt_result {
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
}

pub fn open_url(url: &str) -> io::Result<()> {
    if webbrowser::open(url).is_ok() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, "Failed to open web browser"))
    }
}
