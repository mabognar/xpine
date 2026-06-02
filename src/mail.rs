pub struct EmailMeta {
    pub id: String,
    pub uid: u32,
    pub timestamp: i64,
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

pub fn format_reply_text(original_text: &str) -> String {
    // Start with 3 blank lines using CRLF to match the terminal editor's expected line endings
    let mut reply = String::from("\r\n\r\n\r\n");

    // Iterate through the original email and prefix each line
    for line in original_text.lines() {
        reply.push_str("> ");
        reply.push_str(line);
        reply.push_str("\r\n");
    }

    reply
}

pub(crate) fn extract_email(formatted: &str) -> String {
    // If it contains <...>, extract what's inside
    if let Some(start) = formatted.find('<') {
        if let Some(end) = formatted.find('>') {
            return formatted[start + 1..end].trim().to_string();
        }
    }
    // Otherwise, assume it's already just an email
    formatted.trim().to_string()
}

