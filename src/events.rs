use crate::app::{App, AppMode};
use crate::net::{self, ImapSession};
use crate::compose::compose_email;
use crate::editor::Editor;
use crate::config::ConfigExt;
use crate::ui::UiExt;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::size as term_size;

pub fn handle_event(event: Event, app: &mut App, session: &mut ImapSession, theme_provider: &mut Editor, stdout: &mut std::io::Stdout) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {
            match &mut app.mode {
                AppMode::Settings { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(1),

                        // Use Left or '<' to go back, replacing Esc
                        KeyCode::Left | KeyCode::Char('<') | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('s') | KeyCode::Char('S') => app.mode = AppMode::MainMenu { selected_idx: 3 },

                        // Right arrow or Enter to toggle
                        KeyCode::Right | KeyCode::Enter => {
                            if *selected_idx == 0 { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_config(); }
                            else if *selected_idx == 1 { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_config(); }
                        }

                        KeyCode::Char('w') | KeyCode::Char('W') => { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_config(); }
                        KeyCode::Char('l') | KeyCode::Char('L') => { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_config(); }
                        _ => {}
                    }
                }
                AppMode::AddressBook { selected_idx, addresses } => {
                    match k.code {
                        KeyCode::Up => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down => if *selected_idx + 1 < addresses.len() { *selected_idx += 1; },
                        KeyCode::Esc => app.mode = AppMode::MainMenu { selected_idx: 1 },
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if !addresses.is_empty() {
                                let prompt_msg = format!("Delete '{}'? (y/n)", addresses[*selected_idx]);
                                if let Ok(Some(true)) = theme_provider.prompt_yn(&prompt_msg) {
                                    addresses.remove(*selected_idx);
                                    let _ = crate::config::save_address_book(addresses);
                                    if *selected_idx >= addresses.len() {
                                        *selected_idx = addresses.len().saturating_sub(1);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('e') | KeyCode::Char('E') => {
                            if !addresses.is_empty() {
                                let prompt_msg = format!("Replace '{}' with: ", addresses[*selected_idx]);
                                if let Ok(Some(new_val)) = theme_provider.prompt(&prompt_msg, false) {
                                    if !new_val.trim().is_empty() {
                                        addresses[*selected_idx] = new_val.trim().to_string();
                                        let _ = crate::config::save_address_book(addresses);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                AppMode::MainMenu { selected_idx } => {
                    match k.code {
                        KeyCode::Up => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down => *selected_idx = (*selected_idx + 1).min(5),
                        KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('M') => app.mode = AppMode::List,
                        KeyCode::Char('q') | KeyCode::Char('Q') => quit = true,
                        KeyCode::Enter => {
                            match *selected_idx {
                                0 => { // Inbox
                                    app.current_folder = "INBOX".to_string();
                                    app.current_page = 0; app.selected_index = 0; app.needs_fetch = true;
                                    app.mode = AppMode::List;
                                }
                                1 => app.mode = AppMode::AddressBook { selected_idx: 0, addresses: crate::config::load_address_book() },
                                2 => app.mode = AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() },
                                3 => app.mode = AppMode::Settings { selected_idx: 0 },
                                4 => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::List; },
                                5 => quit = true,
                                _ => {}
                            }
                        }
                        KeyCode::Char('i') | KeyCode::Char('I') => {
                            app.current_folder = "INBOX".to_string();
                            app.current_page = 0; app.selected_index = 0; app.needs_fetch = true;
                            app.mode = AppMode::List;
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => app.mode = AppMode::AddressBook { selected_idx: 0, addresses: crate::config::load_address_book() },
                        KeyCode::Char('f') | KeyCode::Char('F') => app.mode = AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() },
                        KeyCode::Char('s') | KeyCode::Char('S') => app.mode = AppMode::Settings { selected_idx: 0 },
                        KeyCode::Char('h') | KeyCode::Char('H') => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::List; },
                        _ => {}
                    }
                }
                AppMode::FolderList { step, selected_idx, folders } => {
                    let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };
                    match k.code {
                        KeyCode::Up => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down => *selected_idx = (*selected_idx + 1).min(items_count.saturating_sub(1)),
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            app.mode = AppMode::MainMenu { selected_idx: 2 };
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                            if *step == 1 { *step = 0; *selected_idx = app.current_account_idx; }
                            else { app.mode = AppMode::MainMenu { selected_idx: 2 }; }
                        }
                        KeyCode::Enter => {
                            if *step == 0 {
                                let new_idx = *selected_idx;
                                if new_idx != app.current_account_idx {
                                    app.current_account_idx = new_idx;
                                    app.active_account = app.accounts[app.current_account_idx].clone();
                                    let _ = session.logout();
                                    *session = net::connect(&app.active_account.email, &app.active_account.password).unwrap();
                                }

                                let mut fetched = Vec::new();
                                if let Ok(mailboxes) = session.list(Some(""), Some("*")) {
                                    for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                                }
                                if fetched.is_empty() { fetched.push("INBOX".to_string()); }
                                fetched.sort();
                                *folders = fetched;
                                *step = 1;
                                *selected_idx = 0;
                            } else {
                                app.current_folder = folders[*selected_idx].clone();
                                app.current_page = 0; app.selected_index = 0; app.needs_fetch = true;
                                app.mode = AppMode::List;
                            }
                        }
                        _ => {}
                    }
                }
                AppMode::List => {
                    let (_, rows) = term_size().unwrap_or((80, 24));
                    let items_per_page = (rows.saturating_sub(3) as u32).max(1);
                    let total_pages = if app.total_messages == 0 { 1 } else { (app.total_messages + items_per_page - 1) / items_per_page };

                    match k.code {
                        KeyCode::Char('t') | KeyCode::Char('T') if k.modifiers.contains(KeyModifiers::ALT) => {
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();
                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                                app.update_status(format!("Theme: {}", theme_provider.current_theme));
                            }
                        }
                        KeyCode::Char('<') | KeyCode::Left => {
                            if app.current_folder.eq_ignore_ascii_case("INBOX") {
                                app.mode = AppMode::MainMenu { selected_idx: 0 };
                            } else {
                                let separator = if app.current_folder.contains('/') { '/' }
                                else if app.current_folder.contains('.') { '.' }
                                else { '\0' };

                                if separator != '\0' {
                                    let mut parts: Vec<&str> = app.current_folder.split(separator).collect();
                                    parts.pop();
                                    if parts.is_empty() || (parts.len() == 1 && parts[0] == "") {
                                        app.mode = AppMode::MainMenu { selected_idx: 0 };
                                    } else {
                                        app.current_folder = parts.join(&separator.to_string());
                                        app.current_page = 0;
                                        app.selected_index = 0;
                                        app.needs_fetch = true;
                                    }
                                } else {
                                    app.mode = AppMode::MainMenu { selected_idx: 0 };
                                }
                            }
                        }
                        KeyCode::Tab => {
                            if app.accounts.len() > 1 {
                                app.current_account_idx = (app.current_account_idx + 1) % app.accounts.len();
                                app.needs_reconnect = true;
                                app.restore_index_from_end = Some(0);

                                let email = &app.accounts[app.current_account_idx].email;
                                app.update_status(format!("Switching to {}...", email));
                                let _ = crate::ui::draw_app(stdout, app, theme_provider);
                            }
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(digit) = c.to_digit(10) {
                                let idx = (digit as usize).saturating_sub(1);
                                if idx < app.accounts.len() && idx != app.current_account_idx {
                                    app.current_account_idx = idx;
                                    app.needs_reconnect = true;
                                    app.restore_index_from_end = Some(0);

                                    let email = &app.accounts[app.current_account_idx].email;
                                    app.update_status(format!("Switching to {}...", email));
                                    let _ = crate::ui::draw_app(stdout, app, theme_provider);
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
                            if app.selected_index > 0 { app.selected_index -= 1; }
                            else if app.current_page + 1 < total_pages { app.current_page += 1; app.needs_fetch = true; app.selected_index = (items_per_page - 1) as usize; }
                        }
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                            if !app.page_emails.is_empty() {
                                let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                                if app.selected_index + 1 < max_visible { app.selected_index += 1; }
                                else if app.current_page > 0 { app.current_page -= 1; app.needs_fetch = true; app.selected_index = 0; }
                            }
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => app.mode = AppMode::MainMenu { selected_idx: 0 },

                        KeyCode::Char('*') => net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Flagged"),
                        KeyCode::Char('d') | KeyCode::Char('D') => net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Deleted"),
                        KeyCode::Char('u') | KeyCode::Char('U') => net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Seen"),

                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            if let Some(status) = compose_email(&app.active_account, None, None, None, &mut theme_provider.current_theme) {
                                app.update_status(status);
                            }
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if !app.page_emails.is_empty() && session.expunge().is_ok() {
                                let offset = app.current_page * items_per_page + app.page_emails.len().saturating_sub(1).saturating_sub(app.selected_index) as u32;
                                if let Ok(m) = session.select(&app.current_folder) {
                                    app.total_messages = m.exists;
                                    let safe_offset = offset.min(app.total_messages.saturating_sub(1));
                                    app.current_page = safe_offset / items_per_page;
                                    app.restore_index_from_end = Some(safe_offset % items_per_page);
                                    app.needs_fetch = true;
                                }
                            }
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('r') | KeyCode::Char('R') => {
                            if !app.page_emails.is_empty() {
                                let (fetch_seq, from, date, subject, reply_to) = {
                                    let current = &app.page_emails[app.selected_index];
                                    (current.id.to_string(), current.from.clone(), current.date.clone(), current.subject.clone(), current.reply_to.clone())
                                };

                                let (t_body, _, _) = net::fetch_email_body(session, &fetch_seq);

                                if k.code == KeyCode::Char('f') || k.code == KeyCode::Char('F') {
                                    let sub = if subject.to_lowercase().starts_with("fwd:") { subject.clone() } else { format!("Fwd: {}", subject) };
                                    let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", from, date, subject, t_body);
                                    if let Some(s) = compose_email(&app.active_account, None, Some(&sub), Some(&fwd_body), &mut theme_provider.current_theme) {
                                        app.update_status(s);
                                    }
                                } else {
                                    let _ = session.store(&fetch_seq, "+FLAGS (\\Answered)");
                                    app.page_emails[app.selected_index].is_answered = true;

                                    let sub = if subject.to_lowercase().starts_with("re:") { subject.clone() } else { format!("Re: {}", subject) };
                                    if let Some(s) = compose_email(&app.active_account, Some(&reply_to), Some(&sub), None, &mut theme_provider.current_theme) {
                                        app.update_status(s);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('>') | KeyCode::Enter | KeyCode::Right => {
                            if !app.page_emails.is_empty() {
                                let fetch_seq = app.page_emails[app.selected_index].id.to_string();
                                let (t_body, h_body, atts) = net::fetch_email_body(session, &fetch_seq);

                                if !app.page_emails[app.selected_index].is_read {
                                    let _ = session.store(&fetch_seq, "+FLAGS (\\Seen)");
                                    app.page_emails[app.selected_index].is_read = true;
                                }

                                app.mode = AppMode::Reading { text_body: t_body, html_body: h_body, attachments: atts };
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => quit = true,
                        _ => {}
                    }
                }
                _ => {} // Reading mode uses editor loop, handled inside main
            }
        }
    } else if let Event::Resize(_, _) = event {
        app.needs_fetch = true;
    }

    quit
}
