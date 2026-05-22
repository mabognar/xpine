use syntect::highlighting::{Theme, ThemeSet};
use crate::editor::Editor;
use crate::config::ConfigExt;
use include_dir::{include_dir, Dir};
use std::fs;
use std::path::PathBuf;

// Embed the entire 'themes' directory from the root of your project workspace
// into your final compiled executable artifact binary
static EMBEDDED_THEMES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/themes");

pub fn is_dark_theme(theme: &Theme) -> bool {
    let bg = theme.settings.background.unwrap_or(syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 });
    let luminance = 0.299 * (bg.r as f32) + 0.587 * (bg.g as f32) + 0.114 * (bg.b as f32);
    luminance < 128.0
}

// Safely guarantees that ~/.xpine/themes is fully populated
pub fn ensure_themes_unpacked() -> std::io::Result<PathBuf> {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let xpine_themes_dir = home.join(".xpine").join("themes");

    // If the folder doesn't exist, create it and extract embedded themes
    if !xpine_themes_dir.exists() {
        fs::create_dir_all(&xpine_themes_dir)?;

        // Extract each embedded theme asset file structure to disk
        for file in EMBEDDED_THEMES_DIR.files() {
            let file_path = xpine_themes_dir.join(file.path());

            // Ensure any inner structural subdirectories exist
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Write binary content of theme to user's local disk environment
            fs::write(file_path, file.contents())?;
        }
    }

    Ok(xpine_themes_dir)
}

pub trait ThemeExt {
    fn load_theme_set() -> (ThemeSet, usize, Option<String>);
}

impl ThemeExt for Editor {
    fn load_theme_set() -> (ThemeSet, usize, Option<String>) {
        let mut theme_set = ThemeSet::load_defaults();
        let mut themes_found = 0;
        let mut error_occurred = None;

        if let Some(theme_dir) = Self::get_theme_dir() {
            if let Ok(custom_themes) = ThemeSet::load_from_folder(&theme_dir) {
                themes_found += custom_themes.themes.len();
                theme_set.themes.extend(custom_themes.themes);
            }
        }

        match ThemeSet::load_from_folder("themes") {
            Ok(custom_themes) => {
                themes_found += custom_themes.themes.len();
                theme_set.themes.extend(custom_themes.themes);
            }
            Err(e) => {
                error_occurred = Some(format!("Local themes not found: {}", e));
            }
        }

        (theme_set, themes_found, error_occurred)
    }
}