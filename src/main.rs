// LiteChat: a lightweight desktop client for WhatsApp Web.
// Uses the OS native WebView (WebKitGTK on Linux, WebView2 on Windows)
// instead of bundling a whole browser.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod badge;
mod platform;
mod toast;
mod tray;

use std::path::{Path, PathBuf};

use tao::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    window::{Icon, UserAttentionType, Window, WindowBuilder},
};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

#[cfg(target_os = "windows")]
use tao::platform::windows::WindowExtWindows;
#[cfg(target_os = "windows")]
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};

#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
#[cfg(target_os = "linux")]
use wry::{WebViewBuilderExtUnix, WebViewExtUnix};

use tray::{TrayCommand, TrayHandle};

const APP_NAME: &str = "LiteChat";
const WHATSAPP_URL: &str = "https://web.whatsapp.com/";

#[cfg(target_os = "windows")]
const OLD_APP_NAME: &str = "Talkapp";

/// Extra WebView2 flags (Windows).
#[cfg(target_os = "windows")]
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,\
msEdgeTranslate,msShoppingTrigger,msEdgeCollections,AutofillServerCommunication \
--disable-background-networking --disable-component-update --disable-sync \
--no-first-run --disable-default-apps \
--disable-gpu --disable-gpu-compositing --js-flags=--lite-mode";

/// Clicking a WhatsApp notification brings the LiteChat window to the front.
const NOTIFICATION_CLICK_JS: &str = r#"(() => {
  const Native = window.Notification;
  if (!Native || Native.__litechat) return;
  class LiteChatNotification extends Native {
    constructor(title, options) {
      super(title, options);
      this.addEventListener('click', () => window.ipc.postMessage('show'));
    }
  }
  LiteChatNotification.__litechat = true;
  window.Notification = LiteChatNotification;
})();"#;

/// 64x64 RGBA icon (white feather in a green bubble).
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon64.rgba");
const ICON_SIZE: u32 = 64;

enum UserEvent {
    Tray(TrayCommand),
    Title(String),
    Show,
    ToastClicked(String),
}

/// Domains that stay inside the app; everything else opens in the browser.
fn is_internal(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or("");
    url.starts_with("about:")
        || url.starts_with("blob:")
        || url.starts_with("data:")
        || host == "whatsapp.com"
        || host.ends_with(".whatsapp.com")
        || host.ends_with(".whatsapp.net")
}

#[cfg(target_os = "windows")]
fn open_external(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
}

#[cfg(target_os = "linux")]
fn open_external(url: &str) {
    platform::open_external(url);
}

#[cfg(target_os = "windows")]
fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_NAME);
    let old = base.join(OLD_APP_NAME);
    if !dir.exists() && old.exists() {
        let _ = std::fs::rename(&old, &dir);
    }
    dir
}

#[cfg(target_os = "linux")]
fn data_dir() -> PathBuf {
    platform::data_dir()
}

#[cfg(target_os = "windows")]
fn set_low_memory(webview: &WebView, low: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows_core::Interface;

    if let Ok(wv) = webview.webview().cast::<ICoreWebView2_19>() {
        let level = if low {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        unsafe {
            let _ = wv.SetMemoryUsageTargetLevel(level);
        }
    }
}

#[cfg(target_os = "linux")]
fn set_low_memory(webview: &WebView, low: bool) {
    platform::set_low_memory(&webview.webview(), low);
}

/// Parses the unread chat count from WhatsApp Web's title, e.g. "(3) WhatsApp".
fn unread_from_title(title: &str) -> u32 {
    title
        .strip_prefix('(')
        .and_then(|t| t.split(')').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

// --- Window position and size, remembered between launches -----------------

struct WindowState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    maximized: bool,
}

fn load_window_state(path: &Path, target: &EventLoopWindowTarget<UserEvent>) -> Option<WindowState> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Vec<i64> = text.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    let [x, y, w, h, max] = v[..] else { return None };
    let s = WindowState {
        x: x as i32,
        y: y as i32,
        w: w as u32,
        h: h as u32,
        maximized: max != 0,
    };
    // Only restore if the title bar would be on a connected monitor (or if
    // monitors aren't reported, e.g. some Wayland compositors).
    let mut monitors = target.available_monitors().peekable();
    let visible = if monitors.peek().is_none() {
        true
    } else {
        monitors.any(|m| {
            let (p, size) = (m.position(), m.size());
            let (cx, cy) = (s.x + 80, s.y + 16);
            cx >= p.x && cy >= p.y && cx < p.x + size.width as i32 && cy < p.y + size.height as i32
        })
    };
    (visible && s.w >= 300 && s.h >= 300).then_some(s)
}

fn save_window_state(path: &Path, window: &Window) {
    if window.is_minimized() || !window.is_visible() {
        return;
    }
    let maximized = window.is_maximized();
    // When maximized keep the last normal bounds already on disk.
    if maximized && let Ok(text) = std::fs::read_to_string(path) {
        let mut parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() == 5 {
            parts[4] = "1";
            let _ = std::fs::write(path, parts.join(" "));
            return;
        }
    }
    let size = window.inner_size();
    let (x, y) = window.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
    let _ = std::fs::write(
        path,
        format!("{x} {y} {} {} {}", size.width, size.height, maximized as u8),
    );
}

fn main() -> wry::Result<()> {
    #[cfg(target_os = "linux")]
    platform::init_process();

    // Set GTK program name so Wayland/X11 match litechat.desktop (icon + taskbar badge).
    #[cfg(target_os = "linux")]
    glib::set_prgname(Some("litechat"));

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // A second launch just shows the window of the instance already running.
    let proxy = event_loop.create_proxy();
    if !platform::single_instance(move || {
        let _ = proxy.send_event(UserEvent::Show);
    }) {
        return Ok(());
    }

    // Started by autostart: go straight to the tray.
    let start_hidden = std::env::args().any(|a| a == "--hidden");

    let data = data_dir();
    let _ = std::fs::create_dir_all(&data);
    let state_file = data.join("window.txt");
    let icon_png = include_bytes!("../assets/litechat.png");
    toast::register(icon_png, &data);
    #[cfg(target_os = "linux")]
    platform::ensure_desktop_integration(icon_png);

    let mut builder = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_visible(!start_hidden)
        .with_min_inner_size(LogicalSize::new(480.0, 400.0))
        .with_window_icon(Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).ok());
    builder = match load_window_state(&state_file, &event_loop) {
        Some(s) => builder
            .with_position(PhysicalPosition::new(s.x, s.y))
            .with_inner_size(PhysicalSize::new(s.w, s.h))
            .with_maximized(s.maximized),
        None => builder.with_inner_size(LogicalSize::new(1100.0, 760.0)),
    };
    let window = builder.build(&event_loop).expect("failed to create the window");

    // System tray
    let tray_proxy = event_loop.create_proxy();
    let tray = TrayHandle::new(
        APP_NAME,
        ICON_RGBA,
        ICON_SIZE,
        platform::autostart_enabled(),
        move |cmd| {
            let _ = tray_proxy.send_event(UserEvent::Tray(cmd));
        },
    );

    // Persistent profile: the QR code only needs to be scanned once.
    let mut web_context = WebContext::new(Some(data));
    let title_proxy = event_loop.create_proxy();
    let ipc_proxy = event_loop.create_proxy();

    let mut wv_builder = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_devtools(cfg!(debug_assertions))
        .with_initialization_script_for_main_only(NOTIFICATION_CLICK_JS, true)
        .with_ipc_handler(move |req| {
            if req.body() == "show" {
                let _ = ipc_proxy.send_event(UserEvent::Show);
            }
        })
        .with_navigation_handler(|url| {
            if is_internal(&url) {
                true
            } else {
                open_external(&url);
                false
            }
        })
        .with_new_window_req_handler(|url, _features| {
            open_external(&url);
            NewWindowResponse::Deny
        })
        .with_document_title_changed_handler(move |title| {
            let _ = title_proxy.send_event(UserEvent::Title(title));
        });

    #[cfg(target_os = "linux")]
    {
        wv_builder = wv_builder.with_initialization_script(platform::SMOOTH_SCROLL_JS);
        if !platform::has_gstreamer_audio_sink() {
            wv_builder = wv_builder.with_initialization_script(platform::SILENT_MEDIA_GUARD_JS);
        } else {
            wv_builder = wv_builder.with_initialization_script(platform::GIF_THROTTLE_JS);
        }
    }

    #[cfg(target_os = "windows")]
    let webview = wv_builder
        .with_url(WHATSAPP_URL)
        .with_additional_browser_args(match std::env::var("LITECHAT_EXTRA_ARGS") {
            Ok(extra) => format!("{BROWSER_ARGS} {extra}"),
            Err(_) => BROWSER_ARGS.to_string(),
        })
        .build(&window)?;

    #[cfg(target_os = "linux")]
    let webview = {
        let vbox = window.default_vbox().expect("failed to get GTK default vbox");
        wv_builder.build_gtk(vbox)?
    };

    #[cfg(target_os = "windows")]
    {
        let core = webview.webview();
        platform::auto_grant_permissions(&core);
        platform::clean_context_menu(&core);
        let toast_proxy = event_loop.create_proxy();
        toast::hook(&core, move |tag| {
            let _ = toast_proxy.send_event(UserEvent::ToastClicked(tag));
        });
    }

    #[cfg(target_os = "linux")]
    {
        let core = webview.webview();
        platform::configure_webview(&core, cfg!(debug_assertions));
        platform::auto_grant_permissions(&core);
        platform::clean_context_menu(&core, cfg!(debug_assertions));
        let toast_proxy = event_loop.create_proxy();
        toast::hook(&core, move |tag| {
            let _ = toast_proxy.send_event(UserEvent::ToastClicked(tag));
        });
        let _ = webview.load_url(WHATSAPP_URL);
    }

    if start_hidden {
        set_low_memory(&webview, true);
    }

    let mut badge = platform::TaskbarBadge::new();
    let mut unread = 0u32;
    let mut focused = !start_hidden;

    let show_window = |w: &Window, wv: &WebView| {
        set_low_memory(wv, false);
        w.set_visible(true);
        w.set_minimized(false);
        w.set_focus();
    };
    let hide_state_file = state_file.clone();
    let hide_window = move |w: &Window, wv: &WebView| {
        save_window_state(&hide_state_file, w);
        w.set_visible(false);
        set_low_memory(wv, true);
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            // The close button hides to the tray, so notifications keep arriving.
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                hide_window(&window, &webview);
            }
            Event::WindowEvent { event: WindowEvent::Focused(f), .. } => {
                focused = f;
                #[cfg(target_os = "linux")]
                if f {
                    set_low_memory(&webview, false);
                }
            }
            Event::UserEvent(UserEvent::Show) => show_window(&window, &webview),
            Event::UserEvent(UserEvent::ToastClicked(tag)) => {
                show_window(&window, &webview);
                toast::clicked(&tag);
            }
            Event::UserEvent(UserEvent::Title(title)) => {
                let count = unread_from_title(&title);
                if count != unread {
                    let label = if count > 0 {
                        format!("({count}) {APP_NAME}")
                    } else {
                        APP_NAME.to_string()
                    };
                    window.set_title(&label);
                    if count > unread && !focused && window.is_visible() {
                        window.request_user_attention(Some(UserAttentionType::Informational));
                    }
                    #[cfg(target_os = "windows")]
                    badge.set(window.hwnd() as isize, count);
                    #[cfg(target_os = "linux")]
                    badge.set(count);

                    if let Some(t) = &tray {
                        let rgba = badge::tray_icon_rgba(ICON_RGBA, ICON_SIZE, count);
                        t.set_icon(rgba, ICON_SIZE);
                        t.set_tooltip(&label);
                    }
                    unread = count;
                }
            }
            Event::UserEvent(UserEvent::Tray(cmd)) => match cmd {
                TrayCommand::Toggle => {
                    if window.is_visible() && !window.is_minimized() {
                        hide_window(&window, &webview);
                    } else {
                        show_window(&window, &webview);
                    }
                }
                TrayCommand::Open => show_window(&window, &webview),
                TrayCommand::Reload => {
                    let _ = webview.reload();
                }
                TrayCommand::SetAutostart(enabled) => platform::set_autostart(enabled),
                TrayCommand::Quit => {
                    save_window_state(&state_file, &window);
                    *control_flow = ControlFlow::Exit;
                }
            },
            _ => {}
        }
    });
}
