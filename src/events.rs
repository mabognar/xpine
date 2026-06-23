use crate::app::{App, AppMode};
use crate::net::{self, MailSession};
use crate::compose::compose_email;
use crate::editor::Editor;
use crate::config::ConfigExt;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, KeyEvent};
use crossterm::execute;
use crossterm::terminal::size as term_size;
use crate::prompt::PromptExt;
use crate::ui::UiExt;
use crate::browser::BrowserExt;

fn check_and_expunge_outlook(app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor) {
    let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook") ||
        app.active_account.email.to_lowercase().contains("outlook") ||
        app.active_account.email.to_lowercase().contains("hotmail");

    if !is_outlook { return; }

    // Check if there are deletes on this page OR stored in the Graph tracking set
    let has_pending = app.page_emails.iter().any(|e| e.is_deleted) || !app.graph_pending_deleted.is_empty();
    if !has_pending { return; }

    if let Ok(Some(yes)) = theme_provider.prompt_yn("Expunge emails marked for deletion?") {
        if yes {
            if let Some(sess) = session {
                match sess {
                    MailSession::Imap(imap_sess) => {
                        for email in &app.page_emails {
                            if email.is_deleted {
                                let _ = imap_sess.uid_store(&email.uid.to_string(), "+FLAGS (\\Deleted)");
                            }
                        }
                    }
                    MailSession::Graph { .. } => {}
                }

                let _ = net::expunge_deleted(sess, app);
                app.needs_fetch = true;
            }
        } else {
            for email in &mut app.page_emails {
                email.is_deleted = false;
            }
            app.graph_pending_deleted.clear(); // Clear Graph tracking if cancelled
        }
    }
}

pub fn handle_event(event: Event, app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor, stdout: &mut std::io::Stdout) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {
            match &app.mode {
                AppMode::AddressBook { .. } => handle_address_book_events(k, app, theme_provider, stdout),
                AppMode::EmailAccounts { .. } => handle_email_accounts_events(k, app, theme_provider, stdout),
                AppMode::EmailList => handle_email_list_events(k, app, session, theme_provider, stdout, &mut quit),
                // AppMode::FolderList { .. } => handle_folder_list_events(k, app, session, theme_provider, stdout),
                AppMode::FolderList { .. } => handle_folder_list_events(k, app, session, theme_provider, stdout, &mut quit),
                AppMode::MainMenu { .. } => handle_main_menu_events(k, app, session, theme_provider, &mut quit),
                AppMode::Settings { .. } => handle_settings_events(k, app, theme_provider),
                AppMode::EmailRead { .. } => {} // Handled completely in src/read.rs
            }
        }
    } else if let Event::Resize(_, _) = event {
        app.needs_fetch = true;
    }

    quit
}

fn handle_address_book_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor, _stdout: &mut std::io::Stdout) {
    let (mut selected_idx, mut addresses) = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::AddressBook { selected_idx, addresses } => (selected_idx, addresses),
        other => { app.mode = other; return; }
    };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down  | KeyCode::Char('n') | KeyCode::Char('N') => if selected_idx + 1 < addresses.len() { selected_idx += 1; },
        KeyCode::Char('<') | KeyCode::Left | KeyCode::Esc => next_mode = Some(AppMode::MainMenu { selected_idx: 1 }),
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if !addresses.is_empty() {
                let prompt_msg = format!("Delete '{}'?", addresses[selected_idx]);
                if let Ok(Some(true)) = theme_provider.prompt_yn(&prompt_msg) {
                    if let Ok(Some(true)) = theme_provider.prompt_yn("Are you sure?") {
                        addresses.remove(selected_idx);
                        let _ = crate::address::save_address_book(&addresses);

                        if selected_idx >= addresses.len() {
                            selected_idx = addresses.len().saturating_sub(1);
                        }
                    }
                }
            }
        }
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::PageUp => {
            if let Ok((_, rows)) = term_size() {
                let visible = rows.saturating_sub(3) as usize;
                selected_idx = selected_idx.saturating_sub(visible);
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::PageDown => {
            if let Ok((_, rows)) = term_size() {
                let visible = rows.saturating_sub(3) as usize;
                selected_idx = (selected_idx + visible).min(addresses.len().saturating_sub(1));
            }
        }
        KeyCode::Char('t') | KeyCode::Char('T') => {
            if k.modifiers.contains(KeyModifiers::ALT) {
                let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                themes.sort();

                if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                    theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
                    theme_provider.save_settings();
                    app.update_status(format!("Theme: {}", theme_provider.current_theme));
                }
            } else {
                if let Ok(Some(team_name)) = theme_provider.prompt("Team Name (e.g. My Team): ", false) {
                    let team_name = team_name.trim();
                    if !team_name.is_empty() {
                        if let Ok(Some(emails)) = theme_provider.prompt_with_autocomplete("Emails (comma separated): ", &addresses) {
                            let mut unique_emails = Vec::new();
                            for email in emails.split(',') {
                                let trimmed = email.trim().trim_end_matches(';');
                                if !trimmed.is_empty() && !unique_emails.contains(&trimmed) {
                                    unique_emails.push(trimmed);
                                }
                            }

                            if !unique_emails.is_empty() {
                                let formatted_list = format!("{}: {};", team_name, unique_emails.join(", "));
                                addresses.push(formatted_list);
                                crate::address::clean_and_save_address_book(&mut addresses);
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
                    crate::address::clean_and_save_address_book(&mut addresses);                }
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if !addresses.is_empty() && !addresses[selected_idx].trim().is_empty() {
                let current_val = &addresses[selected_idx];

                if current_val.contains(':') {
                    let (prefix, emails_part) = if let Some(colon_idx) = current_val.find(':') {
                        let prefix = &current_val[..colon_idx];
                        let emails = current_val[colon_idx + 1..].trim_end_matches(';').trim();
                        (prefix, emails)
                    } else {
                        ("", current_val.as_str())
                    };

                    let multiline_emails = emails_part
                        .split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<&str>>()
                        .join("\n");

                    let title = format!("Edit Team: {}", prefix);

                    let _ = execute!(std::io::stdout(), crossterm::terminal::Clear(crossterm::terminal::ClearType::All));
                    let edit_result = theme_provider.edit_buffer(&title, &multiline_emails, crate::editor::MenuState::TeamEditor);
                    let _ = execute!(std::io::stdout(), crossterm::terminal::Clear(crossterm::terminal::ClearType::All));

                    if let Ok(Some(edited_text)) = edit_result {
                        let normalized_text = edited_text.replace('\n', ",").replace(';', ",");

                        let mut unique_emails = Vec::new();
                        for email in normalized_text.split(',') {
                            let trimmed = email.trim();
                            if !trimmed.is_empty() && !unique_emails.contains(&trimmed) {
                                unique_emails.push(trimmed);
                            }
                        }

                        let cleaned_emails = unique_emails.join(", ");

                        if !cleaned_emails.is_empty() {
                            addresses[selected_idx] = format!("{}: {};", prefix, cleaned_emails);
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                    }

                } else {
                    if let Ok(Some(new_val)) = theme_provider.prompt_edit("Edit: ", current_val) {
                        if !new_val.trim().is_empty() {
                            addresses[selected_idx] = new_val.trim().to_string();
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                    }
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('?') => {
            let _ = theme_provider.show_help("address_book");
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            if let Ok(Some(filepath)) = theme_provider.run_file_browser(false, None) {
                let expanded_path = crate::editor::Editor::expand_tilde(&filepath);

                match std::fs::read_to_string(&expanded_path) {
                    Ok(contents) => {
                        let mut added_count = 0;
                        let normalized = contents.replace('\n', ",").replace('\r', "").replace(';', ",");

                        for email in normalized.split(',') {
                            let trimmed = email.trim();
                            if !trimmed.is_empty() && !addresses.iter().any(|a| a.trim() == trimmed) {
                                addresses.push(trimmed.to_string());
                                added_count += 1;
                            }
                        }

                        if added_count > 0 {
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                        theme_provider.set_status(format!("Import successful - {} emails added to address book", added_count));
                    }
                    Err(_) => {
                        theme_provider.set_status("Import not successful".to_string());
                    }
                }
            }
        }
        _ => {}
    }

    if let Some(mode) = next_mode { app.mode = mode; }
    else { app.mode = AppMode::AddressBook { selected_idx, addresses }; }
}

fn handle_email_accounts_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor, _stdout: &mut std::io::Stdout) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::EmailAccounts { selected_idx } => selected_idx,
        other => { app.mode = other; return; }
    };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => selected_idx = (selected_idx + 1).min(app.accounts.len().saturating_sub(1)),
        KeyCode::Char('<') | KeyCode::Left | KeyCode::Esc => next_mode = Some(AppMode::MainMenu { selected_idx: 4 }),
        KeyCode::Char('c') | KeyCode::Char('C') if k.modifiers.contains(KeyModifiers::CONTROL) => next_mode = Some(AppMode::MainMenu { selected_idx: 4 }),
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let auth_options = vec![
                "1. Microsoft Exchange (Graph API, OAuth2.0)".to_string(),
                "2. Google Gmail (OAuth2.0)".to_string(),
                "3. Google Gmail (App Password)".to_string(),
                "4. Basic IMAP (Yahoo/Legacy/Other)".to_string(),
            ];

            if let Ok(Some(selection)) = theme_provider.prompt_select_item("Choose account type:", &auth_options) {

                if let Ok(Some(email)) = theme_provider.prompt("Email: ", false) {
                    let email_lower = email.trim().to_lowercase();

                    // OPTION 1: Microsoft Graph API
                    if selection.starts_with('1') {
                        let client_id = "014bd274-beed-47dd-afba-c2fc4f48ede0".to_string();

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
                        let _ = execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

                        println!("Account Email: {}", new_acc.email);
                        println!("Client ID being sent: '{}'", client_id);

                        match net::run_microsoft_auth_flow(&client_id, "") {
                            Ok(tokens) => {
                                if let Some(refresh) = tokens.refresh_token {
                                    new_acc.refresh_token = Some(refresh);
                                    app.update_status("MS Auth Successful. Account added.".to_string());
                                }
                            },
                            Err(e) => {
                                println!("\r\nAuthentication Failed!");
                                println!("Error details: {}\r\n", e);
                                println!("Press ENTER to return to xpine...");
                                let mut input = String::new();
                                let _ = std::io::stdin().read_line(&mut input);
                                app.update_status("MS Auth Failed. Account added without token.".to_string());
                            }
                        }

                        let _ = crossterm::terminal::enable_raw_mode();
                        let _ = execute!(
                            std::io::stdout(),
                            crossterm::terminal::EnterAlternateScreen,
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                        );

                        app.accounts.push(new_acc);
                        crate::config::save_config(&app.accounts);

                        app.current_account_idx = app.accounts.len() - 1;
                        app.active_account = app.accounts[app.current_account_idx].clone();
                        app.needs_reconnect = true;

                        app.current_folder = "INBOX".to_string();
                        app.graph_pending_deleted.clear();
                        app.current_page = 0;
                        app.restore_index_from_end = Some(0);
                        selected_idx = app.current_account_idx;

                    // OPTION 2: Gmail OAuth 2.0
                    } else if selection.starts_with('2') {
                        let defaults = crate::config::get_provider_defaults(&email);
                        let default_imap = defaults.as_ref().map(|d| d.imap).unwrap_or("imap.gmail.com");

                        if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", default_imap) {
                            let default_port = defaults.as_ref().map(|d| d.port.to_string()).unwrap_or("993".to_string());

                            if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &default_port) {
                                let default_smtp = defaults.as_ref().map(|d| d.smtp).unwrap_or("smtp.gmail.com");

                                if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", default_smtp) {
                                    if let Ok(Some(smtp_port)) = theme_provider.prompt_edit("SMTP Port: ", "587") {

                                        let mut new_acc = crate::config::Account {
                                            email: email.trim().to_string(),
                                            password: None,
                                            client_id: None,
                                            client_secret: None,
                                            refresh_token: None,
                                            imap_server: imap_server.trim().to_string(),
                                            imap_port: imap_port.trim().parse().unwrap_or(993),
                                            smtp_server: smtp_server.trim().to_string(),
                                            smtp_port: smtp_port.trim().parse().unwrap_or(587),
                                        };

                                        let _ = crossterm::terminal::disable_raw_mode();
                                        let _ = execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

                                        println!("Starting OAuth2 flow for {}...", new_acc.email);

                                        match net::run_gmail_oauth_flow(&email_lower) {
                                            Ok((client_id, secret, token)) => {
                                                new_acc.client_id = Some(client_id);
                                                new_acc.client_secret = Some(secret);
                                                new_acc.refresh_token = Some(token);
                                                app.update_status("OAuth Successful!".to_string());
                                            },
                                            Err(e) => {
                                                println!("\r\nOAuth Failed: {}", e);
                                                println!("Press ENTER to return to xpine...");
                                                let mut input = String::new();
                                                let _ = std::io::stdin().read_line(&mut input);
                                                app.update_status("OAuth Failed.".to_string());
                                            }
                                        }

                                        let _ = crossterm::terminal::enable_raw_mode();
                                        let _ = execute!(
                                            std::io::stdout(),
                                            crossterm::terminal::EnterAlternateScreen,
                                            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                                        );

                                        app.accounts.push(new_acc);
                                        crate::config::save_config(&app.accounts);

                                        app.current_account_idx = app.accounts.len() - 1;
                                        app.active_account = app.accounts[app.current_account_idx].clone();
                                        app.needs_reconnect = true;

                                        app.current_folder = "INBOX".to_string();
                                        app.graph_pending_deleted.clear();
                                        app.current_page = 0;
                                        app.restore_index_from_end = Some(0);
                                        selected_idx = app.current_account_idx;
                                    }
                                }
                            }
                        }

                    // OPTION 3 & 4: App Password or Basic IMAP
                    } else if selection.starts_with('3') || selection.starts_with('4') {
                        let pass_prompt = if selection.starts_with('3') { "App Password: " } else { "Password: " };

                        if let Ok(Some(password)) = theme_provider.prompt(pass_prompt, false) {
                            let defaults = crate::config::get_provider_defaults(&email);
                            let default_imap = defaults.as_ref().map(|d| d.imap).unwrap_or("imap.");

                            if let Ok(Some(imap_server)) = theme_provider.prompt_edit("IMAP Server: ", default_imap) {
                                let default_port = defaults.as_ref().map(|d| d.port.to_string()).unwrap_or("993".to_string());

                                if let Ok(Some(imap_port)) = theme_provider.prompt_edit("IMAP Port: ", &default_port) {
                                    let default_smtp = defaults.as_ref().map(|d| d.smtp).unwrap_or("smtp.");

                                    if let Ok(Some(smtp_server)) = theme_provider.prompt_edit("SMTP Server: ", default_smtp) {
                                        if let Ok(Some(smtp_port)) = theme_provider.prompt_edit("SMTP Port: ", "587") {

                                            let new_acc = crate::config::Account {
                                                email: email.trim().to_string(),
                                                password: Some(password.trim().to_string()),
                                                client_id: None,
                                                client_secret: None,
                                                refresh_token: None,
                                                imap_server: imap_server.trim().to_string(),
                                                imap_port: imap_port.trim().parse().unwrap_or(993),
                                                smtp_server: smtp_server.trim().to_string(),
                                                smtp_port: smtp_port.trim().parse().unwrap_or(587),
                                            };

                                            app.accounts.push(new_acc);
                                            crate::config::save_config(&app.accounts);

                                            app.current_account_idx = app.accounts.len() - 1;
                                            app.active_account = app.accounts[app.current_account_idx].clone();
                                            app.needs_reconnect = true;

                                            app.current_folder = "INBOX".to_string();
                                            app.graph_pending_deleted.clear();
                                            app.current_page = 0;
                                            app.restore_index_from_end = Some(0);
                                            selected_idx = app.current_account_idx;
                                        }
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
                let account_email = &app.accounts[selected_idx].email;
                let prompt_msg = format!("Delete account '{}'?", account_email);

                if let Ok(Some(true)) = theme_provider.prompt_yn(&prompt_msg) {
                    if let Ok(Some(true)) = theme_provider.prompt_yn("Are you absolutely sure?") {
                        app.accounts.remove(selected_idx);
                        crate::config::save_config(&app.accounts);

                        if !app.accounts.is_empty() && selected_idx >= app.accounts.len() {
                            selected_idx = app.accounts.len() - 1;
                        }

                        if selected_idx == app.current_account_idx {
                            app.needs_reconnect = true;
                            app.current_account_idx = 0;
                        } else if selected_idx < app.current_account_idx {
                            app.current_account_idx = app.current_account_idx.saturating_sub(1);
                        }
                    }
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('?') => {
            let _ = theme_provider.show_help("email_accounts");
        }
        _ => {}
    }

    if let Some(mode) = next_mode { app.mode = mode; }
    else { app.mode = AppMode::EmailAccounts { selected_idx }; }
}

fn handle_email_list_events(k: KeyEvent, app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor, stdout: &mut std::io::Stdout, quit: &mut bool) {
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
                theme_provider.save_settings();
                app.update_status(format!("Theme: {}", theme_provider.current_theme));
            }
        }
        KeyCode::Char('<') | KeyCode::Left | KeyCode::Esc => {
            if app.search_query.is_some() {
                app.search_query = None;
                app.current_page = 0;
                app.needs_fetch = true;
            } else {
                let mut fetched = Vec::new();
                if let Some(sess) = session {
                    match sess {
                        MailSession::Imap(imap_sess) => {
                            if let Ok(mailboxes) = imap_sess.list(Some(""), Some("*")) {
                                for mb in mailboxes.iter() { fetched.push(mb.name().to_string()); }
                            }
                        }
                        MailSession::Graph { access_token } => {
                            let client = reqwest::blocking::Client::new();
                            let url = "https://graph.microsoft.com/v1.0/me/mailFolders?includeHiddenFolders=true&$top=100";
                            if let Ok(res) = client.get(url)
                                .header("Authorization", format!("Bearer {}", access_token))
                                .send() {
                                if let Ok(graph_data) = res.json::<net::GraphFolderResponse>() {
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
        KeyCode::Char('m') | KeyCode::Char('M') if k.modifiers.contains(KeyModifiers::ALT) => {
            if app.page_emails.is_empty() {
                app.update_status("No email selected.".to_string());
            } else {
                if let Some(sess) = session {
                    match net::list_folders(sess) {
                        Ok(folders) => {
                            if let Ok(Some(dest_input)) = theme_provider.prompt_for_folder("Move to folder: ", &folders) {
                                let clean_dest = dest_input.trim();
                                if !clean_dest.is_empty() {
                                    if let Some(exact_folder) = folders.iter().find(|f| f.eq_ignore_ascii_case(clean_dest)) {
                                        let msg_id = app.page_emails[app.selected_index].id.to_string();
                                        match net::move_email(sess, &msg_id, exact_folder) {
                                            Ok(_) => {
                                                app.update_status(format!("Moved to '{}'", exact_folder));
                                                app.needs_fetch = true;
                                            }
                                            Err(e) => {
                                                app.update_status(e);
                                            }
                                        }
                                    } else {
                                        app.update_status("Folder does not exist. Email not moved.".to_string());
                                    }
                                } else {
                                    app.update_status("Move cancelled.".to_string());
                                }
                            } else {
                                app.update_status("Move cancelled.".to_string());
                            }
                        }
                        Err(e) => {
                            app.update_status(format!("Failed to fetch folders: {}", e));
                        }
                    }
                } else {
                    app.update_status("Offline: Cannot move email".to_string());
                }
            }
        }
        KeyCode::Char('m') | KeyCode::Char('M') if k.modifiers.contains(KeyModifiers::NONE) => {
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

                    let email_id = app.page_emails[idx].id.clone();
                    if app.page_emails[idx].is_deleted {
                        app.graph_pending_deleted.insert(email_id);
                    } else {
                        app.graph_pending_deleted.remove(&email_id);
                    }
                }
            } else {
                if let Some(sess) = session {
                    if !app.page_emails.is_empty() {
                        net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Deleted");

                        // Track standard IMAP deletes identically, allowing cross-page expunge detection
                        let idx = app.selected_index;
                        let email_id = app.page_emails[idx].id.clone();
                        if app.page_emails[idx].is_deleted {
                            app.graph_pending_deleted.insert(email_id);
                        } else {
                            app.graph_pending_deleted.remove(&email_id);
                        }
                    }
                }
            }

            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
            if app.selected_index + 1 < max_visible { app.selected_index += 1; }
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            if !app.page_emails.is_empty() {
                if let Some(sess) = session {
                    // Unified check precisely leverages our cross-page ID array for both protocols
                    let has_deleted = app.page_emails.iter().any(|e| e.is_deleted) || !app.graph_pending_deleted.is_empty();

                    if !has_deleted {
                        app.update_status("Nothing to expunge - no messages marked for deletion".to_string());
                    } else {
                        let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook") ||
                            app.active_account.email.to_lowercase().contains("outlook") ||
                            app.active_account.email.to_lowercase().contains("hotmail");

                        if is_outlook {
                            match sess {
                                MailSession::Imap(imap_sess) => {
                                    for email in &app.page_emails {
                                        if email.is_deleted {
                                            let _ = imap_sess.uid_store(&email.uid.to_string(), "+FLAGS (\\Deleted)");
                                        }
                                    }
                                }
                                MailSession::Graph { .. } => {}
                            }
                        }

                        if let Ok(Some(true)) = theme_provider.prompt_yn("Expunge?") {
                            if net::expunge_deleted(sess, app).is_ok() {
                                // Clear the tracker to reset system logic
                                app.graph_pending_deleted.clear();

                                let offset = if theme_provider.sort_newest_first {
                                    app.current_page * items_per_page + app.selected_index as u32
                                } else {
                                    app.current_page * items_per_page + app.page_emails.len().saturating_sub(1).saturating_sub(app.selected_index) as u32
                                };

                                match sess {
                                    MailSession::Imap(imap_sess) => {
                                        if let Ok(m) = imap_sess.select(&app.current_folder) {
                                            app.total_messages = m.exists;
                                            let safe_offset = offset.min(app.total_messages.saturating_sub(1));
                                            app.current_page = safe_offset / items_per_page;
                                            app.restore_index_from_end = Some(safe_offset % items_per_page);
                                        }
                                    }
                                    MailSession::Graph { .. } => {}
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
            let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);
            if app.selected_index + 1 < max_visible { app.selected_index += 1; }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if !app.accounts.is_empty() {
                // Notice the extra `None` passed for default_cc
                if let Some(status) = compose_email(&app.active_account, None, None, None, None, &mut theme_provider.current_theme) {
                    app.update_status(status);
                }
            } else {
                app.update_status("No account configured for sending.".to_string());
            }
        }
        KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('r') | KeyCode::Char('R') => {
            if !app.page_emails.is_empty() {
                if let Some(sess) = session {
                    // Extract to_addr and cc from the selected email
                    let (fetch_seq, from, date, subject, reply_to, to_addr, cc) = {
                        let current = &app.page_emails[app.selected_index];
                        (current.id.to_string(), current.from.clone(), current.date.clone(), current.subject.clone(), current.reply_to.clone(), current.to_addr.clone(), current.cc.clone())
                    };

                    let (t_body, _, _) = net::fetch_email_body(sess, &fetch_seq);

                    if k.code == KeyCode::Char('f') || k.code == KeyCode::Char('F') {
                        let sub = if subject.to_lowercase().starts_with("fwd:") { subject.clone() } else { format!("Fwd: {}", subject) };
                        let fwd_body = format!("--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", from, date, subject, t_body);
                        if let Some(s) = compose_email(&app.active_account, None, None, Some(&sub), Some(&fwd_body), &mut theme_provider.current_theme) {
                            app.update_status(s);
                        }
                    }
                    else {
                        // Determine if Ctrl is pressed for Reply All
                        let is_reply_all = k.modifiers.contains(KeyModifiers::CONTROL);

                        let raw_reply = if reply_to.trim().is_empty() {
                            crate::mail::extract_email(&from)
                        } else {
                            crate::mail::extract_email(&reply_to)
                        };

                        let sub = if subject.to_lowercase().starts_with("re:") { subject.clone() } else { format!("Re: {}", subject) };
                        let reply_body = crate::mail::format_reply_text(&t_body, &date, &from);

                        // Generate appropriate TO and CC headers
                        let (final_to, final_cc) = if is_reply_all {
                            let (all_to, all_cc) = crate::mail::build_reply_all_addresses(&app.active_account.email, &raw_reply, &to_addr, &cc);
                            (all_to, Some(all_cc))
                        } else {
                            (raw_reply, None)
                        };

                        if let Some(_) = compose_email(&app.active_account, Some(&final_to), final_cc.as_deref(), Some(&sub), Some(&reply_body), &mut theme_provider.current_theme) {
                            match sess {
                                MailSession::Imap(imap_sess) => {
                                    let _ = imap_sess.store(&fetch_seq, "+FLAGS (\\Answered)");
                                }
                                MailSession::Graph { access_token } => {
                                    let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", fetch_seq);
                                    let client = reqwest::blocking::Client::new();
                                    let payload = serde_json::json!({
                                    "singleValueExtendedProperties": [
                                        { "id": "Integer 0x1081", "value": "102" },
                                        { "id": "Integer 0x1080", "value": "261" }
                                    ]
                                    });

                                    let res = client.patch(&url)
                                        .header("Authorization", format!("Bearer {}", access_token))
                                        .header("Content-Type", "application/json")
                                        .json(&payload)
                                        .send();

                                    if let Ok(response) = res {
                                        if !response.status().is_success() {
                                            app.update_status(format!("Failed to mark 'A' on server: {}", response.status()));
                                        }
                                    }
                                }
                            }

                            app.page_emails[app.selected_index].is_answered = true;
                            app.needs_fetch = true;
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
                            MailSession::Imap(imap_sess) => {
                                let _ = imap_sess.store(&fetch_seq, "+FLAGS (\\Seen)");
                            }
                            MailSession::Graph { .. } => {
                                net::toggle_flag(sess, &mut app.page_emails, app.selected_index, "\\Seen");
                            }
                        }
                        app.page_emails[app.selected_index].is_read = true;
                    }

                    app.mode = AppMode::EmailRead { text_body: t_body, html_body: h_body, attachments: atts };
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('?') => {
            let _ = theme_provider.show_help("email_list");
        }
        KeyCode::Char('q')  => {
            check_and_expunge_outlook(app, session, theme_provider);
            *quit = true;
        },
        _ => {}
    }
}

fn handle_folder_list_events(k: KeyEvent, app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor, _stdout: &mut std::io::Stdout, quit: &mut bool) {
    let (mut step, mut selected_idx, mut folders) = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::FolderList { step, selected_idx, folders } => (step, selected_idx, folders),
        other => { app.mode = other; return; }
    };

    let items_count = if step == 0 { app.accounts.len() } else { folders.len() };
    let (_, rows) = term_size().unwrap_or((80, 24));
    let items_per_page = (rows.saturating_sub(3) as usize).max(1);

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => { selected_idx = selected_idx.saturating_sub(1); }
        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => { selected_idx = (selected_idx + 1).min(items_count.saturating_sub(1)); }
        KeyCode::PageUp | KeyCode::Char('y') | KeyCode::Char('Y') => { selected_idx = selected_idx.saturating_sub(items_per_page); }
        KeyCode::PageDown | KeyCode::Char('v') | KeyCode::Char('V') => { selected_idx = (selected_idx + items_per_page).min(items_count.saturating_sub(1)); }
        KeyCode::Char('m') | KeyCode::Char('M') => { next_mode = Some(AppMode::MainMenu { selected_idx: 2 }); }
        KeyCode::Esc | KeyCode::Char('<') | KeyCode::Left => {
            if step == 1 { step = 0; selected_idx = app.current_account_idx; }
            else { next_mode = Some(AppMode::MainMenu { selected_idx: 2 }); }
        }
        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
            if step == 0 {
                if selected_idx < app.accounts.len() {
                    app.active_account = app.accounts[selected_idx].clone();
                    app.current_account_idx = selected_idx;
                    *session = net::connect(&mut app.active_account).ok();

                    let mut fetched = Vec::new();
                    if let Some(sess) = session {
                        match sess {
                            MailSession::Imap(imap_sess) => {
                                if let Ok(mailboxes) = imap_sess.list(Some(""), Some("*")) {
                                    for mb in mailboxes.iter() {
                                        fetched.push(mb.name().to_string());
                                    }
                                }
                            }
                            MailSession::Graph { access_token } => {
                                let client = reqwest::blocking::Client::new();
                                let url = "https://graph.microsoft.com/v1.0/me/mailFolders?includeHiddenFolders=true&$top=100";
                                if let Ok(res) = client.get(url)
                                    .header("Authorization", format!("Bearer {}", access_token))
                                    .send() {
                                    if let Ok(graph_data) = res.json::<net::GraphFolderResponse>() {
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

                    folders = fetched;
                    step = 1;
                    selected_idx = 0;
                }
            } else if step == 1 {
                if !folders.is_empty() {
                    app.current_folder = folders[selected_idx].clone();
                    app.graph_pending_deleted.clear(); // Wiping memory for pure folder integrity
                    app.current_page = 0;
                    app.restore_index_from_end = Some(0);
                    app.needs_fetch = true;
                    next_mode = Some(AppMode::EmailList);
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
        KeyCode::Char('a') | KeyCode::Char('A') => {
            // NEW: Only trigger the prompt if we are actually viewing folders (step != 0)
            if step != 0 {
                if let Ok(Some(folder_name)) = theme_provider.prompt("New Folder Name: ", false) {
                    let clean_name = folder_name.trim();

                    if !clean_name.is_empty() {
                        if let Some(sess) = session {
                            match net::create_folder(sess, clean_name) {
                                Ok(_) => {
                                    app.update_status(format!("Created folder: {}", clean_name));
                                    if let Ok(new_folders) = net::list_folders(sess) {
                                        folders = new_folders;
                                    }
                                }
                                Err(e) => { app.update_status(e); }
                            }
                        } else {
                            app.update_status("Offline: Cannot create folder".to_string());
                        }
                    }
                }
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            let folder_name = folders[selected_idx].clone();
            let lower_name = folder_name.to_lowercase();

            let is_system = matches!(
                lower_name.as_str(),
                "inbox" | "sent" | "sent items" | "drafts" | "trash" | "deleted items" |
                "spam" | "junk" | "junk email" | "outbox" | "archive" | "conversation history" |
                "[gmail]" | "[gmail]/all mail" | "[gmail]/sent mail" | "[gmail]/drafts" |
                "[gmail]/trash" | "[gmail]/spam" | "[gmail]/important" | "[gmail]/starred"
            );

            if is_system {
                app.update_status(format!("Cannot delete system folder: {}", folder_name));
            } else {
                if let Ok(Some(true)) = theme_provider.prompt_yn(&format!("Really delete folder '{}'? (y/n): ", folder_name)) {
                    let absolute_msg = format!("Are you absolutely sure? All emails in '{}' will be lost. (y/n): ", folder_name);
                    if let Ok(Some(true)) = theme_provider.prompt_yn(&absolute_msg) {
                        if let Some(sess) = session {
                            match net::delete_folder(sess, &folder_name) {
                                Ok(_) => {
                                    app.update_status(format!("Deleted folder: {}", folder_name));
                                    if let Ok(new_folders) = net::list_folders(sess) {
                                        folders = new_folders;
                                        if selected_idx >= folders.len() {
                                            selected_idx = folders.len().saturating_sub(1);
                                        }
                                    }
                                }
                                Err(e) => { app.update_status(e); }
                            }
                        } else {
                            app.update_status("Offline: Cannot delete folder".to_string());
                        }
                    } else {
                        app.update_status("Folder deletion cancelled.".to_string());
                    }
                } else {
                    app.update_status("Folder deletion cancelled.".to_string());
                }
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let folder_name = folders[selected_idx].clone();
            let lower_name = folder_name.to_lowercase();

            let is_system = matches!(
                lower_name.as_str(),
                "inbox" | "sent" | "sent items" | "drafts" | "trash" | "deleted items" |
                "spam" | "junk" | "junk email" | "outbox" | "archive" | "conversation history" |
                "[gmail]" | "[gmail]/all mail" | "[gmail]/sent mail" | "[gmail]/drafts" |
                "[gmail]/trash" | "[gmail]/spam" | "[gmail]/important" | "[gmail]/starred"
            );

            if is_system {
                app.update_status(format!("Cannot rename system folder: {}", folder_name));
            } else {
                let prompt_str = format!("Rename '{}' to: ", folder_name);
                if let Ok(Some(new_name_input)) = theme_provider.prompt(&prompt_str, false) {
                    let clean_new_name = new_name_input.trim();
                    if !clean_new_name.is_empty() && clean_new_name != folder_name {
                        if let Some(sess) = session {
                            match net::rename_folder(sess, &folder_name, clean_new_name) {
                                Ok(_) => {
                                    app.update_status(format!("Renamed to: {}", clean_new_name));
                                    if let Ok(new_folders) = net::list_folders(sess) {
                                        folders = new_folders.clone();
                                        if let Some(new_pos) = new_folders.iter().position(|f| f == clean_new_name) {
                                            selected_idx = new_pos;
                                        } else if selected_idx >= folders.len() {
                                            selected_idx = folders.len().saturating_sub(1);
                                        }
                                    }
                                }
                                Err(e) => { app.update_status(e); }
                            }
                        } else {
                            app.update_status("Offline: Cannot rename folder".to_string());
                        }
                    } else if clean_new_name.is_empty() {
                        app.update_status("Rename cancelled: Name cannot be empty.".to_string());
                    }
                } else {
                    app.update_status("Rename cancelled.".to_string());
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('?') => {
            let _ = theme_provider.show_help("folders_list");
        }
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            check_and_expunge_outlook(app, session, theme_provider);
            *quit = true;
        }
        _ => {}
    }

    if let Some(mode) = next_mode { app.mode = mode; }
    else { app.mode = AppMode::FolderList { step, selected_idx, folders }; }
}

fn handle_main_menu_events(k: KeyEvent, app: &mut App, session: &mut Option<MailSession>, theme_provider: &mut Editor, quit: &mut bool) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::MainMenu { selected_idx } => selected_idx,
        other => { app.mode = other; return; }
    };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Char('m') | KeyCode::Char('M') => next_mode = Some(AppMode::EmailList),
        KeyCode::Char('e') | KeyCode::Char('E') => next_mode = Some(AppMode::EmailAccounts { selected_idx: 0 }),
        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => selected_idx = (selected_idx + 1).min(7),
        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
            match selected_idx {
                0 => {
                    app.current_folder = "INBOX".to_string();
                    app.graph_pending_deleted.clear();
                    app.current_page = 0;
                    app.restore_index_from_end = Some(0);
                    app.needs_fetch = true;
                    next_mode = Some(AppMode::EmailList);
                }
                1 => next_mode = Some(AppMode::AddressBook { selected_idx: 0, addresses: crate::address::load_address_book() }),
                2 => next_mode = Some(AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() }),
                3 => next_mode = Some(AppMode::Settings { selected_idx: 0 }),
                4 => {
                    check_and_expunge_outlook(app, session, theme_provider);
                    next_mode = Some(AppMode::EmailAccounts { selected_idx: 0 });
                },
                5 => { let _ = theme_provider.show_help("main_menu"); },
                6 => {
                    if let Some(latest) = &app.latest_version {
                        if latest != env!("CARGO_PKG_VERSION") {
                            let _ = crate::browser::open_url("https://github.com/mabognar/xpine/releases/latest");
                        } else {
                            theme_provider.set_status("xpine is the latest version, nothing to update".to_string());
                        }
                    } else {
                        theme_provider.set_status("Still checking for updates...".to_string());
                    }
                },
                7 => {
                    check_and_expunge_outlook(app, session, theme_provider);
                    *quit = true;
                },
                _ => {}
            }
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            if let Some(latest) = &app.latest_version {
                if latest != env!("CARGO_PKG_VERSION") {
                    let _ = crate::browser::open_url("https://github.com/mabognar/xpine/releases/latest");
                } else {
                    theme_provider.set_status("xpine is the latest version, nothing to update".to_string());
                }
            } else {
                theme_provider.set_status("Still checking for updates...".to_string());
            }
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            app.current_folder = "INBOX".to_string();
            app.graph_pending_deleted.clear();
            app.current_page = 0;
            app.restore_index_from_end = Some(0);
            app.needs_fetch = true;
            next_mode = Some(AppMode::EmailList);
        }
        KeyCode::Char('a') | KeyCode::Char('A') => next_mode = Some(AppMode::AddressBook { selected_idx: 0, addresses: crate::address::load_address_book() }),
        KeyCode::Char('f') | KeyCode::Char('F') => next_mode = Some(AppMode::FolderList { step: 0, selected_idx: app.current_account_idx, folders: Vec::new() }),
        KeyCode::Char('s') | KeyCode::Char('S') => next_mode = Some(AppMode::Settings { selected_idx: 0 }),
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('?') => { let _ = theme_provider.show_help("main_menu"); },
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            check_and_expunge_outlook(app, session, theme_provider);
            *quit = true;
        },
        _ => {}
    }

    if let Some(mode) = next_mode { app.mode = mode; }
    else { app.mode = AppMode::MainMenu { selected_idx }; }
}

fn handle_settings_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::Settings { selected_idx } => selected_idx,
        other => { app.mode = other; return; }
    };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => selected_idx = (selected_idx + 1).min(4),
        KeyCode::Left | KeyCode::Char('<') | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('s') | KeyCode::Char('S') => next_mode = Some(AppMode::MainMenu { selected_idx: 3 }),
        KeyCode::Char('x') | KeyCode::Char('X') | KeyCode::Right | KeyCode::Enter => {
            if selected_idx == 0 { theme_provider.soft_wrap = !theme_provider.soft_wrap; theme_provider.save_settings(); }
            else if selected_idx == 1 { theme_provider.show_line_numbers = !theme_provider.show_line_numbers; theme_provider.save_settings(); }
            else if selected_idx == 2 {
                theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                theme_provider.save_settings();
                app.needs_fetch = true;
            }
            else if selected_idx == 3 {
                theme_provider.spellcheck_before_send = !theme_provider.spellcheck_before_send;
                theme_provider.save_settings();
            }
            else if selected_idx == 4 {
                let current_sig = crate::config::load_signature();
                let edit_result = theme_provider.edit_buffer("Edit Email Signature (leave blank for no signature)", &current_sig, crate::editor::MenuState::EmailComposer);
                let _ = execute!(std::io::stdout(), crossterm::terminal::Clear(crossterm::terminal::ClearType::All));
                if let Ok(Some(new_sig)) = edit_result {
                    crate::config::save_signature(&new_sig);
                }
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

    if let Some(mode) = next_mode { app.mode = mode; }
    else { app.mode = AppMode::Settings { selected_idx }; }
}


