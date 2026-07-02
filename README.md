# xpine

`xpine` is a fast, efficient terminal-based email client written in Rust. 

<img width="740" height="588" alt="xpine-processed" src="https://github.com/user-attachments/assets/7d03509f-08dc-46c8-86cd-4d336dd0acac" />

---

## Features

* **Authentication:** Full support for standard IMAP/SMTP accounts
* **Google Accounts:** OAuth 2.0 authentication or Application Specific Passwords
* **Microsoft Accounts:** Graph API with OAuth 2.0 authentication for Outlook, Hotmail, and Exchange accounts
* **Yahoo, Apple:** Application Specific Passwords
* **Multiple Accounts:** Multiple accounts are supported -- switch between accounts with a single key-stroke
* **Help:** Built-in help describing the hot-keys and functionality
* **Editor:** Compose emails with a built-in text editor that includes soft-wrapping and paragraph justification
* **Address Book & Teams:** Manage individual contacts, import lists, and create distribution groups ("Teams")
* **Address Completion:** `xpine` provides for autocompletion of email addresses (just hit `tab`)
* **Spell Checking:** With the option to automatically spellcheck before sending an email
* **Flexible Signature:** Insert your signature where/if you want with a single key-stroke 
* **Auto Prettify:** `xpine` automatically justifies your email upon sending --- ragged emails will look nice
* **Folders:** Move emails from folder-to-folder
* **Theming:** Multiple built-in color themes
* **Updates:** Built-in update checker
* **Platforms:** Runs on macOS and Linux

---

## Initial Setup

### macOS
In your Terminal settings, go to `Profiles > select your profile > Keyboard` and make sure
`Use Option as Meta key` is checked.

### Gmail Users
In Gmail (on the web), go to `Settings > See all settings > Forwarding and POP/IMAP` and make sure
`Auto-Expunge off - Wait for the client to update the server` 
is checked.

---

## Determine Connection Method
Before running `xpine`, determine your connection method and get the necessary information.
1. Google Gmail App Specific Password (less secure, easier to setup) [instructions](#google-gmail)
2. Google Gmail OAuth 2.0 (more secure, harder setup, need to generate Google client_id and client_secret) [instructions](#setting-up-google-gmail-oauth-20)
3. Microsoft Outlook, Hotmail, Exchange (secure, easy setup, can be setup directly in xpine)
4. Yahoo App Specific Password [instructions](#yahoo-mail)
5. Apple/iCloud App Specific Password [instructions](#apple-icloud-mail)

---

## Installation

### macOS

The easiest way to install `xpine` on macOS is using the pre-compiled installer package. 
The application is fully signed and notarized by Apple.

1. Go to the [Releases](https://github.com/mabognar/xpine/releases/latest) page.
2. Download the latest `xpine-macOS.pkg` file.
3. Double-click the downloaded `.pkg` file and follow the standard installation prompts.
4. Open your terminal and type `xpine` to launch the application!

### Linux

#### Option 1: Debian/Ubuntu (.deb package)
If you are using Debian, Ubuntu, Linux Mint, Pop!_OS, or any other Debian derivative:

1. Download the latest `xpine-linux.deb` file from the [Releases](https://github.com/mabognar/xpine/releases/latest) page.
2. Open your terminal and navigate to your downloads folder.
3. Install the package using `apt`:

   ```bash
   sudo apt install ./xpine-linux.deb
   ```

4. Run `xpine` from your terminal.

Using the `.deb` package will allow for Google Gmail OAuth 2.0 authentication.

#### Option 2: Compile from Source
For other Linux distributions (Fedora, Arch, etc.), or if you prefer to build from source, 
you can easily compile `xpine` using `cargo`.

**1. Install System Dependencies**
You will need the standard C compiler tools and OpenSSL headers to handle secure IMAP and HTTP connections.

* **Debian / Ubuntu / Pop!_OS:**
  ```bash
  sudo apt update
  sudo apt install build-essential pkg-config libssl-dev
  ```

* **Fedora / RHEL / CentOS:**
  ```bash
  sudo dnf groupinstall "Development Tools"
  sudo dnf install pkgconf-pkg-config openssl-devel
  ```

* **Arch Linux / Manjaro:**
  ```bash
  sudo pacman -S base-devel pkgconf openssl
  ```

**2. Install Rust**
If you do not have Rust installed on your system, install it via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf [https://sh.rustup.rs](https://sh.rustup.rs) | sh
```

**3. Build xpine**
Clone the repository and build the release binary:

```bash
git clone [https://github.com/mabognar/xpine.git](https://github.com/mabognar/xpine.git)
cd xpine
cargo build --release
```

Once compiled, the executable will be located at `target/release/xpine`. You can safely move this binary to `/usr/local/bin/` or any other directory in your `$PATH` to run it globally.

---

## Getting Started

To launch the client, simply open your terminal and type:

```bash
xpine
```

On your first launch, you will be greeted by the Main Menu. Press `E` to navigate to **Email Accounts** and add your 
first account. `xpine` will safely and locally store your configurations in `~/.xpine/xpinerc`.

---

## **Connecting via Basic IMAP/SMTP (App Passwords)**

If you are using Yahoo, Apple iCloud, or prefer to connect to Gmail via standard IMAP instead of OAuth, 
you cannot use your regular email account password to log into `xpine`.

Modern email providers require you to generate a unique **Application-Specific Password**. This is a 
special 16-character password created just for `xpine` that allows it to securely sync your mail.

*Note: For almost all providers, you **must** have Two-Factor Authentication (2FA) enabled on your 
account before the option to create an App Password will appear.*

---

### **Google Gmail**
*(Note: If you use the Gmail OAuth option in the xpine menu, you do not need to do this. This is only for the "Basic IMAP" menu option).*

1. Go to your [Google Account management page](https://myaccount.google.com/).
2. On the left navigation panel, click **Security**.
3. Under the "How you sign in to Google" section, ensure **2-Step Verification** is turned **ON**.
4. Click on **2-Step Verification** and scroll all the way to the bottom of the page.
5. Click on **App passwords**.
6. In the "App name" field, type `xpine` and click **Create**.
7. Google will generate a 16-character password in a yellow box. Copy this exact password (without spaces) and paste it into the `xpine` password prompt.

### **Yahoo Mail**
1. Log into your Yahoo account and go to your [Account Security page](https://login.yahoo.com/account/security).
2. Scroll down to the "Other ways to sign in" section.
3. Click on **Generate and manage app passwords**.
4. Type `xpine` into the "App name" field and click **Generate password**.
5. Yahoo will display a one-time, 16-character password. Copy this (without spaces) and use it to log into `xpine`.

### **Apple iCloud Mail**
1. Go to [appleid.apple.com](https://appleid.apple.com/) and sign in with your Apple ID.
2. In the left navigation panel, click **App-Specific Passwords**.
3. Click the **Generate an app-specific password** button (or the "+" icon).
4. Enter `xpine` as the name and click **Create**.
5. Enter your standard Apple ID password to confirm.
6. Apple will reveal a 16-character password (formatted like `xxxx-xxxx-xxxx-xxxx`). 
7. Copy this password and use it as your `xpine` password.

**Troubleshooting:**
* Never type the spaces. Even if the provider displays the password with spaces (e.g., `abcd efgh ijkl mnop`), always enter it into `xpine` as a single continuous string (`abcdefghijklmnop`).
* App Passwords are shown exactly once. If you lose it or need to reinstall `xpine` on a new machine without copying your secure `secrets.enc` vault, simply delete the old password from your provider's website and generate a new one.

---

## Setting Up Google Gmail OAuth 2.0
Because xpine is a fully local, open-source terminal application that requires full read/write access to your inbox, 
Google classifies it as requesting a "Restricted Scope." For a centralized application to offer this 
automatically, Google requires a costly third-party security audit.

To keep xpine free, open-source, and secure, it utilizes a "Bring Your Own Credentials" (BYOC) model. 
You will act as your own developer by generating a personal, free Google Cloud credential. 
This takes about 3 minutes and ensures your OAuth tokens are tied exclusively to your personal project.

### Step 1: Create a Google Cloud Project
1. Navigate to the Google Cloud Console (https://console.cloud.google.com/) and log in with your Google account.
2. Click the project dropdown menu in the top-left navigation bar and select New Project.
3. Name your project (e.g., xpine-local-client) and click Create.

### Step 2: Enable the Gmail API
1. Make sure your newly created project is selected in the top-left dropdown.
2. In the left sidebar, click APIs & Services, then click Library.
3. Search for Gmail API, click on it, and click Enable.

### Step 3: Configure the OAuth Consent Screen
1. In the left sidebar under APIs & Services, click OAuth consent screen.
2. Select External as the User Type and click Create.
3. Fill in the required fields: App Name (e.g., xpine-client), User Support Email (your email), and 
Developer Contact Information (your email). Ignore all other optional fields and click Save and Continue.
4. On the Scopes screen, click Add or Remove Scopes.
5. Manually paste https://mail.google.com/ into the search/filter box, check the box to add it, click Update, 
and then Save and Continue.
6. On the Test Users screen, click Add Users. Type your exact Gmail address, click Add, and then Save and Continue.

### Step 4: Generate Your Credentials
1. In the left sidebar, click Credentials.
2. Click + Create Credentials at the top of the screen and select OAuth client ID.
3. Select Desktop app from the Application type dropdown.
4. Name it (e.g., xpine-desktop) and click Create.
5. A popup will appear containing your Client ID and Client Secret. Keep this window open.

### Step 5: Connect xpine
1. Launch xpine in your terminal.
2. Navigate to the Accounts menu and select the option to add a new Gmail OAuth account.
3. When prompted, carefully paste your Client ID and Client Secret.
4. Your browser will open. You will see a warning stating "Google hasn’t verified this app." 
Because you just created this app in Testing mode for yourself, this is expected. Click Advanced, 
then Go to xpine-client (unsafe), and click Continue.

Security Note: xpine uses AES-256-GCM encryption to automatically secure your Client Secret and 
refresh tokens locally on your machine. They are never stored in plain text.

---

## Security & Credential Management

`xpine` prioritizes your privacy and security by ensuring your sensitive information (such as IMAP passwords, OAuth 2.0 refresh tokens, and client secrets) is never stored in plain text on your drive.

Here is how `xpine` manages your credentials securely:

* **Separation of Data:** Non-sensitive settings (like your email address and IMAP server ports) are stored in a standard plain text TOML file (`~/.xpine/xpinerc`). However, all sensitive credentials bypass this file entirely.
* **AES-256-GCM Encryption:** Passwords and OAuth tokens are stored in a separate, fully encrypted binary vault (`~/.xpine/secrets.enc`). `xpine` uses AES-256-GCM, an industry-standard authenticated encryption algorithm, to secure this data.
* **Auto-Generated Master Key:** On first launch, `xpine` generates a cryptographically secure 256-bit master key (`~/.xpine/.master.key`) used to encrypt and decrypt your vault.
* **Strict File Permissions:** On macOS and Linux systems, `xpine` automatically enforces strict `0600` (read/write by owner only) file permissions on the master key. This ensures that other users on the same machine or unauthorized applications cannot read your encryption key.

*Note: If you migrate your `xpine` configuration to a new computer, you must copy both the `secrets.enc` vault and the hidden `.master.key` file for your encrypted credentials to carry over successfully.*

---

## License

This project is licensed under the [LICENSE](LICENSE) file included in the repository.


