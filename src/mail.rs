use std::collections::HashSet;

const JUSTIFY_WIDTH: usize = 72;

pub struct EmailMeta {
    pub id: String,
    pub uid: u32,
    pub timestamp: i64,
    pub subject: String,
    pub from: String,
    pub reply_to: String,
    pub to_addr: String,
    pub cc: String,
    pub date: String,
    pub full_date: String,
    pub size: u32,
    pub is_read: bool,
    pub is_deleted: bool,
    pub is_flagged: bool,
    pub is_answered: bool,
    pub has_attachments: bool,
}

fn normalize_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn split_unquoted_commas(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in input.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        if c == ',' && !in_quotes {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

fn clean_single_address(addr: &str) -> String {
    if let Some(start) = addr.find('<') {
        if let Some(end_offset) = addr[start + 1..].find('>') {
            let name_part = addr[..start].trim().trim_matches('"').trim();
            let email_part = addr[start + 1..start + 1 + end_offset].trim();

            return if name_part.is_empty() || name_part.eq_ignore_ascii_case(email_part) {
                email_part.to_string()
            } else {
                format!("{} <{}>", name_part, email_part)
            };
        }
    }
    addr.to_string()
}

fn is_list_item(trimmed: &str) -> bool {
    let is_bullet =
        trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ");
    if is_bullet {
        return true;
    }

    trimmed
        .split_whitespace()
        .next()
        .and_then(|w| w.strip_suffix('.').or_else(|| w.strip_suffix(')')))
        .is_some_and(|prefix| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
}

pub fn parse_email_body(body_data: &[u8]) -> (String, Option<String>, Vec<(String, Vec<u8>)>) {
    let mut text_body = String::new();
    let mut html_body: Option<String> = None;
    let mut attachments: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(parsed) = mailparse::parse_mail(body_data) {
        fn walk(
            part: &mailparse::ParsedMail,
            text: &mut String,
            html: &mut Option<String>,
            atts: &mut Vec<(String, Vec<u8>)>,
        ) {
            let ctype = part.ctype.mimetype.as_str();
            let disposition = part.get_content_disposition();

            let is_attachment = disposition.disposition == mailparse::DispositionType::Attachment
                || part.headers.iter().any(|h| {
                h.get_key().eq_ignore_ascii_case("content-disposition")
                    && h.get_value().to_lowercase().contains("attachment")
            });
            let is_inline_non_text = disposition.disposition == mailparse::DispositionType::Inline
                && ctype != "text/plain"
                && ctype != "text/html"
                && !ctype.starts_with("multipart/");

            if is_attachment || is_inline_non_text {
                let filename = disposition
                    .params
                    .get("filename")
                    .or_else(|| part.ctype.params.get("name"))
                    .cloned()
                    .unwrap_or_else(|| format!("attachment_{}", atts.len() + 1));

                if let Ok(data) = part.get_body_raw() {
                    atts.push((filename, data));
                }
                return;
            }

            match ctype {
                "text/plain" => {
                    if let Ok(body) = part.get_body() {
                        text.push_str(&body);
                    }
                }
                "text/html" => {
                    if let Ok(body) = part.get_body() {
                        *html = Some(body);
                    }
                }
                _ => {
                    for subpart in &part.subparts {
                        walk(subpart, text, html, atts);
                    }
                }
            }
        }

        walk(&parsed, &mut text_body, &mut html_body, &mut attachments);

        text_body = if !text_body.is_empty() {
            normalize_crlf(&text_body)
        } else if let Some(ref html) = html_body {
            normalize_crlf(html)
        } else {
            normalize_crlf(&String::from_utf8_lossy(body_data))
        };
    } else {
        text_body = normalize_crlf(&String::from_utf8_lossy(body_data));
    }

    (text_body, html_body, attachments)
}

pub fn format_reply_text(original_text: &str, date: &str, sender: &str) -> String {
    let mut reply = format!("\r\n\r\n\r\nOn {}, {} wrote:\r\n", date, sender);
    for line in original_text.lines() {
        reply.push_str("> ");
        reply.push_str(line);
        reply.push_str("\r\n");
    }
    reply
}

pub(crate) fn extract_email(formatted: &str) -> String {
    if let Some(start) = formatted.find('<') {
        if let Some(end_offset) = formatted[start + 1..].find('>') {
            return formatted[start + 1..start + 1 + end_offset]
                .trim()
                .to_string();
        }
    }
    formatted.trim().to_string()
}

pub fn wrap_email_body(text: &str, width: usize) -> String {
    let mut result = String::with_capacity(text.len());

    for line in text.lines() {
        if line.chars().count() <= width {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let mut current_width = 0;
        let mut is_first_word = true;

        for word in line.split(' ') {
            let word_len = word.chars().count();

            if !is_first_word {
                if current_width + word_len + 1 > width {
                    result.push('\n');
                    current_width = 0;
                } else {
                    result.push(' ');
                    current_width += 1;
                }
            }

            result.push_str(word);
            current_width += word_len;
            is_first_word = false;
        }
        result.push('\n');
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
        if trimmed.is_empty() || line.starts_with('>') || is_list_item(trimmed) {
            if !current_paragraph.is_empty() {
                result.push_str(&flow_paragraph_words(&current_paragraph, JUSTIFY_WIDTH));
                current_paragraph.clear();
            }
            result.push_str(line);
            result.push('\n');
        } else {
            current_paragraph.push(line);
        }
    }

    if !current_paragraph.is_empty() {
        result.push_str(&flow_paragraph_words(&current_paragraph, JUSTIFY_WIDTH));
    }

    result
}

fn flow_paragraph_words(lines: &[&str], max_width: usize) -> String {
    let mut reflowed = String::new();
    let mut current_line_len = 0;

    for word in lines.iter().flat_map(|line| line.split_whitespace()) {
        let word_len = word.chars().count();
        let space_needed = usize::from(current_line_len > 0);

        if current_line_len + word_len + space_needed > max_width {
            if current_line_len > 0 {
                reflowed.push('\n');
            }
            reflowed.push_str(word);
            current_line_len = word_len;
        } else {
            if current_line_len > 0 {
                reflowed.push(' ');
            }
            reflowed.push_str(word);
            current_line_len += word_len + space_needed;
        }
    }

    if !reflowed.is_empty() {
        reflowed.push('\n');
    }
    reflowed
}

pub fn build_reply_all_addresses(
    user_email: &str,
    reply_to_or_from: &str,
    original_to: &str,
    original_cc: &str,
) -> (String, String) {
    let user_email_lower = user_email.to_lowercase();
    let mut to_addresses = vec![clean_display_addresses(reply_to_or_from)];
    let mut cc_addresses = Vec::new();
    let mut seen_emails = HashSet::from([extract_email(reply_to_or_from).to_lowercase()]);

    let mut add_addrs = |raw_list: &str, target_vec: &mut Vec<String>| {
        for addr in split_unquoted_commas(raw_list) {
            let extracted = extract_email(&addr).to_lowercase();
            if !extracted.contains(&user_email_lower) && seen_emails.insert(extracted) {
                target_vec.push(clean_single_address(&addr));
            }
        }
    };

    add_addrs(original_to, &mut to_addresses);
    add_addrs(original_cc, &mut cc_addresses);

    (to_addresses.join(", "), cc_addresses.join(", "))
}

pub fn clean_display_addresses(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }

    split_unquoted_commas(raw)
        .into_iter()
        .map(|addr| clean_single_address(&addr))
        .collect::<Vec<_>>()
        .join(", ")
}