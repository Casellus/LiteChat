//! Cross-platform system tray abstraction.
//! - Linux: StatusNotifierItem over D-Bus via `ksni` (works natively on KDE Plasma,
//!   GNOME AppIndicator, Waybar, XFCE, Hyprland, etc., with left-click toggle).
//! - Windows: `tray-icon` crate.

#[derive(Debug, Clone)]
pub enum TrayCommand {
    Toggle,
    Open,
    Reload,
    SetAutostart(bool),
    Quit,
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::Arc;

    use ksni::blocking::{Handle, TrayMethods};
    use ksni::{Icon, MenuItem, ToolTip, Tray};

    use super::TrayCommand;

    fn rgba_to_argb(rgba: &[u8]) -> Vec<u8> {
        let mut argb = Vec::with_capacity(rgba.len());
        for px in rgba.as_chunks::<4>().0 {
            argb.push(px[3]); // A
            argb.push(px[0]); // R
            argb.push(px[1]); // G
            argb.push(px[2]); // B
        }
        argb
    }

    struct LiteChatTray {
        app_name: String,
        icon_argb: Vec<u8>,
        icon_size: i32,
        tooltip: String,
        autostart: bool,
        on_cmd: Arc<dyn Fn(TrayCommand) + Send + Sync>,
    }

    impl Tray for LiteChatTray {
        fn id(&self) -> String {
            "litechat".into()
        }

        fn title(&self) -> String {
            self.tooltip.clone()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            vec![Icon {
                width: self.icon_size,
                height: self.icon_size,
                data: self.icon_argb.clone(),
            }]
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip {
                title: self.tooltip.clone(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on_cmd)(TrayCommand::Toggle);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: format!("Open {}", self.app_name),
                    activate: Box::new(|this: &mut Self| (this.on_cmd)(TrayCommand::Open)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Reload".into(),
                    activate: Box::new(|this: &mut Self| (this.on_cmd)(TrayCommand::Reload)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                CheckmarkItem {
                    label: "Start on login".into(),
                    checked: self.autostart,
                    activate: Box::new(|this: &mut Self| {
                        this.autostart = !this.autostart;
                        (this.on_cmd)(TrayCommand::SetAutostart(this.autostart));
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|this: &mut Self| (this.on_cmd)(TrayCommand::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub struct TrayHandle {
        handle: Handle<LiteChatTray>,
    }

    impl TrayHandle {
        pub fn new(
            app_name: &str,
            icon_rgba: &[u8],
            icon_size: u32,
            autostart: bool,
            on_cmd: impl Fn(TrayCommand) + Send + Sync + 'static,
        ) -> Option<Self> {
            let tray = LiteChatTray {
                app_name: app_name.to_string(),
                icon_argb: rgba_to_argb(icon_rgba),
                icon_size: icon_size as i32,
                tooltip: app_name.to_string(),
                autostart,
                on_cmd: Arc::new(on_cmd),
            };
            tray.spawn().ok().map(|handle| Self { handle })
        }

        pub fn set_icon(&self, rgba: Vec<u8>, size: u32) {
            let argb = rgba_to_argb(&rgba);
            self.handle.update(move |tray| {
                tray.icon_argb = argb;
                tray.icon_size = size as i32;
            });
        }

        pub fn set_tooltip(&self, label: &str) {
            let text = label.to_string();
            self.handle.update(move |tray| {
                tray.tooltip = text;
            });
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::Arc;

    use tray_icon::{
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::TrayCommand;

    pub struct TrayHandle {
        tray: TrayIcon,
    }

    impl TrayHandle {
        pub fn new(
            app_name: &str,
            icon_rgba: &[u8],
            icon_size: u32,
            autostart: bool,
            on_cmd: impl Fn(TrayCommand) + Send + Sync + 'static,
        ) -> Option<Self> {
            let on_cmd = Arc::new(on_cmd);
            let tray_menu = Menu::new();
            let item_show = MenuItem::new(format!("Open {app_name}"), true, None);
            let item_reload = MenuItem::new("Reload", true, None);
            let item_autostart = CheckMenuItem::new("Start with Windows", true, autostart, None);
            let item_quit = MenuItem::new("Quit", true, None);
            let _ = tray_menu.append_items(&[
                &item_show,
                &item_reload,
                &PredefinedMenuItem::separator(),
                &item_autostart,
                &PredefinedMenuItem::separator(),
                &item_quit,
            ]);

            let id_show = item_show.id().clone();
            let id_reload = item_reload.id().clone();
            let id_autostart = item_autostart.id().clone();
            let id_quit = item_quit.id().clone();

            let cb_tray = on_cmd.clone();
            TrayIconEvent::set_event_handler(Some(move |e| {
                if matches!(
                    e,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    cb_tray(TrayCommand::Toggle);
                }
            }));

            let cb_menu = on_cmd;
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if e.id == id_show {
                    cb_menu(TrayCommand::Open);
                } else if e.id == id_reload {
                    cb_menu(TrayCommand::Reload);
                } else if e.id == id_autostart {
                    cb_menu(TrayCommand::SetAutostart(item_autostart.is_checked()));
                } else if e.id == id_quit {
                    cb_menu(TrayCommand::Quit);
                }
            }));

            let icon = tray_icon::Icon::from_rgba(icon_rgba.to_vec(), icon_size, icon_size).ok()?;
            let tray = TrayIconBuilder::new()
                .with_tooltip(app_name)
                .with_icon(icon)
                .with_menu(Box::new(tray_menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;
            Some(Self { tray })
        }

        pub fn set_icon(&self, rgba: Vec<u8>, size: u32) {
            if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, size, size) {
                let _ = self.tray.set_icon(Some(icon));
            }
        }

        pub fn set_tooltip(&self, label: &str) {
            let _ = self.tray.set_tooltip(Some(label));
        }
    }
}

pub use imp::TrayHandle;
