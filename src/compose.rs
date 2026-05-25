use crate::config::Account;
use crate::editor::{Editor, EditorResult, MenuState};
use crate::theme::{derive_ui_colors};
use crate::ui::UiExt;
use std::path::Path;
use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::{Message, SmtpTransport, Transport};

use ropey::Rope;
use std::fs;
use std::io::{stdout, Write};
use std::time::Duration;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType, size as term_size},
};

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attachments: Vec<String>,
    active_idx: usize,
}

fn find_suggestion(input: &str, address_book: &[String]) -> Option<String> {
    if input.is_empty() { return None; }

    let last_part = input.split(',').last().unwrap_or("").trim_start();
    if last_part.is_empty() { return None; }

    for addr in address_book {
        if addr.to_lowercase().starts_with(&last_part.to_lowercase()) {
            let remainder = &addr[last_part.len()..];
            // Fix: Only return a suggestion if there is actually text left to autocomplete
            if !remainder.is_empty() {
                return Some(remainder.to_string());
            }
        }
    }
    None
}

// pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {
//
//     let mut is_oauth = false;
//
//     // 1. Determine which credentials to use (OAuth2 vs Password)
//     let creds = if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
//         (&account.client_id, &account.client_secret, &account.refresh_token) {
//
//         let token = match crate::net::get_google_access_token(client_id, client_secret, refresh_token) {
//             Ok(t) => t,
//             Err(e) => return Some(format!("Error: Failed to refresh OAuth token: {}", e)),
//         };
//         is_oauth = true;
//         SmtpCredentials::new(account.email.clone(), token)
//
//     } else if let Some(password) = &account.password {
//
//         // Fallback to standard app passwords
//         SmtpCredentials::new(account.email.clone(), password.clone())
//
//     } else {
//         return Some("Error: No valid authentication method (OAuth2 or Password) provided.".to_string());
//     };
//
//     // 2. Build the transport using the appropriate credentials
//     let mut mailer_builder = SmtpTransport::relay(&account.smtp_server)
//         .unwrap()
//         .credentials(creds);
//
//     // Only force XOAUTH2 if we successfully retrieved an OAuth token
//     if is_oauth {
//         mailer_builder = mailer_builder.authentication(vec![Mechanism::Xoauth2]);
//     }

// pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {
//
//     let mut is_oauth = false;
//     let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");
//
//     // Determine which credentials to use (OAuth2 vs Password)
//     let creds = if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
//         (&account.client_id, &account.client_secret, &account.refresh_token) {
//
//         let token = match crate::net::get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft) {
//             Ok(t) => t,
//             Err(e) => return Some(format!("Error: Failed to refresh OAuth token: {}", e)),
//         };
//         is_oauth = true;
//         SmtpCredentials::new(account.email.clone(), token)
//
//     } else if let Some(password) = &account.password {
//         SmtpCredentials::new(account.email.clone(), password.clone())
//     } else {
//         return Some("Error: No valid authentication method provided.".to_string());
//     };

pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {

    let mut is_oauth = false;
    // Determine if this is a Microsoft account
    let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

    let creds = if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&account.client_id, &account.client_secret, &account.refresh_token) {

        // Pass the is_microsoft flag into our new function
        let token = match crate::net::get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft) {
            Ok(t) => t,
            Err(e) => return Some(format!("Error: Failed to refresh OAuth token: {}", e)),
        };
        is_oauth = true;
        SmtpCredentials::new(account.email.clone(), token)

    } else if let Some(password) = &account.password {
        SmtpCredentials::new(account.email.clone(), password.clone())
    } else {
        return Some("Error: No valid authentication method provided.".to_string());
    };

    // Build the transport
    // let mut mailer_builder = SmtpTransport::relay(&account.smtp_server)
    //     .unwrap()
    //     .credentials(creds);
    //
    // if is_oauth {
    //     mailer_builder = mailer_builder.authentication(vec![Mechanism::Xoauth2]);
    // }

    let mut mailer_builder = SmtpTransport::starttls_relay(&account.smtp_server)
        .unwrap()
        .port(587) // Explicitly set to 587 from your config
        .timeout(Some(std::time::Duration::from_secs(15))) // Prevent infinite hanging!
        .credentials(creds);

    // Only force XOAUTH2 if we successfully retrieved an OAuth token
    if is_oauth {
        mailer_builder = mailer_builder.authentication(vec![Mechanism::Xoauth2]);
    }

    let mailer = mailer_builder.build();

    let mut state = ComposeState {
        to: default_to.unwrap_or("").to_string(),
        cc: String::new(),
        bcc: String::new(),
        subject: default_subject.unwrap_or("").to_string(),
        attachments: Vec::new(),
        active_idx: if default_to.is_some() { 4 } else { 0 },
    };

    let mut editor = Editor::new(None);
    editor.menu_state = MenuState::EmailComposer;
    editor.top_margin = 6;
    editor.current_theme = current_theme.clone();

    if let Some(body) = default_body {
        editor.buffer = Rope::from_str(body);
    }

    let mut stdout = stdout();
    let mut final_body = String::new();
    let mut cancelled = false;

    let address_book = crate::address::load_address_book();

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));
        let theme = &editor.theme_set.themes[&editor.current_theme];
        let colors = derive_ui_colors(theme);

        for i in 0..6 {
            queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(colors.menu_bg), Clear(ClearType::UntilNewLine)).unwrap();
        }

        let header_title = format!("Compose Email ({})", account.email);
        queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title)).unwrap();

        let fields = ["To:", "Cc:", "Bcc:", "Subject:"];
        let vals = [&state.to, &state.cc, &state.bcc, &state.subject];

        for i in 0..4 {
            queue!(
                stdout, cursor::MoveTo(0, (i + 1) as u16),
                SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(format!("{:>8}", fields[i])),
                SetForegroundColor(colors.fg), Print(" "), Print(vals[i])
            ).unwrap();

            if i < 3 && i == state.active_idx {
                if let Some(suggestion) = find_suggestion(vals[i], &address_book) {
                    let dim_c = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
                    queue!(stdout, SetForegroundColor(dim_c), Print(suggestion)).unwrap();
                }
            }
        }

        queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(" Attach: "), SetForegroundColor(colors.fg)).unwrap();

        if state.attachments.is_empty() {
            let dim_c = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
            queue!(stdout, SetForegroundColor(dim_c), Print("(Press ^A to attach a file)")).unwrap(); // Update to ^A
        } else {
            // Apply the viewer's red attachment color
            let att_color = if colors.is_dark {
                Color::Rgb { r: 255, g: 80, b: 80 }
            } else {
                Color::Rgb { r: 220, g: 0, b: 0 }
            };

            let att_names: Vec<String> = state.attachments.iter().enumerate().map(|(i, p)| {
                let file_name = Path::new(p).file_name().unwrap_or_default().to_string_lossy();
                format!("{}. {}", i + 1, file_name)
            }).collect();

            queue!(stdout, SetForegroundColor(att_color), Print(att_names.join("   "))).unwrap();
        }
        queue!(stdout, ResetColor).unwrap();

        editor.draw_screen().unwrap();

        if state.active_idx < 4 {
            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[("^X", " Send"),   (" ^P", " Prev"), ("^A", " Attach"), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();
            Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("Tab", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();

            queue!(stdout, cursor::Show).unwrap();
            let cursor_y = (state.active_idx as u16) + 1;
            let cursor_x = 9 + vals[state.active_idx].chars().count() as u16;
            execute!(stdout, cursor::MoveTo(cursor_x, cursor_y)).unwrap();
        } else {
            queue!(stdout, cursor::Show).unwrap();
        }

        stdout.flush().unwrap();

        let timeout = if let Some(time) = editor.status_time {
            let elapsed = time.elapsed();
            if elapsed >= Duration::from_secs(3) { Duration::from_millis(1) } else { Duration::from_secs(3) - elapsed }
        } else { Duration::from_secs(3600) };

        if event::poll(timeout).unwrap() {
            if let Event::Key(key_event) = event::read().unwrap() {
                if key_event.kind == KeyEventKind::Press {
                    if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                        if key_event.code == KeyCode::Char('x') {
                            final_body = editor.buffer.to_string();
                            break;
                        }
                        if key_event.code == KeyCode::Char('c') {
                            if crate::prompt::prompt_cancel(&mut stdout, &colors) {
                                cancelled = true;
                                break;
                            } else {
                                continue;
                            }
                        }
                        if key_event.code == KeyCode::Char('a') {
                            if let Ok(Some(path)) = editor.run_file_browser(false) {
                                state.attachments.push(path);
                            }
                            continue;
                        }
                    }

                    if state.active_idx == 4 {
                        if key_event.code == KeyCode::Up && editor.cursor_y == 0 {
                            state.active_idx = 3;
                            continue;
                        }

                        match editor.handle_keypress(key_event).unwrap() {
                            EditorResult::Send(content) => { final_body = content; break; }
                            EditorResult::Cancel => {
                                if crate::prompt::prompt_cancel(&mut stdout, &colors) {
                                    cancelled = true;
                                    break;
                                }
                            }
                            EditorResult::Continue => {}
                        }
                    } else {
                        match key_event.code {
                            KeyCode::Char('p') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = state.active_idx.saturating_sub(1),
                            KeyCode::Char('n') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = (state.active_idx + 1).min(4),
                            KeyCode::Up | KeyCode::BackTab => state.active_idx = state.active_idx.saturating_sub(1),
                            KeyCode::Down => { state.active_idx = (state.active_idx + 1).min(4); }
                            KeyCode::Tab | KeyCode::Enter => {
                                if state.active_idx < 3 {
                                    let target = match state.active_idx {
                                        0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, _ => unreachable!()
                                    };

                                    if let Some(suggestion) = find_suggestion(target, &address_book) {
                                        target.push_str(&suggestion);
                                        continue;
                                    }
                                }
                                state.active_idx = (state.active_idx + 1).min(4);
                            }
                            KeyCode::Backspace => {
                                let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
                                target.pop();
                            }
                            KeyCode::Char(c) => {
                                if !key_event.modifiers.contains(KeyModifiers::CONTROL) && !key_event.modifiers.contains(KeyModifiers::ALT) {
                                    let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
                                    target.push(c);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else {
            editor.clear_status();
        }
    }

    *current_theme = editor.current_theme.clone();

    if cancelled { return None; }

    if state.to.trim().is_empty() && state.cc.trim().is_empty() && state.bcc.trim().is_empty() {
        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
        queue!(stdout, Print("No recipients specified. Message cancelled.\r\n\nPress Enter to return...")).unwrap();
        stdout.flush().unwrap();
        while let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } }
        return None;
    }

    let (_, rows) = term_size().unwrap_or((80, 24));
    let theme = &editor.theme_set.themes[&editor.current_theme];
    let colors = derive_ui_colors(theme);
    queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(" Sending message... Please wait "), ResetColor).unwrap();
    stdout.flush().unwrap();

    let mut builder = Message::builder()
        .from(format!("<{}>", account.email).parse().unwrap())
        .subject(state.subject);

    let parse_and_add = |mut b: lettre::message::MessageBuilder, input: &str, field_type: &str| -> lettre::message::MessageBuilder {
        for addr in input.split(',') {
            // Strip whitespace AND the trailing semicolon, then convert to an owned String
            let mut trimmed = addr.trim().trim_end_matches(';').to_string();

            if trimmed.eq_ignore_ascii_case(&account.email) {
                if let Some((user, domain)) = trimmed.split_once('@') {
                    // Only append if they haven't already explicitly used an alias
                    if !user.contains('+') {
                        trimmed = format!("{}+me@{}", user, domain);
                    }
                }
            }

            if !trimmed.is_empty() {
                if let Ok(mailbox) = trimmed.parse::<lettre::message::Mailbox>() {
                    b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, };
                } else if let Ok(mailbox) = format!("<{}>", trimmed).parse::<lettre::message::Mailbox>() {
                    b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, };
                }
            }
        }
        b
    };

    let final_to = crate::address::expand_address_lists(&state.to, &address_book);
    let final_cc = crate::address::expand_address_lists(&state.cc, &address_book);
    let final_bcc = crate::address::expand_address_lists(&state.bcc, &address_book);

    builder = parse_and_add(builder, &final_to, "to");
    builder = parse_and_add(builder, &final_cc, "cc");
    builder = parse_and_add(builder, &final_bcc, "bcc");

    let formatted_body = crate::editor::Editor::justify_all_text(&final_body);

    let mut multipart = lettre::message::MultiPart::mixed()
        .singlepart(lettre::message::SinglePart::plain(formatted_body));

    for att in &state.attachments {
        if let Ok(file_data) = fs::read(att) {
            let file_name = Path::new(att).file_name().unwrap_or_default().to_string_lossy().into_owned();
            let ext = Path::new(att).extension().unwrap_or_default().to_string_lossy().to_lowercase();
            let mime_str = match ext.as_str() {
                "txt" | "rs" | "c" | "cpp" | "md" | "toml" | "json" => "text/plain",
                "html" | "htm" => "text/html",
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "pdf" => "application/pdf",
                "zip" => "application/zip",
                "csv" => "text/csv",
                _ => "application/octet-stream",
            };
            if let Ok(content_type) = mime_str.parse::<lettre::message::header::ContentType>() {
                let attachment = lettre::message::Attachment::new(file_name).body(file_data, content_type);
                multipart = multipart.singlepart(attachment);
            }
        }
    }

    match builder.multipart(multipart) {
        Ok(email_msg) => {
            // The `mailer` was already built dynamically at the top of the function,
            // so we can directly send the message without fetching a new token.
            match mailer.send(&email_msg) {
                Ok(_) => Some("Message Sent".to_string()),
                Err(e) => {
                    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                    queue!(stdout, Print(format!("-> Failed to send message: {:?}\r\n", e))).unwrap();
                    queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
                    stdout.flush().unwrap();
                    loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                    None
                }
            }
        }
        Err(e) => {
            // ... error handling remains unchanged
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap();
            queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
            stdout.flush().unwrap();
            loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
            None
        }
    }
}

