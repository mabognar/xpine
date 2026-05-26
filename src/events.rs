use crate::app::{App, AppMode};
use crate::net::{self, ImapSession};
use crate::compose::compose_email;
use crate::editor::Editor;
use crate::config::ConfigExt;
use crate::ui::UiExt;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::size as term_size;
use crate::prompt::PromptExt;

pub fn handle_event(event: Event, app: &mut App, session: &mut Option<ImapSession>, theme_provider: &mut Editor, stdout: &mut std::io::Stdout) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {
            match &mut app.mode {

                AppMode::AddressBook { selected_idx, addresses } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down  | KeyCode::Char('n') | KeyCode::Char('N') => if *selected_idx + 1 < addresses.len() { *selected_idx += 1; },
                        KeyCode::Char('<') | KeyCode::Left => app.mode = AppMode::MainMenu { selected_idx: 1 },
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if !addresses.is_empty() {
                                let prompt_msg = format!("Delete '{}'?", addresses[*selected_idx]);
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
                                let visible = rows.saturating_sub(3) as usize;
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
                                    theme_provider.save_settings();
                                    theme_provider.set_status(format!("Theme: {}", theme_provider.current_theme));

                                    let _ = crate::ui::draw_app(stdout, app, theme_provider);
                                }
                            } else {
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

                AppMode::EmailAccounts { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(app.accounts.len().saturating_sub(1)),
                        KeyCode::Char('<') | KeyCode::Left | KeyCode::Char('q') | KeyCode::Esc => app.mode = AppMode::MainMenu { selected_idx: 4 },
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            if let Ok(Some(email)) = theme_provider.prompt("Email: ", false) {
                                if let Ok(Some(password)) = theme_provider.prompt("Password: ", false) {

                                    // Look up suggestions based on email domain
                                    let defaults = crate::config::get_provider_defaults(&email);

                                    let default_imap = defaults.as_ref().map(|d| d.imap).unwrap_or("imap.");
                                    if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", default_imap) {

                                        let default_port = defaults.as_ref().map(|d| d.port.to_string()).unwrap_or("993".to_string());
                                        if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &default_port) {

                                            let default_smtp = defaults.as_ref().map(|d| d.smtp).unwrap_or("smtp.");
                                            if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", default_smtp) {

                                                let new_acc = crate::config::Account {
                                                    email: email.trim().to_string(),
                                                    password: Some(password.trim().to_string()), // Wrapped in Some()
                                                    client_id: None,                             // Added OAuth fields
                                                    client_secret: None,
                                                    refresh_token: None,
                                                    imap_server: imap_server.trim().to_string(),
                                                    imap_port: imap_port.trim().parse().unwrap_or(993),
                                                    smtp_server: smtp_server.trim().to_string(),
                                                };

                                                app.accounts.push(new_acc);
                                                crate::config::save_config(&app.accounts);

                                                // --- Make the account active immediately ---
                                                app.current_account_idx = app.accounts.len() - 1;
                                                app.active_account = app.accounts[app.current_account_idx].clone();
                                                app.needs_reconnect = true;

                                                // Reset viewing state
                                                app.current_folder = "INBOX".to_string();
                                                app.current_page = 0;
                                                app.restore_index_from_end = Some(0);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            if !app.accounts.is_empty() {
                                let mut acc = app.accounts[*selected_idx].clone();

                                // Fallbacks in case the struct fields are empty
                                let client_id = acc.client_id.as_deref().unwrap_or("YOUR_MS_CLIENT_ID");
                                let client_secret = acc.client_secret.as_deref().unwrap_or("YOUR_MS_CLIENT_SECRET");

                                // 1. Suspend the alternate screen
                                let _ = crossterm::terminal::disable_raw_mode();
                                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

                                // --- ADD THESE DEBUG LINES ---
                                println!("\r\n===== DEBUG INFO =====");
                                println!("Account Email: {}", acc.email);
                                println!("Client ID being sent: '{}'", client_id);
                                println!("Client Secret being sent: '{}'", client_secret);
                                println!("======================\r\n");
                                // -----------------------------

                                // 2. Run the blocking auth flow and CAPTURE the result
                                let auth_result = crate::net::run_microsoft_auth_flow(client_id, client_secret);

                                // 3. Handle the result BEFORE restoring the screen
                                match auth_result {
                                    Ok(tokens) => {
                                        if let Some(refresh) = tokens.refresh_token {
                                            acc.refresh_token = Some(refresh);
                                            app.accounts[*selected_idx] = acc;
                                            crate::config::save_config(&app.accounts);

                                            app.update_status("MS Auth Successful. Token saved.".to_string());
                                        }
                                    },
                                    Err(e) => {
                                        // Print the error directly to the standard terminal so it's impossible to miss
                                        println!("\r\nAuthentication Failed!");
                                        println!("Error details: {}\r\n", e);
                                        println!("Press ENTER to return to xpine...");

                                        // Block and wait for the user to hit Enter
                                        let mut input = String::new();
                                        let _ = std::io::stdin().read_line(&mut input);

                                        app.update_status("MS Auth Failed.".to_string());
                                    }
                                }

                                // 4. NOW restore the UI state
                                let _ = crossterm::terminal::enable_raw_mode();
                                let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        );
                            }
                        },

                        KeyCode::Char('e') | KeyCode::Char('E') => {
                            if !app.accounts.is_empty() {
                                let acc = &app.accounts[*selected_idx].clone();
                                if let Ok(Some(email)) = theme_provider.prompt_edit("Email: ", &acc.email) {

                                    // Extract the password string safely, defaulting to empty if it's an OAuth account
                                    let current_pass = acc.password.clone().unwrap_or_default();

                                    if let Ok(Some(password)) = theme_provider.prompt_edit("Password: ", &current_pass) {
                                        if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", &acc.imap_server) {
                                            if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &acc.imap_port.to_string()) {
                                                if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", &acc.smtp_server) {
                                                    app.accounts[*selected_idx] = crate::config::Account {
                                                        email: email.trim().to_string(),
                                                        password: Some(password.trim().to_string()), // Wrapped in Some()

                                                        // Preserve any existing OAuth tokens so they aren't lost on edit
                                                        client_id: acc.client_id.clone(),
                                                        client_secret: acc.client_secret.clone(),
                                                        refresh_token: acc.refresh_token.clone(),

                                                        imap_server: imap_server.trim().to_string(),
                                                        imap_port: imap_port.trim().parse().unwrap_or(993),
                                                        smtp_server: smtp_server.trim().to_string(),
                                                    };
                                                    crate::config::save_config(&app.accounts);

                                                    if *selected_idx == app.current_account_idx {
                                                        app.needs_reconnect = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if !app.accounts.is_empty() {
                                let account_email = &app.accounts[*selected_idx].email;
                                let prompt_msg = format!("Are you sure you want to delete {}? (y/n) ", account_email);

                                if let Ok(Some(confirm)) = theme_provider.prompt(&prompt_msg, false) {
                                    if confirm.trim().to_lowercase() == "y" {
                                        app.accounts.remove(*selected_idx);
                                        crate::config::save_config(&app.accounts);

                                        if !app.accounts.is_empty() && *selected_idx >= app.accounts.len() {
                                            *selected_idx = app.accounts.len() - 1;
                                        }

                                        if *selected_idx == app.current_account_idx {
                                            app.needs_reconnect = true;
                                            app.current_account_idx = 0;
                                        } else if *selected_idx < app.current_account_idx {
                                            app.current_account_idx = app.current_account_idx.saturating_sub(1);
                                        }
                                    }
                                }
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
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();

                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                let next_theme = themes[(pos + 1) % themes.len()].clone();
                                theme_provider.current_theme = next_theme;

                                app.update_status(format!("Theme: {}", theme_provider.current_theme));
                                let _ = crate::ui::draw_app(stdout, app, theme_provider);
                                theme_provider.save_settings();
                            }
                        }
                        KeyCode::Char('<') | KeyCode::Left => {
                            if app.search_query.is_some() {
                                app.search_query = None;
                                app.current_page = 0;
                                app.needs_fetch = true;
                            } else {
                                let mut fetched = Vec::new();
                                if let Some(sess) = session {
                                    if let Ok(mailboxes) = sess.list(Some(""), Some("*")) {
                                        for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                                    }
                                }
                                if fetched.is_empty() { fetched.push("INBOX".to_string()); }
                                fetched.sort();

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
                            let current_query = app.search_query.clone().unwrap_or_default();

                            if let Ok(Some(query)) = theme_provider.prompt_edit("Search:", &current_query) {
                                let trimmed = query.trim();

                                if !trimmed.is_empty() {
                                    app.search_query = Some(trimmed.to_string());
                                } else {
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
                        KeyCode::Char('*') => {
                            if let Some(sess) = session {
                                net::toggle_imap_flag(sess, &mut app.page_emails, app.selected_index, "\\Flagged");
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if let Some(sess) = session {
                                net::toggle_imap_flag(sess, &mut app.page_emails, app.selected_index, "\\Deleted");
                            }
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible {
                                app.selected_index += 1;
                            }
                        }
                        KeyCode::Char('u') | KeyCode::Char('U') => {
                            if let Some(sess) = session {
                                net::toggle_imap_flag(sess, &mut app.page_emails, app.selected_index, "\\Seen");
                            }

                            let (_, rows) = term_size().unwrap_or((80, 24));
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible {
                                app.selected_index += 1;
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            if !app.accounts.is_empty() {
                                if let Some(status) = compose_email(&app.active_account, None, None, None, &mut theme_provider.current_theme) {
                                    app.update_status(status);
                                }
                            } else {
                                app.update_status("No account configured for sending.".to_string());
                            }
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if !app.page_emails.is_empty() {
                                if let Some(sess) = session {
                                    let has_deleted = sess.search("DELETED").map(|res| !res.is_empty()).unwrap_or(false);

                                    if !has_deleted {
                                        app.update_status("Nothing to expunge - no messages marked for deletion".to_string());
                                        app.list_status_duration = std::time::Duration::from_secs(3);
                                    } else {
                                        if let Ok(Some(true)) = theme_provider.prompt_yn("Expunge?") {
                                            if sess.expunge().is_ok() {
                                                let offset = if theme_provider.sort_newest_first {
                                                    app.current_page * items_per_page + app.selected_index as u32
                                                } else {
                                                    app.current_page * items_per_page + app.page_emails.len().saturating_sub(1).saturating_sub(app.selected_index) as u32
                                                };

                                                if let Ok(m) = sess.select(&app.current_folder) {
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
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('r') | KeyCode::Char('R') => {
                            if !app.page_emails.is_empty() {
                                if let Some(sess) = session {
                                    let (fetch_seq, from, date, subject, reply_to) = {
                                        let current = &app.page_emails[app.selected_index];
                                        (current.id.to_string(), current.from.clone(), current.date.clone(), current.subject.clone(), current.reply_to.clone())
                                    };

                                    let (t_body, _, _) = net::fetch_email_body(sess, &fetch_seq);

                                    if k.code == KeyCode::Char('f') || k.code == KeyCode::Char('F') {
                                        let sub = if subject.to_lowercase().starts_with("fwd:") { subject.clone() } else { format!("Fwd: {}", subject) };
                                        let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", from, date, subject, t_body);
                                        if let Some(s) = compose_email(&app.active_account, None, Some(&sub), Some(&fwd_body), &mut theme_provider.current_theme) {
                                            app.update_status(s);
                                        }
                                    } else {
                                        let _ = sess.store(&fetch_seq, "+FLAGS (\\Answered)");
                                        app.page_emails[app.selected_index].is_answered = true;

                                        let sub = if subject.to_lowercase().starts_with("re:") { subject.clone() } else { format!("Re: {}", subject) };
                                        if let Some(s) = compose_email(&app.active_account, Some(&reply_to), Some(&sub), None, &mut theme_provider.current_theme) {
                                            app.update_status(s);
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('>') | KeyCode::Enter | KeyCode::Right => {
                            if !app.page_emails.is_empty() {
                                if let Some(sess) = session {
                                    let fetch_seq = app.page_emails[app.selected_index].id.to_string();
                                    let (t_body, h_body, atts) = net::fetch_email_body(sess, &fetch_seq);

                                    if !app.page_emails[app.selected_index].is_read {
                                        let _ = sess.store(&fetch_seq, "+FLAGS (\\Seen)");
                                        app.page_emails[app.selected_index].is_read = true;
                                    }

                                    app.mode = AppMode::EmailRead { text_body: t_body, html_body: h_body, attachments: atts };
                                }
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => quit = true,
                        _ => {}
                    }
                }

                AppMode::FolderList { step, selected_idx, folders } => {
                    let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };

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

                                    if let Some(mut s) = session.take() {
                                        let _ = s.logout();
                                    }

                                    // CHANGE THIS LINE: Pass a mutable reference here
                                    *session = net::connect(&mut app.active_account).ok();
                                }
                                
                                let mut fetched = Vec::new();
                                if let Some(sess) = session {
                                    if let Ok(mailboxes) = sess.list(Some(""), Some("*")) {
                                        for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                                    }
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
                                theme_provider.save_settings();
                            }
                        }
                        _ => {}
                    }
                }

                AppMode::MainMenu { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(6),
                        KeyCode::Char('m') | KeyCode::Char('M') => app.mode = AppMode::EmailList,
                        KeyCode::Char('e') | KeyCode::Char('E') => app.mode = AppMode::EmailAccounts { selected_idx: 0 },
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
                                4 => app.mode = AppMode::EmailAccounts { selected_idx: 0 },
                                5 => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::EmailList; },
                                6 => quit = true,
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

                AppMode::Settings { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(2),

                        KeyCode::Left | KeyCode::Char('<') | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('s') | KeyCode::Char('S') => app.mode = AppMode::MainMenu { selected_idx: 3 },

                        KeyCode::Char('x') | KeyCode::Char('X') | KeyCode::Right | KeyCode::Enter => {
                            if *selected_idx == 0 { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_settings(); }
                            else if *selected_idx == 1 { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_settings(); }
                            else if *selected_idx == 2 {
                                theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                                theme_provider.save_settings();
                                app.needs_fetch = true;
                            }
                        }

                        KeyCode::Char('w') | KeyCode::Char('W') => { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_settings(); }
                        KeyCode::Char('l') | KeyCode::Char('L') => { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_settings(); }
                        KeyCode::Char('o') | KeyCode::Char('O') => {
                            theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                            theme_provider.save_settings();
                            app.needs_fetch = true;
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') if k.modifiers.contains(KeyModifiers::ALT) => {
                            let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                            themes.sort();
                            if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                                theme_provider.save_settings();
                            }
                        }
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