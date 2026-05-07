mod app;
mod compose;
mod config;
mod editor;
mod events;
mod mail;
mod net;
mod spell;
mod ui;

use app::{App, AppMode};
use config::load_config;
use editor::{Editor, MenuState, EditorResult};
use ui::UiExt;

use ropey::Rope;
use crossterm::{
    cursor, event, execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, size as term_size, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::stdout;
use std::time::{Duration, Instant};

fn main() {
    let config = load_config();
    let mut app = App::new(config.accounts);

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    let mut theme_provider = Editor::new(None);
    let mut session = net::connect(&app.active_account.email, &app.active_account.password).expect("Initial IMAP Login failed");

    loop {
        if app.needs_reconnect {
            app.active_account = app.accounts[app.current_account_idx].clone();
            let _ = session.logout();
            session = net::connect(&app.active_account.email, &app.active_account.password).expect("IMAP Login failed");
            app.needs_fetch = true;
            app.needs_reconnect = false;
            app.last_fetch_time = Instant::now();
        }

        if app.last_fetch_time.elapsed() >= app.auto_refresh_interval {
            app.needs_fetch = true;
        }

        let (_, rows) = term_size().unwrap_or((80, 24));
        let items_per_page = (rows.saturating_sub(3) as u32).max(1);
        let total_pages = if app.total_messages == 0 { 1 } else { (app.total_messages + items_per_page - 1) / items_per_page };

        if app.current_page >= total_pages {
            app.current_page = total_pages.saturating_sub(1);
            app.needs_fetch = true;
        }

        if app.needs_fetch && matches!(app.mode, AppMode::List) {
            net::fetch_emails(&mut session, &mut app, items_per_page);
            app.last_fetch_time = Instant::now();
            app.needs_fetch = false;
        }

        if let AppMode::Reading { text_body, html_body: _, attachments } = &app.mode {
            let mut reader = Editor::new(None);
            reader.menu_state = MenuState::EmailReader;
            reader.top_margin = 6;
            reader.buffer = Rope::from_str(text_body.as_str());
            reader.current_theme = theme_provider.current_theme.clone();

            let email_from = app.page_emails[app.selected_index].from.clone();
            let email_to = app.page_emails[app.selected_index].to_addr.clone();
            let email_cc = app.page_emails[app.selected_index].cc.clone();
            let email_subject = app.page_emails[app.selected_index].subject.clone();
            let active_email = app.active_account.email.clone();

            loop {
                let r_theme = &reader.theme_set.themes[&reader.current_theme];
                let r_colors = ui::derive_ui_colors(r_theme);

                for i in 0..6 {
                    queue!(stdout, cursor::MoveTo(0, i as u16), SetBackgroundColor(r_colors.ui_bg), Clear(ClearType::UntilNewLine)).unwrap();
                }

                let header_title = format!("View Email ({})", active_email);
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

                if event::poll(Duration::from_secs(3600)).unwrap() {
                    let ev = event::read().unwrap();

                    if let event::Event::Key(mut key) = ev {
                        if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == event::KeyCode::Char('y') {
                            reader.set_status("Text copied to clipboard".to_string());
                            continue;
                        }

                        match reader.handle_keypress(key).unwrap() {
                            EditorResult::Cancel => break,
                            _ => {}
                        }
                    } else if let event::Event::Resize(_, _) = ev {
                        // Resizing naturally triggers a loop iteration to redraw
                    }
                }
            }
            theme_provider.current_theme = reader.current_theme;
            app.mode = AppMode::List;
            continue;
        }

        ui::draw_app(&mut stdout, &app, &theme_provider).unwrap();

        let mut timeout = if app.last_fetch_time.elapsed() >= app.auto_refresh_interval { Duration::from_millis(1) } else { app.auto_refresh_interval - app.last_fetch_time.elapsed() };

        if let Some(time) = app.list_status_time {
            let elapsed = time.elapsed();
            if elapsed >= app.list_status_duration {
                app.list_status.clear();
                app.list_status_time = None;
                timeout = Duration::from_millis(1);
            } else {
                timeout = timeout.min(app.list_status_duration - elapsed);
            }
        }

        if event::poll(timeout).unwrap() {
            if events::handle_event(event::read().unwrap(), &mut app, &mut session, &mut theme_provider, &mut stdout) {
                break;
            }
        }
    }

    execute!(stdout, LeaveAlternateScreen).unwrap();
    disable_raw_mode().expect("Failed to disable raw mode");
    let _ = session.logout();
}