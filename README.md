# ElecWhat - Simple desktop WhatsApp client for Linux
<p align="center">
<img width="400" alt="elecwhat" src="https://github.com/user-attachments/assets/3a352d5b-04a8-4e90-a063-330b706d096c" />
</p>

## Features
* Desktop notifications (click one to open the chat)
* Tray icon with unread count (aka AppIndicator / StatusNotifierItem), [custom tray icons](https://github.com/piec/elecwhat/issues/30#issuecomment-3470456738).
* Custom keyboard shortcuts ([default](src/keys.rs))
* Custom CSS
* Spellcheck
* CLI & D-Bus interface to show/hide/toggle window
* Dock unread count badge
* Small and light: a single ~5 MB native binary written in Rust, using the system WebKitGTK (no bundled browser)
* Can be trusted:
  * Code easy to review
  * Automatically built by GitHub Actions (CI)
* Preserve window position and zoom level
* Custom user scripts in `~/.config/elecwhat/user-scripts/*.js`
* Custom tray icons in `~/.config/elecwhat/user-icons/*.png` by default
* Supports Wayland or X11

## Why?
I created this client after due to recurrent stability issues with other linux clients.
Also I just wanted a simple and stable client.

ElecWhat started as an Electron app; it is now written in Rust with
[tao](https://github.com/tauri-apps/tao) + [wry](https://github.com/tauri-apps/wry) (WebKitGTK),
[ksni](https://github.com/iovxw/ksni) for the tray icon and [zbus](https://github.com/dbus2/zbus) for D-Bus.

## Install

### From the releases
* .deb, .rpm or .tar.xz in [Releases]
* **or** [Buildkite deb] / [Buildkite rpm]

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

Then copy `target/release/elecwhat` to your `PATH`, `packaging/elecwhat.desktop` to
`~/.local/share/applications/` and `static/app-512.png` to
`~/.local/share/icons/hicolor/512x512/apps/elecwhat.png`.
Or build a package with [cargo-deb](https://github.com/kornelski/cargo-deb) (`cargo deb`) or
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm) (`cargo generate-rpm`).

### Voice/video calls
Calls depend on WebRTC support in your WebKitGTK version (2.40+ built with WebRTC,
plus the GStreamer plugins), and on WhatsApp Web accepting it. They may not work
on every distribution.

## Command line

```
elecwhat [--show | --hide | --toggle | --quit] [--log-level=LEVEL] [whatsapp://URL]
```

When ElecWhat is already running, the command is sent to it (single instance through D-Bus).

## D-Bus

Service `fr.carru.elecwhat`, object `/fr/carru/elecwhat`, interface `fr.carru.elecwhat`:
`Show`, `Hide`, `Visible → b`, `ToggleVisibility → b`, `OpenUrl(s)`, `Activate(s token)`, `Quit`.

```bash
gdbus call --session --dest fr.carru.elecwhat --object-path /fr/carru/elecwhat --method fr.carru.elecwhat.ToggleVisibility
```

## Configuration


`~/.config/elecwhat/config.json`:

```
{
  "log-level": "info",
  "notification-prefix": "elecwhat - ",

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
    "https://gist.githubusercontent.com/piec/3f9a198a7694f1df37895ee3faee9b6e/raw/whatsapp-test.css",
    "file://~/mysheet.css"
  ],

  "spellcheck": true,
  "spellcheck-languages": ["en-US", "fr"],

  "user-agent": "...",
  "open-dev-tools": false,

  "scripts": [
    # ~/.config/elecwhat/user-scripts/myscript.js
    "${userData}/user-scripts/myscripts.js"
  ],
  "icons-directory": "..."
}
```

## Custom icons

The naming scheme is based on WhatsApp Web favicon urls:
* `~/.config/elecwhat/user-icons/app.png` during startup
* `~/.config/elecwhat/user-icons/favicon.png` when there is no unread
* `~/.config/elecwhat/user-icons/f01.png` where fXX.png is the number of unread


## Keyboard shortcuts
Bindings are `"<modifiers> <key>"` (`C`=Ctrl, `S`=Shift, `A`=Alt, `M`=Meta) or just `"<key>"`,
`key` being a [KeyboardEvent.key](https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values) value.
Besides WhatsApp's own actions (`{"whatsappAction": "..."}`), `{"action": "IPC", "method": "..."}`
can call: `windowToggle`, `windowShow`, `windowHide`, `reload`, `quit`, `zoomIn`, `zoomOut`,
`zoomReset`, `fullscreenToggle`, `devTools`.

The defaults include `Ctrl+R` (reload), `Ctrl+Q` (quit), `Ctrl+=`/`Ctrl+-` (zoom), `F11` (fullscreen)
and `Ctrl+Shift+I` (developer tools), which used to come from the Electron menu bar.

## Data
* `~/.config/elecwhat/`: `config.json`, `persistent-state.json` (window position, zoom), `user-scripts/`, `user-icons/`
* `~/.local/share/elecwhat/webview/`: WhatsApp Web session (cookies, local storage, cache)

Coming from the Electron version, you'll have to link your device again (scan the QR code):
the browser profile can't be carried over. `config.json` is read as before;
`menu-bar` and `menu-bar-auto-hide` are no longer used (there is no menu bar, see the keyboard shortcuts).

## Not planed
* Multi account
* Complex features
* Other OS support, official apps already exist.

[Buildkite deb]: https://buildkite.com/organizations/piec/packages/registries/elecwhat-deb
[Buildkite rpm]: https://buildkite.com/organizations/piec/packages/registries/elecwhat-rpm
[Releases]: https://github.com/piec/elecwhat/releases
[AppIndicator]: https://extensions.gnome.org/extension/615/appindicator-support/
