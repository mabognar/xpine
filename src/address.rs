use std::fs;
use std::io::BufRead;
use std::path::PathBuf;

pub fn enforce_quotes(address: &str) -> String {
    let trimmed = address.trim();
    if let Some(start) = trimmed.find('<') {
        let name_part = trimmed[..start].trim();
        if name_part.is_empty() {
            return trimmed.to_string();
        }

        // If it's already properly quoted, leave it alone
        if name_part.starts_with('"') && name_part.ends_with('"') {
            return trimmed.to_string();
        }

        // Remove any dangling outer quotes just in case, then explicitly wrap
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
    let path = get_address_book_path();
    let mut addresses = Vec::new();

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

    if let Some(first_team_idx) = addresses.iter().position(|a| a.contains(':')) {
        if first_team_idx > 0 {
            addresses.insert(first_team_idx, String::new());
        }
    }

    addresses
}

pub fn add_to_address_book(address: &str) -> std::io::Result<bool> {
    let mut addresses = load_address_book();

    let mut final_address = address.trim().to_string();
    if let Some((team_name, emails)) = final_address.clone().split_once(':') {
        let expanded_emails = expand_address_lists(emails, &addresses);
        final_address = format!("{}: {};", team_name.trim(), expanded_emails.trim_end_matches(';'));

        if addresses.iter().any(|a| a.trim() == final_address) {
            return Ok(false);
        }
    } else {
        // NEW: Enforce quotes before checking or saving
        final_address = enforce_quotes(&final_address);

        // --- Individual Email Upgrade Logic ---
        let new_raw = crate::mail::extract_email(&final_address).to_lowercase();
        let new_has_name = final_address.contains('<') && final_address.contains('>');
        let mut replaced = false;

        for existing_addr in addresses.iter_mut() {
            if existing_addr.contains(':') { continue; }

            let existing_raw = crate::mail::extract_email(existing_addr).to_lowercase();

            if new_raw == existing_raw {
                let existing_has_name = existing_addr.contains('<') && existing_addr.contains('>');

                if new_has_name && !existing_has_name {
                    // Upgrade: Replace the raw email with the formatted named version
                    *existing_addr = final_address.clone();
                    replaced = true;
                    break;
                } else {
                    return Ok(false);
                }
            }
        }

        if replaced {
            save_address_book(&addresses)?;
            return Ok(true);
        }
    }

    let path = get_address_book_path();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    use std::io::Write;
    writeln!(file, "{}", final_address)?;

    Ok(true)
}

pub fn save_address_book(addresses: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let path = get_address_book_path();
    let mut file = fs::File::create(path)?;
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

    // Expand any nested teams inside of teams before sorting/saving
    // Expand any nested teams inside of teams before sorting/saving
    let current_book = addresses.clone();
    for a in addresses.iter_mut() {
        if let Some((team_name, emails)) = a.clone().split_once(':') {
            let expanded_emails = expand_address_lists(emails, &current_book);
            *a = format!("{}: {};", team_name.trim(), expanded_emails.trim_end_matches(';'));
        } else {
            // NEW: Enforce quotes on individual emails saved via the Editor
            *a = enforce_quotes(a);
        }
    }

    // Sort: Individuals first, Teams (containing ':') at the bottom
    addresses.sort_by(|a, b| {
        let a_is_team = a.contains(':');
        let b_is_team = b.contains(':');

        if a_is_team == b_is_team {
            a.cmp(b) // Sort alphabetically within their respective groups
        } else if a_is_team {
            std::cmp::Ordering::Greater // Teams are pushed to the bottom
        } else {
            std::cmp::Ordering::Less    // Individuals are pulled to the top
        }
    });

    // Insert the blank spacer line exactly before the first Team
    if let Some(first_team_idx) = addresses.iter().position(|a| a.contains(':')) {
        if first_team_idx > 0 {
            addresses.insert(first_team_idx, String::new());
        }
    }

    // Save
    let mut save_list = addresses.clone();
    save_list.retain(|a| !a.trim().is_empty());
    let _ = save_address_book(&save_list);
}

pub fn expand_address_lists(input: &str, address_book: &[String]) -> String {
    // 1. Pre-process: (Kept for backward compatibility if you have drafts
    // containing the old raw string insertion format)
    let mut processed_input = input.to_string();
    for addr in address_book {
        if let Some((_, emails)) = addr.split_once(':') {
            processed_input = processed_input.replace(addr, emails.trim());
        }
    }

    let mut expanded = Vec::new();

    // 2. Now it is safe to split by comma
    for part in processed_input.split(',') {
        let part = part.trim();

        // --- NEW: Strip out the " (Team)" suffix safely before checking ---
        let clean_part = part.strip_suffix(" (Team)").unwrap_or(part).trim();
        let mut matched_list = false;

        // 3. Check if the cleaned name matches a team in the address book
        for addr in address_book {
            if let Some((list_name, emails)) = addr.split_once(':') {
                if clean_part.to_lowercase() == list_name.trim().to_lowercase() {
                    expanded.push(emails.trim().trim_end_matches(';').to_string());
                    matched_list = true;
                    break;
                }
            }
        }

        if !matched_list && !clean_part.is_empty() {
            // If it wasn't a team, enforce quotes and push the cleaned part.
            expanded.push(enforce_quotes(clean_part));
        }
    }

    expanded.join(", ")
}

