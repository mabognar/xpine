use std::fs;
use std::io::{self, stdout, Stdout, Write};
use std::path::Path;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{size as term_size, Clear, ClearType},
};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MessageBuilder, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::{Message, SmtpTransport, Transport};
use ropey::Rope;

use crate::browser::BrowserExt;
use crate::config::Account;
use crate::editor::{Editor, EditorResult, MenuState};
use crate::prompt::PromptExt;
use crate::spell::SpellExt;
use crate::theme::derive_ui_colors;
use crate::ui::UiExt;

const LABEL_WIDTH: u16 = 9;
const HEADER_LABELS: [&str; 4] = ["To:", "Cc:", "Bcc:", "Subject:"];

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attachments: Vec<String>,
    active_idx: usize,
    scroll_offset: usize,
}

impl ComposeState {
    fn field(&self, idx: usize) -> Option<&str> {
        match idx {
            0 => Some(&self.to),
            1 => Some(&self.cc),
            2 => Some(&self.bcc),
            3 => Some(&self.subject),
            _ => None,
        }
    }

    fn active_field(&self) -> Option<&str> {
        self.field(self.active_idx)
    }

    fn active_field_mut(&mut self) -> Option<&mut String> {
        match self.active_idx {
            0 => Some(&mut self.to),
            1 => Some(&mut self.cc),
            2 => Some(&mut self.bcc),
            3 => Some(&mut self.subject),
            _ => None,
        }
    }
}

fn strip_newlines(s: Option<&str>) -> String {
    s.unwrap_or("").replace(['\r', '\n'], "")
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let idx = floor_char_boundary(s, idx);
    s[..idx]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let idx = floor_char_boundary(s, idx);
    s[idx..]
        .chars()
        .next()
        .map(|c| idx + c.len_utf8())
        .unwrap_or(s.len())
}

fn find_last_unquoted_comma(s: &str) -> Option<usize> {
    let mut last_comma = None;
    let mut in_quotes = false;
    for (idx, c) in s.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == ',' && !in_quotes {
            last_comma = Some(idx);
        }
    }
    last_comma
}

fn split_unquoted_commas(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in input.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        if c == ',' && !in_quotes {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn print_highlighted(
    out: &mut Stdout,
    text: &str,
    base_color: Color,
    accent_color: Color,
) -> io::Result<()> {
    let mut parts = text.split("(Team)");
    if let Some(first) = parts.next() {
        queue!(out, SetForegroundColor(base_color), Print(first))?;
    }
    for part in parts {
        queue!(
            out,
            SetForegroundColor(accent_color),
            Print("(Team)"),
            SetForegroundColor(base_color),
            Print(part)
        )?;
    }
    Ok(())
}

fn build_autocomplete_hint(val: &str, address_book: &[String], suggestion_idx: usize) -> String {
    let suggestions = crate::prompt::find_email_suggestions(val, address_book);
    if suggestions.is_empty() {
        return String::new();
    }

    let chosen_idx = suggestion_idx % suggestions.len();
    let current_suggestion = &suggestions[chosen_idx];
    let last_part = match find_last_unquoted_comma(val) {
        Some(idx) => val[idx + 1..].trim_start(),
        None => val.trim_start(),
    };

    if last_part.eq_ignore_ascii_case(current_suggestion)
        || last_part.to_lowercase() == current_suggestion.to_lowercase()
    {
        return String::new();
    }

    let match_indicator = if suggestions.len() > 1 {
        format!(" ({} of {})", chosen_idx + 1, suggestions.len())
    } else {
        String::new()
    };

    if current_suggestion
        .to_lowercase()
        .starts_with(&last_part.to_lowercase())
    {
        let remainder: String = current_suggestion
            .chars()
            .skip(last_part.chars().count())
            .collect();
        format!(" {}{}", remainder, match_indicator)
    } else {
        format!("  -> {}{}", current_suggestion, match_indicator)
    }
}

fn confirm_send(editor: &mut Editor) -> bool {
    if editor.spellcheck_before_send {
        if let Ok(false) = editor.spell_check() {
            return false;
        }
    }
    true
}

fn insert_signature(editor: &mut Editor) {
    let signature = crate::config::load_signature();
    let clean_sig = signature.trim();
    if clean_sig.is_empty() {
        editor.set_status("Signature is empty (Check Settings)".to_string());
        return;
    }

    let sig_block = format!("{}\n", clean_sig);
    let idx = editor.get_cursor_char_idx();
    editor.buffer.insert(idx, &sig_block);

    let new_idx = idx + sig_block.chars().count();
    editor.cursor_y = editor.buffer.char_to_line(new_idx);
    editor.cursor_x = new_idx - editor.buffer.line_to_char(editor.cursor_y);
    editor.desired_cursor_x = editor.cursor_x;

    editor.highlight_cache.clear();
    editor.is_modified = true;
    editor.set_status("Signature inserted".to_string());
}

fn show_blocking_message(stdout: &mut Stdout, message: &str, prompt: &str) {
    let _ = execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0));
    let _ = queue!(stdout, Print(message), Print(prompt));
    let _ = stdout.flush();
    while let Ok(ev) = event::read() {
        if let Event::Key(k) = ev {
            if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter {
                break;
            }
        }
    }
}

fn mime_for_extension(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    match ext.as_str() {
        "txt" | "rs" | "c" | "cpp" | "md" | "toml" | "json" => "text/plain",
        "html" | "htm" => "text/html",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}

fn add_recipients(
    mut builder: MessageBuilder,
    input: &str,
    field_type: &str,
    account_email: &str,
) -> MessageBuilder {
    for addr in split_unquoted_commas(input) {
        let mut trimmed = addr.trim_end_matches(';').to_string();
        if trimmed.eq_ignore_ascii_case(account_email) {
            if let Some((user, domain)) = trimmed.split_once('@') {
                if !user.contains('+') {
                    trimmed = format!("{}+me@{}", user, domain);
                }
            }
        }
        if trimmed.is_empty() {
            continue;
        }
        let mailbox = trimmed
            .parse::<Mailbox>()
            .or_else(|_| format!("<{}>", trimmed).parse::<Mailbox>());
        if let Ok(mb) = mailbox {
            builder = match field_type {
                "to" => builder.to(mb),
                "cc" => builder.cc(mb),
                "bcc" => builder.bcc(mb),
                _ => builder,
            };
        }
    }
    builder
}

fn build_email_message(
    account: &Account,
    state: ComposeState,
    final_body: &str,
    address_book: &[String],
) -> Result<Message, lettre::error::Error> {
    let from_addr = match &account.name {
        Some(name) => format!("{} <{}>", name, account.email),
        None => format!("<{}>", account.email),
    };

    let mut builder = Message::builder()
        .from(from_addr.parse().unwrap())
        .subject(state.subject);

    let final_to = crate::address::expand_address_lists(&state.to, address_book);
    let final_cc = crate::address::expand_address_lists(&state.cc, address_book);
    let final_bcc = crate::address::expand_address_lists(&state.bcc, address_book);

    builder = add_recipients(builder, &final_to, "to", &account.email);
    builder = add_recipients(builder, &final_cc, "cc", &account.email);
    builder = add_recipients(builder, &final_bcc, "bcc", &account.email);

    // Protect the signature from being squashed by the text wrapper
    let signature = crate::config::load_signature();
    let clean_sig = signature.trim();
    let formatted_body = if !clean_sig.is_empty() {
        if let Some((top, bottom)) = final_body.split_once(clean_sig) {
            format!(
                "{}{}{}",
                crate::mail::justify_all_text(top),
                clean_sig,
                crate::mail::justify_all_text(bottom)
            )
        } else {
            crate::mail::justify_all_text(final_body)
        }
    } else {
        crate::mail::justify_all_text(final_body)
    };

    let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(formatted_body));

    for att in &state.attachments {
        if let Ok(file_data) = fs::read(att) {
            let path = Path::new(att);
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if let Ok(content_type) = mime_for_extension(path).parse::<ContentType>() {
                multipart =
                    multipart.singlepart(Attachment::new(file_name).body(file_data, content_type));
            }
        }
    }

    builder.multipart(multipart)
}

fn send_email_message(
    account: &Account,
    email_msg: &Message,
    stdout: &mut Stdout,
) -> Option<String> {
    let email_lower = account.email.to_lowercase();
    let is_microsoft =
        email_lower.ends_with("@outlook.com") || email_lower.ends_with("@hotmail.com");

    let token_or_pass = if let Some(ref rt) = account.refresh_token {
        let target_scope = is_microsoft.then_some("https://graph.microsoft.com/Mail.Send");
        match crate::net::get_oauth_access_token(
            account.client_id.as_deref().unwrap_or(""),
            account.client_secret.as_deref().unwrap_or(""),
            rt,
            is_microsoft,
            target_scope,
        ) {
            Ok(token) => token,
            Err(_) if is_microsoft => {
                show_blocking_message(
                    stdout,
                    "Microsoft requires separate authorization to send emails.\r\n\
                     Initiating a one-time sending authorization...\r\n\n",
                    "\r\nPress Enter to return and complete authorization...",
                );
                return None;
            }
            Err(_) => return None,
        }
    } else {
        account.password.clone().unwrap_or_default()
    };

    if is_microsoft && account.refresh_token.is_some() {
        let base64_content = BASE64_STANDARD.encode(email_msg.formatted());
        let res = crate::net::http_client()
            .post("https://graph.microsoft.com/v1.0/me/sendMail")
            .bearer_auth(&token_or_pass)
            .header("Content-Type", "text/plain")
            .body(base64_content)
            .send();

        match res {
            Ok(r) if r.status().is_success() => Some("Message Sent via MS Graph API".to_string()),
            Ok(r) => {
                let status = r.status();
                let text = r.text().unwrap_or_default();
                show_blocking_message(
                    stdout,
                    &format!("-> Graph API Error: {} - {}\r\n", status, text),
                    "\r\nPress Enter to return...",
                );
                None
            }
            Err(e) => {
                show_blocking_message(
                    stdout,
                    &format!("-> Network Error: {:?}\r\n", e),
                    "\r\nPress Enter to return...",
                );
                None
            }
        }
    } else {
        let creds = SmtpCredentials::new(account.email.clone(), token_or_pass);
        let mut mailer = match SmtpTransport::starttls_relay(&account.smtp_server) {
            Ok(m) => m.port(account.smtp_port).credentials(creds),
            Err(e) => {
                show_blocking_message(
                    stdout,
                    &format!("-> Failed to connect to SMTP server: {:?}\r\n", e),
                    "\r\nPress Enter to return...",
                );
                return None;
            }
        };

        if account.refresh_token.is_some() {
            mailer = mailer.authentication(vec![Mechanism::Xoauth2]);
        }

        match mailer.build().send(email_msg) {
            Ok(_) => Some("Message Sent".to_string()),
            Err(e) => {
                show_blocking_message(
                    stdout,
                    &format!("-> Failed to send message: {:?}\r\n", e),
                    "\r\nPress Enter to return...",
                );
                None
            }
        }
    }
}

pub fn compose_email(
    account: &Account,
    default_to: Option<&str>,
    default_cc: Option<&str>,
    default_subject: Option<&str>,
    default_body: Option<&str>,
    current_theme: &mut String,
) -> Option<String> {
    let mut state = ComposeState {
        to: strip_newlines(default_to),
        cc: strip_newlines(default_cc),
        bcc: String::new(),
        subject: strip_newlines(default_subject),
        attachments: Vec::new(),
        active_idx: if default_to.is_some() { 4 } else { 0 },
        scroll_offset: 0,
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
    let mut suggestion_idx = 0;
    let mut cursor_pos = state.to.len();

    execute!(stdout, Clear(ClearType::All)).unwrap();

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));
        let theme = &editor.theme_set.themes[&editor.current_theme];
        let colors = derive_ui_colors(theme);
        let available_width = cols.saturating_sub(LABEL_WIDTH + 2) as usize;

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(colors.menu_bg),
            Clear(ClearType::UntilNewLine),
            SetForegroundColor(colors.accent),
            Print(format!("Compose Email ({})", account.email))
        )
            .unwrap();

        let mut current_y = 1;
        let mut active_cursor_x = 0;
        let mut active_cursor_y = 0;

        for (i, label) in HEADER_LABELS.iter().enumerate() {
            let val = state.field(i).unwrap_or("");
            queue!(
                stdout,
                cursor::MoveTo(0, current_y),
                SetBackgroundColor(colors.menu_bg),
                Clear(ClearType::UntilNewLine),
                SetForegroundColor(colors.accent),
                Print(format!("{:>8}", label)),
                SetForegroundColor(colors.fg),
                Print(" ")
            )
                .unwrap();

            if i == state.active_idx {
                let hint_str = if i < 3 {
                    build_autocomplete_hint(val, &address_book, suggestion_idx)
                } else {
                    String::new()
                };

                let (wrapped_lines, cursor_row, cursor_col) =
                    wrap_text(val, available_width, hint_str.chars().count(), cursor_pos);
                let viewport_height = wrapped_lines.len().min(8);

                if cursor_row < state.scroll_offset {
                    state.scroll_offset = cursor_row;
                } else if cursor_row >= state.scroll_offset + viewport_height {
                    state.scroll_offset = cursor_row - viewport_height + 1;
                }

                let end_idx = (state.scroll_offset + viewport_height).min(wrapped_lines.len());
                for (line_idx, line) in wrapped_lines[state.scroll_offset..end_idx]
                    .iter()
                    .enumerate()
                {
                    let row_y = current_y + line_idx as u16;
                    if line_idx > 0 {
                        queue!(
                            stdout,
                            cursor::MoveTo(0, row_y),
                            SetBackgroundColor(colors.menu_bg),
                            Clear(ClearType::UntilNewLine)
                        )
                            .unwrap();
                    }
                    queue!(
                        stdout,
                        cursor::MoveTo(LABEL_WIDTH, row_y),
                        SetBackgroundColor(colors.menu_bg)
                    )
                        .unwrap();
                    print_highlighted(&mut stdout, line, colors.fg, colors.accent).unwrap();
                }

                if !hint_str.is_empty() {
                    let last_line_idx = wrapped_lines.len().saturating_sub(1);
                    if last_line_idx >= state.scroll_offset
                        && last_line_idx < state.scroll_offset + viewport_height
                    {
                        let relative_row = (last_line_idx - state.scroll_offset) as u16;
                        let last_line_len = wrapped_lines[last_line_idx].chars().count();
                        let screen_space =
                            (cols as usize).saturating_sub(LABEL_WIDTH as usize + last_line_len);

                        if screen_space > 0 {
                            let hint_color = if colors.is_dark {
                                Color::DarkGrey
                            } else {
                                Color::Grey
                            };
                            let hint_disp: String = hint_str.chars().take(screen_space).collect();
                            queue!(
                                stdout,
                                cursor::MoveTo(
                                    LABEL_WIDTH + last_line_len as u16,
                                    current_y + relative_row
                                ),
                                SetBackgroundColor(colors.menu_bg)
                            )
                                .unwrap();
                            print_highlighted(&mut stdout, &hint_disp, hint_color, colors.accent)
                                .unwrap();
                        }
                    }
                }

                active_cursor_x = LABEL_WIDTH + cursor_col as u16;
                active_cursor_y = current_y + (cursor_row - state.scroll_offset) as u16;
                current_y += viewport_height as u16;
            } else {
                let display_text = if val.chars().count() > available_width {
                    let take = available_width.saturating_sub(3);
                    format!("{}...", val.chars().take(take).collect::<String>())
                } else {
                    val.to_string()
                };
                queue!(
                    stdout,
                    cursor::MoveTo(LABEL_WIDTH, current_y),
                    SetBackgroundColor(colors.menu_bg)
                )
                    .unwrap();
                print_highlighted(&mut stdout, &display_text, colors.fg, colors.accent).unwrap();
                current_y += 1;
            }
        }

        queue!(
            stdout,
            cursor::MoveTo(0, current_y),
            Clear(ClearType::UntilNewLine),
            SetBackgroundColor(colors.menu_bg),
            SetForegroundColor(colors.accent),
            Print(" Attach: "),
            SetForegroundColor(colors.fg)
        )
            .unwrap();
        current_y += 1;

        if state.attachments.is_empty() {
            let dim_c = if colors.is_dark {
                Color::DarkGrey
            } else {
                Color::Grey
            };
            queue!(stdout, SetForegroundColor(dim_c), Print("(Press ^A to attach a file)"))
                .unwrap();
        } else {
            let att_color = if colors.is_dark {
                Color::Rgb { r: 255, g: 80, b: 80 }
            } else {
                Color::Rgb { r: 220, g: 0, b: 0 }
            };
            let att_names: Vec<String> = state
                .attachments
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let file_name = Path::new(p)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    format!("{}. {}", i + 1, file_name)
                })
                .collect();
            queue!(stdout, SetForegroundColor(att_color), Print(att_names.join("   "))).unwrap();
        }
        queue!(stdout, ResetColor).unwrap();

        editor.top_margin = current_y;
        editor.draw_screen().unwrap();

        if state.active_idx < 4 {
            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(
                &mut stdout,
                rows.saturating_sub(2),
                cols,
                m_col,
                &[("^X", " Send"), ("^P", " Prev"), ("^A", " Attach"), ("", ""), ("", "")],
                colors.menu_bg,
                colors.accent,
                colors.fg,
            )
                .unwrap();
            Editor::draw_menu_line(
                &mut stdout,
                rows.saturating_sub(1),
                cols,
                m_col,
                &[("^C", " Cancel"), ("^N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")],
                colors.menu_bg,
                colors.accent,
                colors.fg,
            )
                .unwrap();
            queue!(stdout, cursor::MoveTo(active_cursor_x, active_cursor_y), cursor::Show)
                .unwrap();
        } else {
            queue!(stdout, cursor::Show).unwrap();
        }
        stdout.flush().unwrap();

        let timeout = match editor.status_time {
            Some(time) => Duration::from_secs(3)
                .checked_sub(time.elapsed())
                .unwrap_or(Duration::from_millis(1)),
            None => Duration::from_secs(3600),
        };

        if !event::poll(timeout).unwrap() {
            editor.clear_status();
            continue;
        }

        let Event::Key(key_event) = event::read().unwrap() else {
            continue;
        };
        if key_event.kind != KeyEventKind::Press {
            continue;
        }

        let is_ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let is_alt = key_event.modifiers.contains(KeyModifiers::ALT);

        if is_ctrl {
            match key_event.code {
                KeyCode::Char('x') => {
                    if confirm_send(&mut editor) {
                        final_body = editor.buffer.to_string();
                        break;
                    }
                    continue;
                }
                KeyCode::Char('c') => {
                    if let Ok(true) = editor.prompt_cancel() {
                        cancelled = true;
                        break;
                    }
                    continue;
                }
                KeyCode::Char('a') => {
                    if let Ok(Some(path)) = editor.run_file_browser(false, None) {
                        state.attachments.push(path);
                    }
                    continue;
                }
                KeyCode::Char('g') => {
                    if state.active_idx == 4 {
                        insert_signature(&mut editor);
                    } else {
                        editor.set_status(
                            "Move to the message body to insert signature".to_string(),
                        );
                    }
                    continue;
                }
                _ => {}
            }
        }

        if state.active_idx == 4 {
            let is_up_cmd =
                key_event.code == KeyCode::Up || (is_ctrl && key_event.code == KeyCode::Char('p'));
            if is_up_cmd && editor.cursor_y == 0 {
                state.active_idx = 3;
                cursor_pos = state.subject.len();
                continue;
            }

            match editor.handle_keypress(key_event).unwrap() {
                EditorResult::Send => {
                    if confirm_send(&mut editor) {
                        final_body = editor.buffer.to_string();
                        break;
                    }
                }
                EditorResult::Cancel => {
                    if let Ok(true) = editor.prompt_cancel() {
                        cancelled = true;
                        break;
                    }
                }
                EditorResult::Continue => {}
            }
            continue;
        }

        let effective_code = match (key_event.code, is_ctrl) {
            (KeyCode::Char('p'), true) => KeyCode::Up,
            (KeyCode::Char('n'), true) => KeyCode::Down,
            (code, _) => code,
        };

        match effective_code {
            KeyCode::Left => {
                if let Some(target) = state.active_field() {
                    cursor_pos = prev_char_boundary(target, cursor_pos);
                }
            }
            KeyCode::Right => {
                if let Some(target) = state.active_field() {
                    cursor_pos = next_char_boundary(target, cursor_pos);
                }
            }
            KeyCode::Up => {
                let target = state.active_field().unwrap_or("");
                if cursor_pos >= available_width {
                    cursor_pos = floor_char_boundary(target, cursor_pos - available_width);
                } else {
                    let suggestions_len = if state.active_idx < 3 {
                        crate::prompt::find_email_suggestions(target, &address_book).len()
                    } else {
                        0
                    };
                    if suggestions_len > 1 {
                        suggestion_idx = if suggestion_idx == 0 {
                            suggestions_len - 1
                        } else {
                            suggestion_idx - 1
                        };
                    } else {
                        if state.active_idx > 0 {
                            state.active_idx -= 1;
                            cursor_pos = state.active_field().map_or(0, str::len);
                            state.scroll_offset = 0;
                        } else {
                            cursor_pos = 0;
                        }
                        suggestion_idx = 0;
                    }
                }
            }
            KeyCode::Down => {
                let target = state.active_field().unwrap_or("");
                if cursor_pos + available_width <= target.len() {
                    cursor_pos = floor_char_boundary(target, cursor_pos + available_width);
                } else {
                    let suggestions_len = if state.active_idx < 3 {
                        crate::prompt::find_email_suggestions(target, &address_book).len()
                    } else {
                        0
                    };
                    if suggestions_len > 1 {
                        suggestion_idx = (suggestion_idx + 1) % suggestions_len;
                    } else {
                        state.scroll_offset = 0;
                        state.active_idx = (state.active_idx + 1).min(4);
                        if let Some(new_target) = state.active_field() {
                            cursor_pos = new_target.len();
                        }
                        suggestion_idx = 0;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Enter => {
                let mut completed = false;
                if state.active_idx < 3 {
                    if let Some(target) = state.active_field_mut() {
                        let suggestions =
                            crate::prompt::find_email_suggestions(target, &address_book);
                        if !suggestions.is_empty() {
                            let clean_suggestion =
                                suggestions[suggestion_idx % suggestions.len()].trim_end();
                            let last_comma_idx = find_last_unquoted_comma(target);
                            let last_part = match last_comma_idx {
                                Some(idx) => target[idx + 1..].trim_start(),
                                None => target.trim_start(),
                            };

                            if last_part.to_lowercase() != clean_suggestion.to_lowercase() {
                                if let Some(idx) = last_comma_idx {
                                    target.truncate(idx + 1);
                                    target.push(' ');
                                    target.push_str(clean_suggestion);
                                } else {
                                    *target = clean_suggestion.to_string();
                                }
                                cursor_pos = target.len();
                                suggestion_idx = 0;
                                completed = true;
                            }
                        }
                    }
                }

                if !completed {
                    state.scroll_offset = 0;
                    state.active_idx = (state.active_idx + 1).min(4);
                    if let Some(new_target) = state.active_field() {
                        cursor_pos = new_target.len();
                    }
                    suggestion_idx = 0;
                }
            }
            KeyCode::Char(c) if !is_ctrl && !is_alt => {
                if let Some(target) = state.active_field_mut() {
                    let pos = floor_char_boundary(target, cursor_pos);
                    if pos >= target.len() {
                        target.push(c);
                    } else {
                        target.insert(pos, c);
                    }
                    cursor_pos = pos + c.len_utf8();
                }
                suggestion_idx = 0;
            }
            KeyCode::Backspace => {
                if let Some(target) = state.active_field_mut() {
                    let pos = floor_char_boundary(target, cursor_pos);
                    if pos > 0 {
                        if let Some(c) = target[..pos].chars().next_back() {
                            cursor_pos = pos - c.len_utf8();
                            target.remove(cursor_pos);
                        }
                    }
                }
                suggestion_idx = 0;
            }
            _ => {}
        }
    }

    *current_theme = editor.current_theme.clone();
    if cancelled {
        return None;
    }

    if state.to.trim().is_empty() && state.cc.trim().is_empty() && state.bcc.trim().is_empty() {
        show_blocking_message(
            &mut stdout,
            "No recipients specified. Message cancelled.\r\n\n",
            "Press Enter to return...",
        );
        return None;
    }

    let (_, rows) = term_size().unwrap_or((80, 24));
    let theme = &editor.theme_set.themes[&editor.current_theme];
    let colors = derive_ui_colors(theme);
    queue!(
        stdout,
        cursor::Hide,
        cursor::MoveTo(0, rows.saturating_sub(3)),
        SetBackgroundColor(colors.menu_bg),
        Clear(ClearType::UntilNewLine),
        SetForegroundColor(colors.accent),
        Print("Sending message - Please wait"),
        ResetColor
    )
        .unwrap();
    stdout.flush().unwrap();

    match build_email_message(account, state, &final_body, &address_book) {
        Ok(email_msg) => send_email_message(account, &email_msg, &mut stdout),
        Err(e) => {
            show_blocking_message(
                &mut stdout,
                &format!("-> Failed to build message: {:?}\r\n", e),
                "\r\nPress Enter to return...",
            );
            None
        }
    }
}

fn wrap_text(
    text: &str,
    width: usize,
    hint_len: usize,
    cursor_pos: usize,
) -> (Vec<String>, usize, usize) {
    let clean_text = strip_newlines(Some(text));
    if clean_text.is_empty() || width == 0 {
        return (vec![String::new()], 0, 0);
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut c_row = 0;
    let mut c_col = 0;
    let mut char_idx = 0;

    let words: Vec<&str> = clean_text.split(' ').collect();
    let num_words = words.len();

    for (i, word) in words.iter().enumerate() {
        let word_len = word.chars().count();
        let space_needed = usize::from(!current_line.is_empty());
        let effective_len = if i == num_words - 1 {
            word_len + hint_len
        } else {
            word_len
        };

        if current_line.chars().count() + space_needed + effective_len > width {
            if !current_line.is_empty() {
                lines.push(current_line);
                current_line = String::new();
            }
            if space_needed > 0 {
                if char_idx == cursor_pos {
                    c_row = lines.len();
                    c_col = 0;
                }
                char_idx += 1;
            }
        } else if space_needed > 0 {
            if char_idx == cursor_pos {
                c_row = lines.len();
                c_col = current_line.chars().count();
            }
            current_line.push(' ');
            char_idx += 1;
        }

        for c in word.chars() {
            if char_idx == cursor_pos {
                c_row = lines.len();
                c_col = current_line.chars().count();
            }
            current_line.push(c);
            char_idx += c.len_utf8();
        }
    }
    lines.push(current_line);

    if char_idx == cursor_pos {
        c_row = lines.len().saturating_sub(1);
        c_col = lines.last().unwrap().chars().count();
    }

    (lines, c_row, c_col)
}