use crate::editor::Editor;
use crossterm::style::Color;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use crate::syntax::SyntaxExt;

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

#[derive(Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,

    // Standard Auth
    pub password: Option<String>,

    // Google OAuth 2.0
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub refresh_token: Option<String>,

    #[serde(default = "default_imap")]
    pub imap_server: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default = "default_smtp")]
    pub smtp_server: String,

    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
}

fn default_imap() -> String { "imap.gmail.com".to_string() }
fn default_imap_port() -> u16 { 993 }
fn default_smtp() -> String { "smtp.gmail.com".to_string() }
fn default_smtp_port() -> u16 { 587 }

#[derive(Deserialize, Serialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Deserialize, Serialize)]
pub struct EditorSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub line_numbers: bool,
    #[serde(default)]
    pub soft_wrap: bool,
    #[serde(default)]
    pub sort_newest_first: bool,
}

fn default_theme() -> String { "Default-Dark".to_string() }

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            soft_wrap: false,
            sort_newest_first: false,
        }
    }
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
        // Create an empty file with a TOML template so it can be written to later
        let template = "[[accounts]]\nemail = \"\"\npassword = \"\"\n";
        fs::write(&config_path, template).expect("Failed to write xpinerc.");
        return AppConfig { accounts: Vec::new() };
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");

    match toml::from_str(&contents) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to parse xpinerc: {}", e);
            AppConfig { accounts: Vec::new() }
        }
    }
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
        let default_settings = EditorSettings::default();

        if let Some(path) = Self::get_settings_path() {
            if let Ok(content) = fs::read_to_string(path) {
                // Parse the TOML, falling back to defaults if parsing fails
                if let Ok(settings) = toml::from_str::<EditorSettings>(&content) {
                    return (
                        settings.theme,
                        settings.line_numbers,
                        settings.soft_wrap,
                        settings.sort_newest_first
                    );
                }
            }
        }

        // Return defaults if the file doesn't exist or couldn't be parsed
        (
            default_settings.theme,
            default_settings.line_numbers,
            default_settings.soft_wrap,
            default_settings.sort_newest_first
        )
    }

    fn save_settings(&self) {
        if let Some(path) = Self::get_settings_path() {
            let settings = EditorSettings {
                theme: self.current_theme.clone(),
                line_numbers: self.show_line_numbers,
                soft_wrap: self.soft_wrap,
                sort_newest_first: self.sort_newest_first,
            };

            if let Ok(toml_string) = toml::to_string_pretty(&settings) {
                let _ = fs::write(path, toml_string);
            }
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

    let config = AppConfig { accounts: accounts.to_vec() };
    if let Ok(toml_string) = toml::to_string_pretty(&config) {
        std::fs::write(config_path, toml_string).expect("Failed to write config file.");
    }
}
