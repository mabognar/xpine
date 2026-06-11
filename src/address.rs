use std::fs;
use std::io::BufRead;
use std::path::PathBuf;

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
    let addresses = load_address_book();

    // Check if the address already exists (ignoring whitespace differences)
    if addresses.iter().any(|a| a.trim() == address.trim()) {
        return Ok(false); // Return false indicating it's a duplicate
    }

    let path = get_address_book_path();
    let mut file = fs::OpenOptions::new()
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
    // 1. Pre-process: If the user autocompleted a team, it will contain the full
    // "teamname: email1, email2;" string. We must replace the full string
    // with just the emails before splitting by comma.
    let mut processed_input = input.to_string();
    for addr in address_book {
        if let Some((_, emails)) = addr.split_once(':') {
            // Replace the full team definition with just the emails
            processed_input = processed_input.replace(addr, emails.trim());
        }
    }

    let mut expanded = Vec::new();

    // 2. Now it is safe to split by comma
    for part in processed_input.split(',') {
        let part = part.trim();
        let mut matched_list = false;

        // 3. Check if they just typed the team name manually (e.g., "me")
        for addr in address_book {
            if let Some((list_name, emails)) = addr.split_once(':') {
                if part.to_lowercase() == list_name.trim().to_lowercase() {
                    expanded.push(emails.trim().trim_end_matches(';').to_string());
                    matched_list = true;
                    break;
                }
            }
        }

        if !matched_list && !part.is_empty() {
            expanded.push(part.to_string());
        }
    }

    expanded.join(", ")
}
