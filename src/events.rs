use crate::app::{App, AppMode};
use crate::net::{self, ImapSession};
use crate::compose::compose_email;
use crate::editor::Editor;
use crate::config::ConfigExt;
use crate::ui::UiExt;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::size as term_size;
use crate::prompt::PromptExt;

pub fn handle_event(event: Event, app: &mut App, session: &mut ImapSession, theme_provider: &mut Editor, stdout: &mut std::io::Stdout) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {
            match &mut app.mode {
                AppMode::Settings { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(2),

                        KeyCode::Left | KeyCode::Char('<') | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('s') | KeyCode::Char('S') => app.mode = AppMode::MainMenu { selected_idx: 3 },

                        KeyCode::Char('x') | KeyCode::Char('X') | KeyCode::Right | KeyCode::Enter => {
                            if *selected_idx == 0 { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_config(); }
                            else if *selected_idx == 1 { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_config(); }
                            else if *selected_idx == 2 {
                                theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                                theme_provider.save_config();
                                app.needs_fetch = true;
                            }
                        }

                        KeyCode::Char('w') | KeyCode::Char('W') => { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_config(); }
                        KeyCode::Char('l') | KeyCode::Char('L') => { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_config(); }
                        KeyCode::Char('o') | KeyCode::Char('O') => {
                            theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                            theme_provider.save_config();
                            app.needs_fetch = true;
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') if k.modifiers.contains(KeyModifiers::ALT) => {
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();
                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                                theme_provider.save_config();
                            }
                        }
                        _ => {}
                    }
                }
                AppMode::AddressBook { selected_idx, addresses } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down  | KeyCode::Char('n') | KeyCode::Char('N') => if *selected_idx + 1 < addresses.len() { *selected_idx += 1; },
                        KeyCode::Char('<') | KeyCode::Left => app.mode = AppMode::MainMenu { selected_idx: 1 },
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if !addresses.is_empty() {
                                let prompt_msg = format!("Delete '{}'? (y/n)", addresses[*selected_idx]);
                                if let Ok(Some(true)) = theme_provider.prompt_yn(&prompt_msg) {
                                    addresses.remove(*selected_idx);
                                    let _ = crate::address::save_address_book(addresses);
                                    if *selected_idx >= addresses.len() {
                                        *selected_idx = addresses.len().saturating_sub(1);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::PageUp => {
                            if let Ok((_, rows)) = term_size() {
                                let visible = rows.saturating_sub(3) as usize; // matches ui.rs items_per_page
                                *selected_idx = selected_idx.saturating_sub(visible);
                            }
                        }
                        KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::PageDown => {
                            if let Ok((_, rows)) = term_size() {
                                let visible = rows.saturating_sub(3) as usize;
                                *selected_idx = (*selected_idx + visible).min(addresses.len().saturating_sub(1));
                            }
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            if k.modifiers.contains(KeyModifiers::ALT) {
                                let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                                themes.sort();

                                if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                    theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                                    theme_provider.save_config();
                                    theme_provider.set_status(format!("Theme: {}", theme_provider.current_theme));

                                    let _ = crate::ui::draw_app(stdout, app, theme_provider);
                                }
                            } else {
                                // Logic for creating teams (T only)
                                if let Ok(Some(team_name)) = theme_provider.prompt("Team Name (e.g. My Team): ", false) {
                                    let team_name = team_name.trim();
                                    if !team_name.is_empty() {
                                        if let Ok(Some(emails)) = theme_provider.prompt_with_autocomplete("Emails (comma separated): ", addresses) {
                                            let trimmed_emails = emails.trim().trim_end_matches(';');
                                            if !trimmed_emails.is_empty() {
                                                let formatted_list = format!("{}: {};", team_name, trimmed_emails);
                                                addresses.push(formatted_list);
                                                crate::address::clean_and_save_address_book(addresses);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            if let Ok(Some(new_val)) = theme_provider.prompt("Add address: ", false) {
                                let trimmed = new_val.trim();
                                if !trimmed.is_empty() && !addresses.iter().any(|a| a.trim() == trimmed) {
                                    addresses.push(trimmed.to_string());
                                    crate::address::clean_and_save_address_book(addresses);
                                }
                            }
                        }
                        KeyCode::Char('e') | KeyCode::Char('E') => {
                            if !addresses.is_empty() && !addresses[*selected_idx].trim().is_empty() {
                                let current_val = &addresses[*selected_idx];
                                // Use the new prompt_edit function, passing the current value
                                if let Ok(Some(new_val)) = theme_provider.prompt_edit("Edit: ", current_val) {
                                    if !new_val.trim().is_empty() {
                                        addresses[*selected_idx] = new_val.trim().to_string();
                                        crate::address::clean_and_save_address_book(addresses);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                AppMode::MainMenu { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(5),
                        KeyCode::Char('m') | KeyCode::Char('M') => app.mode = AppMode::EmailList,
                        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
                            match *selected_idx {
                                0 => {
                                    app.current_folder = "INBOX".to_string();
                                    app.current_page = 0;
                                    app.restore_index_from_end = Some(0);
                                    app.needs_fetch = true;
                                    app.mode = AppMode::EmailList;
                                }
                                1 => app.mode = AppMode::AddressBook { selected_idx: 0, addresses: crate::address::load_address_book() },
                                2 => app.mode = AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() },
                                3 => app.mode = AppMode::Settings { selected_idx: 0 },
                                4 => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::EmailList; },
                                5 => quit = true,
                                _ => {}
                            }
                        }
                        KeyCode::Char('i') | KeyCode::Char('I') => {
                            app.current_folder = "INBOX".to_string();
                            app.current_page = 0;
                            app.restore_index_from_end = Some(0);
                            app.needs_fetch = true;
                            app.mode = AppMode::EmailList;
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => app.mode = AppMode::AddressBook { selected_idx: 0, addresses: crate::address::load_address_book() },
                        KeyCode::Char('f') | KeyCode::Char('F') => app.mode = AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() },
                        KeyCode::Char('s') | KeyCode::Char('S') => app.mode = AppMode::Settings { selected_idx: 0 },
                        KeyCode::Char('h') | KeyCode::Char('H') => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::EmailList; },
                        KeyCode::Char('q') | KeyCode::Char('Q') => quit = true,
                        _ => {}
                    }
                }
                AppMode::FolderList { step, selected_idx, folders } => {
                    let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };

                    // Calculate items per page for the Y/V jumps
                    let (_, rows) = term_size().unwrap_or((80, 24));
                    let items_per_page = (rows.saturating_sub(3) as usize).max(1);

                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
                            *selected_idx = selected_idx.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                            *selected_idx = (*selected_idx + 1).min(items_count.saturating_sub(1));
                        }
                        KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            *selected_idx = selected_idx.saturating_sub(items_per_page);
                        }
                        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') => {
                            *selected_idx = (*selected_idx + items_per_page).min(items_count.saturating_sub(1));
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            app.mode = AppMode::MainMenu { selected_idx: 2 };
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('<') | KeyCode::Left => {
                            if *step == 1 { *step = 0; *selected_idx = app.current_account_idx; }
                            else { app.mode = AppMode::MainMenu { selected_idx: 2 }; }
                        }
                        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
                            if *step == 0 {
                                let new_idx = *selected_idx;
                                if new_idx != app.current_account_idx {
                                    app.current_account_idx = new_idx;
                                    app.active_account = app.accounts[app.current_account_idx].clone();
                                    let _ = session.logout();
                                    *session = net::connect(&app.active_account).unwrap();
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
                                app.current_page = 0;
                                app.restore_index_from_end = Some(0);
                                app.needs_fetch = true;
                                app.mode = AppMode::EmailList;
                            }
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') if k.modifiers.contains(KeyModifiers::ALT) => {
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();
                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                                theme_provider.save_config();
                            }
                        }
                        _ => {}
                    }
                }
                AppMode::EmailList => {
                    let (_, rows) = term_size().unwrap_or((80, 24));
                    let items_per_page = (rows.saturating_sub(3) as u32).max(1);
                    let total_pages = if app.total_messages == 0 { 1 } else { (app.total_messages + items_per_page - 1) / items_per_page };

                    match k.code {
                        KeyCode::Char('t') | KeyCode::Char('T') if k.modifiers.contains(KeyModifiers::ALT) => {
                            // 1. Get a stable list of themes
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();

                            // 2. Find the current position
                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                // 3. Update to the next theme
                                let next_theme = themes[(pos + 1) % themes.len()].clone();
                                theme_provider.current_theme = next_theme;

                                // 4. Update status and force an immediate redraw of the UI
                                app.update_status(format!("Theme: {}", theme_provider.current_theme));
                                let _ = crate::ui::draw_app(stdout, app, theme_provider);
                            }
                        }
                        KeyCode::Char('<') | KeyCode::Left => {
                            if app.search_query.is_some() {
                                // If we are currently searching, back out of the search to the full list
                                app.search_query = None;
                                app.current_page = 0;
                                app.needs_fetch = true;
                            } else {
                                // Otherwise, back out to the folder selection screen
                                let mut fetched = Vec::new();
                                if let Ok(mailboxes) = session.list(Some(""), Some("*")) {
                                    for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                                }
                                if fetched.is_empty() { fetched.push("INBOX".to_string()); }
                                fetched.sort();

                                // Auto-select the folder we just backed out of
                                let prev_folder = app.current_folder.clone();
                                let idx = fetched.iter().position(|f| f == &prev_folder).unwrap_or(0);

                                app.mode = AppMode::FolderList { step: 1, selected_idx: idx, folders: fetched };
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
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            // Get the current search query, or default to an empty string
                            let current_query = app.search_query.clone().unwrap_or_default();

                            // Pass current_query to prompt_edit
                            if let Ok(Some(query)) = theme_provider.prompt_edit("Search: ", &current_query) {
                                let trimmed = query.trim();

                                if !trimmed.is_empty() {
                                    app.search_query = Some(trimmed.to_string());
                                } else {
                                    // Leaving it blank (or clearing it out) returns to the full list
                                    app.search_query = None;
                                }

                                app.current_page = 0;
                                app.needs_fetch = true;
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
                            else {
                                if theme_provider.sort_newest_first {
                                    if app.current_page > 0 { app.current_page -= 1; app.needs_fetch = true; app.selected_index = (items_per_page - 1) as usize; }
                                } else {
                                    if app.current_page + 1 < total_pages { app.current_page += 1; app.needs_fetch = true; app.selected_index = (items_per_page - 1) as usize; }
                                }
                            }
                        }
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                            if !app.page_emails.is_empty() {
                                let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                                if app.selected_index + 1 < max_visible { app.selected_index += 1; }
                                else {
                                    if theme_provider.sort_newest_first {
                                        if app.current_page + 1 < total_pages { app.current_page += 1; app.needs_fetch = true; app.selected_index = 0; }
                                    } else {
                                        if app.current_page > 0 { app.current_page -= 1; app.needs_fetch = true; app.selected_index = 0; }
                                    }
                                }
                            }
                        }
                        KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('-') => {
                            if theme_provider.sort_newest_first {
                                if app.current_page > 0 {
                                    app.current_page -= 1;
                                    app.needs_fetch = true;
                                    app.selected_index = 0;
                                } else {
                                    app.selected_index = 0;
                                }
                            } else {
                                if app.current_page + 1 < total_pages {
                                    app.current_page += 1;
                                    app.needs_fetch = true;
                                    app.selected_index = 0;
                                } else {
                                    app.selected_index = 0;
                                }
                            }
                        }
                        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::Char(' ') => {
                            if theme_provider.sort_newest_first {
                                if app.current_page + 1 < total_pages {
                                    app.current_page += 1;
                                    app.needs_fetch = true;
                                    app.selected_index = 0;
                                } else {
                                    app.selected_index = app.page_emails.len().saturating_sub(1);
                                }
                            } else {
                                if app.current_page > 0 {
                                    app.current_page -= 1;
                                    app.needs_fetch = true;
                                    app.selected_index = 0;
                                } else {
                                    app.selected_index = app.page_emails.len().saturating_sub(1);
                                }
                            }
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => app.mode = AppMode::MainMenu { selected_idx: 0 },
                        KeyCode::Char('o') | KeyCode::Char('O') => {
                            app.menu_page = if app.menu_page == 1 { 2 } else { 1 };
                        }
                        KeyCode::Char('*') => net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Flagged"),
                        // KeyCode::Char('d') | KeyCode::Char('D') => net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Deleted"),
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Deleted");
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible {
                                app.selected_index += 1;
                            }
                        }
                        KeyCode::Char('u') | KeyCode::Char('U') => {
                            net::toggle_imap_flag(session, &mut app.page_emails, app.selected_index, "\\Seen");

                            // Move cursor down
                            let (_, rows) = term_size().unwrap_or((80, 24));
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible {
                                app.selected_index += 1;
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            if let Some(status) = compose_email(&app.active_account, None, None, None, &mut theme_provider.current_theme) {
                                app.update_status(status);
                            }
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if !app.page_emails.is_empty() {
                                // Query the IMAP server to see if any messages in the current folder are marked deleted
                                let has_deleted = session.search("DELETED").map(|res| !res.is_empty()).unwrap_or(false);

                                if !has_deleted {
                                    app.update_status("Nothing to expunge - no messages marked for deletion".to_string());
                                    // Override the default 1.5s duration to 3 seconds
                                    app.list_status_duration = std::time::Duration::from_secs(3);
                                } else {
                                    if let Ok(Some(true)) = theme_provider.prompt_yn("Expunge?") {
                                        if session.expunge().is_ok() {
                                            let offset = if theme_provider.sort_newest_first {
                                                app.current_page * items_per_page + app.selected_index as u32
                                            } else {
                                                app.current_page * items_per_page + app.page_emails.len().saturating_sub(1).saturating_sub(app.selected_index) as u32
                                            };

                                            if let Ok(m) = session.select(&app.current_folder) {
                                                app.total_messages = m.exists;
                                                let safe_offset = offset.min(app.total_messages.saturating_sub(1));
                                                app.current_page = safe_offset / items_per_page;
                                                app.restore_index_from_end = Some(safe_offset % items_per_page);
                                                app.needs_fetch = true;
                                            }
                                        }
                                    }
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

                                app.mode = AppMode::EmailRead { text_body: t_body, html_body: h_body, attachments: atts };
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => quit = true,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    } else if let Event::Resize(_, _) = event {
        app.needs_fetch = true;
    }

    quit
}

