# xpine

`xpine` is a fast, efficient terminal-based email client written in Rust. 

<img width="740" height="588" alt="xpine-processed" src="https://github.com/user-attachments/assets/7d03509f-08dc-46c8-86cd-4d336dd0acac" />

---

## Features

* **Authentication:** Full support for standard IMAP/SMTP accounts
* **Google Accounts:** OAuth 2.0 authentication for Gmail accounts (pre-compiled binaries only --- use Application Specific Passwords for self-compiled binaries)
* **Microsoft Accounts:** Graph API with OAuth 2.0 authentication for Outlook, Hotmail, and Exchange accounts
* **Multiple Accounts:** Multiple accounts are supported, and accounts can be changed with a single key-stroke
* **Help:** Built-in help describing the hot-keys and functionality
* **Editor:** Compose emails with a built-in text editor that includes soft-wrapping and paragraph justification
* **Address Book & Teams:** Manage individual contacts, import lists, and create distribution groups ("Teams")
* **Address Completion:** `xpine` provides for autocompletion of email addresses (just hit `tab`)
* **Spell Checking:** With the option to automatically spellcheck before sending an email
* **Flexible Signature:** Insert your signature where/if you want with a single key-stroke 
* **Auto Prettify:** Before sending, `xpine` automatically justifies your email --- raggedly typed emails will look nice
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

### Pre-compiled binary vs. Compiling yourself
The pre-compiled binaries allow connecting to Google Gmail via modern authentication (OAuth 2.0). Compiling yourself 
will NOT allow you to connect to Google Gmail via modern authentication (OAuth 2.0). 
Use an Application Specific Password instead (search online how to do this). 



## Installation

### macOS

The easiest way to install `xpine` on macOS is using the pre-compiled installer package. 
The application is fully signed and notarized by Apple.

1. Go to the [Releases](https://github.com/mabognar/xpine/releases/latest) page.
2. Download the latest `xpine-macOS.pkg` file.
3. Double-click the downloaded `.pkg` file and follow the standard installation prompts.
4. Open your terminal and type `xpine` to launch the application!

### Linux

Linux requirements:
1. Need Secret Service API such as `gnome-keyring` or `kwallet` installed
2. Need the `libdbus-1-dev` package installed

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

Note: Compiling from source will not allow Google Gmail OAuth 2.0 authentication. Use an Application Specific Password
instead (search online how to do this). 

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


## Security & Credential Management

`xpine` prioritizes your privacy and security by ensuring your sensitive information (such as IMAP passwords, OAuth 2.0 refresh tokens, and client secrets) is never stored in plain text on your drive.

Here is how `xpine` manages your credentials securely:

* **Separation of Data:** Non-sensitive settings (like your email address and IMAP server ports) are stored in a standard plain text TOML file (`~/.xpine/xpinerc`). However, all sensitive credentials bypass this file entirely.
* **AES-256-GCM Encryption:** Passwords and OAuth tokens are stored in a separate, fully encrypted binary vault (`~/.xpine/secrets.enc`). `xpine` uses AES-256-GCM, an industry-standard authenticated encryption algorithm, to secure this data.
* **Auto-Generated Master Key:** On first launch, `xpine` generates a cryptographically secure 256-bit master key (`~/.xpine/.master.key`) used to encrypt and decrypt your vault.
* **Strict File Permissions:** On macOS and Linux systems, `xpine` automatically enforces strict `0600` (read/write by owner only) file permissions on the master key. This ensures that other users on the same machine or unauthorized applications cannot read your encryption key.

*Note: If you migrate your `xpine` configuration to a new computer, you must copy both the `secrets.enc` vault and the hidden `.master.key` file for your encrypted credentials to carry over successfully.*


## License

This project is licensed under the [LICENSE](LICENSE) file included in the repository.

