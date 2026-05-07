use crate::app::App;
use crate::mail::{EmailMeta, parse_email_body};
use chrono::{DateTime, Local, Utc};
use native_tls::TlsConnector;
use imap::Session;
use std::net::TcpStream;

pub type ImapSession = Session<native_tls::TlsStream<TcpStream>>;

pub fn connect(email: &str, password: &str) -> Result<ImapSession, imap::Error> {
    let domain = "imap.gmail.com";
    let tls = TlsConnector::builder().build().unwrap();
    let client = imap::connect((domain, 993), domain, &tls).unwrap();
    client.login(email, password).map_err(|(e, _)| e)
}

pub fn fetch_emails(session: &mut ImapSession, app: &mut App, items_per_page: u32) {
    app.page_emails.clear();

    match session.select(&app.current_folder) {
        Ok(m) => app.total_messages = m.exists,
        Err(_) => { app.needs_reconnect = true; return; }
    }

    if app.total_messages > 0 {
        let end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
        let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
        let sequence = format!("{}:{}", start_idx, end_idx);

        if let Ok(messages) = session.fetch(&sequence, "(ENVELOPE FLAGS RFC822.SIZE)") {
            for message in messages.iter() {
                let size = message.size.unwrap_or(0);
                let mut is_seen = false; let mut is_deleted = false;
                let mut is_flagged = false; let mut is_answered = false;

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
                let mut date = "Unknown Date".to_string();

                if let Some(env) = message.envelope() {
                    if let Some(s) = env.subject.as_ref() { subject = String::from_utf8_lossy(s).into_owned(); }
                    if let Some(d) = env.date.as_ref() {
                        let raw_date = String::from_utf8_lossy(d).into_owned();
                        if let Ok(dt) = DateTime::parse_from_rfc2822(&raw_date) {
                            let now = Utc::now().timestamp();
                            let diff = now - dt.timestamp();
                            let local_dt = dt.with_timezone(&Local);
                            date = if diff < 7 * 24 * 3600 && diff >= -86400 { local_dt.format("%a %H:%M").to_string() } else { local_dt.format("%b %d").to_string() };
                        } else {
                            date = raw_date.split(" +").next().unwrap_or(&raw_date).to_string();
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

                app.page_emails.push(EmailMeta { id: message.message, subject, from, reply_to, reply_to_display: String::new(), to_addr: String::new(), cc: String::new(), date, size, is_read: is_seen, is_deleted, is_flagged, is_answered });
            }
        }
        app.page_emails.sort_by(|a, b| a.id.cmp(&b.id));

        if let Some(idx_from_end) = app.restore_index_from_end {
            app.selected_index = if !app.page_emails.is_empty() { app.page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize) } else { 0 };
            app.restore_index_from_end = None;
        } else if app.selected_index >= app.page_emails.len() {
            app.selected_index = app.page_emails.len().saturating_sub(1);
        }
    }
}

pub fn toggle_imap_flag(session: &mut ImapSession, emails: &mut [EmailMeta], selected_index: usize, flag_name: &str) {
    if emails.is_empty() { return; }

    let seq_id = emails[selected_index].id.to_string();
    let is_set = match flag_name {
        "\\Flagged" => emails[selected_index].is_flagged,
        "\\Deleted" => emails[selected_index].is_deleted,
        "\\Seen"    => emails[selected_index].is_read,
        _ => false,
    };

    let op = if is_set { format!("-FLAGS ({})", flag_name) } else { format!("+FLAGS ({})", flag_name) };

    if session.store(&seq_id, &op).is_ok() {
        let new_val = !is_set;
        match flag_name {
            "\\Flagged" => emails[selected_index].is_flagged = new_val,
            "\\Deleted" => emails[selected_index].is_deleted = new_val,
            "\\Seen"    => emails[selected_index].is_read = new_val,
            _ => {}
        }
    }
}

pub fn fetch_email_body(session: &mut ImapSession, fetch_seq: &str) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    let mut text_body = String::from("Could not load email body.");
    let mut html_body = None;
    let mut attachments = Vec::new();

    if let Ok(fetched_messages) = session.fetch(fetch_seq, "RFC822") {
        if let Some(message) = fetched_messages.iter().next() {
            if let Some(body_data) = message.body() {
                let (t, h, a) = parse_email_body(body_data);
                text_body = t;
                html_body = h;
                attachments = a;
            }
        }
    }

    (text_body, html_body, attachments)
}