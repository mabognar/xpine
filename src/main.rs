mod address;
mod app;
mod browser;
mod compose;
pub mod config;
mod editor;
mod events;
mod mail;
mod net;
mod prompt;
mod read;
pub mod search;
mod spell;
pub mod syntax;
pub mod theme;
mod ui;

use std::io::stdout;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    cursor, event, execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use app::{App, AppMode};
use config::load_config;
use editor::Editor;
use net::{MailSession, SessionPool};

const IMAP_FLUSH_DELAY: Duration = Duration::from_secs(2);
const LONG_ACTION_THRESHOLD: Duration = Duration::from_secs(2);
const GITHUB_RELEASE_URL: &str = "https://api.github.com/repos/mabognar/xpine/releases/latest";

fn restore_terminal() {
    let _ = execute!(stdout(), cursor::Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn spawn_update_check() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok(response) = net::http_client()
            .get(GITHUB_RELEASE_URL)
            .header(reqwest::header::USER_AGENT, "xpine-updater")
            .send()
        {
            if let Ok(json) = response.json::<serde_json::Value>() {
                if let Some(tag) = json["tag_name"].as_str() {
                    let _ = tx.send(tag.to_string());
                }
            }
        }
    });
    rx
}

fn fetch_current_page(
    app: &mut App,
    session: &mut Option<MailSession>,
    session_pool: &mut SessionPool,
    settings_provider: &Editor,
    items_per_page: u32,
) {
    // Attempt once, and transparently reconnect + retry if a pooled socket or Graph token expired
    for attempt in 0..2 {
        if let Some(s) = session.as_mut() {
            net::fetch_emails(
                s,
                app,
                items_per_page,
                settings_provider.sort_newest_first,
                settings_provider.use_dmy_dates,
            );
        }

        if !app.needs_reconnect || attempt == 1 {
            break;
        }
        net::reconnect(app, session, session_pool);
    }

    app.cache_ready = session.is_some() && !app.needs_reconnect;
    app.needs_reconnect = false;
    app.last_fetch_time = Instant::now();
    app.needs_fetch = false;
}

fn main() {
    let config = load_config();
    let mut app = App::new(config.accounts);

    if let Err(e) = theme::ensure_themes_unpacked() {
        eprintln!(
            "Warning: Failed to unpack default asset themes to disk: {}",
            e
        );
    }

    enable_raw_mode().expect("Failed to enable raw mode");
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        restore_terminal();
        original_hook(panic_info);
    }));

    let mut settings_provider = Editor::new(None);
    let mut session = None;
    let mut session_pool = SessionPool::new();
    if !app.accounts.is_empty() {
        net::reconnect(&mut app, &mut session, &mut session_pool);
    }

    let update_rx = spawn_update_check();

    loop {
        if let Ok(version) = update_rx.try_recv() {
            app.latest_version = Some(version);
            app.needs_fetch = true;
        }

        if app.needs_reconnect {
            net::reconnect(&mut app, &mut session, &mut session_pool);
        }

        if app.last_fetch_time.elapsed() >= app.auto_refresh_interval {
            app.needs_fetch = true;
            app.last_fetch_time = Instant::now();
        }

        let (_, rows) = term_size().unwrap_or((80, 24));
        let items_per_page = (rows.saturating_sub(3) as u32).max(1);
        let total_pages = app.total_messages.div_ceil(items_per_page).max(1);

        if app.current_page >= total_pages {
            app.current_page = total_pages - 1;
            app.needs_fetch = true;
        }

        if app.needs_fetch && matches!(app.mode, AppMode::EmailList) {
            fetch_current_page(
                &mut app,
                &mut session,
                &mut session_pool,
                &settings_provider,
                items_per_page,
            );
        }

        if matches!(app.mode, AppMode::EmailRead { .. }) {
            let AppMode::EmailRead {
                text_body,
                html_body,
                attachments,
            } = std::mem::replace(&mut app.mode, AppMode::EmailList)
            else {
                unreachable!();
            };

            read::view_email(
                &mut app,
                &mut session,
                &mut settings_provider,
                &mut stdout,
                &text_body,
                &html_body,
                &attachments,
            );

            app.last_fetch_time = Instant::now();
            continue;
        }

        ui::draw_app(&mut stdout, &app, &settings_provider).unwrap();

        let mut timeout = app
            .auto_refresh_interval
            .saturating_sub(app.last_fetch_time.elapsed())
            .max(Duration::from_millis(1));

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

        if !app.pending_imap_flags.is_empty() {
            if let Some(last_flag_time) = app.last_imap_flag_time {
                let elapsed = last_flag_time.elapsed();
                if elapsed >= IMAP_FLUSH_DELAY {
                    net::flush_pending_flags(&mut session, &mut app);
                    if app.needs_reconnect {
                        continue;
                    }
                } else {
                    timeout = timeout.min(IMAP_FLUSH_DELAY - elapsed);
                }
            }
        }

        if event::poll(timeout).unwrap() {
            let ev = event::read().unwrap();
            let is_key_event = matches!(ev, event::Event::Key(_));
            let handle_start = Instant::now();

            if events::handle_event(
                ev,
                &mut app,
                &mut session,
                &mut session_pool,
                &mut settings_provider,
                &mut stdout,
            ) {
                break;
            }

            if is_key_event && !app.pending_imap_flags.is_empty() {
                app.last_imap_flag_time = Some(Instant::now());
            }

            if handle_start.elapsed() > LONG_ACTION_THRESHOLD {
                app.last_fetch_time = Instant::now();
            }
        }
    }

    restore_terminal();
    net::flush_pending_flags(&mut session, &mut app);
    drop(session);
    session_pool.close_all();
}