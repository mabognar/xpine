use crate::app::App;
use crate::mail::{EmailMeta, parse_email_body};
use chrono::{DateTime, Local, Utc};
use native_tls::TlsConnector;
use imap::Session;
use std::net::TcpStream;
use crate::config::Account; // Or crate::mail::Account depending on your imports
use reqwest::blocking::Client;
use serde::Deserialize;

pub type ImapSession = Session<native_tls::TlsStream<TcpStream>>;

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    // expires_in: u64, // Not strictly needed right now since we fetch fresh
    // token_type: String,
}

struct OAuth2 {
    user: String,
    access_token: String,
}

impl imap::Authenticator for OAuth2 {
    type Response = String;
    #[allow(unused_variables)]
    fn process(&self, data: &[u8]) -> Self::Response {
        // The XOAUTH2 format required by Gmail: user={email}^Aauth=Bearer {token}^A^A
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

// pub fn get_google_access_token(client_id: &str, client_secret: &str, refresh_token: &str) -> Result<String, String> {
//     let client = Client::new();
//     let params = [
//         ("client_id", client_id),
//         ("client_secret", client_secret),
//         ("refresh_token", refresh_token),
//         ("grant_type", "refresh_token"),
//     ];
//
//     let res = client.post("https://oauth2.googleapis.com/token")
//         .form(&params)
//         .send()
//         .map_err(|e| format!("Network error: {}", e))?;
//
//     if res.status().is_success() {
//         let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
//         Ok(token_res.access_token)
//     } else {
//         Err(format!("OAuth error: {:?}", res.text().unwrap_or_default()))
//     }
// }

// pub fn get_oauth_access_token(
//     client_id: &str,
//     client_secret: &str,
//     refresh_token: &str,
//     is_microsoft: bool,
// ) -> Result<String, String> {
//     let client = reqwest::blocking::Client::new();
//     let params = [
//         ("client_id", client_id),
//         ("client_secret", client_secret),
//         ("refresh_token", refresh_token),
//         ("grant_type", "refresh_token"),
//     ];
//
//     // Swap the endpoint based on the provider
//     let endpoint = if is_microsoft {
//         "https://login.microsoftonline.com/common/oauth2/v2.0/token"
//     } else {
//         "https://oauth2.googleapis.com/token"
//     };
//
//     let res = client.post(endpoint)
//         .form(&params)
//         .send()
//         .map_err(|e| format!("Network error: {}", e))?;
//
//     if res.status().is_success() {
//         // Assuming you have a TokenResponse struct that derives Deserialize
//         let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
//         Ok(token_res.access_token)
//     } else {
//         Err(format!("OAuth error: {:?}", res.text().unwrap_or_default()))
//     }
// }

// pub fn get_oauth_access_token(client_id: &str, client_secret: &str, refresh_token: &str, is_microsoft: bool) -> Result<String, String> {
//     let client = reqwest::blocking::Client::new();
//     let params = [
//         ("client_id", client_id),
//         ("client_secret", client_secret),
//         ("refresh_token", refresh_token),
//         ("grant_type", "refresh_token"),
//     ];
//
//     let endpoint = if is_microsoft {
//         "https://login.microsoftonline.com/common/oauth2/v2.0/token"
//     } else {
//         "https://oauth2.googleapis.com/token"
//     };
//
//     let res = client.post(endpoint)
//         .form(&params)
//         .send()
//         .map_err(|e| format!("Network error: {}", e))?;
//
//     if res.status().is_success() {
//         let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
//         Ok(token_res.access_token)
//     } else {
//         // --- DEBUG INJECTION 1 ---
//         let err_text = res.text().unwrap_or_default();
//         std::fs::write("oauth_debug.txt", format!("TOKEN FETCH FAILED: {}", err_text)).ok();
//         Err(format!("OAuth error: {:?}", err_text))
//     }
// }

pub fn get_oauth_access_token(client_id: &str, client_secret: &str, refresh_token: &str, is_microsoft: bool) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();

    let mut params = vec![
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    // Force Microsoft to issue an Outlook API token instead of a Graph API token
    if is_microsoft {
        params.push(("scope", "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send"));
    }

    let endpoint = if is_microsoft {
        "https://login.microsoftonline.com/common/oauth2/v2.0/token"
    } else {
        "https://oauth2.googleapis.com/token"
    };

    let res = client.post(endpoint)
        .form(&params)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if res.status().is_success() {
        let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
        Ok(token_res.access_token)
    } else {
        let err_text = res.text().unwrap_or_default();
        std::fs::write("oauth_debug.txt", format!("TOKEN FETCH FAILED: {}", err_text)).ok();
        Err(format!("OAuth error: {:?}", err_text))
    }
}

// pub fn connect(account: &mut Account) -> Result<ImapSession, imap::Error> {
//     let domain = account.imap_server.as_str();
//     let port = account.imap_port;
//     let tls = TlsConnector::builder().build().unwrap();
//
//     // Connect using the dynamic domain and port
//     let client = imap::connect((domain, port), domain, &tls).unwrap();
//
//     // Try OAuth 2.0 First
//     if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
//         (&account.client_id, &account.client_secret, &account.refresh_token) {
//
//         let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");
//
//         match get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft) {
//             Ok(access_token) => {
//                 let auth = OAuth2 {
//                     user: account.email.clone(),
//                     access_token,
//                 };
//                 // Authenticate using the custom XOAUTH2 implementation
//                 return client.authenticate("XOAUTH2", &auth).map_err(|(e, _)| e);
//             }
//             Err(e) => {
//                 eprintln!("Failed to get OAuth token: {}", e);
//                 // If it fails, it will drop down to try the password fallback
//             }
//         }
//     }
//
//     // Fallback to standard app passwords
//     if let Some(password) = &account.password {
//         client.login(&account.email, password).map_err(|(e, _)| e)
//     } else {
//         Err(imap::Error::Bad("No password or valid OAuth credentials provided in xpinerc".to_string()))
//     }
// }

pub fn connect(account: &mut Account) -> Result<ImapSession, imap::Error> {
    let domain = account.imap_server.as_str();
    let port = account.imap_port;
    let tls = TlsConnector::builder().build().unwrap();

    // Make client mutable so we can re-assign it if OAuth fails
    let mut client = imap::connect((domain, port), domain, &tls).unwrap();

    if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&account.client_id, &account.client_secret, &account.refresh_token) {

        let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

        match get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft) {
            Ok(access_token) => {
                let auth = OAuth2 {
                    user: account.email.clone(),
                    access_token,
                };

                // --- DEBUG INJECTION 2 ---
                match client.authenticate("XOAUTH2", &auth) {
                    Ok(session) => return Ok(session),
                    Err((e, returned_client)) => {
                        std::fs::write("oauth_debug.txt", format!("IMAP REJECTED TOKEN: {:?}", e)).ok();
                        // Restore ownership of the client so it survives to the password fallback
                        client = returned_client;
                    }
                }
            }
            Err(_) => {}
        }
    }

    if let Some(password) = &account.password {
        client.login(&account.email, password).map_err(|(e, _)| e)
    } else {
        Err(imap::Error::Bad("No password or valid OAuth credentials provided".to_string()))
    }
}

pub fn fetch_emails(session: &mut ImapSession, app: &mut App, items_per_page: u32, sort_newest_first: bool) {
    app.page_emails.clear();

    match session.select(&app.current_folder) {
        Ok(m) => app.total_messages = m.exists,
        Err(_) => { app.needs_reconnect = true; return; }
    }

    // let sequence = if let Some(ref q) = app.search_query {
    //
    //     let query = if q.trim() == "*" {
    //         String::from("FLAGGED")
    //     } else {
    //         format!("OR FROM \"{}\" OR SUBJECT \"{}\" CC \"{}\"", q, q, q)
    //     };

    let sequence = if let Some(ref q) = app.search_query {

        let query = if q.trim() == "*" {
            String::from("FLAGGED")
        } else {
            // ONLY search within the From and Subject fields
            format!("OR FROM \"{}\" SUBJECT \"{}\"", q, q)
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