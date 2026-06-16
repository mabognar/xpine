use crate::config::Account;
use crate::mail::EmailMeta;
use std::time::{Duration, Instant};
// use std::collections::{HashMap, HashSet};
use std::thread;
use reqwest::header::USER_AGENT;
use serde::Deserialize;

pub enum AppMode {
    AddressBook {
        selected_idx: usize,
        addresses: Vec<String>,
    },
    EmailAccounts {
        selected_idx: usize,
    },
    EmailList,
    EmailRead {
        text_body: String,
        html_body: Option<String>,
        attachments: Vec<(String, Vec<u8>)>,
    },
    FolderList {
        step: u8,
        selected_idx: usize,
        folders: Vec<String>,
    },
    MainMenu {
        selected_idx: usize,
    },
    Settings {
        selected_idx: usize,
    },
}

#[derive(Clone)]
pub enum _BrowserAction {
    SaveEmail(String), // Holds the text_body to save
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
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
    pub latest_version: Option<String>,
}

// Call this function when xpine starts
pub fn spawn_update_checker(app_state_sender: std::sync::mpsc::Sender<String>) {
    thread::spawn(move || {
        let url = "https://api.github.com/repos/mabognar/xpine/releases/latest";
        let client = reqwest::blocking::Client::new();

        if let Ok(response) = client.get(url).header(USER_AGENT, "xpine-updater").send() {
            if let Ok(release) = response.json::<GithubRelease>() {
                let latest = release.tag_name.trim_start_matches('v').to_string();
                // Send the version string back to your main event loop
                let _ = app_state_sender.send(latest);
            }
        }
    });
}

impl App {
    pub fn new(accounts: Vec<Account>) -> Self {
        let is_empty = accounts.is_empty();

        let (active_account, mode, needs_fetch) = if is_empty {
            (
                Account {
                    email: String::new(),
                    password: None,
                    client_id: None,
                    client_secret: None,
                    refresh_token: None,
                    imap_server: String::new(),
                    imap_port: 993,
                    smtp_server: String::new(),
                    smtp_port: 587,
                },
                AppMode::MainMenu { selected_idx: 3 },
                false, // Don't fetch if no accounts exist
            )
        } else {
            (accounts[0].clone(), AppMode::EmailList, true)
        };

        let mut app = Self {
            mode,
            current_account_idx: 0,
            active_account,
            current_folder: String::from("INBOX"),
            total_messages: 0,
            current_page: 0,
            selected_index: 0,
            page_emails: Vec::new(),
            needs_fetch,
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
            latest_version: None,
        };

        if is_empty {
            app.update_status("No email account specified. Press 'E' to add email account.".to_string());
        }

        app
    }

    pub fn update_status(&mut self, msg: String) {
        self.list_status = msg;
        self.list_status_time = Some(Instant::now());
        self.list_status_duration = Duration::from_millis(3000);
    }
}