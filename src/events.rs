use crate::app::{App, AppMode};
use crate::net::{self, MailSession};
use crate::compose::compose_email;
use crate::editor::Editor;
use crate::config::ConfigExt;
use crate::ui::UiExt;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::size as term_size;
use crate::prompt::PromptExt;

// Restored from earlier today to handle Outlook/Hotmail auto-expunging
fn check_and_expunge_outlook(app: &mut App, session: &mut Option<net::MailSession>, theme_provider: &mut Editor) {
    let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook") ||
        app.active_account.email.to_lowercase().contains("outlook") ||
        app.active_account.email.to_lowercase().contains("hotmail");

    if !is_outlook {
        return;
    }

    let has_pending = app.page_emails.iter().any(|e| e.is_deleted);
    if !has_pending {
        return;
    }

    if let Ok(Some(yes)) = theme_provider.prompt_yn("Expunge emails marked for deletion?") {
        if yes {
            if let Some(sess) = session {
                match sess {
                    net::MailSession::Imap(imap_sess) => {
                        for email in &app.page_emails {
                            if email.is_deleted {
                                let _ = imap_sess.uid_store(&email.uid.to_string(), "+FLAGS (\\Deleted)");
                            }
                        }
                    }
                    net::MailSession::Graph { .. } => {}
                }

                let _ = net::expunge_deleted(sess, app);
                app.needs_fetch = true;
            }
        } else {
            for email in &mut app.page_emails {
                email.is_deleted = false;
            }
        }
    }
}

pub fn handle_event(event: Event, app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor, stdout: &mut std::io::Stdout) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {

            let mut pending_status = None;

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

                AppMode::Compose { to, cc, bcc, subject, attachments, active_idx, editor } => {
                    // 1. Global Shortcuts (Control)
                }

                AppMode::EmailAccounts { selected_idx } => {
                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => *selected_idx = selected_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => *selected_idx = (*selected_idx + 1).min(app.accounts.len().saturating_sub(1)),
                        KeyCode::Char('<') | KeyCode::Left | KeyCode::Esc => app.mode = AppMode::MainMenu { selected_idx: 4 },
                        KeyCode::Char('c') | KeyCode::Char('C') if k.modifiers.contains(KeyModifiers::CONTROL) => app.mode = AppMode::MainMenu { selected_idx: 4 },
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            if let Ok(Some(email)) = theme_provider.prompt("Email: ", false) {
                                let email_lower = email.trim().to_lowercase();

                                // Auto-detect standard Microsoft domains
                                let mut is_microsoft = email_lower.ends_with("@outlook.com")
                                    || email_lower.ends_with("@hotmail.com")
                                    || email_lower.ends_with("@live.com")
                                    || email_lower.ends_with("@msn.com");

                                // If it isn't a standard domain, ask the user (handles custom/enterprise MS accounts)
                                if !is_microsoft {
                                    if let Ok(Some(yes)) = theme_provider.prompt_yn("Is this a Microsoft / Graph API account?") {
                                        is_microsoft = yes;
                                    }
                                }

                                if is_microsoft {
                                    // Hardcode your application's Client ID
                                    let client_id = "014bd274-beed-47dd-afba-c2fc4f48ede0".to_string();

                                    // 1. Create the Microsoft Account with the hardcoded ID and no secret
                                    let mut new_acc = crate::config::Account {
                                        email: email.trim().to_string(),
                                        password: None,
                                        client_id: Some(client_id.clone()),
                                        client_secret: Some("dummy_secret_do_not_remove".to_string()),
                                        refresh_token: None,
                                        imap_server: String::new(),
                                        imap_port: 0,
                                        smtp_server: String::new(),
                                        smtp_port: 0,
                                    };

                                    let _ = crossterm::terminal::disable_raw_mode();
                                    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

                                    println!("Account Email: {}", new_acc.email);
                                    println!("Client ID being sent: '{}'", client_id);

                                    // 2. Trigger the "M" screen authentication flow
                                    // Passing an empty string for the secret since it is not needed
                                    match crate::net::run_microsoft_auth_flow(&client_id, "") {
                                        Ok(tokens) => {
                                            if let Some(refresh) = tokens.refresh_token {
                                                new_acc.refresh_token = Some(refresh);
                                                pending_status = Some("MS Auth Successful. Account added.".to_string());
                                            }
                                        },
                                        Err(e) => {
                                            println!("\r\nAuthentication Failed!");
                                            println!("Error details: {}\r\n", e);
                                            println!("Press ENTER to return to xpine...");
                                            let mut input = String::new();
                                            let _ = std::io::stdin().read_line(&mut input);
                                            pending_status = Some("MS Auth Failed. Account added without token.".to_string());
                                        }
                                    }

                                    let _ = crossterm::terminal::enable_raw_mode();
                                    let _ = crossterm::execute!(
                                        std::io::stdout(),
                                        crossterm::terminal::EnterAlternateScreen,
                                        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                                    );

                                    // 3. Save the account and set it as active
                                    app.accounts.push(new_acc);
                                    crate::config::save_config(&app.accounts);

                                    app.current_account_idx = app.accounts.len() - 1;
                                    app.active_account = app.accounts[app.current_account_idx].clone();
                                    app.needs_reconnect = true;

                                    app.current_folder = "INBOX".to_string();
                                    app.current_page = 0;
                                    app.restore_index_from_end = Some(0);
                                    *selected_idx = app.current_account_idx;
                                } else {
                                    // 4. STANDARD IMAP FLOW (Google, Custom, etc.)
                                    if let Ok(Some(password)) = theme_provider.prompt("Password: ", false) {
                                        let defaults = crate::config::get_provider_defaults(&email);
                                        let default_imap = defaults.as_ref().map(|d| d.imap).unwrap_or("imap.");

                                        if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", default_imap) {
                                            let default_port = defaults.as_ref().map(|d| d.port.to_string()).unwrap_or("993".to_string());

                                            if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &default_port) {
                                                let default_smtp = defaults.as_ref().map(|d| d.smtp).unwrap_or("smtp.");

                                                if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", default_smtp) {
                                                    let new_acc = crate::config::Account {
                                                        email: email.trim().to_string(),
                                                        password: Some(password.trim().to_string()),
                                                        client_id: None,
                                                        client_secret: None,
                                                        refresh_token: None,
                                                        imap_server: imap_server.trim().to_string(),
                                                        imap_port: imap_port.trim().parse().unwrap_or(993),
                                                        smtp_server: smtp_server.trim().to_string(),
                                                        smtp_port: 587,
                                                    };

                                                    app.accounts.push(new_acc);
                                                    crate::config::save_config(&app.accounts);

                                                    app.current_account_idx = app.accounts.len() - 1;
                                                    app.active_account = app.accounts[app.current_account_idx].clone();
                                                    app.needs_reconnect = true;

                                                    app.current_folder = "INBOX".to_string();
                                                    app.current_page = 0;
                                                    app.restore_index_from_end = Some(0);
                                                    *selected_idx = app.current_account_idx;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            if !app.accounts.is_empty() {
                                let mut acc = app.accounts[*selected_idx].clone();

                                let client_id = acc.client_id.as_deref().unwrap_or("YOUR_MS_CLIENT_ID");
                                let client_secret = acc.client_secret.as_deref().unwrap_or("YOUR_MS_CLIENT_SECRET");

                                let _ = crossterm::terminal::disable_raw_mode();
                                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

                                println!("Account Email: {}", acc.email);
                                println!("Client ID being sent: '{}'", client_id);
                                println!("Client Secret being sent: '{}'", client_secret);

                                let auth_result = crate::net::run_microsoft_auth_flow(client_id, client_secret);

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
                                        println!("\r\nAuthentication Failed!");
                                        println!("Error details: {}\r\n", e);
                                        println!("Press ENTER to return to xpine...");

                                        let mut input = String::new();
                                        let _ = std::io::stdin().read_line(&mut input);

                                        app.update_status("MS Auth Failed.".to_string());
                                    }
                                }

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
                                    let current_pass = acc.password.clone().unwrap_or_default();

                                    if let Ok(Some(password)) = theme_provider.prompt_edit("Password: ", &current_pass) {
                                        if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", &acc.imap_server) {
                                            if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &acc.imap_port.to_string()) {
                                                if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", &acc.smtp_server) {
                                                    app.accounts[*selected_idx] = crate::config::Account {
                                                        email: email.trim().to_string(),
                                                        password: Some(password.trim().to_string()),
                                                        client_id: acc.client_id.clone(),
                                                        client_secret: acc.client_secret.clone(),
                                                        refresh_token: acc.refresh_token.clone(),
                                                        imap_server: imap_server.trim().to_string(),
                                                        imap_port: imap_port.trim().parse().unwrap_or(993),
                                                        smtp_server: smtp_server.trim().to_string(),
                                                        smtp_port: imap_port.trim().parse().unwrap_or(587),
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
                                    match sess {
                                        net::MailSession::Imap(imap_sess) => {
                                            if let Ok(mailboxes) = imap_sess.list(Some(""), Some("*")) {
                                                for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                                            }
                                        }
                                        net::MailSession::Graph { access_token } => {
                                            let client = reqwest::blocking::Client::new();
                                            let url = "https://graph.microsoft.com/v1.0/me/mailFolders?includeHiddenFolders=true&$top=100";                                            if let Ok(res) = client.get(url)
                                                .header("Authorization", format!("Bearer {}", access_token))
                                                .send() {
                                                if let Ok(graph_data) = res.json::<crate::net::GraphFolderResponse>() {
                                                    for folder in graph_data.value {
                                                        fetched.push(folder.display_name);
                                                    }
                                                }
                                            }
                                        }
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
                            check_and_expunge_outlook(app, session, theme_provider);
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

                            let _ = execute!(std::io::stdout(), crossterm::cursor::Show);
                        }
                        KeyCode::Char('j') if k.modifiers.contains(KeyModifiers::ALT) => {
                            if !app.page_emails.is_empty() {
                                if let Ok(Some(true)) = theme_provider.prompt_yn("Move to Junk/Spam folder?") {
                                    if let Some(sess) = session {
                                        let seq_id = app.page_emails[app.selected_index].id.to_string();
                                        let junk_folder = if app.active_account.email.contains("@gmail.com") { "[Gmail]/Spam" } else { "Junk" };

                                        match net::move_to_folder(sess, &seq_id, junk_folder) {
                                            Ok(_) => { app.update_status(format!("Moved to {}.", junk_folder)); app.needs_fetch = true; },
                                            Err(e) => app.update_status(format!("Error: {}", e)),
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('i') if k.modifiers.contains(KeyModifiers::ALT) => {
                            if !app.page_emails.is_empty() {
                                if let Ok(Some(true)) = theme_provider.prompt_yn("Move to Inbox?") {
                                    if let Some(sess) = session {
                                        let seq_id = app.page_emails[app.selected_index].id.to_string();

                                        match net::move_to_folder(sess, &seq_id, "INBOX") {
                                            Ok(_) => { app.update_status("Moved to INBOX.".to_string()); app.needs_fetch = true; },
                                            Err(e) => app.update_status(format!("Error: {}", e)),
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(digit) = c.to_digit(10) {
                                let idx = (digit as usize).saturating_sub(1);
                                if idx < app.accounts.len() && idx != app.current_account_idx {
                                    check_and_expunge_outlook(app, session, theme_provider);
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
                                if app.current_page > 0 { app.current_page -= 1; app.needs_fetch = true; app.selected_index = 0; }
                                else { app.selected_index = 0; }
                            } else {
                                if app.current_page + 1 < total_pages { app.current_page += 1; app.needs_fetch = true; app.selected_index = 0; }
                                else { app.selected_index = 0; }
                            }
                        }
                        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::Char(' ') => {
                            if theme_provider.sort_newest_first {
                                if app.current_page + 1 < total_pages { app.current_page += 1; app.needs_fetch = true; app.selected_index = 0; }
                                else { app.selected_index = app.page_emails.len().saturating_sub(1); }
                            } else {
                                if app.current_page > 0 { app.current_page -= 1; app.needs_fetch = true; app.selected_index = 0; }
                                else { app.selected_index = app.page_emails.len().saturating_sub(1); }
                            }
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            check_and_expunge_outlook(app, session, theme_provider);
                            app.mode = AppMode::MainMenu { selected_idx: 0 };
                        },
                        KeyCode::Char('o') | KeyCode::Char('O') => { app.menu_page = if app.menu_page == 1 { 2 } else { 1 }; }
                        KeyCode::Char('*') => {
                            if let Some(sess) = session { net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Flagged"); }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook") ||
                                app.active_account.email.to_lowercase().contains("outlook") ||
                                app.active_account.email.to_lowercase().contains("hotmail");

                            if is_outlook {
                                if !app.page_emails.is_empty() {
                                    let idx = app.selected_index;
                                    app.page_emails[idx].is_deleted = !app.page_emails[idx].is_deleted;
                                }
                            } else {
                                if let Some(sess) = session {
                                    if !app.page_emails.is_empty() {
                                        net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Deleted");
                                    }
                                }
                            }

                            let (_, rows) = term_size().unwrap_or((80, 24));
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible { app.selected_index += 1; }
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if !app.page_emails.is_empty() {
                                if let Some(sess) = session {
                                    let has_deleted = app.page_emails.iter().any(|e| e.is_deleted);

                                    if !has_deleted {
                                        app.update_status("Nothing to expunge - no messages marked for deletion".to_string());
                                        app.list_status_duration = std::time::Duration::from_secs(3);
                                    } else {
                                        let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook") ||
                                            app.active_account.email.to_lowercase().contains("outlook") ||
                                            app.active_account.email.to_lowercase().contains("hotmail");

                                        if is_outlook {
                                            match sess {
                                                net::MailSession::Imap(imap_sess) => {
                                                    for email in &app.page_emails {
                                                        if email.is_deleted {
                                                            let _ = imap_sess.uid_store(&email.uid.to_string(), "+FLAGS (\\Deleted)");
                                                        }
                                                    }
                                                }
                                                net::MailSession::Graph { .. } => {}
                                            }
                                        }

                                        if let Ok(Some(true)) = theme_provider.prompt_yn("Expunge?") {
                                            if net::expunge_deleted(sess, app).is_ok() {
                                                let offset = if theme_provider.sort_newest_first {
                                                    app.current_page * items_per_page + app.selected_index as u32
                                                } else {
                                                    app.current_page * items_per_page + app.page_emails.len().saturating_sub(1).saturating_sub(app.selected_index) as u32
                                                };

                                                match sess {
                                                    net::MailSession::Imap(imap_sess) => {
                                                        if let Ok(m) = imap_sess.select(&app.current_folder) {
                                                            app.total_messages = m.exists;
                                                            let safe_offset = offset.min(app.total_messages.saturating_sub(1));
                                                            app.current_page = safe_offset / items_per_page;
                                                            app.restore_index_from_end = Some(safe_offset % items_per_page);
                                                        }
                                                    }
                                                    net::MailSession::Graph { .. } => {}
                                                }
                                            } else {
                                                app.update_status("Expunge failed.".to_string());
                                                app.list_status_duration = std::time::Duration::from_secs(3);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('u') | KeyCode::Char('U') => {
                            if let Some(sess) = session {
                                net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Seen");
                            }
                            let (_, rows) = term_size().unwrap_or((80, 24));
                            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
                            if app.selected_index + 1 < max_visible { app.selected_index += 1; }
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
                        KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('r') | KeyCode::Char('R') => {
                            if !app.page_emails.is_empty() {
                                if let Some(sess) = session {
                                    let (fetch_seq, from, date, subject, reply_to) = {
                                        let current = &app.page_emails[app.selected_index];
                                        (current.id.to_string(), current.from.clone(), current.date.clone(), current.subject.clone(), current.reply_to.clone())
                                    };

                                    let (t_body, _, _) = net::fetch_email_body(sess, &fetch_seq);

                                    // Forwarding Logic
                                    if k.code == KeyCode::Char('f') || k.code == KeyCode::Char('F') {
                                        let sub = if subject.to_lowercase().starts_with("fwd:") { subject.clone() } else { format!("Fwd: {}", subject) };
                                        let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", from, date, subject, t_body);
                                        if let Some(s) = compose_email(&app.active_account, None, Some(&sub), Some(&fwd_body), &mut theme_provider.current_theme) {
                                            app.update_status(s);
                                        }
                                    }
                                    // Replying Logic
                                    else {
                                        let raw_reply = if reply_to.trim().is_empty() {
                                            crate::mail::extract_email(&from)
                                        } else {
                                            crate::mail::extract_email(&reply_to)
                                        };

                                        let sub = if subject.to_lowercase().starts_with("re:") { subject.clone() } else { format!("Re: {}", subject) };
                                        let reply_body = crate::mail::format_reply_text(&t_body);

                                        // --- 1. Compose First ---
                                        if let Some(s) = compose_email(&app.active_account, Some(&raw_reply), Some(&sub), Some(&reply_body), &mut theme_provider.current_theme) {
                                            match sess {
                                                net::MailSession::Imap(imap_sess) => {
                                                    let _ = imap_sess.store(&fetch_seq, "+FLAGS (\\Answered)");
                                                }
                                                net::MailSession::Graph { access_token } => {
                                                    let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", fetch_seq);
                                                    let client = reqwest::blocking::Client::new();

                                                    // Ensure the payload is exactly what the Graph API expects
                                                    let payload = serde_json::json!({
                                                    "singleValueExtendedProperties": [
                                                        { "id": "Integer 0x1081", "value": "102" }, // ReplyToSender
                                                        { "id": "Integer 0x1080", "value": "261" }  // Replied icon
                                                    ]
                                                    });

                                                    // CRITICAL: Check the response of the PATCH request
                                                    let res = client.patch(&url)
                                                        .header("Authorization", format!("Bearer {}", access_token))
                                                        .header("Content-Type", "application/json")
                                                        .json(&payload)
                                                        .send();

                                                    if let Ok(response) = res {
                                                        if response.status().is_success() {
                                                            // Success: proceed to mark locally
                                                            app.page_emails[app.selected_index].is_answered = true;
                                                        } else {
                                                            // Debugging: If this prints an error, the PATCH failed
                                                            app.update_status(format!("Failed to mark 'A' on server: {}", response.status()));
                                                        }
                                                    }
                                                }
                                            }

                                            app.page_emails[app.selected_index].is_answered = true;
                                            app.needs_fetch = true;
                                            // app.update_status(s);
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
                                        match sess {
                                            net::MailSession::Imap(imap_sess) => {
                                                let _ = imap_sess.store(&fetch_seq, "+FLAGS (\\Seen)");
                                            }
                                            net::MailSession::Graph { .. } => {
                                                if let Some(sess) = session {
                                                    net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Seen");
                                                }
                                            }
                                        }
                                        app.page_emails[app.selected_index].is_read = true;
                                    }

                                    app.mode = AppMode::EmailRead { text_body: t_body, html_body: h_body, attachments: atts };
                                }
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => {
                            check_and_expunge_outlook(app, session, theme_provider);
                            quit = true;
                        },
                        _ => {}
                    }
                }

                AppMode::FolderList { step, selected_idx, folders } => {
                    let items_count = if *step == 0 { app.accounts.len() } else { folders.len() };
                    let (_, rows) = term_size().unwrap_or((80, 24));
                    let items_per_page = (rows.saturating_sub(3) as usize).max(1);

                    match k.code {
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => { *selected_idx = selected_idx.saturating_sub(1); }
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => { *selected_idx = (*selected_idx + 1).min(items_count.saturating_sub(1)); }
                        KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') => { *selected_idx = selected_idx.saturating_sub(items_per_page); }
                        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') => { *selected_idx = (*selected_idx + items_per_page).min(items_count.saturating_sub(1)); }
                        KeyCode::Char('m') | KeyCode::Char('M') => { app.mode = AppMode::MainMenu { selected_idx: 2 }; }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('<') | KeyCode::Left => {
                            if *step == 1 { *step = 0; *selected_idx = app.current_account_idx; }
                            else { app.mode = AppMode::MainMenu { selected_idx: 2 }; }
                        }
                        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
                            if *step == 0 {
                                if *selected_idx < app.accounts.len() {
                                    app.active_account = app.accounts[*selected_idx].clone();
                                    app.current_account_idx = *selected_idx;
                                    *session = crate::net::connect(&mut app.active_account).ok();

                                    let mut fetched = Vec::new();
                                    // Removed 'ref mut' here to properly utilize match ergonomics
                                    if let Some(sess) = session {
                                        match sess {
                                            net::MailSession::Imap(imap_sess) => {
                                                if let Ok(mailboxes) = imap_sess.list(Some(""), Some("*")) {
                                                    for mb in mailboxes.iter() {
                                                        fetched.push(mb.name().to_string());
                                                    }
                                                }
                                            }
                                            net::MailSession::Graph { access_token } => {
                                                let client = reqwest::blocking::Client::new();
                                                let url = "https://graph.microsoft.com/v1.0/me/mailFolders?includeHiddenFolders=true&$top=100";
                                                if let Ok(res) = client.get(url)
                                                    .header("Authorization", format!("Bearer {}", access_token))
                                                    .send() {
                                                    if let Ok(graph_data) = res.json::<crate::net::GraphFolderResponse>() {
                                                        for folder in graph_data.value {
                                                            fetched.push(folder.display_name);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if fetched.is_empty() {
                                        fetched.push("INBOX".to_string());
                                    }

                                    *folders = fetched;
                                    *step = 1;
                                    *selected_idx = 0;
                                }
                            } else if *step == 1 {
                                if !folders.is_empty() {
                                    app.current_folder = folders[*selected_idx].clone();
                                    app.current_page = 0;
                                    app.restore_index_from_end = Some(0);
                                    app.needs_fetch = true;
                                    app.mode = AppMode::EmailList;
                                }
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
                        // Inside AppMode::FolderList (or your specific Folders mode) in src/events.rs:

                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            if let Ok(Some(folder_name)) = theme_provider.prompt("New Folder Name: ", false) {
                                let clean_name = folder_name.trim();

                                if !clean_name.is_empty() {
                                    if let Some(sess) = session {
                                        match crate::net::create_folder(sess, clean_name) {
                                            Ok(_) => {
                                                // DEFER THE STATUS
                                                pending_status = Some(format!("Created folder: {}", clean_name));

                                                if let Ok(new_folders) = crate::net::list_folders(sess) {
                                                    *folders = new_folders;
                                                }
                                            }
                                            Err(e) => {
                                                // DEFER THE STATUS
                                                pending_status = Some(e);
                                            }
                                        }
                                    } else {
                                        app.update_status("Offline: Cannot create folder".to_string());
                                    }
                                }
                            }
                        }

                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            let folder_name = folders[*selected_idx].clone();
                            let lower_name = folder_name.to_lowercase();

                            let is_system = matches!(
                                lower_name.as_str(),
                                "inbox" | "sent" | "sent items" | "drafts" | "trash" | "deleted items" |
                                "spam" | "junk" | "archive" | "[gmail]" | "[gmail]/all mail" | "[gmail]/sent mail" | "[gmail]/drafts" |
                                "[gmail]/trash" | "[gmail]/spam" | "[gmail]/important" | "[gmail]/starred"
                            );

                            if is_system {
                                pending_status = Some(format!("Cannot delete system folder: {}", folder_name));
                            } else {
                                // STEP 1: First Confirmation
                                if let Ok(Some(true)) = theme_provider.prompt_yn(&format!("Really delete folder '{}'? (y/n): ", folder_name)) {

                                    // STEP 2: The "Absolutely Sure" Confirmation
                                    let absolute_msg = format!("Are you absolutely sure? All emails in '{}' will be lost. (y/n): ", folder_name);

                                    if let Ok(Some(true)) = theme_provider.prompt_yn(&absolute_msg) {
                                        // Both prompts answered Yes, proceed with deletion
                                        if let Some(sess) = session {
                                            match crate::net::delete_folder(sess, &folder_name) {
                                                Ok(_) => {
                                                    pending_status = Some(format!("Deleted folder: {}", folder_name));

                                                    if let Ok(new_folders) = crate::net::list_folders(sess) {
                                                        *folders = new_folders;

                                                        if *selected_idx >= folders.len() {
                                                            *selected_idx = folders.len().saturating_sub(1);
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    pending_status = Some(e);
                                                }
                                            }
                                        } else {
                                            pending_status = Some("Offline: Cannot delete folder".to_string());
                                        }
                                    } else {
                                        // User backed out at the second prompt
                                        pending_status = Some("Folder deletion cancelled.".to_string());
                                    }
                                } else {
                                    // User backed out at the first prompt
                                    pending_status = Some("Folder deletion cancelled.".to_string());
                                }
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            let folder_name = folders[*selected_idx].clone();
                            let lower_name = folder_name.to_lowercase();

                            let is_system = matches!(
                                lower_name.as_str(),
                                "inbox" | "sent" | "sent items" | "drafts" | "trash" | "deleted items" |
                                "spam" | "junk" | "archive" | "[gmail]/sent mail" | "[gmail]/drafts" |
                                "[gmail]/trash" | "[gmail]/spam" | "[gmail]/important" | "[gmail]/starred"
                            );

                            if is_system {
                                pending_status = Some(format!("Cannot rename system folder: {}", folder_name));
                            } else {
                                // Prompt for the new name, putting the old name in the prompt as a hint
                                let prompt_str = format!("Rename '{}' to: ", folder_name);
                                if let Ok(Some(new_name_input)) = theme_provider.prompt(&prompt_str, false) {
                                    let clean_new_name = new_name_input.trim();

                                    if !clean_new_name.is_empty() && clean_new_name != folder_name {
                                        if let Some(sess) = session {
                                            match crate::net::rename_folder(sess, &folder_name, clean_new_name) {
                                                Ok(_) => {
                                                    pending_status = Some(format!("Renamed to: {}", clean_new_name));

                                                    // Fetch the fresh, newly-sorted list
                                                    if let Ok(new_folders) = crate::net::list_folders(sess) {
                                                        *folders = new_folders.clone();

                                                        // UX Polish: Find where the renamed folder landed after sorting
                                                        // so the user's cursor follows the folder to its new position!
                                                        if let Some(new_pos) = new_folders.iter().position(|f| f == clean_new_name) {
                                                            *selected_idx = new_pos;
                                                        } else if *selected_idx >= folders.len() {
                                                            *selected_idx = folders.len().saturating_sub(1);
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    pending_status = Some(e);
                                                }
                                            }
                                        } else {
                                            pending_status = Some("Offline: Cannot rename folder".to_string());
                                        }
                                    } else if clean_new_name.is_empty() {
                                        pending_status = Some("Rename cancelled: Name cannot be empty.".to_string());
                                    }
                                } else {
                                    pending_status = Some("Rename cancelled.".to_string());
                                }
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
                                4 => {
                                    check_and_expunge_outlook(app, session, theme_provider);
                                    app.mode = AppMode::EmailAccounts { selected_idx: 0 };
                                },
                                5 => { app.update_status("Help not yet implemented.".to_string()); app.mode = AppMode::EmailList; },
                                6 => {
                                    check_and_expunge_outlook(app, session, theme_provider);
                                    quit = true;
                                },
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
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            check_and_expunge_outlook(app, session, theme_provider);
                            quit = true;
                        },
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
            // THIS IS REQUIRED TO ACTUALLY UPDATE THE APP
            if let Some(status) = pending_status {
                app.list_status = status;
                app.list_status_time = Some(std::time::Instant::now());

                // Or if you use a helper method:
                // app.update_status(status);
            }
            // if let Some(status) = pending_status {
            //     app.update_status(status);
            // }
        }
    } else if let Event::Resize(_, _) = event {
        app.needs_fetch = true;
    }

    quit
}

