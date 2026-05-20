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
