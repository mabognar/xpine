use std::fs;
use std::io;
use std::path::PathBuf;

use crossterm::style::Color;
use include_dir::{include_dir, Dir};
use syntect::highlighting::{Color as SyntectColor, Theme, ThemeSet};

use crate::config::{ConfigExt, UiColors};
use crate::editor::Editor;

static EMBEDDED_THEMES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/themes");

const DEFAULT_BG: SyntectColor = SyntectColor {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};
const DEFAULT_FG: SyntectColor = SyntectColor {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

pub trait ThemeExt {
    fn load_theme_set() -> (ThemeSet, usize, Option<String>);
}

fn to_crossterm_color(c: SyntectColor) -> Color {
    Color::Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

fn adjust_rgb(c: SyntectColor, delta: u8, lighten: bool) -> Color {
    if lighten {
        Color::Rgb {
            r: c.r.saturating_add(delta),
            g: c.g.saturating_add(delta),
            b: c.b.saturating_add(delta),
        }
    } else {
        Color::Rgb {
            r: c.r.saturating_sub(delta),
            g: c.g.saturating_sub(delta),
            b: c.b.saturating_sub(delta),
        }
    }
}

fn find_scope_color(theme: &Theme, keys: &[&str]) -> Option<Color> {
    theme.scopes.iter().find_map(|item| {
        let fg = item.style.foreground?;
        let scope_str = format!("{:?}", item.scope).to_lowercase();
        keys.iter()
            .any(|key| scope_str.contains(key))
            .then(|| to_crossterm_color(fg))
    })
}

impl ThemeExt for Editor {
    fn load_theme_set() -> (ThemeSet, usize, Option<String>) {
        let mut theme_set = ThemeSet::load_defaults();
        let mut themes_found = 0;
        let mut error_occurred = None;

        let mut merge = |custom: ThemeSet| {
            themes_found += custom.themes.len();
            theme_set.themes.extend(custom.themes);
        };

        if let Some(theme_dir) = Self::get_theme_dir() {
            if let Ok(custom_themes) = ThemeSet::load_from_folder(&theme_dir) {
                merge(custom_themes);
            }
        }

        match ThemeSet::load_from_folder("themes") {
            Ok(custom_themes) => merge(custom_themes),
            Err(e) => error_occurred = Some(format!("Local themes not found: {}", e)),
        }

        (theme_set, themes_found, error_occurred)
    }
}

pub fn derive_ui_colors(theme: &Theme) -> UiColors {
    let raw_bg = theme.settings.background.unwrap_or(DEFAULT_BG);
    let raw_fg = theme.settings.foreground.unwrap_or(DEFAULT_FG);

    let is_dark = (raw_bg.r as u32 + raw_bg.g as u32 + raw_bg.b as u32) < 384;
    
    let mut accent = find_scope_color(theme, &["entity.name.function", "variable"]).unwrap_or(
        if is_dark {
            Color::Rgb {
                r: 100,
                g: 200,
                b: 255,
            }
        } else {
            Color::Rgb {
                r: 20,
                g: 100,
                b: 180,
            }
        },
    );

    if let Some(name) = &theme.name {
        let lower_name = name.to_lowercase();
        if lower_name.contains("catppuccin") {
            accent = if is_dark {
                Color::Rgb { r: 249, g: 226, b: 175 }
            } else {
                Color::Rgb { r: 223, g: 142, b: 29 }
            };
        } else if lower_name.contains("base16") {
            accent = if is_dark {
                Color::Rgb { r: 250, g: 188, b: 45 }
            } else {
                Color::Rgb { r: 200, g: 60, b: 20 }
            };
        }
    }

    let date_color = shift_brightness(raw_fg, 0.20, !is_dark);

    UiColors {
        bg: to_crossterm_color(raw_bg),
        fg: to_crossterm_color(raw_fg),
        menu_bg: adjust_rgb(raw_bg, 20, is_dark),
        selected_bg: adjust_rgb(raw_bg, 40, raw_bg.r < 128),
        accent,
        date_color,
        flag_n: Color::Yellow,
        flag_d: Color::Magenta,
        flag_a: Color::Green,
        flag_star: Color::Red,
        is_dark,
    }
}

pub fn is_dark_theme(theme: &Theme) -> bool {
    let bg = theme.settings.background.unwrap_or(DEFAULT_BG);
    let luminance = 0.299 * (bg.r as f32) + 0.587 * (bg.g as f32) + 0.114 * (bg.b as f32);
    luminance < 128.0
}

pub fn ensure_themes_unpacked() -> io::Result<PathBuf> {
    let home = dirs::home_dir().expect("Could not find home directory.");
    let xpine_themes_dir = home.join(".xpine").join("themes");

    if !xpine_themes_dir.exists() {
        fs::create_dir_all(&xpine_themes_dir)?;

        for file in EMBEDDED_THEMES_DIR.files() {
            let file_path = xpine_themes_dir.join(file.path());
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(file_path, file.contents())?;
        }
    }

    Ok(xpine_themes_dir)
}

fn shift_brightness(c: SyntectColor, percent: f32, lighten: bool) -> Color {
    let shift = |ch: u8| -> u8 {
        let v = ch as f32;
        let adjusted = if lighten {
            v + (255.0 - v) * percent
        } else {
            v * (1.0 - percent)
        };
        adjusted.round().clamp(0.0, 255.0) as u8
    };

    Color::Rgb {
        r: shift(c.r),
        g: shift(c.g),
        b: shift(c.b),
    }
}

