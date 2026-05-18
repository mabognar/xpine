use crate::editor::Editor;
use crossterm::style::Color;
use syntect::highlighting::Theme;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use include_dir::{include_dir, Dir};
use std::io::{BufRead, Write};
use crate::syntax::SyntaxExt;

static BUNDLED_THEMES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/themes");

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
    pub ui_bg: Color,
    pub selected_bg: Color,
    pub accent: Color,
    pub date_color: Color,
    pub flag_n: Color,
    pub flag_d: Color,
    pub flag_a: Color,
    pub flag_star: Color,
    pub is_dark: bool,
}

pub fn derive_ui_colors(theme: &Theme) -> UiColors {
    let raw_bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
    let raw_fg = theme.settings.foreground.unwrap_or(syntect::highlighting::Color { r: 255, g: 255, b: 255, a: 255 });

    let bg = Color::Rgb { r: raw_bg.r, g: raw_bg.g, b: raw_bg.b };
    let fg = Color::Rgb { r: raw_fg.r, g: raw_fg.g, b: raw_fg.b };
    let is_dark = (raw_bg.r as u32 + raw_bg.g as u32 + raw_bg.b as u32) < 384;

    let ui_bg = if is_dark {
        Color::Rgb { r: raw_bg.r.saturating_add(20), g: raw_bg.g.saturating_add(20), b: raw_bg.b.saturating_add(20) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(20), g: raw_bg.g.saturating_sub(20), b: raw_bg.b.saturating_sub(20) }
    };

    let selected_bg = if raw_bg.r < 128 {
        Color::Rgb { r: raw_bg.r.saturating_add(40), g: raw_bg.g.saturating_add(40), b: raw_bg.b.saturating_add(40) }
    } else {
        Color::Rgb { r: raw_bg.r.saturating_sub(40), g: raw_bg.g.saturating_sub(40), b: raw_bg.b.saturating_sub(40) }
    };

    let get_theme_color = |keys: &[&str]| -> Option<Color> {
        for item in &theme.scopes {
            let scope_str = format!("{:?}", item.scope).to_lowercase();
            for key in keys {
                if scope_str.contains(key) {
                    if let Some(c) = item.style.foreground {
                        return Some(Color::Rgb { r: c.r, g: c.g, b: c.b });
                    }
                }
            }
        }
        None
    };

    let flag_a = Color::Green;
    let flag_d = Color::Magenta;
    let flag_n = Color::Yellow;
    let flag_star = Color::Red;

    let accent = get_theme_color(&["entity.name.function", "variable"])
        .unwrap_or(if is_dark { Color::Rgb { r: 100, g: 200, b: 255 } } else { Color::Rgb { r: 20, g: 100, b: 180 } });

    let date_color = get_theme_color(&["comment", "punctuation.definition.comment"])
        .unwrap_or(if is_dark { Color::Rgb { r: 120, g: 120, b: 120 } } else { Color::Rgb { r: 140, g: 140, b: 140 } });

    UiColors { bg, fg, ui_bg, selected_bg, accent, date_color, flag_n, flag_d, flag_a, flag_star, is_dark }
}

pub fn load_config() -> AppConfig {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let config_dir = home.join(".xpine");
    let config_path = config_dir.join("xpinerc");
    
    if !config_path.exists() {
        fs::create_dir_all(&config_dir).expect("Failed to create .email directory.");
        // Add the new variables to the template
        let template = "# Account 1\nEMAIL=statgod@gmail.com\nPASSWORD=your_16_char_app_password\nIMAP_SERVER=imap.gmail.com\nIMAP_PORT=993\nSMTP_SERVER=smtp.gmail.com\n\n# Account 2\nEMAIL=second@gmail.com\nPASSWORD=app_password\nIMAP_SERVER=imap.gmail.com\nIMAP_PORT=993\nSMTP_SERVER=smtp.gmail.com\n";
        fs::write(&config_path, template).expect("Failed to write .emailrc template.");

        println!("No configuration found.");
        println!("Created a new config template at: {:?}", config_path);
        println!("Please edit this file with your actual credentials and run the program again.");
        std::process::exit(0);
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

    if accounts.is_empty() || accounts[0].password == "your_16_char_app_password" {
        println!("Invalid or default credentials found in {:?}", config_path);
        std::process::exit(1);
    }

    AppConfig { accounts }
}

pub trait ConfigExt {
    fn get_base_dir() -> Option<PathBuf>;
    fn initialize_themes() -> std::io::Result<()>;
    fn get_config_path() -> Option<PathBuf>;
    fn get_theme_dir() -> Option<PathBuf>;
    fn load_config() -> (String, bool, bool, bool);
    fn save_config(&self);
    fn is_dark_theme(theme: &Theme) -> bool;
    fn derive_ui_color(bg: syntect::highlighting::Color, is_dark: bool) -> Color;
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

    fn initialize_themes() -> std::io::Result<()> {
        // ... (Keep existing implementation)
        if let Some(theme_dir) = Self::get_theme_dir() {
            if fs::read_dir(&theme_dir)?.next().is_none() {
                for file in BUNDLED_THEMES.files() {
                    let path = theme_dir.join(file.path());
                    fs::write(path, file.contents())?;
                }
            }
        }
        Ok(())
    }

    fn get_config_path() -> Option<PathBuf> {
        // Save UI configuration to "settings" to avoid overwriting "xpinerc"
        Self::get_base_dir().map(|p| p.join("settings"))
    }

    fn get_theme_dir() -> Option<PathBuf> {
        // ... (Keep existing implementation)
        Self::get_base_dir().map(|p| {
            let theme_path = p.join("themes");
            let _ = fs::create_dir_all(&theme_path);
            theme_path
        })
    }

    fn load_config() -> (String, bool, bool, bool) {
        // Set Default-Dark as the fallback theme on first install
        let mut theme = String::from("Default-Dark");
        let mut line_numbers = false;
        let mut soft_wrap = false;
        let mut sort_newest_first = false;

        if let Some(path) = Self::get_config_path() {
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
    
    fn save_config(&self) {
        if let Some(path) = Self::get_config_path() {
            let content = format!(
                "theme={}\nline_numbers={}\nsoft_wrap={}\nsort_newest_first={}\n",
                self.current_theme, self.show_line_numbers, self.soft_wrap, self.sort_newest_first
            );
            let _ = fs::write(path, content);
        }
    }

    fn is_dark_theme(theme: &Theme) -> bool {
        let bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
        let luminance = 0.299 * (bg.r as f32) + 0.587 * (bg.g as f32) + 0.114 * (bg.b as f32);
        luminance < 128.0
    }

    fn derive_ui_color(bg: syntect::highlighting::Color, is_dark: bool) -> Color {
        let offset: i16 = if is_dark { 20 } else { -20 };
        let r = (bg.r as i16 + offset).clamp(0, 255) as u8;
        let g = (bg.g as i16 + offset).clamp(0, 255) as u8;
        let b = (bg.b as i16 + offset).clamp(0, 255) as u8;
        Color::Rgb { r, g, b }
    }

    fn cycle_theme(&mut self) {
        let mut themes: Vec<String> = self.theme_set.themes.keys().cloned().collect();
        themes.sort();

        if let Some(current_idx) = themes.iter().position(|t| t == &self.current_theme) {
            let next_idx = (current_idx + 1) % themes.len();
            self.current_theme = themes[next_idx].clone();

            self.save_config();
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

pub fn get_address_book_path() -> PathBuf {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let xpine_dir = home.join(".xpine");
    if !xpine_dir.exists() {
        let _ = fs::create_dir_all(&xpine_dir);
    }
    xpine_dir.join("addressbook")
}

pub fn load_address_book() -> Vec<String> {
    let path = get_address_book_path();
    let mut addresses = Vec::new();

    // 1. Read from the file
    if let Ok(file) = fs::File::open(path) {
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            if let Ok(addr) = line {
                let trimmed = addr.trim().to_string();
                if !trimmed.is_empty() {
                    addresses.push(trimmed);
                }
            }
        }
    }

    // 2. Force the correct custom sort every time it loads
    addresses.sort_by(|a, b| {
        let a_is_team = a.contains(':');
        let b_is_team = b.contains(':');
        if a_is_team == b_is_team {
            a.cmp(b) // Sort alphabetically within their respective groups
        } else if a_is_team {
            std::cmp::Ordering::Greater // Teams go to the bottom
        } else {
            std::cmp::Ordering::Less    // Individuals go to the top
        }
    });

    // 3. Inject the UI spacer line
    if let Some(first_team_idx) = addresses.iter().position(|a| a.contains(':')) {
        if first_team_idx > 0 {
            addresses.insert(first_team_idx, String::new());
        }
    }

    addresses
}

pub fn add_to_address_book(address: &str) -> std::io::Result<bool> {
    let addresses = load_address_book();

    // Check if the address already exists (ignoring whitespace differences)
    if addresses.iter().any(|a| a.trim() == address.trim()) {
        return Ok(false); // Return false indicating it's a duplicate
    }

    let path = get_address_book_path();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    use std::io::Write;
    writeln!(file, "{}", address.trim())?;

    Ok(true) // Return true indicating it was added
}

pub fn save_address_book(addresses: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let path = get_address_book_path();
    let mut file = std::fs::File::create(path)?;
    for addr in addresses {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            writeln!(file, "{}", trimmed)?;
        }
    }
    Ok(())
}
