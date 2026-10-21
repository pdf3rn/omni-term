# omni-term

A native, cross-platform terminal written in Rust, built like an IDE: local shells, saved SSH, Telnet and serial connections, and a two-pane file explorer for SFTP, FTP/FTPS and SMB.

> **Status:** version 0.x. Expect rough edges and changes between minor versions. See [Status](#status).

## Features

**Terminal**
- Tabs and split panes with draggable dividers; the session is restored when you reopen the app.
- Mouse support for `vim`/`tmux`/`htop`, kitty keyboard protocol, history search, clickable links, true color with italics, dim and all underline styles (curly, double, dotted, dashed, colored), answers to color queries (OSC 4/10/11) so editors pick the right theme, and block/box characters drawn as geometry.
- Double-click selects a word, triple-click a line; context menu, primary selection on Linux.
- Command palette, menu bar, light/dark/system theme, selectable accent and ANSI palette (Catppuccin, GNOME, Tango), any installed monospace font, font size and cursor.

**Connections**
- **SSH** with password, key (with passphrase), `ssh-agent` and interactive (2FA) authentication, jump hosts (`ProxyJump`), host key verification against `~/.ssh/known_hosts`, and a known-hosts browser.
- **Telnet** and **serial** ports (baud, parity, flow control, live line controls).
- **Containers**: a small manager for Docker and Podman that lists every container (also the stopped ones), starts, stops and restarts them, and opens a shell inside a running one. Open it from the "+" menu, *Ir → Contenedores* or the command palette.
- **Android (ADB)**: a shell and the file explorer for a device connected by USB or Wi-Fi, through the `adb` server (needs Android platform-tools installed).
- Saved connections with a color and environment ("production", "staging") shown in the sidebar, tab, a band under the tabs and the window title, so you don't type into the wrong server.
- Port forwarding per connection: `-L`, `-R` and `-D` (SOCKS5 proxy).
- Snippets with search and `{{name}}` / `{{name=default}}` variables.

**Files**
- Two-pane explorer (local and remote) over **SFTP**, **FTP/FTPS** and **SMB 2/3**: drag and drop, recursive background transfers with cancellation, conflict resolution, permissions (where the protocol supports them) and a local trash.
- A saved "Files" connection opens the explorer directly, without a terminal.

**Security**
- Secrets live in the system keyring, or in your own master key (Argon2id + XChaCha20-Poly1305) when no keyring is available.
- Plain FTP and Telnet show a lock icon and a warning before connecting.
- Closing a tab with a running process asks for confirmation.

## Install

Download the archive for your system from the [Releases](https://github.com/pdf3rn/omni-term/releases) page. On Linux it includes `install.sh`, which installs the binary, icon and desktop entry for your user:

```bash
./install.sh
```

Or build from source (Rust stable):

```bash
cargo build --release --locked
packaging/install-linux.sh        # optional, Linux only
```

On Linux you need the development headers for `libxkbcommon`, `wayland`, `x11` and `dbus`. `omni-term --version` prints the installed version.

## Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy selection / paste |
| `Ctrl+Shift+F` | Search history (`Enter` next, `Shift+Enter` previous, `Esc` closes) |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+Shift+B` / `Ctrl+Shift+K` | Toggle sidebar / search connections |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split right / split down |
| `Ctrl+Shift+W` | Close the focused pane (or the tab if it is the only one) |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Increase, decrease and reset the font |
| `F11` | Fullscreen |
| `Ctrl` + click | Open the link under the pointer |
| `Shift` + drag | Select text even when the application uses the mouse |

`Ctrl+C` always goes to the application (it interrupts the process).

## Data

The database (connections, forwards, snippets, session and settings) is in the system data directory (`~/.local/share/omniterm/omniterm.db` on Linux). Passwords are not stored there, except encrypted with the master key.

## Status

omni-term is below 1.0. These parts have only been tested against local servers:

- FTPS with real certificates (self-signed certificates are rejected).
- SMB uploads, renames and deletes against a fully conformant server (tested with a local test server only).
- Podman support is untested (only Docker was available), and the container manager does not work on Windows yet (named pipe).
- Android (ADB) has only been tested against a simulated `adb` server, not a real device. Files larger than 4 GB are not supported yet.

Bug reports are welcome. For vulnerabilities, see [SECURITY.md](SECURITY.md).

## Contributing and releases

See [CONTRIBUTING.md](CONTRIBUTING.md). The project follows [Semantic Versioning](https://semver.org/); changes are listed in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Bundled fonts and icons are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
