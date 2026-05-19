mod app;
mod compose;
mod config;
mod editor;
mod events;
mod mail;
mod net;
mod spell;
mod ui;

pub mod theme;
pub mod syntax;
pub mod search;

use app::{App, AppMode};
use config::load_config;
use editor::{Editor, MenuState, EditorResult};
use ui::UiExt;

use ropey::Rope;
use crossterm::{
    cursor, event, execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, size as term_size, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::stdout;
use std::time::{Duration, Instant};

fn main() {
    let config = load_config();
    let mut app = App::new(config.accounts);

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    let mut theme_provider = Editor::new(None);
    let mut session = net::connect(&app.active_account).unwrap();

    loop {
        if app.needs_reconnect {
            app.active_account = app.accounts[app.current_account_idx].clone();
            let _ = session.logout();
            session = net::connect(&app.active_account).expect("IMAP Login failed");
            app.needs_fetch = true;
            app.needs_reconnect = false;
            app.last_fetch_time = Instant::now();
        }

        if app.last_fetch_time.elapsed() >= app.auto_refresh_interval {
            app.needs_fetch = true;
        }

        let (_, rows) = term_size().unwrap_or((80, 24));
        let items_per_page = (rows.saturating_sub(3) as u32).max(1);
        let total_pages = if app.total_messages == 0 { 1 } else { (app.total_messages + items_per_page - 1) / items_per_page };

        if app.current_page >= total_pages {
            app.current_page = total_pages.saturating_sub(1);
            app.needs_fetch = true;
        }

        if app.needs_fetch && matches!(app.mode, AppMode::List) {
            net::fetch_emails(&mut session, &mut app, items_per_page, theme_provider.sort_newest_first);
            app.last_fetch_time = Instant::now();
            app.needs_fetch = false;
        }

        if let AppMode::Reading { text_body, html_body, attachments } = &app.mode {
            let mut reader = Editor::new(None);
            reader.menu_state = MenuState::EmailReader;

            let attach_lines = if attachments.is_empty() { 1 } else { attachments.len() };
            reader.top_margin = (5 + attach_lines) as u16;

            reader.buffer = Rope::from_str(text_body.as_str());
            reader.current_theme = theme_provider.current_theme.clone();

            // Set the status message if an HTML body is present
            if let Some(html) = html_body {
                if !html.is_empty() {
                    reader.set_status("Email contains HTML. Type 'B' to view in browser".to_string());
                }
            }

            let email_from = app.page_emails[app.selected_index].from.clone();
            let email_to = app.page_emails[app.selected_index].to_addr.clone();
            let email_cc = app.page_emails[app.selected_index].cc.clone();
            let email_subject = app.page_emails[app.selected_index].subject.clone();
            let active_email = app.active_account.email.clone();

            let reply_to = app.page_emails[app.selected_index].reply_to.clone();
            let date = app.page_emails[app.selected_index].date.clone();
            let fetch_seq = app.page_emails[app.selected_index].id.to_string();

            loop {
                let r_theme = &reader.theme_set.themes[&reader.current_theme];
                let r_colors = ui::derive_ui_colors(r_theme);

                for i in 0..(5 + attach_lines) {
                    queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(r_colors.ui_bg), Clear(ClearType::UntilNewLine)).unwrap();
                }

                let header_title = format!("View Email ({})", active_email);
                queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(r_colors.accent), Print(header_title)).unwrap();

                let fields = ["From:", "To:", "Cc:", "Subject:"];
                let vals = [&email_from, &email_to, &email_cc, &email_subject];

                for i in 0..4 {
                    queue!(
                        stdout, cursor::MoveTo(0, (i + 1) as u16),
                        SetBackgroundColor(r_colors.ui_bg), SetForegroundColor(r_colors.accent), Print(format!("{:>8}", fields[i])),
                        SetForegroundColor(r_colors.fg), Print(" "), Print(vals[i])
                    ).unwrap();
                }

                queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(r_colors.ui_bg), SetForegroundColor(r_colors.accent), Print(" Attach: ")).unwrap();

                if attachments.is_empty() {
                    let dim_c = if r_colors.is_dark { Color::DarkGrey } else { Color::Grey };
                    queue!(stdout, SetForegroundColor(dim_c), Print("None")).unwrap();
                } else {
                    let att_color = if r_colors.is_dark {
                        Color::Rgb { r: 255, g: 80, b: 80 }
                    } else {
                        Color::Rgb { r: 220, g: 0, b: 0 }
                    };

                    for (i, (n, data)) in attachments.iter().enumerate() {
                        let size_kb = (data.len() as f32 / 1024.0).max(1.0);
                        let size_str = if size_kb < 1024.0 { format!("{:.0}K", size_kb) } else { format!("{:.1}M", size_kb / 1024.0) };
                        let att_str = format!("{}. {} ({})", i + 1, n, size_str);

                        queue!(stdout, cursor::MoveTo(9, (5 + i) as u16), SetBackgroundColor(r_colors.ui_bg), SetForegroundColor(att_color), Print(att_str)).unwrap();
                    }
                }
                queue!(stdout, ResetColor).unwrap();

                reader.draw_screen().unwrap();

                let timeout = if let Some(time) = reader.status_time {
                    let elapsed = time.elapsed();
                    if elapsed >= Duration::from_secs(3) {
                        reader.clear_status();
                        Duration::from_millis(1)
                    } else {
                        Duration::from_secs(3) - elapsed
                    }
                } else {
                    Duration::from_secs(3600)
                };

                if event::poll(timeout).unwrap() {
                    let ev = event::read().unwrap();
                    if let event::Event::Key(mut key) = ev {
                        if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == event::KeyCode::Char('y') {
                            reader.set_status("Text copied to clipboard".to_string());
                            continue;
                        }

                        if !key.modifiers.contains(event::KeyModifiers::CONTROL) && !key.modifiers.contains(event::KeyModifiers::ALT) {

                            if key.code == event::KeyCode::Char('a') || key.code == event::KeyCode::Char('A') {
                                if let Ok(added) = crate::config::add_to_address_book(&email_from) {
                                    if added {
                                        reader.set_status(format!("Added {} to address book.", email_from));
                                    } else {
                                        reader.set_status("Address already in address book".to_string());
                                    }
                                }
                                continue;
                            }

                            if key.code == event::KeyCode::Char('r') || key.code == event::KeyCode::Char('R') {
                                let _ = session.store(&fetch_seq, "+FLAGS (\\Answered)");
                                app.page_emails[app.selected_index].is_answered = true;

                                let sub = if email_subject.to_lowercase().starts_with("re:") { email_subject.clone() } else { format!("Re: {}", email_subject) };
                                if let Some(s) = compose::compose_email(&app.active_account, Some(&reply_to), Some(&sub), None, &mut reader.current_theme) {
                                    reader.set_status(s);
                                }
                                continue;
                            }

                            if key.code == event::KeyCode::Char('f') || key.code == event::KeyCode::Char('F') {
                                let sub = if email_subject.to_lowercase().starts_with("fwd:") { email_subject.clone() } else { format!("Fwd: {}", email_subject) };
                                let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", email_from, date, email_subject, text_body);
                                if let Some(s) = compose::compose_email(&app.active_account, None, Some(&sub), Some(&fwd_body), &mut reader.current_theme) {
                                    reader.set_status(s);
                                }
                                continue;
                            }

                            if key.code == event::KeyCode::Char('b') || key.code == event::KeyCode::Char('B') {
                                let temp_dir = std::env::temp_dir().join("xpine_attachments");
                                let _ = std::fs::create_dir_all(&temp_dir);

                                let opened = if let Some(html) = html_body {
                                    if !html.is_empty() {
                                        let file_path = temp_dir.join("email_view.html");
                                        if std::fs::write(&file_path, html).is_ok() {
                                            if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                                reader.set_status("Opened HTML version in browser.".to_string());
                                                true
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if !opened {
                                    let file_path = temp_dir.join("email_view.txt");
                                    if std::fs::write(&file_path, text_body).is_ok() {
                                        if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                            reader.set_status("Opened text version in browser.".to_string());
                                        } else {
                                            reader.set_status("Failed to open browser.".to_string());
                                        }
                                    } else {
                                        reader.set_status("Failed to save text file.".to_string());
                                    }
                                }
                                continue;
                            }

                            if key.code == event::KeyCode::Char('s') || key.code == event::KeyCode::Char('S') {
                                if let Ok(Some(path)) = reader.run_file_browser(true) {
                                    if std::fs::write(&path, text_body.as_bytes()).is_ok() {
                                        reader.set_status(format!("Saved to {}", path));
                                    } else {
                                        reader.set_status(format!("Failed to save to {}", path));
                                    }
                                }
                                continue;
                            }

                            if let event::KeyCode::Char(c) = key.code {
                                if c.is_ascii_digit() && c != '0' {
                                    let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
                                    if idx < attachments.len() {
                                        let (filename, data) = &attachments[idx];
                                        let temp_dir = std::env::temp_dir().join("xpine_attachments");
                                        let _ = std::fs::create_dir_all(&temp_dir);
                                        let file_path = temp_dir.join(filename);
                                        if std::fs::write(&file_path, data).is_ok() {
                                            if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                                reader.set_status(format!("Opened {}", filename));
                                            } else {
                                                reader.set_status(format!("Failed to open {}", filename));
                                            }
                                        } else {
                                            reader.set_status(format!("Failed to save {}", filename));
                                        }
                                        continue;
                                    }
                                }
                            }
                        }

                        match reader.handle_keypress(key).unwrap() {
                            EditorResult::Cancel => break,
                            _ => {}
                        }
                    } else if let event::Event::Resize(_, _) = ev {}
                }
            }
            theme_provider.current_theme = reader.current_theme;

            // Only revert to List mode if we didn't just intentionally switch to the FileBrowser
            if matches!(app.mode, AppMode::Reading { .. }) {
                app.mode = AppMode::List;
            }
            continue;
        }
        
        ui::draw_app(&mut stdout, &app, &theme_provider).unwrap();

        let mut timeout = if app.last_fetch_time.elapsed() >= app.auto_refresh_interval { Duration::from_millis(1) } else { app.auto_refresh_interval - app.last_fetch_time.elapsed() };

        if let Some(time) = app.list_status_time {
            let elapsed = time.elapsed();
            if elapsed >= app.list_status_duration {
                app.list_status.clear();
                app.list_status_time = None;
                timeout = Duration::from_millis(1);
            } else {
                timeout = timeout.min(app.list_status_duration - elapsed);
            }
        }

        if event::poll(timeout).unwrap() {
            if events::handle_event(event::read().unwrap(), &mut app, &mut session, &mut theme_provider, &mut stdout) {
                break;
            }
        }
    }

    execute!(stdout, LeaveAlternateScreen).unwrap();
    disable_raw_mode().expect("Failed to disable raw mode");
    let _ = session.logout();
}
