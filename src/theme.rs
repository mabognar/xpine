use syntect::highlighting::{Theme, ThemeSet};
use crate::editor::Editor;
use crate::config::{ConfigExt, UiColors};
use include_dir::{include_dir, Dir};
use std::fs;
use std::path::PathBuf;
use crossterm::style::Color;

// Embed the entire 'themes' directory from the root of your project workspace
// into the final compiled executable artifact binary
static EMBEDDED_THEMES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/themes");

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

    UiColors { bg, fg, menu_bg: ui_bg, selected_bg, accent, date_color, flag_n, flag_d, flag_a, flag_star, is_dark }
}


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


