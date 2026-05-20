use crate::app::App;
use crate::mail::{EmailMeta, parse_email_body};
use chrono::{DateTime, Local, Utc};
use native_tls::TlsConnector;
use imap::Session;
use std::net::TcpStream;
use crate::config::Account; // Or crate::mail::Account depending on your imports

pub type ImapSession = Session<native_tls::TlsStream<TcpStream>>;

pub fn connect(account: &Account) -> Result<ImapSession, imap::Error> {
    let domain = account.imap_server.as_str();
    let port = account.imap_port;
    let tls = TlsConnector::builder().build().unwrap();

    // Connect using the dynamic domain and port
    let client = imap::connect((domain, port), domain, &tls).unwrap();
    client.login(&account.email, &account.password).map_err(|(e, _)| e)
}

pub fn fetch_emails(session: &mut ImapSession, app: &mut App, items_per_page: u32, sort_newest_first: bool) {
    app.page_emails.clear();

    match session.select(&app.current_folder) {
        Ok(m) => app.total_messages = m.exists,
        Err(_) => { app.needs_reconnect = true; return; }
    }

    // // Determine the sequence of emails to fetch based on whether a search query is active
    // let sequence = if let Some(ref q) = app.search_query {
    //     // IMAP search string chaining ORs to search multiple fields
    //     let query = format!("OR FROM \"{q}\" OR SUBJECT \"{q}\" OR TO \"{q}\" CC \"{q}\"");
    //
    //     match session.search(&query) {
    //         Ok(seq_ids) if !seq_ids.is_empty() => {
    //             app.total_messages = seq_ids.len() as u32;
    //
    //             // Collect and sort sequence IDs to preserve correct oldest-to-newest pagination
    //             let mut sorted_seqs: Vec<u32> = seq_ids.into_iter().collect();
    //             sorted_seqs.sort();
    //
    //             // Paginate the search results
    //             let end_idx = sorted_seqs.len().saturating_sub((app.current_page * items_per_page) as usize);
    //             let start_idx = end_idx.saturating_sub(items_per_page as usize - 1).max(1);
    //
    //             // Extract sequence IDs for the current page and join them with commas for the fetch command
    //             let page_seqs = &sorted_seqs[(start_idx - 1)..end_idx];
    //             page_seqs.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
    //         }
    //         _ => {
    //             app.total_messages = 0;
    //             return; // Break out early if no search results match
    //         }
    //     }
    // } else {
    // Determine the sequence of emails to fetch based on whether a search query is active
    let sequence = if let Some(ref q) = app.search_query {

        // 1. Check if the user is searching for flagged/starred emails
        // 2. Otherwise, use TEXT to force a broader substring match across the email
        // let query = if q.trim() == "*" {
        //     String::from("FLAGGED")
        // } else {
        //     format!("TEXT \"{}\"", q)
        // };
        let query = if q.trim() == "*" {
            String::from("FLAGGED")
        } else {
            format!("OR FROM \"{}\" OR SUBJECT \"{}\" OR To \"{}\" CC \"{}\"", q, q, q, q)
        };

        match session.search(&query) {
            Ok(seq_ids) if !seq_ids.is_empty() => {
                app.total_messages = seq_ids.len() as u32;

                // Collect and sort sequence IDs to preserve correct oldest-to-newest pagination
                let mut sorted_seqs: Vec<u32> = seq_ids.into_iter().collect();
                sorted_seqs.sort();

                // Paginate the search results
                let end_idx = sorted_seqs.len().saturating_sub((app.current_page * items_per_page) as usize);
                let start_idx = end_idx.saturating_sub(items_per_page as usize - 1).max(1);

                // Extract sequence IDs for the current page and join them with commas for the fetch command
                let page_seqs = &sorted_seqs[(start_idx - 1)..end_idx];
                page_seqs.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
            }
            _ => {
                app.total_messages = 0;
                return; // Break out early if no search results match
            }
        }
    } else {
        // Standard unsearched fetch logic
        if app.total_messages > 0 {
            let end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
            let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
            format!("{}:{}", start_idx, end_idx)
        } else {
            return;
        }
    };

    if !sequence.is_empty() {
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
                let mut to_addr = String::new(); // Added
                let mut cc = String::new();      // Added
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

                    // A macro safely handles the imap Address lifetimes and joins multiple addresses safely
                    macro_rules! format_addrs {
                        ($addrs:expr) => {{
                            let mut result = Vec::new();
                            if let Some(a_vec) = $addrs {
                                for addr in a_vec {
                                    let name = addr.name.as_ref().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned()).unwrap_or_default();
                                    let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m.as_ref()).into_owned()).unwrap_or_default();
                                    let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h.as_ref()).into_owned()).unwrap_or_default();

                                    let email_raw = format!("{}@{}", mailbox, host);
                                    let formatted = if !name.is_empty() { format!("{} <{}>", name, email_raw) } else { email_raw };
                                    result.push(formatted);
                                }
                            }
                            result.join(", ")
                        }};
                    }

                    // Apply the macro to populate the fields
                    from = format_addrs!(env.from.as_ref());
                    if from.is_empty() { from = "Unknown Sender".to_string(); }

                    to_addr = format_addrs!(env.to.as_ref());
                    cc = format_addrs!(env.cc.as_ref());

                    // Safely extract a single reply_to address for the composer
                    if let Some(f_vec) = env.reply_to.as_ref().or(env.from.as_ref()) {
                        if let Some(addr) = f_vec.first() {
                            let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m.as_ref()).into_owned()).unwrap_or_default();
                            let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h.as_ref()).into_owned()).unwrap_or_default();
                            reply_to = format!("{}@{}", mailbox, host);
                        }
                    }
                }

                // Push the newly populated to_addr and cc into EmailMeta
                app.page_emails.push(EmailMeta {
                    id: message.message,
                    subject,
                    from,
                    reply_to,
                    reply_to_display: String::new(),
                    to_addr, // Updated
                    cc,      // Updated
                    date,
                    size,
                    is_read: is_seen,
                    is_deleted,
                    is_flagged,
                    is_answered
                });
            }
        }

        // Always sort initially by id for stability
        app.page_emails.sort_by(|a, b| a.id.cmp(&b.id));

        if sort_newest_first {
            app.page_emails.sort_by(|a, b| b.id.cmp(&a.id));
        } else {
            app.page_emails.sort_by(|a, b| a.id.cmp(&b.id));
        }

        if let Some(idx_from_end) = app.restore_index_from_end {
            if sort_newest_first {
                app.selected_index = if !app.page_emails.is_empty() { idx_from_end as usize } else { 0 };
            } else {
                app.selected_index = if !app.page_emails.is_empty() { app.page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize) } else { 0 };
            }
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