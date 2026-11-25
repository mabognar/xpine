use std::io::Stdout;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    self, size as term_size, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};

use crate::app::{App, AppMode};
use crate::browser::BrowserExt;
use crate::compose::compose_email;
use crate::config::{Account, ConfigExt};
use crate::editor::{Editor, MenuState};
use crate::net::{self, MailSession};
use crate::prompt::PromptExt;
use crate::ui::UiExt;

fn is_outlook_account(account: &Account) -> bool {
    account.imap_server.to_lowercase().contains("outlook")
        || account.email.to_lowercase().contains("outlook")
        || account.email.to_lowercase().contains("hotmail")
}

fn is_system_folder(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "inbox"
            | "sent"
            | "sent items"
            | "drafts"
            | "trash"
            | "deleted items"
            | "spam"
            | "junk"
            | "junk email"
            | "outbox"
            | "archive"
            | "conversation history"
            | "[gmail]"
            | "[gmail]/all mail"
            | "[gmail]/sent mail"
            | "[gmail]/drafts"
            | "[gmail]/trash"
            | "[gmail]/spam"
            | "[gmail]/important"
            | "[gmail]/starred"
    )
}

fn stage_outlook_imap_deletes(sess: &mut MailSession, app: &App) {
    if let MailSession::Imap(imap_sess) = sess {
        for email in &app.page_emails {
            if email.is_deleted {
                let _ = imap_sess.uid_store(&email.uid.to_string(), "+FLAGS (\\Deleted)");
            }
        }
    }
}

fn check_and_expunge_outlook(
    app: &mut App,
    session: &mut Option<MailSession>,
    theme_provider: &mut Editor,
) {
    if !is_outlook_account(&app.active_account) {
        return;
    }

    // let has_pending =
    //     app.page_emails.iter().any(|e| e.is_deleted) || !app.graph_pending_deleted.is_empty();
    // if !has_pending {
    //     return;
    // }

    let delete_count = count_pending_expunge(app);
    if delete_count == 0 {
        return;
    }

    if let Ok(Some(yes)) = theme_provider.prompt_yn(&expunge_prompt_text(delete_count)) {
        if yes {
            if let Some(sess) = session {
                stage_outlook_imap_deletes(sess, app);
                let _ = net::expunge_deleted(sess, app);
                app.needs_fetch = true;
            }
        } else {
            for email in &mut app.page_emails {
                email.is_deleted = false;
            }
            app.graph_pending_deleted.clear();
        }
    }
}

fn cycle_theme(theme_provider: &mut Editor, app: &mut App, show_status: bool) {
    let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
    themes.sort();

    if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
        theme_provider.current_theme = themes[(pos + 1) % themes.len()].clone();
        theme_provider.save_settings();
        if show_status {
            app.update_status(format!("Theme: {}", theme_provider.current_theme));
        }
    }
}

fn reset_view_cache(app: &mut App) {
    app.account_cache.clear();
    app.cache_ready = false;
    app.needs_fetch = true;
}

fn open_folder(app: &mut App, folder: String) {
    app.current_folder = folder;
    app.graph_pending_deleted.clear();
    app.current_page = 0;
    app.restore_index_from_end = Some(0);
    app.cache_ready = false;
    app.needs_fetch = true;
}

fn switch_account(
    app: &mut App,
    session: &mut Option<MailSession>,
    theme_provider: &mut Editor,
    stdout: &mut Stdout,
    target_idx: usize,
) {
    check_and_expunge_outlook(app, session, theme_provider);
    let email = app.accounts[target_idx].email.clone();

    app.update_status(format!("Switching to {}...", email));
    let _ = crate::ui::draw_app(stdout, app, theme_provider);

    net::flush_pending_flags(session, app);
    app.current_account_idx = target_idx;
    app.needs_reconnect = true;
}

fn fetch_folder_list(session: &mut Option<MailSession>) -> (Vec<String>, bool) {
    let mut fetched = Vec::new();
    let mut failed = false;

    if let Some(sess) = session {
        match sess {
            MailSession::Imap(imap_sess) => {
                if let Ok(mailboxes) = imap_sess.list(Some(""), Some("*")) {
                    fetched.extend(mailboxes.iter().map(|mb| mb.name().to_string()));
                } else {
                    failed = true;
                }
            }
            MailSession::Graph { access_token } => {
                let url = "https://graph.microsoft.com/v1.0/me/mailFolders?includeHiddenFolders=true&$top=100";
                match net::http_client().get(url).bearer_auth(access_token.as_str()).send() {
                    Ok(res) if res.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        failed = true;
                    }
                    Ok(res) => {
                        if let Ok(graph_data) = res.json::<net::GraphFolderResponse>() {
                            fetched.extend(graph_data.value.into_iter().map(|f| f.display_name));
                        }
                    }
                    Err(_) => failed = true,
                }
            }
        }
    }

    (fetched, failed)
}

fn mark_message_answered(sess: &mut MailSession, app: &mut App, fetch_seq: &str) {
    match sess {
        MailSession::Imap(imap_sess) => {
            let _ = imap_sess.store(fetch_seq, "+FLAGS (\\Answered)");
        }
        MailSession::Graph { access_token } => {
            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", fetch_seq);
            let payload = serde_json::json!({
                "singleValueExtendedProperties": [
                    { "id": "Integer 0x1081", "value": "102" },
                    { "id": "Integer 0x1080", "value": "261" }
                ]
            });

            if let Ok(response) = net::http_client()
                .patch(&url)
                .bearer_auth(access_token.as_str())
                .json(&payload)
                .send()
            {
                if !response.status().is_success() {
                    app.update_status(format!(
                        "Failed to mark 'A' on server: {}",
                        response.status()
                    ));
                }
            }
        }
    }
}

fn step_page(app: &mut App, total_pages: u32, go_down: bool, sort_newest_first: bool) -> bool {
    let increase_page = go_down == sort_newest_first;
    if increase_page {
        if app.current_page + 1 < total_pages {
            app.current_page += 1;
            app.needs_fetch = true;
            return true;
        }
    } else if app.current_page > 0 {
        app.current_page -= 1;
        app.needs_fetch = true;
        return true;
    }
    false
}

fn format_contact(name: &str, email: &str) -> Option<String> {
    let email_trimmed = email.trim();
    if email_trimmed.is_empty() {
        return None;
    }
    let name_trimmed = name.trim();
    Some(
        if name_trimmed.is_empty() || name_trimmed.eq_ignore_ascii_case("none") {
            email_trimmed.to_string()
        } else {
            format!("{} <{}>", name_trimmed, email_trimmed)
        },
    )
}

fn dedupe_emails(raw: &str) -> Vec<String> {
    let mut unique = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in raw.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        if matches!(c, ',' | '\n' | ';') && !in_quotes {
            let trimmed = current.trim();
            if !trimmed.is_empty() && !unique.iter().any(|u: &String| u == trimmed) {
                unique.push(trimmed.to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() && !unique.iter().any(|u: &String| u == trimmed) {
        unique.push(trimmed.to_string());
    }
    unique
}

fn with_cooked_terminal<T>(f: impl FnOnce() -> T) -> T {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    let result = f();
    let _ = terminal::enable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All)
    );
    result
}

fn prompt_optional_name(theme_provider: &mut Editor) -> Option<Option<String>> {
    let input = theme_provider
        .prompt_with_placeholder("Name: ", "None (enter name - no commas)")
        .ok()??;
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        Some(None)
    } else {
        Some(Some(trimmed.to_string()))
    }
}

fn prompt_server_config(
    theme_provider: &mut Editor,
    email: &str,
    fallback_imap: &str,
    fallback_smtp: &str,
) -> Option<(String, u16, String, u16)> {
    let defaults = crate::config::get_provider_defaults(email);
    let default_imap = defaults.as_ref().map(|d| d.imap).unwrap_or(fallback_imap);
    let default_port = defaults
        .as_ref()
        .map(|d| d.port.to_string())
        .unwrap_or_else(|| "993".to_string());
    let default_smtp = defaults.as_ref().map(|d| d.smtp).unwrap_or(fallback_smtp);

    let imap_server = theme_provider.prompt_edit("IMAP Server: ", default_imap).ok()??;
    let imap_port = theme_provider.prompt_edit("IMAP Port: ", &default_port).ok()??;
    let smtp_server = theme_provider.prompt_edit("SMTP Server: ", default_smtp).ok()??;
    let smtp_port = theme_provider.prompt_edit("SMTP Port: ", "587").ok()??;

    Some((
        imap_server.trim().to_string(),
        imap_port.trim().parse().unwrap_or(993),
        smtp_server.trim().to_string(),
        smtp_port.trim().parse().unwrap_or(587),
    ))
}

fn prompt_new_account(app: &mut App, theme_provider: &mut Editor) -> Option<Account> {
    let auth_options = vec![
        "1. Microsoft Exchange (Graph API, OAuth2.0)".to_string(),
        "2. Google Gmail (OAuth2.0)".to_string(),
        "3. Google Gmail (App Password)".to_string(),
        "4. Basic IMAP (Yahoo/Legacy/Other)".to_string(),
    ];

    let selection = theme_provider
        .prompt_select_item("Choose account type:", &auth_options)
        .ok()??;
    let email = theme_provider.prompt("Email: ", false).ok()??;
    let email_trimmed = email.trim().to_string();
    let email_lower = email_trimmed.to_lowercase();
    let final_name = prompt_optional_name(theme_provider)?;

    // 1: Microsoft Graph API
    if selection.starts_with('1') {
        let client_id = "014bd274-beed-47dd-afba-c2fc4f48ede0".to_string();
        let mut new_acc = Account {
            name: final_name,
            email: email_trimmed,
            password: None,
            client_id: Some(client_id.clone()),
            client_secret: Some("dummy_secret_do_not_remove".to_string()),
            refresh_token: None,
            imap_server: String::new(),
            imap_port: 0,
            smtp_server: String::new(),
            smtp_port: 0,
        };

        let auth_result = with_cooked_terminal(|| {
            println!("Account Email: {}", new_acc.email);
            println!("Client ID being sent: '{}'", client_id);
            let res = net::run_microsoft_auth_flow(&client_id, "");
            if let Err(ref e) = res {
                println!("\r\nAuthentication Failed!");
                println!("Error details: {}\r\n", e);
                println!("Press ENTER to return to xpine...");
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);
            }
            res
        });

        match auth_result {
            Ok(tokens) => {
                if let Some(refresh) = tokens.refresh_token {
                    new_acc.refresh_token = Some(refresh);
                    app.update_status("MS Auth Successful. Account added.".to_string());
                }
            }
            Err(_) => {
                app.update_status("MS Auth Failed. Account added without token.".to_string());
            }
        }

        return Some(new_acc);
    }

    // 2: Gmail OAuth 2.0
    if selection.starts_with('2') {
        let (imap_server, imap_port, smtp_server, smtp_port) =
            prompt_server_config(theme_provider, &email, "imap.gmail.com", "smtp.gmail.com")?;

        let user_client_id = theme_provider
            .prompt("Google Client ID: ", false)
            .ok()??
            .trim()
            .to_string();
        let user_client_secret = theme_provider
            .prompt("Google Client Secret: ", false)
            .ok()??
            .trim()
            .to_string();

        if user_client_id.is_empty() || user_client_secret.is_empty() {
            app.update_status("OAuth cancelled: Client ID and Secret are required.".to_string());
            return None;
        }

        let auth_result = with_cooked_terminal(|| {
            println!("Starting OAuth2 flow for {}...", email_trimmed);
            let res =
                net::run_gmail_oauth_flow(&email_lower, &user_client_id, &user_client_secret);
            if let Err(ref e) = res {
                println!("\r\nOAuth Failed: {}", e);
                println!("Press ENTER to return to xpine...");
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);
            }
            res
        });

        return match auth_result {
            Ok((_, _, token)) => {
                app.update_status("OAuth Successful!".to_string());
                Some(Account {
                    name: final_name,
                    email: email_trimmed,
                    password: None,
                    client_id: Some(user_client_id),
                    client_secret: Some(user_client_secret),
                    refresh_token: Some(token),
                    imap_server,
                    imap_port,
                    smtp_server,
                    smtp_port,
                })
            }
            Err(_) => {
                app.update_status("OAuth Failed.".to_string());
                None
            }
        };
    }

    // 3 & 4: Gmail App Password or Basic IMAP
    if selection.starts_with('3') || selection.starts_with('4') {
        let pass_prompt = if selection.starts_with('3') {
            "App Password: "
        } else {
            "Password: "
        };
        let password = theme_provider.prompt(pass_prompt, false).ok()??;
        let (imap_server, imap_port, smtp_server, smtp_port) =
            prompt_server_config(theme_provider, &email, "imap.", "smtp.")?;

        return Some(Account {
            name: final_name,
            email: email_trimmed,
            password: Some(password.trim().to_string()),
            client_id: None,
            client_secret: None,
            refresh_token: None,
            imap_server,
            imap_port,
            smtp_server,
            smtp_port,
        });
    }

    None
}

fn activate_new_account(app: &mut App, new_acc: Account) -> usize {
    app.accounts.push(new_acc);
    crate::config::save_config(&app.accounts);

    app.current_account_idx = app.accounts.len() - 1;
    app.active_account = app.accounts[app.current_account_idx].clone();
    app.needs_reconnect = true;

    app.current_folder = "INBOX".to_string();
    app.graph_pending_deleted.clear();
    app.current_page = 0;
    app.restore_index_from_end = Some(0);
    app.current_account_idx
}

pub fn handle_event(
    event: Event,
    app: &mut App,
    session: &mut Option<MailSession>,
    session_pool: &mut net::SessionPool,
    theme_provider: &mut Editor,
    stdout: &mut Stdout,
) -> bool {
    let mut quit = false;

    if let Event::Key(k) = event {
        if k.kind == KeyEventKind::Press {
            match &app.mode {
                AppMode::AddressBook { .. } => {
                    handle_address_book_events(k, app, theme_provider);
                }
                AppMode::EmailAccounts { .. } => {
                    handle_email_accounts_events(k, app, theme_provider);
                }
                AppMode::EmailList => {
                    handle_email_list_events(k, app, session, theme_provider, stdout, &mut quit);
                }
                AppMode::FolderList { .. } => {
                    handle_folder_list_events(
                        k,
                        app,
                        session,
                        session_pool,
                        theme_provider,
                        &mut quit,
                    );
                }
                AppMode::MainMenu { .. } => {
                    handle_main_menu_events(k, app, session, theme_provider, &mut quit);
                }
                AppMode::Settings { .. } => {
                    handle_settings_events(k, app, theme_provider);
                }
                AppMode::EmailRead { .. } => {} // Handled completely in src/read.rs
            }
        }
    } else if let Event::Resize(_, _) = event {
        reset_view_cache(app);
    }

    quit
}

fn handle_address_book_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor) {
    let (mut selected_idx, mut addresses) =
        match std::mem::replace(&mut app.mode, AppMode::EmailList) {
            AppMode::AddressBook {
                selected_idx,
                addresses,
            } => (selected_idx, addresses),
            other => {
                app.mode = other;
                return;
            }
        };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p' | 'P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            if selected_idx + 1 < addresses.len() {
                selected_idx += 1;
            }
        }
        KeyCode::Char('<' | 'm' | 'M') | KeyCode::Left | KeyCode::Esc => {
            next_mode = Some(AppMode::MainMenu { selected_idx: 1 });
        }
        KeyCode::Char('d' | 'D') => {
            if !addresses.is_empty() {
                let current_val = addresses[selected_idx].clone();
                let is_team = current_val.contains(':');

                if let Ok(Some(true)) = theme_provider.prompt_yn(&format!("Delete {}?", current_val))
                {
                    let confirmed = !is_team
                        || matches!(theme_provider.prompt_yn("Are you sure?"), Ok(Some(true)));
                    if confirmed {
                        addresses.remove(selected_idx);
                        let _ = crate::address::save_address_book(&addresses);
                        if !addresses.is_empty() && selected_idx >= addresses.len() {
                            selected_idx = addresses.len().saturating_sub(1);
                        }
                    }
                }
            }
        }
        KeyCode::Char('y' | 'Y') | KeyCode::PageUp => {
            if let Ok((_, rows)) = term_size() {
                let visible = rows.saturating_sub(3) as usize;
                selected_idx = selected_idx.saturating_sub(visible);
            }
        }
        KeyCode::Char('v' | 'V') | KeyCode::PageDown => {
            if let Ok((_, rows)) = term_size() {
                let visible = rows.saturating_sub(3) as usize;
                selected_idx = (selected_idx + visible).min(addresses.len().saturating_sub(1));
            }
        }
        KeyCode::Char('t' | 'T') if k.modifiers.contains(KeyModifiers::ALT) => {
            cycle_theme(theme_provider, app, true);
        }
        KeyCode::Char('t' | 'T') => {
            if let Ok(Some(team_name)) = theme_provider.prompt("Team Name (e.g. My Team): ", false)
            {
                let team_name = team_name.trim();
                if !team_name.is_empty() {
                    if let Ok(Some(emails)) = theme_provider
                        .prompt_with_autocomplete("Emails (comma separated): ", &addresses)
                    {
                        let unique_emails = dedupe_emails(&emails);
                        if !unique_emails.is_empty() {
                            addresses
                                .push(format!("{}: {};", team_name, unique_emails.join(", ")));
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                    }
                }
            }
        }
        KeyCode::Char('a' | 'A') => {
            if let Ok(Some(name_val)) = theme_provider.prompt_with_placeholder("Name: ", "None") {
                if let Ok(Some(email_val)) = theme_provider.prompt("Email: ", false) {
                    if let Some(final_entry) = format_contact(&name_val, &email_val) {
                        if !addresses
                            .iter()
                            .any(|a| a.trim().eq_ignore_ascii_case(&final_entry))
                        {
                            addresses.push(final_entry);
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                    }
                }
            }
        }
        KeyCode::Char('e' | 'E') => {
            if !addresses.is_empty() && !addresses[selected_idx].trim().is_empty() {
                let current_val = addresses[selected_idx].clone();

                if let Some(colon_idx) = current_val.find(':') {
                    let prefix = &current_val[..colon_idx];
                    let emails_part = current_val[colon_idx + 1..].trim_end_matches(';').trim();

                    let multiline_emails = crate::address::split_unquoted_commas(emails_part).join("\n");
                    
                    let title = format!("Edit Team: {}", prefix);
                    let _ = execute!(std::io::stdout(), Clear(ClearType::All));
                    let edit_result =
                        theme_provider.edit_buffer(&title, &multiline_emails, MenuState::TeamEditor);
                    let _ = execute!(std::io::stdout(), Clear(ClearType::All));

                    if let Ok(Some(edited_text)) = edit_result {
                        let cleaned_emails = dedupe_emails(&edited_text).join(", ");
                        if !cleaned_emails.is_empty() {
                            addresses[selected_idx] = format!("{}: {};", prefix, cleaned_emails);
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                    }
                } else {
                    let (existing_name, existing_email) = current_val
                        .find('<')
                        .and_then(|start| {
                            current_val.find('>').map(|end| {
                                let name = current_val[..start]
                                    .trim()
                                    .trim_matches('"')
                                    .trim()
                                    .to_string();
                                let email = current_val[start + 1..end].trim().to_string();
                                (name, email)
                            })
                        })
                        .unwrap_or_else(|| (String::new(), current_val.clone()));

                    let name_result = if existing_name.is_empty() {
                        theme_provider.prompt_with_placeholder("Edit Name: ", "None")
                    } else {
                        theme_provider.prompt_edit("Edit Name: ", &existing_name)
                    };

                    if let Ok(Some(new_name)) = name_result {
                        if let Ok(Some(new_email)) =
                            theme_provider.prompt_edit("Edit Email: ", &existing_email)
                        {
                            if let Some(final_entry) = format_contact(&new_name, &new_email) {
                                addresses[selected_idx] = final_entry;
                                crate::address::clean_and_save_address_book(&mut addresses);
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Char('h' | 'H' | '?') => {
            let _ = theme_provider.show_help("address_book");
        }
        KeyCode::Char('i' | 'I') => {
            if let Ok(Some(filepath)) = theme_provider.run_file_browser(false, None) {
                let expanded_path = Editor::expand_tilde(&filepath);
                match std::fs::read_to_string(&expanded_path) {
                    Ok(contents) => {
                        let mut added_count = 0;
                        for email in contents.split([',', '\n', ';']) {
                            let trimmed = email.trim();
                            if !trimmed.is_empty() && !addresses.iter().any(|a| a.trim() == trimmed)
                            {
                                addresses.push(trimmed.to_string());
                                added_count += 1;
                            }
                        }

                        if added_count > 0 {
                            crate::address::clean_and_save_address_book(&mut addresses);
                        }
                        theme_provider.set_status(format!(
                            "Import successful - {} emails added to address book",
                            added_count
                        ));
                    }
                    Err(_) => {
                        theme_provider.set_status("Import not successful".to_string());
                    }
                }
            }
        }
        _ => {}
    }

    app.mode = next_mode.unwrap_or(AppMode::AddressBook {
        selected_idx,
        addresses,
    });
}

fn handle_email_accounts_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::EmailAccounts { selected_idx } => selected_idx,
        other => {
            app.mode = other;
            return;
        }
    };

    let mut next_mode = None;

    match k.code {
        KeyCode::Up if k.modifiers.contains(KeyModifiers::SHIFT) => {
            if selected_idx > 0 && selected_idx < app.accounts.len() {
                app.accounts.swap(selected_idx, selected_idx - 1);
                if app.current_account_idx == selected_idx {
                    app.current_account_idx = selected_idx - 1;
                } else if app.current_account_idx == selected_idx - 1 {
                    app.current_account_idx = selected_idx;
                }
                selected_idx -= 1;
                crate::config::save_config(&app.accounts);
            }
        }
        KeyCode::Down if k.modifiers.contains(KeyModifiers::SHIFT) => {
            if selected_idx + 1 < app.accounts.len() {
                app.accounts.swap(selected_idx, selected_idx + 1);
                if app.current_account_idx == selected_idx {
                    app.current_account_idx = selected_idx + 1;
                } else if app.current_account_idx == selected_idx + 1 {
                    app.current_account_idx = selected_idx;
                }
                selected_idx += 1;
                crate::config::save_config(&app.accounts);
            }
        }
        KeyCode::Char('+' | '=') => {
            if selected_idx > 0 && selected_idx < app.accounts.len() {
                app.accounts.swap(selected_idx, selected_idx - 1);
                if app.current_account_idx == selected_idx {
                    app.current_account_idx = selected_idx - 1;
                } else if app.current_account_idx == selected_idx - 1 {
                    app.current_account_idx = selected_idx;
                }
                selected_idx -= 1;
                crate::config::save_config(&app.accounts);
            }
        }
        KeyCode::Char('-') => {
            if selected_idx + 1 < app.accounts.len() {
                app.accounts.swap(selected_idx, selected_idx + 1);
                if app.current_account_idx == selected_idx {
                    app.current_account_idx = selected_idx + 1;
                } else if app.current_account_idx == selected_idx + 1 {
                    app.current_account_idx = selected_idx;
                }
                selected_idx += 1;
                crate::config::save_config(&app.accounts);
            }
        }
        KeyCode::Up | KeyCode::Char('p' | 'P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            selected_idx = (selected_idx + 1).min(app.accounts.len().saturating_sub(1));
        }
        KeyCode::Char('<' | 'm' | 'M') | KeyCode::Left | KeyCode::Esc => {
            next_mode = Some(AppMode::MainMenu { selected_idx: 4 });
        }
        KeyCode::Char('c' | 'C') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            next_mode = Some(AppMode::MainMenu { selected_idx: 4 });
        }
        KeyCode::Char('a' | 'A') => {
            if let Some(new_acc) = prompt_new_account(app, theme_provider) {
                selected_idx = activate_new_account(app, new_acc);
            }
        }
        KeyCode::Char('d' | 'D') => {
            if !app.accounts.is_empty() {
                let prompt_msg = format!("Delete account '{}'?", app.accounts[selected_idx].email);
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
        KeyCode::Char('h' | 'H' | '?') => {
            let _ = theme_provider.show_help("email_accounts");
        }
        _ => {}
    }

    app.mode = next_mode.unwrap_or(AppMode::EmailAccounts { selected_idx });
}

fn handle_email_list_events(
    k: KeyEvent,
    app: &mut App,
    session: &mut Option<MailSession>,
    theme_provider: &mut Editor,
    stdout: &mut Stdout,
    quit: &mut bool,
) {
    let (_, rows) = term_size().unwrap_or((80, 24));
    let items_per_page = (rows.saturating_sub(3) as u32).max(1);
    let total_pages = if app.total_messages == 0 {
        1
    } else {
        (app.total_messages + items_per_page - 1) / items_per_page
    };
    let max_visible = app.page_emails.len().min(rows.saturating_sub(3) as usize);

    match k.code {
        KeyCode::Char('t' | 'T') if k.modifiers.contains(KeyModifiers::ALT) => {
            cycle_theme(theme_provider, app, true);
        }
        KeyCode::Char('<') | KeyCode::Left | KeyCode::Esc => {
            if app.search_query.is_some() {
                app.search_query = None;
                app.current_page = 0;
                app.needs_fetch = true;
            } else {
                net::flush_pending_flags(session, app);
                let (mut fetched, _) = fetch_folder_list(session);
                if fetched.is_empty() {
                    fetched.push("INBOX".to_string());
                }
                fetched.sort();

                let idx = fetched
                    .iter()
                    .position(|f| f == &app.current_folder)
                    .unwrap_or(0);
                app.mode = AppMode::FolderList {
                    step: 1,
                    selected_idx: idx,
                    folders: fetched,
                };
            }
        }
        KeyCode::Tab => {
            if app.accounts.len() > 1 {
                let next_idx = (app.current_account_idx + 1) % app.accounts.len();
                switch_account(app, session, theme_provider, stdout, next_idx);
            }
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            if let Some(digit) = c.to_digit(10) {
                let idx = (digit as usize).saturating_sub(1);
                if idx < app.accounts.len() && idx != app.current_account_idx {
                    switch_account(app, session, theme_provider, stdout, idx);
                }
            }
        }
        KeyCode::Char('s' | 'S') => {
            let current_query = app.search_query.clone().unwrap_or_default();
            if let Ok(Some(query)) = theme_provider.prompt_edit("Search:", &current_query) {
                let trimmed = query.trim();
                app.search_query = (!trimmed.is_empty()).then(|| trimmed.to_string());
                app.current_page = 0;
                app.needs_fetch = true;
            }
            let _ = execute!(std::io::stdout(), crossterm::cursor::Show);
        }
        KeyCode::Up | KeyCode::Char('p' | 'P') => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            } else if step_page(app, total_pages, false, theme_provider.sort_newest_first) {
                app.selected_index = (items_per_page - 1) as usize;
            }
        }
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            if !app.page_emails.is_empty() {
                if app.selected_index + 1 < max_visible {
                    app.selected_index += 1;
                } else if step_page(app, total_pages, true, theme_provider.sort_newest_first) {
                    app.selected_index = 0;
                }
            }
        }
        KeyCode::PageUp | KeyCode::Char('y' | 'Y' | '-') => {
            step_page(app, total_pages, false, theme_provider.sort_newest_first);
            app.selected_index = 0;
        }
        KeyCode::PageDown | KeyCode::Char('v' | 'V' | ' ') => {
            if step_page(app, total_pages, true, theme_provider.sort_newest_first) {
                app.selected_index = 0;
            } else {
                app.selected_index = app.page_emails.len().saturating_sub(1);
            }
        }
        KeyCode::Char('m' | 'M') if k.modifiers.contains(KeyModifiers::ALT) => {
            if app.page_emails.is_empty() {
                app.update_status("No email selected.".to_string());
                return;
            }
            let Some(sess) = session else {
                app.update_status("Offline: Cannot move email".to_string());
                return;
            };
            match net::list_folders(sess) {
                Ok(folders) => {
                    match theme_provider.prompt_for_folder("Move to folder: ", &folders) {
                        Ok(Some(dest_input)) if !dest_input.trim().is_empty() => {
                            let clean_dest = dest_input.trim();
                            if let Some(exact_folder) =
                                folders.iter().find(|f| f.eq_ignore_ascii_case(clean_dest))
                            {
                                let msg_id = app.page_emails[app.selected_index].id.clone();
                                net::flush_pending_flags_with_session(sess, app);
                                match net::move_email(sess, &msg_id, exact_folder) {
                                    Ok(_) => {
                                        app.update_status(format!("Moved to '{}'", exact_folder));
                                        app.needs_fetch = true;
                                    }
                                    Err(e) => app.update_status(e),
                                }
                            } else {
                                app.update_status(
                                    "Folder does not exist. Email not moved.".to_string(),
                                );
                            }
                        }
                        _ => app.update_status("Move cancelled.".to_string()),
                    }
                }
                Err(e) => app.update_status(format!("Failed to fetch folders: {}", e)),
            }
        }
        KeyCode::Char('m' | 'M') => {
            net::flush_pending_flags(session, app);
            check_and_expunge_outlook(app, session, theme_provider);
            app.mode = AppMode::MainMenu { selected_idx: 0 };
        }
        KeyCode::Char('o' | 'O') => {
            app.menu_page = if app.menu_page == 1 { 2 } else { 1 };
        }
        KeyCode::Char('*') => {
            if let Some(sess) = session {
                net::toggle_flag(sess, app, "\\Flagged");
            }
        }
        KeyCode::Char('d' | 'D') => {
            if !app.page_emails.is_empty() {
                let toggled = if is_outlook_account(&app.active_account) {
                    let email = &mut app.page_emails[app.selected_index];
                    email.is_deleted = !email.is_deleted;
                    true
                } else if let Some(sess) = session {
                    net::toggle_flag(sess, app, "\\Deleted");
                    true
                } else {
                    false
                };

                if toggled {
                    let email = &app.page_emails[app.selected_index];
                    if email.is_deleted {
                        app.graph_pending_deleted.insert(email.id.clone());
                    } else {
                        app.graph_pending_deleted.remove(&email.id);
                    }
                }
            }

            if app.selected_index + 1 < max_visible {
                app.selected_index += 1;
            }
        }
        // KeyCode::Char('x' | 'X') => {
        //     if app.page_emails.is_empty() {
        //         return;
        //     }
        //     let Some(sess) = session else {
        //         return;
        //     };
        //
        //     let has_deleted = app.page_emails.iter().any(|e| e.is_deleted)
        //         || !app.graph_pending_deleted.is_empty();
        //     if !has_deleted {
        //         app.update_status(
        //             "Nothing to expunge - no messages marked for deletion".to_string(),
        //         );
        //         return;
        //     }
        //
        //     if is_outlook_account(&app.active_account) {
        //         stage_outlook_imap_deletes(sess, app);
        //     }
        //
        //     if let Ok(Some(true)) = theme_provider.prompt_yn("Expunge?") {
        //         app.update_status("Expunging messages...".to_string());
        //         let _ = crate::ui::draw_app(stdout, app, theme_provider);

        KeyCode::Char('x' | 'X') => {
            if app.page_emails.is_empty() {
                return;
            }
            let Some(sess) = session else {
                return;
            };

            let delete_count = count_pending_expunge(app);
            if delete_count == 0 {
                app.update_status(
                    "Nothing to expunge - no messages marked for deletion".to_string(),
                );
                return;
            }

            if is_outlook_account(&app.active_account) {
                stage_outlook_imap_deletes(sess, app);
            }

            if let Ok(Some(true)) = theme_provider.prompt_yn(&expunge_prompt_text(delete_count)) {
                app.update_status("Expunging messages...".to_string());
                let _ = crate::ui::draw_app(stdout, app, theme_provider);

                if net::expunge_deleted(sess, app).is_ok() {
                    app.graph_pending_deleted.clear();

                    let offset = if theme_provider.sort_newest_first {
                        app.current_page * items_per_page + app.selected_index as u32
                    } else {
                        app.current_page * items_per_page
                            + app
                            .page_emails
                            .len()
                            .saturating_sub(1)
                            .saturating_sub(app.selected_index)
                            as u32
                    };

                    if let MailSession::Imap(imap_sess) = sess {
                        if let Ok(m) = imap_sess.select(&app.current_folder) {
                            app.total_messages = m.exists;
                            let safe_offset = offset.min(app.total_messages.saturating_sub(1));
                            app.current_page = safe_offset / items_per_page;
                            app.restore_index_from_end = Some(safe_offset % items_per_page);
                        }
                    }
                } else {
                    app.update_status("Expunge failed.".to_string());
                    app.list_status_duration = Duration::from_secs(3);
                }
            }
        }
        KeyCode::Char('u' | 'U') => {
            if let Some(sess) = session {
                net::toggle_flag(sess, app, "\\Seen");
            }
            if app.selected_index + 1 < max_visible {
                app.selected_index += 1;
            }
        }
        KeyCode::Char('c' | 'C') => {
            if !app.accounts.is_empty() {
                if let Some(status) = compose_email(
                    &app.active_account,
                    None,
                    None,
                    None,
                    None,
                    &mut theme_provider.current_theme,
                ) {
                    app.update_status(status);
                }
            } else {
                app.update_status("No account configured for sending.".to_string());
            }
        }
        KeyCode::Char('f' | 'F' | 'r' | 'R') => {
            if app.page_emails.is_empty() {
                return;
            }
            let Some(sess) = session else {
                return;
            };

            let current = &app.page_emails[app.selected_index];
            let fetch_seq = current.id.clone();
            let from = current.from.clone();
            let date = current.date.clone();
            let subject = current.subject.clone();
            let reply_to = current.reply_to.clone();
            let to_addr = current.to_addr.clone();
            let cc = current.cc.clone();

            let (t_body, _, _) = net::fetch_email_body(sess, &fetch_seq);

            if matches!(k.code, KeyCode::Char('f' | 'F')) {
                let sub = if subject.to_lowercase().starts_with("fwd:") {
                    subject.clone()
                } else {
                    format!("Fwd: {}", subject)
                };
                let fwd_body = format!(
                    "--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}",
                    from, date, subject, t_body
                );
                if let Some(s) = compose_email(
                    &app.active_account,
                    None,
                    None,
                    Some(&sub),
                    Some(&fwd_body),
                    &mut theme_provider.current_theme,
                ) {
                    app.update_status(s);
                }
            } else {
                let is_reply_all = k.modifiers.contains(KeyModifiers::CONTROL);
                let raw_reply = if reply_to.trim().is_empty()
                    || crate::mail::extract_email(&reply_to) == crate::mail::extract_email(&from)
                {
                    crate::mail::clean_display_addresses(&from)
                } else {
                    crate::mail::clean_display_addresses(&reply_to)
                };

                let sub = if subject.to_lowercase().starts_with("re:") {
                    subject
                } else {
                    format!("Re: {}", subject)
                };
                let reply_body = crate::mail::format_reply_text(&t_body, &date, &from);

                let (final_to, final_cc) = if is_reply_all {
                    let (all_to, all_cc) = crate::mail::build_reply_all_addresses(
                        &app.active_account.email,
                        &raw_reply,
                        &to_addr,
                        &cc,
                    );
                    (all_to, Some(all_cc))
                } else {
                    (raw_reply, None)
                };

                if compose_email(
                    &app.active_account,
                    Some(&final_to),
                    final_cc.as_deref(),
                    Some(&sub),
                    Some(&reply_body),
                    &mut theme_provider.current_theme,
                )
                    .is_some()
                {
                    mark_message_answered(sess, app, &fetch_seq);
                    app.page_emails[app.selected_index].is_answered = true;
                    app.needs_fetch = true;
                }
            }
        }
        KeyCode::Char('>') | KeyCode::Enter | KeyCode::Right => {
            if !app.page_emails.is_empty() {
                if let Some(sess) = session {
                    let fetch_seq = app.page_emails[app.selected_index].id.clone();
                    let (t_body, h_body, atts) = net::fetch_email_body(sess, &fetch_seq);

                    if !app.page_emails[app.selected_index].is_read {
                        match sess {
                            MailSession::Imap(_) => {
                                app.page_emails[app.selected_index].is_read = true;
                            }
                            MailSession::Graph { .. } => {
                                net::toggle_flag(sess, app, "\\Seen");
                            }
                        }
                    }

                    app.mode = AppMode::EmailRead {
                        text_body: t_body,
                        html_body: h_body,
                        attachments: atts,
                    };
                }
            }
        }
        KeyCode::Char('h' | 'H' | '?') => {
            let _ = theme_provider.show_help("email_list");
        }
        KeyCode::Char('q') => {
            net::flush_pending_flags(session, app);
            check_and_expunge_outlook(app, session, theme_provider);
            *quit = true;
        }
        _ => {}
    }
}

fn handle_folder_list_events(
    k: KeyEvent,
    app: &mut App,
    session: &mut Option<MailSession>,
    session_pool: &mut net::SessionPool,
    theme_provider: &mut Editor,
    quit: &mut bool,
) {
    let (mut step, mut selected_idx, mut folders) =
        match std::mem::replace(&mut app.mode, AppMode::EmailList) {
            AppMode::FolderList {
                step,
                selected_idx,
                folders,
            } => (step, selected_idx, folders),
            other => {
                app.mode = other;
                return;
            }
        };

    let items_count = if step == 0 {
        app.accounts.len()
    } else {
        folders.len()
    };
    let (_, rows) = term_size().unwrap_or((80, 24));
    let items_per_page = (rows.saturating_sub(3) as usize).max(1);

    let mut next_mode = None;

    match k.code {
        KeyCode::Up | KeyCode::Char('p' | 'P') => selected_idx = selected_idx.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            selected_idx = (selected_idx + 1).min(items_count.saturating_sub(1));
        }
        KeyCode::PageUp | KeyCode::Char('y' | 'Y') => {
            selected_idx = selected_idx.saturating_sub(items_per_page);
        }
        KeyCode::PageDown | KeyCode::Char('v' | 'V') => {
            selected_idx = (selected_idx + items_per_page).min(items_count.saturating_sub(1));
        }
        KeyCode::Char('m' | 'M') => {
            next_mode = Some(AppMode::MainMenu { selected_idx: 2 });
        }
        KeyCode::Esc | KeyCode::Char('<') | KeyCode::Left => {
            if step == 1 {
                step = 0;
                selected_idx = app.current_account_idx;
            } else {
                next_mode = Some(AppMode::MainMenu { selected_idx: 2 });
            }
        }
        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => {
            if step == 0 && selected_idx < app.accounts.len() {
                net::flush_pending_flags(session, app);
                app.current_account_idx = selected_idx;
                net::reconnect(app, session, session_pool);
                app.needs_fetch = false;

                let mut fetched = Vec::new();
                for attempt in 0..2 {
                    let (list, failed) = fetch_folder_list(session);
                    fetched = list;
                    if !failed || attempt == 1 {
                        break;
                    }
                    net::reconnect(app, session, session_pool);
                    app.needs_fetch = false;
                }

                if fetched.is_empty() {
                    fetched.push("INBOX".to_string());
                }
                folders = fetched;
                step = 1;
                selected_idx = 0;
            } else if step == 1 && !folders.is_empty() {
                open_folder(app, folders[selected_idx].clone());
                next_mode = Some(AppMode::EmailList);
            }
        }
        KeyCode::Char('t' | 'T') if k.modifiers.contains(KeyModifiers::ALT) => {
            cycle_theme(theme_provider, app, false);
        }
        KeyCode::Char('a' | 'A') if step != 0 => {
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
                            Err(e) => app.update_status(e),
                        }
                    } else {
                        app.update_status("Offline: Cannot create folder".to_string());
                    }
                }
            }
        }
        KeyCode::Char('d' | 'D') if step != 0 && !folders.is_empty() => {
            let folder_name = folders[selected_idx].clone();
            if is_system_folder(&folder_name) {
                app.update_status(format!("Cannot delete system folder: {}", folder_name));
            } else if matches!(
                theme_provider.prompt_yn(&format!("Really delete folder '{}'? (y/n): ", folder_name)),
                Ok(Some(true))
            ) && matches!(
                theme_provider.prompt_yn(&format!(
                    "Are you absolutely sure? All emails in '{}' will be lost. (y/n): ",
                    folder_name
                )),
                Ok(Some(true))
            ) {
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
                        Err(e) => app.update_status(e),
                    }
                } else {
                    app.update_status("Offline: Cannot delete folder".to_string());
                }
            } else {
                app.update_status("Folder deletion cancelled.".to_string());
            }
        }
        KeyCode::Char('r' | 'R') if step != 0 && !folders.is_empty() => {
            let folder_name = folders[selected_idx].clone();
            if is_system_folder(&folder_name) {
                app.update_status(format!("Cannot rename system folder: {}", folder_name));
            } else {
                match theme_provider.prompt(&format!("Rename '{}' to: ", folder_name), false) {
                    Ok(Some(new_name_input)) => {
                        let clean_new_name = new_name_input.trim();
                        if clean_new_name.is_empty() {
                            app.update_status(
                                "Rename cancelled: Name cannot be empty.".to_string(),
                            );
                        } else if clean_new_name != folder_name {
                            if let Some(sess) = session {
                                match net::rename_folder(sess, &folder_name, clean_new_name) {
                                    Ok(_) => {
                                        app.update_status(format!("Renamed to: {}", clean_new_name));
                                        if let Ok(new_folders) = net::list_folders(sess) {
                                            if let Some(pos) =
                                                new_folders.iter().position(|f| f == clean_new_name)
                                            {
                                                selected_idx = pos;
                                            } else if selected_idx >= new_folders.len() {
                                                selected_idx = new_folders.len().saturating_sub(1);
                                            }
                                            folders = new_folders;
                                        }
                                    }
                                    Err(e) => app.update_status(e),
                                }
                            } else {
                                app.update_status("Offline: Cannot rename folder".to_string());
                            }
                        }
                    }
                    _ => app.update_status("Rename cancelled.".to_string()),
                }
            }
        }
        KeyCode::Char('h' | 'H' | '?') => {
            let _ = theme_provider.show_help("folders_list");
        }
        KeyCode::Char('q' | 'Q') => {
            check_and_expunge_outlook(app, session, theme_provider);
            *quit = true;
        }
        _ => {}
    }

    app.mode = next_mode.unwrap_or(AppMode::FolderList {
        step,
        selected_idx,
        folders,
    });
}

fn handle_main_menu_events(
    k: KeyEvent,
    app: &mut App,
    session: &mut Option<MailSession>,
    theme_provider: &mut Editor,
    quit: &mut bool,
) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::MainMenu { selected_idx } => selected_idx,
        other => {
            app.mode = other;
            return;
        }
    };

    let mut next_mode = None;

    let action_idx = match k.code {
        KeyCode::Up | KeyCode::Char('p' | 'P') => {
            selected_idx = selected_idx.saturating_sub(1);
            None
        }
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            selected_idx = (selected_idx + 1).min(7);
            None
        }
        KeyCode::Enter | KeyCode::Char('>') | KeyCode::Right => Some(selected_idx),
        KeyCode::Char('i' | 'I') => Some(0),
        KeyCode::Char('a' | 'A') => Some(1),
        KeyCode::Char('f' | 'F') => Some(2),
        KeyCode::Char('s' | 'S') => Some(3),
        KeyCode::Char('e' | 'E') => Some(4),
        KeyCode::Char('h' | 'H' | '?') => Some(5),
        KeyCode::Char('u' | 'U') => Some(6),
        KeyCode::Char('q' | 'Q') => Some(7),
        _ => None,
    };

    if let Some(idx) = action_idx {
        match idx {
            0 => {
                open_folder(app, "INBOX".to_string());
                next_mode = Some(AppMode::EmailList);
            }
            1 => {
                next_mode = Some(AppMode::AddressBook {
                    selected_idx: 0,
                    addresses: crate::address::load_address_book(),
                });
            }
            2 => {
                next_mode = Some(AppMode::FolderList {
                    step: 0,
                    selected_idx: app.current_account_idx,
                    folders: Vec::new(),
                });
            }
            3 => next_mode = Some(AppMode::Settings { selected_idx: 0 }),
            4 => {
                check_and_expunge_outlook(app, session, theme_provider);
                next_mode = Some(AppMode::EmailAccounts { selected_idx: 0 });
            }
            5 => {
                let _ = theme_provider.show_help("main_menu");
            }
            6 => {
                if let Some(latest) = &app.latest_version {
                    if latest != env!("CARGO_PKG_VERSION") {
                        let _ = crate::browser::open_url(
                            "https://github.com/mabognar/xpine/releases/latest",
                        );
                    } else {
                        theme_provider.set_status(
                            "xpine is the latest version, nothing to update".to_string(),
                        );
                    }
                } else {
                    theme_provider.set_status("Still checking for updates...".to_string());
                }
            }
            7 => {
                check_and_expunge_outlook(app, session, theme_provider);
                *quit = true;
            }
            _ => {}
        }
    }

    app.mode = next_mode.unwrap_or(AppMode::MainMenu { selected_idx });
}

fn handle_settings_events(k: KeyEvent, app: &mut App, theme_provider: &mut Editor) {
    let mut selected_idx = match std::mem::replace(&mut app.mode, AppMode::EmailList) {
        AppMode::Settings { selected_idx } => selected_idx,
        other => {
            app.mode = other;
            return;
        }
    };

    let mut next_mode = None;

    let action_idx = match k.code {
        KeyCode::Up | KeyCode::Char('p' | 'P') => {
            selected_idx = selected_idx.saturating_sub(1);
            None
        }
        KeyCode::Down | KeyCode::Char('n' | 'N') => {
            selected_idx = (selected_idx + 1).min(4);
            None
        }
        KeyCode::Left | KeyCode::Char('<' | 'm' | 'M') | KeyCode::Esc => {
            next_mode = Some(AppMode::MainMenu { selected_idx: 3 });
            None
        }
        KeyCode::Char('t' | 'T') if k.modifiers.contains(KeyModifiers::ALT) => {
            cycle_theme(theme_provider, app, false);
            None
        }
        KeyCode::Char('x' | 'X') | KeyCode::Right | KeyCode::Enter => Some(selected_idx),
        KeyCode::Char('o' | 'O') => Some(0),
        KeyCode::Char('l' | 'L') => Some(2),
        _ => None,
    };

    if let Some(idx) = action_idx {
        match idx {
            0 => {
                theme_provider.sort_newest_first = !theme_provider.sort_newest_first;
                theme_provider.save_settings();
                reset_view_cache(app);
            }
            1 => {
                theme_provider.spellcheck_before_send = !theme_provider.spellcheck_before_send;
                theme_provider.save_settings();
            }
            2 => {
                theme_provider.show_line_numbers = !theme_provider.show_line_numbers;
                theme_provider.save_settings();
            }
            3 => {
                theme_provider.use_dmy_dates = !theme_provider.use_dmy_dates;
                theme_provider.save_settings();
                reset_view_cache(app);
            }
            4 => {
                let current_sig = crate::config::load_signature();
                let edit_result = theme_provider.edit_buffer(
                    "Edit Email Signature (leave blank for no signature)",
                    &current_sig,
                    MenuState::SignatureEditor,
                );
                let _ = execute!(std::io::stdout(), Clear(ClearType::All));
                if let Ok(Some(new_sig)) = edit_result {
                    crate::config::save_signature(&new_sig);
                }
            }
            _ => {}
        }
    }

    app.mode = next_mode.unwrap_or(AppMode::Settings { selected_idx });
}

fn count_pending_expunge(app: &App) -> usize {
    let page_only_deleted = app
        .page_emails
        .iter()
        .filter(|e| e.is_deleted && !app.graph_pending_deleted.contains(&e.id))
        .count();
    app.graph_pending_deleted.len() + page_only_deleted
}

fn expunge_prompt_text(count: usize) -> String {
    if count == 1 {
        "Expunge 1 message?".to_string()
    } else {
        format!("Expunge {} messages?", count)
    }
}
