# TauriWA - Simple desktop WhatsApp client for Linux

[![Build](https://github.com/Aegis-plus/TauriWA/actions/workflows/build.yml/badge.svg)](https://github.com/Aegis-plus/TauriWA/actions/workflows/build.yml)

A small native WhatsApp Web client written in Rust, using the system WebKitGTK through
[tao](https://github.com/tauri-apps/tao) and [wry](https://github.com/tauri-apps/wry)
(the window and webview libraries behind Tauri), with
[ksni](https://github.com/iovxw/ksni) for the tray icon and [zbus](https://github.com/dbus2/zbus) for D-Bus.

TauriWA is a Rust port of [ElecWhat](https://github.com/piec/elecwhat) by Pierre Carru.

## Features
* Desktop notifications (click one to open the chat)
* Tray icon with unread count (aka AppIndicator / StatusNotifierItem), [custom tray icons](#custom-icons)
* Custom keyboard shortcuts ([default](src/keys.rs))
* Custom CSS
* Spellcheck
* CLI & D-Bus interface to show/hide/toggle window
* Dock unread count badge
* Small and light: a single ~5 MB binary, no bundled browser
* Built by GitHub Actions (CI) from this repository
* Preserve window position and zoom level
* Custom user scripts in `~/.config/tauriwa/user-scripts/*.js`
* Custom tray icons in `~/.config/tauriwa/user-icons/*.png` by default
* Supports Wayland or X11

## Install

### Download
Every push is built by [GitHub Actions](https://github.com/Aegis-plus/TauriWA/actions/workflows/build.yml):
open a run and download `tauriwa-linux-x86_64` from its *Artifacts*.
Tagged versions are published in [Releases].

They contain:
* `tauriwa_<version>-1_amd64.deb` for Debian/Ubuntu: `sudo apt install ./tauriwa_*.deb`
* `tauriwa-<version>-1.x86_64.rpm` for Fedora/openSUSE: `sudo dnf install ./tauriwa-*.rpm`
* `tauriwa-<version>-linux-x86_64.tar.xz`: the binary, to run anywhere WebKitGTK 4.1 is installed

To publish a release, push a tag:

```bash
git tag v1.0.0 && git push origin v1.0.0
```

### From source
Dependencies: Rust (stable), WebKitGTK 4.1 and GTK 3 development files, git-lfs (icons).

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev   # Debian/Ubuntu
sudo dnf install webkit2gtk4.1-devel gtk3-devel        # Fedora
sudo pacman -S webkit2gtk-4.1 gtk3                     # Arch
```

```bash
git lfs pull
cargo build --release
```

Then copy `target/release/tauriwa` to your `PATH`, `packaging/tauriwa.desktop` to
`~/.local/share/applications/` and `static/app-512.png` to
`~/.local/share/icons/hicolor/512x512/apps/tauriwa.png`.
Or build a package with [cargo-deb](https://github.com/kornelski/cargo-deb) (`cargo deb`) or
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm) (`cargo generate-rpm`).

### Voice/video calls
Calls depend on WebRTC support in your WebKitGTK version (2.40+ built with WebRTC,
plus the GStreamer plugins), and on WhatsApp Web accepting it. They may not work
on every distribution.

## Command line

```
tauriwa [--show | --hide | --toggle | --quit] [--log-level=LEVEL] [whatsapp://URL]
```

When TauriWA is already running, the command is sent to it (single instance through D-Bus).

## D-Bus

Service `io.github.aegis_plus.tauriwa`, object `/io/github/aegis_plus/tauriwa`, interface `io.github.aegis_plus.tauriwa`:
`Show`, `Hide`, `Visible → b`, `ToggleVisibility → b`, `OpenUrl(s)`, `Activate(s token)`, `Quit`.

```bash
gdbus call --session --dest io.github.aegis_plus.tauriwa --object-path /io/github/aegis_plus/tauriwa --method io.github.aegis_plus.tauriwa.ToggleVisibility
```

## Configuration

`~/.config/tauriwa/config.json` (comments and trailing commas are allowed):

```
{
  "log-level": "info",
  "notification-prefix": "tauriwa - ",

  "quit-on-close": false,
  "show-at-startup": false,

  "dbus": true,
  "close-chat-on-hide": true,
  "register-url-scheme": true,
  "retry-interval": 15000,

  "keys": {
    "C ArrowDown": {
      "whatsappAction": "GO_TO_NEXT_CHAT"
    }
  },
  "esc-toggle-window": false,

  "css": [
    "span { font-size: small !important; }",
    "https://example.com/whatsapp.css",
    "file://~/mysheet.css"
  ],

  "spellcheck": true,
  "spellcheck-languages": ["en-US", "fr"],

  "user-agent": "...",
  "open-dev-tools": false,

  "scripts": [
    // ~/.config/tauriwa/user-scripts/myscript.js
    "${userData}/user-scripts/myscript.js"
  ],
  "icons-directory": "..."
}
```

## Custom icons

The naming scheme is based on WhatsApp Web favicon urls:
* `~/.config/tauriwa/user-icons/app.png` during startup
* `~/.config/tauriwa/user-icons/favicon.png` when there is no unread
* `~/.config/tauriwa/user-icons/f01.png` where fXX.png is the number of unread

## Keyboard shortcuts
Bindings are `"<modifiers> <key>"` (`C`=Ctrl, `S`=Shift, `A`=Alt, `M`=Meta) or just `"<key>"`,
`key` being a [KeyboardEvent.key](https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values) value.
Besides WhatsApp's own actions (`{"whatsappAction": "..."}`), `{"action": "IPC", "method": "..."}`
can call: `windowToggle`, `windowShow`, `windowHide`, `reload`, `quit`, `zoomIn`, `zoomOut`,
`zoomReset`, `fullscreenToggle`, `devTools`.

The defaults include `Ctrl+R` (reload), `Ctrl+Q` (quit), `Ctrl+=`/`Ctrl+-` (zoom), `F11` (fullscreen)
and `Ctrl+Shift+I` (developer tools).

## Data
* `~/.config/tauriwa/`: `config.json`, `persistent-state.json` (window position, zoom), `user-scripts/`, `user-icons/`
* `~/.local/share/tauriwa/webview/`: WhatsApp Web session (cookies, local storage, cache)

### Coming from ElecWhat
On first start, `config.json`, `user-scripts/` and `user-icons/` are copied from `~/.config/elecwhat/`.
You'll have to link your device again (scan the QR code): the browser session can't be carried over.
`menu-bar` and `menu-bar-auto-hide` are not used (there is no menu bar, see the keyboard shortcuts).

## Not planned
* Multi account
* Complex features
* Other OS support, official apps already exist.

## License
GPL-3.0-or-later, see [LICENSE](LICENSE).

[Releases]: https://github.com/Aegis-plus/TauriWA/releases
