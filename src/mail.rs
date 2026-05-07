use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Account {
    pub email: String,
    pub password: String,
}

pub struct AppConfig {
    pub accounts: Vec<Account>,
}

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

pub fn open_in_default_app(file_path: &Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", file_path.to_str().unwrap()]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(file_path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(file_path).spawn();
}

pub fn load_config() -> AppConfig {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_dir = home.join(".email");
    let config_path = config_dir.join(".emailrc");

    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .email directory.");
        let template = "# Account 1\nEMAIL=statgod@gmail.com\nPASSWORD=your_16_char_app_password\n\n# Account 2\nEMAIL=second@gmail.com\nPASSWORD=app_password\n";
        fs::write(&config_path, template).expect("Failed to write .emailrc template.");

        println!("No configuration found.");
        println!("Created a new config template at: {:?}", config_path);
        println!("Please edit this file with your actual App Password(s) and run the program again.");
        std::process::exit(0);
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read .emailrc");
    let mut accounts = Vec::new();
    let mut current_email = String::new();

    for line in contents.lines() {
        if line.trim().is_empty() || line.starts_with('#') { continue; }
        if let Some((key, value)) = line.split_once('=') {
            let val = value.trim().to_string();
            match key.trim().to_uppercase().as_str() {
                "EMAIL" => current_email = val,
                "PASSWORD" => {
                    if !current_email.is_empty() && !val.is_empty() {
                        accounts.push(Account { email: current_email.clone(), password: val });
                        current_email.clear();
                    }
                }
                _ => {}
            }
        }
    }

    if accounts.is_empty() || accounts[0].password == "your_16_char_app_password" {
        println!("Invalid or default credentials found in {:?}", config_path);
        std::process::exit(1);
    }

    AppConfig { accounts }
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
            text_body = "[This message only contains an HTML body. Press ^B to view it in your browser.]\r\n".to_string();
        } else if !text_body.is_empty() {
            text_body = text_body.replace('\n', "\r\n");
        } else {
            text_body = String::from_utf8_lossy(body_data).replace('\n', "\r\n");
        }
    } else {
        let raw = String::from_utf8_lossy(body_data);
        text_body = raw.replace('\n', "\r\n");
    }

    (text_body, html_body, attachments)
}
