use crate::config::Account;
use crate::mail::EmailMeta;
use std::time::{Duration, Instant};
use std::path::{Path, PathBuf};
use std::fs;

pub enum AppMode {
    List,
    Reading {
        text_body: String,
        html_body: Option<String>,
        attachments: Vec<(String, Vec<u8>)>,
    },
    MainMenu {
        selected_idx: usize,
    },
    Settings {
        selected_idx: usize,
    },
    FolderList {
        step: u8,
        selected_idx: usize,
        folders: Vec<String>,
    },
    AddressBook {
        selected_idx: usize,
        addresses: Vec<String>,
    },
}

#[derive(Clone)]
pub enum BrowserAction {
    SaveEmail(String), // Holds the text_body to save
}

pub struct App {
    pub mode: AppMode,
    pub current_account_idx: usize,
    pub active_account: Account,
    pub current_folder: String,
    pub total_messages: u32,
    pub current_page: u32,
    pub selected_index: usize,
    pub page_emails: Vec<EmailMeta>,
    pub needs_fetch: bool,
    pub needs_reconnect: bool,
    pub restore_index_from_end: Option<u32>,
    pub list_status: String,
    pub list_status_time: Option<Instant>,
    pub list_status_duration: Duration,
    pub last_fetch_time: Instant,
    pub auto_refresh_interval: Duration,
    pub accounts: Vec<Account>,
    pub menu_page: u8,
    pub search_query: Option<String>,
}

impl App {
    pub fn new(accounts: Vec<Account>) -> Self {
        Self {
            mode: AppMode::List,
            current_account_idx: 0,
            active_account: accounts[0].clone(),
            current_folder: String::from("INBOX"),
            total_messages: 0,
            current_page: 0,
            selected_index: 0,
            page_emails: Vec::new(),
            needs_fetch: true,
            needs_reconnect: false,
            restore_index_from_end: Some(0),
            list_status: String::new(),
            list_status_time: None,
            list_status_duration: Duration::from_secs(3),
            last_fetch_time: Instant::now(),
            auto_refresh_interval: Duration::from_secs(60),
            accounts,
            menu_page: 1,
            search_query: None,
        }
    }
    
    pub fn update_status(&mut self, msg: String) {
        self.list_status = msg;
        self.list_status_time = Some(Instant::now());
        self.list_status_duration = Duration::from_millis(1500);
    }

    // pub fn refresh_browser_entries(current_dir: &Path) -> Vec<(String, PathBuf, bool)> {
    //     let mut entries = vec![];
    //     entries.push((".".to_string(), current_dir.to_path_buf(), true));
    //     if let Some(parent) = current_dir.parent() {
    //         entries.push(("..".to_string(), parent.to_path_buf(), true));
    //     }
    //
    //     if let Ok(read_dir) = fs::read_dir(current_dir) {
    //         let mut dirs = vec![];
    //         let mut files = vec![];
    //         let mut dot_dirs = vec![];
    //         let mut dot_files = vec![];
    //
    //         for entry in read_dir.flatten() {
    //             let path = entry.path();
    //             let name = entry.file_name().to_string_lossy().into_owned();
    //             let is_dir = path.is_dir();
    //             let is_dot = name.starts_with('.');
    //
    //             if is_dir {
    //                 if is_dot { dot_dirs.push((name, path, true)); } else { dirs.push((name, path, true)); }
    //             } else {
    //                 if is_dot { dot_files.push((name, path, false)); } else { files.push((name, path, false)); }
    //             }
    //         }
    //
    //         dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    //         files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    //         dot_dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    //         dot_files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    //
    //         entries.extend(dirs);
    //         entries.extend(files);
    //         entries.extend(dot_dirs);
    //         entries.extend(dot_files);
    //     }
    //     entries
    // }
}
