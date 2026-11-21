//! Linux-specific integrations: single instance (Unix socket), XDG autostart,
//! Unity/KDE taskbar badge (D-Bus), and WebKitGTK hooks (permissions, context menu).

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;

use glib::ObjectExt;
use webkit2gtk::{
    ContextMenuAction, ContextMenuExt, ContextMenuItemExt, HardwareAccelerationPolicy,
    NotificationPermissionRequest, PermissionRequestExt, SettingsExt,
    UserMediaPermissionRequest, WebProcessTerminationReason, WebViewExt,
};

const APP_NAME: &str = "LiteChat";
const DESKTOP_ID: &str = "litechat.desktop";

/// Chrome-on-Linux User-Agent so WhatsApp Web does not reject WebKitGTK.
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// Sets WebKitGTK environment workarounds before GTK/WebKit initializes.
/// Must be called at the very start of `main()` before any threads are spawned.
pub fn init_env() {
    // WebKitGTK's DMA-BUF renderer freezes after idling or hiding on Wayland
    // (especially on NVIDIA). Disable it by default on Wayland unless the user
    // explicitly configured WEBKIT_DISABLE_DMABUF_RENDERER.
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        // SAFETY: called at process startup before any threads are spawned.
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}

// ---------------------------------------------------------------------------
// Data directory & external links
// ---------------------------------------------------------------------------

pub fn data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join(APP_NAME);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home).join(".local/share").join(APP_NAME);
    }
    std::env::temp_dir().join(APP_NAME)
}

pub fn open_external(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

/// Ensures the app icon and `.desktop` entry exist in `$XDG_DATA_HOME` so
/// Wayland compositors (KDE Plasma, GNOME) display the exact LiteChat icon on
/// the taskbar and Alt+Tab switcher even when running the binary directly.
pub fn ensure_desktop_integration(icon_png: &[u8]) {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".local/share"))
        });
    let Some(data_home) = data_home else { return };

    let icon_dir = data_home.join("icons/hicolor/256x256/apps");
    let icon_path = icon_dir.join("litechat.png");
    if !icon_path.exists() {
        let _ = std::fs::create_dir_all(&icon_dir);
        let _ = std::fs::write(&icon_path, icon_png);
    }

    let app_dir = data_home.join("applications");
    let desktop_path = app_dir.join(DESKTOP_ID);
    if !desktop_path.exists()
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = std::fs::create_dir_all(&app_dir);
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Version=1.0\n\
             Name={APP_NAME}\n\
             Comment=Lightweight desktop client for WhatsApp Web\n\
             Exec=\"{}\"\n\
             Icon=litechat\n\
             Terminal=false\n\
             Categories=Network;InstantMessaging;\n\
             StartupWMClass=litechat\n\
             StartupNotify=true\n",
            exe.display()
        );
        let _ = std::fs::write(&desktop_path, desktop);
    }

    gtk::Window::set_default_icon_name("litechat");
}

// ---------------------------------------------------------------------------
// Single instance (Unix domain socket)
// ---------------------------------------------------------------------------

fn socket_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(runtime).join("litechat.sock");
    }
    std::env::temp_dir().join("litechat.sock")
}

/// Returns `false` if another LiteChat is already running (it has been asked to
/// show its window, so this process should exit). Otherwise calls `on_show`
/// from a background thread every time a second instance is launched.
pub fn single_instance(on_show: impl Fn() + Send + 'static) -> bool {
    let path = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let _ = stream.write_all(b"show\n");
        return false;
    }

    // Stale socket from a previous crash: remove and bind fresh.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return true; // cannot tell: just run
    };

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            drop(stream);
            on_show();
        }
    });
    true
}

// ---------------------------------------------------------------------------
// Start on login (~/.config/autostart/litechat.desktop)
// ---------------------------------------------------------------------------

fn autostart_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("autostart").join(DESKTOP_ID));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".config/autostart").join(DESKTOP_ID))
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.exists())
}

pub fn set_autostart(enabled: bool) {
    let Some(path) = autostart_path() else { return };
    if enabled {
        let Ok(exe) = std::env::current_exe() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Version=1.0\n\
             Name={APP_NAME}\n\
             Comment=Lightweight desktop client for WhatsApp Web\n\
             Exec=\"{}\" --hidden\n\
             Icon=litechat\n\
             Terminal=false\n\
             Categories=Network;InstantMessaging;\n\
             StartupWMClass=litechat\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        let _ = std::fs::write(&path, desktop);
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

// ---------------------------------------------------------------------------
// Unread badge (Unity / KDE Plasma LauncherEntry D-Bus signal)
// ---------------------------------------------------------------------------

pub struct TaskbarBadge {
    bus: Option<gio::DBusConnection>,
}

impl TaskbarBadge {
    pub fn new() -> Self {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok();
        Self { bus }
    }

    pub fn set(&mut self, unread: u32) {
        let Some(bus) = &self.bus else { return };
        let mut props: HashMap<&str, glib::Variant> = HashMap::new();
        props.insert("count", (unread as i64).into());
        props.insert("count-visible", (unread > 0).into());
        let params = (format!("application://{DESKTOP_ID}"), props);
        let _ = bus.emit_signal(
            None,
            "/com/litechat/LiteChat",
            "com.canonical.Unity.LauncherEntry",
            "Update",
            Some(&params.into()),
        );
    }
}

// ---------------------------------------------------------------------------
// WebKitGTK hooks (settings, permissions, context menu, memory)
// ---------------------------------------------------------------------------

fn is_whatsapp(uri: &str) -> bool {
    let host = uri
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', ':', '?', '#']).next())
        .unwrap_or("");
    host == "whatsapp.com" || host.ends_with(".whatsapp.com")
}

/// Tunes WebKitGTK settings for WhatsApp Web (User-Agent, WebRTC, media stream,
/// optional GPU disable, and automatic recovery if the web process terminates).
pub fn configure_webview(webview: &webkit2gtk::WebView, is_debug: bool) {
    if let Some(settings) = WebViewExt::settings(webview) {
        settings.set_user_agent(Some(USER_AGENT));
        settings.set_enable_webrtc(true);
        settings.set_enable_media_stream(true);
        settings.set_javascript_can_access_clipboard(true);
        settings.set_enable_developer_extras(is_debug);

        let disable_gpu = std::env::var("LITECHAT_DISABLE_GPU")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        if disable_gpu {
            settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
        }
    }

    // Automatically reload if WebKitWebProcess crashes or hits a memory limit while idle.
    webview.connect_web_process_terminated(|wv, reason| {
        if matches!(
            reason,
            WebProcessTerminationReason::Crashed
                | WebProcessTerminationReason::ExceededMemoryLimit
        ) {
            wv.reload();
        }
    });
}

/// Grants notifications, microphone and camera to WhatsApp only, so calls and
/// voice messages work without extra prompts (like the official app).
pub fn auto_grant_permissions(webview: &webkit2gtk::WebView) {
    webview.connect_permission_request(|wv, req| {
        let uri = wv.uri().map(|u| u.to_string()).unwrap_or_default();
        if !is_whatsapp(&uri) {
            return false;
        }
        let allowed = req.is::<NotificationPermissionRequest>()
            || req.is::<UserMediaPermissionRequest>();
        if allowed {
            req.allow();
            true
        } else {
            false
        }
    });
}

/// Browser-only entries removed from the right-click menu. Copy/paste,
/// spell-check, image and link actions are kept.
pub fn clean_context_menu(webview: &webkit2gtk::WebView, is_debug: bool) {
    webview.connect_context_menu(move |_wv, menu, _event, _hit| {
        let items = menu.items();
        for item in &items {
            let action = item.stock_action();
            let hide = matches!(
                action,
                ContextMenuAction::GoBack
                    | ContextMenuAction::GoForward
                    | ContextMenuAction::Reload
                    | ContextMenuAction::Stop
                    | ContextMenuAction::OpenLinkInNewWindow
                    | ContextMenuAction::DownloadLinkToDisk
                    | ContextMenuAction::DownloadImageToDisk
                    | ContextMenuAction::OpenImageInNewWindow
                    | ContextMenuAction::OpenFrameInNewWindow
            ) || (!is_debug && action == ContextMenuAction::InspectElement);
            if hide {
                menu.remove(item);
            }
        }

        // Tidy up separators left at the edges or doubled.
        let remaining = menu.items();
        let len = remaining.len();
        let mut prev_sep = true;
        for (i, item) in remaining.iter().enumerate() {
            let is_sep = item.is_separator();
            if is_sep && (prev_sep || i + 1 == len) {
                menu.remove(item);
                continue;
            }
            prev_sep = is_sep;
        }
        // Remove trailing separator if the last kept item was a separator.
        let final_items = menu.items();
        if let Some(last) = final_items.last()
            && last.is_separator()
        {
            menu.remove(last);
        }
        false
    });
}

/// Trigger a redraw when restoring the window from the tray.
/// Avoids calling `webview.set_visible(false)` while hidden, which tears down
/// the Wayland surface and causes WebKitGTK to freeze after idling.
pub fn set_low_memory(webview: &webkit2gtk::WebView, low: bool) {
    use gtk::prelude::WidgetExt;
    if !low {
        webview.queue_draw();
    }
}
