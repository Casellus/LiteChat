//! Linux-specific integrations: single instance (Unix socket), XDG autostart,
//! Unity/KDE taskbar badge (D-Bus), and WebKitGTK hooks (permissions, context menu).

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// Injected before page scripts when GStreamer's `autoaudiosink` plugin
/// (`gst-plugins-good`) is missing. WebKitGTK 2.54+ aborts `WebKitWebProcess`
/// inside `MediaPlayerPrivateGStreamer::createAudioSink()` whenever an
/// `<audio>`/`<video>` element loads a source without `autoaudiosink`.
pub const SILENT_MEDIA_GUARD_JS: &str = r#"(() => {
  if (window.__litechatSilentMedia) return;
  window.__litechatSilentMedia = true;
  const noop = () => {};
  const resolved = () => Promise.resolve();
  class SilentAudio extends EventTarget {
    constructor(src) {
      super();
      this._src = src ? String(src) : '';
      this.volume = 1;
      this.muted = true;
      this.paused = true;
      this.ended = true;
      this.currentTime = 0;
      this.duration = 0;
      this.readyState = 4;
      this.preload = 'none';
    }
    get src() { return this._src; }
    set src(v) { this._src = v ? String(v) : ''; }
    play() { return Promise.resolve(); }
    pause() {}
    load() {}
    canPlayType() { return ''; }
  }
  try { window.Audio = SilentAudio; } catch (_) {}
  if (typeof HTMLMediaElement !== 'undefined') {
    const proto = HTMLMediaElement.prototype;
    try { proto.play = resolved; } catch (_) {}
    try { proto.pause = noop; } catch (_) {}
    try { proto.load = noop; } catch (_) {}
    try { proto.canPlayType = () => ''; } catch (_) {}
    try {
      Object.defineProperty(proto, 'src', {
        configurable: true,
        get() { return this._lcSrc || ''; },
        set(v) { this._lcSrc = v ? String(v) : ''; },
      });
    } catch (_) {}
    try {
      Object.defineProperty(proto, 'srcObject', {
        configurable: true,
        get() { return this._lcSrcObj || null; },
        set(v) { this._lcSrcObj = v; },
      });
    } catch (_) {}
  }
  if (typeof HTMLSourceElement !== 'undefined') {
    try {
      Object.defineProperty(HTMLSourceElement.prototype, 'src', {
        configurable: true,
        get() { return this._lcSrc || ''; },
        set(v) { this._lcSrc = v ? String(v) : ''; },
      });
    } catch (_) {}
  }
  const origSetAttr = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function(name, value) {
    if (
      String(name).toLowerCase() === 'src' &&
      (this instanceof HTMLMediaElement ||
        (typeof HTMLSourceElement !== 'undefined' && this instanceof HTMLSourceElement))
    ) {
      this._lcSrc = value ? String(value) : '';
      return;
    }
    return origSetAttr.call(this, name, value);
  };
  const origCreateEl = Document.prototype.createElement;
  Document.prototype.createElement = function(tagName, options) {
    const el = origCreateEl.call(this, tagName, options);
    if (typeof tagName === 'string' && tagName.toLowerCase() === 'audio') {
      el.preload = 'none';
      el.muted = true;
    }
    return el;
  };
})();"#;

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

fn launcher_entry_params(unread: u32) -> glib::Variant {
    let props = glib::VariantDict::new(None);
    props.insert("count", unread as i64);
    props.insert("count-visible", unread > 0);
    (format!("application://{DESKTOP_ID}"), props.end()).into()
}

impl TaskbarBadge {
    pub fn new() -> Self {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok();
        Self { bus }
    }

    pub fn set(&mut self, unread: u32) {
        let Some(bus) = &self.bus else { return };
        let params = launcher_entry_params(unread);
        let _ = bus.emit_signal(
            None,
            "/com/litechat/LiteChat",
            "com.canonical.Unity.LauncherEntry",
            "Update",
            Some(&params),
        );
    }
}

// ---------------------------------------------------------------------------
// WebKitGTK hooks (settings, permissions, context menu, memory)
// ---------------------------------------------------------------------------

/// Checks whether GStreamer's `autoaudiosink` plugin (`gst-plugins-good`) is
/// installed. WebKitGTK 2.54+ aborts the web process inside `createAudioSink()`
/// if a page plays audio while `autoaudiosink` is missing.
pub fn has_gstreamer_audio_sink() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(detect_gstreamer_audio_sink)
}

fn detect_gstreamer_audio_sink() -> bool {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in [
        "GST_PLUGIN_PATH_1_0",
        "GST_PLUGIN_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GST_PLUGIN_SYSTEM_PATH",
    ] {
        if let Some(val) = std::env::var_os(var) {
            dirs.extend(std::env::split_paths(&val));
        }
    }
    for default_dir in [
        "/usr/lib/gstreamer-1.0",
        "/usr/lib64/gstreamer-1.0",
        "/usr/lib/x86_64-linux-gnu/gstreamer-1.0",
        "/usr/lib/aarch64-linux-gnu/gstreamer-1.0",
        "/usr/local/lib/gstreamer-1.0",
    ] {
        dirs.push(PathBuf::from(default_dir));
    }

    if dirs
        .iter()
        .any(|d| Path::new(d).join("libgstautodetect.so").is_file())
    {
        return true;
    }

    // Fallback for non-standard layouts (e.g. NixOS): ask gst-inspect-1.0 if present.
    Command::new("gst-inspect-1.0")
        .arg("autoaudiosink")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

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
    let has_audio = has_gstreamer_audio_sink();
    if !has_audio {
        eprintln!(
            "LiteChat: GStreamer 'autoaudiosink' (gst-plugins-good) not found; \
             disabling WebKit media/WebAudio to prevent WebKitWebProcess crashes. \
             Install gst-plugins-good to enable sound, voice messages, and calls."
        );
    }

    if let Some(settings) = WebViewExt::settings(webview) {
        settings.set_user_agent(Some(USER_AGENT));
        settings.set_enable_webrtc(has_audio);
        settings.set_enable_media_stream(has_audio);
        settings.set_enable_media(has_audio);
        settings.set_enable_webaudio(has_audio);
        settings.set_javascript_can_access_clipboard(true);
        settings.set_enable_developer_extras(is_debug);

        let disable_gpu = std::env::var("LITECHAT_DISABLE_GPU")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        if disable_gpu {
            settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
        }
    }

    // Automatically reload if WebKitWebProcess crashes or hits a memory limit,
    // rate-limited so repeated crashes cannot trigger a reload loop.
    let last_reload = std::cell::Cell::new(None::<Instant>);
    webview.connect_web_process_terminated(move |wv, reason| {
        if matches!(
            reason,
            WebProcessTerminationReason::Crashed
                | WebProcessTerminationReason::ExceededMemoryLimit
        ) {
            let now = Instant::now();
            let allow = last_reload
                .get()
                .is_none_or(|prev| now.duration_since(prev) >= Duration::from_secs(10));
            if allow {
                last_reload.set(Some(now));
                wv.reload();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_entry_variant_signature_is_valid_dbus_asv() {
        let params = launcher_entry_params(3);
        assert_eq!(params.type_().as_str(), "(sa{sv})");
    }
}
