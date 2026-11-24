use std::io::{self, stdout, Write};
use std::path::{Path, PathBuf};
use std::{env, fs};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor},
    terminal::{self, ClearType},
};

use crate::editor::Editor;
use crate::prompt::PromptExt;
use crate::theme::derive_ui_colors;
use crate::ui::UiExt;

pub trait BrowserExt {
    fn run_file_browser(
        &mut self,
        is_saving: bool,
        default_filename: Option<&str>,
    ) -> io::Result<Option<String>>;
}

fn read_directory_entries(dir: &Path) -> Vec<(String, bool)> {
    let mut entries = vec![(".".to_string(), true)];
    if dir.parent().is_some() {
        entries.push(("..".to_string(), true));
    }

    if let Ok(read_dir) = fs::read_dir(dir) {
        let mut items: Vec<(String, bool)> = read_dir
            .flatten()
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                (name, is_dir)
            })
            .collect();

        items.sort_by_cached_key(|(name, is_dir)| {
            (name.starts_with('.'), !*is_dir, name.to_lowercase())
        });
        entries.extend(items);
    }

    entries
}

impl BrowserExt for Editor {
    fn run_file_browser(
        &mut self,
        is_saving: bool,
        default_filename: Option<&str>,
    ) -> io::Result<Option<String>> {
        let mut current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut entries = read_directory_entries(&current_dir);
        let mut selected_idx = 0;
        let mut scroll_offset = 0;
        let mut error_msg = String::new();
        let mut stdout = stdout();

        loop {
            let (cols, rows) = terminal::size()?;
            let theme = &self.theme_set.themes[&self.current_theme];
            let ui_colors = derive_ui_colors(theme);
            let ui_bg = ui_colors.menu_bg;
            let title_fg = ui_colors.fg;
            let menu_key_fg = ui_colors.accent;

            selected_idx = selected_idx.min(entries.len().saturating_sub(1));

            let title_prefix = if default_filename == Some("<DIR_ONLY>") {
                "xpine - Select Directory"
            } else {
                "xpine - File Browser"
            };

            queue!(
                stdout,
                SetBackgroundColor(ui_colors.bg),
                terminal::Clear(ClearType::All),
                cursor::MoveTo(0, 0),
                SetBackgroundColor(ui_bg),
                terminal::Clear(ClearType::CurrentLine),
                SetForegroundColor(menu_key_fg),
                Print(format!("{}: {}", title_prefix, current_dir.display()))
            )?;

            let visible_rows = (rows.saturating_sub(4) as usize).max(1);

            if selected_idx < scroll_offset {
                scroll_offset = selected_idx;
            } else if selected_idx >= scroll_offset + visible_rows {
                scroll_offset = selected_idx - visible_rows + 1;
            }
            scroll_offset = scroll_offset.min(entries.len().saturating_sub(visible_rows));

            for (i, (name, is_dir)) in entries
                .iter()
                .skip(scroll_offset)
                .take(visible_rows)
                .enumerate()
            {
                let actual_idx = scroll_offset + i;
                let is_selected = actual_idx == selected_idx;
                let row_y = i as u16 + 1;
                let row_bg = if is_selected {
                    ui_colors.selected_bg
                } else {
                    ui_colors.bg
                };
                let fg_color = if is_selected {
                    Color::White
                } else if *is_dir {
                    menu_key_fg
                } else {
                    title_fg
                };
                let prefix = if *is_dir { "[DIR] " } else { "      " };

                queue!(
                    stdout,
                    cursor::MoveTo(0, row_y),
                    SetBackgroundColor(row_bg),
                    terminal::Clear(ClearType::CurrentLine),
                    cursor::MoveTo(2, row_y),
                    SetForegroundColor(fg_color),
                    Print(format!("{}{}", prefix, name))
                )?;
            }

            if !error_msg.is_empty() {
                queue!(
                    stdout,
                    cursor::MoveTo(0, rows.saturating_sub(3)),
                    SetBackgroundColor(ui_bg),
                    terminal::Clear(ClearType::CurrentLine),
                    SetForegroundColor(Color::Red),
                    Print(format!("   {}", error_msg))
                )?;
            }

            let m_col = (cols as usize / 6).max(1);
            Self::draw_menu_line(
                &mut stdout,
                rows.saturating_sub(2),
                cols,
                m_col,
                &[
                    ("", ""),
                    ("P", " Prev"),
                    ("Y", " Prev Pg"),
                    ("Enter", " Select"),
                    ("", ""),
                    ("", ""),
                ],
                ui_bg,
                menu_key_fg,
                title_fg,
            )?;
            Self::draw_menu_line(
                &mut stdout,
                rows.saturating_sub(1),
                cols,
                m_col,
                &[
                    ("^C", " Cancel"),
                    ("N", " Next"),
                    ("V", " Next Pg"),
                    ("", ""),
                    ("", ""),
                    ("", ""),
                ],
                ui_bg,
                menu_key_fg,
                title_fg,
            )?;

            stdout.flush()?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            error_msg.clear();
            match key.code {
                KeyCode::Up | KeyCode::Char('p' | 'P') => {
                    selected_idx = selected_idx.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('n' | 'N') => {
                    if selected_idx + 1 < entries.len() {
                        selected_idx += 1;
                    }
                }
                KeyCode::PageUp | KeyCode::Char('y' | 'Y') => {
                    selected_idx = selected_idx.saturating_sub(visible_rows);
                }
                KeyCode::PageDown | KeyCode::Char('v' | 'V') => {
                    selected_idx = (selected_idx + visible_rows).min(entries.len().saturating_sub(1));
                }
                KeyCode::Home => {
                    selected_idx = 0;
                    scroll_offset = 0;
                }
                KeyCode::End => {
                    selected_idx = entries.len().saturating_sub(1);
                }
                KeyCode::Enter | KeyCode::Right => {
                    if let Some((name, is_dir)) = entries.get(selected_idx).cloned() {
                        match (name.as_str(), is_dir) {
                            (".", _) => {
                                if default_filename == Some("<DIR_ONLY>") {
                                    return Ok(Some(current_dir.to_string_lossy().into_owned()));
                                }

                                let prompt_str =
                                    if is_saving { "Save as: " } else { "Attach file: " };
                                let prompt_result = match default_filename {
                                    Some(default_name) => self.prompt_edit(prompt_str, default_name),
                                    None => self.prompt(prompt_str, false),
                                };

                                if let Ok(Some(filename)) = prompt_result {
                                    let trimmed = filename.trim();
                                    if !trimmed.is_empty() {
                                        let target_path = current_dir.join(trimmed);
                                        if !is_saving && !target_path.exists() {
                                            error_msg = format!(
                                                "File '{}' does not exist in this directory.",
                                                trimmed
                                            );
                                        } else {
                                            return Ok(Some(
                                                target_path.to_string_lossy().into_owned(),
                                            ));
                                        }
                                    }
                                }
                            }
                            ("..", _) => {
                                if let Some(parent) = current_dir.parent() {
                                    current_dir = parent.to_path_buf();
                                    entries = read_directory_entries(&current_dir);
                                    selected_idx = 0;
                                    scroll_offset = 0;
                                }
                            }
                            (_, true) => {
                                current_dir = current_dir.join(&name);
                                entries = read_directory_entries(&current_dir);
                                selected_idx = 0;
                                scroll_offset = 0;
                            }
                            (_, false) => {
                                return Ok(Some(
                                    current_dir.join(&name).to_string_lossy().into_owned(),
                                ));
                            }
                        }
                    }
                }
                KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                KeyCode::Esc => return Ok(None),
                _ => {}
            }
        }
    }
}

pub fn open_url(url: &str) -> io::Result<()> {
    webbrowser::open(url)
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "Failed to open web browser"))
}