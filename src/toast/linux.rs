//! Native Linux desktop notifications via FreeDesktop D-Bus
//! (`org.freedesktop.Notifications`).
//!
//! WhatsApp Web notifications are intercepted from WebKitGTK and re-sent as
//! native desktop notifications. Clicking a notification brings LiteChat to the
//! front and opens that chat; reading the chat dismisses the notification.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use glib::Variant;
use webkit2gtk::{Notification, NotificationExt, WebView, WebViewExt};

const NOTIF_BUS: &str = "org.freedesktop.Notifications";
const NOTIF_PATH: &str = "/org/freedesktop/Notifications";
const NOTIF_IFACE: &str = "org.freedesktop.Notifications";

static ICON_PATH: OnceLock<String> = OnceLock::new();

thread_local! {
    /// Notifications currently on screen, by FreeDesktop notification ID (UI thread only).
    static ACTIVE: RefCell<HashMap<u32, Notification>> = RefCell::new(HashMap::new());
}

/// Saves the app icon so desktop notification daemons can display it.
pub fn register(icon_png: &[u8], data_dir: &Path) {
    let icon_path = data_dir.join("notification-icon.png");
    if std::fs::write(&icon_path, icon_png).is_ok() {
        let _ = ICON_PATH.set(icon_path.to_string_lossy().into_owned());
    }
}

fn build_notify_params(title: &str, body: &str, suppress_sound: bool) -> Variant {
    let icon = ICON_PATH.get().map(String::as_str).unwrap_or("litechat");
    let actions: Vec<&str> = vec!["default", "Open"];
    let hints = glib::VariantDict::new(None);
    hints.insert("desktop-entry", "litechat");
    // Suppress the desktop sound only when WhatsApp Web can play its own chime.
    hints.insert("suppress-sound", suppress_sound);
    hints.insert("category", "im.received");

    (
        "LiteChat",
        0u32,
        icon,
        title,
        body,
        actions,
        hints.end(),
        -1i32,
    )
        .into()
}

fn send_notification(bus: &gio::DBusConnection, title: &str, body: &str) -> Option<u32> {
    let suppress_sound = crate::platform::has_gstreamer_audio_sink();
    let params = build_notify_params(title, body, suppress_sound);

    let reply = bus
        .call_sync(
            Some(NOTIF_BUS),
            NOTIF_PATH,
            NOTIF_IFACE,
            "Notify",
            Some(&params),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .ok()?;

    reply.child_value(0).get::<u32>()
}

fn close_notification(bus: &gio::DBusConnection, id: u32) {
    let params: Variant = (id,).into();
    let _ = bus.call_sync(
        Some(NOTIF_BUS),
        NOTIF_PATH,
        NOTIF_IFACE,
        "CloseNotification",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    );
}

/// Takes over WhatsApp Web notifications and shows them via `org.freedesktop.Notifications`.
/// `on_click` is called with the notification ID; forward it to the UI thread and call [`clicked`].
pub fn hook(webview: &WebView, on_click: impl Fn(String) + Send + Sync + Clone + 'static) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };

    // Listen for ActionInvoked (user clicked the notification).
    let click_cb = on_click.clone();
    let _sub_action = bus.signal_subscribe(
        Some(NOTIF_BUS),
        Some(NOTIF_IFACE),
        Some("ActionInvoked"),
        Some(NOTIF_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_conn, _sender, _path, _iface, _signal, params| {
            if let Some((id, _action)) = params.get::<(u32, String)>() {
                click_cb(id.to_string());
            }
        },
    );

    // Listen for NotificationClosed to drop stale entries.
    let _sub_closed = bus.signal_subscribe(
        Some(NOTIF_BUS),
        Some(NOTIF_IFACE),
        Some("NotificationClosed"),
        Some(NOTIF_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_conn, _sender, _path, _iface, _signal, params| {
            if let Some((id, _reason)) = params.get::<(u32, u32)>() {
                ACTIVE.with(|a| {
                    a.borrow_mut().remove(&id);
                });
            }
        },
    );

    // Keep subscriptions alive for the lifetime of the app by leaking their IDs
    // (gio::SignalSubscriptionId drops without unsubscribing, so nothing extra needed).
    let _ = (_sub_action, _sub_closed);

    webview.connect_show_notification(move |_wv, notification| {
        let title = notification.title().map(|s| s.to_string()).unwrap_or_default();
        let body = notification.body().map(|s| s.to_string()).unwrap_or_default();
        let Some(id) = send_notification(&bus, &title, &body) else {
            return false; // fall back to WebKitGTK's default behavior
        };

        let close_bus = bus.clone();
        notification.connect_closed(move |_| {
            close_notification(&close_bus, id);
            ACTIVE.with(|a| {
                a.borrow_mut().remove(&id);
            });
        });

        ACTIVE.with(|a| {
            a.borrow_mut().insert(id, notification.clone());
        });
        true
    });
}

/// Tells WhatsApp Web the notification was clicked, so it opens that chat.
/// Must run on the UI thread.
pub fn clicked(tag: &str) {
    let Ok(id) = tag.parse::<u32>() else { return };
    if let Some(n) = ACTIVE.with(|a| a.borrow_mut().remove(&id)) {
        n.clicked();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_variant_signature_is_valid_dbus_asv() {
        let params = build_notify_params("Alice", "Hello", true);
        assert_eq!(params.type_().as_str(), "(susssasa{sv}i)");
    }
}
