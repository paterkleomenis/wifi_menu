# Wifi Menu (Rust)

A simple, TUI-based Wi-Fi manager for Linux, written in Rust. It wraps `nmcli` to provide an interactive menu for scanning and connecting to networks.

## Features

- **Scan & List:** Automatically scans and lists available networks with signal strength and security status.
- **Connect:** Connect to open or secured (WPA/WPA2) networks.
- **Manage:** Disconnect or forget known networks.
- **Desktop & Server:** Works anywhere `nmcli` is available (Terminal-based).

## Requirements

- Linux
- `NetworkManager` (specifically the `nmcli` tool)

## Installation

Download the latest binary from the [Releases](https://github.com/paterkleomenis/wifi_menu/releases) page.

Or build from source:

```bash
git clone https://github.com/paterkleomenis/wifi_menu.git
cd wifi_menu
cargo build --release
sudo cp target/release/wifi_menu /usr/local/bin/
```

## Usage

Run the tool from your terminal:

```bash
wifi_menu
```

- **Arrow Keys / j/k:** Navigate
- **Enter:** Connect / Action Menu
- **i:** Switch Wi-Fi interface
- **r:** Rescan
- **q / Esc:** Quit
- **Tab (password prompt):** Show/Hide password

### Non-interactive mode

```bash
wifi_menu --status
wifi_menu --status --interface wlan0
wifi_menu --rescan --interface wlan0
wifi_menu --disconnect --interface wlan0
```
