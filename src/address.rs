use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

fn has_display_name(addr: &str) -> bool {
    addr.contains('<') && addr.contains('>')
}

fn sort_and_separate_teams(addresses: &mut Vec<String>) {
    addresses.sort_by(|a, b| {
        a.contains(':')
            .cmp(&b.contains(':'))
            .then_with(|| a.cmp(b))
    });

    if let Some(first_team_idx) = addresses.iter().position(|a| a.contains(':')) {
        if first_team_idx > 0 {
            addresses.insert(first_team_idx, String::new());
        }
    }
}

fn format_team_entry(team_name: &str, emails: &str, address_book: &[String]) -> String {
    let expanded_emails = expand_address_lists(emails, address_book);
    format!(
        "{}: {};",
        team_name.trim(),
        expanded_emails.trim_end_matches(';')
    )
}

fn split_unquoted_commas(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in input.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        if c == ',' && !in_quotes {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

pub fn enforce_quotes(address: &str) -> String {
    let trimmed = address.trim();
    if let Some(start) = trimmed.find('<') {
        let name_part = trimmed[..start].trim();
        if name_part.is_empty() || (name_part.starts_with('"') && name_part.ends_with('"')) {
            return trimmed.to_string();
        }

        let clean_name = name_part.trim_matches('"');
        return format!("\"{}\" {}", clean_name, &trimmed[start..]);
    }
    trimmed.to_string()
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
    let mut addresses = File::open(get_address_book_path())
        .map(|file| {
            BufReader::new(file)
                .lines()
                .flatten()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect()
        })
        .unwrap_or_default();

    sort_and_separate_teams(&mut addresses);
    addresses
}

pub fn add_to_address_book(address: &str) -> io::Result<bool> {
    let mut addresses = load_address_book();
    let trimmed = address.trim();

    let final_address = if let Some((team_name, emails)) = trimmed.split_once(':') {
        let formatted = format_team_entry(team_name, emails, &addresses);
        if addresses.iter().any(|a| a.trim() == formatted) {
            return Ok(false);
        }
        formatted
    } else {
        let quoted = enforce_quotes(trimmed);
        let new_raw = crate::mail::extract_email(&quoted).to_lowercase();

        if let Some(existing) = addresses.iter_mut().find(|a| {
            !a.contains(':') && crate::mail::extract_email(a).to_lowercase() == new_raw
        }) {
            if has_display_name(&quoted) && !has_display_name(existing) {
                *existing = quoted;
                save_address_book(&addresses)?;
                return Ok(true);
            }
            return Ok(false);
        }
        quoted
    };

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(get_address_book_path())?;
    writeln!(file, "{}", final_address)?;
    Ok(true)
}

pub fn save_address_book(addresses: &[String]) -> io::Result<()> {
    let mut file = File::create(get_address_book_path())?;
    for addr in addresses {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            writeln!(file, "{}", trimmed)?;
        }
    }
    Ok(())
}

pub fn clean_and_save_address_book(addresses: &mut Vec<String>) {
    addresses.retain(|a| !a.trim().is_empty());

    let current_book = addresses.clone();
    for a in addresses.iter_mut() {
        let normalized = if let Some((team_name, emails)) = a.split_once(':') {
            format_team_entry(team_name, emails, &current_book)
        } else {
            enforce_quotes(a)
        };
        *a = normalized;
    }

    sort_and_separate_teams(addresses);
    let _ = save_address_book(addresses);
}

pub fn expand_address_lists(input: &str, address_book: &[String]) -> String {
    let mut processed_input = input.to_string();
    for addr in address_book {
        if let Some((_, emails)) = addr.split_once(':') {
            processed_input = processed_input.replace(addr, emails.trim());
        }
    }

    let mut expanded = Vec::new();
    for part in split_unquoted_commas(&processed_input) {
        let clean_part = part.strip_suffix(" (Team)").unwrap_or(&part).trim();
        let clean_lower = clean_part.to_lowercase();

        let matched = address_book.iter().find_map(|addr| {
            let (list_name, emails) = addr.split_once(':')?;
            (clean_lower == list_name.trim().to_lowercase())
                .then(|| emails.trim().trim_end_matches(';').to_string())
        });

        if let Some(team_emails) = matched {
            expanded.push(team_emails);
        } else if !clean_part.is_empty() {
            expanded.push(enforce_quotes(clean_part));
        }
    }

    expanded.join(", ")
}