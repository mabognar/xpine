use crate::app::App;
use crate::mail::{EmailMeta, parse_email_body};
use std::thread;
use std::time::Duration;
use chrono::{DateTime, Local, Utc};
use native_tls::TlsConnector;
use imap::Session;
use std::net::TcpStream;
use crate::config::Account;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,

    // This tells Serde: "If you see verification_uri (Microsoft),
    // map it to verification_url so our code doesn't break."
    #[serde(alias = "verification_uri")]
    pub verification_url: String,

    pub expires_in: u64,
    pub interval: u64,
}

// Function to initiate the device flow
pub fn request_google_device_code(client_id: &str) -> Result<DeviceCodeResponse, String> {
    let client = reqwest::blocking::Client::new();

    // Google's device code endpoint
    let endpoint = "https://oauth2.googleapis.com/device/code";

    // For Google, we need full IMAP/SMTP access
    let params = vec![
        ("client_id", client_id),
        ("scope", "https://mail.google.com/"),
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

pub fn request_microsoft_device_code(client_id: &str) -> Result<DeviceCodeResponse, String> {
    let client = reqwest::blocking::Client::new();

    // Microsoft's device code endpoint (using 'common' to allow personal and work accounts)
    let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";

    // Microsoft requires offline_access to get a refresh token, plus specific IMAP/SMTP scopes
    let params = vec![
        ("client_id", client_id),
        ("scope", "offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send"),
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

#[derive(Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

// Internal struct to catch Google's specific polling errors
#[derive(Deserialize, Debug)]
struct PollingError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Polls Google until the user authorizes, denies, or the code expires
pub fn poll_google_token(
    client_id: &str,
    client_secret: &str, // Google Desktop App credentials usually require the secret here
    device_code: &str,
    base_interval: u64,
) -> Result<TokenResponse, String> {
    let client = reqwest::blocking::Client::new();
    let endpoint = "https://oauth2.googleapis.com/token";

    let mut current_interval = base_interval;

    loop {
        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let res = client.post(endpoint)
            .form(&params)
            .send()
            .map_err(|e| format!("Network error during polling: {}", e))?;

        if res.status().is_success() {
            // The user approved it!
            let token_res: TokenResponse = res.json()
                .map_err(|e| format!("Failed to parse token: {}", e))?;
            return Ok(token_res);
        } else {
            // Parse the error to see if we should keep waiting or give up
            let err_res: PollingError = res.json()
                .map_err(|e| format!("Failed to parse polling error: {}", e))?;

            match err_res.error.as_str() {
                "authorization_pending" => {
                    // Normal state, just wait and try again
                    thread::sleep(Duration::from_secs(current_interval));
                },
                "slow_down" => {
                    // Google wants us to back off a bit
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
        // let params = [
        //     ("client_id", client_id),
        //     ("client_secret", client_secret),
        //     ("device_code", device_code),
        //     ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        // ];
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
            // The user approved it!
            let token_res: TokenResponse = res.json()
                .map_err(|e| format!("Failed to parse token: {}", e))?;
            return Ok(token_res);
        } else {
            // Parse the error to see if we should keep waiting or give up
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


use crossterm::{
    cursor, execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::{self, stdout};

/// Initiates the flow, updates the UI, and returns the tokens upon success
pub fn run_google_auth_flow(client_id: &str, client_secret: &str) -> Result<TokenResponse, String> {
    // 1. Kick off the request to get the codes
    let auth_req = request_google_device_code(client_id)?;

    let mut stdout = stdout();

    // 2. Clear the screen and show the Alpine-style prompt
    let _ = execute!(
        stdout,
        Clear(ClearType::All),
        cursor::MoveTo(0, 2),
        SetForegroundColor(Color::Cyan),
        Print(" xpine - Google OAuth2 Device Authorization\r\n\r\n"),
        ResetColor,
        Print(format!("   To authorize this account, please visit: {}\r\n", auth_req.verification_url)),
        Print("   And enter the following code:            "),
        SetForegroundColor(Color::Yellow),
        Print(format!("{}\r\n\r\n", auth_req.user_code)),
        ResetColor,
        Print("   Waiting for authorization (check your browser)...\r\n")
    );

    // 3. Block and poll for the token (this loops until success or error)
    let token = poll_google_token(client_id, client_secret, &auth_req.device_code, auth_req.interval)?;

    // 4. Success feedback
    let _ = execute!(
        stdout,
        SetForegroundColor(Color::Green),
        Print("\r\n   Authorization successful! Returning to xpine...\r\n"),
        ResetColor
    );

    // Pause briefly so the user actually sees the success message before the screen redraws
    std::thread::sleep(std::time::Duration::from_millis(1500));

    Ok(token)
}

// pub fn run_microsoft_auth_flow(client_id: &str, client_secret: &str) -> Result<TokenResponse, String> {
//     // 1. Kick off the request to get the codes
//     let auth_req = request_microsoft_device_code(client_id)?;
//
//     let mut stdout = stdout();
//
//     // 2. Clear the screen and show the Alpine-style prompt
//     let _ = execute!(
//         stdout,
//         Clear(ClearType::All),
//         cursor::MoveTo(0, 2),
//         SetForegroundColor(Color::Cyan),
//         Print(" xpine - Microsoft OAuth2 Device Authorization\r\n\r\n"),
//         ResetColor,
//         Print(format!("   To authorize this account, please visit: {}\r\n", auth_req.verification_url)),
//         Print("   And enter the following code:            "),
//         SetForegroundColor(Color::Yellow),
//         Print(format!("{}\r\n\r\n", auth_req.user_code)),
//         ResetColor,
//         Print("   Waiting for authorization (check your browser)...\r\n")
//     );
//
//     // 3. Block and poll for the token (this loops until success or error)
//     let token = poll_google_token(client_id, client_secret, &auth_req.device_code, auth_req.interval)?;
//
//     // 4. Success feedback
//     let _ = execute!(
//         stdout,
//         SetForegroundColor(Color::Green),
//         Print("\r\n   Authorization successful! Returning to xpine...\r\n"),
//         ResetColor
//     );
//
//     // Pause briefly so the user actually sees the success message before the screen redraws
//     std::thread::sleep(std::time::Duration::from_millis(1500));
//
//     Ok(token)
// }

pub fn run_microsoft_auth_flow(client_id: &str, client_secret: &str) -> Result<TokenResponse, String> {
    // 1. Kick off the request to get the MS codes
    let auth_req = request_microsoft_device_code(client_id)?;

    let mut stdout = stdout();

    // 2. Clear the screen and show the prompt
    let _ = execute!(
        stdout,
        Clear(ClearType::All),
        cursor::MoveTo(0, 2),
        SetForegroundColor(Color::Cyan),
        Print(" xpine - Microsoft OAuth2 Device Authorization\r\n\r\n"),
        ResetColor,
        Print(format!("   To authorize this account, please visit: {}\r\n", auth_req.verification_url)),
        Print("   And enter the following code:            "),
        SetForegroundColor(Color::Yellow),
        Print(format!("{}\r\n\r\n", auth_req.user_code)),
        ResetColor,
        Print("   Waiting for authorization (check your browser)...\r\n")
    );

    // 3. Block and poll for the token using the MICROSOFT polling function!
    // THIS is where the unused function goes:
    let token = poll_microsoft_token(client_id, client_secret, &auth_req.device_code, auth_req.interval)?;

    // 4. Success feedback
    let _ = execute!(
        stdout,
        SetForegroundColor(Color::Green),
        Print("\r\n   Authorization successful! Returning to xpine...\r\n"),
        ResetColor
    );

    std::thread::sleep(std::time::Duration::from_millis(1500));

    Ok(token)
}

pub type ImapSession = Session<native_tls::TlsStream<TcpStream>>;

struct OAuth2 {
    user: String,
    access_token: String,
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

    // let mut params = vec![
    //     ("client_id", client_id),
    //     ("client_secret", client_secret),
    //     ("refresh_token", refresh_token),
    //     ("grant_type", "refresh_token"),
    // ];
    //
    // // If it's Microsoft, we apply the requested scope (or default to IMAP/SMTP)
    // if is_microsoft {
    //     let scope = target_scope.unwrap_or("https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send");
    //     params.push(("scope", scope));
    // }

    let mut params = vec![
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    // Only append the client secret if we actually have one (and aren't a public MS client)
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

pub fn connect(account: &mut Account) -> Result<ImapSession, imap::Error> {
    let domain = account.imap_server.as_str();
    let port = account.imap_port;
    let tls = TlsConnector::builder().build().unwrap();

    let mut client = imap::connect((domain, port), domain, &tls).unwrap();

    if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&account.client_id, &account.client_secret, &account.refresh_token) {

        let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

        // Pass 'None' here so we use the default IMAP scopes
        match get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft, None) {
            Ok(access_token) => {
                let auth = OAuth2 {
                    user: account.email.clone(),
                    access_token,
                };

                match client.authenticate("XOAUTH2", &auth) {
                    Ok(session) => return Ok(session),
                    Err((e, returned_client)) => {
                        std::fs::write("oauth_debug.txt", format!("IMAP REJECTED TOKEN: {:?}", e)).ok();
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