use crate::config::{derive_ui_colors, Account, UiColors};
use crate::editor::{Editor, EditorResult, MenuState};

use lettre::transport::smtp::authentication::Credentials as SmtpCredentials;
use lettre::{Message, SmtpTransport, Transport};
use ropey::Rope;
use std::fs;
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType, size as term_size},
};
use crate::ui::UiExt;

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attachments: Vec<String>,
    active_idx: usize,
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
            queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(colors.ui_bg), Clear(ClearType::UntilNewLine)).unwrap();
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

    let (_, rows) = term_size().unwrap_or((80, 24));
    let theme = &editor.theme_set.themes[&editor.current_theme];
    let colors = derive_ui_colors(theme);
    queue!(stdout, cursor::MoveTo(0, rows - 1), SetBackgroundColor(colors.selected_bg), Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print(" Sending message... Please wait "), ResetColor).unwrap();
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
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            queue!(stdout, Print(format!("-> Failed to build message: {:?}\r\n", e))).unwrap();
            queue!(stdout, Print("\r\nPress Enter to return to the mailbox...")).unwrap();
            stdout.flush().unwrap();
            loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }
            None
        }
    }
}
