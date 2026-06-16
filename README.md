# xpine

A fast, lightweight, and fully-featured terminal-based email client written in Rust. Designed for speed, privacy, and productivity, `xpine` keeps your inbox right in the terminal without sacrificing modern features.

## Features

* **Modern Authentication:** Full support for standard IMAP/SMTP as well as Microsoft Graph API (OAuth 2.0) for Outlook, Hotmail, and Exchange accounts.
* **Built-in Editor:** Compose emails with a custom, feature-rich text editor that includes soft-wrapping, line numbers, paragraph justification, and built-in spellchecking.
* **Address Book & Teams:** Manage individual contacts, import lists, and create distribution groups ("Teams") directly within the app.
* **Theming:** Multiple built-in color themes optimized for both light and dark terminal backgrounds.
* **Smart Updates:** Built-in, non-intrusive update checker to ensure you are always running the latest version.
* **Cross-Platform:** Runs beautifully on macOS and Linux.

---

## Installation

### macOS

The easiest way to install `xpine` on macOS is using the pre-compiled installer package. The application is fully signed and notarized by Apple.

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

#### Option 2: Compile from Source
For other Linux distributions (Fedora, Arch, etc.), or if you prefer to build from source, you can easily compile `xpine` using `cargo`.

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

On your first launch, you will be greeted by the Main Menu. Press `E` to navigate to **Email Accounts** and add your first account. `xpine` will safely and locally store your configurations in `~/.xpine/xpinerc`.

**Navigation Tip:** You can press `?` (or `^H` when inside the email composer) on almost any screen to bring up the contextual help menu detailing all available keyboard shortcuts.

## License

This project is licensed under the [LICENSE](LICENSE) file included in the repository.

