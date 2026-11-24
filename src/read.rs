use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{size as term_size, Clear, ClearType},
};
use ropey::Rope;

use crate::app::{App, AppMode};
use crate::browser::BrowserExt;
use crate::editor::{Editor, EditorResult, MenuState};
use crate::net::MailSession;
use crate::prompt::PromptExt;
use crate::ui::UiExt;
use crate::{address, compose, mail, net, ui};

struct ReaderEmail {
    text_body: String,
    html_body: Option<String>,
    attachments: Vec<(String, Vec<u8>)>,
    from: String,
    to: String,
    cc: String,
    subject: String,
    reply_to: String,
    date: String,
    fetch_seq: String,
}

impl ReaderEmail {
    fn from_current(
        app: &App,
        text_body: String,
        html_body: Option<String>,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> Self {
        let meta = &app.page_emails[app.selected_index];
        Self {
            text_body,
            html_body,
            attachments,
            from: meta.from.clone(),
            to: meta.to_addr.clone(),
            cc: meta.cc.clone(),
            subject: meta.subject.clone(),
            reply_to: meta.reply_to.clone(),
            date: meta.full_date.clone(),
            fetch_seq: meta.id.to_string(),
        }
    }

    fn apply_to_reader(&self, reader: &mut Editor, cols: u16) {
        let attach_lines = if self.attachments.is_empty() {
            1
        } else {
            1 + self.attachments.len()
        };
        reader.top_margin = (5 + attach_lines) as u16;

        let wrap_width = (cols as usize).saturating_sub(2);
        let wrapped_text = mail::wrap_email_body(&self.text_body, wrap_width);
        reader.buffer = Rope::from_str(&wrapped_text);
        reader.row_offset = 0;
        reader.col_offset = 0;
        reader.cursor_y = 0;
        reader.cursor_x = 0;
        reader.desired_cursor_x = 0;
        reader.highlight_cache.clear();

        if self.html_body.as_deref().is_some_and(|h| !h.is_empty()) {
            reader.set_status("Email contains HTML. Type 'B' to view in browser".to_string());
        } else {
            reader.clear_status();
        }
    }
}

fn print_colorized_emails(
    stdout: &mut Stdout,
    text: &str,
    name_color: Color,
    email_color: Color,
    mut current_y: u16,
    menu_bg: Color,
    is_expanded: bool,
    max_len: Option<usize>,
) -> io::Result<u16> {
    let mut tokens = Vec::new();
    let mut current_idx = 0;

    while let Some(start) = text[current_idx..].find('<') {
        let absolute_start = current_idx + start;
        let chunk = &text[current_idx..absolute_start];

        let split_pos = if let Some(last_at) = chunk.rfind('@') {
            chunk[last_at..]
                .find(',')
                .map_or(0, |comma_offset| last_at + comma_offset + 1)
        } else if chunk.trim_start().starts_with(',') {
            chunk.find(',').unwrap() + 1
        } else {
            0
        };

        let plain_emails_part = &chunk[..split_pos];
        let name_part = &chunk[split_pos..];

        if !plain_emails_part.is_empty() {
            tokens.push((plain_emails_part.to_string(), email_color));
        }

        let trimmed_name = name_part.trim();
        if !trimmed_name.is_empty() {
            let start_ws = name_part.find(|c: char| !c.is_whitespace()).unwrap();
            let end_ws = start_ws + trimmed_name.len();
            let left_space = &name_part[..start_ws];
            let clean_name = trimmed_name.trim_matches('"');
            let right_space = &name_part[end_ws..];
            tokens.push((
                format!("{}\"{}\"{}", left_space, clean_name, right_space),
                name_color,
            ));
        } else if !name_part.is_empty() {
            tokens.push((name_part.to_string(), name_color));
        }

        if let Some(end) = text[absolute_start..].find('>') {
            let absolute_end = absolute_start + end + 1;
            tokens.push((text[absolute_start..absolute_end].to_string(), email_color));
            current_idx = absolute_end;
        } else {
            current_idx = absolute_start;
            break;
        }
    }

    if current_idx < text.len() {
        tokens.push((text[current_idx..].to_string(), email_color));
    }

    let total_chars: usize = tokens.iter().map(|(s, _)| s.chars().count()).sum();
    let safe_limit = max_len
        .filter(|&limit| total_chars > limit)
        .map(|limit| limit.saturating_sub(3));
    let mut chars_printed = 0;

    for (text_chunk, color) in tokens {
        queue!(stdout, SetForegroundColor(color))?;

        if is_expanded {
            for (idx, part) in text_chunk.split('\n').enumerate() {
                if idx > 0 {
                    current_y += 1;
                    queue!(
                        stdout,
                        cursor::MoveTo(0, current_y),
                        SetBackgroundColor(menu_bg),
                        Clear(ClearType::UntilNewLine),
                        cursor::MoveTo(9, current_y),
                        SetForegroundColor(color)
                    )?;
                }
                queue!(stdout, Print(part))?;
            }
        } else if let Some(limit) = safe_limit {
            let chunk_chars = text_chunk.chars().count();
            if chars_printed + chunk_chars > limit {
                let allowed = limit.saturating_sub(chars_printed);
                let truncated: String = text_chunk.chars().take(allowed).collect();
                queue!(
                    stdout,
                    Print(truncated),
                    SetForegroundColor(email_color),
                    Print("...")
                )?;
                break;
            }
            queue!(stdout, Print(&text_chunk))?;
            chars_printed += chunk_chars;
        } else {
            queue!(stdout, Print(&text_chunk))?;
        }
    }

    Ok(current_y)
}

fn render_headers(
    stdout: &mut Stdout,
    cols: u16,
    reader: &Editor,
    active_email: &str,
    is_deleted: bool,
    email: &ReaderEmail,
    expand_headers: bool,
) -> io::Result<u16> {
    let r_theme = &reader.theme_set.themes[&reader.current_theme];
    let r_colors = ui::derive_ui_colors(r_theme);

    for row in 0..reader.top_margin {
        queue!(
            stdout,
            cursor::MoveTo(0, row),
            SetBackgroundColor(r_colors.menu_bg),
            Clear(ClearType::UntilNewLine)
        )?;
    }

    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        SetForegroundColor(r_colors.accent),
        Print(format!("View Email ({})", active_email))
    )?;

    if is_deleted {
        queue!(
            stdout,
            cursor::MoveTo(cols.saturating_sub(8), 0),
            SetBackgroundColor(Color::Red),
            SetForegroundColor(r_colors.fg),
            Print("[Delete]")
        )?;
    }

    let fields = [
        ("Date:", email.date.as_str(), false),
        ("From:", email.from.as_str(), true),
        ("To:", email.to.as_str(), true),
        ("Cc:", email.cc.as_str(), true),
        ("Subject:", email.subject.as_str(), false),
    ];

    let max_len = (cols as usize).saturating_sub(10);
    let mut current_y = 1;

    for (label, val, is_email_field) in fields {
        if !expand_headers {
            queue!(
                stdout,
                cursor::MoveTo(0, current_y),
                SetBackgroundColor(r_colors.menu_bg),
                SetForegroundColor(r_colors.accent),
                Print(format!("{:>8}", label)),
                SetForegroundColor(r_colors.fg),
                Print(" ")
            )?;

            if is_email_field {
                print_colorized_emails(
                    stdout,
                    val,
                    r_colors.date_color,
                    r_colors.fg,
                    current_y,
                    r_colors.menu_bg,
                    false,
                    Some(max_len),
                )?;
            } else if val.chars().count() > max_len {
                let truncated: String = val.chars().take(max_len.saturating_sub(3)).collect();
                queue!(stdout, Print(format!("{}...", truncated)))?;
            } else {
                queue!(stdout, Print(val))?;
            }

            queue!(stdout, Clear(ClearType::UntilNewLine))?;
            current_y += 1;
        } else {
            let wrapped_val = if val.is_empty() {
                String::new()
            } else {
                mail::wrap_email_body(val, max_len)
            };

            if is_email_field {
                queue!(
                    stdout,
                    cursor::MoveTo(0, current_y),
                    SetBackgroundColor(r_colors.menu_bg),
                    Clear(ClearType::UntilNewLine),
                    SetForegroundColor(r_colors.accent),
                    Print(format!("{:>8}", label)),
                    SetForegroundColor(r_colors.fg),
                    Print(" ")
                )?;
                current_y = print_colorized_emails(
                    stdout,
                    &wrapped_val,
                    r_colors.date_color,
                    r_colors.fg,
                    current_y,
                    r_colors.menu_bg,
                    true,
                    None,
                )? + 1;
            } else {
                let lines: Vec<&str> = if wrapped_val.is_empty() {
                    vec![""]
                } else {
                    wrapped_val.lines().collect()
                };
                for (line_idx, line) in lines.iter().enumerate() {
                    queue!(
                        stdout,
                        cursor::MoveTo(0, current_y),
                        SetBackgroundColor(r_colors.menu_bg),
                        Clear(ClearType::UntilNewLine)
                    )?;
                    if line_idx == 0 {
                        queue!(
                            stdout,
                            SetForegroundColor(r_colors.accent),
                            Print(format!("{:>8}", label)),
                            SetForegroundColor(r_colors.fg),
                            Print(" "),
                            Print(line)
                        )?;
                    } else {
                        queue!(
                            stdout,
                            cursor::MoveTo(9, current_y),
                            SetForegroundColor(r_colors.fg),
                            Print(line)
                        )?;
                    }
                    current_y += 1;
                }
            }
        }
    }

    let attach_hint = if email.attachments.is_empty() {
        "None"
    } else {
        "'1' to open, 'Meta+1' to save, 'Meta+0' to save all"
    };
    queue!(
        stdout,
        cursor::MoveTo(0, current_y),
        SetBackgroundColor(r_colors.menu_bg),
        SetForegroundColor(r_colors.accent),
        Print(" Attach: "),
        SetForegroundColor(r_colors.date_color),
        Print(attach_hint),
        Clear(ClearType::UntilNewLine)
    )?;
    current_y += 1;

    if !email.attachments.is_empty() {
        let att_color = if r_colors.is_dark {
            Color::Rgb { r: 255, g: 80, b: 80 }
        } else {
            Color::Rgb { r: 220, g: 0, b: 0 }
        };

        for (i, (name, data)) in email.attachments.iter().enumerate() {
            let size_kb = (data.len() as f32 / 1024.0).max(1.0);
            let size_str = if size_kb < 1024.0 {
                format!("{:.0}K", size_kb)
            } else {
                format!("{:.1}M", size_kb / 1024.0)
            };
            queue!(
                stdout,
                cursor::MoveTo(0, current_y),
                SetBackgroundColor(r_colors.menu_bg),
                SetForegroundColor(att_color),
                Print(format!("         {}. {} ({})", i + 1, name, size_str)),
                Clear(ClearType::UntilNewLine)
            )?;
            current_y += 1;
        }
    }

    queue!(stdout, ResetColor)?;
    Ok(current_y)
}

fn reply_to_email(
    app: &mut App,
    session: &mut Option<MailSession>,
    email: &ReaderEmail,
    active_email: &str,
    reader: &mut Editor,
    reply_all: bool,
) {
    let reply_body = mail::format_reply_text(&email.text_body, &email.date, &email.from);
    let sub = if email.subject.to_lowercase().starts_with("re:") {
        email.subject.clone()
    } else {
        format!("Re: {}", email.subject)
    };
    let raw_reply = if email.reply_to.trim().is_empty()
        || mail::extract_email(&email.reply_to) == mail::extract_email(&email.from)
    {
        mail::clean_display_addresses(&email.from)
    } else {
        mail::clean_display_addresses(&email.reply_to)
    };

    let (final_to, final_cc) = if reply_all {
        let (all_to, all_cc) =
            mail::build_reply_all_addresses(active_email, &raw_reply, &email.to, &email.cc);
        (all_to, Some(all_cc))
    } else {
        (raw_reply, None)
    };

    if let Some(status) = compose::compose_email(
        &app.active_account,
        Some(&final_to),
        final_cc.as_deref(),
        Some(&sub),
        Some(&reply_body),
        &mut reader.current_theme,
    ) {
        if let Some(MailSession::Imap(imap_sess)) = session.as_mut() {
            let _ = imap_sess.store(&email.fetch_seq, "+FLAGS (\\Answered)");
        }
        app.page_emails[app.selected_index].is_answered = true;
        reader.set_status(status);
    }
}

fn navigate_adjacent_email(
    app: &mut App,
    session: &mut Option<MailSession>,
    settings_provider: &Editor,
    reader: &mut Editor,
    email: &mut ReaderEmail,
    stdout: &mut Stdout,
    is_next: bool,
) {
    let sort_newest = settings_provider.sort_newest_first;
    let (cols, rows) = term_size().unwrap_or((80, 24));
    let items_per_page = (rows.saturating_sub(3) as u32).max(1);
    let total_pages = if app.total_messages == 0 {
        1
    } else {
        (app.total_messages + items_per_page - 1) / items_per_page
    };
    let max_idx = (items_per_page as usize).saturating_sub(1);

    let prev_page = app.current_page;
    let prev_index = app.selected_index;
    let mut can_move = false;
    let mut fetch_new_page = false;

    if is_next {
        if app.selected_index + 1 < app.page_emails.len() {
            app.selected_index += 1;
            can_move = true;
        } else if sort_newest {
            if app.current_page + 1 < total_pages {
                app.current_page += 1;
                app.selected_index = 0;
                fetch_new_page = true;
            } else {
                reader.set_status("This is the oldest email".to_string());
            }
        } else if app.current_page > 0 {
            app.current_page -= 1;
            app.selected_index = 0;
            fetch_new_page = true;
        } else {
            reader.set_status("This is the newest email".to_string());
        }
    } else if app.selected_index > 0 {
        app.selected_index -= 1;
        can_move = true;
    } else if sort_newest {
        if app.current_page > 0 {
            app.current_page -= 1;
            app.selected_index = max_idx;
            fetch_new_page = true;
        } else {
            reader.set_status("Already at the newest email.".to_string());
        }
    } else if app.current_page + 1 < total_pages {
        app.current_page += 1;
        app.selected_index = max_idx;
        fetch_new_page = true;
    } else {
        reader.set_status("Already at the oldest email.".to_string());
    }

    if fetch_new_page {
        if let Some(sess) = session.as_mut() {
            let loading_msg = if is_next {
                "Fetching next page..."
            } else {
                "Fetching previous page..."
            };
            reader.set_status(loading_msg.to_string());
            reader.draw_screen().unwrap();

            net::fetch_emails(
                sess,
                app,
                items_per_page,
                sort_newest,
                settings_provider.use_dmy_dates,
            );

            if !app.page_emails.is_empty() {
                if app.selected_index >= app.page_emails.len() {
                    app.selected_index = app.page_emails.len().saturating_sub(1);
                }
                can_move = true;
            } else {
                app.current_page = prev_page;
                app.selected_index = prev_index;
                reader.set_status("Failed to fetch page.".to_string());
            }
        }
    }

    if !can_move {
        return;
    }

    if let Some(sess) = session.as_mut() {
        let next_seq = app.page_emails[app.selected_index].id.to_string();

        if !app.page_emails[app.selected_index].is_read {
            match sess {
                MailSession::Imap(_) => app.page_emails[app.selected_index].is_read = true,
                MailSession::Graph { .. } => net::toggle_flag(sess, app, "\\Seen"),
            }
        }

        reader.set_status("Fetching email...".to_string());
        reader.draw_screen().unwrap();

        let (t_body, h_body, atts) = net::fetch_email_body(sess, &next_seq);
        *email = ReaderEmail::from_current(app, t_body, h_body, atts);
        queue!(stdout, Clear(ClearType::All)).unwrap();
        email.apply_to_reader(reader, cols);
    }
}

fn toggle_delete(app: &mut App, session: &mut Option<MailSession>, reader: &mut Editor) {
    if app.page_emails.is_empty() {
        return;
    }

    let is_outlook = app.active_account.imap_server.to_lowercase().contains("outlook")
        || app.active_account.email.to_lowercase().contains("outlook")
        || app.active_account.email.to_lowercase().contains("hotmail");

    let toggled = if is_outlook {
        let entry = &mut app.page_emails[app.selected_index];
        entry.is_deleted = !entry.is_deleted;
        true
    } else if let Some(sess) = session.as_mut() {
        net::toggle_flag(sess, app, "\\Deleted");
        true
    } else {
        false
    };

    if toggled {
        let entry = &app.page_emails[app.selected_index];
        if entry.is_deleted {
            app.graph_pending_deleted.insert(entry.id.clone());
        } else {
            app.graph_pending_deleted.remove(&entry.id);
        }
    }

    let status_msg = if app.page_emails[app.selected_index].is_deleted {
        "Message marked for deletion"
    } else {
        "Message un-marked for deletion"
    };
    reader.set_status(status_msg.to_string());
}

fn collect_email_addresses(email: &ReaderEmail) -> Vec<String> {
    let combined = format!("{}, {}, {}", email.from, email.to, email.cc);
    let mut addrs = Vec::new();
    let mut current = String::new();
    let mut in_angle_brackets = false;

    for c in combined.chars() {
        match c {
            '<' => {
                in_angle_brackets = true;
                current.push(c);
            }
            '>' => {
                in_angle_brackets = false;
                current.push(c);
            }
            ',' if !in_angle_brackets && current.contains('@') => {
                let trimmed = current.trim().to_string();
                if trimmed.chars().any(char::is_alphanumeric) {
                    addrs.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    let final_trim = current.trim().to_string();
    if final_trim.chars().any(char::is_alphanumeric) {
        addrs.push(final_trim);
    }

    addrs.sort();
    addrs.dedup();
    addrs
}

fn temp_attachments_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("xpine_attachments");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn open_in_browser(email: &ReaderEmail, reader: &mut Editor) {
    let temp_dir = temp_attachments_dir();

    if let Some(html) = email.html_body.as_deref().filter(|h| !h.is_empty()) {
        let file_path = temp_dir.join("email_view.html");
        if fs::write(&file_path, html).is_ok()
            && file_path
            .to_str()
            .is_some_and(|p| webbrowser::open(p).is_ok())
        {
            reader.set_status("Opened HTML version in browser.".to_string());
            return;
        }
    }

    let file_path = temp_dir.join("email_view.txt");
    if fs::write(&file_path, &email.text_body).is_err() {
        reader.set_status("Failed to save text file.".to_string());
    } else if file_path
        .to_str()
        .is_some_and(|p| webbrowser::open(p).is_ok())
    {
        reader.set_status("Opened text version in browser.".to_string());
    } else {
        reader.set_status("Failed to open browser.".to_string());
    }
}

fn save_all_attachments(attachments: &[(String, Vec<u8>)], reader: &mut Editor) {
    let Ok(Some(save_dir)) = reader.run_file_browser(true, Some("<DIR_ONLY>")) else {
        reader.set_status("Save all cancelled.".to_string());
        return;
    };

    if !matches!(
        reader.prompt_yn(&format!("Save all attachments to '{}'?", save_dir)),
        Ok(Some(true))
    ) {
        reader.set_status("Save all cancelled.".to_string());
        return;
    }

    let target_dir = Path::new(&save_dir);
    let success_count = attachments
        .iter()
        .filter(|(filename, data)| fs::write(target_dir.join(filename), data).is_ok())
        .count();

    if success_count == attachments.len() {
        reader.set_status(format!("Saved {} attachments to {}", success_count, save_dir));
    } else {
        reader.set_status(format!(
            "Saved {}/{} attachments to {}",
            success_count,
            attachments.len(),
            save_dir
        ));
    }
}

fn save_attachment(filename: &str, data: &[u8], reader: &mut Editor) {
    if let Ok(Some(save_path)) = reader.run_file_browser(true, Some(filename)) {
        if fs::write(&save_path, data).is_ok() {
            reader.set_status(format!("Saved {} to {}", filename, save_path));
        } else {
            reader.set_status(format!("Failed to save {}", filename));
        }
    } else {
        reader.set_status("Save cancelled.".to_string());
    }
}

fn open_attachment(filename: &str, data: &[u8], reader: &mut Editor) {
    let file_path = temp_attachments_dir().join(filename);
    if fs::write(&file_path, data).is_err() {
        reader.set_status(format!("Failed to save {}", filename));
    } else if file_path
        .to_str()
        .is_some_and(|p| webbrowser::open(p).is_ok())
    {
        reader.set_status(format!("Opened {}", filename));
    } else {
        reader.set_status(format!("Failed to open {}", filename));
    }
}

pub fn view_email(
    app: &mut App,
    session: &mut Option<MailSession>,
    settings_provider: &mut Editor,
    stdout: &mut Stdout,
    text_body: &str,
    html_body: &Option<String>,
    attachments: &[(String, Vec<u8>)],
) {
    let mut email = ReaderEmail::from_current(
        app,
        text_body.to_string(),
        html_body.clone(),
        attachments.to_vec(),
    );

    let mut reader = Editor::new(None);
    reader.menu_state = MenuState::EmailReader;
    reader.current_theme = settings_provider.current_theme.clone();
    reader.soft_wrap = false;

    let (init_cols, _) = term_size().unwrap_or((80, 24));
    email.apply_to_reader(&mut reader, init_cols);

    let active_email = app.active_account.email.clone();
    let mut expand_headers = false;

    loop {
        let (cols, _) = term_size().unwrap_or((80, 24));
        let is_deleted = app
            .page_emails
            .get(app.selected_index)
            .is_some_and(|e| e.is_deleted);

        reader.top_margin = render_headers(
            stdout,
            cols,
            &reader,
            &active_email,
            is_deleted,
            &email,
            expand_headers,
        ).unwrap();

        reader.draw_screen().unwrap();

        let timeout = match reader.status_time {
            Some(time) => Duration::from_secs(3)
                .checked_sub(time.elapsed())
                .unwrap_or(Duration::from_millis(1)),
            None => Duration::from_secs(3600),
        };

        if !event::poll(timeout).unwrap() {
            reader.clear_status();
            continue;
        }

        match event::read().unwrap() {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let is_alt = key.modifiers.contains(KeyModifiers::ALT);

                if is_ctrl {
                    match key.code {
                        KeyCode::Char('y') => {
                            reader.set_status("Text copied to clipboard".to_string());
                            continue;
                        }
                        KeyCode::Char('r') => {
                            reply_to_email(
                                app,
                                session,
                                &email,
                                &active_email,
                                &mut reader,
                                true,
                            );
                            continue;
                        }
                        KeyCode::Char('n' | 'p') => {
                            navigate_adjacent_email(
                                app,
                                session,
                                settings_provider,
                                &mut reader,
                                &mut email,
                                stdout,
                                key.code == KeyCode::Char('n'),
                            );
                            continue;
                        }
                        _ => {}
                    }
                }

                if is_alt {
                    if let KeyCode::Char(c) = key.code {
                        if c == '0' && !email.attachments.is_empty() {
                            save_all_attachments(&email.attachments, &mut reader);
                            continue;
                        } else if c.is_ascii_digit() && c != '0' {
                            let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
                            if let Some((filename, data)) = email.attachments.get(idx) {
                                save_attachment(filename, data, &mut reader);
                                continue;
                            }
                        }
                    }
                }

                if !is_ctrl && !is_alt {
                    match key.code {
                        KeyCode::Char('d' | 'D') => {
                            toggle_delete(app, session, &mut reader);
                            continue;
                        }
                        KeyCode::Char('o' | 'O') => {
                            reader.menu_page = if reader.menu_page == 1 { 2 } else { 1 };
                            continue;
                        }
                        KeyCode::Char('e' | 'E') => {
                            expand_headers = !expand_headers;
                            let status = if expand_headers {
                                "Headers expanded"
                            } else {
                                "Headers collapsed"
                            };
                            reader.set_status(status.to_string());
                            queue!(stdout, Clear(ClearType::All)).unwrap();
                            continue;
                        }
                        KeyCode::Char('a' | 'A') => {
                            let addrs = collect_email_addresses(&email);
                            if addrs.is_empty() {
                                reader.set_status("No addresses found to add.".to_string());
                            } else if let Ok(Some(selected_addr)) =
                                reader.prompt_select_item("Add address:", &addrs)
                            {
                                match address::add_to_address_book(&selected_addr) {
                                    Ok(true) => reader.set_status(format!(
                                        "Added {} to address book.",
                                        selected_addr
                                    )),
                                    Ok(false) => reader.set_status(
                                        "Address already in address book".to_string(),
                                    ),
                                    Err(_) => reader
                                        .set_status("Failed to access address book".to_string()),
                                }
                            } else {
                                reader.set_status("Add address cancelled.".to_string());
                            }
                            continue;
                        }
                        KeyCode::Char('r' | 'R') => {
                            reply_to_email(
                                app,
                                session,
                                &email,
                                &active_email,
                                &mut reader,
                                false,
                            );
                            continue;
                        }
                        KeyCode::Char('f' | 'F') => {
                            let sub = if email.subject.to_lowercase().starts_with("fwd:") {
                                email.subject.clone()
                            } else {
                                format!("Fwd: {}", email.subject)
                            };
                            let fwd_body = format!(
                                "--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}",
                                email.from, email.date, email.subject, email.text_body
                            );
                            if let Some(s) = compose::compose_email(
                                &app.active_account,
                                None,
                                None,
                                Some(&sub),
                                Some(&fwd_body),
                                &mut reader.current_theme,
                            ) {
                                reader.set_status(s);
                            }
                            continue;
                        }
                        KeyCode::Char('b' | 'B') => {
                            open_in_browser(&email, &mut reader);
                            continue;
                        }
                        KeyCode::Char('s' | 'S') => {
                            if let Ok(Some(path)) = reader.run_file_browser(true, None) {
                                if fs::write(&path, email.text_body.as_bytes()).is_ok() {
                                    reader.set_status(format!("Saved to {}", path));
                                } else {
                                    reader.set_status(format!("Failed to save to {}", path));
                                }
                            }
                            continue;
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                            let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
                            if let Some((filename, data)) = email.attachments.get(idx) {
                                open_attachment(filename, data, &mut reader);
                                continue;
                            }
                        }
                        KeyCode::Char('h' | 'H' | '?') => {
                            let _ = reader.show_help("email_reader");
                            queue!(stdout, Clear(ClearType::All)).unwrap();
                            continue;
                        }
                        _ => {}
                    }
                }

                if matches!(reader.handle_keypress(key).unwrap(), EditorResult::Cancel) {
                    break;
                }
            }
            Event::Resize(_, _) => {
                queue!(stdout, Clear(ClearType::All)).unwrap();
            }
            _ => {}
        }
    }

    settings_provider.current_theme = reader.current_theme;
    app.last_fetch_time = Instant::now();
    app.mode = AppMode::EmailList;
}