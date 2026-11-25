use std::path::PathBuf;
use std::sync::Arc;

use crate::icons::RgbaIcon;

/// Everything that is sent to the main (GTK) thread from other threads:
/// D-Bus calls, tray menu, notifications, the page, background downloads.
#[derive(Debug)]
pub enum UserEvent {
    Show {
        activation_token: Option<String>,
    },
    Hide,
    Toggle,
    Quit,
    /// whatsapp: link opened while running
    OpenUrl(String),
    /// message posted by the page (`web/bridge.js`)
    Ipc(String),
    PageLoaded,
    Title(String),
    NotificationClicked {
        js_id: Option<u64>,
        activation_token: Option<String>,
    },
    NotificationClosed {
        js_id: u64,
    },
    Favicon {
        url: String,
        icon: Option<Arc<RgbaIcon>>,
    },
    Css(String),
    Reload,
    ClearCache,
    CacheCleared {
        size_before: u64,
    },
    SetQuitOnClose(bool),
    SetAutostart(bool),
    About,
    DownloadFinished {
        path: Option<PathBuf>,
        success: bool,
    },
}

pub type Proxy = tao::event_loop::EventLoopProxy<UserEvent>;

pub fn send(proxy: &Proxy, event: UserEvent) {
    if proxy.send_event(event).is_err() {
        log::debug!("event loop closed");
    }
}
