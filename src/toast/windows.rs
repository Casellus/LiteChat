//! Native Windows toast notifications.
//!
//! WhatsApp Web notifications are intercepted from WebView2 and re-sent as real
//! Windows toasts. Unlike WebView2's own popups, toasts go through the Windows
//! notification system: they show up in the Action Center, respect Focus /
//! Do Not Disturb, and can be picked up by notification widgets
//! (e.g. "dynamic island" apps).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use webview2_com::{
    take_pwstr, NotificationCloseRequestedEventHandler, NotificationReceivedEventHandler,
    Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Notification, ICoreWebView2_24},
};
use windows::{
    core::{Interface, HSTRING, PCWSTR, PWSTR},
    Data::Xml::Dom::XmlDocument,
    Foundation::TypedEventHandler,
    Win32::{
        System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ},
        UI::Shell::SetCurrentProcessExplicitAppUserModelID,
    },
    UI::Notifications::{ToastNotification, ToastNotificationManager},
};

pub const AUMID: &str = "LiteChat.Desktop";
const GROUP: &str = "litechat";

thread_local! {
    /// Notifications currently on screen, by toast tag (UI thread only).
    static ACTIVE: RefCell<HashMap<String, ICoreWebView2Notification>> = RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
}

/// Registers LiteChat as a notification sender (name + icon) for the current
/// user and tags this process with it. No installer or admin rights needed.
pub fn register(icon_png: &[u8], data_dir: &Path) {
    let icon_path = data_dir.join("notification-icon.png");
    let _ = std::fs::write(&icon_path, icon_png);
    let key = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
    let set = |name: &str, value: &str| {
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(HSTRING::from(name).as_ptr()),
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            );
        }
    };
    set("DisplayName", "LiteChat");
    set("IconUri", &icon_path.to_string_lossy());
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID));
    }
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn read(f: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut p = PWSTR::null();
    if f(&mut p).is_ok() { take_pwstr(p) } else { String::new() }
}

fn show_toast(title: &str, body: &str, tag: &str, on_click: impl Fn(String) + Send + 'static) -> windows::core::Result<()> {
    // WhatsApp Web plays its own sound, so the toast is silent to avoid a double ding.
    let xml = format!(
        "<toast launch=\"{tag}\"><visual><binding template=\"ToastGeneric\">\
         <text hint-maxLines=\"1\">{}</text><text>{}</text>\
         </binding></visual><audio silent=\"true\"/></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from(tag))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;
    let tag = tag.to_string();
    toast.Activated(&TypedEventHandler::new(move |_, _| {
        on_click(tag.clone());
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

fn remove_toast(tag: &str) {
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(&HSTRING::from(tag), &HSTRING::from(GROUP), &HSTRING::from(AUMID));
    }
}

/// Takes over WhatsApp Web notifications and shows them as Windows toasts.
/// `on_click` runs on a background thread with the toast tag; forward it to the
/// UI thread and call [`clicked`] there.
pub fn hook(webview: &ICoreWebView2, on_click: impl Fn(String) + Send + Sync + Clone + 'static) {
    let Ok(wv24) = webview.cast::<ICoreWebView2_24>() else { return };
    let handler = NotificationReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let notification = args.Notification()?;
            let title = read(|p| notification.Title(p));
            let body = read(|p| notification.Body(p));
            let tag = NEXT_ID.with(|n| {
                let mut n = n.borrow_mut();
                *n += 1;
                format!("n{}", *n)
            });
            if show_toast(&title, &body, &tag, on_click.clone()).is_err() {
                return Ok(()); // fall back to WebView2's default popup
            }
            args.SetHandled(true)?;
            let _ = notification.ReportShown();

            // WhatsApp closes the notification once the chat is read: drop the toast too.
            let close_tag = tag.clone();
            let mut token = 0i64;
            let _ = notification.add_CloseRequested(
                &NotificationCloseRequestedEventHandler::create(Box::new(move |_, _| {
                    remove_toast(&close_tag);
                    ACTIVE.with(|a| a.borrow_mut().remove(&close_tag));
                    Ok(())
                })),
                &mut token,
            );
            ACTIVE.with(|a| a.borrow_mut().insert(tag, notification));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = wv24.add_NotificationReceived(&handler, &mut token);
    }
}

/// Tells WhatsApp Web the notification was clicked, so it opens that chat.
/// Must run on the UI thread.
pub fn clicked(tag: &str) {
    if let Some(n) = ACTIVE.with(|a| a.borrow_mut().remove(tag)) {
        unsafe {
            let _ = n.ReportClicked();
        }
    }
}
