//! Tray icon (StatusNotifierItem, aka AppIndicator)

use std::sync::Arc;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};

use crate::events::{send, Proxy, UserEvent};
use crate::i18n::Translations;
use crate::icons::RgbaIcon;
use crate::util;

pub struct Tray {
    proxy: Proxy,
    t: Translations,
    pub icon: Option<Arc<RgbaIcon>>,
    pub quit_on_close: bool,
    pub autostart: bool,
    pub autostart_supported: bool,
}

impl Tray {
    fn item(&self, key: &str, default: &str, event: fn() -> UserEvent) -> MenuItem<Self> {
        StandardItem {
            label: self.t.get(key, default),
            activate: Box::new(move |tray: &mut Self| send(&tray.proxy, event())),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        util::APP_NAME.to_owned()
    }

    fn title(&self) -> String {
        util::APP_NAME.to_owned()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: util::APP_NAME.to_owned(),
            ..Default::default()
        }
    }

    fn icon_name(&self) -> String {
        if self.icon.is_some() {
            String::new()
        } else {
            util::APP_NAME.to_owned()
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        match &self.icon {
            Some(icon) => vec![ksni::Icon {
                width: icon.width,
                height: icon.height,
                data: icon.argb.clone(),
            }],
            None => vec![],
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        send(&self.proxy, UserEvent::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            self.item("show_hide", "Show/Hide", || UserEvent::Toggle),
            self.item("reload", "Reload", || UserEvent::Reload),
            self.item("clear_cache_menu", "Clear cache…", || {
                UserEvent::ClearCache
            }),
            MenuItem::Separator,
            CheckmarkItem {
                label: self.t.get("quit_on_close", "Quit on close"),
                checked: self.quit_on_close,
                activate: Box::new(|tray: &mut Self| {
                    tray.quit_on_close = !tray.quit_on_close;
                    send(&tray.proxy, UserEvent::SetQuitOnClose(tray.quit_on_close));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: self.t.get("autostart", "Start on login"),
                enabled: self.autostart_supported,
                checked: self.autostart,
                // the main thread writes the entry, then updates `autostart` with the result
                activate: Box::new(|tray: &mut Self| {
                    send(&tray.proxy, UserEvent::SetAutostart(!tray.autostart))
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            self.item("about", "About", || UserEvent::About),
            self.item("quit", "Quit", || UserEvent::Quit),
        ]
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        log::warn!("tray: no StatusNotifierWatcher ({reason:?}), the tray icon is not shown");
        true
    }
}

pub fn spawn(
    proxy: Proxy,
    t: Translations,
    icon: Option<Arc<RgbaIcon>>,
    quit_on_close: bool,
    autostart: bool,
    autostart_supported: bool,
) -> Option<Handle<Tray>> {
    let tray = Tray {
        proxy,
        t,
        icon,
        quit_on_close,
        autostart,
        autostart_supported,
    };
    // a sandbox can't own the StatusNotifierItem-PID-ID name
    let sandboxed = util::flatpak_id().is_some();
    // the panel may start after us (autostart): keep waiting for a StatusNotifierWatcher
    match tray
        .disable_dbus_name(sandboxed)
        .assume_sni_available(true)
        .spawn()
    {
        Ok(handle) => Some(handle),
        Err(err) => {
            log::error!("tray: {err}");
            None
        }
    }
}
