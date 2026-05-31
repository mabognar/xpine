use crate::config::Account;
use crate::editor::{Editor, EditorResult, MenuState};
use crate::theme::{derive_ui_colors};
use crate::ui::UiExt;
use std::path::Path;
use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::{Message, SmtpTransport, Transport};
use lettre::message::Mailbox;
use std::str::FromStr;

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
            if !remainder.is_empty() { return Some(remainder.to_string()); }
        }
    }
    None
}

pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {
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

    if let Some(body) = default_body { editor.buffer = Rope::from_str(body); }

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
            queue!(stdout, SetForegroundColor(dim_c), Print("(Press ^A to attach a file)")).unwrap();
        } else {
            let att_color = if colors.is_dark { Color::Rgb { r: 255, g: 80, b: 80 } } else { Color::Rgb { r: 220, g: 0, b: 0 } };
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
                        if key_event.code == KeyCode::Char('x') { final_body = editor.buffer.to_string(); break; }
                        if key_event.code == KeyCode::Char('c') {
                            if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } else { continue; }
                        }
                        if key_event.code == KeyCode::Char('a') {
                            if let Ok(Some(path)) = editor.run_file_browser(false) { state.attachments.push(path); }
                            continue;
                        }
                    }

                    if state.active_idx == 4 {
                        if key_event.code == KeyCode::Up && editor.cursor_y == 0 { state.active_idx = 3; continue; }
                        match editor.handle_keypress(key_event).unwrap() {
                            EditorResult::Send(content) => { final_body = content; break; }
                            EditorResult::Cancel => { if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } }
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
                                    let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, _ => unreachable!() };
                                    if let Some(suggestion) = find_suggestion(target, &address_book) { target.push_str(&suggestion); continue; }
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
        } else { editor.clear_status(); }
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
            let mut trimmed = addr.trim().trim_end_matches(';').to_string();
            if trimmed.eq_ignore_ascii_case(&account.email) {
                if let Some((user, domain)) = trimmed.split_once('@') { if !user.contains('+') { trimmed = format!("{}+me@{}", user, domain); } }
            }
            if !trimmed.is_empty() {
                if let Ok(mailbox) = trimmed.parse::<lettre::message::Mailbox>() { b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, }; }
                else if let Ok(mailbox) = format!("<{}>", trimmed).parse::<lettre::message::Mailbox>() { b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, }; }
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
    let mut multipart = lettre::message::MultiPart::mixed().singlepart(lettre::message::SinglePart::plain(formatted_body));

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
            let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

            let token_or_pass = if let Some(ref rt) = account.refresh_token {
                // let target_scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.Send") } else { None };
                let target_scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send offline_access") } else { None };
                
                match crate::net::get_oauth_access_token(
                    account.client_id.as_deref().unwrap_or(""),
                    account.client_secret.as_deref().unwrap_or(""),
                    rt,
                    is_microsoft,
                    target_scope
                ) {
                    Ok(token) => token,
                    Err(_) if is_microsoft => {
                        // INCREMENTAL CONSENT TRIGGER
                        // The refresh failed because the user hasn't consented to Mail.Send yet.
                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                        queue!(stdout, Print("Microsoft requires separate authorization to send emails.\r\n")).unwrap();
                        queue!(stdout, Print("Initiating a one-time sending authorization...\r\n\n")).unwrap();
                        stdout.flush().unwrap();

                        // 1. Run Device Flow requesting BOTH read and send scopes
                        let client = reqwest::blocking::Client::new();
                        let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
                        let client_id = account.client_id.as_deref().unwrap_or("");
                        let params = vec![
                            ("client_id", client_id),
                            ("scope", "offline_access https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send"),
                        ];

                        if let Ok(res) = client.post(endpoint).form(&params).send() {
                            if res.status().is_success() {
                                if let Ok(auth_res) = res.json::<crate::net::DeviceCodeResponse>() {
                                    queue!(stdout, Print(format!("   To authorize sending, please visit: {}\r\n", auth_res.verification_url))).unwrap();
                                    queue!(stdout, Print("   And enter the following code:            ")).unwrap();
                                    queue!(stdout, SetForegroundColor(Color::Red), Print(format!("{}\r\n\r\n", auth_res.user_code)), ResetColor).unwrap();
                                    queue!(stdout, Print("   Waiting for authorization (check your browser)...\r\n")).unwrap();
                                    stdout.flush().unwrap();

                                    if let Ok(token_res) = crate::net::poll_microsoft_token(client_id, account.client_secret.as_deref().unwrap_or(""), &auth_res.device_code, auth_res.interval) {
                                        if let Some(refresh) = token_res.refresh_token {
                                            // 2. Save the NEW multi-resource refresh_token to config
                                            let mut config_accounts = crate::config::load_config().accounts;
                                            if let Some(acc) = config_accounts.iter_mut().find(|a| a.email == account.email) {
                                                acc.refresh_token = Some(refresh);
                                                crate::config::save_config(&config_accounts);
                                            }
                                        }

                                        execute!(stdout, SetForegroundColor(Color::Green), Print("\r\n   Authorization successful! Returning to send message...\r\n"), ResetColor).unwrap();
                                        stdout.flush().unwrap();
                                        std::thread::sleep(Duration::from_millis(1500));

                                        // 3. Return the new access_token so the email seamlessly sends
                                        token_res.access_token
                                    } else {
                                        return None;
                                    }
                                } else {
                                    return None;
                                }
                            } else {
                                queue!(stdout, Print(format!("\r\n   Device code request failed: {}\r\n", res.text().unwrap_or_default()))).unwrap();
                                queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
                                stdout.flush().unwrap();
                                loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }
                    Err(e) => {
                        // Handle standard Google/Network errors
                        return None;
                    }
                }
            } else {
                account.password.clone().unwrap_or_default()
            };

            if is_microsoft && account.refresh_token.is_some() {
                // MICROSOFT GRAPH API BYPASS
                // Microsoft blocks standard SMTP Client Submission on many personal accounts.
                // We must submit the raw MIME as a base64 encoded text/plain string to the Graph API.

                use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
                let email_bytes = email_msg.formatted();
                let base64_content = base64_engine.encode(&email_bytes);

                // Assuming you have reqwest or ureq available in your dependencies
                let client = reqwest::blocking::Client::new();
                let res = client.post("https://graph.microsoft.com/v1.0/me/sendMail")
                    .header("Authorization", format!("Bearer {}", token_or_pass))
                    .header("Content-Type", "text/plain")
                    .body(base64_content)
                    .send();

                match res {
                    Ok(r) if r.status().is_success() => Some("Message Sent via Graph API".to_string()),
                    Ok(r) => {
                        let status = r.status();
                        let text = r.text().unwrap_or_default();
                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                        queue!(stdout, Print(format!("-> Graph API Error: {} - {}\r\n", status, text))).unwrap();
                        queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
                        stdout.flush().unwrap();
                        loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                        None
                    }
                    Err(e) => {
                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                        queue!(stdout, Print(format!("-> Network Error: {:?}\r\n", e))).unwrap();
                        queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
                        stdout.flush().unwrap();
                        loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                        None
                    }
                }
            } else {
                // STANDARD LETTRE SMTP (Used for Google / Custom SMTP / Enabled Enterprise Accounts)
                let creds = SmtpCredentials::new(account.email.clone(), token_or_pass);
                let mut mailer = SmtpTransport::starttls_relay(&account.smtp_server)
                    .unwrap()
                    .port(587)
                    .credentials(creds);

                if account.refresh_token.is_some() {
                    mailer = mailer.authentication(vec![Mechanism::Xoauth2]);
                }

                match mailer.build().send(&email_msg) {
                    Ok(_) => Some("Message Sent".to_string()),
                    Err(e) => {
                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                        queue!(stdout, Print(format!("-> Failed to send message: {:?}\r\n", e))).unwrap();
                        queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
                        stdout.flush().unwrap();
                        loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                        None
                    }
                }
            }
        }
        Err(e) => {
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap();
            queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
            stdout.flush().unwrap();
            loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
            None
        }
    }
}

// use crate::config::Account;
// use crate::editor::{Editor, EditorResult, MenuState};
// use crate::theme::{derive_ui_colors};
// use crate::ui::UiExt;
// use std::path::Path;
// use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
// use lettre::{Message, SmtpTransport, Transport};
// use lettre::message::Mailbox;
// use std::str::FromStr;
//
// use ropey::Rope;
// use std::fs;
// use std::io::{stdout, Write};
// use std::time::Duration;
// use crossterm::{
//     cursor,
//     event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
//     execute, queue,
//     style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
//     terminal::{Clear, ClearType, size as term_size},
// };
//
// struct ComposeState {
//     to: String,
//     cc: String,
//     bcc: String,
//     subject: String,
//     attachments: Vec<String>,
//     active_idx: usize,
// }
//
// fn find_suggestion(input: &str, address_book: &[String]) -> Option<String> {
//     if input.is_empty() { return None; }
//     let last_part = input.split(',').last().unwrap_or("").trim_start();
//     if last_part.is_empty() { return None; }
//     for addr in address_book {
//         if addr.to_lowercase().starts_with(&last_part.to_lowercase()) {
//             let remainder = &addr[last_part.len()..];
//             if !remainder.is_empty() { return Some(remainder.to_string()); }
//         }
//     }
//     None
// }
//
// pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {
//     let mut state = ComposeState {
//         to: default_to.unwrap_or("").to_string(),
//         cc: String::new(),
//         bcc: String::new(),
//         subject: default_subject.unwrap_or("").to_string(),
//         attachments: Vec::new(),
//         active_idx: if default_to.is_some() { 4 } else { 0 },
//     };
//
//     let mut editor = Editor::new(None);
//     editor.menu_state = MenuState::EmailComposer;
//     editor.top_margin = 6;
//     editor.current_theme = current_theme.clone();
//
//     if let Some(body) = default_body { editor.buffer = Rope::from_str(body); }
//
//     let mut stdout = stdout();
//     let mut final_body = String::new();
//     let mut cancelled = false;
//     let address_book = crate::address::load_address_book();
//
//     loop {
//         let (cols, rows) = term_size().unwrap_or((80, 24));
//         let theme = &editor.theme_set.themes[&editor.current_theme];
//         let colors = derive_ui_colors(theme);
//
//         for i in 0..6 {
//             queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(colors.menu_bg), Clear(ClearType::UntilNewLine)).unwrap();
//         }
//
//         let header_title = format!("Compose Email ({})", account.email);
//         queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title)).unwrap();
//
//         let fields = ["To:", "Cc:", "Bcc:", "Subject:"];
//         let vals = [&state.to, &state.cc, &state.bcc, &state.subject];
//
//         for i in 0..4 {
//             queue!(
//                 stdout, cursor::MoveTo(0, (i + 1) as u16),
//                 SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(format!("{:>8}", fields[i])),
//                 SetForegroundColor(colors.fg), Print(" "), Print(vals[i])
//             ).unwrap();
//
//             if i < 3 && i == state.active_idx {
//                 if let Some(suggestion) = find_suggestion(vals[i], &address_book) {
//                     let dim_c = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
//                     queue!(stdout, SetForegroundColor(dim_c), Print(suggestion)).unwrap();
//                 }
//             }
//         }
//
//         queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(" Attach: "), SetForegroundColor(colors.fg)).unwrap();
//
//         if state.attachments.is_empty() {
//             let dim_c = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
//             queue!(stdout, SetForegroundColor(dim_c), Print("(Press ^A to attach a file)")).unwrap();
//         } else {
//             let att_color = if colors.is_dark { Color::Rgb { r: 255, g: 80, b: 80 } } else { Color::Rgb { r: 220, g: 0, b: 0 } };
//             let att_names: Vec<String> = state.attachments.iter().enumerate().map(|(i, p)| {
//                 let file_name = Path::new(p).file_name().unwrap_or_default().to_string_lossy();
//                 format!("{}. {}", i + 1, file_name)
//             }).collect();
//             queue!(stdout, SetForegroundColor(att_color), Print(att_names.join("   "))).unwrap();
//         }
//         queue!(stdout, ResetColor).unwrap();
//
//         editor.draw_screen().unwrap();
//
//         if state.active_idx < 4 {
//             let m_col = (cols as usize / 6).max(1);
//             Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[("^X", " Send"),   (" ^P", " Prev"), ("^A", " Attach"), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();
//             Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("Tab", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();
//             queue!(stdout, cursor::Show).unwrap();
//             let cursor_y = (state.active_idx as u16) + 1;
//             let cursor_x = 9 + vals[state.active_idx].chars().count() as u16;
//             execute!(stdout, cursor::MoveTo(cursor_x, cursor_y)).unwrap();
//         } else {
//             queue!(stdout, cursor::Show).unwrap();
//         }
//         stdout.flush().unwrap();
//
//         let timeout = if let Some(time) = editor.status_time {
//             let elapsed = time.elapsed();
//             if elapsed >= Duration::from_secs(3) { Duration::from_millis(1) } else { Duration::from_secs(3) - elapsed }
//         } else { Duration::from_secs(3600) };
//
//         if event::poll(timeout).unwrap() {
//             if let Event::Key(key_event) = event::read().unwrap() {
//                 if key_event.kind == KeyEventKind::Press {
//                     if key_event.modifiers.contains(KeyModifiers::CONTROL) {
//                         if key_event.code == KeyCode::Char('x') { final_body = editor.buffer.to_string(); break; }
//                         if key_event.code == KeyCode::Char('c') {
//                             if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } else { continue; }
//                         }
//                         if key_event.code == KeyCode::Char('a') {
//                             if let Ok(Some(path)) = editor.run_file_browser(false) { state.attachments.push(path); }
//                             continue;
//                         }
//                     }
//
//                     if state.active_idx == 4 {
//                         if key_event.code == KeyCode::Up && editor.cursor_y == 0 { state.active_idx = 3; continue; }
//                         match editor.handle_keypress(key_event).unwrap() {
//                             EditorResult::Send(content) => { final_body = content; break; }
//                             EditorResult::Cancel => { if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } }
//                             EditorResult::Continue => {}
//                         }
//                     } else {
//                         match key_event.code {
//                             KeyCode::Char('p') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = state.active_idx.saturating_sub(1),
//                             KeyCode::Char('n') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = (state.active_idx + 1).min(4),
//                             KeyCode::Up | KeyCode::BackTab => state.active_idx = state.active_idx.saturating_sub(1),
//                             KeyCode::Down => { state.active_idx = (state.active_idx + 1).min(4); }
//                             KeyCode::Tab | KeyCode::Enter => {
//                                 if state.active_idx < 3 {
//                                     let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, _ => unreachable!() };
//                                     if let Some(suggestion) = find_suggestion(target, &address_book) { target.push_str(&suggestion); continue; }
//                                 }
//                                 state.active_idx = (state.active_idx + 1).min(4);
//                             }
//                             KeyCode::Backspace => {
//                                 let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
//                                 target.pop();
//                             }
//                             KeyCode::Char(c) => {
//                                 if !key_event.modifiers.contains(KeyModifiers::CONTROL) && !key_event.modifiers.contains(KeyModifiers::ALT) {
//                                     let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
//                                     target.push(c);
//                                 }
//                             }
//                             _ => {}
//                         }
//                     }
//                 }
//             }
//         } else { editor.clear_status(); }
//     }
//
//     *current_theme = editor.current_theme.clone();
//     if cancelled { return None; }
//
//     if state.to.trim().is_empty() && state.cc.trim().is_empty() && state.bcc.trim().is_empty() {
//         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//         queue!(stdout, Print("No recipients specified. Message cancelled.\r\n\nPress Enter to return...")).unwrap();
//         stdout.flush().unwrap();
//         while let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } }
//         return None;
//     }
//
//     let (_, rows) = term_size().unwrap_or((80, 24));
//     let theme = &editor.theme_set.themes[&editor.current_theme];
//     let colors = derive_ui_colors(theme);
//     queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(" Sending message... Please wait "), ResetColor).unwrap();
//     stdout.flush().unwrap();
//
//     let mut builder = Message::builder()
//         .from(format!("<{}>", account.email).parse().unwrap())
//         .subject(state.subject);
//
//     let parse_and_add = |mut b: lettre::message::MessageBuilder, input: &str, field_type: &str| -> lettre::message::MessageBuilder {
//         for addr in input.split(',') {
//             let mut trimmed = addr.trim().trim_end_matches(';').to_string();
//             if trimmed.eq_ignore_ascii_case(&account.email) {
//                 if let Some((user, domain)) = trimmed.split_once('@') { if !user.contains('+') { trimmed = format!("{}+me@{}", user, domain); } }
//             }
//             if !trimmed.is_empty() {
//                 if let Ok(mailbox) = trimmed.parse::<lettre::message::Mailbox>() { b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, }; }
//                 else if let Ok(mailbox) = format!("<{}>", trimmed).parse::<lettre::message::Mailbox>() { b = match field_type { "to" => b.to(mailbox), "cc" => b.cc(mailbox), "bcc" => b.bcc(mailbox), _ => b, }; }
//             }
//         }
//         b
//     };
//
//     let final_to = crate::address::expand_address_lists(&state.to, &address_book);
//     let final_cc = crate::address::expand_address_lists(&state.cc, &address_book);
//     let final_bcc = crate::address::expand_address_lists(&state.bcc, &address_book);
//
//     builder = parse_and_add(builder, &final_to, "to");
//     builder = parse_and_add(builder, &final_cc, "cc");
//     builder = parse_and_add(builder, &final_bcc, "bcc");
//
//     let formatted_body = crate::editor::Editor::justify_all_text(&final_body);
//     let mut multipart = lettre::message::MultiPart::mixed().singlepart(lettre::message::SinglePart::plain(formatted_body));
//
//     for att in &state.attachments {
//         if let Ok(file_data) = fs::read(att) {
//             let file_name = Path::new(att).file_name().unwrap_or_default().to_string_lossy().into_owned();
//             let ext = Path::new(att).extension().unwrap_or_default().to_string_lossy().to_lowercase();
//             let mime_str = match ext.as_str() {
//                 "txt" | "rs" | "c" | "cpp" | "md" | "toml" | "json" => "text/plain",
//                 "html" | "htm" => "text/html",
//                 "jpg" | "jpeg" => "image/jpeg",
//                 "png" => "image/png",
//                 "pdf" => "application/pdf",
//                 "zip" => "application/zip",
//                 "csv" => "text/csv",
//                 _ => "application/octet-stream",
//             };
//             if let Ok(content_type) = mime_str.parse::<lettre::message::header::ContentType>() {
//                 let attachment = lettre::message::Attachment::new(file_name).body(file_data, content_type);
//                 multipart = multipart.singlepart(attachment);
//             }
//         }
//     }
//
//     match builder.multipart(multipart) {
//         Ok(email_msg) => {
//             // ... inside the Ok(email_msg) block ...
//
//             let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");
//
//             let target_scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.Send") } else { None };
//
//             let token_or_pass = if let Some(ref rt) = account.refresh_token {
//                 let target_scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.Send") } else { None };
//
//                 match crate::net::get_oauth_access_token(
//                     account.client_id.as_deref().unwrap_or(""),
//                     account.client_secret.as_deref().unwrap_or(""),
//                     rt,
//                     is_microsoft,
//                     target_scope
//                 ) {
//                     Ok(token) => token,
//                     Err(_) if is_microsoft => {
//                         // INCREMENTAL CONSENT TRIGGER
//                         // The refresh failed because the user hasn't consented to Mail.Send yet.
//                         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//                         queue!(stdout, Print("Microsoft requires separate authorization to send emails.\r\n")).unwrap();
//                         queue!(stdout, Print("Initiating a one-time sending authorization...\r\n\n")).unwrap();
//                         stdout.flush().unwrap();
//
//                         // TODO: Call your device code polling logic here, but explicitly request:
//                         // "offline_access https://graph.microsoft.com/Mail.Send"
//
//                         // 1. Run Device Flow for the Graph scope
//                         // 2. Save the NEW refresh_token to your xpinerc TOML file
//                         // 3. Return the new access_token so the email can send
//
//                         queue!(stdout, Print("\r\nPress Enter to return and complete authorization...")).unwrap();
//                         stdout.flush().unwrap();
//                         loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
//
//                         return None;
//                     }
//                     Err(e) => {
//                         // Handle standard Google/Network errors...
//                         return None;
//                     }
//                 }
//             } else {
//                 account.password.clone().unwrap_or_default()
//             };
//
//             // let token_or_pass = if let Some(ref rt) = account.refresh_token {
//             //     crate::net::get_oauth_access_token(
//             //         account.client_id.as_deref().unwrap_or(""),
//             //         account.client_secret.as_deref().unwrap_or(""),
//             //         rt,
//             //         is_microsoft,
//             //         target_scope
//             //     ).unwrap_or_default()
//             // } else {
//             //     account.password.clone().unwrap_or_default()
//             // };
//
//             // let token_or_pass = if let Some(ref rt) = account.refresh_token {
//             //     crate::net::get_oauth_access_token(
//             //         account.client_id.as_deref().unwrap_or(""),
//             //         account.client_secret.as_deref().unwrap_or(""),
//             //         rt,
//             //         is_microsoft,
//             //         None
//             //     ).unwrap_or_default()
//             // } else {
//             //     account.password.clone().unwrap_or_default()
//             // };
//
//             if is_microsoft && account.refresh_token.is_some() {
//                 // MICROSOFT GRAPH API BYPASS
//                 // Microsoft blocks standard SMTP Client Submission on many personal accounts.
//                 // We must submit the raw MIME as a base64 encoded text/plain string to the Graph API.
//
//                 use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
//                 let email_bytes = email_msg.formatted();
//                 let base64_content = base64_engine.encode(&email_bytes);
//
//                 // Assuming you have reqwest or ureq available in your dependencies
//                 let client = reqwest::blocking::Client::new();
//                 let res = client.post("https://graph.microsoft.com/v1.0/me/sendMail")
//                     .header("Authorization", format!("Bearer {}", token_or_pass))
//                     .header("Content-Type", "text/plain")
//                     .body(base64_content)
//                     .send();
//
//                 match res {
//                     Ok(r) if r.status().is_success() => Some("Message Sent via Graph API".to_string()),
//                     Ok(r) => {
//                         let status = r.status();
//                         let text = r.text().unwrap_or_default();
//                         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//                         queue!(stdout, Print(format!("-> Graph API Error: {} - {}\r\n", status, text))).unwrap();
//                         queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
//                         stdout.flush().unwrap();
//                         loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
//                         None
//                     }
//                     Err(e) => {
//                         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//                         queue!(stdout, Print(format!("-> Network Error: {:?}\r\n", e))).unwrap();
//                         queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
//                         stdout.flush().unwrap();
//                         loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
//                         None
//                     }
//                 }
//             } else {
//                 // STANDARD LETTRE SMTP (Used for Google / Custom SMTP / Enabled Enterprise Accounts)
//                 let creds = SmtpCredentials::new(account.email.clone(), token_or_pass);
//                 let mut mailer = SmtpTransport::starttls_relay(&account.smtp_server)
//                     .unwrap()
//                     .port(587)
//                     .credentials(creds);
//
//                 if account.refresh_token.is_some() {
//                     mailer = mailer.authentication(vec![Mechanism::Xoauth2]);
//                 }
//
//                 match mailer.build().send(&email_msg) {
//                     Ok(_) => Some("Message Sent".to_string()),
//                     Err(e) => {
//                         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//                         queue!(stdout, Print(format!("-> Failed to send message: {:?}\r\n", e))).unwrap();
//                         queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
//                         stdout.flush().unwrap();
//                         loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
//                         None
//                     }
//                 }
//             }
//         }
//         Err(e) => {
//             execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//             queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap();
//             queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
//             stdout.flush().unwrap();
//             loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
//             None
//         }
//     }
// }
//
