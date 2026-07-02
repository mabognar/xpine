use crate::config::Account;
use crate::editor::{Editor, EditorResult, MenuState};
use crate::theme::{derive_ui_colors};
use crate::ui::UiExt;
use std::path::Path;
use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::{Message, SmtpTransport, Transport};
use std;

use crate::spell::SpellExt;
use crate::browser::BrowserExt;

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
use crate::prompt::PromptExt;

struct ComposeState {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attachments: Vec<String>,
    active_idx: usize,
    scroll_offset: usize,
}

pub fn compose_email(account: &Account, default_to: Option<&str>, default_cc: Option<&str>, default_subject: Option<&str>, default_body: Option<&str>, current_theme: &mut String) -> Option<String> {
    let mut state = ComposeState {
        // Strip out hidden newlines from folded headers
        to: default_to.unwrap_or("").replace('\r', "").replace('\n', ""),
        cc: default_cc.unwrap_or("").replace('\r', "").replace('\n', ""), // <-- UPDATE THIS LINE
        bcc: String::new(),
        subject: default_subject.unwrap_or("").replace('\r', "").replace('\n', ""),
        attachments: Vec::new(),
        active_idx: if default_to.is_some() { 4 } else { 0 },
        scroll_offset: 0,
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

    let mut suggestion_idx = 0;
    let mut cursor_pos = state.to.len(); // Starts at the end of the To: field

    // clear before starting the composer
    execute!(stdout, Clear(ClearType::All)).unwrap();

    loop {
        let (cols, rows) = term_size().unwrap_or((80, 24));
        let theme = &editor.theme_set.themes[&editor.current_theme];
        let colors = derive_ui_colors(theme);

        // print strings with highlighted "(Team)"
        macro_rules! print_highlighted {
            ($out:expr, $text:expr, $base_color:expr, $accent_color:expr) => {
                let mut parts = $text.split("(Team)");
                if let Some(first) = parts.next() {
                    queue!($out, SetForegroundColor($base_color), Print(first)).unwrap();
                }
                for part in parts {
                    queue!(
                        $out,
                        SetForegroundColor($accent_color), Print("(Team)"),
                        SetForegroundColor($base_color), Print(part)
                    ).unwrap();
                }
            };
        }

        let header_title = format!("Compose Email ({})", account.email);
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(colors.menu_bg),
            Clear(ClearType::UntilNewLine),
            SetForegroundColor(colors.accent),
            Print(header_title)
        ).unwrap();

        let fields = ["To:", "Cc:", "Bcc:", "Subject:"];
        let vals = [&state.to, &state.cc, &state.bcc, &state.subject];

        let label_width = 9;
        let available_width = cols.saturating_sub(label_width + 2) as usize;
        let mut current_y = 1; // Start at row 1

        // Add these two variables:
        let mut active_cursor_x = 0;
        let mut active_cursor_y = 0;

        for i in 0..4 {
            let val = vals[i];
            let is_active = i == state.active_idx;

            // Draw the label
            queue!(
                stdout, cursor::MoveTo(0, current_y),
                SetBackgroundColor(colors.menu_bg), Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent),
                Print(format!("{:>8}", fields[i])),
                SetForegroundColor(colors.fg), Print(" ")
            ).unwrap();

            if is_active {
                // calculate the hint to get width
                let mut hint_str = String::new();
                if i < 3 {
                    let suggestions = crate::prompt::find_email_suggestions(val, &address_book);
                    if !suggestions.is_empty() {
                        let current_suggestion = &suggestions[suggestion_idx % suggestions.len()];
                        let last_part = val.split(',').last().unwrap_or("").trim_start();

                        if last_part.to_lowercase() != current_suggestion.to_lowercase() {
                            // calculate how many matches exist to hint at scrolling
                            let match_indicator = if suggestions.len() > 1 {
                                format!(" ({} of {})", (suggestion_idx % suggestions.len()) + 1, suggestions.len())
                            } else {
                                String::new() // do not show anything if there is only 1 match
                            };

                            hint_str = if current_suggestion.to_lowercase().starts_with(&last_part.to_lowercase()) {
                                // append the indicator to the inline remainder
                                format!(" {}{}", &current_suggestion[last_part.len()..], match_indicator)
                            } else {
                                // append the indicator to the fallback substring match
                                format!("  -> {}{}", current_suggestion, match_indicator)
                            };
                        }
                    }
                }

                // wrap text & calculate accurate cursor mapping
                let (wrapped_lines, cursor_row, cursor_col) = wrap_text(val, available_width, hint_str.chars().count(), cursor_pos);
                let viewport_height = wrapped_lines.len().min(8);

                // auto-scroll logic: Keep cursor within viewport
                if cursor_row < state.scroll_offset {
                    state.scroll_offset = cursor_row;
                } else if cursor_row >= state.scroll_offset + viewport_height {
                    state.scroll_offset = cursor_row - viewport_height + 1;
                }

                // render only the visible viewport slice
                let end_idx = (state.scroll_offset + viewport_height).min(wrapped_lines.len());
                let visible_lines = &wrapped_lines[state.scroll_offset..end_idx];

                for (line_idx, line) in visible_lines.iter().enumerate() {
                    if line_idx > 0 {
                        queue!(
                            stdout,
                            cursor::MoveTo(0, current_y + line_idx as u16),
                            SetBackgroundColor(colors.menu_bg),
                            Clear(ClearType::UntilNewLine)
                        ).unwrap();
                    }

                    queue!(
                        stdout,
                        cursor::MoveTo(label_width, current_y + line_idx as u16),
                        SetBackgroundColor(colors.menu_bg) // Only set BG here
                    ).unwrap();

                    // Render the line with conditional highlights
                    print_highlighted!(stdout, line, colors.fg, colors.accent);
                }

                // render the autocomplete hint on same line
                if !hint_str.is_empty() {
                    let last_line_idx = wrapped_lines.len().saturating_sub(1);
                    if last_line_idx >= state.scroll_offset && last_line_idx < state.scroll_offset + viewport_height {
                        let relative_row = last_line_idx - state.scroll_offset;
                        let last_line_len = wrapped_lines[last_line_idx].chars().count();

                        // Safety truncation if a single word + hint is wider than the terminal screen
                        let screen_space = (cols as usize).saturating_sub(label_width as usize + last_line_len);

                        if screen_space > 0 {
                            let hint_color = if colors.is_dark { Color::DarkGrey } else { Color::Grey };
                            queue!(
                                stdout,
                                cursor::MoveTo(label_width + last_line_len as u16, current_y + relative_row as u16),
                                SetBackgroundColor(colors.menu_bg)
                            ).unwrap();

                            let hint_disp = hint_str.chars().take(screen_space).collect::<String>();
                            print_highlighted!(stdout, &hint_disp, hint_color, colors.accent);
                        }
                    }
                }

                // calculate hardware cursor position
                let relative_row = (cursor_row - state.scroll_offset) as u16;
                active_cursor_x = label_width + cursor_col as u16;
                active_cursor_y = current_y + relative_row;

                current_y += viewport_height as u16;
            } else {
                // truncate to one line
                let display_text = if val.len() > available_width {
                    // Show as much as possible, ending with an ellipsis
                    format!("{}...", &val[..available_width.saturating_sub(3)])
                } else {
                    val.to_string()
                };

                queue!(
                    stdout, cursor::MoveTo(label_width, current_y),
                    SetBackgroundColor(colors.menu_bg)
                ).unwrap();

                print_highlighted!(stdout, &display_text, colors.fg, colors.accent);

                current_y += 1;
            }
        }

        queue!(stdout, cursor::MoveTo(0, current_y), Clear(ClearType::UntilNewLine), SetBackgroundColor(colors.menu_bg), SetForegroundColor(colors.accent), Print(" Attach: "), SetForegroundColor(colors.fg)).unwrap();
        current_y += 1;

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

        editor.top_margin = current_y;

        // draw the editor body
        editor.draw_screen().unwrap();

        if state.active_idx < 4 {
            let m_col = (cols as usize / 6).max(1);
            Editor::draw_menu_line(&mut stdout, rows - 2, cols, m_col, &[("^X", " Send"),   ("^P", " Prev"), ("^A", " Attach"), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();
            Editor::draw_menu_line(&mut stdout, rows - 1, cols, m_col, &[("^C", " Cancel"), ("^N", " Next"), ("", ""), ("", ""), ("", ""), ("", "")], colors.menu_bg, colors.accent, colors.fg).unwrap();
            queue!(stdout, cursor::Show).unwrap();

            queue!(stdout, cursor::MoveTo(active_cursor_x, active_cursor_y), cursor::Show).unwrap();
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
                            if editor.spellcheck_before_send {
                                // if spellcheck is cancelled, return to the composer
                                if let Ok(false) = editor.spell_check() {
                                    continue;
                                }
                            }

                            final_body = editor.buffer.to_string();
                            break;
                        }
                        // if key_event.code == KeyCode::Char('c') {
                        //     if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } else { continue; }
                        // }
                        if key_event.code == KeyCode::Char('c') {
                            if let Ok(true) = editor.prompt_cancel() { cancelled = true; break; } else { continue; }
                        }
                        if key_event.code == KeyCode::Char('a') {
                            if let Ok(Some(path)) = editor.run_file_browser(false, None) { state.attachments.push(path); }
                            continue;
                        }

                        // insert signature at cursor
                        if key_event.code == KeyCode::Char('g') {
                            if state.active_idx == 4 { // Ensure they are typing in the body
                                let signature = crate::config::load_signature();
                                let clean_sig = signature.trim();
                                if !clean_sig.is_empty() {
                                    let sig_block = format!("{}\n", clean_sig);
                                    let idx = editor.get_cursor_char_idx();
                                    editor.buffer.insert(idx, &sig_block);

                                    // update the cursor and force redraw
                                    let new_idx = idx + sig_block.chars().count();
                                    editor.cursor_y = editor.buffer.char_to_line(new_idx);
                                    editor.cursor_x = new_idx - editor.buffer.line_to_char(editor.cursor_y);
                                    editor.desired_cursor_x = editor.cursor_x;

                                    // invalidate the visual cache so the pasted text appears instantly
                                    editor.highlight_cache.clear();
                                    editor.is_modified = true;

                                    editor.set_status("Signature inserted".to_string());
                                } else {
                                    editor.set_status("Signature is empty (Check Settings)".to_string());
                                }
                            } else {
                                editor.set_status("Move to the message body to insert signature".to_string());
                            }
                            continue;
                        }
                    }

                    if state.active_idx == 4 {
                        // treat both physical Up arrow and Ctrl+P as upward jump command
                        let is_up_cmd = key_event.code == KeyCode::Up ||
                            (key_event.code == KeyCode::Char('p') && key_event.modifiers.contains(KeyModifiers::CONTROL));

                        if is_up_cmd && editor.cursor_y == 0 {
                            state.active_idx = 3;
                            cursor_pos = state.subject.len(); // reset cursor to end of subject
                            continue;
                        }
                        match editor.handle_keypress(key_event).unwrap() {
                            EditorResult::Send => {
                                if editor.spellcheck_before_send {
                                    // if the spellcheck is cancelled, return to the composer
                                    if let Ok(false) = editor.spell_check() {
                                        continue;
                                    }
                                }

                                final_body = editor.buffer.to_string();
                                break;
                            }
                            // EditorResult::Cancel => { if crate::prompt::prompt_cancel(&mut stdout, &colors) { cancelled = true; break; } }
                            EditorResult::Cancel => { if let Ok(true) = editor.prompt_cancel() { cancelled = true; break; } }
                            EditorResult::Continue => {}
                        }
                    } else {

                        let label_width = 9;
                        let available_width = cols.saturating_sub(label_width + 2) as usize;

                        // --- NEW: Map Ctrl+P / Ctrl+N to Up / Down ---
                        let mut effective_code = key_event.code;
                        if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                            if key_event.code == KeyCode::Char('p') { effective_code = KeyCode::Up; }
                            if key_event.code == KeyCode::Char('n') { effective_code = KeyCode::Down; }
                        }

                        match effective_code {
                            KeyCode::Left => {
                                if cursor_pos > 0 { cursor_pos -= 1; }
                            }
                            KeyCode::Right => {
                                let target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, 3 => &state.subject, _ => "" };
                                if cursor_pos < target.len() { cursor_pos += 1; }
                            }
                            KeyCode::Up => {
                                if cursor_pos >= available_width {
                                    cursor_pos -= available_width;
                                } else {
                                    let mut scrolled_suggestion = false;

                                    // scroll suggestions up
                                    if state.active_idx < 3 {
                                        let target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, _ => unreachable!() };
                                        let suggestions = crate::prompt::find_email_suggestions(target, &address_book);
                                        if suggestions.len() > 1 {
                                            // wrap around to the end of the list if we hit the top
                                            suggestion_idx = if suggestion_idx == 0 { suggestions.len() - 1 } else { suggestion_idx - 1 };
                                            scrolled_suggestion = true;
                                        }
                                    }

                                    if !scrolled_suggestion {
                                        if state.active_idx > 0 {
                                            state.active_idx -= 1;
                                            let new_target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, 3 => &state.subject, _ => unreachable!() };
                                            cursor_pos = new_target.len();
                                            state.scroll_offset = 0; // reset scroll
                                        } else {
                                            cursor_pos = 0;
                                        }
                                        suggestion_idx = 0;
                                    }
                                }
                            }
                            KeyCode::Down => {
                                // move down one wrapped line
                                let target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, 3 => &state.subject, _ => "" };
                                if cursor_pos + available_width <= target.len() {
                                    cursor_pos += available_width;
                                } else {
                                    let mut scrolled_suggestion = false;
                                    if state.active_idx < 3 {
                                        let target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, _ => unreachable!() };
                                        let suggestions = crate::prompt::find_email_suggestions(target, &address_book);
                                        if suggestions.len() > 1 {
                                            suggestion_idx = (suggestion_idx + 1) % suggestions.len();
                                            scrolled_suggestion = true;
                                        }
                                    }

                                    if !scrolled_suggestion {
                                        state.scroll_offset = 0;
                                        state.active_idx = (state.active_idx + 1).min(4);
                                        if state.active_idx < 4 {
                                            let new_target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, 3 => &state.subject, _ => unreachable!() };
                                            cursor_pos = new_target.len();
                                        }
                                        suggestion_idx = 0;
                                    }
                                }
                            }
                            KeyCode::Tab | KeyCode::Enter => {
                                if state.active_idx < 3 {
                                    let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, _ => unreachable!() };
                                    let suggestions = crate::prompt::find_email_suggestions(target, &address_book);

                                    if !suggestions.is_empty() {
                                        let suggestion = &suggestions[suggestion_idx % suggestions.len()];

                                        // strip hidden newlines/carriage returns
                                        let clean_suggestion = suggestion.trim_end();

                                        let last_part = target.split(',').last().unwrap_or("").trim_start();

                                        if last_part.to_lowercase() != clean_suggestion.to_lowercase() {
                                            if let Some(last_comma_idx) = target.rfind(',') {
                                                target.truncate(last_comma_idx + 1);
                                                target.push(' ');
                                                target.push_str(clean_suggestion); // append cleaned version
                                            } else {
                                                *target = clean_suggestion.to_string(); // replace with cleaned version
                                            }

                                            cursor_pos = target.len();

                                            suggestion_idx = 0;
                                            continue;
                                        }
                                    }
                                }
                                state.scroll_offset = 0;
                                state.active_idx = (state.active_idx + 1).min(4);
                                if state.active_idx < 4 {
                                    let new_target = match state.active_idx { 0 => &state.to, 1 => &state.cc, 2 => &state.bcc, 3 => &state.subject, _ => unreachable!() };
                                    cursor_pos = new_target.len();
                                }
                                suggestion_idx = 0;
                            }
                            KeyCode::Char(c) => {
                                let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
                                if cursor_pos >= target.len() {
                                    target.push(c);
                                } else {
                                    target.insert(cursor_pos, c);
                                }
                                cursor_pos += c.len_utf8();
                                suggestion_idx = 0;
                            }
                            KeyCode::Backspace => {
                                let target = match state.active_idx { 0 => &mut state.to, 1 => &mut state.cc, 2 => &mut state.bcc, 3 => &mut state.subject, _ => unreachable!() };
                                if cursor_pos > 0 {
                                    if let Some(c) = target[..cursor_pos].chars().next_back() {
                                        cursor_pos -= c.len_utf8();
                                        target.remove(cursor_pos);
                                    }
                                }
                                suggestion_idx = 0;
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
    queue!(stdout, cursor::MoveTo(0, rows - 3), SetBackgroundColor(colors.selected_bg), Clear(ClearType::UntilNewLine), SetForegroundColor(colors.accent), Print("Sending message... Please wait "), ResetColor).unwrap();
    stdout.flush().unwrap();

    let from_addr = if let Some(name) = &account.name {
        format!("{} <{}>", name, account.email)
    } else {
        format!("<{}>", account.email)
    };

    let mut builder = Message::builder()
        .from(from_addr.parse().unwrap())
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

    // protect the signature from being squashed by the text wrapper
    let signature = crate::config::load_signature();
    let clean_sig = signature.trim();

    let formatted_body = if !clean_sig.is_empty() && final_body.contains(clean_sig) {
        // Split the email into two halves: everything above the signature, and everything below
        if let Some((top, bottom)) = final_body.split_once(clean_sig) {
            let justified_top = crate::mail::justify_all_text(top);
            let justified_bottom = crate::mail::justify_all_text(bottom);

            format!("{}{}{}", justified_top, clean_sig, justified_bottom)
        } else {
            crate::mail::justify_all_text(&final_body)
        }
    } else {
        crate::mail::justify_all_text(&final_body)
    };

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
                let target_scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.Send") } else { None };

                match crate::net::get_oauth_access_token(
                    account.client_id.as_deref().unwrap_or(""),
                    account.client_secret.as_deref().unwrap_or(""),
                    rt,
                    is_microsoft,
                    target_scope
                ) {
                    Ok(token) => token,
                    Err(_) if is_microsoft => {
                        execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
                        queue!(stdout, Print("Microsoft requires separate authorization to send emails.\r\n")).unwrap();
                        queue!(stdout, Print("Initiating a one-time sending authorization...\r\n\n")).unwrap();
                        stdout.flush().unwrap();

                        queue!(stdout, Print("\r\nPress Enter to return and complete authorization...")).unwrap();
                        stdout.flush().unwrap();
                        loop { if let Ok(Event::Key(k)) = event::read() { if k.code == KeyCode::Enter { break; } } }

                        return None;
                    }
                    Err(_) => {
                        return None;
                    }
                }
            } else {
                account.password.clone().unwrap_or_default()
            };

            if is_microsoft && account.refresh_token.is_some() {
                // submit raw MIME as a base64 encoded text to Graph API
                use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
                let email_bytes = email_msg.formatted();
                let base64_content = base64_engine.encode(&email_bytes);

                let client = reqwest::blocking::Client::new();
                let res = client.post("https://graph.microsoft.com/v1.0/me/sendMail")
                    .header("Authorization", format!("Bearer {}", token_or_pass))
                    .header("Content-Type", "text/plain")
                    .body(base64_content)
                    .send();

                match res {
                    Ok(r) if r.status().is_success() => Some("Message Sent via MS Graph API".to_string()),
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
                let creds = SmtpCredentials::new(account.email.clone(), token_or_pass);
                let mut mailer = SmtpTransport::starttls_relay(&account.smtp_server)
                    .unwrap()
                    // --- CHANGE THIS LINE ---
                    .port(account.smtp_port)
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

fn wrap_text(text: &str, width: usize, hint_len: usize, cursor_pos: usize) -> (Vec<String>, usize, usize) {
    let clean_text = text.replace('\r', "").replace('\n', "");
    if clean_text.is_empty() || width == 0 { return (vec![String::new()], 0, 0); }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut c_row = 0;
    let mut c_col = 0;
    let mut char_idx = 0;

    // split by spaces to keep "email@domain.com," together
    let words: Vec<&str> = clean_text.split(' ').collect();
    let num_words = words.len();

    for (i, word) in words.iter().enumerate() {
        let word_len = word.chars().count();
        let space_needed = if current_line.is_empty() { 0 } else { 1 };

        // add the hint length to last word to preemptively wrap it if the hint won't fit
        let effective_len = if i == num_words - 1 { word_len + hint_len } else { word_len };

        if current_line.chars().count() + space_needed + effective_len > width {
            if !current_line.is_empty() {
                lines.push(current_line);
                current_line = String::new();
            }

            if space_needed > 0 {
                if char_idx == cursor_pos { c_row = lines.len(); c_col = 0; }
                char_idx += 1;
            }

            for c in word.chars() {
                if char_idx == cursor_pos { c_row = lines.len(); c_col = current_line.chars().count(); }
                current_line.push(c);
                char_idx += c.len_utf8();
            }
        } else {
            if space_needed > 0 {
                if char_idx == cursor_pos { c_row = lines.len(); c_col = current_line.chars().count(); }
                current_line.push(' ');
                char_idx += 1;
            }
            for c in word.chars() {
                if char_idx == cursor_pos { c_row = lines.len(); c_col = current_line.chars().count(); }
                current_line.push(c);
                char_idx += c.len_utf8();
            }
        }
    }
    lines.push(current_line);

    // edge case for when the cursor is at the very end of the string
    if char_idx == cursor_pos {
        c_row = lines.len().saturating_sub(1);
        c_col = lines.last().unwrap().chars().count();
    }

    (lines, c_row, c_col)
}
