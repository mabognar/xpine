use crate::editor::Editor;
use crossterm::style::Color;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use crate::syntax::SyntaxExt;
use std::fs::OpenOptions;
use std::io::Write;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce
};
use rand::{rngs::OsRng, RngCore};
use std::collections::HashMap;

pub struct ProviderDefaults {
    pub imap: &'static str,
    pub smtp: &'static str,
    pub port: u16,
}

pub fn get_provider_defaults(email: &str) -> Option<ProviderDefaults> {
    if email.ends_with("@gmail.com") {
        Some(ProviderDefaults { imap: "imap.gmail.com", smtp: "smtp.gmail.com", port: 993 })
    } else if email.ends_with("@yahoo.com") {
        Some(ProviderDefaults { imap: "imap.mail.yahoo.com", smtp: "smtp.mail.yahoo.com", port: 993 })
    } else {
        None
    }
}

// #[derive(Clone, Deserialize, Serialize)]
// pub struct Account {
//     pub email: String,
//
//     // Standard Auth
//     #[serde(skip_serializing)]
//     pub password: Option<String>,
//
//     // Google OAuth 2.0
//     pub client_id: Option<String>,
//     pub client_secret: Option<String>,
//
//     #[serde(skip_serializing)]
//     pub refresh_token: Option<String>,
//
//     #[serde(default = "default_imap")]
//     pub imap_server: String,
//     #[serde(default = "default_imap_port")]
//     pub imap_port: u16,
//     #[serde(default = "default_smtp")]
//     pub smtp_server: String,
//
//     #[serde(default = "default_smtp_port")]
//     pub smtp_port: u16,
// }

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
    #[serde(default)]
    pub spellcheck_before_send: bool,
}

fn default_theme() -> String { "Default-Dark".to_string() }

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            line_numbers: false,
            soft_wrap: false,
            sort_newest_first: false,
            spellcheck_before_send: false,
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


// --- 1. THE UPDATED STRUCT ---
#[derive(Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,

    // Serde entirely ignores these fields for xpinerc TOML operations
    #[serde(skip)]
    pub password: Option<String>,

    pub client_id: Option<String>,

    #[serde(skip_serializing)]
    pub client_secret: Option<String>,

    // Serde entirely ignores these fields for xpinerc TOML operations
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

// --- 2. ENCRYPTION ENGINE ---

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

fn get_or_create_key() -> Key<Aes256Gcm> {
    let key_path = dirs::home_dir().unwrap().join(".xpine").join(".master.key");

    // If the key exists, read it
    if key_path.exists() {
        if let Ok(key_bytes) = fs::read(&key_path) {
            if key_bytes.len() == 32 {
                return *Key::<Aes256Gcm>::from_slice(&key_bytes);
            }
        }
    }

    // Otherwise, generate a secure random 32-byte key
    let key = Aes256Gcm::generate_key(OsRng);

    // Save it securely. On macOS/Linux, enforce strict 0600 permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        if let Ok(mut file) = options.open(&key_path) {
            use std::io::Write;
            let _ = file.write_all(&key);
        } else {
            let _ = fs::write(&key_path, &key); // Fallback
        }
    }
    #[cfg(not(unix))]
    {
        let _ = fs::write(&key_path, &key);
    }

    key
}

fn load_secrets() -> SecretStore {
    let path = dirs::home_dir().unwrap().join(".xpine").join("secrets.enc");
    if !path.exists() {
        return SecretStore::default();
    }

    let encrypted_data = fs::read(path).unwrap_or_default();
    if encrypted_data.len() < 12 { return SecretStore::default(); }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let key = get_or_create_key();
    let cipher = Aes256Gcm::new(&key);
    let nonce = Nonce::from_slice(nonce_bytes);

    match cipher.decrypt(nonce, ciphertext) {
        Ok(plaintext) => serde_json::from_slice(&plaintext).unwrap_or_default(),
        Err(_) => SecretStore::default(), // If decryption fails, start fresh
    }
}

fn save_secrets(store: &SecretStore) {
    let path = dirs::home_dir().unwrap().join(".xpine").join("secrets.enc");
    let plaintext = serde_json::to_vec(store).unwrap_or_default();

    let key = get_or_create_key();
    let cipher = Aes256Gcm::new(&key);

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    if let Ok(ciphertext) = cipher.encrypt(nonce, plaintext.as_ref()) {
        let mut output = nonce_bytes.to_vec();
        output.extend_from_slice(&ciphertext);
        let _ = fs::write(path, output);
    }
}

// --- 3. THE REWRITTEN SAVE/LOAD FUNCTIONS ---

pub fn load_config() -> AppConfig {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_dir = home.join(".xpine");
    let config_path = config_dir.join("xpinerc");

    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
        let template = "[[accounts]]\nemail = \"\"\n";
        fs::write(&config_path, template).expect("Failed to write xpinerc.");
        return AppConfig { accounts: Vec::new() };
    }

    let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");

    let mut config: AppConfig = toml::from_str(&contents).unwrap_or_else(|e| {
        eprintln!("Failed to parse xpinerc: {}", e);
        AppConfig { accounts: Vec::new() }
    });

    // Merge the encrypted secrets back into the config in memory
    let secrets = load_secrets();
    let mut needs_migration = false; // <--- NEW

    for account in &mut config.accounts {
        let mut vault_has_secret = false;

        if let Some(secret_data) = secrets.accounts.get(&account.email) {
            account.password = secret_data.password.clone();
            account.refresh_token = secret_data.refresh_token.clone();

            // Override TOML if the vault has the secret
            if secret_data.client_secret.is_some() {
                account.client_secret = secret_data.client_secret.clone();
                vault_has_secret = true;
            }
        }

        // If TOML had a cleartext secret, but the secure vault didn't, trigger a migration
        if account.client_secret.is_some() && !vault_has_secret {
            needs_migration = true;
        }
    }

    // Rewrite the files immediately to secure the secret and scrub xpinerc
    if needs_migration {
        save_config(&config.accounts);
    }

    config
}
//     let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");
//
//     let mut config: AppConfig = toml::from_str(&contents).unwrap_or_else(|e| {
//         eprintln!("Failed to parse xpinerc: {}", e);
//         AppConfig { accounts: Vec::new() }
//     });
//
//     // Merge the encrypted secrets back into the config in memory
//     let secrets = load_secrets();
//     for account in &mut config.accounts {
//         if let Some(secret_data) = secrets.accounts.get(&account.email) {
//             account.password = secret_data.password.clone();
//             account.refresh_token = secret_data.refresh_token.clone();
//         }
//     }
//
//     config
// }

pub fn save_config(accounts: &[Account]) {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_path = home.join(".xpine").join("xpinerc");

    // 1. Write the safe TOML file (#[serde(skip)] ensures no passwords go here)
    let config = AppConfig { accounts: accounts.to_vec() };
    if let Ok(toml_string) = toml::to_string_pretty(&config) {
        fs::write(config_path, toml_string).expect("Failed to write config file.");
    }

    // 2. Encrypt and save the passwords to the secure binary file
    // 2. Encrypt and save the passwords to the secure binary file
    let mut secrets = SecretStore::default();
    for account in accounts {
        if !account.email.is_empty() && (account.password.is_some() || account.refresh_token.is_some() || account.client_secret.is_some()) {
            secrets.accounts.insert(
                account.email.clone(),
                SecretData {
                    password: account.password.clone(),
                    refresh_token: account.refresh_token.clone(),
                    client_secret: account.client_secret.clone(), // <--- NEW
                }
            );
        }
    }
    save_secrets(&secrets);
    // let mut secrets = SecretStore::default();
    // for account in accounts {
    //     if !account.email.is_empty() && (account.password.is_some() || account.refresh_token.is_some()) {
    //         secrets.accounts.insert(
    //             account.email.clone(),
    //             SecretData {
    //                 password: account.password.clone(),
    //                 refresh_token: account.refresh_token.clone(),
    //             }
    //         );
    //     }
    // }
    // save_secrets(&secrets);
}

// pub fn load_config() -> AppConfig {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_dir = home.join(".xpine");
//     let config_path = config_dir.join("xpinerc");
//
//     if !config_path.exists() {
//         fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
//         let template = "[[accounts]]\nemail = \"\"\n"; // Removed password from template
//         fs::write(&config_path, template).expect("Failed to write xpinerc.");
//         return AppConfig { accounts: Vec::new() };
//     }
//
//     let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");
//
//     let mut config: AppConfig = toml::from_str(&contents).unwrap_or_else(|e| {
//         AppConfig { accounts: Vec::new() }
//     });
//
//     let mut needs_migration = false;
//
//     for account in &mut config.accounts {
//         if !account.email.is_empty() {
//             // --- Migrate or Load Password ---
//             if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                 if let Ok(saved_pw) = entry.get_password() {
//                     // It's in the Keychain, use it!
//                     account.password = Some(saved_pw);
//                 } else if let Some(plaintext_pw) = &account.password {
//                     // Not in Keychain, but found in xpinerc. Migrate it!
//                     if entry.set_password(plaintext_pw).is_ok() {
//                         needs_migration = true;
//                     }
//                 }
//             }
//
//             // --- Migrate or Load OAuth Token ---
//             if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                 if let Ok(saved_token) = entry.get_password() {
//                     account.refresh_token = Some(saved_token);
//                 } else if let Some(plaintext_token) = &account.refresh_token {
//                     if entry.set_password(plaintext_token).is_ok() {
//                         needs_migration = true;
//                     }
//                 }
//             }
//         }
//     }
//
//     // Rewrite the xpinerc file immediately to strip the newly secured passwords
//     if needs_migration {
//         save_config(&config.accounts);
//     }
//
//     config
// }

// pub fn load_config() -> AppConfig {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_dir = home.join(".xpine");
//     let config_path = config_dir.join("xpinerc");
//
//     if !config_path.exists() {
//         fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
//         let template = "[[accounts]]\nemail = \"\"\n"; // Removed password from template
//         fs::write(&config_path, template).expect("Failed to write xpinerc.");
//         return AppConfig { accounts: Vec::new() };
//     }
//
//     let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");
//
//     let mut config: AppConfig = toml::from_str(&contents).unwrap_or_else(|e| {
//         eprintln!("Failed to parse xpinerc: {}", e);
//         AppConfig { accounts: Vec::new() }
//     });
//
//     // --- NEW: Populate secrets from the native OS Keyring ---
//     for account in &mut config.accounts {
//         if !account.email.is_empty() {
//             // Attempt to load standard password
//             if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                 if let Ok(pw) = entry.get_password() {
//                     account.password = Some(pw);
//                 }
//             }
//
//             // Attempt to load OAuth refresh token
//             if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                 if let Ok(token) = entry.get_password() {
//                     account.refresh_token = Some(token);
//                 }
//             }
//         }
//     }
//
//     config
// }

// pub fn load_config() -> AppConfig {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_dir = home.join(".xpine");
//     let config_path = config_dir.join("xpinerc");
//
//     if !config_path.exists() {
//         fs::create_dir_all(&config_dir).expect("Failed to create .xpine directory.");
//         // Create an empty file with a TOML template so it can be written to later
//         let template = "[[accounts]]\nemail = \"\"\npassword = \"\"\n";
//         fs::write(&config_path, template).expect("Failed to write xpinerc.");
//         return AppConfig { accounts: Vec::new() };
//     }
//
//     let contents = fs::read_to_string(&config_path).expect("Failed to read xpinerc");
//
//     toml::from_str(&contents).unwrap_or_else(|e| {
//         eprintln!("Failed to parse xpinerc: {}", e);
//         AppConfig { accounts: Vec::new() }
//     })
// }

pub trait ConfigExt {
    fn get_base_dir() -> Option<PathBuf>;
    fn get_settings_path() -> Option<PathBuf>;
    fn get_theme_dir() -> Option<PathBuf>;
    fn load_settings() -> (String, bool, bool, bool, bool);
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

    fn load_settings() -> (String, bool, bool, bool, bool) {
        let default_settings = EditorSettings::default();

        if let Some(path) = Self::get_settings_path() {
            if let Ok(content) = fs::read_to_string(path) {
                // Parse the TOML, falling back to defaults if parsing fails
                if let Ok(settings) = toml::from_str::<EditorSettings>(&content) {
                    return (
                        settings.theme,
                        settings.line_numbers,
                        settings.soft_wrap,
                        settings.sort_newest_first,
                        settings.spellcheck_before_send,
                    );
                }
            }
        }

        // Return defaults if the file doesn't exist or couldn't be parsed
        (
            default_settings.theme,
            default_settings.line_numbers,
            default_settings.soft_wrap,
            default_settings.sort_newest_first,
            default_settings.spellcheck_before_send,
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

fn log_keyring_error(msg: &str) {
    let home = dirs::home_dir().unwrap();
    let log_path = home.join(".xpine").join("keyring_debug.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{}\n", msg);
    }
}

// pub fn save_config(accounts: &[Account]) {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_path = home.join(".xpine").join("xpinerc");
//
//     let config = AppConfig { accounts: accounts.to_vec() };
//
//     if let Ok(toml_string) = toml::to_string_pretty(&config) {
//         let _ = fs::write(config_path, toml_string);
//     }
//
//     for account in accounts {
//         if !account.email.is_empty() {
//             if let Some(pw) = &account.password {
//                 if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                     if let Err(e) = entry.set_password(pw) {
//                         log_keyring_error(&format!("Failed to save password for {}: {:?}", account.email, e));
//                     }
//                 }
//             }
//
//             if let Some(token) = &account.refresh_token {
//                 if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                     if let Err(e) = entry.set_password(token) {
//                         log_keyring_error(&format!("Failed to save token for {}: {:?}", account.email, e));
//                     }
//                 }
//             }
//         }
//     }
// }

// pub fn save_config(accounts: &[Account]) {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_path = home.join(".xpine").join("xpinerc");
//
//     let config = AppConfig { accounts: accounts.to_vec() };
//
//     if let Ok(toml_string) = toml::to_string_pretty(&config) {
//         fs::write(config_path, toml_string).expect("Failed to write config file.");
//     }
//
//     for account in accounts {
//         if !account.email.is_empty() {
//             // ONLY save if we have a value. Never delete automatically.
//             if let Some(pw) = &account.password {
//                 if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                     let _ = entry.set_password(pw);
//                 }
//             }
//
//             if let Some(token) = &account.refresh_token {
//                 if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                     let _ = entry.set_password(token);
//                 }
//             }
//         }
//     }
// }

// pub fn save_config(accounts: &[Account]) {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_path = home.join(".xpine").join("xpinerc");
//
//     let config = AppConfig { accounts: accounts.to_vec() };
//
//     // This will only write non-skipped fields to xpinerc
//     if let Ok(toml_string) = toml::to_string_pretty(&config) {
//         fs::write(config_path, toml_string).expect("Failed to write config file.");
//     }
//
//     // --- NEW: Save secrets directly to the native OS Keyring ---
//     for account in accounts {
//         if !account.email.is_empty() {
//             // Save standard password
//             if let Some(pw) = &account.password {
//                 if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                     let _ = entry.set_password(pw);
//                 }
//             } else {
//                 // Optional: Clear it from the keyring if it was removed in the app
//                 if let Ok(entry) = Entry::new("xpine_password", &account.email) {
//                     let _ = entry.delete_credential();
//                 }
//             }
//
//             // Save OAuth refresh token
//             if let Some(token) = &account.refresh_token {
//                 if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                     let _ = entry.set_password(token);
//                 }
//             } else {
//                 if let Ok(entry) = Entry::new("xpine_refresh_token", &account.email) {
//                     let _ = entry.delete_credential();
//                 }
//             }
//         }
//     }
// }

// pub fn save_config(accounts: &[Account]) {
//     let home = dirs::home_dir().expect("Could not find home directory.");
//     let config_path = home.join(".xpine").join("xpinerc");
//
//     let config = AppConfig { accounts: accounts.to_vec() };
//     if let Ok(toml_string) = toml::to_string_pretty(&config) {
//         fs::write(config_path, toml_string).expect("Failed to write config file.");
//     }
// }

pub fn load_signature() -> String {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let path = home.join(".xpine").join("signature");
    std::fs::read_to_string(path).unwrap_or_default()
}

pub fn save_signature(sig: &str) {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let path = home.join(".xpine").join("signature");
    let _ = std::fs::write(path, sig);
}
