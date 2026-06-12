mod app;
mod address;
mod compose;
pub mod config;
mod editor;
mod events;
mod mail;
mod net;
mod spell;
mod ui;
pub mod theme;
pub mod syntax;
pub mod search;
mod prompt;
mod read;
mod browser;

use app::{App, AppMode};
use config::load_config;
use editor::{Editor};
use crossterm::{
    cursor, event, execute,
    terminal::{disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::stdout;
use std::time::{Duration, Instant};

fn main() {
    let config = load_config();
    let mut app = App::new(config.accounts);

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    // Set a panic hook to safely restore the terminal if the app crashes
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Suppress errors here since we are already panicking
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);

        // Run the default panic hook to print the error
        original_hook(panic_info);
    }));

    if let Err(e) = theme::ensure_themes_unpacked() {
        eprintln!("Warning: Failed to unpack default asset themes to disk: {}", e);
    }

    let mut settings_provider = Editor::new(None);

    let mut session = None;
    if !app.accounts.is_empty() {
        net::reconnect(&mut app, &mut session);
    }

    loop {
        // 2. The Reconnect Block shrinks to this:
        if app.needs_reconnect {
            net::reconnect(&mut app, &mut session);
        }

        if app.last_fetch_time.elapsed() >= app.auto_refresh_interval {
            app.needs_fetch = true;
            // Add this line to reset the timer and stop the 1ms spin loop!
            app.last_fetch_time = Instant::now();
        }

        let (_, rows) = term_size().unwrap_or((80, 24));
        let items_per_page = (rows.saturating_sub(3) as u32).max(1);
        let total_pages = if app.total_messages == 0 { 1 } else { (app.total_messages + items_per_page - 1) / items_per_page };

        if app.current_page >= total_pages {
            app.current_page = total_pages.saturating_sub(1);
            app.needs_fetch = true;
        }

        if app.needs_fetch && matches!(app.mode, AppMode::EmailList) {
            if let Some(ref mut s) = session {
                net::fetch_emails(s, &mut app, items_per_page, settings_provider.sort_newest_first);
            }
            app.last_fetch_time = Instant::now();
            app.needs_fetch = false;
        }

        if matches!(app.mode, AppMode::EmailRead { .. }) {
            if let AppMode::EmailRead { text_body, html_body, attachments } = std::mem::replace(&mut app.mode, AppMode::EmailList) {

                read::view_email(
                    &mut app, &mut session, &mut settings_provider, &mut stdout,
                    &text_body, &html_body, &attachments
                );

                app.last_fetch_time = Instant::now();
                continue;
            }
        }

        ui::draw_app(&mut stdout, &app, &settings_provider).unwrap();

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
            let ev = event::read().unwrap();
            let handle_start = Instant::now();

            if events::handle_event(ev, &mut app, &mut session, &mut settings_provider, &mut stdout) {
                break;
            }

            if handle_start.elapsed() > Duration::from_secs(2) {
                app.last_fetch_time = Instant::now();
            }
        }
    }

    execute!(stdout, cursor::Show, LeaveAlternateScreen).unwrap();
    disable_raw_mode().expect("Failed to disable raw mode");

    if let Some(s) = session {
        match s {
            net::MailSession::Imap(mut imap_sess) => { let _ = imap_sess.logout(); }
            net::MailSession::Graph { .. } => {}
        }
    }
}

