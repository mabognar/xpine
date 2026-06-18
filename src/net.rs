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
use std::net::{TcpListener};
use std::io::{Read, Write};

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
    // pub expires_in: u64,
    pub interval: u64,
}

#[derive(Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    // pub expires_in: u64,
    // pub token_type: String,
}

#[derive(Deserialize, Debug)]
struct PollingError {
    error: String,
    // #[serde(default)]
    // error_description: String,
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
    #[serde(rename = "singleValueExtendedProperties")]
    pub single_value_extended_properties: Option<Vec<GraphExtendedProperty>>,
}

#[derive(Deserialize, Debug)]
pub struct GraphExtendedProperty {
    pub id: String,
    pub value: String,
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

#[derive(Deserialize, Debug)]
pub struct GraphFolderResponse {
    pub value: Vec<GraphFolder>,
}

#[derive(Deserialize, Debug)]
pub struct GraphFolder {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}



pub fn request_microsoft_device_code(client_id: &str) -> Result<DeviceCodeResponse, String> {
    let client = reqwest::blocking::Client::new();
    let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";

    let params = vec![
        ("client_id", client_id),
        // Add Mail.Send to the requested scopes
        ("scope", "offline_access https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send"),
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
    _client_secret: &str,
    device_code: &str,
    base_interval: u64,
) -> Result<TokenResponse, String> {
    let client = reqwest::blocking::Client::new();
    let endpoint = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

    let mut current_interval = base_interval;

    loop {
        let params = vec![
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

    thread::sleep(Duration::from_millis(1500));

    Ok(token)
}

// pub fn run_generic_oauth_flow(email: &str) -> Result<(String, String, String), String> {
//     let email_lower = email.to_lowercase();
//     let redirect_uri = "http://127.0.0.1:8080";
//
//     // 1. Setup provider URLs based on the email domain
//     let (client_id, client_secret, auth_url, token_endpoint) = if email_lower.ends_with("@gmail.com") {
//         let cid = option_env!("XPINE_GOOGLE_CLIENT_ID").unwrap_or("YOUR_GOOGLE_CLIENT_ID").to_string();
//         let sec = option_env!("XPINE_GOOGLE_CLIENT_SECRET").unwrap_or("YOUR_GOOGLE_CLIENT_SECRET").to_string();
//         let url = format!(
//             "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=https://mail.google.com/&access_type=offline&prompt=consent",
//             cid, redirect_uri
//         );
//         (cid, sec, url, "https://oauth2.googleapis.com/token")
//
//     } else if email_lower.ends_with("@yahoo.com") {
//         let cid = option_env!("XPINE_YAHOO_CLIENT_ID").unwrap_or("YOUR_YAHOO_CLIENT_ID").to_string();
//         let sec = option_env!("XPINE_YAHOO_CLIENT_SECRET").unwrap_or("YOUR_YAHOO_CLIENT_SECRET").to_string();
//         let url = format!(
//             "https://api.login.yahoo.com/oauth2/request_auth?client_id={}&redirect_uri={}&response_type=code",
//             cid, redirect_uri
//         );
//         (cid, sec, url, "https://api.login.yahoo.com/oauth2/get_token")
//     } else {
//         return Err("OAuth is currently only implemented for Gmail and Yahoo accounts.".into());
//     };
//
//     // 2. Clear the screen and draw the explicit UI
//     let mut stdout = std::io::stdout();
//     let _ = execute!(
//         stdout,
//         Clear(ClearType::All),
//         cursor::MoveTo(0, 2),
//         SetForegroundColor(Color::Cyan),
//         Print(" xpine - OAuth2 Web Authorization\r\n\r\n"),
//         ResetColor,
//         Print("   Attempting to open your default web browser...\r\n\r\n"),
//         Print("   If the browser does NOT open automatically, please manually\r\n"),
//         Print("   Cmd+Click (or Ctrl+Click) the link below to authorize:\r\n\r\n"),
//         SetForegroundColor(Color::Yellow),
//         Print(format!("   {}\r\n\r\n", auth_url)),
//         ResetColor,
//         Print(format!("   Waiting for authorization code on {}...\r\n", redirect_uri))
//     );
//
//     // 3. Attempt to open the browser automatically
//     let _ = crate::browser::open_url(&auth_url);
//
//     // 4. Start the local server
//     let listener = std::net::TcpListener::bind("127.0.0.1:8080").map_err(|e| format!("Failed to bind to port 8080: {}", e))?;
//     let mut auth_code = String::new();
//
//     for stream in listener.incoming() {
//         match stream {
//             Ok(mut stream) => {
//                 let mut buffer = [0; 2048];
//                 if let Ok(bytes_read) = std::io::Read::read(&mut stream, &mut buffer) {
//                     let request = String::from_utf8_lossy(&buffer[..bytes_read]);
//
//                     if request.starts_with("GET") {
//                         if let Some(code_start) = request.find("code=") {
//                             let code_part = &request[code_start + 5..];
//                             if let Some(code_end) = code_part.find('&').or_else(|| code_part.find(' ')) {
//                                 auth_code = code_part[..code_end].to_string();
//
//                                 let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style=\"font-family: sans-serif; text-align: center; margin-top: 50px;\"><h2>Authentication successful!</h2><p>You can close this window and return to the terminal.</p></body></html>";
//                                 let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
//                                 break;
//                             }
//                         }
//                     }
//                 }
//                 let error_response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Authentication failed!</h2><p>No authorization code found in the request.</p></body></html>";
//                 let _ = std::io::Write::write_all(&mut stream, error_response.as_bytes());
//                 break;
//             }
//             Err(e) => {
//                 return Err(format!("Local server error: {}", e));
//             }
//         }
//     }
//
//     if auth_code.is_empty() {
//         return Err("Failed to retrieve the authorization code from the browser.".into());
//     }
//
//     let _ = execute!(
//         stdout,
//         SetForegroundColor(Color::Green),
//         Print("\r\n   Code received! Exchanging for tokens...\r\n"),
//         ResetColor
//     );
//
//     // 5. Exchange code for permanent token
//     let client = reqwest::blocking::Client::new();
//     let params = vec![
//         ("client_id", client_id.as_str()),
//         ("client_secret", client_secret.as_str()),
//         ("code", auth_code.as_str()),
//         ("grant_type", "authorization_code"),
//         ("redirect_uri", redirect_uri),
//     ];
//
//     let res = client.post(token_endpoint)
//         .form(&params)
//         .send()
//         .map_err(|e| format!("Network error during token exchange: {}", e))?;
//
//     if res.status().is_success() {
//         let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
//         if let Some(refresh) = token_res.refresh_token {
//             Ok((client_id, client_secret, refresh))
//         } else {
//             Err("Provider did not return a refresh token.".into())
//         }
//     } else {
//         let err_text = res.text().unwrap_or_default();
//         Err(format!("Token exchange failed: {}", err_text))
//     }
// }

pub fn run_generic_oauth_flow(email: &str) -> Result<(String, String, String), String> {
    let email_lower = email.to_lowercase();

    // For this phase, we are strictly targeting Gmail
    if !email_lower.ends_with("@gmail.com") {
        return Err("OAuth is currently only implemented for Gmail/Google accounts.".into());
    }

    // IMPORTANT: You will need to replace these with credentials from the Google Cloud Console
    let client_id = option_env!("XPINE_GOOGLE_CLIENT_ID")
        .unwrap_or("YOUR_GOOGLE_CLIENT_ID")
        .to_string();
    let client_secret = option_env!("XPINE_GOOGLE_CLIENT_SECRET")
        .unwrap_or("YOUR_GOOGLE_CLIENT_SECRET")
        .to_string();
    let redirect_uri = "http://127.0.0.1:8080";

    // 1. Construct the Google Authorization URL
    // We request 'offline' access and force 'consent' to guarantee Google gives us a refresh token
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=https://mail.google.com/&access_type=offline&prompt=consent",
        client_id, redirect_uri
    );

    println!("   Opening default web browser for OAuth authentication...");
    println!("   If the browser does not open automatically, manually visit:\r\n\r\n{}\r\n", auth_url);

    // Attempt to open the browser automatically using your existing browser module
    let _ = crate::browser::open_url(&auth_url);

    // 2. Start the local server to catch the browser redirect
    println!("   Listening for authorization code on {}...", redirect_uri);
    let listener = TcpListener::bind("127.0.0.1:8080").map_err(|e| format!("Failed to bind to port 8080: {}", e))?;

    let mut auth_code = String::new();

    // Block and wait for a single connection (the browser redirecting back to us)
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let mut buffer = [0; 2048];
                if let Ok(bytes_read) = stream.read(&mut buffer) {
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

                    // Parse the raw HTTP GET request to extract the ?code=... parameter
                    if request.starts_with("GET") {
                        if let Some(code_start) = request.find("code=") {
                            let code_part = &request[code_start + 5..];
                            // The code ends at the next '&' or at the space before HTTP/1.1
                            if let Some(code_end) = code_part.find('&').or_else(|| code_part.find(' ')) {
                                auth_code = code_part[..code_end].to_string();

                                // Send a nice HTML success message back to the user's browser
                                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style=\"font-family: sans-serif; text-align: center; margin-top: 50px;\"><h2>Authentication successful!</h2><p>You can close this window and return to xpine.</p></body></html>";
                                let _ = stream.write_all(response.as_bytes());
                                break;
                            }
                        }
                    }
                }
                // Send an error message if the code was missing or malformed
                let error_response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Authentication failed!</h2><p>No authorization code found in the request.</p></body></html>";
                let _ = stream.write_all(error_response.as_bytes());
                break;
            }
            Err(e) => {
                return Err(format!("Local server error: {}", e));
            }
        }
    }

    if auth_code.is_empty() {
        return Err("Failed to retrieve the authorization code from the browser.".into());
    }

    println!("   Code received! Exchanging for tokens...");

    // 3. Exchange the short-lived code for a permanent refresh token
    let client = reqwest::blocking::Client::new();
    let params = vec![
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code", auth_code.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];

    let res = client.post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .map_err(|e| format!("Network error during token exchange: {}", e))?;

    if res.status().is_success() {
        let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;

        // Google only issues a refresh token on the very first authorization.
        // We forced `prompt=consent` above to ensure it is always provided.
        if let Some(refresh) = token_res.refresh_token {
            Ok((client_id, client_secret, refresh))
        } else {
            Err("Google did not return a refresh token. You may need to revoke xpine's access in your Google account settings and try again.".into())
        }
    } else {
        let err_text = res.text().unwrap_or_default();
        Err(format!("Token exchange failed: {}", err_text))
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
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let is_microsoft = account.email.ends_with("@outlook.com") || account.email.ends_with("@hotmail.com");

    // 1. Try OAuth Flow First
    if let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&account.client_id, &account.client_secret, &account.refresh_token) {

        let scope = if is_microsoft { Some("https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send offline_access") } else { None };

        match get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft, scope) {
            Ok(access_token) => {
                if is_microsoft {
                    // THE FLIP: Bypass IMAP entirely and return a stateless Graph session!
                    return Ok(MailSession::Graph { access_token });
                } else {
                    // Standard IMAP XOAUTH2 for Gmail/Yahoo
                    let domain = account.imap_server.as_str();
                    let port = account.imap_port;

                    // -- 15 SECOND TIMEOUT LOGIC --
                    let addr_str = format!("{}:{}", domain, port);
                    let addr = addr_str.to_socket_addrs().map_err(|e| e.to_string())?
                        .next().ok_or("Could not resolve IMAP server address")?;

                    let timeout = Duration::from_secs(15);
                    let tcp_stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
                    tcp_stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
                    tcp_stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;

                    let tls = TlsConnector::builder().build().map_err(|e| e.to_string())?;
                    let tls_stream = tls.connect(domain, tcp_stream).map_err(|e| e.to_string())?;

                    let client = imap::Client::new(tls_stream);
                    // -----------------------------

                    let auth = OAuth2 {
                        user: account.email.clone(),
                        access_token,
                    };

                    match client.authenticate("XOAUTH2", &auth) {
                        Ok(session) => return Ok(MailSession::Imap(session)),
                        Err((e, _returned_client)) => {
                            std::fs::write("oauth_debug.txt", format!("IMAP REJECTED TOKEN: {:?}", e)).ok();
                            // It will naturally fall through to the password fallback below
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

    // -- 15 SECOND TIMEOUT LOGIC --
    let addr_str = format!("{}:{}", domain, port);
    let addr = addr_str.to_socket_addrs().map_err(|e| e.to_string())?
        .next().ok_or("Could not resolve IMAP server address")?;

    let timeout = Duration::from_secs(15);
    let tcp_stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    tcp_stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    tcp_stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;

    let tls = TlsConnector::builder().build().map_err(|e| e.to_string())?;
    let tls_stream = tls.connect(domain, tcp_stream).map_err(|e| e.to_string())?;

    let client = imap::Client::new(tls_stream);
    // -----------------------------

    if let Some(password) = &account.password {
        let session = client.login(&account.email, password).map_err(|(e, _)| e.to_string())?;
        Ok(MailSession::Imap(session))
    } else {
        Err("No password or valid OAuth credentials provided".to_string())
    }
}

fn decode_mime_string(raw: &[u8]) -> String {
    let mut header_bytes = b"X-Dummy: ".to_vec();
    header_bytes.extend_from_slice(raw);
    header_bytes.extend_from_slice(b"\r\n");

    // FIX: The tuple order is (MailHeader, usize)
    if let Ok((header, _)) = mailparse::parse_header(&header_bytes) {
        header.get_value()
    } else {
        String::from_utf8_lossy(raw).into_owned()
    }
}

pub fn fetch_emails(session: &mut MailSession, app: &mut App, items_per_page: u32, sort_newest_first: bool) {
    match session {
        MailSession::Imap(imap_sess) => {
            app.page_emails.clear();

            match imap_sess.select(&app.current_folder) {
                Ok(m) => app.total_messages = m.exists,
                Err(_) => { app.needs_reconnect = true; return; }
            }

            // let mut overlap_shift = 0; // <-- 1. ADD THIS VARIABLE

            let sequence = if let Some(ref q) = app.search_query {
                let query = if q.trim() == "*" {
                    String::from("FLAGGED")
                } else if q.trim().eq_ignore_ascii_case("n") {
                    String::from("UNSEEN")
                } else {
                    format!("OR FROM \"{}\" SUBJECT \"{}\"", q, q)
                };

                match imap_sess.search(&query) {
                    Ok(seq_ids) if !seq_ids.is_empty() => {
                        app.total_messages = seq_ids.len() as u32;

                        let mut sorted_seqs: Vec<u32> = seq_ids.into_iter().collect();
                        sorted_seqs.sort();

                        let mut end_idx = sorted_seqs.len().saturating_sub((app.current_page * items_per_page) as usize);
                        let start_idx = end_idx.saturating_sub(items_per_page as usize - 1).max(1);

                        if start_idx == 1 {
                            // let original_end = end_idx; // <-- 2. TRACK SHIFT
                            end_idx = (items_per_page as usize).min(sorted_seqs.len());
                            // overlap_shift = (end_idx.saturating_sub(original_end)) as u32;
                        }

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
                    let mut end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
                    let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);

                    if start_idx == 1 {
                        // let original_end = end_idx; // <-- 3. TRACK SHIFT
                        end_idx = items_per_page.min(app.total_messages);
                        // overlap_shift = end_idx.saturating_sub(original_end);
                    }

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
                        let mut is_flagged = false;
                        let mut is_answered = false;

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
                            // Update the Subject to use our decoder
                            if let Some(s) = env.subject.as_ref() {
                                subject = decode_mime_string(s);
                            }

                            if let Some(d) = env.date.as_ref() {
                                let raw_date = String::from_utf8_lossy(d).into_owned();
                                if let Ok(dt) = DateTime::parse_from_rfc2822(&raw_date) {
                                    let now = Utc::now().timestamp();
                                    let diff = now - dt.timestamp();
                                    let local_dt = dt.with_timezone(&Local);
                                    date = if diff < 7 * 24 * 3600 && diff >= -86400 { local_dt.format("%a %H:%M").to_string() } else { local_dt.format("%b %d").to_string() };
                                } else if let Some(dt) = message.internal_date() {
                                    // Fallback to the server's guaranteed internal receive time
                                    let now = Utc::now().timestamp();
                                    let diff = now - dt.timestamp();
                                    let local_dt = dt.with_timezone(&Local);
                                    date = if diff < 7 * 24 * 3600 && diff >= -86400 { local_dt.format("%a %H:%M").to_string() } else { local_dt.format("%b %d").to_string() };
                                } else {
                                    // Extreme fallback: truncate to exactly 6 characters (e.g., "Apr 02") to preserve UI column width
                                    let mut s = raw_date.split(" +").next().unwrap_or(&raw_date).to_string();
                                    s.truncate(6);
                                    date = s;
                                }
                            } else if let Some(dt) = message.internal_date() {
                                // Fallback if the Date header is completely missing
                                let now = Utc::now().timestamp();
                                let diff = now - dt.timestamp();
                                let local_dt = dt.with_timezone(&Local);
                                date = if diff < 7 * 24 * 3600 && diff >= -86400 { local_dt.format("%a %H:%M").to_string() } else { local_dt.format("%b %d").to_string() };
                            }

                            macro_rules! format_addrs {
                                ($addrs:expr) => {{
                                    let mut result = Vec::new();
                                    if let Some(a_vec) = $addrs {
                                        for addr in a_vec {
                                            // Update the name to use our decoder
                                            let name = addr.name.as_ref().map(|n| decode_mime_string(n.as_ref())).unwrap_or_default();
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
                            id: message.message.to_string(), // IMAP integer to String
                            uid: message.uid.unwrap_or(0),
                            timestamp: internal_date,
                            subject,
                            from,
                            reply_to,
                            // reply_to_display: String::new(),
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

                let last_page = app.total_messages.saturating_sub(1) / items_per_page;
                let overlap = if app.total_messages >= items_per_page {
                    (last_page * items_per_page).saturating_sub(app.total_messages.saturating_sub(items_per_page))
                } else {
                    0
                } as usize;

                if let Some(idx_from_end) = app.restore_index_from_end {
                    if sort_newest_first {
                        app.selected_index = if !app.page_emails.is_empty() { idx_from_end as usize } else { 0 };
                    } else {
                        app.selected_index = if !app.page_emails.is_empty() { app.page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize) } else { 0 };
                    }
                    app.restore_index_from_end = None;
                } else {
                    let max_idx = (items_per_page as usize).saturating_sub(1);

                    if overlap > 0 {
                        if sort_newest_first {
                            if app.current_page == last_page && app.selected_index == 0 {
                                app.selected_index = overlap;
                            } else if app.current_page == last_page.saturating_sub(1) && app.selected_index == max_idx {
                                app.selected_index = max_idx.saturating_sub(overlap);
                            }
                        } else {
                            if app.current_page == last_page && app.selected_index == max_idx {
                                app.selected_index = max_idx.saturating_sub(overlap);
                            } else if app.current_page == last_page.saturating_sub(1) && app.selected_index == 0 {
                                app.selected_index = overlap;
                            }
                        }
                    }

                    if app.selected_index >= app.page_emails.len() {
                        app.selected_index = app.page_emails.len().saturating_sub(1);
                    }
                }
            }
        },

        MailSession::Graph { access_token } => {
            app.page_emails.clear();

            // let folder = if app.current_folder == "INBOX" { "inbox" } else { &app.current_folder };

            let folder_id = match app.current_folder.as_str() {
                "INBOX" => "inbox".to_string(),
                "Sent Items" => "sentitems".to_string(),
                "Deleted Items" => "deleteditems".to_string(),
                "Drafts" => "drafts".to_string(),
                "Junk Email" => "junkemail".to_string(),
                "Archive" => "archive".to_string(),
                other => {
                    let mut id = other.to_string();
                    let client = reqwest::blocking::Client::new();
                    let encoded = urlencoding::encode(other);
                    let lookup_url = format!("https://graph.microsoft.com/v1.0/me/mailFolders?$filter=displayName%20eq%20'{}'", encoded);

                    if let Ok(res) = client.get(&lookup_url).header("Authorization", format!("Bearer {}", access_token)).send() {
                        if let Ok(data) = res.json::<GraphFolderResponse>() {
                            if let Some(f) = data.value.into_iter().next() {
                                id = f.id;
                            }
                        }
                    }
                    id
                }
            };

            let folder = &folder_id;

            let mut skip = app.current_page * items_per_page;

            // NEW: Shift the skip offset back to populate the page fully if there are enough messages
            if app.current_page > 0 && skip + items_per_page > app.total_messages && app.total_messages >= items_per_page {
                skip = app.total_messages - items_per_page;
            }

            let mut is_text_search = false;

            let url = if let Some(ref q) = app.search_query {
                if q.trim() == "*" {
                    // FIX: Always fetch newest first from the server
                    let order = "receivedDateTime%20DESC";
                    format!(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$count=true&$top={}&$skip={}&$orderby={}&$filter=flag/flagStatus%20eq%20'flagged'&$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x1081'%20or%20id%20eq%20'Long%200x0E08')",
                        folder, items_per_page, skip, order
                    )
                } else if q.trim().eq_ignore_ascii_case("n") { // Intercept "N" or "n"
                    // FIX: Always fetch newest first from the server
                    let order = "receivedDateTime%20DESC";
                    format!(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$count=true&$top={}&$skip={}&$orderby={}&$filter=isRead%20eq%20false&$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x1081'%20or%20id%20eq%20'Long%200x0E08')",
                        folder, items_per_page, skip, order
                    )
                } else {
                    is_text_search = true;
                    let search_str = format!("\"{}\"", q);
                    let encoded_q = urlencoding::encode(&search_str);

                    format!(
                        "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$top={}&$skip={}&$search={}&$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x1081'%20or%20id%20eq%20'Long%200x0E08')",
                        folder, items_per_page, skip, encoded_q
                    )
                }
            } else {
                // FIX: Always fetch newest first from the server
                let order = "receivedDateTime%20DESC";
                format!(
                    "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?$count=true&$top={}&$skip={}&$orderby={}&$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x1081'%20or%20id%20eq%20'Long%200x0E08')",
                    folder, items_per_page, skip, order
                )
            };

            let client = reqwest::blocking::Client::new();
            let res = client.get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("ConsistencyLevel", "eventual")
                .send();

            match res {
                Ok(response) if response.status().is_success() => {
                    if let Ok(graph_data) = response.json::<GraphMessageResponse>() {

                        let messages_iter: Box<dyn Iterator<Item = GraphMessage>> = if is_text_search {
                            app.total_messages = graph_data.value.len() as u32;
                            let mut all_msgs = graph_data.value;

                            if sort_newest_first {
                                all_msgs.sort_by(|a, b| b.received_date_time.cmp(&a.received_date_time));
                            } else {
                                all_msgs.sort_by(|a, b| a.received_date_time.cmp(&b.received_date_time));
                            }

                            let start = (app.current_page * items_per_page) as usize;
                            Box::new(all_msgs.into_iter().skip(start).take(items_per_page as usize))
                        } else {
                            if let Some(total) = graph_data.count {
                                app.total_messages = total;
                            }
                            Box::new(graph_data.value.into_iter())
                        };

                        for msg in messages_iter {
                            // --- PARSING LOGIC START ---
                            let mut is_answered = false;
                            let mut msg_size = 0; // Initialize size to 0

                            if let Some(props) = &msg.single_value_extended_properties {
                                for prop in props {
                                    // 102 is the MAPI code for "Replied"
                                    if prop.id == "Integer 0x1081" && prop.value == "102" {
                                        is_answered = true;
                                    }
                                    // Make the check robust against 'Long' or 'Integer' and missing leading zeros
                                    else if prop.id.to_lowercase().contains("0x0e08") || prop.id.to_lowercase().contains("0xe08") {
                                        if let Ok(parsed_size) = prop.value.parse::<u32>() {
                                            msg_size = parsed_size;
                                        }
                                    }
                                }
                            }

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
                                id: msg.id,
                                uid: 0,
                                timestamp: internal_date,
                                subject: msg.subject.unwrap_or_else(|| "No Subject".to_string()),
                                from,
                                reply_to,
                                to_addr,
                                cc,
                                date: date_str,
                                size: msg_size, // Successfully parsed MAPI property!
                                is_read: msg.is_read,
                                is_deleted: false,
                                is_flagged,
                                is_answered,
                            });
                        }

                        if sort_newest_first {
                            app.page_emails.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        } else {
                            app.page_emails.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                        }

                        // --- UNIVERSAL SORTING AND PAGINATION CURSOR SYNC ---
                        if sort_newest_first {
                            app.page_emails.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                        } else {
                            app.page_emails.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                        }

                        let last_page = app.total_messages.saturating_sub(1) / items_per_page;
                        let overlap = if app.total_messages >= items_per_page {
                            (last_page * items_per_page).saturating_sub(app.total_messages.saturating_sub(items_per_page))
                        } else {
                            0
                        } as usize;

                        if let Some(idx_from_end) = app.restore_index_from_end {
                            if sort_newest_first {
                                app.selected_index = if !app.page_emails.is_empty() { idx_from_end as usize } else { 0 };
                            } else {
                                app.selected_index = if !app.page_emails.is_empty() { app.page_emails.len().saturating_sub(1).saturating_sub(idx_from_end as usize) } else { 0 };
                            }
                            app.restore_index_from_end = None;
                        } else {
                            let max_idx = (items_per_page as usize).saturating_sub(1);

                            if overlap > 0 {
                                if sort_newest_first {
                                    if app.current_page == last_page && app.selected_index == 0 {
                                        app.selected_index = overlap;
                                    } else if app.current_page == last_page.saturating_sub(1) && app.selected_index == max_idx {
                                        app.selected_index = max_idx.saturating_sub(overlap);
                                    }
                                } else {
                                    if app.current_page == last_page && app.selected_index == max_idx {
                                        app.selected_index = max_idx.saturating_sub(overlap);
                                    } else if app.current_page == last_page.saturating_sub(1) && app.selected_index == 0 {
                                        app.selected_index = overlap;
                                    }
                                }
                            }

                            if app.selected_index >= app.page_emails.len() {
                                app.selected_index = app.page_emails.len().saturating_sub(1);
                            }
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

pub fn toggle_flag(session: &mut MailSession, emails: &mut [EmailMeta], selected_index: usize, flag_name: &str) {
    if emails.is_empty() { return; }

    match session {
        MailSession::Imap(imap_sess) => {
            let uid = emails[selected_index].uid.to_string();
            let is_set = match flag_name {
                "\\Flagged" => emails[selected_index].is_flagged,
                "\\Deleted" => emails[selected_index].is_deleted,
                "\\Seen"    => emails[selected_index].is_read,
                "\\Answered"=> emails[selected_index].is_answered,
                _ => false,
            };
            let op = if is_set { format!("-FLAGS.SILENT ({})", flag_name) } else { format!("+FLAGS.SILENT ({})", flag_name) };
            if imap_sess.uid_store(&uid, &op).is_ok() {
                let new_val = !is_set;
                match flag_name {
                    "\\Flagged" => emails[selected_index].is_flagged = new_val,
                    "\\Deleted" => emails[selected_index].is_deleted = new_val,
                    "\\Seen"    => emails[selected_index].is_read = new_val,
                    "\\Answered"=> emails[selected_index].is_answered = new_val,
                    _ => {}
                }
            }
        },
        MailSession::Graph { access_token } => {
            let id = &emails[selected_index].id;
            let (requires_network, is_set, body_str) = match flag_name {
                "\\Seen" => {
                    let current = emails[selected_index].is_read;
                    (true, current, format!(r#"{{"isRead": {}}}"#, !current))
                },
                "\\Flagged" => {
                    let current = emails[selected_index].is_flagged;
                    let status = if !current { "flagged" } else { "notFlagged" };
                    (true, current, format!(r#"{{"flag": {{"flagStatus": "{}"}}}}"#, status))
                },
                "\\Answered" => {
                    let current = emails[selected_index].is_answered;
                    // Setting 102 (Replied) to indicate answered
                    let val = if !current { "102" } else { "0" };
                    (true, current, format!(r#"{{"singleValueExtendedProperties": [{{"id": "Integer 0x1081", "value": "{}"}}]}}"#, val))
                },
                _ => (false, false, String::new()),
            };

            if requires_network {
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
                        "\\Answered" => emails[selected_index].is_answered = new_val,
                        _ => {}
                    }
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

pub fn create_folder(session: &mut MailSession, folder_name: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            // IMAP standard CREATE command
            imap_sess.create(folder_name).map_err(|e| format!("Failed to create folder: {}", e))?;
            Ok(())
        }
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            // Graph API expects a simple JSON body with the displayName
            let body = serde_json::json!({
                "displayName": folder_name
            });

            let res = client.post("https://graph.microsoft.com/v1.0/me/mailFolders")
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| e.to_string())?;

            if res.status().is_success() {
                Ok(())
            } else {
                Err(format!("Graph API error: {}", res.status()))
            }
        }
    }
}


pub fn delete_folder(session: &mut MailSession, folder_name: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            // IMAP standard DELETE command
            imap_sess.delete(folder_name).map_err(|e| format!("Failed to delete folder: {}", e))?;
            Ok(())
        }
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            // STEP 1: Ask Microsoft to find the folder by its name so we can get its ID
            let filter_query = format!("displayName eq '{}'", folder_name);
            let res = client.get("https://graph.microsoft.com/v1.0/me/mailFolders")
                .query(&[("$filter", filter_query)]) // .query() safely URL-encodes spaces!
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .map_err(|e| e.to_string())?;

            if !res.status().is_success() {
                return Err(format!("Graph API error: {}", res.status()));
            }

            let json: serde_json::Value = res.json().map_err(|e| e.to_string())?;

            // Drill down into the JSON response to grab the first matching ID
            let folder_id = json.get("value")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|f| f.get("id"))
                .and_then(|id| id.as_str())
                .ok_or("Folder not found on Microsoft servers")?;

            // STEP 2: Issue the actual DELETE request using the ID
            let delete_url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/{}", folder_id);
            let del_res = client.delete(&delete_url)
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .map_err(|e| e.to_string())?;

            if del_res.status().is_success() {
                Ok(())
            } else {
                Err(format!("Graph API deletion error: {}", del_res.status()))
            }
        }
    }
}

pub fn list_folders(session: &mut MailSession) -> Result<Vec<String>, String> {
    match session {
        MailSession::Imap(imap_sess) => {
            // Ask the IMAP server for all folders ("*" wildcard)
            let mailboxes = imap_sess.list(Some(""), Some("*"))
                .map_err(|e| format!("Failed to fetch folders: {}", e))?;

            let mut folders = Vec::new();
            for mbox in mailboxes.iter() {
                folders.push(mbox.name().to_string());
            }

            // folders.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            Ok(folders)
        }
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            // Get up to 250 folders
            let res = client.get("https://graph.microsoft.com/v1.0/me/mailFolders?$top=250")
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .map_err(|e| e.to_string())?;

            if !res.status().is_success() {
                return Err(format!("Graph API error: {}", res.status()));
            }

            let json: serde_json::Value = res.json().map_err(|e| e.to_string())?;
            let mut folders = Vec::new();

            // Parse the JSON array and extract all the displayNames
            if let Some(values) = json.get("value").and_then(|v| v.as_array()) {
                for v in values {
                    if let Some(name) = v.get("displayName").and_then(|n| n.as_str()) {
                        folders.push(name.to_string());
                    }
                }
            }

            Ok(folders)
        }
    }
}
pub fn rename_folder(session: &mut MailSession, old_name: &str, new_name: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            imap_sess.rename(old_name, new_name)
                .map_err(|e| format!("Failed to rename folder: {}", e))?;
            Ok(())
        }
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            // STEP 1: Find the folder ID by its old name
            let filter_query = format!("displayName eq '{}'", old_name);
            let res = client.get("https://graph.microsoft.com/v1.0/me/mailFolders")
                .query(&[("$filter", filter_query)])
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .map_err(|e| e.to_string())?;

            if !res.status().is_success() {
                return Err(format!("Graph API error finding folder: {}", res.status()));
            }

            let json: serde_json::Value = res.json().map_err(|e| e.to_string())?;

            let folder_id = json.get("value")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|f| f.get("id"))
                .and_then(|id| id.as_str())
                .ok_or("Folder not found on Microsoft servers")?;

            // STEP 2: Issue a PATCH request to change the displayName
            let patch_url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/{}", folder_id);
            let body = serde_json::json!({
                "displayName": new_name
            });

            let patch_res = client.patch(&patch_url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| e.to_string())?;

            if patch_res.status().is_success() {
                Ok(())
            } else {
                Err(format!("Graph API rename error: {}", patch_res.status()))
            }
        }
    }
}

pub fn move_email(session: &mut MailSession, message_seq: &str, dest_folder: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            // Step 1: Copy to destination
            imap_sess.copy(message_seq, dest_folder)
                .map_err(|e| format!("Failed to copy email: {}", e))?;

            // Step 2: Flag the original as deleted
            imap_sess.store(message_seq, "+FLAGS (\\Deleted)")
                .map_err(|e| format!("Failed to delete original: {}", e))?;

            // Step 3: Expunge the current folder to finalize the move
            imap_sess.expunge()
                .map_err(|e| format!("Failed to expunge: {}", e))?;

            Ok(())
        }
        MailSession::Graph { access_token } => {
            let client = reqwest::blocking::Client::new();

            // STEP 1: Find the Destination Folder ID
            let filter_query = format!("displayName eq '{}'", dest_folder);
            let res = client.get("https://graph.microsoft.com/v1.0/me/mailFolders")
                .query(&[("$filter", filter_query)])
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .map_err(|e| e.to_string())?;

            let json: serde_json::Value = res.json().map_err(|e| e.to_string())?;
            let dest_id = json.get("value")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|f| f.get("id"))
                .and_then(|id| id.as_str())
                .ok_or("Destination folder not found on Microsoft servers")?;

            // STEP 2: POST to the /move endpoint
            // Note: message_seq here must be the Graph Message ID, not an IMAP sequence number
            let move_url = format!("https://graph.microsoft.com/v1.0/me/messages/{}/move", message_seq);
            let body = serde_json::json!({
                "destinationId": dest_id
            });

            let move_res = client.post(&move_url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| e.to_string())?;

            if move_res.status().is_success() {
                Ok(())
            } else {
                Err(format!("Graph API move error: {}", move_res.status()))
            }
        }
    }
}

pub fn reconnect(app: &mut App, session: &mut Option<MailSession>) {
    if !app.accounts.is_empty() {
        app.active_account = app.accounts[app.current_account_idx].clone();

        if let Some(s) = session.take() {
            match s {
                MailSession::Imap(mut imap_sess) => { let _ = imap_sess.logout(); }
                MailSession::Graph { .. } => {}
            }
        }

        match connect(&mut app.active_account) {
            Ok(sess) => {
                *session = Some(sess);
                app.needs_fetch = true;
            }
            Err(e) => {
                *session = None;
                let err_str = e.to_lowercase();
                if err_str.contains("timeout") || err_str.contains("timed out") || err_str.contains("would block") {
                    app.update_status("Attempted connection timed out".to_string());
                } else {
                    app.update_status("Connection failed".to_string());
                }
            }
        }
    }
    app.needs_reconnect = false;
    app.last_fetch_time = std::time::Instant::now();
}
