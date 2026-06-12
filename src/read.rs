use crate::net::MailSession;
use crate::app::{App, AppMode};
use crate::editor::{Editor, MenuState, EditorResult};
use crate::ui::UiExt;

use ropey::Rope;
use crossterm::{
    cursor, event, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{size as term_size, Clear, ClearType},
};
use std::time::{Duration, Instant};
use crate::{address, compose, mail, net, ui};
use crate::prompt::PromptExt;
use crate::browser::BrowserExt;

pub fn view_email(
    app: &mut App,
    session: &mut Option<MailSession>,
    settings_provider: &mut Editor,
    stdout: &mut std::io::Stdout,
    text_body: &str,
    html_body: &Option<String>,
    attachments: &[(String, Vec<u8>)]
) {
    let mut reader = Editor::new(None);
    reader.menu_state = MenuState::EmailReader;

    let attach_lines = if attachments.is_empty() { 1 } else { 1 + attachments.len() };
    reader.top_margin = (5 + attach_lines) as u16;

    let (cols, _) = term_size().unwrap_or((80, 24));

    let wrap_width = (cols as usize).saturating_sub(2);
    let wrapped_text = crate::mail::wrap_email_body(text_body, wrap_width);
    // let wrapped_text = wrap_email_body(&text_body, wrap_width);

    reader.buffer = Rope::from_str(&wrapped_text);
    reader.current_theme = settings_provider.current_theme.clone();

    reader.soft_wrap = false;

    if let Some(html) = &html_body {
        if !html.is_empty() {
            reader.set_status("Email contains HTML. Type 'B' to view in browser".to_string());
        }
    }

    let email_from = app.page_emails[app.selected_index].from.clone();
    let email_to = app.page_emails[app.selected_index].to_addr.clone();
    let email_cc = app.page_emails[app.selected_index].cc.clone();
    let email_subject = app.page_emails[app.selected_index].subject.clone();
    let active_email = app.active_account.email.clone();

    let reply_to = app.page_emails[app.selected_index].reply_to.clone();
    let date = app.page_emails[app.selected_index].date.clone();
    let fetch_seq = app.page_emails[app.selected_index].id.to_string();

    loop {
        let r_theme = &reader.theme_set.themes[&reader.current_theme];
        let r_colors = ui::derive_ui_colors(r_theme);

        for i in 0..(5 + attach_lines) {
            queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(r_colors.menu_bg), Clear(ClearType::UntilNewLine)).unwrap();
        }

        let header_title = format!("View Email ({})", active_email);
        queue!(stdout, cursor::MoveTo(0, 0), SetForegroundColor(r_colors.accent), Print(header_title)).unwrap();

        let fields = ["From:", "To:", "Cc:", "Subject:"];
        let vals = [&email_from, &email_to, &email_cc, &email_subject];

        for i in 0..4 {
            queue!(
                        stdout,
                        cursor::MoveTo(0, (i + 1) as u16),
                        SetBackgroundColor(r_colors.menu_bg),
                        SetForegroundColor(r_colors.accent),
                        Print(format!("{:>8}", fields[i])),
                        SetForegroundColor(r_colors.fg),
                        Print(" "),
                        Print(vals[i]),
                        Clear(ClearType::UntilNewLine)
                    ).unwrap();
        }

        queue!(
                    stdout,
                    cursor::MoveTo(0, 5),
                    SetBackgroundColor(r_colors.menu_bg),
                    SetForegroundColor(r_colors.accent),
                    Print(" Attach: "),
                    Clear(ClearType::UntilNewLine)
                ).unwrap();

        if attachments.is_empty() {
            queue!(
                        stdout,
                        cursor::MoveTo(0, 5),
                        SetBackgroundColor(r_colors.menu_bg),
                        SetForegroundColor(r_colors.accent),
                        Print(" Attach: "),
                        SetForegroundColor(if r_colors.is_dark { Color::DarkGrey } else { Color::Grey }),
                        Print("None"),
                        Clear(ClearType::UntilNewLine)
                    ).unwrap();
        } else {
            queue!(
                        stdout,
                        cursor::MoveTo(0, 5),
                        SetBackgroundColor(r_colors.menu_bg),
                        SetForegroundColor(r_colors.accent),
                        Print(" Attach: "),
                        SetForegroundColor(if r_colors.is_dark { Color::DarkGrey } else { Color::Grey }),
                        Print("'1' to open, 'Meta+1' (ALT+1) to save, 'Meta+0' to save all"),
                        Clear(ClearType::UntilNewLine)
                    ).unwrap();

            let att_color = if r_colors.is_dark {
                Color::Rgb { r: 255, g: 80, b: 80 }
            } else {
                Color::Rgb { r: 220, g: 0, b: 0 }
            };

            for (i, (n, data)) in attachments.iter().enumerate() {
                let size_kb = (data.len() as f32 / 1024.0).max(1.0);
                let size_str = if size_kb < 1024.0 { format!("{:.0}K", size_kb) } else { format!("{:.1}M", size_kb / 1024.0) };
                // Indent the list so it aligns nicely under the header
                let att_str = format!("         {}. {} ({})", i + 1, n, size_str);

                queue!(
                            stdout,
                            cursor::MoveTo(0, (6 + i) as u16),
                            SetBackgroundColor(r_colors.menu_bg),
                            SetForegroundColor(att_color),
                            Print(att_str),
                            Clear(ClearType::UntilNewLine)
                        ).unwrap();
            }
        }

        queue!(stdout, ResetColor).unwrap();

        reader.draw_screen().unwrap();

        let timeout = if let Some(time) = reader.status_time {
            let elapsed = time.elapsed();
            if elapsed >= Duration::from_secs(3) {
                reader.clear_status();
                Duration::from_millis(1)
            } else {
                Duration::from_secs(3) - elapsed
            }
        } else {
            Duration::from_secs(3600)
        };

        if event::poll(timeout).unwrap() {
            let ev = event::read().unwrap();
            if let event::Event::Key(key) = ev {
                if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == event::KeyCode::Char('y') {
                    reader.set_status("Text copied to clipboard".to_string());
                    continue;
                }

                // --- Handle Alt+Number to save attachments ---
                if key.modifiers.contains(event::KeyModifiers::ALT) {
                    if let event::KeyCode::Char(c) = key.code {
                        // NEW: Catch Alt+0 to save all attachments
                        if c == '0' {
                            if !attachments.is_empty() {
                                // Pass the special flag to trigger directory-only selection
                                if let Ok(Some(save_dir)) = reader.run_file_browser(true, Some("<DIR_ONLY>")) {

                                    // --- NEW: Add the Confirmation Prompt ---
                                    let prompt_msg = format!("Save all attachments to '{}'?", save_dir);

                                    if let Ok(Some(true)) = reader.prompt_yn(&prompt_msg) {
                                        let mut success_count = 0;
                                        let target_dir = std::path::Path::new(&save_dir);

                                        // Save each attachment into the chosen directory
                                        for (filename, data) in attachments {
                                            let file_path = target_dir.join(filename);
                                            if std::fs::write(&file_path, data).is_ok() {
                                                success_count += 1;
                                            }
                                        }

                                        if success_count == attachments.len() {
                                            reader.set_status(format!("Saved {} attachments to {}", success_count, save_dir));
                                        } else {
                                            reader.set_status(format!("Saved {}/{} attachments to {}", success_count, attachments.len(), save_dir));
                                        }
                                    } else {
                                        reader.set_status("Save all cancelled.".to_string());
                                    }
                                    // ----------------------------------------

                                } else {
                                    reader.set_status("Save all cancelled.".to_string());
                                }
                                continue;
                            }
                        } else if c.is_ascii_digit() && c != '0' {
                            let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
                            if idx < attachments.len() {
                                let (filename, data) = &attachments[idx];

                                if let Ok(Some(save_path)) = reader.run_file_browser(true, Some(filename.as_str())) {
                                    if std::fs::write(&save_path, data).is_ok() {
                                        reader.set_status(format!("Saved {} to {}", filename, save_path));
                                    } else {
                                        reader.set_status(format!("Failed to save {}", filename));
                                    }
                                } else {
                                    reader.set_status("Save cancelled.".to_string());
                                }
                                continue;
                            }
                        }
                    }
                }

                if !key.modifiers.contains(event::KeyModifiers::CONTROL) && !key.modifiers.contains(event::KeyModifiers::ALT) {

                    if key.code == event::KeyCode::Char('a') || key.code == event::KeyCode::Char('A') {
                        if let Ok(added) = address::add_to_address_book(&email_from) {
                            if added {
                                reader.set_status(format!("Added {} to address book.", email_from));
                            } else {
                                reader.set_status("Address already in address book".to_string());
                            }
                        }
                        continue;
                    }
                    if key.code == event::KeyCode::Char('r') || key.code == event::KeyCode::Char('R') {
                        let reply_body = mail::format_reply_text(&text_body, &date, &email_from);

                        let sub = if email_subject.to_lowercase().starts_with("re:") {
                            email_subject.clone()
                        } else {
                            format!("Re: {}", email_subject)
                        };

                        let raw_reply = if reply_to.trim().is_empty() {
                            mail::extract_email(&email_from)
                        } else {
                            mail::extract_email(&reply_to)
                        };

                        // ONLY apply the 'Answered' flag if compose_email returns a success status
                        if let Some(s) = compose::compose_email(
                            &app.active_account,
                            Some(&raw_reply),
                            Some(&sub),
                            Some(&reply_body),
                            &mut reader.current_theme
                        ) {
                            if let Some(sess) = session {
                                match sess {
                                    net::MailSession::Imap(imap_sess) => {
                                        let _ = imap_sess.store(&fetch_seq, "+FLAGS (\\Answered)");
                                    }
                                    net::MailSession::Graph { .. } => {}
                                }
                            }

                            // Update the local UI state immediately after sending
                            app.page_emails[app.selected_index].is_answered = true;

                            reader.set_status(s);
                        }

                        continue;
                    }
                    if key.code == event::KeyCode::Char('f') || key.code == event::KeyCode::Char('F') {
                        let sub = if email_subject.to_lowercase().starts_with("fwd:") { email_subject.clone() } else { format!("Fwd: {}", email_subject) };
                        let fwd_body = format!("\n\n--- Forwarded message ---\nFrom: {}\nDate: {}\nSubject: {}\n\n{}", email_from, date, email_subject, text_body);
                        if let Some(s) = compose::compose_email(&app.active_account, None, Some(&sub), Some(&fwd_body), &mut reader.current_theme) {
                            reader.set_status(s);
                        }
                        continue;
                    }
                    if key.code == event::KeyCode::Char('b') || key.code == event::KeyCode::Char('B') {
                        let temp_dir = std::env::temp_dir().join("xpine_attachments");
                        let _ = std::fs::create_dir_all(&temp_dir);

                        let opened = if let Some(html) = &html_body {
                            if !html.is_empty() {
                                let file_path = temp_dir.join("email_view.html");
                                if std::fs::write(&file_path, html).is_ok() {
                                    if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                        reader.set_status("Opened HTML version in browser.".to_string());
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        if !opened {
                            let file_path = temp_dir.join("email_view.txt");
                            if std::fs::write(&file_path, text_body).is_ok() {
                                if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                    reader.set_status("Opened text version in browser.".to_string());
                                } else {
                                    reader.set_status("Failed to open browser.".to_string());
                                }
                            } else {
                                reader.set_status("Failed to save text file.".to_string());
                            }
                        }
                        continue;
                    }

                    if key.code == event::KeyCode::Char('s') || key.code == event::KeyCode::Char('S') {
                        if let Ok(Some(path)) = reader.run_file_browser(true, None) {
                            if std::fs::write(&path, text_body.as_bytes()).is_ok() {                                        reader.set_status(format!("Saved to {}", path));
                            } else {
                                reader.set_status(format!("Failed to save to {}", path));
                            }
                        }
                        continue;
                    }

                    if let event::KeyCode::Char(c) = key.code {
                        if c.is_ascii_digit() && c != '0' {
                            let idx = (c.to_digit(10).unwrap() as usize).saturating_sub(1);
                            if idx < attachments.len() {
                                let (filename, data) = &attachments[idx];
                                let temp_dir = std::env::temp_dir().join("xpine_attachments");
                                let _ = std::fs::create_dir_all(&temp_dir);
                                let file_path = temp_dir.join(filename);
                                if std::fs::write(&file_path, data).is_ok() {
                                    if webbrowser::open(file_path.to_str().unwrap()).is_ok() {
                                        reader.set_status(format!("Opened {}", filename));
                                    } else {
                                        reader.set_status(format!("Failed to open {}", filename));
                                    }
                                } else {
                                    reader.set_status(format!("Failed to save {}", filename));
                                }
                                continue;
                            }
                        }
                    }
                }

                match reader.handle_keypress(key).unwrap() {
                    EditorResult::Cancel => break,
                    _ => {}
                }
            } else if let event::Event::Resize(_, _) = ev {}
        }
    }
    settings_provider.current_theme = reader.current_theme;

    if matches!(app.mode, AppMode::EmailRead { .. }) {
        app.mode = AppMode::EmailList;
    }

    app.last_fetch_time = Instant::now();

    app.mode = AppMode::EmailList;
}
