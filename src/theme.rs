use syntect::highlighting::ThemeSet;
use crate::editor::Editor;
use crate::config::ConfigExt;

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