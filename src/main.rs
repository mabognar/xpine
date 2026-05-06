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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
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
    is_answered: bool,
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

#[derive(Clone, Copy)]
struct UiColors {
    bg: Color,
    fg: Color,
    ui_bg: Color,
    selected_bg: Color,
    accent: Color,
    date_color: Color,
    flag_n: Color,
    flag_d: Color,
    flag_a: Color,
    flag_star: Color,
    is_dark: bool,
}

fn derive_ui_colors(theme: &syntect::highlighting::Theme) -> UiColors {
    let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
    let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });

    let bg = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
    let fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
    let is_dark = (raw_bg.r as u32 + raw_bg.g as u32 + raw_bg.b as u32) < 384;

    let ui_bg = if is_dark {
        Color::Rgb { r: raw_bg.r.saturating_add(20), g: raw_bg.g.saturating_add(20), b: raw_bg.b.saturating_add(20) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(20), g: raw_bg.g.saturating_sub(20), b: raw_bg.b.saturating_sub(20) }
    };

    let selected_bg = if raw_bg.r < 128 {
        Color::Rgb { r: raw_bg.r.saturating_add(40), g: raw_bg.g.saturating_add(40), b: raw_bg.b.saturating_add(40) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(40), g: raw_bg.g.saturating_sub(40), b: raw_bg.b.saturating_sub(40) }
    };

    let get_theme_color = |keys: &[&str]| -> Option<Color> {
        for item in &theme.scopes {
            let scope_str = format!("{:?}", item.scope).to_lowercase();
            for key in keys {
                if scope_str.contains(key) {
                    if let Some(c) = item.style.foreground {
                        return Some(Color::Rgb { r: c.r, g: c.g, b: c.b });
                    }
                }
            }
        }
        None
    };

    let flag_a = Color::Green;
    let flag_d = Color::Magenta;
    let flag_n = Color::Yellow;
    let flag_star = Color::Red;

    let accent = get_theme_color(&["entity.name.function", "variable"])
        .unwrap_or(if is_dark { Color::Rgb { r: 100, g: 200, b: 255 } } else { Color::Rgb { r: 20, g: 100, b: 180 } });

    let date_color = get_theme_color(&["comment", "punctuation.definition.comment"])
        .unwrap_or(if is_dark { Color::Rgb { r: 120, g: 120, b: 120 } } else { Color::Rgb { r: 140, g: 140, b: 140 } });

    UiColors { bg, fg, ui_bg, selected_bg, accent, date_color, flag_n, flag_d, flag_a, flag_star, is_dark }
}

fn open_in_default_app(file_path: &Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", file_path.to_str().unwrap()]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(file_path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(file_path).spawn();
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

fn file_browser(stdout: &mut std::io::Stdout, rows: u16, cols: u16, colors: &UiColors) -> Option<String> {
    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut selected_idx = 0;

    loop {
        let mut entries = vec![];
        if current_dir.parent().is_some() {
            entries.push(("..".to_string(), current_dir.parent().unwrap().to_path_buf(), true));
        }

        if let Ok(read_dir) = fs::read_dir(&current_dir) {
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

        execute!(stdout, SetBackgroundColor(colors.bg), Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        let title = format!(" --- Browse to Attach: {} ", current_dir.display());
        let pad_len = (cols as usize).saturating_sub(title.chars().count());

        queue!(
            stdout, SetBackgroundColor(colors.ui_bg), SetForegroundColor(colors.accent),
            Print(title), Print(" ".repeat(pad_len)), ResetColor
        ).unwrap();

        let items_per_page = (rows.saturating_sub(3) as usize).max(1);
        let start_idx = if selected_idx >= items_per_page { selected_idx - items_per_page + 1 } else { 0 };

        for i in 0..items_per_page {
            let actual_idx = start_idx + i;
            if actual_idx < entries.len() {
                let entry = &entries[actual_idx];
                let prefix = if entry.2 { "[DIR]  " } else { "       " };
                let mut display_str = format!("{}{}", prefix, entry.0);
                if display_str.chars().count() > cols as usize {
                    display_str = display_str.chars().take((cols as usize).saturating_sub(3)).collect::<String>();
                }

                execute!(stdout, cursor::MoveTo(0, (i + 1) as u16)).unwrap();
                if actual_idx == selected_idx {
                    queue!(stdout, SetBackgroundColor(colors.ui_bg), SetForegroundColor(colors.fg), Print(display_str), ResetColor).unwrap();
                } else {
                    let fg = if entry.2 { colors.accent } else { colors.fg };
                    queue!(stdout, SetBackgroundColor(colors.bg), SetForegroundColor(fg), Print(display_str), ResetColor).unwrap();
                }
            }
        }

        let m_col = (cols as usize / 6).max(1);
        Editor::draw_menu_line(stdout, rows - 2, cols, m_col, &[("Up/Dn", " Nav"), ("Enter", " Select"), ("", ""), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg).unwrap();
        Editor::draw_menu_line(stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg).unwrap();
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

    if let Some(body) = default_body {
        editor.buffer = Rope::from_str(body);
    }

    let mut stdout = stdout();
    let mut final_body = String::new();
    let mut cancelled = false;

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));
        let theme = &editor.theme_set.themes[&editor.current_theme];
        let colors = derive_ui_colors(theme);

        for i in 0..6 {
            queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine)).unwrap();
        }

        let header_title = format!("Compose Email ({})", account.email);
        queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title)).unwrap();

        let fields = ["To:", "Cc:", "Bcc:", "Subject:"];
        let vals = [&state.to, &state.cc, &state.bcc, &state.subject];

        for i in 0..4 {
            queue!(
                stdout, cursor::MoveTo(0, (i + 1) as u16),
                SetBackgroundColor(colors.ui_bg), SetForegroundColor(colors.accent), Print(format!("{:>8}", fields[i])),
                SetForegroundColor(colors.fg), Print(" "), Print(vals[i])
            ).unwrap();
        }

        queue!(stdout, cursor::MoveTo(0, 5), SetBackgroundColor(colors.ui_bg), SetForegroundColor(colors.accent), Print(" Attach: "), SetForegroundColor(colors.fg)).unwrap();

        if state.attachments.is_empty() {
            let dim_c = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
            queue!(stdout, SetForegroundColor(dim_c), Print("(Press ^T to attach a file)")).unwrap();
        } else {
            let att_names: Vec<String> = state.attachments.iter().enumerate().map(|(i, p)| {
                let fname = Path::new(p).file_name().unwrap_or_default().to_string_lossy();
                format!("{}. {}", i + 1, fname)
            }).collect();
            queue!(stdout, Print(att_names.join("   "))).unwrap();
        }
        queue!(stdout, ResetColor).unwrap();

        editor.draw_screen().unwrap();

        if state.active_idx < 4 {
            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[("^P", " Prev"), ("Tab", " Next"), ("^T", " Attach"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg).unwrap();
            Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("Enter", " Body"), ("^X", " Send"), ("", ""), ("", ""), ("", "")], colors.ui_bg, colors.accent, colors.fg).unwrap();

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
                            cancelled = true;
                            break;
                        }
                    }

                    if state.active_idx == 4 {
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
                                if let Some(path) = file_browser(&mut stdout, rows, cols, &colors) { state.attachments.push(path); }
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

    // Flash status message instantly to give user feedback before blocking
    let (_, rows) = term_size().unwrap_or((80, 24));
    let theme = &editor.theme_set.themes[&editor.current_theme];
    let colors = derive_ui_colors(theme);
    queue!(stdout, cursor::MoveTo(0, rows - 1), SetBackgroundColor(colors.selected_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(" Sending message... Please wait "), ResetColor).unwrap();
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
            let filename = Path::new(att).file_name().unwrap_or_default().to_string_lossy().into_owned();
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
                Ok(_) => return Some("Message Sent".to_string()),
                Err(e) => {
                    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                    queue!(stdout, Print(format!("-> Failed to send message: {:?}\r\n", e))).unwrap();
                    queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
                    stdout.flush().unwrap();
                    loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                    return None;
                }
            }
        }
        Err(e) => {
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap();
            queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
            stdout.flush().unwrap();
            loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
            return None;
        }
    }
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

    let mut list_status = String::new();
    let mut list_status_time: Option<Instant> = None;
    let mut list_status_duration = Duration::from_secs(3);

    let mut last_fetch_time = Instant::now();
    let auto_refresh_interval = Duration::from_secs(60);

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();

    execute!(stdout, EnterAlternateScreen).unwrap();

    let mut theme_provider = Editor::new(None);

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));
        let theme = &theme_provider.theme_set.themes[&theme_provider.current_theme];
        let colors = derive_ui_colors(theme);

        if last_fetch_time.elapsed() >= auto_refresh_interval {
            needs_fetch = true;
        }

        if needs_reconnect {
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            active_account = config.accounts[current_account_idx].clone();

            queue!(stdout, Print(format!("Switching to Account: {}...\r\n", active_account.email))).unwrap();
            stdout.flush().unwrap();

            let _ = session.logout();

            let new_client = imap::connect((domain, 993), domain, &tls).expect("Could not connect to IMAP server");
            session = new_client.login(&active_account.email, &active_account.password).expect("IMAP Login failed");

            match session.select("INBOX") {
                Ok(m) => total_messages = m.exists,
                Err(_) => total_messages = 0,
            }
            current_page = 0;
            selected_index = 0;
            needs_fetch = true;
            needs_reconnect = false;
            last_fetch_time = Instant::now();
        }

        let items_per_page = (rows.saturating_sub(3) as u32).max(1);
        let total_pages = if total_messages == 0 { 1 } else { (total_messages + items_per_page - 1) / items_per_page };

        if current_page >= total_pages {
            current_page = total_pages.saturating_sub(1);
            needs_fetch = true;
        }

        if needs_fetch {
            page_emails.clear();

            match session.select("INBOX") {
                Ok(m) => total_messages = m.exists,
                Err(_) => {
                    needs_reconnect = true;
                    continue;
                }
            }

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
                        let mut is_answered = false;

                        for flag in message.flags() {
                            match flag {
                                imap::types::Flag::Seen => is_seen = true,
                                imap::types::Flag::Deleted => is_deleted = true,
                                imap::types::Flag::Flagged => is_flagged = true,
                                imap::types::Flag::Answered => is_answered = true,
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

                        page_emails.push(EmailMeta { id: message.message, subject, from, reply_to, reply_to_display, to_addr, cc, date, size, is_read: is_seen, is_deleted, is_flagged, is_answered });
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
            last_fetch_time = Instant::now();
            needs_fetch = false;
        }

        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        if let AppMode::Reading { text_body, html_body, attachments } = &mode {
            let email_from = page_emails[selected_index].from.clone();
            let email_to = page_emails[selected_index].to_addr.clone();
            let email_cc = page_emails[selected_index].cc.clone();
            let email_subject = page_emails[selected_index].subject.clone();
            let email_reply_to = page_emails[selected_index].reply_to.clone();
            let email_id = page_emails[selected_index].id.to_string();
            let email_date = page_emails[selected_index].date.clone();

            let mut reader = Editor::new(None);
            reader.menu_state = MenuState::EmailReader;
            reader.top_margin = 6;
            reader.buffer = Rope::from_str(text_body.as_str());

            reader.current_theme = theme_provider.current_theme.clone();

            loop {
                let r_theme = &reader.theme_set.themes[&reader.current_theme];
                let r_colors = derive_ui_colors(r_theme);

                for i in 0..6 { queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(r_colors.ui_bg), terminal::Clear(ClearType::UntilNewLine)).unwrap(); }

                let header_title = format!("View Email ({})", active_account.email);
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
                    let att_names: Vec<String> = attachments.iter().enumerate().map(|(i, (n, _))| format!("{}. {}", i + 1, n)).collect();
                    queue!(stdout, SetForegroundColor(r_colors.flag_n), Print(att_names.join("   "))).unwrap();
                }
                queue!(stdout, ResetColor).unwrap();

                reader.draw_screen().unwrap();

                let m_col = (cols as usize / 6).max(1);
                Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[
                    ("Up/Dn", " Scroll"), ("Esc", " Mailbox"), (">", " Browser"), ("R", " Reply"), ("F", " Forward"), ("V", " Save Atts")
                ], r_colors.ui_bg, r_colors.accent, r_colors.fg).unwrap();
                Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[
                    ("PgUp/Dn", " Page"), ("1-9", " Open Att"), ("", ""), ("", ""), ("", ""), ("", "")
                ], r_colors.ui_bg, r_colors.accent, r_colors.fg).unwrap();
                stdout.flush().unwrap();

                let timeout = if let Some(time) = reader.status_time {
                    let elapsed = time.elapsed();
                    if elapsed >= Duration::from_secs(3) { Duration::from_millis(1) } else { Duration::from_secs(3) - elapsed }
                } else { Duration::from_secs(3600) };

                if event::poll(timeout).unwrap() {
                    if let Event::Key(mut key) = event::read().unwrap() {
                        if key.kind == KeyEventKind::Press {

                            if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) {
                                if let KeyCode::Char(c) = key.code {
                                    if c.is_ascii_digit() && c != '0' {
                                        let idx = c.to_digit(10).unwrap() as usize - 1;
                                        if idx < attachments.len() {
                                            let (filename, data) = &attachments[idx];
                                            let temp_dir = std::env::temp_dir();
                                            let safe_name = filename.replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '-', "_");
                                            let file_path = temp_dir.join(&safe_name);

                                            if fs::write(&file_path, data).is_ok() {
                                                open_in_default_app(&file_path);
                                                reader.set_status(format!("Opened '{}' in default program.", safe_name));
                                            } else {
                                                reader.set_status(format!("Failed to open '{}'.", safe_name));
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }

                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                                    key.code = KeyCode::PageUp;
                                    key.modifiers = KeyModifiers::empty();
                                } else if key.code == KeyCode::Char('v') || key.code == KeyCode::Char('V') {
                                    key.code = KeyCode::PageDown;
                                    key.modifiers = KeyModifiers::empty();
                                }
                            } else {
                                if key.code == KeyCode::Char('>') {
                                    if let Some(html) = &html_body {
                                        let temp_dir = std::env::temp_dir();
                                        let file_path = temp_dir.join("xpine_email.html");
                                        if fs::write(&file_path, html).is_ok() {
                                            open_in_default_app(&file_path);
                                            reader.set_status(String::from("Opened HTML version in default browser."));
                                        }
                                    } else {
                                        reader.set_status(String::from("No HTML body found for this email."));
                                    }
                                    continue;
                                }

                                if key.code == KeyCode::Char('v') || key.code == KeyCode::Char('V') {
                                    if !attachments.is_empty() {
                                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                                        queue!(stdout, Print("Saving attachments...\r\n")).unwrap();
                                        for (filename, data) in attachments {
                                            let safe_name = filename.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-', "_");
                                            let filepath = Path::new(&safe_name);
                                            match fs::write(filepath, data) {
                                                Ok(_) => queue!(stdout, Print(format!("Saved: {}\r\n", safe_name))).unwrap(),
                                                Err(e) => queue!(stdout, Print(format!("Failed to save {}: {}\r\n", safe_name, e))).unwrap(),
                                            }
                                        }
                                        queue!(stdout, Print("\r\nPress Enter to return...")).unwrap();
                                        stdout.flush().unwrap();
                                        loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
                                        reader.set_status(String::from("Attachments saved."));
                                    } else {
                                        reader.set_status(String::from("No attachments found."));
                                    }
                                    continue;
                                }
                            }

                            match reader.handle_keypress(key).unwrap() {
                                EditorResult::Cancel => break,
                                EditorResult::Send(action) if action == "REPLY" => {
                                    if session.store(&email_id, "+FLAGS (\\Answered)").is_ok() {
                                        page_emails[selected_index].is_answered = true;
                                    }
                                    let subject = if email_subject.to_lowercase().starts_with("re:") { email_subject.clone() } else { format!("Re: {}", email_subject) };
                                    if let Some(msg) = compose_email(&active_account, Some(&email_reply_to), Some(&subject), None, &mut reader.current_theme) {
                                        list_status = msg;
                                        list_status_time = Some(Instant::now());
                                        list_status_duration = Duration::from_millis(1500);
                                    }
                                    break;
                                }
                                EditorResult::Send(action) if action == "FORWARD" => {
                                    let subject = if email_subject.to_lowercase().starts_with("fwd:") { email_subject.clone() } else { format!("Fwd: {}", email_subject) };
                                    let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", email_from, email_date, email_subject, text_body);
                                    if let Some(msg) = compose_email(&active_account, None, Some(&subject), Some(&fwd_body), &mut reader.current_theme) {
                                        list_status = msg;
                                        list_status_time = Some(Instant::now());
                                        list_status_duration = Duration::from_millis(1500);
                                    }
                                    break;
                                }
                                _ => {}
                            }

                            theme_provider.current_theme = reader.current_theme.clone();
                        }
                    }
                } else {
                    reader.clear_status();
                }
            }
            mode = AppMode::List;
            continue;
        }

        queue!(stdout, SetBackgroundColor(colors.bg), Clear(ClearType::All)).unwrap();

        let header_title = format!("xpine - Inbox ({})", active_account.email);
        queue!(stdout, cursor::MoveTo(0, 0), SetBackgroundColor(colors.ui_bg), terminal::Clear(ClearType::UntilNewLine), cursor::MoveTo(0, 0), SetForegroundColor(colors.accent), Print(header_title), ResetColor).unwrap();

        let list_start_y = 1;
        let visible_capacity = rows.saturating_sub(3) as usize;

        for (i, email) in page_emails.iter().enumerate() {
            if i >= visible_capacity { break; }

            let row_y = (i + list_start_y) as u16;
            let row_bg = if i == selected_index { colors.selected_bg } else { colors.bg };

            queue!(stdout, cursor::MoveTo(0, row_y), SetBackgroundColor(row_bg), Clear(ClearType::UntilNewLine)).unwrap();

            let flag_char = if email.is_flagged { "*" } else { " " };
            let status_char = if email.is_deleted { "D" } else if !email.is_read { "N" } else if email.is_answered { "A" } else { " " };

            let size_kb = (email.size / 1024).max(1) as f32;
            let size_str = if size_kb < 1024.0 {
                format!("({}K)", size_kb as u32)
            } else {
                format!("({}M)", (size_kb / 1024.0) as u32)
            };
            let size_display = format!("{:>6}", size_str);
            let heat = (size_kb.log2() / 12.3).min(1.0).max(0.0);

            let (base_r, base_g, base_b) = match colors.fg {
                Color::Rgb { r, g, b } => (r as f32, g as f32, b as f32),
                _ => (255.0, 255.0, 255.0),
            };

            let hot_r = if colors.is_dark { 255.0 } else { 220.0 };
            let hot_g = if colors.is_dark { 80.0 } else { 0.0 };
            let hot_b = if colors.is_dark { 80.0 } else { 0.0 };

            let size_color = Color::Rgb {
                r: (base_r + (hot_r - base_r) * heat) as u8,
                g: (base_g + (hot_g - base_g) * heat) as u8,
                b: (base_b + (hot_b - base_b) * heat) as u8,
            };

            let from_width = 22;
            let from_str = format!("{:<width$}", email.from.chars().take(from_width).collect::<String>(), width = from_width);

            let date_width = 9;
            let date_str = format!("{:<width$}", email.date, width = date_width);

            let fixed_width = 47;
            let subject_width = (cols as usize).saturating_sub(fixed_width);
            let subj_truncated = email.subject.chars().take(subject_width).collect::<String>();
            let padded_subj = format!("{:<width$}", subj_truncated, width = subject_width);

            let _ = queue!(stdout, SetBackgroundColor(row_bg));

            let _ = queue!(stdout, SetForegroundColor(colors.flag_star), Print(flag_char));
            let _ = queue!(stdout, Print(" "));

            let status_color = match status_char {
                "N" => colors.flag_n,
                "D" => colors.flag_d,
                "A" => colors.flag_a,
                _ => colors.fg,
            };

            let _ = queue!(stdout, SetForegroundColor(status_color), Print(status_char));
            let _ = queue!(stdout, Print(" "));

            let _ = queue!(stdout, SetForegroundColor(colors.date_color), Print(date_str));
            let _ = queue!(stdout, Print("  "));

            let _ = queue!(stdout, SetForegroundColor(colors.fg), Print(from_str));
            let _ = queue!(stdout, Print("  "));

            let _ = queue!(stdout, Print(padded_subj));
            let _ = queue!(stdout, Print("  "));

            let _ = queue!(stdout, SetForegroundColor(size_color), Print(size_display));
        }

        let r_col = (cols as usize / 6).max(1);
        Editor::draw_menu_line(&mut stdout, rows - 2, cols, r_col, &[
            ("Enter", " Read"), ("C", " Compose"), ("R", " Reply"), ("F", " Forward"), ("D", " Delete"), ("*", " Flag")
        ], colors.ui_bg, colors.accent, colors.fg).unwrap();

        Editor::draw_menu_line(&mut stdout, rows - 1, cols, r_col, &[
            ("Q/Esc", " Quit"), ("Arrows", " Nav"), ("Tab", " Acct"), ("M", " Mark Read"), ("X", " Expunge"), ("", "")
        ], colors.ui_bg, colors.accent, colors.fg).unwrap();

        if let Some(time) = list_status_time {
            if time.elapsed() >= list_status_duration {
                list_status.clear();
                list_status_time = None;
            } else if !list_status.is_empty() {
                queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), terminal::Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(format!(" {} ", list_status)), ResetColor).unwrap();
            }
        }

        stdout.flush().unwrap();

        let mut timeout = if last_fetch_time.elapsed() >= auto_refresh_interval {
            Duration::from_millis(1)
        } else {
            auto_refresh_interval - last_fetch_time.elapsed()
        };

        if let Some(time) = list_status_time {
            let elapsed = time.elapsed();
            if elapsed >= list_status_duration {
                timeout = Duration::from_millis(1);
            } else {
                let status_timeout = list_status_duration - elapsed;
                if status_timeout < timeout {
                    timeout = status_timeout;
                }
            }
        }

        if event::poll(timeout).unwrap() {
            match event::read().expect("Failed to read event") {
                Event::Key(key_event) => {
                    if key_event.kind == KeyEventKind::Press {
                        match key_event.code {
                            KeyCode::Char('t') | KeyCode::Char('T') if key_event.modifiers.contains(KeyModifiers::ALT) => {
                                let mut themes: Vec<_> = theme_provider.theme_set.themes.keys().cloned().collect();
                                themes.sort();
                                if let Some(pos) = themes.iter().position(|t| t == &theme_provider.current_theme) {
                                    let next = (pos + 1) % themes.len();
                                    theme_provider.current_theme = themes[next].clone();
                                    list_status = format!("Theme: {}", theme_provider.current_theme);
                                    list_status_time = Some(Instant::now());
                                    list_status_duration = Duration::from_secs(3);
                                }
                            }
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
                                    let max_visible = page_emails.len().min(rows.saturating_sub(3) as usize);
                                    if selected_index + 1 < max_visible { selected_index += 1; }
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
                                if let Some(msg) = compose_email(&active_account, None, None, None, &mut theme_provider.current_theme) {
                                    list_status = msg;
                                    list_status_time = Some(Instant::now());
                                    list_status_duration = Duration::from_millis(1500);
                                }
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
                                        if let Ok(m) = session.select("INBOX") {
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
                                    if let Some(msg) = compose_email(&active_account, None, Some(&subject), Some(&fwd_body), &mut theme_provider.current_theme) {
                                        list_status = msg;
                                        list_status_time = Some(Instant::now());
                                        list_status_duration = Duration::from_millis(1500);
                                    }
                                }
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                if !page_emails.is_empty() {
                                    let seq_id = page_emails[selected_index].id.to_string();
                                    let reply_to = page_emails[selected_index].reply_to.clone();
                                    let mut subject = page_emails[selected_index].subject.clone();

                                    if session.store(&seq_id, "+FLAGS (\\Answered)").is_ok() {
                                        page_emails[selected_index].is_answered = true;
                                    }

                                    if !subject.to_lowercase().starts_with("re:") {
                                        subject = format!("Re: {}", subject);
                                    }
                                    if let Some(msg) = compose_email(&active_account, Some(&reply_to), Some(&subject), None, &mut theme_provider.current_theme) {
                                        list_status = msg;
                                        list_status_time = Some(Instant::now());
                                        list_status_duration = Duration::from_millis(1500);
                                    }
                                }
                            }
                            KeyCode::Enter | KeyCode::Right => {
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
    }

    execute!(stdout, LeaveAlternateScreen).unwrap();
    disable_raw_mode().expect("Failed to disable raw mode");
    let _ = session.logout();
}