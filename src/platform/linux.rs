//! Linux-specific integrations: single instance (Unix socket), XDG autostart,
//! Unity/KDE taskbar badge (D-Bus), and WebKitGTK hooks (permissions, context menu).

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use glib::ObjectExt;
use glib::object::ObjectType;
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

/// Raises `RLIMIT_NOFILE` and applies Linux GPU/GStreamer workarounds at
/// process startup before GTK/WebKit initializes or spawns child processes.
pub fn init_process() {
    raise_nofile_limit();

    // On NVIDIA proprietary drivers, WebKitGTK's GStreamer GL video sink can
    // produce solid green (#008800, all-zero YUV) flashing frames for `<video>`
    // GIFs. Disabling only the GL sink lets WebKitGTK use its DMA-BUF/standard
    // video path cleanly while keeping full GPU acceleration for the WebView.
    if Path::new("/proc/driver/nvidia/version").exists()
        && std::env::var_os("WEBKIT_GST_DISABLE_GL_SINK").is_none()
    {
        // SAFETY: called at process startup before any threads are spawned.
        unsafe {
            std::env::set_var("WEBKIT_GST_DISABLE_GL_SINK", "1");
        }
    }
}

fn raise_nofile_limit() {
    const TARGET_NOFILE: libc::rlim_t = 65_536;
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `lim` is a valid mutable `libc::rlimit` struct.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0 {
        let desired = if lim.rlim_max == libc::RLIM_INFINITY {
            TARGET_NOFILE
        } else {
            lim.rlim_max.min(TARGET_NOFILE)
        };
        if lim.rlim_cur < desired {
            lim.rlim_cur = desired;
            // SAFETY: raising soft limit up to `rlim_max` requires no extra privileges.
            let _ = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        }
    }
}

/// Smooths discrete mouse-wheel scrolling on overflow containers (such as
/// WhatsApp Web's virtualized chat list and message pane), where WebKitGTK
/// otherwise jumps in abrupt stair-steps. Touchpad gestures, Ctrl/Meta zoom,
/// horizontal scroll, and programmatic scroll jumps are left untouched.
pub const SMOOTH_SCROLL_JS: &str = r#"(() => {
  if (window.__litechatSmoothScroll) return;
  window.__litechatSmoothScroll = true;

  const states = new WeakMap();
  const active = new Set();
  let rafId = 0;
  let lastTime = 0;

  const findScrollable = (el, dy) => {
    let cur = el;
    while (cur && cur !== document.documentElement && cur !== document.body) {
      if (cur.nodeType === 1 && cur.scrollHeight > cur.clientHeight + 1) {
        const oy = getComputedStyle(cur).overflowY;
        if (oy === 'auto' || oy === 'scroll' || oy === 'overlay') {
          const max = cur.scrollHeight - cur.clientHeight;
          if ((dy > 0 && cur.scrollTop < max - 0.5) || (dy < 0 && cur.scrollTop > 0.5)) {
            return cur;
          }
        }
      }
      cur = cur.parentElement;
    }
    return null;
  };

  const step = (now) => {
    const dt = lastTime ? Math.min(0.05, Math.max(0.001, (now - lastTime) / 1000)) : 1 / 60;
    lastTime = now;
    // Critically-damped exponential glide (~130ms settle, framerate-independent).
    const factor = 1 - Math.exp(-dt * 18);

    for (const el of active) {
      const st = states.get(el);
      if (!st || !el.isConnected) {
        active.delete(el);
        continue;
      }
      // If WhatsApp or the user's scrollbar moved scrollTop externally, stop smoothly.
      if (Math.abs(el.scrollTop - st.lastApplied) > 2) {
        active.delete(el);
        continue;
      }
      const max = el.scrollHeight - el.clientHeight;
      st.target = Math.max(0, Math.min(max, st.target));
      const diff = st.target - st.current;
      if (Math.abs(diff) < 0.4) {
        st.current = st.target;
        el.scrollTop = st.target;
        st.lastApplied = el.scrollTop;
        active.delete(el);
      } else {
        st.current += diff * factor;
        el.scrollTop = st.current;
        st.lastApplied = el.scrollTop;
      }
    }

    if (active.size > 0) {
      rafId = requestAnimationFrame(step);
    } else {
      rafId = 0;
      lastTime = 0;
    }
  };

  window.addEventListener(
    'wheel',
    (e) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.shiftKey) return;
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || e.deltaY === 0) return;

      let dy = e.deltaY;
      if (e.deltaMode === 1) dy *= 32;
      else if (e.deltaMode === 2) dy *= window.innerHeight * 0.85;

      // High-precision touchpads emit small incremental pixel deltas: let WebKit handle them directly.
      if (e.deltaMode === 0 && Math.abs(dy) < 20) return;

      const target = findScrollable(e.target, dy);
      if (!target) return;

      e.preventDefault();
      let st = states.get(target);
      if (!st || !active.has(target) || Math.abs(target.scrollTop - st.lastApplied) > 2) {
        st = {
          current: target.scrollTop,
          target: target.scrollTop,
          lastApplied: target.scrollTop,
        };
        states.set(target, st);
      }
      const max = target.scrollHeight - target.clientHeight;
      st.target = Math.max(0, Math.min(max, st.target + dy));
      active.add(target);
      if (!rafId) {
        lastTime = 0;
        rafId = requestAnimationFrame(step);
      }
    },
    { passive: false, capture: true }
  );
})();"#;

/// Pauses off-screen muted looping `<video>` previews (WhatsApp's GIF picker
/// and animated stickers) and resumes them when scrolled into view. Uses a
/// capture-phase `play` listener instead of a document-wide `MutationObserver`
/// so scrolling virtualized chat lists incurs zero DOM observer overhead.
pub const GIF_THROTTLE_JS: &str = r#"(() => {
  if (window.__litechatGifThrottle) return;
  window.__litechatGifThrottle = true;

  const tracked = new WeakSet();
  const visible = new Set();

  const isLoopingGif = (el) => {
    if (!el || el.tagName !== 'VIDEO' || !el.loop || !el.muted || el.controls) return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.width <= 320 && rect.height <= 320;
  };

  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const video = entry.target;
        if (!video.isConnected) {
          visible.delete(video);
          observer.unobserve(video);
          continue;
        }
        if (entry.isIntersecting) {
          visible.add(video);
          if (!document.hidden && video.paused && isLoopingGif(video)) {
            video.play().catch(() => {});
          }
        } else {
          visible.delete(video);
          if (!video.paused && isLoopingGif(video)) {
            video.pause();
          }
        }
      }
    },
    { rootMargin: '120px' }
  );

  document.addEventListener(
    'play',
    (e) => {
      const el = e.target;
      if (!isLoopingGif(el) || tracked.has(el)) return;
      tracked.add(el);
      observer.observe(el);
    },
    true
  );

  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      for (const v of visible) {
        if (!v.paused && isLoopingGif(v)) v.pause();
      }
    } else {
      for (const v of visible) {
        if (!v.isConnected) {
          visible.delete(v);
        } else if (v.paused && isLoopingGif(v)) {
          v.play().catch(() => {});
        }
      }
    }
  });
})();"#;

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

/// Applies WebKitGTK 2.42+ feature flags via `dlsym` so older distros still link
/// cleanly while modern WebKitGTK unlocks native high-refresh-rate rendering
/// (120/144 Hz+), GPU compositing, and threaded async scrolling.
fn apply_performance_features(settings: &webkit2gtk::Settings, enable_gpu: bool) {
    type GetAllFeaturesFn = unsafe extern "C" fn() -> *mut c_void;
    type FeatureListLenFn = unsafe extern "C" fn(*mut c_void) -> usize;
    type FeatureListGetFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
    type FeatureListUnrefFn = unsafe extern "C" fn(*mut c_void);
    type FeatureGetIdentFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type SetFeatureEnabledFn = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int);

    unsafe fn sym(name: &CStr) -> *mut c_void {
        // SAFETY: `name` is a valid NUL-terminated static C string.
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
    }

    // SAFETY: Each symbol is resolved by exact WebKitGTK C ABI name from the
    // already-loaded `libwebkit2gtk-4.1.so.0` and checked for non-null before calling.
    unsafe {
        let p_get_all = sym(c"webkit_settings_get_all_features");
        let p_len = sym(c"webkit_feature_list_get_length");
        let p_get = sym(c"webkit_feature_list_get");
        let p_unref = sym(c"webkit_feature_list_unref");
        let p_ident = sym(c"webkit_feature_get_identifier");
        let p_set = sym(c"webkit_settings_set_feature_enabled");

        if p_get_all.is_null()
            || p_len.is_null()
            || p_get.is_null()
            || p_unref.is_null()
            || p_ident.is_null()
            || p_set.is_null()
        {
            return;
        }

        let get_all: GetAllFeaturesFn = std::mem::transmute(p_get_all);
        let list_len: FeatureListLenFn = std::mem::transmute(p_len);
        let list_get: FeatureListGetFn = std::mem::transmute(p_get);
        let list_unref: FeatureListUnrefFn = std::mem::transmute(p_unref);
        let get_ident: FeatureGetIdentFn = std::mem::transmute(p_ident);
        let set_enabled: SetFeatureEnabledFn = std::mem::transmute(p_set);

        let list = get_all();
        if list.is_null() {
            return;
        }

        let desired: &[(&str, bool)] = if enable_gpu {
            &[
                // Unlock native 120Hz/144Hz+ refresh rate instead of throttling to 60fps.
                ("PreferPageRenderingUpdatesNear60FPS", false),
                ("AsyncFrameScrolling", true),
                ("AsyncOverflowScrolling", true),
                ("ThreadedScrolling", true),
                ("MomentumScrollingAnimator", true),
                ("ScrollAnimator", true),
                ("UseDamagingInformationForCompositing", true),
                ("AnimatedImageAsyncDecoding", true),
                ("CanvasUsesAcceleratedDrawing", true),
            ]
        } else {
            &[
                ("PreferPageRenderingUpdatesNear60FPS", false),
                ("AsyncFrameScrolling", true),
                ("AsyncOverflowScrolling", true),
                ("ThreadedScrolling", true),
                ("MomentumScrollingAnimator", true),
                ("ScrollAnimator", true),
                ("AnimatedImageAsyncDecoding", true),
            ]
        };

        let settings_ptr = settings.as_ptr() as *mut c_void;
        let n = list_len(list);
        for i in 0..n {
            let feat = list_get(list, i);
            if feat.is_null() {
                continue;
            }
            let ident_ptr = get_ident(feat);
            if ident_ptr.is_null() {
                continue;
            }
            if let Ok(ident) = CStr::from_ptr(ident_ptr).to_str()
                && let Some((_, enabled)) = desired.iter().find(|(k, _)| *k == ident)
            {
                set_enabled(settings_ptr, feat, i32::from(*enabled));
            }
        }
        list_unref(list);
    }
}

/// Tunes WebKitGTK settings for WhatsApp Web (User-Agent, high-FPS GPU
/// compositing, smooth threaded scrolling, WebRTC/media, optional GPU disable,
/// and automatic recovery if the web process terminates).
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
        let disable_gpu = std::env::var("LITECHAT_DISABLE_GPU")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

        settings.set_user_agent(Some(USER_AGENT));
        settings.set_enable_webrtc(has_audio);
        settings.set_enable_media_stream(has_audio);
        settings.set_enable_media(has_audio);
        settings.set_enable_webaudio(has_audio);
        settings.set_media_playback_requires_user_gesture(false);
        settings.set_media_playback_allows_inline(true);
        settings.set_javascript_can_access_clipboard(true);
        settings.set_enable_smooth_scrolling(true);
        settings.set_enable_page_cache(true);
        settings.set_enable_webgl(!disable_gpu);
        settings.set_hardware_acceleration_policy(if disable_gpu {
            HardwareAccelerationPolicy::Never
        } else {
            HardwareAccelerationPolicy::Always
        });
        settings.set_enable_developer_extras(is_debug);

        apply_performance_features(&settings, !disable_gpu);
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

// ---------------------------------------------------------------------------
// File drag-and-drop bridge (Wayland + bwrap sandbox)
// ---------------------------------------------------------------------------

const MAX_DROP_FILE_BYTES: u64 = 128 * 1024 * 1024;
const DROP_CHUNK_BYTES: usize = 3 * 1024 * 1024; // multiple of 3 for clean base64 chunks

fn mime_from_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("3gp") => "video/3gpp",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn js_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Injects local files dropped onto the GTK window as a synthetic HTML5 `drop`
/// event with real `File` objects. Needed on Wayland + WebKitGTK `bwrap`
/// sandbox, where `WebKitWebProcess` cannot read arbitrary paths from Dolphin.
pub fn inject_dropped_files(webview: &wry::WebView, paths: &[PathBuf], position: (i32, i32)) {
    let mut valid_files = Vec::new();
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else { continue };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_DROP_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else { continue };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let mime = mime_from_path(path);
        valid_files.push((name, mime, bytes));
    }
    if valid_files.is_empty() {
        return;
    }

    let _ = webview.evaluate_script("window.__lcDropFiles = [];");
    for (name, mime, bytes) in valid_files {
        let _ = webview.evaluate_script("window.__lcDropChunks = [];");
        for chunk in bytes.chunks(DROP_CHUNK_BYTES) {
            let b64 = encode_base64(chunk);
            let _ = webview.evaluate_script(&format!("window.__lcDropChunks.push('{b64}');"));
        }
        let finalize_file = format!(
            "(() => {{\
               const parts = window.__lcDropChunks.map((b64) => {{\
                 const bin = atob(b64);\
                 const arr = new Uint8Array(bin.length);\
                 for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);\
                 return arr;\
               }});\
               window.__lcDropChunks = [];\
               window.__lcDropFiles.push(new File(parts, {}, {{ type: {} }}));\
             }})();",
            js_escape_string(&name),
            js_escape_string(mime),
        );
        let _ = webview.evaluate_script(&finalize_file);
    }

    let (x, y) = position;
    let dispatch = format!(
        "(() => {{\
           const files = window.__lcDropFiles || [];\
           window.__lcDropFiles = [];\
           if (!files.length) return;\
           const dt = new DataTransfer();\
           for (const f of files) dt.items.add(f);\
           const el = document.elementFromPoint({x}, {y})\
             || document.querySelector('#main')\
             || document.body;\
           if (!el) return;\
           const opts = {{ bubbles: true, cancelable: true, dataTransfer: dt, clientX: {x}, clientY: {y} }};\
           el.dispatchEvent(new DragEvent('dragenter', opts));\
           el.dispatchEvent(new DragEvent('dragover', opts));\
           el.dispatchEvent(new DragEvent('drop', opts));\
         }})();"
    );
    let _ = webview.evaluate_script(&dispatch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_entry_variant_signature_is_valid_dbus_asv() {
        let params = launcher_entry_params(3);
        assert_eq!(params.type_().as_str(), "(sa{sv})");
    }

    #[test]
    fn nofile_limit_is_raised_above_default() {
        raise_nofile_limit();
        let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
        if lim.rlim_max >= 4096 {
            assert!(lim.rlim_cur >= 4096, "rlim_cur was {}", lim.rlim_cur);
        }
    }

    #[test]
    fn base64_and_mime_work() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        assert_eq!(mime_from_path(Path::new("/tmp/clip.MP4")), "video/mp4");
        assert_eq!(mime_from_path(Path::new("/tmp/photo.webp")), "image/webp");
    }

    #[test]
    fn performance_features_unlock_high_refresh_rate() {
        if gtk::init().is_err() {
            return;
        }
        let settings = webkit2gtk::Settings::new();
        apply_performance_features(&settings, true);

        type GetAllFn = unsafe extern "C" fn() -> *mut c_void;
        type LenFn = unsafe extern "C" fn(*mut c_void) -> usize;
        type GetFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
        type UnrefFn = unsafe extern "C" fn(*mut c_void);
        type IdentFn = unsafe extern "C" fn(*mut c_void) -> *const c_char;
        type GetEnabledFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

        unsafe {
            let p_get_all = libc::dlsym(libc::RTLD_DEFAULT, c"webkit_settings_get_all_features".as_ptr());
            if p_get_all.is_null() {
                return; // WebKitGTK < 2.42
            }
            let get_all: GetAllFn = std::mem::transmute(p_get_all);
            let list_len: LenFn = std::mem::transmute(libc::dlsym(libc::RTLD_DEFAULT, c"webkit_feature_list_get_length".as_ptr()));
            let list_get: GetFn = std::mem::transmute(libc::dlsym(libc::RTLD_DEFAULT, c"webkit_feature_list_get".as_ptr()));
            let list_unref: UnrefFn = std::mem::transmute(libc::dlsym(libc::RTLD_DEFAULT, c"webkit_feature_list_unref".as_ptr()));
            let get_ident: IdentFn = std::mem::transmute(libc::dlsym(libc::RTLD_DEFAULT, c"webkit_feature_get_identifier".as_ptr()));
            let get_enabled: GetEnabledFn = std::mem::transmute(libc::dlsym(libc::RTLD_DEFAULT, c"webkit_settings_get_feature_enabled".as_ptr()));

            let list = get_all();
            let settings_ptr = settings.as_ptr() as *mut c_void;
            let mut checked_60fps = false;
            for i in 0..list_len(list) {
                let feat = list_get(list, i);
                let ident = CStr::from_ptr(get_ident(feat)).to_str().unwrap_or("");
                if ident == "PreferPageRenderingUpdatesNear60FPS" {
                    assert_eq!(get_enabled(settings_ptr, feat), 0);
                    checked_60fps = true;
                }
            }
            list_unref(list);
            assert!(checked_60fps, "PreferPageRenderingUpdatesNear60FPS feature not found");
        }
    }
}
