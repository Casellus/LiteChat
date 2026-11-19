<p align="center">
  <img src="assets/litechat.png" width="112" alt="LiteChat icon">
</p>

<h1 align="center">LiteChat</h1>

<p align="center">A featherweight desktop client for WhatsApp Web on Windows.</p>

---

LiteChat opens the **official WhatsApp Web** in a native window. It uses the WebView2 engine that already ships with Windows instead of bundling a whole browser. The design, settings and features are WhatsApp's own, and you sign in with a **QR code** just like in the regular app.

## Why

| | WhatsApp for Windows | LiteChat |
|---|---|---|
| Disk space | ~292 MB | **~0.7 MB** |
| RAM with chats open* | ~1,150–1,690 MB | **~450 MB** |

\* Measured on the same PC and the same account, with chats already synced. The first login uses more memory for a few minutes while the chat history downloads.

## Features

- QR code login with a **persistent session**: you scan the QR code only once.
- **System tray icon**: the close button hides the window and notifications keep arriving.
  - Left click on the icon: show or hide the window.
  - Right click: Open, Reload and Quit.
- **Unread badge** like the official app:
  - a number on the taskbar button;
  - a red dot on the tray icon;
  - the count in the window title, for example `(3) LiteChat`.
- The taskbar button **flashes** when a new message arrives while the window is in the background.
- **Native Windows notifications**: WhatsApp messages arrive as real Windows toasts instead of browser popups.
  - They show up in the Action Center and follow Focus / Do Not Disturb.
  - Notification widgets such as "dynamic island" apps can pick them up.
  - Clicking one opens LiteChat on that chat.
  - Reading the chat removes the notification.
- **Calls and voice messages** work without extra prompts: microphone, camera, notification and clipboard permissions are granted to WhatsApp only.
- **Single instance**: launching LiteChat again just shows the existing window.
- **Start with Windows** toggle in the tray menu. When started this way, it starts hidden in the tray.
- Window size, position and maximized state are **remembered**.
- **Clean right-click menu**: browser-only entries such as Back, Reload, Print and Save as are removed. Copy, paste and spell check stay.
- Drag and drop files into chats.
- External links open in your default browser.
- Automatic **low-memory mode** while the window is hidden.
- WebView2 tuning, with the effect of each option measured:

  | Option | Effect |
  |---|---|
  | `--disable-gpu` | GPU process drops from ~130 MB to ~18 MB |
  | `--js-flags=--lite-mode` | page drops from ~465 MB to ~330 MB |
  | Edge services disabled | no translator, shopping, sync, etc. |

## Installation

Requirements: Windows 10/11 with the **WebView2 Runtime**, which is built into Windows 11.

### From a release (no build needed)

1. Download `LiteChat-vX.Y.Z-windows-x64.zip` from the [Releases](https://github.com/Casellus/LiteChat/releases/latest) page and extract it.
2. In the extracted folder, run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

The executable is not code-signed. On first launch Windows SmartScreen may show a warning: click **More info → Run anyway**.

### From source

```powershell
git clone https://github.com/Casellus/LiteChat.git
cd LiteChat
cargo build --release
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

The script installs LiteChat for the current user only, without administrator rights:

- copies the app to `%LOCALAPPDATA%\Programs\LiteChat`;
- creates Start menu and Desktop shortcuts;
- adds LiteChat to **Settings → Installed apps**.

Options:

| Option | Effect |
|---|---|
| `-Autostart` | starts LiteChat with Windows |
| `-NoDesktopShortcut` | skips the Desktop shortcut |

### Uninstalling

Uninstall from **Settings → Apps → Installed apps → LiteChat**, or run:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\LiteChat\uninstall.ps1" -RemoveData
```

Without `-RemoveData` the session is kept in `%LOCALAPPDATA%\LiteChat`.

## Development

Requirements: [Rust](https://rustup.rs) (2024 edition) and the MSVC toolchain.

```powershell
cargo run                  # debug build, DevTools enabled
cargo build --release      # size-optimized executable
```

| Path | Contents |
|---|---|
| `src/main.rs` | window, WebView, tray and optimizations |
| `build.rs` | embeds the icon and version info into the `.exe` |
| `assets/` | icon; regenerate it with `tools\make-icon.ps1` |
| `scripts/` | install and uninstall scripts |

To try other WebView2 flags without rebuilding, set the `LITECHAT_EXTRA_ARGS` environment variable:

```powershell
$env:LITECHAT_EXTRA_ARGS = "--flag-to-try"; .\target\release\litechat.exe
```

## Disclaimer

LiteChat is **not affiliated with WhatsApp or Meta**. It only displays the official `web.whatsapp.com` website. It does not modify or intercept messages, which stay end-to-end encrypted. It does not use unofficial libraries, so there is no risk of your account being banned.

## License

[MIT](LICENSE)
