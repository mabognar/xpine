use crate::editor::Editor;
use crossterm::style::Color;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::io::{BufRead, Write};
use crate::syntax::SyntaxExt;

// In a new file or in your config.rs
pub struct ProviderDefaults {
    pub imap: &'static str,
    pub smtp: &'static str,
    pub port: u16,
}

pub fn get_provider_defaults(email: &str) -> Option<ProviderDefaults> {
    if email.ends_with("@gmail.com") {
        Some(ProviderDefaults { imap: "imap.gmail.com", smtp: "smtp.gmail.com", port: 993 })
    // } else if email.ends_with("@outlook.com") || email.ends_with("@hotmail.com") {
    //     Some(ProviderDefaults { imap: "outlook.office365.com", smtp: "smtp.office365.com", port: 993 })
    } else if email.ends_with("@yahoo.com") {
        Some(ProviderDefaults { imap: "imap.mail.yahoo.com", smtp: "smtp.mail.yahoo.com", port: 993 })
    } else {
        None
    }
}


#[derive(Clone)]
pub struct Account {
    pub email: String,
    pub password: String,
    pub imap_server: String,
    pub imap_port: u16,
    pub smtp_server: String,
}

pub struct AppConfig {
    pub accounts: Vec<Account>,
}

#[derive(Clone, Copy)]
pub struct UiColors {
    pub bg: Color,
    pub fg: Color,
    pub menu_bg: Color,
    pub selected_bg: Color,
    pub accent: Color,
    pub date_color: Color,
    pub flag_n: Color,
    pub flag_d: Color,
    pub flag_a: Color,
    pub flag_star: Color,
    pub is_dark: bool,
}

pub fn load_config() -> AppConfig {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_dir = home.join(".xpine");
    let config_path = config_dir.join("xpinerc");

    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
        // Create an empty file so it can be written to later
        fs::write(&config_path, "").expect("Failed to write xpinerc.");
        return AppConfig { accounts: Vec::new() };
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read .emailrc");
    let mut accounts = Vec::new();

    // Set fallback defaults in case an existing user's file is missing these fields
    let mut current_email = String::new();
    let mut current_password = String::new();
    let mut current_imap_server = String::from("imap.gmail.com");
    let mut current_imap_port = 993;
    let mut current_smtp_server = String::from("smtp.gmail.com");

    for line in contents.lines() {
        if line.trim().is_empty() || line.starts_with('#') { continue; }
        if let Some((key, value)) = line.split_once('=') {
            let val = value.trim().to_string();
            match key.trim().to_uppercase().as_str() {
                "EMAIL" => {
                    // Push the previous account when we hit a new EMAIL line
                    if !current_email.is_empty() && !current_password.is_empty() {
                        accounts.push(Account {
                            email: current_email.clone(), password: current_password.clone(),
                            imap_server: current_imap_server.clone(), imap_port: current_imap_port, smtp_server: current_smtp_server.clone(),
                        });
                        // Reset defaults for the next account block
                        current_password.clear();
                        current_imap_server = String::from("imap.gmail.com");
                        current_imap_port = 993;
                        current_smtp_server = String::from("smtp.gmail.com");
                    }
                    current_email = val;
                }
                "PASSWORD" => current_password = val,
                "IMAP_SERVER" => current_imap_server = val,
                "IMAP_PORT" => if let Ok(p) = val.parse() { current_imap_port = p },
                "SMTP_SERVER" => current_smtp_server = val,
                _ => {}
            }
        }
    }

    // Push the final account at the end of the file
    if !current_email.is_empty() && !current_password.is_empty() {
        accounts.push(Account {
            email: current_email, password: current_password,
            imap_server: current_imap_server, imap_port: current_imap_port, smtp_server: current_smtp_server,
        });
    }

    // if accounts.is_empty() || accounts[0].password == "your_16_char_app_password" {
    //     println!("Invalid or default credentials found in {:?}", config_path);
    //     std::process::exit(1);
    // }

    AppConfig { accounts }
}

pub trait ConfigExt {
    fn get_base_dir() -> Option<PathBuf>;
    fn get_settings_path() -> Option<PathBuf>;
    fn get_theme_dir() -> Option<PathBuf>;
    fn load_settings() -> (String, bool, bool, bool);
    fn save_settings(&self);
    fn cycle_theme(&mut self);
    fn update_cursor_color(&self);
}

impl ConfigExt for Editor {
    fn get_base_dir() -> Option<PathBuf> {
        let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).unwrap_or_default();
        if home.is_empty() {
            None
        } else {
            // Update to use the .xpine directory
            let path = Path::new(&home).join(".xpine");
            let _ = fs::create_dir_all(&path);
            Some(path)
        }
    }

    fn get_settings_path() -> Option<PathBuf> {
        Self::get_base_dir().map(|p| p.join("settings"))
    }

    fn get_theme_dir() -> Option<PathBuf> {
        Self::get_base_dir().map(|p| {
            let theme_path = p.join("themes");
            let _ = fs::create_dir_all(&theme_path);
            theme_path
        })
    }

    fn load_settings() -> (String, bool, bool, bool) {
        let mut theme = String::from("Default-Dark");
        let mut line_numbers = false;
        let mut soft_wrap = false;
        let mut sort_newest_first = false;

        if let Some(path) = Self::get_settings_path() {
            if let Ok(content) = fs::read_to_string(path) {
                for line in content.lines() {
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        match parts[0] {
                            "theme" => theme = parts[1].to_string(),
                            "line_numbers" => line_numbers = parts[1] == "true",
                            "soft_wrap" => soft_wrap = parts[1] == "true",
                            "sort_newest_first" => sort_newest_first = parts[1] == "true",
                            _ => {}
                        }
                    }
                }
            }
        }
        (theme, line_numbers, soft_wrap, sort_newest_first)
    }

    fn save_settings(&self) {
        if let Some(path) = Self::get_settings_path() {
            let content = format!(
                "theme={}\nline_numbers={}\nsoft_wrap={}\nsort_newest_first={}\n",
                self.current_theme, self.show_line_numbers, self.soft_wrap, self.sort_newest_first
            );
            let _ = fs::write(path, content);
        }
    }

    fn cycle_theme(&mut self) {
        let mut themes: Vec<String> = self.theme_set.themes.keys().cloned().collect();
        themes.sort();

        if let Some(current_idx) = themes.iter().position(|t| t == &self.current_theme) {
            let next_idx = (current_idx + 1) % themes.len();
            self.current_theme = themes[next_idx].clone();

            self.save_settings();
            self.clear_cache();

            self.status_message = format!("Theme changed to: {}", self.current_theme);
            self.status_time = Some(std::time::Instant::now());

            self.update_cursor_color();
        }
    }

    fn update_cursor_color(&self) {
        print!("\x1b]12;#888888\x07");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

pub fn save_config(accounts: &[Account]) {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_path = home.join(".xpine").join("xpinerc");

    let mut out = String::new();
    for (i, acc) in accounts.iter().enumerate() {
        out.push_str(&format!("# Account {}\n", i + 1));
        out.push_str(&format!("EMAIL={}\n", acc.email));
        out.push_str(&format!("PASSWORD={}\n", acc.password));
        out.push_str(&format!("IMAP_SERVER={}\n", acc.imap_server));
        out.push_str(&format!("IMAP_PORT={}\n", acc.imap_port));
        out.push_str(&format!("SMTP_SERVER={}\n", acc.smtp_server));
        out.push('\n');
    }

    std::fs::write(config_path, out).expect("Failed to write config file.");
}
