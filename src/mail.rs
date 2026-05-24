// #[derive(Clone)]
// pub struct Account {
//     pub email: String,
//     pub password: String,
//     pub imap_server: String,
//     pub imap_port: u16,
//     pub smtp_server: String,
// }

use crate::config::Account;
use serde::{Deserialize, Serialize};

// #[derive(Clone, Deserialize, Serialize)]
// pub struct Account {
//     pub email: String,
//
//     // Standard Auth
//     pub password: Option<String>,
//
//     // Google OAuth 2.0
//     pub client_id: Option<String>,
//     pub client_secret: Option<String>,
//     pub refresh_token: Option<String>,
//
//     #[serde(default = "default_imap")]
//     pub imap_server: String,
//     #[serde(default = "default_imap_port")]
//     pub imap_port: u16,
//     #[serde(default = "default_smtp")]
//     pub smtp_server: String,
// }

pub struct EmailMeta {
    pub id: u32,
    pub subject: String,
    pub from: String,
    pub reply_to: String,
    pub reply_to_display: String,
    pub to_addr: String,
    pub cc: String,
    pub date: String,
    pub size: u32,
    pub is_read: bool,
    pub is_deleted: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
}

pub fn parse_email_body(body_data: &[u8]) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    let mut text_body = String::new();
    let mut html_body: Option<String> = None;
    let mut attachments: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(parsed) = mailparse::parse_mail(body_data) {
        fn walk(part: &mailparse::ParsedMail, text: &mut String, html: &mut Option<String>, atts: &mut Vec<(String, Vec<u8>)>) {
            let ctype = part.ctype.mimetype.as_str();
            let disposition = part.get_content_disposition();

            let is_attachment = disposition.disposition == mailparse::DispositionType::Attachment ||
                part.headers.iter().any(|header| header.get_key().eq_ignore_ascii_case("content-disposition") && header.get_value().to_lowercase().contains("attachment"));

            if is_attachment || (disposition.disposition == mailparse::DispositionType::Inline && ctype != "text/plain" && ctype != "text/html" && !ctype.starts_with("multipart/")) {
                let filename = disposition.params.get("filename").cloned()
                    .or_else(|| part.ctype.params.get("name").cloned())
                    .unwrap_or_else(|| format!("attachment_{}", atts.len() + 1));

                if let Ok(data) = part.get_body_raw() {
                    atts.push((filename, data));
                }
            } else {
                if ctype == "text/plain" {
                    if let Ok(body) = part.get_body() {
                        text.push_str(&body);
                    }
                } else if ctype == "text/html" {
                    if let Ok(body) = part.get_body() {
                        *html = Some(body);
                    }
                } else {
                    for subpart in &part.subparts {
                        walk(subpart, text, html, atts);
                    }
                }
            }
        }

        walk(&parsed, &mut text_body, &mut html_body, &mut attachments);

        if text_body.is_empty() && html_body.is_some() {
            text_body = html_body.as_ref().unwrap().replace("\r\n", "\n").replace('\n', "\r\n");
        } else if !text_body.is_empty() {
            text_body = text_body.replace("\r\n", "\n").replace('\n', "\r\n");
        } else {
            text_body = String::from_utf8_lossy(body_data).replace("\r\n", "\n").replace('\n', "\r\n");
        }
    } else {
        let raw = String::from_utf8_lossy(body_data);
        text_body = raw.replace('\n', "\r\n");
    }

    (text_body, html_body, attachments)
}
