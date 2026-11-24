use std::collections::HashMap;
use std::io::{stdout, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Local, Utc};
use crossterm::{
    cursor, execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use imap::Session;
use native_tls::TlsConnector;
use serde::Deserialize;

use crate::app::{App, CachedAccountView};
use crate::config::Account;
use crate::mail::{parse_email_body, EmailMeta};

const NETWORK_TIMEOUT: Duration = Duration::from_secs(15);
const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0/me";
const GRAPH_EXTENDED_PROPS_EXPAND: &str =
    "$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x1081'%20or%20id%20eq%20'Long%200x0E08')";
const MS_OAUTH_SCOPE: &str =
    "offline_access https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send";
const MS_DEVICE_CODE_ENDPOINT: &str =
    "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
const MS_TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

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
    pub interval: u64,
}

#[derive(Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Deserialize, Debug)]
struct PollingError {
    error: String,
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
    #[serde(rename = "hasAttachments")]
    pub has_attachments: Option<bool>,
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

impl imap::Authenticator for OAuth2 {
    type Response = String;

    #[allow(unused_variables)]
    fn process(&self, data: &[u8]) -> Self::Response {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.access_token)
    }
}

pub fn http_client() -> &'static reqwest::blocking::Client {
    static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(NETWORK_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

pub fn request_microsoft_device_code(client_id: &str) -> Result<DeviceCodeResponse, String> {
    let params = [("client_id", client_id), ("scope", MS_OAUTH_SCOPE)];

    let res = http_client()
        .post(MS_DEVICE_CODE_ENDPOINT)
        .form(&params)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if res.status().is_success() {
        res.json().map_err(|e| format!("Parse error: {}", e))
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
    let client = http_client();
    let mut current_interval = base_interval;
    let params = [
        ("client_id", client_id),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];

    loop {
        let res = client
            .post(MS_TOKEN_ENDPOINT)
            .form(&params)
            .send()
            .map_err(|e| format!("Network error during polling: {}", e))?;

        if res.status().is_success() {
            return res
                .json()
                .map_err(|e| format!("Failed to parse token: {}", e));
        }

        let err_res: PollingError = res
            .json()
            .map_err(|e| format!("Failed to parse polling error: {}", e))?;

        match err_res.error.as_str() {
            "authorization_pending" => {
                thread::sleep(Duration::from_secs(current_interval));
            }
            "slow_down" => {
                current_interval += 2;
                thread::sleep(Duration::from_secs(current_interval));
            }
            "access_denied" => return Err("Authorization denied by the user.".to_string()),
            "expired_token" => {
                return Err("The device code expired. Please try again.".to_string())
            }
            _ => return Err(format!("Unexpected OAuth error: {}", err_res.error)),
        }
    }
}

pub fn run_microsoft_auth_flow(
    client_id: &str,
    client_secret: &str,
) -> Result<TokenResponse, String> {
    let auth_req = request_microsoft_device_code(client_id)?;

    let mut stdout = stdout();
    let _ = execute!(
        stdout,
        Clear(ClearType::All),
        cursor::MoveTo(0, 2),
        Print(" xpine - Microsoft OAuth2 Device Authorization\r\n\r\n"),
        ResetColor,
        Print(format!(
            "   To authorize this account, please visit: {}\r\n",
            auth_req.verification_url
        )),
        Print("   And enter the following code:            "),
        SetForegroundColor(Color::Red),
        Print(format!("{}\r\n\r\n", auth_req.user_code)),
        ResetColor,
        Print("   Waiting for authorization (check your browser)...\r\n")
    );

    let token = poll_microsoft_token(
        client_id,
        client_secret,
        &auth_req.device_code,
        auth_req.interval,
    )?;

    let _ = execute!(
        stdout,
        SetForegroundColor(Color::Green),
        Print("\r\n   Authorization successful! Returning to xpine...\r\n"),
        ResetColor
    );

    thread::sleep(Duration::from_millis(1500));

    Ok(token)
}

pub fn run_gmail_oauth_flow(
    email: &str,
    user_client_id: &str,
    user_client_secret: &str,
) -> Result<(String, String, String), String> {
    if !email.to_lowercase().ends_with("@gmail.com") {
        return Err("OAuth is currently only implemented for Gmail/Google accounts.".into());
    }

    let redirect_uri = "http://127.0.0.1:8080";
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=https://mail.google.com/&access_type=offline&prompt=consent",
        user_client_id, redirect_uri
    );

    println!("   Opening default web browser for OAuth authentication...");
    println!(
        "   If the browser does not open automatically, manually visit:\r\n\r\n{}\r\n",
        auth_url
    );

    let _ = crate::browser::open_url(&auth_url);

    println!("   Listening for authorization code on {}...", redirect_uri);
    let listener = TcpListener::bind("127.0.0.1:8080")
        .map_err(|e| format!("Failed to bind to port 8080: {}", e))?;

    let mut auth_code = String::new();

    if let Some(stream_res) = listener.incoming().next() {
        let mut stream = stream_res.map_err(|e| format!("Local server error: {}", e))?;
        let mut buffer = [0; 2048];
        if let Ok(bytes_read) = stream.read(&mut buffer) {
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            if request.starts_with("GET") {
                if let Some(code_start) = request.find("code=") {
                    let code_part = &request[code_start + 5..];
                    if let Some(code_end) = code_part.find('&').or_else(|| code_part.find(' ')) {
                        auth_code = code_part[..code_end].to_string();
                    }
                }
            }
        }

        let response = if !auth_code.is_empty() {
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style=\"font-family: sans-serif; text-align: center; margin-top: 50px;\"><h2>Authentication successful!</h2><p>You can close this window and return to xpine.</p></body></html>"
        } else {
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Authentication failed!</h2><p>No authorization code found in the request.</p></body></html>"
        };
        let _ = stream.write_all(response.as_bytes());
    }

    if auth_code.is_empty() {
        return Err("Failed to retrieve the authorization code from the browser.".into());
    }

    println!("   Code received! Exchanging for tokens...");

    let params = [
        ("client_id", user_client_id),
        ("client_secret", user_client_secret),
        ("code", auth_code.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];

    let res = http_client()
        .post(GOOGLE_TOKEN_ENDPOINT)
        .form(&params)
        .send()
        .map_err(|e| format!("Network error during token exchange: {}", e))?;

    if !res.status().is_success() {
        let err_text = res.text().unwrap_or_default();
        return Err(format!("Token exchange failed: {}", err_text));
    }

    let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
    if let Some(refresh) = token_res.refresh_token {
        Ok((
            user_client_id.to_string(),
            user_client_secret.to_string(),
            refresh,
        ))
    } else {
        Err("Google did not return a refresh token. You may need to revoke xpine's access in your Google account settings and try again.".into())
    }
}

pub fn get_oauth_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    is_microsoft: bool,
    target_scope: Option<&str>,
) -> Result<String, String> {
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
        MS_TOKEN_ENDPOINT
    } else {
        GOOGLE_TOKEN_ENDPOINT
    };

    let res = http_client()
        .post(endpoint)
        .form(&params)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if res.status().is_success() {
        let token_res: TokenResponse = res.json().map_err(|e| format!("Parse error: {}", e))?;
        Ok(token_res.access_token)
    } else {
        let err_text = res.text().unwrap_or_default();
        Err(format!("OAuth error: {:?}", err_text))
    }
}

fn open_imap_client(
    domain: &str,
    port: u16,
) -> Result<imap::Client<native_tls::TlsStream<TcpStream>>, String> {
    let addr = format!("{}:{}", domain, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("Could not resolve IMAP server address")?;

    let tcp_stream =
        TcpStream::connect_timeout(&addr, NETWORK_TIMEOUT).map_err(|e| e.to_string())?;
    tcp_stream
        .set_read_timeout(Some(NETWORK_TIMEOUT))
        .map_err(|e| e.to_string())?;
    tcp_stream
        .set_write_timeout(Some(NETWORK_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let tls = TlsConnector::builder().build().map_err(|e| e.to_string())?;
    let tls_stream = tls.connect(domain, tcp_stream).map_err(|e| e.to_string())?;

    Ok(imap::Client::new(tls_stream))
}

pub fn connect(account: &mut Account) -> Result<MailSession, String> {
    let email_lower = account.email.to_lowercase();
    let is_microsoft =
        email_lower.ends_with("@outlook.com") || email_lower.ends_with("@hotmail.com");

    // 1. Try OAuth flow first
    if let (Some(client_id), Some(client_secret), Some(refresh_token)) = (
        &account.client_id,
        &account.client_secret,
        &account.refresh_token,
    ) {
        let scope = if is_microsoft {
            Some(MS_OAUTH_SCOPE)
        } else {
            None
        };

        if let Ok(access_token) =
            get_oauth_access_token(client_id, client_secret, refresh_token, is_microsoft, scope)
        {
            if is_microsoft {
                return Ok(MailSession::Graph { access_token });
            }

            let client = open_imap_client(&account.imap_server, account.imap_port)?;
            let auth = OAuth2 {
                user: account.email.clone(),
                access_token,
            };

            if let Ok(session) = client.authenticate("XOAUTH2", &auth) {
                return Ok(MailSession::Imap(session));
            }
        }
    }

    // 2. Fallback to password-based IMAP
    let client = open_imap_client(&account.imap_server, account.imap_port)?;
    if let Some(password) = &account.password {
        let session = client
            .login(&account.email, password)
            .map_err(|(e, _)| e.to_string())?;
        Ok(MailSession::Imap(session))
    } else {
        Err("No password or valid OAuth credentials provided".to_string())
    }
}

fn decode_mime_string(raw: &[u8]) -> String {
    let mut header_bytes = b"X-Dummy: ".to_vec();
    header_bytes.extend_from_slice(raw);
    header_bytes.extend_from_slice(b"\r\n");

    if let Ok((header, _)) = mailparse::parse_header(&header_bytes) {
        header.get_value()
    } else {
        String::from_utf8_lossy(raw).into_owned()
    }
}

fn format_email_dates(dt: &DateTime<FixedOffset>, old_date_fmt: &str) -> (String, String) {
    let diff = Utc::now().timestamp() - dt.timestamp();
    let local_dt = dt.with_timezone(&Local);
    let short_date = if (-86400..7 * 24 * 3600).contains(&diff) {
        local_dt.format("%a %H:%M").to_string()
    } else {
        local_dt.format(old_date_fmt).to_string()
    };
    let full_date = local_dt.format("%a, %d %b %Y %H:%M:%S %z").to_string();
    (short_date, full_date)
}

fn format_graph_recipient(recipient: &GraphRecipient) -> Option<String> {
    let email = recipient.email_address.as_ref()?;
    let name = email.name.as_deref().unwrap_or("");
    let addr = email.address.as_deref().unwrap_or("");
    Some(if !name.is_empty() {
        format!("{} <{}>", name, addr)
    } else {
        addr.to_string()
    })
}

fn format_graph_addrs(recipients: Option<&[GraphRecipient]>) -> String {
    recipients
        .map(|list| {
            list.iter()
                .filter_map(format_graph_recipient)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn find_graph_folder_id(access_token: &str, display_name: &str) -> Result<String, String> {
    let escaped = display_name.replace('\'', "''");
    let res = http_client()
        .get(format!("{}/mailFolders", GRAPH_BASE_URL))
        .query(&[("$filter", format!("displayName eq '{}'", escaped))])
        .bearer_auth(access_token)
        .send()
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Graph API error: {}", res.status()));
    }

    let data: GraphFolderResponse = res.json().map_err(|e| e.to_string())?;
    data.value
        .into_iter()
        .next()
        .map(|f| f.id)
        .ok_or_else(|| format!("Folder '{}' not found on Microsoft servers", display_name))
}

fn resolve_graph_folder_id(access_token: &str, folder_name: &str) -> String {
    match folder_name {
        "INBOX" => "inbox".to_string(),
        "Sent Items" => "sentitems".to_string(),
        "Deleted Items" => "deleteditems".to_string(),
        "Drafts" => "drafts".to_string(),
        "Junk Email" => "junkemail".to_string(),
        "Archive" => "archive".to_string(),
        other => {
            find_graph_folder_id(access_token, other).unwrap_or_else(|_| other.to_string())
        }
    }
}

fn finalize_page_emails(app: &mut App, items_per_page: u32, sort_newest_first: bool) {
    if sort_newest_first {
        app.page_emails.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    } else {
        app.page_emails.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    }

    let last_page = app.total_messages.saturating_sub(1) / items_per_page;
    let overlap = if app.total_messages >= items_per_page {
        (last_page * items_per_page)
            .saturating_sub(app.total_messages.saturating_sub(items_per_page))
    } else {
        0
    } as usize;

    if let Some(idx_from_end) = app.restore_index_from_end.take() {
        app.selected_index = if app.page_emails.is_empty() {
            0
        } else if sort_newest_first {
            idx_from_end as usize
        } else {
            app.page_emails
                .len()
                .saturating_sub(1)
                .saturating_sub(idx_from_end as usize)
        };
        return;
    }

    let max_idx = (items_per_page as usize).saturating_sub(1);
    if overlap > 0 {
        if sort_newest_first {
            if app.current_page == last_page && app.selected_index == 0 {
                app.selected_index = overlap;
            } else if app.current_page == last_page.saturating_sub(1)
                && app.selected_index == max_idx
            {
                app.selected_index = max_idx.saturating_sub(overlap);
            }
        } else if app.current_page == last_page && app.selected_index == max_idx {
            app.selected_index = max_idx.saturating_sub(overlap);
        } else if app.current_page == last_page.saturating_sub(1) && app.selected_index == 0 {
            app.selected_index = overlap;
        }
    }

    if app.selected_index >= app.page_emails.len() {
        app.selected_index = app.page_emails.len().saturating_sub(1);
    }
}

pub fn fetch_emails(
    session: &mut MailSession,
    app: &mut App,
    items_per_page: u32,
    sort_newest_first: bool,
    use_dmy_dates: bool,
) {
    let old_date_fmt = if use_dmy_dates { "%d/%m/%y" } else { "%m/%d/%y" };
    flush_pending_flags_with_session(session, app);
    app.page_emails.clear();

    match session {
        MailSession::Imap(imap_sess) => {
            fetch_imap_emails(imap_sess, app, items_per_page, sort_newest_first, old_date_fmt);
        }
        MailSession::Graph { access_token } => {
            fetch_graph_emails(
                access_token,
                app,
                items_per_page,
                sort_newest_first,
                old_date_fmt,
            );
        }
    }
}

fn fetch_imap_emails(
    imap_sess: &mut RawImapSession,
    app: &mut App,
    items_per_page: u32,
    sort_newest_first: bool,
    old_date_fmt: &str,
) {
    match imap_sess.select(&app.current_folder) {
        Ok(m) => app.total_messages = m.exists,
        Err(_) => {
            app.needs_reconnect = true;
            return;
        }
    }

    if !flush_imap_flags(imap_sess, app) {
        return;
    }

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

                let mut end_idx =
                    sorted_seqs.len().saturating_sub((app.current_page * items_per_page) as usize);
                let start_idx = end_idx.saturating_sub(items_per_page as usize - 1).max(1);
                if start_idx == 1 {
                    end_idx = (items_per_page as usize).min(sorted_seqs.len());
                }

                sorted_seqs[(start_idx - 1)..end_idx]
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            }
            _ => {
                app.total_messages = 0;
                return;
            }
        }
    } else if app.total_messages > 0 {
        let mut end_idx = app.total_messages.saturating_sub(app.current_page * items_per_page);
        let start_idx = end_idx.saturating_sub(items_per_page - 1).max(1);
        if start_idx == 1 {
            end_idx = items_per_page.min(app.total_messages);
        }
        format!("{}:{}", start_idx, end_idx)
    } else {
        return;
    };

    if sequence.is_empty() {
        return;
    }

    if let Ok(messages) = imap_sess.fetch(
        &sequence,
        "(UID ENVELOPE FLAGS RFC822.SIZE INTERNALDATE BODYSTRUCTURE)",
    ) {
        for message in messages.iter() {
            let size = message.size.unwrap_or(0);
            let mut is_seen = false;
            let mut is_deleted = false;
            let mut is_flagged = false;
            let mut is_answered = false;

            let has_attachments = message.bodystructure().is_some_and(|bs| {
                let bs_str = format!("{:?}", bs).to_lowercase();
                bs_str.contains("attachment") || bs_str.contains("filename")
            });

            for flag in message.flags() {
                match flag {
                    imap::types::Flag::Seen => is_seen = true,
                    imap::types::Flag::Deleted => is_deleted = true,
                    imap::types::Flag::Flagged => is_flagged = true,
                    imap::types::Flag::Answered => is_answered = true,
                    _ => {}
                }
            }

            let internal_date = message.internal_date().map(|dt| dt.timestamp()).unwrap_or(0);

            let mut subject = "No Subject".to_string();
            let mut from = "Unknown Sender".to_string();
            let mut reply_to = "unknown@example.com".to_string();
            let mut to_addr = String::new();
            let mut cc = String::new();
            let mut date = "Unknown Date".to_string();
            let mut full_date = "Unknown Date".to_string();

            if let Some(env) = message.envelope() {
                if let Some(s) = env.subject.as_ref() {
                    subject = decode_mime_string(s);
                }

                (date, full_date) = match env.date.as_ref() {
                    Some(d) => {
                        let raw_date = String::from_utf8_lossy(d).into_owned();
                        if let Ok(dt) = DateTime::parse_from_rfc2822(&raw_date) {
                            format_email_dates(&dt, old_date_fmt)
                        } else if let Some(dt) = message.internal_date() {
                            format_email_dates(&dt, old_date_fmt)
                        } else {
                            let short = raw_date
                                .split(" +")
                                .next()
                                .unwrap_or(&raw_date)
                                .chars()
                                .take(6)
                                .collect();
                            (short, raw_date)
                        }
                    }
                    None => message
                        .internal_date()
                        .map(|dt| format_email_dates(&dt, old_date_fmt))
                        .unwrap_or_else(|| {
                            ("Unknown Date".to_string(), "Unknown Date".to_string())
                        }),
                };

                macro_rules! format_addrs {
                    ($addrs:expr) => {{
                        let mut result = Vec::new();
                        if let Some(a_vec) = $addrs {
                            for addr in a_vec {
                                let name = addr
                                    .name
                                    .as_ref()
                                    .map(|n| decode_mime_string(n.as_ref()))
                                    .unwrap_or_default();
                                let mailbox = addr
                                    .mailbox
                                    .as_ref()
                                    .map(|m| String::from_utf8_lossy(m.as_ref()).into_owned())
                                    .unwrap_or_default();
                                let host = addr
                                    .host
                                    .as_ref()
                                    .map(|h| String::from_utf8_lossy(h.as_ref()).into_owned())
                                    .unwrap_or_default();

                                let email_raw = format!("{}@{}", mailbox, host);
                                let formatted = if !name.is_empty() {
                                    format!("{} <{}>", name, email_raw)
                                } else {
                                    email_raw
                                };
                                result.push(formatted);
                            }
                        }
                        result.join(", ")
                    }};
                }

                from = format_addrs!(env.from.as_ref());
                if from.is_empty() {
                    from = "Unknown Sender".to_string();
                }

                to_addr = format_addrs!(env.to.as_ref());
                cc = format_addrs!(env.cc.as_ref());

                if let Some(addr) = env
                    .reply_to
                    .as_ref()
                    .or(env.from.as_ref())
                    .and_then(|v| v.first())
                {
                    let mailbox = addr
                        .mailbox
                        .as_ref()
                        .map(|m| String::from_utf8_lossy(m.as_ref()).into_owned())
                        .unwrap_or_default();
                    let host = addr
                        .host
                        .as_ref()
                        .map(|h| String::from_utf8_lossy(h.as_ref()).into_owned())
                        .unwrap_or_default();
                    reply_to = format!("{}@{}", mailbox, host);
                }
            }

            app.page_emails.push(EmailMeta {
                id: message.message.to_string(),
                uid: message.uid.unwrap_or(0),
                timestamp: internal_date,
                subject,
                from,
                reply_to,
                to_addr,
                cc,
                date,
                full_date,
                size,
                is_read: is_seen,
                is_deleted,
                is_flagged,
                is_answered,
                has_attachments,
            });
        }
    }

    finalize_page_emails(app, items_per_page, sort_newest_first);
}

fn fetch_graph_emails(
    access_token: &str,
    app: &mut App,
    items_per_page: u32,
    sort_newest_first: bool,
    old_date_fmt: &str,
) {
    let folder = resolve_graph_folder_id(access_token, &app.current_folder);

    let mut skip = app.current_page * items_per_page;
    if app.current_page > 0
        && skip + items_per_page > app.total_messages
        && app.total_messages >= items_per_page
    {
        skip = app.total_messages - items_per_page;
    }

    let is_search = app.search_query.is_some();
    let url = if let Some(ref q) = app.search_query {
        let trimmed = q.trim();
        if trimmed == "*" {
            format!(
                "{}/mailFolders/{}/messages?$top=250&$filter=flag/flagStatus%20eq%20'flagged'&{}",
                GRAPH_BASE_URL, folder, GRAPH_EXTENDED_PROPS_EXPAND
            )
        } else if trimmed.eq_ignore_ascii_case("n") {
            format!(
                "{}/mailFolders/{}/messages?$top=250&$orderby=receivedDateTime%20DESC&$filter=isRead%20eq%20false&{}",
                GRAPH_BASE_URL, folder, GRAPH_EXTENDED_PROPS_EXPAND
            )
        } else {
            let clean_q = trimmed.replace('\'', "''");
            let encoded_q = urlencoding::encode(&clean_q);
            format!(
                "{}/mailFolders/{}/messages?$top=250&$count=true&$filter=contains(subject,'{}')%20or%20contains(from/emailAddress/name,'{}')%20or%20contains(from/emailAddress/address,'{}')&{}",
                GRAPH_BASE_URL, folder, encoded_q, encoded_q, encoded_q, GRAPH_EXTENDED_PROPS_EXPAND
            )
        }
    } else {
        format!(
            "{}/mailFolders/{}/messages?$count=true&$top={}&$skip={}&$orderby=receivedDateTime%20DESC&{}",
            GRAPH_BASE_URL, folder, items_per_page, skip, GRAPH_EXTENDED_PROPS_EXPAND
        )
    };

    let res = http_client()
        .get(&url)
        .bearer_auth(access_token)
        .header("ConsistencyLevel", "eventual")
        .send();

    match res {
        Ok(response) if response.status().is_success() => {
            let Ok(graph_data) = response.json::<GraphMessageResponse>() else {
                app.update_status("Failed to parse Graph JSON data.".to_string());
                return;
            };

            if let Some(total) = graph_data.count {
                app.total_messages = total;
            } else if is_search {
                app.total_messages = graph_data.value.len() as u32;
            }

            let messages: Vec<GraphMessage> = if is_search {
                let mut all_msgs = graph_data.value;
                if sort_newest_first {
                    all_msgs.sort_by(|a, b| b.received_date_time.cmp(&a.received_date_time));
                } else {
                    all_msgs.sort_by(|a, b| a.received_date_time.cmp(&b.received_date_time));
                }

                let page_len = items_per_page as usize;
                let mut local_start = (app.current_page * items_per_page) as usize;
                if app.current_page > 0
                    && local_start + page_len > all_msgs.len()
                    && all_msgs.len() >= page_len
                {
                    local_start = all_msgs.len() - page_len;
                }

                all_msgs
                    .into_iter()
                    .skip(local_start)
                    .take(page_len)
                    .collect()
            } else {
                graph_data.value
            };

            for msg in messages {
                let mut is_answered = false;
                let mut msg_size = 0;

                if let Some(props) = &msg.single_value_extended_properties {
                    for prop in props {
                        if prop.id == "Integer 0x1081" && prop.value == "102" {
                            is_answered = true;
                        } else if prop.id.to_lowercase().contains("0x0e08")
                            || prop.id.to_lowercase().contains("0xe08")
                        {
                            if let Ok(parsed_size) = prop.value.parse::<u32>() {
                                msg_size = parsed_size;
                            }
                        }
                    }
                }

                let (internal_date, date_str, full_date_str) =
                    match DateTime::parse_from_rfc3339(&msg.received_date_time) {
                        Ok(dt) => {
                            let (short, full) = format_email_dates(&dt, old_date_fmt);
                            (dt.timestamp(), short, full)
                        }
                        Err(_) => (0, "Unknown Date".to_string(), msg.received_date_time.clone()),
                    };

                let from = msg
                    .from
                    .as_ref()
                    .and_then(format_graph_recipient)
                    .unwrap_or_else(|| "Unknown Sender".to_string());

                let to_addr = format_graph_addrs(msg.to_recipients.as_deref());
                let cc = format_graph_addrs(msg.cc_recipients.as_deref());
                let reply_to = format_graph_addrs(msg.reply_to.as_deref());

                let is_flagged = msg
                    .flag
                    .and_then(|f| f.flag_status)
                    .is_some_and(|s| s.eq_ignore_ascii_case("flagged"));
                let is_deleted_locally = app.graph_pending_deleted.contains(&msg.id);

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
                    full_date: full_date_str,
                    size: msg_size,
                    is_read: msg.is_read,
                    is_deleted: is_deleted_locally,
                    is_flagged,
                    is_answered,
                    has_attachments: msg.has_attachments.unwrap_or(false),
                });
            }

            finalize_page_emails(app, items_per_page, sort_newest_first);
        }
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            app.needs_reconnect = true;
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

enum GraphFlagOp {
    Patch {
        access_token: String,
        message_id: String,
        body: String,
    },
    Sync(mpsc::Sender<()>),
}

// Mutex is required because mpsc::Sender is Send but not Sync
static GRAPH_FLAG_TX: OnceLock<Mutex<mpsc::Sender<GraphFlagOp>>> = OnceLock::new();

fn graph_flag_sender() -> &'static Mutex<mpsc::Sender<GraphFlagOp>> {
    GRAPH_FLAG_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<GraphFlagOp>();
        thread::spawn(move || {
            let client = http_client();
            while let Ok(op) = rx.recv() {
                match op {
                    GraphFlagOp::Patch {
                        access_token,
                        message_id,
                        body,
                    } => {
                        let url = format!("{}/messages/{}", GRAPH_BASE_URL, message_id);
                        let _ = client
                            .patch(&url)
                            .bearer_auth(&access_token)
                            .header("Content-Type", "application/json")
                            .body(body)
                            .send();
                    }
                    GraphFlagOp::Sync(done_tx) => {
                        let _ = done_tx.send(());
                    }
                }
            }
        });
        Mutex::new(tx)
    })
}

pub fn flush_graph_flags() {
    if let Some(tx_mutex) = GRAPH_FLAG_TX.get() {
        let (done_tx, done_rx) = mpsc::channel();
        if let Ok(tx) = tx_mutex.lock() {
            if tx.send(GraphFlagOp::Sync(done_tx)).is_ok() {
                drop(tx);
                let _ = done_rx.recv_timeout(Duration::from_secs(5));
            }
        }
    }
}

fn flush_imap_flags(imap_sess: &mut RawImapSession, app: &mut App) -> bool {
    if app.pending_imap_flags.is_empty() {
        app.last_imap_flag_time = None;
        return true;
    }

    let pending = std::mem::take(&mut app.pending_imap_flags);
    app.last_imap_flag_time = None;

    let mut grouped: HashMap<(&'static str, bool), Vec<u32>> = HashMap::new();
    for ((uid, flag_name), is_set) in pending {
        if uid > 0 {
            grouped.entry((flag_name, is_set)).or_default().push(uid);
        }
    }

    let mut failed = false;
    for ((flag_name, is_set), uids) in grouped {
        if failed {
            for uid in uids {
                app.pending_imap_flags.insert((uid, flag_name), is_set);
            }
            continue;
        }

        let uid_set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let sign = if is_set { "+" } else { "-" };
        let op = format!("{}FLAGS.SILENT ({})", sign, flag_name);

        if imap_sess.uid_store(&uid_set, &op).is_err() {
            failed = true;
            for uid in uids {
                app.pending_imap_flags.insert((uid, flag_name), is_set);
            }
        }
    }

    if failed {
        app.needs_reconnect = true;
        false
    } else {
        true
    }
}

pub fn flush_pending_flags_with_session(session: &mut MailSession, app: &mut App) {
    match session {
        MailSession::Imap(imap_sess) => {
            flush_imap_flags(imap_sess, app);
        }
        MailSession::Graph { .. } => {
            flush_graph_flags();
        }
    }
}

pub fn flush_pending_flags(session: &mut Option<MailSession>, app: &mut App) {
    if let Some(sess) = session.as_mut() {
        flush_pending_flags_with_session(sess, app);
    }
}

pub fn toggle_flag(session: &MailSession, app: &mut App, flag_name: &'static str) {
    let Some(email) = app.page_emails.get_mut(app.selected_index) else {
        return;
    };

    // 1. Flip locally right away so the UI responds with zero delay
    let new_val = match flag_name {
        "\\Flagged" => {
            email.is_flagged = !email.is_flagged;
            email.is_flagged
        }
        "\\Deleted" => {
            email.is_deleted = !email.is_deleted;
            email.is_deleted
        }
        "\\Seen" => {
            email.is_read = !email.is_read;
            email.is_read
        }
        "\\Answered" => {
            email.is_answered = !email.is_answered;
            email.is_answered
        }
        _ => return,
    };

    let uid = email.uid;
    let id = email.id.clone();

    // 2. Queue IMAP flush or dispatch Graph PATCH in the background
    match session {
        MailSession::Imap(_) => {
            app.pending_imap_flags.insert((uid, flag_name), new_val);
            app.last_imap_flag_time = Some(Instant::now());
        }
        MailSession::Graph { access_token } => {
            let body_str = match flag_name {
                "\\Seen" => format!(r#"{{"isRead": {}}}"#, new_val),
                "\\Flagged" => {
                    let status = if new_val { "flagged" } else { "notFlagged" };
                    format!(r#"{{"flag": {{"flagStatus": "{}"}}}}"#, status)
                }
                "\\Answered" => {
                    let val = if new_val { "102" } else { "0" };
                    format!(
                        r#"{{"singleValueExtendedProperties": [{{"id": "Integer 0x1081", "value": "{}"}}]}}"#,
                        val
                    )
                }
                _ => return,
            };

            if let Ok(tx) = graph_flag_sender().lock() {
                let _ = tx.send(GraphFlagOp::Patch {
                    access_token: access_token.clone(),
                    message_id: id,
                    body: body_str,
                });
            }
        }
    }
}

pub fn expunge_deleted(session: &mut MailSession, app: &mut App) -> Result<(), String> {
    flush_pending_flags_with_session(session, app);
    match session {
        MailSession::Imap(imap_sess) => {
            imap_sess
                .expunge()
                .map_err(|e| format!("Expunge failed: {}", e))?;
            app.needs_fetch = true;
            Ok(())
        }
        MailSession::Graph { access_token } => {
            let client = http_client();
            for id in &app.graph_pending_deleted {
                let url = format!("{}/messages/{}", GRAPH_BASE_URL, id);
                let _ = client.delete(&url).bearer_auth(access_token.as_str()).send();
            }
            app.graph_pending_deleted.clear();
            app.needs_fetch = true;
            Ok(())
        }
    }
}

pub fn fetch_email_body(
    session: &mut MailSession,
    fetch_seq: &str,
) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    match session {
        MailSession::Imap(imap_sess) => {
            if let Ok(fetched_messages) = imap_sess.fetch(fetch_seq, "RFC822") {
                if let Some(body_data) = fetched_messages.iter().next().and_then(|m| m.body()) {
                    return parse_email_body(body_data);
                }
            }
            (String::from("Could not load email body."), None, Vec::new())
        }
        MailSession::Graph { access_token } => {
            let url = format!("{}/messages/{}/$value", GRAPH_BASE_URL, fetch_seq);
            if let Ok(res) = http_client().get(&url).bearer_auth(access_token).send() {
                if res.status().is_success() {
                    if let Ok(bytes) = res.bytes() {
                        return parse_email_body(&bytes);
                    }
                } else {
                    return (format!("Graph API Error: {}", res.status()), None, Vec::new());
                }
            }
            (String::from("Could not load email body."), None, Vec::new())
        }
    }
}

pub fn create_folder(session: &mut MailSession, folder_name: &str) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => imap_sess
            .create(folder_name)
            .map_err(|e| format!("Failed to create folder: {}", e)),
        MailSession::Graph { access_token } => {
            let res = http_client()
                .post(format!("{}/mailFolders", GRAPH_BASE_URL))
                .bearer_auth(access_token)
                .json(&serde_json::json!({ "displayName": folder_name }))
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
        MailSession::Imap(imap_sess) => imap_sess
            .delete(folder_name)
            .map_err(|e| format!("Failed to delete folder: {}", e)),
        MailSession::Graph { access_token } => {
            let folder_id = find_graph_folder_id(access_token, folder_name)?;
            let del_res = http_client()
                .delete(format!("{}/mailFolders/{}", GRAPH_BASE_URL, folder_id))
                .bearer_auth(access_token)
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
            let mailboxes = imap_sess
                .list(Some(""), Some("*"))
                .map_err(|e| format!("Failed to fetch folders: {}", e))?;
            Ok(mailboxes.iter().map(|m| m.name().to_string()).collect())
        }
        MailSession::Graph { access_token } => {
            let res = http_client()
                .get(format!("{}/mailFolders?$top=250", GRAPH_BASE_URL))
                .bearer_auth(access_token)
                .send()
                .map_err(|e| e.to_string())?;

            if !res.status().is_success() {
                return Err(format!("Graph API error: {}", res.status()));
            }

            let data: GraphFolderResponse = res.json().map_err(|e| e.to_string())?;
            Ok(data.value.into_iter().map(|f| f.display_name).collect())
        }
    }
}

pub fn rename_folder(
    session: &mut MailSession,
    old_name: &str,
    new_name: &str,
) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => imap_sess
            .rename(old_name, new_name)
            .map_err(|e| format!("Failed to rename folder: {}", e)),
        MailSession::Graph { access_token } => {
            let folder_id = find_graph_folder_id(access_token, old_name)?;
            let patch_res = http_client()
                .patch(format!("{}/mailFolders/{}", GRAPH_BASE_URL, folder_id))
                .bearer_auth(access_token)
                .json(&serde_json::json!({ "displayName": new_name }))
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

pub fn move_email(
    session: &mut MailSession,
    message_seq: &str,
    dest_folder: &str,
) -> Result<(), String> {
    match session {
        MailSession::Imap(imap_sess) => {
            imap_sess
                .copy(message_seq, dest_folder)
                .map_err(|e| format!("Failed to copy email: {}", e))?;
            imap_sess
                .store(message_seq, "+FLAGS (\\Deleted)")
                .map_err(|e| format!("Failed to delete original: {}", e))?;
            imap_sess
                .expunge()
                .map_err(|e| format!("Failed to expunge: {}", e))?;
            Ok(())
        }
        MailSession::Graph { access_token } => {
            let dest_id = find_graph_folder_id(access_token, dest_folder)?;
            let move_res = http_client()
                .post(format!("{}/messages/{}/move", GRAPH_BASE_URL, message_seq))
                .bearer_auth(access_token)
                .json(&serde_json::json!({ "destinationId": dest_id }))
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

pub struct SessionPool {
    sessions: HashMap<String, (MailSession, Instant)>,
    active_email: Option<String>,
}

impl SessionPool {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            active_email: None,
        }
    }

    pub fn prune(&mut self, accounts: &[Account]) {
        self.sessions
            .retain(|email, _| accounts.iter().any(|a| &a.email == email));
    }

    pub fn close_all(&mut self) {
        self.sessions.clear();
        self.active_email = None;
    }
}

pub fn reconnect(app: &mut App, session: &mut Option<MailSession>, pool: &mut SessionPool) {
    pool.prune(&app.accounts);
    app.account_cache
        .retain(|email, _| app.accounts.iter().any(|a| &a.email == email));

    if app.accounts.is_empty() {
        app.pending_imap_flags.clear();
        app.last_imap_flag_time = None;
        if let Some(MailSession::Imap(mut imap_sess)) = session.take() {
            let _ = imap_sess.logout();
        }
        pool.active_email = None;
        app.cache_ready = false;
        app.needs_reconnect = false;
        app.last_fetch_time = Instant::now();
        return;
    }

    if app.current_account_idx >= app.accounts.len() {
        app.current_account_idx = 0;
    }

    let target_email = app.accounts[app.current_account_idx].email.clone();
    let is_same_account = pool.active_email.as_deref() == Some(target_email.as_str());

    // Only discard pending flags when switching to a different account;
    // keep them on a same-account reconnect so they flush on the fresh socket.
    if !is_same_account {
        app.pending_imap_flags.clear();
        app.last_imap_flag_time = None;
    }

    // 1. Park the previous account's session and UI state when switching accounts
    if let Some(old_sess) = session.take() {
        if let Some(old_email) = pool.active_email.take() {
            let old_account_exists = app.accounts.iter().any(|a| a.email == old_email);

            if !is_same_account && old_account_exists {
                pool.sessions
                    .insert(old_email.clone(), (old_sess, Instant::now()));

                if app.cache_ready {
                    app.account_cache.insert(
                        old_email,
                        CachedAccountView {
                            page_emails: std::mem::take(&mut app.page_emails),
                            total_messages: app.total_messages,
                            current_folder: app.current_folder.clone(),
                            current_page: app.current_page,
                            selected_index: app.selected_index,
                            search_query: app.search_query.take(),
                            graph_pending_deleted: std::mem::take(&mut app.graph_pending_deleted),
                            last_fetched: app.last_fetch_time,
                        },
                    );
                }
            } else {
                // Broken/forced reconnect or deleted account: drop directly instead of blocking on logout()
                drop(old_sess);
            }
        } else {
            drop(old_sess);
        }
    }

    app.active_account = app.accounts[app.current_account_idx].clone();

    // 2. Acquire a session (reuse pooled session only if it hasn't gone stale)
    let mut connected = false;
    let mut reused_session = false;
    if !is_same_account {
        if let Some((cached_sess, parked_at)) = pool.sessions.remove(&target_email) {
            let max_idle = match &cached_sess {
                MailSession::Imap(_) => Duration::from_secs(120),       // 2 min for IMAP
                MailSession::Graph { .. } => Duration::from_secs(2700), // 45 min for Graph
            };
            if parked_at.elapsed() < max_idle {
                *session = Some(cached_sess);
                pool.active_email = Some(target_email.clone());
                connected = true;
                reused_session = true;
            } else {
                // Expired idle session: drop without blocking on logout()
                drop(cached_sess);
            }
        }
    } else {
        pool.sessions.remove(&target_email);
        app.account_cache.remove(&target_email);
    }

    if !connected {
        match connect(&mut app.active_account) {
            Ok(sess) => {
                *session = Some(sess);
                pool.active_email = Some(target_email.clone());
                connected = true;
            }
            Err(e) => {
                *session = None;
                pool.active_email = None;
                let err_str = e.to_lowercase();
                if err_str.contains("timeout")
                    || err_str.contains("timed out")
                    || err_str.contains("would block")
                {
                    app.update_status("Attempted connection timed out".to_string());
                } else {
                    app.update_status("Connection failed".to_string());
                }
            }
        }
    }

    // 3. Restore cached view on account switch
    if !is_same_account {
        if let Some(cached) = app.account_cache.remove(&target_email) {
            app.page_emails = cached.page_emails;
            app.total_messages = cached.total_messages;
            app.current_folder = cached.current_folder;
            app.current_page = cached.current_page;
            app.selected_index = cached.selected_index;
            app.search_query = cached.search_query;
            app.graph_pending_deleted = cached.graph_pending_deleted;
            app.restore_index_from_end = None;
            app.last_fetch_time = cached.last_fetched;
            app.cache_ready = reused_session;
            app.needs_fetch = !reused_session && connected;
        } else {
            app.page_emails.clear();
            app.total_messages = 0;
            app.current_folder = "INBOX".to_string();
            app.current_page = 0;
            app.selected_index = 0;
            app.search_query = None;
            app.graph_pending_deleted.clear();
            app.restore_index_from_end = Some(0);
            app.cache_ready = false;
            app.needs_fetch = connected;
            app.last_fetch_time = Instant::now();
        }
    } else {
        app.cache_ready = false;
        app.needs_fetch = connected;
        app.last_fetch_time = Instant::now();
    }

    app.needs_reconnect = false;
}