use crate::app::App;
use crate::mail::{EmailMeta, parse_email_body};
use crate::config::Account;
use std::io::{stdout};
use std::thread;
use std::time::Duration;
use chrono::{DateTime, Local, Utc};
use native_tls::TlsConnector;
use imap::Session;
use std::net::TcpStream;
use serde::Deserialize;
use crossterm::{
    cursor, execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};

// 1. Define our new wrapper type
pub type RawImapSession = Session<native_tls::TlsStream<TcpStream>>;

pub enum MailSession {
    Imap(RawImapSession),
    Graph { access_token: String },
}

struct OAuth2 {
    user: String,
    access_token: String,
}

#[derive(Deserialize, Debug)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

#[derive(Deserialize, Debug)]
struct PollingError {
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Deserialize, Debug)]
pub struct GraphMessageResponse {
    pub value: Vec<GraphMessage>,
    #[serde(rename = "@odata.count")]
    pub count: Option<u32>,
}

#[derive(Deserialize, Debug)]
pub struct GraphMessage {
    pub id: String,
    #[serde(rename = "receivedDateTime")]
    pub received_date_time: String,
    pub subject: Option<String>,
    pub from: Option<GraphRecipient>,
    #[serde(rename = "toRecipients")]
    pub to_recipients: Option<Vec<GraphRecipient>>,
    #[serde(rename = "ccRecipients")]
    pub cc_recipients: Option<Vec<GraphRecipient>>,
    #[serde(rename = "replyTo")]
    pub reply_to: Option<Vec<GraphRecipient>>,
    #[serde(rename = "isRead")]
    pub is_read: bool,
    pub flag: Option<GraphFlag>,
}

#[derive(Deserialize, Debug)]
pub struct GraphFlag {
    #[serde(rename = "flagStatus")]
    pub flag_status: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct GraphRecipient {
    #[serde(rename = "emailAddress")]
    pub email_address: Option<GraphEmailAddress>,
}

#[derive(Deserialize, Debug)]
pub struct GraphEmailAddress {
    pub name: Option<String>,
    pub address: Option<String>,
}

pub fn request_microsoft_device_code(client_id: &str) -> Result<DeviceCodeResponse, String> {
    let client = reqwest::blocking::Client::new();
    let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";

    let params = vec![
        ("client_id", client_id),
        ("scope", "offline_access https://graph.microsoft.com/Mail.ReadWrite"),
    ];

    let res = client.post(endpoint)
        .form(&params)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if res.status().is_success() {
        let auth_res: DeviceCodeResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
        Ok(auth_res)
    } else {
        let err_text = res.text().unwrap_or_default();
        Err(format!("Device flow request failed: {}", err_text))
    }
}

pub fn poll_microsoft_token(
    client_id: &str,
    client_secret: &str,
    device_code: &str,
    base_interval: u64,
) -> Result<TokenResponse, String> {
    let client = reqwest::blocking::Client::new();
    let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

    let mut current_interval = base_interval;

    loop {
        let mut params = vec![
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let res = client.post(endpoint)
            .form(&params)
            .send()
            .map_err(|e| format!("Network error during polling: {}", e))?;

        if res.status().is_success() {
            let token_res: TokenResponse = res.json()
                .map_err(|e| format!("Failed to parse token: {}", e))?;
            return Ok(token_res);
        } else {
            let err_res: PollingError = res.json()
                .map_err(|e| format!("Failed to parse polling error: {}", e))?;

            match err_res.error.as_str() {
                "authorization_pending" => {
                    thread::sleep(Duration::from_secs(current_interval));
                },
                "slow_down" => {
                    current_interval += 2;
                    thread::sleep(Duration::from_secs(current_interval));
                },
                "access_denied" => return Err("Authorization denied by the user.".to_string()),
                "expired_token" => return Err("The device code expired. Please try again.".to_string()),
                _ => return Err(format!("Unexpected OAuth error: {}", err_res.error)),
            }
        }
    }
}

pub fn run_microsoft_auth_flow(client_id: &str, client_secret: &str) -> Result<TokenResponse, String> {
    let auth_req = request_microsoft_device_code(client_id)?;

    let mut stdout = stdout();
    let _ = execute!(
        stdout,
        Clear(ClearType::All),
        cursor::MoveTo(0, 2),
        Print(" xpine - Microsoft OAuth2 Device Authorization\r\n\r\n"),
        ResetColor,
        Print(format!("   To authorize this account, please visit: {}\r\n", auth_req.verification_url)),
        Print("   And enter the following code:            "),
        SetForegroundColor(Color::Red),
        Print(format!("{}\r\n\r\n", auth_req.user_code)),
        ResetColor,
        Print("   Waiting for authorization (check your browser)...\r\n")
    );

    let token = poll_microsoft_token(client_id, client_secret, &auth_req.device_code, auth_req.interval)?;

    let _ = execute!(
        stdout,
        SetForegroundColor(Color::Green),
        Print("\r\n   Authorization successful! Returning to xpine...\r\n"),
        ResetColor
    );

    std::thread::sleep(std::time::Duration::from_millis(1500));

    Ok(token)
}

impl imap::Authenticator for OAuth2 {
    type Response = String;
    #[allow(unused_variables)]
    fn process(&self, data: &[u8]) -> Self::Response {
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

pub fn get_oauth_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    is_microsoft: bool,
    target_scope: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();

    let mut params = vec![
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    if let Some(scope) = target_scope {
        params.push(("scope", scope));
    }

    if !is_microsoft && !client_secret.is_empty() && client_secret != "YOUR_GOOGLE_CLIENT_SECRET" {
        params.push(("client_secret", client_secret));
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

pub fn connect(account: &mut Account) -> Result<MailSession, String> {
    let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

    // 1. Try OAuth Flow First
    if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&account.client_id, &account.client_secret, &account.refresh_token) {

        // Use Graph scopes for Microsoft, None for Gmail
        let scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.ReadWrite offline_access") } else { None };

        match get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft, scope) {
            Ok(access_token) => {
                if is_microsoft {
                    // THE FLIP: Bypass IMAP entirely and return a stateless Graph session!
                    return Ok(MailSession::Graph { access_token });
                } else {
                    // Standard IMAP XOAUTH2 for Gmail/Yahoo
                    let domain = account.imap_server.as_str();
                    let port = account.imap_port;
                    let tls = TlsConnector::builder().build().map_err(|e| e.to_string())?;
                    let mut client = imap::connect((domain, port), domain, &tls).map_err(|e| e.to_string())?;

                    let auth = OAuth2 {
                        user: account.email.clone(),
                        access_token,
                    };

                    match client.authenticate("XOAUTH2", &auth) {
                        Ok(session) => return Ok(MailSession::Imap(session)),
                        Err((e, returned_client)) => {
                            std::fs::write("oauth_debug.txt", format!("IMAP REJECTED TOKEN: {:?}", e)).ok();
                            client = returned_client;
                        }
                    }
                }
            }
            Err(_) => {}
        }
    }

    // 2. Fallback to Password-based IMAP if OAuth isn't configured
    let domain = account.imap_server.as_str();
    let port = account.imap_port;
    let tls = TlsConnector::builder().build().map_err(|e| e.to_string())?;

    let mut client = imap::connect((domain, port), domain, &tls).map_err(|e| e.to_string())?;

    if let Some(password) = &account.password {
        let session = client.login(&account.email, password).map_err(|(e, _)| e.to_string())?;
        Ok(MailSession::Imap(session))
    } else {
        Err("No password or valid OAuth credentials provided".to_string())
    }
}

// 3. Updated fetch_emails with the Enum match wrapper
pub fn fetch_emails(session: &mut MailSession, app: &mut App, items_per_page: u32, sort_newest_first: bool) {
    match session {
        MailSession::Imap(imap_sess) => {
            app.page_emails.clear();

            match imap_sess.select(&app.current_folder) {
                Ok(m) => app.total_messages = m.exists,
                Err(_) => { app.needs_reconnect = true; return; }
            }

            let sequence = if let Some(ref q) = app.search_query {
                let query = if q.trim() == "*" {
                    String::from("FLAGGED")
                } else {
                    format!("OR FROM \"{}\" SUBJECT \"{}\"", q, q)
                };

                match imap_sess.search(&query) {
                    Ok(seq_ids) if !seq_ids.is_empty() => {
                        app.total_messages = seq_ids.len() as u32;

                        let mut sorted_seqs: Vec<u32> = seq_ids.into_iter().collect();
                        sorted_seqs.sort();

                        let end_idx = sorted_seqs.len().saturating_sub((app.current_page * items_per_page) as usize);
                        let start_idx = end_idx.saturating_sub(items_per_page as usize - 1).max(1);

                        let page_seqs = &sorted_seqs[(start_idx - 1)..end_idx];
                        page_seqs.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
                    }
                    _ => {
                        app.total_messages = 0;
                        return;
                    }
                }
            } else {
                if app.total_messages > 0 {
                    let end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
                    let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
                    format!("{}:{}", start_idx, end_idx)
                } else {
                    return;
                }
            };

            if !sequence.is_empty() {
                if let Ok(messages) = imap_sess.fetch(&sequence, "(UID ENVELOPE FLAGS RFC822.SIZE INTERNALDATE)") {
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

                        let internal_date = message.internal_date()
                            .map(|dt| dt.timestamp())
                            .unwrap_or(0);

                        let mut subject = "No Subject".to_string();
                        let mut from = "Unknown Sender".to_string();
                        let mut reply_to = "unknown@example.com".to_string();
                        let mut to_addr = String::new();
                        let mut cc = String::new();
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

                            from = format_addrs!(env.from.as_ref());
                            if from.is_empty() { from = "Unknown Sender".to_string(); }

                            to_addr = format_addrs!(env.to.as_ref());
                            cc = format_addrs!(env.cc.as_ref());

                            if let Some(f_vec) = env.reply_to.as_ref().or(env.from.as_ref()) {
                                if let Some(addr) = f_vec.first() {
                                    let mailbox = addr.mailbox.as_ref().map(|m| String::from_utf8_lossy(m.as_ref()).into_owned()).unwrap_or_default();
                                    let host = addr.host.as_ref().map(|h| String::from_utf8_lossy(h.as_ref()).into_owned()).unwrap_or_default();
                                    reply_to = format!("{}@{}", mailbox, host);
                                }
                            }
                        }

                        app.page_emails.push(EmailMeta {
                            id: message.message.to_string(), // Upgraded to String
                            uid: message.uid.unwrap_or(0),
                            timestamp: internal_date,
                            subject,
                            from,
                            reply_to,
                            reply_to_display: String::new(),
                            to_addr,
                            cc,
                            date,
                            size,
                            is_read: is_seen,
                            is_deleted,
                            is_flagged,
                            is_answered,
                        });
                    }
                }

                if sort_newest_first {
                    app.page_emails.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                } else {
                    app.page_emails.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
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
        }, // <--- THIS is the brace that was likely missing!

        MailSession::Graph { access_token } => {
            app.page_emails.clear();

            let folder = if app.current_folder == "INBOX" { "inbox" } else { &app.current_folder };
            let skip = app.current_page * items_per_page;

            let order = if sort_newest_first { "receivedDateTime DESC" } else { "receivedDateTime ASC" };

            let mut url = format!(
                "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$count=true&$top={}&$skip={}&$orderby={}&$select=id,receivedDateTime,subject,from,toRecipients,ccRecipients,replyTo,isRead,flag",
                folder, items_per_page, skip, order
            );

            if let Some(ref q) = app.search_query {
                if q.trim() == "*" {
                    url.push_str("&$filter=flag/flagStatus eq 'flagged'");
                } else {
                    let encoded_q = urlencoding::encode(q);
                    url = format!("{}&$search=\"{}\"", url, encoded_q);
                }
            }

            let client = reqwest::blocking::Client::new();
            let res = client.get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("ConsistencyLevel", "eventual")
                .send();

            match res {
                Ok(response) if response.status().is_success() => {
                    if let Ok(graph_data) = response.json::<GraphMessageResponse>() {

                        if let Some(total) = graph_data.count {
                            app.total_messages = total;
                        }

                        for msg in graph_data.value {

                            let internal_date = DateTime::parse_from_rfc3339(&msg.received_date_time)
                                .map(|dt| dt.timestamp())
                                .unwrap_or(0);

                            let mut date_str = "Unknown Date".to_string();
                            if let Ok(dt) = DateTime::parse_from_rfc3339(&msg.received_date_time) {
                                let now = Utc::now().timestamp();
                                let diff = now - dt.timestamp();
                                let local_dt = dt.with_timezone(&Local);
                                date_str = if diff < 7 * 24 * 3600 && diff >= -86400 {
                                    local_dt.format("%a %H:%M").to_string()
                                } else {
                                    local_dt.format("%b %d").to_string()
                                };
                            }

                            macro_rules! format_graph_addrs {
                                ($addrs:expr) => {{
                                    let mut result = Vec::new();
                                    if let Some(a_vec) = $addrs {
                                        for recipient in a_vec {
                                            if let Some(email) = &recipient.email_address {
                                                let name = email.name.as_deref().unwrap_or("");
                                                let addr = email.address.as_deref().unwrap_or("");
                                                let formatted = if !name.is_empty() { format!("{} <{}>", name, addr) } else { addr.to_string() };
                                                result.push(formatted);
                                            }
                                        }
                                    }
                                    result.join(", ")
                                }};
                            }

                            let mut from = "Unknown Sender".to_string();
                            if let Some(f) = msg.from {
                                if let Some(email) = f.email_address {
                                    let name = email.name.unwrap_or_default();
                                    let addr = email.address.unwrap_or_default();
                                    from = if !name.is_empty() { format!("{} <{}>", name, addr) } else { addr };
                                }
                            }

                            let to_addr = format_graph_addrs!(msg.to_recipients);
                            let cc = format_graph_addrs!(msg.cc_recipients);
                            let reply_to = format_graph_addrs!(msg.reply_to);

                            let is_flagged = msg.flag.and_then(|f| f.flag_status).map_or(false, |s| s.to_lowercase() == "flagged");

                            app.page_emails.push(EmailMeta {
                                id: msg.id, // Native Graph String ID
                                uid: 0,
                                timestamp: internal_date,
                                subject: msg.subject.unwrap_or_else(|| "No Subject".to_string()),
                                from,
                                reply_to,
                                reply_to_display: String::new(),
                                to_addr,
                                cc,
                                date: date_str,
                                size: 0,
                                is_read: msg.is_read,
                                is_deleted: false,
                                is_flagged,
                                is_answered: false,
                            });
                        }

                        if let Some(idx_from_end) = app.restore_index_from_end {
                            if sort_newest_first {
                                app.selected_index = if !app.page_emails.is_empty() { idx_from_end as usize } else { 0 };
                            } else {
                                app.selected_index = if !app.page_emails.is_empty() { app.page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize) } else { 0 };
                            }
                            app.restore_index_from_end = None;
                        } else if app.selected_index >= app.page_emails.len() {
                            // If the cursor is out of bounds, snap it to the bottom of the current list!
                            app.selected_index = app.page_emails.len().saturating_sub(1);
                        }

                    } else {
                        app.update_status("Failed to parse Graph JSON data.".to_string());
                    }
                }
                Ok(response) => {
                    app.update_status(format!("Graph API Error: {}", response.status()));
                }
                Err(e) => {
                    app.update_status(format!("Network error: {}", e));
                    app.needs_reconnect = true;
                }
            }
        }
    }
}

pub fn toggle_imap_flag(session: &mut MailSession, emails: &mut [EmailMeta], selected_index: usize, flag_name: &str) {
    match session {
        MailSession::Imap(imap_sess) => {
            if emails.is_empty() { return; }

            let uid = emails[selected_index].uid.to_string();

            let is_set = match flag_name {
                "\\Flagged" => emails[selected_index].is_flagged,
                "\\Deleted" => emails[selected_index].is_deleted,
                "\\Seen"    => emails[selected_index].is_read,
                _ => false,
            };

            let op = if is_set {
                format!("-FLAGS.SILENT ({})", flag_name)
            } else {
                format!("+FLAGS.SILENT ({})", flag_name)
            };

            if imap_sess.uid_store(&uid, &op).is_ok() {
                let new_val = !is_set;
                match flag_name {
                    "\\Flagged" => emails[selected_index].is_flagged = new_val,
                    "\\Deleted" => emails[selected_index].is_deleted = new_val,
                    "\\Seen"    => emails[selected_index].is_read = new_val,
                    _ => {}
                }
            }
        },
        MailSession::Graph { access_token } => {
            if emails.is_empty() { return; }

            let id = &emails[selected_index].id;

            // Format the JSON body manually to avoid needing extra crate imports
            let (is_set, body_str) = match flag_name {
                "\\Seen" => {
                    let current = emails[selected_index].is_read;
                    (current, format!(r#"{{"isRead": {}}}"#, !current))
                },
                "\\Flagged" => {
                    let current = emails[selected_index].is_flagged;
                    let status = if !current { "flagged" } else { "notFlagged" };
                    (current, format!(r#"{{"flag": {{"flagStatus": "{}"}}}}"#, status))
                },
                _ => return, // \Deleted is handled locally for Outlook until expunge
            };

            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", id);
            let client = reqwest::blocking::Client::new();

            if client.patch(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .body(body_str)
                .send().is_ok() {

                let new_val = !is_set;
                match flag_name {
                    "\\Seen" => emails[selected_index].is_read = new_val,
                    "\\Flagged" => emails[selected_index].is_flagged = new_val,
                    _ => {}
                }
            }
        }
    }
}

pub fn expunge_deleted(session: &mut MailSession, app: &mut App) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            imap_sess.expunge().map_err(|e| format!("Expunge failed: {}", e))?;
            app.needs_fetch = true;
            Ok(())
        },
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            for email in &app.page_emails {
                if email.is_deleted {
                    let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", email.id);
                    let _ = client.delete(&url)
                        .header("Authorization", format!("Bearer {}", access_token))
                        .send();
                }
            }
            app.needs_fetch = true;
            Ok(())
        }
    }
}

pub fn fetch_email_body(session: &mut MailSession, fetch_seq: &str) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    match session {
        MailSession::Imap(imap_sess) => {
            let mut text_body = String::from("Could not load email body.");
            let mut html_body = None;
            let mut attachments = Vec::new();

            if let Ok(fetched_messages) = imap_sess.fetch(fetch_seq, "RFC822") {
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
        },
        MailSession::Graph { access_token } => {
            let mut text_body = String::from("Could not load email body.");
            let mut html_body = None;
            let mut attachments = Vec::new();

            // The /$value endpoint returns the raw MIME/RFC822 bytes of the email!
            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}/$value", fetch_seq);
            let client = reqwest::blocking::Client::new();

            if let Ok(res) = client.get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .send() {

                if res.status().is_success() {
                    if let Ok(bytes) = res.bytes() {
                        // Because it's raw RFC822, we can re-use your exact IMAP parser!
                        let (t, h, a) = parse_email_body(&bytes);
                        text_body = t;
                        html_body = h;
                        attachments = a;
                    }
                } else {
                    text_body = format!("Graph API Error: {}", res.status());
                }
            }

            (text_body, html_body, attachments)
        }
    }
}

pub fn move_to_folder(session: &mut MailSession, seq_id: &str, folder: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            imap_sess.copy(seq_id, folder)
                .map_err(|e| format!("Copy failed: {}", e))?;

            imap_sess.store(seq_id, "+FLAGS.SILENT (\\Deleted)")
                .map_err(|e| format!("Flagging failed: {}", e))?;

            Ok(())
        },
        MailSession::Graph { access_token } => {
            // Resolve standard folder names to Graph's known folder IDs
            let destination_id = match folder {
                "INBOX" => "inbox",
                "Junk" | "[Gmail]/Spam" => "junkemail",
                _ => folder,
            };

            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}/move", seq_id);
            let body_str = format!(r#"{{"destinationId": "{}"}}"#, destination_id);

            let client = reqwest::blocking::Client::new();
            client.post(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .body(body_str)
                .send()
                .map_err(|e| format!("Move failed: {}", e))?;

            Ok(())
        }
    }
}
