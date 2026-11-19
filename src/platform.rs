//! Windows-specific integrations: single instance, autostart, taskbar badge,
//! and WebView2 hooks (permissions, context menu).

use std::ffi::c_void;

use webview2_com::{
    take_pwstr, ContextMenuRequestedEventHandler, PermissionRequestedEventHandler,
    SetPermissionStateCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2Profile4, ICoreWebView2_11, ICoreWebView2_13,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR, COREWEBVIEW2_PERMISSION_KIND,
        COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    },
};
use windows::{
    core::{w, Interface, HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE, HWND},
        Graphics::Gdi::{CreateBitmap, DeleteObject},
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            Registry::{
                RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
                RRF_RT_REG_SZ,
            },
            Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE},
        },
        UI::{
            Shell::{ITaskbarList3, TaskbarList},
            WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO},
        },
    },
};

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

/// Returns `false` if another LiteChat is already running (it has been asked to
/// show its window, so this process should exit). Otherwise calls `on_show`
/// from a background thread every time a second instance is launched.
pub fn single_instance(on_show: impl Fn() + Send + 'static) -> bool {
    unsafe {
        let Ok(event) = CreateEventW(None, false, false, w!("Local\\LiteChat.ShowWindow")) else {
            return true; // cannot tell: just run
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = SetEvent(event);
            return false;
        }
        let raw = event.0 as usize;
        std::thread::spawn(move || {
            let event = HANDLE(raw as *mut c_void);
            loop {
                WaitForSingleObject(event, INFINITE);
                on_show();
            }
        });
        true
    }
}

// ---------------------------------------------------------------------------
// Start with Windows (HKCU\...\Run)
// ---------------------------------------------------------------------------

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("LiteChat");

pub fn autostart_enabled() -> bool {
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None)
            == ERROR_SUCCESS
    }
}

pub fn set_autostart(enabled: bool) {
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd = format!("\"{}\" --hidden", exe.display());
            let wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(wide.as_ptr() as *const c_void),
                (wide.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

// ---------------------------------------------------------------------------
// Unread badge (taskbar overlay + tray icon)
// ---------------------------------------------------------------------------

const BADGE_RED: [u8; 3] = [0xE5, 0x39, 0x35];

/// 3x5 pixel font for the badge digits.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        _ => [0b000, 0b010, 0b111, 0b010, 0b000], // '+'
    }
}

fn put(px: &mut [u8], size: u32, x: i32, y: i32, rgb: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 || a <= 0.0 {
        return;
    }
    let i = ((y as u32 * size + x as u32) * 4) as usize;
    let a = a.min(1.0);
    let base_a = px[i + 3] as f32 / 255.0;
    let out_a = a + base_a * (1.0 - a);
    for c in 0..3 {
        let src = rgb[c] as f32;
        let dst = px[i + c] as f32;
        px[i + c] = ((src * a + dst * base_a * (1.0 - a)) / out_a.max(1e-6)) as u8;
    }
    px[i + 3] = (out_a * 255.0) as u8;
}

/// Draws a red circle (anti-aliased) and, if `scale > 0`, the count in white.
fn draw_badge(px: &mut [u8], size: u32, cx: f32, cy: f32, r: f32, count: u32, scale: i32) {
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            put(px, size, x, y, BADGE_RED, r + 0.5 - d);
        }
    }
    if scale <= 0 {
        return;
    }
    let text = if count > 9 { "9+".to_string() } else { count.to_string() };
    let chars: Vec<char> = text.chars().collect();
    let w = chars.len() as i32 * 3 * scale + (chars.len() as i32 - 1) * scale;
    let h = 5 * scale;
    let mut ox = (cx - w as f32 / 2.0).round() as i32;
    let oy = (cy - h as f32 / 2.0).round() as i32;
    for c in chars {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(px, size, ox + col * scale + dx, oy + row as i32 * scale + dy, [255; 3], 1.0);
                        }
                    }
                }
            }
        }
        ox += 4 * scale;
    }
}

/// Tray icon with a small red dot in the top-right corner when there are unread chats.
pub fn tray_icon_rgba(base: &[u8], size: u32, unread: u32) -> Vec<u8> {
    let mut px = base.to_vec();
    if unread > 0 {
        let s = size as f32;
        draw_badge(&mut px, size, s * 0.78, s * 0.22, s * 0.22, unread, 0);
    }
    px
}

fn hicon_from_rgba(rgba: &[u8], size: u32) -> Option<HICON> {
    // GDI wants BGRA, top-down rows.
    let bgra: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mask = vec![0u8; (size * size / 8) as usize];
    unsafe {
        let color = CreateBitmap(size as i32, size as i32, 1, 32, Some(bgra.as_ptr() as *const c_void));
        let mask_bmp = CreateBitmap(size as i32, size as i32, 1, 1, Some(mask.as_ptr() as *const c_void));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask_bmp, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask_bmp.into());
        icon
    }
}

/// Number badge on the taskbar button (like the official app).
pub struct TaskbarBadge {
    list: Option<ITaskbarList3>,
    icon: Option<HICON>,
}

impl TaskbarBadge {
    pub fn new() -> Self {
        let list: Option<ITaskbarList3> =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok() };
        if let Some(l) = &list {
            unsafe {
                let _ = l.HrInit();
            }
        }
        Self { list, icon: None }
    }

    pub fn set(&mut self, hwnd: isize, unread: u32) {
        let Some(list) = &self.list else { return };
        let hwnd = HWND(hwnd as *mut c_void);
        let new_icon = if unread > 0 {
            const S: u32 = 32;
            let mut px = vec![0u8; (S * S * 4) as usize];
            let scale = if unread > 9 { 3 } else { 4 };
            draw_badge(&mut px, S, 16.0, 16.0, 15.5, unread, scale);
            hicon_from_rgba(&px, S)
        } else {
            None
        };
        unsafe {
            let label = HSTRING::from(format!("{unread} unread chats"));
            let _ = list.SetOverlayIcon(hwnd, new_icon.unwrap_or_default(), &label);
            if let Some(old) = self.icon.take() {
                let _ = DestroyIcon(old);
            }
        }
        self.icon = new_icon;
    }
}

// ---------------------------------------------------------------------------
// WebView2 hooks
// ---------------------------------------------------------------------------

fn is_whatsapp(uri: &str) -> bool {
    let host = uri
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', ':', '?', '#']).next())
        .unwrap_or("");
    host == "whatsapp.com" || host.ends_with(".whatsapp.com")
}

/// Grants notifications, microphone, camera and clipboard to WhatsApp only,
/// so calls and voice messages work without extra prompts (like the official app).
pub fn auto_grant_permissions(webview: &ICoreWebView2) {
    let handler = PermissionRequestedEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            let uri = take_pwstr(uri);
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            let allowed = [
                COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                COREWEBVIEW2_PERMISSION_KIND_CAMERA,
                COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
            ];
            if is_whatsapp(&uri) && allowed.contains(&kind) {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = webview.add_PermissionRequested(&handler, &mut token);
    }

    // A "block" saved earlier in the profile would stop the event above from
    // ever firing, so store "allow" for WhatsApp explicitly as well.
    let profile = webview
        .cast::<ICoreWebView2_13>()
        .and_then(|wv| unsafe { wv.Profile() })
        .and_then(|p| p.cast::<ICoreWebView2Profile4>());
    if let Ok(profile) = profile {
        for kind in [
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        ] {
            unsafe {
                let _ = profile.SetPermissionState(
                    kind,
                    w!("https://web.whatsapp.com"),
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                    &SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(()))),
                );
            }
        }
    }
}

/// Browser-only entries removed from the right-click menu. Copy/paste,
/// spell-check, emoji, image and link actions are kept.
const HIDDEN_MENU_ITEMS: &[&str] = &[
    "back", "forward", "reload", "saveAs", "print", "createQrCode", "inspectElement",
    "webCapture", "share", "sendTabToSelf", "readAloud", "translate", "other",
];

pub fn clean_context_menu(webview: &ICoreWebView2) {
    let Ok(wv11) = webview.cast::<ICoreWebView2_11>() else { return };
    let handler = ContextMenuRequestedEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let items = args.MenuItems()?;
            let mut count = 0u32;
            items.Count(&mut count)?;
            for i in (0..count).rev() {
                let item = items.GetValueAtIndex(i)?;
                let mut name = PWSTR::null();
                item.Name(&mut name)?;
                if HIDDEN_MENU_ITEMS.contains(&take_pwstr(name).as_str()) {
                    items.RemoveValueAtIndex(i)?;
                }
            }
            // Tidy up separators left at the edges or doubled.
            let mut prev_sep = true;
            let mut i = 0u32;
            items.Count(&mut count)?;
            while i < count {
                let item = items.GetValueAtIndex(i)?;
                let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
                item.Kind(&mut kind)?;
                let is_sep = kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR;
                if is_sep && (prev_sep || i + 1 == count) {
                    items.RemoveValueAtIndex(i)?;
                    count -= 1;
                    continue;
                }
                prev_sep = is_sep;
                i += 1;
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = wv11.add_ContextMenuRequested(&handler, &mut token);
    }
}
