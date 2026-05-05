mod config;
mod editor;
mod spell;
mod ui;

use crate::editor::{Editor, MenuState, EditorResult};
use crate::ui::UiExt;

use lettre::{Message, SmtpTransport, Transport};
use lettre::transport::smtp::authentication::Credentials as SmtpCredentials;
use ropey::Rope;
use std::io::{stdout, Write};
use std::fs;
use chrono::{DateTime, Local, Utc};
use crossterm::{cursor, event::{self, Event, KeyCode, KeyEventKind, KeyModifiers}, execute, queue, style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor}, terminal, terminal::{Clear, ClearType, size as term_size}};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use native_tls::TlsConnector;

struct EmailMeta {
    id: u32,
    subject: String,
    from: String,
    reply_to: String,
    reply_to_display: String,
    to_addr: String,
    cc: String,
    date: String,
    size: u32,
    is_read: bool,
    is_deleted: bool,
    is_flagged: bool,
}

enum AppMode {
    List,
    Reading {
        text_body: String,
        html_body: Option<String>,
        attachments: Vec<(String, Vec<u8>)>,
    },
}

#[derive(Clone)]
struct Account {
    email: String,
    password: String,
}

struct AppConfig {
    accounts: Vec<Account>,
}

fn load_config() -> AppConfig {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_dir = home.join(".email");
    let config_path = config_dir.join(".emailrc");

    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .email directory.");
        let template = "# Account 1\nEMAIL=statgod@gmail.com\nPASSWORD=your_16_char_app_password\n\n# Account 2\nEMAIL=second@gmail.com\nPASSWORD=app_password\n";
        fs::write(&config_path, template).expect("Failed to write .emailrc template.");

        println!("No configuration found.");
        println!("Created a new config template at: {:?}", config_path);
        println!("Please edit this file with your actual App Password(s) and run the program again.");
        std::process::exit(0);
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read .emailrc");
    let mut accounts = Vec::new();
    let mut current_email = String::new();

    for line in contents.lines() {
        if line.trim().is_empty() || line.starts_with('#') { continue; }
        if let Some((key, value)) = line.split_once('=') {
            let val = value.trim().to_string();
            match key.trim().to_uppercase().as_str() {
                "EMAIL" => current_email = val,
                "PASSWORD" => {
                    if !current_email.is_empty() && !val.is_empty() {
                        accounts.push(Account { email: current_email.clone(), password: val });
                        current_email.clear();
                    }
                }
                _ => {}
            }
        }
    }

    if accounts.is_empty() || accounts[0].password == "your_16_char_app_password" {
        println!("Invalid or default credentials found in {:?}", config_path);
        std::process::exit(1);
    }

    AppConfig { accounts }
}

fn file_browser(stdout: &mut std::io::Stdout, rows: u16, cols: u16) -> Option<String> {
    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let mut selected_idx = 0;

    loop {
        let mut entries = vec![];
        if current_dir.parent().is_some() {
            entries.push(("..".to_string(), current_dir.parent().unwrap().to_path_buf(), true));
        }

        if let Ok(read_dir) = std::fs::read_dir(&current_dir) {
            let mut dirs = vec![];
            let mut files = vec![];
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() { dirs.push((name, path, true)); }
                else { files.push((name, path, false)); }
            }
            dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            entries.extend(dirs);
            entries.extend(files);
        }

        if selected_idx >= entries.len() {
            selected_idx = entries.len().saturating_sub(1);
        }

        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        let ui_bg = Color::Rgb { r: 20, g: 20, b: 20 };
        let title = format!(" --- Browse to Attach: {} ", current_dir.display());
        let pad_len = (cols as usize).saturating_sub(title.chars().count());

        queue!(
            stdout, SetBackgroundColor(ui_bg), SetForegroundColor(Color::White),
            Print(title), Print(" ".repeat(pad_len)), ResetColor
        ).unwrap();

        let items_per_page = (rows.saturating_sub(5)) as usize;
        let start_idx = if selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

        for i in 0..items_per_page {
            let actual_idx = start_idx + i;
            if actual_idx < entries.len() {
                let entry = &entries[actual_idx];
                let prefix = if entry.2 { "[DIR]  " } else { "       " };
                let mut display_str = format!("{}{}", prefix, entry.0);
                if display_str.chars().count() > (cols as usize) {
                    display_str = display_str.chars().take((cols as usize).saturating_sub(3)).collect::<String>();
                }

                execute!(stdout, cursor::MoveTo(0, (i + 2) as u16)).unwrap();
                if actual_idx == selected_idx {
                    queue!(stdout, SetBackgroundColor(Color::Rgb { r: 50, g: 50, b: 50 }), Print(display_str), ResetColor).unwrap();
                } else {
                    let fg = if entry.2 { Color::Cyan } else { Color::Reset };
                    queue!(stdout, SetForegroundColor(fg), Print(display_str), ResetColor).unwrap();
                }
            }
        }

        let m_col = ((cols as usize) / 6).max(1);
        Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("Up/Dn", " Nav"), ("Enter", " Select"), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, Color::Cyan, Color::Reset).unwrap();
        Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], ui_bg, Color::Cyan, Color::Reset).unwrap();
        // Editor::draw_menu_line(stdout, rows - 2, cols, (cols / 6) as usize, &[(" Up/Dn ", "Nav"), (" Enter ", "Select")], ui_bg, Color::Cyan, Color::Reset).unwrap();
        // Editor::draw_menu_line(stdout, rows - 1, cols, (cols / 6) as usize, &[(" ^C ", "Cancel")], ui_bg, Color::Cyan, Color::Reset).unwrap();
        stdout.flush().unwrap();

        if let Event::Key(k) = event::read().unwrap() {
            if k.kind == KeyEventKind::Press {
                match k.code {
                    KeyCode::Up => selected_idx = selected_idx.saturating_sub(1),
                    KeyCode::Down => if selected_idx + 1 < entries.len() { selected_idx += 1 },
                    KeyCode::Enter => {
                        if !entries.is_empty() {
                            let selected = &entries[selected_idx];
                            if selected.2 {
                                current_dir = selected.1.clone();
                                selected_idx = 0;
                            } else {
                                return Some(selected.1.to_string_lossy().into_owned());
                            }
                        }
                    }
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return None,
                    KeyCode::Esc => return None,
                    _ => {}
                }
            }
        }
    }
}

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attachments: Vec<String>,
    active_idx: usize,
}

// Rewritten to use xnano for the body
pub fn compose_email(account: &Account, default_to: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>) {
    let mut state = ComposeState {
        to: default_to.unwrap_or("").to_string(),
        cc: String::new(),
        bcc: String::new(),
        subject: default_subject.unwrap_or("").to_string(),
        attachments: Vec::new(),
        // Drops the cursor directly into the body (4) if 'default_to' is populated (Reply Mode)
        active_idx: if default_to.is_some() { 4 } else { 0 },
    };

    let mut editor = Editor::new(None);
    editor.menu_state = MenuState::EmailComposer;
    editor.top_margin = 6;

    if let Some(body) = default_body {
        editor.buffer = Rope::from_str(body);
    }

    let mut stdout = stdout();
    let mut final_body = String::new();
    let mut cancelled = false;

    // --- UNIFIED DRAWING & INPUT LOOP ---
    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));

        let theme = &editor.theme_set.themes[&editor.current_theme];
        let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
        let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });

        let header_bg = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
        let text_fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
        let label_color = Color::Rgb { r: 50, g: 150, b: 200 };

        for i in 0..6 {
            queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(header_bg), terminal::Clear(ClearType::UntilNewLine)).unwrap();
        }

        let header_title = format!("Compose Email ({})", account.email);
        queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(label_color), Print(header_title), ResetColor).unwrap();

        let fields = ["To:", "Cc:", "Bcc:", "Subject:"];
        let vals = [&state.to, &state.cc, &state.bcc, &state.subject];

        for i in 0..4 {
            queue!(
                stdout, cursor::MoveTo(0, (i + 1) as u16),
                SetBackgroundColor(header_bg), SetForegroundColor(label_color), Print(format!("{:>8}", fields[i])),
                SetForegroundColor(text_fg), Print(" "), Print(vals[i]), ResetColor
            ).unwrap();
        }

        queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(header_bg), SetForegroundColor(label_color), Print(" Attach: "), SetForegroundColor(text_fg)).unwrap();
        if state.attachments.is_empty() {
            queue!(stdout, SetForegroundColor(Color::DarkGrey), Print("(Press ^T to attach a file)")).unwrap();
        } else {
            let att_names: Vec<&str> = state.attachments.iter().map(|p| std::path::Path::new(p).file_name().unwrap_or_default().to_str().unwrap_or_default()).collect();
            queue!(stdout, Print(att_names.join(", "))).unwrap();
        }

        editor.draw_screen().unwrap();

        if state.active_idx < 4 {
            let m_col = ((cols as usize) / 6).max(1);
            Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[("^P", " Prev"), ("Tab", " Next"), ("^T", " Attach"), ("", ""), ("", ""), ("", "")], header_bg, Color::Cyan, text_fg).unwrap();
            Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("Enter", " Body"), ("^X", " Send"), ("", ""), ("", ""), ("", "")], header_bg, Color::Cyan, text_fg).unwrap();

            queue!(stdout, cursor::Show).unwrap();
            let cursor_y = (state.active_idx as u16) + 1;
            let cursor_x = 9 + vals[state.active_idx].chars().count() as u16;
            execute!(stdout, cursor::MoveTo(cursor_x, cursor_y)).unwrap();
        } else {
            queue!(stdout, cursor::Show).unwrap();
        }

        stdout.flush().unwrap();

        if let Event::Key(key_event) = event::read().unwrap() {
            if key_event.kind == KeyEventKind::Press {

                // GLOBAL OVERRIDES: Allows sending (^X) or canceling (^C) from anywhere!
                if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                    if key_event.code == KeyCode::Char('x') {
                        final_body = editor.buffer.to_string();
                        break;
                    }
                    if key_event.code == KeyCode::Char('c') {
                        cancelled = true;
                        break;
                    }
                }

                if state.active_idx == 4 {
                    // Allows moving back up to headers from the top line of the body
                    if key_event.code == KeyCode::Up && editor.cursor_y == 0 {
                        state.active_idx = 3;
                        continue;
                    }

                    match editor.handle_keypress(key_event).unwrap() {
                        EditorResult::Send(content) => { final_body = content; break; }
                        EditorResult::Cancel => { cancelled = true; break; }
                        EditorResult::Continue => {}
                    }
                } else {
                    match key_event.code {
                        KeyCode::Char('t') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(path) = file_browser(&mut stdout, rows, cols) { state.attachments.push(path); }
                        }
                        KeyCode::Char('p') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = state.active_idx.saturating_sub(1),
                        KeyCode::Char('n') if key_event.modifiers.contains(KeyModifiers::CONTROL) => state.active_idx = (state.active_idx + 1).min(4),
                        KeyCode::Up | KeyCode::BackTab => state.active_idx = state.active_idx.saturating_sub(1),
                        KeyCode::Down | KeyCode::Tab | KeyCode::Enter => { state.active_idx = (state.active_idx + 1).min(4); }
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
    }

    if cancelled { return; }

    if state.to.trim().is_empty() && state.cc.trim().is_empty() && state.bcc.trim().is_empty() {
        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
        queue!(stdout, Print("No recipients specified. Message cancelled.\r\n\nPress Enter to return...")).unwrap();
        stdout.flush().unwrap();
        while let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } }
        return;
    }

    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
    queue!(stdout, Print("Sending message...\r\n")).unwrap();
    stdout.flush().unwrap();

    let mut builder = Message::builder()
        .from(format!("<{}>", account.email).parse().unwrap())
        .subject(state.subject);

    let parse_and_add = |mut b: lettre::message::MessageBuilder, input: &str, field_type: &str| -> lettre::message::MessageBuilder {
        for addr in input.split(',') {
            let trimmed = addr.trim();
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

    builder = parse_and_add(builder, &state.to, "to");
    builder = parse_and_add(builder, &state.cc, "cc");
    builder = parse_and_add(builder, &state.bcc, "bcc");

    let mut multipart = lettre::message::MultiPart::mixed()
        .singlepart(lettre::message::SinglePart::plain(final_body));

    for att in &state.attachments {
        if let Ok(file_data) = fs::read(att) {
            let filename = std::path::Path::new(att).file_name().unwrap_or_default().to_string_lossy().into_owned();
            let ext = std::path::Path::new(att).extension().unwrap_or_default().to_string_lossy().to_lowercase();
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
                let attachment = lettre::message::Attachment::new(filename).body(file_data, content_type);
                multipart = multipart.singlepart(attachment);
            }
        }
    }

    match builder.multipart(multipart) {
        Ok(email_msg) => {
            let creds = SmtpCredentials::new(account.email.clone(), account.password.clone());
            let mailer = SmtpTransport::relay("smtp.gmail.com").unwrap().credentials(creds).build();
            match mailer.send(&email_msg) {
                Ok(_) => queue!(stdout, Print("-> Message sent successfully!\r\n")).unwrap(),
                Err(e) => queue!(stdout, Print(format!("-> Failed to send message: {:?}\r\n", e))).unwrap(),
            }
        }
        Err(e) => queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap(),
    }

    queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
    stdout.flush().unwrap();
    loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
}

fn parse_email_body(body_data: &[u8]) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    let mut text_body = String::new();
    let mut html_body: Option<String> = None;
    let mut attachments: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(parsed) = mailparse::parse_mail(body_data) {
        fn walk(part: &mailparse::ParsedMail, text: &mut String, html: &mut Option<String>, atts: &mut Vec<(String, Vec<u8>)>) {
            let ctype = part.ctype.mimetype.as_str();
            let disposition = part.get_content_disposition();

            let is_attachment = disposition.disposition == mailparse::DispositionType::Attachment ||
                part.headers.iter().any(|header| header.get_key().eq_ignore_ascii_case("content-disposition") && header.get_value().to_lowercase().contains("attachment"));

            if is_attachment || (disposition.disposition == mailparse::DispositionType::Inline && ctype != "text/plain" && ctype != "text/html" && !ctype.starts_with("multipart/")) {
                let filename = disposition.params.get("filename").cloned()
                    .or_else(|| part.ctype.params.get("name").cloned())
                    .unwrap_or_else(|| format!("attachment_{}", atts.len() + 1));

                if let Ok(data) = part.get_body_raw() {
                    atts.push((filename, data));
                }
            } else {
                if ctype == "text/plain" {
                    if let Ok(body) = part.get_body() {
                        text.push_str(&body);
                    }
                } else if ctype == "text/html" {
                    if let Ok(body) = part.get_body() {
                        *html = Some(body);
                    }
                } else {
                    for subpart in &part.subparts {
                        walk(subpart, text, html, atts);
                    }
                }
            }
        }

        walk(&parsed, &mut text_body, &mut html_body, &mut attachments);

        if text_body.is_empty() && html_body.is_some() {
            text_body = "[This message only contains an HTML body. Press ^B to view it in your browser.]\r\n".to_string();
        } else if !text_body.is_empty() {
            text_body = text_body.replace('\n', "\r\n");
        } else {
            text_body = String::from_utf8_lossy(body_data).replace('\n', "\r\n");
        }
    } else {
        let raw = String::from_utf8_lossy(body_data);
        text_body = raw.replace('\n', "\r\n");
    }

    (text_body, html_body, attachments)
}

fn draw_xnano_menu(stdout: &mut std::io::Stdout, rows: u16, cols: u16, items_row1: &[(&str, &str)], items_row2: &[(&str, &str)]) {
    let ui_bg = Color::Rgb { r: 20, g: 20, b: 20 };
    let key_fg = Color::Rgb { r: 0, g: 150, b: 200 };
    let text_fg = Color::Reset;

    // Prevent division by zero if an empty array is passed
    let col_width = (cols as usize) / items_row1.len().max(1);

    // Route the drawing logic through the integrated xnano UI module
    let _ = Editor::draw_menu_line(stdout, rows.saturating_sub(2), cols, col_width, items_row1, ui_bg, key_fg, text_fg);
    let _ = Editor::draw_menu_line(stdout, rows.saturating_sub(1), cols, col_width, items_row2, ui_bg, key_fg, text_fg);
}

// fn main() {
//     let config = load_config();
//
//     let mut current_account_idx = 0;
//     let mut active_account = config.accounts[current_account_idx].clone();
//
//     let domain = "imap.gmail.com";
//     let tls = TlsConnector::builder().build().expect("Failed to build TLS connector");
//     let client = imap::connect((domain, 993), domain, &tls).expect("Could not connect to Gmail IMAP server");
//     let mut session = client.login(&active_account.email, &active_account.password).expect("IMAP Login failed.");
//
//     let mut mailbox = session.select("INBOX").expect("Could not select INBOX");
//     let mut total_messages = mailbox.exists;
//
//     let mut current_page: u32 = 0;
//     let mut selected_index: usize = 0;
//     let mut page_emails: Vec<EmailMeta> = Vec::new();
//
//     let mut needs_fetch = true;
//     let mut needs_reconnect = false;
//     let mut mode = AppMode::List;
//
//     let mut restore_index_from_end: Option<u32> = Some(0);
//
//     enable_raw_mode().expect("Failed to enable raw mode");
//     let mut stdout = stdout();
//
//     execute!(stdout, EnterAlternateScreen).unwrap();
//
//     loop {
//         let (cols, rows) = term_size().unwrap_or((80, 24));
//
//         if needs_reconnect {
//             execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//             active_account = config.accounts[current_account_idx].clone();
//
//             queue!(stdout, Print(format!("Switching to Account: {}...\r\n", active_account.email))).unwrap();
//             stdout.flush().unwrap();
//
//             let _ = session.logout();
//
//             let new_client = imap::connect((domain, 993), domain, &tls).expect("Could not connect to IMAP server");
//             session = new_client.login(&active_account.email, &active_account.password).expect("IMAP Login failed");
//
//             mailbox = session.select("INBOX").expect("Could not select INBOX");
//             total_messages = mailbox.exists;
//             current_page = 0;
//             selected_index = 0;
//             needs_fetch = true;
//             needs_reconnect = false;
//         }
//
//         let items_per_page = (rows.saturating_sub(4) as u32).max(1);
//         let total_pages = if total_messages == 0 { 1 } else { (total_messages + items_per_page - 1) / items_per_page };
//
//         if current_page >= total_pages {
//             current_page = total_pages.saturating_sub(1);
//             needs_fetch = true;
//         }
//
//         if needs_fetch {
//             page_emails.clear();
//
//             if total_messages > 0 {
//                 let end_idx = total_messages.saturating_sub(current_page * items_per_page);
//                 let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
//                 let sequence = format!("{}:{}", start_idx, end_idx);
//
//                 if let Ok(messages) = session.fetch(&sequence, "(ENVELOPE FLAGS RFC822.SIZE)") {
//                     for message in messages.iter() {
//                         let size = message.size.unwrap_or(0);
//                         let mut is_seen = false;
//                         let mut is_deleted = false;
//                         let mut is_flagged = false;
//
//                         for flag in message.flags() {
//                             match flag {
//                                 imap::types::Flag::Seen => is_seen = true,
//                                 imap::types::Flag::Deleted => is_deleted = true,
//                                 imap::types::Flag::Flagged => is_flagged = true,
//                                 _ => {}
//                             }
//                         }
//
//                         let mut subject = "No Subject".to_string();
//                         let mut from = "Unknown Sender".to_string();
//                         let mut reply_to = "unknown@example.com".to_string();
//                         let mut to_addr = String::new();
//                         let mut reply_to_display = String::new();
//                         let mut cc = String::new();
//
//                         let mut date = "Unknown Date".to_string();
//                         let mut parsed_dt = None;
//
//                         if let Some(env) = message.envelope() {
//                             if let Some(s) = env.subject.as_ref() { subject = String::from_utf8_lossy(s).into_owned(); }
//
//                             if let Some(d) = env.date.as_ref() {
//                                 let raw_date = String::from_utf8_lossy(d).into_owned();
//                                 if let Ok(parsed_time) = DateTime::parse_from_rfc2822(&raw_date) {
//                                     parsed_dt = Some(parsed_time);
//                                 } else {
//                                     if let Some(idx) = raw_date.find(" +").or_else(|| raw_date.find(" -")) {
//                                         date = raw_date[..idx].to_string();
//                                     } else {
//                                         date = raw_date;
//                                     }
//                                 }
//                             }
//
//                             if let Some(f_vec) = env.from.as_ref() {
//                                 if let Some(addr) = f_vec.first() {
//                                     let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
//                                     let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
//                                     let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h).into_owned()).unwrap_or_default();
//
//                                     let email_raw = format!("{}@{}", mailbox, host);
//                                     reply_to = email_raw.clone();
//                                     from = if !name.is_empty() { format!("{} <{}>", name, email_raw) } else { email_raw };
//                                 }
//                             }
//
//                             if let Some(t_vec) = env.to.as_ref() {
//                                 let mut tos = Vec::new();
//                                 for addr in t_vec {
//                                     let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
//                                     let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
//                                     let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h).into_owned()).unwrap_or_default();
//                                     let email = format!("{}@{}", mailbox, host);
//                                     if !name.is_empty() { tos.push(format!("{} <{}>", name, email)); }
//                                     else { tos.push(email); }
//                                 }
//                                 to_addr = tos.join(", ");
//                             }
//
//                             if let Some(rt_vec) = env.reply_to.as_ref() {
//                                 let mut rts = Vec::new();
//                                 for addr in rt_vec {
//                                     let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
//                                     let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
//                                     let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h).into_owned()).unwrap_or_default();
//                                     let email = format!("{}@{}", mailbox, host);
//                                     if !name.is_empty() { rts.push(format!("{} <{}>", name, email)); }
//                                     else { rts.push(email); }
//                                 }
//                                 reply_to_display = rts.join(", ");
//                                 if reply_to_display == from || reply_to_display == reply_to {
//                                     reply_to_display.clear();
//                                 }
//                             }
//
//                             if let Some(cc_vec) = env.cc.as_ref() {
//                                 let mut ccs = Vec::new();
//                                 for addr in cc_vec {
//                                     let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
//                                     let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
//                                     let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h).into_owned()).unwrap_or_default();
//                                     let email = format!("{}@{}", mailbox, host);
//                                     if !name.is_empty() { ccs.push(format!("{} <{}>", name, email)); }
//                                     else { ccs.push(email); }
//                                 }
//                                 cc = ccs.join(", ");
//                             }
//                         }
//
//                         if let Some(dt) = parsed_dt {
//                             let now = Utc::now().timestamp();
//                             let diff = now - dt.timestamp();
//                             let local_dt = dt.with_timezone(&Local);
//
//                             if diff < 7 * 24 * 3600 && diff >= -86400 {
//                                 date = local_dt.format("%a %H:%M").to_string();
//                             } else {
//                                 date = local_dt.format("%b %d").to_string();
//                             }
//                         }
//
//                         page_emails.push(EmailMeta { id: message.message, subject, from, reply_to, reply_to_display, to_addr, cc, date, size, is_read: is_seen, is_deleted, is_flagged });
//                     }
//                 }
//
//                 page_emails.sort_by(|a, b| a.id.cmp(&b.id));
//
//                 if let Some(idx_from_end) = restore_index_from_end {
//                     if !page_emails.is_empty() {
//                         selected_index = page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize);
//                     } else {
//                         selected_index = 0;
//                     }
//                     restore_index_from_end = None;
//                 } else if selected_index >= page_emails.len() {
//                     selected_index = page_emails.len().saturating_sub(1);
//                 }
//             }
//             needs_fetch = false;
//         }
//
//         execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
//
//         let ui_bg = Color::Rgb { r: 20, g: 20, b: 20 };
//         let title_fg = Color::Rgb{ r: 50, g: 200, b: 250 };
//
//         match &mode {
//             AppMode::List => {
//                 let (cols, rows) = term_size().unwrap_or((80, 24));
//
//                 // 1. Paint the ENTIRE terminal background with the theme's background color
//                 queue!(stdout, SetBackgroundColor(bg_color), Clear(ClearType::All)).unwrap();
//
//                 // 2. Draw your top title bar
//                 let header_title = format!(" Inbox ({})", active_account.email);
//                 queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(bg_color), SetForegroundColor(accent_color), Print(header_title), ResetColor).unwrap();
//
//                 // 3. Draw your emails
//                 let list_start_y = 2;
//                 let visible_capacity = rows.saturating_sub(6) as usize; // Leave room for top margin and bottom menus
//
//                 for (i, email) in page_emails.iter().enumerate() {
//                     if i >= visible_capacity { break; } // Prevent drawing off the bottom of the screen
//
//                     let row_y = (i + list_start_y) as u16;
//
//                     // Determine background color based on whether this row is currently selected
//                     let row_bg = if i == selected_index { selected_bg } else { bg_color };
//
//                     queue!(stdout, cursor::MoveTo(0, row_y), SetBackgroundColor(row_bg), SetForegroundColor(text_fg), Clear(ClearType::UntilNewLine)).unwrap();
//
//                     // Format the email row fields
//                     let read_status = if email.is_read { " " } else { "*" };
//
//                     // Truncate 'From' field safely
//                     let from_width = 25;
//                     let from_str = if email.from.chars().count() > from_width {
//                         format!("{}...", email.from.chars().take(from_width.saturating_sub(3)).collect::<String>())
//                     } else {
//                         format!("{:width$}", email.from, width = from_width)
//                     };
//
//                     // Format Date
//                     let date_width = 12;
//                     let date_str = format!("{:width$}", email.date, width = date_width);
//
//                     // Calculate remaining space for 'Subject' to prevent terminal wrapping
//                     let fixed_width = 4 + from_width + date_width + 4; // status(2) + date + from + spaces
//                     let subject_width = (cols as usize).saturating_sub(fixed_width);
//                     let subj_str = if email.subject.chars().count() > subject_width {
//                         format!("{}...", email.subject.chars().take(subject_width.saturating_sub(3)).collect::<String>())
//                     } else {
//                         email.subject.clone()
//                     };
//
//                     // Print the formatted row
//                     queue!(
//                         stdout,
//                         Print(format!(" {}  {}  {}  {}", read_status, date_str, from_str, subj_str))
//                     ).unwrap();
//                 }
//
//                 // 4. Draw the unified bottom menus using xnano's exact UI formatter
//                 let r_col = ((cols as usize) / 4).max(1);
//                 Editor::draw_menu_line(&mut stdout, rows - 2, cols, r_col, &[("Enter", " Read Email"), ("C", " Compose"), ("D", " Delete")], bg_color, accent_color, text_fg).unwrap();
//                 Editor::draw_menu_line(&mut stdout, rows - 1, cols, r_col, &[("Q / Esc", " Quit App"), ("Arrows", " Navigate"), ("Tab", " Switch Acct")], bg_color, accent_color, text_fg).unwrap();
//
//                 stdout.flush().unwrap();
//
//                 // 5. Handle List View Input
//                 if let Event::Key(key) = event::read().unwrap() {
//                     if key.kind == KeyEventKind::Press {
//                         match key.code {
//                             KeyCode::Char('q') | KeyCode::Esc => break, // Exits the main application loop
//                             KeyCode::Down => {
//                                 if selected_index + 1 < page_emails.len() {
//                                     selected_index += 1;
//                                 } else {
//                                     // Optional: Trigger fetching the next page of emails here
//                                 }
//                             }
//                             KeyCode::Up => {
//                                 if selected_index > 0 {
//                                     selected_index -= 1;
//                                 } else {
//                                     // Optional: Trigger fetching the previous page of emails here
//                                 }
//                             }
//                             KeyCode::Enter => {
//                                 if !page_emails.is_empty() {
//                                     let current = &page_emails[selected_index];
//
//                                     // NOTE: You will likely need to insert your actual IMAP fetch
//                                     // logic here to get the full body text and attachments!
//                                     let fetched_body = "Placeholder: Fetch body for UID...".to_string();
//                                     let fetched_attachments = Vec::new();
//
//                                     mode = AppMode::Reading {
//                                         text_body: fetched_body,
//                                         html_body: None,
//                                         attachments: fetched_attachments,
//                                     };
//                                 }
//                             }
//                             KeyCode::Char('c') | KeyCode::Char('C') => {
//                                 compose_email(&active_account, None, None, None);
//                                 needs_fetch = true; // Refresh list upon returning from compose
//                             }
//                             KeyCode::Char('d') | KeyCode::Char('D') => {
//                                 if !page_emails.is_empty() {
//                                     // NOTE: Insert your IMAP delete logic here
//                                     // e.g., session.uid_store(page_emails[selected_index].id.to_string(), "+FLAGS (\\Deleted)").unwrap();
//                                     needs_fetch = true;
//                                 }
//                             }
//                             KeyCode::Tab => {
//                                 // NOTE: Insert logic to switch active_account here
//                                 needs_fetch = true;
//                             }
//                             _ => {}
//                         }
//                     }
//                 }
//             }
//             AppMode::Reading { text_body, attachments, .. } => {
//                 let current = &page_emails[selected_index];
//                 let mut reader = Editor::new(None);
//                 reader.menu_state = MenuState::EmailReader;
//                 reader.top_margin = 6;
//
//                 reader.buffer = Rope::from_str(text_body.as_str());
//
//                 loop {
//                     // Removed the unused `cols` and `rows` variables!
//
//                     let theme = &reader.theme_set.themes[&reader.current_theme];
//                     let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
//                     let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });
//
//                     let header_bg = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
//                     let text_fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
//                     let label_color = Color::Rgb { r: 50, g: 150, b: 200 };
//                     let soft_yellow = Color::Rgb { r: 255, g: 255, b: 150 };
//
//                     for i in 0..7 {
//                         queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(header_bg), terminal::Clear(ClearType::UntilNewLine)).unwrap();
//                     }
//
//                     let header_title = format!("View Email ({})", active_account.email);
//                     queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(label_color), Print(header_title), ResetColor).unwrap();
//
//                     let fields = ["From:", "To:", "Cc:", "Subject:"];
//                     let vals = [&current.from, &current.to_addr, &current.cc, &current.subject];
//
//                     for i in 0..4 {
//                         queue!(
//                                 stdout, cursor::MoveTo(0, (i + 1) as u16),
//                                 SetBackgroundColor(header_bg), SetForegroundColor(label_color), Print(format!("{:>8}", fields[i])),
//                                 SetForegroundColor(text_fg), Print(" "), Print(vals[i]), ResetColor
//                             ).unwrap();
//                     }
//
//                     queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(header_bg), SetForegroundColor(label_color), Print(" Attach: "), ResetColor).unwrap();
//                     if attachments.is_empty() {
//                         queue!(stdout, SetForegroundColor(Color::DarkGrey), Print("None"), ResetColor).unwrap();
//                     } else {
//                         let att_names: Vec<&str> = attachments.iter().map(|(n, _)| n.as_str()).collect();
//                         queue!(stdout, SetForegroundColor(soft_yellow), Print(att_names.join(", ")), ResetColor).unwrap();
//                     }
//
//                     reader.draw_screen().unwrap();
//
//                     if let Event::Key(key) = event::read().unwrap() {
//                         if key.kind == KeyEventKind::Press {
//                             match reader.handle_keypress(key).unwrap() {
//                                 EditorResult::Cancel => {
//                                     mode = AppMode::List;
//                                     needs_fetch = true;
//                                     break; // Exits the reading loop
//                                 }
//                                 EditorResult::Send(action) if action == "REPLY" => {
//                                     let subject = if current.subject.to_lowercase().starts_with("re:") { current.subject.clone() } else { format!("Re: {}", current.subject) };
//                                     compose_email(&active_account, Some(&current.reply_to), Some(&subject), None);
//                                     break; // Exits the reading loop
//                                 }
//                                 EditorResult::Send(action) if action == "FORWARD" => {
//                                     let subject = if current.subject.to_lowercase().starts_with("fwd:") { current.subject.clone() } else { format!("Fwd: {}", current.subject) };
//                                     let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", current.from, current.date, current.subject, text_body);
//                                     compose_email(&active_account, None, Some(&subject), Some(&fwd_body));
//                                     break; // Exits the reading loop
//                                 }
//                                 _ => {}
//                             }
//                         }
//                     }
//                 }
//
//                 // THE MAGIC FIX: After the inner loop breaks, this instantly tells the outer
//                 // application loop to restart and draw the List mode, skipping the blank screen!
//                 continue;
//             }
//         }
//         stdout.flush().unwrap();
//
//         match event::read().expect("Failed to read event") {
//             Event::Key(key_event) => {
//                 if key_event.kind == KeyEventKind::Press {
//                     match &mode {
//                         AppMode::List => {
//                             match key_event.code {
//                                 KeyCode::Tab => {
//                                     if config.accounts.len() > 1 {
//                                         current_account_idx = (current_account_idx + 1) % config.accounts.len();
//                                         needs_reconnect = true;
//                                         restore_index_from_end = Some(0);
//                                     }
//                                 }
//                                 KeyCode::Char(c) if c.is_ascii_digit() => {
//                                     if let Some(digit) = c.to_digit(10) {
//                                         let idx = (digit as usize).saturating_sub(1);
//                                         if idx < config.accounts.len() && idx != current_account_idx {
//                                             current_account_idx = idx;
//                                             needs_reconnect = true;
//                                             restore_index_from_end = Some(0);
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
//                                     if selected_index > 0 { selected_index -= 1; }
//                                     else if current_page + 1 < total_pages { current_page += 1; needs_fetch = true; selected_index = (items_per_page - 1) as usize; }
//                                 }
//                                 KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
//                                     if !page_emails.is_empty() {
//                                         if selected_index + 1 < page_emails.len() { selected_index += 1; }
//                                         else if current_page > 0 { current_page -= 1; needs_fetch = true; selected_index = 0; }
//                                     }
//                                 }
//                                 KeyCode::Char('*') => {
//                                     if !page_emails.is_empty() {
//                                         let seq_id = page_emails[selected_index].id.to_string();
//                                         if page_emails[selected_index].is_flagged {
//                                             if session.store(&seq_id, "-FLAGS (\\Flagged)").is_ok() { page_emails[selected_index].is_flagged = false; }
//                                         } else {
//                                             if session.store(&seq_id, "+FLAGS (\\Flagged)").is_ok() { page_emails[selected_index].is_flagged = true; }
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char('c') | KeyCode::Char('C') => {
//                                     compose_email(&active_account, None, None, None);
//                                 }
//                                 KeyCode::Char('d') | KeyCode::Char('D') => {
//                                     if !page_emails.is_empty() {
//                                         let seq_id = page_emails[selected_index].id.to_string();
//                                         if page_emails[selected_index].is_deleted {
//                                             if session.store(&seq_id, "-FLAGS (\\Deleted)").is_ok() {
//                                                 page_emails[selected_index].is_deleted = false;
//                                             }
//                                         } else {
//                                             if session.store(&seq_id, "+FLAGS (\\Deleted)").is_ok() {
//                                                 page_emails[selected_index].is_deleted = true;
//                                             }
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char('m') | KeyCode::Char('M') => {
//                                     if !page_emails.is_empty() {
//                                         let seq_id = page_emails[selected_index].id.to_string();
//                                         if page_emails[selected_index].is_read {
//                                             if session.store(&seq_id, "-FLAGS (\\Seen)").is_ok() { page_emails[selected_index].is_read = false; }
//                                         } else {
//                                             if session.store(&seq_id, "+FLAGS (\\Seen)").is_ok() { page_emails[selected_index].is_read = true; }
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char('x') | KeyCode::Char('X') => {
//                                     if !page_emails.is_empty() {
//                                         let offset = current_page * items_per_page + (page_emails.len().saturating_sub(1).saturating_sub(selected_index)) as u32;
//
//                                         if session.expunge().is_ok() {
//                                             if let Ok(m) = session.select("INBOX") {
//                                                 total_messages = m.exists;
//
//                                                 let safe_offset = offset.min(total_messages.saturating_sub(1));
//                                                 current_page = safe_offset / items_per_page;
//
//                                                 restore_index_from_end = Some(safe_offset % items_per_page);
//                                                 needs_fetch = true;
//                                             }
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char('f') | KeyCode::Char('F') => {
//                                     if !page_emails.is_empty() {
//                                         let current = &page_emails[selected_index];
//                                         let fetch_seq = current.id.to_string();
//                                         let mut t_body = String::from("Could not load email body.");
//
//                                         if let Ok(full_msgs) = session.fetch(&fetch_seq, "RFC822") {
//                                             if let Some(full_msg) = full_msgs.iter().next() {
//                                                 if let Some(body_data) = full_msg.body() {
//                                                     let (t, _, _) = parse_email_body(body_data);
//                                                     t_body = t;
//                                                 }
//                                             }
//                                         }
//
//                                         let subject = if current.subject.to_lowercase().starts_with("fwd:") { current.subject.clone() } else { format!("Fwd: {}", current.subject) };
//                                         let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", current.from, current.date, current.subject, t_body);
//                                         compose_email(&active_account, None, Some(&subject), Some(&fwd_body));
//                                     }
//                                 }
//                                 KeyCode::Char('r') | KeyCode::Char('R') => {
//                                     if !page_emails.is_empty() {
//                                         let current = &page_emails[selected_index];
//                                         let subject = if current.subject.to_lowercase().starts_with("re:") { current.subject.clone() } else { format!("Re: {}", current.subject) };
//                                         compose_email(&active_account, Some(&current.reply_to), Some(&subject), None);
//                                     }
//                                 }
//                                 KeyCode::Enter | KeyCode::Right | KeyCode::Char('>') => {
//                                     if !page_emails.is_empty() {
//                                         let fetch_seq = page_emails[selected_index].id.to_string();
//                                         let mut t_body = String::from("Could not load email body.");
//                                         let mut h_body = None;
//                                         let mut atts = Vec::new();
//
//                                         if let Ok(full_msgs) = session.fetch(&fetch_seq, "RFC822") {
//                                             if let Some(full_msg) = full_msgs.iter().next() {
//                                                 if let Some(body_data) = full_msg.body() {
//                                                     let (t, h, a) = parse_email_body(body_data);
//                                                     t_body = t;
//                                                     h_body = h;
//                                                     atts = a;
//                                                 }
//                                             }
//                                             page_emails[selected_index].is_read = true;
//                                         }
//                                         mode = AppMode::Reading { text_body: t_body, html_body: h_body, attachments: atts };
//                                     }
//                                 }
//                                 KeyCode::Char('q') | KeyCode::Esc => break,
//                                 _ => {}
//                             }
//                         }
//                         AppMode::Reading { .. } => {
//                             match key_event.code {
//                                 KeyCode::Char('b') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
//                                     if let AppMode::Reading { html_body: Some(html), .. } = &mode {
//                                         let temp_dir = std::env::temp_dir();
//                                         let file_path = temp_dir.join("rustmail_temp.html");
//                                         if std::fs::write(&file_path, html).is_ok() {
//                                             let _ = webbrowser::open(file_path.to_str().unwrap());
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
//                                     if !key_event.modifiers.contains(KeyModifiers::CONTROL) && !key_event.modifiers.contains(KeyModifiers::ALT) {
//                                         if let AppMode::Reading { attachments, .. } = &mode {
//                                             let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
//                                             if idx < attachments.len() {
//                                                 let (filename, data) = &attachments[idx];
//                                                 let temp_dir = std::env::temp_dir();
//                                                 let safe_filename = filename.replace(|ch: char| !ch.is_alphanumeric() && ch != '.' && ch != '-' && ch != '_', "_");
//                                                 let file_path = temp_dir.join(&safe_filename);
//                                                 if std::fs::write(&file_path, data).is_ok() {
//                                                     let _ = webbrowser::open(file_path.to_str().unwrap());
//                                                 }
//                                             }
//                                         }
//                                     }
//                                 }
//                                 KeyCode::Char('f') | KeyCode::Char('F') => {
//                                     if let AppMode::Reading { text_body, .. } = &mode {
//                                         let current = &page_emails[selected_index];
//                                         let subject = if current.subject.to_lowercase().starts_with("fwd:") { current.subject.clone() } else { format!("Fwd: {}", current.subject) };
//                                         let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", current.from, current.date, current.subject, text_body);
//                                         compose_email(&active_account, None, Some(&subject), Some(&fwd_body));
//                                     }
//                                 }
//                                 KeyCode::Char('r') | KeyCode::Char('R') => {
//                                     let current = &page_emails[selected_index];
//                                     let subject = if current.subject.to_lowercase().starts_with("re:") { current.subject.clone() } else { format!("Re: {}", current.subject) };
//                                     compose_email(&active_account, Some(&current.reply_to), Some(&subject), None);
//                                 }
//                                 KeyCode::Char('<') | KeyCode::Left => { mode = AppMode::List; }
//                                 KeyCode::Char('q') | KeyCode::Esc => break,
//                                 _ => {}
//                             }
//                         }
//                     }
//                 }
//             }
//             Event::Resize(_, _) => {
//                 if let AppMode::List = mode { needs_fetch = true; }
//             }
//             _ => {}
//         }
//     }
//
//     execute!(stdout, LeaveAlternateScreen).unwrap();
//     disable_raw_mode().expect("Failed to disable raw mode");
//     let _ = session.logout();
// }

fn main() {
    let config = load_config();

    let mut current_account_idx = 0;
    let mut active_account = config.accounts[current_account_idx].clone();

    let domain = "imap.gmail.com";
    let tls = TlsConnector::builder().build().expect("Failed to build TLS connector");
    let client = imap::connect((domain, 993), domain, &tls).expect("Could not connect to Gmail IMAP server");
    let mut session = client.login(&active_account.email, &active_account.password).expect("IMAP Login failed.");

    let mut mailbox = session.select("INBOX").expect("Could not select INBOX");
    let mut total_messages = mailbox.exists;

    let mut current_page: u32 = 0;
    let mut selected_index: usize = 0;
    let mut page_emails: Vec<EmailMeta> = Vec::new();

    let mut needs_fetch = true;
    let mut needs_reconnect = false;
    let mut mode = AppMode::List;

    let mut restore_index_from_end: Option<u32> = Some(0);

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();

    execute!(stdout, EnterAlternateScreen).unwrap();

    // --- EXTRACT THEME ONCE FOR PERFORMANCE ---
    let theme_provider = Editor::new(None);
    let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];

    let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
    let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });

    let bg_color = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
    let text_fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
    let accent_color = Color::Rgb { r: 50, g: 150, b: 200 };

    let selected_bg = if raw_bg.r < 128 {
        Color::Rgb { r: raw_bg.r.saturating_add(40), g: raw_bg.g.saturating_add(40), b: raw_bg.b.saturating_add(40) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(40), g: raw_bg.g.saturating_sub(40), b: raw_bg.b.saturating_sub(40) }
    };

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));

        if needs_reconnect {
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            active_account = config.accounts[current_account_idx].clone();

            queue!(stdout, Print(format!("Switching to Account: {}...\r\n", active_account.email))).unwrap();
            stdout.flush().unwrap();

            let _ = session.logout();

            let new_client = imap::connect((domain, 993), domain, &tls).expect("Could not connect to IMAP server");
            session = new_client.login(&active_account.email, &active_account.password).expect("IMAP Login failed");

            mailbox = session.select("INBOX").expect("Could not select INBOX");
            total_messages = mailbox.exists;
            current_page = 0;
            selected_index = 0;
            needs_fetch = true;
            needs_reconnect = false;
        }

        let items_per_page = (rows.saturating_sub(4) as u32).max(1);
        let total_pages = if total_messages == 0 { 1 } else { (total_messages + items_per_page - 1) / items_per_page };

        if current_page >= total_pages {
            current_page = total_pages.saturating_sub(1);
            needs_fetch = true;
        }

        if needs_fetch {
            page_emails.clear();

            if total_messages > 0 {
                let end_idx = total_messages.saturating_sub(current_page * items_per_page);
                let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
                let sequence = format!("{}:{}", start_idx, end_idx);

                if let Ok(messages) = session.fetch(&sequence, "(ENVELOPE FLAGS RFC822.SIZE)") {
                    for message in messages.iter() {
                        let size = message.size.unwrap_or(0);
                        let mut is_seen = false;
                        let mut is_deleted = false;
                        let mut is_flagged = false;

                        for flag in message.flags() {
                            match flag {
                                imap::types::Flag::Seen => is_seen = true,
                                imap::types::Flag::Deleted => is_deleted = true,
                                imap::types::Flag::Flagged => is_flagged = true,
                                _ => {}
                            }
                        }

                        let mut subject = "No Subject".to_string();
                        let mut from = "Unknown Sender".to_string();
                        let mut reply_to = "unknown@example.com".to_string();
                        let to_addr = String::new();
                        let reply_to_display = String::new();
                        let cc = String::new();

                        let mut date = "Unknown Date".to_string();
                        let mut parsed_dt = None;

                        if let Some(env) = message.envelope() {
                            if let Some(s) = env.subject.as_ref() { subject = String::from_utf8_lossy(s).into_owned(); }

                            if let Some(d) = env.date.as_ref() {
                                let raw_date = String::from_utf8_lossy(d).into_owned();
                                if let Ok(parsed_time) = DateTime::parse_from_rfc2822(&raw_date) {
                                    parsed_dt = Some(parsed_time);
                                } else {
                                    if let Some(idx) = raw_date.find(" +").or_else(|| raw_date.find(" -")) {
                                        date = raw_date[..idx].to_string();
                                    } else {
                                        date = raw_date;
                                    }
                                }
                            }

                            if let Some(f_vec) = env.from.as_ref() {
                                if let Some(addr) = f_vec.first() {
                                    let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
                                    let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
                                    let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h).into_owned()).unwrap_or_default();

                                    let email_raw = format!("{}@{}", mailbox, host);
                                    reply_to = email_raw.clone();
                                    from = if !name.is_empty() { format!("{} <{}>", name, email_raw) } else { email_raw };
                                }
                            }
                            // ... Parsing logic completes ...
                        }

                        if let Some(dt) = parsed_dt {
                            let now = Utc::now().timestamp();
                            let diff = now - dt.timestamp();
                            let local_dt = dt.with_timezone(&Local);

                            if diff < 7 * 24 * 3600 && diff >= -86400 {
                                date = local_dt.format("%a %H:%M").to_string();
                            } else {
                                date = local_dt.format("%b %d").to_string();
                            }
                        }

                        page_emails.push(EmailMeta { id: message.message, subject, from, reply_to, reply_to_display, to_addr, cc, date, size, is_read: is_seen, is_deleted, is_flagged });
                    }
                }

                page_emails.sort_by(|a, b| a.id.cmp(&b.id));

                if let Some(idx_from_end) = restore_index_from_end {
                    if !page_emails.is_empty() {
                        selected_index = page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize);
                    } else {
                        selected_index = 0;
                    }
                    restore_index_from_end = None;
                } else if selected_index >= page_emails.len() {
                    selected_index = page_emails.len().saturating_sub(1);
                }
            }
            needs_fetch = false;
        }

        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        // ------------------------------------------------------------------
        // APP MODE: READING (Has its own inner loop)
        // ------------------------------------------------------------------
        if let AppMode::Reading { text_body, attachments, .. } = &mode {
            let current = &page_emails[selected_index];
            let mut reader = Editor::new(None);
            reader.menu_state = MenuState::EmailReader;
            reader.top_margin = 6;
            reader.buffer = Rope::from_str(text_body.as_str());

            let soft_yellow = Color::Rgb { r: 255, g: 255, b: 150 };

            loop {
                for i in 0..7 { queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(bg_color), terminal::Clear(ClearType::UntilNewLine)).unwrap(); }

                let header_title = format!("View Email ({})", active_account.email);
                queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(accent_color), Print(header_title), ResetColor).unwrap();

                let fields = ["From:", "To:", "Cc:", "Subject:"];
                let vals = [&current.from, &current.to_addr, &current.cc, &current.subject];

                for i in 0..4 {
                    queue!(
                        stdout, cursor::MoveTo(0, (i + 1) as u16),
                        SetBackgroundColor(bg_color), SetForegroundColor(accent_color), Print(format!("{:>8}", fields[i])),
                        SetForegroundColor(text_fg), Print(" "), Print(vals[i]), ResetColor
                    ).unwrap();
                }

                queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(bg_color), SetForegroundColor(accent_color), Print(" Attach: "), ResetColor).unwrap();
                if attachments.is_empty() {
                    queue!(stdout, SetForegroundColor(Color::DarkGrey), Print("None"), ResetColor).unwrap();
                } else {
                    let att_names: Vec<&str> = attachments.iter().map(|(n, _)| n.as_str()).collect();
                    queue!(stdout, SetForegroundColor(soft_yellow), Print(att_names.join(", ")), ResetColor).unwrap();
                }

                reader.draw_screen().unwrap();

                if let Event::Key(key) = event::read().unwrap() {
                    if key.kind == KeyEventKind::Press {
                        match reader.handle_keypress(key).unwrap() {
                            EditorResult::Cancel => break,
                            EditorResult::Send(action) if action == "REPLY" => {
                                let subject = if current.subject.to_lowercase().starts_with("re:") { current.subject.clone() } else { format!("Re: {}", current.subject) };
                                compose_email(&active_account, Some(&current.reply_to), Some(&subject), None);
                                break;
                            }
                            EditorResult::Send(action) if action == "FORWARD" => {
                                let subject = if current.subject.to_lowercase().starts_with("fwd:") { current.subject.clone() } else { format!("Fwd: {}", current.subject) };
                                let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", current.from, current.date, current.subject, text_body);
                                compose_email(&active_account, None, Some(&subject), Some(&fwd_body));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Once broken out of the reading view, instantly revert to list mode
            mode = AppMode::List;
            needs_fetch = true;
            continue;
        }

        // ------------------------------------------------------------------
        // APP MODE: LIST (Draws the List View)
        // ------------------------------------------------------------------
        queue!(stdout, SetBackgroundColor(bg_color), Clear(ClearType::All)).unwrap();

        let header_title = format!("xpine - Inbox ({})", active_account.email);
        queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(bg_color), SetForegroundColor(accent_color), Print(header_title), ResetColor).unwrap();

        let list_start_y = 2;
        let visible_capacity = rows.saturating_sub(6) as usize;

        for (i, email) in page_emails.iter().enumerate() {
            if i >= visible_capacity { break; }

            let row_y = (i + list_start_y) as u16;
            let row_bg = if i == selected_index { selected_bg } else { bg_color };

            queue!(stdout, cursor::MoveTo(0, row_y), SetBackgroundColor(row_bg), Clear(ClearType::UntilNewLine)).unwrap();

            // 1. Status & ID Logic
            let flag_char = if email.is_flagged { "*" } else { " " };
            let status_char = if email.is_deleted { "D" } else if !email.is_read { "N" } else { " " };
            let email_num = format!("{:>4}", email.id);

            // 2. Log-Scale Size Coloring
            let size_kb = (email.size / 1024).max(1) as f32;
            let size_display = if size_kb < 1024.0 { format!("{:>4}K", size_kb as u32) } else { format!("{:>4}M", (size_kb / 1024.0) as u32) };
            let heat = (size_kb.log2() / 12.3).min(1.0).max(0.0);
            let size_color = Color::Rgb {
                r: (180.0 + (75.0 * heat)) as u8,
                g: (180.0 * (1.0 - heat)) as u8,
                b: (180.0 * (1.0 - heat)) as u8
            };

            // 3. Truncation
            let from_width = 22;
            let from_str = format!("{:<width$}", email.from.chars().take(from_width).collect::<String>(), width = from_width);

            let date_width = 12;
            let date_str = format!("{:width$}", email.date, width = date_width);

            let fixed_width = 51;
            let subject_width = (cols as usize).saturating_sub(fixed_width);
            let subj_truncated = email.subject.chars().take(subject_width).collect::<String>();
            let padded_subj = format!("{:<width$}", subj_truncated, width = subject_width);

            // 4. Render Row
            let _ = queue!(stdout, SetBackgroundColor(row_bg));

            // * (Bright Red) and N/D (Yellow/Purple)
            let _ = queue!(stdout, SetForegroundColor(Color::Red), Print(flag_char));

            let status_color = match status_char {
                "N" => Color::Yellow,
                "D" => Color::Magenta,
                _ => text_fg,
            };
            let _ = queue!(stdout, SetForegroundColor(status_color), Print(status_char));

            // Email Number (Theme text_fg)
            let _ = queue!(stdout, SetForegroundColor(text_fg), Print(format!(" {} ", email_num)));

            // Date (Themed Accent Color) + 1 space
            let _ = queue!(stdout, SetForegroundColor(accent_color), Print(format!("{} ", date_str)));

            // Sender (Theme text_fg) + 1 space
            let _ = queue!(stdout, SetForegroundColor(text_fg), Print(format!("{} ", from_str)));

            // Subject + 2 spaces
            let _ = queue!(stdout, Print(format!("{}  ", padded_subj)));

            // Size (Heat map color)
            let _ = queue!(stdout, SetForegroundColor(size_color), Print(size_display));
        }

        // Standardized 12-item menu logic
        let r_col = ((cols as usize) / 6).max(1);
        Editor::draw_menu_line(&mut stdout, rows - 2, cols, r_col, &[
            ("Enter", " Read"), ("C", " Compose"), ("R", " Reply"), ("F", " Forward"), ("D", " Delete"), ("*", " Flag")
        ], bg_color, accent_color, text_fg).unwrap();

        Editor::draw_menu_line(&mut stdout, rows - 1, cols, r_col, &[
            ("Q/Esc", " Quit"), ("Arrows", " Nav"), ("Tab", " Acct"), ("M", " Mark Read"), ("X", " Expunge"), ("", "")
        ], bg_color, accent_color, text_fg).unwrap();

        stdout.flush().unwrap();

        // ------------------------------------------------------------------
        // APP MODE: LIST (Handles events exclusively for the List View)
        // ------------------------------------------------------------------
        match event::read().expect("Failed to read event") {
            Event::Key(key_event) => {
                if key_event.kind == KeyEventKind::Press {
                    match key_event.code {
                        KeyCode::Tab => {
                            if config.accounts.len() > 1 {
                                current_account_idx = (current_account_idx + 1) % config.accounts.len();
                                needs_reconnect = true;
                                restore_index_from_end = Some(0);
                            }
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(digit) = c.to_digit(10) {
                                let idx = (digit as usize).saturating_sub(1);
                                if idx < config.accounts.len() && idx != current_account_idx {
                                    current_account_idx = idx;
                                    needs_reconnect = true;
                                    restore_index_from_end = Some(0);
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('P') => {
                            if selected_index > 0 { selected_index -= 1; }
                            else if current_page + 1 < total_pages { current_page += 1; needs_fetch = true; selected_index = (items_per_page - 1) as usize; }
                        }
                        KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('N') => {
                            if !page_emails.is_empty() {
                                if selected_index + 1 < page_emails.len() { selected_index += 1; }
                                else if current_page > 0 { current_page -= 1; needs_fetch = true; selected_index = 0; }
                            }
                        }
                        KeyCode::Char('*') => {
                            if !page_emails.is_empty() {
                                let seq_id = page_emails[selected_index].id.to_string();
                                if page_emails[selected_index].is_flagged {
                                    if session.store(&seq_id, "-FLAGS (\\Flagged)").is_ok() { page_emails[selected_index].is_flagged = false; }
                                } else {
                                    if session.store(&seq_id, "+FLAGS (\\Flagged)").is_ok() { page_emails[selected_index].is_flagged = true; }
                                }
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            compose_email(&active_account, None, None, None);
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if !page_emails.is_empty() {
                                let seq_id = page_emails[selected_index].id.to_string();
                                if page_emails[selected_index].is_deleted {
                                    if session.store(&seq_id, "-FLAGS (\\Deleted)").is_ok() { page_emails[selected_index].is_deleted = false; }
                                } else {
                                    if session.store(&seq_id, "+FLAGS (\\Deleted)").is_ok() { page_emails[selected_index].is_deleted = true; }
                                }
                            }
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            if !page_emails.is_empty() {
                                let seq_id = page_emails[selected_index].id.to_string();
                                if page_emails[selected_index].is_read {
                                    if session.store(&seq_id, "-FLAGS (\\Seen)").is_ok() { page_emails[selected_index].is_read = false; }
                                } else {
                                    if session.store(&seq_id, "+FLAGS (\\Seen)").is_ok() { page_emails[selected_index].is_read = true; }
                                }
                            }
                        }
                        KeyCode::Char('x') | KeyCode::Char('X') => {
                            if !page_emails.is_empty() {
                                let offset = current_page * items_per_page + (page_emails.len().saturating_sub(1).saturating_sub(selected_index)) as u32;

                                if session.expunge().is_ok() {
                                    if let Ok(m) = session.select("Inbox") {
                                        total_messages = m.exists;
                                        let safe_offset = offset.min(total_messages.saturating_sub(1));
                                        current_page = safe_offset / items_per_page;
                                        restore_index_from_end = Some(safe_offset % items_per_page);
                                        needs_fetch = true;
                                    }
                                }
                            }
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            if !page_emails.is_empty() {
                                let current = &page_emails[selected_index];
                                let fetch_seq = current.id.to_string();
                                let mut t_body = String::from("Could not load email body.");

                                if let Ok(full_msgs) = session.fetch(&fetch_seq, "RFC822") {
                                    if let Some(full_msg) = full_msgs.iter().next() {
                                        if let Some(body_data) = full_msg.body() {
                                            let (t, _, _) = parse_email_body(body_data);
                                            t_body = t;
                                        }
                                    }
                                }

                                let subject = if current.subject.to_lowercase().starts_with("fwd:") { current.subject.clone() } else { format!("Fwd: {}", current.subject) };
                                let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", current.from, current.date, current.subject, t_body);
                                compose_email(&active_account, None, Some(&subject), Some(&fwd_body));
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            if !page_emails.is_empty() {
                                let current = &page_emails[selected_index];
                                let subject = if current.subject.to_lowercase().starts_with("re:") { current.subject.clone() } else { format!("Re: {}", current.subject) };
                                compose_email(&active_account, Some(&current.reply_to), Some(&subject), None);
                            }
                        }
                        KeyCode::Enter | KeyCode::Right | KeyCode::Char('>') => {
                            if !page_emails.is_empty() {
                                let fetch_seq = page_emails[selected_index].id.to_string();
                                let mut t_body = String::from("Could not load email body.");
                                let mut h_body = None;
                                let mut atts = Vec::new();

                                if let Ok(full_msgs) = session.fetch(&fetch_seq, "RFC822") {
                                    if let Some(full_msg) = full_msgs.iter().next() {
                                        if let Some(body_data) = full_msg.body() {
                                            let (t, h, a) = parse_email_body(body_data);
                                            t_body = t;
                                            h_body = h;
                                            atts = a;
                                        }
                                    }
                                    page_emails[selected_index].is_read = true;
                                }
                                mode = AppMode::Reading { text_body: t_body, html_body: h_body, attachments: atts };
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
            Event::Resize(_, _) => {
                needs_fetch = true;
            }
            _ => {}
        }
    }

    execute!(stdout, LeaveAlternateScreen).unwrap();
    disable_raw_mode().expect("Failed to disable raw mode");
    let _ = session.logout();
}
