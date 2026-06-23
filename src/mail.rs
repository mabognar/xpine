pub struct EmailMeta {
    pub id: String,
    pub uid: u32,
    pub timestamp: i64,
    pub subject: String,
    pub from: String,
    pub reply_to: String,
    // pub reply_to_display: String,
    pub to_addr: String,
    pub cc: String,
    pub date: String,
    pub size: u32,
    pub is_read: bool,
    pub is_deleted: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub has_attachments: bool,
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

pub fn format_reply_text(original_text: &str, date: &str, sender: &str) -> String {
    // Start with 3 blank lines using CRLF to match the terminal editor's expected line endings
    let mut reply = String::from("\r\n\r\n\r\n");

    // NEW: Insert the Alpine-style attribution header
    reply.push_str(&format!("On {}, {} wrote:\r\n", date, sender));

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

pub fn wrap_email_body(text: &str, width: usize) -> String {
    let mut result = String::with_capacity(text.len());

    for line in text.lines() {
        if line.chars().count() <= width {
            result.push_str(line);
            result.push('\n');
        } else {
            let mut current_width = 0;
            let mut is_first_word = true;

            for word in line.split(' ') {
                let word_len = word.chars().count();

                if current_width + word_len + 1 > width && !is_first_word {
                    result.push('\n');
                    current_width = 0;
                } else if !is_first_word {
                    result.push(' ');
                    current_width += 1;
                }

                result.push_str(word);
                current_width += word_len;
                is_first_word = false;
            }
            result.push('\n');
        }
    }

    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

pub fn justify_all_text(input: &str) -> String {
    let mut result = String::new();
    let mut current_paragraph = Vec::new();

    for line in input.lines() {
        let trimmed = line.trim();

        let is_numbered_list = trimmed.split_whitespace().next()
            .map(|first_word| {
                let ends_with_punct = first_word.ends_with('.') || first_word.ends_with(')');
                ends_with_punct && first_word.len() > 1 && first_word[..first_word.len()-1].chars().all(|c| c.is_ascii_digit())
            })
            .unwrap_or(false);

        let is_bullet_list = trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ");

        // Removed line.starts_with(' ') and '\t' so paragraphs pack fully together
        if trimmed.is_empty() || line.starts_with('>') || is_numbered_list || is_bullet_list {
            if !current_paragraph.is_empty() {
                result.push_str(&flow_paragraph_words(&current_paragraph, 72));
                current_paragraph.clear();
            }

            result.push_str(line);
            result.push_str("\n");
        } else {
            current_paragraph.push(line);
        }
    }

    if !current_paragraph.is_empty() {
        result.push_str(&flow_paragraph_words(&current_paragraph, 72));
    }

    result
}

fn flow_paragraph_words(lines: &[&str], max_width: usize) -> String {
    let joined_text = lines.join(" ");
    let words: Vec<&str> = joined_text.split_whitespace().collect();
    if words.is_empty() { return String::new(); }

    let mut reflowed = String::new();
    let mut current_line_len = 0;

    for word in words {
        let word_len = word.chars().count(); // Count visual characters, not bytes
        let space_needed = if current_line_len > 0 { 1 } else { 0 };

        if current_line_len + word_len + space_needed > max_width {
            if current_line_len > 0 {
                reflowed.push('\n');
                reflowed.push_str(word);
                current_line_len = word_len;
            } else {
                reflowed.push_str(word);
                current_line_len = word_len;
            }
        } else {
            if current_line_len > 0 { reflowed.push(' '); }
            reflowed.push_str(word);
            current_line_len += word_len + space_needed;
        }
    }
    reflowed.push('\n');
    reflowed
}

pub fn build_reply_all_addresses(
    user_email: &str,
    reply_to_or_from: &str,
    original_to: &str,
    original_cc: &str,
) -> (String, String) {
    let mut to_addresses = Vec::new();
    let mut cc_addresses = Vec::new();

    // Helper to extract and add unique addresses while ignoring self
    let mut add_addrs = |raw_list: &str, target_vec: &mut Vec<String>| {
        for addr in raw_list.split(',') {
            let trimmed = addr.trim();
            if !trimmed.is_empty() {
                // Ignore self
                if !trimmed.to_lowercase().contains(&user_email.to_lowercase()) {
                    let extracted = extract_email(trimmed);
                    if !target_vec.contains(&extracted) {
                        target_vec.push(extracted);
                    }
                }
            }
        }
    };

    // 1. Always add the original sender to the 'To' field
    let sender_extracted = extract_email(reply_to_or_from);
    to_addresses.push(sender_extracted.clone());

    // 2. Add other 'To' addresses to our 'To' field
    add_addrs(original_to, &mut to_addresses);

    // 3. Add 'Cc' addresses to our 'Cc' field
    add_addrs(original_cc, &mut cc_addresses);

    // Remove the sender from CC if they accidentally got caught in it
    cc_addresses.retain(|c| c != &sender_extracted);

    (to_addresses.join(", "), cc_addresses.join(", "))
}

