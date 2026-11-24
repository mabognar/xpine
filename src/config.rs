use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use crossterm::style::Color;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::editor::Editor;
use crate::syntax::SyntaxExt;

pub trait ConfigExt {
    fn get_base_dir() -> Option<PathBuf>;
    fn get_settings_path() -> Option<PathBuf>;
    fn get_theme_dir() -> Option<PathBuf>;
    fn load_settings() -> (String, bool, bool, bool, bool, bool);
    fn save_settings(&self);
    fn cycle_theme(&mut self);
    fn update_cursor_color(&self);
}

pub struct ProviderDefaults {
    pub imap: &'static str,
    pub smtp: &'static str,
    pub port: u16,
}

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
    #[serde(default)]
    pub spellcheck_before_send: bool,
    #[serde(default)]
    pub use_dmy_dates: bool,
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

#[derive(Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,
    pub name: Option<String>,
    #[serde(skip)]
    pub password: Option<String>,
    pub client_id: Option<String>,
    #[serde(skip_serializing)]
    pub client_secret: Option<String>,
    #[serde(skip)]
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

#[derive(Serialize, Deserialize, Default)]
struct SecretStore {
    accounts: HashMap<String, SecretData>,
}

#[derive(Serialize, Deserialize)]
struct SecretData {
    password: Option<String>,
    refresh_token: Option<String>,
    client_secret: Option<String>,
}

fn default_imap() -> String {
    "imap.gmail.com".to_string()
}
fn default_imap_port() -> u16 {
    993
}
fn default_smtp() -> String {
    "smtp.gmail.com".to_string()
}
fn default_smtp_port() -> u16 {
    587
}
fn default_theme() -> String {
    "Default-Dark".to_string()
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            soft_wrap: false,
            sort_newest_first: false,
            spellcheck_before_send: false,
            use_dmy_dates: false,
        }
    }
}

fn xpine_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory.")
        .join(".xpine")
}

fn xpine_path(filename: &str) -> PathBuf {
    xpine_dir().join(filename)
}

fn write_private_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
        {
            let _ = file.write_all(bytes);
            return;
        }
    }

    let _ = fs::write(path, bytes);
}

impl ConfigExt for Editor {
    fn get_base_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|home| {
            let path = home.join(".xpine");
            let _ = fs::create_dir_all(&path);
            path
        })
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

    fn load_settings() -> (String, bool, bool, bool, bool, bool) {
        let settings = Self::get_settings_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|content| toml::from_str::<EditorSettings>(&content).ok())
            .unwrap_or_default();

        (
            settings.theme,
            settings.line_numbers,
            settings.soft_wrap,
            settings.sort_newest_first,
            settings.spellcheck_before_send,
            settings.use_dmy_dates,
        )
    }

    fn save_settings(&self) {
        if let Some(path) = Self::get_settings_path() {
            let settings = EditorSettings {
                theme: self.current_theme.clone(),
                line_numbers: self.show_line_numbers,
                soft_wrap: self.soft_wrap,
                sort_newest_first: self.sort_newest_first,
                spellcheck_before_send: self.spellcheck_before_send,
                use_dmy_dates: self.use_dmy_dates,
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
            self.current_theme = themes[(current_idx + 1) % themes.len()].clone();
            self.save_settings();
            self.clear_cache();

            self.status_message = format!("Theme changed to: {}", self.current_theme);
            self.status_time = Some(Instant::now());
            self.update_cursor_color();
        }
    }

    fn update_cursor_color(&self) {
        print!("\x1b]12;#888888\x07");
        let _ = std::io::stdout().flush();
    }
}

pub fn load_signature() -> String {
    fs::read_to_string(xpine_path("signature")).unwrap_or_default()
}

pub fn save_signature(sig: &str) {
    let _ = fs::create_dir_all(xpine_dir());
    let _ = fs::write(xpine_path("signature"), sig);
}

pub fn get_provider_defaults(email: &str) -> Option<ProviderDefaults> {
    let lower = email.trim().to_lowercase();
    if lower.ends_with("@gmail.com") {
        Some(ProviderDefaults {
            imap: "imap.gmail.com",
            smtp: "smtp.gmail.com",
            port: 993,
        })
    } else if lower.ends_with("@yahoo.com") {
        Some(ProviderDefaults {
            imap: "imap.mail.yahoo.com",
            smtp: "smtp.mail.yahoo.com",
            port: 993,
        })
    } else {
        None
    }
}

fn get_or_create_key() -> Key<Aes256Gcm> {
    let key_path = xpine_path(".master.key");

    if let Ok(key_bytes) = fs::read(&key_path) {
        if key_bytes.len() == 32 {
            return *Key::<Aes256Gcm>::from_slice(&key_bytes);
        }
    }

    let key = Aes256Gcm::generate_key(OsRng);
    write_private_file(&key_path, &key);
    key
}

fn load_secrets() -> SecretStore {
    let Ok(encrypted_data) = fs::read(xpine_path("secrets.enc")) else {
        return SecretStore::default();
    };
    if encrypted_data.len() < 12 {
        return SecretStore::default();
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let cipher = Aes256Gcm::new(&get_or_create_key());
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .ok()
        .and_then(|plaintext| serde_json::from_slice(&plaintext).ok())
        .unwrap_or_default()
}

fn save_secrets(store: &SecretStore) {
    let plaintext = serde_json::to_vec(store).unwrap_or_default();
    let cipher = Aes256Gcm::new(&get_or_create_key());

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    if let Ok(ciphertext) = cipher.encrypt(nonce, plaintext.as_ref()) {
        let mut output = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        write_private_file(&xpine_path("secrets.enc"), &output);
    }
}

pub fn load_config() -> AppConfig {
    let config_dir = xpine_dir();
    let config_path = config_dir.join("xpinerc");

    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
        fs::write(&config_path, "[[accounts]]\nemail = \"\"\n").expect("Failed to write xpinerc.");
        return AppConfig::default();
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");
    let mut config: AppConfig = toml::from_str(&contents).unwrap_or_else(|e| {
        eprintln!("Failed to parse xpinerc: {}", e);
        AppConfig::default()
    });

    let secrets = load_secrets();
    let mut needs_migration = false;

    for account in &mut config.accounts {
        let mut vault_has_secret = false;

        if let Some(secret_data) = secrets.accounts.get(&account.email) {
            account.password = secret_data.password.clone();
            account.refresh_token = secret_data.refresh_token.clone();

            if secret_data.client_secret.is_some() {
                account.client_secret = secret_data.client_secret.clone();
                vault_has_secret = true;
            }
        }

        if account.client_secret.is_some() && !vault_has_secret {
            needs_migration = true;
        }
    }

    if needs_migration {
        save_config(&config.accounts);
    }

    config
}

pub fn save_config(accounts: &[Account]) {
    let config_dir = xpine_dir();
    let _ = fs::create_dir_all(&config_dir);

    let config = AppConfig {
        accounts: accounts.to_vec(),
    };
    if let Ok(toml_string) = toml::to_string_pretty(&config) {
        fs::write(config_dir.join("xpinerc"), toml_string).expect("Failed to write config file.");
    }

    let mut secrets = SecretStore::default();
    for account in accounts {
        let has_secret = account.password.is_some()
            || account.refresh_token.is_some()
            || account.client_secret.is_some();
        if !account.email.is_empty() && has_secret {
            secrets.accounts.insert(
                account.email.clone(),
                SecretData {
                    password: account.password.clone(),
                    refresh_token: account.refresh_token.clone(),
                    client_secret: account.client_secret.clone(),
                },
            );
        }
    }
    save_secrets(&secrets);
}